# gpui-kit evaluation: results

Code: branch `spike/gpui-kit` (not merged), cut from `main` at `e324532`; file paths below such as
`kit.rs` and `kit_tests.rs` refer to that branch. Plan: `todos/eval-gpui-kit.md`.

**Recommendation: don't adopt gpui-kit as a dependency; borrow selectively.** See the end of this document.
Machine: Linux (GNOME/Wayland, 32 cores, GTX 1080 Ti). gpui-kit 0.7.3, gpui-pre 0.3.9.

## Numbers

Clean build is `cargo build --release -p sheaf-app` with a fresh target directory.
Incremental is a rebuild after touching `crates/sheaf-app/src/main.rs` (3 runs).
Startup is the median of 7 runs of `sheaf --timing qoi-specification.pdf` (1 page),
measured as each step landed; see *Startup* below for an interleaved re-run.

| Step | Clean release | Peak RSS | Incr. release | Incr. dev | Binary | Cargo.lock pkgs | `cargo tree` crates | Open / first pixels | sheaf-app LOC (+/−) |
|---|---|---|---|---|---|---|---|---|---|
| baseline | 210 s | 3.1 GB | 48 s | 2.2 s | 36.65 MB | 828 | 569 | 215 / 224 ms | 13,470 |
| 1. gpui-pre | 211 s | 3.0 GB | 48 s | 2.2 s | 36.63 MB | 837 | 569 | 218 / 236 ms | 0 |
| 2. kit setup | 296 s | 3.9 GB | 51 s | 2.5 s | 42.49 MB | 934 | 635 | 217 / 227 ms | 13,405 (+145 −210) |
| 3. text fields | 305 s | 3.9 GB | 59 s | 2.3 s | 45.14 MB | 952¹ | 635 | 223 / 234 ms | 12,365 (+146 −1,374, +188 tests) |
| 4. palette | 305 s | 3.9 GB | 59 s | 2.2 s | 45.24 MB | 952¹ | 635 | 221 / 232 ms | 12,407 (+107 −133, +68 tests) |
| 5. window chrome | 302 s | 3.8 GB | 59 s | 2.3 s | 45.37 MB | 952¹ | 635 | 218 / 232 ms | 12,309 (+32 −130) |
| 6. OCR dialog | 305 s | 3.9 GB | 61 s | 2.3 s | 45.56 MB | 952¹ | 635 | see *Startup* | 12,418 (+184 −186, +111 tests) |

LOC counts every `.rs` file in `crates/sheaf-app/src`, including the new
`kit_tests.rs` (367 lines) from step 3 on.

¹ Includes dev-only packages (proptest etc.) pulled in by gpui's `test-support`, added for the headless tests.

**Overall (baseline → step 6):** clean release build +95 s (+45%), peak
RSS +0.8 GB, incremental release build +13 s (+27%), incremental dev build
unchanged, binary +8.9 MB (+24%), +66 crates in the Linux build graph, and
startup unchanged. sheaf-app shrinks by about 1,420 lines, not counting tests:

| File | Before | After |
|---|---|---|
| text_input.rs | 675 | 133 |
| text_area.rs | 779 | 129 |
| palette.rs | 305 | 277 |
| frame.rs | 335 | 60 |
| ocr_dialog.rs | 622 | 590 |
| kit.rs (theme) | — | 72 |

**Startup.** The display setup changed during the session (windows began
opening maximized on a 2048×1536 output), so the per-step numbers above
aren't comparable with each other. An interleaved re-run (2 rounds × 5 runs
per build, same conditions) puts every build at 260–300 ms open and
283–317 ms to first pixels, with no trend. gpui-kit has no measurable
startup cost.

## Step notes

### 1. Switch to gpui-pre

No source changes; it is the same Zed commit. Lockfile differences beyond the
`gpui_*` → `gpui-pre-*` renames:

- `dirs` 6.0.0 → 5.0.1 (with `dirs-sys`/`redox_users`): gpui-pre's published
  manifests depend on dirs 5; Zed's workspace uses 6.
- a second `windows-sys` 0.48 / `windows-targets` 0.48 tree (9 packages,
  Windows only, pulled in by dirs 5). This is the whole +9 in the lockfile.
