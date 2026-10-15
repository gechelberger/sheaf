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
use sheaf_doc::{Annot, AnnotId, AnnotKind, Box2, Document, Edit, FixedCount, Pt, SourceInfo};
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
        cloudy: false,
    }
}

/// A one-page red stamp, 100 × 50, in the generated-file cache; its name.
fn red_stamp() -> String {
    let mut pdf = Pdf::with_version("1.5");
    let pages_id = pdf.new_object_id();
    let content = pdf.add_object(Stream::new(
        dictionary! {},
        b"1 0 0 rg 0 0 100 50 re f".to_vec(),
    ));
    let page = pdf.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => content,
        "MediaBox" => vec![0.into(), 0.into(), 100.into(), 50.into()],
    });
    pdf.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1,
        }),
    );
    let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    pdf.trailer.set("Root", catalog);
    let mut bytes = Vec::new();
    pdf.save_to(&mut bytes).unwrap();
    sheaf_doc::generated::stem_of(&sheaf_doc::generated::store(&bytes).unwrap())
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
        // A red page stamp, 100 × 50, and a blue text stamp's frame.
        mark(
            AnnotKind::PageStamp {
                rect: Box2 {
                    x: 100.0,
                    y: 500.0,
                    w: 100.0,
                    h: 50.0,
                },
                stamp: red_stamp(),
                name: "Red".into(),
                text: None,
            },
            BLUE,
        ),
        mark(
            AnnotKind::TextStamp {
                rect: Box2 {
                    x: 300.0,
                    y: 500.0,
                    w: 150.0,
                    h: 60.0,
                },
                text: "APPROVED\nGE 10/14/2026".into(),
                status: false,
            },
            BLUE,
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
    // The stamp's page fills its box, upright as shown.
    assert!(
        near(px(110.0, 510.0), RED),
        "{ctx}: page stamp {:?}",
        px(110.0, 510.0)
    );
    assert!(
        near(px(190.0, 545.0), RED),
        "{ctx}: page stamp's far corner"
    );
    assert!(
        near(px(210.0, 525.0), [255, 255, 255]),
        "{ctx}: beside the page stamp"
    );
    // The text stamp's frame, at its left edge, and its first line.
    assert!(
        near(px(301.0, 530.0), BLUE),
        "{ctx}: text stamp frame {:?}",
        px(301.0, 530.0)
    );
    let inked = (320..430).any(|x| {
        let c = px(x as f32, 520.0);
        c[2] > 150 && c[0] < 120
    });
    assert!(inked, "{ctx}: text stamp's first line");
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

#[test]
fn another_tools_comment_hides_and_shows_by_slot() {
    let dir = tempdir();
    let path = dir.join("square.pdf");
    let mut pdf = Pdf::with_version("1.5");
    let pages_id = pdf.new_object_id();
    let content = pdf.add_object(Stream::new(dictionary! {}, Vec::new()));
    // A filled square, its look drawn by its appearance stream.
    let ap = pdf.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Form",
            "BBox" => vec![100.into(), 100.into(), 200.into(), 200.into()],
        },
        b"1 0 0 rg 100 100 100 100 re f".to_vec(),
    ));
    let link = pdf.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Link",
        "Rect" => vec![0.into(), 0.into(), 10.into(), 10.into()],
    });
    let square = pdf.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Square",
        "Rect" => vec![100.into(), 100.into(), 200.into(), 200.into()],
        "AP" => dictionary! { "N" => ap },
    });
    let page = pdf.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => content,
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        "Annots" => vec![Object::Reference(link), Object::Reference(square)],
    });
    pdf.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1,
        }),
    );
    let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    pdf.trailer.set("Root", catalog);
    pdf.save(&path).unwrap();

    let engine = PdfiumEngine::open(&path, None).unwrap();
    let red_at_center = |e: &PdfiumEngine| {
        let r = e
            .render(
                PageIndex(0),
                Job::Page {
                    width: 612,
                    rotation: Rotation::R0,
                },
                PixelFormat::Rgba,
            )
            .unwrap();
        // (150, 150) in user space is 642 down from the top.
        let i = ((642 * r.width + 150) * 4) as usize;
        r.data[i] > 200 && r.data[i + 1] < 80
    };
    assert!(red_at_center(&engine));
    // Slot 1: the link is slot 0.
    assert_eq!(
        engine
            .set_annots_hidden(PageIndex(0), &[(1, true)])
            .unwrap(),
        1
    );
    assert!(!red_at_center(&engine));
    engine
        .set_annots_hidden(PageIndex(0), &[(1, false)])
        .unwrap();
    assert!(red_at_center(&engine));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tapes_draw_turned_and_from_old_text() {
    use sheaf_doc::tape::{Line, Op, Row, Tape, TapeLook};
    let dir = tempdir();
    let src = dir.join("src.pdf");
    blank_pdf(&src, 0);
    let letter = PageSize {
        width: 612.0,
        height: 792.0,
    };
    let mut model = Document::new(
        SourceInfo {
            path: src.clone(),
            page_count: 1,
            first_page: letter,
            page_sizes: None,
            bytes: 0,
            password: None,
            data: None,
        },
        &[],
        Box::new(FixedCount(10)),
    );
    let page = model.page(0).unwrap().id;
    let mut tape = Tape {
        lines: vec![
            Line::new(Row::Title("Cash".into())),
            Line::new(Row::Entry {
                value: 1234.5,
                op: Op::Add,
            }),
            Line::new(Row::Entry {
                value: 250.0,
                op: Op::Add,
            }),
            Line::new(Row::Total(0.0)),
        ],
    };
    tape.retotal();
    let look = TapeLook {
        rotation: 90,
        paper: [255, 250, 205],
        ..Default::default()
    };
    // Turned: its box is tall.
    let (w, h) = sheaf_doc::annot::tape_size(&tape, 10.0, &look);
    let turned = Box2 {
        x: 100.0,
        y: 100.0,
        w: h,
        h: w,
    };
    let mut a = mark(
        AnnotKind::Tape {
            rect: turned,
            size: 10.0,
            tape: tape.clone(),
            look,
        },
        [20, 20, 20],
    );
    a.contents = tape.text(2);
    model
        .apply(Edit::AnnotAdd { page, annot: a }, "tape")
        .unwrap();
    // From before rows were kept: text only, read back from it.
    let mut old = mark(
        AnnotKind::Tape {
            rect: Box2 {
                x: 300.0,
                y: 100.0,
                w: 110.0,
                h: 60.0,
            },
            size: 10.0,
            tape: Tape::default(),
            look: TapeLook::default(),
        },
        [20, 20, 20],
    );
    old.contents = "1,234.50 +\n250.00 +\n1,484.50 T".into();
    model
        .apply(Edit::AnnotAdd { page, annot: old }, "tape")
        .unwrap();
    let out = dir.join("tapes.pdf");
    sheaf_doc::save::rewrite_to_path(&model.snapshot(), &out).unwrap();

    let engine = PdfiumEngine::open(&out, None).unwrap();
    let r = engine
        .render(
            PageIndex(0),
            Job::Page {
                width: 612,
                rotation: Rotation::R0,
            },
            PixelFormat::Rgba,
        )
        .unwrap();
    let px = |x: f32, y: f32| {
        let i = ((y as u32 * r.width + x as u32) * 4) as usize;
        [r.data[i], r.data[i + 1], r.data[i + 2]]
    };
    let inked = |x0: f32, y0: f32, x1: f32, y1: f32| {
        let mut n = 0;
        for y in y0 as u32..y1 as u32 {
            for x in x0 as u32..x1 as u32 {
                if px(x as f32, y as f32).iter().all(|c| *c < 100) {
                    n += 1;
                }
            }
        }
        n
    };
    // The turned tape: yellow paper over its whole tall box, with text on
    // it, and the page white beside it.
    let paper = px(turned.x + 3.0, turned.bottom() - 3.0);
    assert!(paper[0] > 240 && paper[2] < 230, "paper {paper:?}");
    assert!(inked(turned.x, turned.y, turned.right(), turned.bottom()) > 50);
    let beside = px(turned.right() + 5.0, turned.y + 10.0);
    assert!(beside.iter().all(|c| *c > 245), "beside {beside:?}");
    // The old one: its text drawn too.
    assert!(inked(300.0, 100.0, 410.0, 160.0) > 50);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn clouds_and_callouts_draw() {
    let dir = tempdir();
    let src = dir.join("src.pdf");
    blank_pdf(&src, 0);
    let mut model = Document::new(
        SourceInfo {
            path: src.clone(),
            page_count: 1,
            first_page: PageSize {
                width: 612.0,
                height: 792.0,
            },
            page_sizes: None,
            bytes: 0,
            password: None,
            data: None,
        },
        &[],
        Box::new(FixedCount(10)),
    );
    let page = model.page(0).unwrap().id;
    let mut cloud = mark(
        AnnotKind::Square {
            rect: Box2 {
                x: 100.0,
                y: 100.0,
                w: 200.0,
                h: 100.0,
            },
        },
        RED,
    );
    cloud.cloudy = true;
    cloud.stroke = 2.0;
    let callout = mark(
        AnnotKind::Callout {
            rect: Box2 {
                x: 350.0,
                y: 300.0,
                w: 150.0,
                h: 36.0,
            },
            knee: Pt { x: 330.0, y: 318.0 },
            point: Pt { x: 250.0, y: 400.0 },
            size: 12.0,
        },
        BLUE,
    );
    for a in [cloud, callout] {
        model
            .apply(Edit::AnnotAdd { page, annot: a }, "mark")
            .unwrap();
    }
    let out = dir.join("shapes.pdf");
    sheaf_doc::save::rewrite_to_path(&model.snapshot(), &out).unwrap();
    let engine = PdfiumEngine::open(&out, None).unwrap();
    let r = engine
        .render(
            PageIndex(0),
            Job::Page {
                width: 612,
                rotation: Rotation::R0,
            },
            PixelFormat::Rgba,
        )
        .unwrap();
    let px = |x: u32, y: u32| {
        let i = ((y * r.width + x) * 4) as usize;
        [r.data[i], r.data[i + 1], r.data[i + 2]]
    };
    let count = |x0: u32, y0: u32, x1: u32, y1: u32, f: &dyn Fn([u8; 3]) -> bool| {
        (y0..y1)
            .flat_map(|y| (x0..x1).map(move |x| (x, y)))
            .filter(|&(x, y)| f(px(x, y)))
            .count()
    };
    let red = |c: [u8; 3]| c[0] > 180 && c[1] < 100;
    // Thin lines antialias: clearly bluer than red is blue enough.
    let blue = |c: [u8; 3]| c[2] > 180 && c[2] as i32 - c[0] as i32 > 60;
    // The cloud's scallops bulge outside the square, above its top edge.
    assert!(count(100, 80, 300, 99, &red) > 20, "scallops above the box");
    // Inside, well away from the edge, the page shows.
    assert_eq!(count(150, 130, 250, 170, &red), 0);
    // The callout's box edge, and its leader down to the point.
    let edge = count(348, 300, 353, 336, &blue);
    let leader = count(255, 320, 335, 395, &blue);
    assert!(edge > 10, "box's left edge: {edge}");
    assert!(leader > 10, "leader: {leader}");
    let _ = std::fs::remove_dir_all(&dir);
}
