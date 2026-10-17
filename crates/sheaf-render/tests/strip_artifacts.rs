//! A header removed by splicing leaves the rest of the page as it drew:
//! an Acrobat header is stripped (`stamps::strip`), and PDFium renders
//! every pixel outside the header's box as before. Once drawn between the
//! body's text objects, and once as Acrobat lays it out: a `q` before the
//! page's content, and the header after it, opening with the `Q` that
//! ends that `q`, with more drawn after it.
//!
//! Needs libpdfium (`scripts/fetch-pdfium.sh`), like the app itself.

use std::sync::Arc;

use lopdf::{Document as Pdf, Object, ObjectId, Stream, dictionary};
use sheaf_core::{PageIndex, Rotation};
use sheaf_doc::content::{DrawnForms, PagOrigin, PageBlocks, PaginationKind, contents};
use sheaf_doc::source::Limits;
use sheaf_doc::stamps;
use sheaf_image::raster::{PixelFormat, Raster};
use sheaf_render::pdfium::PdfiumEngine;
use sheaf_render::{Job, Renderer};

/// The page's size, in points and so in pixels.
const W: u32 = 300;
const H: u32 = 200;
/// The header's box: its form's, placed at (20, 170), in pixels from the
/// top left.
const HEADER: [u32; 4] = [20, 10, 220, 30];

/// A page of `streams`, drawn with a font `/F1` and a header Acrobat
/// would add, `/Xi0`: a form whose `/PieceInfo` says so.
fn page(streams: &[&[u8]]) -> (Pdf, ObjectId) {
    let mut pdf = Pdf::with_version("1.7");
    let pages = pdf.new_object_id();
    let font = pdf.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
    });
    let piece = dictionary! { "ADBE_CompoundType" => dictionary! {
        "DocSettings" => dictionary! {}, "Private" => "Header",
    } };
    let header = pdf.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Form", "PieceInfo" => piece,
            "BBox" => vec![0.into(), 0.into(), 200.into(), 20.into()],
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } },
        },
        b"0 0 1 rg BT /F1 14 Tf 2 5 Td (HEADER TEXT) Tj ET".to_vec(),
    ));
    let contents: Vec<Object> = streams
        .iter()
        .map(|s| {
            pdf.add_object(Stream::new(dictionary! {}, s.to_vec()))
                .into()
        })
        .collect();
    let page = pdf.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages, "Contents" => contents,
        "MediaBox" => vec![0.into(), 0.into(), W.into(), H.into()],
        "Resources" => dictionary! {
            "Font" => dictionary! { "F1" => font },
            "XObject" => dictionary! { "Xi0" => header },
        },
    });
    pdf.objects.insert(
        pages,
        dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 }.into(),
    );
    let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
    pdf.trailer.set("Root", catalog);
    (pdf, page)
}

fn render(pdf: &mut Pdf) -> Raster {
    let mut bytes = Vec::new();
    pdf.save_to(&mut bytes).unwrap();
    let engine = PdfiumEngine::open_bytes(Arc::from(bytes), None).unwrap();
    engine
        .render(
            PageIndex(0),
            Job::Page {
                width: W,
                rotation: Rotation::R0,
            },
            PixelFormat::Rgba,
        )
        .unwrap()
}

/// Body text above and below the header, drawn between them.
#[test]
fn a_stripped_header_leaves_the_rest_of_the_page() {
    stripped_leaves_the_rest(&[b"BT /F1 14 Tf 20 120 Td (Body above) Tj ET\n\
        /Artifact <</Type /Pagination /Subtype /Header>> BDC q 0 0 300 200 re W n \
        1 0 0 1 20 170 cm /Xi0 Do Q EMC\n\
        BT /F1 14 Tf 20 60 Td (Body below) Tj ET q 1 0 0 rg 200 20 60 40 re f Q"]);
}

/// Acrobat's own layout, the body scaled with no `q` of its own: the `Q`
/// the header opens with stays, so what's drawn after it is drawn at the
/// page's scale, not the body's.
#[test]
fn acrobats_header_keeps_the_q_it_opens_with() {
    stripped_leaves_the_rest(&[
        b"q\n",
        b"BT /F1 14 Tf 20 120 Td (Body above) Tj ET 0.5 0 0 0.5 0 0 cm \
          BT /F1 28 Tf 40 120 Td (Body below) Tj ET",
        b"/Artifact <</Type /Pagination /Subtype /Header>> BDC Q q \
          1 0 0 1 20 170 cm /Xi0 Do Q EMC",
        b"q 1 0 0 rg 200 20 60 40 re f Q",
    ]);
}

/// The page of `streams` with its header stripped: every pixel outside
/// the header's box as before, the header gone, and the body drawn.
fn stripped_leaves_the_rest(streams: &[&[u8]]) {
    let (mut pdf, page) = page(streams);
    let mut dict = pdf.get_dictionary(page).unwrap().clone();
    let blocks = PageBlocks::read(
        &pdf,
        &contents(&dict),
        dict.get(b"Resources").ok(),
        &Limits::DEFAULT,
        &DrawnForms::default(),
    )
    .unwrap();
    let found: Vec<_> = blocks
        .pagination
        .iter()
        .map(|p| (p.kind, p.origin))
        .collect();
    assert_eq!(found, [(PaginationKind::Header, PagOrigin::Acrobat)]);

    let before = render(&mut pdf);
    assert!(stamps::strip(&mut pdf, &mut dict, 0, stamps::HEADER_FOOTER).unwrap());
    pdf.objects.insert(page, Object::Dictionary(dict));
    let after = render(&mut pdf);
    assert_eq!((before.width, before.height), (W, H));
    assert_eq!((after.width, after.height), (W, H));

    let [left, top, right, bottom] = HEADER;
    let in_header = |x: u32, y: u32| (left..right).contains(&x) && (top..bottom).contains(&y);
    let pixel = |r: &Raster, x: u32, y: u32| {
        let i = ((y * r.width + x) * 4) as usize;
        [r.data[i], r.data[i + 1], r.data[i + 2]]
    };
    let blue = |p: [u8; 3]| p[2] > 150 && p[0] < 100;
    let mut header_pixels = 0;
    for y in 0..H {
        for x in 0..W {
            let (was, is) = (pixel(&before, x, y), pixel(&after, x, y));
            if in_header(x, y) {
                header_pixels += usize::from(blue(was));
                assert!(!blue(is), "the header is still at ({x}, {y})");
            } else {
                assert_eq!(was, is, "({x}, {y}) changed outside the header");
            }
        }
    }
    assert!(header_pixels > 50, "the header drew {header_pixels} pixels");
    // The body drew, before and after: black text, the red box.
    assert!(
        before
            .data
            .chunks(4)
            .any(|p| p[0] < 60 && p[1] < 60 && p[2] < 60)
    );
    assert_eq!(pixel(&after, 230, 160), [255, 0, 0]);
}
