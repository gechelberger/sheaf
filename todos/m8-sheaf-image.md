# M8 follow-up: `sheaf-image`

Status: **in progress** (2026-10-14). Branch `m8`, after slice 12. One commit per slice.

## Context
`sheaf-scan` was made a crate on the grounds that it's "image in, image out; it knows
nothing about PDFs" (`todos/cleanup-cli.md`). That boundary is worth keeping, but the
name is too narrow. Other pure-pixel code is spread across `sheaf-ops`, `sheaf-doc`,
`sheaf-ocr` and `sheaf-render`. Some of it is written out by hand more than once, and the
copies disagree:

- **Raster to gray, four copies:**
  - `OcrImage::from_raster` (`sheaf-ocr/src/engine.rs`);
  - the blank detector's render path (`sheaf-ops/src/blank.rs`, in `find`);
  - auto-crop's `content_margins` (`sheaf-ops/src/crop.rs`);
  - export's `RgbaImage::from_raw` + `into_luma8()` (`sheaf-ops/src/export.rs`).

  Three of them use BT.601 integer weights (77/150/29). The `image` crate's `to_luma8`,
  used by export and by every scan path, uses Rec.709. So a page rendered from a PDF and
  the same page from a photo get slightly different gray levels, and the blank detector
  can judge them differently.
- **Image codecs live in two crates for no structural reason:**
  - decoding (JPEG header and DPI, PNG `pHYs`, multi-page TIFF, alpha flattening) and the
    CCITT G4 encoder are in `sheaf-doc/src/images.rs`;
  - encoding (PNG/JPEG with DPI, the per-page TIFF writer) is in
    `sheaf-ops/src/export.rs`, which reaches into `sheaf_doc::images::g4` for its
    black-and-white TIFF.

Rename `sheaf-scan` to **`sheaf-image`** and gather the pure-pixel code there.

## The boundary
`sheaf-image` depends on `image`, `imageproc`, `png`, `tiff`, `fax`, `sheaf-progress`,
`anyhow`, `log` and `serde`. It must **not** depend on:
- `lopdf`, `pdfium-render`, or any `sheaf-*` crate that does (`sheaf-doc`,
  `sheaf-render`, `sheaf-ops`);
- OCR runtimes or models (`sheaf-ocr`, tesseract, onnx/oar);
- gpui.

`sheaf-core` is allowed if a type needs it, but nothing below needs it today.

Rule of thumb: if it takes pixels or image files and returns pixels, bytes or a verdict,
and needs no document, it belongs here. If it needs a page of a PDF, a model, or a
window, it doesn't.

Dependency order after the move:
`sheaf-image` → `sheaf-render`, `sheaf-doc`, `sheaf-ocr` → `sheaf-ops` → `sheaf-cli`,
`sheaf-app`.

## Decided
- **`PageImage` moves as-is** (`PageImage`, `Pixels`, `from_image`, `jpeg`, `size_pt`).
  There's no separate `Jpeg` type. Its forms (JPEG bytes kept as they are, gray, RGB,
  black and white) are image ideas with no lopdf in them; `sheaf-doc`'s `image_stream`
  picks the PDF filter. A standalone JPEG type waits for a second user; the parked
  plugin-image-writes memo is the likely one. Its docs describe it as "an image with its
  resolution, kept in the most compact form it can be embedded in without re-encoding".
- **Migrate; don't re-export.** Code that moves gets one home, and its callers switch in
  the same slice. Two paths to one item would hide callers from search and leave new code
  guessing which path to use. All callers are in this workspace, and the crates aren't
  published. This covers `PageImage` too, even though it appears in `sheaf-doc`'s
  signatures: every caller that builds one already uses `sheaf_image`. If an old path
  creeps back, a clippy `disallowed-*` entry can forbid it.
