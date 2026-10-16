//! Open a PDF into a document model with no window: what the app does
//! when a file opens, minus the view state.

use std::path::Path;

use anyhow::{Context, Result};
use sheaf_doc::file::ForeignAnnot;
use sheaf_doc::{Document, FlatOutlineItem, SourceInfo, Store};
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
    let store = Store::new();
    let (source, outline) = describe(&store, path, password)?;
    let (annots, facts) = sheaf_doc::file::read_open(&source.src)
        .with_context(|| format!("reading {}", path.display()))?;
    let mut doc = Document::new(
        store,
        source,
        &outline,
        sheaf_doc::history::default_policy(),
    );
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

/// A renderer for a source, on its blob's bytes.
pub fn engine_for(info: &SourceInfo) -> Result<PdfiumEngine> {
    PdfiumEngine::open_bytes(info.src.bytes().clone(), info.src.password())
}

/// `path` as a source in `store`, read once and parsed, and its bookmarks
/// (page numbers are 0-based indexes into the file), which PDFium reads
/// from the same bytes.
pub fn describe(
    store: &Store,
    path: &Path,
    password: Option<&str>,
) -> Result<(SourceInfo, Vec<FlatOutlineItem>)> {
    let source = SourceInfo::read(store, path, password)?;
    let engine = engine_for(&source)?;
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
        && let Ok(pdf) = sheaf_doc::file::load_source(&source)
    {
        sheaf_doc::file::style_flat_items(&mut outline, &sheaf_doc::file::outline_styles(&pdf));
    }
    Ok((source, outline))
}
