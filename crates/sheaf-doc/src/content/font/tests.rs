//! Fonts: widths that are Adobe's, encoders that are exact, `?` for what
//! a font can't draw, wrapping, and fonts as resources.

use std::collections::HashMap;

use lopdf::{Dictionary, Document as Pdf, Object, dictionary};

use super::*;
use crate::content::ResObjects;

/// An AFM's widths and codes, read here as plainly as it can be: each
/// `C <code> ; WX <width> ; N <name>` line.
fn afm(text: &str) -> (HashMap<&str, f32>, HashMap<u8, &str>) {
    let mut widths = HashMap::new();
    let mut codes = HashMap::new();
    for line in text.lines().filter(|l| l.starts_with("C ")) {
        let field = |key: &str| {
            line.split(';')
                .find_map(|f| f.trim().strip_prefix(key))
                .map(str::trim)
        };
        let name = field("N ").unwrap();
        widths.insert(name, field("WX ").unwrap().parse().unwrap());
        if let Ok(code) = field("C ").unwrap().parse::<u8>() {
            codes.insert(code, name);
        }
    }
    (widths, codes)
}

/// Windows-1252, from its code chart: None for the five codes it leaves
/// empty.
fn cp1252(code: u8) -> Option<char> {
    const HIGH: &str = "€\u{81}‚ƒ„…†‡ˆ‰Š‹Œ\u{8d}Ž\u{8f}\u{90}‘’“”•–—˜™š›œ\u{9d}žŸ";
    match code {
        0x81 | 0x8d | 0x8f | 0x90 | 0x9d => None,
        0x80..=0x9f => HIGH.chars().nth(code as usize - 0x80),
        _ => Some(code as char),
    }
}

/// Every code a font's encoding maps advances by its glyph's width in
/// Adobe's AFM, and every glyph it names is in the AFM.
#[test]
fn widths_equal_the_afm() {
    for font in Std14::ALL {
        let (widths, codes) = afm(font.afm_text());
        let mut mapped = 0;
        for code in 0..=255u8 {
            let advance = font.advance(&[code], 1000.0);
            let Some(glyph) = font.encoding().glyph(code) else {
                assert_eq!(advance, 0.0, "{font:?} {code:#x}");
                continue;
            };
            let width = widths.get(glyph).copied();
            assert_eq!(Some(advance), width, "{font:?} {code:#x} {glyph}");
            mapped += 1;
        }
        let expected = match font {
            // Printable ASCII, 27 of 0x80–0x9F, and 0xA0–0xFF.
            Std14::Symbol | Std14::ZapfDingbats => codes.len(),
            _ => 95 + 27 + 96,
        };
        assert_eq!(mapped, expected, "{font:?}");
        assert_eq!(font.afm().len(), widths.len(), "{font:?}");
    }
    let width = |font: Std14, s: &str| font.width(s, 1000.0);
    assert_eq!(width(Std14::Helvetica, "A"), 667.0);
    assert_eq!(width(Std14::Helvetica, "€"), 556.0);
    assert_eq!(width(Std14::HelveticaBold, "m"), 889.0);
    assert_eq!(width(Std14::Courier, "Œ€i"), 1800.0);
    assert_eq!(width(Std14::TimesRoman, "ƒ"), 500.0);
    assert_eq!(width(Std14::Symbol, "α"), 631.0);
    assert_eq!(width(Std14::ZapfDingbats, "✔"), 846.0);
    // At a size: per 1000 em.
    assert_eq!(
        Std14::Helvetica.width("AA", 12.0),
        2.0 * 667.0 * 12.0 / 1000.0
    );
}

