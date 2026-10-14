//! Page labels on screen, and View > Page Navigation > Go to Page (Ctrl+Shift+N), which
//! takes a label ("iv", "A-3") or a page number.

use gpui::{AnyElement, Context, Entity, Focusable, Subscription, Window, div, prelude::*, px};
use sheaf_ui::text_input::{TextInput, TextInputEvent};
use sheaf_ui::theme;

use crate::docview::{Doc, DocView, ScrollTo, State};

/// The open Go to Page box.
pub(crate) struct GoTo {
    input: Entity<TextInput>,
    _sub: Subscription,
    error: bool,
}

impl Doc {
    /// Every page's label when the document has page labels, worked out
    /// once per version.
    pub(crate) fn labels(&self) -> Option<std::sync::Arc<Vec<String>>> {
        let mut cache = self.labels.borrow_mut();
        if cache.0 != self.version {
            *cache = (
                self.version,
                self.model.state().page_labels().map(std::sync::Arc::new),
            );
        }
        cache.1.clone()
    }

    /// Page `index`'s label, or its position when there are no labels.
    pub(crate) fn page_label(&self, index: usize) -> String {
        self.labels()
            .and_then(|l| l.get(index).cloned())
            .unwrap_or_else(|| (index + 1).to_string())
    }

    /// The page a typed label or number names: an exact label first
    /// (ignoring case), then a page number.
    pub(crate) fn find_page(&self, text: &str) -> Option<usize> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        let n = self.model.page_count();
        if self.labels().is_some()
            && let Some(i) = (0..n).find(|&i| self.page_label(i).eq_ignore_ascii_case(text))
        {
            return Some(i);
        }
        text.parse::<usize>()
            .ok()
            .filter(|&p| p >= 1 && p <= n)
            .map(|p| p - 1)
    }

    /// "iv (4 of 120)" with labels, "4 / 120" without.
    pub(crate) fn page_position(&self, index: usize, count: usize) -> String {
        let label = self.page_label(index);
        let number = (index + 1).min(count);
        if label == number.to_string() {
            format!("{number} / {count}")
        } else {
            format!("{label} ({number} of {count})")
        }
    }
}

impl DocView {
    pub(crate) fn open_goto(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.state, State::Ready(_)) {
            return;
        }
        let input = cx.new(|cx| TextInput::new("Page number or label", cx));
        let sub = cx.subscribe(&input, |this, input, ev: &TextInputEvent, cx| match ev {
            TextInputEvent::Confirm => {
                let text = input.read(cx).text().to_string();
                this.goto_confirm(&text, cx);
            }
            TextInputEvent::Cancel => this.close_goto(cx),
        });
        window.focus(&input.read(cx).focus_handle(cx), cx);
        self.goto = Some(GoTo {
            input,
            _sub: sub,
            error: false,
        });
        cx.notify();
    }

    fn goto_confirm(&mut self, text: &str, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        match doc.find_page(text) {
            Some(i) => {
                doc.scroll_to(ScrollTo::Page(i));
                self.close_goto(cx);
            }
            None => {
                if let Some(g) = &mut self.goto {
                    g.error = true;
                }
                cx.notify();
            }
        }
    }

    fn close_goto(&mut self, cx: &mut Context<Self>) {
        self.goto = None;
        self.pending_focus = true;
        cx.notify();
    }

    pub(crate) fn render_goto(&self) -> Option<AnyElement> {
        let g = self.goto.as_ref()?;
        Some(
            div()
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .bg(theme::PANEL)
                .border_b_1()
                .border_color(theme::DIVIDER)
                .child(div().text_sm().child("Go to page"))
                .child(div().w(px(200.0)).child(g.input.clone()))
                .child(
                    div()
                        .text_sm()
                        .text_color(if g.error {
                            theme::ERROR
                        } else {
                            theme::TEXT_MUTED
                        })
                        .child(if g.error {
                            "No page has that number or label"
                        } else {
                            "Enter to go, Esc to cancel"
                        }),
                )
                .into_any_element(),
        )
    }
}