- `zed-xim` comes from crates.io (0.4.0-zed) instead of Zed's git rev.
- Our `[patch.crates-io]` block still applies (async-task, async-process,
  calloop, khronos-egl, windows-capture resolve to Zed's git forks).

The 19 Zed crates map one-to-one onto 19 `gpui-pre-*` crates, so the Linux
build graph is the same size.

### 2. gpui-kit setup

- `gpui_kit::init` plus a theme mapped onto sheaf's literal palette
  (`kit.rs`, ~45 color assignments). The
  palette has no names, so it is the ten most frequent `rgb(...)` values.
- Windows open through `gpui_kit::open_window`, which makes kit's `Root`
  the window root. Every `WindowHandle<Workspace>` and
  `downcast::<Workspace>()` broke (7 places in main.rs/workspace.rs);
  replaced by a `WorkspaceWindow { handle, ws }` pair that finds the
  workspace through `Root::view()`.
- **kit's Root always draws its own client-side frame.** The component
  plugin's `decorate` hook wraps every window in `window_border()`; there is
  no option to turn it off short of not using kit's Root. Ours would draw a
  second frame inside it, so `frame::client_side_decorations` (≈170 lines)
  was deleted here rather than in step 5. Visible differences: square
  corners (kit keeps `BORDER_RADIUS = 0` because GPUI can't clip children to
  rounded masks) and a 20 px shadow band instead of 10 px.
- The Root also sets the window rem size from `Theme::font_size` and paints
  the background/foreground/font family. With `font_size = 16px` nothing
  moved.
- Pulled in with only `component` + `assets`: lsp-types, html5ever,
  markdown, ropey, notify, rust-i18n, rust-embed, a reqwest fork and more
  (63 new packages, +66 crates in the Linux graph). gpui-component has no
  finer feature gates. This is the bulk of the +85 s clean build and +5.8 MB.

Key-binding check against `bind_keys` and `keymap.json`:

- Root context binds `tab` / `shift-tab` to focus-next/prev. sheaf left Tab
  unbound outside the OCR dialog, so Tab now moves keyboard focus between
  focusable elements everywhere. Harmless, but new behavior.
- Root binds `ctrl-c` to copy kit text selection, propagating when there is
  none; our deeper `TextInput`/`TextArea` bindings still win.
- Debug builds only: kit binds `ctrl-shift-i` globally to its inspector.
  Ours is `InsertPdf` in DocView, which wins when the document has focus; in
  a panel it opens the inspector instead. Release builds are unaffected.
- No other kit binding is global; all are scoped to kit contexts (`Input`,
  `PopupMenu`, `Dialog`, `Tree`, `Command`, ...). User `keymap.json` entries
  name sheaf contexts and are unaffected.

### 3. Text fields

`text_input.rs` (675 lines) and `text_area.rs` (779) are now ~130-line
adapters over kit's `InputState` / `TextareaState` that keep sheaf's API
(`new`, `text`, `set_text`, `select_all`, `masked`, `rows`, `Confirm`/
`Cancel` events). No call site changed. The 36 text-field key bindings in
`main.rs` are gone, and the exclusions are now `!Input`.

What the adapter has to work around:

- **Enter and Escape propagate out of kit's single-line input**, and GPUI
  then dispatches the next matching binding. In the find bar, Enter would
  emit `PressEnter` *and* fire DocView's `SearchNext`; Escape would also run
  `CloseSearch`. The adapter handles `Enter`/`Escape` on a wrapper div and
  stops them there. Removing that handler makes `kit_tests::find_field_keeps_its_keys`
  fail with `["SearchNext"]`, so the test covers it.
- **Ctrl+Enter in kit's textarea inserts a newline** and stops; there is no
  "secondary Enter submits" mode, only `submit_on_enter`, which makes plain
  Enter submit. The adapter takes `Enter { secondary: true }` in the
  capture phase.
- **Kit needs `&mut Window` to create a field and to set its text.**
  In sheaf, about 13 functions create or fill a field without a window: the
  panels' `ensure_*`/`calc_apply`/`calc_total`/`focus_comments`/
  `run_initial_pattern`/`start_pattern_search`, a `cx.observe` callback in
  the calculator, and the async document-open path (`open` → `lock`,
  `install` → `new_search`). Threading `window` through would have meant
  `spawn_in` for the async path and `observe_in` for the callback. Instead
  the adapter creates the kit state on first render and queues `set_text`
  until the next render. It keeps its own focus handle and forwards focus
  to the kit field; `on_focus_out` on the adapter's handle still covers the
  kit field, since that is a descendant.
- `value()` returns an owned `SharedString` rebuilt from the rope, so the
  adapter mirrors the text on every `InputEvent::Change` to keep
  `text() -> &str`.

Behavior changes that come with kit's input:

- New: undo/redo inside fields (Ctrl+Z previously undid the *document* edit
  while typing in a field), word movement, Home/End selection, a right-click
  menu, and Tab indents inside the note editor.
- User `keymap.json` files that exclude `TextInput`/`TextArea` stop working
  silently (the contexts no longer exist); README and keymap docs updated to
  `!Input`.

Headless tests (`kit_tests.rs`, `#[gpui::test]`, 4 tests, all pass):
single-key shortcuts and space type into the find field; Enter/Escape
confirm/cancel exactly once with no DocView fallthrough; Ctrl+Enter confirms
the note without a newline and Enter inserts one; up in the palette field
reaches `PaletteUp`, Tab in the OCR range field reaches `DialogNext`;
password field accepts input; `set_text` before first render selects all.
IME preedit isn't covered headlessly.

### 4. Command palette

`palette.rs` now renders kit's `Command` (search field, virtual list,
Up/Down/Enter/Escape, mouse) inside our existing backdrop. `commands(cx)`
and the workspace's `PaletteEvent::Run` → focus doc → `dispatch_action`
path are unchanged. `PaletteUp`/`PaletteDown` and their bindings are gone.

