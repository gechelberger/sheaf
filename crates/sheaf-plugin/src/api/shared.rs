// What API 0.2 and 0.3 have in common: the conversions between their
// types and sheaf's, and the host side of their interfaces. Included in
// each version's module (`mod.rs`), where `api` is that version's
// bindings; a change here changes both.
//
// Their page coordinates are a page's frame (`page_frame`): sheaf's
// PageSpace converts to it on the way out and from it on the way in.

use std::sync::Arc;
use std::time::Instant;

use sheaf_core::{PageSize, Rotation};
use sheaf_doc::geom::{Affine, Geom};
use sheaf_doc::{
    Annot, AnnotId, AnnotKind, Box2, DestView, Edit, NewOutlineItem, OutlineId, OutlineNode,
    PageId, PageRef, Pt, Snapshot, SourceId, TextHit,
};
use in_progress::{Level, Progress, Tracker, Unit};
use wasmtime::component::Resource;

use crate::progress::Task;
use crate::render::{MAX_DPI, MAX_LIVE_IMAGES, PIXEL_BUDGET, PageRenderer, RenderedImage};
use crate::runtime::{DocEvent, EditBatch, HostState, TextMatch};
use crate::ui::{self, BuiltinPanel, ShowPanel};
use api::types as w;
use api::ui as wu;

// ----- conversions -----

pub(crate) fn rotation_to_wit(r: Rotation) -> w::Rotation {
    match r {
        Rotation::R0 => w::Rotation::R0,
        Rotation::R90 => w::Rotation::R90,
        Rotation::R180 => w::Rotation::R180,
        Rotation::R270 => w::Rotation::R270,
    }
}

pub(crate) fn rotation_from_wit(r: w::Rotation) -> Rotation {
    match r {
        w::Rotation::R0 => Rotation::R0,
        w::Rotation::R90 => Rotation::R90,
        w::Rotation::R180 => Rotation::R180,
        w::Rotation::R270 => Rotation::R270,
    }
}

fn rgb_to_wit(c: [u8; 3]) -> w::Rgb {
    w::Rgb {
        r: c[0],
        g: c[1],
        b: c[2],
    }
}

fn rgb_from_wit(c: w::Rgb) -> [u8; 3] {
    [c.r, c.g, c.b]
}

fn pt_to_wit(p: Pt) -> w::Pt {
    w::Pt { x: p.x, y: p.y }
}

fn pt_from_wit(p: w::Pt) -> Pt {
    Pt { x: p.x, y: p.y }
}

pub(crate) fn box_to_wit(b: Box2) -> w::Box2 {
    w::Box2 {
        x: b.x,
        y: b.y,
        w: b.w,
        h: b.h,
    }
}

pub(crate) fn box_from_wit(b: w::Box2) -> Box2 {
    Box2 {
        x: b.x,
        y: b.y,
        w: b.w,
        h: b.h,
    }
}

/// A page's size as displayed.
pub(crate) fn page_size(snap: &Snapshot, p: &PageRef) -> PageSize {
    snap.geom(p).shown()
}

/// A page's frame, what the API's page coordinates are in: its source
/// page as the file displays it, after the file's own `/Rotate` and before
/// any sheaf adds (`wit/sheaf.wit`). None for a page that isn't in the
/// document, whose geometry passes as it is.
fn page_frame(snap: &Snapshot, page: PageId) -> Option<Geom> {
    let p = snap.state.pages.iter().find(|p| p.id == page)?;
    snap.source(p.source).map(|s| s.src.geom(p.index))
}

/// PageSpace into `page`'s frame, for geometry going out.
fn to_frame(snap: &Snapshot, page: PageId) -> Affine {
    page_frame(snap, page).map_or(Affine::IDENTITY, |g| g.to_view())
}

/// An OCR layer's hits as matches in its page's frame (`to_view`).
fn layer_matches(hits: Vec<TextHit>, to_view: Affine) -> Vec<TextMatch> {
    hits.into_iter()
        .map(|h| TextMatch {
            text: h.text,
            rects: h.rects.into_iter().map(|r| to_view.apply_box(r)).collect(),
            groups: h.groups,
        })
        .collect()
}

