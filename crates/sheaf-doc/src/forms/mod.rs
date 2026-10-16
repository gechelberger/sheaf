//! AcroForms (M10). The field model ([`field`]) and reading it from a
//! source with the facts sheaf shows about the form ([`read`]), and the
//! form check ([`check`]): what in a file's form other viewers would trip
//! on, or what a save would carry where it shouldn't.
//!
//! Tests build forms in code ([`fixture`]) and read real ones from Greg's
//! corpus ([`corpus`]). Both are compiled for this crate's tests and, with
//! the `fixtures` feature, for other crates' tests.

pub mod check;
#[cfg(any(test, feature = "fixtures"))]
pub mod corpus;
pub mod field;
#[cfg(any(test, feature = "fixtures"))]
pub mod fixture;
pub(crate) mod merge;
pub mod read;
pub mod rules;
pub mod xfa;

#[cfg(test)]
mod tests;

pub use check::{Finding, Report, check, check_file};
pub use field::{Field, FieldId, FieldKind, Value, Widget};
pub use read::{FileForm, FormClass, FormFacts, read};
