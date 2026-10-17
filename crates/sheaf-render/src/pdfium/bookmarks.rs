//! PDFium's bookmark walk, which the app read its outline with until
//! M10.5 slice 4 read it from the source (`sheaf_doc::open::outline`),
//! kept as the reference that reader is checked against: positions
//! sheaf-doc writes come back where they were put through both, and over
//! Greg's corpus the two read the same outline.

use std::path::Path;

use lopdf::{Document as Pdf, Object, Stream, dictionary};
use sheaf_core::DestView;
use sheaf_doc::forms::corpus::{Corpus, Entry, each_entry};
use sheaf_doc::store::BlobKind;
use sheaf_doc::{Document, FixedCount, FlatOutlineItem, SourceInfo, Store};

use super::*;

const MAX_DEPTH: u16 = 32;
const MAX_ITEMS: usize = 200_000;

/// A bookmark as PDFium reads it.
#[derive(Debug)]
struct Bookmark {
    title: String,
    page: Option<u32>,
    depth: u16,
    /// Where on `page` it lands: an `/XYZ` position in PageSpace, or
    /// another destination kind, as the file has them.
    view: Option<DestView>,
}

/// `engine`'s bookmark tree, flattened in pre-order, read under one
/// hold of the lock.
fn bookmarks(engine: &PdfiumEngine) -> Vec<Bookmark> {
    let _pdfium = lock();
    let root = engine.doc.bookmarks().root();
    if root.is_some() {
        // A destination's page index walks the page tree for each page
        // PDFium hasn't loaded yet, so load every page's dictionary
        // first: 60 ms at 7,000 pages, where 2,000 bookmarks took 1.2 s
        // without.
        for i in 0..engine.doc.pages().len() {
            let _ = engine.doc.pages().page_size(i);
        }
    }
    let mut out = Vec::new();
    walk(root, 0, &mut out);
    out
}

fn walk(first: Option<PdfBookmark<'_>>, depth: u16, out: &mut Vec<Bookmark>) {
    let mut node = first;
    while let Some(b) = node {
        if out.len() >= MAX_ITEMS {
            return;
        }
        let (page, view) = locate(&b);
        out.push(Bookmark {
            title: b.title().unwrap_or_default(),
            page,
            depth,
            view,
        });
        if depth < MAX_DEPTH {
            walk(b.first_child(), depth + 1, out);
        }
        node = b.next_sibling();
    }
}

/// The bookmark's page and position, from its `/Dest` or its GoTo
/// action.
fn locate(b: &PdfBookmark<'_>) -> (Option<u32>, Option<DestView>) {
    let from = |d: &PdfDestination<'_>| {
        let page = d.page_index().ok().map(|i| i as u32);
        let view = page.and_then(|_| view_of(d));
        (page, view)
    };
    if let Some(d) = b.destination() {
        return from(&d);
    }
    let Some(action) = b.action() else {
        return (None, None);
    };
    let Some(local) = action.as_local_destination_action() else {
        return (None, None);
    };
    match local.destination() {
        Ok(d) => from(&d),
        Err(_) => (None, None),
    }
}

/// Where a destination lands, in PDFium's user-space coordinates.
fn view_of(d: &PdfDestination<'_>) -> Option<DestView> {
    use PdfDestinationViewSettings as V;
    let pt = |p: Option<PdfPoints>| p.map(|p| p.value);
    let other = |kind: &str, params: Vec<Option<f32>>| {
        Some(DestView::Other {
            kind: kind.into(),
            params,
        })
    };
    match d.view_settings().ok()? {
        V::SpecificCoordinatesAndZoom(x, y, zoom) => {
            let (left, top) = (pt(x), pt(y));
            if left.is_none() && top.is_none() && zoom.is_none() {
                return None;
            }
            Some(DestView::Xyz { left, top, zoom })
        }
        V::FitPageToWindow => other("Fit", vec![]),
        V::FitPageHorizontallyToWindow(y) => other("FitH", vec![pt(y)]),
        V::FitPageVerticallyToWindow(x) => other("FitV", vec![pt(x)]),
        V::FitPageToRectangle(r) => other(
            "FitR",
            vec![
                Some(r.left().value),
                Some(r.bottom().value),
                Some(r.right().value),
                Some(r.top().value),
            ],
        ),
        V::FitBoundsToWindow => other("FitB", vec![]),
        V::FitBoundsHorizontallyToWindow(y) => other("FitBH", vec![pt(y)]),
        V::FitBoundsVerticallyToWindow(x) => other("FitBV", vec![pt(x)]),
        V::Unknown => None,
    }
}

