//! The page table and what a source says of itself: pages numbered and
//! sized as PDFium numbers and sizes them, a hostile tree a typed error
//! within the caps, and the conformance claims in its XMP.

use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use lopdf::{Object, ObjectId};
use sheaf_core::{PageSize, Rotation};

use super::lazy_tests::{QUICK, catalog, stream, with_table};
use super::{Conformance, Limits, OpenError, PageTreeError, PageTreeLimits, PdfA};
use crate::geom::Rect;
use crate::store::{BlobKind, Src, Store};

fn object(number: u32, body: &str) -> (u32, Vec<u8>) {
    (number, body.as_bytes().to_vec())
}

/// A file of `objects`, the first the catalog, opened as a source under
/// `limits`.
fn open_with(objects: &[(u32, Vec<u8>)], limits: Limits) -> Result<Src, OpenError> {
    let kind = BlobKind::file(Path::new("t.pdf"), None);
    Store::with_limits(limits).open(with_table(objects).into(), kind)
}

fn open(objects: &[(u32, Vec<u8>)]) -> Result<Src, OpenError> {
    open_with(objects, Limits::DEFAULT)
}

fn tree_error(result: Result<Src, OpenError>) -> PageTreeError {
    match result {
        Err(OpenError::PageTree(e)) => e,
        Err(e) => panic!("not a page tree error: {e}"),
        Ok(src) => panic!("opened, {} pages", src.page_count()),
    }
}

fn ids(src: &Src) -> Vec<(ObjectId, u32)> {
    (0..src.page_count())
        .map(|i| src.page(i).unwrap())
        .map(|p| (p.id, p.repeat))
        .collect()
}

fn size(width: f32, height: f32) -> PageSize {
    PageSize { width, height }
}

#[test]
fn cyclic_kids_are_a_typed_error() {
    let started = Instant::now();
    let page = object(4, "<< /Type /Page /Parent 2 0 R >>");
    // 2 lists 3, which lists 2.
    let cycle = [
        catalog(),
        object(2, "<< /Type /Pages /Kids [4 0 R 3 0 R] /Count 2 >>"),
        object(3, "<< /Type /Pages /Parent 2 0 R /Kids [2 0 R] /Count 1 >>"),
        page.clone(),
    ];
    assert_eq!(tree_error(open(&cycle)), PageTreeError::Cycle);
    // The root lists itself.
    let itself = [
        catalog(),
        object(2, "<< /Type /Pages /Kids [4 0 R 2 0 R] /Count 2 >>"),
        page,
    ];
    assert_eq!(tree_error(open(&itself)), PageTreeError::Cycle);
    assert!(started.elapsed() < QUICK);
}

