# Plan: ops layer, CLI, shared progress, scan cleanup

Status: **done** (2026-10-14), branch `cleanup-cli`. See "Slice notes" at the end.
`sheaf-scan` has since become `sheaf_image::clean` ([`m8-sheaf-image.md`](m8-sheaf-image.md)).

## Why

Many commands still to come (export, OCR, combine, scan cleanup, running
plugin commands in batch) are as useful headless as in the GUI. Most of the
real work already lives in GUI-free crates (`sheaf-doc`, `sheaf-render`,
`sheaf-ocr`, `sheaf-plugin`). The glue between them is stuck in `sheaf-app`:
the export pipeline, `patterns.rs`, and the logic inside the `--selftest-*`
hooks. Those flags in `main.rs` are a CLI already, but they have to start GPUI
to run.

Scan cleanup is the first op. It is on the roadmap as an unfinished M5 slice
("image import → cleanup filters"). The roadmap covers flatbed-style cleanup
(deskew, despeckle, auto-rotate, blank pages) but not **phone photos**:
perspective, shadows, white balance and curled pages. Turning a pile of phone
shots into a straight, readable, OCR-ready PDF is valuable in both the GUI
and the CLI.

## Decided

- **The command registry stays in the app.** `commands.rs` is GPUI (actions,
  menus, globals). Its built-in handlers become thin wrappers over ops.
- **The CLI ships both ways.** The `sheaf-cli` crate holds the clap
  subcommands and builds a lean `sheaf-cli` binary with no GPUI, for servers
  and CI. `sheaf-app/src/main.rs` hands off to `sheaf_cli::run` when the first
  argument is a known subcommand, so `sheaf scan …` and `sheaf combine …` work
  as the roadmap writes them.
- **We write our own progress trait.** No dedicated crate fits:
  - prodash is MIT only, on its 31st major version, has no cancellation, and
    its `counter()` hands out a shared atomic that can't cross into WASM;
  - howudoin is stale (2023) and relies on a global consumer loop;
  - progress-token pulls in tokio;
  - indicatif, kdam and the like are renderers, not traits.

  We borrow ideas, not code: nesting, max/unit and levelled messages from
  prodash; weighted children and a `check()?` cancel from progress-token. No
  attribution is needed.

  (2026-10-17: the crate was later published as `in-progress`, which sheaf
  now takes from crates.io; see m10.5-edit-api.md, between slices 6 and 7.)
- **Plugins get progress too**, through a WIT resource that mirrors the trait.
- **Scan cleanup starts with the classic, pure-Rust path.** ML unwarping
  (UVDoc) comes in the slice after, opt-in only. See "UVDoc" below.
- **Move ops one at a time.** An op moves into `sheaf-ops` only when a CLI
  subcommand needs it. No big-bang move out of `docview.rs`.

## Crates

### `sheaf-progress` (std only)

```rust
pub enum Unit { Items, Pages, Bytes }   // closed set, mirrors a WIT enum
pub enum Level { Info, Warn, Error }
pub struct Cancelled;                   // error type for `?`

pub trait Progress: Send + Sync {
    fn set_total(&self, total: Option<u64>, unit: Unit); // None = indeterminate
    fn set(&self, done: u64);
    fn inc(&self, n: u64);
    fn message(&self, level: Level, msg: &str);
    fn is_cancelled(&self) -> bool;
    fn check(&self) -> Result<(), Cancelled> { … }
    /// Covers `weight` of this progress's span; shares cancellation.
    fn child(&self, name: &str, weight: f32) -> Box<dyn Progress + '_>;
}
pub struct NoProgress;
```

- Ops take `&dyn Progress`, so generics don't spread through the API.
- It ships an `Arc`-of-atomics implementation (plus a `Mutex<String>` for the
  latest message). The GUI polls it each frame. The CLI renders it with
  `indicatif`, which only `sheaf-cli` depends on.
