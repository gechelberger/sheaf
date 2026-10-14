# Plan: Remove Blank Pages

Status: **done** (2026-10-14), as M8 slices 9–12. See [`m8.md`](m8.md) for notes.
The detector lives in `sheaf_ops::blank`, not a crate of its own, and the scan option is
`ScanOptions::drop_blanks` (in `sheaf-ops`, where the detector is), not `ScanParams`.

## Context
We want a way to scan a document and remove its blank pages: duplex backs, scanner
fillers, empty separators. There were three candidate homes: a second command in the
duplicate-pages plugin, a toggle in scan cleanup, or a core command. The roadmap already
lists "blank-page removal" under M5 scan cleanup (`ROADMAP.md:50-53`).
`todos/cleanup-cli.md:165` also lists it as "later". Nothing is built. The only blank
check in the code is the private `flat` flag in `plugins/duplicate-pages/src/lib.rs:30`
(`image_bits`). That flag is a 16×16 average hash at 24 dpi, made for *skipping* pages,
not for deleting them.

## Recommendation: one core detector, three entry points

**Put it in core, as an op in `sheaf-ops`.** Then expose it as a core command, a CLI
subcommand, and an option in the scan pipeline.

Why not the duplicate-pages plugin:
- Blank detection is a different job. A false positive deletes content, so it needs a
  stronger detector than the 24 dpi hash.
- The scan pipeline (native, core) and the CLI both need the same detector. Neither can
  call into a WASM plugin today, because plugins can't run headless.
- The plugin keeps its own `flat` check for skipping pages. It doesn't need to change.

Why not only a scan-cleanup toggle:
- Scan cleanup works on images going *into* a PDF. Its "clean an existing PDF" variant is
  explicitly a later slice, because it moves geometry under OCR text and annotations.
- Removing blanks from an existing PDF only *deletes pages*. It writes no image and moves
  no geometry, so it can ship now, before `sheaf-scan` exists.
- The toggle still happens: scan intake reuses the same detector.

## Detector: `sheaf-ops/src/blank.rs`
- `pub fn is_blank(gray: &[u8], w, h, &BlankParams) -> BlankVerdict`. This is pure
  pixels, so `sheaf-scan` can call it on images before assembly.
- `pub fn find(snapshot, pages: &[usize], &BlankParams, &dyn Progress) -> Result<Vec<Candidate>, Cancelled>`
  - **Content check first (cheap).** A page with annotations, form fields or links is
    *not* blank. A page with text (including unsaved OCR layers) is not blank either,
    *unless* the text, once normalized, matches the "intentionally blank" phrase list
    ("this page (is )?intentionally left blank", "intentionally blank", …), optionally
    alongside a page number or form header/footer. Such a page is a candidate with
    reason `IntentionallyBlank`, and it's **dropped by default**. On a scanned page with
    no OCR, this match can't happen. In that case it relies on the "nearly blank" ink
    rule, or the user runs OCR first.
  - **Pixel check.** Render to Gray at about 60 dpi. Reuse `render_at` from
    `sheaf-ops/src/export.rs:129`.
    - Ignore a margin of about 4%: scanner edge shadows, punch holes, staple marks.
    - Estimate the paper level as a high percentile (about the 90th) of the gray
      histogram. This tolerates off-white and gray paper.
    - Count a pixel as *ink* only if it's well below the paper level, for example under
      60% of it. Faint bleed-through from the back side stays "paper".
    - Despeckle: over a grid of about 32×32 cells, a cell counts as inked only if its
      ink fraction is above a small floor. Isolated dust specks then don't count.
    - The page is blank when the total ink fraction is below a threshold *and* the
      inked-cell count is below a threshold. The second condition keeps a page with one
      short line on it, such as a signature, from being dropped on ink alone. Such pages
      can be reported as "nearly blank" and are unchecked by default.
  - `Candidate { index, ink: f32, reason }`, so the UI can show *how* blank a page is.
- `BlankParams { sensitivity: Strict | Normal | Lenient, keep_intentional: bool }`
  maps to the thresholds above. Default is Normal.
- Unit tests use synthetic rasters: pure white, off-white with noise, bleed-through,
  one line of text, a lone signature, a dark scanner border.
- Add a fixture test on real scans if we have any.

## Entry point 1: core GUI command "Find Blank Pages…"
- Put it in the Document menu next to Delete Pages (`crates/sheaf-app/src/menus.rs:222`),
  category Pages, `When::Document`.
