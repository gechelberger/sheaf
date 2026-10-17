//! Drawing polygons, polylines and clouds a vertex at a time (and the
//! Perimeter and Area measurements, see `measure_tools`), and placing
//! callouts. Click each vertex; double-click or Enter finishes, Backspace
//! takes the last one off, Esc drops the shape. The Cloud tool (Q) draws a
//! polygon with a cloudy border. A callout is a press where it points and
//! a drag to where its text goes.

use gpui::{AnyElement, Context, prelude::*};
use sheaf_core::Rotation;
use sheaf_doc::geom::Geom;
use sheaf_doc::{AnnotKind, Box2, PageId, Pt};

use crate::docview::{DocView, State};
use crate::markup_tools::{Tool, stroke_canvas};

/// A shape being drawn: its page and the vertices so far, in its edit
/// frame.
pub struct PolyDraft {
    pub page: usize,
    pub page_id: PageId,
    pub points: Vec<Pt>,
    /// Where the pointer is, for the line to it.
    pub hover: Option<Pt>,
    /// A measurement being drawn: what it measures.
    pub measure: Option<sheaf_doc::measure::Measured>,
}

/// A callout's text box, from its size.
const CALLOUT_W: f32 = 150.0;
const CALLOUT_H: f32 = 36.0;

impl DocView {
    /// A click with a shape tool: a vertex (a double-click finishes).
    pub(crate) fn add_vertex(
        &mut self,
        page: usize,
        page_id: PageId,
        p: Pt,
        clicks: usize,
        cx: &mut Context<Self>,
    ) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        if clicks >= 2 {
            self.finish_shape(cx);
            return;
        }
        let measure = self.tool.measures();
        // A click on another page starts over there.
        let draft = doc.poly.get_or_insert(PolyDraft {
            page,
            page_id,
            points: Vec::new(),
            hover: None,
            measure,
        });
        if draft.page_id != page_id {
            *draft = PolyDraft {
                page,
                page_id,
                points: Vec::new(),
                hover: None,
                measure,
            };
        }
        draft.points.push(p);
        let n = draft.points.len();
        if let Some(what) = measure
            && n > 1
        {
            let so_far = draft.points.clone();
            self.measure_notice(what, &so_far, cx);
            return;
        }
        doc.notice = Some(match n {
            1 => "Click the next corner; double-click or Enter finishes, Backspace takes one off"
                .into(),
            n => format!("{n} corners"),
        });
        cx.notify();
    }

    /// The pointer moved over a page with no button down: the line to it
    /// follows, and a measurement shows what it would come to.
    pub(crate) fn shape_hover(
        &mut self,
        page: usize,
        p: Pt,
        shift: bool,
        k: f32,
        cx: &mut Context<Self>,
    ) {
        let (last, measure) = match &self.state {
            State::Ready(doc) => match &doc.poly {
                Some(d) if d.page == page => (d.points.last().copied(), d.measure),
                _ => return,
            },
            _ => return,
        };
        let p = match measure {
            Some(_) => self.measure_point(page, p, last, shift, k),
            None => p,
        };
        let mut so_far = Vec::new();
        if let State::Ready(doc) = &mut self.state
            && let Some(d) = &mut doc.poly
        {
            d.hover = Some(p);
            so_far = d.points.iter().copied().chain([p]).collect();
        }
        if let Some(what) = measure {
            self.measure_notice(what, &so_far, cx);
        }
        cx.notify();
    }

    /// Backspace: the last vertex off. False when no shape is being drawn.
    pub(crate) fn undo_vertex(&mut self, cx: &mut Context<Self>) -> bool {
        let State::Ready(doc) = &mut self.state else {
            return false;
        };
        let Some(d) = &mut doc.poly else {
            return false;
        };
        d.points.pop();
        if d.points.is_empty() {
            doc.poly = None;
        }
        cx.notify();
        true
    }

    /// Esc: drop the shape being drawn. False when there's none.
    pub(crate) fn cancel_shape(&mut self, cx: &mut Context<Self>) -> bool {
        let State::Ready(doc) = &mut self.state else {
            return false;
        };
        if doc.poly.take().is_some() {
            doc.notice = Some("Shape dropped".into());
            cx.notify();
            true
        } else {
            false
        }
    }

    /// Double-click or Enter: the shape drawn so far becomes a mark (a
    /// polygon needs three corners, a line two). False when none is being
    /// drawn.
    pub(crate) fn finish_shape(&mut self, cx: &mut Context<Self>) -> bool {
        let tool = self.tool;
        let State::Ready(doc) = &mut self.state else {
            return false;
        };
        let Some(d) = doc.poly.take() else {
            return false;
        };
        // A double-click's two clicks land on one spot.
        let mut points: Vec<Pt> = Vec::new();
        for p in d.points {
            if points
                .last()
                .is_none_or(|q| (q.x - p.x).abs() > 1.0 || (q.y - p.y).abs() > 1.0)
            {
                points.push(p);
            }
        }
        let closed = !matches!(tool, Tool::Polyline | Tool::Perimeter);
        if points.len() < if closed { 3 } else { 2 } {
            doc.notice = Some("Too few corners for a shape".into());
            cx.notify();
            return true;
        }
        let scale = Box::new(doc.measure_scale.clone());
        let kind = match (d.measure, closed) {
            (Some(what), _) => AnnotKind::Measure {
                what,
                points,
                scale,
            },
            (None, true) => AnnotKind::Polygon { points },
            (None, false) => AnnotKind::Polyline { points },
        };
        let mut a = self.new_annot(kind, crate::markup_tools::color_of(tool, cx), cx);
        a.cloudy = tool == Tool::Cloud;
        self.add_annot(d.page_id, a, tool.label(), cx);
        true
    }

    /// Callout tool: pointing at `point`, its text at `to`; the box goes on
    /// the far side of `to`, and its text is typed next.
    pub(crate) fn place_callout(
        &mut self,
        page: PageId,
        point: Pt,
        to: Pt,
        cx: &mut Context<Self>,
    ) {
        // A click with no drag points up and right of itself.
        let to = if (to.x - point.x).abs() + (to.y - point.y).abs() < 6.0 {
            Pt {
                x: point.x + 50.0,
                y: point.y - 50.0,
            }
        } else {
            to
        };
        let x = if to.x >= point.x {
            to.x + 20.0
        } else {
            to.x - 20.0 - CALLOUT_W
        };
        let kind = AnnotKind::Callout {
            rect: Box2 {
                x,
                y: to.y - CALLOUT_H / 2.0,
                w: CALLOUT_W,
                h: CALLOUT_H,
            },
            knee: to,
            point,
            size: 12.0,
        };
        let a = self.new_annot(kind, crate::markup_tools::color_of(Tool::Callout, cx), cx);
        self.add_annot(page, a, "Callout", cx);
        self.pending_edit_last = Some(page);
    }

    /// Shift+D: the next drawing tool.
    pub(crate) fn cycle_drawing_tools(&mut self, cx: &mut Context<Self>) {
        const ORDER: [Tool; 8] = [
            Tool::Square,
            Tool::Ellipse,
            Tool::Line,
            Tool::Arrow,
            Tool::Polyline,
            Tool::Polygon,
            Tool::Cloud,
            Tool::Ink,
        ];
        let next = ORDER
            .iter()
            .position(|t| *t == self.tool)
            .map_or(ORDER[0], |i| ORDER[(i + 1) % ORDER.len()]);
        self.set_tool(next, cx);
        self.set_notice(next.label(), cx);
    }

    /// Shift+X: text box, then callout.
    pub(crate) fn cycle_text_tools(&mut self, cx: &mut Context<Self>) {
        let next = if self.tool == Tool::FreeText {
            Tool::Callout
        } else {
            Tool::FreeText
        };
        self.set_tool(next, cx);
        self.set_notice(next.label(), cx);
    }
}

