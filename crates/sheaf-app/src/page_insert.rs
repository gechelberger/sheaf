//! Document > Insert Pages > Blank Pages… and From Images…, and Replace
//! Pages…: pages with no file of their own become PDFs in the generated-
//! file cache (`sheaf_doc::generated`), then go in like any inserted file.

use std::path::PathBuf;

use gpui::Context;
use sheaf_core::PageSize;
use sheaf_doc::images::PageFit;
use sheaf_doc::{Edit, PageId, SourceInfo};
use sheaf_render::RenderHandle;

use crate::docview::{DocView, State};

/// Where inserted pages go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Where {
    AfterCurrent,
    BeforeCurrent,
    Start,
    End,
}

/// A page size choice: the current page's, or a sheet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Size {
    Current,
    Sheet(PageSize),
}

pub const LETTER: PageSize = PageSize {
    width: 612.0,
    height: 792.0,
};
pub const A4: PageSize = PageSize {
    width: 595.28,
    height: 841.89,
};
pub const LEGAL: PageSize = PageSize {
    width: 612.0,
    height: 1008.0,
};

impl DocView {
    /// The page inserted pages go before, for `at` (None: the end).
    pub(crate) fn before_for(&self, at: Where) -> Option<PageId> {
        let State::Ready(doc) = &self.state else {
            return None;
        };
        match at {
            Where::AfterCurrent => self.insert_point(),
            Where::BeforeCurrent => self.target_ids().first().copied(),
            Where::Start => doc.page_ref(0).map(|p| p.id),
            Where::End => None,
        }
    }

    /// The current page's size as shown.
    pub(crate) fn current_size(&self) -> PageSize {
        let State::Ready(doc) = &self.state else {
            return LETTER;
        };
        doc.page_ref(doc.current_page())
            .map(|p| doc.model.page_size(&p))
            .unwrap_or(LETTER)
    }

    fn size_of(&self, size: Size) -> PageSize {
        match size {
            Size::Current => self.current_size(),
            Size::Sheet(s) => s,
        }
    }

    pub(crate) fn insert_blank(
        &mut self,
        count: usize,
        size: Size,
        at: Where,
        cx: &mut Context<Self>,
    ) {
        let s = self.size_of(size);
        let made = sheaf_doc::generated::blank_pages(s.width, s.height, count)
            .and_then(|pdf| sheaf_doc::generated::store(&pdf));
        match made {
            Ok(path) => {
                let before = self.before_for(at);
                let name = if count == 1 {
                    "blank page".to_string()
                } else {
                    format!("{count} blank pages")
                };
                self.insert_file(path, before, name, false, cx);
            }
            Err(e) => self.set_notice(format!("Insert failed: {e:#}"), cx),
        }
    }

    /// Images as pages, fitted to `size` (None: each image's own size).
    pub(crate) fn insert_images(
        &mut self,
        files: Vec<PathBuf>,
        size: Option<Size>,
        at: Where,
        cx: &mut Context<Self>,
    ) {
        let fit = match size.map(|s| self.size_of(s)) {
            Some(s) => PageFit::Paper {
                width: s.width.min(s.height),
                height: s.width.max(s.height),
                margin: 0.0,
            },
            None => PageFit::Original,
        };
        let before = self.before_for(at);
        let name = match files.as_slice() {
            [one] => one
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            many => format!("{} images", many.len()),
        };
        self.set_notice(format!("Reading {name}…"), cx);
        cx.spawn(async move |this, cx| {
            let made = cx
                .background_executor()
                .spawn(async move {
                    let pdf = sheaf_doc::images::files_to_pdf(&files, fit)?;
                    sheaf_doc::generated::store(&pdf)
                })
                .await;
            this.update(cx, |v, cx| match made {
                Ok(path) => v.insert_file(path, before, name, false, cx),
                Err(e) => v.set_notice(format!("Insert failed: {e:#}"), cx),
            })
            .ok();
        })
        .detach();
    }

