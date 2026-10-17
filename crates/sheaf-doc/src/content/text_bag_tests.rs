//! TextBags: the same words whatever fonts and codes draw them, read
//! through `/ToUnicode`, `/Differences` and the base encodings.

use lopdf::{Document as Pdf, Object, Stream, dictionary};

use super::*;
use crate::source::Limits;

/// The bag `content` draws with the fonts of `resources`.
fn bag(pdf: &Pdf, resources: &Object, content: &[u8]) -> TextBag {
    let lim = Limits::DEFAULT;
    let fonts = FontDecoders::read(pdf, Some(resources), &lim);
    text_bag(&Concat::new(&[content]), &ResCx::none(), &lim, &fonts).unwrap()
}

/// A bag of `text`'s words.
fn words(text: &str) -> TextBag {
    let mut bag = TextBag::default();
    bag.add(text);
    bag
}

/// `cmap`'s mappings in a ToUnicode CMap stream.
fn to_unicode(pdf: &mut Pdf, cmap: &str) -> Object {
    let cmap = format!(
        "/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n{cmap}\n\
         endcmap CMapName currentdict /CMap defineresource pop end end"
    );
    let mut stream = Stream::new(dictionary! {}, cmap.into_bytes());
    let _ = stream.compress();
    pdf.add_object(stream).into()
}

/// "Hello World" in a simple TrueType subset whose `/Differences` give
/// its glyphs codes from 1, and in a Type0 Identity-H subset whose
/// ToUnicode CMap reads other codes: the same bag.
#[test]
fn the_same_text_in_two_subsets_gives_equal_bags() {
    let mut pdf = Pdf::with_version("1.7");
    let simple = pdf.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "TrueType", "BaseFont" => "ABCDEF+Arial",
        "FirstChar" => 1, "LastChar" => 8,
        "Encoding" => dictionary! {
            "Type" => "Encoding", "BaseEncoding" => "WinAnsiEncoding",
            "Differences" => vec![
                1.into(), "H".into(), "e".into(), "l".into(), "o".into(), "W".into(),
                "r".into(), "uni0064".into(), "space".into(),
            ],
        },
    });
    let cmap = to_unicode(
        &mut pdf,
        "1 begincodespacerange <0000> <FFFF> endcodespacerange\n\
         3 beginbfchar <0010> <0048> <0011> <0065> <0003> <0020> endbfchar\n\
         2 beginbfrange <0012> <0013> <006C> <0020> <0022> [<0057> <006F> <0072>] endbfrange\n\
         1 beginbfchar <0030> <0064> endbfchar",
    );
    let composite = pdf.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type0", "BaseFont" => "GHIJKL+Arial",
        "Encoding" => "Identity-H", "ToUnicode" => cmap,
    });
    let resources: Object = dictionary! {
        "Font" => dictionary! { "F1" => simple, "F2" => composite },
    }
    .into();
    // H e l l o space W o r l d, in each font's codes.
    let simple = bag(
        &pdf,
        &resources,
        b"BT /F1 12 Tf <01020303040805040603 07> Tj ET",
    );
    let composite = bag(
        &pdf,
        &resources,
        b"BT /F2 12 Tf [<0010001100120012> -20 <0021000300200021>] TJ <00220012 0030> Tj ET",
    );
    assert_eq!(simple, words("Hello World"));
    assert_eq!(composite, simple);
    // The bfrange's string counts up: <0013> is "m".
    assert_eq!(bag(&pdf, &resources, b"/F2 1 Tf <0013> Tj"), words("m"));
}

