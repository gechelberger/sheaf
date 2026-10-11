//! M1 slice 1: open a PDF, show one page, flip pages from the keyboard.
//!
//!     sheaf [--timing] <file.pdf>

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    App, Bounds, Context, FocusHandle, Focusable, KeyBinding, ObjectFit, RenderImage, SharedString,
    TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div, img, prelude::*, px, rgb,
    size,
};
use gpui_platform::application;
use sheaf_core::PageIndex;
use sheaf_render::{PixelFormat, RenderHandle, RenderRequest, Rendered};

actions!(viewer, [NextPage, PrevPage, FirstPage, LastPage, Quit]);

const KEY_CONTEXT: &str = "Viewer";

struct Cli {
    path: Option<PathBuf>,
    timing: bool,
}

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

enum State {
    Idle,
    Loading,
    Failed(String),
    Ready(Doc),
}

struct Doc {
    handle: RenderHandle,
    page: u32,
    /// The currently displayed image and what it was rendered for.
    image: Option<(Arc<RenderImage>, RenderRequest)>,
    inflight: Option<RenderRequest>,
    first_paint: Option<Instant>,
    /// Images we've stopped showing; their GPU textures are released in
    /// `render`, the only place we have a `Window`.
    retired: Vec<Arc<RenderImage>>,
}

struct Viewer {
    focus_handle: FocusHandle,
    path: Option<PathBuf>,
    timing: bool,
    launched: Instant,
    state: State,
}

