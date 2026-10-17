//! OCR text layers.
//!
//! A [`TextLayer`] is the words an OCR engine recognized on one page. It
//! lives in [`DocState`](crate::DocState) until the document is saved, when
//! it is written into the page as an invisible text layer (text render mode
//! 3) so PDFium, Acrobat and everything else can search and select it. The
//! model-side search over unsaved layers lives here too, so Find works
//! before the first save.
//!
//! Word rects are in PageSpace (M10.5 §5), like marks, so cropping or
//! turning the page moves none of them. The frame the text reads upright
//! in is recorded as [`TextLayer::upright`], the page's rotation when it
//! was recognized; lines run along that frame's x, which the writer turns
//! the text to.
//!
//! The written layer is in [`Glyphless`], Tesseract's approach
//! (`pdfrenderer.cpp`): a Type0 font with an embedded glyphless TrueType
//! whose every CID advances half an em, so a word is fitted to its box by
//! horizontal scaling alone and any Unicode text round-trips.

use std::cmp::Ordering;
use std::ops::Range;

use lopdf::{Dictionary, Document as Pdf, Object, Stream, dictionary};
use serde::{Deserialize, Serialize};
use sheaf_core::Rotation;

use crate::annot::{Box2, Pt};
use crate::content::{
    self, ContentWriter, Font, Glyphless, ResObjects, Reserved, ResourceNeeds, TextMode,
};
use crate::geom::{Affine, Geom};

/// One recognized word.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OcrWord {
    pub text: String,
    /// PageSpace.
    pub rect: Box2,
    /// 0 to 1.
    pub confidence: f32,
}

/// The OCR result for one page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextLayer {
    /// Sorted by line (top to bottom in the upright frame), then left to right.
    pub words: Vec<OcrWord>,
    /// Index ranges into `words`, one per line.
    pub lines: Vec<Range<usize>>,
    /// The frame the text reads upright in: the page's rotation, absolute,
    /// when it was recognized.
    pub upright: Rotation,
    /// Which engine produced it, e.g. "tesseract 5.5.2".
    pub engine: String,
}