/// An OCR layer, its words in their page's frame (`to_view`).
pub(crate) fn layer_to_wit(l: &sheaf_doc::TextLayer, to_view: Affine) -> api::ocr::TextLayer {
    use api::ocr as wo;
    wo::TextLayer {
        words: l
            .words
            .iter()
            .map(|w| wo::OcrWord {
                text: w.text.clone(),
                rect: box_to_wit(to_view.apply_box(w.rect)),
                confidence: w.confidence,
            })
            .collect(),
        lines: l
            .lines
            .iter()
            .map(|r| (r.start as u32, r.end as u32))
            .collect(),
        engine: l.engine.clone(),
    }
}

pub(crate) fn page_info(snap: &Snapshot, p: &PageRef) -> w::PageInfo {
    let size = page_size(snap, p);
    w::PageInfo {
        id: p.id.0,
        rotation: rotation_to_wit(snap.render_delta(p)),
        width: size.width,
        height: size.height,
    }
}

/// A bookmark's position, `/XYZ` mapped by `to_view` one axis at a time,
/// so a missing coordinate stays missing. Other kinds are user space, as
/// the file has them.
pub(crate) fn dest_to_wit(v: &DestView, to_view: Affine) -> w::DestView {
    match v {
        DestView::Xyz { left, top, zoom } => {
            let (left, top) = to_view.apply_axes(*left, *top);
            w::DestView::Xyz(w::DestXyz {
                left,
                top,
                zoom: *zoom,
            })
        }
        DestView::Other { kind, params } => w::DestView::Other(w::DestOther {
            kind: kind.clone(),
            params: params.clone(),
        }),
    }
}

/// A plugin's bookmark position, `/XYZ` mapped by `from_view` (see
/// [`dest_to_wit`]).
pub(crate) fn dest_from_wit(v: w::DestView, from_view: Affine) -> DestView {
    match v {
        w::DestView::Xyz(x) => {
            let (left, top) = from_view.apply_axes(x.left, x.top);
            DestView::Xyz {
                left,
                top,
                zoom: x.zoom.filter(|z| *z > 0.0),
            }
        }
        w::DestView::Other(o) => DestView::Other {
            kind: o.kind,
            params: o.params,
        },
    }
}

pub(crate) fn outline_item(snap: &Snapshot, depth: u16, n: &OutlineNode) -> w::OutlineItem {
    w::OutlineItem {
        id: n.id.0,
        title: n.title.clone(),
        target: n.target.map(|p| p.0),
        view: n.view.as_ref().map(|v| {
            let to_view = n.target.map_or(Affine::IDENTITY, |t| to_frame(snap, t));
            dest_to_wit(v, to_view)
        }),
        color: n.color.map(rgb_to_wit),
        depth,
    }
}

