//! The Comments panel: every annotation in the document listed by page, as
//! in Acrobat's comments pane, in threads. Our own marks are editable
//! (double-click or F2 edits the text, Delete removes); comments other
//! tools left in the file can be replied to and given a status, and
//! jumped to.
//!
//! A thread is a comment and what's in reply to it, by `/NM`: replies, and
//! statuses (review states and checkmarks), ours (`AnnotKind::Reply`,
//! `AnnotKind::Status`) and other tools' (`ForeignAnnot::parent`) alike.

use std::collections::HashMap;

use gpui::{
    AnchoredPositionMode, AnyElement, Context, Entity, FocusHandle, Focusable, Pixels, Point,
    Subscription, Window, anchored, deferred, div, prelude::*, px,
};
use sheaf_core::PageSize;
use sheaf_doc::annot::{MARKED, REVIEW, REVIEW_STATES};
use sheaf_doc::{Annot, AnnotId, AnnotKind, PageId, file::ForeignAnnot};
use sheaf_render::Rect;
use sheaf_ui::list::ListLayout;
use sheaf_ui::text_area::{TextArea, TextAreaEvent};
use sheaf_ui::text_input::{TextInput, TextInputEvent};
use sheaf_ui::theme;

use crate::docview::{DocView, ScrollTo, State, rotate_rect};
use crate::{DeleteComment, EditComment, Redo, ReplyToComment, Save, ToggleCheckmark, Undo};

pub const KEY_CONTEXT: &str = "Comments";

const HEADER_H: f32 = 22.0;
/// A comment row: padding, the kind/author line, then the text, wrapped
/// to at most `MAX_TEXT_LINES` lines.
const ROW_PAD: f32 = 4.0;
const TITLE_LH: f32 = 16.0;
const TEXT_LH: f32 = 20.0;
const MAX_TEXT_LINES: usize = 4;
/// A status line in a thread.
const STATUS_H: f32 = 22.0;
/// The reply box: three lines of text and its buttons.
const REPLY_BOX_H: f32 = 116.0;
/// Indent per level of a thread.
const INDENT: f32 = 14.0;

fn row_h(lines: usize) -> f32 {
    ROW_PAD * 2.0 + TITLE_LH + TEXT_LH * lines as f32
}

pub struct Comments {
    pub filter: Entity<TextInput>,
    pub _sub: Subscription,
    pub scroll: gpui::ScrollHandle,
    pub focus: FocusHandle,
    /// A reply being written or edited.
    pub reply: Option<ReplyBox>,
    /// The right-click menu: on which comment, and where.
    pub menu: Option<(Item, Point<Pixels>)>,
}

/// The box a reply is written in, under what it replies to.
pub struct ReplyBox {
    /// The comment replied to, or the reply being edited.
    pub to: Item,
    /// The page the reply goes on: its comment's.
    pub page: PageId,
    /// The name of the comment replied to.
    pub parent: String,
    /// The reply being edited; None for a new one.
    pub editing: Option<AnnotId>,
    pub input: Entity<TextArea>,
    pub _sub: Subscription,
}

/// A comment in the list: one of our marks, or another tool's (an index
/// into `Doc::foreign_annots`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Item {
    Ours(PageId, AnnotId),
    Foreign(usize),
}

#[derive(Clone, Copy)]
enum Row {
    Header {
        index: usize,
        count: usize,
    },
    /// A comment, or (`depth` > 0) a reply in a thread. `root` is the
    /// thread's comment, which a click on any of it goes to.
    Comment {
        item: Item,
        root: Item,
        depth: usize,
    },
    /// A status set in a thread.
    Status {
        item: Item,
        root: Item,
        depth: usize,
    },
    ReplyBox {
        depth: usize,
    },
}

/// What threading needs to know of a comment.
#[derive(Clone, Debug)]
struct Node {
    item: Item,
    name: String,
    /// What it's in the thread of, by name.
    parent: Option<String>,
    /// Part of its parent (`/RT /Group`), not listed.
    grouped: bool,
    /// When it was made: its creation date, else its last change.
    date: String,
    /// A status: (model, state).
    state: Option<(String, String)>,
    author: String,
    /// What the filter looks in.
    text: String,
}

/// A page's comments as threads: the top-level ones in order, and the
/// replies and statuses in each one's thread (by its name) in date order.
/// One whose parent isn't on the page is top-level; grouped parts are
/// left out.
fn threads(nodes: &[Node]) -> (Vec<usize>, HashMap<String, Vec<usize>>) {
    let names: std::collections::HashSet<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
    let mut roots = Vec::new();
    let mut children: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, n) in nodes.iter().enumerate() {
        match &n.parent {
            Some(p) if names.contains(p.as_str()) && n.grouped => {}
            Some(p) if names.contains(p.as_str()) => children.entry(p.clone()).or_default().push(i),
            _ => roots.push(i),
        }
    }
    for list in children.values_mut() {
        list.sort_by(|&a, &b| nodes[a].date.cmp(&nodes[b].date));
    }
    (roots, children)
}

/// The status a thread's statuses come to: the last review state, and
/// whether the last checkmark change checked it.
fn thread_status(nodes: &[Node], thread: &[usize]) -> (Option<String>, bool) {
    let mut review = None;
    let mut marked = false;
    for &i in thread {
        match &nodes[i].state {
            Some((m, s)) if m == MARKED => marked = s == "Marked",
            Some((_, s)) => review = (s != "None").then(|| s.clone()),
            None => {}
        }
    }
    (review, marked)
}