#[test]
fn a_parent_loop_is_a_typed_error() {
    // The page's /Parent is 3, 3's is 4, and 4's is 3 again.
    let objects = [
        catalog(),
        object(2, "<< /Type /Pages /Kids [5 0 R] /Count 1 >>"),
        object(3, "<< /Type /Pages /Parent 4 0 R /Kids [5 0 R] >>"),
        object(4, "<< /Type /Pages /Parent 3 0 R /Kids [3 0 R] >>"),
        object(5, "<< /Type /Page /Parent 3 0 R >>"),
    ];
    assert_eq!(tree_error(open(&objects)), PageTreeError::ParentLoop);
    // A page that is its own /Parent.
    let objects = [
        catalog(),
        object(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
        object(3, "<< /Type /Page /Parent 3 0 R >>"),
    ];
    assert_eq!(tree_error(open(&objects)), PageTreeError::ParentLoop);
}

/// A chain of `depth` nodes from the root (2) down, each listing the next
/// and naming the one above as its `/Parent`, and a page under the last.
fn chain(depth: u32) -> Vec<(u32, Vec<u8>)> {
    let mut objects = vec![catalog()];
    for n in 2..2 + depth {
        let parent = if n == 2 {
            String::new()
        } else {
            format!("/Parent {} 0 R ", n - 1)
        };
        objects.push(object(
            n,
            &format!("<< /Type /Pages {parent}/Kids [{} 0 R] /Count 1 >>", n + 1),
        ));
    }
    let last = 1 + depth;
    objects.push(object(
        last + 1,
        &format!("<< /Type /Page /Parent {last} 0 R /MediaBox [0 0 100 200] >>"),
    ));
    objects
}

#[test]
fn a_10000_deep_page_tree_is_a_typed_error() {
    let started = Instant::now();
    assert_eq!(tree_error(open(&chain(10_000))), PageTreeError::TooDeep);
    assert!(started.elapsed() < QUICK, "{:?}", started.elapsed());
    // PDFium's 1,024 levels open, and the page inherits down its 1,024
    // parents.
    let deepest = open(&chain(1024)).unwrap();
    assert_eq!(deepest.page_size(0), size(100.0, 200.0));
    assert_eq!(deepest.page(0).unwrap().parents().count(), 1024);
    assert_eq!(tree_error(open(&chain(1025))), PageTreeError::TooDeep);

    // A shallow tree whose page names a /Parent 10,000 nodes from the top.
    let mut objects = vec![
        catalog(),
        object(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
        object(3, "<< /Type /Page /Parent 4 0 R >>"),
    ];
    for n in 4..10_004 {
        objects.push(object(
            n,
            &format!("<< /Type /Pages /Parent {} 0 R >>", n + 1),
        ));
    }
    let started = Instant::now();
    assert_eq!(tree_error(open(&objects)), PageTreeError::TooDeep);
    assert!(started.elapsed() < QUICK, "{:?}", started.elapsed());
}

#[test]
fn too_many_pages_or_none_are_typed_errors() {
    let limits = Limits {
        page_tree: PageTreeLimits {
            pages: 2,
            ..Limits::DEFAULT.page_tree
        },
        ..Limits::DEFAULT
    };
    let three = [
        catalog(),
        object(2, "<< /Type /Pages /Kids [3 0 R 3 0 R 3 0 R] /Count 3 >>"),
        object(3, "<< /Type /Page /Parent 2 0 R >>"),
    ];
    assert_eq!(
        open_with(&three, limits).unwrap_err().to_string(),
        "the page tree lists too many pages"
    );
    assert_eq!(open(&three).unwrap().page_count(), 3);
    // A node listing the next twice, 20 deep: a million visits the walk
    // stops at its cap.
    let mut objects = vec![catalog()];
    for n in 2..22 {
        objects.push(object(
            n,
            &format!("<< /Type /Pages /Kids [{0} 0 R {0} 0 R] >>", n + 1),
        ));
    }
    objects.push(object(22, "<< /Type /Pages /Kids [] >>"));
    let started = Instant::now();
    assert_eq!(
        tree_error(open_with(&objects, limits)),
        PageTreeError::TooManyPages
    );
    assert!(started.elapsed() < QUICK);

    let empty = [catalog(), object(2, "<< /Type /Pages /Kids [] /Count 0 >>")];
    assert_eq!(tree_error(open(&empty)), PageTreeError::NoPages);
    let no_tree = [object(1, "<< /Type /Catalog >>")];
    assert_eq!(tree_error(open(&no_tree)), PageTreeError::NoPages);
}

#[test]
fn a_page_listed_twice_is_two_pages() {
    let objects = [
        catalog(),
        object(
            2,
            "<< /Type /Pages /Kids [3 0 R 4 0 R 3 0 R 5 0 R] /Count 5 >>",
        ),
        object(3, "<< /Type /Page /Parent 2 0 R >>"),
        object(4, "<< /Type /Page /Parent 2 0 R >>"),
        // A node listed twice lists its page twice, as PDFium walks it.
        object(
            5,
            "<< /Type /Pages /Parent 2 0 R /Kids [6 0 R 6 0 R] /Count 2 >>",
        ),
        object(6, "<< /Type /Page /Parent 5 0 R >>"),
    ];
    let src = open(&objects).unwrap();
    assert_eq!(
        ids(&src),
        [
            ((3, 0), 0),
            ((4, 0), 0),
            ((3, 0), 1),
            ((6, 0), 0),
            ((6, 0), 1)
        ]
    );
    // A kid with /Kids is a node whatever its /Type, and any other a page.
    // A kid that names no dictionary keeps its place as a Letter page, as
    // PDFium and lopdf's get_pages keep it; one written in place, which
    // has no object to be, is skipped, as get_pages skips it.
    let objects = [
        catalog(),
        object(
            2,
            "<< /Type /Pages /Kids [3 0 R 4 0 R 9 0 R << /Type /Page >> 5 0 R] /Count 4 >>",
        ),
        object(3, "<< /Type /Page /Kids [4 0 R] >>"),
        object(4, "<< /Type /Pages /Parent 2 0 R /MediaBox [0 0 10 10] >>"),
        object(5, "[1 2 3]"),
    ];
    let src = open(&objects).unwrap();
    assert_eq!(
        ids(&src),
        [((4, 0), 0), ((4, 0), 1), ((9, 0), 0), ((5, 0), 0)]
    );
    assert_eq!(src.page_size(1), size(10.0, 10.0));
    assert_eq!(src.page_size(2), size(612.0, 792.0));
    assert!(src.page(3).unwrap().parent.is_none());
    // A root without /Kids is the one page.
    let one = [
        catalog(),
        object(2, "<< /Type /Page /MediaBox [0 0 10 20] >>"),
    ];
    let src = open(&one).unwrap();
    assert_eq!(ids(&src), [((2, 0), 0)]);
    assert_eq!(src.page_size(0), size(10.0, 20.0));
    // Past the last page, Letter.
    assert_eq!(src.page_size(1), size(612.0, 792.0));
}

/// The root's `/Kids`, `n` pages from object `first` on, under a root
/// with `keys`.
fn root(first: u32, n: u32, keys: &str) -> (u32, Vec<u8>) {
    let kids: String = (first..first + n).map(|k| format!("{k} 0 R ")).collect();
    object(2, &format!("<< /Type /Pages /Kids [{kids}] {keys} >>"))
}

#[test]
fn pages_share_what_they_hold_however_often_it_s_listed() {
    // 1,000 pages naming one /Annots array of 10,000 entries hold it once.
    let mut objects = vec![
        catalog(),
        root(4, 1000, ""),
        object(3, &format!("[{}]", "10 0 R ".repeat(10_000))),
    ];
    for n in 4..1004 {
        objects.push(object(n, "<< /Type /Page /Parent 2 0 R /Annots 3 0 R >>"));
    }
    let started = Instant::now();
    let src = open(&objects).unwrap();
    assert!(started.elapsed() < QUICK, "{:?}", started.elapsed());
    let annots = &src.page(0).unwrap().annots;
    assert_eq!((src.page_count(), annots.len()), (1000, 10_000));
    assert!(src.pages().iter().all(|p| Arc::ptr_eq(&p.annots, annots)));

    // The root lists a node 300 times, and the node its page 300 times:
    // 90,000 pages, whose own /Annots, /Contents and /Resources, written
    // in place, are held once.
    let objects = [
        catalog(),
        object(
            2,
            &format!("<< /Type /Pages /Kids [{}] >>", "3 0 R ".repeat(300)),
        ),
        object(
            3,
            &format!(
                "<< /Type /Pages /Parent 2 0 R /Kids [{}] >>",
                "4 0 R ".repeat(300)
            ),
        ),
        object(
            4,
            &format!(
                "<< /Type /Page /Parent 3 0 R /Annots [{}] /Contents [5 0 R 6 0 R] \
                 /Resources << /Font << /F1 7 0 R >> >> >>",
                "10 0 R ".repeat(100)
            ),
        ),
    ];
    let started = Instant::now();
    let src = open(&objects).unwrap();
    assert!(started.elapsed() < QUICK, "{:?}", started.elapsed());
    assert_eq!(src.page_count(), 90_000);
    let first = src.page(0).unwrap();
    for (i, page) in src.pages().iter().enumerate() {
        assert_eq!((page.id, page.repeat), ((4, 0), i as u32));
        assert!(Arc::ptr_eq(&page.annots, &first.annots));
        assert!(Arc::ptr_eq(&page.contents, &first.contents));
        let resources = (page.resources.as_ref(), first.resources.as_ref());
        assert!(Arc::ptr_eq(resources.0.unwrap(), resources.1.unwrap()));
    }
}

#[test]
fn a_broken_object_every_page_names_is_read_once() {
    // 1,000 pages whose /MediaBox is object 3, an array of 300,000
    // numbers that never closes: parsed once, not once a page.
    let mut objects = vec![
        catalog(),
        root(4, 1000, ""),
        object(3, &format!("[{}", "1 ".repeat(300_000))),
    ];
    for n in 4..1004 {
        objects.push(object(n, "<< /Type /Page /Parent 2 0 R /MediaBox 3 0 R >>"));
    }
    let started = Instant::now();
    let src = open(&objects).unwrap();
    assert!(started.elapsed() < QUICK, "{:?}", started.elapsed());
    assert_eq!(src.page_count(), 1000);
    assert_eq!(src.page_size(999), size(612.0, 792.0));
    // And it fails as it failed the first time.
    for _ in 0..2 {
        assert!(matches!(
            src.pdf().get((3, 0)),
            Err(lopdf::Error::IndirectObject { .. })
        ));
    }
}

#[test]
fn the_root_s_count_ends_the_table_as_it_ends_pdfium_s() {
    let tree = |count: &str| {
        let mut objects = vec![catalog(), root(3, 3, &format!("/Count {count}"))];
        for n in 3..6 {
            objects.push(object(n, "<< /Type /Page /Parent 2 0 R >>"));
        }
        objects.push(object(9, "1"));
        objects
    };
    // PDFium loads no page past a /Count it counts by, a real's whole part
    // and a reference followed.
    for (count, pages) in [("2", 2), ("2.9", 2), ("9 0 R", 1)] {
        assert_eq!(open(&tree(count)).unwrap().page_count(), pages, "{count}");
    }
    // Past the tree, or one PDFium doesn't count by, the tree counts.
    for count in ["5", "0", "-1", "1048575", "(2)"] {
        assert_eq!(open(&tree(count)).unwrap().page_count(), 3, "{count}");
    }
    // The walk stops there, so a cycle past it isn't met, as PDFium
    // doesn't meet it.
    let objects = [
        catalog(),
        object(2, "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 1 >>"),
        object(3, "<< /Type /Page /Parent 2 0 R >>"),
        object(4, "<< /Type /Pages /Parent 2 0 R /Kids [2 0 R] >>"),
    ];
    assert_eq!(ids(&open(&objects).unwrap()), [((3, 0), 0)]);
}

#[test]
fn inherited_boxes_and_rotation_read_as_pdfium_reads_them() {
    let objects = [
        catalog(),
        object(
            2,
            "<< /Type /Pages /Kids [3 0 R 10 0 R 11 0 R 12 0 R 13 0 R 14 0 R 15 0 R] \
             /MediaBox 20 0 R /Rotate 135 >>",
        ),
        object(
            3,
            "<< /Type /Pages /Parent 2 0 R /Kids [4 0 R 5 0 R] /CropBox [50 50 550 750] >>",
        ),
        // Everything inherited: a crop of 500 × 700, a quarter turn.
        object(4, "<< /Type /Page /Parent 3 0 R >>"),
        // -90 is 270; its own media box, corners reversed, still clips the
        // crop it inherits.
        object(
            5,
            "<< /Type /Page /Parent 3 0 R /Rotate -90 /MediaBox [300 400 0 0] >>",
        ),
        // 45 is no turn; an empty media box is Letter, and the crop is
        // clipped to it.
        object(
            10,
            "<< /Type /Page /Parent 2 0 R /Rotate 45 /MediaBox [0 0 0 0] /CropBox [-100 -100 300 21 0 R] >>",
        ),
        // A crop outside the media box leaves nothing shown, as in PDFium.
        object(
            11,
            "<< /Type /Page /Parent 2 0 R /Rotate 0 /CropBox [700 0 900 100] >>",
        ),
        // UserUnit is kept, not applied; the other boxes are the page's own.
        object(
            12,
            "<< /Type /Page /Parent 2 0 R /Rotate 0 /UserUnit 2.5 /MediaBox [0 0 100 200] \
             /TrimBox [90 190 10 10] /BleedBox [0 0 100 200] /ArtBox [1 2 3] >>",
        ),
        // Inherited by /Parent, not by the path that listed it: from 3.
        object(13, "<< /Type /Page /Parent 3 0 R /Rotate 0 >>"),
        // A media box that isn't four numbers is Letter; a /Rotate that's
        // a real turns by its whole part.
        object(
            14,
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612] /Rotate 180.9 >>",
        ),
        // A null /CropBox is found, as in PDFium, and isn't a box: 3's
        // crop isn't inherited past it, and the crop is the media box.
        object(15, "<< /Type /Page /Parent 3 0 R /CropBox null >>"),
        object(20, "[0 0 600 800]"),
        object(21, "300"),
    ];
    let src = open(&objects).unwrap();
    let geom = |i| src.page(i).unwrap().geom;
    let shown = |i| src.page_size(i);
    assert_eq!(src.page_count(), 8);

    assert_eq!(geom(0).media, Rect::new(0.0, 0.0, 600.0, 800.0));
    assert_eq!(geom(0).crop, Rect::new(50.0, 50.0, 550.0, 750.0));
    assert_eq!(geom(0).rotate, Rotation::R90);
    assert_eq!(shown(0), size(700.0, 500.0));

    assert_eq!(geom(1).rotate, Rotation::R270);
    assert_eq!(geom(1).visible(), Rect::new(50.0, 50.0, 300.0, 400.0));
    assert_eq!(shown(1), size(350.0, 250.0));

    assert_eq!(geom(2).rotate, Rotation::R0);
    assert_eq!(geom(2).media, Rect::LETTER);
    assert_eq!(geom(2).crop, Rect::new(0.0, 0.0, 300.0, 300.0));
    assert_eq!(shown(2), size(300.0, 300.0));

    assert_eq!(geom(3).crop, Rect::default());
    assert_eq!(shown(3), size(0.0, 0.0));

    let g = geom(4);
    assert_eq!((g.user_unit, shown(4)), (2.5, size(100.0, 200.0)));
    assert_eq!(g.trim, Some(Rect::new(10.0, 10.0, 90.0, 190.0)));
    assert_eq!(g.bleed, Some(Rect::new(0.0, 0.0, 100.0, 200.0)));
    assert_eq!(g.art, None);
    assert_eq!(geom(0).user_unit, 1.0);
    assert_eq!(geom(0).trim, None);

    assert_eq!(geom(5).crop, Rect::new(50.0, 50.0, 550.0, 750.0));
    assert_eq!(shown(5), size(500.0, 700.0));

    assert_eq!(
        (geom(6).media, geom(6).rotate),
        (Rect::LETTER, Rotation::R180)
    );
    assert_eq!(geom(7).crop, Rect::new(0.0, 0.0, 600.0, 800.0));
    assert_eq!(geom(7).rotate, Rotation::R90);
}

#[test]
fn per_page_annots_are_listed() {
    let objects = [
        catalog(),
        object(
            2,
            "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Resources 9 0 R >>",
        ),
        object(
            3,
            "<< /Type /Page /Parent 2 0 R /Annots [10 0 R 11 0 R] /Contents 20 0 R /StructParents 7 >>",
        ),
        object(
            4,
            "<< /Type /Page /Parent 2 0 R /Annots 12 0 R /Contents [21 0 R 22 0 R] \
             /Resources << /Font << >> >> >>",
        ),
        object(5, "<< /Type /Page /Parent 2 0 R /Annots 99 0 R >>"),
        object(9, "<< /Font << >> >>"),
        object(12, "[13 0 R << /Subtype /Link >>]"),
    ];
    let src = open(&objects).unwrap();
    let page = |i| src.page(i).unwrap();
    assert_eq!(
        &*page(0).annots,
        [Object::Reference((10, 0)), Object::Reference((11, 0))]
    );
    // An indirect /Annots is followed; a direct entry is kept as written.
    assert_eq!(page(1).annots.len(), 2);
    assert_eq!(page(1).annots[0], Object::Reference((13, 0)));
    assert!(page(1).annots[1].as_dict().is_ok());
    // One that names nothing lists none.
    assert!(page(2).annots.is_empty());

    assert_eq!(&*page(0).contents, [(20, 0)]);
    assert_eq!(&*page(1).contents, [(21, 0), (22, 0)]);
    assert!(page(2).contents.is_empty());
    assert_eq!(page(0).struct_parents, Some(7));
    assert_eq!(page(1).struct_parents, None);
    assert_eq!(
        page(0).resources.as_deref(),
        Some(&Object::Reference((9, 0)))
    );
    assert!(page(1).resources.as_ref().unwrap().as_dict().is_ok());
    assert_eq!(page(0).parents().collect::<Vec<_>>(), [(2, 0)]);
    // Pages under one node share its chain.
    assert!(std::sync::Arc::ptr_eq(
        page(0).parent.as_ref().unwrap(),
        page(2).parent.as_ref().unwrap()
    ));
}

/// A file whose catalog has XMP `xml` as its `/Metadata`, deflated when
/// `deflate`.
fn with_xmp(xml: &str, deflate: bool) -> Result<Src, OpenError> {
    let (filter, data) = if deflate {
        let mut d = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        d.write_all(xml.as_bytes()).unwrap();
        ("/Filter /FlateDecode ", d.finish().unwrap())
    } else {
        ("", xml.as_bytes().to_vec())
    };
    let metadata = stream(
        &format!(
            "/Type /Metadata /Subtype /XML {filter}/Length {}",
            data.len()
        ),
        &data,
    );
    open(&[
        object(1, "<< /Type /Catalog /Pages 2 0 R /Metadata 4 0 R >>"),
        object(2, "<< /Type /Pages /Kids [3 0 R] >>"),
        object(3, "<< /Type /Page /Parent 2 0 R >>"),
        (4, metadata),
    ])
}

fn xmp(description: &str) -> String {
    format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\
         <x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\
         <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
         {description}</rdf:RDF></x:xmpmeta><?xpacket end=\"w\"?>"
    )
}

