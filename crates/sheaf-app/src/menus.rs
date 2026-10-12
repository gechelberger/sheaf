//! The application menus, defined once. macOS shows them in the system menu
//! bar; everywhere else `menu_bar.rs` draws them in the title bar. A `&`
//! before a letter marks its Alt-key mnemonic, as in Win32 menus.

use gpui::{Action, Menu, MenuItem};

use crate::*;

pub fn app_menus() -> Vec<Menu> {
    let mut menus = Vec::new();
    // macOS takes the first menu as the application menu.
    if cfg!(target_os = "macos") {
        menus.push(menu(
            "sheaf",
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
                item("&Open…", OpenFile),
                sep(),
                item("&Save", Save),
                item("Save &As…", SaveAs),
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
                item("&Delete Pages", DeletePage),
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
        menu(
            "&Window",
            vec![
                item("&Next Tab", NextTab),
                item("&Previous Tab", PrevTab),
                sep(),
                item("&Close Tab", CloseTab),
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
fn label(s: &str) -> String {
    if cfg!(target_os = "macos") {
        parse_label(s).0
    } else {
        s.to_string()
    }
}

fn menu(name: &str, items: Vec<MenuItem>) -> Menu {
    Menu::new(label(name)).items(items)
}

fn item(name: &str, action: impl Action) -> MenuItem {
    MenuItem::action(label(name), action)
}

fn sep() -> MenuItem {
    MenuItem::separator()
}
