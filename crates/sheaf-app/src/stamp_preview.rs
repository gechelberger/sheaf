//! Pictures of stamps for the screen, made in memory with PDFium: a
//! stamp's page (its one-page PDF, from the document's stamps or the
//! library) drawn transparent where the stamp draws nothing, and a turned
//! tape as the file draws it.
//!
//! Drawing asks for a picture with [`picture`], [`library_picture`] or
//! [`tape_picture`]; one that isn't made yet is remembered with where its
//! PDF is, and [`make_wanted`] (called once a frame) makes it off the main
//! thread and redraws the windows when it's done. Pictures are kept for
//! the session: a stamp's by its stem, which names the same page wherever
//! it's found, and a tape's by what it is. One that can't be made isn't
//! asked for again until it may be: from bytes given, never; from a
//! document, once what the document knows has changed.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex, Weak};

use anyhow::Context as _;
use gpui::RenderImage;
use sheaf_core::PageIndex;
use sheaf_doc::assets::MarkAssets;

/// Width of a picture in pixels: sharp at a stamp's usual size, zoomed in
/// a little.
const WIDTH: u32 = 600;

/// Where a wanted picture's PDF is.
enum Pdf {
    /// A document's stamp, by the picture's stem.
    Stamp(Arc<MarkAssets>),
    /// These bytes.
    Bytes(Arc<[u8]>),
}

/// A document's stamps, as they were when a picture was asked of them:
/// which, and their generation then.
type Tried = (Weak<MarkAssets>, u64);

#[derive(Default)]
struct Previews {
    ready: HashMap<String, Arc<RenderImage>>,
    wanted: HashMap<String, Pdf>,
    making: HashSet<String>,
    /// Those that couldn't be made from the bytes given.
    failed: HashSet<String>,
    /// Stamps that couldn't be made from a document's, by stem: the
    /// documents' stamps as they were then.
    missing: HashMap<String, Vec<Tried>>,
}

impl Previews {
    /// Whether stamp `stem` couldn't be made from `assets` as they are.
    fn missing(&self, stem: &str, assets: &Arc<MarkAssets>) -> bool {
        self.missing.get(stem).is_some_and(|tried| {
            tried.iter().any(|(was, generation)| {
                std::ptr::eq(was.as_ptr(), Arc::as_ptr(assets))
                    && *generation == assets.generation()
            })
        })
    }

    /// That picture `key` couldn't be made from `pdf`, `tried` if it's a
    /// document's.
    fn fail(&mut self, key: String, tried: Option<Tried>) {
        let Some((assets, generation)) = tried else {
            self.failed.insert(key);
            return;
        };
        let tried = self.missing.entry(key).or_default();
        tried.retain(|(was, _)| was.strong_count() > 0 && !was.ptr_eq(&assets));
        tried.push((assets, generation));
    }
}

static PREVIEWS: LazyLock<Mutex<Previews>> = LazyLock::new(Default::default);

/// The picture `key` names when it's made; otherwise it's asked for, its
/// PDF from `pdf` (called only then), unless `failed` says it couldn't be
/// made as things are.
fn get(
    key: &str,
    failed: impl FnOnce(&Previews) -> bool,
    pdf: impl FnOnce() -> Option<Pdf>,
) -> Option<Arc<RenderImage>> {
    let mut p = PREVIEWS.lock().ok()?;
    if let Some(image) = p.ready.get(key) {
        return Some(image.clone());
    }
    if !p.making.contains(key) && !p.wanted.contains_key(key) && !failed(&p) {
        let pdf = pdf()?;
        p.wanted.insert(key.to_string(), pdf);
    }
    None
}

/// The picture of stamp `stem`, which `assets` (a document's) has.
pub fn picture(stem: &str, assets: &Arc<MarkAssets>) -> Option<Arc<RenderImage>> {
    get(
        stem,
        |p| p.missing(stem, assets),
        || Some(Pdf::Stamp(assets.clone())),
    )
}

/// The picture of a library stamp, `stem`, whose page is `pdf`.
pub fn library_picture(stem: &str, pdf: &Arc<[u8]>) -> Option<Arc<RenderImage>> {
    get(
        stem,
        |p| p.failed.contains(stem),
        || Some(Pdf::Bytes(pdf.clone())),
    )
}

