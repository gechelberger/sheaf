//! The resource names content uses: what a page's `/Resources` must keep
//! for it, by category, for pruning what a removal leaves unused.

use std::collections::BTreeSet;

use super::lex::{self, LexError, ResCx, Tok};
use super::page::{Concat, SpanAcross};
use crate::source::Limits;

/// A resource category: a key of `/Resources`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ResKind {
    ColorSpace,
    ExtGState,
    Font,
    Pattern,
    Properties,
    Shading,
    XObject,
}

impl ResKind {
    pub const ALL: [ResKind; 7] = [
        ResKind::ColorSpace,
        ResKind::ExtGState,
        ResKind::Font,
        ResKind::Pattern,
        ResKind::Properties,
        ResKind::Shading,
        ResKind::XObject,
    ];

    /// Its key in `/Resources`.
    pub fn key(self) -> &'static [u8] {
        match self {
            ResKind::ColorSpace => b"ColorSpace",
            ResKind::ExtGState => b"ExtGState",
            ResKind::Font => b"Font",
            ResKind::Pattern => b"Pattern",
            ResKind::Properties => b"Properties",
            ResKind::Shading => b"Shading",
            ResKind::XObject => b"XObject",
        }
    }
}

/// Resource names, each in its category, decoded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NameSet {
    names: BTreeSet<(ResKind, Vec<u8>)>,
}

impl NameSet {
    pub fn contains(&self, kind: ResKind, name: &[u8]) -> bool {
        self.names.contains(&(kind, name.to_vec()))
    }

    /// The names of `kind`, in order.
    pub fn of(&self, kind: ResKind) -> impl Iterator<Item = &[u8]> {
        self.names
            .range((kind, Vec::new())..)
            .take_while(move |(k, _)| *k == kind)
            .map(|(_, name)| name.as_slice())
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    fn insert(&mut self, kind: ResKind, name: Option<impl Into<Vec<u8>>>) {
        if let Some(name) = name {
            self.names.insert((kind, name.into()));
        }
    }
}

/// The resource names `content` uses, lexed with `res`, but for those
/// used only by steps inside `except` (ranges about to be taken out):
/// - `Tf`'s font, `Do`'s XObject, `gs`'s ExtGState and `sh`'s shading;
/// - `cs` and `CS`'s color space, and an inline image's, but for the
///   device spaces and `/Pattern`, which aren't resources;
/// - `scn` and `SCN`'s pattern, when the last operand is a name;
/// - `BDC` and `DP`'s property list, when it's named.
pub fn names_used(
    content: &Concat,
    res: &ResCx,
    lim: &Limits,
    except: &[SpanAcross],
) -> Result<NameSet, LexError> {
    let except: Vec<_> = except.iter().filter_map(|&s| content.offsets(s)).collect();
    let s = content.bytes();
    let mut names = NameSet::default();
    lex::steps(s, res, lim, |step| {
        let range = step.range();
        if except
            .iter()
            .any(|e| e.start <= range.start && range.end <= e.end)
        {
            return Ok::<_, LexError>(());
        }
        let name = |i| step.name(i);
        match step.tok {
            Tok::Op(b"Tf") => names.insert(ResKind::Font, name(1)),
            Tok::Op(b"Do") => names.insert(ResKind::XObject, name(0)),
            Tok::Op(b"gs") => names.insert(ResKind::ExtGState, name(0)),
            Tok::Op(b"sh") => names.insert(ResKind::Shading, name(0)),
            Tok::Op(b"cs" | b"CS") => {
                names.insert(ResKind::ColorSpace, name(0).filter(|n| !device(n)));
            }
            Tok::Op(b"scn" | b"SCN") => names.insert(ResKind::Pattern, name(0)),
            Tok::Op(b"BDC" | b"DP") => names.insert(ResKind::Properties, name(0)),
            Tok::InlineImage(_) => {
                let cs = lex::image_color_space(s, step.at.clone(), lim.content.depth);
                let cs = cs
                    .and_then(lex::name)
                    .filter(|n| !device(n) && !abbreviated(n));
                names.insert(ResKind::ColorSpace, cs);
            }
            _ => {}
        }
        Ok(())
    })?;
    Ok(names)
}

/// Whether a color space name is a device space or `/Pattern`: no
/// resource.
fn device(name: &[u8]) -> bool {
    matches!(
        name,
        b"DeviceGray" | b"DeviceRGB" | b"DeviceCMYK" | b"Pattern"
    )
}

/// Whether a color space name is an inline image's abbreviation.
fn abbreviated(name: &[u8]) -> bool {
    matches!(name, b"G" | b"RGB" | b"CMYK" | b"I")
}
