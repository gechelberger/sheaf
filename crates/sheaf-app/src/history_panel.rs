//! The History panel: every version of the document, oldest first, each
//! with the edits that made it. Click one, or use Up and Down, to go there:
//! that moves through history as undo does, so going back and forth is
//! free, and Save writes whichever version is current. Where edits were
//! undone and then replaced by others, the replaced ones stay, as a branch
//! you can open and go into.

use std::collections::HashSet;
use std::rc::Rc;

use gpui::{AnyElement, Context, FocusHandle, Window, div, prelude::*, px};
use sheaf_doc::{Damaged, VersionId, VersionView};
use sheaf_ui::list::ListLayout;
use sheaf_ui::theme;

use crate::docview::{DocView, State};
use crate::{HistoryNext, HistoryPrev, Redo, Save, Undo};

pub const KEY_CONTEXT: &str = "History";

const VERSION_H: f32 = 40.0;
const EDIT_H: f32 = 18.0;
const BRANCH_H: f32 = 22.0;
const INDENT: f32 = 14.0;

pub struct History {
    pub scroll: gpui::ScrollHandle,
    pub focus: FocusHandle,
    /// Versions whose edits are listed under them.
    expanded: HashSet<VersionId>,
    /// Branches opened, by their first version.
    open_branches: HashSet<VersionId>,
    /// The rows as last built, and what they're for.
    cache: Option<(CacheKey, Rc<Vec<Row>>)>,
}

/// The document's version counter, and whether it has unsaved changes.
type CacheKey = (u64, bool);

#[derive(Clone)]
enum Row {
    Version {
        view: VersionView,
        depth: u8,
    },
    Edit {
        text: String,
        depth: u8,
    },
    Branch {
        start: VersionId,
        len: usize,
        open: bool,
    },
}

impl Row {
    fn height(&self) -> f32 {
        match self {
            Row::Version { .. } => VERSION_H,
            Row::Edit { .. } => EDIT_H,
            Row::Branch { .. } => BRANCH_H,
        }
    }
}

/// `D:20261014153000Z` (UTC, as sheaf writes it) as local time: just the
/// time today, else the date too.
fn fmt_local(d: &str) -> String {
    use chrono::{Local, NaiveDateTime, TimeZone, Utc};
    let s = d.strip_prefix("D:").unwrap_or(d).trim_end_matches('Z');
    let Some(stamp) = s.get(..14) else {
        return crate::comments_panel::fmt_pdf_date(d);
    };
    let Ok(naive) = NaiveDateTime::parse_from_str(stamp, "%Y%m%d%H%M%S") else {
        return crate::comments_panel::fmt_pdf_date(d);
    };
    let local = Utc.from_utc_datetime(&naive).with_timezone(&Local);
    if local.date_naive() == Local::now().date_naive() {
        local.format("%H:%M").to_string()
    } else {
        local.format("%b %-d, %H:%M").to_string()
    }
}

impl History {
    fn new(cx: &mut Context<DocView>) -> Self {
        Self {
            scroll: gpui::ScrollHandle::new(),
            focus: cx.focus_handle(),
            expanded: HashSet::new(),
            open_branches: HashSet::new(),
            cache: None,
        }
    }
}

impl DocView {
    fn ensure_history(&mut self, cx: &mut Context<Self>) {
        if self.history.is_none() {
            self.history = Some(History::new(cx));
        }
    }

    /// Make sure the panel exists and give it keyboard focus.
    pub fn focus_history(&mut self, cx: &mut Context<Self>) {
        self.ensure_history(cx);
        self.pending_history_focus = true;
        cx.notify();
    }

    /// Go to version `id`, as undo and redo do.
    pub(crate) fn jump_to(&mut self, id: VersionId, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        if doc.model.current_version() == id {
            return;
        }
        if let Err(e) = doc.model.jump(id) {
            doc.notice = Some(format!("Can't go there: {e:#}"));
            self.refresh_history();
            cx.notify();
            return;
        }
        let label = doc
            .model
            .timeline()
            .into_iter()
            .find(|v| v.id == id)
            .map(|v| v.label)
            .unwrap_or_default();
        doc.notice = Some(format!("Went to: {label}"));
        doc.touched();
        self.after_model_change(format!("Go to: {label}"), Default::default(), cx);
    }

