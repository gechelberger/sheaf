# Prior art

What other PDF tools do, what our users already know from them, and where
sheaf can be both more useful and faster. Researched 2026-10-13. Each file
links its sources inline and marks claims it could not confirm as
"(unverified)".

| File | Covers |
|---|---|
| [acrobat.md](acrobat.md) | Adobe Acrobat Pro: the full official shortcut list, feature inventory by tool, the 2023+ UI, quantified performance complaints, and Acrobat's rendering preferences |
| [pdflyer.md](pdflyer.md) | CCH ProSystem fx PDFlyer: every tool and stamp, the calculator tape format and keys, bookmark and page tools, Target Sheets, pain points, and corrections to sheaf's docs |
| [foxit.md](foxit.md) | Foxit PDF Editor: shortcuts compared with Acrobat, feature inventory with what Foxit does differently, performance claims, and its ribbon and plug-in UX |
| [other-tools.md](other-tools.md) | PDF-XChange, SumatraPDF, Okular, zathura, macOS Preview, Apryse/Xodo, PDF.js, Chrome's PDF viewer, and a table of performance techniques |
| [rust-ecosystem.md](rust-ecosystem.md) | Our reviews of gpui, gpui-pre, gpui-ce, gpui-kit, gpui-pdf, pdfium-render, hayro, lopdf, mupdf-rs, krilla, and pdfplumber ports |
| [vision-models.md](vision-models.md) | ML models for scans and photos: DocRes (dewarp, deshadow, appearance, deblur, binarize) against our current cleanup, and the layout/table/page-to-Markdown models in `oar-ocr` (researched 2026-10-14) |

The gpui-kit spike itself is in [../gpui-kit-eval.md](../gpui-kit-eval.md).

## What the prior art says about "fast"

