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

**One sheaf at a time.** Opening a file while sheaf is running, from the
file manager or with `sheaf file.pdf`, hands it to the running sheaf
instead of starting another: a file that's already open comes forward, and
the rest open in windows of their own. Each user on a machine has their
own. `sheaf --new-instance` runs a separate sheaf beside it, which doesn't
remember window layout or recent files.

### Command line

Some work runs without a window. `sheaf <subcommand> …` runs it headless,
and so does `sheaf-cli <subcommand> …`, a separate binary with no GUI at
all, for servers and CI. `--help` on either lists the options. A progress
bar shows on stderr (`-q` hides it), and Ctrl-C stops the work cleanly
(exit status 130).

```sh
sheaf export binder.pdf -o pngs/ --pages "1-3, 10-" --dpi 200
sheaf export binder.pdf -f tiff --tiff bw -o binder.tif
sheaf-cli export binder.pdf -f text -o binder.txt
sheaf scan photos/*.jpg -o receipts.pdf --mode bw --ocr
sheaf remove-blanks scans.pdf -o clean.pdf --rejects removed.pdf
sheaf comments summary binder.pdf -l pages --summary stamps
sheaf comments csv binder.pdf -c number,page,author,text --author "Pat Smith" > pat.csv
```

| Subcommand | Does |
|---|---|
| `combine` | PDFs and images into one PDF in order; `file:range` takes some pages (`a.pdf:1-3,7`). A bookmark per file (`--no-file-bookmarks` to skip), `--image-page` |
| `export` | Pages to PNG, JPEG, TIFF or text, as File > Export Pages. Prints the files written; refuses to overwrite without `--force` |
| `split` | One PDF into several: `--every N`, `--bookmarks` (a file per top-level bookmark), `--max-size 10MB`, or `--ranges "1-3; 4-10; 11-"` |
| `remove-blanks` | Blank, and "intentionally left blank", pages out of a PDF, as Document > Find Blank Pages…. Prints each page found and whether it goes; `--dry-run` writes nothing; `--rejects` keeps the removed pages in a file of their own, bookmarked "p. 4 — blank". Nearly blank pages (a signature) stay unless `--nearly-blank`; `--sensitivity strict\|normal\|lenient`, `--keep-intentional`, `--pages` |
| `scan` | Photos or scans of paper to a clean PDF: finds the page in the photo and straightens it, turns it upright, levels the text, evens out shadows and color casts. `--mode color\|gray\|bw`, `--page letter\|a4\|original`, `--dpi`, `--ocr` (searchable text that lines up with the page), `--no-crop`/`--no-deskew`/… to skip a step, `--drop-blanks` to leave out blank pages, `--flatten-curl` for curled or bent pages (UVDoc model; `scripts/fetch-ocr-data.sh --uvdoc`) |
| `images-to-pdf` | JPEG, PNG, TIFF (every page), BMP, GIF and WebP files to one PDF, a page each: `--page original\|letter\|a4`, `--margin`, `--dpi` |
| `comments` | A PDF's comments, as File > Comments: `summary` (a PDF; `-l pages` puts each page beside its numbered list), `csv` (`-c` picks the columns), `text`, or `export` (XFDF or FDF, by `-o`'s extension or `--format`), to stdout unless `-o`. `import file.pdf data.xfdf…` brings comments from FDF, XFDF or other PDFs into a copy (`-o`; default "<name> - imported.pdf"), updating those it has by name. Filter and sort as the Comments panel does: `--pages 3-10`, `--type`, `--author`, `--status` (each repeats), `--checked`/`--unchecked`, `--summary stamps\|notes`, `--search`, `--sort page\|author\|date\|type\|color` |

## Why

Acrobat has become slow enough that routine work — opening a 400-page scanned
workpaper binder, reordering pages, dropping a tickmark — is painful,
especially on older hardware. PDFlyer makes the workflow tolerable but is
Windows-only and runs inside a paid copy of Acrobat Standard or Pro (64-bit
Acrobat since PDFlyer v20; v22 dropped Windows 10).

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
| macOS (arm64) | 2 | Should mostly fall out of the Linux work. CI builds and tests it; not blocking releases. |