impl Viewer {
    fn new(cli: Cli, launched: Instant, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            path: cli.path,
            timing: cli.timing,
            launched,
            state: State::Idle,
        };
        this.open(cx);
        this
    }

    fn open(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.path.clone() else {
            return;
        };
        self.state = State::Loading;
        let timing = self.timing;
        let launched = self.launched;
        cx.spawn(async move |this, cx| {
            let opened = cx
                .background_executor()
                .spawn(async move { RenderHandle::open(path, None) })
                .await;
            this.update(cx, |viewer, cx| {
                viewer.state = match opened {
                    Ok(handle) => {
                        if timing {
                            let info = handle.info();
                            eprintln!(
                                "[timing] open: {:?} engine, {:?} since launch, {} pages",
                                info.open_time,
                                launched.elapsed(),
                                info.page_count
                            );
                        }
                        State::Ready(Doc {
                            handle,
                            page: 0,
                            image: None,
                            inflight: None,
                            first_paint: None,
                            retired: Vec::new(),
                        })
                    }
                    Err(e) => State::Failed(format!("{e:#}")),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn go_to(&mut self, page: i64, cx: &mut Context<Self>) {
        if let State::Ready(doc) = &mut self.state {
            let last = doc.handle.info().page_count.saturating_sub(1) as i64;
            let target = page.clamp(0, last) as u32;
            if target != doc.page {
                doc.page = target;
                cx.notify();
            }
        }
    }

    /// Called from `render`: if the displayed image doesn't match what the
    /// viewport wants, queue a render. Pull-based so resize and page flips go
    /// through one path.
    fn ensure_rendered(&mut self, want: RenderRequest, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let up_to_date = doc.image.as_ref().is_some_and(|(_, have)| {
            have.page == want.page && have.target_width.abs_diff(want.target_width) <= 2
        });
        if up_to_date || doc.inflight == Some(want) {
            return;
        }
        doc.inflight = Some(want);
        let rx = doc.handle.render(want);
        cx.spawn(async move |this, cx| {
            let result = rx.await;
            this.update(cx, |viewer, cx| {
                viewer.on_rendered(want, result, cx);
            })
            .ok();
        })
        .detach();
    }

    fn on_rendered(
        &mut self,
        want: RenderRequest,
        result: Result<anyhow::Result<Rendered>, futures::channel::oneshot::Canceled>,
        cx: &mut Context<Self>,
    ) {
        let timing = self.timing;
        let launched = self.launched;
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        if doc.inflight == Some(want) {
            doc.inflight = None;
        }
        match result {
            Err(_canceled) => {} // superseded; the newer request will land
            Ok(Err(e)) => log::error!("render failed: {e:#}"),
            Ok(Ok(rendered)) => {
                // Only accept a result for the page we're still on.
                if rendered.request.page.0 != doc.page {
                    return;
                }
                if timing {
                    eprintln!(
                        "[timing] page {} {}x{} rendered in {:?}",
                        rendered.request.page.0,
                        rendered.raster.width,
                        rendered.raster.height,
                        rendered.render_time
                    );
                    if doc.first_paint.is_none() {
                        eprintln!(
                            "[timing] first page ready {:?} since launch",
                            launched.elapsed()
                        );
                    }
                }
                doc.first_paint.get_or_insert_with(Instant::now);
                let fresh = Arc::new(to_render_image(rendered.raster));
                if let Some((old, _)) = doc.image.replace((fresh, rendered.request)) {
                    doc.retired.push(old);
                }
                cx.notify();
            }
        }
    }
}

/// Wrap a BGRA raster as a GPUI image without copying the pixel data.
/// `RenderImage` is documented as BGRA; `RgbaImage` here is just the container.
fn to_render_image(raster: sheaf_render::Raster) -> RenderImage {
    debug_assert_eq!(raster.format, PixelFormat::Bgra);
    let buf = image::RgbaImage::from_raw(raster.width, raster.height, raster.data)
        .expect("raster size matches buffer");
    RenderImage::new(smallvec::smallvec![image::Frame::new(buf)])
}

impl Focusable for Viewer {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Viewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        const PAD: f32 = 16.0;
        const STATUS_H: f32 = 24.0;
        let viewport = window.viewport_size();
        let scale = window.scale_factor();
        let avail_w = (f32::from(viewport.width) - 2.0 * PAD).max(1.0);
        let avail_h = (f32::from(viewport.height) - 2.0 * PAD - STATUS_H).max(1.0);

        // Release textures for images we've replaced since the last frame.
        if let State::Ready(doc) = &mut self.state {
            for old in doc.retired.drain(..) {
                if let Err(e) = window.drop_image(old) {
                    log::warn!("drop_image: {e:#}");
                }
            }
        }

        // Figure out what to draw and whether we need a (re)render.
        let mut page_el = None;
        let mut want = None;
        let status: SharedString = match &self.state {
            State::Idle => "sheaf — no file. Usage: sheaf [--timing] <file.pdf>".into(),
            State::Loading => "Opening…".into(),
            State::Failed(e) => format!("Failed: {e}").into(),
            State::Ready(doc) => {
                let info = doc.handle.info();
                let ps = info.page_sizes[doc.page as usize];
                // Fit the whole page in the viewport (fit-width scrolling is slice 2).
                let fit = (avail_w / ps.width).min(avail_h / ps.height);
                let disp_w = ps.width * fit;
                let disp_h = ps.height * fit;
                want = Some(RenderRequest {
                    page: PageIndex(doc.page),
                    target_width: (disp_w * scale).round().max(1.0) as u32,
                    format: PixelFormat::Bgra,
                });
                // Show the last image (even a stale page) until the new one lands;
                // that reads as "instant" rather than flashing to blank.
                let stale = doc.image.as_ref().map(|(im, _)| im.clone());
                page_el = Some(
                    div()
                        .w(px(disp_w))
                        .h(px(disp_h))
                        .bg(rgb(0xffffff))
                        .shadow_lg()
                        .when_some(stale, |d, im| {
                            d.child(img(im).w_full().h_full().object_fit(ObjectFit::Fill))
                        }),
                );
                format!(
                    "{}  —  page {} / {}  —  {:.0}×{:.0} pt @ {:.0}%",
                    self.path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                    doc.page + 1,
                    info.page_count,
                    ps.width,
                    ps.height,
                    // Acrobat-style zoom: 100% = 72 pt per 96 logical px (1 CSS inch).
                    fit * 100.0 / (96.0 / 72.0)
                )
                .into()
            }
        };
        if let Some(want) = want {
            self.ensure_rendered(want, cx);
        }

        div()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|v, _: &NextPage, _, cx| v.go_to(v.current_page() + 1, cx)))
            .on_action(cx.listener(|v, _: &PrevPage, _, cx| v.go_to(v.current_page() - 1, cx)))
            .on_action(cx.listener(|v, _: &FirstPage, _, cx| v.go_to(0, cx)))
            .on_action(cx.listener(|v, _: &LastPage, _, cx| v.go_to(i64::MAX, cx)))
            .on_action(|_: &Quit, _, cx| cx.quit())
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x2b2b2b))
            .text_color(rgb(0xd0d0d0))
            .child(
                div()
                    .flex()
                    .flex_grow(1.0)
                    .items_center()
                    .justify_center()
                    .p(px(PAD))
                    .children(page_el),
            )
            .child(
                div()
                    .h(px(STATUS_H))
                    .px_2()
                    .flex()
                    .items_center()
                    .bg(rgb(0x1e1e1e))
                    .text_sm()
                    .child(status),
            )
    }
}

impl Viewer {
    fn current_page(&self) -> i64 {
        match &self.state {
            State::Ready(doc) => doc.page as i64,
            _ => 0,
        }
    }
}

fn main() {
    env_logger::init();
    let launched = Instant::now();
    let cli = parse_cli();

    application().run(move |cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("pagedown", NextPage, Some(KEY_CONTEXT)),
            KeyBinding::new("right", NextPage, Some(KEY_CONTEXT)),
            KeyBinding::new("space", NextPage, Some(KEY_CONTEXT)),
            KeyBinding::new("pageup", PrevPage, Some(KEY_CONTEXT)),
            KeyBinding::new("left", PrevPage, Some(KEY_CONTEXT)),
            KeyBinding::new("home", FirstPage, Some(KEY_CONTEXT)),
            KeyBinding::new("end", LastPage, Some(KEY_CONTEXT)),
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
                viewer.read(cx).focus_handle.clone().focus(window, cx);
                viewer
            },
        )
        .expect("open window");
        cx.activate(true);
    });
}
