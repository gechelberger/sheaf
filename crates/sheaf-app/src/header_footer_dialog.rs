//! Document > Header & Footer… and Bates Numbering…: text in six places
//! (left, center, right of the header and of the footer) with tokens for
//! the page number, page count, date, file name and Bates number. Shown
//! over the pages until the document is saved. Tab moves between fields,
//! Enter applies, Escape cancels.

use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, MouseButton,
    Subscription, Window, div, prelude::*, px,
};
use sheaf_doc::stamps::{Bates, HeaderFooter, TOKENS};
use sheaf_ui::button::{Button, Chip};
use sheaf_ui::text_input::{TextInput, TextInputEvent};
use sheaf_ui::{dialog, theme};

use crate::{DialogCancel, DialogNext, DialogPrev, DialogRun};

pub const KEY_CONTEXT: &str = "HeaderFooterDialog";

const COLORS: [([u8; 3], &str); 4] = [
    ([0, 0, 0], "Black"),
    ([110, 110, 110], "Gray"),
    ([20, 60, 170], "Blue"),
    ([190, 20, 20], "Red"),
];

pub enum HeaderFooterEvent {
    /// The settings, and the 0-based pages they're on (none: all).
    Apply(Box<HeaderFooter>, Option<Vec<usize>>),
    Remove,
    Dismiss,
}

pub struct HeaderFooterContext {
    /// What's there now, to update.
    pub current: Option<HeaderFooter>,
    pub page_count: usize,
    pub file: String,
    /// The current scope as a page range ("" for all pages).
    pub scope: String,
    /// Start with Bates numbering on.
    pub bates: bool,
}

/// The fields, in Tab order.
const F_SIZE: usize = 6;
const F_TOP: usize = 7;
const F_BOTTOM: usize = 8;
const F_SIDES: usize = 9;
const F_FIRST: usize = 10;
const F_PAGES: usize = 11;
const F_PREFIX: usize = 12;
const F_SUFFIX: usize = 13;
const F_DIGITS: usize = 14;
const F_START: usize = 15;
const FIELDS: usize = 16;

pub struct HeaderFooterDialog {
    focus: FocusHandle,
    ctx: HeaderFooterContext,
    fields: Vec<Entity<TextInput>>,
    color: [u8; 3],
    bates: bool,
    /// The field with focus, for Tab.
    at: usize,
    error: Option<String>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<HeaderFooterEvent> for HeaderFooterDialog {}

impl Focusable for HeaderFooterDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn inches(pt: f32) -> String {
    format!("{}", (pt / 72.0 * 100.0).round() / 100.0)
}

/// Today as MM/DD/YYYY.
fn today() -> String {
    let d = sheaf_doc::pdf_date_now();
    // D:YYYYMMDDhhmmss
    let (y, m, day) = (&d[2..6], &d[6..8], &d[8..10]);
    format!("{m}/{day}/{y}")
}

impl HeaderFooterDialog {
    pub fn new(ctx: HeaderFooterContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut hf = ctx.current.clone().unwrap_or_default();
        if ctx.current.is_none() && ctx.bates {
            hf.texts[5] = "<<bates>>".into();
        }
        let bates = hf.bates.clone().unwrap_or_default();
        let range = ctx.scope.clone();
        let texts: Vec<String> = hf
            .texts
            .iter()
            .cloned()
            .chain([
                format!("{}", hf.font_size),
                inches(hf.top),
                inches(hf.bottom),
                inches(hf.left.min(hf.right)),
                format!("{}", hf.first_page_number),
                range,
                bates.prefix.clone(),
                bates.suffix.clone(),
                format!("{}", bates.digits),
                format!("{}", bates.start),
            ])
            .collect();
        let placeholders = [
            "Header left",
            "Header center",
            "Header right",
            "Footer left",
            "Footer center",
            "Footer right",
            "10",
            "0.5",
            "0.5",
            "1",
            "1",
            "All pages",
            "e.g. ABC",
            "",
            "6",
            "1",
        ];
        let fields: Vec<Entity<TextInput>> = (0..FIELDS)
            .map(|i| {
                let text = texts[i].clone();
                cx.new(|cx| {
                    let mut t = TextInput::new(placeholders[i], cx);
                    t.set_text(text, cx);
                    t
                })
            })
            .collect();
        let mut subs: Vec<Subscription> = fields
            .iter()
            .map(|f| {
                cx.subscribe(f, |d, _, ev: &TextInputEvent, cx| match ev {
                    TextInputEvent::Confirm => d.go(cx),
                    TextInputEvent::Cancel => cx.emit(HeaderFooterEvent::Dismiss),
                })
            })
            .collect();
        for f in &fields {
            subs.push(cx.observe(f, |_, _, cx| cx.notify()));
        }
        let focus = cx.focus_handle();
        let first = if ctx.bates && ctx.current.is_none() {
            F_PREFIX
        } else {
            4
        };
        window.focus(&fields[first].read(cx).focus_handle(cx), cx);
        let has_bates = ctx.bates || hf.bates.is_some();
        Self {
            focus,
            color: hf.color,
            bates: has_bates,
            ctx,
            fields,
            at: first,
            error: None,
            _subs: subs,
        }
    }

