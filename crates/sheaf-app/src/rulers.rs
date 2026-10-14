//! Rulers, grid and guides (View > Rulers, Grid; drag from a ruler for a
//! guide). Everything measures the page as shown, from its top-left
//! corner, in inches. Guides apply to every page and last while the
//! document is open; they're display only.

use gpui::{
    AnyElement, Bounds, Context, Hsla, MouseButton, Pixels, Point, canvas, div, fill, point,
    prelude::*, px, size,
};
use sheaf_ui::theme;

use crate::docview::{DocView, State};
use crate::pane::Pane;

/// Ruler thickness, logical px.
pub(crate) const RULER: f32 = 18.0;
const PT_PER_INCH: f32 = 72.0;

/// A guide line on every page, in points from the shown page's top
/// (horizontal) or left (vertical) edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Guide {
    pub horizontal: bool,
    pub at: f32,
}

/// A guide being dragged out of a ruler.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GuideDrag {
    pub horizontal: bool,
    pub pane: usize,
    /// Where the pointer is, in window coordinates.
    pub pos: Option<Point<Pixels>>,
}

/// One ruler tick: where it is (points from the page edge) and how long
/// it's drawn: 0 for inches, 1 for halves, 2 for quarters, 3 for finer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Tick {
    pub pt: f32,
    pub level: u8,
    /// Inches, on labelled ticks.
    pub label: Option<i32>,
}

/// Ticks between `from` and `to` (points) at `k` logical px per point:
/// as fine as keeps ticks at least 5 px apart, and labels at least 36 px
/// apart.
pub(crate) fn ticks(from: f32, to: f32, k: f32) -> Vec<Tick> {
    let inch_px = PT_PER_INCH * k;
    if inch_px <= 0.0 || to <= from {
        return Vec::new();
    }
    let per_inch = [16u32, 8, 4, 2, 1]
        .into_iter()
        .find(|n| inch_px / *n as f32 >= 5.0)
        .unwrap_or(1);
    let label_every = [1i32, 2, 5, 10, 20, 50, 100]
        .into_iter()
        .find(|n| inch_px * *n as f32 >= 36.0)
        .unwrap_or(100);
    // Whole inches too far apart to draw every one: step by the label gap.
    let inch_step = if inch_px < 5.0 { label_every } else { 1 };
    let step = PT_PER_INCH / per_inch as f32;
    let mut out = Vec::new();
    let first = (from / step).floor() as i64;
    let last = (to / step).ceil() as i64;
    for i in first..=last {
        let sub = i.rem_euclid(per_inch as i64) as u32;
        let inch = i.div_euclid(per_inch as i64) as i32;
        if sub == 0 && inch % inch_step != 0 {
            continue;
        }
        let level = if sub == 0 {
            0
        } else if sub * 2 == per_inch {
            1
        } else if (sub * 4).is_multiple_of(per_inch) {
            2
        } else {
            3
        };
        out.push(Tick {
            pt: i as f32 * step,
            level,
            label: (sub == 0 && inch % label_every == 0).then_some(inch),
        });
    }
    out
}

/// A ruler `len` px long whose zero sits `origin` px from its start.
fn ruler(horizontal: bool, len: f32, origin: f32, k: f32) -> AnyElement {
    let ticks = ticks(-origin / k, (len - origin) / k, k);
    let tick_color: Hsla = theme::TEXT_MUTED.into();
    let marks = ticks.clone();
    let painter = canvas(
        |_, _, _| (),
        move |b: Bounds<Pixels>, _, window, _| {
            for t in &marks {
                let at = origin + t.pt * k;
                let long = RULER * [0.9, 0.55, 0.4, 0.25][t.level as usize];
                let r = if horizontal {
                    Bounds::new(
                        point(b.origin.x + px(at), b.origin.y + px(RULER - long)),
                        size(px(1.0), px(long)),
                    )
                } else {
                    Bounds::new(
                        point(b.origin.x + px(RULER - long), b.origin.y + px(at)),
                        size(px(long), px(1.0)),
                    )
                };
                window.paint_quad(fill(r, tick_color));
            }
        },
    )
    .absolute()
    .inset_0();
    let labels = ticks.into_iter().filter_map(|t| {
        let n = t.label?;
        let at = origin + t.pt * k;
        let el = div()
            .absolute()
            .text_size(px(9.0))
            .line_height(px(10.0))
            .text_color(theme::TEXT_MUTED)
            .child(n.to_string());
        Some(if horizontal {
            el.left(px(at + 2.0)).top(px(0.0))
        } else {
            el.top(px(at + 1.0)).left(px(2.0))
        })
    });
    div()
        .relative()
        .size_full()
        .overflow_hidden()
        .bg(theme::BACKGROUND)
        .child(painter)
        .children(labels)
        .into_any_element()
}

