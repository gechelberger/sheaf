//! The lexical layer: a content stream's tokens, each with where it is
//! and its bytes as written. It knows the syntax and nothing of what an
//! operator does, so an operator it doesn't know is a token like any
//! other, and M11's interpreter reads the same tokens.
//!
//! Tokens end where PDFium's `CPDF_StreamParser` ends them: at white-space
//! and delimiters, a word of digits, signs and points is a number, strings
//! nest, and an inline image, `BI` to `EI`, is one token, its data found
//! by PDFium's `ReadInlineStream` rules ([`lex`]). Where PDFium reads on
//! regardless, this fails: a string, array, dictionary or inline image
//! that doesn't end, or one past [`ContentLimits`], is a [`LexError`]
//! saying where. It never panics, and every token moves it on.

use std::borrow::Cow;
use std::collections::HashMap;
use std::ops::Range;

use lopdf::{Dictionary, Object, StringFormat};

use crate::source::{ContentLimits, Limits};

/// A token of a content stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tok<'a> {
    /// An operator: any word that isn't a number, `true`, `false` or
    /// `null`.
    Op(&'a [u8]),
    /// An operand as written: a number, a name, a string, an array or a
    /// dictionary with all it holds, `true`, `false` or `null`. A closing
    /// delimiter with nothing to close is one too, as PDFium skips it.
    Operand(&'a [u8]),
    /// An inline image, `BI` to `EI`: where its data is.
    InlineImage(Range<usize>),
}

/// Content that can't be lexed: what's wrong, and where it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{kind} at byte {at}")]
pub struct LexError {
    pub at: usize,
    pub kind: LexErrorKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LexErrorKind {
    #[error("a string that doesn't end")]
    String,
    #[error("a hex string that doesn't end")]
    HexString,
    #[error("an array that doesn't end")]
    Array,
    #[error("a dictionary that doesn't end")]
    Dict,
    #[error("an inline image that doesn't end")]
    InlineImage,
    #[error("arrays, dictionaries or marked content nested too deep")]
    Depth,
    #[error("too many operands for one operator")]
    Operands,
    #[error("an inline image too large")]
    InlineImageSize,
    #[error("marked content that doesn't end")]
    Marked,
    #[error("an EMC that ends no marked content")]
    Emc,
}

/// What lexing needs of the resources content is drawn with: how many
/// components each named color space has, which sizes an unfiltered
/// inline image drawn in one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResCx {
    spaces: HashMap<Vec<u8>, u32>,
}

impl ResCx {
    /// No resources: what a `/DA` is lexed with.
    pub fn none() -> ResCx {
        ResCx::default()
    }

    /// Color spaces by name, each with its components.
    pub fn new(spaces: impl IntoIterator<Item = (Vec<u8>, u32)>) -> ResCx {
        ResCx {
            spaces: spaces.into_iter().collect(),
        }
    }
}

/// The tokens of `s`, in order, each with where it is. Between them is
/// only white-space and comments. After an error there's nothing more.
///
/// An inline image's data is found as PDFium's `ReadInlineStream` finds
/// it, by the first of these rules that applies:
/// - unfiltered, its exact size: `/H` rows of `/W` samples of its color
///   space's components, `/BPC` bits each, padded to a byte. With no
///   color space, an image mask's, one bit of one component; a name the
///   resources don't have reads as none, as in PDFium.
/// - `/AHx`, `/A85` and `/RL`, to their end-of-data marker;
/// - `/DCT`, to the JPEG's end of image;
/// - `/L` or `/Length` bytes, where they're given (PDF 2.0).
///
/// Past the data so found, the image ends at the next `EI` operator.
/// When no rule applies, at the first `EI` with white-space before it and
/// white-space, a delimiter or the end after. Only an image's first
/// filter counts, and the abbreviated keys and names read as the full
/// ones.
pub fn lex<'a>(
    s: &'a [u8],
    res: &ResCx,
    lim: &Limits,
) -> impl Iterator<Item = Result<(Range<usize>, Tok<'a>), LexError>> {
    Lexer {
        s,
        at: 0,
        res,
        lim: lim.content,
        operands: 0,
        eoi: None,
        done: false,
    }
}

