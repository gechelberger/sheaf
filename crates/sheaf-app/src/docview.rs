//! One open document: continuous vertical scroll of pages at a shared zoom,
//! rendered as tiles so only what's on screen is rasterized, with a cheap
//! low-res preview under the tiles so something appears immediately. Also
//! owns the per-document sidebar content (thumbnails, outline), since those
//! share its render handles and caches.
//!
//! The page sequence comes from `sheaf_doc::Document` (see
//! docs/document-model.md): a virtual list of `PageRef`s over one or more
//! source files, each with its own render worker. Caches key by `PageId`, so
//! reordering pages costs nothing visually.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use futures::StreamExt;
use gpui::{
    AnyElement, App, Context, DragMoveEvent, Entity, FocusHandle, Focusable, ObjectFit,
    RenderImage, ScrollHandle, ScrollWheelEvent, SharedString, Subscription, Task, Window, div,
    img, point, prelude::*, px, rgb, rgba,
};
use sheaf_core::{PageIndex, PageSize, Rotation};
use sheaf_doc::save;
use sheaf_doc::{
    Document, Edit, FlatOutlineItem, OutlineId, PageId, PageRef, SourceId, SourceInfo,
};
use sheaf_render::{
    Job, OutlineItem, PixelFormat, Rect, RenderHandle, RenderRequest, Rendered, SearchEvent,
    SearchHit,
};

use crate::text_input::TextInput;
use crate::{
    CloseSearch, DeletePage, DuplicatePages, Find, FirstPage, LastPage, LineDown, LineUp, NextPage,
    PrevPage, Redo, RotatePageCcw, RotatePageCw, Save, SaveAs, ScreenDown, ScreenUp, SearchNext,
    SearchPrev, SelectAllPages, Undo, ZoomActual, ZoomFitPage, ZoomFitWidth, ZoomIn, ZoomOut,
};

pub const KEY_CONTEXT: &str = "DocView";

/// Space around the page column and between pages, in logical px.
const MARGIN: f32 = 16.0;
const GAP: f32 = 12.0;
const STATUS_H: f32 = 24.0;
/// Pages beyond the visible range to prefetch previews for.
const PREFETCH: usize = 1;
/// Pages beyond the visible range to keep cached before releasing textures.
const KEEP: usize = 3;
const LINE_STEP: f32 = 80.0;
/// Tile edge in device pixels. 1024² BGRA = 4 MB per tile.
const TILE: u32 = 1024;
/// Width of the low-res whole-page preview, device px. Also used for thumbnails.
const PREVIEW_W: u32 = 320;
/// Vertical overdraw for tile prefetch, as a fraction of the viewport.
const TILE_OVERDRAW: f32 = 0.5;
/// 100% zoom = 96 logical px per inch = 96/72 px per PDF point (Acrobat's convention).
const PX_PER_PT_AT_100: f32 = 96.0 / 72.0;
const ZOOM_STEP: f32 = 1.25;
const ZOOM_MIN: f32 = 0.1;
const ZOOM_MAX: f32 = 16.0;

/// Thumbnail panel geometry, logical px.
const THUMB_W: f32 = 150.0;
const THUMB_PAD: f32 = 10.0;
const THUMB_LABEL_H: f32 = 18.0;
const OUTLINE_ROW_H: f32 = 24.0;
const OUTLINE_INDENT: f32 = 14.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Zoom {
    FitWidth,
    FitPage,
    /// 1.0 = 100%.
    Custom(f32),
}

enum State {
    Loading,
    Failed(String),
    Ready(Box<Doc>),
}

/// Page geometry in logical px at the current zoom, for one snapshot of the
/// page sequence.
struct Layout {
    zoom: Zoom,
    view_w: f32,
    view_h: f32,
    version: u64,
    /// Logical px per PDF point.
    k: f32,
    refs: Vec<PageRef>,
    widths: Vec<f32>,
    heights: Vec<f32>,
    tops: Vec<f32>,
    lefts: Vec<f32>,
    /// Scrollable content size.
    total_w: f32,
    total_h: f32,
}

impl Layout {
    fn build(
        zoom: Zoom,
        view_w: f32,
        view_h: f32,
        version: u64,
        refs: Vec<PageRef>,
        model: &Document,
    ) -> Self {
        let avail_w = (view_w - 2.0 * MARGIN).max(1.0);
        let avail_h = (view_h - 2.0 * MARGIN).max(1.0);
        let sizes: Vec<PageSize> = refs.iter().map(|r| model.page_size(r)).collect();
        let (mut max_w, mut max_h) = (1.0f32, 1.0f32);
        for s in &sizes {
            max_w = max_w.max(s.width);
            max_h = max_h.max(s.height);
        }
        let k = match zoom {
            Zoom::FitWidth => avail_w / max_w,
            Zoom::FitPage => (avail_w / max_w).min(avail_h / max_h),
            Zoom::Custom(z) => z * PX_PER_PT_AT_100,
        };
        let total_w = (max_w * k + 2.0 * MARGIN).max(view_w);
        let n = refs.len();
        let (mut widths, mut heights, mut tops, mut lefts) = (
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
        );
        let mut y = MARGIN;
        for s in &sizes {
            let (w, h) = (s.width * k, s.height * k);
            widths.push(w);
            heights.push(h);
            tops.push(y);
            lefts.push((total_w - w) / 2.0);
            y += h + GAP;
        }
        let total_h = (if n > 0 { y - GAP } else { y } + MARGIN).max(view_h);
        Self {
            zoom,
            view_w,
            view_h,
            version,
            k,
            refs,
            widths,
            heights,
            tops,
            lefts,
            total_w,
            total_h,
        }
    }

    fn len(&self) -> usize {
        self.refs.len()
    }

    /// Page whose vertical span contains `y` (or the last one that starts above it).
    fn page_at(&self, y: f32) -> usize {
        self.tops.partition_point(|&top| top <= y).saturating_sub(1)
    }

    fn visible(&self, top: f32, bottom: f32) -> std::ops::RangeInclusive<usize> {
        let last = self.len().saturating_sub(1);
        self.page_at(top).min(last)..=self.page_at(bottom).min(last)
    }

    fn zoom_percent(&self) -> f32 {
        self.k / PX_PER_PT_AT_100 * 100.0
    }
}

type TileKey = (PageId, Job);

struct Doc {
    model: Document,
    handles: HashMap<SourceId, RenderHandle>,
    /// Bumped on every model change so layouts and lookups can tell.
    version: u64,
    zoom: Zoom,
    scroll: ScrollHandle,
    layout: Option<Layout>,
    previews: HashMap<PageId, (Rotation, Arc<RenderImage>)>,
    tiles: HashMap<TileKey, Arc<RenderImage>>,
    /// Dropping the task drops the oneshot receiver, which tells the worker
    /// to skip the render if it hasn't started.
    inflight: HashMap<TileKey, Task<()>>,
    /// Images we've stopped showing; GPU textures are released in `render`,
    /// the only place we have a `Window`.
    retired: Vec<Arc<RenderImage>>,
    current_page: usize,
    pending_scroll: Option<ScrollTo>,
    first_paint: Option<Instant>,
    /// Last edit notice, shown in the status bar.
    notice: Option<String>,
    // Sidebar state.
    thumb_scroll: ScrollHandle,
    /// Pages whose previews the thumbnail panel is showing; kept cached.
    thumb_keep: HashSet<PageId>,
    /// Last page the thumbnail panel auto-scrolled to follow.
    thumb_followed: Option<PageId>,
    outline_loaded: bool,
    outline_expanded: HashSet<OutlineId>,
    outline_scroll: ScrollHandle,
    // Selection and drag state for the thumbnail panel.
    selection: HashSet<PageId>,
    /// Index the last plain/ctrl click landed on; shift-click extends from it.
    sel_anchor: Option<usize>,
    /// Insert index under the cursor while pages are being dragged.
    drop_target: Option<usize>,
    /// A save is running on a background thread.
    saving: bool,
}

impl Doc {
    fn primary(&self) -> &RenderHandle {
        &self.handles[&self.model.primary()]
    }

    fn page_ref(&self, index: usize) -> Option<PageRef> {
        self.model.page(index).copied()
    }

    /// Display index of the primary source's page `index`, if it's still in
    /// the document (search hits are reported in source pages).
    fn index_of_source_page(&self, index: u32) -> Option<usize> {
        let primary = self.model.primary();
        self.model
            .state()
            .pages
            .iter()
            .position(|p| p.source == primary && p.index == index)
    }

