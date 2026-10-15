//! The Comments panel: every annotation in the document listed by page, as
//! in Acrobat's comments pane, in threads. Our own marks are editable
//! (double-click or F2 edits the text, Delete removes); comments other
//! tools left in the file can be replied to and given a status, and
//! jumped to.
//!
//! A thread is a comment and what's in reply to it, by `/NM`: replies, and
//! statuses (review states and checkmarks), ours (`AnnotKind::Reply`,
//! `AnnotKind::Status`) and other tools' (`ForeignAnnot::parent`) alike.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use gpui::{
    AnchoredPositionMode, AnyElement, App, Context, Entity, FocusHandle, Focusable, Pixels, Point,
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
    /// The Filter or Sort popover, and where.
    pub popover: Option<(Popover, Point<Pixels>)>,
    /// The filter's page range, as typed.
    pub pages: Entity<TextInput>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Popover {
    Filter,
    Sort,
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
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Item {
    Ours(PageId, AnnotId),
    Foreign(usize),
}

/// What the panel shows, and the page with it. Each set lists what's
/// hidden, so everything shows until something is turned off.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CommentFilter {
    pub types: BTreeSet<String>,
    pub authors: BTreeSet<String>,
    /// Review states, "None" included.
    pub statuses: BTreeSet<String>,
    pub colors: BTreeSet<[u8; 3]>,
    /// Only checked (true) or only unchecked (false) ones.
    pub checked: Option<bool>,
    /// Display pages, from 0, inclusive.
    pub pages: Option<(usize, usize)>,
    /// PDFlyer's Stamp Summary or Note Summary.
    pub summary: Option<Summary>,
    pub sort: Sort,
}

impl CommentFilter {
    /// Whether it hides anything.
    pub fn is_active(&self) -> bool {
        !self.types.is_empty()
            || !self.authors.is_empty()
            || !self.statuses.is_empty()
            || !self.colors.is_empty()
            || self.checked.is_some()
            || self.pages.is_some()
            || self.summary.is_some()
    }
}

/// PDFlyer's summaries, as filter presets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Summary {
    /// Tickmarks, stamps and connectors.
    Stamps,
    /// Notes and calculator tapes.
    Notes,
}

impl Summary {
    fn types(self) -> &'static [&'static str] {
        match self {
            Summary::Stamps => &["Tickmark", "Stamp", "Connector"],
            Summary::Notes => &["Note", "Calculator tape"],
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Summary::Stamps => "Stamp Summary",
            Summary::Notes => "Note Summary",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Page,
    Author,
    Date,
    Type,
    Color,
}

impl Sort {
    pub const ALL: [Sort; 5] = [
        Sort::Page,
        Sort::Author,
        Sort::Date,
        Sort::Type,
        Sort::Color,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Sort::Page => "Page",
            Sort::Author => "Author",
            Sort::Date => "Date",
            Sort::Type => "Type",
            Sort::Color => "Color",
        }
    }
}

/// The panel's rows and what goes with them, made in one pass over the
/// document and kept until something changes (`ViewKey`).
#[derive(Default)]
pub struct CommentView {
    rows: Vec<Row>,
    /// Each thread's status, by its comment.
    statuses: HashMap<Item, (Option<String>, bool)>,
    /// Comments listed (threads), and threads the filter hides.
    count: usize,
    filtered: usize,
    /// What there is to filter by.
    pub types: BTreeSet<String>,
    pub authors: BTreeSet<String>,
    pub colors: BTreeSet<[u8; 3]>,
    /// Comments the filter (or a deletion) hides on the page.
    pub hidden_ours: HashSet<AnnotId>,
    pub hidden_foreign: HashSet<usize>,
}

/// When the view must be made again: the document's version, the
/// third-party list's, the filter, the search text and the reply box.
pub(crate) type ViewKey = (
    u64,
    u64,
    CommentFilter,
    String,
    Option<(Item, Option<AnnotId>)>,
);

