# CI gate failure: `sheaf` dies with SIGILL on a CPU without AVX2

Status: fix implemented and partly verified on 2026-10-12; **uncommitted** on
branch `m5-ocr` on top of `ec89018`. Finish the verification below on the
next machine before committing.

## The problem

CI step "Starts on a CPU without AVX2 (Linux)" runs
`qemu-x86_64 -cpu Nehalem target/debug/sheaf --version` and failed:

```
qemu: uncaught target signal 4 (Illegal instruction) - core dumped
```

`--version` exits before any window or engine is touched, so the illegal
instruction runs **before `main`**, in a C++ global constructor. The only
component built for x86-64-v3 is the ONNX Runtime that the `ort` crate
downloads from pyke.io (its docs say so; sheaf's `ocr-oar` feature pulled it
in through `oar-ocr`'s default `download-binaries` feature). Linked
statically, its initialisers run at startup on every launch, so a user on a
pre-Haswell machine (the README's "older hardware" target) could not open
sheaf at all, even to use Tesseract. Tesseract and Leptonica are built by
`tesseract-rs` with baseline flags and runtime SIMD dispatch and are not
suspected.

This was the top risk in `todos/ocr.md`, and the gate exists to catch it.

## What's done (uncommitted working tree)

The planned fallback: don't link ONNX Runtime at all; load Microsoft's
generic x86-64 build (runtime CPU dispatch, no AVX2 requirement) as a shared
library, and only when the PaddleOCR engine is actually built.

- `crates/sheaf-ocr/Cargo.toml`: `oar-ocr` now has `default-features = false,
  features = ["simd"]` (drops `download-binaries`), and a new optional
  `ort = "=2.0.0-rc.13"` dependency with `features = ["load-dynamic"]`
  (same pin as `oar-ocr-core`; feature unification turns dynamic loading on
  for its copy too). The `oar` feature enables both.
- `crates/sheaf-ocr/src/data.rs`: `ORT_LIB` (`libonnxruntime.so` /
  `onnxruntime.dll` / `libonnxruntime.dylib`) and `onnxruntime_lib()`:
  `$ORT_DYLIB_PATH` (ort's own override), else `<data dir>/lib/<ORT_LIB>`,
  where the data dir is `$SHEAF_OCR_DATA_DIR`, `ocr/` beside the binary, or
  `vendor/ocr/`. `OarModels` gained a `runtime` path; `oar_models` fails
  with a "run scripts/fetch-ocr-data.sh" message when it is missing. Test
  updated.
- `crates/sheaf-ocr/src/oar.rs`: `load_runtime()` calls
  `ort::init_from(path)?.commit()` once per process (`OnceLock`) before the
  first `OAROCRBuilder::build`.
- `crates/sheaf-ocr/src/engine.rs`: the `NoAvx2` error and the
  `is_x86_feature_detected!("avx2")` check are removed; nothing needs them
  now.
- `scripts/fetch-ocr-data.sh`: also downloads
  `onnxruntime-{linux-x64,win-x64,osx-arm64,…}-1.28.0` from
  `github.com/microsoft/onnxruntime/releases` (1.28.0 is the version pyke's
  binaries track for `ort` rc.13; `ORT_VERSION` overrides) and installs the
  one real library file as `vendor/ocr/lib/<ORT_LIB>` plus its LICENSE. The
  Windows zip is extracted with PowerShell `Expand-Archive`, as
  `package.sh` already does.
- `scripts/package.sh`: copies `vendor/ocr/lib/*` to `<bundle>/ocr/lib/`
  and fails if it is empty.
- Docs: README (build paragraph, key-decisions row, packaging paragraph),
  `packaging/licenses/NOTICE-ocr.txt` (now "Microsoft's release build,
  loaded at run time"), the CI step comment, and an "Outcome of the AVX2
  risk" section in `todos/ocr.md`.

Verified so far on the Linux dev machine (WSL), after fetching the library:

- `cargo test -p sheaf-ocr --features tesseract,oar`: 15 unit tests pass.
- `cargo test -p sheaf-ocr --features tesseract,oar -- --ignored`: both
  engine smoke tests pass, i.e. PaddleOCR recognizes a PDFium-rendered page
  through the dynamically loaded `vendor/ocr/lib/libonnxruntime.so`.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.

Not yet verified: the `sheaf-app` build with these changes was still
compiling when work stopped, and `cargo fmt --all` has run but the
formatted files have not been re-checked by clippy since (they should be
identical in content).

## What's left

1. `scripts/fetch-ocr-data.sh` (one-time per machine; adds `vendor/ocr/lib`).
2. `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings`.
3. `cargo build -p sheaf-app`, then:
   - `ldd target/debug/sheaf` must list no `onnxruntime`, `libc++`,
     `tesseract` or `leptonica` (the CI "No dynamic engine libraries" gate).
   - With `qemu-user` installed: `qemu-x86_64 -cpu Nehalem target/debug/sheaf
     --version` must print the version. This is the failed gate; it could
     not be run on the WSL machine (no sudo for `apt install qemu-user`).
     If it still fails, the culprit is not ORT: bisect with
     `cargo build -p sheaf-app --no-default-features` (expect pass) and
     `--no-default-features --features ocr-tesseract` (if that fails,
     Tesseract/Leptonica need `-march` pinned in `tesseract-rs`'s cmake
     flags, or the tests must run the Tesseract engine under qemu to see
     which).
4. End-to-end: `cargo run -q -p sheaf-ocr --example scanify -- target/spike/synth.pdf /tmp/scan.pdf 150`
   (needs `cargo run -p sheaf-doc --example roundtrip` first), then
   `target/debug/sheaf --timing --selftest-ocr /tmp/out.pdf /tmp/scan.pdf`
   and `cargo run -q -p sheaf-ocr --example pdftext -- /tmp/out.pdf "spike page"`:
   three pages with 15 chars each and one hit per page; page 2 (landscape)
   at about `[713,170]`.
5. Optionally switch the default engine in the `--selftest-ocr` hook or
   the app settings to `oar` for one run to exercise the dynamic load
   through the app itself (the smoke test already does it in-process).
6. `cargo test --workspace`, then commit on `m5-ocr`, push, and watch the
   CI "Starts on a CPU without AVX2" and "No dynamic engine libraries"
   steps. On Windows CI the fetch script's PowerShell extraction path is
   unexercised so far.
7. If the Windows bundle needs it: `onnxruntime.dll` from Microsoft's zip
   depends only on the system CRT; no extra DLLs are expected.

## Why not the alternatives

- Keeping the static binary and checking `is_x86_feature_detected!` at
  runtime cannot work: the crash happens in static initialisers before any
  Rust code runs.
- Building ONNX Runtime from source for baseline x86-64 would fix it
  without a shared library but costs tens of minutes per CI run.
- `ort`'s `alternative-backend` (tract) would avoid native code entirely but
  is unverified with PP-OCRv5's operators and `oar-ocr` does not wire it.

## Trade-offs of the chosen fix

- The bundle grows by the 24 MB library (Linux) / about 12 MB (Windows).
- A user who deletes `ocr/lib` only loses the PaddleOCR engine; the dialog
  shows the missing-file message and Tesseract keeps working.
- `ORT_DYLIB_PATH` lets a developer point at any ONNX Runtime ≥ 1.17
  (`ort` is compiled against API 17 because `oar-ocr-core` enables no
  `api-*` feature).
