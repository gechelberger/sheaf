//! Top-level window content: document tabs, three docks (left, right,
//! bottom) that each host a tabbed set of panels, and the active document
//! view in the middle. F4 hides and shows all docks at once, as Acrobat's
//! navigation-pane toggle does; individual docks have their own keys.

use std::path::PathBuf;
use std::time::Instant;

use gpui::{
    AnchoredPositionMode, AnyElement, App, Context, Entity, Focusable, KeyDownEvent, MouseButton,
    MouseMoveEvent, PathPromptOptions, Pixels, Point, PromptButton, PromptLevel, SharedString,
    Subscription, Task, Window, anchored, deferred, div, prelude::*, px, rgb,
};

use crate::docview::DocView;
use crate::frame;
use crate::menu_bar::MenuBar;
use crate::palette::{Palette, PaletteEvent};
use crate::settings::{DockLayout, DockState, Settings, WindowPlacement};
use crate::{
    About, ActivateMenuBar, CloseTab, CloseWindow, CommandPalette, NewWindow, NextTab, OpenFile,
    OpenInWindows, PrevTab, Quit, ShowCalc, ShowComments, ShowOutline, ShowPatterns,
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
    pub selftest_clean: Option<PathBuf>,
    pub start_panel: Option<String>,
    /// Run this regex in the Patterns panel of the first document.
    pub pattern: Option<String>,
    pub launched: Instant,
    pub settings: Settings,
    /// Dev: open the command-line files with this password.
    pub password: Option<String>,
    pub selftest_extract: Option<PathBuf>,
    pub selftest_flatten: Option<PathBuf>,
}

impl Launch {
    pub fn files(paths: Vec<PathBuf>) -> Self {
        Self {
            paths,
            timing: false,
            find: None,
            selftest_save: None,
            selftest_insert: None,
            selftest_annot: None,
            selftest_clean: None,
            start_panel: None,
            pattern: None,
            launched: Instant::now(),
            settings: Settings::load(),
            password: None,
            selftest_extract: None,
            selftest_flatten: None,
        }
    }
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
    Calc,
    Comments,
}

impl Panel {
    const ALL: [Panel; 5] = [
        Panel::Thumbnails,
        Panel::Outline,
        Panel::Patterns,
        Panel::Calc,
        Panel::Comments,
    ];

    /// Stable id for settings and the `--panel` flag.
    fn id(self) -> &'static str {
        match self {
            Panel::Thumbnails => "pages",
            Panel::Outline => "bookmarks",
            Panel::Patterns => "patterns",
            Panel::Calc => "calc",
            Panel::Comments => "comments",
        }
    }

    fn from_id(id: &str) -> Option<Panel> {
        Panel::ALL.into_iter().find(|p| p.id() == id)
    }

    fn label(self) -> &'static str {
        match self {
            Panel::Thumbnails => "Pages",
            Panel::Outline => "Bookmarks",
            Panel::Patterns => "Patterns",
            Panel::Calc => "Calculator",
            Panel::Comments => "Comments",
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
    fn active_panel(&self) -> Option<Panel> {
        self.panels.get(self.active).copied()
    }

    fn shows(&self) -> bool {
        self.open && !self.panels.is_empty()
    }

    fn to_state(&self) -> DockState {
        DockState {
            panels: self.panels.iter().map(|p| p.id().to_string()).collect(),
            active: self.active,
            open: self.open,
            size: self.size,
        }
    }
}

/// Docks from a saved layout, made consistent: unknown panel ids are
/// dropped, each panel appears once, panels that can't sit on a side go
/// back to their default dock, and any panel the file doesn't mention is
/// added where the defaults put it.
fn docks_from_layout(layout: &DockLayout) -> ([Dock; 3], bool) {
    let defaults = DockLayout::default();
    let mut docks: Vec<Dock> = Vec::new();
    let mut seen: Vec<Panel> = Vec::new();
    for (side, st) in [
        (Side::Left, &layout.left),
        (Side::Right, &layout.right),
        (Side::Bottom, &layout.bottom),
    ] {
        let mut panels: Vec<Panel> = Vec::new();
        for p in st.panels.iter().filter_map(|id| Panel::from_id(id)) {
            if p.can_dock(side) && !seen.contains(&p) {
                seen.push(p);
                panels.push(p);
            }
        }
        docks.push(Dock {
            panels,
            active: st.active,
            open: st.open,
            size: st.size.clamp(MIN_DOCK, 2000.0),
        });
    }
    for (i, st) in [&defaults.left, &defaults.right, &defaults.bottom]
        .into_iter()
        .enumerate()
    {
        for id in &st.panels {
            if let Some(p) = Panel::from_id(id)
                && !seen.contains(&p)
            {
                seen.push(p);
                docks[i].panels.push(p);
            }
        }
    }
    for d in &mut docks {
        d.active = d.active.min(d.panels.len().saturating_sub(1));
    }
    let docks: [Dock; 3] = docks.try_into().ok().expect("three docks");
    (docks, layout.all_hidden)
}

