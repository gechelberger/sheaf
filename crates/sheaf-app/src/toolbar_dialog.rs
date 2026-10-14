//! View > Customize Toolbar…: every command on the left (searchable like
//! the palette), the toolbar's items on the right. Add, remove, reorder
//! (buttons, Alt+Up/Down, or drag), insert separators, reset. Changes show
//! on the bar as they're made; Cancel puts it back.
//!
//! Keys: type to search; Up/Down pick in the list in use; Tab switches
//! lists; Enter adds (left) or removes (right); Alt+Up/Down move the
//! toolbar item; Delete removes it; Esc cancels.

use gpui::{
    AnyElement, App, Bounds, Context, Entity, EventEmitter, FocusHandle, Focusable, MouseButton,
    Pixels, ScrollHandle, SharedString, Subscription, Window, actions, canvas, div, prelude::*, px,
    svg,
};
use sheaf_ui::button::Button;
use sheaf_ui::list::ListLayout;
use sheaf_ui::text_input::{TextInput, TextInputEvent};
use sheaf_ui::{dialog, theme};

use crate::commands::CommandRegistry;
use crate::toolbar::{Entry, WIDGETS, default_layout};

pub const KEY_CONTEXT: &str = "CustomizeToolbar";

actions!(
    customize_toolbar,
    [
        ListUp,
        ListDown,
        SwitchList,
        AddOrRemove,
        MoveUp,
        MoveDown,
        RemoveItem,
        Cancel
    ]
);

const ROW_H: f32 = 30.0;
const LIST_H: f32 = 360.0;

pub enum CustomizeEvent {
    /// The layout as it stands; show it.
    Changed(Vec<Entry>),
    /// Closed: keep the changes (`true`) or put the layout back.
    Done(bool),
}

/// Something that can go on the toolbar.
struct Choice {
    /// `None`: a separator.
    id: Option<String>,
    icon: SharedString,
    category: String,
    title: String,
    help: String,
    keywords: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Commands,
    Toolbar,
}

pub struct CustomizeToolbar {
    focus: FocusHandle,
    search: Entity<TextInput>,
    _sub: Subscription,
    choices: Vec<Choice>,
    /// Indices into `choices` matching the search, best first.
    matches: Vec<usize>,
    query: String,
    layout: Vec<Entry>,
    left: usize,
    right: Option<usize>,
    active: Side,
    left_scroll: ScrollHandle,
    right_scroll: ScrollHandle,
    /// A toolbar row being dragged, and where it would land.
    drag: Option<usize>,
    drop: Option<usize>,
    right_bounds: Bounds<Pixels>,
}

impl EventEmitter<CustomizeEvent> for CustomizeToolbar {}

impl Focusable for CustomizeToolbar {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// The left list: separator and widgets first, then every command in
/// menu order (Open Recent's entries left out: they change).
fn choices(cx: &App) -> Vec<Choice> {
    let mut out = vec![Choice {
        id: None,
        icon: "".into(),
        category: "Toolbar".into(),
        title: "Separator".into(),
        help: "A line between groups of buttons.".into(),
        keywords: vec!["divider".into(), "line".into()],
    }];
    out.extend(WIDGETS.iter().map(|(id, title)| Choice {
        id: Some(id.to_string()),
        icon: "".into(),
        category: "Toolbar".into(),
        title: title.to_string(),
        help: match *id {
            crate::toolbar::COLORS => "The markup colour swatches.".into(),
            _ => "The tickmark and stamp palette.".into(),
        },
        keywords: vec!["markup".into()],
    }));
    out.extend(
        cx.global::<CommandRegistry>()
            .commands()
            .iter()
            .filter(|c| !c.id.starts_with("recent::"))
            .map(|c| Choice {
                id: Some(c.id.clone()),
                icon: c.icon.clone().into(),
                category: c.category.clone(),
                title: sheaf_ui::menu_bar::parse_label(&c.label)
                    .0
                    .trim_end_matches('…')
                    .to_string(),
                help: c.help.clone(),
                keywords: c.keywords.clone(),
            }),
    );
    out
}

impl CustomizeToolbar {
    pub fn new(layout: Vec<Entry>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| TextInput::new("Search commands", cx));
        let sub = cx.subscribe_in(
            &search,
            window,
            |this, _, ev: &TextInputEvent, window, cx| match ev {
                TextInputEvent::Confirm => this.add(window, cx),
                TextInputEvent::Cancel => cx.emit(CustomizeEvent::Done(false)),
            },
        );
        window.focus(&search.read(cx).focus_handle(cx), cx);
        let choices = choices(cx);
        let matches = (0..choices.len()).collect();
        Self {
            focus: cx.focus_handle(),
            search,
            _sub: sub,
            choices,
            matches,
            query: String::new(),
            layout,
            left: 0,
            right: None,
            active: Side::Commands,
            left_scroll: ScrollHandle::new(),
            right_scroll: ScrollHandle::new(),
            drag: None,
            drop: None,
            right_bounds: Bounds::default(),
        }
    }

