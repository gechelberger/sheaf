//! Markup tools: a tool mode, mouse handling on the page canvas, live
//! previews while dragging, selection/move/delete of our annotations, and
//! path-based drawing for lines and ink.
//!
//! Coordinates: the page container reports window positions; those become
//! page-local logical px, then points in the page's *displayed* orientation,
//! then points in the source page's own orientation (what the model stores).

use gpui::{
    AnyElement, Context, CursorStyle, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PathBuilder, Pixels, Point, Window, canvas, div, prelude::*, px, rgb, rgba, svg,
};
use sheaf_core::{PageSize, Rotation};
use sheaf_doc::{Annot, AnnotId, AnnotKind, Box2, Edit, PageId, Pt, annot::CONNECTOR_SIZE};
use sheaf_render::Rect;

use crate::docview::{DocView, State, rotate_rect};
use crate::tooltip::Tooltip;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Select,
    Note,
    Highlight,
    FreeText,
    Square,
    Ellipse,
    Line,
    Arrow,
    Ink,
    Tick,
    /// PDFlyer connector: two clicks place a linked pair of markers.
    Connector,
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Note => "Note",
            Tool::Highlight => "Highlight",
            Tool::FreeText => "Text",
            Tool::Square => "Rect",
            Tool::Ellipse => "Ellipse",
            Tool::Line => "Line",
            Tool::Arrow => "Arrow",
            Tool::Ink => "Pencil",
            Tool::Tick => "Tick",
            Tool::Connector => "Connector",
        }
    }

    /// Embedded artwork path (see `assets.rs`).
    pub fn icon(self) -> &'static str {
        match self {
            Tool::Select => "icons/select.svg",
            Tool::Note => "icons/note.svg",
            Tool::Highlight => "icons/highlight.svg",
            Tool::FreeText => "icons/text.svg",
            Tool::Square => "icons/rectangle.svg",
            Tool::Ellipse => "icons/ellipse.svg",
            Tool::Line => "icons/line.svg",
            Tool::Arrow => "icons/arrow.svg",
            Tool::Ink => "icons/pencil.svg",
            Tool::Tick => "icons/tick.svg",
            Tool::Connector => "icons/connector.svg",
        }
    }

    /// Tooltip title; fuller than the undo-history `label`.
    pub fn title(self) -> &'static str {
        match self {
            Tool::Note => "Sticky Note",
            Tool::FreeText => "Free Text",
            Tool::Square => "Rectangle",
            Tool::Tick => "Tickmark",
            t => t.label(),
        }
    }

    /// Tooltip flavor text.
    pub fn description(self) -> &'static str {
        match self {
            Tool::Select => "Select, move, and delete annotations",
            Tool::Note => "Click to pin a comment to the page",
            Tool::Highlight => "Drag across text to highlight it",
            Tool::FreeText => "Drag a box, then type text onto the page",
            Tool::Square => "Drag to draw a rectangle",
            Tool::Ellipse => "Drag to draw an ellipse",
            Tool::Line => "Drag to draw a straight line",
            Tool::Arrow => "Drag to draw an arrow",
            Tool::Ink => "Draw freehand",
            Tool::Tick => "Click to stamp the selected tickmark",
            Tool::Connector => "Click twice to place a linked pair of markers",
        }
    }

    /// The action that selects this tool; its binding is the shortcut shown.
    pub fn action(self) -> Box<dyn gpui::Action> {
        use crate::*;
        match self {
            Tool::Select => Box::new(ToolSelect),
            Tool::Note => Box::new(ToolNote),
            Tool::Highlight => Box::new(ToolHighlight),
            Tool::FreeText => Box::new(ToolFreeText),
            Tool::Square => Box::new(ToolSquare),
            Tool::Ellipse => Box::new(ToolEllipse),
            Tool::Line => Box::new(ToolLine),
            Tool::Arrow => Box::new(ToolArrow),
            Tool::Ink => Box::new(ToolInk),
            Tool::Tick => Box::new(ToolTick),
            Tool::Connector => Box::new(ToolConnector),
        }
    }

    pub const ALL: [Tool; 11] = [
        Tool::Select,
        Tool::Note,
        Tool::Highlight,
        Tool::FreeText,
        Tool::Square,
        Tool::Ellipse,
        Tool::Line,
        Tool::Arrow,
        Tool::Ink,
        Tool::Tick,
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
    /// Select tool: the annotation being moved, as it was.
    pub moving: Option<Annot>,
}

