//! A plugin's `ui.show-form`: the form it describes, drawn as a card over
//! the workspace like the OCR options. Tab moves between rows, Left/Right
//! (or Space) change a choice or checkbox, Enter submits, Escape cancels.

use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, MouseButton,
    PathPromptOptions, Subscription, Window, div, prelude::*, px,
};
use sheaf_doc::PageId;
use sheaf_plugin::ui::{FieldAnswer, FieldKind, FieldValue, Form, parse_page_range};
use sheaf_ui::button::{Button, Chip};
use sheaf_ui::text_input::{TextInput, TextInputEvent};
use sheaf_ui::{dialog, theme};

use crate::{DialogCancel, DialogLeft, DialogNext, DialogPrev, DialogRight, DialogRun};

pub const KEY_CONTEXT: &str = "PluginForm";

pub enum PluginFormEvent {
    Submit(Vec<FieldAnswer>),
    Dismiss,
}

enum FieldState {
    Text(Entity<TextInput>),
    Number {
        input: Entity<TextInput>,
        min: Option<f64>,
        max: Option<f64>,
    },
    Choice {
        options: Vec<String>,
        index: usize,
    },
    Checkbox(bool),
    File {
        path: Option<String>,
        save: bool,
    },
    PageRange(Entity<TextInput>),
}

pub struct PluginForm {
    focus: FocusHandle,
    /// Which plugin is asking, for the title bar.
    source: String,
    form: Form,
    fields: Vec<FieldState>,
    /// The document's pages, for page-range fields.
    pages: Vec<PageId>,
    _subs: Vec<Subscription>,
    /// Highlighted row; `fields.len()` is the buttons.
    row: usize,
    error: Option<String>,
}

impl EventEmitter<PluginFormEvent> for PluginForm {}

impl Focusable for PluginForm {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl PluginForm {
    pub fn new(
        source: String,
        form: Form,
        pages: Vec<PageId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut subs = Vec::new();
        let mut input = |text: String, placeholder: &str, cx: &mut Context<Self>| {
            let i = cx.new(|cx| {
                let mut t = TextInput::new(placeholder.to_string(), cx);
                t.set_text(text, cx);
                t
            });
            subs.push(
                cx.subscribe(&i, |this, _, ev: &TextInputEvent, cx| match ev {
                    TextInputEvent::Confirm => this.submit(cx),
                    TextInputEvent::Cancel => cx.emit(PluginFormEvent::Dismiss),
                }),
            );
            i
        };
        let fields = form
            .fields
            .iter()
            .map(|f| match &f.kind {
                FieldKind::Text {
                    default,
                    placeholder,
                } => FieldState::Text(input(default.clone(), placeholder, cx)),
                FieldKind::Number { default, min, max } => FieldState::Number {
                    input: input(format_number(*default), "", cx),
                    min: *min,
                    max: *max,
                },
                FieldKind::Choice { options, default } => FieldState::Choice {
                    options: options.clone(),
                    index: *default,
                },
                FieldKind::Checkbox(b) => FieldState::Checkbox(*b),
                FieldKind::File { save, .. } => FieldState::File {
                    path: None,
                    save: *save,
                },
                FieldKind::PageRange(text) => {
                    FieldState::PageRange(input(text.clone(), "all pages, or e.g. 1-3, 7", cx))
                }
            })
            .collect();
        let mut this = Self {
            focus: cx.focus_handle(),
            source,
            form,
            fields,
            pages,
            _subs: subs,
            row: 0,
            error: None,
        };
        this.set_row(0, window, cx);
        this
    }

    fn rows(&self) -> usize {
        self.fields.len() + 1
    }

    fn set_row(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.row = row.min(self.rows() - 1);
        let target = match self.fields.get(self.row) {
            Some(FieldState::Text(i))
            | Some(FieldState::Number { input: i, .. })
            | Some(FieldState::PageRange(i)) => i.read(cx).focus_handle(cx),
            _ => self.focus.clone(),
        };
        window.focus(&target, cx);
        cx.notify();
    }

    fn step_row(&mut self, by: i64, window: &mut Window, cx: &mut Context<Self>) {
        let row = (self.row as i64 + by).rem_euclid(self.rows() as i64) as usize;
        self.set_row(row, window, cx);
    }

    /// Left/Right/Space on the highlighted row.
    fn adjust(&mut self, by: i64, cx: &mut Context<Self>) {
        match self.fields.get_mut(self.row) {
            Some(FieldState::Choice { options, index }) if !options.is_empty() => {
                *index = (*index as i64 + by).rem_euclid(options.len() as i64) as usize;
            }
            Some(FieldState::Checkbox(b)) => *b = !*b,
            _ => return,
        }
        self.error = None;
        cx.notify();
    }

