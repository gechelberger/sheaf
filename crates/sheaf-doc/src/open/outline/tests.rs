//! The outline read from built files: each way a destination is named,
//! positions in displayed space, every view kind, styles, the closed
//! state, and the caps on cycles and depth.

use std::path::Path;

use lopdf::{Dictionary, Object, ObjectId, StringFormat, dictionary};
use sheaf_core::DestView;

use super::*;
use crate::forms::fixture::Form;
use crate::forms::fixture::outline_tree;
use crate::history::FixedCount;
use crate::model::{Document, SourceInfo};
use crate::store::{BlobKind, Store};

fn text(s: &str) -> Object {
    Object::string_literal(s)
}

/// An item titled `title` going to `dest`.
fn to(title: &str, dest: Object) -> Dictionary {
    dictionary! { "Title" => text(title), "Dest" => dest }
}

/// A destination: `page` (a page object or an index), then the rest.
fn dest(page: impl Into<Object>, rest: Vec<Object>) -> Object {
    let mut d = vec![page.into()];
    d.extend(rest);
    Object::Array(d)
}

fn fit(page: ObjectId) -> Object {
    dest(page, vec!["Fit".into()])
}

fn xyz(page: impl Into<Object>, left: Object, top: Object, zoom: Object) -> Object {
    dest(page, vec!["XYZ".into(), left, top, zoom])
}

fn pages(items: &[FlatOutlineItem]) -> Vec<Option<u32>> {
    items.iter().map(|i| i.page).collect()
}

fn titles(items: &[FlatOutlineItem]) -> Vec<&str> {
    items.iter().map(|i| i.title.as_str()).collect()
}

fn at(left: Option<f32>, top: Option<f32>, zoom: Option<f32>) -> Option<DestView> {
    Some(DestView::Xyz { left, top, zoom })
}

/// Within a hundredth of a point, as positions computed in f32 come back.
fn near(got: &Option<DestView>, want: &Option<DestView>) -> bool {
    let close = |a: &Option<f32>, b: &Option<f32>| match (a, b) {
        (Some(a), Some(b)) => (a - b).abs() < 0.01,
        (a, b) => a == b,
    };
    match (got, want) {
        (
            Some(DestView::Xyz { left, top, zoom }),
            Some(DestView::Xyz {
                left: l,
                top: t,
                zoom: z,
            }),
        ) => close(left, l) && close(top, t) && close(zoom, z),
        (got, want) => got == want,
    }
}

/// `f`'s pages with `items` as bookmarks, written by a Rewrite.
fn rewritten(f: &mut Form, items: &[FlatOutlineItem]) -> Vec<u8> {
    let store = Store::new();
    let src = store
        .open(f.bytes().into(), BlobKind::file(Path::new("a.pdf"), None))
        .unwrap();
    let primary = SourceInfo {
        src,
        name: "a.pdf".into(),
        derived: None,
    };
    let doc = Document::new(store, primary, items, Box::new(FixedCount(5)));
    crate::save::rewrite(&doc.snapshot()).unwrap()
}

#[test]
fn an_explicit_dest_names_its_page() {
    let mut f = Form::new(3);
    let p = f.pages.clone();
    let indirect = f.pdf.add_object(fit(p[2]));
    outline_tree(
        &mut f,
        vec![
            (0, to("Second", fit(p[1]))),
            (0, to("Third, by reference", indirect.into())),
            (0, to("No such page", fit((999, 0)))),
            (0, dictionary! { "Title" => text("Nowhere") }),
        ],
    );
    let got = outline(&f.source());
    assert_eq!(pages(&got), [Some(1), Some(2), None, None]);
    let fit = Some(DestView::Other {
        kind: "Fit".into(),
        params: vec![],
    });
    assert_eq!(got[0].view, fit);
    assert_eq!(got[2].view, None, "no page, no position");
}

/// Names and strings, looked up in the `/Names /Dests` tree first and
/// then the catalog's `/Dests`; a value may be a dictionary's `/D`.
#[test]
fn named_destinations_are_looked_up() {
    let mut f = Form::new(4);
    let p = f.pages.clone();
    let kid = f.pdf.add_object(dictionary! {
        "Names" => vec![
            text("Boxed"), dictionary! { "D" => fit(p[2]) }.into(),
            text("New"), fit(p[1]),
        ],
    });
    let catalog = f.catalog;
    f.dict(catalog).set(
        "Names",
        dictionary! { "Dests" => dictionary! { "Kids" => vec![kid.into()] } },
    );
    f.dict(catalog).set(
        "Dests",
        dictionary! { "Old" => fit(p[3]), "New" => fit(p[0]) },
    );
    let name = |n: &str| Object::Name(n.into());
    outline_tree(
        &mut f,
        vec![
            (0, to("A name in /Dests", name("Old"))),
            (0, to("A string in the tree", text("New"))),
            (0, to("A name in both", name("New"))),
            (0, to("A dictionary's /D", text("Boxed"))),
            (0, to("Neither", name("Missing"))),
        ],
    );
    let got = outline(&f.source());
    assert_eq!(pages(&got), [Some(3), Some(1), Some(1), Some(2), None]);
}

