# M5: Scan & OCR — OCR slices (reference)

Status: OCR engine + text layer and batch OCR over a binder implemented 2026-10-12 (Linux). Remaining M5 slices (scan cleanup, image import, scanner intake, separator sheets) are listed in ROADMAP.md.

This is the design that was implemented, kept for reference the way `plugins.md` is for M6. Line numbers were accurate at the time of writing.

## Coordinate contract (decide first)

Annots live in **source-displayed space**: points, top-left origin, after the
file's own `/Rotate`, *before* `PageRef.rotation` (`annot.rs:5-8`). PDFium
hits arrive in that space; docview rotates both by `pref.rotation` at draw
time (`rotate_rect`, `docview.rs:2452, 2857`). The text layer uses the **same
space**, so save (`PageGeom::to_user`) and the search merge need no conversion.

The engine sees the page rendered with `Job::Page{rotation: pref.rotation}`
(fully displayed frame). The layer records that frame as `upright: Rotation`
and stores word rects unrotated into source-displayed space. Rotating the
page later doesn't move glyphs; only the frame in which text reads upright
changes, which is exactly what the save path needs. This also settles the
rotate/undo interplay: `RotatePages` never touches layers.

---

## Part A — Document model, save, render, search

### A1. Types and edits — `crates/sheaf-doc/src/text_layer.rs` (new), `model.rs`

```rust
pub struct OcrWord { pub text: String, pub rect: Box2 /* source-displayed */, pub confidence: f32 /* 0..=1 */ }
pub struct TextLayer {
    pub words: Vec<OcrWord>,        // sorted by line (top→bottom in upright frame), then x
    pub lines: Vec<Range<usize>>,   // derived at construction
    pub upright: Rotation,          // PageRef.rotation at OCR time
    pub page: PageSize,             // source-displayed size
    pub engine: String,             // "tesseract 5.5.2" / "oar-ocr pp-ocrv5 en"
}
impl TextLayer {
    pub fn from_engine(words: Vec<OcrWord> /* rendered frame, points */, rendered: PageSize, upright: Rotation, engine: &str) -> Self;
    pub fn is_empty(&self) -> bool;
    pub fn byte_estimate(&self) -> usize;
    pub fn boxes_in_rect(&self, rect: Box2) -> Vec<Box2>;   // follow-up: highlight tool
}
```
No serde (recovery goes through the incremental section). `PartialEq` because `DocState` derives it.

`DocState` (`model.rs:79-85`) gains `text_layers: im::HashMap<PageId, Arc<TextLayer>>`
plus `text_layer(page)`/`has_text_layer(page)`. Construction sites:
`model.rs:318` (`Document::new`), `model.rs:886` (`OutlineMove` rest_state),
`save.rs:77` (`extract_pages`, also `retain` kept ids like annots).

New `Edit` variants (`model.rs:145-229`):
- `SetTextLayers { layers: Vec<(PageId, Arc<TextLayer>)> }` (unknown id = error)
- `ClearTextLayers { ids: Vec<PageId> }` (unsaved layers only)

In `compute` (`model.rs:646`): `DeletePages` removes layers with annots
(:707-709); `DuplicatePages` shares the `Arc` (:756); `ReplacePages` drops
layers of replaced ids (:779). Other edits untouched.

`Document::marginal` (`model.rs:582`): when `text_layers` differ by pointer,
add `byte_estimate()` of entries whose `Arc` changed.

### A2. History coalescing (one undo step per OCR job) — `model.rs`

`Version` gains `job: Option<JobId>`. Add:
```rust
pub struct JobId(u64);
pub fn begin_job(&mut self) -> JobId;
pub fn apply_in_job(&mut self, job: JobId, edit: Edit, label: impl Into<String>) -> Result<EditOutcome>;
pub fn job_is_current(&self, job: JobId) -> bool;   // false ⇒ user undid it; the job cancels itself
```
`apply_in_job` amends the top version in place (state, label, marginal
recomputed vs predecessor) iff `history[cursor].job == Some(job)`,
`cursor + 1 == history.len()`, and `saved != Some(cursor)`; else behaves like
`apply` and tags the pushed version. Skip the no-op state compare on amend;
`trim()` after. Plain `apply` sets `job: None`. A Save mid-job or a foreign
edit splits the job into two undo steps (acceptable, documented).

