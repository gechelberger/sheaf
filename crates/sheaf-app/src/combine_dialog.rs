//! File > Create > Combine Files…: one new PDF from PDFs and images
//! (`sheaf_ops::combine`, as `sheaf combine` does). Files can be added,
//! a folder at a time, or dropped on the dialog; reordered and removed;
//! and limited to some of their pages. Create asks where to save, runs
//! with progress (Escape stops it), and opens the result.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{
    App, Context, Entity, EventEmitter, ExternalPaths, FocusHandle, Focusable, MouseButton,
    Subscription, Window, div, prelude::*, px, relative,
};
use sheaf_ops::combine::{CombineItem, CombineParams};
use sheaf_ops::images::Paper;
use sheaf_ops::{Tracker, was_cancelled};
use sheaf_ui::button::{Button, Chip};
use sheaf_ui::text_input::{TextInput, TextInputEvent};
use sheaf_ui::{dialog, theme};

use crate::{DialogCancel, DialogRun};

pub const KEY_CONTEXT: &str = "CombineDialog";

const PAPERS: [(Paper, &str); 3] = [
    (Paper::Original, "Original size"),
    (Paper::Letter, "Letter"),
    (Paper::A4, "A4"),
];

pub enum CombineEvent {
    /// The PDF is written; `params` are worth remembering.
    Created {
        path: PathBuf,
        params: CombineParams,
    },
    Dismiss,
}

/// What a file holds, once looked at.
#[derive(Clone, PartialEq)]
enum Kind {
    Pending,
    Pdf(usize),
    Image(usize),
    Unreadable(String),
}

struct Row {
    path: PathBuf,
    kind: Kind,
    range: Entity<TextInput>,
    _sub: Subscription,
}

pub struct CombineDialog {
    focus: FocusHandle,
    rows: Vec<Row>,
    params: CombineParams,
    running: Option<Tracker>,
    status: Option<String>,
    error: Option<String>,
}

impl EventEmitter<CombineEvent> for CombineDialog {}

impl Focusable for CombineDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// Files `add` takes from a folder: PDFs and images, by name.
fn folder_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    files.retain(|p| p.is_file() && accepted(p));
    files.sort();
    files
}

fn accepted(p: &Path) -> bool {
    sheaf_image::io::is_image_path(p)
        || p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}

/// Look at a file: a PDF's page count, or how many pages an image makes.
fn inspect(path: &Path) -> Kind {
    if sheaf_ops::combine::is_pdf(path) {
        match sheaf_doc::SourceInfo::read(&sheaf_doc::Store::new(), path, None) {
            Ok(info) => Kind::Pdf(info.src.page_count() as usize),
            Err(e) if crate::docview::needs_password(&e) => {
                Kind::Unreadable("needs a password".into())
            }
            Err(e) => Kind::Unreadable(format!("{e:#}")),
        }
    } else {
        match sheaf_image::io::read(path) {
            Ok(pages) => Kind::Image(pages.len()),
            Err(e) => Kind::Unreadable(format!("{e:#}")),
        }
    }
}

impl CombineDialog {
    pub fn new(
        files: Vec<PathBuf>,
        params: CombineParams,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let mut d = Self {
            focus,
            rows: Vec::new(),
            params,
            running: None,
            status: None,
            error: None,
        };
        d.add(files, cx);
        d
    }

