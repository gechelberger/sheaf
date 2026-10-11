# Document model and edit history

Status: accepted 2026-10-11. Decides the shape of `sheaf-doc` for M2+.

## The problem

Three things want to look at "the document" and they disagree about what it is:

- **PDFium** (rendering, text, search) sees a *file*: bytes it parsed once.
  Reparsing is cheap for small files and ~100 ms for a 7,000-page one, so we
  don't want every edit to reserialize.
- **lopdf** (writing) sees an *object graph*: dictionaries and streams with
  numbers. Mutating it is how a PDF gets written, but it is a terrible
  structure to edit interactively — nothing has stable identity, and inverse
  operations are hard to get right.
- **The user** sees a *sequence of pages* with bookmarks and markup, and
  expects reorder/rotate/delete/insert to be instant and undoable.

The user's view is the one to model. The other two become *inputs* (sources)
and *outputs* (serialization) of it.

## Core idea: a virtual document over immutable sources

```
Source               an opened PDF file: bytes, a RenderHandle (PDFium worker),
                     page count/sizes. Immutable once opened. Refcounted.

PageRef              { id: PageId, source: SourceId, index: u32, rotation: Rotation }
                     one page of the virtual document, pointing at a page of a source.
                     PageId is a fresh, stable identity (never reused).

DocState             { pages: Arc<Vec<PageRef>>,
                       outline: Arc<Vec<OutlineNode>>,     // tree; targets are PageIds
                       annots: im::HashMap<PageId, Arc<Vec<Annot>>>,   // M3
                       ... }
                     a complete, immutable snapshot of what the user sees.

Document             { sources: HashMap<SourceId, Arc<Source>>,
                       history: Vec<Version { label, state: DocState }>,
                       cursor: usize,            // which version is current
                       saved: Option<usize> }    // which version is on disk
```

**Editing never touches a source or a lopdf object.** An edit is a pure
function `DocState -> Result<DocState>` that builds a new snapshot sharing
everything it didn't change. Reordering 7,000 pages clones one `Vec<PageRef>`
(~110 KB) and nothing else.

**Rendering never waits on an edit.** The viewer displays `state.pages` in
order; a render request for display index *i* becomes
`(sources[pages[i].source], pages[i].index, rotation)`. PDFium is never told
about reorders, deletions, or insertions. Inserting a 400-page PDF means
opening it as a second source and splicing 400 `PageRef`s — milliseconds.

**Saving is a materialization.** Given a `DocState` and its sources, produce
bytes. Two strategies, chosen per save (see below). The snapshot is
immutable and `Send`, so saving runs on a background thread while the user
keeps editing.

## Undo/redo: snapshots, not inverse commands

Three candidates were considered:

| Model | Undo | Cost per edit | Risk |
|---|---|---|---|
| **Inverse commands** (`apply`/`revert` per op) | run the inverse | O(edit) | every op needs a correct inverse; a wrong one silently corrupts state. Deleted pages must be stashed for restore. |
| **Event replay** (base + edit log, state = fold) | drop the last event, re-fold | O(edits) per undo | re-fold cost grows; inserts need their sources kept alive anyway |
| **Snapshots** (persistent state, structural sharing) | move the cursor | O(changed part) | memory per version — bounded by history cap |

Snapshots win. Undo/redo cannot be wrong because there is no inverse logic;
"what is the document" is always one pointer. Grouping is free: a drag that
moves 50 pages pushes one version at the end of the gesture. Dirty tracking
is `cursor != saved`. "Revert to saved" is `cursor = saved`.

Costs and how they're bounded:

- Per-version memory is proportional to what changed, except `pages`, which
  is cloned whole (`Arc<Vec<PageRef>>`, 16 bytes per page). At 7,000 pages
  and a 200-version cap that is ~22 MB worst case. If it ever matters, swap
  `Vec` for `im::Vector` and the clone becomes O(log n); nothing else changes.
- Outline and annotation maps are persistent from the start (`im` crate), so
  per-page markup edits don't scale with document size.
- Sources referenced only by undone versions are freed when the history is
  truncated (new edit after undo) or capped.

What an `Edit` looks like:

```rust
pub enum Edit {
    MovePages { ids: Vec<PageId>, before: Option<PageId> },
    RotatePages { ids: Vec<PageId>, by: Rotation },
    DeletePages { ids: Vec<PageId> },
    InsertPages { source: SourceId, range: Range<u32>, before: Option<PageId>, merge_outline: bool },
    DuplicatePages { ids: Vec<PageId> },
    // outline
    OutlineInsert { parent: Option<OutlineId>, after: Option<OutlineId>, node: OutlineNode },
    OutlineMove { id: OutlineId, parent: Option<OutlineId>, after: Option<OutlineId>, with_pages: bool },
    OutlineEdit { id: OutlineId, title: Option<String>, target: Option<Option<PageId>>, color: Option<Option<Color>> },
    OutlineDelete { id: OutlineId },
    // M3: Annot* ...
}

impl Document {
    pub fn apply(&mut self, edit: Edit, label: impl Into<String>) -> Result<()>;
    pub fn undo(&mut self) -> Option<&str>;   // returns the label of what was undone
    pub fn redo(&mut self) -> Option<&str>;
    pub fn state(&self) -> &DocState;
}
```