- Each root has one `AtomicBool`, shared by all its children. Ops call
  `p.check()?` once per page or chunk.
- What this replaces:
  - export's `FnMut(Progress) -> bool` (`export.rs:67`, `:79`) moves to the
    trait when export moves.
  - OCR keeps its `OcrEvent` channel for now, because the events carry
    results. Its `JobHandle` cancel can adopt the trait later.

### `sheaf-scan`

Image in, image out; it knows nothing about PDFs. It uses `imageproc` (MIT,
pure Rust, already in the lockfile through oar-ocr). An optional `uvdoc`
feature adds ML unwarping.

### `sheaf-ops`

- Ops have the shape `fn(inputs, &Params, &dyn Progress) -> Result<Output>`,
  with serde `Params`. A keymap `args` blob, a plugin's `args` and CLI flags
  can then all deserialize into the same struct.
- Depends on `sheaf-doc`, `sheaf-render` and `sheaf-scan`, and optionally on
  `sheaf-ocr`. **No gpui.**
- Owns the page-range parser (`1-3, 7, 10-`) that turns a range into
  `PageId`s. M8 Combine needs it too.

### `sheaf-cli`

- A library of clap subcommands, plus the `sheaf-cli` binary.
- The `--selftest-*` flags move here as integration tests wherever they don't
  need a window.

## Image import (prerequisite, `sheaf-doc`)

No image import exists today: `SourceInfo` (`model.rs:31`) covers PDFs only.
Each image becomes a one-page PDF in memory, built with lopdf, and is added
as an ordinary source:

- JPEG passes through as DCTDecode.
- Gray and color images use Flate.
- 1-bit images use CCITT G4, reusing `g4` from `export.rs`.

M8 Combine Files and Insert from image share this path. Multi-page TIFF
input comes in the same slice.

## Scan pipeline (classic)

Each step is a pure function with its own params, so the GUI can preview the
steps one by one. Noise-sensitive analysis runs on a downscaled copy (~1000px);
pixel operations run at full resolution.

1. **Decode** and apply the EXIF orientation.
2. **Page quad:**
   - blur → `canny` → dilate → `find_contours` → `approximate_polygon_dp`;
   - keep the largest convex 4-gon covering at least 20% of the frame;
   - fall back to `min_area_rect`.

   Warp the full-resolution image with `Projection::from_control_points` +
   `warp`. The output aspect ratio comes from the mean edge lengths.
3. **Coarse orientation (0/90/180/270):**
   - use oar-ocr's doc-orientation model when the `oar` feature and the model
     are present, with model lookup through `sheaf-ocr` `data.rs`;
   - otherwise use a projection-profile heuristic, which decides 0 vs 90 only.
4. **Deskew:** maximize projection-profile variance over ±15° (1° steps, then
   0.1°), then rotate with a white fill.
5. **Illumination:**
   - per channel, estimate the background (dilate, then median) and divide the
     image by it, which removes shadows and lighting gradients;
   - gray-world white balance on that background, so the paper comes out
     white;
   - percentile contrast stretch.
6. **Output mode `color | gray | bw`.** `bw` uses Sauvola thresholding (integral
   images; k ≈ 0.2–0.34, R = 128, window ≈ 2% of page width), then despeckle
   (drop connected components below about 0.02% of the page area).
7. **Page size `original | letter | a4`** at a chosen DPI, centered.
8. **Assemble:** build the pages through image import, then optionally OCR the
   cleaned pages. The text layer lines up because OCR ran on the very image
   that became the page.

Written by us, since no permissive crate has these: the quad finder, deskew,
Sauvola, background flattening and the orientation heuristic. That's roughly
1.5–2 weeks with a fixture corpus; most of it is tuning on real photos.

Ruled out:

- `opencv` (needs native OpenCV and libclang to build);
- unpaper and ScanTailor (GPL, though their published algorithms are fair
  game as ideas);
