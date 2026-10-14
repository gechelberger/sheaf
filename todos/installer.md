# `sheaf-pdf`: crates.io installer (planned)

Status: notes only, not started. Written 2026-10-13 after switching to alpha
versioning (`0.1.0-alpha.N`). Short-term distribution is GitHub release
bundles plus building from a git clone.

## Why an installer, not the app

The app can't be published to crates.io:

- GPUI (`gpui`, `gpui_platform`) is a git dependency at a pinned Zed rev, and
  crates.io rejects git dependencies.
- Our `[patch.crates-io]` block only applies inside this workspace; it would
  not reach anyone building the crate from crates.io.
- The binary needs libpdfium and `ocr/` (Tesseract data, PaddleOCR models,
  ONNX Runtime) beside it. A crate can't carry them.

`cargo install --git` isn't a substitute either: it skips
`scripts/fetch-pdfium.sh` and `scripts/fetch-ocr-data.sh`, and rustup
resolves the toolchain from the user's cwd, so our `rust-toolchain.toml` is
ignored.

`sheaf-pdf` (free on crates.io as of 2026-10-13; `sheaf` is taken) is
instead a small installer that fetches the release bundle.

### Why not cargo-binstall

binstall copies only the binary into `~/.cargo/bin` and leaves libpdfium and
`ocr/` behind. `sheaf-render` looks for libpdfium in `$PDFIUM_LIB_DIR`, beside
the executable (and its `lib/`), `vendor/`, then the system; `sheaf-ocr`
looks for `ocr/` the same way. A binstalled `sheaf` would fail to find PDFium
on most machines.

If we ever want binstall too, the metadata would look like this. Release
assets are named `<os>-<arch>`, not by target triple, hence the overrides.
The Windows zip has no top-level folder because `package.sh` zips
`$stage\*`.

```toml
[package.metadata.binstall]
pkg-fmt = "tgz"
# Never fall back to compiling the crate itself.
disabled-strategies = ["quick-install", "compile"]

[package.metadata.binstall.overrides.x86_64-unknown-linux-gnu]
pkg-url = "{ repo }/releases/download/v{ version }/sheaf-{ version }-linux-x86_64.tar.gz"
bin-dir = "sheaf-{ version }-linux-x86_64/{ bin }{ binary-ext }"

[package.metadata.binstall.overrides.x86_64-pc-windows-msvc]
pkg-url = "{ repo }/releases/download/v{ version }/sheaf-{ version }-windows-x86_64.zip"
pkg-fmt = "zip"
bin-dir = "{ bin }{ binary-ext }"
```

binstall builds URLs from the crate's crates.io version, so this only works
if the crate version equals the release tag.

## Shape

`cargo install sheaf-pdf` builds a binary named `sheaf-pdf`, not `sheaf`, so
it never shadows the app.

- `sheaf-pdf install [--version X]`
  - Download the bundle for the host from GitHub Releases and verify its
    SHA-256.
  - Unpack into a per-user dir: `~/.local/share/sheaf/<version>/` or
    `%LOCALAPPDATA%\sheaf\<version>\`.
  - Linux: symlink `sheaf` into `~/.local/bin`. `current_exe()` resolves the
    link through `/proc/self/exe`, so the app still finds its sidecars.
    Install the bundle's `sheaf.desktop`, `sheaf.png` (hicolor
    256x256/apps) and `sheaf.svg` (hicolor scalable/apps).
  - Windows: Start Menu shortcut, plus a `sheaf.cmd` shim on PATH (symlinks
    need admin or developer mode).
- `sheaf-pdf update`: check for a newer release, only when run. The app
  itself must never check (README: "no updater").
- `sheaf-pdf uninstall`: remove the version dirs, link or shortcut, desktop
  entry and icons.
- Probably `sheaf-pdf list` for installed versions, and keep the previous
  version until the new one launches.

Dependencies: an HTTP client (ureq + rustls), flate2, tar, zip, sha2, and a
per-user dirs crate. Small, quick to build.

## Decision: versioning

1. **Independent version (preferred for alpha).** The installer is a stable
   `0.1.0`. It asks the GitHub API for the latest release (or `--version`)
   and verifies against that release's `SHA256SUMS`. `cargo install
   sheaf-pdf` works without `@version`, and it is only republished when the
   installer changes.
2. **Lockstep version.** Each installer release embeds the exact bundle
   checksums. Since crates.io versions are immutable, that's stronger tamper
   protection, but it brings back the pre-release problem (`cargo install`
   skips pre-releases unless asked for, so users would need
   `sheaf-pdf@0.1.0-alpha.N`) and adds a publish step to every release.

Start with 1. Revisit 2, or GitHub artifact attestations, if tampering
becomes a real concern.

## Release workflow changes

- `release.yml` publishes `SHA256SUMS` alongside the bundles. The Linux and
  Windows jobs run separately, so either a final job that downloads both
  bundles and writes the file, or one `.sha256` per bundle.
- Publishing `sheaf-pdf` stays manual (`cargo publish`). A crates.io version
  can be yanked but never deleted, and the name is permanent once taken.

## Where it lives

`packaging/crates-io/sheaf-pdf/` (or `crates/sheaf-pdf/`), listed in the root
`Cargo.toml` `exclude` so it doesn't pull in the workspace's GPUI patches or
build in CI's app job. It needs its own `.gitignore` for `/target`.

## Audience

Developers and early testers: it needs a Rust toolchain. End users will need
real platform packaging later (MSI or winget on Windows, Flatpak or AppImage
on Linux). This is not a replacement for that.
