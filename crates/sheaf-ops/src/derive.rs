//! Pages that show a generated copy of their source page: cropped, or
//! with the header/footer the file had drawn stripped out. The copies are
//! made in memory and interned into the document's store, named after
//! their originals; `Edit::RebasePages` then moves the document's pages
//! onto them, keeping ids, rotation, bookmarks, marks and OCR text.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use in_progress::Progress;
use sheaf_doc::content::{ContentError, PaginationKind};
use sheaf_doc::geom::Geom;
use sheaf_doc::store::{BlobKind, MemoryKind, Origin};
use sheaf_doc::{Derived, PAGES, PageId, Rebase, Snapshot, SourceId, SourceInfo, Store};

/// A generated PDF holding copies of some pages, and the rebases that
/// move the document's pages onto it once it's a source.
pub struct DerivedSource {
    pub info: SourceInfo,
    /// (page, its index in `info`'s file).
    pub pages: Vec<(PageId, u32)>,
}

impl DerivedSource {
    /// The edit's pages, once `info` is added to the document as `source`.
    pub fn rebases(&self, source: SourceId) -> Vec<Rebase> {
        self.pages
            .iter()
            .map(|&(id, index)| Rebase { id, source, index })
            .collect()
    }
}

/// `bytes`, sheaf's copy of pages `pages` of source `from` (`original`),
/// as a source in `store`, under the original's name.
pub(crate) fn copy_of(
    store: &Store,
    bytes: Vec<u8>,
    original: &SourceInfo,
    from: SourceId,
    pages: Vec<u32>,
) -> Result<SourceInfo> {
    let kind = BlobKind::Pdf {
        password: None,
        origin: Origin::Memory(MemoryKind::Derived),
    };
    Ok(SourceInfo {
        src: store.open(bytes.into(), kind)?,
        name: original.name.clone(),
        derived: Some(Derived { from, pages }),
    })
}

/// Whether page `index` of `info` has pagination artifacts of `kinds`
/// that `stamps::strip` removes, by its blocks: an error names `page`, its
/// place in the document.
fn has_kinds(
    info: &SourceInfo,
    index: u32,
    page: u32,
    kinds: &[PaginationKind],
) -> Result<bool, ContentError> {
    let blocks = info.src.blocks(index).map_err(|e| e.on_page(page))?;
    Ok(sheaf_doc::stamps::stripped(&blocks, kinds).next().is_some())
}

/// Pages whose content has header/footer artifacts sheaf or Acrobat drew,
/// as stripped copies in `store`; an author's own stay. Empty when none
/// have any; a source with none is never loaded.
pub fn strip_stamps(
    store: &Store,
    snap: &Snapshot,
    progress: &dyn Progress,
) -> Result<Vec<DerivedSource>> {
    let kinds = sheaf_doc::stamps::HEADER_FOOTER;
    progress.set_total(Some(snap.state.pages.len() as u64), PAGES);
    // (page, index in source, place in the document)
    let mut by_source: BTreeMap<SourceId, Vec<(PageId, u32, u32)>> = BTreeMap::new();
    for (n, p) in snap.state.pages.iter().enumerate() {
        by_source
            .entry(p.source)
            .or_default()
            .push((p.id, p.index, n as u32));
    }
    let mut out = Vec::new();
    let mut done = 0;
    for (sid, pages) in by_source {
        let info = snap.source(sid).context("unknown source")?;
        let mut keep = Vec::new();
        for (id, index, n) in pages {
            progress.check()?;
            if has_kinds(info, index, n, kinds)? {
                keep.push((id, index, n));
            }
            done += 1;
            progress.set(done);
        }
        if keep.is_empty() {
            continue;
        }
        let list: Vec<(u32, Option<[f32; 4]>)> = keep.iter().map(|k| (k.1, None)).collect();
        let bytes = sheaf_doc::save::derived_copy(info, &list, &mut |pdf, d, _, at| {
            sheaf_doc::stamps::strip(pdf, d, keep[at].2, kinds)?;
            Ok(())
        })?;
        let pages = keep.iter().map(|k| k.1).collect();
        out.push(DerivedSource {
            info: copy_of(store, bytes, info, sid, pages)?,
            pages: keep
                .iter()
                .enumerate()
                .map(|(k, &(id, ..))| (id, k as u32))
                .collect(),
        });
    }
    Ok(out)
}

