//! Virtual document, edits, and snapshot history.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use im::Vector;
use serde::{Deserialize, Serialize};
use sheaf_core::{DestView, PageSize, Rotation};

use crate::annot::{Annot, AnnotId, AnnotKind};
use crate::assets::{MarkAssets, SourceAssets};
use crate::forms::{Field, FieldId, FileForm, Widget};
use crate::geom::Geom;
use crate::history::{HistoryPolicy, HistoryStats};
use crate::open::{Facts, Renamed, ViewFacts};
use crate::prim::{HeapSize, Key, Prim, Rec, Val, same};
use crate::store::{BlobKind, Src, Store};
use crate::text_layer::TextLayer;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SourceId(pub u32);

/// Stable identity of a page in the virtual document. Never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PageId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct OutlineId(pub u64);

/// A file registered for the document's pages: its source, and the name
/// it goes by. Every insert registers anew, so inserting the same file
/// twice gives two registrations of one [`Src`], and two forms (until
/// M10.5 slice 11 makes the source its blob). The bytes, page table and
/// password are the source's.
#[derive(Debug, Clone)]
pub struct SourceInfo {
    pub src: Src,
    /// The path the bytes came from, or a derived copy's original's name.
    /// Notices, rename stems (`name_apart`), `form_notice`, the recovery
    /// sidecar and save's same-file check read it; it's never read for
    /// bytes: after a save in place the path holds the new file, and a
    /// version from before the save still means the old one.
    pub name: PathBuf,
    /// For a copy sheaf made of another source's pages (cropped,
    /// watermarked, a header/footer stripped): which pages. A save writes
    /// each as the page it was made from, with its annotations and form
    /// fields, and the copy's content (M10 bug 1).
    pub derived: Option<Derived>,
}

/// Where a derived source's pages come from: its page `k` is a copy of page
/// `pages[k]` of `from`.
#[derive(Debug, Clone, PartialEq)]
pub struct Derived {
    pub from: SourceId,
    pub pages: Vec<u32>,
}

impl SourceInfo {
    /// The file at `path`, read once, opened with `password` and parsed
    /// into `store`, under its own path. A wrong or missing password is
    /// [`OpenError::Password`](crate::source::OpenError::Password) in the
    /// chain.
    pub fn read(store: &Store, path: &Path, password: Option<&str>) -> Result<SourceInfo> {
        let bytes =
            sheaf_core::read_file(path).with_context(|| format!("reading {}", path.display()))?;
        let src = store
            .open(bytes, BlobKind::file(path, password))
            .with_context(|| format!("opening {}", path.display()))?;
        Ok(SourceInfo {
            src,
            name: path.to_path_buf(),
            derived: None,
        })
    }

    /// For tests: `pages` blank Letter pages, named `a.pdf`, parsed into
    /// `store`.
    #[cfg(any(test, feature = "fixtures"))]
    pub fn blank(store: &Store, pages: u32) -> SourceInfo {
        let bytes = crate::generated::blank_pages(612.0, 792.0, pages as usize)
            .expect("blank pages are written");
        let kind = BlobKind::file(Path::new("a.pdf"), None);
        SourceInfo {
            src: store.open(bytes.into(), kind).expect("blank pages open"),
            name: "a.pdf".into(),
            derived: None,
        }
    }
}

/// One page of the virtual document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRef {
    pub id: PageId,
    pub source: SourceId,
    pub index: u32,
    /// The page's `/Rotate`, absolute, as a save writes it: its source
    /// page's own until it's rotated.
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
    /// Whether its children show, as the file has it: a save writes a
    /// closed one's `/Count` negative.
    pub open: bool,
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
    /// `/C` and `/F`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[u8; 3]>,
    #[serde(default, skip_serializing_if = "TextStyle::is_plain")]
    pub style: TextStyle,
    /// Its `/Count` isn't negative (see [`OutlineNode::open`]).
    #[serde(default = "yes", skip_serializing_if = "is_yes")]
    pub open: bool,
}

fn yes() -> bool {
    true
}

fn is_yes(b: &bool) -> bool {
    *b
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

/// One immutable snapshot of the document. Only tests compare whole
/// states; history knows what an edit changed from its prims.
#[derive(Debug, Clone)]
#[cfg_attr(any(test, feature = "fixtures"), derive(PartialEq))]
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
    /// The form's terminal fields (M10 proposal 8): the primary file's,
    /// read at open. Inserted files' come in with M10 slice 4.
    pub fields: im::OrdMap<FieldId, Field>,
    /// Their widgets, per page, each page's in `/Annots` order: they move
    /// and go with their page.
    pub widgets: im::HashMap<PageId, Vector<Widget>>,
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

    pub fn widgets_on(&self, page: PageId) -> Option<&Vector<Widget>> {
        self.widgets.get(&page)
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
    /// keeping their ids, rotation, bookmarks, marks and OCR text, which
    /// are in PageSpace, which a copy keeps: nothing moves.
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
    /// Forms that came in with pages and were put under a new parent
    /// field, since their names clashed (M10 proposal 5): its name, and the
    /// file they came from.
    pub forms_renamed: Vec<(String, SourceId)>,
    /// Files whose signed fields came in with their pages: the signatures
    /// don't cover this file (M10 proposal 7).
    pub signatures_in: Vec<SourceId>,
    /// Text it added or changed (a mark's, a header's or footer's) has
    /// characters the built-in fonts can't draw: they show as `?` in the
    /// file (see [`Document::unencodable_notice`]).
    pub unencodable: bool,
}

/// A version in the history tree. It names one state, which never changes:
/// a job's amend or a save puts a new version in its place. Ids aren't
/// reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VersionId(pub u64);

/// One edit as applied, and what it changed: prims with the values they
/// replaced. A replay applies them and checks each before-value, so it
/// rebuilds the version exactly, ids included, and never runs the edit
/// again.
#[derive(Debug, Clone)]
pub struct Step {
    pub edit: Edit,
    /// What it did, in words (see [`crate::describe`]), against the
    /// document just before it.
    pub line: String,
    /// What it changed. Coalesced over its version's steps (see
    /// [`coalesce`]), so a slot's change is on the last step that made
    /// one, and an edit whose change a later one undid has none.
    pub(crate) prims: Arc<[Prim]>,
}

/// A version's prims don't fit the state they were made from: an internal
/// bug, never the reader's. The version can't be rebuilt.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("History is damaged at version {}", .version.0)]
pub struct Damaged {
    pub version: VersionId,
}

/// An edit came while the file is still opening: its facts aren't read
/// yet, so there's nothing to edit (M10.5 proposal 28). Slice 11 makes it
/// `EditError::NotReady`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the file is still opening")]
pub struct NotReady;

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
    /// The registrations it holds: the files registered since the parent
    /// (inserted ones, and the file a save wrote), which its steps and
    /// state refer to; the first version holds those of the versions
    /// pruned before it too. They go when it does (see `remove_version`).
    sources: Vec<SourceId>,
    /// The state, unless trimming dropped it; then a jump rebuilds it by
    /// applying the steps' prims from the nearest ancestor that kept one.
    state: Option<DocState>,
    /// Rebuilding it failed (see [`Damaged`]), so it can't be reached.
    damaged: bool,
    /// What its snapshot holds that no other version's does, which
    /// trimming frees, and what its steps hold, which only pruning the
    /// version does (see [`accounting`]).
    snapshot_bytes: usize,
    step_bytes: usize,
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
    /// Its replay failed, so it can't be reached (see [`Damaged`]).
    pub damaged: bool,
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
    /// Fields that differ from what's in the primary file (see
    /// `Document::fields_dirty`).
    pub fields_dirty: HashSet<FieldId>,
    /// Something in the primary file is gone from the document (see
    /// `Document::removes_content`): only a full rewrite takes it out of
    /// the file.
    pub removes_content: bool,
    /// The form differs from the primary file's (see
    /// `Document::form_changed`): a save takes out what described it as
    /// it was, its XFA copy and usage rights (M10 proposals 6 and 7).
    pub form_changed: bool,
    /// Each source's marks the open gave new ids (see
    /// `Document::set_source_renamed`).
    pub(crate) renamed: HashMap<SourceId, Arc<Renames>>,
    /// The stamps and attachments the marks name (see
    /// [`Document::assets`]).
    pub assets: Arc<MarkAssets>,
}

/// A source's marks the open gave new ids ([`Renamed`]): by page index,
/// the id each had, which the page's `/NM` says, and the one it has.
pub(crate) type Renames = HashMap<u32, Vec<(AnnotId, AnnotId)>>;

impl Snapshot {
    pub fn source(&self, id: SourceId) -> Option<&Arc<SourceInfo>> {
        self.sources.get(&id)
    }

    /// [`Document::geom`].
    pub fn geom(&self, page: &PageRef) -> Geom {
        geom_in(&self.sources, page)
    }

    /// [`Document::render_delta`].
    pub fn render_delta(&self, page: &PageRef) -> Rotation {
        render_delta_in(&self.sources, page)
    }

    /// [`Document::group_members`].
    pub fn group_members(&self, group: &crate::annot::Group) -> Vec<(usize, PageId, Annot)> {
        group_members_in(&self.state, &self.sources, group)
    }

    /// The marks on page `index` of `source` the open gave new ids: the id
    /// each had, which the page's `/NM` says, and the one it has.
    pub(crate) fn renamed_on(&self, source: SourceId, index: u32) -> &[(AnnotId, AnnotId)] {
        self.renamed
            .get(&source)
            .and_then(|r| r.get(&index))
            .map_or(&[], Vec::as_slice)
    }
}

/// Every mark of `state` in `group`, in document order: by page, then
/// down the page as it shows, then across.
fn group_members_in(
    state: &DocState,
    sources: &HashMap<SourceId, Arc<SourceInfo>>,
    group: &crate::annot::Group,
) -> Vec<(usize, PageId, Annot)> {
    let in_group = |a: &&Annot| crate::annot::group_of(&a.kind).as_ref() == Some(group);
    let mut out = Vec::new();
    for (i, p) in state.pages.iter().enumerate() {
        let on_page: Vec<&Annot> = state
            .annots_on(p.id)
            .into_iter()
            .flatten()
            .filter(in_group)
            .collect();
        if on_page.is_empty() {
            continue;
        }
        let to_view = geom_in(sources, p).to_view();
        let mut shown: Vec<(crate::annot::Box2, &Annot)> = on_page
            .into_iter()
            .map(|a| (to_view.apply_box(a.page_bounds()), a))
            .collect();
        shown.sort_by(|(a, _), (b, _)| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)));
        out.extend(shown.into_iter().map(|(_, a)| (i, p.id, a.clone())));
    }
    out
}

/// Page `page`'s source page's boxes and `/Rotate`: a Letter page's when
/// its source isn't in `sources`.
fn source_geom(sources: &HashMap<SourceId, Arc<SourceInfo>>, page: &PageRef) -> Geom {
    sources
        .get(&page.source)
        .map_or_else(|| Geom::read(None, None, 0), |s| s.src.geom(page.index))
}

fn geom_in(sources: &HashMap<SourceId, Arc<SourceInfo>>, page: &PageRef) -> Geom {
    Geom {
        rotate: page.rotation,
        ..source_geom(sources, page)
    }
}

fn render_delta_in(sources: &HashMap<SourceId, Arc<SourceInfo>>, page: &PageRef) -> Rotation {
    page.rotation.minus(source_geom(sources, page).rotate)
}

/// Where page, bookmark and field ids come from. The counters only go up,
/// so an id is never handed out twice, whatever undo does. Edits draw from
/// it; a replay applies their prims, which hold the ids, and draws nothing.
#[derive(Default)]
struct IdAlloc {
    page: AtomicU64,
    outline: AtomicU64,
    field: AtomicU64,
}

impl IdAlloc {
    fn page(&self) -> PageId {
        PageId(self.page.fetch_add(1, Ordering::Relaxed))
    }

    fn outline(&self) -> OutlineId {
        OutlineId(self.outline.fetch_add(1, Ordering::Relaxed))
    }

    fn field(&self) -> FieldId {
        FieldId(self.field.fetch_add(1, Ordering::Relaxed))
    }

    /// Field ids from `next` on: a file's fields keep the ids it read them
    /// with.
    fn fields_from(&self, next: u64) {
        self.field.fetch_max(next, Ordering::Relaxed);
    }
}

pub struct Document {
    /// The blobs its sources are in, outside undo.
    store: Arc<Store>,
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
    ids: IdAlloc,
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
    /// Fields as they are in the primary file on disk: their values for
    /// dirty tracking.
    saved_fields: im::OrdMap<FieldId, Field>,
    /// Each source's form as read (`set_source_form`), for the fields that
    /// come in with its pages.
    forms: HashMap<SourceId, Arc<FileForm>>,
    /// Each source's marks the open gave new ids (`set_source_renamed`),
    /// which its pages name by the old ones until a save writes them.
    renamed: HashMap<SourceId, Arc<Renames>>,
    /// The stamps and attachments the marks name, outside history.
    assets: Arc<MarkAssets>,
    /// A save made one form of forms whose scripts came from more than one
    /// file (see [`Document::scripts_mixed`]).
    scripts_mixed: bool,
    /// The reader has been told some text shows as `?` in the file
    /// ([`Document::unencodable_notice`]). Outside history: undo doesn't
    /// make it news again.
    told_unencodable: bool,
    /// The opened version holds the file's facts: until then edits are
    /// refused (see [`NotReady`]).
    ready: bool,
}

impl Document {
    /// A document showing `primary`, a source in `store`, in its
    /// entirety, with `outline` and nothing else: a file sheaf has just
    /// made, which says nothing more about itself. A file that's opened
    /// opens through [`crate::open`], which reads what it says.
    pub fn new(
        store: Arc<Store>,
        primary: SourceInfo,
        outline: &[FlatOutlineItem],
        policy: Box<dyn HistoryPolicy>,
    ) -> Self {
        let mut doc = Self::loading(store, primary, policy);
        let facts = Facts {
            outline: outline.to_vec(),
            ..Default::default()
        };
        let (state, _) = doc.opened(facts);
        let root = doc.root;
        doc.versions
            .get_mut(&root)
            .expect("the opened version")
            .state = Some(state);
        doc.ready = true;
        doc
    }