- `superbook-pdf` (AGPL).

Leptonica (BSD-2) is a possible fallback for deskew and background
normalization in tesseract-enabled builds; not pursued.

Later, optional: a **noteshrink** mode, a palette reduction for colored
handwritten notes (an MIT algorithm, about 150 lines), and blank-page removal.

## UVDoc (ML unwarp, opt-in)

- **What it does:** for curled or bound pages. An 8M-parameter network predicts
  a coarse 45×31 sampling grid. The grid is upsampled and applied to the
  full-resolution photo, so detail is kept, but sharp creases don't fully
  flatten.
- **License:** the weights are MIT (original repo) or Apache-2.0 (Paddle port).
  Unverified: whether the Paddle port was retrained.
- **Cost:** about 31 MB of model and about 0.9 s per page on CPU (PaddleX
  figures, not measured here).
- **In oar-ocr 0.10:** `DocumentRectificationPredictor` (example:
  `examples/document_rectification.rs`). The model is `uvdoc.onnx` from
  ModelScope `greatv/oar-ocr`.
- **Placement:** after the perspective warp and before deskew, behind
  `--flatten-curl`, and never on by default. It can add slight wobble to pages
  that are already flat, and flat receipts and forms don't need it.
- **OCR:** we disabled UVDoc inside OCR (`oar.rs:50`) because its text boxes
  can't be mapped back to the original page. That doesn't apply here, since
  OCR runs on the rectified output.
- **Alternatives:**
  - DocTr, DocGeoNet and DocTr++ have non-commercial or undeclared licenses.
  - Newer permissive models (SyntheticDoc, AADD) have no confirmed weights or
    ONNX exports.
  - Revisit if that changes.

## Plugins

- Add a `progress` interface to `wit/sheaf.wit` with a `task` resource:
  `constructor(label, total: option<u64>, unit)`, `set`, `inc`, `message`,
  `is-cancelled` and `child`.
- The host adapts the resource to `sheaf_progress::Progress` and shows it in
  the GUI status area.
- `is-cancelled` lets a plugin stop cleanly before the epoch deadline
  (`runtime.rs`).
- The safe wrapper goes in a new `crates/sheaf-plugin-api/src/progress.rs`.
- Bump the WIT package version as the icons change did.
- Test: a command in the test-plugin fixture that reports progress and honours
  cancellation (`crates/sheaf-plugin/tests/host.rs`).

## Slices (one commit each)

1. **`sheaf-progress`** + tests. Move export's GUI-free half (`export.rs:67-345`)
   into `sheaf-ops`. `export::run` takes `&dyn Progress`, and the app's
   `run_export` uses the shared implementation.
2. **`sheaf-cli`** skeleton, the `export` subcommand, and the `main.rs` hand-off.
3. **Image import** in `sheaf-doc`, plus `sheaf images-to-pdf`.
4. **`sheaf-scan` classic pipeline**, plus
   `sheaf scan photos/*.jpg -o out.pdf [--mode bw] [--page letter] [--ocr]`.
   Fixtures: real phone shots in `crates/sheaf-scan/tests/fixtures`, including
   a shadowed one, a skewed one and one shot at an angle.
5. **GUI:** a File → Create → From Scans/Photos… dialog over the same
   `ScanParams`, with a per-step preview.
6. **UVDoc** opt-in (`--flatten-curl` and a dialog toggle).
7. **Plugin progress:** WIT, API and host.
8. **`ROADMAP.md`:**
   - M5 cleanup gains camera capture (perspective, illumination, white
     balance, curl);
   - the M15 CLI line notes the ops/CLI split;
   - fix the stale parity rows that say M6 for scanner intake and separator
     sheets (should be M5).

## Verification

- `cargo test --workspace`. `cargo tree -p sheaf-cli | grep gpui` prints
  nothing.
