//! Crop pages (Document > Crop Pages…). A cropped page becomes a page of
//! a PDF sheaf makes in memory, whose `/CropBox` is the new shown area:
//! the layout sizes it, and PDFium renders and maps text for it, like any
//! page. `Edit::RebasePages` moves the document's page onto it, keeping
//! its id, rotation, bookmarks, marks and OCR text. A crop is a box edit:
//! all of those are in PageSpace, which the copy keeps, so they stay on
//! the same content.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sheaf_core::{PageIndex, PageSize, Rotation};
use sheaf_doc::geom::Geom;
use sheaf_doc::{Box2, PageId, Snapshot, SourceId, Store};
use sheaf_image::raster::{PixelFormat, content_bounds};
use sheaf_progress::{Progress, Unit};
use sheaf_render::pdfium::PdfiumEngine;
use sheaf_render::{Job, Renderer};

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

/// Plan the crop of `ids` in `snap`; the cropped copies are sources in
/// `store`.
pub fn plan(
    store: &Store,
    snap: &Snapshot,
    ids: &[PageId],
    spec: CropSpec,
    progress: &dyn Progress,
) -> Result<Vec<CroppedSource>> {
    progress.set_total(Some(ids.len() as u64), Unit::Pages);
    // Pages grouped by the source they show now, each with its geometry as
    // it shows and the turn PDFium adds to its source page's.
    let mut by_source: BTreeMap<SourceId, Vec<(PageId, u32, Geom, Rotation)>> = BTreeMap::new();
    for id in ids {
        let p = snap
            .state
            .pages
            .iter()
            .find(|p| p.id == *id)
            .context("page not found")?;
        by_source.entry(p.source).or_default().push((
            p.id,
            p.index,
            snap.geom(p),
            snap.render_delta(p),
        ));
    }
    let mut out = Vec::new();
    let mut done = 0;
    for (sid, pages) in by_source {
        let info = snap.source(sid).context("unknown source")?;
        let engine = match spec {
            CropSpec::TrimWhite { .. } => Some(crate::engine_for(info)?),
            CropSpec::Margins(_) => None,
        };
        let mut boxes = Vec::new();
        let mut planned = Vec::new();
        for (id, index, geom, delta) in pages {
            progress.check()?;
            let shown = geom.shown();
            let m = match spec {
                CropSpec::Margins(m) => m,
                CropSpec::TrimWhite { pad } => {
                    let e = engine.as_ref().expect("opened above");
                    content_margins(e, index, shown, delta, pad)?
                }
            };
            // What's left as it shows, back in PageSpace.
            let kept = Box2 {
                x: m.left,
                y: m.top,
                w: shown.width - m.left - m.right,
                h: shown.height - m.top - m.bottom,
            };
            if kept.w < MIN_SIDE || kept.h < MIN_SIDE {
                bail!("those margins leave nothing of page {}", index + 1);
            }
            let crop = geom.from_view().apply_box(kept);
            boxes.push((index, [crop.x, crop.y, crop.right(), crop.bottom()]));
            planned.push((id, boxes.len() as u32 - 1));
            done += 1;
            progress.set(done);
        }
        let bytes = sheaf_doc::save::cropped_copy(info, &boxes)?;
        let pages = boxes.iter().map(|&(i, _)| i).collect();
        out.push(CroppedSource {
            info: crate::derive::copy_of(store, bytes, info, sid, pages)?,
            pages: planned,
        });
    }
    Ok(out)
}

/// Margins down to the content of page `index` as it shows, `shown` and
/// turned `delta` beyond its source page (anything darker than
/// near-white), keeping `pad` points; none when the page is blank.
fn content_margins(
    e: &PdfiumEngine,
    index: u32,
    shown: PageSize,
    delta: Rotation,
    pad: f32,
) -> Result<Margins> {
    let (w, h) = (shown.width, shown.height);
    let raster = e.render(
        PageIndex(index),
        Job::Page {
            width: w.round().max(1.0) as u32,
            rotation: delta,
        },
        PixelFormat::Rgba,
    )?;
    let (rw, rh) = (raster.width, raster.height);
    let gray = raster.to_gray().context("short render")?;
    let Some((x0, y0, x1, y1)) = content_bounds(&gray, 235) else {
        return Ok(Margins::default());
    };
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
    use std::path::Path;

    use sheaf_doc::geom::Rect;
    use sheaf_doc::store::BlobKind;
    use sheaf_doc::{Document, Edit, FixedCount, SourceInfo};

    use super::*;

    /// Margins are taken off the sides as the page shows, turned by its
    /// `/Rotate` and by sheaf: on a page the file turns 90°, the top
    /// margin comes off its user space's left, and turned 90° more in
    /// sheaf, off its bottom.
    #[test]
    fn margins_crop_the_shown_sides() {
        let m = Margins {
            top: 10.0,
            right: 20.0,
            bottom: 30.0,
            left: 40.0,
        };
        let mut f = sheaf_doc::forms::fixture::Form::new(1);
        let page = f.pages[0];
        let media = [100.0f32, 50.0, 712.0, 842.0].map(lopdf::Object::from);
        f.dict(page).set("MediaBox", media.to_vec());
        f.dict(page).set("Rotate", 90);
        let bytes = f.bytes();
        for (by, want) in [
            (Rotation::R0, Rect::new(110.0, 90.0, 682.0, 822.0)),
            (Rotation::R90, Rect::new(120.0, 60.0, 672.0, 812.0)),
        ] {
            let store = Store::new();
            let primary = SourceInfo {
                src: store
                    .open(
                        bytes.clone().into(),
                        BlobKind::file(Path::new("t.pdf"), None),
                    )
                    .unwrap(),
                name: "t.pdf".into(),
                derived: None,
            };
            let mut doc = Document::new(store, primary, &[], Box::new(FixedCount(10)));
            let id = doc.state().pages[0].id;
            let rotate = Edit::RotatePages { ids: vec![id], by };
            doc.apply(rotate, "rotate").unwrap();
            let shown = doc.page_size(&doc.state().pages[0]);
            let plan = plan(
                doc.store(),
                &doc.snapshot(),
                &[id],
                CropSpec::Margins(m),
                &crate::NoProgress,
            )
            .unwrap();
            let cropped = plan[0].info.src.geom(0);
            assert_eq!(cropped.crop, want, "{by:?}");
            let left = doc.geom(&doc.state().pages[0]);
            let size = Geom { crop: want, ..left }.shown();
            assert_eq!(size.width, shown.width - 60.0, "{by:?}");
            assert_eq!(size.height, shown.height - 40.0, "{by:?}");
        }
    }
}
