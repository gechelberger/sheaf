//! sheaf — a fast PDF editor. Currently: a fast PDF viewer.
//!
//!     sheaf [--timing] [file.pdf ...]

mod calc_panel;
mod comments_panel;
mod docview;
mod frame;
mod markup_tools;
mod menu_bar;
mod menus;
mod outline_panel;
mod pattern_panel;
mod text_input;
mod workspace;

use std::path::PathBuf;
use std::time::Instant;

use gpui::{
    App, Bounds, KeyBinding, TitlebarOptions, WindowBounds, WindowOptions, actions, prelude::*, px,
    size,
};
use gpui_platform::application;

use workspace::{Launch, Workspace};

actions!(
    docview,
    [
        NextPage,
        PrevPage,
        FirstPage,
        LastPage,
        LineDown,
        LineUp,
        ScreenDown,
        ScreenUp,
        ZoomIn,
        ZoomOut,
        ZoomActual,
        ZoomFitWidth,
        ZoomFitPage,
        Find,
        CloseSearch,
        SearchNext,
        SearchPrev,
        RotatePageCw,
        RotatePageCcw,
        DeletePage,
        Undo,
        Redo,
        DuplicatePages,
        SelectAllPages,
        Save,
        SaveAs,
        InsertPdf,
        NewBookmark,
        ExportCleanCopy
    ]
);
actions!(
    tools,
    [
        ToolSelect,
        ToolNote,
        ToolHighlight,
        ToolFreeText,
        ToolSquare,
        ToolEllipse,
        ToolLine,
        ToolArrow,
        ToolInk,
        ToolTick,
        ToolConnector
    ]
);
actions!(comments, [EditComment, DeleteComment]);
actions!(
    outline,
    [
        RenameBookmark,
        DeleteBookmark,
        SetBookmarkDestination,
        DeselectBookmark,
        ExpandAllBookmarks,
        CollapseAllBookmarks
    ]
);
actions!(
    workspace,
    [
        ToggleSidebar,
        ToggleLeftDock,
        ToggleRightDock,
        ToggleBottomDock,
        ShowThumbnails,
        ShowOutline,
        ShowPatterns,
        ShowCalc,
        ShowComments,
        NextTab,
        PrevTab,
        CloseTab,
        OpenFile,
        Quit,
        ActivateMenuBar,
        About
    ]
);

struct Cli {
    paths: Vec<PathBuf>,
    timing: bool,
    /// Open the search bar with this query once the first document loads.
    find: Option<String>,
    /// Dev: rotate page 1 of the first document and save it here.
    selftest_save: Option<PathBuf>,
    /// Dev: insert this PDF after page 1 of the first document.
    selftest_insert: Option<PathBuf>,
    /// Dev: add one of each mark to page 1 and save here.
    selftest_annot: Option<PathBuf>,
    /// Add a couple of marks, then export a clean copy here.
    selftest_clean: Option<PathBuf>,
    /// Panel to open with: "pages" (default), "bookmarks", "patterns", "calc"
    /// or "comments".
    panel: Option<String>,
    /// Run this regex in the Patterns panel once the first document loads.
    pattern: Option<String>,
    /// Dev: open the menu with this mnemonic letter at startup.
    menu: Option<char>,
}

fn parse_cli() -> Cli {
    let mut cli = Cli {
        paths: Vec::new(),
        timing: false,
        find: None,
        selftest_save: None,
        selftest_insert: None,
        selftest_annot: None,
        selftest_clean: None,
        panel: None,
        pattern: None,
        menu: None,
    };
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--timing") => cli.timing = true,
            Some("--find") => cli.find = args.next().and_then(|q| q.to_str().map(String::from)),
            Some("--selftest-save") => cli.selftest_save = args.next().map(PathBuf::from),
            Some("--selftest-insert") => cli.selftest_insert = args.next().map(PathBuf::from),
            Some("--selftest-annot") => cli.selftest_annot = args.next().map(PathBuf::from),
            Some("--selftest-clean") => cli.selftest_clean = args.next().map(PathBuf::from),
            Some("--panel") => cli.panel = args.next().and_then(|p| p.to_str().map(String::from)),
            Some("--pattern") => {
                cli.pattern = args.next().and_then(|p| p.to_str().map(String::from))
            }
            Some("--menu") => {
                cli.menu = args
                    .next()
                    .and_then(|m| m.to_str().and_then(|m| m.chars().next()))
            }
            Some(s) if s.starts_with("--") => eprintln!("unknown flag {s}"),
            _ => cli.paths.push(arg.into()),
        }
    }
    cli
}

