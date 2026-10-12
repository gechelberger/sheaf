//! OCR text layers.
//!
//! A [`TextLayer`] is the words an OCR engine recognized on one page. It
//! lives in [`DocState`](crate::DocState) until the document is saved, when
//! it is written into the page as an invisible text layer (text render mode
//! 3) so PDFium, Acrobat and everything else can search and select it. The
//! model-side search over unsaved layers lives here too, so Find works
//! before the first save.
//!
//! Coordinates follow `annot.rs`: word rects are in the source page's
//! displayed space (points, top-left origin, after the file's own
//! `/Rotate`). The frame the text reads upright in is recorded separately
//! as [`TextLayer::upright`], the page's extra rotation at the time of OCR.
//! Rotating the page later doesn't move the glyphs, so the words stay put;
//! only the direction the text runs in changes, which the writer accounts
//! for.
//!
//! The written layer uses Tesseract's approach (`pdfrenderer.cpp`): a
//! Type0 font with an embedded glyphless TrueType whose every CID advances
//! half an em, so a word is fitted to its box by horizontal scaling alone
//! and any Unicode text round-trips.

use std::cmp::Ordering;
use std::ops::Range;

use lopdf::{Dictionary, Document as Pdf, Object, ObjectId, Stream, dictionary};
use sheaf_core::{PageSize, Rotation};

use crate::annot::{Box2, PageGeom, Pt};

/// One recognized word.
#[derive(Debug, Clone, PartialEq)]
pub struct OcrWord {
    pub text: String,
    /// Source-displayed space (see module docs).
    pub rect: Box2,
    /// 0 to 1.
    pub confidence: f32,
}

/// The OCR result for one page.
#[derive(Debug, Clone, PartialEq)]
pub struct TextLayer {
    /// Sorted by line (top to bottom in the upright frame), then left to right.
    pub words: Vec<OcrWord>,
    /// Index ranges into `words`, one per line.
    pub lines: Vec<Range<usize>>,
    /// The frame the text reads upright in: the page's extra rotation when
    /// it was recognized.
    pub upright: Rotation,
    /// Source-displayed page size, needed to rotate between frames.
    pub page: PageSize,
    /// Which engine produced it, e.g. "tesseract 5.5.2".
    pub engine: String,
}

