"""PDFium through ctypes, shared by bench.py, real.py, warm.py and form.py.

The calls go straight to the libpdfium they are given (sheaf's is
vendor/pdfium/<os>-<arch>/libpdfium.so, from scripts/fetch-pdfium.sh),
without sheaf's lock and without the form-fill environment pdfium-render
makes on every load, so they time PDFium alone.
"""

import ctypes as C

vp = C.c_void_p
BOOL = C.c_int

# FPDF_RenderPageBitmap's flag to draw annotations' appearances.
FPDF_ANNOT = 0x01
WHITE = 0xFFFFFFFF


class SizeF(C.Structure):
    _fields_ = [("w", C.c_float), ("h", C.c_float)]


# FPDF_FILEWRITE's and FPDF_FILEACCESS's callbacks.
WriteBlock = C.CFUNCTYPE(C.c_int, vp, vp, C.c_ulong)
GetBlock = C.CFUNCTYPE(C.c_int, vp, C.c_ulong, C.POINTER(C.c_ubyte), C.c_ulong)


class FileWrite(C.Structure):
    _fields_ = [("version", C.c_int), ("WriteBlock", WriteBlock)]


class FileAccess(C.Structure):
    _fields_ = [("m_FileLen", C.c_ulong), ("m_GetBlock", GetBlock), ("m_Param", vp)]


# Every call the scripts make: (argtypes, restype).
SIGNATURES = {
    "FPDF_InitLibrary": ([], None),
    "FPDF_GetLastError": ([], C.c_ulong),
    "FPDF_LoadMemDocument64": ([vp, C.c_size_t, C.c_char_p], vp),
    "FPDF_LoadCustomDocument": ([C.POINTER(FileAccess), C.c_char_p], vp),
    "FPDF_CloseDocument": ([vp], None),
    "FPDF_GetPageCount": ([vp], C.c_int),
    "FPDF_GetPageSizeByIndexF": ([vp, C.c_int, C.POINTER(SizeF)], BOOL),
    "FPDF_LoadPage": ([vp, C.c_int], vp),
    "FPDF_ClosePage": ([vp], None),
    "FPDF_GetPageWidthF": ([vp], C.c_float),
    "FPDF_GetPageHeightF": ([vp], C.c_float),
    "FPDFBitmap_Create": ([C.c_int, C.c_int, C.c_int], vp),
    "FPDFBitmap_FillRect": ([vp, C.c_int, C.c_int, C.c_int, C.c_int, C.c_ulong], BOOL),
    "FPDFBitmap_Destroy": ([vp], None),
    "FPDF_RenderPageBitmap": ([vp, vp] + [C.c_int] * 6, None),
    "FPDFText_LoadPage": ([vp], vp),
    "FPDFText_CountChars": ([vp], C.c_int),
    "FPDFText_GetUnicode": ([vp, C.c_int], C.c_uint),
    "FPDFText_ClosePage": ([vp], None),
    "FPDF_CreateNewDocument": ([], vp),
    "FPDF_ImportPagesByIndex": ([vp, vp, C.POINTER(C.c_int), C.c_ulong, C.c_int], BOOL),
    "FPDF_SaveAsCopy": ([vp, C.POINTER(FileWrite), C.c_ulong], BOOL),
    "FPDFPageObj_NewTextObj": ([vp, C.c_char_p, C.c_float], vp),
    "FPDFText_SetText": ([vp, C.c_char_p], BOOL),
    "FPDFPageObj_Transform": ([vp] + [C.c_double] * 6, None),
    "FPDFPage_InsertObject": ([vp, vp], BOOL),
    "FPDFPage_GenerateContent": ([vp], BOOL),
    "FPDFDOC_InitFormFillEnvironment": ([vp, vp], vp),
    "FPDFDOC_ExitFormFillEnvironment": ([vp], None),
    "FORM_OnAfterLoadPage": ([vp, vp], None),
    "FORM_OnBeforeClosePage": ([vp, vp], None),
    "FPDF_FFLDraw": ([vp, vp, vp] + [C.c_int] * 6, None),
}


def open_lib(path):
    """The libpdfium at `path`, its SIGNATURES declared, initialized."""
    lib = C.CDLL(path)
    for name, (args, ret) in SIGNATURES.items():
        f = getattr(lib, name)
        f.argtypes, f.restype = args, ret
    lib.FPDF_InitLibrary()
    return lib


def load(lib, buf):
    """A document over `buf`, or None. PDFium reads `buf` in place, so it
    must outlive the document."""
    return lib.FPDF_LoadMemDocument64(buf, len(buf), None)


def render(lib, doc, index, width=1024, form=None):
    """Load page `index`, render it `width` px wide on white with its
    annotations' appearances, and close it. With a form-fill handle,
    FPDF_FFLDraw draws the widgets over it, as pdfium-render does."""
    page = lib.FPDF_LoadPage(doc, index)
    if form:
        lib.FORM_OnAfterLoadPage(page, form)
    w, h = lib.FPDF_GetPageWidthF(page), lib.FPDF_GetPageHeightF(page)
    height = max(1, int(width * h / max(w, 1)))
    bitmap = lib.FPDFBitmap_Create(width, height, 1)
    lib.FPDFBitmap_FillRect(bitmap, 0, 0, width, height, WHITE)
    lib.FPDF_RenderPageBitmap(bitmap, page, 0, 0, width, height, 0, FPDF_ANNOT)
    if form:
        lib.FPDF_FFLDraw(form, bitmap, page, 0, 0, width, height, 0, FPDF_ANNOT)
        lib.FORM_OnBeforeClosePage(page, form)
    lib.FPDFBitmap_Destroy(bitmap)
    lib.FPDF_ClosePage(page)
