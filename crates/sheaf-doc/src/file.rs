//! Direct access to a PDF file's object graph via lopdf. This is the
//! serialization backend (see docs/document-model.md), not the editing
//! model; the viewer never touches it.

use std::path::Path;

use anyhow::{Context, Result};
use sheaf_core::{PageIndex, PageSize, Rotation};

pub struct PdfFile {
    inner: lopdf::Document,
}

impl PdfFile {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let inner =
            lopdf::Document::load(path).with_context(|| format!("loading {}", path.display()))?;
        Ok(Self { inner })
    }

    pub fn page_count(&self) -> u32 {
        self.inner.get_pages().len() as u32
    }

    /// Size and rotation of a page, resolving inherited attributes.
    pub fn page_geometry(&self, index: PageIndex) -> Result<(PageSize, Rotation)> {
        let pages = self.inner.get_pages();
        let id = *pages
            .get(&(index.0 + 1))
            .with_context(|| format!("no page {}", index.0))?;
        let media_box = self
            .inherited(id, b"MediaBox")?
            .and_then(|o| o.as_array().ok().cloned())
            .unwrap_or_default();
        let num = |i: usize| {
            media_box
                .get(i)
                .and_then(|o| o.as_float().ok())
                .unwrap_or(0.0)
        };
        let size = PageSize {
            width: (num(2) - num(0)).abs(),
            height: (num(3) - num(1)).abs(),
        };
        let rot = self
            .inherited(id, b"Rotate")?
            .and_then(|o| o.as_i64().ok())
            .map(|d| Rotation::from_degrees(d as i32))
            .unwrap_or_default();
        Ok((size, rot))
    }

    /// Walk up the `/Parent` chain looking for `key`.
    fn inherited(&self, mut id: lopdf::ObjectId, key: &[u8]) -> Result<Option<lopdf::Object>> {
        loop {
            let dict = self.inner.get_dictionary(id)?;
            if let Ok(obj) = dict.get(key) {
                return Ok(Some(self.inner.dereference(obj)?.1.clone()));
            }
            match dict.get(b"Parent") {
                Ok(parent) => id = parent.as_reference()?,
                Err(_) => return Ok(None),
            }
        }
    }

    /// Full rewrite. Incremental save comes in M2.
    pub fn save_as(&mut self, path: impl AsRef<Path>) -> Result<()> {
        self.inner.save(path.as_ref())?;
        Ok(())
    }

    pub fn into_inner(self) -> lopdf::Document {
        self.inner
    }
}

use std::collections::HashMap;

use crate::annot::{Annot, Box2, PART, PageGeom, Pt, from_json};
use crate::model::{FlatOutlineItem, TextStyle};

/// The mark an annotation dictionary carries, if this build can read it.
fn sheaf_mark(dict: &lopdf::Dictionary) -> Option<Annot> {
    from_json(pdf_text(dict.get(b"SheafAnnot").ok()?).as_bytes())
}

/// Is this `/Annots` entry one of ours to rewrite: a mark this build can
/// read, or a companion of one (see `annot::PART`)? A `/SheafAnnot` it
/// can't read, from a newer sheaf, isn't: it's kept like any other tool's
/// annotation, so a save here never loses it.
pub(crate) fn is_ours(pdf: &lopdf::Document, entry: &lopdf::Object) -> bool {
    let Some(dict) = pdf
        .dereference(entry)
        .ok()
        .and_then(|(_, o)| o.as_dict().ok())
    else {
        return false;
    };
    if dict.has(b"SheafAnnot") {
        return sheaf_mark(dict).is_some();
    }
    // One step only: a companion's /IRT is its mark, never another companion.
    dict.has(PART.as_bytes())
        && dict
            .get(b"IRT")
            .ok()
            .and_then(|irt| pdf.dereference(irt).ok())
            .and_then(|(_, o)| o.as_dict().ok())
            .and_then(sheaf_mark)
            .is_some()
}

