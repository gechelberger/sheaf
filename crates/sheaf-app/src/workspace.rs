//! Top-level window content: one document per window, with three docks
//! (left, right, bottom) that each host a tabbed set of panels around the
//! document view. F4 hides and shows all docks at once, as Acrobat's
//! navigation-pane toggle does; individual docks have their own keys.

use std::path::{Path, PathBuf};
use std::time::Instant;

use gpui::{
    AnchoredPositionMode, AnyElement, App, Context, Entity, Focusable, KeyDownEvent, MouseButton,
    MouseMoveEvent, PathPromptOptions, Pixels, Point, PromptButton, PromptLevel, SharedString,
    Subscription, Task, Window, WindowHandle, anchored, deferred, div, prelude::*, px,
};
use sheaf_ui::frame;
use sheaf_ui::menu_bar::MenuBar;
use sheaf_ui::palette::{Palette, PaletteEvent};
use sheaf_ui::theme;

use crate::commands::{PanelAction, Run, ShowPluginPanel};
use crate::docview::{DocEvent, DocView};
use crate::ocr_dialog::{OcrDialog, OcrDialogContext, OcrDialogEvent, OcrSpec};
use crate::settings::{DockLayout, DockState, Settings, WindowPlacement};
use crate::{
    About, ActivateMenuBar, CloseDocument, CloseWindow, CommandPalette, NewWindow, OpenFile, Quit,
    RecognizeText, ShowCalc, ShowComments, ShowOutline, ShowPatterns, ShowThumbnails,
    ToggleBottomDock, ToggleLeftDock, ToggleRightDock, ToggleSidebar,
};

pub const KEY_CONTEXT: &str = "Workspace";

/// What the command line asked for at startup.
pub struct Launch {
    /// The window's document; None opens an empty window.
    pub path: Option<PathBuf>,
    pub timing: bool,
    /// Open the search bar with this query once the document loads.
    pub find: Option<String>,
    /// Dev: rotate page 1 of the document and save it here.
    pub selftest_save: Option<PathBuf>,
    /// Dev: insert this PDF after page 1 of the document.
    pub selftest_insert: Option<PathBuf>,
    pub selftest_annot: Option<PathBuf>,
    pub selftest_clean: Option<PathBuf>,
    pub start_panel: Option<String>,
    /// Run this regex in the Patterns panel of the document.
    pub pattern: Option<String>,
    pub launched: Instant,
    pub settings: Settings,
    /// Dev: open the command-line file with this password.
    pub password: Option<String>,
    pub selftest_extract: Option<PathBuf>,
    pub selftest_flatten: Option<PathBuf>,
    pub selftest_ocr: Option<PathBuf>,
}

impl Launch {
    pub fn file(path: Option<PathBuf>) -> Self {
        Self {
            path,
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
            selftest_ocr: None,
        }
    }
}

const DOCK_TAB_H: f32 = 28.0;
const SPLITTER: f32 = 5.0;
const MIN_DOCK: f32 = 140.0;
const MAX_DOCK_FRACTION: f32 = 0.7;

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Panel {
    Thumbnails,
    Outline,
    Patterns,
    Calc,
    Comments,
    Attachments,
    Layers,
    /// A panel a plugin contributes, by its id without the `plugin:` prefix.
    /// Kept in the layout even while its plugin isn't loaded, so disabling
    /// a plugin doesn't forget where its panel was.
    Plugin(SharedString),
}

/// Saved-layout ids of plugin panels start with this.
const PLUGIN_PANEL_PREFIX: &str = "plugin:";

impl Panel {
    const ALL: [Panel; 7] = [
        Panel::Thumbnails,
        Panel::Outline,
        Panel::Patterns,
        Panel::Calc,
        Panel::Comments,
        Panel::Attachments,
        Panel::Layers,
    ];

    /// Stable id for settings and the `--panel` flag.
    fn id(&self) -> SharedString {
        match self {
            Panel::Thumbnails => "pages".into(),
            Panel::Outline => "bookmarks".into(),
            Panel::Patterns => "patterns".into(),
            Panel::Calc => "calc".into(),
            Panel::Comments => "comments".into(),
            Panel::Attachments => "attachments".into(),
            Panel::Layers => "layers".into(),
            Panel::Plugin(id) => format!("{PLUGIN_PANEL_PREFIX}{id}").into(),
        }
    }

    fn from_id(id: &str) -> Option<Panel> {
        if let Some(rest) = id.strip_prefix(PLUGIN_PANEL_PREFIX) {
            return (!rest.is_empty()).then(|| Panel::Plugin(rest.to_string().into()));
        }
        Panel::ALL.into_iter().find(|p| p.id() == id)
    }

