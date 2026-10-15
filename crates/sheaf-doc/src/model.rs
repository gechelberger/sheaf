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
    /// The file's bytes as read when it was opened, when the opener kept
    /// them (the app does, sharing the renderer's copy). Saves read these,
    /// not the path: after a save in place the path holds the new file, and
    /// a version from before the save still means the old one. Without
    /// them, the path is read (a cached file sheaf made, which never
    /// changes; or a headless run).
    pub data: Option<SourceData>,
}

/// A file's bytes, shared (an `Arc`, cheap to clone).
#[derive(Clone)]
pub struct SourceData(pub Arc<Vec<u8>>);

impl std::fmt::Debug for SourceData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SourceData({} bytes)", self.0.len())
    }
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
    pub style: TextStyle,
    pub children: Vector<OutlineNode>,
}

/// How a bookmark's title is drawn (its `/F`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextStyle {
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
}

impl TextStyle {
    pub fn is_plain(&self) -> bool {
        !self.bold && !self.italic
    }
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
    /// `/C` and `/F`, which PDFium's outline leaves out (see
    /// `file::style_flat_items`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[u8; 3]>,
    #[serde(default, skip_serializing_if = "TextStyle::is_plain")]
    pub style: TextStyle,
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
    /// Other tools' comments deleted in sheaf, by page and name
    /// (`ForeignAnnot::name`). A save leaves them, and their own replies,
    /// out of the file.
    pub removed_comments: im::HashMap<PageId, im::OrdSet<String>>,
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

    /// The first connector label (1–50, then A–ZZ) no connector uses yet;
    /// past those, one more than the highest number.
    pub fn next_connector_label(&self) -> String {
        let used: HashSet<&str> = self
            .annots
            .values()
            .flatten()
            .filter_map(|a| match &a.kind {
                AnnotKind::Connector { label, .. } => Some(label.as_str()),
                _ => None,
            })
            .collect();
        if let Some(free) = crate::annot::connector_labels().find(|l| !used.contains(l.as_str())) {
            return free;
        }
        let max = used
            .iter()
            .filter_map(|l| l.trim().parse::<u32>().ok())
            .max()
            .unwrap_or(0);
        (max + 1).to_string()
    }

