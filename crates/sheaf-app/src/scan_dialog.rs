//! File > Create > From Scans/Photos…: photos or scans of paper to a
//! clean PDF (`sheaf_ops::scan`, as `sheaf scan` does). The images are
//! picked first; the dialog sets the options and previews each cleanup
//! step on one image at a time. Create asks where to save, runs with
//! progress (Escape stops it), and opens the result.
//!
//! Tab moves between rows, Left/Right change, Enter creates, Escape
//! cancels.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyElement, App, Context, EventEmitter, FocusHandle, Focusable, MouseButton, ObjectFit,
    RenderImage, Task, Window, div, img, prelude::*, px, relative,
};
use image::RgbImage;
use sheaf_image::clean::{Mode, Stage};
use sheaf_ocr::OcrConfig;
use sheaf_ops::images::Paper;
use sheaf_ops::scan::ScanOptions;
use sheaf_ops::{NoProgress, Tracker, was_cancelled};
use sheaf_ui::button::{Button, Chip};
use sheaf_ui::{dialog, theme};

use crate::{DialogCancel, DialogLeft, DialogNext, DialogPrev, DialogRight, DialogRun};

pub const KEY_CONTEXT: &str = "ScanDialog";

/// Previews are computed at this many pixels on the long side.
const PREVIEW_SIZE: u32 = 1200;
const DPIS: [u32; 4] = [150, 200, 300, 400];
const MODES: [(Mode, &str); 3] = [
    (Mode::Color, "Color"),
    (Mode::Gray, "Grayscale"),
    (Mode::Bw, "Black and white"),
];
const PAPERS: [(Paper, &str); 3] = [
    (Paper::Letter, "Letter"),
    (Paper::A4, "A4"),
    (Paper::Original, "As photographed"),
];

pub enum ScanEvent {
    /// The PDF is written; `opts` are worth remembering.
    Created {
        path: PathBuf,
        opts: ScanOptions,
    },
    Dismiss,
}

/// What the preview shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    /// The photo, with the page found outlined.
    Photo,
    Stage(Stage),
    Result,
}

impl View {
    fn label(self) -> &'static str {
        match self {
            View::Photo => "Photo",
            View::Stage(Stage::Cropped) => "Page",
            View::Stage(Stage::Uncurled) => "Uncurled",
            View::Stage(Stage::Flattened) => "Lighting",
            View::Stage(Stage::Upright) => "Upright",
            View::Stage(Stage::Straightened) => "Straight",
            View::Result => "Result",
        }
    }
}

const ROW_MODE: usize = 0;
const ROW_PAGE: usize = 1;
const ROW_DPI: usize = 2;
const ROW_CROP: usize = 3;
const ROW_ORIENT: usize = 4;
const ROW_DESKEW: usize = 5;
const ROW_FLATTEN: usize = 6;
const ROW_OCR: usize = 7;
const ROW_IMAGE: usize = 8;
const ROW_VIEW: usize = 9;
const ROW_CURL: usize = 10;
const ROW_BLANKS: usize = 11;

pub struct ScanDialog {
    focus: FocusHandle,
    files: Vec<PathBuf>,
    opts: ScanOptions,
    ocr: OcrConfig,
    /// Why OCR can't run, when it can't.
    ocr_unavailable: Option<String>,
    /// Why curled pages can't be flattened, when they can't.
    curl_unavailable: Option<String>,
    row: usize,
    /// Which file the preview shows.
    index: usize,
    view: View,
    /// The previewed file, decoded and shrunk.
    loaded: Option<(usize, Arc<RgbImage>)>,
    previews: Vec<(View, Arc<RenderImage>)>,
    /// The previewed page came out blank.
    preview_blank: bool,
    preview_error: Option<String>,
    generation: u64,
    _preview_task: Option<Task<()>>,
    running: Option<Tracker>,
    status: Option<String>,
    error: Option<String>,
}

impl EventEmitter<ScanEvent> for ScanDialog {}