/// Lines every inch (and fainter at quarters, when they're far enough
/// apart) over a `w`×`h` px page.
pub(crate) fn grid(w: f32, h: f32, k: f32) -> AnyElement {
    let inch = PT_PER_INCH * k;
    let quarters = inch / 4.0 >= 8.0;
    let major: Hsla = theme::GRID_MAJOR.into();
    let minor: Hsla = theme::GRID_MINOR.into();
    canvas(
        |_, _, _| (),
        move |b: Bounds<Pixels>, _, window, _| {
            let step = if quarters { inch / 4.0 } else { inch };
            let mut line = |i: usize, at: f32, vertical: bool| {
                let color = if !quarters || i.is_multiple_of(4) {
                    major
                } else {
                    minor
                };
                let r = if vertical {
                    Bounds::new(point(b.origin.x + px(at), b.origin.y), size(px(1.0), px(h)))
                } else {
                    Bounds::new(point(b.origin.x, b.origin.y + px(at)), size(px(w), px(1.0)))
                };
                window.paint_quad(fill(r, color));
            };
            let mut i = 1;
            while i as f32 * step < w {
                line(i, i as f32 * step, true);
                i += 1;
            }
            let mut i = 1;
            while i as f32 * step < h {
                line(i, i as f32 * step, false);
                i += 1;
            }
        },
    )
    .absolute()
    .left(px(0.0))
    .top(px(0.0))
    .w(px(w))
    .h(px(h))
    .into_any_element()
}

/// The guides across a `w`×`h` px page at `k` px per point.
pub(crate) fn guide_lines(guides: &[Guide], w: f32, h: f32, k: f32) -> Vec<AnyElement> {
    guides
        .iter()
        .filter_map(|g| {
            let at = g.at * k;
            let el = div().absolute().bg(theme::GUIDE);
            if g.horizontal {
                (at >= 0.0 && at <= h).then(|| el.left(px(0.0)).top(px(at)).w(px(w)).h(px(1.0)))
            } else {
                (at >= 0.0 && at <= w).then(|| el.top(px(0.0)).left(px(at)).h(px(h)).w(px(1.0)))
            }
        })
        .map(IntoElement::into_any_element)
        .collect()
}

impl DocView {
    /// View > Rulers (Ctrl+R).
    pub(crate) fn toggle_rulers(&mut self, cx: &mut Context<Self>) {
        self.rulers = !self.rulers;
        cx.notify();
    }

    /// View > Grid (Ctrl+U).
    pub(crate) fn toggle_grid(&mut self, cx: &mut Context<Self>) {
        self.grid = !self.grid;
        cx.notify();
    }

    /// View > Clear Guides.
    pub(crate) fn clear_guides(&mut self, cx: &mut Context<Self>) {
        if let State::Ready(doc) = &mut self.state {
            let n = std::mem::take(&mut self.guides).len();
            doc.notice = Some(match n {
                0 => "No guides to clear".into(),
                1 => "Cleared 1 guide".into(),
                n => format!("Cleared {n} guides"),
            });
        }
        cx.notify();
    }

    fn start_guide(&mut self, horizontal: bool, pane: usize, cx: &mut Context<Self>) {
        self.guide_drag = Some(GuideDrag {
            horizontal,
            pane,
            pos: None,
        });
        cx.stop_propagation();
        cx.notify();
    }

    pub(crate) fn move_guide(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        if let Some(d) = &mut self.guide_drag {
            d.pos = Some(pos);
            cx.notify();
        }
    }

