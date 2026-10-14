# Configurable floating toolbar (planned)

Status: slice 1 (command metadata) done 2026-10-14 on `cmd-icons`; the
toolbar itself not started. Written 2026-10-13. The fixed markup
toolbar (`markup_tools.rs` `render_toolbar`) covers what we have today and
stays as is for now; this is the toolbar we want once the command set is
big enough that no fixed layout fits everyone.

## Prior art

Details and sources in [`docs/prior-art/`](../docs/prior-art/README.md).

- **Acrobat** (2023+ UI): a quick-action toolbar the user builds themselves
  through Options > Customize toolbar (add, reorder, remove). It floats over
  the page and can be dragged, but it **can't be docked** or snapped to an
  edge, and covering the page when zoomed is the most common complaint about
  the new UI. The usual workaround is switching back to the classic UI
  ([acrobat.md §3.1](../docs/prior-art/acrobat.md#31-the-new-acrobat-modern-ui-rolled-out-2023-default-since-about-2024);
  [Adobe community thread](https://community.adobe.com/questions-9/how-do-i-move-the-quick-actions-toolbar-in-acrobat-pro-2022-003-20281-on-a-mac-1276794)).
  Docking is something we add, not parity.
- **Foxit**: a Quick Access Toolbar takes any command by right-click, and
  the whole ribbon and toolbar layout **imports and exports as XML**, so a
  firm can share or deploy one layout
  ([foxit.md §5](../docs/prior-art/foxit.md#5-ux-patterns-copy-or-avoid)).
  That's the case for `toolbar.json` being plain, shareable text.
- **PDF-XChange**: toolbars and keys are customized in one dialog, which
  shows when a key is already taken
  ([other-tools.md](../docs/prior-art/other-tools.md#pdf-xchange-editor-tracker-software--pdf-xchange-co)).
- **SumatraPDF**: one command id per action drives menus, palette and keymap,
  as our `CommandRegistry` already does.
- **PDFlyer**: has no hotkeys and tells users to pin its tools to Acrobat's
  Quick Tools bar. Those pinned tools **vanish** after Acrobat upgrades or
  when new PDFs are opened ([pdflyer.md §4](../docs/prior-art/pdflyer.md#4-known-pain-points)).
  Unregistered ids should be hidden, not dropped (see *Config* below).

The configurability is the point. Moving it around is secondary.

## What we already have

Most of the plumbing exists, because the menu bar and command palette
already work off one list of commands:

- `CommandRegistry` (`commands.rs`) holds every command, built-in or from a
  plugin, by a stable id: the action name for built-ins
  (`docview::Save`), `<plugin>::<command>` for plugins. That id is exactly
  what a toolbar layout should store.
- `Target::action()` turns an entry into something we can dispatch.
- `MenuEntry` already has `Command { id, label }` / `Separator` /
  `Submenu`. A toolbar layout is the same shape (with groups instead of
  submenus).
- `keymap.json` and `marks.json` set the pattern for user config in
  `settings::config_dir()`: JSON, loaded at startup, bad entries reported
  and skipped rather than failing the whole file.
- `Tooltip::for_action` already shows a title, a description line and the
  shortcut.

## Command metadata (do first, independent of the toolbar)

Every command, built-in or from a plugin, should carry one standard set of
metadata, and every surface (menus, palette, keymap, toolbar, tooltips)
reads from it. Plugins already declare a list of commands in
`[[commands]]` (`CommandDecl`: `id`, `label`, `category`, `menu`, `keys`),
so this extends what exists rather than adding a new mechanism. Today
`Command` in the registry has only `id`, `label`, `category`; built-in
tools keep their icon and description on `Tool` instead, and nothing else
has either.

We own every plugin and are pre-alpha, so there's no compatibility to keep:
the required fields are required, the manifest rejects a command without
them, and our plugins get updated in the same change.

Per command, for `Command` and `CommandDecl` alike:

| Field | | Used by |
|---|---|---|
| `id` | required (exists) | everything; what configs store |
| `label` | required (exists) | menus, palette, toolbar text mode; `&` mnemonic |
| `icon` | **required, new** | toolbar, palette, customize list, menus (optional) |
| `help` | **required, new** | one or two sentences: tooltips, palette second line, customize list |
| `category` | optional (exists) | palette prefix, customize-dialog grouping; defaults to the plugin's `name` |
| `menu` | optional (exists, plugins) | menu placement; built-ins place via `MenuSpec` |
| `keys` | optional (exists, plugins) | default binding; user keymap overrides |
| `keys-context` | optional, new | where `keys` applies; today a plugin binding is window-wide, so a single-letter key would fire inside text fields |
| `when` | optional, new | availability; greyed out in menus, palette and toolbar. Absent means always available |
| `keywords` | optional, new | extra palette search terms ("rotate" finds "Turn page") |

Per plugin, alongside `name` and `description`:

| Field | | Used by |
|---|---|---|
| `homepage` | optional, new | an `https://` URL, shown as a link in the plugin manager and opened in the system browser. Docs, source, changelog: whatever the author points it at. |

Notes:

- **Icons, everywhere.** Required for built-ins too: the registry
  refuses a command with no icon, with a test that walks every built-in.
  `assets/icons/fns/` already has the house style (24 × 24 grid,
  1.75-unit rounded strokes, `currentColor`, readable at 20–32px) and
  about 30 commands drawn. The menus register about 60, so roughly half
  still need art; drawing those is part of step 1, not a later polish
  pass. No lettered fallback.
- **Plugin icons** are SVG files in the plugin package,
  `icon = "icons/scan.svg"`, relative to the manifest, in the same house
  style. The loader checks the file exists, parses as SVG, and doesn't
  escape the package folder. The package grows from two files
  (`sheaf-plugin.toml`, `plugin.wasm`) to a folder, so Install from
  Folder/File and `.sheafplugin` zips must copy the whole thing. The
  panel-row `icon` enum in `wit/sheaf.wit` stays separate: that's a fixed
  set of status glyphs, not command art.
- **`help`** replaces the tooltip descriptions `Tool` keeps today
  (`Tool::description()`). It's what `Tooltip::for_action` shows under
  the title. Plain text, no markup. Anything longer belongs on the
  plugin's `homepage`.
- **`when`.** A fixed vocabulary the host evaluates, so no plugin code runs
  to decide whether a button is grey: `document` (a document is open),
  `editable`, `selection` (annotations), `text-selection`, `pages`
  (pages selected in the organizer), `region` (Shift+drag region). Several
  means all of them. Built-ins use the same vocabulary, plus their own
  `fn(&App) -> bool` where a fixed word isn't enough (Undo with nothing to
  undo, Save with no changes). The menu bar and palette get greying from
  this before the toolbar exists.
- **Active/checked state** (current tool, a panel being open) is a
  built-ins-only hook for now. Plugins asking for toggle state would mean
  calling into WASM on every repaint; leave it until a plugin needs it.
- Update in the same change: `docs/plugins.md` (`[[commands]]` row, new
  `homepage` key, icon style pointer to `assets/icons/fns/README.md`),
  the six plugins in `plugins/` and the test fixture plugin (icons, help
  text, homepages), and bump `api` since old manifests stop loading.

## Other gaps

- **Non-command items.** The current bar has a color picker and the
  tickmark palette, which aren't single commands. Either they become
  "widget" items with their own ids (`toolbar::Colors`,
  `toolbar::Tickmarks`) or they stay on the fixed bar only. Widget items
  are the better answer so a user can move them too.
- **Commands with arguments.** `plugin::Run` takes `args`; a toolbar entry
  should be able to carry them, the same as a `keymap.json` binding can
  (`["plugin::Run", { ... }]`).

## Shape

### Config

`toolbar.json` beside `keymap.json`:

```json
{
  "toolbars": [
    {
      "name": "Review",
      "dock": "floating",
      "position": [640, 96],
      "orientation": "horizontal",
      "visible": true,
      "items": [
        "tools::ToolHighlight",
        "tools::ToolTick",
        "toolbar::Colors",
        "separator",
        "docview::RotatePageCw",
        ["plugin::Run", { "plugin": "tax-form-bookmarks", "command": "scan" }],
        "separator",
        "docview::Save"
      ]
    }
  ]
}
```

- `dock`: `floating`, `top`, `bottom`, `left`, `right`. Docked bars
  reserve space in the document view rather than covering pages; floating
  bars draw over it.
- `position` is only for floating bars, relative to the document view, and
  is clamped back on screen when the window shrinks.
- Items reference ids, never labels, so renames don't break layouts.
- An id that isn't registered (plugin uninstalled or not loaded yet) is
  kept in the file and hidden at runtime, not dropped. Reinstalling the
  plugin brings the button back.
- Possibly more than one toolbar. Start with one, but keep the file a list
  so we don't need a format change later.

### Where it lives

- Widget in `sheaf-ui` (`toolbar.rs`), like the palette: it takes a list of
  resolved items (icon, title, help, state, action) and an on-run
  callback. It knows nothing about `CommandRegistry`.
- App side (`sheaf-app/src/toolbar.rs`) loads `toolbar.json`, resolves ids
  against the registry, re-resolves when plugins load or unload, and saves
  changes back.
- One layout shared by all windows; position is per window, as the
  workspace already is.

### Floating behavior

- Drag by a grip at one end. Dragging near an edge of the document view
  shows a dock target; dropping there docks it. Dragging a docked bar off
  the edge floats it.
- Vertical when docked left/right, horizontal top/bottom; floating
  remembers its own orientation (toggle in its context menu).
- Overflow: when the bar is longer than the space, the rest goes into a
  `»` dropdown, like Zed's and Acrobat's.
- Never covers a modal or the command palette; sits below popovers.

### Customize mode

This is the part that makes it worth doing.

- Right-click the bar → **Customize toolbar…**, also a command
  (`workspace::CustomizeToolbar`) so it's in the palette.
- Dialog with two lists: every registered command on the left (searchable
  with the palette's fuzzy `score`, grouped by `category`, plugin
  commands under their plugin), the current toolbar on the right.
- Add, remove, reorder (drag, plus move-up/down buttons and keys for
  keyboard users), insert separator, reset to default.
- Changes show live on the bar while the dialog is open; Cancel reverts.
- Right-click a button on the bar itself → Remove from toolbar. Right-click
  a menu item or palette row → Add to toolbar. Cheap, and it's how most
  people will actually build their bar.
- Reuses `sheaf-ui`'s virtualized list for the command list and the shared
  dialog parts and buttons.

### Plugins

- Plugins don't place themselves on the toolbar. The user does. (A
  manifest could suggest a default group, but adding buttons without
  asking is how toolbars get cluttered.)
- Plugin commands appear in the customize list as soon as the plugin
  loads.
- `docs/plugins.md` gets a note on toolbar use (the metadata fields land
  in step 1).

## Slices

1. Command metadata: required `icon` and `help`, optional `keywords` and
   `keys-context`, on `Command` and `CommandDecl`; plugin `homepage`
   linked from the plugin manager. Built-ins fill them in (moving `Tool`'s
   icon and description onto their commands) and the missing icons get
   drawn. Plugin packages become folders with icon files; our plugins are
   updated. Palette shows help text and searches keywords. Tests for
   manifest parsing, missing icons and path escapes. Useful on its own,
   toolbar or not.
   *Done 2026-10-14.* Notes and deviations:
   - Built-in metadata lives in `sheaf-app/src/command_info.rs`, keyed by
     action name, rather than on each menu item (several commands sit in
     two menus). `Command::builtin` fills it in; the registry refuses a
     command with an empty icon or help (`RegistryError::Incomplete`), and
     an `assets.rs` test walks every built-in and loads its icon.
   - 20 new icons for the M7 commands (print, export pages, properties,
     page layouts, cover page, view rotation, split, read mode, full
     screen, rulers, grid, clear guides, go to page, attachments, layers,
     clear recent). `ShowPatterns` uses a copy of the regex variant
     (`pattern-search/icon.svg`) until one is picked.
   - Commands that aren't in a manifest get stand-ins: Open Recent entries
     use `open`, plugin panels' Show commands use `right-dock`.
   - `Tool::description()` now reads the command's help; tool icons come
     from the same table.
   - Plugin icons are read at plugin start (`Manifest::icon` checks path,
     size ≤ 64 KB and `usvg` parse) and served from memory as
     `plugin-icons/<plugin>/<command>-<hash>.svg`, so a reinstalled
     plugin's new artwork isn't hidden by GPUI's cache.
   - Packages copy only top-level files and `icons/` (not `target/`).
   - API bumped to 0.2 (WIT text unchanged). Bundled upgrades compare the
     installed manifest's `version` alone, so 0.1 copies that no longer
     load still upgrade.
   - Palette rows are two lines (icon, label, id; help underneath);
     keyword-only matches rank below label matches. The plugin manager
     shows a Website link and each plugin's command icons.
   - Not done: icons in the menus (optional in the plan).
2. `when` availability plus the built-in active/enabled hooks, used by the
   menu bar and palette first so it's exercised before the toolbar exists.
3. `sheaf-ui` toolbar widget: static items, separators, tooltips, state,
   overflow. Docked at the top only. App renders one from a hardcoded
   default layout that matches today's bar, behind a setting, beside the
   old bar.
4. `toolbar.json`: load, resolve, report bad entries like `keymap.json`,
   keep unknown ids. Widget items for colors and tickmarks.
5. Docking to all four edges and floating with drag, dock targets and
   on-screen clamping. Position saved per window.
6. Customize dialog, plus Add/Remove from the context menus of the bar,
   the menu bar and the palette.
7. Replace the fixed markup toolbar with the default layout; remove the
   setting from slice 3.

Each slice should get a `--script` test (`script.rs`): dispatch
`workspace::CustomizeToolbar`, click, `dump` a new `"toolbar"` state.

## Open questions

- Multiple toolbars from the start, or one?
- Should the toolbar change with context (different layout while a form
  field or text selection is active), or stay fixed and only grey out?
  Fixed is simpler and predictable; context bars could come later as a
  separate "contextual" toolbar.
- Import/export of a layout so a firm can hand out a standard bar. A file
  copy of `toolbar.json` may be enough.
- Icon format for plugins: SVG only, or allow PNG too?
