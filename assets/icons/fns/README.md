# Function icons

Original SVG button artwork, on a 24 × 24 grid with 1.75-unit rounded strokes.
Each command lives at `<command>/icon.svg`. Transparent backgrounds and
`currentColor` allow the same artwork to follow normal, hover, active, and
disabled button colors. Designed for 20–32px display.

Open `preview.html` in a browser to review the set on light and dark surfaces
at 16, 20, 24, and 32px. These are assets only; the app still uses its existing labels.
When integrating, retain the command label and shortcut as a tooltip and an
accessible button name. Inline SVG inherits CSS color; an SVG loaded through
an image element does not inherit the parent element's color.

| Directory | Command | Rust action |
| --- | --- | --- |
| [select](select/icon.svg) | Select | `ToolSelect` |
| [note](note/icon.svg) | Sticky note | `ToolNote` |
| [highlight](highlight/icon.svg) | Highlight | `ToolHighlight` |
| [text](text/icon.svg) | Text | `ToolFreeText` |
| [rectangle](rectangle/icon.svg) | Rectangle | `ToolSquare` |
| [ellipse](ellipse/icon.svg) | Ellipse | `ToolEllipse` |
| [line](line/icon.svg) | Line | `ToolLine` |
| [arrow](arrow/icon.svg) | Arrow | `ToolArrow` |
| [pencil](pencil/icon.svg) | Pencil | `ToolInk` |
| [tick](tick/icon.svg) | Tickmark | `ToolTick` |
| [connector](connector/icon.svg) | Connector | `ToolConnector` |
| [rotate-right](rotate-right/icon.svg) | Rotate clockwise | `RotatePageCw` |
| [rotate-left](rotate-left/icon.svg) | Rotate counterclockwise | `RotatePageCcw` |
| [undo](undo/icon.svg) | Undo | `Undo` |
| [redo](redo/icon.svg) | Redo | `Redo` |
| [open](open/icon.svg) | Open PDF | `OpenFile` |
| [save](save/icon.svg) | Save | `Save` |
| [insert-pdf](insert-pdf/icon.svg) | Insert PDF | `InsertPdf` |
| [duplicate-pages](duplicate-pages/icon.svg) | Duplicate pages | `DuplicatePages` |
| [extract-pages](extract-pages/icon.svg) | Extract pages | `ExtractPages` |
| [delete-pages](delete-pages/icon.svg) | Delete pages | `DeletePage` |
| [find](find/icon.svg) | Find | `Find` |
| [zoom-in](zoom-in/icon.svg) | Zoom in | `ZoomIn` |
| [zoom-out](zoom-out/icon.svg) | Zoom out | `ZoomOut` |
| [fit-width](fit-width/icon.svg) | Fit width | `ZoomFitWidth` |
| [fit-page](fit-page/icon.svg) | Fit page | `ZoomFitPage` |
| [bookmark](bookmark/icon.svg) | New bookmark | `NewBookmark` |
| [flatten](flatten/icon.svg) | Export flattened copy | `ExportFlattened` |
| [export-clean](export-clean/icon.svg) | Export clean copy | `ExportCleanCopy` |

## Pattern-search alternatives

Compare these side by side at the top of `preview.html`, on light and dark surfaces.
All represent `ShowPatterns`; no default has been selected yet.

- [Repeated matches](pattern-search/rows.svg): three text rows with aligned bold matches and a magnifier.
- [Regex](pattern-search/regex.svg): a magnifier containing a path-drawn `.*` symbol (no font dependency).
- [Plain `.*`](pattern-search/asterisk.svg): a solid dot and six-spoke asterisk, with no magnifier or font dependency.
- [Needle in a haystack](pattern-search/needle-haystack.svg): a diagonal sewing needle emerging from a small pile of straw.

## Clean alternatives

Compare these at the top of `preview.html` at 16, 20, 24, and 32px on light
and dark surfaces, alongside the current floppy-disk clean export icon.
No alternative is selected as the default.

- [Eraser + sparkles](clean/eraser-sparkles.svg): removal with a clean finish.
- [Sparkles only](clean/sparkles.svg): a simple clean symbol.
- [Broom](clean/broom.svg): a cleanup metaphor.