    fn label(&self) -> SharedString {
        match self {
            Panel::Thumbnails => "Pages".into(),
            Panel::Outline => "Bookmarks".into(),
            Panel::Patterns => "Patterns".into(),
            Panel::Calc => "Calculator".into(),
            Panel::Comments => "Comments".into(),
            Panel::Attachments => "Attachments".into(),
            Panel::Layers => "Layers".into(),
            // Plugins name their panels once they can contribute them
            // (M6 slice 3); until then, the id.
            Panel::Plugin(id) => id.clone(),
        }
    }

    /// Thumbnails have no horizontal layout yet, so they stay on the sides.
    fn can_dock(&self, side: Side) -> bool {
        !(*self == Panel::Thumbnails && side == Side::Bottom)
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
        self.panels.get(self.active).cloned()
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
/// added where the defaults put it. Plugin panels the file doesn't mention
/// (`plugin_panels`, by id) go in the right dock.
fn docks_from_layout(layout: &DockLayout, plugin_panels: &[String]) -> ([Dock; 3], bool) {
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
                seen.push(p.clone());
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
                seen.push(p.clone());
                docks[i].panels.push(p);
            }
        }
    }
    for id in plugin_panels {
        let p = Panel::Plugin(id.clone().into());
        if !seen.contains(&p) {
            seen.push(p.clone());
            docks[Side::Right as usize].panels.push(p);
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
    doc: Option<Entity<DocView>>,
    docks: [Dock; 3],
    /// F4: everything hidden regardless of per-dock state.
    all_hidden: bool,
    /// A splitter is being dragged.
    resizing: Option<Side>,
    /// Right-click menu on a dock tab: (panel, where).
    dock_menu: Option<(Panel, Point<Pixels>)>,
    /// The command palette, while open.
    palette: Option<(Entity<Palette>, Subscription)>,
    /// Document > Recognize Text (OCR)…, while open.
    ocr_dialog: Option<(Entity<OcrDialog>, Subscription)>,
    /// A plugin's `ui.show-form`, while open.
    #[cfg(feature = "plugins")]
    pub(crate) plugin_form: Option<(Entity<crate::plugin_form::PluginForm>, Subscription)>,
    /// Plugins > Manage Plugins…, while open.
    #[cfg(feature = "plugins")]
    plugin_manager: Option<(Entity<crate::plugin_manager::PluginManager>, Subscription)>,
    /// The document's events, forwarded to plugins. A closed document's
    /// entry stays until its `Closed` event is delivered.
    #[cfg(feature = "plugins")]
    doc_subs: Vec<(gpui::EntityId, Subscription)>,
    /// File/Edit/View… in the title bar; None where the OS draws menus.
    menu_bar: Option<Entity<MenuBar>>,
    /// Bounds of the dock area from the last frame, for splitter math.
    area: gpui::Bounds<Pixels>,
    timing: bool,
    launched: Instant,
    settings: Settings,
    /// Only for the file named on the command line.
    password: Option<String>,
}

impl Workspace {
    pub fn new(window: &mut Window, launch: Launch, cx: &mut Context<Self>) -> Self {
        let Launch {
            path,
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
            selftest_ocr,
        } = launch;
        let plugin_panels: Vec<String> = cx
            .global::<crate::contrib::Contributions>()
            .plugin_panels()
            .iter()
            .map(|p| p.id.clone())
            .collect();
        let (docks, all_hidden) = docks_from_layout(
            settings.docks.as_ref().unwrap_or(&DockLayout::default()),
            &plugin_panels,
        );
        let mut this = Self {
            focus: cx.focus_handle(),
            closing: false,
            native_title: "sheaf".into(),
            doc: None,
            docks,
            all_hidden,
            resizing: None,
            dock_menu: None,
            palette: None,
            ocr_dialog: None,
            #[cfg(feature = "plugins")]
            plugin_form: None,
            #[cfg(feature = "plugins")]
            plugin_manager: None,
            #[cfg(feature = "plugins")]
            doc_subs: Vec::new(),
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
            Some("attachments") => this.show_panel(Panel::Attachments),
            Some("layers") => this.show_panel(Panel::Layers),
            _ => {}
        }
        if let Some(p) = path {
            this.open_path(p, window, cx);
        }
        #[cfg(feature = "plugins")]
        crate::plugins::ask_consent(window, cx);
        this.password = None;
        if let (Some(q), Some(doc)) = (find, this.doc.clone()) {
            doc.update(cx, |d, cx| d.set_initial_query(q, cx));
        }
        if let (Some(p), Some(doc)) = (selftest_save, this.doc.clone()) {
            doc.update(cx, |d, _| d.set_selftest_save(p));
        }
        if let (Some(p), Some(doc)) = (selftest_insert, this.doc.clone()) {
            doc.update(cx, |d, _| d.set_selftest_insert(p));
        }
        if let (Some(p), Some(doc)) = (selftest_annot, this.doc.clone()) {
            doc.update(cx, |d, _| d.set_selftest_annot(p));
        }
        if let (Some(p), Some(doc)) = (selftest_clean, this.doc.clone()) {
            doc.update(cx, |d, _| d.set_selftest_clean(p));
        }
        if let (Some(p), Some(doc)) = (selftest_extract, this.doc.clone()) {
            doc.update(cx, |d, _| d.selftest_extract = Some(p));
        }
        if let (Some(p), Some(doc)) = (selftest_flatten, this.doc.clone()) {
            doc.update(cx, |d, _| d.selftest_flatten = Some(p));
        }
        if let (Some(p), Some(doc)) = (selftest_ocr, this.doc.clone()) {
            doc.update(cx, |d, _| d.selftest_ocr = Some(p));
        }
        if let (Some(p), Some(doc)) = (pattern, this.doc.clone()) {
            doc.update(cx, |d, _| d.set_initial_pattern(p));
        }
        this
    }

    pub fn doc(&self) -> Option<&Entity<DocView>> {
        self.doc.as_ref()
    }

    fn dock(&self, side: Side) -> &Dock {
        &self.docks[side as usize]
    }

    fn dock_mut(&mut self, side: Side) -> &mut Dock {
        &mut self.docks[side as usize]
    }

    fn side_of(&self, panel: &Panel) -> Option<Side> {
        Side::ALL
            .into_iter()
            .find(|s| self.dock(*s).panels.contains(panel))
    }

    /// Make a panel visible: activate it in its dock, open the dock, and
    /// undo F4 if everything was hidden.
    fn show_panel(&mut self, panel: Panel) {
        let Some(side) = self.side_of(&panel) else {
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
        if let Some(from) = self.side_of(&panel) {
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

    /// Show `path` in this window if it's empty; otherwise in a new one.
    pub fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.shows(&path, cx) {
            return;
        }
        if self.doc.is_some() {
            Self::new_window(Some(path), cx);
            return;
        }
        let (timing, launched) = (self.timing, self.launched);
        let password = self.password.clone();
        let doc = cx.new(|cx| DocView::new(path, password, timing, launched, cx));
        self.watch(&doc, window, cx);
        self.doc = Some(doc);
        self.focus_active(window, cx);
        cx.notify();
    }

    /// Whether this window's document is this file.
    fn shows(&self, path: &Path, cx: &App) -> bool {
        self.doc
            .as_ref()
            .is_some_and(|d| same_file(&d.read(cx).path, path))
    }

    pub fn focus_active(&self, window: &mut Window, cx: &mut App) {
        if let Some(doc) = self.doc() {
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

    /// File > Close: close the window, asking about unsaved changes first.
    /// The last window stays open, empty, so another file can be opened.
    fn close_document(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let here = window.window_handle().window_id();
        let others = cx
            .windows()
            .into_iter()
            .filter_map(|w| w.downcast::<Workspace>())
            .any(|w| w.window_id() != here);
        let Some(doc) = self.doc.clone().filter(|_| !others) else {
            self.request_close(window, cx);
            return;
        };
        if !doc.read(cx).is_dirty() {
            self.close_now(window, cx);
            return;
        }
        let may_close = self.confirm_close(doc.clone(), window, cx);
        cx.spawn_in(window, async move |this, cx| {
            if may_close.await {
                this.update_in(cx, |ws, window, cx| {
                    if ws.doc.as_ref() == Some(&doc) {
                        ws.close_now(window, cx);
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    /// Drop the document without asking.
    fn close_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.take() else {
            return;
        };
        doc.update(cx, |_, cx| cx.emit(DocEvent::Closed));
        // Subscriptions hold the view weakly; keep it alive until the event
        // has been delivered.
        cx.defer(move |_| drop(doc));
        self.focus_active(window, cx);
        cx.notify();
    }

    /// Forward a document's events to the plugins that want them.
    #[cfg(feature = "plugins")]
    fn watch(&mut self, doc: &Entity<DocView>, window: &mut Window, cx: &mut Context<Self>) {
        let id = doc.entity_id();
        let sub = cx.subscribe_in(doc, window, move |ws, doc, ev: &DocEvent, window, cx| {
            crate::plugins::dispatch_event(doc, ev, window, cx);
            if *ev == DocEvent::Closed {
                ws.doc_subs.retain(|(d, _)| *d != id);
            }
        });
        self.doc_subs.push((id, sub));
    }

    #[cfg(not(feature = "plugins"))]
    fn watch(&mut self, _: &Entity<DocView>, _: &mut Window, _: &mut Context<Self>) {}

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
        let rx = sheaf_ui::prompts::ask(
            window,
            cx,
            PromptLevel::Warning,
            &format!("Save changes to \"{name}\" before closing?"),
            Some("Your changes will be lost if you don't save them."),
            &[
                PromptButton::ok("Save"),
                PromptButton::new("Don't Save"),
                PromptButton::cancel("Cancel"),
            ],
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

    /// `plugin::Run`, from a plugin's menu item, the palette or a key
    /// binding: runs against the active document.
    #[cfg(feature = "plugins")]
    fn run_plugin_command(&mut self, run: &Run, window: &mut Window, cx: &mut Context<Self>) {
        let call = sheaf_plugin::Call::Command {
            id: run.command.clone(),
            args: run.args.as_ref().map(|a| a.to_string()),
        };
        crate::plugins::run(&run.plugin, call, self.doc().cloned(), window, cx);
    }

    /// A row action in a plugin panel.
    #[cfg(feature = "plugins")]
    fn run_panel_action(&mut self, a: &PanelAction, window: &mut Window, cx: &mut Context<Self>) {
        let Some((plugin, panel)) = a.panel.split_once('.') else {
            return;
        };
        let call = sheaf_plugin::Call::PanelAction {
            panel: panel.to_string(),
            row: a.row.clone(),
            action: a.action.clone(),
        };
        crate::plugins::run(plugin, call, self.doc().cloned(), window, cx);
    }

    /// Pick up changed menus in the title bar.
    #[cfg_attr(not(feature = "plugins"), allow(dead_code))]
    pub fn reload_menus(&mut self, cx: &mut Context<Self>) {
        if let Some(bar) = &self.menu_bar {
            bar.update(cx, |b, cx| b.reload(cx));
        }
        cx.notify();
    }

    /// Plugins > Manage Plugins…: open the manager (or close it).
    #[cfg(feature = "plugins")]
    fn toggle_plugin_manager(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use crate::plugin_manager::{PluginManager, PluginManagerEvent};
        if self.plugin_manager.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
            return;
        }
        let m = cx.new(|cx| PluginManager::new(window, cx));
        let sub = cx.subscribe_in(&m, window, |ws, _, ev: &PluginManagerEvent, window, cx| {
            match ev {
                PluginManagerEvent::Dismiss => {
                    ws.plugin_manager = None;
                    ws.focus_active(window, cx);
                }
            }
            cx.notify();
        });
        self.plugin_manager = Some((m, sub));
        cx.notify();
    }

    #[cfg(not(feature = "plugins"))]
    fn toggle_plugin_manager(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        drop(sheaf_ui::prompts::ask(
            window,
            cx,
            PromptLevel::Info,
            "This build of sheaf has no plugin support",
            None,
            &["OK"],
        ));
    }

    /// Carry out a plugin's `ui` request for a call on `doc`.
    #[cfg(feature = "plugins")]
    pub(crate) fn plugin_ui(
        &mut self,
        plugin: &str,
        doc: Option<Entity<DocView>>,
        req: crate::plugins::UiRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::plugin_form::{PluginForm, PluginFormEvent};
        use crate::plugins::UiRequest;
        match req {
            UiRequest::Notice(msg) => {
                if let Some(d) = doc {
                    d.update(cx, |d, cx| {
                        if msg.trim().is_empty() {
                            d.clear_notice(cx)
                        } else {
                            d.set_notice(msg, cx)
                        }
                    });
                }
            }
            UiRequest::Panel(panel, rows) => {
                let id = format!("{plugin}.{panel}");
                if let Some(d) = doc {
                    d.update(cx, |d, cx| d.set_plugin_panel(id.clone(), rows, cx));
                }
                self.show_panel(Panel::Plugin(id.into()));
                cx.notify();
            }
            UiRequest::ShowPanel(which) => {
                use sheaf_plugin::ui::{BuiltinPanel, ShowPanel};
                self.show_panel(match which {
                    ShowPanel::Plugin(panel) => Panel::Plugin(format!("{plugin}.{panel}").into()),
                    ShowPanel::Builtin(BuiltinPanel::Pages) => Panel::Thumbnails,
                    ShowPanel::Builtin(BuiltinPanel::Bookmarks) => Panel::Outline,
                    ShowPanel::Builtin(BuiltinPanel::Patterns) => Panel::Patterns,
                    ShowPanel::Builtin(BuiltinPanel::Calc) => Panel::Calc,
                    ShowPanel::Builtin(BuiltinPanel::Comments) => Panel::Comments,
                });
                cx.notify();
            }
            UiRequest::Form(form, reply) => {
                if self.plugin_form.is_some() {
                    log::warn!("{plugin}: a form is already open; cancelling the new one");
                    let _ = reply.send(None);
                    return;
                }
                let pages = doc.map(|d| d.read(cx).page_ids()).unwrap_or_default();
                let source = crate::plugins::plugin_name(plugin, cx);
                let form = cx.new(|cx| PluginForm::new(source, form, pages, window, cx));
                let mut reply = Some(reply);
                let sub = cx.subscribe_in(
                    &form,
                    window,
                    move |ws, _, ev: &PluginFormEvent, window, cx| {
                        let answer = match ev {
                            PluginFormEvent::Submit(a) => Some(a.clone()),
                            PluginFormEvent::Dismiss => None,
                        };
                        if let Some(r) = reply.take() {
                            let _ = r.send(answer);
                        }
                        ws.plugin_form = None;
                        ws.focus_active(window, cx);
                        cx.notify();
                    },
                );
                self.plugin_form = Some((form, sub));
                cx.notify();
            }
        }
    }

    #[cfg(not(feature = "plugins"))]
    fn run_panel_action(&mut self, _: &PanelAction, _: &mut Window, _: &mut Context<Self>) {}

    #[cfg(not(feature = "plugins"))]
    fn run_plugin_command(&mut self, run: &Run, _window: &mut Window, cx: &mut Context<Self>) {
        let msg = format!(
            "Can't run {}::{}: this build has no plugin support",
            run.plugin, run.command
        );
        log::warn!("{msg}");
        if let Some(doc) = self.doc().cloned() {
            doc.update(cx, |d, cx| d.set_notice(msg, cx));
        }
    }

    /// Ctrl+Shift+K: open the command palette (or close it if it's open).
    fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
            return;
        }
        let commands = crate::commands::palette_commands(cx);
        let palette = cx.new(|cx| Palette::new(commands, window, cx));
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

    /// Document > Recognize Text (OCR)…: open the options card for the
    /// active document (or close it if it's open).
    fn toggle_ocr_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.ocr_dialog.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
            return;
        }
        let Some(doc) = self.doc().cloned() else {
            return;
        };
        let Some((page_count, current_page, selected)) = doc.read(cx).ocr_context() else {
            return;
        };
        let config = self.settings.ocr.clone();
        let engines = sheaf_ocr::EngineKind::ALL
            .into_iter()
            .map(|k| {
                (
                    k,
                    sheaf_ocr::check_engine(k, &config)
                        .err()
                        .map(|e| e.to_string()),
                )
            })
            .collect();
        let languages = sheaf_ocr::EngineKind::ALL
            .into_iter()
            .map(|k| (k, sheaf_ocr::language_options(k, &config)))
            .collect();
        let ctx = OcrDialogContext {
            page_count,
            current_page,
            selected,
            config,
            engines,
            languages,
        };
        let dialog = cx.new(|cx| OcrDialog::new(ctx, window, cx));
        let sub = cx.subscribe_in(
            &dialog,
            window,
            move |ws, _, ev: &OcrDialogEvent, window, cx| {
                ws.ocr_dialog = None;
                ws.focus_active(window, cx);
                if let OcrDialogEvent::Run(spec) = ev {
                    ws.run_ocr(spec.clone(), cx);
                }
                cx.notify();
            },
        );
        self.ocr_dialog = Some((dialog, sub));
        cx.notify();
    }

    /// Remember the dialog's choices and start the run on the active document.
    fn run_ocr(&mut self, spec: OcrSpec, cx: &mut Context<Self>) {
        let cfg = &mut self.settings.ocr;
        cfg.engine = spec.engine;
        cfg.dpi = spec.dpi;
        cfg.skip_pages_with_text = spec.skip_pages_with_text;
        cfg.last_scope = spec.scope.id().to_string();
        sheaf_ocr::set_language(spec.engine, cfg, &spec.language);
        let config = cfg.clone();
        if let Err(e) = self.settings.save() {
            log::warn!("saving settings: {e:#}");
        }
        if let Some(doc) = self.doc().cloned() {
            doc.update(cx, |d, cx| d.start_ocr(spec, config, cx));
        }
    }

    /// Close this window, asking first if its document has unsaved changes.
    pub fn request_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        self.closing = true;
        self.continue_close(window, cx);
    }

    fn continue_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dirty) = self.doc.clone().filter(|d| d.read(cx).is_dirty()) else {
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
                    if ws.doc.as_ref() == Some(&dirty) {
                        ws.close_now(window, cx);
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

    pub(crate) fn new_window(path: Option<PathBuf>, cx: &mut App) {
        if let Err(e) = crate::open_workspace(Launch::file(path), None, cx) {
            log::error!("opening window: {e:#}");
        }
    }

    fn prompt_open(&mut self, window: &Window, cx: &mut Context<Self>) {
        let rx = crate::file_dialog::open(
            cx,
            PathPromptOptions {
                files: true,
                directories: false,
                multiple: true,
                prompt: Some("Open".into()),
            },
        );
        let Some(origin) = window.window_handle().downcast::<Self>() else {
            return;
        };
        cx.spawn(async move |_, cx| match rx.await {
            Ok(Ok(Some(paths))) => cx.update(|cx| open_paths(origin, paths, cx)),
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
            .bg(if active { theme::ACCENT } else { theme::SUNKEN })
            .hover(|s| s.bg(theme::ACCENT))
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

        let tabs = panels.iter().enumerate().map(|(i, panel)| {
            let is_active = active.as_ref() == Some(panel);
            let (show, menu) = (panel.clone(), panel.clone());
            div()
                .id(("dock-tab", i + 100 * side as usize))
                .px_2()
                .h_full()
                .flex()
                .items_center()
                .text_xs()
                .cursor_pointer()
                .border_b_2()
                .border_color(if is_active {
                    theme::ACCENT
                } else {
                    theme::PANEL
                })
                .text_color(if is_active {
                    theme::TEXT_BRIGHT
                } else {
                    theme::TEXT_MUTED
                })
                .hover(|s| s.bg(theme::HOVER_SUBTLE))
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.show_panel(show.clone());
                    cx.notify();
                }))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |ws, ev: &gpui::MouseDownEvent, _, cx| {
                        ws.dock_menu = Some((menu.clone(), ev.position));
                        cx.notify();
                    }),
                )
                .child(panel_label(panel, cx))
        });
        let header = div()
            .h(px(DOCK_TAB_H))
            .flex_shrink_0()
            .flex()
            .items_stretch()
            .gap_1()
            .px_1()
            .bg(theme::PANEL)
            .border_b_1()
            .border_color(theme::DIVIDER)
            .children(tabs)
            .child(div().flex_1())
            .child(
                div()
                    .id(("dock-close", side as usize))
                    .px_2()
                    .flex()
                    .items_center()
                    .text_xs()
                    .text_color(theme::TEXT_DIM)
                    .cursor_pointer()
                    .hover(|s| s.text_color(theme::TEXT_BRIGHT))
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.dock_mut(side).open = false;
                        cx.notify();
                    }))
                    .child("×"),
            );

        let body: AnyElement = match (self.doc().cloned(), active) {
            (Some(doc), Some(panel)) => doc.update(cx, |d, cx| match panel {
                Panel::Thumbnails => d.render_thumbnails(size, window, cx),
                Panel::Outline => d.render_outline(window, cx),
                Panel::Patterns => d.render_patterns(wide, window, cx),
                Panel::Calc => d.render_calc(wide, window, cx),
                Panel::Comments => d.render_comments(window, cx),
                Panel::Attachments => d.render_attachments(window, cx),
                Panel::Layers => d.render_layers(window, cx),
                Panel::Plugin(id) => plugin_panel_body(d, &id, window, cx),
            }),
            _ => div().into_any_element(),
        };

        let mut el = div().flex_shrink_0().flex().flex_col().bg(theme::PANEL);
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
        let current = self.side_of(&panel);
        let items: Vec<_> = Side::ALL
            .into_iter()
            .filter(|s| Some(*s) != current && panel.can_dock(*s))
            .map(|side| {
                let panel = panel.clone();
                menu_item(
                    ("move-to", side as usize),
                    format!("Move to {}", side.label()),
                    cx,
                    move |ws, _, _| ws.move_panel(panel.clone(), side),
                )
            })
            .collect();
        self.popup_menu("dock-menu", pos, items, cx)
    }

    /// A right-click menu at `pos`; clicking outside it dismisses it.
    fn popup_menu(
        &self,
        id: &'static str,
        pos: Point<Pixels>,
        items: impl IntoIterator<Item = AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        deferred(
            anchored()
                .position(pos)
                .position_mode(AnchoredPositionMode::Window)
                .child(
                    div()
                        .id(id)
                        .occlude()
                        .on_mouse_down_out(cx.listener(|ws, _, _, cx| {
                            ws.dock_menu = None;
                            cx.notify();
                        }))
                        .min_w(px(160.0))
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

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let main: AnyElement = match self.doc().cloned() {
            Some(doc) => doc.into_any_element(),
            None => div()
                .size_full()
                .flex()
                .flex_col()
                .gap_4()
                .items_center()
                .justify_center()
                .text_color(theme::TEXT_DIM)
                .child(crate::brand::logo(px(96.0)))
                .child(SharedString::from(
                    "No document open — Ctrl+O to open a PDF",
                ))
                .into_any_element(),
        };
        // Read Mode and Full Screen put the docks (and in full screen, the
        // title bar) away.
        let presentation = self
            .doc()
            .map(|d| d.read(cx).presentation())
            .unwrap_or_default();
        let bare = presentation != crate::presentation::Presentation::Normal;
        let full = presentation == crate::presentation::Presentation::FullScreen;
        if let Some(doc) = self.doc().cloned()
            && let Some(id) = doc.update(cx, |d, _| d.take_wanted_panel())
            && let Some(panel) = Panel::from_id(id)
        {
            self.show_panel(panel);
        }
        let show = |ws: &Self, side: Side| {
            !bare && !ws.all_hidden && ws.doc.is_some() && ws.dock(side).shows()
        };
        let left = show(self, Side::Left).then(|| self.render_dock(Side::Left, window, cx));
        let right = show(self, Side::Right).then(|| self.render_dock(Side::Right, window, cx));
        let bottom = show(self, Side::Bottom).then(|| self.render_dock(Side::Bottom, window, cx));
        let menu = self
            .dock_menu
            .clone()
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

        let title: SharedString = match self.doc() {
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
        let on_close =
            cx.listener(|ws, _: &gpui::ClickEvent, window, cx| ws.request_close(window, cx));
        let icon = crate::brand::mark(px(16.0), window).into_any_element();
        let title_bar = frame::title_bar(icon, title, menu_bar, on_close, window, cx);

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
                if let Some(doc) = ws.doc().cloned() {
                    doc.update(cx, |d, cx| d.focus_pattern_input(window, cx));
                }
                cx.notify();
            }))
            .on_action(cx.listener(|ws, _: &ShowComments, _, cx| {
                ws.show_panel(Panel::Comments);
                if let Some(doc) = ws.doc().cloned() {
                    doc.update(cx, |d, cx| d.focus_comments(cx));
                }
                cx.notify();
            }))
            .on_action(cx.listener(|ws, _: &crate::ShowLayers, _, cx| {
                ws.show_panel(Panel::Layers);
                cx.notify();
            }))
            .on_action(cx.listener(|ws, _: &crate::ShowAttachments, _, cx| {
                ws.show_panel(Panel::Attachments);
                cx.notify();
            }))
            .on_action(cx.listener(|ws, _: &ShowCalc, window, cx| {
                ws.show_panel(Panel::Calc);
                if let Some(doc) = ws.doc().cloned() {
                    doc.update(cx, |d, cx| d.focus_calc_input(window, cx));
                }
                cx.notify();
            }))
            .on_action(
                cx.listener(|ws, _: &CloseDocument, window, cx| ws.close_document(window, cx)),
            )
            .on_action(cx.listener(|ws, _: &OpenFile, window, cx| ws.prompt_open(window, cx)))
            .on_action(|_: &NewWindow, _, cx| Self::new_window(None, cx))
            .on_action(cx.listener(|ws, _: &CloseWindow, window, cx| ws.request_close(window, cx)))
            .on_action(|_: &Quit, _, cx| cx.defer(crate::request_quit))
            .on_action(
                cx.listener(|ws, run: &Run, window, cx| ws.run_plugin_command(run, window, cx)),
            )
            .on_action(cx.listener(|ws, a: &ShowPluginPanel, _, cx| {
                ws.show_panel(Panel::Plugin(a.panel.clone().into()));
                cx.notify();
            }))
            .on_action(
                cx.listener(|ws, a: &PanelAction, window, cx| ws.run_panel_action(a, window, cx)),
            )
            .on_action(cx.listener(|ws, _: &crate::ManagePlugins, window, cx| {
                ws.toggle_plugin_manager(window, cx)
            }))
            .on_action(
                cx.listener(|ws, _: &CommandPalette, window, cx| ws.toggle_palette(window, cx)),
            )
            .on_action(
                cx.listener(|ws, _: &RecognizeText, window, cx| ws.toggle_ocr_dialog(window, cx)),
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
                drop(sheaf_ui::prompts::ask(
                    window,
                    cx,
                    PromptLevel::Info,
                    "sheaf",
                    Some(&detail),
                    &["OK"],
                ));
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
            .bg(theme::RAISED)
            .text_color(theme::TEXT_SOFT)
            .when(!full, |d| d.child(title_bar))
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
            )
            .children(
                self.ocr_dialog
                    .as_ref()
                    .map(|(d, _)| deferred(d.clone()).with_priority(2)),
            );
        #[cfg(feature = "plugins")]
        let content = content
            .children(
                self.plugin_manager
                    .as_ref()
                    .map(|(m, _)| deferred(m.clone()).with_priority(2)),
            )
            .children(
                self.plugin_form
                    .as_ref()
                    .map(|(f, _)| deferred(f.clone()).with_priority(3)),
            );
        frame::client_side_decorations(content, window, cx)
    }
}

/// Open files picked in `origin`'s dialog: the first fills `origin` if it's
/// empty, the rest get windows of their own. A file that is already open
/// just brings its window forward: two views of one file would share a
/// recovery sidecar and each save would discard the other's edits.
fn open_paths(origin: WindowHandle<Workspace>, paths: Vec<PathBuf>, cx: &mut App) {
    for p in paths {
        if let Some(handle) = find_open(&p, cx) {
            handle
                .update(cx, |_, window, _| window.activate_window())
                .ok();
        } else {
            origin
                .update(cx, |ws, window, cx| ws.open_path(p, window, cx))
                .ok();
        }
    }
}

/// The window showing this file, if any.
fn find_open(path: &Path, cx: &App) -> Option<WindowHandle<Workspace>> {
    cx.windows()
        .into_iter()
        .filter_map(|w| w.downcast::<Workspace>())
        .find(|handle| handle.read(cx).is_ok_and(|ws| ws.shows(path, cx)))
}

/// One row of a right-click menu; clicking it closes the menu.
fn menu_item(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    cx: &mut Context<Workspace>,
    on_click: impl Fn(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .px_3()
        .py_1()
        .text_sm()
        .cursor_pointer()
        .hover(|s| s.bg(theme::HOVER))
        .on_click(cx.listener(move |ws, _, window, cx| {
            ws.dock_menu = None;
            on_click(ws, window, cx);
            cx.notify();
        }))
        .child(label.into())
        .into_any_element()
}

pub fn same_file(a: &Path, b: &Path) -> bool {
    a == b || matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
}

/// A dock tab's label: plugin panels are named in their manifests.
fn panel_label(panel: &Panel, cx: &App) -> SharedString {
    match panel {
        Panel::Plugin(id) => cx
            .global::<crate::contrib::Contributions>()
            .panel(id)
            .map(|p| SharedString::from(p.label.clone()))
            .unwrap_or_else(|| id.clone()),
        p => p.label(),
    }
}

/// Body of a plugin panel: the document's rows, or a note when the plugin
/// that provides it isn't loaded.
#[cfg_attr(not(feature = "plugins"), allow(unused_variables))]
fn plugin_panel_body(
    doc: &mut DocView,
    id: &str,
    window: &mut Window,
    cx: &mut Context<DocView>,
) -> AnyElement {
    #[cfg(feature = "plugins")]
    if cx
        .global::<crate::contrib::Contributions>()
        .panel(id)
        .is_some()
    {
        return doc.render_plugin_panel(id, window, cx);
    }
    div()
        .p_3()
        .text_sm()
        .text_color(theme::TEXT_DIM)
        .child(format!("The plugin that provides “{id}” isn't loaded."))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_panels_survive_a_layout_round_trip() {
        let mut layout = DockLayout::default();
        layout.right.panels = vec![
            "comments".into(),
            "plugin:amount-crossref.pairs".into(),
            "plugin:".into(),
            "nonsense".into(),
        ];
        layout.right.active = 1;
        // A plugin panel the layout doesn't mention yet joins the right dock;
        // one it does mention stays where it was.
        let (docks, _) = docks_from_layout(
            &layout,
            &["wp.ticks".to_string(), "amount-crossref.pairs".to_string()],
        );
        let right = &docks[Side::Right as usize];
        assert_eq!(
            right.panels,
            [
                Panel::Comments,
                Panel::Plugin("amount-crossref.pairs".into()),
                Panel::Plugin("wp.ticks".into()),
            ]
        );
        assert_eq!(
            right.active_panel(),
            Some(Panel::Plugin("amount-crossref.pairs".into()))
        );
        assert_eq!(right.to_state().panels[1], "plugin:amount-crossref.pairs");
        // Built-ins the file didn't mention still come back.
        assert!(
            docks[Side::Left as usize]
                .panels
                .contains(&Panel::Thumbnails)
        );
    }
}
