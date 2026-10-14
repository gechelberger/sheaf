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
    img, point, prelude::*, px, relative, rgb, rgba,
};
use sheaf_core::{PageIndex, PageSize, Rotation};
use sheaf_doc::save;
use sheaf_doc::{Annot, AnnotId, AnnotKind, Box2, Pt};

use crate::markup_tools::{Drag, Tool, arrow_head, rotate_pt, stroke_canvas};
use sheaf_doc::{
    Document, Edit, EditOutcome, FlatOutlineItem, OutlineId, PageId, PageRef, SourceId, SourceInfo,
};
use sheaf_render::{
    Job, OutlineItem, PixelFormat, Rect, RenderHandle, RenderRequest, Rendered, SearchEvent,
    SearchHit,
};

use crate::text_area::{TextArea, TextAreaEvent};
use crate::text_input::{TextInput, TextInputEvent};
use crate::{
    CancelOcr, CloseSearch, DeletePage, DuplicatePages, ExportCleanCopy, ExportFlattened,
    ExtractPages, Find, FirstPage, InsertPdf, LastPage, LineDown, LineUp, NewBookmark, NextPage,
    PrevPage, Redo, RotatePageCcw, RotatePageCw, Save, SaveAs, ScreenDown, ScreenUp, SearchNext,
    SearchPrev, SelectAllPages, ToolArrow, ToolConnector, ToolEllipse, ToolFreeText, ToolHighlight,
    ToolInk, ToolLine, ToolNote, ToolSelect, ToolSquare, ToolTick, Undo, ZoomActual, ZoomFitPage,
    ZoomFitWidth, ZoomIn, ZoomOut,
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Zoom {
    FitWidth,
    FitPage,
    /// 1.0 = 100%.
    Custom(f32),
}

pub(crate) enum State {
    Loading,
    Failed(String),
    /// Encrypted: waiting for a password.
    Locked(Locked),
    Ready(Box<Doc>),
}

pub(crate) struct Locked {
    input: Entity<TextInput>,
    _sub: Subscription,
    /// A password was tried and rejected.
    wrong: bool,
    focus_pending: bool,
}

/// Page geometry in logical px at the current zoom, for one snapshot of the
/// page sequence.
pub(crate) struct Layout {
    pub(crate) zoom: Zoom,
    pub(crate) view_w: f32,
    pub(crate) view_h: f32,
    pub(crate) version: u64,
    /// Logical px per PDF point.
    pub(crate) k: f32,
    pub(crate) refs: Vec<PageRef>,
    pub(crate) widths: Vec<f32>,
    pub(crate) heights: Vec<f32>,
    pub(crate) tops: Vec<f32>,
    pub(crate) lefts: Vec<f32>,
    /// Scrollable content size.
    pub(crate) total_w: f32,
    pub(crate) total_h: f32,
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

    pub(crate) fn len(&self) -> usize {
        self.refs.len()
    }

    /// Page whose vertical span contains `y` (or the last one that starts above it).
    pub(crate) fn page_at(&self, y: f32) -> usize {
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

pub(crate) struct Doc {
    pub(crate) model: Document,
    pub(crate) handles: HashMap<SourceId, RenderHandle>,
    /// Bumped on every model change so layouts and lookups can tell.
    pub(crate) version: u64,
    pub(crate) zoom: Zoom,
    pub(crate) scroll: ScrollHandle,
    pub(crate) layout: Option<Layout>,
    pub(crate) previews: HashMap<PageId, (Rotation, Arc<RenderImage>)>,
    pub(crate) tiles: HashMap<TileKey, Arc<RenderImage>>,
    /// Dropping the task drops the oneshot receiver, which tells the worker
    /// to skip the render if it hasn't started.
    pub(crate) inflight: HashMap<TileKey, Task<()>>,
    /// Images we've stopped showing; GPU textures are released in `render`,
    /// the only place we have a `Window`.
    pub(crate) retired: Vec<Arc<RenderImage>>,
    pub(crate) current_page: usize,
    pub(crate) pending_scroll: Option<ScrollTo>,
    pub(crate) first_paint: Option<Instant>,
    /// Last edit notice, shown in the status bar.
    pub(crate) notice: Option<String>,
    // Sidebar state.
    pub(crate) thumb_scroll: ScrollHandle,
    /// Pages whose previews the thumbnail panel is showing; kept cached.
    pub(crate) thumb_keep: HashSet<PageId>,
    /// Last page the thumbnail panel auto-scrolled to follow.
    pub(crate) thumb_followed: Option<PageId>,
    pub(crate) outline_loaded: bool,
    pub(crate) outline_expanded: HashSet<OutlineId>,
    pub(crate) outline_scroll: ScrollHandle,
    // Selection and drag state for the thumbnail panel.
    pub(crate) selection: HashSet<PageId>,
    /// Index the last plain/ctrl click landed on; shift-click extends from it.
    pub(crate) sel_anchor: Option<usize>,
    /// Insert index under the cursor while pages are being dragged.
    pub(crate) drop_target: Option<usize>,
    /// A save is running on a background thread.
    pub(crate) saving: bool,
    // Outline panel editing state (see outline_panel.rs).
    pub(crate) outline_focus: FocusHandle,
    pub(crate) outline_selected: Option<OutlineId>,
    pub(crate) outline_drop: Option<crate::outline_panel::OutlineDrop>,
    pub(crate) outline_rename: Option<crate::outline_panel::Rename>,
    pub(crate) outline_menu: Option<(OutlineId, gpui::Point<gpui::Pixels>)>,
    /// Rows as last rendered: (id, depth, display index of target).
    pub(crate) outline_rows: Vec<(OutlineId, u16, Option<usize>)>,
    // Markup tool state (see markup_tools.rs).
    pub(crate) drag: Option<Drag>,
    pub(crate) selected_annot: Option<(PageId, AnnotId)>,
    pub(crate) annot_editor: Option<AnnotEditor>,
    pub(crate) pending_connector: Option<crate::markup_tools::PendingConnector>,
    /// Comments other tools left in the primary file (read-only).
    pub(crate) foreign_annots: Vec<sheaf_doc::file::ForeignAnnot>,
    pub(crate) selected_foreign: Option<usize>,
}

impl Doc {
    pub(crate) fn primary(&self) -> &RenderHandle {
        &self.handles[&self.model.primary()]
    }

    pub(crate) fn page_ref(&self, index: usize) -> Option<PageRef> {
        self.model.page(index).copied()
    }

    /// Display index of the primary source's page `index`, if it's still in
    /// the document (search hits are reported in source pages).
    pub(crate) fn index_of_source_page(&self, index: u32) -> Option<usize> {
        let primary = self.model.primary();
        self.model
            .state()
            .pages
            .iter()
            .position(|p| p.source == primary && p.index == index)
    }

    /// Matches in unsaved OCR layers, which PDFium can't see, in document
    /// order. `find` runs the query over one layer.
    pub(crate) fn layer_hits(
        &self,
        find: impl Fn(&sheaf_doc::TextLayer) -> Vec<sheaf_doc::TextHit>,
    ) -> Vec<Hit> {
        let state = self.model.state();
        let mut out = Vec::new();
        for (order, p) in state.pages.iter().enumerate() {
            let Some(layer) = state.text_layer(p.id) else {
                continue;
            };
            for h in find(layer) {
                out.push(Hit {
                    page: p.id,
                    order,
                    rects: h
                        .rects
                        .into_iter()
                        .map(|b| Rect {
                            x: b.x,
                            y: b.y,
                            w: b.w,
                            h: b.h,
                        })
                        .collect(),
                    text: h.text,
                });
            }
        }
        out
    }

    /// Merge hits from the primary file's own text into `hits`, keeping
    /// document order. A page with an unsaved OCR layer is skipped: the
    /// layer's hits stand in for it (after a save the layer is in the file
    /// and gone from the model, so nothing is counted twice).
    pub(crate) fn merge_source_hits(&self, hits: &mut Vec<Hit>, incoming: Vec<SearchHit>) {
        let state = self.model.state();
        for h in incoming {
            let Some(order) = self.index_of_source_page(h.page) else {
                continue;
            };
            let page = state.pages[order].id;
            if state.has_text_layer(page) {
                continue;
            }
            let at = hits.partition_point(|x| x.order <= order);
            hits.insert(
                at,
                Hit {
                    page,
                    order,
                    rects: h.rects,
                    text: h.text,
                },
            );
        }
    }

    pub(crate) fn touched(&mut self) {
        self.version += 1;
        self.layout = None;
    }

    /// Drop every rendered image so pages re-rasterize (after the engine's
    /// view of a page changed, e.g. annotations hidden).
    fn invalidate_tiles(&mut self) {
        for (_, im) in self.tiles.drain() {
            self.retired.push(im);
        }
        for (_, (_, im)) in self.previews.drain() {
            self.retired.push(im);
        }
        self.inflight.clear();
        self.version += 1;
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ScrollTo {
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
    /// Put a page point (fractions of the page as shown) at the top-left of
    /// the viewport: a bookmark's position.
    Point {
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

/// Inline editor for an annotation's text.
pub(crate) struct AnnotEditor {
    pub page: PageId,
    pub id: AnnotId,
    pub input: Entity<TextArea>,
    pub _sub: Subscription,
    pub _blur: Subscription,
}

/// What happened to a document, for listeners outside the view (plugins,
/// see `todos/plugins.md`).
#[derive(Debug, Clone, PartialEq)]
pub enum DocEvent {
    /// The file is open and its model exists.
    Opened,
    /// The model changed: an edit, a batch, an undo or a redo. `label` is
    /// the undo label ("Undo: …"/"Redo: …" for those two).
    Edited {
        label: String,
        outcome: EditOutcome,
        /// The plugin whose edits these were, if any.
        plugin: Option<String>,
    },
    Saved {
        path: PathBuf,
    },
    /// The set of selected pages changed.
    SelectionChanged,
    /// The selected region (Shift+drag) changed: live while dragging.
    Region {
        page: PageId,
        /// Source-displayed space, like annotations.
        rect: Box2,
    },
    /// The region was cleared.
    RegionCleared,
    /// The tab closed. Emitted by the workspace just before it drops the view.
    Closed,
}

impl gpui::EventEmitter<DocEvent> for DocView {}

pub struct DocView {
    pub(crate) focus_handle: FocusHandle,
    pub(crate) path: PathBuf,
    pub(crate) timing: bool,
    pub(crate) launched: Instant,
    pub(crate) state: State,
    pub(crate) search: Option<Search>,
    pub(crate) pattern: Option<crate::pattern_panel::PatternSearch>,
    pub(crate) initial_pattern: Option<String>,
    pub(crate) calc: Option<crate::calc_panel::CalcTape>,
    pub(crate) comments: Option<crate::comments_panel::Comments>,
    /// A Recognize Text run in progress (see ocr_run.rs).
    pub(crate) ocr: Option<crate::ocr_run::OcrRun>,
    pub(crate) pending_comments_focus: bool,
    pub(crate) initial_query: Option<String>,
    pub(crate) search_started: Option<Instant>,
    pub(crate) selftest_save: Option<PathBuf>,
    /// Re-focus the document on the next frame (after a text field closes).
    pub(crate) pending_focus: bool,
    /// Re-focus the Bookmarks panel on its next render (after a rename field closes).
    pub(crate) pending_outline_focus: bool,
    pub(crate) selftest_insert: Option<PathBuf>,
    pub(crate) selftest_annot: Option<PathBuf>,
    pub(crate) selftest_clean: Option<PathBuf>,
    pub(crate) selftest_extract: Option<PathBuf>,
    pub(crate) selftest_flatten: Option<PathBuf>,
    /// Dev: OCR every page, then save to this path (see ocr_run.rs).
    pub(crate) selftest_ocr: Option<PathBuf>,
    /// Password the file needs, once known.
    password: Option<String>,
    /// Dev: open the inline editor on this page's last mark next frame.
    pending_edit_last: Option<PageId>,
    /// Recovery data found when opening, until the user has been asked.
    pending_recovery: Option<sheaf_doc::recovery::Recovery>,
    autosave: Option<Task<()>>,
    /// When the edits since the last sidecar write began.
    autosave_since: Option<Instant>,
    pub(crate) tool: Tool,
    pub(crate) tool_color: [u8; 3],
    pub(crate) tick_glyph: String,
    /// Set while a plugin's edits are applied, so its own edits aren't
    /// reported back to it.
    pub(crate) edit_source: Option<String>,
    /// Shift+drag region (see region.rs).
    pub(crate) region: Option<crate::region::Region>,
    /// Rows of plugin panels for this document, by `<plugin>.<panel>`.
    #[cfg(feature = "plugins")]
    pub(crate) plugin_panels: HashMap<String, crate::plugin_panel::PluginPanelState>,
}

/// A search match, addressed by page id so it survives reordering and so
/// unsaved OCR layers (which PDFium can't see) can contribute.
#[derive(Debug, Clone)]
pub(crate) struct Hit {
    pub page: PageId,
    /// Display index when the hit was recorded; keeps hits in document order.
    pub order: usize,
    pub rects: Vec<Rect>,
    pub text: String,
}

/// An open search bar and the running / finished query behind it.
pub(crate) struct Search {
    input: Entity<TextInput>,
    _input_sub: Subscription,
    _events_sub: Subscription,
    /// Query the current results are for.
    query: String,
    /// Hits in document order.
    hits: Vec<Hit>,
    current: Option<usize>,
    scanned: u32,
    done: bool,
    /// Dropping this cancels the worker-side search.
    _task: Option<Task<()>>,
}

impl DocView {
    pub fn new(
        path: PathBuf,
        password: Option<String>,
        timing: bool,
        launched: Instant,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            password,
            path,
            timing,
            launched,
            state: State::Loading,
            search: None,
            pattern: None,
            initial_pattern: None,
            calc: None,
            comments: None,
            ocr: None,
            pending_comments_focus: false,
            initial_query: None,
            search_started: None,
            selftest_save: None,
            pending_focus: false,
            pending_outline_focus: false,
            selftest_insert: None,
            selftest_annot: None,
            selftest_clean: None,
            selftest_extract: None,
            selftest_flatten: None,
            selftest_ocr: None,
            pending_recovery: None,
            pending_edit_last: None,
            autosave: None,
            autosave_since: None,
            tool: Tool::Select,
            tool_color: [255, 230, 0],
            tick_glyph: "✓".into(),
            edit_source: None,
            region: None,
            #[cfg(feature = "plugins")]
            plugin_panels: HashMap::new(),
        };
        this.open(cx);
        this
    }

    /// The file's name, without the dirty marker.
    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
            .to_string()
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
        let password = self.password.clone();
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
                    let recovery = sheaf_doc::recovery::find(&path);
                    RenderHandle::open(path.clone(), password).map(|h| (h, path, bytes, recovery))
                })
                .await;
            this.update(cx, |viewer, cx| {
                match opened {
                    Ok((handle, path, bytes, recovery)) => {
                        let info = handle.info();
                        if timing {
                            eprintln!(
                                "[timing] open: {:?} engine, {:?} since launch, {} pages",
                                info.open_time,
                                launched.elapsed(),
                                info.page_count
                            );
                        }
                        viewer.pending_recovery = recovery;
                        viewer.install(handle, path, bytes, cx);
                    }
                    Err(e) if sheaf_render::OpenError::is_password(&e) => viewer.lock(cx),
                    Err(e) => viewer.state = State::Failed(format!("{e:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Show the password prompt; Enter retries the open with what was typed.
    fn lock(&mut self, cx: &mut Context<Self>) {
        let wrong = self.password.take().is_some();
        let input = cx.new(|cx| TextInput::new("Password", cx).masked());
        let sub = cx.subscribe(&input, |this, input, ev: &TextInputEvent, cx| {
            if *ev == TextInputEvent::Confirm {
                let pw = input.read(cx).text().to_string();
                if !pw.is_empty() {
                    this.password = Some(pw);
                    this.open(cx);
                }
            }
        });
        self.state = State::Locked(Locked {
            input,
            _sub: sub,
            wrong,
            focus_pending: true,
        });
        cx.notify();
    }

    /// Make an opened file the primary source and start the background
    /// loads. `path` is what the engine opened: the document's own path,
    /// except while showing recovered changes from a temporary file.
    fn install(
        &mut self,
        handle: RenderHandle,
        path: PathBuf,
        bytes: usize,
        cx: &mut Context<Self>,
    ) {
        let info = handle.info();
        let source = SourceInfo {
            path,
            page_count: info.page_count,
            first_page: info.first_page,
            page_sizes: None,
            bytes,
            password: self.password.clone(),
        };
        let model = Document::new(source, &[], sheaf_doc::history::default_policy());
        let mut handles = HashMap::new();
        handles.insert(model.primary(), handle);
        self.state = State::Ready(Box::new(Doc {
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
            outline_focus: cx.focus_handle(),
            outline_selected: None,
            outline_drop: None,
            outline_rename: None,
            outline_menu: None,
            outline_rows: Vec::new(),
            drag: None,
            selected_annot: None,
            annot_editor: None,
            pending_connector: None,
            foreign_annots: Vec::new(),
            selected_foreign: None,
        }));
        self.fetch_page_sizes(cx);
        self.fetch_outline(cx);
        self.load_sheaf_annots(cx);
        cx.emit(DocEvent::Opened);
        if let Some(p) = self.selftest_save.take() {
            self.selftest_save(p, cx);
        }
        if let Some(p) = self.selftest_insert.take() {
            self.insert_pdf(p, cx);
        }
        if let Some(p) = self.selftest_annot.take() {
            self.selftest_annot(p, cx);
        }
        if let Some(p) = self.selftest_clean.take() {
            self.selftest_clean(p, cx);
        }
        if let Some(p) = self.selftest_extract.take() {
            let ids: Vec<PageId> = (0..2).filter_map(|i| self.doc_page_id(i)).collect();
            self.extract_to(ids, p, cx);
        }
        if let Some(p) = self.selftest_flatten.take() {
            self.selftest_flatten(p, cx);
        }
        if let Some(p) = self.selftest_ocr.take() {
            self.selftest_ocr(p, cx);
        }
        if let Some(src) = self.initial_pattern.take() {
            self.run_initial_pattern(src, cx);
        }
        if let Some(q) = self.initial_query.take() {
            self.new_search(cx);
            self.start_search(q, cx);
        }
    }

    /// Ask whether to apply recovery data found next to the file.
    fn offer_recovery(
        &mut self,
        rec: sheaf_doc::recovery::Recovery,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = self.file_name();
        let rx = window.prompt(
            gpui::PromptLevel::Warning,
            &format!("Recover unsaved changes to \"{name}\"?"),
            Some("sheaf found edits that weren't saved when it last closed. Recovered changes stay unsaved until you save."),
            &[gpui::PromptButton::ok("Recover"), gpui::PromptButton::new("Discard")],
            cx,
        );
        let path = self.path.clone();
        cx.spawn(async move |this, cx| match rx.await {
            Ok(0) => {
                this.update(cx, |v, cx| v.recover(rec, cx)).ok();
            }
            Ok(_) => sheaf_doc::recovery::discard(&path),
            Err(_) => {}
        })
        .detach();
    }

    /// Open the original plus its recovery section from a temporary file,
    /// leaving the document dirty so Save writes it back to its own path.
    fn recover(&mut self, rec: sheaf_doc::recovery::Recovery, cx: &mut Context<Self>) {
        let path = self.path.clone();
        self.state = State::Loading;
        let timing = self.timing;
        cx.spawn(async move |this, cx| {
            let t = Instant::now();
            let opened = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    async move {
                        let stamp = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis())
                            .unwrap_or(0);
                        let tmp = std::env::temp_dir().join(format!(
                            "sheaf-recovered-{}-{stamp}.pdf",
                            std::process::id()
                        ));
                        sheaf_doc::recovery::materialize(&path, &rec, &tmp)?;
                        let bytes = std::fs::metadata(&tmp)
                            .map(|m| m.len() as usize)
                            .unwrap_or(0);
                        let handle = RenderHandle::open(tmp.clone(), None)?;
                        anyhow::Ok((handle, tmp, bytes))
                    }
                })
                .await;
            this.update(cx, |viewer, cx| {
                match opened {
                    Ok((handle, tmp, bytes)) => {
                        viewer.install(handle, tmp, bytes, cx);
                        if let State::Ready(doc) = &mut viewer.state {
                            doc.model.mark_unsaved();
                            doc.notice =
                                Some("Recovered unsaved changes — Ctrl+S to keep them".into());
                        }
                        if timing {
                            eprintln!("[timing] recovered unsaved changes in {:?}", t.elapsed());
                        }
                    }
                    Err(e) => {
                        // Unusable recovery data: drop it and open the file as is.
                        log::error!("recovery failed: {e:#}");
                        sheaf_doc::recovery::discard(&path);
                        viewer.open(cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ----- crash-recovery autosave ------------------------------------------

    /// Write the recovery sidecar a few seconds after the last edit, and at
    /// least every half minute while edits keep coming.
    pub(crate) fn schedule_autosave(&mut self, cx: &mut Context<Self>) {
        let first = *self.autosave_since.get_or_insert_with(Instant::now);
        let delay = if first.elapsed() >= std::time::Duration::from_secs(30) {
            std::time::Duration::ZERO
        } else {
            std::time::Duration::from_secs(3)
        };
        self.autosave = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update(cx, |v, cx| v.write_recovery(cx)).ok();
        }));
    }

    fn write_recovery(&mut self, cx: &mut Context<Self>) {
        self.autosave = None;
        self.autosave_since = None;
        let path = self.path.clone();
        let State::Ready(doc) = &self.state else {
            return;
        };
        let (dirty, saving) = (doc.model.is_dirty(), doc.saving);
        // Only against the file itself, not a recovered temporary.
        let primary_is_file = doc
            .model
            .source(doc.model.primary())
            .is_some_and(|s| s.path == path);
        if !dirty {
            sheaf_doc::recovery::discard(&path);
            return;
        }
        if saving {
            self.schedule_autosave(cx);
            return;
        }
        if !primary_is_file || self.password.is_some() {
            return;
        }
        let snap = doc.model.snapshot();
        let timing = self.timing;
        cx.background_executor()
            .spawn(async move {
                let t = Instant::now();
                match sheaf_doc::recovery::write(&snap) {
                    Ok(Some(n)) => {
                        if timing {
                            eprintln!("[timing] recovery sidecar: {n} bytes in {:?}", t.elapsed());
                        }
                    }
                    Ok(None) => {}
                    Err(e) => log::warn!("writing recovery sidecar: {e:#}"),
                }
            })
            .detach();
    }

    /// The user chose to drop unsaved changes: the sidecar goes with them.
    pub fn discard_recovery(&self) {
        sheaf_doc::recovery::discard(&self.path);
    }

    fn fetch_page_sizes(&mut self, cx: &mut Context<Self>) {
        let primary = match &self.state {
            State::Ready(doc) => doc.model.primary(),
            _ => return,
        };
        self.fetch_source_sizes(primary, cx);
    }

    /// Enumerate a source's page sizes on its worker and feed them to the model.
    fn fetch_source_sizes(&mut self, sid: SourceId, cx: &mut Context<Self>) {
        let State::Ready(doc) = &self.state else {
            return;
        };
        let Some(handle) = doc.handles.get(&sid) else {
            return;
        };
        let rx = handle.page_sizes();
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
                        doc.model.set_source_sizes(sid, sizes);
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

    pub(crate) fn apply(&mut self, edit: Edit, label: &str, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let result = doc.model.apply(edit, label);
        self.finish_edit(label, result, cx);
    }

    /// Apply edits as one undo step; if any fails, none are applied.
    #[cfg_attr(not(feature = "plugins"), allow(dead_code))]
    pub(crate) fn apply_batch(&mut self, edits: Vec<Edit>, label: &str, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let result = doc.model.apply_batch(edits, label);
        self.finish_edit(label, result, cx);
    }

    /// After `apply`/`apply_batch`: notice, redraw, autosave, and tell
    /// listeners.
    fn finish_edit(
        &mut self,
        label: &str,
        result: anyhow::Result<EditOutcome>,
        cx: &mut Context<Self>,
    ) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        match result {
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
                self.after_model_change(label.to_string(), out, cx);
            }
            Err(e) => {
                doc.notice = Some(format!("{label} failed: {e:#}"));
                cx.notify();
            }
        }
    }

    /// Bookkeeping after any change to the model.
    pub(crate) fn after_model_change(
        &mut self,
        label: String,
        outcome: EditOutcome,
        cx: &mut Context<Self>,
    ) {
        if self.prune_selection() {
            cx.emit(DocEvent::SelectionChanged);
        }
        self.prune_region(cx);
        self.schedule_autosave(cx);
        let plugin = self.edit_source.clone();
        cx.emit(DocEvent::Edited {
            label,
            outcome,
            plugin,
        });
        cx.notify();
    }

    /// What a plugin command sees of this document.
    #[cfg(feature = "plugins")]
    pub(crate) fn plugin_context(&self) -> Option<sheaf_plugin::DocContext> {
        let State::Ready(doc) = &self.state else {
            return None;
        };
        let selection = doc
            .model
            .state()
            .pages
            .iter()
            .filter(|p| doc.selection.contains(&p.id))
            .map(|p| p.id)
            .collect();
        Some(sheaf_plugin::DocContext {
            path: self.path.clone(),
            snapshot: doc.model.snapshot(),
            selection,
            current_page: doc.page_ref(doc.current_page).map(|p| p.id),
            text: crate::plugins::text_source(&doc.handles),
            ocr: Some(crate::plugins::ocr_source(&doc.handles)),
        })
    }

    /// Pages an operation applies to: the selection, or the current page.
    pub(crate) fn target_ids(&self) -> Vec<PageId> {
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
        cx.emit(DocEvent::SelectionChanged);
        cx.notify();
    }

    fn clear_selection(&mut self, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        if !doc.selection.is_empty() {
            doc.selection.clear();
            cx.emit(DocEvent::SelectionChanged);
            cx.notify();
        }
    }

    /// Drop pages that no longer exist (after delete/undo/redo). True if
    /// the page selection changed.
    fn prune_selection(&mut self) -> bool {
        let State::Ready(doc) = &mut self.state else {
            return false;
        };
        if let Some((pid, aid)) = doc.selected_annot
            && !doc
                .model
                .state()
                .annots_on(pid)
                .is_some_and(|l| l.iter().any(|a| a.id == aid))
        {
            doc.selected_annot = None;
        }
        let live: HashSet<PageId> = doc.model.state().pages.iter().map(|p| p.id).collect();
        let before = doc.selection.len();
        doc.selection.retain(|id| live.contains(id));
        doc.selection.len() != before
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
        cx.emit(DocEvent::SelectionChanged);
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
            cx.emit(DocEvent::SelectionChanged);
        }
    }

    pub(crate) fn undo(&mut self, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let Some(label) = doc.model.undo().map(str::to_string) else {
            doc.notice = Some("Nothing to undo".into());
            cx.notify();
            return;
        };
        doc.notice = Some(format!("Undid: {label}"));
        doc.touched();
        self.after_model_change(format!("Undo: {label}"), EditOutcome::default(), cx);
    }

    pub(crate) fn redo(&mut self, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let Some(label) = doc.model.redo().map(str::to_string) else {
            doc.notice = Some("Nothing to redo".into());
            cx.notify();
            return;
        };
        doc.notice = Some(format!("Redid: {label}"));
        doc.touched();
        self.after_model_change(format!("Redo: {label}"), EditOutcome::default(), cx);
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
        let rx = crate::file_dialog::save(cx, &dir, suggested.as_deref());
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                this.update(cx, |view, cx| view.save_to(path, cx).detach())
                    .ok();
            }
        })
        .detach();
    }

    /// Unsaved changes, or a save still in flight.
    pub fn is_dirty(&self) -> bool {
        matches!(&self.state, State::Ready(d) if d.model.is_dirty() || d.saving)
    }

    /// Plain Save: to the document's own path.
    pub fn save(&mut self, cx: &mut Context<Self>) -> Task<bool> {
        let path = self.path.clone();
        self.save_to(path, cx)
    }

    /// Save to `path` (the document's own path for plain Save). Runs the
    /// rewrite on a background thread from a snapshot, then reopens the saved
    /// file as the new primary source without disturbing undo history.
    /// Resolves to whether the save succeeded.
    pub fn save_to(&mut self, path: PathBuf, cx: &mut Context<Self>) -> Task<bool> {
        let State::Ready(doc) = &mut self.state else {
            return Task::ready(false);
        };
        if doc.saving {
            doc.notice = Some("Already saving…".into());
            cx.notify();
            return Task::ready(false);
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
                    return false;
                };
                doc.saving = false;
                let ok = match result {
                    Ok((handle, bytes, strategy)) => {
                        let (page_count, first_page) =
                            (handle.info().page_count, handle.info().first_page);
                        let saved = SourceInfo {
                            path: path.clone(),
                            page_count,
                            first_page,
                            page_sizes: None,
                            bytes,
                            password: None,
                        };
                        let sid = doc.model.rebase_to_saved(saved);
                        // Our marks are now in the file; keep PDFium from drawing them.
                        let pages: Vec<u32> = doc
                            .model
                            .state()
                            .pages
                            .iter()
                            .enumerate()
                            .filter(|(_, p)| doc.model.state().annots.contains_key(&p.id))
                            .map(|(i, _)| i as u32)
                            .collect();
                        if !pages.is_empty() {
                            let rx = handle.hide_sheaf_annots(pages);
                            cx.spawn(async move |this, cx| {
                                let _ = rx.await;
                                this.update(cx, |view, cx| {
                                    if let State::Ready(doc) = &mut view.state {
                                        doc.invalidate_tiles();
                                    }
                                    cx.notify();
                                })
                                .ok();
                            })
                            .detach();
                        }
                        doc.handles.insert(sid, handle);
                        sheaf_doc::recovery::discard(&view.path);
                        sheaf_doc::recovery::discard(&path);
                        view.autosave = None;
                        view.autosave_since = None;
                        view.path = path.clone();
                        let was_encrypted = view.password.take().is_some();
                        doc.notice = Some(format!(
                            "Saved {} ({strategy:?}) in {:?}{}",
                            path.display(),
                            t.elapsed(),
                            if was_encrypted {
                                " — password protection removed"
                            } else {
                                ""
                            }
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
                        cx.emit(DocEvent::Saved { path: path.clone() });
                        true
                    }
                    Err(e) => {
                        doc.notice = Some(format!("Save failed: {e:#}"));
                        log::error!("save: {e:#}");
                        false
                    }
                };
                cx.notify();
                ok
            })
            .unwrap_or(false)
        })
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
        self.save_to(path, cx).detach();
    }

    pub fn set_selftest_save(&mut self, path: PathBuf) {
        self.selftest_save = Some(path);
    }

    // ----- markup -------------------------------------------------------------

    /// Read back annotations sheaf wrote into the primary file earlier and
    /// take over their rendering.
    fn load_sheaf_annots(&mut self, cx: &mut Context<Self>) {
        let path = self.path.clone();
        let password = self.password.clone();
        let timing = self.timing;
        cx.spawn(async move |this, cx| {
            let t = Instant::now();
            let found = cx
                .background_executor()
                .spawn(async move { sheaf_doc::file::read_annots(&path, password.as_deref()) })
                .await;
            let (by_index, foreign) = match found {
                Ok(a) => (a.sheaf, a.foreign),
                Err(e) => {
                    log::warn!("reading annotations: {e:#}");
                    return;
                }
            };
            this.update(cx, |view, cx| {
                let State::Ready(doc) = &mut view.state else {
                    return;
                };
                if timing && !foreign.is_empty() {
                    eprintln!(
                        "[timing] found {} third-party annotations in {:?}",
                        foreign.len(),
                        t.elapsed()
                    );
                }
                doc.foreign_annots = foreign;
                if by_index.is_empty() {
                    cx.notify();
                    return;
                }
                let pages: Vec<u32> = by_index.keys().copied().collect();
                let n: usize = by_index.values().map(|v| v.len()).sum();
                match doc.model.set_initial_annots(by_index) {
                    Ok(()) => {
                        if timing {
                            eprintln!(
                                "[timing] loaded {n} sheaf annotations on {} pages in {:?}",
                                pages.len(),
                                t.elapsed()
                            );
                        }
                        let rx = doc.primary().hide_sheaf_annots(pages);
                        doc.touched();
                        cx.spawn(async move |this, cx| {
                            let _ = rx.await;
                            this.update(cx, |view, cx| {
                                if let State::Ready(doc) = &mut view.state {
                                    doc.invalidate_tiles();
                                }
                                cx.notify();
                            })
                            .ok();
                        })
                        .detach();
                    }
                    Err(e) => log::warn!("annotations arrived after edits; ignored: {e:#}"),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// A new annotation with the document's defaults filled in.
    pub(crate) fn new_annot(&self, kind: AnnotKind, color: [u8; 3]) -> Annot {
        Annot {
            id: sheaf_doc::AnnotId(0),
            kind,
            color,
            opacity: 1.0,
            stroke: 1.5,
            author: sheaf_doc::default_author(),
            contents: String::new(),
            modified: sheaf_doc::pdf_date_now(),
        }
    }

    pub(crate) fn add_annot(
        &mut self,
        page: PageId,
        annot: Annot,
        label: &str,
        cx: &mut Context<Self>,
    ) {
        self.apply(Edit::AnnotAdd { page, annot }, label, cx);
    }

    /// Dev hook: drop one of each mark on page 1 and save to `path`.
    pub fn selftest_annot(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let State::Ready(doc) = &self.state else {
            return;
        };
        let Some(p0) = doc.page_ref(0).map(|p| p.id) else {
            return;
        };
        let mut hl = self.new_annot(
            AnnotKind::Highlight {
                rects: vec![Box2 {
                    x: 72.0,
                    y: 60.0,
                    w: 300.0,
                    h: 30.0,
                }],
            },
            [255, 230, 0],
        );
        hl.opacity = 0.6;
        self.add_annot(p0, hl, "Highlight", cx);
        let mut note = self.new_annot(
            AnnotKind::Note {
                pos: Pt { x: 400.0, y: 60.0 },
            },
            [255, 200, 0],
        );
        note.contents = "A sticky note".into();
        self.add_annot(p0, note, "Note", cx);
        let mut text = self.new_annot(
            AnnotKind::FreeText {
                rect: Box2 {
                    x: 72.0,
                    y: 120.0,
                    w: 240.0,
                    h: 48.0,
                },
                size: 12.0,
            },
            [200, 0, 0],
        );
        text.contents = "Free text written by sheaf, wrapped onto a second line.".into();
        self.add_annot(p0, text, "Text", cx);
        let sq = self.new_annot(
            AnnotKind::Square {
                rect: Box2 {
                    x: 350.0,
                    y: 120.0,
                    w: 120.0,
                    h: 48.0,
                },
            },
            [0, 120, 255],
        );
        self.add_annot(p0, sq, "Square", cx);
        let tick = self.new_annot(
            AnnotKind::Stamp {
                rect: Box2 {
                    x: 500.0,
                    y: 120.0,
                    w: 28.0,
                    h: 28.0,
                },
                glyph: "✓".into(),
            },
            [0, 150, 0],
        );
        self.add_annot(p0, tick, "Tickmark", cx);
        let tick2 = self.new_annot(
            AnnotKind::Stamp {
                rect: Box2 {
                    x: 540.0,
                    y: 120.0,
                    w: 28.0,
                    h: 28.0,
                },
                glyph: "F".into(),
            },
            [200, 0, 0],
        );
        self.add_annot(p0, tick2, "Tickmark", cx);
        let arrow = self.new_annot(
            AnnotKind::Line {
                a: Pt { x: 80.0, y: 220.0 },
                b: Pt { x: 300.0, y: 260.0 },
                arrow: true,
            },
            [0, 120, 255],
        );
        self.add_annot(p0, arrow, "Arrow", cx);
        let mut ink = self.new_annot(
            AnnotKind::Ink {
                paths: vec![
                    (0..40)
                        .map(|i| Pt {
                            x: 350.0 + i as f32 * 5.0,
                            y: 240.0 + (i as f32 * 0.5).sin() * 20.0,
                        })
                        .collect(),
                ],
            },
            [160, 60, 220],
        );
        ink.stroke = 2.5;
        self.add_annot(p0, ink, "Pencil", cx);
        let State::Ready(doc) = &self.state else {
            return;
        };
        let p1 = doc.page_ref(1).map(|p| p.id).unwrap_or(p0);
        let mk = |rect: Box2| {
            self.new_annot(
                AnnotKind::Connector {
                    rect,
                    label: "1".into(),
                    partner: None,
                },
                [220, 40, 40],
            )
        };
        let a = mk(Box2 {
            x: 500.0,
            y: 220.0,
            w: 22.0,
            h: 22.0,
        });
        let b = mk(Box2 {
            x: 100.0,
            y: 100.0,
            w: 22.0,
            h: 22.0,
        });
        self.apply(
            Edit::ConnectorAdd {
                a: (p0, a),
                b: (p1, b),
            },
            "Connector 1",
            cx,
        );
        let mut tape = self.new_annot(
            AnnotKind::Tape {
                rect: Box2 {
                    x: 360.0,
                    y: 300.0,
                    w: 110.0,
                    h: 60.0,
                },
                size: 10.0,
            },
            [20, 20, 20],
        );
        tape.contents = "1,234.50 +\n250.00 +\n84.50 -\n1,400.00 T".into();
        self.add_annot(p0, tape, "Place tape", cx);
        self.save_to(path, cx).detach();
        // Leave the multi-line editor open on the tape so it gets exercised.
        self.pending_edit_last = Some(p0);
    }

    pub fn set_selftest_annot(&mut self, path: PathBuf) {
        self.selftest_annot = Some(path);
    }

    pub fn set_selftest_clean(&mut self, path: PathBuf) {
        self.selftest_clean = Some(path);
    }

    /// Dev hook: drop a couple of marks, then export a clean copy to `path`
    /// (the document itself is left dirty, as the real command would).
    pub fn selftest_clean(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let State::Ready(doc) = &self.state else {
            return;
        };
        let Some(p0) = doc.page_ref(0).map(|p| p.id) else {
            return;
        };
        let mut note = self.new_annot(
            AnnotKind::Note {
                pos: Pt { x: 400.0, y: 60.0 },
            },
            [255, 200, 0],
        );
        note.contents = "should not survive".into();
        self.add_annot(p0, note, "Note", cx);
        let tick = self.new_annot(
            AnnotKind::Stamp {
                rect: Box2 {
                    x: 500.0,
                    y: 120.0,
                    w: 28.0,
                    h: 28.0,
                },
                glyph: "✓".into(),
            },
            [0, 150, 0],
        );
        self.add_annot(p0, tick, "Tickmark", cx);
        self.export_clean_to(path, cx);
    }

    /// Every page's id, in document order.
    #[cfg_attr(not(feature = "plugins"), allow(dead_code))]
    pub(crate) fn page_ids(&self) -> Vec<PageId> {
        match &self.state {
            State::Ready(doc) => doc.model.state().pages.iter().map(|p| p.id).collect(),
            _ => Vec::new(),
        }
    }

    fn doc_page_id(&self, index: usize) -> Option<PageId> {
        match &self.state {
            State::Ready(doc) => doc.page_ref(index).map(|p| p.id),
            _ => None,
        }
    }

    /// Dev hook: a few marks, then a flattened export to `path`.
    pub fn selftest_flatten(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(p0) = self.doc_page_id(0) else {
            return;
        };
        let mut text = self.new_annot(
            AnnotKind::FreeText {
                rect: Box2 {
                    x: 72.0,
                    y: 200.0,
                    w: 240.0,
                    h: 40.0,
                },
                size: 12.0,
            },
            [200, 0, 0],
        );
        text.contents = "Flattened free text".into();
        self.add_annot(p0, text, "Text", cx);
        let sq = self.new_annot(
            AnnotKind::Square {
                rect: Box2 {
                    x: 350.0,
                    y: 200.0,
                    w: 120.0,
                    h: 48.0,
                },
            },
            [0, 120, 255],
        );
        self.add_annot(p0, sq, "Square", cx);
        let tick = self.new_annot(
            AnnotKind::Stamp {
                rect: Box2 {
                    x: 500.0,
                    y: 200.0,
                    w: 28.0,
                    h: 28.0,
                },
                glyph: "✓".into(),
            },
            [0, 150, 0],
        );
        self.add_annot(p0, tick, "Tickmark", cx);
        self.export_flattened_to(path, cx);
    }

    // ----- extract pages -------------------------------------------------------

    /// Document > Extract Pages: the selection (or current page) to a new file.
    fn extract_pages_prompt(&mut self, cx: &mut Context<Self>) {
        let ids = self.target_ids();
        if ids.is_empty() {
            return;
        }
        let dir = self
            .path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default();
        let suggested = self
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| format!("{s} - pages.pdf"));
        let rx = crate::file_dialog::save(cx, &dir, suggested.as_deref());
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                this.update(cx, |view, cx| view.extract_to(ids, path, cx))
                    .ok();
            }
        })
        .detach();
    }

    pub fn extract_to(&mut self, ids: Vec<PageId>, path: PathBuf, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        doc.notice = Some(format!("Extracting to {}…", path.display()));
        let snapshot = doc.model.snapshot();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    async move { save::extract_pages(&snapshot, &ids, &path) }
                })
                .await;
            this.update(cx, |view, cx| {
                if let State::Ready(doc) = &mut view.state {
                    doc.notice = Some(match result {
                        Ok(n) => format!(
                            "Extracted {n} page{} to {}",
                            if n == 1 { "" } else { "s" },
                            path.display()
                        ),
                        Err(e) => format!("Extract failed: {e:#}"),
                    });
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ----- export flattened copy ----------------------------------------------

    fn export_flattened_prompt(&mut self, cx: &mut Context<Self>) {
        let dir = self
            .path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default();
        let suggested = self
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| format!("{s} - flattened.pdf"));
        let rx = crate::file_dialog::save(cx, &dir, suggested.as_deref());
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                this.update(cx, |view, cx| view.export_flattened_to(path, cx))
                    .ok();
            }
        })
        .detach();
    }

    pub fn export_flattened_to(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        doc.notice = Some(format!("Exporting flattened copy to {}…", path.display()));
        let snapshot = doc.model.snapshot();
        let timing = self.timing;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let t = Instant::now();
            let result = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    async move { save::export_flattened(&snapshot, &path) }
                })
                .await;
            this.update(cx, |view, cx| {
                if let State::Ready(doc) = &mut view.state {
                    doc.notice = Some(match result {
                        Ok(n) => {
                            if timing {
                                eprintln!(
                                    "[timing] exported flattened copy ({n} marks) in {:?}",
                                    t.elapsed()
                                );
                            }
                            format!(
                                "Exported flattened copy to {} ({n} mark{})",
                                path.display(),
                                if n == 1 { "" } else { "s" }
                            )
                        }
                        Err(e) => format!("Export failed: {e:#}"),
                    });
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ----- export clean copy ---------------------------------------------------

    /// File > Export Clean Copy: ask for a path, then write the document
    /// there with all markup removed.
    fn export_clean_copy_prompt(&mut self, cx: &mut Context<Self>) {
        let dir = self
            .path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default();
        let suggested = self
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| format!("{s} - clean.pdf"));
        let rx = crate::file_dialog::save(cx, &dir, suggested.as_deref());
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                this.update(cx, |view, cx| view.export_clean_to(path, cx))
                    .ok();
            }
        })
        .detach();
    }

    pub fn export_clean_to(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        doc.notice = Some(format!("Exporting clean copy to {}…", path.display()));
        let snapshot = doc.model.snapshot();
        let timing = self.timing;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let t = Instant::now();
            let result = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    async move { save::export_clean_copy(&snapshot, &path) }
                })
                .await;
            this.update(cx, |view, cx| {
                if let State::Ready(doc) = &mut view.state {
                    doc.notice = Some(match &result {
                        Ok(()) => {
                            if timing {
                                eprintln!(
                                    "[timing] exported clean copy {} in {:?}",
                                    path.display(),
                                    t.elapsed()
                                );
                            }
                            format!("Exported clean copy to {}", path.display())
                        }
                        Err(e) => format!("Export failed: {e:#}"),
                    });
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Open an inline text field for the contents of the most recently
    /// added annotation on `page` (notes and free text).
    pub(crate) fn start_annot_editor(
        &mut self,
        page: PageId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let last = match &self.state {
            State::Ready(doc) => doc
                .model
                .state()
                .annots_on(page)
                .and_then(|l| l.last().map(|a| a.id)),
            _ => None,
        };
        if let Some(id) = last {
            self.start_annot_editor_for(page, id, window, cx);
        }
    }

    /// Open the inline text field for annotation `id` on `page`.
    pub(crate) fn start_annot_editor_for(
        &mut self,
        page: PageId,
        id: AnnotId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let Some(annot) = doc
            .model
            .state()
            .annots_on(page)
            .and_then(|l| l.iter().find(|a| a.id == id).cloned())
        else {
            return;
        };
        let input = cx.new(|cx| {
            let mut i =
                TextArea::new("Type a note; Ctrl+Enter or click away when done", cx).rows(3, 10);
            i.set_text(annot.contents.clone(), cx);
            i
        });
        // Enter, Escape and clicking elsewhere all keep the text, as in
        // Acrobat; there's no way to lose what was typed except Undo.
        let sub = cx.subscribe(&input, move |this, _, _: &TextAreaEvent, cx| {
            this.finish_annot_editor(cx);
        });
        let focus = input.read(cx).focus_handle(cx);
        let blur = cx.on_focus_out(&focus, window, move |this, _, _, cx| {
            this.finish_annot_editor(cx);
        });
        window.focus(&focus, cx);
        doc.annot_editor = Some(AnnotEditor {
            page,
            id,
            input,
            _sub: sub,
            _blur: blur,
        });
        doc.selected_annot = Some((page, id));
        cx.notify();
    }

    /// Close the inline editor, saving its text if it changed.
    fn finish_annot_editor(&mut self, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let Some(ed) = doc.annot_editor.take() else {
            return;
        };
        let text = ed.input.read(cx).text().to_string();
        if let Some(cur) = doc
            .model
            .state()
            .annots_on(ed.page)
            .and_then(|l| l.iter().find(|a| a.id == ed.id).cloned())
            && cur.contents != text
        {
            let mut upd = cur;
            upd.contents = text;
            upd.modified = sheaf_doc::pdf_date_now();
            self.apply(
                Edit::AnnotUpdate {
                    page: ed.page,
                    annot: upd,
                },
                "Edit text",
                cx,
            );
        }
        self.pending_focus = true;
        cx.notify();
    }

    // ----- insert pdf ---------------------------------------------------------

    fn insert_pdf_prompt(&mut self, cx: &mut Context<Self>) {
        let rx = crate::file_dialog::open(
            cx,
            gpui::PathPromptOptions {
                files: true,
                directories: false,
                multiple: true,
                prompt: Some("Insert".into()),
            },
        );
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                this.update(cx, |view, cx| {
                    for p in paths {
                        view.insert_pdf(p, cx);
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    /// Insert every page of `path` after the current page (or after the
    /// selection), merging its bookmarks PDFlyer-style.
    pub fn insert_pdf(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let targets = self.target_ids();
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        // Insert after the last targeted page: "before" is the one after it.
        let last_target = targets.last().and_then(|id| doc.model.index_of(*id));
        let before = last_target.and_then(|i| doc.page_ref(i + 1)).map(|p| p.id);
        doc.notice = Some(format!("Inserting {}…", path.display()));
        cx.notify();
        let timing = self.timing;
        cx.spawn(async move |this, cx| {
            let t = Instant::now();
            let opened = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    async move {
                        let bytes = std::fs::metadata(&path)
                            .map(|m| m.len() as usize)
                            .unwrap_or(0);
                        let handle = RenderHandle::open(path, None)?;
                        anyhow::Ok((handle, bytes))
                    }
                })
                .await;
            let (handle, bytes) = match opened {
                Ok(v) => v,
                Err(e) => {
                    this.update(cx, |view, cx| {
                        if let State::Ready(doc) = &mut view.state {
                            doc.notice = Some(format!("Insert failed: {e:#}"));
                        }
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            // Bookmarks come from the worker before we apply the edit, so the
            // insert and its merge are one undo step.
            let outline = handle
                .outline()
                .await
                .ok()
                .and_then(|r| r.ok())
                .unwrap_or_default();
            let sizes_rx = handle.page_sizes();
            this.update(cx, |view, cx| {
                let State::Ready(doc) = &mut view.state else {
                    return;
                };
                let (page_count, first_page) = (handle.info().page_count, handle.info().first_page);
                let sid = doc.model.add_source(SourceInfo {
                    path: path.clone(),
                    page_count,
                    first_page,
                    page_sizes: None,
                    bytes,
                    password: None,
                });
                doc.handles.insert(sid, handle);
                let flat: Vec<FlatOutlineItem> = outline.iter().map(flat_item).collect();
                let name = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("file")
                    .to_string();
                let label = format!("Insert {name}");
                let mut outcome = None;
                match doc.model.apply(
                    Edit::InsertPages {
                        source: sid,
                        range: 0..page_count,
                        before,
                        outline: Some(flat),
                    },
                    &label,
                ) {
                    Ok(out) => {
                        outcome = Some(out.clone());
                        doc.notice = Some(format!(
                            "Inserted {} page{} from {name}; {} bookmark{} merged",
                            out.pages_added,
                            if out.pages_added == 1 { "" } else { "s" },
                            out.bookmarks_added,
                            if out.bookmarks_added == 1 { "" } else { "s" }
                        ));
                        if timing {
                            eprintln!(
                                "[timing] inserted {name} ({page_count} pages) in {:?}",
                                t.elapsed()
                            );
                        }
                    }
                    Err(e) => doc.notice = Some(format!("Insert failed: {e:#}")),
                }
                doc.touched();
                match outcome {
                    Some(out) => view.after_model_change(label, out, cx),
                    None => cx.notify(),
                }
                // Sizes for the new source arrive on their own.
                cx.spawn(async move |this, cx| {
                    if let Ok(Ok(sizes)) = sizes_rx.await {
                        this.update(cx, |view, cx| {
                            if let State::Ready(doc) = &mut view.state {
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

    pub fn set_selftest_insert(&mut self, path: PathBuf) {
        self.selftest_insert = Some(path);
    }

    // ----- search -----------------------------------------------------------

    fn new_search(&mut self, cx: &mut Context<Self>) {
        let input = cx.new(|cx| TextInput::new("Find in document", cx));
        let sub = cx.observe(&input, |this, _, cx| this.on_search_input_changed(cx));
        let events = cx.subscribe(&input, |this, _, ev: &TextInputEvent, cx| match ev {
            TextInputEvent::Confirm => this.search_step(1, cx),
            TextInputEvent::Cancel => this.close_search_keep_focus(cx),
        });
        self.search = Some(Search {
            input,
            _input_sub: sub,
            _events_sub: events,
            query: String::new(),
            hits: Vec::new(),
            current: None,
            scanned: 0,
            done: true,
            _task: None,
        });
    }

    fn close_search_keep_focus(&mut self, cx: &mut Context<Self>) {
        self.search = None;
        self.pending_focus = true;
        cx.notify();
    }

    fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.is_none() {
            self.new_search(cx);
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
        // Unsaved OCR layers are invisible to PDFium; search them here.
        let seeded = doc.layer_hits(|l| sheaf_doc::text_layer::search(l, &query, false));
        let has_seed = !seeded.is_empty();
        search.hits = seeded;
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
                            SearchEvent::Hits(hits) => {
                                let first = search.hits.is_empty();
                                if let State::Ready(doc) = &view.state {
                                    doc.merge_source_hits(&mut search.hits, hits);
                                }
                                if first && !search.hits.is_empty() {
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
        if has_seed {
            self.go_to_hit(0, cx);
        }
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
        let Some(index) = doc.model.index_of(hit.page) else {
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

    pub(crate) fn scroll(&mut self, to: ScrollTo, cx: &mut Context<Self>) {
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
        view: i.view.clone(),
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
pub(crate) fn rotate_rect(r: Rect, src: PageSize, rot: Rotation) -> Rect {
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
    index: usize,
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
        if let Some(rec) = self.pending_recovery.take() {
            self.offer_recovery(rec, window, cx);
        }
        if let Some(page) = self.pending_edit_last.take() {
            self.start_annot_editor(page, window, cx);
        }
        let viewport = window.viewport_size();
        if std::mem::take(&mut self.pending_focus) {
            window.focus(&self.focus_handle, cx);
        }
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
        let toolbar = self.render_toolbar(cx);
        let page_cursor = self.page_cursor();
        let (tool, tool_color) = (self.tool, self.tool_color);
        let mut body = None;
        let mut wants: Vec<(PageRef, RenderRequest)> = Vec::new();
        let region = self.region.clone();
        let status: SharedString = match &mut self.state {
            State::Loading => "Opening…".into(),
            State::Failed(e) => format!("Failed: {e}").into(),
            State::Locked(l) => {
                if std::mem::take(&mut l.focus_pending) {
                    window.focus(&l.input.read(cx).focus_handle(cx), cx);
                }
                body = Some(
                    div()
                        .id("locked")
                        .size_full()
                        .child(locked_card(&self.path, l, cx)),
                );
                "Password required".into()
            }
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
                        ScrollTo::Point { page, fx, fy } => {
                            let p = page.min(last);
                            scroll_y = layout.tops[p] + fy * layout.heights[p];
                            let x = layout.lefts[p] + fx * layout.widths[p];
                            if x < scroll_x || x > scroll_x + view_w {
                                scroll_x = x;
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
                            index: i,
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
                        // Search highlights on top of everything. Hit rects
                        // are in the source page's displayed space.
                        if let Some(search) = &self.search
                            && !search.hits.is_empty()
                        {
                            let src = doc
                                .model
                                .source(pl.pref.source)
                                .map(|s| s.page_size(pl.pref.index))
                                .unwrap_or(PageSize {
                                    width: 612.0,
                                    height: 792.0,
                                });
                            let k = pl.w / src.rotated(pl.pref.rotation).width.max(1.0);
                            for (ix, hit) in search.hits.iter().enumerate() {
                                if hit.page != pl.pref.id {
                                    continue;
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
                        // Pattern hits, in their own color.
                        if let Some(ps) = &self.pattern
                            && !ps.hits.is_empty()
                        {
                            let src = doc
                                .model
                                .source(pl.pref.source)
                                .map(|s| s.page_size(pl.pref.index))
                                .unwrap_or(PageSize {
                                    width: 612.0,
                                    height: 792.0,
                                });
                            let k = pl.w / src.rotated(pl.pref.rotation).width.max(1.0);
                            for (ix, hit) in ps.hits.iter().enumerate() {
                                if hit.page != pl.pref.id {
                                    continue;
                                }
                                let current = ps.current == Some(ix);
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
                                                rgba(0xd040d0a0)
                                            } else {
                                                rgba(0x6aa6ff70)
                                            }),
                                    );
                                }
                            }
                        }
                        // Our markup, drawn over the page in the displayed orientation.
                        if let Some(list) = doc.model.state().annots_on(pl.pref.id) {
                            let src = doc
                                .model
                                .source(pl.pref.source)
                                .map(|s| s.page_size(pl.pref.index))
                                .unwrap_or(PageSize {
                                    width: 612.0,
                                    height: 792.0,
                                });
                            let k = pl.w / src.rotated(pl.pref.rotation).width.max(1.0);
                            for a in list.iter() {
                                page_div =
                                    page_div.children(annot_elements(a, src, pl.pref.rotation, k));
                            }
                        }
                        // Selection outline, inline editor, drag preview.
                        {
                            let src = doc
                                .model
                                .source(pl.pref.source)
                                .map(|s| s.page_size(pl.pref.index))
                                .unwrap_or(PageSize {
                                    width: 612.0,
                                    height: 792.0,
                                });
                            let k = pl.w / src.rotated(pl.pref.rotation).width.max(1.0);
                            if let Some((pid, aid)) = doc.selected_annot
                                && pid == pl.pref.id
                                && let Some(a) = doc
                                    .model
                                    .state()
                                    .annots_on(pid)
                                    .and_then(|l| l.iter().find(|a| a.id == aid))
                            {
                                let b = a.bounds();
                                let r = rotate_rect(
                                    Rect {
                                        x: b.x - 2.0,
                                        y: b.y - 2.0,
                                        w: b.w + 4.0,
                                        h: b.h + 4.0,
                                    },
                                    src,
                                    pl.pref.rotation,
                                );
                                page_div = page_div.child(
                                    div()
                                        .absolute()
                                        .left(px(r.x * k))
                                        .top(px(r.y * k))
                                        .w(px(r.w * k))
                                        .h(px(r.h * k))
                                        .border_1()
                                        .border_dashed()
                                        .border_color(rgb(0x4a9eff)),
                                );
                            }
                            if let Some(ed) = &doc.annot_editor
                                && ed.page == pl.pref.id
                                && let Some(a) = doc
                                    .model
                                    .state()
                                    .annots_on(ed.page)
                                    .and_then(|l| l.iter().find(|a| a.id == ed.id))
                            {
                                let b = a.bounds();
                                let r = rotate_rect(
                                    Rect {
                                        x: b.x,
                                        y: b.y + b.h + 2.0,
                                        w: b.w,
                                        h: 24.0,
                                    },
                                    src,
                                    pl.pref.rotation,
                                );
                                page_div = page_div.child(
                                    div()
                                        .absolute()
                                        .left(px(r.x * k))
                                        .top(px(r.y * k))
                                        .w(px(300.0))
                                        .child(ed.input.clone()),
                                );
                            }
                            if let Some(f) =
                                doc.selected_foreign.and_then(|i| doc.foreign_annots.get(i))
                                && pl.pref.source == doc.model.primary()
                                && f.page == pl.pref.index
                            {
                                page_div = page_div.child(crate::comments_panel::foreign_outline(
                                    f,
                                    src,
                                    pl.pref.rotation,
                                    k,
                                ));
                            }
                            if let Some(overlay) = crate::region::overlay(
                                region.as_ref(),
                                pl.pref.id,
                                src,
                                pl.pref.rotation,
                                k,
                            ) {
                                page_div = page_div.child(overlay);
                            }
                            if let Some(preview) = DocView::drag_preview(
                                doc,
                                tool,
                                tool_color,
                                pl.index,
                                src,
                                pl.pref.rotation,
                                k,
                            ) {
                                page_div = page_div.child(preview);
                            }
                            if let Some(preview) = DocView::pending_connector_preview(
                                doc,
                                tool_color,
                                pl.pref.id,
                                src,
                                pl.pref.rotation,
                                k,
                            ) {
                                page_div = page_div.child(preview);
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
                        .cursor(page_cursor)
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(|v, ev, window, cx| v.on_page_mouse_down(ev, window, cx)),
                        )
                        .on_mouse_move(
                            cx.listener(|v, ev, window, cx| v.on_page_mouse_move(ev, window, cx)),
                        )
                        .on_mouse_up(
                            gpui::MouseButton::Left,
                            cx.listener(|v, ev, window, cx| v.on_page_mouse_up(ev, window, cx)),
                        )
                        .on_mouse_up_out(
                            gpui::MouseButton::Left,
                            cx.listener(|v, ev, window, cx| v.on_page_mouse_up(ev, window, cx)),
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
        let ocr_progress = self.ocr_progress();

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
                if v.clear_region(cx) {
                    // Esc drops a selected region first.
                } else if v.search.is_some() {
                    v.close_search(window, cx)
                } else if v.cancel_ocr(cx) {
                    // Esc during a Recognize Text run stops it.
                } else if v.cancel_tape_placing(cx) || v.cancel_pending_connector(cx) {
                    // Esc with a tape or connector half-placed just drops it.
                } else {
                    if let State::Ready(doc) = &mut v.state {
                        doc.selected_annot = None;
                        doc.selected_foreign = None;
                        doc.drag = None;
                    }
                    if v.tool != Tool::Select {
                        v.set_tool(Tool::Select, cx);
                    }
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
            .on_action(cx.listener(|v, _: &DeletePage, _, cx| {
                if !v.delete_selected_annot(cx) {
                    v.delete_current(cx)
                }
            }))
            .on_action(cx.listener(|v, _: &DuplicatePages, _, cx| v.duplicate_current(cx)))
            .on_action(cx.listener(|v, _: &SelectAllPages, _, cx| v.select_all(cx)))
            .on_action(cx.listener(|v, _: &Save, _, cx| v.save(cx).detach()))
            .on_action(cx.listener(|v, _: &SaveAs, _, cx| v.save_as(cx)))
            .on_action(cx.listener(|v, _: &ExportCleanCopy, _, cx| v.export_clean_copy_prompt(cx)))
            .on_action(cx.listener(|v, _: &ExportFlattened, _, cx| v.export_flattened_prompt(cx)))
            .on_action(cx.listener(|v, _: &ExtractPages, _, cx| v.extract_pages_prompt(cx)))
            .on_action(cx.listener(|v, _: &InsertPdf, _, cx| v.insert_pdf_prompt(cx)))
            .on_action(cx.listener(|v, _: &ToolSelect, _, cx| v.set_tool(Tool::Select, cx)))
            .on_action(cx.listener(|v, _: &ToolNote, _, cx| v.set_tool(Tool::Note, cx)))
            .on_action(cx.listener(|v, _: &ToolHighlight, _, cx| v.set_tool(Tool::Highlight, cx)))
            .on_action(cx.listener(|v, _: &ToolFreeText, _, cx| v.set_tool(Tool::FreeText, cx)))
            .on_action(cx.listener(|v, _: &ToolSquare, _, cx| v.set_tool(Tool::Square, cx)))
            .on_action(cx.listener(|v, _: &ToolEllipse, _, cx| v.set_tool(Tool::Ellipse, cx)))
            .on_action(cx.listener(|v, _: &ToolLine, _, cx| v.set_tool(Tool::Line, cx)))
            .on_action(cx.listener(|v, _: &ToolArrow, _, cx| v.set_tool(Tool::Arrow, cx)))
            .on_action(cx.listener(|v, _: &ToolInk, _, cx| v.set_tool(Tool::Ink, cx)))
            .on_action(cx.listener(|v, _: &ToolTick, _, cx| v.set_tool(Tool::Tick, cx)))
            .on_action(cx.listener(|v, _: &ToolConnector, _, cx| v.set_tool(Tool::Connector, cx)))
            .on_action(cx.listener(|v, _: &NewBookmark, window, cx| v.new_bookmark(window, cx)))
            .on_action(cx.listener(|v, _: &Undo, _, cx| v.undo(cx)))
            .on_action(cx.listener(|v, _: &Redo, _, cx| v.redo(cx)))
            // Only while a run exists, so the menu item greys out otherwise.
            .when(self.ocr_running(), |d| {
                d.on_action(cx.listener(|v, _: &CancelOcr, _, cx| {
                    v.cancel_ocr(cx);
                }))
            })
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x2b2b2b))
            .text_color(rgb(0xd0d0d0))
            .children(search_bar)
            .child(toolbar)
            .child(div().flex_grow(1.0).min_h(px(0.0)).children(body))
            .when_some(ocr_progress, |d, frac| {
                d.child(
                    div()
                        .h(px(3.0))
                        .flex_shrink_0()
                        .bg(rgb(0x1e1e1e))
                        .child(div().h_full().w(relative(frac)).bg(rgb(0x4a9eff))),
                )
            })
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

/// GPUI elements for one annotation, positioned in page-local logical px.
/// `src` is the source page size, `rot` the page's extra rotation, `k` logical
/// px per point.
pub(crate) fn annot_elements(a: &Annot, src: PageSize, rot: Rotation, k: f32) -> Vec<AnyElement> {
    let rgb_u32 = ((a.color[0] as u32) << 16) | ((a.color[1] as u32) << 8) | a.color[2] as u32;
    let color = rgb(rgb_u32);
    let alpha = (a.opacity.clamp(0.0, 1.0) * 255.0) as u32;
    let fill = rgba((rgb_u32 << 8) | alpha);
    let place = |b: Box2| {
        let r = rotate_rect(
            Rect {
                x: b.x,
                y: b.y,
                w: b.w,
                h: b.h,
            },
            src,
            rot,
        );
        div()
            .absolute()
            .left(px(r.x * k))
            .top(px(r.y * k))
            .w(px(r.w * k))
            .h(px(r.h * k))
    };
    let mut out = Vec::new();
    match &a.kind {
        AnnotKind::Highlight { rects } => {
            for r in rects {
                out.push(place(*r).bg(fill).into_any_element());
            }
        }
        AnnotKind::Note { pos } => {
            let b = Box2 {
                x: pos.x,
                y: pos.y,
                w: sheaf_doc::annot::NOTE_SIZE,
                h: sheaf_doc::annot::NOTE_SIZE,
            };
            out.push(
                place(b)
                    .bg(fill)
                    .border_1()
                    .border_color(rgb(0x000000))
                    .rounded_sm()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(0x000000))
                    .text_size(px((NOTE_GLYPH * k).max(6.0)))
                    .child("…")
                    .into_any_element(),
            );
        }
        AnnotKind::FreeText { rect, size } => {
            out.push(
                place(*rect)
                    .p(px(2.0 * k))
                    .overflow_hidden()
                    .text_color(color)
                    .text_size(px(*size * k))
                    .line_height(px(*size * 1.2 * k))
                    .child(a.contents.clone())
                    .into_any_element(),
            );
        }
        AnnotKind::Square { rect } => {
            out.push(
                place(*rect)
                    .border(px((a.stroke * k).max(1.0)))
                    .border_color(color)
                    .into_any_element(),
            );
        }
        AnnotKind::Ellipse { rect } => {
            out.push(
                place(*rect)
                    .border(px((a.stroke * k).max(1.0)))
                    .border_color(color)
                    .rounded_full()
                    .into_any_element(),
            );
        }
        AnnotKind::Stamp { rect, glyph } => {
            out.push(
                place(*rect)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(color)
                    .text_size(px(rect.h * 0.8 * k))
                    .font_weight(gpui::FontWeight::BOLD)
                    .child(glyph.clone())
                    .into_any_element(),
            );
        }
        AnnotKind::Tape { rect, size } => {
            use sheaf_doc::annot::{TAPE_LINE, TAPE_OP_W, TAPE_PAD, tape_lines};
            let fs = *size * k;
            let rows: Vec<AnyElement> = tape_lines(&a.contents)
                .into_iter()
                .map(|(v, op)| {
                    let total = op == "T";
                    div()
                        .flex()
                        .items_center()
                        .justify_end()
                        .h(px(*size * TAPE_LINE * k))
                        .when(total, |d| {
                            d.border_t_1()
                                .border_color(rgb(0x606060))
                                .font_weight(gpui::FontWeight::BOLD)
                        })
                        .child(div().child(v))
                        .child(
                            div()
                                .w(px(*size * TAPE_OP_W * k))
                                .pl(px(fs * 0.2))
                                .child(op),
                        )
                        .into_any_element()
                })
                .collect();
            out.push(
                place(*rect)
                    .bg(rgb(0xffffff))
                    .border_1()
                    .border_color(rgb(0x999999))
                    .p(px(TAPE_PAD * k))
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .text_color(color)
                    .text_size(px(fs))
                    .line_height(px(*size * TAPE_LINE * k))
                    .children(rows)
                    .into_any_element(),
            );
        }
        AnnotKind::Connector { rect, label, .. } => {
            out.push(
                place(*rect)
                    .rounded_full()
                    .bg(fill)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(0xffffff))
                    .text_size(px(rect.h * 0.55 * k))
                    .font_weight(gpui::FontWeight::BOLD)
                    .child(label.clone())
                    .into_any_element(),
            );
        }
        AnnotKind::Line {
            a: pa,
            b: pb,
            arrow,
        } => {
            let (qa, qb) = (rotate_pt(*pa, src, rot), rotate_pt(*pb, src, rot));
            let (a2, b2) = ((qa.x * k, qa.y * k), (qb.x * k, qb.y * k));
            let mut paths = vec![vec![a2, b2]];
            if *arrow {
                paths.push(arrow_head(a2, b2, (6.0 + a.stroke * 2.0) * k));
            }
            out.push(stroke_canvas(
                paths,
                color,
                a.stroke * k,
                src.rotated(rot),
                k,
            ));
        }
        AnnotKind::Ink { paths } => {
            let polylines: Vec<Vec<(f32, f32)>> = paths
                .iter()
                .map(|p| {
                    p.iter()
                        .map(|q| {
                            let r = rotate_pt(*q, src, rot);
                            (r.x * k, r.y * k)
                        })
                        .collect()
                })
                .collect();
            out.push(stroke_canvas(
                polylines,
                color,
                a.stroke * k,
                src.rotated(rot),
                k,
            ));
        }
    }
    out
}

const NOTE_GLYPH: f32 = 12.0;

/// The password prompt shown in place of the pages for an encrypted file.
fn locked_card(path: &std::path::Path, l: &Locked, cx: &mut Context<DocView>) -> AnyElement {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("This file")
        .to_string();
    let input = l.input.clone();
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(380.0))
                .p_4()
                .flex()
                .flex_col()
                .gap_2()
                .bg(rgb(0x262626))
                .border_1()
                .border_color(rgb(0x4a4a4a))
                .rounded_md()
                .child(
                    div()
                        .text_sm()
                        .child(format!("\"{name}\" is password protected.")),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(if l.wrong {
                            rgb(0xe08080)
                        } else {
                            rgb(0x9a9a9a)
                        })
                        .child(if l.wrong {
                            "That password didn't work. Try again."
                        } else {
                            "Enter the password to open it."
                        }),
                )
                .child(input.clone())
                .child(
                    div().flex().justify_end().child(
                        div()
                            .id("unlock")
                            .px_3()
                            .py_1()
                            .rounded_sm()
                            .text_sm()
                            .cursor_pointer()
                            .bg(rgb(0x4a9eff))
                            .text_color(rgb(0xffffff))
                            .hover(|s| s.bg(rgb(0x5aaeff)))
                            .on_click(cx.listener(move |v, _, _, cx| {
                                let pw = input.read(cx).text().to_string();
                                if !pw.is_empty() {
                                    v.password = Some(pw);
                                    v.open(cx);
                                }
                            }))
                            .child("Open"),
                    ),
                ),
        )
        .into_any_element()
}
