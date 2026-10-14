//! A toolbar: a row (or column) of command buttons, separators and the
//! odd widget (a color picker, say). It knows nothing about where its items
//! come from: the app hands it a resolved list each frame with
//! [`Toolbar::set_items`] and dispatches what [`ToolbarEvent::Run`] asks
//! for.
//!
//! Items that don't fit go into a `»` dropdown at the end. Lengths along
//! the bar are fixed per kind of item (widgets say theirs), so what fits is
//! known before layout; the space available is measured each frame.

use gpui::{
    Action, AnyElement, Context, EventEmitter, MouseButton, Pixels, Point, SharedString, Window,
    anchored, canvas, deferred, div, prelude::*, px, svg,
};

use crate::theme;
use crate::tooltip::Tooltip;

/// Square buttons, logical px.
pub const BUTTON: f32 = 26.0;
const GAP: f32 = 4.0;
const SEPARATOR: f32 = 9.0;
/// Padding at each end of the bar.
const PAD: f32 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Orientation {
    #[default]
    Horizontal,
    Vertical,
}

/// A command button.
pub struct Button {
    /// Stable within the bar; element ids derive from it.
    pub key: SharedString,
    /// Asset path of the artwork.
    pub icon: SharedString,
    pub title: SharedString,
    pub help: SharedString,
    pub action: Box<dyn Action>,
    pub enabled: bool,
    /// Drawn as pressed: a tool in use, a panel showing.
    pub checked: bool,
}

pub enum Item {
    Button(Button),
    Separator,
    /// Something the app draws itself, `length` px along the bar.
    Widget {
        key: SharedString,
        title: SharedString,
        length: f32,
        element: AnyElement,
    },
}

impl Item {
    fn length(&self) -> f32 {
        match self {
            Item::Button(_) => BUTTON,
            Item::Separator => SEPARATOR - GAP,
            Item::Widget { length, .. } => *length,
        }
    }
}

pub enum ToolbarEvent {
    Run(Box<dyn Action>),
    /// Right-click: on the item at this index of the last `set_items`, or
    /// on the bar itself (`None`), at a window position.
    Context {
        item: Option<usize>,
        position: Point<Pixels>,
    },
}

pub struct Toolbar {
    items: Vec<Item>,
    orientation: Orientation,
    /// Space along the bar, measured last frame.
    extent: Option<f32>,
    overflow_open: bool,
}

impl EventEmitter<ToolbarEvent> for Toolbar {}

impl Toolbar {
    pub fn new(orientation: Orientation) -> Self {
        Self {
            items: Vec::new(),
            orientation,
            extent: None,
            overflow_open: false,
        }
    }

    /// The items to draw next. Call each frame: widget elements are used up
    /// by drawing.
    pub fn set_items(&mut self, items: Vec<Item>) {
        self.items = items;
    }

    pub fn set_orientation(&mut self, orientation: Orientation) {
        self.orientation = orientation;
    }

    pub fn orientation(&self) -> Orientation {
        self.orientation
    }

    /// The bar's length along its axis with every item showing.
    pub fn natural_length(&self) -> f32 {
        natural(&self.items)
    }

    /// How many leading items fit in `extent` px; the rest overflow.
    fn fits(&self) -> usize {
        let Some(extent) = self.extent else {
            return self.items.len();
        };
        if natural(&self.items) <= extent {
            return self.items.len();
        }
        // Leave room for the overflow button.
        let room = extent - 2.0 * PAD - BUTTON - GAP;
        let mut used = 0.0;
        let mut n = 0;
        for item in &self.items {
            let len = item.length() + GAP;
            if used + len > room {
                break;
            }
            used += len;
            n += 1;
        }
        // Don't end on a separator.
        while n > 0 && matches!(self.items[n - 1], Item::Separator) {
            n -= 1;
        }
        n
    }

    fn run(&mut self, action: Box<dyn Action>, cx: &mut Context<Self>) {
        self.overflow_open = false;
        cx.emit(ToolbarEvent::Run(action));
        cx.notify();
    }
}

