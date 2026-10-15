//! The appearances sheaf writes for text markup and text edits are what
//! other viewers draw: this saves an underline, a strikethrough, a
//! replacement and an insertion, renders the file with PDFium, and looks
//! for their colors where they should be, on a plain page and on one with
//! `/Rotate 90`.
//!
//! Needs libpdfium (`scripts/fetch-pdfium.sh`), like the app itself.

use std::path::{Path, PathBuf};

use lopdf::{Document as Pdf, Object, Stream, dictionary};
use sheaf_core::{PageIndex, PageSize, Rotation};
use sheaf_doc::annot::caret_box;
use sheaf_doc::{Annot, AnnotId, AnnotKind, Box2, Document, Edit, FixedCount, SourceInfo};
use sheaf_image::raster::PixelFormat;
use sheaf_render::pdfium::PdfiumEngine;
use sheaf_render::{Job, Renderer};

fn tempdir() -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "sheaf-render-markup-{}-{}",
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

fn mark(kind: AnnotKind, color: [u8; 3]) -> Annot {
    Annot {
        id: AnnotId(0),
        kind,
        color,
        opacity: 1.0,
        stroke: 1.0,
        author: "test".into(),
        contents: "x".into(),
        modified: String::new(),
        created: String::new(),
        fill: None,
        dashed: false,
    }
}

fn row(y: f32) -> Box2 {
    Box2 {
        x: 100.0,
        y,
        w: 200.0,
        h: 20.0,
    }
}

const BLUE: [u8; 3] = [0, 0, 255];
const RED: [u8; 3] = [255, 0, 0];
const GREEN: [u8; 3] = [0, 160, 0];
const PURPLE: [u8; 3] = [160, 0, 160];

fn check(rotate: i64) {
    let dir = tempdir();
    let src = dir.join("src.pdf");
    blank_pdf(&src, rotate);
    let shown = PageSize {
        width: 612.0,
        height: 792.0,
    }
    .rotated(Rotation::from_degrees(rotate as i32));
    let mut model = Document::new(
        SourceInfo {
            path: src.clone(),
            page_count: 1,
            first_page: shown,
            page_sizes: None,
            bytes: 0,
            password: None,
            data: None,
        },
        &[],
        Box::new(FixedCount(10)),
    );
    let page = model.page(0).unwrap().id;
    let replaced = row(300.0);
    for a in [
        mark(
            AnnotKind::Underline {
                rects: vec![row(100.0)],
            },
            BLUE,
        ),
        mark(
            AnnotKind::StrikeOut {
                rects: vec![row(200.0)],
            },
            RED,
        ),
        mark(
            AnnotKind::Replace {
                rects: vec![replaced],
                struck: "old".into(),
                at: caret_box(replaced.right(), replaced.bottom(), replaced.h),
            },
            GREEN,
        ),
        mark(
            AnnotKind::Insert {
                at: caret_box(400.0, 420.0, 20.0),
            },
            PURPLE,
        ),
    ] {
        model
            .apply(Edit::AnnotAdd { page, annot: a }, "mark")
            .unwrap();
    }
    let out = dir.join("marked.pdf");
    sheaf_doc::save::rewrite_to_path(&model.snapshot(), &out).unwrap();

    // One pixel per point, in the page as shown.
    let engine = PdfiumEngine::open(&out, None).unwrap();
    let raster = engine
        .render(
            PageIndex(0),
            Job::Page {
                width: shown.width as u32,
                rotation: Rotation::R0,
            },
            PixelFormat::Rgba,
        )
        .unwrap();
    let px = |x: f32, y: f32| {
        let i = ((y as u32 * raster.width + x as u32) * 4) as usize;
        [raster.data[i], raster.data[i + 1], raster.data[i + 2]]
    };
    let ctx = format!("rotate {rotate}");
    let near = |c: [u8; 3], want: [u8; 3]| {
        c.iter()
            .zip(want)
            .all(|(a, b)| (*a as i32 - b as i32).abs() < 90)
    };
    // Under the first row, through the second and third.
    assert!(
        near(px(200.0, 118.0), BLUE),
        "{ctx}: underline {:?}",
        px(200.0, 118.0)
    );
    assert!(
        near(px(200.0, 105.0), [255, 255, 255]),
        "{ctx}: above the underline"
    );
    assert!(
        near(px(200.0, 211.0), RED),
        "{ctx}: strikeout {:?}",
        px(200.0, 211.0)
    );
    assert!(
        near(px(200.0, 311.0), GREEN),
        "{ctx}: replacement's strikeout"
    );
    // Carets: filled triangles, tip up, centred on their spot.
    assert!(
        near(px(300.0, 322.0), GREEN),
        "{ctx}: replacement's caret {:?}",
        px(300.0, 322.0)
    );
    assert!(
        near(px(400.0, 422.0), PURPLE),
        "{ctx}: insertion's caret {:?}",
        px(400.0, 422.0)
    );
    assert!(
        near(px(396.0, 416.0), [255, 255, 255]),
        "{ctx}: beside the caret's tip"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn text_markup_draws_where_it_should() {
    check(0);
}

#[test]
fn text_markup_draws_where_it_should_on_a_turned_page() {
    check(90);
}