/// Short label for one of our marks.
pub(crate) fn kind_label(kind: &AnnotKind) -> String {
    match kind {
        AnnotKind::Highlight { .. } => "Highlight".into(),
        AnnotKind::Note { .. } => "Note".into(),
        AnnotKind::FreeText { .. } => "Text".into(),
        AnnotKind::Square { .. } => "Rectangle".into(),
        AnnotKind::Ellipse { .. } => "Ellipse".into(),
        AnnotKind::Line { arrow: true, .. } => "Arrow".into(),
        AnnotKind::Line { .. } => "Line".into(),
        AnnotKind::Ink { .. } => "Pencil".into(),
        AnnotKind::Stamp { glyph, .. } => format!("Tickmark {glyph}"),
        AnnotKind::Connector { label, .. } => format!("Connector {label}"),
        AnnotKind::Tape { .. } => "Calculator tape".into(),
        AnnotKind::Underline { .. } => "Underline".into(),
        AnnotKind::StrikeOut { .. } => "Strikethrough".into(),
        AnnotKind::Squiggly { .. } => "Squiggly".into(),
        AnnotKind::Replace { .. } => "Replace text".into(),
        AnnotKind::Insert { .. } => "Insert text".into(),
        AnnotKind::Reply { .. } => "Reply".into(),
        AnnotKind::Status { state, .. } => state.clone(),
    }
}

/// Acrobat's names for the common subtypes.
fn subtype_label(subtype: &str) -> &str {
    match subtype {
        "Text" => "Note",
        "FreeText" => "Text",
        "Square" => "Rectangle",
        "Circle" => "Ellipse",
        "Ink" => "Pencil",
        "StrikeOut" => "Strikethrough",
        other => other,
    }
}

/// `D:20261011153000Z` → `2026-10-11 15:30`.
pub fn fmt_pdf_date(d: &str) -> String {
    let s = d.strip_prefix("D:").unwrap_or(d);
    let digit = |i: usize| s.as_bytes().get(i).filter(|b| b.is_ascii_digit()).is_some();
    if !(0..8).all(digit) {
        return String::new();
    }
    let mut out = format!("{}-{}-{}", &s[0..4], &s[4..6], &s[6..8]);
    if (8..12).all(digit) {
        out.push_str(&format!(" {}:{}", &s[8..10], &s[10..12]));
    }
    out
}

impl DocView {
    fn ensure_comments(&mut self, cx: &mut Context<Self>) {
        if self.comments.is_some() {
            return;
        }
        let filter = cx.new(|cx| TextInput::new("Filter comments", cx));
        let sub = cx.subscribe(&filter, |this, _, ev: &TextInputEvent, cx| {
            if *ev == TextInputEvent::Cancel {
                this.pending_comments_focus = true;
            }
            cx.notify();
        });
        self.comments = Some(Comments {
            filter,
            _sub: sub,
            scroll: gpui::ScrollHandle::new(),
            focus: cx.focus_handle(),
            reply: None,
            menu: None,
        });
    }

    /// Make sure the panel exists and give it keyboard focus.
    pub fn focus_comments(&mut self, cx: &mut Context<Self>) {
        self.ensure_comments(cx);
        self.pending_comments_focus = true;
        cx.notify();
    }

    /// Scroll to a third-party comment.
    fn reveal_foreign(&mut self, ix: usize, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let Some((source, f)) = doc.foreign_annots.get(ix).cloned() else {
            return;
        };
        if let Some(index) = doc.index_of_page_in(source, f.page)
            && let Some(pref) = doc.shown_ref(index)
            && let Some(src) = doc
                .model
                .source(pref.source)
                .map(|s| s.page_size(pref.index))
        {
            let r = rotate_rect(
                Rect {
                    x: f.rect.x,
                    y: f.rect.y,
                    w: f.rect.w,
                    h: f.rect.h,
                },
                src,
                pref.rotation,
            );
            let ps = src.rotated(pref.rotation);
            doc.scroll_to(ScrollTo::Reveal {
                page: index,
                fx: (r.x + r.w / 2.0) / ps.width.max(1.0),
                fy: (r.y + r.h / 2.0) / ps.height.max(1.0),
            });
        }
        cx.notify();
    }

    /// Click on a row: select it, and go to its thread's comment.
    fn pick_item(&mut self, item: Item, root: Item, cx: &mut Context<Self>) {
        match root {
            Item::Ours(_, id) => self.jump_to_annot(id, cx),
            Item::Foreign(ix) => self.reveal_foreign(ix, cx),
        }
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        match item {
            Item::Ours(page, id) => {
                doc.selected_foreign = None;
                doc.select_mark(Some((page, id)));
            }
            Item::Foreign(ix) => {
                doc.selected_marks.clear();
                doc.selected_foreign = Some(ix);
            }
        }
        cx.notify();
    }

    /// The comment selected here or on the page.
    pub(crate) fn selected_item(&self) -> Option<Item> {
        let State::Ready(doc) = &self.state else {
            return None;
        };
        match (doc.selected_marks.last(), doc.selected_foreign) {
            (Some(&(page, id)), _) => Some(Item::Ours(page, id)),
            (None, Some(ix)) => Some(Item::Foreign(ix)),
            _ => None,
        }
    }