Gaps:

- **Kit's filter is a case-insensitive substring match with no ranking.**
  "fw" finds nothing, where ours ranks "View: Fit Width" first. Kept ours:
  `.filterable(false)`, re-filter in `on_query`, and supply the items in
  ranked order. That works, but it is the whole point of a palette.
- **Kit dispatches the item's action from the palette's own focus.** The
  palette is not inside the document view, so DocView actions (zoom, rotate,
  save, ...) would silently do nothing, and Workspace actions would run
  twice if we also ran them. Rows carry no action; `on_confirm` emits
  `Run` as before. Without an action, kit shows no shortcut hint, so rows
  use a custom `child` (category, label, action name, shortcut), which is
  our old row markup.
- Custom row content makes kit treat the model as changed on every render
  (`same_layout` is false for custom children), so it re-matches and
  re-measures all ~90 rows per render. Not noticeable at this size.
- No PageUp/PageDown in kit's list; ours remain (`set_selected_index`).
- Escape now clears a non-empty query first and dismisses on the second
  press (kit behavior; previously one press dismissed).
- Rows are ~34 px with kit's padding (ours were 30 px).

Headless test `palette_ranks_runs_and_dismisses`: "fw" + Enter runs
`docview::ZoomFitWidth`; PageDown + Enter runs a different command; Escape
twice dismisses. A spike-only `--palette` flag opens the palette at startup
for screenshots.

### 5. Window chrome

- `frame::title_bar` is now kit's `TitleBar` with our brand mark, app name,
  `MenuBar` and centered document title as children (frame.rs 175 → 63
  lines). The client-side frame was already kit's since step 2.
- **Kept our `MenuBar`.** Kit's `AppMenuBar` has no Alt-tap/F10 activation
  and no `&` mnemonics (it binds only Escape/Left/Right in its own context).
  It sits in an `occlude()`d div so presses on menus don't start a window
  drag (kit arms the drag on mouse-down anywhere in the bar) and, on
  Windows, aren't swallowed by the bar's `WindowControlArea::Drag`.
- Close button: Linux calls `on_close_window` → `Workspace::request_close`
  (unsaved-changes prompt). Windows uses `WindowControlArea::Close`, i.e.
  WM_CLOSE → our `on_window_should_close`, same as before.
