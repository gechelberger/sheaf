//! A page's text as characters in reading order, each with its box: what
//! text selection works on, and what text editing will. Built from the
//! engine's characters, or from an OCR layer that isn't saved yet.
//!
//! Boxes are in PageSpace (M10.5 §5), like marks. Lines read along x in
//! the [`PageChars::upright`] frame, so a page recognized while turned
//! still selects along its lines.
//!
//! A position in the text is a *boundary*: 0 is before the first
//! character, `len()` after the last. A selection is the characters
//! between two boundaries.

use sheaf_core::Rotation;

use crate::annot::{Box2, Pt, caret_box};
use crate::geom::{Affine, Geom};
use crate::text_layer::{TextLayer, overlaps_line};

#[derive(Debug, Clone, PartialEq)]
pub struct Glyph {
    pub ch: char,
    /// None for separators the extractor adds: spaces between words, line
    /// breaks.
    pub rect: Option<Box2>,
    /// Which line, from 0, in reading order.
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PageChars {
    pub glyphs: Vec<Glyph>,
    /// The frame the text reads upright in.
    pub upright: Rotation,
}

impl PageChars {
    /// From the engine's characters, in its order, with boxes in the view
    /// frame of `base`, the page the engine read as its file shows it.
    /// Lines are found there, by where the characters are, not by the
    /// engine's line breaks, which it sometimes puts mid-line (on a page
    /// with `/Rotate 270`, say): a character starts a new line when it
    /// isn't level with the line so far or goes back left. A break the
    /// engine left inside a line reads as a space; one it left out between
    /// lines is added. The boxes are then stored in PageSpace.
    pub fn from_engine(chars: Vec<(char, Option<Box2>)>, base: &Geom) -> PageChars {
        let mut glyphs: Vec<Glyph> = Vec::with_capacity(chars.len());
        let mut line = 0;
        // The line so far, and the last character on it.
        let mut current: Option<(Box2, Box2)> = None;
        let mut engine_break = false;
        for (ch, rect) in chars {
            match (ch, rect) {
                ('\r', _) => {}
                ('\n', _) => engine_break = true,
                (_, None) => glyphs.push(Glyph {
                    ch,
                    rect: None,
                    line,
                }),
                (_, Some(r)) => {
                    match current {
                        Some((lb, last)) if overlaps_line(lb, r) && r.x >= last.x - last.w => {
                            let spaced = ch.is_whitespace()
                                || glyphs.last().is_none_or(|g| g.ch.is_whitespace());
                            if engine_break && !spaced {
                                glyphs.push(Glyph {
                                    ch: ' ',
                                    rect: None,
                                    line,
                                });
                            }
                            current = Some((lb.union(r), r));
                        }
                        Some(_) => {
                            glyphs.push(Glyph {
                                ch: '\n',
                                rect: None,
                                line,
                            });
                            line += 1;
                            current = Some((r, r));
                        }
                        None => current = Some((r, r)),
                    }
                    engine_break = false;
                    glyphs.push(Glyph {
                        ch,
                        rect: Some(r),
                        line,
                    });
                }
            }
        }
        let from_view = base.from_view();
        for g in &mut glyphs {
            g.rect = g.rect.map(|r| from_view.apply_box(r));
        }
        PageChars {
            glyphs,
            upright: base.rotate,
        }
    }

    /// From an OCR layer: words joined by spaces and lines by line breaks,
    /// each word's box shared evenly by its characters (as the written
    /// layer's glyphless font does).
    pub fn from_layer(layer: &TextLayer) -> PageChars {
        let mut glyphs = Vec::new();
        for (li, words) in layer.lines.iter().enumerate() {
            let line = li as u32;
            if li > 0 {
                glyphs.push(Glyph {
                    ch: '\n',
                    rect: None,
                    line: line - 1,
                });
            }
            for (k, wi) in words.clone().enumerate() {
                if k > 0 {
                    glyphs.push(Glyph {
                        ch: ' ',
                        rect: None,
                        line,
                    });
                }
                let w = &layer.words[wi];
                let n = w.text.chars().count().max(1) as f32;
                let up = layer.upright_box(w);
                for (ci, ch) in w.text.chars().enumerate() {
                    let slice = Box2 {
                        x: up.x + up.w * ci as f32 / n,
                        y: up.y,
                        w: up.w / n,
                        h: up.h,
                    };
                    glyphs.push(Glyph {
                        ch,
                        rect: Some(layer.unrotate_from_upright(slice)),
                        line,
                    });
                }
            }
        }
        PageChars {
            glyphs,
            upright: layer.upright,
        }
    }

    pub fn len(&self) -> usize {
        self.glyphs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }

