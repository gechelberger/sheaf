//! The built-in menus, defined once. They register their commands in the
//! [`CommandRegistry`], which plugins extend; the menus shown come from
//! there. macOS shows them in the system menu bar; everywhere else
//! `menu_bar.rs` draws them in the title bar. A `&` before a letter marks
//! its Alt-key mnemonic, as in Win32 menus.

use gpui::{Action, App, Menu};

use crate::commands::{Command, CommandRegistry, MenuEntry, MenuSpec, Target};
use crate::*;

/// The menus to show, built-in and contributed.
pub fn app_menus(cx: &App) -> Vec<Menu> {
    cx.global::<CommandRegistry>().gpui_menus()
}

/// After the registry changes (a plugin turned on or off): set the menus
/// again and redraw every window's title-bar menus.
#[cfg_attr(not(feature = "plugins"), allow(dead_code))]
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
        for it in items {
            if let Item::Action(label, action) = it
                && reg.get(action.name()).is_none()
            {
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
        let entries = items
            .into_iter()
            .map(|it| match it {
                Item::Action(label, action) => MenuEntry::Command {
                    id: action.name().to_string(),
                    label: label.to_string(),
                },
                Item::Sep => MenuEntry::Separator,
            })
            .collect();
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
                item("Open in Separate &Windows…", OpenInWindows),
                sep(),
                item("&Save", Save),
                item("Save &As…", SaveAs),
                item("&Export Clean Copy…", ExportCleanCopy),
                item("Export &Flattened Copy…", ExportFlattened),
                sep(),
                item("&Close", CloseTab),
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
                item("&First Page", FirstPage),
                item("Pre&vious Page", PrevPage),
                item("&Next Page", NextPage),
                item("&Last Page", LastPage),
                sep(),
                item("&Thumbnails", ShowThumbnails),
                item("&Bookmarks", ShowOutline),
                item("Patte&rns", ShowPatterns),
                item("&Calculator", ShowCalc),
                item("Co&mments", ShowComments),
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
        menu(
            "&Window",
            vec![
                item("&Next Tab", NextTab),
                item("&Previous Tab", PrevTab),
                sep(),
                item("&Move Tab to New Window", MoveTabToNewWindow),
                sep(),
                item("&Close Tab", CloseTab),
                item("Close &Window", CloseWindow),
            ],
        ),
        menu("&Help", vec![item("&About sheaf", About)]),
    ]);
    menus
}

/// Split a `&`-marked label into display text and the byte offset of its
/// mnemonic character, if any.
pub fn parse_label(label: &str) -> (String, Option<usize>) {
    match label.find('&') {
        Some(i) if i + 1 < label.len() => {
            let mut text = String::with_capacity(label.len() - 1);
            text.push_str(&label[..i]);
            text.push_str(&label[i + 1..]);
            (text, Some(i))
        }
        _ => (label.to_string(), None),
    }
}

/// The mnemonic letter of a `&`-marked label, lowercased.
pub fn mnemonic(label: &str) -> Option<char> {
    let (text, at) = parse_label(label);
    text[at?..].chars().next().map(|c| c.to_ascii_lowercase())
}

/// macOS menus don't use mnemonics; strip the markers there.
pub fn label(s: &str) -> String {
    if cfg!(target_os = "macos") {
        parse_label(s).0
    } else {
        s.to_string()
    }
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
