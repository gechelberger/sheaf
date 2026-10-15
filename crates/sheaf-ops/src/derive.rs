//! Pages that show a generated copy of their source page: cropped, or
//! with the header/footer the file had drawn stripped out. Sources are
//! files, so the copies are written to the generated-file cache;
//! `Edit::RebasePages` then moves the document's pages onto them, keeping
//! ids, rotation, bookmarks and marks.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use sheaf_core::PageSize;
use sheaf_doc::{PageId, Rebase, Snapshot, SourceId, SourceInfo};
use sheaf_progress::{Progress, Unit};

/// A generated PDF holding copies of some pages, and the rebases that
/// move the document's pages onto it once it's a source.
pub struct DerivedSource {
    pub info: SourceInfo,
    /// (page, its index in `info`'s file, shift).
    pub pages: Vec<(PageId, u32, (f32, f32))>,
}

impl DerivedSource {
    /// The edit's pages, once `info` is added to the document as `source`.
    pub fn rebases(&self, source: SourceId) -> Vec<Rebase> {
        self.pages
            .iter()
            .map(|&(id, index, shift)| Rebase {
                id,
                source,
                index,
                shift,
            })
            .collect()
    }
}

/// Pages whose content has header/footer artifacts (Acrobat's or
/// sheaf's), as stripped copies. Empty when none have any.
pub fn strip_stamps(snap: &Snapshot, progress: &dyn Progress) -> Result<Vec<DerivedSource>> {
    progress.set_total(Some(snap.state.pages.len() as u64), Unit::Pages);
    let mut by_source: BTreeMap<SourceId, Vec<(PageId, u32)>> = BTreeMap::new();
    for p in snap.state.pages.iter() {
        by_source.entry(p.source).or_default().push((p.id, p.index));
    }
    let mut out = Vec::new();
    let mut done = 0;
    for (sid, pages) in by_source {
        let info = snap.source(sid).context("unknown source")?;
        let file = sheaf_doc::file::load_source(info)?;
        let objs: Vec<_> = file.get_pages().into_values().collect();
        let mut keep = Vec::new();
        for (id, index) in pages {
            progress.check()?;
            if let Some(&obj) = objs.get(index as usize)
                && let Ok(dict) = file.get_dictionary(obj)
                && sheaf_doc::stamps::has_artifacts(&file, dict, sheaf_doc::stamps::HEADER_FOOTER)
            {
                keep.push((id, index));
            }
            done += 1;
            progress.set(done);
        }
        if keep.is_empty() {
            continue;
        }
        let list: Vec<(u32, Option<[f32; 4]>)> = keep.iter().map(|&(_, i)| (i, None)).collect();
        let bytes = sheaf_doc::save::derived_copy(info, &list, &mut |pdf, d, _, _| {
            sheaf_doc::stamps::strip(pdf, d, sheaf_doc::stamps::HEADER_FOOTER);
            Ok(())
        })?;
        let path = sheaf_doc::generated::store(&bytes)?;
        let sizes: Vec<PageSize> = keep.iter().map(|&(_, i)| info.page_size(i)).collect();
        out.push(DerivedSource {
            info: SourceInfo {
                path,
                page_count: sizes.len() as u32,
                first_page: sizes[0],
                page_sizes: Some(sizes),
                bytes: bytes.len(),
                password: None,
                data: None,
            },
            pages: keep
                .iter()
                .enumerate()
                .map(|(k, &(id, _))| (id, k as u32, (0.0, 0.0)))
                .collect(),
        });
    }
    Ok(out)
}

/// Pages with `wm` drawn on (its kind stripped first), and pages outside
/// `scope` that have one of its kind stripped; `None` strips only. Each
/// page's sheaf rotation is passed on, so the mark sits upright as the
/// page is shown.
pub fn watermark(
    snap: &Snapshot,
    behind: bool,
    wm: Option<&sheaf_doc::watermark::Watermark>,
    scope: &[PageId],
    progress: &dyn Progress,
) -> Result<Vec<DerivedSource>> {
    let kinds = if behind {
        sheaf_doc::stamps::BACKGROUND
    } else {
        sheaf_doc::stamps::WATERMARK
    };
    progress.set_total(Some(snap.state.pages.len() as u64), Unit::Pages);
    // (page, index in source, draw?, its sheaf rotation)
    let mut by_source: BTreeMap<SourceId, Vec<(PageId, u32, bool, i32)>> = BTreeMap::new();
    for p in snap.state.pages.iter() {
        let draw = wm.is_some() && scope.contains(&p.id);
        by_source
            .entry(p.source)
            .or_default()
            .push((p.id, p.index, draw, p.rotation.degrees()));
    }
    let mut out = Vec::new();
    let mut done = 0;
    for (sid, pages) in by_source {
        let info = snap.source(sid).context("unknown source")?;
        let file = sheaf_doc::file::load_source(info)?;
        let objs: Vec<_> = file.get_pages().into_values().collect();
        let mut keep = Vec::new();
        for (id, index, draw, rot) in pages {
            progress.check()?;
            let has = objs
                .get(index as usize)
                .and_then(|&o| file.get_dictionary(o).ok())
                .is_some_and(|d| sheaf_doc::stamps::has_artifacts(&file, d, kinds));
            if draw || has {
                keep.push((id, index, draw, rot));
            }
            done += 1;
            progress.set(done);
        }
        if keep.is_empty() {
            continue;
        }
        let list: Vec<(u32, Option<[f32; 4]>)> = keep.iter().map(|k| (k.1, None)).collect();
        let bytes = sheaf_doc::save::derived_copy(info, &list, &mut |pdf, d, geom, at| {
            sheaf_doc::stamps::strip(pdf, d, kinds);
            let (_, _, draw, rot) = keep[at];
            if draw && let Some(wm) = wm {
                sheaf_doc::watermark::install(pdf, d, wm, geom, rot)?;
            }
            Ok(())
        })?;
        let path = sheaf_doc::generated::store(&bytes)?;
        let sizes: Vec<PageSize> = keep.iter().map(|k| info.page_size(k.1)).collect();
        out.push(DerivedSource {
            info: SourceInfo {
                path,
                page_count: sizes.len() as u32,
                first_page: sizes[0],
                page_sizes: Some(sizes),
                bytes: bytes.len(),
                password: None,
                data: None,
            },
            pages: keep
                .iter()
                .enumerate()
                .map(|(k, &(id, ..))| (id, k as u32, (0.0, 0.0)))
                .collect(),
        });
    }
    Ok(out)
}

/// The watermark (or background) settings on the first page that has
/// one sheaf wrote, for the dialog to start from.
pub fn current_watermark(snap: &Snapshot, behind: bool) -> Option<sheaf_doc::watermark::Watermark> {
    let mut by_source: BTreeMap<SourceId, Vec<u32>> = BTreeMap::new();
    for p in snap.state.pages.iter() {
        by_source.entry(p.source).or_default().push(p.index);
    }
    for (sid, indexes) in by_source {
        let info = snap.source(sid)?;
        let file = sheaf_doc::file::load_source(info).ok()?;
        let objs: Vec<_> = file.get_pages().into_values().collect();
        for i in indexes {
            if let Some(d) = objs
                .get(i as usize)
                .and_then(|&o| file.get_dictionary(o).ok())
                && let Some(wm) = sheaf_doc::watermark::read(&file, d, behind)
            {
                return Some(wm);
            }
        }
    }
    None
}