    fn order(&self) -> Vec<usize> {
        (0..FIELDS)
            .filter(|&i| self.bates || !(F_PREFIX..=F_START).contains(&i))
            .collect()
    }

    fn step(&mut self, by: i64, window: &mut Window, cx: &mut Context<Self>) {
        let order = self.order();
        let at = order.iter().position(|&i| i == self.at).unwrap_or(0) as i64;
        self.at = order[(at + by).rem_euclid(order.len() as i64) as usize];
        let h = self.fields[self.at].read(cx).focus_handle(cx);
        window.focus(&h, cx);
        cx.notify();
    }

    fn text(&self, i: usize, cx: &App) -> String {
        self.fields[i].read(cx).text().trim().to_string()
    }

    /// The header/footer the fields describe, or what's wrong.
    fn settings(&self, cx: &App) -> Result<(HeaderFooter, Option<Vec<usize>>), String> {
        let num = |i: usize, name: &str, lo: f32, hi: f32| -> Result<f32, String> {
            let t = self.text(i, cx);
            let v: f32 = t
                .parse()
                .map_err(|_| format!("{name}: '{t}' isn't a number"))?;
            if !(lo..=hi).contains(&v) {
                return Err(format!("{name}: from {lo} to {hi}"));
            }
            Ok(v)
        };
        let mut texts: [String; 6] = Default::default();
        for (i, t) in texts.iter_mut().enumerate() {
            *t = self.fields[i].read(cx).text().to_string();
        }
        if texts.iter().all(|t| t.trim().is_empty()) {
            return Err("Type something in at least one place".into());
        }
        let font_size = num(F_SIZE, "Size", 4.0, 72.0)?;
        let top = num(F_TOP, "Top", 0.0, 5.0)? * 72.0;
        let bottom = num(F_BOTTOM, "Bottom", 0.0, 5.0)? * 72.0;
        let sides = num(F_SIDES, "Sides", 0.0, 5.0)? * 72.0;
        let first = num(F_FIRST, "Start numbers at", 1.0, 1e6)? as u32;
        let pages = match self.text(F_PAGES, cx) {
            t if t.is_empty() => None,
            t => Some(
                sheaf_ops::pages::parse(&t, self.ctx.page_count)
                    .map_err(|e| format!("Pages: {e}"))?,
            ),
        };
        let bates = if self.bates {
            let digits = num(F_DIGITS, "Digits", 1.0, 12.0)? as u8;
            let start = self
                .text(F_START, cx)
                .parse::<u64>()
                .map_err(|_| "Bates start must be a whole number".to_string())?;
            Some(Bates {
                prefix: self.fields[F_PREFIX].read(cx).text().to_string(),
                suffix: self.fields[F_SUFFIX].read(cx).text().to_string(),
                digits,
                start,
            })
        } else {
            None
        };
        if bates.is_some() && !texts.iter().any(|t| t.contains("<<bates>>")) {
            return Err("Put <<bates>> where the Bates number goes".into());
        }
        let hf = HeaderFooter {
            texts,
            font_size,
            color: self.color,
            top,
            bottom,
            left: sides,
            right: sides,
            first_page_number: first,
            pages: None,
            date: self
                .ctx
                .current
                .as_ref()
                .map(|c| c.date.clone())
                .filter(|d| !d.is_empty())
                .unwrap_or_else(today),
            file: self.ctx.file.clone(),
            bates,
        };
        Ok((hf, pages))
    }

    fn go(&mut self, cx: &mut Context<Self>) {
        match self.settings(cx) {
            Ok((hf, pages)) => cx.emit(HeaderFooterEvent::Apply(Box::new(hf), pages)),
            Err(e) => {
                self.error = Some(e);
                cx.notify();
            }
        }
    }

