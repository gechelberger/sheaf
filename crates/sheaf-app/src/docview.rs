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
use sheaf_ui::button::Button;
use sheaf_ui::list::ListLayout;
use sheaf_ui::text_area::{TextArea, TextAreaEvent};
use sheaf_ui::text_input::{TextInput, TextInputEvent};
use sheaf_ui::theme;

use crate::markup_tools::{Drag, Tool, arrow_head, rotate_pt, stroke_canvas, stroke_canvas_dashed};
pub(crate) use crate::pane::Zoom;
use crate::pane::{Layout, LayoutSpec, MARGIN, PageMode, Pane, Spread};
use crate::{ClearGuides, FullScreen, GoToPage, ReadMode, ToggleGrid, ToggleRulers};
use sheaf_doc::{
    Document, Edit, EditOutcome, FlatOutlineItem, OutlineId, PageId, PageRef, SourceId, SourceInfo,
};
use sheaf_image::raster::PixelFormat;
use sheaf_render::{
    Job, OutlineItem, Rect, RenderHandle, RenderRequest, Rendered, SearchEvent, SearchHit,
};

use crate::comments_panel::ExportKind;
use crate::{
    CancelOcr, CloseSearch, CopySelection, CycleTextMarkup, DeletePage, DuplicatePages,
    ExportCleanCopy, ExportFlattened, ExtractPages, Find, FirstPage, InsertPdf, LastPage, LineDown,
    LineUp, NewBookmark, NextPage, NextPane, PasteMarks, PasteMarksInPlace, PrevPage, Redo,
    RotatePageCcw, RotatePageCw, RotateViewCcw, RotateViewCw, Save, SaveAs, ScreenDown, ScreenUp,
    SearchNext, SearchPrev, SelectAllPages, SinglePage, SinglePageContinuous, SplitView,
    ToggleCheckmark, ToggleComments, ToggleCoverPage, ToolArrow, ToolConnector, ToolEllipse,
    ToolFreeText, ToolHighlight, ToolInk, ToolLine, ToolNote, ToolSelect, ToolSquare, ToolSquiggly,
    ToolStamp, ToolStrikeOut, ToolTextEdit, ToolTick, ToolUnderline, TwoUp, TwoUpContinuous, Undo,
    ZoomActual, ZoomFitPage, ZoomFitWidth, ZoomIn, ZoomOut,
};

pub const KEY_CONTEXT: &str = "DocView";

const STATUS_H: f32 = 24.0;
/// Height of the divider between split panes, logical px.
const SPLITTER_H: f32 = 5.0;
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
const ZOOM_STEP: f32 = 1.25;
const ZOOM_MIN: f32 = 0.1;
const ZOOM_MAX: f32 = 16.0;

/// Thumbnail panel geometry, logical px.
const THUMB_W: f32 = 150.0;
const THUMB_PAD: f32 = 10.0;
const THUMB_LABEL_H: f32 = 18.0;

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

type TileKey = (PageId, Job);

pub(crate) struct Doc {
    pub(crate) model: Document,
    pub(crate) handles: HashMap<SourceId, RenderHandle>,
    /// Bumped on every model change so layouts and lookups can tell.
    pub(crate) version: u64,
    /// One view of the document, or two when the window is split.
    pub(crate) panes: Vec<Pane>,
    /// The pane keys, menus and the status bar act on.
    pub(crate) active: usize,
    /// Low-res whole-page renders, by page and the rotation drawn: the
    /// thumbnails and a rotated view of the same page need both.
    pub(crate) previews: HashMap<(PageId, Rotation), Arc<RenderImage>>,
    pub(crate) tiles: HashMap<TileKey, Arc<RenderImage>>,
    /// The source page each page id showed when last checked: a page that
    /// now shows another (replaced, cropped, or undone) drops its images.
    pub(crate) shown: HashMap<PageId, (SourceId, u32)>,
    /// Dropping the task drops the oneshot receiver, which tells the worker
    /// to skip the render if it hasn't started.
    pub(crate) inflight: HashMap<TileKey, Task<()>>,
    /// Images we've stopped showing; GPU textures are released in `render`,
    /// the only place we have a `Window`.
    pub(crate) retired: Vec<Arc<RenderImage>>,
    pub(crate) first_paint: Option<Instant>,
    /// Last edit notice, shown in the status bar.
    pub(crate) notice: Option<String>,
    // Sidebar state.
    pub(crate) thumb_scroll: ScrollHandle,
    /// Pages whose previews the thumbnail panel is showing; kept cached.
    pub(crate) thumb_keep: HashSet<(PageId, Rotation)>,
    /// Previews the blank-page review is showing; kept cached.
    pub(crate) review_keep: HashSet<(PageId, Rotation)>,
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
    /// The marks selected, in the order picked. The last is the one the
    /// handles and the text editor are on ([`Doc::selected_annot`]).
    pub(crate) selected_marks: Vec<(PageId, AnnotId)>,
    /// What the Comments panel shows (and so the page): filter and sort.
    pub(crate) comment_filter: crate::comments_panel::CommentFilter,
    /// View > Hide Comments.
    pub(crate) comments_hidden: bool,
    /// The panel's view, and what it was made from.
    pub(crate) comment_view: Option<(
        crate::comments_panel::ViewKey,
        Arc<crate::comments_panel::CommentView>,
    )>,
    /// Our marks the comment filter hides on the page.
    pub(crate) hidden_marks: HashSet<AnnotId>,
    /// Other tools' comments sheaf has hidden in the engines: (source, page
    /// in it, slot in its /Annots).
    pub(crate) foreign_hidden: HashSet<(SourceId, u32, usize)>,
    /// Bumped when `foreign_annots` changes.
    pub(crate) foreign_generation: u64,
    /// Text selected with the Select tool (see text_select.rs).
    pub(crate) text_sel: Option<crate::text_select::TextSel>,
    pub(crate) text_drag: Option<crate::text_select::TextDrag>,
    /// Pages' characters, as read for selecting text, with what they were
    /// read from.
    pub(crate) page_chars:
        HashMap<PageId, (crate::text_select::CharsKey, Arc<sheaf_doc::PageChars>)>,
    /// Pages whose characters the engine is reading.
    pub(crate) chars_loading: HashSet<PageId>,
    pub(crate) annot_editor: Option<AnnotEditor>,
    /// A polygon, polyline or cloud being drawn (see shapes.rs).
    pub(crate) poly: Option<crate::shapes::PolyDraft>,
    /// The scale new measurements take (see `measure_tools`).
    pub(crate) measure_scale: sheaf_doc::measure::Scale,
    pub(crate) calibrate: crate::measure_tools::Calibrate,
    /// "Pages with" or "without" a group of marks (see groups.rs).
    pub(crate) group_list: Option<crate::groups::GroupList>,
    /// The right-click menu on a mark, where it was opened.
    pub(crate) mark_menu: Option<gpui::Point<gpui::Pixels>>,
    /// Comments other tools left in the files the pages come from
    /// (read-only), with the source each is in.
    pub(crate) foreign_annots: Vec<(SourceId, sheaf_doc::file::ForeignAnnot)>,
    pub(crate) selected_foreign: Option<usize>,
    /// Another tool's comment being dragged, made ours when it's let go.
    pub(crate) adopting: Option<usize>,
    /// Files embedded in the primary file, and its portfolio schema if it
    /// is one (see attachments_panel.rs). `facts_loaded` once read.
    pub(crate) attachments: Vec<sheaf_doc::attachments::Attachment>,
    pub(crate) collection: Option<sheaf_doc::portfolio::Collection>,
    pub(crate) attachment_selected: Option<usize>,
    pub(crate) facts_loaded: bool,
    /// Optional content groups and which are hidden (see layers_panel.rs).
    pub(crate) layers: Vec<sheaf_doc::layers::Layer>,
    pub(crate) layers_hidden: HashSet<sheaf_doc::layers::LayerId>,
    pub(crate) layers_default: HashSet<sheaf_doc::layers::LayerId>,
    pub(crate) layers_generation: u64,
    /// The primary file's marks that sheaf draws itself, as `/NM`s by page
    /// index: PDFium is told not to draw them, again whenever its renderer
    /// is reopened.
    pub(crate) drawn_marks: Vec<(u32, Vec<String>)>,
    /// Bookmark colors and styles from the file, when they arrived before
    /// the outline did.
    pub(crate) pending_outline_styles: Option<Vec<sheaf_doc::file::OutlineStyle>>,
    /// Every page's label (from the model), and the version it's for.
    pub(crate) labels: std::cell::RefCell<(u64, Option<std::sync::Arc<Vec<String>>>)>,
    /// With two panes: the top pane's share of the height.
    pub(crate) split: f32,
    /// The split divider is being dragged.
    pub(crate) split_drag: bool,
}

/// What an inserted or replacing file brings besides its pages, read on
/// a background thread: sheaf's own marks (which the edit carries in),
/// other tools' comments, and its bookmarks' colors and styles.
#[derive(Default)]
pub(crate) struct Incoming {
    pub marks: HashMap<u32, Vec<sheaf_doc::Annot>>,
    pub foreign: Vec<sheaf_doc::file::ForeignAnnot>,
    pub outline_styles: Vec<sheaf_doc::file::OutlineStyle>,
}

impl Incoming {
    pub(crate) fn read(path: &std::path::Path) -> Incoming {
        match sheaf_doc::file::read_open(path, None) {
            Ok((annots, facts)) => Incoming {
                marks: annots.sheaf,
                foreign: annots.foreign,
                outline_styles: facts.outline_styles,
            },
            Err(e) => {
                log::warn!("reading marks in {}: {e:#}", path.display());
                Incoming::default()
            }
        }
    }

    /// The marks, as `Edit::InsertPages` and `ReplacePages` carry them.
    pub(crate) fn edit_marks(&self) -> Vec<(u32, Vec<sheaf_doc::Annot>)> {
        self.marks.iter().map(|(i, l)| (*i, l.clone())).collect()
    }
}

impl Doc {
    /// The mark picked last, if any: the one with handles.
    pub(crate) fn selected_annot(&self) -> Option<(PageId, AnnotId)> {
        self.selected_marks.last().copied()
    }

    /// Select just this mark, or nothing.
    pub(crate) fn select_mark(&mut self, mark: Option<(PageId, AnnotId)>) {
        self.selected_marks = mark.into_iter().collect();
    }

    /// Shift+click: add the mark to the selection, or take it out.
    pub(crate) fn toggle_mark(&mut self, mark: (PageId, AnnotId)) {
        match self.selected_marks.iter().position(|m| *m == mark) {
            Some(i) => {
                self.selected_marks.remove(i);
            }
            None => self.selected_marks.push(mark),
        }
    }

    /// The selected marks themselves, with their pages.
    pub(crate) fn selected_mark_list(&self) -> Vec<(PageId, sheaf_doc::Annot)> {
        let st = self.model.state();
        self.selected_marks
            .iter()
            .filter_map(|(p, id)| {
                let a = st.annots_on(*p)?.iter().find(|a| a.id == *id)?.clone();
                Some((*p, a))
            })
            .collect()
    }

    /// After `source`'s pages came in with `incoming`: PDFium stops drawing
    /// the marks sheaf now draws, and the file's other comments are listed.
    pub(crate) fn take_in(&mut self, source: SourceId, incoming: Incoming) {
        if !incoming.marks.is_empty()
            && let Some(handle) = self.handles.get(&source)
        {
            drop(handle.hide_annots(marks_by_page(&incoming.marks)));
        }
        self.foreign_annots
            .extend(incoming.foreign.into_iter().map(|f| (source, f)));
        self.foreign_generation += 1;
    }
    pub(crate) fn primary(&self) -> &RenderHandle {
        &self.handles[&self.model.primary()]
    }

    pub(crate) fn page_ref(&self, index: usize) -> Option<PageRef> {
        self.model.page(index).copied()
    }

    pub(crate) fn pane(&self) -> &Pane {
        &self.panes[self.active]
    }

    pub(crate) fn pane_mut(&mut self) -> &mut Pane {
        &mut self.panes[self.active]
    }

    /// The active pane's current page.
    pub(crate) fn current_page(&self) -> usize {
        self.pane().current_page
    }

    /// Page `index` as the active pane shows it (with rotate view): for
    /// mapping between page and screen, never for edits.
    pub(crate) fn shown_ref(&self, index: usize) -> Option<PageRef> {
        self.page_ref(index).map(|p| self.pane().shown(p))
    }

    /// Scroll the active pane.
    pub(crate) fn scroll_to(&mut self, to: ScrollTo) {
        self.pane_mut().pending_scroll = Some(to);
    }