/// A plugin's edit, its geometry from its pages' frames into PageSpace.
/// `snap` is the document the call started with; with none, nothing is
/// mapped.
pub(crate) fn edit_from_wit(snap: Option<&Snapshot>, e: w::Edit) -> Edit {
    let pages = |ids: Vec<u64>| ids.into_iter().map(PageId).collect::<Vec<_>>();
    let frame = |page: PageId| snap.and_then(|s| page_frame(s, page));
    let from_view = |page: Option<PageId>| {
        page.and_then(frame)
            .map_or(Affine::IDENTITY, |g| g.from_view())
    };
    // A mark reads upright in the frame it was given in.
    let page_annot = |pa: w::PageAnnot| {
        let page = PageId(pa.page);
        let annot = annot_from_wit(pa.annot);
        let annot = match frame(page) {
            Some(g) => annot.to_page(&g),
            None => annot,
        };
        (page, annot)
    };
    match e {
        w::Edit::MovePages(m) => Edit::MovePages {
            ids: pages(m.ids),
            before: m.before.map(PageId),
        },
        w::Edit::RotatePages(r) => Edit::RotatePages {
            ids: pages(r.ids),
            by: rotation_from_wit(r.by),
        },
        w::Edit::DeletePages(ids) => Edit::DeletePages { ids: pages(ids) },
        w::Edit::DuplicatePages(ids) => Edit::DuplicatePages { ids: pages(ids) },
        w::Edit::OutlineInsert(o) => {
            let target = o.target.map(PageId);
            Edit::OutlineInsert {
                parent: o.parent.map(OutlineId),
                after: o.after.map(OutlineId),
                title: o.title,
                target,
                view: o.view.map(|v| dest_from_wit(v, from_view(target))),
            }
        }
        w::Edit::OutlineInsertTree(t) => Edit::OutlineInsertTree {
            parent: t.parent.map(OutlineId),
            after: t.after.map(OutlineId),
            items: t
                .items
                .into_iter()
                .map(|i| {
                    let target = i.target.map(PageId);
                    NewOutlineItem {
                        title: i.title,
                        target,
                        view: i.view.map(|v| dest_from_wit(v, from_view(target))),
                        depth: i.depth,
                    }
                })
                .collect(),
        },
        w::Edit::OutlineMove(o) => Edit::OutlineMove {
            id: OutlineId(o.id),
            parent: o.parent.map(OutlineId),
            after: o.after.map(OutlineId),
            with_pages: o.with_pages,
        },
        w::Edit::OutlineEdit(o) => {
            let id = OutlineId(o.id);
            let target = o.target.map(|t| t.map(PageId));
            // A position is on the bookmark's new page, or else its own.
            let page = || match target {
                Some(t) => t,
                None => snap.and_then(|s| {
                    let flat = s.state.outline_flat();
                    flat.into_iter().find(|(_, n)| n.id == id)?.1.target
                }),
            };
            Edit::OutlineEdit {
                id,
                title: o.title,
                target,
                color: o.color.map(|c| c.map(rgb_from_wit)),
                view: o
                    .view
                    .map(|v| v.map(|v| dest_from_wit(v, from_view(page())))),
                style: None,
            }
        }
        w::Edit::OutlineDelete(id) => Edit::OutlineDelete { id: OutlineId(id) },
        w::Edit::AnnotAdd(pa) => {
            let (page, annot) = page_annot(pa);
            Edit::AnnotAdd { page, annot }
        }
        w::Edit::AnnotUpdate(pa) => {
            let (page, annot) = page_annot(pa);
            Edit::AnnotUpdate { page, annot }
        }
        w::Edit::AnnotDelete(r) => Edit::AnnotDelete {
            page: PageId(r.page),
            id: AnnotId(r.id),
        },
        w::Edit::ConnectorAdd(c) => Edit::ConnectorAdd {
            a: page_annot(c.a),
            b: page_annot(c.b),
        },
    }
}

/// An event as a plugin sees it, a region in its page's frame. `snap` is
/// the document the call started with; with none, nothing is mapped.
pub(crate) fn event_to_wit(snap: Option<&Snapshot>, e: &DocEvent) -> w::DocEvent {
    match e {
        DocEvent::Opened => w::DocEvent::Opened,
        DocEvent::Edited(l) => w::DocEvent::Edited(l.clone()),
        DocEvent::Saved(p) => w::DocEvent::Saved(p.clone()),
        DocEvent::SelectionChanged => w::DocEvent::SelectionChanged,
        DocEvent::Closed => w::DocEvent::Closed,
        DocEvent::Region { page, rect } => {
            let to_view = snap.map_or(Affine::IDENTITY, |s| to_frame(s, *page));
            w::DocEvent::Region(w::Region {
                page: page.0,
                rect: box_to_wit(to_view.apply_box(*rect)),
            })
        }
        DocEvent::RegionCleared => w::DocEvent::RegionCleared,
    }
}

pub(crate) fn text_match_to_wit(m: TextMatch) -> w::TextMatch {
    w::TextMatch {
        text: m.text,
        rects: m.rects.into_iter().map(box_to_wit).collect(),
        groups: m.groups,
    }
}

pub(crate) fn form_from_wit(f: wu::Form) -> ui::Form {
    ui::Form {
        title: f.title,
        submit_label: f
            .submit_label
            .filter(|l| !l.trim().is_empty())
            .unwrap_or_else(|| "OK".into()),
        fields: f
            .fields
            .into_iter()
            .map(|f| ui::Field {
                id: f.id,
                label: f.label,
                kind: match f.kind {
                    wu::FieldKind::Text(t) => ui::FieldKind::Text {
                        default: t.default,
                        placeholder: t.placeholder,
                    },
                    wu::FieldKind::Number(n) => ui::FieldKind::Number {
                        default: n.default,
                        min: n.min,
                        max: n.max,
                    },
                    wu::FieldKind::Choice(c) => ui::FieldKind::Choice {
                        default: (c.default as usize).min(c.options.len().saturating_sub(1)),
                        options: c.options,
                    },
                    wu::FieldKind::Checkbox(b) => ui::FieldKind::Checkbox(b),
                    wu::FieldKind::File(f) => ui::FieldKind::File {
                        save: f.save,
                        extensions: f.extensions,
                    },
                    wu::FieldKind::PageRange(r) => ui::FieldKind::PageRange(r),
                },
            })
            .collect(),
    }
}