/// WinAnsiEncoding is Windows-1252 exactly, all 256 codes: each character
/// it draws encodes to its code and its code reads as it; a control code
/// or an empty one draws nothing, so its character is `?`.
#[test]
fn win_ansi_is_cp1252_both_ways() {
    for code in 0..=255u8 {
        let c = cp1252(code);
        let drawn = c.filter(|c| !c.is_control());
        assert_eq!(
            Encoding::WinAnsi.text(code),
            drawn.map(String::from),
            "{code:#x}"
        );
        let Some(c) = c else { continue };
        for font in Std14::ALL
            .into_iter()
            .filter(|f| f.encoding() == Encoding::WinAnsi)
        {
            let encoded = font.encode(&c.to_string());
            match drawn {
                Some(_) => assert_eq!(encoded, (vec![code], Unencoded(0)), "{font:?} {c:?}"),
                None => assert_eq!(encoded, (b"?".to_vec(), Unencoded(1)), "{font:?} {c:?}"),
            }
            assert_eq!(font.covers(c), drawn.is_some());
        }
    }
    // The ten cp1252 characters outside Latin-1, which the old header
    // encoder drew as `?`.
    let (codes, none) = Std14::Helvetica.encode("Š š Ž ž Œ œ Ÿ ƒ ˆ ˜");
    assert_eq!(codes, b"\x8a \x9a \x8e \x9e \x8c \x9c \x9f \x83 \x88 \x98");
    assert_eq!(none, Unencoded(0));
    // The second space and the soft hyphen are their own.
    assert_eq!(Std14::Helvetica.encode("\u{a0}\u{ad} -").0, b"\xa0\xad -");
}

#[test]
fn what_a_font_cant_encode_draws_a_question_mark_and_is_counted() {
    assert_eq!(
        Std14::Helvetica.encode("a中文b€"),
        (b"a??b\x80".to_vec(), Unencoded(2))
    );
    assert_eq!(Std14::Helvetica.width("中", 10.0), 5.56);
    // Composed first: a decomposed é is one character.
    assert_eq!(
        Std14::TimesRoman.encode("e\u{301}"),
        (vec![0xe9], Unencoded(0))
    );
    assert!(!Std14::Helvetica.covers('α'));
    // Symbol has a question mark; ZapfDingbats hasn't, so it leaves the
    // character out.
    assert_eq!(Std14::Symbol.encode("αx"), (b"a?".to_vec(), Unencoded(1)));
    assert_eq!(
        Std14::ZapfDingbats.encode("✔x✘"),
        (b"48".to_vec(), Unencoded(1))
    );
}

#[test]
fn symbol_and_zapf_dingbats_encode_their_own() {
    let symbol = Std14::Symbol;
    assert_eq!(symbol.encode("αβ∑≠ ").0, b"ab\xe5\xb9 ");
    assert!(symbol.covers('π') && symbol.covers('€') && !symbol.covers('A'));
    // Omega, Delta and mu, as the Greek letters and as the AGL reads
    // Symbol's glyphs: the ohm sign, the increment and the micro sign.
    let greek = (
        symbol.encode("\u{3a9}\u{394}\u{3bc}"),
        symbol.encode("\u{2126}\u{2206}\u{b5}"),
    );
    assert_eq!(greek.0, (b"WDm".to_vec(), Unencoded(0)));
    assert_eq!(greek.1, greek.0);
    assert!(symbol.covers('\u{2126}') && symbol.covers('\u{3a9}'));
    // The checks and crosses: ✓ is a19, ✔ a20, ✗ a23, ✘ a24.
    let dingbats = Std14::ZapfDingbats;
    assert_eq!(dingbats.encode("✓✔✗✘").0, b"3478");
    assert_eq!(dingbats.encode("☎ ").0, b"\x25 ");
    assert!(dingbats.covers('❤') && !dingbats.covers('a'));
}