impl TextLayer {
    /// Build a layer from words as the engine saw them: in points, in the
    /// frame of the page as rendered (`rendered` is that frame's size, the
    /// source-displayed size rotated by `upright`). Words are grouped into
    /// lines there, then stored unrotated into source-displayed space.
    pub fn from_engine(
        words: Vec<OcrWord>,
        rendered: PageSize,
        upright: Rotation,
        engine: &str,
    ) -> Self {
        let mut words: Vec<OcrWord> = words
            .into_iter()
            .filter(|w| !w.text.trim().is_empty() && w.rect.w > 0.0 && w.rect.h > 0.0)
            .collect();
        let lines = group_lines(&mut words);
        let back = Rotation::R0.minus(upright);
        for w in &mut words {
            w.rect = rotate_box(w.rect, rendered, back);
        }
        Self {
            words,
            lines,
            upright,
            page: rendered.rotated(upright),
            engine: engine.to_string(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    /// Rough heap bytes, for the history memory policy.
    pub fn byte_estimate(&self) -> usize {
        self.words.iter().map(|w| 64 + w.text.len()).sum::<usize>() + 16 * self.lines.len()
    }

    /// Boxes of the words whose centre lies inside `rect`, merged per line
    /// (the shape the highlight tool wants). Source-displayed space.
    pub fn boxes_in_rect(&self, rect: Box2) -> Vec<Box2> {
        let mut out = Vec::new();
        for line in &self.lines {
            let mut merged: Option<Box2> = None;
            for w in &self.words[line.clone()] {
                let c = Pt {
                    x: w.rect.x + w.rect.w / 2.0,
                    y: w.rect.y + w.rect.h / 2.0,
                };
                if rect.contains(c) {
                    merged = Some(merged.map_or(w.rect, |m| m.union(w.rect)));
                }
            }
            out.extend(merged);
        }
        out
    }

    /// The size of the upright frame.
    fn rendered(&self) -> PageSize {
        self.page.rotated(self.upright)
    }

    /// A word's box in the upright frame.
    fn upright_box(&self, w: &OcrWord) -> Box2 {
        rotate_box(w.rect, self.page, self.upright)
    }

    /// An upright-frame box back in source-displayed space.
    fn unrotate_from_upright(&self, b: Box2) -> Box2 {
        rotate_box(b, self.rendered(), Rotation::R0.minus(self.upright))
    }

    fn line_of(&self, word: usize) -> usize {
        self.lines
            .iter()
            .position(|l| l.contains(&word))
            .unwrap_or(0)
    }
}

/// Order `words` into lines (top to bottom, then left to right) and return
/// the index range of each line. A word joins the current line when its
/// vertical overlap with it is at least half the smaller height.
fn group_lines(words: &mut Vec<OcrWord>) -> Vec<Range<usize>> {
    let cmp = |a: f32, b: f32| a.partial_cmp(&b).unwrap_or(Ordering::Equal);
    let mut order: Vec<usize> = (0..words.len()).collect();
    order.sort_by(|&a, &b| {
        let (ra, rb) = (&words[a].rect, &words[b].rect);
        cmp(ra.y + ra.h / 2.0, rb.y + rb.h / 2.0).then(cmp(ra.x, rb.x))
    });
    let mut lines: Vec<(Box2, Vec<usize>)> = Vec::new();
    for i in order {
        let r = words[i].rect;
        match lines.last_mut() {
            Some((lb, members)) if overlaps_line(*lb, r) => {
                *lb = lb.union(r);
                members.push(i);
            }
            _ => lines.push((r, vec![i])),
        }
    }
    let mut out = Vec::with_capacity(words.len());
    let mut ranges = Vec::with_capacity(lines.len());
    for (_, mut members) in lines {
        members.sort_by(|&a, &b| cmp(words[a].rect.x, words[b].rect.x));
        let start = out.len();
        for i in members {
            out.push(words[i].clone());
        }
        ranges.push(start..out.len());
    }
    *words = out;
    ranges
}

fn overlaps_line(line: Box2, r: Box2) -> bool {
    let overlap = line.bottom().min(r.bottom()) - line.y.max(r.y);
    overlap >= 0.5 * line.h.min(r.h)
}

/// Rotate a box within a frame of size `frame` by `by` (clockwise quarter
/// turns), into the rotated frame. Same mapping as the viewer's.
pub fn rotate_box(b: Box2, frame: PageSize, by: Rotation) -> Box2 {
    match by {
        Rotation::R0 => b,
        Rotation::R90 => Box2 {
            x: frame.height - (b.y + b.h),
            y: b.x,
            w: b.h,
            h: b.w,
        },
        Rotation::R180 => Box2 {
            x: frame.width - (b.x + b.w),
            y: frame.height - (b.y + b.h),
            w: b.w,
            h: b.h,
        },
        Rotation::R270 => Box2 {
            x: b.y,
            y: frame.width - (b.x + b.w),
            w: b.h,
            h: b.w,
        },
    }
}

fn rotate_pt(p: Pt, frame: PageSize, by: Rotation) -> Pt {
    match by {
        Rotation::R0 => p,
        Rotation::R90 => Pt {
            x: frame.height - p.y,
            y: p.x,
        },
        Rotation::R180 => Pt {
            x: frame.width - p.x,
            y: frame.height - p.y,
        },
        Rotation::R270 => Pt {
            x: p.y,
            y: frame.width - p.x,
        },
    }
}

// ----- search ---------------------------------------------------------------

/// A match in a layer: one rect per line it spans, source-displayed space.
#[derive(Debug, Clone, PartialEq)]
pub struct TextHit {
    pub rects: Vec<Box2>,
    pub text: String,
}

/// The page as one string: words joined by a space within a line, lines
/// joined by a newline, with each character's owner word.
struct PageText {
    s: String,
    chars: Vec<char>,
    /// Byte offset of each char in `s`.
    starts: Vec<usize>,
    /// `(word index, char index within the word)`; None for separators.
    owner: Vec<Option<(usize, usize)>>,
}

fn page_text(layer: &TextLayer) -> PageText {
    let mut t = PageText {
        s: String::new(),
        chars: Vec::new(),
        starts: Vec::new(),
        owner: Vec::new(),
    };
    let push = |ch: char, owner: Option<(usize, usize)>, t: &mut PageText| {
        t.starts.push(t.s.len());
        t.s.push(ch);
        t.chars.push(ch);
        t.owner.push(owner);
    };
    for (li, line) in layer.lines.iter().enumerate() {
        if li > 0 {
            push('\n', None, &mut t);
        }
        for (k, wi) in line.clone().enumerate() {
            if k > 0 {
                push(' ', None, &mut t);
            }
            for (ci, ch) in layer.words[wi].text.chars().enumerate() {
                push(ch, Some((wi, ci)), &mut t);
            }
        }
    }
    t
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Literal search, like Find.
pub fn search(layer: &TextLayer, query: &str, match_case: bool) -> Vec<TextHit> {
    let q: Vec<char> = if match_case {
        query.chars().collect()
    } else {
        query.chars().map(fold).collect()
    };
    if q.is_empty() || layer.is_empty() {
        return Vec::new();
    }
    let t = page_text(layer);
    let hay: Vec<char> = if match_case {
        t.chars.clone()
    } else {
        t.chars.iter().copied().map(fold).collect()
    };
    let mut hits = Vec::new();
    let mut i = 0;
    while i + q.len() <= hay.len() {
        if hay[i..i + q.len()] == q[..] {
            hits.extend(hit_from_span(layer, &t, i, i + q.len()));
            i += q.len();
        } else {
            i += 1;
        }
    }
    hits
}

/// Regex search, like the Patterns panel.
pub fn pattern_search(layer: &TextLayer, re: &regex::Regex) -> Vec<TextHit> {
    if layer.is_empty() {
        return Vec::new();
    }
    let t = page_text(layer);
    let mut hits = Vec::new();
    for m in re.find_iter(&t.s) {
        if m.is_empty() {
            continue;
        }
        let first = t.starts.partition_point(|&b| b < m.start());
        let last = t.starts.partition_point(|&b| b < m.end());
        hits.extend(hit_from_span(layer, &t, first, last));
    }
    hits
}

/// The hit for chars `start..end` of the page string: per touched word, the
/// proportional slice of its box (every glyph advances equally once
/// written), unioned per line.
fn hit_from_span(layer: &TextLayer, t: &PageText, start: usize, end: usize) -> Option<TextHit> {
    // (word, first char, one past last char) in order of first appearance.
    let mut spans: Vec<(usize, usize, usize)> = Vec::new();
    for o in t.owner[start..end].iter().flatten() {
        match spans.last_mut() {
            Some((w, _, c1)) if *w == o.0 => *c1 = o.1 + 1,
            _ => spans.push((o.0, o.1, o.1 + 1)),
        }
    }
    if spans.is_empty() {
        return None;
    }
    let mut rects: Vec<(usize, Box2)> = Vec::new();
    for (wi, c0, c1) in spans {
        let w = &layer.words[wi];
        let n = w.text.chars().count().max(1) as f32;
        let up = layer.upright_box(w);
        let slice = Box2 {
            x: up.x + up.w * c0 as f32 / n,
            y: up.y,
            w: up.w * (c1 - c0) as f32 / n,
            h: up.h,
        };
        let r = layer.unrotate_from_upright(slice);
        let line = layer.line_of(wi);
        match rects.last_mut() {
            Some((l, prev)) if *l == line => *prev = prev.union(r),
            _ => rects.push((line, r)),
        }
    }
    Some(TextHit {
        rects: rects.into_iter().map(|(_, r)| r).collect(),
        text: t.chars[start..end].iter().collect(),
    })
}

// ----- PDF writing ------------------------------------------------------------

/// Resource name of the layer's font on a page.
pub(crate) const FONT_NAME: &str = "SheafOcr";
/// Marks the content streams we add, so a re-save can find and replace them.
const MARKER: &str = "SheafOcrLayer";
/// Tesseract's glyphless font (Apache-2.0; see `assets/LICENSE-pdf.ttf`).
static GLYPHLESS_TTF: &[u8] = include_bytes!("../assets/pdf.ttf");

const TO_UNICODE: &str = "/CIDInit /ProcSet findresource begin
12 dict begin
begincmap
/CIDSystemInfo
<<
  /Registry (Adobe)
  /Ordering (UCS)
  /Supplement 0
>> def
/CMapName /Adobe-Identity-UCS def
/CMapType 2 def
1 begincodespacerange
<0000> <FFFF>
endcodespacerange
1 beginbfrange
<0000> <FFFF> <0000>
endbfrange
endcmap
CMapName currentdict /CMap defineresource pop
end
end
";

/// The shared objects every layer in a file uses, created on first use.
#[derive(Debug, Clone, Copy)]
pub(crate) struct OcrObjects {
    /// The Type0 font.
    pub font: ObjectId,
    /// A content stream holding just `q`, prepended to the page content.
    pub q: ObjectId,
}

fn add_stream(pdf: &mut Pdf, dict: Dictionary, content: Vec<u8>, compress: bool) -> ObjectId {
    let mut s = Stream::new(dict, content);
    if compress {
        let _ = s.compress();
    } else {
        s.allows_compression = false;
    }
    pdf.add_object(Object::Stream(s))
}

pub(crate) fn ocr_objects(pdf: &mut Pdf, cache: &mut Option<OcrObjects>) -> OcrObjects {
    if let Some(o) = *cache {
        return o;
    }
    let ff2 = add_stream(
        pdf,
        dictionary! { "Length1" => GLYPHLESS_TTF.len() as i64 },
        GLYPHLESS_TTF.to_vec(),
        true,
    );
    // Every CID maps to glyph 1, the font's one (empty) glyph.
    let mut map = vec![0u8; 2 * 65536];
    for b in map.iter_mut().skip(1).step_by(2) {
        *b = 1;
    }
    let c2g = add_stream(pdf, dictionary! {}, map, true);
    let cmap = add_stream(pdf, dictionary! {}, TO_UNICODE.as_bytes().to_vec(), true);
    let descriptor = pdf.add_object(dictionary! {
        "Type" => "FontDescriptor",
        "FontName" => "GlyphLessFont",
        "Flags" => 5i64, // FixedPitch + Symbolic
        "FontBBox" => vec![0.into(), 0.into(), 500.into(), 1000.into()],
        "Ascent" => 1000i64,
        "Descent" => -1i64,
        "CapHeight" => 1000i64,
        "ItalicAngle" => 0i64,
        "StemV" => 80i64,
        "FontFile2" => ff2,
    });
    let cid = pdf.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "CIDFontType2",
        "BaseFont" => "GlyphLessFont",
        "CIDSystemInfo" => dictionary! {
            "Registry" => Object::string_literal("Adobe"),
            "Ordering" => Object::string_literal("Identity"),
            "Supplement" => 0i64,
        },
        "FontDescriptor" => descriptor,
        "DW" => 500i64,
        "CIDToGIDMap" => c2g,
    });
    let font = pdf.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type0",
        "BaseFont" => "GlyphLessFont",
        "Encoding" => "Identity-H",
        "DescendantFonts" => vec![Object::Reference(cid)],
        "ToUnicode" => cmap,
    });
    let q = add_stream(pdf, dictionary! { MARKER => true }, b"q\n".to_vec(), false);
    let o = OcrObjects { font, q };
    *cache = Some(o);
    o
}

