//! Markup tools: a tool mode, mouse handling on the page canvas, live
//! previews while dragging, selection/move/delete of our annotations, and
//! path-based drawing for lines and ink.
//!
//! Coordinates (M10.5 §5): the page container reports window positions;
//! those become page-local logical px, then points on the page as the pane
//! shows it, then PageSpace, which the model stores. Gestures work in the
//! page's edit frame (see `Doc::edit_geom`): the page as it shows without
//! rotate view, y down, so a drag reads the same in a turned pane. A mark
//! built or reshaped there reaches the model through [`Annot::to_page`],
//! once; [`Annot::to_frame`] brings one back into the frame to be moved,
//! reshaped or copied.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use gpui::{
    AnyElement, Context, CursorStyle, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PathBuilder, Pixels, Point, Window, canvas, div, prelude::*, px, rgb,
};
use sheaf_core::{PageSize, Rotation};
use sheaf_doc::assets::MarkAssets;
use sheaf_doc::geom::{Affine, Geom};
use sheaf_doc::{Annot, AnnotId, AnnotKind, Box2, Edit, PageId, Pt};
use sheaf_ui::theme;
use sheaf_ui::tooltip::Tooltip;

use crate::docview::{DocView, State};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Select,
    Note,
    Highlight,
    Underline,
    StrikeOut,
    Squiggly,
    /// Text Edits: replace selected text, or insert at a click.
    TextEdit,
    FreeText,
    Square,
    Ellipse,
    Line,
    Arrow,
    Ink,
    Tick,
    /// A stamp from the Stamps panel.
    Stamp,
    Polygon,
    Polyline,
    /// A polygon with a cloudy border.
    Cloud,
    /// A text box with a leader line.
    Callout,
    /// A file, behind an icon.
    AttachFile,
    /// PDFlyer connector: two clicks place a linked pair of markers.
    Connector,
    /// Measure between two points (a drag).
    Distance,
    /// Measure along a path (corner by corner).
    Perimeter,
    /// Measure inside a shape (corner by corner).
    Area,
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Note => "Note",
            Tool::Highlight => "Highlight",
            Tool::Underline => "Underline",
            Tool::StrikeOut => "Strikethrough",
            Tool::Squiggly => "Squiggly",
            Tool::TextEdit => "Text Edits",
            Tool::FreeText => "Text",
            Tool::Square => "Rect",
            Tool::Ellipse => "Ellipse",
            Tool::Line => "Line",
            Tool::Arrow => "Arrow",
            Tool::Ink => "Pencil",
            Tool::Tick => "Tick",
            Tool::Stamp => "Stamp",
            Tool::Polygon => "Polygon",
            Tool::Polyline => "Polyline",
            Tool::Cloud => "Cloud",
            Tool::Callout => "Callout",
            Tool::AttachFile => "Attach File",
            Tool::Connector => "Connector",
            Tool::Distance => "Distance",
            Tool::Perimeter => "Perimeter",
            Tool::Area => "Area",
        }
    }

    /// Its name in `state.json`'s `tool_colors`.
    pub fn id(self) -> &'static str {
        match self {
            Tool::Select => "select",
            Tool::Note => "note",
            Tool::Highlight => "highlight",
            Tool::Underline => "underline",
            Tool::StrikeOut => "strikeout",
            Tool::Squiggly => "squiggly",
            Tool::TextEdit => "text-edit",
            Tool::FreeText => "free-text",
            Tool::Square => "square",
            Tool::Ellipse => "ellipse",
            Tool::Line => "line",
            Tool::Arrow => "arrow",
            Tool::Ink => "ink",
            Tool::Tick => "tick",
            Tool::Stamp => "stamp",
            Tool::Polygon => "polygon",
            Tool::Polyline => "polyline",
            Tool::Cloud => "cloud",
            Tool::Callout => "callout",
            Tool::AttachFile => "attach-file",
            Tool::Connector => "connector",
            Tool::Distance => "distance",
            Tool::Perimeter => "perimeter",
            Tool::Area => "area",
        }
    }

    /// What it draws in until the user picks another color for it, as
    /// Acrobat starts its tools: yellow notes and highlights, red
    /// strikethroughs, drawings and tickmarks, blue text edits.
    pub fn default_color(self) -> [u8; 3] {
        const YELLOW: [u8; 3] = [255, 230, 0];
        const RED: [u8; 3] = [220, 40, 40];
        const BLUE: [u8; 3] = [0, 120, 255];
        const GREEN: [u8; 3] = [0, 150, 0];
        const ORANGE: [u8; 3] = [255, 140, 0];
        match self {
            Tool::Select | Tool::Note | Tool::Highlight => YELLOW,
            Tool::Underline => GREEN,
            Tool::Squiggly => ORANGE,
            Tool::TextEdit | Tool::AttachFile | Tool::Connector => BLUE,
            Tool::StrikeOut
            | Tool::FreeText
            | Tool::Square
            | Tool::Ellipse
            | Tool::Line
            | Tool::Arrow
            | Tool::Ink
            | Tool::Tick
            | Tool::Stamp
            | Tool::Polygon
            | Tool::Polyline
            | Tool::Cloud
            | Tool::Callout
            | Tool::Distance
            | Tool::Perimeter
            | Tool::Area => RED,
        }
    }

    /// Tools that mark up text: a press on text selects it (as the Select
    /// tool does), and letting go marks the selection up.
    pub fn marks_text(self) -> bool {
        matches!(
            self,
            Tool::Highlight | Tool::Underline | Tool::StrikeOut | Tool::Squiggly | Tool::TextEdit
        )
    }

    /// Shift+U: the next of the text markup tools.
    pub fn next_text_markup(self) -> Tool {
        match self {
            Tool::Highlight => Tool::Underline,
            Tool::Underline => Tool::StrikeOut,
            Tool::StrikeOut => Tool::Squiggly,
            _ => Tool::Highlight,
        }
    }

    /// The action that selects this tool; its binding is the shortcut shown.
    pub fn action(self) -> Box<dyn gpui::Action> {
        use crate::*;
        match self {
            Tool::Select => Box::new(ToolSelect),
            Tool::Note => Box::new(ToolNote),
            Tool::Highlight => Box::new(ToolHighlight),
            Tool::Underline => Box::new(ToolUnderline),
            Tool::StrikeOut => Box::new(ToolStrikeOut),
            Tool::Squiggly => Box::new(ToolSquiggly),
            Tool::TextEdit => Box::new(ToolTextEdit),
            Tool::FreeText => Box::new(ToolFreeText),
            Tool::Square => Box::new(ToolSquare),
            Tool::Ellipse => Box::new(ToolEllipse),
            Tool::Line => Box::new(ToolLine),
            Tool::Arrow => Box::new(ToolArrow),
            Tool::Ink => Box::new(ToolInk),
            Tool::Tick => Box::new(ToolTick),
            Tool::Stamp => Box::new(ToolStamp),
            Tool::Polygon => Box::new(ToolPolygon),
            Tool::Polyline => Box::new(ToolPolyline),
            Tool::Cloud => Box::new(ToolCloud),
            Tool::Callout => Box::new(ToolCallout),
            Tool::AttachFile => Box::new(ToolAttachFile),
            Tool::Connector => Box::new(ToolConnector),
            Tool::Distance => Box::new(ToolDistance),
            Tool::Perimeter => Box::new(ToolPerimeter),
            Tool::Area => Box::new(ToolArea),
        }
    }

    pub const ALL: [Tool; 24] = [
        Tool::Select,
        Tool::Note,
        Tool::Highlight,
        Tool::Underline,
        Tool::StrikeOut,
        Tool::Squiggly,
        Tool::TextEdit,
        Tool::FreeText,
        Tool::Square,
        Tool::Ellipse,
        Tool::Line,
        Tool::Arrow,
        Tool::Polyline,
        Tool::Polygon,
        Tool::Cloud,
        Tool::Callout,
        Tool::Ink,
        Tool::Tick,
        Tool::Stamp,
        Tool::AttachFile,
        Tool::Connector,
        Tool::Distance,
        Tool::Perimeter,
        Tool::Area,
    ];
}

pub use sheaf_doc::annot::COLORS;

/// The color `tool` draws in: the last one picked for it, else its own
/// default ([`Tool::default_color`]). Each tool keeps its own, in
/// `state.json`, so a strikethrough after a highlight is still red.
pub(crate) fn color_of(tool: Tool, cx: &gpui::App) -> [u8; 3] {
    crate::state::get(cx)
        .tool_colors
        .get(tool.id())
        .copied()
        .unwrap_or_else(|| tool.default_color())
}

/// A mouse drag in progress on a page.
#[derive(Debug, Clone)]
pub struct Drag {
    pub page: usize,
    pub page_id: PageId,
    /// Points in the page's edit frame.
    pub start: Pt,
    pub current: Pt,
    /// Every sampled point (ink).
    pub path: Vec<Pt>,
    /// Marks being moved, as they were, in the page's edit frame
    /// ([`Annot::to_frame`]): the one grabbed and the others selected on its
    /// page.
    pub moving: Vec<Annot>,
    /// A mark being reshaped by one of its handles.
    pub resize: Option<Resize>,
    /// Shift is down: keep a box's proportions, or a line at 45°.
    pub shift: bool,
}

/// Which handle of a mark is dragged.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Spot {
    /// A box handle: which sides move, 0 the left or top, 1 the right or
    /// bottom, 0.5 neither.
    Box { fx: f32, fy: f32 },
    /// A line's end: 0 is `a`, 1 is `b`.
    End(u8),
    /// A polygon's or polyline's vertex; a callout's point (0) or knee
    /// (1).
    Vertex(usize),
}