/// The name an operand is, its `#` escapes decoded; None when it isn't
/// one.
pub fn name(operand: &[u8]) -> Option<Cow<'_, [u8]>> {
    let raw = operand.strip_prefix(b"/")?;
    if !raw.contains(&b'#') {
        return Some(Cow::Borrowed(raw));
    }
    let digit = |c: Option<&u8>| (*c? as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        match (raw[i], digit(raw.get(i + 1)), digit(raw.get(i + 2))) {
            (b'#', Some(high), Some(low)) => {
                out.push(high << 4 | low);
                i += 3;
            }
            (c, ..) => {
                out.push(c);
                i += 1;
            }
        }
    }
    Some(Cow::Owned(out))
}

/// An operand as a PDF object: a name's escapes decoded, a string's
/// escapes or hex digits read, and an array's or a dictionary's elements
/// each read so. None for what isn't an object, such as a stray closing
/// delimiter or a word of signs, and for one that doesn't end or nests
/// more than `depth` deep. A dictionary's entry whose key isn't a name is
/// passed over. It's read in one pass, so a deep operand costs what a
/// flat one of its size does.
pub fn object(operand: &[u8], depth: usize) -> Option<Object> {
    match item(operand, &mut 0, depth, None)? {
        Item::Value(object) => object,
        Item::Close => None,
    }
}

/// What's next in an operand read as an object.
enum Item {
    /// An object, or None for what isn't one.
    Value(Option<Object>),
    /// The delimiter that ends the array or dictionary being read.
    Close,
}

/// The item from `*at` in `s`, `*at` moved past it, inside the array or
/// dictionary that `close` ends, if one's open, `depth` more of which may
/// nest. None at the end, and for a string, array or dictionary that
/// doesn't end, or one nested too deep.
fn item(s: &[u8], at: &mut usize, depth: usize, close: Option<&[u8]>) -> Option<Item> {
    let start = skip_space(s, *at);
    let rest = s.get(start..)?;
    let closes = |delimiter: &[u8]| close == Some(delimiter);
    let (end, value) = match *rest.first()? {
        b'(' => {
            let end = literal_string(s, start).ok()?;
            let string = Object::String(literal(&s[start..end]), StringFormat::Literal);
            (end, Some(string))
        }
        b'<' if rest.starts_with(b"<<") => {
            *at = start + 2;
            return Some(Item::Value(Some(dictionary(s, at, depth.checked_sub(1)?)?)));
        }
        b'<' => {
            let end = hex_string(s, start).ok()?;
            let string = Object::String(hex(&s[start..end]), StringFormat::Hexadecimal);
            (end, Some(string))
        }
        b'[' => {
            *at = start + 1;
            return Some(Item::Value(Some(array(s, at, depth.checked_sub(1)?)?)));
        }
        b']' if closes(b"]") => {
            *at = start + 1;
            return Some(Item::Close);
        }
        b'>' if rest.starts_with(b">>") => {
            *at = start + 2;
            return Some(if closes(b">>") {
                Item::Close
            } else {
                Item::Value(None)
            });
        }
        b')' | b'>' | b']' | b'{' | b'}' => (start + 1, None),
        b'/' => {
            let end = word_end(s, start + 1);
            (end, Some(Object::Name(name(&s[start..end])?.into_owned())))
        }
        _ => {
            let end = word_end(s, start);
            (end, word(&s[start..end]))
        }
    };
    *at = end;
    Some(Item::Value(value))
}

/// The array whose `[` ends at `*at`: its elements read, to its `]`.
fn array(s: &[u8], at: &mut usize, depth: usize) -> Option<Object> {
    let mut elements = Vec::new();
    loop {
        match item(s, at, depth, Some(b"]")) {
            Some(Item::Value(element)) => elements.extend(element),
            Some(Item::Close) => return Some(Object::Array(elements)),
            None => return None,
        }
    }
}

/// The dictionary whose `<<` ends at `*at`: its entries read, to its
/// `>>`.
fn dictionary(s: &[u8], at: &mut usize, depth: usize) -> Option<Object> {
    let mut dict = Dictionary::new();
    let next = |at: &mut usize| match item(s, at, depth, Some(b">>")) {
        Some(Item::Value(value)) => Some(Ok(value)),
        Some(Item::Close) => Some(Err(())),
        None => None,
    };
    loop {
        let Ok(key) = next(at)? else {
            return Some(Object::Dictionary(dict));
        };
        let Some(Object::Name(key)) = key else {
            continue;
        };
        match next(at)? {
            Ok(Some(value)) => dict.set(key, value),
            Ok(None) => {}
            Err(()) => return Some(Object::Dictionary(dict)),
        }
    }
}

