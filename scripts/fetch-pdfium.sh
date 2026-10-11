#!/usr/bin/env bash
# Download a prebuilt libpdfium from bblanchon/pdfium-binaries into
# vendor/pdfium/<os>-<arch>/ so sheaf-render can dlopen it.
# Usage: scripts/fetch-pdfium.sh [linux-x64|win-x64|mac-arm64|mac-x64]
set -euo pipefail

# Must match the pdfium_latest feature in pdfium-render (see its README).
PDFIUM_BUILD="${PDFIUM_BUILD:-7881}"
cd "$(dirname "$0")/.."

case "${1:-}" in
  "")
    case "$(uname -s)-$(uname -m)" in
      Linux-x86_64)  target=linux-x64;  dir=linux-x86_64 ;;
      Linux-aarch64) target=linux-arm64; dir=linux-aarch64 ;;
      Darwin-arm64)  target=mac-arm64;  dir=macos-aarch64 ;;
      Darwin-x86_64) target=mac-x64;    dir=macos-x86_64 ;;
      MINGW64*-x86_64|MSYS*-x86_64|CYGWIN*-x86_64) target=win-x64; dir=windows-x86_64 ;;
      *) echo "unknown host; pass a target explicitly" >&2; exit 1 ;;
    esac ;;
  linux-x64)  target=linux-x64;  dir=linux-x86_64 ;;
  linux-arm64) target=linux-arm64; dir=linux-aarch64 ;;
  win-x64)    target=win-x64;    dir=windows-x86_64 ;;
  mac-arm64)  target=mac-arm64;  dir=macos-aarch64 ;;
  mac-x64)    target=mac-x64;    dir=macos-x86_64 ;;
  *) echo "unknown target $1" >&2; exit 1 ;;
esac

url="https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F${PDFIUM_BUILD}/pdfium-${target}.tgz"
dest="vendor/pdfium/${dir}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "fetching $url"
curl -fsSL "$url" -o "$tmp/pdfium.tgz"
mkdir -p "$tmp/x" && tar -xzf "$tmp/pdfium.tgz" -C "$tmp/x"
mkdir -p "$dest"
# Only the shared library is needed at runtime; keep LICENSE for attribution.
find "$tmp/x" -type f \( -name 'libpdfium.so' -o -name 'libpdfium.dylib' -o -name 'pdfium.dll' -o -name 'LICENSE' \) \
  -exec cp {} "$dest/" \;
echo "installed into $dest:"; ls -l "$dest"
