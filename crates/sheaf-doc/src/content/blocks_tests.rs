//! Marked content, balance, splices, names and blocks: ranges found across
//! streams and their lists, splices that keep every other byte and refuse
//! what would change the rest of the page, and pages classified by who
//! drew their pagination artifacts.

use std::borrow::Cow;
use std::sync::Arc;

use lopdf::{Dictionary, Document as Pdf, Object, ObjectId, Stream, dictionary};

use super::*;
use crate::annot::Box2;
use crate::geom::Geom;
use crate::source::Limits;
use crate::text_layer::{OcrWord, TextLayer};

const LIM: &Limits = &Limits::DEFAULT;

fn found(streams: &[&[u8]], props: &PropertiesDict) -> Result<Vec<Marked>, LexError> {
    marked(&Concat::new(streams), props, &ResCx::none(), LIM)
}

fn span(start: (u32, u32), end: (u32, u32)) -> SpanAcross {
    let pos = |(stream, at)| Pos { stream, at };
    SpanAcross {
        start: pos(start),
        end: pos(end),
    }
}

/// The span of `part` in `s`, one stream.
fn span_of(s: &[u8], part: &[u8]) -> SpanAcross {
    let at = s.windows(part.len()).position(|w| w == part).unwrap() as u32;
    span((0, at), (0, at + part.len() as u32))
}

#[test]
fn marked_ranges_have_their_lists() {
    let mut pdf = Pdf::with_version("1.7");
    let ocg = pdf.add_object(dictionary! { "Type" => "OCG" });
    let resources: Object = dictionary! {
        "Properties" => dictionary! { "P0" => dictionary! { "Subtype" => "Header" }, "oc1" => ocg },
    }
    .into();
    let props = PropertiesDict::read(&pdf, Some(&resources));
    let first: &[u8] =
        b"/Artifact <</Type /Pagination /S#75btype /Footer /A [1 (b\\)) <0a>]>> BDC q";
    let second: &[u8] =
        b"/OC /oc1 BDC /Span BMC EMC EMC Q EMC /Artifact /P0 BDC EMC /X /None BDC EMC";
    let m = found(&[first, second], &props).unwrap();
    let tags: Vec<&[u8]> = m.iter().map(|m| m.tag.as_slice()).collect();
    assert_eq!(tags, [&b"Artifact"[..], b"OC", b"Span", b"Artifact", b"X"]);
    assert_eq!(
        m.iter().map(|m| m.depth).collect::<Vec<_>>(),
        [0, 1, 2, 0, 0]
    );
    // The first runs from the first stream into the second.
    assert_eq!(m[0].span, span((0, 0), (1, 36)));
    let Some(Props::Inline(list)) = &m[0].props else {
        panic!("{:?}", m[0].props)
    };
    assert_eq!(
        list.get(b"Subtype").unwrap(),
        &Object::Name(b"Footer".to_vec())
    );
    assert_eq!(
        list.get(b"A").unwrap(),
        &Object::Array(vec![
            1.into(),
            Object::string_literal("b)"),
            Object::String(vec![10], lopdf::StringFormat::Hexadecimal),
        ])
    );
    // Named lists, found or not.
    assert_eq!(
        m[1].props,
        Some(Props::Named(
            b"oc1".to_vec(),
            Some(dictionary! { "Type" => "OCG" })
        ))
    );
    assert_eq!(m[2].props, None);
    assert_eq!(
        m[3].props.as_ref().and_then(Props::dict),
        Some(&dictionary! { "Subtype" => "Header" })
    );
    assert_eq!(m[4].props, Some(Props::Named(b"None".to_vec(), None)));
}

#[test]
fn marked_content_must_balance() {
    let props = PropertiesDict::default();
    let kind = |s: &[u8]| found(&[s], &props).unwrap_err();
    assert_eq!(
        kind(b"q /A BMC Q EMC EMC"),
        LexError {
            at: 15,
            kind: LexErrorKind::Emc
        }
    );
    assert_eq!(
        kind(b"/A BMC /B BMC EMC"),
        LexError {
            at: 0,
            kind: LexErrorKind::Marked
        }
    );
    let deep = Limits {
        content: crate::source::ContentLimits {
            depth: 2,
            ..LIM.content
        },
        ..*LIM
    };
    let e = marked(
        &Concat::new(&[b"/A BMC /B BMC /C BMC EMC EMC EMC"]),
        &props,
        &ResCx::none(),
        &deep,
    )
    .unwrap_err();
    assert_eq!(e.kind, LexErrorKind::Depth);
    // Marked content inside a string or an inline image isn't.
    assert!(found(&[b"(EMC) Tj BI /W 3 /H 1 /CS /G /BPC 8 ID EMC EI"], &props).is_ok());
}

