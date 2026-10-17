//! The lexical layer: tokens that tile what they're lexed from, inline
//! images by each of PDFium's rules, errors that say where, the caps, and
//! a page's streams read and lexed as one.

use std::borrow::Cow;
use std::ops::Range;
use std::sync::Arc;

use lopdf::{Document as Pdf, LazyPdf, Object, ObjectId, Stream, dictionary};
use proptest::prelude::*;

use super::*;
use crate::source::{ContentLimits, Limits};

fn tokens<'a>(s: &'a [u8], res: &ResCx, lim: &Limits) -> Vec<(Range<usize>, Tok<'a>)> {
    lex(s, res, lim).map(Result::unwrap).collect()
}

/// `s`'s tokens under the default caps, with no resources.
fn toks(s: &[u8]) -> Vec<Tok<'_>> {
    tokens(s, &ResCx::none(), &Limits::DEFAULT)
        .into_iter()
        .map(|(_, t)| t)
        .collect()
}

fn error(s: &[u8], lim: &Limits) -> LexError {
    lex(s, &ResCx::none(), lim)
        .find_map(Result::err)
        .expect("an error")
}

/// Whether `gap` is white-space and comments only.
pub(super) fn blank(gap: &[u8]) -> bool {
    let mut comment = false;
    gap.iter().all(|&c| {
        comment = match c {
            b'%' => true,
            b'\r' | b'\n' => false,
            _ => comment,
        };
        comment || matches!(c, b'\0' | b'\t' | b'\n' | b'\x0c' | b'\r' | b' ')
    })
}

/// Lexed until the end or an error, `s`'s tokens are in order, each
/// within it, and only white-space and comments are between them; to the
/// end, after the last one too. The lexer stops after an error.
fn assert_tiles(s: &[u8], res: &ResCx, lim: &Limits) {
    let mut at = 0;
    let mut lexed = lex(s, res, lim);
    for token in lexed.by_ref() {
        let Ok((r, _)) = token else {
            assert!(lexed.next().is_none(), "nothing after an error");
            return;
        };
        assert!(
            at <= r.start && r.start < r.end && r.end <= s.len(),
            "{r:?} after {at}"
        );
        assert!(
            blank(&s[at..r.start]),
            "{:?}",
            String::from_utf8_lossy(&s[at..r.start])
        );
        at = r.end;
    }
    assert!(blank(&s[at..]), "{:?}", String::from_utf8_lossy(&s[at..]));
}

const PAGE: &[u8] = b"%!comment (not a string\n\
    q 0.5 0 0 -1.25 +.5 72. cm /Artifact <</Type /Pagination /Attached [/Top]>> BDC\n\
    BT /F#311 12 Tf [(a\\)(b) c) -250 <4142 43>] TJ (\\\\)Tj ET EMC\n\
    [1 [2 [3]] <</A <</B (c)>>>>] true false null Q { } ) ] >> > %end";

#[test]
fn tokens_are_as_written() {
    use Tok::{Op, Operand};
    assert_eq!(
        toks(PAGE),
        vec![
            Op(b"q"),
            Operand(b"0.5"),
            Operand(b"0"),
            Operand(b"0"),
            Operand(b"-1.25"),
            Operand(b"+.5"),
            Operand(b"72."),
            Op(b"cm"),
            Operand(b"/Artifact"),
            Operand(b"<</Type /Pagination /Attached [/Top]>>"),
            Op(b"BDC"),
            Op(b"BT"),
            Operand(b"/F#311"),
            Operand(b"12"),
            Op(b"Tf"),
            Operand(b"[(a\\)(b) c) -250 <4142 43>]"),
            Op(b"TJ"),
            Operand(b"(\\\\)"),
            Op(b"Tj"),
            Op(b"ET"),
            Op(b"EMC"),
            Operand(b"[1 [2 [3]] <</A <</B (c)>>>>]"),
            Operand(b"true"),
            Operand(b"false"),
            Operand(b"null"),
            Op(b"Q"),
            Operand(b"{"),
            Operand(b"}"),
            Operand(b")"),
            Operand(b"]"),
            Operand(b">>"),
            Operand(b">"),
        ]
    );
    assert_tiles(PAGE, &ResCx::none(), &Limits::DEFAULT);
}

