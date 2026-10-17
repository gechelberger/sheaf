# Writing sheaf plugins

Status: API `sheaf:plugin@0.3.0` (0.2 still served), 2026-10-15. Design notes live in
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
sheaf-plugin-api = "0.3"   # or { path = ".../sheaf/crates/sheaf-plugin-api" }

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
api = "^0.3"
homepage = "https://example.com/my-plugin"

[[commands]]
id = "bookmark-first-lines"
label = "Bookmark &First Lines"
icon = "icons/bookmark-first-lines.svg"
help = "Bookmark each page, titled with its first line of text."
menu = "Tools/My Plugin"
```

and `icons/bookmark-first-lines.svg` beside it (see [Icons](#icons)).

Build and install:

```sh
cargo build --release --target wasm32-wasip2
dest=~/.config/sheaf/plugins/my-plugin     # %APPDATA%\sheaf\plugins on Windows
mkdir -p "$dest"
cp -r sheaf-plugin.toml icons "$dest/"
cp target/wasm32-wasip2/release/my_plugin.wasm "$dest/plugin.wasm"
```

Restart sheaf, or use **Plugins → Manage Plugins… → Install from Folder…**
on a folder holding `sheaf-plugin.toml`, `plugin.wasm` and `icons/`. To hand
a plugin to someone else, zip those (at the top level or in one folder) and
rename the zip to `name.sheafplugin`; **Install from File…** takes it.

Other languages work too: anything that builds a WASI 0.2 component for the
`plugin` world in [`wit/sheaf.wit`](../wit/sheaf.wit) (the 0.2 world, still
served, is in [`wit/0.2`](../wit/0.2/sheaf.wit)) (C with wit-bindgen,
Go with TinyGo, JavaScript with ComponentizeJS, …).

## The manifest

`sheaf-plugin.toml` is static data: menus, presets and marks appear without
running any plugin code.

| Key | Meaning |
|---|---|
| `id` | 1–64 lowercase letters, digits and dashes. Unique. Can't be a built-in action namespace (`docview`, `tools`, `workspace`, …). |
| `name`, `description` | Shown in the plugin manager. |
| `homepage` | Optional `https://` URL (docs, source, changelog), linked from the plugin manager. |
| `version` | Semver of the plugin. |
| `api` | Semver requirement on the API, e.g. `"^0.3"`. sheaf serves 0.3 and 0.2, and refuses a plugin whose range takes neither. |
| `[[commands]]` | `id`; `label` (`&` marks the menu mnemonic); `icon`, an SVG in the plugin folder such as `"icons/scan.svg"` (see [Icons](#icons)); `help`, a sentence or two of plain text shown in tooltips and the command palette. Optional: `keywords` (more words the palette finds it by), `category` (palette prefix; defaults to `name`), `menu` (a path like `"Tools/Workpapers"`; menus and submenus are created as needed), `keys` (a default binding in `keymap.json` syntax; the user's keymap overrides it), `keys-context` (where `keys` applies, as a keymap context such as `"DocView && !TextInput"`; by default anywhere in the window, text fields included, so give single-key bindings a context), `when` (what must hold for it to run, all of: `"document"` a document is open, `"selection"` an annotation is selected, `"pages"` pages are selected in the thumbnails, `"region"` a region is Shift+dragged; menus and the palette grey the command out otherwise, and sheaf decides without running plugin code). `icon` and `help` are required. |
| `[[panels]]` | `id`, `label`. Each gets a dock tab and a **View → Plugin Panels** entry. |
| `events` | Document events to receive: any of `"opened"` (once the file has opened: its marks, form, bookmarks and the rest are read), `"edited"`, `"saved"`, `"selection-changed"`, `"closed"`, `"region"` (the region the user Shift+drags on a page, live while dragging, and its clearing). |
| `[[presets]]` | `name`, `regex`: pattern-search presets shown (read-only) in the Patterns panel. |
| `[marks]` | `tickmarks = [{ name, marks = ["Σ", …] }]` and `stamps = ["PBC", …]` for the markup toolbar. |
| `[permissions]` | `read = ["~/Documents/Clients"]`, `write = [...]`: absolute paths or `~/…`. `network = ["api.example.com"]`: hosts the plugin may make HTTP requests to (`"*.example.com"` includes subdomains, `"host:8080"` limits the port). See [Trust model](#trust-model). |
| `preferences` | The keys the plugin reads from its section of `settings.toml` (see `storage`), e.g. `["target", "round"]`. A key there that isn't listed is reported in the plugin manager as a likely typo. Leave it out and nothing is checked. |

### Icons

Every command has one: it's what toolbars, the command palette and the
plugin manager show. Draw it in the style of sheaf's own
([`assets/icons/fns/README.md`](../assets/icons/fns/README.md)): a 24 × 24
viewBox, 1.75-unit rounded strokes, no fill, and `currentColor` for every
stroke, so it follows the theme and the button's state. It should read at
16–32px. sheaf checks when it loads the plugin that the file exists inside
the plugin folder, is valid SVG and is at most 64 KB; a command whose icon
fails is left out and the plugin manager says why.

Plugins don't put themselves on the toolbar; users do, from View >
Customize Toolbar… or by right-clicking the command in a menu or the
command palette. A plugin command on the toolbar is hidden while the
plugin is off and comes back with it.

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
`pages` (id, rotation, displayed size), `outline` (pre-order with depth,
each bookmark's page and position),
`annotations(page)`, `selection`, `current-page`. Ids are opaque and stable
while the document is open; they aren't page numbers. With no document open,
`path` is none and the lists are empty.

**Marks** (`annot` in `types`) under 0.3 are every kind sheaf has:
highlight, underline, strike-out, squiggly, replace (a suggested
replacement) and insert, note, reply and status (threaded by `name`),
free-text and callout, square, ellipse, line (with `arrow-start`),
polygon, polyline and ink, tickmark, connector, page-stamp, text-stamp
and page-border, file-attachment, measure (distance, perimeter or area
with its scale), and tape. Each has its `fill`, `dashed`, `cloudy`,
`created`, `modified` and `name` (its `/NM`). Dates and the name are the
host's: they're ignored when you add or update a mark. An update replaces
what you send, so read a mark, change it and send it back.

A mark's author is the user when you leave it empty, and `<<name>>`,
`<<initials>>`, `<<date>>` and `<<time>>` in its text (and in a text
stamp's) are filled in as dynamic stamps fill them: write
`"Reviewed <<initials>> <<date>>"` and never ask who the user is.

**`text`**: `page-text`, `pattern-search(page, regex)`,
`search(page, query, match-case)`, `text-boxes(page, rect)` and
`words(page, rect?)` (every word with its box). Matches come
with their boxes in page coordinates: points, top-left origin, the page as
the file displays it (the same space annotations use). Pattern matches also
carry their capture groups. OCR text the user hasn't saved yet counts.
This frame is 0.3's and stays so: sheaf keeps pages' geometry in their
unrotated user space now, and converts at the API's edge.

**`edits`**: `submit(label, edits)` queues a batch that becomes one undo step
labelled `label`. Batches are applied in order **after your call returns**,
so ids of things a batch creates (bookmarks, annotations) aren't known during
the call. If any edit in a batch fails (a page that's gone, say), that whole
batch is skipped and the user sees why. The edits mirror sheaf's own: move,
rotate, delete and duplicate pages; insert, move, edit and delete bookmarks;
add, update and delete annotations; add a connector pair. Bookmarks can land
at a position on their page (`dest-view`: `/XYZ` with a point and zoom, or
another PDF destination kind kept as is), and `outline-insert-tree` adds
several nested bookmarks in one edit, such as a new heading with its
children. `Batch` in the SDK builds the common ones (`bookmark_at`,
`bookmark_tree`, `at(left, top)`).

**`ui`**: `show-form(form)` asks the user and blocks until they answer (none
if they cancel). Fields: text, number (with optional bounds), choice,
checkbox, file (open or save) and page range (answered as page ids).
`FormBuilder` in the SDK builds forms and reads answers. `notice(message)`
shows a line in the status area (an empty message clears it). `set-panel(panel, rows)` replaces the rows
of one of your panels for the current document. Each row has a label,
detail, badge, icon, indent, an optional page (clicking the row goes there)
and buttons that call `panel-action`. `show-panel(panel)` brings a panel
into view without taking keyboard focus: one of yours, or sheaf's own
`pages`, `bookmarks`, `patterns`, `calc` or `comments` (say, to show the
comments a command just added). Setting a panel's rows also shows it.

**`patterns`**: `presets()` returns the user's Patterns presets, then those
plugins ship.

**`render`**: read-only page images. `page-renderer(page)` draws the page
(or a region of it, in page coordinates) at a dpi you choose (up to 600), in
`gray8` or `rgba8`, as the file draws it, without sheaf's own unsaved
markup. The pixels stay in sheaf: an `image` gives its size, maps pixels to
page points (`to-page`), and hands over rows or tiles as you read them.
Handles last for one call. Rendering time doesn't count against the call's
limit, but each call may render at most 400 million pixels and keep 16
images alive at once. `plugins/duplicate-pages` uses it to fingerprint
pages.

**`ocr`**: `recognize(page, options)` runs the user's OCR engine on one page
and returns its words with boxes. It doesn't change the document.
`page-text-layer(page)` returns the unsaved OCR layer, if any.

**`storage`**: `get`, `set`, `delete`, `keys` for a key/value store kept in
`<config>/plugin-data/<id>/store.json` (4 MiB per plugin), and `settings()`,
the JSON the user puts in `<config>/plugin-data/<id>/settings.json` (`"{}"`
when there is none). Uninstalling keeps this data.

`preferences()` (0.3) is the plugin's own section of the user's
`settings.toml`, as JSON (`"{}"` when there is none):

```toml
[plugins.currency-convert]
target = "EUR"
round = 2
```

It's read when the call starts and stays the same for the whole call, even
if the user saves the file meanwhile. A plugin reads only its own section,
and needs no permission for it. Declare its keys in the manifest
(`preferences`) so a misspelled one is pointed out.

**`progress`** (0.2.1): for long commands. `task(label, total, unit)` shows
in the document's status bar as "label: 12 of 300 pages…" with a progress
strip; `set`, `inc` and `set-total` move it, `message` replaces the line,
and `child(label, weight)` makes a step covering part of it. Dropping a
task or step, or returning from the command, fills its part of the strip;
one the user stopped stays where it was. Make one task per command: the
strip follows the first, and a later one only changes the status line.
Escape asks the command to stop: `is-cancelled` turns true, and the plugin
should return (an error like "Stopped" is fine). The call's time limit still
applies. The SDK's `progress::pages(label, n)` and `progress::check(&task)?`
cover the usual loop:

```rust
let task = progress::pages("Reading pages", pages.len() as u64);
for p in &pages {
    progress::check(&task)?;
    // … work on p …
    task.inc(1);
}
```

A plugin using it declares `api = "^0.2.1"` or later.

**`log`**: `log(level, message)` goes to sheaf's log with the plugin id.

**HTTP** is standard `wasi:http`: sheaf makes the request (and the TLS
connection) if the host is one the manifest lists and the user allowed;
anything else fails as denied. The SDK's `http::get(url)` reads a whole
response. Time spent waiting on the network doesn't count against the
call's limit. There are no raw sockets.

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
  made itself. Region events are latest-wins: while a plugin is handling
  one, newer ones replace each other and only the newest is sent next. For
  live work, fetch a page's words once and filter them yourself, as
  `plugins/subtotal` does.
- **While a file opens**, its pages show before sheaf has read the rest
  of it, and a plugin's commands are greyed out. A call given meanwhile
  (a command by its keys, a panel action, an event) waits: once the file
  is read, `opened` comes first, then the calls that waited, in order,
  each seeing the document as it is then. Of region events only the
  newest waits. A file closed before it's read drops the calls that
  waited, and `closed` comes only to a document that had `opened` (a
  recovery opens a file again).

## Versioning

**0.3.0** (2026-10-15) has every kind of mark sheaf has, and their new
fields (`fill`, `dashed`, `cloudy`, `arrow-start`, `created`, `modified`,
`name`); and `storage.preferences`. Records and variants grew, which isn't
backward compatible, so it's a minor bump while 0.x. sheaf still serves
0.2: a 0.2 plugin links to the 0.2 world, sees only the kinds 0.2 had (a
newer mark isn't in `annotations`), and an update it makes keeps the
fields it can't see. Rebuild against SDK 0.3 and set `api = "^0.3"` to
get the rest.


The WIT package `sheaf:plugin` follows semver, and the SDK crate's version
is the package's version. Additive changes (new interfaces or functions)
bump the minor version; anything else bumps the major (the minor, while
0.x). A sheaf release keeps serving the previous world (N−1) next to the
current one, so a plugin built against the last API keeps loading for at
least one more release. A manifest's `api` range says which worlds it can
take; sheaf refuses one it can't satisfy, with a message, rather than
guessing.

**0.2.1** (2026-10-14) added the `progress` interface. Adding an interface
is backward compatible, so it's a patch: plugins built against 0.2.0 import
`@0.2.0` names, and wasmtime links them to the 0.2.1 host unchanged.

**0.2.0** (2026-10-14) changed the manifest, not the WIT: every command
needs `icon` and `help`, and a plugin package is a folder (or archive) with
its `icons/`. 0.1 plugins aren't served alongside, since nothing outside
this repository was built against them; rebuild against the 0.2 SDK and add
the new keys.

## Trust model

Plugins are untrusted code. The sandbox is the WebAssembly component
boundary (wasmtime) plus WASI with nothing granted by default:

- **Files.** None, unless the manifest lists directories under
  `[permissions]` *and* the user allows it. A granted directory is visible to
  the plugin at its own path (with `/` separators). `read` directories are
  read-only. `write` directories can be read and written.
- **Network.** HTTP only, to the hosts the manifest names, and only after
  the user allows it (the prompt lists each host). No raw sockets.
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