#[test]
fn balance_is_the_q_count() {
    let of = |s: &[u8]| balance(&Concat::new(&[s]), &ResCx::none(), LIM).unwrap();
    // §3.4's worked bases.
    let b = |net, min_prefix| Balance { net, min_prefix };
    assert_eq!(of(b"q 1 0 0 1 0 0 cm"), b(1, 0));
    assert_eq!(of(b"Q q"), b(0, -1));
    assert_eq!(of(b"Q"), b(-1, -1));
    assert_eq!(of(b"q q Q Q Q q"), b(0, -1));
    // Not in strings, names or an image's data.
    assert_eq!(
        of(b"(Q) Tj /Q gs BI /W 1 /H 1 /CS /G /BPC 8 ID Q EI"),
        b(0, 0)
    );
    // Across streams.
    let c = Concat::new(&[b"q", b"Q Q"]);
    assert_eq!(balance(&c, &ResCx::none(), LIM).unwrap(), b(-1, -1));
}

fn spliced<'a>(c: &'a Concat, drop: &[SpanAcross]) -> Result<Vec<Cow<'a, [u8]>>, SpliceError> {
    splice(c, &ResCx::none(), LIM, drop)
}

/// `s` with `part` taken out.
fn without(s: &[u8], part: &[u8]) -> Result<Vec<u8>, SpliceError> {
    let c = Concat::new(&[s]);
    let mut out = spliced(&c, &[span_of(s, part)])?;
    Ok(out.remove(0).into_owned())
}

fn is_unsafe(r: Result<Vec<u8>, SpliceError>) -> bool {
    matches!(r, Err(SpliceError::Unsafe(_)))
}

#[test]
fn a_splice_of_nothing_is_the_streams_as_they_are() {
    let streams: [&[u8]; 3] = [b"q /P <</MCID 0>> BDC (ab", b"c) Tj", b"EMC Q % end"];
    let c = Concat::new(&streams);
    let out = spliced(&c, &[]).unwrap();
    assert!(out.iter().all(|s| matches!(s, Cow::Borrowed(_))));
    assert_eq!(out, streams.map(Cow::Borrowed));
}

#[test]
fn a_range_across_streams_is_taken_out_of_both() {
    let streams: [&[u8]; 3] = [b"q /A BMC q 0 g", b"0 0 9 9 re f Q EMC 1 g", b"Q"];
    let c = Concat::new(&streams);
    let m = marked(&c, &PropertiesDict::default(), &ResCx::none(), LIM).unwrap();
    let out = spliced(&c, &[m[0].span]).unwrap();
    assert_eq!(out[0], Cow::<[u8]>::Owned(b"q ".to_vec()));
    assert_eq!(out[1], Cow::<[u8]>::Owned(b" 1 g".to_vec()));
    assert!(matches!(out[2], Cow::Borrowed(b"Q")));
}

