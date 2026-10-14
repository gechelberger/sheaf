//! sheaf — a fast PDF editor. Currently: a fast PDF viewer.
//!
//!     sheaf [--timing] [file.pdf ...]
//!
//! Each file opens in its own window.

mod assets;
mod brand;
mod calc_panel;
mod commands;
mod comments_panel;
mod contrib;
mod docview;
mod file_dialog;
mod keymap;
mod markup_tools;
mod menus;
mod ocr_dialog;
mod ocr_run;
mod outline_panel;
mod pattern_panel;
mod patterns;
#[cfg(feature = "plugins")]
mod plugin_form;
#[cfg(feature = "plugins")]
mod plugin_manager;
#[cfg(feature = "plugins")]
mod plugin_panel;
#[cfg(feature = "plugins")]
mod plugins;
mod region;
mod script;
mod settings;
mod workspace;

use std::path::PathBuf;
use std::time::Instant;

use gpui::{
    App, Bounds, KeyBinding, TitlebarOptions, WindowBounds, WindowOptions, actions, point,
    prelude::*, px, size,
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
        ExportCleanCopy,
        ExportFlattened,
        ExtractPages,
        CancelOcr
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
        CloseDocument,
        OpenFile,
        NewWindow,
        CloseWindow,
        Quit,
        ActivateMenuBar,
        About,
        CommandPalette,
        RecognizeText,
        ManagePlugins
    ]
);
actions!(
    ocr_dialog,
    [
        DialogNext,
        DialogPrev,
        DialogLeft,
        DialogRight,
        DialogRun,
        DialogCancel
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
    /// Open the files with this password (dev; prefer the prompt).
    password: Option<String>,
    /// Extract the first two pages to this path.
    selftest_extract: Option<PathBuf>,
    /// Add marks, then export a flattened copy here.
    selftest_flatten: Option<PathBuf>,
    /// Dev: OCR every page of the first file with the default engine, then
    /// save to this path.
    selftest_ocr: Option<PathBuf>,
    /// Panel to open with: "pages" (default), "bookmarks", "patterns", "calc"
    /// or "comments".
    panel: Option<String>,
    /// Run this regex in the Patterns panel once the first document loads.
    pattern: Option<String>,
    /// Dev: open the menu with this mnemonic letter at startup.
    menu: Option<char>,
    /// Dev: drive the first window from these steps (see script.rs).
    script: Option<PathBuf>,
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
        password: None,
        selftest_extract: None,
        selftest_flatten: None,
        selftest_ocr: None,
        panel: None,
        pattern: None,
        menu: None,
        script: None,
    };
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--version") | Some("-V") => {
                println!(
                    "sheaf {} ({})",
                    env!("CARGO_PKG_VERSION"),
                    env!("SHEAF_GIT_COMMIT")
                );
                std::process::exit(0);
            }
            Some("--timing") => cli.timing = true,
            Some("--find") => cli.find = args.next().and_then(|q| q.to_str().map(String::from)),
            Some("--selftest-save") => cli.selftest_save = args.next().map(PathBuf::from),
            Some("--selftest-insert") => cli.selftest_insert = args.next().map(PathBuf::from),
            Some("--selftest-annot") => cli.selftest_annot = args.next().map(PathBuf::from),
            Some("--selftest-clean") => cli.selftest_clean = args.next().map(PathBuf::from),
            Some("--selftest-extract") => cli.selftest_extract = args.next().map(PathBuf::from),
            Some("--selftest-flatten") => cli.selftest_flatten = args.next().map(PathBuf::from),
            Some("--selftest-ocr") => cli.selftest_ocr = args.next().map(PathBuf::from),
            Some("--password") => {
                cli.password = args.next().and_then(|p| p.to_str().map(String::from))
            }
            Some("--panel") => cli.panel = args.next().and_then(|p| p.to_str().map(String::from)),
            Some("--pattern") => {
                cli.pattern = args.next().and_then(|p| p.to_str().map(String::from))
            }
            Some("--menu") => {
                cli.menu = args
                    .next()
                    .and_then(|m| m.to_str().and_then(|m| m.chars().next()))
            }
            Some("--script") => cli.script = args.next().map(PathBuf::from),
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

    application()
        .with_assets(assets::Assets)
        .run(move |cx: &mut App| {
            let mut cli = cli;
            let doc = Some(docview::KEY_CONTEXT);
            // Bare keys that a focused text field nested in these contexts must
            // receive as typed text; otherwise the outer binding swallows them.
            let doc_keys = Some("DocView && !TextInput && !TextArea");
            let outline_keys = Some("Outline && !TextInput");
            let comments_keys = Some("Comments && !TextInput && !TextArea");
            let ws = Some(workspace::KEY_CONTEXT);
            let outline = Some(outline_panel::KEY_CONTEXT);
            let comments = Some(comments_panel::KEY_CONTEXT);
            let either = Some("DocView || Outline || Comments");
            let od = Some(ocr_dialog::KEY_CONTEXT);
            let od_keys = Some("OcrDialog && !TextInput");
            let pf = Some("PluginForm");
            let pm = Some("PluginManager");
            let pf_keys = Some("PluginForm && !TextInput");
            cx.bind_keys([
                KeyBinding::new("right", NextPage, doc_keys),
                KeyBinding::new("left", PrevPage, doc_keys),
                KeyBinding::new("down", LineDown, doc_keys),
                KeyBinding::new("up", LineUp, doc_keys),
                KeyBinding::new("pagedown", ScreenDown, doc_keys),
                KeyBinding::new("space", ScreenDown, doc_keys),
                KeyBinding::new("pageup", ScreenUp, doc_keys),
                KeyBinding::new("shift-space", ScreenUp, doc_keys),
                KeyBinding::new("home", FirstPage, doc_keys),
                KeyBinding::new("end", LastPage, doc_keys),
                KeyBinding::new("ctrl-=", ZoomIn, doc),
                KeyBinding::new("ctrl-+", ZoomIn, doc),
                KeyBinding::new("ctrl--", ZoomOut, doc),
                KeyBinding::new("ctrl-0", ZoomFitPage, doc),
                KeyBinding::new("ctrl-1", ZoomActual, doc),
                KeyBinding::new("ctrl-2", ZoomFitWidth, doc),
                KeyBinding::new("r", RotatePageCw, doc_keys),
                KeyBinding::new("shift-r", RotatePageCcw, doc_keys),
                KeyBinding::new("delete", DeletePage, doc_keys),
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
                KeyBinding::new("f2", RenameBookmark, outline_keys),
                KeyBinding::new("delete", DeleteBookmark, outline),
                KeyBinding::new("escape", DeselectBookmark, outline),
                // Comments panel (when it has focus).
                KeyBinding::new("f2", EditComment, comments_keys),
                KeyBinding::new("delete", DeleteComment, comments),
                // Markup tools (Acrobat's single-key accelerators where it has them).
                KeyBinding::new("v", ToolSelect, doc_keys),
                KeyBinding::new("s", ToolNote, doc_keys),
                KeyBinding::new("u", ToolHighlight, doc_keys),
                KeyBinding::new("t", ToolFreeText, doc_keys),
                KeyBinding::new("d", ToolSquare, doc_keys),
                KeyBinding::new("o", ToolEllipse, doc_keys),
                KeyBinding::new("l", ToolLine, doc_keys),
                KeyBinding::new("a", ToolArrow, doc_keys),
                KeyBinding::new("n", ToolInk, doc_keys),
                KeyBinding::new("k", ToolTick, doc_keys),
                KeyBinding::new("i", ToolConnector, doc_keys),
                KeyBinding::new("ctrl-f", Find, doc),
                KeyBinding::new("escape", CloseSearch, doc),
                KeyBinding::new("f3", SearchNext, doc),
                KeyBinding::new("shift-f3", SearchPrev, doc),
                KeyBinding::new("enter", SearchNext, doc),
                KeyBinding::new("shift-enter", SearchPrev, doc),
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
                KeyBinding::new("ctrl-w", CloseDocument, ws),
                KeyBinding::new("ctrl-o", OpenFile, ws),
                KeyBinding::new("ctrl-n", NewWindow, ws),
                KeyBinding::new("ctrl-shift-w", CloseWindow, ws),
                KeyBinding::new("ctrl-q", Quit, None),
                // Acrobat has no palette; Ctrl+Shift+P is its Page Setup, so not that.
                KeyBinding::new("ctrl-shift-k", CommandPalette, None),
                // Recognize Text (OCR). Acrobat has no default shortcut for it.
                KeyBinding::new("ctrl-shift-t", RecognizeText, ws),
                KeyBinding::new("tab", DialogNext, od),
                KeyBinding::new("shift-tab", DialogPrev, od),
                KeyBinding::new("down", DialogNext, od_keys),
                KeyBinding::new("up", DialogPrev, od_keys),
                KeyBinding::new("left", DialogLeft, od_keys),
                KeyBinding::new("right", DialogRight, od_keys),
                KeyBinding::new("space", DialogRight, od_keys),
                KeyBinding::new("enter", DialogRun, od_keys),
                KeyBinding::new("escape", DialogCancel, od_keys),
                // Plugin forms: same keys as the OCR card.
                KeyBinding::new("tab", DialogNext, pf),
                KeyBinding::new("shift-tab", DialogPrev, pf),
                KeyBinding::new("left", DialogLeft, pf_keys),
                KeyBinding::new("right", DialogRight, pf_keys),
                KeyBinding::new("space", DialogRight, pf_keys),
                KeyBinding::new("enter", DialogRun, pf_keys),
                KeyBinding::new("escape", DialogCancel, pf_keys),
                KeyBinding::new("escape", DialogCancel, pm),
                // Menu bar: Alt tapped alone or F10 highlights it, as on Windows.
                KeyBinding::new("alt", ActivateMenuBar, ws),
                KeyBinding::new("f10", ActivateMenuBar, ws),
            ]);
            sheaf_ui::bind_keys(cx);
            cx.set_global(commands::CommandRegistry::builtin());
            cx.set_global(contrib::Contributions::load());
            // Plugin bindings first, so the user's keymap can override them.
            #[cfg(feature = "plugins")]
            plugins::init(cx);
            for e in keymap::load(cx) {
                log::warn!("keymap.json: {e}");
                eprintln!("keymap.json: {e}");
            }
            cx.set_menus(menus::app_menus(cx));
            sheaf_ui::prompts::install(cx);

            let mut windows = window_paths(cli.paths).into_iter();
            let password = cli.password.clone();
            let first = open_workspace(
                Launch {
                    path: windows.next().expect("at least one window"),
                    timing: cli.timing,
                    find: cli.find,
                    selftest_save: cli.selftest_save,
                    selftest_insert: cli.selftest_insert,
                    selftest_annot: cli.selftest_annot,
                    selftest_clean: cli.selftest_clean,
                    start_panel: cli.panel.take(),
                    pattern: cli.pattern.take(),
                    launched,
                    settings: settings::Settings::load(),
                    password: cli.password.take(),
                    selftest_extract: cli.selftest_extract,
                    selftest_flatten: cli.selftest_flatten,
                    selftest_ocr: cli.selftest_ocr,
                },
                cli.menu,
                cx,
            )
            .expect("open window");
            if let Some(path) = cli.script.take() {
                script::start(path, first, cx);
            }
            for path in windows {
                let mut launch = Launch::file(path);
                launch.timing = cli.timing;
                launch.launched = launched;
                launch.password = password.clone();
                open_workspace(launch, None, cx).expect("open window");
            }
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

/// Every window gets an independent workspace and the same close protection.
fn open_workspace(
    launch: Launch,
    menu: Option<char>,
    cx: &mut App,
) -> anyhow::Result<gpui::WindowHandle<Workspace>> {
    let window_bounds = launch
        .settings
        .window
        .as_ref()
        .and_then(|w| workspace::restore_window_bounds(w, cx))
        .unwrap_or_else(|| {
            WindowBounds::Windowed(Bounds::centered(None, size(px(1200.0), px(850.0)), cx))
        });
    // Cascade so a new window doesn't sit exactly on top of the last one.
    let window_bounds = match window_bounds {
        WindowBounds::Windowed(mut b) => {
            let step = px(24.0 * cx.windows().len() as f32);
            b.origin += point(step, step);
            WindowBounds::Windowed(b)
        }
        other => other,
    };
    cx.open_window(
        WindowOptions {
            window_bounds: Some(window_bounds),
            titlebar: Some(TitlebarOptions {
                title: Some("sheaf".into()),
                // We draw the title bar ourselves (see sheaf_ui::frame).
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
            // Matches StartupWMClass in sheaf.desktop, which is how
            // Wayland finds the icon.
            app_id: Some("sheaf".into()),
            icon: brand::window_icon(),
            ..Default::default()
        },
        move |window, cx| {
            let ws = cx.new(|cx| Workspace::new(window, launch, cx));
            ws.update(cx, |ws, cx| {
                ws.focus_active(window, cx);
                if let Some(ch) = menu {
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
}

#[derive(Default)]
struct Quitting(bool);
impl gpui::Global for Quitting {}

fn request_quit(cx: &mut App) {
    if cx.try_global::<Quitting>().is_some_and(|q| q.0) {
        return;
    }
    cx.set_global(Quitting(true));
    close_next_window(cx);
}

fn close_next_window(cx: &mut App) {
    if !cx.try_global::<Quitting>().is_some_and(|q| q.0) {
        return;
    }
    if let Some(handle) = cx
        .windows()
        .into_iter()
        .find_map(|w| w.downcast::<Workspace>())
    {
        let _ = handle.update(cx, |ws, window, cx| ws.request_close(window, cx));
    } else {
        cx.quit();
    }
}

/// One window per file; an empty invocation still opens one, empty.
fn window_paths(paths: Vec<PathBuf>) -> Vec<Option<PathBuf>> {
    // One view per file, as when opening from the dialog.
    let mut unique: Vec<PathBuf> = Vec::new();
    for p in paths {
        if !unique.iter().any(|u| workspace::same_file(u, &p)) {
            unique.push(p);
        }
    }
    if unique.is_empty() {
        return vec![None];
    }
    unique.into_iter().map(Some).collect()
}

#[cfg(test)]
mod window_tests {
    use super::*;

    #[test]
    fn five_pdfs_open_five_windows() {
        let paths: Vec<_> = (1..=5).map(|n| PathBuf::from(format!("{n}.pdf"))).collect();
        let windows = window_paths(paths.clone());
        assert_eq!(windows, paths.into_iter().map(Some).collect::<Vec<_>>());
    }

    #[test]
    fn a_file_named_twice_opens_once() {
        let paths = vec![
            PathBuf::from("a.pdf"),
            PathBuf::from("b.pdf"),
            PathBuf::from("a.pdf"),
        ];
        assert_eq!(
            window_paths(paths),
            vec![Some(PathBuf::from("a.pdf")), Some(PathBuf::from("b.pdf"))]
        );
    }

    #[test]
    fn no_files_opens_one_empty_workspace() {
        assert_eq!(window_paths(Vec::new()), vec![None]);
    }
}
