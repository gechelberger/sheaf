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

use crate::annot::{Annot, PageGeom, from_json};

/// Annotations sheaf wrote into `path` earlier, keyed by page index.
pub fn read_sheaf_annots(path: impl AsRef<Path>) -> Result<HashMap<u32, Vec<Annot>>> {
    let pdf = lopdf::Document::load(path.as_ref())?;
    Ok(sheaf_annots_in(&pdf))
}

/// Same as `read_sheaf_annots`, from bytes already in memory.
pub fn read_sheaf_annots_mem(bytes: &[u8]) -> Result<HashMap<u32, Vec<Annot>>> {
    let pdf = lopdf::Document::load_mem(bytes)?;
    Ok(sheaf_annots_in(&pdf))
}

fn sheaf_annots_in(pdf: &lopdf::Document) -> HashMap<u32, Vec<Annot>> {
    let mut out: HashMap<u32, Vec<Annot>> = HashMap::new();
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
        for entry in list {
            let Ok((_, obj)) = pdf.dereference(entry) else {
                continue;
            };
            let Ok(dict) = obj.as_dict() else { continue };
            let Ok(payload) = dict.get(b"SheafAnnot").and_then(|o| o.as_str()) else {
                continue;
            };
            let bytes = if payload.starts_with(&[0xFE, 0xFF]) {
                let units: Vec<u16> = payload[2..]
                    .chunks(2)
                    .map(|c| u16::from_be_bytes([c[0], c[1]]))
                    .collect();
                String::from_utf16_lossy(&units).into_bytes()
            } else {
                payload.to_vec()
            };
            if let Some(a) = from_json(&bytes) {
                out.entry(i as u32).or_default().push(a);
            }
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
