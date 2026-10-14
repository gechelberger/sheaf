//! Hover tooltip for toolbar buttons: a title, an optional shortcut, and a
//! line of description.

use gpui::{Action, AnyView, App, Context, IntoElement, SharedString, Window, div, prelude::*, px};
use sheaf_ui::theme;

pub struct Tooltip {
    title: SharedString,
    shortcut: Option<SharedString>,
    description: Option<SharedString>,
}

impl Tooltip {
    /// A builder for `.tooltip(...)`. The shortcut is looked up from the live
    /// keymap when the tooltip opens, so user rebinds show up.
    pub fn for_action(
        title: impl Into<SharedString>,
        description: impl Into<SharedString>,
        action: Box<dyn Action>,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        let title = title.into();
        let description = description.into();
        move |_, cx| {
            let shortcut = crate::menu_bar::shortcut(action.as_ref(), cx);
            cx.new(|_| Tooltip {
                title: title.clone(),
                shortcut,
                description: Some(description.clone()),
            })
            .into()
        }
    }

    /// A builder for a tooltip with just a title.
    pub fn text(title: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView {
        let title = title.into();
        move |_, cx| {
            cx.new(|_| Tooltip {
                title: title.clone(),
                shortcut: None,
                description: None,
            })
            .into()
        }
    }
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // Offset from the cursor so the pointer doesn't cover the text.
        div().pl(px(8.0)).pt(px(16.0)).child(
            div()
                .flex()
                .flex_col()
                .gap_0p5()
                .max_w(px(280.0))
                .px_2()
                .py_1()
                .rounded_md()
                .bg(theme::RAISED)
                .border_1()
                .border_color(theme::BORDER_CARD)
                .shadow_md()
                .text_xs()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            div()
                                .text_color(theme::TEXT_STRONG)
                                .child(self.title.clone()),
                        )
                        .children(self.shortcut.clone().map(|s| {
                            div()
                                .px_1()
                                .rounded_sm()
                                .bg(theme::CONTROL)
                                .text_color(theme::TEXT_DETAIL)
                                .child(s)
                        })),
                )
                .children(
                    self.description
                        .clone()
                        .map(|d| div().text_color(theme::TEXT_MUTED).child(d)),
                ),
        )
    }
}
