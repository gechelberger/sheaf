//! The document model: a virtual sequence of pages over immutable source
//! files, with an outline tree, snapshot-based undo/redo, and (later)
//! per-page markup. See `docs/document-model.md`.
//!
//! Nothing in here knows about PDFium or lopdf. Sources are described by
//! [`SourceInfo`]; rendering and saving live in other crates.

pub mod file;
pub mod history;
pub mod model;

pub use history::{AllOf, FixedCount, HistoryPolicy, HistoryStats, MemoryBudget};
pub use model::{
    DocState, Document, Edit, EditOutcome, FlatOutlineItem, OutlineId, OutlineNode, PageId,
    PageRef, SourceId, SourceInfo,
};