#[test]
fn a_splice_inside_a_text_object_keeps_the_text_where_it_was() {
    let body =
        |artifact: &str| format!("BT /F1 12 Tf 72 700 Td {artifact} (Body) Tj ET").into_bytes();
    // Color in its own `q`…`Q` can go.
    let s = body("/Artifact BMC q 1 0 0 rg Q EMC");
    assert_eq!(
        without(&s, b"/Artifact BMC q 1 0 0 rg Q EMC").unwrap(),
        b"BT /F1 12 Tf 72 700 Td  (Body) Tj ET"
    );
    // A color, a font, a move, or a line moved by `'` would change the
    // body text; a move would even inside a `q`, which doesn't save it.
    for artifact in [
        "/Artifact BMC 1 0 0 rg EMC",
        "/Artifact BMC /F2 9 Tf EMC",
        "/Artifact BMC 0 50 Td EMC",
        "/Artifact BMC q 0 50 Td Q EMC",
        "/Artifact BMC (x) ' EMC",
        "/Artifact BMC 2 Tr EMC",
    ] {
        assert!(
            is_unsafe(without(&body(artifact), artifact.as_bytes())),
            "{artifact}"
        );
    }
    // Text it shows moves the body text on, unless a line is moved first:
    // setting text state doesn't move one.
    let shows = "/Artifact BMC (Head) Tj EMC";
    assert!(is_unsafe(without(&body(shows), shows.as_bytes())));
    let moved = format!("BT /F1 12 Tf 72 700 Td {shows} 0 -14 Td (Body) Tj ET").into_bytes();
    assert!(without(&moved, shows.as_bytes()).is_ok());
    for state in [
        "/F1 12 Tf",
        "0 Tc",
        "0 Tw",
        "100 Tz",
        "14 TL",
        "0 Tr",
        "0 Ts",
    ] {
        let s = format!("BT 100 700 Td {shows} {state} (Body) Tj ET").into_bytes();
        assert!(is_unsafe(without(&s, shows.as_bytes())), "{state}");
    }
    let last = format!("BT /F1 12 Tf 72 700 Td (Body) Tj {shows} ET").into_bytes();
    assert!(without(&last, shows.as_bytes()).is_ok());
    // Out of the text object it's in: an end without a start.
    let s = b"BT /A BMC (x) Tj ET EMC";
    assert!(is_unsafe(without(s, b"/A BMC (x) Tj ET EMC")));
}

#[test]
fn a_range_that_doesnt_balance_is_refused() {
    let s = b"q /A BMC Q q EMC Q";
    assert!(is_unsafe(without(s, b"/A BMC Q q EMC")));
    let s = b"/A BMC BT /F1 9 Tf EMC (x) Tj ET";
    assert!(is_unsafe(without(s, b"/A BMC BT /F1 9 Tf EMC")));
    // Its own marked content, and q and Q, balanced: it goes.
    let s = b"0 g /A BMC q /B BMC 1 g EMC Q EMC 0 0 m";
    assert_eq!(
        without(s, b"/A BMC q /B BMC 1 g EMC Q EMC").unwrap(),
        b"0 g  0 0 m"
    );
    // Half a range.
    let s = b"/A BMC /B BMC EMC EMC";
    assert!(is_unsafe(without(s, b"/A BMC /B BMC EMC")));
}

/// Graphics state a range sets outside a `q`…`Q` of its own would last
/// past it: the CTM, a color, a line width, a clip, a `gs`, a font.
#[test]
fn a_range_that_sets_state_is_refused() {
    for state in [
        "2 0 0 2 0 0 cm",
        "1 0 0 rg",
        "/CS0 cs 0 scn",
        "3 w",
        "/GS0 gs",
        "0 0 9 9 re W n",
        "/F1 9 Tf",
    ] {
        let range = format!("/Artifact BMC {state} EMC");
        let s = format!("0 g {range} 0 0 9 9 re f");
        assert!(
            is_unsafe(without(s.as_bytes(), range.as_bytes())),
            "{state}"
        );
        let inside = format!("/Artifact BMC q {state} Q EMC");
        let s = format!("0 g {inside} 0 0 9 9 re f");
        assert_eq!(
            without(s.as_bytes(), inside.as_bytes()).unwrap(),
            b"0 g  0 0 9 9 re f",
            "{state}"
        );
    }
}

/// A path is painted by the operator that ends it: a range can't take
/// part of one.
#[test]
fn a_range_inside_a_path_is_refused() {
    let s = b"0 0 m /A BMC EMC 9 9 l S";
    assert!(is_unsafe(without(s, b"/A BMC EMC")));
    let s = b"/A BMC 0 0 m 9 9 l EMC S";
    assert!(is_unsafe(without(s, b"/A BMC 0 0 m 9 9 l EMC")));
    let s = b"/A BMC 0 0 m 9 9 l S EMC 0 0 9 9 re f";
    assert_eq!(
        without(s, b"/A BMC 0 0 m 9 9 l S EMC").unwrap(),
        b" 0 0 9 9 re f"
    );
}

