//! Document > Split Document…: the open document (with its unsaved
//! edits) into several files, every N pages, at each top-level bookmark,
//! by file size, or by page ranges. Where the files go is asked for
//! afterwards. Tab moves, Left/Right change the method, Enter splits,
//! Escape cancels.

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, MouseButton, Subscription, Window,
    div, prelude::*, px,
};
use sheaf_doc::Snapshot;
use sheaf_ops::split::SplitBy;
use sheaf_ui::button::{Button, Chip};
use sheaf_ui::text_input::{TextInput, TextInputEvent};
use sheaf_ui::{dialog, theme};

use crate::{DialogCancel, DialogLeft, DialogNext, DialogPrev, DialogRight, DialogRun};

pub const KEY_CONTEXT: &str = "SplitDialog";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Method {
    EveryN,
    Bookmarks,
    Size,
    Ranges,
}

const METHODS: [(Method, &str); 4] = [
    (Method::EveryN, "Every N pages"),
    (Method::Bookmarks, "Top-level bookmarks"),
    (Method::Size, "File size"),
    (Method::Ranges, "Page ranges"),
];

pub enum SplitEvent {
    Split(SplitBy),
    Dismiss,
}

const ROW_METHOD: usize = 0;
const ROW_VALUE: usize = 1;

pub struct SplitDialog {
    focus: FocusHandle,
    snapshot: Snapshot,
    method: Method,
    every: Entity<TextInput>,
    size: Entity<TextInput>,
    ranges: Entity<TextInput>,
    row: usize,
    error: Option<String>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<SplitEvent> for SplitDialog {}

impl Focusable for SplitDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl SplitDialog {
    pub fn new(snapshot: Snapshot, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let every = cx.new(|cx| {
            let mut t = TextInput::new("pages per file", cx);
            t.set_text("10", cx);
            t
        });
        let size = cx.new(|cx| {
            let mut t = TextInput::new("e.g. 10 MB", cx);
            t.set_text("10 MB", cx);
            t
        });
        let ranges = cx.new(|cx| TextInput::new("e.g. 1-3; 4-10; 11-", cx));
        let subs = [&every, &size, &ranges]
            .into_iter()
            .map(|input| {
                cx.subscribe(input, |d, _, ev: &TextInputEvent, cx| match ev {
                    TextInputEvent::Confirm => d.split(cx),
                    TextInputEvent::Cancel => cx.emit(SplitEvent::Dismiss),
                })
            })
            .collect();
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let method = if snapshot.state.outline.is_empty() {
            Method::EveryN
        } else {
            Method::Bookmarks
        };
        Self {
            focus,
            snapshot,
            method,
            every,
            size,
            ranges,
            row: ROW_METHOD,
            error: None,
            _subs: subs,
        }
    }

    fn input(&self) -> Option<&Entity<TextInput>> {
        match self.method {
            Method::EveryN => Some(&self.every),
            Method::Size => Some(&self.size),
            Method::Ranges => Some(&self.ranges),
            Method::Bookmarks => None,
        }
    }

    fn set_row(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let row = if row == ROW_VALUE && self.input().is_none() {
            ROW_METHOD
        } else {
            row
        };
        self.row = row;
        let target = match (row, self.input()) {
            (ROW_VALUE, Some(i)) => i.read(cx).focus_handle(cx),
            _ => self.focus.clone(),
        };
        window.focus(&target, cx);
        cx.notify();
    }

    fn adjust(&mut self, by: i64, cx: &mut Context<Self>) {
        if self.row != ROW_METHOD {
            return;
        }
        let all: Vec<Method> = METHODS.iter().map(|m| m.0).collect();
        let i = all.iter().position(|m| *m == self.method).unwrap_or(0) as i64;
        self.method = all[(i + by).rem_euclid(all.len() as i64) as usize];
        self.error = None;
        cx.notify();
    }

    /// What the choices mean, or why they don't.
    fn split_by(&self, cx: &App) -> Result<SplitBy, String> {
        let text = |i: &Entity<TextInput>| i.read(cx).text().trim().to_string();
        match self.method {
            Method::EveryN => match text(&self.every).parse::<usize>() {
                Ok(n) if n > 0 => Ok(SplitBy::EveryN(n)),
                _ => Err("Pages per file must be a whole number above zero".into()),
            },
            Method::Bookmarks => Ok(SplitBy::TopBookmarks),
            Method::Size => sheaf_ops::split::parse_size(&text(&self.size)).map(SplitBy::MaxBytes),
            Method::Ranges => Ok(SplitBy::Ranges(
                text(&self.ranges)
                    .split(';')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect(),
            )),
        }
    }

