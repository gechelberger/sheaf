//! The built-in menus, defined once. They register their commands in the
//! [`CommandRegistry`], which plugins extend; the menus shown come from
//! there. macOS shows them in the system menu bar; everywhere else
//! `sheaf_ui::menu_bar` draws them in the title bar. A `&` before a letter marks
//! its Alt-key mnemonic, as in Win32 menus.

use gpui::{Action, App, Menu};
use sheaf_ui::menu_bar::parse_label;

use crate::commands::{Command, CommandRegistry, MenuEntry, MenuSpec, Target};
use crate::*;

/// The menus to show, built-in and contributed.
pub fn app_menus(cx: &App) -> Vec<Menu> {
    cx.global::<CommandRegistry>().gpui_menus()
}

/// After the registry changes (a plugin turned on or off): set the menus
/// again and redraw every window's title-bar menus.
pub fn refresh(cx: &mut App) {
    cx.set_menus(app_menus(cx));
    for w in cx.windows() {
        if let Some(ws) = w.downcast::<crate::workspace::Workspace>() {
            ws.update(cx, |ws, _, cx| ws.reload_menus(cx)).ok();
        }
    }
}

/// Register every built-in command, then the menus that place them. A
/// command's palette category is the first menu it appears in, skipping
/// the macOS application menu (which only repeats File and Help items).
pub fn register_builtins(reg: &mut CommandRegistry) {
    let menus = builtin_menus();
    let app_menu_last = menus
        .iter()
        .filter(|m| m.0 != APP_MENU)
        .chain(menus.iter().filter(|m| m.0 == APP_MENU));
    for (name, items) in app_menu_last {
        for (label, action) in actions(items) {
            if reg.get(action.name()).is_none() {
                reg.add(Command {
                    id: action.name().to_string(),
                    label: label.to_string(),
                    category: parse_label(name).0,
                    target: Target::Builtin(action.boxed_clone()),
                })
                .expect("unique built-in id");
            }
        }
    }
    for (name, items) in menus {
        let entries = items.into_iter().map(entry).collect();
        reg.add_menu(MenuSpec {
            name: name.to_string(),
            entries,
        })
        .expect("built-in menus name registered commands");
    }
}

const APP_MENU: &str = "sheaf";

