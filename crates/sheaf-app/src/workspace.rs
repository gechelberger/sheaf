//! Top-level window content: tab strip, optional sidebar, and the active
//! document view.

use std::path::PathBuf;
use std::time::Instant;

use gpui::{
    AnyElement, App, Context, Entity, Focusable, PathPromptOptions, SharedString, Window, div,
    prelude::*, px, rgb,
};

use crate::docview::DocView;
use crate::{
    CloseTab, NextTab, OpenFile, PrevTab, Quit, ShowOutline, ShowPatterns, ShowThumbnails,
    ToggleSidebar,
};

pub const KEY_CONTEXT: &str = "Workspace";

/// What the command line asked for at startup.
pub struct Launch {
    pub paths: Vec<PathBuf>,
    pub timing: bool,
    /// Open the search bar with this query once the first document loads.
    pub find: Option<String>,
    /// Dev: rotate page 1 of the first document and save it here.
    pub selftest_save: Option<PathBuf>,
    /// Dev: insert this PDF after page 1 of the first document.
    pub selftest_insert: Option<PathBuf>,
    pub start_panel: Option<String>,
    /// Run this regex in the Patterns panel of the first document.
    pub pattern: Option<String>,
    pub launched: Instant,
}

const TAB_H: f32 = 32.0;
const SIDEBAR_W: f32 = 220.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Thumbnails,
    Outline,
    Patterns,
}

pub struct Workspace {
    docs: Vec<Entity<DocView>>,
    active: usize,
    sidebar_open: bool,
    panel: Panel,
    timing: bool,
    launched: Instant,
}

impl Workspace {
    pub fn new(launch: Launch, cx: &mut Context<Self>) -> Self {
        let Launch {
            paths,
            timing,
            find,
            selftest_save,
            selftest_insert,
            start_panel,
            pattern,
            launched,
        } = launch;
        let mut this = Self {
            docs: Vec::new(),
            active: 0,
            sidebar_open: true,
            panel: match start_panel.as_deref() {
                Some("bookmarks") => Panel::Outline,
                Some("patterns") => Panel::Patterns,
                _ => Panel::Thumbnails,
            },
            timing,
            launched,
        };
        for p in paths {
            this.open_path(p, cx);
        }
        if let (Some(q), Some(doc)) = (find, this.docs.first().cloned()) {
            doc.update(cx, |d, cx| d.set_initial_query(q, cx));
        }
        if let (Some(p), Some(doc)) = (selftest_save, this.docs.first().cloned()) {
            doc.update(cx, |d, _| d.set_selftest_save(p));
        }
        if let (Some(p), Some(doc)) = (selftest_insert, this.docs.first().cloned()) {
            doc.update(cx, |d, _| d.set_selftest_insert(p));
        }
        if let (Some(p), Some(doc)) = (pattern, this.docs.first().cloned()) {
            doc.update(cx, |d, _| d.set_initial_pattern(p));
        }
        this
    }

    pub fn active_doc(&self) -> Option<&Entity<DocView>> {
        self.docs.get(self.active)
    }