fn natural(items: &[Item]) -> f32 {
    let along: f32 = items.iter().map(|i| i.length() + GAP).sum();
    2.0 * PAD + (along - GAP).max(0.0)
}

fn button(ix: usize, b: Button, cx: &mut Context<Toolbar>) -> AnyElement {
    let Button {
        key,
        icon,
        title,
        help,
        action,
        enabled,
        checked,
    } = b;
    let fg = match (enabled, checked) {
        (false, _) => theme::TEXT_DISABLED,
        (true, true) => theme::TEXT_BRIGHT,
        (true, false) => theme::TEXT_SUBTLE,
    };
    let tip = Tooltip::for_action(title, help, action.boxed_clone());
    div()
        .id(SharedString::from(format!("tb-{key}")))
        .size(px(BUTTON))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .bg(if checked {
            theme::ACCENT
        } else {
            theme::CONTROL
        })
        .when(enabled, |d| {
            d.cursor_pointer()
                .hover(move |s| {
                    s.bg(if checked {
                        theme::ACCENT_HOVER
                    } else {
                        theme::CONTROL_HOVER
                    })
                })
                .on_click(cx.listener(move |bar, _, _, cx| bar.run(action.boxed_clone(), cx)))
        })
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |_, e: &gpui::MouseDownEvent, _, cx| {
                cx.stop_propagation();
                cx.emit(ToolbarEvent::Context {
                    item: Some(ix),
                    position: e.position,
                });
            }),
        )
        .tooltip(tip)
        .child(svg().path(icon).size(px(18.0)).text_color(fg))
        .into_any_element()
}

fn separator(vertical: bool) -> AnyElement {
    let d = div().flex_shrink_0().bg(theme::DIVIDER);
    if vertical {
        d.h(px(1.0)).w(px(BUTTON - 6.0)).into_any_element()
    } else {
        d.w(px(1.0)).h(px(BUTTON - 6.0)).into_any_element()
    }
}

impl gpui::Render for Toolbar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let vertical = self.orientation == Orientation::Vertical;
        let fits = self.fits();
        let mut items = std::mem::take(&mut self.items).into_iter().enumerate();
        let shown: Vec<AnyElement> = items
            .by_ref()
            .take(fits)
            .map(|(ix, item)| match item {
                Item::Button(b) => button(ix, b, cx),
                Item::Separator => separator(vertical),
                Item::Widget { element, .. } => element,
            })
            .collect();
        let rest: Vec<(usize, Item)> = items.collect();
        let overflow = (!rest.is_empty()).then(|| {
            let open = self.overflow_open;
            let rows: Vec<AnyElement> = rest
                .into_iter()
                .map(|(ix, item)| match item {
                    Item::Button(b) => overflow_row(ix, b, cx),
                    Item::Separator => div()
                        .my_1()
                        .h(px(1.0))
                        .bg(theme::SEPARATOR)
                        .into_any_element(),
                    Item::Widget { element, .. } => {
                        div().px_2().py_1().child(element).into_any_element()
                    }
                })
                .collect();
            div()
                .relative()
                .child(
                    div()
                        .id("tb-overflow")
                        .size(px(BUTTON))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_color(theme::TEXT_SUBTLE)
                        .bg(if open {
                            theme::CONTROL_HOVER
                        } else {
                            theme::CONTROL
                        })
                        .hover(|s| s.bg(theme::CONTROL_HOVER))
                        .tooltip(Tooltip::text("More"))
                        .on_click(cx.listener(|bar, _, _, cx| {
                            bar.overflow_open = !bar.overflow_open;
                            cx.notify();
                        }))
                        .child("»"),
                )
                .when(open, |d| {
                    d.child(deferred(
                        anchored().snap_to_window().child(
                            div()
                                .id("tb-overflow-list")
                                .occlude()
                                .on_mouse_down_out(cx.listener(|bar, _, _, cx| {
                                    bar.overflow_open = false;
                                    cx.notify();
                                }))
                                .min_w(px(200.0))
                                .py_1()
                                .rounded_md()
                                .bg(theme::POPOVER)
                                .border_1()
                                .border_color(theme::DIVIDER)
                                .shadow_lg()
                                .flex()
                                .flex_col()
                                .children(rows),
                        ),
                    ))
                })
        });
        let this = cx.entity().downgrade();
        let measure = canvas(
            move |bounds, _, cx| {
                let len = if vertical {
                    bounds.size.height
                } else {
                    bounds.size.width
                };
                let len = f32::from(len);
                this.update(cx, |bar, cx| {
                    if bar.extent.is_none_or(|e| (e - len).abs() > 0.5) {
                        bar.extent = Some(len);
                        cx.notify();
                    }
                })
                .ok();
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        div()
            .id("toolbar")
            .relative()
            .flex()
            .when(vertical, |d| d.flex_col().size_full().py(px(PAD)).px_1())
            .when(!vertical, |d| d.w_full().px(px(PAD)).py_1())
            .items_center()
            .gap(px(GAP))
            .bg(theme::PANEL)
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|_, e: &gpui::MouseDownEvent, _, cx| {
                    cx.emit(ToolbarEvent::Context {
                        item: None,
                        position: e.position,
                    });
                }),
            )
            .child(measure)
            .children(shown)
            .children(overflow)
    }
}

