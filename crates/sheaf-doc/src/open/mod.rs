//! Opening a file (M10.5 §14.2), the one open the app, the command line
//! and every other headless caller share. [`start`] makes the document
//! from the pages alone, so the first page paints at once; [`finish`]
//! reads what the file says besides them, on any thread; and
//! [`Document::ready`] makes that the opened version, after which edits
//! are taken. [`file`] does the three in a row.
//!
//! What [`finish`] reads: the file's marks and the other tools' comments,
//! its page labels, the header/footer sheaf drew into it, its form,
//! attachments, portfolio schema and layers, and its bookmarks
//! ([`outline`]). All of it is read from the source alone, its
//! [`LazyPdf`](lopdf::LazyPdf) and page table, with no eager load of the
//! file. Nothing in it lexes a page's content: of a page it reads its
//! dictionary and annotations, and elsewhere only the objects the facts
//! name.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use sheaf_progress::{NoProgress, Progress, Unit};

use crate::annot::Annot;
use crate::attachments::Attachment;
use crate::file::ForeignAnnot;
use crate::forms::{FileForm, FormFacts};
use crate::history::HistoryPolicy;
use crate::labels::LabelRange;
use crate::layers::Layer;
use crate::model::FlatOutlineItem;
use crate::portfolio::Collection;
use crate::stamps::HeaderFooter;
use crate::{Document, Source, SourceInfo, Store};

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
    /// Every other tool's comments.
    pub foreign: Vec<ForeignAnnot>,
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

/// What the view shows beside a document that its state doesn't hold, as
/// the file opened: [`Document::ready`] gives it.
#[derive(Debug, Default)]
#[cfg_attr(any(test, feature = "fixtures"), derive(PartialEq))]
pub struct ViewFacts {
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
/// the page table's annotations finds the marks and the other tools'
/// comments, and what the marks keep in this machine's cache is made
/// again from the file where it's missing; then the form, the catalog's
/// facts and the outline. Each reader passes over what it can't read, so
/// this never fails once the source has opened. `p` counts the pages of
/// the first pass, the longest.
pub fn finish(src: &Source, p: &dyn Progress) -> Facts {
    let pdf = src.pdf();
    p.set_total(Some(u64::from(src.page_count())), Unit::Pages);
    let pages = src.page_ids().inspect(|_| p.inc(1));
    let (found, cached) = crate::file::annots_and_cached(pdf, pages);
    crate::file::recover_cached(pdf, &cached);
    Facts {
        marks: found.sheaf,
        foreign: found.foreign,
        labels: crate::labels::label_ranges(pdf),
        header_footer: crate::stamps::from_catalog(pdf),
        form: crate::forms::read(src),
        attachments: crate::attachments::attachments_in(pdf, src.page_ids()),
        collection: crate::portfolio::collection(pdf),
        layers: crate::layers::layers_in(pdf),
        outline: outline(src),
    }
}
