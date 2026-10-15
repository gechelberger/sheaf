//! The Comments panel: every annotation in the document listed by page, as
//! in Acrobat's comments pane. Our own marks are editable (double-click or
//! F2 edits the text, Delete removes); comments other tools left in the
//! file are listed read-only and can be jumped to.

use gpui::{AnyElement, Context, Entity, FocusHandle, Subscription, Window, div, prelude::*, px};
use sheaf_core::PageSize;
use sheaf_doc::{Annot, AnnotId, AnnotKind, PageId, file::ForeignAnnot};
use sheaf_render::Rect;
use sheaf_ui::list::ListLayout;
use sheaf_ui::text_input::{TextInput, TextInputEvent};
use sheaf_ui::theme;

use crate::docview::{DocView, ScrollTo, State, rotate_rect};
use crate::{DeleteComment, EditComment, Redo, Save, Undo};

pub const KEY_CONTEXT: &str = "Comments";

const HEADER_H: f32 = 22.0;
/// A comment row: padding, the kind/author line, then the text, wrapped
/// to at most `MAX_TEXT_LINES` lines.
const ROW_PAD: f32 = 4.0;
const TITLE_LH: f32 = 16.0;
const TEXT_LH: f32 = 20.0;
const MAX_TEXT_LINES: usize = 4;

fn row_h(lines: usize) -> f32 {
    ROW_PAD * 2.0 + TITLE_LH + TEXT_LH * lines as f32
}

pub struct Comments {
    pub filter: Entity<TextInput>,
    pub _sub: Subscription,
    pub scroll: gpui::ScrollHandle,
    pub focus: FocusHandle,
}

