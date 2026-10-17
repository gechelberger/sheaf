//! The OCR text layer `sheaf-doc` writes must be real text to PDFium: this
//! saves a layer and checks the engine finds the words where the glyphs
//! are, for every combination of the file's own `/Rotate` and the turn
//! the page was shown with when it was recognized, on a page whose crop
//! box is off the origin.
//!
//! Needs libpdfium (`scripts/fetch-pdfium.sh`), like the app itself; it
//! fails rather than skips without it so CI can't silently lose it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lopdf::{Document as Pdf, Object, Stream, dictionary};
use sheaf_core::{PageIndex, Rotation};
use sheaf_doc::{
    Box2, Document, Edit, FixedCount, OcrWord, PageChars, Pt, SourceInfo, Store, TextLayer,
};
use sheaf_render::Renderer;
use sheaf_render::pdfium::PdfiumEngine;

fn tempdir() -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "sheaf-render-ocr-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A one-page Letter PDF with no content, a crop box off the origin and
/// the given `/Rotate`.
fn blank_pdf(path: &Path, rotate: i64) {
    let mut pdf = Pdf::with_version("1.5");
    let pages_id = pdf.new_object_id();
    let content = pdf.add_object(Stream::new(dictionary! {}, Vec::new()));
    let mut page = dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => content,
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        "CropBox" => vec![20.into(), 30.into(), 600.into(), 770.into()],
    };
    if rotate != 0 {
        page.set("Rotate", rotate);
    }
    let page_id = pdf.add_object(page);
    pdf.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Kids" => vec![Object::Reference(page_id)], "Count" => 1,
        }),
    );
    let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    pdf.trailer.set("Root", catalog);
    pdf.save(path).unwrap();
}

fn word(text: &str, b: Box2) -> OcrWord {
    OcrWord {
        text: text.into(),
        rect: b,
        confidence: 1.0,
    }
}

fn to_box(r: &sheaf_render::Rect) -> Box2 {
    Box2 {
        x: r.x,
        y: r.y,
        w: r.w,
        h: r.h,
    }
}

fn union(rects: &[sheaf_render::Rect]) -> Box2 {
    rects.iter().map(to_box).reduce(Box2::union).unwrap()
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= 2.0
}

fn close_box(a: Box2, b: Box2) -> bool {
    close(a.x, b.x)
        && close(a.y, b.y)
        && close(a.right(), b.right())
        && close(a.bottom(), b.bottom())
}

