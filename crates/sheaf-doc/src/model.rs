//! Virtual document, edits, and snapshot history.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use im::Vector;
use serde::{Deserialize, Serialize};
use sheaf_core::{DestView, PageSize, Rotation};

use crate::annot::{Annot, AnnotId, AnnotKind};
use crate::history::{HistoryPolicy, HistoryStats};
use crate::text_layer::TextLayer;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SourceId(pub u32);

/// Stable identity of a page in the virtual document. Never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PageId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct OutlineId(pub u64);

/// What the model knows about an opened file. Sizes are as the engine
/// renders them (after the file's own `/Rotate`). `page_sizes` may be absent
/// briefly after open while the engine enumerates them.
#[derive(Debug, Clone)]
pub struct SourceInfo {
    pub path: PathBuf,
    pub page_count: u32,
    pub first_page: PageSize,
    pub page_sizes: Option<Vec<PageSize>>,
    /// File size, for the history memory estimate.
    pub bytes: usize,
    /// The password the file was opened with, if it needed one. Saves
    /// decrypt with it; the written file is not re-encrypted.
    pub password: Option<String>,
}

impl SourceInfo {
    pub fn page_size(&self, index: u32) -> PageSize {
        self.page_sizes
            .as_ref()
            .and_then(|s| s.get(index as usize).copied())
            .unwrap_or(self.first_page)
    }
}

/// One page of the virtual document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRef {
    pub id: PageId,
    pub source: SourceId,
    pub index: u32,
    /// Extra rotation on top of the source page as rendered.
    pub rotation: Rotation,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutlineNode {
    pub id: OutlineId,
    pub title: String,
    pub target: Option<PageId>,
    /// Where on `target` it lands; None is the top of the page.
    pub view: Option<DestView>,
    pub color: Option<[u8; 3]>,
    pub children: Vector<OutlineNode>,
}

/// A bookmark as read from a file: pre-order with depth.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlatOutlineItem {
    pub title: String,
    pub page: Option<u32>,
    pub depth: u16,
    /// Where on `page` it lands (see [`OutlineNode::view`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<DestView>,
}

/// One bookmark of a subtree inserted by [`Edit::OutlineInsertTree`], in
/// pre-order. `depth` is relative to the insert point: 0 is a sibling there,
/// 1 a child of the item above at 0, and so on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewOutlineItem {
    pub title: String,
    pub target: Option<PageId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<DestView>,
    pub depth: u16,
}

/// One immutable snapshot of the document.
#[derive(Debug, Clone, PartialEq)]
pub struct DocState {
    pub pages: Vector<PageRef>,
    pub outline: Vector<OutlineNode>,
    /// Our markup, per page. Annotations already in a source file are not
    /// here unless they were written by sheaf (see `annot`).
    pub annots: im::HashMap<PageId, Vector<Annot>>,
    /// OCR text not yet written to a file. A save bakes these into the
    /// page content and `rebase_to_saved` drops them, so presence here
    /// means unsaved (see `text_layer`).
    pub text_layers: im::HashMap<PageId, Arc<TextLayer>>,
    /// Page labels, anchored to the pages ranges start at. None: the file
    /// has none, or they haven't been read.
    pub labels: Option<Vector<crate::labels::LabelSpec>>,
    /// The header/footer (with Bates numbers) sheaf draws on pages.
    pub header_footer: Option<Arc<crate::stamps::HeaderFooter>>,
}

impl DocState {
    /// Every page's label, when the document has page labels.
    pub fn page_labels(&self) -> Option<Vec<String>> {
        let specs = self.labels.as_ref()?;
        let ids: Vec<PageId> = self.pages.iter().map(|p| p.id).collect();
        let ranges = crate::labels::resolve_specs(&specs.iter().cloned().collect::<Vec<_>>(), &ids);
        if ranges.is_empty() {
            return None;
        }
        Some(crate::labels::labels_for(&ranges, ids.len() as u32))
    }

    /// The label ranges by page position, when there are labels.
    pub fn label_ranges(&self) -> Option<Vec<crate::labels::LabelRange>> {
        let specs = self.labels.as_ref()?;
        let ids: Vec<PageId> = self.pages.iter().map(|p| p.id).collect();
        Some(crate::labels::resolve_specs(
            &specs.iter().cloned().collect::<Vec<_>>(),
            &ids,
        ))
    }

    pub fn annots_on(&self, page: PageId) -> Option<&Vector<Annot>> {
        self.annots.get(&page)
    }

    pub fn text_layer(&self, page: PageId) -> Option<&Arc<TextLayer>> {
        self.text_layers.get(&page)
    }

    pub fn has_text_layer(&self, page: PageId) -> bool {
        self.text_layers.contains_key(&page)
    }

    /// Locate an annotation by id on any page.
    pub fn find_annot(&self, id: AnnotId) -> Option<(PageId, &Annot)> {
        self.annots
            .iter()
            .find_map(|(page, list)| list.iter().find(|a| a.id == id).map(|a| (*page, a)))
    }

    /// The label for the next connector pair: one past the highest numeric
    /// label in use.
    pub fn next_connector_label(&self) -> String {
        let max = self
            .annots
            .values()
            .flatten()
            .filter_map(|a| match &a.kind {
                AnnotKind::Connector { label, .. } => label.trim().parse::<u32>().ok(),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        (max + 1).to_string()
    }

    pub fn index_of(&self, id: PageId) -> Option<usize> {
        self.pages.iter().position(|p| p.id == id)
    }

    /// Pre-order walk of the outline with depth.
    /// The pages bookmark `id` owns (as moving it with its pages takes
    /// them), in document order.
    pub fn bookmark_pages(&self, id: OutlineId) -> Option<Vec<PageId>> {
        let flat = self.outline_flat();
        let (_, node) = flat.iter().find(|(_, n)| n.id == id)?;
        Some(
            bookmark_span(self, node)?
                .into_iter()
                .map(|p| p.id)
                .collect(),
        )
    }

    pub fn outline_flat(&self) -> Vec<(u16, &OutlineNode)> {
        fn walk<'a>(
            nodes: &'a Vector<OutlineNode>,
            depth: u16,
            out: &mut Vec<(u16, &'a OutlineNode)>,
        ) {
            for n in nodes {
                out.push((depth, n));
                walk(&n.children, depth + 1, out);
            }
        }
        let mut out = Vec::new();
        walk(&self.outline, 0, &mut out);
        out
    }

    fn page_positions(&self) -> HashMap<PageId, usize> {
        self.pages
            .iter()
            .enumerate()
            .map(|(i, p)| (p.id, i))
            .collect()
    }
}

/// One change to the document. Serializable so plugins can submit edits
/// as data (see `todos/plugins.md`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Edit {
    /// Move `ids` (kept in document order) to just before `before`, or to the end.
    MovePages {
        ids: Vec<PageId>,
        before: Option<PageId>,
    },
    RotatePages {
        ids: Vec<PageId>,
        by: Rotation,
    },
    /// Removes the pages and cascades to bookmarks targeting them (their
    /// children are promoted).
    DeletePages {
        ids: Vec<PageId>,
    },
    /// Insert `range` of `source` before `before` (or at the end). If
    /// `outline` is given, merge it: top-level bookmarks whose title matches
    /// an existing top-level bookmark contribute their children to it;
    /// others are appended.
    InsertPages {
        source: SourceId,
        range: Range<u32>,
        before: Option<PageId>,
        outline: Option<Vec<FlatOutlineItem>>,
    },
    /// Copies inserted directly after each original.
    DuplicatePages {
        ids: Vec<PageId>,
    },
    /// Replace pages in place. The first `min(ids.len(), range.len())`
    /// keep their `PageId` (and so their bookmarks); extra originals are
    /// deleted, extra replacements inserted after.
    ReplacePages {
        ids: Vec<PageId>,
        source: SourceId,
        range: Range<u32>,
    },
    OutlineInsert {
        parent: Option<OutlineId>,
        after: Option<OutlineId>,
        title: String,
        target: Option<PageId>,
        /// Where on `target` it lands; None is the top of the page.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        view: Option<DestView>,
    },
    /// Insert several bookmarks at once, nested by depth (a new parent and
    /// its children, say), after `after` under `parent`, in order.
    OutlineInsertTree {
        parent: Option<OutlineId>,
        after: Option<OutlineId>,
        items: Vec<NewOutlineItem>,
    },
    /// Move a bookmark subtree. With `with_pages`, the pages it spans (from
    /// its first target up to the next bookmark's target) move with it.
    OutlineMove {
        id: OutlineId,
        parent: Option<OutlineId>,
        after: Option<OutlineId>,
        with_pages: bool,
    },
    /// `None` fields are left unchanged. For `target` and `color`,
    /// `Some(None)` clears the value; in JSON that is `null`, and an absent
    /// key means unchanged.
    OutlineEdit {
        id: OutlineId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "double_option"
        )]
        target: Option<Option<PageId>>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "double_option"
        )]
        color: Option<Option<[u8; 3]>>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "double_option"
        )]
        view: Option<Option<DestView>>,
    },
    /// Delete a bookmark and its subtree.
    OutlineDelete {
        id: OutlineId,
    },
    /// Add markup to a page. The annotation's id is assigned by the document.
    AnnotAdd {
        page: PageId,
        annot: Annot,
    },
    /// Replace an annotation (matched by id) with a new version.
    AnnotUpdate {
        page: PageId,
        annot: Annot,
    },
    /// Delete an annotation. Deleting one end of a connector pair deletes
    /// the other end too.
    AnnotDelete {
        page: PageId,
        id: AnnotId,
    },
    /// Add both ends of a connector pair in one step. Ids are assigned by the
    /// document and each end's `partner` is pointed at the other.
    ConnectorAdd {
        a: (PageId, Annot),
        b: (PageId, Annot),
    },
    /// Set (or replace) the OCR layer of each page. Unknown pages are an
    /// error.
    SetTextLayers {
        layers: Vec<(PageId, Arc<TextLayer>)>,
    },
    /// Drop unsaved OCR layers. Pages without one are ignored; a layer
    /// already written to the file can't be removed from the model.
    ClearTextLayers {
        ids: Vec<PageId>,
    },
    /// Show pages from other source pages (a cropped copy, say) while
    /// keeping their ids, rotation and bookmarks. Their marks and OCR text
    /// move by each `shift` (source-displayed points) to stay on the same
    /// content.
    RebasePages {
        pages: Vec<Rebase>,
    },
    /// Replace the page labels. Empty removes them.
    SetPageLabels {
        ranges: Vec<crate::labels::LabelSpec>,
    },
    /// Set or remove the header/footer.
    SetHeaderFooter {
        header_footer: Option<crate::stamps::HeaderFooter>,
    },
}

