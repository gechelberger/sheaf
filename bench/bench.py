"""Time PDFium on the questions rendering an edited page raises (M10.5
proposal 16 and its §10.1), over gen.py's synthetic N-page file.

A  load from memory, count pages and close, as PdfiumEngine::open_bytes
   does; then the page-size walk, as Renderer::page_sizes did for the
   layout until M10.5 slice 3 read sizes from the source's page table
B  load page N/2, render it 1024 px wide and close; then at 320 px, a
   preview
C  load its text page and count its chars
D  the file plus an update section replacing page N/2's content (gen.py's
   incremental()), concatenated and loaded; the edited page's text, and
   renders of it and of an untouched page
E  the same two segments through FPDF_LoadCustomDocument, without the copy
F  a one-page scratch PDF made with ImportPagesByIndex + SaveAsCopy, then
   loaded and rendered
G  a text object inserted into page N/2 and FPDFPage_GenerateContent

The planning run (AMD EPYC 7302P, N = 7000) measured: A 2.0 ms, the walk
61 ms (8.7 us/page); B 5.2 ms, 1.8 ms at 320 px; C 1.3 ms; D 2.0-2.4 ms
with a 370-byte section, of which the copy 0.15 ms; E 2.16 ms; F 0.04 ms
and 1,096 bytes, 5.65 ms to load and render; G about 95 ms, growing with
the document's page count.

Usage: python3 -I bench/bench.py libpdfium.so N [img_every]
"""

import ctypes as C
import os
import statistics
import sys
import time

# -I leaves the script's directory off sys.path.
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import gen
import pdfium


def ms(t):
    return f"{t * 1000:.2f} ms"


def timed(f, reps=5):
    """f()'s last result, and the median and min of `reps` runs."""
    ts = []
    out = None
    for _ in range(reps):
        t = time.perf_counter()
        out = f()
        ts.append(time.perf_counter() - t)
    return out, statistics.median(ts), min(ts)