Edits address pages and bookmarks by **id**, never by index. Indices are a
view-layer concern and go stale the moment anything moves. This is also what
makes PDFlyer's "move bookmark with its pages" a plain edit: the bookmark
targets a `PageId`; `OutlineMove { with_pages: true }` moves the node and
then moves the pages its subtree targets to sit after the preceding
bookmark's pages. Deleting a page strips (or orphans, by preference) the
bookmarks that targeted it.

## What the viewer changes

- Caches (previews, tiles, sizes) key by `PageId`, not display index, so a
  reorder costs nothing visually.
- `Job` gains `rotation`; PDFium renders rotated bitmaps, so tiles at a
  different rotation are just a cache miss.
- `DocView` holds a `Document` instead of a bare `RenderHandle`; sidebar
  thumbnails become the primary editing surface (drag to reorder,
  multi-select, keyboard rotate/delete).
- Sizes and outline come from `DocState`, not from the worker, after open.

## Saving

Materialization of `(DocState, sources)` → bytes. Both strategies use lopdf
for object serialization.

**Rewrite** (M2 slice 3, first): load `sources[0]` in lopdf, rebuild the page
tree from `state.pages` (copying page object subgraphs from other sources
with renumbering), set `/Rotate` per page, rebuild `/Outlines`, write
annotations (M3), save. Simple, always correct, O(file). Used for "Save As"
and as the fallback.

**Incremental** (M2 slice 3, second): append to the original bytes a new
section containing only changed/new objects, a new xref, and a trailer with
`/Prev`. Preserves the original bytes verbatim (audit-friendly, and what
Acrobat does by default). Requires knowing which objects changed — derived by
diffing `state` against the structure read from the source, not tracked
during editing. lopdf does not write incremental updates, so this is a small
writer of our own on top of lopdf's object serializer. Only possible when
`sources[0]` is the file being saved to; otherwise fall back to rewrite.

Either way the save runs on a background thread from a cloned snapshot. On
completion the saved file is reopened as a fresh single source replacing all
the others, and `saved = cursor`. History is kept (undo across a save works;
the snapshots still reference the old sources, which stay alive until
truncated).

## Non-goals of this design

- Collaborative / concurrent editing. Single writer, single history.
- Editing page *content* in place (M-later). When it comes, a page's edited
  content stream becomes another per-`PageId` persistent map entry and
  rendering of that page goes through a scratch PDF; the history model is
  unchanged.
- Making lopdf's object graph the live model. It stays a serialization
  backend.

## Decisions (2026-10-11)

- **Persistent maps use the `im` crate** from the start.
- **History trimming is a policy trait**, composable:

  ```rust
  pub trait HistoryPolicy {
      /// How many of the oldest versions to drop, given current stats.
      fn trim(&self, stats: &HistoryStats) -> usize;
  }
  pub struct FixedCount(pub usize);
  pub struct MemoryBudget(pub usize);
  pub struct AllOf(pub Vec<Box<dyn HistoryPolicy>>); // drop until all are satisfied
  ```

  `HistoryStats` reports each version's *marginal* bytes — what it doesn't
  share with its neighbour, estimated at push time via `Arc::ptr_eq` per
  field and `im`'s `diff()` — plus source bytes referenced only by undone
  versions (an inserted scan that was then undone). Default:
  `AllOf([FixedCount(200), MemoryBudget(256 MB)])`.
- **Deleting pages cascades to bookmarks that target them**, with a
  non-modal notice ("Deleted 3 pages and 2 bookmarks — Undo"). No modal;
  snapshot undo makes the cascade zero-risk. Rationale: for both "delete a
  section" and "delete a bookmarked page", a retargeted or orphaned
  bookmark would be misleading.
- **Replace Pages is its own edit** that keeps the `PageId` (and so every
  bookmark/link to the slot). This is the workflow where people actually
  want bookmarks preserved, and no automatic retargeting rule gets it right.
- **Set Destination** on a bookmark re-anchors it to the current page. We
  never create orphans ourselves, but files in the wild have dangling
  bookmarks (the outline panel already greys them), so they need a fix path.
- Thumbnails show a bookmark glyph on pages that are bookmark targets, so a
  cascade is visible before the delete.
- `PageRef.rotation` is a **delta** over the source page as PDFium renders
  it (i.e. on top of its own `/Rotate`). Reading originals up front would
  cost a page load per page; the absolute value is only needed at save time,
  where lopdf has the original anyway.
- `im` is MPL-2.0 (file-scoped copyleft); fine as an unmodified dependency
  of an MIT/Apache project.

## Open questions

- None blocking slice 1.