/// The picture of a turned tape (GPUI can't turn text): its appearance as
/// a one-page PDF, rendered like a stamp's.
pub fn tape_picture(a: &sheaf_doc::Annot) -> Option<Arc<RenderImage>> {
    let key = sheaf_core::checksum(
        serde_json::to_string(&(&a.kind, a.color, &a.contents, a.turn))
            .ok()?
            .as_bytes(),
    );
    // Stems have no colon.
    let key = format!("tape:{key:032x}");
    get(
        &key,
        |p| p.failed.contains(&key),
        || sheaf_doc::annot::tape_pdf(a).map(|pdf| Pdf::Bytes(pdf.into())),
    )
}

/// Make the pictures asked for since last time.
pub fn make_wanted(cx: &mut gpui::App) {
    let wanted: Vec<(String, Pdf)> = {
        let Ok(mut p) = PREVIEWS.lock() else {
            return;
        };
        let w: Vec<(String, Pdf)> = p.wanted.drain().collect();
        p.making.extend(w.iter().map(|(key, _)| key.clone()));
        w
    };
    if wanted.is_empty() {
        return;
    }
    let task = cx.background_executor().spawn(async move {
        wanted
            .into_iter()
            .map(|(key, pdf)| {
                let tried = match &pdf {
                    Pdf::Stamp(assets) => Some((Arc::downgrade(assets), assets.generation())),
                    Pdf::Bytes(_) => None,
                };
                let made = make(&key, pdf);
                if let Err(e) = &made {
                    log::warn!("stamp picture {key}: {e:#}");
                }
                (key, made.ok(), tried)
            })
            .collect::<Vec<_>>()
    });
    cx.spawn(async move |cx| {
        let done = task.await;
        if let Ok(mut p) = PREVIEWS.lock() {
            for (key, image, tried) in done {
                p.making.remove(&key);
                match image {
                    Some(image) => {
                        p.ready.insert(key, image);
                    }
                    None => p.fail(key, tried),
                }
            }
        }
        cx.update(|cx| cx.refresh_windows());
    })
    .detach();
}

/// Render picture `key`'s page from `pdf`, as GPUI draws it (BGRA).
fn make(key: &str, pdf: Pdf) -> anyhow::Result<Arc<RenderImage>> {
    let bytes = match pdf {
        Pdf::Stamp(assets) => assets
            .stamp_pdf(key)
            .context("the stamp isn't in the document or on this machine")?,
        Pdf::Bytes(bytes) => bytes,
    };
    let engine = sheaf_render::pdfium::PdfiumEngine::open_bytes(bytes, None)?;
    let mut r = engine.render_transparent(PageIndex(0), WIDTH)?;
    for px in r.data.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    let image = image::RgbaImage::from_raw(r.width, r.height, r.data).context("short render")?;
    Ok(Arc::new(RenderImage::new(smallvec::smallvec![
        image::Frame::new(image)
    ])))
}

#[cfg(test)]
mod tests {
    use sheaf_doc::Store;

    use super::*;

    /// A stamp a document couldn't give is asked of it again once what it
    /// knows has changed, and of another document at once; one that bytes
    /// given couldn't make, never.
    #[test]
    fn a_picture_that_cant_be_made_is_asked_for_once_it_may_be() {
        let mut p = Previews::default();
        let (one, two) = (
            Arc::new(MarkAssets::new(Store::new())),
            Arc::new(MarkAssets::new(Store::new())),
        );
        p.fail("s".into(), Some((Arc::downgrade(&one), one.generation())));
        assert!(p.missing("s", &one));
        assert!(!p.missing("s", &two) && !p.failed.contains("s"));
        one.intern_file("f", b"notes".as_slice().into());
        assert!(!p.missing("s", &one));
        p.fail("s".into(), Some((Arc::downgrade(&one), one.generation())));
        assert_eq!(p.missing["s"].len(), 1);
        p.fail("t".into(), None);
        assert!(p.failed.contains("t"));
    }
}