- `sheaf-cli scan <phone photos> -o out.pdf --mode bw --ocr`, then open the
  result in sheaf. Pages are straight, cropped and on white backgrounds, and
  text selection lines up.
- `sheaf export …` through the main binary gives the same output as File >
  Export in the GUI.
- Ctrl-C during `sheaf-cli scan` exits cleanly through `check()`, and the GUI
  Cancel button stops an export.

## Open

- Should a scan job save the raw photos as attachments, so it can be redone
  with other settings later?
- Cleaning pages already in a PDF (rather than importing photos) changes
  geometry under existing OCR text and annotations (see
  [`plugin-image-writes.md`](plugin-image-writes.md)). It's a separate later
  slice, and it probably reruns OCR and refuses to run on pages that have
  annotations.

## Slice notes

1. Done. `sheaf_progress::Tracker` places a child after its previous
   sibling's share as well as after the parent's own count, so steps
   ("render, then OCR") line up without the parent counting them. Export
   now stops with `Cancelled` instead of returning the partial list: per-page
   files already written stay, and a TIFF's `.part` file is removed. The GUI
   gained what it lacked before: Esc stops a running Export Pages, and the
   OCR progress strip shows export progress too.
2. Done. `sheaf_ops::open` loads a file headlessly: pages, bookmarks and
   sheaf's own marks, as the app does on open. The page-range parser moved
   from `sheaf_ocr::page_range` to `sheaf_ops::pages`, which also accepts an
   open start (`-3`). `sheaf_plugin::ui::parse_page_range` stays separate:
   its empty input means every page, and the plugin crate shouldn't pull in
   the renderer. `sheaf_cli::run` returns the exit status (0, 1, 2 for bad
   arguments, 130 when cancelled), so the app's hand-off keeps it.
   `scripts/package.sh` ships `sheaf-cli` beside `sheaf`. A file named like
   a subcommand opens in the app as `./export`. None of the `--selftest-*`
   flags moved: each drives `DocView` (adding marks through the view, then
   saving), so each needs a window. They can move when their ops (clean,
   flatten, extract) do.
3. Done. `sheaf_doc::images` reads an image file into `PageImage`s, one
   per page (`read`; TIFF through the `tiff` crate, since `image` reads
   only a TIFF's first page), and `to_pdf` builds the PDF bytes.
   `PageImage::from_image` takes pixels the caller already has; the scan
   pipeline uses it. The details:
   - A JPEG's EXIF rotation becomes the page's `/Rotate`. Mirrored or CMYK
     JPEGs are decoded instead.
   - All-0/255 gray is stored as G4. `g4` moved here from export.
   - With no recorded resolution, 96 dpi is assumed, as img2pdf does.

   Deviation: there's no "add as a source" helper yet. Sources are
   path-based (save, incremental save and the renderer all reopen the
   path), so an image source needs its PDF bytes written to a file. M8's
   Insert from Image adds that helper (a content-addressed file in the
   cache directory) when the GUI needs it. HEIC isn't readable: there's no
   permissive pure-Rust decoder.
4. Done: `sheaf-scan`, `sheaf_ops::scan`, and `sheaf scan … [--mode bw]
   [--page letter] [--ocr]`. Deviations, found by running it:
   - **Order.** Crop → clear edges → flatten → orient → deskew → fit →
     mode. Flatten comes before deskew because deskew fills the rotated
     corners with white: before flattening, that white beside the still-tan
     paper made dark strips. The orientation model reads flattened pages
     far better (0.8 against 0.5 confidence), so orientation waits for it.
   - **Background slivers.** The crop is insetted 0.75%, and `clear_edges`
     whitens dark shapes touching a cropped page's border, as unpaper's
     border filter does. It runs before flattening: afterwards Sauvola
     can't see a uniform dark strip.
   - **White point.** The contrast stretch also maps the median (paper) to
     white, clamped at 200 so mostly-picture pages aren't blown out.
   - **Despeckle size.** "0.02% of the page area" would erase punctuation
     (1,700 px at 300 dpi Letter). It drops shapes under 1/200,000 of the
     page instead, about 42 px (half a millimeter across) at 300 dpi.
   - **Orientation.** Projection profiles got a real page wrong (row/column
     energy 0.65 when upright). `quarter_turns` uses word shape instead:
     with letters smeared into words, words lie wide. That gives the axis.
     When the oar model is present (`sheaf_ocr::orient::scores`), it picks
     between the two turns on that axis; on its own the model was wrong
     about a sideways shaded page. The model's label is how far the
     content is turned clockwise (checked by an ignored test in
     `sheaf-ocr/tests/engines.rs`).
   - `sheaf-ops` depends on `sheaf-ocr` outright: without engine features
     it's light, and the binaries choose the engines (`sheaf-cli` has the
     same `ocr-tesseract`/`ocr-oar` defaults as the app).
   - **Fixtures.** There are no real phone shots in the repo yet (none were
     on this machine). The unit tests build synthetic photos in code
     instead: an angled shot on a dark background, a skewed page, a
     sideways page, and a shadow with a warm cast. An end-to-end run on a
     synthetic photo of rendered text, in all four orientations, came out
     upright and straight with clean edges, and OCR read it back. Real
     photos should still be added to `crates/sheaf-scan/tests/fixtures`
     for tuning.
