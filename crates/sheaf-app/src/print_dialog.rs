//! File > Print (Ctrl+P): sheaf's own print dialog with a preview of each
//! sheet, as Acrobat's is. Printers and their paper come from
//! `sheaf_print`, read in the background when the dialog opens; the job
//! itself is run by the document view (`DocView::print`).
//!
//! Tab moves between rows, Left/Right change the highlighted row,
//! Page Up/Page Down step the preview, Enter prints, Escape cancels.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, MouseButton, ObjectFit,
    RenderImage, Subscription, Task, Window, div, img, prelude::*, px,
};
use serde::{Deserialize, Serialize};
use sheaf_core::PageIndex;
use sheaf_doc::{PageId, Snapshot, SourceId};
use sheaf_image::raster::PixelFormat;
use sheaf_print::{Duplex, JobSettings, Orientation, Paper, Printer, Sizing};
use sheaf_render::{Job, RenderHandle, RenderRequest};
use sheaf_ui::button::{Button, Chip};
use sheaf_ui::text_input::{TextInput, TextInputEvent};
use sheaf_ui::{dialog, theme};

use crate::{DialogCancel, DialogLeft, DialogNext, DialogPrev, DialogRight, DialogRun};

pub const KEY_CONTEXT: &str = "PrintDialog";

/// The pseudo-printer that writes the print file instead.
pub const SAVE_AS_PDF: &str = "Save as PDF";

/// The preview's sheet fits in this box, logical px.
const PREVIEW_W: f32 = 300.0;
const PREVIEW_H: f32 = 390.0;

/// Choices remembered between jobs (in `state.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrintPrefs {
    pub printer: Option<String>,
    pub paper: Option<String>,
    pub sizing: String,
    pub orientation: String,
    pub duplex: String,
    pub markup: bool,
    pub collate: bool,
}

impl Default for PrintPrefs {
    fn default() -> Self {
        Self {
            printer: None,
            paper: None,
            sizing: "shrink".into(),
            orientation: "auto".into(),
            duplex: "off".into(),
            markup: true,
            collate: true,
        }
    }
}

const SIZINGS: [(Sizing, &str, &str); 3] = [
    (Sizing::Fit, "fit", "Fit"),
    (Sizing::ActualSize, "actual", "Actual size"),
    (Sizing::ShrinkOversized, "shrink", "Shrink oversized pages"),
];
const ORIENTATIONS: [(Orientation, &str, &str); 3] = [
    (Orientation::Auto, "auto", "Auto"),
    (Orientation::Portrait, "portrait", "Portrait"),
    (Orientation::Landscape, "landscape", "Landscape"),
];
const DUPLEXES: [(Duplex, &str, &str); 3] = [
    (Duplex::Off, "off", "Off"),
    (Duplex::LongEdge, "long", "Flip on long edge"),
    (Duplex::ShortEdge, "short", "Flip on short edge"),
];

fn by_id<T: Copy>(table: &[(T, &str, &str)], id: &str, fallback: T) -> T {
    table
        .iter()
        .find(|(_, i, _)| *i == id)
        .map_or(fallback, |t| t.0)
}