pub(crate) fn answer_to_wit(a: ui::FieldAnswer) -> wu::FieldAnswer {
    wu::FieldAnswer {
        id: a.id,
        value: match a.value {
            ui::FieldValue::Text(t) => wu::FieldValue::Text(t),
            ui::FieldValue::Number(n) => wu::FieldValue::Number(n),
            ui::FieldValue::Choice(i) => wu::FieldValue::Choice(i as u32),
            ui::FieldValue::Checkbox(b) => wu::FieldValue::Checkbox(b),
            ui::FieldValue::File(p) => wu::FieldValue::File(p),
            ui::FieldValue::PageRange(ids) => {
                wu::FieldValue::PageRange(ids.into_iter().map(|p| p.0).collect())
            }
        },
    }
}

pub(crate) fn row_from_wit(r: wu::PanelRow) -> ui::PanelRow {
    ui::PanelRow {
        id: r.id,
        label: r.label,
        detail: r.detail,
        badge: r.badge,
        icon: match r.icon {
            wu::Icon::None => ui::Icon::None,
            wu::Icon::Check => ui::Icon::Check,
            wu::Icon::Cross => ui::Icon::Cross,
            wu::Icon::Link => ui::Icon::Link,
            wu::Icon::Warning => ui::Icon::Warning,
            wu::Icon::Info => ui::Icon::Info,
            wu::Icon::Page => ui::Icon::Page,
        },
        depth: r.depth,
        page: r.page.map(PageId),
        actions: r
            .actions
            .into_iter()
            .map(|a| ui::RowAction {
                id: a.id,
                label: a.label,
            })
            .collect(),
    }
}

// ----- interfaces -----

impl api::types::Host for HostState {}

impl api::document::Host for HostState {
    fn path(&mut self) -> Option<String> {
        self.doc().map(|d| d.path.display().to_string())
    }

    fn pages(&mut self) -> Vec<api::types::PageInfo> {
        let Some(d) = self.doc() else {
            return Vec::new();
        };
        d.snapshot
            .state
            .pages
            .iter()
            .map(|p| page_info(&d.snapshot, p))
            .collect()
    }

    fn outline(&mut self) -> Vec<api::types::OutlineItem> {
        let Some(d) = self.doc() else {
            return Vec::new();
        };
        d.snapshot
            .state
            .outline_flat()
            .into_iter()
            .map(|(depth, n)| outline_item(&d.snapshot, depth, n))
            .collect()
    }

