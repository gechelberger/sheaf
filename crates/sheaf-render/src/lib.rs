//! Rasterization.
//!
//! Two layers:
//! - [`Renderer`]: a synchronous engine interface. [`pdfium::PdfiumEngine`] is
//!   the only implementation for now.
//! - [`RenderHandle`]: a dedicated worker thread that owns an engine and
//!   services render requests over a channel, so the UI thread never blocks on
//!   PDFium. Replies come back on `oneshot` channels the caller can `.await`.
//!
//! PDFium is process-global and (with pdfium-render's `thread_safe` feature)
//! serializes every FFI call behind a mutex, so one worker thread is the right
//! amount of parallelism; more threads would just queue on that lock.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Instant;

use anyhow::{Context, Result};
use futures::channel::oneshot;
use sheaf_core::{PageIndex, PageSize, Rotation};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PixelFormat {
    /// Straight RGBA8, what the `image` crate and most encoders want.
    Rgba,
    /// BGRA8 — PDFium's native output and what GPUI uploads without conversion.
    Bgra,
}

/// A rendered page or tile: tightly packed 4 bytes/pixel, row-major.
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub data: Vec<u8>,
}

/// What to rasterize. Sizes are device pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Job {
    /// The whole page, scaled to `width` wide (after `rotation`).
    Page { width: u32, rotation: Rotation },
    /// The `w`×`h` region at (`x`, `y`) of the page scaled to
    /// `page_width`×`page_height` (after `rotation`). Lets a zoomed-in page
    /// be rendered a viewport's worth at a time instead of as one huge bitmap.
    Tile {
        page_width: u32,
        page_height: u32,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        rotation: Rotation,
    },
}

impl Job {
    pub fn rotation(&self) -> Rotation {
        match self {
            Job::Page { rotation, .. } | Job::Tile { rotation, .. } => *rotation,
        }
    }
}

/// One bookmark, in pre-order with its depth, so a tree can be stored flat.
#[derive(Debug, Clone)]
pub struct OutlineItem {
    pub id: u32,
    pub title: String,
    pub page: Option<u32>,
    pub depth: u16,
    pub has_children: bool,
}

/// Axis-aligned rectangle in page points, top-left origin, in the page's
/// *displayed* orientation (i.e. after /Rotate).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// One search match: the page it's on and the glyph boxes that make it up
/// (usually one, more if the match wraps a line).
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub page: u32,
    pub rects: Vec<Rect>,
    /// The matched text.
    pub text: String,
}

/// Progress of a document-wide search, streamed as pages are scanned.
#[derive(Debug)]
pub enum SearchEvent {
    /// Hits found on one page (only sent when non-empty).
    Hits(Vec<SearchHit>),
    /// Pages scanned so far.
    Progress(u32),
    Done,
}

/// Synchronous engine interface. Implementations may block.
pub trait Renderer {
    fn page_count(&self) -> u32;
    fn page_size(&self, page: PageIndex) -> Result<PageSize>;
    fn page_sizes(&self) -> Result<Vec<PageSize>>;
    /// The bookmark tree, flattened in pre-order.
    fn outline(&self) -> Result<Vec<OutlineItem>>;
    /// All matches of `query` on one page.
    fn search_page(&self, page: PageIndex, query: &str, match_case: bool)
    -> Result<Vec<SearchHit>>;
    /// All matches of a regular expression on one page, run over the
    /// page's extracted text.
    fn pattern_search_page(&self, page: PageIndex, re: &regex::Regex) -> Result<Vec<SearchHit>>;
    /// Hide (from rendering) every annotation on `page` whose /NM starts
    /// with `sheaf:`; the app draws those itself. Returns how many.
    fn hide_sheaf_annots(&self, page: PageIndex) -> Result<usize>;
    /// Glyph boxes (merged per line) of text intersecting `rect`, in the
    /// page's displayed space. Used to turn a drag into a text highlight.
    fn text_boxes_in_rect(&self, page: PageIndex, rect: Rect) -> Result<Vec<Rect>>;
    /// Render a whole page scaled so its width is `target_width` device pixels.
    fn render(&self, page: PageIndex, job: Job, format: PixelFormat) -> Result<Raster>;