/// An annotation some other tool left in the file. Listed read-only in the
/// Comments panel; PDFium draws it.
#[derive(Debug, Clone, PartialEq)]
pub struct ForeignAnnot {
    /// Page index in the file.
    pub page: u32,
    /// `/Subtype`, e.g. `Text`, `Highlight`.
    pub subtype: String,
    pub author: String,
    pub contents: String,
    /// `/M`, a PDF date string, or empty.
    pub modified: String,
    /// Bounds in the page's displayed space.
    pub rect: Box2,
    /// `/NM`, or for one without, a stand-in made from what it says
    /// ([`anon_name`]), so replies can still name it.
    pub name: String,
    /// The comment whose thread it's in (`/IRT`), by name.
    pub parent: Option<String>,
    /// `/RT /Group`: part of its parent (the StrikeOut of Acrobat's
    /// replace-text edit, say) rather than a reply to it.
    pub grouped: bool,
    /// A review state it sets on its parent: (`/StateModel`, `/State`).
    pub state: Option<(String, String)>,
    /// `/CreationDate`, or empty.
    pub created: String,
    /// Its position in the page's `/Annots`, which is how PDFium counts
    /// them too, so sheaf can hide it on screen.
    pub slot: usize,
    /// The file hides it (`/F` bit 2): sheaf never shows it.
    pub hidden: bool,
    /// `/C`, when it's RGB.
    pub color: Option<[u8; 3]>,
}

/// The names deleting `removed` takes off a page: those, and every
/// comment in their threads. `comments` is each comment's name and its
/// parent's.
pub fn removed_with_threads<'a>(
    comments: impl Iterator<Item = (&'a str, Option<&'a str>)> + Clone,
    removed: &im::OrdSet<String>,
) -> std::collections::HashSet<String> {
    let mut gone: std::collections::HashSet<String> = removed.iter().cloned().collect();
    loop {
        let more: Vec<String> = comments
            .clone()
            .filter(|(name, parent)| {
                !gone.contains(*name) && parent.is_some_and(|p| gone.contains(p))
            })
            .map(|(name, _)| name.to_string())
            .collect();
        if more.is_empty() {
            return gone;
        }
        gone.extend(more);
    }
}

/// A name for an annotation without `/NM`, from what it says: the same
/// every time it's read, so a reply made in sheaf can name it, and a save
/// writes it in as its `/NM`.
pub fn anon_name(dict: &lopdf::Dictionary) -> String {
    // FNV-1a: stable across builds, unlike std's hasher.
    let mut h: u64 = 0xcbf29ce484222325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes.iter().chain(&[0xff]) {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    };
    for key in [
        b"Subtype".as_slice(),
        b"Rect",
        b"T",
        b"M",
        b"CreationDate",
        b"Contents",
    ] {
        let v = dict.get(key).ok();
        match v {
            Some(lopdf::Object::Array(a)) => {
                for o in a {
                    eat(format!("{:.2}", o.as_float().unwrap_or(0.0)).as_bytes());
                }
            }
            Some(lopdf::Object::Name(n)) => eat(n),
            Some(o) => eat(pdf_text(o).as_bytes()),
            None => eat(b""),
        }
    }
    format!("sheaf-anon:{h:016x}")
}

/// What an annotation is called in threads: its `/NM`, else [`anon_name`].
pub fn thread_name(dict: &lopdf::Dictionary) -> String {
    dict.get(b"NM")
        .ok()
        .map(pdf_text)
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| anon_name(dict))
}

/// Everything annotation-shaped in a file: our marks (editable, keyed by
/// page index) and everyone else's.
#[derive(Debug, Default)]
pub struct AnnotsInFile {
    pub sheaf: HashMap<u32, Vec<Annot>>,
    pub foreign: Vec<ForeignAnnot>,
}

/// A source's bytes: the ones kept from when it was opened, else the file's
/// now (see `SourceInfo::data`).
pub fn source_bytes(info: &crate::SourceInfo) -> Result<std::sync::Arc<Vec<u8>>> {
    match &info.data {
        Some(d) => Ok(d.0.clone()),
        None => Ok(std::sync::Arc::new(
            std::fs::read(&info.path)
                .with_context(|| format!("reading {}", info.path.display()))?,
        )),
    }
}

