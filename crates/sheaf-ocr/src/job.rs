//! The batch job: render pages, recognize them on worker threads, stream
//! layers back.
//!
//! Threads are plain `std::thread`s rather than the UI's executor: a
//! 400-page run is minutes of CPU and must not starve saves, searches or
//! tile uploads. A coordinator thread renders one page ahead through the
//! source's render worker (at the lowest priority, so visible tiles still
//! win) and hands images to N workers, each owning one engine instance.
//! Results arrive in whatever order they finish; the app batches them into
//! the document.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use anyhow::{Context, Result};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use sheaf_core::{PageSize, Rotation};
use sheaf_doc::{PageId, SourceId, TextLayer};
use sheaf_render::{Job, PixelFormat, Raster, RenderHandle, RenderRequest};

use crate::config::OcrConfig;
use crate::engine::{EngineError, EngineKind, OcrEngine, OcrImage, build_engine};
use crate::geom::layer_from_words;

/// A page that already has at least this many characters of text is not
/// an OCR candidate.
pub const HAS_TEXT_MIN_CHARS: usize = 20;

/// One page to recognize.
#[derive(Debug, Clone, PartialEq)]
pub struct OcrPage {
    pub id: PageId,
    pub source: SourceId,
    /// Index within the source file.
    pub index: u32,
    /// The page's extra rotation; it is rendered and recognized that way.
    pub rotation: Rotation,
    /// Size in points as rendered (after `rotation`). Only a fallback: the
    /// job asks the source for the real size, since the app's size table
    /// fills in asynchronously after a file opens.
    pub size: PageSize,
}

#[derive(Debug, Clone)]
pub struct OcrRequest {
    pub pages: Vec<OcrPage>,
    pub skip_pages_with_text: bool,
    pub engine: EngineKind,
    pub config: OcrConfig,
}

/// What the job needs from an opened file. Implemented for the render
/// worker; faked in tests.
pub trait PageSource: Send + Sync {
    /// The page's size in points as the file shows it (after its own
    /// `/Rotate`, before any extra rotation).
    fn page_size(&self, page: u32) -> Result<PageSize>;
    /// Real characters of text on the page (0 for image-only).
    fn text_len(&self, page: u32) -> Result<usize>;
    /// Render the page `width_px` wide, rotated by `rotation`.
    fn render(&self, page: u32, width_px: u32, rotation: Rotation) -> Result<Raster>;
}

impl PageSource for RenderHandle {
    fn page_size(&self, page: u32) -> Result<PageSize> {
        futures::executor::block_on(self.page_size(page)).context("render worker gone")?
    }

    fn text_len(&self, page: u32) -> Result<usize> {
        futures::executor::block_on(self.page_text_len(page)).context("render worker gone")?
    }

    fn render(&self, page: u32, width_px: u32, rotation: Rotation) -> Result<Raster> {
        let rendered = futures::executor::block_on(self.render(RenderRequest {
            page: sheaf_core::PageIndex(page),
            job: Job::Page {
                width: width_px,
                rotation,
            },
            format: PixelFormat::Rgba,
            priority: u8::MAX,
        }))
        .context("render worker gone")??;
        Ok(rendered.raster)
    }
}

#[derive(Debug)]
pub enum OcrEvent {
    /// Engines are loaded and work begins.
    Started {
        total: usize,
        engine: String,
        workers: usize,
    },
    /// The page already had text.
    Skipped {
        id: PageId,
    },
    Page {
        id: PageId,
        layer: TextLayer,
    },
    Failed {
        id: PageId,
        index: u32,
        error: String,
    },
    /// Always the last event unless `Aborted`.
    Finished {
        done: usize,
        skipped: usize,
        failed: usize,
        cancelled: bool,
    },
    /// No engine could be loaded; nothing was done.
    Aborted(String),
}

/// Owns a running job. Dropping it cancels the job; threads wind down on
/// their own once the page they are on is finished.
pub struct JobHandle {
    cancel: Arc<AtomicBool>,
}