    /// Replace `ids` with pages `range` (0-based) of the PDF at `path`, as
    /// one undo step. They keep their bookmarks; extra pages on either
    /// side are deleted or inserted after.
    pub(crate) fn replace_pages_with(
        &mut self,
        path: PathBuf,
        ids: Vec<PageId>,
        range: std::ops::Range<u32>,
        cx: &mut Context<Self>,
    ) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        self.set_notice(format!("Replacing pages from {name}…"), cx);
        cx.spawn(async move |this, cx| {
            let opened = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    async move {
                        let bytes = std::fs::metadata(&path)
                            .map(|m| m.len() as usize)
                            .unwrap_or(0);
                        anyhow::Ok((RenderHandle::open(path, None)?, bytes))
                    }
                })
                .await;
            this.update(cx, |v, cx| {
                let (handle, bytes) = match opened {
                    Ok(h) => h,
                    Err(e) => {
                        v.set_notice(format!("Replace failed: {e:#}"), cx);
                        return;
                    }
                };
                let State::Ready(doc) = &mut v.state else {
                    return;
                };
                let info = handle.info();
                let sid = doc.model.add_source(SourceInfo {
                    path: path.clone(),
                    page_count: info.page_count,
                    first_page: info.first_page,
                    page_sizes: None,
                    bytes,
                    password: None,
                });
                let sizes_rx = handle.page_sizes();
                doc.handles.insert(sid, handle);
                let n = ids.len();
                let label = format!("Replace {n} page{}", if n == 1 { "" } else { "s" });
                match doc.model.apply(
                    Edit::ReplacePages {
                        ids,
                        source: sid,
                        range: range.clone(),
                    },
                    &label,
                ) {
                    Ok(out) => {
                        doc.touched();
                        v.after_model_change(label, out, cx);
                        v.set_notice(
                            format!(
                                "Replaced {n} page{} with {} from {name}",
                                if n == 1 { "" } else { "s" },
                                range.len()
                            ),
                            cx,
                        );
                    }
                    Err(e) => v.set_notice(format!("Replace failed: {e:#}"), cx),
                }
                cx.spawn(async move |this, cx| {
                    if let Ok(Ok(sizes)) = sizes_rx.await {
                        this.update(cx, |v, cx| {
                            if let State::Ready(doc) = &mut v.state {
                                doc.model.set_source_sizes(sid, sizes);
                                doc.touched();
                                cx.notify();
                            }
                        })
                        .ok();
                    }
                })
                .detach();
            })
            .ok();
        })
        .detach();
    }
}

impl DocView {
    /// The page count, and the 0-based indexes of the selected pages (or
    /// the current one).
    pub(crate) fn target_indexes(&self) -> (usize, Vec<usize>) {
        let State::Ready(doc) = &self.state else {
            return (0, Vec::new());
        };
        let idx = self
            .target_ids()
            .iter()
            .filter_map(|id| doc.model.index_of(*id))
            .collect();
        (doc.model.page_count(), idx)
    }

    /// The ids of the pages at these 0-based indexes.
    pub(crate) fn page_ids_at(&self, indexes: &[usize]) -> Vec<PageId> {
        let State::Ready(doc) = &self.state else {
            return Vec::new();
        };
        indexes
            .iter()
            .filter_map(|&i| doc.page_ref(i).map(|p| p.id))
            .collect()
    }
}

impl DocView {
    /// Document > Crop Pages: plan in the background (cropped copies go to
    /// the generated cache), then show the pages from them as one step.
    pub(crate) fn crop_pages(
        &mut self,
        pages: Vec<usize>,
        spec: sheaf_ops::crop::CropSpec,
        cx: &mut Context<Self>,
    ) {
        let ids = self.page_ids_at(&pages);
        let State::Ready(doc) = &self.state else {
            return;
        };
        if ids.is_empty() {
            return;
        }
        let snapshot = doc.model.snapshot();
        let n = ids.len();
        self.set_notice(
            format!("Cropping {n} page{}…", if n == 1 { "" } else { "s" }),
            cx,
        );
        cx.spawn(async move |this, cx| {
            let planned = cx
                .background_executor()
                .spawn(async move {
                    let plan =
                        sheaf_ops::crop::plan(&snapshot, &ids, spec, &sheaf_ops::NoProgress)?;
                    let mut opened = Vec::new();
                    for c in plan {
                        let handle = RenderHandle::open(c.info.path.clone(), None)?;
                        opened.push((c, handle));
                    }
                    anyhow::Ok(opened)
                })
                .await;
            this.update(cx, |v, cx| {
                let opened = match planned {
                    Ok(o) => o,
                    Err(e) => {
                        v.set_notice(format!("Crop failed: {e:#}"), cx);
                        return;
                    }
                };
                let State::Ready(doc) = &mut v.state else {
                    return;
                };
                let mut rebases = Vec::new();
                for (c, handle) in opened {
                    let sid = doc.model.add_source(c.info.clone());
                    doc.handles.insert(sid, handle);
                    rebases.extend(c.rebases(sid));
                }
                let label = format!("Crop {n} page{}", if n == 1 { "" } else { "s" });
                match doc
                    .model
                    .apply(Edit::RebasePages { pages: rebases }, &label)
                {
                    Ok(out) => {
                        doc.touched();
                        v.after_model_change(label.clone(), out, cx);
                        v.set_notice(
                            format!("Cropped {n} page{}", if n == 1 { "" } else { "s" }),
                            cx,
                        );
                    }
                    Err(e) => v.set_notice(format!("Crop failed: {e:#}"), cx),
                }
            })
            .ok();
        })
        .detach();
    }
}

impl DocView {
    /// The current page's 0-based index.
    pub(crate) fn current_index(&self) -> usize {
        match &self.state {
            State::Ready(doc) => doc.current_page(),
            _ => 0,
        }
    }
}
