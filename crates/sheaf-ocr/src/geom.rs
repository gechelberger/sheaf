//! Pixel-to-point mapping and word grouping shared by the engines.

use sheaf_core::PageSize;
use sheaf_doc::geom::Geom;
use sheaf_doc::{Box2, TextLayer};

use crate::engine::OcrWord;

/// Scale from a raster's pixels to the page's points. Uses the raster's
/// actual size (PDFium rounds the height), so the mapping is exact.
#[derive(Debug, Clone, Copy)]
pub struct PxToPt {
    sx: f32,
    sy: f32,
}

impl PxToPt {
    pub fn new(page: PageSize, raster_w: u32, raster_h: u32) -> Self {
        Self {
            sx: page.width / raster_w.max(1) as f32,
            sy: page.height / raster_h.max(1) as f32,
        }
    }

    /// `[left, top, right, bottom]` pixels → a box in points.
    pub fn rect(&self, b: [f32; 4]) -> Box2 {
        let (l, t, r, btm) = (
            b[0].min(b[2]),
            b[1].min(b[3]),
            b[0].max(b[2]),
            b[1].max(b[3]),
        );
        Box2 {
            x: l * self.sx,
            y: t * self.sy,
            w: (r - l) * self.sx,
            h: (btm - t) * self.sy,
        }
    }
}

/// Split a recognized line into words when the engine only gives boxes
/// without text per box. Cases: one box per whitespace-separated token;
/// one box per character of the line (spaces included, what oar-ocr's CTC
/// decoding yields); one box per non-space character; or no usable boxes,
/// in which case the whole line becomes one word.
pub fn words_from_line(
    text: &str,
    line: [f32; 4],
    boxes: Option<&[[f32; 4]]>,
    confidence: f32,
) -> Vec<OcrWord> {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.is_empty() {
        return Vec::new();
    }
    let whole = || {
        vec![OcrWord {
            text: tokens.join(" "),
            bbox_px: line,
            confidence,
        }]
    };
    let Some(boxes) = boxes else {
        return whole();
    };
    let word = |t: &str, b: [f32; 4]| OcrWord {
        text: t.to_string(),
        bbox_px: b,
        confidence,
    };
    if boxes.len() == tokens.len() {
        return tokens.iter().zip(boxes).map(|(t, b)| word(t, *b)).collect();
    }
    let chars: Vec<char> = text.chars().collect();
    if boxes.len() == chars.len() {
        // Union the boxes under each run of non-space characters.
        let mut out = Vec::with_capacity(tokens.len());
        let mut i = 0;
        while i < chars.len() {
            if chars[i].is_whitespace() {
                i += 1;
                continue;
            }
            let start = i;
            while i < chars.len() && !chars[i].is_whitespace() {
                i += 1;
            }
            let t: String = chars[start..i].iter().collect();
            out.push(word(&t, union(&boxes[start..i])));
        }
        return out;
    }
    let non_space: usize = tokens.iter().map(|t| t.chars().count()).sum();
    if boxes.len() == non_space {
        let mut out = Vec::with_capacity(tokens.len());
        let mut i = 0;
        for t in &tokens {
            let n = t.chars().count();
            out.push(word(t, union(&boxes[i..i + n])));
            i += n;
        }
        return out;
    }
    whole()
}

fn union(boxes: &[[f32; 4]]) -> [f32; 4] {
    boxes.iter().fold(boxes[0], |a, b| {
        [
            a[0].min(b[0]),
            a[1].min(b[1]),
            a[2].max(b[2]),
            a[3].max(b[3]),
        ]
    })
}