    fn pick_file(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(FieldState::File { save, .. }) = self.fields.get(index) else {
            return;
        };
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(std::path::PathBuf::from)
            .unwrap_or_default();
        let rx = if *save {
            let rx = crate::file_dialog::save(cx, &home, None);
            cx.spawn(async move |this, cx| {
                if let Ok(Ok(Some(path))) = rx.await {
                    this.update(cx, |f, cx| f.set_file(index, path, cx)).ok();
                }
            })
        } else {
            let rx = crate::file_dialog::open(
                cx,
                PathPromptOptions {
                    files: true,
                    directories: false,
                    multiple: false,
                    prompt: None,
                },
            );
            cx.spawn(async move |this, cx| {
                if let Ok(Ok(Some(mut paths))) = rx.await
                    && !paths.is_empty()
                {
                    let path = paths.remove(0);
                    this.update(cx, |f, cx| f.set_file(index, path, cx)).ok();
                }
            })
        };
        rx.detach();
    }

    fn set_file(&mut self, index: usize, path: std::path::PathBuf, cx: &mut Context<Self>) {
        if let Some(FieldState::File { path: p, .. }) = self.fields.get_mut(index) {
            *p = Some(path.display().to_string());
            cx.notify();
        }
    }

    /// Dev (`--script`): set fields by id from JSON, then submit.
    pub(crate) fn script_fill(
        &mut self,
        values: &serde_json::Map<String, serde_json::Value>,
        cx: &mut Context<Self>,
    ) {
        for (i, f) in self.form.fields.iter().enumerate() {
            let Some(v) = values.get(&f.id) else { continue };
            let text = || match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            match &mut self.fields[i] {
                FieldState::Text(input)
                | FieldState::Number { input, .. }
                | FieldState::PageRange(input) => {
                    let t = text();
                    input.update(cx, |inp, cx| inp.set_text(t, cx));
                }
                FieldState::Choice { index, options } => {
                    if let Some(n) = v.as_u64() {
                        *index = (n as usize).min(options.len().saturating_sub(1));
                    }
                }
                FieldState::Checkbox(b) => *b = v.as_bool().unwrap_or(*b),
                FieldState::File { path, .. } => *path = v.as_str().map(String::from),
            }
        }
        self.submit(cx);
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        match self.answers(cx) {
            Ok(a) => cx.emit(PluginFormEvent::Submit(a)),
            Err(e) => {
                self.error = Some(e);
                cx.notify();
            }
        }
    }

    fn answers(&self, cx: &App) -> Result<Vec<FieldAnswer>, String> {
        self.form
            .fields
            .iter()
            .zip(&self.fields)
            .map(|(f, st)| {
                let value = match st {
                    FieldState::Text(i) => FieldValue::Text(i.read(cx).text().to_string()),
                    FieldState::Number { input, min, max } => {
                        let text = input.read(cx).text().trim().replace(',', "");
                        let n: f64 = text
                            .parse()
                            .map_err(|_| format!("{}: “{text}” isn't a number", f.label))?;
                        if min.is_some_and(|m| n < m) || max.is_some_and(|m| n > m) {
                            return Err(format!(
                                "{}: must be {}",
                                f.label,
                                bounds_text(*min, *max)
                            ));
                        }
                        FieldValue::Number(n)
                    }
                    FieldState::Choice { index, .. } => FieldValue::Choice(*index),
                    FieldState::Checkbox(b) => FieldValue::Checkbox(*b),
                    FieldState::File { path, .. } => FieldValue::File(path.clone()),
                    FieldState::PageRange(i) => {
                        let idx = parse_page_range(i.read(cx).text(), self.pages.len())
                            .map_err(|e| format!("{}: {e}", f.label))?;
                        FieldValue::PageRange(idx.into_iter().map(|i| self.pages[i]).collect())
                    }
                };
                Ok(FieldAnswer {
                    id: f.id.clone(),
                    value,
                })
            })
            .collect()
    }