fn utf16_hex(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 4);
    for u in s.encode_utf16() {
        out.push_str(&format!("{u:04X}"));
    }
    out
}

/// The content stream for `layer` on a page whose own `/Rotate` is
/// `geom.rotate`: invisible text, one `Tj` per word, fitted to the word's
/// box by horizontal scaling. Starts with `Q` to close the `q` that
/// precedes the original content.
pub(crate) fn to_content(layer: &TextLayer, geom: &PageGeom) -> Vec<u8> {
    let rendered = layer.rendered();
    let back = Rotation::R0.minus(layer.upright);
    // Text runs along the upright frame's x axis; rotate it with the page's
    // own /Rotate plus the frame's rotation.
    let theta = (geom.rotate + layer.upright.degrees()).rem_euclid(360) as f32;
    let (cs, sn) = (theta.to_radians().cos(), theta.to_radians().sin());
    let tm = |x: f32, y: f32| {
        let o = geom.to_user(rotate_pt(Pt { x, y }, rendered, back));
        format!(
            "{cs:.4} {sn:.4} {:.4} {cs:.4} {:.3} {:.3} Tm",
            -sn, o.x, o.y
        )
    };
    let mut s = String::from("Q\nBT\n3 Tr\n");
    let mut last_fs: Option<f32> = None;
    for line in &layer.lines {
        let mut prev_right: Option<f32> = None;
        for w in &layer.words[line.clone()] {
            let r = layer.upright_box(w);
            let n = w.text.encode_utf16().count();
            if n == 0 || r.h < 0.1 || r.w <= 0.0 {
                prev_right = None;
                continue;
            }
            let fs = r.h;
            if last_fs.is_none_or(|f| (fs - f).abs() > 1e-3) {
                s.push_str(&format!("/{FONT_NAME} {fs:.2} Tf\n"));
                last_fs = Some(fs);
            }
            // The gap to the previous word becomes a real space so viewers
            // separate the words.
            if let Some(pr) = prev_right {
                let gap = r.x - pr;
                if gap > 0.1 {
                    s.push_str(&format!(
                        "{} {:.2} Tz <0020> Tj\n",
                        tm(pr, r.bottom()),
                        200.0 * gap / fs
                    ));
                }
            }
            // Each CID advances half an em, so scale n half-ems to the box.
            s.push_str(&format!(
                "{} {:.2} Tz <{}> Tj\n",
                tm(r.x, r.bottom()),
                200.0 * r.w / (fs * n as f32),
                utf16_hex(&w.text)
            ));
            prev_right = Some(r.right());
        }
    }
    s.push_str("ET\n");
    s.into_bytes()
}

