//! sheaf — a fast PDF editor. Currently: a fast PDF viewer.
//!
//!     sheaf [--timing] [file.pdf ...]

mod docview;
mod workspace;

use std::path::PathBuf;
use std::time::Instant;

use gpui::{
    App, Bounds, KeyBinding, TitlebarOptions, WindowBounds, WindowOptions, actions, prelude::*, px,
    size,
};
use gpui_platform::application;

use workspace::Workspace;

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
        ZoomFitPage
    ]
);
actions!(
    workspace,
    [
        ToggleSidebar,
        ShowThumbnails,
        ShowOutline,
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
}

fn parse_cli() -> Cli {
    let mut cli = Cli {
        paths: Vec::new(),
        timing: false,
    };
    for arg in std::env::args_os().skip(1) {
        match arg.to_str() {
            Some("--timing") => cli.timing = true,
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
        let doc = Some(docview::KEY_CONTEXT);
        let ws = Some(workspace::KEY_CONTEXT);
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
            KeyBinding::new("ctrl-0", ZoomFitWidth, doc),
            KeyBinding::new("ctrl-9", ZoomFitPage, doc),
            KeyBinding::new("ctrl-1", ZoomActual, doc),
            KeyBinding::new("f4", ToggleSidebar, ws),
            KeyBinding::new("ctrl-shift-p", ShowThumbnails, ws),
            KeyBinding::new("ctrl-shift-b", ShowOutline, ws),
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
                let ws = cx.new(|cx| Workspace::new(cli.paths, cli.timing, launched, cx));
                ws.update(cx, |ws, cx| ws.focus_active(window, cx));
                ws
            },
        )
        .expect("open window");
        cx.activate(true);
    });
}