/// A source as lopdf sees it, decrypted as [`load_pdf`] does: from the
/// bytes kept from when it was opened when there are any, so a file
/// replaced since (by a save in place) doesn't matter.
pub fn load_source(info: &crate::SourceInfo) -> Result<lopdf::Document> {
    let Some(data) = &info.data else {
        return load_pdf(&info.path, info.password.as_deref());
    };
    let path = info.path.display();
    let mut pdf = lopdf::Document::load_mem(&data.0).with_context(|| format!("loading {path}"))?;
    if pdf.is_encrypted() {
        let options = lopdf::LoadOptions::with_password(info.password.as_deref().unwrap_or(""));
        pdf = lopdf::Document::load_mem_with_options(&data.0, options)
            .map_err(|e| anyhow::anyhow!("{e}"))
            .with_context(|| format!("decrypting {path}"))?;
        pdf.trailer.remove(b"Encrypt");
        pdf.encryption_state = None;
    }
    Ok(pdf)
}

/// Load a file with lopdf and, if it's encrypted, decrypt it in memory with
/// `password` (or the empty user password many files use). The result is
/// plain: strings and streams readable, `/Encrypt` gone, so writing it
/// produces an unencrypted file.
pub fn load_pdf(path: &Path, password: Option<&str>) -> Result<lopdf::Document> {
    let mut pdf =
        lopdf::Document::load(path).with_context(|| format!("loading {}", path.display()))?;
    if pdf.is_encrypted() {
        // lopdf only parses an encrypted file's objects when it has the
        // password at load time, so load again with it.
        pdf = lopdf::Document::load_with_password(path, password.unwrap_or(""))
            .map_err(|e| anyhow::anyhow!("{e}"))
            .with_context(|| format!("decrypting {}", path.display()))?;
        pdf.trailer.remove(b"Encrypt");
        pdf.encryption_state = None;
    }
    Ok(pdf)
}

/// What sheaf shows about a file besides its pages and marks, read once
/// when it opens.
#[derive(Debug, Default)]
pub struct FileFacts {
    /// The file's `/PageLabels` ranges, by page index.
    pub label_ranges: Option<Vec<crate::labels::LabelRange>>,
    /// The header/footer sheaf drew into the file, and the page indexes
    /// it's on (none: all).
    pub header_footer: Option<(crate::stamps::HeaderFooter, Option<Vec<usize>>)>,
    /// Embedded files, document-level and on annotations.
    pub attachments: Vec<crate::attachments::Attachment>,
    /// Set when the file is a PDF Portfolio.
    pub collection: Option<crate::portfolio::Collection>,
    /// Optional content groups, in display order.
    pub layers: Vec<crate::layers::Layer>,
    /// Each bookmark's color and style, in pre-order; empty when none has
    /// either (see [`outline_styles`]).
    pub outline_styles: Vec<OutlineStyle>,
}

/// A bookmark's `/C` and `/F`, which PDFium's outline doesn't carry.
#[derive(Debug, Clone, PartialEq)]
pub struct OutlineStyle {
    pub title: String,
    pub color: Option<[u8; 3]>,
    pub style: TextStyle,
}

