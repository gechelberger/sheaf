# M7: Viewing & output

Status: done 2026-10-14 (Linux), all 14 slices, one commit each; notes per
slice at the end. Not yet exercised on Windows: printing (GDI backend
type-checks only) and the rest of M7 generally.

## Decided at kickoff (2026-10-14)

**Print uses sheaf's own dialog with a live preview, not the OS dialog.**
Acrobat does the same: its dialog has the printer, copies, page range,
sizing (Fit / Actual size / Shrink oversized), orientation, comments on or
off, and a preview of the sheet. The native dialogs can't preview sheaf's own
marks or offer "pages of the selected bookmarks". A "Printer Properties…"
button can open the driver's own dialog later on Windows.

The job itself:

1. **A print PDF** is written from a snapshot of the pages being printed: the
   same rewrite as Extract Pages. With "Comments and markups" on, every markup
   annotation is flattened into the page content (the Export Flattened Copy
   code). With them off, markup is stripped (the Export Clean Copy code).
   Form fields print either way.
2. **Linux and macOS: CUPS through its command-line tools.** `lpstat -e` /
   `lpstat -d` list printers, `lpoptions -p <printer> -l` gives paper sizes,
   and `lp -d <printer> -n <copies> -o media=… -o print-scaling=…
   -o orientation-requested=… job.pdf` submits the job. This avoids linking
   libcups and its version differences, and `lp` is present wherever CUPS
   is. CUPS's `print-scaling` (`fit`, `none`, `auto`) are Acrobat's Fit,
   Actual size and Shrink oversized, so the preview uses the same math. No
   desktop portal: it's sandbox-only and offers no preview control.
3. **Windows: GDI.** `EnumPrintersW` / `GetDefaultPrinterW` list printers,
   `DeviceCapabilitiesW` gives paper sizes, and the job is drawn with
   `StartDocW` and one `StartPage` per sheet. PDFium rasterizes each page of
   the print PDF at the printer's resolution in bands, blitted with
   `StretchDIBits` at the placement the preview shows.

The platform code lives in a new crate, `sheaf-print`, so it can be
type-checked for Windows on its own (the app's OCR dependencies don't
cross-compile).

**Page modes follow Acrobat:** Single Page, Single Page Continuous, Two-Up,
Two-Up Continuous, with "Show Cover Page" for the two-up modes. These are
View settings for each window, not saved with the document.

**Split view is Acrobat's Window > Split:** two panes stacked vertically on
the same document. Each pane has its own scroll position, zoom, page mode
and rotate view. Caches (tiles, previews) are shared. The pane last clicked
is active: menus, keys and the status bar act on it.

## Slices

1. **Page modes + rotate view.** Lay pages out in rows (spreads) and add a
   per-pane `Pane` state (zoom, mode, view rotation, scroll, layout, current
   page). Non-continuous modes show one row; scrolling past either end turns
   the page. Rotate View (Ctrl+Shift+Plus / Ctrl+Shift+Minus, as in Acrobat)
   rotates the display only: thumbnails, edits and saves don't see it.
2. **Split view.** View > Split (Ctrl+\\): a second pane with a draggable
   divider; each pane renders from the shared caches.
3. **Full screen / read mode.** Full Screen (Ctrl+L; Esc leaves): the window
   fills the screen with no chrome or docks, single page, fit page, arrows
   and clicks turn pages. Read Mode (Ctrl+H) hides the docks and toolbar but
   keeps the window.
4. **Rulers, grid, guides.** Rulers (Ctrl+R) along the top and left of the
   active pane in the page's units (inches by default); grid (Ctrl+U)
   overlaid on pages; drag from a ruler to make a guide; guides are kept
   per document while it's open.
5. **Page labels (display).** Read `/PageLabels` from each source. Show the
   label in the status bar ("iv (4 of 120)") and under thumbnails; a Go to
   Page box (Ctrl+Shift+N) accepts a label or a number.
6. **Attachments panel.** List the document's embedded files (name,
   description, size, modified), open one (with the system handler, after a
   warning for anything not a PDF; PDFs open in sheaf) and save it out.
7. **Portfolios (read-only).** When the catalog has `/Collection`, open in
   portfolio view: the attachments panel shows its folders and metadata
   columns and opens first, and the cover sheet is the page shown.
8. **Layers panel.** List optional content groups and toggle visibility
   (display only). PDFium renders with the default configuration, so a
   toggle reopens the source from a copy of its bytes with the `/OFF` array
   rewritten in memory.