#[test]
fn wrap_breaks_at_spaces_and_inside_long_words() {
    let font = Std14::Courier;
    // Courier is 600 per 1000 em: at 10 pt, 6 a character.
    let wrap = |s: &str, chars: f32| font.wrap(s, 10.0, chars * 6.0);
    assert_eq!(wrap("one two three", 7.0), ["one two", "three"]);
    assert_eq!(wrap("one  two\nthree", 20.0), ["one two", "three"]);
    assert_eq!(wrap("abcdefghij k", 4.0), ["abcd", "efgh", "ij k"]);
    assert_eq!(wrap("", 4.0), [""]);
    assert_eq!(wrap("a\n\nb", 4.0), ["a", "", "b"]);
    // A line holds at least a character, however narrow.
    assert_eq!(wrap("ab", 0.5), ["a", "b"]);
}

#[test]
fn glyphless_encodes_utf16_at_half_an_em() {
    let (codes, none) = Glyphless.encode("Hé𝄞");
    assert_eq!(codes, [0x00, 0x48, 0x00, 0xe9, 0xd8, 0x34, 0xdd, 0x1e]);
    assert_eq!(none, Unencoded(0));
    assert_eq!(Glyphless.advance(&codes, 10.0), 20.0);
    assert!(Glyphless.covers('中'));
    // Decomposed text stays as it was recognized.
    assert_eq!(Glyphless.encode("e\u{301}").0, [0, 0x65, 0x03, 0x01]);
    // Any font goes where a `dyn Font` does.
    let fonts: [&dyn Font; 2] = [&Std14::Courier, &Glyphless];
    assert_eq!(fonts.map(|f| f.width("ab", 10.0)), [12.0, 10.0]);
}

/// A font as a resource is its dictionary and what that reaches, written
/// into a document with numbers of its own.
#[test]
fn fonts_as_resources() {
    let helvetica = Std14::Helvetica.resource();
    let [Object::Dictionary(dict)] = &helvetica.objects[..] else {
        panic!("one dictionary")
    };
    assert_eq!(
        dict,
        &dictionary! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
            "Encoding" => "WinAnsiEncoding",
        }
    );
    let symbol = Std14::Symbol.resource();
    assert!(!symbol.objects[0].as_dict().unwrap().has(b"Encoding"));
    // Named by what they are.
    assert_eq!(helvetica.id, Std14::Helvetica.resource().id);
    let ids: std::collections::HashSet<_> = Std14::ALL.map(|f| f.resource().id).into();
    assert_eq!(ids.len(), 14);
    let mut reordered = Dictionary::new();
    for (key, value) in dict.iter().rev() {
        reordered.set(key.clone(), value.clone());
    }
    assert_eq!(ResId::of(&[reordered.into()]), helvetica.id);
    assert_ne!(Glyphless.resource().id, helvetica.id);

    let mut pdf = Pdf::with_version("1.7");
    pdf.add_object(dictionary! {});
    let font = ResObjects::default().object(&mut pdf, &Glyphless.resource().into());
    assert_eq!(pdf.objects.len(), 7);
    let type0 = pdf.get_dictionary(font).unwrap();
    let cid = type0.get(b"DescendantFonts").unwrap().as_array().unwrap()[0]
        .as_reference()
        .unwrap();
    let cid = pdf.get_dictionary(cid).unwrap();
    assert_eq!(
        cid.get(b"Subtype").unwrap().as_name().unwrap(),
        b"CIDFontType2"
    );
    let descriptor = cid.get(b"FontDescriptor").unwrap().as_reference().unwrap();
    let program = pdf
        .get_dictionary(descriptor)
        .unwrap()
        .get(b"FontFile2")
        .unwrap()
        .as_reference()
        .unwrap();
    let program = pdf.get_object(program).unwrap().as_stream().unwrap();
    assert!(program.dict.has(b"Filter"), "compressed");
    assert_eq!(program.decompressed_content().unwrap(), Glyphless::PROGRAM);
    let to_unicode = type0.get(b"ToUnicode").unwrap().as_reference().unwrap();
    let to_unicode = pdf.get_object(to_unicode).unwrap().as_stream().unwrap();
    let cmap = to_unicode.decompressed_content().unwrap();
    assert!(cmap.windows(21).any(|w| w == b"<0000> <FFFF> <0000>\n"));
}