5. Done. File > Create > From Scans/Photos… (`workspace::CreateFromScans`,
   with its own icon) picks images, then opens `scan_dialog.rs`:
   - **Options.** Output, page size, resolution, the four cleanup steps,
     and OCR (greyed out with the reason when no engine can run).
   - **Preview.** One image at a time, at 1200 px, and per step: Photo
     (the page found, outlined), Page, Lighting, Upright, Straight, Result.
     `sheaf_scan::clean_with` hands each step's image to a callback.
   - **Running it.** Create asks where to save, runs `sheaf_ops::scan` with
     a progress bar (Esc stops it), then opens the PDF. The choices persist
     as `settings.scan`.

   It also works in an empty window. Found while testing: colour pages
   were stored as Flate, 6.9 MB for one Letter page. Colour and gray scan
   pages are now JPEG (quality 80, 0.8 MB); black and white stays G4
   (50 KB). New `SHEAF_SCRIPT_SAVE` answers save pickers in scripts, so
   a scripted scan doesn't write its PDF over the photo it picked.
6. Done. `sheaf_ocr::rectify` wraps oar-ocr's `DocumentRectificationPredictor`.
   `ScanParams::flatten_curl` (off by default; `--flatten-curl`, and a
   dialog toggle greyed out with the reason) runs it right after the
   crop, through `sheaf_scan::Models` (the orientation model moved in there
   too). The model is `uvdoc.onnx` from the oar-ocr v0.3.0 release, the
   same tag as the other models. `fetch-ocr-data.sh --uvdoc` gets it, and
   `package.sh` bundles it only with `SHEAF_PACKAGE_UVDOC=1`. Asking for
   it without the model is an error, not a silent no-op.

   On a synthetic curled page it straightened arcing text lines well
   (about 1 s extra). Found along the way, both in the classic path:
   - A curled page's bowed edges aren't a 4-gon, so nothing was cropped.
     Now it falls back to `min_area_rect`, as planned.
   - `clear_edges` missed the wide wedges that leaves. It now also counts
     as dark anything under 60% of the paper brightness nearby (a 4%-radius
     max filter, so gradual shadows don't count).

   Covered by a test of the curled-page crop, and an ignored UVDoc smoke
   test next to the orientation one.
7. Done. The WIT has a `progress` interface with a `task` resource; each
   task is a `Tracker` under the call's own (`CallContext::progress`, a
   new field). The app creates one for every command and panel action,
   shows it in the document's status bar and progress strip, and Esc
   cancels it once it has reported something.
   - **Version.** Deviation: 0.2.1, not a minor bump. Adding an interface
     is backward compatible, and the package version is part of every
     import name. A 0.3.0 bump would have stopped every 0.2 plugin from
     linking; with 0.2.1, the six bundled plugins still built against
     0.2.0 all load Ready.
   - **Pieces.** `sheaf_progress::Tracker::child_tracker` gives an owned
     child for the resource table; tasks are freed when the call returns,
     and at most 256 per call are tracked. The SDK adds
     `sheaf_plugin_api::progress` (`pages`, `items`, `check`).
   - **Tests.** The fixture's `count` command reports, posts a message,
     and stops when cancelled (`progress_reports_and_cancels`).
   - **A bundled user.** `tax-form-bookmarks` 0.2.2 (`api = "^0.2.1"`)
     reports as it reads pages. On the 3,837-page Title 26 PDF the status
     bar read "Reading pages: 646 of 3837 pages…", and Esc stopped it.
