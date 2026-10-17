//! The Calculator panel: a 10-key adding machine whose tape is placed on a
//! page as an annotation, the way PDFlyer's calculator tape is.
//!
//! The tape runs in sections (`sheaf_doc::tape`): `1-1`, `1-2`… then the
//! total, `1-T`, and the next section is `2-`. Typed into the entry:
//!
//! - an amount, then `+ - x /`: add, subtract, multiply, divide;
//! - Space: subtotal (◊), the section goes on; `*` or Enter: total, which
//!   closes it; Shift+Enter: grand total (GT) of the section totals;
//! - S changes the entry's sign, C clears it, D cycles the decimals
//!   (2, 0, 4);
//! - memory: G adds the entry (or the running total) to it, H takes it
//!   off, J recalls it into the entry, K clears it, L stores the running
//!   total in it.
//!
//! Click a row to pick it: Insert puts the entry in above it, Delete takes
//! it out, and the totals are worked out again. Double-click a row for its
//! note (Ctrl+T puts in the date and time); Title… adds a section title.
//! Place puts the tape on the next page clicked; a placed tape's Edit Tape
//! (double-click it, or its right-click menu) brings it back here, and
//! Place then updates it where it is.
//!
//! Pick (a sheaf extension; PDFlyer's numbers are typed): while it's on, a
//! click on a number on the page adds it, read from the page's text (or
//! its unsaved OCR), and Shift+drag over a region adds every number in it.
//! Each can get a tickmark beside it.

use gpui::{
    AnyElement, Context, Entity, Focusable, KeyDownEvent, Subscription, Window, div, prelude::*, px,
};
use sheaf_core::Rotation;
use sheaf_doc::annot::tape_size;
use sheaf_doc::geom::Affine;
use sheaf_doc::tape::{Line, Op, Row, Tape, TapeLook, fmt_amount, parse_amount};
use sheaf_doc::{Annot, AnnotId, AnnotKind, Box2, Edit, PageId, Pt};
use sheaf_ui::text_input::{TextInput, TextInputEvent};
use sheaf_ui::theme;

use crate::docview::{DocView, State};

const ROW_H: f32 = 22.0;
/// Point size of a placed tape.
const TAPE_PT: f32 = 10.0;
/// What D cycles through.
const DECIMALS: [usize; 3] = [2, 0, 4];

pub struct CalcTape {
    pub input: Entity<TextInput>,
    pub _events: Subscription,
    pub _watch: Subscription,
    pub tape: Tape,
    pub memory: f64,
    pub decimals: usize,
    /// The row picked, for Insert, Delete and its note.
    pub selected: Option<usize>,
    /// A note (or, for `None`, a title) being written, and its box.
    pub writing: Option<(Option<usize>, Entity<TextInput>, Subscription)>,
    /// The next click on a page places the tape.
    pub placing: bool,
    /// The placed tape this one came from: Place updates it.
    pub editing: Option<(PageId, AnnotId)>,
    /// Clicks on the page pick numbers.
    pub picking: bool,
    /// The tickmark put beside each picked number, if any.
    pub tick: Option<String>,
    pub scroll: gpui::ScrollHandle,
}

impl CalcTape {
    /// The tape as text, for a placed tape's `/Contents`.
    pub fn contents(&self) -> String {
        self.tape.text(self.decimals)
    }
}