/// é composed and decomposed, as two fonts' CMaps read their codes: the
/// same word.
#[test]
fn composed_and_decomposed_text_give_equal_bags() {
    let mut pdf = Pdf::with_version("1.7");
    let decomposed = to_unicode(&mut pdf, "1 beginbfchar <01> <00650301> endbfchar");
    let decomposed = pdf.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
        "ToUnicode" => decomposed,
    });
    let win_ansi = pdf.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding",
    });
    let resources: Object = dictionary! {
        "Font" => dictionary! { "D" => decomposed, "W" => win_ansi },
    }
    .into();
    let one = bag(&pdf, &resources, b"BT /D 9 Tf (caf\\001) Tj ET");
    let other = bag(&pdf, &resources, b"BT /W 9 Tf (caf\\351) Tj ET");
    assert_eq!(one, words("café"));
    assert_eq!(one, other);
    assert_eq!(words("cafe\u{301} cafe\u{301}"), words("café café"));
    assert_eq!(
        words("café café").words().collect::<Vec<_>>(),
        [("café", 2)]
    );
}

/// A word ends at white-space, at `BT` and `ET`, and where the text moves;
/// `Tf` picks the font, and `Q` restores the one `q` saved.
#[test]
fn words_end_where_the_text_moves() {
    let mut pdf = Pdf::with_version("1.7");
    let symbol = pdf.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Symbol",
    });
    let standard = pdf.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Times-Roman",
    });
    let resources: Object = dictionary! {
        "Font" => dictionary! { "S" => symbol, "T" => standard },
    }
    .into();
    let read = |content: &[u8]| {
        let bag = bag(&pdf, &resources, content);
        bag.words()
            .flat_map(|(w, n)| std::iter::repeat_n(w.to_string(), n))
            .collect::<Vec<_>>()
    };
    // Symbol's own encoding: "a" is alpha. Standard: 0x27 is ’.
    assert_eq!(
        read(b"BT /T 9 Tf (it\\047s) Tj q /S 9 Tf (a) Tj Q (b) Tj ET"),
        ["it’sαb"]
    );
    assert_eq!(
        read(b"BT (one) Tj 0 -12 Td (two) Tj T* (three) ' ET"),
        ["one", "three", "two"]
    );
    assert_eq!(
        read(b"BT (a) Tj ET BT (b) Tj ET (c d) Tj"),
        ["a", "b", "c", "d"]
    );
    // No font, or one the resources haven't: the bytes as they are.
    assert_eq!(read(b"BT /X 9 Tf (caf\\351) Tj ET"), ["café"]);
    assert_eq!(bag(&pdf, &resources, b"").len(), 0);
    assert!(bag(&pdf, &resources, b"q Q").is_empty());
}

/// A composite font whose encoding isn't Identity: its CMap's codespace
/// ranges say how long each code is.
#[test]
fn codespace_ranges_split_mixed_codes() {
    let mut pdf = Pdf::with_version("1.7");
    let cmap = to_unicode(
        &mut pdf,
        "2 begincodespacerange <00> <7F> <8140> <9FFC> endcodespacerange\n\
         2 beginbfchar <41> <0041> <8140> <3000> endbfchar\n\
         1 beginbfrange <889F> <88A0> <4E9C> endbfrange",
    );
    let font = pdf.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type0", "BaseFont" => "MSGothic",
        "Encoding" => "90ms-RKSJ-H", "ToUnicode" => cmap,
    });
    let resources: Object = dictionary! { "Font" => dictionary! { "J" => font } }.into();
    // A, then 889F and 88A0 as two-byte codes, counting up from U+4E9C,
    // then 8140, U+3000, an ideographic space.
    assert_eq!(
        bag(&pdf, &resources, b"BT /J 9 Tf <41889F88A0814041> Tj ET"),
        words("A\u{4e9c}\u{4e9d} A")
    );
}

/// A string that doesn't end is an error; content with no text is an
/// empty bag.
#[test]
fn content_that_doesnt_lex_is_an_error() {
    let lim = Limits::DEFAULT;
    let fonts = FontDecoders::default();
    let content = Concat::new(&[b"BT (Hello) Tj (open"]);
    let e = text_bag(&content, &ResCx::none(), &lim, &fonts).unwrap_err();
    assert_eq!(e.at, 14);
}