/// Two Letter pages: the first unrotated, the second with `/Rotate 90`.
fn two_pages() -> Vec<u8> {
    let mut pdf = Pdf::with_version("1.5");
    let pages_id = pdf.new_object_id();
    let mut kids = Vec::new();
    for rotate in [0, 90] {
        let content = pdf.add_object(Stream::new(dictionary! {}, Vec::new()));
        let mut page = dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => content,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        };
        if rotate != 0 {
            page.set("Rotate", rotate);
        }
        kids.push(Object::Reference(pdf.add_object(page)));
    }
    pdf.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => 2 }),
    );
    let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    pdf.trailer.set("Root", catalog);
    let mut bytes = Vec::new();
    pdf.save_to(&mut bytes).unwrap();
    bytes
}

fn xyz(left: f32, top: f32) -> Option<DestView> {
    Some(DestView::Xyz {
        left: Some(left),
        top: Some(top),
        zoom: None,
    })
}

/// Bookmark positions written by sheaf-doc come back where they were put,
/// in PageSpace whatever the file's own `/Rotate`, both through PDFium and
/// from the source; other destination kinds come back as they were.
#[test]
fn positions_round_trip_through_pdfium_and_the_source() {
    let item = |title: &str, page, view| FlatOutlineItem {
        title: title.into(),
        page: Some(page),
        depth: 0,
        view,
        color: None,
        style: Default::default(),
        open: true,
    };
    let fit_h = Some(DestView::Other {
        kind: "FitH".into(),
        params: vec![Some(500.0)],
    });
    let store = Store::new();
    let src = store
        .open(
            two_pages().into(),
            BlobKind::file(Path::new("src.pdf"), None),
        )
        .unwrap();
    let primary = SourceInfo {
        src,
        name: "src.pdf".into(),
        derived: None,
    };
    let model = Document::new(
        store,
        primary,
        &[
            item("Upright", 0, xyz(36.0, 100.0)),
            // PageSpace on a /Rotate 90 page is still 612 wide, 792 tall.
            item("Turned", 1, xyz(400.0, 50.0)),
            item("Fit width", 1, fit_h.clone()),
            item("Top", 0, None),
        ],
        Box::new(FixedCount(5)),
    );
    let out = sheaf_doc::save::rewrite(&model.snapshot()).unwrap();

    let engine = PdfiumEngine::open_bytes(out.clone().into(), None).unwrap();
    let through_pdfium: Vec<(String, Option<u32>, Option<DestView>)> = bookmarks(&engine)
        .into_iter()
        .map(|b| (b.title, b.page, b.view))
        .collect();
    let from_source: Vec<(String, Option<u32>, Option<DestView>)> =
        sheaf_doc::open::outline(&sheaf_doc::forms::fixture::source(out))
            .into_iter()
            .map(|i| (i.title, i.page, i.view))
            .collect();
    let close = |v: &Option<DestView>, l: f32, t: f32| match v {
        Some(DestView::Xyz {
            left: Some(a),
            top: Some(b),
            zoom: None,
        }) => (a - l).abs() < 0.01 && (b - t).abs() < 0.01,
        _ => false,
    };
    for got in [through_pdfium, from_source] {
        assert_eq!(got.len(), 4, "{got:?}");
        assert!(close(&got[0].2, 36.0, 100.0), "{got:?}");
        assert!(close(&got[1].2, 400.0, 50.0), "{got:?}");
        assert_eq!(got[2].2, fit_h);
        assert_eq!(got[3].2, None, "a page-only bookmark has no position");
        assert_eq!(
            got.iter().map(|g| g.1).collect::<Vec<_>>(),
            [Some(0), Some(1), Some(1), Some(0)]
        );
    }
}

/// How a corpus file's outline from the source differs from PDFium's.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Differs {
    /// How many items each read.
    Count { source: usize, pdfium: usize },
    /// The first item that differs, and in what.
    Item { index: usize, what: &'static str },
}

