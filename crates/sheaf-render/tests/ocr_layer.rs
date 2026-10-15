//! The OCR text layer `sheaf-doc` writes must be real text to PDFium: this
//! saves a layer and checks the engine finds the words where the glyphs
//! are, for every combination of the file's own `/Rotate` and the extra
//! rotation the page was shown with when it was recognized.
//!
//! Needs libpdfium (`scripts/fetch-pdfium.sh`), like the app itself; it
//! fails rather than skips without it so CI can't silently lose it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lopdf::{Document as Pdf, Object, Stream, dictionary};
use sheaf_core::{PageIndex, PageSize, Rotation};
use sheaf_doc::text_layer::rotate_box;
use sheaf_doc::{Box2, Document, Edit, FixedCount, OcrWord, PageChars, Pt, SourceInfo, TextLayer};
use sheaf_render::Renderer;
use sheaf_render::pdfium::PdfiumEngine;

const LETTER: PageSize = PageSize {
    width: 612.0,
    height: 792.0,
};

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

/// A one-page Letter PDF with no content and the given `/Rotate`.
fn blank_pdf(path: &Path, rotate: i64) {
    let mut pdf = Pdf::with_version("1.5");
    let pages_id = pdf.new_object_id();
    let content = pdf.add_object(Stream::new(dictionary! {}, Vec::new()));
    let mut page = dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => content,
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
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

fn word(text: &str, x: f32, y: f32, w: f32, h: f32) -> OcrWord {
    OcrWord {
        text: text.into(),
        rect: Box2 { x, y, w, h },
        confidence: 1.0,
    }
}

fn union(rects: &[sheaf_render::Rect]) -> Box2 {
    rects
        .iter()
        .map(|r| Box2 {
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
        })
        .reduce(Box2::union)
        .unwrap()
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= 2.0
}

/// Save a layer recognized with the page shown at `upright` on top of a
/// file whose page has `/Rotate rotate`, then search the saved file.
fn check(rotate: i64, upright: Rotation) {
    let dir = tempdir();
    let src = dir.join("src.pdf");
    blank_pdf(&src, rotate);
    let displayed = LETTER.rotated(Rotation::from_degrees(rotate as i32));
    let mut model = Document::new(
        SourceInfo {
            path: src.clone(),
            page_count: 1,
            first_page: displayed,
            page_sizes: None,
            bytes: 0,
            password: None,
            data: None,
            derived: None,
        },
        &[],
        Box::new(FixedCount(10)),
    );
    let page = model.page(0).unwrap().id;
    if upright != Rotation::R0 {
        model
            .apply(
                Edit::RotatePages {
                    ids: vec![page],
                    by: upright,
                },
                "rotate",
            )
            .unwrap();
    }
    // Two words on a line, as the engine would report them in the frame
    // the page was shown in.
    let rendered = displayed.rotated(upright);
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
        vec![
            word("Hello", hello.x, hello.y, hello.w, hello.h),
            word("World", world.x, world.y, world.w, world.h),
        ],
        rendered,
        upright,
        "test",
    );
    let unsaved = PageChars::from_layer(&layer);
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

    // The saved page's /Rotate is the file's plus the extra, so PDFium's
    // displayed space is exactly the frame the words were recognized in.
    let engine = PdfiumEngine::open(&out, None).unwrap();
    let ctx = format!("rotate {rotate}, upright {upright:?}");
    assert_eq!(engine.page_text_len(PageIndex(0)).unwrap(), 10, "{ctx}");
    let hits = engine.search_page(PageIndex(0), "hello", false).unwrap();
    assert_eq!(hits.len(), 1, "{ctx}: {hits:?}");
    let r = union(&hits[0].rects);
    assert!(
        close(r.x, hello.x)
            && close(r.y, hello.y)
            && close(r.right(), hello.right())
            && close(r.bottom(), hello.bottom()),
        "{ctx}: hit {r:?} vs word {hello:?}"
    );
    // The gap between the words became a space, so a phrase matches too.
    let hits = engine
        .search_page(PageIndex(0), "Hello World", true)
        .unwrap();
    assert_eq!(hits.len(), 1, "{ctx}: {hits:?}");
    let r = union(&hits[0].rects);
    assert!(
        close(r.x, hello.x) && close(r.right(), world.right()),
        "{ctx}: phrase hit {r:?}"
    );

    // Selecting text: the saved page's characters read the same, and a
    // point on "W" is just before it.
    let chars = PageChars::from_engine(
        engine
            .page_chars(PageIndex(0))
            .unwrap()
            .into_iter()
            .map(|(c, r)| {
                (
                    c,
                    r.map(|r| Box2 {
                        x: r.x,
                        y: r.y,
                        w: r.w,
                        h: r.h,
                    }),
                )
            })
            .collect(),
        rendered,
    );
    assert_eq!(chars.text(0, chars.len()).trim(), "Hello World", "{ctx}");
    let on_w = Pt {
        x: world.x + 2.0,
        y: world.y + 6.0,
    };
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
    let saved_w = w_of(&chars);
    let unsaved_w = rotate_box(w_of(&unsaved), layer.page, upright);
    assert!(
        close(saved_w.x, unsaved_w.x) && close(saved_w.right(), unsaved_w.right()),
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
