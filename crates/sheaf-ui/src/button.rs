//! Buttons and chips, the clickable controls dialogs and panels share.

use gpui::{App, ClickEvent, ElementId, SharedString, Window, div, prelude::*};

use crate::theme;

type OnClick = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// A push button: secondary (gray) unless made `primary` (accent).
#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: SharedString,
    primary: bool,
    focused: bool,
    compact: bool,
    on_click: Option<OnClick>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            primary: false,
            focused: false,
            compact: false,
            on_click: None,
        }
    }

    /// The dialog's main action, drawn in the accent color.
    pub fn primary(mut self) -> Self {
        self.primary = true;
        self
    }

    /// Draw the keyboard-focus ring (dialogs that move focus with Tab).
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Smaller text and padding, for buttons inside a list row.
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(f));
        self
    }
}

impl RenderOnce for Button {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let (bg, hover, border) = if self.primary {
            (theme::ACCENT, theme::ACCENT_HOVER, theme::ACCENT)
        } else {
            (theme::CONTROL, theme::CONTROL_HOVER, theme::BORDER)
        };
        div()
            .id(self.id)
            .flex_shrink_0()
            .rounded_sm()
            .cursor_pointer()
            .map(|d| {
                if self.compact {
                    d.px_2().py_0p5().text_xs()
                } else {
                    d.px_3().py_1().text_sm()
                }
            })
            .bg(bg)
            .when(self.primary, |d| d.text_color(theme::TEXT_BRIGHT))
            .border_1()
            .border_color(if self.focused {
                theme::FOCUS_RING
            } else {
                border
            })
            .hover(move |s| s.bg(hover))
            .when_some(self.on_click, |d, f| {
                d.on_click(move |e, window, cx| {
                    f(e, window, cx);
                    cx.stop_propagation();
                })
            })
            .child(self.label)
    }
}

/// One option of a choice: outlined, filled when selected.
#[derive(IntoElement)]
pub struct Chip {
    id: ElementId,
    label: SharedString,
    selected: bool,
    disabled: bool,
    on_click: Option<OnClick>,
}

impl Chip {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            selected: false,
            disabled: false,
            on_click: None,
        }
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Grayed out and not clickable.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(f));
        self
    }
}

impl RenderOnce for Chip {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .id(self.id)
            .px_2()
            .py_1()
            .rounded_sm()
            .text_sm()
            .border_1()
            .border_color(if self.selected {
                theme::ACCENT
            } else {
                theme::BORDER
            })
            .when(self.selected, |d| d.bg(theme::SELECTED))
            .text_color(if self.disabled {
                theme::TEXT_DISABLED
            } else {
                theme::TEXT_STRONG
            })
            .when(!self.disabled, |d| {
                d.cursor_pointer()
                    .hover(|s| s.bg(theme::HOVER))
                    .when_some(self.on_click, |d, f| d.on_click(f))
            })
            .child(self.label)
    }
}
