//! The comment summary and exports, from a file with sheaf's marks and
//! another tool's comments: what the summary PDF says, where it draws the
//! pages and their numbers (read back with PDFium), and the CSV.
//!
//! Needs libpdfium (`scripts/fetch-pdfium.sh`), like the app itself.

use std::path::{Path, PathBuf};

use lopdf::{Document as Pdf, Object, Stream, dictionary};
use sheaf_core::{PageIndex, Rotation};
use sheaf_doc::save::text_string;
use sheaf_doc::{Annot, AnnotId, AnnotKind, Box2, Edit, Pt};
use sheaf_image::raster::{PixelFormat, Raster};
use sheaf_ops::comments::{self, Column, CommentFilter, Layout};
use sheaf_render::pdfium::PdfiumEngine;
use sheaf_render::{Job, Renderer};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sheaf-comments-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Three Letter pages, each with a black square: page 1 plain; page 2
/// with Ann's note and Bo's reply to it; page 3 with `/Rotate 90`.
fn pdf(dir: &Path) -> PathBuf {
    let mut pdf = Pdf::with_version("1.5");
    let pages_id = pdf.new_object_id();
    let mut kids = Vec::new();
    for n in 0..3 {
        // Page 3's square is off its middle, to show which way it turned.
        let square: &[u8] = if n == 2 {
            b"0 0 0 rg 100 500 100 100 re f"
        } else {
            b"0 0 0 rg 300 300 100 100 re f"
        };
        let content = pdf.add_object(Stream::new(dictionary! {}, square.to_vec()));
        let mut page = dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => content,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        };
        if n == 2 {
            page.set("Rotate", 90);
        }
        let page_id = pdf.add_object(page);
        if n == 1 {
            let note = pdf.add_object(dictionary! {
                "Type" => "Annot", "Subtype" => "Text", "P" => page_id,
                "Rect" => vec![100.into(), 600.into(), 120.into(), 620.into()],
                "Contents" => text_string("Check “this” (please)"),
                "T" => Object::string_literal("Ann"),
                "NM" => Object::string_literal("ann-1"),
                "M" => Object::string_literal("D:20261001120000"),
                "C" => vec![0.into(), 0.into(), 1.into()],
            });
            let reply = pdf.add_object(dictionary! {
                "Type" => "Annot", "Subtype" => "Text", "P" => page_id,
                "Rect" => vec![100.into(), 600.into(), 120.into(), 620.into()],
                "Contents" => Object::string_literal("Done"),
                "T" => Object::string_literal("Bo"),
                "NM" => Object::string_literal("bo-1"),
                "IRT" => note,
                "M" => Object::string_literal("D:20261002120000"),
            });
            pdf.get_dictionary_mut(page_id)
                .unwrap()
                .set("Annots", vec![note.into(), reply.into()]);
        }
        kids.push(Object::Reference(page_id));
    }
    pdf.objects.insert(
        pages_id,
        dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => 3 }.into(),
    );
    let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    pdf.trailer.set("Root", catalog);
    let path = dir.join("review.pdf");
    pdf.save(&path).unwrap();
    path
}

fn mark(kind: AnnotKind, color: [u8; 3], contents: &str) -> Annot {
    Annot {
        id: AnnotId(0),
        kind,
        color,
        opacity: 1.0,
        stroke: 1.0,
        author: "Greg".into(),
        contents: contents.into(),
        modified: "D:20261005090000".into(),
        created: String::new(),
        fill: None,
        dashed: false,
        cloudy: false,
        turn: Rotation::R0,
        name: String::new(),
    }
}

/// `a`, placed where page `i` of `doc` shows it, as the model keeps it:
/// in PageSpace, reading upright as the page shows.
fn placed(doc: &sheaf_doc::Document, i: usize, a: Annot) -> Annot {
    a.to_page(&doc.geom(doc.page(i).unwrap()))
}

