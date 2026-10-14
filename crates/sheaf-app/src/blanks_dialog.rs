//! Document > Find Blank Pages…: which pages to look at, how strict to be,
//! and whether pages saying "intentionally left blank" stay. Tab moves,
//! Left/Right change, Enter searches, Escape cancels.

use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, MouseButton,
    Subscription, Window, div, prelude::*, px,
};
use sheaf_image::blank::Sensitivity;
use sheaf_ops::blank::BlankParams;
use sheaf_ui::button::{Button, Chip};
use sheaf_ui::text_input::{TextInput, TextInputEvent};
use sheaf_ui::{dialog, theme};

use crate::{DialogCancel, DialogLeft, DialogNext, DialogPrev, DialogRight, DialogRun};

pub const KEY_CONTEXT: &str = "BlanksDialog";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pages {
    All,
    Selected,
    Range,
}

const SENSITIVITIES: [(Sensitivity, &str); 3] = [
    (Sensitivity::Strict, "Strict: only empty pages"),
    (Sensitivity::Normal, "Normal"),
    (Sensitivity::Lenient, "Lenient: noisy scans"),
];

pub enum BlanksEvent {
    /// 0-based pages to look at (none: all).
    Find(Option<Vec<usize>>, BlankParams),
    Dismiss,
}

const ROW_PAGES: usize = 0;
const ROW_RANGE: usize = 1;
const ROW_SENSITIVITY: usize = 2;
const ROW_KEEP: usize = 3;

pub struct BlanksDialog {
    focus: FocusHandle,
    page_count: usize,
    selected: Vec<usize>,
    pages: Pages,
    range: Entity<TextInput>,
    params: BlankParams,
    row: usize,
    error: Option<String>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<BlanksEvent> for BlanksDialog {}

impl Focusable for BlanksDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl BlanksDialog {
    pub fn new(
        page_count: usize,
        selected: Vec<usize>,
        params: BlankParams,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let range = cx.new(|cx| TextInput::new("e.g. 1-3, 7, 10-", cx));
        let subs = vec![
            cx.subscribe(&range, |d, _, ev: &TextInputEvent, cx| match ev {
                TextInputEvent::Confirm => {
                    d.pages = Pages::Range;
                    d.go(cx)
                }
                TextInputEvent::Cancel => cx.emit(BlanksEvent::Dismiss),
            }),
        ];
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            focus,
            page_count,
            pages: if selected.len() > 1 {
                Pages::Selected
            } else {
                Pages::All
            },
            selected,
            range,
            params,
            row: ROW_PAGES,
            error: None,
            _subs: subs,
        }
    }

    fn set_row(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.row = row;
        let target = if row == ROW_RANGE {
            self.range.read(cx).focus_handle(cx)
        } else {
            self.focus.clone()
        };
        window.focus(&target, cx);
        cx.notify();
    }

    fn step_row(&mut self, by: i64, window: &mut Window, cx: &mut Context<Self>) {
        let rows = [ROW_PAGES, ROW_RANGE, ROW_SENSITIVITY, ROW_KEEP];
        let at = rows.iter().position(|&r| r == self.row).unwrap_or(0) as i64;
        self.set_row(rows[(at + by).rem_euclid(4) as usize], window, cx);
    }

    fn adjust(&mut self, by: i64, cx: &mut Context<Self>) {
        fn cycle<T: Copy + PartialEq>(items: &[T], current: T, by: i64) -> T {
            let i = items.iter().position(|x| *x == current).unwrap_or(0) as i64;
            items[(i + by).rem_euclid(items.len() as i64) as usize]
        }
        match self.row {
            ROW_PAGES => {
                let opts: Vec<Pages> = [Pages::All, Pages::Selected, Pages::Range]
                    .into_iter()
                    .filter(|p| *p != Pages::Selected || self.selected.len() > 1)
                    .collect();
                self.pages = cycle(&opts, self.pages, by);
            }
            ROW_SENSITIVITY => {
                let all: Vec<Sensitivity> = SENSITIVITIES.iter().map(|s| s.0).collect();
                self.params.sensitivity = cycle(&all, self.params.sensitivity, by);
            }
            ROW_KEEP => self.params.keep_intentional = !self.params.keep_intentional,
            _ => {}
        }
        self.error = None;
        cx.notify();
    }