/// A word as an object: a number, `true`, `false` or `null`.
fn word(word: &[u8]) -> Option<Object> {
    Some(match word {
        b"true" => Object::Boolean(true),
        b"false" => Object::Boolean(false),
        b"null" => Object::Null,
        n if !is_number(n) => return None,
        n => {
            let n = std::str::from_utf8(n).ok()?;
            if n.contains('.') {
                Object::Real(n.parse().ok()?)
            } else {
                Object::Integer(n.parse().ok()?)
            }
        }
    })
}

/// A literal string's bytes, its escapes read: `(…)` as lexed.
fn literal(string: &[u8]) -> Vec<u8> {
    let inner = &string[1..string.len().saturating_sub(1).max(1)];
    let mut out = Vec::with_capacity(inner.len());
    let mut i = 0;
    while let Some(&c) = inner.get(i) {
        i += 1;
        match c {
            b'\\' => {
                let Some(&e) = inner.get(i) else { break };
                i += 1;
                match e {
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'b' => out.push(0x08),
                    b'f' => out.push(0x0c),
                    b'0'..=b'7' => {
                        let mut v = u32::from(e - b'0');
                        for _ in 0..2 {
                            match inner.get(i) {
                                Some(&d @ b'0'..=b'7') => {
                                    v = v * 8 + u32::from(d - b'0');
                                    i += 1;
                                }
                                _ => break,
                            }
                        }
                        out.push(v as u8);
                    }
                    // A line continued: the end of line is no part of it.
                    b'\r' => i += usize::from(inner.get(i) == Some(&b'\n')),
                    b'\n' => {}
                    e => out.push(e),
                }
            }
            // Any end of line reads as a line feed.
            b'\r' => {
                i += usize::from(inner.get(i) == Some(&b'\n'));
                out.push(b'\n');
            }
            c => out.push(c),
        }
    }
    out
}

/// A hex string's bytes: `<…>` as lexed, what isn't a hex digit passed
/// over, an odd last digit followed by 0.
fn hex(string: &[u8]) -> Vec<u8> {
    let digits: Vec<u8> = string
        .iter()
        .filter_map(|&c| (c as char).to_digit(16).map(|d| d as u8))
        .collect();
    digits
        .chunks(2)
        .map(|pair| pair[0] << 4 | pair.get(1).copied().unwrap_or(0))
        .collect()
}

/// What content does, an operator at a time: an operator or an inline
/// image, and the operands before it.
pub(crate) struct Step<'a, 'o> {
    s: &'a [u8],
    /// The operator, or the inline image.
    pub tok: Tok<'a>,
    /// Where `tok` is.
    pub at: Range<usize>,
    /// Where each operand is.
    pub operands: &'o [Range<usize>],
}

impl<'a> Step<'a, '_> {
    /// Where the step is, from its first operand.
    pub fn range(&self) -> Range<usize> {
        self.operands.first().map_or(self.at.start, |r| r.start)..self.at.end
    }

    /// Operand `i` from the last, as PDFium takes them: 0 is the last.
    /// Operands before those an operator takes are passed over.
    pub fn arg(&self, i: usize) -> Option<&'a [u8]> {
        let r = self.operands.get(self.operands.len().checked_sub(i + 1)?)?;
        Some(&self.s[r.clone()])
    }

    /// The name operand `i` from the last is, decoded.
    pub fn name(&self, i: usize) -> Option<Cow<'a, [u8]>> {
        name(self.arg(i)?)
    }

    /// Every operand, in order.
    pub fn args(&self) -> impl Iterator<Item = &'a [u8]> + '_ {
        self.operands.iter().map(|r| &self.s[r.clone()])
    }
}

