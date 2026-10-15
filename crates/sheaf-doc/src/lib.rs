//! The document model: a virtual sequence of pages over immutable source
//! files, with an outline tree, snapshot-based undo/redo, and (later)
//! per-page markup. See `docs/document-model.md`.
//!
//! Nothing in here knows about PDFium or lopdf. Sources are described by
//! [`SourceInfo`]; rendering and saving live in other crates.

pub mod annot;
pub mod attachments;
pub mod describe;
pub mod exchange;
pub mod file;
pub mod generated;
pub mod history;
pub mod images;
pub mod import;
pub mod incremental;
pub(crate) mod kept;
pub mod labels;
pub mod layers;
pub mod model;
pub mod page_chars;
pub mod portfolio;
pub mod properties;
pub mod recovery;
pub mod save;
pub mod stamp_lib;
pub mod stamps;
pub mod tape;
pub mod text_layer;
pub mod watermark;

pub use sheaf_core::{DestView, PageFrame};

pub use annot::{Annot, AnnotId, AnnotKind, Box2, PageGeom, Pt, default_author, pdf_date_now};
pub use history::{AllOf, FixedCount, HistoryPolicy, HistoryStats, MemoryBudget};
pub use model::{
    DocState, Document, Edit, EditOutcome, FlatOutlineItem, JobId, NewOutlineItem, OutlineId,
    OutlineNode, PageId, PageRef, Rebase, Saved, Snapshot, SourceData, SourceId, SourceInfo, Step,
    TextStyle, VersionId, VersionView,
};
pub use page_chars::{Glyph, PageChars};
pub use text_layer::{OcrWord, TextHit, TextLayer};
