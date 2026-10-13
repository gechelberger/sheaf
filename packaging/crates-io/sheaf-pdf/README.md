# sheaf-pdf

[Sheaf](https://github.com/gechelberger/sheaf) is a fast, local-first PDF
editor for Windows and Linux. It is **alpha** software.

This crate is a placeholder. Sheaf can't be built from crates.io: its UI
framework (GPUI) is a git dependency, and the app needs the PDFium library
and OCR data beside its binary. Installing this crate gives you a `sheaf`
command that only prints these directions.

## Prebuilt bundles

Download a bundle for your platform from the
[releases page](https://github.com/gechelberger/sheaf/releases), unpack it
anywhere and run `sheaf` (`sheaf.exe` on Windows). No install step.

## Building from source

`cargo install --git` doesn't work: it skips the PDFium and OCR downloads
and ignores the repo's pinned toolchain. Clone the repo instead:

```sh
git clone https://github.com/gechelberger/sheaf
cd sheaf
scripts/fetch-pdfium.sh          # prebuilt libpdfium → vendor/pdfium/
scripts/fetch-ocr-data.sh        # OCR language data and models → vendor/ocr/
scripts/package.sh               # → dist/sheaf-<version>-<os>-<arch>.tar.gz
```

Unpack the bundle from `dist/` anywhere and run `sheaf`, or use
`cargo run --release -p sheaf-app` from the checkout.

On Linux (Debian/Ubuntu), install GPUI's system dependencies first:

```sh
sudo apt install build-essential clang lld cmake pkg-config libfontconfig-dev \
  libwayland-dev libx11-xcb-dev libxkbcommon-x11-dev libvulkan1 libasound2-dev \
  libglib2.0-dev libssl-dev libzstd-dev xdg-desktop-portal xdg-desktop-portal-gtk
```

On Windows, install Visual Studio 2022 (or the Build Tools) with the
"Desktop development with C++" workload, and run the scripts from Git Bash.

The first build compiles Tesseract from source and takes a while.
`cargo build --release -p sheaf-app --no-default-features` skips the OCR
engines for a quick build. See the
[main README](https://github.com/gechelberger/sheaf#building) for details.

## License

MIT OR Apache-2.0