/// `s`'s steps ([`lex`]), each handed to `step`, until the end or an
/// error, `step`'s or the lexer's. Operands no operator takes, at the end,
/// are no step.
pub(crate) fn steps<'a, E: From<LexError>>(
    s: &'a [u8],
    res: &ResCx,
    lim: &Limits,
    mut step: impl FnMut(Step<'a, '_>) -> Result<(), E>,
) -> Result<(), E> {
    let mut operands = Vec::new();
    for token in lex(s, res, lim) {
        let (at, tok) = token?;
        if let Tok::Operand(_) = tok {
            operands.push(at);
            continue;
        }
        step(Step {
            s,
            tok,
            at,
            operands: &operands,
        })?;
        operands.clear();
    }
    Ok(())
}

struct Lexer<'a, 'r> {
    s: &'a [u8],
    at: usize,
    res: &'r ResCx,
    lim: ContentLimits,
    /// Operands since the last operator.
    operands: usize,
    /// The last search for a JPEG's end of image: from where, and where
    /// it was. Content of many JPEG images that don't end is searched
    /// once, not once per image.
    eoi: Option<(usize, Option<usize>)>,
    done: bool,
}

impl<'a> Iterator for Lexer<'a, '_> {
    type Item = Result<(Range<usize>, Tok<'a>), LexError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let token = self.token();
        self.done = !matches!(token, Some(Ok(_)));
        token
    }
}

impl<'a> Lexer<'a, '_> {
    fn token(&mut self) -> Option<Result<(Range<usize>, Tok<'a>), LexError>> {
        let s = self.s;
        let (r, kind) = match next_raw(s, self.at, self.lim.depth)? {
            Ok(raw) => raw,
            Err(e) => return Some(Err(e)),
        };
        self.at = r.end;
        let tok = match kind {
            Raw::Op if &s[r.clone()] == b"BI" => {
                self.operands = 0;
                return Some(self.inline_image(r.start).map(|(end, data)| {
                    self.at = end;
                    (r.start..end, Tok::InlineImage(data))
                }));
            }
            Raw::Op => {
                self.operands = 0;
                Tok::Op(&s[r.clone()])
            }
            Raw::Operand => {
                self.operands += 1;
                if self.operands > self.lim.operands {
                    return Some(Err(LexError {
                        at: r.start,
                        kind: LexErrorKind::Operands,
                    }));
                }
                Tok::Operand(&s[r.clone()])
            }
        };
        Some(Ok((r, tok)))
    }

    /// The inline image whose `BI` is at `bi`: where it ends, past its
    /// `EI`, and where its data is.
    fn inline_image(&mut self, bi: usize) -> Result<(usize, Range<usize>), LexError> {
        let s = self.s;
        let unterminated = LexError {
            at: bi,
            kind: LexErrorKind::InlineImage,
        };
        let mut dict = ImageDict::default();
        let at = image_dict(s, bi, self.lim.depth, |key, value| dict.set(key, value))?;
        // One white-space byte after `ID`, then the data.
        let start = at + usize::from(s.get(at).is_some_and(|&c| is_space(c)));
        let counted = match self.first_filter(&dict) {
            None => self.unfiltered(&dict).map(|n| start.saturating_add(n)),
            Some(Filter::Hex) => Some(find(s, start, b">").ok_or(unterminated)? + 1),
            Some(Filter::A85) => Some(find(s, start, b"~>").ok_or(unterminated)? + 2),
            Some(Filter::RunLength) => Some(run_length_end(s, start).ok_or(unterminated)?),
            Some(Filter::Dct) => self.eoi(start).map(|eoi| eoi + 2),
            Some(Filter::Other) => None,
        }
        .or_else(|| {
            let length = dict.l.and_then(|l| int(&s[l]))?;
            start
                .checked_add(usize::try_from(length).ok()?)
                .filter(|&end| end <= s.len())
        });
        let (data, end) = match counted {
            Some(end) if end <= s.len() => (start..end, self.ei_from(end).ok_or(unterminated)?),
            Some(_) => return Err(unterminated),
            None => {
                let ei = ei_scan(s, start).ok_or(unterminated)?;
                (start..(ei - 1).max(start), ei + 2)
            }
        };
        if data.len() > self.lim.inline_image {
            return Err(LexError {
                at: bi,
                kind: LexErrorKind::InlineImageSize,
            });
        }
        Ok((end, data))
    }

    /// An image's first filter, by what finds its data's end; None when
    /// it has none.
    fn first_filter(&self, dict: &ImageDict) -> Option<Filter> {
        let f = &self.s[dict.f.clone()?];
        let first = match f.first() {
            Some(b'[') => *elements(f, self.lim.depth).first()?,
            _ => f,
        };
        // A filter that isn't a name is none, as PDFium reads it.
        Some(match &*name(first)? {
            b"AHx" | b"ASCIIHexDecode" => Filter::Hex,
            b"A85" | b"ASCII85Decode" => Filter::A85,
            b"RL" | b"RunLengthDecode" => Filter::RunLength,
            b"DCT" | b"DCTDecode" => Filter::Dct,
            _ => Filter::Other,
        })
    }

    /// The size of an unfiltered image's data, as PDFium counts it; None
    /// when it counts none.
    fn unfiltered(&self, dict: &ImageDict) -> Option<usize> {
        let s = self.s;
        let value = |r: &Option<Range<usize>>| int(&s[r.clone()?]);
        let (components, bpc) = match &dict.cs {
            None => (1, 1),
            Some(cs) => match self.components(&s[cs.clone()]) {
                Some(n) => (n, value(&dict.bpc)?),
                None => (1, 1),
            },
        };
        let pitch = value(&dict.w)?
            .checked_mul(bpc)?
            .checked_mul(components)?
            .div_ceil(8);
        let size = pitch.checked_mul(value(&dict.h)?)?;
        usize::try_from(size).ok().filter(|&n| n > 0)
    }

    /// The components of an inline image's color space, as written; None
    /// for a name the resources don't have, which PDFium reads as no
    /// color space. What it can't make out has 3, as in PDFium.
    fn components(&self, cs: &[u8]) -> Option<u64> {
        if let Some(n) = name(cs) {
            return match &*n {
                b"G" | b"DeviceGray" => Some(1),
                b"RGB" | b"DeviceRGB" => Some(3),
                b"CMYK" | b"DeviceCMYK" => Some(4),
                n => self.res.spaces.get(n).map(|&c| c.into()),
            };
        }
        if cs.first() != Some(&b'[') {
            return Some(3);
        }
        let parts = elements(cs, self.lim.depth);
        let family = parts.first().and_then(|f| name(f));
        Some(match family.as_deref() {
            Some(b"DeviceN") => parts
                .get(1)
                .map(|names| elements(names, self.lim.depth).len() as u64)
                .filter(|&n| n > 0)
                .unwrap_or(3),
            Some(family) => family_components(family).into(),
            None => 3,
        })
    }

    /// The next `EI` operator from `at`: where it ends.
    fn ei_from(&self, mut at: usize) -> Option<usize> {
        loop {
            let (r, kind) = next_raw(self.s, at, self.lim.depth)?.ok()?;
            if matches!(kind, Raw::Op) && &self.s[r.clone()] == b"EI" {
                return Some(r.end);
            }
            at = r.end;
        }
    }

    /// Where the next JPEG end of image (`FF D9`) from `at` is.
    fn eoi(&mut self, at: usize) -> Option<usize> {
        if let Some((from, found)) = self.eoi
            && at >= from
            && found.is_none_or(|f| f >= at)
        {
            return found;
        }
        let found = find(self.s, at, b"\xff\xd9");
        self.eoi = Some((at, found));
        found
    }
}

