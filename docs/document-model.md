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
Store                the document's bytes, named by their content: one Blob for
                     the same bytes however often they're interned, and one Src
                     for a Blob while anything holds it. Weak references only.

Blob                 { id: BlobId { hash, len },   // compared byte for byte too
                       bytes: Arc<[u8]>,           // shared with PDFium
                       kind: Pdf { password, origin: File(path) | Memory(Derived) } }

Src = Arc<Source>    a Blob parsed lazily (lopdf's LazyPdf), under one Limits,
                     with the facts read from it once: the page table (each
                     page's Geom, inherited /Resources, /Contents, /Annots),
                     XrefFacts, conformance. Immutable.

SourceInfo           { src: Src, name: PathBuf, derived: Option<Derived> }
                     a registration, which a SourceId names (until M10.5 slice
                     11 makes the BlobId the identity): the same file inserted
                     twice is two registrations of one Src.

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

Document             { store: Arc<Store>,
                       sources: HashMap<SourceId, Arc<SourceInfo>>,
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

**Sources are read lazily** (M10.5 slice 3). A file is read once, into a
blob, and a source parses the blob in place:

- **The open (`Store::source`) reads the cross-reference sections, the
  trailer and the page tree.** Every other object is read when it's first
  asked for, decrypted on its own (a source keeps `/Encrypt`), and kept,
  as is a failure to read it that the bytes alone decide; an object
  stream is decoded once. `XrefFacts` records what a later
  append needs: the newest section's kind and start, `/Size`, the largest
  cross-reference stream number, `/ID` and where the header starts.
- **The page table** numbers pages as PDFium does (a kid with `/Kids` is
  a node, any other a page) and reads the inheritable keys up `/Parent`
  as it does. Each page's `Geom` is its boxes and rotation as PDFium
  sizes them: an empty media box is Letter, the crop box is clipped to the
  media box (one outside it shows nothing), `/Rotate` is whole quarter
  turns, `(degrees / 90) mod 4`, and `/UserUnit` doesn't scale the size.
  The layout's page sizes are `Geom::shown()` from the first paint, and
  equal PDFium's on every corpus page; PDFium isn't asked for sizes. A
  page the tree lists twice is two entries, which share what the page
  holds, as pages naming one `/Annots` array share it. The table ends at
  the root's `/Count` when PDFium counts by it, since PDFium loads no
  page past it.
- **One `Limits`** caps every reader of a source: lopdf's (256 MiB a
  stream decompressed, 1,024 sections, 8,388,607 objects and twice as
  many cross-reference stream entries, references 64 deep), the page
  tree's (1,024 levels, fewer than 0xFFFFF pages) and the form reader's.
  Breaking one, a `/Kids` cycle, a `/Parent` loop or an object that
  doesn't decrypt is a typed error, never a panic, a hang or memory
  without bound; nothing is repaired. So the app doesn't open such a
  file, though PDFium shows some of them (none in the corpus: every
  corpus file PDFium opens opens as a source).
- **A table that can't be read** is rebuilt by scanning, as lopdf's eager
  reader rebuilds it, and every object is read then: `base_sound()` is
  false, and such a file is never appended to. With no `trailer` keyword,
  the newest cross-reference stream that decodes is the trailer, as
  PDFium takes it.
- **Bytes stay in memory.** PDFium opens the blob's own `Arc<[u8]>`
  (`RenderHandle::open_bytes`). A derived copy (cropped, watermarked, a
  header and footer stripped) is a memory blob, written nowhere; blank and
  image pages are read back from the generated cache once, into blobs.
  `LazyPdf` can also read a file by position (`FileBytes`).
- **The open reads every fact from the source** (M10.5 slice 4; see
  Opening a file): its marks, other tools' comments, labels, header and
  footer, form, attachments, layers and outline, as File > Properties
  and the signature check read theirs. Nothing it reads loads the file
  eagerly. The old writers still load their sources eagerly until
  slice 9.

At 7,000 pages the open (the geometry walk, asserted by the 7k gate
harness, `crates/sheaf-ops/tests/gate_7k.rs`) takes 48 ms on the
synthetic fixture and 72–80 ms on the heavy one, against gates of 100 and
250 ms.

## Opening a file (M10.5 slice 4)

A file opens once, the same way in the app, on the command line and in
every other headless caller (`sheaf_doc::open`; `open::file` takes the
three steps in a row):

- **`start` makes the document from its pages.** Its one version holds
  the pages from the page table the source built as it opened, and
  nothing else, so the first page paints at once. It reads nothing of
  its own. (`Document::new` is for a file sheaf has just made, which says
  nothing more about itself: it's complete at once, with only an
  outline.)
- **`finish` reads the rest, on any thread, from the source alone:**
  sheaf's marks and other tools' comments in one pass over the page
  table's annotations (what a mark keeps in this machine's cache is made
  again from the file where it's missing), the form, the page labels,
  the header and footer sheaf drew, attachments, the portfolio schema,
  layers, and the outline with its colors, styles and closed state
  (`/C`, `/F`, a negative `/Count`). Of a page it reads the dictionary
  and the annotations, never its content. Each reader passes over what
  it can't read, so `finish` doesn't fail once the source has opened,
  and reads a tree a node at a time however often the file lists a node
  (a name or number tree, the layers' order, a portfolio's folders, the
  outline), so it ends. It counts the pages of its first pass to the
  progress it's given. The outline's destinations (`/Dest`, named
  destinations, a GoTo action's `/D`) are found through the page table,
  and their views are read as PDFium reads them; the renderer is never
  asked for the outline.
- **`Document::ready(facts)` builds the complete opened version once**
  and puts it in the loading version's place under a new `VersionId`,
  the way a save or a job's amend does. The pages keep their ids, and the
  current and saved versions follow it, so the document isn't dirty and
  there's nothing to undo. It gives back what the view shows beside the
  model: other tools' comments, attachments, the portfolio, layers and
  what sheaf says about the form. A second call is refused.
- **Until then the document is loading.** `apply`, `apply_batch` and
  `apply_in_job` return `NotReady`, making no version and drawing no ids.
  The app greys out what edits the document or writes it out (saves,
  exports, print, split, extract, comment import, page and bookmark
  edits, the markup tools, undo and redo) and every plugin command, says
  "Opening…" in the status bar, with the page the first pass is on, and
  refuses any of them run by its keys with a notice (an insert or a
  replace before it reads the file it was given). A plugin call given
  meanwhile (a command, a panel action, an event) waits: once the file is
  ready, plugins get `opened`, then the calls that waited run, in order,
  each seeing the document as it is then. Only a plugin's newest region
  event waits, and a file closed before it's ready drops what waited,
  saying `closed` only after an `opened` (a recovery opens a file again).
- **There are no late facts.** Everything the file says arrives in the
  one `ready`, so nothing is read after an edit and dropped; a file
  that's slow to read keeps edits waiting longer, with its progress
  shown.

On the heavy 7k fixture, ready (the source's open, `finish` and `ready`,
`open::file` in the 7k gate harness) takes 391–406 ms against a gate of
1 s, asserted; the scanned one's 539–545 ms and 514 MB at its peak, the
474 MB file's bytes and little more.

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
  (see Text layers), a save and the end of a file's opening
  (`Document::ready`) each put a new version, with a new id, in the old
  one's place under the same parent, and the old id stops resolving.
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
  moving half the pages before the first 1 ms (release;
  `history_at_7k_pages_and_20k_marks` in the 7k gate harness,
  `crates/sheaf-ops/tests/gate_7k.rs`).
- Sources referenced only by undone versions are counted and freed. Each
  version owns the registrations made for it, and pruning a version
  drops them; a blob goes when its last registration does.
  `HistoryStats` counts every live blob once (`blob_bytes`) and those only
  versions off the current line registered (`undone_source_bytes`), and
  `MemoryBudget` weighs the second (see Decisions). Pruning is by count
  until M10.5 slice 12 prunes by bytes. Inserting a 400-page scan, undoing
  it and editing past `MAX_VERSIONS` frees its bytes
  (`pruning_an_undone_400_page_scan_frees_its_bytes`).

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
- Sizes come from the sources' page tables (`Geom`) from the first paint,
  and the outline from `DocState`, which the open reads from the source;
  the worker is never asked for either.

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
completion the saved file is read back into the store as a fresh single
source replacing all the others, and `saved = cursor`. History is kept (undo
across a save works; the snapshots still reference the old sources, which
stay registered until the versions holding them are pruned).

## Rendering edited pages: the spikes (M10.5 slice 2)

From M10.5 slice 13 an edited page renders from an **overlay**: an append
section built in memory over the source's own bytes, in which each edited
page is replaced in place under its own object number, so it keeps its
inherited attributes and shared resources. The section is its group's,
and scoped to it (GROUP, below): its root `/Pages` lists only the
group's pages and its AcroForm only their fields, so in the group's
engine a page's index is its place in the group. PDFium loads the base
and the section as two segments through `FPDF_LoadCustomDocument`,
without copying the base, and an edit reopens only its group's engine
(`todos/m10.5-edit-api.md`, §10.1 and §10.3).
Before anything depended on that, slice 2 measured it with three
throwaway spikes against written criteria and set the realizer's and the
engine pool's numbers from the results. The spike code is gone; this
section keeps the criteria, the results and the verdicts.

**How they ran.** Sections came from vendored lopdf's
`IncrementalDocument`: the base's xref kind, `/Prev`, `/ID` kept,
`/Encrypt` repeated and each new object encrypted by `encrypt_object`; a
stale `/XRefStm` and `/DecodeParms` dropped; offsets counted from the
header on a file with leading junk; new objects numbered above `/Size`.
The reference was lopdf's Full write of the same edit, decrypted for an
encrypted base. A two-segment `Read + Seek` reader went to pdfium-render's
`load_pdf_from_reader`. Pages were compared at 1,024 px, pixel for pixel,
with no tolerance, two ways: **appearances only** (each shown annotation
flattened, then the page rendered, as `render_appearances` does), which
the criteria use, and **live** (widgets by `FPDF_FFLDraw`, as the page
view draws them). Machine: AMD EPYC 7302P, 32 threads, release builds,
libpdfium chromium/7881, pdfium-render 0.9.4.

### Spike A: encrypted overlay sections

*Criterion:* overlays on RC4-128, AES-128 and AES-256 bases render
pixel-equal to a decrypted Full write. *Fallback:* a decrypted in-memory
image per source.

The synthetic 7k was encrypted by lopdf (user and owner passwords), pages
0, 3,499 and 6,999 were replaced in place, and the base and section were
loaded through two segments with the password. Three edited and two
untouched pages were compared:

| Algorithm | Pixels differing, appearances / live | Overlay open | Base open | Section |
|---|---|---|---|---|
| RC4-128 (V2) | 0 / 0 on all five | 1.52 ms | 1.49 ms | 2,840 B |
| AES-128 (V4, AESV2) | 0 / 0 | 1.52 ms | 1.49 ms | 3,181 B |
| AES-256 (V5 R6, AESV3) | 0 / 0 | 8.14 ms | 8.09 ms | 3,180 B |

Each edited page differs from the base and each untouched one doesn't,
the section holds none of the edit's text in plain text, and poppler
renders the overlay as it renders the Full write. 50 encrypted corpus
files (the first 75 encrypted rows of the inventory, at most 15 an
algorithm; 30 of them xref-stream bases) pass too: AES-128 15, AES-256
10, RC4-128 11, RC4-128 under V4 6, RC4-40 8. **A passes on all three; no
fallback.** An AES-256 (R6) source pays its key derivation on every
open: 8.1 ms at 7k, against 1.5 ms.

### Spike B: overlays on real files

*Criterion:* per base class, at least 99% of sampled pages pixel-equal
(appearances only) to a Full write, and RSS per engine within the cap the
spike sets. *Fallback:* a failing class realizes as a standalone PDF per
group.

883 bases: the 402-file pool sample (`bench/pool-sample.txt`), all 76
government forms with fields, a seeded 400 of the corpus's other 3,976
SafeDocs forms with fields, and five built bases (the synthetic, heavy and
scanned 7k, the synthetic with leading junk, and the synthetic with a
broken `startxref`). The first, middle and last pages are edited; two
others are left alone. A base is in each class it has: classic table or
xref stream (its newest section's kind, which the overlay writes), hybrid
(`/XRefStm`), linearized, damaged (lopdf rebuilt the xref, or lopdf and
PDFium count different pages), leading junk, encrypted. Times are
medians, base → overlay, of the middle edited page drawn live at
1,024 px; cold is the first render in a fresh engine, warm the next.

| Class | Bases | Pages | Pixel-equal | Open, ms | Cold, ms | Warm, ms | MB per engine, median / p99 |
|---|---|---|---|---|---|---|---|
| classic table | 355 | 1,057 | 100% | 0.23 → 0.24 | 24.9 → 32.9 | 17.3 → 17.7 | 1.20 / 9.63 |
| xref stream | 527 | 1,517 | 99.93% | 0.58 → 0.61 | 24.3 → 29.1 | 14.5 → 14.5 | 1.80 / 13.36 |
| hybrid | 36 | 107 | 100% | 0.44 → 0.42 | 22.1 → 29.4 | 15.8 → 16.4 | 2.74 / 9.09 |
| linearized | 502 | 1,448 | 99.93% | 0.54 → 0.56 | 24.5 → 30.2 | 14.8 → 14.8 | 1.80 / 10.47 |
| damaged | 3 | 6 | 100% | 55.0 → 55.7 | 22.9 → 24.3 | 12.7 → 14.3 | 5.35 / 5.35 |
| leading junk | 2 | 6 | 100% | 1.55 → 1.56 | 24.3 → 24.0 | 15.1 → 15.5 | 5.34 / 5.34 |
| encrypted | 69 | 166 | 99.40% | 0.58 → 0.64 | 22.9 → 28.1 | 15.2 → 14.9 | 1.51 / 10.11 |
| all | 883 | 2,574 | 99.96% | 0.43 → 0.45 | 24.5 → 30.3 | 15.4 → 15.4 | 1.55 / 10.87 |

- The one miss in the xref-stream, linearized and encrypted rows is one
  page: lopdf's write of `pool/safedocs/2932/2932995.pdf` recolours page
  0's logo with no edit at all, and the overlay matches the base. In
  this run no page differed because of the overlay, and live renders
  matched at the same rates. Two pool files are untestable: lopdf finds
  no pages in 0676477 (PDFium 47) and rejects 2289457's xref.
- **RSS per engine** is measured over 24 overlay engines on one base,
  each with its own section and one page rendered, from
  `/proc/self/statm` after `malloc_trim`; the growth is glibc heap,
  linear in the engine count. The pool sample's: median 1.82 MB, p95
  7.74, p99 10.87, max 22.11. **The per-engine cap is 16 MB:** the pool
  sample's p99, rounded up to a multiple of 8 MB. Every class's p99 is
  within it.
- The 7k bases, one at a time:

  | Base | MB per engine | Open, ms | Cold, ms | Warm, ms |
  |---|---|---|---|---|
  | synthetic | 5.34 | 1.52 → 1.50 | 23.0 → 24.1 | 5.63 → 5.97 |
  | heavy | 35.42, over the cap | 8.66 → 9.36 | 738 → 751 | 5.50 → 6.31 |
  | scanned | 7.20 | 3.66 → 3.67 | 37.3 → 63.8 | 16.4 → 17.1 |
  | leading junk | 5.34 | 1.55 → 1.56 | 24.3 → 24.0 | 5.32 → 5.91 |
  | damaged | 5.35 | 55.0 → 55.7 | 22.9 → 24.3 | 5.75 → 5.93 |

  25 MB of the heavy's engine, and nearly all of its cold render, is
  PDFium's form environment loading all 7,000 fields at the first page
  load. Scoped group sections (GROUP, below) remove both.

**The thin classes, again.** That run's damaged and leading-junk rows
are 3 and 2 bases, mostly built: too few for a 99% criterion. A second
run took 90 pool files outside the sample, in inventory order: the first
50 with `%PDF-` past offset 0 (1 to 307 bytes in, 37 of them behind a
UTF-8 byte-order mark) and the first 50 whose `startxref` leads to no
xref (10 are both). Both runs together:

| Class | Bases | Pages | Pixel-equal | MB per engine, median / p99 |
|---|---|---|---|---|
| damaged | 56 | 133 | 80.45% | 0.96 / 5.35 |
| leading junk | 52 | 130 | 100% | 0.11 / 5.34 |

- Leading junk passes, with the section's offsets counted from the
  header (§11.7). 32 of the 50 picks load without a rebuilt xref; 18 are
  damaged too.
- Damaged fails: 26 pages of 24 bases differ, all of them damaged and
  none because of lopdf's writer. On 22 signed files the overlay loses
  the visible signature stamp on page 0 that the Full write shows; on
  one a page left alone loses a seal; on one (encrypted, linearized) the
  edit doesn't show. Five picks were untestable (lopdf finds no
  pages in two and rejects three xrefs).

**B passes on every class but damaged,** and damaged bases realize as a
standalone PDF per group, which §10.1 has for them by design.

### Spike C: render-time box edits

*Criterion:* a crop rendered from the base engine, through
`FPDF_RenderPageBitmapWithMatrix` and a clip, is pixel-equal to the crop
written as `/CropBox` on 100% of the sample. *Fallback:* crops realize
as overlays.

1,267 pages: the first page of 400 pool files; spike B's widget pages of
the forms (165 government, 683 SafeDocs); 16 synthetic pages, four boxes
(plain, off the origin, an offset fractional CropBox, an upside-down
MediaBox with a negative origin) at each `/Rotate`; and three heavy
pages. 1,231 are at `/Rotate` 0, 19 at 90, 4 at 180 and 13 at 270, and
1,046 have widgets. The crop is inset 7.3% left, 11.1% bottom, 5.7% right
and 13.9% top. The matrix is solved in PDFium's own f32 arithmetic so
that it composes to the written page's transform, bit for bit on 3,364
of 3,801 transforms. Two other geometry edits were compared the same way:
the crop with a 90° turn, and the turn alone through today's rotate
parameter.

| Edit | Pixel-equal, live | Appearances only |
|---|---|---|
| crop | 590 (46.60%) | 1,266 (99.92%) |
| crop and a 90° turn | 549 (43.36%) | 1,167 (92.11%) |
| the turn alone | 1,228 (97.00%) | 1,232 (97.31%) |

- `FPDF_FFLDraw` takes an integer rect and a rotate, never a matrix, so
  a matrix render draws no live widgets: 676 of the 1,046 widget pages
  lose them. All 221 pages without widgets are equal both ways.
- Some translations can't be landed exactly in f32: one crop (3 px, one
  level), and 17 (live) to 100 (appearances) with a turn, 99 of those by
  at most one level. A sub-ulp difference resamples a whole image.
- The turn alone isn't the written `/Rotate` on 38 pages live: 32
  transforms f32 can't make exact, 22 of them resampling an image across
  its area (over 1,000 px) and 10 off by one level, and 6 forms whose
  widgets `FPDF_FFLDraw` draws differently under a rotate.
- Edge rounding and annotations outside the box caused no failure.

**C fails: crops, and every box edit, realize as overlays.** No
geometry-only page renders from the base engine.

### The verdicts

**GROUP = 128 pages, scoped.** *Rule:* the largest G whose overlay opens
within 0.5 ms of the bare base and whose engine, with every page of its
group previewed, is within the 16 MB per-engine cap, on all three 7k
fixtures (8 engines a fixture, a header and a watermark on every page of
the group). In place, the heavy's engine is 35–46 MB at every G, so no G
fits. **Scoped**, the section replaces the root `/Pages` with one whose
`/Kids` are the group's pages, in order, and the AcroForm with one whose
`/Fields` are the top-level fields with a widget on those pages, so an
engine's page j is its group's page j and its form loads no other field.
Engine MB, in place → scoped, and the scoped open and cold render:

| G | Synthetic, MB | Heavy, MB | Scanned, MB | Scoped open, ms | Scoped cold, ms |
|---|---|---|---|---|---|
| 8 | 5.01 → 0.94 | 35.16 → 4.02 | 7.19 → 1.82 | 1.39 / 7.72 / 2.02 | 9.0 / 8.8 / 28.5 |
| 16 | 5.11 → 1.05 | 35.48 → 4.41 | 7.75 → 2.39 | 1.39 / 7.78 / 2.05 | 9.0 / 8.9 / 28.3 |
| 32 | 5.32 → 1.28 | 36.20 → 5.21 | 8.88 → 3.54 | 1.36 / 7.70 / 2.06 | 9.2 / 8.9 / 28.3 |
| 64 | 5.75 → 1.73 | 37.63 → 6.77 | 11.18 → 5.87 | 1.42 / 7.76 / 2.12 | 9.1 / 9.2 / 28.7 |
| **128** | 6.62 → 2.68 | 40.44 → 9.89 | 15.74 → 10.51 | 1.49 / 7.86 / 2.16 | 9.1 / 10.1 / 28.3 |
| 256 | 8.44 → 4.57 | 46.27 → 16.32 | 24.91 → 19.80 | 1.60 / 7.90 / 2.28 | 9.1 / 12.8 / 28.2 |

The bare bases open in 1.35–1.42, 8.58–8.73 and 3.59–3.69 ms (synthetic,
heavy, scanned), and in place the cold renders are 24, 740 and 48 ms. A
scoped section of 128 is 55–58 KB. Scoped renders matched in place on
every page previewed, and on the corpus (spike B again) matched the Full
write on 1,982 of 1,982 edited pages over 881 bases, 854 of them form
pages with widgets, live and appearances only. Scoping also spares
PDFium's first `FPDF_LoadPage(k)` in a document, which loads every page
dictionary up to k (20 ms at the synthetic's middle page).

**What scoping gives up.** §10.1 has an edited page keep "its page
index, AcroForm, OCProperties, inherited attributes and shared
resources". A scoped section keeps the last three and not the first two,
which slice 13 builds on:
- **The page tree is flattened, and inconsistent:** each page's
  `/Parent` still names its own node, whose `/Kids` and `/Count` still
  list pages outside the group. Inherited attributes resolve because
  PDFium walks `/Parent`. Slice 13 can instead write each page's
  inheritable keys out and point its `/Parent` at the new root, as §11.6
  does for duplicates.
- **An edited page's index is its place in its group's engine**
  (`PageView.index`), not its base index. Whatever reads an edited page
  from its realization, a render, a size or §10.5's text, goes through
  `PageView`.
- **Destinations into other groups** (a link's `/Dest` or `/GoTo`, a
  named destination) name page objects the group's engine doesn't list,
  so they resolve to no page there. Sheaf reads links and the outline
  from the file through lopdf, not from engines.
- **A field whose widgets span groups** is listed in each group that has
  one of them, and its other widgets are on pages that engine doesn't
  list; a calculation order (`/CO`) or script naming a field outside the
  group finds nothing. Widgets still draw from their appearances and the
  field's value.

**LRU count: 6 group engines, pinned ones included.** *Rule:* in each
pane of the page view (two when the window is split, each scrolled on its
own) the visible pages with `PREFETCH` span at most two groups, and so do
the thumbnails in view, while each shows fewer than GROUP + 1 pages. They
do: two-up at `ZOOM_MIN` in a 2,160 px view shows 48 pages and prefetches
2; the strip shows 10 thumbnails at `THUMB_W` and 27 at its 40 px
minimum. Two groups a pane, two panes and two for the strip: 6. The count
holds the pinned engines (those serving visible pages, §10.3) with the
rest, so it never closes a visible group's engine, and what the pinned
ones leave keeps the groups last scrolled away from: 2 with one pane.
Each visible source's base engine is pinned and outside the count.

**Engine byte cap: 96 MB,** 6 × the 16 MB per-engine cap, pinned engines
included. In the fill below the screen's engines peaked at 48.5 MB
(heavy) and 12.9 MB (synthetic), no group was opened twice, and both
panes' pinned engines stayed open.

**Background budget: 2 engines, 32 MB** (2 × the per-engine cap).
*Rule:* an engine for each of two background consumers walking at once,
such as OCR beside Trim White, a job or a plugin's page scan (a document
runs each of those on its own). One walk in page order never goes back
to a group, so it needs one engine; two taking turns need one each.
Measured on the heavy 7k at 300 dpi, every page behind a scoped group of
128, while the screen holds its 6 engines pinned: two walks taking turns,
one from the first page and one from the middle, 1,024 pages each, opened
14 engines with 2, none twice, at most 16.8 MB of them, in 106.9 s; with
1, they opened 1,538, 1,524 of them a group again, in 136.3 s. Neither
closed any of the screen's. One walk over all 7,000 pages with 2: 364.5 s
(52.1 ms a page), 49 engines opened, none twice, at most 20.6 MB, and 768
renders served by the screen's engines. A third consumer at once shares
the 2 and reopens at every turn, as the 1-engine case shows: 2 is chosen
for two at once, not measured to be enough for more.

**Thumbnail fill rate: 310 pages/s.** *Definition:* 320 px previews
(`PREVIEW_W`) of all 7,000 pages in order, every page behind a scoped
group overlay of 128 with a header, a watermark and the crop in the
section, through the screen's LRU (6 engines, 96 MB) with two panes'
groups pinned; pages over the time spent in engine opens and renders.
*Rule:* the heavy 7k's rate, 318 pages/s, rounded down to a multiple of
10. The synthetic fills at 389. The bare base, through one engine, fills
at 389 (heavy) and 512 (synthetic) pages/s while that engine grows to 338
and 91 MB. The scanned 7k isn't held to the rate: decoding a 1275 × 1650
image a page caps its previews at about 150 pages/s on the bare base.

The rate leaves out building the sections: lopdf took 121 ms a group on
the heavy and 21 ms on the synthetic, and that isn't slice 13's assembly.
Slice 13's strip gate counts its own assembly in the time: at 310
pages/s, the heavy's 22.0 s of opens and renders leave 0.57 s for its 55
groups, about 10 ms a group of 128.

### What the spikes found

- **Widgets draw only through `FPDF_FFLDraw` or a flatten.** A render
  with `FPDF_ANNOT` alone draws none, form environment or not, and
  `FPDF_FFLDraw` takes no matrix.
- **A section numbers its objects above `/Size`** and above every xref
  stream's own number. Numbered from lopdf's `max_id`, a section on
  `pool/safedocs/5801/5801985.pdf` (`max_id` 76, `/Size` 78, the newest
  xref stream 77) put a content stream at 77, and PDFium read it as the
  xref stream.
- **The two-segment reader fills each read across the boundary.**
  pdfium-render hands each of PDFium's block requests to one
  `Read::read`, and PDFium takes any nonzero count as the whole block.
- **What a fresh engine costs:** an open of 1.5 ms (synthetic 7k),
  8.6 ms (heavy), 3.6 ms (scanned) or 55 ms (damaged, the xref rebuilt),
  and about 6.6 ms more on an AES-256 R6 source; then, unscoped, the
  first page load walks the page tree up to it, and the first form page
  loads every field (0.7 s and 25 MB on the heavy).
- **The base engine isn't bounded by the LRU:** after previewing every
  page it holds 91 MB (synthetic) and 338 MB (heavy).
- **A rotation delta carried by the render isn't a written `/Rotate`**
  on 38 of 1,267 pages (`RotatePolicy::KeepBase`, §10.1).
- **Slice 13's realize gates, read early, without the realize:** with
  scoped sections the synthetic's open and cold render take 1.49 + 9.1 =
  10.6 ms, so realizing the page, the section's assembly included, must
  fit in the 4.4 ms left of the gate's 15 ms. The heavy's open and cold
  render take 18.0 ms, 2.9× its warm render in the same run (6.12 ms)
  against ≤ 1.7×, and its bare open alone, 8.6 ms, is about 1.7× its
  warm render.
- **A damaged base's overlay isn't its Full write:** on 22 damaged,
  signed files the overlay loses the signature stamp the Full write
  shows. Damaged bases realize standalone (§10.1).
- **Appearances-only renders need care.** PDFium's `FPDFPage_Flatten`
  writes `/MediaBox` and `/CropBox` from the page's own keys, so a page
  that inherits a box other than letter comes out 612 × 792
  (`forms/safedocs/0163162.pdf` page 2, 419.5 × 595.3), and
  `render_appearances` sizes its render from the flattened page.
  pdfium-render 0.9.4's `PdfPage::flatten` reloads the page under a new
  handle but leaves `boundaries()` on the closed one.

The 7k fixtures and the planning scripts are in `bench/`
(`bench/README.md`). The gate harness, `crates/sheaf-ops/tests/gate_7k.rs`,
prints today's numbers on the three fixtures beside slice 2's baselines.

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
  made it, and an `Arc` once however many of its prims put it), the bytes
  of every blob the store holds, each once (`blob_bytes`), and those of
  the blobs only versions off the current line registered
  (`undone_source_bytes`: an inserted scan that was then undone). Steps
  name sources by registration, so source bytes are never step bytes.
  Snapshot and step bytes are computed when a version is made, from its
  prims and the sizes of the maps they changed. They are estimates, and
  one is coarse: the pages, the outline and the labels are each one value,
  and a prim that puts one counts all of it, walking it, though `im`
  shares all but the changed path with the version before (about 170 KB to
  rotate one page of 7,000, about 375 KB to rename one bookmark of 2,000).
  So in a long session of such edits the steps alone can pass
  `MemoryBudget`, which then drops every snapshot it may, and undo replays
  from the nearest version that kept one. M10.5 slice 8's records leave
  only the page order, the outline's top level and the labels counted this
  way. Trimming frees snapshot bytes only; a version's step bytes, and the
  registrations it holds, go when it is pruned (past `MAX_VERSIONS`, and
  by bytes from M10.5 slice 12), and a blob goes with the last
  registration holding it (an engine opened on it keeps its bytes until
  M10.5 slice 13's engine pool closes it). A policy weighs undone
  sources but not `blob_bytes`, so a large undone insert makes
  `MemoryBudget` drop every snapshot it may until its branch is pruned.
  Default: `AllOf([FixedCount(200), MemoryBudget(256 MB)])`.
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
