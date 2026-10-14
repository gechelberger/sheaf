//! How a document pane shows its pages: the page mode (single or two-up,
//! continuous or one spread at a time), rotate view, zoom, and the page
//! geometry that follows from them. A document has one pane, or two when
//! the window is split (see todos/m7.md).

use std::ops::Range;
use std::time::Instant;

use gpui::ScrollHandle;
use sheaf_core::{PageSize, Rotation};
use sheaf_doc::{Document, PageRef};

use crate::docview::ScrollTo;

/// Space around the page column and between pages, in logical px.
pub(crate) const MARGIN: f32 = 16.0;
pub(crate) const GAP: f32 = 12.0;
/// 100% zoom = 96 logical px per inch = 96/72 px per PDF point (Acrobat's convention).
pub(crate) const PX_PER_PT_AT_100: f32 = 96.0 / 72.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Zoom {
    FitWidth,
    FitPage,
    /// 1.0 = 100%.
    Custom(f32),
}

/// How many pages sit side by side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spread {
    Single,
    /// Pages 1–2, 3–4, …
    TwoUp,
    /// Page 1 alone on the right, then 2–3, 4–5, …: a book's spreads.
    TwoUpCover,
}

/// Acrobat's Page Display modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageMode {
    pub spread: Spread,
    /// Scroll through every page; otherwise show one row (a page or a
    /// spread) at a time.
    pub continuous: bool,
}

impl Default for PageMode {
    fn default() -> Self {
        Self {
            spread: Spread::Single,
            continuous: true,
        }
    }
}

impl PageMode {
    /// The row (page or spread) page `i` is in.
    pub fn row_of(self, i: usize) -> usize {
        match self.spread {
            Spread::Single => i,
            Spread::TwoUp => i / 2,
            Spread::TwoUpCover => i.div_ceil(2),
        }
    }

    pub fn row_count(self, pages: usize) -> usize {
        if pages == 0 {
            0
        } else {
            self.row_of(pages - 1) + 1
        }
    }

    /// The pages in row `r` of a `pages`-page document.
    pub fn row_pages(self, r: usize, pages: usize) -> Range<usize> {
        let range = match self.spread {
            Spread::Single => r..r + 1,
            Spread::TwoUp => 2 * r..2 * r + 2,
            Spread::TwoUpCover if r == 0 => 0..1,
            Spread::TwoUpCover => 2 * r - 1..2 * r + 1,
        };
        range.start.min(pages)..range.end.min(pages)
    }

    fn columns(self) -> usize {
        match self.spread {
            Spread::Single => 1,
            _ => 2,
        }
    }

    /// In a two-up mode, whether page `i` sits on the right of its spread.
    fn on_right(self, i: usize) -> bool {
        match self.spread {
            Spread::Single => false,
            Spread::TwoUp => i % 2 == 1,
            Spread::TwoUpCover => i.is_multiple_of(2),
        }
    }

    pub fn label(self) -> &'static str {
        match (self.spread, self.continuous) {
            (Spread::Single, true) => "single page continuous",
            (Spread::Single, false) => "single page",
            (Spread::TwoUp, true) => "two-up continuous",
            (Spread::TwoUp, false) => "two-up",
            (Spread::TwoUpCover, true) => "two-up continuous, cover page",
            (Spread::TwoUpCover, false) => "two-up, cover page",
        }
    }
}

/// Where a page is drawn, in the pane's content (logical px).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PageRect {
    pub left: f32,
    pub top: f32,
    pub w: f32,
    pub h: f32,
}

impl PageRect {
    fn contains_x(&self, x: f32) -> bool {
        x >= self.left && x <= self.left + self.w
    }
}

#[derive(Debug, Clone)]
struct Row {
    pages: Range<usize>,
    top: f32,
}