impl Focusable for ScanDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ScanDialog {
    pub fn new(
        files: Vec<PathBuf>,
        opts: ScanOptions,
        ocr: OcrConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let ocr_unavailable = sheaf_ocr::check_engine(ocr.engine, &ocr)
            .err()
            .map(|e| e.to_string());
        let curl_unavailable = sheaf_ocr::rectify::unavailable_reason();
        let mut opts = opts;
        if ocr_unavailable.is_some() {
            opts.ocr = false;
        }
        if curl_unavailable.is_some() {
            opts.clean.flatten_curl = false;
        }
        let mut d = Self {
            focus,
            files,
            opts,
            ocr,
            ocr_unavailable,
            curl_unavailable,
            row: ROW_MODE,
            index: 0,
            view: View::Result,
            loaded: None,
            previews: Vec::new(),
            preview_blank: false,
            preview_error: None,
            generation: 0,
            _preview_task: None,
            running: None,
            status: None,
            error: None,
        };
        d.refresh_preview(cx);
        d
    }

    fn rows(&self) -> Vec<usize> {
        let mut rows = vec![
            ROW_MODE,
            ROW_PAGE,
            ROW_DPI,
            ROW_CROP,
            ROW_CURL,
            ROW_ORIENT,
            ROW_DESKEW,
            ROW_FLATTEN,
            ROW_BLANKS,
            ROW_OCR,
        ];
        if self.files.len() > 1 {
            rows.push(ROW_IMAGE);
        }
        rows.push(ROW_VIEW);
        rows
    }

    fn set_row(&mut self, row: usize, cx: &mut Context<Self>) {
        self.row = row;
        cx.notify();
    }

    fn step_row(&mut self, by: i64, cx: &mut Context<Self>) {
        let rows = self.rows();
        let at = rows.iter().position(|&r| r == self.row).unwrap_or(0) as i64;
        let next = rows[(at + by).rem_euclid(rows.len() as i64) as usize];
        self.set_row(next, cx);
    }

    fn views(&self) -> Vec<View> {
        self.previews.iter().map(|(v, _)| *v).collect()
    }

    fn adjust(&mut self, by: i64, cx: &mut Context<Self>) {
        if self.running.is_some() {
            return;
        }
        fn cycle<T: Copy + PartialEq>(items: &[T], current: T, by: i64) -> T {
            let i = items.iter().position(|x| *x == current).unwrap_or(0) as i64;
            items[(i + by).rem_euclid(items.len() as i64) as usize]
        }
        let o = &mut self.opts;
        match self.row {
            ROW_MODE => {
                let all: Vec<Mode> = MODES.iter().map(|m| m.0).collect();
                o.clean.mode = cycle(&all, o.clean.mode, by);
            }
            ROW_PAGE => {
                let all: Vec<Paper> = PAPERS.iter().map(|p| p.0).collect();
                o.page = cycle(&all, o.page, by);
            }
            ROW_DPI => o.dpi = cycle(&DPIS, o.dpi, by),
            ROW_CROP => o.clean.crop = !o.clean.crop,
            ROW_CURL => {
                if self.curl_unavailable.is_some() {
                    return;
                }
                o.clean.flatten_curl = !o.clean.flatten_curl;
            }
            ROW_ORIENT => o.clean.orient = !o.clean.orient,
            ROW_DESKEW => o.clean.deskew = !o.clean.deskew,
            ROW_FLATTEN => o.clean.flatten = !o.clean.flatten,
            ROW_BLANKS => {
                o.drop_blanks = !o.drop_blanks;
                cx.notify();
                return;
            }
            ROW_OCR => {
                if self.ocr_unavailable.is_none() {
                    o.ocr = !o.ocr;
                }
                cx.notify();
                return;
            }
            ROW_IMAGE => {
                let n = self.files.len() as i64;
                self.index = (self.index as i64 + by).rem_euclid(n) as usize;
            }
            ROW_VIEW => {
                let views = self.views();
                if !views.is_empty() {
                    self.view = cycle(&views, self.view, by);
                }
                cx.notify();
                return;
            }
            _ => return,
        }
        self.error = None;
        self.refresh_preview(cx);
        cx.notify();
    }

