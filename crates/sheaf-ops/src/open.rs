//! Open a PDF into a document model with no window: what the app does
//! when a file opens, minus the view state.

use std::path::Path;

use anyhow::{Context, Result};
use sheaf_doc::file::ForeignAnnot;
use sheaf_doc::{Document, FlatOutlineItem, SourceInfo};
use sheaf_render::Renderer;
use sheaf_render::pdfium::PdfiumEngine;

/// `path` as a document: its pages, bookmarks, page labels, sheaf's own
/// marks and the form's fields, as the app shows them on open.
pub fn open(path: &Path, password: Option<&str>) -> Result<Document> {
    Ok(open_with_foreign(path, password)?.0)
}

/// [`open`], and the other tools' annotations in the file.
pub fn open_with_foreign(
    path: &Path,
    password: Option<&str>,
) -> Result<(Document, Vec<ForeignAnnot>)> {
    let (source, outline) = describe(path, password)?;
    let mut doc = Document::new(source, &outline, sheaf_doc::history::default_policy());
    let (annots, facts) = sheaf_doc::file::read_open(path, password)?;
    if !annots.sheaf.is_empty() {
        doc.set_initial_annots(annots.sheaf)?;
    }
    if let Some(ranges) = &facts.label_ranges {
        doc.set_initial_labels(ranges)?;
    }
    if let Some((hf, scope)) = facts.header_footer {
        doc.set_initial_header_footer(hf, scope)?;
    }
    if !facts.form.fields.is_empty() || !facts.form.widgets.is_empty() {
        doc.set_initial_fields(&facts.form)?;
    }
    Ok((doc, annots.foreign))
}

/// A renderer for a source: on the bytes the document kept from when it
/// opened, when it kept them (see `SourceInfo::data`), else on the file.
pub fn engine_for(info: &SourceInfo) -> Result<PdfiumEngine> {
    match &info.data {
        Some(data) => PdfiumEngine::open_bytes(data.0.clone(), info.password.as_deref()),
        None => PdfiumEngine::open(&info.path, info.password.as_deref()),
    }
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
        data: None,
    };
    let mut outline: Vec<FlatOutlineItem> = engine
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
            color: None,
            style: Default::default(),
        })
        .collect();
    // PDFium doesn't report bookmark colors and styles; lopdf does.
    if !outline.is_empty()
        && let Ok(pdf) = sheaf_doc::file::load_pdf(path, password)
    {
        sheaf_doc::file::style_flat_items(&mut outline, &sheaf_doc::file::outline_styles(&pdf));
    }
    Ok((source, outline))
}
