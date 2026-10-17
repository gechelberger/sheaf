//! The Measure tools (M9 slice 19): Distance (drag), and Perimeter and
//! Area (click the corners; double-click or Enter finishes, as Polyline
//! and Polygon do), at the document's scale.
//!
//! The scale bar under the toolbar shows the scale. Type another ("1 in =
//! 10 ft", "1/8 in = 1 ft", "1:100") and press Enter, pick the decimals,
//! or calibrate: draw a line of known length, then type what it is.
//! While drawing, the status bar shows the measurement so far. Shift
//! keeps a segment to 45°; with the grid or guides shown, points snap to
//! them.

use gpui::{AnyElement, Context, Entity, Focusable, Subscription, Window, div, prelude::*, px};
use sheaf_doc::measure::{self, Measured, Scale};
use sheaf_doc::{AnnotKind, PageId, Pt};
use sheaf_ui::text_input::{TextInput, TextInputEvent};
use sheaf_ui::theme;

use crate::docview::{DocView, State};
use crate::markup_tools::Tool;

/// Where calibrating is up to.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Calibrate {
    #[default]
    Off,
    /// The next Distance drag is the known length.
    Draw,
    /// A line this many points long was drawn: what is it really?
    Ask(f64),
}

/// The scale bar's text box.
pub struct MeasureBar {
    pub input: Entity<TextInput>,
    pub _sub: Subscription,
}

/// How near a point must be to a grid line or guide to snap, in screen
/// pixels.
const SNAP_PX: f32 = 6.0;

impl Tool {
    /// The Measure tools, and what each measures.
    pub fn measures(self) -> Option<Measured> {
        match self {
            Tool::Distance => Some(Measured::Distance),
            Tool::Perimeter => Some(Measured::Perimeter),
            Tool::Area => Some(Measured::Area),
            _ => None,
        }
    }
}

impl DocView {
    /// The document's scale, for new measurements.
    pub(crate) fn measure_scale(&self) -> Scale {
        match &self.state {
            State::Ready(doc) => doc.measure_scale.clone(),
            _ => Scale::default(),
        }
    }

    /// `p` on display page `page` (in its edit frame) where a measurement
    /// takes it: at 45° from `from` with Shift, else onto a grid line or
    /// guide when one is shown and near.
    pub(crate) fn measure_point(
        &self,
        page: usize,
        p: Pt,
        from: Option<Pt>,
        shift: bool,
        k: f32,
    ) -> Pt {
        if shift && let Some(from) = from {
            return measure::at_45(from, p);
        }
        let State::Ready(doc) = &self.state else {
            return p;
        };
        if !self.grid && self.guides.is_empty() {
            return p;
        }
        let (Some(edit), Some(pane)) = (doc.edit_geom(page), doc.pane_geom(page)) else {
            return p;
        };
        // Grid and guides are on the page as the pane shows it.
        let to_pane = edit.from_view().then(pane.to_view());
        let mut q = to_pane.apply(p);
        let near = SNAP_PX / k.max(0.01);
        let snap = |v: f32, to: f32| if (v - to).abs() <= near { to } else { v };
        if self.grid {
            // As the grid is drawn: inches, quarters when they show.
            let step = if 72.0 * k / 4.0 >= 8.0 { 18.0 } else { 72.0 };
            q.x = snap(q.x, (q.x / step).round() * step);
            q.y = snap(q.y, (q.y / step).round() * step);
        }
        for g in &self.guides {
            if g.horizontal {
                q.y = snap(q.y, g.at);
            } else {
                q.x = snap(q.x, g.at);
            }
        }
        to_pane.inverse().apply(q)
    }

    /// The running measurement, in the status bar.
    pub(crate) fn measure_notice(&mut self, what: Measured, points: &[Pt], cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let value = measure::label(what, points, &doc.measure_scale);
        doc.notice = Some(match doc.calibrate {
            Calibrate::Draw => format!(
                "Calibrating: {:.1} pt drawn",
                measure::length(points, false)
            ),
            _ => format!("{} {value}  ({})", what.label(), doc.measure_scale.ratio()),
        });
        cx.notify();
    }

    /// A Distance drag let go at `b` from `a`: a measurement, or the
    /// calibration line.
    pub(crate) fn finish_distance(&mut self, page: PageId, a: Pt, b: Pt, cx: &mut Context<Self>) {
        let length = measure::length(&[a, b], false);
        if length < 2.0 {
            return;
        }
        let calibrating =
            matches!(&self.state, State::Ready(doc) if doc.calibrate == Calibrate::Draw);
        if calibrating {
            if let State::Ready(doc) = &mut self.state {
                doc.calibrate = Calibrate::Ask(length);
                doc.notice = Some(
                    "How long is that line? Type it in the scale bar (\"25 ft\"), then Enter"
                        .into(),
                );
            }
            self.ensure_measure_bar(cx);
            let unit = self.measure_scale().unit;
            if let Some(bar) = &self.measure_bar {
                bar.input.update(cx, |t, cx| {
                    t.set_text(format!("1 {unit}"), cx);
                    t.select_all(cx);
                });
            }
            self.pending_measure_focus = true;
            cx.notify();
            return;
        }
        let a = self.new_annot(
            AnnotKind::Measure {
                what: Measured::Distance,
                points: vec![a, b],
                scale: Box::new(self.measure_scale()),
            },
            crate::markup_tools::color_of(Tool::Distance, cx),
            cx,
        );
        self.add_annot(page, a, "Distance", cx);
    }

