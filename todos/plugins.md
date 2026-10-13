# M6: Plugin system (WASM components + declarative host API)

## Context
Plugins that extend behavior almost arbitrarily have been a goal from the start, but no groundwork exists yet. Today:
- Commands are GPUI `actions!` structs fixed at compile time (`main.rs:35-135`).
- Menus come from a static `app_menus()` (`menus.rs`), and the palette walks those menus (`palette.rs:44-74`).
- Panels are a closed `enum Panel` (`workspace.rs:78-125`).
- Pattern presets and tickmarks are `const` arrays.
- Nothing announces document changes.

Some existing pieces help:
- Every mutation goes through `Document::apply(Edit)` (`sheaf-doc/src/model.rs:146-230, 528`).
- `Annot` already supports serde.
- `keymap.rs` builds actions from string names with JSON arguments.
- Dock layout is saved by string panel IDs (`settings.rs`).

Decisions (from Greg, 2026-10-11):
- **Authors:** in-house and third-party developers, so plugins need a sandbox and a stable API.
- **UI:** declarative. Plugins describe UI as data and the host draws it.
- **Runtime:** WASM components, built with wasmtime and WIT interface files (the same approach as Zed's extensions).
- **Timing:** M6, after Scan & OCR (M5). OCR is a core feature (`sheaf-ocr`), not a plugin: Tesseract and ONNX Runtime are native code and can't run in wasmtime. Plugins get OCR as a host capability instead.

Why WASM: one `.wasm` file runs unchanged on Windows and Linux, with no Rust ABI or DLL problems. A plugin crash becomes a caught error instead of a dead app. Plugins can be written in Rust, C, Go and other languages.

## Architecture
- **New crate `sheaf-plugin`** (host side). It handles wasmtime and the component model, loading manifests, the plugin registry and running plugins. It depends on `sheaf-doc` and `sheaf-core`, but not on GPUI. The app connects it to the UI.
- **New crate `sheaf-plugin-api`** (guest SDK). It wraps `wit-bindgen` with a `#[sheaf_plugin]` macro and ergonomic types. It is published separately and versioned with the WIT package.
- **`wit/sheaf.wit`:** package `sheaf:plugin@0.1.0`, defining the world `plugin`.
  - Host-provided interfaces (what plugins can call):
    - `document`: read-only snapshot queries for pages, outline, annotations and sources.
    - `text`: page text, text boxes, search and pattern search. These are proxied to the `RenderHandle` worker.
    - `edits`: submit `list<edit>`, applied as one undo step. The WIT `edit` variant mirrors `sheaf_doc::Edit`.
    - `ui`: notices, `show-form(form) -> result`, and panel model updates.
    - `storage`: a per-plugin key/value store plus a settings blob.
    - `ocr`: `recognize(page, options) -> text-layer` and `page-text-layer(page)`, proxied to `sheaf_ocr::job`, so plugins never link an engine.
    - `log`.
  - Plugin-provided exports (what the host calls):
    - `activate`
    - `run-command(id, args-json)`
    - `on-event(doc-event)`
    - `panel-action(panel, item, action)`
- **Manifest `sheaf-plugin.toml`** sits next to `plugin.wasm`. It holds:
  - id, name, version, and the `api` version range.
  - Contributions:
    - commands, with label and category;
    - menu placement, e.g. `menu = "Tools/Workpapers"`;
    - default keybindings;
    - panels;
    - pattern presets, tickmark sets and stamps (static data, so no code needs to run).
  - Permissions: read/write directories through WASI preopens, and network access (off by default).
- **Install location:** `config_dir()/plugins/<id>/` (reusing `settings.rs:86-101`). Enable/disable state is stored in `settings.json`.
- **Execution:**
  - Each plugin runs on its own worker thread with a wasmtime `Store`, so it never blocks the UI thread.
  - Epoch interruption enforces timeouts, and a memory limit is set per store.
  - Before each call the plugin gets a `Document` snapshot, which is cheap because the document uses `im` structures.
  - Returned edits go back to the UI thread and are applied through `DocView::apply`.
  - If a plugin traps, the user sees a notice, the plugin is disabled and the error is logged.
- **Cargo feature `plugins`** is on by default, so wasmtime's build time and binary size can be left out of dev builds if needed.

## Slices

### Slice 1: data-driven host (no WASM yet) — done 2026-10-13
Landed as `commands.rs` (registry + `plugin::Run`), `contrib.rs` (presets, tickmark sets, stamps; user marks in `marks.json`), `DocEvent` in `docview.rs`, `Panel::Plugin` (saved as `plugin:<id>`), and `Document::apply_batch` plus serde on `Edit` in `sheaf-doc`. `plugin::Run` only reports "plugins aren't supported yet" until slice 2. Deviation: the palette lists each command once (it used to list an action once per menu it appeared in).
- **`CommandRegistry`** (a global in `sheaf-app`):
  - Entries hold id, label, category and target. The target is either `Builtin(Box<dyn Action>)` or `Plugin{plugin, command}`.
  - Built-in commands register from the current `app_menus()` data.
  - `menus.rs` builds menus from the registry plus contributed menu paths.
  - `palette::commands()` reads the registry instead of walking menus.
- **New action `plugin::Run { plugin, command, args }`** handled at workspace level. Users can bind it in `keymap.json`, because `[name, args]` pairs already work in `keymap.rs:62-64`.
- **`sheaf-doc` changes:**
  - Add `#[derive(Serialize, Deserialize)]` to `Edit` and its ID types.
  - Add `Document::apply_batch(Vec<Edit>, label)`, which creates a single undo snapshot.
- **`DocView` becomes an `EventEmitter<DocEvent>`** with events `Opened`, `Edited{outcome}`, `Saved`, `SelectionChanged` and `Closed`. It emits them from `DocView::apply` (`docview.rs:821-857`) and from the save/open paths.
- **Contribution points become data:**
  - Pattern presets become a `Vec` loaded from built-ins, user files and plugin manifests. This also delivers the planned user-defined presets.
  - Tickmark sets and stamps become data the same way.
  - `Panel` gets a `Plugin(String)` variant.

### Slice 2: WASM runtime core — done 2026-10-13
Landed: `crates/sheaf-plugin` (wasmtime 49 component host, manifest loader, `discover`), `crates/sheaf-plugin-api` (minimal guest SDK over wit-bindgen 0.62, `export!`), `wit/sheaf.wit`, app `plugins.rs` (startup discovery, registration, `plugin::Run` applies each `edits.submit` as one undo step), Cargo feature `plugins` (default on), `settings.plugins.disabled`, `RenderHandle::page_text`. Host tests build the fixture and the example for `wasm32-wasip2` on first use (target pinned in `rust-toolchain.toml`).
Pulled forward from later slices: a minimal `text.page-text` (slice 2's example needs it; unsaved OCR layers win over the engine), WASI with no preopens and no network, the trap/timeout/memory/file-denied tests, and the `sheaf-plugin-api` crate.
Decisions: edits a plugin submits are applied after its command returns (so it can't see ids of what it just created within the same call); each `submit` is one undo step. Plugin ids can't take a built-in action namespace (`docview`, `tools`, …). Plugin key bindings use the `Workspace` context and load before `keymap.json`, so users can override them.
- `sheaf-plugin` crate: wasmtime component host and manifest loader.
- WIT covering the `document`, `edits`, `log` and `run-command` interfaces.
- Plugin discovery at startup.
- Commands contributed by plugins appear in menus and the palette.
- First plugin, `plugins/tax-form-bookmarks` (chosen 2026-10-11): scans page text (including OCR layers) for form headers such as "Form 1040", "Schedule C", "W-2", "1099-INT" and inserts bookmarks named like "1099-INT — Chase". It targets `wasm32-wasip2` and exercises `text` plus batch edits.

### Slice 3: declarative UI — done 2026-10-13
Landed: WIT `ui` interface (`show-form`, `notice`, `set-panel`) and `panel-action` export; `text.pattern-search` (pulled forward from slice 4 for amount-crossref); `plugin_form.rs` (form card, same keys as the OCR card); `plugin_panel.rs` (per-document rows; row click shows the page; actions dispatch `plugin::PanelAction`); panels declared in manifests get a View → Plugin Panels entry and join the right dock unless the saved layout places them. Time a call spends waiting on a form doesn't count against its time limit (wall-clock deadline in the epoch callback). The SDK derives `PartialEq` on WIT types.
**Bundled plugins (Greg, 2026-10-13):** plugins that make sense as plugins ship with sheaf so we dogfood the API — `plugins/` (was `examples/plugins/`): `tax-form-bookmarks`, `amount-crossref`. No difference from user plugins except being available at install: `scripts/build-plugins.sh` stages them, `package.sh` puts them in `plugins/` beside the binary, and on startup sheaf installs any it hasn't installed before into `<config>/plugins` (recorded in `.bundled.json`, so a deleted one stays deleted and a replaced one isn't overwritten; untouched copies upgrade). Debug builds read `target/plugins`.
- `ui.show-form`: a modal with text, number, choice, checkbox, file-picker and page-range fields. The host draws it with the existing `text_input.rs`.
- Notices.
- Panel contribution: a plugin supplies a tree/list model (rows with label, icon, badge and actions). The host draws it in the dock and saves its layout by string ID.
- Second plugin, `plugins/amount-crossref` (chosen 2026-10-11): finds the same dollar amount on different pages (a K-1 total and the 1040 line it flows to) via pattern search, lists the candidate pairs in a panel, and places `ConnectorAdd` pairs for the ones the user accepts. Exercises text boxes, a review panel and connector edits.

### Slice 4: events, storage, permissions, manager
- `on-event` subscriptions, declared in the manifest.
- `text` interface, proxied to the render worker.
- `storage` interface.
- Permissions enforced through WASI preopens, with a first-run consent prompt for each permission.
- Plugins → Manage dialog: list plugins, enable/disable, show errors and permissions, install from folder or `.sheafplugin` zip.

### Slice 5: SDK, docs, CI, packaging
- `sheaf-plugin-api` polished.
- `docs/plugins.md`: authoring guide, API versioning policy (semver on the WIT package; the host keeps N-1 worlds), and the trust model.
- CI builds the example plugins on ubuntu and windows and runs host integration tests against them on both.
- `package.sh` ships the example plugins.
- README milestone table: M6 row.

## Explicitly out of scope
- Plugins drawing their own content on the canvas or in panels.
- Native dylibs.
- A plugin marketplace or code signing (noted as future work).
- Rhai macros. A later slice can expose the same host API to Rhai.

## Critical files
- `crates/sheaf-app/src/{main.rs, menus.rs, palette.rs, keymap.rs, workspace.rs, docview.rs, settings.rs, pattern_panel.rs, markup_tools.rs}`
- `crates/sheaf-doc/src/model.rs` (serde on `Edit`, `apply_batch`)
- New: `crates/sheaf-plugin/`, `crates/sheaf-plugin-api/`, `wit/sheaf.wit`, `plugins/*`, `docs/plugins.md`
- `.github/workflows/ci.yml`, `scripts/package.sh`, `README.md`

## Verification
- **Unit tests:**
  - `apply_batch` creates a single undo step.
  - `Edit` survives a serde round trip.
  - Registry merge, including collisions between built-in and plugin commands.
  - Manifest parsing and rejection of invalid manifests.
- **Host integration tests** (`sheaf-plugin/tests`), using fixture `.wasm` files built in CI:
  - running a command returns edits;
  - a trap disables the plugin;
  - an epoch timeout fires on an infinite loop;
  - file access is denied without the permission.
- **Manual check:**
  1. Install the example plugin into `$SHEAF_CONFIG_DIR/plugins`.
  2. Launch `sheaf` and open a binder.
  3. Run "Bookmarks from Pattern" from the palette and from a `keymap.json` binding.
  4. Confirm the bookmarks appear and one Ctrl+Z reverts all of them.
  5. Open the plugin panel, save the layout, restart, and confirm the panel is restored.
- **CI:** `cargo test --workspace` passes on windows-latest and ubuntu-latest, including the wasm fixtures.
