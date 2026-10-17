//! Simple fonts' encodings, code to glyph name, and glyph names to text
//! through the Adobe Glyph List and the ITC Zapf Dingbats Glyph List
//! (`assets/glyphlist.txt` and `assets/zapfdingbats.txt`;
//! `assets/LICENSE-agl`).

use std::collections::HashMap;
use std::sync::OnceLock;

use super::std14::Std14;

/// A simple font's encoding: which glyph each one-byte code draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Encoding {
    /// The Latin fonts' own (PDF 32000-1, Annex D).
    Standard,
    /// Windows-1252's characters, by the names PDF gives them.
    WinAnsi,
    MacRoman,
    /// Symbol's own, from its AFM.
    Symbol,
    /// ZapfDingbats' own, from its AFM.
    ZapfDingbats,
}

impl Encoding {
    /// The base encoding a font's `/Encoding` or `/BaseEncoding` names.
    /// `MacExpertEncoding`'s small capitals and old-style figures aren't
    /// read: it's None, as an unknown name is.
    pub fn named(name: &[u8]) -> Option<Encoding> {
        match name {
            b"StandardEncoding" => Some(Encoding::Standard),
            b"WinAnsiEncoding" => Some(Encoding::WinAnsi),
            b"MacRomanEncoding" => Some(Encoding::MacRoman),
            _ => None,
        }
    }

    /// The glyph `code` draws.
    pub fn glyph(self, code: u8) -> Option<&'static str> {
        match self {
            Encoding::Standard => Std14::Helvetica.afm().glyph(code),
            Encoding::WinAnsi => tables().win_ansi_names[code as usize],
            Encoding::MacRoman => tables().mac_roman_names[code as usize],
            Encoding::Symbol => Std14::Symbol.afm().glyph(code),
            Encoding::ZapfDingbats => Std14::ZapfDingbats.afm().glyph(code),
        }
    }

    /// The text `code` draws. WinAnsi's is Windows-1252's exactly, so its
    /// second space and hyphen, 0xA0 and 0xAD, are U+00A0 and U+00AD.
    pub fn text(self, code: u8) -> Option<String> {
        match self {
            Encoding::WinAnsi => cp1252(code).map(String::from),
            _ => glyph_text(self.glyph(code)?, self == Encoding::ZapfDingbats),
        }
    }
}

/// Windows-1252's character for `code`, where WinAnsiEncoding draws one:
/// none for the control codes or the five codes it leaves empty.
pub(crate) fn cp1252(code: u8) -> Option<char> {
    match code {
        0x20..=0x7e | 0xa0..=0xff => Some(code as char),
        0x80..=0x9f => Some(CP1252_HIGH[code as usize - 0x80]).filter(|&c| c != '\0'),
        _ => None,
    }
}

/// The text glyph `name` stands for, by the AGL specification's rules:
/// what follows a period is passed over, the parts an underscore joins
/// are each read, and a part is read from the ITC Zapf Dingbats list (in
/// a ZapfDingbats font), the Adobe Glyph List, `uniXXXX…` or `uXXXX` to
/// `uXXXXXX`. None when no part reads as anything.
pub(crate) fn glyph_text(name: &str, dingbats: bool) -> Option<String> {
    let name = name.split('.').next().unwrap_or_default();
    let mut text = String::new();
    for part in name.split('_') {
        let listed = dingbats
            .then(|| tables().dingbats.get(part))
            .flatten()
            .or_else(|| tables().agl.get(part));
        match listed {
            Some(t) => text.push_str(t),
            None => text.extend(uni(part).or_else(|| u(part)).into_iter().flatten()),
        }
    }
    (!text.is_empty()).then_some(text)
}

/// `uniXXXX…`: groups of four upper-case hex digits, none a surrogate.
fn uni(part: &str) -> Option<Vec<char>> {
    let hex = part.strip_prefix("uni")?;
    if hex.is_empty() || hex.len() % 4 != 0 {
        return None;
    }
    (0..hex.len())
        .step_by(4)
        .map(|i| scalar(&hex[i..i + 4]))
        .collect()
}

/// `uXXXX` to `uXXXXXX`: one scalar value, in upper-case hex digits.
fn u(part: &str) -> Option<Vec<char>> {
    let hex = part.strip_prefix('u')?;
    (4..=6).contains(&hex.len()).then_some(())?;
    Some(vec![scalar(hex)?])
}

/// Upper-case hex digits as a Unicode scalar value.
fn scalar(hex: &str) -> Option<char> {
    if !hex.bytes().all(|c| matches!(c, b'0'..=b'9' | b'A'..=b'F')) {
        return None;
    }
    char::from_u32(u32::from_str_radix(hex, 16).ok()?)
}

