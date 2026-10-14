//! Document > Page Labels…: number a run of pages ("i, ii, iii", "A-1,
//! A-2"), as Acrobat's Page Labels dialog does; the pages after keep the
//! labels they had. Tab moves, Left/Right change, Enter applies, Escape
//! cancels.

use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, MouseButton,
    Subscription, Window, div, prelude::*, px,
};
use sheaf_doc::labels::{LabelRange, Style, format_label};
use sheaf_ui::button::{Button, Chip};
use sheaf_ui::text_input::{TextInput, TextInputEvent};
use sheaf_ui::{dialog, theme};

use crate::{DialogCancel, DialogLeft, DialogNext, DialogPrev, DialogRight, DialogRun};

pub const KEY_CONTEXT: &str = "LabelsDialog";

const STYLES: [(Style, &str); 6] = [
    (Style::Decimal, "1, 2, 3"),
    (Style::LowerRoman, "i, ii, iii"),
    (Style::UpperRoman, "I, II, III"),
    (Style::LowerLetters, "a, b, c"),
    (Style::UpperLetters, "A, B, C"),
    (Style::None, "None"),
];

pub enum LabelsEvent {
    /// Pages `from..=to` (0-based): style, prefix, first number.
    Apply {
        from: usize,
        to: usize,
        style: Style,
        prefix: String,
        first: u32,
    },
    RemoveAll,
    Dismiss,
}

const ROW_PAGES: usize = 0;
const ROW_STYLE: usize = 1;
const ROW_PREFIX: usize = 2;
const ROW_START: usize = 3;

pub struct LabelsDialog {
    focus: FocusHandle,
    page_count: usize,
    has_labels: bool,
    pages: Entity<TextInput>,
    prefix: Entity<TextInput>,
    start: Entity<TextInput>,
    style: Style,
    row: usize,
    error: Option<String>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<LabelsEvent> for LabelsDialog {}

impl Focusable for LabelsDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl LabelsDialog {
    /// `targets`: 0-based selected (or current) pages; the run from the
    /// first to the last is the default.
    pub fn new(
        page_count: usize,
        targets: &[usize],
        has_labels: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (a, b) = (
            targets.first().copied().unwrap_or(0),
            targets.last().copied().unwrap_or(0),
        );
        let range = if a == b {
            format!("{}", a + 1)
        } else {
            format!("{}-{}", a + 1, b + 1)
        };
        let input = |placeholder: &'static str, text: String, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut t = TextInput::new(placeholder, cx);
                t.set_text(text, cx);
                t
            })
        };
        let pages = input("e.g. 1-4", range, cx);
        let prefix = input("none", String::new(), cx);
        let start = input("1", "1".into(), cx);
        let mut subs: Vec<Subscription> = [&pages, &prefix, &start]
            .into_iter()
            .map(|i| {
                cx.subscribe(i, |d, _, ev: &TextInputEvent, cx| match ev {
                    TextInputEvent::Confirm => d.go(cx),
                    TextInputEvent::Cancel => cx.emit(LabelsEvent::Dismiss),
                })
            })
            .collect();
        for i in [&pages, &prefix, &start] {
            subs.push(cx.observe(i, |_, _, cx| cx.notify()));
        }
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            focus,
            page_count,
            has_labels,
            pages,
            prefix,
            start,
            style: Style::LowerRoman,
            row: ROW_STYLE,
            error: None,
            _subs: subs,
        }
    }

    fn input_for(&self, row: usize) -> Option<&Entity<TextInput>> {
        match row {
            ROW_PAGES => Some(&self.pages),
            ROW_PREFIX => Some(&self.prefix),
            ROW_START => Some(&self.start),
            _ => None,
        }
    }

    fn set_row(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.row = row;
        let target = match self.input_for(row) {
            Some(i) => i.read(cx).focus_handle(cx),
            None => self.focus.clone(),
        };
        window.focus(&target, cx);
        cx.notify();
    }

    fn step_row(&mut self, by: i64, window: &mut Window, cx: &mut Context<Self>) {
        let rows = [ROW_PAGES, ROW_STYLE, ROW_PREFIX, ROW_START];
        let at = rows.iter().position(|&r| r == self.row).unwrap_or(0) as i64;
        let next = rows[(at + by).rem_euclid(rows.len() as i64) as usize];
        self.set_row(next, window, cx);
    }

    fn adjust(&mut self, by: i64, cx: &mut Context<Self>) {
        if self.row == ROW_STYLE {
            let all: Vec<Style> = STYLES.iter().map(|s| s.0).collect();
            let i = all.iter().position(|s| *s == self.style).unwrap_or(0) as i64;
            self.style = all[(i + by).rem_euclid(all.len() as i64) as usize];
            self.error = None;
            cx.notify();
        }
    }

    /// The run, prefix and first number, or what's wrong.
    fn plan(&self, cx: &App) -> Result<(usize, usize, String, u32), String> {
        let idx = sheaf_ops::pages::parse(self.pages.read(cx).text(), self.page_count)
            .map_err(|e| format!("Pages: {e}"))?;
        if idx.windows(2).any(|w| w[1] != w[0] + 1) {
            return Err("Pages must be one run, like 1-4".into());
        }
        let first: u32 = match self.start.read(cx).text().trim() {
            "" => 1,
            t => t
                .parse()
                .ok()
                .filter(|n| *n >= 1)
                .ok_or("Start must be a whole number from 1")?,
        };
        Ok((
            idx[0],
            *idx.last().unwrap(),
            self.prefix.read(cx).text().to_string(),
            first,
        ))
    }

    fn go(&mut self, cx: &mut Context<Self>) {
        match self.plan(cx) {
            Ok((from, to, prefix, first)) => cx.emit(LabelsEvent::Apply {
                from,
                to,
                style: self.style,
                prefix,
                first,
            }),
            Err(e) => {
                self.error = Some(e);
                cx.notify();
            }
        }
    }

    fn row_el(&self, index: usize, label: &str, content: AnyElement) -> AnyElement {
        dialog::field_row(label.to_string(), px(90.0), self.row == index, content)
            .into_any_element()
    }
}