fn main() {
    env_logger::init();
    let launched = Instant::now();
    let cli = parse_cli();

    application().run(move |cx: &mut App| {
        let mut cli = cli;
        let doc = Some(docview::KEY_CONTEXT);
        let ws = Some(workspace::KEY_CONTEXT);
        let ti = Some(text_input::KEY_CONTEXT);
        let outline = Some(outline_panel::KEY_CONTEXT);
        let comments = Some(comments_panel::KEY_CONTEXT);
        let either = Some("DocView || Outline || Comments");
        let mb = Some(menu_bar::KEY_CONTEXT);
        cx.bind_keys([
            KeyBinding::new("right", NextPage, doc),
            KeyBinding::new("left", PrevPage, doc),
            KeyBinding::new("down", LineDown, doc),
            KeyBinding::new("up", LineUp, doc),
            KeyBinding::new("pagedown", ScreenDown, doc),
            KeyBinding::new("space", ScreenDown, doc),
            KeyBinding::new("pageup", ScreenUp, doc),
            KeyBinding::new("shift-space", ScreenUp, doc),
            KeyBinding::new("home", FirstPage, doc),
            KeyBinding::new("end", LastPage, doc),
            KeyBinding::new("ctrl-=", ZoomIn, doc),
            KeyBinding::new("ctrl-+", ZoomIn, doc),
            KeyBinding::new("ctrl--", ZoomOut, doc),
            KeyBinding::new("ctrl-0", ZoomFitPage, doc),
            KeyBinding::new("ctrl-1", ZoomActual, doc),
            KeyBinding::new("ctrl-2", ZoomFitWidth, doc),
            KeyBinding::new("r", RotatePageCw, doc),
            KeyBinding::new("shift-r", RotatePageCcw, doc),
            KeyBinding::new("delete", DeletePage, doc),
            KeyBinding::new("ctrl-d", DuplicatePages, doc),
            KeyBinding::new("ctrl-a", SelectAllPages, doc),
            KeyBinding::new("ctrl-z", Undo, either),
            KeyBinding::new("ctrl-shift-z", Redo, either),
            KeyBinding::new("ctrl-y", Redo, either),
            KeyBinding::new("ctrl-s", Save, either),
            KeyBinding::new("ctrl-shift-s", SaveAs, either),
            KeyBinding::new("ctrl-shift-i", InsertPdf, doc),
            KeyBinding::new("ctrl-b", NewBookmark, either),
            // Bookmarks panel (when it has focus).
            KeyBinding::new("f2", RenameBookmark, outline),
            KeyBinding::new("delete", DeleteBookmark, outline),
            KeyBinding::new("escape", DeselectBookmark, outline),
            // Comments panel (when it has focus).
            KeyBinding::new("f2", EditComment, comments),
            KeyBinding::new("delete", DeleteComment, comments),
            // Markup tools (Acrobat's single-key accelerators where it has them).
            KeyBinding::new("v", ToolSelect, doc),
            KeyBinding::new("s", ToolNote, doc),
            KeyBinding::new("u", ToolHighlight, doc),
            KeyBinding::new("t", ToolFreeText, doc),
            KeyBinding::new("d", ToolSquare, doc),
            KeyBinding::new("o", ToolEllipse, doc),
            KeyBinding::new("l", ToolLine, doc),
            KeyBinding::new("a", ToolArrow, doc),
            KeyBinding::new("n", ToolInk, doc),
            KeyBinding::new("k", ToolTick, doc),
            KeyBinding::new("i", ToolConnector, doc),
            KeyBinding::new("ctrl-f", Find, doc),
            KeyBinding::new("escape", CloseSearch, doc),
            KeyBinding::new("f3", SearchNext, doc),
            KeyBinding::new("shift-f3", SearchPrev, doc),
            KeyBinding::new("enter", SearchNext, doc),
            KeyBinding::new("shift-enter", SearchPrev, doc),
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
            KeyBinding::new("f4", ToggleSidebar, ws),
            KeyBinding::new("ctrl-alt-1", ShowThumbnails, ws),
            KeyBinding::new("ctrl-alt-2", ShowOutline, ws),
            KeyBinding::new("ctrl-alt-3", ShowPatterns, ws),
            KeyBinding::new("ctrl-alt-4", ShowCalc, ws),
            KeyBinding::new("ctrl-alt-5", ShowComments, ws),
            KeyBinding::new("ctrl-alt-l", ToggleLeftDock, ws),
            KeyBinding::new("ctrl-alt-r", ToggleRightDock, ws),
            KeyBinding::new("ctrl-alt-j", ToggleBottomDock, ws),
            KeyBinding::new("ctrl-shift-f", ShowPatterns, ws),
            KeyBinding::new("ctrl-tab", NextTab, ws),
            KeyBinding::new("ctrl-shift-tab", PrevTab, ws),
            KeyBinding::new("ctrl-pagedown", NextTab, ws),
            KeyBinding::new("ctrl-pageup", PrevTab, ws),
            KeyBinding::new("ctrl-w", CloseTab, ws),
            KeyBinding::new("ctrl-o", OpenFile, ws),
            KeyBinding::new("ctrl-q", Quit, None),
            // Menu bar: Alt tapped alone or F10 highlights it, as on Windows.
            KeyBinding::new("alt", ActivateMenuBar, ws),
            KeyBinding::new("f10", ActivateMenuBar, ws),
            KeyBinding::new("left", menu_bar::MenuLeft, mb),
            KeyBinding::new("right", menu_bar::MenuRight, mb),
            KeyBinding::new("up", menu_bar::MenuUp, mb),
            KeyBinding::new("down", menu_bar::MenuDown, mb),
            KeyBinding::new("enter", menu_bar::MenuConfirm, mb),
            KeyBinding::new("space", menu_bar::MenuConfirm, mb),
            KeyBinding::new("escape", menu_bar::MenuCancel, mb),
        ]);
        cx.set_menus(menus::app_menus());

        let bounds = Bounds::centered(None, size(px(1200.0), px(850.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("sheaf".into()),
                    // We draw the title bar ourselves (see frame.rs).
                    appears_transparent: true,
                    ..Default::default()
                }),
                // Wayland compositors (WSLg included) may not draw a frame
                // at all; ask to draw it ourselves there.
                window_decorations: if cfg!(target_os = "linux") {
                    Some(gpui::WindowDecorations::Client)
                } else {
                    None
                },
                window_background: gpui::WindowBackgroundAppearance::Opaque,
                ..Default::default()
            },
            move |window, cx| {
                let ws = cx.new(|cx| {
                    Workspace::new(
                        window,
                        Launch {
                            paths: cli.paths,
                            timing: cli.timing,
                            find: cli.find,
                            selftest_save: cli.selftest_save,
                            selftest_insert: cli.selftest_insert,
                            selftest_annot: cli.selftest_annot,
                            selftest_clean: cli.selftest_clean,
                            start_panel: cli.panel.take(),
                            pattern: cli.pattern.take(),
                            launched,
                        },
                        cx,
                    )
                });
                ws.update(cx, |ws, cx| {
                    ws.focus_active(window, cx);
                    if let Some(ch) = cli.menu {
                        ws.open_menu(ch, window, cx);
                    }
                });
                // Alt+F4 / the compositor's close: prompt for unsaved work.
                window.on_window_should_close(cx, {
                    let ws = ws.clone();
                    move |window, cx| ws.update(cx, |ws, cx| ws.window_should_close(window, cx))
                });
                ws
            },
        )
        .expect("open window");
        // No windows means nothing to do (macOS keeps apps alive without
        // windows, but sheaf has no dock-menu life yet).
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}