/// Each bookmark's title, color and style, in pre-order (the order PDFium
/// lists them), or nothing when no bookmark has a color or style. Matched to
/// PDFium's items by position, checked by title.
pub fn outline_styles(pdf: &lopdf::Document) -> Vec<OutlineStyle> {
    let first = pdf
        .catalog()
        .ok()
        .and_then(|c| c.get(b"Outlines").ok())
        .and_then(|o| pdf.dereference(o).ok())
        .and_then(|(_, o)| o.as_dict().ok())
        .and_then(|d| d.get(b"First").ok())
        .and_then(|f| f.as_reference().ok());
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    // (item, depth); children before siblings, as in pre-order.
    let mut stack: Vec<(lopdf::ObjectId, u16)> = first.into_iter().map(|f| (f, 0)).collect();
    while let Some((id, depth)) = stack.pop() {
        if !seen.insert(id) || out.len() >= 100_000 {
            continue;
        }
        let Ok(item) = pdf.get_dictionary(id) else {
            continue;
        };
        let color = item
            .get(b"C")
            .ok()
            .and_then(|c| pdf.dereference(c).ok())
            .and_then(|(_, c)| c.as_array().ok())
            .filter(|a| a.len() == 3)
            .map(|a| {
                let v = |i: usize| {
                    (a[i].as_float().unwrap_or(0.0).clamp(0.0, 1.0) * 255.0).round() as u8
                };
                [v(0), v(1), v(2)]
            })
            // Black is the default; don't call it a color.
            .filter(|c| *c != [0, 0, 0]);
        let flags = item
            .get(b"F")
            .ok()
            .and_then(|f| f.as_i64().ok())
            .unwrap_or(0);
        out.push(OutlineStyle {
            title: item.get(b"Title").map(pdf_text).unwrap_or_default(),
            color,
            style: TextStyle {
                italic: flags & 1 != 0,
                bold: flags & 2 != 0,
            },
        });
        if let Ok(next) = item.get(b"Next").and_then(|n| n.as_reference()) {
            stack.push((next, depth));
        }
        if depth < 64
            && let Ok(child) = item.get(b"First").and_then(|c| c.as_reference())
        {
            stack.push((child, depth + 1));
        }
    }
    if out
        .iter()
        .all(|s| s.color.is_none() && s.style == TextStyle::default())
    {
        out.clear();
    }
    out
}

/// Give PDFium's flattened bookmarks their colors and styles: the `i`th
/// takes the `i`th style when their titles agree.
pub fn style_flat_items(items: &mut [FlatOutlineItem], styles: &[OutlineStyle]) {
    for (item, s) in items.iter_mut().zip(styles) {
        if item.title.trim() == s.title.trim() {
            item.color = s.color;
            item.style = s.style;
        }
    }
}

/// The facts sheaf reads from an open file's catalog.
pub fn facts_in(pdf: &lopdf::Document) -> FileFacts {
    FileFacts {
        label_ranges: crate::labels::label_ranges(pdf),
        header_footer: crate::stamps::from_catalog(pdf),
        attachments: crate::attachments::attachments_in(pdf),
        collection: crate::portfolio::collection(pdf),
        layers: crate::layers::layers_in(pdf),
        outline_styles: outline_styles(pdf),
    }
}

/// Everything read from a file when it opens: annotations and facts, from
/// one load.
pub fn read_open(
    path: impl AsRef<Path>,
    password: Option<&str>,
) -> Result<(AnnotsInFile, FileFacts)> {
    let pdf = load_pdf(path.as_ref(), password)?;
    let found = annots_in(&pdf);
    recover_stamps(&pdf);
    Ok((found, facts_in(&pdf)))
}

/// Our stamps whose page isn't in this machine's cache (a file from
/// someone else), made again from their saved appearance.
pub fn recover_stamps(pdf: &lopdf::Document) {
    for (_, page_id) in pdf.get_pages() {
        let Ok(page) = pdf.get_dictionary(page_id) else {
            continue;
        };
        let Some(list) = page
            .get(b"Annots")
            .ok()
            .and_then(|a| pdf.dereference(a).ok())
            .and_then(|(_, a)| a.as_array().ok())
        else {
            continue;
        };
        for entry in list {
            let Some(dict) = pdf
                .dereference(entry)
                .ok()
                .and_then(|(_, o)| o.as_dict().ok())
            else {
                continue;
            };
            let Some(Annot {
                kind: crate::annot::AnnotKind::PageStamp { stamp, .. },
                ..
            }) = sheaf_mark(dict)
            else {
                continue;
            };
            if crate::generated::path_of(&stamp).exists() {
                continue;
            }
            // /AP /N → its /Resources /XObject /SheafStamp.
            let form = dict
                .get(b"AP")
                .ok()
                .and_then(|o| pdf.dereference(o).ok())
                .and_then(|(_, o)| o.as_dict().ok())
                .and_then(|ap| ap.get(b"N").ok())
                .and_then(|n| n.as_reference().ok())
                .and_then(|n| match pdf.get_object(n) {
                    Ok(lopdf::Object::Stream(s)) => Some(s.dict.clone()),
                    _ => None,
                })
                .and_then(|n| {
                    let res = pdf
                        .dereference(n.get(b"Resources").ok()?)
                        .ok()?
                        .1
                        .as_dict()
                        .ok()?
                        .clone();
                    let xo = pdf
                        .dereference(res.get(b"XObject").ok()?)
                        .ok()?
                        .1
                        .as_dict()
                        .ok()?
                        .clone();
                    xo.get(b"SheafStamp").ok()?.as_reference().ok()
                });
            let made = form
                .ok_or_else(|| anyhow::anyhow!("no saved appearance"))
                .and_then(|f| crate::stamp_lib::form_to_pdf(pdf, f))
                .and_then(|bytes| crate::generated::store_as(&stamp, &bytes));
            if let Err(e) = made {
                log::warn!("recovering stamp {stamp}: {e:#}");
            }
        }
    }
}