impl Render for LabelsDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let styles = div()
            .flex()
            .flex_wrap()
            .gap_1()
            .children(STYLES.iter().enumerate().map(|(i, (s, label))| {
                let s = *s;
                Chip::new(("label-style", i), *label)
                    .selected(s == self.style)
                    .on_click(cx.listener(move |d, _, window, cx| {
                        d.style = s;
                        d.error = None;
                        d.set_row(ROW_STYLE, window, cx);
                    }))
            }))
            .into_any_element();
        let field = |input: &Entity<TextInput>, w: f32, note: &str| -> AnyElement {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().w(px(w)).child(input.clone()))
                .child(dialog::subtitle(note.to_string()))
                .into_any_element()
        };
        let preview = self.plan(cx).map(|(from, to, prefix, first)| {
            let r = LabelRange {
                start: from as u32,
                style: self.style,
                prefix,
                first,
            };
            let n = (to - from + 1).min(4);
            let mut shown: Vec<String> = (0..n as u32).map(|k| format_label(&r, k)).collect();
            if to - from + 1 > n {
                shown.push("…".into());
            }
            let what = if from == to {
                format!("Page {} is labelled", from + 1)
            } else {
                format!("Pages {}–{} are labelled", from + 1, to + 1)
            };
            format!("{what} {}", shown.join(", "))
        });
        let preview = preview.unwrap_or_else(|e| e);
        dialog::backdrop("labels-backdrop")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|d, _: &DialogNext, window, cx| d.step_row(1, window, cx)))
            .on_action(cx.listener(|d, _: &DialogPrev, window, cx| d.step_row(-1, window, cx)))
            .on_action(cx.listener(|d, _: &DialogLeft, _, cx| d.adjust(-1, cx)))
            .on_action(cx.listener(|d, _: &DialogRight, _, cx| d.adjust(1, cx)))
            .on_action(cx.listener(|d, _: &DialogRun, _, cx| d.go(cx)))
            .on_action(cx.listener(|_, _: &DialogCancel, _, cx| cx.emit(LabelsEvent::Dismiss)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(LabelsEvent::Dismiss)),
            )
            .child(
                dialog::card("labels-dialog", px(620.0))
                    .child(
                        dialog::header("Page Labels")
                            .child(dialog::subtitle("pages after the run keep their labels")),
                    )
                    .child(self.row_el(
                        ROW_PAGES,
                        "Pages",
                        field(&self.pages, 140.0, &format!("of {}", self.page_count)),
                    ))
                    .child(self.row_el(ROW_STYLE, "Style", styles))
                    .child(self.row_el(
                        ROW_PREFIX,
                        "Prefix",
                        field(&self.prefix, 140.0, "e.g. A- or Exhibit "),
                    ))
                    .child(self.row_el(ROW_START, "Start at", field(&self.start, 80.0, "")))
                    .child(match &self.error {
                        Some(e) => dialog::message(e.clone(), true),
                        None => dialog::message(preview, false),
                    })
                    .child(
                        dialog::footer()
                            .child(dialog::hint("Tab moves, ←/→ change, Enter applies"))
                            .when(self.has_labels, |d| {
                                d.child(Button::new("labels-clear", "Remove All Labels").on_click(
                                    cx.listener(|_, _, _, cx| cx.emit(LabelsEvent::RemoveAll)),
                                ))
                            })
                            .child(
                                Button::new("labels-cancel", "Cancel").on_click(
                                    cx.listener(|_, _, _, cx| cx.emit(LabelsEvent::Dismiss)),
                                ),
                            )
                            .child(
                                Button::new("labels-go", "Apply")
                                    .primary()
                                    .on_click(cx.listener(|d, _, _, cx| d.go(cx))),
                            ),
                    ),
            )
            .text_color(theme::TEXT_SOFT)
    }
}