    /// Up and Down: the version before or after the current one, along the
    /// line the panel shows.
    fn step_history(&mut self, by: isize, cx: &mut Context<Self>) {
        let State::Ready(doc) = &self.state else {
            return;
        };
        let line = doc.model.timeline();
        let Some(at) = line.iter().position(|v| v.current) else {
            return;
        };
        let to = at as isize + by;
        if to >= 0
            && let Some(v) = line.get(to as usize)
        {
            let id = v.id;
            self.jump_to(id, cx);
        }
    }

    fn history_rows(&mut self) -> Rc<Vec<Row>> {
        let State::Ready(doc) = &self.state else {
            return Rc::new(Vec::new());
        };
        let key = (doc.version, doc.model.is_dirty());
        let h = self.history.as_mut().expect("ensured");
        if let Some((k, rows)) = &h.cache
            && *k == key
        {
            return rows.clone();
        }
        let mut rows = Vec::new();
        let push_version = |rows: &mut Vec<Row>, view: VersionView, depth: u8| {
            let open = h.expanded.contains(&view.id);
            let edits = view.edits.clone();
            let single_same = edits.len() == 1 && edits[0] == view.label;
            rows.push(Row::Version { view, depth });
            if open && !single_same {
                rows.extend(edits.into_iter().map(|text| Row::Edit { text, depth }));
            }
        };
        for view in doc.model.timeline() {
            let branches = view.branches.clone();
            push_version(&mut rows, view, 0);
            for start in branches {
                let open = h.open_branches.contains(&start);
                let branch = doc.model.branch(start);
                rows.push(Row::Branch {
                    start,
                    len: branch.len(),
                    open,
                });
                if open {
                    for v in branch {
                        push_version(&mut rows, v, 1);
                    }
                }
            }
        }
        let rows = Rc::new(rows);
        h.cache = Some((key, rows.clone()));
        rows
    }

    /// Build the rows again: a version was found damaged, which no edit
    /// counts.
    pub(crate) fn refresh_history(&mut self) {
        if let Some(h) = &mut self.history {
            h.cache = None;
        }
    }

    fn toggle_expanded(&mut self, id: VersionId, cx: &mut Context<Self>) {
        if let Some(h) = &mut self.history {
            if !h.expanded.remove(&id) {
                h.expanded.insert(id);
            }
            h.cache = None;
        }
        cx.notify();
    }

    fn toggle_branch(&mut self, start: VersionId, cx: &mut Context<Self>) {
        if let Some(h) = &mut self.history {
            if !h.open_branches.remove(&start) {
                h.open_branches.insert(start);
            }
            h.cache = None;
        }
        cx.notify();
    }

