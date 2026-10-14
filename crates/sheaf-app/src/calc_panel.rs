//! The Calculator panel: a 10-key adding machine whose tape can be placed on
//! a page as an annotation (PDFlyer's calculator tape).
//!
//! Type an amount and an operator (`+ - * /`, or Enter for `+`); `=`, `t`
//! or Enter on an empty entry prints the running total and starts a new
//! subtotal. "Place on page" then drops the tape where you next click.

use gpui::{AnyElement, Context, Entity, Focusable, Subscription, Window, div, prelude::*, px};
use sheaf_doc::{
    AnnotKind, Box2, PageId, Pt,
    annot::{TAPE_LINE, TAPE_OP_W, TAPE_PAD, helv_width, tape_lines},
};
use sheaf_ui::text_input::{TextInput, TextInputEvent};
use sheaf_ui::theme;

use crate::docview::{DocView, State};

const ROW_H: f32 = 22.0;
/// Point size of a placed tape.
const TAPE_PT: f32 = 10.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Total,
}

impl Op {
    pub fn symbol(self) -> &'static str {
        match self {
            Op::Add => "+",
            Op::Sub => "-",
            Op::Mul => "×",
            Op::Div => "÷",
            Op::Total => "T",
        }
    }

    fn from_char(c: char) -> Option<Op> {
        match c {
            '+' => Some(Op::Add),
            '-' => Some(Op::Sub),
            '*' | 'x' | 'X' | '×' => Some(Op::Mul),
            '/' | '÷' => Some(Op::Div),
            '=' | 't' | 'T' => Some(Op::Total),
            _ => None,
        }
    }

    const KEYS: [Op; 5] = [Op::Add, Op::Sub, Op::Mul, Op::Div, Op::Total];
}

#[derive(Clone, Copy, Debug)]
pub struct TapeLine {
    pub value: f64,
    pub op: Op,
}

pub struct CalcTape {
    pub input: Entity<TextInput>,
    pub _events: Subscription,
    pub _watch: Subscription,
    pub lines: Vec<TapeLine>,
    /// Running total since the last `T`.
    pub running: f64,
    /// The next click on a page places the tape.
    pub placing: bool,
    pub scroll: gpui::ScrollHandle,
}

