//! A library stamp draws as the page it's made from: the page alone
//! (`stamp_lib::one_page`), made a form (`page_form`) whose content is
//! the page's streams as one and which draws the page as it shows from
//! the origin, drawn on a page of its size, renders in PDFium pixel for
//! pixel as the page does; and placed as a page stamp and saved, it draws
//! in the mark's box as PDFium shows the page, cropped and turned.
//!
//! Needs libpdfium (`scripts/fetch-pdfium.sh`), like the app itself.

use std::sync::Arc;

use lopdf::{Document as Pdf, Object, Stream, dictionary};
use sheaf_core::{PageIndex, Rotation};
use sheaf_doc::store::BlobKind;
use sheaf_doc::{Annot, AnnotId, AnnotKind, Box2, Document, Edit, FixedCount, SourceInfo, Store};
use sheaf_doc::{generated, stamp_lib};
use sheaf_image::raster::{PixelFormat, Raster};
use sheaf_render::pdfium::{PdfiumEngine, render_appearances};
use sheaf_render::{Job, Renderer};

/// The box of the stamp's page, off the origin.
const BOX: [i64; 4] = [50, 50, 250, 150];
/// The box of a page the stamp is drawn on, as big.
const PLACED: [i64; 4] = [0, 0, 200, 100];

/// A one-page PDF of `content`, on a page of `media` with Helvetica and
/// the XObjects `xobjects`.
fn one_page(
    pdf: &mut Pdf,
    media: [i64; 4],
    content: Object,
    xobjects: lopdf::Dictionary,
) -> Vec<u8> {
    let pages = pdf.new_object_id();
    let font = pdf.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
    });
    let page = pdf.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages, "Contents" => content,
        "MediaBox" => media.map(Object::from).to_vec(),
        "Resources" => dictionary! {
            "Font" => dictionary! { "F1" => font },
            "XObject" => xobjects,
        },
    });
    pdf.objects.insert(
        pages,
        dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 }.into(),
    );
    let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
    pdf.trailer.set("Root", catalog);
    let mut bytes = Vec::new();
    pdf.save_to(&mut bytes).unwrap();
    bytes
}

/// Page 1 of `bytes`, `width` pixels wide, as it shows.
fn render_at(bytes: Vec<u8>, width: u32) -> Raster {
    let engine = PdfiumEngine::open_bytes(Arc::from(bytes), None).unwrap();
    engine
        .render(
            PageIndex(0),
            Job::Page {
                width,
                rotation: Rotation::R0,
            },
            PixelFormat::Rgba,
        )
        .unwrap()
}

fn render(bytes: Vec<u8>) -> Raster {
    render_at(bytes, 400)
}

#[test]
fn a_stamp_of_several_streams_draws_as_its_page() {
    // Three streams, each ending with no white-space, that carry state
    // from one to the next: a `q` and its `cm` and color, then a text
    // object begun with its font and place.
    let mut collection = Pdf::with_version("1.7");
    let streams: Vec<Object> = [
        &b"q 0 0 1 rg 1 0 0 1 60 60 cm"[..],
        b"0 0 120 30 re f Q BT /F1 20 Tf 1 0 0 rg 60 110 Td",
        b"(Stamp) Tj ET",
    ]
    .into_iter()
    .map(|s| {
        collection
            .add_object(Stream::new(dictionary! {}, s.to_vec()))
            .into()
    })
    .collect();
    let page = one_page(&mut collection, BOX, streams.into(), dictionary! {});
    let collection = Pdf::load_mem(&page).unwrap();

    let stamp = Pdf::load_mem(&stamp_lib::one_page(&collection, 0).unwrap()).unwrap();
    let mut placed = Pdf::with_version("1.7");
    let form = stamp_lib::page_form(&stamp, &mut placed).unwrap();
    let size = form.geom.shown();
    assert_eq!((size.width, size.height), (200.0, 100.0));
    let content = placed.add_object(Stream::new(dictionary! {}, b"/St Do".to_vec()));
    let placed = one_page(
        &mut placed,
        PLACED,
        content.into(),
        dictionary! { "St" => form.id },
    );

    let (page, placed) = (render(page), render(placed));
    assert_eq!((page.width, page.height), (placed.width, placed.height));
    // The rectangle is blue, so the first stream's state reached the
    // second; and the stamp draws as its page.
    let at = |x: u32, y: u32| {
        let i = ((y * page.width + x) * 4) as usize;
        &page.data[i..i + 3]
    };
    assert_eq!(at(60, 160), [0, 0, 255]);
    assert!(
        page.data.chunks(4).any(|p| p[0] > 200 && p[1] < 60),
        "red text"
    );
    assert!(page.data == placed.data, "the stamp draws as its page");
}