    /// A document showing `primary` while it opens: its pages, from the
    /// page table its source read as it opened, and nothing else, until
    /// [`ready`](Self::ready). See [`crate::open::start`].
    pub(crate) fn loading(
        store: Arc<Store>,
        primary: SourceInfo,
        policy: Box<dyn HistoryPolicy>,
    ) -> Self {
        let source = SourceId(0);
        let mut doc = Self {
            assets: Arc::new(MarkAssets::new(store.clone())),
            store,
            sources: HashMap::new(),
            primary: source,
            versions: HashMap::new(),
            root: VersionId(0),
            cursor: VersionId(0),
            saved: Some(VersionId(0)),
            next_version: 1,
            unlogged_sources: Vec::new(),
            author: crate::annot::default_author(),
            ids: IdAlloc::default(),
            next_source: 1,
            policy,
            next_job: 1,
            saved_outline: Vector::new(),
            saved_labels: None,
            saved_removed: im::HashMap::new(),
            baked_header_footer: None,
            saved_annots: im::HashMap::new(),
            saved_fields: im::OrdMap::new(),
            forms: HashMap::new(),
            renamed: HashMap::new(),
            scripts_mixed: false,
            told_unencodable: false,
            ready: false,
        };
        doc.debug_assert_ours(&primary);
        let pages: Vector<PageRef> = (0..primary.src.page_count())
            .map(|index| PageRef {
                id: doc.ids.page(),
                source,
                index,
                rotation: primary.src.geom(index).rotate,
            })
            .collect();
        doc.sources.insert(source, Arc::new(primary));
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
                    outline: Vector::new(),
                    annots: im::HashMap::new(),
                    text_layers: im::HashMap::new(),
                    removed_comments: im::HashMap::new(),
                    labels: None,
                    header_footer: None,
                    fields: im::OrdMap::new(),
                    widgets: im::HashMap::new(),
                }),
                damaged: false,
                snapshot_bytes: 0,
                step_bytes: 0,
                job: None,
                saves: Vec::new(),
            },
        );
        doc
    }

    /// The file has opened: `facts`, as [`crate::open::finish`] read them,
    /// make the opened version complete. It's a new version in place of
    /// the one with the pages alone, which no longer resolves; the current
    /// and saved versions are this one, so the document isn't dirty and
    /// there's nothing to undo. Gives what the view shows beside the
    /// document. Only once, while the file opens: there are no late facts
    /// (M10.5 proposal 28).
    pub fn ready(&mut self, facts: Facts) -> Result<ViewFacts> {
        if self.ready {
            bail!("the file has opened already");
        }
        let (state, view) = self.opened(facts);
        let old = self.root;
        let mut v = self.versions.remove(&old).expect("the opened version");
        v.state = Some(state);
        self.replace(old, v);
        self.ready = true;
        Ok(view)
    }

    /// Whether the file's facts are in: until then edits are refused.
    pub fn is_ready(&self) -> bool {
        self.ready
    }

    /// The opened version's state: its pages, and what `facts` say about
    /// them, each page index mapped to its page once. That's what the
    /// primary file holds, so the `saved_*` baselines, the header/footer
    /// drawn into the file and the primary's form are set from it. Gives
    /// back what the state doesn't hold.
    fn opened(&mut self, facts: Facts) -> (DocState, ViewFacts) {
        let pages = self.state().pages.clone();
        let by_index: Vec<PageId> = pages.iter().map(|p| p.id).collect();
        let at = |index: usize| by_index.get(index).copied();
        let outline = self.outline_from_flat(&facts.outline, &by_index);
        let annots: im::HashMap<PageId, Vector<Annot>> = facts
            .marks
            .into_iter()
            .filter_map(|(index, list)| Some((at(index as usize)?, list.into_iter().collect())))
            .collect();
        let labels = facts
            .labels
            .map(|ranges| -> Vector<_> {
                crate::labels::to_specs(&ranges, &by_index)
                    .into_iter()
                    .collect()
            })
            .filter(|specs| !specs.is_empty());
        let header_footer = facts.header_footer.map(|(mut hf, scope)| {
            hf.pages = scope.map(|ix| ix.into_iter().filter_map(at).collect());
            Arc::new(hf)
        });
        let form = facts.form;
        let fields: im::OrdMap<FieldId, Field> = form
            .fields
            .iter()
            .map(|f| {
                let mut f = f.clone();
                f.source = self.primary;
                (f.id, f)
            })
            .collect();
        let widgets = form
            .widgets
            .iter()
            .filter_map(|(&index, list)| {
                Some((at(index as usize)?, list.iter().cloned().collect()))
            })
            .collect();
        self.ids
            .fields_from(fields.keys().next_back().map_or(0, |id| id.0 + 1));
        let renamed: Vec<(PageId, AnnotId)> = facts
            .renamed
            .iter()
            .filter_map(|r| Some((at(r.page as usize)?, r.was)))
            .collect();
        self.set_source_renamed(self.primary, &facts.renamed);
        self.set_source_assets(self.primary, &facts.assets);
        self.saved_outline = outline.clone();
        self.saved_labels = labels.clone();
        self.baked_header_footer = header_footer.clone();
        self.saved_annots = annots.clone();
        // The file says other ids for the marks the open gave new ones:
        // their pages are written at the next save, of either kind.
        for (page, _) in &renamed {
            self.saved_annots.remove(page);
        }
        self.saved_fields = fields.clone();
        let view = ViewFacts {
            renamed,
            foreign: facts.foreign,
            attachments: facts.attachments,
            collection: facts.collection,
            layers: facts.layers,
            form: form.facts.clone(),
        };
        if !form.fields.is_empty() {
            self.forms.insert(self.primary, Arc::new(form));
        }
        let state = DocState {
            pages,
            outline,
            annots,
            text_layers: im::HashMap::new(),
            removed_comments: im::HashMap::new(),
            labels,
            header_footer,
            fields,
            widgets,
        };
        (state, view)
    }

    /// Who the versions made from now on are by.
    pub fn set_author(&mut self, name: impl Into<String>) {
        self.author = name.into();
    }

    pub fn primary(&self) -> SourceId {
        self.primary
    }

    /// Where its sources' bytes are: what a registration's source is
    /// interned into.
    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    pub fn source(&self, id: SourceId) -> Option<&Arc<SourceInfo>> {
        self.sources.get(&id)
    }

    pub fn sources(&self) -> impl Iterator<Item = (SourceId, &Arc<SourceInfo>)> {
        self.sources.iter().map(|(k, v)| (*k, v))
    }

    /// Register another file so its pages can be inserted. The next
    /// version records it, with the edit that uses it, and the
    /// registration goes when that version is pruned: an edit on another
    /// branch registers the file again.
    pub fn add_source(&mut self, info: SourceInfo) -> SourceId {
        let id = self.register_source(info);
        self.unlogged_sources.push(id);
        id
    }

    fn register_source(&mut self, info: SourceInfo) -> SourceId {
        self.debug_assert_ours(&info);
        let id = SourceId(self.next_source);
        self.next_source += 1;
        self.sources.insert(id, Arc::new(info));
        id
    }

    /// Forget registrations whose version is gone, and their forms and
    /// renamed marks: a
    /// source goes with the last registration holding it, and an engine
    /// opened on it keeps only its bytes, until slice 13's engine pool. The
    /// primary stays while it is the primary.
    fn unregister(&mut self, ids: &[SourceId]) {
        for id in ids.iter().filter(|&&id| id != self.primary) {
            self.sources.remove(id);
            self.forms.remove(id);
            self.renamed.remove(id);
        }
    }

    /// A registration's source is a blob of this document's store: the
    /// store is what counts its bytes.
    fn debug_assert_ours(&self, info: &SourceInfo) {
        debug_assert!(
            self.store.holds(info.src.blob()),
            "{} is in another store",
            info.name.display()
        );
    }

    /// Source `id`'s form, as `forms::read` read it: the fields that come
    /// in with its pages when they're inserted or replace others. Call it
    /// before the edit that brings them.
    pub fn set_source_form(&mut self, id: SourceId, form: FileForm) {
        if !form.fields.is_empty() {
            self.forms.insert(id, Arc::new(form));
        }
    }

    /// Source `id`'s marks the open gave new ids, as `open::finish` read
    /// them: its pages name them by their old ids until a save writes
    /// them, and a save follows other tools' replies to them through these
    /// (see `save::write_page_annots`). Call it before the edit that brings
    /// its pages.
    pub fn set_source_renamed(&mut self, id: SourceId, renamed: &[Renamed]) {
        let mut by_page = Renames::new();
        for r in renamed {
            by_page.entry(r.page).or_default().push((r.was, r.id));
        }
        if !by_page.is_empty() {
            self.renamed.insert(id, Arc::new(by_page));
        }
    }

    /// What source `id`'s marks hold, as `open::finish` read it: their
    /// stamps' pages and attachments' files, which marks naming them draw
    /// and embed from now on where the document didn't know them. Call it
    /// before the edit that brings its pages.
    pub fn set_source_assets(&self, id: SourceId, assets: &SourceAssets) {
        if let Some(info) = self.sources.get(&id) {
            self.assets.register(&info.src, assets);
        }
    }

    /// What source `id`, the file a save wrote, holds for the marks: what
    /// they draw and embed from now on, in place of what the save took
    /// them from.
    pub fn set_saved_assets(&self, id: SourceId, assets: &SourceAssets) {
        if let Some(info) = self.sources.get(&id) {
            self.assets.register_saved(&info.src, assets);
        }
    }

    /// The stamps and attachments the marks name: those the document's
    /// files hold, those placed or pasted since, and, failing those, the
    /// generated cache's. Outside history: a stamp placed and undone is
    /// still known, and every version's marks find theirs here.
    pub fn assets(&self) -> &Arc<MarkAssets> {
        &self.assets
    }

    /// Sources whose fields in the document, or the document scripts
    /// beside them, have scripts. More than one, and scripts are off by
    /// default (M10 proposal 5): document scripts share one scope.
    pub fn script_sources(&self) -> Vec<SourceId> {
        let st = self.state();
        let mut widgets: HashMap<FieldId, Vec<&Widget>> = HashMap::new();
        for w in st.widgets.values().flatten() {
            widgets.entry(w.field).or_default().push(w);
        }
        let mut out: Vec<SourceId> = Vec::new();
        for f in st.fields.values() {
            if out.contains(&f.source) {
                continue;
            }
            let documents = self
                .forms
                .get(&f.source)
                .is_some_and(|form| form.facts.document_scripts > 0);
            let own = widgets.get(&f.id).map_or(&[][..], |w| &w[..]);
            if documents || f.has_scripts(own) {
                out.push(f.source);
            }
        }
        out.sort();
        out
    }

    /// Whether the document's scripts came from more than one file, now
    /// or before a save this session: then they're off by default (M10
    /// proposal 5).
    pub fn scripts_mixed(&self) -> bool {
        self.scripts_mixed || self.script_sources().len() > 1
    }

    /// What to tell the reader about the forms an edit brought: fields put
    /// under a new parent (a copy's, after Duplicate Pages, when `copies`),
    /// whether their scripts still name the old fields, and scripts turned
    /// off when `mixed_before` (`scripts_mixed` before the edit) wasn't.
    pub fn form_notice(
        &self,
        out: &EditOutcome,
        copies: bool,
        mixed_before: bool,
    ) -> Option<String> {
        let st = self.state();
        let mut parts = Vec::new();
        for (under, sid) in &out.forms_renamed {
            let file = self
                .sources
                .get(sid)
                .and_then(|s| s.name.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let scripted = st.fields.values().any(|f| {
                f.under.as_deref() == Some(under.as_str())
                    && (f.has_scripts(&[])
                        || self
                            .forms
                            .get(sid)
                            .is_some_and(|form| form.facts.document_scripts > 0))
            });
            let mut part = if copies {
                format!("the copies' fields are under “{under}”, apart from the originals")
            } else {
                format!("fields from {file} are under “{under}”, since their names were taken")
            };
            if scripted {
                part.push_str("; its scripts still use the old names");
            }
            parts.push(part);
        }
        for sid in &out.signatures_in {
            let file = self
                .sources
                .get(sid)
                .and_then(|s| s.name.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            parts.push(format!("the signature from {file} is no longer valid here"));
        }
        if !mixed_before && self.scripts_mixed() {
            parts.push("scripts are off: fields from more than one form have them".into());
        }
        if parts.is_empty() {
            return None;
        }
        let sentences: Vec<String> = parts
            .iter()
            .map(|p| {
                let mut chars = p.chars();
                chars
                    .next()
                    .map(|c| c.to_uppercase().chain(chars).collect())
                    .unwrap_or_default()
            })
            .collect();
        Some(sentences.join(". "))
    }

    /// What to tell the reader when an edit brought text the built-in
    /// fonts can't draw ([`EditOutcome::unencodable`]): once a document,
    /// the first time, and never again after.
    pub fn unencodable_notice(&mut self, out: &EditOutcome) -> Option<String> {
        if !out.unencodable || self.told_unencodable {
            return None;
        }
        self.told_unencodable = true;
        Some(
            "Some characters can't be drawn in the built-in fonts and show as “?” in the file; \
             the text itself is kept"
                .into(),
        )
    }

    /// The header/footer differs from what the primary file has drawn.
    pub fn header_footer_dirty(&self) -> bool {
        self.state().header_footer != self.baked_header_footer
    }

    /// Fields that differ from what's in the primary file: changed, added
    /// or gone.
    pub fn fields_dirty(&self) -> HashSet<FieldId> {
        let cur = &self.state().fields;
        if cur.ptr_eq(&self.saved_fields) {
            return HashSet::new();
        }
        let mut dirty: HashSet<FieldId> = cur
            .iter()
            .filter(|(id, f)| self.saved_fields.get(id) != Some(f))
            .map(|(id, _)| *id)
            .collect();
        dirty.extend(self.saved_fields.keys().filter(|id| !cur.contains_key(id)));
        dirty
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
            fields_dirty: self.fields_dirty(),
            removes_content: self.removes_content(),
            form_changed: self.form_changed(),
            renamed: self.renamed.clone(),
            assets: self.assets.clone(),
        }
    }

    /// Whether the form differs from the primary file's: a field added,
    /// removed or changed, or widgets of a page not as they were (it's
    /// gone, or shows another file's page now). Then a save takes out what
    /// described the form as it was: its XFA copy and Reader's usage rights
    /// (M10 proposals 6 and 7).
    pub fn form_changed(&self) -> bool {
        if !self.fields_dirty().is_empty() {
            return true;
        }
        let Some(saved) = self
            .saved
            .and_then(|v| self.versions.get(&v))
            .and_then(|v| v.state.as_ref())
        else {
            return false;
        };
        let now = &self.state().widgets;
        !saved.widgets.ptr_eq(now)
            && saved
                .widgets
                .iter()
                .any(|(page, list)| now.get(page) != Some(list))
    }

    /// Whether something in the primary file is gone from the document: a
    /// page (deleted, or shown from another source now, as a crop makes
    /// it), a mark or reply, another tool's comment, a bookmark. An
    /// incremental save would keep it in the file's earlier revision (M9
    /// bug 14). Undoing back to what was saved clears it.
    pub fn removes_content(&self) -> bool {
        let st = self.state();
        if let Some(primary) = self.sources.get(&self.primary) {
            let shown: HashSet<u32> = st
                .pages
                .iter()
                .filter(|p| p.source == self.primary)
                .map(|p| p.index)
                .collect();
            if (0..primary.src.page_count()).any(|i| !shown.contains(&i)) {
                return true;
            }
        }
        let marks: HashSet<AnnotId> = st.annots.values().flatten().map(|a| a.id).collect();
        if self
            .saved_annots
            .values()
            .flatten()
            .any(|a| !marks.contains(&a.id))
        {
            return true;
        }
        let deleted_since = st.removed_comments.iter().any(|(pid, names)| {
            let before = self.saved_removed.get(pid);
            names.iter().any(|n| !before.is_some_and(|b| b.contains(n)))
        });
        if deleted_since {
            return true;
        }
        fn ids(nodes: &Vector<OutlineNode>, out: &mut HashSet<OutlineId>) {
            for n in nodes {
                out.insert(n.id);
                ids(&n.children, out);
            }
        }
        let (mut now, mut before) = (HashSet::new(), HashSet::new());
        ids(&st.outline, &mut now);
        ids(&self.saved_outline, &mut before);
        before.iter().any(|id| !now.contains(id))
    }

    /// After a successful save of the current version to a file described by
    /// `saved`: register it as a new source and make the current version an
    /// identity mapping over it, keeping `PageId`s (and bookmarks) intact so
    /// the view doesn't flicker. Older versions still reference the old
    /// sources, so undo across a save keeps working. OCR layers are now
    /// baked into the file, so they reset to nothing; a page's rotation is
    /// the saved file's `/Rotate`, so it stays.
    ///
    /// The content is the same, so this isn't an edit and makes no step,
    /// but the state is expressed anew, so the version gets a new id under
    /// the same parent (see [`replace`](Self::replace)). Its steps still
    /// lead to its state as it was before the save; it keeps its snapshot,
    /// as do its children (see `keeps_snapshot`), so nothing is ever
    /// replayed across a save. The new version holds the saved file's
    /// registration. M10.5 slice 9 deletes this.
    pub fn rebase_to_saved(&mut self, saved: SourceInfo) -> SourceId {
        debug_assert!(self.is_ready(), "a save of a file still opening");
        // The forms it was made of are one now; whether their scripts
        // were from more than one is the session's to remember.
        self.scripts_mixed |= self.script_sources().len() > 1;
        let path = saved.name.clone();
        let id = self.register_source(saved);
        let replaced = std::mem::replace(&mut self.primary, id);
        // Versions made from it before (and undone) were made from it as
        // it was, so their prims don't fit it re-expressed: they keep their
        // states (see `keeps_snapshot`). A damaged one stays out of reach.
        for child in self.versions[&self.cursor].children.clone() {
            let _ = self.ensure_state(child);
        }
        let old = self.cursor;
        let mut v = self.versions.remove(&old).expect("current version");
        v.sources.push(id);
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
                rotation: p.rotation,
            })
            .collect();
        // Every field is the saved file's own now, under the name it was
        // saved with. (Their objects there are slice 9's to find.)
        st.fields = st
            .fields
            .iter()
            .map(|(fid, f)| {
                let f = Field {
                    source: id,
                    copy: 0,
                    under: None,
                    ..f.clone()
                };
                (*fid, f)
            })
            .collect();
        // The pages and fields expressed anew are its snapshot's alone.
        v.snapshot_bytes = accounting(&v.steps, st).0
            + st.pages.heap_size()
            + st.fields
                .values()
                .map(|f| size_of::<(FieldId, Field)>() + f.heap_size())
                .sum::<usize>();
        let new = self.replace(old, v);
        // The primary it replaces stayed only as the primary when pruning
        // took the version that held it: nothing refers to it now.
        let held = self
            .versions
            .values()
            .any(|v| v.sources.contains(&replaced));
        if !held {
            self.unregister(&[replaced]);
        }
        let st = self.state().clone();
        self.saved_outline = st.outline;
        self.saved_annots = st.annots;
        self.saved_fields = st.fields;
        self.saved_removed = st.removed_comments;
        self.saved_labels = st.labels;
        self.baked_header_footer = st.header_footer;
        self.saved = Some(new);
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

    /// A page's boxes, its source page's, with its rotation.
    pub fn geom(&self, page: &PageRef) -> Geom {
        geom_in(&self.sources, page)
    }

    /// The turn PDFium adds to the source page's own `/Rotate`: what a
    /// render `Job` takes.
    pub fn render_delta(&self, page: &PageRef) -> Rotation {
        render_delta_in(&self.sources, page)
    }

    /// A page's size as shown.
    pub fn page_size(&self, page: &PageRef) -> PageSize {
        self.geom(page).shown()
    }

    /// Every mark in `group` (see `annot::group_of`), in document order:
    /// by page, then down the page as it shows, then across. With each,
    /// its page's position.
    pub fn group_members(&self, group: &crate::annot::Group) -> Vec<(usize, PageId, Annot)> {
        group_members_in(self.state(), &self.sources, group)
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

    /// Back to the parent version: the label of the edit undone, or None
    /// with nothing to undo.
    pub fn undo(&mut self) -> Result<Option<&str>, Damaged> {
        let from = self.cursor;
        let Some(parent) = self.versions[&from].parent else {
            return Ok(None);
        };
        self.ensure_state(parent)?;
        self.versions.get_mut(&parent).expect("parent").redo_to = Some(from);
        self.cursor = parent;
        Ok(Some(self.versions[&from].label.as_str()))
    }

    /// On to the version redo goes to: its label, or None with nothing to
    /// redo.
    pub fn redo(&mut self) -> Result<Option<&str>, Damaged> {
        let Some(to) = self.redo_target() else {
            return Ok(None);
        };
        self.ensure_state(to)?;
        self.cursor = to;
        Ok(Some(self.versions[&to].label.as_str()))
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

    /// The edits that made version `id` from its parent, and the
    /// registrations it holds (the files that came in with them): the edge
    /// into it, which is what a merge would replay
    /// (`todos/file-contention.md`).
    pub fn edge(&self, id: VersionId) -> Option<(&[Step], &[SourceId])> {
        let v = self.versions.get(&id)?;
        Some((&v.steps, &v.sources))
    }

    /// Run `edit` as a new version. Refused with [`NotReady`] while the
    /// file opens, as are [`apply_batch`](Self::apply_batch) and
    /// [`apply_in_job`](Self::apply_in_job).
    pub fn apply(&mut self, edit: Edit, label: impl Into<String>) -> Result<EditOutcome> {
        self.check_ready()?;
        let (state, outcome, step) = self.step(self.state().clone(), edit)?;
        if step.prims.is_empty() {
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
        self.check_ready()?;
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
            total.forms_renamed.extend(out.forms_renamed);
            total.signatures_in.extend(out.signatures_in);
            total.unencodable |= out.unencodable;
        }
        coalesce(&mut steps);
        if steps.iter().any(|s| !s.prims.is_empty()) {
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
    /// by `job`, has nothing after it, was never saved, and isn't the first
    /// (pruning can make it so), the edit joins it instead of making a new
    /// version. The first batch of a job makes a version; later batches
    /// amend it, so the whole job is one undo step. An amend replaces the
    /// version with a new id under the same parent, and when the job has
    /// put back all it did, it leaves no version at all. A save, an undo or
    /// an unrelated edit in between ends the chain, and the rest of the job
    /// becomes another step.
    pub fn apply_in_job(
        &mut self,
        job: JobId,
        edit: Edit,
        label: impl Into<String>,
    ) -> Result<EditOutcome> {
        self.check_ready()?;
        let (state, outcome, step) = self.step(self.state().clone(), edit)?;
        if step.prims.is_empty() {
            return Ok(outcome);
        }
        let current = &self.versions[&self.cursor];
        let amend = current.job == Some(job)
            && current.parent.is_some()
            && current.children.is_empty()
            && current.saves.is_empty()
            && self.saved != Some(self.cursor);
        if !amend {
            self.push(state, label.into(), vec![step], Some(job));
            return Ok(outcome);
        }
        let old = self.cursor;
        let mut v = self.versions.remove(&old).expect("current version");
        let parent = v.parent.expect("a job's version has a parent");
        v.steps.push(step);
        coalesce(&mut v.steps);
        if v.steps.iter().all(|s| s.prims.is_empty()) {
            // The job put back all it did: back to the parent, whose state
            // this is (trimming may have dropped its snapshot). What the
            // version held waits for the next one, as if the job hadn't
            // run.
            let p = self.versions.get_mut(&parent).expect("parent");
            p.children.retain(|c| *c != old);
            if p.redo_to == Some(old) {
                p.redo_to = None;
            }
            p.state.get_or_insert(state);
            self.cursor = parent;
            v.sources.append(&mut self.unlogged_sources);
            self.unlogged_sources = v.sources;
        } else {
            (v.snapshot_bytes, v.step_bytes) = accounting(&v.steps, &state);
            v.state = Some(state);
            v.label = label.into();
            v.sources.append(&mut self.unlogged_sources);
            self.replace(old, v);
        }
        self.trim();
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

    /// Edits wait for the file's facts: there are none to edit before
    /// them, and none come after.
    fn check_ready(&self) -> Result<(), NotReady> {
        if self.ready { Ok(()) } else { Err(NotReady) }
    }

    /// Run `edit` on `state` as a new step, recording what it changed and
    /// describing it against `state`.
    fn step(&self, state: DocState, edit: Edit) -> Result<(DocState, EditOutcome, Step)> {
        let line = crate::describe::describe(&edit, &state);
        let mut rec = Rec::new(state);
        let out = self.compute(&mut rec, edit.clone())?;
        let (state, prims) = rec.finish();
        let prims = prims.into();
        Ok((state, out, Step { edit, line, prims }))
    }

    /// Version `id`'s state, rebuilt from the nearest ancestor that kept
    /// one by applying the prims of each version on the way. Damaged when
    /// one of them is already, or when a prim doesn't fit.
    fn replay(&self, id: VersionId) -> Result<DocState, Damaged> {
        let mut path = vec![id];
        let mut at = id;
        let base = loop {
            let parent = self.versions[&at]
                .parent
                .expect("the opened version keeps its state");
            if self.versions[&parent].state.is_some() {
                break parent;
            }
            path.push(parent);
            at = parent;
        };
        let mut state = self.versions[&base].state.clone().expect("base state");
        for &version in path.iter().rev() {
            let v = &self.versions[&version];
            if v.damaged {
                return Err(Damaged { version });
            }
            for p in v.steps.iter().flat_map(|s| s.prims.iter()) {
                if let Err(m) = state.apply(p) {
                    log::error!("version {}: {m}", version.0);
                    return Err(Damaged { version });
                }
            }
        }
        Ok(state)
    }

    /// Make sure version `id` has its state, replaying it when trimming
    /// dropped it. A version that can't be rebuilt is marked damaged; that
    /// is a bug, so a debug build stops there.
    fn ensure_state(&mut self, id: VersionId) -> Result<(), Damaged> {
        if self.versions[&id].state.is_some() {
            return Ok(());
        }
        match self.replay(id) {
            Ok(state) => {
                self.versions.get_mut(&id).expect("version").state = Some(state);
                Ok(())
            }
            Err(e) => {
                self.versions.get_mut(&e.version).expect("version").damaged = true;
                log::error!("{e}");
                if cfg!(debug_assertions) {
                    panic!("{e}");
                }
                Err(e)
            }
        }
    }

    /// A new version after the current one, with the edits that made it.
    /// The versions that were after the current one stay, as a branch.
    fn push(&mut self, state: DocState, label: String, steps: Vec<Step>, job: Option<JobId>) {
        let id = VersionId(self.next_version);
        self.next_version += 1;
        let parent = self.cursor;
        let (snapshot_bytes, step_bytes) = accounting(&steps, &state);
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
                damaged: false,
                snapshot_bytes,
                step_bytes,
                job,
                saves: Vec::new(),
            },
        );
        self.cursor = id;
        self.trim();
    }

    /// Put `v` in version `old`'s place under a new id: a VersionId names
    /// one state, so a version that changes is another version. It takes
    /// `old`'s place among its parent's children and as its children's
    /// parent, and becomes the root, current or saved version where `old`
    /// was. The caller has taken `old` out of the tree, so it no longer
    /// resolves.
    fn replace(&mut self, old: VersionId, v: Version) -> VersionId {
        let id = VersionId(self.next_version);
        self.next_version += 1;
        if let Some(p) = v.parent {
            let parent = self.versions.get_mut(&p).expect("parent");
            for c in parent.children.iter_mut().filter(|c| **c == old) {
                *c = id;
            }
            if parent.redo_to == Some(old) {
                parent.redo_to = Some(id);
            }
        }
        for c in &v.children {
            self.versions.get_mut(c).expect("child").parent = Some(id);
        }
        if self.root == old {
            self.root = id;
        }
        if self.cursor == old {
            self.cursor = id;
        }
        if self.saved == Some(old) {
            self.saved = Some(id);
        }
        self.versions.insert(id, v);
        id
    }

    /// Versions that must keep their snapshots: the opened one (every
    /// replay starts somewhere), the current one, every saved one (a save
    /// re-expressed it, so its steps don't lead to it), and the children
    /// of saved ones (those made before the save can't be replayed from it
    /// either).
    fn keeps_snapshot(&self, id: VersionId) -> bool {
        let v = &self.versions[&id];
        id == self.root
            || id == self.cursor
            || Some(id) == self.saved
            || !v.saves.is_empty()
            || v.parent
                .is_some_and(|p| !self.versions[&p].saves.is_empty())
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
    /// off the current `line` first (other branches, undone ones), then the
    /// line's, oldest first.
    fn droppable(&self, line: &HashSet<VersionId>) -> Vec<VersionId> {
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
        let line = self.line().into_iter().collect();
        let droppable = self.droppable(&line);
        let stats = self.stats_for(&line, &droppable);
        let n = self.policy.trim(&stats).min(droppable.len());
        for id in &droppable[..n] {
            self.versions.get_mut(id).expect("version").state = None;
        }
        self.prune();
    }

    /// Keep the tree to [`crate::history::MAX_VERSIONS`]: drop versions on
    /// other branches, oldest first, then the oldest versions before the
    /// saved one. Edits back to the last save always stay (a later merge
    /// replays them). A version dropped from a branch takes its
    /// registrations with it; the first version's go to the next, whose
    /// state was made over them.
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
            let mut sources = self.versions.remove(&old).expect("old root").sources;
            let new_root = self.versions.get_mut(&next).expect("next");
            new_root.parent = None;
            sources.append(&mut new_root.sources);
            new_root.sources = sources;
            self.root = next;
            i += 1;
        }
    }

    /// Take version `id` out of the tree, and the registrations it holds
    /// out of the document.
    fn remove_version(&mut self, id: VersionId) {
        let Some(v) = self.versions.remove(&id) else {
            return;
        };
        if let Some(p) = v.parent
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
        self.unregister(&v.sources);
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

    fn stats_for(&self, line: &HashSet<VersionId>, droppable: &[VersionId]) -> HistoryStats {
        let mut kept: Vec<VersionId> = self
            .versions
            .iter()
            .filter(|(id, v)| v.state.is_some() && !droppable.contains(id))
            .map(|(id, _)| *id)
            .collect();
        // The current version's last.
        kept.sort_by_key(|id| (*id == self.cursor, *id));
        let snapshot_bytes: Vec<usize> = droppable
            .iter()
            .chain(&kept)
            .map(|id| self.versions[id].snapshot_bytes)
            .collect();
        HistoryStats {
            versions: snapshot_bytes.len(),
            cursor: snapshot_bytes.len() - 1,
            snapshot_bytes,
            step_bytes: self.versions.values().map(|v| v.step_bytes).sum(),
            blob_bytes: self.store.bytes(),
            undone_source_bytes: self.undone_source_bytes(line),
        }
    }

    /// The bytes of the blobs only versions off the current `line` hold
    /// registrations of (inserts that were undone), each blob once. A blob
    /// a registration on the line, one waiting for the next version, or the
    /// primary shares isn't counted: pruning wouldn't free it.
    fn undone_source_bytes(&self, line: &HashSet<VersionId>) -> usize {
        let held = self.versions.iter().flat_map(|(id, v)| {
            let on_line = line.contains(id);
            v.sources.iter().map(move |s| (s, on_line))
        });
        let waiting = self.unlogged_sources.iter().chain([&self.primary]);
        let (mut kept, mut undone) = (HashSet::new(), HashMap::new());
        for (sid, on_line) in held.chain(waiting.map(|s| (s, true))) {
            let Some(blob) = self.sources.get(sid).map(|s| s.src.blob()) else {
                continue;
            };
            if on_line {
                kept.insert(blob.id());
            } else {
                undone.insert(blob.id(), blob.len());
            }
        }
        undone
            .into_iter()
            .filter(|(id, _)| !kept.contains(id))
            .map(|(_, len)| len)
            .sum()
    }

    /// Snapshots kept and their estimated bytes (droppable ones first), the
    /// bytes every version's steps hold, and the sources' bytes.
    pub fn stats(&self) -> HistoryStats {
        let line = self.line().into_iter().collect();
        self.stats_for(&line, &self.droppable(&line))
    }

    /// How many versions the history holds, with or without snapshots.
    pub fn version_count(&self) -> usize {
        self.versions.len()
    }

    fn view(&self, id: VersionId, shown_child: Option<VersionId>, undone: bool) -> VersionView {
        let v = &self.versions[&id];
        VersionView {
            id,
            label: v.label.clone(),
            author: v.author.clone(),
            at: v.at.clone(),
            edits: v.steps.iter().map(|s| s.line.clone()).collect(),
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
            damaged: v.damaged,
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

    /// Source `source`'s fields with widgets on the pages coming in
    /// (`shown`: each page's index in the source, and the page showing it),
    /// and its fields with no widget at all, into `rec` as a copy of their
    /// own (M10 proposal 5). Only widgets on those pages come along. When
    /// any of their names clashes with a field in the document, or a parent
    /// of one, they all go under a new parent field named for the file:
    /// its name. Pages of a file whose form is here already (a run of its
    /// pages after another, as Combine inserts them) join that form.
    fn take_in_fields(
        &self,
        rec: &mut Rec,
        source: SourceId,
        shown: &[(u32, PageId)],
    ) -> Option<String> {
        let form = self.forms.get(&source)?.clone();
        let on_pages: HashSet<FieldId> = shown
            .iter()
            .flat_map(|(index, _)| form.widgets.get(index).into_iter().flatten())
            .map(|w| w.field)
            .collect();
        if on_pages.is_empty() {
            return None;
        }
        let with_widgets: HashSet<FieldId> =
            form.widgets.values().flatten().map(|w| w.field).collect();
        let fields: Vec<&Field> = form
            .fields
            .iter()
            .filter(|f| on_pages.contains(&f.id) || !with_widgets.contains(&f.id))
            .collect();
        // The form as it's here already, by object: each field's id, and
        // the parent they're all under.
        let here: HashMap<lopdf::ObjectId, (FieldId, Option<Arc<str>>)> = rec
            .fields
            .values()
            .filter(|f| f.source == source && f.copy == 0)
            .map(|f| (f.object, (f.id, f.under.clone())))
            .collect();
        let (under, ids) = match here.values().next() {
            Some((_, joined)) => {
                let joined = joined.as_deref().map(str::to_string);
                let mut ids = HashMap::new();
                let mut new = Vec::new();
                for f in fields {
                    match here.get(&f.object) {
                        Some((id, _)) => {
                            ids.insert(f.id, *id);
                        }
                        None => new.push(f),
                    }
                }
                ids.extend(self.bring_fields(rec, source, 0, new, joined));
                (None, ids)
            }
            None => {
                let under = self.name_apart(rec, source, fields.iter().map(|f| f.name.as_str()));
                let copy = next_copy(rec, source);
                (
                    under.clone(),
                    self.bring_fields(rec, source, copy, fields, under),
                )
            }
        };
        for &(index, page) in shown {
            let list: Vector<Widget> = form
                .widgets
                .get(&index)
                .into_iter()
                .flatten()
                .filter_map(|w| {
                    Some(Widget {
                        field: *ids.get(&w.field)?,
                        ..w.clone()
                    })
                })
                .collect();
            if !list.is_empty() {
                rec.put(Key::WidgetsOf(page), Some(Val::Widgets(list)));
            }
        }
        under
    }

    /// Duplicate Pages: each copy's widgets get fields of their own, a new
    /// copy of each form the pages show, renamed by
    /// [`take_in_fields`](Self::take_in_fields)'s rule, so filling one never
    /// fills the other (M10 bug 4). `copies` pairs each page with its copy.
    fn copy_fields(&self, rec: &mut Rec, copies: &[(PageId, PageId)]) -> Vec<(String, SourceId)> {
        // The forms the pages show, and their fields there, in order.
        let mut forms: BTreeMap<(SourceId, u32), BTreeSet<FieldId>> = BTreeMap::new();
        for (page, _) in copies {
            for w in rec.widgets.get(page).into_iter().flatten() {
                if let Some(f) = rec.fields.get(&w.field) {
                    forms.entry((f.source, f.copy)).or_default().insert(f.id);
                }
            }
        }
        let mut renamed = Vec::new();
        let mut ids = HashMap::new();
        for ((source, _), fields) in forms {
            let fields: Vec<Field> = fields.iter().map(|id| rec.fields[id].clone()).collect();
            let under = self.name_apart(rec, source, fields.iter().map(|f| f.source_name()));
            renamed.extend(under.clone().map(|u| (u, source)));
            let copy = next_copy(rec, source);
            ids.extend(self.bring_fields(rec, source, copy, fields.iter(), under));
        }
        for &(page, copy) in copies {
            let list: Vector<Widget> = rec
                .widgets
                .get(&page)
                .into_iter()
                .flatten()
                .filter_map(|w| {
                    Some(Widget {
                        field: *ids.get(&w.field)?,
                        ..w.clone()
                    })
                })
                .collect();
            if !list.is_empty() {
                rec.put(Key::WidgetsOf(copy), Some(Val::Widgets(list)));
            }
        }
        renamed
    }

    /// `fields` of `source` into `rec` with new ids, as copy `copy` of its
    /// form, under `under` when given. Each old id, and its new one.
    fn bring_fields<'a>(
        &self,
        rec: &mut Rec,
        source: SourceId,
        copy: u32,
        fields: impl IntoIterator<Item = &'a Field>,
        under: Option<String>,
    ) -> HashMap<FieldId, FieldId> {
        let under: Option<Arc<str>> = under.map(Arc::from);
        let mut ids = HashMap::new();
        for f in fields {
            let id = self.ids.field();
            let name = match &under {
                Some(u) => format!("{u}.{}", f.source_name()),
                None => f.source_name().to_string(),
            };
            let field = Field {
                id,
                source,
                copy,
                under: under.clone(),
                name,
                ..f.clone()
            };
            rec.put(Key::Field(id), Some(Val::Field(Box::new(field))));
            ids.insert(f.id, id);
        }
        ids
    }

    /// The parent field `names` (full names, as their source has them) go
    /// under, when any of them, or a parent of one, is a field or parent
    /// in `st` already: the file's name, or with `_2`, `_3`… when that's
    /// taken. None when nothing clashes.
    fn name_apart<'a>(
        &self,
        st: &DocState,
        source: SourceId,
        names: impl IntoIterator<Item = &'a str>,
    ) -> Option<String> {
        let existing = field_nodes(st.fields.values().map(|f| f.name.as_str()));
        if !field_nodes(names).iter().any(|n| existing.contains(n)) {
            return None;
        }
        // A dot would make it two names.
        let stem = self
            .sources
            .get(&source)
            .and_then(|s| s.name.file_stem())
            .map(|s| s.to_string_lossy().replace('.', "_"))
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "form".to_string());
        (1..)
            .map(|n| {
                if n == 1 {
                    stem.clone()
                } else {
                    format!("{stem}_{n}")
                }
            })
            .find(|name| !existing.contains(name.as_str()))
    }

    /// Nodes for [`Edit::OutlineInsertTree`]: pre-order items with depths
    /// that start at 0 and go down at most one level at a time.
    fn tree_from_items(&self, items: &[NewOutlineItem]) -> Result<Vec<OutlineNode>> {
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
                id: self.ids.outline(),
                title: item.title.clone(),
                target: item.target,
                view: item.target.and(item.view.clone()),
                color: None,
                style: TextStyle::default(),
                open: true,
                children: Vector::new(),
            };
            stack.push((item.depth, node));
        }
        close(&mut stack, &mut roots, 0);
        Ok(roots)
    }

    fn outline_from_flat(
        &self,
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
                id: self.ids.outline(),
                title: item.title.clone(),
                target: item.page.and_then(|p| by_index.get(p as usize).copied()),
                view: item.view.clone(),
                color: item.color,
                style: item.style,
                open: item.open,
                children: Vector::new(),
            };
            stack.push((item.depth, node));
        }
        flush(&mut stack, &mut roots, 0);
        roots.into_iter().collect()
    }

    // ----- edits ------------------------------------------------------------

    fn compute(&self, rec: &mut Rec, edit: Edit) -> Result<EditOutcome> {
        let mut out = EditOutcome::default();
        match edit {
            Edit::MovePages { ids, before } => {
                let set: HashSet<PageId> = ids.iter().copied().collect();
                if let Some(b) = before
                    && set.contains(&b)
                {
                    bail!("cannot move pages before one of themselves");
                }
                let moving: Vec<PageRef> = rec
                    .pages
                    .iter()
                    .filter(|p| set.contains(&p.id))
                    .copied()
                    .collect();
                let rest: Vector<PageRef> = rec
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
                rec.put(Key::Pages, Some(Val::Pages(splice(&rest, at, moving))));
            }
            Edit::RotatePages { ids, by } => {
                let set: HashSet<PageId> = ids.into_iter().collect();
                let pages = rec
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
                rec.put(Key::Pages, Some(Val::Pages(pages)));
            }
            Edit::DeletePages { ids } => {
                let set: HashSet<PageId> = ids.into_iter().collect();
                let pages: Vector<PageRef> = rec
                    .pages
                    .iter()
                    .filter(|p| !set.contains(&p.id))
                    .copied()
                    .collect();
                out.pages_removed = rec.pages.len() - pages.len();
                rec.put(Key::Pages, Some(Val::Pages(pages)));
                for &id in &set {
                    rec.put(Key::AnnotsOf(id), None);
                    rec.put(Key::TextLayer(id), None);
                    rec.put(Key::RemovedOf(id), None);
                    rec.put(Key::WidgetsOf(id), None);
                }
                let (outline, removed) = cascade_delete(&rec.outline, &set);
                rec.put(Key::Outline, Some(Val::Outline(outline)));
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
                if range.end > info.src.page_count() {
                    bail!("range exceeds source page count");
                }
                let at = match before {
                    Some(b) => rec.index_of(b).context("destination page not found")?,
                    None => rec.pages.len(),
                };
                let new: Vec<PageRef> = range
                    .clone()
                    .map(|index| PageRef {
                        id: self.ids.page(),
                        source,
                        index,
                        rotation: info.src.geom(index).rotate,
                    })
                    .collect();
                out.pages_added = new.len();
                // Map source index -> new PageId for bookmark targets.
                let mut by_index = vec![PageId(u64::MAX); info.src.page_count() as usize];
                for p in &new {
                    by_index[p.index as usize] = p.id;
                }
                let pages = splice(&rec.pages, at, new);
                rec.put(Key::Pages, Some(Val::Pages(pages)));
                if let Some(items) = outline {
                    let valid: Vec<PageId> = by_index.to_vec();
                    let incoming = self.outline_from_flat(&items, &valid);
                    // Targets outside the inserted range resolve to a sentinel; drop them.
                    let incoming = strip_targets(&incoming, PageId(u64::MAX));
                    let (merged, added) = merge_outline(&rec.outline, &incoming);
                    rec.put(Key::Outline, Some(Val::Outline(merged)));
                    out.bookmarks_added = added;
                }
                let incoming = annots
                    .into_iter()
                    .filter_map(|(index, list)| {
                        let page = *by_index.get(index as usize)?;
                        (page != PageId(u64::MAX)).then_some((page, list))
                    })
                    .collect();
                out.unencodable = import_marks(rec, incoming);
                let shown: Vec<(u32, PageId)> = range.map(|i| (i, by_index[i as usize])).collect();
                if let Some(under) = self.take_in_fields(rec, source, &shown) {
                    out.forms_renamed.push((under, source));
                }
                if signed_from(rec, source) {
                    out.signatures_in.push(source);
                }
            }
            Edit::DuplicatePages { ids } => {
                let set: HashSet<PageId> = ids.into_iter().collect();
                let mut pages = Vector::new();
                // (page, its copy), for their fields.
                let mut copies = Vec::new();
                for p in rec.pages.clone() {
                    pages.push_back(p);
                    if set.contains(&p.id) {
                        let copy = self.ids.page();
                        if let Some(list) = rec.annots.get(&p.id).cloned() {
                            let mut taken = HashSet::new();
                            let list: Vector<Annot> = list
                                .into_iter()
                                .map(|mut a| {
                                    a.id = fresh_annot_id(rec, &mut taken);
                                    // The copy isn't the comment replies
                                    // elsewhere are to.
                                    a.name.clear();
                                    a
                                })
                                .collect();
                            rec.put(Key::AnnotsOf(copy), Some(Val::Annots(list)));
                        }
                        if let Some(layer) = rec.text_layers.get(&p.id).cloned() {
                            rec.put(Key::TextLayer(copy), Some(Val::TextLayer(layer)));
                        }
                        copies.push((p.id, copy));
                        pages.push_back(PageRef { id: copy, ..p });
                        out.pages_added += 1;
                    }
                }
                rec.put(Key::Pages, Some(Val::Pages(pages)));
                out.forms_renamed = self.copy_fields(rec, &copies);
            }
            Edit::ReplacePages {
                ids,
                source,
                range,
                annots,
            } => {
                let info = self.sources.get(&source).context("unknown source")?.clone();
                if range.end > info.src.page_count() {
                    bail!("range exceeds source page count");
                }
                let mut replacements = range.clone();
                let mut last_pos = None;
                let mut extra_old = HashSet::new();
                // Which page now shows each replacement, for its marks.
                let mut shown_by: HashMap<u32, PageId> = HashMap::new();
                let pos = rec.page_positions();
                let mut pages = rec.pages.clone();
                for id in &ids {
                    let i = *pos.get(id).context("page to replace not found")?;
                    // The page shows different content now; its OCR is
                    // stale, and its widgets aren't on it.
                    rec.put(Key::TextLayer(*id), None);
                    rec.put(Key::WidgetsOf(*id), None);
                    match replacements.next() {
                        Some(index) => {
                            pages.set(
                                i,
                                PageRef {
                                    id: *id,
                                    source,
                                    index,
                                    rotation: info.src.geom(index).rotate,
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
                let extra: Vec<PageRef> = replacements
                    .map(|index| {
                        let id = self.ids.page();
                        shown_by.insert(index, id);
                        PageRef {
                            id,
                            source,
                            index,
                            rotation: info.src.geom(index).rotate,
                        }
                    })
                    .collect();
                out.pages_added += extra.len();
                pages = splice(&pages, at, extra);
                if !extra_old.is_empty() {
                    out.pages_removed = extra_old.len();
                    pages = pages
                        .iter()
                        .filter(|p| !extra_old.contains(&p.id))
                        .copied()
                        .collect();
                    for &id in &extra_old {
                        rec.put(Key::AnnotsOf(id), None);
                        rec.put(Key::TextLayer(id), None);
                    }
                    let (outline, removed) = cascade_delete(&rec.outline, &extra_old);
                    rec.put(Key::Outline, Some(Val::Outline(outline)));
                    out.bookmarks_removed = removed;
                }
                rec.put(Key::Pages, Some(Val::Pages(pages)));
                let incoming = annots
                    .into_iter()
                    .filter_map(|(index, list)| Some((*shown_by.get(&index)?, list)))
                    .collect();
                out.unencodable = import_marks(rec, incoming);
                let mut shown: Vec<(u32, PageId)> = shown_by.into_iter().collect();
                shown.sort();
                if let Some(under) = self.take_in_fields(rec, source, &shown) {
                    out.forms_renamed.push((under, source));
                }
                if signed_from(rec, source) {
                    out.signatures_in.push(source);
                }
            }
            Edit::OutlineInsert {
                parent,
                after,
                title,
                target,
                view,
            } => {
                let node = OutlineNode {
                    id: self.ids.outline(),
                    title,
                    view: target.and(view),
                    target,
                    color: None,
                    style: TextStyle::default(),
                    open: true,
                    children: Vector::new(),
                };
                let outline = outline_insert(&rec.outline, parent, after, node)?;
                rec.put(Key::Outline, Some(Val::Outline(outline)));
                out.bookmarks_added = 1;
            }
            Edit::OutlineInsertTree {
                parent,
                after,
                items,
            } => {
                let roots = self.tree_from_items(&items)?;
                out.bookmarks_added = items.len();
                let mut outline = rec.outline.clone();
                let mut after = after;
                for node in roots {
                    let id = node.id;
                    outline = outline_insert(&outline, parent, after, node)?;
                    after = Some(id);
                }
                rec.put(Key::Outline, Some(Val::Outline(outline)));
            }
            Edit::OutlineMove {
                id,
                parent,
                after,
                with_pages,
            } => {
                if parent == Some(id) || outline_find(&rec.outline, id).is_none() {
                    bail!("bookmark not found or moved into itself");
                }
                if let Some(p) = parent
                    && subtree_contains(&rec.outline, id, p)
                {
                    bail!("cannot move a bookmark under its own descendant");
                }
                let (without, node) = outline_remove(&rec.outline, id);
                let node = node.context("bookmark not found")?;
                let span = with_pages.then(|| bookmark_span(rec, &node)).flatten();
                let outline = outline_insert(&without, parent, after, node.clone())?;
                rec.put(Key::Outline, Some(Val::Outline(outline)));
                if let Some(span) = span {
                    // Destination: right after the span of whatever precedes the
                    // moved node in the new pre-order (or the start).
                    let flat = rec.outline_flat();
                    let my_pos = flat.iter().position(|(_, n)| n.id == id).unwrap();
                    let moving: Vec<PageId> = span.iter().map(|p| p.id).collect();
                    let moving_set: HashSet<PageId> = moving.iter().copied().collect();
                    let rest: Vector<PageRef> = rec
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
                                outline: rec.outline.clone(),
                                annots: rec.annots.clone(),
                                text_layers: rec.text_layers.clone(),
                                labels: rec.labels.clone(),
                                header_footer: rec.header_footer.clone(),
                                removed_comments: rec.removed_comments.clone(),
                                fields: rec.fields.clone(),
                                widgets: rec.widgets.clone(),
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
                    rec.put(Key::Pages, Some(Val::Pages(splice(&rest, dest, span))));
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
                let updated = outline_update(&rec.outline, id, |n| {
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
                let outline = updated.context("bookmark not found")?;
                rec.put(Key::Outline, Some(Val::Outline(outline)));
            }
            Edit::OutlineDelete { id } => {
                let (without, node) = outline_remove(&rec.outline, id);
                let node = node.context("bookmark not found")?;
                out.bookmarks_removed = 1 + count_nodes(&node.children);
                rec.put(Key::Outline, Some(Val::Outline(without)));
            }
            Edit::AnnotAdd { page, mut annot } => {
                if rec.index_of(page).is_none() {
                    bail!("page not found");
                }
                annot.id = fresh_annot_id(rec, &mut HashSet::new());
                out.unencodable = annot.unencodable();
                let mut list = rec.annots.get(&page).cloned().unwrap_or_default();
                list.push_back(annot);
                rec.put(Key::AnnotsOf(page), Some(Val::Annots(list)));
            }
            Edit::AnnotUpdate { page, annot } => {
                let mut list = rec
                    .annots
                    .get(&page)
                    .cloned()
                    .context("page has no annotations")?;
                let i = list
                    .iter()
                    .position(|a| a.id == annot.id)
                    .context("annotation not found")?;
                out.unencodable = annot.unencodable();
                list.set(i, annot);
                rec.put(Key::AnnotsOf(page), Some(Val::Annots(list)));
            }
            Edit::AnnotDelete { page, id } => {
                let mut list = rec
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
                rec.put(
                    Key::AnnotsOf(page),
                    (!list.is_empty()).then_some(Val::Annots(list)),
                );
                // A connector is one of a group by label: the others stay
                // (M9 decision 8).
            }
            Edit::CommentsDelete { page, names } => {
                if rec.index_of(page).is_none() {
                    bail!("page not found");
                }
                // Only names not deleted yet change what a save leaves out.
                let mut set = rec.removed_comments.get(&page).cloned().unwrap_or_default();
                let had = set.len();
                set.extend(names.iter().cloned());
                if set.len() > had {
                    rec.put(Key::RemovedOf(page), Some(Val::Removed(set)));
                }
                // Our replies and statuses to them, and to those.
                if let Some(mut list) = rec.annots.get(&page).cloned() {
                    let mut gone = names;
                    while let Some(name) = gone.pop() {
                        let (thread, rest): (Vector<Annot>, Vector<Annot>) = list
                            .into_iter()
                            .partition(|a| a.thread_parent() == Some(name.as_str()));
                        gone.extend(thread.iter().map(Annot::name));
                        list = rest;
                    }
                    rec.put(
                        Key::AnnotsOf(page),
                        (!list.is_empty()).then_some(Val::Annots(list)),
                    );
                }
            }
            Edit::ConnectorAdd {
                a: (page_a, mut a),
                b: (page_b, mut b),
            } => {
                if rec.index_of(page_a).is_none() || rec.index_of(page_b).is_none() {
                    bail!("page not found");
                }
                let mut taken = HashSet::new();
                a.id = fresh_annot_id(rec, &mut taken);
                b.id = fresh_annot_id(rec, &mut taken);
                if let AnnotKind::Connector { partner, .. } = &mut a.kind {
                    *partner = Some(b.id);
                }
                if let AnnotKind::Connector { partner, .. } = &mut b.kind {
                    *partner = Some(a.id);
                }
                out.unencodable = a.unencodable() || b.unencodable();
                for (page, annot) in [(page_a, a), (page_b, b)] {
                    let mut list = rec.annots.get(&page).cloned().unwrap_or_default();
                    list.push_back(annot);
                    rec.put(Key::AnnotsOf(page), Some(Val::Annots(list)));
                }
            }
            Edit::SetTextLayers { layers } => {
                let pos = rec.page_positions();
                for (id, layer) in layers {
                    if !pos.contains_key(&id) {
                        bail!("page not found");
                    }
                    rec.put(Key::TextLayer(id), Some(Val::TextLayer(layer)));
                }
            }
            Edit::ClearTextLayers { ids } => {
                for id in ids {
                    rec.put(Key::TextLayer(id), None);
                }
            }
            Edit::SetHeaderFooter { header_footer } => {
                let hf = header_footer.filter(|h| !h.is_empty());
                out.unencodable = hf.as_ref().is_some_and(|h| h.unencodable());
                rec.put(
                    Key::HeaderFooter,
                    hf.map(|h| Val::HeaderFooter(Arc::new(h))),
                );
            }
            Edit::SetPageLabels { ranges } => {
                let labels =
                    (!ranges.is_empty()).then(|| Val::Labels(ranges.into_iter().collect()));
                rec.put(Key::Labels, labels);
            }
            Edit::RebasePages { pages: rebases } => {
                let pos = rec.page_positions();
                let mut pages = rec.pages.clone();
                for r in rebases {
                    let i = *pos.get(&r.id).context("page to rebase not found")?;
                    let info = self.sources.get(&r.source).context("unknown source")?;
                    if r.index >= info.src.page_count() {
                        bail!("page {} isn't in the source", r.index);
                    }
                    let old = pages[i];
                    pages.set(
                        i,
                        PageRef {
                            id: r.id,
                            source: r.source,
                            index: r.index,
                            rotation: old.rotation,
                        },
                    );
                }
                rec.put(Key::Pages, Some(Val::Pages(pages)));
            }
        }
        Ok(out)
    }
}

/// Each slot's change over `steps` as one prim, on the last step that
/// changed it: from the first step's before to the last one's after, and
/// none when they're the same (M10.5 §6.2). So a batch that puts a slot
/// back changes nothing. Every step keeps its edit, with or without prims.
fn coalesce(steps: &mut [Step]) {
    // Each slot's first before, and the first and last steps that put it.
    let mut spans: BTreeMap<Key, (Option<Val>, usize, usize)> = BTreeMap::new();
    for (i, step) in steps.iter().enumerate() {
        for p in step.prims.iter() {
            spans
                .entry(p.key)
                .and_modify(|(_, _, last)| *last = i)
                .or_insert_with(|| (p.before.clone(), i, i));
        }
    }
    let spans_steps = |p: &Prim| {
        let (_, first, last) = &spans[&p.key];
        first != last
    };
    for (i, step) in steps.iter_mut().enumerate() {
        if !step.prims.iter().any(spans_steps) {
            continue;
        }
        step.prims = step
            .prims
            .iter()
            .filter_map(|p| {
                let (before, _, last) = &spans[&p.key];
                (*last == i && !same(before.as_ref(), p.after.as_ref())).then(|| Prim {
                    key: p.key,
                    before: before.clone(),
                    after: p.after.clone(),
                })
            })
            .collect();
    }
}

/// What a version holds that no other does (M10.5 §3.9), from the steps
/// that made it and its state: its snapshot's bytes, which trimming frees,
/// and its steps', which only pruning the version frees.
///
/// A step's before-values are its parent's after-values, so each value is
/// counted once, by the version that made it, and an `Arc` once however
/// many of the version's prims put it. Those values are the snapshot's
/// too, so the snapshot adds only the im nodes it copied to change them:
/// for each slot it changed in a per-page or per-field map, a path down
/// from the root. A whole value (the pages, the outline, the labels)
/// counts in full, though im shares all but its changed path with the
/// parent's: an over-count M10.5 slice 8's records narrow.
fn accounting(steps: &[Step], state: &DocState) -> (usize, usize) {
    // im's HashMap is a tree of 32 entries a node; its OrdMap, of 64.
    const HAMT: usize = 32;
    const BTREE: usize = 64;
    let mut step_bytes = 0;
    let mut arcs = HashSet::new();
    let (mut annots, mut layers, mut removed, mut fields, mut widgets) = (0, 0, 0, 0, 0);
    // `DocState` holds fields by value, so its copy is its own.
    let mut field_copies = 0;
    for s in steps {
        step_bytes += size_of::<Step>() + s.edit.heap_size() + s.line.heap_size();
        for p in s.prims.iter() {
            let counted = match &p.after {
                Some(Val::TextLayer(l)) => arcs.insert(Arc::as_ptr(l).cast::<()>()),
                Some(Val::HeaderFooter(h)) => arcs.insert(Arc::as_ptr(h).cast::<()>()),
                _ => true,
            };
            step_bytes += size_of::<Prim>();
            if counted {
                step_bytes += p.after.heap_size();
            }
            match p.key {
                Key::Pages | Key::Outline | Key::Labels | Key::HeaderFooter => {}
                Key::AnnotsOf(_) => annots += 1,
                Key::TextLayer(_) => layers += 1,
                Key::RemovedOf(_) => removed += 1,
                Key::Field(_) => {
                    fields += 1;
                    if let Some(Val::Field(f)) = &p.after {
                        field_copies += Field::heap_size(f);
                    }
                }
                Key::WidgetsOf(_) => widgets += 1,
            }
        }
    }
    let snapshot_bytes = size_of::<DocState>()
        + field_copies
        + path_copies::<(PageId, Vector<Annot>)>(annots, state.annots.len(), HAMT)
        + path_copies::<(PageId, Arc<TextLayer>)>(layers, state.text_layers.len(), HAMT)
        + path_copies::<(PageId, im::OrdSet<String>)>(removed, state.removed_comments.len(), HAMT)
        + path_copies::<(FieldId, Field)>(fields, state.fields.len(), BTREE)
        + path_copies::<(PageId, Vector<Widget>)>(widgets, state.widgets.len(), HAMT);
    (snapshot_bytes, step_bytes)
}

/// The bytes of the im nodes copied to change `touched` of a map's `len`
/// entries, at `fanout` entries `E` a node: a path down from the root to
/// each, sharing the nodes near the root.
fn path_copies<E>(touched: usize, len: usize, fanout: usize) -> usize {
    if touched == 0 {
        return 0;
    }
    let (mut nodes, mut level, mut reach) = (0, 1, fanout);
    loop {
        nodes += touched.min(level);
        if reach >= len {
            return nodes * fanout * size_of::<E>();
        }
        level *= fanout;
        reach *= fanout;
    }
}

/// `pages` with `new` put in at position `at`, by one split and two joins:
/// inserting them one by one edits the tree once a page (189 ms for half
/// of 7,000 pages, against half a millisecond).
fn splice(
    pages: &Vector<PageRef>,
    at: usize,
    new: impl IntoIterator<Item = PageRef>,
) -> Vector<PageRef> {
    let mut out = pages.clone();
    let tail = out.split_off(at);
    out.extend(new);
    out.append(tail);
    out
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
/// `/NM`; 53 bits, so the JSON is exact in any reader.
fn fresh_annot_id(st: &DocState, taken: &mut HashSet<AnnotId>) -> AnnotId {
    loop {
        let id = AnnotId(fastrand::u64(1..1 << 53));
        if !taken.contains(&id) && st.find_annot(id).is_none() {
            taken.insert(id);
            return id;
        }
    }
}

/// Whether `st` has signed fields from `source` (another file's: they're
/// kept, and their signatures don't cover this one).
fn signed_from(st: &DocState, source: SourceId) -> bool {
    st.fields
        .values()
        .any(|f| f.source == source && f.value == Some(crate::forms::Value::Signed))
}

/// The copy of `source`'s form fields brought in next take: one past the
/// last one `st` has, or 0.
fn next_copy(st: &DocState, source: SourceId) -> u32 {
    st.fields
        .values()
        .filter(|f| f.source == source)
        .map(|f| f.copy + 1)
        .max()
        .unwrap_or(0)
}

/// Every node of a field tree with these full names: each name, and the
/// parents in it (what comes before each dot).
fn field_nodes<'a>(names: impl IntoIterator<Item = &'a str>) -> HashSet<&'a str> {
    let mut out = HashSet::new();
    for name in names {
        out.insert(name);
        for (i, _) in name.match_indices('.') {
            out.insert(&name[..i]);
        }
    }
    out
}

/// Marks that came with pages from another file, onto their new pages in
/// `rec`. They get fresh ids (the file's could clash with ours), and a
/// connector keeps its partner only if the partner came too. Whether any
/// draws text the built-in fonts can't ([`Annot::unencodable`]).
fn import_marks(rec: &mut Rec, incoming: Vec<(PageId, Vec<Annot>)>) -> bool {
    let unencodable = incoming.iter().flat_map(|(_, l)| l).any(Annot::unencodable);
    let mut taken = HashSet::new();
    let mut ids = HashMap::new();
    for (_, list) in &incoming {
        for a in list {
            ids.insert(a.id, fresh_annot_id(rec, &mut taken));
        }
    }
    for (page, list) in incoming {
        let mut on_page = rec.annots.get(&page).cloned().unwrap_or_default();
        for mut a in list {
            a.id = ids[&a.id];
            if let AnnotKind::Connector { partner, .. } = &mut a.kind {
                *partner = partner.and_then(|p| ids.get(&p).copied());
            }
            on_page.push_back(a);
        }
        if !on_page.is_empty() {
            rec.put(Key::AnnotsOf(page), Some(Val::Annots(on_page)));
        }
    }
    unencodable
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
mod history_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::FixedCount;

    /// A document of `n` blank pages, in a store of its own.
    pub(super) fn new_doc(
        n: u32,
        outline: &[FlatOutlineItem],
        policy: Box<dyn HistoryPolicy>,
    ) -> Document {
        let store = Store::new();
        let primary = SourceInfo::blank(&store, n);
        Document::new(store, primary, outline, policy)
    }

    /// `n` blank pages opened as a file opens, saying `facts`: started on
    /// its pages, then ready.
    pub(super) fn opened_doc(n: u32, facts: Facts, policy: Box<dyn HistoryPolicy>) -> Document {
        let store = Store::new();
        let primary = SourceInfo::blank(&store, n);
        let mut d = crate::open::start(store, primary, policy);
        d.ready(facts).unwrap();
        d
    }

    pub(super) fn flat(items: &[(&str, Option<u32>, u16)]) -> Vec<FlatOutlineItem> {
        items
            .iter()
            .map(|(t, p, d)| FlatOutlineItem {
                title: t.to_string(),
                page: *p,
                depth: *d,
                view: None,
                color: None,
                style: Default::default(),
                open: true,
            })
            .collect()
    }

    fn doc() -> Document {
        // Pages 0..6; bookmarks: A->0 { A1->1, A2->2 }, B->3, C->5
        new_doc(
            6,
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

    /// A header of `text`, top left.
    pub(super) fn header(text: String) -> crate::stamps::HeaderFooter {
        let mut texts: [String; 6] = Default::default();
        texts[0] = text;
        crate::stamps::HeaderFooter {
            texts,
            ..Default::default()
        }
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

    /// While the file opens, every edit is refused before it draws an id
    /// or makes a version; once it's ready, the same edits are taken.
    #[test]
    fn an_edit_before_ready_is_not_ready() {
        let store = Store::new();
        let primary = SourceInfo::blank(&store, 3);
        let mut d = crate::open::start(store, primary, Box::new(FixedCount(10)));
        assert!(!d.is_ready());
        let page = id_at(&d, 0);
        let duplicate = || Edit::DuplicatePages { ids: vec![page] };
        let bookmark = || Edit::OutlineInsert {
            parent: None,
            after: None,
            title: "One".into(),
            target: Some(page),
            view: None,
        };
        let drawn = |d: &Document| {
            [&d.ids.page, &d.ids.outline, &d.ids.field].map(|c| c.load(Ordering::Relaxed))
        };
        let before = (d.current_version(), d.next_version, drawn(&d));
        let job = d.begin_job();
        let refused = [
            d.apply(duplicate(), "Duplicate").unwrap_err(),
            d.apply_batch(vec![duplicate(), bookmark()], "Both")
                .unwrap_err(),
            d.apply_in_job(job, bookmark(), "Bookmark").unwrap_err(),
        ];
        for e in &refused {
            assert!(e.is::<NotReady>(), "{e:#}");
        }
        assert_eq!((d.current_version(), d.next_version, drawn(&d)), before);
        assert_eq!(d.versions.len(), 1);
        d.ready(Facts::default()).unwrap();
        d.apply(duplicate(), "Duplicate").unwrap();
        d.apply_batch(vec![duplicate(), bookmark()], "Both")
            .unwrap();
        d.apply_in_job(job, bookmark(), "Bookmark").unwrap();
        assert_eq!((d.page_count(), d.state().outline.len()), (5, 2));
        assert_eq!(d.versions.len(), 4);
    }

    /// `ready` puts the complete opened version in place of the one with
    /// the pages alone: the facts are on the pages the document showed
    /// while it opened, and there's nothing to save or undo. It's taken
    /// once.
    #[test]
    fn ready_replaces_the_opened_version() {
        let store = Store::new();
        let bytes = crate::forms::fixture::every_fact().bytes();
        let primary = file(&store, bytes, "facts.pdf");
        let src = primary.src.clone();
        let mut d = crate::open::start(store, primary, Box::new(FixedCount(10)));
        let loading = d.current_version();
        let pages: Vec<PageId> = d.state().pages.iter().map(|p| p.id).collect();
        let st = d.state();
        assert_eq!(pages.len(), 3);
        assert!(st.outline.is_empty() && st.annots.is_empty() && st.fields.is_empty());
        assert!(st.labels.is_none() && st.header_footer.is_none());
        let view = d
            .ready(crate::open::finish(&src, &sheaf_progress::NoProgress))
            .unwrap();
        assert!(d.is_ready());
        let opened = d.current_version();
        assert_ne!(opened, loading);
        assert!(!d.versions.contains_key(&loading));
        assert!(d.jump(loading).is_err());
        assert_eq!((d.root, d.saved), (opened, Some(opened)));
        let now: Vec<PageId> = d.state().pages.iter().map(|p| p.id).collect();
        assert_eq!(now, pages);
        crate::open::tests::assert_every_fact(d.state());
        assert_eq!(view.form.fields, 1);
        assert!(!d.is_dirty() && !d.can_undo() && !d.can_redo());
        let snap = d.snapshot();
        assert!(!snap.outline_dirty && !snap.labels_dirty && !snap.header_footer_dirty);
        assert!(snap.annots_dirty.is_empty() && snap.fields_dirty.is_empty());
        assert!(!snap.removes_content && !snap.form_changed);
        // Facts that came again would be late: they're refused.
        assert!(
            d.ready(crate::open::finish(&src, &sheaf_progress::NoProgress))
                .is_err()
        );
        assert_eq!(d.current_version(), opened);
        crate::open::tests::assert_every_fact(d.state());
    }

    #[test]
    fn form_fields_follow_their_pages() {
        let form = crate::forms::read(&crate::forms::fixture::every_kind().source());
        let facts = Facts {
            form,
            ..Default::default()
        };
        let mut d = opened_doc(2, facts, Box::new(FixedCount(10)));
        let (p1, p2) = (id_at(&d, 0), id_at(&d, 1));
        let st = d.state();
        assert_eq!(st.fields.len(), 9);
        assert!(st.fields.values().all(|f| f.source == d.primary()));
        let on = |d: &Document, p: PageId| d.state().widgets_on(p).map_or(0, |w| w.len());
        assert_eq!((on(&d, p1), on(&d, p2)), (6, 4));
        assert!(d.fields_dirty().is_empty() && d.snapshot().fields_dirty.is_empty());
        // Moving a page carries its widgets; deleting it takes them, and
        // undo brings them back.
        d.apply(
            Edit::MovePages {
                ids: vec![p2],
                before: Some(p1),
            },
            "move",
        )
        .unwrap();
        assert_eq!(on(&d, p2), 4);
        // The file's facts came once: none come after.
        let again = crate::forms::read(&crate::forms::fixture::every_kind().source());
        let late = Facts {
            form: again,
            ..Default::default()
        };
        assert!(d.ready(late).is_err());
        d.apply(Edit::DeletePages { ids: vec![p2] }, "delete")
            .unwrap();
        assert_eq!((on(&d, p2), d.state().fields.len()), (0, 9));
        d.undo().unwrap();
        assert_eq!(on(&d, p2), 4);
        // A replaced page shows another page, without them.
        let other = d.add_source(SourceInfo::blank(d.store(), 1));
        d.apply(
            Edit::ReplacePages {
                ids: vec![p1],
                source: other,
                range: 0..1,
                annots: Vec::new(),
            },
            "replace",
        )
        .unwrap();
        assert_eq!(on(&d, p1), 0);
        // Nothing changed a field.
        assert!(d.fields_dirty().is_empty());
    }

    #[test]
    fn fields_dirty_compares_with_the_saved_form() {
        let form = crate::forms::read(&crate::forms::fixture::every_kind().source());
        let facts = Facts {
            form,
            ..Default::default()
        };
        let mut d = opened_doc(2, facts, Box::new(FixedCount(10)));
        // A field changed, one gone and one added in the state `ready` set,
        // as edits will do.
        let (changed, gone, added) = (FieldId(0), FieldId(1), FieldId(99));
        let root = d.root;
        let st = d.versions.get_mut(&root).unwrap().state.as_mut().unwrap();
        let mut f = st.fields.get(&changed).unwrap().clone();
        f.value = Some(crate::forms::Value::Text("Bo Ng".into()));
        st.fields.insert(changed, f.clone());
        st.fields.remove(&gone);
        f.id = added;
        st.fields.insert(added, f);
        let want = HashSet::from([changed, gone, added]);
        assert_eq!(d.fields_dirty(), want);
        assert_eq!(d.snapshot().fields_dirty, want);
        // Saved, they're what's in the file.
        d.rebase_to_saved(SourceInfo::blank(d.store(), 2));
        assert!(d.fields_dirty().is_empty() && d.snapshot().fields_dirty.is_empty());
    }

    /// One file's bytes, interned twice, are one source; each insert still
    /// registers it anew, so each brings a form of its own (M10.5 slice 3:
    /// form identity stays per registration until slice 11).
    #[test]
    fn inserting_the_same_file_twice_gives_two_registrations_and_two_forms() {
        let mut fixture = crate::forms::fixture::every_kind();
        let bytes: Arc<[u8]> = fixture.bytes().into();
        let mut d = new_doc(1, &[], Box::new(FixedCount(10)));
        let mut sids = Vec::new();
        let mut renamed = Vec::new();
        for _ in 0..2 {
            let kind = BlobKind::file(Path::new("form.pdf"), None);
            let src = d.store().open(bytes.clone(), kind).unwrap();
            let form = crate::forms::read(&src);
            let sid = d.add_source(SourceInfo {
                src,
                name: "form.pdf".into(),
                derived: None,
            });
            d.set_source_form(sid, form);
            let out = d
                .apply(
                    Edit::InsertPages {
                        source: sid,
                        range: 0..2,
                        before: None,
                        outline: None,
                        annots: Vec::new(),
                    },
                    "insert",
                )
                .unwrap();
            renamed.push(out.forms_renamed);
            sids.push(sid);
        }
        let [a, b] = [sids[0], sids[1]];
        assert_ne!(a, b);
        let (first, second) = (&d.source(a).unwrap().src, &d.source(b).unwrap().src);
        assert!(std::ptr::eq(&**first, &**second), "one source");
        // Its bytes are in the store once, beside the blank primary's.
        assert_eq!(
            d.store().bytes(),
            first.bytes().len() + d.source(d.primary()).unwrap().src.bytes().len()
        );
        // Two forms: the second's fields are under a name of their own.
        let st = d.state();
        let names = |sid| -> BTreeSet<&str> {
            st.fields
                .values()
                .filter(|f| f.source == sid)
                .map(|f| f.name.as_str())
                .collect()
        };
        let (of_a, of_b) = (names(a), names(b));
        assert_eq!((of_a.len(), of_b.len()), (9, 9));
        assert!(of_a.is_disjoint(&of_b), "{of_a:?} {of_b:?}");
        assert!(of_b.iter().all(|n| n.starts_with("form.")), "{of_b:?}");
        assert_eq!(renamed, [vec![], vec![("form".to_string(), b)]]);
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
        assert_eq!(d.undo(), Ok(Some("rotate")));
        assert_eq!(d.page(0).unwrap().rotation, Rotation::R0);
        assert_eq!(d.undo(), Ok(Some("move")));
        assert_eq!(order(&d), vec![0, 1, 2, 3, 4, 5]);
        assert!(!d.is_dirty());
        assert_eq!(d.redo(), Ok(Some("move")));
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
        d.undo().unwrap();
        assert_eq!(titles(&d).len(), 5);
    }

    #[test]
    fn insert_with_outline_merge() {
        let mut d = doc();
        let s = d.add_source(SourceInfo {
            name: "b.pdf".into(),
            ..SourceInfo::blank(d.store(), 3)
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
            name: "b.pdf".into(),
            ..SourceInfo::blank(d.store(), 2)
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
        d.undo().unwrap();
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
        d.undo().unwrap();
        d.undo().unwrap();
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
        assert_eq!(d.redo(), Ok(Some("c")));
        // Undo to a, and redo now follows the branch last visited.
        d.undo().unwrap();
        d.undo().unwrap();
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

    #[test]
    fn history_trims_snapshots_and_replays_them() {
        let mut d = new_doc(3, &[], Box::new(FixedCount(3)));
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
            assert_eq!(d.undo(), Ok(Some(format!("r{i}").as_str())));
            assert_eq!(d.page(0).unwrap().rotation.degrees(), (i * 90) % 360);
        }
        assert!(!d.can_undo());
        assert!(!d.is_dirty(), "the opened version is the saved one");
        for _ in 0..5 {
            d.redo().unwrap();
        }
        assert_eq!(d.page(0).unwrap().rotation, Rotation::R90);
    }

    /// Three rotations, keeping only the snapshots that must stay (the
    /// opened version's and the current one's): the versions, oldest
    /// first.
    fn trimmed_history() -> (Document, Vec<VersionId>) {
        let mut d = new_doc(6, &[], Box::new(FixedCount(1)));
        for i in 0..3 {
            rotate(&mut d, i, &format!("r{i}"));
        }
        let ids = d.timeline().iter().map(|v| v.id).collect();
        (d, ids)
    }

    /// Version `v`'s Pages prim, made to say the pages were in another
    /// order before: the prim, as it is now.
    fn damage_pages_before(d: &mut Document, v: VersionId) -> Prim {
        let step = &mut d.versions.get_mut(&v).unwrap().steps[0];
        let mut prims = step.prims.to_vec();
        let p = prims.iter_mut().find(|p| p.key == Key::Pages).unwrap();
        let Some(Val::Pages(pages)) = &p.before else {
            panic!("pages hold a value");
        };
        p.before = Some(Val::Pages(pages.iter().rev().copied().collect()));
        let damaged = p.clone();
        step.prims = prims.into();
        damaged
    }

    /// A version's lines are made when its edits are applied, so they still
    /// name pages once its parent's snapshot is dropped.
    #[test]
    fn a_trimmed_version_keeps_its_lines() {
        let (d, _) = trimmed_history();
        let views = d.timeline();
        assert!(
            views[1].replays && views[2].replays,
            "their snapshots were dropped"
        );
        let lines: Vec<&[String]> = views[1..].iter().map(|v| &v.edits[..]).collect();
        assert_eq!(
            lines,
            [
                ["Rotate page 1 90°"],
                ["Rotate page 2 90°"],
                ["Rotate page 3 90°"]
            ]
        );
    }

    #[test]
    fn a_corrupted_before_value_is_a_mismatch() {
        let (mut d, ids) = trimmed_history();
        let (parent, v) = (ids[1], ids[2]);
        assert!(d.versions[&v].state.is_none(), "its snapshot was dropped");
        let p = damage_pages_before(&mut d, v);
        assert_eq!(d.replay(v), Err(Damaged { version: v }));
        let mut st = d.replay(parent).unwrap();
        assert_eq!(st.apply(&p), Err(crate::prim::Mismatch { key: Key::Pages }));
    }

    /// `f`'s result in a release build. A debug build stops at damage in
    /// history, once it's marked, so there `f` must panic.
    fn unless_debug<T>(f: impl FnOnce() -> T) -> Option<T> {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        assert_eq!(r.is_err(), cfg!(debug_assertions), "panics in debug only");
        r.ok()
    }

    #[test]
    fn undo_over_damaged_history_says_so() {
        let (mut d, ids) = trimmed_history();
        let (v, current) = (ids[2], ids[3]);
        let prims = d.versions[&v].steps[0].prims.clone();
        damage_pages_before(&mut d, v);
        if let Some(undone) = unless_debug(|| d.undo().map(|l| l.map(str::to_owned))) {
            assert_eq!(undone, Err(Damaged { version: v }));
        }
        assert_eq!(d.current_version(), current, "nothing was undone");
        let damaged: Vec<VersionId> = d
            .timeline()
            .iter()
            .filter(|view| view.damaged)
            .map(|view| view.id)
            .collect();
        assert_eq!(damaged, [v]);
        // Marked, it stays out of reach, even with its prims put right.
        d.versions.get_mut(&v).unwrap().steps[0].prims = prims;
        assert_eq!(d.replay(v), Err(Damaged { version: v }));
        if let Some(jumped) = unless_debug(|| d.jump(v).is_ok()) {
            assert!(!jumped, "it can't be reached");
        }
        assert_eq!(d.current_version(), current);
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
            name: "saved.pdf".into(),
            ..SourceInfo::blank(d.store(), 6)
        });
        assert_ne!(new, old_primary);
        assert!(!d.is_dirty());
        assert_eq!(d.page(0).unwrap().id, p5);
        assert_eq!(d.page(0).unwrap().source, new);
        assert_eq!(order(&d), vec![0, 1, 2, 3, 4, 5]);
        // The rotation is the page's /Rotate, which a save writes as is.
        assert_eq!(d.page(0).unwrap().rotation, Rotation::R90);
        // Undo still reaches the pre-save version over the old source.
        d.undo().unwrap();
        assert_eq!(d.page(0).unwrap().source, old_primary);
        assert_eq!(d.page(0).unwrap().index, 5);
        assert!(d.is_dirty());
    }

    /// One Letter page turned `degrees` by its `/Rotate`, or by its
    /// parent's when `inherited`, as a source in `store`.
    fn turned(store: &Store, own: Option<i64>, parent: Option<i64>) -> SourceInfo {
        let mut f = crate::forms::fixture::Form::new(1);
        let page = f.pages[0];
        let up = f.dict(page).get(b"Parent").unwrap().as_reference().unwrap();
        for (at, degrees) in [(page, own), (up, parent)] {
            if let Some(degrees) = degrees {
                f.dict(at).set("Rotate", degrees);
            }
        }
        let kind = BlobKind::file(Path::new("turned.pdf"), None);
        SourceInfo {
            src: store.open(f.bytes().into(), kind).unwrap(),
            name: "turned.pdf".into(),
            derived: None,
        }
    }

    /// `d` saved, incrementally or by a rewrite, and rebased onto the
    /// saved file, which it returns.
    fn save_and_rebase(d: &mut Document, incremental: bool) -> Src {
        let snap = d.snapshot();
        let bytes = if incremental {
            crate::incremental::incremental(&snap)
        } else {
            crate::save::rewrite(&snap)
        }
        .unwrap();
        let kind = BlobKind::file(Path::new("saved.pdf"), None);
        let src = d.store().open(bytes.into(), kind).unwrap();
        d.rebase_to_saved(SourceInfo {
            src: src.clone(),
            name: "saved.pdf".into(),
            derived: None,
        });
        src
    }

    const LANDSCAPE: PageSize = PageSize {
        width: 792.0,
        height: 612.0,
    };

    /// A page from a file that turns it has that turn as its rotation,
    /// with nothing for PDFium to add, and shows turned.
    #[test]
    fn an_inserted_page_keeps_its_own_rotate() {
        let mut d = new_doc(1, &[], Box::new(FixedCount(10)));
        let s = d.add_source(turned(d.store(), Some(90), None));
        d.apply(
            Edit::InsertPages {
                source: s,
                range: 0..1,
                before: None,
                outline: None,
                annots: Vec::new(),
            },
            "insert",
        )
        .unwrap();
        let p = *d.page(1).unwrap();
        assert_eq!(p.rotation, Rotation::R90);
        assert_eq!(d.render_delta(&p), Rotation::R0);
        assert_eq!(d.page_size(&p), LANDSCAPE);
        assert_eq!(d.geom(&p).rotate, Rotation::R90);
        // Turned again, it's turned from there, and PDFium adds the turn.
        d.apply(
            Edit::RotatePages {
                ids: vec![p.id],
                by: Rotation::R90,
            },
            "rotate",
        )
        .unwrap();
        let p = *d.page(1).unwrap();
        assert_eq!(p.rotation, Rotation::R180);
        assert_eq!(d.render_delta(&p), Rotation::R90);
        assert_eq!(d.snapshot().render_delta(&p), Rotation::R90);
        assert_eq!(d.page_size(&p).width, 612.0);
    }

    /// A page's rotation is its `/Rotate`: a save writes it as it is, and
    /// the document rebased onto the saved file keeps it, with no turn
    /// left for PDFium to add.
    #[test]
    fn a_rotation_survives_a_save() {
        for incremental in [false, true] {
            let mut d = new_doc(2, &[], Box::new(FixedCount(10)));
            rotate(&mut d, 0, "rotate");
            let saved = save_and_rebase(&mut d, incremental);
            assert_eq!(saved.geom(0).rotate, Rotation::R90, "{incremental}");
            assert_eq!(saved.geom(1).rotate, Rotation::R0, "{incremental}");
            let p = *d.page(0).unwrap();
            assert_eq!(p.rotation, Rotation::R90, "{incremental}");
            assert_eq!(d.render_delta(&p), Rotation::R0, "{incremental}");
            assert_eq!(d.page_size(&p), LANDSCAPE, "{incremental}");
        }
    }

    /// A page turned upright is upright in the saved file whatever its
    /// parent's `/Rotate`: an incremental save leaves it under its parent,
    /// so it says 0. A page that's only marked keeps the `/Rotate` it has,
    /// its own or its parent's, a 135 too, which reads as 90.
    #[test]
    fn a_page_turned_upright_stays_upright_through_a_save() {
        let square = crate::annot::AnnotKind::Square {
            rect: crate::annot::Box2 {
                x: 100.0,
                y: 100.0,
                w: 50.0,
                h: 40.0,
            },
        };
        // The page's own /Rotate, its parent's, and the turn sheaf adds.
        let cases = [
            (None, Some(90), Some(Rotation::R270)),
            (Some(90), Some(180), Some(Rotation::R270)),
            (Some(0), Some(90), None),
            (Some(135), Some(180), None),
        ];
        for (own, parent, by) in cases {
            for incremental in [false, true] {
                let case = format!("{own:?} under {parent:?}, turned {by:?}, {incremental}");
                let store = Store::new();
                let primary = turned(&store, own, parent);
                let mut d = Document::new(store, primary, &[], Box::new(FixedCount(10)));
                let page = id_at(&d, 0);
                let was = d.page(0).unwrap().rotation;
                let edit = match by {
                    Some(by) => Edit::RotatePages {
                        ids: vec![page],
                        by,
                    },
                    None => Edit::AnnotAdd {
                        page,
                        annot: mark(0, square.clone()),
                    },
                };
                d.apply(edit, "edit").unwrap();
                let want = d.page(0).unwrap().rotation;
                assert_eq!(want, was.plus(by.unwrap_or(Rotation::R0)), "{case}");
                let saved = save_and_rebase(&mut d, incremental);
                assert_eq!(saved.geom(0).rotate, want, "{case}");
                assert_eq!(d.page(0).unwrap().rotation, want, "{case}");
                if incremental && by.is_none() {
                    let pdf = lopdf::Document::load_mem(saved.bytes()).unwrap();
                    let dict = pdf.get_dictionary(pdf.get_pages()[&1]).unwrap();
                    let rotate = dict.get(b"Rotate").and_then(lopdf::Object::as_i64).ok();
                    assert_eq!(rotate, own, "{case}: the page's own /Rotate");
                }
            }
        }
    }

    /// A mark on a page sheaf turned shows where it did after a save, and
    /// the saved file has it where the model does: marks are in PageSpace,
    /// which neither the save nor the rebase moves.
    #[test]
    fn a_mark_on_a_rotated_page_stays_put_through_a_save() {
        let rect = crate::annot::Box2 {
            x: 100.0,
            y: 650.0,
            w: 50.0,
            h: 40.0,
        };
        for incremental in [false, true] {
            let mut d = new_doc(1, &[], Box::new(FixedCount(10)));
            rotate(&mut d, 0, "rotate");
            let page = id_at(&d, 0);
            let annot = mark(0, AnnotKind::Square { rect });
            d.apply(Edit::AnnotAdd { page, annot }, "add").unwrap();
            let shown = |d: &Document| {
                let AnnotKind::Square { rect } = d.state().annots[&page][0].kind else {
                    unreachable!("the square");
                };
                d.geom(d.page(0).unwrap()).to_view().apply_box(rect)
            };
            let before = shown(&d);
            let saved = save_and_rebase(&mut d, incremental);
            assert_eq!(shown(&d), before, "{incremental}");
            let pdf = lopdf::Document::load_mem(saved.bytes()).unwrap();
            let dict = pdf.get_dictionary(pdf.get_pages()[&1]).unwrap();
            let annots = dict.get(b"Annots").unwrap().as_array().unwrap();
            let (_, annot) = pdf.dereference(&annots[0]).unwrap();
            let r: Vec<f32> = annot
                .as_dict()
                .unwrap()
                .get(b"Rect")
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_float().unwrap() as f32)
                .collect();
            let centre = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
            let want = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
            assert!(
                (centre.0 - want.0).abs() < 1.0 / 64.0 && (centre.1 - want.1).abs() < 1.0 / 64.0,
                "{incremental}: /Rect {r:?}"
            );
        }
    }

    /// A save re-expresses the version it wrote, so the versions made from
    /// it before (undone, then saved) keep their own states: their prims
    /// are from it as it was.
    #[test]
    fn a_save_keeps_what_was_undone_before_it() {
        let mut d = new_doc(3, &[], Box::new(FixedCount(1)));
        rotate(&mut d, 0, "a");
        rotate(&mut d, 1, "b");
        let (b, want_b) = (d.current_version(), d.state().clone());
        d.undo().unwrap();
        rotate(&mut d, 2, "c");
        let (c, want_c) = (d.current_version(), d.state().clone());
        assert!(d.versions[&b].state.is_none(), "b's snapshot was dropped");
        d.undo().unwrap();
        d.rebase_to_saved(SourceInfo::blank(d.store(), 3));
        rotate(&mut d, 0, "d");
        d.jump(b).unwrap();
        assert_eq!(*d.state(), want_b);
        d.jump(c).unwrap();
        assert_eq!(*d.state(), want_c);
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
            turn: Rotation::R0,
            name: String::new(),
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
        d.undo().unwrap();
        assert_eq!(d.state().annots_on(p0).unwrap().len(), 1);
        assert_eq!(d.state().annots_on(p2).unwrap().len(), 1);
    }

    #[test]
    fn a_group_runs_by_page_then_down_the_page_as_it_shows() {
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
        let order = |d: &Document| -> Vec<(usize, f32)> {
            d.group_members(&Group::Tickmark("✓".into()))
                .iter()
                .map(|(i, _, a)| (*i, a.page_bounds().y))
                .collect()
        };
        // PageSpace is y up: the higher one is first.
        assert_eq!(order(&d), [(0, 500.0), (1, 300.0), (1, 100.0)]);
        assert_eq!(
            d.snapshot()
                .group_members(&Group::Tickmark("✓".into()))
                .len(),
            3
        );
        // Turned, the page shows them side by side, the lower one left.
        d.apply(
            Edit::RotatePages {
                ids: vec![p1],
                by: Rotation::R90,
            },
            "rotate",
        )
        .unwrap();
        assert_eq!(order(&d), [(0, 500.0), (1, 100.0), (1, 300.0)]);
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
            turn: Rotation::R0,
            name: String::new(),
        }
    }

    /// Text the built-in fonts can't draw (M10.5 proposal 22): a FreeText
    /// in Japanese draws a `?` for each character, its `/Contents` keeps
    /// the text exactly, and the reader is told once however many such
    /// edits come. A header/footer is read the same way.
    #[test]
    fn cjk_text_draws_question_marks_and_is_told_once() {
        use crate::annot::{AnnotKind, Box2};
        let mut d = doc();
        let page = id_at(&d, 0);
        let text = "日本語のメモ";
        let mut note = mark(
            0,
            AnnotKind::FreeText {
                rect: Box2 {
                    x: 10.0,
                    y: 10.0,
                    w: 200.0,
                    h: 40.0,
                },
                size: 12.0,
            },
        );
        note.contents = text.into();
        let add = |d: &mut Document, annot: Annot| {
            d.apply(Edit::AnnotAdd { page, annot }, "add").unwrap()
        };
        let first = add(&mut d, note.clone());
        assert!(first.unencodable);
        assert!(d.unencodable_notice(&first).is_some());
        let second = add(&mut d, note);
        assert!(second.unencodable);
        assert_eq!(d.unencodable_notice(&second), None, "told once");
        let plain = add(&mut d, square(0));
        assert!(!plain.unencodable);

        let written = &d.state().annots[&page][0];
        let mut pdf = lopdf::Document::with_version("1.7");
        let main = crate::annot::to_pdf(written, &mut pdf).main;
        let utf16: Vec<u8> = [0xFE, 0xFF]
            .into_iter()
            .chain(text.encode_utf16().flat_map(u16::to_be_bytes))
            .collect();
        assert_eq!(main.get(b"Contents").unwrap().as_str().unwrap(), utf16);
        let ap = main.get(b"AP").unwrap().as_dict().unwrap();
        let ap = ap.get(b"N").unwrap().as_reference().unwrap();
        let content = &pdf.get_object(ap).unwrap().as_stream().unwrap().content;
        let drawn = b"(??????) Tj";
        assert!(
            content.windows(drawn.len()).any(|w| w == drawn),
            "{}",
            String::from_utf8_lossy(content)
        );

        let mut hf = crate::stamps::HeaderFooter::default();
        hf.texts[1] = "<<file>>".into();
        hf.file = "報告.pdf".into();
        let set = |d: &mut Document, hf: &crate::stamps::HeaderFooter| {
            let header_footer = Some(hf.clone());
            d.apply(Edit::SetHeaderFooter { header_footer }, "hf")
                .unwrap()
        };
        assert!(set(&mut d, &hf).unencodable);
        hf.file = "report.pdf".into();
        hf.texts[2] = "Page <<1>> of <<N>> – <<date>>".into();
        assert!(!set(&mut d, &hf).unencodable);
    }

    pub(super) fn square(id: u64) -> Annot {
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

    pub(super) fn connector(id: u64, partner: u64) -> Annot {
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
    fn bookmark_styles_come_from_the_file() {
        use lopdf::{Object, dictionary};
        let mut f = crate::forms::fixture::Form::new(3);
        let item = |title: &str, color: Option<f32>, flags: i64| {
            let mut d = dictionary! { "Title" => Object::string_literal(title), "F" => flags };
            if let Some(blue) = color {
                d.set("C", vec![0.into(), 0.into(), blue.into()]);
            }
            d
        };
        crate::forms::fixture::outline_tree(
            &mut f,
            vec![
                (0, item("A", Some(1.0), 2)),
                (1, item("A1", Some(1.0), 0)),
                (1, item("A2", None, 0)),
                (0, item("B", Some(1.0), 1)),
                (0, item("C", None, 0)),
            ],
        );
        let store = Store::new();
        let src = store
            .open(f.bytes().into(), BlobKind::file(Path::new("a.pdf"), None))
            .unwrap();
        let outline = crate::open::outline(&src);
        let primary = SourceInfo {
            src,
            name: "a.pdf".into(),
            derived: None,
        };
        let d = Document::new(store, primary, &outline, Box::new(FixedCount(5)));
        let flat = d.state().outline_flat();
        let by = |t: &str| flat.iter().find(|(_, n)| n.title == t).unwrap().1.clone();
        assert!(by("A").style.bold);
        assert_eq!(by("A1").color, Some([0, 0, 255]));
        assert_eq!(by("A2").color, None);
        assert!(by("B").style.italic && by("B").color == Some([0, 0, 255]));
        assert!(by("C").color.is_none() && by("C").style.is_plain());
        // Read, not edited: nothing to save.
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
        let sid = d.add_source(SourceInfo::blank(d.store(), 3));
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
        d.undo().unwrap();
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
        let sid = d.add_source(SourceInfo::blank(d.store(), 1));
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
            turn: Rotation::R0,
            name: String::new(),
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
        d.undo().unwrap();
        assert_eq!(d.state().annots_on(p1).unwrap().len(), 1);
    }

    #[test]
    fn a_batch_that_puts_things_back_pushes_nothing() {
        let mut d = doc();
        let n = d.version_count();
        let (p0, p1) = (id_at(&d, 0), id_at(&d, 1));
        let rotate = |by| Edit::RotatePages { ids: vec![p0], by };
        d.apply_batch(vec![rotate(Rotation::R90), rotate(Rotation::R270)], "spin")
            .unwrap();
        let to = |before| Edit::MovePages {
            ids: vec![p0],
            before,
        };
        d.apply_batch(vec![to(None), to(Some(p1))], "there and back")
            .unwrap();
        d.apply_batch(
            vec![
                Edit::SetHeaderFooter {
                    header_footer: Some(header("Draft".into())),
                },
                Edit::SetHeaderFooter {
                    header_footer: None,
                },
            ],
            "header",
        )
        .unwrap();
        let roman = crate::labels::LabelSpec {
            start: p0,
            style: crate::labels::Style::LowerRoman,
            prefix: String::new(),
            first: 1,
        };
        d.apply_batch(
            vec![
                Edit::SetPageLabels {
                    ranges: vec![roman],
                },
                Edit::SetPageLabels { ranges: Vec::new() },
            ],
            "labels",
        )
        .unwrap();
        assert!(!d.can_undo());
        assert_eq!(d.version_count(), n);
    }

    #[test]
    fn a_batch_keeps_each_slot_s_change_on_its_last_step() {
        let mut d = doc();
        let before = d.state().pages.clone();
        let rotate = |d: &Document, i| Edit::RotatePages {
            ids: vec![id_at(d, i)],
            by: Rotation::R90,
        };
        d.apply_batch(vec![rotate(&d, 0), rotate(&d, 1)], "rotate two")
            .unwrap();
        let steps = &d.versions[&d.cursor].steps;
        assert_eq!(steps.len(), 2, "each edit stays");
        assert!(steps[0].prims.is_empty());
        let [p] = &steps[1].prims[..] else {
            panic!("one prim: {:?}", steps[1].prims)
        };
        assert_eq!(p.key, Key::Pages);
        assert!(same(p.before.as_ref(), Some(&Val::Pages(before))));
        assert!(same(p.after.as_ref(), d.state().get(&Key::Pages).as_ref()));
    }

    #[test]
    fn a_batch_describes_each_edit_against_the_document_before_it() {
        let mut d = doc();
        // Pages 3 and 4 go first, which makes page 2 the 4th.
        let (first, second) = (id_at(&d, 0), id_at(&d, 1));
        d.apply_batch(
            vec![
                Edit::MovePages {
                    ids: vec![id_at(&d, 2), id_at(&d, 3)],
                    before: Some(first),
                },
                Edit::DeletePages { ids: vec![second] },
            ],
            "move and delete",
        )
        .unwrap();
        let last = d.timeline().pop().unwrap();
        assert_eq!(last.edits, ["Move pages 3, 4", "Delete page 4"]);
    }

    // ----- OCR text layers ---------------------------------------------------

    pub(super) fn tl(text: &str) -> Arc<TextLayer> {
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
            &crate::geom::Geom::read(None, None, 0),
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
        d.undo().unwrap();
        assert!(d.state().text_layers.is_empty());
        d.redo().unwrap();
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
        let other = d.add_source(SourceInfo::blank(d.store(), 2));
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
        d.undo().unwrap();
        assert!(d.state().text_layers.is_empty());
        assert!(!d.job_is_current(job));
        assert!(d.job_is_undone(job));
        d.redo().unwrap();
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
        d.undo().unwrap();
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
        d.undo().unwrap();
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

    /// A version names one state, so a job's later batch makes another
    /// version in place of the one it amends.
    #[test]
    fn an_ocr_amend_gives_the_cursor_a_new_version() {
        let mut d = doc();
        let parent = d.current_version();
        let job = d.begin_job();
        let ocr = |d: &mut Document, page: usize| {
            d.apply_in_job(
                job,
                Edit::SetTextLayers {
                    layers: vec![(id_at(d, page), tl("a"))],
                },
                "ocr",
            )
            .unwrap();
        };
        ocr(&mut d, 0);
        let v1 = d.current_version();
        ocr(&mut d, 1);
        let v2 = d.current_version();
        assert_ne!(v2, v1);
        assert!(d.jump(v1).is_err(), "the amended version is gone");
        assert!(d.edge(v1).is_none());
        let ids: Vec<VersionId> = d.timeline().iter().map(|view| view.id).collect();
        assert_eq!(ids, [parent, v2]);
        assert!(d.job_is_current(job));
        assert_eq!(d.undo().unwrap(), Some("ocr"));
        assert_eq!(d.current_version(), parent);
        assert_eq!(d.redo().unwrap(), Some("ocr"));
        assert_eq!(d.current_version(), v2);
        assert_eq!(layer_texts(&d).len(), 2);
    }

    /// A save expresses the version anew over the saved file, so it's
    /// another version, in the same place. The opened one too.
    #[test]
    fn a_save_gives_the_cursor_a_new_version() {
        let mut d = doc();
        let opened = d.current_version();
        d.rebase_to_saved(SourceInfo::blank(d.store(), 6));
        let root = d.current_version();
        assert_ne!(root, opened);
        assert!(d.jump(opened).is_err());
        rotate(&mut d, 0, "rotate");
        let old = d.current_version();
        d.rebase_to_saved(SourceInfo::blank(d.store(), 6));
        let new = d.current_version();
        assert_ne!(new, old);
        assert!(d.jump(old).is_err(), "the old id doesn't resolve");
        assert!(!d.is_dirty());
        let saved: Vec<VersionId> = d
            .timeline()
            .iter()
            .filter(|view| view.saved)
            .map(|view| view.id)
            .collect();
        assert_eq!(saved, [new]);
        assert_eq!(d.undo().unwrap(), Some("rotate"));
        assert_eq!(d.current_version(), root);
        assert!(d.is_dirty());
        assert_eq!(d.redo().unwrap(), Some("rotate"));
        assert_eq!(d.current_version(), new);
        assert!(!d.is_dirty());
    }

    /// A job whose batches put back all they did leaves no version: the
    /// parent is current again, as if the job never ran, even when its
    /// snapshot was dropped. A file registered before it waits for the
    /// next version again.
    #[test]
    fn a_job_that_nets_to_nothing_leaves_no_version() {
        let mut d = new_doc(3, &[], Box::new(FixedCount(1)));
        rotate(&mut d, 2, "rotate");
        let (parent, want) = (d.current_version(), d.state().clone());
        let n = d.version_count();
        let job = d.begin_job();
        let p0 = id_at(&d, 0);
        let s = d.add_source(SourceInfo::blank(d.store(), 1));
        d.apply_in_job(
            job,
            Edit::SetTextLayers {
                layers: vec![(p0, tl("a"))],
            },
            "ocr",
        )
        .unwrap();
        let v = d.current_version();
        assert_eq!(d.edge(v).unwrap().1, [s]);
        assert!(d.versions[&parent].state.is_none(), "trimmed");
        d.apply_in_job(job, Edit::ClearTextLayers { ids: vec![p0] }, "ocr")
            .unwrap();
        assert_eq!(d.current_version(), parent);
        assert_eq!(d.version_count(), n);
        assert!(d.jump(v).is_err());
        assert!(!d.can_redo());
        assert!(!d.job_is_current(job));
        assert_eq!(*d.state(), want);
        rotate(&mut d, 0, "rotate");
        assert_eq!(d.edge(d.current_version()).unwrap().1, [s]);
    }

    /// An amend takes the sources added since the version it joins, and a
    /// batch that changes nothing leaves that version as it is.
    #[test]
    fn a_job_s_amend_takes_its_sources_and_its_no_op_changes_nothing() {
        let mut d = doc();
        let job = d.begin_job();
        let ocr = |d: &mut Document, page: usize, label: &str| {
            d.apply_in_job(
                job,
                Edit::SetTextLayers {
                    layers: vec![(id_at(d, page), tl("a"))],
                },
                label,
            )
            .unwrap();
        };
        ocr(&mut d, 0, "ocr");
        let s = d.add_source(SourceInfo::blank(d.store(), 1));
        ocr(&mut d, 1, "ocr");
        let v = d.current_version();
        let (steps, sources) = d.edge(v).unwrap();
        assert_eq!((steps.len(), sources), (2, &[s][..]));
        d.apply_in_job(job, Edit::SetTextLayers { layers: vec![] }, "other")
            .unwrap();
        assert_eq!(d.current_version(), v);
        assert_eq!(d.undo_label(), Some("ocr"));
        assert_eq!(d.edge(v).unwrap().0.len(), 2, "no step added");
    }

    /// Pruning past `MAX_VERSIONS` can make a job's version the first one,
    /// with no parent to amend it under: the job's next batch makes a
    /// version after it.
    #[test]
    fn a_job_s_version_pruned_to_the_first_is_not_amended() {
        let mut d = new_doc(3, &[], Box::new(FixedCount(1)));
        for i in 1..crate::history::MAX_VERSIONS {
            rotate(&mut d, i % 3, "rotate");
        }
        d.rebase_to_saved(SourceInfo::blank(d.store(), 3));
        let opened = d.timeline()[0].id;
        d.jump(opened).unwrap();
        let job = d.begin_job();
        let ocr = |d: &mut Document, page: usize| {
            d.apply_in_job(
                job,
                Edit::SetTextLayers {
                    layers: vec![(id_at(d, page), tl("a"))],
                },
                "ocr",
            )
            .unwrap();
        };
        // One version too many: the saved branch goes, and the job's
        // version becomes the first.
        ocr(&mut d, 0);
        let v = d.current_version();
        assert_eq!(d.version_count(), 1);
        assert!(!d.can_undo());
        ocr(&mut d, 1);
        assert_ne!(d.current_version(), v);
        assert_eq!(d.undo().unwrap(), Some("ocr"));
        assert_eq!(d.current_version(), v);
        assert_eq!(layer_texts(&d).len(), 1);
    }

    #[test]
    fn rebase_clears_text_layers() {
        let mut d = doc();
        set(&mut d, &[0], "a");
        d.rebase_to_saved(SourceInfo::blank(d.store(), 6));
        assert!(d.state().text_layers.is_empty());
        assert!(!d.is_dirty());
        // The version before the save still carries its layer, so undoing
        // across the save re-emits it against the old source.
        d.undo().unwrap();
        assert!(d.state().text_layers.is_empty(), "pre-OCR version");
        d.redo().unwrap();
        assert!(d.state().text_layers.is_empty(), "rebased version");
    }

    #[test]
    fn history_counts_the_layer_a_step_holds() {
        let mut d = doc();
        let layer = tl(&"x".repeat(100_000));
        let p0 = id_at(&d, 0);
        d.apply(
            Edit::SetTextLayers {
                layers: vec![(p0, layer.clone())],
            },
            "ocr",
        )
        .unwrap();
        let stats = d.stats();
        assert!(stats.step_bytes >= layer.byte_estimate());
        assert!(
            stats.step_bytes < 2 * layer.byte_estimate(),
            "counted once, though the edit holds it too"
        );
        // The snapshot shares the layer: it adds only the map's nodes.
        assert!(*stats.snapshot_bytes.last().unwrap() < 4 << 10);
    }

    #[test]
    fn an_added_mark_costs_its_page_not_the_document() {
        let marks = (0..2000u32)
            .map(|i| (i, (1..=10).map(|j| square(u64::from(i) * 10 + j)).collect()))
            .collect();
        let facts = Facts {
            marks,
            ..Default::default()
        };
        let mut d = opened_doc(2000, facts, Box::new(FixedCount(10)));
        let page = id_at(&d, 1000);
        d.apply(
            Edit::AnnotAdd {
                page,
                annot: square(0),
            },
            "add",
        )
        .unwrap();
        // 256 bytes for each of the document's 20,000 marks was 5 MB.
        let stats = d.stats();
        assert!(stats.step_bytes < 4 << 10, "{}", stats.step_bytes);
        let snapshot = *stats.snapshot_bytes.last().unwrap();
        assert!(snapshot < 16 << 10, "{snapshot}");
    }

    /// `bytes` as the file `name`, parsed into `store`.
    fn file(store: &Store, bytes: Vec<u8>, name: &str) -> SourceInfo {
        let kind = BlobKind::file(Path::new(name), None);
        SourceInfo {
            src: store.open(bytes.into(), kind).unwrap(),
            name: name.into(),
            derived: None,
        }
    }

    /// All of `info`'s pages inserted at the end, as a new version.
    fn insert_all(d: &mut Document, info: SourceInfo) -> SourceId {
        let count = info.src.page_count();
        let source = d.add_source(info);
        d.apply(
            Edit::InsertPages {
                source,
                range: 0..count,
                before: None,
                outline: None,
                annots: Vec::new(),
            },
            "insert",
        )
        .unwrap();
        source
    }

    #[test]
    fn an_undone_insert_counts_its_source_until_redo() {
        let mut d = doc();
        let primary = d.store().bytes();
        let blank = |d: &Document| SourceInfo::blank(d.store(), 2);
        let info = blank(&d);
        let sid = insert_all(&mut d, info);
        let len = d.source(sid).unwrap().src.bytes().len();
        let stats = d.stats();
        assert_eq!(stats.undone_source_bytes, 0);
        assert_eq!(stats.blob_bytes, d.store().bytes());
        assert_eq!(stats.blob_bytes, primary + len);
        d.undo().unwrap();
        let stats = d.stats();
        assert_eq!(stats.undone_source_bytes, len);
        assert_eq!(stats.blob_bytes, primary + len, "undo frees nothing");
        assert_eq!(
            stats.total_bytes(),
            stats.snapshot_bytes.iter().sum::<usize>() + stats.step_bytes + len
        );
        d.redo().unwrap();
        assert_eq!(d.stats().undone_source_bytes, 0);
        // Undone again: the same bytes registered for the next version,
        // then inserted on the line, share its blob.
        d.undo().unwrap();
        let waiting = d.add_source(blank(&d));
        assert_eq!(d.stats().undone_source_bytes, 0);
        d.apply(
            Edit::InsertPages {
                source: waiting,
                range: 0..2,
                before: None,
                outline: None,
                annots: Vec::new(),
            },
            "insert",
        )
        .unwrap();
        assert_ne!(waiting, sid);
        let stats = d.stats();
        assert_eq!(stats.undone_source_bytes, 0);
        assert_eq!(stats.blob_bytes, primary + len, "one blob");
    }

    #[test]
    fn pruning_an_undone_400_page_scan_frees_its_bytes() {
        use sheaf_image::io::{PageImage, Pixels};
        let mut d = new_doc(3, &[], Box::new(FixedCount(1)));
        let rasters: Vec<PageImage> = (0..400u32)
            .map(|i| PageImage {
                width: 8,
                height: 8,
                dpi: (72.0, 72.0),
                rotate: 0,
                pixels: Pixels::Gray((0..64).map(|j| (i * 7 + j) as u8).collect()),
            })
            .collect();
        let scan = crate::images::to_pdf(&rasters, crate::images::PageFit::LETTER).unwrap();
        let (len, before) = (scan.len(), d.store().bytes());
        let info = file(d.store(), scan, "scan.pdf");
        let sid = insert_all(&mut d, info);
        let id = d.source(sid).unwrap().src.id();
        assert_eq!((d.page_count(), d.store().bytes()), (403, before + len));
        d.undo().unwrap();
        assert_eq!(d.stats().undone_source_bytes, len);
        // One edit branches off; past `MAX_VERSIONS`, the branch goes.
        for i in 0..crate::history::MAX_VERSIONS {
            rotate(&mut d, i % 3, "rotate");
        }
        assert!(d.source(sid).is_none(), "its registration went with it");
        assert_eq!(d.store().bytes(), before);
        assert!(d.store().get(id).is_none());
        let stats = d.stats();
        assert_eq!((stats.undone_source_bytes, stats.blob_bytes), (0, before));
    }

    #[test]
    fn the_root_advancing_keeps_its_sources() {
        let mut d = new_doc(3, &[], Box::new(FixedCount(1)));
        // Unsaved, as a recovered document is: the first version can move.
        d.mark_unsaved();
        let info = SourceInfo::blank(d.store(), 1);
        let gone = insert_all(&mut d, info);
        d.undo().unwrap();
        let info = SourceInfo::blank(d.store(), 2);
        let kept = insert_all(&mut d, info);
        for i in 0..crate::history::MAX_VERSIONS {
            rotate(&mut d, i % 3, "rotate");
        }
        let first = &d.versions[&d.root];
        assert!(!first.steps.is_empty(), "pruned to a later one");
        assert_eq!(first.sources, [d.primary(), kept]);
        assert!(d.source(gone).is_none(), "its branch was pruned");
        assert_eq!(d.page_count(), 5);
        assert!(d.state().pages.iter().all(|p| d.source(p.source).is_some()));
        let bytes = |sid| d.source(sid).unwrap().src.bytes().len();
        assert_eq!(d.store().bytes(), bytes(d.primary()) + bytes(kept));
    }

    /// A save's file stays while it's the primary, though pruning took the
    /// version that held it, and goes when the next save replaces it.
    #[test]
    fn a_pruned_save_s_file_goes_when_the_next_save_replaces_it() {
        let mut d = new_doc(3, &[], Box::new(FixedCount(1)));
        for i in 1..crate::history::MAX_VERSIONS {
            rotate(&mut d, i % 3, "rotate");
        }
        let a4 = crate::generated::blank_pages(595.0, 842.0, 3).unwrap();
        let saved = d.rebase_to_saved(file(d.store(), a4, "saved.pdf"));
        let id = d.source(saved).unwrap().src.id();
        // Back to the first version, and one edit too many: the saved
        // branch goes.
        let opened = d.timeline()[0].id;
        d.jump(opened).unwrap();
        rotate(&mut d, 0, "rotate");
        assert_eq!(d.version_count(), 1);
        assert_eq!(d.primary(), saved);
        assert!(d.store().get(id).is_some());
        d.rebase_to_saved(SourceInfo::blank(d.store(), 3));
        assert!(d.source(saved).is_none());
        assert!(d.store().get(id).is_none());
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
        d.undo().unwrap();
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
        d.undo().unwrap();
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
            turn: Rotation::R0,
            name: String::new(),
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