    /// Display index of the primary source's page `index`, if it's still in
    /// the document (search hits are reported in source pages).
    /// Where page `index` of `source` is shown, if it is.
    pub(crate) fn index_of_page_in(&self, source: SourceId, index: u32) -> Option<usize> {
        self.model
            .state()
            .pages
            .iter()
            .position(|p| p.source == source && p.index == index)
    }

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
        for p in &mut self.panes {
            p.layout = None;
        }
        self.drop_stale_images();
    }

    /// Forget images of pages that now show a different source page.
    fn drop_stale_images(&mut self) {
        let mut stale: HashSet<PageId> = HashSet::new();
        for p in self.model.state().pages.iter() {
            let now = (p.source, p.index);
            if let Some(was) = self.shown.insert(p.id, now)
                && was != now
            {
                stale.insert(p.id);
            }
        }
        if stale.is_empty() {
            return;
        }
        let tiles: Vec<TileKey> = self
            .tiles
            .keys()
            .filter(|k| stale.contains(&k.0))
            .copied()
            .collect();
        for k in tiles {
            if let Some(im) = self.tiles.remove(&k) {
                self.retired.push(im);
            }
        }
        let previews: Vec<(PageId, Rotation)> = self
            .previews
            .keys()
            .filter(|k| stale.contains(&k.0))
            .copied()
            .collect();
        for k in previews {
            if let Some(im) = self.previews.remove(&k) {
                self.retired.push(im);
            }
        }
        self.inflight.retain(|k, _| !stale.contains(&k.0));
    }

    /// Drop every rendered image so pages re-rasterize (after the engine's
    /// view of a page changed, e.g. annotations hidden).
    pub(crate) fn invalidate_tiles(&mut self) {
        for (_, im) in self.tiles.drain() {
            self.retired.push(im);
        }
        for (_, im) in self.previews.drain() {
            self.retired.push(im);
        }
        self.inflight.clear();
        self.version += 1;
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ScrollTo {
    /// The top of the row (page or spread) page `n` is in.
    Page(usize),
    By(f32),
    /// The end of the document.
    End,
    /// The top or bottom of what's laid out (in a non-continuous mode, the
    /// shown spread).
    Top,
    Bottom,
    /// In a non-continuous mode: the next spread's top, or the previous
    /// spread's bottom (scrolling on past an end).
    NextRow,
    PrevRowEnd,
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
    /// The document closed. Emitted by the workspace just before it drops the view.
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
    /// Focus to give the calculator's entry next frame (a note finished).
    pub(crate) pending_calc_focus: Option<Entity<sheaf_ui::text_input::TextInput>>,
    pub(crate) comments: Option<crate::comments_panel::Comments>,
    pub(crate) history: Option<crate::history_panel::History>,
    /// The icon the Attach File tool gives.
    pub(crate) attach_icon: String,
    /// The Connector tool's label; None for the next unused one.
    pub(crate) connector_label: Option<String>,
    pub(crate) stamps: Option<crate::stamps_panel::StampsPanel>,
    /// The stamp the Stamp tool places.
    pub(crate) stamp_choice: Option<sheaf_ops::stamps::StampRef>,
    pub(crate) pending_history_focus: bool,
    /// A Recognize Text run in progress (see ocr_run.rs).
    pub(crate) ocr: Option<crate::ocr_run::OcrRun>,
    /// A background job in progress: Export Pages, Split (see jobs.rs).
    pub(crate) job: Option<crate::jobs::Job>,
    /// Blank pages found, waiting for review (see blank_review.rs).
    pub(crate) blank_review: Option<crate::blank_review::Review>,
    /// A plugin command running on this document, with its progress
    /// (see plugins.rs).
    #[cfg_attr(not(feature = "plugins"), allow(dead_code))]
    pub(crate) plugin_run: Option<(String, sheaf_ops::Tracker)>,
    pub(crate) pending_comments_focus: bool,
    /// The Measure tools' scale bar, and whether to focus it next frame.
    pub(crate) measure_bar: Option<crate::measure_tools::MeasureBar>,
    pub(crate) pending_measure_focus: bool,
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
    /// Open the inline editor on this page's last mark next frame (a text
    /// edit just placed; dev hooks).
    pub(crate) pending_edit_last: Option<PageId>,
    /// Recovery data found when opening, until the user has been asked.
    pending_recovery: Option<sheaf_doc::recovery::Recovery>,
    autosave: Option<Task<()>>,
    /// When the edits since the last sidecar write began.
    autosave_since: Option<Instant>,
    pub(crate) tool: Tool,
    pub(crate) tool_color: [u8; 3],
    pub(crate) tick_glyph: String,
    /// The Select tool is over text (the pointer is an I-beam).
    pub(crate) text_hover: bool,
    /// Where the pointer last was over a page: page, point, and how near
    /// counts as on text.
    pub(crate) last_hover: Option<(PageId, sheaf_doc::Pt, f32)>,
    /// Set while a plugin's edits are applied, so its own edits aren't
    /// reported back to it.
    pub(crate) edit_source: Option<String>,
    /// Shift+drag region (see region.rs).
    pub(crate) region: Option<crate::region::Region>,
    /// Full Screen / Read Mode (see presentation.rs).
    pub(crate) presentation: crate::presentation::Presentation,
    pub(crate) saved_view: Option<crate::presentation::SavedView>,
    /// A dock panel this document asks the workspace to show (by panel id),
    /// e.g. Attachments when a portfolio opens.
    pub(crate) wanted_panel: Option<&'static str>,
    /// View > Go to Page, while open.
    pub(crate) goto: Option<crate::goto::GoTo>,
    /// View > Rulers / Grid, and the guides dragged out of the rulers.
    pub(crate) rulers: bool,
    pub(crate) grid: bool,
    pub(crate) guides: Vec<crate::rulers::Guide>,
    pub(crate) guide_drag: Option<crate::rulers::GuideDrag>,
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
            pending_calc_focus: None,
            comments: None,
            ocr: None,
            job: None,
            blank_review: None,
            plugin_run: None,
            pending_comments_focus: false,
            measure_bar: None,
            pending_measure_focus: false,
            history: None,
            connector_label: None,
            attach_icon: "PushPin".into(),
            stamps: None,
            stamp_choice: None,
            pending_history_focus: false,
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
            text_hover: false,
            last_hover: None,
            edit_source: None,
            region: None,
            presentation: Default::default(),
            saved_view: None,
            goto: None,
            wanted_panel: None,
            rulers: false,
            grid: false,
            guides: Vec::new(),
            guide_drag: None,
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
            data: Some(sheaf_doc::SourceData(handle.data())),
        };
        let mut model = Document::new(source, &[], sheaf_doc::history::default_policy());
        model.set_author(crate::prefs::get(cx).identity.name());
        let opened = self.path.clone();
        cx.defer(move |cx| crate::recent::add(&opened, cx));
        let mut handles = HashMap::new();
        handles.insert(model.primary(), handle);
        self.state = State::Ready(Box::new(Doc {
            model,
            handles,
            version: 0,
            panes: vec![Pane::new(Zoom::FitWidth, PageMode::default())],
            active: 0,
            previews: HashMap::new(),
            tiles: HashMap::new(),
            shown: HashMap::new(),
            inflight: HashMap::new(),
            retired: Vec::new(),
            first_paint: None,
            notice: None,
            thumb_scroll: ScrollHandle::new(),
            thumb_keep: HashSet::new(),
            review_keep: HashSet::new(),
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
            selected_marks: Vec::new(),
            comment_filter: Default::default(),
            comments_hidden: false,
            comment_view: None,
            hidden_marks: HashSet::new(),
            foreign_hidden: HashSet::new(),
            foreign_generation: 0,
            text_sel: None,
            text_drag: None,
            page_chars: HashMap::new(),
            chars_loading: HashSet::new(),
            annot_editor: None,
            poly: None,
            measure_scale: crate::state::get(cx)
                .measure_scale
                .clone()
                .unwrap_or_default(),
            calibrate: Default::default(),
            group_list: None,
            mark_menu: None,
            foreign_annots: Vec::new(),
            selected_foreign: None,
            adopting: None,
            labels: std::cell::RefCell::new((u64::MAX, None)),
            attachments: Vec::new(),
            collection: None,
            attachment_selected: None,
            facts_loaded: false,
            layers: Vec::new(),
            layers_hidden: HashSet::new(),
            layers_default: HashSet::new(),
            layers_generation: 0,
            drawn_marks: Vec::new(),
            pending_outline_styles: None,
            split: 0.5,
            split_drag: false,
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
        let rx = sheaf_ui::prompts::ask(
            window,
            cx,
            gpui::PromptLevel::Warning,
            &format!("Recover unsaved changes to \"{name}\"?"),
            Some(
                "sheaf found edits that weren't saved when it last closed. Recovered changes stay unsaved until you save.",
            ),
            &[
                gpui::PromptButton::ok("Recover"),
                gpui::PromptButton::new("Discard"),
            ],
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
                        let mut flat: Vec<FlatOutlineItem> = items.iter().map(flat_item).collect();
                        if let Some(styles) = doc.pending_outline_styles.take() {
                            sheaf_doc::file::style_flat_items(&mut flat, &styles);
                        }
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
    pub(crate) fn apply_batch(&mut self, edits: Vec<Edit>, label: &str, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let before: HashSet<OutlineId> = doc
            .model
            .state()
            .outline_flat()
            .iter()
            .map(|(_, n)| n.id)
            .collect();
        let result = doc.model.apply_batch(edits, label);
        if result.is_ok() {
            reveal_new_bookmarks(
                &doc.model.state().outline,
                &before,
                &mut doc.outline_expanded,
            );
        }
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

    /// What a plugin command sees of this document; `ocr` is the OCR
    /// choices it runs with.
    #[cfg(feature = "plugins")]
    pub(crate) fn plugin_context(
        &self,
        ocr: sheaf_ocr::OcrConfig,
    ) -> Option<sheaf_plugin::DocContext> {
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
            current_page: doc.page_ref(doc.current_page()).map(|p| p.id),
            text: crate::plugins::text_source(&doc.handles),
            ocr: Some(crate::plugins::ocr_source(&doc.handles, ocr)),
            render: Some(crate::plugins::render_source(&doc.handles)),
        })
    }

    /// Pages an operation applies to: the selection, or the current page.
    pub(crate) fn target_ids(&self) -> Vec<PageId> {
        let State::Ready(doc) = &self.state else {
            return Vec::new();
        };
        if doc.selection.is_empty() {
            return doc
                .page_ref(doc.current_page())
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
        let st = doc.model.state().clone();
        doc.selected_marks.retain(|(pid, aid)| {
            st.annots_on(*pid)
                .is_some_and(|l| l.iter().any(|a| a.id == *aid))
        });
        let live: HashSet<PageId> = doc.model.state().pages.iter().map(|p| p.id).collect();
        if doc.text_sel.is_some_and(|s| !live.contains(&s.page)) {
            doc.text_sel = None;
        }
        doc.page_chars.retain(|id, _| live.contains(id));
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
                .unwrap_or(doc.current_page())
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

    /// Sign the versions made from now on with the name in `settings.toml`.
    pub(crate) fn sync_author(&mut self, cx: &mut Context<Self>) {
        if let State::Ready(doc) = &mut self.state {
            doc.model.set_author(crate::prefs::get(cx).identity.name());
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
                        let (strategy, notes) = save::save_to_path_noting(&snapshot, &path)?;
                        let bytes = std::fs::metadata(&path)
                            .map(|m| m.len() as usize)
                            .unwrap_or(0);
                        let handle = RenderHandle::open(path.clone(), None)?;
                        anyhow::Ok((handle, bytes, strategy, notes))
                    }
                })
                .await;
            this.update(cx, |view, cx| {
                let State::Ready(doc) = &mut view.state else {
                    return false;
                };
                doc.saving = false;
                let ok = match result {
                    Ok((handle, bytes, strategy, notes)) => {
                        let (page_count, first_page) =
                            (handle.info().page_count, handle.info().first_page);
                        let saved = SourceInfo {
                            path: path.clone(),
                            page_count,
                            first_page,
                            page_sizes: None,
                            bytes,
                            password: None,
                            data: Some(sheaf_doc::SourceData(handle.data())),
                        };
                        let sid = doc.model.rebase_to_saved(saved);
                        // Our marks are now in the file; keep PDFium from drawing them.
                        let st = doc.model.state();
                        let pages: Vec<(u32, Vec<String>)> = st
                            .pages
                            .iter()
                            .enumerate()
                            .filter_map(|(i, p)| {
                                let list = st.annots.get(&p.id)?;
                                Some((i as u32, list.iter().flat_map(|a| a.pdf_names()).collect()))
                            })
                            .collect();
                        doc.drawn_marks = pages.clone();
                        // The saved file's third-party comments, at their
                        // new page numbers.
                        doc.foreign_annots.clear();
                        doc.foreign_generation += 1;
                        // A new engine: nothing of theirs is hidden in it.
                        doc.foreign_hidden.retain(|(s, _, _)| *s != sid);
                        reread_foreign(sid, path.clone(), cx);
                        if !pages.is_empty() {
                            let rx = handle.hide_annots(pages);
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
                        let saved_path = path.clone();
                        cx.defer(move |cx| crate::recent::add(&saved_path, cx));
                        let was_encrypted = view.password.take().is_some();
                        doc.notice = Some(format!(
                            "Saved {} ({strategy:?}) in {:?}{}{}",
                            path.display(),
                            t.elapsed(),
                            if was_encrypted {
                                " — password protection removed"
                            } else {
                                ""
                            },
                            notes.iter().map(|n| format!(" — {n}")).collect::<String>()
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
                        view.refresh_facts(cx);
                        if let State::Ready(doc) = &view.state
                            && doc.layers_changed()
                        {
                            view.apply_layers(cx);
                        }
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
                .spawn(async move { sheaf_doc::file::read_open(&path, password.as_deref()) })
                .await;
            let (by_index, foreign, facts) = match found {
                Ok((a, facts)) => (a.sheaf, a.foreign, facts),
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
                let primary = doc.model.primary();
                doc.foreign_annots = foreign.into_iter().map(|f| (primary, f)).collect();
                doc.foreign_generation += 1;
                if !facts.outline_styles.is_empty() {
                    if doc.outline_loaded {
                        if let Err(e) = doc.model.set_initial_outline_styles(&facts.outline_styles)
                        {
                            log::warn!("bookmark colors arrived after edits; ignored: {e:#}");
                        }
                    } else {
                        doc.pending_outline_styles = Some(facts.outline_styles.clone());
                    }
                }
                if let Some(ranges) = &facts.label_ranges
                    && let Err(e) = doc.model.set_initial_labels(ranges)
                {
                    log::warn!("page labels arrived after edits; ignored: {e:#}");
                }
                if let Some((hf, scope)) = facts.header_footer.clone()
                    && let Err(e) = doc.model.set_initial_header_footer(hf, scope)
                {
                    log::warn!("header/footer settings arrived after edits; ignored: {e:#}");
                }
                // The labels changed under the same version.
                doc.labels.borrow_mut().0 = u64::MAX;
                doc.attachments = facts.attachments;
                doc.collection = facts.collection;
                doc.set_layers(facts.layers);
                doc.facts_loaded = true;
                if doc.collection.is_some() {
                    doc.open_portfolio();
                    view.wanted_panel = Some("attachments");
                }
                if by_index.is_empty() {
                    cx.notify();
                    return;
                }
                let drawn = marks_by_page(&by_index);
                let n: usize = by_index.values().map(|v| v.len()).sum();
                match doc.model.set_initial_annots(by_index) {
                    Ok(()) => {
                        if timing {
                            eprintln!(
                                "[timing] loaded {n} sheaf annotations on {} pages in {:?}",
                                drawn.len(),
                                t.elapsed()
                            );
                        }
                        doc.drawn_marks = drawn.clone();
                        let rx = doc.primary().hide_annots(drawn);
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
    /// A new mark by whoever `settings.toml` names (the OS user otherwise).
    pub(crate) fn new_annot(&self, kind: AnnotKind, color: [u8; 3], cx: &App) -> Annot {
        Annot {
            id: sheaf_doc::AnnotId(0),
            kind,
            color,
            opacity: 1.0,
            stroke: 1.5,
            author: crate::prefs::get(cx).identity.name(),
            contents: String::new(),
            modified: sheaf_doc::pdf_date_now(),
            created: sheaf_doc::pdf_date_now(),
            fill: None,
            dashed: false,
            cloudy: false,
            name: String::new(),
        }
    }

    pub(crate) fn add_annot(
        &mut self,
        page: PageId,
        annot: Annot,
        label: &str,
        cx: &mut Context<Self>,
    ) {
        let count = |v: &Self| match &v.state {
            State::Ready(doc) => doc.model.state().annots_on(page).map_or(0, |l| l.len()),
            _ => 0,
        };
        let before = count(self);
        self.apply(Edit::AnnotAdd { page, annot }, label, cx);
        // The new mark comes in selected, so its properties can be set
        // straight away.
        if count(self) > before
            && let State::Ready(doc) = &mut self.state
            && let Some(id) = doc
                .model
                .state()
                .annots_on(page)
                .and_then(|l| l.last().map(|a| a.id))
        {
            doc.select_mark(Some((page, id)));
        }
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
            cx,
        );
        hl.opacity = 0.6;
        self.add_annot(p0, hl, "Highlight", cx);
        let mut note = self.new_annot(
            AnnotKind::Note {
                pos: Pt { x: 400.0, y: 60.0 },
            },
            [255, 200, 0],
            cx,
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
            cx,
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
            cx,
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
            cx,
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
            cx,
        );
        self.add_annot(p0, tick2, "Tickmark", cx);
        let arrow = self.new_annot(
            AnnotKind::Line {
                a: Pt { x: 80.0, y: 220.0 },
                b: Pt { x: 300.0, y: 260.0 },
                arrow: true,
                arrow_start: false,
            },
            [0, 120, 255],
            cx,
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
            cx,
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
                cx,
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
                tape: Default::default(),
                look: Default::default(),
            },
            [20, 20, 20],
            cx,
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
            cx,
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
            cx,
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
            cx,
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
            cx,
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
            cx,
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
        // Flattening keeps an attachment's icon and drops its file, as
        // Acrobat does: say so.
        let files = snapshot
            .state
            .annots
            .values()
            .flatten()
            .filter(|a| matches!(a.kind, AnnotKind::FileAttachment { .. }))
            .count()
            + doc
                .attachments
                .iter()
                .filter(|a| matches!(a.at, sheaf_doc::attachments::AttachmentRef::Annot { .. }))
                .count();
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
                                "Exported flattened copy to {} ({n} mark{}){}",
                                path.display(),
                                if n == 1 { "" } else { "s" },
                                match files {
                                    0 => String::new(),
                                    1 => " — the attachment's icon is kept, its file isn't".into(),
                                    _ =>
                                        " — attachments' icons are kept, their files aren't".into(),
                                }
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
        let hint = match annot.kind {
            AnnotKind::Replace { .. } => "Type the replacement; Ctrl+Enter or click away when done",
            AnnotKind::Insert { .. } => {
                "Type the text to insert; Ctrl+Enter or click away when done"
            }
            _ => "Type a note; Ctrl+Enter or click away when done",
        };
        let input = cx.new(|cx| {
            let mut i = TextArea::new(hint, cx).rows(3, 10);
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
        doc.select_mark(Some((page, id)));
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
        // An insertion with nothing to insert isn't one.
        if text.trim().is_empty()
            && let Some(cur) = doc
                .model
                .state()
                .annots_on(ed.page)
                .and_then(|l| l.iter().find(|a| a.id == ed.id))
            && matches!(cur.kind, AnnotKind::Insert { .. })
        {
            self.apply(
                Edit::AnnotDelete {
                    page: ed.page,
                    id: ed.id,
                },
                "Delete text insertion",
                cx,
            );
            self.pending_focus = true;
            return;
        }
        let State::Ready(doc) = &mut self.state else {
            return;
        };
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

    /// Where inserted pages go by default: after the current page, or
    /// after the last selected page. The page they go before (None: the
    /// end).
    pub(crate) fn insert_point(&self) -> Option<PageId> {
        let targets = self.target_ids();
        let State::Ready(doc) = &self.state else {
            return None;
        };
        let last_target = targets.last().and_then(|id| doc.model.index_of(*id));
        last_target.and_then(|i| doc.page_ref(i + 1)).map(|p| p.id)
    }

    /// Insert every page of `path` after the current page (or after the
    /// selection), merging its bookmarks PDFlyer-style.
    pub fn insert_pdf(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let before = self.insert_point();
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("file")
            .to_string();
        self.insert_file(path, before, name, true, cx);
    }

    /// Insert every page of `path` before `before` (None: at the end), as
    /// one undo step labelled "Insert `name`", with its bookmarks merged
    /// when `bookmarks`.
    pub(crate) fn insert_file(
        &mut self,
        path: PathBuf,
        before: Option<PageId>,
        name: String,
        bookmarks: bool,
        cx: &mut Context<Self>,
    ) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        doc.notice = Some(format!("Inserting {name}…"));
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
                        // Pages sheaf made (blank, images) carry nothing more.
                        let incoming = if bookmarks {
                            Incoming::read(&path)
                        } else {
                            Incoming::default()
                        };
                        let handle = RenderHandle::open(path, None)?;
                        anyhow::Ok((handle, bytes, incoming))
                    }
                })
                .await;
            let (handle, bytes, incoming) = match opened {
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
                    data: Some(sheaf_doc::SourceData(handle.data())),
                });
                doc.handles.insert(sid, handle);
                let mut flat: Vec<FlatOutlineItem> = if bookmarks {
                    outline.iter().map(flat_item).collect()
                } else {
                    Vec::new()
                };
                sheaf_doc::file::style_flat_items(&mut flat, &incoming.outline_styles);
                let label = format!("Insert {name}");
                let mut outcome = None;
                match doc.model.apply(
                    Edit::InsertPages {
                        source: sid,
                        range: 0..page_count,
                        before,
                        outline: Some(flat),
                        annots: incoming.edit_marks(),
                    },
                    &label,
                ) {
                    Ok(out) => {
                        outcome = Some(out.clone());
                        doc.take_in(sid, incoming);
                        doc.notice = Some(if !bookmarks {
                            // Pages sheaf made (blank, images): name says it all.
                            format!("Inserted {name}")
                        } else {
                            format!(
                                "Inserted {} page{} from {name}{}",
                                out.pages_added,
                                if out.pages_added == 1 { "" } else { "s" },
                                match out.bookmarks_added {
                                    0 => String::new(),
                                    1 => "; 1 bookmark merged".into(),
                                    n => format!("; {n} bookmarks merged"),
                                }
                            )
                        });
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
        let Some(pref) = doc.shown_ref(index) else {
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
            doc.scroll_to(ScrollTo::Reveal {
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
                .bg(theme::CONTROL)
                .hover(|s| s.bg(theme::CONTROL_HOVER))
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
                .bg(theme::PANEL)
                .border_b_1()
                .border_color(theme::DIVIDER)
                .child(div().w(px(320.0)).child(search.input.clone()))
                .child(
                    div()
                        .text_sm()
                        .text_color(theme::TEXT_MUTED)
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
            doc.scroll_to(to);
            cx.notify();
        }
    }

    /// Next/previous page, or spread in a two-up mode.
    fn turn_page(&mut self, delta: isize, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let n = doc.model.page_count();
        let mode = doc.pane().mode;
        let rows = mode.row_count(n);
        if rows == 0 {
            return;
        }
        let row = mode.row_of(doc.current_page()) as isize + delta;
        let row = row.clamp(0, rows as isize - 1) as usize;
        doc.scroll_to(ScrollTo::Page(mode.row_pages(row, n).start));
        cx.notify();
    }

    /// Window > Split: a second pane on the same document, or back to one
    /// (keeping the active pane's view).
    pub(crate) fn toggle_split(&mut self, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        if doc.panes.len() > 1 {
            let keep = doc.panes.swap_remove(doc.active);
            doc.panes = vec![keep];
            doc.active = 0;
            doc.notice = Some("Split view off".into());
        } else {
            let second = doc.pane().split();
            doc.panes.push(second);
            doc.active = 1;
            doc.notice = Some(
                "Split view: each pane scrolls and zooms on its own; Ctrl+\\ again to close".into(),
            );
        }
        cx.notify();
    }

    /// Make pane `ix` the one keys, menus and the status bar act on.
    fn activate_pane(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let State::Ready(doc) = &mut self.state
            && ix < doc.panes.len()
            && doc.active != ix
        {
            doc.active = ix;
            cx.notify();
        }
    }

    /// F6-style: the other pane becomes active.
    fn cycle_pane(&mut self, cx: &mut Context<Self>) {
        if let State::Ready(doc) = &self.state {
            let next = (doc.active + 1) % doc.panes.len();
            self.activate_pane(next, cx);
        }
    }

    fn drag_split(&mut self, y: gpui::Pixels, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        if !doc.split_drag || doc.panes.len() < 2 {
            return;
        }
        let (a, b) = (doc.panes[0].scroll.bounds(), doc.panes[1].scroll.bounds());
        let top = f32::from(a.origin.y);
        let bottom = f32::from(b.origin.y + b.size.height);
        if bottom - top < 1.0 {
            return;
        }
        doc.split = ((f32::from(y) - top) / (bottom - top)).clamp(0.1, 0.9);
        cx.notify();
    }

    fn end_split_drag(&mut self, cx: &mut Context<Self>) {
        if let State::Ready(doc) = &mut self.state
            && std::mem::take(&mut doc.split_drag)
        {
            cx.notify();
        }
    }

    /// Two-Up or Two-Up Continuous, keeping the cover page setting.
    fn set_two_up(&mut self, continuous: bool, cx: &mut Context<Self>) {
        let State::Ready(doc) = &self.state else {
            return;
        };
        let spread = match doc.pane().mode.spread {
            Spread::TwoUpCover => Spread::TwoUpCover,
            _ => Spread::TwoUp,
        };
        self.set_page_mode(PageMode { spread, continuous }, cx);
    }

    /// Height of the active pane's viewport, for page-at-a-time scrolling.
    fn pane_view_h(&self) -> f32 {
        match &self.state {
            State::Ready(doc) => doc
                .pane()
                .layout
                .as_ref()
                .map(|l| l.view_h)
                .unwrap_or(600.0),
            _ => 600.0,
        }
    }

    /// Change zoom, keeping the document point under `anchor` (viewport-local
    /// logical px; defaults to the viewport centre) where it is.
    fn set_zoom(&mut self, zoom: Zoom, anchor: Option<(f32, f32)>, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let pane = doc.pane_mut();
        if pane.zoom == zoom {
            return;
        }
        pane.pending_scroll = pane.layout.as_ref().and_then(|layout| {
            let (vx, vy) = anchor.unwrap_or((layout.view_w / 2.0, layout.view_h / 2.0));
            let sx = -f32::from(pane.scroll.offset().x);
            let sy = -f32::from(pane.scroll.offset().y);
            let (dx, dy) = (sx + vx, sy + vy);
            let page = layout.page_at(dx, dy)?;
            let r = layout.rect(page)?;
            Some(ScrollTo::Anchor {
                page,
                fx: (dx - r.left) / r.w,
                fy: (dy - r.top) / r.h,
                vx,
                vy,
            })
        });
        pane.zoom = zoom;
        cx.notify();
    }

    fn zoom_by(&mut self, factor: f32, anchor: Option<(f32, f32)>, cx: &mut Context<Self>) {
        let State::Ready(doc) = &self.state else {
            return;
        };
        let current = doc
            .pane()
            .layout
            .as_ref()
            .map(|l| l.zoom_percent() / 100.0)
            .unwrap_or(1.0);
        let z = (current * factor).clamp(ZOOM_MIN, ZOOM_MAX);
        self.set_zoom(Zoom::Custom(z), anchor, cx);
    }

    /// View > Page Display: the active pane's page mode. The page in view
    /// stays in view.
    pub(crate) fn set_page_mode(&mut self, mode: PageMode, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let page = doc.current_page();
        let pane = doc.pane_mut();
        if pane.mode == mode {
            return;
        }
        pane.mode = mode;
        pane.row = mode.row_of(page);
        pane.pending_scroll = Some(ScrollTo::Page(page));
        doc.notice = Some(format!("Page display: {}", mode.label()));
        cx.notify();
    }

    /// Show Cover Page in Two-Up: toggles between the two two-up spreads
    /// (switching to two-up from single page).
    fn toggle_cover_page(&mut self, cx: &mut Context<Self>) {
        let State::Ready(doc) = &self.state else {
            return;
        };
        let mut mode = doc.pane().mode;
        mode.spread = match mode.spread {
            Spread::TwoUpCover => Spread::TwoUp,
            Spread::TwoUp | Spread::Single => Spread::TwoUpCover,
        };
        self.set_page_mode(mode, cx);
    }

    /// Rotate View: turn the active pane's display, not the pages.
    fn rotate_view(&mut self, by: Rotation, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let page = doc.current_page();
        let pane = doc.pane_mut();
        pane.rotation = pane.rotation.plus(by);
        pane.pending_scroll = Some(ScrollTo::Page(page));
        let deg = pane.rotation.degrees();
        doc.notice = Some(if deg == 0 {
            "Rotate view: off".into()
        } else {
            format!("Rotate view: {deg}° (display only; Document > Rotate turns pages)")
        });
        cx.notify();
    }

    fn on_wheel(
        &mut self,
        ix: usize,
        ev: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        if ix >= doc.panes.len() {
            return;
        }
        let dy = f32::from(ev.delta.pixel_delta(window.line_height()).y);
        if dy == 0.0 {
            return;
        }
        if !ev.modifiers.control {
            // One spread at a time: scrolling on past either end turns the page.
            let pane = &mut doc.panes[ix];
            if pane.mode.continuous {
                return;
            }
            let Some(layout) = &pane.layout else { return };
            let max_y = (layout.total_h - layout.view_h).max(0.0);
            let sy = -f32::from(pane.scroll.offset().y);
            let turn = if dy < 0.0 && sy >= max_y - 0.5 {
                1
            } else if dy > 0.0 && sy <= 0.5 {
                -1
            } else {
                return;
            };
            if pane
                .last_turn
                .is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(350))
            {
                return;
            }
            pane.last_turn = Some(Instant::now());
            let to = if turn > 0 {
                ScrollTo::NextRow
            } else {
                ScrollTo::PrevRowEnd
            };
            pane.pending_scroll = Some(to);
            cx.notify();
            return;
        }
        // Zoom the pane under the mouse, anchored on the mouse: position
        // relative to its scroll container.
        doc.active = ix;
        let b = doc.pane().scroll.bounds();
        let anchor = (
            f32::from(ev.position.x - b.origin.x),
            f32::from(ev.position.y - b.origin.y),
        );
        let factor = if dy > 0.0 { 1.1 } else { 1.0 / 1.1 };
        self.zoom_by(factor, Some(anchor), cx);
        cx.stop_propagation();
    }

    // ----- rendering pipeline -------------------------------------------------

    pub(crate) fn ensure_rendered(
        &mut self,
        pref: PageRef,
        want: RenderRequest,
        cx: &mut Context<Self>,
    ) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let key = (pref.id, want.job);
        if doc.tiles.contains_key(&key) || doc.inflight.contains_key(&key) {
            return;
        }
        if let Job::Page { rotation, .. } = want.job
            && doc.previews.contains_key(&(pref.id, rotation))
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
                    Job::Page { rotation, .. } => doc.previews.insert((id, rotation), image),
                    Job::Tile { .. } => doc.tiles.insert(key, image),
                };
                if let Some(old) = old {
                    doc.retired.push(old);
                }
                cx.notify();
            }
        }
    }

    /// What File > Print needs from the view.
    pub(crate) fn print_context(
        &self,
        prefs: crate::print_dialog::PrintPrefs,
    ) -> Option<crate::print_dialog::PrintContext> {
        let State::Ready(doc) = &self.state else {
            return None;
        };
        let st = doc.model.state();
        let selected: Vec<usize> = st
            .pages
            .iter()
            .enumerate()
            .filter(|(_, p)| doc.selection.contains(&p.id))
            .map(|(i, _)| i)
            .collect();
        let bookmark = doc.outline_selected.and_then(|id| {
            let title = st
                .outline_flat()
                .into_iter()
                .find(|(_, n)| n.id == id)
                .map(|(_, n)| n.title.clone())?;
            let pages = st
                .bookmark_pages(id)?
                .into_iter()
                .filter_map(|p| st.index_of(p))
                .collect();
            Some((title, pages))
        });
        Some(crate::print_dialog::PrintContext {
            title: self.file_name(),
            snapshot: doc.model.snapshot(),
            handles: doc.handles.clone(),
            current: doc.current_page(),
            selected,
            bookmark,
            prefs,
        })
    }

    /// Run a print job: write the print file in the background, then hand
    /// it to the printer (or, for Save as PDF, ask where to save it).
    pub(crate) fn print(&mut self, req: crate::print_dialog::PrintRequest, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let snapshot = doc.model.snapshot();
        let n = req.pages.len();
        let pages = format!("{n} page{}", if n == 1 { "" } else { "s" });
        if req.save_pdf {
            let dir = self
                .path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_default();
            let suggested = self
                .path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| format!("{s} - print.pdf"));
            let rx = crate::file_dialog::save(cx, &dir, suggested.as_deref());
            cx.spawn(async move |this, cx| {
                let Ok(Ok(Some(path))) = rx.await else { return };
                let result = cx
                    .background_executor()
                    .spawn({
                        let path = path.clone();
                        async move {
                            let bytes = save::print_pdf(&snapshot, &req.pages, req.markup)?;
                            std::fs::write(&path, bytes)?;
                            anyhow::Ok(())
                        }
                    })
                    .await;
                this.update(cx, |v, cx| {
                    if let State::Ready(doc) = &mut v.state {
                        doc.notice = Some(match result {
                            Ok(()) => format!("Saved {pages} to {}", path.display()),
                            Err(e) => format!("Couldn't save: {e:#}"),
                        });
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
            return;
        }
        let printer = req.job.printer.clone();
        doc.notice = Some(format!("Printing {pages} to {printer}…"));
        cx.notify();
        let timing = self.timing;
        cx.spawn(async move |this, cx| {
            let t = Instant::now();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let bytes = save::print_pdf(&snapshot, &req.pages, req.markup)?;
                    let tmp = std::env::temp_dir().join(format!(
                        "sheaf-print-{}-{}.pdf",
                        std::process::id(),
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis())
                            .unwrap_or(0)
                    ));
                    std::fs::write(&tmp, bytes)?;
                    let sent = sheaf_print::submit(&req.job, &tmp);
                    let _ = std::fs::remove_file(&tmp);
                    sent
                })
                .await;
            this.update(cx, |v, cx| {
                if timing {
                    eprintln!(
                        "[timing] print job of {pages} built and sent in {:?}",
                        t.elapsed()
                    );
                }
                if let State::Ready(doc) = &mut v.state {
                    doc.notice = Some(match result {
                        Ok(what) => format!("Sent {pages} to {printer} ({what})"),
                        Err(e) => format!("Printing failed: {e:#}"),
                    });
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// What File > Properties needs from the view.
    pub(crate) fn properties_context(&self) -> Option<crate::properties_dialog::PropertiesContext> {
        let State::Ready(doc) = &self.state else {
            return None;
        };
        let src = doc.model.source(doc.model.primary())?;
        Some(crate::properties_dialog::PropertiesContext {
            path: self.path.clone(),
            source: src.path.clone(),
            password: src.password.clone(),
            bytes: src.bytes,
            page_size: doc
                .shown_ref(doc.current_page())
                .map(|p| doc.model.page_size(&p))
                .map(|s| (s.width, s.height)),
            dirty: doc.model.is_dirty(),
        })
    }

    pub fn current_page(&self) -> usize {
        match &self.state {
            State::Ready(doc) => doc.current_page(),
            _ => 0,
        }
    }
}

/// Each page's marks as the `/NM`s PDFium should hide, by page index.
pub(crate) fn marks_by_page(
    by_index: &std::collections::HashMap<u32, Vec<sheaf_doc::Annot>>,
) -> Vec<(u32, Vec<String>)> {
    by_index
        .iter()
        .map(|(i, list)| (*i, list.iter().flat_map(|a| a.pdf_names()).collect()))
        .collect()
}

/// Read `path`'s third-party comments in the background and list them as
/// `source`'s (after a save, the file's pages have new numbers).
fn reread_foreign(source: SourceId, path: PathBuf, cx: &mut Context<DocView>) {
    cx.spawn(async move |this, cx| {
        let found = cx
            .background_executor()
            .spawn(async move { sheaf_doc::file::read_annots(&path, None) })
            .await;
        let foreign = match found {
            Ok(a) => a.foreign,
            Err(e) => {
                log::warn!("reading third-party comments: {e:#}");
                return;
            }
        };
        this.update(cx, |view, cx| {
            if let State::Ready(doc) = &mut view.state
                && doc.model.primary() == source
            {
                doc.foreign_annots.retain(|(s, _)| *s != source);
                doc.foreign_annots
                    .extend(foreign.into_iter().map(|f| (source, f)));
                doc.foreign_generation += 1;
                cx.notify();
            }
        })
        .ok();
    })
    .detach();
}

fn flat_item(i: &OutlineItem) -> FlatOutlineItem {
    FlatOutlineItem {
        title: i.title.clone(),
        page: i.page,
        depth: i.depth,
        view: i.view.clone(),
        color: None,
        style: Default::default(),
    }
}

/// Wrap a BGRA raster as a GPUI image without copying the pixel data.
/// `RenderImage` is documented as BGRA; `RgbaImage` here is just the container.
pub(crate) fn to_render_image(raster: sheaf_image::raster::Raster) -> RenderImage {
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
    /// As the pane shows it (rotate view included).
    pref: PageRef,
    left: f32,
    top: f32,
    w: f32,
    h: f32,
    dev_w: u32,
    /// Tiles intersecting the (overdrawn) viewport, with priority.
    tiles: Vec<(Job, u8)>,
}

/// One pane's frame: its pages and the renders they want.
struct PaneFrame {
    plans: Vec<PagePlan>,
    /// Pages near the viewport whose images stay cached, with the rotation
    /// this pane draws them at.
    keep: Vec<(PageId, Rotation)>,
    wants: Vec<(PageRef, RenderRequest)>,
}

/// In a non-continuous mode, pick the row a queued scroll lands on before
/// the pane is laid out, and turn scrolling past either end into a page
/// turn.
fn resolve_row(pane: &mut Pane, pages: usize) {
    let mode = pane.mode;
    let rows = mode.row_count(pages);
    if mode.continuous || rows == 0 {
        return;
    }
    let last = rows - 1;
    let at = |pane: &Pane| {
        pane.layout.as_ref().map(|l| {
            let max_y = (l.total_h - l.view_h).max(0.0);
            (-f32::from(pane.scroll.offset().y), max_y)
        })
    };
    match pane.pending_scroll {
        Some(
            ScrollTo::Page(p)
            | ScrollTo::Reveal { page: p, .. }
            | ScrollTo::Point { page: p, .. }
            | ScrollTo::Anchor { page: p, .. },
        ) => pane.row = mode.row_of(p.min(pages - 1)),
        Some(ScrollTo::End) => pane.row = last,
        Some(ScrollTo::NextRow) => {
            pane.pending_scroll = (pane.row < last).then(|| {
                pane.row += 1;
                ScrollTo::Top
            });
        }
        Some(ScrollTo::PrevRowEnd) => {
            pane.pending_scroll = (pane.row > 0).then(|| {
                pane.row -= 1;
                ScrollTo::Bottom
            });
        }
        Some(ScrollTo::By(dy)) => {
            if let Some((y, max_y)) = at(pane) {
                if dy > 0.0 && y >= max_y - 0.5 && pane.row < last {
                    pane.row += 1;
                    pane.pending_scroll = Some(ScrollTo::Top);
                } else if dy < 0.0 && y <= 0.5 && pane.row > 0 {
                    pane.row -= 1;
                    pane.pending_scroll = Some(ScrollTo::Bottom);
                }
            }
        }
        Some(ScrollTo::Top | ScrollTo::Bottom) | None => {}
    }
}

/// Lay out pane `ix`, apply its queued scroll, and plan the renders its
/// visible pages need.
fn plan_pane(
    doc: &mut Doc,
    ix: usize,
    view: (f32, f32),
    bounds_known: bool,
    scale: f32,
) -> PaneFrame {
    let (view_w, view_h) = view;
    let pages = doc.model.page_count();
    let version = doc.version;
    let pane = &mut doc.panes[ix];
    resolve_row(pane, pages);
    let stale = pane.layout.as_ref().is_none_or(|l| {
        l.zoom != pane.zoom
            || l.mode != pane.mode
            || l.rotation != pane.rotation
            || (!pane.mode.continuous && l.row != pane.row)
            || l.version != version
            || (l.view_w - view_w).abs() > 0.5
            || (l.view_h - view_h).abs() > 0.5
    });
    if stale {
        // A resize (or zoom to fit following one) keeps the point at the
        // top centre of the pane where it is, unless a scroll is queued.
        if pane.pending_scroll.is_none()
            && let Some(old) = &pane.layout
            && ((old.view_w - view_w).abs() > 0.5 || (old.view_h - view_h).abs() > 0.5)
        {
            let sx = -f32::from(pane.scroll.offset().x);
            let sy = -f32::from(pane.scroll.offset().y);
            let (vx, vy) = (old.view_w.min(view_w) / 2.0, 0.0);
            if let Some(page) = old.page_at(sx + vx, sy + vy)
                && let Some(r) = old.rect(page)
            {
                pane.pending_scroll = Some(ScrollTo::Anchor {
                    page,
                    fx: (sx + vx - r.left) / r.w,
                    fy: (sy + vy - r.top) / r.h,
                    vx,
                    vy,
                });
            }
        }
        let layout = Layout::build(
            LayoutSpec {
                zoom: pane.zoom,
                mode: pane.mode,
                rotation: pane.rotation,
                row: pane.row,
                view_w,
                view_h,
                version,
            },
            &doc.model,
        );
        pane.row = layout.row;
        pane.layout = Some(layout);
    }
    let layout = pane.layout.as_ref().expect("built above");
    let count = layout.len();

    // Apply any queued scroll now that we know geometry.
    let max_x = (layout.total_w - view_w).max(0.0);
    let max_y = (layout.total_h - view_h).max(0.0);
    let mut scroll_x = (-f32::from(pane.scroll.offset().x)).clamp(0.0, max_x);
    let mut scroll_y = (-f32::from(pane.scroll.offset().y)).clamp(0.0, max_y);
    if let Some(to) = pane.pending_scroll.take()
        && count > 0
    {
        let last = count - 1;
        let rect = |p: usize| layout.rect(p.min(last));
        match to {
            ScrollTo::Page(p) => {
                if let Some(top) = layout.row_top_of(p.min(last)) {
                    scroll_y = top - MARGIN;
                }
            }
            ScrollTo::By(dy) => scroll_y += dy,
            ScrollTo::End | ScrollTo::Bottom => scroll_y = max_y,
            ScrollTo::Top => scroll_y = 0.0,
            ScrollTo::NextRow | ScrollTo::PrevRowEnd => {}
            ScrollTo::Reveal { page, fx, fy } => {
                if let Some(r) = rect(page) {
                    let (x, y) = (r.left + fx * r.w, r.top + fy * r.h);
                    if y < scroll_y + 8.0 || y > scroll_y + view_h - 8.0 {
                        scroll_y = y - view_h / 3.0;
                    }
                    if x < scroll_x || x > scroll_x + view_w {
                        scroll_x = x - view_w / 2.0;
                    }
                }
            }
            ScrollTo::Point { page, fx, fy } => {
                if let Some(r) = rect(page) {
                    scroll_y = r.top + fy * r.h;
                    let x = r.left + fx * r.w;
                    if x < scroll_x || x > scroll_x + view_w {
                        scroll_x = x;
                    }
                }
            }
            ScrollTo::Anchor {
                page,
                fx,
                fy,
                vx,
                vy,
            } => {
                if let Some(r) = rect(page) {
                    scroll_x = r.left + fx * r.w - vx;
                    scroll_y = r.top + fy * r.h - vy;
                }
            }
        }
        scroll_x = scroll_x.clamp(0.0, max_x);
        scroll_y = scroll_y.clamp(0.0, max_y);
        pane.scroll.set_offset(point(px(-scroll_x), px(-scroll_y)));
    }

    let mut frame = PaneFrame {
        plans: Vec::new(),
        keep: Vec::new(),
        wants: Vec::new(),
    };
    if count == 0 {
        pane.current_page = 0;
        return frame;
    }
    let visible = layout.visible(scroll_y, scroll_y + view_h);
    pane.current_page = layout
        .page_at(scroll_x + view_w / 2.0, scroll_y + view_h * 0.5)
        .unwrap_or(0)
        .min(count - 1);

    // Plan tiles for pages near the viewport.
    let overdraw = view_h * TILE_OVERDRAW;
    let fetch = visible.start.saturating_sub(PREFETCH)..(visible.end + PREFETCH).min(count);
    for i in visible.start.saturating_sub(KEEP)..(visible.end + KEEP).min(count) {
        frame
            .keep
            .push((layout.refs[i].id, layout.refs[i].rotation));
    }
    for i in fetch {
        let Some(r) = layout.rect(i) else { continue };
        let pref = layout.refs[i];
        let (left, top, w, h) = (r.left, r.top, r.w, r.h);
        let dev_w = (w * scale).round().max(1.0) as u32;
        let dev_h = (h * scale).round().max(1.0) as u32;
        // Priorities: visible previews, visible tiles, then the
        // same for prefetch pages — so an expensive off-screen
        // preview never delays on-screen tiles.
        let on_screen_page = visible.contains(&i);
        frame.wants.push((
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
        let vis_y1 = ((scroll_y + view_h + overdraw - top) * scale).min(dev_h as f32);
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
                    let on_screen = (y as f32) < strict_y1 && ((y + th) as f32) > strict_y0;
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
            frame.wants.push((
                pref,
                RenderRequest {
                    page: PageIndex(pref.index),
                    job: *job,
                    format: PixelFormat::Bgra,
                    priority: *priority,
                },
            ));
        }
        frame.plans.push(PagePlan {
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
    frame
}

/// Drop cached images no pane needs: anything outside every pane's keep
/// set; tiles at a rotation no pane draws the page at; and, per page,
/// tiles at a stale zoom once every pane showing the page has all its
/// tiles at the current one.
fn evict(doc: &mut Doc, frames: &[PaneFrame]) {
    let wanted: HashSet<TileKey> = frames
        .iter()
        .flat_map(|f| &f.plans)
        .flat_map(|pl| pl.tiles.iter().map(move |(j, _)| (pl.pref.id, *j)))
        .collect();
    let mut kept: HashMap<PageId, smallvec::SmallVec<[Rotation; 2]>> = HashMap::new();
    for &(id, rot) in frames.iter().flat_map(|f| &f.keep) {
        let rots = kept.entry(id).or_default();
        if !rots.contains(&rot) {
            rots.push(rot);
        }
    }
    let mut complete: HashMap<PageId, bool> = HashMap::new();
    for pl in frames.iter().flat_map(|f| &f.plans) {
        let done = pl
            .tiles
            .iter()
            .all(|(j, _)| doc.tiles.contains_key(&(pl.pref.id, *j)));
        *complete.entry(pl.pref.id).or_insert(true) &= done;
    }
    let evict: Vec<TileKey> = doc
        .tiles
        .keys()
        .copied()
        .filter(|k @ (p, j)| {
            let Some(rots) = kept.get(p) else {
                return true;
            };
            !rots.contains(&j.rotation())
                || (!wanted.contains(k) && complete.get(p).copied().unwrap_or(true))
        })
        .collect();
    for k in evict {
        if let Some(im) = doc.tiles.remove(&k) {
            doc.retired.push(im);
        }
    }
    let keep_preview = |doc: &Doc, &(p, r): &(PageId, Rotation)| {
        kept.get(&p).is_some_and(|rots| rots.contains(&r))
            || doc.thumb_keep.contains(&(p, r))
            || doc.review_keep.contains(&(p, r))
    };
    let evict: Vec<(PageId, Rotation)> = doc
        .previews
        .keys()
        .filter(|k| !keep_preview(doc, k))
        .copied()
        .collect();
    for k in evict {
        if let Some(im) = doc.previews.remove(&k) {
            doc.retired.push(im);
        }
    }
    let keep_inflight: Vec<TileKey> = doc
        .inflight
        .keys()
        .filter(|k| {
            wanted.contains(k)
                || matches!(k.1, Job::Page { rotation, .. } if keep_preview(doc, &(k.0, rotation)))
        })
        .copied()
        .collect();
    let keep_inflight: HashSet<TileKey> = keep_inflight.into_iter().collect();
    doc.inflight.retain(|k, _| keep_inflight.contains(k));
}

/// What's drawn over pages besides the document's own marks.
struct Overlays<'a> {
    search: Option<&'a Search>,
    pattern: Option<&'a crate::pattern_panel::PatternSearch>,
    region: Option<&'a crate::region::Region>,
    tool: Tool,
    tool_color: [u8; 3],
    grid: bool,
    guides: &'a [crate::rulers::Guide],
}

/// One page as drawn in a pane: the preview underneath, any tiles we have
/// for it (stale-zoom ones scaled to fit, current on top), then search and
/// pattern hits, markup, and the editing overlays.
fn page_element(doc: &Doc, pl: &PagePlan, ov: &Overlays) -> gpui::Div {
    let mut page_div = div()
        .absolute()
        .top(px(pl.top))
        .left(px(pl.left))
        .w(px(pl.w))
        .h(px(pl.h))
        .bg(theme::PAGE)
        .shadow_md();
    if let Some(prev) = doc.previews.get(&(pl.pref.id, pl.pref.rotation)) {
        page_div = page_div.child(
            div()
                .absolute()
                .inset_0()
                .child(img(prev.clone()).size_full().object_fit(ObjectFit::Fill)),
        );
    }
    let mut page_tiles: Vec<(&TileKey, &Arc<RenderImage>)> = doc
        .tiles
        .iter()
        .filter(|((p, j), _)| *p == pl.pref.id && j.rotation() == pl.pref.rotation)
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
    // Hit rects and marks are in the source page's displayed space.
    let src = doc
        .model
        .source(pl.pref.source)
        .map(|s| s.page_size(pl.pref.index))
        .unwrap_or(PageSize {
            width: 612.0,
            height: 792.0,
        });
    let rot = pl.pref.rotation;
    let k = pl.w / src.rotated(rot).width.max(1.0);
    let hit_rects = |hits: &[Hit], current: Option<usize>, color, current_color| {
        let mut out = Vec::new();
        for (ix, hit) in hits.iter().enumerate() {
            if hit.page != pl.pref.id {
                continue;
            }
            for r in &hit.rects {
                let r = rotate_rect(*r, src, rot);
                out.push(
                    div()
                        .absolute()
                        .left(px(r.x * k))
                        .top(px(r.y * k))
                        .w(px(r.w * k))
                        .h(px(r.h * k))
                        .bg(if current == Some(ix) {
                            current_color
                        } else {
                            color
                        }),
                );
            }
        }
        out
    };
    if ov.grid {
        page_div = page_div.child(crate::rulers::grid(pl.w, pl.h, k));
    }
    page_div = page_div.children(crate::rulers::guide_lines(ov.guides, pl.w, pl.h, k));
    // Search highlights on top of everything; pattern hits in their own color.
    if let Some(search) = ov.search {
        page_div = page_div.children(hit_rects(
            &search.hits,
            search.current,
            theme::SEARCH_HIT,
            theme::SEARCH_HIT_CURRENT,
        ));
    }
    if let Some(ps) = ov.pattern {
        page_div = page_div.children(hit_rects(
            &ps.hits,
            ps.current,
            theme::PATTERN_HIT,
            theme::PATTERN_HIT_CURRENT,
        ));
    }
    // Selected text.
    if let Some(sel) = doc.text_sel.filter(|s| s.page == pl.pref.id)
        && let Some((_, chars)) = doc.page_chars.get(&sel.page)
    {
        let (a, b) = sel.range();
        page_div = page_div.children(chars.rects(a, b).into_iter().map(|r| {
            let r = rotate_rect(
                Rect {
                    x: r.x,
                    y: r.y,
                    w: r.w,
                    h: r.h,
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
                .bg(theme::SELECTION)
        }));
    }
    // A header/footer not yet saved into the file, drawn as it will be.
    page_div = page_div.children(header_footer_elements(doc, pl, k));
    // Our markup, drawn over the page in the displayed orientation, less
    // what the comment filter or View > Hide Comments hides.
    if let Some(list) = doc.model.state().annots_on(pl.pref.id)
        && !doc.comments_hidden
    {
        for a in list.iter().filter(|a| !doc.hidden_marks.contains(&a.id)) {
            page_div = page_div.children(annot_elements(a, src, rot, k));
        }
    }
    // Selection outlines; handles when one mark is selected and nothing is
    // being dragged.
    let selected: Vec<sheaf_doc::Annot> = doc
        .selected_mark_list()
        .into_iter()
        .filter(|(pid, a)| {
            *pid == pl.pref.id
                && a.thread_parent().is_none()
                && !doc.comments_hidden
                && !doc.hidden_marks.contains(&a.id)
        })
        .map(|(_, a)| a)
        .collect();
    for a in &selected {
        let b = a.bounds();
        let r = rotate_rect(
            Rect {
                x: b.x - 2.0,
                y: b.y - 2.0,
                w: b.w + 4.0,
                h: b.h + 4.0,
            },
            src,
            rot,
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
                .border_color(theme::ACCENT),
        );
    }
    if let [a] = selected.as_slice()
        && doc.selected_marks.len() == 1
        && doc.drag.is_none()
    {
        const HANDLE: f32 = 7.0;
        for (_, q) in crate::markup_tools::handle_spots(a) {
            let d = rotate_pt(q, src, rot);
            page_div = page_div.child(
                div()
                    .absolute()
                    .left(px(d.x * k - HANDLE / 2.0))
                    .top(px(d.y * k - HANDLE / 2.0))
                    .w(px(HANDLE))
                    .h(px(HANDLE))
                    .bg(theme::PAGE)
                    .border_1()
                    .border_color(theme::ACCENT),
            );
        }
    }
    if let Some(ed) = &doc.annot_editor
        && ed.page == pl.pref.id
        && let Some(a) = doc
            .model
            .state()
            .annots_on(ed.page)
            .and_then(|l| l.iter().find(|a| a.id == ed.id))
    {
        // Under the mark (a callout's: under its box).
        let b = match &a.kind {
            AnnotKind::Callout { rect, .. } => *rect,
            _ => a.bounds(),
        };
        let r = rotate_rect(
            Rect {
                x: b.x,
                y: b.y + b.h + 2.0,
                w: b.w,
                h: 24.0,
            },
            src,
            rot,
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
    if let Some((source, f)) = doc.selected_foreign.and_then(|i| doc.foreign_annots.get(i))
        && pl.pref.source == *source
        && f.page == pl.pref.index
    {
        page_div = page_div.child(crate::comments_panel::foreign_outline(f, src, rot, k));
    }
    if let Some(overlay) = crate::region::overlay(ov.region, pl.pref.id, src, rot, k) {
        page_div = page_div.child(overlay);
    }
    if let Some(preview) = DocView::drag_preview(doc, ov.tool, ov.tool_color, pl.index, src, rot, k)
    {
        page_div = page_div.child(preview);
    }
    let [r, g, b] = ov.tool_color;
    let tool_rgb = rgb(((r as u32) << 16) | ((g as u32) << 8) | b as u32);
    if let Some(draft) = crate::shapes::draft_preview(doc, tool_rgb, pl.index, src, rot, k) {
        page_div = page_div.child(draft);
    }
    page_div
}

impl DocView {
    /// The document's panes, laid out and drawn, with the status line for
    /// the active one.
    fn render_ready(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (AnyElement, SharedString, Option<SharedString>) {
        let viewport = window.viewport_size();
        let scale = window.scale_factor();
        let page_cursor = self.page_cursor();
        let State::Ready(doc) = &mut self.state else {
            unreachable!("render_ready on a document that isn't ready");
        };
        let full = self.presentation == crate::presentation::Presentation::FullScreen;
        // Full screen shows only the active pane.
        let shown: Vec<usize> = if full {
            vec![doc.active]
        } else {
            (0..doc.panes.len()).collect()
        };
        let panes = shown.len();
        let mut frames = Vec::with_capacity(panes);
        for &ix in &shown {
            // The scroll container's bounds lag a frame behind on first
            // layout; fall back to the window size until we have them.
            let bounds = doc.panes[ix].scroll.bounds();
            let bounds_known = f32::from(bounds.size.width) > 0.0;
            let view = if bounds_known {
                (f32::from(bounds.size.width), f32::from(bounds.size.height))
            } else {
                (
                    f32::from(viewport.width),
                    (f32::from(viewport.height) - STATUS_H) / panes as f32,
                )
            };
            frames.push(plan_pane(doc, ix, view, bounds_known, scale));
        }
        evict(doc, &frames);

        let ov = Overlays {
            search: self.search.as_ref().filter(|s| !s.hits.is_empty()),
            pattern: self.pattern.as_ref().filter(|p| !p.hits.is_empty()),
            region: self.region.as_ref(),
            tool: self.tool,
            tool_color: self.tool_color,
            grid: self.grid && !full,
            guides: if full { &[] } else { &self.guides },
        };
        let mut pane_els = Vec::with_capacity(panes);
        for (&ix, frame) in shown.iter().zip(&frames) {
            let pane = &doc.panes[ix];
            let layout = pane.layout.as_ref().expect("planned");
            let pages: Vec<_> = frame
                .plans
                .iter()
                .map(|pl| page_element(doc, pl, &ov))
                .collect();
            let preview = self.guide_drag.filter(|d| d.pane == ix).and_then(|d| {
                let pos = d.pos?;
                let b = pane.scroll.bounds();
                Some((
                    d.horizontal,
                    if d.horizontal {
                        pos.y - b.origin.y
                    } else {
                        pos.x - b.origin.x
                    },
                ))
            });
            let scroll_el = div()
                .id(("pages", ix))
                .track_scroll(&pane.scroll)
                .overflow_scroll()
                .size_full()
                .on_scroll_wheel(
                    cx.listener(move |v, ev, window, cx| v.on_wheel(ix, ev, window, cx)),
                )
                .cursor(page_cursor)
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |v, ev: &gpui::MouseDownEvent, window, cx| {
                        if v.presentation == crate::presentation::Presentation::FullScreen {
                            // Acrobat: click for the next page, Shift+click back.
                            v.turn_page(if ev.modifiers.shift { -1 } else { 1 }, cx);
                            return;
                        }
                        v.activate_pane(ix, cx);
                        v.on_page_mouse_down(ev, window, cx)
                    }),
                )
                .on_mouse_down(
                    gpui::MouseButton::Right,
                    cx.listener(move |v, ev: &gpui::MouseDownEvent, _, cx| {
                        if v.presentation == crate::presentation::Presentation::FullScreen {
                            v.turn_page(-1, cx);
                            return;
                        }
                        v.activate_pane(ix, cx);
                        // On a mark: its menu.
                        v.open_mark_menu(ev.position, cx);
                    }),
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
                )
                .into_any_element();
            pane_els.push(crate::rulers::with_rulers(
                ix,
                self.rulers && !full,
                preview,
                pane,
                scroll_el,
                cx,
            ));
        }
        let body = if pane_els.len() == 1 {
            pane_els.pop().expect("one pane")
        } else {
            // Split: the panes stacked, with a divider to drag. The active
            // pane gets an accent edge.
            let frac = doc.split;
            let active = doc.active;
            let mut els = pane_els.into_iter().enumerate();
            let (_, top) = els.next().expect("two panes");
            let (_, bottom) = els.next().expect("two panes");
            let edge = |on: bool| {
                div()
                    .h(px(2.0))
                    .flex_shrink_0()
                    .bg(if on { theme::ACCENT } else { theme::RAISED })
            };
            div()
                .id("split")
                .size_full()
                .flex()
                .flex_col()
                .on_mouse_move(cx.listener(|v, ev: &gpui::MouseMoveEvent, _, cx| {
                    v.drag_split(ev.position.y, cx)
                }))
                .on_mouse_up(
                    gpui::MouseButton::Left,
                    cx.listener(|v, _, _, cx| v.end_split_drag(cx)),
                )
                .on_mouse_up_out(
                    gpui::MouseButton::Left,
                    cx.listener(|v, _, _, cx| v.end_split_drag(cx)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .h(relative(frac))
                        .min_h(px(0.0))
                        .child(edge(active == 0))
                        .child(div().flex_1().min_h(px(0.0)).child(top)),
                )
                .child(
                    div()
                        .id("split-divider")
                        .h(px(SPLITTER_H))
                        .flex_shrink_0()
                        .bg(theme::DIVIDER)
                        .cursor(gpui::CursorStyle::ResizeRow)
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(|v, _, _, cx| {
                                if let State::Ready(doc) = &mut v.state {
                                    doc.split_drag = true;
                                }
                                cx.stop_propagation();
                            }),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h(px(0.0))
                        .child(edge(active == 1))
                        .child(div().flex_1().min_h(px(0.0)).child(bottom)),
                )
                .into_any_element()
        };

        let pane = doc.pane();
        let layout = pane.layout.as_ref().expect("planned");
        let count = layout.len();
        let zoom_mode = match pane.zoom {
            Zoom::FitWidth => "fit width",
            Zoom::FitPage => "fit page",
            Zoom::Custom(_) => "custom",
        };
        let size = doc
            .shown_ref(doc.current_page())
            .map(|p| doc.model.page_size(&p))
            .map(|s| format!("{:.0}×{:.0} pt", s.width, s.height))
            .unwrap_or_default();
        let mut view_mode = String::new();
        if pane.mode != PageMode::default() {
            view_mode = format!("  —  {}", pane.mode.label());
        }
        if pane.rotation != Rotation::R0 {
            view_mode.push_str(&format!("  —  view rotated {}°", pane.rotation.degrees()));
        }
        let status = format!(
            "page {}  —  {size}  —  {:.0}% ({zoom_mode}){view_mode}",
            doc.page_position(doc.current_page(), count),
            layout.zoom_percent(),
        );
        let notice = doc.notice.clone().map(Into::into);
        let wants: Vec<_> = frames.into_iter().flat_map(|f| f.wants).collect();
        for (pref, want) in wants {
            self.ensure_rendered(pref, want, cx);
        }
        (body, status.into(), notice)
    }
}

impl Render for DocView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(rec) = self.pending_recovery.take() {
            self.offer_recovery(rec, window, cx);
        }
        if let Some(page) = self.pending_edit_last.take() {
            self.start_annot_editor(page, window, cx);
        }
        // What the comment filter hides on the page, kept current whether
        // or not the panel is open (cached; cheap when nothing changed).
        self.comment_view(cx);
        // Stamp pictures the last frame asked for.
        crate::stamp_preview::make_wanted(cx);
        if std::mem::take(&mut self.pending_focus) {
            window.focus(&self.focus_handle, cx);
        }

        // Release textures for images we've replaced or evicted since last frame.
        if let State::Ready(doc) = &mut self.state {
            for old in doc.retired.drain(..) {
                if let Err(e) = window.drop_image(old) {
                    log::warn!("drop_image: {e:#}");
                }
            }
        }

        use crate::presentation::Presentation;
        let presentation = self.presentation;
        let full = presentation == Presentation::FullScreen;
        let search_bar = (!full).then(|| self.render_search_bar(cx)).flatten();
        let goto_bar = (!full).then(|| self.render_goto()).flatten();
        let props_bar = (!full).then(|| self.render_mark_properties(cx)).flatten();
        let connector_bar = (!full).then(|| self.render_connector_bar(cx)).flatten();
        let measure_bar = (!full)
            .then(|| self.render_measure_bar(window, cx))
            .flatten();
        let group_list = self.render_group_list(cx);
        let mark_menu = self.render_mark_menu(cx);
        let mut body: Option<AnyElement> = None;
        // The status bar: where you are, then the latest notice, which
        // gives way (ellipsis, full text on hover) when space is short.
        let mut status_notice: Option<SharedString> = None;
        let status: SharedString = if matches!(self.state, State::Ready(_)) {
            let (el, status, notice) = self.render_ready(window, cx);
            body = Some(el);
            status_notice = notice;
            status
        } else {
            match &mut self.state {
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
                            .child(locked_card(&self.path, l, cx))
                            .into_any_element(),
                    );
                    "Password required".into()
                }
                State::Ready(_) => unreachable!(),
            }
        };
        let review = self.render_blank_review(cx);
        let ocr_progress = self
            .ocr_progress()
            .or_else(|| self.job_progress())
            .or_else(|| {
                let (_, t) = self.plugin_run.as_ref()?;
                t.snapshot().fraction
            });

        div()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|v, _: &NextPage, _, cx| v.turn_page(1, cx)))
            .on_action(cx.listener(|v, _: &PrevPage, _, cx| v.turn_page(-1, cx)))
            .on_action(cx.listener(|v, _: &FirstPage, _, cx| v.scroll(ScrollTo::Page(0), cx)))
            .on_action(cx.listener(|v, _: &LastPage, _, cx| v.scroll(ScrollTo::End, cx)))
            .on_action(cx.listener(|v, _: &LineDown, _, cx| v.scroll(ScrollTo::By(LINE_STEP), cx)))
            .on_action(cx.listener(|v, _: &LineUp, _, cx| v.scroll(ScrollTo::By(-LINE_STEP), cx)))
            .on_action(cx.listener(|v, _: &ScreenDown, _, cx| {
                let h = v.pane_view_h();
                v.scroll(ScrollTo::By(h * 0.9), cx)
            }))
            .on_action(cx.listener(|v, _: &ScreenUp, _, cx| {
                let h = v.pane_view_h();
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
            .on_action(cx.listener(|v, _: &SinglePage, _, cx| {
                v.set_page_mode(
                    PageMode {
                        spread: Spread::Single,
                        continuous: false,
                    },
                    cx,
                )
            }))
            .on_action(cx.listener(|v, _: &SinglePageContinuous, _, cx| {
                v.set_page_mode(PageMode::default(), cx)
            }))
            .on_action(cx.listener(|v, _: &TwoUp, _, cx| v.set_two_up(false, cx)))
            .on_action(cx.listener(|v, _: &TwoUpContinuous, _, cx| v.set_two_up(true, cx)))
            .on_action(cx.listener(|v, _: &ToggleCoverPage, _, cx| v.toggle_cover_page(cx)))
            .on_action(cx.listener(|v, _: &SplitView, _, cx| v.toggle_split(cx)))
            .on_action(cx.listener(|v, _: &NextPane, _, cx| v.cycle_pane(cx)))
            .on_action(cx.listener(|v, _: &RotateViewCw, _, cx| v.rotate_view(Rotation::R90, cx)))
            .on_action(cx.listener(|v, _: &RotateViewCcw, _, cx| v.rotate_view(Rotation::R270, cx)))
            .on_action(cx.listener(|v, _: &Find, window, cx| v.open_search(window, cx)))
            .on_action(
                cx.listener(|v, _: &FullScreen, window, cx| v.toggle_full_screen(window, cx)),
            )
            .on_action(cx.listener(|v, _: &ReadMode, window, cx| v.toggle_read_mode(window, cx)))
            .on_action(cx.listener(|v, _: &ToggleRulers, _, cx| v.toggle_rulers(cx)))
            .on_action(cx.listener(|v, _: &GoToPage, window, cx| v.open_goto(window, cx)))
            .on_action(cx.listener(|v, _: &ToggleGrid, _, cx| v.toggle_grid(cx)))
            .on_action(cx.listener(|v, _: &ClearGuides, _, cx| v.clear_guides(cx)))
            .when(self.guide_drag.is_some(), |d| {
                d.on_mouse_move(
                    cx.listener(|v, ev: &gpui::MouseMoveEvent, _, cx| {
                        v.move_guide(ev.position, cx)
                    }),
                )
                .on_mouse_up(
                    gpui::MouseButton::Left,
                    cx.listener(|v, _, _, cx| v.finish_guide(cx)),
                )
                .on_mouse_up_out(
                    gpui::MouseButton::Left,
                    cx.listener(|v, _, _, cx| v.finish_guide(cx)),
                )
            })
            .on_action(cx.listener(|v, _: &CloseSearch, window, cx| {
                if v.presentation == crate::presentation::Presentation::FullScreen
                    && v.leave_presentation(window, cx)
                {
                    // Esc leaves full screen before anything else.
                } else if v.clear_region(cx) {
                    // Esc drops a selected region first.
                } else if v.search.is_some() {
                    v.close_search(window, cx)
                } else if v.cancel_ocr(cx) {
                    // Esc during a Recognize Text run stops it.
                } else if v.close_blank_review(cx) {
                    // Esc closes the blank-page review.
                } else if v.cancel_job(cx) {
                    // Likewise an export or a split.
                } else if v.cancel_plugin_run(cx) {
                    // And a plugin command that reports progress.
                } else if v.cancel_tape_placing(cx) || v.cancel_shape(cx) || v.close_group_list(cx)
                {
                    // Esc with a tape half-placed drops it; then the pages
                    // list closes.
                } else if v.clear_text_sel(cx) {
                    // Then selected text.
                } else {
                    if let State::Ready(doc) = &mut v.state {
                        doc.selected_marks.clear();
                        doc.selected_foreign = None;
                        doc.drag = None;
                    }
                    if v.tool != Tool::Select {
                        v.set_tool(Tool::Select, cx);
                    }
                    v.clear_selection(cx);
                    v.leave_presentation(window, cx);
                }
            }))
            .on_action(cx.listener(|v, _: &SearchNext, _, cx| {
                // Enter finishes a shape being drawn, else finds the next.
                if !v.finish_shape(cx) {
                    v.search_step(1, cx)
                }
            }))
            .on_action(cx.listener(|v, _: &crate::UndoVertex, _, cx| {
                v.undo_vertex(cx);
            }))
            .on_action(
                cx.listener(|v, _: &crate::ToolPolygon, _, cx| v.set_tool(Tool::Polygon, cx)),
            )
            .on_action(
                cx.listener(|v, _: &crate::ToolPolyline, _, cx| v.set_tool(Tool::Polyline, cx)),
            )
            .on_action(cx.listener(|v, _: &crate::ToolCloud, _, cx| v.set_tool(Tool::Cloud, cx)))
            .on_action(
                cx.listener(|v, _: &crate::ToolDistance, _, cx| v.set_tool(Tool::Distance, cx)),
            )
            .on_action(
                cx.listener(|v, _: &crate::ToolPerimeter, _, cx| v.set_tool(Tool::Perimeter, cx)),
            )
            .on_action(cx.listener(|v, _: &crate::ToolArea, _, cx| v.set_tool(Tool::Area, cx)))
            .on_action(cx.listener(|v, _: &crate::CalibrateScale, _, cx| v.start_calibrate(cx)))
            .on_action(
                cx.listener(|v, _: &crate::ToolCallout, _, cx| v.set_tool(Tool::Callout, cx)),
            )
            .on_action(
                cx.listener(|v, _: &crate::ToolAttachFile, _, cx| v.set_tool(Tool::AttachFile, cx)),
            )
            .on_action(cx.listener(|v, _: &crate::CycleStampTools, _, cx| v.cycle_stamp_tools(cx)))
            .on_action(
                cx.listener(|v, _: &crate::CycleDrawingTools, _, cx| v.cycle_drawing_tools(cx)),
            )
            .on_action(cx.listener(|v, _: &crate::CycleTextTools, _, cx| v.cycle_text_tools(cx)))
            .on_action(cx.listener(|v, _: &SearchPrev, _, cx| v.search_step(-1, cx)))
            .on_action(
                cx.listener(|v, _: &RotatePageCw, _, cx| v.rotate_current(Rotation::R90, cx)),
            )
            .on_action(
                cx.listener(|v, _: &RotatePageCcw, _, cx| v.rotate_current(Rotation::R270, cx)),
            )
            .on_action(cx.listener(|v, _: &DeletePage, _, cx| {
                if !v.delete_selected_annot(cx) && !v.delete_selected_foreign(cx) {
                    v.delete_current(cx)
                }
            }))
            .on_action(cx.listener(|v, _: &ToggleComments, _, cx| v.toggle_comments_shown(cx)))
            .on_action(cx.listener(|v, _: &CopySelection, _, cx| {
                if !v.copy_text(cx) {
                    v.copy_marks(cx);
                }
            }))
            .on_action(cx.listener(|v, _: &PasteMarks, _, cx| {
                v.paste_marks(false, cx);
            }))
            .on_action(cx.listener(|v, _: &PasteMarksInPlace, _, cx| {
                v.paste_marks(true, cx);
            }))
            .on_action(cx.listener(|v, _: &DuplicatePages, _, cx| v.duplicate_current(cx)))
            .on_action(cx.listener(|v, _: &SelectAllPages, _, cx| v.select_all(cx)))
            .on_action(cx.listener(|v, _: &Save, _, cx| v.save(cx).detach()))
            .on_action(cx.listener(|v, _: &SaveAs, _, cx| v.save_as(cx)))
            .on_action(cx.listener(|v, _: &ExportCleanCopy, _, cx| v.export_clean_copy_prompt(cx)))
            .on_action(cx.listener(|v, _: &ExportFlattened, _, cx| v.export_flattened_prompt(cx)))
            .on_action(cx.listener(|v, _: &crate::SummarizeComments, _, cx| {
                v.export_comments_prompt(
                    ExportKind::Summary(sheaf_ops::comments::Layout::CommentsOnly),
                    cx,
                )
            }))
            .on_action(
                cx.listener(|v, _: &crate::SummarizeCommentsWithPages, _, cx| {
                    v.export_comments_prompt(
                        ExportKind::Summary(sheaf_ops::comments::Layout::Pages),
                        cx,
                    )
                }),
            )
            .on_action(cx.listener(|v, _: &crate::ExportCommentsCsv, _, cx| {
                v.export_comments_prompt(ExportKind::Csv, cx)
            }))
            .on_action(cx.listener(|v, _: &crate::ExportCommentsText, _, cx| {
                v.export_comments_prompt(ExportKind::Text, cx)
            }))
            .on_action(cx.listener(|v, _: &crate::ExportCommentsXfdf, _, cx| {
                v.export_comments_prompt(
                    ExportKind::Data(sheaf_ops::comments::DataFormat::Xfdf, false),
                    cx,
                )
            }))
            .on_action(cx.listener(|v, _: &crate::ExportCommentsFdf, _, cx| {
                v.export_comments_prompt(
                    ExportKind::Data(sheaf_ops::comments::DataFormat::Fdf, false),
                    cx,
                )
            }))
            .on_action(
                cx.listener(|v, _: &crate::ImportComments, _, cx| v.import_comments_prompt(cx)),
            )
            .on_action(cx.listener(|v, _: &ExtractPages, _, cx| v.extract_pages_prompt(cx)))
            .on_action(cx.listener(|v, _: &InsertPdf, _, cx| v.insert_pdf_prompt(cx)))
            .on_action(cx.listener(|v, _: &ToolSelect, _, cx| v.set_tool(Tool::Select, cx)))
            .on_action(cx.listener(|v, _: &ToolNote, _, cx| v.set_tool(Tool::Note, cx)))
            .on_action(
                cx.listener(|v, _: &ToolHighlight, _, cx| v.markup_tool(Tool::Highlight, cx)),
            )
            .on_action(
                cx.listener(|v, _: &ToolUnderline, _, cx| v.markup_tool(Tool::Underline, cx)),
            )
            .on_action(
                cx.listener(|v, _: &ToolStrikeOut, _, cx| v.markup_tool(Tool::StrikeOut, cx)),
            )
            .on_action(cx.listener(|v, _: &ToolSquiggly, _, cx| v.markup_tool(Tool::Squiggly, cx)))
            .on_action(cx.listener(|v, _: &ToolTextEdit, _, cx| v.markup_tool(Tool::TextEdit, cx)))
            .on_action(cx.listener(|v, _: &CycleTextMarkup, _, cx| v.cycle_text_markup(cx)))
            .on_action(cx.listener(|v, _: &ToggleCheckmark, _, cx| v.toggle_checkmark(cx)))
            .on_action(
                cx.listener(|v, _: &crate::NextConnectorLabel, _, cx| v.next_unused_label(cx)),
            )
            .on_action(cx.listener(|v, _: &crate::GroupNext, _, cx| v.group_step(1, cx)))
            .on_action(cx.listener(|v, _: &crate::GroupPrev, _, cx| v.group_step(-1, cx)))
            .on_action(cx.listener(|v, _: &crate::GroupPagesWith, _, cx| v.group_pages(true, cx)))
            .on_action(
                cx.listener(|v, _: &crate::GroupPagesWithout, _, cx| v.group_pages(false, cx)),
            )
            .on_action(cx.listener(|v, _: &ToolFreeText, _, cx| v.set_tool(Tool::FreeText, cx)))
            .on_action(cx.listener(|v, _: &ToolSquare, _, cx| v.set_tool(Tool::Square, cx)))
            .on_action(cx.listener(|v, _: &ToolEllipse, _, cx| v.set_tool(Tool::Ellipse, cx)))
            .on_action(cx.listener(|v, _: &ToolLine, _, cx| v.set_tool(Tool::Line, cx)))
            .on_action(cx.listener(|v, _: &ToolArrow, _, cx| v.set_tool(Tool::Arrow, cx)))
            .on_action(cx.listener(|v, _: &ToolInk, _, cx| v.set_tool(Tool::Ink, cx)))
            .on_action(cx.listener(|v, _: &ToolTick, _, cx| v.set_tool(Tool::Tick, cx)))
            .on_action(cx.listener(|v, _: &ToolStamp, _, cx| {
                v.set_tool(Tool::Stamp, cx);
                // The workspace shows the Stamps panel too.
                cx.propagate();
            }))
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
            .bg(if full {
                theme::FULL_SCREEN
            } else {
                theme::RAISED
            })
            .text_color(theme::TEXT_SOFT)
            .children(search_bar)
            .children(goto_bar)
            .children(props_bar)
            .children(connector_bar)
            .children(measure_bar)
            .child(
                div()
                    .relative()
                    .flex_grow(1.0)
                    .min_h(px(0.0))
                    .children(body)
                    .children(review)
                    .children(group_list)
                    .children(mark_menu),
            )
            .when_some(ocr_progress, |d, frac| {
                d.child(
                    div()
                        .h(px(3.0))
                        .flex_shrink_0()
                        .bg(theme::BACKGROUND)
                        .child(div().h_full().w(relative(frac)).bg(theme::ACCENT)),
                )
            })
            .when(!full, |d| {
                d.child(
                    div()
                        .h(px(STATUS_H))
                        .flex_shrink_0()
                        .px_2()
                        .flex()
                        .items_center()
                        .bg(theme::BACKGROUND)
                        .text_sm()
                        .whitespace_nowrap()
                        .child(div().flex_shrink_0().child(status))
                        .when_some(status_notice, |d, notice| {
                            d.child(
                                div()
                                    .id("status-notice")
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .tooltip(sheaf_ui::tooltip::Tooltip::text(notice.clone()))
                                    .child(format!("  —  {notice}")),
                            )
                        }),
                )
            })
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
                .text_color(theme::TEXT_MUTED)
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
        let layout = ListLayout::variable((0..count).map(item_h)).padded(THUMB_PAD, 0.0);
        // Midpoints decide which gap a drag is over.
        let mids: Arc<Vec<f32>> = Arc::new(
            (0..count)
                .map(|i| layout.top(i) + (layout.height(i) - THUMB_PAD) / 2.0)
                .collect(),
        );

        let bounds = doc.thumb_scroll.bounds();
        let view_h = f32::from(bounds.size.height).max(1.0);
        let max_y = (layout.total_height() - view_h).max(0.0);
        let mut scroll_y = (-f32::from(doc.thumb_scroll.offset().y)).clamp(0.0, max_y);

        // Follow the main view: when the current page changes, make sure its
        // thumbnail is visible.
        let cur = doc.current_page().min(last);
        let cur_id = refs[cur].id;
        if doc.thumb_followed != Some(cur_id) {
            if let Some(y) = layout.reveal_in(cur, THUMB_PAD, scroll_y, view_h) {
                scroll_y = y;
            }
            doc.thumb_scroll.set_offset(point(px(0.0), px(-scroll_y)));
            doc.thumb_followed = Some(cur_id);
        }

        let visible = layout.visible_in(scroll_y, view_h);
        doc.thumb_keep = (visible.start.saturating_sub(2)..(visible.end + 2).min(count))
            .map(|i| (refs[i].id, refs[i].rotation))
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
        let labels: Vec<String> = visible.clone().map(|i| doc.page_label(i)).collect();
        let visible_start = visible.start;

        let mut wants = Vec::new();
        let mut items = Vec::new();
        for i in visible {
            let pref = &refs[i];
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
            let image = doc.previews.get(&(pref.id, pref.rotation)).cloned();
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
                    .top(px(layout.top(i)))
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
                            .bg(theme::PAGE)
                            .border_2()
                            .border_color(if current {
                                theme::ACCENT
                            } else if selected {
                                theme::SELECTED_BORDER
                            } else {
                                theme::BORDER
                            })
                            .when_some(image, |d, im| {
                                d.child(img(im).size_full().object_fit(ObjectFit::Fill))
                            })
                            .when(selected, |d| {
                                d.child(div().absolute().inset_0().bg(theme::SELECTION_WASH))
                            })
                            .when(bookmarked, |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .top(px(0.0))
                                        .right(px(4.0))
                                        .w(px(10.0))
                                        .h(px(14.0))
                                        .bg(theme::BOOKMARK),
                                )
                            }),
                    )
                    .child(
                        div()
                            .h(px(THUMB_LABEL_H))
                            .text_xs()
                            .text_color(if current || selected {
                                theme::TEXT_BRIGHT
                            } else {
                                theme::TEXT_MUTED
                            })
                            .child(labels[i - visible_start].clone()),
                    ),
            );
        }

        // Drop indicator: a line in the gap the drag is over.
        let indicator = doc.drop_target.map(|at| {
            let y = if at >= count {
                layout.top(last) + layout.height(last) - THUMB_PAD / 2.0
            } else {
                layout.top(at) - THUMB_PAD / 2.0
            };
            div()
                .absolute()
                .left(px(THUMB_PAD / 2.0))
                .top(px(y - 1.0))
                .w(px(thumb_w + THUMB_PAD))
                .h(px(2.0))
                .bg(theme::ACCENT)
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
                    .h(px(layout.total_height()))
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
            .bg(theme::ACCENT)
            .text_color(theme::TEXT_BRIGHT)
            .text_sm()
            .child(if self.count == 1 {
                "1 page".to_string()
            } else {
                format!("{} pages", self.count)
            })
    }
}

/// The unsaved header/footer on one page, in page-local logical px. Only
/// without a view rotation: GPUI can't turn text, and the header/footer
/// reads upright on the page as the document has it.
fn header_footer_elements(doc: &Doc, pl: &PagePlan, k: f32) -> Vec<AnyElement> {
    let st = doc.model.state();
    let Some(hf) = &st.header_footer else {
        return Vec::new();
    };
    let own = st.pages.get(pl.index);
    if !doc.model.header_footer_dirty()
        || !hf.covers(pl.pref.id)
        || own.is_none_or(|p| p.rotation != pl.pref.rotation)
    {
        return Vec::new();
    }
    let size = doc.model.page_size(&pl.pref);
    let bates = if hf.bates.is_some() {
        let ids: Vec<PageId> = st.pages.iter().map(|p| p.id).collect();
        hf.bates_numbers(&ids).get(pl.index).copied().flatten()
    } else {
        None
    };
    let facts = sheaf_doc::stamps::PageFacts {
        index: pl.index,
        count: st.pages.len(),
        bates,
    };
    let [r, g, b] = hf.color;
    let color = rgb(((r as u32) << 16) | ((g as u32) << 8) | b as u32);
    hf.layout(size, &facts)
        .into_iter()
        .map(|p| {
            div()
                .absolute()
                .left(px(p.x * k))
                .top(px((p.baseline - p.size * 0.9) * k))
                .h(px(p.size * 1.1 * k))
                .whitespace_nowrap()
                .font_family("Liberation Sans")
                .text_size(px(p.size * k))
                .line_height(px(p.size * 1.1 * k))
                .text_color(color)
                .child(p.text)
                .into_any_element()
        })
        .collect()
}

/// GPUI elements for one annotation, positioned in page-local logical px.
/// `src` is the source page size, `rot` the page's extra rotation, `k` logical
/// px per point.
/// A polygon's or polyline's outline (a cloud's scallops when it's
/// cloudy), filled when it's closed and has a fill.
fn outline_elements(
    a: &Annot,
    closed: bool,
    src: PageSize,
    rot: Rotation,
    k: f32,
    stroke: gpui::Rgba,
    alpha: u32,
) -> Vec<AnyElement> {
    let px_of = |p: Pt| {
        let q = rotate_pt(p, src, rot);
        (q.x * k, q.y * k)
    };
    let mut path: Vec<(f32, f32)> = if closed && a.cloudy {
        let outline = sheaf_doc::annot::outline_of(&a.kind).unwrap_or_default();
        let curves =
            sheaf_doc::annot::cloud_curves(&outline, sheaf_doc::annot::cloud_step(a.stroke));
        let mut pts = Vec::new();
        for [p0, c1, c2, p1] in curves {
            for i in 0..8 {
                let t = i as f32 / 8.0;
                let u = 1.0 - t;
                pts.push(px_of(Pt {
                    x: u * u * u * p0.x
                        + 3.0 * u * u * t * c1.x
                        + 3.0 * u * t * t * c2.x
                        + t * t * t * p1.x,
                    y: u * u * u * p0.y
                        + 3.0 * u * u * t * c1.y
                        + 3.0 * u * t * t * c2.y
                        + t * t * t * p1.y,
                }));
            }
        }
        pts
    } else {
        match &a.kind {
            AnnotKind::Polygon { points } | AnnotKind::Polyline { points } => {
                points.iter().map(|p| px_of(*p)).collect()
            }
            other => sheaf_doc::annot::outline_of(other)
                .unwrap_or_default()
                .into_iter()
                .map(px_of)
                .collect(),
        }
    };
    if closed && let Some(&first) = path.first() {
        path.push(first);
    }
    let mut out = Vec::new();
    if closed && let Some(f) = a.fill {
        let u = ((f[0] as u32) << 16) | ((f[1] as u32) << 8) | f[2] as u32;
        out.push(crate::markup_tools::fill_canvas(
            vec![path.clone()],
            rgba((u << 8) | alpha),
            src.rotated(rot),
            k,
        ));
    }
    out.push(stroke_canvas_dashed(
        vec![path],
        stroke,
        a.stroke * k,
        a.dashed.then_some(k),
        src.rotated(rot),
        k,
    ));
    out
}

/// A text edit's caret, drawn as the file draws it.
fn caret_element(at: Box2, color: gpui::Rgba, src: PageSize, rot: Rotation, k: f32) -> AnyElement {
    let tri = sheaf_doc::annot::caret_points(at)
        .into_iter()
        .map(|p| {
            let q = rotate_pt(p, src, rot);
            (q.x * k, q.y * k)
        })
        .collect();
    crate::markup_tools::fill_canvas(vec![tri], color, src.rotated(rot), k)
}

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
                    .border_color(theme::INK)
                    .rounded_sm()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme::INK)
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
        AnnotKind::Square { .. } | AnnotKind::Ellipse { .. } | AnnotKind::Polygon { .. }
            if a.cloudy =>
        {
            out.extend(outline_elements(a, true, src, rot, k, fill, alpha));
        }
        AnnotKind::Polygon { .. } | AnnotKind::Polyline { .. } => {
            let closed = matches!(a.kind, AnnotKind::Polygon { .. });
            out.extend(outline_elements(a, closed, src, rot, k, fill, alpha));
        }
        AnnotKind::Measure {
            what,
            points,
            scale,
        } => {
            use sheaf_doc::measure::Measured;
            let px_of = |p: Pt| {
                let q = rotate_pt(p, src, rot);
                (q.x * k, q.y * k)
            };
            let mut line: Vec<(f32, f32)> = points.iter().map(|p| px_of(*p)).collect();
            if *what == Measured::Area && points.len() > 2 {
                line.push(line[0]);
            }
            let mut paths = vec![line];
            if *what == Measured::Distance
                && let [first, .., last] = points.as_slice()
            {
                let (p0, p1) = (px_of(*first), px_of(*last));
                let head = (6.0 + a.stroke * 2.0) * k;
                paths.push(arrow_head(p0, p1, head));
                paths.push(arrow_head(p1, p0, head));
            }
            out.push(stroke_canvas_dashed(
                paths,
                fill,
                a.stroke * k,
                a.dashed.then_some(k),
                src.rotated(rot),
                k,
            ));
            if let Some(b) = sheaf_doc::annot::measure_label_box(*what, points, scale) {
                let size = sheaf_doc::annot::MEASURE_TEXT;
                out.push(
                    place(b)
                        .px(px(2.0 * k))
                        .whitespace_nowrap()
                        .text_color(color)
                        .text_size(px(size * k))
                        .line_height(px((size + 3.0) * k))
                        .child(sheaf_doc::measure::label(*what, points, scale))
                        .into_any_element(),
                );
            }
        }
        AnnotKind::FileAttachment { pos, icon, .. } => {
            let at = Box2 {
                x: pos.x,
                y: pos.y,
                w: sheaf_doc::annot::NOTE_SIZE,
                h: sheaf_doc::annot::NOTE_SIZE,
            };
            let paths = sheaf_doc::annot::attachment_icon(icon, at)
                .into_iter()
                .map(|path| {
                    path.into_iter()
                        .map(|p| {
                            let q = rotate_pt(p, src, rot);
                            (q.x * k, q.y * k)
                        })
                        .collect()
                })
                .collect();
            out.push(stroke_canvas(paths, fill, 1.5 * k, src.rotated(rot), k));
        }
        AnnotKind::Callout {
            rect,
            knee,
            point,
            size,
        } => {
            let px_of = |p: Pt| {
                let q = rotate_pt(p, src, rot);
                (q.x * k, q.y * k)
            };
            let end = sheaf_doc::annot::callout_end(*rect, *knee);
            let (e, kn, pt) = (px_of(end), px_of(*knee), px_of(*point));
            out.push(stroke_canvas(
                vec![
                    vec![e, kn, pt],
                    arrow_head(kn, pt, (6.0 + a.stroke * 2.0) * k),
                ],
                fill,
                a.stroke * k,
                src.rotated(rot),
                k,
            ));
            out.push(
                place(*rect)
                    .bg(theme::PAGE)
                    .border(px((a.stroke * k).max(1.0)))
                    .border_color(fill)
                    .p(px(2.0 * k))
                    .overflow_hidden()
                    .text_color(color)
                    .text_size(px(*size * k))
                    .line_height(px(*size * 1.2 * k))
                    .child(a.contents.clone())
                    .into_any_element(),
            );
        }
        AnnotKind::Square { rect } | AnnotKind::Ellipse { rect } => {
            let inside = a.fill.map(|f| {
                let u = ((f[0] as u32) << 16) | ((f[1] as u32) << 8) | f[2] as u32;
                rgba((u << 8) | alpha)
            });
            out.push(
                place(*rect)
                    .border(px((a.stroke * k).max(1.0)))
                    .border_color(fill)
                    .when(a.dashed, |d| d.border_dashed())
                    .when_some(inside, |d, c| d.bg(c))
                    .when(matches!(a.kind, AnnotKind::Ellipse { .. }), |d| {
                        d.rounded_full()
                    })
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
        AnnotKind::Tape {
            rect, size, look, ..
        } => {
            use sheaf_doc::annot::{TAPE_LINE, TAPE_PAD, tape_columns};
            use sheaf_doc::tape::{Row, fmt_amount};
            let [pr, pg, pb] = look.paper;
            let paper = rgb(((pr as u32) << 16) | ((pg as u32) << 8) | pb as u32);
            if look.rotation % 360 != 0 {
                // GPUI can't turn text: a picture of the tape as the file
                // draws it.
                let el = match crate::stamp_preview::tape_picture(a) {
                    Some(path) => {
                        place(*rect).child(img(path).size_full().object_fit(ObjectFit::Fill))
                    }
                    None => place(*rect)
                        .bg(paper)
                        .border_1()
                        .border_color(theme::TAPE_BORDER),
                };
                out.push(el.into_any_element());
            } else {
                let tape = a.tape().unwrap_or_default();
                let (number_w, amount_w, symbol_w, _) = tape_columns(&tape, *size, look);
                let fs = *size * k;
                let line_h = *size * TAPE_LINE * k;
                let rows: Vec<AnyElement> =
                    look.shown(&tape)
                        .into_iter()
                        .map(|(number, l)| {
                            let total = l.row.is_total();
                            let body: AnyElement = match &l.row {
                                Row::Title(t) => div()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .whitespace_nowrap()
                                    .child(t.clone())
                                    .into_any_element(),
                                row => div()
                                    .flex()
                                    .child(div().w(px(amount_w * k)).flex().justify_end().child(
                                        fmt_amount(row.value().unwrap_or(0.0), look.decimals),
                                    ))
                                    .child(
                                        div()
                                            .w(px(symbol_w * k))
                                            .pl(px(*size * 0.3 * k))
                                            .child(row.symbol()),
                                    )
                                    .into_any_element(),
                            };
                            div()
                                .flex()
                                .items_center()
                                .h(px(line_h))
                                .whitespace_nowrap()
                                .when(total, |d| {
                                    d.border_t_1()
                                        .border_color(theme::TAPE_RULE)
                                        .font_weight(gpui::FontWeight::BOLD)
                                })
                                .when(look.numbers, |d| {
                                    d.child(
                                        div()
                                            .w(px(number_w * k))
                                            .flex_shrink_0()
                                            .text_color(theme::TAPE_RULE)
                                            .font_weight(gpui::FontWeight::NORMAL)
                                            .child(number),
                                    )
                                })
                                .child(body)
                                .when(!l.note.is_empty(), |d| {
                                    d.child(
                                        div()
                                            .pl(px(*size * 0.4 * k))
                                            .font_weight(gpui::FontWeight::NORMAL)
                                            .child(l.note.clone()),
                                    )
                                })
                                .into_any_element()
                        })
                        .collect();
                out.push(
                    place(*rect)
                        .bg(paper)
                        .border_1()
                        .border_color(theme::TAPE_BORDER)
                        .p(px(TAPE_PAD * k))
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .text_color(color)
                        .text_size(px(fs))
                        .line_height(px(line_h))
                        .children(rows)
                        .into_any_element(),
                );
            }
        }
        AnnotKind::Connector { rect, label, .. } => {
            out.push(
                place(*rect)
                    .rounded_full()
                    .bg(fill)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme::TEXT_BRIGHT)
                    .text_size(px(rect.h * 0.55 * k))
                    .font_weight(gpui::FontWeight::BOLD)
                    .child(label.clone())
                    .into_any_element(),
            );
        }
        AnnotKind::Underline { rects }
        | AnnotKind::StrikeOut { rects }
        | AnnotKind::Squiggly { rects }
        | AnnotKind::Replace { rects, .. } => {
            // A replacement crosses its text out, then shows its caret.
            let lines = match &a.kind {
                AnnotKind::Replace { .. } => AnnotKind::StrikeOut {
                    rects: rects.clone(),
                },
                other => other.clone(),
            };
            let paths = sheaf_doc::annot::text_line_paths(&lines)
                .into_iter()
                .map(|path| {
                    path.into_iter()
                        .map(|p| {
                            let q = rotate_pt(p, src, rot);
                            (q.x * k, q.y * k)
                        })
                        .collect()
                })
                .collect();
            let width = sheaf_doc::annot::text_line_width(rects);
            out.push(stroke_canvas(paths, fill, width * k, src.rotated(rot), k));
            if let AnnotKind::Replace { at, .. } = &a.kind {
                out.push(caret_element(*at, fill, src, rot, k));
            }
        }
        AnnotKind::Insert { at } => out.push(caret_element(*at, fill, src, rot, k)),
        // In their parent's thread, not on the page.
        AnnotKind::Reply { .. } | AnnotKind::Status { .. } => {}
        AnnotKind::PageStamp {
            rect,
            stamp,
            name,
            text,
        } => {
            let el = match crate::stamp_preview::picture(stamp) {
                Some(path) => place(*rect)
                    .opacity(a.opacity.clamp(0.0, 1.0))
                    .child(img(path).size_full().object_fit(ObjectFit::Fill)),
                // Until its picture is made (a moment), its name in a box.
                None => place(*rect)
                    .border_1()
                    .border_dashed()
                    .border_color(color)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(color)
                    .text_size(px((10.0 * k).max(6.0)))
                    .child(name.clone()),
            };
            let el = match text.as_ref().filter(|t| !t.is_empty()) {
                Some(t) => {
                    let size = (rect.h * 0.16).clamp(5.0, 24.0);
                    el.child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .bottom(px(rect.h * 0.08 * k))
                            .flex()
                            .justify_center()
                            .text_color(color)
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_size(px(size * k))
                            .line_height(px(size * k))
                            .child(t.clone()),
                    )
                }
                None => el,
            };
            out.push(el.into_any_element());
        }
        AnnotKind::PageBorder { rect, .. } => out.push(
            place(*rect)
                .border(px((a.stroke * k).max(1.0)))
                .border_color(fill)
                .into_any_element(),
        ),
        AnnotKind::TextStamp { rect, text, .. } => {
            let lw = sheaf_doc::annot::text_stamp_line(*rect);
            let mut el = place(*rect)
                .border(px(lw * k))
                .border_color(fill)
                .rounded(px(rect.h * 0.18 * k))
                .text_color(fill)
                .font_weight(gpui::FontWeight::BOLD);
            for (line, size, baseline) in sheaf_doc::annot::text_stamp_lines(*rect, text) {
                el = el.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(px((baseline - rect.y - size * 0.9) * k))
                        .flex()
                        .justify_center()
                        .text_size(px(size * k))
                        .line_height(px(size * k))
                        .child(line),
                );
            }
            out.push(el.into_any_element());
        }
        AnnotKind::Line {
            a: pa,
            b: pb,
            arrow,
            arrow_start,
        } => {
            let (qa, qb) = (rotate_pt(*pa, src, rot), rotate_pt(*pb, src, rot));
            let (a2, b2) = ((qa.x * k, qa.y * k), (qb.x * k, qb.y * k));
            let head = (6.0 + a.stroke * 2.0) * k;
            out.push(stroke_canvas_dashed(
                vec![vec![a2, b2]],
                fill,
                a.stroke * k,
                a.dashed.then_some(k),
                src.rotated(rot),
                k,
            ));
            let mut heads = Vec::new();
            if *arrow {
                heads.push(arrow_head(a2, b2, head));
            }
            if *arrow_start {
                heads.push(arrow_head(b2, a2, head));
            }
            if !heads.is_empty() {
                out.push(stroke_canvas(
                    heads,
                    fill,
                    a.stroke * k,
                    src.rotated(rot),
                    k,
                ));
            }
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
            out.push(stroke_canvas_dashed(
                polylines,
                fill,
                a.stroke * k,
                a.dashed.then_some(k),
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
                .bg(theme::CARD)
                .border_1()
                .border_color(theme::BORDER_CARD)
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
                            theme::ERROR
                        } else {
                            theme::TEXT_MUTED
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
                        Button::new("unlock", "Open")
                            .primary()
                            .on_click(cx.listener(move |v, _, _, cx| {
                                let pw = input.read(cx).text().to_string();
                                if !pw.is_empty() {
                                    v.password = Some(pw);
                                    v.open(cx);
                                }
                            })),
                    ),
                ),
        )
        .into_any_element()
}

/// Expand what's needed to see bookmarks a batch added: their parents,
/// and new headings with children of their own. Everything else keeps its
/// expanded or collapsed state.
fn reveal_new_bookmarks(
    tree: &im::Vector<sheaf_doc::OutlineNode>,
    before: &HashSet<OutlineId>,
    expanded: &mut HashSet<OutlineId>,
) {
    fn walk(
        nodes: &im::Vector<sheaf_doc::OutlineNode>,
        path: &mut Vec<OutlineId>,
        before: &HashSet<OutlineId>,
        expanded: &mut HashSet<OutlineId>,
    ) {
        for n in nodes {
            if !before.contains(&n.id) {
                expanded.extend(path.iter().copied());
                if !n.children.is_empty() {
                    expanded.insert(n.id);
                }
            }
            path.push(n.id);
            walk(&n.children, path, before, expanded);
            path.pop();
        }
    }
    walk(tree, &mut Vec::new(), before, expanded);
}

#[cfg(test)]
mod tests {
    use super::*;
    use sheaf_doc::OutlineNode;

    fn node(id: u64, children: Vec<OutlineNode>) -> OutlineNode {
        OutlineNode {
            id: OutlineId(id),
            title: id.to_string(),
            target: None,
            view: None,
            color: None,
            style: Default::default(),
            children: children.into(),
        }
    }

    #[test]
    fn new_bookmarks_are_revealed() {
        // 1 (old, collapsed) > 2 (old) > 3 (new); 4 (new heading) > 5 (new);
        // 6 (old, collapsed, untouched) > 7 (old).
        let tree: im::Vector<OutlineNode> = vec![
            node(1, vec![node(2, vec![node(3, vec![])])]),
            node(4, vec![node(5, vec![])]),
            node(6, vec![node(7, vec![])]),
        ]
        .into();
        let before: HashSet<OutlineId> = [1, 2, 6, 7].map(OutlineId).into();
        let mut expanded = HashSet::new();
        reveal_new_bookmarks(&tree, &before, &mut expanded);
        let mut got: Vec<u64> = expanded.iter().map(|i| i.0).collect();
        got.sort();
        assert_eq!(got, [1, 2, 4]);
    }
}