#[derive(Clone)]
enum Row {
    Header {
        label: String,
        swatch: Option<[u8; 3]>,
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
    /// Its type for the filter and sort (`ours_type`, `subtype_label`).
    kind: String,
    color: Option<[u8; 3]>,
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

/// Every comment on the page `pref`: our marks, then the other tools'
/// that are on it (`foreign`, indexes into `Doc::foreign_annots`), less
/// those deleted in sheaf and their threads, which come back second.
fn nodes_on(
    st: &sheaf_doc::DocState,
    foreign_all: &[(sheaf_doc::SourceId, ForeignAnnot)],
    pref: &sheaf_doc::PageRef,
    foreign: &[usize],
) -> (Vec<Node>, Vec<usize>) {
    let page = pref.id;
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
            kind: ours_type(&a.kind).to_string(),
            color: Some(a.color),
        });
    }
    let theirs: Vec<(usize, &ForeignAnnot)> = foreign
        .iter()
        .filter_map(|&i| foreign_all.get(i).map(|(_, f)| (i, f)))
        .collect();
    let gone = match st.removed_comments.get(&page) {
        Some(removed) => sheaf_doc::file::removed_with_threads(
            theirs
                .iter()
                .map(|(_, f)| (f.name.as_str(), f.parent.as_deref())),
            removed,
        ),
        None => HashSet::new(),
    };
    let mut deleted = Vec::new();
    for (i, f) in theirs {
        if gone.contains(&f.name) {
            deleted.push(i);
            continue;
        }
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
            kind: subtype_label(&f.subtype).to_string(),
            color: f.color,
        });
    }
    (nodes, deleted)
}