/// One page of [`Edit::RebasePages`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rebase {
    pub id: PageId,
    pub source: SourceId,
    pub index: u32,
    pub shift: (f32, f32),
}

/// Serde for `Option<Option<T>>` fields that distinguishes "absent"
/// (outer `None`, via `#[serde(default)]`) from `null` (`Some(None)`).
mod double_option {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<T: Serialize, S: Serializer>(
        v: &Option<Option<T>>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        match v {
            Some(inner) => inner.serialize(s),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Option<Option<T>>, D::Error> {
        Option::<T>::deserialize(d).map(Some)
    }
}

/// Identifies a long-running job whose results arrive in batches but
/// should undo as one step. See [`Document::apply_in_job`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct JobId(u64);

/// What an edit did, for user-facing notices.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EditOutcome {
    pub pages_removed: usize,
    pub pages_added: usize,
    pub bookmarks_removed: usize,
    pub bookmarks_added: usize,
}

struct Version {
    label: String,
    state: DocState,
    marginal_bytes: usize,
    /// The job that produced this version, if it may still amend it.
    job: Option<JobId>,
}

/// Everything a save needs, detached from the document so it can run on
/// another thread while editing continues.
#[derive(Clone)]
pub struct Snapshot {
    pub state: DocState,
    pub primary: SourceId,
    pub sources: HashMap<SourceId, Arc<SourceInfo>>,
    /// The outline differs from what's in the primary file.
    pub outline_dirty: bool,
    /// The page labels differ from what's in the primary file.
    pub labels_dirty: bool,
    /// The header/footer differs from what's drawn in the primary file:
    /// it's drawn over the pages on screen, and a save writes it.
    pub header_footer_dirty: bool,
    /// Pages whose markup differs from what's in the primary file.
    pub annots_dirty: HashSet<PageId>,
}

impl Snapshot {
    pub fn source(&self, id: SourceId) -> Option<&Arc<SourceInfo>> {
        self.sources.get(&id)
    }
}

pub struct Document {
    sources: HashMap<SourceId, Arc<SourceInfo>>,
    primary: SourceId,
    history: Vec<Version>,
    cursor: usize,
    saved: Option<usize>,
    next_page: u64,
    next_outline: u64,
    next_source: u32,
    policy: Box<dyn HistoryPolicy>,
    next_annot: u64,
    next_job: u64,
    /// Outline as it exists in the primary file on disk.
    saved_outline: Vector<OutlineNode>,
    saved_labels: Option<Vector<crate::labels::LabelSpec>>,
    /// The header/footer already drawn into the primary file's pages.
    baked_header_footer: Option<Arc<crate::stamps::HeaderFooter>>,
    /// Markup as it exists in the primary file on disk.
    saved_annots: im::HashMap<PageId, Vector<Annot>>,
}

impl Document {
    /// A document showing `primary` in its entirety.
    pub fn new(
        primary: SourceInfo,
        outline: &[FlatOutlineItem],
        policy: Box<dyn HistoryPolicy>,
    ) -> Self {
        let source = SourceId(0);
        let mut doc = Self {
            sources: HashMap::new(),
            primary: source,
            history: Vec::new(),
            cursor: 0,
            saved: Some(0),
            next_page: 0,
            next_outline: 0,
            next_source: 1,
            policy,
            next_annot: 1,
            next_job: 1,
            saved_outline: Vector::new(),
            saved_labels: None,
            baked_header_footer: None,
            saved_annots: im::HashMap::new(),
        };
        let pages: Vector<PageRef> = (0..primary.page_count)
            .map(|index| PageRef {
                id: doc.fresh_page_id(),
                source,
                index,
                rotation: Rotation::R0,
            })
            .collect();
        doc.sources.insert(source, Arc::new(primary));
        let by_index: Vec<PageId> = pages.iter().map(|p| p.id).collect();
        let outline = doc.outline_from_flat(outline, &by_index);
        doc.saved_outline = outline.clone();
        doc.history.push(Version {
            label: "Open".into(),
            state: DocState {
                pages,
                outline,
                annots: im::HashMap::new(),
                text_layers: im::HashMap::new(),
                labels: None,
                header_footer: None,
            },
            marginal_bytes: 0,
            job: None,
        });
        doc
    }

    pub fn primary(&self) -> SourceId {
        self.primary
    }

    pub fn source(&self, id: SourceId) -> Option<&Arc<SourceInfo>> {
        self.sources.get(&id)
    }

    pub fn sources(&self) -> impl Iterator<Item = (SourceId, &Arc<SourceInfo>)> {
        self.sources.iter().map(|(k, v)| (*k, v))
    }

    /// Register another file so its pages can be inserted.
    pub fn add_source(&mut self, info: SourceInfo) -> SourceId {
        let id = SourceId(self.next_source);
        self.next_source += 1;
        self.sources.insert(id, Arc::new(info));
        id
    }

    /// Page sizes arrive after open; they're not part of any snapshot.
    pub fn set_source_sizes(&mut self, id: SourceId, sizes: Vec<PageSize>) {
        if let Some(src) = self.sources.get_mut(&id) {
            let mut info = (**src).clone();
            info.page_sizes = Some(sizes);
            *src = Arc::new(info);
        }
    }

    /// The outline also arrives after open. Only valid before any edit.
    pub fn set_initial_outline(&mut self, outline: &[FlatOutlineItem]) -> Result<()> {
        if self.history.len() != 1 {
            bail!("outline can only be set before editing");
        }
        let by_index: Vec<PageId> = self.history[0].state.pages.iter().map(|p| p.id).collect();
        self.history[0].state.outline = self.outline_from_flat(outline, &by_index);
        self.saved_outline = self.history[0].state.outline.clone();
        Ok(())
    }

    /// Markup read back from the primary file (annotations sheaf wrote
    /// earlier), keyed by source page index. Only valid before any edit.
    /// The header/footer differs from what the primary file has drawn.
    pub fn header_footer_dirty(&self) -> bool {
        self.state().header_footer != self.baked_header_footer
    }

    /// The header/footer sheaf drew into the file, read after open (page
    /// scope by index).
    pub fn set_initial_header_footer(
        &mut self,
        mut hf: crate::stamps::HeaderFooter,
        indexes: Option<Vec<usize>>,
    ) -> Result<()> {
        if self.history.len() != 1 {
            bail!("the header/footer can only be set before editing");
        }
        let ids: Vec<PageId> = self.history[0].state.pages.iter().map(|p| p.id).collect();
        hf.pages = indexes.map(|ix| ix.iter().filter_map(|&i| ids.get(i).copied()).collect());
        let hf = Some(Arc::new(hf));
        self.history[0].state.header_footer = hf.clone();
        self.baked_header_footer = hf;
        Ok(())
    }

    /// The file's page labels, read after open: ranges by page index.
    pub fn set_initial_labels(&mut self, ranges: &[crate::labels::LabelRange]) -> Result<()> {
        if self.history.len() != 1 {
            bail!("page labels can only be set before editing");
        }
        let ids: Vec<PageId> = self.history[0].state.pages.iter().map(|p| p.id).collect();
        let specs: Vector<_> = crate::labels::to_specs(ranges, &ids).into_iter().collect();
        let specs = (!specs.is_empty()).then_some(specs);
        self.history[0].state.labels = specs.clone();
        self.saved_labels = specs;
        Ok(())
    }

    pub fn set_initial_annots(&mut self, by_index: HashMap<u32, Vec<Annot>>) -> Result<()> {
        if self.history.len() != 1 {
            bail!("annotations can only be loaded before editing");
        }
        let ids: Vec<PageId> = self.history[0].state.pages.iter().map(|p| p.id).collect();
        let mut map = im::HashMap::new();
        for (index, list) in by_index {
            if let Some(&pid) = ids.get(index as usize) {
                for a in &list {
                    self.next_annot = self.next_annot.max(a.id.0 + 1);
                }
                map.insert(pid, list.into_iter().collect());
            }
        }
        self.history[0].state.annots = map.clone();
        self.saved_annots = map;
        Ok(())
    }

    fn annots_dirty_pages(&self) -> HashSet<PageId> {
        let cur = &self.state().annots;
        let mut dirty = HashSet::new();
        for (pid, list) in cur.iter() {
            if self.saved_annots.get(pid) != Some(list) {
                dirty.insert(*pid);
            }
        }
        for pid in self.saved_annots.keys() {
            if !cur.contains_key(pid) {
                dirty.insert(*pid);
            }
        }
        dirty
    }

    pub fn fresh_annot_id(&mut self) -> AnnotId {
        let id = AnnotId(self.next_annot);
        self.next_annot += 1;
        id
    }

    pub fn state(&self) -> &DocState {
        &self.history[self.cursor].state
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            state: self.state().clone(),
            primary: self.primary,
            sources: self.sources.clone(),
            outline_dirty: self.state().outline != self.saved_outline,
            labels_dirty: self.state().labels != self.saved_labels,
            header_footer_dirty: self.header_footer_dirty(),
            annots_dirty: self.annots_dirty_pages(),
        }
    }

    /// After a successful save of the current version to a file described by
    /// `saved`: register it as a new source and make the current version an
    /// identity mapping over it, keeping `PageId`s (and bookmarks) intact so
    /// the view doesn't flicker. Older versions still reference the old
    /// sources, so undo across a save keeps working. Rotation deltas and OCR
    /// layers are now baked into the file, so they reset to nothing.
    pub fn rebase_to_saved(&mut self, saved: SourceInfo) -> SourceId {
        let id = self.add_source(saved);
        self.primary = id;
        let v = &mut self.history[self.cursor];
        v.state.text_layers = im::HashMap::new();
        v.state.pages = v
            .state
            .pages
            .iter()
            .enumerate()
            .map(|(i, p)| PageRef {
                id: p.id,
                source: id,
                index: i as u32,
                rotation: Rotation::R0,
            })
            .collect();
        self.saved_outline = self.history[self.cursor].state.outline.clone();
        self.saved_annots = self.history[self.cursor].state.annots.clone();
        self.saved_labels = self.history[self.cursor].state.labels.clone();
        self.baked_header_footer = self.history[self.cursor].state.header_footer.clone();
        self.saved = Some(self.cursor);
        id
    }

