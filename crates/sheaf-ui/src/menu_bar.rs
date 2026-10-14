//! The classic menu bar (File, Edit, View, …) drawn in the title bar on
//! platforms without a system menu bar. Menus come from `cx.get_menus()`,
//! so the same definitions in `menus.rs` drive the macOS bar.
//!
//! Mouse: click a title to open it; while one is open, hovering another
//! switches to it. Keyboard: Alt (tap) or F10 highlights the bar, Alt+letter
//! opens a menu directly, arrows move, Enter runs, Esc backs out, and a
//! letter runs the item it underlines.
//!
//! The app can say more about each item through [`MenuBar::set_state`]:
//! whether it's enabled now (Undo with nothing to undo isn't) and, for
//! items that switch something on and off, whether it's on (drawn with a
//! checkmark).
//!
//! While active the bar holds keyboard focus. The element focused before
//! that is remembered: items are enabled and dispatched as if it still had
//! focus, and it gets focus back when the bar closes.

use gpui::{
    Action, App, Bounds, Context, FocusHandle, HighlightStyle, KeyDownEvent, Keystroke,
    MouseButton, OwnedMenu, OwnedMenuItem, Pixels, SharedString, StyledText, Subscription,
    UnderlineStyle, Window, actions, anchored, canvas, deferred, div, prelude::*, px,
};

use crate::theme;

pub const KEY_CONTEXT: &str = "MenuBar";

/// What the app says about a menu item right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemState {
    pub enabled: bool,
    /// For an item that switches something on and off: whether it's on.
    pub checked: Option<bool>,
}

impl Default for ItemState {
    fn default() -> Self {
        Self {
            enabled: true,
            checked: None,
        }
    }
}

/// Asked about each item as it's drawn or stepped over.
pub type StateFn = std::rc::Rc<dyn Fn(&dyn Action, &Window, &App) -> ItemState>;