    /// Every mark in `group` (see `annot::group_of`), in document order:
    /// by page, then down the page, then across. With each, its page's
    /// position.
    pub fn group_members(&self, group: &crate::annot::Group) -> Vec<(usize, PageId, Annot)> {
        let mut out = Vec::new();
        for (i, p) in self.pages.iter().enumerate() {
            let mut on_page: Vec<&Annot> = self
                .annots
                .get(&p.id)
                .into_iter()
                .flatten()
                .filter(|a| crate::annot::group_of(&a.kind).as_ref() == Some(group))
                .collect();
            on_page.sort_by(|a, b| {
                let (ba, bb) = (a.bounds(), b.bounds());
                ba.y.total_cmp(&bb.y).then(ba.x.total_cmp(&bb.x))
            });
            out.extend(on_page.into_iter().map(|a| (i, p.id, a.clone())));
        }
        out
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
        /// sheaf's own marks in the file, by source page index. They come
        /// in on the new pages with fresh ids (see `import_marks`).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        annots: Vec<(u32, Vec<Annot>)>,
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
        /// sheaf's own marks on the replacement pages, by source page
        /// index; added beside the marks the kept pages already have.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        annots: Vec<(u32, Vec<Annot>)>,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        style: Option<TextStyle>,
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
    /// Delete other tools' comments from a page, by name; a save leaves
    /// them out. Our replies and statuses in their threads go too.
    CommentsDelete {
        page: PageId,
        names: Vec<String>,
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

/// A version's place in the history tree. Ids aren't reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VersionId(pub u64);

/// One edit as applied, with what it takes to apply it again exactly: the
/// id counters it started from and the seed its mark ids came from. So a
/// replay hands out the same page, bookmark and mark ids as the first time.
#[derive(Debug, Clone)]
pub struct Step {
    pub edit: Edit,
    ids: Ids,
}

#[derive(Debug, Clone, Copy)]
struct Ids {
    next_page: u64,
    next_outline: u64,
    seed: u64,
}

/// A save that wrote a version.
#[derive(Debug, Clone, PartialEq)]
pub struct Saved {
    pub path: PathBuf,
    /// PDF date string.
    pub at: String,
}

/// A node of the history tree: a version of the document, and the edits
/// that made it from its parent (the edge into it). See `todos/m9.md`,
/// decision 16.
struct Version {
    parent: Option<VersionId>,
    /// Oldest first.
    children: Vec<VersionId>,
    /// The child redo goes to: the one last left by undo, or made last.
    redo_to: Option<VersionId>,
    label: String,
    author: String,
    /// When it was made: a PDF date string.
    at: String,
    /// The edits from the parent; empty for the root. A job's later
    /// batches add to its version's steps.
    steps: Vec<Step>,
    /// Sources registered since the parent (inserted files), which the
    /// steps refer to.
    sources: Vec<SourceId>,
    /// The state, unless trimming dropped it; then a jump rebuilds it by
    /// replaying the steps from the nearest ancestor that kept one.
    state: Option<DocState>,
    marginal_bytes: usize,
    /// The job that produced this version, if it may still amend it.
    job: Option<JobId>,
    /// Saves that wrote it, oldest first.
    saves: Vec<Saved>,
}

/// A version as the History panel shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct VersionView {
    pub id: VersionId,
    pub label: String,
    pub author: String,
    /// PDF date string.
    pub at: String,
    /// What its edits did, a line each.
    pub edits: Vec<String>,
    pub saves: Vec<Saved>,
    pub current: bool,
    /// The version the file on disk holds.
    pub saved: bool,
    /// Past the current version: undone, and reachable with redo.
    pub undone: bool,
    /// First versions of other branches from here: edits that were undone
    /// and then replaced by others.
    pub branches: Vec<VersionId>,
    /// Its snapshot was dropped to save memory; jumping there replays it.
    pub replays: bool,
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
    versions: HashMap<VersionId, Version>,
    root: VersionId,
    cursor: VersionId,
    /// The version the primary file on disk holds, if any.
    saved: Option<VersionId>,
    next_version: u64,
    /// Sources registered since the last version was made, for the next.
    unlogged_sources: Vec<SourceId>,
    /// Who makes the versions from now on (the app sets it).
    author: String,
    /// Where mark ids come from: reseeded for every step (see [`Step`]).
    rng: fastrand::Rng,
    next_page: u64,
    next_outline: u64,
    next_source: u32,
    policy: Box<dyn HistoryPolicy>,
    next_job: u64,
    /// Outline as it exists in the primary file on disk.
    saved_outline: Vector<OutlineNode>,
    saved_labels: Option<Vector<crate::labels::LabelSpec>>,
    saved_removed: im::HashMap<PageId, im::OrdSet<String>>,
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
            versions: HashMap::new(),
            root: VersionId(0),
            cursor: VersionId(0),
            saved: Some(VersionId(0)),
            next_version: 1,
            unlogged_sources: Vec::new(),
            author: crate::annot::default_author(),
            rng: fastrand::Rng::new(),
            next_page: 0,
            next_outline: 0,
            next_source: 1,
            policy,
            next_job: 1,
            saved_outline: Vector::new(),
            saved_labels: None,
            saved_removed: im::HashMap::new(),
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
        doc.versions.insert(
            VersionId(0),
            Version {
                parent: None,
                children: Vec::new(),
                redo_to: None,
                label: "Open".into(),
                author: String::new(),
                at: crate::annot::pdf_date_now(),
                steps: Vec::new(),
                sources: vec![source],
                state: Some(DocState {
                    pages,
                    outline,
                    annots: im::HashMap::new(),
                    text_layers: im::HashMap::new(),
                    removed_comments: im::HashMap::new(),
                    labels: None,
                    header_footer: None,
                }),
                marginal_bytes: 0,
                job: None,
                saves: Vec::new(),
            },
        );
        doc
    }

    /// Who the versions made from now on are by.
    pub fn set_author(&mut self, name: impl Into<String>) {
        self.author = name.into();
    }

    /// Edits are only loaded into the opened version before any edit.
    fn opened_state_mut(&mut self, what: &str) -> Result<&mut DocState> {
        if self.versions.len() != 1 {
            bail!("{what} can only be loaded before editing");
        }
        Ok(self
            .versions
            .get_mut(&self.root)
            .and_then(|v| v.state.as_mut())
            .expect("the opened version keeps its state"))
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

    /// Register another file so its pages can be inserted. The next
    /// version records it, with the edit that uses it.
    pub fn add_source(&mut self, info: SourceInfo) -> SourceId {
        let id = self.register_source(info);
        self.unlogged_sources.push(id);
        id
    }

    fn register_source(&mut self, info: SourceInfo) -> SourceId {
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
        let by_index: Vec<PageId> = self
            .opened_state_mut("the outline")?
            .pages
            .iter()
            .map(|p| p.id)
            .collect();
        let outline = self.outline_from_flat(outline, &by_index);
        self.opened_state_mut("the outline")?.outline = outline.clone();
        self.saved_outline = outline;
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
        let st = self.opened_state_mut("the header/footer")?;
        let ids: Vec<PageId> = st.pages.iter().map(|p| p.id).collect();
        hf.pages = indexes.map(|ix| ix.iter().filter_map(|&i| ids.get(i).copied()).collect());
        let hf = Some(Arc::new(hf));
        st.header_footer = hf.clone();
        self.baked_header_footer = hf;
        Ok(())
    }

    /// The file's page labels, read after open: ranges by page index.
    pub fn set_initial_labels(&mut self, ranges: &[crate::labels::LabelRange]) -> Result<()> {
        let st = self.opened_state_mut("page labels")?;
        let ids: Vec<PageId> = st.pages.iter().map(|p| p.id).collect();
        let specs: Vector<_> = crate::labels::to_specs(ranges, &ids).into_iter().collect();
        let specs = (!specs.is_empty()).then_some(specs);
        st.labels = specs.clone();
        self.saved_labels = specs;
        Ok(())
    }

    pub fn set_initial_annots(&mut self, by_index: HashMap<u32, Vec<Annot>>) -> Result<()> {
        let st = self.opened_state_mut("annotations")?;
        let ids: Vec<PageId> = st.pages.iter().map(|p| p.id).collect();
        let mut map = im::HashMap::new();
        for (index, list) in by_index {
            if let Some(&pid) = ids.get(index as usize) {
                map.insert(pid, list.into_iter().collect());
            }
        }
        st.annots = map.clone();
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
        // A changed connector changes its group's cycle of /Dest: every
        // page with one of its label is written again.
        let labels: HashSet<&str> = dirty
            .iter()
            .flat_map(|pid| cur.get(pid).into_iter().chain(self.saved_annots.get(pid)))
            .flatten()
            .filter_map(|a| match &a.kind {
                AnnotKind::Connector { label, .. } => Some(label.as_str()),
                _ => None,
            })
            .collect();
        if !labels.is_empty() {
            for (pid, list) in cur.iter() {
                if list.iter().any(|a| {
                    matches!(&a.kind, AnnotKind::Connector { label, .. } if labels.contains(label.as_str()))
                }) {
                    dirty.insert(*pid);
                }
            }
        }
        // Other tools' comments deleted (or brought back) since.
        let removed = &self.state().removed_comments;
        for pid in removed.keys().chain(self.saved_removed.keys()) {
            if removed.get(pid) != self.saved_removed.get(pid) {
                dirty.insert(*pid);
            }
        }
        dirty
    }

    /// Bookmark colors and styles read from the file (which PDFium's
    /// outline leaves out), matched to the loaded bookmarks in pre-order by
    /// title. Only before any edit, like the other `set_initial_*`.
    pub fn set_initial_outline_styles(
        &mut self,
        styles: &[crate::file::OutlineStyle],
    ) -> Result<()> {
        if styles.is_empty() {
            return Ok(());
        }
        fn walk(
            nodes: &Vector<OutlineNode>,
            styles: &mut std::slice::Iter<crate::file::OutlineStyle>,
        ) -> Vector<OutlineNode> {
            nodes
                .iter()
                .map(|n| {
                    let mut n = n.clone();
                    if let Some(s) = styles.next()
                        && s.title.trim() == n.title.trim()
                    {
                        n.color = s.color;
                        n.style = s.style;
                    }
                    n.children = walk(&n.children, styles);
                    n
                })
                .collect()
        }
        let st = self.opened_state_mut("bookmark styles")?;
        let styled = walk(&st.outline, &mut styles.iter());
        st.outline = styled.clone();
        self.saved_outline = styled;
        Ok(())
    }

    pub fn state(&self) -> &DocState {
        self.versions[&self.cursor]
            .state
            .as_ref()
            .expect("the current version keeps its state")
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
    ///
    /// The content is the same, so this re-expresses the version in place
    /// rather than making a new one: a save is a mark on the version, not
    /// an edit. Saved versions keep their snapshots (see `trim`), so
    /// nothing is ever replayed across a save.
    pub fn rebase_to_saved(&mut self, saved: SourceInfo) -> SourceId {
        let path = saved.path.clone();
        let id = self.register_source(saved);
        self.primary = id;
        let v = self
            .versions
            .get_mut(&self.cursor)
            .expect("current version");
        v.saves.push(Saved {
            path,
            at: crate::annot::pdf_date_now(),
        });
        let st = v
            .state
            .as_mut()
            .expect("the current version keeps its state");
        st.text_layers = im::HashMap::new();
        st.pages = st
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
        let st = self.state().clone();
        self.saved_outline = st.outline;
        self.saved_annots = st.annots;
        self.saved_removed = st.removed_comments;
        self.saved_labels = st.labels;
        self.baked_header_footer = st.header_footer;
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
        self.versions[&self.cursor].parent.is_some()
    }

    pub fn can_redo(&self) -> bool {
        self.redo_target().is_some()
    }

    /// Label of the edit that `undo` would revert.
    pub fn undo_label(&self) -> Option<&str> {
        let v = &self.versions[&self.cursor];
        v.parent.map(|_| v.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo_target()
            .map(|id| self.versions[&id].label.as_str())
    }

    /// Where redo goes: the child last left by undo (or made last).
    fn redo_target(&self) -> Option<VersionId> {
        let v = &self.versions[&self.cursor];
        v.redo_to
            .filter(|c| v.children.contains(c))
            .or_else(|| v.children.last().copied())
    }

    pub fn undo(&mut self) -> Option<&str> {
        let from = self.cursor;
        let parent = self.versions[&from].parent?;
        if let Err(e) = self.ensure_state(parent) {
            log::warn!("undo: {e:#}");
            return None;
        }
        self.versions.get_mut(&parent).expect("parent").redo_to = Some(from);
        self.cursor = parent;
        Some(self.versions[&from].label.as_str())
    }

    pub fn redo(&mut self) -> Option<&str> {
        let to = self.redo_target()?;
        if let Err(e) = self.ensure_state(to) {
            log::warn!("redo: {e:#}");
            return None;
        }
        self.cursor = to;
        Some(self.versions[&to].label.as_str())
    }

    /// Make `id` the current version, as undo and redo do: not an edit,
    /// so jumping back and forth is free. Redo from any version on the way
    /// to it then leads toward it.
    pub fn jump(&mut self, id: VersionId) -> Result<()> {
        if !self.versions.contains_key(&id) {
            bail!("that version is no longer kept");
        }
        self.ensure_state(id)?;
        let mut child = id;
        while let Some(parent) = self.versions[&child].parent {
            self.versions.get_mut(&parent).expect("parent").redo_to = Some(child);
            child = parent;
        }
        self.cursor = id;
        Ok(())
    }

    pub fn current_version(&self) -> VersionId {
        self.cursor
    }

    /// The edits that made version `id` from its parent, and the sources
    /// that came in with them: the edge into it, which is what a merge
    /// would replay (`todos/file-contention.md`).
    pub fn edge(&self, id: VersionId) -> Option<(&[Step], &[SourceId])> {
        let v = self.versions.get(&id)?;
        Some((&v.steps, &v.sources))
    }

    pub fn apply(&mut self, edit: Edit, label: impl Into<String>) -> Result<EditOutcome> {
        let (state, outcome, step) = self.step(self.state().clone(), edit)?;
        if state == *self.state() {
            return Ok(outcome);
        }
        self.push(state, label.into(), vec![step], None);
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
        let mut steps = Vec::with_capacity(edits.len());
        for (i, edit) in edits.into_iter().enumerate() {
            let (next, out, step) = self
                .step(state, edit)
                .with_context(|| format!("edit {} of batch", i + 1))?;
            state = next;
            steps.push(step);
            total.pages_removed += out.pages_removed;
            total.pages_added += out.pages_added;
            total.bookmarks_removed += out.bookmarks_removed;
            total.bookmarks_added += out.bookmarks_added;
        }
        if state != *self.state() {
            self.push(state, label.into(), steps, None);
        }
        Ok(total)
    }

    /// Start a job whose batches of edits should undo as one step.
    pub fn begin_job(&mut self) -> JobId {
        let id = JobId(self.next_job);
        self.next_job += 1;
        id
    }

    /// Like [`apply`](Self::apply), but when the current version was made
    /// by `job`, has nothing after it, and isn't the saved one, the edit
    /// joins it instead of making a new version. The first batch of a job
    /// makes a version; later batches join it, so the whole job is one
    /// undo step. A save, an undo or an unrelated edit in between ends the
    /// chain, and the rest of the job becomes another step.
    pub fn apply_in_job(
        &mut self,
        job: JobId,
        edit: Edit,
        label: impl Into<String>,
    ) -> Result<EditOutcome> {
        let (state, outcome, step) = self.step(self.state().clone(), edit)?;
        let current = &self.versions[&self.cursor];
        let amend = current.job == Some(job)
            && current.children.is_empty()
            && self.saved != Some(self.cursor);
        if amend {
            let marginal_bytes = current
                .parent
                .and_then(|p| self.versions[&p].state.as_ref())
                .map_or(0, |prev| Self::marginal(prev, &state));
            let v = self.versions.get_mut(&self.cursor).expect("current");
            v.state = Some(state);
            v.label = label.into();
            v.steps.push(step);
            v.marginal_bytes = marginal_bytes;
            self.trim();
        } else {
            if state == *self.state() {
                return Ok(outcome);
            }
            self.push(state, label.into(), vec![step], Some(job));
        }
        Ok(outcome)
    }

    /// Whether the current version is the one `job` is building, i.e. the
    /// user hasn't undone it or edited past it. Meaningful once the job has
    /// applied its first batch.
    pub fn job_is_current(&self, job: JobId) -> bool {
        self.versions[&self.cursor].job == Some(job)
    }

    /// Whether the user has undone `job`'s version: it's past the current
    /// version, reachable by redo. An unrelated edit after the job doesn't
    /// count.
    pub fn job_is_undone(&self, job: JobId) -> bool {
        let mut stack = self.versions[&self.cursor].children.clone();
        while let Some(id) = stack.pop() {
            let v = &self.versions[&id];
            if v.job == Some(job) {
                return true;
            }
            stack.extend(v.children.iter().copied());
        }
        false
    }

    /// Run `edit` on `state` as a new step, recording the id counters and a
    /// fresh seed for its mark ids, so a replay hands out the same ids.
    fn step(&mut self, state: DocState, edit: Edit) -> Result<(DocState, EditOutcome, Step)> {
        let ids = Ids {
            next_page: self.next_page,
            next_outline: self.next_outline,
            seed: fastrand::u64(..),
        };
        self.rng = fastrand::Rng::with_seed(ids.seed);
        let (state, out) = self.compute(state, edit.clone())?;
        Ok((state, out, Step { edit, ids }))
    }

    /// Run a recorded step again, exactly as the first time.
    fn replay_step(&mut self, state: DocState, step: &Step) -> Result<DocState> {
        let (page, outline) = (self.next_page, self.next_outline);
        self.next_page = step.ids.next_page;
        self.next_outline = step.ids.next_outline;
        self.rng = fastrand::Rng::with_seed(step.ids.seed);
        let result = self.compute(state, step.edit.clone()).map(|(s, _)| s);
        // Counters only grow; leave them as they were before the replay.
        self.next_page = self.next_page.max(page);
        self.next_outline = self.next_outline.max(outline);
        result
    }

    /// Make sure version `id` has its state, replaying its steps (and its
    /// ancestors') from the nearest ancestor that kept one.
    fn ensure_state(&mut self, id: VersionId) -> Result<()> {
        if self.versions[&id].state.is_some() {
            return Ok(());
        }
        let mut path = vec![id];
        let mut at = id;
        let base = loop {
            let parent = self.versions[&at]
                .parent
                .context("no version to replay from")?;
            if self.versions[&parent].state.is_some() {
                break parent;
            }
            path.push(parent);
            at = parent;
        };
        let mut state = self.versions[&base].state.clone().expect("base state");
        for v in path.iter().rev() {
            let steps = self.versions[v].steps.clone();
            for step in &steps {
                state = self
                    .replay_step(state, step)
                    .with_context(|| format!("replaying {:?}", self.versions[v].label))?;
            }
        }
        self.versions.get_mut(&id).expect("version").state = Some(state);
        Ok(())
    }

    /// A new version after the current one, with the edits that made it.
    /// The versions that were after the current one stay, as a branch.
    fn push(&mut self, state: DocState, label: String, steps: Vec<Step>, job: Option<JobId>) {
        let id = VersionId(self.next_version);
        self.next_version += 1;
        let parent = self.cursor;
        let marginal_bytes = self.versions[&parent]
            .state
            .as_ref()
            .map_or(0, |prev| Self::marginal(prev, &state));
        let p = self.versions.get_mut(&parent).expect("parent");
        p.children.push(id);
        p.redo_to = Some(id);
        self.versions.insert(
            id,
            Version {
                parent: Some(parent),
                children: Vec::new(),
                redo_to: None,
                label,
                author: self.author.clone(),
                at: crate::annot::pdf_date_now(),
                steps,
                sources: std::mem::take(&mut self.unlogged_sources),
                state: Some(state),
                marginal_bytes,
                job,
                saves: Vec::new(),
            },
        );
        self.cursor = id;
        self.trim();
    }

    /// Versions that must keep their snapshots: the opened one (every
    /// replay starts somewhere), the current one, and every saved one (a
    /// save re-expressed it in place, so it can't be replayed).
    fn keeps_snapshot(&self, id: VersionId) -> bool {
        id == self.root
            || id == self.cursor
            || Some(id) == self.saved
            || !self.versions[&id].saves.is_empty()
    }

    /// The versions from the opened one to the current one.
    fn line(&self) -> Vec<VersionId> {
        let mut line = vec![self.cursor];
        while let Some(p) = self.versions[line.last().expect("nonempty")].parent {
            line.push(p);
        }
        line.reverse();
        line
    }

    /// Snapshots the policy may drop, in the order to drop them: versions
    /// off the current line first (other branches, undone ones), then the
    /// current line's, oldest first.
    fn droppable(&self) -> Vec<VersionId> {
        let line: HashSet<VersionId> = self.line().into_iter().collect();
        let mut ids: Vec<VersionId> = self
            .versions
            .iter()
            .filter(|(id, v)| v.state.is_some() && !self.keeps_snapshot(**id))
            .map(|(id, _)| *id)
            .collect();
        ids.sort_by_key(|id| (line.contains(id), *id));
        ids
    }

    fn trim(&mut self) {
        let droppable = self.droppable();
        let stats = self.stats_for(&droppable);
        let n = self.policy.trim(&stats).min(droppable.len());
        for id in &droppable[..n] {
            self.versions.get_mut(id).expect("version").state = None;
        }
        self.prune();
    }

    /// Keep the tree to [`crate::history::MAX_VERSIONS`]: drop versions on
    /// other branches, oldest first, then the oldest versions before the
    /// saved one. Edits back to the last save always stay (a later merge
    /// replays them).
    fn prune(&mut self) {
        let max = crate::history::MAX_VERSIONS;
        while self.versions.len() > max {
            let line: HashSet<VersionId> = self.line().into_iter().collect();
            let leaf = self
                .versions
                .iter()
                .filter(|(id, v)| {
                    v.children.is_empty() && !line.contains(id) && Some(**id) != self.saved
                })
                .map(|(id, _)| *id)
                .min();
            let Some(leaf) = leaf else { break };
            self.remove_version(leaf);
        }
        // Then move the root forward along the line, up to the saved
        // version (or the current one when the saved one is elsewhere).
        let line = self.line();
        let stop = self
            .saved
            .and_then(|s| line.iter().position(|v| *v == s))
            .unwrap_or(line.len() - 1);
        let mut i = 0;
        while self.versions.len() > max && i < stop {
            let next = line[i + 1];
            if self.ensure_state(next).is_err() {
                break;
            }
            let old = line[i];
            for sibling in self.versions[&old].children.clone() {
                if sibling != next {
                    self.remove_subtree(sibling);
                }
            }
            self.versions.remove(&old);
            let new_root = self.versions.get_mut(&next).expect("next");
            new_root.parent = None;
            self.root = next;
            i += 1;
        }
    }

    fn remove_version(&mut self, id: VersionId) {
        if let Some(v) = self.versions.remove(&id)
            && let Some(p) = v.parent
            && let Some(parent) = self.versions.get_mut(&p)
        {
            parent.children.retain(|c| *c != id);
            if parent.redo_to == Some(id) {
                parent.redo_to = None;
            }
        }
        if self.saved == Some(id) {
            self.saved = None;
        }
    }

    fn remove_subtree(&mut self, id: VersionId) {
        let mut stack = vec![id];
        while let Some(v) = stack.pop() {
            if let Some(version) = self.versions.get(&v) {
                stack.extend(version.children.iter().copied());
            }
            self.remove_version(v);
        }
    }

    fn stats_for(&self, droppable: &[VersionId]) -> HistoryStats {
        let mut kept: Vec<VersionId> = self
            .versions
            .iter()
            .filter(|(id, v)| v.state.is_some() && !droppable.contains(id))
            .map(|(id, _)| *id)
            .collect();
        // The current version's last.
        kept.sort_by_key(|id| (*id == self.cursor, *id));
        let marginal_bytes: Vec<usize> = droppable
            .iter()
            .chain(&kept)
            .map(|id| self.versions[id].marginal_bytes)
            .collect();
        HistoryStats {
            versions: marginal_bytes.len(),
            cursor: marginal_bytes.len() - 1,
            marginal_bytes,
            undone_source_bytes: 0,
        }
    }

    /// Snapshots kept, and their estimated bytes (droppable ones first).
    pub fn stats(&self) -> HistoryStats {
        self.stats_for(&self.droppable())
    }

    /// How many versions the history holds, with or without snapshots.
    pub fn version_count(&self) -> usize {
        self.versions.len()
    }

    fn view(&self, id: VersionId, shown_child: Option<VersionId>, undone: bool) -> VersionView {
        let v = &self.versions[&id];
        let before = v.parent.and_then(|p| self.versions[&p].state.as_ref());
        VersionView {
            id,
            label: v.label.clone(),
            author: v.author.clone(),
            at: v.at.clone(),
            edits: v
                .steps
                .iter()
                .map(|s| crate::describe::describe(&s.edit, before))
                .collect(),
            saves: v.saves.clone(),
            current: id == self.cursor,
            saved: Some(id) == self.saved,
            undone,
            branches: v
                .children
                .iter()
                .copied()
                .filter(|c| Some(*c) != shown_child)
                .collect(),
            replays: v.state.is_none(),
        }
    }

    /// The history as the panel lists it: from the oldest version kept to
    /// the current one, then on along redo.
    pub fn timeline(&self) -> Vec<VersionView> {
        let line = self.line();
        let mut out: Vec<VersionView> = line
            .iter()
            .enumerate()
            .map(|(i, id)| {
                let next = line.get(i + 1).copied().or_else(|| self.redo_target());
                self.view(*id, next, false)
            })
            .collect();
        let mut at = self.redo_target();
        while let Some(id) = at {
            let v = &self.versions[&id];
            let next = v
                .redo_to
                .filter(|c| v.children.contains(c))
                .or_else(|| v.children.last().copied());
            out.push(self.view(id, next, true));
            at = next;
        }
        out
    }

    /// The versions of a branch, from its first along its newest edits.
    pub fn branch(&self, start: VersionId) -> Vec<VersionView> {
        let mut out = Vec::new();
        let mut at = Some(start);
        while let Some(id) = at.filter(|id| self.versions.contains_key(id)) {
            let v = &self.versions[&id];
            let next = v
                .redo_to
                .filter(|c| v.children.contains(c))
                .or_else(|| v.children.last().copied());
            out.push(self.view(id, next, id != self.cursor));
            at = next;
        }
        out
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
                style: TextStyle::default(),
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
                color: item.color,
                style: item.style,
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
                    st.removed_comments.remove(id);
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
                annots,
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
                let incoming = annots
                    .into_iter()
                    .filter_map(|(index, list)| {
                        let page = *by_index.get(index as usize)?;
                        (page != PageId(u64::MAX)).then_some((page, list))
                    })
                    .collect();
                import_marks(&mut self.rng, &mut st, incoming);
            }
            Edit::DuplicatePages { ids } => {
                let set: HashSet<PageId> = ids.into_iter().collect();
                let mut pages = Vector::new();
                for p in st.pages.iter() {
                    pages.push_back(*p);
                    if set.contains(&p.id) {
                        let copy = self.fresh_page_id();
                        if let Some(list) = st.annots.get(&p.id).cloned() {
                            let mut taken = HashSet::new();
                            let list: Vector<Annot> = list
                                .into_iter()
                                .map(|mut a| {
                                    a.id = fresh_annot_id(&mut self.rng, &st, &mut taken);
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
            Edit::ReplacePages {
                ids,
                source,
                range,
                annots,
            } => {
                let info = self.sources.get(&source).context("unknown source")?.clone();
                if range.end > info.page_count {
                    bail!("range exceeds source page count");
                }
                let mut replacements = range.clone();
                let mut last_pos = None;
                let mut extra_old = HashSet::new();
                // Which page now shows each replacement, for its marks.
                let mut shown_by: HashMap<u32, PageId> = HashMap::new();
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
                            shown_by.insert(index, *id);
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
                    let id = self.fresh_page_id();
                    pages.insert(
                        at + k,
                        PageRef {
                            id,
                            source,
                            index,
                            rotation: Rotation::R0,
                        },
                    );
                    shown_by.insert(index, id);
                    out.pages_added += 1;
                }
                if !extra_old.is_empty() {
                    out.pages_removed = extra_old.len();
                    pages = pages
                        .iter()
                        .filter(|p| !extra_old.contains(&p.id))
                        .copied()
                        .collect();
                    for id in &extra_old {
                        st.annots.remove(id);
                        st.text_layers.remove(id);
                    }
                    let (outline, removed) = cascade_delete(&st.outline, &extra_old);
                    st.outline = outline;
                    out.bookmarks_removed = removed;
                }
                st.pages = pages;
                let incoming = annots
                    .into_iter()
                    .filter_map(|(index, list)| Some((*shown_by.get(&index)?, list)))
                    .collect();
                import_marks(&mut self.rng, &mut st, incoming);
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
                    style: TextStyle::default(),
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
                                removed_comments: st.removed_comments.clone(),
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
                style,
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
                    if let Some(s) = style {
                        n.style = s;
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
                annot.id = fresh_annot_id(&mut self.rng, &st, &mut HashSet::new());
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
                // Its thread goes with it: replies and statuses, and theirs.
                let mut gone = vec![removed.name()];
                while let Some(name) = gone.pop() {
                    let (thread, rest): (Vector<Annot>, Vector<Annot>) = list
                        .into_iter()
                        .partition(|a| a.thread_parent() == Some(name.as_str()));
                    gone.extend(thread.iter().map(Annot::name));
                    list = rest;
                }
                if list.is_empty() {
                    st.annots.remove(&page);
                } else {
                    st.annots.insert(page, list);
                }
                // A connector is one of a group by label: the others stay
                // (M9 decision 8).
            }
            Edit::CommentsDelete { page, names } => {
                if st.index_of(page).is_none() {
                    bail!("page not found");
                }
                let mut set = st.removed_comments.get(&page).cloned().unwrap_or_default();
                set.extend(names.iter().cloned());
                st.removed_comments.insert(page, set);
                // Our replies and statuses to them, and to those.
                if let Some(mut list) = st.annots.get(&page).cloned() {
                    let mut gone = names;
                    while let Some(name) = gone.pop() {
                        let (thread, rest): (Vector<Annot>, Vector<Annot>) = list
                            .into_iter()
                            .partition(|a| a.thread_parent() == Some(name.as_str()));
                        gone.extend(thread.iter().map(Annot::name));
                        list = rest;
                    }
                    if list.is_empty() {
                        st.annots.remove(&page);
                    } else {
                        st.annots.insert(page, list);
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
                let mut taken = HashSet::new();
                a.id = fresh_annot_id(&mut self.rng, &st, &mut taken);
                b.id = fresh_annot_id(&mut self.rng, &st, &mut taken);
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

/// A mark id no mark in `st` has, and not in `taken` (ids handed out
/// earlier in the same edit, which records this one). Random rather than
/// counted, so two copies of a file never give different marks the same
/// `/NM`; 53 bits, so the JSON is exact in any reader. From the step's
/// seeded `rng`, so a replay gives the same ids.
fn fresh_annot_id(rng: &mut fastrand::Rng, st: &DocState, taken: &mut HashSet<AnnotId>) -> AnnotId {
    loop {
        let id = AnnotId(rng.u64(1..1 << 53));
        if !taken.contains(&id) && st.find_annot(id).is_none() {
            taken.insert(id);
            return id;
        }
    }
}

/// Marks that came with pages from another file, onto their new pages in
/// `st`. They get fresh ids (the file's could clash with ours), and a
/// connector keeps its partner only if the partner came too.
fn import_marks(rng: &mut fastrand::Rng, st: &mut DocState, incoming: Vec<(PageId, Vec<Annot>)>) {
    let mut taken = HashSet::new();
    let mut ids = HashMap::new();
    for (_, list) in &incoming {
        for a in list {
            ids.insert(a.id, fresh_annot_id(rng, st, &mut taken));
        }
    }
    for (page, list) in incoming {
        let mut on_page = st.annots.get(&page).cloned().unwrap_or_default();
        for mut a in list {
            a.id = ids[&a.id];
            if let AnnotKind::Connector { partner, .. } = &mut a.kind {
                *partner = partner.and_then(|p| ids.get(&p).copied());
            }
            on_page.push_back(a);
        }
        if !on_page.is_empty() {
            st.annots.insert(page, on_page);
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
            data: None,
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
                color: None,
                style: Default::default(),
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
                    annots: Vec::new(),
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
                annots: Vec::new(),
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
                style: None,
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

    fn rotate(d: &mut Document, page: usize, label: &str) {
        d.apply(
            Edit::RotatePages {
                ids: vec![id_at(d, page)],
                by: Rotation::R90,
            },
            label,
        )
        .unwrap();
    }

    #[test]
    fn an_edit_after_undo_keeps_the_old_branch() {
        let mut d = doc();
        rotate(&mut d, 0, "a");
        rotate(&mut d, 1, "b");
        rotate(&mut d, 2, "c");
        d.undo();
        d.undo();
        rotate(&mut d, 3, "d");
        let line: Vec<String> = d.timeline().iter().map(|v| v.label.clone()).collect();
        assert_eq!(line, vec!["Open", "a", "d"]);
        let a = &d.timeline()[1];
        assert_eq!(a.branches.len(), 1, "b and c are still there");
        let b = a.branches[0];
        assert_eq!(
            d.branch(b)
                .iter()
                .map(|v| v.label.as_str())
                .collect::<Vec<_>>(),
            vec!["b", "c"]
        );
        d.jump(b).unwrap();
        assert_eq!(d.page(1).unwrap().rotation, Rotation::R90);
        assert_eq!(d.page(3).unwrap().rotation, Rotation::R0);
        assert_eq!(d.redo(), Some("c"));
        // Undo to a, and redo now follows the branch last visited.
        d.undo();
        d.undo();
        assert_eq!(d.redo_label(), Some("b"));
    }

    #[test]
    fn jumps_go_back_and_forth_freely() {
        let mut d = doc();
        for i in 0..10 {
            rotate(&mut d, i % 6, &format!("r{i}"));
        }
        let states: Vec<(VersionId, DocState)> = d
            .timeline()
            .iter()
            .map(|v| v.id)
            .collect::<Vec<_>>()
            .into_iter()
            .map(|id| {
                d.jump(id).unwrap();
                (id, d.state().clone())
            })
            .collect();
        let last = states.last().unwrap().0;
        d.jump(states[2].0).unwrap();
        assert_eq!(*d.state(), states[2].1);
        assert!(d.timeline()[3].undone);
        d.jump(last).unwrap();
        assert_eq!(*d.state(), states.last().unwrap().1);
        assert_eq!(d.timeline().len(), 11);
    }

    /// Every version, rebuilt by replaying its edits, is the state it was
    /// when it was made: over random edit sequences, with almost every
    /// snapshot dropped.
    #[test]
    fn replaying_the_log_rebuilds_every_version() {
        for seed in 0..20 {
            let mut rng = fastrand::Rng::with_seed(seed);
            let mut d = Document::new(
                src(6),
                &flat(&[("A", Some(0), 0), ("B", Some(3), 0)]),
                Box::new(FixedCount(2)),
            );
            let other = d.add_source(src(4));
            let mut made: Vec<(VersionId, DocState)> = Vec::new();
            for step in 0..25 {
                let n = d.page_count();
                let page =
                    |rng: &mut fastrand::Rng, d: &Document| d.page(rng.usize(..n)).unwrap().id;
                let edit = match rng.u8(..8) {
                    0 => Edit::RotatePages {
                        ids: vec![page(&mut rng, &d)],
                        by: Rotation::R90,
                    },
                    1 if n > 2 => Edit::DeletePages {
                        ids: vec![page(&mut rng, &d)],
                    },
                    2 => Edit::InsertPages {
                        source: other,
                        range: 0..rng.u32(1..4),
                        before: Some(page(&mut rng, &d)),
                        outline: Some(flat(&[("A", Some(0), 0), ("X", Some(1), 0)])),
                        annots: vec![(0, vec![square(1)]), (1, vec![connector(2, 3)])],
                    },
                    3 => Edit::DuplicatePages {
                        ids: vec![page(&mut rng, &d)],
                    },
                    4 => Edit::AnnotAdd {
                        page: page(&mut rng, &d),
                        annot: square(0),
                    },
                    5 => Edit::OutlineInsert {
                        parent: None,
                        after: None,
                        title: format!("b{step}"),
                        target: Some(page(&mut rng, &d)),
                        view: None,
                    },
                    6 => Edit::MovePages {
                        ids: vec![page(&mut rng, &d)],
                        before: None,
                    },
                    _ => {
                        // Go back a little first, so the tree branches.
                        if rng.bool() {
                            d.undo();
                        }
                        Edit::RotatePages {
                            ids: vec![page(&mut rng, &d)],
                            by: Rotation::R180,
                        }
                    }
                };
                if d.apply(edit, format!("e{step}")).is_ok() {
                    made.push((d.current_version(), d.state().clone()));
                }
            }
            assert!(d.stats().versions < made.len(), "snapshots were dropped");
            for (id, want) in &made {
                d.jump(*id).unwrap();
                assert_eq!(d.state(), want, "seed {seed}, {id:?}");
            }
        }
    }

    #[test]
    fn history_trims_snapshots_and_replays_them() {
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
        // Three snapshots (the opened version's, the current one's and one
        // more); all six versions, with their edits.
        assert_eq!(d.stats().versions, 3);
        assert_eq!(d.version_count(), 6);
        // Undo goes all the way back, replaying what was dropped.
        for i in (0..5).rev() {
            assert_eq!(d.undo(), Some(format!("r{i}").as_str()));
            assert_eq!(d.page(0).unwrap().rotation.degrees(), (i * 90) % 360);
        }
        assert!(!d.can_undo());
        assert!(!d.is_dirty(), "the opened version is the saved one");
        for _ in 0..5 {
            d.redo();
        }
        assert_eq!(d.page(0).unwrap().rotation, Rotation::R90);
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
    fn a_connector_pair_is_a_group_of_two() {
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
            created: String::new(),
            fill: None,
            dashed: false,
            cloudy: false,
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
        // Deleting one end leaves the other: a group of one (decision 8).
        d.apply(Edit::AnnotDelete { page: p2, id: b.id }, "rm")
            .unwrap();
        assert_eq!(d.state().annots_on(p0).unwrap().len(), 1);
        assert!(d.state().annots_on(p2).is_none());
        d.undo();
        assert_eq!(d.state().annots_on(p0).unwrap().len(), 1);
        assert_eq!(d.state().annots_on(p2).unwrap().len(), 1);
    }

    #[test]
    fn a_group_runs_by_page_then_down_the_page() {
        use crate::annot::{AnnotKind, Box2, Group};
        let mut d = doc();
        let (p0, p1) = (d.page(0).unwrap().id, d.page(1).unwrap().id);
        let tick = |glyph: &str, y: f32| {
            mark(
                0,
                AnnotKind::Stamp {
                    rect: Box2 {
                        x: 10.0,
                        y,
                        w: 20.0,
                        h: 20.0,
                    },
                    glyph: glyph.into(),
                },
            )
        };
        for (page, glyph, y) in [
            (p1, "✓", 300.0),
            (p1, "✓", 100.0),
            (p0, "✓", 500.0),
            (p0, "✗", 50.0),
        ] {
            d.apply(
                Edit::AnnotAdd {
                    page,
                    annot: tick(glyph, y),
                },
                "t",
            )
            .unwrap();
        }
        let order: Vec<(usize, f32)> = d
            .state()
            .group_members(&Group::Tickmark("✓".into()))
            .iter()
            .map(|(i, _, a)| (*i, a.bounds().y))
            .collect();
        assert_eq!(order, [(0, 500.0), (1, 100.0), (1, 300.0)]);
    }

    fn mark(id: u64, kind: crate::annot::AnnotKind) -> Annot {
        Annot {
            id: AnnotId(id),
            kind,
            color: [255, 0, 0],
            opacity: 1.0,
            stroke: 1.0,
            author: "t".into(),
            contents: String::new(),
            modified: String::new(),
            created: String::new(),
            fill: None,
            dashed: false,
            cloudy: false,
        }
    }

    fn square(id: u64) -> Annot {
        use crate::annot::{AnnotKind, Box2};
        mark(
            id,
            AnnotKind::Square {
                rect: Box2 {
                    x: 1.0,
                    y: 1.0,
                    w: 5.0,
                    h: 5.0,
                },
            },
        )
    }

    fn connector(id: u64, partner: u64) -> Annot {
        use crate::annot::{AnnotKind, Box2};
        mark(
            id,
            AnnotKind::Connector {
                rect: Box2 {
                    x: 1.0,
                    y: 1.0,
                    w: 22.0,
                    h: 22.0,
                },
                label: "1".into(),
                partner: Some(AnnotId(partner)),
            },
        )
    }

    #[test]
    fn bookmark_styles_arriving_after_the_outline_are_matched_by_title() {
        use crate::file::OutlineStyle;
        let mut d = doc();
        let style = |title: &str, bold| OutlineStyle {
            title: title.into(),
            color: Some([0, 0, 255]),
            style: TextStyle {
                bold,
                italic: false,
            },
        };
        // In pre-order: A, A1, A2, B, C. The third doesn't match its title.
        d.set_initial_outline_styles(&[
            style("A", true),
            style("A1", false),
            style("Not A2", false),
            style("B", false),
        ])
        .unwrap();
        let flat = d.state().outline_flat();
        let by = |t: &str| flat.iter().find(|(_, n)| n.title == t).unwrap().1.clone();
        assert!(by("A").style.bold);
        assert_eq!(by("A1").color, Some([0, 0, 255]));
        assert_eq!(by("A2").color, None);
        assert_eq!(by("B").color, Some([0, 0, 255]));
        assert_eq!(by("C").color, None);
        // Loaded, not edited: nothing to save.
        assert!(!d.is_dirty());
    }

    #[test]
    fn inserted_pages_bring_their_marks_with_fresh_ids() {
        let mut d = doc();
        let mine = d.apply(
            Edit::AnnotAdd {
                page: id_at(&d, 0),
                annot: square(0),
            },
            "add",
        );
        assert!(mine.is_ok());
        let sid = d.add_source(src(3));
        d.apply(
            Edit::InsertPages {
                source: sid,
                range: 0..2,
                before: None,
                outline: None,
                annots: vec![
                    // Page 0: a square and one end of a pair whose other end
                    // is on page 1; page 2 isn't inserted.
                    (0, vec![square(1), connector(2, 3)]),
                    (1, vec![connector(3, 2), connector(4, 9)]),
                    (2, vec![square(5)]),
                ],
            },
            "insert",
        )
        .unwrap();
        let (new0, new1) = (id_at(&d, 6), id_at(&d, 7));
        let on0 = d.state().annots_on(new0).unwrap().clone();
        let on1 = d.state().annots_on(new1).unwrap().clone();
        assert_eq!((on0.len(), on1.len()), (2, 2));
        let all: Vec<AnnotId> = on0.iter().chain(on1.iter()).map(|a| a.id).collect();
        // Fresh ids, none of the file's, all distinct.
        assert!(all.iter().all(|id| id.0 > 9), "{all:?}");
        let unique: HashSet<_> = all.iter().collect();
        assert_eq!(unique.len(), 4);
        // The pair still points at each other; a partner left behind is dropped.
        assert_eq!(on0[1].partner(), Some(on1[0].id));
        assert_eq!(on1[0].partner(), Some(on0[1].id));
        assert_eq!(on1[1].partner(), None);
        // Undo takes them away with the pages.
        d.undo();
        assert!(d.state().annots_on(new0).is_none());
    }

    #[test]
    fn replacing_pages_drops_the_marks_of_pages_removed() {
        let mut d = doc();
        let (p1, p2) = (id_at(&d, 1), id_at(&d, 2));
        for p in [p1, p2] {
            d.apply(
                Edit::AnnotAdd {
                    page: p,
                    annot: square(0),
                },
                "add",
            )
            .unwrap();
        }
        let sid = d.add_source(src(1));
        d.apply(
            Edit::ReplacePages {
                ids: vec![p1, p2],
                source: sid,
                range: 0..1,
                annots: vec![(0, vec![square(7)])],
            },
            "replace",
        )
        .unwrap();
        // p1 keeps its mark and gains the replacement's; p2 is gone, and so
        // are its marks.
        assert_eq!(d.state().annots_on(p1).unwrap().len(), 2);
        assert!(d.state().annots.get(&p2).is_none());
    }

    #[test]
    fn mark_ids_are_random_and_distinct() {
        let mut d = doc();
        let p = id_at(&d, 0);
        for _ in 0..50 {
            d.apply(
                Edit::AnnotAdd {
                    page: p,
                    annot: square(0),
                },
                "add",
            )
            .unwrap();
        }
        let ids: Vec<u64> = d
            .state()
            .annots_on(p)
            .unwrap()
            .iter()
            .map(|a| a.id.0)
            .collect();
        let unique: HashSet<_> = ids.iter().collect();
        assert_eq!(unique.len(), 50);
        assert!(ids.iter().all(|&id| id > 0 && id < 1 << 53));
        // Not handed out in order, as two copies of a file would.
        assert_ne!(ids, (ids[0]..ids[0] + 50).collect::<Vec<_>>());
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
            created: String::new(),
            fill: None,
            dashed: false,
            cloudy: false,
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
                annots: Vec::new(),
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
            created: String::new(),
            fill: None,
            dashed: false,
            cloudy: false,
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
                annots: Vec::new(),
            },
            Edit::OutlineEdit {
                id: OutlineId(3),
                title: Some("T".into()),
                target: Some(None),
                color: Some(Some([1, 2, 3])),
                view: None,
                style: None,
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
                style: None,
            }
        );
    }
}