    pub fn page_count(&self) -> usize {
        self.state().pages.len()
    }

    pub fn page(&self, index: usize) -> Option<&PageRef> {
        self.state().pages.get(index)
    }

    pub fn index_of(&self, id: PageId) -> Option<usize> {
        self.state().index_of(id)
    }

    /// Displayed size of a page: its source size rotated by its delta.
    pub fn page_size(&self, page: &PageRef) -> PageSize {
        let src = self
            .sources
            .get(&page.source)
            .map(|s| s.page_size(page.index))
            .unwrap_or(PageSize {
                width: 612.0,
                height: 792.0,
            });
        src.rotated(page.rotation)
    }

    pub fn is_dirty(&self) -> bool {
        self.saved != Some(self.cursor)
    }

    pub fn mark_saved(&mut self) {
        self.saved = Some(self.cursor);
    }

    /// Treat the current state as unsaved even though nothing was edited
    /// (a recovered document: its primary source is a temporary file).
    pub fn mark_unsaved(&mut self) {
        self.saved = None;
    }

    pub fn can_undo(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_redo(&self) -> bool {
        self.cursor + 1 < self.history.len()
    }

    /// Label of the edit that `undo` would revert.
    pub fn undo_label(&self) -> Option<&str> {
        self.can_undo()
            .then(|| self.history[self.cursor].label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.can_redo()
            .then(|| self.history[self.cursor + 1].label.as_str())
    }

    pub fn undo(&mut self) -> Option<&str> {
        if !self.can_undo() {
            return None;
        }
        self.cursor -= 1;
        Some(self.history[self.cursor + 1].label.as_str())
    }

    pub fn redo(&mut self) -> Option<&str> {
        if !self.can_redo() {
            return None;
        }
        self.cursor += 1;
        Some(self.history[self.cursor].label.as_str())
    }

    /// Apply an edit as a new version. Versions after the cursor are discarded.
    pub fn apply(&mut self, edit: Edit, label: impl Into<String>) -> Result<EditOutcome> {
        let (state, outcome) = self.compute(self.state().clone(), edit)?;
        if state == *self.state() {
            return Ok(outcome);
        }
        self.push_version(state, label.into(), None);
        Ok(outcome)
    }

    /// Apply several edits in order as one version, so one undo reverts them
    /// all. Each edit sees the result of the ones before it. If any edit
    /// fails, none are applied.
    pub fn apply_batch(
        &mut self,
        edits: Vec<Edit>,
        label: impl Into<String>,
    ) -> Result<EditOutcome> {
        let mut state = self.state().clone();
        let mut total = EditOutcome::default();
        for (i, edit) in edits.into_iter().enumerate() {
            let (next, out) = self
                .compute(state, edit)
                .with_context(|| format!("edit {} of batch", i + 1))?;
            state = next;
            total.pages_removed += out.pages_removed;
            total.pages_added += out.pages_added;
            total.bookmarks_removed += out.bookmarks_removed;
            total.bookmarks_added += out.bookmarks_added;
        }
        if state != *self.state() {
            self.push_version(state, label.into(), None);
        }
        Ok(total)
    }

    /// Start a job whose batches of edits should undo as one step.
    pub fn begin_job(&mut self) -> JobId {
        let id = JobId(self.next_job);
        self.next_job += 1;
        id
    }

    /// Like [`apply`](Self::apply), but when the current version is the
    /// newest, was produced by `job`, and isn't the saved one, the edit
    /// amends it in place instead of pushing a new version. The first batch
    /// of a job pushes; later batches amend, so the whole job is one undo
    /// step. A save or an unrelated edit in between ends the chain, and the
    /// rest of the job becomes a second step.
    pub fn apply_in_job(
        &mut self,
        job: JobId,
        edit: Edit,
        label: impl Into<String>,
    ) -> Result<EditOutcome> {
        let (state, outcome) = self.compute(self.state().clone(), edit)?;
        let amend = self.history[self.cursor].job == Some(job)
            && self.cursor + 1 == self.history.len()
            && self.saved != Some(self.cursor);
        if amend {
            let marginal_bytes = match self.cursor.checked_sub(1) {
                Some(prev) => Self::marginal(&self.history[prev].state, &state),
                None => 0,
            };
            let v = &mut self.history[self.cursor];
            v.state = state;
            v.label = label.into();
            v.marginal_bytes = marginal_bytes;
            self.trim();
        } else {
            if state == *self.state() {
                return Ok(outcome);
            }
            self.push_version(state, label.into(), Some(job));
        }
        Ok(outcome)
    }

    /// Whether the current version is the one `job` is building, i.e. the
    /// user hasn't undone it or edited past it. Meaningful once the job has
    /// applied its first batch.
    pub fn job_is_current(&self, job: JobId) -> bool {
        self.history[self.cursor].job == Some(job)
    }

    /// Whether the user has undone `job`'s version (it sits in the redo
    /// branch). An unrelated edit after the job does not count.
    pub fn job_is_undone(&self, job: JobId) -> bool {
        self.history[self.cursor + 1..]
            .iter()
            .any(|v| v.job == Some(job))
    }

    fn push_version(&mut self, state: DocState, label: String, job: Option<JobId>) {
        self.history.truncate(self.cursor + 1);
        if let Some(s) = self.saved
            && s > self.cursor
        {
            self.saved = None;
        }
        let marginal_bytes = Self::marginal(&self.history[self.cursor].state, &state);
        self.history.push(Version {
            label,
            state,
            marginal_bytes,
            job,
        });
        self.cursor += 1;
        self.trim();
    }

    fn trim(&mut self) {
        let stats = self.stats();
        let n = self.policy.trim(&stats).min(self.cursor);
        if n > 0 {
            self.history.drain(0..n);
            self.cursor -= n;
            self.saved = self.saved.and_then(|s| s.checked_sub(n));
        }
    }

    pub fn stats(&self) -> HistoryStats {
        let current: HashSet<SourceId> = self.history[..=self.cursor]
            .iter()
            .flat_map(|v| v.state.pages.iter().map(|p| p.source))
            .collect();
        let undone_source_bytes = self.history[self.cursor + 1..]
            .iter()
            .flat_map(|v| v.state.pages.iter().map(|p| p.source))
            .collect::<HashSet<_>>()
            .difference(&current)
            .filter_map(|s| self.sources.get(s))
            .map(|s| s.bytes)
            .sum();
        HistoryStats {
            versions: self.history.len(),
            cursor: self.cursor,
            marginal_bytes: self.history.iter().map(|v| v.marginal_bytes).collect(),
            undone_source_bytes,
        }
    }

    /// Rough bytes `new` holds that `old` doesn't share.
    fn marginal(old: &DocState, new: &DocState) -> usize {
        let mut bytes = 0;
        if !old.pages.ptr_eq(&new.pages) {
            // im shares structure, but assume the worst for a modified vector.
            bytes += new.pages.len() * std::mem::size_of::<PageRef>();
        }
        if !old.outline.ptr_eq(&new.outline) {
            bytes += new.outline_flat().len() * 96;
        }
        if !old.annots.ptr_eq(&new.annots) {
            bytes += new.annots.values().map(|l| l.len() * 256).sum::<usize>();
        }
        if !old.text_layers.ptr_eq(&new.text_layers) {
            bytes += new
                .text_layers
                .iter()
                .filter(|(k, v)| old.text_layers.get(k).is_none_or(|o| !Arc::ptr_eq(o, v)))
                .map(|(_, v)| v.byte_estimate())
                .sum::<usize>();
        }
        bytes
    }

    fn fresh_page_id(&mut self) -> PageId {
        let id = PageId(self.next_page);
        self.next_page += 1;
        id
    }

    fn fresh_outline_id(&mut self) -> OutlineId {
        let id = OutlineId(self.next_outline);
        self.next_outline += 1;
        id
    }

    /// Nodes for [`Edit::OutlineInsertTree`]: pre-order items with depths
    /// that start at 0 and go down at most one level at a time.
    fn tree_from_items(&mut self, items: &[NewOutlineItem]) -> Result<Vec<OutlineNode>> {
        let mut roots: Vec<OutlineNode> = Vec::new();
        // Open path: (depth, node) from the current root down.
        let mut stack: Vec<(u16, OutlineNode)> = Vec::new();
        let close = |stack: &mut Vec<(u16, OutlineNode)>, roots: &mut Vec<OutlineNode>, d: u16| {
            while let Some((top, _)) = stack.last()
                && *top >= d
            {
                let (_, node) = stack.pop().unwrap();
                match stack.last_mut() {
                    Some((_, parent)) => parent.children.push_back(node),
                    None => roots.push(node),
                }
            }
        };
        let mut prev: Option<u16> = None;
        for item in items {
            match prev {
                None if item.depth != 0 => bail!("the first bookmark must be at depth 0"),
                Some(p) if item.depth > p + 1 => {
                    bail!("bookmark depth jumps from {p} to {}", item.depth)
                }
                _ => {}
            }
            prev = Some(item.depth);
            close(&mut stack, &mut roots, item.depth);
            let node = OutlineNode {
                id: self.fresh_outline_id(),
                title: item.title.clone(),
                target: item.target,
                view: item.target.and(item.view.clone()),
                color: None,
                children: Vector::new(),
            };
            stack.push((item.depth, node));
        }
        close(&mut stack, &mut roots, 0);
        Ok(roots)
    }

    fn outline_from_flat(
        &mut self,
        items: &[FlatOutlineItem],
        by_index: &[PageId],
    ) -> Vector<OutlineNode> {
        // Build with a stack of (depth, node) and attach children on pop.
        let mut roots: Vec<OutlineNode> = Vec::new();
        let mut stack: Vec<(u16, OutlineNode)> = Vec::new();
        let flush =
            |stack: &mut Vec<(u16, OutlineNode)>, roots: &mut Vec<OutlineNode>, depth: u16| {
                while let Some((d, _)) = stack.last()
                    && *d >= depth
                {
                    let (_, node) = stack.pop().unwrap();
                    match stack.last_mut() {
                        Some((_, parent)) => parent.children.push_back(node),
                        None => roots.push(node),
                    }
                }
            };
        for item in items {
            flush(&mut stack, &mut roots, item.depth);
            let node = OutlineNode {
                id: self.fresh_outline_id(),
                title: item.title.clone(),
                target: item.page.and_then(|p| by_index.get(p as usize).copied()),
                view: item.view.clone(),
                color: None,
                children: Vector::new(),
            };
            stack.push((item.depth, node));
        }
        flush(&mut stack, &mut roots, 0);
        roots.into_iter().collect()
    }

    // ----- edits ------------------------------------------------------------

    fn compute(&mut self, mut st: DocState, edit: Edit) -> Result<(DocState, EditOutcome)> {
        let mut out = EditOutcome::default();
        match edit {
            Edit::MovePages { ids, before } => {
                let set: HashSet<PageId> = ids.iter().copied().collect();
                if let Some(b) = before
                    && set.contains(&b)
                {
                    bail!("cannot move pages before one of themselves");
                }
                let moving: Vec<PageRef> = st
                    .pages
                    .iter()
                    .filter(|p| set.contains(&p.id))
                    .copied()
                    .collect();
                let mut rest: Vector<PageRef> = st
                    .pages
                    .iter()
                    .filter(|p| !set.contains(&p.id))
                    .copied()
                    .collect();
                let at = match before {
                    Some(b) => rest
                        .iter()
                        .position(|p| p.id == b)
                        .context("destination page not found")?,
                    None => rest.len(),
                };
                for (k, p) in moving.into_iter().enumerate() {
                    rest.insert(at + k, p);
                }
                st.pages = rest;
            }
            Edit::RotatePages { ids, by } => {
                let set: HashSet<PageId> = ids.into_iter().collect();
                st.pages = st
                    .pages
                    .iter()
                    .map(|p| {
                        if set.contains(&p.id) {
                            PageRef {
                                rotation: p.rotation.plus(by),
                                ..*p
                            }
                        } else {
                            *p
                        }
                    })
                    .collect();
            }
            Edit::DeletePages { ids } => {
                let set: HashSet<PageId> = ids.into_iter().collect();
                let before = st.pages.len();
                st.pages = st
                    .pages
                    .iter()
                    .filter(|p| !set.contains(&p.id))
                    .copied()
                    .collect();
                out.pages_removed = before - st.pages.len();
                for id in &set {
                    st.annots.remove(id);
                    st.text_layers.remove(id);
                }
                let (outline, removed) = cascade_delete(&st.outline, &set);
                st.outline = outline;
                out.bookmarks_removed = removed;
            }
            Edit::InsertPages {
                source,
                range,
                before,
                outline,
            } => {
                let info = self.sources.get(&source).context("unknown source")?.clone();
                if range.end > info.page_count {
                    bail!("range exceeds source page count");
                }
                let at = match before {
                    Some(b) => st.index_of(b).context("destination page not found")?,
                    None => st.pages.len(),
                };
                let new: Vec<PageRef> = range
                    .clone()
                    .map(|index| PageRef {
                        id: self.fresh_page_id(),
                        source,
                        index,
                        rotation: Rotation::R0,
                    })
                    .collect();
                out.pages_added = new.len();
                // Map source index -> new PageId for bookmark targets.
                let mut by_index = vec![PageId(u64::MAX); info.page_count as usize];
                for p in &new {
                    by_index[p.index as usize] = p.id;
                }
                for (k, p) in new.into_iter().enumerate() {
                    st.pages.insert(at + k, p);
                }
                if let Some(items) = outline {
                    let valid: Vec<PageId> = by_index.to_vec();
                    let incoming = self.outline_from_flat(&items, &valid);
                    // Targets outside the inserted range resolve to a sentinel; drop them.
                    let incoming = strip_targets(&incoming, PageId(u64::MAX));
                    let (merged, added) = merge_outline(&st.outline, &incoming);
                    st.outline = merged;
                    out.bookmarks_added = added;
                }
            }
            Edit::DuplicatePages { ids } => {
                let set: HashSet<PageId> = ids.into_iter().collect();
                let mut pages = Vector::new();
                for p in st.pages.iter() {
                    pages.push_back(*p);
                    if set.contains(&p.id) {
                        let copy = self.fresh_page_id();
                        if let Some(list) = st.annots.get(&p.id).cloned() {
                            let list: Vector<Annot> = list
                                .into_iter()
                                .map(|mut a| {
                                    a.id = self.fresh_annot_id();
                                    a
                                })
                                .collect();
                            st.annots.insert(copy, list);
                        }
                        if let Some(layer) = st.text_layers.get(&p.id).cloned() {
                            st.text_layers.insert(copy, layer);
                        }
                        pages.push_back(PageRef { id: copy, ..*p });
                        out.pages_added += 1;
                    }
                }
                st.pages = pages;
            }
            Edit::ReplacePages { ids, source, range } => {
                let info = self.sources.get(&source).context("unknown source")?.clone();
                if range.end > info.page_count {
                    bail!("range exceeds source page count");
                }
                let mut replacements = range.clone();
                let mut last_pos = None;
                let mut extra_old = HashSet::new();
                let pos = st.page_positions();
                let mut pages = st.pages.clone();
                for id in &ids {
                    let i = *pos.get(id).context("page to replace not found")?;
                    // The page shows different content now; its OCR is stale.
                    st.text_layers.remove(id);
                    match replacements.next() {
                        Some(index) => {
                            pages.set(
                                i,
                                PageRef {
                                    id: *id,
                                    source,
                                    index,
                                    rotation: Rotation::R0,
                                },
                            );
                            last_pos = Some(i);
                        }
                        None => {
                            extra_old.insert(*id);
                        }
                    }
                }
                // Extra replacements go after the last replaced page.
                let at = last_pos.map(|i| i + 1).unwrap_or(pages.len());
                for (k, index) in replacements.enumerate() {
                    pages.insert(
                        at + k,
                        PageRef {
                            id: self.fresh_page_id(),
                            source,
                            index,
                            rotation: Rotation::R0,
                        },
                    );
                    out.pages_added += 1;
                }
                if !extra_old.is_empty() {
                    out.pages_removed = extra_old.len();
                    pages = pages
                        .iter()
                        .filter(|p| !extra_old.contains(&p.id))
                        .copied()
                        .collect();
                    let (outline, removed) = cascade_delete(&st.outline, &extra_old);
                    st.outline = outline;
                    out.bookmarks_removed = removed;
                }
                st.pages = pages;
            }
            Edit::OutlineInsert {
                parent,
                after,
                title,
                target,
                view,
            } => {
                let node = OutlineNode {
                    id: self.fresh_outline_id(),
                    title,
                    view: target.and(view),
                    target,
                    color: None,
                    children: Vector::new(),
                };
                st.outline = outline_insert(&st.outline, parent, after, node)?;
                out.bookmarks_added = 1;
            }
            Edit::OutlineInsertTree {
                parent,
                after,
                items,
            } => {
                let roots = self.tree_from_items(&items)?;
                out.bookmarks_added = items.len();
                let mut after = after;
                for node in roots {
                    let id = node.id;
                    st.outline = outline_insert(&st.outline, parent, after, node)?;
                    after = Some(id);
                }
            }
            Edit::OutlineMove {
                id,
                parent,
                after,
                with_pages,
            } => {
                if parent == Some(id) || outline_find(&st.outline, id).is_none() {
                    bail!("bookmark not found or moved into itself");
                }
                if let Some(p) = parent
                    && subtree_contains(&st.outline, id, p)
                {
                    bail!("cannot move a bookmark under its own descendant");
                }
                let (without, node) = outline_remove(&st.outline, id);
                let node = node.context("bookmark not found")?;
                let span = with_pages.then(|| bookmark_span(&st, &node)).flatten();
                st.outline = outline_insert(&without, parent, after, node.clone())?;
                if let Some(span) = span {
                    // Destination: right after the span of whatever precedes the
                    // moved node in the new pre-order (or the start).
                    let flat = st.outline_flat();
                    let my_pos = flat.iter().position(|(_, n)| n.id == id).unwrap();
                    let moving: Vec<PageId> = span.iter().map(|p| p.id).collect();
                    let moving_set: HashSet<PageId> = moving.iter().copied().collect();
                    let rest: Vector<PageRef> = st
                        .pages
                        .iter()
                        .filter(|p| !moving_set.contains(&p.id))
                        .copied()
                        .collect();
                    let dest = match flat[..my_pos].iter().rev().find_map(|(_, n)| n.target) {
                        Some(prev_target) => {
                            // End of the previous bookmark's span, in `rest`.
                            let rest_state = DocState {
                                pages: rest.clone(),
                                outline: st.outline.clone(),
                                annots: st.annots.clone(),
                                text_layers: st.text_layers.clone(),
                                labels: st.labels.clone(),
                                header_footer: st.header_footer.clone(),
                            };
                            let prev_node = flat[..my_pos]
                                .iter()
                                .rev()
                                .find(|(_, n)| n.target.is_some())
                                .unwrap()
                                .1;
                            match bookmark_span(&rest_state, prev_node) {
                                Some(s) => rest_state
                                    .index_of(s.last().unwrap().id)
                                    .map(|i| i + 1)
                                    .unwrap_or(0),
                                None => {
                                    rest_state.index_of(prev_target).map(|i| i + 1).unwrap_or(0)
                                }
                            }
                        }
                        None => 0,
                    };
                    let mut pages = rest;
                    for (k, p) in span.into_iter().enumerate() {
                        pages.insert(dest + k, p);
                    }
                    st.pages = pages;
                }
            }
            Edit::OutlineEdit {
                id,
                title,
                target,
                color,
                view,
            } => {
                let updated = outline_update(&st.outline, id, |n| {
                    if let Some(t) = &title {
                        n.title = t.clone();
                    }
                    if let Some(t) = target {
                        n.target = t;
                        // A new page: the old position meant the old page.
                        if view.is_none() {
                            n.view = None;
                        }
                    }
                    if let Some(v) = &view {
                        n.view = v.clone();
                    }
                    if n.target.is_none() {
                        n.view = None;
                    }
                    if let Some(c) = color {
                        n.color = c;
                    }
                });
                st.outline = updated.context("bookmark not found")?;
            }
            Edit::OutlineDelete { id } => {
                let (without, node) = outline_remove(&st.outline, id);
                let node = node.context("bookmark not found")?;
                out.bookmarks_removed = 1 + count_nodes(&node.children);
                st.outline = without;
            }
            Edit::AnnotAdd { page, mut annot } => {
                if st.index_of(page).is_none() {
                    bail!("page not found");
                }
                annot.id = self.fresh_annot_id();
                let mut list = st.annots.get(&page).cloned().unwrap_or_default();
                list.push_back(annot);
                st.annots.insert(page, list);
            }
            Edit::AnnotUpdate { page, annot } => {
                let mut list = st
                    .annots
                    .get(&page)
                    .cloned()
                    .context("page has no annotations")?;
                let i = list
                    .iter()
                    .position(|a| a.id == annot.id)
                    .context("annotation not found")?;
                list.set(i, annot);
                st.annots.insert(page, list);
            }
            Edit::AnnotDelete { page, id } => {
                let mut list = st
                    .annots
                    .get(&page)
                    .cloned()
                    .context("page has no annotations")?;
                let i = list
                    .iter()
                    .position(|a| a.id == id)
                    .context("annotation not found")?;
                let removed = list.remove(i);
                if list.is_empty() {
                    st.annots.remove(&page);
                } else {
                    st.annots.insert(page, list);
                }
                // The other end of a connector is meaningless alone.
                if let Some(pid) = removed.partner()
                    && let Some((ppage, _)) = st.find_annot(pid)
                {
                    let mut plist = st.annots.get(&ppage).cloned().unwrap_or_default();
                    plist.retain(|a| a.id != pid);
                    if plist.is_empty() {
                        st.annots.remove(&ppage);
                    } else {
                        st.annots.insert(ppage, plist);
                    }
                }
            }
            Edit::ConnectorAdd {
                a: (page_a, mut a),
                b: (page_b, mut b),
            } => {
                if st.index_of(page_a).is_none() || st.index_of(page_b).is_none() {
                    bail!("page not found");
                }
                a.id = self.fresh_annot_id();
                b.id = self.fresh_annot_id();
                if let AnnotKind::Connector { partner, .. } = &mut a.kind {
                    *partner = Some(b.id);
                }
                if let AnnotKind::Connector { partner, .. } = &mut b.kind {
                    *partner = Some(a.id);
                }
                for (page, annot) in [(page_a, a), (page_b, b)] {
                    let mut list = st.annots.get(&page).cloned().unwrap_or_default();
                    list.push_back(annot);
                    st.annots.insert(page, list);
                }
            }
            Edit::SetTextLayers { layers } => {
                let pos = st.page_positions();
                for (id, layer) in layers {
                    if !pos.contains_key(&id) {
                        bail!("page not found");
                    }
                    st.text_layers.insert(id, layer);
                }
            }
            Edit::ClearTextLayers { ids } => {
                for id in ids {
                    st.text_layers.remove(&id);
                }
            }
            Edit::SetHeaderFooter { header_footer } => {
                st.header_footer = header_footer.filter(|h| !h.is_empty()).map(Arc::new);
            }
            Edit::SetPageLabels { ranges } => {
                st.labels = (!ranges.is_empty()).then(|| ranges.into_iter().collect());
            }
            Edit::RebasePages { pages } => {
                let pos = st.page_positions();
                for r in pages {
                    let i = *pos.get(&r.id).context("page to rebase not found")?;
                    let info = self.sources.get(&r.source).context("unknown source")?;
                    if r.index >= info.page_count {
                        bail!("page {} isn't in the source", r.index);
                    }
                    let size = info.page_size(r.index);
                    let old = st.pages[i];
                    st.pages.set(
                        i,
                        PageRef {
                            id: r.id,
                            source: r.source,
                            index: r.index,
                            rotation: old.rotation,
                        },
                    );
                    let (dx, dy) = r.shift;
                    if let Some(list) = st.annots.get(&r.id).cloned() {
                        let moved = list
                            .into_iter()
                            .map(|mut a| {
                                a.kind = a.kind.translated(dx, dy);
                                a
                            })
                            .collect();
                        st.annots.insert(r.id, moved);
                    }
                    if let Some(layer) = st.text_layers.get(&r.id).cloned() {
                        st.text_layers
                            .insert(r.id, Arc::new(layer.shifted(dx, dy, size)));
                    }
                }
            }
        }
        Ok((st, out))
    }
}

// ----- outline helpers (path-copying over im::Vector) ---------------------

fn count_nodes(nodes: &Vector<OutlineNode>) -> usize {
    nodes.iter().map(|n| 1 + count_nodes(&n.children)).sum()
}

/// Path of child indices to `id`.
fn outline_find(nodes: &Vector<OutlineNode>, id: OutlineId) -> Option<Vec<usize>> {
    for (i, n) in nodes.iter().enumerate() {
        if n.id == id {
            return Some(vec![i]);
        }
        if let Some(mut path) = outline_find(&n.children, id) {
            path.insert(0, i);
            return Some(path);
        }
    }
    None
}

fn subtree_contains(nodes: &Vector<OutlineNode>, root: OutlineId, needle: OutlineId) -> bool {
    fn get<'a>(nodes: &'a Vector<OutlineNode>, path: &[usize]) -> &'a OutlineNode {
        let n = &nodes[path[0]];
        if path.len() == 1 {
            n
        } else {
            get(&n.children, &path[1..])
        }
    }
    match outline_find(nodes, root) {
        Some(path) => outline_find(&get(nodes, &path).children, needle).is_some(),
        None => false,
    }
}