8. Done. In `ROADMAP.md`:
   - M5 gains camera capture and marks image import and cleanup done
     (blank-page removal is still open).
   - M15's CLI line describes the ops/CLI split.
   - The parity rows for scanner intake, separator sheets, images and OCR
     say M5. Two stray "M5 plugin host" references now say M6.

Verification:
- **Automated.** `cargo test --workspace`, clippy, the
  `--no-default-features` app build, and `cargo tree -p sheaf-cli | grep
  gpui` (empty) all pass.
- **CLI.**
  - `sheaf export` through the main binary matches File > Export Pages,
    since both run the same op.
  - Ctrl-C on a 400-page `sheaf-cli export` exits with status 130 through
    `check()`.
  - `sheaf-cli scan --mode bw --ocr` on a synthetic phone photo gives a
    straight, white page whose text reads back.
- **GUI.** Esc stops a running export, a plugin command, and a scan in the
  dialog.

Blank-page removal landed in M8 (slices 9–12, [`blanks.md`](blanks.md)).

Still open: real phone-photo fixtures; and the two
"Open" questions above (keeping the raw photos as attachments, and cleaning
pages already in a PDF).

## Tuning on real photos (2026-10-14)

Ten phone photos of engineering drawings on a granite counter: landscape
sheets shot portrait, fine lines, little text, uneven light, and three
close-ups of part of a sheet. What changed:

- **Orientation.** Each signal was wrong somewhere on its own:
  - word shape misjudged a drawing whose labels run both ways;
  - PaddleOCR's orientation model was confidently wrong, and flipped its
    answer when the input changed slightly;
  - Tesseract's OSD (`osd.traineddata`, now fetched by default) missed
    one.

  `sheaf_ops::scan::orientation_votes` now combines them: the model
  averaged over the page turned all four ways, plus OSD on the gray page
  and on a black-and-white copy (weighted by confidence/8), with word
  shape as a 0.15 tiebreak. All ten come out upright (before: two wrong).
  The orienter now gets the full-resolution page, because OSD needs it.
- **Edge clearing.**
  - It only takes dark shapes nearly all within 6% of the edges. A table
    in a close-up runs off the photo, and it had been erased whole.
  - It fills with the local paper colour, not white. White made
    flattening leave tan halos around cleared areas and along the edges.
- **Detection.** It ignores quads whose corners fall more than 2% outside
  the photo: in a close-up of part of a sheet, those corners are guesses.

Still open:
- **Page edges on busy backgrounds.** Light granite gives a loose crop:
  stone along an edge, and in black and white a short dark sliver.
  Ranking candidates by contrast across their sides, a smoothness mask,
  and Hough lines were all tried; on these drawings each did worse than
  the largest quad.
- **Close-ups.** One with a table can still be cropped to the table.
- **Fixtures.** The photos (25 MB) aren't committed. The new regression
  test covers the table-off-the-edge case synthetically.