    /// The page a comment is on, and its name in threads.
    fn item_page_and_name(&self, item: Item) -> Option<(PageId, String)> {
        let State::Ready(doc) = &self.state else {
            return None;
        };
        match item {
            Item::Ours(page, id) => {
                let a = doc
                    .model
                    .state()
                    .annots_on(page)?
                    .iter()
                    .find(|a| a.id == id)?
                    .clone();
                Some((page, a.name()))
            }
            Item::Foreign(ix) => {
                let (source, f) = doc.foreign_annots.get(ix)?;
                let index = doc.index_of_page_in(*source, f.page)?;
                Some((doc.page_ref(index)?.id, f.name.clone()))
            }
        }
    }

    /// One of our marks by page and id.
    fn ours(&self, page: PageId, id: AnnotId) -> Option<Annot> {
        let State::Ready(doc) = &self.state else {
            return None;
        };
        doc.model
            .state()
            .annots_on(page)?
            .iter()
            .find(|a| a.id == id)
            .cloned()
    }

    /// R: open the reply box under `item`'s thread (or, for one of our
    /// replies, to edit it when it's the user's own).
    pub(crate) fn start_reply(&mut self, item: Item, window: &mut Window, cx: &mut Context<Self>) {
        let me = crate::prefs::get(cx).identity.name();
        let (to, page, parent, editing, text) = match item {
            Item::Ours(page, id) => match self.ours(page, id) {
                Some(a) if matches!(a.kind, AnnotKind::Reply { .. }) && a.author == me => {
                    let parent = a.thread_parent().unwrap_or_default().to_string();
                    (item, page, parent, Some(id), a.contents.clone())
                }
                Some(a) => (item, page, a.name(), None, String::new()),
                None => return,
            },
            Item::Foreign(_) => match self.item_page_and_name(item) {
                Some((page, name)) => (item, page, name, None, String::new()),
                None => return,
            },
        };
        self.ensure_comments(cx);
        let input = cx.new(|cx| {
            let mut t = TextArea::new("Reply; Ctrl+Enter sends, Esc cancels", cx).rows(3, 3);
            t.set_text(text, cx);
            t
        });
        let sub = cx.subscribe(&input, |v, _, ev: &TextAreaEvent, cx| {
            v.finish_reply(*ev == TextAreaEvent::Confirm, cx);
        });
        window.focus(&input.read(cx).focus_handle(cx), cx);
        if let Some(cm) = &mut self.comments {
            cm.menu = None;
            cm.reply = Some(ReplyBox {
                to,
                page,
                parent,
                editing,
                input,
                _sub: sub,
            });
        }
        cx.notify();
    }

    /// Send (or drop) what's in the reply box.
    fn finish_reply(&mut self, send: bool, cx: &mut Context<Self>) {
        let Some(rb) = self.comments.as_mut().and_then(|cm| cm.reply.take()) else {
            return;
        };
        self.pending_comments_focus = true;
        cx.notify();
        let text = rb.input.read(cx).text().trim_end().to_string();
        if !send || text.trim().is_empty() {
            return;
        }
        match rb.editing {
            Some(id) => {
                if let Some(mut a) = self.ours(rb.page, id)
                    && a.contents != text
                {
                    a.contents = text;
                    a.modified = sheaf_doc::pdf_date_now();
                    self.apply(
                        sheaf_doc::Edit::AnnotUpdate {
                            page: rb.page,
                            annot: a,
                        },
                        "Edit reply",
                        cx,
                    );
                }
            }
            None => {
                let mut a =
                    self.new_annot(AnnotKind::Reply { parent: rb.parent }, self.tool_color, cx);
                a.contents = text;
                self.add_annot(rb.page, a, "Reply", cx);
            }
        }
    }

    /// Set a status on `item`: a review state, or the checkmark.
    pub(crate) fn set_comment_status(
        &mut self,
        item: Item,
        model: &str,
        state: &str,
        cx: &mut Context<Self>,
    ) {
        let Some((page, parent)) = self.item_page_and_name(item) else {
            return;
        };
        let mut a = self.new_annot(
            AnnotKind::Status {
                parent,
                model: model.to_string(),
                state: state.to_string(),
            },
            self.tool_color,
            cx,
        );
        // What Acrobat writes, for viewers that list statuses as text.
        a.contents = format!("{state} set by {}", a.author);
        self.add_annot(page, a, "Status", cx);
        if let Some(cm) = &mut self.comments {
            cm.menu = None;
        }
        // Keep the comment selected, not the status.
        if let State::Ready(doc) = &mut self.state
            && let Item::Ours(page, id) = item
        {
            doc.select_mark(Some((page, id)));
        }
        cx.notify();
    }

    /// The status `item`'s thread has now: (review state, checked).
    fn status_of(&self, item: Item) -> (Option<String>, bool) {
        let Some((page, name)) = self.item_page_and_name(item) else {
            return (None, false);
        };
        let nodes = self.page_nodes(page);
        let (_, children) = threads(&nodes);
        thread_status(&nodes, children.get(&name).map_or(&[][..], |v| v))
    }

    /// Shift+K: check or uncheck the selected comment.
    pub(crate) fn toggle_checkmark(&mut self, cx: &mut Context<Self>) {
        let Some(item) = self.selected_item() else {
            return;
        };
        // A reply or status isn't checked itself; its comment is.
        if let Item::Ours(page, id) = item
            && self
                .ours(page, id)
                .is_some_and(|a| a.thread_parent().is_some())
        {
            return;
        }
        let (_, marked) = self.status_of(item);
        let state = if marked { "Unmarked" } else { "Marked" };
        self.set_comment_status(item, MARKED, state, cx);
    }