### A3. Dirty tracking and rebase

**A layer present in `state.text_layers` is unsaved by construction.**
`rebase_to_saved` (`model.rs:431`) clears `text_layers` beside the rotation
reset (:435-446). No baseline map, no new `Snapshot` field; save paths test
`snap.state.text_layers.contains_key(&id)`. Undo across a save still works
(older versions reference the old source and keep their layers). Exports
don't rebase. Recovery needs nothing: `recovery::write` → `incremental_section`
→ `incremental::build`, which writes layers (A4).

### A4. Save: glyphless invisible text layer

Approach: Tesseract's **embedded glyphless Type0 font** (`pdfrenderer.cpp`).
Every CID advances 0.5 em (`/DW 500`), so per-word `Tz` is a closed formula;
UTF-16 text round-trips any script; every Tesseract PDF already uses it.
Rejected: unembedded Helvetica/WinAnsi (needs AFM widths, Latin-1 only).

Vendored asset: `crates/sheaf-doc/assets/pdf.ttf` (tesseract repo
`tessdata/pdf.ttf`, a few hundred bytes, Apache-2.0) via `include_bytes!`,
plus `assets/LICENSE-pdf.ttf`.

`text_layer.rs` (`pub(crate)`):
- `ocr_font(pdf, cache: &mut Option<OcrFont>) -> ObjectId`, lazy, once per file:
  Type0 `/BaseFont /GlyphLessFont /Encoding /Identity-H /ToUnicode <cmap>`;
  CIDFontType2 `/CIDSystemInfo (Adobe/Identity/0) /DW 500 /CIDToGIDMap <65536 × 0x0001>`;
  FontDescriptor `/Flags 5 /FontBBox [0 0 500 1000] /Ascent 1000 /Descent -1 /CapHeight 1000 /ItalicAngle 0 /StemV 80 /FontFile2 <pdf.ttf, /Length1>`;
  ToUnicode CMap: codespace `<0000> <FFFF>`, one identity `bfrange`.
  `Stream::compress()` at creation on c2g/ff2/cmap/content (the incremental
  serializer writes streams as stored, `incremental.rs:335-341`).
- `to_content(layer, geom: &PageGeom) -> Vec<u8>`: `Q`, `BT 3 Tr`, per word
  `/SheafOcr fs Tf` (only when fs changes), `a b c d ox oy Tm`, `tz Tz`,
  `<UTF-16BE hex> Tj`; between words on a line with gap > 0.1 pt:
  `tz_gap Tz <0020> Tj`; `ET`. Geometry: rotate rect into upright frame F,
  `fs = h_F`, `w = w_F`, origin = F bottom-left unrotated and `geom.to_user`'d;
  `tz = 200·w/(fs·n)` (n = UTF-16 units), `tz_gap = 200·gap/fs`.
  θ = `(geom.rotate + upright.degrees()).rem_euclid(360)`, matrix
  `[cos θ, sin θ, −sin θ, cos θ]` (form of `annot.rs:500-509`):
  θ=0 `1 0 0 1`; 90 `0 1 -1 0`; 180 `-1 0 0 -1`; 270 `0 -1 1 0`.
  Page `/Rotate` written is `original + pref.rotation` (`save.rs:376`); text
  direction uses `original + upright`.