/// Page geometry in logical px for one pane, at one zoom and page mode, for
/// one snapshot of the page sequence. In a non-continuous mode only the
/// shown row has geometry.
pub(crate) struct Layout {
    pub(crate) zoom: Zoom,
    pub(crate) mode: PageMode,
    /// The pane's rotate view.
    pub(crate) rotation: Rotation,
    /// The row shown in a non-continuous mode.
    pub(crate) row: usize,
    pub(crate) view_w: f32,
    pub(crate) view_h: f32,
    pub(crate) version: u64,
    /// Logical px per PDF point.
    pub(crate) k: f32,
    /// Every page, with the pane's rotate view added to its rotation: what
    /// to render and how to map between the page and the screen.
    pub(crate) refs: Vec<PageRef>,
    rects: Vec<Option<PageRect>>,
    /// Rows that have geometry, top to bottom.
    rows: Vec<Row>,
    /// Scrollable content size.
    pub(crate) total_w: f32,
    pub(crate) total_h: f32,
}

/// What a layout is built from.
pub(crate) struct LayoutSpec {
    pub zoom: Zoom,
    pub mode: PageMode,
    pub rotation: Rotation,
    pub row: usize,
    pub view_w: f32,
    pub view_h: f32,
    pub version: u64,
}

impl Layout {
    pub(crate) fn build(spec: LayoutSpec, model: &Document) -> Self {
        let refs: Vec<PageRef> = model
            .state()
            .pages
            .iter()
            .map(|p| PageRef {
                rotation: p.rotation.plus(spec.rotation),
                ..*p
            })
            .collect();
        let sizes: Vec<PageSize> = refs.iter().map(|r| model.page_size(r)).collect();
        Self::from_sizes(spec, refs, &sizes)
    }