    /// Every comment on a page, for threading.
    fn page_nodes(&self, page: PageId) -> Vec<Node> {
        let State::Ready(doc) = &self.state else {
            return Vec::new();
        };
        let st = doc.model.state();
        let mut nodes = Vec::new();
        for a in st.annots_on(page).into_iter().flatten() {
            nodes.push(Node {
                item: Item::Ours(page, a.id),
                name: a.name(),
                parent: a.thread_parent().map(str::to_string),
                grouped: false,
                date: if a.created.is_empty() {
                    a.modified.clone()
                } else {
                    a.created.clone()
                },
                state: match &a.kind {
                    AnnotKind::Status { model, state, .. } => Some((model.clone(), state.clone())),
                    _ => None,
                },
                author: a.author.clone(),
                text: format!("{} {}", ours_text(a), kind_label(&a.kind)),
            });
        }
        let Some(pref) = st.pages.iter().find(|p| p.id == page) else {
            return nodes;
        };
        for (i, (source, f)) in doc.foreign_annots.iter().enumerate() {
            if *source == pref.source && f.page == pref.index {
                nodes.push(Node {
                    item: Item::Foreign(i),
                    name: f.name.clone(),
                    parent: f.parent.clone(),
                    grouped: f.grouped,
                    date: if f.created.is_empty() {
                        f.modified.clone()
                    } else {
                        f.created.clone()
                    },
                    state: f.state.clone(),
                    author: f.author.clone(),
                    text: format!("{} {}", foreign_text(f), subtype_label(&f.subtype)),
                });
            }
        }
        nodes
    }

