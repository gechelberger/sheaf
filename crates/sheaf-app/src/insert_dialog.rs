//! Document > Insert Pages > Blank Pages… / From Images…, and Replace
//! Pages…. Tab moves between rows, Left/Right change, Enter goes, Escape
//! cancels.

use std::path::PathBuf;

use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, MouseButton,
    Subscription, Window, div, prelude::*, px,
};
use sheaf_ui::button::{Button, Chip};
use sheaf_ui::text_input::{TextInput, TextInputEvent};
use sheaf_ui::{dialog, theme};

use crate::page_insert::{A4, LEGAL, LETTER, Size, Where};
use crate::{DialogCancel, DialogLeft, DialogNext, DialogPrev, DialogRight, DialogRun};

pub const KEY_CONTEXT: &str = "InsertDialog";

/// What's being inserted.
pub enum Insert {
    Blank,
    Images(Vec<PathBuf>),
}

pub enum InsertEvent {
    Blank {
        count: usize,
        size: Size,
        at: Where,
    },
    Images {
        files: Vec<PathBuf>,
        /// None: each image's own size.
        size: Option<Size>,
        at: Where,
    },
    Replace {
        /// 0-based indexes of this document's pages to replace.
        pages: Vec<usize>,
        file: PathBuf,
        /// 0-based, contiguous pages of `file`.
        from: std::ops::Range<u32>,
    },
    Dismiss,
}

const WHERE: [(Where, &str); 4] = [
    (Where::AfterCurrent, "After current page"),
    (Where::BeforeCurrent, "Before current page"),
    (Where::Start, "At the start"),
    (Where::End, "At the end"),
];

/// Size choices; None is "each image's own size".
fn sizes(images: bool) -> Vec<(Option<Size>, &'static str)> {
    let mut v = vec![
        (Some(Size::Current), "Same as current page"),
        (Some(Size::Sheet(LETTER)), "Letter"),
        (Some(Size::Sheet(A4)), "A4"),
        (Some(Size::Sheet(LEGAL)), "Legal"),
    ];
    if images {
        v.push((None, "Image's own size"));
    }
    v
}

const ROW_SIZE: usize = 0;
const ROW_COUNT: usize = 1;
const ROW_WHERE: usize = 2;

pub struct InsertDialog {
    focus: FocusHandle,
    kind: Insert,
    size: Option<Size>,
    at: Where,
    count: Entity<TextInput>,
    row: usize,
    error: Option<String>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<InsertEvent> for InsertDialog {}

impl Focusable for InsertDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl InsertDialog {
    pub fn new(kind: Insert, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let count = cx.new(|cx| {
            let mut t = TextInput::new("1", cx);
            t.set_text("1", cx);
            t
        });
        let subs = vec![
            cx.subscribe(&count, |d, _, ev: &TextInputEvent, cx| match ev {
                TextInputEvent::Confirm => d.go(cx),
                TextInputEvent::Cancel => cx.emit(InsertEvent::Dismiss),
            }),
        ];
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            focus,
            kind,
            size: Some(Size::Current),
            at: Where::AfterCurrent,
            count,
            row: ROW_SIZE,
            error: None,
            _subs: subs,
        }
    }

    fn images(&self) -> bool {
        matches!(self.kind, Insert::Images(_))
    }

    fn rows(&self) -> Vec<usize> {
        if self.images() {
            vec![ROW_SIZE, ROW_WHERE]
        } else {
            vec![ROW_SIZE, ROW_COUNT, ROW_WHERE]
        }
    }

    fn set_row(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.row = row;
        let target = if row == ROW_COUNT {
            self.count.read(cx).focus_handle(cx)
        } else {
            self.focus.clone()
        };
        window.focus(&target, cx);
        cx.notify();
    }

    fn step_row(&mut self, by: i64, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.rows();
        let at = rows.iter().position(|&r| r == self.row).unwrap_or(0) as i64;
        let next = rows[(at + by).rem_euclid(rows.len() as i64) as usize];
        self.set_row(next, window, cx);
    }

    fn adjust(&mut self, by: i64, cx: &mut Context<Self>) {
        fn cycle<T: Copy + PartialEq>(items: &[T], current: T, by: i64) -> T {
            let i = items.iter().position(|x| *x == current).unwrap_or(0) as i64;
            items[(i + by).rem_euclid(items.len() as i64) as usize]
        }
        match self.row {
            ROW_SIZE => {
                let all: Vec<Option<Size>> = sizes(self.images()).iter().map(|s| s.0).collect();
                self.size = cycle(&all, self.size, by);
            }
            ROW_WHERE => {
                let all: Vec<Where> = WHERE.iter().map(|w| w.0).collect();
                self.at = cycle(&all, self.at, by);
            }
            _ => {}
        }
        cx.notify();
    }