    pub fn open_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let (timing, launched) = (self.timing, self.launched);
        let doc = cx.new(|cx| DocView::new(path, timing, launched, cx));
        self.docs.push(doc);
        self.active = self.docs.len() - 1;
        cx.notify();
    }

    fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.docs.is_empty() {
            return;
        }
        self.active = ix.min(self.docs.len() - 1);
        self.focus_active(window, cx);
        cx.notify();
    }

    pub fn focus_active(&self, window: &mut Window, cx: &mut App) {
        if let Some(doc) = self.active_doc() {
            let handle = doc.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
    }

    fn close(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.docs.len() {
            return;
        }
        self.docs.remove(ix);
        if !self.docs.is_empty() {
            self.activate(self.active.min(self.docs.len() - 1), window, cx);
        }
        cx.notify();
    }

    fn prompt_open(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Open".into()),
        });
        cx.spawn(async move |this, cx| match rx.await {
            Ok(Ok(Some(paths))) => {
                this.update(cx, |ws, cx| {
                    for p in paths {
                        ws.open_path(p, cx);
                    }
                })
                .ok();
            }
            Ok(Ok(None)) => {}
            Ok(Err(e)) => log::error!("open dialog: {e:#}"),
            Err(_) => {}
        })
        .detach();
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let tabs = self.docs.iter().enumerate().map(|(i, doc)| {
            let active = i == self.active;
            let title = doc.read(cx).title();
            div()
                .id(("tab", i))
                .h_full()
                .px_3()
                .flex()
                .items_center()
                .gap_2()
                .text_sm()
                .cursor_pointer()
                .bg(if active { rgb(0x2b2b2b) } else { rgb(0x1e1e1e) })
                .text_color(if active { rgb(0xffffff) } else { rgb(0x9a9a9a) })
                .hover(|s| s.bg(rgb(0x333333)))
                .on_click(cx.listener(move |ws, _, window, cx| ws.activate(i, window, cx)))
                .child(title)
                .child(
                    div()
                        .id(("tab-close", i))
                        .px_1()
                        .rounded_sm()
                        .text_color(rgb(0x8a8a8a))
                        .hover(|s| s.bg(rgb(0x555555)).text_color(rgb(0xffffff)))
                        .on_click(cx.listener(move |ws, _, window, cx| {
                            ws.close(i, window, cx);
                            cx.stop_propagation();
                        }))
                        .child("×"),
                )
        });
        div()
            .h(px(TAB_H))
            .flex_shrink_0()
            .flex()
            .items_stretch()
            .bg(rgb(0x1e1e1e))
            .border_b_1()
            .border_color(rgb(0x111111))
            .children(tabs)
            .into_any_element()
    }

    fn render_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let panel = self.panel;
        let header_btn = |label: &'static str, which: Panel, cx: &mut Context<Self>| {
            div()
                .id(label)
                .px_2()
                .py_1()
                .text_xs()
                .cursor_pointer()
                .rounded_sm()
                .bg(if panel == which {
                    rgb(0x3a3a3a)
                } else {
                    rgb(0x252525)
                })
                .text_color(if panel == which {
                    rgb(0xffffff)
                } else {
                    rgb(0x9a9a9a)
                })
                .hover(|s| s.bg(rgb(0x444444)))
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.panel = which;
                    cx.notify();
                }))
                .child(label)
        };
        let body: AnyElement = match self.active_doc().cloned() {
            Some(doc) => doc.update(cx, |d, cx| match panel {
                Panel::Thumbnails => d.render_thumbnails(SIDEBAR_W, window, cx),
                Panel::Outline => d.render_outline(window, cx),
                Panel::Patterns => d.render_patterns(window, cx),
            }),
            None => div().into_any_element(),
        };
        div()
            .w(px(SIDEBAR_W))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(rgb(0x252525))
            .border_r_1()
            .border_color(rgb(0x111111))
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .gap_1()
                    .p_1()
                    .child(header_btn("Pages", Panel::Thumbnails, cx))
                    .child(header_btn("Bookmarks", Panel::Outline, cx))
                    .child(header_btn("Patterns", Panel::Patterns, cx)),
            )
            .child(div().flex_1().min_h(px(0.0)).child(body))
            .into_any_element()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let main: AnyElement = match self.active_doc().cloned() {
            Some(doc) => doc.into_any_element(),
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(0x8a8a8a))
                .child(SharedString::from(
                    "No document open — Ctrl+O to open a PDF",
                ))
                .into_any_element(),
        };
        let sidebar = if self.sidebar_open && !self.docs.is_empty() {
            Some(self.render_sidebar(window, cx))
        } else {
            None
        };

        div()
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(|ws, _: &ToggleSidebar, _, cx| {
                ws.sidebar_open = !ws.sidebar_open;
                cx.notify();
            }))
            .on_action(cx.listener(|ws, _: &ShowThumbnails, _, cx| {
                ws.sidebar_open = true;
                ws.panel = Panel::Thumbnails;
                cx.notify();
            }))
            .on_action(cx.listener(|ws, _: &ShowPatterns, window, cx| {
                ws.sidebar_open = true;
                ws.panel = Panel::Patterns;
                if let Some(doc) = ws.active_doc().cloned() {
                    doc.update(cx, |d, cx| d.focus_pattern_input(window, cx));
                }
                cx.notify();
            }))
            .on_action(cx.listener(|ws, _: &ShowOutline, _, cx| {
                ws.sidebar_open = true;
                ws.panel = Panel::Outline;
                cx.notify();
            }))
            .on_action(cx.listener(|ws, _: &NextTab, window, cx| {
                let n = ws.docs.len();
                if n > 0 {
                    ws.activate((ws.active + 1) % n, window, cx);
                }
            }))
            .on_action(cx.listener(|ws, _: &PrevTab, window, cx| {
                let n = ws.docs.len();
                if n > 0 {
                    ws.activate((ws.active + n - 1) % n, window, cx);
                }
            }))
            .on_action(cx.listener(|ws, _: &CloseTab, window, cx| ws.close(ws.active, window, cx)))
            .on_action(cx.listener(|ws, _: &OpenFile, _, cx| ws.prompt_open(cx)))
            .on_action(|_: &Quit, _, cx| cx.quit())
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x2b2b2b))
            .text_color(rgb(0xd0d0d0))
            .child(self.render_tabs(cx))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .children(sidebar)
                    .child(div().flex_1().min_w(px(0.0)).child(main)),
            )
    }
}
