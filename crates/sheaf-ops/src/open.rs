//! Open a PDF into a document model with no window: what the app does
//! when a file opens, minus the view state.

use std::path::Path;

use anyhow::{Context, Result};
use sheaf_doc::{Document, FlatOutlineItem, SourceInfo};
use sheaf_render::Renderer;
use sheaf_render::pdfium::PdfiumEngine;

/// `path` as a document: its pages, bookmarks and sheaf's own marks, as
/// the app shows them on open.
pub fn open(path: &Path, password: Option<&str>) -> Result<Document> {
    let (source, outline) = describe(path, password)?;
    let mut doc = Document::new(source, &outline, sheaf_doc::history::default_policy());
    let (annots, _) = sheaf_doc::file::read_open(path, password)?;
    if !annots.sheaf.is_empty() {
        doc.set_initial_annots(annots.sheaf)?;
    }
    Ok(doc)
}

/// What the model needs to know about a file to use its pages, and its
/// bookmarks (page numbers are 0-based indexes into the file).
pub fn describe(path: &Path, password: Option<&str>) -> Result<(SourceInfo, Vec<FlatOutlineItem>)> {
    let engine = PdfiumEngine::open(path, password)?;
    let sizes = engine.page_sizes()?;
    let first_page = *sizes
        .first()
        .with_context(|| format!("{} has no pages", path.display()))?;
    let source = SourceInfo {
        path: path.to_path_buf(),
        page_count: engine.page_count(),
        first_page,
        page_sizes: Some(sizes),
        bytes: std::fs::metadata(path)
            .map(|m| m.len() as usize)
            .unwrap_or(0),
        password: password.map(str::to_string),
    };
    let outline = engine
        .outline()
        .unwrap_or_else(|e| {
            log::warn!("{}: bookmarks: {e:#}", path.display());
            Vec::new()
        })
        .into_iter()
        .map(|i| FlatOutlineItem {
            title: i.title,
            page: i.page,
            depth: i.depth,
            view: i.view,
        })
        .collect();
    Ok((source, outline))
}
