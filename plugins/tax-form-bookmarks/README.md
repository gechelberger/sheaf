# Tax Form Bookmarks

A sheaf plugin that reads each page's text (including OCR text not yet saved)
and bookmarks every tax form it finds: "Form 1040", "Schedule C", "W-2 — Acme
Corp", "1099-INT — Chase". A form spanning several pages gets one bookmark,
on its first page. Running it again doesn't duplicate bookmarks. All the
bookmarks undo as one step.

Tools → Tax → Bookmarks from Tax Forms, the command palette, or Ctrl+Alt+B.

## Build

Ships with sheaf. `scripts/build-plugins.sh` builds every plugin in
`plugins/`; sheaf installs them into its plugin directory on first run.
To work on just this one: `cargo build --release --target wasm32-wasip2`.

The detection logic has unit tests that run on the host: `cargo test`.
