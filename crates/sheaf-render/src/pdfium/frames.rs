//! `Geom`'s frame against PDFium's (M10.5 §5): over Greg's corpus, a
//! page's `to_view` is PDFium's page-to-device mapping, and on the pages a
//! file turns, each widget lands where PDFium puts it. sheaf has no
//! widget hit test of its own yet (M10 slice 10 adds one), so PDFium's
//! mapping is the one to match. Skips without the corpus.

use sheaf_core::Rotation;
use sheaf_doc::forms::corpus::{Corpus, Entry, each_entry};
use sheaf_doc::geom::Geom;
use sheaf_doc::store::BlobKind;
use sheaf_doc::{Pt, Src, Store};

use super::*;

/// Device pixels per point PDFium maps at, so its whole pixels are
/// sixteenths of a point.
const SCALE: f32 = 16.0;

/// Within a pixel at [`SCALE`], which covers PDFium's rounding to whole
/// pixels.
fn near(a: Pt, b: Pt) -> bool {
    (a.x - b.x).abs() <= 1.0 / SCALE && (a.y - b.y).abs() <= 1.0 / SCALE
}

/// PDFium's mappings between a page and a device the size it shows at
/// [`SCALE`].
struct Device<'a> {
    page: PdfPage<'a>,
    config: PdfRenderConfig,
    /// Pixels per point across and down: [`SCALE`], but for the device's
    /// size in whole pixels.
    scale: (f32, f32),
}