/// Annotations sheaf wrote into `path` earlier, keyed by page index.
pub fn read_sheaf_annots(path: impl AsRef<Path>) -> Result<HashMap<u32, Vec<Annot>>> {
    Ok(read_annots(path, None)?.sheaf)
}

/// Same as `read_sheaf_annots`, from bytes already in memory.
pub fn read_sheaf_annots_mem(bytes: &[u8]) -> Result<HashMap<u32, Vec<Annot>>> {
    let pdf = lopdf::Document::load_mem(bytes)?;
    Ok(annots_in(&pdf).sheaf)
}

/// All annotations in a file's bytes.
pub fn read_annots_mem(bytes: &[u8]) -> Result<AnnotsInFile> {
    let pdf = lopdf::Document::load_mem(bytes)?;
    Ok(annots_in(&pdf))
}

/// All annotations in `path`.
pub fn read_annots(path: impl AsRef<Path>, password: Option<&str>) -> Result<AnnotsInFile> {
    let pdf = load_pdf(path.as_ref(), password)?;
    Ok(annots_in(&pdf))
}

/// Decode a PDF text string: UTF-16BE with BOM, else PDFDoc/Latin-1.
pub fn pdf_text(o: &lopdf::Object) -> String {
    let Ok(bytes) = o.as_str() else {
        return String::new();
    };
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = bytes[2..]
            .chunks(2)
            .map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        bytes.iter().map(|&b| b as char).collect()
    }
}

/// Subtypes that aren't comments: links, form fields, and the popup
/// windows that belong to other annotations.
fn is_comment_subtype(s: &str) -> bool {
    !matches!(
        s,
        "Link"
            | "Widget"
            | "Popup"
            | "Screen"
            | "PrinterMark"
            | "TrapNet"
            | "Watermark"
            | "3D"
            | "Movie"
            | "Sound"
            | "RichMedia"
    )
}

