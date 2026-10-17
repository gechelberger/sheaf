//! Fonts sheaf draws text in: what codes a string is in a font, how far
//! they advance, and the font's resource for a page.
//!
//! - [`Std14`]: the standard 14, with Adobe's widths and exact encoders.
//! - [`Glyphless`]: OCR's invisible text.
//!
//! Text a font can't encode draws `?` and is counted ([`Unencoded`]), so
//! the writer can say so once; the text itself (`/Contents`, a recipe) is
//! kept exactly. M10 slice 7's fallback font ends that, behind the same
//! trait.

mod afm;
mod encoding;
mod glyphless;
mod std14;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use lopdf::Object;

pub(crate) use encoding::{Encoding, glyph_text};
pub use glyphless::Glyphless;
pub use std14::Std14;

use super::ResId;

/// Characters a font couldn't encode: each drew `?`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Unencoded(pub usize);

/// A font sheaf draws text in.
///
/// Any font sheaf embeds is GID-preserving: a subset keeps each glyph's
/// number from the whole font (CID = GID, `/CIDToGIDMap /Identity`), so
/// the codes a string is drawn with depend only on the font and the text,
/// and re-subsetting never changes them. [`Glyphless`], embedded whole,
/// is the one exception: its CIDs are the text's UTF-16, all drawing its
/// one glyph.
pub trait Font: Send + Sync {
    /// `s` as this font's codes. A character it can't encode is drawn as
    /// `?`, or left out by a font without one (ZapfDingbats), and counted.
    fn encode(&self, s: &str) -> (Vec<u8>, Unencoded);

    /// How far codes `b` advance at `size`, in text space: the sum of
    /// their widths, without character or word spacing.
    fn advance(&self, b: &[u8], size: f32) -> f32;

    /// How far `s` advances at `size`, drawn as [`Font::encode`] draws it.
    fn width(&self, s: &str, size: f32) -> f32 {
        self.advance(&self.encode(s).0, size)
    }

    /// `s` in lines no wider than `width` at `size`, as [`wrap_with`]
    /// breaks them.
    fn wrap(&self, s: &str, size: f32, width: f32) -> Vec<String> {
        wrap_with(s, width, |t| self.width(t, size))
    }

    /// Whether it encodes `c`.
    fn covers(&self, c: char) -> bool;

    /// The font as a page resource.
    fn resource(&self) -> FontRes;
}

/// `s` in lines no wider than `width` by `measure`, which says how wide a
/// line of text is: text in more than one font wraps as one. Each `\n`
/// starts a paragraph; words are what spaces separate, a run of them one,
/// and a word wider than a line is broken between characters.
pub fn wrap_with(s: &str, width: f32, measure: impl Fn(&str) -> f32) -> Vec<String> {
    let fits = |t: &str| measure(t) <= width;
    let mut out = Vec::new();
    for para in s.split('\n') {
        let mut line = String::new();
        for word in para.split(' ').filter(|w| !w.is_empty()) {
            let joined = if line.is_empty() {
                word.to_string()
            } else {
                format!("{line} {word}")
            };
            if fits(&joined) {
                line = joined;
                continue;
            }
            if !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            for c in word.chars() {
                line.push(c);
                if !fits(&line) && line.chars().count() > 1 {
                    line.pop();
                    out.push(std::mem::replace(&mut line, c.to_string()));
                }
            }
        }
        out.push(line);
    }
    out
}

/// A font as a page resource ([`Font::resource`]): its dictionary and the
/// objects it reaches, named by a digest of all of them, so a save writes
/// one font once however many pages draw in it. It's a [`Res`](super::Res)
/// like any other.
#[derive(Debug, Clone, PartialEq)]
pub struct FontRes {
    pub id: ResId,
    /// The font dictionary first, then the objects it reaches (descendant
    /// font, descriptor, program, maps), streams decoded. A reference
    /// `(i, 0)` among them is to `objects[i]`.
    pub objects: Arc<[Object]>,
}
