//! sheaf — a fast PDF editor. Currently: a fast PDF viewer.
//!
//!     sheaf [--timing] <file.pdf>

mod viewer;

use std::time::Instant;

use gpui::{
    App, Bounds, Focusable, KeyBinding, SharedString, TitlebarOptions, WindowBounds, WindowOptions,
    actions, prelude::*, px, size,
};
use gpui_platform::application;

use viewer::{Cli, KEY_CONTEXT, Viewer};

actions!(
    viewer,
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
        Quit
    ]
);

fn parse_cli() -> Cli {
    let mut cli = Cli {
        path: None,
        timing: false,
    };
    for arg in std::env::args_os().skip(1) {
        match arg.to_str() {
            Some("--timing") => cli.timing = true,
            Some(s) if s.starts_with("--") => eprintln!("unknown flag {s}"),
            _ => cli.path = Some(arg.into()),
        }
    }
    cli
}

fn main() {
    env_logger::init();
    let launched = Instant::now();
    let cli = parse_cli();

    application().run(move |cx: &mut App| {
        let ctx = Some(KEY_CONTEXT);
        cx.bind_keys([
            KeyBinding::new("right", NextPage, ctx),
            KeyBinding::new("left", PrevPage, ctx),
            KeyBinding::new("down", LineDown, ctx),
            KeyBinding::new("up", LineUp, ctx),
            KeyBinding::new("pagedown", ScreenDown, ctx),
            KeyBinding::new("space", ScreenDown, ctx),
            KeyBinding::new("pageup", ScreenUp, ctx),
            KeyBinding::new("shift-space", ScreenUp, ctx),
            KeyBinding::new("home", FirstPage, ctx),
            KeyBinding::new("end", LastPage, ctx),
            KeyBinding::new("ctrl-=", ZoomIn, ctx),
            KeyBinding::new("ctrl-+", ZoomIn, ctx),
            KeyBinding::new("ctrl--", ZoomOut, ctx),
            KeyBinding::new("ctrl-0", ZoomFitWidth, ctx),
            KeyBinding::new("ctrl-9", ZoomFitPage, ctx),
            KeyBinding::new("ctrl-1", ZoomActual, ctx),
            KeyBinding::new("ctrl-q", Quit, None),
            KeyBinding::new("ctrl-w", Quit, None),
        ]);

        let title: SharedString = match &cli.path {
            Some(p) => format!(
                "{} — sheaf",
                p.file_name().and_then(|s| s.to_str()).unwrap_or("?")
            )
            .into(),
            None => "sheaf".into(),
        };
        let bounds = Bounds::centered(None, size(px(1000.0), px(800.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(title),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| {
                let viewer = cx.new(|cx| Viewer::new(cli, launched, cx));
                viewer.read(cx).focus_handle(cx).focus(window, cx);
                viewer
            },
        )
        .expect("open window");
        cx.activate(true);
    });
}