/// Called when an item is right-clicked (after the menu closes), with its
/// action and the pointer's window position.
pub type SecondaryFn = std::rc::Rc<dyn Fn(&dyn Action, gpui::Point<Pixels>, &mut Window, &mut App)>;

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
    /// The open submenu: its item index in the dropdown, and the
    /// highlighted item in it.
    sub: Option<usize>,
    sub_item: Option<usize>,
    /// Activated from the keyboard: show mnemonic underlines.
    keyboard: bool,
    /// Highlight the first enabled item on the next draw. Deferred rather
    /// than done when the menu opens: the app's [`StateFn`] may read the
    /// view that asked for it to open.
    pick_first: bool,
    state: Option<StateFn>,
    secondary: Option<SecondaryFn>,
    /// Bounds of the bar from the last frame, so a click on a title isn't
    /// also treated as a click outside the open dropdown.
    bounds: Bounds<Pixels>,
    /// Bounds of the open submenu, likewise.
    sub_bounds: Bounds<Pixels>,
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
            sub: None,
            sub_item: None,
            keyboard: false,
            pick_first: false,
            state: None,
            secondary: None,
            bounds: Bounds::default(),
            sub_bounds: Bounds::default(),
            _blur: blur,
        }
    }

    /// Ask `state` about items from now on.
    pub fn set_state(&mut self, state: StateFn) {
        self.state = Some(state);
    }

    /// Call `f` when an item is right-clicked.
    pub fn set_on_secondary(&mut self, f: SecondaryFn) {
        self.secondary = Some(f);
    }

    fn app_state(&self, action: &dyn Action, window: &Window, cx: &App) -> ItemState {
        self.state
            .as_ref()
            .map(|f| f(action, window, cx))
            .unwrap_or_default()
    }

    /// Pick up changed menus (a plugin turned on or off).
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.menus = cx.get_menus().unwrap_or_default();
        self.restore = None;
        self.reset(cx);
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
        self.sub = None;
        self.sub_item = None;
        self.item = None;
        self.pick_first = keyboard;
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
            self.sub = None;
            self.sub_item = None;
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

    /// The open submenu's items.
    fn sub_items(&self) -> &[OwnedMenuItem] {
        match self.sub.and_then(|i| self.items().get(i)) {
            Some(OwnedMenuItem::Submenu(m)) => &m.items,
            _ => &[],
        }
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
            } => {
                !disabled
                    && self.enabled(action.as_ref(), window, cx)
                    && self.app_state(action.as_ref(), window, cx).enabled
            }
            OwnedMenuItem::Submenu(m) => m.items.iter().any(|i| self.item_enabled(i, window, cx)),
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
        self.step_in(self.items(), from, dir, window, cx)
    }

    fn step_in(
        &self,
        items: &[OwnedMenuItem],
        from: Option<usize>,
        dir: isize,
        window: &Window,
        cx: &App,
    ) -> Option<usize> {
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
        if let OwnedMenuItem::Submenu(_) = item {
            self.open_sub(ix, true, window, cx);
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

    /// Show the submenu at dropdown item `ix`; from the keyboard, with its
    /// first item highlighted.
    fn open_sub(&mut self, ix: usize, keyboard: bool, window: &Window, cx: &mut Context<Self>) {
        self.item = Some(ix);
        self.sub = Some(ix);
        self.sub_item = if keyboard {
            self.step_in(self.sub_items(), None, 1, window, cx)
        } else {
            None
        };
        cx.notify();
    }

    fn run_sub(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.sub_items().get(ix) else {
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
        if self.sub.is_some() {
            self.sub_item = self.step_in(self.sub_items(), self.sub_item, dir, window, cx);
        } else {
            self.item = self.step_item(self.item, dir, window, cx);
        }
        cx.notify();
    }

    /// Right: into the highlighted submenu, else on to the next menu.
    fn right(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open
            && self.sub.is_none()
            && let Some(ix) = self.item
            && matches!(self.items().get(ix), Some(OwnedMenuItem::Submenu(_)))
        {
            self.open_sub(ix, true, window, cx);
            return;
        }
        self.move_title(1, window, cx);
    }

    /// Left: out of a submenu, else to the previous menu.
    fn left(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sub.take().is_some() {
            self.sub_item = None;
            cx.notify();
            return;
        }
        self.move_title(-1, window, cx);
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sub.is_some() {
            if let Some(ix) = self.sub_item {
                self.run_sub(ix, window, cx);
            }
            return;
        }
        match (self.active, self.open, self.item) {
            (Some(_), true, Some(ix)) => self.run(ix, window, cx),
            (Some(ix), false, _) => self.open_menu(ix, true, window, cx),
            _ => {}
        }
    }

    /// Esc closes the dropdown but leaves the title highlighted, as Windows
    /// does; a second Esc leaves the bar.
    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sub.take().is_some() {
            self.sub_item = None;
            cx.notify();
        } else if self.open {
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
        let hit = |items: &[OwnedMenuItem]| {
            items.iter().position(|item| match item {
                OwnedMenuItem::Action { name, .. } => mnemonic(name) == Some(ch),
                OwnedMenuItem::Submenu(m) => mnemonic(&m.name) == Some(ch),
                _ => false,
            })
        };
        if self.sub.is_some() {
            if let Some(ix) = hit(self.sub_items()) {
                cx.stop_propagation();
                self.run_sub(ix, window, cx);
            }
        } else if self.open {
            if let Some(ix) = hit(self.items()) {
                cx.stop_propagation();
                self.run(ix, window, cx);
            }
        } else if self.open_mnemonic(ch, window, cx) {
            cx.stop_propagation();
        }
    }

    /// One dropdown's rows. `sub`: these are the open submenu's.
    fn rows(
        &self,
        items: &[OwnedMenuItem],
        sub: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let highlighted_ix = if sub { self.sub_item } else { self.item };
        let checks: Vec<Option<bool>> = items
            .iter()
            .map(|item| match item {
                OwnedMenuItem::Action {
                    action, checked, ..
                } => self
                    .app_state(action.as_ref(), window, cx)
                    .checked
                    .or(checked.then_some(true)),
                _ => None,
            })
            .collect();
        // A gutter for checkmarks, in menus that have any.
        let gutter = checks.iter().any(Option::is_some);
        let check = move |ix: usize| {
            gutter.then(|| {
                div()
                    .w(px(14.0))
                    .flex_shrink_0()
                    .when(checks[ix] == Some(true), |d| d.child("✓"))
            })
        };
        items
            .iter()
            .enumerate()
            .map(|(ix, item)| {
                let enabled = self.item_enabled(item, window, cx);
                let highlighted = highlighted_ix == Some(ix);
                let row = div()
                    .id(("menu-item", ix + if sub { 1000 } else { 0 }))
                    .px_3()
                    .py_1()
                    .flex()
                    .justify_between()
                    .gap_6()
                    .text_sm()
                    .when(highlighted && enabled, |d| d.bg(theme::HOVER))
                    .text_color(if enabled {
                        theme::TEXT_SOFT
                    } else {
                        theme::TEXT_DISABLED
                    });
                let dim = if enabled {
                    theme::TEXT_DIM
                } else {
                    theme::TEXT_DISABLED_DIM
                };
                match item {
                    OwnedMenuItem::Action { name, action, .. } => row
                        .on_mouse_down(MouseButton::Right, {
                            let action = action.boxed_clone();
                            cx.listener(move |bar, e: &gpui::MouseDownEvent, window, cx| {
                                cx.stop_propagation();
                                let Some(f) = bar.secondary.clone() else {
                                    return;
                                };
                                bar.close(window, cx);
                                let action = action.boxed_clone();
                                let at = e.position;
                                window.defer(cx, move |window, cx| {
                                    f(action.as_ref(), at, window, cx)
                                });
                            })
                        })
                        .on_mouse_move(cx.listener(move |bar, _, _, cx| {
                            if sub {
                                if bar.sub_item != Some(ix) {
                                    bar.sub_item = Some(ix);
                                    cx.notify();
                                }
                            } else if bar.item != Some(ix) || bar.sub.is_some() {
                                bar.item = Some(ix);
                                bar.sub = None;
                                bar.sub_item = None;
                                cx.notify();
                            }
                        }))
                        .on_click(cx.listener(move |bar, _, window, cx| {
                            if sub {
                                bar.run_sub(ix, window, cx)
                            } else {
                                bar.run(ix, window, cx)
                            }
                        }))
                        .child(
                            div()
                                .flex()
                                .children(check(ix))
                                .child(label(name, self.keyboard)),
                        )
                        .children(
                            shortcut(action.as_ref(), cx).map(|s| div().text_color(dim).child(s)),
                        )
                        .into_any_element(),
                    OwnedMenuItem::Submenu(m) if !sub => {
                        let open = self.sub == Some(ix);
                        row.relative()
                            .when(open && enabled, |d| d.bg(theme::HOVER))
                            .on_mouse_move(cx.listener(move |bar, _, window, cx| {
                                if bar.sub != Some(ix) {
                                    bar.open_sub(ix, false, window, cx);
                                }
                            }))
                            .on_click(cx.listener(move |bar, _, window, cx| {
                                bar.open_sub(ix, true, window, cx)
                            }))
                            .child(
                                div()
                                    .flex()
                                    .children(check(ix))
                                    .child(label(&m.name, self.keyboard)),
                            )
                            .child(div().text_color(dim).child("›"))
                            .when(open, |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .left_full()
                                        .top(px(-4.0))
                                        .child(self.render_list(&m.items, true, window, cx)),
                                )
                            })
                            .into_any_element()
                    }
                    _ => div()
                        .my_1()
                        .h(px(1.0))
                        .bg(theme::SEPARATOR)
                        .into_any_element(),
                }
            })
            .collect()
    }

    /// A dropdown (or, with `sub`, the open submenu) floating over the window.
    fn render_list(
        &self,
        items: &[OwnedMenuItem],
        sub: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let rows = self.rows(items, sub, window, cx);
        let bar_bounds = self.bounds;
        let this = cx.entity().downgrade();
        deferred(
            anchored().snap_to_window().child(
                div()
                    .id(if sub { "menu-submenu" } else { "menu-dropdown" })
                    .occlude()
                    .when(!sub, |d| {
                        d.on_mouse_down_out(cx.listener(
                            move |bar, e: &gpui::MouseDownEvent, window, cx| {
                                // Clicks on the bar's titles are handled
                                // there, and the submenu's in it.
                                let in_sub =
                                    bar.sub.is_some() && bar.sub_bounds.contains(&e.position);
                                if !bar_bounds.contains(&e.position) && !in_sub {
                                    bar.close(window, cx);
                                }
                            },
                        ))
                    })
                    .relative()
                    .min_w(px(220.0))
                    .py_1()
                    .rounded_md()
                    .bg(theme::POPOVER)
                    .border_1()
                    .border_color(theme::DIVIDER)
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .when(sub, |d| {
                        d.child(
                            canvas(
                                move |bounds, _, cx| {
                                    this.update(cx, |bar, _| bar.sub_bounds = bounds).ok();
                                },
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .size_full(),
                        )
                    })
                    .children(rows),
            ),
        )
        .with_priority(if sub { 2 } else { 1 })
    }

    fn render_dropdown(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_list(self.items(), false, window, cx)
    }
}

impl Render for MenuBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.pick_first) && self.item.is_none() {
            self.item = self.step_item(None, 1, window, cx);
        }
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
                    .when(active, |d| d.bg(theme::HOVER))
                    .hover(|s| s.bg(theme::ROW_FOCUS))
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
            .text_color(theme::TEXT_SOFT)
            .on_action(cx.listener(|bar, _: &MenuLeft, window, cx| bar.left(window, cx)))
            .on_action(cx.listener(|bar, _: &MenuRight, window, cx| bar.right(window, cx)))
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

