//! Text as words: what content's text-showing operators draw, decoded
//! through each font's `/ToUnicode` or encoding, composed (NFC) and
//! counted by word. The same text drawn in two fonts, with different
//! codes, gives equal bags: what the removal check compares (§11.9).

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};

use lopdf::{Dictionary, Object};
use unicode_normalization::UnicodeNormalization;

use super::font::{Encoding, glyph_text};
use super::lex::{self, LexError, ResCx, Tok};
use super::page::{Concat, ReadObjects, direct};
use crate::source::Limits;

/// Words, each with how many times it's drawn.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextBag {
    words: BTreeMap<String, usize>,
}

impl TextBag {
    /// Counts `text`'s words: composed (NFC), and split at white-space.
    pub fn add(&mut self, text: &str) {
        let text: String = text.nfc().collect();
        for word in text.split_whitespace() {
            *self.words.entry(word.to_string()).or_default() += 1;
        }
    }

    /// Each word, in order, with how many times it's drawn.
    pub fn words(&self) -> impl Iterator<Item = (&str, usize)> {
        self.words.iter().map(|(w, &n)| (w.as_str(), n))
    }

    /// How many words it holds, each as many times as it's drawn.
    pub fn len(&self) -> usize {
        self.words.values().sum()
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }
}

/// The words `content`, lexed with `res`, draws with `fonts`. `Tf` picks
/// the font, and `Q` restores the one `q` saved. The strings `Tj`, `TJ`,
/// `'` and `"` show are decoded and run together, and a word ends at
/// white-space, at `BT` and `ET`, and where the text moves to a new place
/// (`Td`, `TD`, `Tm`, `T*`, `'`, `"`); `TJ`'s adjustments don't end one.
/// A font the resources haven't reads each byte as the code it is.
pub fn text_bag(
    content: &Concat,
    res: &ResCx,
    lim: &Limits,
    fonts: &FontDecoders,
) -> Result<TextBag, LexError> {
    let mut bag = TextBag::default();
    let mut run = String::new();
    let mut font = None;
    let mut saved = Vec::new();
    let depth = lim.content.depth;
    let show = |run: &mut String, font: Option<&Decoder>, operand: Option<&[u8]>| match operand
        .and_then(|o| lex::object(o, depth))
    {
        Some(Object::String(s, _)) => decode(font, &s, run),
        Some(Object::Array(items)) => {
            for item in items {
                if let Object::String(s, _) = item {
                    decode(font, &s, run);
                }
            }
        }
        _ => {}
    };
    lex::steps(content.bytes(), res, lim, |step| {
        let Tok::Op(op) = step.tok else {
            return Ok::<_, LexError>(());
        };
        match op {
            b"q" => saved.push(font),
            b"Q" => font = saved.pop().unwrap_or(font),
            b"Tf" => font = step.name(1).and_then(|n| fonts.get(&n)),
            b"Tj" | b"TJ" => show(&mut run, font, step.arg(0)),
            b"'" | b"\"" => {
                bag.add(&std::mem::take(&mut run));
                show(&mut run, font, step.arg(0));
            }
            b"BT" | b"ET" | b"Td" | b"TD" | b"Tm" | b"T*" => bag.add(&std::mem::take(&mut run)),
            _ => {}
        }
        Ok(())
    })?;
    bag.add(&run);
    Ok(bag)
}

/// `s`'s codes in `font` as text, pushed onto `run`; with no font, each
/// byte as the code it is.
fn decode(font: Option<&Decoder>, s: &[u8], run: &mut String) {
    match font {
        Some(font) => font.decode(s, run),
        None => run.extend(s.iter().map(|&b| char::from(b))),
    }
}

/// How the fonts of some resources read as text, by resource name.
#[derive(Debug, Clone, Default)]
pub struct FontDecoders {
    fonts: HashMap<Vec<u8>, Decoder>,
}

impl FontDecoders {
    /// The fonts of `resources`, a page's or a form's `/Resources` as
    /// written. Each reads its codes as text through, in order:
    /// - its `/ToUnicode` CMap: `bfchar`, `bfrange` with a string or an
    ///   array, and a composite font's codespace ranges, which say how
    ///   long its codes are where its encoding isn't Identity;
    /// - a simple font's `/Differences`, by glyph name, through the Adobe
    ///   Glyph List and `uniXXXX`/`uXXXXX` names;
    /// - its base encoding: WinAnsi, MacRoman or Standard as named, or
    ///   its own: Symbol's or ZapfDingbats' for those, Standard for
    ///   another Type 1 font;
    /// - otherwise the code as is.
    pub fn read(
        objects: &impl ReadObjects,
        resources: Option<&Object>,
        lim: &Limits,
    ) -> FontDecoders {
        let fonts = resources
            .and_then(|r| direct(objects, r))
            .and_then(|r| r.as_dict().ok())
            .and_then(|r| direct(objects, r.get(b"Font").ok()?))
            .and_then(|f| f.as_dict().ok());
        FontDecoders {
            fonts: fonts
                .into_iter()
                .flat_map(|fonts| fonts.iter())
                .filter_map(|(name, font)| {
                    let font = direct(objects, font)?.as_dict().ok()?;
                    Some((name.clone(), Decoder::read(objects, font, lim)))
                })
                .collect(),
        }
    }

