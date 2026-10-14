//! sheaf — a fast PDF editor. Currently: a fast PDF viewer.
//!
//!     sheaf [--timing] [file.pdf ...]
//!
//! Each file opens in its own window.

mod assets;
mod attachments_panel;
mod availability;
mod blank_review;
mod blanks_dialog;
mod bookmark_tools;
mod brand;
mod calc_panel;
mod combine_dialog;
mod command_info;
mod commands;
mod comments_panel;
mod contrib;
mod crop_dialog;
mod docview;
mod export;
mod export_dialog;
mod file_dialog;
mod goto;
mod header_footer_dialog;
mod insert_dialog;
mod instance;
mod jobs;
mod keymap;
mod labels_dialog;
mod layers_panel;
mod markup_tools;
mod menus;
mod ocr_dialog;
mod ocr_run;
mod outline_panel;
mod page_insert;
mod pane;
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
mod prefs;
mod presentation;
mod print_dialog;
mod properties_dialog;
mod recent;
mod region;
mod rulers;
mod scan_dialog;
mod script;
mod split_dialog;
mod state;
mod toolbar;
mod toolbar_dialog;
mod watermark_dialog;
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
        CancelOcr,
        SinglePage,
        SinglePageContinuous,
        TwoUp,
        TwoUpContinuous,
        ToggleCoverPage,
        RotateViewCw,
        RotateViewCcw,
        SplitView,
        NextPane,
        FullScreen,
        ReadMode,
        ToggleRulers,
        ToggleGrid,
        ClearGuides,
        GoToPage
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
        ShowAttachments,
        ShowLayers,
        CloseDocument,
        OpenFile,
        NewWindow,
        CloseWindow,
        Quit,
        ActivateMenuBar,
        About,
        CommandPalette,
        RecognizeText,
        ManagePlugins,
        DocumentProperties,
        ClearRecent,
        Print,
        ExportPages,
        CreateFromScans,
        CombineFiles,
        SplitDocument,
        InsertBlankPages,
        InsertFromImages,
        ReplacePages,
        CropPages,
        PageLabels,
        HeaderFooter,
        BatesNumbering,
        AddWatermark,
        AddBackground,
        BookmarksFromPages,
        ColorBookmarksByLevel,
        FindBlankPages,
        ToggleToolbar,
        CustomizeToolbar,
        OpenSettingsFile
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
    /// Don't hand the files to a running sheaf; run alongside it.
    new_instance: bool,
}

impl Cli {
    /// Whether this launch runs in a process of its own rather than handing
    /// its files to a running sheaf. The dev flags can't be handed over;
    /// debug builds keep to themselves unless `SHEAF_SINGLE_INSTANCE=1`,
    /// so `cargo run` never sends files to the installed sheaf.
    fn separate(&self) -> bool {
        self.new_instance
            || self.timing
            || self.find.is_some()
            || self.selftest_save.is_some()
            || self.selftest_insert.is_some()
            || self.selftest_annot.is_some()
            || self.selftest_clean.is_some()
            || self.password.is_some()
            || self.selftest_extract.is_some()
            || self.selftest_flatten.is_some()
            || self.selftest_ocr.is_some()
            || self.panel.is_some()
            || self.pattern.is_some()
            || self.menu.is_some()
            || self.script.is_some()
            || (cfg!(debug_assertions)
                && std::env::var_os("SHEAF_SINGLE_INSTANCE").is_none_or(|v| v != "1"))
    }
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
        new_instance: false,
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
            Some("--new-instance") => cli.new_instance = true,
            Some(s) if s.starts_with("--") => eprintln!("unknown flag {s}"),
            _ => cli.paths.push(arg.into()),
        }
    }
    cli
}

