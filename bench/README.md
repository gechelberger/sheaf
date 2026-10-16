# bench

The measurements behind M10.5's planning
([`todos/m10.5-edit-api.md`](../todos/m10.5-edit-api.md), proposal 16),
kept so they can be re-run, and the generators of the 7k fixtures the gate
harness runs on.

The scripts are Python 3, standard library only, run from the repo root
with `python3 -I`. The timing scripts call PDFium through ctypes
(`pdfium.py`), without sheaf's lock, and take the libpdfium to time as
their first argument: sheaf's is
`vendor/pdfium/linux-x86_64/libpdfium.so`, from `scripts/fetch-pdfium.sh`.
Each one's docstring says what it measures and what the planning run
measured. Fixtures go under `target/gate-7k/`, where the gate harness
looks for them; no PDF is committed.

| File | What | Run |
|---|---|---|
| `gen.py` | Generates the fixtures below, writing each through `out.pdf.<pid>.tmp` | `python3 -I bench/gen.py <fixture> N out.pdf` |
| `bench.py` | PDFium on the synthetic file: load, the size walk, render and text (A–C); an update section concatenated or as two segments (D, E); a scratch PDF (F); `GenerateContent` (G) | `python3 -I bench/bench.py vendor/pdfium/linux-x86_64/libpdfium.so 7000` |
| `real.py` | Load and page-0 render of every file in a list | `python3 -I bench/real.py vendor/pdfium/linux-x86_64/libpdfium.so bench/pool-sample.txt` |
| `warm.py` | A page rendered first, again, and in a fresh document | `python3 -I bench/warm.py vendor/pdfium/linux-x86_64/libpdfium.so 0 ../pdf-corpus/forms/irs/f1040.pdf ../pdf-corpus/forms/ca-ftb/2025-540.pdf` |
| `form.py` | The form-fill environment and `FPDF_FFLDraw` | `python3 -I bench/form.py vendor/pdfium/linux-x86_64/libpdfium.so ../pdf-corpus/forms/irs/f1040.pdf` |
| `pdfium.py` | The ctypes bindings the timing scripts share | |
| `pool-sample.txt` | The planning sample: 400 SafeDocs pool files of 4.2–5.0 MB (up to 1,050 pages) and 2 SafeDocs forms of 370 and 466 pages | `real.py`'s list |
| synthetic 7k | 7,000 text pages, one shared font, a classic xref: 3,510,145 bytes, sha256 `297917810fe2447c611d20bf8de377184cf641a122f49ab5357be5fd2008142c` | `python3 -I bench/gen.py synthetic 7000 target/gate-7k/synthetic-7000.pdf` |
| heavy 7k | 7,000 text pages in 70 sections, each with its own embedded subset font; 21,000 annotations (links, markups, 7,000 widgets) with appearances; 2,030 bookmarks; page labels; object streams and an xref stream. With `--font`'s default, DejaVuSans.ttf of sha256 `b4c632e3cdf9acc7f28758fb5a323c8524d7fc6660d46904d9b6cbe2809c419c`: 14,364,550 bytes, sha256 `cfe374eee27177512c978b7ad78fab4951e502be688e17263824f5952c34b218` | `python3 -I bench/gen.py heavy 7000 target/gate-7k/heavy-7000.pdf` |
| scanned 7k | 7,000 pages, each a 1275x1650 gray image, no text; a flat page tree and a classic xref: 496,665,679 bytes, sha256 `198bb7fc386450396150667ab0af0a4d5471de6134b3192ed91e45054ff42361` | `python3 -I bench/gen.py scanned 7000 target/gate-7k/scanned-7000.pdf` |

**The corpus.** `pool-sample.txt`'s paths are relative to the corpus root.
`real.py` resolves a relative path against `$SHEAF_CORPUS`, else
`../pdf-corpus` beside the repo, whichever first has a `manifest.tsv`, as
the tests do. Without either, it says it skipped and exits 0.

**The gate harness,** `crates/sheaf-ops/tests/gate_7k.rs`, times today's
geometry walk (a source's open: its cross-reference sections and page
table), open, render, saves, crop, Bates and flatten on the three
fixtures, and history at scale, and prints each beside slice 2's
baseline. The walk (from slice 3) and history assert their gates. It
makes a fixture that's missing or older than `gen.py` (in
`$SHEAF_GATE_DIR`, else `target/gate-7k/`):

```sh
cargo test -p sheaf-ops --release --test gate_7k -- --ignored --nocapture --test-threads=1
```

**The spikes.** Slice 2's spikes on overlays (encrypted sections, real
files by base class, render-time crops) and the measurements behind
GROUP, the engine caps and the fill rate were throwaway Rust, deleted at
the end of the slice. Their criteria, results and verdicts are in
[`docs/document-model.md`](../docs/document-model.md), "Rendering edited
pages: the spikes".
