# Prior art: Rust and GPUI ecosystem

Our reviews of the crates sheaf could build on or borrow from. Collected
2026-10-13 from `docs/gpui-kit-eval.md`, `todos/eval-gpui-kit.md`,
`todos/table-extraction.md`, and an earlier gpui-pdf / pdfplumber review that
was only in a chat transcript until now.

## Summary

| Crate | What it is | Verdict |
|---|---|---|
| **gpui** (Zed, git) | The UI framework sheaf uses, pinned to Zed commit `93d7456` | Stay on it. Fork only when we need a fix, as a `sheaf/gpui` branch rebased each milestone, and send each fix upstream as a PR. |
| **gpui-pre** | Longbridge's weekly, unmodified crates.io snapshot of Zed's gpui | Not needed on its own. Matters only because gpui-kit and gpui-pdf require it. |
| **gpui-ce** | Community fork moving away from Zed (Parley text system, animation) | Watch it. Re-check around 2027-04. |
| **gpui-kit** (formerly gpui-component) | Longbridge's component library: 75+ components, AccessKit | **Don't adopt; borrow selectively.** Spike measured and documented in [../gpui-kit-eval.md](../gpui-kit-eval.md). |
| **gpui-pdf** | A small PDF *viewer* component on hayro, split out of the Zorite app | **Don't adopt; borrow** `forms.rs` (MIT) when forms work starts. |
| **pdfium-render** | PDFium bindings; sheaf's renderer today | Keep. Expose PDFium's progressive (pausable) render API. |
| **hayro** | Pure-Rust PDF renderer | Candidate second `Renderer` backend once it matures. |
| **lopdf** | Object-level PDF read/write; sheaf's edit layer | Keep. sheaf already writes its own incremental updates on top (`sheaf-doc/src/incremental.rs`). |
| **pdfplumber** (Rust port) | Python pdfplumber port; tables are close to the original, words aren't | Take only `pdfplumber-core`'s table finder, fed from PDFium. Plan in `todos/table-extraction.md`. |
| **mupdf-rs** | MuPDF bindings | **AGPL. Don't use.** Copy the display-list *idea* only. |

## GPUI distributions

### gpui and Zed

- All Zed crates sheaf depends on are **Apache-2.0**. Zed's GPL covers only the
  editor crates. Forking is legally fine; it means keeping the license and
  notices and not using the "Zed" name.