    /// A box in the upright frame (y down; where on the page doesn't
    /// matter, only which way the text runs).
    fn up(&self, b: Box2) -> Box2 {
        Affine::upright(self.upright).apply_box(b)
    }

    fn up_pt(&self, p: Pt) -> Pt {
        Affine::upright(self.upright).apply(p)
    }

    /// The boundary before or after glyph `i`, whichever half of it `p`
    /// (upright) is in.
    fn side(&self, i: usize, p: Pt) -> usize {
        match self.glyphs[i].rect {
            Some(r) => {
                let r = self.up(r);
                if p.x > r.x + r.w / 2.0 { i + 1 } else { i }
            }
            None => i,
        }
    }

    /// The boundary at `p` when it's on a character (within `tolerance`
    /// points of its box); None off the text. A drag that starts here
    /// selects text.
    pub fn boundary_at(&self, p: Pt, tolerance: f32) -> Option<usize> {
        let i = self.glyphs.iter().position(|g| {
            g.rect.is_some_and(|r| {
                Box2 {
                    x: r.x - tolerance,
                    y: r.y - tolerance,
                    w: r.w + 2.0 * tolerance,
                    h: r.h + 2.0 * tolerance,
                }
                .contains(p)
            })
        })?;
        Some(self.side(i, self.up_pt(p)))
    }

    /// The boundary nearest `p`, wherever it is: on the line it's level
    /// with (or the nearest line), at the character nearest along it.
    /// Above the text is the start; below it, the end.
    pub fn boundary_near(&self, p: Pt) -> usize {
        let p = self.up_pt(p);
        // Each line's extent across (top, bottom) in the upright frame.
        let mut lines: Vec<(u32, f32, f32)> = Vec::new();
        for g in &self.glyphs {
            let Some(r) = g.rect else { continue };
            let r = self.up(r);
            match lines.last_mut() {
                Some((l, top, bottom)) if *l == g.line => {
                    *top = top.min(r.y);
                    *bottom = bottom.max(r.bottom());
                }
                _ => lines.push((g.line, r.y, r.bottom())),
            }
        }
        let (Some(first), Some(last)) = (lines.first(), lines.last()) else {
            return 0;
        };
        if p.y < first.1 {
            return 0;
        }
        if p.y > last.2 {
            return self.len();
        }
        let dist = |top: f32, bottom: f32| {
            if p.y < top {
                top - p.y
            } else if p.y > bottom {
                p.y - bottom
            } else {
                0.0
            }
        };
        let line = lines
            .iter()
            .min_by(|a, b| dist(a.1, a.2).total_cmp(&dist(b.1, b.2)))
            .map(|l| l.0)
            .unwrap_or(0);
        // The nearest character along that line.
        let along = |r: Box2| {
            if p.x < r.x {
                r.x - p.x
            } else if p.x > r.right() {
                p.x - r.right()
            } else {
                0.0
            }
        };
        let best = self
            .glyphs
            .iter()
            .enumerate()
            .filter(|(_, g)| g.line == line)
            .filter_map(|(i, g)| g.rect.map(|r| (i, self.up(r))))
            .min_by(|a, b| along(a.1).total_cmp(&along(b.1)));
        match best {
            Some((i, _)) => self.side(i, p),
            None => 0,
        }
    }

    /// Where a caret at boundary `at` goes (see [`caret_box`]): after the
    /// character before it, or before the one after when it starts a line
    /// or follows a space. None away from any character.
    pub fn caret_at(&self, at: usize) -> Option<Box2> {
        let before = at
            .checked_sub(1)
            .and_then(|i| self.glyphs.get(i))
            .and_then(|g| g.rect)
            .map(|r| (self.up(r), true));
        let after = self
            .glyphs
            .get(at)
            .and_then(|g| g.rect)
            .map(|r| (self.up(r), false));
        let (r, after_it) = before.or(after)?;
        let x = if after_it { r.right() } else { r.x };
        let b = caret_box(x, r.bottom(), r.h);
        // Back from the upright frame.
        Some(Affine::upright(self.upright).inverse().apply_box(b))
    }

    /// The word around boundary `at`: the run of characters that aren't
    /// whitespace, as two boundaries. Empty on whitespace.
    pub fn word_at(&self, at: usize) -> (usize, usize) {
        let word = |i: usize| !self.glyphs[i].ch.is_whitespace();
        // The character under the pointer: after the boundary, or before
        // it at a word's end.
        let i = if at < self.len() && word(at) {
            at
        } else if at > 0 && word(at - 1) {
            at - 1
        } else {
            return (at, at);
        };
        let mut a = i;
        while a > 0 && word(a - 1) {
            a -= 1;
        }
        let mut b = i + 1;
        while b < self.len() && word(b) {
            b += 1;
        }
        (a, b)
    }

