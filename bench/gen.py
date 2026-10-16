"""Generate the 7k fixtures the bench scripts and the gate harness run on.

synthetic: text pages (55 lines each) sharing one Type1 font and one
  resource dictionary, a balanced page tree of 100-page groups and a
  classic xref. img_every > 0 also draws one shared 600x800 gray image on
  every img_every-th page. The bytes are pinned, so proposal 16's numbers
  re-run on the same input: at 7,000 pages it is 3,510,145 bytes, sha256
  297917810fe2447c611d20bf8de377184cf641a122f49ab5357be5fd2008142c, and
  with img_every 1, 4,123,353 bytes, sha256
  4ee9f637d3c96a9bd515c67bfeb77762d7ec504a6a47288797486d60847dfd9a.

heavy: what a long real document carries, on 100-page sections. Each
  section is a /Pages node with its own embedded subset of --font (a
  simple /TrueType font, glyph ids kept, unhinted) for its pages' 50 lines
  of text, and a bookmark with 28 under it. Every page has a link to
  another section, a markup annotation (square, highlight or note) and a
  text field's widget, every tenth a check box's, all with appearances.
  Page labels run roman, decimal, then an appendix. Every object but the
  streams is in an object stream, and the xref is a stream. The bytes
  depend on the font's: with DejaVuSans.ttf of sha256
  b4c632e3cdf9acc7f28758fb5a323c8524d7fc6660d46904d9b6cbe2809c419c, at
  7,000 pages it is 14,364,550 bytes, sha256
  cfe374eee27177512c978b7ad78fab4951e502be688e17263824f5952c34b218: 70
  embedded fonts, 21,000 annotations of which 7,000 widgets, and 2,030
  bookmarks.

scanned: a scan without a text layer, about 70 KB a page. Every page
  draws its own image, 1275x1650 8-bit gray (letter at 150 dpi, Flate),
  a copy of one of 32 pseudo-scans: white paper, speckle, and lines of
  word-shaped ink. One flat /Pages node and a classic xref. At 7,000 pages
  it is 496,665,679 bytes, sha256
  198bb7fc386450396150667ab0af0a4d5471de6134b3192ed91e45054ff42361.

bench.py imports build() and incremental(): the synthetic file, and an
update section over it that replaces one page's content.

Every fixture is seeded, so the same arguments give the same bytes. It is
written to out.pdf.<pid>.tmp and moved into place once complete, so a
killed run never leaves a partial one and two runs never share a
temporary file.

Usage: python3 -I bench/gen.py synthetic N out.pdf [img_every]
       python3 -I bench/gen.py heavy N out.pdf [--font font.ttf]
       python3 -I bench/gen.py scanned N out.pdf
"""

import argparse
import contextlib
import hashlib
import os
import random
import re
import struct
import time
import zlib