fn id_of<T: Copy + PartialEq>(table: &[(T, &'static str, &str)], v: T) -> &'static str {
    table
        .iter()
        .find(|(t, _, _)| *t == v)
        .map_or("", |(_, i, _)| i)
}

/// Which pages to print.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pages {
    All,
    Current,
    Selected,
    Bookmark,
    Range,
}

/// What the document view knows that printing needs.
pub struct PrintContext {
    /// The document's file name, for the print queue.
    pub title: String,
    pub snapshot: Snapshot,
    pub handles: HashMap<SourceId, RenderHandle>,
    pub current: usize,
    /// Selected pages (display indices, in order).
    pub selected: Vec<usize>,
    /// The selected bookmark's title and its pages.
    pub bookmark: Option<(String, Vec<usize>)>,
    pub prefs: PrintPrefs,
}

/// What to print and how.
#[derive(Debug, Clone)]
pub struct PrintRequest {
    pub job: JobSettings,
    pub pages: Vec<PageId>,
    pub markup: bool,
    /// Save the print file instead of printing (the Save as PDF printer).
    pub save_pdf: bool,
    /// To remember for next time.
    pub prefs: PrintPrefs,
}

pub enum PrintEvent {
    Print(Box<PrintRequest>),
    Dismiss,
}

const ROW_PRINTER: usize = 0;
const ROW_COPIES: usize = 1;
const ROW_PAGES: usize = 2;
const ROW_RANGE: usize = 3;
const ROW_SIZING: usize = 4;
const ROW_ORIENT: usize = 5;
const ROW_PAPER: usize = 6;
const ROW_DUPLEX: usize = 7;
const ROW_MARKUP: usize = 8;
const ROWS: usize = 9;

pub struct PrintDialog {
    focus: FocusHandle,
    ctx: PrintContext,
    /// None while the system is being asked.
    printers: Option<Vec<Printer>>,
    printer: usize,
    paper: usize,
    copies: Entity<TextInput>,
    collate: bool,
    pages: Pages,
    range: Entity<TextInput>,
    sizing: Sizing,
    orientation: Orientation,
    duplex: Duplex,
    markup: bool,
    /// Which of the pages to print the preview shows.
    preview: usize,
    /// Preview images by (display index, device width).
    images: HashMap<(usize, u32), Arc<RenderImage>>,
    /// Images no longer shown, released on the next render.
    retired: Vec<Arc<RenderImage>>,
    pending: Option<((usize, u32), Task<()>)>,
    row: usize,
    error: Option<String>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<PrintEvent> for PrintDialog {}

impl Focusable for PrintDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl PrintDialog {
    pub fn new(ctx: PrintContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let copies = cx.new(|cx| {
            let mut t = TextInput::new("1", cx);
            t.set_text("1".to_string(), cx);
            t
        });
        let range = cx.new(|cx| TextInput::new("e.g. 1-3, 7, 10-", cx));
        let subs = vec![
            cx.subscribe(&copies, |this, _, ev: &TextInputEvent, cx| match ev {
                TextInputEvent::Confirm => this.print(cx),
                TextInputEvent::Cancel => cx.emit(PrintEvent::Dismiss),
            }),
            cx.subscribe(&range, |this, _, ev: &TextInputEvent, cx| match ev {
                TextInputEvent::Confirm => {
                    this.pages = Pages::Range;
                    this.print(cx);
                }
                TextInputEvent::Cancel => cx.emit(PrintEvent::Dismiss),
            }),
            cx.observe(&range, |this, _, cx| {
                if this.pages == Pages::Range {
                    this.preview = 0;
                    cx.notify();
                }
            }),
        ];
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move { sheaf_print::printers() })
                .await;
            this.update(cx, |d, cx| d.got_printers(found, cx)).ok();
        })
        .detach();
        let prefs = ctx.prefs.clone();
        let pages = if !ctx.selected.is_empty() {
            Pages::Selected
        } else {
            Pages::All
        };
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            focus,
            printers: None,
            printer: 0,
            paper: 0,
            copies,
            collate: prefs.collate,
            pages,
            range,
            sizing: by_id(&SIZINGS, &prefs.sizing, Sizing::ShrinkOversized),
            orientation: by_id(&ORIENTATIONS, &prefs.orientation, Orientation::Auto),
            duplex: by_id(&DUPLEXES, &prefs.duplex, Duplex::Off),
            markup: prefs.markup,
            preview: 0,
            images: HashMap::new(),
            retired: Vec::new(),
            pending: None,
            row: ROW_PRINTER,
            error: None,
            _subs: subs,
            ctx,
        }
    }

    fn got_printers(&mut self, found: anyhow::Result<Vec<Printer>>, cx: &mut Context<Self>) {
        let mut list = match found {
            Ok(l) => l,
            Err(e) => {
                log::warn!("listing printers: {e:#}");
                Vec::new()
            }
        };
        // Always offered last: writes the print file.
        list.push(Printer {
            name: SAVE_AS_PDF.into(),
            is_default: false,
            papers: vec![
                Paper::letter(),
                Paper {
                    id: "A4".into(),
                    label: "A4".into(),
                    width: 595.0,
                    height: 842.0,
                },
            ],
            default_paper: 0,
        });
        self.printer = self
            .ctx
            .prefs
            .printer
            .as_ref()
            .and_then(|want| list.iter().position(|p| &p.name == want))
            .or_else(|| list.iter().position(|p| p.is_default))
            .unwrap_or(0);
        self.printers = Some(list);
        self.reset_paper();
        cx.notify();
    }

    fn current_printer(&self) -> Option<&Printer> {
        self.printers.as_ref()?.get(self.printer)
    }

    /// The printer's default paper, or the remembered one if it has it.
    fn reset_paper(&mut self) {
        let Some(p) = self.current_printer() else {
            return;
        };
        let remembered = self
            .ctx
            .prefs
            .paper
            .as_ref()
            .and_then(|id| p.papers.iter().position(|x| &x.id == id));
        self.paper = remembered.unwrap_or(p.default_paper);
    }

    fn current_paper(&self) -> Paper {
        self.current_printer()
            .and_then(|p| p.papers.get(self.paper))
            .cloned()
            .unwrap_or_else(Paper::letter)
    }

    /// Display indices of the pages to print, or why there are none.
    fn page_list(&self, cx: &App) -> Result<Vec<usize>, String> {
        let n = self.ctx.snapshot.state.pages.len();
        match self.pages {
            Pages::All => Ok((0..n).collect()),
            Pages::Current => Ok(vec![self.ctx.current.min(n.saturating_sub(1))]),
            Pages::Selected if self.ctx.selected.is_empty() => Err("No pages are selected".into()),
            Pages::Selected => Ok(self.ctx.selected.clone()),
            Pages::Bookmark => match &self.ctx.bookmark {
                Some((_, pages)) if !pages.is_empty() => Ok(pages.clone()),
                _ => Err("The selected bookmark has no pages".into()),
            },
            Pages::Range => sheaf_ops::pages::parse(self.range.read(cx).text(), n),
        }
    }

    fn pages_enabled(&self, p: Pages) -> bool {
        match p {
            Pages::Selected => !self.ctx.selected.is_empty(),
            Pages::Bookmark => self.ctx.bookmark.is_some(),
            _ => true,
        }
    }

    fn set_row(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.row = row.min(ROWS - 1);
        let target = match self.row {
            ROW_COPIES => self.copies.read(cx).focus_handle(cx),
            ROW_RANGE => self.range.read(cx).focus_handle(cx),
            _ => self.focus.clone(),
        };
        window.focus(&target, cx);
        cx.notify();
    }

    fn step_row(&mut self, by: i64, window: &mut Window, cx: &mut Context<Self>) {
        let row = (self.row as i64 + by).rem_euclid(ROWS as i64) as usize;
        self.set_row(row, window, cx);
    }

    /// Left/Right on the highlighted row.
    fn adjust(&mut self, by: i64, cx: &mut Context<Self>) {
        fn cycle<T: Copy + PartialEq>(items: &[T], current: T, by: i64) -> T {
            let i = items.iter().position(|x| *x == current).unwrap_or(0) as i64;
            items[(i + by).rem_euclid(items.len() as i64) as usize]
        }
        let wrap = |i: usize, n: usize| (i as i64 + by).rem_euclid(n.max(1) as i64) as usize;
        match self.row {
            ROW_PRINTER => {
                if let Some(n) = self.printers.as_ref().map(Vec::len) {
                    self.printer = wrap(self.printer, n);
                    self.reset_paper();
                }
            }
            ROW_COPIES => self.collate = !self.collate,
            ROW_PAGES => {
                let opts: Vec<Pages> = [
                    Pages::All,
                    Pages::Current,
                    Pages::Selected,
                    Pages::Bookmark,
                    Pages::Range,
                ]
                .into_iter()
                .filter(|p| self.pages_enabled(*p))
                .collect();
                self.pages = cycle(&opts, self.pages, by);
                self.preview = 0;
            }
            ROW_SIZING => {
                let opts: Vec<Sizing> = SIZINGS.iter().map(|s| s.0).collect();
                self.sizing = cycle(&opts, self.sizing, by);
            }
            ROW_ORIENT => {
                let opts: Vec<Orientation> = ORIENTATIONS.iter().map(|s| s.0).collect();
                self.orientation = cycle(&opts, self.orientation, by);
            }
            ROW_PAPER => {
                let n = self.current_printer().map_or(0, |p| p.papers.len());
                self.paper = wrap(self.paper, n);
            }
            ROW_DUPLEX => {
                let opts: Vec<Duplex> = DUPLEXES.iter().map(|s| s.0).collect();
                self.duplex = cycle(&opts, self.duplex, by);
            }
            ROW_MARKUP => self.markup = !self.markup,
            _ => {}
        }
        self.error = None;
        cx.notify();
    }

    fn step_preview(&mut self, by: i64, cx: &mut Context<Self>) {
        let n = self.page_list(cx).map(|l| l.len()).unwrap_or(0);
        if n > 0 {
            self.preview = (self.preview as i64 + by).clamp(0, n as i64 - 1) as usize;
            cx.notify();
        }
    }

    fn print(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        let Some(printer) = self.current_printer().cloned() else {
            self.error = Some("Still looking for printers…".into());
            cx.notify();
            return;
        };
        let copies = match self.copies.read(cx).text().trim().parse::<u32>() {
            Ok(n) if (1..=999).contains(&n) => n,
            _ => {
                self.error = Some("Copies must be 1–999".into());
                cx.notify();
                return;
            }
        };
        let list = match self.page_list(cx) {
            Ok(l) if !l.is_empty() => l,
            Ok(_) => {
                self.error = Some("No pages to print".into());
                cx.notify();
                return;
            }
            Err(e) => {
                self.error = Some(e);
                cx.notify();
                return;
            }
        };
        let pages = self.ctx.snapshot.state.pages.clone();
        let ids: Vec<PageId> = list
            .iter()
            .filter_map(|&i| pages.get(i))
            .map(|p| p.id)
            .collect();
        let paper = self.current_paper();
        let prefs = PrintPrefs {
            printer: Some(printer.name.clone()),
            paper: Some(paper.id.clone()),
            sizing: id_of(&SIZINGS, self.sizing).into(),
            orientation: id_of(&ORIENTATIONS, self.orientation).into(),
            duplex: id_of(&DUPLEXES, self.duplex).into(),
            markup: self.markup,
            collate: self.collate,
        };
        cx.emit(PrintEvent::Print(Box::new(PrintRequest {
            save_pdf: printer.name == SAVE_AS_PDF,
            job: JobSettings {
                printer: printer.name,
                title: self.ctx.title.clone(),
                copies,
                collate: self.collate,
                paper,
                sizing: self.sizing,
                orientation: self.orientation,
                duplex: self.duplex,
            },
            pages: ids,
            markup: self.markup,
            prefs,
        })));
    }

    /// Let go of the preview's GPU textures (on close).
    pub fn release(&mut self, window: &mut Window) {
        for (_, im) in self.images.drain() {
            let _ = window.drop_image(im);
        }
        for im in self.retired.drain(..) {
            let _ = window.drop_image(im);
        }
    }

    /// Ask for the preview image of display page `index` at `width` device px.
    fn want_image(&mut self, index: usize, width: u32, cx: &mut Context<Self>) {
        let key = (index, width);
        if self.images.contains_key(&key) || self.pending.as_ref().is_some_and(|(k, _)| *k == key) {
            return;
        }
        let Some(pref) = self.ctx.snapshot.state.pages.get(index).copied() else {
            return;
        };
        let Some(handle) = self.ctx.handles.get(&pref.source) else {
            return;
        };
        let rx = handle.render(RenderRequest {
            page: PageIndex(pref.index),
            job: Job::Page {
                width,
                rotation: self.ctx.snapshot.render_delta(&pref),
            },
            format: PixelFormat::Bgra,
            priority: 0,
        });
        let task = cx.spawn(async move |this, cx| {
            let Ok(Ok(r)) = rx.await else { return };
            this.update(cx, |d, cx| {
                let image = Arc::new(crate::docview::to_render_image(r.raster));
                // Keep only what's on screen.
                let old: Vec<(usize, u32)> = d.images.keys().copied().collect();
                for k in old {
                    if let Some(im) = d.images.remove(&k) {
                        d.retired.push(im);
                    }
                }
                d.images.insert(key, image);
                d.pending = None;
                cx.notify();
            })
            .ok();
        });
        self.pending = Some((key, task));
    }

    // ----- rendering ------------------------------------------------------------

    fn chip(&self, id: (&'static str, usize), label: String, on: bool, enabled: bool) -> Chip {
        Chip::new(id, label).selected(on).disabled(!enabled)
    }

    fn row(&self, index: usize, label: &str, content: AnyElement) -> AnyElement {
        dialog::field_row(label.to_string(), px(100.0), self.row == index, content)
            .into_any_element()
    }

    /// "‹ value ›" for a long list of choices.
    fn stepper(
        &self,
        id: &'static str,
        value: String,
        row: usize,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let arrow = |dir: i64, glyph: &'static str| {
            div()
                .id((id, (dir + 1) as usize))
                .px_2()
                .rounded_sm()
                .cursor_pointer()
                .text_color(if enabled {
                    theme::TEXT_SOFT
                } else {
                    theme::TEXT_DISABLED
                })
                .hover(|s| s.bg(theme::CONTROL_HOVER))
                .on_click(cx.listener(move |d, _, window, cx| {
                    d.set_row(row, window, cx);
                    d.adjust(dir, cx);
                }))
                .child(glyph)
        };
        div()
            .flex()
            .items_center()
            .gap_1()
            .pt(px(2.0))
            .child(arrow(-1, "‹"))
            .child(
                div()
                    .min_w(px(160.0))
                    .text_sm()
                    .text_color(theme::TEXT_BRIGHT)
                    .child(value),
            )
            .child(arrow(1, "›"))
            .into_any_element()
    }

    fn controls(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let printer = match &self.printers {
            None => div()
                .pt_1()
                .text_sm()
                .text_color(theme::TEXT_MUTED)
                .child("Looking for printers…")
                .into_any_element(),
            Some(list) => {
                let p = &list[self.printer];
                let label = if p.is_default {
                    format!("{} (default)", p.name)
                } else {
                    p.name.clone()
                };
                let only_pdf = list.len() == 1;
                let el = self.stepper("printer", label, ROW_PRINTER, true, cx);
                if only_pdf {
                    div()
                        .flex()
                        .flex_col()
                        .child(el)
                        .child(dialog::subtitle(
                            "No printers found; add one in the system's printer settings",
                        ))
                        .into_any_element()
                } else {
                    el
                }
            }
        };
        let copies = div()
            .flex()
            .items_center()
            .gap_2()
            .child(div().w(px(60.0)).child(self.copies.clone()))
            .child(
                self.chip(
                    ("collate", 0),
                    format!("{} Collate", if self.collate { "☑" } else { "☐" }),
                    false,
                    true,
                )
                .on_click(cx.listener(|d, _, window, cx| {
                    d.set_row(ROW_COPIES, window, cx);
                    d.collate = !d.collate;
                    cx.notify();
                })),
            )
            .into_any_element();
        let n = self.ctx.snapshot.state.pages.len();
        let page_opts = [
            (Pages::All, format!("All ({n})")),
            (
                Pages::Current,
                format!("Current page ({})", self.ctx.current + 1),
            ),
            (
                Pages::Selected,
                format!("Selected ({})", self.ctx.selected.len()),
            ),
            (
                Pages::Bookmark,
                match &self.ctx.bookmark {
                    Some((t, pages)) => format!("Bookmark \"{t}\" ({})", pages.len()),
                    None => "Selected bookmark".into(),
                },
            ),
            (Pages::Range, "Pages…".into()),
        ];
        let pages = div()
            .flex()
            .flex_wrap()
            .gap_1()
            .children(page_opts.into_iter().enumerate().map(|(i, (p, label))| {
                self.chip(("pages", i), label, self.pages == p, self.pages_enabled(p))
                    .on_click(cx.listener(move |d, _, window, cx| {
                        if d.pages_enabled(p) {
                            d.pages = p;
                            d.preview = 0;
                            d.set_row(
                                if p == Pages::Range {
                                    ROW_RANGE
                                } else {
                                    ROW_PAGES
                                },
                                window,
                                cx,
                            );
                        }
                    }))
            }))
            .into_any_element();
        let range = div()
            .w(px(200.0))
            .child(self.range.clone())
            .into_any_element();
        let choice = |name: &'static str,
                      row: usize,
                      opts: Vec<(&'static str, bool)>,
                      cx: &mut Context<Self>| {
            div()
                .flex()
                .flex_wrap()
                .gap_1()
                .children(opts.into_iter().enumerate().map(|(i, (label, on))| {
                    self.chip((name, i), label.to_string(), on, true)
                        .on_click(cx.listener(move |d, _, window, cx| {
                            d.set_row(row, window, cx);
                            match row {
                                ROW_SIZING => d.sizing = SIZINGS[i].0,
                                ROW_ORIENT => d.orientation = ORIENTATIONS[i].0,
                                ROW_DUPLEX => d.duplex = DUPLEXES[i].0,
                                _ => {}
                            }
                            cx.notify();
                        }))
                }))
                .into_any_element()
        };
        let sizing = choice(
            "sizing",
            ROW_SIZING,
            SIZINGS
                .iter()
                .map(|(s, _, l)| (*l, *s == self.sizing))
                .collect(),
            cx,
        );
        let orient = choice(
            "orient",
            ROW_ORIENT,
            ORIENTATIONS
                .iter()
                .map(|(s, _, l)| (*l, *s == self.orientation))
                .collect(),
            cx,
        );
        let duplex = choice(
            "duplex",
            ROW_DUPLEX,
            DUPLEXES
                .iter()
                .map(|(s, _, l)| (*l, *s == self.duplex))
                .collect(),
            cx,
        );
        let paper = {
            let p = self.current_paper();
            self.stepper(
                "paper",
                format!(
                    "{} ({:.2} × {:.2} in)",
                    p.label,
                    p.width / 72.0,
                    p.height / 72.0
                ),
                ROW_PAPER,
                self.printers.is_some(),
                cx,
            )
        };
        let markup = self
            .chip(
                ("markup", 0),
                format!(
                    "{} Comments and markups",
                    if self.markup { "☑" } else { "☐" }
                ),
                false,
                true,
            )
            .on_click(cx.listener(|d, _, window, cx| {
                d.set_row(ROW_MARKUP, window, cx);
                d.markup = !d.markup;
                cx.notify();
            }))
            .into_any_element();
        vec![
            self.row(ROW_PRINTER, "Printer", printer),
            self.row(ROW_COPIES, "Copies", copies),
            self.row(ROW_PAGES, "Pages", pages),
            self.row(ROW_RANGE, "", range),
            self.row(ROW_SIZING, "Size", sizing),
            self.row(ROW_ORIENT, "Orientation", orient),
            self.row(ROW_PAPER, "Paper", paper),
            self.row(ROW_DUPLEX, "Two-sided", duplex),
            self.row(ROW_MARKUP, "Print", markup),
        ]
    }

    /// The sheet as it will come out, with the page placed on it.
    fn preview_pane(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let list = self.page_list(cx);
        let (index, count) = match &list {
            Ok(l) if !l.is_empty() => {
                let at = self.preview.min(l.len() - 1);
                (Some(l[at]), l.len())
            }
            _ => (None, 0),
        };
        let paper = self.current_paper();
        let mut sheet = div()
            .relative()
            .overflow_hidden()
            .bg(theme::PAGE)
            .shadow_md();
        let mut caption = String::new();
        if let Some(index) = index
            && let Some(pref) = self.ctx.snapshot.state.pages.get(index).copied()
        {
            let geom = self.ctx.snapshot.geom(&pref);
            let shown = geom.shown();
            let p = sheaf_print::place(
                shown.width,
                shown.height,
                &paper,
                self.sizing,
                self.orientation,
            );
            let s = (PREVIEW_W / p.sheet_w).min(PREVIEW_H / p.sheet_h);
            let (w, h) = (p.w * s, p.h * s);
            let dev_w = (w * window.scale_factor()).round().max(1.0) as u32;
            self.want_image(index, dev_w, cx);
            let mut page = div()
                .absolute()
                .left(px(p.x * s))
                .top(px(p.y * s))
                .w(px(w))
                .h(px(h))
                .border_1()
                .border_color(theme::BORDER);
            if let Some(im) = self.images.get(&(index, dev_w)) {
                page = page.child(img(im.clone()).size_full().object_fit(ObjectFit::Fill));
            }
            if self.markup
                && let Some(list) = self.ctx.snapshot.state.annots_on(pref.id)
            {
                let (to_view, k) = (geom.to_view(), w / shown.width.max(1.0));
                for a in list.iter() {
                    let assets = &self.ctx.snapshot.assets;
                    page = page.children(crate::docview::annot_elements(
                        a, assets, &to_view, shown, k,
                    ));
                }
            }
            sheet = sheet.w(px(p.sheet_w * s)).h(px(p.sheet_h * s)).child(page);
            caption = format!(
                "{} {} · {:.0}%",
                paper.label,
                if p.landscape { "landscape" } else { "portrait" },
                p.scale * 100.0
            );
        } else {
            sheet = sheet.w(px(PREVIEW_W * 0.77)).h(px(PREVIEW_H));
        }
        let nav = |dir: i64, glyph: &'static str, enabled: bool| {
            div()
                .id(("preview-nav", (dir + 1) as usize))
                .px_2()
                .rounded_sm()
                .cursor_pointer()
                .text_color(if enabled {
                    theme::TEXT_SOFT
                } else {
                    theme::TEXT_DISABLED
                })
                .hover(|s| s.bg(theme::CONTROL_HOVER))
                .on_click(cx.listener(move |d, _, _, cx| d.step_preview(dir, cx)))
                .child(glyph)
        };
        let at = self.preview.min(count.saturating_sub(1));
        div()
            .w(px(PREVIEW_W + 20.0))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .child(
                div()
                    .w(px(PREVIEW_W))
                    .h(px(PREVIEW_H))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(theme::SUNKEN)
                    .child(sheet),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_sm()
                    .child(nav(-1, "‹", at > 0))
                    .child(if count == 0 {
                        "No pages".to_string()
                    } else {
                        format!("Sheet {} of {count}", at + 1)
                    })
                    .child(nav(1, "›", at + 1 < count)),
            )
            .child(dialog::subtitle(caption))
            .into_any_element()
    }
}