    fn touched(&mut self) {
        self.version += 1;
        self.layout = None;
    }
}

#[derive(Clone, Copy)]
enum ScrollTo {
    Page(usize),
    By(f32),
    End,
    /// Bring a page-relative point (fractions) into view if it isn't already,
    /// landing it a third of the way down the viewport.
    Reveal {
        page: usize,
        fx: f32,
        fy: f32,
    },
    /// Keep the document point (`page`, `fx`, `fy` as fractions of the page)
    /// under viewport-local point (`vx`, `vy`). Used to anchor zooms.
    Anchor {
        page: usize,
        fx: f32,
        fy: f32,
        vx: f32,
        vy: f32,
    },
}

pub struct DocView {
    focus_handle: FocusHandle,
    path: PathBuf,
    timing: bool,
    launched: Instant,
    state: State,
    search: Option<Search>,
    initial_query: Option<String>,
    search_started: Option<Instant>,
    selftest_save: Option<PathBuf>,
}

/// An open search bar and the running / finished query behind it.
struct Search {
    input: Entity<TextInput>,
    _input_sub: Subscription,
    /// Query the current results are for.
    query: String,
    /// Hits in primary-source page order (the worker scans pages in order).
    hits: Vec<SearchHit>,
    current: Option<usize>,
    scanned: u32,
    done: bool,
    /// Dropping this cancels the worker-side search.
    _task: Option<Task<()>>,
}