/// The file, with a red note of ours on page 1 at (100, 100) and a green
/// tickmark on page 3 at (50, 60), as they show, and the other tools'
/// comments in it.
fn opened(
    dir: &Path,
) -> (
    sheaf_doc::Document,
    Vec<(sheaf_doc::SourceId, sheaf_doc::file::ForeignAnnot)>,
) {
    let (mut doc, foreign) = comments::open(&pdf(dir), None).unwrap();
    let pages: Vec<_> = doc.state().pages.iter().map(|p| p.id).collect();
    let note = mark(
        AnnotKind::Note {
            pos: Pt { x: 100.0, y: 100.0 },
        },
        [255, 0, 0],
        "First note (with parens)",
    );
    doc.apply(
        Edit::AnnotAdd {
            page: pages[0],
            annot: placed(&doc, 0, note),
        },
        "note",
    )
    .unwrap();
    let tick = mark(
        AnnotKind::Stamp {
            rect: Box2 {
                x: 50.0,
                y: 60.0,
                w: 20.0,
                h: 20.0,
            },
            glyph: "✓".into(),
        },
        [0, 160, 0],
        "",
    );
    doc.apply(
        Edit::AnnotAdd {
            page: pages[2],
            annot: placed(&doc, 2, tick),
        },
        "tick",
    )
    .unwrap();
    (doc, foreign)
}

fn engine(bytes: Vec<u8>) -> PdfiumEngine {
    PdfiumEngine::open_bytes(bytes.into(), None).unwrap()
}

fn render(e: &PdfiumEngine, page: u32) -> Raster {
    e.render(
        PageIndex(page),
        Job::Page {
            width: 792,
            rotation: Rotation::R0,
        },
        PixelFormat::Rgba,
    )
    .unwrap()
}

fn px(r: &Raster, x: f32, y: f32) -> [u8; 3] {
    let i = ((y.round() as u32 * r.width + x.round() as u32) * 4) as usize;
    [r.data[i], r.data[i + 1], r.data[i + 2]]
}

/// The same summary gives the same pages, twice and on two threads:
/// their content and resources, byte for byte, the file's creation date
/// aside.
#[test]
fn the_summary_writes_the_same_pages_every_time() {
    let dir = scratch("same");
    let (doc, foreign) = opened(&dir);
    let snap = doc.snapshot();
    let write = || {
        let bytes = comments::summary_pdf(
            &snap,
            &foreign,
            &CommentFilter::default(),
            "",
            Layout::Pages,
            "review.pdf",
        )
        .unwrap();
        let mut pdf = Pdf::load_mem(&bytes).unwrap();
        let info = pdf.trailer.remove(b"Info").unwrap();
        pdf.objects.remove(&info.as_reference().unwrap());
        let mut out = Vec::new();
        pdf.save_to(&mut out).unwrap();
        out
    };
    let first = write();
    assert_eq!(write(), first);
    std::thread::scope(|s| {
        let threads: Vec<_> = (0..2).map(|_| s.spawn(write)).collect();
        for t in threads {
            assert_eq!(t.join().unwrap(), first);
        }
    });
}

#[test]
fn the_summary_lists_every_comment_and_thread() {
    let dir = scratch("only");
    let (doc, foreign) = opened(&dir);
    let bytes = comments::summary_pdf(
        &doc.snapshot(),
        &foreign,
        &CommentFilter::default(),
        "",
        Layout::CommentsOnly,
        "review.pdf",
    )
    .unwrap();
    let e = engine(bytes);
    assert_eq!(e.page_count(), 1);
    let text = e.page_text(PageIndex(0)).unwrap();
    for want in [
        "review.pdf",
        "3 comments",
        "Page 1",
        "First note (with parens)",
        "Page 2",
        "Check “this” (please)",
        "2.1",
        "Done",
        "Page 3",
        "Tickmark ✔",
        "Page 1 of 1",
    ] {
        assert!(text.contains(want), "no {want:?} in:\n{text}");
    }
    // Filtered to page 2, the rest are gone.
    let cf = CommentFilter {
        pages: Some((1, 1)),
        ..Default::default()
    };
    let bytes = comments::summary_pdf(
        &doc.snapshot(),
        &foreign,
        &cf,
        "",
        Layout::CommentsOnly,
        "review.pdf",
    )
    .unwrap();
    let text = engine(bytes).page_text(PageIndex(0)).unwrap();
    assert!(
        text.contains("Filtered: page 2") && text.contains("Done"),
        "{text}"
    );
    assert!(!text.contains("First note"), "{text}");
}

