# Prior art: Adobe Acrobat Pro (desktop, 2025–2026)

Research date: 2026-10-13. Scope: Acrobat Pro on Windows and macOS, continuous (subscription) track plus perpetual Acrobat Pro 2024. Primary sources are Adobe help pages. helpx.adobe.com returns HTTP 403 to scripted fetches, so pages were read from Wayback Machine snapshots (2025–2026) of the same URLs. Adobe is moving its docs to a new tree (`helpx.adobe.com/acrobat/desktop/...`), and many old `acrobat/using/*` pages are now short task-only stubs.

Legend: **[A]** means verified on an Adobe page. **[3P]** means a third-party or community source only. **(unverified)** means from memory or not confirmed against a current source.

---

## 1. Keyboard shortcuts

Source: [Adobe, *Keyboard shortcuts* (last updated Sep 23, 2025)](https://helpx.adobe.com/acrobat/using/keyboard-shortcuts.html) [A]. Tables follow Adobe's grouping and wording. "Windows/UNIX" is Adobe's column label.

> **Single-key accelerators:** single letters such as H, V, Z, R, O, A, C, L, F, M, T, S, K, J, U, D, Q and X, and Shift+letter cycles, work **only** when *Preferences > General > Basic Tools > "Use single-key accelerators to access tools"* is on. It is **off by default** ([Viewing preferences](https://helpx.adobe.com/acrobat/using/viewing-pdfs-viewing-preferences.html) [A]). Adobe publishes **no shortcut editor**, and community threads confirm users can't remap keys ([3P](https://wiert.me/2026/08/28/i-wish-adobe-reader-acrobat-had-keyboard-shortcuts-for-1-up-2-up-like-macos-preview-has/)).

### 1.1 Navigating tools on the tools pane (modern UI)
| Action | Windows | macOS |
|---|---|---|
| Next / previous tool in global toolbar | Tab / Shift+Tab | Tab / Shift+Tab |
| Expand tool in focus; close open tool (close btn focused) | Space or Enter | Space or Enter |
| Focus "View more" in All tools pane | Tab | Tab |
| Next / previous touchpoint in All tools pane | Down / Up | Down / Up |
| Open touchpoint in focus; expand/collapse View more/less | Space or Enter | Space or Enter |
| Close All tools pane | Esc | Esc |

### 1.2 Accessing and switching between tools (single-key, needs the preference)
| Tool | Windows | macOS |
|---|---|---|
| Hand tool, temporary | Space (hold) | Space (hold) |
| Pan (Hand) | H | H |
| Select | V | V |
| Marquee Zoom | Z | Z |
| Cycle Marquee Zoom / Dynamic Zoom / Loupe | Shift+Z | Shift+Z |
| Dynamic Zoom, temporary while Marquee Zoom is active | Shift | Shift |
| Zoom out, temporary while Marquee Zoom is active | Ctrl | Option |
| Zoom In, temporary | Ctrl+Space | Cmd+Space |
| Select Object | R | R |
| Edit Object | O | O |
| Enter/exit Forms editing | A | A |
| Crop | C | C |
| Link | L | L |
| Text Field | F | F |
| Cycle form tools (Text, Check Box, Radio, List, Dropdown, Button, Digital Signature, Barcode) | Shift+F | Shift+F |
| 3D tool | M | M |
| Cycle multimedia (3D, SWF, Sound, Video) | Shift+M | Shift+M |
| Edit text | T | T |
| Redact | Shift+Y | Shift+Y |
| Cycle Touch Up (Text, Reading Order, Object) | Shift+T | Shift+T |
| JavaScript Debugger | Ctrl+J | Cmd+J |
| Insert blank pages | Shift+Ctrl+T | Shift+Cmd+T |
| Insert files | Ctrl+Shift+I | Shift+Cmd+I |
| Delete pages | Ctrl+Shift+D | Shift+Cmd+D |
| Output Preview | ~ | ~ |
| Touch Up Reading Order (or refocus its dialog) | Shift+Ctrl+U | Shift+Cmd+U |

### 1.3 Navigating task panes
| Action | Windows | macOS |
|---|---|---|
| Next / previous region (document pane, task panes, message bar, nav bar) | F6 / Shift+F6 | F6 / Shift+F6 |
| Next / previous panel in task pane | Ctrl+Tab / Ctrl+Shift+Tab | Option+Tab / Cmd+Shift+Tab |
| Next / previous control in task pane | Tab / Shift+Tab | Tab / Shift+Tab |
| Next / previous command button in panel | Down / Up | Down / Up |
| Expand/collapse panel; run command; open submenu | Space or Enter | Space or Enter |
| Open/close task pane | Shift+F4 | Shift+F4 |
| Close the pane listing an action's tasks | Ctrl+Shift+F4 | Ctrl+Shift+F4 |
| Back to parent from submenu | Esc | Esc |
| Navigate items in Create/Edit Action and Custom Tool dialogs | Tab / Shift+Tab | Tab / Shift+Tab |

### 1.4 Adding, selecting, and navigating comments
| Action | Windows | macOS |
|---|---|---|
| Add a comment (sticky note) | S | S |
| Add a stamp | K or J | K or J |
| Current highlight tool | U | U |
| Cycle Highlight / Underline / Strikethrough | Shift+U | Shift+U |
| Current drawing markup tool | D | D |
| Cycle drawing tools | Shift+D | Shift+D |
| Cloud | Q | Q |
| Add text comment (text box) | X | X |
| Cycle Stamp / Attach file / Sound | Shift+J | Shift+J |
| Next / previous comment or form field | Tab / Shift+Tab | Tab / Shift+Tab |
| Toggle checkmark on selected comment in Comments list | Shift+K | Shift+K |
| Open pop-up note (or text field in list) for focused comment | Enter | Return |
| Reply to focused comment (Comments list) | R | R |
| Close pop-up | Esc | Esc |

### 1.5 Scrolling, zooming, and jumping between pages
| Action | Windows | macOS |
|---|---|---|
| Previous / next screen | PgUp or Shift+Enter / PgDn or Enter | PgUp or Shift+Return / PgDn or Return |
| First page | Home, Shift+Ctrl+PgUp, or Shift+Ctrl+Up | Home or Shift+Cmd+Up |
| Last page | End, Shift+Ctrl+PgDn, or Shift+Ctrl+Down | End or Shift+Cmd+Down |
| Previous / next page | Left or Ctrl+PgUp / Right or Ctrl+PgDn | Left or Cmd+PgUp / Right or Cmd+PgDn |
| **Go to page** | **Ctrl+Shift+N** | **Cmd+Shift+N** |
| Previous / next open document | Ctrl+F6 / Shift+Ctrl+F6 (UNIX) | Cmd+F6 / Shift+Cmd+F6 |
| Scroll up / down | Up / Down | Up / Down |
| Scroll (Hand tool active) | Space | Space |
| Zoom in / out | Ctrl+= / Ctrl+- | Cmd+= / Cmd+- |

### 1.6 Filling out and navigating forms (form-editing mode, single-key)
| Action | Windows | macOS |
|---|---|---|
| Toggle edit / preview | P | P |
| Toggle guides | G | G |
| Align left / right / top / bottom | L / R / T / B | L / R / T / B |
| Align horizontal / vertical | H / V | H / V |
| Center horizontally / vertically | Shift+H / Shift+V | Shift+H / Shift+V |
| Highlight fields | Shift+L | Shift+L |
| Show tab order | Shift+N | Shift+N |
| Document JavaScripts | Shift+D | Shift+D |

**Working with forms (fill-in mode)** [A]: Tab / Shift+Tab accepts the entry and moves to the next or previous field. Up or Left selects the previous radio button and Down or Right the next. Esc rejects the entry and deselects the field. Esc twice exits full screen. Enter in a single-line field accepts it. Enter in a multiline field inserts a paragraph return. Enter toggles a checkbox. Keypad Enter accepts and deselects. Ctrl+Tab (Win) or Cmd+Tab (Mac) inserts a tab into a text field.

### 1.7 PDF Portfolios
| Action | Windows | macOS |
|---|---|---|
| Next / previous row | Up / Down | Up / Down |
| Up one folder level | Backspace | Delete |
| Open subfolder or attachment preview | Enter | Enter |
| First / last row; page of rows | Home/End; PgDn/PgUp | same |
| Select all / deselect all | Ctrl+A / Shift+Ctrl+A | Cmd+A / Shift+Cmd+A |
| Extend selection | Shift+Up/Down | Shift+Up/Down |
| Toggle row in selection | Ctrl+Space | Cmd+Space |
| Move focus without changing selection | Ctrl+Up/Down | Cmd+Up/Down |

### 1.8 General navigation
| Action | Windows | macOS |
|---|---|---|
| Invoke the menu (F6 to the hamburger, then) | Space or Enter | — |
| Hide others | n/a | Option+Cmd+T |
| Focus toolbar in browser / Home view | Shift+F8 | Shift+F8 |
| Move among top-bar controls (Home, Tools, Document) | Left/Right/Tab/Shift+Tab | same |
| Next / previous document tab | Ctrl+Tab / Ctrl+Shift+Tab | Ctrl+Tab / Ctrl+Shift+Tab |
| Next / previous document window | Ctrl+F6 / Ctrl+Shift+F6 | Cmd+F6 / Cmd+Shift+F6 |
| Close current document | Ctrl+F4 | Cmd+F4 |
| Close all documents | n/a | Cmd+Option+W |
| Focus document pane | F5 | F5 |
| Next / previous comment, link, or field | Tab / Shift+Tab | Tab / Shift+Tab |
| Activate selected tool or item | Space or Enter | Space or Return |
| Context menu / close it | Tab then Shift+F10 / F10 | Ctrl+click / Esc |
| Return to Hand or Select tool | Esc | Esc |
| Next tab in tabbed dialog | Ctrl+Tab | n/a |
| Find | Ctrl+F | Cmd+F |
| Next search result | F3 or Ctrl+G | Cmd+G |
| Previous / next document in multi-file search | Alt+Shift+Left/Right | Cmd+Shift+Left/Right |
| Select text (Select tool) | Shift+arrows | Shift+arrows |
| Select next word / deselect previous word | Shift+Ctrl+Right/Left | n/a |
| Slider value | Right/Left | Ctrl+Option+Right/Left |

### 1.9 Navigation panels (Bookmarks, Pages, etc.)
| Action | Windows | macOS |
|---|---|---|
| Open navigation pane and focus it | Ctrl+Shift+F5 | Cmd+Shift+F5 |
| Cycle focus between document, message bar, and panels | F6 / Shift+F6 | F6 / Shift+F6 |
| Move among elements of active panel | Tab | Tab |
| Previous / next panel (panel button focused) | Up / Down | Up / Down |
| Next panel (focus anywhere in pane) | Ctrl+Tab | n/a |
| File list: select/deselect file (opens Context pane) | Space | Space |
| File list: open selected file or To Do card | Enter | Enter |
| File list: move, extend selection, move without selecting | arrows / Shift+arrows / Ctrl+arrows | arrows / Shift+arrows / Cmd+arrows |
| **Expand current bookmark** | Right or Shift+"+" | Right or Shift+"+" |
| **Collapse current bookmark** | Left or "-" | Left or "-" |
| **Expand all bookmarks** | Shift+* | Shift+* |
| Collapse selected bookmark | / | / |
| Next / previous item in panel | Down / Up | Down / Up |

### 1.10 Help window
| Action | Windows | macOS |
|---|---|---|
| Open user guide | F1 | F1 or Cmd+? |
| Close user guide | Ctrl+W or Alt+F4 | Cmd+W |
| Back / forward topic | Alt+Left / Alt+Right | Cmd+Left / Cmd+Right |
| Next / previous pane | Ctrl+Tab / Shift+Ctrl+Tab | browser-dependent |
| Next / previous link; activate link | Tab / Shift+Tab; Enter | n/a |
| Print topic | Ctrl+P | Cmd+P |

### 1.11 Accessibility and screen readers
| Action | Windows | macOS |
|---|---|---|
| Edit Tag | F2 | F2 |
| **Change reading settings for current document** | **Shift+Ctrl+5** | Shift+Cmd+5 |
| **Reflow tagged PDF / return to normal view** | **Ctrl+4** | Cmd+4 |
| Activate/deactivate Read Out Loud | Shift+Ctrl+Y | Shift+Cmd+Y |
| Read current page / whole document | Shift+Ctrl+V / Shift+Ctrl+B | Shift+Cmd+V / Shift+Cmd+B |
| Pause / stop reading | Shift+Ctrl+C / Shift+Ctrl+E | Shift+Cmd+C / Shift+Cmd+E |

### 1.12 Common shortcuts missing from Adobe's list (verification status)
| Action | Windows | macOS | Status |
|---|---|---|---|
| Full Screen mode toggle | Ctrl+L | Cmd+L | **[A]** ([viewing prefs](https://helpx.adobe.com/acrobat/using/viewing-pdfs-viewing-preferences.html): "use the keyboard shortcut Ctrl + L"; Esc exits only if "Escape Key Exits" is on) |
| Rotate *view* clockwise (temporary, not saved) | Shift+Ctrl+Plus | Shift+Cmd+Plus | **[A]** ([Adjusting PDF views](https://helpx.adobe.com/acrobat/using/adjusting-pdf-views.html)) |
| Rotate view counter-clockwise | Shift+Ctrl+Minus | Shift+Cmd+Minus | (unverified). [3P list](https://ipac-docs.jacow.org/PaperEditing/Editing/shortcuts/) gives Ctrl+Shift+1 |
| Show/hide menu bar (macOS) | — | Shift+Cmd+M | **[A]** (viewing prefs) |
| Read Mode (hide toolbars and panes) | Ctrl+H | Cmd+H (conflicts with macOS Hide, so unverified) | [3P](https://ipac-docs.jacow.org/PaperEditing/Editing/shortcuts/). The **menu item** View > Read Mode is [A] |
| Automatically scroll (toggle) | Ctrl+Shift+H | Cmd+Shift+H | [3P](https://community.adobe.com/t5/acrobat-reader-discussions/what-s-the-shortcut-key-combo-for-quot-fit-to-width-scrolling-quot/m-p/15259736/highlight/true). The **menu item** View > Page display > Automatically scroll (Esc stops) is [A] ([Navigate pages](https://helpx.adobe.com/acrobat/using/navigating-pdf-pages.html)) |
| Zoom to Page Level (fit page) | Ctrl+0 | Cmd+0 | [3P](https://community.adobe.com/t5/acrobat-discussions/default-view-settings/m-p/11016020/highlight/true). Menu View > Zoom > Zoom to page level is [A] |
| Actual Size (100%) | Ctrl+1 | Cmd+1 | (unverified, widely cited). Menu item [A] |
| Fit Width | Ctrl+2 | Cmd+2 | [3P](https://community.adobe.com/t5/acrobat-reader-discussions/what-s-the-shortcut-key-combo-for-quot-fit-to-width-scrolling-quot/m-p/15259736/highlight/true) |
| Fit Visible | Ctrl+3 | Cmd+3 | [3P](https://ipac-docs.jacow.org/PaperEditing/Editing/shortcuts/) |
| Rotate *pages* (Organize, saved) dialog | Ctrl+Shift+R | Cmd+Shift+R | [3P](https://ipac-docs.jacow.org/PaperEditing/Editing/shortcuts/) |
| Document Properties | Ctrl+D | Cmd+D | [3P](https://ipac-docs.jacow.org/PaperEditing/Editing/shortcuts/) |
| Preferences | Ctrl+K | Cmd+, (unverified) | [3P] |
| Add bookmark (from current view or selection) | Ctrl+B | Cmd+B | [3P] |
| Rulers / Grid toggle | Ctrl+R / Ctrl+U | Cmd+R / Cmd+U | [3P]. Menu items Show/Hide > Rulers & Grids are [A] |
| Next / previous page (alternate) | Ctrl+M / Ctrl+Shift+M (unverified in modern UI) | — | [3P] |
| Previous view / next view (navigation history) | Alt+Left / Alt+Right | Cmd+Left / Cmd+Right | (unverified). Menu Page navigation > Previous/Next view is [A] |
| Find (advanced search pane) | Ctrl+Shift+F | Cmd+Shift+F | [3P] |
| Navigation pane toggle (classic) | F4 | F4 (unverified) | [3P]. Modern UI uses Ctrl+Shift+F5 [A] |
| Toolbar toggle (classic) / menu bar toggle (Windows) | F8 / F9 | F8 / — | [3P]. Behavior in the modern UI is (unverified) |
| Cascade / tile horizontally / tile vertically | Ctrl+Shift+J / K / L | — | [3P] |
| Compare results: side-by-side view | Ctrl+\ | Cmd+\ (unverified) | **[A]** ([Compare files](https://helpx.adobe.com/acrobat/using/compare-documents.html)) |
| Single page / two-page / continuous display modes | — | — | **No default shortcut** ([3P complaint](https://wiert.me/2026/08/28/i-wish-adobe-reader-acrobat-had-keyboard-shortcuts-for-1-up-2-up-like-macos-preview-has/)). Menu only |
| "Ctrl+Shift+4" | — | — | Not found in any source (unverified, probably not assigned) |

Standard OS-level shortcuts (open, save, save as, print, close, undo, redo, cut, copy, paste, select all, quit) follow OS conventions: Ctrl/Cmd+O/S/Shift+S/P/W/Z/Shift+Z (Win also Ctrl+Y)/X/C/V/A/Q [3P].

---

## 2. Feature inventory

The modern UI reaches tools through **All tools** in the left rail. Tool names below are the 2025–2026 labels in quotes.

### 2.1 Viewing and navigation
| Feature | Details | Source |
|---|---|---|
| Page display | Single page view, Enable scrolling, Two page view, Two page scrolling, plus "Show cover page in two page view". Default layout pref options: Automatic, Single Page, Single Page Continuous, Two-Up, Two-Up Continuous. Document Initial View overrides the pref | [A](https://helpx.adobe.com/acrobat/using/adjusting-pdf-views.html) |
| Zoom | Actual size, Zoom to page level, Fit to width, Fit height, Fit visible content, Marquee zoom, Dynamic zoom, Pan & Zoom window, Loupe tool (separate magnified window with a draggable rectangle). Zoom box in right rail. You can pin zoom and fit items to the right pane | [A](https://helpx.adobe.com/acrobat/using/adjusting-pdf-views.html) |
| Split view | Window > Split gives 2 panes, each scrolling and zooming independently. Window > Spreadsheet split gives 4 panes with shared zoom and coordinated scrolling, so headers stay frozen. Window > Remove split. On macOS the drag handle above the scrollbar was removed in the Sep 2023 scroll rewrite | [A](https://helpx.adobe.com/acrobat/using/adjusting-pdf-views.html), [A](https://helpx.adobe.com/acrobat/using/scroll-framework-upgrade-macos.html) |
| New Window | Window > New window opens the same doc in another window with suffixes ":1", ":2" that renumber when one closes. Not available in browser | [A](https://helpx.adobe.com/acrobat/using/adjusting-pdf-views.html) |
| Read Mode | Hides toolbars and panes and shows a semi-transparent floating toolbar (page nav and zoom) | [A](https://helpx.adobe.com/acrobat/using/viewing-pdfs-viewing-preferences.html) |
| Full Screen | Ctrl+L. Prefs: current doc only, one page at a time, monitor choice, Esc exits, nav bar, left/right click to page, loop, auto-advance every N s, background color, transitions | [A](https://helpx.adobe.com/acrobat/using/viewing-pdfs-viewing-preferences.html) |
| Rotate view | Temporary, 90° steps, not saved. Separate from Organize > Rotate pages, which is saved | [A](https://helpx.adobe.com/acrobat/using/adjusting-pdf-views.html) |
| Navigation history | Previous view / Next view; Automatically scroll; article threads | [A](https://helpx.adobe.com/acrobat/using/navigating-pdf-pages.html) |
| Page labels | Logical page numbers (Roman front matter, prefixes like "1-1") shown in nav and Go To Page as "i (1 of N)". Set via Organize pages > Page labels | [A](https://helpx.adobe.com/acrobat/using/manipulating-deleting-renumbering-pdf-pages.html) |
| Rulers, grids, guides | Show/Hide > Rulers & Grids > Rulers / Grid / Snap to Grid / Guides / Line Weights. Grid spacing, offset, subdivisions and colors in Units & Guides prefs. Drag guides from rulers; "Clear All Guides" | [A](https://helpx.adobe.com/acrobat/using/grids-guides-measurements-pdfs.html) |
| Cursor coordinates | Live x/y readout plus object W/H while resizing; units from Units & Guides prefs | [A](https://helpx.adobe.com/acrobat/using/grids-guides-measurements-pdfs.html) |
| Measure | All tools > Measure object: Distance, Perimeter, Area. Snap to paths, endpoints, midpoints, intersections; Shift constrains to 45°. Scale ratio and precision, ortho, measurement markup saved as **comments**, **Export Measurement Markup to Excel (CSV)**, Measurement Info panel | [A](https://helpx.adobe.com/acrobat/using/grids-guides-measurements-pdfs.html) |
| Display themes and sizing | System / Light Gray / Dark Gray themes; Display Size Small/Standard/Large; Touch mode | [A](https://helpx.adobe.com/acrobat/using/viewing-pdfs-viewing-preferences.html) |
| Tabs | Documents open as tabs in one window, which can be dragged out to their own windows. "Open PDFs from last session on launch" pref | [A](https://helpx.adobe.com/acrobat/using/viewing-pdfs-viewing-preferences.html) |
| Page thumbnails | Generated lazily ("may require several seconds… drawing may pause if you interact"). Thumbnail shows a draggable and resizable **page-view box** as a pan/zoom proxy. Reduce/Enlarge sizes | [A](https://helpx.adobe.com/acrobat/using/page-thumbnails-bookmarks-pdfs.html) |
| Search | Find (Ctrl+F) bar plus Advanced Search (multi-doc, folders, indexes via Catalog) | [A](https://helpx.adobe.com/acrobat/using/searching-pdfs.html) |
| Other view aids | Show art/trim/bleed boxes, transparency grid, overprint preview, always show page size, Line Weights view, layers, PDF/A view mode (opens read-only) | [A](https://helpx.adobe.com/acrobat/using/viewing-pdfs-viewing-preferences.html) |

### 2.2 Organize Pages ("Organize pages" tool)
| Function | Notes | Source |
|---|---|---|
| Thumbnail grid with drag reorder, multi-select, zoom slider | (unverified detail) | |
| Rotate page left/right (saved) | [A](https://helpx.adobe.com/acrobat/using/adjusting-pdf-views.html) | |
| Insert pages (from file, blank, clipboard, scanner, web page) | Insert blank page Shift+Ctrl+T and Insert files Ctrl+Shift+I [A] | [A](https://helpx.adobe.com/acrobat/using/keyboard-shortcuts.html) |
| Delete pages | Ctrl+Shift+D [A]; [delete pages](https://helpx.adobe.com/acrobat/using/delete-pages-from-pdf.html) | |
| Extract pages, **discontinuous selection** (Acrobat Pro 2024) | Right-click thumbnail > Extract Pages | [A](https://helpx.adobe.com/acrobat/using/whats-new-acrobat-pro-2024.html) |
| Replace pages, Split (by page count, file size, top-level bookmarks) | (unverified against the current page) | |
| Page labels | see 2.1 | [A] |
| Page properties > Tab order (row / column / document structure) | | [A](https://helpx.adobe.com/acrobat/using/page-thumbnails-bookmarks-pdfs.html) |
| Crop pages | Crop tool "C"; separate Crop pages page | [A](https://helpx.adobe.com/acrobat/using/crop-pdf-pages.html) |

### 2.3 Combine Files
Combine and merge many files and types (Office, images, PDFs) into one PDF with reordering. The 2026 option "Create PDF Spaces while combining files" makes a PDF Space instead of a merged PDF ([what's new](https://helpx.adobe.com/acrobat/using/whats-new.html) [A]). Classic option: create a PDF Portfolio instead of a merged file (portfolio shortcuts in 1.7). Detailed merge options (bookmarks per source file, file-size presets) are (unverified).

### 2.4 Edit PDF ("Edit" in the global bar)
| Feature | Notes | Source |
|---|---|---|
| Edit text | Click into a text box (blue box plus rotation handle). FORMAT TEXT panel (font, size, color, alignment, **Horizontal Scaling**). Needs the font installed; if it's only embedded, only color and size change | [A](https://helpx.adobe.com/acrobat/using/edit-text-pdfs.html), [A](https://helpx.adobe.com/acrobat/using/grids-guides-measurements-pdfs.html) |
| Add text and images; edit, replace, crop, flip, rotate images; arrange z-order | | [A](https://helpx.adobe.com/acrobat/using/edit-images-or-objects-pdf.html) |
| Links (L tool), attachments, Web links from URLs pref | | [A](https://helpx.adobe.com/acrobat/using/links-attachments-pdfs.html) |
| Edit with **AI chat** (Aug 2026): rewrite, replace, format, or highlight via natural language; Apr 2026: reorganize pages, header/footer, watermark via chat | | [A](https://helpx.adobe.com/acrobat/using/whats-new.html) |
| Headers and footers, watermark, background, Bates (see 2.15) | | |
| Edit Object (O): move and transform raster and vector objects; Touch Up tools | | [A](https://helpx.adobe.com/acrobat/using/print-production-tools-overview-acrobat.html) |
| Selection floating toolbar: Highlight, Strikethrough, Underline, Redact, Copy, Edit | | [A](https://helpx.adobe.com/acrobat/using/mark-text-edits.html) |

### 2.5 Comments and annotation tools
| Feature | Notes | Source |
|---|---|---|
| Types | Sticky note (S), Text box (X), Highlight, Underline, Strikethrough (U / Shift+U), Insert/Replace text edits, drawing tools (D / Shift+D: rectangle, oval, line, arrow, polygon, polyline, pencil/freeform, eraser), Cloud (Q), Callout, Stamp (K/J), Attach file, Sound (Shift+J), Measurement markups | [A](https://helpx.adobe.com/acrobat/using/keyboard-shortcuts.html) |
| Quick action toolbar holds commenting tools; selection floating toolbar has "Add a comment" | | [A](https://helpx.adobe.com/acrobat/using/commenting-pdfs.html) |
| Stamps | Categories, custom stamps (from image or PDF), favorites ("Add Current Stamp To Favorites"), properties (opacity, color), rotate handle | [A](https://helpx.adobe.com/acrobat/using/adding-stamp-pdf.html) |
| Dynamic stamps | Built-in "Dynamic" set (Approved, Reviewed, Received, Revised, Confidential, …) stamps name, date and time from Identity prefs. Custom dynamic stamps via JavaScript in stamp PDFs. Very common in audit "tick and tie" | (unverified on a current Adobe page; long-standing feature) |
| Comments list | Filter by **reviewer, type, status, color**, also checked/unchecked. Filtering hides the comments both on the page and in the list. Search comments, sort, reply (R), status (Accepted/Rejected/Cancelled/Completed), **checkmark** (Shift+K) | [A](https://helpx.adobe.com/acrobat/current/comments.html), [A](https://helpx.adobe.com/acrobat/desktop/share-and-review-documents/manage-reviews/view-comments.html) |
| Summarize | "Create Comment Summary" (layouts: doc + comments with connector lines, comments only, etc.; sort order; page range; include hidden or not) produces a static PDF. "Print With Comments Summary" | [A](https://helpx.adobe.com/acrobat/current/comments.html) |
| Import/export | Export All / Export Selected **to FDF/XFDF**, Import Data File (FDF/XFDF/PDF), **Export to Word** (Windows; tagged PDF from Word; integrate text edits one by one), **Export to AutoCAD** (Pro, Windows) | [A](https://helpx.adobe.com/acrobat/using/importing-exporting-comments.html) |
| Shared review | Share links, SharePoint/Office 365 hosted reviews, tracking | [A](https://helpx.adobe.com/acrobat/using/host-shared-reviews-on-sharepoint-office-365-sites.html) |

### 2.6 Forms ("Prepare a form")
| Feature | Notes | Source |
|---|---|---|
| Auto field detection | Converting a doc or scan auto-detects fields; pref "Automatically detect Form fields"; "This document requires signatures" | [A](https://helpx.adobe.com/acrobat/using/create-form.html), [A](https://helpx.adobe.com/acrobat/using/pdf-forms-basics.html) |
| Start from blank page (Pro 2024) | | [A](https://helpx.adobe.com/acrobat/using/whats-new-acrobat-pro-2024.html) |
| Field types | Text, Check Box, Radio, List, Dropdown, Button, Digital Signature, Barcode, Date, Image (Shift+F cycles) | [A](https://helpx.adobe.com/acrobat/using/keyboard-shortcuts.html) |
| Fields pane, Highlight existing fields, Preview (P), alignment/centering hotkeys, guides, tab order | | [A](https://helpx.adobe.com/acrobat/using/create-form.html) |
| Field properties | Format (number, %, date, special, custom JS), Validate, **Calculate** (sum/product/avg/min/max of fields, Simplified Field Notation, custom JavaScript), Actions | [A](https://helpx.adobe.com/acrobat/using/pdf-form-field-properties.html) |
| Doc-level JavaScripts (Shift+D in form mode), JS Debugger (Ctrl+J) | | [A] |
| Prefs | Auto-calc field values; auto-adjust tab order; focus rectangle; overflow "+" indicator; highlight color; required-field color; Auto-Complete Off/Basic/Advanced with numeric memory | [A](https://helpx.adobe.com/acrobat/using/pdf-forms-basics.html) |
| Distribute and collect | Send, forms tracker, collect responses, export data | [A](https://helpx.adobe.com/acrobat/using/collecting-pdf-form-data.html) |
| XFA | Can't edit XFA (LiveCycle) forms in Acrobat | [A KB title](https://helpx.adobe.com/acrobat/using/keyboard-shortcuts.html) (nav list: "Unable to edit XFA forms") |

### 2.7 Fill & Sign
Fill & Sign tool for flat (non-interactive) forms: auto-detected fields (detection improved Apr 2026), text, checkmark/cross/dot, initials, signature (type, draw, image; synced to account) ([A](https://helpx.adobe.com/acrobat/using/fill-and-sign-new-experience.html), [A](https://helpx.adobe.com/acrobat/using/whats-new.html)).

### 2.8 Request e-signatures and certificates
- "Request e-signatures" (Acrobat Sign): recipients, order, fields, reminders, web forms, bulk send, payments, branding, Agreements tab in Home ([A](https://helpx.adobe.com/acrobat/using/send-for-signature-new-experience.html)).
- Certificate (digital ID) signatures: "Use a certificate" sign and certify, validation, Adobe Approved Trust List (AATL), trusted identities ([A](https://helpx.adobe.com/acrobat/using/certificate-based-signatures.html), [A](https://helpx.adobe.com/acrobat/using/validating-digital-signatures.html)).

### 2.9 Protect
| Feature | Notes | Source |
|---|---|---|
| "Protect with password" | Open (view) password and/or permissions (editing) password; Advanced options | [A](https://helpx.adobe.com/acrobat/using/securing-pdfs-passwords.html) |
| Permissions | Printing (none / low-res / high-res), changes allowed, copy, screen-reader access (classic dialog; unverified on current page) | |
| Encrypt with certificate | Choose components (all content / all except metadata / only attachments), AES-128 (Acrobat 7+) or AES-256 (Acrobat 9+), recipients from trusted identities, LDAP, file or Windows store, **per-recipient permissions** | [A](https://helpx.adobe.com/acrobat/using/securing-pdfs-certificates.html) |
| Security policies, AEM rights management, **Microsoft Purview (MPIP) labels** (Pro 2024) | | [A](https://helpx.adobe.com/acrobat/using/setting-security-policies-pdfs.html), [A](https://helpx.adobe.com/acrobat/using/whats-new-acrobat-pro-2024.html) |

### 2.10 Redact ("Redact a PDF", Pro)
| Feature | Notes | Source |
|---|---|---|
| Redact text & images | Drag-mark, then **Apply**. The apply dialog has a toggle to **sanitize and remove hidden information**, then "Save Sanitized Document" | [A](https://helpx.adobe.com/acrobat/using/removing-sensitive-content-pdfs.html) |
| Find text & redact | Search a single word or phrase, multiple words or phrases, or **Patterns** (phone, credit card, SSN, email, dates; localized per language pref). Check All or individual hits; whole- or partial-word marks. Doesn't search encrypted PDFs or image-only text | [A](https://helpx.adobe.com/acrobat/desktop/protect-documents/redact-pdfs/redact-text.html) |
| Sanitize document | Remove all hidden info, or selectively: metadata, attachments, comments, form fields, hidden text, layers, bookmarks, embedded search index, overlapping objects | [A](https://helpx.adobe.com/acrobat/desktop/protect-documents/redact-pdfs/sanitize.html) |
| Prefs | Filename prefix/suffix on saving redacted files; remove hidden info on close or email | [A](https://helpx.adobe.com/acrobat/using/viewing-pdfs-viewing-preferences.html) |
| Redaction mark properties (overlay text, fill color, repeat), code sets | (unverified on current page) | |
| AI redaction suggestions (Aug 2026) | | [A](https://helpx.adobe.com/acrobat/using/whats-new.html) |
| Shift+Y Redact tool | | [A] |

### 2.11 Scan & OCR
| Feature | Notes | Source |
|---|---|---|
| Scan from Create > Scanner | Front or both sides, quality slider, Create New / **Append to File** / Save Multiple Files, OCR toggle with language and output, add metadata, **PDF/A-1b**. Scan More / Reverse Sides / Complete. WIA push-scan on Windows | [A](https://helpx.adobe.com/acrobat/using/scan-documents-pdf.html) |
| Recognize Text | Page range and language; output searchable image or editable text | [A](https://helpx.adobe.com/acrobat/using/scan-documents-pdf.html) |
| Enhance scanned file / camera image | Deskew, background removal, de-speckle | [A](https://helpx.adobe.com/acrobat/using/edit-scanned-pdfs.html) |
| **Edit text in scans** | Choosing Edit on a scan auto-runs OCR and makes an editable copy; new text matches the scanned font | [A](https://helpx.adobe.com/acrobat/using/edit-scanned-pdfs.html) |
| Correct suspected OCR errors / "Recognize text > Correct recognized text" | (unverified on current page) | |

### 2.12 Export PDF ("Export a PDF" / Convert)
Word (DOCX/DOC), Excel (XLSX), PowerPoint (PPTX), images (JPEG/PNG/TIFF), RTF, XML, HTML, text, CSV. "Export all images" with a min-size filter (raster only). **Export Selection As** (right-click selection to DOCX/XLSX/CSV/HTML…). Pro 2024 improved hybrid and borderless table detection and rotated tables ([A](https://helpx.adobe.com/acrobat/using/exporting-pdfs-file-formats.html), [A](https://helpx.adobe.com/acrobat/using/whats-new-acrobat-pro-2024.html)). For tax use, PDF-to-Excel table fidelity is the key competitive surface.

### 2.13 Optimize / reduce size
"Compress a PDF" (simple) plus **Save as Other > Optimized PDF** (PDF Optimizer). Panels: Images (downsample, compress), Fonts (unembed/subset), Transparency (flatten), Discard Objects (thumbnails, search index, flatten fields), Discard User Data (comments, forms, multimedia, private data), Clean Up (incl. **fast web view**). "Make compatible with" version selector. "Audit space usage" (unverified). Pref: "Always reduce size for files more than 10MB" ([A](https://helpx.adobe.com/acrobat/using/optimizing-pdfs-acrobat-pro.html), [A](https://helpx.adobe.com/acrobat/using/viewing-pdfs-viewing-preferences.html)).

### 2.14 Preflight, standards, and print production
- **Preflight**: more than 400 predefined checks, Fixups, profiles and libraries, reports, droplets, variables ([A](https://helpx.adobe.com/acrobat/using/print-production-tools-overview-acrobat.html)).
- "Apply PDF standards": Save as PDF/A, PDF/X, or PDF/E. Verify conformance (incl. PDF/UA and PDF/VT) in the Standards panel; remove PDF/A info via fixup ([A](https://helpx.adobe.com/acrobat/using/pdf-x-pdf-a-pdf.html)).
- "Use print production": Output Preview (~), Edit Object, Convert Colors, Flattener Preview, Set Page Boxes, Add Printer Marks, Fix Hairlines, Ink Manager, Trap Presets, Add Article Box ([A](https://helpx.adobe.com/acrobat/using/print-production-tools-overview-acrobat.html)).

### 2.15 Headers, footers, watermarks, backgrounds, Bates
- Header & Footer: 6 positions, tokens (page number, date), margins, page range, saved settings, Update/Remove ([A](https://helpx.adobe.com/acrobat/current/add-headers-footers-pdfs.html)).
- **Bates numbering (Pro)**: inserted through the Header/Footer dialog; **3–15 digits (default 6)**, start number, prefix and suffix (avoid "#"). **Batch** across multiple files with output folder and naming, file order Move Up/Down ([A](https://helpx.adobe.com/acrobat/desktop/edit-documents/apply-bates-numbering/add-bates.html)).
- Watermark (text or file, opacity, rotation, position, appearance on print/screen) ([A](https://helpx.adobe.com/acrobat/using/add-watermarks-pdfs.html)); Background ([A](https://helpx.adobe.com/acrobat/using/add-backgrounds-pdfs.html)).

### 2.16 Action Wizard / batch ("Use guided actions", Pro)
Predefined plus custom **actions** (multi-step, multi-file; files, folders, email attachments, scanner, web page, clipboard as input). Steps can "Prompt User" or use preset settings. Panels, instructions and dividers make guided UIs. Stop/Resume; full report. Export and import as **.sequ** (warns if JavaScript is present). Replaces the old batch sequences (Acrobat XI scripts importable). Auto password entry via digital ID login. **Custom Commands**: preconfigured single-step commands (e.g. Watermark, Header & Footer) with label and tooltip ([A](https://helpx.adobe.com/acrobat/using/action-wizard-acrobat-pro.html)).

### 2.17 Compare Files (Pro)
Old vs new file selection with swap. Page ranges. Doc type modes: **Autodetect**; **Reports/Spreadsheets/Magazine** (continuous text flow); **Presentation decks** (page-matching, detects moved slides); **Scanned docs/drawings** (pixel compare with page matching). "Compare text only". The result is a new PDF with a summary page (timestamp, total changes, distribution, formatting changes, "Go to First Change"). **Side-by-side** (Ctrl+\) with connecting highlight bands and pop-ups; single old or new view; Filter (annotations, formatting and background are off by default) and Show (legends, row highlights); Prev/Next change; changes listed as comments. Text mode is aimed at 250+ page docs ([A](https://helpx.adobe.com/acrobat/using/compare-documents.html)).

### 2.18 Accessibility
Accessibility checker (Full Check, report, Fix), Reading Order tool (Shift+Ctrl+U), Content and Tags panels, autotag including **cloud auto-tagging**, Reflow (Ctrl+4), Read Out Loud (incl. 2026 premium online voices), PDF/UA verify, MathML read-aloud for LaTeX-tagged PDFs (Jun 2026) ([A](https://helpx.adobe.com/acrobat/using/create-verify-pdf-accessibility.html), [A](https://helpx.adobe.com/acrobat/using/whats-new.html)).

### 2.19 AI Assistant and generative features (add-on or Studio)
Summaries and Q&A with **source citations**, multi-doc insights, compare docs, generative summary banner (60–70 words for docs over 3 pages), suggested questions, "smart assistance" tool-based actions via chat, edit or redact via chat (2026), suggested highlights, PDF Spaces (shared AI workspace), podcasts or audio summaries, presentation generation (Adobe Express), cover pages. Pref to disable GenAI. Admin GPO to turn it off ([A](https://helpx.adobe.com/acrobat/using/whats-new.html), [A](https://helpx.adobe.com/acrobat/using/empower-work-documents-ai-assistant-acrobat.html), [A](https://helpx.adobe.com/acrobat/using/disable-generative-ai.html)). The "AI interjections" bother some users enough that they disable the modern UI ([3P Capterra summary](https://capterra.com/p/251329/Adobe-Acrobat-DC/reviews/?page=5)).

### 2.20 Bookmarks panel behaviors
| Behavior | Source |
|---|---|
| **New bookmark from selection**: select text with the Select tool, then Options > New Bookmark. **Selected text becomes the label**. Selecting an image or marquee region bookmarks that area. Destination is the current page and view, so zoom first. It inserts under the selected bookmark, otherwise at the end | [A](https://helpx.adobe.com/acrobat/using/page-thumbnails-bookmarks-pdfs.html) |
| **Wrap Long Bookmarks** toggle (Options menu) shows full text regardless of pane width | [A] same |
| Rename, Set Bookmark Destination (context-sensitive), Properties (color, bold/italic style, actions), "Use Current Appearance as New Default", Text Size | [A] same |
| Set Default Bookmark Zoom Level (default Inherit Zoom); bulk-edit zoom across many bookmarks while keeping page numbers | [A] same |
| Nesting by drag (line indicator); Cut + "Paste under Selected Bookmark"; Expand/Collapse Top-Level Bookmarks; deleting a parent deletes its children | [A] same |
| **New Bookmarks From Structure** produces **tagged bookmarks**. Moving or deleting tagged bookmarks **moves or deletes the corresponding pages** (children follow parents) | [A] same |
| Bookmark actions: go to view, open file or URL, run JavaScript, submit form, etc. | [A] same |
| Classic Options menu "Print Pages / Extract Pages / Delete Pages" for a bookmark's page range | (unverified in current UI) |
| Split document by top-level bookmarks (Organize > Split) | (unverified) |
| Keyboard: Right/Shift+ expands, Left/- collapses, Shift+* expands all, / collapses selected | [A](https://helpx.adobe.com/acrobat/using/keyboard-shortcuts.html) |

### 2.21 Misc. Pro features
Create PDF (PDFMaker for Office, Distiller, printer, web capture), PDF Portfolios, Index creation (Catalog), layers, articles, geospatial, 3D and multimedia, JavaScript (doc, field, folder-level, batch), translate, Outlook and SharePoint integrations, MPIP ([A nav](https://helpx.adobe.com/acrobat/using/workspace-basics.html)).

---

## 3. UX patterns worth noting

### 3.1 The "new Acrobat" (modern) UI, rolled out 2023, default since about 2024
| Element | What it is | Source |
|---|---|---|
| **Three views** | **Home** (recent, starred, files, agreements, recommended tools, prompt bar in Studio), **All tools** (categorized tool catalog), **Document view** (tabs) | [A](https://helpx.adobe.com/acrobat/using/workspace-basics.html) |
| **Global bar** (top) | All tools, Edit, Convert, E-Sign, plus Search, Save, Print, Share; hamburger "Menu" on Windows replaces the classic menu bar | [A] same |
| **All tools left rail** | Choosing a tool swaps the **left panel** to that tool's contextual commands (e.g. Organize pages, Redact, Prepare a form) | [A] same |
| **Quick action toolbar** | **Floating, draggable but not dockable** widget with Select, Hand, comment, highlight, draw, fill, sign. Customizable through Options > Customize toolbar (add with "+", reorder with arrows, remove with trash) | [A] same; [3P](https://community.adobe.com/questions-9/questions-about-new-acrobat-toggle-and-toolbar-customization-1629467) |
| **Right panel** | Top: Comments, Bookmarks, Thumbnails, Articles, AI summary. Bottom: page nav (up/down, page box), rotate, page display menu, zoom in/out. Items can be **pinned** to the right pane; "Customize side panels" | [A](https://helpx.adobe.com/acrobat/using/workspace-basics.html), [A](https://helpx.adobe.com/acrobat/using/adjusting-pdf-views.html) |
| **Selection floating toolbar** | Text or image selection pops a contextual toolbar (comment, highlight, redact, copy, edit); pref "Show Quick actions on text selection" | [A](https://helpx.adobe.com/acrobat/using/mark-text-edits.html) |
| **Classic toggle** | Menu > "Disable new Acrobat" (Windows) or View menu (Mac) reverts to the classic UI. It's a common workaround for performance and AI clutter | [3P](https://community.adobe.com/questions-9/questions-about-new-acrobat-toggle-and-toolbar-customization-1629467), [3P](https://community.adobe.com/t5/acrobat-discussions/acrobat-very-slow-to-start-up-and-very-laggy-when-using-it-on-windows-11/m-p/15555450) |
| **macOS scroll rewrite** (Sep 2023) | NSView replaced by NSScrollView; overlay scrollbars; Shift+wheel for horizontal; page size shown on hover at lower-left; split only via the Window menu | [A](https://helpx.adobe.com/acrobat/using/scroll-framework-upgrade-macos.html) |

**Praise:** TechRadar's Pro 2024 review calls the interface "clean and well designed" with tools that are "easy to find" ([3P](https://www.techradar.com/da-dk/pro/software-services/adobe-acrobat-pro-2024-review)). PCWorld finds the feature volume "dizzying" for new users ([3P](https://www.pcworld.com/article/3215096/adobe-acrobat-pro-dc-review.html)).
**Complaints:** the toolbar can't be docked; the classic menu bar is gone on Windows; constant AI prompts ([3P](https://capterra.com/p/251329/Adobe-Acrobat-DC/reviews/?page=5)); no display-mode shortcuts ([3P](https://wiert.me/2026/08/28/i-wish-adobe-reader-acrobat-had-keyboard-shortcuts-for-1-up-2-up-like-macos-preview-has/)).

### 3.2 Performance complaints with quantified reports
All of these are anecdotal forum reports, not benchmarks.
| Report | Numbers | Source |
|---|---|---|
| Win11, i9 with 32 GB: "Not responding" on launch, on opening any PDF (even 1 page), and on choosing Edit/Combine/Organize | **20–30 s** freezes; 15 s after a prefs reset. Fix: **Protected View = Off** (marked answer, re-confirmed 2024) | [3P](https://community.adobe.com/t5/acrobat-discussions/acrobat-dc-extremely-slow-and-often-quot-not-responding-quot-for-30-seconds/m-p/14207975) |
| Win11 startup | **20–30 s** black screen (2024); **over 1 min** to start and **over 30 s** interactions on an i5-8265U; "at least a minute" to open PDFs after moving from Win10. Fixes: disable Protected Mode at startup, Protected View, Enhanced Security, **Run in AppContainer**; disable New Acrobat; uninstall Grammarly | [3P](https://community.adobe.com/t5/acrobat-discussions/acrobat-very-slow-to-start-up-and-very-laggy-when-using-it-on-windows-11/m-p/15555450) |
| 2025 slow file opening (v25.001.x) | **30+ s** to open PDFs; Protected Mode at startup was the trigger (problem returns when re-enabled). Adobe CM (Aug 2026) suggests turning off **Use Page Cache** and **Generative AI**, and testing local vs network | [3P](https://community.adobe.com/questions-9/how-to-troubleshoot-slow-pdf-opening-in-acrobat-1298515/index1.html) |
| Memory | **About 22 GB** RAM after running unattended; Organize Pages on a **26-page** file reaching **about 90% of 32 GB**; 400-page files using about 3.8 GB, then out-of-memory | [3P](https://community.adobe.com/questions-9/acrobat-suddenly-uses-22gb-of-ram-1302443), [3P](https://community.adobe.com/t5/acrobat-discussions/adobe-pro-is-using-90-of-32-gb-ram-when-managing-sites/td-p/15362886) |
| Big annotated docs | 900 pages with 1,000 comments was very slow. Changing General prefs cut CPU from 80–100% to 30–35% | [3P](https://community.adobe.com/questions-12/adobe-slow-1506388) (via search summary) |
| Redraw and zoom after the 2023 UI and scroll change | "Disastrously slow" zoom redraw on large hi-res files; M2 Pro over 100% CPU and **10–15 s** to resize a window; a user rolled back to 24.003.20121 to fix lag | [3P](https://community.adobe.com/t5/acrobat-discussions/acrobat-pro-2023-extremely-slow-redraw-and-page-navigation/m-p/14253688/highlight/true), [3P](https://community.adobe.com/t5/acrobat-discussions/adobe-acrobat-pro-for-new-mac-driving-over-100-cpu-usage-and-freezing-help/m-p/14264427), [3P](https://community.adobe.com/t5/acrobat-discussions/acrobat-pro-run-slow-and-choppy-on-macbook-pro-with-m1-chipset/m-p/14508266/highlight/true) |
| Review sites | Capterra: "quite slow to load large files… consumes a lot of system resources". TrustRadius: 40% of a 5-review sample cite rendering or OCR speed | [3P](https://capterra.com/p/251329/Adobe-Acrobat-DC/reviews/?page=9), [3P](https://tr-web.dev.trustradius.com/products/adobe-acrobat/reviews/all) |

---

## 4. Performance architecture notes (useful to a competitor)

| Item | Detail | Source |
|---|---|---|
| **Use Page Cache** (Page Display > Rendering) | "Places the next page in a buffer before the current page is viewed to reduce the time required to page through a document." It is **single-page lookahead**, and Adobe staff suggest turning it off when troubleshooting | [A](https://helpx.adobe.com/acrobat/using/viewing-pdfs-viewing-preferences.html), [3P](https://community.adobe.com/questions-9/how-to-troubleshoot-slow-pdf-opening-in-acrobat-1298515/index1.html) |
| **Use 2D Graphics Acceleration** (Windows only, on by default) | "Speeds up zooming, scrolling, and redrawing… rendering and manipulation of 2D PDF content." It needs a supported GPU driver. Separate General pref **"Check 2D Graphics Accelerator"** enables HW accel for the *first* document; it's off by default because it "can slow startup time" | [A] same |
| Rendering quality toggles | Smooth Text (type selectable), Smooth Line Art, Smooth Images, **Enhance Thin Lines**, Use Local Fonts (off: substitute fonts; unsubstitutable shows bullets) | [A] same |
| **Show Large Images** | Turn off "if your system is slow to display image-intensive pages". This is a progressive or omitted image draw | [A] same |
| **Use Smooth Zooming** (Windows) | Animation; "When deselected… improves performance" | [A] same |
| Resolution | System or custom DPI; "Scale for screen resolution (restart required)" | [A] same |
| **Fast Web View** | Docs pref "Save As Optimizes For Fast Web View": restructure for page-at-a-time download (linearization). Also an Optimizer Clean Up option | [A] same, [A](https://helpx.adobe.com/acrobat/using/optimizing-pdfs-acrobat-pro.html) |
| Autosave | "Automatically save document changes to temporary file every _ minutes" | [A](https://helpx.adobe.com/acrobat/using/viewing-pdfs-viewing-preferences.html) |
| Thumbnails | Not embedded (Acrobat no longer embeds); generated on demand; generation pauses during interaction | [A](https://helpx.adobe.com/acrobat/using/page-thumbnails-bookmarks-pdfs.html) |
| Sandboxing | **Protected Mode** (Reader, on by default; Acrobat option "at startup"), **Protected View** (Acrobat, **off** by default; Off / unsafe locations / all files), Enhanced Security, **Run in AppContainer**, Privileged Locations. These are the most-cited slow-startup causes | [A](https://helpx.adobe.com/acrobat/using/protected-view-feature-pdfs-windows.html), [3P](https://community.adobe.com/t5/acrobat-discussions/acrobat-very-slow-to-start-up-and-very-laggy-when-using-it-on-windows-11/m-p/15555450) |
| **AcroCEF.exe** helper processes | Separate processes for network and Document Cloud services (Fill & Sign, Send for signature, Share, Home). Adobe says they "cannot be removed or disabled". Long history of high CPU/RAM reports, especially on RDS/Citrix. The name implies the Chromium Embedded Framework, so much of the modern UI chrome is web-tech (unverified that the main UI is CEF) | [A](https://helpx.adobe.com/acrobat/kb/RdrCEF-exe-and-AcroCEF-exe-can-I-disable.html), [A](https://helpx.adobe.com/acrobat/kb/acrocef-rdrcef-errors.html) |
| Startup work | Cloud and online storage integration, Home view feeds, in-product messages ("Show me messages when I launch"), AI document analysis, plug-in loading ("Use only certified plug-ins"). Adobe staff suggest disabling online storage and GenAI to diagnose | [A](https://helpx.adobe.com/acrobat/using/viewing-pdfs-viewing-preferences.html), [3P](https://community.adobe.com/questions-9/how-to-troubleshoot-slow-pdf-opening-in-acrobat-1298515/index1.html) |
| Platform builds | Native 64-bit on Windows and Apple Silicon. Rosetta compatibility warning page exists | [A nav](https://helpx.adobe.com/acrobat/using/scan-documents-pdf.html) (unverified detail) |

**Implications for sheaf:**
- Adobe exposes its rendering trade-offs as user toggles (lookahead cache, GPU, smoothing, large images). A native Rust renderer can make them automatic with progressive tiles, a multi-page prefetch window and a GPU compositor.
- The slowness users report most comes from security sandbox startup, web-tech chrome, cloud and AI features, and resource growth in Organize Pages on large documents, not from PDF rasterization.
- A process-light design with sub-second cold start, flat memory on 500+ page workpapers with thousands of annotations, and keyboard-first display modes (Acrobat has no 1-up/2-up shortcuts and no shortcut remapping) are clear points of difference.