/// A mark's type for the filter and sort: coarser than its label (every
/// tickmark is a "Tickmark").
fn ours_type(kind: &AnnotKind) -> &'static str {
    match kind {
        AnnotKind::Highlight { .. } => "Highlight",
        AnnotKind::Underline { .. } => "Underline",
        AnnotKind::StrikeOut { .. } => "Strikethrough",
        AnnotKind::Squiggly { .. } => "Squiggly",
        AnnotKind::Replace { .. } => "Replace text",
        AnnotKind::Insert { .. } => "Insert text",
        AnnotKind::Note { .. } => "Note",
        AnnotKind::FreeText { .. } => "Text",
        AnnotKind::Square { .. } => "Rectangle",
        AnnotKind::Ellipse { .. } => "Ellipse",
        AnnotKind::Line { arrow: true, .. } => "Arrow",
        AnnotKind::Line { .. } => "Line",
        AnnotKind::Ink { .. } => "Pencil",
        AnnotKind::Stamp { .. } => "Tickmark",
        AnnotKind::Connector { .. } => "Connector",
        AnnotKind::Tape { .. } => "Calculator tape",
        AnnotKind::Reply { .. } => "Reply",
        AnnotKind::Status { .. } => "Status",
        AnnotKind::PageStamp { .. } | AnnotKind::TextStamp { .. } => "Stamp",
        AnnotKind::PageBorder { .. } => "Page border",
        AnnotKind::Polygon { .. } => "Polygon",
        AnnotKind::Polyline { .. } => "Polyline",
        AnnotKind::Callout { .. } => "Callout",
        AnnotKind::FileAttachment { .. } => "Attachment",
    }
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
        AnnotKind::PageStamp { name, .. } => format!("Stamp {name}"),
        AnnotKind::TextStamp { text, .. } => {
            format!("Stamp {}", text.lines().next().unwrap_or_default())
        }
        AnnotKind::PageBorder { status, .. } => format!("{status} border"),
        AnnotKind::Polygon { .. } => "Polygon".into(),
        AnnotKind::Polyline { .. } => "Polyline".into(),
        AnnotKind::Callout { .. } => "Callout".into(),
        AnnotKind::FileAttachment { name, .. } => format!("Attachment {name}"),
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
        let pages = cx.new(|cx| TextInput::new("All pages", cx));
        self.comments = Some(Comments {
            filter,
            _sub: sub,
            scroll: gpui::ScrollHandle::new(),
            focus: cx.focus_handle(),
            reply: None,
            menu: None,
            popover: None,
            pages,
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
        let Some(pref) = doc
            .model
            .state()
            .pages
            .iter()
            .find(|p| p.id == page)
            .copied()
        else {
            return Vec::new();
        };
        let foreign: Vec<usize> = doc
            .foreign_annots
            .iter()
            .enumerate()
            .filter(|(_, (source, f))| *source == pref.source && f.page == pref.index)
            .map(|(i, _)| i)
            .collect();
        nodes_on(doc.model.state(), &doc.foreign_annots, &pref, &foreign).0
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

    /// Delete: one of our marks, a reply or status of the user's own, or
    /// another tool's comment.
    fn delete_selected_comment(&mut self, cx: &mut Context<Self>) {
        if self.delete_selected_foreign(cx) {
            return;
        }
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

    /// The search text in the panel's box.
    fn comment_needle(&self, cx: &App) -> String {
        self.comments
            .as_ref()
            .map(|cm| cm.filter.read(cx).text().trim().to_lowercase())
            .unwrap_or_default()
    }

    /// The panel's rows and the rest (see `CommentView`), made again only
    /// when something they depend on changed. Also keeps what the page
    /// hides in step: our marks here, other tools' in the engines.
    pub(crate) fn comment_view(&mut self, cx: &mut Context<Self>) -> Arc<CommentView> {
        let needle = self.comment_needle(cx);
        let reply = self
            .comments
            .as_ref()
            .and_then(|cm| cm.reply.as_ref())
            .map(|r| (r.to, r.editing));
        // The page range as typed, into the filter.
        let typed = self
            .comments
            .as_ref()
            .map(|cm| parse_pages(cm.pages.read(cx).text()));
        if let (Some(range), State::Ready(doc)) = (typed, &mut self.state)
            && doc.comment_filter.pages != range
        {
            doc.comment_filter.pages = range;
        }
        let State::Ready(doc) = &self.state else {
            return Arc::default();
        };
        let key: ViewKey = (
            doc.version,
            doc.foreign_generation,
            doc.comment_filter.clone(),
            needle.clone(),
            reply,
        );
        if let Some((k, view)) = &doc.comment_view
            && *k == key
        {
            return view.clone();
        }
        let view = Arc::new(build_view(
            doc.model.state(),
            &doc.foreign_annots,
            &doc.comment_filter,
            &needle,
            reply,
        ));
        if let State::Ready(doc) = &mut self.state {
            doc.hidden_marks = view.hidden_ours.clone();
            doc.comment_view = Some((key, view.clone()));
        }
        self.sync_foreign_visibility(cx);
        view
    }

    /// Hide on screen the other tools' comments that should be (filtered
    /// out, deleted, or all with View > Hide Comments), and show again the
    /// ones sheaf hid that shouldn't be any more. Never touches one the
    /// file hides itself.
    pub(crate) fn sync_foreign_visibility(&mut self, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let hidden_by_view = doc
            .comment_view
            .as_ref()
            .map(|(_, v)| v.hidden_foreign.clone())
            .unwrap_or_default();
        let want: HashSet<(sheaf_doc::SourceId, u32, usize)> = doc
            .foreign_annots
            .iter()
            .enumerate()
            .filter(|(i, (_, f))| !f.hidden && (doc.comments_hidden || hidden_by_view.contains(i)))
            .map(|(_, (source, f))| (*source, f.page, f.slot))
            .collect();
        let hide: Vec<_> = want.difference(&doc.foreign_hidden).copied().collect();
        let show: Vec<_> = doc.foreign_hidden.difference(&want).copied().collect();
        if hide.is_empty() && show.is_empty() {
            return;
        }
        let mut by_source: HashMap<sheaf_doc::SourceId, HashMap<u32, Vec<(usize, bool)>>> =
            HashMap::new();
        for (on, set) in [(true, &hide), (false, &show)] {
            for &(source, page, slot) in set {
                by_source
                    .entry(source)
                    .or_default()
                    .entry(page)
                    .or_default()
                    .push((slot, on));
            }
        }
        doc.foreign_hidden = want;
        for (source, pages) in by_source {
            let Some(handle) = doc.handles.get(&source) else {
                continue;
            };
            let rx = handle.set_annots_hidden(pages.into_iter().collect());
            cx.spawn(async move |this, cx| {
                let _ = rx.await;
                this.update(cx, |v, cx| {
                    if let State::Ready(doc) = &mut v.state {
                        doc.invalidate_tiles();
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    fn set_comment_filter(&mut self, f: impl FnOnce(&mut CommentFilter), cx: &mut Context<Self>) {
        if let State::Ready(doc) = &mut self.state {
            f(&mut doc.comment_filter);
        }
        cx.notify();
    }

    /// Show All: no filter, and comments shown on the page.
    fn show_all_comments(&mut self, cx: &mut Context<Self>) {
        if let State::Ready(doc) = &mut self.state {
            let sort = doc.comment_filter.sort;
            doc.comment_filter = CommentFilter {
                sort,
                ..Default::default()
            };
            doc.comments_hidden = false;
        }
        if let Some(cm) = &mut self.comments {
            cm.pages.update(cx, |t, cx| t.set_text(String::new(), cx));
        }
        cx.notify();
    }

    /// View > Hide Comments / Show Comments.
    pub(crate) fn toggle_comments_shown(&mut self, cx: &mut Context<Self>) {
        if let State::Ready(doc) = &mut self.state {
            doc.comments_hidden = !doc.comments_hidden;
            doc.notice = Some(if doc.comments_hidden {
                "Comments hidden (View > Show Comments)".into()
            } else {
                "Comments shown".into()
            });
        }
        self.sync_foreign_visibility(cx);
        cx.notify();
    }

    /// Delete the selected third-party comment (and its thread), as one
    /// undo step. False when none is selected.
    pub(crate) fn delete_selected_foreign(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(Item::Foreign(ix)) = self.selected_item() else {
            return false;
        };
        let Some((page, name)) = self.item_page_and_name(Item::Foreign(ix)) else {
            return false;
        };
        if let State::Ready(doc) = &mut self.state {
            doc.selected_foreign = None;
        }
        self.apply(
            sheaf_doc::Edit::CommentsDelete {
                page,
                names: vec![name],
            },
            "Delete comment",
            cx,
        );
        true
    }

    /// The Comments panel.
    pub fn render_comments(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.ensure_comments(cx);
        if std::mem::take(&mut self.pending_comments_focus) {
            let focus = self.comments.as_ref().unwrap().focus.clone();
            window.focus(&focus, cx);
        }
        let needle = self.comment_needle(cx);
        let view = self.comment_view(cx);
        let rows = &view.rows;
        let me = crate::prefs::get(cx).identity.name();
        let State::Ready(doc) = &self.state else {
            return div().p_2().text_sm().child("…").into_any_element();
        };
        let cm = self.comments.as_ref().unwrap();
        let filter = cm.filter.clone();
        let pages_input = cm.pages.clone();
        let focus = cm.focus.clone();
        let reply_input = cm.reply.as_ref().map(|r| r.input.clone());
        let menu = cm.menu;
        let popover = cm.popover;
        let st = doc.model.state();
        let cf = doc.comment_filter.clone();
        let comments_hidden = doc.comments_hidden;

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
        // A summary shows when each was applied.
        let applied = cf.summary.is_some();
        let mut els: Vec<AnyElement> = Vec::new();
        for i in layout.visible(&cm.scroll) {
            match rows[i].clone() {
                Row::Header {
                    label,
                    swatch,
                    count,
                } => els.push(
                    layout
                        .place(i)
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .text_xs()
                        .text_color(theme::TEXT_MUTED)
                        .bg(theme::LIST_HEADER)
                        .when_some(swatch, |d, c| {
                            d.child(div().w(px(10.0)).h(px(10.0)).rounded_sm().bg(gpui::rgb(
                                ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | c[2] as u32,
                            )))
                        })
                        .child(format!("{label}  ·  {count}"))
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
                    let (title, snippet, modified, created, deletable, editable) = match item {
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
                                a.created.clone(),
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
                                f.created.clone(),
                                // Their comments, not their replies.
                                depth == 0,
                                false,
                            )
                        }
                    };
                    // A comment's title carries its thread's status.
                    let title = match (depth, view.statuses.get(&item)) {
                        (0, Some((review, marked))) => {
                            let mut t = title;
                            if let Some(r) = review {
                                t.push_str(&format!("  ·  {r}"));
                            }
                            if *marked {
                                t = format!("✓ {t}");
                            }
                            t
                        }
                        _ => title,
                    };
                    let date = if applied && !created.is_empty() {
                        created
                    } else {
                        modified
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
                    if deletable {
                        let del_id = match item {
                            Item::Ours(_, id) => ("del", id.0 as usize),
                            Item::Foreign(ix) => ("del-foreign", ix),
                        };
                        row = row.child(
                            div()
                                .id(del_id)
                                .absolute()
                                .top(px(4.0))
                                .right(px(6.0))
                                .px_1()
                                .rounded_sm()
                                .text_xs()
                                .text_color(theme::TEXT_MUTED)
                                .hover(|s| s.bg(theme::DANGER_HOVER).text_color(theme::TEXT_BRIGHT))
                                .on_click(cx.listener(move |v, _, _, cx| {
                                    v.pick_item(item, item, cx);
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

        let mut status = match (view.count, needle.is_empty()) {
            (0, true) => "No comments".to_string(),
            (n, true) => format!("{n} comment{}", if n == 1 { "" } else { "s" }),
            (n, false) => format!("{n} matching"),
        };
        if view.filtered > 0 {
            status.push_str(&format!("  ·  {} hidden by the filter", view.filtered));
        }
        if comments_hidden {
            status.push_str("  ·  hidden on the page");
        }
        let menu = menu.map(|(item, pos)| self.comment_menu(item, pos, cx));
        let popover = popover.map(|(which, pos)| match which {
            Popover::Filter => self.filter_menu(&view, &cf, pages_input.clone(), pos, cx),
            Popover::Sort => self.sort_menu(cf.sort, pos, cx),
        });
        let chip = |id: &'static str, label: String, on: bool| {
            div()
                .id(id)
                .px_2()
                .py(px(1.0))
                .rounded_sm()
                .cursor_pointer()
                .bg(if on { theme::ACCENT } else { theme::CONTROL })
                .hover(|s| s.bg(theme::CONTROL_HOVER))
                .child(label)
        };
        let filter_on = cf.is_active();
        let controls = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .text_xs()
            .child(
                chip("cf-filter", "Filter ▾".into(), filter_on).on_click(cx.listener(
                    |v, ev: &gpui::ClickEvent, _, cx| {
                        if let Some(cm) = &mut v.comments {
                            cm.popover = Some((Popover::Filter, ev.position()));
                        }
                        cx.notify();
                    },
                )),
            )
            .child(
                chip("cf-sort", format!("Sort: {} ▾", cf.sort.label()), false).on_click(
                    cx.listener(|v, ev: &gpui::ClickEvent, _, cx| {
                        if let Some(cm) = &mut v.comments {
                            cm.popover = Some((Popover::Sort, ev.position()));
                        }
                        cx.notify();
                    }),
                ),
            )
            .children([Summary::Stamps, Summary::Notes].map(|sm| {
                let on = cf.summary == Some(sm);
                chip(
                    if sm == Summary::Stamps {
                        "cf-stamps"
                    } else {
                        "cf-notes"
                    },
                    sm.label().into(),
                    on,
                )
                .on_click(cx.listener(move |v, _, _, cx| {
                    v.set_comment_filter(|f| f.summary = if on { None } else { Some(sm) }, cx)
                }))
            }))
            .when(filter_on || comments_hidden, |d| {
                d.child(
                    chip("cf-all", "Show All".into(), false)
                        .on_click(cx.listener(|v, _, _, cx| v.show_all_comments(cx))),
                )
            });
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
                    .child(controls)
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
            .children(popover)
            .into_any_element()
    }

    /// The Filter popover: a chip per type, author, status and color
    /// there is (on: shown), checked or not, and a page range.
    fn filter_menu(
        &self,
        view: &CommentView,
        cf: &CommentFilter,
        pages: Entity<TextInput>,
        pos: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let section = |label: &'static str| {
            div()
                .pt_2()
                .pb_1()
                .text_xs()
                .text_color(theme::TEXT_MUTED)
                .child(label)
        };
        let chip = |id: (&'static str, usize), label: String, on: bool| {
            div()
                .id(id)
                .px_2()
                .py(px(1.0))
                .rounded_sm()
                .cursor_pointer()
                .text_xs()
                .bg(if on { theme::SELECTED } else { theme::CONTROL })
                .text_color(if on {
                    theme::TEXT_BRIGHT
                } else {
                    theme::TEXT_DIM
                })
                .hover(|s| s.bg(theme::CONTROL_HOVER))
                .child(label)
        };
        let wrap = || div().flex().flex_wrap().gap_1();
        let types = view.types.iter().enumerate().map(|(i, t)| {
            let t = t.clone();
            chip(("cf-type", i), t.clone(), !cf.types.contains(&t)).on_click(cx.listener(
                move |v, _, _, cx| {
                    let t = t.clone();
                    v.set_comment_filter(|f| toggle(&mut f.types, t), cx)
                },
            ))
        });
        let authors = view.authors.iter().enumerate().map(|(i, a)| {
            let a = a.clone();
            chip(("cf-author", i), a.clone(), !cf.authors.contains(&a)).on_click(cx.listener(
                move |v, _, _, cx| {
                    let a = a.clone();
                    v.set_comment_filter(|f| toggle(&mut f.authors, a), cx)
                },
            ))
        });
        let statuses = REVIEW_STATES.iter().enumerate().map(|(i, s)| {
            chip(("cf-status", i), s.to_string(), !cf.statuses.contains(*s)).on_click(cx.listener(
                move |v, _, _, cx| {
                    v.set_comment_filter(|f| toggle(&mut f.statuses, s.to_string()), cx)
                },
            ))
        });
        let checked = [
            (None, "Any"),
            (Some(true), "Checked"),
            (Some(false), "Unchecked"),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (want, label))| {
            chip(("cf-checked", i), label.into(), cf.checked == want).on_click(
                cx.listener(move |v, _, _, cx| v.set_comment_filter(|f| f.checked = want, cx)),
            )
        });
        let colors = view.colors.iter().enumerate().map(|(i, &c)| {
            let on = !cf.colors.contains(&c);
            div()
                .id(("cf-color", i))
                .w(px(16.0))
                .h(px(16.0))
                .rounded_sm()
                .cursor_pointer()
                .bg(gpui::rgb(
                    ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | c[2] as u32,
                ))
                .border_2()
                .border_color(if on { theme::FOCUS_RING } else { theme::SUNKEN })
                .when(!on, |d| d.opacity(0.35))
                .on_click(cx.listener(move |v, _, _, cx| {
                    v.set_comment_filter(|f| toggle(&mut f.colors, c), cx)
                }))
        });
        let body = div()
            .id("filter-menu")
            .occlude()
            .on_mouse_down_out(cx.listener(|v, _, _, cx| {
                if let Some(cm) = &mut v.comments {
                    cm.popover = None;
                }
                cx.notify();
            }))
            .w(px(280.0))
            .max_h(px(480.0))
            .overflow_y_scroll()
            .p_2()
            .rounded_md()
            .bg(theme::POPOVER)
            .border_1()
            .border_color(theme::DIVIDER)
            .shadow_lg()
            .flex()
            .flex_col()
            .child(section("Type"))
            .child(wrap().children(types))
            .child(section("Author"))
            .child(wrap().children(authors))
            .child(section("Status"))
            .child(wrap().children(statuses))
            .child(section("Checkmark"))
            .child(wrap().children(checked))
            .when(!view.colors.is_empty(), |d| {
                d.child(section("Color")).child(wrap().children(colors))
            })
            .child(section("Pages (e.g. 3-10)"))
            .child(pages);
        deferred(
            anchored()
                .position(pos)
                .position_mode(AnchoredPositionMode::Window)
                .child(body),
        )
        .into_any_element()
    }

    fn sort_menu(&self, sort: Sort, pos: Point<Pixels>, cx: &mut Context<Self>) -> AnyElement {
        let items = Sort::ALL.iter().enumerate().map(|(i, &s)| {
            div()
                .id(("sort", i))
                .px_3()
                .py_1()
                .text_sm()
                .cursor_pointer()
                .hover(|st| st.bg(theme::HOVER))
                .child(format!(
                    "{} {}",
                    if s == sort { "•" } else { "  " },
                    s.label()
                ))
                .on_click(cx.listener(move |v, _, _, cx| {
                    if let Some(cm) = &mut v.comments {
                        cm.popover = None;
                    }
                    v.set_comment_filter(|f| f.sort = s, cx)
                }))
        });
        deferred(
            anchored()
                .position(pos)
                .position_mode(AnchoredPositionMode::Window)
                .child(
                    div()
                        .id("sort-menu")
                        .occlude()
                        .on_mouse_down_out(cx.listener(|v, _, _, cx| {
                            if let Some(cm) = &mut v.comments {
                                cm.popover = None;
                            }
                            cx.notify();
                        }))
                        .min_w(px(140.0))
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

/// Make the panel's view of `doc` (see `CommentView`).
fn build_view(
    st: &sheaf_doc::DocState,
    foreign_all: &[(sheaf_doc::SourceId, ForeignAnnot)],
    cf: &CommentFilter,
    needle: &str,
    reply: Option<(Item, Option<AnnotId>)>,
) -> CommentView {
    let mut foreign_on: HashMap<(sheaf_doc::SourceId, u32), Vec<usize>> = HashMap::new();
    for (i, (source, f)) in foreign_all.iter().enumerate() {
        foreign_on.entry((*source, f.page)).or_default().push(i);
    }
    let mut view = CommentView::default();
    // Each thread shown: what it sorts by, its page, its rows.
    let mut kept: Vec<(GroupKey, usize, Vec<Row>)> = Vec::new();
    for (index, pref) in st.pages.iter().enumerate() {
        let foreign = foreign_on
            .get(&(pref.source, pref.index))
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let (nodes, deleted) = nodes_on(st, foreign_all, pref, foreign);
        view.hidden_foreign.extend(deleted);
        if nodes.is_empty() {
            continue;
        }
        let (roots, children) = threads(&nodes);
        for &r in &roots {
            let root = &nodes[r];
            let mut thread = vec![r];
            let mut i = 0;
            while i < thread.len() {
                if let Some(kids) = children.get(&nodes[thread[i]].name) {
                    thread.extend(kids);
                }
                i += 1;
            }
            view.types.insert(root.kind.clone());
            view.authors.insert(author_label(root));
            view.colors.extend(root.color);
            let status = thread_status(&nodes, children.get(&root.name).map_or(&[][..], |v| v));
            view.statuses.insert(root.item, status.clone());
            if !passes(cf, root, &status, index) {
                // Hidden here and on the page, thread and all.
                view.filtered += 1;
                for &n in &thread {
                    match nodes[n].item {
                        Item::Ours(_, id) => {
                            view.hidden_ours.insert(id);
                        }
                        Item::Foreign(i) => {
                            view.hidden_foreign.insert(i);
                        }
                    }
                }
                continue;
            }
            let matches = |n: &Node| {
                needle.is_empty()
                    || n.text.to_lowercase().contains(needle)
                    || n.author.to_lowercase().contains(needle)
            };
            if !thread.iter().any(|&n| matches(&nodes[n])) {
                continue;
            }
            view.count += 1;
            let mut rows = Vec::new();
            push_thread(&nodes, &children, r, root.item, 0, reply, &mut rows);
            kept.push((group_key(cf.sort, root, index), index, rows));
        }
    }
    kept.sort_by(|a, b| a.0.order.cmp(&b.0.order).then(a.1.cmp(&b.1)));
    let mut i = 0;
    while i < kept.len() {
        let j = i + kept[i..]
            .iter()
            .take_while(|k| k.0.order == kept[i].0.order)
            .count();
        view.rows.push(Row::Header {
            label: kept[i].0.label.clone(),
            swatch: kept[i].0.swatch,
            count: j - i,
        });
        for k in &mut kept[i..j] {
            view.rows.append(&mut k.2);
        }
        i = j;
    }
    view
}

/// What a thread is grouped and sorted under.
struct GroupKey {
    order: String,
    label: String,
    swatch: Option<[u8; 3]>,
}

fn group_key(sort: Sort, n: &Node, page: usize) -> GroupKey {
    let simple = |order: String, label: String| GroupKey {
        order,
        label,
        swatch: None,
    };
    match sort {
        Sort::Page => simple(format!("{page:08}"), format!("Page {}", page + 1)),
        Sort::Author => {
            let a = author_label(n);
            simple(a.to_lowercase(), a)
        }
        Sort::Date => {
            let day = fmt_pdf_date(&n.date).chars().take(10).collect::<String>();
            let label = if day.is_empty() {
                "No date".into()
            } else {
                day
            };
            simple(n.date.chars().take(10).collect(), label)
        }
        Sort::Type => simple(n.kind.to_lowercase(), n.kind.clone()),
        Sort::Color => match n.color {
            Some(c) => GroupKey {
                order: format!("{:02x}{:02x}{:02x}", c[0], c[1], c[2]),
                label: crate::markup_tools::COLORS
                    .iter()
                    .find(|(k, _)| *k == c)
                    .map(|(_, name)| name.to_string())
                    .unwrap_or_else(|| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])),
                swatch: Some(c),
            },
            None => simple("~".into(), "No color".into()),
        },
    }
}

fn author_label(n: &Node) -> String {
    if n.author.is_empty() {
        "(no author)".into()
    } else {
        n.author.clone()
    }
}

/// Whether the filter shows a thread whose comment is `n`, with thread
/// status `status`, on display page `page`.
fn passes(cf: &CommentFilter, n: &Node, status: &(Option<String>, bool), page: usize) -> bool {
    if let Some(sm) = cf.summary
        && !sm.types().contains(&n.kind.as_str())
    {
        return false;
    }
    if cf.types.contains(&n.kind) || cf.authors.contains(&author_label(n)) {
        return false;
    }
    if cf.statuses.contains(status.0.as_deref().unwrap_or("None")) {
        return false;
    }
    if n.color.is_some_and(|c| cf.colors.contains(&c)) {
        return false;
    }
    if cf.checked.is_some_and(|want| status.1 != want) {
        return false;
    }
    !cf.pages.is_some_and(|(a, b)| page < a || page > b)
}

/// "3-10" → pages 3 to 10 (from 0: 2 to 9); "5" → page 5; blank or
/// unreadable → all pages.
fn parse_pages(text: &str) -> Option<(usize, usize)> {
    let t = text.trim();
    let num = |s: &str| s.trim().parse::<usize>().ok().filter(|n| *n > 0);
    let (a, b) = match t.split_once(['-', '–']) {
        Some((a, b)) => (num(a)?, num(b)?),
        None => {
            let n = num(t)?;
            (n, n)
        }
    };
    Some((a.min(b) - 1, a.max(b) - 1))
}

fn toggle<T: Ord>(set: &mut BTreeSet<T>, v: T) {
    if !set.remove(&v) {
        set.insert(v);
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
            cloudy: false,
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
            kind: "Note".into(),
            color: None,
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

    #[test]
    fn page_ranges_read_as_typed() {
        assert_eq!(parse_pages("3-10"), Some((2, 9)));
        assert_eq!(parse_pages(" 10 – 3 "), Some((2, 9)));
        assert_eq!(parse_pages("5"), Some((4, 4)));
        assert_eq!(parse_pages(""), None);
        assert_eq!(parse_pages("0"), None);
        assert_eq!(parse_pages("x-2"), None);
    }

    #[test]
    fn the_filter_hides_by_each_facet() {
        let mut n = node(0, "a", None, "D:1");
        n.kind = "Tickmark".into();
        n.color = Some([255, 0, 0]);
        let open = (None, false);
        let mut cf = CommentFilter::default();
        assert!(passes(&cf, &n, &open, 0));
        cf.summary = Some(Summary::Notes);
        assert!(!passes(&cf, &n, &open, 0));
        cf.summary = Some(Summary::Stamps);
        assert!(passes(&cf, &n, &open, 0));
        cf.colors.insert([255, 0, 0]);
        assert!(!passes(&cf, &n, &open, 0));
        cf.colors.clear();
        cf.authors.insert("Pat".into());
        assert!(!passes(&cf, &n, &open, 0));
        cf.authors.clear();
        cf.statuses.insert("None".into());
        assert!(!passes(&cf, &n, &open, 0));
        assert!(passes(&cf, &n, &(Some("Accepted".into()), false), 0));
        cf.statuses.clear();
        cf.checked = Some(true);
        assert!(!passes(&cf, &n, &open, 0));
        assert!(passes(&cf, &n, &(None, true), 0));
        cf.checked = None;
        cf.pages = Some((3, 5));
        assert!(!passes(&cf, &n, &open, 2) && passes(&cf, &n, &open, 4));
    }

    /// The case Acrobat crawled on (acrobat.md §3.2): 1,000 comments over
    /// 900 pages, filtered and sorted.
    #[test]
    fn a_thousand_comments_make_a_view_quickly() {
        use sheaf_doc::{Box2, Document, FixedCount, Pt, SourceId, SourceInfo};
        let mut d = Document::new(
            SourceInfo {
                path: "big.pdf".into(),
                page_count: 900,
                first_page: PageSize {
                    width: 612.0,
                    height: 792.0,
                },
                page_sizes: None,
                bytes: 0,
                password: None,
                data: None,
            },
            &[],
            Box::new(FixedCount(10)),
        );
        let authors = ["Pat", "Greg", "Lee"];
        let mut ours: HashMap<u32, Vec<Annot>> = HashMap::new();
        for i in 0..600u32 {
            ours.entry(i * 3 % 900).or_default().push(Annot {
                id: AnnotId(i as u64 + 1),
                kind: if i % 2 == 0 {
                    AnnotKind::Note {
                        pos: Pt { x: 10.0, y: 10.0 },
                    }
                } else {
                    AnnotKind::Stamp {
                        rect: Box2 {
                            x: 10.0,
                            y: 10.0,
                            w: 20.0,
                            h: 20.0,
                        },
                        glyph: "✓".into(),
                    }
                },
                color: [255, 0, 0],
                opacity: 1.0,
                stroke: 1.0,
                author: authors[i as usize % 3].into(),
                contents: format!("comment {i}"),
                modified: format!("D:2026101{}000000", i % 10),
                created: String::new(),
                fill: None,
                dashed: false,
                cloudy: false,
            });
        }
        d.set_initial_annots(ours).unwrap();
        let source = d.state().pages[0].source;
        let foreign: Vec<(SourceId, ForeignAnnot)> = (0..400u32)
            .map(|i| {
                (
                    source,
                    ForeignAnnot {
                        page: i * 2 % 900,
                        subtype: "Text".into(),
                        author: authors[i as usize % 3].into(),
                        contents: format!("theirs {i}"),
                        modified: "D:20261001000000".into(),
                        rect: Box2 {
                            x: 0.0,
                            y: 0.0,
                            w: 10.0,
                            h: 10.0,
                        },
                        name: format!("t{i}"),
                        parent: None,
                        grouped: false,
                        state: None,
                        created: String::new(),
                        slot: 0,
                        hidden: false,
                        color: Some([0, 0, 255]),
                    },
                )
            })
            .collect();
        let started = std::time::Instant::now();
        let all = build_view(d.state(), &foreign, &CommentFilter::default(), "", None);
        assert_eq!(all.count, 1000);
        let mut cf = CommentFilter {
            sort: Sort::Author,
            ..Default::default()
        };
        cf.authors.insert("Lee".into());
        let some = build_view(d.state(), &foreign, &cf, "comment", None);
        let elapsed = started.elapsed();
        // Lee's are hidden on the page too; the rest are grouped by author.
        assert_eq!(some.filtered, 200 + 133);
        assert!(some.hidden_ours.len() == 200 && some.hidden_foreign.len() == 133);
        let headers: Vec<String> = some
            .rows
            .iter()
            .filter_map(|r| match r {
                Row::Header { label, .. } => Some(label.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(headers, ["Greg", "Pat"]);
        let summary = build_view(
            d.state(),
            &foreign,
            &CommentFilter {
                summary: Some(Summary::Stamps),
                ..Default::default()
            },
            "",
            None,
        );
        assert_eq!(summary.count, 300);
        assert!(
            elapsed < std::time::Duration::from_millis(250),
            "two views took {elapsed:?}"
        );
    }
}