/// Marked content draws nothing, but for optional content: one range may
/// take out the start of an artifact and another its end, if no marked
/// content is kept between them. Acrobat's first artifact on a page opens with the `Q` that ends
/// the page's `q`; keeping it leaves the page as it was.
#[test]
fn marked_content_may_open_in_one_range_and_close_in_another() {
    let s: &[u8] = b"q 0 g /Artifact <</Subtype /Header>> BDC Q q 1 0 0 1 9 9 cm /X0 Do Q EMC 1 g";
    let c = Concat::new(&[s]);
    let header = marked(&c, &PropertiesDict::default(), &ResCx::none(), LIM).unwrap()[0].span;
    // Alone, it would end the page's `q`.
    assert!(matches!(
        spliced(&c, &[header]),
        Err(SpliceError::Unsafe(_))
    ));
    let pagination = Pagination {
        kind: PaginationKind::Header,
        origin: PagOrigin::Acrobat,
        span: header,
    };
    let cuts = pagination.cuts(&c, &ResCx::none(), LIM).unwrap();
    assert_eq!(
        cuts,
        [
            span_of(s, b"/Artifact <</Subtype /Header>> BDC "),
            span_of(s, b" q 1 0 0 1 9 9 cm /X0 Do Q EMC")
        ]
    );
    assert_eq!(spliced(&c, &cuts).unwrap()[0], &b"q 0 g Q 1 g"[..]);
    // A range with no `Q` to keep is one cut.
    let plain = Pagination {
        span: span_of(s, b"/X0 Do"),
        ..pagination
    };
    assert_eq!(plain.cuts(&c, &ResCx::none(), LIM).unwrap(), [plain.span]);
    // Marked content kept between them, or an end with no start taken.
    let s = b"/A BMC /B BMC EMC EMC";
    let c = Concat::new(&[s]);
    let ends = [span_of(s, b"/A BMC"), span((0, 18), (0, 21))];
    assert!(matches!(spliced(&c, &ends), Err(SpliceError::Unsafe(_))));
    let s = b"/A BMC EMC EMC";
    assert!(is_unsafe(without(s, b"EMC EMC")));
    let s = b"/A BMC 0 g EMC";
    let c = Concat::new(&[s]);
    let ends = [span_of(s, b"/A BMC"), span_of(s, b"EMC")];
    assert_eq!(spliced(&c, &ends).unwrap()[0], &b" 0 g "[..]);
    // Optional content hides what it marks: its ends stay together.
    let s = b"/OC /oc1 BDC 0 g EMC";
    let c = Concat::new(&[s]);
    let ends = [span_of(s, b"/OC /oc1 BDC"), span_of(s, b"EMC")];
    assert!(matches!(spliced(&c, &ends), Err(SpliceError::Unsafe(_))));
}

#[test]
fn ranges_must_lie_between_tokens() {
    let s: &[u8] = b"(a b) Tj % note\n0 g";
    // Inside a string.
    assert!(is_unsafe(without(s, b"b) Tj")));
    // Ending in a comment: what's left of it would draw.
    assert!(is_unsafe(without(s, b"Tj % no")));
    // Starting in one: what follows would be commented out.
    assert!(is_unsafe(without(s, b"ote\n0 g")));
    assert_eq!(without(s, b"% note\n").unwrap(), b"(a b) Tj 0 g");
    // Overlapping, outside the content, or ending before it starts.
    let c = Concat::new(&[s]);
    let overlap = spliced(&c, &[span((0, 0), (0, 8)), span((0, 6), (0, 10))]);
    assert!(matches!(overlap, Err(SpliceError::Overlap(_))));
    for outside in [
        span((0, 0), (0, 99)),
        span((1, 0), (1, 0)),
        span((0, 5), (0, 2)),
    ] {
        assert!(matches!(
            spliced(&c, &[outside]),
            Err(SpliceError::OutOfRange(_))
        ));
    }
}

#[test]
fn tokens_either_side_of_a_range_stay_apart() {
    assert_eq!(
        without(b"Q/A BMC EMC/F1 9 Tf", b"/A BMC EMC").unwrap(),
        b"Q /F1 9 Tf"
    );
    assert_eq!(
        without(b"(x)Tj/A BMC EMC(y)Tj", b"/A BMC EMC").unwrap(),
        b"(x)Tj (y)Tj"
    );
    assert_eq!(
        without(b"Q\n/A BMC EMC/F1 9 Tf", b"/A BMC EMC").unwrap(),
        b"Q\n/F1 9 Tf"
    );
    // Two ranges that touch: the bytes either side of both decide.
    let s = b"0 g/A Do  /B Do 0 0 1 1 re f";
    let c = Concat::new(&[s]);
    let out = spliced(&c, &[span((0, 3), (0, 9)), span((0, 9), (0, 16))]).unwrap();
    assert_eq!(out[0], &b"0 g 0 0 1 1 re f"[..]);
}

