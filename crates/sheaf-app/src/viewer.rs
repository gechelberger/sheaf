//! The document view: continuous vertical scroll of fit-width pages, with
//! only the pages near the viewport rendered and kept on the GPU.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    App, Context, FocusHandle, Focusable, ObjectFit, RenderImage, ScrollHandle, SharedString, Task,
    Window, div, img, point, prelude::*, px, rgb,
};
use sheaf_core::{PageIndex, PageSize};
use sheaf_render::{PixelFormat, RenderHandle, RenderRequest, Rendered};

use crate::{
    FirstPage, LastPage, LineDown, LineUp, NextPage, PrevPage, Quit, ScreenDown, ScreenUp,
};

pub const KEY_CONTEXT: &str = "Viewer";

/// Space around the page column and between pages, in logical px.
const MARGIN: f32 = 16.0;
const GAP: f32 = 12.0;
const STATUS_H: f32 = 24.0;
/// Pages beyond the visible range to render ahead of time.
const PREFETCH: u32 = 1;
/// Pages beyond the visible range to keep cached before releasing textures.
const KEEP: u32 = 3;
const LINE_STEP: f32 = 80.0;

pub struct Cli {
    pub path: Option<PathBuf>,
    pub timing: bool,
}

enum State {
    Idle,
    Loading,
    Failed(String),
    Ready(Box<Doc>),
}

/// Per-page vertical layout at the current content width.
struct Layout {
    content_w: f32,
    sizes_known: bool,
    tops: Vec<f32>,
    heights: Vec<f32>,
    total_h: f32,
}

impl Layout {
    fn build(
        content_w: f32,
        count: u32,
        size_of: impl Fn(u32) -> PageSize,
        sizes_known: bool,
    ) -> Self {
        let mut tops = Vec::with_capacity(count as usize);
        let mut heights = Vec::with_capacity(count as usize);
        let mut y = MARGIN;
        for i in 0..count {
            let s = size_of(i);
            let h = if s.width > 0.0 {
                s.height / s.width * content_w
            } else {
                content_w
            };
            tops.push(y);
            heights.push(h);
            y += h + GAP;
        }
        let total_h = y - GAP + MARGIN;
        Self {
            content_w,
            sizes_known,
            tops,
            heights,
            total_h,
        }
    }

    /// First page whose bottom edge is at or below `y`.
    fn page_at(&self, y: f32) -> u32 {
        let i = self.tops.partition_point(|&top| top <= y);
        i.saturating_sub(1) as u32
    }

    fn visible(&self, top: f32, bottom: f32) -> std::ops::RangeInclusive<u32> {
        let last = self.tops.len().saturating_sub(1) as u32;
        self.page_at(top).min(last)..=self.page_at(bottom).min(last)
    }
}

struct Doc {
    handle: RenderHandle,
    sizes: Option<Arc<Vec<PageSize>>>,
    scroll: ScrollHandle,
    layout: Option<Layout>,
    images: HashMap<u32, (Arc<RenderImage>, RenderRequest)>,
    /// Dropping the task drops the oneshot receiver, which tells the worker
    /// to skip the render if it hasn't started.
    inflight: HashMap<u32, (RenderRequest, Task<()>)>,
    /// Images we've stopped showing; GPU textures are released in `render`,
    /// the only place we have a `Window`.
    retired: Vec<Arc<RenderImage>>,
    current_page: u32,
    /// Scroll request to apply once layout exists (or on the next frame).
    pending_scroll: Option<ScrollTo>,
    first_paint: Option<Instant>,
}

#[derive(Clone, Copy)]
enum ScrollTo {
    Page(u32),
    By(f32),
    End,
}

pub struct Viewer {
    focus_handle: FocusHandle,
    path: Option<PathBuf>,
    timing: bool,
    launched: Instant,
    state: State,
}

impl Viewer {
    pub fn new(cli: Cli, launched: Instant, cx: &mut Context<Self>) -> Self {
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
                match opened {
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
                        viewer.state = State::Ready(Box::new(Doc {
                            handle,
                            sizes: None,
                            scroll: ScrollHandle::new(),
                            layout: None,
                            images: HashMap::new(),
                            inflight: HashMap::new(),
                            retired: Vec::new(),
                            current_page: 0,
                            pending_scroll: None,
                            first_paint: None,
                        }));
                        viewer.fetch_page_sizes(cx);
                    }
                    Err(e) => viewer.state = State::Failed(format!("{e:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn fetch_page_sizes(&mut self, cx: &mut Context<Self>) {
        let State::Ready(doc) = &self.state else {
            return;
        };
        let rx = doc.handle.page_sizes();
        let timing = self.timing;
        let t = Instant::now();
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.await else { return };
            this.update(cx, |viewer, cx| {
                let State::Ready(doc) = &mut viewer.state else {
                    return;
                };
                match result {
                    Ok(sizes) => {
                        if timing {
                            eprintln!(
                                "[timing] page sizes for {} pages in {:?}",
                                sizes.len(),
                                t.elapsed()
                            );
                        }
                        doc.sizes = Some(Arc::new(sizes));
                        doc.layout = None; // rebuild with real heights
                        cx.notify();
                    }
                    Err(e) => log::error!("page sizes: {e:#}"),
                }
            })
            .ok();
        })
        .detach();
    }

