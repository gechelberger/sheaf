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
use sheaf_doc::annot::{MARKED, REVIEW, REVIEW_STATES};
use sheaf_doc::geom::Affine;
use sheaf_doc::{Annot, AnnotId, AnnotKind, PageId, file::ForeignAnnot};
use sheaf_ops::comments::{
    self, Column, Comment, DataFormat, Layout, Sort, Summary, foreign_text, ours_text, parse_pages,
    status_words, subtype_label, thread_status, threads,
};
pub use sheaf_ops::comments::{CommentFilter, Item, fmt_pdf_date, kind_label};
use sheaf_ui::list::ListLayout;
use sheaf_ui::text_area::{TextArea, TextAreaEvent};
use sheaf_ui::text_input::{TextInput, TextInputEvent};
use sheaf_ui::theme;

use crate::docview::{DocView, State};
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
    Export,
}

/// What File > Export Comments and the panel's Export menu write: the
/// comments the panel lists, in its order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportKind {
    Summary(Layout),
    Csv,
    Text,
    /// XFDF or FDF: what the panel lists, or (`true`) what's selected.
    Data(DataFormat, bool),
}

impl ExportKind {
    fn extension(self) -> &'static str {
        match self {
            ExportKind::Summary(_) => "pdf",
            ExportKind::Csv => "csv",
            ExportKind::Text => "txt",
            ExportKind::Data(f, _) => f.extension(),
        }
    }
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
        if let Some(index) = doc.index_of_page_in(source, f.page) {
            doc.reveal(index, f.rect);
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
                let color = crate::markup_tools::color_of(crate::markup_tools::Tool::Note, cx);
                let mut a = self.new_annot(AnnotKind::Reply { parent: rb.parent }, color, cx);
                a.contents = text;
                self.add_page_annot(rb.page, a, "Reply", cx);
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
            crate::markup_tools::color_of(crate::markup_tools::Tool::Note, cx),
            cx,
        );
        // What Acrobat writes, for viewers that list statuses as text.
        a.contents = format!("{state} set by {}", a.author);
        self.add_page_annot(page, a, "Status", cx);
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
        let list = self.page_comments(page);
        let (_, children) = threads(&list);
        thread_status(&list, children.get(&name).map_or(&[][..], |v| v))
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
    fn page_comments(&self, page: PageId) -> Vec<Comment> {
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
        comments::comments_on(doc.model.state(), &doc.foreign_annots, &pref, &foreign).0
    }

    /// F2 / double-click: edit the selected mark's text in place (a
    /// reply's in the reply box).
    fn edit_selected_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Item::Foreign(ix)) = self.selected_item() {
            self.edit_foreign(ix, window, cx);
            return;
        }
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
                                // Made ours to be edited, when sheaf can
                                // hold it (a reply only if it's the user's).
                                f.mark.is_some() && (depth == 0 || f.author == me),
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
            Popover::Export => self.export_menu(pos, cx),
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
            })
            .child(
                chip("cf-export", "Export ▾".into(), false).on_click(cx.listener(
                    |v, ev: &gpui::ClickEvent, _, cx| {
                        if let Some(cm) = &mut v.comments {
                            cm.popover = Some((Popover::Export, ev.position()));
                        }
                        cx.notify();
                    },
                )),
            );
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

    /// The Export popover: the summary PDFs, CSV and text, and the CSV's
    /// columns (on: included).
    fn export_menu(&self, pos: Point<Pixels>, cx: &mut Context<Self>) -> AnyElement {
        let entry = |i: usize, label: &'static str, kind: ExportKind| {
            div()
                .id(("export", i))
                .px_3()
                .py_1()
                .text_sm()
                .cursor_pointer()
                .hover(|s| s.bg(theme::HOVER))
                .child(label)
                .on_click(cx.listener(move |v, _, _, cx| {
                    if let Some(cm) = &mut v.comments {
                        cm.popover = None;
                    }
                    v.export_comments_prompt(kind, cx);
                }))
        };
        let has_selection = self.selected_item().is_some();
        let chosen = csv_columns(cx);
        let columns = Column::ALL.iter().enumerate().map(|(i, &c)| {
            let on = chosen.contains(&c);
            div()
                .id(("export-col", i))
                .px_2()
                .py(px(1.0))
                .rounded_sm()
                .cursor_pointer()
                .text_xs()
                .bg(if on { theme::ACCENT } else { theme::CONTROL })
                .hover(|s| s.bg(theme::CONTROL_HOVER))
                .child(c.name())
                .on_click(cx.listener(move |_, _, _, cx| {
                    toggle_csv_column(c, cx);
                    cx.notify();
                }))
        });
        deferred(
            anchored()
                .position(pos)
                .position_mode(AnchoredPositionMode::Window)
                .child(
                    div()
                        .id("export-menu")
                        .occlude()
                        .on_mouse_down_out(cx.listener(|v, _, _, cx| {
                            if let Some(cm) = &mut v.comments {
                                cm.popover = None;
                            }
                            cx.notify();
                        }))
                        .w(px(260.0))
                        .py_1()
                        .rounded_md()
                        .bg(theme::POPOVER)
                        .border_1()
                        .border_color(theme::DIVIDER)
                        .shadow_lg()
                        .flex()
                        .flex_col()
                        .child(entry(
                            0,
                            "Summary…",
                            ExportKind::Summary(Layout::CommentsOnly),
                        ))
                        .child(entry(
                            1,
                            "Summary with Pages…",
                            ExportKind::Summary(Layout::Pages),
                        ))
                        .child(entry(2, "CSV…", ExportKind::Csv))
                        .child(entry(3, "Text…", ExportKind::Text))
                        .child(entry(4, "XFDF…", ExportKind::Data(DataFormat::Xfdf, false)))
                        .child(entry(5, "FDF…", ExportKind::Data(DataFormat::Fdf, false)))
                        .when(has_selection, |d| {
                            d.child(entry(
                                6,
                                "Selected as XFDF…",
                                ExportKind::Data(DataFormat::Xfdf, true),
                            ))
                        })
                        .child(
                            div()
                                .px_3()
                                .pt_2()
                                .text_xs()
                                .text_color(theme::TEXT_MUTED)
                                .child("CSV columns"),
                        )
                        .child(
                            div()
                                .px_3()
                                .py_1()
                                .flex()
                                .flex_wrap()
                                .gap_1()
                                .children(columns),
                        ),
                ),
        )
        .into_any_element()
    }

    /// File > Export Comments, or the panel's Export menu: ask where, then
    /// write what the panel lists.
    pub(crate) fn export_comments_prompt(&mut self, kind: ExportKind, cx: &mut Context<Self>) {
        let dir = self
            .path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default();
        let stem = self
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("document");
        let suggested = match kind {
            ExportKind::Summary(Layout::Pages) => format!("{stem} - comments with pages.pdf"),
            ExportKind::Data(..) => format!("{stem}.{}", kind.extension()),
            _ => format!("{stem} - comments.{}", kind.extension()),
        };
        let rx = crate::file_dialog::save(cx, &dir, Some(&suggested));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                this.update(cx, |view, cx| view.export_comments_to(kind, path, cx))
                    .ok();
            }
        })
        .detach();
    }

    /// Write the comments the panel lists, as `kind`, to `path`: with its
    /// filter, sort and search, whether or not it's open.
    pub fn export_comments_to(
        &mut self,
        kind: ExportKind,
        path: std::path::PathBuf,
        cx: &mut Context<Self>,
    ) {
        if self.still_opening(cx) {
            return;
        }
        let needle = self.comment_needle(cx);
        let columns = csv_columns(cx);
        let title = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let selected: Vec<Item> = doc
            .selected_marks
            .iter()
            .map(|&(p, a)| Item::Ours(p, a))
            .chain(doc.selected_foreign.map(Item::Foreign))
            .collect();
        doc.notice = Some(format!("Writing comments to {}…", path.display()));
        let snapshot = doc.model.snapshot();
        let foreign = doc.foreign_annots.clone();
        let cf = doc.comment_filter.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    async move {
                        let listing = comments::list(&snapshot.state, &foreign, &cf, &needle);
                        let labels = snapshot.state.page_labels();
                        let bytes = match kind {
                            ExportKind::Summary(layout) => comments::summary_pdf(
                                &snapshot, &foreign, &cf, &needle, layout, &title,
                            )?,
                            ExportKind::Csv => {
                                comments::csv(&listing, labels.as_deref(), &columns).into_bytes()
                            }
                            ExportKind::Text => {
                                comments::text(&listing, labels.as_deref(), &title, cf.sort)
                                    .into_bytes()
                            }
                            ExportKind::Data(format, only_selected) => {
                                let items = if only_selected {
                                    selected.clone()
                                } else {
                                    comments::items(&listing)
                                };
                                comments::export_data(&snapshot, &foreign, &items, format)?
                            }
                        };
                        std::fs::write(&path, bytes)
                            .map_err(|e| anyhow::anyhow!("writing {}: {e}", path.display()))?;
                        anyhow::Ok(match kind {
                            ExportKind::Data(_, true) => selected.len(),
                            _ => listing.count,
                        })
                    }
                })
                .await;
            this.update(cx, |view, cx| {
                if let State::Ready(doc) = &mut view.state {
                    doc.notice = Some(match result {
                        Ok(n) => format!(
                            "Wrote {n} comment{} to {}",
                            if n == 1 { "" } else { "s" },
                            path.display()
                        ),
                        Err(e) => format!("Export failed: {e:#}"),
                    });
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// File > Comments > Import: ask for the file, then bring its comments
    /// in.
    pub(crate) fn import_comments_prompt(&mut self, cx: &mut Context<Self>) {
        let rx = crate::file_dialog::open_filtered(
            cx,
            gpui::PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("Import Comments".into()),
            },
            crate::file_dialog::Filter::Comments,
        );
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(path) = paths.into_iter().next()
            {
                this.update(cx, |view, cx| view.import_comments_from(path, cx))
                    .ok();
            }
        })
        .detach();
    }

    /// Bring in the comments in the FDF, XFDF or PDF file at `path`, as
    /// one undo step: one here already by name is updated, the rest added.
    pub fn import_comments_from(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        // What's brought in is matched against the comments already here.
        if self.still_opening(cx) {
            return;
        }
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        doc.notice = Some(format!("Reading comments from {}…", path.display()));
        let snapshot = doc.model.snapshot();
        let foreign = doc.foreign_annots.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    async move {
                        let bytes = std::fs::read(&path)?;
                        comments::import_data(&snapshot, &foreign, &bytes, None)
                    }
                })
                .await;
            this.update(cx, |view, cx| {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                match result {
                    Ok(got) => {
                        let summary = got.summary();
                        if !got.edits.is_empty() {
                            view.apply_batch(
                                got.edits,
                                &format!("Import comments from {name}"),
                                cx,
                            );
                        }
                        view.set_notice(format!("Imported from {name}: {summary}"), cx);
                    }
                    Err(e) => view.set_notice(format!("Import failed: {e:#}"), cx),
                }
            })
            .ok();
        })
        .detach();
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
        // Another tool's comment that sheaf can hold: make it a mark, to
        // move, resize and restyle.
        if let Item::Foreign(ix) = item
            && self
                .foreign_mark(ix)
                .is_some_and(|(_, m)| m.thread_parent().is_none())
        {
            items.push(
                entry(("cm-adopt", 0), "Make Editable".into())
                    .on_click(cx.listener(move |v, _, _, cx| {
                        if let Some(cm) = &mut v.comments {
                            cm.menu = None;
                        }
                        v.adopt_foreign(ix, |_| {}, "Make comment editable", cx);
                    }))
                    .into_any_element(),
            );
        }
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

