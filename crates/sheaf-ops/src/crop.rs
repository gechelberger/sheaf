//! Crop pages (Document > Crop Pages…). A cropped page becomes a page of
//! a generated PDF whose `/CropBox` is the new shown area: sources are
//! files, and PDFium then renders, sizes and maps text for it like any
//! page. `Edit::RebasePages` moves the document's page onto it, keeping
//! its id, rotation and bookmarks, and shifts its marks and OCR text so
//! they stay on the same content.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sheaf_core::{PageIndex, PageSize, Rotation};
use sheaf_doc::annot::Pt;
use sheaf_doc::{PageId, Snapshot, SourceId, SourceInfo};
use sheaf_progress::{Progress, Unit};
use sheaf_render::pdfium::PdfiumEngine;
use sheaf_render::{Job, PixelFormat, Renderer};

/// Points to take off each side, as the page is shown (after sheaf's
/// rotation).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Margins {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CropSpec {
    Margins(Margins),
    /// Down to the page's content, plus `pad` points.
    TrimWhite {
        pad: f32,
    },
}

pub use crate::derive::DerivedSource as CroppedSource;

/// Smallest page a crop may leave, in points (a quarter inch).
const MIN_SIDE: f32 = 18.0;

/// `m` (as shown with `rot`) as margins of the unrotated page.
fn unrotate(m: Margins, rot: Rotation) -> Margins {
    match rot {
        Rotation::R0 => m,
        // Shown = turned clockwise: the page's left edge is shown on top.
        Rotation::R90 => Margins {
            left: m.top,
            top: m.right,
            right: m.bottom,
            bottom: m.left,
        },
        Rotation::R180 => Margins {
            top: m.bottom,
            right: m.left,
            bottom: m.top,
            left: m.right,
        },
        Rotation::R270 => Margins {
            right: m.top,
            bottom: m.right,
            left: m.bottom,
            top: m.left,
        },
    }
}

/// Plan the crop of `ids` in `snap`; the cropped copies are written to the
/// generated-file cache.
pub fn plan(
    snap: &Snapshot,
    ids: &[PageId],
    spec: CropSpec,
    progress: &dyn Progress,
) -> Result<Vec<CroppedSource>> {
    progress.set_total(Some(ids.len() as u64), Unit::Pages);
    // Pages grouped by the source they show now.
    let mut by_source: BTreeMap<SourceId, Vec<(PageId, u32, Rotation)>> = BTreeMap::new();
    for id in ids {
        let p = snap
            .state
            .pages
            .iter()
            .find(|p| p.id == *id)
            .context("page not found")?;
        by_source
            .entry(p.source)
            .or_default()
            .push((p.id, p.index, p.rotation));
    }
    let mut out = Vec::new();
    let mut done = 0;
    for (sid, pages) in by_source {
        let info = snap.source(sid).context("unknown source")?;
        let file = sheaf_doc::file::load_pdf(&info.path, info.password.as_deref())?;
        let objs: Vec<_> = file.get_pages().into_values().collect();
        let engine = match spec {
            CropSpec::TrimWhite { .. } => {
                Some(PdfiumEngine::open(&info.path, info.password.as_deref())?)
            }
            CropSpec::Margins(_) => None,
        };
        let mut boxes = Vec::new();
        let mut planned = Vec::new();
        let mut sizes = Vec::new();
        for (id, index, rot) in pages {
            progress.check()?;
            let obj = *objs.get(index as usize).context("source page missing")?;
            let geom = sheaf_doc::file::page_geom(&file, obj);
            // Shown size before sheaf's rotation (the file's /Rotate applied).
            let [x0, y0, x1, y1] = geom.media;
            let (w, h) = if geom.rotate % 180 == 0 {
                (x1 - x0, y1 - y0)
            } else {
                (y1 - y0, x1 - x0)
            };
            let m = match spec {
                CropSpec::Margins(m) => unrotate(m, rot),
                CropSpec::TrimWhite { pad } => {
                    let e = engine.as_ref().expect("opened above");
                    content_margins(e, index, w, h, pad)?
                }
            };
            let (nw, nh) = (w - m.left - m.right, h - m.top - m.bottom);
            if nw < MIN_SIDE || nh < MIN_SIDE {
                bail!("those margins leave nothing of page {}", index + 1);
            }
            let a = geom.to_user(Pt {
                x: m.left,
                y: m.top,
            });
            let b = geom.to_user(Pt {
                x: w - m.right,
                y: h - m.bottom,
            });
            boxes.push((
                index,
                [a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y)],
            ));
            planned.push((id, boxes.len() as u32 - 1, (-m.left, -m.top)));
            sizes.push(PageSize {
                width: nw,
                height: nh,
            });
            done += 1;
            progress.set(done);
        }
        let bytes = sheaf_doc::save::cropped_copy(info, &boxes)?;
        let path = sheaf_doc::generated::store(&bytes)?;
        out.push(CroppedSource {
            info: SourceInfo {
                path,
                page_count: sizes.len() as u32,
                first_page: sizes[0],
                page_sizes: Some(sizes),
                bytes: bytes.len(),
                password: None,
            },
            pages: planned,
        });
    }
    Ok(out)
}

/// Margins down to the page's content (anything darker than near-white),
/// keeping `pad` points; none when the page is blank.
fn content_margins(e: &PdfiumEngine, index: u32, w: f32, h: f32, pad: f32) -> Result<Margins> {
    let dpi = 72.0;
    let raster = e.render(
        PageIndex(index),
        Job::Page {
            width: (w * dpi / 72.0).round().max(1.0) as u32,
            rotation: Rotation::R0,
        },
        PixelFormat::Rgba,
    )?;
    let (rw, rh) = (raster.width as usize, raster.height as usize);
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for y in 0..rh {
        for x in 0..rw {
            let p = &raster.data[(y * rw + x) * 4..][..4];
            let lum = (77 * p[0] as u32 + 150 * p[1] as u32 + 29 * p[2] as u32) >> 8;
            if lum < 235 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    if x0 == usize::MAX {
        return Ok(Margins::default());
    }
    let (sx, sy) = (w / rw as f32, h / rh as f32);
    Ok(Margins {
        left: (x0 as f32 * sx - pad).max(0.0),
        top: (y0 as f32 * sy - pad).max(0.0),
        right: (w - x1 as f32 * sx - pad).max(0.0),
        bottom: (h - y1 as f32 * sy - pad).max(0.0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn margins_follow_the_rotation() {
        let m = Margins {
            top: 1.0,
            right: 2.0,
            bottom: 3.0,
            left: 4.0,
        };
        assert_eq!(unrotate(m, Rotation::R0), m);
        // Turned clockwise, what shows on top is the page's left edge.
        assert_eq!(unrotate(m, Rotation::R90).left, 1.0);
        assert_eq!(unrotate(m, Rotation::R180).bottom, 1.0);
        assert_eq!(unrotate(m, Rotation::R270).right, 1.0);
    }
}