/// A saved window placement, if it still lands on a connected display.
pub fn restore_window_bounds(w: &WindowPlacement, cx: &App) -> Option<gpui::WindowBounds> {
    if !(w.w >= 400.0 && w.h >= 300.0 && w.w.is_finite() && w.h.is_finite()) {
        return None;
    }
    let origin = gpui::point(px(w.x), px(w.y));
    let on_screen = cx.displays().iter().any(|d| {
        d.bounds()
            .contains(&gpui::point(px(w.x + 50.0), px(w.y + 20.0)))
    });
    if !on_screen {
        return None;
    }
    let bounds = gpui::Bounds::new(origin, gpui::size(px(w.w), px(w.h)));
    Some(if w.maximized {
        gpui::WindowBounds::Maximized(bounds)
    } else {
        gpui::WindowBounds::Windowed(bounds)
    })
}

pub struct Workspace {
    focus: gpui::FocusHandle,
    closing: bool,
    native_title: String,
    docs: Vec<Entity<DocView>>,
    active: usize,
    docks: [Dock; 3],
    /// F4: everything hidden regardless of per-dock state.
    all_hidden: bool,
    /// A splitter is being dragged.
    resizing: Option<Side>,
    /// Right-click menu on a dock tab: (panel, where).
    dock_menu: Option<(Panel, Point<Pixels>)>,
    /// The command palette, while open.
    palette: Option<(Entity<Palette>, Subscription)>,
    /// File/Edit/View… in the title bar; None where the OS draws menus.
    menu_bar: Option<Entity<MenuBar>>,
    /// Bounds of the dock area from the last frame, for splitter math.
    area: gpui::Bounds<Pixels>,
    timing: bool,
    launched: Instant,
    settings: Settings,
    /// Only for files named on the command line.
    password: Option<String>,
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
            selftest_clean,
            start_panel,
            pattern,
            launched,
            settings,
            password,
            selftest_extract,
            selftest_flatten,
        } = launch;
        let (docks, all_hidden) =
            docks_from_layout(settings.docks.as_ref().unwrap_or(&DockLayout::default()));
        let mut this = Self {
            focus: cx.focus_handle(),
            closing: false,
            native_title: "sheaf".into(),
            docs: Vec::new(),
            active: 0,
            docks,
            all_hidden,
            resizing: None,
            dock_menu: None,
            palette: None,
            menu_bar: frame::draws_menu_bar().then(|| cx.new(|cx| MenuBar::new(window, cx))),
            area: gpui::Bounds::default(),
            timing,
            launched,
            settings,
            password,
        };
        match start_panel.as_deref() {
            Some("bookmarks") => this.show_panel(Panel::Outline),
            Some("patterns") => this.show_panel(Panel::Patterns),
            Some("calc") => this.show_panel(Panel::Calc),
            Some("comments") => this.show_panel(Panel::Comments),
            _ => {}
        }
        for p in paths {
            this.open_path(p, cx);
        }
        this.password = None;
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
        if let (Some(p), Some(doc)) = (selftest_clean, this.docs.first().cloned()) {
            doc.update(cx, |d, _| d.set_selftest_clean(p));
        }
        if let (Some(p), Some(doc)) = (selftest_extract, this.docs.first().cloned()) {
            doc.update(cx, |d, _| d.selftest_extract = Some(p));
        }
        if let (Some(p), Some(doc)) = (selftest_flatten, this.docs.first().cloned()) {
            doc.update(cx, |d, _| d.selftest_flatten = Some(p));
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
        self.persist_layout();
    }

    /// Write the dock layout to the settings file.
    fn persist_layout(&mut self) {
        self.settings.docks = Some(DockLayout {
            left: self.dock(Side::Left).to_state(),
            right: self.dock(Side::Right).to_state(),
            bottom: self.dock(Side::Bottom).to_state(),
            all_hidden: self.all_hidden,
        });
        if let Err(e) = self.settings.save() {
            log::warn!("saving settings: {e:#}");
        }
    }

    /// Remember where the window is, then write everything.
    fn persist_window(&mut self, window: &Window) {
        let (bounds, maximized) = match window.window_bounds() {
            gpui::WindowBounds::Windowed(b) => (b, false),
            gpui::WindowBounds::Maximized(b) => (b, true),
            gpui::WindowBounds::Fullscreen(b) => (b, false),
        };
        self.settings.window = Some(WindowPlacement {
            x: f32::from(bounds.origin.x),
            y: f32::from(bounds.origin.y),
            w: f32::from(bounds.size.width),
            h: f32::from(bounds.size.height),
            maximized,
        });
        self.persist_layout();
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
        self.persist_layout();
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
        self.persist_layout();
        cx.notify();
    }

    pub fn open_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let (timing, launched) = (self.timing, self.launched);
        let password = self.password.clone();
        let doc = cx.new(|cx| DocView::new(path, password, timing, launched, cx));
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
        } else {
            window.focus(&self.focus, cx);
        }
    }

    /// Open the title-bar menu with this mnemonic letter, if there is one.
    pub fn open_menu(&self, ch: char, window: &mut Window, cx: &mut App) {
        if let Some(bar) = &self.menu_bar {
            bar.update(cx, |bar, cx| bar.open_mnemonic(ch, window, cx));
        }
    }

    /// Close a tab, asking about unsaved changes first.
    fn close(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.docs.get(ix).cloned() else {
            return;
        };
        if !doc.read(cx).is_dirty() {
            self.close_now(ix, window, cx);
            return;
        }
        let may_close = self.confirm_close(doc.clone(), window, cx);
        cx.spawn_in(window, async move |this, cx| {
            if may_close.await {
                this.update_in(cx, |ws, window, cx| {
                    if let Some(i) = ws.docs.iter().position(|d| *d == doc) {
                        ws.close_now(i, window, cx);
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    fn close_now(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.docs.len() {
            return;
        }
        self.docs.remove(ix);
        if ix < self.active {
            self.active -= 1;
        }
        self.active = self.active.min(self.docs.len().saturating_sub(1));
        self.focus_active(window, cx);
        cx.notify();
    }

    /// Acrobat's "Save changes?" prompt for one document. Resolves to true
    /// once the document may be closed: either it was saved, or the user
    /// chose to discard the changes.
    fn confirm_close(
        &mut self,
        doc: Entity<DocView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        let name = doc.read(cx).file_name();
        let rx = window.prompt(
            PromptLevel::Warning,
            &format!("Save changes to \"{name}\" before closing?"),
            Some("Your changes will be lost if you don't save them."),
            &[
                PromptButton::ok("Save"),
                PromptButton::new("Don't Save"),
                PromptButton::cancel("Cancel"),
            ],
            cx,
        );
        cx.spawn(async move |_, cx| match rx.await {
            Ok(0) => cx.update(|cx| doc.update(cx, |d, cx| d.save(cx))).await,
            Ok(1) => {
                cx.update(|cx| doc.read(cx).discard_recovery());
                true
            }
            _ => false,
        })
    }

    /// Ctrl+Shift+K: open the command palette (or close it if it's open).
    fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
            return;
        }
        let palette = cx.new(|cx| Palette::new(window, cx));
        let sub = cx.subscribe_in(&palette, window, |ws, _, ev: &PaletteEvent, window, cx| {
            ws.palette = None;
            ws.focus_active(window, cx);
            if let PaletteEvent::Run(action) = ev {
                window.dispatch_action(action.boxed_clone(), cx);
            }
            cx.notify();
        });
        self.palette = Some((palette, sub));
        cx.notify();
    }

    /// Close only this window, checking each edited document in turn.
    pub fn request_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        self.closing = true;
        self.continue_close(window, cx);
    }

    fn continue_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dirty) = self.docs.iter().find(|d| d.read(cx).is_dirty()).cloned() else {
            self.persist_window(window);
            window.remove_window();
            if cx.try_global::<crate::Quitting>().is_some_and(|q| q.0) {
                cx.defer(crate::close_next_window);
            }
            return;
        };
        window.activate_window();
        let may_close = self.confirm_close(dirty.clone(), window, cx);
        cx.spawn_in(window, async move |this, cx| {
            let may_close = may_close.await;
            this.update_in(cx, |ws, window, cx| {
                if may_close {
                    if let Some(i) = ws.docs.iter().position(|d| *d == dirty) {
                        ws.close_now(i, window, cx);
                    }
                    ws.continue_close(window, cx);
                } else {
                    ws.closing = false;
                    cx.set_global(crate::Quitting(false));
                }
            })
            .ok();
        })
        .detach();
    }

    /// Route native close through GPUI cleanup on every platform, including
    /// Windows where native destruction would otherwise precede cleanup.
    pub fn window_should_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.request_close(window, cx);
        false
    }

    fn new_window(paths: Vec<PathBuf>, cx: &mut App) {
        if let Err(e) = crate::open_workspace(Launch::files(paths), None, cx) {
            log::error!("opening window: {e:#}");
        }
    }

    fn prompt_open(&mut self, separate_windows: bool, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Open".into()),
        });
        cx.spawn_in(window, async move |this, cx| match rx.await {
            Ok(Ok(Some(paths))) => {
                this.update_in(cx, |ws, window, cx| {
                    for p in paths {
                        if separate_windows {
                            Self::new_window(vec![p], cx);
                        } else {
                            ws.open_path(p, cx);
                        }
                    }
                    if !separate_windows {
                        ws.focus_active(window, cx);
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
            self.persist_layout();
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
                Panel::Calc => d.render_calc(wide, window, cx),
                Panel::Comments => d.render_comments(window, cx),
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
                .flex_shrink_0()
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
            .id("document-tabs")
            .overflow_x_scroll()
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
                .flex_col()
                .gap_4()
                .items_center()
                .justify_center()
                .text_color(rgb(0x8a8a8a))
                .child(crate::brand::logo(px(96.0)))
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
        let native_title = if title.is_empty() {
            "sheaf".to_string()
        } else {
            format!("{title} — sheaf")
        };
        if self.native_title != native_title {
            window.set_window_title(&native_title);
            self.native_title = native_title;
        }
        let menu_bar = self.menu_bar.clone().map(|m| m.into_any_element());
        let title_bar = frame::title_bar(title, menu_bar, window, cx);

        let content = div()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|ws, _: &ToggleSidebar, _, cx| {
                ws.all_hidden = !ws.all_hidden;
                ws.persist_layout();
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
            .on_action(cx.listener(|ws, _: &ShowComments, _, cx| {
                ws.show_panel(Panel::Comments);
                if let Some(doc) = ws.active_doc().cloned() {
                    doc.update(cx, |d, cx| d.focus_comments(cx));
                }
                cx.notify();
            }))
            .on_action(cx.listener(|ws, _: &ShowCalc, window, cx| {
                ws.show_panel(Panel::Calc);
                if let Some(doc) = ws.active_doc().cloned() {
                    doc.update(cx, |d, cx| d.focus_calc_input(window, cx));
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
            .on_action(
                cx.listener(|ws, _: &OpenFile, window, cx| ws.prompt_open(false, window, cx)),
            )
            .on_action(
                cx.listener(|ws, _: &OpenInWindows, window, cx| ws.prompt_open(true, window, cx)),
            )
            .on_action(|_: &NewWindow, _, cx| Self::new_window(Vec::new(), cx))
            .on_action(cx.listener(|ws, _: &CloseWindow, window, cx| ws.request_close(window, cx)))
            .on_action(|_: &Quit, _, cx| cx.defer(crate::request_quit))
            .on_action(
                cx.listener(|ws, _: &CommandPalette, window, cx| ws.toggle_palette(window, cx)),
            )
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
                        ws.persist_layout();
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
            .children(menu)
            .children(
                self.palette
                    .as_ref()
                    .map(|(p, _)| deferred(p.clone()).with_priority(2)),
            );
        frame::client_side_decorations(content, window, cx)
    }
}
