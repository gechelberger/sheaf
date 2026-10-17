//! Groups of marks, as PDFlyer has them (M9 decision 8): connectors by
//! label, tickmarks by glyph, stamps by name. Previous/Next walks a group
//! in page order ([ and ], the right-click menu, and a click on a
//! connector), and "pages with" or "without" lists the pages to go to.
//!
//! The Connector tool keeps its label until another is picked, so two
//! clicks make a pair and more extend the group; Shift+I picks the next
//! label no connector uses.

use gpui::{
    AnchoredPositionMode, AnyElement, Context, Pixels, Point, anchored, deferred, div, prelude::*,
    px,
};
use sheaf_doc::annot::{Group, connector_labels, group_of};
use sheaf_doc::{AnnotId, AnnotKind, Box2, PageId, Pt};
use sheaf_ui::theme;

use crate::docview::{DocView, ScrollTo, State};
use crate::markup_tools::Tool;

/// Paper colors a placed tape offers.
const PAPERS: [([u8; 3], &str); 4] = [
    ([255, 255, 255], "White"),
    ([255, 250, 205], "Yellow"),
    ([230, 245, 225], "Green"),
    ([225, 238, 250], "Blue"),
];

/// "Pages with" or "without" a group: which, and the display pages.
pub struct GroupList {
    pub title: String,
    pub pages: Vec<usize>,
}

/// Connector size, as the tool has always placed them.
fn connector_rect(center: Pt) -> Box2 {
    let s = sheaf_doc::annot::CONNECTOR_SIZE;
    Box2 {
        x: center.x - s / 2.0,
        y: center.y - s / 2.0,
        w: s,
        h: s,
    }
}

impl DocView {
    /// The Connector tool's label: the one picked, else the next unused.
    pub(crate) fn connector_label(&mut self) -> String {
        if let Some(l) = &self.connector_label {
            return l.clone();
        }
        let next = match &self.state {
            State::Ready(doc) => doc.model.state().next_connector_label(),
            _ => "1".into(),
        };
        self.connector_label = Some(next.clone());
        next
    }

    /// Shift+I: the next label no connector uses, for a new group.
    pub(crate) fn next_unused_label(&mut self, cx: &mut Context<Self>) {
        self.connector_label = None;
        let l = self.connector_label();
        if self.tool != Tool::Connector {
            self.set_tool(Tool::Connector, cx);
        }
        self.say(format!("Connector {l}: a new group"), cx);
    }

    /// ◀ ▶ on the label bar: the label before or after this one.
    fn step_label(&mut self, by: isize, cx: &mut Context<Self>) {
        let all: Vec<String> = connector_labels().collect();
        let cur = self.connector_label();
        let i = all.iter().position(|l| *l == cur).unwrap_or(0) as isize;
        let j = (i + by).rem_euclid(all.len() as isize) as usize;
        self.connector_label = Some(all[j].clone());
        cx.notify();
    }

    /// Connector tool: a connector with the tool's label at `p`.
    pub(crate) fn place_connector(&mut self, page: PageId, p: Pt, cx: &mut Context<Self>) {
        let label = self.connector_label();
        let a = self.new_annot(
            AnnotKind::Connector {
                rect: connector_rect(p),
                label: label.clone(),
                partner: None,
            },
            crate::markup_tools::color_of(Tool::Connector, cx),
            cx,
        );
        self.add_annot(page, a, &format!("Connector {label}"), cx);
    }

    fn say(&mut self, msg: String, cx: &mut Context<Self>) {
        if let State::Ready(doc) = &mut self.state {
            doc.notice = Some(msg);
        }
        cx.notify();
    }

    /// The selected mark's group, and the mark.
    pub(crate) fn selected_group(&self) -> Option<(Group, AnnotId)> {
        let State::Ready(doc) = &self.state else {
            return None;
        };
        let (_, a) = doc.selected_mark_list().pop()?;
        Some((group_of(&a.kind)?, a.id))
    }

    /// [ and ]: the previous or next mark in the selected one's group,
    /// wrapping.
    pub(crate) fn group_step(&mut self, by: isize, cx: &mut Context<Self>) {
        let Some((group, id)) = self.selected_group() else {
            return self.say("Select a connector, tickmark or stamp first".into(), cx);
        };
        self.group_step_from(group, id, by, cx);
    }