- **Pixel types move from `sheaf-render`; the trait stays.**
  - `PixelFormat` and `Raster` move to `sheaf_image::raster`. Their users import them
    from there; `sheaf-render` doesn't re-export them.
  - The `Renderer` trait stays in `sheaf-render`. It's about documents (page count,
    sizes, outline, search, text, `hide_sheaf_annots`). Only its output type is an image.
    So do `Job`, `RenderRequest`, `Rendered` and `RenderHandle`: they name pages.
  - `sheaf-ocr`'s `PageSource` trait stays, for the same reason (`text_len`, page sizes).
- **Conversions are methods on `Raster`**, not a new trait. Every caller has a concrete
  `Raster`. A trait can come later if a second pixel container appears.
- **One gray formula.** `Raster::to_gray` uses BT.601 integer weights (77/150/29, as the
  render paths do now). Scan cleanup keeps using `image`'s `to_luma8` on its own
  `DynamicImage`s; only rasters go through the one helper. Re-check the blank detector
  tests and the ten eval photos after the switch.
- **The type names in the scan code stay** (`ScanParams`, `Mode`, `Stage`, `Cleaned`,
  `Quad`). Settings serialize field names, not type names, so a rename later is free, but
  it isn't part of this work.
- **The OCR models stay out.** `orient` and `rectify` in `sheaf-ocr` take and return
  images, but they load ONNX models and Tesseract. Likewise the orientation ensemble in
  `sheaf_ops::scan` stays, because it calls OCR.

## Module layout
```
sheaf-image/src/
  lib.rs      re-exports; crate docs state the boundary
  clean.rs    today's sheaf-scan lib.rs: find_page, perspective, flatten, deskew,
              orient by word shape, Sauvola, despeckle, clean()  (+ tests)
  blank.rs    is_blank, judge_image, Verdict, Sensitivity and its limits  (+ pixel tests)
  raster.rs   PixelFormat, Raster, conversions, content_bounds
  io.rs       load (EXIF-upright), read/read_bytes → PageImage, JPEG/PNG/TIFF parsing,
              flatten_alpha, is_image_path
  encode.rs   g4, PNG/JPEG with DPI, TIFF page writer (color, gray, G4)
```

## Slices

### 1. Rename `sheaf-scan` to `sheaf-image`; move in the blank pixel check
- `git mv crates/sheaf-scan crates/sheaf-image`. Its `lib.rs` becomes `clean.rs`, and
  `tests.rs` goes with it.
- No re-export of `clean::*`. Callers use `sheaf_image::clean::…` (`sheaf-ops`
  `scan`, the scan dialog, the CLI).
- Workspace `Cargo.toml`, and the dependents: `sheaf-ops`, `sheaf-cli`, `sheaf-app`.
- Move these from `sheaf_ops::blank` to `sheaf_image::blank`, with the `pixel_tests`
  module: `is_blank`, `judge_image`, `Verdict`, `Sensitivity`, and the private
  `Limits`/`MARGIN`/`GRID`/`INK`/`CELL_FLOOR`.
- `sheaf_ops::blank` keeps the content checks (the phrase regex, `text_says`,
  annotations), `find`, `write`, `Candidate`, `Reason` and `BlankParams`. The dialog
  and the CLI import `Sensitivity` from `sheaf_image::blank`.
- Docs: the README, ROADMAP M5, `todos/cleanup-cli.md` and `todos/blanks.md`, which
  mention `sheaf-scan` or `sheaf_ops::blank`'s pixel check.
- Check: all workspace tests, and clippy.

### 2. `Raster` and the shared conversions
- Move `PixelFormat` and `Raster` into `sheaf_image::raster`. `sheaf-render`,
  `sheaf-ocr` and `sheaf-print` gain a `sheaf-image` dependency, and every import
  switches (about 15 files).
- Add, on `Raster`:
  - `to_gray(&self) -> GrayImage`, with BT.601 weights, honoring `format`;
  - `to_rgb(&self) -> RgbImage`;
  - `into_rgba(self) -> RgbaImage`, swapping channels in place when the raster is BGRA.
