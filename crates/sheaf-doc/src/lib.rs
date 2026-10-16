//! The document model: a virtual sequence of pages over immutable source
//! files, with an outline tree, marks and form fields, and undo and redo.
//! See `docs/document-model.md`.
//!
//! The files are blobs in a [`Store`], each PDF read lazily as a
//! [`Source`] with lopdf (vendored, `vendor/lopdf`), which also writes
//! saves. Nothing in here knows about PDFium: rendering lives in other
//! crates.

pub mod annot;
pub mod attachments;
pub mod describe;
pub mod exchange;
pub mod file;
pub mod forms;
pub mod generated;
pub mod geom;
pub mod history;
pub mod images;
pub mod import;
pub mod incremental;
pub(crate) mod kept;
pub mod labels;
pub mod layers;
pub mod measure;
pub mod model;
pub mod open;
pub mod page_chars;
pub mod portfolio;
pub mod prim;
pub mod properties;
pub mod recovery;
pub mod save;
pub mod source;
pub mod stamp_lib;
pub mod stamps;
pub mod store;
pub mod tape;
pub mod text_layer;
pub mod watermark;

pub use sheaf_core::{DestView, PageFrame};

pub use annot::{Annot, AnnotId, AnnotKind, Box2, PageGeom, Pt, default_author, pdf_date_now};
pub use history::{AllOf, FixedCount, HistoryPolicy, HistoryStats, MemoryBudget};
pub use model::{
    Damaged, Derived, DocState, Document, Edit, EditOutcome, FlatOutlineItem, JobId,
    NewOutlineItem, NotReady, OutlineId, OutlineNode, PageId, PageRef, Rebase, Saved, Snapshot,
    SourceId, SourceInfo, Step, TextStyle, VersionId, VersionView,
};
pub use page_chars::{Glyph, PageChars};
pub use source::Source;
pub use store::{Blob, BlobId, Src, Store};
pub use text_layer::{OcrWord, TextHit, TextLayer};
