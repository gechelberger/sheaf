//! Opening a file (M10.5 §14.2), the one open the app, the command line
//! and every other headless caller share. [`start`] makes the document
//! from the pages alone, so the first page paints at once; [`finish`]
//! reads what the file says besides them, on any thread; and
//! [`Document::ready`] makes that the opened version, after which edits
//! are taken. [`file`] does the three in a row.
//!
//! What [`finish`] reads: the file's marks, the stamps' pages and the
//! files they hold, and the other tools' comments, its page labels, the
//! header/footer sheaf drew into it, its form, attachments, portfolio
//! schema and layers, and its bookmarks ([`outline`]). Marks another
//! program copied, which say an id an earlier mark has, are given new
//! ids. All of it is read from the
//! source alone, its [`LazyPdf`](lopdf::LazyPdf) and page table, with no
//! eager load of the file. Nothing in it lexes a page's content: of a
//! page it reads its dictionary and annotations, and elsewhere only the
//! objects the facts name.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use in_progress::{NoProgress, Progress};

use crate::annot::{Annot, AnnotId, AnnotKind};
use crate::assets::SourceAssets;
use crate::attachments::Attachment;
use crate::file::ForeignAnnot;
use crate::forms::{FileForm, FormFacts};
use crate::history::HistoryPolicy;
use crate::labels::LabelRange;
use crate::layers::Layer;
use crate::model::{FlatOutlineItem, PageId};
use crate::portfolio::Collection;
use crate::stamps::HeaderFooter;
use crate::{Document, PAGES, Source, SourceInfo, Store};

mod outline;
#[cfg(test)]
pub(crate) mod tests;

pub use outline::outline;

/// What a file says about itself when it opens, every page by its index
/// in the source's page table.
#[derive(Debug, Default)]
pub struct Facts {
    /// sheaf's own marks, by page.
    pub marks: HashMap<u32, Vec<Annot>>,
    /// The marks given new ids whose `/NM`s change (see `fresh_ids`), in
    /// page order and then `/Annots` order.
    pub renamed: Vec<Renamed>,
    /// Every other tool's comments.
    pub foreign: Vec<ForeignAnnot>,
    /// What sheaf's marks hold: their stamps' pages and attachments' files.
    pub assets: SourceAssets,
    /// The file's `/PageLabels` ranges.
    pub labels: Option<Vec<LabelRange>>,
    /// The header/footer sheaf drew into the file, and the pages it's on
    /// (none: all).
    pub header_footer: Option<(HeaderFooter, Option<Vec<usize>>)>,
    /// The form: its fields and widgets, and what sheaf shows about it.
    pub form: FileForm,
    /// Embedded files, document-level and on annotations.
    pub attachments: Vec<Attachment>,
    /// Set when the file is a PDF Portfolio.
    pub collection: Option<Collection>,
    /// Optional content groups, in display order.
    pub layers: Vec<Layer>,
    /// The bookmarks, in pre-order.
    pub outline: Vec<FlatOutlineItem>,
}

/// A mark the open gave a new id, whose `/NM` the next save of its page
/// changes (see `fresh_ids`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Renamed {
    /// Its page, by index in the source.
    pub page: u32,
    /// The id it had, which the file's `/NM` says until the page is saved.
    pub was: AnnotId,
    /// The id it has.
    pub id: AnnotId,
}

/// What the view shows beside a document that its state doesn't hold, as
/// the file opened: [`Document::ready`] gives it.
#[derive(Debug, Default)]
#[cfg_attr(any(test, feature = "fixtures"), derive(PartialEq))]
pub struct ViewFacts {
    /// The marks the open gave new ids, and the ids they had, which their
    /// `/NM`s still say until they're saved.
    pub renamed: Vec<(PageId, AnnotId)>,
    /// Every other tool's comments.
    pub foreign: Vec<ForeignAnnot>,
    pub attachments: Vec<Attachment>,
    pub collection: Option<Collection>,
    pub layers: Vec<Layer>,
    /// What sheaf shows about the form.
    pub form: FormFacts,
}

/// A file opened with no window.
pub struct Opened {
    pub doc: Document,
    pub view: ViewFacts,
}

/// `primary` as a document while it opens: its pages, as the page table
/// [`SourceInfo::read`] built has them, and nothing else, so the first
/// page can paint at once. It reads nothing of its own. Edits are refused
/// with [`NotReady`](crate::model::NotReady) until [`Document::ready`]
/// takes [`finish`]'s facts.
pub fn start(store: Arc<Store>, primary: SourceInfo, policy: Box<dyn HistoryPolicy>) -> Document {
    Document::loading(store, primary, policy)
}

