<img src="assets/icons/sheaf-f2-transparent.svg" alt="" width="96" align="right">

# sheaf

A fast, native PDF editor. Rust + GPUI.

Sheaf aims for feature parity with Adobe Acrobat Pro's editing tools, plus the
workpaper tooling from CCH ProSystem fx **PDFlyer** (tickmarks, connectors,
calculator tapes, bookmark-driven page management) — without the bloat.

> **Status:** pre-alpha. Nothing works yet. This README is the design doc.

## One document per window

Every open PDF gets its own window; there are no document tabs. Switching
and arranging documents is left to the OS window manager, and sheaf
spends its effort on PDF features instead.

- **File → Open…** (`Ctrl+O`): select one or more PDFs. The first fills the
  current window if it's empty; each of the rest opens in a new window. A
  file that's already open brings its window forward instead.
- **File → New Window** (`Ctrl+N`): open an empty window.
- **File → Close** (`Ctrl+W`) closes the document and its window; the last
  window stays open, empty. **Close Window** (`Ctrl+Shift+W`) or the window
  close button always closes the window. `Ctrl+Q` quits across all windows.
  Unsaved documents prompt before closing; Cancel or a failed save stops
  the close/quit sequence.

From the command line, `sheaf one.pdf two.pdf three.pdf` opens three windows.

## Why

Acrobat has become slow enough that routine work — opening a 400-page scanned
workpaper binder, reordering pages, dropping a tickmark — is painful,
especially on older hardware. PDFlyer makes the workflow tolerable but is
locked to 32-bit Acrobat on Windows.

Sheaf's bet is that a PDF editor written in Rust, with a GPU-accelerated UI and
a render pipeline designed around tiling and caching, can be an order of
magnitude snappier than Acrobat on the same machine.

## Goals

1. **Fast.** The real test is qualitative: does it feel obviously snappier
   than Acrobat on the same (older) machine? The numbers below are directional
   targets to keep us honest about trends, not release gates — at least until
   there's a working viewer to measure:
   - Cold start to empty window: **< 300 ms**
   - Open a 1,000-page scanned PDF to first painted page: **< 500 ms**
   - Page flip (cached): **< 1 frame**; (uncached): first tile visible < 100 ms
   - Idle CPU: **0%**. No background daemons, no updater, no telemetry.
   - Memory: bounded tile cache (configurable, default ~256 MB), not
     "load every page bitmap and hope".
2. **Correct.** Round-trip arbitrary PDFs without corrupting them. Incremental
   saves by default so the original bytes are preserved.
