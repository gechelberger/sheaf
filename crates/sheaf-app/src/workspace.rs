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
use crate::state::{DockLayout, DockState, WindowPlacement};
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
    History,
    Stamps,
    /// A panel a plugin contributes, by its id without the `plugin:` prefix.
    /// Kept in the layout even while its plugin isn't loaded, so disabling
    /// a plugin doesn't forget where its panel was.
    Plugin(SharedString),
}

/// Saved-layout ids of plugin panels start with this.
const PLUGIN_PANEL_PREFIX: &str = "plugin:";

impl Panel {
    const ALL: [Panel; 9] = [
        Panel::Thumbnails,
        Panel::Outline,
        Panel::Patterns,
        Panel::Calc,
        Panel::Comments,
        Panel::Attachments,
        Panel::Layers,
        Panel::History,
        Panel::Stamps,
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
            Panel::History => "history".into(),
            Panel::Stamps => "stamps".into(),
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
            Panel::History => "History".into(),
            Panel::Stamps => "Stamps".into(),
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
    pub(crate) const ALL: [Side; 3] = [Side::Left, Side::Right, Side::Bottom];

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

/// Where a window's toolbar sits. Starts from `toolbar.json`; moving the
/// bar changes this window's and writes the file (for new windows).
#[derive(Debug, Clone, Copy, PartialEq)]
struct ToolbarPlace {
    dock: crate::toolbar::Dock,
    /// A floating bar's top-left in the document area.
    position: [f32; 2],
    /// A floating bar's orientation.
    orientation: sheaf_ui::toolbar::Orientation,
}

impl ToolbarPlace {
    fn from_spec(spec: &crate::toolbar::Spec) -> Self {
        Self {
            dock: spec.dock,
            position: spec.position.unwrap_or([24.0, 24.0]),
            orientation: spec.orientation,
        }
    }
}

/// What a toolbar right-click menu is about.
#[derive(Debug, Clone, PartialEq)]
enum ToolbarMenu {
    /// The bar, and the layout index of the item clicked, if any.
    Bar(Option<usize>),
    /// A command right-clicked in a menu or the palette.
    Command(String),
}

/// The toolbar being dragged by its grip.
#[derive(Debug, Clone, Copy)]
struct ToolbarDrag {
    /// Where in the bar it was grabbed.
    offset: Point<Pixels>,
    /// The pointer, in the window.
    at: Point<Pixels>,
}

/// How close to an edge of the document area (px) a dragged toolbar
/// docks there.
const TOOLBAR_SNAP: f32 = 48.0;
/// A floating toolbar keeps this far inside the document area.
const TOOLBAR_MARGIN: f32 = 4.0;

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
    /// File > Export Pages, while open.
    export: Option<(Entity<crate::export_dialog::ExportDialog>, Subscription)>,
    scan: Option<(Entity<crate::scan_dialog::ScanDialog>, Subscription)>,
    combine: Option<(Entity<crate::combine_dialog::CombineDialog>, Subscription)>,
    split: Option<(Entity<crate::split_dialog::SplitDialog>, Subscription)>,
    insert_dialog: Option<(Entity<crate::insert_dialog::InsertDialog>, Subscription)>,
    replace_dialog: Option<(Entity<crate::insert_dialog::ReplaceDialog>, Subscription)>,
    crop: Option<(Entity<crate::crop_dialog::CropDialog>, Subscription)>,
    labels: Option<(Entity<crate::labels_dialog::LabelsDialog>, Subscription)>,
    header_footer: Option<(
        Entity<crate::header_footer_dialog::HeaderFooterDialog>,
        Subscription,
    )>,
    watermark: Option<(
        Entity<crate::watermark_dialog::WatermarkDialog>,
        Subscription,
    )>,
    bookmark_tool: Option<(
        Entity<crate::bookmark_tools::BookmarkToolsDialog>,
        Subscription,
    )>,
    set_status: Option<(Entity<crate::set_status::SetStatusDialog>, Subscription)>,
    blanks: Option<(Entity<crate::blanks_dialog::BlanksDialog>, Subscription)>,
    /// File > Print, while open.
    print: Option<(Entity<crate::print_dialog::PrintDialog>, Subscription)>,
    /// File > Properties, while open.
    properties: Option<(
        Entity<crate::properties_dialog::PropertiesDialog>,
        Subscription,
    )>,
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
    /// The configurable toolbar, where it sits in this window, and the
    /// drag moving it.
    toolbar: Entity<sheaf_ui::toolbar::Toolbar>,
    toolbar_place: ToolbarPlace,
    toolbar_drag: Option<ToolbarDrag>,
    /// Bounds of the document area (toolbar included) last frame.
    doc_area: gpui::Bounds<Pixels>,
    /// Per toolbar item drawn last frame, its index in the layout.
    toolbar_from: Vec<usize>,
    /// A right-click menu about the toolbar or a command.
    toolbar_menu: Option<(ToolbarMenu, Point<Pixels>)>,
    /// View > Customize Toolbar…, while open, with the layout to restore
    /// on Cancel.
    customize: Option<(
        Entity<crate::toolbar_dialog::CustomizeToolbar>,
        Subscription,
        Vec<crate::toolbar::Entry>,
    )>,
    _toolbar_sub: Subscription,
    /// Bounds of the dock area from the last frame, for splitter math.
    area: gpui::Bounds<Pixels>,
    timing: bool,
    launched: Instant,
    /// The dock layout changed since it was last put in the shared state.
    layout_dirty: bool,
    /// Coming forward re-reads settings files edited by hand.
    _activation: Subscription,
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
            crate::state::get(cx)
                .docks
                .as_ref()
                .unwrap_or(&DockLayout::default()),
            &plugin_panels,
        );
        let activation = cx.observe_window_activation(window, |ws, window, cx| {
            if window.is_window_active() {
                crate::prefs::reload_if_changed(cx);
                crate::patterns::reload_if_changed(cx);
                crate::toolbar::reload_if_changed(cx);
                // A new name in settings.toml signs the edits from now on.
                if let Some(doc) = ws.doc().cloned() {
                    doc.update(cx, |d, cx| d.sync_author(cx));
                }
            }
        });
        let toolbar = cx.new(|_| sheaf_ui::toolbar::Toolbar::new(Default::default()));
        let toolbar_sub = cx.subscribe_in(&toolbar, window, |ws, _, ev, window, cx| {
            ws.on_toolbar(ev, window, cx)
        });
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
            export: None,
            scan: None,
            combine: None,
            split: None,
            insert_dialog: None,
            replace_dialog: None,
            crop: None,
            labels: None,
            header_footer: None,
            watermark: None,
            bookmark_tool: None,
            set_status: None,
            blanks: None,
            print: None,
            properties: None,
            ocr_dialog: None,
            #[cfg(feature = "plugins")]
            plugin_form: None,
            #[cfg(feature = "plugins")]
            plugin_manager: None,
            #[cfg(feature = "plugins")]
            doc_subs: Vec::new(),
            menu_bar: frame::draws_menu_bar().then(|| {
                cx.new(|cx| {
                    let mut bar = MenuBar::new(window, cx);
                    bar.set_state(std::rc::Rc::new(crate::availability::menu_item_state));
                    bar.set_on_secondary(std::rc::Rc::new(|action, at, window, cx| {
                        let Some(id) = cx
                            .global::<crate::commands::CommandRegistry>()
                            .for_action(action)
                            .map(|c| c.id.clone())
                        else {
                            return;
                        };
                        if let Some(ws) = window.root::<Workspace>().flatten() {
                            ws.update(cx, |ws, cx| ws.open_command_menu(id, at, cx));
                        }
                    }));
                    bar
                })
            }),
            toolbar,
            toolbar_place: ToolbarPlace::from_spec(
                &cx.try_global::<crate::toolbar::Toolbars>()
                    .map(|t| t.main().clone())
                    .unwrap_or_default(),
            ),
            toolbar_drag: None,
            doc_area: gpui::Bounds::default(),
            toolbar_from: Vec::new(),
            toolbar_menu: None,
            customize: None,
            _toolbar_sub: toolbar_sub,
            area: gpui::Bounds::default(),
            timing,
            launched,
            layout_dirty: false,
            _activation: activation,
            password,
        };
        match start_panel.as_deref() {
            Some("bookmarks") => this.show_panel(Panel::Outline),
            Some("patterns") => this.show_panel(Panel::Patterns),
            Some("calc") => this.show_panel(Panel::Calc),
            Some("comments") => this.show_panel(Panel::Comments),
            Some("attachments") => this.show_panel(Panel::Attachments),
            Some("layers") => this.show_panel(Panel::Layers),
            Some("history") => this.show_panel(Panel::History),
            Some("stamps") => this.show_panel(Panel::Stamps),
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

    /// Whether a dialog over the document has to wait for it to open,
    /// which it then says: such a dialog starts from what the file says,
    /// and its keys open it even while the menus have it greyed out.
    fn still_opening(&mut self, cx: &mut Context<Self>) -> bool {
        self.doc()
            .cloned()
            .is_some_and(|d| d.update(cx, |d, cx| d.still_opening(cx)))
    }

    /// Whether the dock on `side` is set to open (F4 aside).
    pub(crate) fn dock_shown(&self, side: Side) -> bool {
        self.dock(side).shows()
    }

    /// F4: every dock hidden.
    pub(crate) fn all_hidden(&self) -> bool {
        self.all_hidden
    }

    /// The panels on screen: each showing dock's active panel.
    pub(crate) fn visible_panels(&self, cx: &App) -> Vec<Panel> {
        let bare = self
            .doc
            .as_ref()
            .is_none_or(|d| d.read(cx).presentation() != crate::presentation::Presentation::Normal);
        if bare || self.all_hidden {
            return Vec::new();
        }
        Side::ALL
            .iter()
            .filter(|&&s| self.dock(s).shows())
            .filter_map(|&s| self.dock(s).active_panel())
            .collect()
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

    /// Remember the dock layout as the one new windows start with. Many
    /// places change it without a `cx`, so it goes to the shared state on
    /// the next frame ([`sync_layout`](Self::sync_layout)).
    fn persist_layout(&mut self) {
        self.layout_dirty = true;
    }

    fn layout(&self) -> DockLayout {
        DockLayout {
            left: self.dock(Side::Left).to_state(),
            right: self.dock(Side::Right).to_state(),
            bottom: self.dock(Side::Bottom).to_state(),
            all_hidden: self.all_hidden,
        }
    }

    /// Put a changed dock layout in the shared state.
    fn sync_layout(&mut self, cx: &mut App) {
        if std::mem::take(&mut self.layout_dirty) {
            let layout = self.layout();
            crate::state::update(cx, |s| s.docks = Some(layout));
        }
    }

    /// Remember where the window is, and its docks, for the next window.
    fn persist_window(&mut self, window: &Window, cx: &mut App) {
        let (bounds, maximized) = match window.window_bounds() {
            gpui::WindowBounds::Windowed(b) => (b, false),
            gpui::WindowBounds::Maximized(b) => (b, true),
            gpui::WindowBounds::Fullscreen(b) => (b, false),
        };
        let placement = WindowPlacement {
            x: f32::from(bounds.origin.x),
            y: f32::from(bounds.origin.y),
            w: f32::from(bounds.size.width),
            h: f32::from(bounds.size.height),
            maximized,
        };
        let layout = self.layout();
        self.layout_dirty = false;
        crate::state::update(cx, |s| {
            s.window = Some(placement);
            s.docks = Some(layout);
        });
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
        let facts = crate::availability::Facts::from_workspace(self, cx);
        let commands = crate::commands::palette_commands(&facts, window, cx);
        let palette = cx.new(|cx| Palette::new(commands, window, cx));
        let sub = cx.subscribe_in(&palette, window, |ws, _, ev: &PaletteEvent, window, cx| {
            ws.palette = None;
            ws.focus_active(window, cx);
            if let PaletteEvent::Context { id, position } = ev {
                ws.open_command_menu(id.clone(), *position, cx);
            }
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
    /// File > Open Recent: open it like File > Open, or say it's gone.
    fn open_recent(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if path.exists() {
            let origin = window.window_handle().downcast::<Workspace>();
            if let Some(origin) = origin {
                cx.defer(move |cx| open_paths(origin, vec![path], cx));
            }
            return;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        drop(sheaf_ui::prompts::ask(
            window,
            cx,
            PromptLevel::Warning,
            &format!("\"{name}\" can't be found"),
            Some(&format!(
                "It may have been moved, renamed or deleted. It's been taken off the recent files list.\n\n{}",
                path.display()
            )),
            &["OK"],
        ));
        crate::recent::forget(&path, cx);
    }

    /// File > Create > Combine Files: pick some files, then the dialog
    /// (where more can be added).
    fn combine_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.combine.is_some() {
            return;
        }
        let rx = crate::file_dialog::open_filtered(
            cx,
            gpui::PathPromptOptions {
                files: true,
                directories: false,
                multiple: true,
                prompt: Some("Choose Files to Combine".into()),
            },
            crate::file_dialog::Filter::PdfAndImages,
        );
        cx.spawn_in(window, async move |ws, cx| {
            let Ok(Ok(Some(files))) = rx.await else {
                return;
            };
            ws.update_in(cx, |ws, window, cx| {
                ws.open_combine_dialog(files, window, cx)
            })
            .ok();
        })
        .detach();
    }

    fn open_combine_dialog(
        &mut self,
        files: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let params = crate::state::get(cx).combine.clone();
        let dialog =
            cx.new(|cx| crate::combine_dialog::CombineDialog::new(files, params, window, cx));
        let sub = cx.subscribe_in(
            &dialog,
            window,
            |ws, _, ev: &crate::combine_dialog::CombineEvent, window, cx| {
                ws.combine = None;
                ws.focus_active(window, cx);
                if let crate::combine_dialog::CombineEvent::Created { path, params } = ev {
                    let params = params.clone();
                    crate::state::update(cx, |s| s.combine = params);
                    let origin = window.window_handle().downcast::<Workspace>();
                    let path = path.clone();
                    if let Some(origin) = origin {
                        cx.defer(move |cx| open_paths(origin, vec![path], cx));
                    }
                }
                cx.notify();
            },
        );
        self.combine = Some((dialog, sub));
        cx.notify();
    }

    /// File > Create > From Scans/Photos: pick the images, then the dialog.
    fn create_from_scans(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.scan.is_some() {
            return;
        }
        let rx = crate::file_dialog::open_filtered(
            cx,
            gpui::PathPromptOptions {
                files: true,
                directories: false,
                multiple: true,
                prompt: Some("Choose Photos or Scans".into()),
            },
            crate::file_dialog::Filter::Images,
        );
        cx.spawn_in(window, async move |ws, cx| {
            let Ok(Ok(Some(mut files))) = rx.await else {
                return;
            };
            files.retain(|p| sheaf_image::io::is_image_path(p));
            files.sort();
            if files.is_empty() {
                return;
            }
            ws.update_in(cx, |ws, window, cx| ws.open_scan_dialog(files, window, cx))
                .ok();
        })
        .detach();
    }

    fn open_scan_dialog(
        &mut self,
        files: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (opts, ocr) = (
            crate::state::get(cx).scan.clone(),
            crate::prefs::ocr_config(cx),
        );
        let dialog = cx.new(|cx| crate::scan_dialog::ScanDialog::new(files, opts, ocr, window, cx));
        let sub = cx.subscribe_in(
            &dialog,
            window,
            |ws, _, ev: &crate::scan_dialog::ScanEvent, window, cx| {
                ws.scan = None;
                ws.focus_active(window, cx);
                if let crate::scan_dialog::ScanEvent::Created { path, opts } = ev {
                    let opts = opts.clone();
                    crate::state::update(cx, |s| s.scan = opts);
                    let origin = window.window_handle().downcast::<Workspace>();
                    let path = path.clone();
                    if let Some(origin) = origin {
                        cx.defer(move |cx| open_paths(origin, vec![path], cx));
                    }
                }
                cx.notify();
            },
        );
        self.scan = Some((dialog, sub));
        cx.notify();
    }

    /// What the insert and replace dialogs asked for, done on the document.
    fn on_insert_event(
        &mut self,
        ev: &crate::insert_dialog::InsertEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::insert_dialog::InsertEvent;
        self.insert_dialog = None;
        self.replace_dialog = None;
        self.focus_active(window, cx);
        let Some(doc) = self.doc().cloned() else {
            cx.notify();
            return;
        };
        match ev {
            InsertEvent::Blank { count, size, at } => {
                let (count, size, at) = (*count, *size, *at);
                doc.update(cx, |d, cx| d.insert_blank(count, size, at, cx));
            }
            InsertEvent::Images { files, size, at } => {
                let (files, size, at) = (files.clone(), *size, *at);
                doc.update(cx, |d, cx| d.insert_images(files, size, at, cx));
            }
            InsertEvent::Replace { pages, file, from } => {
                let (file, from) = (file.clone(), from.clone());
                let pages = pages.clone();
                doc.update(cx, |d, cx| {
                    let ids = d.page_ids_at(&pages);
                    d.replace_pages_with(file, ids, from, cx)
                });
            }
            InsertEvent::Dismiss => {}
        }
        cx.notify();
    }

    fn open_insert_dialog(
        &mut self,
        kind: crate::insert_dialog::Insert,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.doc().is_none() || self.insert_dialog.is_some() || self.still_opening(cx) {
            return;
        }
        let dialog = cx.new(|cx| crate::insert_dialog::InsertDialog::new(kind, window, cx));
        let sub = cx.subscribe_in(&dialog, window, |ws, _, ev, window, cx| {
            ws.on_insert_event(ev, window, cx)
        });
        self.insert_dialog = Some((dialog, sub));
        cx.notify();
    }

    fn insert_from_images(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.doc().is_none() {
            return;
        }
        let rx = crate::file_dialog::open_filtered(
            cx,
            gpui::PathPromptOptions {
                files: true,
                directories: false,
                multiple: true,
                prompt: Some("Insert Images".into()),
            },
            crate::file_dialog::Filter::Images,
        );
        cx.spawn_in(window, async move |ws, cx| {
            let Ok(Ok(Some(mut files))) = rx.await else {
                return;
            };
            files.retain(|p| sheaf_image::io::is_image_path(p));
            if files.is_empty() {
                return;
            }
            ws.update_in(cx, |ws, window, cx| {
                ws.open_insert_dialog(crate::insert_dialog::Insert::Images(files), window, cx)
            })
            .ok();
        })
        .detach();
    }

    /// Replace Pages: pick the PDF, read its page count, then the dialog.
    fn replace_pages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.doc().is_none() || self.replace_dialog.is_some() {
            return;
        }
        let rx = crate::file_dialog::open(
            cx,
            gpui::PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("Replace With".into()),
            },
        );
        cx.spawn_in(window, async move |ws, cx| {
            let Ok(Ok(Some(mut files))) = rx.await else {
                return;
            };
            let Some(file) = files.pop() else { return };
            let p = file.clone();
            let counted = cx
                .background_executor()
                .spawn(async move {
                    sheaf_doc::SourceInfo::read(&sheaf_doc::Store::new(), &p, None)
                        .map(|info| info.src.page_count())
                })
                .await;
            ws.update_in(cx, |ws, window, cx| {
                let Some(doc) = ws.doc().cloned() else { return };
                let file_pages = match counted {
                    Ok(pages) => pages as usize,
                    Err(e) => {
                        doc.update(cx, |d, cx| d.set_notice(format!("Replace: {e:#}"), cx));
                        return;
                    }
                };
                let (doc_pages, targets) = doc.read_with(cx, |d, _| d.target_indexes());
                let dialog = cx.new(|cx| {
                    crate::insert_dialog::ReplaceDialog::new(
                        file, file_pages, doc_pages, &targets, window, cx,
                    )
                });
                let sub = cx.subscribe_in(&dialog, window, |ws, _, ev, window, cx| {
                    ws.on_insert_event(ev, window, cx)
                });
                ws.replace_dialog = Some((dialog, sub));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Document > Find Blank Pages….
    fn open_blanks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.blanks.is_some() || self.still_opening(cx) {
            return;
        }
        let Some(doc) = self.doc().cloned() else {
            return;
        };
        let (count, targets) = doc.read(cx).target_indexes();
        let params = crate::state::get(cx).blanks;
        let dialog = cx
            .new(|cx| crate::blanks_dialog::BlanksDialog::new(count, targets, params, window, cx));
        let sub = cx.subscribe_in(
            &dialog,
            window,
            |ws, _, ev: &crate::blanks_dialog::BlanksEvent, window, cx| {
                ws.blanks = None;
                ws.focus_active(window, cx);
                if let crate::blanks_dialog::BlanksEvent::Find(pages, params) = ev
                    && let Some(doc) = ws.doc().cloned()
                {
                    let remembered = *params;
                    crate::state::update(cx, |s| s.blanks = remembered);
                    let (pages, params) = (pages.clone(), *params);
                    doc.update(cx, |d, cx| d.find_blank_pages(pages, params, cx));
                }
                cx.notify();
            },
        );
        self.blanks = Some((dialog, sub));
        cx.notify();
    }

    /// Bookmarks from Pages… or Color Bookmarks by Level….
    fn open_bookmark_tool(&mut self, color: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.bookmark_tool.is_some() || self.still_opening(cx) {
            return;
        }
        let Some(doc) = self.doc().cloned() else {
            return;
        };
        let tool = if color {
            crate::bookmark_tools::Tool::ColorByLevel
        } else {
            let (page_count, targets) = doc.read(cx).target_indexes();
            crate::bookmark_tools::Tool::FromPages {
                page_count,
                targets,
            }
        };
        let dialog = cx.new(|cx| crate::bookmark_tools::BookmarkToolsDialog::new(tool, window, cx));
        let sub = cx.subscribe_in(
            &dialog,
            window,
            |ws, _, ev: &crate::bookmark_tools::BookmarkToolsEvent, window, cx| {
                use crate::bookmark_tools::BookmarkToolsEvent;
                ws.bookmark_tool = None;
                ws.focus_active(window, cx);
                if let Some(doc) = ws.doc().cloned() {
                    match ev {
                        BookmarkToolsEvent::FromPages {
                            pages,
                            template,
                            parent,
                        } => {
                            let (p, t, par) = (pages.clone(), template.clone(), parent.clone());
                            doc.update(cx, |d, cx| d.bookmarks_from_pages(p, t, par, cx));
                        }
                        BookmarkToolsEvent::ColorByLevel(colors) => {
                            let colors = *colors;
                            doc.update(cx, |d, cx| d.color_bookmarks_by_level(colors, cx));
                        }
                        BookmarkToolsEvent::Dismiss => {}
                    }
                }
                cx.notify();
            },
        );
        self.bookmark_tool = Some((dialog, sub));
        cx.notify();
    }

    /// Document > Set Status…: on the selected bookmark.
    fn open_set_status(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.set_status.is_some() || self.still_opening(cx) {
            return;
        }
        let Some(doc) = self.doc().cloned() else {
            return;
        };
        let Some((_, title, pages)) = doc.read(cx).status_bookmark() else {
            doc.update(cx, |d, cx| d.set_notice("Select a bookmark first", cx));
            return;
        };
        let spec = crate::state::get(cx).set_status.clone().unwrap_or_default();
        let dialog =
            cx.new(|cx| crate::set_status::SetStatusDialog::new(spec, title, pages, window, cx));
        let sub = cx.subscribe_in(
            &dialog,
            window,
            |ws, _, ev: &crate::set_status::SetStatusEvent, window, cx| {
                use crate::set_status::SetStatusEvent;
                ws.set_status = None;
                ws.focus_active(window, cx);
                if let Some(doc) = ws.doc().cloned() {
                    match ev {
                        SetStatusEvent::Set(spec) => {
                            let spec = spec.clone();
                            doc.update(cx, |d, cx| d.set_status(spec, cx));
                        }
                        SetStatusEvent::Reset { stamps } => {
                            let stamps = *stamps;
                            doc.update(cx, |d, cx| d.reset_status(stamps, cx));
                        }
                        SetStatusEvent::Dismiss => {}
                    }
                }
                cx.notify();
            },
        );
        self.set_status = Some((dialog, sub));
        cx.notify();
    }

    /// Watermark… or Background…: read what's there (in the background,
    /// since it means opening the files), then the dialog.
    fn open_watermark(&mut self, behind: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.watermark.is_some() || self.still_opening(cx) {
            return;
        }
        let Some(doc) = self.doc().cloned() else {
            return;
        };
        let Some(snapshot) = doc.read(cx).model_snapshot() else {
            return;
        };
        let page_count = snapshot.state.pages.len();
        cx.spawn_in(window, async move |ws, cx| {
            let current = cx
                .background_executor()
                .spawn(async move { sheaf_ops::derive::current_watermark(&snapshot, behind) })
                .await;
            ws.update_in(cx, |ws, window, cx| {
                let ctx = crate::watermark_dialog::WatermarkContext {
                    behind,
                    current,
                    page_count,
                };
                let dialog =
                    cx.new(|cx| crate::watermark_dialog::WatermarkDialog::new(ctx, window, cx));
                let sub = cx.subscribe_in(
                    &dialog,
                    window,
                    move |ws, _, ev: &crate::watermark_dialog::WatermarkEvent, window, cx| {
                        use crate::watermark_dialog::WatermarkEvent;
                        ws.watermark = None;
                        ws.focus_active(window, cx);
                        if let Some(doc) = ws.doc().cloned() {
                            match ev {
                                WatermarkEvent::Apply(wm, pages) => {
                                    let (wm, pages) = ((**wm).clone(), pages.clone());
                                    doc.update(cx, |d, cx| {
                                        d.apply_watermark(behind, Some(wm), pages, cx)
                                    });
                                }
                                WatermarkEvent::Remove => {
                                    doc.update(cx, |d, cx| {
                                        d.apply_watermark(behind, None, None, cx)
                                    });
                                }
                                WatermarkEvent::Dismiss => {}
                            }
                        }
                        cx.notify();
                    },
                );
                ws.watermark = Some((dialog, sub));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Header & Footer… (or Bates Numbering…, which starts with Bates on).
    fn open_header_footer(&mut self, bates: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.header_footer.is_some() || self.still_opening(cx) {
            return;
        }
        let Some(doc) = self.doc().cloned() else {
            return;
        };
        let ctx = doc.read_with(cx, |d, _| {
            let current = d.header_footer();
            let (page_count, _) = d.target_indexes();
            let scope = current
                .as_ref()
                .and_then(|h| h.pages.as_ref())
                .map(|ids| crate::insert_dialog::ranges_text(&d.indexes_of(ids)))
                .unwrap_or_default();
            crate::header_footer_dialog::HeaderFooterContext {
                current,
                page_count,
                file: d
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
                scope,
                bates,
            }
        });
        let dialog =
            cx.new(|cx| crate::header_footer_dialog::HeaderFooterDialog::new(ctx, window, cx));
        let sub = cx.subscribe_in(
            &dialog,
            window,
            |ws, _, ev: &crate::header_footer_dialog::HeaderFooterEvent, window, cx| {
                use crate::header_footer_dialog::HeaderFooterEvent;
                ws.header_footer = None;
                ws.focus_active(window, cx);
                if let Some(doc) = ws.doc().cloned() {
                    match ev {
                        HeaderFooterEvent::Apply(hf, pages) => {
                            let (mut hf, pages) = ((**hf).clone(), pages.clone());
                            doc.update(cx, |d, cx| {
                                hf.pages = pages.map(|ix| d.page_ids_at(&ix));
                                d.set_header_footer(Some(hf), cx)
                            });
                        }
                        HeaderFooterEvent::Remove => {
                            doc.update(cx, |d, cx| d.set_header_footer(None, cx));
                        }
                        HeaderFooterEvent::Dismiss => {}
                    }
                }
                cx.notify();
            },
        );
        self.header_footer = Some((dialog, sub));
        cx.notify();
    }

    fn toggle_labels(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.labels.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
            return;
        }
        if self.still_opening(cx) {
            return;
        }
        let Some(doc) = self.doc().cloned() else {
            return;
        };
        let (count, targets, has) = doc.read_with(cx, |d, _| {
            let (count, targets) = d.target_indexes();
            (count, targets, d.has_page_labels())
        });
        if count == 0 {
            return;
        }
        let dialog =
            cx.new(|cx| crate::labels_dialog::LabelsDialog::new(count, &targets, has, window, cx));
        let sub = cx.subscribe_in(
            &dialog,
            window,
            |ws, _, ev: &crate::labels_dialog::LabelsEvent, window, cx| {
                use crate::labels_dialog::LabelsEvent;
                ws.labels = None;
                ws.focus_active(window, cx);
                if let Some(doc) = ws.doc().cloned() {
                    match ev {
                        LabelsEvent::Apply {
                            from,
                            to,
                            style,
                            prefix,
                            first,
                        } => {
                            let change = (*from, *to, *style, prefix.clone(), *first);
                            doc.update(cx, |d, cx| d.set_page_labels(Some(change), cx));
                        }
                        LabelsEvent::RemoveAll => {
                            doc.update(cx, |d, cx| d.set_page_labels(None, cx));
                        }
                        LabelsEvent::Dismiss => {}
                    }
                }
                cx.notify();
            },
        );
        self.labels = Some((dialog, sub));
        cx.notify();
    }

    fn toggle_crop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.crop.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
            return;
        }
        if self.still_opening(cx) {
            return;
        }
        let Some(doc) = self.doc().cloned() else {
            return;
        };
        let ctx = doc.read_with(cx, |d, _| {
            let (page_count, targets) = d.target_indexes();
            crate::crop_dialog::CropContext {
                page_count,
                current: d.current_index(),
                selected: if targets.len() > 1 {
                    targets
                } else {
                    Vec::new()
                },
                size: d.current_size(),
            }
        });
        if ctx.page_count == 0 {
            return;
        }
        let dialog = cx.new(|cx| crate::crop_dialog::CropDialog::new(ctx, window, cx));
        let sub = cx.subscribe_in(
            &dialog,
            window,
            |ws, _, ev: &crate::crop_dialog::CropEvent, window, cx| {
                ws.crop = None;
                ws.focus_active(window, cx);
                if let crate::crop_dialog::CropEvent::Crop { pages, spec } = ev
                    && let Some(doc) = ws.doc().cloned()
                {
                    let (pages, spec) = (pages.clone(), *spec);
                    doc.update(cx, |d, cx| d.crop_pages(pages, spec, cx));
                }
                cx.notify();
            },
        );
        self.crop = Some((dialog, sub));
        cx.notify();
    }

    fn toggle_split(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.split.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
            return;
        }
        if self.still_opening(cx) {
            return;
        }
        let Some(snapshot) = self.doc().and_then(|d| d.read(cx).model_snapshot()) else {
            return;
        };
        let dialog = cx.new(|cx| crate::split_dialog::SplitDialog::new(snapshot, window, cx));
        let sub = cx.subscribe_in(
            &dialog,
            window,
            |ws, _, ev: &crate::split_dialog::SplitEvent, window, cx| {
                ws.split = None;
                ws.focus_active(window, cx);
                if let crate::split_dialog::SplitEvent::Split(by) = ev
                    && let Some(doc) = ws.doc().cloned()
                {
                    let by = by.clone();
                    doc.update(cx, |d, cx| d.split_document(by, window, cx));
                }
                cx.notify();
            },
        );
        self.split = Some((dialog, sub));
        cx.notify();
    }

    fn toggle_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.export.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
            return;
        }
        if self.still_opening(cx) {
            return;
        }
        let Some(ctx) = self.doc().and_then(|d| d.read(cx).export_context()) else {
            return;
        };
        let dialog = cx.new(|cx| crate::export_dialog::ExportDialog::new(ctx, window, cx));
        let sub = cx.subscribe_in(
            &dialog,
            window,
            |ws, _, ev: &crate::export_dialog::ExportEvent, window, cx| {
                ws.export = None;
                ws.focus_active(window, cx);
                if let crate::export_dialog::ExportEvent::Export(spec) = ev
                    && let Some(doc) = ws.doc().cloned()
                {
                    let spec = spec.clone();
                    doc.update(cx, |d, cx| d.export_pages(spec, window, cx));
                }
                cx.notify();
            },
        );
        self.export = Some((dialog, sub));
        cx.notify();
    }

    fn toggle_print(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((d, _)) = self.print.take() {
            d.update(cx, |d, _| d.release(window));
            self.focus_active(window, cx);
            cx.notify();
            return;
        }
        if self.still_opening(cx) {
            return;
        }
        let prefs = crate::state::get(cx).print.clone();
        let Some(ctx) = self.doc().and_then(|d| d.read(cx).print_context(prefs)) else {
            return;
        };
        let dialog = cx.new(|cx| crate::print_dialog::PrintDialog::new(ctx, window, cx));
        let sub = cx.subscribe_in(
            &dialog,
            window,
            |ws, dialog, ev: &crate::print_dialog::PrintEvent, window, cx| {
                dialog.update(cx, |d, _| d.release(window));
                ws.print = None;
                ws.focus_active(window, cx);
                if let crate::print_dialog::PrintEvent::Print(req) = ev {
                    let prefs = req.prefs.clone();
                    crate::state::update(cx, |s| s.print = prefs);
                    if let Some(doc) = ws.doc().cloned() {
                        let req = (**req).clone();
                        doc.update(cx, |d, cx| d.print(req, cx));
                    }
                }
                cx.notify();
            },
        );
        self.print = Some((dialog, sub));
        cx.notify();
    }

    fn toggle_properties(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.properties.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
            return;
        }
        let Some(ctx) = self.doc().and_then(|d| d.read(cx).properties_context()) else {
            return;
        };
        let dialog = cx.new(|cx| crate::properties_dialog::PropertiesDialog::new(ctx, window, cx));
        let sub = cx.subscribe_in(
            &dialog,
            window,
            |ws, _, _: &crate::properties_dialog::PropertiesEvent, window, cx| {
                ws.properties = None;
                ws.focus_active(window, cx);
                cx.notify();
            },
        );
        self.properties = Some((dialog, sub));
        cx.notify();
    }

    fn toggle_ocr_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.ocr_dialog.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
            return;
        }
        if self.still_opening(cx) {
            return;
        }
        let Some(doc) = self.doc().cloned() else {
            return;
        };
        let Some((page_count, current_page, selected)) = doc.read(cx).ocr_context() else {
            return;
        };
        let config = crate::prefs::ocr_config(cx);
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
        crate::state::update(cx, |s| {
            let cfg = &mut s.ocr;
            cfg.engine = spec.engine;
            cfg.dpi = spec.dpi;
            cfg.skip_pages_with_text = spec.skip_pages_with_text;
            cfg.last_scope = spec.scope.id().to_string();
            sheaf_ocr::set_language(spec.engine, cfg, &spec.language);
        });
        let config = crate::prefs::ocr_config(cx);
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
            self.persist_window(window, cx);
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
        if let Some(drag) = &mut self.toolbar_drag {
            if ev.pressed_button == Some(MouseButton::Left) {
                drag.at = ev.position;
                cx.notify();
            } else {
                self.drop_toolbar(cx);
            }
            return;
        }
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
        // Tabs that don't fit scroll (Shift+wheel) rather than spill
        // under the document, where they'd catch its clicks.
        let header = div()
            .id(("dock-tabs", side as usize))
            .overflow_x_scroll()
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
                Panel::History => d.render_history(window, cx),
                Panel::Stamps => d.render_stamps(window, cx),
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

    /// The document with the configurable toolbar docked at an edge or
    /// floating over it (or without one while it's off or presenting).
    fn render_doc_area(&mut self, doc: Entity<DocView>, cx: &mut Context<Self>) -> AnyElement {
        use crate::toolbar::Dock;
        use sheaf_ui::toolbar::{Orientation, THICKNESS};

        let this = cx.entity().downgrade();
        let tracker = gpui::canvas(
            |bounds, _, _| bounds,
            move |bounds: gpui::Bounds<Pixels>, _, _, cx| {
                this.update(cx, |ws, _| ws.doc_area = bounds).ok();
            },
        )
        .absolute()
        .size_full();
        let doc_el = div()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .child(doc.clone());
        let area = div().relative().size_full().flex().child(tracker);

        let spec = self.toolbar_spec(cx);
        let normal = doc.read(cx).presentation() == crate::presentation::Presentation::Normal;
        if !self.toolbar_visible(cx) || !normal {
            return area.child(doc_el).into_any_element();
        }
        let place = self.toolbar_place;
        let floating = self.toolbar_drag.is_some() || place.dock == Dock::Floating;
        let orientation = if floating {
            place.orientation
        } else {
            place.dock.orientation(place.orientation)
        };
        let facts = crate::availability::Facts::from_workspace(self, cx);
        let (items, from) = crate::toolbar::resolve(&spec.items, &facts, Some(&doc), cx);
        self.toolbar_from = from;
        let natural = self.toolbar.update(cx, |t, _| {
            t.set_orientation(orientation);
            t.set_items(items);
            t.natural_length()
        });
        let bar = self.toolbar.clone();

        if !floating {
            let vertical = orientation == Orientation::Vertical;
            let wrap = div()
                .flex_shrink_0()
                .border_color(theme::DIVIDER)
                .when(vertical, |d| d.h_full().w(px(THICKNESS)))
                .when(!vertical, |d| d.w_full())
                .child(bar);
            return match place.dock {
                Dock::Top => area.flex_col().child(wrap.border_b_1()).child(doc_el),
                Dock::Bottom => area.flex_col().child(doc_el).child(wrap.border_t_1()),
                Dock::Left => area.child(wrap.border_r_1()).child(doc_el),
                Dock::Right | Dock::Floating => area.child(doc_el).child(wrap.border_l_1()),
            }
            .into_any_element();
        }

        let size = self.doc_area.size;
        let (aw, ah) = (f32::from(size.width), f32::from(size.height));
        let (w, h) = match orientation {
            Orientation::Horizontal => (natural.min(aw - 2.0 * TOOLBAR_MARGIN).max(0.0), THICKNESS),
            Orientation::Vertical => (THICKNESS, natural.min(ah - 2.0 * TOOLBAR_MARGIN).max(0.0)),
        };
        let [x, y] = match &self.toolbar_drag {
            Some(d) => {
                let p = d.at - d.offset - self.doc_area.origin;
                [f32::from(p.x), f32::from(p.y)]
            }
            None => place.position,
        };
        let (x, y) = clamp_float(x, y, w, h, aw, ah);
        let target = self.toolbar_drag.and_then(|d| self.snap_target(d.at));
        let targets = self.toolbar_drag.is_some().then(|| {
            [Dock::Top, Dock::Bottom, Dock::Left, Dock::Right].map(|edge| {
                let strip = div()
                    .absolute()
                    .bg(theme::ACCENT)
                    .opacity(if target == Some(edge) { 0.9 } else { 0.25 });
                match edge {
                    Dock::Top => strip.top_0().left_0().w_full().h(px(6.0)),
                    Dock::Bottom => strip.bottom_0().left_0().w_full().h(px(6.0)),
                    Dock::Left => strip.top_0().left_0().h_full().w(px(6.0)),
                    _ => strip.top_0().right_0().h_full().w(px(6.0)),
                }
            })
        });
        area.child(doc_el)
            .children(targets.into_iter().flatten())
            .child(
                div()
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .w(px(w))
                    .h(px(h))
                    .rounded_md()
                    .overflow_hidden()
                    .border_1()
                    .border_color(theme::DIVIDER)
                    .shadow_lg()
                    .child(bar),
            )
            .into_any_element()
    }

    /// The edge a toolbar dropped at `at` docks to, if it's near one.
    fn snap_target(&self, at: Point<Pixels>) -> Option<crate::toolbar::Dock> {
        use crate::toolbar::Dock;
        let a = self.doc_area;
        if !a.contains(&at) {
            return None;
        }
        let x = f32::from(at.x - a.origin.x);
        let y = f32::from(at.y - a.origin.y);
        let (w, h) = (f32::from(a.size.width), f32::from(a.size.height));
        [
            (Dock::Top, y),
            (Dock::Bottom, h - y),
            (Dock::Left, x),
            (Dock::Right, w - x),
        ]
        .into_iter()
        .filter(|(_, d)| *d < TOOLBAR_SNAP)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(edge, _)| edge)
    }

    /// The grip was let go: dock to the edge it's near or float where it
    /// is, here and as the default for new windows.
    fn drop_toolbar(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.toolbar_drag.take() else {
            return;
        };
        match self.snap_target(drag.at) {
            Some(edge) => self.toolbar_place.dock = edge,
            None => {
                let p = drag.at - drag.offset - self.doc_area.origin;
                self.toolbar_place.dock = crate::toolbar::Dock::Floating;
                self.toolbar_place.position = [f32::from(p.x).max(0.0), f32::from(p.y).max(0.0)];
            }
        }
        self.store_toolbar_place(cx);
        cx.notify();
    }

    /// Write this window's toolbar placement to `toolbar.json`.
    fn store_toolbar_place(&self, cx: &mut Context<Self>) {
        let place = self.toolbar_place;
        if cx.has_global::<crate::toolbar::Toolbars>() {
            let spec = cx.global_mut::<crate::toolbar::Toolbars>().main_mut();
            spec.dock = place.dock;
            spec.position = Some(place.position);
            spec.orientation = place.orientation;
            crate::toolbar::save(cx);
        }
    }

    /// The toolbar's items as they'd be drawn now, one line each.
    pub(crate) fn toolbar_dump(&mut self, cx: &mut Context<Self>) -> Vec<String> {
        let facts = crate::availability::Facts::from_workspace(self, cx);
        let doc = self.doc().cloned();
        let spec = self.toolbar_spec(cx);
        let (items, _) = crate::toolbar::resolve(&spec.items, &facts, doc.as_ref(), cx);
        crate::toolbar::describe(&items)
    }

    /// Whether the toolbar is turned on (View > Toolbar).
    pub(crate) fn toolbar_visible(&self, cx: &App) -> bool {
        self.toolbar_spec(cx).visible
    }

    /// Change the toolbar's layout or visibility (every window's), and save.
    fn edit_toolbar(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut crate::toolbar::Spec)) {
        if !cx.has_global::<crate::toolbar::Toolbars>() {
            cx.set_global(crate::toolbar::Toolbars::default());
        }
        f(cx.global_mut::<crate::toolbar::Toolbars>().main_mut());
        crate::toolbar::save(cx);
        cx.notify();
    }

    fn toggle_toolbar(&mut self, cx: &mut Context<Self>) {
        self.edit_toolbar(cx, |s| s.visible = !s.visible);
    }

    /// Put a command at the end of the toolbar.
    fn add_to_toolbar(&mut self, id: String, cx: &mut Context<Self>) {
        self.edit_toolbar(cx, |s| {
            s.visible = true;
            s.items.push(crate::toolbar::Entry::item(&id));
        });
    }

    /// Take every copy of a command off the toolbar.
    fn remove_from_toolbar(&mut self, id: &str, cx: &mut Context<Self>) {
        self.edit_toolbar(cx, |s| {
            s.items
                .retain(|e| !matches!(e, crate::toolbar::Entry::Item { id: i, .. } if i == id))
        });
    }

    fn on_toolbar_bar(&self, id: &str, cx: &App) -> bool {
        self.toolbar_spec(cx)
            .items
            .iter()
            .any(|e| matches!(e, crate::toolbar::Entry::Item { id: i, .. } if i == id))
    }

    /// Right-click on a command in a menu or the palette.
    pub(crate) fn open_command_menu(
        &mut self,
        id: String,
        pos: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.toolbar_menu = Some((ToolbarMenu::Command(id), pos));
        cx.notify();
    }

    fn toggle_customize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((_, _, original)) = self.customize.take() {
            self.edit_toolbar(cx, |s| s.items = original);
            self.focus_active(window, cx);
            return;
        }
        let original = self.toolbar_spec(cx).items;
        let dialog =
            cx.new(|cx| crate::toolbar_dialog::CustomizeToolbar::new(original.clone(), window, cx));
        let sub = cx.subscribe_in(
            &dialog,
            window,
            |ws, _, ev: &crate::toolbar_dialog::CustomizeEvent, window, cx| match ev {
                crate::toolbar_dialog::CustomizeEvent::Changed(items) => {
                    let items = items.clone();
                    if cx.has_global::<crate::toolbar::Toolbars>() {
                        let spec = cx.global_mut::<crate::toolbar::Toolbars>().main_mut();
                        spec.items = items;
                        spec.visible = true;
                    }
                    cx.notify();
                }
                crate::toolbar_dialog::CustomizeEvent::Done(keep) => {
                    if let Some((_, _, original)) = ws.customize.take() {
                        if *keep {
                            crate::toolbar::save(cx);
                            cx.notify();
                        } else {
                            ws.edit_toolbar(cx, |s| s.items = original);
                        }
                    }
                    ws.focus_active(window, cx);
                }
            },
        );
        self.customize = Some((dialog, sub, original));
        cx.notify();
    }

    fn render_toolbar_menu(
        &self,
        menu: ToolbarMenu,
        pos: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::toolbar::Dock;
        let sep = || {
            div()
                .my_1()
                .h(px(1.0))
                .bg(theme::SEPARATOR)
                .into_any_element()
        };
        let mut items: Vec<AnyElement> = Vec::new();
        match menu {
            ToolbarMenu::Command(id) => {
                if self.on_toolbar_bar(&id, cx) {
                    items.push(menu_item(
                        "tbm-remove",
                        "Remove from Toolbar",
                        cx,
                        move |ws, _, cx| ws.remove_from_toolbar(&id, cx),
                    ));
                } else {
                    items.push(menu_item(
                        "tbm-add",
                        "Add to Toolbar",
                        cx,
                        move |ws, _, cx| ws.add_to_toolbar(id.clone(), cx),
                    ));
                }
            }
            ToolbarMenu::Bar(ix) => {
                if let Some(ix) = ix {
                    items.push(menu_item(
                        "tbm-remove",
                        "Remove from Toolbar",
                        cx,
                        move |ws, _, cx| {
                            ws.edit_toolbar(cx, |s| {
                                if ix < s.items.len() {
                                    s.items.remove(ix);
                                }
                            })
                        },
                    ));
                    items.push(sep());
                }
                items.push(menu_item(
                    "tbm-customize",
                    "Customize Toolbar…",
                    cx,
                    |ws, window, cx| ws.toggle_customize(window, cx),
                ));
                items.push(sep());
                let place = self.toolbar_place;
                for (dock, label) in [
                    (Dock::Top, "Dock at Top"),
                    (Dock::Bottom, "Dock at Bottom"),
                    (Dock::Left, "Dock at Left"),
                    (Dock::Right, "Dock at Right"),
                    (Dock::Floating, "Float over Document"),
                ] {
                    if dock != place.dock {
                        items.push(menu_item(
                            ("tbm-dock", dock as usize),
                            label,
                            cx,
                            move |ws, _, cx| {
                                ws.toolbar_place.dock = dock;
                                ws.store_toolbar_place(cx);
                            },
                        ));
                    }
                }
                if place.dock == Dock::Floating {
                    let vertical = place.orientation == sheaf_ui::toolbar::Orientation::Vertical;
                    items.push(menu_item(
                        "tbm-orient",
                        if vertical {
                            "Lay Out Across"
                        } else {
                            "Lay Out Down"
                        },
                        cx,
                        move |ws, _, cx| {
                            ws.toolbar_place.orientation = if vertical {
                                sheaf_ui::toolbar::Orientation::Horizontal
                            } else {
                                sheaf_ui::toolbar::Orientation::Vertical
                            };
                            ws.store_toolbar_place(cx);
                        },
                    ));
                }
                items.push(sep());
                items.push(menu_item("tbm-hide", "Hide Toolbar", cx, |ws, _, cx| {
                    ws.toggle_toolbar(cx)
                }));
            }
        }
        self.popup_menu("toolbar-menu", pos, items, cx)
    }

    /// A problem in `settings.toml`, shown until it's fixed or dismissed.
    fn render_settings_banner(&self, text: String, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_1()
            .bg(theme::RAISED)
            .border_b_1()
            .border_color(theme::BORDER)
            .text_sm()
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_color(theme::WARNING)
                    .child(text),
            )
            .child(
                sheaf_ui::button::Button::new("settings-open", "Open Settings File")
                    .compact()
                    .on_click(cx.listener(|_, _, _, cx| crate::prefs::open_settings_file(cx))),
            )
            .child(
                sheaf_ui::button::Button::new("settings-dismiss", "Dismiss")
                    .compact()
                    .on_click(cx.listener(|_, _, _, cx| crate::prefs::dismiss(cx))),
            )
            .into_any_element()
    }

    /// The toolbar's layout (`toolbar.json`, shared by every window).
    fn toolbar_spec(&self, cx: &App) -> crate::toolbar::Spec {
        cx.try_global::<crate::toolbar::Toolbars>()
            .map(|t| t.main().clone())
            .unwrap_or_default()
    }

    /// What the toolbar asked for.
    fn on_toolbar(
        &mut self,
        ev: &sheaf_ui::toolbar::ToolbarEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            sheaf_ui::toolbar::ToolbarEvent::Run(action) => {
                self.focus_active(window, cx);
                window.dispatch_action(action.boxed_clone(), cx);
            }
            sheaf_ui::toolbar::ToolbarEvent::Context { item, position } => {
                let ix = item.and_then(|i| self.toolbar_from.get(i).copied());
                self.toolbar_menu = Some((ToolbarMenu::Bar(ix), *position));
            }
            sheaf_ui::toolbar::ToolbarEvent::Grab { position, offset } => {
                self.toolbar_drag = Some(ToolbarDrag {
                    offset: *offset,
                    at: *position,
                });
            }
        }
        cx.notify();
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
                            ws.toolbar_menu = None;
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
        self.sync_layout(cx);
        let main: AnyElement = match self.doc().cloned() {
            Some(doc) => self.render_doc_area(doc, cx),
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
            .map(|(panel, pos)| self.render_dock_menu(panel, pos, cx))
            .or_else(|| {
                self.toolbar_menu
                    .clone()
                    .map(|(m, pos)| self.render_toolbar_menu(m, pos, cx))
            });

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

        let banner = crate::prefs::banner(cx).map(|text| self.render_settings_banner(text, cx));
        let middle = div()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .when_some(banner, |d, b| d.child(b))
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
            .on_action(cx.listener(|ws, _: &crate::ShowHistory, _, cx| {
                ws.show_panel(Panel::History);
                if let Some(doc) = ws.doc().cloned() {
                    doc.update(cx, |d, cx| d.focus_history(cx));
                }
                cx.notify();
            }))
            .on_action(cx.listener(|ws, _: &crate::ShowStamps, _, cx| {
                ws.show_panel(Panel::Stamps);
                cx.notify();
            }))
            // J picks the Stamp tool (in the document) and shows where
            // stamps are picked.
            .on_action(cx.listener(|ws, _: &crate::ToolStamp, _, cx| {
                ws.show_panel(Panel::Stamps);
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
            .on_action(cx.listener(|ws, _: &crate::Print, window, cx| ws.toggle_print(window, cx)))
            .on_action(cx.listener(|ws, _: &crate::ToggleToolbar, _, cx| ws.toggle_toolbar(cx)))
            .on_action(cx.listener(|ws, _: &crate::CustomizeToolbar, window, cx| {
                ws.toggle_customize(window, cx)
            }))
            .on_action(
                cx.listener(|ws, _: &crate::ExportPages, window, cx| ws.toggle_export(window, cx)),
            )
            .on_action(cx.listener(|ws, _: &crate::CreateFromScans, window, cx| {
                ws.create_from_scans(window, cx)
            }))
            .on_action(
                cx.listener(|ws, _: &crate::CombineFiles, window, cx| ws.combine_files(window, cx)),
            )
            .on_action(
                cx.listener(|ws, _: &crate::SplitDocument, window, cx| ws.toggle_split(window, cx)),
            )
            .on_action(cx.listener(|ws, _: &crate::InsertBlankPages, window, cx| {
                ws.open_insert_dialog(crate::insert_dialog::Insert::Blank, window, cx)
            }))
            .on_action(cx.listener(|ws, _: &crate::InsertFromImages, window, cx| {
                ws.insert_from_images(window, cx)
            }))
            .on_action(
                cx.listener(|ws, _: &crate::ReplacePages, window, cx| ws.replace_pages(window, cx)),
            )
            .on_action(
                cx.listener(|ws, _: &crate::CropPages, window, cx| ws.toggle_crop(window, cx)),
            )
            .on_action(
                cx.listener(|ws, _: &crate::PageLabels, window, cx| ws.toggle_labels(window, cx)),
            )
            .on_action(cx.listener(|ws, _: &crate::HeaderFooter, window, cx| {
                ws.open_header_footer(false, window, cx)
            }))
            .on_action(cx.listener(|ws, _: &crate::BatesNumbering, window, cx| {
                ws.open_header_footer(true, window, cx)
            }))
            .on_action(cx.listener(|ws, _: &crate::AddWatermark, window, cx| {
                ws.open_watermark(false, window, cx)
            }))
            .on_action(cx.listener(|ws, _: &crate::AddBackground, window, cx| {
                ws.open_watermark(true, window, cx)
            }))
            .on_action(
                cx.listener(|ws, _: &crate::BookmarksFromPages, window, cx| {
                    ws.open_bookmark_tool(false, window, cx)
                }),
            )
            .on_action(
                cx.listener(|ws, _: &crate::ColorBookmarksByLevel, window, cx| {
                    ws.open_bookmark_tool(true, window, cx)
                }),
            )
            .on_action(
                cx.listener(|ws, _: &crate::SetStatus, window, cx| ws.open_set_status(window, cx)),
            )
            .on_action(cx.listener(|ws, _: &crate::SetStatusAgain, window, cx| {
                let done = ws
                    .doc()
                    .cloned()
                    .is_some_and(|d| d.update(cx, |d, cx| d.set_status_again(cx)));
                if !done {
                    ws.open_set_status(window, cx);
                }
            }))
            .on_action(cx.listener(|ws, _: &crate::ResetStatus, _, cx| {
                if let Some(d) = ws.doc().cloned() {
                    d.update(cx, |d, cx| d.reset_status(false, cx));
                }
            }))
            .on_action(
                cx.listener(|ws, _: &crate::FindBlankPages, window, cx| ws.open_blanks(window, cx)),
            )
            .on_action(
                cx.listener(|ws, a: &crate::recent::OpenRecent, window, cx| {
                    ws.open_recent(a.path.clone(), window, cx)
                }),
            )
            .on_action(|_: &crate::ClearRecent, _, cx| crate::recent::clear(cx))
            .on_action(|_: &crate::OpenSettingsFile, _, cx| crate::prefs::open_settings_file(cx))
            .on_action(
                cx.listener(|ws, _: &crate::DocumentProperties, window, cx| {
                    ws.toggle_properties(window, cx)
                }),
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
                    ws.drop_toolbar(cx);
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
            )
            .children(
                self.properties
                    .as_ref()
                    .map(|(d, _)| deferred(d.clone()).with_priority(2)),
            )
            .children(
                self.print
                    .as_ref()
                    .map(|(d, _)| deferred(d.clone()).with_priority(2)),
            )
            .children(
                self.export
                    .as_ref()
                    .map(|(d, _)| deferred(d.clone()).with_priority(2)),
            )
            .children(
                self.scan
                    .as_ref()
                    .map(|(d, _)| deferred(d.clone()).with_priority(2)),
            )
            .children(
                self.combine
                    .as_ref()
                    .map(|(d, _)| deferred(d.clone()).with_priority(2)),
            )
            .children(
                self.split
                    .as_ref()
                    .map(|(d, _)| deferred(d.clone()).with_priority(2)),
            )
            .children(
                self.insert_dialog
                    .as_ref()
                    .map(|(d, _)| deferred(d.clone()).with_priority(2)),
            )
            .children(
                self.replace_dialog
                    .as_ref()
                    .map(|(d, _)| deferred(d.clone()).with_priority(2)),
            )
            .children(
                self.crop
                    .as_ref()
                    .map(|(d, _)| deferred(d.clone()).with_priority(2)),
            )
            .children(
                self.labels
                    .as_ref()
                    .map(|(d, _)| deferred(d.clone()).with_priority(2)),
            )
            .children(
                self.header_footer
                    .as_ref()
                    .map(|(d, _)| deferred(d.clone()).with_priority(2)),
            )
            .children(
                self.watermark
                    .as_ref()
                    .map(|(d, _)| deferred(d.clone()).with_priority(2)),
            )
            .children(
                self.bookmark_tool
                    .as_ref()
                    .map(|(d, _)| deferred(d.clone()).with_priority(2)),
            )
            .children(
                self.set_status
                    .as_ref()
                    .map(|(d, _)| deferred(d.clone()).with_priority(2)),
            )
            .children(
                self.blanks
                    .as_ref()
                    .map(|(d, _)| deferred(d.clone()).with_priority(2)),
            )
            .children(
                self.customize
                    .as_ref()
                    .map(|(d, _, _)| deferred(d.clone()).with_priority(2)),
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

/// Files another launch handed over, or Finder opened: as File > Open, one
/// that's already open comes forward, the next fills an empty window, and
/// the rest get windows of their own. With none, the frontmost window comes
/// forward.
pub fn open_from_launch(paths: Vec<PathBuf>, cx: &mut App) {
    let activate = |handle: WindowHandle<Workspace>, cx: &mut App| {
        handle
            .update(cx, |_, window, _| window.activate_window())
            .ok();
    };
    if paths.is_empty() {
        let front = cx
            .window_stack()
            .and_then(|stack| stack.into_iter().find_map(|w| w.downcast::<Workspace>()))
            .or_else(|| {
                cx.windows()
                    .into_iter()
                    .find_map(|w| w.downcast::<Workspace>())
            });
        match front {
            Some(handle) => activate(handle, cx),
            None => Workspace::new_window(None, cx),
        }
        return;
    }
    for path in crate::window_paths(paths).into_iter().flatten() {
        if let Some(handle) = find_open(&path, cx) {
            activate(handle, cx);
        } else if let Some(handle) = find_empty(cx) {
            handle
                .update(cx, |ws, window, cx| {
                    ws.open_path(path, window, cx);
                    window.activate_window();
                })
                .ok();
        } else {
            Workspace::new_window(Some(path), cx);
        }
    }
}

/// A window with no document in it.
fn find_empty(cx: &App) -> Option<WindowHandle<Workspace>> {
    cx.windows()
        .into_iter()
        .filter_map(|w| w.downcast::<Workspace>())
        .find(|handle| handle.read(cx).is_ok_and(|ws| ws.doc.is_none()))
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
            ws.toolbar_menu = None;
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

/// A floating toolbar's top-left, kept inside an area `aw` × `ah`.
fn clamp_float(x: f32, y: f32, w: f32, h: f32, aw: f32, ah: f32) -> (f32, f32) {
    let m = TOOLBAR_MARGIN;
    (
        x.clamp(m, (aw - w - m).max(m)),
        y.clamp(m, (ah - h - m).max(m)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floating_toolbars_stay_inside_the_document_area() {
        let m = TOOLBAR_MARGIN;
        assert_eq!(
            clamp_float(100.0, 50.0, 200.0, 34.0, 800.0, 600.0),
            (100.0, 50.0)
        );
        assert_eq!(clamp_float(-40.0, -9.0, 200.0, 34.0, 800.0, 600.0), (m, m));
        assert_eq!(
            clamp_float(700.0, 590.0, 200.0, 34.0, 800.0, 600.0),
            (800.0 - 200.0 - m, 600.0 - 34.0 - m)
        );
        // Wider than the area: pinned to the left.
        assert_eq!(clamp_float(50.0, 0.0, 900.0, 34.0, 800.0, 600.0).0, m);
    }

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
                // Built-in panels the layout predates join their dock.
                Panel::History,
                Panel::Stamps,
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
