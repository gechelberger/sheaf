//! sheaf — a fast PDF editor. Currently: a fast PDF viewer.
//!
//!     sheaf [--timing] [file.pdf ...]

mod docview;
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
        NewBookmark
    ]
);
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
        NextTab,
        PrevTab,
        CloseTab,
        OpenFile,
        Quit
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
    /// Sidebar panel to open with: "pages" (default), "bookmarks" or "patterns".
    panel: Option<String>,
    /// Run this regex in the Patterns panel once the first document loads.
    pattern: Option<String>,
}

fn parse_cli() -> Cli {
    let mut cli = Cli {
        paths: Vec::new(),
        timing: false,
        find: None,
        selftest_save: None,
        selftest_insert: None,
        panel: None,
        pattern: None,
    };
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--timing") => cli.timing = true,
            Some("--find") => cli.find = args.next().and_then(|q| q.to_str().map(String::from)),
            Some("--selftest-save") => cli.selftest_save = args.next().map(PathBuf::from),
            Some("--selftest-insert") => cli.selftest_insert = args.next().map(PathBuf::from),
            Some("--panel") => cli.panel = args.next().and_then(|p| p.to_str().map(String::from)),
            Some("--pattern") => {
                cli.pattern = args.next().and_then(|p| p.to_str().map(String::from))
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
        let either = Some("DocView || Outline");
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
        ]);

        let bounds = Bounds::centered(None, size(px(1200.0), px(850.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("sheaf".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                let ws = cx.new(|cx| {
                    Workspace::new(
                        Launch {
                            paths: cli.paths,
                            timing: cli.timing,
                            find: cli.find,
                            selftest_save: cli.selftest_save,
                            selftest_insert: cli.selftest_insert,
                            start_panel: cli.panel.take(),
                            pattern: cli.pattern.take(),
                            launched,
                        },
                        cx,
                    )
                });
                ws.update(cx, |ws, cx| ws.focus_active(window, cx));
                ws
            },
        )
        .expect("open window");
        cx.activate(true);
    });
}
