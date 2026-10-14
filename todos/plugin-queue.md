# Plugin queue

Plugins to build and ship in `plugins/`, in order. Status: **final**
(agreed 2026-10-13; #4 added the same day with the page-images decision);
nothing below is built yet.

The two already shipped are `tax-form-bookmarks` and `amount-crossref` (see
[`plugins.md`](plugins.md)).

## Queue

### 1. Bookmark from Pattern (`bookmark-from-pattern`) — done 2026-10-13
Landed with: positioned bookmarks in core (`DestView` on bookmarks: read
through PDFium, kept on save in both paths, `/XYZ` converted per axis and
other kinds kept as is; clicking goes there; New Bookmark and Set
Destination record the current view), `Edit::OutlineInsertTree` (instead of
batch references: a new heading and its children in one edit, no ids
needed), capture groups on pattern matches, and `patterns.presets()`. The
form shows every field at once (no conditional fields in the declarative
UI yet).

The original first example from the plugin plan. Turn pattern-search hits
into bookmarks: the user gives a regex (or picks a Patterns preset), and
each match gets a bookmark at its position, titled from the match.
- Decided 2026-10-13:
  - **Title**: a template field, default `{0}` (whole match); `{1}`, `{2}`…
    are capture groups, `{name}` a named group.
  - **Which matches**: every match, each bookmark pointing at the match's
    position on the page (positioned bookmarks are in scope, see API
    additions). Re-running skips bookmarks that already exist.
  - **Placement**: a choice of top level (after existing bookmarks), under
    an existing bookmark, or under a new parent named from the pattern (needs
    batch references, see API additions).
  - **Memory**: remember the last settings (pattern, template, mode,
    placement) in storage; offer the user's Patterns presets in a choice
    beside a custom pattern (needs `patterns.presets()`).

### 2. Subtotal (`subtotal`; was "Footing Checker")
Drag a region; the plugin reads every number in it and shows the total in
the status bar, like Excel's status-bar sum. Needs the region API (see API
additions).
- Decided 2026-10-13:
  - A plain sum of every number in the region (no total-line check).
  - No calculator-tape placement.
  - Updates live while dragging.
  - Number parsing as in `amount-crossref`: `$`, commas, `(1,234)`
    negative, a dash as zero.