/// The components a color space family has with no more said: what a
/// name in the resources, or an array's first element, gives. PDFium
/// gives what it can't load 3.
pub(crate) fn family_components(family: &[u8]) -> u32 {
    match family {
        b"G" | b"DeviceGray" | b"CalGray" | b"I" | b"Indexed" | b"Separation" | b"Pattern" => 1,
        b"CMYK" | b"DeviceCMYK" => 4,
        _ => 3,
    }
}

/// What an inline image's dictionary says that finds its data's end:
/// where each value is.
#[derive(Default)]
struct ImageDict {
    w: Option<Range<usize>>,
    h: Option<Range<usize>>,
    bpc: Option<Range<usize>>,
    cs: Option<Range<usize>>,
    f: Option<Range<usize>>,
    l: Option<Range<usize>>,
}

impl ImageDict {
    fn set(&mut self, key: &[u8], value: Range<usize>) {
        let slot = match name(key).as_deref() {
            Some(b"W" | b"Width") => &mut self.w,
            Some(b"H" | b"Height") => &mut self.h,
            Some(b"BPC" | b"BitsPerComponent") => &mut self.bpc,
            Some(b"CS" | b"ColorSpace") => &mut self.cs,
            Some(b"F" | b"Filter") => &mut self.f,
            Some(b"L" | b"Length") => &mut self.l,
            _ => return,
        };
        *slot = Some(value);
    }
}