/// Make the panel's view of `doc` (see `CommentView`): the list, as
/// rows, with the reply box where one is open.
fn build_view(
    st: &sheaf_doc::DocState,
    foreign_all: &[(sheaf_doc::SourceId, ForeignAnnot)],
    cf: &CommentFilter,
    needle: &str,
    reply: Option<(Item, Option<AnnotId>)>,
) -> CommentView {
    let listing = comments::list(st, foreign_all, cf, needle);
    let mut rows = Vec::new();
    for g in &listing.groups {
        rows.push(Row::Header {
            label: g.label.clone(),
            swatch: g.swatch,
            count: g.threads.len(),
        });
        for t in &g.threads {
            push_thread(t, reply, &mut rows);
        }
    }
    CommentView {
        rows,
        statuses: listing.statuses,
        count: listing.count,
        filtered: listing.filtered,
        types: listing.types,
        authors: listing.authors,
        colors: listing.colors,
        hidden_ours: listing.hidden_ours,
        hidden_foreign: listing.hidden_foreign,
    }
}

/// A thread's rows, depth first, and the reply box where one is open: in
/// place of the reply being edited, or under everything in reply to the
/// comment being replied to.
fn push_thread(t: &comments::Thread, reply: Option<(Item, Option<AnnotId>)>, rows: &mut Vec<Row>) {
    let root = t.entries[0].comment.item;
    // A new reply's box waits for the end of what's under its comment,
    // at whose depth.
    let mut pending: Option<usize> = None;
    for e in &t.entries {
        if let Some(d) = pending
            && e.depth <= d
        {
            rows.push(Row::ReplyBox {
                depth: (d + 1).min(3),
            });
            pending = None;
        }
        let item = e.comment.item;
        let depth = e.depth.min(3);
        match reply {
            Some((to, Some(_))) if to == item => rows.push(Row::ReplyBox { depth }),
            _ if e.comment.state.is_some() => rows.push(Row::Status { item, root, depth }),
            _ => rows.push(Row::Comment { item, root, depth }),
        }
        if matches!(reply, Some((to, None)) if to == item) {
            pending = Some(e.depth);
        }
    }
    if let Some(d) = pending {
        rows.push(Row::ReplyBox {
            depth: (d + 1).min(3),
        });
    }
}

