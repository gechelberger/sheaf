# Function icons

Original SVG button artwork, on a 24 × 24 grid with 1.75-unit rounded strokes.
Each command lives at `<command>/icon.svg`. Transparent backgrounds and
`currentColor` allow the same artwork to follow normal, hover, active, and
disabled button colors. Designed for 20–32px display.

Open `preview.html` in a browser to review the set on light and dark surfaces
at 16, 20, 24, and 32px. Every built-in command names its icon in
`crates/sheaf-app/src/command_info.rs`, and `crates/sheaf-app/src/assets.rs`
embeds them all; a new command needs both (a test checks). Plugins ship their
own in the same style (see `docs/plugins.md`).
Keep the command label and shortcut as a tooltip and an accessible button name. Inline SVG inherits CSS color; an SVG loaded through
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
| [insert-blank](insert-blank/icon.svg) | Insert blank pages | `InsertBlankPages` |
| [insert-image](insert-image/icon.svg) | Insert pages from images | `InsertFromImages` |
| [replace-pages](replace-pages/icon.svg) | Replace pages | `ReplacePages` |
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
| [about](about/icon.svg) | About sheaf | `About` |
| [quit](quit/icon.svg) | Quit | `Quit` |
| [new-window](new-window/icon.svg) | New window | `NewWindow` |
| [save-as](save-as/icon.svg) | Save as | `SaveAs` |
| [close-document](close-document/icon.svg) | Close document | `CloseDocument` |
| [close-window](close-window/icon.svg) | Close window | `CloseWindow` |
| [select-all-pages](select-all-pages/icon.svg) | Select all pages | `SelectAllPages` |
| [actual-size](actual-size/icon.svg) | Actual size | `ZoomActual` |
| [first-page](first-page/icon.svg) | First page | `FirstPage` |
| [prev-page](prev-page/icon.svg) | Previous page | `PrevPage` |
| [next-page](next-page/icon.svg) | Next page | `NextPage` |
| [last-page](last-page/icon.svg) | Last page | `LastPage` |
| [thumbnails](thumbnails/icon.svg) | Thumbnails | `ShowThumbnails` |
| [outline](outline/icon.svg) | Bookmarks | `ShowOutline` |
| [calculator](calculator/icon.svg) | Calculator | `ShowCalc` |
| [comments](comments/icon.svg) | Comments | `ShowComments` |
| [command-palette](command-palette/icon.svg) | Command palette | `CommandPalette` |
| [navigation-panes](navigation-panes/icon.svg) | Navigation panes | `ToggleSidebar` |
| [left-dock](left-dock/icon.svg) | Left dock | `ToggleLeftDock` |
| [right-dock](right-dock/icon.svg) | Right dock | `ToggleRightDock` |
| [bottom-dock](bottom-dock/icon.svg) | Bottom dock | `ToggleBottomDock` |
| [recognize-text](recognize-text/icon.svg) | Recognize text (OCR) | `RecognizeText` |
| [cancel-ocr](cancel-ocr/icon.svg) | Cancel OCR | `CancelOcr` |
| [manage-plugins](manage-plugins/icon.svg) | Manage plugins | `ManagePlugins` |
| [combine-files](combine-files/icon.svg) | Combine files | `CombineFiles` |
| [split-document](split-document/icon.svg) | Split document | `SplitDocument` |
| [crop-pages](crop-pages/icon.svg) | Crop pages | `CropPages` |
| [page-labels](page-labels/icon.svg) | Page labels | `PageLabels` |
| [header-footer](header-footer/icon.svg) | Header & footer | `HeaderFooter` |
| [bates](bates/icon.svg) | Bates numbering | `BatesNumbering` |
| [watermark](watermark/icon.svg) | Watermark | `AddWatermark` |
| [background](background/icon.svg) | Background | `AddBackground` |
| [create-from-scans](create-from-scans/icon.svg) | Create from scans/photos | `CreateFromScans` |
| [export-pages](export-pages/icon.svg) | Export pages | `ExportPages` |
| [print](print/icon.svg) | Print | `Print` |
| [properties](properties/icon.svg) | Document properties | `DocumentProperties` |
| [clear-recent](clear-recent/icon.svg) | Clear recent files | `ClearRecent` |
| [pattern-search](pattern-search/icon.svg) | Pattern search | `ShowPatterns` |
| [single-page](single-page/icon.svg) | Single page | `SinglePage` |
| [single-page-continuous](single-page-continuous/icon.svg) | Single page continuous | `SinglePageContinuous` |
| [two-up](two-up/icon.svg) | Two-up | `TwoUp` |
| [two-up-continuous](two-up-continuous/icon.svg) | Two-up continuous | `TwoUpContinuous` |
| [cover-page](cover-page/icon.svg) | Cover page in two-up | `ToggleCoverPage` |
| [rotate-view-right](rotate-view-right/icon.svg) | Rotate view clockwise | `RotateViewCw` |
| [rotate-view-left](rotate-view-left/icon.svg) | Rotate view counterclockwise | `RotateViewCcw` |
| [split](split/icon.svg) | Split view | `SplitView` |
| [read-mode](read-mode/icon.svg) | Read mode | `ReadMode` |
| [full-screen](full-screen/icon.svg) | Full screen | `FullScreen` |
| [rulers](rulers/icon.svg) | Rulers | `ToggleRulers` |
| [grid](grid/icon.svg) | Grid | `ToggleGrid` |
| [clear-guides](clear-guides/icon.svg) | Clear guides | `ClearGuides` |
| [go-to-page](go-to-page/icon.svg) | Go to page | `GoToPage` |
| [attachments](attachments/icon.svg) | Attachments | `ShowAttachments` |
| [layers](layers/icon.svg) | Layers | `ShowLayers` |
| [toolbar](toolbar/icon.svg) | Toolbar | `ToggleToolbar` |
| [customize-toolbar](customize-toolbar/icon.svg) | Customize toolbar | `CustomizeToolbar` |

## Pattern-search alternatives

Compare these side by side at the top of `preview.html`, on light and dark surfaces.
All represent `ShowPatterns`. [`pattern-search/icon.svg`](pattern-search/icon.svg)
is a copy of the regex variant, in use until one is chosen.

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