#[test]
fn the_pages_layout_draws_each_page_with_its_numbers() {
    let dir = scratch("pages");
    let (doc, foreign) = opened(&dir);
    let bytes = comments::summary_pdf(
        &doc.snapshot(),
        &foreign,
        &CommentFilter::default(),
        "",
        Layout::Pages,
        "review.pdf",
    )
    .unwrap();
    let e = engine(bytes);
    assert_eq!(e.page_count(), 3);
    let text = e.page_text(PageIndex(1)).unwrap();
    assert!(
        text.contains("Page 2") && text.contains("2.1") && text.contains("Done"),
        "{text}"
    );

    // Page 1 fills the left half: 351 wide at 0.5735, from (36, 36) down.
    let r = render(&e, 0);
    let k = 351.0 / 612.0;
    let (left, top) = (36.0, 36.0);
    // Its square's middle, user (350, 350), shows at (350, 442).
    assert_eq!(px(&r, left + 350.0 * k, top + 442.0 * k), [0, 0, 0]);
    // The note's number, a red disc at its corner.
    let (cx, cy) = (left + 100.0 * k, top + 100.0 * k);
    for (dx, dy) in [(-5.0, 0.0), (5.0, 0.0), (0.0, -5.0), (0.0, 5.0)] {
        let [red, g, b] = px(&r, cx + dx, cy + dy);
        assert!(
            red > 200 && g < 80 && b < 80,
            "{:?} at {dx},{dy}",
            [red, g, b]
        );
    }

    // Page 3 turned a quarter: 792 × 612 shown, at 351 / 792.
    let r = render(&e, 2);
    let k = 351.0 / 792.0;
    // Its square's middle, user (150, 550), shows at (550, 150).
    assert_eq!(px(&r, left + 550.0 * k, top + 150.0 * k), [0, 0, 0]);
    let (cx, cy) = (left + 50.0 * k, top + 60.0 * k);
    for (dx, dy) in [(-5.0, 0.0), (5.0, 0.0), (0.0, -5.0), (0.0, 5.0)] {
        let [red, g, b] = px(&r, cx + dx, cy + dy);
        assert!(
            red < 60 && g > 120 && b < 60,
            "{:?} at {dx},{dy}",
            [red, g, b]
        );
    }
}

#[test]
fn the_csv_numbers_replies_under_their_comment() {
    let dir = scratch("csv");
    let (doc, foreign) = opened(&dir);
    let listing = comments::list(doc.state(), &foreign, &CommentFilter::default(), "");
    let out = comments::csv(
        &listing,
        None,
        &[
            Column::Number,
            Column::Page,
            Column::Author,
            Column::Text,
            Column::ReplyTo,
        ],
    );
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines,
        [
            "Number,Page,Author,Text,Reply to",
            "1,1,Greg,First note (with parens),",
            "2,2,Ann,Check “this” (please),",
            "2.1,2,Bo,Done,2",
            "3,3,Greg,Tickmark ✓,",
        ]
    );
}

#[test]
fn a_measurement_shows_its_value_and_lists_it() {
    use sheaf_doc::measure::{Measured, Scale};
    let dir = scratch("measure");
    let (mut doc, foreign) = opened(&dir);
    let p0 = doc.state().pages[0].id;
    let feet = Scale {
        page: 1.0,
        page_unit: "in".into(),
        real: 10.0,
        unit: "ft".into(),
        precision: 2,
    };
    let m = mark(
        AnnotKind::Measure {
            what: Measured::Distance,
            points: vec![Pt { x: 100.0, y: 300.0 }, Pt { x: 244.0, y: 300.0 }],
            scale: Box::new(feet),
        },
        [0, 0, 255],
        "",
    );
    doc.apply(Edit::AnnotAdd { page: p0, annot: m }, "measure")
        .unwrap();
    // Two inches at 1 in = 10 ft: drawn on the page with its value.
    let printed = sheaf_doc::save::print_pdf(&doc.snapshot(), &[p0], true).unwrap();
    let text = engine(printed).page_text(PageIndex(0)).unwrap();
    assert!(text.contains("20.00 ft"), "{text}");
    let listing = comments::list(doc.state(), &foreign, &CommentFilter::default(), "");
    let out = comments::csv(
        &listing,
        None,
        &[Column::Type, Column::Measurement, Column::Scale],
    );
    assert!(out.contains("Distance,20.00 ft,1 in = 10 ft"), "{out}");
}