fn overflow_row(ix: usize, b: Button, cx: &mut Context<Toolbar>) -> AnyElement {
    let Button {
        key,
        icon,
        title,
        action,
        enabled,
        checked,
        ..
    } = b;
    let fg = if enabled {
        theme::TEXT_SOFT
    } else {
        theme::TEXT_DISABLED
    };
    div()
        .id(SharedString::from(format!("tb-more-{key}")))
        .px_3()
        .py_1()
        .flex()
        .items_center()
        .gap_2()
        .text_sm()
        .text_color(fg)
        .when(enabled, |d| {
            d.cursor_pointer()
                .hover(|s| s.bg(theme::HOVER))
                .on_click(cx.listener(move |bar, _, _, cx| bar.run(action.boxed_clone(), cx)))
        })
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |_, e: &gpui::MouseDownEvent, _, cx| {
                cx.stop_propagation();
                cx.emit(ToolbarEvent::Context {
                    item: Some(ix),
                    position: e.position,
                });
            }),
        )
        .child(svg().path(icon).size(px(16.0)).text_color(fg))
        .child(div().flex_1().child(title))
        .when(checked, |d| {
            d.child(div().text_color(theme::ACCENT).child("✓"))
        })
        .into_any_element()
}

/// Lengths are what overflow is decided on; keep them honest.
#[cfg(test)]
mod tests {
    use super::*;

    fn sep() -> Item {
        Item::Separator
    }

    fn widget(length: f32) -> Item {
        Item::Widget {
            key: "w".into(),
            title: "W".into(),
            length,
            element: div().into_any_element(),
        }
    }

    #[test]
    fn natural_length_adds_items_gaps_and_padding() {
        assert_eq!(natural(&[]), 2.0 * PAD);
        let items = [widget(100.0), sep(), widget(20.0)];
        assert_eq!(
            natural(&items),
            2.0 * PAD + 100.0 + (SEPARATOR - GAP) + 20.0 + 2.0 * GAP
        );
    }

    #[test]
    fn overflow_keeps_what_fits_and_never_ends_on_a_separator() {
        let mut bar = Toolbar::new(Orientation::Horizontal);
        bar.set_items(vec![widget(50.0), widget(50.0), sep(), widget(50.0)]);
        assert_eq!(bar.fits(), 4, "unmeasured: everything");
        bar.extent = Some(bar.natural_length());
        assert_eq!(bar.fits(), 4);
        // Room for two widgets and the overflow button, and the
        // separator that would follow them is dropped.
        bar.extent = Some(2.0 * PAD + 2.0 * (50.0 + GAP) + BUTTON + GAP + 3.0);
        assert_eq!(bar.fits(), 2);
        bar.extent = Some(10.0);
        assert_eq!(bar.fits(), 0);
    }
}