    fn get(&self, name: &[u8]) -> Option<&Decoder> {
        self.fonts.get(name)
    }
}

/// How one font's strings read as text.
#[derive(Debug, Clone)]
struct Decoder {
    /// Bytes a code takes where the codespace ranges don't say.
    width: usize,
    /// Whether the codespace ranges say how long a code is.
    ranged: bool,
    cmap: CMap,
    /// A simple font's codes, by its encoding.
    encoded: Option<Box<[Option<String>]>>,
}

impl Decoder {
    fn read(objects: &impl ReadObjects, font: &Dictionary, lim: &Limits) -> Decoder {
        let name = |key: &[u8]| {
            let object = direct(objects, font.get(key).ok()?)?;
            object.as_name().ok()
        };
        let cmap = font
            .get(b"ToUnicode")
            .ok()
            .and_then(|o| o.as_reference().ok())
            .and_then(|id| {
                let cap = lim.content.decompressed;
                let read = |o: &Object| {
                    o.as_stream()
                        .ok()?
                        .decompressed_content_with_limit(cap)
                        .ok()
                };
                objects.read(id, read).ok().flatten()
            })
            .map(|bytes| CMap::parse(&bytes, lim))
            .unwrap_or_default();
        if name(b"Subtype") == Some(b"Type0") {
            let identity = matches!(name(b"Encoding"), Some(b"Identity-H" | b"Identity-V"));
            return Decoder {
                width: 2,
                ranged: !identity && !cmap.codespace.is_empty(),
                cmap,
                encoded: None,
            };
        }
        // A subset's name has a tag: ABCDEF+Symbol.
        let base = name(b"BaseFont").map(|n| match n.iter().position(|&c| c == b'+') {
            Some(6) => &n[7..],
            _ => n,
        });
        let own = match base {
            Some(b) if b.starts_with(b"Symbol") => Some(Encoding::Symbol),
            Some(b"ZapfDingbats" | b"Dingbats") => Some(Encoding::ZapfDingbats),
            _ => matches!(name(b"Subtype"), Some(b"Type1" | b"MMType1"))
                .then_some(Encoding::Standard),
        };
        let encoding = font.get(b"Encoding").ok().and_then(|e| direct(objects, e));
        let (base, differences) = match encoding {
            Some(Object::Name(n)) => (Encoding::named(n).or(own), None),
            Some(Object::Dictionary(d)) => {
                let base = d.get(b"BaseEncoding").ok().and_then(|b| b.as_name().ok());
                let differences = d
                    .get(b"Differences")
                    .ok()
                    .and_then(|o| direct(objects, o))
                    .and_then(|o| o.as_array().ok());
                (base.and_then(Encoding::named).or(own), differences)
            }
            _ => (own, None),
        };
        let mut names: [Option<&[u8]>; 256] = [None; 256];
        let mut code = 0;
        for item in differences.into_iter().flatten() {
            match item {
                Object::Integer(c) => code = *c,
                Object::Name(n) => {
                    if let Some(slot) = usize::try_from(code).ok().and_then(|c| names.get_mut(c)) {
                        *slot = Some(n);
                    }
                    code = code.saturating_add(1);
                }
                _ => {}
            }
        }
        let dingbats = own == Some(Encoding::ZapfDingbats);
        let encoded = (0..=255u8)
            .map(|code| match names[code as usize] {
                Some(n) => glyph_text(std::str::from_utf8(n).ok()?, dingbats),
                None => base?.text(code),
            })
            .collect();
        Decoder {
            width: 1,
            ranged: false,
            cmap,
            encoded: Some(encoded),
        }
    }

    /// `s`'s codes as text, pushed onto `out`.
    fn decode(&self, s: &[u8], out: &mut String) {
        let mut at = 0;
        while at < s.len() {
            let rest = &s[at..];
            let len = self.code_len(rest).clamp(1, rest.len());
            let code = rest[..len].iter().fold(0u32, |c, &b| c << 8 | u32::from(b));
            at += len;
            if let Some(text) = self.cmap.get(code) {
                out.push_str(&text);
            } else if let Some(Some(text)) =
                self.encoded.as_ref().and_then(|e| e.get(code as usize))
            {
                out.push_str(text);
            } else {
                out.push(char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER));
            }
        }
    }

    /// How many bytes the code `rest` starts with takes: the codespace
    /// range it's in, or the shortest one's, or the font's width.
    fn code_len(&self, rest: &[u8]) -> usize {
        if !self.ranged {
            return self.width;
        }
        let space = &self.cmap.codespace;
        let within = |(lo, hi): &(Vec<u8>, Vec<u8>)| {
            rest.len() >= lo.len() && (0..lo.len()).all(|i| lo[i] <= rest[i] && rest[i] <= hi[i])
        };
        space
            .iter()
            .find(|r| within(r))
            .or_else(|| space.iter().min_by_key(|(lo, _)| lo.len()))
            .map_or(self.width, |(lo, _)| lo.len())
    }
}