    fn go(&mut self, cx: &mut Context<Self>) {
        match &self.kind {
            Insert::Blank => match self.count.read(cx).text().trim().parse::<usize>() {
                Ok(n) if (1..=500).contains(&n) => cx.emit(InsertEvent::Blank {
                    count: n,
                    size: self.size.unwrap_or(Size::Current),
                    at: self.at,
                }),
                _ => {
                    self.error = Some("How many pages: 1 to 500".into());
                    cx.notify();
                }
            },
            Insert::Images(files) => cx.emit(InsertEvent::Images {
                files: files.clone(),
                size: self.size,
                at: self.at,
            }),
        }
    }

    fn row_el(&self, index: usize, label: &str, content: AnyElement) -> AnyElement {
        dialog::field_row(label.to_string(), px(90.0), self.row == index, content)
            .into_any_element()
    }
}

impl Render for InsertDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let size_chips = div()
            .flex()
            .flex_wrap()
            .gap_1()
            .children(
                sizes(self.images())
                    .into_iter()
                    .enumerate()
                    .map(|(i, (s, label))| {
                        Chip::new(("insert-size", i), label)
                            .selected(s == self.size)
                            .on_click(cx.listener(move |d, _, window, cx| {
                                d.size = s;
                                d.set_row(ROW_SIZE, window, cx);
                            }))
                    }),
            )
            .into_any_element();
        let where_chips = div()
            .flex()
            .flex_wrap()
            .gap_1()
            .children(WHERE.iter().enumerate().map(|(i, (w, label))| {
                let w = *w;
                Chip::new(("insert-where", i), *label)
                    .selected(w == self.at)
                    .on_click(cx.listener(move |d, _, window, cx| {
                        d.at = w;
                        d.set_row(ROW_WHERE, window, cx);
                    }))
            }))
            .into_any_element();
        let (title, subtitle) = match &self.kind {
            Insert::Blank => ("Insert Blank Pages", String::new()),
            Insert::Images(f) => (
                "Insert Pages from Images",
                format!("{} image{}", f.len(), if f.len() == 1 { "" } else { "s" }),
            ),
        };
        dialog::backdrop("insert-backdrop")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|d, _: &DialogNext, window, cx| d.step_row(1, window, cx)))
            .on_action(cx.listener(|d, _: &DialogPrev, window, cx| d.step_row(-1, window, cx)))
            .on_action(cx.listener(|d, _: &DialogLeft, _, cx| d.adjust(-1, cx)))
            .on_action(cx.listener(|d, _: &DialogRight, _, cx| d.adjust(1, cx)))
            .on_action(cx.listener(|d, _: &DialogRun, _, cx| d.go(cx)))
            .on_action(cx.listener(|_, _: &DialogCancel, _, cx| cx.emit(InsertEvent::Dismiss)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(InsertEvent::Dismiss)),
            )
            .child(
                dialog::card("insert-dialog", px(600.0))
                    .child(dialog::header(title).child(dialog::subtitle(subtitle)))
                    .child(self.row_el(
                        ROW_SIZE,
                        if self.images() { "Fit to" } else { "Size" },
                        size_chips,
                    ))
                    .when(!self.images(), |d| {
                        d.child(
                            self.row_el(
                                ROW_COUNT,
                                "How many",
                                div()
                                    .w(px(80.0))
                                    .child(self.count.clone())
                                    .into_any_element(),
                            ),
                        )
                    })
                    .child(self.row_el(ROW_WHERE, "Where", where_chips))
                    .when_some(self.error.clone(), |d, e| d.child(dialog::message(e, true)))
                    .child(
                        dialog::footer()
                            .child(dialog::hint("Tab moves, ←/→ change, Enter inserts"))
                            .child(
                                Button::new("insert-cancel", "Cancel").on_click(
                                    cx.listener(|_, _, _, cx| cx.emit(InsertEvent::Dismiss)),
                                ),
                            )
                            .child(
                                Button::new("insert-go", "Insert")
                                    .primary()
                                    .on_click(cx.listener(|d, _, _, cx| d.go(cx))),
                            ),
                    ),
            )
            .text_color(theme::TEXT_SOFT)
    }
}

/// Replace Pages: which of this document's pages, with which of a file's.
pub struct ReplaceDialog {
    focus: FocusHandle,
    file: PathBuf,
    file_pages: usize,
    doc_pages: usize,
    these: Entity<TextInput>,
    with: Entity<TextInput>,
    error: Option<String>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<InsertEvent> for ReplaceDialog {}

impl Focusable for ReplaceDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ReplaceDialog {
    /// `targets`: 0-based indexes of the selected (or current) pages.
    pub fn new(
        file: PathBuf,
        file_pages: usize,
        doc_pages: usize,
        targets: &[usize],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let default_these = ranges_text(targets);
        let n = targets.len().max(1).min(file_pages);
        let these = cx.new(|cx| {
            let mut t = TextInput::new("e.g. 3-5", cx);
            t.set_text(default_these, cx);
            t
        });
        let with = cx.new(|cx| {
            let mut t = TextInput::new("e.g. 1-3", cx);
            t.set_text(if n == 1 { "1".into() } else { format!("1-{n}") }, cx);
            t
        });
        let subs = [&these, &with]
            .into_iter()
            .map(|i| {
                cx.subscribe(i, |d, _, ev: &TextInputEvent, cx| match ev {
                    TextInputEvent::Confirm => d.go(cx),
                    TextInputEvent::Cancel => cx.emit(InsertEvent::Dismiss),
                })
            })
            .collect();
        let focus = these.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        Self {
            focus: cx.focus_handle(),
            file,
            file_pages,
            doc_pages,
            these,
            with,
            error: None,
            _subs: subs,
        }
    }

