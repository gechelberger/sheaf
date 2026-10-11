# sheaf

A fast, native PDF editor. Rust + GPUI.

Sheaf aims for feature parity with Adobe Acrobat Pro's editing tools, plus the
workpaper tooling from CCH ProSystem fx **PDFlyer** (tickmarks, connectors,
calculator tapes, bookmark-driven page management) — without the bloat.

> **Status:** pre-alpha. Nothing works yet. This README is the design doc.

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
| Text / OCR | PDFium text extraction; OCR via `ocrs` or Tesseract behind a feature flag | OCR is post-MVP. |
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
| Open / render PDF (incl. encrypted, forms, large scans) | 🎯 |
| Continuous / single / facing page modes | 🎯 |
| Zoom (fit page / fit width / custom), rotate view | 🎯 |
| Thumbnails sidebar (lazy, virtualized) | 🎯 |
| Bookmarks (outline) sidebar | 🎯 |
| Text search (find in document) | 🎯 |
| Tabs / multiple documents | 🎯 |
| Split view (same doc, two positions) | 📋 |
| Attachments panel | 📋 |
| Layers (OCG) panel | 📋 |

### Page organization
| Feature | Status |
|---|---|
| Reorder pages (drag in thumbnails) | 🎯 |
| Rotate **individual** pages (PDFlyer-style, not whole doc) | 🎯 |
| Delete / extract / duplicate pages | 🎯 |
| Insert PDF at position (PDFlyer "Insert PDF") | 🎯 |
| Merge bookmarks on insert (parents merged, children appended) | 🎯 |
| Split document (by page range, by bookmark, by size) | 📋 |
| Insert blank page / from image / from scanner | 📋 |
| Crop pages | 📋 |
| Page labels (roman numerals, prefixes) | 📋 |
| Headers / footers / Bates numbering | 📋 |
| Watermarks / backgrounds | 📋 |

### Bookmarks (PDFlyer-heavy)
| Feature | Status |
|---|---|
| Create / rename / delete / nest bookmarks | 🎯 |
| Move bookmark **with its pages** | 🎯 |
| Colorize bookmarks by level / status | 🎯 |
| Bookmark from page range | 📋 |
| Target sheets / auto-bookmark from scan separators | ❓ |

### Markup & annotation
| Feature | Status |
|---|---|
| Sticky notes, highlight, underline, strikethrough | 🎯 |
| Free text, shapes (rect, ellipse, line, arrow, polygon) | 🎯 |
| Ink / freehand | 📋 |
| Stamps (built-in + custom) | 🎯 |
| **Tickmarks** (PDFlyer): quick-stamp library, keyboard-driven | 🎯 |
| **Connectors** (PDFlyer): linked markers that jump between pages | 🎯 |
| **Calculator tape** (PDFlyer): 10-key calc, tape placed as annotation | 🎯 |
| Comments list / review panel, status, replies | 📋 |
| Flatten annotations | 📋 |
| Export Clean Copy (strip annots, notes, tapes, stamps, attachments) | 🎯 |
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
| OCR (searchable text layer) | 📋 |
| Export text | 📋 |
| Print | 📋 |
| PDF/A conversion | ❓ |
| Office ↔ PDF | ❌ not planned (use LibreOffice) |

### Security
| Feature | Status |
|---|---|
| Open password-protected PDFs | 🎯 |
| Set / remove passwords, permissions | 📋 |
| Digital signatures (sign / verify) | 📋 |
| Certificate encryption | ❓ |

### Other Acrobat Pro features
| Feature | Status |
|---|---|
| Compare documents | 📋 |
| Optimize / reduce file size | 📋 |
| Preflight / accessibility checker | ❓ |
| Batch actions / scripting | 📋 (Lua or Rhai, not JS) |
| Portfolios | ❌ not planned |

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

**v0.2 is OCR** (searchable text layer on scanned pages, batch over a binder).
Candidate engines: `ocrs` (pure Rust, ONNX models) vs Tesseract bindings;
decide on accuracy for scanned tax forms, not on purity.

### Milestones

| # | Milestone | Proves |
|---|---|---|
| M0 | **Spikes**: GPUI window on Windows and Linux (both tier 1); pdfium-render renders a page; lopdf round-trips a PDF. *Linux: all three pass (2026-10-11). Windows: pending.* | The stack is viable |
| M1 | **Viewer**: tiled async rendering, continuous scroll, zoom, thumbnails, outline, tabs, search | The performance thesis |
| M2 | **Organizer**: page reorder/rotate/delete/insert, bookmark tree editing, incremental save, undo | The document model |
| M3 | **Markup**: standard annotations, stamps, tickmarks, connectors, calc tape, clean copy export | The PDFlyer workflow |
| M4 | **Polish → v0.1**: keybindings, command palette, settings, crash-safe autosave, packaging | Daily-driver readiness |

## Open questions

- **GPUI risk.** Pre-1.0, API churn, and we'd be an early non-Zed user on Linux.
  The Linux spike passed first try (wgpu backend, Wayland via WSLg on
  llvmpipe). Remaining risk is Windows and API churn across rev bumps; fallback
  is still `iced` or `egui` if that bites. Decide after the Windows spike.
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
  libglib2.0-dev libssl-dev libzstd-dev

scripts/fetch-pdfium.sh          # prebuilt libpdfium → vendor/pdfium/<os>-<arch>/
cargo run -p sheaf-app           # the GPUI window
```

The toolchain is pinned in `rust-toolchain.toml` (tracks Zed's requirement,
since GPUI is consumed as a git dependency at a pinned rev in `Cargo.toml`).

### M0 spikes

```sh
cargo run -p sheaf-doc --example roundtrip [some.pdf]        # lopdf load → resave → reload
cargo run -p sheaf-render --example render_page [some.pdf]   # PDFium → target/spike/page-*.png
cargo run -p sheaf-app                                        # GPUI window
```

`sheaf-render` finds libpdfium via `$PDFIUM_LIB_DIR`, then
`vendor/pdfium/<os>-<arch>/`, then the system library.

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

## Name

A *sheaf* is a bundle of papers. It's also short, unclaimed on crates.io
and most package managers, and not an Adobe trademark.

## License

Dual-licensed under MIT or Apache-2.0, at your option. Third-party engines:
PDFium (BSD-3), hayro (Apache-2.0/MIT). MuPDF is deliberately not used (AGPL).