#[test]
fn a_goto_action_is_followed() {
    let mut f = Form::new(3);
    let p = f.pages.clone();
    let action = f
        .pdf
        .add_object(dictionary! { "S" => "GoTo", "D" => fit(p[2]) });
    outline_tree(
        &mut f,
        vec![
            (
                0,
                dictionary! {
                    "Title" => text("Go to"),
                    "A" => dictionary! { "S" => "GoTo", "D" => fit(p[1]) },
                },
            ),
            (
                0,
                dictionary! { "Title" => text("By reference"), "A" => action },
            ),
            (
                0,
                dictionary! {
                    "Title" => text("A link"),
                    "A" => dictionary! { "S" => "URI", "URI" => text("https://example.com") },
                },
            ),
            (
                0,
                dictionary! {
                    "Title" => text("A dest that names nothing, then an action"),
                    "Dest" => Object::Name(b"Missing".to_vec()),
                    "A" => dictionary! { "S" => "GoTo", "D" => fit(p[0]) },
                },
            ),
        ],
    );
    let got = outline(&f.source());
    assert_eq!(pages(&got), [Some(1), Some(2), None, Some(0)]);
}

#[test]
fn an_integer_is_a_page_index() {
    let mut f = Form::new(3);
    outline_tree(
        &mut f,
        vec![
            (0, to("Second", dest(1, vec!["Fit".into()]))),
            (0, to("Past the end", dest(3, vec!["Fit".into()]))),
            (0, to("Negative", dest(-1, vec!["Fit".into()]))),
            (0, to("Placed", xyz(0, 72.into(), 720.into(), Object::Null))),
        ],
    );
    let got = outline(&f.source());
    assert_eq!(pages(&got), [Some(1), None, None, Some(0)]);
    assert!(near(&got[3].view, &at(Some(72.0), Some(72.0), None)));
}

/// A page the tree lists twice is where it's first listed.
#[test]
fn a_page_listed_twice_is_its_first_index() {
    let mut f = Form::new(2);
    let p = f.pages.clone();
    let tree = f
        .pdf
        .catalog()
        .unwrap()
        .get(b"Pages")
        .unwrap()
        .as_reference()
        .unwrap();
    let kids: Vec<Object> = vec![p[0].into(), p[1].into(), p[0].into()];
    f.dict(tree).set("Kids", kids);
    f.dict(tree).set("Count", 3);
    outline_tree(
        &mut f,
        vec![(0, to("First", fit(p[0]))), (0, to("Second", fit(p[1])))],
    );
    let src = f.source();
    assert_eq!(src.page_count(), 3);
    assert_eq!(pages(&outline(&src)), [Some(0), Some(1)]);
}

/// An `/XYZ` position is in the page's displayed space, after its
/// `/Rotate` and within its `/CropBox`, as `write_outline` writes it.
#[test]
fn positions_are_in_displayed_space() {
    let mut f = Form::new(3);
    let p = f.pages.clone();
    f.dict(p[1]).set("Rotate", 90);
    let crop: Vec<Object> = vec![50.into(), 50.into(), 562.into(), 742.into()];
    f.dict(p[2]).set("CropBox", crop);
    outline_tree(
        &mut f,
        vec![
            // User (100, 700) on a /Rotate 90 page: 700 across, 100 down.
            (0, to("Turned", xyz(p[1], 100.into(), 700.into(), 0.into()))),
            (
                0,
                to("Cropped", xyz(p[2], 150.into(), 642.into(), 2.into())),
            ),
            (
                0,
                to(
                    "Top only",
                    xyz(p[0], Object::Null, 500.into(), Object::Null),
                ),
            ),
            (
                0,
                to(
                    "Zoom only",
                    xyz(p[0], Object::Null, Object::Null, 1.5.into()),
                ),
            ),
            (
                0,
                to(
                    "Nothing set",
                    xyz(p[0], Object::Null, Object::Null, Object::Null),
                ),
            ),
            (
                0,
                to("Too short", dest(p[0], vec!["XYZ".into(), 10.into()])),
            ),
        ],
    );
    let got = outline(&f.source());
    let want = [
        at(Some(700.0), Some(100.0), None),
        at(Some(100.0), Some(100.0), Some(2.0)),
        at(None, Some(292.0), None),
        at(None, None, Some(1.5)),
        None,
        at(Some(10.0), None, None),
    ];
    for (got, want) in got.iter().zip(&want) {
        assert!(near(&got.view, want), "{}: {:?}", got.title, got.view);
    }
    // Saved and read again, each is where it was.
    let back = outline(&crate::forms::fixture::source(rewritten(&mut f, &got)));
    for (back, want) in back.iter().zip(&want) {
        assert!(near(&back.view, want), "{}: {:?}", back.title, back.view);
    }
}

