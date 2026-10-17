//! The font OCR text is written in: Tesseract's approach
//! (`pdfrenderer.cpp`), a Type0 font over an embedded TrueType with one
//! empty glyph. Every CID draws that glyph and advances half an em, so a
//! word is fitted to its box by horizontal scaling alone, and the
//! ToUnicode CMap reads each CID back as the UTF-16 code unit it is: any
//! text round-trips.

use std::sync::{Arc, OnceLock};

use lopdf::{Object, Stream, dictionary};

use super::{Font, FontRes, Unencoded};
use crate::content::ResId;

/// Tesseract's glyphless font.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Glyphless;

impl Glyphless {
    /// The embedded program: Tesseract's glyphless TrueType (Apache-2.0;
    /// see `assets/LICENSE-pdf.ttf`).
    pub const PROGRAM: &[u8] = include_bytes!("../../../assets/pdf.ttf");
}

/// Each CID is the UTF-16 code unit it reads as.
const TO_UNICODE: &str = "/CIDInit /ProcSet findresource begin
12 dict begin
begincmap
/CIDSystemInfo
<<
  /Registry (Adobe)
  /Ordering (UCS)
  /Supplement 0
>> def
/CMapName /Adobe-Identity-UCS def
/CMapType 2 def
1 begincodespacerange
<0000> <FFFF>
endcodespacerange
1 beginbfrange
<0000> <FFFF> <0000>
endbfrange
endcmap
CMapName currentdict /CMap defineresource pop
end
end
";

impl Font for Glyphless {
    /// UTF-16 code units, each a two-byte CID, as written: nothing is
    /// unencoded.
    fn encode(&self, s: &str) -> (Vec<u8>, Unencoded) {
        let codes = s.encode_utf16().flat_map(u16::to_be_bytes).collect();
        (codes, Unencoded::default())
    }

    fn advance(&self, b: &[u8], size: f32) -> f32 {
        (b.len() / 2) as f32 * 500.0 * size / 1000.0
    }

    fn covers(&self, _: char) -> bool {
        true
    }

    /// The Type0 font, its CIDFontType2, descriptor and program, the
    /// CIDToGIDMap that sends every CID to glyph 1, and the ToUnicode
    /// CMap.
    fn resource(&self) -> FontRes {
        static RESOURCE: OnceLock<FontRes> = OnceLock::new();
        RESOURCE
            .get_or_init(|| {
                // Every CID maps to glyph 1, the font's one (empty) glyph.
                let map = [0u8, 1].repeat(65536);
                let objects: Arc<[Object]> = Arc::new([
                    dictionary! {
                        "Type" => "Font",
                        "Subtype" => "Type0",
                        "BaseFont" => "GlyphLessFont",
                        "Encoding" => "Identity-H",
                        "DescendantFonts" => vec![Object::Reference((1, 0))],
                        "ToUnicode" => Object::Reference((5, 0)),
                    }
                    .into(),
                    dictionary! {
                        "Type" => "Font",
                        "Subtype" => "CIDFontType2",
                        "BaseFont" => "GlyphLessFont",
                        "CIDSystemInfo" => dictionary! {
                            "Registry" => Object::string_literal("Adobe"),
                            "Ordering" => Object::string_literal("Identity"),
                            "Supplement" => 0i64,
                        },
                        "FontDescriptor" => Object::Reference((2, 0)),
                        "DW" => 500i64,
                        "CIDToGIDMap" => Object::Reference((4, 0)),
                    }
                    .into(),
                    dictionary! {
                        "Type" => "FontDescriptor",
                        "FontName" => "GlyphLessFont",
                        "Flags" => 5i64, // FixedPitch + Symbolic
                        "FontBBox" => vec![0.into(), 0.into(), 500.into(), 1000.into()],
                        "Ascent" => 1000i64,
                        "Descent" => -1i64,
                        "CapHeight" => 1000i64,
                        "ItalicAngle" => 0i64,
                        "StemV" => 80i64,
                        "FontFile2" => Object::Reference((3, 0)),
                    }
                    .into(),
                    Stream::new(
                        dictionary! { "Length1" => Glyphless::PROGRAM.len() as i64 },
                        Glyphless::PROGRAM.to_vec(),
                    )
                    .into(),
                    Stream::new(dictionary! {}, map).into(),
                    Stream::new(dictionary! {}, TO_UNICODE.as_bytes().to_vec()).into(),
                ]);
                FontRes {
                    id: ResId::of(&objects),
                    objects,
                }
            })
            .clone()
    }
}