#[derive(Debug, Clone)]
pub struct Resize {
    pub spot: Spot,
    /// The mark as it was, in the page's edit frame ([`Annot::to_frame`]).
    pub orig: Annot,
}

/// A mark's handles, in the frame it's in.
pub fn handle_spots(a: &Annot) -> Vec<(Spot, Pt)> {
    use sheaf_doc::annot::Handles;
    match a.kind.handles() {
        Handles::None => Vec::new(),
        Handles::Ends => match &a.kind {
            AnnotKind::Line { a: pa, b: pb, .. } => vec![(Spot::End(0), *pa), (Spot::End(1), *pb)],
            _ => Vec::new(),
        },
        Handles::Vertices => match &a.kind {
            AnnotKind::Polygon { points }
            | AnnotKind::Polyline { points }
            | AnnotKind::Measure { points, .. } => points
                .iter()
                .enumerate()
                .map(|(i, p)| (Spot::Vertex(i), *p))
                .collect(),
            _ => Vec::new(),
        },
        Handles::Callout => match &a.kind {
            AnnotKind::Callout {
                rect, knee, point, ..
            } => {
                let mut spots = box_spots(*rect);
                spots.push((Spot::Vertex(0), *point));
                spots.push((Spot::Vertex(1), *knee));
                spots
            }
            _ => Vec::new(),
        },
        Handles::Box => box_spots(a.kind.bounds()),
    }
}

/// A box's eight handles.
fn box_spots(b: Box2) -> Vec<(Spot, Pt)> {
    [
        (0.0, 0.0),
        (0.5, 0.0),
        (1.0, 0.0),
        (1.0, 0.5),
        (1.0, 1.0),
        (0.5, 1.0),
        (0.0, 1.0),
        (0.0, 0.5),
    ]
    .into_iter()
    .map(|(fx, fy)| {
        (
            Spot::Box { fx, fy },
            Pt {
                x: b.x + fx * b.w,
                y: b.y + fy * b.h,
            },
        )
    })
    .collect()
}

/// `r.orig` with its handle dragged to `p`.
pub fn reshaped(r: &Resize, p: Pt, shift: bool) -> Annot {
    let mut out = r.orig.clone();
    match r.spot {
        Spot::Box { fx, fy } => {
            // A callout's box handles size its box, not the leader.
            let b = match &r.orig.kind {
                AnnotKind::Callout { rect, .. } => *rect,
                _ => r.orig.kind.bounds(),
            };
            // The corner or side opposite the handle stays put.
            let anchor = Pt {
                x: if fx == 0.0 { b.right() } else { b.x },
                y: if fy == 0.0 { b.bottom() } else { b.y },
            };
            // A side handle only moves along its own axis.
            let (mut x0, mut x1) = if fx == 0.5 {
                (b.x, b.right())
            } else {
                (anchor.x, p.x)
            };
            let (mut y0, mut y1) = if fy == 0.5 {
                (b.y, b.bottom())
            } else {
                (anchor.y, p.y)
            };
            if shift && fx != 0.5 && fy != 0.5 && b.w > 0.0 && b.h > 0.0 {
                let s = ((x1 - x0).abs() / b.w).max((y1 - y0).abs() / b.h);
                x1 = x0 + (x1 - x0).signum() * b.w * s;
                y1 = y0 + (y1 - y0).signum() * b.h * s;
            }
            // Never smaller than a couple of points.
            if (x1 - x0).abs() < 2.0 {
                x1 = x0 + if x1 >= x0 { 2.0 } else { -2.0 };
            }
            if (y1 - y0).abs() < 2.0 {
                y1 = y0 + if y1 >= y0 { 2.0 } else { -2.0 };
            }
            (x0, x1) = (x0.min(x1), x0.max(x1));
            (y0, y1) = (y0.min(y1), y0.max(y1));
            let to = Box2 {
                x: x0,
                y: y0,
                w: x1 - x0,
                h: y1 - y0,
            };
            out.kind = match &r.orig.kind {
                AnnotKind::Callout {
                    knee, point, size, ..
                } => AnnotKind::Callout {
                    rect: to,
                    knee: *knee,
                    point: *point,
                    size: *size,
                },
                kind => kind.resized(b, to),
            };
        }
        Spot::Vertex(i) => match &mut out.kind {
            AnnotKind::Polygon { points }
            | AnnotKind::Polyline { points }
            | AnnotKind::Measure { points, .. } => {
                if let Some(v) = points.get_mut(i) {
                    *v = p;
                }
            }
            AnnotKind::Callout { point, knee, .. } => {
                if i == 0 {
                    *point = p;
                } else {
                    *knee = p;
                }
            }
            _ => {}
        },
        Spot::End(end) => {
            if let AnnotKind::Line { a, b, .. } = &mut out.kind {
                let fixed = if end == 0 { *b } else { *a };
                let mut q = p;
                if shift {
                    // Snap to the nearest 45° around the other end.
                    let (dx, dy) = (p.x - fixed.x, p.y - fixed.y);
                    let len = (dx * dx + dy * dy).sqrt();
                    let step = std::f32::consts::FRAC_PI_4;
                    let ang = (dy.atan2(dx) / step).round() * step;
                    q = Pt {
                        x: fixed.x + len * ang.cos(),
                        y: fixed.y + len * ang.sin(),
                    };
                }
                if end == 0 {
                    *a = q;
                } else {
                    *b = q;
                }
            }
        }
    }
    out
}

/// `m`, a mark in an edit frame, moved as a drag from `a` to `b` in that
/// frame moves it.
fn dragged(m: &Annot, a: Pt, b: Pt) -> Annot {
    Annot {
        kind: m.kind.translated(b.x - a.x, b.y - a.y),
        ..m.clone()
    }
}

impl DocView {
    pub(crate) fn set_tool(&mut self, tool: Tool, cx: &mut Context<Self>) {
        self.tool = tool;
        self.text_hover = false;
        if let State::Ready(doc) = &mut self.state {
            doc.drag = None;
            if tool != Tool::Select {
                doc.selected_marks.clear();
                doc.text_sel = None;
                doc.text_drag = None;
            }
        }
        cx.notify();
    }

    /// Scroll to an annotation anywhere in the document and select it.
    pub(crate) fn jump_to_annot(&mut self, id: AnnotId, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let Some((page_id, a)) = doc.model.state().find_annot(id) else {
            doc.notice = Some("The other end of this connector is gone".into());
            cx.notify();
            return;
        };
        let b = a.page_bounds();
        let Some(index) = doc.model.state().index_of(page_id) else {
            return;
        };
        doc.reveal(index, b);
        doc.select_mark(Some((page_id, id)));
        doc.notice = Some(format!("Page {}", index + 1));
        cx.notify();
    }

    /// Window position → (page index, point in its edit frame, the page's
    /// scale in px per point, for hit areas that stay the same size on
    /// screen). The pane shows the page turned by rotate view; the point
    /// goes through PageSpace into the frame without it.
    pub(crate) fn page_point_k(&self, pos: Point<Pixels>) -> Option<(usize, Pt, f32)> {
        let State::Ready(doc) = &self.state else {
            return None;
        };
        let pane = doc.pane();
        let layout = pane.layout.as_ref()?;
        let b = pane.scroll.bounds();
        let x = f32::from(pos.x - b.origin.x) - f32::from(pane.scroll.offset().x);
        let y = f32::from(pos.y - b.origin.y) - f32::from(pane.scroll.offset().y);
        let i = layout.page_at(x, y)?;
        let r = layout.rect(i)?;
        let (left, top, w, h) = (r.left, r.top, r.w, r.h);
        if y < top || y > top + h || x < left || x > left + w {
            return None;
        }
        let pane = doc.model.geom(&layout.refs[i]);
        let k = w / pane.shown().width.max(1.0);
        let shown = Pt {
            x: (x - left) / k,
            y: (y - top) / k,
        };
        let edit = doc.edit_geom(i)?;
        Some((i, pane.from_view().then(edit.to_view()).apply(shown), k))
    }

    /// `p`, a point on page `index` in its edit frame, in PageSpace, where
    /// text and regions are.
    pub(crate) fn page_space(&self, index: usize, p: Pt) -> Pt {
        match &self.state {
            State::Ready(doc) => doc.edit_geom(index).map_or(p, |g| g.from_view().apply(p)),
            _ => p,
        }
    }

    /// Page `page_id`'s edit frame.
    pub(crate) fn edit_geom_of(&self, page_id: PageId) -> Option<Geom> {
        let State::Ready(doc) = &self.state else {
            return None;
        };
        doc.edit_geom(doc.model.index_of(page_id)?)
    }

    /// The topmost of our marks on page `page_id` under `p`, a point in
    /// its edit frame. Each mark is hit where it's drawn: laid out as it
    /// reads.
    pub(crate) fn mark_at(&self, page_id: PageId, p: Pt, tolerance: f32) -> Option<Annot> {
        let State::Ready(doc) = &self.state else {
            return None;
        };
        // A mark the comment filter hides can't be clicked.
        if doc.comments_hidden {
            return None;
        }
        let from_edit = self.edit_geom_of(page_id)?.from_view();
        doc.model.state().annots_on(page_id).and_then(|list| {
            list.iter()
                .rev()
                .filter(|a| !doc.hidden_marks.contains(&a.id))
                .find(|a| {
                    let at = from_edit.then(Affine::upright(a.turn)).apply(p);
                    a.hit(at, tolerance)
                })
                .cloned()
        })
    }

