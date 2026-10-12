//! Top-level window content: document tabs, three docks (left, right,
//! bottom) that each host a tabbed set of panels, and the active document
//! view in the middle. F4 hides and shows all docks at once, as Acrobat's
//! navigation-pane toggle does; individual docks have their own keys.

use std::path::PathBuf;
use std::time::Instant;

use gpui::{
    AnchoredPositionMode, AnyElement, App, Context, Entity, Focusable, KeyDownEvent, MouseButton,
    MouseMoveEvent, PathPromptOptions, Pixels, Point, PromptLevel, SharedString, Window, anchored,
    deferred, div, prelude::*, px, rgb,
};

use crate::docview::DocView;
use crate::frame;
use crate::menu_bar::MenuBar;
use crate::{
    About, ActivateMenuBar, CloseTab, NextTab, OpenFile, PrevTab, Quit, ShowOutline, ShowPatterns,
    ShowThumbnails, ToggleBottomDock, ToggleLeftDock, ToggleRightDock, ToggleSidebar,
};

pub const KEY_CONTEXT: &str = "Workspace";

/// What the command line asked for at startup.
pub struct Launch {
    pub paths: Vec<PathBuf>,
    pub timing: bool,
    /// Open the search bar with this query once the first document loads.
    pub find: Option<String>,
    /// Dev: rotate page 1 of the first document and save it here.
    pub selftest_save: Option<PathBuf>,
    /// Dev: insert this PDF after page 1 of the first document.
    pub selftest_insert: Option<PathBuf>,
    pub selftest_annot: Option<PathBuf>,
    pub start_panel: Option<String>,
    /// Run this regex in the Patterns panel of the first document.
    pub pattern: Option<String>,
    pub launched: Instant,
}

const TAB_H: f32 = 32.0;
const DOCK_TAB_H: f32 = 28.0;
const SPLITTER: f32 = 5.0;
const MIN_DOCK: f32 = 140.0;
const MAX_DOCK_FRACTION: f32 = 0.7;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Panel {
    Thumbnails,
    Outline,
    Patterns,
}

impl Panel {
    fn label(self) -> &'static str {
        match self {
            Panel::Thumbnails => "Pages",
            Panel::Outline => "Bookmarks",
            Panel::Patterns => "Patterns",
        }
    }

    /// Thumbnails have no horizontal layout yet, so they stay on the sides.
    fn can_dock(self, side: Side) -> bool {
        !(self == Panel::Thumbnails && side == Side::Bottom)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Left,
    Right,
    Bottom,
}

impl Side {
    const ALL: [Side; 3] = [Side::Left, Side::Right, Side::Bottom];

    fn label(self) -> &'static str {
        match self {
            Side::Left => "Left",
            Side::Right => "Right",
            Side::Bottom => "Bottom",
        }
    }
}

pub struct Dock {
    panels: Vec<Panel>,
    active: usize,
    open: bool,
    /// Width for left/right, height for bottom. Logical px.
    size: f32,
}

impl Dock {
    fn new(panels: Vec<Panel>, open: bool, size: f32) -> Self {
        Self {
            panels,
            active: 0,
            open,
            size,
        }
    }

    fn active_panel(&self) -> Option<Panel> {
        self.panels.get(self.active).copied()
    }

    fn shows(&self) -> bool {
        self.open && !self.panels.is_empty()
    }
}

pub struct Workspace {
    docs: Vec<Entity<DocView>>,
    active: usize,
    docks: [Dock; 3],
    /// F4: everything hidden regardless of per-dock state.
    all_hidden: bool,
    /// A splitter is being dragged.
    resizing: Option<Side>,
    /// Right-click menu on a dock tab: (panel, where).
    dock_menu: Option<(Panel, Point<Pixels>)>,
    /// File/Edit/View… in the title bar; None where the OS draws menus.
    menu_bar: Option<Entity<MenuBar>>,
    /// Bounds of the dock area from the last frame, for splitter math.
    area: gpui::Bounds<Pixels>,
    timing: bool,
    launched: Instant,
}