    /// F2 / double-click: edit the selected mark's text in place (a
    /// reply's in the reply box).
    fn edit_selected_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let State::Ready(doc) = &self.state else {
            return;
        };
        let Some((page, id)) = doc.selected_annot() else {
            return;
        };
        match self.ours(page, id).map(|a| a.kind) {
            Some(AnnotKind::Reply { .. }) => {
                self.start_reply(Item::Ours(page, id), window, cx);
            }
            Some(AnnotKind::Status { .. }) | None => {}
            Some(_) => {
                self.jump_to_annot(id, cx);
                self.start_annot_editor_for(page, id, window, cx);
            }
        }
    }

    /// Delete: one of our marks, or a reply or status of the user's own.
    fn delete_selected_comment(&mut self, cx: &mut Context<Self>) {
        let me = crate::prefs::get(cx).identity.name();
        if let Some(Item::Ours(page, id)) = self.selected_item()
            && let Some(a) = self.ours(page, id)
            && a.thread_parent().is_some()
            && a.author != me
        {
            if let State::Ready(doc) = &mut self.state {
                doc.notice = Some(format!(
                    "That's {}'s; only your own can be deleted",
                    a.author
                ));
            }
            cx.notify();
            return;
        }
        self.delete_selected_annot(cx);
    }

    /// Rows in display-page order, filtered, in threads.
    fn comment_rows(&self, needle: &str) -> Vec<Row> {
        let State::Ready(doc) = &self.state else {
            return Vec::new();
        };
        let reply = self
            .comments
            .as_ref()
            .and_then(|cm| cm.reply.as_ref())
            .map(|r| (r.to, r.editing));
        let matches = |n: &Node| {
            needle.is_empty()
                || n.text.to_lowercase().contains(needle)
                || n.author.to_lowercase().contains(needle)
        };
        let mut rows = Vec::new();
        for (index, pref) in doc.model.state().pages.iter().enumerate() {
            let nodes = self.page_nodes(pref.id);
            if nodes.is_empty() {
                continue;
            }
            let (roots, children) = threads(&nodes);
            let mut page_rows = Vec::new();
            let mut count = 0;
            for &r in &roots {
                // A thread shows when anything in it matches.
                let mut thread = vec![r];
                let mut i = 0;
                while i < thread.len() {
                    if let Some(kids) = children.get(&nodes[thread[i]].name) {
                        thread.extend(kids);
                    }
                    i += 1;
                }
                if !thread.iter().any(|&n| matches(&nodes[n])) {
                    continue;
                }
                count += 1;
                let root = nodes[r].item;
                push_thread(&nodes, &children, r, root, 0, reply, &mut page_rows);
            }
            if !page_rows.is_empty() {
                rows.push(Row::Header { index, count });
                rows.extend(page_rows);
            }
        }
        rows
    }

    /// The Comments panel.
    pub fn render_comments(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.ensure_comments(cx);
        if std::mem::take(&mut self.pending_comments_focus) {
            let focus = self.comments.as_ref().unwrap().focus.clone();
            window.focus(&focus, cx);
        }
        let needle = self
            .comments
            .as_ref()
            .unwrap()
            .filter
            .read(cx)
            .text()
            .trim()
            .to_lowercase();
        let rows = self.comment_rows(&needle);
        let me = crate::prefs::get(cx).identity.name();
        // Each comment's thread status, for its title.
        let statuses: HashMap<usize, (Option<String>, bool)> = rows
            .iter()
            .enumerate()
            .filter_map(|(i, r)| match r {
                Row::Comment { item, depth: 0, .. } => Some((i, self.status_of(*item))),
                _ => None,
            })
            .collect();
        let State::Ready(doc) = &self.state else {
            return div().p_2().text_sm().child("…").into_any_element();
        };
        let cm = self.comments.as_ref().unwrap();
        let filter = cm.filter.clone();
        let focus = cm.focus.clone();
        let reply_input = cm.reply.as_ref().map(|r| r.input.clone());
        let menu = cm.menu;
        let st = doc.model.state();

        let bounds = cm.scroll.bounds();
        // Variable-height rows: headers are short, and a comment's text
        // wraps to the panel's width.
        let wrap_w = f32::from(bounds.size.width) - 16.0;
        let font = window.text_style().font();
        let font_size = window.rem_size() * 0.875;
        let text_lines = |text: &str, depth: usize| -> usize {
            let w = wrap_w - depth as f32 * INDENT;
            if w <= 0.0 || text.is_empty() {
                return 1;
            }
            let run = gpui::TextRun {
                len: text.len(),
                font: font.clone(),
                color: gpui::black(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            window
                .text_system()
                .shape_text(
                    text.to_string().into(),
                    font_size,
                    &[run],
                    Some(px(w)),
                    Some(MAX_TEXT_LINES),
                )
                .map(|ls| ls.iter().map(|l| l.wrap_boundaries().len() + 1).sum())
                .unwrap_or(1)
                .clamp(1, MAX_TEXT_LINES)
        };
        let ours_of = |page: PageId, id: AnnotId| {
            st.annots_on(page)
                .and_then(|l| l.iter().find(|a| a.id == id))
                .cloned()
        };
        let layout = ListLayout::variable(rows.iter().map(|r| {
            match *r {
                Row::Header { .. } => HEADER_H,
                Row::Status { .. } => STATUS_H,
                Row::ReplyBox { .. } => REPLY_BOX_H,
                Row::Comment {
                    item: Item::Ours(page, id),
                    depth,
                    ..
                } => row_h(ours_of(page, id).map_or(1, |a| text_lines(&ours_text(&a), depth))),
                Row::Comment {
                    item: Item::Foreign(ix),
                    depth,
                    ..
                } => row_h(
                    doc.foreign_annots
                        .get(ix)
                        .map_or(1, |(_, f)| text_lines(&foreign_text(f), depth)),
                ),
            }
        }))
        .padded(0.0, 4.0);

        let selected = doc.selected_marks.clone();
        let selected_foreign = doc.selected_foreign;
        let is_selected = |item: Item| match item {
            Item::Ours(page, id) => selected.contains(&(page, id)),
            Item::Foreign(ix) => selected_foreign == Some(ix),
        };
        let mut els: Vec<AnyElement> = Vec::new();
        let count: usize = rows
            .iter()
            .map(|r| match r {
                Row::Header { count, .. } => *count,
                _ => 0,
            })
            .sum();
        for i in layout.visible(&cm.scroll) {
            match rows[i] {
                Row::Header { index, count } => els.push(
                    layout
                        .place(i)
                        .flex()
                        .items_center()
                        .px_2()
                        .text_xs()
                        .text_color(theme::TEXT_MUTED)
                        .bg(theme::LIST_HEADER)
                        .child(format!("Page {}  ·  {count}", index + 1))
                        .into_any_element(),
                ),
                Row::ReplyBox { depth } => {
                    let Some(input) = reply_input.clone() else {
                        continue;
                    };
                    els.push(
                        layout
                            .place(i)
                            .pl(px(8.0 + depth as f32 * INDENT))
                            .pr_2()
                            .py_1()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(input)
                            .child(
                                div()
                                    .flex()
                                    .justify_end()
                                    .gap_1()
                                    .text_xs()
                                    .child(small_button("reply-cancel", "Cancel").on_click(
                                        cx.listener(|v, _, _, cx| v.finish_reply(false, cx)),
                                    ))
                                    .child(
                                        small_button("reply-send", "Send  Ctrl+Enter")
                                            .bg(theme::ACCENT)
                                            .on_click(
                                                cx.listener(|v, _, _, cx| v.finish_reply(true, cx)),
                                            ),
                                    ),
                            )
                            .into_any_element(),
                    );
                }
                Row::Status { item, root, depth } => {
                    let (author, state, date) = match item {
                        Item::Ours(page, id) => match ours_of(page, id) {
                            Some(a) => {
                                let state = match &a.kind {
                                    AnnotKind::Status { model, state, .. } => {
                                        status_words(model, state)
                                    }
                                    _ => String::new(),
                                };
                                let date = if a.created.is_empty() {
                                    a.modified
                                } else {
                                    a.created
                                };
                                (a.author, state, date)
                            }
                            None => continue,
                        },
                        Item::Foreign(ix) => match doc.foreign_annots.get(ix) {
                            Some((_, f)) => {
                                let state = f
                                    .state
                                    .as_ref()
                                    .map(|(m, s)| status_words(m, s))
                                    .unwrap_or_default();
                                let date = if f.created.is_empty() {
                                    f.modified.clone()
                                } else {
                                    f.created.clone()
                                };
                                (f.author.clone(), state, date)
                            }
                            None => continue,
                        },
                    };
                    let who = sheaf_ops::prefs::initials_of(&author);
                    els.push(
                        layout
                            .place(i)
                            .id(("status", i))
                            .pl(px(8.0 + depth as f32 * INDENT))
                            .pr_2()
                            .flex()
                            .items_center()
                            .text_xs()
                            .text_color(theme::TEXT_DIM)
                            .cursor_pointer()
                            .when(is_selected(item), |d| d.bg(theme::SELECTED))
                            .hover(|s| s.bg(theme::HOVER))
                            .on_click(cx.listener(move |v, _, _, cx| v.pick_item(item, root, cx)))
                            .child(format!("{state} by {who}, {}", fmt_pdf_date(&date)))
                            .into_any_element(),
                    );
                }
                Row::Comment { item, root, depth } => {
                    let (title, snippet, date, deletable, editable) = match item {
                        Item::Ours(page, id) => {
                            let Some(a) = ours_of(page, id) else {
                                continue;
                            };
                            let reply = matches!(a.kind, AnnotKind::Reply { .. });
                            let mine = a.author == me;
                            (
                                format!("{}  ·  {}", kind_label(&a.kind), a.author),
                                ours_text(&a),
                                a.modified.clone(),
                                !reply || mine,
                                !reply || mine,
                            )
                        }
                        Item::Foreign(ix) => {
                            let Some((_, f)) = doc.foreign_annots.get(ix) else {
                                continue;
                            };
                            let author = if f.author.is_empty() {
                                "(no author)".to_string()
                            } else {
                                f.author.clone()
                            };
                            let label = if depth > 0 {
                                "Reply"
                            } else {
                                subtype_label(&f.subtype)
                            };
                            (
                                format!("{label}  ·  {author}"),
                                foreign_text(f),
                                f.modified.clone(),
                                false,
                                false,
                            )
                        }
                    };
                    // A comment's title carries its thread's status.
                    let title = match statuses.get(&i) {
                        Some((review, marked)) => {
                            let mut t = title;
                            if let Some(r) = review {
                                t.push_str(&format!("  ·  {r}"));
                            }
                            if *marked {
                                t = format!("✓ {t}");
                            }
                            t
                        }
                        None => title,
                    };
                    let row_id = match item {
                        Item::Ours(_, id) => ("ours", id.0 as usize),
                        Item::Foreign(ix) => ("foreign", ix),
                    };
                    let mut row = comment_row(
                        layout.place(i),
                        row_id,
                        is_selected(item),
                        title,
                        snippet,
                        &date,
                    )
                    .pl(px(8.0 + depth as f32 * INDENT))
                    .on_click(cx.listener(move |v, ev: &gpui::ClickEvent, window, cx| {
                        v.pick_item(item, root, cx);
                        if ev.click_count() >= 2 && editable {
                            v.edit_selected_comment(window, cx);
                        }
                    }))
                    .on_mouse_down(
                        gpui::MouseButton::Right,
                        cx.listener(move |v, ev: &gpui::MouseDownEvent, _, cx| {
                            v.pick_item(item, root, cx);
                            if let Some(cm) = &mut v.comments {
                                cm.menu = Some((item, ev.position));
                            }
                            cx.notify();
                        }),
                    );
                    if deletable && let Item::Ours(page, id) = item {
                        row = row.child(
                            div()
                                .id(("del", id.0 as usize))
                                .absolute()
                                .top(px(4.0))
                                .right(px(6.0))
                                .px_1()
                                .rounded_sm()
                                .text_xs()
                                .text_color(theme::TEXT_MUTED)
                                .hover(|s| s.bg(theme::DANGER_HOVER).text_color(theme::TEXT_BRIGHT))
                                .on_click(cx.listener(move |v, _, _, cx| {
                                    if let State::Ready(doc) = &mut v.state {
                                        doc.select_mark(Some((page, id)));
                                    }
                                    v.delete_selected_comment(cx);
                                    cx.stop_propagation();
                                }))
                                .child("×"),
                        );
                    }
                    els.push(row.into_any_element());
                }
            }
        }

        let status = match (count, needle.is_empty()) {
            (0, true) => "No comments".to_string(),
            (n, true) => format!("{n} comment{}", if n == 1 { "" } else { "s" }),
            (n, false) => format!("{n} matching"),
        };
        let menu = menu.map(|(item, pos)| self.comment_menu(item, pos, cx));
        div()
            .id("comments")
            .key_context(KEY_CONTEXT)
            .track_focus(&focus)
            .size_full()
            .flex()
            .flex_col()
            .on_action(
                cx.listener(|v, _: &EditComment, window, cx| v.edit_selected_comment(window, cx)),
            )
            .on_action(cx.listener(|v, _: &DeleteComment, _, cx| {
                v.delete_selected_comment(cx);
            }))
            .on_action(cx.listener(|v, _: &ReplyToComment, window, cx| {
                if let Some(item) = v.selected_item() {
                    v.start_reply(item, window, cx);
                }
            }))
            .on_action(cx.listener(|v, _: &ToggleCheckmark, _, cx| v.toggle_checkmark(cx)))
            .on_action(cx.listener(|v, _: &Undo, _, cx| v.undo(cx)))
            .on_action(cx.listener(|v, _: &Redo, _, cx| v.redo(cx)))
            .on_action(cx.listener(|v, _: &Save, _, cx| v.save(cx).detach()))
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .border_b_1()
                    .border_color(theme::DIVIDER)
                    .child(filter)
                    .child(div().text_xs().text_color(theme::TEXT_MUTED).child(status)),
            )
            .child(
                div()
                    .id("comment-list")
                    .track_scroll(&cm.scroll)
                    .overflow_y_scroll()
                    .flex_1()
                    .min_h(px(0.0))
                    .child(layout.body().children(els)),
            )
            .children(menu)
            .into_any_element()
    }

    /// The right-click menu on a comment: reply, status, checkmark.
    fn comment_menu(&self, item: Item, pos: Point<Pixels>, cx: &mut Context<Self>) -> AnyElement {
        let (review, marked) = self.status_of(item);
        let thread_part = matches!(item, Item::Ours(page, id)
            if self.ours(page, id).is_some_and(|a| a.thread_parent().is_some()));
        let entry = |id: (&'static str, usize), label: String| {
            div()
                .id(id)
                .px_3()
                .py_1()
                .text_sm()
                .cursor_pointer()
                .hover(|s| s.bg(theme::HOVER))
                .child(label)
        };
        let mut items: Vec<AnyElement> = vec![
            entry(("cm-reply", 0), "Reply  R".into())
                .on_click(cx.listener(move |v, _, window, cx| v.start_reply(item, window, cx)))
                .into_any_element(),
        ];
        if !thread_part {
            items.push(
                entry(
                    ("cm-check", 0),
                    format!("{}  Shift+K", if marked { "Uncheck" } else { "Checkmark" }),
                )
                .on_click(cx.listener(move |v, _, _, cx| {
                    let state = if marked { "Unmarked" } else { "Marked" };
                    v.set_comment_status(item, MARKED, state, cx)
                }))
                .into_any_element(),
            );
            items.push(
                div()
                    .px_3()
                    .pt_1()
                    .text_xs()
                    .text_color(theme::TEXT_MUTED)
                    .child("Status")
                    .into_any_element(),
            );
            let current = review.unwrap_or_else(|| "None".into());
            for (k, state) in REVIEW_STATES.iter().enumerate() {
                let on = current == *state;
                items.push(
                    entry(
                        ("cm-state", k),
                        format!("{} {state}", if on { "•" } else { "  " }),
                    )
                    .on_click(
                        cx.listener(move |v, _, _, cx| {
                            v.set_comment_status(item, REVIEW, state, cx)
                        }),
                    )
                    .into_any_element(),
                );
            }
        }
        deferred(
            anchored()
                .position(pos)
                .position_mode(AnchoredPositionMode::Window)
                .child(
                    div()
                        .id("comment-menu")
                        .occlude()
                        .on_mouse_down_out(cx.listener(|v, _, _, cx| {
                            if let Some(cm) = &mut v.comments {
                                cm.menu = None;
                            }
                            cx.notify();
                        }))
                        .min_w(px(170.0))
                        .py_1()
                        .rounded_md()
                        .bg(theme::POPOVER)
                        .border_1()
                        .border_color(theme::DIVIDER)
                        .shadow_lg()
                        .flex()
                        .flex_col()
                        .children(items),
                ),
        )
        .into_any_element()
    }
}

