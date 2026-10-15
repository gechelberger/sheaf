//! Pictures of stamps for the screen. A stamp's page (a one-page PDF in the
//! generated-file cache) is drawn from a PNG beside it, made once with
//! PDFium and transparent where the stamp draws nothing.
//!
//! Drawing asks for a picture with [`picture`]; one that isn't made yet is
//! remembered, and [`make_wanted`] (called once a frame) makes it off the
//! main thread and redraws the windows when it's done.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

use anyhow::Context as _;
use sheaf_core::PageIndex;

/// Width of a picture in pixels: sharp at a stamp's usual size, zoomed in
/// a little.
const WIDTH: u32 = 600;

#[derive(Default)]
struct Previews {
    ready: HashSet<String>,
    wanted: HashSet<String>,
    making: HashSet<String>,
}

static PREVIEWS: LazyLock<Mutex<Previews>> = LazyLock::new(Default::default);

/// Where stamp `stem`'s picture goes; None if `stem` isn't a cache name.
pub fn png_path(stem: &str) -> Option<PathBuf> {
    sheaf_doc::generated::path_of(stem).map(|p| p.with_extension("png"))
}

/// The picture of stamp `stem` when it's made; otherwise it's asked for.
pub fn picture(stem: &str) -> Option<PathBuf> {
    let mut p = PREVIEWS.lock().ok()?;
    let path = png_path(stem)?;
    if p.ready.contains(stem) {
        return Some(path);
    }
    if path.exists() {
        p.ready.insert(stem.to_string());
        return Some(path);
    }
    if !p.making.contains(stem) {
        p.wanted.insert(stem.to_string());
    }
    None
}

/// Tapes' pictures, by what the tape is: the cache name of its PDF.
static TAPES: LazyLock<Mutex<std::collections::HashMap<u128, String>>> =
    LazyLock::new(Default::default);

/// The picture of a turned tape (GPUI can't turn text): its appearance as a
/// one-page PDF in the cache, rendered like a stamp's.
pub fn tape_picture(a: &sheaf_doc::Annot) -> Option<PathBuf> {
    let key = sheaf_core::checksum(
        serde_json::to_string(&(&a.kind, a.color, &a.contents))
            .ok()?
            .as_bytes(),
    );
    let known = TAPES.lock().ok()?.get(&key).cloned();
    let stem = match known {
        Some(s) => s,
        None => {
            let pdf = sheaf_doc::annot::tape_pdf(a)?;
            let stem = sheaf_doc::generated::stem_of(&sheaf_doc::generated::store(&pdf).ok()?);
            TAPES.lock().ok()?.insert(key, stem.clone());
            stem
        }
    };
    picture(&stem)
}

/// Make the pictures asked for since last time.
pub fn make_wanted(cx: &mut gpui::App) {
    let wanted: Vec<String> = {
        let Ok(mut p) = PREVIEWS.lock() else {
            return;
        };
        let w: Vec<String> = p.wanted.drain().collect();
        p.making.extend(w.iter().cloned());
        w
    };
    if wanted.is_empty() {
        return;
    }
    let task = cx.background_executor().spawn(async move {
        for stem in &wanted {
            if let Err(e) = make(stem) {
                log::warn!("stamp picture {stem}: {e:#}");
            }
        }
        wanted
    });
    cx.spawn(async move |cx| {
        let done = task.await;
        if let Ok(mut p) = PREVIEWS.lock() {
            for stem in done {
                p.making.remove(&stem);
                if png_path(&stem).is_some_and(|p| p.exists()) {
                    p.ready.insert(stem);
                }
            }
        }
        cx.update(|cx| cx.refresh_windows());
    })
    .detach();
}

/// Render stamp `stem`'s page to its PNG.
pub fn make(stem: &str) -> anyhow::Result<PathBuf> {
    let pdf = sheaf_doc::generated::path_of(stem).context("not a cache name")?;
    let engine = sheaf_render::pdfium::PdfiumEngine::open(pdf, None)?;
    let r = engine.render_transparent(PageIndex(0), WIDTH)?;
    let img = image::RgbaImage::from_raw(r.width, r.height, r.data).context("short render")?;
    let png = png_path(stem).context("not a cache name")?;
    let tmp = png.with_extension("png.part");
    img.save_with_format(&tmp, image::ImageFormat::Png)?;
    std::fs::rename(&tmp, &png)?;
    Ok(png)
}