fn main() {
    env_logger::init();
    // `sheaf export …` and the other subcommands run headless.
    if std::env::args_os()
        .nth(1)
        .is_some_and(|a| sheaf_cli::is_subcommand(&a))
    {
        std::process::exit(sheaf_cli::run(std::env::args_os()).into());
    }
    let launched = Instant::now();
    let cli = parse_cli();
    let mode = if cli.separate() {
        sheaf_instance::Mode::Separate
    } else {
        sheaf_instance::Mode::Join
    };
    // Exits here when a running sheaf takes the files.
    let instance = instance::start(mode, &cli.paths);

    let app = application().with_assets(assets::Assets);
    // macOS hands Finder's opens to the running app as file URLs.
    let finder = instance.sink.clone();
    app.on_open_urls(move |urls| {
        let paths: Vec<PathBuf> = urls
            .iter()
            .filter_map(|u| instance::file_url_path(u))
            .collect();
        if !paths.is_empty() {
            let _ = finder.unbounded_send(sheaf_instance::Request::new(paths));
        }
    });
    app.run(move |cx: &mut App| {
        let mut cli = cli;
        instance.serve(cx);
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
        let props = Some(properties_dialog::KEY_CONTEXT);
        let tbc = Some(toolbar_dialog::KEY_CONTEXT);
        let tbc_keys = Some("CustomizeToolbar && !TextInput");
        let pd = Some(print_dialog::KEY_CONTEXT);
        let ed = Some(export_dialog::KEY_CONTEXT);
        let sd = Some(scan_dialog::KEY_CONTEXT);
        let cd = Some("CombineDialog && !TextInput");
        let spd = Some(split_dialog::KEY_CONTEXT);
        let ind = Some(insert_dialog::KEY_CONTEXT);
        let crd = Some(crop_dialog::KEY_CONTEXT);
        let lbd = Some(labels_dialog::KEY_CONTEXT);
        let hfd = Some(header_footer_dialog::KEY_CONTEXT);
        let wmd = Some(watermark_dialog::KEY_CONTEXT);
        let bmd = Some(bookmark_tools::KEY_CONTEXT);
        let bld = Some(blanks_dialog::KEY_CONTEXT);
        let bld_keys = Some("BlanksDialog && !TextInput");
        let lbd_keys = Some("LabelsDialog && !TextInput");
        let crd_keys = Some("CropDialog && !TextInput");
        let ind_keys = Some("InsertDialog && !TextInput");
        let spd_keys = Some("SplitDialog && !TextInput");
        let ed_keys = Some("ExportDialog && !TextInput");
        let pd_keys = Some("PrintDialog && !TextInput");
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
            // Acrobat: Rotate View Clockwise / Counterclockwise.
            KeyBinding::new("ctrl-shift-=", RotateViewCw, doc),
            KeyBinding::new("ctrl-shift--", RotateViewCcw, doc),
            KeyBinding::new("ctrl-\\", SplitView, doc),
            KeyBinding::new("f6", NextPane, doc),
            KeyBinding::new("ctrl-l", FullScreen, doc),
            KeyBinding::new("ctrl-h", ReadMode, doc),
            KeyBinding::new("ctrl-r", ToggleRulers, doc),
            // Acrobat: Go to Page.
            KeyBinding::new("ctrl-shift-n", GoToPage, doc),
            KeyBinding::new("ctrl-u", ToggleGrid, doc),
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
            KeyBinding::new("ctrl-alt-6", ShowAttachments, ws),
            KeyBinding::new("ctrl-alt-7", ShowLayers, ws),
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
            KeyBinding::new("alt-enter", DocumentProperties, ws),
            KeyBinding::new("ctrl-p", Print, ws),
            KeyBinding::new("ctrl-shift-e", ExportPages, ws),
            KeyBinding::new("tab", DialogNext, ed),
            KeyBinding::new("shift-tab", DialogPrev, ed),
            KeyBinding::new("left", DialogLeft, ed_keys),
            KeyBinding::new("right", DialogRight, ed_keys),
            KeyBinding::new("space", DialogRight, ed_keys),
            KeyBinding::new("enter", DialogRun, ed_keys),
            KeyBinding::new("escape", DialogCancel, ed_keys),
            KeyBinding::new("tab", DialogNext, sd),
            KeyBinding::new("shift-tab", DialogPrev, sd),
            KeyBinding::new("left", DialogLeft, sd),
            KeyBinding::new("right", DialogRight, sd),
            KeyBinding::new("space", DialogRight, sd),
            KeyBinding::new("enter", DialogRun, sd),
            KeyBinding::new("escape", DialogCancel, sd),
            KeyBinding::new("enter", DialogRun, cd),
            KeyBinding::new("escape", DialogCancel, cd),
            KeyBinding::new("tab", DialogNext, spd),
            KeyBinding::new("shift-tab", DialogPrev, spd),
            KeyBinding::new("left", DialogLeft, spd_keys),
            KeyBinding::new("right", DialogRight, spd_keys),
            KeyBinding::new("enter", DialogRun, spd_keys),
            KeyBinding::new("escape", DialogCancel, spd_keys),
            KeyBinding::new("tab", DialogNext, ind),
            KeyBinding::new("shift-tab", DialogPrev, ind),
            KeyBinding::new("left", DialogLeft, ind_keys),
            KeyBinding::new("right", DialogRight, ind_keys),
            KeyBinding::new("enter", DialogRun, ind_keys),
            KeyBinding::new("escape", DialogCancel, ind_keys),
            KeyBinding::new("tab", DialogNext, crd),
            KeyBinding::new("shift-tab", DialogPrev, crd),
            KeyBinding::new("left", DialogLeft, crd_keys),
            KeyBinding::new("right", DialogRight, crd_keys),
            KeyBinding::new("enter", DialogRun, crd_keys),
            KeyBinding::new("escape", DialogCancel, crd_keys),
            KeyBinding::new("tab", DialogNext, lbd),
            KeyBinding::new("shift-tab", DialogPrev, lbd),
            KeyBinding::new("left", DialogLeft, lbd_keys),
            KeyBinding::new("right", DialogRight, lbd_keys),
            KeyBinding::new("enter", DialogRun, lbd_keys),
            KeyBinding::new("escape", DialogCancel, lbd_keys),
            KeyBinding::new("tab", DialogNext, hfd),
            KeyBinding::new("shift-tab", DialogPrev, hfd),
            KeyBinding::new("escape", DialogCancel, hfd),
            KeyBinding::new("tab", DialogNext, wmd),
            KeyBinding::new("shift-tab", DialogPrev, wmd),
            KeyBinding::new("escape", DialogCancel, wmd),
            KeyBinding::new("tab", DialogNext, bmd),
            KeyBinding::new("shift-tab", DialogPrev, bmd),
            KeyBinding::new("enter", DialogRun, bmd),
            KeyBinding::new("escape", DialogCancel, bmd),
            KeyBinding::new("tab", DialogNext, bld),
            KeyBinding::new("shift-tab", DialogPrev, bld),
            KeyBinding::new("left", DialogLeft, bld_keys),
            KeyBinding::new("right", DialogRight, bld_keys),
            KeyBinding::new("space", DialogRight, bld_keys),
            KeyBinding::new("enter", DialogRun, bld_keys),
            KeyBinding::new("escape", DialogCancel, bld_keys),
            KeyBinding::new("tab", DialogNext, pd),
            KeyBinding::new("shift-tab", DialogPrev, pd),
            KeyBinding::new("left", DialogLeft, pd_keys),
            KeyBinding::new("right", DialogRight, pd_keys),
            KeyBinding::new("space", DialogRight, pd_keys),
            KeyBinding::new("enter", DialogRun, pd_keys),
            KeyBinding::new("escape", DialogCancel, pd_keys),
            KeyBinding::new("pageup", print_dialog::PreviewPrev, pd),
            KeyBinding::new("pagedown", print_dialog::PreviewNext, pd),
            KeyBinding::new("tab", DialogNext, props),
            KeyBinding::new("shift-tab", DialogPrev, props),
            KeyBinding::new("left", DialogLeft, props),
            KeyBinding::new("right", DialogRight, props),
            KeyBinding::new("escape", DialogCancel, props),
            KeyBinding::new("up", toolbar_dialog::ListUp, tbc),
            KeyBinding::new("down", toolbar_dialog::ListDown, tbc),
            KeyBinding::new("tab", toolbar_dialog::SwitchList, tbc),
            KeyBinding::new("shift-tab", toolbar_dialog::SwitchList, tbc),
            KeyBinding::new("alt-up", toolbar_dialog::MoveUp, tbc),
            KeyBinding::new("alt-down", toolbar_dialog::MoveDown, tbc),
            KeyBinding::new("escape", toolbar_dialog::Cancel, tbc),
            KeyBinding::new("enter", toolbar_dialog::AddOrRemove, tbc_keys),
            KeyBinding::new("delete", toolbar_dialog::RemoveItem, tbc_keys),
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
        state::init(cx);
        prefs::init(cx);
        patterns::init(cx);
        cx.set_global(commands::CommandRegistry::builtin());
        recent::init(cx);
        cx.set_global(contrib::Contributions::load());
        // Plugin bindings first, so the user's keymap can override them.
        #[cfg(feature = "plugins")]
        plugins::init(cx);
        for e in keymap::load(cx) {
            log::warn!("keymap.json: {e}");
            eprintln!("keymap.json: {e}");
        }
        toolbar::init(cx);
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
                instance::quit(cx);
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
    let window_bounds = state::get(cx)
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
        instance::quit(cx);
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
