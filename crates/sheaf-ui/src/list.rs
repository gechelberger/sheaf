//! Virtualized lists: only the rows in view are built. A `ListLayout` holds
//! each row's top (rows of one height or of many); the list is a scrolling
//! div holding `body()`, a block the full list's height, with the visible
//! rows placed in it by `place(i)`.
//!
//! ```ignore
//! let layout = ListLayout::fixed(rows.len(), ROW_H).padded(0.0, PAD);
//! let els = layout.visible(&scroll).map(|i| layout.place(i).child(..));
//! div().id("list").track_scroll(&scroll).overflow_y_scroll()
//!     .child(layout.body().children(els))
//! ```

use std::ops::Range;

use gpui::{Div, ScrollHandle, div, point, prelude::*, px};

#[derive(Clone, Debug, PartialEq)]
pub struct ListLayout {
    /// Row `i` spans `tops[i]..tops[i + 1]`; one more entry than rows.
    tops: Vec<f32>,
    /// Space after the last row.
    bottom_pad: f32,
}

impl ListLayout {
    /// `len` rows of `row_h` each.
    pub fn fixed(len: usize, row_h: f32) -> Self {
        Self::variable(std::iter::repeat_n(row_h, len))
    }

    /// Rows of the given heights, in order.
    pub fn variable(heights: impl IntoIterator<Item = f32>) -> Self {
        let mut tops = vec![0.0];
        let mut y = 0.0;
        for h in heights {
            y += h;
            tops.push(y);
        }
        Self {
            tops,
            bottom_pad: 0.0,
        }
    }

    /// Empty space above the first row and below the last.
    pub fn padded(mut self, top: f32, bottom: f32) -> Self {
        for t in &mut self.tops {
            *t += top;
        }
        self.bottom_pad = bottom;
        self
    }

    pub fn len(&self) -> usize {
        self.tops.len() - 1
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn top(&self, i: usize) -> f32 {
        self.tops[i]
    }

    pub fn height(&self, i: usize) -> f32 {
        self.tops[i + 1] - self.tops[i]
    }

    /// The whole list's height, padding included.
    pub fn total_height(&self) -> f32 {
        self.tops[self.len()] + self.bottom_pad
    }

    /// The row at list-relative `y`, clamped to the first and last rows.
    /// None for an empty list.
    pub fn row_at(&self, y: f32) -> Option<usize> {
        let n = self.len();
        (n > 0).then(|| self.tops[1..n].partition_point(|&t| t <= y))
    }

    /// The rows that overlap `scroll_y..scroll_y + view_h`.
    pub fn visible_in(&self, scroll_y: f32, view_h: f32) -> Range<usize> {
        let n = self.len();
        let bottom = scroll_y + view_h;
        let r0 = self.tops[1..].partition_point(|&b| b <= scroll_y);
        let r1 = self.tops[..n].partition_point(|&t| t < bottom);
        r0.min(r1)..r1
    }

    /// The rows in view in the list `scroll` tracks.
    pub fn visible(&self, scroll: &ScrollHandle) -> Range<usize> {
        let (scroll_y, view_h) = view(scroll);
        self.visible_in(scroll_y, view_h)
    }

    /// Where to scroll so row `i` is in view, `margin` clear of the edges,
    /// moving as little as possible; None if it already is.
    pub fn reveal_in(&self, i: usize, margin: f32, scroll_y: f32, view_h: f32) -> Option<f32> {
        let max_y = (self.total_height() - view_h).max(0.0);
        let (top, bottom) = (self.tops[i], self.tops[i + 1]);
        let to = if top < scroll_y {
            top - margin
        } else if bottom > scroll_y + view_h {
            bottom - view_h + margin
        } else {
            return None;
        };
        Some(to.clamp(0.0, max_y))
    }

    /// Scroll `scroll` so row `i` is in view. Returns the new offset (or
    /// the current one, if it didn't move), as a positive distance.
    pub fn reveal(&self, i: usize, margin: f32, scroll: &ScrollHandle) -> f32 {
        let (scroll_y, view_h) = view(scroll);
        match self.reveal_in(i, margin, scroll_y, view_h) {
            Some(y) => {
                scroll.set_offset(point(px(0.0), px(-y)));
                y
            }
            None => scroll_y,
        }
    }

    /// A full-width box at row `i`'s place in `body()`.
    pub fn place(&self, i: usize) -> Div {
        div()
            .absolute()
            .top(px(self.top(i)))
            .left(px(0.0))
            .w_full()
            .h(px(self.height(i)))
    }

    /// The block, as tall as the whole list, that the rows go in.
    pub fn body(&self) -> Div {
        div().relative().w_full().h(px(self.total_height()))
    }
}

/// Before its first layout a list's view has no height yet; build rows as
/// if it were this tall, so that frame isn't empty.
const UNLAID_VIEW_H: f32 = 2000.0;

/// The scroll offset (as a positive distance) and height of the view.
fn view(scroll: &ScrollHandle) -> (f32, f32) {
    let h = f32::from(scroll.bounds().size.height);
    let view_h = if h > 0.0 { h } else { UNLAID_VIEW_H };
    let scroll_y = (-f32::from(scroll.offset().y)).max(0.0);
    (scroll_y, view_h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_rows() {
        let l = ListLayout::fixed(100, 10.0).padded(0.0, 4.0);
        assert_eq!(l.total_height(), 1004.0);
        assert_eq!(l.visible_in(0.0, 25.0), 0..3);
        assert_eq!(l.visible_in(10.0, 20.0), 1..3);
        assert_eq!(l.visible_in(15.0, 20.0), 1..4);
        assert_eq!(l.visible_in(995.0, 50.0), 99..100);
        assert_eq!(l.row_at(-5.0), Some(0));
        assert_eq!(l.row_at(19.9), Some(1));
        assert_eq!(l.row_at(5000.0), Some(99));
    }

    #[test]
    fn variable_rows_and_padding() {
        let l = ListLayout::variable([10.0, 30.0, 10.0]).padded(5.0, 5.0);
        assert_eq!((l.top(1), l.height(1)), (15.0, 30.0));
        assert_eq!(l.total_height(), 60.0);
        assert_eq!(l.visible_in(0.0, 10.0), 0..1);
        assert_eq!(l.visible_in(20.0, 5.0), 1..2);
        assert_eq!(l.visible_in(0.0, 100.0), 0..3);
        assert_eq!(l.row_at(44.0), Some(1));
        assert_eq!(l.row_at(45.0), Some(2));
    }

    #[test]
    fn empty_list() {
        let l = ListLayout::fixed(0, 10.0);
        assert!(l.is_empty());
        assert_eq!(l.visible_in(0.0, 100.0), 0..0);
        assert_eq!(l.row_at(3.0), None);
    }

    #[test]
    fn reveal_moves_the_least() {
        let l = ListLayout::fixed(100, 10.0);
        assert_eq!(l.reveal_in(5, 0.0, 0.0, 100.0), None);
        assert_eq!(l.reveal_in(12, 0.0, 0.0, 100.0), Some(30.0));
        assert_eq!(l.reveal_in(2, 4.0, 50.0, 100.0), Some(16.0));
        assert_eq!(l.reveal_in(99, 8.0, 0.0, 100.0), Some(900.0));
    }
}
