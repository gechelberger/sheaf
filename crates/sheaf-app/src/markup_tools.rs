//! Markup tools: a tool mode, mouse handling on the page canvas, live
//! previews while dragging, selection/move/delete of our annotations, and
//! path-based drawing for lines and ink.
//!
//! Coordinates: the page container reports window positions; those become
//! page-local logical px, then points in the page's *displayed* orientation,
//! then points in the source page's own orientation (what the model stores).

use gpui::{
    AnyElement, Context, CursorStyle, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PathBuilder, Pixels, Point, Window, canvas, div, prelude::*, px, rgb,
};
use sheaf_core::{PageSize, Rotation};
use sheaf_doc::{Annot, AnnotId, AnnotKind, Box2, Edit, PageId, Pt};
use sheaf_render::Rect;
use sheaf_ui::theme;
use sheaf_ui::tooltip::Tooltip;

use crate::docview::{DocView, State, rotate_rect};

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
        }
    }

    pub const ALL: [Tool; 21] = [
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
    ];
}

/// Colors offered in the toolbar, with their tooltip names.
pub const COLORS: &[([u8; 3], &str)] = &[
    ([255, 230, 0], "Yellow"),
    ([220, 40, 40], "Red"),
    ([0, 120, 255], "Blue"),
    ([0, 150, 0], "Green"),
    ([255, 140, 0], "Orange"),
    ([160, 60, 220], "Purple"),
    ([20, 20, 20], "Black"),
];

/// A mouse drag in progress on a page.
#[derive(Debug, Clone)]
pub struct Drag {
    pub page: usize,
    pub page_id: PageId,
    /// Points in source-page space.
    pub start: Pt,
    pub current: Pt,
    /// Every sampled point (ink).
    pub path: Vec<Pt>,
    /// Marks being moved, as they were: the one grabbed and the others
    /// selected on its page.
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
    pub orig: Annot,
}

