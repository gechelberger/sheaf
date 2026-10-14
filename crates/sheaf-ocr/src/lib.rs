//! OCR for sheaf.
//!
//! An [`OcrEngine`] turns a page image into words with boxes. Two engines
//! are built in behind cargo features: Tesseract (`tesseract`, built from
//! source and linked statically) and PaddleOCR models on ONNX Runtime
//! (`oar`, via the `oar-ocr` crate). With neither feature the crate still
//! compiles so the app's UI can say "no engine in this build".
//!
//! [`job`] runs an engine over a list of pages on its own threads, renders
//! pages through the app's render worker, and streams per-page
//! [`TextLayer`](sheaf_doc::TextLayer)s back for the document model.

pub mod config;
pub mod data;
pub mod engine;
pub mod geom;
pub mod job;

#[cfg(feature = "oar")]
mod oar;
#[cfg(feature = "tesseract")]
mod tesseract;

pub use config::{OarConfig, OcrConfig, Scope, TesseractConfig};
pub use engine::{
    EngineError, EngineInfo, EngineKind, InputKind, OcrEngine, OcrImage, OcrWord, build_engine,
    check_engine, configured_language, engines, language_options, languages, set_language,
};
pub use job::{JobHandle, OcrEvent, OcrPage, OcrRequest, PageSource};
