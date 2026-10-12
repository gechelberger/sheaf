//! The classic menu bar (File, Edit, View, …) drawn in the title bar on
//! platforms without a system menu bar. Menus come from `cx.get_menus()`,
//! so the same definitions in `menus.rs` drive the macOS bar.
//!
//! Mouse: click a title to open it; while one is open, hovering another
//! switches to it. Keyboard: Alt (tap) or F10 highlights the bar, Alt+letter
//! opens a menu directly, arrows move, Enter runs, Esc backs out, and a
//! letter runs the item it underlines.
//!
//! While active the bar holds keyboard focus. The element focused before
//! that is remembered: items are enabled and dispatched as if it still had
//! focus, and it gets focus back when the bar closes.

use gpui::{
    Action, App, Bounds, Context, FocusHandle, HighlightStyle, KeyDownEvent, Keystroke,
    MouseButton, OwnedMenu, OwnedMenuItem, Pixels, SharedString, StyledText, Subscription,
    UnderlineStyle, Window, actions, anchored, canvas, deferred, div, prelude::*, px, rgb,
};

use crate::menus::{mnemonic, parse_label};

pub const KEY_CONTEXT: &str = "MenuBar";

actions!(
    menu_bar,
    [
        MenuLeft,
        MenuRight,
        MenuUp,
        MenuDown,
        MenuConfirm,
        MenuCancel
    ]
);

pub struct MenuBar {
    menus: Vec<OwnedMenu>,
    focus: FocusHandle,
    /// What had focus before the bar took it.
    restore: Option<FocusHandle>,
    /// The highlighted title, when the bar is active.
    active: Option<usize>,
    /// Whether `active`'s dropdown is showing.
    open: bool,
    /// The highlighted dropdown item.
    item: Option<usize>,
    /// Activated from the keyboard: show mnemonic underlines.
    keyboard: bool,
    /// Bounds of the bar from the last frame, so a click on a title isn't
    /// also treated as a click outside the open dropdown.
    bounds: Bounds<Pixels>,
    _blur: Subscription,
}