/// The dictionary of the inline image whose `BI` is at `bi`, to its `ID`:
/// `entry` is given each key, as written, and where its value is. What
/// isn't a name where a key should be is passed over. Returns where `ID`
/// ends.
fn image_dict(
    s: &[u8],
    bi: usize,
    depth: usize,
    mut entry: impl FnMut(&[u8], Range<usize>),
) -> Result<usize, LexError> {
    let unterminated = LexError {
        at: bi,
        kind: LexErrorKind::InlineImage,
    };
    let mut key = None;
    let mut at = bi + 2;
    loop {
        let (r, kind) = next_raw(s, at, depth).ok_or(unterminated)??;
        at = r.end;
        if matches!(kind, Raw::Op) && &s[r.clone()] == b"ID" {
            return Ok(at);
        }
        match key.take() {
            Some(key) => entry(&s[key], r),
            None if s[r.start] == b'/' => key = Some(r),
            None => {}
        }
    }
}

/// The color space of the inline image `image` (its token, `BI` to `EI`,
/// in `s`), as written.
pub(crate) fn image_color_space(s: &[u8], image: Range<usize>, depth: usize) -> Option<&[u8]> {
    let mut cs = None;
    image_dict(&s[..image.end], image.start, depth, |key, value| {
        if matches!(name(key).as_deref(), Some(b"CS" | b"ColorSpace")) {
            cs = Some(value);
        }
    })
    .ok()?;
    cs.map(|r| &s[r])
}

enum Filter {
    Hex,
    A85,
    RunLength,
    Dct,
    Other,
}

/// What a raw token is, before an inline image is made one.
enum Raw {
    Op,
    Operand,
}

/// The token from `from` on, past white-space and comments; None at the
/// end.
fn next_raw(s: &[u8], from: usize, depth: usize) -> Option<Result<(Range<usize>, Raw), LexError>> {
    let start = skip_space(s, from);
    let end = match *s.get(start)? {
        b'(' => literal_string(s, start),
        b'[' => nested(s, start, depth),
        b'<' if s.get(start + 1) == Some(&b'<') => nested(s, start, depth),
        b'<' => hex_string(s, start),
        b'>' if s.get(start + 1) == Some(&b'>') => Ok(start + 2),
        b')' | b'>' | b']' | b'{' | b'}' => Ok(start + 1),
        b'/' => Ok(word_end(s, start + 1)),
        _ => {
            let end = word_end(s, start);
            let word = &s[start..end];
            let kind = if is_number(word) || matches!(word, b"true" | b"false" | b"null") {
                Raw::Operand
            } else {
                Raw::Op
            };
            return Some(Ok((start..end, kind)));
        }
    };
    Some(end.map(|end| (start..end, Raw::Operand)))
}

/// An array or dictionary from `start`, with all it holds: where it
/// ends. A closing delimiter that doesn't close what's open is passed
/// over.
fn nested(s: &[u8], start: usize, cap: usize) -> Result<usize, LexError> {
    // Where each array or dictionary still open starts, and whether it's
    // a dictionary.
    let mut open: Vec<(usize, bool)> = Vec::new();
    let mut at = start;
    loop {
        let Some(&c) = s.get(at) else {
            let (at, dict) = open.last().copied().unwrap_or((start, false));
            let kind = if dict {
                LexErrorKind::Dict
            } else {
                LexErrorKind::Array
            };
            return Err(LexError { at, kind });
        };
        let pair = |second: u8| c == second && s.get(at + 1) == Some(&second);
        let closes = match c {
            b'[' | b'<' if c == b'[' || pair(b'<') => {
                if open.len() >= cap {
                    return Err(LexError {
                        at,
                        kind: LexErrorKind::Depth,
                    });
                }
                open.push((at, c == b'<'));
                at += 1 + usize::from(c == b'<');
                continue;
            }
            b']' => Some(false),
            b'>' if pair(b'>') => Some(true),
            _ => None,
        };
        at = match (c, closes) {
            (_, Some(dict)) => {
                let end = at + 1 + usize::from(dict);
                if open.last().is_some_and(|&(_, d)| d == dict) {
                    open.pop();
                    if open.is_empty() {
                        return Ok(end);
                    }
                }
                end
            }
            (b'(', _) => literal_string(s, at)?,
            (b'<', _) => hex_string(s, at)?,
            (b'%', _) => line_end(s, at),
            _ => at + 1,
        };
    }
}