impl CalcTape {
    /// The tape as annotation contents: one `"<amount> <op>"` per line.
    pub fn contents(&self) -> String {
        self.lines
            .iter()
            .map(|l| format!("{} {}", fmt_amount(l.value), l.op.symbol()))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Two decimals with thousands separators; negatives get a leading minus.
pub fn fmt_amount(v: f64) -> String {
    let neg = v < -0.004;
    let cents = (v.abs() * 100.0).round() as u64;
    let (whole, frac) = (cents / 100, cents % 100);
    let digits = whole.to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    format!("{}{out}.{frac:02}", if neg { "-" } else { "" })
}

/// Accept what people paste from workpapers: commas, `$`, spaces, and
/// accounting-style `(negative)`.
fn parse_amount(s: &str) -> Option<f64> {
    let cleaned: String = s
        .chars()
        .filter(|c| !matches!(c, ',' | '$' | ' '))
        .collect();
    let cleaned = cleaned.trim_start_matches('+');
    let (neg, body) = match cleaned.strip_prefix('(').and_then(|b| b.strip_suffix(')')) {
        Some(b) => (true, b),
        None => (false, cleaned),
    };
    let v: f64 = body.parse().ok()?;
    Some(if neg { -v } else { v })
}

impl DocView {
    fn ensure_calc(&mut self, cx: &mut Context<Self>) {
        if self.calc.is_some() {
            return;
        }
        let input = cx.new(|cx| TextInput::new("0.00", cx));
        let events = cx.subscribe(&input, |this, input, ev: &TextInputEvent, cx| match ev {
            TextInputEvent::Confirm => {
                if input.read(cx).text().trim().is_empty() {
                    this.calc_total(cx);
                } else {
                    this.calc_apply(Op::Add, cx);
                }
            }
            TextInputEvent::Cancel => this.pending_focus = true,
        });
        // Operators typed into the entry act immediately.
        let watch = cx.observe(&input, |this, input, cx| {
            let text = input.read(cx).text().to_string();
            let Some(last) = text.chars().last() else {
                return;
            };
            let Some(op) = Op::from_char(last) else {
                return;
            };
            let body = text[..text.len() - last.len_utf8()].trim().to_string();
            if body.is_empty() {
                // A lone minus is a sign; other operators with nothing to
                // apply to are dropped, except that `=` prints the total.
                if op == Op::Sub {
                    return;
                }
                input.update(cx, |i, cx| i.set_text("", cx));
                if op == Op::Total {
                    this.calc_total(cx);
                }
                return;
            }
            input.update(cx, |i, cx| i.set_text(body, cx));
            this.calc_apply(op, cx);
        });
        self.calc = Some(CalcTape {
            input,
            _events: events,
            _watch: watch,
            lines: Vec::new(),
            running: 0.0,
            placing: false,
            scroll: gpui::ScrollHandle::new(),
        });
    }

    /// Make sure the panel exists and put the cursor in its entry.
    pub fn focus_calc_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_calc(cx);
        let input = self.calc.as_ref().unwrap().input.clone();
        input.update(cx, |i, cx| i.select_all(cx));
        window.focus(&input.read(cx).focus_handle(cx), cx);
        cx.notify();
    }

    /// Apply `op` to the amount in the entry. An unparseable entry is left
    /// in place for the user to fix.
    pub(crate) fn calc_apply(&mut self, op: Op, cx: &mut Context<Self>) {
        self.ensure_calc(cx);
        let calc = self.calc.as_mut().unwrap();
        let text = calc.input.read(cx).text().to_string();
        if text.trim().is_empty() {
            if op == Op::Total {
                self.calc_total(cx);
            }
            return;
        }
        let Some(v) = parse_amount(&text) else { return };
        match op {
            Op::Add | Op::Total => calc.running += v,
            Op::Sub => calc.running -= v,
            Op::Mul => calc.running *= v,
            Op::Div => {
                if v != 0.0 {
                    calc.running /= v
                }
            }
        }
        calc.lines.push(TapeLine {
            value: v,
            op: if op == Op::Total { Op::Add } else { op },
        });
        calc.input.update(cx, |i, cx| i.set_text("", cx));
        if op == Op::Total {
            self.calc_total(cx);
        } else {
            self.scroll_tape_to_end();
            cx.notify();
        }
    }

    /// Print the running total and start a new subtotal.
    pub(crate) fn calc_total(&mut self, cx: &mut Context<Self>) {
        self.ensure_calc(cx);
        let calc = self.calc.as_mut().unwrap();
        if calc.lines.is_empty() {
            return;
        }
        calc.lines.push(TapeLine {
            value: calc.running,
            op: Op::Total,
        });
        calc.running = 0.0;
        self.scroll_tape_to_end();
        cx.notify();
    }

    pub(crate) fn calc_clear(&mut self, cx: &mut Context<Self>) {
        let Some(calc) = &mut self.calc else { return };
        calc.lines.clear();
        calc.running = 0.0;
        calc.placing = false;
        calc.input.update(cx, |i, cx| i.set_text("", cx));
        cx.notify();
    }

    fn scroll_tape_to_end(&mut self) {
        let Some(calc) = &mut self.calc else { return };
        let total_h = calc.lines.len() as f32 * ROW_H + ROW_H;
        let view_h = f32::from(calc.scroll.bounds().size.height);
        let y = (total_h - view_h).max(0.0);
        calc.scroll.set_offset(gpui::point(px(0.0), px(-y)));
    }

    /// "Place on page": the next click on a page drops the tape there.
    pub(crate) fn calc_place_requested(&mut self, cx: &mut Context<Self>) {
        let Some(calc) = &mut self.calc else { return };
        if calc.lines.is_empty() {
            return;
        }
        calc.placing = true;
        self.pending_focus = true;
        if let State::Ready(doc) = &mut self.state {
            doc.notice = Some("Click on a page to place the tape (Esc cancels)".into());
        }
        cx.notify();
    }

    pub(crate) fn cancel_tape_placing(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(calc) = &mut self.calc else {
            return false;
        };
        if !calc.placing {
            return false;
        }
        calc.placing = false;
        if let State::Ready(doc) = &mut self.state {
            doc.notice = Some("Tape not placed".into());
        }
        cx.notify();
        true
    }

    /// Page click while placing: add the tape with its top-left at `p`.
    /// Returns whether the click was consumed.
    pub(crate) fn place_tape_at(&mut self, page: PageId, p: Pt, cx: &mut Context<Self>) -> bool {
        let Some(calc) = &mut self.calc else {
            return false;
        };
        if !calc.placing {
            return false;
        }
        calc.placing = false;
        let contents = calc.contents();
        if contents.is_empty() {
            return true;
        }
        let size = TAPE_PT;
        let lines = tape_lines(&contents);
        let widest = lines
            .iter()
            .map(|(v, _)| helv_width(v, size))
            .fold(0.0, f32::max);
        let w = (widest + size * TAPE_OP_W + 2.0 * TAPE_PAD + size * 0.5).max(72.0);
        let h = lines.len() as f32 * size * TAPE_LINE + 2.0 * TAPE_PAD + size * 0.3;
        let mut a = self.new_annot(
            AnnotKind::Tape {
                rect: Box2 {
                    x: p.x,
                    y: p.y,
                    w,
                    h,
                },
                size,
            },
            [20, 20, 20],
        );
        a.contents = contents;
        self.add_annot(page, a, "Place tape", cx);
        true
    }

    /// The Calculator panel. `wide`: it sits in a bottom dock, so the keys
    /// go in a column on the left with the tape beside them.
    pub fn render_calc(
        &mut self,
        wide: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.ensure_calc(cx);
        let calc = self.calc.as_ref().unwrap();
        let input = calc.input.clone();

        let key = |id: &'static str, label: &'static str, wide_key: bool| {
            div()
                .id(id)
                .h(px(26.0))
                .when(wide_key, |d| d.px_3())
                .when(!wide_key, |d| d.w(px(34.0)))
                .rounded_sm()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .cursor_pointer()
                .bg(theme::CONTROL)
                .hover(|s| s.bg(theme::CONTROL_HOVER))
                .child(label)
        };
        let mut keys: Vec<AnyElement> = Op::KEYS
            .iter()
            .map(|&op| {
                key(op.symbol(), op.symbol(), false)
                    .on_click(cx.listener(move |v, _, _, cx| v.calc_apply(op, cx)))
                    .into_any_element()
            })
            .collect();
        keys.push(
            key("C", "C", false)
                .on_click(cx.listener(|v, _, _, cx| v.calc_clear(cx)))
                .into_any_element(),
        );

        let n = calc.lines.len();
        let open_section = calc.lines.last().is_some_and(|l| l.op != Op::Total);
        let status = if n == 0 {
            "Type an amount and + − × ÷; Enter totals".to_string()
        } else if open_section {
            format!("{n} entries · running {}", fmt_amount(calc.running))
        } else {
            format!("{n} entries")
        };
        let placing = calc.placing;
        let place = key(
            "place",
            if placing {
                "Click a page…"
            } else {
                "Place on page"
            },
            true,
        )
        .when(placing, |d| d.bg(theme::ACCENT))
        .on_click(cx.listener(|v, _, _, cx| v.calc_place_requested(cx)));

        // Tape rows.
        let mut rows: Vec<AnyElement> = calc
            .lines
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let total = l.op == Op::Total;
                div()
                    .absolute()
                    .top(px(i as f32 * ROW_H))
                    .left(px(0.0))
                    .w_full()
                    .h(px(ROW_H))
                    .flex()
                    .items_center()
                    .justify_end()
                    .px_2()
                    .text_sm()
                    .text_color(theme::TEXT)
                    .when(total, |d| {
                        d.border_t_1()
                            .border_color(theme::TOTAL_RULE)
                            .font_weight(gpui::FontWeight::BOLD)
                    })
                    .child(div().child(fmt_amount(l.value)))
                    .child(
                        div()
                            .w(px(18.0))
                            .pl_1()
                            .text_color(theme::TEXT_MUTED)
                            .child(l.op.symbol()),
                    )
                    .into_any_element()
            })
            .collect();
        if open_section {
            rows.push(
                div()
                    .absolute()
                    .top(px(n as f32 * ROW_H))
                    .left(px(0.0))
                    .w_full()
                    .h(px(ROW_H))
                    .flex()
                    .items_center()
                    .justify_end()
                    .px_2()
                    .text_sm()
                    .text_color(theme::TEXT_DIM)
                    .child(div().child(fmt_amount(calc.running)))
                    .child(div().w(px(18.0)).pl_1().child("="))
                    .into_any_element(),
            );
        }
        let total_h = (n + 1) as f32 * ROW_H;

        let controls = div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .child(input)
            .child(div().flex().flex_wrap().gap_1().children(keys))
            .child(div().flex().child(place))
            .child(div().text_xs().text_color(theme::TEXT_MUTED).child(status));
        let tape = div()
            .id("calc-tape")
            .track_scroll(&calc.scroll)
            .overflow_y_scroll()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .child(div().relative().w_full().h(px(total_h)).children(rows));
        if wide {
            div()
                .size_full()
                .flex()
                .child(
                    controls
                        .w(px(280.0))
                        .border_r_1()
                        .border_color(theme::DIVIDER),
                )
                .child(tape)
                .into_any_element()
        } else {
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(controls.border_b_1().border_color(theme::DIVIDER))
                .child(tape)
                .into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_format_and_parse() {
        assert_eq!(fmt_amount(0.0), "0.00");
        assert_eq!(fmt_amount(1234.5), "1,234.50");
        assert_eq!(fmt_amount(-1234567.891), "-1,234,567.89");
        assert_eq!(parse_amount("$1,234.50"), Some(1234.5));
        assert_eq!(parse_amount("(84.50)"), Some(-84.5));
        assert_eq!(parse_amount("-7"), Some(-7.0));
        assert_eq!(parse_amount("abc"), None);
    }
}