- Add `content_bounds(gray: &GrayImage, below: u8) -> Option<(u32, u32, u32, u32)>`:
  the box of pixels darker than `below`.
- Replace the hand-written copies:
  - `OcrImage::from_raster` becomes `Raster::to_gray` / `to_rgb`. Keep its early drop of
    the 4-byte buffer: convert, then drop.
  - `sheaf_ops::blank::find`: `raster.to_gray()`.
  - `sheaf_ops::crop::content_margins`: `content_bounds(&raster.to_gray(), 235)`.
  - `sheaf_ops::export`: `into_rgba` + `DynamicImage` conversions, or `to_gray` for the
    gray and G4 TIFF.
- Not changed: `docview.rs`'s raster-to-`RenderImage` conversion (gpui, BGRA
  passthrough), and the print paths (`print_dialog.rs`, `sheaf-print/src/gdi.rs`), which
  hand BGRA straight to the platform.
- Tests:
  - unit tests for each conversion on a 2×2 raster in each format;
  - `sheaf-render/tests/pixel_format.rs` still passes through the re-export;
  - the blank, crop and export tests are unchanged.
- Check: the ten eval photos still scan the same, and none is dropped with
  `--drop-blanks`.

### 3. Image codecs
- **From `sheaf-doc/src/images.rs` to `sheaf_image::io`:**
  - `PageImage`, `Pixels`, `read`, `read_bytes`, `decoded`, `jpeg`, `jpeg_info`,
    `png_dpi`, `tiff_pages`, `flatten_alpha` and `is_image_path`;
  - `sheaf_scan::load` (EXIF orientation) joins them.
- **What stays in `sheaf-doc`:** `PageFit`, `Placement`, `placement`, `to_pdf`,
  `image_stream`, `flate` and `files_to_pdf`, all of which build PDF objects.
  `sheaf-doc` depends on `sheaf-image`. **There are no re-exports.** Every user switches
  to `sheaf_image` in this slice: about 19 uses in 10 files.
  - `sheaf-ops`: `images`, `combine`, `scan`, `export` (`g4`), and `tests/blank.rs`.
  - `sheaf-app`: `page_insert`, `combine_dialog`, `workspace`.
  - `sheaf-doc`: `watermark`.

  `sheaf_doc::images` keeps only image pages as PDF objects: `PageFit`, `Placement`,
  `placement`, `to_pdf`, `image_stream` and `files_to_pdf`.
- **To `sheaf_image::encode`:**
  - `g4`, from `sheaf-doc`;
  - from `sheaf-ops/src/export.rs`: `png_with_dpi`, the PNG/JPEG half of `encode`
    (taking an `RgbImage`/`GrayImage` and a DPI, not a `Raster`), and the per-page body
    of `write_tiff_to` as a `TiffWriter` that adds one page (color, gray or G4) with its
    resolution and page-number tags.
  - `export.rs` keeps the rendering, file naming, temp-file-and-rename, and progress.
- **Cargo:**
  - `fax`, `png` and `tiff` move to `sheaf-image`;
  - `sheaf-doc` drops `fax` and `tiff`, and keeps `image` only if `image_stream` still
    needs it;
  - `sheaf-ops` drops `png` and `tiff`.
- Tests:
  - move the decode tests with the code;
  - the `sheaf-cli` `images`, `combine` and `export` tests and the `sheaf-ops` export
    tests must pass unchanged, since they prove the bytes on disk didn't change;
  - add a round-trip test (encode G4 TIFF, decode, same pixels).

### 4. Tidy-up
- Crate docs in `sheaf-image/src/lib.rs` state the boundary rule and its forbidden
  dependencies.
- Add an `m8.md` note pointing here.
- Optionally, a CI check that `cargo tree -p sheaf-image` contains no `lopdf`, `pdfium`,
  `sheaf-ocr` or `gpui`.