1. **The incumbents are slow for reasons other than drawing pages.** Acrobat's
   most-reported slowness is 20–30 s launch freezes and 30+ s opens traced to
   Protected Mode / Protected View / AppContainer sandboxing, web-based UI
   chrome (AcroCEF processes that can't be disabled), cloud and AI features at
   startup, and memory growth (22 GB unattended; 90% of 32 GB organizing a
   26-page file). See [acrobat.md §3.2 and §4](acrobat.md#32-performance-complaints-with-quantified-reports).
   sheaf's advantage is mostly in *not doing* that work.
2. **Nobody publishes reproducible numbers.** Foxit's "faster and lighter"
   reputation is anecdote; its published disk requirement (7.5 GB) is larger
   than Acrobat's (4.5 GB). A public, scripted benchmark (cold start, open a
   1,000-page scan, jump to page 500, RSS) is a differentiator on its own.
   sheaf already has `--timing` and `--selftest-*` to build on.
3. **Never show a blank page.** PDF.js (low-res page + hi-res detail view),
   EmbedPDF (base layer + 768 px tiles with overlap and prefetch rings),
   Apryse (cached image, then live render) and gpui-pdf (stretched old bitmap
   during zoom) all converge on this.
4. **Make expensive renders interruptible.** Chrome drives PDFium's
   progressive API with a `NeedToPauseNow` callback so a slow page can't block
   newer, more important tiles.
5. **PDFium is one lock per process.** pdfium-render's `thread_safe` is a
   global mutex. Real parallelism means more processes (Chrome) or a second,
   thread-safe engine (hayro, later).
6. **Acrobat exposes its render trade-offs as user toggles** (one-page
   lookahead cache, 2D GPU acceleration, smooth zooming, "show large images").
   sheaf should make these automatic, not settings.

### Where sheaf stands on these techniques

| Technique | sheaf today |
|---|---|
| Tiles + LRU cache keyed by (page, zoom, tile) | Done |
| Low-res preview before full-res tiles | Done (priority 0 in the render queue) |
| Render off the UI thread | Done: one dedicated PDFium worker (`sheaf-render/src/lib.rs`) |
| Incremental save by default, rewrite on demand | Done (`sheaf-doc/src/incremental.rs`), plus crash recovery as an incremental section. Encrypted files fall back to rewrite. |
| Lazy open (xref + page tree, sizes on demand) | Partly; page sizes are fetched on the worker between renders |
| Pausable / cancellable renders (PDFium progressive API) | Not yet. Dropped receivers skip renders that haven't started, but one in progress runs to completion. |
| Thumbnail disk cache (instant first paint on reopen) | Not yet |
| Multiple render processes | Not yet; only worth it if one worker proves to be the bottleneck |
| Published, scripted benchmark vs Acrobat/Foxit | Not yet |

## What the prior art says about "useful"

For the tax and audit preparer sheaf is built for, the bar is "never open
Acrobat for binder work". In priority order:

1. **PDFlyer parity details sheaf doesn't have yet** ([pdflyer.md §6](pdflyer.md#6-corrections-and-implications-for-sheafs-readme-and-roadmap)):
   - connector *groups* (every instance of a label linked; Prev/Next, "all
     pages with/without"), alongside sheaf's pairwise links;
   - Split Bookmark; Delete Bookmark *with its pages*; Delete Page that
     re-attaches the bookmark to the next page; Hide/Show empty bookmarks;
     colorize by populated vs empty;
   - Insert PDF modes (merge by parent bookmarks / append as one bookmark at a
     chosen level / one bookmark per page / no bookmark; offer to delete the
     source file);
   - **Set Status** (bookmark color + page border + text/date stamp), the
     real PDFlyer analog of "sign-off";
   - Stamp Summary (page, author, time; export) and Note Summary;
   - Recent and Favorite stamps, a sticky-stamp mode, user stamps from images
     with export/import for sharing across staff;
   - the 10-key tape's section/row format (`1-0` title, `1-1`…, `1-T`, `GT`),
     re-editable after placing, rotatable and resizable;
   - multi-page TIFF export with annotations burned in.
   Unlike PDFlyer, sheaf has undo for all of these, which PDFlyer's delete
   tools lack.
2. **Reading existing PDFlyer workpapers.** How PDFlyer stores stamps,
   connectors and editable tapes in the PDF is undocumented. A PDFlyer-marked
   sample file is the single most valuable thing to get; without one, sheaf
   can't round-trip a firm's existing binders.
3. **Acrobat features workpaper users lean on** ([acrobat.md §2](acrobat.md#2-feature-inventory)):
   comment list filtering by reviewer/type/status/color and export;
   bookmark from selected text (the selection becomes the label); Bates
   numbering and headers/footers; search-and-redact with patterns plus
   sanitize; Compare Files; four-pane spreadsheet split with frozen headers;
   measurement with CSV export; batch actions.
4. **Foxit's review-oriented extras** ([foxit.md §3](foxit.md#3-feature-inventory)):
   sequential stamps, audit tickmarks, an accounting calculator with the tape
   placed on the page, comment summaries in several layouts or CSV, and
   Compare with table detection. Foxit has clearly also studied PDFlyer's
   users.
5. **Power-user ergonomics** ([other-tools.md §4](other-tools.md#4-top-ux-steals-summary)):
   - one command registry behind menus, palette, keymap and toolbar (sheaf
     has this) with a conflict-aware rebinding UI (PDF-XChange) and
     hot-reloaded keymap (SumatraPDF);
   - palette prefix modes (`>` commands, `#` recent files, `@` windows;
     SumatraPDF);
   - hold Space for a temporary Hand tool; double-click a tool to lock it
     (Okular, PDF-XChange, PDFlyer's Keep Stamp Selected);
   - thumbnails as the page-ops surface, including **dragging pages between
     windows** (Preview); this matters more now that sheaf is one document
     per window;
   - shortcut docs generated from the keymap and linted for collisions in CI
     (Foxit's docs and maps disagree with each other).

### Things to avoid

- Two parallel UIs (Acrobat's "new" vs classic, Foxit's Classic vs Ribbon).
- Feature gating by edition (Foxit's "Pro only" labels; Acrobat Standard vs Pro).
- A floating toolbar that can't be docked (Acrobat's top complaint about its
  new UI). sheaf's planned toolbar ([`todos/toolbar.md`](../../todos/toolbar.md))
  docks, and its layout is plain shareable text, like Foxit's XML export.
- A plug-in that breaks on every host upgrade (PDFlyer's main pain point).
  sheaf's plugins are WASM components against our own WIT interface
  (`wit/sheaf.wit`), not the host's internals.
- Destructive operations with no undo (PDFlyer Delete Page/Bookmark).

## Keyboard map: sheaf vs Acrobat vs Foxit

Users arrive with Acrobat muscle memory, and Foxit copied Acrobat's
single-key scheme almost exactly to ease switching. sheaf's bindings are in
`crates/sheaf-app/src/main.rs`; users can override them in `keymap.json`.

Acrobat's single-letter tool keys only work with "Use single-key accelerators
to access tools" turned on (off by default). sheaf's single keys are always
on while the document has focus.

### Where sheaf matches

| Action | sheaf | Acrobat | Foxit (Win) |
|---|---|---|---|
| Next / previous page | Right / Left | Right / Left | same |
| Next / previous screen | Space, PgDn / Shift+Space, PgUp | PgDn, Enter / PgUp, Shift+Enter | same |
| First / last page | Home / End | Home / End | same |
| Zoom in / out | Ctrl+= / Ctrl+- | Ctrl+= / Ctrl+- | Ctrl++ / Ctrl+- |
| Fit page / actual size / fit width | Ctrl+0 / Ctrl+1 / Ctrl+2 | Ctrl+0 / Ctrl+1 / Ctrl+2 (third-party sources) | Ctrl+0 / Ctrl+1 / Ctrl+2 (Mac differs) |
| Find; next / previous | Ctrl+F; F3 / Shift+F3 | Ctrl+F; F3 or Ctrl+G | Ctrl+F; F3 / Shift+F3 |
| Insert pages from file | Ctrl+Shift+I | Ctrl+Shift+I | Ctrl+Shift+I |
| New bookmark | Ctrl+B | Ctrl+B | Ctrl+B |
| Toggle navigation pane | F4 | F4 (classic UI); Ctrl+Shift+F5 (modern) | Shift+F4 (task pane) |
| Select tool | V | V | V |
| Sticky note | S | S | S |
| Highlight | U | U | U |
| Drawing / shape | D (rectangle) | D (current drawing tool) | D |
| Stamp / tickmark | K (tickmark) | K | K |
| Open / Save / Save As / Close / Quit / Undo / Redo | standard | standard | standard |

### Where sheaf conflicts

| Key | sheaf | Acrobat | Foxit | Suggestion |
|---|---|---|---|---|
| **Ctrl+D** | Duplicate pages | Document Properties | Document Properties (Duplicate is Ctrl+Alt+D) | Move Duplicate to Ctrl+Alt+D (Foxit's); Ctrl+D → Document Properties when that exists |
| **Ctrl+Shift+T** | Recognize Text (OCR) | Insert blank page | Insert blank page | Pick another OCR key; keep Ctrl+Shift+T for blank page (planned M8) |
| **Delete** | Delete page | Delete pages is Ctrl+Shift+D | Delete pages is Ctrl+Shift+D | Keep Delete in thumbnails; add Ctrl+Shift+D everywhere |
| **R / Shift+R** | Rotate page CW / CCW | R = Select Object | R = Select Annotation | Keep (page rotation is a constant workpaper action), but add Acrobat's Ctrl+Shift+R |
| **T** | Free text | Edit text | Edit text | X is the text box in both; bind X too, and give T to Edit Text when M13 lands |
| **O** | Ellipse | Edit Object | Edit Object | Low priority until object editing exists |
| **L** | Line | Link | Link | Same |
| **A** | Arrow | Enter/exit forms editing | — | Revisit at M10 (forms) |
| **Ctrl+Shift+K** | Command palette | — (Ctrl+K is Preferences) | Alt+Q is command search | Fine; consider Ctrl+K only if Preferences gets another key |

### Gaps: common bindings sheaf doesn't have

| Action | Acrobat | Foxit | Note |
|---|---|---|---|
| Go to page | Ctrl+Shift+N | Ctrl+G | sheaf has no go-to-page command yet. Bind both; F3 already covers find-next, so Ctrl+G is free. |
| Full screen | Ctrl+L | F11 | No full-screen mode yet. Bind both. |
| Read mode (hide chrome) | Ctrl+H (community sources) | Ctrl+H (unverified); Ctrl+F11 collapses the ribbon | |
| Previous / next view (history) | Alt+Left / Alt+Right (unverified; the menu items are documented) | not documented | No view history yet. Connectors make this important. |
| Rotate view (not saved) | Ctrl+Shift+Plus / Minus | same | Rotate view is planned (M7). |
| Single / two-page / continuous modes | **none** (users complain) | none documented | Easy win: give them keys. |
| Delete pages / insert blank page | Ctrl+Shift+D / Ctrl+Shift+T | same | |
| Temporary Hand (hold Space) | Space | Space | sheaf uses Space for Screen Down; holding could still mean pan. |
| Hand tool | H | H | |
| Text box | X | X | |
| Cloud | Q | Q | Not a sheaf tool yet. |
| Search & Replace | — | Ctrl+T | For M13 content editing. |

PDFlyer itself documents no hotkeys outside its calculators (it relies on
Acrobat's Quick Tools bar). Its calculator keys are in
[pdflyer.md §3](pdflyer.md#3-keyboard-shortcuts) and are worth matching in
sheaf's Calculator panel: `Space` subtotal (◊), `Enter` total, `Esc` clear,
`C` clear entry, `S` change sign, `D` decimals, `T` timestamp into the note
column, `Insert`/`Delete` add/delete line, `G H J K L` for MC MS MR M− M+.

## Corrections to sheaf's own docs

From [pdflyer.md §6](pdflyer.md#6-corrections-and-implications-for-sheafs-readme-and-roadmap).
Applied to README.md and ROADMAP.md on 2026-10-14. The feature gaps from
the same section are under ROADMAP.md
[Pending](../../ROADMAP.md#pdflyer-parity-gaps).

| sheaf currently says | Finding |
|---|---|
| README: PDFlyer "is locked to 32-bit Acrobat on Windows" | Outdated. PDFlyer v20 (Dec 2023) supports 64-bit Acrobat. It is still Windows-only and needs a paid Acrobat Standard/Pro; v22 (Dec 2025) dropped Windows 10. |
| ROADMAP M9: calculator tape "picks up numbers by clicking page text (PDFlyer)" | PDFlyer has no such feature; numbers are typed. Keep it, labeled as a sheaf extension. |
| ROADMAP M9: "Sign-off / review stamps (preparer and reviewer initials + date) (PDFlyer)" | Not PDFlyer. The nearest is **Set Status**. Real sign-off lives at binder level in ProSystem fx Engagement. |
| README: connectors "saved as Link annotations" | PDFlyer connectors are label *groups* (1–50, A–ZZ) with Prev/Next and "all pages with/without" navigation, not pairwise links. Consider supporting both. |
| README: "Colorize bookmarks by level / status" | PDFlyer colors by populated vs empty (parent and child), and separately by status. |

## Open questions

1. How PDFlyer stores stamps, connectors, tapes, author and timestamp in the
   PDF. Needs a sample file. (Highest value.)
2. Whether PDFlyer stamps navigate in plain Acrobat without the plug-in.
3. Whether pdfium-render exposes PDFium's progressive render API, or we need
   raw FFI for pausable tiles.
4. Acrobat's Ctrl+0/1/2 zoom keys and Ctrl+H read mode are confirmed only by
   community sources; Adobe's own page doesn't list them.
5. Whether hayro is fast enough on large scans to serve as a parallel tile
   renderer. Nobody has published numbers.