/// `path` opened as the app opens it, with no window: [`start`],
/// [`finish`] and [`Document::ready`] in a row, in a store of its own.
pub fn file(path: &Path, password: Option<&str>) -> Result<Opened> {
    let store = Store::new();
    let primary = SourceInfo::read(&store, path, password)?;
    let src = primary.src.clone();
    let mut doc = start(store, primary, crate::history::default_policy());
    let view = doc.ready(finish(&src, &NoProgress))?;
    Ok(Opened { doc, view })
}

/// Everything `src` says about itself besides its pages. One pass over
/// the page table's annotations finds the marks, what they hold, and the
/// other tools' comments; then the form, the catalog's facts and the
/// outline. Each reader passes over what it can't read, so
/// this never fails once the source has opened. `p` counts the pages of
/// the first pass, the longest.
pub fn finish(src: &Source, p: &dyn Progress) -> Facts {
    let pdf = src.pdf();
    p.set_total(Some(u64::from(src.page_count())), PAGES);
    let pages = src.page_ids().inspect(|_| p.inc(1));
    let mut found = crate::file::annots_in(pdf, pages);
    let hash = src.id().hash;
    let mut rng = fastrand::Rng::with_seed(hash as u64 ^ (hash >> 64) as u64);
    let renamed = fresh_ids(&mut found.sheaf, &mut rng);
    follow_renamed(&mut found.foreign, &found.sheaf, &renamed);
    Facts {
        marks: found.sheaf,
        renamed,
        foreign: found.foreign,
        assets: found.assets,
        labels: crate::labels::label_ranges(pdf),
        header_footer: crate::stamps::from_catalog(pdf),
        form: crate::forms::read(src),
        attachments: crate::attachments::attachments_in(pdf, src.page_ids()),
        collection: crate::portfolio::collection(pdf),
        layers: crate::layers::layers_in(pdf),
        outline: outline(src),
    }
}

/// New ids for the marks whose id an earlier mark has, in page order and
/// then `/Annots` order (M10.5 §3.6): copies another program made, which
/// say the `/NM sheaf:<id>` they were copied with (Acrobat's Insert Pages
/// makes them, since `/NM` need only be unique on its page). The first
/// keeps its id. Each new one is drawn from `rng`, which the open seeds
/// with the file's hash so that the same bytes open the same, below 2^53
/// and taken by no mark in the file. A mark sheaf made has no name of its
/// own, so its next write gives it the new id's `/NM`, and a reply copied
/// with it stays in its thread; those are the ones given back. One that
/// keeps another tool's `/NM` keeps it, and only its id changes.
fn fresh_ids(marks: &mut HashMap<u32, Vec<Annot>>, rng: &mut fastrand::Rng) -> Vec<Renamed> {
    let mut taken: HashSet<AnnotId> = marks.values().flatten().map(|a| a.id).collect();
    let mut seen = HashSet::new();
    let mut renamed = Vec::new();
    let mut pages: Vec<u32> = marks.keys().copied().collect();
    pages.sort_unstable();
    for page in pages {
        let list = marks.get_mut(&page).expect("a page with marks");
        // The names the copies on this page had, and have now.
        let mut names = HashMap::new();
        let mut copies = Vec::new();
        for (i, a) in list.iter_mut().enumerate() {
            if seen.insert(a.id) {
                continue;
            }
            let old = a.id;
            a.id = loop {
                let id = AnnotId(rng.u64(1..1 << 53));
                if taken.insert(id) {
                    break id;
                }
            };
            if a.name.is_empty() {
                let [was, _] = old.sheaf_names();
                names.insert(was, a.name());
                renamed.push(Renamed {
                    page,
                    was: old,
                    id: a.id,
                });
            }
            copies.push(i);
        }
        for i in copies {
            if let AnnotKind::Reply { parent } | AnnotKind::Status { parent, .. } =
                &mut list[i].kind
                && let Some(new) = names.get(parent)
            {
                *parent = new.clone();
            }
        }
    }
    renamed
}

/// Other tools' comments in the thread of a mark `fresh_ids` renamed, as
/// a reply to a copy is: they name it as the file does, by its old id, and
/// now name it by its new one, on its page. Where a mark on that page
/// still has the old name, they're left naming that one.
fn follow_renamed(
    foreign: &mut [ForeignAnnot],
    marks: &HashMap<u32, Vec<Annot>>,
    renamed: &[Renamed],
) {
    if renamed.is_empty() {
        return;
    }
    let names: HashMap<(u32, String), String> = renamed
        .iter()
        .map(|r| {
            let ([was, _], [now, _]) = (r.was.sheaf_names(), r.id.sheaf_names());
            ((r.page, was), now)
        })
        .collect();
    for f in foreign {
        let Some(parent) = &mut f.parent else {
            continue;
        };
        let Some(new) = names.get(&(f.page, parent.clone())) else {
            continue;
        };
        let kept = marks
            .get(&f.page)
            .is_some_and(|list| list.iter().any(|a| a.name() == *parent));
        if !kept {
            *parent = new.clone();
        }
    }
}