    /// Drop the dragged guide where the pointer is, if that's over a page
    /// in the pane it came from.
    pub(crate) fn finish_guide(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.guide_drag.take() else {
            return;
        };
        cx.notify();
        let (State::Ready(doc), Some(pos)) = (&mut self.state, drag.pos) else {
            return;
        };
        let Some(pane) = doc.panes.get(drag.pane) else {
            return;
        };
        let Some(layout) = &pane.layout else { return };
        let b = pane.scroll.bounds();
        if !b.contains(&pos) {
            return;
        }
        let x = f32::from(pos.x - b.origin.x - pane.scroll.offset().x);
        let y = f32::from(pos.y - b.origin.y - pane.scroll.offset().y);
        let Some(r) = layout.page_at(x, y).and_then(|i| layout.rect(i)) else {
            return;
        };
        let at = if drag.horizontal {
            (y - r.top) / layout.k
        } else {
            (x - r.left) / layout.k
        };
        self.guides.push(Guide {
            horizontal: drag.horizontal,
            at,
        });
        doc.notice = Some(format!(
            "Guide at {:.2} in — View > Clear Guides removes them",
            at / PT_PER_INCH
        ));
    }
}

/// Wrap pane `ix`'s scroll area with its rulers (when shown) and the guide
/// being dragged (`preview`: horizontal, and its offset in the area).
pub(crate) fn with_rulers(
    ix: usize,
    rulers: bool,
    preview: Option<(bool, Pixels)>,
    pane: &Pane,
    pane_el: AnyElement,
    cx: &mut Context<DocView>,
) -> AnyElement {
    let Some(layout) = &pane.layout else {
        return pane_el;
    };
    let scroll = (
        -f32::from(pane.scroll.offset().x),
        -f32::from(pane.scroll.offset().y),
    );
    let current = pane.current_page;
    let area = div()
        .relative()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .child(pane_el)
        .when_some(preview, |d, (horizontal, at)| {
            let line = div().absolute().bg(theme::GUIDE);
            d.child(if horizontal {
                line.left(px(0.0)).right(px(0.0)).top(at).h(px(1.0))
            } else {
                line.top(px(0.0)).bottom(px(0.0)).left(at).w(px(1.0))
            })
        });
    if !rulers {
        return div().size_full().flex().child(area).into_any_element();
    }
    let (sx, sy) = scroll;
    let (ox, oy) = layout
        .rect(current)
        .map(|r| (r.left - sx, r.top - sy))
        .unwrap_or((0.0, 0.0));
    let k = layout.k;
    let top = div()
        .id(("ruler-h", ix))
        .flex_1()
        .h(px(RULER))
        .cursor(gpui::CursorStyle::ResizeRow)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |v, _, _, cx| v.start_guide(true, ix, cx)),
        )
        .child(div().size_full().child(ruler(true, layout.view_w, ox, k)));
    let left = div()
        .id(("ruler-v", ix))
        .w(px(RULER))
        .h_full()
        .cursor(gpui::CursorStyle::ResizeColumn)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |v, _, _, cx| v.start_guide(false, ix, cx)),
        )
        .child(div().size_full().child(ruler(false, layout.view_h, oy, k)));
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .flex_shrink_0()
                .child(div().size(px(RULER)).bg(theme::BACKGROUND))
                .child(top),
        )
        .child(div().flex().flex_1().min_h(px(0.0)).child(left).child(area))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_at_100_percent_are_sixteenths_with_inch_labels() {
        // 96 px per inch: sixteenths are 6 px apart.
        let k = 96.0 / 72.0;
        let t = ticks(0.0, 72.0, k);
        assert_eq!(t.len(), 17);
        assert_eq!(t[0].label, Some(0));
        assert_eq!(t[8].level, 1);
        assert_eq!(t[4].level, 2);
        assert_eq!(t[1].level, 3);
        assert_eq!(t[16].label, Some(1));
    }

    #[test]
    fn zoomed_out_ticks_thin_and_labels_spread() {
        // 7.2 px per inch: whole inches only, labelled every fifth.
        let t = ticks(0.0, 720.0, 0.1);
        assert!(t.iter().all(|t| t.level == 0));
        let labels: Vec<i32> = t.iter().filter_map(|t| t.label).collect();
        assert_eq!(labels, [0, 5, 10]);
        // Left of the page counts down from zero.
        let t = ticks(-144.0, 0.0, 96.0 / 72.0);
        assert_eq!(t.first().unwrap().label, Some(-2));
    }
}
