//! M0 spike: a GPUI window opens on Linux and Windows. That's it.

use gpui::{App, Bounds, Context, Window, WindowBounds, WindowOptions, div, prelude::*, px, rgb, size};
use gpui_platform::application;

struct Shell;

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .size_full()
            .justify_center()
            .items_center()
            .bg(rgb(0x1e1e1e))
            .text_color(rgb(0xd0d0d0))
            .text_xl()
            .child("sheaf — M0 spike")
    }
}

fn main() {
    env_logger::init();
    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(900.0), px(700.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| Shell),
        )
        .expect("open window");
        cx.activate(true);
    });
}