- Linux drag starts on the first mouse move after a press (ours started on
  press), double-click zooms, right-click opens the window menu only under
  client-side decorations. `app_owns_titlebar_drag: true` added per
  `TitleBar::window_options()`.
- **Missed in step 2: kit's icons need its asset source.** Without
  `application().with_assets(gpui_kit::assets::Assets::new(""))` every kit
  icon (window controls, the palette's search glyph, input clear buttons)
  silently renders nothing. Nothing warns; it only shows up visually.
  sheaf had no asset source of its own, so kit's is the only one; an app
  that has one needs a combining `AssetSource`.
- Visual: same 34 px height and layout; kit's control glyphs are thinner
  SVGs and its background is a subtle vertical gradient.

Pre-existing bug found while screenshotting (also on `main` @ `e324532`,
X11 and Wayland): `sheaf --menu f` panics in GPUI
`key_dispatch.rs:384` ("index out of bounds: the len is 0") — the menu is
opened before the window has rendered a dispatch tree. Unrelated to kit.

### 6. OCR dialog

- The backdrop and card are now a kit `Dialog` opened with
  `window.open_dialog` (title, close button, overlay, focus trap, footer
  with kit `Button`s). The ☑/☐ text checkbox is a kit `Checkbox`; the
  scope, engine and language chips are kit `RadioGroup`s (horizontal,
  wrapping). The workspace no longer renders the dialog itself.
- **Kept our keyboard row navigation.** Kit's radio group has no arrow-key
  movement, and every radio and checkbox is its own Tab stop (about 12 Tab
  stops here against our 7 rows). Kit widgets are set `tab_stop(false)`, and our
  `OcrDialog` context keeps Tab/Shift-Tab/arrows/Space/Enter/Escape. That
  context is deeper than kit's `Dialog` (Enter=Confirm, Escape=Cancel) and
  `Root` (Tab), so ours win, and `keyboard(false)` stops kit's Escape.
- **`open_dialog` moves focus to kit's own dialog handle**, below which none
  of our row bindings apply; the first key press would go nowhere. `show()`
  re-focuses our content right after opening. The headless test
  `ocr_dialog_rows_work_inside_kit_dialog` fails without that line.
- Kit closes the dialog itself on backdrop click and the × button, so
  "dialog open" now lives in two places (kit's dialog stack and
  `Workspace::ocr_dialog`), kept in sync through `on_close` and
  `close_dialog`. The palette avoided this by keeping our own backdrop.
- A long language list (Tesseract can have dozens) wraps as radios; kit's
  `Select` would suit it better but needs its own state entity and window
  plumbing. Not done in the time box.
- Spike-only `--ocr-dialog` flag opens the dialog 1.5 s after launch for
  screenshots.

Headless test: Escape dismisses on open; Right, Tab, Tab, Space, Enter runs
"Current page" with skip toggled; typing `2-3` in the range row and pressing
Enter runs pages `[1, 2]`.

### 7. Outline tree (stretch): stopped

Kit's `Tree` (`gpui_base::tree`, 715 lines) has no drag-and-drop:
nothing in `TreeState`/`Tree` handles drag, drop or reordering. Rows are a
`uniform_list` with keyboard selection, expand/collapse and a
`context_menu` hook, and items are `TreeItem { id: SharedString, label }`
with no payload. Bookmark drag-to-reorder (outline_panel.rs:540+) is the
point of that panel, so per the plan this stops here and is recorded as a
gap. Reordering could be added through the custom `item` renderer
(`on_drag`/`on_drop` on each row), but then the virtual scrolling would be
the only part kit provides. That isn't worth the move.

## Assessed on paper

- **Workspace docks, tabs and splitters → kit `dock`.** Kit's `DockArea`
  owns the layout model (a tree of tab groups and splits; each panel
  implements kit's `Panel` trait and serializes through `dump` →
  `PanelState`). sheaf's
  workspace keeps three fixed docks with a tab strip each, plus document
  tabs and our own `DockLayout` persistence in `settings.json`. Moving means
  rewriting workspace.rs around kit's model and migrating saved layouts.
  It would buy user-arrangeable panels, which is not on the roadmap. Not
  recommended.
- **calc_panel's button grid.** It is a fixed grid of plain buttons with
  custom keyboard handling. Kit `Button`s would restyle it, about 20 lines
  either way. No gain.
- **docview, markup_tools, sheaf-render.** These draw the page, selections and
  annotations with custom elements and stay ours under any plan. Kit sits
  beside them; the only contact points are the find bar, password field
  and note editor (step 3).

## Behavior checklist

GNOME 50 on Wayland, plus X11 via Xwayland. No Windows machine was available.
Synthetic input on this desktop has to go through the remote-desktop portal
(Xwayland runs with `-enable-ei-portal`), which prompts the user, so
interactive checks were done as headless `#[gpui::test]`s against the real
widgets and bindings. Rendering was checked by screenshots of the running
app under X11.

| Check | Wayland | X11 | Windows | How |
|---|---|---|---|---|
| IME input | untested | untested | n/a | Typed text goes through kit's `EntityInputHandler` (`kit_tests`); preedit/composition not exercised |
| Password masking | partial | partial | n/a | field is `masked(true)` and accepts input (test); screenshot shows the empty field only |
| Ctrl+Enter in note editor | pass | pass | n/a | `note_editor_ctrl_enter_confirms` (needed a capture-phase workaround) |
| Single-key shortcuts don't leak into fields | pass | pass | n/a | `find_field_keeps_its_keys` (`v`, space typed; Enter/Escape don't fall through) |
| Alt/F10 menu bar | not run | not run | n/a | our `MenuBar`, unchanged; `--menu` panics on `main` too (see step 5) |
| Command palette | pass | pass (screenshot) | n/a | `palette_ranks_runs_and_dismisses` + screenshot |
| Title bar: Snap Layouts | n/a | n/a | not run | kit uses the same `WindowControlArea` Min/Max/Close/Drag as ours |
| Title bar: drag, double-click zoom, edge resize | not run | not run | n/a | needs pointer input; kit code reviewed (step 5) |
| OCR dialog keyboard navigation | pass | pass | n/a | `ocr_dialog_rows_work_inside_kit_dialog` (needed a focus workaround) |
| `--selftest-*` (save, insert, annot, clean, flatten, extract, ocr) | pass | — | n/a | identical output sizes to baseline; OCR on a 3-page scan: same result, 31,784 bytes both |