- It needs the usual built-in wiring:
  - an action;
  - a `command_info.rs` entry with help and keywords ("blank, empty, remove, scan");
  - an icon under `assets/icons/fns/find-blank-pages/` and in `assets.rs`.
- Flow:
  1. A small dialog with the page range (reuse `sheaf_ops::pages::parse`), the
     sensitivity, and "Keep 'intentionally blank' pages" (off by default).
  2. `find` runs in the background with a `Tracker`, like export. The status strip shows
     progress and Esc cancels.
  3. A **preview pane** opens with a thumbnail of each page that will be dropped. Each
     thumbnail shows its page number and reason ("blank", "intentionally blank", "nearly
     blank 0.3%") and a checkbox, all checked to start. Clicking a thumbnail jumps to
     that page. "Delete N pages" applies one `Edit::DeletePages`, which is one undo step.
- **Never auto-delete in the GUI.**

## Entry point 2: CLI (after slice 2, `sheaf-cli`)
`sheaf remove-blanks in.pdf -o kept.pdf [--rejects blanks.pdf] [--pages 1-] [--sensitivity normal] [--keep-intentional] [--dry-run]`
- **`--rejects blanks.pdf`** writes the dropped pages to a second PDF, so
  kept ∪ blanks = original. Each rejected page gets a **bookmark** like "p. 4 — blank"
  or "p. 17 — intentionally blank", so the user can trace it back to the original
  without the page content being changed. The user flips through blanks.pdf to verify.
  If something was wrongly dropped, the bookmark says where it came from.
- `--dry-run` prints a table (page, reason, ink %) and writes nothing.
- Possible later addition, `--contact-sheet sheet.pdf`: a grid of labelled thumbnails of
  the rejects, about 12 per page. It's faster to scan for a 500-page binder than
  blanks.pdf. Hold off until rejects proves too slow to review.
- These are separate concerns: detection is shared, and verification is per front end.
  The GUI has the preview pane; the CLI has rejects / dry-run.

## Entry point 3: scan pipeline (slice 4, `sheaf-scan`)
- Add `ScanParams.drop_blanks: bool` (CLI `--drop-blanks`, a checkbox in the From
  Scans/Photos dialog).
- After cleanup and before assembly, call `blank::is_blank` on the cleaned gray image.
  Blank detection works better *after* illumination flattening, so it belongs late in the
  pipeline.
- The preview dims dropped pages instead of hiding them.

## Sequencing
- The detector and the GUI command depend only on existing pieces (`sheaf-ops`, render,
  `Edit::DeletePages`), so they can land as their own slice at any time.
- Add a line to `todos/cleanup-cli.md` and to the M5 bullet in `ROADMAP.md` saying where
  it lives.
- The CLI and scan hooks follow their own slices.

## Critical files (when implemented)
- New: `crates/sheaf-ops/src/blank.rs`, and `crates/sheaf-ops/src/lib.rs` (add `pub mod blank`)
- `crates/sheaf-app/src/menus.rs`, `command_info.rs`, `assets.rs`, and a handler module
  (pattern: `export.rs` / `export_dialog.rs` for the background job and dialog)
- Reuse: `sheaf_ops::export::render_at`, `sheaf_ops::pages::parse`,
  `sheaf_progress::Tracker`, `Edit::DeletePages` (`crates/sheaf-doc/src/model.rs:204`)

## Verification (when implemented)
- `cargo test -p sheaf-ops` for the synthetic-raster cases.
- Build a mixed test PDF: text pages, a true blank, a scanned blank with noise and
  bleed-through, and an "intentionally left blank" page. Then:
  - Run Find Blank Pages and check that the true blanks and the "intentionally left
    blank" page are listed, and that no text page is.
  - Turn on "Keep 'intentionally blank' pages" and check that the "intentionally left
    blank" page drops out of the list.
  - Delete them, then Undo, and check everything is restored in one step.
- Cancel with Esc mid-run on a large binder.

## Decided
- Core detector in `sheaf-ops`. The GUI gets a preview pane, and "intentionally blank"
  pages are dropped by default.
- The CLI keeps `--dry-run` for sure.

## Open questions
- CLI verification beyond `--dry-run`: `--rejects blanks.pdf`, a contact sheet, or both.
  Decide when the CLI slice lands.
- Phrase list for "intentionally blank": English only at first? Should it be
  configurable in settings?