fn annots_in(pdf: &lopdf::Document) -> AnnotsInFile {
    let mut out = AnnotsInFile::default();
    for (i, page_id) in pdf.get_pages().into_values().enumerate() {
        let Ok(page) = pdf.get_dictionary(page_id) else {
            continue;
        };
        let Ok(annots) = page
            .get(b"Annots")
            .and_then(|a| pdf.dereference(a).map(|(_, o)| o))
        else {
            continue;
        };
        let Ok(list) = annots.as_array() else {
            continue;
        };
        let mut geom: Option<PageGeom> = None;
        for (slot, entry) in list.iter().enumerate() {
            let Ok((_, obj)) = pdf.dereference(entry) else {
                continue;
            };
            let Ok(dict) = obj.as_dict() else { continue };
            if let Some(a) = sheaf_mark(dict) {
                out.sheaf.entry(i as u32).or_default().push(a);
                continue;
            }
            // A companion of one of ours is part of it. Anything else,
            // including a mark this build can't read, is listed as another
            // tool's.
            if dict.has(PART.as_bytes()) && is_ours(pdf, entry) {
                continue;
            }
            let subtype = dict
                .get(b"Subtype")
                .ok()
                .and_then(|o| o.as_name().ok())
                .map(|n| String::from_utf8_lossy(n).into_owned())
                .unwrap_or_default();
            if subtype.is_empty() || !is_comment_subtype(&subtype) {
                continue;
            }
            let rect: Vec<f32> = dict
                .get(b"Rect")
                .ok()
                .and_then(|o| pdf.dereference(o).ok())
                .and_then(|(_, o)| o.as_array().ok())
                .map(|a| a.iter().map(|v| v.as_float().unwrap_or(0.0)).collect())
                .unwrap_or_default();
            if rect.len() != 4 {
                continue;
            }
            let g = *geom.get_or_insert_with(|| page_geom(pdf, page_id));
            let p = g.to_display(Pt {
                x: rect[0],
                y: rect[1],
            });
            let q = g.to_display(Pt {
                x: rect[2],
                y: rect[3],
            });
            let text = |key: &[u8]| dict.get(key).map(pdf_text).unwrap_or_default();
            let name_of = |key: &[u8]| -> Option<String> {
                let (_, o) = pdf.dereference(dict.get(key).ok()?).ok()?;
                Some(thread_name(o.as_dict().ok()?))
            };
            let state = match (text(b"StateModel"), text(b"State")) {
                (_, s) if s.is_empty() => None,
                // Acrobat's default model.
                (m, s) if m.is_empty() => Some((crate::annot::REVIEW.to_string(), s)),
                (m, s) => Some((m, s)),
            };
            out.foreign.push(ForeignAnnot {
                page: i as u32,
                subtype,
                author: text(b"T"),
                contents: text(b"Contents"),
                modified: text(b"M"),
                rect: Box2::from_corners(p, q),
                name: thread_name(dict),
                parent: name_of(b"IRT"),
                grouped: dict
                    .get(b"RT")
                    .ok()
                    .and_then(|o| o.as_name().ok())
                    .is_some_and(|n| n == b"Group"),
                state,
                created: text(b"CreationDate"),
                slot,
                hidden: dict
                    .get(b"F")
                    .ok()
                    .and_then(|f| f.as_i64().ok())
                    .is_some_and(|f| f & 2 != 0),
                color: dict
                    .get(b"C")
                    .ok()
                    .and_then(|c| pdf.dereference(c).ok())
                    .and_then(|(_, c)| c.as_array().ok())
                    .filter(|c| c.len() == 3)
                    .map(|c| {
                        let v = |i: usize| {
                            (c[i].as_float().unwrap_or(0.0).clamp(0.0, 1.0) * 255.0).round() as u8
                        };
                        [v(0), v(1), v(2)]
                    }),
            });
        }
    }
    out
}