fn outline_update(
    nodes: &Vector<OutlineNode>,
    id: OutlineId,
    f: impl FnOnce(&mut OutlineNode),
) -> Option<Vector<OutlineNode>> {
    fn go(
        nodes: &Vector<OutlineNode>,
        path: &[usize],
        f: Box<dyn FnOnce(&mut OutlineNode) + '_>,
    ) -> Vector<OutlineNode> {
        let mut nodes = nodes.clone();
        let mut node = nodes[path[0]].clone();
        if path.len() == 1 {
            f(&mut node);
        } else {
            node.children = go(&node.children, &path[1..], f);
        }
        nodes.set(path[0], node);
        nodes
    }
    let path = outline_find(nodes, id)?;
    Some(go(nodes, &path, Box::new(f)))
}

fn outline_remove(
    nodes: &Vector<OutlineNode>,
    id: OutlineId,
) -> (Vector<OutlineNode>, Option<OutlineNode>) {
    fn go(nodes: &Vector<OutlineNode>, path: &[usize]) -> (Vector<OutlineNode>, OutlineNode) {
        let mut nodes = nodes.clone();
        if path.len() == 1 {
            let removed = nodes.remove(path[0]);
            (nodes, removed)
        } else {
            let mut node = nodes[path[0]].clone();
            let (children, removed) = go(&node.children, &path[1..]);
            node.children = children;
            nodes.set(path[0], node);
            (nodes, removed)
        }
    }
    match outline_find(nodes, id) {
        Some(path) => {
            let (n, r) = go(nodes, &path);
            (n, Some(r))
        }
        None => (nodes.clone(), None),
    }
}