/// Rows for `n`'s thread: it, then what's in reply to it, depth first,
/// and the reply box where one is open.
fn push_thread(
    nodes: &[Node],
    children: &HashMap<String, Vec<usize>>,
    n: usize,
    root: Item,
    depth: usize,
    reply: Option<(Item, Option<AnnotId>)>,
    rows: &mut Vec<Row>,
) {
    let item = nodes[n].item;
    let editing_this = matches!(reply, Some((to, Some(_))) if to == item);
    if editing_this {
        rows.push(Row::ReplyBox { depth });
    } else if nodes[n].state.is_some() {
        rows.push(Row::Status { item, root, depth });
    } else {
        rows.push(Row::Comment { item, root, depth });
    }
    if let Some(kids) = children.get(&nodes[n].name) {
        for &k in kids {
            push_thread(nodes, children, k, root, (depth + 1).min(3), reply, rows);
        }
    }
    if matches!(reply, Some((to, None)) if to == item) {
        rows.push(Row::ReplyBox {
            depth: (depth + 1).min(3),
        });
    }
}

/// "Accepted", "Checked", "Unchecked".
fn status_words(model: &str, state: &str) -> String {
    match (model, state) {
        (MARKED, "Marked") => "Checked".into(),
        (MARKED, _) => "Unchecked".into(),
        (_, s) => s.to_string(),
    }
}

