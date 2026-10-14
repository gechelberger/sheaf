# Move reusable widgets into sheaf-ui

Status: in progress on branch `widgets` (from main). A separate PR from the plugin work.

`crates/sheaf-ui` exists but is an empty stub ("Empty until M1"). The
README gives it the page canvas, tiled viewport, sidebars and command
palette. Below is what in `sheaf-app` is reusable today, based on each
module's `crate::` imports.

## Ready to move (no app dependencies)
- `text_input.rs`, `text_area.rs`: single- and multi-line text fields.
  Their key bindings live in `main.rs` `bind_keys`, so sheaf-ui needs to
  export a `bind_keys(cx)` (and keep the `TextInput`/`TextArea` key
  contexts the app's exclusions use, e.g. `"DocView && !TextInput"`).
- `prompts.rs`: the per-window prompt queue (`ask`) and sheaf's prompt
  view (`install`, set as GPUI's prompt builder).
- `frame.rs`: the title bar and client-side decorations. Only uses
  `brand::mark`; take the icon as a parameter.
- `tooltip.rs`: only uses `menu_bar::shortcut`, so it moves with the
  menu bar.

## Need a small seam first
- `menu_bar.rs`: uses `menus::{mnemonic, parse_label}` (`&File`-style
  labels). Move those helpers with it; the menu definitions stay in the
  app.
- `palette.rs`: uses `CommandRegistry`, `menus::parse_label` and the app's
  `Palette*` actions. Make the widget take a list of commands and an
  on-run callback; the fuzzy `score` is pure and moves as is.

## Repeated code to turn into shared components
- **Dialog shell** (dimmed backdrop, card, header, footer buttons):
  written four times, in `plugin_form`, `plugin_manager`, `ocr_dialog`
  and `prompts`.
- **Chips and buttons**: `ocr_dialog::chip` and `plugin_form::chip` are
  near copies. There are three button styles (`plugin_manager::button`,
  the form's, the prompt's).
- **Virtualized lists** (scrolling lists that draw only visible rows):
  Comments, Bookmarks, Patterns and the thumbnails each hand-roll
  `tops` + `partition_point`. Comments now also has rows of different
  heights, with the text wrapped and measured.
- **Colors**: there are about 275 `rgb(0x…)` literals and no named
  color set. Do this first, so moved widgets don't carry their own hex
  values and a later theme change doesn't touch everything again.

## Stays in the app
`docview`, `markup_tools`, `region`, the document panels (outline,
comments, pattern, calc; they render through `DocView`) and the plugin
UI. The page canvas and tile viewport the README assigns to sheaf-ui
live inside `docview.rs` (about 4,100 lines): a much bigger extraction,
for later.

## Relation to gpui-kit
The gpui-kit spike (`todos/eval-gpui-kit.md`, `docs/gpui-kit-spike.md` on
`spike/gpui-kit`) concluded "don't adopt; borrow selectively". sheaf-ui
is where borrowed pieces would go (e.g. undo and word movement for the
text fields).

## Suggested order (one commit each)
1. Named colors (a theme module).
2. Move the ready modules: text fields, prompts, tooltip, frame, menu bar.
3. Shared dialog shell, buttons and chips; replace the four dialog
   copies.
4. A generic palette.
5. A virtualized list (fixed and variable row heights).

Steps 1–2 are mostly mechanical. Steps 3–5 change existing screens, so
check them with screenshots (the `--script` UI harness).

## Progress
1. Done. `sheaf_ui::theme` names every UI color by role (`ACCENT`,
   `HOVER`, `CONTROL`, `TEXT_MUTED`, `DIVIDER`, ...); the app has no
   `rgb(0x…)` literals left, only colors computed from document data.
   Values are unchanged except two near-duplicates folded together:
   `0x5aaaff` into `ACCENT_HOVER` (`0x5aaeff`) and `0x555555` into
   `CONTROL_HOVER_STRONG` (`0x5a5a5a`).
