# Writing sheaf plugins

Status: API `sheaf:plugin@0.1.0`, 2026-10-13. Design notes live in
[`todos/plugins.md`](../todos/plugins.md).

A sheaf plugin is a WebAssembly component plus a manifest. It can add
commands to the menus, the command palette and the keymap; read the open
document's pages, bookmarks, markup and text (including OCR text that hasn't
been saved yet); change the document through the same edits sheaf itself
uses, each batch one undo step; ask the user questions with forms; list
things in its own dock panel; react to document events; keep a little data;
and, if the user allows it, read or write files in named folders or use the
network.

It runs sandboxed, so one `.wasm` file works on every platform sheaf runs on.
A crash, a hang or runaway memory unloads the plugin and leaves the app
running.

sheaf ships its own plugins this way (the repo's [`plugins/`](../plugins)):
[`tax-form-bookmarks`](../plugins/tax-form-bookmarks) and
[`amount-crossref`](../plugins/amount-crossref) are good worked examples.

## Quick start (Rust)

```sh
rustup target add wasm32-wasip2
cargo new --lib my-plugin && cd my-plugin
```

`Cargo.toml`:

```toml
[package]
name = "my-plugin"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
sheaf-plugin-api = "0.1"   # or { path = ".../sheaf/crates/sheaf-plugin-api" }

[profile.release]
opt-level = "s"
lto = true
strip = true
```

`src/lib.rs`:

```rust
use sheaf_plugin_api::{Batch, Plugin, document, text};

struct MyPlugin;

impl Plugin for MyPlugin {
    fn run_command(id: String, _args: Option<String>) -> Result<(), String> {
        match id.as_str() {
            "bookmark-first-lines" => {
                let mut batch = Batch::new("Bookmark first lines");
                for p in document::pages() {
                    let t = text::page_text(p.id)?;
                    if let Some(line) = t.lines().map(str::trim).find(|l| !l.is_empty()) {
                        batch = batch.bookmark(line, Some(p.id));
                    }
                }
                batch.submit();
                Ok(())
            }
            other => Err(format!("unknown command {other}")),
        }
    }
}

sheaf_plugin_api::export!(MyPlugin);
```

`sheaf-plugin.toml`, next to `Cargo.toml`:

```toml
id = "my-plugin"
name = "My Plugin"
version = "0.1.0"
api = "^0.1"

[[commands]]
id = "bookmark-first-lines"
label = "Bookmark &First Lines"
menu = "Tools/My Plugin"
```

Build and install:

```sh
cargo build --release --target wasm32-wasip2
dest=~/.config/sheaf/plugins/my-plugin     # %APPDATA%\sheaf\plugins on Windows
mkdir -p "$dest"
cp sheaf-plugin.toml "$dest/"
cp target/wasm32-wasip2/release/my_plugin.wasm "$dest/plugin.wasm"
```

Restart sheaf, or use **Plugins → Manage Plugins… → Install from Folder…**
on a folder holding `sheaf-plugin.toml` and `plugin.wasm`. To hand a plugin
to someone else, zip those two files (at the top level or in one folder) and
rename the zip to `name.sheafplugin`; **Install from File…** takes it.

Other languages work too: anything that builds a WASI 0.2 component for the
`plugin` world in [`wit/sheaf.wit`](../wit/sheaf.wit) (C with wit-bindgen,
Go with TinyGo, JavaScript with ComponentizeJS, …).

## The manifest

`sheaf-plugin.toml` is static data: menus, presets and marks appear without
running any plugin code.

| Key | Meaning |
|---|---|
| `id` | 1–64 lowercase letters, digits and dashes. Unique. Can't be a built-in action namespace (`docview`, `tools`, `workspace`, …). |
| `name`, `description` | Shown in the plugin manager. |
| `version` | Semver of the plugin. |
| `api` | Semver requirement on the API, e.g. `"^0.1"`. sheaf refuses a plugin whose range it doesn't satisfy. |
| `[[commands]]` | `id`, `label` (`&` marks the menu mnemonic), optional `category` (palette prefix; defaults to `name`), `menu` (a path like `"Tools/Workpapers"`; menus and submenus are created as needed), `keys` (a default binding in `keymap.json` syntax, active anywhere in the window; the user's keymap overrides it). |
| `[[panels]]` | `id`, `label`. Each gets a dock tab and a **View → Plugin Panels** entry. |
| `events` | Document events to receive: any of `"opened"`, `"edited"`, `"saved"`, `"selection-changed"`, `"closed"`. |
| `[[presets]]` | `name`, `regex`: pattern-search presets shown (read-only) in the Patterns panel. |
| `[marks]` | `tickmarks = [{ name, marks = ["Σ", …] }]` and `stamps = ["PBC", …]` for the markup toolbar. |
| `[permissions]` | `read = ["~/Documents/Clients"]`, `write = [...]`, `network = false`. Absolute paths or `~/…`. See [Trust model](#trust-model). |

A command runs as the action `plugin::Run`, so users can bind it themselves:

```json
[{ "bindings": { "ctrl-alt-b": ["plugin::Run", { "plugin": "my-plugin", "command": "bookmark-first-lines" }] } }]
```

The second element's `args` (any JSON) reaches `run_command` as a string.

## The API

All of it is in [`wit/sheaf.wit`](../wit/sheaf.wit), with each function
documented. The Rust SDK re-exports each interface as a module and adds a
few helpers.

**Entry points** (the `Plugin` trait; only `run_command` is required):
`activate` once after loading, `run_command(id, args)`,
`panel_action(panel, row, action)` when the user clicks a row action, and
`on_event(event)` for subscribed events.

**`document`**: the document as it was when the call started. `path`,
`pages` (id, rotation, displayed size), `outline` (pre-order with depth),
`annotations(page)`, `selection`, `current-page`. Ids are opaque and stable
while the document is open; they aren't page numbers. With no document open,
`path` is none and the lists are empty.

**`text`**: `page-text`, `pattern-search(page, regex)`,
`search(page, query, match-case)` and `text-boxes(page, rect)`. Matches come
with their boxes in page coordinates: points, top-left origin, the page as
the file displays it (the same space annotations use). OCR text the user
hasn't saved yet counts.

**`edits`**: `submit(label, edits)` queues a batch that becomes one undo step
labelled `label`. Batches are applied in order **after your call returns**,
so ids of things a batch creates (bookmarks, annotations) aren't known during
the call. If any edit in a batch fails (a page that's gone, say), that whole
batch is skipped and the user sees why. The edits mirror sheaf's own: move,
rotate, delete and duplicate pages; insert, move, edit and delete bookmarks;
add, update and delete annotations; add a connector pair. `Batch` in the SDK
builds the common ones.

**`ui`**: `show-form(form)` asks the user and blocks until they answer (none
if they cancel). Fields: text, number (with optional bounds), choice,
checkbox, file (open or save) and page range (answered as page ids).
`FormBuilder` in the SDK builds forms and reads answers. `notice(message)`
shows a line in the status area. `set-panel(panel, rows)` replaces the rows
of one of your panels for the current document. Each row has a label,
detail, badge, icon, indent, an optional page (clicking the row goes there)
and buttons that call `panel-action`.

**`ocr`**: `recognize(page, options)` runs the user's OCR engine on one page
and returns its words with boxes. It doesn't change the document.
`page-text-layer(page)` returns the unsaved OCR layer, if any.

**`storage`**: `get`, `set`, `delete`, `keys` for a key/value store kept in
`<config>/plugin-data/<id>/store.json` (4 MiB per plugin), and `settings()`,
the JSON the user puts in `<config>/plugin-data/<id>/settings.json` (`"{}"`
when there is none). Uninstalling keeps this data.

**`log`**: `log(level, message)` goes to sheaf's log with the plugin id.

### Limits

- **Time**: each call has 30 seconds of the plugin's own time. Waiting for the
  user (`show-form`) and for OCR doesn't count. Past the limit, the call
  traps.
- **Memory**: 256 MiB of linear memory.
- **Traps** (a panic, a timeout, running out of memory) unload the plugin
  for the rest of the session. The user sees what happened, and the plugin
  manager shows the error. Returning `Err(message)` from an entry point is
  not a trap: the user sees the message and the plugin stays loaded.
- **Threading**: one call at a time per plugin, in order, on the plugin's own
  thread. Global state in your module persists between calls until sheaf
  exits or the plugin is turned off. Keep anything that must outlive that in
  `storage`.
- **Events** arrive like any other call. A plugin isn't told about edits it
  made itself.

## Versioning

The WIT package `sheaf:plugin` follows semver, and the SDK crate's version
is the package's version. Additive changes (new interfaces or functions)
bump the minor version; anything else bumps the major (the minor, while
0.x). A sheaf release keeps serving the previous world (N−1) next to the
current one, so a plugin built against the last API keeps loading for at
least one more release. A manifest's `api` range says which worlds it can
take; sheaf refuses one it can't satisfy, with a message, rather than
guessing.

## Trust model

Plugins are untrusted code. The sandbox is the WebAssembly component
boundary (wasmtime) plus WASI with nothing granted by default:

- **Files.** None, unless the manifest lists directories under
  `[permissions]` *and* the user allows it. A granted directory is visible to
  the plugin at its own path (with `/` separators). `read` directories are
  read-only. `write` directories can be read and written.
- **Network.** None, unless `network = true` and the user allows it.
- **Consent.** When a plugin asks for permissions the user hasn't answered,
  it runs without them and sheaf asks once, in a prompt listing exactly what
  it wants. Answers are kept in `<config>/plugins.json`. A later version
  asking for more asks again. The plugin manager shows each plugin's
  permissions and can allow or revoke them at any time.
- **The document.** A plugin sees the documents the user runs it on (or that
  its events fire for), and changes them only through undoable edits.
- **Not yet:** code signing and a plugin catalog. Install plugins from
  people you trust. sheaf's bundled plugins come from this repository.

Plugins can't draw their own UI. Forms and panels are described as data and
sheaf draws them, so a plugin can't imitate sheaf's own dialogs or read the
screen.

## Plugins that ship with sheaf

The `plugins/` directory holds plugins that are part of sheaf. They use the
same API as anyone's. `scripts/build-plugins.sh` builds and stages them;
release bundles carry them in `plugins/` beside the binary. On startup,
sheaf installs any it hasn't installed before into the user's plugin
directory. From then on they're ordinary plugins: the user can turn them off,
replace them or uninstall them. An uninstalled bundled plugin stays
uninstalled, and only untouched copies are upgraded when sheaf ships a newer
version. Debug builds read the staged set from `target/plugins`.

## Testing a plugin

Unit-test your logic on the host: the SDK compiles there (calls into sheaf
panic outside a plugin). Keep pure functions apart from the calls, as the
bundled plugins do. To run a plugin against real documents, see
`crates/sheaf-plugin/tests/host.rs`. It loads components with fake text and
UI sources and checks the edits they submit.