#[test]
fn names_decode_their_escapes() {
    assert_eq!(name(b"/F#311").as_deref(), Some(&b"F11"[..]));
    assert_eq!(name(b"/A#20B").as_deref(), Some(&b"A B"[..]));
    assert!(matches!(name(b"/Plain"), Some(Cow::Borrowed(b"Plain"))));
    // An escape with no two hex digits stays as written.
    assert_eq!(name(b"/A#2").as_deref(), Some(&b"A#2"[..]));
    assert_eq!(name(b"/#zz").as_deref(), Some(&b"#zz"[..]));
    assert_eq!(name(b"/").as_deref(), Some(&b""[..]));
    assert_eq!(name(b"(F1)"), None);
}

#[test]
fn operands_read_as_objects() {
    use lopdf::StringFormat::{Hexadecimal, Literal};
    let o = |s: &[u8]| object(s, 8);
    assert_eq!(o(b"12"), Some(Object::Integer(12)));
    assert_eq!(o(b"-.5"), Some(Object::Real(-0.5)));
    assert_eq!(o(b"72."), Some(Object::Real(72.0)));
    assert_eq!(o(b"true"), Some(Object::Boolean(true)));
    assert_eq!(o(b"null"), Some(Object::Null));
    assert_eq!(o(b"/A#20B"), Some(Object::Name(b"A B".to_vec())));
    // Escapes, octal codes, a line continued, and an end of line.
    assert_eq!(
        o(b"(a\\(b\\)\\101\\0533\\\nc\r\nd)"),
        Some(Object::String(b"a(b)A+3c\nd".to_vec(), Literal))
    );
    assert_eq!(
        o(b"<48 65 6>"),
        Some(Object::String(b"He`".to_vec(), Hexadecimal))
    );
    assert_eq!(
        o(b"[1 /N (s) [2]]"),
        Some(Object::Array(vec![
            1.into(),
            Object::Name(b"N".to_vec()),
            Object::string_literal("s"),
            Object::Array(vec![2.into()]),
        ]))
    );
    // A key that isn't a name is passed over.
    assert_eq!(
        o(b"<</A 1 2 /B /C>>"),
        Some(Object::Dictionary(dictionary! { "A" => 1, "B" => "C" }))
    );
    for not in [&b"--5"[..], b")", b"Tj", b"1.2.3"] {
        assert_eq!(o(not), None, "{}", String::from_utf8_lossy(not));
    }
    // Stray closers inside are no elements; one past `depth`, or one that
    // doesn't end, is no object.
    assert_eq!(
        o(b"[1 >> <</A 2 ]>>]"),
        Some(Object::Array(vec![
            1.into(),
            Object::Dictionary(dictionary! { "A" => 2 }),
        ]))
    );
    assert_eq!(object(b"[[[1]]]", 2), None);
    assert!(object(b"[[1]]", 2).is_some());
    assert_eq!(o(b"[1 (2"), None);
}

/// An operand nested 500 deep around 8 MiB is read in one pass, not once
/// for each level it's inside.
#[test]
fn deep_operands_read_in_one_pass() {
    let mut s = b"[".repeat(500);
    s.push(b'<');
    s.resize(s.len() + (8 << 20), b'0');
    s.push(b'>');
    s.extend(b"]".repeat(500));
    let started = std::time::Instant::now();
    let mut o = object(&s, Limits::DEFAULT.content.depth).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    for _ in 0..500 {
        let Object::Array(mut inner) = o else {
            panic!("an array")
        };
        o = inner.pop().unwrap();
    }
    assert!(matches!(o, Object::String(bytes, _) if bytes.len() == 4 << 20));
}

/// `s`'s one inline image: where the token is, and its data.
fn image<'a>(s: &'a [u8], res: &ResCx) -> (Range<usize>, &'a [u8]) {
    tokens(s, res, &Limits::DEFAULT)
        .into_iter()
        .find_map(|(r, t)| match t {
            Tok::InlineImage(data) => Some((r, &s[data])),
            _ => None,
        })
        .expect("an inline image")
}

/// An inline image with `dict`, `data` and `EI`, between `q` and `Q`:
/// `data` is what's found, and `Q` follows it.
fn assert_image(dict: &str, data: &[u8], res: &ResCx) {
    let mut s = format!("q BI {dict} ID ").into_bytes();
    s.extend_from_slice(data);
    s.extend_from_slice(b"\nEI Q");
    let (r, found) = image(&s, res);
    assert_eq!(found, data, "{dict}");
    assert_eq!(&s[r.clone()][..2], b"BI");
    assert_eq!(&s[r.end - 2..r.end], b"EI");
    let ops: Vec<Tok> = tokens(&s, res, &Limits::DEFAULT)
        .into_iter()
        .map(|(_, t)| t)
        .collect();
    assert_eq!(ops.len(), 3, "{dict}");
    assert_eq!(ops[2], Tok::Op(b"Q"), "{dict}");
    assert_tiles(&s, res, &Limits::DEFAULT);
}

