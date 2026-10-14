//! Reusable GPUI widgets: text fields, buttons and chips, dialogs and
//! prompts, tooltips, the title bar and menu bar, the command palette,
//! virtualized lists, and the named colors they share. The page canvas,
//! tiled viewport and sidebars are to follow.

use gpui::{App, KeyBinding};

pub mod button;
pub mod dialog;
pub mod frame;
pub mod list;
pub mod menu_bar;
pub mod palette;
pub mod prompts;
pub mod text_area;
pub mod text_input;
pub mod theme;
pub mod toolbar;
pub mod tooltip;

/// The widgets' own key bindings. Call once at startup, after the app's
/// bindings: these are in the widgets' contexts (`TextInput`, `TextArea`,
/// `MenuBar`, `Palette`), which sit deeper than the app's and so win for shared keys.
pub fn bind_keys(cx: &mut App) {
    let ti = Some(text_input::KEY_CONTEXT);
    let ta = Some(text_area::KEY_CONTEXT);
    let mb = Some(menu_bar::KEY_CONTEXT);
    let pal = Some(palette::KEY_CONTEXT);
    cx.bind_keys([
        // Text input (deeper context wins over DocView for shared keys).
        KeyBinding::new("backspace", text_input::Backspace, ti),
        KeyBinding::new("delete", text_input::Delete, ti),
        KeyBinding::new("left", text_input::Left, ti),
        KeyBinding::new("right", text_input::Right, ti),
        KeyBinding::new("shift-left", text_input::SelectLeft, ti),
        KeyBinding::new("shift-right", text_input::SelectRight, ti),
        KeyBinding::new("home", text_input::Home, ti),
        KeyBinding::new("end", text_input::End, ti),
        KeyBinding::new("ctrl-a", text_input::SelectAll, ti),
        KeyBinding::new("ctrl-v", text_input::Paste, ti),
        KeyBinding::new("ctrl-c", text_input::Copy, ti),
        KeyBinding::new("ctrl-x", text_input::Cut, ti),
        KeyBinding::new("space", text_input::InsertSpace, ti),
        KeyBinding::new("shift-space", text_input::InsertSpace, ti),
        KeyBinding::new("enter", text_input::Confirm, ti),
        KeyBinding::new("escape", text_input::Cancel, ti),
        // Multi-line text area (annotation text).
        KeyBinding::new("backspace", text_area::Backspace, ta),
        KeyBinding::new("delete", text_area::Delete, ta),
        KeyBinding::new("left", text_area::Left, ta),
        KeyBinding::new("right", text_area::Right, ta),
        KeyBinding::new("up", text_area::Up, ta),
        KeyBinding::new("down", text_area::Down, ta),
        KeyBinding::new("shift-left", text_area::SelectLeft, ta),
        KeyBinding::new("shift-right", text_area::SelectRight, ta),
        KeyBinding::new("home", text_area::Home, ta),
        KeyBinding::new("end", text_area::End, ta),
        KeyBinding::new("ctrl-a", text_area::SelectAll, ta),
        KeyBinding::new("ctrl-v", text_area::Paste, ta),
        KeyBinding::new("ctrl-c", text_area::Copy, ta),
        KeyBinding::new("ctrl-x", text_area::Cut, ta),
        KeyBinding::new("space", text_area::InsertSpace, ta),
        KeyBinding::new("shift-space", text_area::InsertSpace, ta),
        KeyBinding::new("enter", text_area::Newline, ta),
        KeyBinding::new("shift-enter", text_area::Newline, ta),
        KeyBinding::new("ctrl-enter", text_area::Confirm, ta),
        KeyBinding::new("escape", text_area::Cancel, ta),
        KeyBinding::new("left", menu_bar::MenuLeft, mb),
        KeyBinding::new("right", menu_bar::MenuRight, mb),
        KeyBinding::new("up", menu_bar::MenuUp, mb),
        KeyBinding::new("down", menu_bar::MenuDown, mb),
        KeyBinding::new("enter", menu_bar::MenuConfirm, mb),
        KeyBinding::new("space", menu_bar::MenuConfirm, mb),
        KeyBinding::new("escape", menu_bar::MenuCancel, mb),
        KeyBinding::new("up", palette::PaletteUp, pal),
        KeyBinding::new("down", palette::PaletteDown, pal),
        KeyBinding::new("pageup", palette::PalettePageUp, pal),
        KeyBinding::new("pagedown", palette::PalettePageDown, pal),
    ]);
}