def main():
    if len(sys.argv) not in (3, 4):
        sys.exit("usage: python3 -I bench/bench.py libpdfium.so N [img_every]")
    lib = pdfium.open_lib(sys.argv[1])
    n = int(sys.argv[2])
    img_every = int(sys.argv[3]) if len(sys.argv) > 3 else 0

    def load(buf):
        d = pdfium.load(lib, buf)
        if not d:
            sys.exit(f"load failed: {lib.FPDF_GetLastError()}")
        return d

    def text(doc, i):
        p = lib.FPDF_LoadPage(doc, i)
        t = lib.FPDFText_LoadPage(p)
        count = lib.FPDFText_CountChars(t)
        s = "".join(chr(lib.FPDFText_GetUnicode(t, c)) for c in range(min(count, 40)))
        lib.FPDFText_ClosePage(t)
        lib.FPDF_ClosePage(p)
        return count, s

    print(f"building {n} pages (image every {img_every})")
    t = time.perf_counter()
    data, page_ids, objs, size, xref = gen.build(n, img_every=img_every)
    print(f"  built {len(data) / 1e6:.1f} MB in {ms(time.perf_counter() - t)}")

    def load_close():
        d = load(data)
        count = lib.FPDF_GetPageCount(d)
        lib.FPDF_CloseDocument(d)
        return count

    count, med, mn = timed(load_close)
    print(f"A. FPDF_LoadMemDocument64 + page count + close: median {ms(med)} (min {ms(mn)}), {count} pages")

    doc = load(data)
    sz = pdfium.SizeF()
    t = time.perf_counter()
    for i in range(n):
        lib.FPDF_GetPageSizeByIndexF(doc, i, C.byref(sz))
    walk = time.perf_counter() - t
    print(f"   page size walk: {ms(walk)} ({walk / n * 1e6:.1f} us/page)")

    k = n // 2
    _, med, mn = timed(lambda: pdfium.render(lib, doc, k))
    print(f"B. load page {k} + render 1024px wide + close: median {ms(med)} (min {ms(mn)})")
    _, med, mn = timed(lambda: pdfium.render(lib, doc, k, 320))
    print(f"   same at 320px (a preview): median {ms(med)}")
    (chars, s), med, mn = timed(lambda: text(doc, k))
    print(f"C. text page load + count chars: median {ms(med)}; {chars} chars, starts {s!r}")

    sec = gen.incremental(data, page_ids, objs, size, xref, k)

    def concat_load():
        buf = data + sec
        return load(buf), buf

    # d2_bytes keeps alive the buffer d2 reads.
    (d2, d2_bytes), med, mn = timed(concat_load)
    print(f"D. original + {len(sec)} B section: concat + load median {ms(med)} (min {ms(mn)})")
    _, med, _ = timed(lambda: data + sec)
    print(f"   (of which the concat copy: {ms(med)})")
    chars, s = text(d2, k)
    print(f"   page {k} text now {chars} chars: {s!r}")
    _, med, _ = timed(lambda: pdfium.render(lib, d2, k))
    print(f"   render edited page: median {ms(med)}")
    _, med, _ = timed(lambda: pdfium.render(lib, d2, 0))
    print(f"   render an untouched page: median {ms(med)}")

    # The base and the section stay two buffers; the callback copies each
    # block from whichever it falls in, or from both across the seam.
    seg_base = (C.c_ubyte * len(data)).from_buffer_copy(data)
    seg_sec = (C.c_ubyte * len(sec)).from_buffer_copy(sec)
    blen = len(data)

    @pdfium.GetBlock
    def get_block(param, pos, out, length):
        end = pos + length
        if pos < blen:
            a = min(end, blen) - pos
            C.memmove(out, C.addressof(seg_base) + pos, a)
            if end > blen:
                C.memmove(C.addressof(out.contents) + a, C.addressof(seg_sec), end - blen)
        else:
            C.memmove(out, C.addressof(seg_sec) + (pos - blen), length)
        return 1

    access = pdfium.FileAccess(blen + len(sec), get_block, None)

    def custom_load():
        d = lib.FPDF_LoadCustomDocument(C.byref(access), None)
        if not d:
            sys.exit("custom load failed")
        count = lib.FPDF_GetPageCount(d)
        lib.FPDF_CloseDocument(d)
        return count

    _, med, mn = timed(custom_load)
    print(f"E. FPDF_LoadCustomDocument (two segments, python callback): median {ms(med)} (min {ms(mn)})")

    def scratch():
        nd = lib.FPDF_CreateNewDocument()
        lib.FPDF_ImportPagesByIndex(nd, doc, (C.c_int * 1)(k), 1, 0)
        chunks = []

        @pdfium.WriteBlock
        def write_block(this, p, length):
            chunks.append(C.string_at(p, length))
            return 1

        lib.FPDF_SaveAsCopy(nd, C.byref(pdfium.FileWrite(1, write_block)), 0)
        lib.FPDF_CloseDocument(nd)
        return b"".join(chunks)

    scratch_bytes, med, mn = timed(scratch)
    print(f"F. scratch one-page doc via ImportPagesByIndex + SaveAsCopy: median {ms(med)}, {len(scratch_bytes)} B")

    def scratch_load_render():
        d = load(scratch_bytes)
        pdfium.render(lib, d, 0)
        lib.FPDF_CloseDocument(d)

    _, med, mn = timed(scratch_load_render)
    print(f"   load scratch from memory + render 1024px: median {ms(med)}")

    def edit_page():
        p = lib.FPDF_LoadPage(doc, k)
        o = lib.FPDFPageObj_NewTextObj(doc, b"Helvetica", 24.0)
        lib.FPDFText_SetText(o, "ADDED".encode("utf-16-le") + b"\0\0")
        lib.FPDFPageObj_Transform(o, 1, 0, 0, 1, 72, 100)
        lib.FPDFPage_InsertObject(p, o)
        t = time.perf_counter()
        lib.FPDFPage_GenerateContent(p)
        g = time.perf_counter() - t
        lib.FPDF_ClosePage(p)
        return g

    g, med, mn = timed(edit_page, reps=3)
    print(f"G. load page + insert text object + GenerateContent + close: median {ms(med)} (GenerateContent {ms(g)})")
    chars, _ = text(doc, k)
    print(f"   page {k} text after 3 edits: {chars} chars (edits persist in the open document)")
    lib.FPDF_CloseDocument(doc)
    lib.FPDF_CloseDocument(d2)


if __name__ == "__main__":
    main()
