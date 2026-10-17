//! The standard 14 fonts: every PDF reader has them, so sheaf draws in
//! them without embedding anything. The twelve Latin ones are written in
//! WinAnsiEncoding, Symbol and ZapfDingbats in their own encodings, and
//! every width is Adobe's, from the font's AFM.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use lopdf::{Object, dictionary};
use unicode_normalization::UnicodeNormalization;

use super::afm::Afm;
use super::encoding::{Encoding, glyph_text};
use super::{Font, FontRes, Unencoded};
use crate::content::ResId;

/// A standard 14 font.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Std14 {
    Courier,
    CourierBold,
    CourierOblique,
    CourierBoldOblique,
    Helvetica,
    HelveticaBold,
    HelveticaOblique,
    HelveticaBoldOblique,
    TimesRoman,
    TimesBold,
    TimesItalic,
    TimesBoldItalic,
    Symbol,
    ZapfDingbats,
}

impl Std14 {
    pub const ALL: [Std14; 14] = [
        Std14::Courier,
        Std14::CourierBold,
        Std14::CourierOblique,
        Std14::CourierBoldOblique,
        Std14::Helvetica,
        Std14::HelveticaBold,
        Std14::HelveticaOblique,
        Std14::HelveticaBoldOblique,
        Std14::TimesRoman,
        Std14::TimesBold,
        Std14::TimesItalic,
        Std14::TimesBoldItalic,
        Std14::Symbol,
        Std14::ZapfDingbats,
    ];

    /// Its `/BaseFont`.
    pub fn name(self) -> &'static str {
        match self {
            Std14::Courier => "Courier",
            Std14::CourierBold => "Courier-Bold",
            Std14::CourierOblique => "Courier-Oblique",
            Std14::CourierBoldOblique => "Courier-BoldOblique",
            Std14::Helvetica => "Helvetica",
            Std14::HelveticaBold => "Helvetica-Bold",
            Std14::HelveticaOblique => "Helvetica-Oblique",
            Std14::HelveticaBoldOblique => "Helvetica-BoldOblique",
            Std14::TimesRoman => "Times-Roman",
            Std14::TimesBold => "Times-Bold",
            Std14::TimesItalic => "Times-Italic",
            Std14::TimesBoldItalic => "Times-BoldItalic",
            Std14::Symbol => "Symbol",
            Std14::ZapfDingbats => "ZapfDingbats",
        }
    }

    /// The encoding sheaf writes it in.
    pub(crate) fn encoding(self) -> Encoding {
        match self {
            Std14::Symbol => Encoding::Symbol,
            Std14::ZapfDingbats => Encoding::ZapfDingbats,
            _ => Encoding::WinAnsi,
        }
    }

    /// Its AFM, as Adobe wrote it but for the kerning.
    pub(crate) fn afm_text(self) -> &'static str {
        match self {
            Std14::Courier => include_str!("../../../assets/afm/Courier.afm"),
            Std14::CourierBold => include_str!("../../../assets/afm/Courier-Bold.afm"),
            Std14::CourierOblique => include_str!("../../../assets/afm/Courier-Oblique.afm"),
            Std14::CourierBoldOblique => {
                include_str!("../../../assets/afm/Courier-BoldOblique.afm")
            }
            Std14::Helvetica => include_str!("../../../assets/afm/Helvetica.afm"),
            Std14::HelveticaBold => include_str!("../../../assets/afm/Helvetica-Bold.afm"),
            Std14::HelveticaOblique => include_str!("../../../assets/afm/Helvetica-Oblique.afm"),
            Std14::HelveticaBoldOblique => {
                include_str!("../../../assets/afm/Helvetica-BoldOblique.afm")
            }
            Std14::TimesRoman => include_str!("../../../assets/afm/Times-Roman.afm"),
            Std14::TimesBold => include_str!("../../../assets/afm/Times-Bold.afm"),
            Std14::TimesItalic => include_str!("../../../assets/afm/Times-Italic.afm"),
            Std14::TimesBoldItalic => include_str!("../../../assets/afm/Times-BoldItalic.afm"),
            Std14::Symbol => include_str!("../../../assets/afm/Symbol.afm"),
            Std14::ZapfDingbats => include_str!("../../../assets/afm/ZapfDingbats.afm"),
        }
    }

    /// Its metrics, read once.
    pub(super) fn afm(self) -> &'static Afm {
        static AFMS: [OnceLock<Afm>; 14] = [const { OnceLock::new() }; 14];
        AFMS[self as usize].get_or_init(|| Afm::parse(self.afm_text()))
    }

    /// Its codes in [`Std14::encoding`], read once.
    fn face(self) -> &'static Face {
        static FACES: [OnceLock<Face>; 14] = [const { OnceLock::new() }; 14];
        FACES[self as usize].get_or_init(|| Face::new(self))
    }
}