impl Render for PrintDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        for im in self.retired.drain(..) {
            let _ = window.drop_image(im);
        }
        let controls = self.controls(cx);
        let preview = self.preview_pane(window, cx);
        let save_pdf = self
            .current_printer()
            .is_some_and(|p| p.name == SAVE_AS_PDF);
        dialog::backdrop("print-backdrop")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|d, _: &DialogNext, window, cx| d.step_row(1, window, cx)))
            .on_action(cx.listener(|d, _: &DialogPrev, window, cx| d.step_row(-1, window, cx)))
            .on_action(cx.listener(|d, _: &DialogLeft, _, cx| d.adjust(-1, cx)))
            .on_action(cx.listener(|d, _: &DialogRight, _, cx| d.adjust(1, cx)))
            .on_action(cx.listener(|d, _: &DialogRun, _, cx| d.print(cx)))
            .on_action(cx.listener(|d, _: &PreviewPrev, _, cx| d.step_preview(-1, cx)))
            .on_action(cx.listener(|d, _: &PreviewNext, _, cx| d.step_preview(1, cx)))
            .on_action(cx.listener(|_, _: &DialogCancel, _, cx| cx.emit(PrintEvent::Dismiss)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(PrintEvent::Dismiss)),
            )
            .child(
                dialog::card("print-dialog", px(880.0))
                    .child(dialog::header("Print"))
                    .child(
                        div()
                            .flex()
                            .gap_3()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .flex()
                                    .flex_col()
                                    .children(controls),
                            )
                            .child(preview),
                    )
                    .when_some(self.error.clone(), |d, e| d.child(dialog::message(e, true)))
                    .child(
                        dialog::footer()
                            .child(dialog::hint(
                                "Tab moves, ←/→ change, PgUp/PgDn page the preview, Enter prints",
                            ))
                            .child(
                                Button::new("print-cancel", "Cancel").on_click(
                                    cx.listener(|_, _, _, cx| cx.emit(PrintEvent::Dismiss)),
                                ),
                            )
                            .child(
                                Button::new("print-go", if save_pdf { "Save…" } else { "Print" })
                                    .primary()
                                    .on_click(cx.listener(|d, _, _, cx| d.print(cx))),
                            ),
                    ),
            )
    }
}

gpui::actions!(print_dialog, [PreviewPrev, PreviewNext]);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefs_round_trip_through_ids() {
        for (s, id, _) in SIZINGS {
            assert_eq!(by_id(&SIZINGS, id, Sizing::Fit), s);
            assert_eq!(id_of(&SIZINGS, s), id);
        }
        assert_eq!(
            by_id(&ORIENTATIONS, "nonsense", Orientation::Auto),
            Orientation::Auto
        );
        let prefs: PrintPrefs = serde_json::from_str("{}").unwrap();
        assert_eq!(prefs, PrintPrefs::default());
    }
}