    fn refilter(&mut self, query: &str) {
        self.query = query.to_string();
        self.matches = rank(&self.choices, query);
        self.left = 0;
        self.left_scroll.set_offset(gpui::point(px(0.0), px(0.0)));
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(CustomizeEvent::Changed(self.layout.clone()));
        cx.notify();
    }

    /// The chosen command after the selected toolbar item (or at the end).
    fn add(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(&c) = self.matches.get(self.left) else {
            return;
        };
        let entry = match &self.choices[c].id {
            Some(id) => Entry::item(id),
            None => Entry::Separator,
        };
        let at = self.right.map_or(self.layout.len(), |r| r + 1);
        self.layout.insert(at, entry);
        self.right = Some(at);
        ListLayout::fixed(self.layout.len(), ROW_H).reveal(at, 0.0, &self.right_scroll);
        self.changed(cx);
    }

    fn remove(&mut self, cx: &mut Context<Self>) {
        let Some(r) = self.right.filter(|&r| r < self.layout.len()) else {
            return;
        };
        self.layout.remove(r);
        self.right = (!self.layout.is_empty()).then(|| r.min(self.layout.len() - 1));
        self.changed(cx);
    }

    fn move_by(&mut self, by: isize, cx: &mut Context<Self>) {
        let Some(r) = self.right else { return };
        let to = r as isize + by;
        if to < 0 || to as usize >= self.layout.len() {
            return;
        }
        self.layout.swap(r, to as usize);
        self.right = Some(to as usize);
        ListLayout::fixed(self.layout.len(), ROW_H).reveal(to as usize, 0.0, &self.right_scroll);
        self.changed(cx);
    }

    fn insert_separator(&mut self, cx: &mut Context<Self>) {
        let at = self.right.map_or(self.layout.len(), |r| r + 1);
        self.layout.insert(at, Entry::Separator);
        self.right = Some(at);
        self.changed(cx);
    }