#[test]
fn unfiltered_data_is_its_exact_size() {
    let none = ResCx::none();
    // Data with ` EI ` in it: 4 bytes of gray.
    assert_image("/W 4 /H 1 /BPC 8 /CS /G", b" EI ", &none);
    assert_image(
        "/Width 2 /Height 2 /BitsPerComponent 8 /ColorSpace /DeviceGray",
        b"a EI",
        &none,
    );
    // RGB and CMYK.
    assert_image("/W 2 /H 1 /BPC 8 /CS /RGB", b" EI EI", &none);
    assert_image("/W 1 /H 1 /BPC 8 /CS /DeviceCMYK", b" EI ", &none);
    // Rows padded to a byte: 3 one-bit samples are a byte a row.
    assert_image("/W 3 /H 2 /BPC 1 /CS /G", b"EI", &none);
    // 16 bits a sample.
    assert_image("/W 1 /H 1 /BPC 16 /CS /G", b"EI", &none);
    // An image mask has no color space: a bit a sample.
    assert_image("/IM true /W 10 /H 2 /D [1 0]", b" EI ", &none);
    // An indexed image: one component, whatever its base.
    assert_image(
        "/W 2 /H 2 /BPC 8 /CS [/I /RGB 1 <000000FFFFFF>] /I true",
        b"EI E",
        &none,
    );
    assert_image(
        "/W 2 /H 1 /BPC 8 /CS [/DeviceN [/Cyan /Spot] /DeviceCMYK {}]",
        b" EI ",
        &none,
    );
    // A name the resources have, by its components; one they don't is
    // none, as in PDFium: one bit of one component.
    let res = ResCx::new([(b"CS0".to_vec(), 3), (b"Sp#1".to_vec(), 1)]);
    assert_image("/W 2 /H 1 /BPC 8 /CS /CS0", b" EI EI", &res);
    assert_image("/W 2 /H 1 /BPC 8 /CS /Sp#231", b"EI", &res);
    assert_image("/W 16 /H 1 /BPC 8 /CS /Absent", b"EI", &res);
    // Reals are their whole part.
    assert_image("/W 4.9 /H 1.0 /BPC 8 /CS /G", b" EI ", &none);
}

#[test]
fn filtered_data_ends_at_its_marker() {
    let none = ResCx::none();
    assert_image("/F /AHx /W 2 /H 2 /BPC 8 /CS /G", b"0E EI 0>", &none);
    assert_image(
        "/Filter /ASCIIHexDecode /W 2 /H 2 /BPC 8 /CS /G",
        b"0E EI 0>",
        &none,
    );
    assert_image(
        "/F [/A85 /Fl] /W 9 /H 9 /BPC 8 /CS /G",
        b"9jqo^ EI ~>",
        &none,
    );
    // RunLength: a run of 4 bytes, ` EI `, then its end.
    assert_image("/F /RL /W 4 /H 1 /BPC 8 /CS /G", b"\x03 EI \x80", &none);
    assert_image(
        "/F /DCT /W 9 /H 9 /BPC 8 /CS /RGB",
        b"\xff\xd8\xff\xe0 EI \x00\xff\xd9",
        &none,
    );
    // PDF 2.0's length, abbreviated or not.
    assert_image("/F /Fl /W 9 /H 9 /BPC 8 /CS /G /L 6", b"x EI y", &none);
    assert_image("/F /LZW /Length 6 /W 9 /H 9", b"x EI y", &none);
    // With nothing to go by, the first `EI` between white-space.
    assert_image(
        "/F /CCF /DP <</K -1 /Columns 9>> /W 9 /H 9",
        b"xEI EIy",
        &none,
    );
    assert_image("/F /Fl /W 9 /H 9 /BPC 8 /CS /G", b"", &none);
}

#[test]
fn an_image_ends_at_the_next_ei_past_its_data() {
    // Past the counted data, PDFium reads tokens to the next `EI`.
    let s = b"BI /W 1 /H 1 /BPC 8 /CS /G ID x 1 2 % EI\nEI Q";
    let (r, data) = image(s, &ResCx::none());
    assert_eq!(data, b"x");
    assert_eq!(&s[r.end..], b" Q");
    // And `EI` straight after the data.
    let s = b"BI /W 1 /H 1 /BPC 8 /CS /G ID xEI Q";
    assert_eq!(image(s, &ResCx::none()).1, b"x");
    // A key that isn't a name is passed over.
    let s = b"BI 7 /W 1 /H 1 /BPC 8 /CS /G ID x EI";
    assert_eq!(image(s, &ResCx::none()).1, b"x");
}