3. **Acrobat Pro parity** for the editing feature set (see [Features](#features)).
4. **PDFlyer parity** for the tax/audit workpaper workflow.
5. **Keyboard-first.** Every action has a binding; the mouse is optional.
6. **Native, single binary.** No Electron, no browser engine, no runtime.

### Non-goals (for now)

- Cloud / collaboration / "Studio Sessions"-style review.
- Mobile.
- Being a PDF *library* for other programs (though the core crates may be
  reusable).
- Pixel-perfect rendering of every exotic PDF on day one. We lean on a
  mature renderer for that.

## Platforms

| Platform | Tier | Notes |
|---|---|---|
| Windows (x64) | **1** | Primary daily-driver target. Must be fast on older hardware. |
| Linux (x64, Wayland + X11) | **1** | Development platform. GPUI uses wgpu here. |
| macOS (arm64) | 2 | Should mostly fall out of the Linux work; not blocking releases. |

Tier 1 platforms gate every release and run the perf budgets in CI.

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│  sheaf-app          GPUI application, views, commands    │
├─────────────────────────────────────────────────────────┤
│  sheaf-ui           Reusable widgets: page canvas, tile  │
│                     viewport, sidebars, command palette  │
├────────────────────────────┬────────────────────────────┤
│  sheaf-doc                 │  sheaf-render              │
│  Document model + edits    │  Page → tiles, cache,      │
│  (pages, outline, annots,  │  background render pool    │
│  undo/redo, incremental    │                            │
│  save)                     │  trait Renderer            │
│                            │   └ pdfium (MVP)           │
│  lopdf / own object layer  │   └ hayro (later, pure Rust)│
├────────────────────────────┴────────────────────────────┤
│  sheaf-core         Shared types: PageId, Rect, units,   │
│                     color, coordinate transforms         │
└─────────────────────────────────────────────────────────┘
```

### Key decisions (and why)

| Decision | Choice | Rationale |
|---|---|---|
| UI framework | **GPUI** (`gpui` + `gpui_platform`) | GPU-accelerated, retained-mode, proven in Zed. Pre-1.0 with breaking changes; pin versions. Linux backend is wgpu (Blade was removed Feb 2026). **Needs a spike** — see [Open questions](#open-questions). |
| Rasterizer | **PDFium** via `pdfium-render` | Chrome's engine: fast, BSD-licensed, handles encrypted files, forms, broken PDFs. Hidden behind a `Renderer` trait so a pure-Rust engine (hayro) can replace it once mature. |
| Document edits | **lopdf** (or our own object layer) | PDFium's write API is limited (esp. outlines). Structural edits — page tree, bookmarks, annotations — are done on the raw object graph and saved incrementally. The renderer re-loads from the edited bytes. |
| Rendering strategy | **Tiled, async, cached** | Viewport requests tiles at the current zoom; a worker pool renders them off the UI thread; LRU cache keyed by `(page, zoom, tile)`. Low-res preview tile first, then full-res. This is the single biggest "feels fast" lever. |
| Text / OCR | PDFium text extraction. OCR is a **core feature** behind an `OcrEngine` trait with two engines compiled in by default: **Tesseract 5** (built from source, linked statically) and **PaddleOCR PP-OCRv5** on ONNX Runtime (`oar-ocr`, with Microsoft's runtime library loaded on demand from `ocr/lib`). | Both engines are native C/C++, so OCR can't live in a WASM plugin; the trait keeps engines swappable, and plugins get OCR as a host capability. Judged on scanned tax forms, not purity. |
| Save model | Incremental update by default; "Save As (rewrite)" optional | Preserves originals, fast saves on large files. Clean rewrite available for size reduction / "Export Clean Copy". |
| Undo | Command pattern over the document model | Every edit is a reversible `Edit` enum; undo stack is per-document. |

### Efficiency principles

- Never render what isn't visible. Never render on the UI thread.
- Cache aggressively, bound strictly.
- Lazy-parse: open the xref and page tree, nothing else, until asked.
- No work at idle. If the window isn't focused and nothing changed, 0% CPU.
- Prefer boring data structures. Profile before optimizing.

## Features

Legend: ✅ done · 🔨 in progress · 🎯 MVP · 📋 planned · ❓ undecided

### Viewing & navigation
| Feature | Status |
|---|---|
| Open / render PDF (incl. encrypted, forms, large scans) | ✅ (password prompt pending) |
| Continuous / single / facing page modes | ✅ View > Single Page, Two-Up (each with Continuous), Show Cover Page |
| Zoom (fit page / fit width / custom), rotate view | ✅ rotate view is display only (Ctrl+Shift+Plus/Minus) |
| Thumbnails sidebar (lazy, virtualized) | ✅ |
| Bookmarks (outline) sidebar | ✅ |
| Text search (find in document) | ✅ progressive; ~6 ms/page |
| Pattern search (regex: SSNs, EINs, amounts, dates) — separate tool from Find, shared engine with redaction | ✅ Patterns panel, Ctrl+Shift+F |
| User-defined patterns: add, save, and delete presets; several date formats built in | ✅ `patterns.json`; right-click a preset |
| Multiple documents, one window each | ✅ |
| Full screen / read mode | ✅ View > Full Screen (Ctrl+L; click or arrows turn pages, Esc leaves), Read Mode (Ctrl+H) |
| Page labels (display) | ✅ status bar and thumbnails; View > Go to Page (Ctrl+Shift+N) takes a label or number |
| Rulers, grid, guides | ✅ View > Rulers (Ctrl+R), Grid (Ctrl+U); drag from a ruler for a guide |
| Split view (same document, two places) | ✅ View > Split (Ctrl+\\); F6 switches pane |
| Split view (same doc, two positions) | 📋 |
| Attachments panel | ✅ View > Attachments (Ctrl+Alt+6): open (PDFs in sheaf, others with the system program after a warning) or save out |
| Open PDF Portfolios (browse and extract embedded files) | ✅ opens on its cover sheet with the Attachments panel: folders, metadata columns, open/save |
| Layers (OCG) panel | ✅ View > Layers (Ctrl+Alt+7): show or hide for viewing; never saved |

### Page organization
| Feature | Status |
|---|---|
| Reorder pages (drag in thumbnails) | ✅ |
| Rotate **individual** pages (PDFlyer-style, not whole doc) | ✅ |
| Delete / extract / duplicate pages | ✅ Document > Extract Pages… keeps marks and relevant bookmarks |
| Insert PDF at position (PDFlyer "Insert PDF") | ✅ |
| Merge bookmarks on insert (parents merged, children appended) | ✅ |
| Combine Files: many PDFs and images into one, choosing pages from each | 📋 |
| Split document (by page range, by bookmark, by size) | 📋 |
| Insert blank page / from image / from scanner | 📋 |
| Crop pages | 📋 |
| Page labels (roman numerals, prefixes) | 📋 |
| Headers / footers / Bates numbering | 📋 |
| Watermarks / backgrounds | 📋 |

### Bookmarks (PDFlyer-heavy)
| Feature | Status |
|---|---|
| Create / rename / delete / nest bookmarks | ✅ |
| Move bookmark **with its pages** | ✅ Shift+drag |
| Colorize bookmarks by level / status | ✅ per bookmark; by level 📋 |
| Bookmark from page range | 📋 |
| Target sheets / auto-bookmark from scan separators | ❓ |

### Markup & annotation
| Feature | Status |
|---|---|
| Sticky notes, highlight, underline, strikethrough | ✅ notes, highlight; underline/strikethrough 📋 |
| Free text, shapes (rect, ellipse, line, arrow, polygon) | ✅ polygon 📋 |
| Ink / freehand | ✅ |
| Stamps (built-in + custom) | ✅ tickmark library; custom 📋 |
| **Tickmarks** (PDFlyer): quick-stamp library, keyboard-driven | ✅ |
| **Connectors** (PDFlyer): linked markers that jump between pages | ✅ saved as Link annotations, so they jump in Acrobat too |
| **Calculator tape** (PDFlyer): 10-key calc, tape placed as annotation | ✅ Calculator panel (Ctrl+Alt+4) |
| Comments list / review panel, status, replies | ✅ Comments panel (Ctrl+Alt+5) lists ours and third-party; status/replies 📋 |
| Flatten annotations | ✅ File > Export Flattened Copy… (links and form fields kept) |
| Export Clean Copy (strip annots, notes, tapes, stamps, attachments) | ✅ File > Export Clean Copy; keeps links, form fields, bookmarks |
| Measurement tools (distance, area, perimeter, scale) | 📋 |

### Content editing
| Feature | Status |
|---|---|
| Edit text in place (reflow within block) | 📋 |
| Edit / replace / move images | 📋 |
| Add text / image objects | 📋 |
| Link editing | 📋 |
| Redaction (true content removal, pattern search) | 📋 |
| Header/footer editing | 📋 |

### Forms
| Feature | Status |
|---|---|
| Fill AcroForms | 📋 |
| Create / edit form fields | 📋 |
| XFA | ❌ not planned |

### Conversion & export
| Feature | Status |
|---|---|
| Export pages to PNG / JPEG | 🎯 |
| Export to multi-page TIFF (PDFlyer) | 📋 |
| Create PDF from images | 📋 |
| OCR (searchable text layer) | ✅ Document > Recognize Text (OCR)… (Ctrl+Shift+T): Tesseract or PaddleOCR, whole binder / current / selected / range, skips pages that already have text, progress + Esc to stop, one undo step; Find and Patterns see the text before the first save |
| Export text | 📋 |
| Print | 📋 |
| PDF/A conversion | ❓ |
| Office ↔ PDF | ❌ not planned (use LibreOffice) |

### Security
| Feature | Status |
|---|---|
| Open password-protected PDFs | ✅ prompt on open; saves write a plain (unencrypted) file |
| Set / remove passwords, permissions; preserve encryption on save | 📋 |
| Digital signatures (sign / verify) | 📋 |
| Certificate encryption | ❓ |

### Other Acrobat Pro features
| Feature | Status |
|---|---|
| Compare documents | 📋 |
| Optimize / reduce file size | 📋 |
| Preflight / accessibility checker | ❓ |
| Batch actions / scripting | 📋 (Lua or Rhai, not JS) |
| Create Portfolios | ❌ not planned (opening them is planned) |

## MVP (v0.1): "the fast workpaper binder"

The MVP is **not** an Acrobat clone. It is the smallest thing that lets a tax or
audit preparer stop opening Acrobat for daily binder work, and it is the
vehicle for proving the performance thesis.

A user should be able to:

1. Open a 1,000-page scanned binder and be scrolling in under half a second.
2. Navigate by bookmarks and thumbnails without stutter.
3. Reorder, rotate, delete, and insert pages — including inserting another PDF
   and having its bookmarks merged sensibly.
4. Build and reorganize the bookmark tree, moving pages along with bookmarks.
5. Drop tickmarks and connectors from the keyboard, add notes and highlights,
   and run a calculator tape onto the page.
6. Search text.
7. Save incrementally in milliseconds, and export a clean copy.

Everything marked 🎯 above is in scope. Nothing else is.

### Explicitly deferred from MVP
Content (text/image) editing, OCR, forms, redaction, signatures, TIFF export,
measurement, compare, batch. These are all real Acrobat parity items and all
come after v0.1.

After v0.1 come Scan & OCR (M5) and plugins (M6), which together make v0.2, then the
rest of the path to parity in [ROADMAP.md](ROADMAP.md).

### Milestones

| # | Milestone | Proves |
|---|---|---|
| M0 | **Spikes**: GPUI window on Windows and Linux (both tier 1); pdfium-render renders a page; lopdf round-trips a PDF. *All three pass on Linux and Windows (2026-10-11).* | The stack is viable |
| M1 | **Viewer**: tiled async rendering, continuous scroll, zoom, thumbnails, outline, multiple windows, search. *Done 2026-10-11 (Linux).* | The performance thesis |
| M2 | **Organizer**: page reorder/rotate/delete/insert, bookmark tree editing, incremental save, undo; pattern (regex) search as its own tool. *Done 2026-10-11 (Linux).* | The document model |
| M3 | **Markup**: standard annotations, stamps, tickmarks, connectors, calc tape, comments panel, clean copy export. *Done 2026-10-11 (Linux).* | The PDFlyer workflow |
| M4 | **Polish → v0.1**: unsaved-changes prompts, settings with persisted layout, command palette, user keymap, crash-recovery autosave, packaging + CI. *Done 2026-10-11 (Linux; Windows bundle and CI workflows not yet exercised).* | Daily-driver readiness |
| M5 | **Scan & OCR**: `OcrEngine` trait with Tesseract and PaddleOCR engines, batch OCR over a binder with progress and cancel, invisible text layer written on save, search over OCR text. *OCR slices done 2026-10-12 (Linux; Windows build not yet exercised). Scan cleanup, image import, scanner intake and separator sheets still planned — see [`todos/ocr.md`](todos/ocr.md).* | Scanned binders become searchable |
| M6 | **Plugins**: WASM components (wasmtime, WIT), command registry, declarative forms and panels, document events, storage, OCR as a host capability, permissions with consent, plugin manager, bundled plugins (tax-form bookmarks, amount cross-reference). *Slices done 2026-10-13 (Linux; Windows build not yet exercised) — see [`docs/plugins.md`](docs/plugins.md) and [`todos/plugins.md`](todos/plugins.md).* | Extensibility without forking |

M7 onward (viewing and output through v1.0 parity) is laid out in [ROADMAP.md](ROADMAP.md).

## Open questions

- **GPUI risk.** Pre-1.0, API churn, and we'd be an early non-Zed user on Linux.
  The Linux spike passed first try (wgpu backend, Wayland via WSLg on
  llvmpipe), and Windows built and ran unmodified (DirectX 11, Intel UHD 620).
  Remaining risk is API churn across rev bumps; fallback is still `iced` or
  `egui` if that bites. Open item: launch-to-first-pixels is 1.2–2 s on
  Windows (PDF open itself is ~10 ms) — investigate platform init.
- **One parser or two?** PDFium for rendering + lopdf for editing means two
  parsers of the same bytes. Alternative: do everything through PDFium's
  (limited) write API, or write our own object layer. Spike in M0.
- **PDFlyer "Target Sheets" / Scan integration.** These depend on CCH Scan.
  Do we want a generic "barcode/separator-sheet → bookmark" feature instead?
- **Scripting language** for batch actions: Rhai (pure Rust, easy embed) vs
  Lua (familiar). Post-MVP, but affects the command architecture.

## Building

```sh
# Linux (Debian/Ubuntu) system deps for GPUI — once:
sudo apt install build-essential clang lld cmake pkg-config libfontconfig-dev \
  libwayland-dev libx11-xcb-dev libxkbcommon-x11-dev libvulkan1 libasound2-dev \
  libglib2.0-dev libssl-dev libzstd-dev xdg-desktop-portal xdg-desktop-portal-gtk


scripts/fetch-pdfium.sh          # prebuilt libpdfium → vendor/pdfium/<os>-<arch>/
scripts/fetch-ocr-data.sh        # Tesseract language data + PaddleOCR models → vendor/ocr/
scripts/build-plugins.sh         # the bundled plugins (plugins/) → target/plugins/
cargo run -p sheaf-app           # the GPUI window
```

**Plugins.** sheaf runs WebAssembly plugins (the `plugins` feature, on by
default; `--no-default-features` leaves out wasmtime too). The bundled
plugins build for `wasm32-wasip2`, which `rust-toolchain.toml` installs;
debug builds pick them up from `target/plugins`. Writing your own:
[`docs/plugins.md`](docs/plugins.md).

**OCR engines and build time.** Both engines are on by default. The first
build compiles Tesseract 5 and Leptonica from source (CMake and a C++17
compiler; a few minutes, cached in `~/.tesseract-rs`) and needs network
access once. `cargo build -p sheaf-app --no-default-features` skips the
engines for a quick build (the Recognize Text dialog then says so). The
PaddleOCR engine does not link ONNX Runtime: it loads Microsoft's generic
build from `vendor/ocr/lib` (or `ocr/lib` beside a packaged binary) the
first time it is used, so the app starts on any x86-64 CPU and the
`ort` crate's AVX2-only static binaries are never involved. CI runs the
binary under `qemu -cpu Nehalem` to keep it that way.

On Linux, file dialogs use the desktop portal when available and fall back to
Zenity if the portal fails. WSLg installations without a desktop portal need
`sudo apt install zenity` for Open, Insert PDF, Save As, and export dialogs.

On Windows, install Visual Studio 2022 (or the Build Tools) with the
"Desktop development with C++" workload and a Windows 10/11 SDK, plus rustup
with the default `x86_64-pc-windows-msvc` host. No other system deps. Run the
fetch script from Git Bash; it detects the host and installs `pdfium.dll`:

```sh
bash scripts/fetch-pdfium.sh     # → vendor/pdfium/windows-x86_64/pdfium.dll
bash scripts/fetch-ocr-data.sh   # → vendor/ocr/
cargo run -p sheaf-app
```

Visual Studio ships the CMake the Tesseract build needs; run the first
build from a Developer PowerShell (or with `VSINSTALLDIR` set) so it uses
the NMake generator.

The toolchain is pinned in `rust-toolchain.toml` (tracks Zed's requirement,
since GPUI is consumed as a git dependency at a pinned rev in `Cargo.toml`).

### Packaging

```sh
scripts/package.sh               # → dist/sheaf-<version>-<os>-<arch>.tar.gz (zip on Windows)
```

The bundle is the release binary next to the PDFium library, the OCR data
in `ocr/` (about 25 MB of language files and models, plus the ONNX Runtime
library in `ocr/lib`), the bundled plugins in `plugins/`, the licenses, and
(on Linux) a `.desktop` entry; unpack it anywhere and run `sheaf`. The
binary looks for `libpdfium` and `ocr/` beside itself first, so no install
step is needed. Pushing a `v*` tag runs `.github/workflows/release.yml`, which builds
the Linux and Windows bundles and attaches them to a GitHub release; `ci.yml`
runs fmt, clippy, tests and a build on every push.

Versions are alpha pre-releases (`0.1.0-alpha.N`) until the app is ready for
daily use. To release, bump `version` in the root `Cargo.toml` and push the
tag `v<version>`. The workflow refuses a tag that doesn't match `Cargo.toml`
and marks tags with a `-` suffix as pre-releases.

### M0 spikes

```sh
cargo run -p sheaf-doc --example roundtrip [some.pdf]        # lopdf load → resave → reload
cargo run -p sheaf-render --example render_page [some.pdf]   # PDFium → target/spike/page-*.png
cargo run -p sheaf-app                                        # GPUI window
```

`sheaf-render` finds libpdfium via `$PDFIUM_LIB_DIR`, then
`vendor/pdfium/<os>-<arch>/`, then the system library. `sheaf-ocr` finds its
data via `$SHEAF_OCR_DATA_DIR`, then `ocr/` beside the binary, then
`vendor/ocr/`; `cargo test -p sheaf-ocr --features tesseract,oar -- --ignored`
runs the engines on a PDFium-rendered page.

`sheaf --timing <file.pdf>` prints open / first-render timings to stderr. On
WSL, keep test PDFs on the Linux filesystem: `/mnt/c` goes through 9P, which
is 5–10× slower for this workload and will mislead you about trends.

> Licensing note: there are two PDFium wrappers on crates.io. `pdfium-render`
> (MIT/Apache) is the one we use. `pdfium` (newinnovations) is GPL-3 and must
> not be added.

### Optional: faster builds (sccache + mold)

The GPUI dependency tree is ~700 crates; a cold build is 10+ minutes on a
modest machine. Two tools cut iteration time substantially. Both are
configured in your **user-level** `~/.cargo/config.toml`, not in this repo, so
nothing here forces them on anyone.

**sccache** caches compiled dependency artifacts across `cargo clean`s,
toolchain bumps, and GPUI rev bumps.

```sh
cargo binstall sccache        # or: cargo install sccache
```

**mold** replaces the system linker. Link time dominates incremental
rebuilds of a GPUI binary, and mold is several times faster than `ld`/`lld`.

```sh
# Linux: grab the release binary (mold isn't on crates.io)
curl -fsSL https://github.com/rui314/mold/releases/download/v3.0.0/mold-3.0.0-x86_64-linux.tar.gz \
  | tar -xz --strip-components=2 -C ~/.local/bin mold-3.0.0-x86_64-linux/bin/mold
# or: sudo apt install mold
```

Then in `~/.cargo/config.toml`:

```toml
[build]
rustc-wrapper = "sccache"

[target.x86_64-unknown-linux-gnu]
linker = "clang"
# Use an absolute path if ~/.local/bin isn't on PATH.
rustflags = ["-C", "link-arg=-fuse-ld=mold"]
```

Notes:
- Changing `rustflags` or `rustc-wrapper` invalidates every artifact; expect
  one full rebuild after setup.
- sccache can't cache incremental compiles, so the `sheaf-*` workspace crates
  always compile locally. That's fine — incremental is faster for code you're
  editing. The win is the dependency tree.
- `sccache --show-stats` to confirm hits; `sccache --zero-stats` to reset.
- On Windows, mold doesn't apply; `lld-link` (bundled with the MSVC toolchain
  via `rust-lld`) is the equivalent lever. sccache works unchanged.

## Configuration

sheaf keeps its files in the platform config directory: `~/.config/sheaf`
on Linux, `%APPDATA%\sheaf` on Windows, `~/Library/Application Support/sheaf`
on macOS (`SHEAF_CONFIG_DIR` overrides).

- `settings.json` — written by the app: dock layout (which panels sit where,
  tab order, sizes, open state), window placement, and the OCR options under
  `"ocr"`: `engine` (`"tesseract"` or `"oar"`), `dpi` (150–600, default
  300), `skip_pages_with_text`, `tesseract.lang` (e.g. `"eng+deu"`, each
  needing a `.traineddata` file), `tesseract.tessdata_dir` and
  `oar.{det,rec,dict,textline_ori,doc_ori}` to point at your own data or
  models, `oar.model_set` (`"en"` or `"multi"`), and
  `oar.use_doc_orientation`. The Recognize Text dialog edits the common ones.
- `patterns.json` — the Patterns panel's presets, seeded from the built-ins on
  first run. Edit, delete, or add them from the panel (right-click a chip;
  "+ Save…" stores the current pattern) or by hand.
- `marks.json` — extra tickmark sets and stamps for the markup toolbar:
  `{ "tickmarks": [{ "name": "Audit", "marks": ["Σ", "◊"] }], "stamps": ["PBC"] }`.
- `plugins/` — installed plugins, one folder each (see
  [`docs/plugins.md`](docs/plugins.md)); `plugins.json` records which are
  turned off and what each may access; `plugin-data/<id>/` holds each
  plugin's storage and the `settings.json` you can give it. Manage them from
  Plugins → Manage Plugins….
- `keymap.json` — your key bindings, loaded after the defaults so they win.
  Zed's format: a list of `{ "context", "bindings" }` entries. Actions are
  named as the command palette (Ctrl+Shift+K) shows them; `null` unbinds.

```json
[
  { "context": "DocView", "bindings": { "ctrl-shift-k": "tools::ToolTick", "f9": null } },
  { "bindings": { "ctrl-shift-p": "workspace::CommandPalette" } }
]
```

Contexts: `DocView` (the page view), `Outline`, `Comments`, `Palette`,
`OcrDialog`, `PluginForm`, `PluginManager`, `TextInput`, `Workspace` (anywhere in the window), or none for global.
Plugin commands are `plugin::Run` with `{ "plugin": "<id>", "command": "<id>" }`.
Bad entries are reported on stderr at startup and skipped.

**Crash recovery.** A few seconds after each edit, sheaf writes the unsaved
changes as an incremental update to `<name>.pdf.sheaf-recovery` next to the
file (only the edits, so it's small and quick). The file itself is never
touched until you save. If sheaf didn't exit cleanly, the next open offers to
recover; saving, or choosing Don't Save, removes the sidecar.

## Name

A *sheaf* is a bundle of papers. It's also short, unclaimed on crates.io
and most package managers, and not an Adobe trademark.

## License

Dual-licensed under MIT or Apache-2.0, at your option. Third-party engines:
PDFium (BSD-3), hayro (Apache-2.0/MIT), Tesseract and its glyphless `pdf.ttf`
(Apache-2.0), Leptonica (BSD-2), ONNX Runtime (MIT), PaddleOCR models and
`oar-ocr` (Apache-2.0); see `packaging/licenses/`. MuPDF is deliberately not
used (AGPL).