    fn page_size(doc: &Doc, page: u32) -> PageSize {
        match &doc.sizes {
            Some(s) => s
                .get(page as usize)
                .copied()
                .unwrap_or(doc.handle.info().first_page),
            None => doc.handle.info().first_page,
        }
    }

    fn scroll(&mut self, to: ScrollTo, cx: &mut Context<Self>) {
        if let State::Ready(doc) = &mut self.state {
            doc.pending_scroll = Some(to);
            cx.notify();
        }
    }

    fn ensure_rendered(&mut self, want: RenderRequest, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let page = want.page.0;
        let up_to_date = doc
            .images
            .get(&page)
            .is_some_and(|(_, have)| have.target_width.abs_diff(want.target_width) <= 2);
        if up_to_date || doc.inflight.get(&page).is_some_and(|(r, _)| *r == want) {
            return;
        }
        let rx = doc.handle.render(want);
        let task = cx.spawn(async move |this, cx| {
            let result = rx.await;
            this.update(cx, |viewer, cx| viewer.on_rendered(want, result, cx))
                .ok();
        });
        // Replacing an entry drops the superseded task, cancelling its request.
        doc.inflight.insert(page, (want, task));
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
        let page = want.page.0;
        if doc.inflight.get(&page).is_some_and(|(r, _)| *r == want) {
            // We're running inside this task; detach rather than drop it.
            if let Some((_, task)) = doc.inflight.remove(&page) {
                task.detach();
            }
        }
        match result {
            Err(_canceled) => {} // superseded or scrolled away
            Ok(Err(e)) => log::error!("render page {page}: {e:#}"),
            Ok(Ok(rendered)) => {
                if timing {
                    eprintln!(
                        "[timing] page {} {}x{} rendered in {:?}",
                        page, rendered.raster.width, rendered.raster.height, rendered.render_time
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
                if let Some((old, _)) = doc.images.insert(page, (fresh, rendered.request)) {
                    doc.retired.push(old);
                }
                cx.notify();
            }
        }
    }

    fn current_page(&self) -> u32 {
        match &self.state {
            State::Ready(doc) => doc.current_page,
            _ => 0,
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
        let viewport = window.viewport_size();
        let scale = window.scale_factor();

        // Release textures for images we've replaced or evicted since last frame.
        if let State::Ready(doc) = &mut self.state {
            for old in doc.retired.drain(..) {
                if let Err(e) = window.drop_image(old) {
                    log::warn!("drop_image: {e:#}");
                }
            }
        }

        let mut body = None;
        let mut wants: Vec<RenderRequest> = Vec::new();
        let status: SharedString = match &mut self.state {
            State::Idle => "sheaf — no file. Usage: sheaf [--timing] <file.pdf>".into(),
            State::Loading => "Opening…".into(),
            State::Failed(e) => format!("Failed: {e}").into(),
            State::Ready(doc) => {
                let info = doc.handle.info();
                let count = info.page_count;

                // The scroll container's bounds lag a frame behind on first
                // layout; fall back to the window size until we have them.
                let bounds = doc.scroll.bounds();
                let (view_w, view_h) = if f32::from(bounds.size.width) > 0.0 {
                    (f32::from(bounds.size.width), f32::from(bounds.size.height))
                } else {
                    (
                        f32::from(viewport.width),
                        f32::from(viewport.height) - STATUS_H,
                    )
                };
                let content_w = (view_w - 2.0 * MARGIN).max(1.0);
                let sizes_known = doc.sizes.is_some();

                let stale = doc.layout.as_ref().is_none_or(|l| {
                    (l.content_w - content_w).abs() > 0.5 || l.sizes_known != sizes_known
                });
                if stale {
                    let sizes = doc.sizes.clone();
                    let first = info.first_page;
                    doc.layout = Some(Layout::build(
                        content_w,
                        count,
                        |i| {
                            sizes
                                .as_ref()
                                .and_then(|s| s.get(i as usize).copied())
                                .unwrap_or(first)
                        },
                        sizes_known,
                    ));
                }
                let layout = doc.layout.as_ref().expect("built above");

                // Apply any queued keyboard scroll now that we know geometry.
                let max_y = (layout.total_h - view_h).max(0.0);
                let mut scroll_y = (-f32::from(doc.scroll.offset().y)).clamp(0.0, max_y);
                if let Some(to) = doc.pending_scroll.take() {
                    scroll_y = match to {
                        ScrollTo::Page(p) => layout.tops[p.min(count - 1) as usize] - MARGIN,
                        ScrollTo::By(dy) => scroll_y + dy,
                        ScrollTo::End => max_y,
                    }
                    .clamp(0.0, max_y);
                    doc.scroll.set_offset(point(px(0.0), px(-scroll_y)));
                }

                let visible = layout.visible(scroll_y, scroll_y + view_h);
                let last = count - 1;
                doc.current_page = layout.page_at(scroll_y + view_h * 0.5).min(last);

                // Render visible pages (+ prefetch), evict far-away ones.
                let fetch =
                    visible.start().saturating_sub(PREFETCH)..=(visible.end() + PREFETCH).min(last);
                let keep = visible.start().saturating_sub(KEEP)..=(visible.end() + KEEP).min(last);
                let target_width = (content_w * scale).round().max(1.0) as u32;
                for p in fetch.clone() {
                    wants.push(RenderRequest {
                        page: PageIndex(p),
                        target_width,
                        format: PixelFormat::Bgra,
                    });
                }
                let evict: Vec<u32> = doc
                    .images
                    .keys()
                    .copied()
                    .filter(|p| !keep.contains(p))
                    .collect();
                for p in evict {
                    if let Some((im, _)) = doc.images.remove(&p) {
                        doc.retired.push(im);
                    }
                }
                doc.inflight.retain(|p, _| keep.contains(p));

                let pages = fetch.map(|p| {
                    let top = layout.tops[p as usize];
                    let h = layout.heights[p as usize];
                    let image = doc.images.get(&p).map(|(im, _)| im.clone());
                    div()
                        .absolute()
                        .top(px(top))
                        .left(px(MARGIN))
                        .w(px(content_w))
                        .h(px(h))
                        .bg(rgb(0xffffff))
                        .shadow_md()
                        .when_some(image, |d, im| {
                            d.child(img(im).w_full().h_full().object_fit(ObjectFit::Fill))
                        })
                });

                body = Some(
                    div()
                        .id("pages")
                        .track_scroll(&doc.scroll)
                        .overflow_y_scroll()
                        .size_full()
                        .child(
                            div()
                                .relative()
                                .w_full()
                                .h(px(layout.total_h))
                                .children(pages),
                        ),
                );

                let ps = Self::page_size(doc, doc.current_page);
                format!(
                    "{}  —  page {} / {}  —  {:.0}×{:.0} pt @ {:.0}%  —  fit width",
                    self.path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                    doc.current_page + 1,
                    count,
                    ps.width,
                    ps.height,
                    // Acrobat-style zoom: 100% = 72 pt per 96 logical px.
                    content_w / ps.width * 100.0 / (96.0 / 72.0)
                )
                .into()
            }
        };
        for want in wants {
            self.ensure_rendered(want, cx);
        }

        div()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|v, _: &NextPage, _, cx| {
                v.scroll(ScrollTo::Page(v.current_page() + 1), cx)
            }))
            .on_action(cx.listener(|v, _: &PrevPage, _, cx| {
                v.scroll(ScrollTo::Page(v.current_page().saturating_sub(1)), cx)
            }))
            .on_action(cx.listener(|v, _: &FirstPage, _, cx| v.scroll(ScrollTo::Page(0), cx)))
            .on_action(cx.listener(|v, _: &LastPage, _, cx| v.scroll(ScrollTo::End, cx)))
            .on_action(cx.listener(|v, _: &LineDown, _, cx| v.scroll(ScrollTo::By(LINE_STEP), cx)))
            .on_action(cx.listener(|v, _: &LineUp, _, cx| v.scroll(ScrollTo::By(-LINE_STEP), cx)))
            .on_action(cx.listener(|v, _: &ScreenDown, window, cx| {
                let h = f32::from(window.viewport_size().height) - STATUS_H;
                v.scroll(ScrollTo::By(h * 0.9), cx)
            }))
            .on_action(cx.listener(|v, _: &ScreenUp, window, cx| {
                let h = f32::from(window.viewport_size().height) - STATUS_H;
                v.scroll(ScrollTo::By(-h * 0.9), cx)
            }))
            .on_action(|_: &Quit, _, cx| cx.quit())
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x2b2b2b))
            .text_color(rgb(0xd0d0d0))
            .child(div().flex_grow(1.0).min_h(px(0.0)).children(body))
            .child(
                div()
                    .h(px(STATUS_H))
                    .flex_shrink_0()
                    .px_2()
                    .flex()
                    .items_center()
                    .bg(rgb(0x1e1e1e))
                    .text_sm()
                    .child(status),
            )
    }
}