    /// Add files (folders expand to their PDFs and images) at the end.
    fn add(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let mut files = Vec::new();
        for p in paths {
            if p.is_dir() {
                files.extend(folder_files(&p));
            } else if accepted(&p) || sheaf_ops::combine::is_pdf(&p) {
                files.push(p);
            }
        }
        for path in files {
            let range = cx.new(|cx| TextInput::new("All pages", cx));
            let sub = cx.subscribe(&range, |d, _, ev: &TextInputEvent, cx| match ev {
                TextInputEvent::Confirm => d.create_from_key(cx),
                TextInputEvent::Cancel => d.cancel(cx),
            });
            self.rows.push(Row {
                path: path.clone(),
                kind: Kind::Pending,
                range,
                _sub: sub,
            });
            cx.spawn(async move |this, cx| {
                let p = path.clone();
                let kind = cx
                    .background_executor()
                    .spawn(async move { inspect(&p) })
                    .await;
                this.update(cx, |d, cx| {
                    for r in d.rows.iter_mut().filter(|r| r.path == path) {
                        r.kind = kind.clone();
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        self.error = None;
        cx.notify();
    }

    fn pick(&mut self, folders: bool, cx: &mut Context<Self>) {
        let rx = crate::file_dialog::open_filtered(
            cx,
            gpui::PathPromptOptions {
                files: !folders,
                directories: folders,
                multiple: true,
                prompt: Some(if folders { "Add Folder" } else { "Add Files" }.into()),
            },
            crate::file_dialog::Filter::PdfAndImages,
        );
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                this.update(cx, |d, cx| d.add(paths, cx)).ok();
            }
        })
        .detach();
    }

    fn move_row(&mut self, i: usize, by: i64, cx: &mut Context<Self>) {
        let j = i as i64 + by;
        if j >= 0 && (j as usize) < self.rows.len() {
            self.rows.swap(i, j as usize);
            cx.notify();
        }
    }

    fn remove_row(&mut self, i: usize, cx: &mut Context<Self>) {
        if i < self.rows.len() {
            self.rows.remove(i);
            cx.notify();
        }
    }

    /// The items to combine, or what's wrong.
    fn items(&self, cx: &App) -> Result<Vec<CombineItem>, String> {
        if self.rows.is_empty() {
            return Err("Add some files first".into());
        }
        let mut items = Vec::with_capacity(self.rows.len());
        for r in &self.rows {
            let name = r
                .path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let count = match &r.kind {
                Kind::Pending => return Err(format!("Still reading {name}…")),
                Kind::Unreadable(e) => return Err(format!("{name}: {e}")),
                Kind::Pdf(n) | Kind::Image(n) => *n,
            };
            let text = r.range.read(cx).text().trim().to_string();
            if !text.is_empty() {
                sheaf_ops::pages::parse(&text, count).map_err(|e| format!("{name}: {e}"))?;
            }
            items.push(CombineItem {
                path: r.path.clone(),
                pages: (!text.is_empty()).then_some(text),
                password: None,
            });
        }
        Ok(items)
    }

    fn create_from_key(&mut self, cx: &mut Context<Self>) {
        // Enter in a range field: create, if nothing's wrong.
        if let Some(w) = cx.active_window() {
            let this = cx.entity();
            cx.defer(move |cx| {
                w.update(cx, |_, window, cx| {
                    this.update(cx, |d, cx| d.create(window, cx));
                })
                .ok();
            });
        }
    }

    /// Ask where to save, then run.
    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.running.is_some() {
            return;
        }
        let items = match self.items(cx) {
            Ok(items) => items,
            Err(e) => {
                self.error = Some(e);
                cx.notify();
                return;
            }
        };
        let dir = items[0]
            .path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default();
        let rx = crate::file_dialog::save(cx, &dir, Some("Combined.pdf"));
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = rx.await else {
                return;
            };
            handle
                .update(cx, |_, _, cx| {
                    this.update(cx, |d, cx| d.run(items, path, cx)).ok();
                })
                .ok();
        })
        .detach();
    }

    fn run(&mut self, items: Vec<CombineItem>, out: PathBuf, cx: &mut Context<Self>) {
        use futures::FutureExt;
        if items
            .iter()
            .any(|i| out.exists() && i.path.canonicalize().ok() == out.canonicalize().ok())
        {
            self.error = Some("The result can't replace one of the files being combined".into());
            cx.notify();
            return;
        }
        let tracker = Tracker::new();
        self.running = Some(tracker.clone());
        self.error = None;
        self.status = Some("Combining…".into());
        cx.notify();
        let params = self.params.clone();
        let progress = tracker.clone();
        let target = out.clone();
        let n = items.len();
        let work = cx
            .background_executor()
            .spawn(async move { sheaf_ops::combine::combine(&items, &params, &target, &progress) });
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
                        let f = snap.fraction.unwrap_or(0.0);
                        d.status = Some(if f >= 0.7 {
                            "Writing…".into()
                        } else {
                            let i = ((f / 0.7 * n as f32) as usize + 1).min(n);
                            format!("Reading file {i} of {n}…")
                        });
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
                    Ok(_) => cx.emit(CombineEvent::Created {
                        path: out,
                        params: d.params.clone(),
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
                self.status = Some("Stopping…".into());
                cx.notify();
            }
            None => cx.emit(CombineEvent::Dismiss),
        }
    }

    fn row_el(&self, i: usize, r: &Row, cx: &mut Context<Self>) -> impl IntoElement {
        let name = r
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let (what, bad) = match &r.kind {
            Kind::Pending => ("…".to_string(), false),
            Kind::Pdf(n) => (format!("{n} page{}", if *n == 1 { "" } else { "s" }), false),
            Kind::Image(1) => ("image".to_string(), false),
            Kind::Image(n) => (format!("image, {n} pages"), false),
            Kind::Unreadable(e) => (e.clone(), true),
        };
        let many = matches!(r.kind, Kind::Pdf(n) | Kind::Image(n) if n > 1);
        div()
            .id(("combine-row", i))
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_sm()
            .hover(|s| s.bg(theme::HOVER))
            .child(
                div()
                    .w(px(24.0))
                    .text_sm()
                    .text_color(theme::TEXT_FAINT)
                    .child(format!("{}", i + 1)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(theme::TEXT)
                            .child(name),
                    )
                    .child(
                        div()
                            .text_xs()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(if bad { theme::ERROR } else { theme::TEXT_FAINT })
                            .child(what),
                    ),
            )
            .child(div().w(px(150.0)).when(many, |d| d.child(r.range.clone())))
            .child(
                Button::new(("combine-up", i), "↑")
                    .compact()
                    .on_click(cx.listener(move |d, _, _, cx| d.move_row(i, -1, cx))),
            )
            .child(
                Button::new(("combine-down", i), "↓")
                    .compact()
                    .on_click(cx.listener(move |d, _, _, cx| d.move_row(i, 1, cx))),
            )
            .child(
                Button::new(("combine-remove", i), "✕")
                    .compact()
                    .on_click(cx.listener(move |d, _, _, cx| d.remove_row(i, cx))),
            )
    }
}