#[test]
fn errors_say_where() {
    let lim = &Limits::DEFAULT;
    let at = |s: &[u8]| {
        let e = error(s, lim);
        (e.at, e.kind)
    };
    use LexErrorKind::*;
    assert_eq!(at(b"0 0 m (a(b)c"), (6, String));
    assert_eq!(at(b"q <414"), (2, HexString));
    assert_eq!(at(b"[1 2 ("), (5, String));
    assert_eq!(at(b"[1 [2]"), (0, Array));
    assert_eq!(at(b"/P << /A [1] >"), (3, Dict));
    // A `]` doesn't close a dictionary: it's passed over.
    assert_eq!(at(b"/P [ << /A 1 ] "), (5, Dict));
    assert_eq!(at(b"q BI /W 1 /H 1"), (2, InlineImage));
    assert_eq!(at(b"q BI /W 1 ID abc"), (2, InlineImage));
    assert_eq!(
        at(b"BI /W 9 /H 9 /BPC 8 /CS /G ID abc EI"),
        (0, InlineImage)
    );
    assert_eq!(at(b"Q BI /F /AHx ID 0E EI"), (2, InlineImage));
    assert_eq!(at(b"BI /F /A85 ID 9jqo EI"), (0, InlineImage));
    assert_eq!(at(b"BI /F /RL ID \x05abc EI"), (0, InlineImage));
    // The tokens before an error come first.
    let lexed: Vec<_> = lex(b"q (a", &ResCx::none(), lim).collect();
    assert_eq!(lexed.len(), 2);
    assert_eq!(lexed[0], Ok((0..1, Tok::Op(b"q"))));
    assert!(lexed[1].is_err());
}

#[test]
fn the_caps_hold() {
    let capped = |content: ContentLimits| Limits {
        content,
        ..Limits::DEFAULT
    };
    let depth = Limits::DEFAULT.content.depth;
    let nested = |n: usize| [b"[".repeat(n), b"]".repeat(n)].concat();
    assert_eq!(toks(&nested(depth)).len(), 1);
    let e = error(&nested(depth + 1), &Limits::DEFAULT);
    assert_eq!((e.at, e.kind), (depth, LexErrorKind::Depth));
    let dicts = [b"<<".repeat(depth + 1), b">>".repeat(depth + 1)].concat();
    assert_eq!(error(&dicts, &Limits::DEFAULT).kind, LexErrorKind::Depth);

    let three = capped(ContentLimits {
        operands: 3,
        ..Limits::DEFAULT.content
    });
    assert_eq!(tokens(b"1 2 3 m 4 5 6 l", &ResCx::none(), &three).len(), 8);
    let e = error(b"1 2 3 4 m", &three);
    assert_eq!((e.at, e.kind), (6, LexErrorKind::Operands));

    let small = capped(ContentLimits {
        inline_image: 3,
        ..Limits::DEFAULT.content
    });
    let s = b"q BI /W 3 /H 1 /BPC 8 /CS /G ID abc EI BI /W 4 /H 1 /BPC 8 /CS /G ID abcd EI";
    let e = error(s, &small);
    assert_eq!((e.at, e.kind), (39, LexErrorKind::InlineImageSize));
}

/// Content of many inline images that don't end is read once: the end
/// of the first is searched for, not each one's.
#[test]
fn unended_images_dont_search_again() {
    let s = b"BI /F /DCT ID x EI ".repeat(50_000);
    let started = std::time::Instant::now();
    assert_eq!(lex(&s, &ResCx::none(), &Limits::DEFAULT).count(), 50_000);
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
}

/// Tokens made of what content is made of, run together.
fn content() -> impl Strategy<Value = Vec<u8>> {
    let pieces = prop::sample::select(vec![
        &b" "[..],
        b"\n",
        b"0",
        b"-1.5",
        b"/N",
        b"/W 2 /H 1 /BPC 8 /CS /G",
        b"/F /AHx",
        b"/F /DCT",
        b"/L 3",
        b"(",
        b")",
        b"\\",
        b"<",
        b">",
        b"<<",
        b">>",
        b"[",
        b"]",
        b"{",
        b"%",
        b"BI",
        b"ID",
        b"EI",
        b"~>",
        b"\xff\xd9",
        b"\x80",
        b"Tj",
        b"q",
        b"Q",
    ]);
    prop::collection::vec(pieces, 0..48).prop_map(|p| p.concat())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Whatever the bytes, the tokens tile them, up to an error.
    #[test]
    fn tokens_tile_any_content(s in content()) {
        let res = ResCx::new([(b"N".to_vec(), 3)]);
        assert_tiles(&s, &res, &Limits::DEFAULT);
    }
}