    /// "Makes 4 files", when it can be known without writing them.
    fn preview(&self, cx: &App) -> Result<String, String> {
        let by = self.split_by(cx)?;
        if let SplitBy::MaxBytes(_) = by {
            return Ok("As many files as the size needs".into());
        }
        let parts = sheaf_ops::split::plan(&self.snapshot, &by, "x").map_err(|e| e.to_string())?;
        let n = parts.len();
        Ok(format!("Makes {n} file{}", if n == 1 { "" } else { "s" }))
    }

    fn split(&mut self, cx: &mut Context<Self>) {
        match self
            .split_by(cx)
            .and_then(|by| self.preview(cx).map(|_| by))
        {
            Ok(by) => cx.emit(SplitEvent::Split(by)),
            Err(e) => {
                self.error = Some(e);
                cx.notify();
            }
        }
    }
}

impl Render for SplitDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let methods = div()
            .flex()
            .flex_wrap()
            .gap_1()
            .children(METHODS.iter().enumerate().map(|(i, (m, label))| {
                let m = *m;
                Chip::new(("split-method", i), *label)
                    .selected(m == self.method)
                    .on_click(cx.listener(move |d, _, window, cx| {
                        d.method = m;
                        d.error = None;
                        d.set_row(ROW_METHOD, window, cx);
                    }))
            }))
            .into_any_element();
        let (value_label, note) = match self.method {
            Method::EveryN => ("Pages", "in each file; the last may have fewer"),
            Method::Bookmarks => ("", ""),
            Method::Size => ("At most", "per file (KB, MB or GB)"),
            Method::Ranges => ("Ranges", "one file per range, separated by ;"),
        };
        let value = self.input().map(|i| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().w(px(200.0)).child(i.clone()))
                .child(dialog::subtitle(note))
                .into_any_element()
        });
        let preview = self.preview(cx);
        let n = self.snapshot.state.pages.len();
        dialog::backdrop("split-backdrop")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|d, _: &DialogNext, window, cx| {
                let r = if d.row == ROW_METHOD {
                    ROW_VALUE
                } else {
                    ROW_METHOD
                };
                d.set_row(r, window, cx)
            }))
            .on_action(cx.listener(|d, _: &DialogPrev, window, cx| {
                let r = if d.row == ROW_METHOD {
                    ROW_VALUE
                } else {
                    ROW_METHOD
                };
                d.set_row(r, window, cx)
            }))
            .on_action(cx.listener(|d, _: &DialogLeft, _, cx| d.adjust(-1, cx)))
            .on_action(cx.listener(|d, _: &DialogRight, _, cx| d.adjust(1, cx)))
            .on_action(cx.listener(|d, _: &DialogRun, _, cx| d.split(cx)))
            .on_action(cx.listener(|_, _: &DialogCancel, _, cx| cx.emit(SplitEvent::Dismiss)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(SplitEvent::Dismiss)),
            )
            .child(
                dialog::card("split-dialog", px(620.0))
                    .child(
                        dialog::header("Split Document").child(dialog::subtitle(format!(
                            "{n} page{}, unsaved changes included",
                            if n == 1 { "" } else { "s" }
                        ))),
                    )
                    .child(
                        dialog::field_row(
                            "Split by".to_string(),
                            px(90.0),
                            self.row == ROW_METHOD,
                            methods,
                        )
                        .into_any_element(),
                    )
                    .when_some(value, |d, v| {
                        d.child(
                            dialog::field_row(
                                value_label.to_string(),
                                px(90.0),
                                self.row == ROW_VALUE,
                                v,
                            )
                            .into_any_element(),
                        )
                    })
                    .child(match (&self.error, preview) {
                        (Some(e), _) => dialog::message(e.clone(), true),
                        (None, Ok(p)) => dialog::message(p, false),
                        (None, Err(e)) => dialog::message(e, true),
                    })
                    .child(
                        dialog::footer()
                            .child(dialog::hint("Tab moves, ←/→ change, Enter splits"))
                            .child(
                                Button::new("split-cancel", "Cancel").on_click(
                                    cx.listener(|_, _, _, cx| cx.emit(SplitEvent::Dismiss)),
                                ),
                            )
                            .child(
                                Button::new("split-go", "Split…")
                                    .primary()
                                    .on_click(cx.listener(|d, _, _, cx| d.split(cx))),
                            ),
                    ),
            )
            .text_color(theme::TEXT_SOFT)
    }
}
