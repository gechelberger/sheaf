#!/usr/bin/env bash
# Download the OCR engines' data into vendor/ocr/ so sheaf-ocr can find it:
#   vendor/ocr/tessdata/eng.traineddata   Tesseract English (tessdata_fast)
#   vendor/ocr/models/*.onnx, *.txt       PaddleOCR PP-OCRv5 models for oar-ocr
#   vendor/ocr/lib/libonnxruntime.so      Microsoft's ONNX Runtime (onnxruntime.dll
#                                         on Windows), loaded at run time
# Usage: scripts/fetch-ocr-data.sh [--multilingual]
# The app looks in $SHEAF_OCR_DATA_DIR, ocr/ beside the binary, then vendor/ocr.
set -euo pipefail
cd "$(dirname "$0")/.."

TESSDATA_REF="${TESSDATA_REF:-main}"
OAR_MODELS_TAG="${OAR_MODELS_TAG:-v0.3.0}"
# Same ONNX Runtime version as the `ort` crate's own prebuilt binaries.
ORT_VERSION="${ORT_VERSION:-1.28.0}"
multilingual=0
for arg in "$@"; do
  case "$arg" in
    --multilingual) multilingual=1 ;;
    *) echo "unknown option $arg" >&2; exit 1 ;;
  esac
done

dest="vendor/ocr"
mkdir -p "$dest/tessdata" "$dest/models" "$dest/lib"

fetch() { # url dest
  if [[ -s "$2" ]]; then
    echo "have $2"
  else
    echo "fetching $1"
    curl -fsSL "$1" -o "$2.part" && mv "$2.part" "$2"
  fi
}

tess="https://github.com/tesseract-ocr/tessdata_fast/raw/${TESSDATA_REF}"
fetch "$tess/eng.traineddata" "$dest/tessdata/eng.traineddata"
fetch "$tess/LICENSE" "$dest/tessdata/LICENSE"

oar="https://github.com/GreatV/oar-ocr/releases/download/${OAR_MODELS_TAG}"
models=(
  pp-ocrv5_mobile_det.onnx
  en_pp-ocrv5_mobile_rec.onnx
  ppocrv5_en_dict.txt
  pp-lcnet_x0_25_textline_ori.onnx
  pp-lcnet_x1_0_doc_ori.onnx
)
if [[ $multilingual == 1 ]]; then
  models+=(pp-ocrv5_mobile_rec.onnx ppocrv5_dict.txt)
fi
for m in "${models[@]}"; do
  fetch "$oar/$m" "$dest/models/$m"
done
fetch "https://raw.githubusercontent.com/PaddlePaddle/PaddleOCR/main/LICENSE" "$dest/models/LICENSE"

# ONNX Runtime: Microsoft's generic x86-64 build (runtime CPU dispatch, so
# no AVX2 requirement), unlike the x86-64-v3 static binaries `ort` downloads.
ort="https://github.com/microsoft/onnxruntime/releases/download/v${ORT_VERSION}"
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64)  ort_pkg="onnxruntime-linux-x64-${ORT_VERSION}.tgz";   ort_lib="libonnxruntime.so" ;;
  Linux-aarch64) ort_pkg="onnxruntime-linux-aarch64-${ORT_VERSION}.tgz"; ort_lib="libonnxruntime.so" ;;
  Darwin-arm64)  ort_pkg="onnxruntime-osx-arm64-${ORT_VERSION}.tgz";   ort_lib="libonnxruntime.dylib" ;;
  Darwin-x86_64) ort_pkg="onnxruntime-osx-x86_64-${ORT_VERSION}.tgz";  ort_lib="libonnxruntime.dylib" ;;
  MINGW64*|MSYS*|CYGWIN*) ort_pkg="onnxruntime-win-x64-${ORT_VERSION}.zip"; ort_lib="onnxruntime.dll" ;;
  *) echo "unknown host for ONNX Runtime; set it up by hand" >&2; ort_pkg="" ;;
esac
if [[ -n "$ort_pkg" && ! -s "$dest/lib/$ort_lib" ]]; then
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  fetch "$ort/$ort_pkg" "$tmp/$ort_pkg"
  mkdir -p "$tmp/x"
  if [[ "$ort_pkg" == *.zip ]]; then
    # Git Bash has no unzip; PowerShell does.
    powershell.exe -NoProfile -Command \
      "Expand-Archive -Path '$(cygpath -w "$tmp/$ort_pkg")' -DestinationPath '$(cygpath -w "$tmp/x")' -Force"
  else
    tar -xzf "$tmp/$ort_pkg" -C "$tmp/x"
  fi
  # The library is versioned (libonnxruntime.so.1.28.0 plus symlinks); keep
  # one real file under the plain name.
  found="$(find "$tmp/x" -type f \( -name "$ort_lib" -o -name "$ort_lib.*" \) | sort | head -1)"
  [[ -n "$found" ]] || { echo "no $ort_lib in $ort_pkg" >&2; exit 1; }
  cp "$found" "$dest/lib/$ort_lib"
  lic="$(find "$tmp/x" -maxdepth 2 -name LICENSE | head -1)"
  [[ -n "$lic" ]] && cp "$lic" "$dest/lib/LICENSE"
fi

echo "installed into $dest:"; du -sh "$dest"/*; ls -l "$dest/models" "$dest/tessdata" "$dest/lib"