- `install(pdf, page: &mut Dictionary, layer, geom, font, q)` / `strip(pdf, page)`:
  - **Contents**: never decode originals. `/Contents` becomes
    `[<q>, …original…, <ocr>]`; `<q>` is a one-per-file stream `q`, `<ocr>`
    starts with `Q` (legal per PDF 32000 7.8.2; OCRmyPDF's trick). Cases:
    missing → `[q, ocr]`; ref to stream → `[q, ref, ocr]`; array → clone and
    wrap. Both streams carry `/SheafOcrLayer true` so `strip` removes them
    (collapse single-entry array to ref): re-OCR replaces, never doubles.
  - **Resources**: mirror `flatten_annots` (`save.rs:255-270`): deref+clone
    `/Resources` and its `/Font`, set `SheafOcr => Ref(type0)`, write back
    inline. `SheafOcr2…` if the name is taken by a foreign font.

Call sites:
- `build_rewrite` (`save.rs:315-425`): after annots (:368-375), before
  rotation (:376). Also when `clean == true`. `export_flattened` and
  `extract_pages` inherit it.
- `incremental::build` (`incremental.rs:34-268`): tree-changed branch after
  annots (:111-116); unchanged-order branch extends the condition at :143
  with `|| text_layers.contains_key(&pref.id)` and, when installing, pulls
  inherited `/Resources`, `/MediaBox`, `/CropBox` from `primary.inherited`.
  Geom via `file::page_geom` (:154). New objects are above `orig_max_id`.

**Pre-existing hazard to pin with a test:** duplicated primary page reads
`pdf.get_dictionary(src_page)` (`save.rs:348`) after the first copy wrote
back (:389), so `original` (:363) may already include the rotation delta.
Fix: capture original dicts before mutation.

### A5. Render crate — `crates/sheaf-render/src/lib.rs`

- `Renderer::page_text_len(&self, page) -> Result<usize>`: non-generated,
  non-whitespace chars of `p.text()?.chars()`. 0 ⇒ OCR candidate.
- `Msg::TextLen { page, reply }` (:643), `RenderHandle::page_text_len(page) -> oneshot::Receiver<Result<usize>>`,
  handled inline in `worker_loop` like `Msg::TextBoxes` (:858).
- `pdfium::bind` (:171-204): add `CARGO_MANIFEST_DIR/../../vendor/pdfium/<os>-<arch>`
  fallback so `cargo test` run from a crate dir finds PDFium.

### A6. Model-side search and the docview merge

`text_layer.rs` (add `regex.workspace = true` to `sheaf-doc/Cargo.toml`):
```rust
pub struct TextHit { pub rects: Vec<Box2>, pub text: String }
pub fn search(layer: &TextLayer, query: &str, match_case: bool) -> Vec<TextHit>;
pub fn pattern_search(layer: &TextLayer, re: &Regex) -> Vec<TextHit>;
```
Mirror `pattern_search_page` (`lib.rs:451-538`): line grouping at
construction (sort by vertical centre; join line when vertical overlap ≥ 50 %
of the smaller height; sort by x); page string = words joined by one space
per line, lines by `\n`; char→word ownership; case-folded per-char compare;
per-word rect = proportional slice of the word box (uniform `Tz` ⇒ PDFium
reports the same after save), unioned per line, unrotated to source space.

Docview contract:
- **Layer hits are authoritative for their page**: PDFium hits for a page
  with a layer are dropped on receipt. After save+rebase layers are gone and
  the new handle finds baked text. No double counting.
- `start_search` (`docview.rs:2079`) / `start_pattern_search`
  (`pattern_panel.rs:114`): compute layer hits up front, seed `hits`, stream
  PDFium events with the filter.
- Hit addressing refactor: today `Search.hits` are keyed by primary-source
  page index and drawing is gated on `pref.source == primary`
  (`docview.rs:2850-2853, 2887`). Switch to `Hit { page: PageId, rects, text }`,
  convert PDFium hits via `index_of_source_page` (`docview.rs:262`), sort by
  display index, draw by `PageId`. (Fallback if deferred: primary pages only.)
- Follow-up: highlight tool (`markup_tools.rs:615`) uses `layer.boxes_in_rect`.

---

## Part B — Engine crate, job pipeline, UI, packaging

### B1. Crate `crates/sheaf-ocr` (no GPUI)

```
src/lib.rs       re-exports; pub mod job
src/engine.rs    OcrEngine, OcrWord(px), OcrImage, InputKind, EngineInfo, EngineKind, EngineError, engines(), check_engine(), build_engine()
src/config.rs    OcrConfig + sub-structs (serde, defaults), page_range::parse
src/data.rs      data_dir() resolution, tessdata language listing, oar model-set listing
src/geom.rs      PxToPt, words_from_line (oar line→word grouping)
src/tesseract.rs #[cfg(feature = "tesseract")]
src/oar.rs       #[cfg(feature = "oar")]
src/job.rs       pipeline: start(), JobHandle, OcrEvent, PageSource trait
tests/engines.rs #[ignore] smoke tests + tests/fixtures/hello-300dpi.png
```
Cargo: features `tesseract = ["dep:tesseract-rs"]` (0.4.0, default feature
`build-tesseract`), `oar = ["dep:oar-ocr"]` (0.10.0, defaults
`download-binaries` + `simd`; pins `ort = "=2.0.0-rc.13"`). Deps:
sheaf-core, sheaf-render (`Raster`, `RenderHandle`), sheaf-doc (`TextLayer`,
ids), image, serde, anyhow, thiserror, log, futures. Root `Cargo.toml`: add
member + workspace dep. `sheaf-app/Cargo.toml`:
`default = ["ocr-tesseract", "ocr-oar"]`, each forwarding to `sheaf-ocr/*`.
`sheaf-ocr` with no features still compiles (dialog reports "no engine
compiled in"); CI checks `cargo check -p sheaf-app --no-default-features`.

Gray conversion lives in sheaf-ocr: `OcrImage::from_raster(&Raster, InputKind)`,
luma `(77R + 150G + 29B) >> 8`, handles Rgba/Bgra. Converted once in the
coordinator so the 4-byte buffer (33 MB at 300 dpi Letter) is dropped early.

### B2. Engine trait and engines

```rust
pub struct OcrWord { pub text: String, pub bbox_px: [f32; 4] /* l,t,r,b */, pub confidence: f32 }
pub enum InputKind { Gray, Rgb }
pub enum OcrImage { Gray(GrayImage), Rgb(RgbImage) }
#[serde(rename_all = "lowercase")] pub enum EngineKind { Tesseract, Oar }
pub trait OcrEngine: Send {
    fn input(&self) -> InputKind;
    fn recognize(&mut self, img: OcrImage, dpi: u32) -> anyhow::Result<Vec<OcrWord>>;  // by value: oar wants owned RgbImage
    fn describe(&self) -> EngineInfo;
}
pub enum EngineError { NotCompiled(&'static str), NoAvx2, MissingData(String), Init(String) }  // Display = user-facing text
pub fn engines() -> Vec<EngineKind>;                                   // compiled-in
pub fn check_engine(kind, &OcrConfig) -> Result<(), EngineError>;     // cheap: files exist + AVX2; dialog greys out
pub fn build_engine(kind, &OcrConfig) -> Result<Box<dyn OcrEngine>, EngineError>;  // one per worker thread
```
`check_engine(Oar)` runs `is_x86_feature_detected!("avx2")` before anything touches ort.

**Tesseract** (`tesseract.rs`, verified 0.4.0 API): `TesseractAPI::new()`,
`init(tessdata_dir, lang)`, `set_page_seg_mode` (default PSM 3 auto),
`set_source_resolution(dpi)`, `set_image(gray, w, h, 1, w)`, `recognize()`,
`get_iterator()` → loop `get_word_with_bounds() -> (text, l, t, r, b, conf)` /
`next_word()`; skip blank words; `conf/100`; `clear()` after each page. Data:
`cfg.tesseract.tessdata_dir` else `data_dir()/tessdata`; require
`<lang>.traineddata` (`MissingData` names the path and the fetch script).
Languages = `*.traineddata` in the dir. One instance per worker thread.

**oar-ocr** (`oar.rs`, verified 0.10.0 API): `OAROCRBuilder::new(det, rec, dict)`
`.with_document_image_orientation_classification(..)` (if `use_doc_orientation`)
`.with_text_line_orientation_classification(..)` `.return_word_box(true)`
`.image_batch_size(1)` `.ort_session(intra-op threads = 2)` `.build()`;
`predict(vec![rgb])` → `text_regions: Vec<TextRegion>` (one per LINE:
`bounding_box` polygon, `text`, `confidence`, `word_boxes: Option<Vec<BoundingBox>>`).
Do **not** enable rectification (boxes would be in rectified coordinates).
Model sets: `"en"` → `en_pp-ocrv5_mobile_rec.onnx` + `ppocrv5_en_dict.txt`;
`"multi"` → `pp-ocrv5_mobile_rec.onnx` + `ppocrv5_dict.txt`; det
`pp-ocrv5_mobile_det.onnx`; textline ori `pp-lcnet_x0_25_textline_ori.onnx`;
doc ori `pp-lcnet_x1_0_doc_ori.onnx`; explicit path overrides win.
`BoundingBox`'s concrete shape: read `oar_ocr::domain` at implementation time.

**geom.rs**: `PxToPt::new(page_pts_displayed, raster_w, raster_h)` using the
raster's actual dimensions; `words_from_line(text, line_bbox, word_boxes, conf)`:
(a) boxes == tokens → pair; (b) boxes == non-space chars → per-char union per
token; (c) else one word = whole line. Pipeline converts px→pt in the
rendered frame then `TextLayer::from_engine(words_pt, rendered_size, upright = page.rotation, engine)`.

**data.rs**: `data_dir()` mirrors pdfium `bind()`: `$SHEAF_OCR_DATA_DIR`,
`<exe_dir>/ocr`, `vendor/ocr` (cwd), `CARGO_MANIFEST_DIR/../../vendor/ocr` under test.

### B3. Job pipeline — `sheaf_ocr::job`

Dedicated `std::thread`s (not GPUI's executor, which also serves saves,
searches, tile uploads).
```rust
pub struct OcrPage { id: PageId, source: SourceId, index: u32, rotation: Rotation, size_pts: PageSize /* displayed */ }
pub struct OcrRequest { pages: Vec<OcrPage>, skip_pages_with_text: bool, engine: EngineKind, config: OcrConfig }
pub trait PageSource: Send + Sync + 'static {        // impl for RenderHandle; faked in tests
    fn text_len(&self, page: u32) -> oneshot::Receiver<Result<usize>>;
    fn render(&self, page: u32, width_px: u32, rotation: Rotation) -> oneshot::Receiver<Result<Raster>>;
}
pub enum OcrEvent { Started{total, engine}, Skipped{id}, Page{id, layer: TextLayer, words}, Failed{id, index, error}, Finished{done, skipped, failed, cancelled}, Aborted(String) }
pub struct JobHandle { cancel: Arc<AtomicBool>, workers: Vec<JoinHandle<()>> }   // Drop cancels
pub fn start(req: OcrRequest, sources: HashMap<SourceId, Arc<dyn PageSource>>) -> (JobHandle, UnboundedReceiver<OcrEvent>);
```
1. **Coordinator thread**: builds one engine first to fail fast (`Aborted`).
   Per page in order: if skipping, `text_len ≥ 20` → `Skipped`; else render
   `width_px = round(w_pt·dpi/72)`, `Rgba`, priority 255 (lowest; viewport
   tiles win), ≤ 2 renders in flight per source; convert to `InputKind`; push
   to a bounded `sync_channel(N)`. Checks `cancel` between pages.
2. **N workers** (`N = clamp(available_parallelism/2, 1, 4)`, oar capped at 2
   for memory): each builds its own engine; recv → `recognize` → `PxToPt` →
   `TextLayer::from_engine` → `Page`; errors → `Failed`, continue. If all
   engines fail to build → `Aborted`.
3. **Finish**: close queue, join, `Finished`.
Tests with `FakeSource` + `MockEngine`: skipped events; page 3 fails and job
continues; cancel after 2 events → `Finished{cancelled: true}`; px→pt for a rotated size.

### B4. UI

**Actions** (`main.rs:35-70`, docview group): `RecognizeText`, `CancelOcr`;
`actions!(ocr_dialog, [DialogNext, DialogPrev, DialogLeft, DialogRight])`.
Bindings: `ctrl-shift-t` → `RecognizeText` (`ws`; unused today, Acrobat has
no default); dialog `tab`/`shift-tab`, `left`/`right`/`space`, `enter` run,
`escape` dismiss, context `"OcrDialog && !TextInput"`.

**Menu** (`menus.rs` Document menu): after Delete Pages: `sep()`,
`item("Recognize &Text (OCR)…", RecognizeText)`, `item("Cancel OC&R", CancelOcr)`.
Palette picks both up automatically. `CancelOcr` listener registered on the
docview element only `.when(self.ocr.is_some(), ..)` so the menu greys it;
`RecognizeText` on the Workspace element (owns `Settings` and the dialog, like `toggle_palette`).

**Dialog** — new `crates/sheaf-app/src/ocr_dialog.rs`, `Entity<OcrDialog>` in
`Workspace.ocr_dialog`, drawn via `deferred(..).with_priority(2)` with an
occluding backdrop (palette + `popup_menu` patterns), card styling from
`locked_card`. Input: page count, current page, selection count, `OcrConfig`,
per-engine `check_engine` results (greyed rows show the error). Rows:
- Pages: All / Current page / Selected pages (disabled if none) / Range `[TextInput]`
- ☑ Skip pages that already have text (default on; All + skip = "pages without text")
- Engine: Tesseract / PaddleOCR (oar-ocr); default from settings
- Language: tessdata names, or oar "English"/"Multilingual" sets present
- Resolution: 300 dpi (150–600)
- Recognize / Cancel. Emits `Run(spec) | Dismiss`. On Run: persist choices to
  `settings.ocr`, then `doc.start_ocr(spec, config, cx)`.

**Run state** — new `crates/sheaf-app/src/ocr_run.rs` (`impl DocView`, like
`pattern_panel.rs`): `DocView.ocr: Option<OcrRun { handle, _task, engine, total, done, skipped, failed: Vec<(u32, String)>, pending: Vec<(PageId, Arc<TextLayer>)>, last_flush, job: JobId }>`.
`start_ocr`: refuse if saving or running; build `OcrPage`s from scope
(`target_ids()` :877-896 for Selected/Current; `page_range::parse` for
Range; sizes rotated by `pref.rotation`); sources = `doc.handles` as
`Arc<dyn PageSource>`; `job = doc.model.begin_job()`; `_task = cx.spawn(... while let Some(ev) = rx.next().await ...)`
(search-task shape, `docview.rs:2102-2144`). `on_ocr_event`: buffer `Page`
layers; flush when ≥ 10 pending or 2 s elapsed or `Finished`:
`doc.model.apply_in_job(job, Edit::SetTextLayers{..}, "Recognize text")` then
the `apply` tail (`touched`, `schedule_autosave`, `notify`); if
`!job_is_current(job)` → cancel. `Aborted` → notice "OCR failed: …".
Status bar: `"OCR 12/400 — skipped 30 — tesseract — Esc to cancel"`;
done: `"OCR done: 370 pages, 30 skipped, 2 failed (p. 17, 211) — Ctrl+Z undoes"`.
Thin 2 px progress strip above the status row. Esc: extend the `CloseSearch`
chain (`docview.rs:3131-3145`). Tab close drops `OcrRun` → cancel.

### B5. Settings — `settings.rs`

`Settings { window, docks, #[serde(default)] ocr: sheaf_ocr::OcrConfig }`.
```rust
#[serde(default)] pub struct OcrConfig { engine: EngineKind /* Tesseract */, dpi: u32 /* 300 */, skip_pages_with_text: bool /* true */, last_scope: String, tesseract: TesseractConfig, oar: OarConfig }
pub struct TesseractConfig { tessdata_dir: Option<PathBuf>, lang: String /* "eng" */, psm: u8 /* 3 */ }
pub struct OarConfig { model_set: String /* "en" */, det/rec/dict/textline_ori/doc_ori: Option<PathBuf>, use_doc_orientation: bool /* true */ }
```

### B6. Data fetch, packaging, CI

- New `scripts/fetch-ocr-data.sh` (shape of `fetch-pdfium.sh`, sha256-pinned):
  `vendor/ocr/tessdata/eng.traineddata` (tessdata_fast, 4.1 MB) + LICENSE;
  `vendor/ocr/models/{pp-ocrv5_mobile_det.onnx, en_pp-ocrv5_mobile_rec.onnx, ppocrv5_en_dict.txt, pp-lcnet_x0_25_textline_ori.onnx, pp-lcnet_x1_0_doc_ori.onnx}`
  from `github.com/GreatV/oar-ocr/releases/download/v0.3.0/`; `--multilingual` adds the multi rec + dict.
- Commit license texts under `packaging/licenses/` (Tesseract Apache-2.0,
  Leptonica BSD-2, ONNX Runtime MIT, PaddleOCR Apache-2.0, tessdata Apache-2.0,
  pdf.ttf Apache-2.0, `NOTICE-ocr.txt` with versions).
- `scripts/package.sh`: copy `vendor/ocr` → `<stage>/ocr/`; copy `packaging/licenses/*`.
- `ci.yml` (+ `release.yml`): cache `~/.tesseract-rs` / `%APPDATA%/tesseract-rs`
  keyed `tesseract-rs-0.4.0`; run `fetch-ocr-data.sh`; `cargo test --workspace`;
  `cargo test -p sheaf-ocr -- --ignored` (engine smoke tests);
  `cargo check -p sheaf-app --no-default-features`; Linux gate:
  `! ldd target/debug/sheaf | grep -E 'libc\+\+|libunwind|onnxruntime|tesseract|leptonica'`;
  Linux AVX2 gate: `qemu-x86_64 -cpu Nehalem target/debug/sheaf --version`
  (add `--version` to `parse_cli`, exits before any window) to prove ORT's
  static initialisers don't SIGILL on pre-Haswell CPUs.
- Build-time network: tesseract-rs fetches Tesseract/Leptonica sources (+ tessdata_best eng/tur, ignored); ort fetches pyke binaries. `--no-default-features` is the offline/fast path.

### B7. Docs and roadmap edits

- `README.md`: Features "OCR (searchable text layer)" → ✅ with engines and
  menu path; Key-decisions "Text / OCR" row updated; Milestones: M5 →
  `**Scan & OCR**: engine trait, Tesseract + oar-ocr, batch OCR with progress/cancel, invisible text layer on save, search over OCR text. *OCR slices done ____; scan slices planned.*`,
  M6 → Plugins (link `todos/plugins.md`); fix the "plugins (M5) and OCR (M6)"
  paragraph and the "M6 onward" line; Building: fetch-ocr-data step, CMake +
  C++17 note (VS 2022 ships CMake), first build needs network,
  `--no-default-features`, AVX2 requirement for the PaddleOCR engine;
  Configuration: `ocr` block and `SHEAF_OCR_DATA_DIR`; contexts list gains
  `OcrDialog`; licenses list.
- `ROADMAP.md`: swap M5/M6 rows and sections; drop the "Decide at kickoff"
  paragraph (decided: Tesseract + oar-ocr, in-process behind a trait); keep
  cleanup/image import/scanner/separator slices under M5 as later slices.
- `todos/plugins.md`: title → M6; timing bullet → "after OCR"; remove the
  "OCR as a WASM plugin" out-of-scope bullet; add host interface `ocr`
  (`recognize(page, options) -> text-layer`, `page-text-layer(page)`, proxied
  to `sheaf-ocr::job`); replace example plugins with (1) tax-form
  auto-bookmarker and (2) amount cross-reference → connectors; refresh stale
  line refs (`main.rs:35-128`, `workspace.rs:78-125`).
- New `todos/ocr.md`: this plan's content as the milestone reference (same
  role `plugins.md` plays), including the deferred scan slices.
- `docs/document-model.md`: "Text layers" subsection (storage, dirty rule,
  coalesced jobs, coordinate contract).

---

## Implementation order (slices)

1. **Model + save** (Part A1–A4, A7 model/save tests), `pdf.ttf` vendored.
   Deliverable: `Edit::SetTextLayers` round-trips through rewrite and
   incremental saves; lopdf-level tests pass.
2. **Render + PDFium proof** (A5, `sheaf-render/tests/ocr_layer.rs`).
   Deliverable: PDFium finds "Hello" at the right rect in a saved layer, incl. rotations.
3. **sheaf-ocr crate** (B1–B3): trait, geom, config, data, job pipeline with
   mock tests; then Tesseract engine; then oar engine; `fetch-ocr-data.sh`;
   `#[ignore]` smoke tests pass locally.
4. **App wiring** (B4–B5): actions, menu, dialog, run state, status bar,
   settings. Deliverable: OCR a scanned binder from the menu, one Ctrl+Z.
5. **Search merge** (A6) incl. the `PageId` hit-addressing refactor.
   Deliverable: Find/Patterns hit OCR text before save and after.
6. **Packaging, CI, docs** (B6–B7).

## Verification

- `cargo test --workspace` green on Linux (and Windows in CI), including the
  new sheaf-doc, sheaf-render and sheaf-ocr tests; `cargo test -p sheaf-ocr -- --ignored`
  with fetched data; `cargo clippy --workspace --all-targets -D warnings`;
  `cargo check -p sheaf-app --no-default-features`.
- Manual, on a scanned (image-only) multi-page PDF on the Linux filesystem:
  1. `scripts/fetch-ocr-data.sh`; `cargo run -p sheaf-app -- scan.pdf`.
  2. Document > Recognize Text (OCR)… → All pages, skip-with-text on,
     Tesseract → Recognize. Status bar counts up; scrolling stays smooth;
     Esc cancels and keeps completed pages.
  3. Ctrl+F for a word on an OCR'd page: hit highlighted at the glyphs. Patterns panel finds an SSN/amount.
  4. Ctrl+Z once removes all layers (Find no longer hits); Ctrl+Shift+Z restores.
  5. Ctrl+S (incremental): file grows by a small delta; reopen in sheaf and
     in Acrobat/Chrome: text selectable/searchable, nothing visible changed.
  6. Rotate a page, OCR it, save, reopen: text selection follows the glyphs.
  7. Re-run OCR on the saved file with skip off: no doubled text (strip works).
  8. Repeat 2–5 with the PaddleOCR engine; compare accuracy on a tax form.
  9. Kill the app mid-session after OCR; relaunch: recovery offers the layers.
- Package: `scripts/package.sh` bundle contains `ocr/` and licenses; the
  unpacked `sheaf` finds data beside the binary (no `vendor/` cwd).

## Risks

- **AVX2 / x86-64-v3 static ORT**: a SIGILL in static initialisers would
  break the whole app on pre-Haswell CPUs (README targets older hardware).
  The qemu gate decides; fallback is `ort/load-dynamic` with Microsoft's
  generic `onnxruntime` shared lib fetched into `ocr/lib/` and loaded only
  when the engine is chosen (pdfium's dlopen model).
- **Linux libc++** from pyke's Clang builds may be dynamic: the `ldd` gate decides; same fallback.
- **Build time/network**: Tesseract+Leptonica from source and ORT downloads
  add 5–10 min cold; Windows NMake path is the least exercised (CI has not
  yet run on Windows). `--no-default-features` must stay green.
- **ort `=2.0.0-rc.13` pin** via oar-ocr; nothing else uses ort today.
- **oar word boxes** per word vs per char unverified; grouping rule (a/b/c) handles both.
- **PDFium glyph boxes for a glyphless font** rely on its font-size fallback (Tesseract PDFs search fine in Chrome); the sheaf-render test pins it.
- **Unbalanced `q` in original content** would leave a CTM applied to our text (same exposure as `flatten_annots`).
- **Hit addressing by primary index** blocks layer search on inserted pages until the `PageId` refactor in slice 5.

## Critical files

- `crates/sheaf-doc/src/{text_layer.rs (new), model.rs, save.rs, incremental.rs, annot.rs (PageGeom reuse)}`, `crates/sheaf-doc/assets/pdf.ttf`
- `crates/sheaf-render/src/lib.rs`, `crates/sheaf-render/tests/ocr_layer.rs` (new)
- `crates/sheaf-ocr/**` (new)
- `crates/sheaf-app/src/{main.rs, menus.rs, workspace.rs, docview.rs, settings.rs, pattern_panel.rs, ocr_dialog.rs (new), ocr_run.rs (new)}`, `crates/sheaf-app/Cargo.toml`
- `Cargo.toml`, `scripts/{fetch-ocr-data.sh (new), package.sh}`, `packaging/licenses/` (new), `.github/workflows/{ci.yml, release.yml}`
- `README.md`, `ROADMAP.md`, `todos/{plugins.md, ocr.md (new)}`, `docs/document-model.md`
