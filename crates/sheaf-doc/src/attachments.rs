//! Files embedded in a PDF: the document-level ones in the catalog's
//! `/EmbeddedFiles` name tree and those on file-attachment annotations.
//! Listed when a file opens; bytes are read on demand.

use anyhow::{Context, Result, bail};
use lopdf::{Dictionary, Document as Pdf, LazyPdf, Object, ObjectId};

use crate::file::pdf_text;

/// Where an attachment lives, stable across reloads of the same file.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AttachmentRef {
    /// A key in `/Names /EmbeddedFiles`.
    Named(String),
    /// The `n`th file-attachment annotation on page `page` (file order).
    Annot { page: u32, n: u32 },
    /// One of sheaf's, by its bytes' name in the cache
    /// (`generated::store_blob`): saved or not yet.
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
/// annotation ones by page.
pub fn attachments_in(pdf: &Pdf) -> Vec<Attachment> {
    let mut out = Vec::new();
    let folders = crate::portfolio::folder_paths(pdf);
    for (key, spec) in named_files(pdf) {
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
    for (page_no, (_, page_id)) in pdf.get_pages().into_iter().enumerate() {
        let mut n = 0;
        for annot in page_annots(pdf, page_id) {
            if annot.get(b"Subtype").and_then(Object::as_name).ok() != Some(b"FileAttachment") {
                continue;
            }
            let at = AttachmentRef::Annot {
                page: page_no as u32,
                n,
            };
            n += 1;
            // sheaf's own are listed from the document (`Cached`).
            if annot.has(b"SheafAnnot") {
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

/// The bytes of attachment `at` in `file`, a PDF opened with `password`.
pub fn read_attachment(file: &[u8], password: Option<&str>, at: &AttachmentRef) -> Result<Vec<u8>> {
    if let AttachmentRef::Cached(stem) = at {
        let p = crate::generated::blob_path(stem)
            .with_context(|| format!("{stem:?} isn't a cache name"))?;
        return std::fs::read(&p).with_context(|| format!("reading {}", p.display()));
    }
    let pdf = crate::file::load_pdf_mem(file, password)?;
    attachment_bytes(&pdf, at)
}

/// The bytes of attachment `at` in a loaded file.
pub fn attachment_bytes(pdf: &Pdf, at: &AttachmentRef) -> Result<Vec<u8>> {
    if let AttachmentRef::Cached(stem) = at {
        let p = crate::generated::blob_path(stem)
            .with_context(|| format!("{stem:?} isn't a cache name"))?;
        return std::fs::read(&p).with_context(|| format!("reading {}", p.display()));
    }
    let spec = match at {
        AttachmentRef::Named(key) => named_files(pdf)
            .into_iter()
            .find(|(k, _)| k == key)
            .and_then(|(_, o)| resolve(pdf, o))
            .and_then(|o| o.as_dict().ok())
            .with_context(|| format!("no embedded file named {key:?}"))?,
        AttachmentRef::Cached(_) => unreachable!("read from the cache above"),
        AttachmentRef::Annot { page, n } => {
            let page_id = *pdf
                .get_pages()
                .get(&(page + 1))
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
    if stream.dict.get(b"Filter").is_err() {
        return Ok(stream.content.clone());
    }
    match stream.decompressed_content() {
        Ok(b) => Ok(b),
        Err(e) => bail!("can't decode the embedded file: {e}"),
    }
}

/// A file specification's name, description and parameters.
fn describe(pdf: &Pdf, at: AttachmentRef, fs: &Dictionary) -> Attachment {
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

/// `/Names /EmbeddedFiles`, flattened: (key, file spec) in tree order.
fn named_files(pdf: &Pdf) -> Vec<(String, &Object)> {
    let mut out = Vec::new();
    let tree = pdf
        .catalog()
        .ok()
        .and_then(|c| c.get(b"Names").ok())
        .and_then(|o| resolve(pdf, o))
        .and_then(|o| o.as_dict().ok())
        .and_then(|n| n.get(b"EmbeddedFiles").ok())
        .and_then(|o| resolve(pdf, o))
        .and_then(|o| o.as_dict().ok());
    if let Some(t) = tree {
        walk_names(pdf, t, &mut out, 0);
    }
    out
}

fn walk_names<'a>(
    pdf: &'a Pdf,
    node: &'a Dictionary,
    out: &mut Vec<(String, &'a Object)>,
    depth: u8,
) {
    if depth > 32 {
        return;
    }
    if let Some(Object::Array(names)) = node.get(b"Names").ok().and_then(|o| resolve(pdf, o)) {
        for pair in names.as_chunks::<2>().0 {
            if let Some(k) = resolve(pdf, &pair[0]).filter(|o| o.as_str().is_ok()) {
                out.push((pdf_text(k), &pair[1]));
            }
        }
    }
    if let Some(Object::Array(kids)) = node.get(b"Kids").ok().and_then(|o| resolve(pdf, o)) {
        for kid in kids {
            if let Some(Ok(d)) = resolve(pdf, kid).map(Object::as_dict) {
                walk_names(pdf, d, out, depth + 1);
            }
        }
    }
}

fn page_annots(pdf: &Pdf, page: ObjectId) -> Vec<&Dictionary> {
    pdf.get_dictionary(page)
        .ok()
        .and_then(|p| p.get(b"Annots").ok())
        .and_then(|o| resolve(pdf, o))
        .and_then(|o| o.as_array().ok())
        .map(|a| {
            a.iter()
                .filter_map(|o| resolve(pdf, o).and_then(|o| o.as_dict().ok()))
                .collect()
        })
        .unwrap_or_default()
}

/// A portfolio key "<12>Report.pdf" as (12, "Report.pdf").
fn folder_key(key: &str) -> Option<(i64, &str)> {
    let rest = key.strip_prefix('<')?;
    let (id, name) = rest.split_once('>')?;
    Some((id.parse().ok()?, name))
}

/// What objects are read from: a source's [`LazyPdf`], or an eager
/// document the old writers load, so [`resolve`] and `forms::xfa::read`
/// serve both. Transitional (M10.5): it goes with the eager documents in
/// slice 9.
pub(crate) trait Objects {
    /// Object `id`, its references followed; None when it can't be read.
    fn object(&self, id: ObjectId) -> Option<&Object>;

    fn dictionary(&self, id: ObjectId) -> Option<&Dictionary> {
        self.object(id)?.as_dict().ok()
    }
}

impl Objects for Pdf {
    fn object(&self, id: ObjectId) -> Option<&Object> {
        self.get_object(id).ok()
    }
}

impl Objects for LazyPdf {
    fn object(&self, id: ObjectId) -> Option<&Object> {
        self.get_object(id).ok()
    }
}

pub(crate) fn resolve<'a, P: Objects + ?Sized>(pdf: &'a P, o: &'a Object) -> Option<&'a Object> {
    match o {
        Object::Reference(id) => pdf.object(*id),
        o => Some(o),
    }
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
        let list = attachments_in(&pdf);
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
        let list = attachments_in(&pdf);
        assert_eq!(attachment_bytes(&pdf, &list[0].at).unwrap(), b"hello");
        assert_eq!(
            attachment_bytes(&pdf, &list[1].at).unwrap(),
            b"%PDF-1.4 tiny"
        );
        assert_eq!(attachment_bytes(&pdf, &list[2].at).unwrap(), b"annot bytes");
        assert!(attachment_bytes(&pdf, &AttachmentRef::Named("nope".into())).is_err());
    }

    #[test]
    fn portfolio_keys_split() {
        assert_eq!(folder_key("<3>a.pdf"), Some((3, "a.pdf")));
        assert_eq!(folder_key("a.pdf"), None);
    }
}