/// Add `layer` to `page` (an owned, flattened page dictionary about to be
/// written): `/Contents` becomes `[q, original…, layer]` and `/Resources`
/// is inlined with the layer's font. An earlier layer of ours is removed
/// first. No-op for an empty layer.
pub(crate) fn install(
    pdf: &mut Pdf,
    page: &mut Dictionary,
    layer: &TextLayer,
    geom: &PageGeom,
    objects: &mut Option<OcrObjects>,
) {
    strip(pdf, page);
    if layer.is_empty() {
        return;
    }
    let o = ocr_objects(pdf, objects);
    let ocr = add_stream(
        pdf,
        dictionary! { MARKER => true },
        to_content(layer, geom),
        true,
    );
    // Never decode the original streams: array entries are concatenated
    // with whitespace, and the q/Q pair isolates whatever state they leave.
    let mut entries = vec![Object::Reference(o.q)];
    match page.get(b"Contents").ok().cloned() {
        None => {}
        Some(Object::Array(a)) => entries.extend(a),
        Some(Object::Reference(r)) => match pdf.get_object(r) {
            Ok(Object::Array(a)) => entries.extend(a.iter().cloned()),
            _ => entries.push(Object::Reference(r)),
        },
        Some(other) => entries.push(other),
    }
    entries.push(Object::Reference(ocr));
    page.set("Contents", Object::Array(entries));

    let deref_dict = |pdf: &Pdf, o: Option<&Object>| -> Dictionary {
        o.and_then(|o| pdf.dereference(o).ok())
            .and_then(|(_, o)| o.as_dict().ok().cloned())
            .unwrap_or_default()
    };
    let mut resources = deref_dict(pdf, page.get(b"Resources").ok());
    let mut fonts = deref_dict(pdf, resources.get(b"Font").ok());
    fonts.set(FONT_NAME, Object::Reference(o.font));
    resources.set("Font", Object::Dictionary(fonts));
    page.set("Resources", Object::Dictionary(resources));
}