/// Pages with `wm` drawn on (sheaf's and Acrobat's of its kind stripped
/// first), and pages outside `scope` that have one of those stripped;
/// `None` strips only. Each
/// page's rotation is passed on, so the mark sits upright as the page is
/// shown. The copies are sources in `store`.
pub fn watermark(
    store: &Store,
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
    progress.set_total(Some(snap.state.pages.len() as u64), PAGES);
    // (page, index in source, place in the document, draw?, its rotation)
    let mut by_source: BTreeMap<SourceId, Vec<_>> = BTreeMap::new();
    for (n, p) in snap.state.pages.iter().enumerate() {
        let draw = wm.is_some() && scope.contains(&p.id);
        by_source
            .entry(p.source)
            .or_default()
            .push((p.id, p.index, n as u32, draw, p.rotation));
    }
    let mut out = Vec::new();
    let mut done = 0;
    for (sid, pages) in by_source {
        let info = snap.source(sid).context("unknown source")?;
        let mut keep = Vec::new();
        for page @ (_, index, n, draw, _) in pages {
            progress.check()?;
            if draw || has_kinds(info, index, n, kinds)? {
                keep.push(page);
            }
            done += 1;
            progress.set(done);
        }
        if keep.is_empty() {
            continue;
        }
        let list: Vec<(u32, Option<[f32; 4]>)> = keep.iter().map(|k| (k.1, None)).collect();
        let bytes = sheaf_doc::save::derived_copy(info, &list, &mut |pdf, d, geom, at| {
            let (_, _, n, draw, rotate) = keep[at];
            sheaf_doc::stamps::strip(pdf, d, n, kinds)?;
            if draw && let Some(wm) = wm {
                sheaf_doc::watermark::install(pdf, d, wm, &Geom { rotate, ..*geom })?;
            }
            Ok(())
        })?;
        let pages = keep.iter().map(|k| k.1).collect();
        out.push(DerivedSource {
            info: copy_of(store, bytes, info, sid, pages)?,
            pages: keep
                .iter()
                .enumerate()
                .map(|(k, &(id, ..))| (id, k as u32))
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use lopdf::{Stream, dictionary};
    use sheaf_doc::store::BlobKind;
    use sheaf_doc::{Document, Edit, FixedCount};

    use super::*;
    use crate::NoProgress;

    const HEADER: &[u8] =
        b"/Artifact <</Type /Pagination /Subtype /Header /SheafStamp true>> BDC (h) Tj EMC";
    const AUTHORED: &[u8] = b"/Artifact <</Type /Pagination /Subtype /Header>> BDC (h) Tj EMC";

    /// A document of a page for each of `contents`.
    fn doc(contents: &[&[u8]]) -> Document {
        let mut f = sheaf_doc::forms::fixture::Form::new(contents.len());
        for (&page, content) in f.pages.clone().iter().zip(contents) {
            let stream = f
                .pdf
                .add_object(Stream::new(dictionary! {}, content.to_vec()));
            f.dict(page).set("Contents", stream);
        }
        let store = Store::new();
        let kind = BlobKind::file(Path::new("t.pdf"), None);
        let primary = SourceInfo {
            src: store.open(f.bytes().into(), kind).unwrap(),
            name: "t.pdf".into(),
            derived: None,
        };
        Document::new(store, primary, &[], Box::new(FixedCount(10)))
    }

    /// Pages are picked by their blocks: those with a header sheaf drew,
    /// not one with an author's own, and none from a source with none.
    #[test]
    fn pages_are_picked_by_their_blocks() {
        let d = doc(&[b"0 g", HEADER, AUTHORED]);
        let ids: Vec<PageId> = d.state().pages.iter().map(|p| p.id).collect();
        let copies = strip_stamps(d.store(), &d.snapshot(), &NoProgress).unwrap();
        assert_eq!(copies.len(), 1);
        assert_eq!(copies[0].pages, [(ids[1], 0)]);
        assert!(
            !copies[0]
                .info
                .src
                .blocks(0)
                .unwrap()
                .has(sheaf_doc::stamps::HEADER_FOOTER)
        );
        let d = doc(&[b"0 g"]);
        assert!(
            strip_stamps(d.store(), &d.snapshot(), &NoProgress)
                .unwrap()
                .is_empty()
        );
        assert!(
            watermark(d.store(), &d.snapshot(), false, None, &[], &NoProgress)
                .unwrap()
                .is_empty()
        );
    }

    /// A page whose content can't be read fails the op, named by its
    /// place in the document.
    #[test]
    fn a_page_that_cant_be_read_is_named() {
        let mut d = doc(&[b"0 g", b"/A BMC"]);
        let ids: Vec<PageId> = d.state().pages.iter().map(|p| p.id).collect();
        let first = Edit::MovePages {
            ids: vec![ids[1]],
            before: Some(ids[0]),
        };
        d.apply(first, "move").unwrap();
        let Err(e) = strip_stamps(d.store(), &d.snapshot(), &NoProgress) else {
            panic!("the page was read");
        };
        assert_eq!(
            e.to_string(),
            "page 1's content can't be read; nothing was changed"
        );
    }
}
