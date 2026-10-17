//! Splicing: a page's streams with ranges taken out and every other byte
//! as it was, never decoded into operations and written again.
//!
//! A range is taken out only when that can't change what's drawn after
//! it (§13), lexically:
//! - it sets no graphics state outside a `q`…`Q` of its own: no `cm`,
//!   color, line style, `gs`, clip or text state (`Tc Tw Tz TL Tf Tr Ts
//!   "`), which would last past it;
//! - outside a text object, its `q`s and `Q`s and its `BT`s and `ET`s
//!   balance on their own;
//! - inside one, it stays inside, its `q`s and `Q`s balance, and it moves
//!   no line (`Td TD Tm T* ' "`), which `q` doesn't save. Text it shows
//!   moved the text on, so text shown after it, before a line is moved or
//!   the object ends, would move back: that refuses it too;
//! - it starts and ends outside a path being made, which what's after it
//!   would paint;
//! - marked content draws nothing, but for optional content (`/OC`),
//!   which shows or hides what it marks: so marked content a range opens,
//!   none of it optional, may be closed by a later one, as long as no
//!   marked content is kept between them, and a range closes only what
//!   it or an earlier one opened. The start and the end of an artifact may
//!   go while what's between stays.
//!
//! Otherwise the splice is refused ([`SpliceError::Unsafe`]), as it is
//! when a range starts or ends inside a token or a comment, or between an
//! operator and its operands, which the next operator would take. Removing
//! a running head drawn in the middle of a text object can't shift the
//! body text.

use std::borrow::Cow;
use std::ops::Range;

use super::lex::{self, LexError, ResCx, Tok};
use super::page::{Concat, SpanAcross};
use crate::source::Limits;

/// Why a splice was refused.
#[derive(Debug, thiserror::Error)]
pub enum SpliceError {
    #[error(transparent)]
    Lex(#[from] LexError),
    #[error("{0} is outside the content")]
    OutOfRange(SpanAcross),
    #[error("{0} overlaps another range taken out")]
    Overlap(SpanAcross),
    #[error("taking out {0} would change what's drawn after it")]
    Unsafe(SpanAcross),
}

/// `content`'s streams with `drop` taken out, lexed with `res`: a stream
/// nothing is taken from is borrowed as it is. Where what's kept either
/// side of a range taken out would join two tokens, a space keeps them
/// apart.
pub fn splice<'a>(
    content: &'a Concat,
    res: &ResCx,
    lim: &Limits,
    drop: &[SpanAcross],
) -> Result<Vec<Cow<'a, [u8]>>, SpliceError> {
    let mut cuts = drop
        .iter()
        .map(|&span| {
            let range = content.offsets(span).ok_or(SpliceError::OutOfRange(span))?;
            Ok(Cut { range, span })
        })
        .collect::<Result<Vec<_>, SpliceError>>()?;
    cuts.retain(|c| !c.range.is_empty());
    cuts.sort_by_key(|c| c.range.start);
    if let Some(pair) = cuts
        .windows(2)
        .find(|pair| pair[1].range.start < pair[0].range.end)
    {
        return Err(SpliceError::Overlap(pair[1].span));
    }
    check(content, res, lim, &cuts)?;
    Ok((0..content.stream_count())
        .map(|i| cut(content, i, &cuts))
        .collect())
}

/// A range to take out, in the joined bytes.
struct Cut {
    range: Range<usize>,
    span: SpanAcross,
}

/// A running count of opens and closes: where it is, and the lowest it's
/// been.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Count {
    net: i32,
    min: i32,
}

impl Count {
    fn add(&mut self, by: i32) {
        self.net = self.net.saturating_add(by);
        self.min = self.min.min(self.net);
    }

    fn balanced(self) -> bool {
        self == Count::default()
    }
}

/// What a range taken out holds.
#[derive(Default)]
struct Tally {
    /// Whether it starts inside a text object.
    in_text: bool,
    q: Count,
    text: Count,
    /// Whether it begins or ends a text object.
    text_objects: bool,
    marked: Count,
    /// Whether it opens optional content.
    optional: bool,
    /// Whether it sets graphics state outside a `q`…`Q` of its own.
    sets_state: bool,
    /// Whether it moves a line.
    moves: bool,
    /// Whether it shows text.
    shows: bool,
}