## Not moving, and why
| Code | Why it stays |
|---|---|
| `Renderer`, `Job`, `RenderRequest`, `RenderHandle` (`sheaf-render`) | Document-level: pages, outline, search, text |
| `PageSource` (`sheaf-ocr/src/job.rs`) | Needs page sizes and text length |
| `orient`, `rectify` (`sheaf-ocr`) | ONNX/Tesseract models |
| `orientation_votes`, `scan`, `clean_one` (`sheaf_ops::scan`) | Call OCR; write PDFs |
| `to_pdf`, `image_stream`, `placement` (`sheaf-doc`) | Build PDF objects |
| Raster to `RenderImage` (`docview.rs`), `shrink`/`outline` (`scan_dialog.rs`) | gpui, or tiny preview helpers |
| `image_bits` (duplicate-pages plugin) | Builds to wasm; `imageproc`'s rayon feature would need gating for 30 lines |
| `brand.rs`, `examples/gen_icons.rs` | App assets |

## Slice notes
1. Done. `crates/sheaf-scan` is now `crates/sheaf-image`: `clean.rs` (was
   `lib.rs`, tests in `clean/tests.rs`) and `blank.rs` (the pixel check and its tests,
   from `sheaf_ops::blank`). Nothing is re-exported. Callers name
   `sheaf_image::clean::…` and `sheaf_image::blank::Sensitivity`. The older plans
   (`cleanup-cli.md`, `blanks.md`) gain a pointer here instead of being rewritten.
2. Done. `sheaf_image::raster` has `PixelFormat`, `Raster` (`to_gray`, `to_rgb`,
   `into_rgba`, each `None` when the data is short), `luma` (BT.601 in integers) and
   `content_bounds`. Every hand-written copy goes through them:
   - OCR's `OcrImage::from_raster`;
   - the blank detector's render;
   - auto-crop's `content_margins`;
   - export's PNG, JPEG and TIFF paths (gray TIFF moves from Rec.709 to BT.601);
   - two lumas inside `clean`;
   - the `scanify` example and an OCR test.

   Not changed: the app's `to_render_image` (BGRA passthrough to gpui) and the print
   paths. Checked with workspace tests, clippy, and the ten eval photos (same result,
   none dropped).
3. Done.
   - **`sheaf_image::io`:** `PageImage`, `Pixels`, `read`, `read_bytes`,
     `is_image_path`, `DEFAULT_DPI` and the JPEG, PNG and TIFF parsing, from
     `sheaf-doc`. `load` (upright RGB) moves there from `clean`.
   - **`sheaf_image::encode`:**
     - `g4`, from `sheaf-doc`;
     - `png` and `jpeg` (each recording its DPI);
     - `TiffWriter`/`TiffPage`, which writes a page at a time in color, gray or G4,
       with resolution and page-number tags.

     All of it came out of `export.rs`, which now only renders, names and
     temp-file-renames.
   - **What `sheaf_doc::images` keeps:** `PageFit`, `Placement`, `placement`,
     `to_pdf`, `image_stream` and `files_to_pdf`.
   - **Dependencies:** `sheaf-doc` drops `image`, `tiff` and `fax`; `sheaf-ops` drops
     `png` and `tiff`.
   - **No re-exports:** callers in `sheaf-ops`, `sheaf-app` and `sheaf-doc`'s
     `watermark` switched.

   **Bug found by the new TIFF round-trip test:** TIFF import never read the
   resolution. `tiff`'s `get_tag_f32` only accepts FLOAT, and TIFF writes RATIONAL, so
   every TIFF came in at 96 dpi. A 300 dpi scanner TIFF made a page three times too
   big. `io::resolution` now reads RATIONAL (and the other numeric types). Checked end
   to end: `sheaf export -f tiff` at 200 dpi, re-imported with `images-to-pdf
   --page original`, gives 612×792 in all three modes.