    /// The handle of the selected mark under `p` (in the edit frame), when
    /// one mark is selected and it's on this page; with the mark in the
    /// edit frame, as a resize reshapes it there.
    fn handle_at(&self, page_id: PageId, p: Pt, tolerance: f32) -> Option<(Spot, Annot)> {
        let State::Ready(doc) = &self.state else {
            return None;
        };
        let [(pid, _)] = doc.selected_marks.as_slice() else {
            return None;
        };
        if *pid != page_id {
            return None;
        }
        let (_, a) = doc.selected_mark_list().pop()?;
        let a = a.to_frame(&self.edit_geom_of(page_id)?);
        handle_spots(&a)
            .into_iter()
            .find(|(_, q)| (q.x - p.x).abs() <= tolerance && (q.y - p.y).abs() <= tolerance)
            .map(|(spot, _)| (spot, a))
    }

    pub(crate) fn on_page_mouse_down(
        &mut self,
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ev.button != MouseButton::Left {
            return;
        }
        let Some((page, p, k)) = self.page_point_k(ev.position) else {
            return;
        };
        window.focus(&self.focus_handle, cx);
        // A press anywhere on a page drops selected text; a drag on text
        // makes a new selection.
        if let State::Ready(doc) = &mut self.state {
            doc.text_sel = None;
            doc.text_drag = None;
            doc.adopting = None;
        }
        // Hit areas of a few screen pixels, whatever the zoom.
        let tolerance = 4.0 / k.max(0.01);
        let page_id = match &self.state {
            State::Ready(doc) => doc.page_ref(page).map(|r| r.id),
            _ => None,
        };
        let Some(page_id) = page_id else {
            return;
        };
        let mark = self.mark_at(page_id, p, tolerance);
        // A measurement's points snap to the grid and guides, or (Shift)
        // keep to 45° from the last corner.
        let p = if self.tool.measures().is_some() {
            let from = match &self.state {
                State::Ready(doc) => doc.poly.as_ref().and_then(|d| d.points.last().copied()),
                _ => None,
            };
            self.measure_point(page, p, from, ev.modifiers.shift, k)
        } else {
            p
        };
        // Where text and regions are.
        let at = self.page_space(page, p);
        // Shift+click on a mark adds it to the selection (or takes it out);
        // Shift+drag elsewhere selects a region, whatever the tool (but a
        // Measure tool, where Shift keeps to 45°).
        if ev.modifiers.shift && self.tool.measures().is_none() {
            if self.tool == Tool::Select
                && let Some(a) = &mark
                && let State::Ready(doc) = &mut self.state
            {
                doc.toggle_mark((page_id, a.id));
                cx.notify();
                return;
            }
            self.start_region(page, at, cx);
            return;
        }
        self.clear_region(cx);
        // A calculator tape waiting to be placed takes the click, whatever
        // the tool.
        if self.place_tape_at(page_id, p, cx) {
            return;
        }
        // Picking numbers into the calculator takes the click too.
        if self.pick_number_at(page_id, at, tolerance, cx) {
            return;
        }
        // The Select tool grabs marks and their handles; with Ctrl, any
        // tool moves a mark (PDFlyer's way of moving a stamp without
        // leaving the stamp tool).
        if self.tool == Tool::Select || ev.modifiers.control {
            if self.tool == Tool::Select
                && let Some((spot, a)) = self.handle_at(page_id, p, tolerance * 1.5)
                && let State::Ready(doc) = &mut self.state
            {
                doc.drag = Some(Drag {
                    page,
                    page_id,
                    start: p,
                    current: p,
                    path: Vec::new(),
                    moving: Vec::new(),
                    resize: Some(Resize { spot, orig: a }),
                    shift: false,
                });
                cx.notify();
                return;
            }
            if let Some(a) = mark {
                if ev.click_count >= 2 && self.tool == Tool::Select {
                    // A tape goes back to the calculator to be edited; an
                    // attachment opens its file.
                    if matches!(a.kind, AnnotKind::Tape { .. }) {
                        self.edit_tape(page_id, a.id, cx);
                    } else if matches!(a.kind, AnnotKind::FileAttachment { .. }) {
                        self.open_attached(page_id, a.id, window, cx);
                    } else {
                        self.start_annot_editor_for(page_id, a.id, window, cx);
                    }
                    return;
                }
                let State::Ready(doc) = &mut self.state else {
                    return;
                };
                if !doc.selected_marks.contains(&(page_id, a.id)) {
                    doc.select_mark(Some((page_id, a.id)));
                }
                let Some(edit) = doc.edit_geom(page) else {
                    return;
                };
                // Everything selected on this page moves together.
                let moving: Vec<Annot> = doc
                    .selected_mark_list()
                    .into_iter()
                    .filter(|(pid, _)| *pid == page_id)
                    .map(|(_, m)| m.to_frame(&edit))
                    .collect();
                doc.drag = Some(Drag {
                    page,
                    page_id,
                    start: p,
                    current: p,
                    path: Vec::new(),
                    moving,
                    resize: None,
                    shift: false,
                });
                cx.notify();
                return;
            }
            // Another tool's comment: selected by a click, made ours when
            // it's dragged or double-clicked (see `adopt`).
            if let Some(ix) = self.foreign_at(page_id, p, tolerance) {
                if ev.click_count >= 2 && self.tool == Tool::Select {
                    self.edit_foreign(ix, window, cx);
                    return;
                }
                let Some((_, mark)) = self.foreign_mark(ix) else {
                    return;
                };
                let State::Ready(doc) = &mut self.state else {
                    return;
                };
                let Some(edit) = doc.edit_geom(page) else {
                    return;
                };
                doc.select_mark(None);
                doc.selected_foreign = Some(ix);
                doc.adopting = Some(ix);
                doc.drag = Some(Drag {
                    page,
                    page_id,
                    start: p,
                    current: p,
                    path: Vec::new(),
                    moving: vec![mark.to_frame(&edit)],
                    resize: None,
                    shift: false,
                });
                cx.notify();
                return;
            }
            if self.tool == Tool::Select {
                // Away from any mark: select text.
                self.start_text_drag(page_id, page, at, tolerance, ev.click_count, cx);
                return;
            }
        }
        // The text tools select text when the press is on it, as the
        // Select tool does; letting go marks the selection up.
        if self.tool.marks_text() {
            if self.over_text(page_id, at, tolerance) {
                self.start_text_drag(page_id, page, at, tolerance, ev.click_count, cx);
                return;
            }
            if self.tool == Tool::TextEdit {
                if let State::Ready(doc) = &mut self.state {
                    doc.notice = Some(
                        "Text Edits: drag across text to replace it, or click in text to insert"
                            .into(),
                    );
                }
                cx.notify();
                return;
            }
        }
        let tool = self.tool;
        let color = color_of(tool, cx);
        let glyph = self.tick_glyph.clone();
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let Some(pref) = doc.page_ref(page) else {
            return;
        };
        let page_id = pref.id;
        match tool {
            Tool::Select => {}
            Tool::Note => {
                let pos = Pt {
                    x: p.x - 10.0,
                    y: p.y - 10.0,
                };
                let a = self.new_annot(AnnotKind::Note { pos }, color, cx);
                self.add_annot(page_id, a, "Note", cx);
                self.start_annot_editor(page_id, window, cx);
            }
            Tool::Tick => {
                let w = crate::contrib::mark_width(&glyph, 24.0);
                let a = self.new_annot(
                    AnnotKind::Stamp {
                        rect: Box2 {
                            x: p.x - w / 2.0,
                            y: p.y - 12.0,
                            w,
                            h: 24.0,
                        },
                        glyph,
                    },
                    color,
                    cx,
                );
                self.add_annot(page_id, a, "Tickmark", cx);
            }
            Tool::Stamp => self.place_stamp(page_id, p, cx),
            Tool::AttachFile => self.attach_file_at(page_id, p, cx),
            Tool::Polygon | Tool::Polyline | Tool::Cloud | Tool::Perimeter | Tool::Area => {
                self.add_vertex(page, page_id, p, ev.click_count, cx)
            }
            Tool::Connector => self.place_connector(page_id, p, cx),
            _ => {
                doc.drag = Some(Drag {
                    page,
                    page_id,
                    start: p,
                    current: p,
                    path: vec![p],
                    moving: Vec::new(),
                    resize: None,
                    shift: false,
                });
                cx.notify();
            }
        }
    }

    pub(crate) fn on_page_mouse_move(
        &mut self,
        ev: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((page, p, k)) = self.page_point_k(ev.position) else {
            if ev.pressed_button.is_none() {
                self.hover_off_pages(cx);
            }
            return;
        };
        if ev.pressed_button.is_none() {
            let page_id = match &self.state {
                State::Ready(doc) => doc.page_ref(page).map(|r| r.id),
                _ => None,
            };
            if let Some(page_id) = page_id {
                let at = self.page_space(page, p);
                self.hover_page(page_id, at, 4.0 / k.max(0.01), cx);
            }
            self.shape_hover(page, p, ev.modifiers.shift, k, cx);
            return;
        }
        if ev.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let at = self.page_space(page, p);
        if self.drag_region(page, at, cx) {
            return;
        }
        if self.move_text_drag(page, at, cx) {
            return;
        }
        let tool = self.tool;
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let Some(drag) = &mut doc.drag else { return };
        if page != drag.page {
            return;
        }
        drag.current = p;
        drag.shift = ev.modifiers.shift;
        if tool == Tool::Ink {
            drag.path.push(p);
        }
        if tool == Tool::Distance && drag.moving.is_empty() && drag.resize.is_none() {
            let (start, shift) = (drag.start, drag.shift);
            let end = self.measure_point(page, p, Some(start), shift, k);
            if let State::Ready(doc) = &mut self.state
                && let Some(drag) = &mut doc.drag
            {
                drag.current = end;
            }
            self.measure_notice(sheaf_doc::measure::Measured::Distance, &[start, end], cx);
        }
        cx.notify();
    }