impl JobHandle {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

impl Drop for JobHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Builds one engine instance per worker.
pub type EngineFactory = Arc<dyn Fn() -> Result<Box<dyn OcrEngine>, EngineError> + Send + Sync>;

/// How many engine instances to run: half the cores, one to four, and at
/// most two for the ONNX engine, whose sessions are memory-hungry and
/// already multi-threaded inside.
pub fn worker_count(kind: EngineKind) -> usize {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2);
    let n = (cores / 2).clamp(1, 4);
    match kind {
        EngineKind::Oar => n.min(2),
        EngineKind::Tesseract => n,
    }
}

/// Start a job with the configured engine.
pub fn start(
    req: OcrRequest,
    sources: HashMap<SourceId, Arc<dyn PageSource>>,
) -> (JobHandle, UnboundedReceiver<OcrEvent>) {
    let kind = req.engine;
    let cfg = req.config.clone();
    let factory: EngineFactory = Arc::new(move || build_engine(kind, &cfg));
    start_with(req, sources, factory, worker_count(kind))
}

/// Start a job with an explicit engine factory and worker count.
pub fn start_with(
    req: OcrRequest,
    sources: HashMap<SourceId, Arc<dyn PageSource>>,
    factory: EngineFactory,
    workers: usize,
) -> (JobHandle, UnboundedReceiver<OcrEvent>) {
    let (tx, rx) = unbounded();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let spawned = std::thread::Builder::new()
        .name("sheaf-ocr".into())
        .spawn(move || coordinate(req, sources, factory, workers.max(1), tx.clone(), flag, tx));
    if let Err(e) = spawned {
        // Can't even make a thread: report it through the same channel.
        let (tx, rx) = unbounded();
        let _ = tx.unbounded_send(OcrEvent::Aborted(format!("spawning OCR thread: {e}")));
        return (JobHandle { cancel }, rx);
    }
    (JobHandle { cancel }, rx)
}

struct Work {
    page: OcrPage,
    img: OcrImage,
}