impl Workspace {
    pub fn new(window: &mut Window, launch: Launch, cx: &mut Context<Self>) -> Self {
        let Launch {
            paths,
            timing,
            find,
            selftest_save,
            selftest_insert,
            selftest_annot,
            start_panel,
            pattern,
            launched,
        } = launch;
        let mut this = Self {
            docs: Vec::new(),
            active: 0,
            docks: [
                Dock::new(vec![Panel::Thumbnails, Panel::Outline], true, 220.0),
                Dock::new(vec![], false, 280.0),
                Dock::new(vec![Panel::Patterns], false, 240.0),
            ],
            all_hidden: false,
            resizing: None,
            dock_menu: None,
            menu_bar: frame::draws_menu_bar().then(|| cx.new(|cx| MenuBar::new(window, cx))),
            area: gpui::Bounds::default(),
            timing,
            launched,
        };
        match start_panel.as_deref() {
            Some("bookmarks") => this.show_panel(Panel::Outline),
            Some("patterns") => this.show_panel(Panel::Patterns),
            _ => {}
        }
        for p in paths {
            this.open_path(p, cx);
        }
        if let (Some(q), Some(doc)) = (find, this.docs.first().cloned()) {
            doc.update(cx, |d, cx| d.set_initial_query(q, cx));
        }
        if let (Some(p), Some(doc)) = (selftest_save, this.docs.first().cloned()) {
            doc.update(cx, |d, _| d.set_selftest_save(p));
        }
        if let (Some(p), Some(doc)) = (selftest_insert, this.docs.first().cloned()) {
            doc.update(cx, |d, _| d.set_selftest_insert(p));
        }
        if let (Some(p), Some(doc)) = (selftest_annot, this.docs.first().cloned()) {
            doc.update(cx, |d, _| d.set_selftest_annot(p));
        }
        if let (Some(p), Some(doc)) = (pattern, this.docs.first().cloned()) {
            doc.update(cx, |d, _| d.set_initial_pattern(p));
        }
        this
    }

    pub fn active_doc(&self) -> Option<&Entity<DocView>> {
        self.docs.get(self.active)
    }

    fn dock(&self, side: Side) -> &Dock {
        &self.docks[side as usize]
    }

    fn dock_mut(&mut self, side: Side) -> &mut Dock {
        &mut self.docks[side as usize]
    }

    fn side_of(&self, panel: Panel) -> Option<Side> {
        Side::ALL
            .into_iter()
            .find(|s| self.dock(*s).panels.contains(&panel))
    }

    /// Make a panel visible: activate it in its dock, open the dock, and
    /// undo F4 if everything was hidden.
    fn show_panel(&mut self, panel: Panel) {
        let Some(side) = self.side_of(panel) else {
            return;
        };
        let dock = self.dock_mut(side);
        dock.active = dock.panels.iter().position(|p| *p == panel).unwrap_or(0);
        dock.open = true;
        self.all_hidden = false;
    }

    fn move_panel(&mut self, panel: Panel, to: Side) {
        if !panel.can_dock(to) {
            return;
        }
        if let Some(from) = self.side_of(panel) {
            let dock = self.dock_mut(from);
            dock.panels.retain(|p| *p != panel);
            dock.active = dock.active.min(dock.panels.len().saturating_sub(1));
        }
        let dock = self.dock_mut(to);
        dock.panels.push(panel);
        dock.active = dock.panels.len() - 1;
        dock.open = true;
        self.all_hidden = false;
    }

    fn toggle_dock(&mut self, side: Side, cx: &mut Context<Self>) {
        if self.all_hidden {
            // Coming out of F4: show just this dock.
            self.all_hidden = false;
            for s in Side::ALL {
                self.dock_mut(s).open = s == side;
            }
        } else {
            let d = self.dock_mut(side);
            d.open = !d.open;
        }
        cx.notify();
    }