impl TextLayer {
    /// Build a layer from words as the engine saw them: in points, in the
    /// view frame of `geom`, the page as it was rendered. Words are grouped
    /// into lines there, then stored in PageSpace.
    pub fn from_engine(words: Vec<OcrWord>, geom: &Geom, engine: &str) -> Self {
        let mut words: Vec<OcrWord> = words
            .into_iter()
            .filter(|w| !w.text.trim().is_empty() && w.rect.w > 0.0 && w.rect.h > 0.0)
            .collect();
        let lines = group_lines(&mut words);
        let from_view = geom.from_view();
        for w in &mut words {
            w.rect = from_view.apply_box(w.rect);
        }
        Self {
            words,
            lines,
            upright: geom.rotate,
            engine: engine.to_string(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    /// The recognized text: words joined by spaces, lines by newlines.
    pub fn text(&self) -> String {
        page_text(self).s
    }

    /// Rough heap bytes, for the history memory policy.
    pub fn byte_estimate(&self) -> usize {
        self.words.iter().map(|w| 64 + w.text.len()).sum::<usize>() + 16 * self.lines.len()
    }

    /// Boxes of the words whose centre lies inside `rect`, merged per line
    /// (the shape the highlight tool wants). PageSpace.
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

    /// A word's box in the upright frame (y down; where on the page
    /// doesn't matter, only which way the text runs).
    pub(crate) fn upright_box(&self, w: &OcrWord) -> Box2 {
        Affine::upright(self.upright).apply_box(w.rect)
    }

    /// An upright-frame box back in PageSpace.
    pub(crate) fn unrotate_from_upright(&self, b: Box2) -> Box2 {
        Affine::upright(self.upright).inverse().apply_box(b)
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

/// Whether `r` is level with a line whose box is `line`: their vertical
/// overlap is at least half the smaller height.
pub(crate) fn overlaps_line(line: Box2, r: Box2) -> bool {
    let overlap = line.bottom().min(r.bottom()) - line.y.max(r.y);
    overlap >= 0.5 * line.h.min(r.h)
}

// ----- search ---------------------------------------------------------------

/// A match in a layer: one rect per line it spans, in PageSpace.
#[derive(Debug, Clone, PartialEq)]
pub struct TextHit {
    pub rects: Vec<Box2>,
    pub text: String,
    /// A pattern's capture groups 1…n (None where a group didn't take
    /// part); empty for a literal search.
    pub groups: Vec<Option<String>>,
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
    for caps in re.captures_iter(&t.s) {
        let m = caps.get(0).expect("group 0 always matches");
        if m.is_empty() {
            continue;
        }
        let first = t.starts.partition_point(|&b| b < m.start());
        let last = t.starts.partition_point(|&b| b < m.end());
        if let Some(mut hit) = hit_from_span(layer, &t, first, last) {
            hit.groups = caps
                .iter()
                .skip(1)
                .map(|g| g.map(|g| g.as_str().to_string()))
                .collect();
            hits.push(hit);
        }
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
        groups: Vec::new(),
    })
}

// ----- PDF writing ------------------------------------------------------------

/// Resource name of the layer's font on a page, until slice 18's blocks.
pub(crate) const FONT_NAME: &str = "SheafOcr";
/// Marks the content streams we add, so a re-save can find and replace them.
const MARKER: &str = "SheafOcrLayer";

/// The content stream for `layer`: invisible text, one `Tj` per word,
/// fitted to the word's box by horizontal scaling, in [`Glyphless`] named
/// [`FONT_NAME`]; and the font it needs. Text runs along the upright
/// frame's x, so it's turned by [`TextLayer::upright`]. Starts with `Q` to
/// close the `q` that precedes the original content.
pub(crate) fn to_content(layer: &TextLayer) -> (Vec<u8>, ResourceNeeds) {
    let to_user = Affine::upright(layer.upright).inverse();
    let theta = layer.upright.degrees() as f32;
    let (cs, sn) = (theta.to_radians().cos(), theta.to_radians().sin());
    let tm = |x: f32, y: f32| {
        let o = to_user.apply(Pt { x, y });
        Affine {
            a: cs,
            b: sn,
            c: -sn,
            d: cs,
            e: o.x,
            f: o.y,
        }
    };
    let mut w = ContentWriter::new();
    w.restore();
    w.begin_text();
    w.text_mode(TextMode::Invisible);
    let mut last_fs: Option<f32> = None;
    for line in &layer.lines {
        let mut prev_right: Option<f32> = None;
        for word in &layer.words[line.clone()] {
            let r = layer.upright_box(word);
            let n = word.text.encode_utf16().count();
            if n == 0 || r.h < 0.1 || r.w <= 0.0 {
                prev_right = None;
                continue;
            }
            let fs = r.h;
            if last_fs.is_none_or(|f| (fs - f).abs() > 1e-3) {
                w.font(&Glyphless, fs);
                last_fs = Some(fs);
            }
            // The gap to the previous word becomes a real space so viewers
            // separate the words.
            if let Some(pr) = prev_right {
                let gap = r.x - pr;
                if gap > 0.1 {
                    w.text_matrix(tm(pr, r.bottom()));
                    w.horizontal_scale(200.0 * gap / fs);
                    w.text(" ");
                }
            }
            // Each CID advances half an em, so scale n half-ems to the box.
            w.text_matrix(tm(r.x, r.bottom()));
            w.horizontal_scale(200.0 * r.w / (fs * n as f32));
            w.text(&word.text);
            prev_right = Some(r.right());
        }
    }
    w.end_text();
    w.finish(
        &Reserved::default(),
        &[(Glyphless.resource().id, FONT_NAME)],
    )
}

/// Add `layer` to `page` (an owned, flattened page dictionary about to be
/// written): `/Contents` becomes `[q, original…, layer]` and `/Resources`
/// is inlined with the layer's font, the `q` and the font written once a
/// save through `res`. An earlier layer of ours is removed first. No-op
/// for an empty layer.
pub(crate) fn install(
    pdf: &mut Pdf,
    page: &mut Dictionary,
    layer: &TextLayer,
    res: &mut ResObjects,
) {
    strip(pdf, page);
    if layer.is_empty() {
        return;
    }
    let (content, needs) = to_content(layer);
    let q = res.stream(pdf, dictionary! { MARKER => true }, b"q\n".to_vec());
    let mut stream = Stream::new(dictionary! { MARKER => true }, content);
    let _ = stream.compress();
    let ocr = pdf.add_object(stream);
    // Never decode the original streams: array entries are concatenated
    // with whitespace, and the q/Q pair isolates whatever state they leave.
    let mut entries = vec![Object::Reference(q)];
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
    let mut resources = content::resources_of(pdf, page);
    res.bind(pdf, &needs, &mut resources);
    page.set("Resources", Object::Dictionary(resources));
}

/// Whether the page has an OCR layer of ours in its content.
pub(crate) fn has_layer(pdf: &Pdf, page: &Dictionary) -> bool {
    let mut probe = page.clone();
    strip(pdf, &mut probe);
    probe.get(b"Contents").ok() != page.get(b"Contents").ok()
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
    use crate::geom::Rect;

    const ROTATIONS: [Rotation; 4] = [Rotation::R0, Rotation::R90, Rotation::R180, Rotation::R270];

    fn word(text: &str, x: f32, y: f32, w: f32, h: f32) -> OcrWord {
        OcrWord {
            text: text.into(),
            rect: Box2 { x, y, w, h },
            confidence: 0.9,
        }
    }

    /// A Letter page turned `r`.
    fn letter(r: Rotation) -> Geom {
        Geom::read(None, None, r.degrees() as i64)
    }

    /// A layer of `words` as the engine saw them on a Letter page turned
    /// `upright`.
    fn layer(words: Vec<OcrWord>, upright: Rotation) -> TextLayer {
        TextLayer::from_engine(words, &letter(upright), "test")
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

    /// A word the engine saw on a page shown at any rotation, with boxes
    /// off the origin, is stored in PageSpace and comes back through the
    /// page's `to_view` to where the engine saw it; in the upright frame
    /// it keeps its size and runs the same way.
    #[test]
    fn words_come_back_through_to_view_for_every_frame() {
        let seen = Box2 {
            x: 10.0,
            y: 20.0,
            w: 30.0,
            h: 5.0,
        };
        let media = Rect::new(-37.0, 52.0, 575.0, 844.0);
        let crop = Rect::new(-17.0, 62.0, 555.0, 824.0);
        for r in ROTATIONS {
            let geom = Geom::read(Some(media), Some(crop), r.degrees() as i64);
            let l = TextLayer::from_engine(
                vec![word("w", seen.x, seen.y, seen.w, seen.h)],
                &geom,
                "test",
            );
            assert_eq!(l.upright, r);
            let back = geom.to_view().apply_box(l.words[0].rect);
            for (got, want) in [
                (back.x, seen.x),
                (back.y, seen.y),
                (back.w, seen.w),
                (back.h, seen.h),
            ] {
                assert!((got - want).abs() < 1e-3, "{r:?}: {back:?}");
            }
            let up = l.upright_box(&l.words[0]);
            assert!(
                (up.w - seen.w).abs() < 1e-3 && (up.h - seen.h).abs() < 1e-3,
                "{r:?}: {up:?}"
            );
            assert_eq!(l.unrotate_from_upright(up), l.words[0].rect, "{r:?}");
        }
        // On a Letter page turned 90°, the top-left as shown is PageSpace's
        // bottom-left.
        let l = layer(vec![word("w", 0.0, 0.0, 10.0, 10.0)], Rotation::R90);
        assert_eq!(
            l.words[0].rect,
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
        // The line is 100 to 110 down the page: 682 to 692 up it.
        assert!(
            (r.y - 682.0).abs() < 1e-3 && (r.h - 10.0).abs() < 1e-3,
            "{r:?}"
        );
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
    fn search_in_a_turned_frame_returns_page_space_rects() {
        // Recognized with the page shown turned 90°: the word sits at the
        // top-left as shown, PageSpace's bottom-left, and runs up the page.
        let l = layer(vec![word("Hi", 0.0, 0.0, 20.0, 10.0)], Rotation::R90);
        assert_eq!(
            l.words[0].rect,
            Box2 {
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 20.0
            }
        );
        let hits = search(&l, "i", true);
        // The second half of the word along the upright frame's x is its
        // upper half in PageSpace.
        assert_eq!(
            hits[0].rects[0],
            Box2 {
                x: 0.0,
                y: 10.0,
                w: 10.0,
                h: 10.0
            }
        );
    }

    #[test]
    fn boxes_in_rect_merges_per_line() {
        let l = two_lines();
        // The lines are 682 to 692 and 661 to 672 up the page.
        let boxes = l.boxes_in_rect(Box2 {
            x: 60.0,
            y: 650.0,
            w: 200.0,
            h: 50.0,
        });
        assert_eq!(boxes.len(), 2);
        assert_eq!(boxes[0].x, 72.0);
        assert_eq!(boxes[0].right(), 180.0);
    }

    #[test]
    fn content_places_invisible_fitted_text() {
        let l = layer(
            vec![
                word("Hello", 72.0, 100.0, 40.0, 10.0),
                word("World", 122.0, 100.0, 40.0, 10.0),
            ],
            Rotation::R0,
        );
        let (c, needs) = to_content(&l);
        let c = String::from_utf8(c).unwrap();
        assert!(c.starts_with("Q\nBT\n3 Tr\n"), "{c}");
        assert!(c.contains("/SheafOcr 10 Tf\n"), "{c}");
        // 5 code units, size 10, width 40 → 200*40/(10*5) = 160.
        assert!(
            c.contains("1 0 0 1 72 682 Tm\n160 Tz\n<00480065006C006C006F> Tj\n"),
            "{c}"
        );
        // The 10 pt gap becomes a space scaled to fit it.
        assert!(c.contains("1 0 0 1 112 682 Tm\n200 Tz\n<0020> Tj\n"), "{c}");
        assert!(c.ends_with("ET\n"));
        assert_eq!(c.matches(" Tf").count(), 1, "font size emitted once");
        let [font] = &needs.0[..] else {
            panic!("{needs:?}")
        };
        assert_eq!(font.name, b"SheafOcr");
        assert_eq!(font.res, Glyphless.resource().into());
    }

    /// The text runs along the upright frame's x: turned with the page it
    /// was recognized on, starting at its word's bottom-left as it read.
    /// The same layer gives the same bytes and names, twice and on two
    /// threads.
    #[test]
    fn identical_layers_write_identical_bytes() {
        fn write() -> (Vec<u8>, ResourceNeeds) {
            let words = vec![
                word("Hello", 72.0, 100.0, 50.0, 10.0),
                word("wörld", 130.0, 100.0, 50.0, 10.0),
                word("日本", 72.0, 120.0, 30.0, 10.0),
            ];
            to_content(&layer(words, Rotation::R90))
        }
        let first = write();
        assert_eq!(write(), first);
        let threads: Vec<_> = (0..2).map(|_| std::thread::spawn(write)).collect();
        for t in threads {
            assert_eq!(t.join().unwrap(), first);
        }
    }

    #[test]
    fn content_turns_with_the_upright_frame() {
        let matrix = |geom: Geom| {
            let l = TextLayer::from_engine(vec![word("Hi", 10.0, 20.0, 20.0, 10.0)], &geom, "t");
            let c = String::from_utf8(to_content(&l).0).unwrap();
            let tm = c.lines().find(|l| l.ends_with(" Tm")).unwrap();
            tm.trim_end_matches(" Tm").to_string()
        };
        let turned = |r| matrix(letter(r));
        // Upright: the word's bottom-left, (10, 30) down the page, is
        // (10, 762) up it.
        assert_eq!(turned(Rotation::R0), "1 0 0 1 10 762");
        // Turned 90°: (10, 30) as shown is user (30, 10).
        assert_eq!(turned(Rotation::R90), "0 1 -1 0 30 10");
        assert!(turned(Rotation::R180).starts_with("-1 0 0 -1 "));
        assert!(turned(Rotation::R270).starts_with("0 -1 1 0 "));
        // A media box off the origin moves the text with it.
        let media = Rect::new(-37.0, 52.0, 575.0, 844.0);
        let off = matrix(Geom::read(Some(media), None, 0));
        assert_eq!(off, "1 0 0 1 -27 814");
    }

    #[test]
    fn install_and_strip_are_idempotent() {
        let mut pdf = Pdf::with_version("1.5");
        let orig = pdf.add_object(Stream::new(dictionary! {}, b"0 0 m".to_vec()));
        let font = pdf.add_object(dictionary! { "Type" => "Font" });
        let res = pdf.add_object(dictionary! { "Font" => dictionary! { "F1" => font } });
        let mut page = dictionary! { "Type" => "Page", "Contents" => orig, "Resources" => res };
        let l = layer(vec![word("Hi", 10.0, 20.0, 20.0, 10.0)], Rotation::R0);
        let mut objs = ResObjects::default();
        install(&mut pdf, &mut page, &l, &mut objs);
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
            &Object::Reference(objs.object(&mut pdf, &Glyphless.resource().into()))
        );
        // A second install replaces rather than stacks.
        let before = pdf.objects.len();
        install(&mut pdf, &mut page, &l, &mut objs);
        assert_eq!(page.get(b"Contents").unwrap().as_array().unwrap().len(), 3);
        assert_eq!(pdf.objects.len(), before + 1, "only a new content stream");
        strip(&pdf, &mut page);
        assert_eq!(page.get(b"Contents").unwrap(), &Object::Reference(orig));
        // An empty layer leaves the page alone.
        let empty = layer(vec![], Rotation::R0);
        install(&mut pdf, &mut page, &empty, &mut objs);
        assert_eq!(page.get(b"Contents").unwrap(), &Object::Reference(orig));
    }

    #[test]
    fn font_objects_match_tesseract_layout() {
        let mut pdf = Pdf::with_version("1.5");
        let l = layer(vec![word("Hi", 10.0, 20.0, 20.0, 10.0)], Rotation::R0);
        let mut res = ResObjects::default();
        let mut pages = [dictionary! {}, dictionary! {}];
        for page in &mut pages {
            install(&mut pdf, page, &l, &mut res);
        }
        let font = |page: &Dictionary| {
            let resources = page.get(b"Resources").unwrap().as_dict().unwrap();
            let fonts = resources.get(b"Font").unwrap().as_dict().unwrap();
            fonts.get(b"SheafOcr").unwrap().as_reference().unwrap()
        };
        let q = |page: &Dictionary| content::contents(page)[0];
        assert_eq!(font(&pages[0]), font(&pages[1]), "one font a save");
        assert_eq!(q(&pages[0]), q(&pages[1]), "one q a save");
        let type0 = pdf.get_dictionary(font(&pages[0])).unwrap();
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
        assert_eq!(ff2.decompressed_content().unwrap(), Glyphless::PROGRAM);
        let cmap = pdf
            .get_object(type0.get(b"ToUnicode").unwrap().as_reference().unwrap())
            .unwrap()
            .as_stream()
            .unwrap();
        let cmap = String::from_utf8(cmap.decompressed_content().unwrap()).unwrap();
        assert!(cmap.contains("beginbfrange"));
        let q = pdf.get_object(q(&pages[0])).unwrap().as_stream().unwrap();
        assert_eq!(q.content, b"q\n");
        assert!(q.dict.has(MARKER.as_bytes()));
    }
}