- The last crates.io release of `gpui` is 0.2.2 (2025-10-22). It predates Zed's
  split into per-platform crates, and `gpui_platform`, `gpui_linux` and the other
  platform crates were never published. Zed declined to publish release notes
  (zed#42771).
- Zed is still developing gpui. In the six months before 2026-10:
  - about 280 commits to core gpui, down about 30% year over year;
  - backend commits roughly tripled: about 72 Linux, 83 macOS, 59 Windows and 41
    GPU renderer.
- That work follows Zed's needs. 118 gpui PRs from outside contributors are
  still open, and features Zed doesn't need get closed as "not planned". Zed
  said in late 2025 that it was putting "major brakes" on gpui work for 2026.
- **Decision:** no public sheaf fork of gpui. Stay on the pinned Zed commit.
  When we first need a fix inside gpui, carry it on a `sheaf/gpui` branch of a
  `zed-industries/zed` fork, rebase on purpose (e.g. once per milestone), and
  submit each fix to Zed.

### gpui-pre

- Longbridge (huacnlee) publishes an **unmodified** snapshot of Zed main to
  crates.io about weekly, renamed `gpui-pre-*` (19 crates, one-to-one with
  Zed's). About 137k downloads in its first month.
- It can't carry patches. Its 0.3.9 release is exactly our pinned commit.
- Switching to it is free at build time (measured in the gpui-kit spike: clean
  release build 210 s → 211 s, binary unchanged, startup unchanged).
- The cost is the lock-in. Every crate built on gpui-pre (gpui-kit, gpui-pdf)
  needs your app on **the same** gpui-pre version, so their release schedule
  becomes yours. Carrying our own gpui patch would mean forking all 19
  `gpui-pre-*` crates at that version and `[patch.crates-io]`-ing them, and
  redoing it on every upgrade.

### gpui-ce

- The community fork, with about five core maintainers.
- It is **deliberately diverging** from Zed: it replaced the text system with
  Parley and adds animation features. It syncs with Zed only occasionally, in
  large merges.
- Governance and licensing are informal; its website wrongly says
  "MIT OR Apache".
- Sources: <https://github.com/gpui-ce/gpui-ce>,
  <https://github.com/gpui-ce/gpui-ce/discussions/318>.
- **Verdict:** worth watching, not adopting. Re-check about six months after
  2026-10-12, alongside gpui-kit.

### gpui-kit

Full results: [../gpui-kit-eval.md](../gpui-kit-eval.md) (spike branch
`spike/gpui-kit`, never merged).

- Longbridge's component library (`longbridge/gpui-kit`, formerly
  `gpui-component`). 17k stars, Apache-2.0 code, CC BY 4.0 docs. Longbridge
  ships it in their desktop app. 75+ components, AccessKit, headless UI tests.
  Weekly 0.x releases with breaking changes.
- Pins an **exact** gpui-pre (`=0.3.9` in kit 0.7.3).
- Measured cost (baseline → six spike steps): clean release build +45%, peak
  build RSS +0.8 GB, incremental release build +27%, binary +24% (+8.9 MB), +66
  crates in the Linux graph (lsp-types, html5ever, ropey, a reqwest fork… none
  used by sheaf). Startup unchanged.
- Measured gain: about 1,420 fewer lines in sheaf-app, mostly the text input
  and text area, which also gain undo/redo, word movement, a context menu and
  IME work.
- 15 behavior gaps needed sheaf glue, mostly focus and key dispatch (Enter and
  Escape propagate, radios aren't arrow-navigable, palette dispatch from the
  wrong focus). The tree has **no drag-and-drop**, which blocked the outline
  panel.
- **Verdict:** don't depend on it. Port its `undo_manager.rs` and `movement.rs`
  ideas into our text widgets (Apache-2.0, with attribution), and keep the
  headless `#[gpui::test]` approach for our own widgets. Re-check if kit adds
  finer feature gates and drops the exact gpui-pre pin.

## gpui-pdf

Reviewed at 0.6.1 (2026-09-22) from the crates.io source.

- **What:** MIT-licensed, about 4,600 lines in four files (`lib.rs` 2,887,
  `forms.rs` 810, `text.rs` 574, `outline.rs` 318). Split out of **Zorite**
  (`packetThrower/zorite`, an app that is itself GPL-3.0-or-later). Two
  releases, 47 downloads total at review time. Its README: "Early, but solid
  for scroll-to-read viewing."
- **Two ways to use it:** a finished `PdfView` entity, or the low-level
  functions `parse`, `page_dims`, `render_page` and `keep_window` (which pages
  are near the viewport).
- **Rendering:** pure Rust via **hayro 0.7**, no native libraries, no system
  fonts. **Whole pages, never tiles.** Every page gets a correctly sized slot up
  front, only pages near the viewport are rasterized, and pages that leave are
  freed from CPU *and* the GPU atlas. Off-thread load, parse and render. DPI ×
  zoom × a host-supplied quality multiplier.
- **Zoom:** the old bitmap stays on screen, stretched, until the sharp one is
  ready, so the page never blanks.
- **Passwords:** a "locked" state plus an event; the host draws its own
  password prompt and calls `unlock`. RC4, AES-128 and AES-256 via hayro.
- **Outline and links:** read-only TOC panel; internal and external links.
- **`markup` feature:** a text layer extracted by a custom hayro `Device`
  (`text.rs`); quote-anchored highlights the *host* stores and the viewer
  locates and draws; drag-to-create with a color picker.
- **`search` feature:** a browser-style find bar.
- **`forms` feature (lopdf):** fixes display of filled AcroForms
  (checkbox/radio appearances resolved through `/AS`, and valued text fields
  with no appearance, the `NeedAppearances` case, get a synthesized one).
  Filling: a click emits `FieldClicked` with bounds, the host seats its own
  input, `set_form_value` writes the value and regenerates the appearance, and
  `replace_bytes` swaps the document without blanking.
- **gpui version:** depends on `gpui-pre` 0.3; your app must resolve to the
  same version.

### Compared with sheaf (2026-10)

| Area | gpui-pdf | sheaf |
|---|---|---|
| Scope | Viewer component | Editor aiming at Acrobat Pro + PDFlyer parity |
| Renderer | hayro only | PDFium behind the `Renderer` trait; hayro planned |
| Render strategy | Whole pages near the viewport | Tiles, LRU cache by (page, zoom, tile), low-res preview first |
| Thumbnails, multiple documents | No | Yes |
| Outline | Read-only | Full bookmark editing, move pages with bookmarks |
| Page organization | None | Reorder, rotate, delete, extract, insert with bookmark merging |
| Annotations | Host-stored highlights, nothing written to the PDF | Real PDF annotations incl. tickmarks, connectors, calc tape, comments panel, flatten, clean copy |
| Search | Plain find bar | Progressive find plus regex Patterns |
| OCR | None | Tesseract and PaddleOCR, text layer written on save |
| Saving | lopdf full rewrite (forms only) | Incremental save, undo/redo, crash recovery |
| **Forms** | **Display and filling work** | Not started |
| Passwords | Locked state + host prompt | Prompt on open; saves come out unencrypted |
| Native dependencies | None | libpdfium, Tesseract, ONNX Runtime |

### What to take

1. **Don't adopt.** It covers roughly sheaf's M1 viewer with less in each area,
   it replaces the tile pipeline sheaf is betting on for speed, and it forces
   the gpui-pre switch we already turned down.
2. **Borrow `forms.rs`** (MIT) when forms work starts. It solves
   checkbox/radio appearance resolution and no-appearance text fields on
   lopdf, the same object layer sheaf uses, and is a reference for
   regenerating a field appearance after filling.
3. **`text.rs`** shows text extraction can work on hayro without PDFium. Useful
   evidence for a hayro `Renderer` backend.
4. **Host-owned inputs** (password prompt, form field editor) fit sheaf's
   declarative plugin UI.
5. It says nothing about hayro on 1,000-page scanned binders. No benchmarks.

Sources: [docs.rs/gpui-pdf](https://docs.rs/gpui-pdf),
[crates.io/crates/gpui-pdf](https://crates.io/crates/gpui-pdf),
[github.com/packetThrower/zorite](https://github.com/packetThrower/zorite).

## PDF crates

| Crate | License | Does well | Use for | Weaknesses |
|---|---|---|---|---|
| **pdfium-render** ([docs](https://docs.rs/crate/pdfium-render/latest)) | MIT OR Apache-2.0 (PDFium is BSD-3). Not to be confused with the GPL-3 `pdfium` crate. | Wraps almost all of PDFium: render pages or page regions, text and image extraction, forms, annotations, page objects, save. | sheaf's renderer. Check whether PDFium's progressive `FPDF_RenderPageBitmap_Start` / `_Continue` / `NeedToPauseNow` API is exposed, and add raw FFI if not. | **PDFium isn't thread-safe.** The `thread_safe` feature is a global mutex with no speed benefit; real parallelism means separate processes (as Chrome does). sheaf runs one dedicated render thread (`sheaf-render/src/lib.rs`). |
| **hayro** ([GitHub](https://github.com/LaurenzV/hayro)) | MIT OR Apache-2.0 | Pure, `#![forbid(unsafe)]` Rust interpreter and renderer, split into syntax/interpret/svg crates plus JPX, JBIG2 and CCITT decoders. Passes 1,400+ PDFs from the pdf.js and PDFBox regression suites. | A second backend that could rasterize tiles in parallel in one process. A fuzz-safe parser for thumbnails or untrusted files. A correctness cross-check against PDFium. | "Experimental, WIP." No knockout groups, no non-embedded CID fonts, encryption gaps. The author says performance isn't a focus yet. Rendering only. |
| **lopdf** ([crates.io](https://crates.io/crates/lopdf)) | MIT | Object-level read/write: merging, splitting, object surgery. | sheaf's structural edit layer (page tree, outline, annotations). | Loads eagerly (unverified). No rendering. |
| **mupdf-rs** (`mupdf`) | **AGPL-3.0** (commercial license from Artifex) | Fast; MuPDF's display lists are interpreted once and replayed at any transform or clip from any thread. | Nothing directly. The display-list pattern is worth copying. | AGPL rules it out for sheaf. |
| **pdf-rs** (`pdf`, [GitHub](https://github.com/pdf-rs/pdf)) | MIT | Typed, lazy-ish object reader. Sibling `pdf_render` tried GPU rasterization via Pathfinder. | Ideas for typed object access; prior art for GPU PDF raster. | Writing is experimental; renderer immature (unverified). |
| **krilla** ([GitHub](https://github.com/LaurenzV/krilla)) | MIT OR Apache-2.0 | High-level PDF *writing* on `pdf-writer`: paths, gradients, glyphs, images, font subsetting, tagging, PDF/A and PDF/UA. | Generating new content: stamps, flattened annotations, pages from images, export. | Writes new files; doesn't edit existing ones. |
| **typst-pdf** | Apache-2.0 (unverified) | Typst 0.14 (Oct 2025) rebuilt PDF export on krilla, with an accessibility and tagging focus. | Evidence krilla holds up in production. | Export only. |

## Text and table extraction (pdfplumber)

- Python **pdfplumber** sits on **pdfminer.six** (character extraction, font and
  encoding handling) and uses pypdfium2 only to draw debug images. Its value
  is the layer on top: every glyph, line and rectangle as a plain object;
  tunable word and line grouping (`x_tolerance`, `y_tolerance`,
  `layout=True`); spatial filters (`crop`, `within_bbox`, `dedupe_chars`);
  table detection from ruling lines or text alignment (the Nurminen/Tabula
  approach); and a visual debugger. It's slow: pure Python, often 100 ms to
  over a second per page.
- **For getting characters out, PDFium is as good as pdfminer.six** and usually
  better on damaged files, and one to two orders of magnitude faster. sheaf
  already reads PDFium characters with `loose_bounds` and builds words in
  `sheaf-doc/src/text_layer.rs`.
- **What Rust lacks is the layer on top.** Candidates read: the Rust
  `pdfplumber` port (developer0hye, Apache-2.0, alpha; its own scorecard puts
  tables at 271/321 exact against Python pdfplumber 0.11.10 but words at
  4/288), `pdfsink-rs` (claims about 250× faster than pdfplumber on tables, the
  author's own benchmark), and `pdfni`.
- **Plan:** take only `pdfplumber-core`'s `TableFinder`, feed it PDFium
  characters and edges built from PDFium path objects, and keep PDFium as the
  only parser. Details in `todos/table-extraction.md`. Also worth building:
  configurable word grouping, and a debug overlay of character and word boxes
  (cheap, since sheaf already draws over pages).
- Other engines for text: lopdf / pdf-extract are weak on unusual encodings and
  CID fonts; MuPDF's "stext" is excellent but AGPL.

Sources: [pdfplumber (Rust) on docs.rs](https://docs.rs/pdfplumber),
[pdfsink-rs](https://docs.rs/crate/pdfsink-rs/0.2.8),
[pdfni](https://docs.rs/crate/pdfni/0.2.0),
[pdfminer.six 20251229](https://newreleases.io/project/github/pdfminer/pdfminer.six/release/20251229).