Tier 1 platforms gate every release and run the perf budgets in CI.

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│  sheaf-app          GPUI application, views, commands    │
├────────────────────────────┬────────────────────────────┤
│  sheaf-cli (no GUI)        │  sheaf-ops: operations     │
│  clap subcommands          │  shared by app and CLI;    │
│                            │  sheaf-progress: progress  │
│                            │  + cancel trait            │
├────────────────────────────┴────────────────────────────┤
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
| Save model | Incremental update by default; "Save As (rewrite)" optional | Preserves originals, fast saves on large files. Clean rewrite available for size reduction / "Export Clean Copy". After anything is removed (a page, a mark, a reply, a bookmark, OCR text replaced) the next save is a full rewrite, so what was removed isn't left in an earlier revision; undoing back to what was saved goes back to incremental. No save or copy carries a page it doesn't keep: links, named destinations, the open action, form fields, tags and article threads that pointed at one are dropped or pruned. |
| Undo | A tree of versions, each with the edits that made it | Every edit is an `Edit` command. A version keeps a snapshot (cheap, structure-shared) and the commands from its parent; undo, redo and jumps move between versions, an edit after undo starts a branch instead of discarding, and a trimmed snapshot is rebuilt by replaying commands. Per-document. |

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
| Continuous / single / facing page modes | ✅ View > Page Display: Single Page, Two-Up (each with Continuous), Show Cover Page |
| Zoom (fit page / fit width / custom), rotate view | ✅ rotate view is display only (Ctrl+Shift+Plus/Minus) |
| Thumbnails sidebar (lazy, virtualized) | ✅ |
| Bookmarks (outline) sidebar | ✅ |
| Text search (find in document) | ✅ progressive; ~6 ms/page |
| Select and copy text | ✅ Select tool (V): drag over text, or double-click a word; Ctrl+C copies it. Recognized (OCR) text works before it's saved. Press U with text selected to highlight it |
| Pattern search (regex: SSNs, EINs, amounts, dates) — separate tool from Find, shared engine with redaction | ✅ Patterns panel, Ctrl+Shift+F |
| User-defined patterns: add, save, and delete presets; several date formats built in | ✅ `patterns.json`; right-click a preset |
| Multiple documents, one window each | ✅ |
| Full screen / read mode | ✅ View > Full Screen (Ctrl+L; click or arrows turn pages, Esc leaves), Read Mode (Ctrl+H) |
| Page labels (display) | ✅ status bar and thumbnails; View > Page Navigation > Go to Page (Ctrl+Shift+N) takes a label or number |
| Rulers, grid, guides | ✅ View > Rulers (Ctrl+R), Grid (Ctrl+U); drag from a ruler for a guide |
| Split view (same document, two places) | ✅ View > Split (Ctrl+\\); F6 switches pane |
| Attachments panel | ✅ View > Panels > Attachments (Ctrl+Alt+6): open (PDFs in sheaf, others with the system program after a warning) or save out. **Attach File** (Shift+J from Stamp) puts a file on the page behind a push pin, paperclip, graph or tag; it's embedded when you save, and double-clicking the icon opens it. Flattening keeps the icon and drops the file, as Acrobat does |
| Open PDF Portfolios (browse and extract embedded files) | ✅ opens on its cover sheet with the Attachments panel: folders, metadata columns, open/save |
| Recent files | ✅ File > Open Recent (last 10, shared by every window) |
| Document Properties | ✅ File > Properties (Alt+Enter): description, security, fonts |
| Layers (OCG) panel | ✅ View > Panels > Layers (Ctrl+Alt+7): show or hide for viewing; never saved |
| History panel | ✅ View > Panels > History (Ctrl+Alt+8): every version since the file opened, who made it and when, and its edits in words; click (or Up/Down) to go back to any version; edits undone and then replaced stay as a branch you can go into |
| Configurable toolbar: any command or plugin command, docked to any edge or floating | ✅ View > Customize Toolbar…, or right-click a menu item, palette row or toolbar button; `toolbar.json` |