/// A mark's handles, in source-page space.
pub fn handle_spots(a: &Annot) -> Vec<(Spot, Pt)> {
    use sheaf_doc::annot::Handles;
    match a.kind.handles() {
        Handles::None => Vec::new(),
        Handles::Ends => match &a.kind {
            AnnotKind::Line { a: pa, b: pb, .. } => vec![(Spot::End(0), *pa), (Spot::End(1), *pb)],
            _ => Vec::new(),
        },
        Handles::Vertices => match &a.kind {
            AnnotKind::Polygon { points } | AnnotKind::Polyline { points } => points
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
        Handles::Box => box_spots(a.bounds()),
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
                _ => r.orig.bounds(),
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
            AnnotKind::Polygon { points } | AnnotKind::Polyline { points } => {
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

/// Map a point in the page's displayed (rotated) space back to the source
/// page's own orientation.
pub fn unrotate_pt(p: Pt, src: PageSize, rot: Rotation) -> Pt {
    match rot {
        Rotation::R0 => p,
        Rotation::R90 => Pt {
            x: p.y,
            y: src.height - p.x,
        },
        Rotation::R180 => Pt {
            x: src.width - p.x,
            y: src.height - p.y,
        },
        Rotation::R270 => Pt {
            x: src.width - p.y,
            y: p.x,
        },
    }
}

pub fn rotate_pt(p: Pt, src: PageSize, rot: Rotation) -> Pt {
    match rot {
        Rotation::R0 => p,
        Rotation::R90 => Pt {
            x: src.height - p.y,
            y: p.x,
        },
        Rotation::R180 => Pt {
            x: src.width - p.x,
            y: src.height - p.y,
        },
        Rotation::R270 => Pt {
            x: p.y,
            y: src.width - p.x,
        },
    }
}

fn translate(kind: &AnnotKind, dx: f32, dy: f32) -> AnnotKind {
    kind.translated(dx, dy)
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
        let b = a.bounds();
        let Some(index) = doc.model.state().index_of(page_id) else {
            return;
        };
        let Some(pref) = doc.shown_ref(index) else {
            return;
        };
        let Some(src) = doc
            .model
            .source(pref.source)
            .map(|s| s.page_size(pref.index))
        else {
            return;
        };
        let r = rotate_rect(
            Rect {
                x: b.x,
                y: b.y,
                w: b.w,
                h: b.h,
            },
            src,
            pref.rotation,
        );
        let ps = src.rotated(pref.rotation);
        doc.scroll_to(crate::docview::ScrollTo::Reveal {
            page: index,
            fx: (r.x + r.w / 2.0) / ps.width.max(1.0),
            fy: (r.y + r.h / 2.0) / ps.height.max(1.0),
        });
        doc.select_mark(Some((page_id, id)));
        doc.notice = Some(format!("Page {}", index + 1));
        cx.notify();
    }

    /// Window position → (page index, point in source-page space, the
    /// page's scale in px per point, for hit areas that stay the same size
    /// on screen).
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
        let pref = layout.refs[i];
        let src = doc
            .model
            .source(pref.source)
            .map(|s| s.page_size(pref.index))?;
        let k = w / src.rotated(pref.rotation).width.max(1.0);
        let disp = Pt {
            x: (x - left) / k,
            y: (y - top) / k,
        };
        Some((i, unrotate_pt(disp, src, pref.rotation), k))
    }

    /// The topmost of our marks on page `page_id` under `p`.
    pub(crate) fn mark_at(&self, page_id: PageId, p: Pt, tolerance: f32) -> Option<Annot> {
        let State::Ready(doc) = &self.state else {
            return None;
        };
        // A mark the comment filter hides can't be clicked.
        if doc.comments_hidden {
            return None;
        }
        doc.model.state().annots_on(page_id).and_then(|list| {
            list.iter()
                .rev()
                .filter(|a| !doc.hidden_marks.contains(&a.id))
                .find(|a| a.hit(p, tolerance))
                .cloned()
        })
    }

    /// The handle of the selected mark under `p`, when one mark is selected
    /// and it's on this page.
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
        // Shift+click on a mark adds it to the selection (or takes it out);
        // Shift+drag elsewhere selects a region, whatever the tool.
        if ev.modifiers.shift {
            if self.tool == Tool::Select
                && let Some(a) = &mark
                && let State::Ready(doc) = &mut self.state
            {
                doc.toggle_mark((page_id, a.id));
                cx.notify();
                return;
            }
            self.start_region(page, p, cx);
            return;
        }
        self.clear_region(cx);
        // A calculator tape waiting to be placed takes the click, whatever
        // the tool.
        if self.place_tape_at(page_id, p, cx) {
            return;
        }
        // Picking numbers into the calculator takes the click too.
        if self.pick_number_at(page_id, p, tolerance, cx) {
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
                // Everything selected on this page moves together.
                let moving: Vec<Annot> = doc
                    .selected_mark_list()
                    .into_iter()
                    .filter(|(pid, _)| *pid == page_id)
                    .map(|(_, m)| m)
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
            if self.tool == Tool::Select {
                // Away from any mark: select text.
                self.start_text_drag(page_id, page, p, tolerance, ev.click_count, cx);
                return;
            }
        }
        // The text tools select text when the press is on it, as the
        // Select tool does; letting go marks the selection up.
        if self.tool.marks_text() {
            if self.over_text(page_id, p, tolerance) {
                self.start_text_drag(page_id, page, p, tolerance, ev.click_count, cx);
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
        let color = self.tool_color;
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
            Tool::Polygon | Tool::Polyline | Tool::Cloud => {
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
                self.hover_page(page_id, p, 4.0 / k.max(0.01), cx);
            }
            self.shape_hover(page, p, cx);
            return;
        }
        if ev.pressed_button != Some(MouseButton::Left) {
            return;
        }
        if self.drag_region(page, p, cx) {
            return;
        }
        if self.move_text_drag(page, p, cx) {
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
        let color = self.tool_color;
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let Some(drag) = doc.drag.take() else { return };
        let page_id = drag.page_id;
        let (a, b) = (drag.start, drag.current);
        if let Some(r) = &drag.resize {
            let mut changed = reshaped(r, b, drag.shift);
            if changed != r.orig {
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
                .into_iter()
                .map(|orig| {
                    let mut moved = orig.clone();
                    moved.kind = translate(&orig.kind, dx, dy);
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
            Tool::Polygon | Tool::Polyline | Tool::Cloud => {}
            Tool::Highlight | Tool::Underline | Tool::StrikeOut | Tool::Squiggly => {
                if too_small {
                    return;
                }
                self.markup_from_drag(tool, page_id, rect, cx);
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
    /// engine's, or an unsaved OCR layer's). Where there's no text, a
    /// highlight covers the area; the line markups do nothing.
    fn markup_from_drag(
        &mut self,
        tool: Tool,
        page_id: PageId,
        rect: Box2,
        cx: &mut Context<Self>,
    ) {
        self.with_chars(page_id, cx, move |view, chars, cx| {
            let mut rects = chars.map(|c| c.rects_in(rect)).unwrap_or_default();
            if rects.is_empty() {
                if tool != Tool::Highlight {
                    if let State::Ready(doc) = &mut view.state {
                        doc.notice = Some(format!("{}: no text there", tool.label()));
                    }
                    cx.notify();
                    return;
                }
                rects = vec![rect];
            }
            view.add_text_markup(tool, page_id, rects, cx);
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

    /// The markup color swatches, and their length along a toolbar.
    pub(crate) fn render_colors(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let color = self.tool_color;
        let colors: Vec<_> = COLORS
            .iter()
            .map(|&(c, name)| {
                let active = c == color;
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
                        v.tool_color = c;
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

    /// Live preview of the current drag on `page`, in page-local logical px.
    pub(crate) fn drag_preview(
        doc: &crate::docview::Doc,
        tool: Tool,
        color: [u8; 3],
        page: usize,
        src: PageSize,
        rot: Rotation,
        k: f32,
    ) -> Option<AnyElement> {
        let drag = doc.drag.as_ref()?;
        if drag.page != page {
            return None;
        }
        let c = rgb(((color[0] as u32) << 16) | ((color[1] as u32) << 8) | color[2] as u32);
        let (a, b) = (drag.start, drag.current);
        // Moving or reshaping marks, whatever the tool (Ctrl+drag).
        let shown: Vec<Annot> = match &drag.resize {
            Some(r) => vec![reshaped(r, b, drag.shift)],
            None => drag
                .moving
                .iter()
                .map(|orig| {
                    let mut moved = orig.clone();
                    moved.kind = translate(&orig.kind, b.x - a.x, b.y - a.y);
                    moved
                })
                .collect(),
        };
        if !shown.is_empty() {
            return Some(
                div()
                    .absolute()
                    .inset_0()
                    .children(
                        shown
                            .iter()
                            .flat_map(|m| crate::docview::annot_elements(m, src, rot, k)),
                    )
                    .into_any_element(),
            );
        }
        match tool {
            Tool::Select => None,
            Tool::Line | Tool::Arrow => {
                let pa = rotate_pt(a, src, rot);
                let pb = rotate_pt(b, src, rot);
                Some(stroke_canvas(
                    vec![vec![(pa.x * k, pa.y * k), (pb.x * k, pb.y * k)]],
                    c,
                    1.5 * k,
                    src.rotated(rot),
                    k,
                ))
            }
            Tool::Ink => {
                let pts: Vec<(f32, f32)> = drag
                    .path
                    .iter()
                    .map(|p| {
                        let q = rotate_pt(*p, src, rot);
                        (q.x * k, q.y * k)
                    })
                    .collect();
                Some(stroke_canvas(vec![pts], c, 2.0 * k, src.rotated(rot), k))
            }
            Tool::TextEdit => None,
            Tool::Highlight
            | Tool::Underline
            | Tool::StrikeOut
            | Tool::Squiggly
            | Tool::FreeText
            | Tool::Square
            | Tool::Ellipse => {
                let r = rotate_rect(
                    Rect {
                        x: a.x.min(b.x),
                        y: a.y.min(b.y),
                        w: (a.x - b.x).abs(),
                        h: (a.y - b.y).abs(),
                    },
                    src,
                    rot,
                );
                let el = div()
                    .absolute()
                    .left(px(r.x * k))
                    .top(px(r.y * k))
                    .w(px(r.w * k))
                    .h(px(r.h * k));
                Some(match tool {
                    Tool::Highlight => el.bg(theme::HIGHLIGHT_PREVIEW).into_any_element(),
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
            Tool::Polygon | Tool::Polyline | Tool::Cloud => None,
            Tool::Callout => {
                let (pa, pb) = (rotate_pt(a, src, rot), rotate_pt(b, src, rot));
                Some(stroke_canvas(
                    vec![vec![(pa.x * k, pa.y * k), (pb.x * k, pb.y * k)]],
                    c,
                    1.5 * k,
                    src.rotated(rot),
                    k,
                ))
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
    /// Bumped if this changes incompatibly.
    sheaf_marks: u32,
    marks: Vec<Annot>,
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
        let marks: Vec<Annot> = doc
            .selected_mark_list()
            .into_iter()
            .map(|(_, a)| a)
            .filter(|a| a.thread_parent().is_none())
            .collect();
        if marks.is_empty() {
            return false;
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
                sheaf_marks: 1,
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

    /// Paste copied marks onto the current page: a little down and right of
    /// where they were, or exactly there (`in_place`). They come in
    /// selected, as one undo step.
    pub(crate) fn paste_marks(&mut self, in_place: bool, cx: &mut Context<Self>) -> bool {
        let Some(copied) = cx.read_from_clipboard().and_then(|item| {
            item.entries().iter().find_map(|e| match e {
                gpui::ClipboardEntry::String(s) => s.metadata_json::<CopiedMarks>(),
                _ => None,
            })
        }) else {
            return false;
        };
        if copied.sheaf_marks != 1 || copied.marks.is_empty() {
            return false;
        }
        let me = crate::prefs::get(cx).identity.name();
        let State::Ready(doc) = &mut self.state else {
            return false;
        };
        let Some(page) = doc.page_ref(doc.current_page()).map(|p| p.id) else {
            return false;
        };
        let before = doc.model.state().annots_on(page).map_or(0, |l| l.len());
        let shift = if in_place { 0.0 } else { 10.0 };
        let now = sheaf_doc::pdf_date_now();
        let n = copied.marks.len();
        let edits = copied
            .marks
            .into_iter()
            .map(|mut a| {
                a.kind = a.kind.translated(shift, shift);
                // A pasted connector is on its own: its partner stayed put.
                if let AnnotKind::Connector { partner, .. } = &mut a.kind {
                    *partner = None;
                }
                a.author = me.clone();
                a.created = now.clone();
                a.modified = now.clone();
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
}