9. **Document Properties.** File > Properties (Alt+Enter; Acrobat's Ctrl+D
   is Duplicate Pages in sheaf): file,
   title/author/subject/keywords, producer/creator, dates, PDF version, page
   size, page count, tagged, fast web view, fonts list, security summary.
10. **Recent files.** File > Open Recent, kept in settings, 10 entries,
    missing files dropped when shown.
11. **Print dialog + preview.** `sheaf-print` crate, File > Print (Ctrl+P):
    printer, copies, pages (all / current / range / selected pages /
    selected bookmark's pages), sizing (Fit, Actual size, Shrink oversized),
    orientation (auto, portrait, landscape), comments on/off, paper size,
    and a preview with page navigation. Booklet and n-up stay ❓.
12. **Export to images.** File > Export > Images: PNG or JPEG, resolution,
    page range; one file per page.
13. **Export to TIFF.** Multi-page TIFF (PDFlyer), resolution, color or
    black-and-white (CCITT G4) for scanned binders.
14. **Export text.** Plain text, page breaks as form feeds; "with layout"
    stays ❓.

## Slice notes

- **Slice 1 (page modes + rotate view): done.** `pane.rs` holds `Pane`
  (per-view zoom, mode, rotate view, scroll, layout, current page) and the
  row-based `Layout`; `Doc` has `panes` and `active`. Previews are cached by
  (page, rotation) so the thumbnails and a rotated view don't evict each
  other. Rotate view is composed into `Layout::refs`, so hit-testing,
  marks and search hits all map through the shown rotation; edits use the
  model's pages (`Doc::shown_ref` is for display math only). Next/Previous
  Page step a spread in two-up modes. Menus have no check marks yet, so the
  status bar names the mode when it isn't the default.
- **Slice 2 (split view): done.** View > Split (Ctrl+\\) adds a second
  `Pane` (top/bottom, draggable divider, 10–90%); F6 or a click makes a
  pane active, shown by an accent edge. Closing the split keeps the active
  pane's view. Cache eviction runs once over every pane's plan, so two
  panes at different zooms or rotations keep each other's tiles.
- **Slice 3 (full screen / read mode): done.** `presentation.rs`. Full
  Screen saves the active pane's mode and zoom, shows only that pane at
  Single Page / Fit Page on black, and toggles the OS full-screen state
  (unless the window was already full screen). Click/right-click or
  Shift+click turn pages. Read Mode hides the toolbar and docks only.
  Also fixed on the way: a pane that resizes now keeps the point at its
  top centre in place (it used to jump pages when leaving full screen).
- **Slice 4 (rulers, grid, guides): done.** `rulers.rs`. Rulers sit on
  each pane, in inches from the top-left of that pane's current page (as
  shown, rotate view included), with ticks as fine as fit (to sixteenths).
  The grid is one canvas per page: inches, plus faint quarters when
  they're at least 8 px apart. Guides apply to every page, live for the
  session, and are cleared with View > Clear Guides; moving or deleting a
  single guide isn't there yet. Units are inches only for now.
- **Slice 5 (page labels, display): done.** `sheaf_doc::labels` reads the
  `/PageLabels` number tree (Kids and Nums; D/R/r/A/a, prefix, start).
  `file::read_open` loads the file once at open for annotations and
  `FileFacts` (the slices after this add to it). Only the primary file's
  pages get labels; inserted pages show their position. Saving still drops
  `/PageLabels` (it did before M7), so labels go away after the first save
  until M8 writes them. Go to Page (`goto.rs`) matches a label first, then
  a number.
- **Slice 6 (attachments panel): done.** `sheaf_doc::attachments` lists
  `/EmbeddedFiles` (name tree, Kids and Names) and FileAttachment
  annotations, addressed by name-tree key or (page, nth annotation) so a
  reload finds them again; bytes are read on demand (Flate etc. via
  lopdf). `sheaf_doc::portfolio` (schema, folders, `/CI` values) went in
  with it since attachments carry their folder and metadata. Opened
  attachments go to a per-process temp directory with a sanitized name;
  non-PDFs open through `xdg-open`/`open`/`explorer` after a warning.
  Facts are re-read after a save. Saving keeps `/Names`.
- **Slice 7 (portfolios, read only): done.** A file with `/Collection`
  asks the workspace (`DocView::take_wanted_panel`) to show Attachments,
  sorts the files by folder, selects the portfolio's initial file (`/D`)
  and shows its visible text/date/number schema columns under each file.
  The cover sheet is the document's own pages. Navigator/Flash layouts and
  per-column sorting aren't attempted.
