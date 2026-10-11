//! One open document: continuous vertical scroll of pages at a shared zoom,
//! rendered as tiles so only what's on screen is rasterized, with a cheap
//! low-res preview under the tiles so something appears immediately. Also
//! owns the per-document sidebar content (thumbnails, outline), since those
//! share its render handle and caches.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    AnyElement, App, Context, FocusHandle, Focusable, ObjectFit, RenderImage, ScrollHandle,
    ScrollWheelEvent, SharedString, Task, Window, div, img, point, prelude::*, px, rgb,
};
use sheaf_core::{PageIndex, PageSize};
use sheaf_render::{Job, OutlineItem, PixelFormat, RenderHandle, RenderRequest, Rendered};

use crate::{
    FirstPage, LastPage, LineDown, LineUp, NextPage, PrevPage, ScreenDown, ScreenUp, ZoomActual,
    ZoomFitPage, ZoomFitWidth, ZoomIn, ZoomOut,
};

pub const KEY_CONTEXT: &str = "DocView";

/// Space around the page column and between pages, in logical px.
const MARGIN: f32 = 16.0;
const GAP: f32 = 12.0;
const STATUS_H: f32 = 24.0;
/// Pages beyond the visible range to prefetch previews for.
const PREFETCH: u32 = 1;
/// Pages beyond the visible range to keep cached before releasing textures.
const KEEP: u32 = 3;
const LINE_STEP: f32 = 80.0;
/// Tile edge in device pixels. 1024² BGRA = 4 MB per tile.
const TILE: u32 = 1024;
/// Width of the low-res whole-page preview, device px. Also used for thumbnails.
const PREVIEW_W: u32 = 320;
/// Vertical overdraw for tile prefetch, as a fraction of the viewport.
const TILE_OVERDRAW: f32 = 0.5;
/// 100% zoom = 96 logical px per inch = 96/72 px per PDF point (Acrobat's convention).
const PX_PER_PT_AT_100: f32 = 96.0 / 72.0;
const ZOOM_STEP: f32 = 1.25;
const ZOOM_MIN: f32 = 0.1;
const ZOOM_MAX: f32 = 16.0;

/// Thumbnail panel geometry, logical px.
const THUMB_W: f32 = 150.0;
const THUMB_PAD: f32 = 10.0;
const THUMB_LABEL_H: f32 = 18.0;
const OUTLINE_ROW_H: f32 = 24.0;
const OUTLINE_INDENT: f32 = 14.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Zoom {
    FitWidth,
    FitPage,
    /// 1.0 = 100%.
    Custom(f32),
}

enum State {
    Loading,
    Failed(String),
    Ready(Box<Doc>),
}

/// Page geometry in logical px at the current zoom.
struct Layout {
    zoom: Zoom,
    view_w: f32,
    view_h: f32,
    sizes_known: bool,
    /// Logical px per PDF point.
    k: f32,
    widths: Vec<f32>,
    heights: Vec<f32>,
    tops: Vec<f32>,
    lefts: Vec<f32>,
    /// Scrollable content size.
    total_w: f32,
    total_h: f32,
}

impl Layout {
    fn build(
        zoom: Zoom,
        view_w: f32,
        view_h: f32,
        count: u32,
        size_of: &dyn Fn(u32) -> PageSize,
        sizes_known: bool,
    ) -> Self {
        let avail_w = (view_w - 2.0 * MARGIN).max(1.0);
        let avail_h = (view_h - 2.0 * MARGIN).max(1.0);
        let (mut max_w, mut max_h) = (1.0f32, 1.0f32);
        for i in 0..count {
            let s = size_of(i);
            max_w = max_w.max(s.width);
            max_h = max_h.max(s.height);
        }
        let k = match zoom {
            Zoom::FitWidth => avail_w / max_w,
            Zoom::FitPage => (avail_w / max_w).min(avail_h / max_h),
            Zoom::Custom(z) => z * PX_PER_PT_AT_100,
        };
        let total_w = (max_w * k + 2.0 * MARGIN).max(view_w);
        let mut widths = Vec::with_capacity(count as usize);
        let mut heights = Vec::with_capacity(count as usize);
        let mut tops = Vec::with_capacity(count as usize);
        let mut lefts = Vec::with_capacity(count as usize);
        let mut y = MARGIN;
        for i in 0..count {
            let s = size_of(i);
            let (w, h) = (s.width * k, s.height * k);
            widths.push(w);
            heights.push(h);
            tops.push(y);
            lefts.push((total_w - w) / 2.0);
            y += h + GAP;
        }
        let total_h = (y - GAP + MARGIN).max(view_h);
        Self {
            zoom,
            view_w,
            view_h,
            sizes_known,
            k,
            widths,
            heights,
            tops,
            lefts,
            total_w,
            total_h,
        }
    }