impl DocView {
    fn ensure_calc(&mut self, cx: &mut Context<Self>) {
        if self.calc.is_some() {
            return;
        }
        let input = cx.new(|cx| TextInput::new("0.00", cx));
        let events = cx.subscribe(&input, |this, input, ev: &TextInputEvent, cx| match ev {
            // Enter totals (adding what's typed first).
            TextInputEvent::Confirm => {
                let _ = input;
                this.calc_key(Key::Total, cx);
            }
            TextInputEvent::Cancel => this.calc_escape(cx),
        });
        // Keys typed into the entry act at once; digits stay.
        let watch = cx.observe(&input, |this, input, cx| {
            let text = input.read(cx).text().to_string();
            let Some(last) = text.chars().last() else {
                return;
            };
            let key = match last {
                '+' => Key::Op(Op::Add),
                // A lone minus starts a negative amount.
                '-' if text.len() > 1 => Key::Op(Op::Sub),
                'x' | 'X' | '×' => Key::Op(Op::Mul),
                '/' | '÷' => Key::Op(Op::Div),
                '*' => Key::Total,
                ' ' => Key::Subtotal,
                's' | 'S' => Key::Sign,
                'c' | 'C' => Key::ClearEntry,
                'd' | 'D' => Key::Decimals,
                'g' | 'G' => Key::MemAdd,
                'h' | 'H' => Key::MemSub,
                'j' | 'J' => Key::MemRecall,
                'k' | 'K' => Key::MemClear,
                'l' | 'L' => Key::MemStore,
                _ => return,
            };
            let body = text[..text.len() - last.len_utf8()].to_string();
            input.update(cx, |i, cx| i.set_text(body, cx));
            this.calc_key(key, cx);
        });
        self.calc = Some(CalcTape {
            input,
            _events: events,
            _watch: watch,
            tape: Tape::default(),
            memory: 0.0,
            decimals: 2,
            selected: None,
            writing: None,
            placing: false,
            editing: None,
            picking: false,
            tick: None,
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

    /// The amount in the entry, if it reads as one.
    fn calc_entry(&self, cx: &Context<Self>) -> Option<f64> {
        let calc = self.calc.as_ref()?;
        parse_amount(calc.input.read(cx).text().trim())
    }

    fn set_entry(&mut self, text: String, cx: &mut Context<Self>) {
        if let Some(calc) = &self.calc {
            calc.input.update(cx, |i, cx| i.set_text(text, cx));
        }
    }

    /// A key of the calculator, from the entry or a button.
    pub(crate) fn calc_key(&mut self, key: Key, cx: &mut Context<Self>) {
        self.ensure_calc(cx);
        let entry = self.calc_entry(cx);
        let typed = !self
            .calc
            .as_ref()
            .unwrap()
            .input
            .read(cx)
            .text()
            .trim()
            .is_empty();
        let calc = self.calc.as_mut().unwrap();
        let push = |calc: &mut CalcTape, row: Row| {
            calc.tape.lines.push(Line::new(row));
        };
        match key {
            Key::Op(op) => {
                let Some(v) = entry else { return };
                push(calc, Row::Entry { value: v, op });
                self.set_entry(String::new(), cx);
            }
            Key::Subtotal | Key::Total => {
                if typed {
                    let Some(v) = entry else { return };
                    push(
                        calc,
                        Row::Entry {
                            value: v,
                            op: Op::Add,
                        },
                    );
                    self.set_entry(String::new(), cx);
                }
                let calc = self.calc.as_mut().unwrap();
                if !calc.tape.open() {
                    return;
                }
                push(
                    calc,
                    if key == Key::Total {
                        Row::Total(0.0)
                    } else {
                        Row::Subtotal(0.0)
                    },
                );
            }
            Key::GrandTotal => {
                if calc.tape.open() {
                    push(calc, Row::Total(0.0));
                }
                if calc
                    .tape
                    .lines
                    .iter()
                    .any(|l| matches!(l.row, Row::Total(_)))
                {
                    push(calc, Row::GrandTotal(0.0));
                }
            }
            Key::Sign => {
                let text = calc.input.read(cx).text().trim().to_string();
                let flipped = match text.strip_prefix('-') {
                    Some(rest) => rest.to_string(),
                    None if !text.is_empty() => format!("-{text}"),
                    None => text,
                };
                self.set_entry(flipped, cx);
            }
            Key::ClearEntry => self.set_entry(String::new(), cx),
            Key::Decimals => {
                let i = DECIMALS
                    .iter()
                    .position(|d| *d == calc.decimals)
                    .unwrap_or(0);
                calc.decimals = DECIMALS[(i + 1) % DECIMALS.len()];
            }
            Key::Zeros(n) => {
                let text = format!("{}{}", calc.input.read(cx).text(), "0".repeat(n));
                self.set_entry(text, cx);
            }
            Key::MemAdd | Key::MemSub => {
                let v = entry.unwrap_or_else(|| calc.tape.running());
                calc.memory += if key == Key::MemAdd { v } else { -v };
            }
            Key::MemRecall => {
                let m = fmt_amount(calc.memory, calc.decimals).replace(',', "");
                self.set_entry(m, cx);
            }
            Key::MemClear => calc.memory = 0.0,
            Key::MemStore => calc.memory = calc.tape.running(),
        }
        if let Some(calc) = &mut self.calc {
            calc.tape.retotal();
        }
        self.scroll_tape_to_end();
        cx.notify();
    }

    /// Esc: clear the entry; on an empty one, back to the document.
    fn calc_escape(&mut self, cx: &mut Context<Self>) {
        let empty = self
            .calc
            .as_ref()
            .is_none_or(|c| c.input.read(cx).text().is_empty());
        if empty {
            self.pending_focus = true;
        } else {
            self.set_entry(String::new(), cx);
        }
        cx.notify();
    }

    pub(crate) fn calc_clear(&mut self, cx: &mut Context<Self>) {
        let Some(calc) = &mut self.calc else { return };
        calc.tape = Tape::default();
        calc.selected = None;
        calc.placing = false;
        calc.editing = None;
        calc.input.update(cx, |i, cx| i.set_text("", cx));
        cx.notify();
    }

    /// Insert: the entry, added, above the picked row (or at the end).
    fn calc_insert(&mut self, cx: &mut Context<Self>) {
        let Some(v) = self.calc_entry(cx) else { return };
        let Some(calc) = &mut self.calc else { return };
        let at = calc.selected.unwrap_or(calc.tape.lines.len());
        calc.tape.lines.insert(
            at,
            Line::new(Row::Entry {
                value: v,
                op: Op::Add,
            }),
        );
        calc.tape.retotal();
        calc.selected = Some(at + 1);
        self.set_entry(String::new(), cx);
        cx.notify();
    }

    /// Delete: the picked row.
    fn calc_delete(&mut self, cx: &mut Context<Self>) {
        let Some(calc) = &mut self.calc else { return };
        let Some(i) = calc.selected.filter(|i| *i < calc.tape.lines.len()) else {
            return;
        };
        calc.tape.lines.remove(i);
        calc.tape.retotal();
        calc.selected = (!calc.tape.lines.is_empty()).then(|| i.min(calc.tape.lines.len() - 1));
        cx.notify();
    }

    /// Write row `row`'s note, or (None) a title for a new section.
    fn calc_start_writing(
        &mut self,
        row: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(calc) = &self.calc else { return };
        let text = row
            .and_then(|i| calc.tape.lines.get(i))
            .map(|l| l.note.clone())
            .unwrap_or_default();
        let input = cx.new(|cx| {
            let mut t = TextInput::new(
                if row.is_some() {
                    "Note; Ctrl+T for the date and time"
                } else {
                    "Section title"
                },
                cx,
            );
            t.set_text(text, cx);
            t
        });
        let sub = cx.subscribe(&input, |v, _, ev: &TextInputEvent, cx| {
            v.calc_finish_writing(*ev == TextInputEvent::Confirm, cx);
        });
        window.focus(&input.read(cx).focus_handle(cx), cx);
        if let Some(calc) = &mut self.calc {
            calc.writing = Some((row, input, sub));
        }
        cx.notify();
    }

    fn calc_finish_writing(&mut self, keep: bool, cx: &mut Context<Self>) {
        let Some(calc) = &mut self.calc else { return };
        let Some((row, input, _)) = calc.writing.take() else {
            return;
        };
        let text = input.read(cx).text().trim().to_string();
        if keep {
            match row {
                Some(i) => {
                    if let Some(l) = calc.tape.lines.get_mut(i) {
                        l.note = text;
                    }
                }
                None if !text.is_empty() => {
                    // A title starts a section: above the picked row, else
                    // at the end.
                    let at = calc.selected.unwrap_or(calc.tape.lines.len());
                    calc.tape.lines.insert(at, Line::new(Row::Title(text)));
                }
                None => {}
            }
            calc.tape.retotal();
        }
        let input = calc.input.clone();
        self.pending_calc_focus = Some(input);
        cx.notify();
    }

    /// Ctrl+T in a note: the date and time where the cursor is.
    fn calc_stamp_time(&mut self, cx: &mut Context<Self>) {
        let Some((_, input, _)) = self.calc.as_ref().and_then(|c| c.writing.as_ref()) else {
            return;
        };
        let prefs = crate::prefs::get(cx);
        let now = chrono::Local::now();
        let stamp = format!(
            "{} {}",
            now.format(prefs.stamps.date_format()),
            now.format(prefs.stamps.time_format())
        );
        let input = input.clone();
        input.update(cx, |i, cx| {
            let text = format!("{}{stamp}", i.text());
            i.set_text(text, cx);
        });
    }

    fn scroll_tape_to_end(&mut self) {
        let Some(calc) = &mut self.calc else { return };
        let total_h = calc.tape.lines.len() as f32 * ROW_H + ROW_H;
        let view_h = f32::from(calc.scroll.bounds().size.height);
        let y = (total_h - view_h).max(0.0);
        calc.scroll.set_offset(gpui::point(px(0.0), px(-y)));
    }

    /// Place: on the next page clicked, or, for a tape being edited, where
    /// it is.
    pub(crate) fn calc_place_requested(&mut self, cx: &mut Context<Self>) {
        let Some(calc) = &mut self.calc else { return };
        if calc.tape.lines.is_empty() {
            return;
        }
        if let Some((page, id)) = calc.editing {
            return self.update_placed_tape(page, id, cx);
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
        if calc.tape.lines.is_empty() {
            return true;
        }
        let look = TapeLook {
            decimals: calc.decimals,
            ..Default::default()
        };
        let (w, h) = tape_size(&calc.tape, TAPE_PT, &look);
        let contents = calc.contents();
        let tape = calc.tape.clone();
        let mut a = self.new_annot(
            AnnotKind::Tape {
                rect: Box2 {
                    x: p.x,
                    y: p.y,
                    w: w.max(72.0),
                    h,
                },
                size: TAPE_PT,
                tape,
                look,
            },
            [20, 20, 20],
            cx,
        );
        a.contents = contents;
        self.add_annot(page, a, "Place tape", cx);
        true
    }

    /// Pick on: a click on a page, at `p` in PageSpace, enters the number
    /// under it. True when the click was taken.
    pub(crate) fn pick_number_at(
        &mut self,
        page: PageId,
        p: Pt,
        tolerance: f32,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.calc.as_ref().is_some_and(|c| c.picking) {
            return false;
        }
        self.with_chars(page, cx, move |v, chars, cx| {
            let hit = chars.and_then(|c| {
                let at = c.boundary_at(p, tolerance)?;
                let (w, _, r) = c
                    .words()
                    .into_iter()
                    .find(|(_, (a, b), _)| *a <= at && at <= *b)?;
                Some((sheaf_doc::tape::number_in(&w)?, r, c.upright))
            });
            match hit {
                Some((value, r, upright)) => v.picked(page, vec![(value, r)], upright, cx),
                None => v.set_notice("No number there", cx),
            }
        });
        true
    }

    /// Pick on, Shift+drag: every number in the region (`rect`, in
    /// PageSpace).
    pub(crate) fn pick_numbers_in(&mut self, page: PageId, rect: Box2, cx: &mut Context<Self>) {
        self.with_chars(page, cx, move |v, chars, cx| {
            let upright = chars.as_ref().map_or(Rotation::R0, |c| c.upright);
            let found: Vec<(f64, Box2)> = chars
                .map(|c| {
                    c.words()
                        .into_iter()
                        .filter(|(_, _, r)| {
                            rect.contains(Pt {
                                x: r.x + r.w / 2.0,
                                y: r.y + r.h / 2.0,
                            })
                        })
                        .filter_map(|(w, _, r)| Some((sheaf_doc::tape::number_in(&w)?, r)))
                        .collect()
                })
                .unwrap_or_default();
            if found.is_empty() {
                v.set_notice("No numbers there", cx);
            } else {
                v.picked(page, found, upright, cx);
            }
        });
    }

    /// Numbers picked on `page` (their boxes in PageSpace, the text reading
    /// upright turned `upright`): onto the tape, each with the tickmark
    /// after it as it reads, when there's one (as one undo step).
    fn picked(
        &mut self,
        page: PageId,
        numbers: Vec<(f64, Box2)>,
        upright: Rotation,
        cx: &mut Context<Self>,
    ) {
        self.ensure_calc(cx);
        let calc = self.calc.as_mut().unwrap();
        for (value, _) in &numbers {
            calc.tape.lines.push(Line::new(Row::Entry {
                value: *value,
                op: Op::Add,
            }));
        }
        calc.tape.retotal();
        let tick = calc.tick.clone();
        let decimals = calc.decimals;
        let n = numbers.len();
        let sum: f64 = numbers.iter().map(|(v, _)| v).sum();
        if let Some(glyph) = tick {
            // Laid out where the text reads upright, and turned to read
            // with it.
            let up = Affine::upright(upright);
            let edits: Vec<Edit> = numbers
                .iter()
                .map(|(_, r)| {
                    let r = up.apply_box(*r);
                    let h = (r.h * 1.2).max(10.0);
                    let w = crate::contrib::mark_width(&glyph, h);
                    let a = self.new_annot(
                        AnnotKind::Stamp {
                            rect: Box2 {
                                x: r.right() + 2.0,
                                y: r.y + r.h / 2.0 - h / 2.0,
                                w,
                                h,
                            },
                            glyph: glyph.clone(),
                        },
                        crate::markup_tools::color_of(crate::markup_tools::Tool::Tick, cx),
                        cx,
                    );
                    Edit::AnnotAdd {
                        page,
                        annot: Annot {
                            turn: upright,
                            ..a.mapped(&up.inverse())
                        },
                    }
                })
                .collect();
            self.apply_batch(edits, &format!("Tick {n} picked"), cx);
        }
        self.set_notice(
            &if n == 1 {
                format!("Picked {}", fmt_amount(sum, decimals))
            } else {
                format!("Picked {n} numbers: {}", fmt_amount(sum, decimals))
            },
            cx,
        );
        self.scroll_tape_to_end();
        cx.notify();
    }

    /// Edit Tape: a placed tape back in the calculator.
    pub(crate) fn edit_tape(&mut self, page: PageId, id: AnnotId, cx: &mut Context<Self>) {
        let annot = match &self.state {
            State::Ready(doc) => doc
                .model
                .state()
                .annots_on(page)
                .and_then(|l| l.iter().find(|a| a.id == id).cloned()),
            _ => None,
        };
        let Some(a) = annot else { return };
        let Some(tape) = a.tape() else { return };
        let decimals = match &a.kind {
            AnnotKind::Tape { look, .. } => look.decimals,
            _ => 2,
        };
        self.ensure_calc(cx);
        let calc = self.calc.as_mut().unwrap();
        calc.tape = tape;
        calc.decimals = decimals;
        calc.selected = None;
        calc.placing = false;
        calc.editing = Some((page, id));
        self.wanted_panel = Some("calc");
        if let State::Ready(doc) = &mut self.state {
            doc.notice = Some("Editing the tape: Update Tape puts it back".into());
        }
        cx.notify();
    }

    /// The tape being edited, written back over the placed one: new rows,
    /// the same place (its top-left as it reads), its size fitted to them.
    fn update_placed_tape(&mut self, page: PageId, id: AnnotId, cx: &mut Context<Self>) {
        let Some(calc) = &mut self.calc else { return };
        let (tape, decimals, contents) = (calc.tape.clone(), calc.decimals, calc.contents());
        calc.editing = None;
        let annot = match &self.state {
            State::Ready(doc) => doc
                .model
                .state()
                .annots_on(page)
                .and_then(|l| l.iter().find(|a| a.id == id).cloned()),
            _ => None,
        };
        let Some(mut a) = annot else {
            return self.set_notice("That tape is gone", cx);
        };
        // Sized as it reads, in the frame it reads upright in.
        let upright = a.upright();
        let AnnotKind::Tape {
            rect, size, look, ..
        } = &upright
        else {
            return;
        };
        let look = TapeLook {
            decimals,
            ..look.clone()
        };
        let (w, h) = tape_size(&tape, *size, &look);
        let (w, h) = if look.rotation % 180 == 90 {
            (h, w)
        } else {
            (w.max(72.0), h)
        };
        a.kind = AnnotKind::Tape {
            rect: Box2 { w, h, ..*rect },
            size: *size,
            tape,
            look,
        }
        .mapped(&Affine::upright(a.turn).inverse());
        a.contents = contents;
        a.modified = sheaf_doc::pdf_date_now();
        self.apply(Edit::AnnotUpdate { page, annot: a }, "Update tape", cx);
    }

    /// The Calculator panel. `wide`: it sits in a bottom dock, so the keys
    /// go in a column on the left with the tape beside them.
    pub fn render_calc(
        &mut self,
        wide: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.ensure_calc(cx);
        if let Some(input) = self.pending_calc_focus.take() {
            window.focus(&input.read(cx).focus_handle(cx), cx);
        }
        let calc = self.calc.as_ref().unwrap();
        let input = calc.input.clone();
        let decimals = calc.decimals;

        let key = |id: &'static str, label: &'static str, wide_key: bool| {
            div()
                .id(id)
                .h(px(26.0))
                .when(wide_key, |d| d.px_2())
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
        let k = |id: &'static str, label: &'static str, wide_key: bool, what: Key| {
            key(id, label, wide_key)
                .on_click(cx.listener(move |v, _, _, cx| v.calc_key(what, cx)))
                .into_any_element()
        };
        let ops = vec![
            k("k-add", "+", false, Key::Op(Op::Add)),
            k("k-sub", "−", false, Key::Op(Op::Sub)),
            k("k-mul", "×", false, Key::Op(Op::Mul)),
            k("k-div", "÷", false, Key::Op(Op::Div)),
            k("k-sub-total", "◊", false, Key::Subtotal),
            k("k-total", "*", false, Key::Total),
            k("k-gt", "GT", false, Key::GrandTotal),
        ];
        let entry_keys = vec![
            k("k-00", "00", false, Key::Zeros(2)),
            k("k-000", "000", false, Key::Zeros(3)),
            k("k-sign", "±", false, Key::Sign),
            k("k-ce", "C", false, Key::ClearEntry),
            k("k-dec", "D", false, Key::Decimals),
        ];
        let memory = vec![
            k("k-madd", "M+", false, Key::MemAdd),
            k("k-msub", "M−", false, Key::MemSub),
            k("k-mr", "MR", false, Key::MemRecall),
            k("k-mc", "MC", false, Key::MemClear),
            k("k-ms", "MS", false, Key::MemStore),
        ];
        let edits = vec![
            key("k-ins", "Insert", true)
                .on_click(cx.listener(|v, _, _, cx| v.calc_insert(cx)))
                .into_any_element(),
            key("k-del", "Delete", true)
                .on_click(cx.listener(|v, _, _, cx| v.calc_delete(cx)))
                .into_any_element(),
            key("k-title", "Title…", true)
                .on_click(cx.listener(|v, _, window, cx| v.calc_start_writing(None, window, cx)))
                .into_any_element(),
            key("k-clear", "Clear", true)
                .on_click(cx.listener(|v, _, _, cx| v.calc_clear(cx)))
                .into_any_element(),
        ];

        let n = calc.tape.lines.len();
        let open = calc.tape.open();
        let running = calc.tape.running();
        let mut status = if n == 0 {
            "Amount, then + − x /; Space subtotals, Enter totals".to_string()
        } else if open {
            format!("running {}", fmt_amount(running, decimals))
        } else {
            format!("{n} rows")
        };
        if calc.memory != 0.0 {
            status.push_str(&format!("  ·  M {}", fmt_amount(calc.memory, decimals)));
        }
        if decimals != 2 {
            status.push_str(&format!("  ·  {decimals} places"));
        }
        let placing = calc.placing;
        let editing = calc.editing.is_some();
        let place = key(
            "place",
            if placing {
                "Click a page…"
            } else if editing {
                "Update Tape"
            } else {
                "Place on Page"
            },
            true,
        )
        .when(placing, |d| d.bg(theme::ACCENT))
        .on_click(cx.listener(|v, _, _, cx| v.calc_place_requested(cx)));
        let writing = calc.writing.as_ref().map(|(_, i, _)| i.clone());
        // Pick, and the tickmark picked numbers get.
        let (picking, tick) = (calc.picking, calc.tick.clone());
        let marks: Vec<String> = cx
            .global::<crate::contrib::Contributions>()
            .toolbar_marks()
            .into_iter()
            .take(6)
            .collect();
        let chip = |id: (&'static str, usize), label: String, on: bool| {
            div()
                .id(id)
                .px_2()
                .h(px(22.0))
                .flex()
                .items_center()
                .rounded_sm()
                .cursor_pointer()
                .text_xs()
                .bg(if on { theme::ACCENT } else { theme::CONTROL })
                .hover(|s| s.bg(theme::CONTROL_HOVER))
                .child(label)
        };
        let pick_row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .child(
                chip(("pick", 0), "Pick from page".into(), picking).on_click(cx.listener(
                    |v, _, _, cx| {
                        if let Some(c) = &mut v.calc {
                            c.picking = !c.picking;
                        }
                        let msg = if v.calc.as_ref().is_some_and(|c| c.picking) {
                            "Click a number to add it; Shift+drag adds all in a box"
                        } else {
                            "Picking off"
                        };
                        v.set_notice(msg, cx);
                    },
                )),
            )
            .when(picking, |d| {
                d.child(div().text_xs().text_color(theme::TEXT_MUTED).child("tick"))
                    .child(
                        chip(("pick-tick", 0), "none".into(), tick.is_none()).on_click(
                            cx.listener(|v, _, _, cx| {
                                if let Some(c) = &mut v.calc {
                                    c.tick = None;
                                }
                                cx.notify();
                            }),
                        ),
                    )
                    .children(marks.into_iter().enumerate().map(|(i, m)| {
                        let on = tick.as_deref() == Some(m.as_str());
                        chip(("pick-tick", i + 1), m.clone(), on).on_click(cx.listener(
                            move |v, _, _, cx| {
                                if let Some(c) = &mut v.calc {
                                    c.tick = Some(m.clone());
                                }
                                cx.notify();
                            },
                        ))
                    }))
            });

        // Tape rows: number, amount (or title), symbol, note.
        let numbers = calc.tape.numbers();
        let selected = calc.selected;
        let mut rows: Vec<AnyElement> = calc
            .tape
            .lines
            .iter()
            .zip(numbers)
            .enumerate()
            .map(|(i, (l, number))| {
                let total = l.row.is_total();
                let body: AnyElement = match &l.row {
                    Row::Title(t) => div()
                        .flex_1()
                        .font_weight(gpui::FontWeight::BOLD)
                        .child(t.clone())
                        .into_any_element(),
                    row => div()
                        .flex_1()
                        .flex()
                        .justify_end()
                        .child(fmt_amount(row.value().unwrap_or(0.0), decimals))
                        .into_any_element(),
                };
                let symbol = l.row.symbol();
                div()
                    .id(("tape-row", i))
                    .absolute()
                    .top(px(i as f32 * ROW_H))
                    .left(px(0.0))
                    .w_full()
                    .h(px(ROW_H))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .text_sm()
                    .cursor_pointer()
                    .text_color(theme::TEXT)
                    .when(selected == Some(i), |d| d.bg(theme::SELECTED))
                    .hover(|s| s.bg(theme::HOVER))
                    .when(total, |d| {
                        d.border_t_1()
                            .border_color(theme::TOTAL_RULE)
                            .font_weight(gpui::FontWeight::BOLD)
                    })
                    .on_click(cx.listener(move |v, ev: &gpui::ClickEvent, window, cx| {
                        if let Some(calc) = &mut v.calc {
                            calc.selected = Some(i);
                        }
                        if ev.click_count() >= 2 {
                            v.calc_start_writing(Some(i), window, cx);
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(32.0))
                            .text_xs()
                            .text_color(theme::TEXT_DIM)
                            .child(number),
                    )
                    .child(body)
                    .child(
                        div()
                            .w(px(22.0))
                            .text_color(theme::TEXT_MUTED)
                            .child(symbol),
                    )
                    .when(!l.note.is_empty(), |d| {
                        d.child(
                            div()
                                .max_w(px(160.0))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_xs()
                                .text_color(theme::TEXT_MUTED)
                                .child(l.note.clone()),
                        )
                    })
                    .into_any_element()
            })
            .collect();
        if open {
            rows.push(
                div()
                    .absolute()
                    .top(px(n as f32 * ROW_H))
                    .left(px(0.0))
                    .w_full()
                    .h(px(ROW_H))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .text_sm()
                    .text_color(theme::TEXT_DIM)
                    .child(div().w(px(32.0)))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .justify_end()
                            .child(fmt_amount(running, decimals)),
                    )
                    .child(div().w(px(22.0)).child("="))
                    .into_any_element(),
            );
        }
        let total_h = (n + 1) as f32 * ROW_H;

        let group = |items: Vec<AnyElement>| div().flex().flex_wrap().gap_1().children(items);
        let controls = div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .child(
                div()
                    .on_key_down(cx.listener(|v, ev: &KeyDownEvent, _, cx| {
                        // Shift+Enter: the grand total.
                        let ks = &ev.keystroke;
                        if ks.key == "enter" && ks.modifiers.shift {
                            v.calc_key(Key::GrandTotal, cx);
                            cx.stop_propagation();
                        }
                    }))
                    .child(input),
            )
            .child(group(ops))
            .child(group(entry_keys))
            .child(group(memory))
            .child(group(edits))
            .child(div().flex().child(place))
            .child(pick_row)
            .children(writing.map(|w| {
                div()
                    .on_key_down(cx.listener(|v, ev: &KeyDownEvent, _, cx| {
                        let ks = &ev.keystroke;
                        if ks.modifiers.control && ks.key == "t" {
                            v.calc_stamp_time(cx);
                            cx.stop_propagation();
                        }
                    }))
                    .child(w)
            }))
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
                        .w(px(300.0))
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

/// A key of the calculator.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    Op(Op),
    Subtotal,
    Total,
    GrandTotal,
    Sign,
    ClearEntry,
    Decimals,
    Zeros(usize),
    MemAdd,
    MemSub,
    MemRecall,
    MemClear,
    MemStore,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_format_and_parse() {
        assert_eq!(fmt_amount(0.0, 2), "0.00");
        assert_eq!(fmt_amount(1234.5, 2), "1,234.50");
        assert_eq!(parse_amount("$1,234.50"), Some(1234.5));
        assert_eq!(parse_amount("-7"), Some(-7.0));
        assert_eq!(parse_amount("abc"), None);
    }
}