/// The operands before a range, or its last ones, would be the next
/// operator's.
#[test]
fn ranges_must_lie_between_steps() {
    let s: &[u8] = b"1 2 3 /A BMC EMC 4 5 6 re f";
    assert!(is_unsafe(without(s, b"/A BMC EMC")));
    assert!(is_unsafe(without(s, b"/A BMC EMC 4 5")));
    assert_eq!(without(s, b"1 2 3 /A BMC EMC").unwrap(), b" 4 5 6 re f");
}

#[test]
fn names_used_are_by_category() {
    let s: &[u8] = b"/GS0 gs /F1 9 Tf /CS0 cs /DeviceRGB CS /P0 scn 0.5 /P1 SCN /Im1 Do \
        /Sh0 sh /OC /oc0 BDC EMC /X /Named DP /Y <<>> BDC EMC \
        BI /W 1 /H 1 /CS /Idx /BPC 8 ID x EI BI /W 1 /H 1 /CS /RGB /BPC 8 ID xyz EI \
        /Artifact BMC /F9 1 Tf EMC";
    let c = Concat::new(&[s]);
    let except = [span_of(s, b"/Artifact BMC /F9 1 Tf EMC")];
    let names = names_used(&c, &ResCx::none(), LIM, &except).unwrap();
    let of = |kind| names.of(kind).collect::<Vec<_>>();
    assert_eq!(of(ResKind::ExtGState), [b"GS0"]);
    assert_eq!(of(ResKind::Font), [b"F1"]);
    assert_eq!(of(ResKind::ColorSpace), [&b"CS0"[..], b"Idx"]);
    assert_eq!(of(ResKind::Pattern), [b"P0", b"P1"]);
    assert_eq!(of(ResKind::XObject), [b"Im1"]);
    assert_eq!(of(ResKind::Shading), [b"Sh0"]);
    assert_eq!(of(ResKind::Properties), [&b"Named"[..], b"oc0"]);
    assert_eq!(names.len(), 10);
    assert!(!names.contains(ResKind::Font, b"F9"));
}

/// A page whose `/Contents` is `streams`, each with its dictionary, drawn
/// with `resources`: its blocks.
fn blocks_of(
    pdf: &mut Pdf,
    streams: &[(Dictionary, &[u8])],
    resources: Dictionary,
) -> (PageBlocks, Vec<ObjectId>) {
    let ids: Vec<ObjectId> = streams
        .iter()
        .map(|(dict, data)| pdf.add_object(Stream::new(dict.clone(), data.to_vec())))
        .collect();
    let resources = Object::Dictionary(resources);
    let blocks =
        PageBlocks::read(&*pdf, &ids, Some(&resources), LIM, &DrawnForms::default()).unwrap();
    (blocks, ids)
}

fn origins(blocks: &PageBlocks) -> Vec<(PaginationKind, PagOrigin)> {
    blocks
        .pagination
        .iter()
        .map(|p| (p.kind, p.origin))
        .collect()
}