/// A `/ToUnicode` CMap.
#[derive(Debug, Clone, Default)]
struct CMap {
    /// Codespace ranges, each its first and last code, both as long as
    /// the codes in it.
    codespace: Vec<(Vec<u8>, Vec<u8>)>,
    /// Codes mapped one at a time.
    chars: HashMap<u32, String>,
    /// Ranges whose text counts up from their first code's, by first
    /// code: first, last, the first code's text.
    ranges: Vec<(u32, u32, Vec<char>)>,
}

impl CMap {
    /// The mappings of `bytes`, a CMap; read as far as it lexes.
    fn parse(bytes: &[u8], lim: &Limits) -> CMap {
        let mut cmap = CMap::default();
        let depth = lim.content.depth;
        let string = |operand: &[u8]| match lex::object(operand, depth)? {
            Object::String(s, _) => Some(s),
            _ => None,
        };
        let code = |operand: &[u8]| {
            let s = string(operand)?;
            (1..=4)
                .contains(&s.len())
                .then(|| s.iter().fold(0u32, |c, &b| c << 8 | u32::from(b)))
        };
        let _ = lex::steps(bytes, &ResCx::none(), lim, |step| {
            let args: Vec<&[u8]> = step.args().collect();
            match step.tok {
                Tok::Op(b"endcodespacerange") => {
                    for &[lo, hi] in args.as_chunks::<2>().0 {
                        if let (Some(lo), Some(hi)) = (string(lo), string(hi))
                            && lo.len() == hi.len()
                            && (1..=4).contains(&lo.len())
                        {
                            cmap.codespace.push((lo, hi));
                        }
                    }
                }
                Tok::Op(b"endbfchar") => {
                    for &[c, text] in args.as_chunks::<2>().0 {
                        if let (Some(c), Some(text)) = (code(c), destination(text, depth)) {
                            cmap.chars.insert(c, text);
                        }
                    }
                }
                Tok::Op(b"endbfrange") => {
                    for &[lo, hi, text] in args.as_chunks::<3>().0 {
                        let (Some(lo), Some(hi)) = (code(lo), code(hi)) else {
                            continue;
                        };
                        if lo > hi {
                            continue;
                        }
                        match lex::object(text, depth) {
                            Some(Object::String(s, _)) => {
                                cmap.ranges.push((lo, hi, utf16(&s).chars().collect()));
                            }
                            Some(Object::Array(items)) => {
                                let texts = items.iter().filter_map(|i| match i {
                                    Object::String(s, _) => Some(utf16(s)),
                                    _ => None,
                                });
                                for (c, text) in (lo..=hi).zip(texts) {
                                    cmap.chars.insert(c, text);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
            Ok::<_, LexError>(())
        });
        cmap.ranges.sort_by_key(|r| r.0);
        cmap
    }

    /// The text `code` maps to: its own mapping, or the range it's in,
    /// the range's text with its last character counted up.
    fn get(&self, code: u32) -> Option<Cow<'_, str>> {
        if let Some(text) = self.chars.get(&code) {
            return Some(Cow::Borrowed(text));
        }
        let i = self
            .ranges
            .partition_point(|r| r.0 <= code)
            .checked_sub(1)?;
        let (lo, hi, first) = &self.ranges[i];
        if code > *hi {
            return None;
        }
        let (last, rest) = first.split_last()?;
        let last = char::from_u32(u32::from(*last).checked_add(code - lo)?)?;
        Some(Cow::Owned(rest.iter().chain([&last]).collect()))
    }
}

/// A `bfchar` destination's text: a string's UTF-16, or a glyph name's.
fn destination(operand: &[u8], depth: usize) -> Option<String> {
    match lex::object(operand, depth)? {
        Object::String(s, _) => Some(utf16(&s)),
        Object::Name(n) => glyph_text(std::str::from_utf8(&n).ok()?, false),
        _ => None,
    }
}

/// A CMap destination string's text: UTF-16BE, or one byte's code.
fn utf16(s: &[u8]) -> String {
    if let [b] = s {
        return char::from(*b).to_string();
    }
    let units = s.as_chunks::<2>().0.iter().map(|&u| u16::from_be_bytes(u));
    char::decode_utf16(units)
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}
