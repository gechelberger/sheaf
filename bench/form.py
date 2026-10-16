"""Time the form-fill environment pdfium-render makes on every load, and a
page-0 render with FPDF_FFLDraw drawing the widgets: what an engine opened
for a form costs beyond the load itself. Medians of five rounds of load,
environment and render.

The planning run (AMD EPYC 7302P) measured FPDFDOC_InitFormFillEnvironment
at 0.14 ms or less on the forms it timed; over all of the corpus's
government forms (forms/ minus safedocs/) the most is 0.35 ms, for
uscis/i-9.pdf.

Usage: python3 -I bench/form.py libpdfium.so file...
"""

import ctypes as C
import os
import statistics
import sys
import time

# -I leaves the script's directory off sys.path.
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import pdfium

ROUNDS = 5


def main():
    if len(sys.argv) < 3:
        sys.exit("usage: python3 -I bench/form.py libpdfium.so file...")
    lib = pdfium.open_lib(sys.argv[1])
    # FPDF_FORMFILLINFO version 1 with every callback null; 1 KiB covers it.
    info = C.create_string_buffer(1024)
    C.c_int.from_buffer(info).value = 1

    def median_ms(ts):
        return f"{statistics.median(ts) * 1000:.2f} ms"

    for path in sys.argv[2:]:
        with open(path, "rb") as f:
            data = f.read()
        loads, envs, draws = [], [], []
        for _ in range(ROUNDS):
            t = time.perf_counter()
            d = pdfium.load(lib, data)
            if not d:
                break
            n = lib.FPDF_GetPageCount(d)
            loads.append(time.perf_counter() - t)
            t = time.perf_counter()
            form = lib.FPDFDOC_InitFormFillEnvironment(d, info)
            envs.append(time.perf_counter() - t)
            t = time.perf_counter()
            pdfium.render(lib, d, 0, form=form)
            draws.append(time.perf_counter() - t)
            lib.FPDFDOC_ExitFormFillEnvironment(form)
            lib.FPDF_CloseDocument(d)
        name = os.path.basename(path)
        if not loads:
            print(f"{name}: PDFium couldn't load it ({lib.FPDF_GetLastError()})")
            continue
        print(f"{name}: {n} pages, {len(data) / 1e6:.2f} MB; load {median_ms(loads)}, "
              f"form env {median_ms(envs)}, page 0 render+FFLDraw {median_ms(draws)}")


if __name__ == "__main__":
    main()