/// Split a `&`-marked label into display text and the byte offset of its
/// mnemonic character, if any.
/// `&&` is a literal `&`, as in Win32 menus.
pub fn parse_label(label: &str) -> (String, Option<usize>) {
    let mut text = String::with_capacity(label.len());
    let mut at = None;
    let mut chars = label.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '&' {
            match chars.peek() {
                Some('&') => {
                    chars.next();
                    text.push('&');
                }
                Some(_) if at.is_none() => at = Some(text.len()),
                _ => {}
            }
        } else {
            text.push(c);
        }
    }
    (text, at)
}

/// The mnemonic letter of a `&`-marked label, lowercased.
pub fn mnemonic(label: &str) -> Option<char> {
    let (text, at) = parse_label(label);
    text[at?..].chars().next().map(|c| c.to_ascii_lowercase())
}

/// macOS menus don't use mnemonics; strip the markers there.
pub fn platform_label(s: &str) -> String {
    if cfg!(target_os = "macos") {
        parse_label(s).0
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_take_the_first_marker_and_unescape_doubles() {
        assert_eq!(parse_label("&File"), ("File".to_string(), Some(0)));
        assert_eq!(parse_label("Save &As…"), ("Save As…".to_string(), Some(5)));
        assert_eq!(
            parse_label("&1 A&&B.pdf"),
            ("1 A&B.pdf".to_string(), Some(0))
        );
        assert_eq!(parse_label("Q&&A"), ("Q&A".to_string(), None));
        assert_eq!(mnemonic("1&0 x.pdf"), Some('0'));
    }
}