    pub fn open_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let (timing, launched) = (self.timing, self.launched);
        let doc = cx.new(|cx| DocView::new(path, timing, launched, cx));
        self.docs.push(doc);
        self.active = self.docs.len() - 1;
        cx.notify();
    }

    fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.docs.is_empty() {
            return;
        }
        self.active = ix.min(self.docs.len() - 1);
        self.focus_active(window, cx);
        cx.notify();
    }

    pub fn focus_active(&self, window: &mut Window, cx: &mut App) {
        if let Some(doc) = self.active_doc() {
            let handle = doc.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
    }

    /// Open the title-bar menu with this mnemonic letter, if there is one.
    pub fn open_menu(&self, ch: char, window: &mut Window, cx: &mut App) {
        if let Some(bar) = &self.menu_bar {
            bar.update(cx, |bar, cx| bar.open_mnemonic(ch, window, cx));
        }
    }

    fn close(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.docs.len() {
            return;
        }
        self.docs.remove(ix);
        if !self.docs.is_empty() {
            self.activate(self.active.min(self.docs.len() - 1), window, cx);
        }
        cx.notify();
    }

    fn prompt_open(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Open".into()),
        });
        cx.spawn(async move |this, cx| match rx.await {
            Ok(Ok(Some(paths))) => {
                this.update(cx, |ws, cx| {
                    for p in paths {
                        ws.open_path(p, cx);
                    }
                })
                .ok();
            }
            Ok(Ok(None)) => {}
            Ok(Err(e)) => log::error!("open dialog: {e:#}"),
            Err(_) => {}
        })
        .detach();
    }

    // ----- splitters ------------------------------------------------------------

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(side) = self.resizing else { return };
        if ev.pressed_button != Some(MouseButton::Left) {
            self.resizing = None;
            cx.notify();
            return;
        }
        let area = self.area;
        let (max_w, max_h) = (
            f32::from(area.size.width) * MAX_DOCK_FRACTION,
            f32::from(area.size.height) * MAX_DOCK_FRACTION,
        );
        let size = match side {
            Side::Left => {
                f32::from(ev.position.x - area.origin.x).clamp(MIN_DOCK, max_w.max(MIN_DOCK))
            }
            Side::Right => f32::from(area.origin.x + area.size.width - ev.position.x)
                .clamp(MIN_DOCK, max_w.max(MIN_DOCK)),
            Side::Bottom => f32::from(area.origin.y + area.size.height - ev.position.y)
                .clamp(MIN_DOCK, max_h.max(MIN_DOCK)),
        };
        self.dock_mut(side).size = size;
        cx.notify();
    }

    fn splitter(&self, side: Side, cx: &mut Context<Self>) -> AnyElement {
        let vertical = side != Side::Bottom;
        let active = self.resizing == Some(side);
        div()
            .id(("splitter", side as usize))
            .flex_shrink_0()
            .when(vertical, |d| d.w(px(SPLITTER)).h_full().cursor_col_resize())
            .when(!vertical, |d| {
                d.h(px(SPLITTER)).w_full().cursor_row_resize()
            })
            .bg(if active { rgb(0x4a9eff) } else { rgb(0x1a1a1a) })
            .hover(|s| s.bg(rgb(0x4a9eff)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |ws, _, _, cx| {
                    ws.resizing = Some(side);
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    // ----- docks -----------------------------------------------------------------

    fn render_dock(
        &mut self,
        side: Side,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dock = self.dock(side);
        let size = dock.size;
        let panels = dock.panels.clone();
        let active = dock.active_panel();
        let wide = side == Side::Bottom;

        let tabs = panels.iter().map(|&panel| {
            let is_active = active == Some(panel);
            div()
                .id(("dock-tab", panel as usize + 10 * side as usize))
                .px_2()
                .h_full()
                .flex()
                .items_center()
                .text_xs()
                .cursor_pointer()
                .border_b_2()
                .border_color(if is_active {
                    rgb(0x4a9eff)
                } else {
                    rgb(0x252525)
                })
                .text_color(if is_active {
                    rgb(0xffffff)
                } else {
                    rgb(0x9a9a9a)
                })
                .hover(|s| s.bg(rgb(0x333333)))
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.show_panel(panel);
                    cx.notify();
                }))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |ws, ev: &gpui::MouseDownEvent, _, cx| {
                        ws.dock_menu = Some((panel, ev.position));
                        cx.notify();
                    }),
                )
                .child(panel.label())
        });
        let header = div()
            .h(px(DOCK_TAB_H))
            .flex_shrink_0()
            .flex()
            .items_stretch()
            .gap_1()
            .px_1()
            .bg(rgb(0x252525))
            .border_b_1()
            .border_color(rgb(0x111111))
            .children(tabs)
            .child(div().flex_1())
            .child(
                div()
                    .id(("dock-close", side as usize))
                    .px_2()
                    .flex()
                    .items_center()
                    .text_xs()
                    .text_color(rgb(0x8a8a8a))
                    .cursor_pointer()
                    .hover(|s| s.text_color(rgb(0xffffff)))
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.dock_mut(side).open = false;
                        cx.notify();
                    }))
                    .child("×"),
            );

        let body: AnyElement = match (self.active_doc().cloned(), active) {
            (Some(doc), Some(panel)) => doc.update(cx, |d, cx| match panel {
                Panel::Thumbnails => d.render_thumbnails(size, window, cx),
                Panel::Outline => d.render_outline(window, cx),
                Panel::Patterns => d.render_patterns(wide, window, cx),
            }),
            _ => div().into_any_element(),
        };

        let mut el = div().flex_shrink_0().flex().flex_col().bg(rgb(0x252525));
        el = match side {
            Side::Left | Side::Right => el.w(px(size)).h_full(),
            Side::Bottom => el.h(px(size)).w_full(),
        };
        el.child(header)
            .child(div().flex_1().min_h(px(0.0)).min_w(px(0.0)).child(body))
            .into_any_element()
    }

    fn render_dock_menu(
        &self,
        panel: Panel,
        pos: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let current = self.side_of(panel);
        let items = Side::ALL
            .into_iter()
            .filter(|s| Some(*s) != current && panel.can_dock(*s))
            .map(|side| {
                div()
                    .id(("move-to", side as usize))
                    .px_3()
                    .py_1()
                    .text_sm()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(0x3a3a3a)))
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.move_panel(panel, side);
                        ws.dock_menu = None;
                        cx.notify();
                    }))
                    .child(format!("Move to {}", side.label()))
            });
        deferred(
            anchored()
                .position(pos)
                .position_mode(AnchoredPositionMode::Window)
                .child(
                    div()
                        .id("dock-menu")
                        .occlude()
                        .on_mouse_down_out(cx.listener(|ws, _, _, cx| {
                            ws.dock_menu = None;
                            cx.notify();
                        }))
                        .min_w(px(160.0))
                        .py_1()
                        .rounded_md()
                        .bg(rgb(0x2a2a2a))
                        .border_1()
                        .border_color(rgb(0x111111))
                        .shadow_lg()
                        .flex()
                        .flex_col()
                        .children(items),
                ),
        )
        .into_any_element()
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let tabs = self.docs.iter().enumerate().map(|(i, doc)| {
            let active = i == self.active;
            let title = doc.read(cx).title();
            div()
                .id(("tab", i))
                .h_full()
                .px_3()
                .flex()
                .items_center()
                .gap_2()
                .text_sm()
                .cursor_pointer()
                .bg(if active { rgb(0x2b2b2b) } else { rgb(0x1e1e1e) })
                .text_color(if active { rgb(0xffffff) } else { rgb(0x9a9a9a) })
                .hover(|s| s.bg(rgb(0x333333)))
                .on_click(cx.listener(move |ws, _, window, cx| ws.activate(i, window, cx)))
                .child(title)
                .child(
                    div()
                        .id(("tab-close", i))
                        .px_1()
                        .rounded_sm()
                        .text_color(rgb(0x8a8a8a))
                        .hover(|s| s.bg(rgb(0x555555)).text_color(rgb(0xffffff)))
                        .on_click(cx.listener(move |ws, _, window, cx| {
                            ws.close(i, window, cx);
                            cx.stop_propagation();
                        }))
                        .child("×"),
                )
        });
        div()
            .h(px(TAB_H))
            .flex_shrink_0()
            .flex()
            .items_stretch()
            .bg(rgb(0x1e1e1e))
            .border_b_1()
            .border_color(rgb(0x111111))
            .children(tabs)
            .into_any_element()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let main: AnyElement = match self.active_doc().cloned() {
            Some(doc) => doc.into_any_element(),
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(0x8a8a8a))
                .child(SharedString::from(
                    "No document open — Ctrl+O to open a PDF",
                ))
                .into_any_element(),
        };
        let show =
            |ws: &Self, side: Side| !ws.all_hidden && !ws.docs.is_empty() && ws.dock(side).shows();
        let left = show(self, Side::Left).then(|| self.render_dock(Side::Left, window, cx));
        let right = show(self, Side::Right).then(|| self.render_dock(Side::Right, window, cx));
        let bottom = show(self, Side::Bottom).then(|| self.render_dock(Side::Bottom, window, cx));
        let menu = self
            .dock_menu
            .map(|(panel, pos)| self.render_dock_menu(panel, pos, cx));

        // Remember where the dock area is so splitter drags can be resolved
        // against it next frame.
        let this = cx.entity().downgrade();
        let area_tracker = gpui::canvas(
            |bounds, _, _| bounds,
            move |bounds: gpui::Bounds<Pixels>, _, _, cx| {
                this.update(cx, |ws, _| ws.area = bounds).ok();
            },
        )
        .absolute()
        .size_full();

        let middle = div()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .child(div().flex_1().min_h(px(0.0)).child(main))
            .when_some(bottom, |d, b| {
                d.child(self.splitter(Side::Bottom, cx)).child(b)
            });

        let title: SharedString = match self.active_doc() {
            Some(doc) => doc.read(cx).title(),
            None => "".into(),
        };
        let menu_bar = self.menu_bar.clone().map(|m| m.into_any_element());
        let title_bar = frame::title_bar(title, menu_bar, window, cx);

        let content = div()
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(|ws, _: &ToggleSidebar, _, cx| {
                ws.all_hidden = !ws.all_hidden;
                cx.notify();
            }))
            .on_action(cx.listener(|ws, _: &ToggleLeftDock, _, cx| ws.toggle_dock(Side::Left, cx)))
            .on_action(
                cx.listener(|ws, _: &ToggleRightDock, _, cx| ws.toggle_dock(Side::Right, cx)),
            )
            .on_action(
                cx.listener(|ws, _: &ToggleBottomDock, _, cx| ws.toggle_dock(Side::Bottom, cx)),
            )
            .on_action(cx.listener(|ws, _: &ShowThumbnails, _, cx| {
                ws.show_panel(Panel::Thumbnails);
                cx.notify();
            }))
            .on_action(cx.listener(|ws, _: &ShowOutline, _, cx| {
                ws.show_panel(Panel::Outline);
                cx.notify();
            }))
            .on_action(cx.listener(|ws, _: &ShowPatterns, window, cx| {
                ws.show_panel(Panel::Patterns);
                if let Some(doc) = ws.active_doc().cloned() {
                    doc.update(cx, |d, cx| d.focus_pattern_input(window, cx));
                }
                cx.notify();
            }))
            .on_action(cx.listener(|ws, _: &NextTab, window, cx| {
                let n = ws.docs.len();
                if n > 0 {
                    ws.activate((ws.active + 1) % n, window, cx);
                }
            }))
            .on_action(cx.listener(|ws, _: &PrevTab, window, cx| {
                let n = ws.docs.len();
                if n > 0 {
                    ws.activate((ws.active + n - 1) % n, window, cx);
                }
            }))
            .on_action(cx.listener(|ws, _: &CloseTab, window, cx| ws.close(ws.active, window, cx)))
            .on_action(cx.listener(|ws, _: &OpenFile, _, cx| ws.prompt_open(cx)))
            .on_action(|_: &Quit, _, cx| cx.quit())
            .on_action(cx.listener(|ws, _: &ActivateMenuBar, window, cx| {
                if let Some(bar) = &ws.menu_bar {
                    bar.update(cx, |bar, cx| bar.toggle(window, cx));
                }
            }))
            .on_action(|_: &About, window, cx| {
                let detail = format!(
                    "Version {} ({})\n\nA fast, native PDF editor.",
                    env!("CARGO_PKG_VERSION"),
                    env!("SHEAF_GIT_COMMIT")
                );
                drop(window.prompt(PromptLevel::Info, "sheaf", Some(&detail), &["OK"], cx));
            })
            // Alt+letter opens the menu with that mnemonic.
            .on_key_down(cx.listener(|ws, e: &KeyDownEvent, window, cx| {
                let m = &e.keystroke.modifiers;
                if !m.alt || m.control || m.shift || m.platform {
                    return;
                }
                let mut chars = e.keystroke.key.chars();
                let (Some(ch), None) = (chars.next(), chars.next()) else {
                    return;
                };
                if let Some(bar) = &ws.menu_bar
                    && bar.update(cx, |bar, cx| bar.open_mnemonic(ch, window, cx))
                {
                    cx.stop_propagation();
                }
            }))
            // Repaint the bar so mnemonics underline while Alt is held.
            .on_modifiers_changed(cx.listener(|ws, _, _, cx| {
                if let Some(bar) = &ws.menu_bar {
                    bar.update(cx, |_, cx| cx.notify());
                }
            }))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|ws, _, _, cx| {
                    if ws.resizing.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x2b2b2b))
            .text_color(rgb(0xd0d0d0))
            .child(title_bar)
            .child(self.render_tabs(cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .child(area_tracker)
                    .when_some(left, |d, l| d.child(l).child(self.splitter(Side::Left, cx)))
                    .child(middle)
                    .when_some(right, |d, r| {
                        d.child(self.splitter(Side::Right, cx)).child(r)
                    }),
            )
            .children(menu);
        frame::client_side_decorations(content, window, cx)
    }
}