    pub(crate) fn group_step_from(
        &mut self,
        group: Group,
        id: AnnotId,
        by: isize,
        cx: &mut Context<Self>,
    ) {
        let members = match &self.state {
            State::Ready(doc) => doc.model.group_members(&group),
            _ => return,
        };
        let n = members.len();
        let Some(i) = members.iter().position(|(_, _, a)| a.id == id) else {
            return;
        };
        if n < 2 {
            return self.say(format!("The only {}", group.label()), cx);
        }
        let j = (i as isize + by).rem_euclid(n as isize) as usize;
        let next = members[j].2.id;
        self.jump_to_annot(next, cx);
        self.say(format!("{}: {} of {n}", group.label(), j + 1), cx);
    }

    /// "Pages with" (or without) the selected mark's group, as a list.
    pub(crate) fn group_pages(&mut self, with: bool, cx: &mut Context<Self>) {
        let Some((group, _)) = self.selected_group() else {
            return self.say("Select a connector, tickmark or stamp first".into(), cx);
        };
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let mut on: Vec<usize> = doc
            .model
            .group_members(&group)
            .into_iter()
            .map(|(i, _, _)| i)
            .collect();
        on.dedup();
        let pages: Vec<usize> = if with {
            on
        } else {
            (0..doc.model.state().pages.len())
                .filter(|i| !on.contains(i))
                .collect()
        };
        doc.group_list = Some(GroupList {
            title: format!(
                "{} {}: {} page{}",
                if with { "Pages with" } else { "Pages without" },
                group.label(),
                pages.len(),
                if pages.len() == 1 { "" } else { "s" }
            ),
            pages,
        });
        cx.notify();
    }

    /// Drop the pages list. Returns whether there was one.
    pub(crate) fn close_group_list(&mut self, cx: &mut Context<Self>) -> bool {
        let State::Ready(doc) = &mut self.state else {
            return false;
        };
        let had = doc.group_list.take().is_some();
        if had {
            cx.notify();
        }
        had
    }