### Page organization
| Feature | Status |
|---|---|
| Reorder pages (drag in thumbnails) | ✅ |
| Rotate **individual** pages (PDFlyer-style, not whole doc) | ✅ |
| Delete / extract / duplicate pages | ✅ Document > Extract Pages… keeps marks and relevant bookmarks |
| Insert PDF at position (PDFlyer "Insert PDF") | ✅ Document > Insert Pages > From File… |
| Merge bookmarks on insert (parents merged, children appended) | ✅ |
| Combine Files: many PDFs and images into one, choosing pages from each | ✅ File > Create > Combine Files…: add files or a folder or drop them, reorder, a page range per file, a bookmark per file holding its own bookmarks, images at their size or fitted to Letter/A4. `sheaf combine -o out.pdf a.pdf:1-3 scan.tif b.png` |
| Split document (by page range, by bookmark, by size) | ✅ Document > Split Document…: every N pages, at each top-level bookmark (files named after them), by maximum file size, or by page ranges; unsaved changes included; Esc stops it. `sheaf split` |
| Insert blank page / from image | ✅ Document > Insert Pages > Blank Pages… (size of the current page or Letter/A4/Legal, how many, where) and From Images… (fitted to a page size or at their own) |
| Remove blank pages | ✅ Document > Find Blank Pages…: blank, nearly blank (a signature) and "intentionally left blank" pages found by content and pixels (margins, off-white paper, bleed-through and dust ignored); reviewed with thumbnails before one undoable delete. `sheaf remove-blanks` |
| Replace pages | ✅ Document > Replace Pages…: pages of this document with a run of another PDF's; bookmarks and comments on them stay |
| Insert from scanner | 📋 M5 |
| Crop pages | ✅ Document > Crop Pages…: margins (in, mm or pt, as the page is shown) or trim white margins, on the current, selected, all or a range of pages; marks and OCR text stay on their content; saved as `/CropBox` |
| Page labels (roman numerals, prefixes) | ✅ Document > Page Labels…: style, prefix and start for a run of pages; the pages after keep their labels; labels follow their pages through reorders and are written as `/PageLabels`; shown in the status bar and thumbnails, and Go to Page takes them |
| Headers / footers / Bates numbering | ✅ Document > Header & Footer… and Bates Numbering…: six places with page number, page count, date, file name and Bates tokens; size, color, margins, page range; shown on the pages at once, written as tagged artifacts like Acrobat's on save; Update and Remove work on ours and Acrobat's |
| Watermarks / backgrounds | ✅ Document > Watermark… and Background…: text or an image, opacity, rotation, nine positions, page range; in front of or behind the content; written as tagged artifacts like Acrobat's; shown exactly as saved; Update and Remove |

### Bookmarks (PDFlyer-heavy)
| Feature | Status |
|---|---|
| Create / rename / delete / nest bookmarks | ✅ |
| Move bookmark **with its pages** | ✅ Shift+drag |
| Colorize bookmarks | ✅ per bookmark; Document > Color Bookmarks by Level…; colors and bold/italic from the file (Acrobat's, PDFlyer's) are kept. PDFlyer colors by populated vs empty; that's 📋 ([Pending](ROADMAP.md#pdflyer-parity-gaps)) |
| **Set Status** (PDFlyer) | ✅ Document > Set Status, or right-click a bookmark: colors and styles it, borders each of its pages, and stamps them with the status and your initials and date; Set Status Again repeats the last one, Reset Status takes it off (the stamps too, if you say so). One undo step each |
| Bookmark from page range | ✅ Document > Bookmarks from Pages…: one per page, titled from a template (`Page <<label>>`), optionally under a new parent |
| Bookmark pattern matches | ✅ Patterns panel > Bookmark matches: one per match, pointing at it |
| Target sheets / auto-bookmark from scan separators | ❓ |