/// First end of a connector pair, placed but not yet committed; the pair is
/// added as one edit when the second end lands.
#[derive(Debug, Clone)]
pub struct PendingConnector {
    pub page_id: PageId,
    pub rect: Box2,
    pub label: String,
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
    let mv = |p: &Pt| Pt {
        x: p.x + dx,
        y: p.y + dy,
    };
    let mb = |b: &Box2| Box2 {
        x: b.x + dx,
        y: b.y + dy,
        w: b.w,
        h: b.h,
    };
    match kind {
        AnnotKind::Highlight { rects } => AnnotKind::Highlight {
            rects: rects.iter().map(mb).collect(),
        },
        AnnotKind::Note { pos } => AnnotKind::Note { pos: mv(pos) },
        AnnotKind::FreeText { rect, size } => AnnotKind::FreeText {
            rect: mb(rect),
            size: *size,
        },
        AnnotKind::Square { rect } => AnnotKind::Square { rect: mb(rect) },
        AnnotKind::Ellipse { rect } => AnnotKind::Ellipse { rect: mb(rect) },
        AnnotKind::Line { a, b, arrow } => AnnotKind::Line {
            a: mv(a),
            b: mv(b),
            arrow: *arrow,
        },
        AnnotKind::Ink { paths } => AnnotKind::Ink {
            paths: paths.iter().map(|p| p.iter().map(mv).collect()).collect(),
        },
        AnnotKind::Stamp { rect, glyph } => AnnotKind::Stamp {
            rect: mb(rect),
            glyph: glyph.clone(),
        },
        AnnotKind::Connector {
            rect,
            label,
            partner,
        } => AnnotKind::Connector {
            rect: mb(rect),
            label: label.clone(),
            partner: *partner,
        },
        AnnotKind::Tape { rect, size } => AnnotKind::Tape {
            rect: mb(rect),
            size: *size,
        },
    }
}

fn connector_rect(center: Pt) -> Box2 {
    Box2 {
        x: center.x - CONNECTOR_SIZE / 2.0,
        y: center.y - CONNECTOR_SIZE / 2.0,
        w: CONNECTOR_SIZE,
        h: CONNECTOR_SIZE,
    }
}

impl DocView {
    pub(crate) fn set_tool(&mut self, tool: Tool, cx: &mut Context<Self>) {
        self.tool = tool;
        if let State::Ready(doc) = &mut self.state {
            doc.drag = None;
            if tool != Tool::Select {
                doc.selected_annot = None;
            }
            if tool != Tool::Connector && doc.pending_connector.take().is_some() {
                doc.notice = Some("Connector cancelled".into());
            }
        }
        cx.notify();
    }