#[test]
fn streams_lex_as_one() {
    let first: &[u8] = b"q /P <</MCID 0>> BDC (ab";
    let second: &[u8] = b"c) Tj";
    let third: &[u8] = b"EMC Q";
    let c = Concat::new(&[first, second, third]);
    assert_eq!(c.bytes(), b"q /P <</MCID 0>> BDC (ab c) Tj EMC Q");
    assert_eq!(c.stream_count(), 3);
    assert_eq!(c.stream(1), second);
    assert_eq!(c.range(2), 31..36);
    let lexed = tokens(c.bytes(), &ResCx::none(), &Limits::DEFAULT);
    // The string runs from the first stream into the second, as in PDFium.
    let (string, _) = &lexed[4];
    assert_eq!(
        c.span(string.clone()),
        SpanAcross {
            start: Pos { stream: 0, at: 21 },
            end: Pos { stream: 1, at: 2 },
        }
    );
    let (emc, _) = &lexed[6];
    assert_eq!(c.span(emc.clone()).start, Pos { stream: 2, at: 0 });
    // The space between is the end of the stream before it.
    assert_eq!(c.pos(24), Pos { stream: 0, at: 24 });
    assert_eq!(c.pos(36), Pos { stream: 2, at: 5 });
    assert_eq!(c.lex(&ResCx::none(), &Limits::DEFAULT).count(), lexed.len());
    // One stream is lexed where it is.
    let one = Concat::new(&[first]);
    assert!(std::ptr::eq(one.bytes(), first));
}

/// A one-page document whose page's `/Contents` is what `contents` adds,
/// numbered from 1.
fn page_with(contents: impl FnOnce(&mut Pdf) -> Object) -> (Pdf, ObjectId) {
    let mut pdf = Pdf::with_version("1.7");
    let contents = contents(&mut pdf);
    let pages = pdf.new_object_id();
    let page = pdf.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages, "Contents" => contents,
        "MediaBox" => vec![0.into(), 0.into(), 100.into(), 100.into()],
    });
    pdf.objects.insert(
        pages,
        dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 }.into(),
    );
    let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
    pdf.trailer.set("Root", catalog);
    (pdf, page)
}

fn flate(data: &[u8]) -> Stream {
    use std::io::Write as _;
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(data).unwrap();
    Stream::new(
        dictionary! { "Filter" => "FlateDecode" },
        z.finish().unwrap(),
    )
}

fn read(pdf: &impl ReadObjects, page: &lopdf::Dictionary) -> Result<PageContent, ContentFault> {
    PageContent::read(pdf, &contents(page), &Limits::DEFAULT)
}

fn page_dict(pdf: &Pdf, page: ObjectId) -> lopdf::Dictionary {
    pdf.get_dictionary(page).unwrap().clone()
}

#[test]
fn a_pages_streams_read_as_one() {
    let (mut pdf, page) = page_with(|pdf| {
        let first = pdf.add_object(flate(b"q 1 0 0 rg"));
        let second = pdf.add_object(Stream::new(dictionary! {}, b"0 0 9 9 re f Q".to_vec()));
        // A missing object draws nothing.
        vec![first.into(), (90, 0).into(), second.into()].into()
    });
    let content = read(&pdf, &page_dict(&pdf, page)).unwrap();
    assert_eq!(content.concat().bytes(), b"q 1 0 0 rg 0 0 9 9 re f Q");
    assert_eq!(content.ids(), &[(1, 0), (2, 0)]);

    // Through a reference to an array, from a source.
    let array = pdf.add_object(Object::Array(vec![(2, 0).into(), (1, 0).into()]));
    pdf.get_dictionary_mut(page).unwrap().set("Contents", array);
    let mut bytes = Vec::new();
    pdf.save_to(&mut bytes).unwrap();
    let lazy = LazyPdf::open(Arc::<[u8]>::from(bytes), None, &Limits::DEFAULT.reader).unwrap();
    let content = read(&lazy, &page_dict(&pdf, page)).unwrap();
    assert_eq!(content.concat().bytes(), b"0 0 9 9 re f Q q 1 0 0 rg");
    // Read, not kept.
    assert!(!lazy.is_read((1, 0)) && !lazy.is_read((2, 0)) && !lazy.is_read(array));
    assert_eq!(content.into_bytes(), b"0 0 9 9 re f Q q 1 0 0 rg");
}