    pub(crate) fn on_page_mouse_up(
        &mut self,
        ev: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ev.button != MouseButton::Left {
            return;
        }
        if self.end_region(cx) || self.end_text_drag(cx) {
            return;
        }
        let tool = self.tool;
        let color = color_of(tool, cx);
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let Some(drag) = doc.drag.take() else { return };
        let adopting = doc.adopting.take();
        let Some(edit) = doc.edit_geom(drag.page) else {
            return;
        };
        let page_id = drag.page_id;
        let (a, b) = (drag.start, drag.current);
        if let Some(r) = &drag.resize {
            let changed = reshaped(r, b, drag.shift);
            if changed != r.orig {
                let mut changed = changed.to_page(&edit);
                changed.modified = sheaf_doc::pdf_date_now();
                self.apply(
                    Edit::AnnotUpdate {
                        page: page_id,
                        annot: changed,
                    },
                    "Resize annotation",
                    cx,
                );
            }
            return;
        }
        if let Some(ix) = adopting {
            // Another tool's comment, moved: ours now, where it was let go.
            let (dx, dy) = (b.x - a.x, b.y - a.y);
            if (dx.abs() >= 0.5 || dy.abs() >= 0.5)
                && let Some(m) = drag.moving.first()
            {
                let mut moved = dragged(m, a, b).to_page(&edit);
                moved.modified = sheaf_doc::pdf_date_now();
                self.adopt_foreign(ix, |m| *m = moved, "Move annotation", cx);
            }
            return;
        }
        if !drag.moving.is_empty() {
            let (dx, dy) = (b.x - a.x, b.y - a.y);
            if dx.abs() < 0.5 && dy.abs() < 0.5 {
                // A click: a connector goes to the next of its label.
                if let [one] = drag.moving.as_slice()
                    && let AnnotKind::Connector { label, .. } = &one.kind
                {
                    let group = sheaf_doc::annot::Group::Connector(label.clone());
                    self.group_step_from(group, one.id, 1, cx);
                }
                return;
            }
            let now = sheaf_doc::pdf_date_now();
            let n = drag.moving.len();
            let edits = drag
                .moving
                .iter()
                .map(|m| {
                    let mut moved = dragged(m, a, b).to_page(&edit);
                    moved.modified = now.clone();
                    Edit::AnnotUpdate {
                        page: page_id,
                        annot: moved,
                    }
                })
                .collect();
            let label = if n == 1 {
                "Move annotation".to_string()
            } else {
                format!("Move {n} annotations")
            };
            self.apply_batch(edits, &label, cx);
            return;
        }
        let rect = Box2::from_corners(a, b);
        let too_small = rect.w < 3.0 && rect.h < 3.0;
        match tool {
            Tool::Select | Tool::TextEdit => {}
            Tool::Callout => self.place_callout(page_id, a, b, cx),
            Tool::Distance => self.finish_distance(page_id, a, b, cx),
            Tool::Polygon | Tool::Polyline | Tool::Cloud | Tool::Perimeter | Tool::Area => {}
            Tool::Highlight | Tool::Underline | Tool::StrikeOut | Tool::Squiggly => {
                if too_small {
                    return;
                }
                let rect = edit.from_view().apply_box(rect);
                self.markup_from_drag(tool, page_id, rect, edit.rotate, cx);
            }
            Tool::FreeText => {
                if too_small {
                    return;
                }
                let r = Box2 {
                    x: rect.x,
                    y: rect.y,
                    w: rect.w.max(60.0),
                    h: rect.h.max(18.0),
                };
                let a = self.new_annot(
                    AnnotKind::FreeText {
                        rect: r,
                        size: 12.0,
                    },
                    color,
                    cx,
                );
                self.add_annot(page_id, a, "Text", cx);
                self.start_annot_editor(page_id, window, cx);
            }
            Tool::Square | Tool::Ellipse => {
                if too_small {
                    return;
                }
                let kind = if tool == Tool::Square {
                    AnnotKind::Square { rect }
                } else {
                    AnnotKind::Ellipse { rect }
                };
                let a = self.new_annot(kind, color, cx);
                self.add_annot(page_id, a, tool.label(), cx);
            }
            Tool::Line | Tool::Arrow => {
                if too_small {
                    return;
                }
                let a = self.new_annot(
                    AnnotKind::Line {
                        a,
                        b,
                        arrow: tool == Tool::Arrow,
                        arrow_start: false,
                    },
                    color,
                    cx,
                );
                self.add_annot(page_id, a, tool.label(), cx);
            }
            Tool::Ink => {
                if drag.path.len() < 2 {
                    return;
                }
                let mut an = self.new_annot(
                    AnnotKind::Ink {
                        paths: vec![drag.path],
                    },
                    color,
                    cx,
                );
                an.stroke = 2.0;
                self.add_annot(page_id, an, "Pencil", cx);
            }
            Tool::Note | Tool::Tick | Tool::Stamp | Tool::AttachFile | Tool::Connector => {}
        }
    }

    /// Mark up the text a rectangle dragged off the text covered (the
    /// engine's, or an unsaved OCR layer's), with `rect` and the markup in
    /// PageSpace, turned as the text reads. Where there's no text, a
    /// highlight covers the area, turned `drawn`, as the page showed when
    /// it was dragged; the line markups do nothing.
    fn markup_from_drag(
        &mut self,
        tool: Tool,
        page_id: PageId,
        rect: Box2,
        drawn: Rotation,
        cx: &mut Context<Self>,
    ) {
        self.with_chars(page_id, cx, move |view, chars, cx| {
            let on_text = chars
                .map(|c| (c.rects_in(rect), c.upright))
                .filter(|(rects, _)| !rects.is_empty());
            let (rects, turn) = match on_text {
                Some(found) => found,
                None if tool == Tool::Highlight => (vec![rect], drawn),
                None => {
                    if let State::Ready(doc) = &mut view.state {
                        doc.notice = Some(format!("{}: no text there", tool.label()));
                    }
                    cx.notify();
                    return;
                }
            };
            view.add_text_markup(tool, page_id, rects, turn, cx);
        });
    }

    pub(crate) fn delete_selected_annot(&mut self, cx: &mut Context<Self>) -> bool {
        let State::Ready(doc) = &mut self.state else {
            return false;
        };
        let marks = std::mem::take(&mut doc.selected_marks);
        let st = doc.model.state();
        // A reply or status goes with its comment; don't delete it twice.
        let names: std::collections::HashSet<String> = marks
            .iter()
            .filter_map(|(_, id)| st.find_annot(*id).map(|(_, a)| a.name()))
            .collect();
        let edits: Vec<Edit> = marks
            .iter()
            .filter(|(_, id)| {
                st.find_annot(*id)
                    .and_then(|(_, a)| a.thread_parent().map(|p| !names.contains(p)))
                    .unwrap_or(true)
            })
            .map(|(page, id)| Edit::AnnotDelete {
                page: *page,
                id: *id,
            })
            .collect();
        let label = match marks.as_slice() {
            [] => return false,
            [(_, id)] => match st.find_annot(*id).map(|(_, a)| a.kind.clone()) {
                Some(AnnotKind::Connector { label, .. }) => format!("Delete connector {label}"),
                _ => "Delete annotation".to_string(),
            },
            more => format!("Delete {} annotations", more.len()),
        };
        self.apply_batch(edits, &label, cx);
        true
    }

    /// The markup color swatches, and their length along a toolbar: the
    /// tool's color, which a click changes for that tool; with Select, the
    /// selected marks' color, which a click changes for them.
    pub(crate) fn render_colors(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let color = match self.tool {
            Tool::Select => match &self.state {
                State::Ready(doc) => {
                    let marks = doc.selected_mark_list();
                    marks
                        .first()
                        .map(|(_, a)| a.color)
                        .filter(|c| marks.iter().all(|(_, a)| a.color == *c))
                }
                _ => None,
            },
            tool => Some(color_of(tool, cx)),
        };
        let colors: Vec<_> = COLORS
            .iter()
            .map(|&(c, name)| {
                let active = Some(c) == color;
                div()
                    .id((
                        "color",
                        ((c[0] as usize) << 16) | ((c[1] as usize) << 8) | c[2] as usize,
                    ))
                    .w(px(16.0))
                    .h(px(16.0))
                    .rounded_sm()
                    .bg(rgb(((c[0] as u32) << 16)
                        | ((c[1] as u32) << 8)
                        | c[2] as u32))
                    .border_2()
                    .border_color(if active {
                        theme::FOCUS_RING
                    } else {
                        theme::SUNKEN
                    })
                    .cursor_pointer()
                    .tooltip(Tooltip::text(name))
                    .on_click(cx.listener(move |v, _, window, cx| {
                        // The toolbar sits outside the document; keep its
                        // keys (Ctrl+Z after a recolor) going to it.
                        window.focus(&v.focus_handle, cx);
                        let tool = v.tool;
                        if tool != Tool::Select {
                            crate::state::update(cx, |s| {
                                s.tool_colors.insert(tool.id().to_string(), c);
                            });
                        }
                        // With marks selected, they take the color too.
                        v.restyle(
                            "Color annotation",
                            |a| {
                                a.color = c;
                                true
                            },
                            cx,
                        );
                        cx.notify();
                    }))
            })
            .collect();
        div()
            .flex()
            .items_center()
            .gap_1()
            .children(colors)
            .into_any_element()
    }

    pub(crate) fn colors_length() -> f32 {
        // 16px swatches, gap_1 between.
        COLORS.len() as f32 * 20.0 - 4.0
    }