/// Remove a layer we wrote earlier from the page's `/Contents`.
pub(crate) fn strip(pdf: &Pdf, page: &mut Dictionary) {
    let Some(entries) = page
        .get(b"Contents")
        .ok()
        .and_then(|c| pdf.dereference(c).ok())
        .and_then(|(_, o)| o.as_array().ok().cloned())
    else {
        return;
    };
    let ours = |e: &Object| {
        e.as_reference()
            .ok()
            .and_then(|r| pdf.get_object(r).ok())
            .and_then(|o| o.as_stream().ok())
            .is_some_and(|s| s.dict.has(MARKER.as_bytes()))
    };
    if !entries.iter().any(ours) {
        return;
    }
    let kept: Vec<Object> = entries.into_iter().filter(|e| !ours(e)).collect();
    match kept.len() {
        0 => {
            page.remove(b"Contents");
        }
        1 => page.set("Contents", kept[0].clone()),
        _ => page.set("Contents", Object::Array(kept)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, x: f32, y: f32, w: f32, h: f32) -> OcrWord {
        OcrWord {
            text: text.into(),
            rect: Box2 { x, y, w, h },
            confidence: 0.9,
        }
    }

    const LETTER: PageSize = PageSize {
        width: 612.0,
        height: 792.0,
    };

    fn layer(words: Vec<OcrWord>, upright: Rotation) -> TextLayer {
        TextLayer::from_engine(words, LETTER.rotated(upright), upright, "test")
    }

    fn two_lines() -> TextLayer {
        layer(
            vec![
                word("World", 130.0, 100.0, 50.0, 10.0),
                word("Hello", 72.0, 100.0, 50.0, 10.0),
                word("second", 72.0, 120.0, 60.0, 10.0),
                word("line", 140.0, 121.0, 30.0, 9.0),
            ],
            Rotation::R0,
        )
    }

    #[test]
    fn groups_and_orders_lines() {
        let l = two_lines();
        let texts: Vec<&str> = l.words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(texts, ["Hello", "World", "second", "line"]);
        assert_eq!(l.lines, vec![0..2, 2..4]);
        assert!(l.byte_estimate() > 0);
    }

    #[test]
    fn drops_blank_and_degenerate_words() {
        let l = layer(
            vec![
                word("  ", 0.0, 0.0, 10.0, 10.0),
                word("x", 0.0, 0.0, 0.0, 10.0),
            ],
            Rotation::R0,
        );
        assert!(l.is_empty());
    }

    #[test]
    fn rotation_round_trips_for_every_frame() {
        let b = Box2 {
            x: 10.0,
            y: 20.0,
            w: 30.0,
            h: 5.0,
        };
        for r in [Rotation::R0, Rotation::R90, Rotation::R180, Rotation::R270] {
            let rotated = rotate_box(b, LETTER, r);
            let frame = LETTER.rotated(r);
            let back = rotate_box(rotated, frame, Rotation::R0.minus(r));
            assert_eq!(back, b, "{r:?}");
            // A word recognized in a rotated frame lands in displayed space
            // such that rotating it back reproduces the engine's box.
            let l = layer(vec![word("w", b.x, b.y, b.w, b.h)], r);
            assert_eq!(l.upright_box(&l.words[0]), b, "{r:?}");
            assert_eq!(l.page, LETTER);
        }
        // R90: the bottom-left corner of the page becomes the top-left.
        let r = rotate_box(
            Box2 {
                x: 0.0,
                y: 782.0,
                w: 10.0,
                h: 10.0,
            },
            LETTER,
            Rotation::R90,
        );
        assert_eq!(
            r,
            Box2 {
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 10.0
            }
        );
    }

    #[test]
    fn literal_search_spans_words_on_a_line() {
        let l = two_lines();
        let hits = search(&l, "lo wor", false);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].text, "lo Wor");
        assert_eq!(hits[0].rects.len(), 1);
        let r = hits[0].rects[0];
        // "lo" is the last 2 of 5 chars of Hello: starts at 72 + 50*3/5.
        assert!((r.x - 102.0).abs() < 1e-3, "{r:?}");
        // "Wor" is the first 3 of 5 chars of World: ends at 130 + 50*3/5.
        assert!((r.right() - 160.0).abs() < 1e-3, "{r:?}");
        assert!(search(&l, "lo wor", true).is_empty());
        assert!(search(&l, "", false).is_empty());
        assert!(
            search(&l, "hello second", false).is_empty(),
            "no match across lines"
        );
    }

    #[test]
    fn repeated_and_partial_matches() {
        let l = layer(vec![word("aaaa", 0.0, 0.0, 40.0, 10.0)], Rotation::R0);
        let hits = search(&l, "aa", true);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[1].rects[0].x, 20.0);
        assert_eq!(hits[1].rects[0].w, 20.0);
    }

    #[test]
    fn pattern_search_maps_bytes_to_words() {
        let l = layer(
            vec![
                word("SSN", 10.0, 10.0, 30.0, 10.0),
                word("123-45-6789", 50.0, 10.0, 110.0, 10.0),
                word("état", 10.0, 30.0, 40.0, 10.0),
            ],
            Rotation::R0,
        );
        let re = regex::Regex::new(r"\b\d{3}-\d{2}-\d{4}\b").unwrap();
        let hits = pattern_search(&l, &re);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].text, "123-45-6789");
        assert_eq!(hits[0].rects[0].x, 50.0);
        assert_eq!(hits[0].rects[0].w, 110.0);
        // Multi-byte chars before a match don't shift it.
        let re = regex::Regex::new("tat").unwrap();
        let hits = pattern_search(&l, &re);
        assert_eq!(hits.len(), 1);
        assert!((hits[0].rects[0].x - 20.0).abs() < 1e-3);
    }

    #[test]
    fn search_in_rotated_frame_returns_displayed_rects() {
        // Recognized with the page shown rotated 90°: the word sits at the
        // top-left of the rotated frame, i.e. the bottom-left of the source.
        let l = layer(vec![word("Hi", 0.0, 0.0, 20.0, 10.0)], Rotation::R90);
        assert_eq!(
            l.words[0].rect,
            Box2 {
                x: 0.0,
                y: 772.0,
                w: 10.0,
                h: 20.0
            }
        );
        let hits = search(&l, "i", true);
        // Second half of the word along the rotated x axis = upper half in
        // source space.
        assert_eq!(
            hits[0].rects[0],
            Box2 {
                x: 0.0,
                y: 772.0,
                w: 10.0,
                h: 10.0
            }
        );
    }

    #[test]
    fn boxes_in_rect_merges_per_line() {
        let l = two_lines();
        let boxes = l.boxes_in_rect(Box2 {
            x: 60.0,
            y: 90.0,
            w: 200.0,
            h: 50.0,
        });
        assert_eq!(boxes.len(), 2);
        assert_eq!(boxes[0].x, 72.0);
        assert_eq!(boxes[0].right(), 180.0);
    }

    #[test]
    fn content_places_invisible_fitted_text() {
        let geom = PageGeom {
            media: [0.0, 0.0, 612.0, 792.0],
            rotate: 0,
        };
        let l = layer(
            vec![
                word("Hello", 72.0, 100.0, 40.0, 10.0),
                word("World", 122.0, 100.0, 40.0, 10.0),
            ],
            Rotation::R0,
        );
        let c = String::from_utf8(to_content(&l, &geom)).unwrap();
        assert!(c.starts_with("Q\nBT\n3 Tr\n"), "{c}");
        assert!(c.contains("/SheafOcr 10.00 Tf"), "{c}");
        // 5 code units, size 10, width 40 → 200*40/(10*5) = 160.
        assert!(
            c.contains(
                "1.0000 0.0000 -0.0000 1.0000 72.000 682.000 Tm 160.00 Tz <00480065006C006C006F> Tj"
            ),
            "{c}"
        );
        // The 10 pt gap becomes a space scaled to fit it.
        assert!(c.contains("112.000 682.000 Tm 200.00 Tz <0020> Tj"), "{c}");
        assert!(c.ends_with("ET\n"));
        assert_eq!(c.matches(" Tf").count(), 1, "font size emitted once");
    }

    #[test]
    fn content_rotates_with_page_and_frame() {
        let w = || vec![word("Hi", 10.0, 20.0, 20.0, 10.0)];
        let matrix = |rot: i32, upright: Rotation| {
            let geom = PageGeom {
                media: [0.0, 0.0, 612.0, 792.0],
                rotate: rot,
            };
            let c = String::from_utf8(to_content(&layer(w(), upright), &geom)).unwrap();
            let tm = c.lines().find(|l| l.ends_with("Tj")).unwrap();
            tm.split(" Tm").next().unwrap().to_string()
        };
        let abcd = |m: &str| m.split(' ').take(4).collect::<Vec<_>>().join(" ");
        assert!(matrix(0, Rotation::R0).starts_with("1.0000 0.0000 -0.0000 1.0000"));
        assert!(matrix(90, Rotation::R0).starts_with("-0.0000 1.0000 -1.0000 -0.0000"));
        assert!(matrix(180, Rotation::R0).starts_with("-1.0000 -0.0000 0.0000 -1.0000"));
        assert!(matrix(270, Rotation::R0).starts_with("0.0000 -1.0000 1.0000 0.0000"));
        // Frame rotation adds to the page's own.
        assert_eq!(
            abcd(&matrix(90, Rotation::R90)),
            abcd(&matrix(180, Rotation::R0))
        );
        assert_eq!(
            abcd(&matrix(270, Rotation::R90)),
            abcd(&matrix(0, Rotation::R0))
        );
        // Upright frame R90 on an unrotated page: the word's bottom-left (10, 30)
        // in the rotated frame is (30, 782) displayed, i.e. user (30, 10).
        assert!(
            matrix(0, Rotation::R90).ends_with("30.000 10.000"),
            "{}",
            matrix(0, Rotation::R90)
        );
        // Source /Rotate 90, word at the top-left of the displayed page: its
        // bottom-left (10, 30) displayed is user (30, 10).
        assert!(
            matrix(90, Rotation::R0).ends_with("30.000 10.000"),
            "{}",
            matrix(90, Rotation::R0)
        );
    }

    #[test]
    fn install_and_strip_are_idempotent() {
        let mut pdf = Pdf::with_version("1.5");
        let orig = pdf.add_object(Stream::new(dictionary! {}, b"0 0 m".to_vec()));
        let font = pdf.add_object(dictionary! { "Type" => "Font" });
        let res = pdf.add_object(dictionary! { "Font" => dictionary! { "F1" => font } });
        let mut page = dictionary! { "Type" => "Page", "Contents" => orig, "Resources" => res };
        let geom = PageGeom {
            media: [0.0, 0.0, 612.0, 792.0],
            rotate: 0,
        };
        let l = layer(vec![word("Hi", 10.0, 20.0, 20.0, 10.0)], Rotation::R0);
        let mut objs = None;
        install(&mut pdf, &mut page, &l, &geom, &mut objs);
        let contents = page.get(b"Contents").unwrap().as_array().unwrap().clone();
        assert_eq!(contents.len(), 3);
        assert_eq!(contents[1], Object::Reference(orig));
        let fonts = page
            .get(b"Resources")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"Font")
            .unwrap()
            .as_dict()
            .unwrap()
            .clone();
        assert!(fonts.has(b"F1"));
        assert_eq!(
            fonts.get(b"SheafOcr").unwrap(),
            &Object::Reference(objs.unwrap().font)
        );
        // A second install replaces rather than stacks.
        let before = pdf.objects.len();
        install(&mut pdf, &mut page, &l, &geom, &mut objs);
        assert_eq!(page.get(b"Contents").unwrap().as_array().unwrap().len(), 3);
        assert_eq!(pdf.objects.len(), before + 1, "only a new content stream");
        strip(&pdf, &mut page);
        assert_eq!(page.get(b"Contents").unwrap(), &Object::Reference(orig));
        // An empty layer leaves the page alone.
        let empty = layer(vec![], Rotation::R0);
        install(&mut pdf, &mut page, &empty, &geom, &mut objs);
        assert_eq!(page.get(b"Contents").unwrap(), &Object::Reference(orig));
    }

    #[test]
    fn font_objects_match_tesseract_layout() {
        let mut pdf = Pdf::with_version("1.5");
        let mut cache = None;
        let o = ocr_objects(&mut pdf, &mut cache);
        assert_eq!(ocr_objects(&mut pdf, &mut cache).font, o.font, "cached");
        let type0 = pdf.get_dictionary(o.font).unwrap();
        assert_eq!(
            type0.get(b"Encoding").unwrap().as_name().unwrap(),
            b"Identity-H"
        );
        let cid = pdf
            .get_dictionary(
                type0.get(b"DescendantFonts").unwrap().as_array().unwrap()[0]
                    .as_reference()
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(cid.get(b"DW").unwrap().as_i64().unwrap(), 500);
        let c2g = pdf
            .get_object(cid.get(b"CIDToGIDMap").unwrap().as_reference().unwrap())
            .unwrap()
            .as_stream()
            .unwrap();
        let map = c2g.decompressed_content().unwrap();
        assert_eq!(map.len(), 131072);
        assert_eq!(&map[..4], &[0, 1, 0, 1]);
        let desc = pdf
            .get_dictionary(cid.get(b"FontDescriptor").unwrap().as_reference().unwrap())
            .unwrap();
        let ff2 = pdf
            .get_object(desc.get(b"FontFile2").unwrap().as_reference().unwrap())
            .unwrap()
            .as_stream()
            .unwrap();
        assert_eq!(ff2.decompressed_content().unwrap(), GLYPHLESS_TTF);
        let cmap = pdf
            .get_object(type0.get(b"ToUnicode").unwrap().as_reference().unwrap())
            .unwrap()
            .as_stream()
            .unwrap();
        let cmap = String::from_utf8(cmap.decompressed_content().unwrap()).unwrap();
        assert!(cmap.contains("beginbfrange"));
        let q = pdf.get_object(o.q).unwrap().as_stream().unwrap();
        assert_eq!(q.content, b"q\n");
    }
}