impl<'a> Device<'a> {
    /// Page `index` of `engine`, which shows `geom`; None when PDFium
    /// can't load it or nothing of it shows. Under [`lock`].
    fn of(engine: &'a PdfiumEngine, index: u32, geom: &Geom) -> Option<Device<'a>> {
        let shown = geom.shown();
        if shown.width < 1.0 || shown.height < 1.0 {
            return None;
        }
        let page = engine.doc.pages().get(index as i32).ok()?;
        let (w, h) = (
            (shown.width * SCALE).round(),
            (shown.height * SCALE).round(),
        );
        Some(Device {
            page,
            config: PdfRenderConfig::new().set_fixed_size(w as i32, h as i32),
            scale: (w / shown.width, h / shown.height),
        })
    }

    /// Where PDFium shows user-space point `p`, in points from the shown
    /// page's top-left.
    fn page_to_device(&self, p: Pt) -> Option<Pt> {
        let (x, y) = self
            .page
            .points_to_pixels(PdfPoints::new(p.x), PdfPoints::new(p.y), &self.config)
            .ok()?;
        Some(Pt {
            x: x as f32 / self.scale.0,
            y: y as f32 / self.scale.1,
        })
    }

    /// The user-space point PDFium shows at `v`.
    fn device_to_page(&self, v: Pt) -> Option<Pt> {
        let (x, y) = (
            (v.x * self.scale.0).round() as i32,
            (v.y * self.scale.1).round() as i32,
        );
        let (x, y) = self.page.pixels_to_points(x, y, &self.config).ok()?;
        Some(Pt {
            x: x.value,
            y: y.value,
        })
    }
}

/// A corpus file's bytes, and the file as a source; None when it doesn't
/// open as one.
fn source(corpus: &Corpus, entry: &Entry) -> Option<(Arc<[u8]>, Src)> {
    let path = corpus.path(entry);
    let bytes: Arc<[u8]> = std::fs::read(&path).ok()?.into();
    let src = Store::new()
        .open(bytes.clone(), BlobKind::file(&path, None))
        .ok()?;
    Some((bytes, src))
}

/// Whether a page's frame is more than the plain one, a y flip below the
/// height: its file turns it, or its visible box is off the origin.
fn moved(geom: &Geom) -> bool {
    let v = geom.visible();
    geom.rotate != Rotation::R0 || v.x0 != 0.0 || v.y0 != 0.0
}

/// How many of a corpus file's pages, `every` one or the [`moved`] ones,
/// PDFium shows, and where a corner of a page's visible box doesn't show
/// where `to_view` puts it.
fn corners(corpus: &Corpus, entry: &Entry, every: bool) -> Option<(usize, Vec<String>)> {
    let (bytes, src) = source(corpus, entry)?;
    let wanted: Vec<u32> = (0..src.page_count())
        .filter(|&i| every || moved(&src.geom(i)))
        .collect();
    if wanted.is_empty() {
        return Some((0, Vec::new()));
    }
    let engine = PdfiumEngine::open_bytes(bytes, None).ok()?;
    let _pdfium = lock();
    let (mut pages, mut differ) = (0, Vec::new());
    for index in wanted {
        let geom = src.geom(index);
        let Some(device) = Device::of(&engine, index, &geom) else {
            continue;
        };
        pages += 1;
        let v = geom.visible();
        for (x, y) in [(v.x0, v.y0), (v.x0, v.y1), (v.x1, v.y1), (v.x1, v.y0)] {
            let corner = Pt { x, y };
            let want = geom.to_view().apply(corner);
            match device.page_to_device(corner) {
                Some(got) if near(got, want) => {}
                got => differ.push(format!(
                    "page {index} ({geom:?}): {corner:?} at {want:?}, PDFium's {got:?}"
                )),
            }
        }
    }
    Some((pages, differ))
}

/// [`corners`] over the corpus: none differ, within 1/16 pt.
fn corners_are_pdfium_s(every: bool) {
    let Some(corpus) = Corpus::or_skip() else {
        return;
    };
    let entries = corpus.manifest().unwrap();
    let outcomes = each_entry(&entries, |entry| corners(&corpus, entry, every));
    let pages: usize = outcomes.iter().flatten().map(|o| o.0).sum();
    eprintln!("{} files: {pages} pages compared", entries.len());
    assert!(pages > 0, "nothing was compared");
    let differ: Vec<(&str, &[String])> = entries
        .iter()
        .zip(&outcomes)
        .filter_map(|(entry, o)| {
            let found = &o.as_ref()?.1;
            (!found.is_empty()).then(|| (entry.path.as_str(), &found[..found.len().min(3)]))
        })
        .collect();
    assert!(differ.is_empty(), "{differ:#?}");
}

/// The corpus pages whose frame is more than the plain one map their
/// visible box's corners to the page as shown as PDFium does. They're
/// about 400 of its 24,000 pages, which take PDFium 100 s to load.
#[test]
fn to_view_is_pdfium_s_page_to_device() {
    corners_are_pdfium_s(false);
}

/// [`to_view_is_pdfium_s_page_to_device`] on every page.
#[test]
#[ignore = "every corpus page, 100 s: cargo test -p sheaf-render -- --ignored to_view_is"]
fn to_view_is_pdfium_s_page_to_device_on_every_page() {
    corners_are_pdfium_s(true);
}

/// How many widgets are on the pages a corpus file turns, and which of
/// them PDFium doesn't put where `to_view` does, or doesn't find under
/// that point again.
fn turned_widgets(corpus: &Corpus, entry: &Entry) -> Option<(usize, Vec<String>)> {
    let (bytes, src) = source(corpus, entry)?;
    let turned = |index: &u32| src.geom(*index).rotate != Rotation::R0;
    if !(0..src.page_count()).any(|i| turned(&i)) {
        return Some((0, Vec::new()));
    }
    let form = sheaf_doc::forms::read(&src);
    let engine = PdfiumEngine::open_bytes(bytes, None).ok()?;
    let _pdfium = lock();
    let (mut widgets, mut differ) = (0, Vec::new());
    for (index, list) in form.widgets.iter().filter(|(i, _)| turned(i)) {
        let geom = src.geom(*index);
        let Some(device) = Device::of(&engine, *index, &geom) else {
            continue;
        };
        for w in list {
            widgets += 1;
            let [x0, y0, x1, y1] = w.rect;
            let centre = Pt {
                x: (x0 + x1) / 2.0,
                y: (y0 + y1) / 2.0,
            };
            let view = geom.to_view().apply(centre);
            let shown = device.page_to_device(centre);
            let back = device.device_to_page(view);
            let inside = |p: Pt| {
                let slack = 1.0 / SCALE;
                p.x >= x0 - slack && p.x <= x1 + slack && p.y >= y0 - slack && p.y <= y1 + slack
            };
            if !shown.is_some_and(|s| near(s, view)) || !back.is_some_and(inside) {
                differ.push(format!(
                    "page {index}, widget {:?} {:?}: at {view:?}, PDFium's {shown:?}, \
                     and back {back:?}",
                    w.object, w.rect
                ));
            }
        }
    }
    Some((widgets, differ))
}

/// On every page a corpus file turns, each widget's centre shows where
/// PDFium shows it, and PDFium's point under that place is in the
/// widget: a click there is on it.
#[test]
fn widgets_land_where_pdfium_puts_them() {
    let Some(corpus) = Corpus::or_skip() else {
        return;
    };
    let entries = corpus.manifest().unwrap();
    let outcomes = each_entry(&entries, |entry| turned_widgets(&corpus, entry));
    let widgets: usize = outcomes.iter().flatten().map(|o| o.0).sum();
    eprintln!("{} files: {widgets} widgets on turned pages", entries.len());
    assert!(widgets > 0, "no widget on a turned page was compared");
    let differ: Vec<(&str, &[String])> = entries
        .iter()
        .zip(&outcomes)
        .filter_map(|(entry, o)| {
            let found = &o.as_ref()?.1;
            (!found.is_empty()).then(|| (entry.path.as_str(), &found[..found.len().min(3)]))
        })
        .collect();
    assert!(differ.is_empty(), "{differ:#?}");
}
