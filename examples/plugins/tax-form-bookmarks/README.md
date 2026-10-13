# Tax Form Bookmarks

A sheaf plugin that reads each page's text (including OCR text not yet saved)
and bookmarks every tax form it finds: "Form 1040", "Schedule C", "W-2 — Acme
Corp", "1099-INT — Chase". A form spanning several pages gets one bookmark,
on its first page. Running it again doesn't duplicate bookmarks. All the
bookmarks undo as one step.

Tools → Tax → Bookmarks from Tax Forms, the command palette, or Ctrl+Alt+B.

## Build and install

```sh
cargo build --release --target wasm32-wasip2
dest="${SHEAF_CONFIG_DIR:-$HOME/.config/sheaf}/plugins/tax-form-bookmarks"
mkdir -p "$dest"
cp sheaf-plugin.toml "$dest/"
cp target/wasm32-wasip2/release/tax_form_bookmarks.wasm "$dest/plugin.wasm"
```

The detection logic has unit tests that run on the host: `cargo test`.
