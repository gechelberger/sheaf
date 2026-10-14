//! The parts of a modal dialog: a dimmed backdrop, the card on it, and the
//! card's header, labeled rows, error line and footer. Each returns a `Div`
//! for the caller to add its own children, actions and listeners to.

use gpui::{
    AnyElement, ElementId, IntoElement, MouseButton, Pixels, SharedString, Stateful, div,
    prelude::*, px,
};

use crate::theme;

/// The dim layer over the window, with the card near its top. Add the
/// dialog's key context and actions here, and `on_mouse_down` to dismiss
/// on a click outside the card.
pub fn backdrop(id: impl Into<ElementId>) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .absolute()
        .inset_0()
        .bg(theme::BACKDROP)
        .flex()
        .justify_center()
        .items_start()
        .pt(px(80.0))
}

/// The dialog itself. Clicks inside it don't reach the backdrop.
pub fn card(id: impl Into<ElementId>, width: Pixels) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .w(width)
        .p_3()
        .flex()
        .flex_col()
        .gap_1()
        .bg(theme::CARD)
        .border_1()
        .border_color(theme::BORDER_CARD)
        .rounded_md()
        .shadow_lg()
        .text_color(theme::TEXT_SOFT)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

/// The title row, ruled off from the body. Add a `subtitle` after it.
pub fn header(title: impl Into<SharedString>) -> gpui::Div {
    div()
        .pb_2()
        .mb_1()
        .border_b_1()
        .border_color(theme::DIVIDER)
        .flex()
        .items_baseline()
        .gap_2()
        .child(
            div()
                .flex_shrink_0()
                .text_sm()
                .text_color(theme::TEXT_STRONG)
                .child(title.into()),
        )
}

/// Small faint text beside a title or under a field.
pub fn subtitle(text: impl IntoElement) -> gpui::Div {
    div().text_xs().text_color(theme::TEXT_FAINT).child(text)
}

/// A labeled row in a form; `focused` marks the row Tab is on.
pub fn field_row(
    label: impl Into<SharedString>,
    label_width: Pixels,
    focused: bool,
    content: AnyElement,
) -> gpui::Div {
    div()
        .flex()
        .items_start()
        .gap_3()
        .py_1()
        .px_2()
        .rounded_sm()
        .when(focused, |d| d.bg(theme::ROW_FOCUS))
        .child(
            div()
                .w(label_width)
                .flex_shrink_0()
                .pt_1()
                .text_sm()
                .text_color(theme::TEXT_MUTED)
                .child(label.into()),
        )
        .child(div().flex_1().min_w(px(0.0)).child(content))
}

/// A one-line message under the body: an error, or a success note.
pub fn message(text: impl Into<SharedString>, error: bool) -> gpui::Div {
    div()
        .px_2()
        .pt_1()
        .text_xs()
        .text_color(if error { theme::ERROR } else { theme::SUCCESS })
        .child(text.into())
}

/// The button row: add the buttons as children, main action last, with a
/// `hint` or `spacer` before those that go on the right.
pub fn footer() -> gpui::Div {
    div().pt_2().flex().items_center().gap_2()
}

/// Faint text (the keys, usually) filling the footer's left side.
pub fn hint(text: impl Into<SharedString>) -> gpui::Div {
    spacer()
        .text_xs()
        .text_color(theme::TEXT_FAINT)
        .child(text.into())
}

/// Empty space pushing what follows to the right of a row.
pub fn spacer() -> gpui::Div {
    div().flex_1()
}