impl Tally {
    /// `op`, and `tag` when it opens marked content.
    fn take(&mut self, op: &[u8], tag: Option<&[u8]>) {
        self.sets_state |= self.q.net == 0 && sets_state(op);
        self.optional |= tag.and_then(lex::name).as_deref() == Some(b"OC");
        match op {
            b"q" => self.q.add(1),
            b"Q" => self.q.add(-1),
            b"BT" => {
                self.text.add(1);
                self.text_objects = true;
            }
            b"ET" => {
                self.text.add(-1);
                self.text_objects = true;
            }
            b"BMC" | b"BDC" => self.marked.add(1),
            b"EMC" => self.marked.add(-1),
            _ => {}
        }
        self.moves |= moves_text(op);
        self.shows |= matches!(op, b"Tj" | b"TJ" | b"'" | b"\"");
    }

    /// Whether taking it out leaves what's drawn after it as it was, but
    /// for text shown after it in its text object, and the marked content
    /// it opens or closes.
    fn safe(&self) -> bool {
        // Optional content left open would show or hide what's kept.
        let hides = self.optional && self.marked.net > 0;
        let kept = self.q.balanced() && !self.sets_state && !hides;
        if self.in_text {
            kept && !self.text_objects && !self.moves
        } else {
            kept && self.text.balanced()
        }
    }
}

/// Whether `op` sets graphics state, which lasts until a `Q` restores it:
/// the CTM, a color or color space, line style, rendering intent,
/// flatness, a `gs`, a clip, or text state.
fn sets_state(op: &[u8]) -> bool {
    matches!(
        op,
        b"cm"
            | b"CS"
            | b"cs"
            | b"SC"
            | b"SCN"
            | b"sc"
            | b"scn"
            | b"G"
            | b"g"
            | b"RG"
            | b"rg"
            | b"K"
            | b"k"
            | b"w"
            | b"J"
            | b"j"
            | b"M"
            | b"d"
            | b"ri"
            | b"i"
            | b"gs"
            | b"W"
            | b"W*"
            | b"Tc"
            | b"Tw"
            | b"Tz"
            | b"TL"
            | b"Tf"
            | b"Tr"
            | b"Ts"
            | b"\""
    )
}

/// Whether `op` moves to a line: sets the text and line matrices, which
/// a text object starts afresh and `q` doesn't save.
fn moves_text(op: &[u8]) -> bool {
    matches!(op, b"Td" | b"TD" | b"Tm" | b"T*" | b"'" | b"\"")
}