### 3. Currency Convert (`currency-convert`; the HTTP plugin)
Find every foreign-currency amount in the document and convert it, using a
free public exchange-rate service (e.g. Frankfurter / ECB rates). Rates are
cached in storage. Pairs with Subtotal (convert a region's sum). Needs the
HTTP additions (see API additions).
- Decided 2026-10-13:
  - Works across the whole document.
  - A dialog picks the target currency (no USD assumption) and the rate
    date: today by default, or a historical date.
  - "Replace": plugins can't rewrite page text yet, so for now each amount
    gets a highlight whose note holds the converted value. Once core can
    edit page text, plugins should be able to as well, and this plugin
    replaces the text for real.

### 4. Find Duplicate Pages (`duplicate-pages`; name not final)
Find near-duplicate pages within one binder: the same statement scanned
twice, a W-2 sent in two batches, pages repeated by a merge. Core doesn't
cover this (M14 "Compare documents" diffs two documents). First user of the
render API (see API additions).
- Fingerprint each page from a small grayscale render (about 50 dpi) with a
  perceptual hash tolerant of scan shift, noise and contrast.
- Duplicates need both image and text (including OCR text) to agree, so
  blank forms and same-layout continuation pages don't match. Born-digital
  exact copies match on text alone.
- Review panel of groups ("Pages 12, 47 — 98% alike"); a row jumps to the
  page; per group, "Keep first, delete others" or "Not duplicates".
  Deletions are one undo step. "Not duplicates" answers are remembered in
  storage.
- Open: the name; a similarity threshold setting; legitimately repeated
  pages (cover and separator sheets, blanks). Later, M14's pixel diff might
  share the fingerprinting.

## Skipped
- **Workpaper Index** (2026-10-13): a dock tree of bookmarks with workpaper
  references, badges and sign-offs. It overlaps the built-in Bookmarks panel
  too much.
- **Review Notes** (2026-10-13): comment status and replies belong in core
  (M9), in the PDF's standard annotation fields.
- **PBC Checklist** (2026-10-13): match a Prepared-by-Client checklist
  against the binder and show received and missing items in a panel.
- **Binder Index Export** (2026-10-13): write a bookmark → page-range CSV,
  on demand or on save. Interesting for the API (see "user-picked files"
  below) but unlikely to be used.
- **Ask Claude** (2026-10-13): questions and figure extraction over the open
  document. AI should be a core feature instead (not in `ROADMAP.md` yet).
- **CCH Workpaper Manager / Engagement integration** (2026-10-13): no
  known interface to build against.
- **Scan Cleanup** (2026-10-13): blank pages, separators, orientation. Belongs
  in core: `ROADMAP.md` M5 already plans scan cleanup (deskew, despeckle,
  auto-rotate, blank-page removal) and separator sheets, and these need page
  images, which plugins can't see.

## Page images for plugins (2026-10-13; reading decided, writing parked)
Plugins see the document model and text; this is how they get images.
- **Rendered pixels**: not model state (derived from sources at some zoom,
  rotation and resolution, like text; rasters would also blow the undo
  history's memory budget). Plugins read them through a render resource (see
  API additions), built with #4.
- **Images embedded in the PDF**: already in the sources. Later, read-only
  facts on the same resource (where each sits, pixel size, dpi, color or
  gray), e.g. to tell a scanned page from a born-digital one. Original bytes
  wait for a use case.
- **Writing images**: parked until core matures; where we landed is in
  [`plugin-image-writes.md`](plugin-image-writes.md). (Plugins should write
  images, in all forms; they should call back into the model from their
  worker rather than return results; an audit log of changes is wanted
  later.)

## API additions
- **User-defined regions** (wanted for Subtotal, 2026-10-13): plugins act on a
  rectangle the user drags. Shape still to decide.
  - Leaning: region selection is a core feature (it also serves copy text
    from a region and the planned table extraction); plugins subscribe to a
    region event in the manifest instead of each owning a drag tool.
  - Live updates need the host to coalesce region events, latest wins: a
    plugin busy with one call gets only the newest region next, not a
    backlog of every mouse move.
  - Likely also `text.text-in-rect(page, rect)` → words with boxes. A plugin
    doing live work should fetch a page's words once and filter locally,
    not ask the render worker on every move (it's shared with tile drawing).

- **HTTP** (needed for Currency Convert, 2026-10-13):
  - `wasi:http` outgoing requests, made by the host (which also does TLS);
    today "network" means raw sockets only.
  - Network permission by domain (`network = ["api.frankfurter.app"]`), so
    the consent prompt says exactly where data goes.
  - Time waiting on a response doesn't count against the call's limit, as
    with forms and OCR.

- **Positioned bookmarks** (decided 2026-10-13, core + API; built with
  Bookmark from Pattern): PDF bookmarks carry a destination, not just a page:
  `[page /XYZ left top zoom]` (Acrobat's New Bookmark records the current
  view this way; null means "keep"), `/FitH top`, `/FitR rect`, and so on.
  Today sheaf keeps only the page: it writes `/XYZ` with nulls and drops any
  position when reading a file. Core change:
  - the outline node keeps the destination (kind, left, top, zoom);
  - reading keeps it, saving writes it back unchanged (no loss on round
    trip), and clicking a bookmark goes to the position;
  - sheaf's own New Bookmark records the current view, as Acrobat does;
  - `outline-insert`/`outline-edit` (and the WIT `outline-item`) carry it.
  Connectors already write positioned `/XYZ` destinations.
- **Batch references** (for Bookmark from Pattern): an edit can name a
  bookmark created earlier in the same batch, e.g. a new parent and its
  children in one submit. Today the parent's id isn't known until after the
  call returns.
- **`patterns.presets()`** (for Bookmark from Pattern): read-only access to
  the user's Patterns presets (`patterns.json`), not just plugin-shipped ones.
- **Page text editing** (later, after core gains it): plugins can edit page
  text through the same edits; Currency Convert would then replace amounts.

- **Render resource** (decided 2026-10-13; built with #4, Find Duplicate
  Pages): read-only page images through WIT resources, so the plugin chooses
  what and how to render.
  ```
  resource page-renderer {
      constructor(page: page-id);
      size: func() -> tuple<f32, f32>;          // points, as displayed
      render: func(opts: render-options) -> result<image, string>;
  }
  record render-options {
      dpi: u32, region: option<box2>, rotation: rotation,
      format: pixel-format,                     // gray8, rgba8, bitonal(threshold)
      file-annotations: bool, form-fields: bool, background: option<rgb>,
  }
  resource image {
      width: func() -> u32; height: func() -> u32;
      to-page: func(x: f32, y: f32) -> pt;
      read-rows: func(y: u32, count: u32) -> list<u8>;
      read-rect: func(x: u32, y: u32, w: u32, h: u32) -> list<u8>;
  }
  ```
  - Pixels stay on the host; plugins pull rows or tiles, so big renders
    needn't fit in plugin memory.
  - Handles live for one call and are freed when it returns.
  - Budgets: pixels rendered per call and images alive at once.
  - Lowest priority on the shared render worker (behind on-screen tiles);
    render time isn't charged to the plugin.
  - Respects sheaf's page mapping: source, index, rotation added in sheaf.
  - No new permission. The consent text should note that a plugin with
    network access could send page images, as it could send text.
  - Not at first: sheaf's own unsaved markup in the render (the app draws
    it, not PDFium; would need compositing).
  - Host work: a `RenderSource` trait like `TextSource` (faked in tests);
    region-at-dpi rendering, grayscale and bitonal conversion, and
    annotation/form switches in the render worker; SDK helpers (rows,
    grayscale, threshold, pixel ↔ page mapping); fixture tests for budgets,
    handle scope and mapping; a `docs/plugins.md` section.

## API gaps these would expose
- **User-picked files** (from Binder Index Export, 2026-10-13): file
  permissions are fixed folders in the manifest, so a plugin can show a save
  dialog but can't write where the user chose. The usual fix: a file the user
  picks in a host dialog is granted to the plugin for that file alone (the
  choice is the consent); maybe also the open document's own folder.
- No edit inserts pages from a file or sets an OCR text layer.
- Panel rows are lost when a plugin restarts.
- Key bindings for a turned-off plugin stay until restart.