/// A stamp page cropped off the origin and turned a quarter, placed as a
/// page stamp and saved, draws where PDFium shows the page: the mark's
/// box, rendered from the saved file's appearance, is PDFium's render of
/// the stamp page scaled into it (M10.5 bug 19). The page shows 100 wide
/// and 200 tall: green, with a blue quarter at its crop box's bottom left
/// and a red one at its top right, and a black bar outside the crop box,
/// which doesn't show.
#[test]
fn a_stamp_from_a_cropped_turned_page_lands_where_it_shows() {
    let mut stamp = Pdf::with_version("1.7");
    let pages = stamp.new_object_id();
    let content = stamp.add_object(Stream::new(
        dictionary! {},
        b"0 g 0 0 300 40 re f 0 0.6 0 rg 50 60 200 100 re f \
          0 0 1 rg 50 60 100 50 re f 1 0 0 rg 150 110 100 50 re f"
            .to_vec(),
    ));
    let page = stamp.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages, "Contents" => content,
        "MediaBox" => vec![0.into(), 0.into(), 300.into(), 200.into()],
        "CropBox" => vec![50.into(), 60.into(), 250.into(), 160.into()],
        "Rotate" => 90,
    });
    stamp.objects.insert(
        pages,
        dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 }.into(),
    );
    let catalog = stamp.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
    stamp.trailer.set("Root", catalog);
    let mut stamp_bytes = Vec::new();
    stamp.save_to(&mut stamp_bytes).unwrap();

    // Placed in the box 50–150 across, 50–250 up, on a page 300 by 400.
    let blank = generated::blank_pdf(300.0, 400.0).unwrap();
    let store = Store::new();
    let primary = SourceInfo {
        src: store
            .open(blank.into(), BlobKind::file("blank.pdf".as_ref(), None))
            .unwrap(),
        name: "blank.pdf".into(),
        derived: None,
    };
    let mut doc = Document::new(store, primary, &[], Box::new(FixedCount(10)));
    doc.assets()
        .intern_stamp("turned", stamp_bytes.as_slice().into());
    let annot = Annot {
        id: AnnotId(0),
        kind: AnnotKind::PageStamp {
            rect: Box2 {
                x: 50.0,
                y: 50.0,
                w: 100.0,
                h: 200.0,
            },
            stamp: "turned".into(),
            name: "Turned".into(),
            text: None,
        },
        color: [0, 0, 0],
        opacity: 1.0,
        stroke: 1.0,
        author: "test".into(),
        contents: String::new(),
        modified: String::new(),
        created: String::new(),
        fill: None,
        dashed: false,
        cloudy: false,
        turn: Rotation::R0,
        name: String::new(),
    };
    let page = doc.page(0).unwrap().id;
    doc.apply(Edit::AnnotAdd { page, annot }, "Stamp").unwrap();
    let saved = sheaf_doc::save::rewrite(&doc.snapshot()).unwrap();

    // A pixel a point, each.
    let shown = render_at(stamp_bytes, 100);
    assert_eq!((shown.width, shown.height), (100, 200));
    let placed = render_appearances(&saved, None, PageIndex(0), 300).unwrap();
    assert_eq!((placed.width, placed.height), (300, 400));
    let pixel = |r: &Raster, x: u32, y: u32| -> [u8; 3] {
        let i = ((y * r.width + x) * 4) as usize;
        [r.data[i], r.data[i + 1], r.data[i + 2]]
    };
    // The box's top left, y down: 50 across, 400 − 250 down.
    let (left, top) = (50, 150);
    let mut differ = 0;
    for y in 0..shown.height {
        for x in 0..shown.width {
            let (a, b) = (pixel(&shown, x, y), pixel(&placed, left + x, top + y));
            if a.iter().zip(b).any(|(a, b)| a.abs_diff(b) > 8) {
                differ += 1;
            }
        }
    }
    assert_eq!(differ, 0, "pixels differ in the mark's box");
    // Turned a quarter clockwise, the crop box's bottom left shows at the
    // top left, and its top right at the bottom right.
    assert_eq!(pixel(&shown, 20, 20), [0, 0, 255]);
    assert_eq!(pixel(&shown, 80, 180), [255, 0, 0]);
    assert_eq!(pixel(&placed, left + 20, top + 20), [0, 0, 255]);
}
