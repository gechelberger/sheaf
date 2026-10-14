//! Command palette: a searchable list of commands. Type to filter, Up/Down
//! to pick, Enter to run. The app supplies the commands; running one emits
//! its action for the app to dispatch. Each row also shows the command's
//! id (in sheaf, what `keymap.json` binds).

use gpui::{
    Action, AnyElement, Context, Entity, EventEmitter, Focusable, MouseButton, Subscription,
    Window, actions, div, prelude::*, px,
};

use crate::list::ListLayout;
use crate::text_input::{TextInput, TextInputEvent};
use crate::{dialog, theme};

actions!(
    palette,
    [PaletteUp, PaletteDown, PalettePageUp, PalettePageDown]
);

pub const KEY_CONTEXT: &str = "Palette";

const ROW_H: f32 = 30.0;
const MAX_ROWS: usize = 12;

/// One row of the palette.
pub struct Command {
    /// Shown faintly after the label.
    pub id: String,
    /// Shown before the label ("View: Fit Width") and searched with it.
    pub category: String,
    pub label: String,
    /// What running the command emits, in `PaletteEvent::Run`.
    pub action: Box<dyn Action>,
    pub shortcut: Option<String>,
}

pub enum PaletteEvent {
    Run(Box<dyn Action>),
    Dismiss,
}

pub struct Palette {
    input: Entity<TextInput>,
    _sub: Subscription,
    commands: Vec<Command>,
    /// Indices into `commands` matching the current query, best first.
    matches: Vec<usize>,
    selected: usize,
    query: String,
    scroll: gpui::ScrollHandle,
}

impl EventEmitter<PaletteEvent> for Palette {}

/// Subsequence match of `query` against `hay` (both lowercase). Higher is
/// better; None when it doesn't match. Contiguous runs and matches at word
/// starts score well, so "fw" finds "Fit Width" before "File > Save".
fn score(query: &str, hay: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let hay: Vec<char> = hay.chars().collect();
    let mut s = 0i32;
    let mut pos = 0usize;
    let mut prev: Option<usize> = None;
    for q in query.chars() {
        let found = hay[pos..].iter().position(|&c| c == q)? + pos;
        let word_start = found == 0 || !hay[found - 1].is_alphanumeric();
        s += match prev {
            Some(p) if p + 1 == found => 3,
            _ if word_start => 2,
            _ => 0,
        };
        prev = Some(found);
        pos = found + 1;
    }
    // Shorter targets win ties.
    Some(s * 100 - hay.len() as i32)
}

impl Palette {
    /// A palette over `commands`, listed in the given order until a query
    /// ranks them. Takes focus.
    pub fn new(commands: Vec<Command>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextInput::new("Type a command", cx));
        let sub = cx.subscribe(&input, |this, _, ev: &TextInputEvent, cx| match ev {
            TextInputEvent::Confirm => this.run_selected(cx),
            TextInputEvent::Cancel => cx.emit(PaletteEvent::Dismiss),
        });
        window.focus(&input.read(cx).focus_handle(cx), cx);
        let matches = (0..commands.len()).collect();
        Self {
            input,
            _sub: sub,
            commands,
            matches,
            selected: 0,
            query: String::new(),
            scroll: gpui::ScrollHandle::new(),
        }
    }

    fn refilter(&mut self, query: &str) {
        self.query = query.to_string();
        let q = query.trim().to_lowercase();
        let mut scored: Vec<(i32, usize)> = self
            .commands
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let hay = format!("{} {}", c.category, c.label).to_lowercase();
                score(&q, &hay).map(|s| (s, i))
            })
            .collect();
        if !q.is_empty() {
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        }
        self.matches = scored.into_iter().map(|(_, i)| i).collect();
        self.selected = 0;
        self.scroll.set_offset(gpui::point(px(0.0), px(0.0)));
    }

    fn run_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(&i) = self.matches.get(self.selected) {
            cx.emit(PaletteEvent::Run(self.commands[i].action.boxed_clone()));
        }
    }

    fn step(&mut self, by: i64, cx: &mut Context<Self>) {
        let n = self.matches.len();
        if n == 0 {
            return;
        }
        self.selected = ((self.selected as i64 + by).rem_euclid(n as i64)) as usize;
        ListLayout::fixed(n, ROW_H).reveal(self.selected, 0.0, &self.scroll);
        cx.notify();
    }
}

impl Render for Palette {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let text = self.input.read(cx).text().to_string();
        if text != self.query {
            self.refilter(&text);
        }
        let n = self.matches.len();
        let layout = ListLayout::fixed(n, ROW_H);
        let rows: Vec<AnyElement> = layout
            .visible(&self.scroll)
            .map(|row| {
                let c = &self.commands[self.matches[row]];
                let selected = row == self.selected;
                layout
                    .place(row)
                    .id(("cmd", row))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .when(selected, |d| d.bg(theme::SELECTED))
                    .hover(|s| s.bg(theme::HOVER))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = row;
                        this.run_selected(cx);
                    }))
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme::TEXT_MUTED)
                            .child(format!("{}:", c.category)),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme::TEXT_STRONG)
                            .child(c.label.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(theme::TEXT_DISABLED)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(c.id.clone()),
                    )
                    .when_some(c.shortcut.clone(), |d, s| {
                        d.child(div().text_xs().text_color(theme::TEXT_DETAIL).child(s))
                    })
                    .into_any_element()
            })
            .collect();
        let list_h = ROW_H * n.clamp(1, MAX_ROWS) as f32;

        dialog::backdrop("palette-backdrop")
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(|this, _: &PaletteUp, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &PaletteDown, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &PalettePageUp, _, cx| this.step(-8, cx)))
            .on_action(cx.listener(|this, _: &PalettePageDown, _, cx| this.step(8, cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(PaletteEvent::Dismiss)),
            )
            .child(
                dialog::card("palette", px(620.0))
                    .p_0()
                    .gap_0()
                    .child(
                        div()
                            .p_2()
                            .border_b_1()
                            .border_color(theme::DIVIDER)
                            .child(self.input.clone()),
                    )
                    .child(
                        div()
                            .id("palette-list")
                            .track_scroll(&self.scroll)
                            .overflow_y_scroll()
                            .h(px(list_h))
                            .child(if n == 0 {
                                div()
                                    .h(px(ROW_H))
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .text_sm()
                                    .text_color(theme::TEXT_DIM)
                                    .child("No matching commands")
                                    .into_any_element()
                            } else {
                                layout.body().children(rows).into_any_element()
                            }),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoring_prefers_word_starts_and_runs() {
        assert!(
            score("fw", "view fit width").unwrap()
                > score("fw", "file save as pdf").unwrap_or(-9999)
        );
        assert!(score("save", "file save").unwrap() > score("save", "file save as").unwrap());
        assert_eq!(score("zzz", "file save"), None);
        assert_eq!(score("", "anything"), Some(0));
    }
}