/// Save a layer recognized with the page shown turned `by` beyond a file
/// whose page has `/Rotate rotate`, then search the saved file.
fn check(rotate: i64, by: Rotation) {
    let dir = tempdir();
    let src = dir.join("src.pdf");
    blank_pdf(&src, rotate);
    let store = Store::new();
    let primary = SourceInfo::read(&store, &src, None).unwrap();
    let mut model = Document::new(store, primary, &[], Box::new(FixedCount(10)));
    let page = model.page(0).unwrap().id;
    if by != Rotation::R0 {
        model
            .apply(
                Edit::RotatePages {
                    ids: vec![page],
                    by,
                },
                "rotate",
            )
            .unwrap();
    }
    // Two words on a line, as the engine would report them in the frame
    // the page was shown in.
    let shown = model.geom(&model.state().pages[0]);
    let hello = Box2 {
        x: 72.0,
        y: 100.0,
        w: 60.0,
        h: 12.0,
    };
    let world = Box2 {
        x: 140.0,
        y: 100.0,
        w: 60.0,
        h: 12.0,
    };
    let layer = TextLayer::from_engine(
        vec![word("Hello", hello), word("World", world)],
        &shown,
        "test",
    );
    let unsaved = PageChars::from_layer(&layer);
    let unsaved_hit = |query: &str| {
        let hits = sheaf_doc::text_layer::search(&layer, query, true);
        hits[0].rects.iter().copied().reduce(Box2::union).unwrap()
    };
    model
        .apply(
            Edit::SetTextLayers {
                layers: vec![(page, Arc::new(layer.clone()))],
            },
            "ocr",
        )
        .unwrap();
    let out = dir.join("ocr.pdf");
    sheaf_doc::save::rewrite_to_path(&model.snapshot(), &out).unwrap();

    // The saved page's /Rotate is the page's rotation, so PDFium's
    // displayed space is the frame the words were recognized in, and
    // through the saved page's geometry its hits are in PageSpace.
    let base = sheaf_doc::forms::fixture::source(std::fs::read(&out).unwrap()).geom(0);
    assert_eq!(base.rotate, shown.rotate);
    let engine = PdfiumEngine::open(&out, None).unwrap();
    let ctx = format!("rotate {rotate}, by {by:?}");
    assert_eq!(engine.page_text_len(PageIndex(0)).unwrap(), 10, "{ctx}");
    let hits = engine.search_page(PageIndex(0), "hello", false).unwrap();
    assert_eq!(hits.len(), 1, "{ctx}: {hits:?}");
    let r = union(&hits[0].rects);
    assert!(close_box(r, hello), "{ctx}: hit {r:?} vs word {hello:?}");
    let r = base.from_view().apply_box(r);
    let want = unsaved_hit("Hello");
    assert!(close_box(r, want), "{ctx}: hit {r:?} vs unsaved {want:?}");
    // The gap between the words became a space, so a phrase matches too.
    let hits = engine
        .search_page(PageIndex(0), "Hello World", true)
        .unwrap();
    assert_eq!(hits.len(), 1, "{ctx}: {hits:?}");
    let r = base.from_view().apply_box(union(&hits[0].rects));
    let want = unsaved_hit("Hello World");
    assert!(close_box(r, want), "{ctx}: phrase hit {r:?} vs {want:?}");

    // Selecting text: the saved page's characters read the same, and a
    // point on "W" is just before it.
    let chars = PageChars::from_engine(
        engine
            .page_chars(PageIndex(0))
            .unwrap()
            .into_iter()
            .map(|(c, r)| (c, r.as_ref().map(to_box)))
            .collect(),
        &base,
    );
    assert_eq!(chars.text(0, chars.len()).trim(), "Hello World", "{ctx}");
    assert_eq!(chars.upright, unsaved.upright, "{ctx}");
    let on_w = shown.from_view().apply(Pt {
        x: world.x + 2.0,
        y: world.y + 6.0,
    });
    let at = chars.boundary_at(on_w, 0.0).expect("on the text");
    assert_eq!(chars.text(at, chars.len()).trim(), "World", "{ctx}");
    // Before the save, the layer's characters are where PDFium puts them.
    assert_eq!(unsaved.text(0, unsaved.len()), "Hello World", "{ctx}");
    let w_of = |c: &PageChars| {
        c.glyphs
            .iter()
            .find(|g| g.ch == 'W')
            .and_then(|g| g.rect)
            .unwrap()
    };
    let (saved_w, unsaved_w) = (w_of(&chars), w_of(&unsaved));
    assert!(
        close_box(saved_w, unsaved_w),
        "{ctx}: W at {saved_w:?} saved, {unsaved_w:?} unsaved"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pdfium_finds_saved_layer_unrotated() {
    check(0, Rotation::R0);
}

#[test]
fn pdfium_finds_saved_layer_on_rotated_source() {
    check(90, Rotation::R0);
    check(270, Rotation::R0);
}

#[test]
fn pdfium_finds_saved_layer_recognized_in_rotated_frame() {
    check(0, Rotation::R90);
    check(0, Rotation::R180);
    check(90, Rotation::R90);
}

#[test]
fn image_only_page_has_no_text() {
    let dir = tempdir();
    let src = dir.join("blank.pdf");
    blank_pdf(&src, 0);
    let engine = PdfiumEngine::open(&src, None).unwrap();
    assert_eq!(engine.page_text_len(PageIndex(0)).unwrap(), 0);
    let _ = std::fs::remove_dir_all(&dir);
}