impl Render for CombineDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let running = self.running.is_some();
        let rows: Vec<_> = (0..self.rows.len())
            .map(|i| {
                let r = &self.rows[i];
                self.row_el(i, r, cx).into_any_element()
            })
            .collect();
        let total: usize = self
            .rows
            .iter()
            .map(|r| match r.kind {
                Kind::Pdf(n) | Kind::Image(n) => n,
                _ => 0,
            })
            .sum();
        let list = div()
            .id("combine-list")
            .h(px(320.0))
            .overflow_y_scroll()
            .bg(theme::BACKGROUND)
            .rounded_md()
            .p_1()
            .when(self.rows.is_empty(), |d| {
                d.flex()
                    .items_center()
                    .justify_center()
                    .child(dialog::subtitle("Add files or a folder, or drop them here"))
            })
            .children(rows);
        let bookmarks = Chip::new(
            ("combine-bm", 0usize),
            format!(
                "{} A bookmark for each file",
                if self.params.bookmark_each_file {
                    "☑"
                } else {
                    "☐"
                }
            ),
        )
        .on_click(cx.listener(|d, _, _, cx| {
            d.params.bookmark_each_file = !d.params.bookmark_each_file;
            cx.notify();
        }));
        let paper =
            div()
                .flex()
                .gap_1()
                .children(PAPERS.iter().enumerate().map(|(i, (p, label))| {
                    let p = *p;
                    Chip::new(("combine-paper", i), *label)
                        .selected(self.params.image_page == p)
                        .on_click(cx.listener(move |d, _, _, cx| {
                            d.params.image_page = p;
                            cx.notify();
                        }))
                }));
        let bar = self.running.as_ref().map(|t| {
            let frac = t.snapshot().fraction.unwrap_or(0.0);
            div()
                .h(px(4.0))
                .w_full()
                .bg(theme::BACKGROUND)
                .child(div().h_full().w(relative(frac)).bg(theme::ACCENT))
        });
        dialog::backdrop("combine-backdrop")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|d, _: &DialogRun, window, cx| d.create(window, cx)))
            .on_action(cx.listener(|d, _: &DialogCancel, _, cx| d.cancel(cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|d, _, _, cx| {
                    if d.running.is_none() {
                        cx.emit(CombineEvent::Dismiss)
                    }
                }),
            )
            .on_drop(cx.listener(|d, paths: &ExternalPaths, _, cx| {
                if d.running.is_none() {
                    d.add(paths.paths().to_vec(), cx);
                }
            }))
            .child(
                dialog::card("combine-dialog", px(720.0))
                    .child(
                        dialog::header("Combine Files").child(dialog::subtitle(format!(
                            "{} file{}, {total} page{} → one PDF",
                            self.rows.len(),
                            if self.rows.len() == 1 { "" } else { "s" },
                            if total == 1 { "" } else { "s" }
                        ))),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("combine-add", "Add Files…")
                                    .on_click(cx.listener(|d, _, _, cx| d.pick(false, cx))),
                            )
                            .child(
                                Button::new("combine-add-folder", "Add Folder…")
                                    .on_click(cx.listener(|d, _, _, cx| d.pick(true, cx))),
                            )
                            .child(div().flex_1())
                            .child(dialog::subtitle("Pages: e.g. 1-3, 7, 10-")),
                    )
                    .child(list)
                    .child(
                        dialog::field_row(
                            "Bookmarks".to_string(),
                            px(100.0),
                            false,
                            bookmarks.into_any_element(),
                        )
                        .into_any_element(),
                    )
                    .child(
                        dialog::field_row(
                            "Images".to_string(),
                            px(100.0),
                            false,
                            paper.into_any_element(),
                        )
                        .into_any_element(),
                    )
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
                                "Enter creates, Esc cancels"
                            }))
                            .child(
                                Button::new(
                                    "combine-cancel",
                                    if running { "Stop" } else { "Cancel" },
                                )
                                .on_click(cx.listener(|d, _, _, cx| d.cancel(cx))),
                            )
                            .when(!running, |d| {
                                d.child(
                                    Button::new("combine-go", "Create…").primary().on_click(
                                        cx.listener(|d, _, window, cx| d.create(window, cx)),
                                    ),
                                )
                            }),
                    ),
            )
            .text_color(theme::TEXT_SOFT)
    }
}