/// The glyph lists and the encodings' tables, read once.
struct Tables {
    agl: HashMap<&'static str, String>,
    dingbats: HashMap<&'static str, String>,
    win_ansi_names: [Option<&'static str>; 256],
    mac_roman_names: [Option<&'static str>; 256],
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let mut win_ansi_names = [None; 256];
        names(&mut win_ansi_names, 0x20, WIN_ANSI);
        let mut mac_roman_names = win_ansi_names;
        names(&mut mac_roman_names, 0x80, MAC_ROMAN_HIGH);
        Tables {
            agl: list(include_str!("../../../assets/glyphlist.txt")),
            dingbats: list(include_str!("../../../assets/zapfdingbats.txt")),
            win_ansi_names,
            mac_roman_names,
        }
    })
}

/// A glyph list's `name;XXXX[ XXXX…]` lines.
fn list(text: &'static str) -> HashMap<&'static str, String> {
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let (name, values) = l.split_once(';')?;
            let text = values.split(' ').map(scalar).collect::<Option<String>>()?;
            Some((name, text))
        })
        .collect()
}

/// `table`'s codes from `from` on, named by `names`' words in turn; `-`
/// is no glyph.
fn names(table: &mut [Option<&'static str>; 256], from: u8, names: &'static str) {
    let slots = table.iter_mut().skip(from as usize);
    for (slot, name) in slots.zip(names.split_whitespace()) {
        *slot = Some(name).filter(|&n| n != "-");
    }
}

/// WinAnsiEncoding's glyphs (PDF 32000-1, Annex D) from 0x20, sixteen a
/// line.
const WIN_ANSI: &str = "
    space exclam quotedbl numbersign dollar percent ampersand quotesingle
        parenleft parenright asterisk plus comma hyphen period slash
    zero one two three four five six seven eight nine colon semicolon less equal greater question
    at A B C D E F G H I J K L M N O
    P Q R S T U V W X Y Z bracketleft backslash bracketright asciicircum underscore
    grave a b c d e f g h i j k l m n o
    p q r s t u v w x y z braceleft bar braceright asciitilde -
    Euro - quotesinglbase florin quotedblbase ellipsis dagger daggerdbl
        circumflex perthousand Scaron guilsinglleft OE - Zcaron -
    - quoteleft quoteright quotedblleft quotedblright bullet endash emdash
        tilde trademark scaron guilsinglright oe - zcaron Ydieresis
    space exclamdown cent sterling currency yen brokenbar section
        dieresis copyright ordfeminine guillemotleft logicalnot hyphen registered macron
    degree plusminus twosuperior threesuperior acute mu paragraph periodcentered
        cedilla onesuperior ordmasculine guillemotright onequarter onehalf threequarters questiondown
    Agrave Aacute Acircumflex Atilde Adieresis Aring AE Ccedilla
        Egrave Eacute Ecircumflex Edieresis Igrave Iacute Icircumflex Idieresis
    Eth Ntilde Ograve Oacute Ocircumflex Otilde Odieresis multiply
        Oslash Ugrave Uacute Ucircumflex Udieresis Yacute Thorn germandbls
    agrave aacute acircumflex atilde adieresis aring ae ccedilla
        egrave eacute ecircumflex edieresis igrave iacute icircumflex idieresis
    eth ntilde ograve oacute ocircumflex otilde odieresis divide
        oslash ugrave uacute ucircumflex udieresis yacute thorn ydieresis
";

/// MacRomanEncoding's glyphs from 0x80, sixteen a line, with the Mac OS
/// symbols Annex D leaves out but for the Apple logo; below, it's
/// WinAnsi's.
const MAC_ROMAN_HIGH: &str = "
    Adieresis Aring Ccedilla Eacute Ntilde Odieresis Udieresis aacute
        agrave acircumflex adieresis atilde aring ccedilla eacute egrave
    ecircumflex edieresis iacute igrave icircumflex idieresis ntilde oacute
        ograve ocircumflex odieresis otilde uacute ugrave ucircumflex udieresis
    dagger degree cent sterling section bullet paragraph germandbls
        registered copyright trademark acute dieresis notequal AE Oslash
    infinity plusminus lessequal greaterequal yen mu partialdiff summation
        product pi integral ordfeminine ordmasculine Omega ae oslash
    questiondown exclamdown logicalnot radical florin approxequal Delta guillemotleft
        guillemotright ellipsis space Agrave Atilde Otilde OE oe
    endash emdash quotedblleft quotedblright quoteleft quoteright divide lozenge
        ydieresis Ydieresis fraction currency guilsinglleft guilsinglright fi fl
    daggerdbl periodcentered quotesinglbase quotedblbase perthousand Acircumflex Ecircumflex Aacute
        Edieresis Egrave Iacute Icircumflex Idieresis Igrave Oacute Ocircumflex
    - Ograve Uacute Ucircumflex Ugrave dotlessi circumflex tilde
        macron breve dotaccent ring cedilla hungarumlaut ogonek caron
";

/// Windows-1252's characters from 0x80 to 0x9F; `\0` for its five empty
/// codes.
const CP1252_HIGH: [char; 32] = [
    '€', '\0', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\0', 'Ž', '\0', '\0', '‘',
    '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\0', 'ž', 'Ÿ',
];