### Markup & annotation
| Feature | Status |
|---|---|
| Sticky notes, highlight, underline, strikethrough | ✅ also squiggly. Drag across text (or select it, then press the tool's key: U highlights); Shift+U cycles highlight, underline, strikethrough, squiggly. Each tool keeps its own color, as Acrobat's do: yellow notes and highlights, red strikethroughs, drawings and tickmarks, blue text edits, until you pick another for it; with the Select tool, the swatches recolor the selected marks |
| Text edits: replace or insert text (Acrobat's) | ✅ Text Edits (E): drag across text and type the replacement, or click between characters and type what to insert. Written as Acrobat writes them (a Caret, with the StrikeOut grouped under it); the Comments panel reads Replace “x” with “y” |
| Free text, shapes (rect, ellipse, line, arrow, polygon) | ✅ also polyline, cloud (Q: a cloudy polygon; any rectangle, ellipse or polygon can be made cloudy) and callout (a text box with a leader; Shift+X from the text box). Polygons: click each corner, double-click or Enter finishes, Backspace takes one off. Shift+D cycles the drawing tools. Every handle drags: corners, vertices, a callout's box, knee and point |
| Ink / freehand | ✅ |
| Edit placed marks: move, resize, restyle | ✅ Drag to move (Ctrl+drag under any tool), handles to resize (Shift keeps the shape, or snaps a line to 45°), Shift+click to select several. A properties bar sets color, fill, opacity, line width, dashes, arrowheads at either end and text size; each change is one undo step |
| Copy and paste marks | ✅ Ctrl+C, Ctrl+V (offset), Ctrl+Alt+V (in place), across documents too |
| Stamps (built-in + custom) | ✅ tickmark library; a stamp library of your own (Stamps panel, Ctrl+Alt+9; Stamp tool J): stamps from images (transparency kept) or PDF pages, in collections, with rename and remove; Recent and Favorites; dynamic stamps (Approved, Reviewed, Received, Prepared) with your initials or name and the date, and dynamic text over a stamp of your own. Collections export and import as PDFs, the way Acrobat keeps custom stamps |
| **Tickmarks** (PDFlyer): quick-stamp library, keyboard-driven | ✅ |
| **Connectors** (PDFlyer): linked markers that jump between pages | ✅ label groups as in PDFlyer (1–50, A–ZZ): the tool keeps its label, so each click adds to the group (Shift+I starts a new one); clicking one goes to the next. Saved as Link annotations that walk the group in page order, so they jump in Acrobat too. Previous/Next ([ and ], or right-click) and "pages with / without" work for tickmarks and stamps as well |
| **Calculator tape** (PDFlyer): 10-key calc, tape placed as annotation | ✅ Calculator panel (Ctrl+Alt+4), in PDFlyer's format: sections numbered `1-1`, `1-2`… `1-T`, titles, subtotal (Space) and total (Enter), grand total (Shift+Enter), notes on rows (double-click; Ctrl+T dates them), memory (G H J K L), 00 and 000, change sign (S), decimals (D), insert and delete rows with the totals worked out again. Double-click a placed tape to edit it; turn it, resize it, hide its numbers or titles, and color its paper. **Pick from page** (a sheaf extension): click a number on the page to add it, or Shift+drag over a box for all of them, with a tickmark beside each if you like; it reads recognized (OCR) text too |
| Comments list / review panel, status, replies | ✅ Comments panel (Ctrl+Alt+5) lists ours and third-party, in threads. Reply (R; Ctrl+Enter sends), set a status (right-click: None, Accepted, Rejected, Cancelled, Completed, with its history), checkmark (Shift+K), on anyone's comments. Written as Acrobat writes them, and Acrobat's replies to our marks stay attached across saves. Filter by type, author, status, color, checkmark and pages (what's filtered out is hidden on the page too; Show All undoes it), sort by page, author, date, type or color, and PDFlyer's Stamp Summary and Note Summary as presets. Other tools' comments can be deleted (one undo step; gone from the file at save). View > Show Comments hides them all |
| Comment summary and export | ✅ File > Comments, or the panel's Export menu: what the panel lists, filtered and sorted as it is, as a summary PDF (the comments numbered and threaded; or each page beside its list, with the numbers on the page), a CSV (columns chosen in the Export menu: number, page, page label, type, author, dates, status, checked, color, text, reply-to) or text. `sheaf comments` |
| Import / export comments (FDF, XFDF) | ✅ File > Comments > Import… takes FDF, XFDF or another PDF's comments: one that's here already (by its `/NM`) is updated, the rest added, as one undo step, and kinds sheaf can't hold are listed. Export XFDF or FDF of what the panel lists, or (panel's Export menu) what's selected. `sheaf comments import`/`export` |
| Edit other tools' comments | ✅ Another tool's note, highlight, shape, line, ink, text box, callout, text edit or stamp becomes a sheaf mark when it's edited: drag it, double-click it (or F2 in the panel) to edit its text, or right-click > Make Editable in the Comments panel. It keeps its name, so its replies stay with it, and a save puts it where it was |
| Flatten annotations | ✅ File > Export Flattened Copy… (links and form fields kept) |
| Export Clean Copy (strip annots, notes, tapes, stamps, attachments) | ✅ File > Export Clean Copy; keeps links, form fields, bookmarks |
| Measurement tools (distance, area, perimeter, scale) | ✅ Tools > Measure: Distance (Ctrl+Shift+D, a drag), Perimeter (Ctrl+Shift+P) and Area (Ctrl+Shift+A, corner by corner, as Polyline and Polygon). The scale bar takes a scale as typed ("1 in = 10 ft", "1/8 in = 1 ft", "1:100") and the decimals; Calibrate (Ctrl+Shift+C) is a line of known length, then its real length. The status bar shows the measurement while drawing; Shift keeps to 45°, and points snap to the grid and guides when they show. Written as Acrobat's dimension annotations (`/Measure`), with the value on the mark; the comment CSV has Measurement and Scale columns |

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
| Export pages to PNG / JPEG | ✅ File > Export Pages (Ctrl+Shift+E): resolution, JPEG quality, page range, marks included or not; Esc stops a running export. Also `sheaf export` on the command line |
| Export to multi-page TIFF (PDFlyer) | ✅ File > Export Pages > TIFF: one file, black-and-white (CCITT G4), grayscale or color |
| Create PDF from images | ✅ File > Create > From Scans/Photos…: phone photos or scans cleaned up (page found and straightened, upright, deskewed, shadows and color casts evened out), color/gray/black-and-white, Letter/A4/as photographed, optional OCR, blank pages left out if asked (the preview dims them), with a preview of each step. `sheaf scan` and `sheaf images-to-pdf` on the command line |
| OCR (searchable text layer) | ✅ Document > Recognize Text (OCR)… (Ctrl+Shift+T): Tesseract or PaddleOCR, whole binder / current / selected / range, skips pages that already have text, progress + Esc to stop, one undo step; Find and Patterns see the text before the first save |
| Export text | ✅ File > Export Pages > Text: one UTF-8 file, a form feed between pages (unsaved OCR text included) |
| Print | ✅ File > Print (Ctrl+P): own dialog with sheet preview; printer, copies, pages (all/current/selected/bookmark/range), Fit/Actual/Shrink, orientation, paper, two-sided, comments on/off; CUPS on Linux/macOS, GDI on Windows; Save as PDF |
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
| M7 | **Viewing & output**: page modes and rotate view, split view, full screen / read mode, rulers / grid / guides, page labels, attachments, portfolios, layers, document properties, recent files, print with preview (`sheaf-print`: CUPS, GDI), export to PNG / JPEG / TIFF (G4) / text. *Slices done 2026-10-14 (Linux; printing tested against stand-in CUPS tools, the Windows print backend only type-checked) — see [`todos/m7.md`](todos/m7.md).* | Viewing and printing never need Acrobat |

M8 onward (viewing and output through v1.0 parity) is laid out in [ROADMAP.md](ROADMAP.md).

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
- ~~**PDFlyer "Target Sheets" / Scan integration.**~~ Decided: CCH target
  sheets are proprietary barcodes read by CCH Scan, so sheaf gets a generic
  barcode / patch-code / keyword separator sheet → bookmark or split instead
  (ROADMAP M5).
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
                                 #   (--uvdoc adds the curled-page model scan cleanup can use)
scripts/build-plugins.sh         # the bundled plugins (plugins/) → target/plugins/
cargo run -p sheaf-app           # the GPUI window
                                 #   (debug builds don't hand files to a running
                                 #   sheaf unless SHEAF_SINGLE_INSTANCE=1)
```

**Plugins.** sheaf runs WebAssembly plugins (the `plugins` feature, on by
default; `--no-default-features` leaves out wasmtime too). The bundled
plugins build for `wasm32-wasip2`, which `rust-toolchain.toml` installs;
debug builds pick them up from `target/plugins`. The API is
`sheaf:plugin` 0.3, which has every kind of mark and a plugin's own
section of `settings.toml`; plugins built for 0.2 still load. Writing
your own: [`docs/plugins.md`](docs/plugins.md).

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

The bundle is the release binaries (`sheaf` and the GUI-less `sheaf-cli`)
next to the PDFium library, the OCR data
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

sheaf keeps what you choose in the platform config directory:
`~/.config/sheaf` on Linux, `%APPDATA%\sheaf` on Windows (which roams with
your profile), `~/Library/Application Support/sheaf` on macOS. What it
remembers for itself goes in a local state directory: `~/.local/state/sheaf`,
`%LOCALAPPDATA%\sheaf\state`, `~/Library/Application Support/sheaf/state`.
`SHEAF_CONFIG_DIR` overrides both (state goes in its `state/`).

- `settings.toml` — your preferences, edited by hand; there's no settings
  view yet. Edit > Open Settings File opens it, writing one with every
  setting explained if there isn't one. sheaf only reads it, again whenever a
  window comes forward; a mistake shows in a banner with its line, and only
  that setting falls back to its default.
  - `[identity]`: `name` (the author of your comments and marks; your user
    name otherwise) and `initials`.
  - `[ocr]`: what the Recognize Text dialog doesn't choose:
    `tessdata_dir` for your own Tesseract data, `det`, `rec`, `dict`,
    `textline_ori` and `doc_ori` for your own PaddleOCR models, `psm`, and
    `use_doc_orientation`.
  - `[stamps]`: `size` (how tall a stamp goes down, in points; 40), and
    `date_format` and `time_format` for dynamic stamps (strftime codes;
    `%m/%d/%Y` and `%-I:%M %p`).
  - `[plugins.<id>]`: a plugin's own settings, whatever it documents; only
    that plugin reads them (as JSON, through API 0.3). A key its manifest
    doesn't name is pointed out in Plugins → Manage Plugins….
- `state.json` (state directory) — written by the app: window placement,
  dock layout (which panels sit where, tab order, sizes, open state), and
  each dialog's last choices, among them the OCR engine, `dpi`, language and
  scope; recent stamps; the comment CSV's columns; the last measuring scale;
  each markup tool's color (`tool_colors`, by tool: `"strikeout": [0, 0, 0]`).
  Safe to delete; a copy that doesn't parse is set aside as
  `state.json.bad`. Only the sheaf that owns the instance writes it.
- `recent.json` (state directory) — File > Open Recent.
- `patterns.json` — the Patterns panel's presets, seeded from the built-ins on
  first run. Edit, delete, or add them from the panel (right-click a chip;
  "+ Save…" stores the current pattern) or by hand.
- `stamps/` — your stamp collections, a PDF each with a stamp per page
  (an Acrobat custom-stamp file is the same shape), and `favorites.json`.
  Recent stamps are in `state.json`.
- `marks.json` — extra tickmark sets and stamps for the markup toolbar:
  `{ "tickmarks": [{ "name": "Audit", "marks": ["Σ", "◊"] }], "stamps": ["PBC"] }`.
- `plugins/` — installed plugins, one folder each (see
  [`docs/plugins.md`](docs/plugins.md)); `plugins.json` records which are
  turned off and what each may access; `plugin-data/<id>/` holds each
  plugin's storage and the `settings.json` you can give it. Manage them from
  Plugins → Manage Plugins….
- `toolbar.json` — the toolbar's buttons and where it sits, written when you
  customize or move it (View > Customize Toolbar…, or drag its grip to an
  edge or over the page). Items are command ids as the palette shows them,
  `"separator"`, the `"toolbar::Colors"` and `"toolbar::Tickmarks"` widgets,
  or `["plugin::Run", { "plugin": "<id>", "command": "<id>" }]`. A plugin's
  buttons hide while it's off and come back with it.

```json
{ "toolbars": [{ "name": "Main", "dock": "top",
    "items": ["docview::Save", "separator", "tools::ToolHighlight", "toolbar::Colors"] }] }
```

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
used (AGPL). lopdf (MIT) is vendored with a patch in `vendor/lopdf`.