**Still needed by hand:** IME composition (e.g. ibus/fcitx Pinyin) in the
find bar and note editor, typing into the password field, Alt and F10 on the
menu bar, title-bar drag, double-click and edge resize on Wayland and X11,
and the Windows title bar (Snap Layouts, WM_CLOSE prompt).

## Visual fit

Mapping the theme took one ~45-line `Theme::update` (kit.rs). sheaf has no
named palette, so the mapping is the ten most-used literal colors. With
that, kit widgets sit in sheaf's dark UI without standing out:

- Title bar (baseline top, spike bottom): same layout and height. Kit's
  frame has square corners and a wider shadow; its control glyphs are
  thinner SVGs.
  ![title bar](gpui-kit-eval/cmp-titlebar.png)
- Find bar: kit's `small` input is a little shorter.
  ![find bar](gpui-kit-eval/cmp-find.png)
- Password prompt: kit adds a focus ring.
  ![password](gpui-kit-eval/cmp-pw.png)
- Palette (spike only; the baseline has no flag to open it):
  ![palette](gpui-kit-eval/c-step4-palette.png)
- OCR dialog (spike only): kit radios, checkbox, buttons, close ×.
  ![OCR dialog](gpui-kit-eval/c-step6-ocr.png)

## Gaps and workarounds (summary)

