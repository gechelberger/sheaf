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
        let file = sheaf_doc::file::load_pdf(&info.path, info.password.as_deref())?;
        let objs: Vec<_> = file.get_pages().into_values().collect();
        let mut keep = Vec::new();
        for (id, index) in pages {
            progress.check()?;
            if let Some(&obj) = objs.get(index as usize)
                && let Ok(dict) = file.get_dictionary(obj)
                && sheaf_doc::stamps::has_artifacts(&file, dict)
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
        let bytes = sheaf_doc::save::derived_copy(info, &list, true)?;
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