impl DocView {
    pub fn new(path: PathBuf, timing: bool, launched: Instant, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            path,
            timing,
            launched,
            state: State::Loading,
            search: None,
            initial_query: None,
            search_started: None,
            selftest_save: None,
        };
        this.open(cx);
        this
    }

    pub fn title(&self) -> SharedString {
        let name = self
            .path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("?");
        let dirty = matches!(&self.state, State::Ready(d) if d.model.is_dirty());
        if dirty {
            format!("{name} •").into()
        } else {
            name.to_string().into()
        }
    }

    fn open(&mut self, cx: &mut Context<Self>) {
        let path = self.path.clone();
        self.state = State::Loading;
        let timing = self.timing;
        let launched = self.launched;
        cx.spawn(async move |this, cx| {
            let opened = cx
                .background_executor()
                .spawn(async move {
                    let bytes = std::fs::metadata(&path)
                        .map(|m| m.len() as usize)
                        .unwrap_or(0);
                    RenderHandle::open(path.clone(), None).map(|h| (h, path, bytes))
                })
                .await;
            this.update(cx, |viewer, cx| {
                match opened {
                    Ok((handle, path, bytes)) => {
                        let info = handle.info();
                        if timing {
                            eprintln!(
                                "[timing] open: {:?} engine, {:?} since launch, {} pages",
                                info.open_time,
                                launched.elapsed(),
                                info.page_count
                            );
                        }
                        let source = SourceInfo {
                            path,
                            page_count: info.page_count,
                            first_page: info.first_page,
                            page_sizes: None,
                            bytes,
                        };
                        let model =
                            Document::new(source, &[], sheaf_doc::history::default_policy());
                        let mut handles = HashMap::new();
                        handles.insert(model.primary(), handle);
                        viewer.state = State::Ready(Box::new(Doc {
                            model,
                            handles,
                            version: 0,
                            zoom: Zoom::FitWidth,
                            scroll: ScrollHandle::new(),
                            layout: None,
                            previews: HashMap::new(),
                            tiles: HashMap::new(),
                            inflight: HashMap::new(),
                            retired: Vec::new(),
                            current_page: 0,
                            pending_scroll: None,
                            first_paint: None,
                            notice: None,
                            thumb_scroll: ScrollHandle::new(),
                            thumb_keep: HashSet::new(),
                            thumb_followed: None,
                            outline_loaded: false,
                            outline_expanded: HashSet::new(),
                            outline_scroll: ScrollHandle::new(),
                            selection: HashSet::new(),
                            sel_anchor: None,
                            drop_target: None,
                            saving: false,
                        }));
                        viewer.fetch_page_sizes(cx);
                        viewer.fetch_outline(cx);
                        if let Some(p) = viewer.selftest_save.take() {
                            viewer.selftest_save(p, cx);
                        }
                        if let Some(q) = viewer.initial_query.take() {
                            viewer.search = Some(Search {
                                input: cx.new(|cx| TextInput::new("Find in document", cx)),
                                _input_sub: Subscription::new(|| {}),
                                query: String::new(),
                                hits: Vec::new(),
                                current: None,
                                scanned: 0,
                                done: true,
                                _task: None,
                            });
                            viewer.start_search(q, cx);
                        }
                    }
                    Err(e) => viewer.state = State::Failed(format!("{e:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn fetch_page_sizes(&mut self, cx: &mut Context<Self>) {
        let State::Ready(doc) = &self.state else {
            return;
        };
        let rx = doc.primary().page_sizes();
        let timing = self.timing;
        let t = Instant::now();
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.await else { return };
            this.update(cx, |viewer, cx| {
                let State::Ready(doc) = &mut viewer.state else {
                    return;
                };
                match result {
                    Ok(sizes) => {
                        if timing {
                            eprintln!(
                                "[timing] page sizes for {} pages in {:?}",
                                sizes.len(),
                                t.elapsed()
                            );
                        }
                        let primary = doc.model.primary();
                        doc.model.set_source_sizes(primary, sizes);
                        doc.touched();
                        cx.notify();
                    }
                    Err(e) => log::error!("page sizes: {e:#}"),
                }
            })
            .ok();
        })
        .detach();
    }

    fn fetch_outline(&mut self, cx: &mut Context<Self>) {
        let State::Ready(doc) = &self.state else {
            return;
        };
        let rx = doc.primary().outline();
        let timing = self.timing;
        let t = Instant::now();
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.await else { return };
            this.update(cx, |viewer, cx| {
                let State::Ready(doc) = &mut viewer.state else {
                    return;
                };
                match result {
                    Ok(items) => {
                        if timing {
                            eprintln!(
                                "[timing] outline: {} items in {:?}",
                                items.len(),
                                t.elapsed()
                            );
                        }
                        let flat: Vec<FlatOutlineItem> = items.iter().map(flat_item).collect();
                        match doc.model.set_initial_outline(&flat) {
                            Ok(()) => {
                                // Start with the top level expanded.
                                doc.outline_expanded =
                                    doc.model.state().outline.iter().map(|n| n.id).collect();
                                doc.outline_loaded = true;
                                doc.touched();
                            }
                            Err(e) => log::warn!("outline arrived after edits; ignored: {e:#}"),
                        }
                        cx.notify();
                    }
                    Err(e) => log::error!("outline: {e:#}"),
                }
            })
            .ok();
        })
        .detach();
    }

    pub fn go_to_page(&mut self, page: usize, cx: &mut Context<Self>) {
        self.scroll(ScrollTo::Page(page), cx);
    }

    // ----- edits ------------------------------------------------------------

    fn apply(&mut self, edit: Edit, label: &str, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        match doc.model.apply(edit, label) {
            Ok(out) => {
                let mut parts = Vec::new();
                if out.pages_removed > 0 {
                    parts.push(format!(
                        "{} page{}",
                        out.pages_removed,
                        if out.pages_removed == 1 { "" } else { "s" }
                    ));
                }
                if out.bookmarks_removed > 0 {
                    parts.push(format!(
                        "{} bookmark{}",
                        out.bookmarks_removed,
                        if out.bookmarks_removed == 1 { "" } else { "s" }
                    ));
                }
                doc.notice = Some(if parts.is_empty() {
                    label.to_string()
                } else {
                    format!("{label}: removed {} — Ctrl+Z to undo", parts.join(" and "))
                });
                doc.touched();
                self.prune_selection();
                cx.notify();
            }
            Err(e) => {
                doc.notice = Some(format!("{label} failed: {e:#}"));
                cx.notify();
            }
        }
    }

    /// Pages an operation applies to: the selection, or the current page.
    fn target_ids(&self) -> Vec<PageId> {
        let State::Ready(doc) = &self.state else {
            return Vec::new();
        };
        if doc.selection.is_empty() {
            return doc
                .page_ref(doc.current_page)
                .map(|p| vec![p.id])
                .unwrap_or_default();
        }
        // Keep document order so the label and any insertion are predictable.
        doc.model
            .state()
            .pages
            .iter()
            .filter(|p| doc.selection.contains(&p.id))
            .map(|p| p.id)
            .collect()
    }

    fn label(&self, verb: &str, n: usize) -> String {
        if n == 1 {
            format!("{verb} page")
        } else {
            format!("{verb} {n} pages")
        }
    }

    fn rotate_current(&mut self, by: Rotation, cx: &mut Context<Self>) {
        let ids = self.target_ids();
        if !ids.is_empty() {
            let label = self.label("Rotate", ids.len());
            self.apply(Edit::RotatePages { ids, by }, &label, cx);
        }
    }

    fn delete_current(&mut self, cx: &mut Context<Self>) {
        let ids = self.target_ids();
        if !ids.is_empty() {
            let label = self.label("Delete", ids.len());
            self.apply(Edit::DeletePages { ids }, &label, cx);
        }
    }

    fn duplicate_current(&mut self, cx: &mut Context<Self>) {
        let ids = self.target_ids();
        if !ids.is_empty() {
            let label = self.label("Duplicate", ids.len());
            self.apply(Edit::DuplicatePages { ids }, &label, cx);
        }
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        doc.selection = doc.model.state().pages.iter().map(|p| p.id).collect();
        cx.notify();
    }

    fn clear_selection(&mut self, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        if !doc.selection.is_empty() {
            doc.selection.clear();
            cx.notify();
        }
    }

    /// Drop pages that no longer exist (after delete/undo/redo).
    fn prune_selection(&mut self) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let live: HashSet<PageId> = doc.model.state().pages.iter().map(|p| p.id).collect();
        doc.selection.retain(|id| live.contains(id));
    }

    /// Thumbnail click: plain selects just that page, Ctrl toggles, Shift
    /// extends from the anchor. All of them navigate there.
    fn select_click(&mut self, index: usize, modifiers: gpui::Modifiers, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let refs: Vec<PageId> = doc.model.state().pages.iter().map(|p| p.id).collect();
        let Some(&id) = refs.get(index) else { return };
        if modifiers.shift {
            let anchor = doc
                .sel_anchor
                .unwrap_or(doc.current_page)
                .min(refs.len() - 1);
            let (a, b) = if anchor <= index {
                (anchor, index)
            } else {
                (index, anchor)
            };
            if !modifiers.control {
                doc.selection.clear();
            }
            doc.selection.extend(refs[a..=b].iter().copied());
        } else if modifiers.control {
            if !doc.selection.remove(&id) {
                doc.selection.insert(id);
            }
            doc.sel_anchor = Some(index);
        } else {
            doc.selection.clear();
            doc.selection.insert(id);
            doc.sel_anchor = Some(index);
        }
        self.go_to_page(index, cx);
    }

    /// Finish a thumbnail drag: move `ids` to the recorded insert index.
    fn drop_pages(&mut self, ids: Vec<PageId>, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let Some(at) = doc.drop_target.take() else {
            return;
        };
        let moving: HashSet<PageId> = ids.iter().copied().collect();
        // "Before" must be a page that isn't moving; walk forward past any that are.
        let before = doc
            .model
            .state()
            .pages
            .iter()
            .skip(at)
            .find(|p| !moving.contains(&p.id))
            .map(|p| p.id);
        let n = ids.len();
        let label = self.label("Move", n);
        self.apply(Edit::MovePages { ids, before }, &label, cx);
        if let State::Ready(doc) = &mut self.state {
            doc.selection = moving;
        }
    }

    fn undo(&mut self, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        doc.notice = match doc.model.undo() {
            Some(label) => Some(format!("Undid: {label}")),
            None => Some("Nothing to undo".into()),
        };
        doc.touched();
        self.prune_selection();
        cx.notify();
    }

    fn redo(&mut self, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        doc.notice = match doc.model.redo() {
            Some(label) => Some(format!("Redid: {label}")),
            None => Some("Nothing to redo".into()),
        };
        doc.touched();
        self.prune_selection();
        cx.notify();
    }

    // ----- save --------------------------------------------------------------

    /// Save As: ask for a path, then save there.
    fn save_as(&mut self, cx: &mut Context<Self>) {
        let dir = self
            .path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default();
        let suggested = self
            .path
            .file_name()
            .and_then(|s| s.to_str())
            .map(String::from);
        let rx = cx.prompt_for_new_path(&dir, suggested.as_deref());
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                this.update(cx, |view, cx| view.save_to(path, cx)).ok();
            }
        })
        .detach();
    }

    /// Save to `path` (the document's own path for plain Save). Runs the
    /// rewrite on a background thread from a snapshot, then reopens the saved
    /// file as the new primary source without disturbing undo history.
    pub fn save_to(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        if doc.saving {
            doc.notice = Some("Already saving…".into());
            cx.notify();
            return;
        }
        doc.saving = true;
        doc.notice = Some(format!("Saving {}…", path.display()));
        let snapshot = doc.model.snapshot();
        let timing = self.timing;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let t = Instant::now();
            let result = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    async move {
                        let strategy = save::save_to_path(&snapshot, &path)?;
                        let bytes = std::fs::metadata(&path)
                            .map(|m| m.len() as usize)
                            .unwrap_or(0);
                        let handle = RenderHandle::open(path.clone(), None)?;
                        anyhow::Ok((handle, bytes, strategy))
                    }
                })
                .await;
            this.update(cx, |view, cx| {
                let State::Ready(doc) = &mut view.state else {
                    return;
                };
                doc.saving = false;
                match result {
                    Ok((handle, bytes, strategy)) => {
                        let (page_count, first_page) =
                            (handle.info().page_count, handle.info().first_page);
                        let saved = SourceInfo {
                            path: path.clone(),
                            page_count,
                            first_page,
                            page_sizes: None,
                            bytes,
                        };
                        let sid = doc.model.rebase_to_saved(saved);
                        doc.handles.insert(sid, handle);
                        view.path = path.clone();
                        doc.notice = Some(format!(
                            "Saved {} ({strategy:?}) in {:?}",
                            path.display(),
                            t.elapsed()
                        ));
                        if timing {
                            eprintln!(
                                "[timing] saved {} ({} pages, {strategy:?}) in {:?}",
                                path.display(),
                                page_count,
                                t.elapsed()
                            );
                        }
                        doc.touched();
                        view.fetch_page_sizes(cx);
                    }
                    Err(e) => {
                        doc.notice = Some(format!("Save failed: {e:#}"));
                        log::error!("save: {e:#}");
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Dev hook: rotate the first page and save to `path`, so the save path
    /// can be exercised without a keyboard.
    pub fn selftest_save(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let State::Ready(doc) = &self.state else {
            return;
        };
        if let Some(p) = doc.page_ref(0) {
            self.apply(
                Edit::RotatePages {
                    ids: vec![p.id],
                    by: Rotation::R90,
                },
                "Rotate page",
                cx,
            );
        }
        self.save_to(path, cx);
    }

    pub fn set_selftest_save(&mut self, path: PathBuf) {
        self.selftest_save = Some(path);
    }

    // ----- search -----------------------------------------------------------

    fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.is_none() {
            let input = cx.new(|cx| TextInput::new("Find in document", cx));
            let sub = cx.observe(&input, |this, _, cx| this.on_search_input_changed(cx));
            self.search = Some(Search {
                input,
                _input_sub: sub,
                query: String::new(),
                hits: Vec::new(),
                current: None,
                scanned: 0,
                done: true,
                _task: None,
            });
        }
        let input = self.search.as_ref().unwrap().input.clone();
        input.update(cx, |i, cx| i.select_all(cx));
        window.focus(&input.read(cx).focus_handle(cx), cx);
        cx.notify();
    }

    /// `--find`: run a query as soon as the document is ready, with the bar open.
    pub fn set_initial_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.initial_query = Some(query);
        cx.notify();
    }

    fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search = None;
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    fn on_search_input_changed(&mut self, cx: &mut Context<Self>) {
        let Some(search) = &self.search else { return };
        let query = search.input.read(cx).text().to_string();
        if query == search.query {
            return;
        }
        self.start_search(query, cx);
    }

    fn start_search(&mut self, query: String, cx: &mut Context<Self>) {
        let State::Ready(doc) = &self.state else {
            return;
        };
        let handle = doc.primary().clone();
        let Some(search) = &mut self.search else {
            return;
        };
        search.query = query.clone();
        search.hits.clear();
        search.current = None;
        search.scanned = 0;
        search._task = None; // cancels the previous search
        if query.trim().is_empty() {
            search.done = true;
            cx.notify();
            return;
        }
        search.done = false;
        self.search_started = Some(Instant::now());
        let Some(search) = &mut self.search else {
            return;
        };
        let mut rx = handle.search(query, false);
        search._task = Some(cx.spawn(async move |this, cx| {
            while let Some(ev) = rx.next().await {
                let stop = this
                    .update(cx, |view, cx| {
                        let Some(search) = &mut view.search else {
                            return true;
                        };
                        match ev {
                            SearchEvent::Hits(mut hits) => {
                                let first = search.hits.is_empty();
                                search.hits.append(&mut hits);
                                if first {
                                    view.go_to_hit(0, cx);
                                }
                            }
                            SearchEvent::Progress(n) => search.scanned = n,
                            SearchEvent::Done => {
                                search.done = true;
                                if view.timing
                                    && let Some(t) = view.search_started.take()
                                {
                                    eprintln!(
                                        "[timing] search {:?}: {} hits across {} pages in {:?}",
                                        search.query,
                                        search.hits.len(),
                                        search.scanned,
                                        t.elapsed()
                                    );
                                }
                            }
                        }
                        cx.notify();
                        false
                    })
                    .unwrap_or(true);
                if stop {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn go_to_hit(&mut self, ix: usize, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let Some(search) = &mut self.search else {
            return;
        };
        if search.hits.is_empty() {
            return;
        }
        let ix = ix % search.hits.len();
        search.current = Some(ix);
        let hit = &search.hits[ix];
        let Some(index) = doc.index_of_source_page(hit.page) else {
            return;
        };
        let Some(pref) = doc.page_ref(index) else {
            return;
        };
        let src = doc
            .model
            .source(pref.source)
            .map(|s| s.page_size(pref.index))
            .unwrap_or(PageSize {
                width: 612.0,
                height: 792.0,
            });
        if let Some(r) = hit.rects.first() {
            let r = rotate_rect(*r, src, pref.rotation);
            let ps = src.rotated(pref.rotation);
            doc.pending_scroll = Some(ScrollTo::Reveal {
                page: index,
                fx: (r.x + r.w / 2.0) / ps.width.max(1.0),
                fy: (r.y + r.h / 2.0) / ps.height.max(1.0),
            });
        }
        cx.notify();
    }

    fn search_step(&mut self, delta: i64, cx: &mut Context<Self>) {
        let Some(search) = &self.search else { return };
        let n = search.hits.len();
        if n == 0 {
            return;
        }
        let cur = search.current.unwrap_or(0) as i64;
        let next = (cur + delta).rem_euclid(n as i64) as usize;
        self.go_to_hit(next, cx);
    }

    fn render_search_bar(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let search = self.search.as_ref()?;
        let total = search.hits.len();
        let status = if search.query.trim().is_empty() {
            String::new()
        } else if total == 0 {
            if search.done {
                "No matches".to_string()
            } else {
                format!("Searching… {}", search.scanned)
            }
        } else {
            let cur = search.current.map(|c| c + 1).unwrap_or(0);
            if search.done {
                format!("{cur} of {total}")
            } else {
                format!("{cur} of {total}+ (searching…)")
            }
        };
        let btn = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px_2()
                .py_1()
                .rounded_sm()
                .text_sm()
                .cursor_pointer()
                .bg(rgb(0x3a3a3a))
                .hover(|s| s.bg(rgb(0x4a4a4a)))
                .child(label)
        };
        Some(
            div()
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .bg(rgb(0x252525))
                .border_b_1()
                .border_color(rgb(0x111111))
                .child(div().w(px(320.0)).child(search.input.clone()))
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(0x9a9a9a))
                        .min_w(px(140.0))
                        .child(status),
                )
                .child(
                    btn("search-prev", "▲")
                        .on_click(cx.listener(|v, _, _, cx| v.search_step(-1, cx))),
                )
                .child(
                    btn("search-next", "▼")
                        .on_click(cx.listener(|v, _, _, cx| v.search_step(1, cx))),
                )
                .child(
                    btn("search-close", "×")
                        .on_click(cx.listener(|v, _, window, cx| v.close_search(window, cx))),
                )
                .into_any_element(),
        )
    }

    // ----- scrolling and zoom ------------------------------------------------

    fn scroll(&mut self, to: ScrollTo, cx: &mut Context<Self>) {
        if let State::Ready(doc) = &mut self.state {
            doc.pending_scroll = Some(to);
            cx.notify();
        }
    }

    /// Change zoom, keeping the document point under `anchor` (viewport-local
    /// logical px; defaults to the viewport centre) where it is.
    fn set_zoom(&mut self, zoom: Zoom, anchor: Option<(f32, f32)>, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        if doc.zoom == zoom {
            return;
        }
        if let Some(layout) = &doc.layout
            && layout.len() > 0
        {
            let (vx, vy) = anchor.unwrap_or((layout.view_w / 2.0, layout.view_h / 2.0));
            let sx = -f32::from(doc.scroll.offset().x);
            let sy = -f32::from(doc.scroll.offset().y);
            let (dx, dy) = (sx + vx, sy + vy);
            let page = layout.page_at(dy);
            let fx = (dx - layout.lefts[page]) / layout.widths[page];
            let fy = (dy - layout.tops[page]) / layout.heights[page];
            doc.pending_scroll = Some(ScrollTo::Anchor {
                page,
                fx,
                fy,
                vx,
                vy,
            });
        }
        doc.zoom = zoom;
        cx.notify();
    }

    fn zoom_by(&mut self, factor: f32, anchor: Option<(f32, f32)>, cx: &mut Context<Self>) {
        let State::Ready(doc) = &self.state else {
            return;
        };
        let current = doc
            .layout
            .as_ref()
            .map(|l| l.zoom_percent() / 100.0)
            .unwrap_or(1.0);
        let z = (current * factor).clamp(ZOOM_MIN, ZOOM_MAX);
        self.set_zoom(Zoom::Custom(z), anchor, cx);
    }

    fn on_wheel(&mut self, ev: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !ev.modifiers.control {
            return;
        }
        let State::Ready(doc) = &self.state else {
            return;
        };
        let dy = f32::from(ev.delta.pixel_delta(window.line_height()).y);
        if dy == 0.0 {
            return;
        }
        // Anchor on the mouse: position relative to the scroll container.
        let b = doc.scroll.bounds();
        let anchor = (
            f32::from(ev.position.x - b.origin.x),
            f32::from(ev.position.y - b.origin.y),
        );
        let factor = if dy > 0.0 { 1.1 } else { 1.0 / 1.1 };
        self.zoom_by(factor, Some(anchor), cx);
        cx.stop_propagation();
    }

    // ----- rendering pipeline -------------------------------------------------

    fn ensure_rendered(&mut self, pref: PageRef, want: RenderRequest, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let key = (pref.id, want.job);
        if doc.tiles.contains_key(&key) || doc.inflight.contains_key(&key) {
            return;
        }
        if let Job::Page { rotation, .. } = want.job
            && doc
                .previews
                .get(&pref.id)
                .is_some_and(|(r, _)| *r == rotation)
        {
            return;
        }
        let Some(handle) = doc.handles.get(&pref.source) else {
            return;
        };
        let rx = handle.render(want);
        let task = cx.spawn(async move |this, cx| {
            let result = rx.await;
            this.update(cx, |viewer, cx| {
                viewer.on_rendered(pref.id, want, result, cx)
            })
            .ok();
        });
        doc.inflight.insert(key, task);
    }

    fn on_rendered(
        &mut self,
        id: PageId,
        want: RenderRequest,
        result: Result<anyhow::Result<Rendered>, futures::channel::oneshot::Canceled>,
        cx: &mut Context<Self>,
    ) {
        let timing = self.timing;
        let launched = self.launched;
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let key = (id, want.job);
        // We're running inside this task; detach rather than drop it.
        if let Some(task) = doc.inflight.remove(&key) {
            task.detach();
        }
        match result {
            Err(_canceled) => {} // superseded or scrolled away
            Ok(Err(e)) => log::error!("render {:?}: {e:#}", want),
            Ok(Ok(rendered)) => {
                if timing {
                    let kind = match want.job {
                        Job::Page { .. } => "preview",
                        Job::Tile { .. } => "tile",
                    };
                    eprintln!(
                        "[timing] page {} {kind} {}x{} rendered in {:?}",
                        want.page.0,
                        rendered.raster.width,
                        rendered.raster.height,
                        rendered.render_time
                    );
                    if doc.first_paint.is_none() {
                        eprintln!(
                            "[timing] first pixels ready {:?} since launch",
                            launched.elapsed()
                        );
                    }
                }
                doc.first_paint.get_or_insert_with(Instant::now);
                let image = Arc::new(to_render_image(rendered.raster));
                let old = match want.job {
                    Job::Page { rotation, .. } => {
                        doc.previews.insert(id, (rotation, image)).map(|(_, im)| im)
                    }
                    Job::Tile { .. } => doc.tiles.insert(key, image),
                };
                if let Some(old) = old {
                    doc.retired.push(old);
                }
                cx.notify();
            }
        }
    }

    pub fn current_page(&self) -> usize {
        match &self.state {
            State::Ready(doc) => doc.current_page,
            _ => 0,
        }
    }
}

fn flat_item(i: &OutlineItem) -> FlatOutlineItem {
    FlatOutlineItem {
        title: i.title.clone(),
        page: i.page,
        depth: i.depth,
    }
}

/// Wrap a BGRA raster as a GPUI image without copying the pixel data.
/// `RenderImage` is documented as BGRA; `RgbaImage` here is just the container.
fn to_render_image(raster: sheaf_render::Raster) -> RenderImage {
    debug_assert_eq!(raster.format, PixelFormat::Bgra);
    let buf = image::RgbaImage::from_raw(raster.width, raster.height, raster.data)
        .expect("raster size matches buffer");
    RenderImage::new(smallvec::smallvec![image::Frame::new(buf)])
}

/// Map a rect in a page's source orientation (size `src`) to where it lands
/// after rotating the page clockwise by `rot`.
fn rotate_rect(r: Rect, src: PageSize, rot: Rotation) -> Rect {
    match rot {
        Rotation::R0 => r,
        Rotation::R90 => Rect {
            x: src.height - (r.y + r.h),
            y: r.x,
            w: r.h,
            h: r.w,
        },
        Rotation::R180 => Rect {
            x: src.width - (r.x + r.w),
            y: src.height - (r.y + r.h),
            w: r.w,
            h: r.h,
        },
        Rotation::R270 => Rect {
            x: r.y,
            y: src.width - (r.x + r.w),
            w: r.h,
            h: r.w,
        },
    }
}

impl Focusable for DocView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// Everything `render` decides for one page: where it goes and which tiles
/// cover the visible part of it.
struct PagePlan {
    pref: PageRef,
    left: f32,
    top: f32,
    w: f32,
    h: f32,
    dev_w: u32,
    /// Tiles intersecting the (overdrawn) viewport, with priority.
    tiles: Vec<(Job, u8)>,
}

impl Render for DocView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let scale = window.scale_factor();

        // Release textures for images we've replaced or evicted since last frame.
        if let State::Ready(doc) = &mut self.state {
            for old in doc.retired.drain(..) {
                if let Err(e) = window.drop_image(old) {
                    log::warn!("drop_image: {e:#}");
                }
            }
        }

        let search_bar = self.render_search_bar(cx);
        let mut body = None;
        let mut wants: Vec<(PageRef, RenderRequest)> = Vec::new();
        let status: SharedString = match &mut self.state {
            State::Loading => "Opening…".into(),
            State::Failed(e) => format!("Failed: {e}").into(),
            State::Ready(doc) => {
                // The scroll container's bounds lag a frame behind on first
                // layout; fall back to the window size until we have them.
                let bounds = doc.scroll.bounds();
                let bounds_known = f32::from(bounds.size.width) > 0.0;
                let (view_w, view_h) = if bounds_known {
                    (f32::from(bounds.size.width), f32::from(bounds.size.height))
                } else {
                    (
                        f32::from(viewport.width),
                        f32::from(viewport.height) - STATUS_H,
                    )
                };
                let zoom = doc.zoom;

                let stale = doc.layout.as_ref().is_none_or(|l| {
                    l.zoom != zoom
                        || l.version != doc.version
                        || (l.view_w - view_w).abs() > 0.5
                        || (l.view_h - view_h).abs() > 0.5
                });
                if stale {
                    let refs: Vec<PageRef> = doc.model.state().pages.iter().copied().collect();
                    doc.layout = Some(Layout::build(
                        zoom,
                        view_w,
                        view_h,
                        doc.version,
                        refs,
                        &doc.model,
                    ));
                }
                let layout = doc.layout.as_ref().expect("built above");
                let count = layout.len();

                // Apply any queued scroll now that we know geometry.
                let max_x = (layout.total_w - view_w).max(0.0);
                let max_y = (layout.total_h - view_h).max(0.0);
                let mut scroll_x = (-f32::from(doc.scroll.offset().x)).clamp(0.0, max_x);
                let mut scroll_y = (-f32::from(doc.scroll.offset().y)).clamp(0.0, max_y);
                if let Some(to) = doc.pending_scroll.take()
                    && count > 0
                {
                    let last = count - 1;
                    match to {
                        ScrollTo::Page(p) => scroll_y = layout.tops[p.min(last)] - MARGIN,
                        ScrollTo::By(dy) => scroll_y += dy,
                        ScrollTo::End => scroll_y = max_y,
                        ScrollTo::Reveal { page, fx, fy } => {
                            let p = page.min(last);
                            let (x, y) = (
                                layout.lefts[p] + fx * layout.widths[p],
                                layout.tops[p] + fy * layout.heights[p],
                            );
                            if y < scroll_y + 8.0 || y > scroll_y + view_h - 8.0 {
                                scroll_y = y - view_h / 3.0;
                            }
                            if x < scroll_x || x > scroll_x + view_w {
                                scroll_x = x - view_w / 2.0;
                            }
                        }
                        ScrollTo::Anchor {
                            page,
                            fx,
                            fy,
                            vx,
                            vy,
                        } => {
                            let p = page.min(last);
                            scroll_x = layout.lefts[p] + fx * layout.widths[p] - vx;
                            scroll_y = layout.tops[p] + fy * layout.heights[p] - vy;
                        }
                    }
                    scroll_x = scroll_x.clamp(0.0, max_x);
                    scroll_y = scroll_y.clamp(0.0, max_y);
                    doc.scroll.set_offset(point(px(-scroll_x), px(-scroll_y)));
                }

                let mut plans: Vec<PagePlan> = Vec::new();
                let mut keep: HashSet<PageId> = HashSet::new();
                if count > 0 {
                    let last = count - 1;
                    let visible = layout.visible(scroll_y, scroll_y + view_h);
                    doc.current_page = layout.page_at(scroll_y + view_h * 0.5).min(last);

                    // Plan tiles for pages near the viewport.
                    let overdraw = view_h * TILE_OVERDRAW;
                    let fetch = visible.start().saturating_sub(PREFETCH)
                        ..=(visible.end() + PREFETCH).min(last);
                    for i in visible.start().saturating_sub(KEEP)..=(visible.end() + KEEP).min(last)
                    {
                        keep.insert(layout.refs[i].id);
                    }
                    for i in fetch {
                        let pref = layout.refs[i];
                        let (left, top, w, h) = (
                            layout.lefts[i],
                            layout.tops[i],
                            layout.widths[i],
                            layout.heights[i],
                        );
                        let dev_w = (w * scale).round().max(1.0) as u32;
                        let dev_h = (h * scale).round().max(1.0) as u32;
                        // Priorities: visible previews, visible tiles, then the
                        // same for prefetch pages — so an expensive off-screen
                        // preview never delays on-screen tiles.
                        let on_screen_page = visible.contains(&i);
                        wants.push((
                            pref,
                            RenderRequest {
                                page: PageIndex(pref.index),
                                job: Job::Page {
                                    width: PREVIEW_W,
                                    rotation: pref.rotation,
                                },
                                format: PixelFormat::Bgra,
                                priority: if on_screen_page { 0 } else { 2 },
                            },
                        ));
                        // Visible part of this page, in page-local device px.
                        let vis_x0 = ((scroll_x - left) * scale).max(0.0);
                        let vis_x1 = ((scroll_x + view_w - left) * scale).min(dev_w as f32);
                        let vis_y0 = ((scroll_y - overdraw - top) * scale).max(0.0);
                        let vis_y1 =
                            ((scroll_y + view_h + overdraw - top) * scale).min(dev_h as f32);
                        let strict_y0 = ((scroll_y - top) * scale).max(0.0);
                        let strict_y1 = ((scroll_y + view_h - top) * scale).min(dev_h as f32);
                        let mut tiles = Vec::new();
                        // Until the container has been laid out once, the width
                        // is a guess; don't rasterize tiles at the wrong zoom.
                        if bounds_known && vis_x1 > vis_x0 && vis_y1 > vis_y0 {
                            let (c0, c1) = (
                                (vis_x0 / TILE as f32) as u32,
                                ((vis_x1 / TILE as f32).ceil() as u32).max(1),
                            );
                            let (r0, r1) = (
                                (vis_y0 / TILE as f32) as u32,
                                ((vis_y1 / TILE as f32).ceil() as u32).max(1),
                            );
                            for row in r0..r1 {
                                for col in c0..c1 {
                                    let (x, y) = (col * TILE, row * TILE);
                                    let (tw, th) = (TILE.min(dev_w - x), TILE.min(dev_h - y));
                                    let on_screen =
                                        (y as f32) < strict_y1 && ((y + th) as f32) > strict_y0;
                                    tiles.push((
                                        Job::Tile {
                                            page_width: dev_w,
                                            page_height: dev_h,
                                            x,
                                            y,
                                            w: tw,
                                            h: th,
                                            rotation: pref.rotation,
                                        },
                                        if on_screen { 1 } else { 3 },
                                    ));
                                }
                            }
                        }
                        for (job, priority) in &tiles {
                            wants.push((
                                pref,
                                RenderRequest {
                                    page: PageIndex(pref.index),
                                    job: *job,
                                    format: PixelFormat::Bgra,
                                    priority: *priority,
                                },
                            ));
                        }
                        plans.push(PagePlan {
                            pref,
                            left,
                            top,
                            w,
                            h,
                            dev_w,
                            tiles,
                        });
                    }
                } else {
                    doc.current_page = 0;
                }

                // Evict: anything outside the keep set; and, per page, tiles at
                // a stale zoom or rotation once every needed tile at the current
                // zoom is in (stale rotation goes immediately).
                let wanted: HashSet<TileKey> = plans
                    .iter()
                    .flat_map(|pl| pl.tiles.iter().map(move |(j, _)| (pl.pref.id, *j)))
                    .collect();
                let mut complete: HashMap<PageId, bool> = HashMap::new();
                let mut rotation_of: HashMap<PageId, Rotation> = HashMap::new();
                for pl in &plans {
                    complete.insert(
                        pl.pref.id,
                        pl.tiles
                            .iter()
                            .all(|(j, _)| doc.tiles.contains_key(&(pl.pref.id, *j))),
                    );
                    rotation_of.insert(pl.pref.id, pl.pref.rotation);
                }
                let evict: Vec<TileKey> = doc
                    .tiles
                    .keys()
                    .copied()
                    .filter(|k @ (p, j)| {
                        !keep.contains(p)
                            || rotation_of.get(p).is_some_and(|r| *r != j.rotation())
                            || (!wanted.contains(k) && complete.get(p).copied().unwrap_or(true))
                    })
                    .collect();
                for k in evict {
                    if let Some(im) = doc.tiles.remove(&k) {
                        doc.retired.push(im);
                    }
                }
                let thumb_keep = doc.thumb_keep.clone();
                let evict: Vec<PageId> = doc
                    .previews
                    .keys()
                    .copied()
                    .filter(|p| !keep.contains(p) && !thumb_keep.contains(p))
                    .collect();
                for p in evict {
                    if let Some((_, im)) = doc.previews.remove(&p) {
                        doc.retired.push(im);
                    }
                }
                doc.inflight.retain(|k, _| {
                    wanted.contains(k)
                        || matches!(k.1, Job::Page { .. })
                            && (keep.contains(&k.0) || thumb_keep.contains(&k.0))
                });

                // Build elements: preview underneath, then any tiles we have
                // for the page (stale-zoom ones scaled to fit, current on top).
                let primary = doc.model.primary();
                let pages: Vec<_> = plans
                    .iter()
                    .map(|pl| {
                        let mut page_div = div()
                            .absolute()
                            .top(px(pl.top))
                            .left(px(pl.left))
                            .w(px(pl.w))
                            .h(px(pl.h))
                            .bg(rgb(0xffffff))
                            .shadow_md();
                        if let Some((rot, prev)) = doc.previews.get(&pl.pref.id)
                            && *rot == pl.pref.rotation
                        {
                            page_div =
                                page_div.child(div().absolute().inset_0().child(
                                    img(prev.clone()).size_full().object_fit(ObjectFit::Fill),
                                ));
                        }
                        let mut page_tiles: Vec<(&TileKey, &Arc<RenderImage>)> = doc
                            .tiles
                            .iter()
                            .filter(|((p, _), _)| *p == pl.pref.id)
                            .collect();
                        // Stale zoom levels first so current ones paint over them.
                        page_tiles.sort_by_key(|((_, j), _)| match j {
                            Job::Tile { page_width, .. } => *page_width == pl.dev_w,
                            _ => false,
                        });
                        for ((_, job), im) in page_tiles {
                            let Job::Tile {
                                page_width,
                                page_height,
                                x,
                                y,
                                w,
                                h,
                                ..
                            } = job
                            else {
                                continue;
                            };
                            // Logical px per device px of the zoom this tile was rendered at.
                            let sx = pl.w / *page_width as f32;
                            let sy = pl.h / *page_height as f32;
                            page_div = page_div.child(
                                div()
                                    .absolute()
                                    .left(px(*x as f32 * sx))
                                    .top(px(*y as f32 * sy))
                                    .w(px(*w as f32 * sx))
                                    .h(px(*h as f32 * sy))
                                    .child(img(im.clone()).size_full().object_fit(ObjectFit::Fill)),
                            );
                        }
                        // Search highlights on top of everything. Hits are in
                        // the primary source's page coordinates.
                        if let Some(search) = &self.search
                            && !search.hits.is_empty()
                            && pl.pref.source == primary
                        {
                            let src = doc
                                .model
                                .source(primary)
                                .map(|s| s.page_size(pl.pref.index))
                                .unwrap_or(PageSize {
                                    width: 612.0,
                                    height: 792.0,
                                });
                            let k = pl.w / src.rotated(pl.pref.rotation).width.max(1.0);
                            let start = search.hits.partition_point(|h| h.page < pl.pref.index);
                            for (ix, hit) in search.hits.iter().enumerate().skip(start) {
                                if hit.page != pl.pref.index {
                                    break;
                                }
                                let current = search.current == Some(ix);
                                for r in &hit.rects {
                                    let r = rotate_rect(*r, src, pl.pref.rotation);
                                    page_div = page_div.child(
                                        div()
                                            .absolute()
                                            .left(px(r.x * k))
                                            .top(px(r.y * k))
                                            .w(px(r.w * k))
                                            .h(px(r.h * k))
                                            .bg(if current {
                                                rgba(0xff9800a0)
                                            } else {
                                                rgba(0xffe60070)
                                            }),
                                    );
                                }
                            }
                        }
                        page_div
                    })
                    .collect();

                body = Some(
                    div()
                        .id("pages")
                        .track_scroll(&doc.scroll)
                        .overflow_scroll()
                        .size_full()
                        .on_scroll_wheel(
                            cx.listener(|v, ev, window, cx| v.on_wheel(ev, window, cx)),
                        )
                        .child(
                            div()
                                .relative()
                                .w(px(layout.total_w))
                                .h(px(layout.total_h))
                                .children(pages),
                        ),
                );

                let mode = match zoom {
                    Zoom::FitWidth => "fit width",
                    Zoom::FitPage => "fit page",
                    Zoom::Custom(_) => "custom",
                };
                let ps = doc
                    .page_ref(doc.current_page)
                    .map(|p| doc.model.page_size(&p));
                let size = ps
                    .map(|s| format!("{:.0}×{:.0} pt", s.width, s.height))
                    .unwrap_or_default();
                let notice = doc
                    .notice
                    .as_deref()
                    .map(|n| format!("  —  {n}"))
                    .unwrap_or_default();
                format!(
                    "page {} / {}  —  {size}  —  {:.0}% ({mode}){notice}",
                    (doc.current_page + 1).min(count),
                    count,
                    layout.zoom_percent(),
                )
                .into()
            }
        };
        for (pref, want) in wants {
            self.ensure_rendered(pref, want, cx);
        }

        div()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|v, _: &NextPage, _, cx| {
                v.scroll(ScrollTo::Page(v.current_page() + 1), cx)
            }))
            .on_action(cx.listener(|v, _: &PrevPage, _, cx| {
                v.scroll(ScrollTo::Page(v.current_page().saturating_sub(1)), cx)
            }))
            .on_action(cx.listener(|v, _: &FirstPage, _, cx| v.scroll(ScrollTo::Page(0), cx)))
            .on_action(cx.listener(|v, _: &LastPage, _, cx| v.scroll(ScrollTo::End, cx)))
            .on_action(cx.listener(|v, _: &LineDown, _, cx| v.scroll(ScrollTo::By(LINE_STEP), cx)))
            .on_action(cx.listener(|v, _: &LineUp, _, cx| v.scroll(ScrollTo::By(-LINE_STEP), cx)))
            .on_action(cx.listener(|v, _: &ScreenDown, window, cx| {
                let h = f32::from(window.viewport_size().height) - STATUS_H;
                v.scroll(ScrollTo::By(h * 0.9), cx)
            }))
            .on_action(cx.listener(|v, _: &ScreenUp, window, cx| {
                let h = f32::from(window.viewport_size().height) - STATUS_H;
                v.scroll(ScrollTo::By(-h * 0.9), cx)
            }))
            .on_action(cx.listener(|v, _: &ZoomIn, _, cx| v.zoom_by(ZOOM_STEP, None, cx)))
            .on_action(cx.listener(|v, _: &ZoomOut, _, cx| v.zoom_by(1.0 / ZOOM_STEP, None, cx)))
            .on_action(
                cx.listener(|v, _: &ZoomActual, _, cx| v.set_zoom(Zoom::Custom(1.0), None, cx)),
            )
            .on_action(
                cx.listener(|v, _: &ZoomFitWidth, _, cx| v.set_zoom(Zoom::FitWidth, None, cx)),
            )
            .on_action(cx.listener(|v, _: &ZoomFitPage, _, cx| v.set_zoom(Zoom::FitPage, None, cx)))
            .on_action(cx.listener(|v, _: &Find, window, cx| v.open_search(window, cx)))
            .on_action(cx.listener(|v, _: &CloseSearch, window, cx| {
                if v.search.is_some() {
                    v.close_search(window, cx)
                } else {
                    v.clear_selection(cx)
                }
            }))
            .on_action(cx.listener(|v, _: &SearchNext, _, cx| v.search_step(1, cx)))
            .on_action(cx.listener(|v, _: &SearchPrev, _, cx| v.search_step(-1, cx)))
            .on_action(
                cx.listener(|v, _: &RotatePageCw, _, cx| v.rotate_current(Rotation::R90, cx)),
            )
            .on_action(
                cx.listener(|v, _: &RotatePageCcw, _, cx| v.rotate_current(Rotation::R270, cx)),
            )
            .on_action(cx.listener(|v, _: &DeletePage, _, cx| v.delete_current(cx)))
            .on_action(cx.listener(|v, _: &DuplicatePages, _, cx| v.duplicate_current(cx)))
            .on_action(cx.listener(|v, _: &SelectAllPages, _, cx| v.select_all(cx)))
            .on_action(cx.listener(|v, _: &Save, _, cx| {
                let path = v.path.clone();
                v.save_to(path, cx)
            }))
            .on_action(cx.listener(|v, _: &SaveAs, _, cx| v.save_as(cx)))
            .on_action(cx.listener(|v, _: &Undo, _, cx| v.undo(cx)))
            .on_action(cx.listener(|v, _: &Redo, _, cx| v.redo(cx)))
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x2b2b2b))
            .text_color(rgb(0xd0d0d0))
            .children(search_bar)
            .child(div().flex_grow(1.0).min_h(px(0.0)).children(body))
            .child(
                div()
                    .h(px(STATUS_H))
                    .flex_shrink_0()
                    .px_2()
                    .flex()
                    .items_center()
                    .bg(rgb(0x1e1e1e))
                    .text_sm()
                    .child(status),
            )
    }
}