    fn annotations(&mut self, page: u64) -> Vec<api::types::Annot> {
        let Some(d) = self.doc() else {
            return Vec::new();
        };
        let page = PageId(page);
        let to_view = to_frame(&d.snapshot, page);
        d.snapshot
            .state
            .annots_on(page)
            .map(|l| {
                l.iter()
                    .filter_map(|a| annot_to_wit(&a.mapped(&to_view)))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn selection(&mut self) -> Vec<u64> {
        self.doc()
            .map(|d| d.selection.iter().map(|p| p.0).collect())
            .unwrap_or_default()
    }

    fn current_page(&mut self) -> Option<u64> {
        self.doc()?.current_page.map(|p| p.0)
    }
}

impl api::text::Host for HostState {
    fn page_text(&mut self, page: u64) -> Result<String, String> {
        let d = self.doc().ok_or("no document is open")?;
        let id = PageId(page);
        if let Some(layer) = d.snapshot.state.text_layer(id) {
            return Ok(layer.text());
        }
        let p = d
            .snapshot
            .state
            .pages
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| format!("no page {page}"))?;
        d.text
            .page_text(p.source, p.index)
            .map_err(|e| format!("{e:#}"))
    }

    fn pattern_search(
        &mut self,
        page: u64,
        regex: String,
    ) -> Result<Vec<api::types::TextMatch>, String> {
        let d = self.doc().ok_or("no document is open")?;
        let re = regex::RegexBuilder::new(&regex)
            .size_limit(1 << 20)
            .build()
            .map_err(|e| e.to_string())?;
        let id = PageId(page);
        let found: Vec<TextMatch> = match d.snapshot.state.text_layer(id) {
            Some(layer) => layer_matches(
                sheaf_doc::text_layer::pattern_search(layer, &re),
                to_frame(&d.snapshot, id),
            ),
            None => {
                let p = d
                    .snapshot
                    .state
                    .pages
                    .iter()
                    .find(|p| p.id == id)
                    .ok_or_else(|| format!("no page {page}"))?;
                d.text
                    .pattern_search(p.source, p.index, &Arc::new(re))
                    .map_err(|e| format!("{e:#}"))?
            }
        };
        Ok(found.into_iter().map(text_match_to_wit).collect())
    }

    fn search(
        &mut self,
        page: u64,
        query: String,
        match_case: bool,
    ) -> Result<Vec<api::types::TextMatch>, String> {
        let (d, p) = self.page_ref(page)?;
        let found: Vec<TextMatch> = match d.snapshot.state.text_layer(p.id) {
            Some(layer) => layer_matches(
                sheaf_doc::text_layer::search(layer, &query, match_case),
                to_frame(&d.snapshot, p.id),
            ),
            None => d
                .text
                .search(p.source, p.index, &query, match_case)
                .map_err(|e| format!("{e:#}"))?,
        };
        Ok(found.into_iter().map(text_match_to_wit).collect())
    }

    fn words(
        &mut self,
        page: u64,
        rect: Option<api::types::Box2>,
    ) -> Result<Vec<api::types::TextMatch>, String> {
        static WORD: std::sync::LazyLock<Arc<regex::Regex>> =
            std::sync::LazyLock::new(|| Arc::new(regex::Regex::new(r"\S+").unwrap()));
        let (d, p) = self.page_ref(page)?;
        let words: Vec<TextMatch> = match d.snapshot.state.text_layer(p.id) {
            Some(layer) => layer_matches(
                sheaf_doc::text_layer::pattern_search(layer, &WORD),
                to_frame(&d.snapshot, p.id),
            ),
            None => d
                .text
                .pattern_search(p.source, p.index, &WORD)
                .map_err(|e| format!("{e:#}"))?,
        };
        let within = |m: &TextMatch| match rect {
            None => true,
            Some(r) => m.rects.first().is_some_and(|b| {
                let (cx, cy) = (b.x + b.w / 2.0, b.y + b.h / 2.0);
                cx >= r.x && cx <= r.x + r.w && cy >= r.y && cy <= r.y + r.h
            }),
        };
        Ok(words
            .into_iter()
            .filter(within)
            .map(|mut m| {
                m.groups.clear();
                text_match_to_wit(m)
            })
            .collect())
    }

    fn text_boxes(
        &mut self,
        page: u64,
        rect: api::types::Box2,
    ) -> Result<Vec<api::types::Box2>, String> {
        let (d, p) = self.page_ref(page)?;
        let rect = box_from_wit(rect);
        let boxes = match d.snapshot.state.text_layer(p.id) {
            // `rect` is in the page's frame, the layer in PageSpace.
            Some(layer) => {
                let to_view = to_frame(&d.snapshot, p.id);
                layer
                    .boxes_in_rect(to_view.inverse().apply_box(rect))
                    .into_iter()
                    .map(|b| to_view.apply_box(b))
                    .collect()
            }
            None => d
                .text
                .text_boxes(p.source, p.index, rect)
                .map_err(|e| format!("{e:#}"))?,
        };
        Ok(boxes.into_iter().map(box_to_wit).collect())
    }
}

impl api::patterns::Host for HostState {
    fn presets(&mut self) -> Vec<api::patterns::Preset> {
        self.call
            .as_ref()
            .map(|c| {
                c.presets
                    .iter()
                    .map(|p| api::patterns::Preset {
                        name: p.name.clone(),
                        regex: p.regex.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl api::ocr::Host for HostState {
    fn recognize(
        &mut self,
        page: u64,
        options: api::ocr::OcrOptions,
    ) -> Result<api::ocr::TextLayer, String> {
        let (d, p) = self.page_ref(page)?;
        let ocr = d.ocr.clone().ok_or("OCR isn't available")?;
        let geom = d.snapshot.geom(&p);
        let render = d.snapshot.render_delta(&p);
        let to_view = to_frame(&d.snapshot, p.id);
        let started = Instant::now();
        let layer = ocr.recognize(&p, &geom, render, options.language, options.dpi);
        // The engine's time isn't the plugin's.
        self.deadline += started.elapsed();
        layer
            .map(|l| layer_to_wit(&l, to_view))
            .map_err(|e| format!("{e:#}"))
    }

    fn page_text_layer(&mut self, page: u64) -> Option<api::ocr::TextLayer> {
        let d = self.doc()?;
        let page = PageId(page);
        d.snapshot
            .state
            .text_layer(page)
            .map(|l| layer_to_wit(l, to_frame(&d.snapshot, page)))
    }
}

impl api::ui::Host for HostState {
    fn show_form(&mut self, form: api::ui::Form) -> Option<Vec<api::ui::FieldAnswer>> {
        let ui = self.ui()?;
        let started = Instant::now();
        let answer = ui.show_form(form_from_wit(form));
        // The user's thinking time isn't the plugin's.
        self.deadline += started.elapsed();
        answer.map(|a| a.into_iter().map(answer_to_wit).collect())
    }

    fn notice(&mut self, message: String) {
        match self.ui() {
            Some(ui) => ui.notice(message),
            None => log::info!(target: "plugin", "[{}] {message}", self.plugin),
        }
    }

    fn set_panel(&mut self, panel: String, rows: Vec<api::ui::PanelRow>) {
        if !self.panels.contains(&panel) {
            log::warn!(
                target: "plugin",
                "[{}] set-panel: {panel:?} isn't declared in the manifest",
                self.plugin
            );
            return;
        }
        if let Some(ui) = self.ui() {
            ui.set_panel(panel, rows.into_iter().map(row_from_wit).collect());
        }
    }

    fn show_panel(&mut self, panel: String) {
        // The plugin's own panels first, so a panel it calls "comments"
        // is still its own.
        let which = if self.panels.contains(&panel) {
            ShowPanel::Plugin(panel)
        } else if let Some(b) = BuiltinPanel::from_name(&panel) {
            ShowPanel::Builtin(b)
        } else {
            log::warn!(
                target: "plugin",
                "[{}] show-panel: there's no panel {panel:?}",
                self.plugin
            );
            return;
        };
        if let Some(ui) = self.ui() {
            ui.show_panel(which);
        }
    }
}

impl api::edits::Host for HostState {
    fn submit(&mut self, label: String, edits: Vec<api::types::Edit>) {
        let snap = self.doc().map(|d| &d.snapshot);
        let edits = edits.into_iter().map(|e| edit_from_wit(snap, e)).collect();
        self.batches.push(EditBatch { label, edits });
    }
}

impl api::log::Host for HostState {
    fn log(&mut self, level: api::log::Level, message: String) {
        use api::log::Level;
        let level = match level {
            Level::Trace => log::Level::Trace,
            Level::Debug => log::Level::Debug,
            Level::Info => log::Level::Info,
            Level::Warn => log::Level::Warn,
            Level::Error => log::Level::Error,
        };
        log::log!(target: "plugin", level, "[{}] {message}", self.plugin);
    }
}

impl api::render::Host for HostState {}

impl api::render::HostPageRenderer for HostState {
    fn new(&mut self, page: u64) -> Resource<PageRenderer> {
        // An unknown page still gets a handle; `render` reports it.
        let found = self.doc().and_then(|d| {
            let p = d
                .snapshot
                .state
                .pages
                .iter()
                .find(|p| p.id == PageId(page))
                .copied()?;
            let s = d.snapshot.source(p.source).map(|s| s.src.page_size(p.index))?;
            Some((p, (s.width, s.height)))
        });
        let (page, size) = found.unwrap_or((
            PageRef {
                id: PageId(page),
                source: SourceId(u32::MAX),
                index: 0,
                rotation: sheaf_core::Rotation::R0,
            },
            (0.0, 0.0),
        ));
        let r = self
            .table
            .push(PageRenderer { page, size })
            .expect("resource table full");
        self.render_handles.renderers.push(r.rep());
        r
    }

    fn size(&mut self, this: Resource<PageRenderer>) -> (f32, f32) {
        self.table.get(&this).map(|r| r.size).unwrap_or((0.0, 0.0))
    }

    fn render(
        &mut self,
        this: Resource<PageRenderer>,
        options: api::render::RenderOptions,
    ) -> Result<Resource<RenderedImage>, String> {
        let (page, (pw, ph)) = {
            let r = self.table.get(&this).map_err(|e| e.to_string())?;
            (r.page, r.size)
        };
        if pw <= 0.0 {
            return Err(format!("no page {}", page.id.0));
        }
        let d = self.doc().ok_or("no document is open")?;
        let source = d.render.clone().ok_or("rendering isn't available")?;
        let dpi = options.dpi.clamp(1, MAX_DPI);
        let scale = dpi as f32 / 72.0;
        let (page_w, page_h) = (
            (pw * scale).round().max(1.0) as u32,
            (ph * scale).round().max(1.0) as u32,
        );
        let (x0, y0, x1, y1) = match options.region {
            Some(r) => {
                let r = box_from_wit(r);
                (
                    (r.x.max(0.0) * scale).floor() as u32,
                    (r.y.max(0.0) * scale).floor() as u32,
                    ((r.x + r.w) * scale).ceil().min(page_w as f32) as u32,
                    ((r.y + r.h) * scale).ceil().min(page_h as f32) as u32,
                )
            }
            None => (0, 0, page_w, page_h),
        };
        if x1 <= x0 || y1 <= y0 {
            return Err("the region is outside the page".into());
        }
        let (w, h) = (x1 - x0, y1 - y0);
        let pixels = w as u64 * h as u64;
        if self.render_handles.pixels + pixels > PIXEL_BUDGET {
            return Err(format!(
                "this call has rendered its limit of {} million pixels",
                PIXEL_BUDGET / 1_000_000
            ));
        }
        if self.render_handles.images.len() >= MAX_LIVE_IMAGES {
            return Err(format!(
                "at most {MAX_LIVE_IMAGES} images can be alive at once; drop some first"
            ));
        }
        let started = Instant::now();
        let rgba = source
            .render(page.source, page.index, page_w, page_h, (x0, y0, w, h))
            .map_err(|e| format!("{e:#}"));
        // Rendering is the host's time, not the plugin's.
        self.deadline += started.elapsed();
        let rgba = rgba?;
        self.render_handles.pixels += pixels;
        let gray = matches!(options.format, api::render::PixelFormat::Gray8);
        let data = if gray {
            rgba.as_chunks::<4>()
                .0
                .iter()
                .map(|p| ((p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000) as u8)
                .collect()
        } else {
            rgba
        };
        let image = RenderedImage {
            width: w,
            height: h,
            gray,
            data,
            origin: (x0 as f32 / scale, y0 as f32 / scale),
            scale,
        };
        let r = self.table.push(image).map_err(|e| e.to_string())?;
        self.render_handles.images.push(r.rep());
        Ok(r)
    }

    fn drop(&mut self, this: Resource<PageRenderer>) -> wasmtime::Result<()> {
        let rep = this.rep();
        self.render_handles.renderers.retain(|r| *r != rep);
        self.table.delete(this)?;
        Ok(())
    }
}

impl api::render::HostImage for HostState {
    fn width(&mut self, this: Resource<RenderedImage>) -> u32 {
        self.table.get(&this).map(|i| i.width).unwrap_or(0)
    }

    fn height(&mut self, this: Resource<RenderedImage>) -> u32 {
        self.table.get(&this).map(|i| i.height).unwrap_or(0)
    }

    fn format(&mut self, this: Resource<RenderedImage>) -> api::render::PixelFormat {
        match self.table.get(&this) {
            Ok(i) if !i.gray => api::render::PixelFormat::Rgba8,
            _ => api::render::PixelFormat::Gray8,
        }
    }

    fn to_page(&mut self, this: Resource<RenderedImage>, x: f32, y: f32) -> api::types::Pt {
        match self.table.get(&this) {
            Ok(i) => api::types::Pt {
                x: i.origin.0 + x / i.scale,
                y: i.origin.1 + y / i.scale,
            },
            Err(_) => api::types::Pt { x: 0.0, y: 0.0 },
        }
    }

    fn read_rows(&mut self, this: Resource<RenderedImage>, y: u32, count: u32) -> Vec<u8> {
        let Ok(i) = self.table.get(&this) else {
            return Vec::new();
        };
        let stride = (i.width * i.bpp()) as usize;
        let start = (y.min(i.height) as usize) * stride;
        let end = ((y.saturating_add(count)).min(i.height) as usize) * stride;
        i.data[start..end].to_vec()
    }

    fn read_rect(
        &mut self,
        this: Resource<RenderedImage>,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
    ) -> Vec<u8> {
        let Ok(i) = self.table.get(&this) else {
            return Vec::new();
        };
        let bpp = i.bpp() as usize;
        let (x0, x1) = (
            x.min(i.width) as usize,
            x.saturating_add(w).min(i.width) as usize,
        );
        let (y0, y1) = (y.min(i.height), y.saturating_add(h).min(i.height));
        let stride = i.width as usize * bpp;
        let mut out = Vec::with_capacity((x1 - x0) * bpp * (y1 - y0) as usize);
        for row in y0..y1 {
            let base = row as usize * stride;
            out.extend_from_slice(&i.data[base + x0 * bpp..base + x1 * bpp]);
        }
        out
    }

    fn drop(&mut self, this: Resource<RenderedImage>) -> wasmtime::Result<()> {
        let rep = this.rep();
        self.render_handles.images.retain(|r| *r != rep);
        self.table.delete(this)?;
        Ok(())
    }
}

pub(crate) fn unit(u: api::progress::Unit) -> Unit {
    match u {
        api::progress::Unit::Items => Unit::ITEMS,
        api::progress::Unit::Pages => sheaf_doc::PAGES,
        api::progress::Unit::Bytes => Unit::Bytes,
    }
}

impl api::progress::Host for HostState {}

impl api::progress::HostTask for HostState {
    fn new(&mut self, label: String, total: Option<u64>, u: api::progress::Unit) -> Resource<Task> {
        let t = match self.progress() {
            Some(root) if self.has_task_room() => root.child_tracker(&label, 1.0),
            _ => Tracker::new(),
        };
        t.set_total(total, unit(u));
        self.push_task(t)
    }

    fn set_total(&mut self, this: Resource<Task>, total: Option<u64>, u: api::progress::Unit) {
        if let Some(t) = self.task(&this) {
            t.set_total(total, unit(u));
        }
    }

    fn set(&mut self, this: Resource<Task>, done: u64) {
        if let Some(t) = self.task(&this) {
            t.set(done);
        }
    }

    fn inc(&mut self, this: Resource<Task>, n: u64) {
        if let Some(t) = self.task(&this) {
            t.inc(n);
        }
    }

    fn message(&mut self, this: Resource<Task>, level: api::progress::Level, message: String) {
        if let Some(t) = self.task(&this) {
            let level = match level {
                api::progress::Level::Info => Level::Info,
                api::progress::Level::Warn => Level::Warn,
                api::progress::Level::Error => Level::Error,
            };
            // A status line, not a document.
            let message: String = message.chars().take(500).collect();
            t.message(level, &message);
        }
    }

    /// Only the call is ever cancelled, so ask it: a task past the cap
    /// isn't under it.
    fn is_cancelled(&mut self, _this: Resource<Task>) -> bool {
        self.progress().is_some_and(|t| t.is_cancelled())
    }

    fn child(&mut self, this: Resource<Task>, label: String, weight: f32) -> Resource<Task> {
        let child = match self.task(&this) {
            Some(t) if self.has_task_room() => {
                t.child_tracker(&label, if weight.is_finite() { f64::from(weight) } else { 0.0 })
            }
            _ => Tracker::new(),
        };
        self.push_task(child)
    }

    fn drop(&mut self, this: Resource<Task>) -> wasmtime::Result<()> {
        let rep = this.rep();
        self.task_handles.retain(|r| *r != rep);
        let _ = self.table.delete(this);
        Ok(())
    }
}