    /// Recompute the preview of the current file with the current options,
    /// a moment after the last change.
    fn refresh_preview(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        let index = self.index;
        let path = self.files[index].clone();
        let loaded = self
            .loaded
            .as_ref()
            .filter(|(i, _)| *i == index)
            .map(|(_, im)| im.clone());
        let mut params = self.opts.params();
        // The preview is small already; the mode's look is what matters.
        params.fit = None;
        self._preview_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(120))
                .await;
            if this
                .read_with(cx, |d, _| d.generation != generation)
                .unwrap_or(true)
            {
                return;
            }
            let result = cx
                .background_executor()
                .spawn(async move {
                    let image = match loaded {
                        Some(im) => im,
                        None => Arc::new(shrink(sheaf_image::io::load(&path)?)),
                    };
                    let views = preview(&image, &params)?;
                    anyhow::Ok((image, views))
                })
                .await;
            this.update(cx, |d, cx| {
                if d.generation != generation {
                    return;
                }
                match result {
                    Ok((image, (views, blank))) => {
                        d.loaded = Some((index, image));
                        d.preview_blank = blank;
                        if !views.iter().any(|(v, _)| *v == d.view) {
                            d.view = View::Result;
                        }
                        d.previews = views;
                        d.preview_error = None;
                    }
                    Err(e) => {
                        d.previews.clear();
                        d.preview_error = Some(format!("{e:#}"));
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Ask where to save, then run.
    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.running.is_some() {
            return;
        }
        let first = &self.files[0];
        let dir = first.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let suggested = if self.files.len() == 1 {
            format!("{}.pdf", sheaf_ops::export::stem_of(first))
        } else {
            "Scans.pdf".to_string()
        };
        let rx = crate::file_dialog::save(cx, &dir, Some(&suggested));
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = rx.await else {
                return;
            };
            handle
                .update(cx, |_, _, cx| {
                    this.update(cx, |d, cx| d.run(path, cx)).ok();
                })
                .ok();
        })
        .detach();
    }

    fn run(&mut self, out: PathBuf, cx: &mut Context<Self>) {
        use futures::FutureExt;
        let tracker = Tracker::new();
        self.running = Some(tracker.clone());
        self.error = None;
        self.status = Some("Starting…".into());
        cx.notify();
        let (files, opts, ocr) = (self.files.clone(), self.opts.clone(), self.ocr.clone());
        let progress = tracker.clone();
        let target = out.clone();
        let work = cx
            .background_executor()
            .spawn(async move { sheaf_ops::scan::scan(&files, &opts, &ocr, &target, &progress) });
        cx.spawn(async move |this, cx| {
            let mut work = work.fuse();
            let result = loop {
                let mut tick = cx
                    .background_executor()
                    .timer(Duration::from_millis(100))
                    .fuse();
                futures::select_biased! {
                    r = work => break r,
                    _ = tick => {}
                }
                let snap = tracker.snapshot();
                let alive = this.update(cx, |d, cx| {
                    if !snap.cancelled {
                        let n = d.files.len();
                        let page = ((snap.fraction.unwrap_or(0.0) * n as f32) as usize + 1).min(n);
                        d.status = Some(format!("Page {page} of {n}…"));
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    tracker.cancel();
                }
            };
            this.update(cx, |d, cx| {
                d.running = None;
                d.status = None;
                match result {
                    Ok(_) => cx.emit(ScanEvent::Created {
                        path: out,
                        opts: d.opts.clone(),
                    }),
                    Err(e) if was_cancelled(&e) => d.error = Some("Stopped".into()),
                    Err(e) => d.error = Some(format!("{e:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        match &self.running {
            Some(t) => {
                t.cancel();
                self.status = Some("Stopping after this page…".into());
                cx.notify();
            }
            None => cx.emit(ScanEvent::Dismiss),
        }
    }

    fn row_el(&self, index: usize, label: &str, content: AnyElement) -> AnyElement {
        dialog::field_row(label.to_string(), px(96.0), self.row == index, content)
            .into_any_element()
    }

    fn chips<T: Copy + PartialEq + 'static>(
        &self,
        name: &'static str,
        row: usize,
        opts: Vec<(T, String)>,
        current: T,
        set: fn(&mut Self, T),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_wrap()
            .gap_1()
            .children(opts.into_iter().enumerate().map(|(i, (v, label))| {
                Chip::new((name, i), label)
                    .selected(v == current)
                    .on_click(cx.listener(move |d, _, _, cx| {
                        if d.running.is_none() {
                            set(d, v);
                            d.error = None;
                            d.set_row(row, cx);
                            d.refresh_preview(cx);
                        }
                    }))
            }))
            .into_any_element()
    }

    fn toggle(
        &self,
        row: usize,
        label: &str,
        on: bool,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        Chip::new(
            ("scan-step", row),
            format!("{} {label}", if on { "☑" } else { "☐" }),
        )
        .disabled(!enabled)
        .on_click(cx.listener(move |d, _, _, cx| {
            if enabled {
                d.row = row;
                d.adjust(1, cx);
            }
        }))
        .into_any_element()
    }
}

/// The photo shrunk to preview size.
fn shrink(image: RgbImage) -> RgbImage {
    let (w, h) = image.dimensions();
    let s = (PREVIEW_SIZE as f32 / w.max(h) as f32).min(1.0);
    if s >= 1.0 {
        return image;
    }
    image::imageops::resize(
        &image,
        (w as f32 * s).round() as u32,
        (h as f32 * s).round() as u32,
        image::imageops::FilterType::Triangle,
    )
}

/// The photo with its page outlined, each step, and the result; and
/// whether the result is blank.
#[allow(clippy::type_complexity)]
fn preview(
    image: &RgbImage,
    params: &sheaf_image::clean::ScanParams,
) -> anyhow::Result<(Vec<(View, Arc<RenderImage>)>, bool)> {
    let mut photo = image.clone();
    if params.crop
        && let Some(q) = sheaf_image::clean::find_page(image)
    {
        outline(&mut photo, q);
    }
    let mut views = vec![(View::Photo, to_render(&photo))];
    let cleaned =
        sheaf_ops::scan::clean_image(image.clone(), params, &NoProgress, &mut |stage, im| {
            views.push((View::Stage(stage), to_render(im)))
        })?;
    let blank = sheaf_ops::scan::is_blank_page(&cleaned.image);
    views.push((View::Result, to_render(&cleaned.image.into_rgb8())));
    Ok((views, blank))
}

/// Draw `q` on `img` in the accent color.
fn outline(img: &mut RgbImage, q: sheaf_image::clean::Quad) {
    let (w, h) = img.dimensions();
    let t = (w.max(h) / 300).max(2) as i32;
    for i in 0..4 {
        let (a, b) = (q[i], q[(i + 1) % 4]);
        let n = ((b.0 - a.0).abs().max((b.1 - a.1).abs()) as i32).max(1);
        for s in 0..=n {
            let f = s as f32 / n as f32;
            let (x, y) = (a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f);
            for dy in -t..=t {
                for dx in -t..=t {
                    let (px, py) = (x as i32 + dx, y as i32 + dy);
                    if px >= 0 && py >= 0 && (px as u32) < w && (py as u32) < h {
                        img.put_pixel(px as u32, py as u32, image::Rgb([40, 120, 230]));
                    }
                }
            }
        }
    }
}

/// An RGB image as GPUI wants it (BGRA).
fn to_render(im: &RgbImage) -> Arc<RenderImage> {
    let (w, h) = im.dimensions();
    let mut bgra = Vec::with_capacity((w * h * 4) as usize);
    for p in im.pixels() {
        bgra.extend_from_slice(&[p[2], p[1], p[0], 255]);
    }
    let buf = image::RgbaImage::from_raw(w, h, bgra).expect("size matches");
    Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(
        buf
    )]))
}

impl Render for ScanDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let o = self.opts.clone();
        let running = self.running.is_some();
        let mode = self.chips(
            "mode",
            ROW_MODE,
            MODES.iter().map(|(m, l)| (*m, l.to_string())).collect(),
            o.clean.mode,
            |d, v| d.opts.clean.mode = v,
            cx,
        );
        let page = self.chips(
            "page",
            ROW_PAGE,
            PAPERS.iter().map(|(p, l)| (*p, l.to_string())).collect(),
            o.page,
            |d, v| d.opts.page = v,
            cx,
        );
        let dpi = self.chips(
            "dpi",
            ROW_DPI,
            DPIS.iter().map(|d| (*d, format!("{d} dpi"))).collect(),
            o.dpi,
            |d, v| d.opts.dpi = v,
            cx,
        );
        let crop = self.toggle(
            ROW_CROP,
            "Find the page and cut it out",
            o.clean.crop,
            true,
            cx,
        );
        let curl = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(self.toggle(
                ROW_CURL,
                "Flatten curled pages (slower)",
                o.clean.flatten_curl,
                self.curl_unavailable.is_none(),
                cx,
            ))
            .when_some(self.curl_unavailable.clone(), |d, why| {
                d.child(dialog::subtitle(why))
            })
            .into_any_element();
        let orient = self.toggle(ROW_ORIENT, "Turn it upright", o.clean.orient, true, cx);
        let deskew = self.toggle(ROW_DESKEW, "Straighten the text", o.clean.deskew, true, cx);
        let flatten = self.toggle(
            ROW_FLATTEN,
            "Even out shadows and color",
            o.clean.flatten,
            true,
            cx,
        );
        let blanks = self.toggle(ROW_BLANKS, "Leave out blank pages", o.drop_blanks, true, cx);
        let engine = self.ocr.engine.label();
        let ocr = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(self.toggle(
                ROW_OCR,
                &format!("Recognize text ({engine})"),
                o.ocr,
                self.ocr_unavailable.is_none(),
                cx,
            ))
            .when_some(self.ocr_unavailable.clone(), |d, why| {
                d.child(dialog::subtitle(why))
            })
            .into_any_element();
        let n = self.files.len();
        let image_row = div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                Button::new("scan-prev", "◀").on_click(cx.listener(|d, _, _, cx| {
                    d.row = ROW_IMAGE;
                    d.adjust(-1, cx);
                })),
            )
            .child(format!("{} of {n}", self.index + 1))
            .child(
                Button::new("scan-next", "▶").on_click(cx.listener(|d, _, _, cx| {
                    d.row = ROW_IMAGE;
                    d.adjust(1, cx);
                })),
            )
            .child(
                div().text_sm().overflow_hidden().text_ellipsis().child(
                    self.files[self.index]
                        .file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_default(),
                ),
            )
            .into_any_element();
        let views = self.views();
        let view_chips = div()
            .flex()
            .flex_wrap()
            .gap_1()
            .children(views.iter().enumerate().map(|(i, v)| {
                let v = *v;
                Chip::new(("scan-view", i), v.label())
                    .selected(v == self.view)
                    .on_click(cx.listener(move |d, _, _, cx| {
                        d.view = v;
                        d.row = ROW_VIEW;
                        cx.notify();
                    }))
            }))
            .into_any_element();
        let shown = self
            .previews
            .iter()
            .find(|(v, _)| *v == self.view)
            .map(|(_, im)| im.clone());
        let picture = div()
            .w(px(420.0))
            .h(px(460.0))
            .flex()
            .items_center()
            .justify_center()
            .bg(theme::BACKGROUND)
            .rounded_md()
            .overflow_hidden()
            .map(|d| match (&shown, &self.preview_error) {
                (_, Some(e)) => d.child(dialog::message(e.clone(), true)),
                (Some(im), _) => {
                    // A page that would be left out is dimmed, not hidden.
                    let dropped = self.opts.drop_blanks && self.preview_blank;
                    d.relative()
                        .child(
                            img(im.clone())
                                .size_full()
                                .object_fit(ObjectFit::Contain)
                                .when(dropped, |i| i.opacity(0.35)),
                        )
                        .when(dropped, |d| {
                            d.child(
                                div()
                                    .absolute()
                                    .top_2()
                                    .left_2()
                                    .px_2()
                                    .rounded_md()
                                    .bg(theme::CARD)
                                    .child(dialog::message("Blank: left out", false)),
                            )
                        })
                }
                _ => d.child(dialog::subtitle("Preparing preview…")),
            });
        let options = div()
            .flex()
            .flex_col()
            .gap_1()
            .w(px(420.0))
            .child(self.row_el(ROW_MODE, "Output", mode))
            .child(self.row_el(ROW_PAGE, "Page size", page))
            .child(self.row_el(ROW_DPI, "Resolution", dpi))
            .child(self.row_el(ROW_CROP, "Clean up", crop))
            .child(self.row_el(ROW_CURL, "", curl))
            .child(self.row_el(ROW_ORIENT, "", orient))
            .child(self.row_el(ROW_DESKEW, "", deskew))
            .child(self.row_el(ROW_FLATTEN, "", flatten))
            .child(self.row_el(ROW_BLANKS, "", blanks))
            .child(self.row_el(ROW_OCR, "Text", ocr))
            .when(n > 1, |d| {
                d.child(self.row_el(ROW_IMAGE, "Preview", image_row))
            })
            .child(self.row_el(ROW_VIEW, if n > 1 { "" } else { "Preview" }, view_chips));
        let bar = self.running.as_ref().map(|t| {
            let frac = t.snapshot().fraction.unwrap_or(0.0);
            div()
                .h(px(4.0))
                .w_full()
                .bg(theme::BACKGROUND)
                .child(div().h_full().w(relative(frac)).bg(theme::ACCENT))
        });
        dialog::backdrop("scan-backdrop")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|d, _: &DialogNext, _, cx| d.step_row(1, cx)))
            .on_action(cx.listener(|d, _: &DialogPrev, _, cx| d.step_row(-1, cx)))
            .on_action(cx.listener(|d, _: &DialogLeft, _, cx| d.adjust(-1, cx)))
            .on_action(cx.listener(|d, _: &DialogRight, _, cx| d.adjust(1, cx)))
            .on_action(cx.listener(|d, _: &DialogRun, window, cx| d.create(window, cx)))
            .on_action(cx.listener(|d, _: &DialogCancel, _, cx| d.cancel(cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|d, _, _, cx| {
                    if d.running.is_none() {
                        cx.emit(ScanEvent::Dismiss)
                    }
                }),
            )
            .child(
                dialog::card("scan-dialog", px(900.0))
                    .child(
                        dialog::header("Create from Scans or Photos").child(dialog::subtitle(
                            format!(
                                "{n} image{} → one PDF, a page each",
                                if n == 1 { "" } else { "s" }
                            ),
                        )),
                    )
                    .child(div().flex().gap_4().child(options).child(picture))
                    .children(bar)
                    .when_some(self.status.clone(), |d, s| {
                        d.child(dialog::message(s, false))
                    })
                    .when_some(self.error.clone(), |d, e| d.child(dialog::message(e, true)))
                    .child(
                        dialog::footer()
                            .child(dialog::hint(if running {
                                "Esc stops"
                            } else {
                                "Tab moves, ←/→ change, Enter creates"
                            }))
                            .child(
                                Button::new("scan-cancel", if running { "Stop" } else { "Cancel" })
                                    .on_click(cx.listener(|d, _, _, cx| d.cancel(cx))),
                            )
                            .when(!running, |d| {
                                d.child(
                                    Button::new("scan-go", "Create…").primary().on_click(
                                        cx.listener(|d, _, window, cx| d.create(window, cx)),
                                    ),
                                )
                            }),
                    ),
            )
            .text_color(theme::TEXT_SOFT)
    }
}