    fn from_sizes(spec: LayoutSpec, refs: Vec<PageRef>, sizes: &[PageSize]) -> Self {
        let LayoutSpec {
            zoom,
            mode,
            rotation,
            view_w,
            view_h,
            version,
            ..
        } = spec;
        let n = sizes.len();
        let row_count = mode.row_count(n);
        let row = spec.row.min(row_count.saturating_sub(1));
        let cols = mode.columns() as f32;
        let gaps = (cols - 1.0) * GAP;
        let avail_w = (view_w - 2.0 * MARGIN - gaps).max(1.0);
        let avail_h = (view_h - 2.0 * MARGIN).max(1.0);
        let (mut max_w, mut max_h) = (1.0f32, 1.0f32);
        for s in sizes {
            max_w = max_w.max(s.width);
            max_h = max_h.max(s.height);
        }
        let k = match zoom {
            Zoom::FitWidth => avail_w / (cols * max_w),
            Zoom::FitPage => (avail_w / (cols * max_w)).min(avail_h / max_h),
            Zoom::Custom(z) => z * PX_PER_PT_AT_100,
        };
        let content_w = cols * max_w * k + gaps;
        let total_w = (content_w + 2.0 * MARGIN).max(view_w);
        let center = total_w / 2.0;

        let shown = if mode.continuous {
            0..row_count
        } else {
            row..(row + 1).min(row_count)
        };
        let mut rects = vec![None; n];
        let mut rows = Vec::with_capacity(shown.len());
        let mut y = MARGIN;
        for r in shown {
            let pages = mode.row_pages(r, n);
            let row_h = pages
                .clone()
                .map(|i| sizes[i].height * k)
                .fold(0.0, f32::max);
            for i in pages.clone() {
                let (w, h) = (sizes[i].width * k, sizes[i].height * k);
                let left = if mode.spread == Spread::Single {
                    (total_w - w) / 2.0
                } else if mode.on_right(i) {
                    center + GAP / 2.0
                } else {
                    center - GAP / 2.0 - w
                };
                rects[i] = Some(PageRect {
                    left,
                    top: y + (row_h - h) / 2.0,
                    w,
                    h,
                });
            }
            rows.push(Row { pages, top: y });
            y += row_h + GAP;
        }
        let content_h = if rows.is_empty() { y } else { y - GAP } + MARGIN;
        // One spread at a time sits in the middle of the pane when it fits.
        if !mode.continuous && content_h < view_h {
            let dy = (view_h - content_h) / 2.0;
            for r in &mut rows {
                r.top += dy;
            }
            for rect in rects.iter_mut().flatten() {
                rect.top += dy;
            }
        }
        Self {
            zoom,
            mode,
            rotation,
            row,
            view_w,
            view_h,
            version,
            k,
            refs,
            rects,
            rows,
            total_w,
            total_h: content_h.max(view_h),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.refs.len()
    }

    /// Where page `i` is drawn, if it's laid out (in a non-continuous mode,
    /// only the shown row is).
    pub(crate) fn rect(&self, i: usize) -> Option<PageRect> {
        self.rects.get(i).copied().flatten()
    }

    /// Index into `rows` of the row whose span contains `y` (or the last one
    /// that starts above it).
    fn row_index_at(&self, y: f32) -> Option<usize> {
        if self.rows.is_empty() {
            return None;
        }
        Some(self.rows.partition_point(|r| r.top <= y).saturating_sub(1))
    }

    /// The page under content point (`x`, `y`): in its row, the one whose
    /// span contains `x`, else the nearest (the first on a tie).
    pub(crate) fn page_at(&self, x: f32, y: f32) -> Option<usize> {
        let row = &self.rows[self.row_index_at(y)?];
        let mut best: Option<(usize, f32)> = None;
        for i in row.pages.clone() {
            let Some(r) = self.rect(i) else { continue };
            if r.contains_x(x) {
                return Some(i);
            }
            let d = (x - (r.left + r.w / 2.0)).abs();
            if best.is_none_or(|(_, bd)| d < bd) {
                best = Some((i, d));
            }
        }
        best.map(|(i, _)| i)
    }

    /// Pages in rows that intersect `top..bottom`.
    pub(crate) fn visible(&self, top: f32, bottom: f32) -> Range<usize> {
        let (Some(a), Some(b)) = (self.row_index_at(top), self.row_index_at(bottom)) else {
            return 0..0;
        };
        self.rows[a].pages.start..self.rows[b].pages.end
    }

    /// Top of the row page `i` is in, if laid out.
    pub(crate) fn row_top_of(&self, i: usize) -> Option<f32> {
        self.rows
            .iter()
            .find(|r| r.pages.contains(&i))
            .map(|r| r.top)
    }

    pub(crate) fn zoom_percent(&self) -> f32 {
        self.k / PX_PER_PT_AT_100 * 100.0
    }
}

/// One view onto a document: its own zoom, page mode, rotate view and
/// scroll position. Render caches are shared between panes.
pub(crate) struct Pane {
    pub(crate) zoom: Zoom,
    pub(crate) mode: PageMode,
    /// Rotate View: display only, on top of each page's own rotation.
    pub(crate) rotation: Rotation,
    /// The row shown in a non-continuous mode.
    pub(crate) row: usize,
    pub(crate) scroll: ScrollHandle,
    pub(crate) layout: Option<Layout>,
    pub(crate) pending_scroll: Option<ScrollTo>,
    pub(crate) current_page: usize,
    /// Last page turn by scrolling past a spread's end, so a trackpad's
    /// stream of wheel events turns one page, not ten.
    pub(crate) last_turn: Option<Instant>,
}

impl Pane {
    pub(crate) fn new(zoom: Zoom, mode: PageMode) -> Self {
        Self {
            zoom,
            mode,
            rotation: Rotation::R0,
            row: 0,
            scroll: ScrollHandle::new(),
            layout: None,
            pending_scroll: None,
            current_page: 0,
            last_turn: None,
        }
    }