#[test]
fn conformance_reads_pdfa_and_pdfua_claims() {
    let claims =
        |description: &str, deflate| with_xmp(&xmp(description), deflate).unwrap().conformance();
    // Attributes.
    let attributes = "<rdf:Description rdf:about=\"\" \
         xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\" pdfaid:part=\"2\" pdfaid:conformance=\"b\"/>";
    let pdfa_2b = Conformance {
        pdfa: Some(PdfA {
            part: 2,
            level: Some('B'),
        }),
        pdfua: None,
    };
    assert_eq!(claims(attributes, false).unwrap(), pdfa_2b);
    assert_eq!(claims(attributes, true).unwrap(), pdfa_2b);
    // Elements, under any prefix, and PDF/UA beside PDF/A.
    let elements = "<rdf:Description rdf:about=\"\" xmlns:a=\"http://www.aiim.org/pdfa/ns/id/\">\
         <a:part>3</a:part><a:conformance>A</a:conformance></rdf:Description>\
         <rdf:Description rdf:about=\"\" xmlns:pdfuaid=\"http://www.aiim.org/pdfua/ns/id/\">\
         <pdfuaid:part> 1 </pdfuaid:part></rdf:Description>";
    assert_eq!(
        claims(elements, false).unwrap(),
        Conformance {
            pdfa: Some(PdfA {
                part: 3,
                level: Some('A'),
            }),
            pdfua: Some(1),
        }
    );
    // PDF/A-4 gives no level; a part in another namespace is no claim.
    let pdfa_4 = "<rdf:Description xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\" \
         xmlns:o=\"http://example.com/other/\" pdfaid:part=\"4\" o:part=\"9\"><o:part>8</o:part>\
         </rdf:Description>";
    assert_eq!(
        claims(pdfa_4, false).unwrap(),
        Conformance {
            pdfa: Some(PdfA {
                part: 4,
                level: None,
            }),
            pdfua: None,
        }
    );
    assert_eq!(claims("", false).unwrap(), Conformance::default());
    let unreadable =
        "<rdf:Description xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\" pdfaid:part=\"two\"/>";
    assert!(claims(unreadable, false).is_err());
    // No /Metadata, no claims.
    let src = open(&[
        catalog(),
        object(2, "<< /Type /Pages /Kids [3 0 R] >>"),
        object(3, "<< /Type /Page /Parent 2 0 R >>"),
    ])
    .unwrap();
    assert_eq!(src.conformance().unwrap(), Conformance::default());
}