#[derive(Clone, Copy)]
enum Row {
    Header { index: usize, count: usize },
    Ours { page: PageId, id: AnnotId },
    Foreign(usize),
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
        });
    }

    /// Make sure the panel exists and give it keyboard focus.
    pub fn focus_comments(&mut self, cx: &mut Context<Self>) {
        self.ensure_comments(cx);
        self.pending_comments_focus = true;
        cx.notify();
    }

    /// Select a third-party comment and scroll to it.
    fn select_foreign(&mut self, ix: usize, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let Some((source, f)) = doc.foreign_annots.get(ix).cloned() else {
            return;
        };
        doc.selected_marks.clear();
        doc.selected_foreign = Some(ix);
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

    /// F2 / double-click: edit the selected mark's text in place.
    fn edit_selected_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let State::Ready(doc) = &self.state else {
            return;
        };
        let Some((page, id)) = doc.selected_annot() else {
            return;
        };
        self.jump_to_annot(id, cx);
        self.start_annot_editor_for(page, id, window, cx);
    }

    /// Rows in display-page order, filtered.
    fn comment_rows(&self, needle: &str) -> Vec<Row> {
        let State::Ready(doc) = &self.state else {
            return Vec::new();
        };
        let st = doc.model.state();
        let matches = |hay: &[&str]| {
            needle.is_empty() || hay.iter().any(|h| h.to_lowercase().contains(needle))
        };
        let mut rows = Vec::new();
        for (index, pref) in st.pages.iter().enumerate() {
            let mut page_rows = Vec::new();
            if let Some(list) = st.annots_on(pref.id) {
                for a in list {
                    if matches(&[&a.contents, &a.author, &kind_label(&a.kind)]) {
                        page_rows.push(Row::Ours {
                            page: pref.id,
                            id: a.id,
                        });
                    }
                }
            }
            for (i, (source, f)) in doc.foreign_annots.iter().enumerate() {
                if *source == pref.source
                    && f.page == pref.index
                    && matches(&[&f.contents, &f.author, subtype_label(&f.subtype)])
                {
                    page_rows.push(Row::Foreign(i));
                }
            }
            if !page_rows.is_empty() {
                rows.push(Row::Header {
                    index,
                    count: page_rows.len(),
                });
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
        let State::Ready(doc) = &self.state else {
            return div().p_2().text_sm().child("…").into_any_element();
        };
        let cm = self.comments.as_ref().unwrap();
        let filter = cm.filter.clone();
        let focus = cm.focus.clone();
        let st = doc.model.state();

        let bounds = cm.scroll.bounds();
        // Variable-height rows: headers are short, and a comment's text
        // wraps to the panel's width.
        let wrap_w = f32::from(bounds.size.width) - 16.0;
        let font = window.text_style().font();
        let font_size = window.rem_size() * 0.875;
        let text_lines = |text: &str| -> usize {
            if wrap_w <= 0.0 || text.is_empty() {
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
                    Some(px(wrap_w)),
                    Some(MAX_TEXT_LINES),
                )
                .map(|ls| ls.iter().map(|l| l.wrap_boundaries().len() + 1).sum())
                .unwrap_or(1)
                .clamp(1, MAX_TEXT_LINES)
        };
        let layout = ListLayout::variable(rows.iter().map(|r| {
            match *r {
                Row::Header { .. } => HEADER_H,
                Row::Ours { page, id } => row_h(
                    st.annots_on(page)
                        .and_then(|l| l.iter().find(|a| a.id == id))
                        .map_or(1, |a| text_lines(&ours_text(a))),
                ),
                Row::Foreign(ix) => row_h(
                    doc.foreign_annots
                        .get(ix)
                        .map_or(1, |(_, f)| text_lines(&foreign_text(f))),
                ),
            }
        }))
        .padded(0.0, 4.0);

        let selected = doc.selected_marks.clone();
        let selected_foreign = doc.selected_foreign;
        let mut els: Vec<AnyElement> = Vec::new();
        let mut count = 0usize;
        for r in &rows {
            if !matches!(r, Row::Header { .. }) {
                count += 1;
            }
        }
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
                Row::Ours { page, id } => {
                    let Some(a) = st
                        .annots_on(page)
                        .and_then(|l| l.iter().find(|a| a.id == id))
                    else {
                        continue;
                    };
                    let is_sel = selected.contains(&(page, id));
                    let snippet = ours_text(a);
                    let title = format!("{}  ·  {}", kind_label(&a.kind), a.author);
                    els.push(
                        comment_row(
                            layout.place(i),
                            ("ours", id.0 as usize),
                            is_sel,
                            title,
                            snippet,
                            &a.modified,
                        )
                        .on_click(cx.listener(move |v, ev: &gpui::ClickEvent, window, cx| {
                            if let State::Ready(doc) = &mut v.state {
                                doc.selected_foreign = None;
                            }
                            v.jump_to_annot(id, cx);
                            if ev.click_count() >= 2 {
                                v.start_annot_editor_for(page, id, window, cx);
                            }
                        }))
                        .child(
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
                                    v.delete_selected_annot(cx);
                                    cx.stop_propagation();
                                }))
                                .child("×"),
                        )
                        .into_any_element(),
                    );
                }
                Row::Foreign(ix) => {
                    let Some((_, f)) = doc.foreign_annots.get(ix) else {
                        continue;
                    };
                    let is_sel = selected_foreign == Some(ix);
                    let snippet = foreign_text(f);
                    let author = if f.author.is_empty() {
                        "(no author)".to_string()
                    } else {
                        f.author.clone()
                    };
                    let title = format!("{}  ·  {author}", subtype_label(&f.subtype));
                    els.push(
                        comment_row(
                            layout.place(i),
                            ("foreign", ix),
                            is_sel,
                            title,
                            snippet,
                            &f.modified,
                        )
                        .on_click(cx.listener(move |v, _, _, cx| v.select_foreign(ix, cx)))
                        .into_any_element(),
                    );
                }
            }
        }

        let status = match (count, needle.is_empty()) {
            (0, true) => "No comments".to_string(),
            (n, true) => format!("{n} comment{}", if n == 1 { "" } else { "s" }),
            (n, false) => format!("{n} matching"),
        };
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
                v.delete_selected_annot(cx);
            }))
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
            .into_any_element()
    }
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
}