#[test]
fn sheafs_blocks_are_found_by_their_markers() {
    let mut pdf = Pdf::with_version("1.7");
    let gen_list = dictionary! {
        "Type" => "Pagination", "Subtype" => "Header", "SheafGen" => Object::string_literal("g1f"),
    };
    let resources = dictionary! { "Properties" => dictionary! {
        "ShP0" => gen_list,
        "ShP1" => dictionary! { "SheafOcrBlock" => Object::string_literal("b2a") },
    } };
    let block = |k: &str, key: &str, id: &str| {
        dictionary! { "SheafBlock" => dictionary! { "K" => k, key => Object::string_literal(id) } }
    };
    let gen_stream: &[u8] = b"/Artifact /ShP0 BDC (Head) Tj EMC";
    let (blocks, ids) = blocks_of(
        &mut pdf,
        &[
            (
                dictionary! { "SheafBlock" => dictionary! { "K" => "Isolate" } },
                b"q q",
            ),
            (
                dictionary! {},
                b"0 g /Artifact /ShP0 BDC (merged) Tj EMC /ShOCR /ShP1 BDC EMC",
            ),
            (
                dictionary! { "SheafBlock" => dictionary! { "K" => "Isolate" } },
                b"Q Q",
            ),
            (block("Gen", "G", "g1f"), gen_stream),
            (block("Ocr", "B", "b2b"), b""),
            (block("Flat", "B", "b2c"), b"q /Fm0 Do Q"),
        ],
        resources,
    );
    use PaginationKind::Header;
    assert_eq!(
        origins(&blocks),
        [(Header, PagOrigin::Sheaf), (Header, PagOrigin::Sheaf)]
    );
    let sheaf: Vec<_> = blocks.sheaf.iter().map(|b| (b.block, b.at)).collect();
    assert_eq!(
        sheaf,
        [
            (SheafBlock::Gen(GenId(0x1f)), BlockAt::Stream(ids[3])),
            (SheafBlock::Ocr(BlockId(0x2b)), BlockAt::Stream(ids[4])),
            (SheafBlock::Flat(BlockId(0x2c)), BlockAt::Stream(ids[5])),
            (
                SheafBlock::Gen(GenId(0x1f)),
                BlockAt::Marked(span((1, 4), (1, 39)))
            ),
            (
                SheafBlock::Ocr(BlockId(0x2a)),
                BlockAt::Marked(span((1, 40), (1, 60)))
            ),
        ]
    );
    assert_eq!(&*blocks.isolation, &[ids[0], ids[2]]);
    assert_eq!(
        blocks.balance,
        Balance {
            net: 0,
            min_prefix: 0
        }
    );
    // Ids written otherwise are no ids.
    for bad in [&b"g"[..], b"b1", b"gx1", b"g12345678901234567"] {
        assert_eq!(GenId::parse(bad), None);
    }
    assert_eq!(BlockId::parse(b"bFF"), Some(BlockId(255)));
}

#[test]
fn sheafs_own_from_before_are_legacy() {
    // A header as save drew it, and an OCR layer.
    let mut pdf = Pdf::with_version("1.7");
    let body = pdf.add_object(Stream::new(dictionary! {}, b"0 0 9 9 re f".to_vec()));
    let mut page = dictionary! { "Contents" => body };
    let placed = [crate::stamps::Placed {
        header: true,
        text: "Head".into(),
        x: 72.0,
        baseline: 750.0,
        size: 9.0,
        width: 20.0,
    }];
    let geom = Geom::read(None, None, 0);
    let mut res = crate::content::ResObjects::default();
    crate::stamps::install(&mut pdf, &mut page, &placed, [0; 3], &geom, &mut res);
    let words = vec![OcrWord {
        text: "word".into(),
        rect: Box2 {
            x: 72.0,
            y: 100.0,
            w: 40.0,
            h: 10.0,
        },
        confidence: 0.9,
    }];
    let layer = TextLayer::from_engine(words, &geom, "test");
    crate::text_layer::install(&mut pdf, &mut page, &layer, &mut res);
    let ids = contents(&page);
    let resources = page.get(b"Resources").ok();
    let blocks = PageBlocks::read(&pdf, &ids, resources, LIM, &DrawnForms::default()).unwrap();
    assert_eq!(
        origins(&blocks),
        [(PaginationKind::Header, PagOrigin::SheafLegacy)]
    );
    let content = PageContent::read(&pdf, &ids, LIM).unwrap();
    let streams = content.ids();
    // `[ocr q, header q, body, header, ocr layer]`
    assert_eq!(
        blocks.legacy_ocr,
        Some(LegacyOcr {
            q: streams[0],
            layer: streams[4]
        })
    );
    assert_eq!(blocks.balance, Balance::default());
    // `/SheafHF` alone marks it, too.
    let mut pdf = Pdf::with_version("1.7");
    let (blocks, _) = blocks_of(
        &mut pdf,
        &[(
            dictionary! {},
            b"/Artifact <</Subtype /Footer>> BDC BT /SheafHF 9 Tf (1) Tj ET EMC",
        )],
        dictionary! {},
    );
    assert_eq!(
        origins(&blocks),
        [(PaginationKind::Footer, PagOrigin::SheafLegacy)]
    );
}