/// A font's encoding, both ways, with each code's advance.
struct Face {
    /// Per 1000 em; 0 where the encoding has no glyph.
    widths: [u16; 256],
    /// The code each character it draws has.
    codes: HashMap<char, u8>,
    /// What it draws for a character it hasn't: `?` where it has one.
    unknown: Option<u8>,
}

impl Face {
    fn new(font: Std14) -> Face {
        let encoding = font.encoding();
        let afm = font.afm();
        let mut face = Face {
            widths: [0; 256],
            codes: HashMap::new(),
            unknown: None,
        };
        // From the top, so where two codes draw one character the lower
        // one stays.
        for code in (0..=255u8).rev() {
            let Some(glyph) = encoding.glyph(code) else {
                continue;
            };
            let Some(width) = afm.width(glyph) else {
                continue;
            };
            face.widths[code as usize] = width;
            // The AGL reads three of Symbol's Greek letters as the math
            // signs (∆ U+2206, Ω U+2126, µ U+00B5); the letters, its
            // `…greek` names, are written with them too.
            let greek = (encoding == Encoding::Symbol)
                .then(|| glyph_text(&format!("{glyph}greek"), false))
                .flatten()
                .filter(|t| t.chars().all(char::is_alphabetic));
            for text in [encoding.text(code), greek].into_iter().flatten() {
                let mut chars = text.nfc();
                if let (Some(c), None) = (chars.next(), chars.next()) {
                    face.codes.insert(c, code);
                }
            }
        }
        face.unknown = face.codes.get(&'?').copied();
        face
    }
}

impl Font for Std14 {
    /// Composed first (NFC), so a decomposed "é" is WinAnsi's 0xE9. Where
    /// the encoding has a character twice, the lower code is written.
    fn encode(&self, s: &str) -> (Vec<u8>, Unencoded) {
        let face = self.face();
        let mut unencoded = Unencoded::default();
        let mut out = Vec::with_capacity(s.len());
        for c in s.nfc() {
            match face.codes.get(&c) {
                Some(&code) => out.push(code),
                None => {
                    unencoded.0 += 1;
                    out.extend(face.unknown);
                }
            }
        }
        (out, unencoded)
    }

    fn advance(&self, b: &[u8], size: f32) -> f32 {
        let widths = &self.face().widths;
        b.iter()
            .map(|&c| f32::from(widths[c as usize]))
            .sum::<f32>()
            * size
            / 1000.0
    }

    /// As [`Font::encode`] would: `c` composed.
    fn covers(&self, c: char) -> bool {
        let mut composed = std::iter::once(c).nfc();
        match (composed.next(), composed.next()) {
            (Some(c), None) => self.face().codes.contains_key(&c),
            _ => false,
        }
    }

    /// Its dictionary, made once.
    fn resource(&self) -> FontRes {
        static RESOURCES: [OnceLock<FontRes>; 14] = [const { OnceLock::new() }; 14];
        RESOURCES[*self as usize]
            .get_or_init(|| {
                let mut dict = dictionary! {
                    "Type" => "Font",
                    "Subtype" => "Type1",
                    "BaseFont" => self.name(),
                };
                if self.encoding() == Encoding::WinAnsi {
                    dict.set("Encoding", "WinAnsiEncoding");
                }
                let objects: Arc<[Object]> = Arc::new([dict.into()]);
                FontRes {
                    id: ResId::of(&objects),
                    objects,
                }
            })
            .clone()
    }
}