    /// Render a whole page scaled so its width is `target_width` device pixels.
    fn render_page(
        &self,
        page: PageIndex,
        target_width: u32,
        format: PixelFormat,
    ) -> Result<Raster> {
        self.render(
            page,
            Job::Page {
                width: target_width,
                rotation: Rotation::R0,
            },
            format,
        )
    }
}

pub mod pdfium {
    use super::*;
    use pdfium_render::prelude::*;
    use std::sync::OnceLock;

    /// Locate and bind libpdfium. Search order: `$PDFIUM_LIB_DIR`,
    /// `vendor/pdfium/<os>-<arch>/` relative to the cwd, then the system library.
    fn bind() -> Result<Pdfium> {
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Ok(d) = std::env::var("PDFIUM_LIB_DIR") {
            dirs.push(d.into());
        }
        dirs.push(
            format!(
                "vendor/pdfium/{}-{}",
                std::env::consts::OS,
                std::env::consts::ARCH
            )
            .into(),
        );

        let mut last_err = None;
        for d in &dirs {
            match Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(d)) {
                Ok(b) => return Ok(Pdfium::new(b)),
                Err(e) => last_err = Some(e),
            }
        }
        Pdfium::bind_to_system_library().map(Pdfium::new).with_context(|| {
            format!(
                "no libpdfium found (searched {dirs:?} and system); run scripts/fetch-pdfium.sh. last error: {last_err:?}"
            )
        })
    }

    /// The process-wide PDFium instance. PDFium itself is a global library
    /// (`FPDF_InitLibrary` once per process), so a `'static` handle is the
    /// honest model and sidesteps the `PdfDocument<'a>` borrow.
    pub fn pdfium() -> Result<&'static Pdfium> {
        static CELL: OnceLock<std::result::Result<Pdfium, String>> = OnceLock::new();
        match CELL.get_or_init(|| bind().map_err(|e| format!("{e:#}"))) {
            Ok(p) => Ok(p),
            Err(e) => anyhow::bail!("{e}"),
        }
    }

    pub struct PdfiumEngine {
        doc: PdfDocument<'static>,
    }

    impl PdfiumEngine {
        pub fn open(path: impl AsRef<Path>, password: Option<&str>) -> Result<Self> {
            let path = path.as_ref();
            // Read the whole file up front. PDFium otherwise pulls objects
            // lazily with many small random reads, which is fine on local
            // SSDs but disastrous on network/9P mounts (3.4 s vs 0.27 s to
            // enumerate 7k page sizes in one measurement). One sequential read
            // of even a few hundred MB is cheap by comparison, and it's what a
            // viewer wants anyway. Truly huge files can revisit this.
            let bytes =
                std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
            let doc = pdfium()?
                .load_pdf_from_byte_vec(bytes, password)
                .with_context(|| format!("pdfium loading {}", path.display()))?;
            Ok(Self { doc })
        }
    }

    impl Renderer for PdfiumEngine {
        fn page_count(&self) -> u32 {
            self.doc.pages().len() as u32
        }

        fn page_size(&self, page: PageIndex) -> Result<PageSize> {
            let r = self.doc.pages().page_size(page.0 as i32)?;
            Ok(PageSize {
                width: r.width().value,
                height: r.height().value,
            })
        }

        fn page_sizes(&self) -> Result<Vec<PageSize>> {
            // FPDF_GetPageSizeByIndexF reads the page dictionary without
            // loading page content: ~30µs/page, so ~200ms for 7k pages. Cheap
            // but not free, which is why DocInfo doesn't wait for it.
            Ok(self
                .doc
                .pages()
                .page_sizes()?
                .into_iter()
                .map(|r| PageSize {
                    width: r.width().value,
                    height: r.height().value,
                })
                .collect())
        }

        fn outline(&self) -> Result<Vec<OutlineItem>> {
            const MAX_DEPTH: u16 = 32;
            const MAX_ITEMS: usize = 200_000;

            fn page_of(b: &PdfBookmark<'_>) -> Option<u32> {
                if let Some(d) = b.destination()
                    && let Ok(i) = d.page_index()
                {
                    return Some(i as u32);
                }
                let action = b.action()?;
                let local = action.as_local_destination_action()?;
                local
                    .destination()
                    .ok()?
                    .page_index()
                    .ok()
                    .map(|i| i as u32)
            }

            fn walk(first: Option<PdfBookmark<'_>>, depth: u16, out: &mut Vec<OutlineItem>) {
                let mut node = first;
                while let Some(b) = node {
                    if out.len() >= MAX_ITEMS {
                        return;
                    }
                    let child = b.first_child();
                    out.push(OutlineItem {
                        id: out.len() as u32,
                        title: b.title().unwrap_or_default(),
                        page: page_of(&b),
                        depth,
                        has_children: child.is_some(),
                    });
                    if depth < MAX_DEPTH {
                        walk(child, depth + 1, out);
                    }
                    node = b.next_sibling();
                }
            }

            let mut out = Vec::new();
            walk(self.doc.bookmarks().root(), 0, &mut out);
            Ok(out)
        }

        fn search_page(
            &self,
            page: PageIndex,
            query: &str,
            match_case: bool,
        ) -> Result<Vec<SearchHit>> {
            if query.is_empty() {
                return Ok(Vec::new());
            }
            let p = self.doc.pages().get(page.0 as i32)?;
            let text = p.text()?;
            let opts = PdfSearchOptions::new().match_case(match_case);
            let search = text.search(query, &opts)?;

            // Map page-space boxes to the displayed orientation via
            // FPDF_PageToDevice, with a scaled "device" so the integer
            // output keeps sub-point precision.
            const SCALE: f32 = 16.0;
            let (pw, ph) = (p.width().value, p.height().value);
            let cfg =
                PdfRenderConfig::new().set_fixed_size((pw * SCALE) as i32, (ph * SCALE) as i32);
            let to_display = |x: f32, y: f32| -> Option<(f32, f32)> {
                let (dx, dy) = p
                    .points_to_pixels(PdfPoints::new(x), PdfPoints::new(y), &cfg)
                    .ok()?;
                Some((dx as f32 / SCALE, dy as f32 / SCALE))
            };

            let mut hits = Vec::new();
            for segments in search.iter(PdfSearchDirection::SearchForward) {
                let mut rects = Vec::new();
                let mut text = String::new();
                for seg in segments.iter() {
                    text.push_str(&seg.text());
                    let b = seg.bounds();
                    let (Some((x0, y0)), Some((x1, y1))) = (
                        to_display(b.left().value, b.bottom().value),
                        to_display(b.right().value, b.top().value),
                    ) else {
                        continue;
                    };
                    rects.push(Rect {
                        x: x0.min(x1),
                        y: y0.min(y1),
                        w: (x1 - x0).abs(),
                        h: (y1 - y0).abs(),
                    });
                }
                if !rects.is_empty() {
                    hits.push(SearchHit {
                        page: page.0,
                        rects,
                        text,
                    });
                }
            }
            Ok(hits)
        }

        fn text_boxes_in_rect(&self, page: PageIndex, rect: Rect) -> Result<Vec<Rect>> {
            let p = self.doc.pages().get(page.0 as i32)?;
            let text = p.text()?;
            const SCALE: f32 = 16.0;
            let (pw, ph) = (p.width().value, p.height().value);
            let cfg =
                PdfRenderConfig::new().set_fixed_size((pw * SCALE) as i32, (ph * SCALE) as i32);
            let to_display = |x: f32, y: f32| -> Option<(f32, f32)> {
                let (dx, dy) = p
                    .points_to_pixels(PdfPoints::new(x), PdfPoints::new(y), &cfg)
                    .ok()?;
                Some((dx as f32 / SCALE, dy as f32 / SCALE))
            };
            let (rx0, ry0, rx1, ry1) = (rect.x, rect.y, rect.x + rect.w, rect.y + rect.h);
            let mut rects: Vec<Rect> = Vec::new();
            for c in text.chars().iter() {
                if c.is_generated().unwrap_or(false) {
                    continue;
                }
                let Ok(b) = c.loose_bounds() else { continue };
                let (Some((x0, y0)), Some((x1, y1))) = (
                    to_display(b.left().value, b.bottom().value),
                    to_display(b.right().value, b.top().value),
                ) else {
                    continue;
                };
                let r = Rect {
                    x: x0.min(x1),
                    y: y0.min(y1),
                    w: (x1 - x0).abs(),
                    h: (y1 - y0).abs(),
                };
                // Keep glyphs whose centre is inside the drag rectangle.
                let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
                if cx < rx0 || cx > rx1 || cy < ry0 || cy > ry1 {
                    continue;
                }
                match rects.last_mut() {
                    Some(prev)
                        if (prev.y - r.y).abs() < prev.h.max(r.h) * 0.5 && r.x >= prev.x - 1.0 =>
                    {
                        let right = (prev.x + prev.w).max(r.x + r.w);
                        let bottom = (prev.y + prev.h).max(r.y + r.h);
                        prev.x = prev.x.min(r.x);
                        prev.y = prev.y.min(r.y);
                        prev.w = right - prev.x;
                        prev.h = bottom - prev.y;
                    }
                    _ => rects.push(r),
                }
            }
            Ok(rects)
        }

        fn hide_sheaf_annots(&self, page: PageIndex) -> Result<usize> {
            let mut p = self.doc.pages().get(page.0 as i32)?;
            let annots = p.annotations_mut();
            let mut n = 0;
            for i in 0..annots.len() {
                if let Ok(mut a) = annots.get(i)
                    && a.name().is_some_and(|name| name.starts_with("sheaf:"))
                    && a.set_is_hidden(true).is_ok()
                {
                    n += 1;
                }
            }
            Ok(n)
        }

        fn pattern_search_page(
            &self,
            page: PageIndex,
            re: &regex::Regex,
        ) -> Result<Vec<SearchHit>> {
            let p = self.doc.pages().get(page.0 as i32)?;
            let text = p.text()?;
            let chars = text.chars();
            if chars.is_empty() {
                return Ok(Vec::new());
            }

            // Build the page string and remember each char's byte offset and
            // box, so regex byte ranges map back to glyphs. Generated chars
            // (synthesized spaces/newlines) still count — "\b" needs them —
            // but carry no box so they never draw.
            let mut s = String::with_capacity(chars.len());
            let mut starts: Vec<usize> = Vec::with_capacity(chars.len());
            let mut boxes: Vec<Option<PdfRect>> = Vec::with_capacity(chars.len());
            for c in chars.iter() {
                let ch = c.unicode_char().unwrap_or(' ');
                starts.push(s.len());
                s.push(ch);
                let generated = c.is_generated().unwrap_or(false);
                boxes.push(if generated {
                    None
                } else {
                    c.loose_bounds().ok()
                });
            }

            const SCALE: f32 = 16.0;
            let (pw, ph) = (p.width().value, p.height().value);
            let cfg =
                PdfRenderConfig::new().set_fixed_size((pw * SCALE) as i32, (ph * SCALE) as i32);
            let to_display = |x: f32, y: f32| -> Option<(f32, f32)> {
                let (dx, dy) = p
                    .points_to_pixels(PdfPoints::new(x), PdfPoints::new(y), &cfg)
                    .ok()?;
                Some((dx as f32 / SCALE, dy as f32 / SCALE))
            };
            let display_rect = |b: &PdfRect| -> Option<Rect> {
                let (x0, y0) = to_display(b.left().value, b.bottom().value)?;
                let (x1, y1) = to_display(b.right().value, b.top().value)?;
                Some(Rect {
                    x: x0.min(x1),
                    y: y0.min(y1),
                    w: (x1 - x0).abs(),
                    h: (y1 - y0).abs(),
                })
            };

            let mut hits = Vec::new();
            for m in re.find_iter(&s) {
                if m.is_empty() {
                    continue;
                }
                let first = starts.partition_point(|&b| b < m.start());
                let last = starts.partition_point(|&b| b < m.end());
                // Merge glyph boxes into one rect per line.
                let mut rects: Vec<Rect> = Vec::new();
                for b in boxes[first..last].iter().flatten() {
                    let Some(r) = display_rect(b) else { continue };
                    match rects.last_mut() {
                        Some(prev)
                            if (prev.y - r.y).abs() < prev.h.max(r.h) * 0.5
                                && r.x >= prev.x - 1.0 =>
                        {
                            let right = (prev.x + prev.w).max(r.x + r.w);
                            let bottom = (prev.y + prev.h).max(r.y + r.h);
                            prev.x = prev.x.min(r.x);
                            prev.y = prev.y.min(r.y);
                            prev.w = right - prev.x;
                            prev.h = bottom - prev.y;
                        }
                        _ => rects.push(r),
                    }
                }
                if !rects.is_empty() {
                    hits.push(SearchHit {
                        page: page.0,
                        rects,
                        text: m.as_str().to_string(),
                    });
                }
            }
            Ok(hits)
        }

        fn render(&self, page: PageIndex, job: Job, format: PixelFormat) -> Result<Raster> {
            let p = self.doc.pages().get(page.0 as i32)?;
            let rot = match job.rotation() {
                Rotation::R0 => PdfPageRenderRotation::None,
                Rotation::R90 => PdfPageRenderRotation::Degrees90,
                Rotation::R180 => PdfPageRenderRotation::Degrees180,
                Rotation::R270 => PdfPageRenderRotation::Degrees270,
            };
            let bmp = match job {
                Job::Page { width, rotation } => {
                    // Output dims are post-rotation; PDFium's size_x/size_y are
                    // the rotated frame, so fixed size + rotate is consistent.
                    let src = PageSize {
                        width: p.width().value,
                        height: p.height().value,
                    }
                    .rotated(rotation);
                    let w = width.max(1);
                    let h = ((w as f32) * src.height / src.width.max(1.0))
                        .round()
                        .max(1.0) as u32;
                    let cfg = PdfRenderConfig::new()
                        .set_fixed_size(w as i32, h as i32)
                        .rotate(rot, false)
                        .set_format(PdfBitmapFormat::BGRA)
                        .render_form_data(true);
                    p.render_with_config(&cfg)?
                }
                Job::Tile {
                    page_width,
                    page_height,
                    x,
                    y,
                    w,
                    h,
                    ..
                } => {
                    // Scale the page to its full size but draw into a
                    // tile-sized bitmap offset so only our region lands.
                    // Unlike a matrix/clip render, this path keeps form-field
                    // drawing (FPDF_FFLDraw) working.
                    let mut bmp =
                        PdfBitmap::empty(w.max(1) as i32, h.max(1) as i32, PdfBitmapFormat::BGRA)?;
                    let cfg = PdfRenderConfig::new()
                        .set_fixed_size(page_width.max(1) as i32, page_height.max(1) as i32)
                        .set_origin(-(x as i32), -(y as i32))
                        .rotate(rot, false)
                        .set_format(PdfBitmapFormat::BGRA)
                        .render_form_data(true);
                    p.render_into_bitmap_with_config(&mut bmp, &cfg)?;
                    bmp
                }
            };
            let data = match format {
                PixelFormat::Bgra => bmp.as_raw_bytes(),
                PixelFormat::Rgba => bmp.as_rgba_bytes(),
            };
            Ok(Raster {
                width: bmp.width() as u32,
                height: bmp.height() as u32,
                format,
                data,
            })
        }
    }
}

