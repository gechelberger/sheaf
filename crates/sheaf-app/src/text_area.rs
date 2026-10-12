//! A small multi-line text editor for annotation text: wrapping, cursor
//! movement by line, selection, clipboard. Enter inserts a newline;
//! Ctrl+Enter confirms, Escape cancels. Grows with its content between a
//! minimum and maximum number of rows. Built on the same input-handler
//! model as `text_input.rs`, with GPUI's wrapped text shaping.

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, GlobalElementId, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, SharedString, Style,
    TextRun, UTF16Selection, UnderlineStyle, Window, WrappedLine, actions, div, fill, point,
    prelude::*, px, relative, rgb, rgba, size,
};
use unicode_segmentation::UnicodeSegmentation;

actions!(
    text_area,
    [
        Backspace,
        Delete,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        Paste,
        Cut,
        Copy,
        InsertSpace,
        Newline,
        Confirm,
        Cancel
    ]
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAreaEvent {
    Confirm,
    Cancel,
}

impl gpui::EventEmitter<TextAreaEvent> for TextArea {}

pub const KEY_CONTEXT: &str = "TextArea";

/// One shaped paragraph and where it sits.
struct Para {
    /// Byte offset of the paragraph's first char in `content`.
    start: usize,
    /// Top of the paragraph, relative to the element.
    top: Pixels,
    line: WrappedLine,
}

pub struct TextArea {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Vec<Para>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
    /// Rows to show at least / at most.
    min_rows: usize,
    max_rows: usize,
    /// The x the cursor wants when moving vertically.
    goal_x: Option<Pixels>,
}

impl TextArea {
    pub fn new(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: "".into(),
            placeholder: placeholder.into(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: Vec::new(),
            last_bounds: None,
            is_selecting: false,
            min_rows: 3,
            max_rows: 10,
            goal_x: None,
        }
    }

    pub fn rows(mut self, min: usize, max: usize) -> Self {
        self.min_rows = min.max(1);
        self.max_rows = max.max(self.min_rows);
        self
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    /// Replace the content and put the cursor at the end.
    pub fn set_text(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.content = text.into();
        self.marked_range = None;
        let end = self.content.len();
        self.selected_range = end..end;
        self.selection_reversed = false;
        cx.notify();
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(TextAreaEvent::Confirm);
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(TextAreaEvent::Cancel);
    }

    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text_in_range(None, "\n", window, cx);
    }

    fn insert_space(&mut self, _: &InsertSpace, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text_in_range(None, " ", window, cx);
    }

    fn select_all(&mut self, _: &SelectAll, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        self.goal_x = None;
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        self.goal_x = None;
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn up(&mut self, _: &Up, window: &mut Window, cx: &mut Context<Self>) {
        self.vertical(-1, window, cx);
    }

    fn down(&mut self, _: &Down, window: &mut Window, cx: &mut Context<Self>) {
        self.vertical(1, window, cx);
    }

    /// Move the cursor one visual row up or down, keeping its x.
    fn vertical(&mut self, dir: i32, window: &mut Window, cx: &mut Context<Self>) {
        let line_height = window.line_height();
        let Some(pos) = self.position_for_offset(self.cursor_offset(), line_height) else {
            return;
        };
        let x = *self.goal_x.get_or_insert(pos.x);
        let y = pos.y + line_height * (dir as f32) + line_height / 2.0;
        let target = if y < px(0.0) {
            0
        } else {
            self.offset_for_position(point(x, y), line_height)
                .unwrap_or(self.content.len())
        };
        self.move_to(target, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        let c = self.cursor_offset();
        let start = self.content[..c].rfind('\n').map(|i| i + 1).unwrap_or(0);
        self.move_to(start, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        let c = self.cursor_offset();
        let end = self.content[c..]
            .find('\n')
            .map(|i| c + i)
            .unwrap_or(self.content.len());
        self.move_to(end, cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let prev = self.previous_boundary(self.cursor_offset());
            if self.cursor_offset() == prev {
                return;
            }
            self.select_to(prev, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let next = self.next_boundary(self.cursor_offset());
            if self.cursor_offset() == next {
                return;
            }
            self.select_to(next, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.is_selecting = true;
        self.goal_x = None;
        let ix = self.index_for_mouse_position(ev.position, window);
        if ev.modifiers.shift {
            self.select_to(ix, cx);
        } else {
            self.move_to(ix, cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            let ix = self.index_for_mouse_position(ev.position, window);
            self.select_to(ix, cx);
        }
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text.replace("\r\n", "\n"), window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify()
    }

    /// Element-relative position of a byte offset, from the last layout.
    fn position_for_offset(&self, offset: usize, line_height: Pixels) -> Option<Point<Pixels>> {
        let para = self.last_layout.iter().rev().find(|p| p.start <= offset)?;
        let local = offset - para.start;
        let p = para.line.position_for_index(local, line_height)?;
        Some(point(p.x, p.y + para.top))
    }

    /// Byte offset nearest an element-relative position.
    fn offset_for_position(&self, pos: Point<Pixels>, line_height: Pixels) -> Option<usize> {
        let last = self.last_layout.last()?;
        if pos.y >= last.top + last.line.size(line_height).height {
            return Some(self.content.len());
        }
        let para = self
            .last_layout
            .iter()
            .rev()
            .find(|p| pos.y >= p.top)
            .or(self.last_layout.first())?;
        let local = para
            .line
            .closest_index_for_position(point(pos.x, pos.y - para.top), line_height)
            .unwrap_or_else(|ix| ix);
        Some(para.start + local.min(para.line.len()))
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>, window: &Window) -> usize {
        let Some(bounds) = self.last_bounds else {
            return 0;
        };
        let local = point(position.x - bounds.left(), position.y - bounds.top());
        if local.y < px(0.0) {
            return 0;
        }
        self.offset_for_position(local, window.line_height())
            .unwrap_or(self.content.len())
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }
}

impl EntityInputHandler for TextArea {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
        self.goal_x = None;
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .map(|new_range| new_range.start + range.start..new_range.end + range.end)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        let lh = window.line_height();
        let a = self.position_for_offset(range.start, lh)?;
        let b = self.position_for_offset(range.end, lh)?;
        Some(Bounds::from_corners(
            point(bounds.left() + a.x, bounds.top() + a.y),
            point(bounds.left() + b.x, bounds.top() + b.y + lh),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let utf8 = self.index_for_mouse_position(point, window);
        Some(self.offset_to_utf16(utf8))
    }
}

struct AreaElement {
    input: Entity<TextArea>,
}

struct PrepaintState {
    paras: Vec<Para>,
    cursor: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
}

impl IntoElement for AreaElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for AreaElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        // Height from the last frame's wrapping, clamped to the row limits.
        let input = self.input.read(cx);
        let lh = window.line_height();
        let used: f32 = input
            .last_layout
            .iter()
            .map(|p| f32::from(p.line.size(lh).height))
            .sum();
        let rows = (used / f32::from(lh)).ceil().max(1.0) as usize;
        let rows = rows.clamp(input.min_rows, input.max_rows);
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = (lh * rows as f32).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let marked = input.marked_range.clone();
        let style = window.text_style();
        let lh = window.line_height();
        let font_size = style.font_size.to_pixels(window.rem_size());

        let empty = content.is_empty();
        let (text, color) = if empty {
            (input.placeholder.clone(), rgba(0xffffff50).into())
        } else {
            (content, style.color)
        };
        let base = TextRun {
            len: text.len(),
            font: style.font(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs: Vec<TextRun> = match &marked {
            Some(m) if !empty => vec![
                TextRun {
                    len: m.start,
                    ..base.clone()
                },
                TextRun {
                    len: m.end - m.start,
                    underline: Some(UnderlineStyle {
                        color: Some(base.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..base.clone()
                },
                TextRun {
                    len: text.len() - m.end,
                    ..base
                },
            ]
            .into_iter()
            .filter(|r| r.len > 0)
            .collect(),
            _ => vec![base],
        };
        let lines = window
            .text_system()
            .shape_text(
                text.clone(),
                font_size,
                &runs,
                Some(bounds.size.width),
                None,
            )
            .unwrap_or_default();

        // shape_text yields one WrappedLine per '\n'-separated paragraph.
        let mut paras = Vec::with_capacity(lines.len());
        let mut start = 0usize;
        let mut top = px(0.0);
        for line in lines {
            let len = line.len();
            let h = line.size(lh).height;
            paras.push(Para { start, top, line });
            start += len + 1;
            top += h;
        }

        let pos_of = |offset: usize| -> Option<Point<Pixels>> {
            let p = paras.iter().rev().find(|p| p.start <= offset)?;
            let q = p.line.position_for_index(offset - p.start, lh)?;
            Some(point(bounds.left() + q.x, bounds.top() + q.y + p.top))
        };

        let mut selection = Vec::new();
        let cursor_quad = if empty || selected_range.is_empty() {
            let p = if empty {
                Some(bounds.origin)
            } else {
                pos_of(cursor)
            };
            p.map(|p| fill(Bounds::new(p, size(px(2.), lh)), rgb(0x4a9eff)))
        } else {
            let color = rgba(0x4a9eff60);
            if let (Some(a), Some(b)) = (pos_of(selected_range.start), pos_of(selected_range.end)) {
                if a.y == b.y {
                    selection.push(fill(Bounds::from_corners(a, point(b.x, b.y + lh)), color));
                } else {
                    selection.push(fill(
                        Bounds::from_corners(a, point(bounds.right(), a.y + lh)),
                        color,
                    ));
                    if b.y > a.y + lh {
                        selection.push(fill(
                            Bounds::from_corners(
                                point(bounds.left(), a.y + lh),
                                point(bounds.right(), b.y),
                            ),
                            color,
                        ));
                    }
                    selection.push(fill(
                        Bounds::from_corners(point(bounds.left(), b.y), point(b.x, b.y + lh)),
                        color,
                    ));
                }
            }
            None
        };
        PrepaintState {
            paras,
            cursor: cursor_quad,
            selection,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        for q in prepaint.selection.drain(..) {
            window.paint_quad(q);
        }
        let lh = window.line_height();
        let paras = std::mem::take(&mut prepaint.paras);
        for p in &paras {
            p.line
                .paint(
                    point(bounds.left(), bounds.top() + p.top),
                    lh,
                    gpui::TextAlign::Left,
                    None,
                    window,
                    cx,
                )
                .ok();
        }
        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }
        self.input.update(cx, |input, _| {
            input.last_layout = paras;
            input.last_bounds = Some(bounds);
        });
    }
}

impl Render for TextArea {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.is_focused(window);
        div()
            .id("text-area")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(|this, a: &SelectAll, _, cx| this.select_all(a, cx)))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::insert_space))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .w_full()
            .px_2()
            .py_1()
            .rounded_sm()
            .bg(rgb(0x1a1a1a))
            .border_1()
            .border_color(if focused {
                rgb(0x4a9eff)
            } else {
                rgb(0x3a3a3a)
            })
            .text_sm()
            .text_color(rgb(0xe0e0e0))
            .child(AreaElement { input: cx.entity() })
    }
}

impl Focusable for TextArea {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