    fn reset(&mut self, cx: &mut Context<Self>) {
        self.layout = default_layout();
        self.right = None;
        self.changed(cx);
    }

    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        match self.active {
            Side::Commands => {
                let n = self.matches.len();
                if n > 0 {
                    self.left = (self.left as isize + by).rem_euclid(n as isize) as usize;
                    ListLayout::fixed(n, ROW_H).reveal(self.left, 0.0, &self.left_scroll);
                }
            }
            Side::Toolbar => {
                let n = self.layout.len();
                if n > 0 {
                    let r = self
                        .right
                        .map_or(0, |r| (r as isize + by).rem_euclid(n as isize) as usize);
                    self.right = Some(r);
                    ListLayout::fixed(n, ROW_H).reveal(r, 0.0, &self.right_scroll);
                }
            }
        }
        cx.notify();
    }

    fn switch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.active = match self.active {
            Side::Commands => {
                if self.right.is_none() && !self.layout.is_empty() {
                    self.right = Some(0);
                }
                window.focus(&self.focus, cx);
                Side::Toolbar
            }
            Side::Toolbar => {
                window.focus(&self.search.read(cx).focus_handle(cx), cx);
                Side::Commands
            }
        };
        cx.notify();
    }

    /// Where a dragged toolbar row would land, from the pointer's y.
    fn drop_index(&self, y: Pixels) -> usize {
        let top = self.right_bounds.origin.y + self.right_scroll.offset().y;
        let row = (f32::from(y - top) / ROW_H + 0.5).floor().max(0.0) as usize;
        row.min(self.layout.len())
    }

    fn finish_drag(&mut self, cx: &mut Context<Self>) {
        if let (Some(from), Some(to)) = (self.drag.take(), self.drop.take()) {
            let to = if to > from { to - 1 } else { to };
            if to != from && from < self.layout.len() {
                let e = self.layout.remove(from);
                self.layout.insert(to, e);
                self.right = Some(to);
                self.changed(cx);
            }
        }
        cx.notify();
    }

    /// How a toolbar entry is listed: icon, title and whether it's
    /// available.
    fn describe(&self, entry: &Entry, cx: &App) -> (SharedString, String, bool) {
        match entry {
            Entry::Separator => ("".into(), "— Separator —".into(), true),
            Entry::Invalid(v) => ("".into(), format!("Unreadable item {v}"), false),
            Entry::Item { id, .. } => {
                if let Some((_, title)) = WIDGETS.iter().find(|(w, _)| w == id) {
                    return ("".into(), title.to_string(), true);
                }
                match cx.global::<CommandRegistry>().get(id) {
                    Some(c) => (
                        c.icon.clone().into(),
                        sheaf_ui::menu_bar::parse_label(&c.label)
                            .0
                            .trim_end_matches('…')
                            .to_string(),
                        true,
                    ),
                    None => ("".into(), format!("{id} (not loaded)"), false),
                }
            }
        }
    }
}

/// Choices matching `query`, best first: by category and title, then
/// (ranked lower) by keyword. An empty query lists them all in order.
fn rank(choices: &[Choice], query: &str) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return (0..choices.len()).collect();
    }
    let mut scored: Vec<(i32, usize)> = choices
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let hay = format!("{} {}", c.category, c.title).to_lowercase();
            sheaf_ui::palette::score(&q, &hay)
                .or_else(|| {
                    c.keywords
                        .iter()
                        .filter_map(|k| sheaf_ui::palette::score(&q, &k.to_lowercase()))
                        .max()
                        .map(|s| s - 1000)
                })
                .map(|s| (s, i))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, i)| i).collect()
}

fn icon_cell(icon: &SharedString, color: gpui::Rgba) -> AnyElement {
    let cell = div().size(px(18.0)).flex_shrink_0();
    if icon.is_empty() {
        cell.into_any_element()
    } else {
        cell.child(svg().path(icon.clone()).size(px(18.0)).text_color(color))
            .into_any_element()
    }
}