impl MenuBar {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        // Focus moved elsewhere (a click into the document, say): just close.
        let blur = cx.on_blur(&focus, window, |bar, _, cx| {
            bar.restore = None;
            bar.reset(cx);
        });
        Self {
            menus: cx.get_menus().unwrap_or_default(),
            focus,
            restore: None,
            active: None,
            open: false,
            item: None,
            keyboard: false,
            bounds: Bounds::default(),
            _blur: blur,
        }
    }

    /// Alt tap / F10: highlight the first title, or close if already active.
    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active.is_some() {
            self.close(window, cx);
        } else if !self.menus.is_empty() {
            self.take_focus(window, cx);
            self.active = Some(0);
            self.open = false;
            self.keyboard = true;
            cx.notify();
        }
    }

    /// Alt+letter: open the menu with that mnemonic. Returns whether one
    /// matched.
    pub fn open_mnemonic(&mut self, ch: char, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(ix) = self
            .menus
            .iter()
            .position(|m| mnemonic(&m.name) == Some(ch))
        else {
            return false;
        };
        self.open_menu(ix, true, window, cx);
        true
    }

    fn open_menu(
        &mut self,
        ix: usize,
        keyboard: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.take_focus(window, cx);
        self.active = Some(ix);
        self.open = true;
        self.keyboard = keyboard;
        self.item = if keyboard {
            self.step_item(None, 1, window, cx)
        } else {
            None
        };
        cx.notify();
    }

    fn take_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus.is_focused(window) {
            self.restore = window.focused(cx);
            window.focus(&self.focus, cx);
        }
    }

    fn reset(&mut self, cx: &mut Context<Self>) {
        if self.active.is_some() {
            self.active = None;
            self.open = false;
            self.item = None;
            cx.notify();
        }
    }

    /// Deactivate and hand focus back. With nothing to hand it back to, the
    /// bar keeps focus so workspace shortcuts still reach the workspace.
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reset(cx);
        if let Some(prev) = self.restore.take() {
            window.focus(&prev, cx);
        }
    }

    fn items(&self) -> &[OwnedMenuItem] {
        self.active
            .and_then(|ix| self.menus.get(ix))
            .map_or(&[], |m| &m.items)
    }

    fn enabled(&self, action: &dyn Action, window: &Window, cx: &App) -> bool {
        match &self.restore {
            Some(prev) => window.is_action_available_in(action, prev),
            None => window.is_action_available(action, cx),
        }
    }

    fn item_enabled(&self, item: &OwnedMenuItem, window: &Window, cx: &App) -> bool {
        match item {
            OwnedMenuItem::Action {
                action, disabled, ..
            } => !disabled && self.enabled(action.as_ref(), window, cx),
            _ => false,
        }
    }

    /// The next enabled item from `from` in direction `dir`, wrapping.
    fn step_item(
        &self,
        from: Option<usize>,
        dir: isize,
        window: &Window,
        cx: &App,
    ) -> Option<usize> {
        let items = self.items();
        let n = items.len() as isize;
        let mut i = from.map_or(if dir > 0 { -1 } else { n }, |i| i as isize);
        for _ in 0..n {
            i = (i + dir).rem_euclid(n);
            if self.item_enabled(&items[i as usize], window, cx) {
                return Some(i as usize);
            }
        }
        None
    }

    fn run(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.items().get(ix) else {
            return;
        };
        if !self.item_enabled(item, window, cx) {
            return;
        }
        let OwnedMenuItem::Action { action, .. } = item else {
            return;
        };
        let action = action.boxed_clone();
        // Restore focus first: dispatch targets whatever is focused now.
        self.close(window, cx);
        window.dispatch_action(action, cx);
    }

    fn move_title(&mut self, dir: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.active else { return };
        let n = self.menus.len() as isize;
        let next = (ix as isize + dir).rem_euclid(n) as usize;
        if self.open {
            self.open_menu(next, true, window, cx);
        } else {
            self.active = Some(next);
            cx.notify();
        }
    }

    fn move_item(&mut self, dir: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.active.is_none() {
            return;
        }
        if !self.open {
            self.open = true;
            self.item = None;
        }
        self.keyboard = true;
        self.item = self.step_item(self.item, dir, window, cx);
        cx.notify();
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match (self.active, self.open, self.item) {
            (Some(_), true, Some(ix)) => self.run(ix, window, cx),
            (Some(ix), false, _) => self.open_menu(ix, true, window, cx),
            _ => {}
        }
    }

    /// Esc closes the dropdown but leaves the title highlighted, as Windows
    /// does; a second Esc leaves the bar.
    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.open = false;
            self.item = None;
            cx.notify();
        } else {
            self.close(window, cx);
        }
    }

    fn on_key_down(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.active.is_none() {
            return;
        }
        let m = &e.keystroke.modifiers;
        if m.control || m.platform || m.function {
            return;
        }
        let mut chars = e.keystroke.key.chars();
        let (Some(ch), None) = (chars.next(), chars.next()) else {
            return;
        };
        let ch = ch.to_ascii_lowercase();
        if self.open {
            let hit = self.items().iter().position(|item| match item {
                OwnedMenuItem::Action { name, .. } => mnemonic(name) == Some(ch),
                _ => false,
            });
            if let Some(ix) = hit {
                cx.stop_propagation();
                self.run(ix, window, cx);
            }
        } else if self.open_mnemonic(ch, window, cx) {
            cx.stop_propagation();
        }
    }

    fn render_dropdown(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self
            .items()
            .iter()
            .enumerate()
            .map(|(ix, item)| match item {
                OwnedMenuItem::Action { name, action, .. } => {
                    let enabled = self.item_enabled(item, window, cx);
                    let highlighted = self.item == Some(ix);
                    div()
                        .id(("menu-item", ix))
                        .px_3()
                        .py_1()
                        .flex()
                        .justify_between()
                        .gap_6()
                        .text_sm()
                        .when(highlighted && enabled, |d| d.bg(rgb(0x3a3a3a)))
                        .text_color(if enabled {
                            rgb(0xd0d0d0)
                        } else {
                            rgb(0x6a6a6a)
                        })
                        .on_mouse_move(cx.listener(move |bar, _, _, cx| {
                            if bar.item != Some(ix) {
                                bar.item = Some(ix);
                                cx.notify();
                            }
                        }))
                        .on_click(cx.listener(move |bar, _, window, cx| bar.run(ix, window, cx)))
                        .child(label(name, self.keyboard))
                        .children(shortcut(action.as_ref(), cx).map(|s| {
                            div()
                                .text_color(if enabled {
                                    rgb(0x8a8a8a)
                                } else {
                                    rgb(0x5a5a5a)
                                })
                                .child(s)
                        }))
                        .into_any_element()
                }
                _ => div().my_1().h(px(1.0)).bg(rgb(0x3a3a3a)).into_any_element(),
            });
        let bar_bounds = self.bounds;
        deferred(
            anchored().snap_to_window().child(
                div()
                    .id("menu-dropdown")
                    .occlude()
                    .on_mouse_down_out(cx.listener(
                        move |bar, e: &gpui::MouseDownEvent, window, cx| {
                            // Clicks on the bar's titles are handled there.
                            if !bar_bounds.contains(&e.position) {
                                bar.close(window, cx);
                            }
                        },
                    ))
                    .min_w(px(220.0))
                    .py_1()
                    .rounded_md()
                    .bg(rgb(0x2a2a2a))
                    .border_1()
                    .border_color(rgb(0x111111))
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .children(rows),
            ),
        )
        .with_priority(1)
    }
}

