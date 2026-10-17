//! Files embedded in a PDF: the document-level ones in the catalog's
//! `/EmbeddedFiles` name tree and those on file-attachment annotations.
//! Listed when a file opens; bytes are read on demand.

use std::collections::HashSet;

use anyhow::{Context, Result, bail};
use lopdf::{Dictionary, Document as Pdf, LazyPdf, Object, ObjectId};

use crate::Source;
use crate::assets::MarkAssets;
use crate::file::pdf_text;

/// Where an attachment lives, stable across reloads of the same file.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AttachmentRef {
    /// A key in `/Names /EmbeddedFiles`.
    Named(String),
    /// The `n`th file-attachment annotation on page `page` (file order).
    Annot { page: u32, n: u32 },
    /// One of sheaf's, by the stem its mark names its file by, which the
    /// document's [`MarkAssets`] find: saved or not yet.
    Cached(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Attachment {
    pub at: AttachmentRef,
    /// The file name (`/UF`, else `/F`), without any folder.
    pub name: String,
    pub description: String,
    /// Uncompressed size, when the file says (`/Params /Size`).
    pub size: Option<u64>,
    /// `/Params /ModDate` and `/CreationDate`, PDF date strings.
    pub modified: String,
    pub created: String,
    /// Portfolio folder path ("Tax/2024"), empty at the top level.
    pub folder: String,
    /// Portfolio metadata (`/CI`) by schema field key, as text.
    pub fields: Vec<(String, String)>,
}

impl Attachment {
    pub fn is_pdf(&self) -> bool {
        self.name.to_ascii_lowercase().ends_with(".pdf")
    }
}

/// Every attachment in the file: named ones in name-tree order, then
/// annotation ones by page. `pages` are the page objects, by index.
pub fn attachments_in<P: Objects + ?Sized>(
    pdf: &P,
    pages: impl IntoIterator<Item = ObjectId>,
) -> Vec<Attachment> {
    let mut out = Vec::new();
    let folders = crate::portfolio::folder_paths(pdf);
    for (key, spec) in name_tree(pdf, b"EmbeddedFiles") {
        if let Some(d) = resolve(pdf, spec).and_then(|o| o.as_dict().ok()) {
            let mut a = describe(pdf, AttachmentRef::Named(key.clone()), d);
            // Portfolio files are keyed "<folder id>name".
            if let Some((id, rest)) = folder_key(&key)
                && let Some(path) = folders.get(&id)
            {
                a.folder = path.clone();
                if a.name.is_empty() {
                    a.name = rest.to_string();
                }
            }
            if a.name.is_empty() {
                a.name = key;
            }
            out.push(a);
        }
    }
    for (page_no, page_id) in pages.into_iter().enumerate() {
        let mut n = 0;
        let entries = page_entries(pdf, page_id);
        let page_annots = crate::file::PageAnnots::new(entries);
        for entry in entries {
            let Some(annot) = resolve(pdf, entry).and_then(|o| o.as_dict().ok()) else {
                continue;
            };
            if annot.get(b"Subtype").and_then(Object::as_name).ok() != Some(b"FileAttachment") {
                continue;
            }
            let at = AttachmentRef::Annot {
                page: page_no as u32,
                n,
            };
            n += 1;
            // sheaf's own are listed from the document (`Cached`); one
            // another tool has edited since is that tool's.
            if crate::file::is_ours(pdf, entry, &page_annots) {
                continue;
            }
            let Some(fs) = annot
                .get(b"FS")
                .ok()
                .and_then(|o| resolve(pdf, o))
                .and_then(|o| o.as_dict().ok())
            else {
                continue;
            };
            let mut a = describe(pdf, at, fs);
            if a.description.is_empty() {
                a.description = annot.get(b"Contents").map(pdf_text).unwrap_or_default();
            }
            out.push(a);
        }
    }
    out
}

/// The bytes of attachment `at` in `src`, its pages numbered as its page
/// table numbers them, as they're listed; one of sheaf's from `assets`,
/// the document's.
pub fn read_attachment(src: &Source, assets: &MarkAssets, at: &AttachmentRef) -> Result<Vec<u8>> {
    attachment_bytes(src.pdf(), src.page_ids(), assets, at)
}

/// The bytes of attachment `at` in a file whose page objects, by index,
/// are `pages`, in a document whose marks' files are `assets`.
pub fn attachment_bytes<P: Objects + ?Sized>(
    pdf: &P,
    pages: impl IntoIterator<Item = ObjectId>,
    assets: &MarkAssets,
    at: &AttachmentRef,
) -> Result<Vec<u8>> {
    if let AttachmentRef::Cached(stem) = at {
        return Ok(assets.file_bytes(stem)?.to_vec());
    }
    let spec = match at {
        AttachmentRef::Named(key) => name_tree(pdf, b"EmbeddedFiles")
            .into_iter()
            .find(|(k, _)| k == key)
            .and_then(|(_, o)| resolve(pdf, o))
            .and_then(|o| o.as_dict().ok())
            .with_context(|| format!("no embedded file named {key:?}"))?,
        AttachmentRef::Cached(_) => unreachable!("read from the document's files above"),
        AttachmentRef::Annot { page, n } => {
            let page_id = pages
                .into_iter()
                .nth(*page as usize)
                .with_context(|| format!("no page {}", page + 1))?;
            page_annots(pdf, page_id)
                .into_iter()
                .filter(|a| {
                    a.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"FileAttachment")
                })
                .nth(*n as usize)
                .and_then(|a| a.get(b"FS").ok())
                .and_then(|o| resolve(pdf, o))
                .and_then(|o| o.as_dict().ok())
                .context("attachment annotation missing")?
        }
    };
    let ef = spec
        .get(b"EF")
        .ok()
        .and_then(|o| resolve(pdf, o))
        .and_then(|o| o.as_dict().ok())
        .context("the attachment has no embedded file (it may be a link to an outside file)")?;
    let stream = [b"UF".as_slice(), b"F"]
        .iter()
        .find_map(|k| ef.get(k).ok())
        .and_then(|o| resolve(pdf, o))
        .and_then(|o| o.as_stream().ok())
        .context("embedded file stream missing")?;
    file_bytes(stream)
}