    fn go(&mut self, cx: &mut Context<Self>) {
        let pages = match self.pages {
            Pages::All => Ok(None),
            Pages::Selected => Ok(Some(self.selected.clone())),
            Pages::Range => {
                sheaf_ops::pages::parse(self.range.read(cx).text(), self.page_count).map(Some)
            }
        };
        match pages {
            Ok(p) => cx.emit(BlanksEvent::Find(p, self.params)),
            Err(e) => {
                self.error = Some(e);
                cx.notify();
            }
        }
    }

    fn row_el(&self, index: usize, label: &str, content: AnyElement) -> AnyElement {
        dialog::field_row(label.to_string(), px(100.0), self.row == index, content)
            .into_any_element()
    }
}

impl Render for BlanksDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let n = self.page_count;
        let pages = div()
            .flex()
            .flex_wrap()
            .gap_1()
            .children(
                [
                    (Pages::All, format!("All ({n})"), true),
                    (
                        Pages::Selected,
                        format!("Selected ({})", self.selected.len()),
                        self.selected.len() > 1,
                    ),
                    (Pages::Range, "Pages…".into(), true),
                ]
                .into_iter()
                .enumerate()
                .map(|(i, (p, label, enabled))| {
                    Chip::new(("blanks-pages", i), label)
                        .selected(p == self.pages)
                        .disabled(!enabled)
                        .on_click(cx.listener(move |d, _, window, cx| {
                            if enabled {
                                d.pages = p;
                                d.set_row(ROW_PAGES, window, cx);
                            }
                        }))
                }),
            )
            .into_any_element();
        let sens = div()
            .flex()
            .flex_wrap()
            .gap_1()
            .children(SENSITIVITIES.iter().enumerate().map(|(i, (s, label))| {
                let s = *s;
                Chip::new(("blanks-sens", i), *label)
                    .selected(s == self.params.sensitivity)
                    .on_click(cx.listener(move |d, _, window, cx| {
                        d.params.sensitivity = s;
                        d.set_row(ROW_SENSITIVITY, window, cx);
                    }))
            }))
            .into_any_element();
        let keep = Chip::new(
            ("blanks-keep", 0usize),
            format!(
                "{} Keep pages that say “intentionally left blank”",
                if self.params.keep_intentional {
                    "☑"
                } else {
                    "☐"
                }
            ),
        )
        .on_click(cx.listener(|d, _, window, cx| {
            d.params.keep_intentional = !d.params.keep_intentional;
            d.set_row(ROW_KEEP, window, cx);
        }))
        .into_any_element();
        dialog::backdrop("blanks-backdrop")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|d, _: &DialogNext, window, cx| d.step_row(1, window, cx)))
            .on_action(cx.listener(|d, _: &DialogPrev, window, cx| d.step_row(-1, window, cx)))
            .on_action(cx.listener(|d, _: &DialogLeft, _, cx| d.adjust(-1, cx)))
            .on_action(cx.listener(|d, _: &DialogRight, _, cx| d.adjust(1, cx)))
            .on_action(cx.listener(|d, _: &DialogRun, _, cx| d.go(cx)))
            .on_action(cx.listener(|_, _: &DialogCancel, _, cx| cx.emit(BlanksEvent::Dismiss)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(BlanksEvent::Dismiss)),
            )
            .child(
                dialog::card("blanks-dialog", px(600.0))
                    .child(dialog::header("Find Blank Pages").child(dialog::subtitle(
                        "you review what's found before anything is deleted",
                    )))
                    .child(self.row_el(ROW_PAGES, "Look at", pages))
                    .child(
                        self.row_el(
                            ROW_RANGE,
                            "",
                            div()
                                .w(px(200.0))
                                .child(self.range.clone())
                                .into_any_element(),
                        ),
                    )
                    .child(self.row_el(ROW_SENSITIVITY, "Sensitivity", sens))
                    .child(self.row_el(ROW_KEEP, "", keep))
                    .when_some(self.error.clone(), |d, e| d.child(dialog::message(e, true)))
                    .child(
                        dialog::footer()
                            .child(dialog::hint("Tab moves, ←/→ change, Enter finds"))
                            .child(
                                Button::new("blanks-cancel", "Cancel").on_click(
                                    cx.listener(|_, _, _, cx| cx.emit(BlanksEvent::Dismiss)),
                                ),
                            )
                            .child(
                                Button::new("blanks-go", "Find")
                                    .primary()
                                    .on_click(cx.listener(|d, _, _, cx| d.go(cx))),
                            ),
                    ),
            )
            .text_color(theme::TEXT_SOFT)
    }
}
