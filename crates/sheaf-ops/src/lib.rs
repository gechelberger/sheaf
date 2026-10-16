//! Document operations with no GUI, shared by the app and `sheaf-cli`.
//!
//! An op is a plain function of its inputs and a `&dyn Progress`
//! (`sheaf_progress`): it reports as it goes and stops at the next
//! `check()` once cancelled. Ops move here one at a time, when the
//! command line needs them.

pub mod blank;
pub mod combine;
pub mod comments;
pub mod crop;
pub mod derive;
pub mod dirs;
pub mod export;
pub mod images;
pub mod pages;
pub mod prefs;
pub mod scan;
pub mod split;
pub mod stamps;

use anyhow::Result;
use sheaf_doc::SourceInfo;
use sheaf_render::pdfium::PdfiumEngine;

pub use sheaf_progress::{Cancelled, NoProgress, Progress, Snapshot as ProgressSnapshot, Tracker};

/// A renderer for a source, on its blob's bytes.
pub fn engine_for(info: &SourceInfo) -> Result<PdfiumEngine> {
    PdfiumEngine::open_bytes(info.src.bytes().clone(), info.src.password())
}