#[allow(clippy::too_many_arguments)]
fn coordinate(
    req: OcrRequest,
    sources: HashMap<SourceId, Arc<dyn PageSource>>,
    factory: EngineFactory,
    workers: usize,
    tx: UnboundedSender<OcrEvent>,
    cancel: Arc<AtomicBool>,
    _keep: UnboundedSender<OcrEvent>,
) {
    // Engines first, so a misconfiguration aborts before any rendering.
    let mut engines: Vec<Box<dyn OcrEngine>> = Vec::with_capacity(workers);
    match factory() {
        Ok(e) => engines.push(e),
        Err(e) => {
            let _ = tx.unbounded_send(OcrEvent::Aborted(e.to_string()));
            return;
        }
    }
    for _ in 1..workers {
        match factory() {
            Ok(e) => engines.push(e),
            Err(e) => {
                log::warn!("ocr: only {} worker(s): {e}", engines.len());
                break;
            }
        }
    }
    let engine_name = engines[0].describe().name;
    let input = engines[0].input();
    let dpi = req.config.clamped_dpi();
    let _ = tx.unbounded_send(OcrEvent::Started {
        total: req.pages.len(),
        engine: engine_name.clone(),
        workers: engines.len(),
    });

    // One queued image per worker: the coordinator renders one page ahead
    // and otherwise waits, so memory stays bounded.
    let (work_tx, work_rx) = mpsc::sync_channel::<Work>(engines.len());
    let work_rx = Arc::new(Mutex::new(work_rx));
    let done = Arc::new(AtomicUsize::new(0));
    let failed = Arc::new(AtomicUsize::new(0));
    let handles: Vec<_> = engines
        .into_iter()
        .enumerate()
        .map(|(i, engine)| {
            let rx = work_rx.clone();
            let tx = tx.clone();
            let cancel = cancel.clone();
            let done = done.clone();
            let failed = failed.clone();
            let name = engine_name.clone();
            std::thread::Builder::new()
                .name(format!("sheaf-ocr-{i}"))
                .spawn(move || worker(engine, rx, tx, cancel, done, failed, dpi, name))
                .expect("spawning OCR worker")
        })
        .collect();

    let mut skipped = 0usize;
    let mut cancelled = false;
    for page in &req.pages {
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            break;
        }
        let fail = |e: String| {
            failed.fetch_add(1, Ordering::Relaxed);
            let _ = tx.unbounded_send(OcrEvent::Failed {
                id: page.id,
                index: page.index,
                error: e,
            });
        };
        let Some(src) = sources.get(&page.source) else {
            fail("source file is not open".into());
            continue;
        };
        if req.skip_pages_with_text {
            match src.text_len(page.index) {
                Ok(n) if n >= HAS_TEXT_MIN_CHARS => {
                    skipped += 1;
                    let _ = tx.unbounded_send(OcrEvent::Skipped { id: page.id });
                    continue;
                }
                Ok(_) => {}
                Err(e) => {
                    fail(format!("reading page text: {e:#}"));
                    continue;
                }
            }
        }
        // The real size, so the pixel→point mapping is exact even before
        // the app has enumerated every page.
        let size = match src.page_size(page.index) {
            Ok(s) => s.rotated(page.rotation),
            Err(e) => {
                log::warn!("ocr: page {} size: {e:#}; using the app's", page.index);
                page.size
            }
        };
        let page = OcrPage {
            size,
            ..page.clone()
        };
        let width = (size.width * dpi as f32 / 72.0).round().max(1.0) as u32;
        let raster = match src.render(page.index, width, page.rotation) {
            Ok(r) => r,
            Err(e) => {
                fail(format!("rendering: {e:#}"));
                continue;
            }
        };
        let Some(img) = OcrImage::from_raster(&raster, input) else {
            fail("rendered page has an unexpected size".into());
            continue;
        };
        drop(raster);
        if work_tx.send(Work { page, img }).is_err() {
            // Every worker is gone.
            break;
        }
    }
    drop(work_tx);
    for h in handles {
        let _ = h.join();
    }
    let _ = tx.unbounded_send(OcrEvent::Finished {
        done: done.load(Ordering::Relaxed),
        skipped,
        failed: failed.load(Ordering::Relaxed),
        cancelled: cancelled || cancel.load(Ordering::Relaxed),
    });
}

