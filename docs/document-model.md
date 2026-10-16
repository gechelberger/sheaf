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

DocState             { pages: im::Vector<PageRef>,
                       outline: im::Vector<OutlineNode>,   // tree; targets are PageIds
                       annots: im::HashMap<PageId, im::Vector<Annot>>,  // sheaf's marks
                       removed_comments: ...,   // other tools' comments deleted, by /NM
                       text_layers: ...,        // unsaved OCR text, by page
                       labels, header_footer,
                       fields: im::OrdMap<FieldId, Field>,   // the form
                       widgets: ... }           // the fields' widgets, by page
                     a complete, immutable snapshot of what the user sees.

Document             { sources: HashMap<SourceId, Arc<Source>>,
                       versions: HashMap<VersionId, Version>,  // a tree (see Undo/redo)
                       cursor: VersionId,            // which version is current
                       saved: Option<VersionId> }    // which version is on disk
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

History also keeps what each edit changed (M10.5 slice 1), so a snapshot
dropped to save memory is rebuilt exactly, never by running the edit again:

```
Version   { parent, children, redo_to,   // undo goes to the parent, redo to redo_to
            label, author, at,
            steps: Vec<Step>,            // the edits from the parent
            state: Option<DocState>,     // the snapshot, unless trimmed
            snapshot_bytes, step_bytes,  // what it holds (see Decisions)
            job, saves, damaged }
Step      { edit: Edit,
            line: String,                // what it did, in words
            prims: Arc<[Prim]> }         // what it changed
Prim      { key: Key, before: Option<Val>, after: Option<Val> }
```

- **An edit records what it changed.** It reads the state and writes only
  through a recording context (`prim::Rec`), which makes a `Prim` for
  each slot it wrote: the slot (`key`), the value it held and the value it
  holds now, `None` for absent. The keys are transitional, one per field
  of `DocState` until M10.5 slice 8 replaces them with the records' keys:
  `Pages`, `Outline`, `AnnotsOf(page)`, `TextLayer(page)`,
  `RemovedOf(page)`, `Labels`, `HeaderFooter`, `Field(id)` and
  `WidgetsOf(page)`. Before-values cost nothing: they are the parent's im
  and `Arc` values.
- **A no-op makes no version.** A slot written back to what it held makes
  no prim (the same im nodes or `Arc`, else equal), and a batch's or job's
  prims are coalesced per key into one, from the first before to the last
  after, on the last step that changed the slot. So a batch that rotates a
  page and rotates it back has none, and nothing is pushed. No whole state
  is ever compared.
- **Replay is exact and checked.** Undo, redo and jump rebuild a version
  whose snapshot was trimmed from the nearest ancestor that kept one, by
  applying each version's prims on the way. `DocState::apply` refuses a
  prim whose before isn't what its slot holds (`Mismatch`), and replay
  never runs an edit again, so what an edit reads without recording it
  (source sizes, the forms, random mark ids) can't make it differ. A
  mismatch is a bug: the version is marked damaged and can't be reached,
  the History panel shows "History is damaged at version N", undo and redo
  return `Err(Damaged)` instead of doing nothing, and a debug build
  panics.
- **A VersionId names one state, which never changes.** A job's amend
  (see Text layers) and a save each put a new version, with a new id, in
  the old one's place under the same parent, and the old id stops
  resolving. The one exception until M10.5 slice 4: the `set_initial_*`
  loaders fill in the opened version before the first edit.
- **One `IdAlloc`.** Page, bookmark and field ids come from counters that
  only go up, so an id is never handed out twice, whatever undo does. The
  prims hold the ids an edit drew, so a replay draws none. Mark ids are
  random below 2^53.
- **Lines are made once.** describe runs at apply, against the state just
  before that edit, so the second edit of "move pages 3–4 before 1, then
  delete page 4" says "Delete page 4". The line is stored on the step, and
  the History panel reads it whether or not the parent's snapshot is kept.
- **Saves pin snapshots.** Until M10.5 slice 9 a save re-expresses its
  version over the saved file (`rebase_to_saved`), so that version's
  prims don't lead to it, and those of its children made before the save
  don't start from it. Both keep their snapshots, and nothing is replayed
  across a save.

`model/history_tests.rs` is the proof: over random runs of all 21 kinds
of edit, with batches, jobs, undo, redo, jumps and trims down to one
snapshot, every version rebuilt from its prims equals the state it was
made as, ids included, and each kind's no-op pushes nothing.

Costs and how they're bounded:

- Per-version memory is proportional to what changed. `pages` is an
  `im::Vector`, so a reorder of 7,000 pages copies tree nodes, by one split
  and two joins, not the list. Outline, marks, layers and fields are
  persistent too (`im` crate), so per-page edits don't scale with document
  size. At 7,000 pages and 20,000 marks, adding a mark takes 0.7 ms and
  moving half the pages before the first 1 ms (release; the ignored
  `history_at_7k_pages_and_20k_marks`).
