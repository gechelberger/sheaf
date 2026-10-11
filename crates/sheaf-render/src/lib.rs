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
use sheaf_core::{PageIndex, PageSize};

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
    /// The whole page, scaled to `width` wide.
    Page { width: u32 },
    /// The `w`×`h` region at (`x`, `y`) of the page scaled to
    /// `page_width`×`page_height`. Lets a zoomed-in page be rendered a
    /// viewport's worth at a time instead of as one huge bitmap.
    Tile {
        page_width: u32,
        page_height: u32,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
    },
}

/// Synchronous engine interface. Implementations may block.
pub trait Renderer {
    fn page_count(&self) -> u32;
    fn page_size(&self, page: PageIndex) -> Result<PageSize>;
    fn page_sizes(&self) -> Result<Vec<PageSize>>;
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

        fn render(&self, page: PageIndex, job: Job, format: PixelFormat) -> Result<Raster> {
            let p = self.doc.pages().get(page.0 as i32)?;
            let bmp = match job {
                Job::Page { width } => {
                    let cfg = PdfRenderConfig::new()
                        .set_target_width(width.max(1) as i32)
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

enum Msg {
    Render(RenderRequest, oneshot::Sender<Result<Rendered>>),
    PageSizes(oneshot::Sender<Result<Vec<PageSize>>>),
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

fn worker_loop(engine: impl Renderer, rx: mpsc::Receiver<Msg>) {
    let page_count = engine.page_count();
    let mut walk: Option<SizeWalk> = None;

    loop {
        // Block only when there's no background work to resume.
        let first = if walk.is_some() {
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

        // Renders done; advance the size walk by one chunk.
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