// ---------------------------------------------------------------------------
// Sidebar panels. Rendered on behalf of the workspace via `update`, so they
// can share this view's caches and render handles.

impl DocView {
    /// Thumbnail strip: virtualized, follows the current page, and is the
    /// page-organizer surface: click/Ctrl/Shift to select, drag to reorder.
    pub fn render_thumbnails(
        &mut self,
        panel_w: f32,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let State::Ready(doc) = &mut self.state else {
            return div().p_2().text_sm().child("…").into_any_element();
        };
        if !cx.has_active_drag() {
            doc.drop_target = None;
        }
        let refs: Vec<PageRef> = doc.model.state().pages.iter().copied().collect();
        let count = refs.len();
        if count == 0 {
            doc.thumb_keep.clear();
            return div()
                .p_2()
                .text_sm()
                .text_color(rgb(0x9a9a9a))
                .child("No pages")
                .into_any_element();
        }
        let last = count - 1;
        let thumb_w = (panel_w - 2.0 * THUMB_PAD).clamp(40.0, THUMB_W);

        // Item geometry. Cumulative tops are cheap enough per frame.
        let sizes: Vec<PageSize> = refs.iter().map(|r| doc.model.page_size(r)).collect();
        let item_h = |i: usize| {
            thumb_w * sizes[i].height / sizes[i].width.max(1.0) + THUMB_LABEL_H + THUMB_PAD
        };
        let mut tops = Vec::with_capacity(count);
        let mut y = THUMB_PAD;
        for i in 0..count {
            tops.push(y);
            y += item_h(i);
        }
        let total_h = y;
        // Midpoints decide which gap a drag is over.
        let mids: Arc<Vec<f32>> = Arc::new(
            (0..count)
                .map(|i| tops[i] + (item_h(i) - THUMB_PAD) / 2.0)
                .collect(),
        );

        let bounds = doc.thumb_scroll.bounds();
        let view_h = f32::from(bounds.size.height).max(1.0);
        let max_y = (total_h - view_h).max(0.0);
        let mut scroll_y = (-f32::from(doc.thumb_scroll.offset().y)).clamp(0.0, max_y);

        // Follow the main view: when the current page changes, make sure its
        // thumbnail is visible.
        let cur = doc.current_page.min(last);
        let cur_id = refs[cur].id;
        if doc.thumb_followed != Some(cur_id) {
            let top = tops[cur];
            let bottom = top + item_h(cur);
            if top < scroll_y {
                scroll_y = (top - THUMB_PAD).clamp(0.0, max_y);
            } else if bottom > scroll_y + view_h {
                scroll_y = (bottom - view_h + THUMB_PAD).clamp(0.0, max_y);
            }
            doc.thumb_scroll.set_offset(point(px(0.0), px(-scroll_y)));
            doc.thumb_followed = Some(cur_id);
        }

        let page_at = |y: f32| tops.partition_point(|&t| t <= y).saturating_sub(1);
        let (v0, v1) = (
            page_at(scroll_y).min(last),
            page_at(scroll_y + view_h).min(last),
        );
        doc.thumb_keep = (v0.saturating_sub(2)..=(v1 + 2).min(last))
            .map(|i| refs[i].id)
            .collect();

        // Pages that are bookmark targets get a marker (a delete would cascade).
        let targets: HashSet<PageId> = doc
            .model
            .state()
            .outline_flat()
            .iter()
            .filter_map(|(_, n)| n.target)
            .collect();
        let selection = doc.selection.clone();

        let mut wants = Vec::new();
        let mut items = Vec::new();
        for (i, pref) in refs.iter().enumerate().take(v1 + 1).skip(v0) {
            wants.push((
                *pref,
                RenderRequest {
                    page: PageIndex(pref.index),
                    job: Job::Page {
                        width: PREVIEW_W,
                        rotation: pref.rotation,
                    },
                    format: PixelFormat::Bgra,
                    priority: 2,
                },
            ));
            let h = thumb_w * sizes[i].height / sizes[i].width.max(1.0);
            let image = doc
                .previews
                .get(&pref.id)
                .filter(|(r, _)| *r == pref.rotation)
                .map(|(_, im)| im.clone());
            let current = i == cur;
            let selected = selection.contains(&pref.id);
            let bookmarked = targets.contains(&pref.id);
            // What a drag from this thumb carries: the selection if it's part
            // of it, otherwise just this page.
            let drag_ids: Vec<PageId> = if selected {
                refs.iter()
                    .filter(|p| selection.contains(&p.id))
                    .map(|p| p.id)
                    .collect()
            } else {
                vec![pref.id]
            };
            items.push(
                div()
                    .id(("thumb", pref.id.0 as usize))
                    .absolute()
                    .top(px(tops[i]))
                    .left(px(THUMB_PAD))
                    .w(px(thumb_w))
                    .flex()
                    .flex_col()
                    .items_center()
                    .cursor_pointer()
                    .on_click(cx.listener(move |v, ev: &gpui::ClickEvent, _, cx| {
                        v.select_click(i, ev.modifiers(), cx)
                    }))
                    .on_drag(DragPages { ids: drag_ids }, |d: &DragPages, _, _, cx| {
                        let n = d.ids.len();
                        cx.new(|_| DragPreview { count: n })
                    })
                    .child(
                        div()
                            .relative()
                            .w(px(thumb_w))
                            .h(px(h))
                            .bg(rgb(0xffffff))
                            .border_2()
                            .border_color(if current {
                                rgb(0x4a9eff)
                            } else if selected {
                                rgb(0x2f6fbf)
                            } else {
                                rgb(0x3a3a3a)
                            })
                            .when_some(image, |d, im| {
                                d.child(img(im).size_full().object_fit(ObjectFit::Fill))
                            })
                            .when(selected, |d| {
                                d.child(div().absolute().inset_0().bg(rgba(0x4a9eff30)))
                            })
                            .when(bookmarked, |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .top(px(0.0))
                                        .right(px(4.0))
                                        .w(px(10.0))
                                        .h(px(14.0))
                                        .bg(rgb(0xe0a000)),
                                )
                            }),
                    )
                    .child(
                        div()
                            .h(px(THUMB_LABEL_H))
                            .text_xs()
                            .text_color(if current || selected {
                                rgb(0xffffff)
                            } else {
                                rgb(0x9a9a9a)
                            })
                            .child(format!("{}", i + 1)),
                    ),
            );
        }

        // Drop indicator: a line in the gap the drag is over.
        let indicator = doc.drop_target.map(|at| {
            let y = if at >= count {
                tops[last] + item_h(last) - THUMB_PAD / 2.0
            } else {
                tops[at] - THUMB_PAD / 2.0
            };
            div()
                .absolute()
                .left(px(THUMB_PAD / 2.0))
                .top(px(y - 1.0))
                .w(px(thumb_w + THUMB_PAD))
                .h(px(2.0))
                .bg(rgb(0x4a9eff))
        });

        let mids_for_move = mids.clone();
        let el = div()
            .id("thumbs")
            .track_scroll(&doc.thumb_scroll)
            .overflow_y_scroll()
            .size_full()
            .on_drag_move(cx.listener(move |v, ev: &DragMoveEvent<DragPages>, _, cx| {
                let State::Ready(doc) = &mut v.state else {
                    return;
                };
                let local_y = f32::from(ev.event.position.y - ev.bounds.origin.y)
                    - f32::from(doc.thumb_scroll.offset().y);
                let at = mids_for_move.partition_point(|&m| m <= local_y);
                if doc.drop_target != Some(at) {
                    doc.drop_target = Some(at);
                    cx.notify();
                }
            }))
            .on_drop(cx.listener(|v, d: &DragPages, _, cx| v.drop_pages(d.ids.clone(), cx)))
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(total_h))
                    .children(items)
                    .children(indicator),
            )
            .into_any_element();
        for (pref, want) in wants {
            self.ensure_rendered(pref, want, cx);
        }
        el
    }

    /// Bookmark tree: virtualized rows over the expanded subset.
    pub fn render_outline(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let State::Ready(doc) = &mut self.state else {
            return div().p_2().text_sm().child("…").into_any_element();
        };
        if !doc.outline_loaded {
            return div()
                .p_2()
                .text_sm()
                .text_color(rgb(0x9a9a9a))
                .child("Loading bookmarks…")
                .into_any_element();
        }
        let flat = doc.model.state().outline_flat();
        if flat.is_empty() {
            return div()
                .p_2()
                .text_sm()
                .text_color(rgb(0x9a9a9a))
                .child("No bookmarks")
                .into_any_element();
        }

        // Flatten to the visible rows: skip descendants of collapsed nodes.
        let mut rows: Vec<usize> = Vec::new();
        let mut hide_below: Option<u16> = None;
        for (ix, (depth, node)) in flat.iter().enumerate() {
            if let Some(d) = hide_below {
                if *depth > d {
                    continue;
                }
                hide_below = None;
            }
            rows.push(ix);
            if !node.children.is_empty() && !doc.outline_expanded.contains(&node.id) {
                hide_below = Some(*depth);
            }
        }
        let total_h = rows.len() as f32 * OUTLINE_ROW_H + THUMB_PAD;
        let bounds = doc.outline_scroll.bounds();
        let view_h = f32::from(bounds.size.height).max(1.0);
        let scroll_y = (-f32::from(doc.outline_scroll.offset().y)).max(0.0);
        let r0 = ((scroll_y / OUTLINE_ROW_H) as usize).min(rows.len());
        let r1 = (((scroll_y + view_h) / OUTLINE_ROW_H) as usize + 1).min(rows.len());

        let cur_id = doc.page_ref(doc.current_page).map(|p| p.id);
        let positions: HashMap<PageId, usize> = doc
            .model
            .state()
            .pages
            .iter()
            .enumerate()
            .map(|(i, p)| (p.id, i))
            .collect();
        let mut els = Vec::new();
        for r in r0..r1 {
            let (depth, node) = &flat[rows[r]];
            let id = node.id;
            let target = node.target.and_then(|t| positions.get(&t).copied());
            let orphan = node.target.is_some() && target.is_none();
            let expanded = doc.outline_expanded.contains(&id);
            let active = node.target.is_some() && node.target == cur_id;
            let chevron = if node.children.is_empty() {
                " "
            } else if expanded {
                "▾"
            } else {
                "▸"
            };
            els.push(
                div()
                    .id(("outline", id.0 as usize))
                    .absolute()
                    .top(px(r as f32 * OUTLINE_ROW_H))
                    .left(px(0.0))
                    .w_full()
                    .h(px(OUTLINE_ROW_H))
                    .pl(px(6.0 + *depth as f32 * OUTLINE_INDENT))
                    .pr_2()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_sm()
                    .cursor_pointer()
                    .when(active, |d| d.bg(rgb(0x3a4a60)))
                    .hover(|s| s.bg(rgb(0x3a3a3a)))
                    .on_click(cx.listener(move |v, _, _, cx| {
                        if let Some(p) = target {
                            v.go_to_page(p, cx);
                        }
                    }))
                    .child(
                        div()
                            .id(("outline-toggle", id.0 as usize))
                            .w(px(14.0))
                            .text_color(rgb(0x9a9a9a))
                            .cursor_pointer()
                            .on_click(cx.listener(move |v, _, _, cx| {
                                if let State::Ready(doc) = &mut v.state {
                                    if !doc.outline_expanded.remove(&id) {
                                        doc.outline_expanded.insert(id);
                                    }
                                    cx.notify();
                                }
                                cx.stop_propagation();
                            }))
                            .child(chevron),
                    )
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_color(if target.is_some() {
                                rgb(0xd0d0d0)
                            } else if orphan {
                                rgb(0x8a6a6a)
                            } else {
                                rgb(0x8a8a8a)
                            })
                            .child(node.title.clone()),
                    ),
            );
        }
        div()
            .id("outline")
            .track_scroll(&doc.outline_scroll)
            .overflow_y_scroll()
            .size_full()
            .child(div().relative().w_full().h(px(total_h)).children(els))
            .into_any_element()
    }
}

/// Payload of a thumbnail drag.
#[derive(Clone)]
struct DragPages {
    ids: Vec<PageId>,
}

/// What follows the cursor while dragging pages.
struct DragPreview {
    count: usize,
}

impl Render for DragPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(rgb(0x4a9eff))
            .text_color(rgb(0xffffff))
            .text_sm()
            .child(if self.count == 1 {
                "1 page".to_string()
            } else {
                format!("{} pages", self.count)
            })
    }
}
