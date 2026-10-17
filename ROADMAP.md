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
New proposals not yet placed in a milestone go under [Pending](#pending).

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
| M9 | **Markup completion**: mark editing, remaining annotation types, custom stamps, replies and status, PDFlyer stamps/connectors/tape, FDF/XFDF, measurement | Review workflow parity | v0.5 |
| M10 | **Forms**: fill, calculate, create, import/export, flatten | Tax forms are filled and calculated in sheaf | v0.6 |
| M10.5 | **The edit API**: page content in the model, one op vocabulary over one primitive, saves as outputs, one emitter for screen and file | M11–M15 edit content without reworking the core | — |
| M11 | **Security & redaction**: true redaction, sanitize, passwords and permissions | Client data can be sent out safely | v0.7 |
| M12 | **Signatures**: Fill & Sign, digital signatures, verification, timestamps | Engagement letters and 8879s are signed in-app | v0.8 |
| M13 | **Content editing**: text, images, links, objects | The hardest Acrobat feature | v0.9 |
| M14 | **Document processing**: compare, optimize, PDF/A, accessibility | Archival and output parity | v0.10 |
| M15 | **Automation → v1.0**: batch actions, CLI, parity audit | Acrobat Pro + PDFlyer parity | v1.0 |

M6 has no release of its own; it ships in v0.2 together with Scan & OCR.
M10.5 has none either; it sits between M10 slices 5 and 6, and ships in
v0.6 with the rest of M10.

Each milestone is broken into slices when it starts, as M1–M4 were. Slices
below are a first cut.

---

### M5: Scan & OCR

- ✅ OCR to an invisible, searchable text layer, per page or over a whole
  binder, in the background: `sheaf-ocr` with an `OcrEngine` trait and two
  engines, Tesseract 5 (static) and PaddleOCR PP-OCRv5 on ONNX Runtime
  (`oar-ocr`); Document > Recognize Text (OCR)…; one undo step; Find and
  Patterns search unsaved layers. Design in [`todos/ocr.md`](todos/ocr.md).
- ✅ Scan cleanup (`sheaf_image::clean`): deskew, despeckle, auto-rotate by detected
  text orientation (word shape, plus PaddleOCR's orientation model when
  present), black-and-white by Sauvola thresholding. Blank-page removal
  (M8, `sheaf_ops::blank` over `sheaf_image::blank`): "Leave out blank
  pages" when scanning, Document > Find Blank Pages… with a review before
  deleting, and `sheaf remove-blanks`.
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
  "Target Sheets". Includes inserting separator sheets into an existing
  PDF above chosen pages, so it can be re-run (PDFlyer's Insert Target
  Sheets; moved here from Pending 2026-10-14).

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

- ✅ **Combine Files** (Acrobat's File → Create → Combine Files): build a new
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
- ✅ Split document by page range, by top-level bookmark, or by file size.
- ✅ Insert blank page; insert pages from images (shares M5 image import).
- ✅ Replace pages (Acrobat's Replace Pages).
- ✅ Crop pages (CropBox), with the option to apply to a range.
- ✅ Page labels: edit numbering style, prefix, start.
- ✅ Headers / footers: add, update, remove (with date, page number, file name).
- ✅ Bates numbering.
- ✅ Watermarks and backgrounds (text or image, opacity, behind/in front).
- ✅ Bookmarks: create from a page range; colorize by level; auto-bookmark from
  a text pattern (reuses the Patterns engine).

Slices: Combine Files ✅ → split ✅ → insert blank/images + replace ✅ → crop ✅ →
page labels ✅ → headers/footers + Bates ✅ → watermark/background ✅ → bookmark
extras ✅. Done 2026-10-14; notes in [`todos/m8.md`](todos/m8.md).

Headers/footers, Bates and watermarks are written as tagged artifacts the way
Acrobat writes them, so Acrobat can update or remove them later and so can we.

### M9: Markup completion

Started 2026-10-14; the plan, with the decisions made at kickoff and the
bugs found while planning, is in [`todos/m9.md`](todos/m9.md).

- Infrastructure first (decided 2026-10-14):
  - **Single instance:** one sheaf process per user on each machine, and
    a second launch hands its files to it. Today two launches can open the
    same PDF in two processes, and each save discards the other's edits.
  - **Preferences split from state:** `settings.toml` (preferences, edited
    by hand, only read by sheaf) and `state.json` (window layout and
    last-used choices; disposable). Today every window saves its own copy
    of `settings.json` over the others'.
  - **Edit history:** every version keeps the commands that made it, in a
    tree so undone work isn't lost, and a History panel jumps to any
    version. It's also the base for merging conflicting edits later.
- Editing placed marks: resize and vertex handles, color, fill, opacity,
  line width and style, multi-select, copy and paste.
- Text selection and Copy, which text markup then acts on.
- Underline, strikethrough, squiggly, replace-text / insert-text marks.
- Polygon, polyline, cloud, callout (free text with leader).
- Custom stamps: from image or PDF page, in a shareable library (PDFlyer's
  user stamp export/import), with Recent and Favorites; dynamic stamps
  (date, time, name, initials) from a new identity entry in
  `settings.toml`.
- **(PDFlyer)** Set Status: bookmark color, page border and status stamp,
  and Reset Status. With dynamic stamps this replaces the planned
  "sign-off / review stamps", which PDFlyer doesn't have (real sign-off
  lives in ProSystem fx Engagement's binder).
- **(PDFlyer)** Connector groups: every connector with a label is one
  group, with Previous/Next and "pages with / without" navigation. Pairs
  keep working as groups of two.
- Comment status (accepted/rejected/cancelled/completed), checkmarks,
  replies, on third-party comments too; filters by type, author, status,
  color and page, sorting, hiding. PDFlyer's Stamp Summary and Note
  Summary are filter presets.
- Import / export comments as FDF and XFDF, or from another PDF; editing
  a third-party comment makes it a sheaf mark.
- Comment summary report (to PDF), and the list as CSV or text.
- File-attachment annotations.
- **(PDFlyer)** Calculator tape: sections and row numbers (`1-0`, `1-1`…,
  `1-T`, `GT`), a note column, editing a placed tape in place, rotating
  and resizing it.
- Calculator tape that picks up numbers by clicking page text. A sheaf
  extension: PDFlyer's tape numbers are typed.
- Measurement: distance, perimeter, area, with page scale and calibration.
  The cut line: it can move later without holding up v0.5.

Slices: single instance ✅ → preferences and state ✅ → groundwork and
bug fixes ✅ → edit history ✅ → editing marks ✅ → text selection ✅ → text
markup ✅ →
replies and status ✅ → comment filters ✅ → stamps ✅ → connector groups ✅ → Set
Status ✅ → tape format ✅ → tape from page text ✅ → shapes ✅ → file attachments ✅ →
summary and CSV ✅ → FDF/XFDF ✅ → measurement ✅ → plugin API 0.3 and docs ✅ →
deleting means deleted ✅ (added 2026-10-14 from M11 planning: deleted pages
stayed in saved files; it must land before v0.5). Done 2026-10-15 (Linux);
notes in [`todos/m9.md`](todos/m9.md).

### M10: Forms

Started 2026-10-15, before M11 after all. The plan, with the decisions
made while planning and at kickoff and the bugs found, is in
[`todos/m10.md`](todos/m10.md).

Paused after slice 5 (2026-10-15) for M10.5, the edit API; slices 6+
resume on it. Done so far: the corpus and form check, the field model,
and forms surviving page tools (crop, watermark, header/footer, delete,
extract, insert, replace, combine, duplicate), with the save rules for
hybrid XFA, usage rights and signed files.

- Fill AcroForms: text, checkbox, radio, combo/list, with correct appearance
  stream generation so values look the same in every viewer.
- Field calculations and format/validate/keystroke scripts. Run a
  sandboxed subset of Acrobat form JavaScript (AFNumber_Format,
  AFSimple_Calculate and friends, plus custom scripts), not a general JS
  runtime: pdf.js's form-script library in boa_engine, run as a
  WebAssembly guest in sheaf's wasmtime.
- Create / edit fields: all field types, properties, tab order, alignment
  tools.
- Automatic field detection on flat forms.
- Import / export form data: FDF, XFDF, CSV, tab-delimited text, JSON.
  Acrobat's XML later.
- Flatten form fields (Export Flattened Copy already keeps fields; add the
  option to flatten them).
- XFA: still not planned. Show a clear notice when a document is XFA-only.
- Forms survive page tools, and saves follow rules for hybrid XFA, usage
  rights and signed files.

Slices: corpus and form check → field model → page tools → appearances →
saving → filling → flatten → data import/export → AF functions → script
host → scripts → field authoring → auto-detect (the cut line) → plugin API.

### M10.5: The edit API

Planned 2026-10-15, between M10 slices 5 and 6: M10 paused so that its
remaining slices are built on this API rather than revisited after it.
The plan, with the decisions made while planning, the proposals for
kickoff and the bugs found, is in
[`todos/m10.5-edit-api.md`](todos/m10.5-edit-api.md).

M11–M15 change PDF content far more surgically than anything so far, and
today three separate mechanisms change what a page shows (overlays drawn
at save, derived source files, export-only transforms), none of which
the model sees as content. M10.5 replaces them with one design:

- `DocState` is the document's definition, page content included, and
  one emitter derives every PDF object from it, on screen and on disk.
  The derived-source file cache goes.
- About 25 public semantic ops (a new `sheaf-edit` crate, serde and
  schemars) lower to one primitive, `Put { key, before, after }`.
  History stores those with their before-values, so replay is exact and
  checked.
- Effects are a pure function of each change, and drive dirty state,
  removal detection, signature impact and the save plan.
- Saves are outputs, never rebases: a planner picks Append or Rewrite
  before a byte is written, deleting or replacing really removes, and
  encryption is kept on every save.
- One frame below the page (unrotated user space), lazily read
  content-addressed sources, and an in-memory realizer for edited pages.
- Every existing feature moves onto it (crop, watermark, header/footer
  and Bates, OCR, flatten, the exports, Combine), plus `sheaf edit` JSON
  and plugin API 0.4 as the only world.

Slices: history stores what changed → spikes and gates → lazy sources and
the Store → one open → PageSpace → content toolkit and fonts → the
serializer → records → Rewrite from records → Append and the planner →
ops and Tx → one funnel → realizer → text service → crop → watermark →
header/footer and Bates → OCR → exports and `sheaf edit` → plugin API
0.4. M10 resumes after slice 20.

### M11: Security & redaction

Planned 2026-10-14; follows M10 (M10 was first skipped, then started
2026-10-15 ahead of it).
The plan, with the decisions made while planning, the proposals for
kickoff and the bugs found, is in [`todos/m11.md`](todos/m11.md).

- Redaction: mark areas, text or whole pages as standard `/Redact`
  annotations, then export a copy with **true content removal** (text, OCR
  text, images, line art and annotations under the marks), optional fill
  and overlay text, a leak check and verification. Guard rails keep the
  unredacted binder from being sent by mistake.
- Pattern-driven redaction: Patterns panel hits (OCR text included) become
  redaction marks (the shared engine planned since M2), covering whole words
  or chosen characters (Acrobat's partial-word marks).
- Sanitize / remove hidden information: metadata, attachments, comments,
  form fields, hidden layers, content outside the crop box, links, actions
  and JavaScript, search indexes, thumbnails, sheaf's private data,
  unreferenced objects. Export Clean Copy becomes one preset.
- Passwords and permissions: set, change, remove; AES-256. Permissions are
  shown and kept, not enforced.
- Preserve encryption on save (today a save writes a plain file), with no
  plaintext copies on disk.
- Certificate encryption: moved to after M12 (decided 2026-10-14), which
  builds the certificate stores it needs.

Slices: encryption kept → Security dialog → content interpreter →
redaction marks → apply: text and OCR → apply: images, line art,
annotations → Export Redacted Copy → search and redact → remove hidden
information → plugin API and docs.

Deleted pages that stayed in saved files, found while planning, are fixed
in M9 (deleting means deleted) rather than here.

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
earlier revisions. Removing content forces a full rewrite (M9), so on a
signed file a save after a removal has to ask first.

Follow-up: certificate encryption ❓ (`/Adobe.PubSec`; moved from M11
2026-10-14). It needs the certificate stores built here, and sheaf has
to decrypt these files itself because PDFium only opens password
encryption.

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
  operations live in `sheaf-ops` (with the `in-progress` crate for progress
  and cancellation), and `sheaf-cli` holds the subcommands. They're reachable
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
| Combine Files (PDFs + images, page subsets) | M8 ✅ |
| Split document (range, bookmark, size) | M8 ✅ |
| Insert blank page / from image | M8 ✅ |
| Insert from scanner | M5 |
| Crop pages | M8 ✅ |
| Page labels | M7 (display), M8 (edit) ✅ |
| Headers / footers / Bates numbering | M8 ✅ |
| Watermarks / backgrounds | M8 ✅ |
| Colorize bookmarks by level | M8 ✅ |
| Bookmark from page range | M8 ✅ |
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
| Certificate encryption ❓ | M12 follow-up |
| Compare documents | M14 |
| Optimize / reduce file size | M14 |
| Preflight / accessibility checker ❓ | M14 |
| Batch actions / scripting | M15 |

## Pending

Features we probably want but haven't placed yet. Some are provisional,
some aspirational. **Every new feature proposal starts here.**

When adding an entry:
1. Check the milestones, the parity checklist and Not planned for overlap.
   Note any partial overlap or clear duplicate in the entry. A clear
   duplicate goes into the existing item, not here.
2. Link the research behind it (`docs/prior-art/`, a `todos/` memo).

When reviewing, each entry ends up in one of three places:
- folded into an existing milestone, which usually means a new slice;
- a new milestone, if several pending entries share a theme;
- Not planned, with a reason.

Entries leave this section when they're placed.

### Export to Markdown

Export a document, or a page range, as Markdown: headings, paragraphs,
lists and tables in reading order. This makes binder content usable in
notes, wikis and LLM tools. Bookmarks could supply the heading structure.

- **Overlap:**
  - M7 Export text (plain done; "with layout ❓" left open). This would be
    another format in that same export dialog and `sheaf export`.
  - M14 accessibility (tags tree, reading order). Tagged PDFs give the
    structure directly, and both need a reading-order model.
  - M5 OCR. Scans need a text layer first.
- **Approach:** born-digital pages use PDFium's text plus heuristics
  (font size for headings, x-alignment for columns and tables). Scans can
  use the structure pipeline in `oar-ocr`, which `sheaf-ocr` already
  depends on: layout detection and table recognition, with
  `to_markdown()`. See
  [prior-art/vision-models.md §2](docs/prior-art/vision-models.md#2-page-parsing-layout-tables-markdown).
- **Open:** which milestone. It fits as an M7 follow-up slice (text-only
  heuristics) or M14 (once reading order exists).

### ML document restoration (DocRes)

Opt-in model-based cleanup for photos and degraded scans: deblurring,
better binarization of faded copies, and shadow and appearance correction
where background division leaves artifacts. One 15 M-parameter model
covers all of these.
- **Overlap:** M5 scan cleanup and camera capture (done). This extends
  them; UVDoc already covers dewarping.
- **Research:** [prior-art/vision-models.md §1](docs/prior-art/vision-models.md#1-docres).
- **Open:**
  - CPU time per page;
  - whether it improves OCR accuracy, not just looks;
  - the license of the weights;
  - we'd have to make the ONNX export ourselves.

  If it's worth it, it's an M5 follow-up slice beside UVDoc.

### PDFlyer parity gaps

PDFlyer tools that sheaf lacks or does differently, from
[prior-art/pdflyer.md §6](docs/prior-art/pdflyer.md#6-corrections-and-implications-for-sheafs-readme-and-roadmap)
and [prior-art/README.md](docs/prior-art/README.md#what-the-prior-art-says-about-useful)
(researched 2026-10-13). Multi-page TIFF with marks rendered in is left
out: M7's TIFF export already renders sheaf's marks.

**Placed 2026-10-14** (M9 planning):
- Into M9: connector groups; Set Status (replacing sign-off stamps); Stamp
  Summary and Note Summary; Recent and Favorite stamps, sticky stamp mode,
  user stamps shared across staff; the tape's section/row format, editing
  a placed tape, rotating and resizing it.
- Into M5: Insert Target Sheets, as part of separator sheets.

What's left is bookmark and page work. Each row was checked against sheaf
on 2026-10-14:

| Gap | sheaf today | Size |
|---|---|---|
| Split Bookmark: split a multi-page bookmark into same-level bookmarks, moving pages between them | Nothing. Split Document splits files. The model already has the edits it needs and the bookmark page-span helper (`DocState::bookmark_pages`). | Medium: a new dialog |
| Delete Bookmark *with its pages* | Delete removes the bookmark and its children; the pages stay. Shift+drag moves a bookmark with its pages. | Small: an option, one undo step (delete the bookmark first, then the pages) |
| Delete Page re-attaches its bookmark to the next page | Every bookmark pointing at a deleted page is removed and its children move up a level; nothing is re-pointed. The test `delete_cascades_and_promotes_children` asserts this, so it changes too. | Small: in `cascade_delete`, point the bookmark at its first surviving page |
| Hide/Show empty bookmarks; colorize by populated vs empty | No hide. The panel dims bookmarks with no destination, which isn't the same as no pages under them. Color Bookmarks by Level colors by depth. | Small to medium. "Empty" is undefined in PDFlyer's help; probably "neither it nor anything under it points at a page". |
| Insert PDF modes: merge by parent bookmarks / append as one bookmark at a chosen level / one bookmark per page / none; offer to delete the source | No dialog. From File… opens a picker and always merges: matching top-level titles take the new children, but the pages aren't moved under them. | Medium: a dialog; modes 2 and 3 are existing edits, mode 1 with real repagination is new |
| **Read existing PDFlyer workpapers** (stamps, connectors, tapes) | No sample. Third-party marks are read-only records that keep only page, subtype, author, text, date and rect. | Large; blocked on a PDFlyer-marked sample. Highest value. M9 slice 3 reads bookmark colors, a prerequisite. |

**Open:**
- These rows fit one short "PDFlyer binder" milestone better than M9.
- The doc corrections from the same §6 were applied to README.md and
  this file on 2026-10-14.

### Follow links in the page view

Clicking a link annotation does nothing today. That covers internal
destinations, URLs (which should get a warning before opening, like the
attachments panel's) and links in files from other tools. Connectors jump
only through sheaf's own code. Found while planning M9.
- **Overlap:** M13 link editing (create, edit, delete) builds on this.
  M9 connector groups write `/Dest` cycles that Acrobat follows; sheaf
  doesn't follow them.
- **Open:** small enough to ride along an M9 slice (connector groups), or
  an M7 follow-up.

### File contention: more than one process editing a PDF

**A known bug, deferred (2026-10-14).** Suppose another process changes a
PDF while sheaf has it open: a colleague's sheaf on a file server,
Acrobat, or a sync client. sheaf's next save then silently overwrites or
mangles those changes, and the recovery file beside the PDF is offered to
the wrong person. It only happens when more than one process edits the
same file. The bug, and the design so far (detect changes before saving,
a lock file, merge later), are in
[`todos/file-contention.md`](todos/file-contention.md).
- **Overlap:** M9 slice 1 (single instance) removes one user's two sheaf
  processes. Slice 2 creates the local state folder the recovery files
  would move to. Slice 3 makes mark ids unique. Slice 4 keeps every
  version's commands back to the last save, which a merge would replay.
  Slice 18's comment import is a manual merge path.

### Agent panel (Claude and Codex)

A chat panel where the user works with an AI agent on the open document,
like Zed's external agents. sheaf launches the agent (Claude Code or
Codex), and the agent handles sign-in with the user's own subscription;
sheaf takes no API keys and never sees a credential. The agent sees the
document, its edit history, and every command with its help text, plugin
commands included. It runs commands and edits live, with the user's
approval. Each approved call is one version in the history, and a whole
turn can be reverted. Decisions and research (2026-10-14) are in
[`todos/agent.md`](todos/agent.md).
- **Placement:** its own milestone, one of the last.
- **Overlap:**
  - M6 plugin host API. The agent's read tools are the WIT `document`,
    `text` and `render` interfaces, and its edits are the same `Edit`
    batches.
  - M9 slice 4 edit history (done): versions, edits in words, jumps.
  - M9 slice 1: `sheaf mcp` reaches the running sheaf through the
    single-instance socket.
  - M15 batch actions. Both need commands that take arguments instead of
    a dialog. Until a command has them, the agent opens its dialog
    pre-filled, so this needn't wait for M15.
  - `todos/plugin-queue.md` skipped "Ask Claude" as a plugin (2026-10-13)
    because AI belongs in core; this is that feature.
  - Foxit's AI Assistant runs commands from chat
    ([prior-art/foxit.md §3.13](docs/prior-art/foxit.md#313-ai-assistant-ch-15-subscription-only)).
- **Open:**
  - Whether Anthropic allows subscription sign-in through the
    `claude-agent-acp` adapter. If not, Claude runs through Claude Code's
    own headless mode instead of ACP.
  - Limiting each agent to sheaf's tools: both come with shell and file
    tools of their own.
  - Client data leaving the machine: IRC §7216 consent and a firm-wide
    off switch in `settings.toml`.

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