/// The CSV's columns, as last chosen.
fn csv_columns(cx: &App) -> Vec<Column> {
    let chosen: Vec<Column> = crate::state::get(cx)
        .comment_columns
        .iter()
        .filter_map(|c| Column::from_name(c))
        .collect();
    if chosen.is_empty() {
        Column::DEFAULT.to_vec()
    } else {
        chosen
    }
}

/// Put `c` in the CSV or take it out; the last one stays.
fn toggle_csv_column(c: Column, cx: &mut App) {
    let mut chosen = csv_columns(cx);
    if chosen.contains(&c) {
        if chosen.len() > 1 {
            chosen.retain(|x| *x != c);
        }
    } else {
        chosen.push(c);
    }
    let names: Vec<String> = Column::ALL
        .iter()
        .filter(|c| chosen.contains(c))
        .map(|c| c.name().to_string())
        .collect();
    crate::state::update(cx, |s| s.comment_columns = names);
}

fn toggle<T: Ord>(set: &mut BTreeSet<T>, v: T) {
    if !set.remove(&v) {
        set.insert(v);
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

/// Dashed outline for the selected third-party comment on a page:
/// `to_pane` takes PageSpace onto the page as drawn, at `k` px a point.
pub(crate) fn foreign_outline(f: &ForeignAnnot, to_pane: &Affine, k: f32) -> AnyElement {
    let r = to_pane.apply_box(f.rect);
    div()
        .absolute()
        .left(px((r.x - 2.0) * k))
        .top(px((r.y - 2.0) * k))
        .w(px((r.w + 4.0) * k))
        .h(px((r.h + 4.0) * k))
        .border_1()
        .border_dashed()
        .border_color(theme::ACCENT)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sheaf_doc::{Box2, Document, FixedCount, SourceInfo, Store};

    /// The rows for a page with a thread (a; r in reply to it; rr in reply
    /// to r) and a comment b, as short strings.
    fn rows(reply: Option<(Item, Option<AnnotId>)>) -> Vec<String> {
        let store = Store::new();
        let primary = SourceInfo {
            name: "x.pdf".into(),
            ..SourceInfo::blank(&store, 1)
        };
        let d = Document::new(store, primary, &[], Box::new(FixedCount(10)));
        let source = d.state().pages[0].source;
        let theirs = |name: &str, parent: Option<&str>, date: &str| {
            (
                source,
                ForeignAnnot {
                    page: 0,
                    subtype: "Text".into(),
                    author: "Ann".into(),
                    contents: name.into(),
                    modified: date.into(),
                    rect: Box2 {
                        x: 0.0,
                        y: 0.0,
                        w: 10.0,
                        h: 10.0,
                    },
                    name: name.into(),
                    parent: parent.map(str::to_string),
                    grouped: false,
                    state: None,
                    created: String::new(),
                    slot: 0,
                    hidden: false,
                    color: None,
                    mark: None,
                },
            )
        };
        let foreign = vec![
            theirs("a", None, "D:1"),
            theirs("r", Some("a"), "D:2"),
            theirs("rr", Some("r"), "D:3"),
            theirs("b", None, "D:4"),
        ];
        let view = build_view(d.state(), &foreign, &CommentFilter::default(), "", reply);
        view.rows
            .iter()
            .map(|r| match r {
                Row::Header { label, count, .. } => format!("{label} ({count})"),
                Row::Comment {
                    item: Item::Foreign(i),
                    depth,
                    ..
                } => format!("{} at {depth}", foreign[*i].1.name),
                Row::ReplyBox { depth } => format!("box at {depth}"),
                _ => "?".into(),
            })
            .collect()
    }

    #[test]
    fn the_reply_box_goes_where_the_reply_will() {
        assert_eq!(
            rows(None),
            ["Page 1 (2)", "a at 0", "r at 1", "rr at 2", "b at 0"]
        );
        // A new reply to a comment goes under everything in reply to it.
        assert_eq!(
            rows(Some((Item::Foreign(0), None))),
            [
                "Page 1 (2)",
                "a at 0",
                "r at 1",
                "rr at 2",
                "box at 1",
                "b at 0"
            ]
        );
        assert_eq!(
            rows(Some((Item::Foreign(1), None))),
            [
                "Page 1 (2)",
                "a at 0",
                "r at 1",
                "rr at 2",
                "box at 2",
                "b at 0"
            ]
        );
        // One being edited is replaced by the box.
        assert_eq!(
            rows(Some((Item::Foreign(1), Some(AnnotId(9))))),
            ["Page 1 (2)", "a at 0", "box at 1", "rr at 2", "b at 0"]
        );
    }
}