fn small_button(id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px_2()
        .py(px(2.0))
        .rounded_sm()
        .cursor_pointer()
        .bg(theme::CONTROL)
        .hover(|s| s.bg(theme::CONTROL_HOVER))
        .child(label)
}

/// A comment's text as its row shows it. Text edits read as what they
/// suggest.
fn ours_text(a: &Annot) -> String {
    let one_line = |s: &str| s.replace('\n', " ");
    match &a.kind {
        AnnotKind::Replace { struck, .. } if a.contents.is_empty() => {
            return format!("Cross out “{}”", one_line(struck));
        }
        AnnotKind::Replace { struck, .. } => {
            return format!(
                "Replace “{}” with “{}”",
                one_line(struck),
                one_line(&a.contents)
            );
        }
        AnnotKind::Insert { .. } => return format!("Insert “{}”", one_line(&a.contents)),
        _ => {}
    }
    if a.contents.is_empty() {
        kind_label(&a.kind)
    } else {
        a.contents.replace('\n', " ")
    }
}

fn foreign_text(f: &ForeignAnnot) -> String {
    if f.contents.is_empty() {
        subtype_label(&f.subtype).to_string()
    } else {
        f.contents.replace('\n', " ")
    }
}

/// One comment row, in its `place` in the list: kind and author over the
/// text (wrapped), date at right.
fn comment_row(
    place: gpui::Div,
    id: (&'static str, usize),
    selected: bool,
    title: String,
    snippet: String,
    modified: &str,
) -> gpui::Stateful<gpui::Div> {
    place
        .id(id)
        .px_2()
        .py(px(ROW_PAD))
        .flex()
        .flex_col()
        .cursor_pointer()
        .when(selected, |d| d.bg(theme::SELECTED))
        .hover(|s| s.bg(theme::HOVER))
        .child(
            div()
                .flex()
                .justify_between()
                .text_xs()
                .line_height(px(TITLE_LH))
                .child(
                    div()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_color(theme::TEXT_SUBTLE)
                        .child(title),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .pl_2()
                        .pr(px(18.0))
                        .text_color(theme::TEXT_FAINT)
                        .child(fmt_pdf_date(modified)),
                ),
        )
        .child(
            div()
                .overflow_hidden()
                .text_sm()
                .line_height(px(TEXT_LH))
                .line_clamp(MAX_TEXT_LINES)
                .text_ellipsis()
                .text_color(theme::TEXT)
                .child(snippet),
        )
}

/// Dashed outline for the selected third-party comment on `page`.
pub(crate) fn foreign_outline(
    f: &ForeignAnnot,
    src: PageSize,
    rot: sheaf_core::Rotation,
    k: f32,
) -> AnyElement {
    let r = rotate_rect(
        Rect {
            x: f.rect.x - 2.0,
            y: f.rect.y - 2.0,
            w: f.rect.w + 4.0,
            h: f.rect.h + 4.0,
        },
        src,
        rot,
    );
    div()
        .absolute()
        .left(px(r.x * k))
        .top(px(r.y * k))
        .w(px(r.w * k))
        .h(px(r.h * k))
        .border_1()
        .border_dashed()
        .border_color(theme::ACCENT)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_format() {
        assert_eq!(fmt_pdf_date("D:20261011153000Z"), "2026-10-11 15:30");
        assert_eq!(fmt_pdf_date("D:20261011"), "2026-10-11");
        assert_eq!(fmt_pdf_date(""), "");
        assert_eq!(fmt_pdf_date("garbage"), "");
    }

    #[test]
    fn text_edits_read_as_what_they_suggest() {
        use sheaf_doc::{AnnotId, Box2};
        let at = Box2 {
            x: 0.0,
            y: 0.0,
            w: 6.0,
            h: 6.0,
        };
        let mk = |kind: AnnotKind, contents: &str| Annot {
            id: AnnotId(1),
            kind,
            color: [0, 0, 255],
            opacity: 1.0,
            stroke: 1.0,
            author: String::new(),
            contents: contents.into(),
            modified: String::new(),
            created: String::new(),
            fill: None,
            dashed: false,
        };
        let replace = |contents| {
            mk(
                AnnotKind::Replace {
                    rects: vec![at],
                    struck: "teh".into(),
                    at,
                },
                contents,
            )
        };
        assert_eq!(ours_text(&replace("the")), "Replace “teh” with “the”");
        assert_eq!(ours_text(&replace("")), "Cross out “teh”");
        assert_eq!(
            ours_text(&mk(AnnotKind::Insert { at }, "not")),
            "Insert “not”"
        );
    }

    fn node(i: usize, name: &str, parent: Option<&str>, date: &str) -> Node {
        Node {
            item: Item::Foreign(i),
            name: name.into(),
            parent: parent.map(str::to_string),
            grouped: false,
            date: date.into(),
            state: None,
            author: "Pat".into(),
            text: String::new(),
        }
    }

    #[test]
    fn replies_thread_under_their_comment_in_date_order() {
        let mut strike = node(4, "s", Some("a"), "D:3");
        strike.grouped = true;
        let nodes = vec![
            node(0, "a", None, "D:1"),
            node(1, "late", Some("a"), "D:5"),
            node(2, "early", Some("a"), "D:2"),
            node(3, "lost", Some("gone"), "D:4"),
            strike,
        ];
        let (roots, children) = threads(&nodes);
        // A reply whose comment isn't here stands alone; a grouped part
        // isn't listed at all.
        assert_eq!(roots, vec![0, 3]);
        assert_eq!(children["a"], vec![2, 1]);
    }

    #[test]
    fn a_thread_shows_its_latest_status_and_checkmark() {
        let status = |i, model: &str, state: &str, date| {
            let mut n = node(i, &format!("s{i}"), Some("a"), date);
            n.state = Some((model.into(), state.into()));
            n
        };
        let nodes = vec![
            node(0, "a", None, "D:1"),
            status(1, REVIEW, "Accepted", "D:2"),
            status(2, MARKED, "Marked", "D:3"),
            status(3, REVIEW, "Rejected", "D:4"),
        ];
        let (_, children) = threads(&nodes);
        assert_eq!(
            thread_status(&nodes, &children["a"]),
            (Some("Rejected".to_string()), true)
        );
        let nodes = vec![
            node(0, "a", None, "D:1"),
            status(1, REVIEW, "Accepted", "D:2"),
            status(2, REVIEW, "None", "D:3"),
            status(3, MARKED, "Marked", "D:4"),
            status(4, MARKED, "Unmarked", "D:5"),
        ];
        let (_, children) = threads(&nodes);
        assert_eq!(thread_status(&nodes, &children["a"]), (None, false));
    }
}