    /// The Connector tool's label bar, under the go-to bar.
    pub(crate) fn render_connector_bar(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.tool != Tool::Connector {
            return None;
        }
        let label = self.connector_label();
        let placed = match &self.state {
            State::Ready(doc) => doc
                .model
                .group_members(&Group::Connector(label.clone()))
                .len(),
            _ => 0,
        };
        let chip = |id: &'static str, text: String| {
            div()
                .id(id)
                .px_2()
                .h(px(20.0))
                .flex()
                .items_center()
                .rounded_sm()
                .cursor_pointer()
                .bg(theme::CONTROL)
                .hover(|s| s.bg(theme::CONTROL_HOVER))
                .child(text)
        };
        Some(
            div()
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .py_1()
                .bg(theme::PANEL)
                .border_b_1()
                .border_color(theme::DIVIDER)
                .text_xs()
                .text_color(theme::TEXT)
                .child(div().text_color(theme::TEXT_MUTED).child("Connector label"))
                .child(
                    chip("label-prev", "◀".into())
                        .on_click(cx.listener(|v, _, _, cx| v.step_label(-1, cx))),
                )
                .child(
                    div()
                        .px_2()
                        .min_w(px(28.0))
                        .flex()
                        .justify_center()
                        .font_weight(gpui::FontWeight::BOLD)
                        .child(label),
                )
                .child(
                    chip("label-next", "▶".into())
                        .on_click(cx.listener(|v, _, _, cx| v.step_label(1, cx))),
                )
                .child(
                    chip("label-new", "Next unused  Shift+I".into())
                        .on_click(cx.listener(|v, _, _, cx| v.next_unused_label(cx))),
                )
                .child(div().text_color(theme::TEXT_MUTED).child(match placed {
                    0 => "none placed yet: each click places one".to_string(),
                    1 => "1 placed: click to add another".to_string(),
                    n => format!("{n} placed: click to add another"),
                }))
                .into_any_element(),
        )
    }

    /// The pages list, over the top right of the pages.
    pub(crate) fn render_group_list(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let State::Ready(doc) = &self.state else {
            return None;
        };
        let list = doc.group_list.as_ref()?;
        let labels = doc.model.state().page_labels();
        let rows = list.pages.iter().map(|&i| {
            let name = labels
                .as_ref()
                .and_then(|l| l.get(i))
                .filter(|l| **l != (i + 1).to_string())
                .map(|l| format!("Page {} ({l})", i + 1))
                .unwrap_or_else(|| format!("Page {}", i + 1));
            div()
                .id(("group-page", i))
                .px_2()
                .py(px(2.0))
                .cursor_pointer()
                .hover(|s| s.bg(theme::HOVER))
                .child(name)
                .on_click(cx.listener(move |v, _, _, cx| {
                    if let State::Ready(doc) = &mut v.state {
                        doc.scroll_to(ScrollTo::Page(i));
                    }
                    cx.notify();
                }))
        });
        Some(
            div()
                .id("group-list")
                .occlude()
                .absolute()
                .top(px(8.0))
                .right(px(16.0))
                .w(px(220.0))
                .max_h(px(360.0))
                .flex()
                .flex_col()
                .rounded_md()
                .bg(theme::POPOVER)
                .border_1()
                .border_color(theme::DIVIDER)
                .shadow_lg()
                .text_sm()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .border_b_1()
                        .border_color(theme::DIVIDER)
                        .text_xs()
                        .text_color(theme::TEXT_MUTED)
                        .child(list.title.clone())
                        .child(
                            div()
                                .id("group-list-close")
                                .px_1()
                                .cursor_pointer()
                                .hover(|s| s.bg(theme::HOVER))
                                .child("×")
                                .on_click(cx.listener(|v, _, _, cx| {
                                    v.close_group_list(cx);
                                })),
                        ),
                )
                .child(
                    div()
                        .id("group-list-rows")
                        .overflow_y_scroll()
                        .flex_1()
                        .min_h(px(0.0))
                        .py_1()
                        .children(rows),
                )
                .into_any_element(),
        )
    }

    /// Right-click on a mark: open its menu there.
    pub(crate) fn open_mark_menu(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) -> bool {
        let Some((page, p, k)) = self.page_point_k(pos) else {
            return false;
        };
        let page_id = match &self.state {
            State::Ready(doc) => doc.page_ref(page).map(|r| r.id),
            _ => None,
        };
        let Some(page_id) = page_id else {
            return false;
        };
        let Some(a) = self.mark_at(page_id, p, 4.0 / k.max(0.01)) else {
            return false;
        };
        if let State::Ready(doc) = &mut self.state {
            doc.select_mark(Some((page_id, a.id)));
            doc.mark_menu = Some(pos);
        }
        cx.notify();
        true
    }

    /// The right-click menu on a mark: its group, and Delete.
    pub(crate) fn render_mark_menu(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let State::Ready(doc) = &self.state else {
            return None;
        };
        let pos = doc.mark_menu?;
        let group = self.selected_group().map(|(g, _)| g);
        let entry = |id: &'static str, label: String| {
            div()
                .id(id)
                .px_3()
                .py_1()
                .text_sm()
                .cursor_pointer()
                .hover(|s| s.bg(theme::HOVER))
                .child(label)
        };
        let close = |v: &mut DocView| {
            if let State::Ready(doc) = &mut v.state {
                doc.mark_menu = None;
            }
        };
        let mut items: Vec<AnyElement> = Vec::new();
        let tape = doc
            .selected_mark_list()
            .pop()
            .and_then(|(page, a)| match a.kind {
                AnnotKind::Tape { look, .. } => Some((page, a.id, look)),
                _ => None,
            });
        if let Some((page, id, look)) = tape {
            items.push(
                entry("mm-edit-tape", "Edit Tape".into())
                    .on_click(cx.listener(move |v, _, _, cx| {
                        close(v);
                        v.edit_tape(page, id, cx)
                    }))
                    .into_any_element(),
            );
            for (key, label, by) in [
                ("mm-turn-l", "Turn Left", 270),
                ("mm-turn-r", "Turn Right", 90),
            ] {
                items.push(
                    entry(key, label.into())
                        .on_click(cx.listener(move |v, _, _, cx| {
                            close(v);
                            v.turn_tape(by, cx)
                        }))
                        .into_any_element(),
                );
            }
            let (numbers, titles) = (look.numbers, look.titles);
            items.push(
                entry(
                    "mm-numbers",
                    format!("{} Row Numbers", if numbers { "Hide" } else { "Show" }),
                )
                .on_click(cx.listener(move |v, _, _, cx| {
                    close(v);
                    v.tape_look("Tape row numbers", move |l| l.numbers = !numbers, cx)
                }))
                .into_any_element(),
            );
            items.push(
                entry(
                    "mm-titles",
                    format!("{} Titles", if titles { "Hide" } else { "Show" }),
                )
                .on_click(cx.listener(move |v, _, _, cx| {
                    close(v);
                    v.tape_look("Tape titles", move |l| l.titles = !titles, cx)
                }))
                .into_any_element(),
            );
            items.push(
                div()
                    .px_3()
                    .py_1()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().text_sm().text_color(theme::TEXT_MUTED).child("Paper"))
                    .children(PAPERS.iter().enumerate().map(|(i, (c, name))| {
                        let c = *c;
                        div()
                            .id(("mm-paper", i))
                            .w(px(16.0))
                            .h(px(16.0))
                            .rounded_sm()
                            .cursor_pointer()
                            .border_1()
                            .border_color(if look.paper == c {
                                theme::FOCUS_RING
                            } else {
                                theme::DIVIDER
                            })
                            .bg(gpui::rgb(
                                ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | c[2] as u32,
                            ))
                            .tooltip(sheaf_ui::tooltip::Tooltip::text(*name))
                            .on_click(cx.listener(move |v, _, _, cx| {
                                close(v);
                                v.tape_look("Tape paper", move |l| l.paper = c, cx)
                            }))
                    }))
                    .into_any_element(),
            );
            items.push(
                div()
                    .h(px(1.0))
                    .my_1()
                    .bg(theme::DIVIDER)
                    .into_any_element(),
            );
        }
        if let Some(g) = &group {
            let what = g.label();
            items.push(
                entry("mm-next", "Next in Group  ]".into())
                    .on_click(cx.listener(move |v, _, _, cx| {
                        close(v);
                        v.group_step(1, cx)
                    }))
                    .into_any_element(),
            );
            items.push(
                entry("mm-prev", "Previous in Group  [".into())
                    .on_click(cx.listener(move |v, _, _, cx| {
                        close(v);
                        v.group_step(-1, cx)
                    }))
                    .into_any_element(),
            );
            items.push(
                entry("mm-with", format!("Pages with {what}"))
                    .on_click(cx.listener(move |v, _, _, cx| {
                        close(v);
                        v.group_pages(true, cx)
                    }))
                    .into_any_element(),
            );
            items.push(
                entry("mm-without", format!("Pages without {what}"))
                    .on_click(cx.listener(move |v, _, _, cx| {
                        close(v);
                        v.group_pages(false, cx)
                    }))
                    .into_any_element(),
            );
            items.push(
                div()
                    .h(px(1.0))
                    .my_1()
                    .bg(theme::DIVIDER)
                    .into_any_element(),
            );
        }
        items.push(
            entry("mm-delete", "Delete".into())
                .on_click(cx.listener(move |v, _, _, cx| {
                    close(v);
                    v.delete_selected_annot(cx);
                }))
                .into_any_element(),
        );
        Some(
            deferred(
                anchored()
                    .position(pos)
                    .position_mode(AnchoredPositionMode::Window)
                    .child(
                        div()
                            .id("mark-menu")
                            .occlude()
                            .on_mouse_down_out(cx.listener(move |v, _, _, cx| {
                                close(v);
                                cx.notify();
                            }))
                            .min_w(px(200.0))
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
            .into_any_element(),
        )
    }

    /// Change the selected tape's look, as one undo step.
    pub(crate) fn tape_look(
        &mut self,
        label: &str,
        f: impl Fn(&mut sheaf_doc::tape::TapeLook) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.restyle(
            label,
            |a| match &mut a.kind {
                AnnotKind::Tape { look, .. } => {
                    f(look);
                    true
                }
                _ => false,
            },
            cx,
        );
    }

    /// Turn the selected tape a quarter (`by` 90: right, 270: left) about
    /// its middle.
    pub(crate) fn turn_tape(&mut self, by: u16, cx: &mut Context<Self>) {
        self.restyle(
            if by == 90 {
                "Turn tape right"
            } else {
                "Turn tape left"
            },
            |a| match &mut a.kind {
                AnnotKind::Tape { rect, look, .. } => {
                    let (cx_, cy) = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
                    *rect = Box2 {
                        x: cx_ - rect.h / 2.0,
                        y: cy - rect.w / 2.0,
                        w: rect.h,
                        h: rect.w,
                    };
                    look.rotation = (look.rotation + by) % 360;
                    true
                }
                _ => false,
            },
            cx,
        );
    }
}
