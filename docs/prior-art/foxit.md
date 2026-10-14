# Prior art: Foxit PDF Editor (Pro), desktop, 2025–2026

Researched 2026-10-13. Primary sources: the Foxit PDF Editor for Windows **User Manual for 2026.2** ([PDF, 514 pp, built 2026-08-26](https://cdn01.foxitsoftware.com/pub/foxit/manual/phantom/en_us/foxit-pdf-editor-user-manual-2026.2.pdf)), Foxit's [Windows keyboard shortcuts page](https://resource-cms.foxit.com/foxit-pdf-editor-for-windows/keyboard-shortcuts), [Windows single-key accelerators page](https://resource-cms.foxit.com/foxit-pdf-editor-for-windows/single-key-accelerators) (both dated 15 Dec 2025), the [Mac v14 shortcuts page](https://help.foxit.com/manuals/pdf-editor/mac/en-us/v14/Keyboard_Shortcuts_and_Single_key_Accelerators.html), and Foxit's [version history](https://www.foxit.com/pdf-editor/version-history.php). For the Acrobat comparison I used Adobe's [Acrobat keyboard shortcuts page](https://helpx.adobe.com/acrobat/using/keyboard-shortcuts.html) (updated 23 Sep 2025).

---

## 1. Overview

### Product line

| Product | What it is | Price (Foxit list, Oct 2026) |
|---|---|---|
| **Foxit PDF Reader** | Free viewer, annotator and form filler. It now includes some AI Assistant features. | Free |
| **Foxit PDF Editor** (subscription) | Full editor for Windows, macOS and web: edit, convert, forms, OCR, compare, password protection, Foxit DMS, and **300 AI credits/month**. | **$129.99/user/yr**, or from $10.99/mo ([pricing](https://www.foxit.com/pdf-editor/pricing/)) |
| **Foxit PDF Editor+** (subscription) | Editor plus the iOS and Android apps, **AI Smart Redact** (PII), and Foxit eSign with 150 envelopes/yr plus request-and-track. | **$159.99/user/yr** ([pricing](https://www.foxit.com/pdf-editor/pricing/)); $13.99/mo per [PCWorld](https://www.pcworld.com/article/2163281/foxit-pdf-editor-review.html) |
| **Foxit PDF Editor 14** (perpetual) | One-time licence for that major version. Third parties report that it excludes AI, Smart Redact, DMS and cloud/mobile features (unverified against Foxit). | **$209.99 one-time** ([pricing](https://www.foxit.com/pdf-editor/pricing/)) |
| **"Pro"** edition | The manual still names two editions, "Foxit PDF Editor" for SMBs and "**Foxit PDF Editor Pro**" for enterprises, and labels some features **(Pro Only)**: Action Wizard, Bates numbering, Redaction, MIP Protect, Certify, Create accessible PDFs (autotag/Make Accessible), Tickmarks, Accounting Calculator, add new image/shape objects, Reflow Editing, Image Editor, layer import/merge/flatten, Shared/Email review initiation, PDF/A–X–E conversion, and grayscale conversion ([manual §1, TOC](https://cdn01.foxitsoftware.com/pub/foxit/manual/phantom/en_us/foxit-pdf-editor-user-manual-2026.2.pdf)). The pricing page no longer sells a SKU called "Pro". Whether the subscription tiers include all Pro-only features is unverified. One aggregator puts Bates, redaction and PDF/A in Editor+ ([inventivehq](https://inventivehq.com/software/foxit/compare), unverified). | Volume licensing at 30+ seats, up to 30% off |

Foxit's marketing claims Foxit is "up to 44% less expensive than Adobe Acrobat" and that "users complete similar tasks 7% faster" ([Foxit vs Adobe one-pager](https://cdn01.foxitsoftware.com/pub/foxit/datasheet/general/en_us/foxit-vs-adobe-one-pager.pdf), a vendor claim with no published method).

### Versioning cadence (Editor desktop)

Since 2023, Foxit has used calendar versions `YYYY.N.patch.build`, with about 3 feature releases a year ([version history](https://www.foxit.com/pdf-editor/version-history.php)).

| Version | Date | Headline items |
|---|---|---|
| 2025.1.0 | 22 Apr 2025 | New dedicated **AI ribbon tab**, chat with images, multi-document analysis, automatic bookmarking ([press](https://www.foxit.com/company/press/10781.html)) |
| 2025.2.0 | 13 Aug 2025 | AI Assistant becomes an "**MCP Host**" (Gmail, Salesforce and Jira actions from the PDF), **Read Summary**, and **Smart Podcast** ([press](https://www.foxit.com/company/press/11087.html)). Also **native 64-bit Windows installers** and OCR gains for digits, checkboxes and handwriting ([PDF Association](https://pdfa.org/foxit-20252-and-v14-release-highlights-faster-smarter-more-secure/)) |
| 2025.3.0 | 19 Dec 2025 | AI responses export to Word/PDF, more translation languages, **Arc** drawing tool, collaboration consent prompts ([press](https://www.foxit.com/company/press/11200.html)) |
| 2026.1.0 | 31 Mar 2026 | **Action Inspector**, which finds malicious JavaScript and self-modifying behaviour. **Overprint Preview**, an AI "Chats" tab on the Start page, and hide-cursor while drawing with the pencil on tablets ([PDF Association](https://pdfa.org/foxit-pdf-editor-release-20261036452-new-features-and-improvements/), via search snippet) |
| 2026.1.2 | 8 Jul 2026 | Security release fixing 28 vulnerabilities ([deskmodder](https://www.deskmodder.de/blog/2026/07/08/foxit-pdf-reader-und-editor-2026-1-2-36540-korrigiert-28-sicherheitsluecken/)) |
| 2026.2.0 / .1 | 1 Sep / 23 Sep 2026 | **Foxit Workspace**, a project-based multi-document AI hub. **Local AI** with small on-device models, **bring-your-own-model**, Purview upgrades, AEC tooling, Arabic ([press](https://www.foxit.com/company/press/12241.html), [PDF Association](https://pdfa.org/?p=236889)) |

### Engine: Foxit PDF SDK, and PDFium's origin (verified)

- Foxit ships its own C++ **Foxit PDF SDK**, which it also sells for Windows, macOS, Linux, mobile and web. The 2026.2 manual credits **Anti-Grain Geometry 2.3** (Maxim Shemanarev) and "Portions … Copyright [2001-2005] Solid Documents" ([manual p.2](https://cdn01.foxitsoftware.com/pub/foxit/manual/phantom/en_us/foxit-pdf-editor-user-manual-2026.2.pdf)). AGG is also the default rasterizer in PDFium, which fits the shared lineage.
- **PDFium is derived from Foxit's engine. This is verified.** On 10 June 2014, Foxit announced that its PDF technology had been "chosen as the open-source rendering engine" for Google's **PDFium** project, would serve as the initial code base, and that Foxit would keep contributing ([Foxit press release 1614](https://www.foxit.com/company/press/1614.html); [PDF Association repost](https://pdfa.org/foxit-pdf-technology-chosen-for-google-open-source/)). PCWorld (12 Jun 2014) reported that Google open-sourced Chrome's previously closed PDF library under **BSD-3-Clause**. Chrome engineer Peter Kasting called Foxit's component "almost certainly the highest-quality PDF engine available in the open-source world" ([PCWorld](https://www.pcworld.com/article/439783/google-open-sources-pdf-software-library.html); [InfoQ](https://www.infoq.com/news/2014/06/google-chrome-pdf-engine-free)).
- **Implication for sheaf:** anything built on pdfium-render or PDFium bindings is running a 2014 fork of Foxit's renderer. The commercial Foxit SDK has diverged a lot since then, adding editing, OCR, redaction and compare.

### Platforms

| Platform | Notes |
|---|---|
| Windows 10/11 | x86 and **ARM** (Snapdragon SQ1+). Minimum 512 MB RAM (1 GB recommended), **7.5 GB disk**, 4K/HiDPI support, Citrix Ready ([system requirements](https://www.foxit.com/resource-hub/user-guide/foxit-pdf-editor-for-windows/system-requirements/)). Native 64-bit since 2025.2. |
| macOS | Separate codebase/manual with feature lag ([Mac v14 manual](https://help.foxit.com/manuals/pdf-editor/mac/en-us/v14/Keyboard_Shortcuts_and_Single_key_Accelerators.html)). Also uses a ribbon. |
| Web, iOS, Android | Web is in Editor. Mobile apps are Editor+ only. |
| Linux | None for the Editor. The SDK supports Linux. |

---

## 2. Keyboard shortcuts

Shortcuts can be rebound under *right-click ribbon › Customize Quick Access Toolbar › **Keyboard** tab*. You pick a category and command, press the new key, then **Assign**. Keys already in use cannot be reassigned, and the dialog says which command owns them. There is also a **Reset All** button. On Mac, the full live list is under **Help › Keyboard Mapping**.

**How the groups below are built.** Foxit's Windows page groups shortcuts **by ribbon tab** (File, Home, Convert, Edit, Organize, Comment, View, Form, Share, Accessibility, Help, Format, Others). Its Mac page groups them by *File / View / Edit / Tool / Comment / Organize & Protect / Help keys*. I used the Windows grouping and mapped Mac rows into it. "—" means Foxit does not document that key for the platform.

### 2.1 File tab

| Action | Windows | macOS | vs Acrobat |
|---|---|---|---|
| Open | Ctrl+O | Cmd+O | same |
| Close file | Ctrl+W / Ctrl+F4 | Cmd+W | same |
| Close all | Ctrl+Shift+W | Cmd+Shift+W | **Acrobat Windows has none** (Mac: Cmd+Opt+W) |
| Save | Ctrl+S | Cmd+S | same |
| Save As | Ctrl+Shift+S | Cmd+Shift+S | same |
| Print | Ctrl+P | Cmd+P | same |
| Exit | Ctrl+Q / Alt+F4 | Cmd+Q | same |
| Document Properties | Ctrl+D | Cmd+D | same |
| Preferences | Ctrl+K | Cmd+K | same |
| Index (full-text index) | Ctrl+Shift+M | — | |

### 2.2 Home tab (tools, find, zoom)

| Action | Windows | macOS | vs Acrobat |
|---|---|---|---|
| Hand tool | **Alt+3** | **Cmd+5** | Acrobat uses only the single key H |
| Select Text and Image | **Alt+6** | **Cmd+6** | Acrobat uses only V |
| Select Annotation | — | Ctrl+6 | |
| Snapshot | Alt+7 | — | |
| Marquee Zoom | Alt+5 (View tab) | Opt+5 | |
| Find (title-bar box) | Ctrl+F | Cmd+F | same |
| Find next / previous | F3 / Shift+F3 | F3 / Shift+F3 | Acrobat also uses **Ctrl+G** for find next. Foxit uses Ctrl+G for Go to Page. |
| Search panel (advanced) | Ctrl+Shift+F | — | same as Acrobat |
| Zoom To… | Ctrl+M | Cmd+M | same |
| Zoom in / out | Ctrl++ / Ctrl+− | Cmd+= / Cmd+− | same |
| Actual Size | Ctrl+1 | Cmd+1 | same |
| **Fit Page** | **Ctrl+0** | **Cmd+2** | **Foxit's two platforms disagree.** Acrobat Windows uses Ctrl+0 (unverified) |
| **Fit Width** | **Ctrl+2** | **Cmd+3** | Platforms disagree |
| **Fit Visible** | **Ctrl+3** | **Cmd+4** | Platforms disagree |
| Reflow | Ctrl+4 | — | same as Acrobat (Ctrl+4) |
| Rotate clockwise / counter-clockwise | Ctrl+Shift+Plus / Ctrl+Shift+Minus | Shift+Cmd+= / Shift+Cmd+− | same |

### 2.3 Convert, Edit, Organize tabs

| Action | Windows | macOS | vs Acrobat |
|---|---|---|---|
| Create PDF from File | Ctrl+N | — | |
| Copy | Ctrl+C / Ctrl+Insert | Cmd+C | same |
| Cut / Paste | Ctrl+X / Ctrl+V | Cmd+X / Cmd+V | same |
| Select all / **Deselect all** | Ctrl+A / Ctrl+Shift+A | Cmd+A / — | |
| Add Bookmark | Ctrl+B | Cmd+B | same |
| **Search & Replace** | Ctrl+T | Cmd+T | Acrobat has no documented equivalent |
| **Search & Redact** | — | Cmd+R | |
| **Cross Reference** | Ctrl+Shift+C | — | Foxit only |
| Delete pages | Ctrl+Shift+D | Cmd+Shift+D | same |
| Insert blank page | Ctrl+Shift+T | Cmd+Shift+T | same |
| Insert from file | Ctrl+Shift+I | Cmd+Shift+I | same |
| **Duplicate pages** | Ctrl+Alt+D | — | Foxit only |
| Unrestricted Access (RMS) | — | Cmd+U | |
| RMS templates: connect / custom | — | Cmd+Opt+G / Cmd+Opt+T | |
| Delete previous word (in edit) | Ctrl+Backspace | Cmd+Backspace | |

### 2.4 Comment tab

This is where Foxit departs most from Acrobat. **Default (non-single-key) shortcuts exist for individual markup tools.** On Windows they are mostly **Shift+letter**. On Mac they are **Control+letter**.

| Action | Windows | macOS | vs Acrobat |
|---|---|---|---|
| Note (sticky) | Ctrl+Shift+N | Ctrl+N | Acrobat uses only single-key S. **Acrobat's Ctrl+Shift+N is Go to Page.** |
| Highlight | Shift+H | — | |
| Area Highlight | — | Ctrl+I | |
| Underline | **Ctrl+U** | — | |
| Squiggly underline | Shift+U | — | **Collides with the single-key "cycle highlight tools" (Shift+U) in both Foxit and Acrobat** |
| Strikeout | Shift+S | — | |
| Insert Text | Shift+I | — | |
| Replace Text | Ctrl+Shift+R | — | |
| **Typewriter** | Shift+T | Ctrl+T | Foxit only; Acrobat folded this into "Add text" |
| Textbox | Shift+E | Ctrl+X | |
| Callout | Shift+C | Ctrl+J | |
| Pencil | Shift+P | Ctrl+P | |
| Eraser | — | Ctrl+E | |
| Rectangle / Oval / Line / Arrow | — | Ctrl+R / Ctrl+O / Ctrl+L / Ctrl+A | |
| Polyline / Polygon / Cloud | — | Ctrl+Y / Ctrl+G / Ctrl+C | |
| Pin a File | — | Ctrl+B | |
| Measure Distance / Perimeter / Area | — | Ctrl+Shift+D / P / A | |
| **Keep Tool Selected** (toggle) | — | Ctrl+Z | |
| **Paste in Place** | Ctrl+Alt+V | — | |
| Export all comments | Ctrl+L | Ctrl+Shift+L | **Acrobat Windows Ctrl+L = Full Screen** (unverified, not on Adobe's 2025 page) |
| Export highlighted text | Ctrl+I | Ctrl+Shift+I | |
| Show / hide all comments | Ctrl+8 / Ctrl+Shift+8 | Ctrl+U / Ctrl+H | |
| Open / close all pop-ups | — | Ctrl+K / Ctrl+M | |
| Submit reply in Comments panel | Ctrl+Enter | — | |

### 2.5 View tab

| Action | Windows | macOS | vs Acrobat |
|---|---|---|---|
| Full Screen | **F11** | **Cmd+L** | Acrobat uses Ctrl/Cmd+L (unverified) |
| Read Mode | Ctrl+H | — | same as Acrobat (unverified) |
| **Text Viewer** (plain-text view) | Ctrl+6 | — | Foxit only |
| AutoScroll / stop | Ctrl+Shift+H / Esc | Cmd+Shift+H | same |
| Rulers | Ctrl+R | Cmd+Opt+R | |
| Line Weights | Ctrl+5 | Ctrl+Cmd+5 | |
| Single / Continuous / Facing / Continuous Facing | — | Cmd+7 / 8 / 9 / 0 | |
| Read current page / from current page | Ctrl+Shift+V / Ctrl+Shift+B | — | same as Acrobat |
| Read: Activate / Stop / Pause | Ctrl+Shift+Y / Ctrl+Shift+E / **Ctrl+Shift+P** | — | Acrobat pauses with Ctrl+Shift+C. **Foxit's own single-key page also says Ctrl+Shift+C, so Foxit's documentation is internally inconsistent.** |
| Show/hide navigation pane | F4 | F4 (or Fn+F4) | same |
| **Compare: next / previous change** | **Ctrl+. / Ctrl+,** | — | Foxit only |
| Page down / up | Space / Shift+Space | — | |
| Caret movement (char, word, row, doc) | arrows; Ctrl+←/→; Home/End; Ctrl+Home/End | — | |
| Next / previous page top | Ctrl+PgDn / Ctrl+PgUp | Cmd+↓ / Cmd+↑ | same |
| Go to Page | **Ctrl+G** | **Cmd+G** | **Acrobat uses Ctrl+Shift+N, and its Ctrl/Cmd+G is find next** |
| First / last page | Ctrl+Home / End | Fn+← / Fn+→ | |
| Previous / next view | Alt+← / Alt+→ | Cmd+← / Cmd+→ | |

### 2.6 Form, Share, Accessibility, Help tabs

| Action | Windows | macOS | vs Acrobat |
|---|---|---|---|
| JavaScript Console | Ctrl+J | Cmd+J | same |
| Next / previous field | Tab / Shift+Tab | Tab / Shift+Tab | same |
| **Email all open tabs** | Ctrl+Shift+K | — | Foxit only |
| **Email current tab** | Ctrl+Shift+L | — | Foxit only |
| Reading options | Ctrl+Shift+5 | Shift+Cmd+5 | same |
| Reading Order tool | Ctrl+Shift+U | Shift+Cmd+U | same |
| User manual | F1 | F1 | same |

### 2.7 Format panel and floating toolbar (Windows)

These apply while an annotation is selected. Acrobat has no equivalent.

| Action | Windows |
|---|---|
| Switch color | Shift+L |
| Apply palette color 1…10 | Shift+1 … Shift+9, Shift+0 |
| Edit note | Shift+O |

### 2.8 Others (Windows)

| Action | Windows | vs Acrobat |
|---|---|---|
| **Collapse ribbon** | Ctrl+F11 | n/a (no ribbon) |
| **Command search (title-bar box)** | **Alt+Q** (from the manual's "Search and Find Commands" section) | Acrobat has a tools search, with no documented key |
| Undo / Redo | Ctrl+Z / Ctrl+Shift+Z | |
| Next / previous doc tab | Ctrl+Tab or Ctrl+Shift+PgDn / Ctrl+Shift+Tab or Ctrl+Shift+PgUp | Acrobat: Ctrl+Tab / Ctrl+Shift+Tab |
| Previous / next page (print preview) | PgUp / PgDn | |
| Extend selection (char, word, line, row, doc) | Shift+arrows; Ctrl+Shift+←/→; Shift+Home/End; Shift+↑/↓; Ctrl+Shift+Home/End | |

Ctrl+Tab can also switch tabs in **most-recently-used order** (a preference), and the mouse wheel over the tab bar switches tabs (also a preference).

### 2.9 Single-key accelerators

These are **off by default**. Enable them at *Preferences › General › "Use single-key accelerators to access tools"*. This is the same opt-in, and almost the same table, as Acrobat: **Foxit copied Acrobat's single-key scheme nearly verbatim.**

| Action | Win | Mac | Acrobat |
|---|---|---|---|
| Hand / temporary Hand | H / Space | H / Space | same |
| Select Text & Image | V | V | V (Select) |
| Marquee/Loupe/Magnifier; cycle | Z; Shift+Z | Z; Shift+Z | same (Acrobat cycles Marquee/Dynamic/Loupe) |
| Temporary zoom out / in | Ctrl / Ctrl+Space | Cmd / Cmd+Space | Acrobat Mac: Option / Cmd+Space |
| Select Annotation | R | R | R (Select Object) |
| Edit Object | O | O | same |
| Crop | C | C | same |
| Link | L | L | same |
| Text Field; cycle form tools | F; Shift+F | F; Shift+F | same (Acrobat adds Barcode) |
| Audio & Video; cycle multimedia | M; Shift+M | M; Shift+M | same |
| Edit Text | T | T | same |
| Text & Images Redaction | Shift+Y | Shift+Y | same |
| Enter/exit forms editing | — | — | **A** (Acrobat only) |
| Cycle Touch Up tools | — | — | Shift+T (Acrobat only) |
| Sticky Note | S | S | same |
| Replace Text | E | E | — |
| Stamp | K | K | K or J |
| Current highlight tool; cycle | U; Shift+U | U; Shift+U | same |
| Current drawing tool; cycle | D; Shift+D | D; Shift+D | same |
| Cloud | Q | Q | same |
| Text Box | X (blank on the Windows page, a documentation bug) | X | X |
| Stamp/Attach; cycle | J; Shift+J | J; Shift+J | same (Acrobat adds Sound) |
| Reply to comment / checkmark comment | — | — | R / Shift+K (Acrobat only) |
| Open / close pop-up | Enter / Esc | Enter / Esc | same |
| **Form align** L/R/T/B/H/V; center Shift+H/V | ✓ | ✓ | same |
| Highlight fields / show tab numbers | Shift+L / Shift+N | same | same |
| Document JavaScript | **P** | **P** | **Acrobat: Shift+D, and P = toggle form edit/preview** |
| Toggle guides | — | — | G (Acrobat only) |
| Task pane open/close | Shift+F4 | Shift+F4 | same |
| Focus document pane / ribbon | F5 / F10 | Fn+F5 / — | F5 |
| Context menu open / close | Shift+F10 / F10 | Shift+Fn+F10 / Fn+F10 | |
| Navigation pane focus | Ctrl+Shift+F5 | Cmd+Shift+Fn+F5 | same |
| Cycle focus doc/status/nav | F6 / Shift+F6 | Fn+F6 / Shift+Fn+F6 | same |
| Bookmarks expand / collapse / all | → or * or + / ← or / or − / Shift+* | same | same |
| Previous / next result document | Alt+Shift+← / → | Opt+Shift+← / → | same (Windows) |
| Portfolio list keys | Acrobat-identical set | Acrobat-identical set | same |

**What sheaf can take from this:** Foxit chose Acrobat-compatible **single-key** and **accessibility/Read-Aloud** keys to ease migration, but **diverges on Ctrl-chords**: Go to Page is Ctrl+G, Full Screen is F11, Ctrl+L exports comments, and Ctrl+T is Search & Replace. Its **Windows and Mac maps disagree** on Fit Page/Width/Visible and on the comment-tool modifier (Shift vs Control). Sheaf should choose one canonical map and keep Mac a pure Ctrl→Cmd substitution.

---

## 3. Feature inventory

Section and page references are to the [2026.2 Windows manual](https://cdn01.foxitsoftware.com/pub/foxit/manual/phantom/en_us/foxit-pdf-editor-user-manual-2026.2.pdf). ★ marks something Foxit does **differently or better than Acrobat**.

### 3.1 Viewing and navigation (ch. 3)

| Feature | Notes |
|---|---|
| ★ **Multi-tab and single-document modes** | Tabs are the default. You can **drag a tab out to a new window** and drag it back. Ctrl+Tab can use MRU order, the mouse wheel over the tab bar switches tabs, and middle-click or double-click closes a tab. An *Always open files in new window / new instance* preference gives SDI behaviour (§ Multi-tab Reading & Single Document Reading). |
| ★ **Parallel view / tab groups** | Splits the window horizontally or vertically into tab groups, so different PDFs sit side by side, each independently editable. Acrobat's split view only covers one document. |
| ★ **Text Viewer** | A pure-text reading mode with its own font, colour and column settings, plus find and copy. |
| Read Mode, Full Screen, Reflow, AutoScroll, Rulers/Grid/Guides, Line Weights, Night mode, page transitions | Read Mode keeps a floating mini-toolbar at the bottom edge. |
| 3D content viewing, measuring and commenting | Adding 3D models is Pro-only. |
| ★ **Word Count**, ★ **Index** (Ctrl+Shift+M), full-text search across folders | |
| Skins | Light, Dark, or follow the system. |
| Mouse/Touch mode toggle on the Quick Access Toolbar | Touch mode spaces out ribbon buttons. |
| Start page | Recent files, a "Tool Wizard", AI Chats history, PDF Collaboration, DMS, and tutorials. |

### 3.2 Organize (ch. 6)

| Feature | Notes |
|---|---|
| Insert, rotate, move, swap, delete, extract, split, crop, resize, flatten | |
| ★ **Interleave pages**, ★ **Reverse pages**, ★ **Swap pages**, ★ **Duplicate pages** (Ctrl+Alt+D) | Acrobat has no one-click interleave or reverse (Foxit lists interleaving as a differentiator in its [one-pager](https://cdn01.foxitsoftware.com/pub/foxit/datasheet/general/en_us/foxit-vs-adobe-one-pager.pdf)). |
| ★ **Rearrange pages by bookmark order** | |
| Header/footer, watermark, background | |
| **Bates numbering** (Pro) | 3–15 digits, prefix/suffix, increment (**a negative increment reverses order**), and rotation ([Foxit blog](https://foxit.com/blog/how-to-add-bates-numbering-in-pdfs)). |
| Page-number label format, PDF Binders, Portfolios, TOC from bookmarks | |

### 3.3 Edit (ch. 5)

| Feature | Notes |
|---|---|
| Paragraph editing; ★ **Link & Join text boxes** so text flows between boxes | PCWorld notes editing is box-based ([review](https://www.pcworld.com/article/2163281/foxit-pdf-editor-review.html)). TechRadar praises the linked flow ([review](https://www.techradar.com/reviews/foxit-pdf-editor-12)). |
| ★ **Reflow Editing** (Pro) | Listed by Foxit as a differentiator over Acrobat. |
| Objects | Text, image, shape, shading. ★ **Merge/split text objects**, ★ **convert text to shapes**, ★ **shear and clip objects** (Pro). |
| ★ **Built-in Image Editor** (Pro) | Includes a clone stamp, so raster edits happen without leaving the app. |
| ★ **Search & Replace** (Ctrl+T) across the document | |
| Spell check, links, bookmarks, ★ **bookmarks from text styles or search strings** (Pro), destinations, articles, attachments | |
| Layers | Import, merge and flatten are Pro. |
| PDF Optimizer | ★ "High compression" (MRC) is Pro. |
| ★ **Cross Reference** (Ctrl+Shift+C) | (unverified scope) |
| Edit scanned text in place | Optional auto-OCR when the Edit Text tool hits an image page, using a substitute system font ("Make all pages editable"). |

### 3.4 Comment (ch. 7)

| Feature | Notes |
|---|---|
| ★ **Typewriter** as a first-class tool, alongside Callout and Textbox | Inline free text with bullets and lists in the right-hand Format panel. Acrobat merged its typewriter into "Add text". |
| Text markup, notes, pins/file attachments, drawing tools, measure tools (distance, perimeter, area; calibrated) | |
| Stamps | Image, dynamic, and ★ **sequential stamps** (auto-increment number or letter, or manual entry per stamp). Also favourites and admin-distributed stamps. |
| ★ **Tickmarks** (Pro) | Symbol, number and letter tickmarks with auto-increment (custom number up to 999). You can jump between tickmarks of the same style and colour. This targets audit workpapers. |
| ★ **Accounting Calculator** (Pro) | A calculator whose **tape is placed on the page as an annotation**. You can edit tape lines and results recalculate. |
| ★ **Keep Tool Selected** toggle and **Paste in Place** | |
| ★ **Format palette hotkeys** (Shift+1…0) | |
| ★ **Summarize Comments** | PDF summary with **5 layouts**: connector lines on separate or same page, comments only, and sequence numbers on separate or same page. Also **CSV summary** with selectable columns (subscription). |
| Export comments, export highlighted text, import, FDF/XFDF, shared and email review (initiation is Pro), real-time link sharing (subscription) | |

### 3.5 Forms (ch. 9)

| Feature | Notes |
|---|---|
| AcroForm and **XFA** fill (both editions) | |
| Field authoring for text, check, radio, list, combo, button, signature, image, date | Auto-detection of fields (unverified exact name). |
| Data import/export, JavaScript console and document JS | |
| ★ **Comment on forms** and non-interactive form fill (Typewriter) | |

### 3.6 Protect and redact (ch. 10)

| Feature | Notes |
|---|---|
| Password and certificate encryption, ★ **MIP/RMS Protect** (Pro, including usage logging of RMS documents), ★ **Windows Information Protection**, FIPS mode, trust manager | |
| Redaction (Pro): mark text/images/pages, **place on multiple pages**, ★ **Whiteout** (one-click removal plus white fill) | |
| Search & Redact | Includes patterns such as phone and credit-card numbers, and ★ searching a **whole folder**. |
| ★ **AI Smart Redact** (Editor+) | AI detection of PII. |
| Sanitize / remove hidden data | |
| ★ **Action Inspector** (2026.1) | Finds malicious JS and self-modifying behaviours. |

### 3.7 OCR and create (ch. 4)

| Feature | Notes |
|---|---|
| Auto-detects scanned PDFs and offers OCR. Outputs: **Searchable Image**, Searchable Image (original image), or **Editable Text** | |
| Multi-language, downsample-on-OCR, **OCR Suspects** review dialog | |
| ★ **Recognize a selected area** (Pro) | |
| OCR engine shipped as a **separately downloadable plug-in** | Keeps the base install smaller. |
| Create from file, scanner, clipboard, web page, blank, Office (Visio and Project are Pro), AutoCAD | |

### 3.8 Convert

| Feature | Notes |
|---|---|
| Export to Word, Excel, PPT, images, HTML, text, RTF | |
| PDF/A, PDF/X, PDF/E conversion and validation (Pro) | ★ Grayscale conversion (Pro). |
| Weak spot: "Convert from Webpage can be glitchy" ([TechRadar](https://www.techradar.com/reviews/foxit-pdf-editor-12)) | |

### 3.9 Sign (ch. 11)

| Feature | Notes |
|---|---|
| Quick PDF Sign (ink/image), digital signatures (PAdES/LTV), ★ **Certify** (Pro), time-stamp documents | |
| ★ **Foxit eSign** built in (request and track; 150 envelopes/yr in Editor+), plus a **DocuSign** integration | |

### 3.10 Batch automation: Action Wizard (Pro, ch. 5)

| Feature | Notes |
|---|---|
| Create an action from an ordered task list with per-task **Specify Settings** or **Prompt User** | Same model as Acrobat's. |
| Inputs | Current file, files, folders, or all open files. |
| ★ UI structuring | **Panels**, **instructions** and **dividers** inside an action. |
| Predefined actions | Distribution, optimize, protect sensitive info, archive, and **Make Accessible (PDF/UA-1)** with a generated check report and a log file. |
| Runs without an open document | |

### 3.11 Compare (ch. 3)

| Feature | Notes |
|---|---|
| View › Compare, old vs new, page-range subsets | |
| Options | **Text only**, ★ **Compare Table**, ★ **Detect page deletions/inserts**, and marking colours. |
| Output | A new "The result of Comparison.pdf" with a **summary page and details page**, side-by-side pages, and a **Compare Results contextual ribbon tab** with Next/Prev (Ctrl+. / Ctrl+,), filters (text, images, annotations, formatting), and changes listed in the Comments panel. |
| Limitation | **Comments are not compared.** |

### 3.12 Accessibility (ch. 12)

| Feature | Notes |
|---|---|
| Full Check, accessibility report, Fix (one-click fixes are Pro), Reading Order and tags (Pro), Make Accessible action | |
| ★ **AI formula recognition** | Tags formulas to **MathML**, or converts to MathML/LaTeX through chat. |
| Read Out Loud, reading-settings shortcut | Read Out Loud keys are identical to Acrobat's. |

### 3.13 AI Assistant (ch. 15, subscription only)

| Feature | Notes |
|---|---|
| Backends: **Azure OpenAI GPT and Gemini**. Credit-based (300/month in Editor) | |
| Agent chat with loop, memory, long context and tool calling, plus preset and custom **skills** | |
| ★ **Agentic PDF editing via chat** | The Support Center chatbot can also execute commands, for example "Delete Page 1". |
| ★ **MCP connectors** (Connector Manager, local config) since 2025.2 | |
| ★ **Local AI**: small on-device models and **BYOM** (2026.2) | |
| Other AI tools | Translate whole document, ★ **AI bookmark generation**, ★ **Smart Podcast**, Read Summary, chat with images, multi-document analysis. |
| ★ **Foxit Workspace** | Separate install or web. Project-based multi-source "Crafts" (slides, diagrams, web pages, flashcards). |

---

## 4. Performance reputation

**Bottom line: no independent, methodical benchmark exists in 2025–2026.** Foxit's "fast and light" reputation rests on its history (Foxit Reader in the 2000s) and on anecdote. Foxit's own published disk requirement is now **larger** than Acrobat's.

| Metric | Foxit | Acrobat | Source / confidence |
|---|---|---|---|
| Disk space required (Windows) | **7.5 GB** | **4.5 GB** | [Foxit system requirements](https://www.foxit.com/resource-hub/user-guide/foxit-pdf-editor-for-windows/system-requirements/) (official); [Adobe technical requirements](https://helpx.adobe.com/acrobat/system-requirements.html) via search snippet (official, but I could not fetch it directly) |
| RAM minimum | 512 MB (1 GB recommended) | 2 GB (current); **4 GB** for Acrobat Pro 2024 classic | same sources ([Acrobat 2024](https://helpx.adobe.com/acrobat/system-requirements-acrobat-classic.html)) |
| Download size | Web stub about 1.4 MB. **Portable Windows 175 MB**, **macOS 196 MB** (v2025.2.1) | Full installer is multi-GB (unverified) | [netzwelt](https://www.netzwelt.de/software-chooser/3968-foxit-pdf-editor.html) (third-party) |
| Installed size | "≈700 MB" | "≈2–3 GB" | [unio.digital](https://unio.digital/compare/foxit-vs-adobe-acrobat) (unverified, no method) |
| Open time, large file | "~2 seconds (large files)" | "slower on large/scanned files" | [pdnob](https://www.pdnob.com/alternative/foxit-vs-adobe-acrobat.html) (unverified; PDF-tool vendor) |
| Startup / RAM | "launches faster … uses less RAM, noticeable on older machines or 100+ page documents" | — | [thebusinessdive](https://thebusinessdive.com/adobe-acrobat-vs-foxit) (anecdotal, no numbers) |
| Batch OCR footprint | "significantly less RAM and CPU" on 500-page scans; "loads large, graphically intense PDF files significantly faster" | — | [AuthenCIO, Aug 2026](https://www.authencio.com/blog/foxit-vs-adobe-it-managers-guide-to-switching) cites unnamed "independent 2026 benchmarks" (**unverified**) |
| Responsiveness | "remains smooth and responsive" (Nov 2025 update) | — | [TechRadar](https://www.techradar.com/reviews/foxit-pdf-editor-12) |
| Task completion | "7% faster than with Adobe Acrobat" | — | [Foxit one-pager](https://cdn01.foxitsoftware.com/pub/foxit/datasheet/general/en_us/foxit-vs-adobe-one-pager.pdf) (vendor claim) |
| Counter-evidence | Users report high memory use in Foxit Reader and occasional slowness | Acrobat is "highly stable … with complex PDF files" | aggregated in [pdfpro](https://pdfpro.com/blog/general/foxit-vs-adobe) and [Capterra reviews](https://www.capterra.com/p/249044/Foxit-PDF-Editor/reviews/) (anecdotal) |

What actually keeps Foxit lighter at the architecture level (from the manual):
- **Lazy plug-ins.** Many features ship as plug-ins that stay in "standby" and load on first use. OCR is a separate download (§ Plug-in Management Platform).
- **Native 64-bit** on Windows only since 2025.2. Before that it ran 32-bit (per the [PDF Association](https://pdfa.org/foxit-20252-and-v14-release-highlights-faster-smarter-more-secure/)).
- **Native Win32/C++ UI**, compared with Acrobat's increasingly web-tech UI (unverified for Foxit's current UI stack).

**Opportunity for sheaf:** nobody publishes reproducible open-time, RSS or cold-start numbers for these products. A public, scripted benchmark (cold start, open a 1,000-page scanned file, scroll to page 500, RSS) would be a credible differentiator on its own.

---

## 5. UX patterns: copy or avoid

| Pattern | Foxit behaviour | Verdict for sheaf |
|---|---|---|
| **Ribbon (Office 2013 model)** | Tabs: File (backstage), Home, Convert, Edit, Organize, Comment, View, Form, Protect, Share, Accessibility, AI Assistant, Help. Contextual tabs appear when needed, for example Compare Results and Format. Hand and Select are pinned at the left of every tab. | **Mixed.** Discoverable and familiar to Office users, and Foxit markets it as a key reason to switch. It costs vertical space and duplicates commands across tabs. **Copy:** contextual tabs, and always-present Hand/Select. **Avoid:** a deep tab hierarchy as the only way in. |
| **Collapsible ribbon** | Ctrl+F11, the pin/arrow, "Minimize the Ribbon" | Copy: one key to reach a chrome-less doc view |
| **Quick Access Toolbar** | Above or below the ribbon, add any command by right-click. **Ribbon fully customizable**: new tabs and groups, rename, reorder, **import/export as XML**. | **Copy.** Make the toolbar layout a plain-text config that can be shared between users or deployed by IT. |
| **Command search** | Title-bar search box (Alt+Q) for commands, text, help, and AI prompts | **Copy.** A command palette is cheap and high-value. Keep it separate from doc search; Foxit overloads one box. |
| **Single-key accelerators off by default, Acrobat-compatible** | Opt-in preference | Copy the Acrobat-compatible set; consider defaulting it on |
| **Tabbed documents and SDI option** | Tabs by default; tear-off to a new window; MRU Ctrl+Tab; wheel-scroll tabs; **parallel tab groups**; SDI via "always open in new window/instance" | Sheaf just dropped tabs for one document per window (commit `861f9e1`). Foxit shows that power users want **both** modes, and side-by-side **cross-document** groups are what Acrobat lacks. If sheaf stays one-doc-per-window, provide good window tiling and Ctrl+Tab across windows instead. |
| **Classic vs ribbon** | Older builds (PhantomPDF era) offered a "Classic toolbar" mode alongside Ribbon. A third-party training page notes Typewriter sat under Comment in Classic and Home in Ribbon ([ustech.ninja](https://ustech.ninja/foxit-phantom-basics), undated). The 2026.2 manual documents **Ribbon mode only**, so Classic appears to have been removed (unverified when). | **Avoid** shipping two parallel UI modes. It doubles documentation and makes commands move between modes. |
| **Right-hand Format panel and floating toolbar** | Properties for the selected object, with palette hotkeys | Copy, especially number-key colour swatches |
| **Touch mode toggle** | One button that inflates hit targets | Cheap to copy if sheaf targets tablets |
| **Start page / Tool Wizard** | Recents, task wizards, AI chat history, cloud | Optional. Users can disable it, and admins can disable it org-wide. Keep any equivalent lightweight and off the startup path. |
| **Contextual help** | Hover tips with a "Tell me more" link into the Help Center, plus an AI Support Center that runs commands | The hover-to-docs link is worth copying |
| **Inconsistencies to avoid** | Win and Mac shortcut maps disagree on Fit Page/Width/Visible and on comment-tool modifiers. The default comment shortcut Shift+U collides with single-key Shift+U. Docs disagree on Pause Read-Aloud. The Windows page has a blank Text Box key. | Generate shortcut docs **from the keymap source**, and lint for collisions in CI |
| **Feature gating by edition** | Pro-only, subscription-only and region-only labels appear throughout the manual | Reviewers' main complaint ("Some advanced features only available in Editor+", [PCWorld](https://www.pcworld.com/article/2163281/foxit-pdf-editor-review.html)). An OSS editor wins here by default. |
| **Plug-in platform** | Signed `.fzip` / `.fpi` plug-ins, manifest-based XML install, per-plug-in enable toggle, lazy load, core plug-ins that cannot be uninstalled | Relevant to sheaf's plugin system (commit `d3508a0`). **Copy** signature status, lazy load and the enable/disable UI. |
| **Command-line switches** | `/p` print, `/t` print to printer, `-pwd`, `/A` open parameters, `-Register` | Copy `/A`-style open parameters (page, zoom, named destination), compatible with Adobe's open parameters |