/// An embedded file stream's bytes, decoded.
pub(crate) fn file_bytes(stream: &lopdf::Stream) -> Result<Vec<u8>> {
    if stream.dict.get(b"Filter").is_err() {
        return Ok(stream.content.clone());
    }
    match stream.decompressed_content() {
        Ok(b) => Ok(b),
        Err(e) => bail!("can't decode the embedded file: {e}"),
    }
}

/// A file specification's name, description and parameters.
fn describe<P: Objects + ?Sized>(pdf: &P, at: AttachmentRef, fs: &Dictionary) -> Attachment {
    let text = |d: &Dictionary, k: &[u8]| {
        d.get(k)
            .ok()
            .and_then(|o| resolve(pdf, o))
            .map(pdf_text)
            .unwrap_or_default()
    };
    let name = [b"UF".as_slice(), b"F"]
        .iter()
        .map(|k| text(fs, k))
        .find(|s| !s.is_empty())
        .unwrap_or_default();
    // Some writers put a path in /F; show the file's own name.
    let name = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_string();
    let params = fs
        .get(b"EF")
        .ok()
        .and_then(|o| resolve(pdf, o))
        .and_then(|o| o.as_dict().ok())
        .and_then(|ef| [b"UF".as_slice(), b"F"].iter().find_map(|k| ef.get(k).ok()))
        .and_then(|o| resolve(pdf, o))
        .and_then(|o| o.as_stream().ok())
        .and_then(|s| s.dict.get(b"Params").ok())
        .and_then(|o| resolve(pdf, o))
        .and_then(|o| o.as_dict().ok());
    let size = params
        .and_then(|p| p.get(b"Size").ok())
        .and_then(|o| resolve(pdf, o))
        .and_then(|o| o.as_i64().ok())
        .and_then(|n| u64::try_from(n).ok());
    let fields = fs
        .get(b"CI")
        .ok()
        .and_then(|o| resolve(pdf, o))
        .and_then(|o| o.as_dict().ok())
        .map(|ci| {
            ci.iter()
                .filter(|(k, _)| k.as_slice() != b"Type")
                .map(|(k, v)| {
                    (
                        String::from_utf8_lossy(k).into_owned(),
                        crate::portfolio::field_text(pdf, v),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    Attachment {
        at,
        name,
        description: text(fs, b"Desc"),
        size,
        modified: params.map(|p| text(p, b"ModDate")).unwrap_or_default(),
        created: params.map(|p| text(p, b"CreationDate")).unwrap_or_default(),
        folder: String::new(),
        fields,
    }
}

/// One of the catalog's `/Names` trees, flattened: (key, value) in tree
/// order. `/EmbeddedFiles` maps names to file specs, `/Dests` to
/// destinations. A node or array the tree reaches again is read once.
pub(crate) fn name_tree<'a, P: Objects + ?Sized>(
    pdf: &'a P,
    tree: &[u8],
) -> Vec<(String, &'a Object)> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let tree = pdf
        .catalog()
        .ok()
        .and_then(|c| c.get(b"Names").ok())
        .and_then(|o| resolve(pdf, o))
        .and_then(|o| o.as_dict().ok())
        .and_then(|n| n.get(tree).ok())
        .and_then(|o| once(pdf, &mut seen, o))
        .and_then(|o| o.as_dict().ok());
    if let Some(t) = tree {
        walk_names(pdf, t, &mut out, 0, &mut seen);
    }
    out
}

fn walk_names<'a, P: Objects + ?Sized>(
    pdf: &'a P,
    node: &'a Dictionary,
    out: &mut Vec<(String, &'a Object)>,
    depth: u8,
    seen: &mut HashSet<ObjectId>,
) {
    if depth > 32 {
        return;
    }
    if let Some(Object::Array(names)) = node.get(b"Names").ok().and_then(|o| once(pdf, seen, o)) {
        for pair in names.as_chunks::<2>().0 {
            if let Some(k) = resolve(pdf, &pair[0]).filter(|o| o.as_str().is_ok()) {
                out.push((pdf_text(k), &pair[1]));
            }
        }
    }
    if let Some(Object::Array(kids)) = node.get(b"Kids").ok().and_then(|o| once(pdf, seen, o)) {
        for kid in kids {
            if let Some(Ok(d)) = once(pdf, seen, kid).map(Object::as_dict) {
                walk_names(pdf, d, out, depth + 1, seen);
            }
        }
    }
}

fn page_annots<P: Objects + ?Sized>(pdf: &P, page: ObjectId) -> Vec<&Dictionary> {
    page_entries(pdf, page)
        .iter()
        .filter_map(|o| resolve(pdf, o).and_then(|o| o.as_dict().ok()))
        .collect()
}

/// A page's `/Annots`, as written.
fn page_entries<P: Objects + ?Sized>(pdf: &P, page: ObjectId) -> &[Object] {
    pdf.get_dictionary(page)
        .ok()
        .and_then(|p| p.get(b"Annots").ok())
        .and_then(|o| resolve(pdf, o))
        .and_then(|o| o.as_array().ok())
        .map_or(&[], Vec::as_slice)
}

/// A portfolio key "<12>Report.pdf" as (12, "Report.pdf").
fn folder_key(key: &str) -> Option<(i64, &str)> {
    let rest = key.strip_prefix('<')?;
    let (id, name) = rest.split_once('>')?;
    Some((id.parse().ok()?, name))
}

/// What objects are read from: a source's [`LazyPdf`], or an eager
/// document the old writers load, so the readers serve both. Its reads
/// are lopdf's, by their names, so a reader's body reads the same over
/// either. Transitional (M10.5): it goes with the eager documents in
/// slice 9.
pub trait Objects {
    /// Object `id`, its references followed.
    fn get_object(&self, id: ObjectId) -> lopdf::Result<&Object>;

    fn get_dictionary(&self, id: ObjectId) -> lopdf::Result<&Dictionary>;

    /// `object`, or what its chain of references ends at, with the last
    /// reference's id.
    fn dereference<'a>(
        &'a self,
        object: &'a Object,
    ) -> lopdf::Result<(Option<ObjectId>, &'a Object)>;

    fn catalog(&self) -> lopdf::Result<&Dictionary>;

    /// Object `id`, its references followed; None when it can't be read.
    fn object(&self, id: ObjectId) -> Option<&Object> {
        self.get_object(id).ok()
    }

    fn dictionary(&self, id: ObjectId) -> Option<&Dictionary> {
        self.object(id)?.as_dict().ok()
    }
}

impl Objects for Pdf {
    fn get_object(&self, id: ObjectId) -> lopdf::Result<&Object> {
        Pdf::get_object(self, id)
    }

    fn get_dictionary(&self, id: ObjectId) -> lopdf::Result<&Dictionary> {
        Pdf::get_dictionary(self, id)
    }

    fn dereference<'a>(
        &'a self,
        object: &'a Object,
    ) -> lopdf::Result<(Option<ObjectId>, &'a Object)> {
        Pdf::dereference(self, object)
    }

    fn catalog(&self) -> lopdf::Result<&Dictionary> {
        Pdf::catalog(self)
    }
}

impl Objects for LazyPdf {
    fn get_object(&self, id: ObjectId) -> lopdf::Result<&Object> {
        LazyPdf::get_object(self, id)
    }

    fn get_dictionary(&self, id: ObjectId) -> lopdf::Result<&Dictionary> {
        LazyPdf::get_dictionary(self, id)
    }

    fn dereference<'a>(
        &'a self,
        object: &'a Object,
    ) -> lopdf::Result<(Option<ObjectId>, &'a Object)> {
        LazyPdf::dereference(self, object)
    }

    fn catalog(&self) -> lopdf::Result<&Dictionary> {
        LazyPdf::catalog(self)
    }
}

pub(crate) fn resolve<'a, P: Objects + ?Sized>(pdf: &'a P, o: &'a Object) -> Option<&'a Object> {
    match o {
        Object::Reference(id) => pdf.object(*id),
        o => Some(o),
    }
}

/// `o` resolved, unless it's a reference `seen` already has: a tree's walk
/// reads each node it reaches by reference once, however often it's
/// listed, and so ends.
pub(crate) fn once<'a, P: Objects + ?Sized>(
    pdf: &'a P,
    seen: &mut HashSet<ObjectId>,
    o: &'a Object,
) -> Option<&'a Object> {
    if let Ok(id) = o.as_reference()
        && !seen.insert(id)
    {
        return None;
    }
    resolve(pdf, o)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use lopdf::{Stream, dictionary};

    /// A one-page file with a named attachment, a compressed one and a
    /// file-attachment annotation.
    pub(crate) fn sample() -> Pdf {
        let mut pdf = Pdf::with_version("1.7");
        let plain = pdf.add_object(Stream::new(
            dictionary! { "Type" => "EmbeddedFile", "Params" => dictionary! { "Size" => 5, "ModDate" => Object::string_literal("D:20240102030405Z") } },
            b"hello".to_vec(),
        ));
        let mut packed = Stream::new(
            dictionary! { "Type" => "EmbeddedFile" },
            b"%PDF-1.4 tiny".to_vec(),
        );
        packed.compress().unwrap();
        let packed = pdf.add_object(packed);
        let spec_a = pdf.add_object(dictionary! {
            "Type" => "Filespec",
            "F" => Object::string_literal("notes.txt"),
            "Desc" => Object::string_literal("Client notes"),
            "EF" => dictionary! { "F" => plain },
        });
        let spec_b = dictionary! {
            "Type" => "Filespec",
            "UF" => Object::string_literal("sub/dir/w2.pdf"),
            "EF" => dictionary! { "F" => packed },
        };
        let annot_stream = pdf.add_object(Stream::new(dictionary! {}, b"annot bytes".to_vec()));
        let annot = pdf.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "FileAttachment",
            "Rect" => vec![0.into(), 0.into(), 10.into(), 10.into()],
            "Contents" => Object::string_literal("Support"),
            "FS" => dictionary! { "Type" => "Filespec", "F" => Object::string_literal("support.xlsx"), "EF" => dictionary! { "F" => annot_stream } },
        });
        let pages_id = pdf.new_object_id();
        let page = pdf.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Annots" => vec![annot.into()],
        });
        pdf.objects.insert(
            pages_id,
            dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 }.into(),
        );
        let catalog = pdf.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
            "Names" => dictionary! {
                "EmbeddedFiles" => dictionary! {
                    "Kids" => vec![dictionary! {
                        "Names" => vec![
                            Object::string_literal("notes.txt"), spec_a.into(),
                            Object::string_literal("w2"), spec_b.into(),
                        ],
                    }.into()],
                },
            },
        });
        pdf.trailer.set("Root", catalog);
        pdf
    }

    #[test]
    fn lists_named_and_annotation_attachments() {
        let pdf = sample();
        let list = attachments_in(&pdf, pdf.page_iter());
        let names: Vec<&str> = list.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["notes.txt", "w2.pdf", "support.xlsx"]);
        assert_eq!(list[0].description, "Client notes");
        assert_eq!(list[0].size, Some(5));
        assert_eq!(list[0].modified, "D:20240102030405Z");
        assert!(list[1].is_pdf());
        assert_eq!(list[2].description, "Support");
        assert_eq!(list[2].at, AttachmentRef::Annot { page: 0, n: 0 });
    }

    #[test]
    fn reads_bytes_plain_and_compressed() {
        let pdf = sample();
        let list = attachments_in(&pdf, pdf.page_iter());
        let assets = MarkAssets::new(crate::Store::new());
        let bytes = |at: &AttachmentRef| attachment_bytes(&pdf, pdf.page_iter(), &assets, at);
        assert_eq!(bytes(&list[0].at).unwrap(), b"hello");
        assert_eq!(bytes(&list[1].at).unwrap(), b"%PDF-1.4 tiny");
        assert_eq!(bytes(&list[2].at).unwrap(), b"annot bytes");
        assert!(bytes(&AttachmentRef::Named("nope".into())).is_err());
        assert!(bytes(&AttachmentRef::Annot { page: 1, n: 0 }).is_err());
    }

    /// Read from a source, as a file opens, the sample lists as it does
    /// loaded, and each attachment reads the same bytes.
    #[test]
    fn a_source_lists_and_reads_as_the_loaded_file() {
        let mut pdf = sample();
        let src = crate::open::tests::lazily(&mut pdf);
        let list = attachments_in(&pdf, pdf.page_iter());
        assert_eq!(attachments_in(src.pdf(), src.page_ids()), list);
        let assets = MarkAssets::new(crate::Store::new());
        for a in &list {
            assert_eq!(
                read_attachment(&src, &assets, &a.at).unwrap(),
                attachment_bytes(&pdf, pdf.page_iter(), &assets, &a.at).unwrap(),
                "{}",
                a.name
            );
        }
    }

    #[test]
    fn portfolio_keys_split() {
        assert_eq!(folder_key("<3>a.pdf"), Some((3, "a.pdf")));
        assert_eq!(folder_key("a.pdf"), None);
    }
}