- **Slice 8 (layers panel): done.** `sheaf_doc::layers` lists `/OCGs` in
  `/D /Order` (headings, nesting, `/Locked`) and `set_hidden` rewrites the
  default configuration (`BaseState /ON`, `ON`/`OFF`, drops `/AS`). A
  toggle loads the primary file with lopdf, rewrites, writes a temp copy,
  opens a new `RenderHandle` on it and deletes the copy (PDFium has read
  it all), then re-hides sheaf's own marks (`Doc::sheaf_pages`) and
  invalidates tiles. A generation counter drops stale reloads; after a
  save the hidden set is reapplied. Cost is a full lopdf load + save per
  toggle, fine for the layered drawings this is for.
- **Found on the way: red and blue were swapped in every rendered page**
  (pdfium-render sets `FPDF_REVERSE_BYTE_ORDER` by default). Fixed in its
  own commit with a test (`sheaf-render/tests/pixel_format.rs`).
- **Slice 9 (Document Properties): done.** `sheaf_doc::properties::read`
  runs when the dialog opens (not at file open): Info dict, version
  (header or catalog `/Version`), tagged, Fast Web View (`/Linearized` in
  the first 2 KB), security method and `/P` permissions (read before
  decrypting), and fonts from every page's resources and the form
  XObjects they draw (subset tags, embedded or not; 5,000-page cap). The
  workspace hosts it like the OCR dialog; Tab/←/→ switch tabs.
- **Slice 10 (recent files): done.** `recent.rs`: `recent.json` beside
  settings (not in `Settings`, which each window holds its own copy of),
  re-read before every change; documents are added on open and on save,
  sheaf's own temp files (`<temp>/sheaf-…`) aren't. File > Open Recent is
  a submenu the registry refills (`CommandRegistry::set_submenu`,
  `remove_prefix`); a missing file is reported and dropped. The title-bar
  menu (`sheaf_ui::menu_bar`) had no submenu support, so plugin commands
  placed at "Tools/Workpapers"-style paths showed as blank rows; it now
  opens submenus on hover, Right/Enter, and their mnemonics, and `&&` in a
  label is a literal `&`. The UI script's checkpoints now draw twice
  before the grab: the X11 grab could show the frame before last.
- **Slice 11 (print dialog + preview): done.** New crate `sheaf-print`
  (`place` placement math shared by preview and backends; `cups.rs` via
  `lpstat`/`lpoptions`/`lp`, `SHEAF_CUPS_BIN` for tests; `gdi.rs` with
  DEVMODE paper/orientation/copies/duplex/collate and 512-row PDFium bands
  into `StretchDIBits`). `sheaf_doc::save::print_pdf` writes the job file
  (subset like Extract Pages, no bookmarks, markup flattened or stripped);
  `DocState::bookmark_pages` exposes "move with its pages" spans for "print
  the selected bookmark". `print_dialog.rs` (workspace-hosted) remembers
  its choices in `settings.json` (`print`) and offers a Save as PDF
  pseudo-printer. Verified on Linux against fake CUPS tools (the job file
  and `lp` arguments); **the Windows backend type-checks
  (`cargo check/clippy -p sheaf-print --target x86_64-pc-windows-msvc`)
  but hasn't been run on Windows.** Known gap: the preview draws
  third-party annotations even with comments off (PDFium draws them; the
  printed file leaves them out). Booklet and n-up stay ❓.
- **Slice 12 (export to images): done.** `export.rs` renders from the
  same print file (so sheaf's marks show, or not), one file per page,
  `<stem>_Page_<n>` zero-padded; PNG (RGB, `pHYs` dpi) and JPEG (quality,
  JFIF dpi). `export_dialog.rs` (Ctrl+Shift+E) asks for a folder after,
  warns before replacing files, and shows progress in the status bar.
  Slices 13–14 add TIFF and text to the same dialog. New dev hook:
  `SHEAF_SCRIPT_PICK` answers every file picker, for UI scripts.
- **Slice 13 (TIFF): done.** Export Pages gains TIFF: one multi-page
  file (asked for by name), black and white as CCITT G4 (the `fax` crate's
  encoder, thresholded at mid-gray; IFDs written by hand through the
  `tiff` crate's directory encoder, WhiteIsZero, `PageNumber`), or
  grayscale/RGB with LZW. Written to `<name>.tif.part` and renamed.
  Checked: Pillow reads the files (3 frames, `group4`, 150 dpi) and a unit
  test round-trips G4 rows through the `fax` decoder.
- **Slice 14 (export text): done.** Export Pages > Text writes the pages'
  text (PDFium's reading order) from the print file, so unsaved OCR text
  layers are included; `\r\n` normalized, trailing spaces trimmed, a form
  feed line between pages. "With layout" stays ❓.