enum Item {
    Action(&'static str, Box<dyn Action>),
    Sep,
    /// A submenu; File > Open Recent fills its own (see recent.rs).
    Sub(&'static str, Vec<Item>),
}

fn entry(it: Item) -> MenuEntry {
    match it {
        Item::Action(label, action) => MenuEntry::Command {
            id: action.name().to_string(),
            label: label.to_string(),
        },
        Item::Sep => MenuEntry::Separator,
        Item::Sub(name, items) => MenuEntry::Submenu(MenuSpec {
            name: name.to_string(),
            entries: items.into_iter().map(entry).collect(),
        }),
    }
}

/// Every action in a menu's items, submenus included.
fn actions(items: &[Item]) -> Vec<(&'static str, &dyn Action)> {
    let mut out = Vec::new();
    for it in items {
        match it {
            Item::Action(label, action) => out.push((*label, action.as_ref())),
            Item::Sub(_, sub) => out.extend(actions(sub)),
            Item::Sep => {}
        }
    }
    out
}

fn builtin_menus() -> Vec<(&'static str, Vec<Item>)> {
    let mut menus = Vec::new();
    // macOS takes the first menu as the application menu.
    if cfg!(target_os = "macos") {
        menus.push(menu(
            APP_MENU,
            vec![
                item("&About sheaf", About),
                sep(),
                item("&Quit sheaf", Quit),
            ],
        ));
    }
    menus.extend([
        menu(
            "&File",
            vec![
                item("&New Window", NewWindow),
                item("&Open…", OpenFile),
                Item::Sub(
                    "Open &Recent",
                    vec![item("&Clear Recent Files", ClearRecent)],
                ),
                sep(),
                item("&Save", Save),
                item("Save &As…", SaveAs),
                item("&Export Clean Copy…", ExportCleanCopy),
                item("Export &Flattened Copy…", ExportFlattened),
                item("Export Pa&ges…", ExportPages),
                sep(),
                item("&Print…", Print),
                item("Propert&ies…", DocumentProperties),
                sep(),
                item("&Close", CloseDocument),
                item("Close &Window", CloseWindow),
                sep(),
                item("E&xit", Quit),
            ],
        ),
        menu(
            "&Edit",
            vec![
                item("&Undo", Undo),
                item("&Redo", Redo),
                sep(),
                item("Select &All Pages", SelectAllPages),
                sep(),
                item("&Find…", Find),
                item("&Pattern Search…", ShowPatterns),
            ],
        ),
        menu(
            "&View",
            vec![
                item("Zoom &In", ZoomIn),
                item("Zoom &Out", ZoomOut),
                item("&Actual Size", ZoomActual),
                item("Fit &Width", ZoomFitWidth),
                item("Fit &Page", ZoomFitPage),
                sep(),
                item("&Single Page", SinglePage),
                item("Single Page &Continuous", SinglePageContinuous),
                item("T&wo-Up", TwoUp),
                item("Two-Up Contin&uous", TwoUpContinuous),
                item("Show Co&ver Page in Two-Up", ToggleCoverPage),
                item("Rotate View Clock&wise", RotateViewCw),
                item("Rotate View Counterclockwi&se", RotateViewCcw),
                item("Spl&it", SplitView),
                item("&Read Mode", ReadMode),
                item("F&ull Screen", FullScreen),
                item("R&ulers", ToggleRulers),
                item("&Grid", ToggleGrid),
                item("Clear Gui&des", ClearGuides),
                sep(),
                item("&First Page", FirstPage),
                item("Pre&vious Page", PrevPage),
                item("&Next Page", NextPage),
                item("&Last Page", LastPage),
                item("&Go to Page…", GoToPage),
                sep(),
                item("&Thumbnails", ShowThumbnails),
                item("&Bookmarks", ShowOutline),
                item("Patte&rns", ShowPatterns),
                item("&Calculator", ShowCalc),
                item("Co&mments", ShowComments),
                item("Attac&hments", ShowAttachments),
                item("La&yers", ShowLayers),
                sep(),
                item("Command &Palette…", CommandPalette),
                sep(),
                item("Navigation Pan&es", ToggleSidebar),
                item("Left &Dock", ToggleLeftDock),
                item("Ri&ght Dock", ToggleRightDock),
                item("Botto&m Dock", ToggleBottomDock),
            ],
        ),
        menu(
            "&Document",
            vec![
                item("Rotate &Clockwise", RotatePageCw),
                item("Rotate Co&unterclockwise", RotatePageCcw),
                sep(),
                item("&Insert PDF…", InsertPdf),
                item("Du&plicate Pages", DuplicatePages),
                item("E&xtract Pages…", ExtractPages),
                item("&Delete Pages", DeletePage),
                sep(),
                item("Recognize &Text (OCR)…", RecognizeText),
                item("Cancel OC&R", CancelOcr),
                sep(),
                item("New &Bookmark", NewBookmark),
            ],
        ),
        menu(
            "&Tools",
            vec![
                item("&Select", ToolSelect),
                sep(),
                item("Sticky &Note", ToolNote),
                item("&Highlight", ToolHighlight),
                item("&Free Text", ToolFreeText),
                sep(),
                item("&Rectangle", ToolSquare),
                item("&Ellipse", ToolEllipse),
                item("&Line", ToolLine),
                item("&Arrow", ToolArrow),
                item("&Pencil", ToolInk),
                sep(),
                item("Tic&kmark", ToolTick),
                item("Co&nnector", ToolConnector),
                sep(),
                item("Calculator &Tape…", ShowCalc),
            ],
        ),
        menu("&Plugins", vec![item("&Manage Plugins…", ManagePlugins)]),
        menu("&Help", vec![item("&About sheaf", About)]),
    ]);
    menus
}

fn menu(name: &'static str, items: Vec<Item>) -> (&'static str, Vec<Item>) {
    (name, items)
}

fn item(name: &'static str, action: impl Action) -> Item {
    Item::Action(name, Box::new(action))
}

fn sep() -> Item {
    Item::Sep
}
