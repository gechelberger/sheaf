# Roadmap

How sheaf gets from v0.1 ("the fast workpaper binder") to feature parity with
**Adobe Acrobat Pro** plus the **CCH ProSystem fx PDFlyer** workpaper tools.

"Parity" means Acrobat Pro's *desktop* feature set and PDFlyer's tax/audit
tools. Adobe's cloud features (shared review, Send for Signature, Document
Cloud storage) are not included; see [Not planned](#not-planned).

Ordering principle: the next thing a tax/audit preparer would otherwise open
Acrobat for comes first. The hardest engine work (in-place content editing)
comes late, once the object-layer groundwork from earlier milestones exists.

The README [feature tables](README.md#features) remain the authoritative
per-feature status. This file groups and orders the 📋 and ❓ items from them.

## Done

**M0–M4 → v0.1**: spikes, viewer, organizer, markup, polish. See the
[README milestone table](README.md#milestones).

## Milestones

| # | Milestone | Proves | Release |
|---|---|---|---|
| M5 | **Scan & OCR**: searchable text layer, scan cleanup, scanner intake, separator sheets | Scanned binders become searchable | v0.2 |
| M6 | **Plugins**: WASM components, declarative UI, command registry | Extensibility without forking | — |
| M7 | **Viewing & output**: page modes, split view, panels, print, image/TIFF/text export | Viewing and printing never need Acrobat | v0.3 |
| M8 | **Page tools**: split, insert/replace, crop, labels, headers/footers, Bates, watermarks | Binder assembly is complete | v0.4 |
| M9 | **Markup completion**: remaining annotation types, custom stamps, replies, FDF/XFDF, measurement | Review workflow parity | v0.5 |
| M10 | **Forms**: fill, calculate, create, import/export, flatten | Tax forms are filled and calculated in sheaf | v0.6 |
| M11 | **Security & redaction**: true redaction, sanitize, passwords and permissions | Client data can be sent out safely | v0.7 |
| M12 | **Signatures**: Fill & Sign, digital signatures, verification, timestamps | Engagement letters and 8879s are signed in-app | v0.8 |
| M13 | **Content editing**: text, images, links, objects | The hardest Acrobat feature | v0.9 |
| M14 | **Document processing**: compare, optimize, PDF/A, accessibility | Archival and output parity | v0.10 |
| M15 | **Automation → v1.0**: batch actions, CLI, parity audit | Acrobat Pro + PDFlyer parity | v1.0 |

M6 has no release of its own; it ships in v0.2 together with Scan & OCR.

Each milestone is broken into slices when it starts, as M1–M4 were. Slices
below are a first cut.

---

### M5: Scan & OCR

- ✅ OCR to an invisible, searchable text layer, per page or over a whole
  binder, in the background: `sheaf-ocr` with an `OcrEngine` trait and two
  engines, Tesseract 5 (static) and PaddleOCR PP-OCRv5 on ONNX Runtime
  (`oar-ocr`); Document > Recognize Text (OCR)…; one undo step; Find and
  Patterns search unsaved layers. Design in [`todos/ocr.md`](todos/ocr.md).
- ✅ Scan cleanup (`sheaf-scan`): deskew, despeckle, auto-rotate by detected
  text orientation (word shape, plus PaddleOCR's orientation model when
  present), black-and-white by Sauvola thresholding. Blank-page removal is
  still to do.
- ✅ Camera capture: phone photos of paper. Find the page and correct the
  perspective, clear the background left at its edges, even out shadows,
  lighting and white balance, and optionally flatten curled pages (UVDoc,
  opt-in). File > Create > From Scans/Photos…, with a preview of each step,
  and `sheaf scan`. Design in [`todos/cleanup-cli.md`](todos/cleanup-cli.md).
- ✅ Create PDF from images (JPEG/PNG/TIFF/BMP/GIF/WebP; multi-page TIFF in;
  JPEG kept as is, black and white as CCITT G4): `sheaf_doc::images`,
  `sheaf images-to-pdf`.
- Scanner intake: WIA/TWAIN on Windows, SANE on Linux.
- Separator sheet → bookmark: a generic barcode/patch-code/keyword separator
  that splits or bookmarks a scanned batch. This replaces PDFlyer/CCH Scan
  "Target Sheets".

Slices: OCR engine + text layer ✅ → batch OCR over binder ✅ → image import ✅
→ cleanup filters ✅ (with camera capture) → scanner intake → separator sheets.

**Decided at kickoff (2026-10-11):** OCR is a core feature, not a plugin
(Tesseract and ONNX Runtime are native code and can't run in wasmtime), with
both engines in-process behind a trait so they can be compared on scanned
tax forms and swapped later.

### M6: Plugins

Full design in [`todos/plugins.md`](todos/plugins.md).

- Data-driven command registry; menus and palette built from it.
- `DocView` events; `Document::apply_batch` as one undo step.
- WASM component host (wasmtime + WIT), manifests, discovery.
- Declarative UI: forms, notices, plugin-supplied panels.
- Permissions, plugin manager, SDK crate, authoring docs.

Later milestones use this host API where it fits. Batch actions (M15) are built
on it, and integrations such as CCH Workpaper Manager are left to plugins.

### M7: Viewing & output

**Done 2026-10-14** (Linux). Slice notes, the print decision and what's
left open (booklet / n-up, text "with layout", the Windows print backend
untested on Windows) are in [`todos/m7.md`](todos/m7.md).

Viewing:
- Single page, two-up (facing), and two-up with cover page modes.
- Rotate view (display only, not saved).
- Split view: the same document at two positions.
- Full-screen / read mode.
- Rulers, grid, guides.
- Page labels shown in the toolbar and thumbnails (display only; editing is M8).
- Attachments panel (list, open, save out).
- Open PDF Portfolios (read-only). When the catalog has a `/Collection`,
  open in portfolio view: the attachments panel lists the embedded files
  with their folders and metadata columns, the page view shows the cover
  sheet, embedded PDFs open in new windows, and any file can be saved out.
- Layers (OCG) panel: toggle visibility.
- Document Properties dialog (metadata, fonts, PDF version, security summary).
- Recent files.

Output:
- **Print**: printer selection, page ranges, scaling (fit/actual/shrink),
  annotations on/off, print selected bookmarks' pages, booklet/n-up ❓.
- Export pages to PNG / JPEG (resolution, range).
- Export to multi-page TIFF **(PDFlyer)**.
- Export text (plain; with layout ❓).

Slices: page modes + rotate view → split view → panels (attachments, layers,
properties) → print → image/TIFF/text export.

**Decided at kickoff (2026-10-14):** sheaf's own print dialog with a
preview, sending to CUPS (`lp`) on Linux/macOS and GDI on Windows; no
portal. Reasons in [`todos/m7.md`](todos/m7.md).

### M8: Page tools

- **Combine Files** (Acrobat's File → Create → Combine Files): build a new
  PDF from any number of PDFs and images (JPEG/PNG/TIFF, multi-page TIFF).
  - Add files or whole folders, or drop them in; reorder and remove in a
    list or thumbnail grid.
  - Expand a multi-page asset to choose a subset of its pages (pick
    thumbnails or type a range like `1-3, 7, 10-`).
  - Optional bookmark per source file (Acrobat's default), with each PDF's
    own bookmarks nested under it using the same merge rules as Insert PDF.
  - Image page size options: original size, or fit to Letter/A4. Image
    decoding reuses the M5 image import.
  - Command line: `sheaf combine -o out.pdf a.pdf:1-3 scan.tif b.png`.
  - Office files are not accepted (no Office conversion, see Not planned).
- Split document by page range, by top-level bookmark, or by file size.
- Insert blank page; insert pages from images (shares M5 image import).
- Replace pages (Acrobat's Replace Pages).
- Crop pages (CropBox), with the option to apply to a range.
- ✅ Page labels: edit numbering style, prefix, start.
- Headers / footers: add, update, remove (with date, page number, file name).
- Bates numbering.
- Watermarks and backgrounds (text or image, opacity, behind/in front).
- Bookmarks: create from a page range; colorize by level; auto-bookmark from
  a text pattern (reuses the Patterns engine).

Slices: Combine Files → split → insert blank/images + replace → crop →
page labels → headers/footers + Bates → watermark/background → bookmark
extras.

Headers/footers, Bates and watermarks are written as tagged artifacts the way
Acrobat writes them, so Acrobat can update or remove them later and so can we.

### M9: Markup completion

- Underline, strikethrough, squiggly, replace-text / insert-text marks.
- Polygon, polyline, cloud, callout (free text with leader).
- Custom stamps: from image or PDF page; dynamic stamps (date, time, user name).
- Sign-off / review stamps (preparer and reviewer initials + date) **(PDFlyer;
  verify against PDFlyer)**.
- Comment status (accepted/rejected/completed), replies, author and
  date filters, sorting.
- Import / export comments as FDF and XFDF.
- Comment summary report (comments with page thumbnails, to PDF).
- File-attachment annotations.
- Measurement: distance, perimeter, area, with page scale and calibration.
- Calculator tape that picks up numbers by clicking page text **(PDFlyer;
  verify against PDFlyer)**.

Slices: text-markup types → shape types → custom/dynamic stamps → replies and
status → FDF/XFDF + summary → measurement → calc tape from page text.

### M10: Forms

- Fill AcroForms: text, checkbox, radio, combo/list, with correct appearance
  stream generation so values look the same in every viewer.
- Field calculations and format/validate/keystroke scripts. IRS and state
  forms depend on these. Run a sandboxed subset of Acrobat form JavaScript
  (AFNumber_Format, AFSimple_Calculate and friends, plus simple custom
  scripts), not a general JS runtime.
- Create / edit fields: all field types, properties, tab order, alignment
  tools.
- Automatic field detection on flat forms.
- Import / export form data: FDF, XFDF, CSV/XML.
- Flatten form fields (Export Flattened Copy already keeps fields; add the
  option to flatten them).
- XFA: still not planned. Show a clear notice when a document is XFA-only.

Slices: fill + appearances → calculation/format scripts → field authoring →
auto-detect → data import/export → flatten.

**Decide at kickoff:** which JS engine (boa, QuickJS via rquickjs) and how big
a subset to allow. Measure against a corpus of real IRS/state fillable forms.

### M11: Security & redaction

- Redaction: mark areas or text, then apply with **true content removal**
  (text, images, vector paths under the mark) and optional overlay text.
- Pattern-driven redaction: Patterns panel hits become redaction marks (the
  shared engine planned since M2).
- Sanitize / remove hidden information: metadata, hidden layers, embedded
  files, scripts, unreferenced objects, content outside the crop box.
- Passwords and permissions: set, change, remove; AES-256.
- Preserve encryption on save (today a save writes a plain file).
- Certificate encryption ❓.

Slices: content-stream rewriter → apply redaction → pattern-driven marks →
sanitize → passwords/permissions + encrypted save.

**Shared infrastructure:** the content-stream parser/rewriter in `sheaf-doc`
built here is the base for M13 content editing and M14 optimize.

### M12: Signatures

- Fill & Sign: typed, drawn, or image signature and initials, saved for reuse.
- Digital signatures: sign with PKCS#12 files, the Windows certificate store,
  and PKCS#11 tokens; visible and invisible signatures; signature fields.
- Signature panel: validation status, certificate details, changes since
  signing.
- RFC 3161 timestamps; LTV (embed revocation info).
- Certify documents; lock fields on sign.

Slices: Fill & Sign → sign with PKCS#12 → verify panel → Windows store and
PKCS#11 → timestamps + LTV → certify/lock.

Incremental save (done in M2) is a prerequisite: signing must never rewrite
earlier revisions.

### M13: Content editing

- Edit text in place: reflow within a text block, font matching with
  substitution when the embedded font is subset, and a clear warning when
  a glyph is missing.
- Add text and image objects.
- Edit, replace, move, resize, crop images.
- Link editing: create, edit destination/action, delete; create links from
  URLs in text.
- Edit existing headers/footers (beyond the M8 update/remove).

Slices: object selection + move/delete → add text/image → image edit/replace →
text edit within a line → block reflow → links.

Builds on the M11 content-stream rewriter.

### M14: Document processing

- Compare documents: text diff and visual (pixel) diff, with a side-by-side
  report.
- Optimize / reduce file size: image downsampling and recompression, font
  subsetting, object streams, removing unused objects.
- PDF/A convert and validate ❓.
- Accessibility: tags tree, reading order, alt text, accessibility checker ❓.
- Preflight-lite (font embedding, color spaces, transparency checks) ❓.

**Decide at kickoff:** how far to go on the ❓ items. They matter for
archival requirements but not for daily binder work.

### M15: Automation → v1.0

- Action Wizard-style batch actions: run a sequence of commands (OCR, Bates,
  optimize, redact by pattern, flatten, export) over files and folders.
- Built on the M6 plugin host API, which settles the README's Rhai vs Lua
  question: plugins first, with an optional Rhai front end for quick macros
  if users want one.
- Headless CLI batch mode (`sheaf batch …`). The split is in place: GUI-free
  operations live in `sheaf-ops` (with `sheaf-progress` for progress and
  cancellation), and `sheaf-cli` holds the subcommands. They're reachable
  as `sheaf <subcommand>` or through the GPUI-free `sheaf-cli` binary;
  `export`, `images-to-pdf` and `scan` ship today. Ops move into
  `sheaf-ops` as subcommands need them, and `batch` chains them.
- Parity audit: walk Acrobat Pro's menus and tools and PDFlyer's toolbar
  item by item; anything still missing becomes a slice here or is moved to
  Not planned with a reason.
- Stabilization for 1.0: corpus round-trip tests, Windows bundle and CI
  workflows exercised, settings/keymap format declared stable.

---

## Parity checklist

Every remaining 📋 / ❓ row from the README feature tables, with its milestone.

| README feature | Milestone |
|---|---|
| Single / facing page modes | M7 |
| Rotate view | M7 |
| Split view | M7 |
| Attachments panel | M7 |
| Open PDF Portfolios (read-only) | M7 |
| Layers (OCG) panel | M7 |
| Combine Files (PDFs + images, page subsets) | M8 |
| Split document (range, bookmark, size) | M8 |
| Insert blank page / from image | M8 |
| Insert from scanner | M5 |
| Crop pages | M8 |
| Page labels | M7 (display), M8 (edit) |
| Headers / footers / Bates numbering | M8 |
| Watermarks / backgrounds | M8 |
| Colorize bookmarks by level | M8 |
| Bookmark from page range | M8 |
| Target sheets / auto-bookmark from scan separators | M5 (separator sheets) |
| Underline / strikethrough | M9 |
| Polygon | M9 |
| Custom stamps | M9 |
| Comment status / replies | M9 |
| Measurement tools | M9 |
| Edit text in place | M13 |
| Edit / replace / move images | M13 |
| Add text / image objects | M13 |
| Link editing | M13 |
| Redaction | M11 |
| Header/footer editing | M8 (update/remove), M13 (edit) |
| Fill AcroForms | M10 |
| Create / edit form fields | M10 |
| Export pages to PNG / JPEG | M7 |
| Export to multi-page TIFF (PDFlyer) | M7 |
| Create PDF from images | M5 ✅ |
| OCR | M5 ✅ |
| Export text | M7 |
| Print | M7 |
| PDF/A conversion ❓ | M14 |
| Set / remove passwords, permissions; preserve encryption | M11 |
| Digital signatures | M12 |
| Certificate encryption ❓ | M11 |
| Compare documents | M14 |
| Optimize / reduce file size | M14 |
| Preflight / accessibility checker ❓ | M14 |
| Batch actions / scripting | M15 |

## Not planned

- **XFA forms.** Dynamic XFA is deprecated and effectively Adobe-only.
- **Office ↔ PDF conversion.** Use LibreOffice.
- **Creating PDF Portfolios.** Received portfolios can be opened (M7);
  building them is a niche, de-emphasized Acrobat feature. Use Combine Files
  (M8) instead.
- **Cloud features**: shared review, Send for Signature, Document Cloud
  storage, Studio Sessions-style collaboration.
- **3D, rich media, audio/video annotations.**
- **Mobile.**
- **CCH Workpaper Manager / Engagement integration** in core. This is a good
  fit for a plugin (M6).
- **General-purpose document JavaScript.** Only the form-scripting subset in
  M10; everything else goes through plugins.