    /// The tickmark and stamp palette, and its length along a toolbar.
    pub(crate) fn render_ticks(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let tool = self.tool;
        let tick = self.tick_glyph.clone();
        let marks = cx.global::<crate::contrib::Contributions>().toolbar_marks();
        let ticks: Vec<_> = marks
            .into_iter()
            .enumerate()
            .map(|(i, g)| {
                let active = tool == Tool::Tick && g == tick;
                let wide = g.chars().count() > 1;
                let pick = g.clone();
                div()
                    .id(("tick", i))
                    .when(!wide, |d| d.w(px(22.0)))
                    .when(wide, |d| d.px_1p5())
                    .h(px(20.0))
                    .rounded_sm()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .font_weight(gpui::FontWeight::BOLD)
                    .cursor_pointer()
                    .bg(if active {
                        theme::ACCENT
                    } else {
                        theme::CONTROL
                    })
                    .hover(|s| s.bg(theme::CONTROL_HOVER))
                    .tooltip(Tooltip::for_action(
                        format!("{} {g}", if wide { "Stamp" } else { "Tickmark" }),
                        "Click on the page to stamp this mark",
                        Box::new(crate::ToolTick),
                    ))
                    .on_click(cx.listener(move |v, _, window, cx| {
                        window.focus(&v.focus_handle, cx);
                        v.tick_glyph = pick.clone();
                        v.set_tool(Tool::Tick, cx);
                    }))
                    .child(g)
            })
            .collect();
        div()
            .flex()
            .items_center()
            .gap_1()
            .children(ticks)
            .into_any_element()
    }

    /// The tickmark palette's length: 22px per single mark, wider stamps
    /// about 7px a character plus padding, gap_1 between.
    pub(crate) fn ticks_length(cx: &gpui::App) -> f32 {
        let marks = cx.global::<crate::contrib::Contributions>().toolbar_marks();
        let along: f32 = marks
            .iter()
            .map(|g| match g.chars().count() {
                1 => 22.0,
                n => n as f32 * 7.0 + 12.0,
            } + 4.0)
            .sum();
        (along - 4.0).max(0.0)
    }

    pub(crate) fn page_cursor(&self) -> CursorStyle {
        if (self.tool == Tool::Select || self.tool.marks_text()) && self.text_hover {
            CursorStyle::IBeam
        } else if self.tool == Tool::Select {
            CursorStyle::Arrow
        } else {
            CursorStyle::Crosshair
        }
    }

    /// Live preview of the current drag on `page`, in page-local logical
    /// px: the drag is in the edit frame of `edit`, and the page is drawn
    /// as `pane` shows it, at `k` px a point.
    pub(crate) fn drag_preview(
        doc: &crate::docview::Doc,
        tool: Tool,
        color: [u8; 3],
        page: usize,
        edit: &Geom,
        pane: &Geom,
        k: f32,
    ) -> Option<AnyElement> {
        let drag = doc.drag.as_ref()?;
        if drag.page != page {
            return None;
        }
        let c = rgb(((color[0] as u32) << 16) | ((color[1] as u32) << 8) | color[2] as u32);
        let (a, b) = (drag.start, drag.current);
        let (to_pane, size) = (pane.to_view(), pane.shown());
        let from_edit = edit.from_view().then(to_pane);
        let px_of = |p: Pt| {
            let q = from_edit.apply(p);
            (q.x * k, q.y * k)
        };
        // Moving or reshaping marks, whatever the tool (Ctrl+drag), shown
        // as they'll be let go.
        let shown: Vec<Annot> = match &drag.resize {
            Some(r) => vec![reshaped(r, b, drag.shift).to_page(edit)],
            None => drag
                .moving
                .iter()
                .map(|m| dragged(m, a, b).to_page(edit))
                .collect(),
        };
        if !shown.is_empty() {
            return Some(
                div()
                    .absolute()
                    .inset_0()
                    .children(shown.iter().flat_map(|m| {
                        crate::docview::annot_elements(m, doc.model.assets(), &to_pane, size, k)
                    }))
                    .into_any_element(),
            );
        }
        match tool {
            Tool::Select => None,
            Tool::Line | Tool::Arrow | Tool::Callout => Some(stroke_canvas(
                vec![vec![px_of(a), px_of(b)]],
                c,
                1.5 * k,
                size,
                k,
            )),
            Tool::Ink => {
                let pts: Vec<(f32, f32)> = drag.path.iter().map(|p| px_of(*p)).collect();
                Some(stroke_canvas(vec![pts], c, 2.0 * k, size, k))
            }
            Tool::TextEdit => None,
            Tool::Highlight
            | Tool::Underline
            | Tool::StrikeOut
            | Tool::Squiggly
            | Tool::FreeText
            | Tool::Square
            | Tool::Ellipse => {
                let r = from_edit.apply_box(Box2::from_corners(a, b));
                let el = div()
                    .absolute()
                    .left(px(r.x * k))
                    .top(px(r.y * k))
                    .w(px(r.w * k))
                    .h(px(r.h * k));
                Some(match tool {
                    Tool::Highlight => el.bg(gpui::Rgba { a: 0.375, ..c }).into_any_element(),
                    Tool::Underline | Tool::StrikeOut | Tool::Squiggly => el
                        .bg(theme::REGION_FILL)
                        .border_1()
                        .border_color(theme::REGION_BORDER)
                        .into_any_element(),
                    Tool::Ellipse => el
                        .border_1()
                        .border_color(c)
                        .rounded_full()
                        .into_any_element(),
                    _ => el.border_1().border_color(c).into_any_element(),
                })
            }
            Tool::Note | Tool::Tick | Tool::Stamp | Tool::AttachFile | Tool::Connector => None,
            Tool::Polygon | Tool::Polyline | Tool::Cloud | Tool::Perimeter | Tool::Area => None,
            Tool::Distance => {
                let m = Annot {
                    id: sheaf_doc::AnnotId(0),
                    kind: AnnotKind::Measure {
                        what: sheaf_doc::measure::Measured::Distance,
                        points: vec![a, b],
                        scale: Box::new(doc.measure_scale.clone()),
                    },
                    color,
                    opacity: 1.0,
                    stroke: 1.0,
                    author: String::new(),
                    contents: String::new(),
                    modified: String::new(),
                    created: String::new(),
                    fill: None,
                    dashed: false,
                    cloudy: false,
                    turn: Rotation::R0,
                    name: String::new(),
                }
                .to_page(edit);
                Some(
                    div()
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
                )
            }
        }
    }
}

/// Stroke polylines (page-local logical px) with GPUI's path renderer. The
/// canvas covers the whole page so paths can be positioned by offsetting
/// from its bounds.
pub fn stroke_canvas(
    paths: Vec<Vec<(f32, f32)>>,
    color: gpui::Rgba,
    width: f32,
    disp: PageSize,
    k: f32,
) -> AnyElement {
    stroke_canvas_dashed(paths, color, width, None, disp, k)
}

/// [`stroke_canvas`], dashed as sheaf writes dashes (`annot::DASH`) at
/// zoom `dash` when there is one.
pub fn stroke_canvas_dashed(
    paths: Vec<Vec<(f32, f32)>>,
    color: gpui::Rgba,
    width: f32,
    dash: Option<f32>,
    disp: PageSize,
    k: f32,
) -> AnyElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            for pts in &paths {
                if pts.len() < 2 {
                    continue;
                }
                let mut pb = PathBuilder::stroke(px(width.max(1.0)));
                if let Some(z) = dash {
                    let [on, off] = sheaf_doc::annot::DASH;
                    pb = pb.dash_array(&[px(on * z), px(off * z)]);
                }
                let o = bounds.origin;
                pb.move_to(gpui::point(o.x + px(pts[0].0), o.y + px(pts[0].1)));
                for &(x, y) in &pts[1..] {
                    pb.line_to(gpui::point(o.x + px(x), o.y + px(y)));
                }
                if let Ok(path) = pb.build() {
                    window.paint_path(path, color);
                }
            }
        },
    )
    .absolute()
    .left(px(0.0))
    .top(px(0.0))
    .w(px(disp.width * k))
    .h(px(disp.height * k))
    .into_any_element()
}

/// Fill closed polygons (page-local logical px), like [`stroke_canvas`].
pub fn fill_canvas(
    polys: Vec<Vec<(f32, f32)>>,
    color: gpui::Rgba,
    disp: PageSize,
    k: f32,
) -> AnyElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            for pts in &polys {
                if pts.len() < 3 {
                    continue;
                }
                let mut pb = PathBuilder::fill();
                let o = bounds.origin;
                pb.move_to(gpui::point(o.x + px(pts[0].0), o.y + px(pts[0].1)));
                for &(x, y) in &pts[1..] {
                    pb.line_to(gpui::point(o.x + px(x), o.y + px(y)));
                }
                pb.close();
                if let Ok(path) = pb.build() {
                    window.paint_path(path, color);
                }
            }
        },
    )
    .absolute()
    .left(px(0.0))
    .top(px(0.0))
    .w(px(disp.width * k))
    .h(px(disp.height * k))
    .into_any_element()
}

/// Arrowhead points for a line from `a` to `b` (page-local px).
pub fn arrow_head(a: (f32, f32), b: (f32, f32), size: f32) -> Vec<(f32, f32)> {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = (dx * dx + dy * dy).sqrt().max(0.001);
    let (ux, uy) = (dx / len, dy / len);
    let l = (
        b.0 - size * (ux * 0.866 - uy * 0.5),
        b.1 - size * (uy * 0.866 + ux * 0.5),
    );
    let r = (
        b.0 - size * (ux * 0.866 + uy * 0.5),
        b.1 - size * (uy * 0.866 - ux * 0.5),
    );
    vec![l, b, r]
}

/// Marks on the clipboard, as the metadata of a text entry (the text is
/// what other programs paste: the marks' text, or their kinds).
#[derive(serde::Serialize, serde::Deserialize)]
struct CopiedMarks {
    /// [`COPIED_MARKS`]; bumped if this changes incompatibly.
    sheaf_marks: u32,
    /// As [`copied`] holds them.
    marks: Vec<Annot>,
}