    /// The words (runs of characters that aren't whitespace), each with
    /// its boundaries and box.
    pub fn words(&self) -> Vec<(String, (usize, usize), Box2)> {
        let mut out = Vec::new();
        let mut at = 0;
        while at < self.len() {
            if self.glyphs[at].ch.is_whitespace() {
                at += 1;
                continue;
            }
            // `at` starts a word here, so it runs on from it.
            let (a, b) = self.word_at(at);
            if let Some(r) = self.rects(a, b).into_iter().reduce(Box2::union) {
                out.push((self.text(a, b), (a, b), r));
            }
            at = b.max(at + 1);
        }
        out
    }

    /// The characters between two boundaries, in either order.
    pub fn text(&self, a: usize, b: usize) -> String {
        let (a, b) = (a.min(b).min(self.len()), a.max(b).min(self.len()));
        self.glyphs[a..b].iter().map(|g| g.ch).collect()
    }

    /// The boxes of the characters between two boundaries, one per line.
    pub fn rects(&self, a: usize, b: usize) -> Vec<Box2> {
        let (a, b) = (a.min(b).min(self.len()), a.max(b).min(self.len()));
        merge_lines(self.glyphs[a..b].iter())
    }

    /// The boxes of the characters whose centre is inside `rect`, one per
    /// line: what a highlight dragged over text covers.
    pub fn rects_in(&self, rect: Box2) -> Vec<Box2> {
        merge_lines(self.glyphs.iter().filter(|g| {
            g.rect.is_some_and(|r| {
                rect.contains(Pt {
                    x: r.x + r.w / 2.0,
                    y: r.y + r.h / 2.0,
                })
            })
        }))
    }
}

fn merge_lines<'a>(glyphs: impl Iterator<Item = &'a Glyph>) -> Vec<Box2> {
    let mut out: Vec<(u32, Box2)> = Vec::new();
    for g in glyphs {
        let Some(r) = g.rect else { continue };
        match out.last_mut() {
            Some((l, b)) if *l == g.line => *b = b.union(r),
            _ => out.push((g.line, r)),
        }
    }
    out.into_iter().map(|(_, b)| b).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text_layer::OcrWord;

    /// A Letter page turned `r`.
    fn letter(r: Rotation) -> Geom {
        Geom::read(None, None, r.degrees() as i64)
    }

    /// A point given as it shows on an upright Letter page, in PageSpace.
    fn shown(x: f32, y: f32) -> Pt {
        letter(Rotation::R0).from_view().apply(Pt { x, y })
    }

    /// A PageSpace box as it shows on an upright Letter page.
    fn as_shown(b: Box2) -> Box2 {
        letter(Rotation::R0).to_view().apply_box(b)
    }

    /// A 10-by-12-point character at (`x`, `y`) as shown.
    fn g(ch: char, x: f32, y: f32) -> (char, Option<Box2>) {
        (
            ch,
            Some(Box2 {
                x,
                y,
                w: 10.0,
                h: 12.0,
            }),
        )
    }

    /// "ab cd\nef": 10-point-wide characters, lines 20 points apart.
    fn sample() -> PageChars {
        PageChars::from_engine(
            vec![
                g('a', 100.0, 100.0),
                g('b', 110.0, 100.0),
                (' ', None),
                g('c', 130.0, 100.0),
                g('d', 140.0, 100.0),
                ('\r', None),
                ('\n', None),
                g('e', 100.0, 120.0),
                g('f', 110.0, 120.0),
            ],
            &letter(Rotation::R0),
        )
    }

    #[test]
    fn engine_text_reads_in_order_with_line_breaks() {
        let c = sample();
        assert_eq!(c.text(0, c.len()), "ab cd\nef");
        assert_eq!(c.glyphs.iter().filter(|g| g.line == 1).count(), 2);
        // Stored in PageSpace: 'a' is 100 to 112 down the page.
        let a = c.glyphs[0].rect.unwrap();
        assert_eq!((a.x, a.y, a.h), (100.0, 680.0, 12.0));
    }

    #[test]
    fn lines_follow_the_characters_not_the_engines_breaks() {
        let c = PageChars::from_engine(
            vec![
                g('a', 100.0, 100.0),
                ('\r', None),
                ('\n', None),
                g('b', 110.0, 100.0),
                // No break from the engine, but a new line.
                g('c', 100.0, 120.0),
            ],
            &letter(Rotation::R0),
        );
        assert_eq!(c.text(0, c.len()), "a b\nc");
        assert_eq!(c.rects(0, c.len()).len(), 2);
    }

    #[test]
    fn a_point_on_a_character_is_the_nearer_boundary() {
        let c = sample();
        assert_eq!(c.boundary_at(shown(102.0, 105.0), 0.0), Some(0));
        assert_eq!(c.boundary_at(shown(108.0, 105.0), 0.0), Some(1));
        assert_eq!(c.boundary_at(shown(300.0, 105.0), 0.0), None);
    }

    #[test]
    fn dragging_off_the_text_still_finds_a_boundary() {
        let c = sample();
        // Right of the first line: its end, before the break.
        assert_eq!(c.boundary_near(shown(400.0, 104.0)), 5);
        // Left of the second line: its start.
        assert_eq!(c.boundary_near(shown(10.0, 125.0)), 6);
        // Above and below everything.
        assert_eq!(c.boundary_near(shown(120.0, 10.0)), 0);
        assert_eq!(c.boundary_near(shown(120.0, 700.0)), c.len());
        // Selecting across the break takes it along.
        assert_eq!(c.text(3, 7), "cd\ne");
        assert_eq!(c.rects(3, 7).len(), 2);
    }

    #[test]
    fn carets_sit_between_characters() {
        let c = sample();
        // After "ab": at b's right edge, straddling the bottom of the text.
        let b = as_shown(c.caret_at(2).unwrap());
        assert_eq!(b.x + b.w / 2.0, 120.0);
        assert!(b.y < 112.0 && b.bottom() > 112.0);
        // At the start of the second line: before "e".
        let e = as_shown(c.caret_at(6).unwrap());
        assert_eq!(e.x + e.w / 2.0, 100.0);
        assert!(e.y > 120.0);
    }

    /// On a page the file turns 90°, the text reads along the page as it
    /// shows: up PageSpace's y.
    #[test]
    fn a_turned_page_reads_as_it_shows() {
        let base = letter(Rotation::R90);
        let c = PageChars::from_engine(vec![g('a', 100.0, 100.0), g('b', 110.0, 100.0)], &base);
        assert_eq!(c.upright, Rotation::R90);
        let (a, b) = (c.glyphs[0].rect.unwrap(), c.glyphs[1].rect.unwrap());
        assert!(b.y > a.y && a.x == b.x, "{a:?} {b:?}");
        // The right half of 'a' as shown is the boundary after it.
        let on_a = base.from_view().apply(Pt { x: 108.0, y: 105.0 });
        assert_eq!(c.boundary_at(on_a, 0.0), Some(1));
        // The caret after "ab" is at b's far edge as shown.
        let caret = base.to_view().apply_box(c.caret_at(2).unwrap());
        assert!((caret.x + caret.w / 2.0 - 120.0).abs() < 1e-3, "{caret:?}");
    }

    #[test]
    fn words_come_with_their_boxes() {
        let c = sample();
        let words: Vec<(String, f32)> = c.words().into_iter().map(|(w, _, r)| (w, r.x)).collect();
        assert_eq!(
            words,
            [
                ("ab".into(), 100.0),
                ("cd".into(), 130.0),
                ("ef".into(), 100.0)
            ]
        );
    }

    #[test]
    fn a_double_click_takes_the_word() {
        let c = sample();
        assert_eq!(c.word_at(4), (3, 5));
        assert_eq!(c.word_at(5), (3, 5));
        assert_eq!(c.word_at(0), (0, 2));
        assert_eq!(c.text(3, 5), "cd");
    }

    #[test]
    fn ocr_words_become_characters() {
        let words = vec![
            OcrWord {
                text: "Total".into(),
                rect: Box2 {
                    x: 100.0,
                    y: 100.0,
                    w: 50.0,
                    h: 12.0,
                },
                confidence: 0.9,
            },
            OcrWord {
                text: "42".into(),
                rect: Box2 {
                    x: 160.0,
                    y: 100.0,
                    w: 20.0,
                    h: 12.0,
                },
                confidence: 0.9,
            },
        ];
        let layer = TextLayer::from_engine(words, &letter(Rotation::R0), "test");
        let c = PageChars::from_layer(&layer);
        assert_eq!(c.text(0, c.len()), "Total 42");
        // "o" is the second tenth-point-wide slice of "Total".
        assert_eq!(c.glyphs[1].rect.unwrap().x, 110.0);
        let at = c.boundary_at(shown(172.0, 105.0), 0.0).unwrap();
        assert_eq!(c.text(at, c.len()), "2");
        let over = Box2 {
            x: 95.0,
            y: 95.0,
            w: 40.0,
            h: 20.0,
        };
        let over = letter(Rotation::R0).from_view().apply_box(over);
        assert_eq!(c.rects_in(over).len(), 1);
    }
}
