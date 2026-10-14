#!/usr/bin/env bash
# Build the plugins that ship with sheaf (plugins/*) for wasm32-wasip2 and
# stage each as <out>/<id>/{sheaf-plugin.toml,plugin.wasm,icons/}, the layout sheaf
# installs from. Debug builds of sheaf find target/plugins on their own;
# package.sh puts the set in plugins/ beside the binary.
#
# Usage: scripts/build-plugins.sh [out-dir]   # default target/plugins
set -euo pipefail
cd "$(dirname "$0")/.."

out="${1:-target/plugins}"
export CARGO_TARGET_DIR="$PWD/target/plugin-build"
mkdir -p "$out"
for dir in plugins/*/; do
  dir="${dir%/}"
  [[ -f "$dir/sheaf-plugin.toml" ]] || continue
  id="$(grep -m1 '^id' "$dir/sheaf-plugin.toml" | sed -E 's/.*"([^"]+)".*/\1/')"
  crate="$(grep -m1 '^name' "$dir/Cargo.toml" | sed -E 's/.*"([^"]+)".*/\1/')"
  cargo build --release --target wasm32-wasip2 --manifest-path "$dir/Cargo.toml"
  mkdir -p "$out/$id"
  cp "$dir/sheaf-plugin.toml" "$out/$id/"
  if [[ -d "$dir/icons" ]]; then
    mkdir -p "$out/$id/icons"
    cp "$dir"/icons/*.svg "$out/$id/icons/"
  fi
  cp "$CARGO_TARGET_DIR/wasm32-wasip2/release/${crate//-/_}.wasm" "$out/$id/plugin.wasm"
  echo "staged $out/$id"
done
