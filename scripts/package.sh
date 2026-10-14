#!/usr/bin/env bash
# Build a release binary and bundle it with libpdfium, licenses and a
# desktop entry into dist/sheaf-<version>-<os>-<arch>.{tar.gz,zip}.
#
# Usage: scripts/package.sh            # host platform
#        scripts/package.sh --no-build # reuse target/release/sheaf
#
# Runs on Linux, macOS, and Windows under Git Bash. Expects the PDFium
# library in vendor/pdfium/<os>-<arch>/ (scripts/fetch-pdfium.sh) and the
# OCR data in vendor/ocr/ (scripts/fetch-ocr-data.sh).
set -euo pipefail
cd "$(dirname "$0")/.."

build=1
[[ "${1:-}" == "--no-build" ]] && build=0

version="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')"
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64)  os=linux;   arch=x86_64;  lib=libpdfium.so;    exe=sheaf ;;
  Linux-aarch64) os=linux;   arch=aarch64; lib=libpdfium.so;    exe=sheaf ;;
  Darwin-arm64)  os=macos;   arch=aarch64; lib=libpdfium.dylib; exe=sheaf ;;
  Darwin-x86_64) os=macos;   arch=x86_64;  lib=libpdfium.dylib; exe=sheaf ;;
  MINGW64*|MSYS*|CYGWIN*) os=windows; arch=x86_64; lib=pdfium.dll; exe=sheaf.exe ;;
  *) echo "unsupported host $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac

vendor="vendor/pdfium/${os}-${arch}"
[[ -f "$vendor/$lib" ]] || { echo "missing $vendor/$lib — run scripts/fetch-pdfium.sh" >&2; exit 1; }
[[ -f vendor/ocr/tessdata/eng.traineddata ]] || { echo "missing vendor/ocr — run scripts/fetch-ocr-data.sh" >&2; exit 1; }

if (( build )); then
  cargo build --release -p sheaf-app
fi
# The plugins that ship with sheaf; installed into the user's plugin
# directory on first run.
scripts/build-plugins.sh target/plugins
[[ -f "target/release/$exe" ]] || { echo "missing target/release/$exe" >&2; exit 1; }

name="sheaf-${version}-${os}-${arch}"
stage="dist/$name"
rm -rf "$stage"
mkdir -p "$stage/licenses"
cp "target/release/$exe" "$stage/"
# The binary looks for the library next to itself first (sheaf-render).
cp "$vendor/$lib" "$stage/"
[[ -f "$vendor/LICENSE" ]] && cp "$vendor/LICENSE" "$stage/licenses/LICENSE-pdfium"
cp LICENSE-MIT LICENSE-APACHE "$stage/licenses/"
# OCR data goes in ocr/ beside the binary (sheaf-ocr looks there first),
# with the engines' and models' licenses.
mkdir -p "$stage/ocr/tessdata" "$stage/ocr/models" "$stage/ocr/lib"
cp vendor/ocr/tessdata/*.traineddata "$stage/ocr/tessdata/"
cp vendor/ocr/models/*.onnx vendor/ocr/models/*.txt "$stage/ocr/models/"
# ONNX Runtime is loaded from here at run time, only when PaddleOCR is used.
cp vendor/ocr/lib/libonnxruntime.* vendor/ocr/lib/onnxruntime.dll "$stage/ocr/lib/" 2>/dev/null || true
[[ -n "$(ls "$stage/ocr/lib")" ]] || { echo "missing vendor/ocr/lib — run scripts/fetch-ocr-data.sh" >&2; exit 1; }
cp packaging/licenses/* "$stage/licenses/"
cp -r target/plugins "$stage/plugins"
cp README.md "$stage/"
if [[ "$os" == linux ]]; then
  cp packaging/sheaf.desktop "$stage/"
  # Icon=sheaf: install these as hicolor 256x256/apps/sheaf.png and scalable/apps/sheaf.svg.
  cp crates/sheaf-app/resources/sheaf-256.png "$stage/sheaf.png"
  cp crates/sheaf-app/resources/sheaf.svg "$stage/"
fi

mkdir -p dist
if [[ "$os" == windows ]]; then
  out="dist/$name.zip"
  rm -f "$out"
  # Git Bash has no zip; PowerShell does.
  powershell.exe -NoProfile -Command \
    "Compress-Archive -Path '$(cygpath -w "$stage")\\*' -DestinationPath '$(cygpath -w "$out")' -Force"
else
  out="dist/$name.tar.gz"
  tar -C dist -czf "$out" "$name"
fi
echo "wrote $out"
ls -l "$out"