/// The clipboard's version: 2 holds marks as their pages showed them (1
/// held them in the frame the file displayed, before PageSpace).
const COPIED_MARKS: u32 = 2;

/// What the marks copied last hold, by stem: each page stamp's page, a
/// one-page PDF, and each attachment's bytes (M10.5 §14.1's clipboard
/// blob cache). A paste in any window gives them to its document, whose
/// files may have neither. It holds one copy's, as the clipboard does.
#[derive(Default)]
struct CopiedAssets {
    stamps: HashMap<String, Arc<[u8]>>,
    files: HashMap<String, Arc<[u8]>>,
}

static COPIED_ASSETS: LazyLock<Mutex<CopiedAssets>> = LazyLock::new(Default::default);

impl CopiedAssets {
    /// What `marks` hold, from `assets`, their document's.
    fn of(marks: &[Annot], assets: &MarkAssets) -> CopiedAssets {
        let mut held = CopiedAssets::default();
        for a in marks {
            match &a.kind {
                AnnotKind::PageStamp { stamp, .. } => {
                    if let Some(pdf) = assets.stamp_pdf(stamp) {
                        held.stamps.insert(stamp.clone(), pdf);
                    }
                }
                AnnotKind::FileAttachment { file, name, .. } => match assets.file_bytes(file) {
                    Ok(bytes) => {
                        held.files.insert(file.clone(), bytes);
                    }
                    Err(e) => log::warn!("copying attachment {name}: {e:#}"),
                },
                _ => {}
            }
        }
        held
    }

    /// What `marks` name of these, given to `assets`, the document they're
    /// pasted into.
    fn give(&self, marks: &[Annot], assets: &MarkAssets) {
        for a in marks {
            match &a.kind {
                AnnotKind::PageStamp { stamp, .. } => {
                    if let Some(pdf) = self.stamps.get(stamp) {
                        assets.intern_stamp(stamp, pdf.clone());
                    }
                }
                AnnotKind::FileAttachment { file, .. } => {
                    if let Some(bytes) = self.files.get(file) {
                        assets.intern_file(file, bytes.clone());
                    }
                }
                _ => {}
            }
        }
    }
}

/// `marks`, copied from the page whose edit frame is `from`: as the page
/// shows them, each turned relative to it ([`Annot::to_frame`]), so a paste
/// onto any page puts them where they were seen.
pub(crate) fn copied(marks: impl IntoIterator<Item = Annot>, from: &Geom) -> Vec<Annot> {
    marks.into_iter().map(|a| a.to_frame(from)).collect()
}

/// Copied `marks` pasted onto the page whose edit frame is `onto`, moved
/// `shift` points down and right as it shows them, and turned with it.
pub(crate) fn pasted(
    marks: impl IntoIterator<Item = Annot>,
    onto: &Geom,
    shift: f32,
) -> Vec<Annot> {
    marks
        .into_iter()
        .map(|a| {
            let moved = Annot {
                kind: a.kind.translated(shift, shift),
                ..a
            };
            moved.to_page(onto)
        })
        .collect()
}

/// Opacities and line widths the properties bar offers.
const OPACITIES: [f32; 4] = [0.25, 0.5, 0.75, 1.0];
const WIDTHS: [f32; 6] = [0.5, 1.0, 1.5, 2.0, 3.0, 5.0];

fn swatch_color(c: [u8; 3]) -> gpui::Rgba {
    rgb(((c[0] as u32) << 16) | ((c[1] as u32) << 8) | c[2] as u32)
}

impl DocView {
    /// Change every selected mark `f` applies to (it says whether it did),
    /// as one undo step.
    pub(crate) fn restyle(
        &mut self,
        label: &str,
        f: impl Fn(&mut Annot) -> bool,
        cx: &mut Context<Self>,
    ) {
        let State::Ready(doc) = &self.state else {
            return;
        };
        let now = sheaf_doc::pdf_date_now();
        let edits: Vec<Edit> = doc
            .selected_mark_list()
            .into_iter()
            .filter_map(|(page, mut a)| {
                let before = a.clone();
                (f(&mut a) && a != before).then(|| {
                    a.modified = now.clone();
                    Edit::AnnotUpdate { page, annot: a }
                })
            })
            .collect();
        if !edits.is_empty() {
            self.apply_batch(edits, label, cx);
        }
    }