    fn field(&self, i: usize, w: f32) -> AnyElement {
        div()
            .w(px(w))
            .child(self.fields[i].clone())
            .into_any_element()
    }
}

impl Render for HeaderFooterDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let slots = |base: usize| {
            div()
                .flex()
                .gap_2()
                .children((0..3).map(|k| div().flex_1().child(self.fields[base + k].clone())))
                .into_any_element()
        };
        let colors = div()
            .flex()
            .gap_1()
            .children(COLORS.iter().enumerate().map(|(i, (c, l))| {
                let c = *c;
                Chip::new(("hf-color", i), *l)
                    .selected(c == self.color)
                    .on_click(cx.listener(move |d, _, _, cx| {
                        d.color = c;
                        cx.notify();
                    }))
            }));
        let row = |label: &str, el: AnyElement| {
            dialog::field_row(label.to_string(), px(110.0), false, el).into_any_element()
        };
        let pair = |a: AnyElement, note: &str| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(a)
                .child(dialog::subtitle(note.to_string()))
                .into_any_element()
        };
        let tokens = TOKENS
            .iter()
            .filter(|(t, _)| self.bates || *t != "<<bates>>")
            .map(|(t, what)| format!("{t} {what}"))
            .collect::<Vec<_>>()
            .join(" · ");
        let bates_toggle = Chip::new(
            ("hf-bates", 0usize),
            format!("{} Bates numbering", if self.bates { "☑" } else { "☐" }),
        )
        .on_click(cx.listener(|d, _, _, cx| {
            d.bates = !d.bates;
            d.error = None;
            cx.notify();
        }))
        .into_any_element();
        let preview = match self.settings(cx) {
            Ok((hf, _)) => {
                let f = sheaf_doc::stamps::PageFacts {
                    index: 0,
                    count: self.ctx.page_count,
                    bates: hf.bates.as_ref().map(|b| b.start),
                };
                let size = sheaf_core::PageSize {
                    width: 612.0,
                    height: 792.0,
                };
                let shown: Vec<String> = hf.layout(size, &f).into_iter().map(|p| p.text).collect();
                Ok(format!("Page 1: {}", shown.join("  ·  ")))
            }
            Err(e) => Err(e),
        };
        dialog::backdrop("hf-backdrop")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|d, _: &DialogNext, window, cx| d.step(1, window, cx)))
            .on_action(cx.listener(|d, _: &DialogPrev, window, cx| d.step(-1, window, cx)))
            .on_action(cx.listener(|d, _: &DialogRun, _, cx| d.go(cx)))
            .on_action(
                cx.listener(|_, _: &DialogCancel, _, cx| cx.emit(HeaderFooterEvent::Dismiss)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(HeaderFooterEvent::Dismiss)),
            )
            .child(
                dialog::card("hf-dialog", px(760.0))
                    .child(dialog::header("Header & Footer").child(dialog::subtitle(
                        "shown on the pages now, written into them when you save",
                    )))
                    .child(row("Header", slots(0)))
                    .child(row("Footer", slots(3)))
                    .child(div().px_2().child(dialog::subtitle(tokens)))
                    .child(row("Text", pair(self.field(F_SIZE, 60.0), "pt")))
                    .child(row("Color", colors.into_any_element()))
                    .child(row(
                        "Margins",
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(dialog::subtitle("top"))
                            .child(self.field(F_TOP, 60.0))
                            .child(dialog::subtitle("bottom"))
                            .child(self.field(F_BOTTOM, 60.0))
                            .child(dialog::subtitle("sides"))
                            .child(self.field(F_SIDES, 60.0))
                            .child(dialog::subtitle("in"))
                            .into_any_element(),
                    ))
                    .child(row(
                        "Page numbers",
                        pair(self.field(F_FIRST, 60.0), "the first page's <<1>>"),
                    ))
                    .child(row(
                        "Pages",
                        pair(
                            self.field(F_PAGES, 160.0),
                            &format!("of {} (empty: all)", self.ctx.page_count),
                        ),
                    ))
                    .child(row("Bates", bates_toggle))
                    .when(self.bates, |d| {
                        d.child(row(
                            "",
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(dialog::subtitle("prefix"))
                                .child(self.field(F_PREFIX, 100.0))
                                .child(dialog::subtitle("suffix"))
                                .child(self.field(F_SUFFIX, 80.0))
                                .child(dialog::subtitle("digits"))
                                .child(self.field(F_DIGITS, 44.0))
                                .child(dialog::subtitle("start"))
                                .child(self.field(F_START, 90.0))
                                .into_any_element(),
                        ))
                    })
                    .child(match (&self.error, preview) {
                        (Some(e), _) => dialog::message(e.clone(), true),
                        (None, Ok(p)) => dialog::message(p, false),
                        (None, Err(e)) => dialog::message(e, true),
                    })
                    .child(
                        dialog::footer()
                            .child(dialog::hint("Tab moves, Enter applies"))
                            .when(self.ctx.current.is_some(), |d| {
                                d.child(Button::new("hf-remove", "Remove").on_click(
                                    cx.listener(|_, _, _, cx| cx.emit(HeaderFooterEvent::Remove)),
                                ))
                            })
                            .child(Button::new("hf-cancel", "Cancel").on_click(
                                cx.listener(|_, _, _, cx| cx.emit(HeaderFooterEvent::Dismiss)),
                            ))
                            .child(
                                Button::new(
                                    "hf-go",
                                    if self.ctx.current.is_some() {
                                        "Update"
                                    } else {
                                        "Add"
                                    },
                                )
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
    fn today_is_a_date() {
        let t = super::today();
        assert_eq!(t.len(), 10);
        assert_eq!(&t[2..3], "/");
    }
}