- Sources referenced only by undone versions stay registered for now;
  M10.5 slices 3 and 12 count and free them.

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
    // marks
    AnnotAdd { page: PageId, annot: Annot },
    AnnotUpdate { page: PageId, annot: Annot },
    AnnotDelete { page: PageId, id: AnnotId },
    CommentsDelete { page: PageId, names: Vec<String> },   // other tools' comments
    ConnectorAdd { a: (PageId, Annot), b: (PageId, Annot) },
    // OCR text, page labels, header and footer, rebasing pages onto a
    // derived source (crop, watermark): see `Edit` in model.rs.
    ...
}

impl Document {
    pub fn apply(&mut self, edit: Edit, label: impl Into<String>) -> Result<()>;
    // the label of what was undone, or None with nothing to undo
    pub fn undo(&mut self) -> Result<Option<&str>, Damaged>;
    pub fn redo(&mut self) -> Result<Option<&str>, Damaged>;
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
sheaf's marks (`annot::to_pdf_in`, each with its appearance and its JSON in
`/SheafAnnot`), then cut whatever still points at a page that isn't kept
(`kept::keep_only`: links, named destinations, the open action, form
widgets, the structure tree) so pruning takes it out, and save. A copy
never follows a reference to a page it isn't copying (`save::remap`).
Simple, always correct, O(file). Used for "Save As" and as the fallback.

**Incremental** (M2 slice 3, second): append to the original bytes a new
section containing only changed/new objects, a new xref, and a trailer with
`/Prev`. Preserves the original bytes verbatim (audit-friendly, and what
Acrobat does by default). Requires knowing which objects changed — derived by
diffing `state` against the structure read from the source, not tracked
during editing. lopdf does not write incremental updates, so this is a small
writer of our own on top of lopdf's object serializer. Only possible when
`sources[0]` is the file being saved to; otherwise fall back to rewrite.
Also a rewrite when anything in the file was removed since it was saved (a
page, a mark, a bookmark, OCR text replaced: `Snapshot::removes_content`),
since an incremental save would leave it in the earlier revision.

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
      /// How many of the oldest snapshots to drop, given current stats.
      fn trim(&self, stats: &HistoryStats) -> usize;
  }
  pub struct FixedCount(pub usize);
  pub struct MemoryBudget(pub usize);
  pub struct AllOf(pub Vec<Box<dyn HistoryPolicy>>); // drop until all are satisfied
  ```

  `HistoryStats` reports what each kept snapshot holds that no other
  version does (`snapshot_bytes`: the `im` nodes it copied for the slots
  its version changed), what all versions' steps hold (`step_bytes`: their
  edits, lines and prims' values, each counted once, by the version that
  made it, and an `Arc` once however many of its prims put it), and source
  bytes referenced only by undone versions (an inserted scan that was then
  undone; 0 until M10.5 slice 3). Snapshot and step bytes are computed
  when a version is made, from its prims and the sizes of the maps they
  changed. They are estimates, and one is coarse: the pages, the outline
  and the labels are each one value, and a prim that puts one counts all
  of it, walking it, though `im` shares all but the changed path with the
  version before (about 170 KB to rotate one page of 7,000, about 375 KB
  to rename one bookmark of 2,000). So in a long session of such edits
  the steps alone can pass `MemoryBudget`, which then drops every snapshot
  it may, and undo replays from the nearest version that kept one. M10.5
  slice 8's records leave only the page order, the outline's top level
  and the labels counted this way. Trimming frees snapshot bytes
  only; a version's step bytes go when it is pruned (past `MAX_VERSIONS`,
  and by bytes from M10.5 slice 12). Default:
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

## Text layers (OCR)

Added in M5. An OCR run produces, per page, a `TextLayer`: words with boxes
in the source page's displayed space (the same space annotations use), the
frame the text reads upright in (`upright`, the page's extra rotation at
recognition time) and the engine's name. It lives in
`DocState.text_layers: im::HashMap<PageId, Arc<TextLayer>>`.

- **Presence means unsaved.** A save writes every layer into its page as an
  invisible text layer (text render mode 3 with Tesseract's glyphless
  embedded font, `/Contents` turned into `[q, original…, layer]`), and
  `rebase_to_saved` drops them, exactly as it resets rotation deltas. The
  model never reads layers back: once in the file, PDFium owns the text.
  Older versions keep their layers, so undo across a save still works.
- **Edits**: `SetTextLayers` and `ClearTextLayers`. Delete removes a page's
  layer, Duplicate shares it (same `Arc`), Replace drops it (the page shows
  other content), Move and Rotate leave it alone.
- **Coalesced undo.** Results arrive in batches over minutes, but a run is
  one undo step: `Document::begin_job` returns a `JobId`, and
  `apply_in_job` amends the newest version when that version belongs to
  the job, has nothing after it and was never saved; otherwise it pushes
  and tags the new version. An amend coalesces the run's prims and
  replaces the version with a new `VersionId` under the same parent (an id
  names one state), and a run whose batches put back all they did leaves
  no version. A save or an unrelated edit mid-run splits the run into two
  steps. `job_is_undone` tells the runner to stop rather than put undone
  results back.
- **Search.** PDFium can't see unsaved layers, so Find and Patterns run the
  same literal/regex search over the model's layers
  (`text_layer::search`, `pattern_search`) and merge the hits with PDFium's,
  dropping PDFium's for any page that has a layer. Hits are addressed by
  `PageId`, so inserted pages and reordered pages work too.

## Open questions

- None blocking slice 1.
