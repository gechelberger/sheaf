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

use crate::annot::{Annot, Box2, PageGeom, Pt, from_json};

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
}

/// Everything annotation-shaped in a file: our marks (editable, keyed by
/// page index) and everyone else's.
#[derive(Debug, Default)]
pub struct AnnotsInFile {
    pub sheaf: HashMap<u32, Vec<Annot>>,
    pub foreign: Vec<ForeignAnnot>,
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

/// Annotations sheaf wrote into `path` earlier, keyed by page index.
pub fn read_sheaf_annots(path: impl AsRef<Path>) -> Result<HashMap<u32, Vec<Annot>>> {
    Ok(read_annots(path, None)?.sheaf)
}

/// Same as `read_sheaf_annots`, from bytes already in memory.
pub fn read_sheaf_annots_mem(bytes: &[u8]) -> Result<HashMap<u32, Vec<Annot>>> {
    let pdf = lopdf::Document::load_mem(bytes)?;
    Ok(annots_in(&pdf).sheaf)
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
        for entry in list {
            let Ok((_, obj)) = pdf.dereference(entry) else {
                continue;
            };
            let Ok(dict) = obj.as_dict() else { continue };
            if let Ok(payload) = dict.get(b"SheafAnnot") {
                if let Some(a) = from_json(pdf_text(payload).as_bytes()) {
                    out.sheaf.entry(i as u32).or_default().push(a);
                }
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
            out.foreign.push(ForeignAnnot {
                page: i as u32,
                subtype,
                author: text(b"T"),
                contents: text(b"Contents"),
                modified: text(b"M"),
                rect: Box2::from_corners(p, q),
            });
        }
    }
    out
}

/// MediaBox and /Rotate of a page (inherited attributes resolved).
pub fn page_geom(pdf: &lopdf::Document, page: lopdf::ObjectId) -> PageGeom {
    let mut media = None;
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
        media: media.unwrap_or([0.0, 0.0, 612.0, 792.0]),
        rotate: rotate.unwrap_or(0).rem_euclid(360),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{Object, Stream, dictionary};

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
        };
        let (dict, _) = crate::annot::to_pdf(
            &ours,
            &PageGeom {
                media: [0.0, 0.0, 612.0, 792.0],
                rotate: 0,
            },
        );
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