impl Render for MenuBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let underline = self.keyboard && self.active.is_some() || window.modifiers().alt;
        let titles = self
            .menus
            .iter()
            .enumerate()
            .map(|(ix, menu)| {
                let active = self.active == Some(ix);
                div()
                    .id(("menu-title", ix))
                    .relative()
                    .h_full()
                    .px_2()
                    .flex()
                    .items_center()
                    .text_sm()
                    .rounded_sm()
                    .when(active, |d| d.bg(rgb(0x3a3a3a)))
                    .hover(|s| s.bg(rgb(0x2e2e2e)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |bar, _, window, cx| {
                            // Don't start a window move.
                            cx.stop_propagation();
                            if bar.open && bar.active == Some(ix) {
                                bar.close(window, cx);
                            } else {
                                bar.open_menu(ix, false, window, cx);
                            }
                        }),
                    )
                    .on_hover(cx.listener(move |bar, hovered: &bool, window, cx| {
                        if *hovered && bar.open && bar.active != Some(ix) {
                            let keyboard = bar.keyboard;
                            bar.open_menu(ix, keyboard, window, cx);
                        }
                    }))
                    .child(label(&menu.name, underline))
                    .when(active && self.open, |d| {
                        d.child(
                            div()
                                .absolute()
                                .left_0()
                                .top_full()
                                .child(self.render_dropdown(window, cx)),
                        )
                    })
            })
            .collect::<Vec<_>>();
        let this = cx.entity().downgrade();
        div()
            .id("menu-bar")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .relative()
            .h_full()
            .py_1()
            .flex()
            .items_stretch()
            .text_color(rgb(0xd0d0d0))
            .on_action(cx.listener(|bar, _: &MenuLeft, window, cx| bar.move_title(-1, window, cx)))
            .on_action(cx.listener(|bar, _: &MenuRight, window, cx| bar.move_title(1, window, cx)))
            .on_action(cx.listener(|bar, _: &MenuUp, window, cx| bar.move_item(-1, window, cx)))
            .on_action(cx.listener(|bar, _: &MenuDown, window, cx| bar.move_item(1, window, cx)))
            .on_action(cx.listener(|bar, _: &MenuConfirm, window, cx| bar.confirm(window, cx)))
            .on_action(cx.listener(|bar, _: &MenuCancel, window, cx| bar.cancel(window, cx)))
            .on_key_down(cx.listener(Self::on_key_down))
            .child(
                canvas(
                    move |bounds, _, cx| {
                        this.update(cx, |bar, _| bar.bounds = bounds).ok();
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .children(titles)
    }
}

/// A menu label with its mnemonic optionally underlined.
fn label(name: &str, underline: bool) -> StyledText {
    let (text, at) = parse_label(name);
    let len = at
        .and_then(|i| text[i..].chars().next())
        .map(char::len_utf8);
    let text = StyledText::new(SharedString::from(text));
    match (underline, at, len) {
        (true, Some(i), Some(len)) => text.with_highlights([(
            i..i + len,
            HighlightStyle {
                underline: Some(UnderlineStyle {
                    thickness: px(1.0),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )]),
        _ => text,
    }
}

/// The action's first key binding, Windows-style ("Ctrl+Shift+S"). The
/// first binding is the canonical one; later ones are alternates.
pub fn shortcut(action: &dyn Action, cx: &App) -> Option<SharedString> {
    let keymap = cx.key_bindings();
    let keymap = keymap.borrow();
    let binding = keymap.bindings_for_action(action).next()?;
    let keys: Vec<String> = binding
        .keystrokes()
        .iter()
        .map(|k| keystroke_text(k.inner()))
        .collect();
    Some(keys.join(" ").into())
}

fn keystroke_text(k: &Keystroke) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if k.modifiers.control {
        parts.push("Ctrl");
    }
    if k.modifiers.alt {
        parts.push("Alt");
    }
    if k.modifiers.shift {
        parts.push("Shift");
    }
    if k.modifiers.platform {
        parts.push(if cfg!(target_os = "macos") {
            "Cmd"
        } else {
            "Super"
        });
    }
    let key = match k.key.as_str() {
        "pageup" => "PgUp".to_string(),
        "pagedown" => "PgDn".to_string(),
        "delete" => "Del".to_string(),
        "escape" => "Esc".to_string(),
        k if k.len() > 1 && k.starts_with('f') && k[1..].parse::<u8>().is_ok() => k.to_uppercase(),
        k => {
            let mut c = k.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect())
                .unwrap_or_default()
        }
    };
    let mut out = parts.join("+");
    if !out.is_empty() {
        out.push('+');
    }
    out.push_str(&key);
    out
}
