#!/usr/bin/env bash
# Download the OCR engines' data into vendor/ocr/ so sheaf-ocr can find it:
#   vendor/ocr/tessdata/eng.traineddata   Tesseract English (tessdata_fast)
#   vendor/ocr/models/*.onnx, *.txt       PaddleOCR PP-OCRv5 models for oar-ocr
# Usage: scripts/fetch-ocr-data.sh [--multilingual]
# The app looks in $SHEAF_OCR_DATA_DIR, ocr/ beside the binary, then vendor/ocr.
set -euo pipefail
cd "$(dirname "$0")/.."

TESSDATA_REF="${TESSDATA_REF:-main}"
OAR_MODELS_TAG="${OAR_MODELS_TAG:-v0.3.0}"
multilingual=0
for arg in "$@"; do
  case "$arg" in
    --multilingual) multilingual=1 ;;
    *) echo "unknown option $arg" >&2; exit 1 ;;
  esac
done

dest="vendor/ocr"
mkdir -p "$dest/tessdata" "$dest/models"

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

echo "installed into $dest:"; du -sh "$dest"/*; ls -l "$dest/models" "$dest/tessdata"