    /// Drop a half-placed connector. Returns whether there was one.
    pub(crate) fn cancel_pending_connector(&mut self, cx: &mut Context<Self>) -> bool {
        let State::Ready(doc) = &mut self.state else {
            return false;
        };
        if doc.pending_connector.take().is_some() {
            doc.notice = Some("Connector cancelled".into());
            cx.notify();
            true
        } else {
            false
        }
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
        let Some(pref) = doc.page_ref(index) else {
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
        doc.pending_scroll = Some(crate::docview::ScrollTo::Reveal {
            page: index,
            fx: (r.x + r.w / 2.0) / ps.width.max(1.0),
            fy: (r.y + r.h / 2.0) / ps.height.max(1.0),
        });
        doc.selected_annot = Some((page_id, id));
        doc.notice = Some(format!("Page {}", index + 1));
        cx.notify();
    }

    /// Window position → (page index, point in source-page space).
    fn page_point(&self, pos: Point<Pixels>) -> Option<(usize, Pt)> {
        let State::Ready(doc) = &self.state else {
            return None;
        };
        let layout = doc.layout.as_ref()?;
        let b = doc.scroll.bounds();
        let x = f32::from(pos.x - b.origin.x) - f32::from(doc.scroll.offset().x);
        let y = f32::from(pos.y - b.origin.y) - f32::from(doc.scroll.offset().y);
        if layout.len() == 0 {
            return None;
        }
        let i = layout.page_at(y).min(layout.len() - 1);
        let (left, top, w, h) = (
            layout.lefts[i],
            layout.tops[i],
            layout.widths[i],
            layout.heights[i],
        );
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
        Some((i, unrotate_pt(disp, src, pref.rotation)))
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
        let Some((page, p)) = self.page_point(ev.position) else {
            return;
        };
        window.focus(&self.focus_handle, cx);
        // Shift+drag selects a region, whatever the tool; a plain click
        // drops the region and goes on as usual.
        if ev.modifiers.shift {
            self.start_region(page, p, cx);
            return;
        }
        self.clear_region(cx);
        // A calculator tape waiting to be placed takes the click, whatever
        // the tool.
        let under = match &self.state {
            State::Ready(doc) => doc.page_ref(page).map(|r| r.id),
            _ => None,
        };
        if let Some(pid) = under
            && self.place_tape_at(pid, p, cx)
        {
            return;
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
            Tool::Select => {
                // Topmost annotation under the cursor wins.
                let hit = doc
                    .model
                    .state()
                    .annots_on(page_id)
                    .and_then(|list| list.iter().rev().find(|a| hit_test(a, p)).cloned());
                doc.selected_annot = hit.as_ref().map(|a| (page_id, a.id));
                if let Some(a) = hit {
                    doc.drag = Some(Drag {
                        page,
                        page_id,
                        start: p,
                        current: p,
                        path: Vec::new(),
                        moving: Some(a),
                    });
                }
                cx.notify();
            }
            Tool::Note => {
                let pos = Pt {
                    x: p.x - 10.0,
                    y: p.y - 10.0,
                };
                let a = self.new_annot(AnnotKind::Note { pos }, color);
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
                );
                self.add_annot(page_id, a, "Tickmark", cx);
            }
            Tool::Connector => {
                let rect = connector_rect(p);
                match doc.pending_connector.take() {
                    None => {
                        let label = doc.model.state().next_connector_label();
                        doc.notice = Some(format!(
                            "Connector {label}: click where its other end goes (Esc cancels)"
                        ));
                        doc.pending_connector = Some(PendingConnector {
                            page_id,
                            rect,
                            label,
                        });
                        cx.notify();
                    }
                    Some(first) => {
                        let mk = |rect: Box2, label: String| {
                            self.new_annot(
                                AnnotKind::Connector {
                                    rect,
                                    label,
                                    partner: None,
                                },
                                color,
                            )
                        };
                        let a = mk(first.rect, first.label.clone());
                        let b = mk(rect, first.label.clone());
                        self.apply(
                            Edit::ConnectorAdd {
                                a: (first.page_id, a),
                                b: (page_id, b),
                            },
                            &format!("Connector {}", first.label),
                            cx,
                        );
                    }
                }
            }
            _ => {
                doc.drag = Some(Drag {
                    page,
                    page_id,
                    start: p,
                    current: p,
                    path: vec![p],
                    moving: None,
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
        if ev.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let Some((page, p)) = self.page_point(ev.position) else {
            return;
        };
        if self.drag_region(page, p, cx) {
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
        if self.end_region(cx) {
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
        let rect = Box2::from_corners(a, b);
        let too_small = rect.w < 3.0 && rect.h < 3.0;
        match tool {
            Tool::Select => {
                if let Some(orig) = drag.moving {
                    let (dx, dy) = (b.x - a.x, b.y - a.y);
                    if dx.abs() < 0.5 && dy.abs() < 0.5 {
                        if let Some(partner) = orig.partner() {
                            self.jump_to_annot(partner, cx);
                        }
                    } else {
                        let mut moved = orig.clone();
                        moved.kind = translate(&orig.kind, dx, dy);
                        moved.modified = sheaf_doc::pdf_date_now();
                        self.apply(
                            Edit::AnnotUpdate {
                                page: page_id,
                                annot: moved,
                            },
                            "Move annotation",
                            cx,
                        );
                    }
                }
            }
            Tool::Highlight => {
                if too_small {
                    return;
                }
                self.highlight_from_drag(page_id, drag.page, rect, cx);
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
                let a = self.new_annot(kind, color);
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
                    },
                    color,
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
                );
                an.stroke = 2.0;
                self.add_annot(page_id, an, "Pencil", cx);
            }
            Tool::Note | Tool::Tick | Tool::Connector => {}
        }
    }

    /// Ask the engine which glyphs the drag covered; fall back to an area
    /// highlight when there's no text there.
    fn highlight_from_drag(
        &mut self,
        page_id: PageId,
        page: usize,
        rect: Box2,
        cx: &mut Context<Self>,
    ) {
        let color = self.tool_color;
        let State::Ready(doc) = &self.state else {
            return;
        };
        let Some(pref) = doc.page_ref(page) else {
            return;
        };
        let Some(handle) = doc.handles.get(&pref.source).cloned() else {
            return;
        };
        let rx = handle.text_boxes_in_rect(
            pref.index,
            Rect {
                x: rect.x,
                y: rect.y,
                w: rect.w,
                h: rect.h,
            },
        );
        cx.spawn(async move |this, cx| {
            let boxes = rx.await.ok().and_then(|r| r.ok()).unwrap_or_default();
            this.update(cx, |view, cx| {
                let rects: Vec<Box2> = if boxes.is_empty() {
                    vec![rect]
                } else {
                    boxes
                        .into_iter()
                        .map(|r| Box2 {
                            x: r.x,
                            y: r.y,
                            w: r.w,
                            h: r.h,
                        })
                        .collect()
                };
                let mut a = view.new_annot(AnnotKind::Highlight { rects }, color);
                a.opacity = 0.6;
                view.add_annot(page_id, a, "Highlight", cx);
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn delete_selected_annot(&mut self, cx: &mut Context<Self>) -> bool {
        let State::Ready(doc) = &mut self.state else {
            return false;
        };
        let Some((page, id)) = doc.selected_annot.take() else {
            return false;
        };
        let label = match doc.model.state().find_annot(id) {
            Some((_, a)) if a.partner().is_some() => "Delete connector pair",
            _ => "Delete annotation",
        };
        self.apply(Edit::AnnotDelete { page, id }, label, cx);
        true
    }

    /// Toolbar: tools, colors, and the tickmark palette.
    pub(crate) fn render_toolbar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let tool = self.tool;
        let color = self.tool_color;
        let tools: Vec<_> = Tool::ALL
            .iter()
            .map(|&t| {
                let active = t == tool;
                div()
                    .id(("tool", t as usize))
                    .size(px(26.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .cursor_pointer()
                    .bg(if active { rgb(0x4a9eff) } else { rgb(0x3a3a3a) })
                    .text_color(if active { rgb(0xffffff) } else { rgb(0xc0c0c0) })
                    .hover(|s| s.bg(if active { rgb(0x5aaaff) } else { rgb(0x4a4a4a) }))
                    .on_click(cx.listener(move |v, _, _, cx| v.set_tool(t, cx)))
                    .tooltip(Tooltip::for_action(t.title(), t.description(), t.action()))
                    .child(svg().path(t.icon()).size(px(18.0)).text_color(if active {
                        rgb(0xffffff)
                    } else {
                        rgb(0xc0c0c0)
                    }))
            })
            .collect();
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
                    .border_color(if active { rgb(0xffffff) } else { rgb(0x1a1a1a) })
                    .cursor_pointer()
                    .tooltip(Tooltip::text(name))
                    .on_click(cx.listener(move |v, _, _, cx| {
                        v.tool_color = c;
                        cx.notify();
                    }))
            })
            .collect();
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
                    .bg(if active { rgb(0x4a9eff) } else { rgb(0x3a3a3a) })
                    .hover(|s| s.bg(rgb(0x4a4a4a)))
                    .tooltip(Tooltip::for_action(
                        format!("{} {g}", if wide { "Stamp" } else { "Tickmark" }),
                        "Click on the page to stamp this mark",
                        Box::new(crate::ToolTick),
                    ))
                    .on_click(cx.listener(move |v, _, _, cx| {
                        v.tick_glyph = pick.clone();
                        v.set_tool(Tool::Tick, cx);
                    }))
                    .child(g)
            })
            .collect();
        div()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .bg(rgb(0x252525))
            .border_b_1()
            .border_color(rgb(0x111111))
            .children(tools)
            .child(div().w(px(12.0)))
            .children(colors)
            .child(div().w(px(12.0)))
            .children(ticks)
            .into_any_element()
    }

    pub(crate) fn page_cursor(&self) -> CursorStyle {
        if self.tool == Tool::Select {
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
        match tool {
            Tool::Select => {
                let orig = drag.moving.as_ref()?;
                let mut moved = orig.clone();
                moved.kind = translate(&orig.kind, b.x - a.x, b.y - a.y);
                Some(
                    div()
                        .absolute()
                        .inset_0()
                        .children(crate::docview::annot_elements(&moved, src, rot, k))
                        .into_any_element(),
                )
            }
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
            Tool::Highlight | Tool::FreeText | Tool::Square | Tool::Ellipse => {
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
                    Tool::Highlight => el.bg(rgba(0xffe60060)).into_any_element(),
                    Tool::Ellipse => el
                        .border_1()
                        .border_color(c)
                        .rounded_full()
                        .into_any_element(),
                    _ => el.border_1().border_color(c).into_any_element(),
                })
            }
            Tool::Note | Tool::Tick | Tool::Connector => None,
        }
    }

    /// The half-placed connector, drawn where it will land.
    pub(crate) fn pending_connector_preview(
        doc: &crate::docview::Doc,
        color: [u8; 3],
        page_id: PageId,
        src: PageSize,
        rot: Rotation,
        k: f32,
    ) -> Option<AnyElement> {
        let pending = doc.pending_connector.as_ref()?;
        if pending.page_id != page_id {
            return None;
        }
        let mut ghost = Annot {
            id: AnnotId(0),
            kind: AnnotKind::Connector {
                rect: pending.rect,
                label: pending.label.clone(),
                partner: None,
            },
            color,
            opacity: 0.6,
            stroke: 1.0,
            author: String::new(),
            contents: String::new(),
            modified: String::new(),
        };
        ghost.opacity = 0.6;
        Some(
            div()
                .absolute()
                .inset_0()
                .children(crate::docview::annot_elements(&ghost, src, rot, k))
                .into_any_element(),
        )
    }
}

fn hit_test(a: &Annot, p: Pt) -> bool {
    let b = a.bounds();
    let pad = 3.0;
    Box2 {
        x: b.x - pad,
        y: b.y - pad,
        w: b.w + 2.0 * pad,
        h: b.h + 2.0 * pad,
    }
    .contains(p)
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
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            for pts in &paths {
                if pts.len() < 2 {
                    continue;
                }
                let mut pb = PathBuilder::stroke(px(width.max(1.0)));
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