/// The shape being drawn on `page`: its lines so far, and one on to the
/// pointer. It's in the edit frame of `edit`, and the page is drawn as
/// `pane` shows it, at `k` px a point.
pub(crate) fn draft_preview(
    doc: &crate::docview::Doc,
    color: gpui::Rgba,
    page: usize,
    edit: &Geom,
    pane: &Geom,
    k: f32,
) -> Option<AnyElement> {
    let d = doc.poly.as_ref()?;
    if d.page != page {
        return None;
    }
    let (to_pane, size) = (pane.to_view(), pane.shown());
    if let Some(what) = d.measure {
        let points: Vec<Pt> = d.points.iter().chain(d.hover.iter()).copied().collect();
        let drawn = sheaf_doc::Annot {
            id: sheaf_doc::AnnotId(0),
            kind: AnnotKind::Measure {
                what,
                points,
                scale: Box::new(doc.measure_scale.clone()),
            },
            color: [
                (color.r * 255.0) as u8,
                (color.g * 255.0) as u8,
                (color.b * 255.0) as u8,
            ],
            opacity: 1.0,
            stroke: 1.0,
            author: String::new(),
            contents: String::new(),
            modified: String::new(),
            created: String::new(),
            fill: None,
            dashed: true,
            cloudy: false,
            turn: Rotation::R0,
            name: String::new(),
        };
        let m = drawn.to_page(edit);
        return Some(
            gpui::div()
                .absolute()
                .inset_0()
                .children(crate::docview::annot_elements(
                    &m,
                    doc.model.assets(),
                    &to_pane,
                    size,
                    k,
                ))
                .into_any_element(),
        );
    }
    let from_edit = edit.from_view().then(to_pane);
    let pts: Vec<(f32, f32)> = d
        .points
        .iter()
        .chain(d.hover.iter())
        .map(|p| {
            let q = from_edit.apply(*p);
            (q.x * k, q.y * k)
        })
        .collect();
    Some(stroke_canvas(vec![pts], color, 1.5 * k, size, k))
}