    /// Tools > Measure > Calibrate Scale: the next line drawn is a known
    /// length.
    pub(crate) fn start_calibrate(&mut self, cx: &mut Context<Self>) {
        self.set_tool(Tool::Distance, cx);
        if let State::Ready(doc) = &mut self.state {
            doc.calibrate = Calibrate::Draw;
            doc.notice = Some("Calibrate: draw a line along something of known length".into());
        }
        cx.notify();
    }

    fn ensure_measure_bar(&mut self, cx: &mut Context<Self>) {
        if self.measure_bar.is_some() {
            return;
        }
        let input = cx.new(|cx| TextInput::new("1 in = 10 ft", cx));
        let sub = cx.subscribe(&input, |this, _, ev: &TextInputEvent, cx| match ev {
            TextInputEvent::Confirm => this.apply_scale_text(cx),
            TextInputEvent::Cancel => {
                if let State::Ready(doc) = &mut this.state {
                    doc.calibrate = Calibrate::Off;
                }
                this.show_scale(cx);
                this.pending_focus = true;
                cx.notify();
            }
        });
        let ratio = self.measure_scale().ratio();
        input.update(cx, |t, cx| t.set_text(ratio, cx));
        self.measure_bar = Some(MeasureBar { input, _sub: sub });
    }

    /// The bar's text back to the scale.
    fn show_scale(&mut self, cx: &mut Context<Self>) {
        let ratio = self.measure_scale().ratio();
        if let Some(bar) = &self.measure_bar {
            bar.input.update(cx, |t, cx| t.set_text(ratio, cx));
        }
    }

    /// Enter in the scale bar: the scale typed, or the calibration line's
    /// real length.
    fn apply_scale_text(&mut self, cx: &mut Context<Self>) {
        let Some(text) = self
            .measure_bar
            .as_ref()
            .map(|b| b.input.read(cx).text().to_string())
        else {
            return;
        };
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let old = doc.measure_scale.clone();
        let next = match doc.calibrate {
            Calibrate::Ask(points) => match measure::parse_length(&text) {
                Some((real, unit)) => Some(Scale::calibrated(
                    points,
                    real,
                    unit.unwrap_or(&old.unit),
                    old.precision,
                )),
                None => {
                    doc.notice = Some("Type the line's real length, like 25 ft".into());
                    cx.notify();
                    return;
                }
            },
            _ => measure::parse_scale(&text, &old.unit, old.precision),
        };
        match next {
            Some(scale) => {
                doc.notice = Some(format!("Scale {}", scale.ratio()));
                doc.calibrate = Calibrate::Off;
                self.set_measure_scale(scale, cx);
                self.pending_focus = true;
            }
            None => {
                doc.notice = Some("Type a scale like 1 in = 10 ft, 1/8 in = 1 ft or 1:100".into());
            }
        }
        cx.notify();
    }

    /// Use `scale` for new measurements here, and remember it for the next
    /// document.
    pub(crate) fn set_measure_scale(&mut self, scale: Scale, cx: &mut Context<Self>) {
        if let State::Ready(doc) = &mut self.state {
            doc.measure_scale = scale.clone();
        }
        crate::state::update(cx, |s| s.measure_scale = Some(scale));
        self.show_scale(cx);
        cx.notify();
    }

    /// The scale bar, under the toolbar while a Measure tool is on.
    pub(crate) fn render_measure_bar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let calibrate = match &self.state {
            State::Ready(doc) => doc.calibrate,
            _ => return None,
        };
        if self.tool.measures().is_none() && calibrate == Calibrate::Off {
            return None;
        }
        self.ensure_measure_bar(cx);
        let bar = self.measure_bar.as_ref()?;
        if std::mem::take(&mut self.pending_measure_focus) {
            let focus = bar.input.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
        }
        let input = bar.input.clone();
        let scale = self.measure_scale();
        let chip = |id: (&'static str, usize), text: String, on: bool| {
            div()
                .id(id)
                .px_2()
                .h(px(20.0))
                .flex()
                .items_center()
                .rounded_sm()
                .cursor_pointer()
                .bg(if on { theme::ACCENT } else { theme::CONTROL })
                .hover(|s| s.bg(theme::CONTROL_HOVER))
                .child(text)
        };
        let (title, hint) = match calibrate {
            Calibrate::Ask(points) => (
                "Real length".to_string(),
                format!("of the {:.1} pt line, then Enter; Esc stops", points),
            ),
            Calibrate::Draw => (
                "Calibrate".to_string(),
                "draw a line along something of known length".to_string(),
            ),
            Calibrate::Off => ("Scale".to_string(), "type one, then Enter".to_string()),
        };
        let decimals = (0..=4u8).map(|d| {
            chip(
                ("measure-decimals", d as usize),
                d.to_string(),
                scale.precision == d,
            )
            .on_click(cx.listener(move |v, _, _, cx| {
                let mut s = v.measure_scale();
                s.precision = d;
                v.set_measure_scale(s, cx);
            }))
        });
        Some(
            div()
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .py_1()
                .bg(theme::PANEL)
                .border_b_1()
                .border_color(theme::DIVIDER)
                .text_xs()
                .text_color(theme::TEXT)
                .child(div().text_color(theme::TEXT_MUTED).child(title))
                .child(div().w(px(150.0)).child(input))
                .child(div().text_color(theme::TEXT_MUTED).child(hint))
                .child(div().pl_2().text_color(theme::TEXT_MUTED).child("Decimals"))
                .children(decimals)
                .child(
                    div().pl_2().child(
                        chip(
                            ("measure-calibrate", 0),
                            "Calibrate  Ctrl+Shift+C".into(),
                            calibrate != Calibrate::Off,
                        )
                        .on_click(cx.listener(|v, _, _, cx| v.start_calibrate(cx))),
                    ),
                )
                .into_any_element(),
        )
    }
}