/// A form of Acrobat's, its `/PieceInfo` naming `private`, and resources
/// that name it `/Xi0`, and a form of no one's `/Fm0` holding `inner`.
fn acrobat_resources(pdf: &mut Pdf, private: &str, inner: &[u8]) -> Dictionary {
    let piece = pdf.add_object(dictionary! {
        "ADBE_CompoundType" => dictionary! {
            "DocSettings" => dictionary! {}, "Private" => Object::Name(private.as_bytes().to_vec()),
        },
    });
    let form = |pdf: &mut Pdf, extra: Dictionary, content: &[u8]| {
        let mut dict = dictionary! {
            "Type" => "XObject", "Subtype" => "Form", "BBox" => vec![0.into(), 0.into(), 9.into(), 9.into()],
        };
        dict.extend(&extra);
        pdf.add_object(Stream::new(dict, content.to_vec()))
    };
    let xi0 = form(
        pdf,
        dictionary! { "PieceInfo" => piece },
        b"BT (Head) Tj ET",
    );
    let fm0 = form(pdf, dictionary! {}, inner);
    dictionary! { "XObject" => dictionary! { "Xi0" => xi0, "Fm0" => fm0 } }
}

#[test]
fn acrobats_draw_its_forms_only() {
    const HEADER: &str = "/Artifact <</Type /Pagination /Subtype /Header>> BDC";
    let page =
        |inside: &str| format!("{HEADER} q 0 0 612 792 re W n /Xi0 Do Q {inside} EMC").into_bytes();
    let mut pdf = Pdf::with_version("1.7");
    let resources = acrobat_resources(&mut pdf, "Header", b"");
    let classify = |pdf: &mut Pdf, content: &[u8], resources: &Dictionary| {
        let (blocks, _) = blocks_of(pdf, &[(dictionary! {}, content)], resources.clone());
        origins(&blocks)
    };
    use PagOrigin::{Acrobat, Authored};
    use PaginationKind::Header;
    assert_eq!(
        classify(&mut pdf, &page(""), &resources),
        [(Header, Acrobat)]
    );
    // Drawing anything else, or a form of no one's, is an author's.
    for inside in ["(x) Tj", "0 0 m 9 9 l S", "/Fm0 Do"] {
        assert_eq!(
            classify(&mut pdf, &page(inside), &resources),
            [(Header, Authored)],
            "{inside}"
        );
    }
    assert_eq!(
        classify(
            &mut pdf,
            format!("{HEADER} BT (Head) Tj ET EMC").as_bytes(),
            &resources
        ),
        [(Header, Authored)]
    );
    // A `/PieceInfo` that names something else isn't Acrobat's pagination.
    let other = acrobat_resources(&mut pdf, "Stamp", b"");
    assert_eq!(classify(&mut pdf, &page(""), &other), [(Header, Authored)]);
}

#[test]
fn artifacts_in_forms_are_reported_left_in_place() {
    let inner = b"/Artifact <</Type /Pagination /Subtype /Footer>> BDC (f) Tj EMC";
    let mut pdf = Pdf::with_version("1.7");
    let resources = acrobat_resources(&mut pdf, "Header", inner);
    let nested = |pdf: &mut Pdf, content: &[u8]| {
        blocks_of(pdf, &[(dictionary! {}, content)], resources.clone())
            .0
            .nested_in_forms
    };
    assert!(nested(&mut pdf, b"q /Fm0 Do Q"));
    // Drawn inside a pagination artifact, it goes with it.
    assert!(!nested(
        &mut pdf,
        b"/Artifact <</Subtype /Header>> BDC /Fm0 Do EMC"
    ));
    assert!(!nested(&mut pdf, b"/Xi0 Do /Absent Do"));
}

