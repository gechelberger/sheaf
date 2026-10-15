//! AcroForms (M10). So far, the form check ([`check`]): what in a file's
//! form other viewers would trip on, or what a save would carry where it
//! shouldn't.
//!
//! Tests build forms in code ([`fixture`]) and read real ones from Greg's
//! corpus ([`corpus`]). Both are compiled for this crate's tests and, with
//! the `fixtures` feature, for other crates' tests.

pub mod check;
#[cfg(any(test, feature = "fixtures"))]
pub mod corpus;
#[cfg(any(test, feature = "fixtures"))]
pub mod fixture;

#[cfg(test)]
mod tests;

pub use check::{Finding, Report, check, check_file};