    /// The History panel.
    pub fn render_history(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.ensure_history(cx);
        if std::mem::take(&mut self.pending_history_focus) {
            let focus = self.history.as_ref().unwrap().focus.clone();
            window.focus(&focus, cx);
        }
        let rows = self.history_rows();
        if !matches!(self.state, State::Ready(_)) {
            return div().p_2().text_sm().child("…").into_any_element();
        }
        let h = self.history.as_ref().unwrap();
        let (scroll, focus) = (h.scroll.clone(), h.focus.clone());
        let layout = ListLayout::variable(rows.iter().map(Row::height)).padded(0.0, 4.0);
        let versions = rows
            .iter()
            .filter(|r| matches!(r, Row::Version { depth: 0, .. }))
            .count();
        let mut els: Vec<AnyElement> = Vec::new();
        for i in layout.visible(&scroll) {
            let place = layout.place(i);
            els.push(match &rows[i] {
                Row::Version { view, depth } => {
                    let id = view.id;
                    let expandable = view.edits.len() > 1
                        || view.edits.first().is_some_and(|e| *e != view.label);
                    let open = h.expanded.contains(&id);
                    let meta = if view.damaged {
                        Damaged { version: id }.to_string()
                    } else if view.edits.is_empty() {
                        format!("Opened · {}", fmt_local(&view.at))
                    } else if view.author.is_empty() {
                        fmt_local(&view.at)
                    } else {
                        format!("{} · {}", view.author, fmt_local(&view.at))
                    };
                    let text = if view.current {
                        theme::TEXT_BRIGHT
                    } else if view.undone {
                        theme::TEXT_DIM
                    } else {
                        theme::TEXT
                    };
                    place
                        .id(("history", i))
                        .flex()
                        .items_center()
                        .gap_1()
                        .pl(px(6.0 + *depth as f32 * INDENT))
                        .pr_2()
                        .cursor_pointer()
                        .when(view.current, |d| d.bg(theme::SELECTED))
                        .when(!view.current, |d| d.hover(|s| s.bg(theme::HOVER_FAINT)))
                        .on_click(cx.listener(move |v, _, _, cx| v.jump_to(id, cx)))
                        .child(
                            div()
                                .id(("history-expand", i))
                                .w(px(12.0))
                                .text_xs()
                                .text_color(theme::TEXT_MUTED)
                                .when(expandable, |d| {
                                    d.child(if open { "▾" } else { "▸" }).on_click(cx.listener(
                                        move |v, _, _, cx| {
                                            cx.stop_propagation();
                                            v.toggle_expanded(id, cx);
                                        },
                                    ))
                                }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(text)
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(view.label.clone()),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(if view.damaged {
                                            theme::ERROR
                                        } else {
                                            theme::TEXT_MUTED
                                        })
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(meta),
                                ),
                        )
                        .when(view.saved, |d| {
                            d.child(div().text_xs().text_color(theme::SUCCESS).child("Saved"))
                        })
                        .into_any_element()
                }
                Row::Edit { text, depth } => place
                    .flex()
                    .items_center()
                    .pl(px(24.0 + *depth as f32 * INDENT))
                    .pr_2()
                    .text_xs()
                    .text_color(theme::TEXT_DETAIL)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(text.clone())
                    .into_any_element(),
                Row::Branch { start, len, open } => {
                    let start = *start;
                    let what = match len {
                        1 => "1 edit".to_string(),
                        n => format!("{n} edits"),
                    };
                    place
                        .id(("history-branch", i))
                        .flex()
                        .items_center()
                        .pl(px(24.0))
                        .pr_2()
                        .text_xs()
                        .text_color(theme::TEXT_MUTED)
                        .cursor_pointer()
                        .hover(|s| s.text_color(theme::TEXT))
                        .on_click(cx.listener(move |v, _, _, cx| v.toggle_branch(start, cx)))
                        .child(format!(
                            "{} {what} undone and replaced",
                            if *open { "▾" } else { "▸" }
                        ))
                        .into_any_element()
                }
            });
        }
        let status = match versions {
            1 => "No edits yet".to_string(),
            n => format!("{} versions", n),
        };
        div()
            .id("history")
            .key_context(KEY_CONTEXT)
            .track_focus(&focus)
            .size_full()
            .flex()
            .flex_col()
            .on_action(cx.listener(|v, _: &HistoryPrev, _, cx| v.step_history(-1, cx)))
            .on_action(cx.listener(|v, _: &HistoryNext, _, cx| v.step_history(1, cx)))
            .on_action(cx.listener(|v, _: &Undo, _, cx| v.undo(cx)))
            .on_action(cx.listener(|v, _: &Redo, _, cx| v.redo(cx)))
            .on_action(cx.listener(|v, _: &Save, _, cx| v.save(cx).detach()))
            .child(
                div()
                    .flex_shrink_0()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(theme::DIVIDER)
                    .text_xs()
                    .text_color(theme::TEXT_MUTED)
                    .child(status),
            )
            .child(
                div()
                    .id("history-list")
                    .track_scroll(&scroll)
                    .overflow_y_scroll()
                    .flex_1()
                    .min_h(px(0.0))
                    .child(layout.body().children(els)),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_read_locally() {
        // Not today: the date is shown too.
        let s = fmt_local("D:20200102030405Z");
        assert!(s.contains("Jan") && s.contains(':'), "{s}");
        // Unparseable falls back to the plain form.
        assert_eq!(fmt_local("garbage"), "");
    }
}