    /// Page `p` as this pane shows it: with rotate view added.
    pub(crate) fn shown(&self, p: PageRef) -> PageRef {
        PageRef {
            rotation: p.rotation.plus(self.rotation),
            ..p
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sheaf_doc::{PageId, SourceId};

    fn layout(mode: PageMode, zoom: Zoom, row: usize, sizes: &[(f32, f32)]) -> Layout {
        let refs = (0..sizes.len())
            .map(|i| PageRef {
                id: PageId(i as u64),
                source: SourceId(0),
                index: i as u32,
                rotation: Rotation::R0,
            })
            .collect();
        let sizes: Vec<PageSize> = sizes
            .iter()
            .map(|&(width, height)| PageSize { width, height })
            .collect();
        Layout::from_sizes(
            LayoutSpec {
                zoom,
                mode,
                rotation: Rotation::R0,
                row,
                view_w: 1000.0,
                view_h: 800.0,
                version: 0,
            },
            refs,
            &sizes,
        )
    }

    const LETTER: (f32, f32) = (612.0, 792.0);

    fn mode(spread: Spread, continuous: bool) -> PageMode {
        PageMode { spread, continuous }
    }

    #[test]
    fn rows_group_pages_by_spread() {
        let two = mode(Spread::TwoUp, true);
        assert_eq!(two.row_count(5), 3);
        assert_eq!(two.row_pages(2, 5), 4..5);
        let cover = mode(Spread::TwoUpCover, true);
        assert_eq!(cover.row_count(4), 3);
        assert_eq!(cover.row_pages(0, 4), 0..1);
        assert_eq!(cover.row_pages(1, 4), 1..3);
        assert_eq!(cover.row_pages(2, 4), 3..4);
        assert_eq!(cover.row_of(3), 2);
        let single = PageMode::default();
        assert_eq!(single.row_count(0), 0);
        assert_eq!(single.row_pages(3, 10), 3..4);
    }

    #[test]
    fn two_up_puts_pages_either_side_of_the_gutter() {
        let l = layout(mode(Spread::TwoUp, true), Zoom::FitWidth, 0, &[LETTER; 3]);
        let (a, b) = (l.rect(0).unwrap(), l.rect(1).unwrap());
        assert!((a.top - b.top).abs() < 0.01);
        assert!((b.left - (a.left + a.w) - GAP).abs() < 0.01);
        // Fit width fills the pane with the spread.
        assert!((a.left - MARGIN).abs() < 0.5);
        // The odd last page sits on the left, in its own row.
        let c = l.rect(2).unwrap();
        assert!((c.left - a.left).abs() < 0.01);
        assert!(c.top > a.top + a.h);
        assert_eq!(l.page_at(b.left + 5.0, b.top + 5.0), Some(1));
        assert_eq!(l.visible(a.top, a.top + 1.0), 0..2);
    }

    #[test]
    fn cover_page_sits_on_the_right() {
        let l = layout(
            mode(Spread::TwoUpCover, true),
            Zoom::FitWidth,
            0,
            &[LETTER; 3],
        );
        let (cover, left, right) = (l.rect(0).unwrap(), l.rect(1).unwrap(), l.rect(2).unwrap());
        assert!((cover.left - right.left).abs() < 0.01);
        assert!(left.left < cover.left);
    }

    #[test]
    fn one_spread_at_a_time_lays_out_only_that_row() {
        let l = layout(mode(Spread::Single, false), Zoom::FitPage, 2, &[LETTER; 5]);
        assert!(l.rect(1).is_none() && l.rect(3).is_none());
        let r = l.rect(2).unwrap();
        // Centred vertically, and nothing to scroll.
        assert!((r.top + r.h / 2.0 - 400.0).abs() < 0.5);
        assert_eq!(l.total_h, 800.0);
        assert_eq!(l.page_at(500.0, 400.0), Some(2));
        assert_eq!(l.visible(0.0, 800.0), 2..3);
        // A row past the end clamps to the last.
        let l = layout(mode(Spread::Single, false), Zoom::FitPage, 9, &[LETTER; 5]);
        assert!(l.rect(4).is_some());
        assert_eq!(l.row, 4);
    }

    #[test]
    fn gutter_counts_as_the_nearest_page() {
        let l = layout(
            mode(Spread::TwoUp, true),
            Zoom::Custom(0.5),
            0,
            &[LETTER; 2],
        );
        let a = l.rect(0).unwrap();
        // Dead centre between the two: the first wins the tie.
        let x = a.left + a.w + GAP / 2.0;
        assert_eq!(l.page_at(x, a.top + 10.0), Some(0));
        assert_eq!(l.page_at(5.0, a.top + 10.0), Some(0));
        assert_eq!(l.page_at(995.0, a.top + 10.0), Some(1));
    }
}
