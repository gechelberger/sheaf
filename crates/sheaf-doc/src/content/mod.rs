//! The content toolkit (M10.5 §13), pulled forward from M11: page
//! content read as tokens, without lopdf's `Content::decode`, which
//! re-encodes what it reads, loses comments and spacing, and reads inline
//! images by the first `EI` it meets.
//!
//! - [`lex`]: a content stream's tokens, each with where it is and its
//!   bytes as written, an inline image one token found by PDFium's rules.
//! - [`page`]: a page's `/Contents` as one, the concatenation PDFium lexes,
//!   read within [`ContentLimits`](crate::source::ContentLimits), and
//!   places in it mapped back to a stream.
//! - [`marked`], [`balance`] and [`names_used`]: its marked ranges, its
//!   `q`/`Q` balance and the resource names it uses.
//! - [`splice`]: its streams with ranges taken out, byte for byte, when
//!   that's safe, and refused when it isn't.
//! - [`PageBlocks`]: its pagination artifacts and who drew them, and
//!   sheaf's own blocks.
//! - [`text_bag`]: the words it draws, decoded through its fonts.
//! - [`Font`]: what sheaf draws text in, [`Std14`] and [`Glyphless`], and
//!   [`FontRes`], a font as a page resource named by its [`ResId`].
//! - [`ContentWriter`]: content written, its resources ([`Res`]) named at
//!   the end, and [`ResObjects`], which writes them for the old writers.

mod balance;
mod blocks;
#[cfg(test)]
mod blocks_tests;
#[cfg(test)]
mod corpus;
mod font;
mod lex;
mod marked;
mod names;
mod page;
mod res;
mod splice;
#[cfg(test)]
mod tests;
mod text_bag;
#[cfg(test)]
mod text_bag_tests;
mod to_lopdf;
mod writer;
#[cfg(test)]
mod writer_tests;

pub use balance::{Balance, balance};
pub use blocks::{
    BlockAt, BlockId, ContentError, DrawnForms, GenId, LegacyOcr, PagOrigin, PageBlocks,
    Pagination, PaginationKind, SheafBlock, SheafBlockRef,
};
pub use font::{Font, FontRes, Glyphless, Std14, Unencoded, wrap_with};
pub use lex::{LexError, LexErrorKind, ResCx, Tok, lex, name, object};
pub use marked::{Marked, PropertiesDict, Props, marked};
pub use names::{NameSet, ResKind, names_used};
pub use page::{Concat, ContentFault, PageContent, Pos, ReadObjects, SpanAcross, contents};
pub use res::{Res, ResBody, ResId};
pub use splice::{SpliceError, splice};
pub use text_bag::{FontDecoders, TextBag, text_bag};
pub use to_lopdf::{ResObjects, resources_of};
pub use writer::{
    Blend, Color, ContentWriter, FillRule, GState, LineCap, LineJoin, Need, Paint, Reserved,
    ResourceNeeds, TextMode, write_string,
};