/// Whether `op` starts or adds to a path, which the next painting
/// operator paints; and whether it ends one.
fn makes_path(op: &[u8]) -> bool {
    matches!(op, b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re")
}

fn ends_path(op: &[u8]) -> bool {
    matches!(
        op,
        b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n"
    )
}

/// Whether `cuts` may be taken out of `content`: each starts and ends
/// between steps, outside a comment and a path, and is safe (the module's
/// rules).
fn check(content: &Concat, res: &ResCx, lim: &Limits, cuts: &[Cut]) -> Result<(), SpliceError> {
    let s = content.bytes();
    let mut in_text = false;
    let mut in_path = false;
    // The cut being lexed, and what it holds.
    let mut open: Option<(usize, Tally)> = None;
    // The next cut to start.
    let mut next = 0;
    // A cut that showed text in a text object that hasn't moved a line
    // since.
    let mut shown: Option<SpanAcross> = None;
    // Marked content cuts opened that no cut has closed yet, and the first
    // cut that left some open.
    let mut pending = 0;
    let mut opener = 0;
    // Where the last token ended, and whether it was an operand: a step
    // its operator hasn't ended; and the last two operands, where a tag
    // is.
    let mut after = 0;
    let mut operand = false;
    let mut operands: [Option<&[u8]>; 2] = [None, None];
    let unsafe_cut = |i: usize| SpliceError::Unsafe(cuts[i].span);
    let mut tokens = content.lex(res, lim);
    loop {
        let token = tokens.next().transpose()?;
        let start = token.as_ref().map_or(s.len(), |(r, _)| r.start);
        // The cuts that end or start before this token, in the white-space
        // and comments since the last.
        let between = |at: usize| !in_comment(&s[after..at]);
        if let Some((i, tally)) = open.take_if(|(i, _)| cuts[*i].range.end <= start) {
            let closes_kept = pending + tally.marked.min < 0;
            if operand || in_path || closes_kept || !between(cuts[i].range.end) || !tally.safe() {
                return Err(unsafe_cut(i));
            }
            if pending == 0 && tally.marked.net > 0 {
                opener = i;
            }
            pending += tally.marked.net;
            if tally.in_text && tally.shows {
                shown = Some(cuts[i].span);
            }
        }
        while open.is_none() && next < cuts.len() && cuts[next].range.start <= start {
            let cut = &cuts[next];
            if operand || in_path || !between(cut.range.start) {
                return Err(unsafe_cut(next));
            }
            if cut.range.end <= start && !between(cut.range.end) {
                return Err(unsafe_cut(next));
            }
            if cut.range.end > start {
                open = Some((
                    next,
                    Tally {
                        in_text,
                        ..Tally::default()
                    },
                ));
            }
            next += 1;
        }
        let Some((r, tok)) = token else {
            return match pending {
                0 => Ok(()),
                _ => Err(unsafe_cut(opener)),
            };
        };
        // A token across either end of a cut.
        if let Some((i, _)) = open
            && r.end > cuts[i].range.end
        {
            return Err(unsafe_cut(i));
        }
        if next < cuts.len() && cuts[next].range.start < r.end {
            return Err(unsafe_cut(next));
        }
        after = r.end;
        operand = matches!(tok, Tok::Operand(_));
        let op = match tok {
            Tok::Op(op) => op,
            Tok::Operand(o) => {
                operands = [operands[1], Some(o)];
                continue;
            }
            Tok::InlineImage(_) => {
                operands = [None, None];
                continue;
            }
        };
        let tag = match op {
            b"BMC" => operands[1],
            b"BDC" => operands[0],
            _ => None,
        };
        operands = [None, None];
        match &mut open {
            Some((_, tally)) => tally.take(op, tag),
            None => {
                if pending > 0 && matches!(op, b"BMC" | b"BDC" | b"EMC") {
                    return Err(unsafe_cut(opener));
                }
                if let Some(span) = shown
                    && matches!(op, b"Tj" | b"TJ")
                {
                    return Err(SpliceError::Unsafe(span));
                }
                if moves_text(op) || matches!(op, b"BT" | b"ET") {
                    shown = None;
                }
            }
        }
        match op {
            b"BT" => in_text = true,
            b"ET" => in_text = false,
            op if makes_path(op) => in_path = true,
            op if ends_path(op) => in_path = false,
            _ => {}
        }
    }
}

/// Whether white-space and comments `gap` end inside a comment.
fn in_comment(gap: &[u8]) -> bool {
    gap.iter().fold(false, |comment, &c| match c {
        b'%' => true,
        b'\r' | b'\n' => false,
        _ => comment,
    })
}

/// Stream `i` of `content` with `cuts` taken out. Whether a cut needs a
/// space in its place is decided by the bytes kept either side of it, as
/// a neighbouring cut may take the bytes next to it too.
fn cut<'a>(content: &'a Concat, i: usize, cuts: &[Cut]) -> Cow<'a, [u8]> {
    let s = content.bytes();
    let stream = content.range(i);
    let mut out: Option<Vec<u8>> = None;
    let mut at = stream.start;
    let mut cuts = cuts
        .iter()
        .filter(|c| c.range.start < stream.end && c.range.end > stream.start)
        .peekable();
    while let Some(cut) = cuts.next() {
        let out = out.get_or_insert_with(|| Vec::with_capacity(stream.len()));
        out.extend_from_slice(&s[at..cut.range.start.max(stream.start)]);
        at = cut.range.end.min(stream.end);
        // The next byte is taken too: the cut after it decides.
        if cuts.peek().is_some_and(|c| c.range.start == at) {
            continue;
        }
        if at < stream.end
            && out.last().is_some_and(|&c| !lex::is_space(c))
            && !lex::is_space(s[at])
        {
            out.push(b' ');
        }
    }
    match out {
        None => Cow::Borrowed(&s[stream]),
        Some(mut out) => {
            out.extend_from_slice(&s[at..stream.end]);
            Cow::Owned(out)
        }
    }
}