    /// Page whose vertical span contains `y` (or the last one that starts above it).
    fn page_at(&self, y: f32) -> u32 {
        self.tops.partition_point(|&top| top <= y).saturating_sub(1) as u32
    }

    fn visible(&self, top: f32, bottom: f32) -> std::ops::RangeInclusive<u32> {
        let last = self.tops.len().saturating_sub(1) as u32;
        self.page_at(top).min(last)..=self.page_at(bottom).min(last)
    }

    fn zoom_percent(&self) -> f32 {
        self.k / PX_PER_PT_AT_100 * 100.0
    }
}

type TileKey = (u32, Job);

struct Doc {
    handle: RenderHandle,
    sizes: Option<Arc<Vec<PageSize>>>,
    zoom: Zoom,
    scroll: ScrollHandle,
    layout: Option<Layout>,
    previews: HashMap<u32, Arc<RenderImage>>,
    tiles: HashMap<TileKey, Arc<RenderImage>>,
    /// Dropping the task drops the oneshot receiver, which tells the worker
    /// to skip the render if it hasn't started.
    inflight: HashMap<TileKey, Task<()>>,
    /// Images we've stopped showing; GPU textures are released in `render`,
    /// the only place we have a `Window`.
    retired: Vec<Arc<RenderImage>>,
    current_page: u32,
    pending_scroll: Option<ScrollTo>,
    first_paint: Option<Instant>,
    // Sidebar state.
    thumb_scroll: ScrollHandle,
    /// Pages whose previews the thumbnail panel is showing; kept cached.
    thumb_keep: Option<std::ops::RangeInclusive<u32>>,
    /// Last page the thumbnail panel auto-scrolled to follow.
    thumb_followed: Option<u32>,
    outline: Option<Arc<Vec<OutlineItem>>>,
    outline_expanded: HashSet<u32>,
    outline_scroll: ScrollHandle,
}

#[derive(Clone, Copy)]
enum ScrollTo {
    Page(u32),
    By(f32),
    End,
    /// Keep the document point (`page`, `fx`, `fy` as fractions of the page)
    /// under viewport-local point (`vx`, `vy`). Used to anchor zooms.
    Anchor {
        page: u32,
        fx: f32,
        fy: f32,
        vx: f32,
        vy: f32,
    },
}

pub struct DocView {
    focus_handle: FocusHandle,
    path: PathBuf,
    timing: bool,
    launched: Instant,
    state: State,
}