#[test]
fn positions_sheaf_writes_come_back() {
    let mut f = Form::new(3);
    let p = f.pages.clone();
    f.dict(p[1]).set("Rotate", 270);
    let crop: Vec<Object> = vec![36.into(), 18.into(), 576.into(), 774.into()];
    f.dict(p[2]).set("CropBox", crop);
    f.dict(p[2]).set("Rotate", 180);
    let put = |title: &str, page, view| FlatOutlineItem {
        title: title.into(),
        page: Some(page),
        depth: 0,
        view,
        color: None,
        style: TextStyle::default(),
        open: true,
    };
    let items = [
        put("Upright", 0, at(Some(36.0), Some(100.0), None)),
        put("Turned", 1, at(Some(400.0), Some(50.0), Some(1.25))),
        put("Cropped and turned", 2, at(Some(12.5), Some(600.0), None)),
        put("Left only", 2, at(Some(80.0), None, None)),
    ];
    let back = outline(&crate::forms::fixture::source(rewritten(&mut f, &items)));
    assert_eq!(back.len(), items.len());
    for (back, put) in back.iter().zip(&items) {
        assert_eq!(back.page, put.page);
        assert!(
            near(&back.view, &put.view),
            "{}: {:?}",
            put.title,
            back.view
        );
    }
}

/// Every other kind keeps its numbers, as many as it takes, null as
/// None.
#[test]
fn every_fit_kind_keeps_its_numbers() {
    let mut f = Form::new(1);
    let p = f.pages[0];
    let kinds: Vec<(&str, Vec<Object>)> = vec![
        ("Fit", vec![1.into()]),
        ("FitB", vec![]),
        ("FitH", vec![500.into()]),
        ("FitV", vec![Object::Null]),
        ("FitBH", vec![]),
        ("FitBV", vec![7.5.into(), 8.into()]),
        ("FitR", vec![1.into(), 2.into(), 3.into(), 4.into()]),
        ("FitR", vec![1.into(), 2.into()]),
        ("Zoom", vec![3.into()]),
    ];
    outline_tree(
        &mut f,
        kinds
            .iter()
            .map(|(kind, params)| {
                let mut rest = vec![Object::Name(kind.as_bytes().to_vec())];
                rest.extend(params.iter().cloned());
                (0, to(kind, dest(p, rest)))
            })
            .collect(),
    );
    let got = outline(&f.source());
    let other = |kind: &str, params: Vec<Option<f32>>| {
        Some(DestView::Other {
            kind: kind.into(),
            params,
        })
    };
    let want = [
        other("Fit", vec![]),
        other("FitB", vec![]),
        other("FitH", vec![Some(500.0)]),
        other("FitV", vec![None]),
        other("FitBH", vec![None]),
        other("FitBV", vec![Some(7.5)]),
        other("FitR", vec![Some(1.0), Some(2.0), Some(3.0), Some(4.0)]),
        other("FitR", vec![Some(1.0), Some(2.0), None, None]),
        None,
    ];
    let views: Vec<Option<DestView>> = got.iter().map(|i| i.view.clone()).collect();
    assert_eq!(views, want);
    assert_eq!(got[8].page, Some(0), "an unknown kind still has its page");
}

/// `/C` (black is no color) and `/F`; the title as text, control
/// characters as spaces.
#[test]
fn colors_styles_and_titles() {
    let mut f = Form::new(1);
    let styled = |title: &str, c: Option<Vec<Object>>, flags: Option<i64>| {
        let mut d = dictionary! { "Title" => text(title) };
        if let Some(c) = c {
            d.set("C", c);
        }
        if let Some(flags) = flags {
            d.set("F", flags);
        }
        (0, d)
    };
    let rgb = |r: f32, g: f32, b: f32| Some(vec![r.into(), g.into(), b.into()]);
    let utf16: Vec<u8> = [0xFE, 0xFF, 0x00, b'P', 0x00, 0xE9, 0x00, b'n']
        .into_iter()
        .collect();
    let two_lines = Object::String(b"Two\r\nlines".to_vec(), StringFormat::Literal);
    outline_tree(
        &mut f,
        vec![
            styled("Red", rgb(1.0, 0.0, 0.0), None),
            styled("Black", rgb(0.0, 0.0, 0.0), Some(1)),
            styled("Clamped", rgb(2.0, -1.0, 0.5), Some(2)),
            styled("Short", Some(vec![1.into(), 0.into()]), Some(3)),
            (
                0,
                dictionary! { "Title" => Object::String(utf16, StringFormat::Hexadecimal) },
            ),
            (0, dictionary! { "Title" => two_lines }),
            (0, dictionary! {}),
        ],
    );
    let got = outline(&f.source());
    let colors: Vec<Option<[u8; 3]>> = got.iter().map(|i| i.color).collect();
    assert_eq!(
        colors,
        [
            Some([255, 0, 0]),
            None,
            Some([255, 0, 128]),
            None,
            None,
            None,
            None
        ]
    );
    let styles: Vec<(bool, bool)> = got.iter().map(|i| (i.style.italic, i.style.bold)).collect();
    assert_eq!(
        &styles[..4],
        [(false, false), (true, false), (false, true), (true, true)]
    );
    assert_eq!(titles(&got)[4..], ["Pén", "Two  lines", ""]);
}