#[allow(clippy::too_many_arguments)]
fn worker(
    mut engine: Box<dyn OcrEngine>,
    rx: Arc<Mutex<mpsc::Receiver<Work>>>,
    tx: UnboundedSender<OcrEvent>,
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicUsize>,
    failed: Arc<AtomicUsize>,
    dpi: u32,
    engine_name: String,
) {
    loop {
        // Hold the lock only while waiting, never while recognizing.
        let next = match rx.lock() {
            Ok(guard) => guard.recv(),
            Err(_) => return,
        };
        let Ok(Work { page, img }) = next else {
            return;
        };
        if cancel.load(Ordering::Relaxed) {
            continue; // drain without working
        }
        let (w, h) = (img.width(), img.height());
        match engine.recognize(img, dpi) {
            Ok(words) => {
                let layer = layer_from_words(words, page.size, w, h, page.rotation, &engine_name);
                done.fetch_add(1, Ordering::Relaxed);
                let _ = tx.unbounded_send(OcrEvent::Page { id: page.id, layer });
            }
            Err(e) => {
                failed.fetch_add(1, Ordering::Relaxed);
                let _ = tx.unbounded_send(OcrEvent::Failed {
                    id: page.id,
                    index: page.index,
                    error: format!("{e:#}"),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{EngineInfo, InputKind, OcrWord};
    use futures::StreamExt;
    use std::time::Duration;

    struct FakeSource {
        /// Text length per page index.
        text: Vec<usize>,
        /// What the file says the page size is, if the fake knows.
        sizes: Option<PageSize>,
    }

    impl PageSource for FakeSource {
        fn page_size(&self, _page: u32) -> Result<PageSize> {
            self.sizes.context("size unknown")
        }

        fn text_len(&self, page: u32) -> Result<usize> {
            self.text
                .get(page as usize)
                .copied()
                .context("no such page")
        }

        fn render(&self, _page: u32, width_px: u32, _rotation: Rotation) -> Result<Raster> {
            let height = width_px * 792 / 612;
            Ok(Raster {
                width: width_px,
                height,
                format: PixelFormat::Rgba,
                data: vec![255; (width_px * height * 4) as usize],
            })
        }
    }

    /// Recognizes one word per page, fails on images 300 px wide, and can
    /// be slowed down to test cancellation.
    struct MockEngine {
        delay: Duration,
    }

    impl OcrEngine for MockEngine {
        fn input(&self) -> InputKind {
            InputKind::Gray
        }

        fn recognize(&mut self, img: OcrImage, _dpi: u32) -> Result<Vec<OcrWord>> {
            std::thread::sleep(self.delay);
            if img.width() == 300 {
                anyhow::bail!("mock failure");
            }
            Ok(vec![OcrWord {
                text: "word".into(),
                bbox_px: [61.2, 79.2, 122.4, 158.4],
                confidence: 0.8,
            }])
        }

        fn describe(&self) -> EngineInfo {
            EngineInfo {
                kind: EngineKind::Tesseract,
                name: "mock".into(),
                languages: vec![],
            }
        }
    }

    fn page(i: u32, width: f32) -> OcrPage {
        OcrPage {
            id: PageId(i as u64),
            source: SourceId(0),
            index: i,
            rotation: Rotation::R0,
            size: PageSize {
                width,
                height: 792.0,
            },
        }
    }

    fn run(
        pages: Vec<OcrPage>,
        text: Vec<usize>,
        delay: Duration,
        cancel_after: Option<usize>,
    ) -> Vec<OcrEvent> {
        let req = OcrRequest {
            pages,
            skip_pages_with_text: true,
            engine: EngineKind::Tesseract,
            config: OcrConfig {
                dpi: 150, // clamped minimum, so a 612 pt page is 1275 px wide
                ..OcrConfig::default()
            },
        };
        let mut sources: HashMap<SourceId, Arc<dyn PageSource>> = HashMap::new();
        sources.insert(SourceId(0), Arc::new(FakeSource { text, sizes: None }));
        let factory: EngineFactory = Arc::new(move || Ok(Box::new(MockEngine { delay }) as _));
        let (handle, rx) = start_with(req, sources, factory, 2);
        let mut events = Vec::new();
        let mut rx = rx;
        futures::executor::block_on(async {
            let mut n = 0;
            while let Some(ev) = rx.next().await {
                let last = matches!(ev, OcrEvent::Finished { .. } | OcrEvent::Aborted(_));
                if matches!(ev, OcrEvent::Page { .. }) {
                    n += 1;
                    if cancel_after == Some(n) {
                        handle.cancel();
                    }
                }
                events.push(ev);
                if last {
                    break;
                }
            }
        });
        events
    }

    #[test]
    fn skips_failures_and_finishes() {
        // Page 1 has text; page 3 is 144 pt wide → 300 px → the mock fails.
        let pages = vec![
            page(0, 612.0),
            page(1, 612.0),
            page(2, 612.0),
            page(3, 144.0),
        ];
        let events = run(pages, vec![0, 500, 3, 0], Duration::ZERO, None);
        assert!(matches!(
            events[0],
            OcrEvent::Started {
                total: 4,
                workers: 2,
                ..
            }
        ));
        let skipped: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                OcrEvent::Skipped { id } => Some(*id),
                _ => None,
            })
            .collect();
        assert_eq!(skipped, vec![PageId(1)]);
        let failed: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                OcrEvent::Failed { id, error, .. } => Some((*id, error.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].0, PageId(3));
        assert!(failed[0].1.contains("mock failure"));
        let mut layers: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                OcrEvent::Page { id, layer } => Some((*id, layer)),
                _ => None,
            })
            .collect();
        layers.sort_by_key(|(id, _)| *id);
        assert_eq!(layers.len(), 2);
        assert_eq!(layers[0].0, PageId(0));
        // 1275 px for 612 pt: the mock's 61.2 px box is 29.376 pt.
        let r = layers[0].1.words[0].rect;
        assert!((r.x - 29.376).abs() < 1e-2, "{r:?}");
        assert_eq!(layers[0].1.engine, "mock");
        assert!(matches!(
            events.last(),
            Some(OcrEvent::Finished {
                done: 2,
                skipped: 1,
                failed: 1,
                cancelled: false
            })
        ));
    }

    #[test]
    fn cancel_stops_early() {
        let pages: Vec<OcrPage> = (0..20).map(|i| page(i, 612.0)).collect();
        let t = std::time::Instant::now();
        let events = run(pages, vec![0; 20], Duration::from_millis(30), Some(1));
        let done = events
            .iter()
            .filter(|e| matches!(e, OcrEvent::Page { .. }))
            .count();
        assert!(done < 20, "{done}");
        assert!(matches!(
            events.last(),
            Some(OcrEvent::Finished {
                cancelled: true,
                ..
            })
        ));
        assert!(
            t.elapsed() < Duration::from_millis(20 * 30),
            "{:?}",
            t.elapsed()
        );
    }

    #[test]
    fn real_page_size_wins_over_the_apps_guess() {
        // The app guessed Letter; the file says landscape. The raster and
        // the layer must follow the file.
        let req = OcrRequest {
            pages: vec![page(0, 612.0)],
            skip_pages_with_text: false,
            engine: EngineKind::Tesseract,
            config: OcrConfig {
                dpi: 150,
                ..OcrConfig::default()
            },
        };
        let mut sources: HashMap<SourceId, Arc<dyn PageSource>> = HashMap::new();
        sources.insert(
            SourceId(0),
            Arc::new(FakeSource {
                text: vec![0],
                sizes: Some(PageSize {
                    width: 792.0,
                    height: 612.0,
                }),
            }),
        );
        let factory: EngineFactory = Arc::new(|| {
            Ok(Box::new(MockEngine {
                delay: Duration::ZERO,
            }) as _)
        });
        let (_handle, mut rx) = start_with(req, sources, factory, 1);
        let layer = futures::executor::block_on(async {
            loop {
                match rx.next().await {
                    Some(OcrEvent::Page { layer, .. }) => break layer,
                    Some(OcrEvent::Finished { .. }) | None => panic!("no page"),
                    _ => {}
                }
            }
        });
        assert_eq!(layer.page.width, 792.0);
        // 792 pt at 150 dpi is 1650 px: the mock's 61.2 px box is 29.376 pt.
        assert!(
            (layer.words[0].rect.x - 29.376).abs() < 1e-2,
            "{:?}",
            layer.words[0].rect
        );
    }

    #[test]
    fn engine_failure_aborts() {
        let req = OcrRequest {
            pages: vec![page(0, 612.0)],
            skip_pages_with_text: false,
            engine: EngineKind::Tesseract,
            config: OcrConfig::default(),
        };
        let factory: EngineFactory = Arc::new(|| Err(EngineError::MissingData("no data".into())));
        let (_handle, mut rx) = start_with(req, HashMap::new(), factory, 1);
        let ev = futures::executor::block_on(rx.next()).unwrap();
        assert!(matches!(ev, OcrEvent::Aborted(m) if m == "no data"));
    }

    #[test]
    fn worker_counts_are_bounded() {
        assert!((1..=4).contains(&worker_count(EngineKind::Tesseract)));
        assert!((1..=2).contains(&worker_count(EngineKind::Oar)));
    }
}