/// Why a corpus file's outline from the source isn't PDFium's.
#[derive(Debug, Clone, Copy)]
enum Why {
    /// A GoTo action's `/S` is a reference to the name: the source
    /// follows it to the action's page, PDFium finds no page.
    IndirectActionType,
}

/// The corpus files whose outline from the source isn't PDFium's, how,
/// and why. Every other file PDFium opens reads the same outline.
const DIFFER: &[(&str, Differs, Why)] = &[(
    "forms/safedocs/1260393.pdf",
    Differs::Item {
        index: 0,
        what: "page",
    },
    Why::IndirectActionType,
)];

/// A corpus file's outlines: PDFium's length (None when PDFium doesn't
/// open the file), and how the source's differs.
fn compare(corpus: &Corpus, entry: &Entry) -> (Option<usize>, Option<Differs>) {
    let path = corpus.path(entry);
    let Ok(bytes) = std::fs::read(&path) else {
        return (None, None);
    };
    let bytes: Arc<[u8]> = bytes.into();
    let Ok(engine) = PdfiumEngine::open_bytes(bytes.clone(), None) else {
        return (None, None);
    };
    let pdfium = bookmarks(&engine);
    drop(engine);
    let source = match Store::new().open(bytes, BlobKind::file(&path, None)) {
        Ok(src) => sheaf_doc::open::outline(&src),
        Err(_) => Vec::new(),
    };
    let read = Some(pdfium.len());
    if source.len() != pdfium.len() {
        let differs = Differs::Count {
            source: source.len(),
            pdfium: pdfium.len(),
        };
        return (read, Some(differs));
    }
    let near = |a: Option<f32>, b: Option<f32>| match (a, b) {
        (Some(a), Some(b)) => (a - b).abs() < 0.01,
        (a, b) => a == b,
    };
    let same_view = |a: &Option<DestView>, b: &Option<DestView>| match (a, b) {
        (
            Some(DestView::Xyz { left, top, zoom }),
            Some(DestView::Xyz {
                left: l,
                top: t,
                zoom: z,
            }),
        ) => near(*left, *l) && near(*top, *t) && near(*zoom, *z),
        (Some(DestView::Other { kind, params }), Some(DestView::Other { kind: k, params: p })) => {
            kind == k && params.len() == p.len() && params.iter().zip(p).all(|(a, b)| near(*a, *b))
        }
        (a, b) => a == b,
    };
    for (index, (s, p)) in source.iter().zip(&pdfium).enumerate() {
        let what = if s.title != p.title {
            "title"
        } else if s.depth != p.depth {
            "depth"
        } else if s.page != p.page {
            "page"
        } else if !same_view(&s.view, &p.view) {
            "view"
        } else {
            continue;
        };
        eprintln!("{}: item {index}: {s:?}, PDFium's {p:?}", entry.path);
        return (read, Some(Differs::Item { index, what }));
    }
    (read, None)
}

/// Every corpus file PDFium opens has the outline from the source that
/// PDFium reads: as many items, with the same titles, depths and pages,
/// and the same views, positions within a hundredth of a point. What
/// differs is [`DIFFER`]'s, and only that. Skips without the corpus.
#[test]
fn the_source_outline_is_pdfium_s() {
    let Some(corpus) = Corpus::or_skip() else {
        return;
    };
    let entries = corpus.manifest().unwrap();
    let outcomes = each_entry(&entries, |entry| compare(&corpus, entry));
    let read: Vec<usize> = outcomes.iter().filter_map(|o| o.0).collect();
    eprintln!(
        "{} files: PDFium opens {}, {} with bookmarks, {} bookmarks",
        outcomes.len(),
        read.len(),
        read.iter().filter(|&&n| n > 0).count(),
        read.iter().sum::<usize>(),
    );
    let found: Vec<(&str, Differs)> = entries
        .iter()
        .zip(&outcomes)
        .filter_map(|(entry, &(_, differs))| Some((entry.path.as_str(), differs?)))
        .collect();
    let unlisted: Vec<_> = found
        .iter()
        .filter(|f| !DIFFER.iter().any(|d| d.0 == f.0 && d.1 == f.1))
        .collect();
    assert!(unlisted.is_empty(), "{unlisted:#?}");
    let stale: Vec<_> = DIFFER
        .iter()
        .filter(|d| !found.iter().any(|f| f.0 == d.0))
        .collect();
    assert!(stale.is_empty(), "no longer differ: {stale:#?}");
}