def build(n, compress=True, img_every=0):
    """The synthetic n-page file: (bytes, the page object numbers, each
    object's body by number, /Size, the xref's offset)."""
    objs = {}
    font = 3
    res = 4
    objs[font] = b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
    nid = 5
    img = None
    if img_every:
        # A 600x800 gray image, mildly compressible noise.
        random.seed(1)
        raw = bytes((random.randrange(200, 256) if (x // 7 + y // 9) % 3 else random.randrange(0, 80))
                    for y in range(800) for x in range(600))
        data = zlib.compress(raw, 6)
        img = nid; nid += 1
        objs[img] = (b"<< /Type /XObject /Subtype /Image /Width 600 /Height 800 /ColorSpace /DeviceGray "
                     b"/BitsPerComponent 8 /Filter /FlateDecode /Length %d >>\nstream\n" % len(data)) + data + b"\nendstream"
        objs[res] = b"<< /Font << /F1 3 0 R >> /XObject << /Im1 %d 0 R >> >>" % img
    else:
        objs[res] = b"<< /Font << /F1 3 0 R >> >>"
    per = 100
    groups = [list(range(i, min(i + per, n))) for i in range(0, n, per)]
    group_ids = []
    page_ids = []
    for g in groups:
        gid = nid; nid += 1
        group_ids.append(gid)
        kids = []
        for p in g:
            pid = nid; cid = nid + 1; nid += 2
            page_ids.append(pid)
            lines = [b"BT /F1 10 Tf 72 740 Td 12 TL"]
            for l in range(55):
                lines.append(b"(Page %d line %d: the quick brown fox jumps over the lazy dog 0123456789) Tj T*" % (p + 1, l + 1))
            lines.append(b"ET")
            if img and p % img_every == 0:
                lines.append(b"q 300 0 0 400 150 200 cm /Im1 Do Q")
            content = b"\n".join(lines)
            if compress:
                z = zlib.compress(content)
                objs[cid] = b"<< /Length %d /Filter /FlateDecode >>\nstream\n" % len(z) + z + b"\nendstream"
            else:
                objs[cid] = b"<< /Length %d >>\nstream\n" % len(content) + content + b"\nendstream"
            objs[pid] = b"<< /Type /Page /Parent %d 0 R /MediaBox [0 0 612 792] /Resources 4 0 R /Contents %d 0 R >>" % (gid, cid)
            kids.append(pid)
        objs[gid] = b"<< /Type /Pages /Parent 2 0 R /Kids [" + b" ".join(b"%d 0 R" % k for k in kids) + b"] /Count %d >>" % len(kids)
    objs[2] = b"<< /Type /Pages /Kids [" + b" ".join(b"%d 0 R" % g for g in group_ids) + b"] /Count %d >>" % n
    objs[1] = b"<< /Type /Catalog /Pages 2 0 R >>"
    size = nid
    out = bytearray(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")
    offsets = {}
    for i in sorted(objs):
        offsets[i] = len(out)
        out += b"%d 0 obj\n" % i + objs[i] + b"\nendobj\n"
    xref = len(out)
    out += b"xref\n0 %d\n" % size
    out += b"0000000000 65535 f \n"
    for i in range(1, size):
        if i in offsets:
            out += b"%010d 00000 n \n" % offsets[i]
        else:
            out += b"0000000000 65535 f \n"
    out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (size, xref)
    return bytes(out), page_ids, objs, size, xref


def incremental(base, page_ids, objs, size, prev_xref, page_index, text=b"EDITED PAGE"):
    """An update section replacing page `page_index`'s content."""
    pid = page_ids[page_index]
    old = objs[pid]
    new_cid = size
    content = b"BT /F1 24 Tf 72 700 Td (" + text + b") Tj ET"
    sec = bytearray()
    start = len(base)
    off = {}
    off[new_cid] = start + len(sec)
    sec += b"%d 0 obj\n<< /Length %d >>\nstream\n" % (new_cid, len(content)) + content + b"\nendstream\nendobj\n"
    new_page = re.sub(rb"/Contents \d+ 0 R", b"/Contents %d 0 R" % new_cid, old)
    off[pid] = start + len(sec)
    sec += b"%d 0 obj\n" % pid + new_page + b"\nendobj\n"
    xref = start + len(sec)
    sec += b"xref\n0 1\n0000000000 65535 f \n"
    sec += b"%d 1\n%010d 00000 n \n" % (pid, off[pid])
    sec += b"%d 1\n%010d 00000 n \n" % (new_cid, off[new_cid])
    sec += b"trailer\n<< /Size %d /Root 1 0 R /Prev %d >>\nstartxref\n%d\n%%%%EOF\n" % (size + 1, prev_xref, xref)
    return bytes(sec)


def refs(nums):
    return b" ".join(b"%d 0 R" % num for num in nums)


def dictionary(*entries):
    """A PDF dictionary of the entries given, skipping empty ones."""
    return b"<< " + b" ".join(e for e in entries if e) + b" >>"


def literal(b):
    """A PDF literal string of the bytes `b`."""
    return b"(" + b.replace(b"\\", b"\\\\").replace(b"(", b"\\(").replace(b")", b"\\)") + b")"


def text(s):
    """A PDF text string: literal if ASCII, else UTF-16BE with its BOM."""
    if s.isascii():
        return literal(s.encode())
    return b"<FEFF%s>" % s.encode("utf-16-be").hex().upper().encode()


DEJAVU = "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"

# A composite glyph component's flags (OpenType's glyf table).
ARGS_ARE_WORDS = 0x0001
HAVE_SCALE = 0x0008
MORE_COMPONENTS = 0x0020
HAVE_XY_SCALE = 0x0040
HAVE_2X2 = 0x0080
HAVE_INSTRUCTIONS = 0x0100


def checksum(data):
    """An sfnt table's checksum: its big-endian uint32s summed, mod 2^32."""
    data = bytes(data) + b"\0" * (-len(data) % 4)
    return sum(struct.unpack(">%dI" % (len(data) // 4), data)) & 0xFFFFFFFF


def sfnt(tables):
    """A TrueType font file of `tables`, tag to data, its checksums set."""
    tags = sorted(tables)
    power = 1 << (len(tags).bit_length() - 1)
    out = bytearray(struct.pack(">IHHHH", 0x00010000, len(tags), 16 * power, power.bit_length() - 1,
                                16 * (len(tags) - power)))
    body = bytearray()
    at = {}
    for tag in tags:
        at[tag] = 12 + 16 * len(tags) + len(body)
        out += struct.pack(">4sIII", tag, checksum(tables[tag]), at[tag], len(tables[tag]))
        body += tables[tag] + b"\0" * (-len(tables[tag]) % 4)
    out += body
    # head's checkSumAdjustment, figured with it zeroed.
    struct.pack_into(">I", out, at[b"head"] + 8, (0xB1B0AFBA - checksum(out)) & 0xFFFFFFFF)
    return bytes(out)


class TrueType:
    """A TrueType font file, read for what a simple PDF font embeds, and
    GID-preserving subsets of it."""

    # The tables a /FontFile2 needs; the layout and hinting tables go.
    KEEP = (b"OS/2", b"cmap", b"glyf", b"head", b"hhea", b"hmtx", b"loca", b"maxp", b"name", b"post")

    def __init__(self, path):
        with open(path, "rb") as f:
            data = f.read()
        self.sha256 = hashlib.sha256(data).hexdigest()
        if data[:4] not in (b"\0\1\0\0", b"true"):
            raise ValueError("no TrueType header")
        self.tables = {}
        for i in range(struct.unpack_from(">H", data, 4)[0]):
            tag, _, off, length = struct.unpack_from(">4sIII", data, 12 + 16 * i)
            if tag in self.KEEP:
                self.tables[tag] = data[off:off + length]
        missing = [tag.decode() for tag in self.KEEP if tag not in self.tables]
        if missing:
            raise ValueError("no " + ", ".join(missing) + " table")
        head = self.tables[b"head"]
        self.units = struct.unpack_from(">H", head, 18)[0]
        self.bbox = struct.unpack_from(">4h", head, 36)
        hhea = self.tables[b"hhea"]
        self.ascent, self.descent = struct.unpack_from(">hh", hhea, 4)
        metrics = struct.unpack_from(">H", hhea, 34)[0]
        self.advances = struct.unpack_from(">%dH" % (2 * metrics), self.tables[b"hmtx"])[::2]
        glyphs = struct.unpack_from(">H", self.tables[b"maxp"], 4)[0]
        if struct.unpack_from(">h", head, 50)[0]:
            self.loca = struct.unpack_from(">%dI" % (glyphs + 1), self.tables[b"loca"])
        else:
            # A short loca holds offsets / 2.
            self.loca = [2 * off for off in struct.unpack_from(">%dH" % (glyphs + 1), self.tables[b"loca"])]
        self.cmap = self._cmap()
        self.name = self._postscript_name()
        h = self.glyph(self.cmap.get(ord("H"), 0))
        self.cap_height = struct.unpack_from(">h", h, 8)[0] if h else self.ascent

    def _cmap(self):
        """The (3,1) format 4 cmap, code point to glyph id."""
        cmap = self.tables[b"cmap"]
        for i in range(struct.unpack_from(">H", cmap, 2)[0]):
            platform, encoding, off = struct.unpack_from(">HHI", cmap, 4 + 8 * i)
            if (platform, encoding) == (3, 1) and struct.unpack_from(">H", cmap, off)[0] == 4:
                break
        else:
            raise ValueError("no (3,1) format 4 cmap")
        segs = struct.unpack_from(">H", cmap, off + 6)[0] // 2
        ends = struct.unpack_from(">%dH" % segs, cmap, off + 14)
        starts = struct.unpack_from(">%dH" % segs, cmap, off + 16 + 2 * segs)
        deltas = struct.unpack_from(">%dH" % segs, cmap, off + 16 + 4 * segs)
        ranges_at = off + 16 + 6 * segs
        ranges = struct.unpack_from(">%dH" % segs, cmap, ranges_at)
        codes = {}
        for i in range(segs):
            for c in range(starts[i], ends[i] + 1):
                if ranges[i]:
                    # idRangeOffset counts from its own slot into glyphIdArray.
                    g = struct.unpack_from(">H", cmap, ranges_at + 2 * i + ranges[i] + 2 * (c - starts[i]))[0]
                    g = (g + deltas[i]) & 0xFFFF if g else 0
                else:
                    g = (c + deltas[i]) & 0xFFFF
                if g:
                    codes[c] = g
        return codes

    def _postscript_name(self):
        name = self.tables[b"name"]
        count, strings = struct.unpack_from(">HH", name, 2)
        for i in range(count):
            platform, _, _, name_id, length, off = struct.unpack_from(">6H", name, 6 + 12 * i)
            if name_id == 6:
                raw = name[strings + off:strings + off + length]
                return raw.decode("utf-16-be" if platform in (0, 3) else "latin-1")
        raise ValueError("no PostScript name")

    def glyph(self, gid):
        return self.tables[b"glyf"][self.loca[gid]:self.loca[gid + 1]]

    def advance(self, gid):
        return self.advances[min(gid, len(self.advances) - 1)]

    @staticmethod
    def components(glyph):
        """A composite glyph's components as (their flags' offset, glyph
        id), and where the last ends; none for a simple or empty glyph."""
        parts = []
        at = 10
        if not glyph or struct.unpack_from(">h", glyph)[0] >= 0:
            return parts, at
        while True:
            flags, gid = struct.unpack_from(">HH", glyph, at)
            parts.append((at, gid))
            at += 8 if flags & ARGS_ARE_WORDS else 6
            if flags & HAVE_SCALE:
                at += 2
            elif flags & HAVE_XY_SCALE:
                at += 4
            elif flags & HAVE_2X2:
                at += 8
            if not flags & MORE_COMPONENTS:
                return parts, at

    def unhinted(self, gid):
        """Glyph `gid` without its instructions."""
        glyph = self.glyph(gid)
        if not glyph:
            return glyph
        contours = struct.unpack_from(">h", glyph)[0]
        if contours >= 0:
            at = 10 + 2 * contours
            length = struct.unpack_from(">H", glyph, at)[0]
            return glyph[:at] + b"\0\0" + glyph[at + 2 + length:]
        parts, end = self.components(glyph)
        out = bytearray(glyph[:end])
        for at, _ in parts:
            struct.pack_into(">H", out, at, struct.unpack_from(">H", out, at)[0] & ~HAVE_INSTRUCTIONS)
        return bytes(out)

    def subset(self, gids):
        """The font file with every glyph but .notdef, `gids` and their
        components emptied, glyph ids kept, its hinting and glyph names
        dropped, and a long loca."""
        keep = {0, *gids}
        todo = list(keep)
        while todo:
            for _, gid in self.components(self.glyph(todo.pop()))[0]:
                if gid not in keep:
                    keep.add(gid)
                    todo.append(gid)
        glyf = bytearray()
        loca = []
        for gid in range(len(self.loca) - 1):
            loca.append(len(glyf))
            if gid in keep:
                glyf += self.unhinted(gid)
                glyf += b"\0" * (-len(glyf) % 4)
        loca.append(len(glyf))
        tables = dict(self.tables)
        tables[b"glyf"] = bytes(glyf)
        tables[b"loca"] = struct.pack(">%dI" % len(loca), *loca)
        head = self.tables[b"head"]
        tables[b"head"] = head[:8] + b"\0\0\0\0" + head[12:50] + b"\0\1" + head[52:]
        # Format 3: no glyph names.
        tables[b"post"] = b"\0\3\0\0" + self.tables[b"post"][4:32]
        return sfnt(tables)


def subset_tag(i):
    """Subset i's six-letter tag: AAAAAA, AAAAAB, ..."""
    letters = ""
    for _ in range(6):
        i, r = divmod(i, 26)
        letters = chr(ord("A") + r) + letters
    return letters.encode()


class Objects:
    """A file's objects, numbered as they are made; stream objects apart,
    since only the others can go in object streams."""

    def __init__(self):
        self.next = 1
        self.plain = {}
        self.streams = {}

    def new(self):
        self.next += 1
        return self.next - 1

    def put(self, num, body):
        self.plain[num] = body
        return num

    def stream(self, num, entries, data, compress=False):
        if compress:
            data = zlib.compress(data)
            entries += b" /Filter /FlateDecode"
        self.streams[num] = b"<< %s /Length %d >>\nstream\n" % (entries.strip(), len(data)) + data + b"\nendstream"
        return num


def with_object_streams(objs, trailer):
    """The file of `objs`: the streams direct, the rest packed 100 to an
    object stream, and an xref stream whose dictionary adds `trailer`."""
    out = bytearray(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")
    # Each object's xref stream entry: (type, field 2, field 3).
    entries = {0: (0, 0, 65535)}
    for num in sorted(objs.streams):
        entries[num] = (1, len(out), 0)
        out += b"%d 0 obj\n" % num + objs.streams[num] + b"\nendobj\n"
    plain = sorted(objs.plain)
    for i in range(0, len(plain), 100):
        packed = plain[i:i + 100]
        stm = objs.new()
        index = []
        at = 0
        for k, num in enumerate(packed):
            entries[num] = (2, stm, k)
            index.append(b"%d %d" % (num, at))
            at += len(objs.plain[num]) + 1
        first = b" ".join(index) + b"\n"
        data = zlib.compress(first + b"\n".join(objs.plain[num] for num in packed))
        entries[stm] = (1, len(out), 0)
        out += (b"%d 0 obj\n<< /Type /ObjStm /N %d /First %d /Filter /FlateDecode /Length %d >>\nstream\n"
                % (stm, len(packed), len(first), len(data)) + data + b"\nendstream\nendobj\n")
    xref = objs.new()
    entries[xref] = (1, len(out), 0)
    data = zlib.compress(b"".join(struct.pack(">BIH", *entries[i]) for i in range(objs.next)))
    out += (b"%d 0 obj\n<< /Type /XRef /Size %d /W [1 4 2] %s /Filter /FlateDecode /Length %d >>\nstream\n"
            % (xref, objs.next, trailer, len(data)) + data + b"\nendstream\nendobj\n")
    out += b"startxref\n%d\n%%%%EOF\n" % entries[xref][1]
    return bytes(out)


SECTION = 100

# The body text's words.
WORDS = ("the of and to in is that for it as with was on be by this are from at or an which have not they but "
         "all were their has been one more will would there can when who also into other its only new some time "
         "could these two may first then any like now over such our out even most made after many before must "
         "through back years where much your way well down should because each just those people how too little "
         "state good very make world still own see men work long get here between both life being under never day "
         "same another know while last might great old year off come since against go came right used take three "
         "quick brown fox jumps lazy dog extra zone").split()

# Words that vary each section's glyphs: its pages' headings name six.
SECTION_WORDS = ("Ångström", "Æsir", "Œuvre", "Zürich", "Straße", "Señor", "café", "naïve", "façade",
                 "jalapeño", "Øresund", "Þórr", "Ýmir", "fiancée", "Göteborg", "Kraków", "São", "Málaga",
                 "Reykjavík", "Ålesund", "€42", "£17", "¥300", "§4.2", "¶7", "©2026", "Brand®", "Mark™", "±0.5",
                 "½", "¾", "12µs", "40°C", "3×4", "9÷3", "«quoted»", "“curly”", "‘single’", "A—B", "wait…",
                 "•item", "note†", "‰", "QUARTERLY", "REVENUE", "Appendix", "Exhibit", "XYZ-42", "JKL#9", "q@w",
                 "[ref]", "{set}", "a|b", "x^2", "~1", "50%", "1+1=2", "Q&A", "<tag>", "(sic)", "back\\slash",
                 "Œil", "Écu", "Ïle", "Ñu")

AUTHORS = ("A. Reviewer", "B. Editor", "C. Counsel")


def build_heavy(n, font):
    """The heavy n-page file, embedding subsets of `font` (a TrueType),
    and what it holds, a fact a line."""
    rng = random.Random(2)
    objs = Objects()
    catalog, root, info, helv, zadb, form, outlines = (objs.new() for _ in range(7))
    sections = [range(i, min(i + SECTION, n)) for i in range(0, n, SECTION)]
    nodes = [objs.new() for _ in sections]
    tops = [objs.new() for _ in sections]
    pages = [objs.new() for _ in range(n)]

    def em(v):
        return round(v * 1000 / font.units)

    fields = []
    kinds = {b"Square": 0, b"Highlight": 0, b"Text": 0}
    boxes = bookmarks = visible = 0
    for s, section in enumerate(sections):
        extras = rng.sample(SECTION_WORDS, 6)
        resources = objs.new()
        used = set()
        for p in section:
            lines = [f"Section {s + 1}, page {p + 1}: " + " ".join(extras)]
            for _ in range(49):
                words = rng.choices(WORDS, k=10)
                if rng.random() < 0.2:
                    words[rng.randrange(10)] = rng.choice(extras)
                lines.append(" ".join(words))
            used.update(*lines)
            content = objs.stream(objs.new(), b"", b"BT /F1 10 Tf 72 740 Td 13 TL\n" + b"\n".join(
                literal(line.encode("cp1252")) + b" Tj T*" for line in lines) + b"\nET", compress=True)

            # A link to a page in another section.
            other = sections[(s + 1 + p % (len(sections) - 1)) % len(sections)] if len(sections) > 1 else section
            target = pages[other[p % len(other)]]
            dest = b"[%d 0 R /XYZ 72 792 null]" % target if p % 2 else b"[%d 0 R /Fit]" % target
            link = objs.put(objs.new(), b"<< /Type /Annot /Subtype /Link /Rect [70 736 340 751] /Border [0 0 0] "
                                        b"/Dest %s >>" % dest)

            kind = (b"Square", b"Highlight", b"Text")[p % 3]
            kinds[kind] += 1
            if kind == b"Square":
                rect, entries = b"[500 600 580 680]", b"/C [0.8 0 0] /BS << /W 1 >>"
                ap = objs.stream(objs.new(), b"/Type /XObject /Subtype /Form /BBox [0 0 80 80]",
                                 b"0.8 0 0 RG 1 w 0.5 0.5 79 79 re S")
            elif kind == b"Highlight":
                rect, entries = b"[72 672 300 685]", b"/C [1 1 0] /QuadPoints [72 685 300 685 72 672 300 672]"
                ap = objs.stream(objs.new(), b"/Type /XObject /Subtype /Form /BBox [0 0 228 13] "
                                 b"/Resources << /ExtGState << /G0 << /BM /Multiply >> >> >>",
                                 b"/G0 gs 1 1 0 rg 0 0 228 13 re f")
            else:
                rect, entries = b"[560 740 580 760]", b"/C [1 0.85 0.2] /Name /Comment /Open false"
                ap = objs.stream(objs.new(), b"/Type /XObject /Subtype /Form /BBox [0 0 20 20]",
                                 b"1 0.85 0.2 rg 0 G 0.5 w 0.5 0.5 19 19 re B 4 6 m 16 6 l 4 10 m 16 10 l "
                                 b"4 14 m 12 14 l S")
            note = f"{kind.decode()} {p + 1}: " + " ".join(rng.choices(WORDS, k=6))
            date = b"(D:2026%02d%02d%02d%02d00Z)" % (1 + p % 12, 1 + p % 28, p % 24, p % 60)
            markup = objs.put(objs.new(), b"<< /Type /Annot /Subtype /%s /Rect %s %s /NM %s /T %s /Contents %s "
                                          b"/M %s /F 4 /P %d 0 R /AP << /N %d 0 R >> >>"
                              % (kind, rect, entries, text(f"heavy-{p + 1}"), text(AUTHORS[p % 3]), text(note),
                                 date, pages[p], ap))

            # A field's widget: a text field, every tenth a check box.
            field = text(f"p{p + 1}")
            if p % 10 == 9:
                state = b"/Yes" if p // 10 % 2 else b"/Off"
                border = b"0 G 0.5 w 0.25 0.25 13.5 13.5 re S"
                yes = objs.stream(objs.new(), b"/Type /XObject /Subtype /Form /BBox [0 0 14 14]",
                                  border + b" 1.5 w 3 7 m 6 3 l 11 11 l S")
                off = objs.stream(objs.new(), b"/Type /XObject /Subtype /Form /BBox [0 0 14 14]", border)
                widget = objs.put(objs.new(), b"<< /Type /Annot /Subtype /Widget /FT /Btn /T %s /V %s /AS %s "
                                              b"/DA (/ZaDb 0 Tf 0 g) /Rect [72 40 86 54] /F 4 /P %d 0 R "
                                              b"/AP << /N << /Yes %d 0 R /Off %d 0 R >> >> >>"
                                  % (field, state, state, pages[p], yes, off))
                boxes += 1
            else:
                value = text(f"Value {p + 1} {rng.choice(WORDS)}")
                ap = objs.stream(objs.new(), b"/Type /XObject /Subtype /Form /BBox [0 0 228 20] "
                                 b"/Resources << /Font << /Helv %d 0 R >> >>" % helv,
                                 b"/Tx BMC q 1 1 226 18 re W n BT /Helv 10 Tf 0 g 2 6 Td %s Tj ET Q EMC" % value)
                widget = objs.put(objs.new(), b"<< /Type /Annot /Subtype /Widget /FT /Tx /T %s /V %s "
                                              b"/DA (/Helv 10 Tf 0 g) /Rect [72 40 300 60] /F 4 /P %d 0 R "
                                              b"/AP << /N %d 0 R >> >>" % (field, value, pages[p], ap))
            fields.append(widget)
            objs.put(pages[p], b"<< /Type /Page /Parent %d 0 R /MediaBox [0 0 612 792] /Resources %d 0 R "
                               b"/Contents %d 0 R /Annots [%s] >>"
                     % (nodes[s], resources, content, refs((link, markup, widget))))
        objs.put(nodes[s], b"<< /Type /Pages /Parent %d 0 R /Kids [%s] /Count %d >>"
                 % (root, refs(pages[p] for p in section), len(section)))

        # The section's font: a subset of what its pages draw.
        chars = "".join(sorted(used))
        gids = {code: font.cmap.get(ord(c), 0) for code, c in zip(chars.encode("cp1252"), chars)}
        first, last = min(gids), max(gids)
        widths = b" ".join(b"%d" % (em(font.advance(gids[c])) if c in gids else 0) for c in range(first, last + 1))
        name = subset_tag(s) + b"+" + font.name.encode()
        program = font.subset(gids.values())
        fontfile = objs.stream(objs.new(), b"/Length1 %d" % len(program), program, compress=True)
        descriptor = objs.put(objs.new(), b"<< /Type /FontDescriptor /FontName /%s /Flags 32 /FontBBox [%s] "
                                          b"/ItalicAngle 0 /Ascent %d /Descent %d /CapHeight %d /StemV 80 "
                                          b"/FontFile2 %d 0 R >>"
                              % (name, b" ".join(b"%d" % em(v) for v in font.bbox), em(font.ascent),
                                 em(font.descent), em(font.cap_height), fontfile))
        f1 = objs.put(objs.new(), b"<< /Type /Font /Subtype /TrueType /BaseFont /%s /FirstChar %d /LastChar %d "
                                  b"/Widths [%s] /Encoding /WinAnsiEncoding /FontDescriptor %d 0 R >>"
                      % (name, first, last, widths, descriptor))
        objs.put(resources, b"<< /Font << /F1 %d 0 R >> >>" % f1)

        # Its bookmarks: one for the section, over up to 28.
        kids = [objs.new() for _ in range(min(28, len(section)))]
        for k, kid in enumerate(kids):
            page = pages[section[k * len(section) // len(kids)]]
            heading = f"{s + 1}.{k + 1} " + " ".join(rng.choices(WORDS, k=3)).capitalize()
            objs.put(kid, dictionary(
                b"/Title %s /Parent %d 0 R" % (text(heading), tops[s]),
                b"/Prev %d 0 R" % kids[k - 1] if k else b"",
                b"/Next %d 0 R" % kids[k + 1] if k + 1 < len(kids) else b"",
                b"/Dest [%d 0 R /XYZ 72 792 null]" % page if k % 2 else b"/Dest [%d 0 R /Fit]" % page,
                b"/C [0 0 0.6]" if k % 3 == 0 else b"",
                b"/F %d" % (1 + k // 4 % 3) if k % 4 == 1 else b""))
        # Every fifth section's bookmark is open.
        shown = len(kids) if s % 5 == 0 else 0
        objs.put(tops[s], dictionary(
            b"/Title %s /Parent %d 0 R" % (text(f"Section {s + 1}: {extras[0]}"), outlines),
            b"/Prev %d 0 R" % tops[s - 1] if s else b"",
            b"/Next %d 0 R" % tops[s + 1] if s + 1 < len(sections) else b"",
            b"/First %d 0 R /Last %d 0 R /Count %d" % (kids[0], kids[-1], shown or -len(kids)),
            b"/Dest [%d 0 R /Fit]" % pages[section[0]]))
        bookmarks += 1 + len(kids)
        visible += 1 + shown

    # Page labels: roman front matter, then decimal from 1, then an appendix.
    front, appendix = min(12, n), n - n // 35
    labels = [b"0 << /S /r >>"]
    if n > front:
        labels.append(b"%d << /S /D >>" % front)
    if front < appendix < n:
        labels.append(b"%d << /S /D /P (A-) >>" % appendix)

    objs.put(catalog, b"<< /Type /Catalog /Pages %d 0 R /Outlines %d 0 R /PageMode /UseOutlines "
                      b"/PageLabels << /Nums [%s] >> /AcroForm %d 0 R >>"
             % (root, outlines, b" ".join(labels), form))
    objs.put(root, b"<< /Type /Pages /Kids [%s] /Count %d >>" % (refs(nodes), n))
    objs.put(info, b"<< /Title %s /Producer (sheaf bench/gen.py) >>" % text(f"Heavy {n}-page fixture"))
    objs.put(helv, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>")
    objs.put(zadb, b"<< /Type /Font /Subtype /Type1 /BaseFont /ZapfDingbats >>")
    objs.put(form, b"<< /Fields [%s] /DR << /Font << /Helv %d 0 R /ZaDb %d 0 R >> >> /DA (/Helv 0 Tf 0 g) >>"
             % (refs(fields), helv, zadb))
    objs.put(outlines, b"<< /Type /Outlines /First %d 0 R /Last %d 0 R /Count %d >>" % (tops[0], tops[-1], visible))

    packed = len(objs.plain)
    ident = hashlib.md5(b"heavy %d" % n).hexdigest().upper().encode()
    data = with_object_streams(objs, b"/Root %d 0 R /Info %d 0 R /ID [<%s> <%s>]" % (catalog, info, ident, ident))
    return data, [
        f"{len(sections)} sections, each with its own {font.name} subset (font sha256 {font.sha256})",
        (f"{3 * n} annotations: {n} links; {n} markups ({kinds[b'Square']} square, {kinds[b'Highlight']} "
         f"highlight, {kinds[b'Text']} note); {n} widgets ({n - boxes} text fields, {boxes} check boxes)"),
        f"{bookmarks} bookmarks, {len(labels)} page-label ranges",
        (f"{objs.next - 1} objects: {packed} in {-(-packed // 100)} object streams, the rest streams, "
         f"and an xref stream"),
    ]


SCAN_W, SCAN_H = 1275, 1650  # US letter at 150 dpi
SCAN_POOL = 32

# Random bytes to gray: ink is two dark grays on 72 of 256, speckle one.
INK = bytes((0x18, 0x2C)[v % 2] if v < 72 else 0xFF for v in range(256))
SPECKLE = bytes(0x50 if v == 0 else 0xFF for v in range(256))


def scan(rng):
    """A pseudo-scan's raw rows: white paper, some rows speckled, and lines
    of word-shaped ink, each ink row twice."""
    white = b"\xff" * SCAN_W
    rows = []

    def paper(k):
        for _ in range(k):
            rows.append(rng.randbytes(SCAN_W).translate(SPECKLE) if rng.random() < 0.3 else white)

    paper(150)
    while len(rows) < SCAN_H - 170:
        if rng.random() < 0.12:
            # A paragraph break.
            paper(rng.randrange(20, 40))
            continue
        words = []
        x, end = 150, SCAN_W - 150 - rng.randrange(200)
        while x < end:
            w = min(rng.randrange(15, 80), end - x)
            words.append((x, w))
            x += w + rng.randrange(8, 18)
        for _ in range(7):
            row = bytearray(white)
            for x, w in words:
                row[x:x + w] = rng.randbytes(w).translate(INK)
            rows += [bytes(row)] * 2
        paper(rng.randrange(8, 12))
    paper(SCAN_H - len(rows))
    return b"".join(rows)


def write_scanned(n, f):
    """Write the scanned n-page file to `f`; return what it holds, a fact
    a line."""
    rng = random.Random(3)
    pool = [zlib.compress(scan(rng)) for _ in range(SCAN_POOL)]
    picks = [rng.randrange(SCAN_POOL) for _ in range(n)]
    offsets = {}

    def put(num, *parts):
        offsets[num] = f.tell()
        f.write(b"%d 0 obj\n" % num)
        for part in parts:
            f.write(part)
        f.write(b"\nendobj\n")

    # 1 is the catalog, 2 the page tree and 3 the info; page p is 4 + 3p,
    # its content 5 + 3p and its image 6 + 3p.
    f.write(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n")
    put(1, b"<< /Type /Catalog /Pages 2 0 R >>")
    put(2, b"<< /Type /Pages /Kids [%s] /Count %d >>" % (refs(4 + 3 * p for p in range(n)), n))
    put(3, b"<< /Title %s /Producer (sheaf bench/gen.py) >>" % text(f"Scanned {n}-page fixture"))
    draw = b"q 612 0 0 792 0 0 cm /Im0 Do Q"
    for p in range(n):
        image = pool[picks[p]]
        put(4 + 3 * p, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] "
                       b"/Resources << /XObject << /Im0 %d 0 R >> >> /Contents %d 0 R >>" % (6 + 3 * p, 5 + 3 * p))
        put(5 + 3 * p, b"<< /Length %d >>\nstream\n" % len(draw), draw, b"\nendstream")
        put(6 + 3 * p, b"<< /Type /XObject /Subtype /Image /Width %d /Height %d /ColorSpace /DeviceGray "
                       b"/BitsPerComponent 8 /Filter /FlateDecode /Length %d >>\nstream\n"
            % (SCAN_W, SCAN_H, len(image)), image, b"\nendstream")
    size = 4 + 3 * n
    xref = f.tell()
    f.write(b"xref\n0 %d\n0000000000 65535 f \n" % size)
    f.write(b"".join(b"%010d 00000 n \n" % offsets[i] for i in range(1, size)))
    ident = hashlib.md5(b"scanned %d" % n).hexdigest().upper().encode()
    f.write(b"trailer\n<< /Size %d /Root 1 0 R /Info 3 0 R /ID [<%s> <%s>] >>\nstartxref\n%d\n%%%%EOF\n"
            % (size, ident, ident, xref))
    images = sum(len(pool[i]) for i in picks)
    return [(f"{n} images of {SCAN_W}x{SCAN_H} 8-bit gray, each a copy of one of {SCAN_POOL} scans: "
             f"{images // n:,} bytes on average")]


@contextlib.contextmanager
def replacing(path):
    """A file to write `path` through: path + '.<pid>.tmp', moved over
    `path` only once the block completes, so two runs making the same
    fixture never share one. Makes `path`'s directory."""
    os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
    tmp = f"{path}.{os.getpid()}.tmp"
    f = open(tmp, "wb")
    try:
        with f:
            yield f
    except BaseException:
        os.remove(tmp)
        raise
    os.replace(tmp, path)


def page_count(s):
    n = int(s)
    if n < 1:
        raise argparse.ArgumentTypeError("a fixture has at least one page")
    return n


def main():
    parser = argparse.ArgumentParser(prog="gen.py", description="Generate a 7k fixture.")
    fixtures = parser.add_subparsers(dest="fixture", required=True)
    synthetic = fixtures.add_parser("synthetic", help="text pages, one shared font, classic xref")
    synthetic.add_argument("pages", type=page_count)
    synthetic.add_argument("out")
    synthetic.add_argument("img_every", type=int, nargs="?", default=0,
                           help="draw a shared gray image on every img_every-th page (0: none)")
    heavy = fixtures.add_parser("heavy", help="a subset font per section, three annotations a page, bookmarks, "
                                              "labels, object streams")
    heavy.add_argument("pages", type=page_count)
    heavy.add_argument("out")
    heavy.add_argument("--font", default=DEJAVU, help="the TrueType font to embed subsets of (default: %(default)s)")
    scanned = fixtures.add_parser("scanned", help="a gray image a page, no text, flat page tree, classic xref")
    scanned.add_argument("pages", type=page_count)
    scanned.add_argument("out")
    args = parser.parse_args()

    t = time.perf_counter()
    facts = []
    if args.fixture == "synthetic":
        data = build(args.pages, img_every=args.img_every)[0]
    elif args.fixture == "heavy":
        try:
            font = TrueType(args.font)
        except OSError as e:
            heavy.error(f"can't read --font {args.font}: {e.strerror}")
        except (ValueError, struct.error) as e:
            heavy.error(f"--font {args.font} isn't a TrueType font gen.py can subset: {e}")
        data, facts = build_heavy(args.pages, font)
    with replacing(args.out) as f:
        if args.fixture == "scanned":
            facts = write_scanned(args.pages, f)
        else:
            f.write(data)
        size = f.tell()
    print(f"{args.fixture}, {args.pages} pages: {size:,} bytes in {time.perf_counter() - t:.2f} s")
    for fact in facts:
        print("  " + fact)


if __name__ == "__main__":
    main()