impl DocView {
    pub fn new(path: PathBuf, timing: bool, launched: Instant, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            path,
            timing,
            launched,
            state: State::Loading,
        };
        this.open(cx);
        this
    }

    pub fn title(&self) -> SharedString {
        self.path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
            .to_string()
            .into()
    }

    fn open(&mut self, cx: &mut Context<Self>) {
        let path = self.path.clone();
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
                            zoom: Zoom::FitWidth,
                            scroll: ScrollHandle::new(),
                            layout: None,
                            previews: HashMap::new(),
                            tiles: HashMap::new(),
                            inflight: HashMap::new(),
                            retired: Vec::new(),
                            current_page: 0,
                            pending_scroll: None,
                            first_paint: None,
                            thumb_scroll: ScrollHandle::new(),
                            thumb_keep: None,
                            thumb_followed: None,
                            outline: None,
                            outline_expanded: HashSet::new(),
                            outline_scroll: ScrollHandle::new(),
                        }));
                        viewer.fetch_page_sizes(cx);
                        viewer.fetch_outline(cx);
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

    fn fetch_outline(&mut self, cx: &mut Context<Self>) {
        let State::Ready(doc) = &self.state else {
            return;
        };
        let rx = doc.handle.outline();
        let timing = self.timing;
        let t = Instant::now();
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.await else { return };
            this.update(cx, |viewer, cx| {
                let State::Ready(doc) = &mut viewer.state else {
                    return;
                };
                match result {
                    Ok(items) => {
                        if timing {
                            eprintln!(
                                "[timing] outline: {} items in {:?}",
                                items.len(),
                                t.elapsed()
                            );
                        }
                        // Start with the top level expanded.
                        doc.outline_expanded = items
                            .iter()
                            .filter(|i| i.depth == 0)
                            .map(|i| i.id)
                            .collect();
                        doc.outline = Some(Arc::new(items));
                        cx.notify();
                    }
                    Err(e) => log::error!("outline: {e:#}"),
                }
            })
            .ok();
        })
        .detach();
    }

    pub fn go_to_page(&mut self, page: u32, cx: &mut Context<Self>) {
        self.scroll(ScrollTo::Page(page), cx);
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

    /// Change zoom, keeping the document point under `anchor` (viewport-local
    /// logical px; defaults to the viewport centre) where it is.
    fn set_zoom(&mut self, zoom: Zoom, anchor: Option<(f32, f32)>, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        if doc.zoom == zoom {
            return;
        }
        if let Some(layout) = &doc.layout {
            let (vx, vy) = anchor.unwrap_or((layout.view_w / 2.0, layout.view_h / 2.0));
            let sx = -f32::from(doc.scroll.offset().x);
            let sy = -f32::from(doc.scroll.offset().y);
            let (dx, dy) = (sx + vx, sy + vy);
            let page = layout.page_at(dy);
            let p = page as usize;
            let fx = (dx - layout.lefts[p]) / layout.widths[p];
            let fy = (dy - layout.tops[p]) / layout.heights[p];
            doc.pending_scroll = Some(ScrollTo::Anchor {
                page,
                fx,
                fy,
                vx,
                vy,
            });
        }
        doc.zoom = zoom;
        cx.notify();
    }

    fn zoom_by(&mut self, factor: f32, anchor: Option<(f32, f32)>, cx: &mut Context<Self>) {
        let State::Ready(doc) = &self.state else {
            return;
        };
        let current = doc
            .layout
            .as_ref()
            .map(|l| l.zoom_percent() / 100.0)
            .unwrap_or(1.0);
        let z = (current * factor).clamp(ZOOM_MIN, ZOOM_MAX);
        self.set_zoom(Zoom::Custom(z), anchor, cx);
    }

    fn on_wheel(&mut self, ev: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !ev.modifiers.control {
            return;
        }
        let State::Ready(doc) = &self.state else {
            return;
        };
        let dy = f32::from(ev.delta.pixel_delta(window.line_height()).y);
        if dy == 0.0 {
            return;
        }
        // Anchor on the mouse: position relative to the scroll container.
        let b = doc.scroll.bounds();
        let anchor = (
            f32::from(ev.position.x - b.origin.x),
            f32::from(ev.position.y - b.origin.y),
        );
        let factor = if dy > 0.0 { 1.1 } else { 1.0 / 1.1 };
        self.zoom_by(factor, Some(anchor), cx);
        cx.stop_propagation();
    }

    fn ensure_rendered(&mut self, want: RenderRequest, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let key = (want.page.0, want.job);
        if doc.tiles.contains_key(&key) || doc.inflight.contains_key(&key) {
            return;
        }
        if let Job::Page { .. } = want.job
            && doc.previews.contains_key(&want.page.0)
        {
            return;
        }
        let rx = doc.handle.render(want);
        let task = cx.spawn(async move |this, cx| {
            let result = rx.await;
            this.update(cx, |viewer, cx| viewer.on_rendered(want, result, cx))
                .ok();
        });
        doc.inflight.insert(key, task);
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
        let key = (want.page.0, want.job);
        // We're running inside this task; detach rather than drop it.
        if let Some(task) = doc.inflight.remove(&key) {
            task.detach();
        }
        match result {
            Err(_canceled) => {} // superseded or scrolled away
            Ok(Err(e)) => log::error!("render {:?}: {e:#}", want),
            Ok(Ok(rendered)) => {
                if timing {
                    let kind = match want.job {
                        Job::Page { .. } => "preview",
                        Job::Tile { .. } => "tile",
                    };
                    eprintln!(
                        "[timing] page {} {kind} {}x{} rendered in {:?}",
                        want.page.0,
                        rendered.raster.width,
                        rendered.raster.height,
                        rendered.render_time
                    );
                    if doc.first_paint.is_none() {
                        eprintln!(
                            "[timing] first pixels ready {:?} since launch",
                            launched.elapsed()
                        );
                    }
                }
                doc.first_paint.get_or_insert_with(Instant::now);
                let image = Arc::new(to_render_image(rendered.raster));
                let old = match want.job {
                    Job::Page { .. } => doc.previews.insert(want.page.0, image),
                    Job::Tile { .. } => doc.tiles.insert(key, image),
                };
                if let Some(old) = old {
                    doc.retired.push(old);
                }
                cx.notify();
            }
        }
    }

    pub fn current_page(&self) -> u32 {
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

impl Focusable for DocView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// Everything `render` decides for one page: where it goes and which tiles
/// cover the visible part of it.
struct PagePlan {
    page: u32,
    left: f32,
    top: f32,
    w: f32,
    h: f32,
    dev_w: u32,
    /// Tiles intersecting the (overdrawn) viewport, with priority.
    tiles: Vec<(Job, u8)>,
}

impl Render for DocView {
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
            State::Loading => "Opening…".into(),
            State::Failed(e) => format!("Failed: {e}").into(),
            State::Ready(doc) => {
                let info = doc.handle.info();
                let count = info.page_count;
                let last = count - 1;

                // The scroll container's bounds lag a frame behind on first
                // layout; fall back to the window size until we have them.
                let bounds = doc.scroll.bounds();
                let bounds_known = f32::from(bounds.size.width) > 0.0;
                let (view_w, view_h) = if bounds_known {
                    (f32::from(bounds.size.width), f32::from(bounds.size.height))
                } else {
                    (
                        f32::from(viewport.width),
                        f32::from(viewport.height) - STATUS_H,
                    )
                };
                let sizes_known = doc.sizes.is_some();
                let zoom = doc.zoom;

                let stale = doc.layout.as_ref().is_none_or(|l| {
                    l.zoom != zoom
                        || (l.view_w - view_w).abs() > 0.5
                        || (l.view_h - view_h).abs() > 0.5
                        || l.sizes_known != sizes_known
                });
                if stale {
                    let sizes = doc.sizes.clone();
                    let first = info.first_page;
                    let size_of = |i: u32| {
                        sizes
                            .as_ref()
                            .and_then(|s| s.get(i as usize).copied())
                            .unwrap_or(first)
                    };
                    doc.layout = Some(Layout::build(
                        zoom,
                        view_w,
                        view_h,
                        count,
                        &size_of,
                        sizes_known,
                    ));
                }
                let layout = doc.layout.as_ref().expect("built above");

                // Apply any queued scroll now that we know geometry.
                let max_x = (layout.total_w - view_w).max(0.0);
                let max_y = (layout.total_h - view_h).max(0.0);
                let mut scroll_x = (-f32::from(doc.scroll.offset().x)).clamp(0.0, max_x);
                let mut scroll_y = (-f32::from(doc.scroll.offset().y)).clamp(0.0, max_y);
                if let Some(to) = doc.pending_scroll.take() {
                    match to {
                        ScrollTo::Page(p) => scroll_y = layout.tops[p.min(last) as usize] - MARGIN,
                        ScrollTo::By(dy) => scroll_y += dy,
                        ScrollTo::End => scroll_y = max_y,
                        ScrollTo::Anchor {
                            page,
                            fx,
                            fy,
                            vx,
                            vy,
                        } => {
                            let p = page.min(last) as usize;
                            scroll_x = layout.lefts[p] + fx * layout.widths[p] - vx;
                            scroll_y = layout.tops[p] + fy * layout.heights[p] - vy;
                        }
                    }
                    scroll_x = scroll_x.clamp(0.0, max_x);
                    scroll_y = scroll_y.clamp(0.0, max_y);
                    doc.scroll.set_offset(point(px(-scroll_x), px(-scroll_y)));
                }

                let visible = layout.visible(scroll_y, scroll_y + view_h);
                doc.current_page = layout.page_at(scroll_y + view_h * 0.5).min(last);

                // Plan tiles for pages near the viewport.
                let overdraw = view_h * TILE_OVERDRAW;
                let fetch =
                    visible.start().saturating_sub(PREFETCH)..=(visible.end() + PREFETCH).min(last);
                let keep = visible.start().saturating_sub(KEEP)..=(visible.end() + KEEP).min(last);
                let mut plans: Vec<PagePlan> = Vec::new();
                for p in fetch.clone() {
                    let i = p as usize;
                    let (left, top, w, h) = (
                        layout.lefts[i],
                        layout.tops[i],
                        layout.widths[i],
                        layout.heights[i],
                    );
                    let dev_w = (w * scale).round().max(1.0) as u32;
                    let dev_h = (h * scale).round().max(1.0) as u32;
                    // Priorities: visible previews, visible tiles, then the
                    // same for prefetch pages — so an expensive off-screen
                    // preview never delays on-screen tiles.
                    let on_screen_page = visible.contains(&p);
                    wants.push(RenderRequest {
                        page: PageIndex(p),
                        job: Job::Page { width: PREVIEW_W },
                        format: PixelFormat::Bgra,
                        priority: if on_screen_page { 0 } else { 2 },
                    });
                    // Visible part of this page, in page-local device px.
                    let vis_x0 = ((scroll_x - left) * scale).max(0.0);
                    let vis_x1 = ((scroll_x + view_w - left) * scale).min(dev_w as f32);
                    let vis_y0 = ((scroll_y - overdraw - top) * scale).max(0.0);
                    let vis_y1 = ((scroll_y + view_h + overdraw - top) * scale).min(dev_h as f32);
                    let strict_y0 = ((scroll_y - top) * scale).max(0.0);
                    let strict_y1 = ((scroll_y + view_h - top) * scale).min(dev_h as f32);
                    let mut tiles = Vec::new();
                    // Until the container has been laid out once, the width
                    // is a guess; don't rasterize tiles at the wrong zoom.
                    if bounds_known && vis_x1 > vis_x0 && vis_y1 > vis_y0 {
                        let (c0, c1) = (
                            (vis_x0 / TILE as f32) as u32,
                            ((vis_x1 / TILE as f32).ceil() as u32).max(1),
                        );
                        let (r0, r1) = (
                            (vis_y0 / TILE as f32) as u32,
                            ((vis_y1 / TILE as f32).ceil() as u32).max(1),
                        );
                        for row in r0..r1 {
                            for col in c0..c1 {
                                let (x, y) = (col * TILE, row * TILE);
                                let (tw, th) = (TILE.min(dev_w - x), TILE.min(dev_h - y));
                                let on_screen =
                                    (y as f32) < strict_y1 && ((y + th) as f32) > strict_y0;
                                tiles.push((
                                    Job::Tile {
                                        page_width: dev_w,
                                        page_height: dev_h,
                                        x,
                                        y,
                                        w: tw,
                                        h: th,
                                    },
                                    if on_screen { 1 } else { 3 },
                                ));
                            }
                        }
                    }
                    for (job, priority) in &tiles {
                        wants.push(RenderRequest {
                            page: PageIndex(p),
                            job: *job,
                            format: PixelFormat::Bgra,
                            priority: *priority,
                        });
                    }
                    plans.push(PagePlan {
                        page: p,
                        left,
                        top,
                        w,
                        h,
                        dev_w,
                        tiles,
                    });
                }

                // Evict: anything outside the keep range; and, per page, tiles
                // at a stale zoom once every needed tile at the current zoom is in.
                let wanted: std::collections::HashSet<TileKey> = plans
                    .iter()
                    .flat_map(|pl| pl.tiles.iter().map(move |(j, _)| (pl.page, *j)))
                    .collect();
                let mut complete: HashMap<u32, bool> = HashMap::new();
                for pl in &plans {
                    let all = pl
                        .tiles
                        .iter()
                        .all(|(j, _)| doc.tiles.contains_key(&(pl.page, *j)));
                    complete.insert(pl.page, all);
                }
                let evict: Vec<TileKey> = doc
                    .tiles
                    .keys()
                    .copied()
                    .filter(|k @ (p, _)| {
                        !keep.contains(p)
                            || (!wanted.contains(k) && complete.get(p).copied().unwrap_or(true))
                    })
                    .collect();
                for k in evict {
                    if let Some(im) = doc.tiles.remove(&k) {
                        doc.retired.push(im);
                    }
                }
                let thumb_keep = doc.thumb_keep.clone();
                let evict: Vec<u32> = doc
                    .previews
                    .keys()
                    .copied()
                    .filter(|p| {
                        !keep.contains(p) && !thumb_keep.as_ref().is_some_and(|r| r.contains(p))
                    })
                    .collect();
                for p in evict {
                    if let Some(im) = doc.previews.remove(&p) {
                        doc.retired.push(im);
                    }
                }
                doc.inflight.retain(|k, _| {
                    wanted.contains(k)
                        || matches!(k.1, Job::Page { .. })
                            && (keep.contains(&k.0)
                                || thumb_keep.as_ref().is_some_and(|r| r.contains(&k.0)))
                });

                // Build elements: preview underneath, then any tiles we have
                // for the page (stale-zoom ones scaled to fit, current on top).
                let pages: Vec<_> = plans
                    .iter()
                    .map(|pl| {
                        let mut page_div = div()
                            .absolute()
                            .top(px(pl.top))
                            .left(px(pl.left))
                            .w(px(pl.w))
                            .h(px(pl.h))
                            .bg(rgb(0xffffff))
                            .shadow_md();
                        if let Some(prev) = doc.previews.get(&pl.page) {
                            page_div =
                                page_div.child(div().absolute().inset_0().child(
                                    img(prev.clone()).size_full().object_fit(ObjectFit::Fill),
                                ));
                        }
                        let mut page_tiles: Vec<(&TileKey, &Arc<RenderImage>)> = doc
                            .tiles
                            .iter()
                            .filter(|((p, _), _)| *p == pl.page)
                            .collect();
                        // Stale zoom levels first so current ones paint over them.
                        page_tiles.sort_by_key(|((_, j), _)| match j {
                            Job::Tile { page_width, .. } => *page_width == pl.dev_w,
                            _ => false,
                        });
                        for ((_, job), im) in page_tiles {
                            let Job::Tile {
                                page_width,
                                page_height,
                                x,
                                y,
                                w,
                                h,
                            } = job
                            else {
                                continue;
                            };
                            // Logical px per device px of the zoom this tile was rendered at.
                            let sx = pl.w / *page_width as f32;
                            let sy = pl.h / *page_height as f32;
                            page_div = page_div.child(
                                div()
                                    .absolute()
                                    .left(px(*x as f32 * sx))
                                    .top(px(*y as f32 * sy))
                                    .w(px(*w as f32 * sx))
                                    .h(px(*h as f32 * sy))
                                    .child(img(im.clone()).size_full().object_fit(ObjectFit::Fill)),
                            );
                        }
                        page_div
                    })
                    .collect();

                body = Some(
                    div()
                        .id("pages")
                        .track_scroll(&doc.scroll)
                        .overflow_scroll()
                        .size_full()
                        .on_scroll_wheel(
                            cx.listener(|v, ev, window, cx| v.on_wheel(ev, window, cx)),
                        )
                        .child(
                            div()
                                .relative()
                                .w(px(layout.total_w))
                                .h(px(layout.total_h))
                                .children(pages),
                        ),
                );

                let ps = Self::page_size(doc, doc.current_page);
                let mode = match zoom {
                    Zoom::FitWidth => "fit width",
                    Zoom::FitPage => "fit page",
                    Zoom::Custom(_) => "custom",
                };
                format!(
                    "{}  —  page {} / {}  —  {:.0}×{:.0} pt  —  {:.0}% ({mode})  —  {} tiles",
                    self.path.display(),
                    doc.current_page + 1,
                    count,
                    ps.width,
                    ps.height,
                    layout.zoom_percent(),
                    doc.tiles.len(),
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
            .on_action(cx.listener(|v, _: &ZoomIn, _, cx| v.zoom_by(ZOOM_STEP, None, cx)))
            .on_action(cx.listener(|v, _: &ZoomOut, _, cx| v.zoom_by(1.0 / ZOOM_STEP, None, cx)))
            .on_action(
                cx.listener(|v, _: &ZoomActual, _, cx| v.set_zoom(Zoom::Custom(1.0), None, cx)),
            )
            .on_action(
                cx.listener(|v, _: &ZoomFitWidth, _, cx| v.set_zoom(Zoom::FitWidth, None, cx)),
            )
            .on_action(cx.listener(|v, _: &ZoomFitPage, _, cx| v.set_zoom(Zoom::FitPage, None, cx)))
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

// ---------------------------------------------------------------------------
// Sidebar panels. Rendered on behalf of the workspace via `update`, so they
// can share this view's caches and render handle.

impl DocView {
    /// Thumbnail strip: virtualized, follows the current page.
    pub fn render_thumbnails(
        &mut self,
        panel_w: f32,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let State::Ready(doc) = &mut self.state else {
            return div().p_2().text_sm().child("…").into_any_element();
        };
        let info = doc.handle.info();
        let count = info.page_count;
        let last = count.saturating_sub(1);
        let thumb_w = (panel_w - 2.0 * THUMB_PAD).clamp(40.0, THUMB_W);

        // Item geometry.
        let sizes = doc.sizes.clone();
        let first = info.first_page;
        let size_of = |i: u32| {
            sizes
                .as_ref()
                .and_then(|s| s.get(i as usize).copied())
                .unwrap_or(first)
        };
        let item_h = |i: u32| {
            let s = size_of(i);
            thumb_w * s.height / s.width.max(1.0) + THUMB_LABEL_H + THUMB_PAD
        };
        // Cumulative tops; cheap enough per frame for the page counts we care about.
        let mut tops = Vec::with_capacity(count as usize);
        let mut y = THUMB_PAD;
        for i in 0..count {
            tops.push(y);
            y += item_h(i);
        }
        let total_h = y;

        let bounds = doc.thumb_scroll.bounds();
        let view_h = f32::from(bounds.size.height).max(1.0);
        let max_y = (total_h - view_h).max(0.0);
        let mut scroll_y = (-f32::from(doc.thumb_scroll.offset().y)).clamp(0.0, max_y);

        // Follow the main view: when the current page changes, make sure its
        // thumbnail is visible.
        let cur = doc.current_page;
        if doc.thumb_followed != Some(cur) && count > 0 {
            let top = tops[cur as usize];
            let bottom = top + item_h(cur);
            if top < scroll_y {
                scroll_y = (top - THUMB_PAD).clamp(0.0, max_y);
            } else if bottom > scroll_y + view_h {
                scroll_y = (bottom - view_h + THUMB_PAD).clamp(0.0, max_y);
            }
            doc.thumb_scroll.set_offset(point(px(0.0), px(-scroll_y)));
            doc.thumb_followed = Some(cur);
        }

        let page_at = |y: f32| tops.partition_point(|&t| t <= y).saturating_sub(1) as u32;
        let (v0, v1) = (
            page_at(scroll_y).min(last),
            page_at(scroll_y + view_h).min(last),
        );
        let keep = v0.saturating_sub(2)..=(v1 + 2).min(last);
        doc.thumb_keep = Some(keep.clone());

        let mut wants = Vec::new();
        let mut items = Vec::new();
        for p in v0..=v1 {
            wants.push(RenderRequest {
                page: PageIndex(p),
                job: Job::Page { width: PREVIEW_W },
                format: PixelFormat::Bgra,
                priority: 2,
            });
            let s = size_of(p);
            let h = thumb_w * s.height / s.width.max(1.0);
            let image = doc.previews.get(&p).cloned();
            let selected = p == cur;
            items.push(
                div()
                    .id(("thumb", p as usize))
                    .absolute()
                    .top(px(tops[p as usize]))
                    .left(px(THUMB_PAD))
                    .w(px(thumb_w))
                    .flex()
                    .flex_col()
                    .items_center()
                    .cursor_pointer()
                    .on_click(cx.listener(move |v, _, _, cx| v.go_to_page(p, cx)))
                    .child(
                        div()
                            .w(px(thumb_w))
                            .h(px(h))
                            .bg(rgb(0xffffff))
                            .border_2()
                            .border_color(if selected {
                                rgb(0x4a9eff)
                            } else {
                                rgb(0x3a3a3a)
                            })
                            .when_some(image, |d, im| {
                                d.child(img(im).size_full().object_fit(ObjectFit::Fill))
                            }),
                    )
                    .child(
                        div()
                            .h(px(THUMB_LABEL_H))
                            .text_xs()
                            .text_color(if selected {
                                rgb(0xffffff)
                            } else {
                                rgb(0x9a9a9a)
                            })
                            .child(format!("{}", p + 1)),
                    ),
            );
        }
        let el = div()
            .id("thumbs")
            .track_scroll(&doc.thumb_scroll)
            .overflow_y_scroll()
            .size_full()
            .child(div().relative().w_full().h(px(total_h)).children(items))
            .into_any_element();
        for want in wants {
            self.ensure_rendered(want, cx);
        }
        el
    }

    /// Bookmark tree: virtualized rows over the expanded subset.
    pub fn render_outline(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let State::Ready(doc) = &mut self.state else {
            return div().p_2().text_sm().child("…").into_any_element();
        };
        let Some(outline) = doc.outline.clone() else {
            return div()
                .p_2()
                .text_sm()
                .text_color(rgb(0x9a9a9a))
                .child("Loading bookmarks…")
                .into_any_element();
        };
        if outline.is_empty() {
            return div()
                .p_2()
                .text_sm()
                .text_color(rgb(0x9a9a9a))
                .child("No bookmarks")
                .into_any_element();
        }

        // Flatten to the visible rows: skip descendants of collapsed nodes.
        let mut rows: Vec<usize> = Vec::new();
        let mut hide_below: Option<u16> = None;
        for (ix, item) in outline.iter().enumerate() {
            if let Some(d) = hide_below {
                if item.depth > d {
                    continue;
                }
                hide_below = None;
            }
            rows.push(ix);
            if item.has_children && !doc.outline_expanded.contains(&item.id) {
                hide_below = Some(item.depth);
            }
        }
        let total_h = rows.len() as f32 * OUTLINE_ROW_H + THUMB_PAD;
        let bounds = doc.outline_scroll.bounds();
        let view_h = f32::from(bounds.size.height).max(1.0);
        let scroll_y = (-f32::from(doc.outline_scroll.offset().y)).max(0.0);
        let r0 = ((scroll_y / OUTLINE_ROW_H) as usize).min(rows.len());
        let r1 = (((scroll_y + view_h) / OUTLINE_ROW_H) as usize + 1).min(rows.len());

        let cur = doc.current_page;
        let mut els = Vec::new();
        for r in r0..r1 {
            let item = &outline[rows[r]];
            let id = item.id;
            let page = item.page;
            let expanded = doc.outline_expanded.contains(&id);
            let active = page == Some(cur);
            let chevron = if !item.has_children {
                " "
            } else if expanded {
                "▾"
            } else {
                "▸"
            };
            els.push(
                div()
                    .id(("outline", id as usize))
                    .absolute()
                    .top(px(r as f32 * OUTLINE_ROW_H))
                    .left(px(0.0))
                    .w_full()
                    .h(px(OUTLINE_ROW_H))
                    .pl(px(6.0 + item.depth as f32 * OUTLINE_INDENT))
                    .pr_2()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_sm()
                    .cursor_pointer()
                    .when(active, |d| d.bg(rgb(0x3a4a60)))
                    .hover(|s| s.bg(rgb(0x3a3a3a)))
                    .on_click(cx.listener(move |v, _, _, cx| {
                        if let Some(p) = page {
                            v.go_to_page(p, cx);
                        }
                    }))
                    .child(
                        div()
                            .id(("outline-toggle", id as usize))
                            .w(px(14.0))
                            .text_color(rgb(0x9a9a9a))
                            .cursor_pointer()
                            .on_click(cx.listener(move |v, _, _, cx| {
                                if let State::Ready(doc) = &mut v.state {
                                    if !doc.outline_expanded.remove(&id) {
                                        doc.outline_expanded.insert(id);
                                    }
                                    cx.notify();
                                }
                                cx.stop_propagation();
                            }))
                            .child(chevron),
                    )
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_color(if page.is_some() {
                                rgb(0xd0d0d0)
                            } else {
                                rgb(0x8a8a8a)
                            })
                            .child(item.title.clone()),
                    ),
            );
        }
        div()
            .id("outline")
            .track_scroll(&doc.outline_scroll)
            .overflow_y_scroll()
            .size_full()
            .child(div().relative().w_full().h(px(total_h)).children(els))
            .into_any_element()
    }
}