impl gpui::Render for CustomizeToolbar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let text = self.search.read(cx).text().to_string();
        if text != self.query {
            self.refilter(&text);
        }
        let left_active = self.active == Side::Commands;
        let left_rows: Vec<AnyElement> = {
            let layout = ListLayout::fixed(self.matches.len(), ROW_H);
            layout
                .visible(&self.left_scroll)
                .map(|row| {
                    let c = &self.choices[self.matches[row]];
                    let selected = row == self.left;
                    layout
                        .place(row)
                        .id(("tbc-left", row))
                        .px_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .cursor_pointer()
                        .when(selected, |d| {
                            d.bg(if left_active {
                                theme::SELECTED
                            } else {
                                theme::HOVER
                            })
                        })
                        .hover(|s| s.bg(theme::HOVER))
                        .tooltip(sheaf_ui::tooltip::Tooltip::text(c.help.clone()))
                        .on_click(cx.listener(move |this, e: &gpui::ClickEvent, window, cx| {
                            this.left = row;
                            this.active = Side::Commands;
                            if e.click_count() >= 2 {
                                this.add(window, cx);
                            }
                            cx.notify();
                        }))
                        .child(icon_cell(&c.icon, theme::TEXT_MUTED))
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme::TEXT_MUTED)
                                .flex_shrink_0()
                                .child(format!("{}:", c.category)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme::TEXT_STRONG)
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(c.title.clone()),
                        )
                        .into_any_element()
                })
                .collect()
        };
        let left_body = ListLayout::fixed(self.matches.len(), ROW_H)
            .body()
            .children(left_rows);

        let right_active = self.active == Side::Toolbar;
        let drop_at = self.drop.filter(|_| self.drag.is_some());
        let right_rows: Vec<AnyElement> = (0..self.layout.len())
            .map(|ix| {
                let (icon, title, ok) = self.describe(&self.layout[ix], cx);
                let selected = self.right == Some(ix);
                div()
                    .id(("tbc-right", ix))
                    .h(px(ROW_H))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .when(drop_at == Some(ix), |d| {
                        d.border_t_2().border_color(theme::ACCENT)
                    })
                    .when(selected, |d| {
                        d.bg(if right_active {
                            theme::SELECTED
                        } else {
                            theme::HOVER
                        })
                    })
                    .hover(|s| s.bg(theme::HOVER))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.right = Some(ix);
                            if this.active != Side::Toolbar {
                                this.switch(window, cx);
                            }
                            this.drag = Some(ix);
                            this.drop = None;
                            cx.notify();
                        }),
                    )
                    .on_click(cx.listener(move |this, e: &gpui::ClickEvent, _, cx| {
                        if e.click_count() >= 2 {
                            this.right = Some(ix);
                            this.remove(cx);
                        }
                    }))
                    .child(icon_cell(
                        &icon,
                        if ok {
                            theme::TEXT_MUTED
                        } else {
                            theme::TEXT_DISABLED
                        },
                    ))
                    .child(
                        div()
                            .text_sm()
                            .text_color(if ok {
                                theme::TEXT_STRONG
                            } else {
                                theme::TEXT_DISABLED
                            })
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(title),
                    )
                    .into_any_element()
            })
            .collect();
        let end_marker =
            (drop_at == Some(self.layout.len())).then(|| div().h(px(2.0)).bg(theme::ACCENT));
        let this = cx.entity().downgrade();
        let right_tracker = canvas(
            move |bounds, _, cx| {
                this.update(cx, |d, _| d.right_bounds = bounds).ok();
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();

        let list = |id: &'static str, scroll: &ScrollHandle, active: bool| {
            div()
                .id(id)
                .track_scroll(scroll)
                .overflow_y_scroll()
                .h(px(LIST_H))
                .rounded_md()
                .border_1()
                .border_color(if active {
                    theme::FOCUS_RING
                } else {
                    theme::DIVIDER
                })
                .bg(theme::SUNKEN)
        };
        let middle = div()
            .flex()
            .flex_col()
            .gap_2()
            .justify_center()
            .child(
                Button::new("tbc-add", "Add →")
                    .compact()
                    .on_click(cx.listener(|d, _, window, cx| d.add(window, cx))),
            )
            .child(
                Button::new("tbc-remove", "← Remove")
                    .compact()
                    .on_click(cx.listener(|d, _, _, cx| d.remove(cx))),
            )
            .child(
                Button::new("tbc-up", "Move Up")
                    .compact()
                    .on_click(cx.listener(|d, _, _, cx| d.move_by(-1, cx))),
            )
            .child(
                Button::new("tbc-down", "Move Down")
                    .compact()
                    .on_click(cx.listener(|d, _, _, cx| d.move_by(1, cx))),
            )
            .child(
                Button::new("tbc-sep", "Separator")
                    .compact()
                    .on_click(cx.listener(|d, _, _, cx| d.insert_separator(cx))),
            );

        dialog::backdrop("tbc-backdrop")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|d, _: &ListUp, _, cx| d.step(-1, cx)))
            .on_action(cx.listener(|d, _: &ListDown, _, cx| d.step(1, cx)))
            .on_action(cx.listener(|d, _: &SwitchList, window, cx| d.switch(window, cx)))
            .on_action(cx.listener(|d, _: &AddOrRemove, window, cx| match d.active {
                Side::Commands => d.add(window, cx),
                Side::Toolbar => d.remove(cx),
            }))
            .on_action(cx.listener(|d, _: &RemoveItem, _, cx| d.remove(cx)))
            .on_action(cx.listener(|d, _: &MoveUp, _, cx| d.move_by(-1, cx)))
            .on_action(cx.listener(|d, _: &MoveDown, _, cx| d.move_by(1, cx)))
            .on_action(cx.listener(|_, _: &Cancel, _, cx| cx.emit(CustomizeEvent::Done(false))))
            .on_mouse_move(cx.listener(|d, e: &gpui::MouseMoveEvent, _, cx| {
                if d.drag.is_some() {
                    if e.pressed_button == Some(MouseButton::Left) {
                        let at = d.drop_index(e.position.y);
                        if d.drop != Some(at) {
                            d.drop = Some(at);
                            cx.notify();
                        }
                    } else {
                        d.finish_drag(cx);
                    }
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|d, _, _, cx| d.finish_drag(cx)),
            )
            .child(
                dialog::card("tbc-card", px(760.0))
                    .child(dialog::header("Customize Toolbar"))
                    .child(dialog::subtitle(
                        "Add commands to the toolbar and put them in order. Right-click a toolbar \
                         button or any menu item for a shortcut.",
                    ))
                    .child(
                        div()
                            .flex()
                            .gap_3()
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(self.search.clone())
                                    .child(
                                        list("tbc-left-list", &self.left_scroll, left_active)
                                            .child(left_body),
                                    ),
                            )
                            .child(middle)
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(
                                        div()
                                            .h(px(30.0))
                                            .flex()
                                            .items_center()
                                            .text_sm()
                                            .text_color(theme::TEXT_MUTED)
                                            .child("On the toolbar"),
                                    )
                                    .child(
                                        list("tbc-right-list", &self.right_scroll, right_active)
                                            .relative()
                                            .child(right_tracker)
                                            .children(right_rows)
                                            .children(end_marker),
                                    ),
                            ),
                    )
                    .child(
                        dialog::footer()
                            .child(
                                Button::new("tbc-reset", "Reset to Default")
                                    .on_click(cx.listener(|d, _, _, cx| d.reset(cx))),
                            )
                            .child(dialog::spacer())
                            .child(
                                Button::new("tbc-cancel", "Cancel").on_click(cx.listener(
                                    |_, _, _, cx| cx.emit(CustomizeEvent::Done(false)),
                                )),
                            )
                            .child(
                                Button::new("tbc-ok", "OK")
                                    .primary()
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(CustomizeEvent::Done(true))
                                    })),
                            ),
                    )
                    .child(dialog::hint(
                        "Tab switches lists · Enter adds or removes · Alt+↑/↓ moves · drag to reorder",
                    )),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice(category: &str, title: &str, keywords: &[&str]) -> Choice {
        Choice {
            id: Some(title.into()),
            icon: "".into(),
            category: category.into(),
            title: title.into(),
            help: String::new(),
            keywords: keywords.iter().map(|k| k.to_string()).collect(),
        }
    }

    #[test]
    fn search_ranks_titles_above_keywords() {
        let choices = [
            choice("View", "Rotate View Clockwise", &["turn"]),
            choice("Document", "Rotate Clockwise", &["turn"]),
            choice("Edit", "Undo", &["revert"]),
        ];
        assert_eq!(rank(&choices, ""), [0, 1, 2]);
        assert_eq!(
            rank(&choices, "rotate clock")[0],
            1,
            "shorter first on a tie"
        );
        assert_eq!(rank(&choices, "revert"), [2]);
        assert!(rank(&choices, "zzz").is_empty());
    }
}