/// Titles as PDFium shows them, each checked against it: PDFDocEncoding's
/// quotes, dashes, accents and the rest of its table, its undefined bytes
/// and control characters as spaces, UTF-16 either way round with a
/// dangling last byte left out, and UTF-8 after its mark.
#[test]
fn titles_decode_as_pdfium_shows_them() {
    let cases: [(&[u8], &str); 10] = [
        (b"What\x90s", "What’s"),
        (b"\x84 and \x85", "— and –"),
        (b"\x18\x19\x1a\x1b\x1c\x1d\x1e\x1f", "˘ˇˆ˙˝˛˚˜"),
        (b"a\x7fb\xadc\x9fd\te\nf", "a b c d e f"),
        (&[0xFE, 0xFF, 0, b'A', 0, 0, 0, b'B'], "A B"),
        (&[0xFF, 0xFE, b'H', 0, b'i', 0], "Hi"),
        (&[0xFE, 0xFF, 0, b'A', 0x00], "A"),
        (&[0xFF, 0xFE, b'A', 0, 0x42], "A"),
        (&[0xEF, 0xBB, 0xBF, b'O', b'K', 0xC3, 0xA9], "OKé"),
        (&[0xFE, 0xFF], ""),
    ];
    for (bytes, want) in cases {
        assert_eq!(title(bytes), want, "{bytes:02x?}");
    }
    let high: Vec<u8> = (0x80..=0xA0).collect();
    assert_eq!(title(&high), "•†‡…—–ƒ⁄‹›−‰„“”‘’‚™ﬁﬂŁŒŠŸŽıłœšž €");
}

#[test]
fn a_negative_count_is_closed() {
    let mut f = Form::new(1);
    let item = |title: &str| dictionary! { "Title" => text(title) };
    let mut closed = item("Closed");
    closed.set("Count", -2);
    outline_tree(
        &mut f,
        vec![
            (0, closed),
            (1, item("Under closed")),
            (1, item("Under closed, too")),
            (0, item("Open")),
            (1, item("Under open")),
            (0, item("Leaf")),
        ],
    );
    let got = outline(&f.source());
    let open: Vec<bool> = got.iter().map(|i| i.open).collect();
    assert_eq!(open, [false, true, true, true, true, true]);
    assert_eq!(
        got.iter().map(|i| i.depth).collect::<Vec<_>>(),
        [0, 1, 1, 0, 1, 0]
    );
}

#[test]
fn a_cycle_reads_each_item_once() {
    let item = |depth: u16, title: &str| (depth, dictionary! { "Title" => text(title) });
    // C's /Next is A.
    let mut f = Form::new(1);
    let ids = outline_tree(&mut f, vec![item(0, "A"), item(0, "B"), item(0, "C")]);
    f.dict(ids[2]).set("Next", ids[0]);
    assert_eq!(titles(&outline(&f.source())), ["A", "B", "C"]);
    // B, A's child, has A as its child, and C as its sibling.
    let mut f = Form::new(1);
    let ids = outline_tree(&mut f, vec![item(0, "A"), item(1, "B"), item(1, "C")]);
    f.dict(ids[1]).set("First", ids[0]);
    f.dict(ids[1]).set("Last", ids[0]);
    let got = outline(&f.source());
    assert_eq!(titles(&got), ["A", "B", "C"]);
    assert_eq!(got.iter().map(|i| i.depth).collect::<Vec<_>>(), [0, 1, 1]);
}

#[test]
fn depth_past_32_is_cut() {
    let mut f = Form::new(1);
    outline_tree(
        &mut f,
        (0..40)
            .map(|d| (d, dictionary! { "Title" => text(&format!("Level {d}")) }))
            .collect(),
    );
    let got = outline(&f.source());
    assert_eq!(got.len(), 33);
    assert_eq!(got.last().unwrap().depth, 32);
}