| # | Gap | Workaround here | Cost |
|---|---|---|---|
| 1 | Kit's Root always draws its own window frame (square corners) | dropped ours | small; can't opt out |
| 2 | `open_window` makes Root the window root; `WindowHandle<Workspace>` stops downcasting | `WorkspaceWindow` pair | small |
| 3 | Icons need kit's `AssetSource`; failure is silent | `with_assets` | trivial once known |
| 4 | Single-line input propagates Enter/Escape, so the next binding also fires | adapter consumes them | small, but every field needs it |
| 5 | Textarea Ctrl+Enter inserts a newline, no "secondary submits" mode | capture-phase handler | small |
| 6 | Creating a field or setting its text needs `&mut Window` | lazy state + queued `set_text` + focus forwarding | moderate; adds indirection |
| 7 | Command filter is substring-only, unranked | `filterable(false)` + our scorer | small |
| 8 | Command dispatches the item action from the palette's focus | no action on rows; custom row content | moderate; loses kit's row rendering |
| 9 | No PageUp/PageDown in the command list | ours via `set_selected_index` | small |
| 10 | `open_dialog` takes focus to kit's handle | re-focus our content | small, easy to miss |
| 11 | Radio groups have no arrow keys; every radio is a Tab stop | our row navigation, `tab_stop(false)` | moderate |
| 12 | Kit's `AppMenuBar`: no Alt/F10, no mnemonics | kept our `MenuBar` | none |
| 13 | Tree has no drag-and-drop | stopped (step 7) | blocks the outline panel |
| 14 | Debug builds bind `ctrl-shift-i` globally (inspector) | none; ours wins in DocView only | debug only |
| 15 | User `keymap.json` contexts `TextInput`/`TextArea` silently stop matching | docs updated to `!Input` | user-visible change |

Most kit components needed sheaf-specific glue to match behavior sheaf
already had. Gaps 4, 6, 8, 10 and 11 are all about focus and key dispatch,
where an app with dense keyboard shortcuts differs from kit's defaults.

## Ongoing cost

- **Version lock.** gpui-kit pins an exact gpui-pre (`=0.3.9` in 0.7.3).
  Every kit upgrade moves GPUI too, and sheaf can't take a newer Zed GPUI fix
  without a matching kit release. Both publish about weekly, and kit is
  0.x with breaking changes, so staying current is a weekly-to-monthly
  chore touching the adapters above. Falling behind means GPUI fixes wait on
  Longbridge's schedule.
- **Our own GPUI patches.** gpui-pre is a renamed, unmodified snapshot.
  To carry a patch we'd fork the gpui-pre crates at the exact version and
  `[patch.crates-io]` them (`gpui-pre`, `gpui-pre-platform`, `gpui-pre-linux`,
  ...: 19 crates), then redo that for every kit release. That replaces the
  plan's "rebase a `sheaf/gpui` branch on Zed per milestone" with something
  more frequent and more awkward.
- **Build.** +45% clean build, +27% incremental release, +66 crates
  (lsp-types, html5ever, markdown, ropey, notify, rust-i18n, rust-embed, a
  reqwest fork, ...), none of which sheaf uses. gpui-component has no
  feature gates finer than `component`.
- **Behavior drift.** Kit decides focus, Escape/Enter propagation and Tab
  behavior. Each upgrade can change them under our adapters; the headless
  tests in `kit_tests.rs` would catch the ones covered here.

## Recommendation

**Don't adopt gpui-kit as a dependency now. Borrow selectively.**

The real win is the text input: about 1,450 lines of our own editing code go
away, and fields gain undo/redo, word movement, a context menu and kit's
IME work. The title bar, dialog and palette shell are small wins.
Against that:

- a 45% slower clean build and +24% binary size for crates sheaf never calls;
- GPUI locked to Longbridge's weekly snapshot, and our own GPUI patches
  become harder to carry, which conflicts with the fork strategy already
  decided;
- glue for nearly every component to keep sheaf's keyboard behavior (15
  gaps above), with no drag-and-drop tree for the outline panel.

What to do instead:

1. **Borrow, with attribution (Apache-2.0), no dependency:** port undo/redo
   and word-wise movement/selection into `text_input.rs`/`text_area.rs`
   (kit's `undo_manager.rs` and `movement.rs` are the references), and
   kit's title-bar drag-on-first-move behavior if it proves better.
2. **Keep the headless tests:** `kit_tests.rs` shows `#[gpui::test]` works
   for sheaf's widgets. Porting its key-dispatch tests to our own widgets
   on `main` would guard the single-key and Enter/Escape behavior with no
   kit involved.
3. **Revisit** if sheaf grows many standard forms (settings, preferences,
   tables), or if kit adds finer feature gates and drops the exact gpui-pre
   pin. Re-check alongside the gpui-ce review in about 6 months.