fn outline_insert(
    nodes: &Vector<OutlineNode>,
    parent: Option<OutlineId>,
    after: Option<OutlineId>,
    node: OutlineNode,
) -> Result<Vector<OutlineNode>> {
    fn insert_in(
        siblings: &Vector<OutlineNode>,
        after: Option<OutlineId>,
        node: OutlineNode,
    ) -> Result<Vector<OutlineNode>> {
        let mut s = siblings.clone();
        let at = match after {
            Some(a) => {
                s.iter()
                    .position(|n| n.id == a)
                    .context("'after' bookmark not among siblings")?
                    + 1
            }
            None => 0,
        };
        s.insert(at, node);
        Ok(s)
    }
    match parent {
        None => insert_in(nodes, after, node),
        Some(p) => {
            let mut err = None;
            let updated = outline_update(nodes, p, |n| match insert_in(&n.children, after, node) {
                Ok(c) => n.children = c,
                Err(e) => err = Some(e),
            })
            .context("parent bookmark not found")?;
            match err {
                Some(e) => Err(e),
                None => Ok(updated),
            }
        }
    }
}

/// Remove nodes targeting deleted pages, promoting their children.
fn cascade_delete(
    nodes: &Vector<OutlineNode>,
    gone: &HashSet<PageId>,
) -> (Vector<OutlineNode>, usize) {
    let mut out = Vector::new();
    let mut removed = 0;
    for n in nodes {
        let (children, r) = cascade_delete(&n.children, gone);
        removed += r;
        if n.target.is_some_and(|t| gone.contains(&t)) {
            removed += 1;
            out.append(children);
        } else {
            let mut n = n.clone();
            n.children = children;
            out.push_back(n);
        }
    }
    (out, removed)
}

fn strip_targets(nodes: &Vector<OutlineNode>, sentinel: PageId) -> Vector<OutlineNode> {
    nodes
        .iter()
        .map(|n| {
            let mut n = n.clone();
            if n.target == Some(sentinel) {
                n.target = None;
                n.view = None;
            }
            n.children = strip_targets(&n.children, sentinel);
            n
        })
        .collect()
}

/// PDFlyer-style merge: incoming top-level nodes with a matching title fold
/// their children into the existing node; others are appended.
fn merge_outline(
    existing: &Vector<OutlineNode>,
    incoming: &Vector<OutlineNode>,
) -> (Vector<OutlineNode>, usize) {
    let mut out = existing.clone();
    let mut added = 0;
    for inc in incoming {
        match out.iter().position(|e| e.title == inc.title) {
            Some(i) => {
                let mut e = out[i].clone();
                added += count_nodes(&inc.children);
                e.children.append(inc.children.clone());
                out.set(i, e);
            }
            None => {
                added += 1 + count_nodes(&inc.children);
                out.push_back(inc.clone());
            }
        }
    }
    (out, added)
}

