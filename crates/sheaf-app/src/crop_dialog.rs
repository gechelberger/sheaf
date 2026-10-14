//! Document > Crop Pages…: take margins off pages, or trim them to their
//! content. Margins are as the page is shown. Tab moves between rows,
//! Left/Right change, Enter crops, Escape cancels.

use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, MouseButton,
    Subscription, Window, div, prelude::*, px,
};
use sheaf_core::PageSize;
use sheaf_ops::crop::{CropSpec, Margins};
use sheaf_ui::button::{Button, Chip};
use sheaf_ui::text_input::{TextInput, TextInputEvent};
use sheaf_ui::{dialog, theme};

use crate::{DialogCancel, DialogLeft, DialogNext, DialogPrev, DialogRight, DialogRun};

pub const KEY_CONTEXT: &str = "CropDialog";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Method {
    Margins,
    Trim,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnitKind {
    Inch,
    Mm,
    Pt,
}

impl UnitKind {
    fn points(self) -> f32 {
        match self {
            UnitKind::Inch => 72.0,
            UnitKind::Mm => 72.0 / 25.4,
            UnitKind::Pt => 1.0,
        }
    }

    fn label(self) -> &'static str {
        match self {
            UnitKind::Inch => "in",
            UnitKind::Mm => "mm",
            UnitKind::Pt => "pt",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pages {
    Current,
    Selected,
    All,
    Range,
}

pub struct CropContext {
    pub page_count: usize,
    pub current: usize,
    /// 0-based, in order.
    pub selected: Vec<usize>,
    /// The current page as shown.
    pub size: PageSize,
}

pub enum CropEvent {
    Crop { pages: Vec<usize>, spec: CropSpec },
    Dismiss,
}

const ROW_METHOD: usize = 0;
const ROW_UNIT: usize = 1;
const ROW_TOP: usize = 2;
const ROW_BOTTOM: usize = 3;
const ROW_LEFT: usize = 4;
const ROW_RIGHT: usize = 5;
const ROW_PAGES: usize = 6;
const ROW_RANGE: usize = 7;

pub struct CropDialog {
    focus: FocusHandle,
    ctx: CropContext,
    method: Method,
    unit: UnitKind,
    /// Top, bottom, left, right.
    sides: [Entity<TextInput>; 4],
    pages: Pages,
    range: Entity<TextInput>,
    row: usize,
    error: Option<String>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<CropEvent> for CropDialog {}

impl Focusable for CropDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl CropDialog {
    pub fn new(ctx: CropContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let side = |cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut t = TextInput::new("0", cx);
                t.set_text("0", cx);
                t
            })
        };
        let sides = [side(cx), side(cx), side(cx), side(cx)];
        let range = cx.new(|cx| TextInput::new("e.g. 1-3, 7, 10-", cx));
        let mut subs: Vec<Subscription> = sides
            .iter()
            .map(|i| {
                cx.subscribe(i, |d, _, ev: &TextInputEvent, cx| match ev {
                    TextInputEvent::Confirm => d.go(cx),
                    TextInputEvent::Cancel => cx.emit(CropEvent::Dismiss),
                })
            })
            .collect();
        subs.push(
            cx.subscribe(&range, |d, _, ev: &TextInputEvent, cx| match ev {
                TextInputEvent::Confirm => {
                    d.pages = Pages::Range;
                    d.go(cx)
                }
                TextInputEvent::Cancel => cx.emit(CropEvent::Dismiss),
            }),
        );
        for i in &sides {
            subs.push(cx.observe(i, |_, _, cx| cx.notify()));
        }
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let pages = if ctx.selected.len() > 1 {
            Pages::Selected
        } else {
            Pages::Current
        };
        Self {
            focus,
            ctx,
            method: Method::Margins,
            unit: UnitKind::Inch,
            sides,
            pages,
            range,
            row: ROW_METHOD,
            error: None,
            _subs: subs,
        }
    }

    fn rows(&self) -> Vec<usize> {
        let mut rows = vec![ROW_METHOD];
        if self.method == Method::Margins {
            rows.extend([ROW_UNIT, ROW_TOP, ROW_BOTTOM, ROW_LEFT, ROW_RIGHT]);
        }
        rows.extend([ROW_PAGES, ROW_RANGE]);
        rows
    }

    fn input_for(&self, row: usize) -> Option<&Entity<TextInput>> {
        match row {
            ROW_TOP => Some(&self.sides[0]),
            ROW_BOTTOM => Some(&self.sides[1]),
            ROW_LEFT => Some(&self.sides[2]),
            ROW_RIGHT => Some(&self.sides[3]),
            ROW_RANGE => Some(&self.range),
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
            ROW_METHOD => self.method = cycle(&[Method::Margins, Method::Trim], self.method, by),
            ROW_UNIT => {
                let old = self.unit;
                self.unit = cycle(&[UnitKind::Inch, UnitKind::Mm, UnitKind::Pt], old, by);
                self.convert(old, cx);
            }
            ROW_PAGES => {
                let opts: Vec<Pages> = [Pages::Current, Pages::Selected, Pages::All, Pages::Range]
                    .into_iter()
                    .filter(|p| *p != Pages::Selected || !self.ctx.selected.is_empty())
                    .collect();
                self.pages = cycle(&opts, self.pages, by);
            }
            _ => {}
        }
        self.error = None;
        cx.notify();
    }

    /// Rewrite the margin fields from `old` units into the current ones.
    fn convert(&mut self, old: UnitKind, cx: &mut Context<Self>) {
        let unit = self.unit;
        for i in &self.sides {
            let v: Option<f32> = i.read(cx).text().trim().parse().ok();
            if let Some(v) = v {
                let pt = v * old.points();
                let shown = pt / unit.points();
                let text = format!("{}", (shown * 100.0).round() / 100.0);
                i.update(cx, |t, cx| t.set_text(text, cx));
            }
        }
    }

    fn margins(&self, cx: &App) -> Result<Margins, String> {
        let names = ["Top", "Bottom", "Left", "Right"];
        let mut v = [0f32; 4];
        for (k, i) in self.sides.iter().enumerate() {
            let t = i.read(cx).text().trim().to_string();
            let n: f32 = if t.is_empty() {
                0.0
            } else {
                t.parse()
                    .map_err(|_| format!("{}: '{t}' isn't a number", names[k]))?
            };
            if !(0.0..=10_000.0).contains(&n) {
                return Err(format!("{}: margins can't be negative", names[k]));
            }
            v[k] = n * self.unit.points();
        }
        Ok(Margins {
            top: v[0],
            bottom: v[1],
            left: v[2],
            right: v[3],
        })
    }

    fn page_list(&self, cx: &App) -> Result<Vec<usize>, String> {
        let n = self.ctx.page_count;
        match self.pages {
            Pages::Current => Ok(vec![self.ctx.current.min(n.saturating_sub(1))]),
            Pages::Selected => Ok(self.ctx.selected.clone()),
            Pages::All => Ok((0..n).collect()),
            Pages::Range => sheaf_ops::pages::parse(self.range.read(cx).text(), n),
        }
    }

    fn go(&mut self, cx: &mut Context<Self>) {
        let spec = match self.method {
            Method::Trim => Ok(CropSpec::TrimWhite { pad: 9.0 }),
            Method::Margins => self.margins(cx).and_then(|m| {
                if m == Margins::default() {
                    Err("Every margin is zero: nothing to crop".into())
                } else {
                    Ok(CropSpec::Margins(m))
                }
            }),
        };
        match (spec, self.page_list(cx)) {
            (Ok(spec), Ok(pages)) if !pages.is_empty() => cx.emit(CropEvent::Crop { pages, spec }),
            (Err(e), _) | (_, Err(e)) => {
                self.error = Some(e);
                cx.notify();
            }
            _ => {
                self.error = Some("No pages to crop".into());
                cx.notify();
            }
        }
    }

    fn row_el(&self, index: usize, label: &str, content: AnyElement) -> AnyElement {
        dialog::field_row(label.to_string(), px(90.0), self.row == index, content)
            .into_any_element()
    }

    fn chips<T: Copy + PartialEq + 'static>(
        &self,
        name: &'static str,
        row: usize,
        opts: Vec<(T, String, bool)>,
        current: T,
        set: fn(&mut Self, T, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_wrap()
            .gap_1()
            .children(
                opts.into_iter()
                    .enumerate()
                    .map(|(i, (v, label, enabled))| {
                        Chip::new((name, i), label)
                            .selected(v == current)
                            .disabled(!enabled)
                            .on_click(cx.listener(move |d, _, window, cx| {
                                if enabled {
                                    set(d, v, cx);
                                    d.error = None;
                                    d.set_row(row, window, cx);
                                }
                            }))
                    }),
            )
            .into_any_element()
    }
}

impl Render for CropDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let method = self.chips(
            "crop-method",
            ROW_METHOD,
            vec![
                (Method::Margins, "Margins".into(), true),
                (Method::Trim, "Trim white margins".into(), true),
            ],
            self.method,
            |d, v, _| d.method = v,
            cx,
        );
        let unit = self.chips(
            "crop-unit",
            ROW_UNIT,
            [UnitKind::Inch, UnitKind::Mm, UnitKind::Pt]
                .into_iter()
                .map(|u| (u, u.label().to_string(), true))
                .collect(),
            self.unit,
            |d, v, cx| {
                let old = d.unit;
                d.unit = v;
                d.convert(old, cx);
            },
            cx,
        );
        let side = |i: usize| -> AnyElement {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().w(px(100.0)).child(self.sides[i].clone()))
                .child(dialog::subtitle(self.unit.label()))
                .into_any_element()
        };
        let n = self.ctx.page_count;
        let pages = self.chips(
            "crop-pages",
            ROW_PAGES,
            vec![
                (
                    Pages::Current,
                    format!("Current page ({})", self.ctx.current + 1),
                    true,
                ),
                (
                    Pages::Selected,
                    format!("Selected ({})", self.ctx.selected.len()),
                    !self.ctx.selected.is_empty(),
                ),
                (Pages::All, format!("All ({n})"), true),
                (Pages::Range, "Pages…".into(), true),
            ],
            self.pages,
            |d, v, _| d.pages = v,
            cx,
        );
        let range = div()
            .w(px(200.0))
            .child(self.range.clone())
            .into_any_element();
        let hint = match (self.method, self.margins(cx)) {
            (Method::Trim, _) => {
                "Each page is cropped to its content, with an eighth of an inch to spare".into()
            }
            (Method::Margins, Ok(m)) => {
                let u = self.unit.points();
                let w = (self.ctx.size.width - m.left - m.right) / u;
                let h = (self.ctx.size.height - m.top - m.bottom) / u;
                format!(
                    "The current page becomes {:.2} × {:.2} {}",
                    w.max(0.0),
                    h.max(0.0),
                    self.unit.label()
                )
            }
            (Method::Margins, Err(e)) => e,
        };
        dialog::backdrop("crop-backdrop")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|d, _: &DialogNext, window, cx| d.step_row(1, window, cx)))
            .on_action(cx.listener(|d, _: &DialogPrev, window, cx| d.step_row(-1, window, cx)))
            .on_action(cx.listener(|d, _: &DialogLeft, _, cx| d.adjust(-1, cx)))
            .on_action(cx.listener(|d, _: &DialogRight, _, cx| d.adjust(1, cx)))
            .on_action(cx.listener(|d, _: &DialogRun, _, cx| d.go(cx)))
            .on_action(cx.listener(|_, _: &DialogCancel, _, cx| cx.emit(CropEvent::Dismiss)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(CropEvent::Dismiss)),
            )
            .child(
                dialog::card("crop-dialog", px(600.0))
                    .child(dialog::header("Crop Pages").child(dialog::subtitle(
                        "what's cropped away is hidden, not deleted",
                    )))
                    .child(self.row_el(ROW_METHOD, "Crop to", method))
                    .when(self.method == Method::Margins, |d| {
                        d.child(self.row_el(ROW_UNIT, "Units", unit))
                            .child(self.row_el(ROW_TOP, "Top", side(0)))
                            .child(self.row_el(ROW_BOTTOM, "Bottom", side(1)))
                            .child(self.row_el(ROW_LEFT, "Left", side(2)))
                            .child(self.row_el(ROW_RIGHT, "Right", side(3)))
                    })
                    .child(self.row_el(ROW_PAGES, "Pages", pages))
                    .child(self.row_el(ROW_RANGE, "", range))
                    .child(match &self.error {
                        Some(e) => dialog::message(e.clone(), true),
                        None => dialog::message(hint, false),
                    })
                    .child(
                        dialog::footer()
                            .child(dialog::hint("Tab moves, ←/→ change, Enter crops"))
                            .child(
                                Button::new("crop-cancel", "Cancel").on_click(
                                    cx.listener(|_, _, _, cx| cx.emit(CropEvent::Dismiss)),
                                ),
                            )
                            .child(
                                Button::new("crop-go", "Crop")
                                    .primary()
                                    .on_click(cx.listener(|d, _, _, cx| d.go(cx))),
                            ),
                    ),
            )
            .text_color(theme::TEXT_SOFT)
    }
}