#[test]
fn a_pages_blocks_are_read_once_and_name_the_page() {
    let mut pdf = Pdf::with_version("1.7");
    let pages = pdf.new_object_id();
    let good = pdf.add_object(Stream::new(
        dictionary! {},
        b"/Artifact <</Subtype /Header>> BDC (h) Tj EMC".to_vec(),
    ));
    let unended = pdf.add_object(Stream::new(dictionary! {}, b"/A BMC".to_vec()));
    let kids: Vec<Object> = [good, unended]
        .map(|c| {
            pdf.add_object(dictionary! { "Type" => "Page", "Parent" => pages, "Contents" => c })
                .into()
        })
        .to_vec();
    pdf.objects.insert(
        pages,
        dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => 2,
        "MediaBox" => vec![0.into(), 0.into(), 9.into(), 9.into()] }
        .into(),
    );
    let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
    pdf.trailer.set("Root", catalog);
    let mut bytes = Vec::new();
    pdf.save_to(&mut bytes).unwrap();
    let src = crate::forms::fixture::source(bytes);
    // Never at open; once read, kept, and the streams aren't.
    assert!(!src.blocks_known(0));
    let blocks = src.blocks(0).unwrap();
    assert!(src.blocks_known(0) && !src.blocks_known(1));
    assert!(blocks.has(&[PaginationKind::Header]));
    assert!(!src.pdf().is_read(good));
    assert!(Arc::ptr_eq(&blocks, &src.blocks(0).unwrap()));
    let e = src.blocks(1).unwrap_err();
    assert_eq!(
        e.to_string(),
        "page 2's content can't be read; nothing was changed"
    );
    assert_eq!(e.on_page(6).page(), 6);
    assert_eq!(*src.blocks(9).unwrap(), PageBlocks::default());
}

/// A form is read once, whatever names and pages draw it: 2,000 names of
/// one 8 MiB form on 50 pages. Past the page's cap, or once what the
/// page's streams leave of it is spent, a form may hold an artifact, and
/// counts as holding one.
#[test]
fn forms_are_read_once_and_within_the_pages_cap() {
    let mut pdf = Pdf::with_version("1.7");
    let form = |pdf: &mut Pdf, content: Vec<u8>| {
        let dict = dictionary! {
            "Type" => "XObject", "Subtype" => "Form", "Resources" => dictionary! {},
            "BBox" => vec![0.into(), 0.into(), 9.into(), 9.into()],
        };
        let mut stream = Stream::new(dict, content);
        stream.compress().unwrap();
        pdf.add_object(stream)
    };
    let big = form(&mut pdf, b"0 0 m 9 9 l S\n".repeat(600_000));
    let mut names = Dictionary::new();
    let mut draws = Vec::new();
    for i in 0..2000 {
        names.set(format!("X{i}"), big);
        draws.extend(format!("/X{i} Do ").bytes());
    }
    let page = pdf.add_object(Stream::new(dictionary! {}, draws.clone()));
    let resources = Object::Dictionary(dictionary! { "XObject" => names });
    let known = DrawnForms::default();
    let started = std::time::Instant::now();
    for _ in 0..50 {
        let blocks = PageBlocks::read(&pdf, &[page], Some(&resources), LIM, &known).unwrap();
        assert!(!blocks.nested_in_forms);
    }
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    assert_eq!(known.nested_known(big), Some(false));

    // Past the cap.
    let cap = |decompressed| Limits {
        content: crate::source::ContentLimits {
            decompressed,
            ..LIM.content
        },
        ..*LIM
    };
    let fresh = DrawnForms::default();
    let blocks = PageBlocks::read(
        &pdf,
        &[page],
        Some(&resources),
        &cap(draws.len() + 99),
        &fresh,
    );
    assert!(blocks.unwrap().nested_in_forms);
    assert_eq!(fresh.nested_known(big), Some(true));
    // The page's share spent on the first form, the second isn't read.
    let small = [0, 1].map(|_| form(&mut pdf, b"0 0 m 9 9 l S\n".repeat(20)));
    let resources = Object::Dictionary(dictionary! {
        "XObject" => dictionary! { "A" => small[0], "B" => small[1] },
    });
    let content = [&b"/A Do /B Do %"[..], &[b'-'; 400]].concat();
    let page = pdf.add_object(Stream::new(dictionary! {}, content.clone()));
    let fresh = DrawnForms::default();
    let blocks = PageBlocks::read(
        &pdf,
        &[page],
        Some(&resources),
        &cap(content.len() + 99),
        &fresh,
    );
    assert!(blocks.unwrap().nested_in_forms);
    assert_eq!(fresh.nested_known(small[0]), Some(false));
    assert_eq!(fresh.nested_known(small[1]), None);
}
