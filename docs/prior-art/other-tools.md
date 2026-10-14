# Prior art: performance-focused and power-user PDF tools

Research for **sheaf**, a native Rust + GPUI PDF editor built on PDFium with tiled rendering. It aims to match Acrobat Pro's features while running much faster. Researched 2026-10-13. Claims marked "(unverified)" come from memory or from secondary sources I could not confirm.

Rust crates (pdfium-render, hayro, lopdf, mupdf-rs, pdf-rs, krilla, typst-pdf) are in [rust-ecosystem.md](rust-ecosystem.md#pdf-crates).

---

## 1. Desktop editors and viewers

### PDF-XChange Editor (Tracker Software / PDF-XChange Co.)

| Aspect | Notes |
|---|---|
| License | Proprietary freemium with a one-time purchase ([AlternativeTo](https://alternativeto.net/software/pdf-xchange-editor/about)). The free tier includes OCR, and paid-only features add a watermark when used unlicensed ([Software Advice](https://www.softwareadvice.com.au/compare/449925/476965/Acrobat-Reader-DC/vs/PDF-XChange)). Windows only. |
| Speed reputation | Users call it "lightweight, fast", say it "loads incredibly fast", and note it works offline ([vendor reviews page](https://www.pdf-xchange.com/product-reviews/pdf-xchange-editor), [AlternativeTo](https://alternativeto.net/software/pdf-xchange-editor/about)). It is the usual "Acrobat Pro features without the bloat" pick. |
| Shortcuts | **Every command can be rebound.** The path is Customize Toolbars, then the Commands tab, then pick an action, then Properties, then the Keys box. The dialog shows when a key is already taken ("Shortcut currently used by") ([V11 manual](https://help.pdf-xchange.com/pdfxe11/keyboard-shortcuts.html), [KB 340](https://www.pdf-xchange.com/knowledgebase/340)). The defaults have collisions: Ctrl+B is both Bold and Bookmarks. Holding Space switches to the hand tool temporarily. |
| Measurement | Distance, perimeter and area tools on the Comment tab. Calibrate/Set Scale (Ctrl+Shift+C) defines named scales, measurements export to CSV, and rulers, grid, guides and snapping are available (Ctrl+R, Ctrl+', Ctrl+;, Ctrl+Shift+;) ([measuring tools V7](https://help.pdf-xchange.com/pdfxp7/measuring-tools_ed.html), [V11](https://help.pdf-xchange.com/pdfxp11/the-measuring-tools.html)). |
| Stamps | A Stamps Palette holds collections of stamps, including user-made ones built from images or pages ([V5 manual TOC](https://downloads.pdf-xchange.com/PDF-Editor5_manual.pdf)). How it works in detail is (unverified). |
| Other | Document compare, spell check, a comments list ([review](https://thebusinessdive.com/pdf-xchange-review)). It also has "Exclusive Mode" (Ctrl+G, Ctrl+E), which keeps a tool selected until you release it. |
| **Steal** | (1) A keymap where every command is rebindable and the binding UI warns about conflicts. (2) Measurement as a full annotation type: calibrated scales, CSV export, snapping. (3) Hold a key for a temporary tool (Space for pan), and a sticky-tool mode. |
| Weaknesses | Users find the UI dense and complex, report unfinished features, and find the pricing tiers confusing ([WPS review](https://www.wps.com/blog/pdf-xchange-editor-review-and-best-alternative-post/), [AlternativeTo](https://alternativeto.net/software/pdf-xchange-editor/about)). Windows only. |

### SumatraPDF

| Aspect | Notes |
|---|---|
| License | GPLv3 (unverified, widely reported). Windows only. |
| Why it's fast | It renders with **MuPDF**, which it switched to in v0.4 "for more speed" after using Xpdf and then Poppler. It ships as a single portable executable with no dependencies (about 6 MB, compared with Adobe Reader XI's 320 MB in 2017), and its feature set is deliberately small ([Wikipedia](https://en.wikipedia.org/wiki/Sumatra_PDF)). MuPDF's **display lists** let it interpret a page once and replay it many times at different transforms and clips, from other threads, without touching the file ([MuPDF Explored](https://ghostscript.com/~robin/mupdf_explored/mupdf_exploredse11.html), [MuPDF multi-threaded cookbook](https://mupdf.readthedocs.io/en/1.27.1/cookbook/c/multi-threaded.html)). |
| Command palette | **Ctrl+K**. Typing filters commands, arrow keys move, Enter runs. Prefixes change mode: `>` for commands, `#` for file history, `@` for tabs, `:` for everything at once ([docs](https://www.sumatrapdfreader.org/docs/Command-Palette)). |
| Keyboard model | Every action is a named command ID such as `CmdClose` or `CmdOpenFile` ([Commands](https://www.sumatrapdfreader.org/docs/Commands)). Shortcuts map keys to command IDs in a text settings file, which you open by typing "adv" in the palette. Changes **apply on save, with no restart** ([Customize shortcuts](https://www.sumatrapdfreader.org/docs/Customize-keyboard-shortcuts)). Vim-ish j/k navigation (unverified for the current version). |
| **Steal** | (1) A single command registry used by menus, the palette, keybindings and the config file. (2) A palette with prefix modes, so recent files and windows are one keystroke away. (3) Settings as hot-reloaded text. (4) An immutable per-page display list, interpreted once and rasterized per tile or zoom. |
| Weaknesses | Annotation is minimal and form filling is weak ([Wikipedia](https://en.wikipedia.org/wiki/Sumatra_PDF)). Not an editor. Windows only. |

### Okular (KDE)

| Aspect | Notes |
|---|---|
| License | GPL-2.0-or-later (unverified). Poppler backend for PDF, with other backends for PS, TIFF and others ([KDE UserBase](https://userbase.kde.org/Okular)). |
| Annotations | Text markup (highlight, underline) and graphic annotations: pop-up note, inline note, freehand, line, polygon, stamp, ellipse, typewriter ([docs](https://docs.kde.org/stable/en/okular/okular/annotations.html)). **F6** opens the annotation toolbar, and **1 to 9 pick a tool**. **Double-clicking a tool locks it on** for repeated use. |
| Storage | Annotations are stored per user by default, outside the file. Saving them into the PDF requires Save As (Poppler 0.20+) ([UserBase](https://userbase.kde.org/Okular)). |
| **Steal** | Number keys pick tools in an annotation toolbar. Double-click to make a tool sticky. A "Reviews" sidebar listing annotations. |
| Weaknesses | Annotations not saved to the PDF by default confuses users. One user reports no keyboard-driven text selection and no way to highlight already-selected text ([KDE Discuss](https://discuss.kde.org/t/highlighting-in-okular/47040)). |

### zathura

| Aspect | Notes |
|---|---|
| License | zlib (unverified). Plugin backends: `zathura-pdf-poppler` or `zathura-pdf-mupdf`, where the MuPDF one also handles EPUB and XPS ([ArchWiki](https://wiki.archlinux.org/title/Zathura)). |
| Keyboard model | Vi-style by default, with optional Emacs-style keys. A `:` command line. Fully configurable through `zathurarc`. Ctrl+R recolors pages for dark mode. SyncTeX forward and inverse search ([pwmt.org](https://www.pwmt.org/projects/zathura/), [ArchWiki](https://wiki.archlinux.org/title/Zathura)). Link "follow" hints (`f`) and marks (unverified). |
| Annotations | **Not supported.** A long-standing feature request mentions a branch that was never released ([issue #7](https://git.pwmt.org/pwmt/zathura/issues/7)). |
| **Steal** | (1) An optional modal/vim keymap profile (hjkl, gg/G, `/` search, `:` commands, marks). (2) Recolor/invert as a render-time option, not a post-process. (3) SyncTeX hooks for LaTeX users. (4) A minimal "no chrome" mode. |
| Weaknesses | Viewer only, with no annotations or forms. Rendering quality and features depend on the backend. |

### macOS Preview

| Aspect | Notes |
|---|---|
| License | Proprietary, bundled with macOS, built on PDFKit. |
| Page management UX | The **thumbnail sidebar is the page-ops surface**. Drag thumbnails to reorder. Drag thumbnails *between two open documents' sidebars* to move or copy pages. Cmd-click selects non-adjacent pages. Drag a PDF from Finder onto the sidebar to insert all its pages at the drop point ([Apple support](https://support.apple.com/en-au/guide/preview/prvw43696)). Delete or rotate the selected thumbnails with the Delete key and Cmd+R/Cmd+L (unverified for current macOS). |
| Save model | Autosave and versions. Apple warns that changes save automatically, so use File > Duplicate to keep the original ([Apple](https://support.apple.com/en-au/guide/preview/prvw43696)). |
| **Steal** | Thumbnails as direct-manipulation objects: multi-select, drag-reorder, cross-window drag, drop a file to insert, keyboard rotate and delete. Sheaf runs one document per window, so **drag between windows** matters a lot. |
| Weaknesses | Autosave surprises users. PDFKit has caused regressions such as lost annotations and form bugs across macOS releases (unverified, widely reported). Weak at large files and professional features. |

---

## 2. Engines and web viewers

### Xodo / Apryse WebViewer (formerly PDFTron)

| Aspect | Notes |
|---|---|
| License | Proprietary/commercial. Xodo is Apryse's consumer app. |
| Architecture | The C++ SDK is compiled to **WebAssembly** with WASM threads, so parsing and rendering run on the client ([Apryse docs](https://docs.apryse.com/documentation/web/guides/overview/), [blog](https://apryse.com/blog/how-to-build-a-java-pdf-viewer)). An optional WebViewer Server **serves images first, then hands off to client-side rendering** ([same](https://docs.apryse.com/documentation/web/guides/overview/)). The DocumentViewer handles "rendering, caching, high level zoom rendering" ([viewer guide](https://docs.apryse.com/web/guides/viewer)). `loadCanvas` accepts `renderRect`, which renders a sub-rectangle of a page, i.e. a tile ([lower-level APIs](https://docs.apryse.com/web/guides/lower-level-document-apis)). Whether it internally runs a low-res base layer plus high-res tiles is (unverified). |
| **Steal** | Render a server or cached raster first, then switch to a live render. For sheaf: show a **persisted thumbnail or last-session tile cache immediately on open**, then replace it with live tiles. Keep a `render_rect` API at the engine boundary. |
| Weaknesses | Heavy SDK. Users report slow first render on single pages in v10.3 ([forum](https://community.apryse.com/t/slowness-issue-with-web-viewer/11295)). Expensive licensing. |

**Related open design:** EmbedPDF (PDFium-WASM) documents the tiling pattern explicitly. A **base layer at fixed low scale** gives instant feedback, and **visible-only high-res tiles** are drawn over it. Defaults are a 768 px tile, a small overlap to hide seams, and "extra rings" of prefetch that trade memory for smooth scrolling ([EmbedPDF tiling plugin](https://www.embedpdf.com/docs/react/headless/plugins/plugin-tiling)).

### PDF.js (Mozilla)

| Aspect | Notes |
|---|---|
| License | Apache-2.0. |
| Pipeline | `core/` parses inside a **Web Worker** and sends **operator lists** to the main thread. `display/` replays them onto a `<canvas>` ([summary](https://tessl.io/registry/tessl/npm-pdfjs-dist), secondary). The **text layer** is a transparent overlay of absolutely positioned spans for selection, find and copy ([pdfjs.express](https://pdfjs.express/blog/how-pdf-js-works)). `getTextContent` comes from the same operator stream ([dev.to](https://dev.to/bonzai2carn/the-three-tier-pdf-extraction-model-demystifying-pdfjs-52bc)). |
| Tiling | There was historically one canvas per page, and above `maxCanvasPixels` it **falls back to CSS-scaling** (blurry). In 2025 [PR #19128](https://github.com/mozilla/pdf.js/pull/19128) added `PDFPageDetailView`. It renders a high-res patch about **2x the visible area in each direction**, re-centres it as you scroll, and draws it over the low-res full-page canvas. That is "low-res page + hi-res detail", not a tile grid. |
| Progressive loading | It uses **HTTP Range requests** to fetch visible pages first, if the server supports them. `disableAutoFetch` stops background prefetch ([FAQ](https://github.com/mozilla/pdf.js/wiki/Frequently-Asked-Questions), [Nutrient](https://www.nutrient.io/blog/pdfjs-advanced-loading-streaming-workers/)). The FAQ recommends that viewers "create and render only visible pages", since a letter page at 96 DPI is about 3.4 MB of canvas. |
| Shortcuts | n/j/Space for the next page, p/k/Shift+Space for the previous, Ctrl+F and Ctrl+G to find, Ctrl+Alt+P for presentation, h for hand, s for select, r to rotate ([FAQ](https://github.com/mozilla/pdf.js/wiki/Frequently-Asked-Questions)). |
| **Steal** | (1) Parsing and interpretation off the UI thread, with an immutable "op list" handed to rasterizers. (2) Keep the detail-view idea even with tiles: always have a low-res whole page so zoom and scroll never show blank. (3) A text layer as a separate, lazily built data structure. |
| Weaknesses | JS and canvas speed on huge or complex pages. CSS-zoom blur before the detail view. Text-layer spans can be coarse, for example one span for a whole equation ([dev.to](https://dev.to/bonzai2carn/the-three-tier-pdf-extraction-model-demystifying-pdfjs-52bc)). |

### Chrome's PDF viewer (PDFium)

| Aspect | Notes |
|---|---|
| License | PDFium is BSD-3-Clause, Google's open-sourcing of Foxit code ([Computerworld](https://www.computerworld.com/article/1393378/google-open-sources-pdf-software-library.html)). The viewer is in Chromium `pdf/`. |
| Rendering | `PaintManager` does **progressive rendering of dirty rects**. One rect can take several engine calls; finished rects go to `ready` and unfinished ones to `pending`. Higher-priority rects such as scrollbars flush immediately ([paint_manager.h](https://chromium.googlesource.com/chromium/src/+/75.0.3770.100/pdf/paint_manager.h)). The engine uses PDFium's **progressive API**: `FPDF_RenderPageBitmap_Start`, then `FPDF_RenderPage_Continue` with an `IFSDK_PAUSE::NeedToPauseNow` callback, then `FPDF_RenderPage_Close`. Newer headers add `FPDF_RenderPageBitmapWithColorScheme_Start` ([fpdf_progressive.h](https://pdfium.googlesource.com/pdfium/+/refs/heads/main/public/fpdf_progressive.h)). The units are dirty rects, not a fixed tile grid. |
| Recent | "PDF Searchify" does local OCR so scanned PDFs become selectable (Chrome ~131) ([Windows Report](https://windowsreport.com/chromes-new-feature-makes-scanned-pdfs-searchable-and-editable-never-type-again/)). Ink and text annotations are (unverified). |
| **Steal** | **Use PDFium's pause callback.** A tile render can yield on a deadline or on a cancel flag when the tile scrolls off-screen, so a slow page never blocks newer, higher-priority tiles. Flush priority regions first. |
| Weaknesses | Editing is minimal. PDFium is single-threaded per process (below). |

---

## 3. Performance techniques across viewers

| Technique | Who does it | Source | Sheaf takeaway |
|---|---|---|---|
| **Low-res first, then hi-res** | PDF.js keeps the full-page canvas and overlays a hi-res detail view. EmbedPDF draws a base layer at scale 1.0 with tiles on top. Apryse shows server images, then client render. | [pdf.js #19128](https://github.com/mozilla/pdf.js/pull/19128), [EmbedPDF](https://www.embedpdf.com/docs/react/headless/plugins/plugin-tiling), [Apryse](https://docs.apryse.com/documentation/web/guides/overview/) | Never show blank: show a thumbnail-scale page texture, scaled, under the tiles. During zoom gestures, scale existing textures and re-tile on settle. |
| **Tiling / partial render** | EmbedPDF uses 768 px tiles with overlap and prefetch rings. Apryse has `renderRect`. pdfium-render does portion rendering. Chrome uses dirty rects. | [EmbedPDF](https://www.embedpdf.com/docs/react/headless/plugins/plugin-tiling), [Apryse](https://docs.apryse.com/web/guides/lower-level-document-apis), [paint_manager.h](https://chromium.googlesource.com/chromium/src/+/75.0.3770.100/pdf/paint_manager.h) | Fixed tile size snapped to zoom levels, 1-2 px overlap, and a configurable prefetch ring sized against memory. |
| **Progressive / interruptible render** | PDFium has `_Start/_Continue` and `NeedToPauseNow`. Chrome's PaintManager tracks pending and ready rects. | [fpdf_progressive.h](https://pdfium.googlesource.com/pdfium/+/refs/heads/main/public/fpdf_progressive.h) | Cancel or pause stale tiles. Use a priority queue: visible first, then the prefetch ring, then thumbnails. |
| **Display-list caching** | MuPDF (SumatraPDF) interprets once and replays at any transform or clip, across threads. | [MuPDF Explored](https://ghostscript.com/~robin/mupdf_explored/mupdf_exploredse11.html), [cookbook](https://mupdf.readthedocs.io/en/1.27.1/cookbook/c/multi-threaded.html) | PDFium lacks an equivalent, but cache loaded `FPDF_PAGE` handles (with an LRU), and don't reload a page per tile. |
| **Thumbnail caching** | Preview and Sumatra keep a thumbnail sidebar. Sumatra caches thumbnails for recent files on its start page (unverified). | ([Apple](https://support.apple.com/en-au/guide/preview/prvw43696)) | Keep a disk cache keyed by (file hash or mtime, page, size). It doubles as the instant "low-res first" layer on reopen. |
| **Linearization / Fast Web View** | Page-1 objects come first, with hint tables of per-page byte offsets. It needs byte-range serving. PDF.js uses ranges. | [loslab](https://blog.loslab.com/en/pdf-essentials/pdf-fast-web-view-optimization-explanation-tips-of-pdf-linearization), [PDF.js FAQ](https://github.com/mozilla/pdf.js/wiki/Frequently-Asked-Questions) | Minor for local files. Matters for opening over network or cloud. Offer "Save optimized/linearized" for Acrobat parity. PDFium's `FPDFAvail_*` API supports progressive availability (unverified). |
| **Incremental save** | Appends changed objects plus a new xref and keeps prior bytes intact. Required to keep existing signatures valid, and fast on huge files. | [Nutrient](https://www.nutrient.io/blog/incremental-and-full-save-in-pdfs/), [Documenso libpdf](https://libpdf.documenso.com/docs/concepts/incremental-saves) | Make incremental the default Ctrl+S, with full rewrite or "optimize" on demand. PDFium has an `FPDF_INCREMENTAL` save flag. Verify how pdfium-render exposes it (unverified). |
| **Lazy parsing** | PDF.js loads only what is needed for visible pages. Viewers parse the xref and page tree, then pages on demand. | [PDF.js FAQ](https://github.com/mozilla/pdf.js/wiki/Frequently-Asked-Questions) | Open on xref plus page count only. Load page dimensions lazily (PDFium `FPDF_GetPageSizeByIndex` avoids a full page load). Build text and search indexes in the background. |
| **Off-UI-thread work** | PDF.js parses in a worker. MuPDF renders on threads. pdfium-render forces a global lock. | [summary](https://tessl.io/registry/tessl/npm-pdfjs-dist), [pdfium-render](https://docs.rs/crate/pdfium-render/latest) | Run a dedicated PDFium render thread, or N worker *processes* for real parallelism. The UI thread never touches PDFium. |
| **GPU compositing** | Browsers composite canvas or tile layers on the GPU. Rasterization stays on the CPU in PDFium, MuPDF and hayro. pdf_render/Pathfinder tried GPU raster. | [pdf-rs](https://gitblind.noratr.app/pdf-rs) | Upload tiles as GPUI image textures and let the GPU handle scroll, zoom-scaling and overlays such as annotations, selection and the text layer. Re-rasterize only on zoom settle. GPU vector raster (vello-style) is a long-term research option. |
| **Asset hygiene** | PDF.js notes cost depends on page size and image resolution, not page count. | [PDF.js FAQ](https://github.com/mozilla/pdf.js/wiki/Frequently-Asked-Questions) | Optimize/compress tools (downsample images, subset fonts) are an Acrobat Pro parity feature that also makes files faster to view. |

## 4. Top UX steals (summary)

1. **Command registry, palette and rebindable keymap**: SumatraPDF's Ctrl+K with prefix modes and hot-reloaded text keymap, plus PDF-XChange's conflict-aware binding dialog.
2. **Annotation tool ergonomics**: Okular's number keys and double-click to lock a tool; PDF-XChange's hold-Space-for-pan, exclusive mode, and calibrated measurement with CSV export.
3. **Thumbnail-sidebar page ops**: Preview's multi-select, drag reorder, drag between windows, and file drop to insert.
4. **Optional vim profile and recolor mode**: zathura.
5. **Never-blank rendering**: a low-res base under hi-res tiles that PDFium can pause, with cancellation on scroll.