/// The visible box and /Rotate of a page (inherited attributes resolved).
pub fn page_geom(pdf: &lopdf::Document, page: lopdf::ObjectId) -> PageGeom {
    let mut media = None;
    let mut crop = None;
    let mut rotate = None;
    let mut id = page;
    for _ in 0..64 {
        let Ok(dict) = pdf.get_dictionary(id) else {
            break;
        };
        if media.is_none()
            && let Ok(mb) = dict
                .get(b"MediaBox")
                .and_then(|o| pdf.dereference(o).map(|(_, o)| o))
            && let Ok(arr) = mb.as_array()
            && arr.len() == 4
        {
            let v: Vec<f32> = arr.iter().map(|o| o.as_float().unwrap_or(0.0)).collect();
            media = Some([
                v[0].min(v[2]),
                v[1].min(v[3]),
                v[0].max(v[2]),
                v[1].max(v[3]),
            ]);
        }
        if crop.is_none()
            && let Ok(cb) = dict
                .get(b"CropBox")
                .and_then(|o| pdf.dereference(o).map(|(_, o)| o))
            && let Ok(arr) = cb.as_array()
        {
            crop = crate::annot::box_of(arr);
        }
        if rotate.is_none()
            && let Ok(r) = dict.get(b"Rotate").and_then(|o| o.as_i64())
        {
            rotate = Some(r as i32);
        }
        match dict.get(b"Parent").and_then(|p| p.as_reference()) {
            Ok(parent) => id = parent,
            Err(_) => break,
        }
    }
    PageGeom {
        media: crate::annot::visible_box(media.unwrap_or([0.0, 0.0, 612.0, 792.0]), crop),
        rotate: rotate.unwrap_or(0).rem_euclid(360),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{Object, Stream, dictionary};

    #[test]
    fn their_threads_read_with_states_and_groups() {
        use lopdf::{Object, Stream, dictionary};
        let mut pdf = lopdf::Document::with_version("1.7");
        let pages_id = pdf.new_object_id();
        let content = pdf.add_object(Stream::new(dictionary! {}, b"".to_vec()));
        let rect = || Object::Array(vec![10.into(), 10.into(), 20.into(), 20.into()]);
        let text = |s: &str| Object::string_literal(s);
        let parent = pdf.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Highlight", "Rect" => rect(), "NM" => text("p"),
        });
        let accepted = pdf.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Text", "Rect" => rect(), "IRT" => parent,
            "State" => text("Accepted"), "StateModel" => text("Review"), "F" => 30,
        });
        // No /StateModel: Review, as Acrobat reads it.
        let completed = pdf.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Text", "Rect" => rect(), "IRT" => parent,
            "State" => text("Completed"),
        });
        let part = pdf.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "StrikeOut", "Rect" => rect(), "IRT" => parent,
            "RT" => Object::Name(b"Group".to_vec()),
        });
        let list: Vec<Object> = [parent, accepted, completed, part]
            .into_iter()
            .map(Object::Reference)
            .collect();
        let page = pdf.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => content,
            "MediaBox" => Object::Array(vec![0.into(), 0.into(), 612.into(), 792.into()]),
            "Annots" => list,
        });
        pdf.objects.insert(
            pages_id,
            Object::Dictionary(
                dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 },
            ),
        );
        let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        pdf.trailer.set("Root", catalog);
        let found = annots_in(&pdf).foreign;
        assert_eq!(found.len(), 4);
        assert_eq!(found[0].name, "p");
        assert_eq!(found[0].parent, None);
        assert_eq!(
            found[1].state,
            Some(("Review".to_string(), "Accepted".to_string()))
        );
        assert_eq!(found[1].parent.as_deref(), Some("p"));
        assert_eq!(found[2].state.as_ref().unwrap().0, "Review");
        assert!(found[3].grouped && !found[1].grouped);
    }

    #[test]
    fn a_companion_follows_its_mark() {
        use lopdf::{Object, Stream, dictionary};
        let mut pdf = lopdf::Document::with_version("1.7");
        let pages_id = pdf.new_object_id();
        let content = pdf.add_object(Stream::new(dictionary! {}, b"".to_vec()));
        let rect = || Object::Array(vec![10.into(), 10.into(), 20.into(), 20.into()]);
        let ours = Annot {
            id: crate::annot::AnnotId(5),
            kind: crate::annot::AnnotKind::Note {
                pos: Pt { x: 1.0, y: 2.0 },
            },
            color: [1, 2, 3],
            opacity: 1.0,
            stroke: 1.0,
            author: "me".into(),
            contents: String::new(),
            modified: String::new(),
            created: String::new(),
            fill: None,
            dashed: false,
        };
        let geom = PageGeom {
            media: [0.0, 0.0, 612.0, 792.0],
            rotate: 0,
        };
        let main = crate::annot::to_pdf(&ours, &geom, &mut pdf).main;
        let main = pdf.add_object(main);
        let part = pdf.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Caret", "Rect" => rect(),
            "IRT" => main, PART => true,
        });
        // A mark from a newer sheaf, and its companion.
        let unknown = pdf.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "StrikeOut", "Rect" => rect(),
            "SheafAnnot" => Object::string_literal(r#"{"id":6,"kind":{"Future":{}}}"#),
        });
        let unknown_part = pdf.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Caret", "Rect" => rect(),
            "IRT" => unknown, PART => true,
        });
        let list: Vec<Object> = [main, part, unknown, unknown_part]
            .into_iter()
            .map(Object::Reference)
            .collect();
        let page = pdf.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => content,
            "MediaBox" => Object::Array(vec![0.into(), 0.into(), 612.into(), 792.into()]),
            "Annots" => list.clone(),
        });
        pdf.objects.insert(
            pages_id,
            Object::Dictionary(
                dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 },
            ),
        );
        let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        pdf.trailer.set("Root", catalog);

        let mine: Vec<bool> = list.iter().map(|e| is_ours(&pdf, e)).collect();
        assert_eq!(mine, vec![true, true, false, false]);
        let found = annots_in(&pdf);
        assert_eq!(found.sheaf[&0], vec![ours]);
        // The newer sheaf's mark and its companion are listed like another
        // tool's, and kept.
        let subtypes: Vec<&str> = found.foreign.iter().map(|f| f.subtype.as_str()).collect();
        assert_eq!(subtypes, vec!["StrikeOut", "Caret"]);
    }

    /// One page carrying a third-party sticky note, a link (not a comment),
    /// a popup (not a comment), and one of ours.
    #[test]
    fn reads_foreign_and_sheaf_annotations() {
        let mut pdf = lopdf::Document::with_version("1.7");
        let pages_id = pdf.new_object_id();
        let content = pdf.add_object(Stream::new(dictionary! {}, b"".to_vec()));
        let note = pdf.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Text",
            "Rect" => Object::Array(vec![100.0.into(), 700.0.into(), 120.0.into(), 720.0.into()]),
            "T" => Object::String(b"Reviewer".to_vec(), lopdf::StringFormat::Literal),
            "Contents" => Object::String(vec![0xFE, 0xFF, 0x00, b'H', 0x00, b'i', 0x20, 0x14], lopdf::StringFormat::Hexadecimal),
            "M" => Object::String(b"D:20260101120000Z".to_vec(), lopdf::StringFormat::Literal),
        });
        let link = pdf.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Link",
            "Rect" => Object::Array(vec![0.0.into(), 0.0.into(), 10.0.into(), 10.0.into()]),
        });
        let popup = pdf.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Popup",
            "Rect" => Object::Array(vec![0.0.into(), 0.0.into(), 10.0.into(), 10.0.into()]),
        });
        let ours = Annot {
            id: crate::annot::AnnotId(3),
            kind: crate::annot::AnnotKind::Note {
                pos: Pt { x: 1.0, y: 2.0 },
            },
            color: [1, 2, 3],
            opacity: 1.0,
            stroke: 1.0,
            author: "me".into(),
            contents: "mine".into(),
            modified: String::new(),
            created: String::new(),
            fill: None,
            dashed: false,
        };
        let dict = crate::annot::to_pdf(
            &ours,
            &PageGeom {
                media: [0.0, 0.0, 612.0, 792.0],
                rotate: 0,
            },
            &mut pdf,
        )
        .main;
        let mine = pdf.add_object(dict);
        let page = pdf.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => Object::Array(vec![0.into(), 0.into(), 612.into(), 792.into()]),
            "Contents" => content,
            "Annots" => Object::Array(vec![note.into(), link.into(), popup.into(), mine.into()]),
        });
        pdf.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1,
            }),
        );
        let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        pdf.trailer.set("Root", catalog);

        let found = annots_in(&pdf);
        assert_eq!(found.sheaf[&0], vec![ours]);
        assert_eq!(found.foreign.len(), 1, "{:?}", found.foreign);
        let f = &found.foreign[0];
        assert_eq!(f.subtype, "Text");
        assert_eq!(f.author, "Reviewer");
        assert_eq!(f.contents, "Hi\u{2014}");
        assert_eq!(f.modified, "D:20260101120000Z");
        // User-space (100,700)-(120,720) on a 792pt page is displayed y 72..92.
        assert_eq!(
            (f.rect.x, f.rect.y, f.rect.w, f.rect.h),
            (100.0, 72.0, 20.0, 20.0)
        );
    }
}