/// The pages a bookmark "owns": from its first target (in document order)
/// up to, but not including, the first page targeted by a bookmark outside
/// its subtree that comes after it in reading order.
fn bookmark_span(st: &DocState, node: &OutlineNode) -> Option<Vec<PageRef>> {
    let pos = st.page_positions();
    let mut subtree = Vec::new();
    collect_targets(node, &mut subtree);
    let start = subtree.iter().filter_map(|t| pos.get(t)).min().copied()?;
    let subtree_ids: HashSet<OutlineId> = {
        let mut ids = HashSet::new();
        collect_ids(node, &mut ids);
        ids
    };
    let end = st
        .outline_flat()
        .iter()
        .filter(|(_, n)| !subtree_ids.contains(&n.id))
        .filter_map(|(_, n)| n.target.and_then(|t| pos.get(&t)).copied())
        .filter(|&i| i > start)
        .min()
        .unwrap_or(st.pages.len());
    Some(
        st.pages
            .iter()
            .skip(start)
            .take(end - start)
            .copied()
            .collect(),
    )
}

fn collect_targets(node: &OutlineNode, out: &mut Vec<PageId>) {
    if let Some(t) = node.target {
        out.push(t);
    }
    for c in &node.children {
        collect_targets(c, out);
    }
}

fn collect_ids(node: &OutlineNode, out: &mut HashSet<OutlineId>) {
    out.insert(node.id);
    for c in &node.children {
        collect_ids(c, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::FixedCount;

    fn src(n: u32) -> SourceInfo {
        SourceInfo {
            path: "a.pdf".into(),
            page_count: n,
            first_page: PageSize {
                width: 612.0,
                height: 792.0,
            },
            page_sizes: None,
            bytes: 1000,
            password: None,
        }
    }

    fn flat(items: &[(&str, Option<u32>, u16)]) -> Vec<FlatOutlineItem> {
        items
            .iter()
            .map(|(t, p, d)| FlatOutlineItem {
                title: t.to_string(),
                page: *p,
                depth: *d,
                view: None,
            })
            .collect()
    }

    fn doc() -> Document {
        // Pages 0..6; bookmarks: A->0 { A1->1, A2->2 }, B->3, C->5
        Document::new(
            src(6),
            &flat(&[
                ("A", Some(0), 0),
                ("A1", Some(1), 1),
                ("A2", Some(2), 1),
                ("B", Some(3), 0),
                ("C", Some(5), 0),
            ]),
            Box::new(FixedCount(100)),
        )
    }

    fn order(d: &Document) -> Vec<u32> {
        d.state().pages.iter().map(|p| p.index).collect()
    }

    fn titles(d: &Document) -> Vec<(u16, String)> {
        d.state()
            .outline_flat()
            .iter()
            .map(|(depth, n)| (*depth, n.title.clone()))
            .collect()
    }

    fn id_at(d: &Document, i: usize) -> PageId {
        d.page(i).unwrap().id
    }

    fn bm(d: &Document, title: &str) -> OutlineId {
        d.state()
            .outline_flat()
            .iter()
            .find(|(_, n)| n.title == title)
            .unwrap()
            .1
            .id
    }

    #[test]
    fn opens_with_identity_pages_and_tree() {
        let d = doc();
        assert_eq!(order(&d), vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(
            titles(&d),
            vec![
                (0, "A".into()),
                (1, "A1".into()),
                (1, "A2".into()),
                (0, "B".into()),
                (0, "C".into())
            ]
        );
        assert!(!d.is_dirty());
    }

    #[test]
    fn move_rotate_undo_redo() {
        let mut d = doc();
        let p5 = id_at(&d, 5);
        d.apply(
            Edit::MovePages {
                ids: vec![p5],
                before: Some(id_at(&d, 0)),
            },
            "move",
        )
        .unwrap();
        assert_eq!(order(&d), vec![5, 0, 1, 2, 3, 4]);
        d.apply(
            Edit::RotatePages {
                ids: vec![p5],
                by: Rotation::R90,
            },
            "rotate",
        )
        .unwrap();
        assert_eq!(d.page(0).unwrap().rotation, Rotation::R90);
        assert_eq!(d.page_size(d.page(0).unwrap()).width, 792.0);
        assert!(d.is_dirty());
        assert_eq!(d.undo(), Some("rotate"));
        assert_eq!(d.page(0).unwrap().rotation, Rotation::R0);
        assert_eq!(d.undo(), Some("move"));
        assert_eq!(order(&d), vec![0, 1, 2, 3, 4, 5]);
        assert!(!d.is_dirty());
        assert_eq!(d.redo(), Some("move"));
        assert_eq!(order(&d), vec![5, 0, 1, 2, 3, 4]);
        // A new edit discards the redo branch.
        d.apply(Edit::DeletePages { ids: vec![p5] }, "delete")
            .unwrap();
        assert!(!d.can_redo());
    }

    #[test]
    fn delete_cascades_and_promotes_children() {
        let mut d = doc();
        let out = d
            .apply(
                Edit::DeletePages {
                    ids: vec![id_at(&d, 0)],
                },
                "delete",
            )
            .unwrap();
        assert_eq!(out.pages_removed, 1);
        assert_eq!(out.bookmarks_removed, 1);
        // A is gone; A1 and A2 promoted to top level.
        assert_eq!(
            titles(&d),
            vec![
                (0, "A1".into()),
                (0, "A2".into()),
                (0, "B".into()),
                (0, "C".into())
            ]
        );
        d.undo();
        assert_eq!(titles(&d).len(), 5);
    }

    #[test]
    fn insert_with_outline_merge() {
        let mut d = doc();
        let s = d.add_source(SourceInfo {
            path: "b.pdf".into(),
            ..src(3)
        });
        let out = d
            .apply(
                Edit::InsertPages {
                    source: s,
                    range: 0..3,
                    before: Some(id_at(&d, 3)),
                    outline: Some(flat(&[
                        ("A", Some(0), 0),
                        ("A3", Some(1), 1),
                        ("D", Some(2), 0),
                    ])),
                },
                "insert",
            )
            .unwrap();
        assert_eq!(out.pages_added, 3);
        assert_eq!(out.bookmarks_added, 2);
        assert_eq!(order(&d), vec![0, 1, 2, 0, 1, 2, 3, 4, 5]);
        assert_eq!(d.page(3).unwrap().source, s);
        // "A" merged: A3 joined A's children; "D" appended.
        assert_eq!(
            titles(&d),
            vec![
                (0, "A".into()),
                (1, "A1".into()),
                (1, "A2".into()),
                (1, "A3".into()),
                (0, "B".into()),
                (0, "C".into()),
                (0, "D".into())
            ]
        );
        // A3 targets the inserted page with source index 1.
        let a3 = d
            .state()
            .outline_flat()
            .iter()
            .find(|(_, n)| n.title == "A3")
            .unwrap()
            .1
            .target
            .unwrap();
        assert_eq!(d.index_of(a3), Some(4));
    }

    #[test]
    fn replace_keeps_ids_and_bookmarks() {
        let mut d = doc();
        let s = d.add_source(SourceInfo {
            path: "b.pdf".into(),
            ..src(2)
        });
        let p3 = id_at(&d, 3);
        d.apply(
            Edit::ReplacePages {
                ids: vec![p3],
                source: s,
                range: 0..2,
            },
            "replace",
        )
        .unwrap();
        assert_eq!(d.page(3).unwrap().id, p3);
        assert_eq!(d.page(3).unwrap().source, s);
        assert_eq!(d.page(4).unwrap().source, s);
        assert_eq!(order(&d), vec![0, 1, 2, 0, 1, 4, 5]);
        let b = d
            .state()
            .outline_flat()
            .iter()
            .find(|(_, n)| n.title == "B")
            .unwrap()
            .1;
        assert_eq!(b.target, Some(p3));
    }

    #[test]
    fn duplicate() {
        let mut d = doc();
        d.apply(
            Edit::DuplicatePages {
                ids: vec![id_at(&d, 1)],
            },
            "dup",
        )
        .unwrap();
        assert_eq!(order(&d), vec![0, 1, 1, 2, 3, 4, 5]);
        assert_ne!(id_at(&d, 1), id_at(&d, 2));
    }

    #[test]
    fn outline_edit_insert_delete() {
        let mut d = doc();
        let a = bm(&d, "A");
        d.apply(
            Edit::OutlineEdit {
                id: a,
                title: Some("Alpha".into()),
                target: None,
                color: Some(Some([255, 0, 0])),
                view: None,
            },
            "e",
        )
        .unwrap();
        assert_eq!(titles(&d)[0].1, "Alpha");
        d.apply(
            Edit::OutlineInsert {
                parent: Some(a),
                after: Some(bm(&d, "A1")),
                title: "A1b".into(),
                target: None,
                view: None,
            },
            "ins",
        )
        .unwrap();
        assert_eq!(titles(&d)[2], (1, "A1b".into()));
        let out = d.apply(Edit::OutlineDelete { id: a }, "del").unwrap();
        assert_eq!(out.bookmarks_removed, 4);
        assert_eq!(titles(&d), vec![(0, "B".into()), (0, "C".into())]);
    }

    #[test]
    fn outline_move_with_pages() {
        let mut d = doc();
        // Move C (span: page 5) to before B (as first top-level after A? use after=A).
        let c = bm(&d, "C");
        d.apply(
            Edit::OutlineMove {
                id: c,
                parent: None,
                after: Some(bm(&d, "A")),
                with_pages: true,
            },
            "mv",
        )
        .unwrap();
        assert_eq!(
            titles(&d).iter().map(|t| t.1.as_str()).collect::<Vec<_>>(),
            vec!["A", "A1", "A2", "C", "B"]
        );
        // A's span is pages 0..3 (A, A1, A2 target 0,1,2; next outside is B->3), so C's page lands at index 3.
        assert_eq!(order(&d), vec![0, 1, 2, 5, 3, 4]);
        // Move B (span: pages 3,4) to the very start.
        let b = bm(&d, "B");
        d.apply(
            Edit::OutlineMove {
                id: b,
                parent: None,
                after: None,
                with_pages: true,
            },
            "mv",
        )
        .unwrap();
        assert_eq!(order(&d), vec![3, 4, 0, 1, 2, 5]);
        assert_eq!(titles(&d)[0].1, "B");
        d.undo();
        assert_eq!(order(&d), vec![0, 1, 2, 5, 3, 4]);
    }

    #[test]
    fn outline_move_rejects_cycles() {
        let mut d = doc();
        let a = bm(&d, "A");
        let a1 = bm(&d, "A1");
        assert!(
            d.apply(
                Edit::OutlineMove {
                    id: a,
                    parent: Some(a1),
                    after: None,
                    with_pages: false
                },
                "x"
            )
            .is_err()
        );
    }

    #[test]
    fn history_trims_oldest() {
        let mut d = Document::new(src(3), &[], Box::new(FixedCount(3)));
        for i in 0..5 {
            d.apply(
                Edit::RotatePages {
                    ids: vec![id_at(&d, 0)],
                    by: Rotation::R90,
                },
                format!("r{i}"),
            )
            .unwrap();
        }
        assert_eq!(d.stats().versions, 3);
        assert_eq!(d.undo_label(), Some("r4"));
        d.undo();
        d.undo();
        assert!(!d.can_undo());
        // The saved version was trimmed away, so we're dirty everywhere.
        assert!(d.is_dirty());
    }

    #[test]
    fn rebase_after_save_keeps_ids_and_history() {
        let mut d = doc();
        let p5 = id_at(&d, 5);
        d.apply(
            Edit::MovePages {
                ids: vec![p5],
                before: Some(id_at(&d, 0)),
            },
            "move",
        )
        .unwrap();
        d.apply(
            Edit::RotatePages {
                ids: vec![p5],
                by: Rotation::R90,
            },
            "rotate",
        )
        .unwrap();
        let old_primary = d.primary();
        let new = d.rebase_to_saved(SourceInfo {
            path: "saved.pdf".into(),
            ..src(6)
        });
        assert_ne!(new, old_primary);
        assert!(!d.is_dirty());
        assert_eq!(d.page(0).unwrap().id, p5);
        assert_eq!(d.page(0).unwrap().source, new);
        assert_eq!(order(&d), vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(d.page(0).unwrap().rotation, Rotation::R0);
        // Undo still reaches the pre-save version over the old source.
        d.undo();
        assert_eq!(d.page(0).unwrap().source, old_primary);
        assert_eq!(d.page(0).unwrap().index, 5);
        assert!(d.is_dirty());
    }

    #[test]
    fn connector_pair_links_and_deletes_together() {
        use crate::annot::{AnnotKind, Box2};
        let mut d = doc();
        let (p0, p2) = (id_at(&d, 0), id_at(&d, 2));
        let mk = |label: &str| Annot {
            id: AnnotId(0),
            kind: AnnotKind::Connector {
                rect: Box2 {
                    x: 10.0,
                    y: 10.0,
                    w: 22.0,
                    h: 22.0,
                },
                label: label.into(),
                partner: None,
            },
            color: [255, 0, 0],
            opacity: 1.0,
            stroke: 1.0,
            author: "t".into(),
            contents: String::new(),
            modified: String::new(),
        };
        assert_eq!(d.state().next_connector_label(), "1");
        d.apply(
            Edit::ConnectorAdd {
                a: (p0, mk("1")),
                b: (p2, mk("1")),
            },
            "c",
        )
        .unwrap();
        let a = d.state().annots_on(p0).unwrap()[0].clone();
        let b = d.state().annots_on(p2).unwrap()[0].clone();
        assert_eq!(a.partner(), Some(b.id));
        assert_eq!(b.partner(), Some(a.id));
        assert_eq!(d.state().find_annot(b.id).map(|(p, _)| p), Some(p2));
        assert_eq!(d.state().next_connector_label(), "2");
        // Deleting one end removes the other.
        d.apply(Edit::AnnotDelete { page: p2, id: b.id }, "rm")
            .unwrap();
        assert!(d.state().annots_on(p0).is_none());
        assert!(d.state().annots_on(p2).is_none());
        d.undo();
        assert_eq!(d.state().annots_on(p0).unwrap().len(), 1);
        assert_eq!(d.state().annots_on(p2).unwrap().len(), 1);
    }

    #[test]
    fn annot_add_update_delete_and_cascade() {
        use crate::annot::{AnnotKind, Box2};
        let mut d = doc();
        let p1 = id_at(&d, 1);
        let a = Annot {
            id: AnnotId(0),
            kind: AnnotKind::Square {
                rect: Box2 {
                    x: 10.0,
                    y: 10.0,
                    w: 50.0,
                    h: 20.0,
                },
            },
            color: [255, 0, 0],
            opacity: 1.0,
            stroke: 2.0,
            author: "t".into(),
            contents: String::new(),
            modified: String::new(),
        };
        d.apply(
            Edit::AnnotAdd {
                page: p1,
                annot: a.clone(),
            },
            "add",
        )
        .unwrap();
        let stored = d.state().annots_on(p1).unwrap()[0].clone();
        assert_ne!(stored.id, AnnotId(0), "id assigned by the document");
        let mut upd = stored.clone();
        upd.color = [0, 0, 255];
        d.apply(
            Edit::AnnotUpdate {
                page: p1,
                annot: upd,
            },
            "upd",
        )
        .unwrap();
        assert_eq!(d.state().annots_on(p1).unwrap()[0].color, [0, 0, 255]);
        assert_eq!(d.snapshot().annots_dirty.len(), 1);
        d.apply(Edit::DuplicatePages { ids: vec![p1] }, "dup")
            .unwrap();
        let p1_copy = id_at(&d, 2);
        assert_ne!(d.state().annots_on(p1_copy).unwrap()[0].id, stored.id);
        d.apply(Edit::DeletePages { ids: vec![p1_copy] }, "del")
            .unwrap();
        assert!(d.state().annots_on(p1_copy).is_none());
        d.apply(
            Edit::AnnotDelete {
                page: p1,
                id: stored.id,
            },
            "rm",
        )
        .unwrap();
        assert!(d.state().annots_on(p1).is_none());
        d.undo();
        assert_eq!(d.state().annots_on(p1).unwrap().len(), 1);
    }

    #[test]
    fn noop_edit_adds_no_version() {
        let mut d = doc();
        d.apply(
            Edit::RotatePages {
                ids: vec![],
                by: Rotation::R90,
            },
            "noop",
        )
        .unwrap();
        assert!(!d.can_undo());
    }

    // ----- OCR text layers ---------------------------------------------------

    fn tl(text: &str) -> Arc<TextLayer> {
        Arc::new(TextLayer::from_engine(
            vec![crate::text_layer::OcrWord {
                text: text.into(),
                rect: crate::annot::Box2 {
                    x: 10.0,
                    y: 10.0,
                    w: 50.0,
                    h: 10.0,
                },
                confidence: 1.0,
            }],
            PageSize {
                width: 612.0,
                height: 792.0,
            },
            Rotation::R0,
            "test",
        ))
    }

    fn set(d: &mut Document, pages: &[usize], text: &str) {
        let layers = pages.iter().map(|&i| (id_at(d, i), tl(text))).collect();
        d.apply(Edit::SetTextLayers { layers }, "ocr").unwrap();
    }

    fn layer_texts(d: &Document) -> Vec<(usize, String)> {
        let mut v: Vec<(usize, String)> = d
            .state()
            .text_layers
            .iter()
            .map(|(id, l)| (d.index_of(*id).unwrap(), l.words[0].text.clone()))
            .collect();
        v.sort();
        v
    }

    #[test]
    fn set_text_layers_undo_redo() {
        let mut d = doc();
        set(&mut d, &[1, 3], "a");
        assert_eq!(layer_texts(&d), vec![(1, "a".into()), (3, "a".into())]);
        assert!(d.state().has_text_layer(id_at(&d, 1)));
        assert!(d.is_dirty());
        d.undo();
        assert!(d.state().text_layers.is_empty());
        d.redo();
        assert_eq!(layer_texts(&d).len(), 2);
        // Replacing a page's layer is a new version.
        set(&mut d, &[1], "b");
        assert_eq!(layer_texts(&d), vec![(1, "b".into()), (3, "a".into())]);
        assert_eq!(d.undo_label(), Some("ocr"));
    }

    #[test]
    fn clear_text_layers() {
        let mut d = doc();
        set(&mut d, &[0, 1], "a");
        d.apply(
            Edit::ClearTextLayers {
                ids: vec![id_at(&d, 0), id_at(&d, 5)],
            },
            "clear",
        )
        .unwrap();
        assert_eq!(layer_texts(&d), vec![(1, "a".into())]);
        // Clearing nothing is a no-op version-wise.
        let n = d.stats().versions;
        d.apply(
            Edit::ClearTextLayers {
                ids: vec![id_at(&d, 5)],
            },
            "clear",
        )
        .unwrap();
        assert_eq!(d.stats().versions, n);
    }

    #[test]
    fn unknown_page_is_an_error() {
        let mut d = doc();
        assert!(
            d.apply(
                Edit::SetTextLayers {
                    layers: vec![(PageId(999), tl("x"))]
                },
                "ocr"
            )
            .is_err()
        );
    }

    #[test]
    fn layers_follow_delete_duplicate_replace() {
        let mut d = doc();
        set(&mut d, &[1, 2, 3], "a");
        d.apply(
            Edit::DeletePages {
                ids: vec![id_at(&d, 1)],
            },
            "del",
        )
        .unwrap();
        // Former pages 2 and 3 are now 1 and 2.
        assert_eq!(layer_texts(&d), vec![(1, "a".into()), (2, "a".into())]);
        let dup = id_at(&d, 1);
        d.apply(Edit::DuplicatePages { ids: vec![dup] }, "dup")
            .unwrap();
        assert_eq!(
            layer_texts(&d),
            vec![(1, "a".into()), (2, "a".into()), (3, "a".into())]
        );
        let copy = id_at(&d, 2);
        assert!(Arc::ptr_eq(
            d.state().text_layer(dup).unwrap(),
            d.state().text_layer(copy).unwrap()
        ));
        // Replacing a page's content makes its OCR stale.
        let other = d.add_source(src(2));
        d.apply(
            Edit::ReplacePages {
                ids: vec![copy],
                source: other,
                range: 0..1,
            },
            "replace",
        )
        .unwrap();
        assert_eq!(layer_texts(&d), vec![(1, "a".into()), (3, "a".into())]);
    }

    #[test]
    fn job_batches_coalesce() {
        let mut d = doc();
        let before = d.stats().versions;
        let job = d.begin_job();
        for (i, page) in [0usize, 1, 2].iter().enumerate() {
            d.apply_in_job(
                job,
                Edit::SetTextLayers {
                    layers: vec![(id_at(&d, *page), tl("a"))],
                },
                format!("OCR batch {i}"),
            )
            .unwrap();
            assert!(d.job_is_current(job));
        }
        assert_eq!(d.stats().versions, before + 1);
        assert_eq!(layer_texts(&d).len(), 3);
        assert_eq!(d.undo_label(), Some("OCR batch 2"));
        d.undo();
        assert!(d.state().text_layers.is_empty());
        assert!(!d.job_is_current(job));
        assert!(d.job_is_undone(job));
        d.redo();
        assert_eq!(layer_texts(&d).len(), 3);
        assert!(!d.job_is_undone(job));
    }

    #[test]
    fn job_never_amends_saved_version() {
        let mut d = doc();
        let job = d.begin_job();
        d.apply_in_job(
            job,
            Edit::SetTextLayers {
                layers: vec![(id_at(&d, 0), tl("a"))],
            },
            "ocr",
        )
        .unwrap();
        let n = d.stats().versions;
        d.mark_saved();
        d.apply_in_job(
            job,
            Edit::SetTextLayers {
                layers: vec![(id_at(&d, 1), tl("a"))],
            },
            "ocr",
        )
        .unwrap();
        assert_eq!(d.stats().versions, n + 1, "pushed, not amended");
        assert!(d.is_dirty());
        d.undo();
        assert!(!d.is_dirty(), "the saved version is intact");
        assert_eq!(layer_texts(&d).len(), 1);
    }

    #[test]
    fn job_breaks_after_foreign_edit() {
        let mut d = doc();
        let job = d.begin_job();
        d.apply_in_job(
            job,
            Edit::SetTextLayers {
                layers: vec![(id_at(&d, 0), tl("a"))],
            },
            "ocr",
        )
        .unwrap();
        d.apply(
            Edit::RotatePages {
                ids: vec![id_at(&d, 5)],
                by: Rotation::R90,
            },
            "rotate",
        )
        .unwrap();
        assert!(!d.job_is_current(job));
        assert!(!d.job_is_undone(job), "a later edit is not an undo");
        let n = d.stats().versions;
        d.apply_in_job(
            job,
            Edit::SetTextLayers {
                layers: vec![(id_at(&d, 1), tl("a"))],
            },
            "ocr",
        )
        .unwrap();
        assert_eq!(d.stats().versions, n + 1);
        assert!(d.job_is_current(job));
        // Undo peels the second batch, then the rotation, then the first.
        d.undo();
        assert_eq!(layer_texts(&d).len(), 1);
        assert_eq!(d.page(5).unwrap().rotation, Rotation::R90);
    }

    #[test]
    fn job_first_batch_noop_pushes_nothing() {
        let mut d = doc();
        let job = d.begin_job();
        let n = d.stats().versions;
        d.apply_in_job(
            job,
            Edit::ClearTextLayers {
                ids: vec![id_at(&d, 0)],
            },
            "ocr",
        )
        .unwrap();
        assert_eq!(d.stats().versions, n);
        assert!(!d.job_is_current(job));
    }

    #[test]
    fn rebase_clears_text_layers() {
        let mut d = doc();
        set(&mut d, &[0], "a");
        d.rebase_to_saved(src(6));
        assert!(d.state().text_layers.is_empty());
        assert!(!d.is_dirty());
        // The version before the save still carries its layer, so undoing
        // across the save re-emits it against the old source.
        d.undo();
        assert!(d.state().text_layers.is_empty(), "pre-OCR version");
        d.redo();
        assert!(d.state().text_layers.is_empty(), "rebased version");
    }

    #[test]
    fn marginal_counts_layer_bytes() {
        let mut d = doc();
        set(&mut d, &[0], "hello");
        let m = d.stats().marginal_bytes;
        assert!(*m.last().unwrap() > 0);
        // A duplicate costs the page list plus (at most) one layer estimate.
        let dup = id_at(&d, 0);
        d.apply(Edit::DuplicatePages { ids: vec![dup] }, "dup")
            .unwrap();
        let m2 = d.stats().marginal_bytes;
        let pages = d.page_count() * std::mem::size_of::<PageRef>();
        assert!(*m2.last().unwrap() <= *m.last().unwrap() + pages);
    }

    #[test]
    fn apply_batch_is_one_undo_step() {
        let mut d = doc();
        let before = d.state().clone();
        let p0 = id_at(&d, 0);
        let p3 = id_at(&d, 3);
        d.apply_batch(
            vec![
                Edit::OutlineInsert {
                    parent: None,
                    after: None,
                    title: "W-2".into(),
                    target: Some(p0),
                    view: None,
                },
                Edit::OutlineInsert {
                    parent: None,
                    after: None,
                    title: "1099-INT".into(),
                    target: Some(p3),
                    view: None,
                },
                Edit::RotatePages {
                    ids: vec![p3],
                    by: Rotation::R90,
                },
            ],
            "Plugin edits",
        )
        .unwrap();
        assert_eq!(d.state().outline.len(), before.outline.len() + 2);
        assert_eq!(d.undo_label(), Some("Plugin edits"));
        d.undo();
        assert_eq!(*d.state(), before);
        assert!(!d.can_undo());
    }

    #[test]
    fn apply_batch_failure_applies_nothing() {
        let mut d = doc();
        let before = d.state().clone();
        let p0 = id_at(&d, 0);
        let err = d.apply_batch(
            vec![
                Edit::DeletePages { ids: vec![p0] },
                Edit::MovePages {
                    ids: vec![id_at(&d, 1)],
                    before: Some(PageId(9999)),
                },
            ],
            "bad",
        );
        assert!(err.is_err());
        assert_eq!(*d.state(), before);
        assert!(!d.can_undo());
    }

    #[test]
    fn apply_batch_sums_outcomes() {
        let mut d = doc();
        let ids = vec![id_at(&d, 0)];
        let out = d
            .apply_batch(
                vec![
                    Edit::DuplicatePages { ids: ids.clone() },
                    Edit::DuplicatePages { ids },
                ],
                "dup twice",
            )
            .unwrap();
        assert_eq!(out.pages_added, 2);
        assert_eq!(d.page_count(), 8);
    }

    #[test]
    fn insert_tree_nests_in_one_step() {
        let mut d = doc();
        let (p0, p3) = (id_at(&d, 0), id_at(&d, 3));
        let first_top = d.state().outline[0].id;
        let item = |title: &str, target, depth| NewOutlineItem {
            title: title.into(),
            target,
            view: None,
            depth,
        };
        d.apply(
            Edit::OutlineInsertTree {
                parent: None,
                after: Some(first_top),
                items: vec![
                    item("Accounts", None, 0),
                    item("…4821", Some(p0), 1),
                    item("…9930", Some(p3), 1),
                    item("Loose", Some(p3), 0),
                ],
            },
            "Bookmark matches",
        )
        .unwrap();
        let titles: Vec<(u16, String)> = d
            .state()
            .outline_flat()
            .into_iter()
            .map(|(depth, n)| (depth, n.title.clone()))
            .filter(|(_, t)| {
                ["Accounts", "…4821", "…9930", "Loose", "A", "B"].contains(&t.as_str())
            })
            .collect();
        assert_eq!(
            titles,
            [
                (0, "A".to_string()),
                (0, "Accounts".to_string()),
                (1, "…4821".to_string()),
                (1, "…9930".to_string()),
                (0, "Loose".to_string()),
                (0, "B".to_string()),
            ]
        );
        assert_eq!(d.undo_label(), Some("Bookmark matches"));
        d.undo();
        assert_eq!(d.state().outline.len(), 3);
        // Depths must nest properly.
        let bad = d.apply(
            Edit::OutlineInsertTree {
                parent: None,
                after: None,
                items: vec![item("x", None, 0), item("y", None, 2)],
            },
            "bad",
        );
        assert!(bad.is_err());
    }

    #[test]
    fn edit_serde_round_trip() {
        let annot = Annot {
            id: AnnotId(0),
            kind: AnnotKind::Connector {
                rect: crate::annot::Box2 {
                    x: 1.0,
                    y: 2.0,
                    w: 3.0,
                    h: 4.0,
                },
                label: "1".into(),
                partner: None,
            },
            color: [255, 0, 0],
            opacity: 1.0,
            stroke: 1.0,
            author: "me".into(),
            contents: String::new(),
            modified: "D:20261013000000".into(),
        };
        let edits = vec![
            Edit::MovePages {
                ids: vec![PageId(1), PageId(2)],
                before: None,
            },
            Edit::RotatePages {
                ids: vec![PageId(0)],
                by: Rotation::R270,
            },
            Edit::InsertPages {
                source: SourceId(1),
                range: 0..3,
                before: Some(PageId(4)),
                outline: Some(flat(&[("X", Some(0), 0)])),
            },
            Edit::OutlineEdit {
                id: OutlineId(3),
                title: Some("T".into()),
                target: Some(None),
                color: Some(Some([1, 2, 3])),
                view: None,
            },
            Edit::ConnectorAdd {
                a: (PageId(0), annot.clone()),
                b: (PageId(1), annot),
            },
            Edit::SetTextLayers {
                layers: vec![(PageId(0), tl("hi"))],
            },
        ];
        let json = serde_json::to_string(&edits).unwrap();
        let back: Vec<Edit> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, edits);
        // Absent keys leave a bookmark field unchanged; null clears it.
        let e: Edit = serde_json::from_str(r#"{"OutlineEdit":{"id":3,"target":null}}"#).unwrap();
        assert_eq!(
            e,
            Edit::OutlineEdit {
                id: OutlineId(3),
                title: None,
                target: Some(None),
                color: None,
                view: None,
            }
        );
    }
}