    fn plan(&self, cx: &App) -> Result<(Vec<usize>, std::ops::Range<u32>), String> {
        let pages = sheaf_ops::pages::parse(self.these.read(cx).text(), self.doc_pages)
            .map_err(|e| format!("Pages to replace: {e}"))?;
        let from = sheaf_ops::pages::parse(self.with.read(cx).text(), self.file_pages)
            .map_err(|e| format!("Pages from the file: {e}"))?;
        if from.windows(2).any(|w| w[1] != w[0] + 1) {
            return Err("Pages from the file must be one run, like 2-6".into());
        }
        Ok((pages, from[0] as u32..*from.last().unwrap() as u32 + 1))
    }

    fn go(&mut self, cx: &mut Context<Self>) {
        match self.plan(cx) {
            Ok((pages, from)) => cx.emit(InsertEvent::Replace {
                pages,
                file: self.file.clone(),
                from,
            }),
            Err(e) => {
                self.error = Some(e);
                cx.notify();
            }
        }
    }
}

/// 0-based indexes as a 1-based range string: [2,3,4,7] → "3-5, 8".
pub(crate) fn ranges_text(idx: &[usize]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < idx.len() {
        let mut j = i;
        while j + 1 < idx.len() && idx[j + 1] == idx[j] + 1 {
            j += 1;
        }
        out.push(if i == j {
            format!("{}", idx[i] + 1)
        } else {
            format!("{}-{}", idx[i] + 1, idx[j] + 1)
        });
        i = j + 1;
    }
    out.join(", ")
}

impl Render for ReplaceDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let name = self
            .file
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let summary = match self.plan(cx) {
            Ok((pages, from)) if pages.len() == from.len() => format!(
                "{} page{} replaced; bookmarks and comments on them stay",
                pages.len(),
                if pages.len() == 1 { "" } else { "s" }
            ),
            Ok((pages, from)) if pages.len() > from.len() => format!(
                "{} replaced, {} more deleted",
                from.len(),
                pages.len() - from.len()
            ),
            Ok((pages, from)) => format!(
                "{} replaced, {} more inserted after them",
                pages.len(),
                from.len() - pages.len()
            ),
            Err(e) => e,
        };
        let ok = self.plan(cx).is_ok();
        let field = |input: &Entity<TextInput>, note: String| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().w(px(160.0)).child(input.clone()))
                .child(dialog::subtitle(note))
                .into_any_element()
        };
        dialog::backdrop("replace-backdrop")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|d, _: &DialogRun, _, cx| d.go(cx)))
            .on_action(cx.listener(|_, _: &DialogCancel, _, cx| cx.emit(InsertEvent::Dismiss)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(InsertEvent::Dismiss)),
            )
            .child(
                dialog::card("replace-dialog", px(600.0))
                    .child(dialog::header("Replace Pages").child(dialog::subtitle(name)))
                    .child(dialog::field_row(
                        "Replace".to_string(),
                        px(90.0),
                        false,
                        field(
                            &self.these,
                            format!("of this document ({})", self.doc_pages),
                        ),
                    ))
                    .child(dialog::field_row(
                        "With".to_string(),
                        px(90.0),
                        false,
                        field(&self.with, format!("of the file ({})", self.file_pages)),
                    ))
                    .child(match &self.error {
                        Some(e) => dialog::message(e.clone(), true),
                        None => dialog::message(summary, !ok),
                    })
                    .child(
                        dialog::footer()
                            .child(dialog::hint("Enter replaces"))
                            .child(
                                Button::new("replace-cancel", "Cancel").on_click(
                                    cx.listener(|_, _, _, cx| cx.emit(InsertEvent::Dismiss)),
                                ),
                            )
                            .child(
                                Button::new("replace-go", "Replace")
                                    .primary()
                                    .on_click(cx.listener(|d, _, _, cx| d.go(cx))),
                            ),
                    ),
            )
            .text_color(theme::TEXT_SOFT)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn ranges_text_runs() {
        assert_eq!(super::ranges_text(&[2, 3, 4, 7]), "3-5, 8");
        assert_eq!(super::ranges_text(&[0]), "1");
        assert_eq!(super::ranges_text(&[]), "");
    }
}