    fn chip(
        &self,
        id: (&'static str, usize),
        label: String,
        selected: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        Chip::new(id, label)
            .selected(selected)
            .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
            .into_any_element()
    }

    fn render_field(&self, i: usize, cx: &mut Context<Self>) -> AnyElement {
        match &self.fields[i] {
            FieldState::Text(input) | FieldState::PageRange(input) => {
                div().child(input.clone()).into_any_element()
            }
            FieldState::Number { input, min, max } => div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().w(px(140.0)).child(input.clone()))
                .when(min.is_some() || max.is_some(), |d| {
                    d.child(
                        div()
                            .text_xs()
                            .text_color(theme::TEXT_FAINT)
                            .child(bounds_text(*min, *max)),
                    )
                })
                .into_any_element(),
            FieldState::Choice { options, index } => div()
                .flex()
                .flex_wrap()
                .gap_1()
                .children(options.iter().enumerate().map(|(k, o)| {
                    self.chip(
                        ("choice", i * 1000 + k),
                        o.clone(),
                        k == *index,
                        move |this, window, cx| {
                            if let Some(FieldState::Choice { index, .. }) = this.fields.get_mut(i) {
                                *index = k;
                            }
                            this.set_row(i, window, cx);
                        },
                        cx,
                    )
                }))
                .into_any_element(),
            FieldState::Checkbox(b) => self.chip(
                ("check", i),
                if *b { "☑ Yes" } else { "☐ No" }.into(),
                false,
                move |this, window, cx| {
                    if let Some(FieldState::Checkbox(b)) = this.fields.get_mut(i) {
                        *b = !*b;
                    }
                    this.set_row(i, window, cx);
                },
                cx,
            ),
            FieldState::File { path, save } => div()
                .flex()
                .items_center()
                .gap_2()
                .child(self.chip(
                    ("file", i),
                    if *save { "Choose…" } else { "Browse…" }.into(),
                    false,
                    move |this, window, cx| {
                        this.set_row(i, window, cx);
                        this.pick_file(i, cx);
                    },
                    cx,
                ))
                .child(
                    div()
                        .flex_1()
                        .text_xs()
                        .text_color(theme::TEXT_DETAIL)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(path.clone().unwrap_or_else(|| "No file chosen".into())),
                )
                .into_any_element(),
        }
    }
}

fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{n:.0}")
    } else {
        n.to_string()
    }
}

fn bounds_text(min: Option<f64>, max: Option<f64>) -> String {
    match (min, max) {
        (Some(a), Some(b)) => format!("{}–{}", format_number(a), format_number(b)),
        (Some(a), None) => format!("at least {}", format_number(a)),
        (None, Some(b)) => format!("at most {}", format_number(b)),
        (None, None) => String::new(),
    }
}

impl Render for PluginForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows: Vec<AnyElement> = (0..self.fields.len())
            .map(|i| {
                let content = self.render_field(i, cx);
                let label = self.form.fields[i].label.clone();
                dialog::field_row(label, px(130.0), self.row == i, content).into_any_element()
            })
            .collect();
        let buttons_focused = self.row == self.fields.len();
        let buttons = [
            Button::new("form-cancel", "Cancel")
                .on_click(cx.listener(|_, _, _, cx| cx.emit(PluginFormEvent::Dismiss))),
            Button::new("form-submit", self.form.submit_label.clone())
                .primary()
                .focused(buttons_focused)
                .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
        ];

        dialog::backdrop("form-backdrop")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &DialogNext, window, cx| this.step_row(1, window, cx)))
            .on_action(
                cx.listener(|this, _: &DialogPrev, window, cx| this.step_row(-1, window, cx)),
            )
            .on_action(cx.listener(|this, _: &DialogLeft, _, cx| this.adjust(-1, cx)))
            .on_action(cx.listener(|this, _: &DialogRight, _, cx| this.adjust(1, cx)))
            .on_action(cx.listener(|this, _: &DialogRun, _, cx| this.submit(cx)))
            .on_action(cx.listener(|_, _: &DialogCancel, _, cx| cx.emit(PluginFormEvent::Dismiss)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(PluginFormEvent::Dismiss)),
            )
            .child(
                dialog::card("plugin-form", px(520.0))
                    .child(
                        dialog::header(self.form.title.clone())
                            .when(!same_name(&self.form.title, &self.source), |d| {
                                d.child(dialog::subtitle(format!("from {}", self.source)))
                            }),
                    )
                    .children(rows)
                    .when_some(self.error.clone(), |d, e| d.child(dialog::message(e, true)))
                    .child(
                        dialog::footer()
                            .child(dialog::hint(
                                "Tab moves, ←/→ change, Enter submits, Esc cancels",
                            ))
                            .children(buttons),
                    ),
            )
    }
}

/// Whether a form's title already names its plugin ("Bookmarks from
/// Pattern" from "Bookmark from Pattern"), so "from <plugin>" would only
/// repeat it: the same words in any order, ignoring case, punctuation and
/// plurals.
fn same_name(title: &str, plugin: &str) -> bool {
    let words = |s: &str| {
        s.split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(|w| {
                let w = w.to_lowercase();
                w.strip_suffix('s').map(str::to_string).unwrap_or(w)
            })
            .collect::<Vec<_>>()
    };
    let (mut a, mut b) = (words(title), words(plugin));
    a.sort();
    b.sort();
    a == b
}

#[cfg(test)]
mod name_tests {
    use super::same_name;

    #[test]
    fn titles_naming_their_plugin() {
        assert!(same_name("Bookmarks from Pattern", "Bookmark from Pattern"));
        assert!(same_name("Find Duplicate Pages…", "Find Duplicate Pages"));
        assert!(same_name("Convert Currency", "Currency Convert"));
        assert!(!same_name("Pick a Date", "Currency Convert"));
        assert!(!same_name(
            "Find Matching Amounts",
            "Amount Cross-Reference"
        ));
    }
}