/// What the worker learned about the document at open time. Deliberately
/// O(1) in page count so first paint doesn't wait on a 7,000-page walk; the
/// full size table comes later via [`RenderHandle::page_sizes`].
#[derive(Debug)]
pub struct DocInfo {
    pub page_count: u32,
    /// Size of page 0, used as the layout estimate until all sizes arrive.
    pub first_page: PageSize,
    /// Wall time to read, parse, and size the first page.
    pub open_time: std::time::Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RenderRequest {
    pub page: PageIndex,
    pub job: Job,
    pub format: PixelFormat,
    /// Lower runs sooner within a batch (0 = previews, then visible tiles,
    /// then prefetch).
    pub priority: u8,
}

/// A finished render plus how long the engine took.
pub struct Rendered {
    pub request: RenderRequest,
    pub raster: Raster,
    pub render_time: std::time::Duration,
}

/// What a document-wide search looks for.
#[derive(Clone)]
pub enum Matcher {
    Literal { query: String, match_case: bool },
    Pattern(std::sync::Arc<regex::Regex>),
}

enum Msg {
    Render(RenderRequest, oneshot::Sender<Result<Rendered>>),
    PageSizes(oneshot::Sender<Result<Vec<PageSize>>>),
    Outline(oneshot::Sender<Result<Vec<OutlineItem>>>),
    TextBoxes {
        page: u32,
        rect: Rect,
        reply: oneshot::Sender<Result<Vec<Rect>>>,
    },
    HideAnnots {
        pages: Vec<u32>,
        reply: oneshot::Sender<Result<usize>>,
    },
    Search {
        matcher: Matcher,
        tx: futures::channel::mpsc::UnboundedSender<SearchEvent>,
    },
}

/// Handle to a render worker thread. Cheap to clone; the thread exits when the
/// last handle is dropped.
#[derive(Clone)]
pub struct RenderHandle {
    tx: mpsc::Sender<Msg>,
    info: Arc<DocInfo>,
}

impl RenderHandle {
    /// Spawn a worker, open `path` on it, and block until the open finishes.
    /// Call from a background task if you don't want to block the UI.
    pub fn open(path: PathBuf, password: Option<String>) -> Result<Self> {
        let (tx, rx) = mpsc::channel::<Msg>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<DocInfo>>();

        std::thread::Builder::new()
            .name("sheaf-render".into())
            .spawn(move || {
                let t = Instant::now();
                let engine = match pdfium::PdfiumEngine::open(&path, password.as_deref()) {
                    Ok(e) => e,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                let info = engine.page_size(PageIndex(0)).map(|first_page| DocInfo {
                    page_count: engine.page_count(),
                    first_page,
                    open_time: t.elapsed(),
                });
                let failed = info.is_err();
                let _ = ready_tx.send(info);
                if failed {
                    return;
                }
                worker_loop(engine, rx);
            })
            .context("spawning render thread")?;

        let info = ready_rx
            .recv()
            .context("render thread died before opening")??;
        Ok(Self {
            tx,
            info: Arc::new(info),
        })
    }

    pub fn info(&self) -> &DocInfo {
        &self.info
    }

    /// Queue a render. The receiver resolves with `Err(Canceled)` if the
    /// request was superseded by a newer one for the same page before it ran.
    /// Dropping the receiver before the worker gets to it skips the render.
    pub fn render(&self, request: RenderRequest) -> oneshot::Receiver<Result<Rendered>> {
        let (tx, rx) = oneshot::channel();
        self.send(Msg::Render(request, tx));
        rx
    }

    /// Fetch the size of every page. Runs on the worker between renders.
    pub fn page_sizes(&self) -> oneshot::Receiver<Result<Vec<PageSize>>> {
        let (tx, rx) = oneshot::channel();
        self.send(Msg::PageSizes(tx));
        rx
    }

    /// Hide sheaf-written annotations on these pages so they aren't drawn
    /// twice (the app overlays its own). Resolves with the count hidden.
    pub fn hide_sheaf_annots(&self, pages: Vec<u32>) -> oneshot::Receiver<Result<usize>> {
        let (tx, rx) = oneshot::channel();
        self.send(Msg::HideAnnots { pages, reply: tx });
        rx
    }

    /// Glyph boxes of text under a drag rectangle (displayed page space).
    pub fn text_boxes_in_rect(
        &self,
        page: u32,
        rect: Rect,
    ) -> oneshot::Receiver<Result<Vec<Rect>>> {
        let (tx, rx) = oneshot::channel();
        self.send(Msg::TextBoxes {
            page,
            rect,
            reply: tx,
        });
        rx
    }

    /// Fetch the bookmark tree.
    pub fn outline(&self) -> oneshot::Receiver<Result<Vec<OutlineItem>>> {
        let (tx, rx) = oneshot::channel();
        self.send(Msg::Outline(tx));
        rx
    }

    /// Start a document-wide search. Events stream until `Done`; dropping
    /// the receiver cancels the search. A new search replaces a running one.
    pub fn search(
        &self,
        query: String,
        match_case: bool,
    ) -> futures::channel::mpsc::UnboundedReceiver<SearchEvent> {
        self.search_with(Matcher::Literal { query, match_case })
    }

    /// Start a regex search over every page's text.
    pub fn pattern_search(
        &self,
        re: std::sync::Arc<regex::Regex>,
    ) -> futures::channel::mpsc::UnboundedReceiver<SearchEvent> {
        self.search_with(Matcher::Pattern(re))
    }

    fn search_with(
        &self,
        matcher: Matcher,
    ) -> futures::channel::mpsc::UnboundedReceiver<SearchEvent> {
        let (tx, rx) = futures::channel::mpsc::unbounded();
        self.send(Msg::Search { matcher, tx });
        rx
    }

    fn send(&self, msg: Msg) {
        if self.tx.send(msg).is_err() {
            log::error!("render thread is gone");
        }
    }
}

/// Pages sized per chunk before checking the channel for render requests.
const SIZE_CHUNK: u32 = 64;

/// A page-size walk in progress, resumed between renders so a 7,000-page
/// enumeration never delays a visible page.
struct SizeWalk {
    reply: oneshot::Sender<Result<Vec<PageSize>>>,
    sizes: Vec<PageSize>,
}

/// Pages searched per chunk before checking for render requests.
const SEARCH_CHUNK: u32 = 4;

/// A document search in progress, likewise resumed between renders.
struct SearchWalk {
    matcher: Matcher,
    next: u32,
    tx: futures::channel::mpsc::UnboundedSender<SearchEvent>,
}

fn worker_loop(engine: impl Renderer, rx: mpsc::Receiver<Msg>) {
    let page_count = engine.page_count();
    let mut walk: Option<SizeWalk> = None;
    let mut search: Option<SearchWalk> = None;

    loop {
        // Block only when there's no background work to resume.
        let first = if walk.is_some() || search.is_some() {
            match rx.try_recv() {
                Ok(m) => Some(m),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => return,
            }
        } else {
            match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => return,
            }
        };

        // Drain whatever else is queued so a burst of requests for the same
        // page (resize storms, rapid flipping) collapses to the newest one.
        let mut batch: Vec<Msg> = first.into_iter().collect();
        while let Ok(m) = rx.try_recv() {
            batch.push(m);
        }
        let mut latest: Vec<(RenderRequest, oneshot::Sender<Result<Rendered>>)> = Vec::new();
        for msg in batch {
            match msg {
                Msg::Render(req, reply) => {
                    if let Some(pos) = latest
                        .iter()
                        .position(|(r, _)| r.page == req.page && r.job == req.job)
                    {
                        // Dropping the older sender cancels its receiver.
                        latest[pos] = (req, reply);
                    } else {
                        latest.push((req, reply));
                    }
                }
                Msg::Outline(reply) => {
                    let _ = reply.send(engine.outline());
                }
                Msg::TextBoxes { page, rect, reply } => {
                    let _ = reply.send(engine.text_boxes_in_rect(PageIndex(page), rect));
                }
                Msg::HideAnnots { pages, reply } => {
                    let mut n = 0;
                    let mut err = None;
                    for p in pages {
                        match engine.hide_sheaf_annots(PageIndex(p)) {
                            Ok(k) => n += k,
                            Err(e) => err = Some(e),
                        }
                    }
                    let _ = reply.send(match err {
                        Some(e) => Err(e),
                        None => Ok(n),
                    });
                }
                Msg::Search { matcher, tx } => {
                    search = Some(SearchWalk {
                        matcher,
                        next: 0,
                        tx,
                    });
                }
                Msg::PageSizes(reply) => {
                    walk = Some(SizeWalk {
                        reply,
                        sizes: Vec::with_capacity(page_count as usize),
                    });
                }
            }
        }

        latest.sort_by_key(|(r, _)| r.priority);
        for (req, reply) in latest {
            // The viewer drops receivers for pages that scrolled out of view;
            // don't waste the worker on them.
            if reply.is_canceled() {
                continue;
            }
            let t = Instant::now();
            let result = engine
                .render(req.page, req.job, req.format)
                .map(|raster| Rendered {
                    request: req,
                    raster,
                    render_time: t.elapsed(),
                });
            let _ = reply.send(result);
        }

        // Renders done; advance the search by one chunk.
        if let Some(sw) = &mut search {
            if sw.tx.is_closed() {
                search = None;
            } else {
                let to = (sw.next + SEARCH_CHUNK).min(page_count);
                for i in sw.next..to {
                    let found = match &sw.matcher {
                        Matcher::Literal { query, match_case } => {
                            engine.search_page(PageIndex(i), query, *match_case)
                        }
                        Matcher::Pattern(re) => engine.pattern_search_page(PageIndex(i), re),
                    };
                    match found {
                        Ok(hits) if !hits.is_empty() => {
                            let _ = sw.tx.unbounded_send(SearchEvent::Hits(hits));
                        }
                        Ok(_) => {}
                        Err(e) => log::warn!("search page {i}: {e:#}"),
                    }
                }
                sw.next = to;
                let _ = sw.tx.unbounded_send(SearchEvent::Progress(to));
                if to >= page_count {
                    let _ = sw.tx.unbounded_send(SearchEvent::Done);
                    search = None;
                }
            }
        }

        // Then the size walk by one chunk.
        if let Some(w) = &mut walk {
            if w.reply.is_canceled() {
                walk = None;
                continue;
            }
            let from = w.sizes.len() as u32;
            let to = (from + SIZE_CHUNK).min(page_count);
            for i in from..to {
                match engine.page_size(PageIndex(i)) {
                    Ok(sz) => w.sizes.push(sz),
                    Err(e) => {
                        let _ = walk.take().unwrap().reply.send(Err(e));
                        break;
                    }
                }
            }
            if let Some(w) = &walk
                && w.sizes.len() as u32 >= page_count
            {
                let w = walk.take().unwrap();
                let _ = w.reply.send(Ok(w.sizes));
            }
        }
    }
}