/// A literal string from its `(`: where it ends, after the `)` that
/// balances it.
fn literal_string(s: &[u8], start: usize) -> Result<usize, LexError> {
    let mut depth = 0usize;
    let mut at = start;
    while let Some(&c) = s.get(at) {
        match c {
            b'\\' => at += 1,
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(at + 1);
                }
            }
            _ => {}
        }
        at += 1;
    }
    Err(LexError {
        at: start,
        kind: LexErrorKind::String,
    })
}

/// A hex string from its `<`: where it ends, after its `>`.
fn hex_string(s: &[u8], start: usize) -> Result<usize, LexError> {
    find(s, start + 1, b">").map(|end| end + 1).ok_or(LexError {
        at: start,
        kind: LexErrorKind::HexString,
    })
}

/// The elements of an array token, as written.
fn elements(array: &[u8], depth: usize) -> Vec<&[u8]> {
    let inner = array.strip_prefix(b"[").unwrap_or(array);
    items(inner.strip_suffix(b"]").unwrap_or(inner), depth)
}

/// The tokens of what's inside an array or a dictionary, as written.
fn items(inner: &[u8], depth: usize) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(Ok((r, _))) = next_raw(inner, at, depth) {
        at = r.end;
        out.push(&inner[r]);
    }
    out
}

/// Where RunLength data from `at` ends, past its end-of-data byte (128).
fn run_length_end(s: &[u8], mut at: usize) -> Option<usize> {
    while let Some(&n) = s.get(at) {
        at += match n {
            0..=127 => usize::from(n) + 2,
            128 => return Some(at + 1),
            _ => 2,
        };
    }
    None
}

/// The first `EI` from `start` with white-space before it and white-space,
/// a delimiter or the end after it: where it is.
fn ei_scan(s: &[u8], start: usize) -> Option<usize> {
    let mut at = start;
    loop {
        let i = find(s, at, b"EI")?;
        let before = i > 0 && is_space(s[i - 1]);
        let after = s.get(i + 2).is_none_or(|&c| is_space(c) || is_delimiter(c));
        if before && after {
            return Some(i);
        }
        at = i + 1;
    }
}

fn skip_space(s: &[u8], mut at: usize) -> usize {
    while let Some(&c) = s.get(at) {
        if c == b'%' {
            at = line_end(s, at);
        } else if is_space(c) {
            at += 1;
        } else {
            break;
        }
    }
    at
}

/// Where the line from `at` ends: its end-of-line byte, or the end.
fn line_end(s: &[u8], at: usize) -> usize {
    s[at..]
        .iter()
        .position(|&c| c == b'\r' || c == b'\n')
        .map_or(s.len(), |n| at + n)
}

fn word_end(s: &[u8], at: usize) -> usize {
    s[at..]
        .iter()
        .position(|&c| is_space(c) || is_delimiter(c))
        .map_or(s.len(), |n| at + n)
}

/// Where `needle` first is in `s` from `at`.
fn find(s: &[u8], at: usize, needle: &[u8]) -> Option<usize> {
    s.get(at..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|n| at + n)
}

/// A non-negative number, as PDFium reads an integer from one: a real's
/// whole part.
fn int(number: &[u8]) -> Option<u64> {
    if !is_number(number) {
        return None;
    }
    let value: f64 = std::str::from_utf8(number).ok()?.parse().ok()?;
    (0.0..4_294_967_296.0)
        .contains(&value)
        .then_some(value as u64)
}

/// Whether a word is a number, as PDFium tells one: digits, signs and
/// points only.
fn is_number(word: &[u8]) -> bool {
    word.iter()
        .all(|c| matches!(c, b'0'..=b'9' | b'+' | b'-' | b'.'))
}

pub(crate) fn is_space(c: u8) -> bool {
    matches!(c, b'\0' | b'\t' | b'\n' | b'\x0c' | b'\r' | b' ')
}

fn is_delimiter(c: u8) -> bool {
    matches!(
        c,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}
