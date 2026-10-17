"""Render a page 1024 px wide in one document: first, again, the next
page, and then in a fresh document over the same bytes, which is what an
engine reopened for an edit pays.

The planning run (AMD EPYC 7302P) found a fresh document 1.0-1.7x slower
than a warm one ("fresh doc" against "again"): page 0 of the corpus's
forms/irs/f1040.pdf 17.9 against 10.4 ms, and of forms/ca-ftb/2025-540.pdf
11.0 against 4.9 ms.

Usage: python3 -I bench/warm.py libpdfium.so page file...
"""

import os
import sys
import time

# -I leaves the script's directory off sys.path.
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import pdfium


def main():
    if len(sys.argv) < 4:
        sys.exit("usage: python3 -I bench/warm.py libpdfium.so page file...")
    lib = pdfium.open_lib(sys.argv[1])
    page = int(sys.argv[2])

    def render_ms(doc, i):
        t = time.perf_counter()
        pdfium.render(lib, doc, i)
        return (time.perf_counter() - t) * 1000

    for path in sys.argv[3:]:
        with open(path, "rb") as f:
            data = f.read()
        d = pdfium.load(lib, data)
        if not d:
            print(f"{os.path.basename(path)}: PDFium couldn't load it ({lib.FPDF_GetLastError()})")
            continue
        last = lib.FPDF_GetPageCount(d) - 1
        i = min(page, last)
        first = render_ms(d, i)
        again = render_ms(d, i)
        other = render_ms(d, min(i + 1, last))
        lib.FPDF_CloseDocument(d)
        d = pdfium.load(lib, data)
        fresh = render_ms(d, i)
        lib.FPDF_CloseDocument(d)
        print(f"{os.path.basename(path)}: page {i} first {first:.1f} ms, again {again:.1f} ms, "
              f"next page {other:.1f} ms, fresh doc {fresh:.1f} ms")


if __name__ == "__main__":
    main()