#[test]
fn a_stream_that_wont_decode_is_a_fault() {
    let (pdf, page) = page_with(|pdf| {
        let first = pdf.add_object(Stream::new(dictionary! {}, b"q".to_vec()));
        let bad = pdf.add_object(Stream::new(
            dictionary! { "Filter" => "JBIG2Decode" },
            b"Q".to_vec(),
        ));
        vec![first.into(), bad.into()].into()
    });
    let fault = read(&pdf, &page_dict(&pdf, page)).unwrap_err();
    assert!(
        matches!(fault, ContentFault::Stream { stream: (2, 0), .. }),
        "{fault}"
    );
    assert!(
        fault.to_string().starts_with("stream 2 0 R can't be read"),
        "{fault}"
    );
}

#[test]
fn streams_past_the_cap_are_a_fault() {
    let (pdf, page) = page_with(|pdf| {
        let ids: Vec<Object> = (0..3)
            .map(|_| pdf.add_object(flate(&[b' '; 1000])).into())
            .collect();
        ids.into()
    });
    let capped = |decompressed| Limits {
        content: ContentLimits {
            decompressed,
            ..Limits::DEFAULT.content
        },
        ..Limits::DEFAULT
    };
    let dict = page_dict(&pdf, page);
    // Three streams and the two spaces between.
    let content = PageContent::read(&pdf, &contents(&dict), &capped(3002)).unwrap();
    assert_eq!(content.concat().bytes().len(), 3002);
    for cap in [3001, 2500, 999] {
        let fault = PageContent::read(&pdf, &contents(&dict), &capped(cap)).unwrap_err();
        assert!(
            matches!(fault, ContentFault::TooLarge(c) if c == cap),
            "{fault}"
        );
    }
}

#[test]
fn a_lex_error_names_its_stream() {
    let (pdf, page) = page_with(|pdf| {
        let first = pdf.add_object(Stream::new(dictionary! {}, b"q".to_vec()));
        let second = pdf.add_object(Stream::new(dictionary! {}, b"0 g (open".to_vec()));
        vec![first.into(), second.into()].into()
    });
    let content = read(&pdf, &page_dict(&pdf, page)).unwrap();
    let e = content
        .concat()
        .lex(&ResCx::none(), &Limits::DEFAULT)
        .find_map(Result::err)
        .unwrap();
    assert_eq!(e.at, 6);
    let fault = content.fault(e);
    assert_eq!(
        fault.to_string(),
        "stream 2 0 R: a string that doesn't end at byte 4"
    );
}

#[test]
fn color_spaces_count_their_components() {
    let mut pdf = Pdf::with_version("1.7");
    let icc = pdf.add_object(Stream::new(dictionary! { "N" => 4 }, Vec::new()));
    let gray = pdf.add_object(Object::Name(b"DeviceGray".to_vec()));
    let spaces = pdf.add_object(dictionary! {
        "Icc" => vec!["ICCBased".into(), icc.into()],
        "Cmyk" => "DeviceCMYK",
        "Gray" => gray,
        "Idx" => vec!["Indexed".into(), "DeviceRGB".into(), 1.into(), Object::string_literal("abcdef")],
        "N2" => vec!["DeviceN".into(), vec!["a".into(), "b".into()].into(), "DeviceRGB".into()],
        "Lab" => vec!["Lab".into(), dictionary! {}.into()],
        "Odd" => 7,
    });
    let resources: Object = dictionary! { "ColorSpace" => spaces }.into();
    let res = ResCx::read(&pdf, Some(&resources));
    let want: [(&[u8], u32); 7] = [
        (b"Icc", 4),
        (b"Cmyk", 4),
        (b"Gray", 1),
        (b"Idx", 1),
        (b"N2", 2),
        (b"Lab", 3),
        (b"Odd", 3),
    ];
    assert_eq!(res, ResCx::new(want.map(|(n, c)| (n.to_vec(), c))));
    assert_eq!(ResCx::read(&pdf, None), ResCx::none());
    // An image in one is sized by it.
    assert_image("/W 1 /H 1 /BPC 8 /CS /Icc", b" EI ", &res);
}