/// Turn an engine's words for the page `geom` rendered into a raster
/// `raster_w` by `raster_h` pixels into the model's layer.
pub fn layer_from_words(
    words: Vec<OcrWord>,
    geom: &Geom,
    raster_w: u32,
    raster_h: u32,
    engine: &str,
) -> TextLayer {
    let m = PxToPt::new(geom.shown(), raster_w, raster_h);
    let words = words
        .into_iter()
        .map(|w| sheaf_doc::OcrWord {
            text: w.text,
            rect: m.rect(w.bbox_px),
            confidence: w.confidence,
        })
        .collect();
    TextLayer::from_engine(words, geom, engine)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixels_scale_to_points() {
        let letter = PageSize {
            width: 612.0,
            height: 792.0,
        };
        // 300 dpi: 2550 × 3300.
        let m = PxToPt::new(letter, 2550, 3300);
        let r = m.rect([300.0, 416.0, 1550.0, 466.0]);
        assert!((r.x - 72.0).abs() < 1e-3 && (r.y - 99.84).abs() < 1e-2);
        assert!((r.w - 300.0).abs() < 1e-3 && (r.h - 12.0).abs() < 1e-3);
        // A rounded raster height still maps the bottom edge to the page edge.
        let m = PxToPt::new(letter, 2550, 3299);
        assert!((m.rect([0.0, 0.0, 2550.0, 3299.0]).bottom() - 792.0).abs() < 1e-3);
        // Reversed corners are normalized.
        assert_eq!(
            m.rect([10.0, 10.0, 0.0, 0.0]).w,
            m.rect([0.0, 0.0, 10.0, 10.0]).w
        );
    }

    #[test]
    fn line_splits_by_word_boxes_char_boxes_or_not_at_all() {
        let line = [0.0, 0.0, 100.0, 10.0];
        let per_word = [[0.0, 0.0, 40.0, 10.0], [50.0, 0.0, 100.0, 10.0]];
        let w = words_from_line("Hello World", line, Some(&per_word), 0.9);
        assert_eq!(w.len(), 2);
        assert_eq!(w[1].text, "World");
        assert_eq!(w[1].bbox_px, per_word[1]);

        let per_char: Vec<[f32; 4]> = (0..5)
            .map(|i| [i as f32 * 10.0, 0.0, i as f32 * 10.0 + 10.0, 10.0])
            .collect();
        let w = words_from_line("ab cde", line, Some(&per_char), 0.9);
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].bbox_px, [0.0, 0.0, 20.0, 10.0]);
        assert_eq!(w[1].bbox_px, [20.0, 0.0, 50.0, 10.0]);

        // One box per character including spaces (oar-ocr's CTC boxes).
        let per_char_sp: Vec<[f32; 4]> = (0..6)
            .map(|i| [i as f32 * 10.0, 0.0, i as f32 * 10.0 + 10.0, 10.0])
            .collect();
        let w = words_from_line("ab cde", line, Some(&per_char_sp), 0.9);
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].bbox_px, [0.0, 0.0, 20.0, 10.0]);
        assert_eq!(w[1].text, "cde");
        assert_eq!(w[1].bbox_px, [30.0, 0.0, 60.0, 10.0]);

        let w = words_from_line("  Hello   World ", line, Some(&per_word[..1]), 0.9);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].text, "Hello World");
        assert_eq!(w[0].bbox_px, line);
        assert!(words_from_line("   ", line, None, 0.9).is_empty());
    }

    /// A word on a Letter page rendered turned 90°, 792 by 612 pixels:
    /// its box in points as shown, then in PageSpace, where the top-left
    /// as shown is the bottom-left.
    #[test]
    fn layer_lands_in_page_space() {
        let geom = Geom::read(None, None, 90);
        let layer = layer_from_words(
            vec![OcrWord {
                text: "x".into(),
                bbox_px: [79.2, 61.2, 158.4, 122.4],
                confidence: 1.0,
            }],
            &geom,
            792,
            612,
            "test",
        );
        assert_eq!(layer.upright, sheaf_core::Rotation::R90);
        assert_eq!(layer.words.len(), 1);
        let r = layer.words[0].rect;
        let want = Box2 {
            x: 61.2,
            y: 79.2,
            w: 61.2,
            h: 79.2,
        };
        for (got, want) in [(r.x, want.x), (r.y, want.y), (r.w, want.w), (r.h, want.h)] {
            assert!((got - want).abs() < 1e-3, "{r:?}");
        }
    }
}
