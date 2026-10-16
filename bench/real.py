"""Load real files from memory with PDFium and render each one's page 0
1024 px wide, timing both: what opening a big real file costs, against
bench.py's synthetic one.

The list holds a path per line. A relative path is under the corpus:
$SHEAF_CORPUS, else ../pdf-corpus beside the repo, the first with a
manifest.tsv, as the tests look. Without one, it says so and exits 0.
pool-sample.txt is the planning sample: 400 SafeDocs pool files of 4.2 to
5.0 MB (up to 1,050 pages) and 2 SafeDocs forms of 370 and 466 pages, on
which the planning run (AMD EPYC 7302P) measured a median load of 0.38 ms
(max 15.7 ms) and a median page-0 render of 31.5 ms (max 1.94 s).

It prints the `top` files with the most pages (12 by default), then the
summary line.

Usage: python3 -I bench/real.py libpdfium.so list.txt [top]
"""

import os
import statistics
import sys
import time

# -I leaves the script's directory off sys.path.
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import pdfium

SKIP = "skipped: no corpus (set SHEAF_CORPUS, or check out ../pdf-corpus)"


def corpus():
    """The corpus root, or None."""
    beside = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "pdf-corpus"))
    for root in (os.environ.get("SHEAF_CORPUS"), beside):
        if root and os.path.isfile(os.path.join(root, "manifest.tsv")):
            return root
    return None


def main():
    if len(sys.argv) not in (3, 4):
        sys.exit("usage: python3 -I bench/real.py libpdfium.so list.txt [top]")
    top = int(sys.argv[3]) if len(sys.argv) > 3 else 12
    with open(sys.argv[2]) as f:
        paths = [line.strip() for line in f if line.strip()]
    root = None
    if not all(os.path.isabs(p) for p in paths):
        root = corpus()
        if root is None:
            print(SKIP)
            return

    lib = pdfium.open_lib(sys.argv[1])
    rows = []
    unreadable = failed = 0
    for path in paths:
        try:
            with open(os.path.join(root or "", path), "rb") as f:
                data = f.read()
        except OSError:
            unreadable += 1
            continue
        t = time.perf_counter()
        d = pdfium.load(lib, data)
        if not d:
            failed += 1
            continue
        n = lib.FPDF_GetPageCount(d)
        load = time.perf_counter() - t
        t = time.perf_counter()
        pdfium.render(lib, d, 0)
        render = time.perf_counter() - t
        lib.FPDF_CloseDocument(d)
        rows.append((n, len(data), load, render, path))
    if not rows:
        sys.exit(f"no file loaded ({unreadable} unreadable, {failed} PDFium couldn't load)")

    rows.sort(reverse=True)
    print("pages  MB     load_ms  page0_render_ms  file")
    for n, size, load, render, path in rows[:top]:
        print(f"{n:5d} {size / 1e6:6.1f} {load * 1000:9.2f} {render * 1000:10.2f}   {path}")
    loads = [r[2] for r in rows]
    renders = [r[3] for r in rows]
    skipped = f" ({unreadable} unreadable, {failed} PDFium couldn't load)" if unreadable or failed else ""
    print(f"{len(rows)} files{skipped}: median load {statistics.median(loads) * 1000:.2f} ms, "
          f"max load {max(loads) * 1000:.2f} ms; median page-0 render {statistics.median(renders) * 1000:.2f} ms, "
          f"max {max(renders) * 1000:.1f} ms")


if __name__ == "__main__":
    main()