    /// Copy the selected marks. False when none are selected.
    pub(crate) fn copy_marks(&mut self, cx: &mut Context<Self>) -> bool {
        let State::Ready(doc) = &mut self.state else {
            return false;
        };
        // A reply or status pasted elsewhere would be in no thread.
        let mut marks = Vec::new();
        for (page, a) in doc.selected_mark_list() {
            if a.thread_parent().is_none()
                && let Some(from) = doc.model.index_of(page).and_then(|i| doc.edit_geom(i))
            {
                marks.extend(copied([a], &from));
            }
        }
        if marks.is_empty() {
            return false;
        }
        let held = CopiedAssets::of(&marks, doc.model.assets());
        if let Ok(mut copied) = COPIED_ASSETS.lock() {
            *copied = held;
        }
        let text = marks
            .iter()
            .map(|a| {
                if a.contents.is_empty() {
                    crate::comments_panel::kind_label(&a.kind)
                } else {
                    a.contents.clone()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let n = marks.len();
        cx.write_to_clipboard(gpui::ClipboardItem::new_string_with_json_metadata(
            text,
            CopiedMarks {
                sheaf_marks: COPIED_MARKS,
                marks,
            },
        ));
        doc.notice = Some(if n == 1 {
            "Copied 1 annotation".into()
        } else {
            format!("Copied {n} annotations")
        });
        cx.notify();
        true
    }

    /// Paste copied marks onto the current page where they were seen on
    /// theirs: a little down and right of there, or exactly there
    /// (`in_place`). They come in selected, as one undo step.
    pub(crate) fn paste_marks(&mut self, in_place: bool, cx: &mut Context<Self>) -> bool {
        let Some(copied) = cx.read_from_clipboard().and_then(|item| {
            item.entries().iter().find_map(|e| match e {
                gpui::ClipboardEntry::String(s) => s.metadata_json::<CopiedMarks>(),
                _ => None,
            })
        }) else {
            return false;
        };
        if copied.sheaf_marks != COPIED_MARKS || copied.marks.is_empty() {
            return false;
        }
        let me = crate::prefs::get(cx).identity.name();
        let State::Ready(doc) = &mut self.state else {
            return false;
        };
        let index = doc.current_page();
        let (Some(page), Some(onto)) = (doc.page_ref(index).map(|p| p.id), doc.edit_geom(index))
        else {
            return false;
        };
        if let Ok(held) = COPIED_ASSETS.lock() {
            held.give(&copied.marks, doc.model.assets());
        }
        let before = doc.model.state().annots_on(page).map_or(0, |l| l.len());
        let shift = if in_place { 0.0 } else { 10.0 };
        let now = sheaf_doc::pdf_date_now();
        let n = copied.marks.len();
        let edits = pasted(copied.marks, &onto, shift)
            .into_iter()
            .map(|mut a| {
                // A pasted connector is on its own: its partner stayed put.
                if let AnnotKind::Connector { partner, .. } = &mut a.kind {
                    *partner = None;
                }
                a.author = me.clone();
                a.created = now.clone();
                a.modified = now.clone();
                // A copy is a comment of its own, not the one its replies
                // are to.
                a.name.clear();
                Edit::AnnotAdd { page, annot: a }
            })
            .collect();
        let label = if n == 1 {
            "Paste annotation".to_string()
        } else {
            format!("Paste {n} annotations")
        };
        self.apply_batch(edits, &label, cx);
        if let State::Ready(doc) = &mut self.state {
            let pasted: Vec<(PageId, AnnotId)> = doc
                .model
                .state()
                .annots_on(page)
                .map(|l| l.iter().skip(before).map(|a| (page, a.id)).collect())
                .unwrap_or_default();
            doc.selected_marks = pasted;
        }
        if self.tool != Tool::Select {
            self.set_tool(Tool::Select, cx);
        }
        true
    }

    /// The selected marks' properties, while any are selected. Each change
    /// applies to all of them that it fits, as one undo step.
    pub(crate) fn render_mark_properties(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let State::Ready(doc) = &self.state else {
            return None;
        };
        // Replies and statuses are in the Comments panel, not on the page.
        let marks: Vec<Annot> = doc
            .selected_mark_list()
            .into_iter()
            .map(|(_, a)| a)
            .filter(|a| a.thread_parent().is_none())
            .collect();
        let last = marks.last()?.clone();
        let fillable = marks.iter().any(|a| a.kind.fillable());
        let stroked = marks.iter().any(|a| a.kind.stroked());
        let lines = marks
            .iter()
            .any(|a| matches!(a.kind, AnnotKind::Line { .. }));
        let cloudable = marks.iter().any(|a| a.kind.cloudable());
        let icon = marks.iter().find_map(|a| match &a.kind {
            AnnotKind::FileAttachment { icon, .. } => Some(icon.clone()),
            _ => None,
        });
        let cloudy = last.cloudy;
        let text_size = marks.iter().find_map(|a| a.kind.text_size());
        let label = |s: &'static str| div().text_color(theme::TEXT_MUTED).child(s);
        let chip = |id: (&'static str, usize), text: String, on: bool| {
            div()
                .id(id)
                .px_1p5()
                .h(px(20.0))
                .flex()
                .items_center()
                .rounded_sm()
                .cursor_pointer()
                .bg(if on { theme::ACCENT } else { theme::CONTROL })
                .hover(|s| s.bg(theme::CONTROL_HOVER))
                .child(text)
        };
        let swatch = |id: (&'static str, usize), c: [u8; 3], on: bool| {
            div()
                .id(id)
                .w(px(16.0))
                .h(px(16.0))
                .rounded_sm()
                .cursor_pointer()
                .bg(swatch_color(c))
                .border_2()
                .border_color(if on { theme::FOCUS_RING } else { theme::SUNKEN })
        };
        let colors = COLORS.iter().enumerate().map(|(i, &(c, name))| {
            swatch(("prop-color", i), c, last.color == c)
                .tooltip(Tooltip::text(name))
                .on_click(cx.listener(move |v, _, _, cx| {
                    v.restyle(
                        "Color annotation",
                        |a| {
                            a.color = c;
                            true
                        },
                        cx,
                    )
                }))
                .into_any_element()
        });
        let fills = std::iter::once(
            chip(("prop-fill-none", 0), "None".into(), last.fill.is_none())
                .on_click(cx.listener(|v, _, _, cx| {
                    v.restyle(
                        "No fill",
                        |a| {
                            a.fill = None;
                            a.kind.fillable()
                        },
                        cx,
                    )
                }))
                .into_any_element(),
        )
        .chain(COLORS.iter().enumerate().map(|(i, &(c, name))| {
            swatch(("prop-fill", i), c, last.fill == Some(c))
                .tooltip(Tooltip::text(name))
                .on_click(cx.listener(move |v, _, _, cx| {
                    v.restyle(
                        "Fill",
                        |a| {
                            a.fill = Some(c);
                            a.kind.fillable()
                        },
                        cx,
                    )
                }))
                .into_any_element()
        }));
        let opacities = OPACITIES.iter().enumerate().map(|(i, &o)| {
            chip(
                ("prop-opacity", i),
                format!("{}%", (o * 100.0) as u32),
                (last.opacity - o).abs() < 0.01,
            )
            .on_click(cx.listener(move |v, _, _, cx| {
                v.restyle(
                    &format!("Opacity {}%", (o * 100.0) as u32),
                    |a| {
                        a.opacity = o;
                        true
                    },
                    cx,
                )
            }))
            .into_any_element()
        });
        let widths = WIDTHS.iter().enumerate().map(|(i, &w)| {
            chip(
                ("prop-width", i),
                format!("{w}"),
                (last.stroke - w).abs() < 0.01,
            )
            .on_click(cx.listener(move |v, _, _, cx| {
                v.restyle(
                    &format!("Line width {w}"),
                    |a| {
                        a.stroke = w;
                        a.kind.stroked()
                    },
                    cx,
                )
            }))
            .into_any_element()
        });
        let dashed = last.dashed;
        let (start_on, end_on) = match &last.kind {
            AnnotKind::Line {
                arrow, arrow_start, ..
            } => (*arrow_start, *arrow),
            _ => (false, false),
        };
        let arrow_chip = |id: usize, text: &'static str, on: bool, start: bool| {
            chip(("prop-arrow", id), text.into(), on).on_click(cx.listener(move |v, _, _, cx| {
                v.restyle(
                    "Arrowheads",
                    |a| match &mut a.kind {
                        AnnotKind::Line {
                            arrow, arrow_start, ..
                        } => {
                            if start {
                                *arrow_start = !on;
                            } else {
                                *arrow = !on;
                            }
                            true
                        }
                        _ => false,
                    },
                    cx,
                )
            }))
        };
        let size_step = |id: usize, text: &'static str, by: f32| {
            chip(("prop-size", id), text.into(), false).on_click(cx.listener(move |v, _, _, cx| {
                v.restyle(
                    "Text size",
                    |a| match a.kind.text_size() {
                        Some(s) => {
                            a.kind = a.kind.with_text_size(s + by);
                            true
                        }
                        None => false,
                    },
                    cx,
                )
            }))
        };
        Some(
            div()
                .id("mark-properties")
                .flex()
                .flex_wrap()
                .items_center()
                .gap_1()
                .px_2()
                .py_1()
                .bg(theme::PANEL)
                .border_b_1()
                .border_color(theme::DIVIDER)
                .text_xs()
                .text_color(theme::TEXT)
                .child(label("Color"))
                .children(colors)
                .when(fillable, |d| {
                    d.child(div().w(px(8.0)))
                        .child(label("Fill"))
                        .children(fills)
                })
                .child(div().w(px(8.0)))
                .child(label("Opacity"))
                .children(opacities)
                .when(stroked, |d| {
                    d.child(div().w(px(8.0)))
                        .child(label("Width"))
                        .children(widths)
                        .child(chip(("prop-dash", 0), "Dashed".into(), dashed).on_click(
                            cx.listener(move |v, _, _, cx| {
                                v.restyle(
                                    if dashed { "Solid line" } else { "Dashed line" },
                                    |a| {
                                        a.dashed = !dashed;
                                        a.kind.stroked()
                                    },
                                    cx,
                                )
                            }),
                        ))
                })
                .when(cloudable, |d| {
                    d.child(
                        chip(("prop-cloud", 0), "Cloudy".into(), cloudy).on_click(cx.listener(
                            move |v, _, _, cx| {
                                v.restyle(
                                    if cloudy {
                                        "Plain border"
                                    } else {
                                        "Cloudy border"
                                    },
                                    |a| {
                                        a.cloudy = !cloudy;
                                        a.kind.cloudable()
                                    },
                                    cx,
                                )
                            },
                        )),
                    )
                })
                .when_some(icon, |d, current| {
                    d.child(div().w(px(8.0))).child(label("Icon")).children(
                        sheaf_doc::annot::ATTACHMENT_ICONS
                            .iter()
                            .enumerate()
                            .map(|(i, name)| {
                                chip(("prop-icon", i), (*name).into(), current == *name).on_click(
                                    cx.listener(move |v, _, _, cx| {
                                        v.attach_icon = name.to_string();
                                        v.restyle(
                                            "Attachment icon",
                                            |a| match &mut a.kind {
                                                AnnotKind::FileAttachment { icon, .. } => {
                                                    *icon = name.to_string();
                                                    true
                                                }
                                                _ => false,
                                            },
                                            cx,
                                        )
                                    }),
                                )
                            }),
                    )
                })
                .when(lines, |d| {
                    d.child(div().w(px(8.0)))
                        .child(label("Arrows"))
                        .child(arrow_chip(0, "◀ start", start_on, true))
                        .child(arrow_chip(1, "end ▶", end_on, false))
                })
                .when_some(text_size, |d, s| {
                    d.child(div().w(px(8.0)))
                        .child(label("Text"))
                        .child(size_step(0, "−", -1.0))
                        .child(div().px_1().child(format!("{s:.0} pt")))
                        .child(size_step(1, "+", 1.0))
                })
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_tool_has_its_own_key_and_a_swatch_for_a_default() {
        let ids: std::collections::HashSet<&str> = Tool::ALL.iter().map(|t| t.id()).collect();
        assert_eq!(ids.len(), Tool::ALL.len());
        assert_eq!(Tool::Highlight.default_color(), [255, 230, 0]);
        assert_eq!(Tool::StrikeOut.default_color(), [220, 40, 40]);
        assert_eq!(Tool::TextEdit.default_color(), [0, 120, 255]);
        // Each default is a swatch, so the toolbar shows which is the tool's.
        for t in Tool::ALL {
            assert!(COLORS.iter().any(|(c, _)| *c == t.default_color()), "{t:?}");
        }
    }

    /// Copied marks take their stamps' pages and files with them, and a
    /// paste gives them to a document that has neither.
    #[test]
    fn copied_marks_take_what_they_hold() {
        use sheaf_doc::forms::fixture::{self, MARK_FILE, MARK_FILE_BYTES, MARK_STAMP};
        let every = fixture::every_mark_kind();
        let marks = [every[17].clone(), every[23].clone(), every[1].clone()];
        let held = CopiedAssets::of(&marks, &fixture::mark_assets());
        assert_eq!((held.stamps.len(), held.files.len()), (1, 1));
        let other = MarkAssets::new(sheaf_doc::Store::new());
        assert!(other.stamp(MARK_STAMP).is_none());
        held.give(&marks, &other);
        assert!(other.stamp_pdf(MARK_STAMP).is_some());
        assert_eq!(&*other.file_bytes(MARK_FILE).unwrap(), MARK_FILE_BYTES);
    }

    fn mark(kind: AnnotKind) -> Annot {
        Annot {
            id: AnnotId(1),
            kind,
            color: [0, 0, 0],
            opacity: 1.0,
            stroke: 1.0,
            author: String::new(),
            contents: String::new(),
            modified: String::new(),
            created: String::new(),
            fill: None,
            dashed: false,
            cloudy: false,
            turn: Rotation::R0,
            name: String::new(),
        }
    }

    /// The rectangle's box, to a hundredth of a point.
    fn rect_of(a: &Annot) -> (f32, f32, f32, f32) {
        let r = |v: f32| (v * 100.0).round() / 100.0;
        match a.kind {
            AnnotKind::Square { rect: b } => (r(b.x), r(b.y), r(b.w), r(b.h)),
            _ => panic!(),
        }
    }

    #[test]
    fn a_corner_handle_keeps_the_opposite_corner() {
        let r = Resize {
            spot: Spot::Box { fx: 1.0, fy: 1.0 },
            orig: mark(AnnotKind::Square {
                rect: Box2 {
                    x: 10.0,
                    y: 10.0,
                    w: 100.0,
                    h: 50.0,
                },
            }),
        };
        assert_eq!(
            rect_of(&reshaped(&r, Pt { x: 60.0, y: 110.0 }, false)),
            (10.0, 10.0, 50.0, 100.0)
        );
        // Shift keeps the proportions, by the larger stretch.
        assert_eq!(
            rect_of(&reshaped(&r, Pt { x: 60.0, y: 110.0 }, true)),
            (10.0, 10.0, 200.0, 100.0)
        );
        // Dragged past the anchor, it flips rather than going negative.
        assert_eq!(
            rect_of(&reshaped(&r, Pt { x: 0.0, y: 0.0 }, false)),
            (0.0, 0.0, 10.0, 10.0)
        );
    }

    #[test]
    fn a_side_handle_moves_one_side() {
        let r = Resize {
            spot: Spot::Box { fx: 0.0, fy: 0.5 },
            orig: mark(AnnotKind::Square {
                rect: Box2 {
                    x: 10.0,
                    y: 10.0,
                    w: 100.0,
                    h: 50.0,
                },
            }),
        };
        assert_eq!(
            rect_of(&reshaped(&r, Pt { x: 50.0, y: 999.0 }, false)),
            (50.0, 10.0, 60.0, 50.0)
        );
    }

    #[test]
    fn shift_snaps_a_line_end_to_45_degrees() {
        let r = Resize {
            spot: Spot::End(1),
            orig: mark(AnnotKind::Line {
                a: Pt { x: 0.0, y: 0.0 },
                b: Pt { x: 10.0, y: 0.0 },
                arrow: false,
                arrow_start: false,
            }),
        };
        let AnnotKind::Line { b, .. } = reshaped(&r, Pt { x: 100.0, y: 10.0 }, true).kind else {
            panic!()
        };
        assert!(b.y.abs() < 1e-3 && (b.x - 100.5).abs() < 0.1, "{b:?}");
    }

    #[test]
    fn vertex_and_callout_handles_move_their_own_part() {
        let poly = mark(AnnotKind::Polygon {
            points: vec![
                Pt { x: 0.0, y: 0.0 },
                Pt { x: 10.0, y: 0.0 },
                Pt { x: 5.0, y: 10.0 },
            ],
        });
        assert_eq!(handle_spots(&poly).len(), 3);
        let moved = reshaped(
            &Resize {
                spot: Spot::Vertex(2),
                orig: poly,
            },
            Pt { x: 5.0, y: 20.0 },
            false,
        );
        let AnnotKind::Polygon { points } = moved.kind else {
            panic!()
        };
        assert_eq!(points[2], Pt { x: 5.0, y: 20.0 });

        let callout = mark(AnnotKind::Callout {
            rect: Box2 {
                x: 100.0,
                y: 100.0,
                w: 100.0,
                h: 30.0,
            },
            knee: Pt { x: 80.0, y: 115.0 },
            point: Pt { x: 40.0, y: 160.0 },
            size: 12.0,
        });
        // Eight on the box, then the point and the knee.
        assert_eq!(handle_spots(&callout).len(), 10);
        let wider = reshaped(
            &Resize {
                spot: Spot::Box { fx: 1.0, fy: 0.5 },
                orig: callout,
            },
            Pt { x: 250.0, y: 0.0 },
            false,
        );
        let AnnotKind::Callout {
            rect, knee, point, ..
        } = wider.kind
        else {
            panic!()
        };
        assert_eq!((rect.x, rect.w), (100.0, 150.0));
        assert_eq!(
            (knee, point),
            (Pt { x: 80.0, y: 115.0 }, Pt { x: 40.0, y: 160.0 })
        );
    }

    /// A page turned `degrees`, its media box off the origin and its crop
    /// box inset from it.
    fn turned(degrees: i64) -> Geom {
        use sheaf_doc::geom::Rect;
        Geom::read(
            Some(Rect::new(-37.0, 52.0, 575.0, 844.0)),
            Some(Rect::new(-17.0, 62.0, 555.0, 824.0)),
            degrees,
        )
    }

    /// Within 1/64 pt.
    fn near(a: Box2, b: Box2) -> bool {
        [a.x - b.x, a.y - b.y, a.w - b.w, a.h - b.h]
            .iter()
            .all(|d| d.abs() <= 1.0 / 64.0)
    }

    fn square(rect: Box2) -> Annot {
        mark(AnnotKind::Square { rect })
    }

    /// Whatever the page's turn, a mark moves on the page as the pointer
    /// does in the edit frame, and keeps its own turn.
    #[test]
    fn a_drag_moves_a_mark_as_the_page_shows() {
        for degrees in [0, 90, 180, 270] {
            let edit = turned(degrees);
            let a = Annot {
                turn: Rotation::R90,
                ..square(Box2 {
                    x: 100.0,
                    y: 200.0,
                    w: 40.0,
                    h: 20.0,
                })
            };
            let before = edit.to_view().apply_box(a.page_bounds());
            let moved = dragged(
                &a.to_frame(&edit),
                Pt { x: 10.0, y: 10.0 },
                Pt { x: 25.0, y: 40.0 },
            )
            .to_page(&edit);
            assert_eq!(moved.turn, a.turn, "{degrees}");
            let after = edit.to_view().apply_box(moved.page_bounds());
            let want = Box2 {
                x: before.x + 15.0,
                y: before.y + 30.0,
                ..before
            };
            assert!(near(after, want), "{degrees}: {after:?}");
        }
    }

    /// A corner handle reshapes a mark in the edit frame, so on a turned
    /// page the corner dragged is the one under the pointer, and the mark
    /// back in PageSpace keeps its turn.
    #[test]
    fn a_handle_reshapes_a_mark_as_the_page_shows() {
        for degrees in [0, 90, 180, 270] {
            let edit = turned(degrees);
            let a = Annot {
                turn: edit.rotate,
                ..square(Box2 {
                    x: 100.0,
                    y: 200.0,
                    w: 40.0,
                    h: 20.0,
                })
            };
            let shown = a.to_frame(&edit);
            assert_eq!(shown.turn, Rotation::R0, "{degrees}");
            let b = shown.kind.bounds();
            // The bottom-right corner as shown, dragged 10 pt right and down.
            let (spot, at) = handle_spots(&shown)[4];
            assert_eq!(
                at,
                Pt {
                    x: b.right(),
                    y: b.bottom()
                },
                "{degrees}"
            );
            let r = Resize { spot, orig: shown };
            let back = reshaped(
                &r,
                Pt {
                    x: at.x + 10.0,
                    y: at.y + 10.0,
                },
                false,
            )
            .to_page(&edit);
            assert_eq!(back.turn, edit.rotate);
            let want = Box2 {
                w: b.w + 10.0,
                h: b.h + 10.0,
                ..b
            };
            let got = edit.to_view().apply_box(back.page_bounds());
            assert!(near(got, want), "{degrees}: {got:?}");
        }
    }

    /// A mark drawn in the edit frame lands where it was drawn, and reads
    /// the way it was drawn: an arrow drawn rightwards runs rightwards as
    /// laid out, whatever the page's turn.
    #[test]
    fn a_mark_drawn_on_a_turned_page_reads_as_drawn() {
        for degrees in [0, 90, 180, 270] {
            let edit = turned(degrees);
            let (from, to) = (Pt { x: 50.0, y: 80.0 }, Pt { x: 150.0, y: 80.0 });
            let drawn = mark(AnnotKind::Line {
                a: from,
                b: to,
                arrow: true,
                arrow_start: false,
            });
            let m = drawn.to_page(&edit);
            assert_eq!(m.turn, edit.rotate, "{degrees}");
            let AnnotKind::Line { a, b, .. } = m.mapped(&edit.to_view()).kind else {
                panic!()
            };
            let back = Box2::from_corners(a, b);
            assert!(
                near(back, Box2::from_corners(from, to)),
                "{degrees}: {back:?}"
            );
            let AnnotKind::Line { a, b, .. } = m.upright() else {
                panic!()
            };
            assert!(
                (a.y - b.y).abs() < 1e-3 && b.x - a.x > 99.0,
                "{degrees}: {a:?} {b:?}"
            );
        }
    }

    /// Marks pasted onto a page with another `/Rotate` and crop land where
    /// they were seen on theirs, or 10 pt down and right of there as seen,
    /// and keep their turn relative to the page, either way between the
    /// two.
    #[test]
    fn paste_keeps_the_place_as_seen_across_rotate_and_crop() {
        use sheaf_doc::geom::Rect;
        let page_a = Geom::read(
            Some(Rect::new(0.0, 0.0, 612.0, 792.0)),
            Some(Rect::new(40.0, 60.0, 572.0, 752.0)),
            90,
        );
        let page_b = Geom::read(Some(Rect::new(-100.0, 200.0, 512.0, 992.0)), None, 0);
        let marks = [
            Annot {
                turn: Rotation::R90,
                ..square(Box2 {
                    x: 100.0,
                    y: 200.0,
                    w: 40.0,
                    h: 20.0,
                })
            },
            Annot {
                turn: Rotation::R180,
                ..mark(AnnotKind::Line {
                    a: Pt { x: 300.0, y: 400.0 },
                    b: Pt { x: 350.0, y: 480.0 },
                    arrow: true,
                    arrow_start: false,
                })
            },
            mark(AnnotKind::Note {
                pos: Pt { x: 500.0, y: 100.0 },
            }),
        ];
        for (from, onto) in [(&page_a, &page_b), (&page_b, &page_a)] {
            let clip = copied(marks.clone(), from);
            for shift in [0.0, 10.0] {
                for (was, now) in marks.iter().zip(pasted(clip.clone(), onto, shift)) {
                    let seen = from.to_view().apply_box(was.page_bounds());
                    let want = Box2 {
                        x: seen.x + shift,
                        y: seen.y + shift,
                        ..seen
                    };
                    let got = onto.to_view().apply_box(now.page_bounds());
                    assert!(near(got, want), "{shift}: {got:?} not {want:?}");
                    assert_eq!(
                        now.turn.minus(onto.rotate),
                        was.turn.minus(from.rotate),
                        "{shift}"
                    );
                }
            }
        }
    }
}
