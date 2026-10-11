//! The Patterns panel: regex search as its own tool, separate from the
//! literal Find bar. Presets cover the things a workpaper preparer scans a
//! binder for (SSNs, EINs, amounts, dates); results are listed by page and
//! highlighted in a different color than Find's.

use std::sync::Arc;
use std::time::Instant;

use futures::StreamExt;
use gpui::{
    AnyElement, Context, Entity, Focusable, Subscription, Task, Window, div, prelude::*, px, rgb,
};
use sheaf_render::{SearchEvent, SearchHit};

use crate::docview::{DocView, ScrollTo, State};
use crate::text_input::{TextInput, TextInputEvent};

const ROW_H: f32 = 22.0;
const PAD: f32 = 8.0;

/// Named presets. Patterns are deliberately permissive: a scan should
/// over-match slightly rather than miss a 9-digit SSN typed with spaces.
pub const PRESETS: &[(&str, &str)] = &[
    ("SSN", r"\b\d{3}[- ]?\d{2}[- ]?\d{4}\b"),
    ("EIN", r"\b\d{2}-\d{7}\b"),
    ("Phone", r"\(?\b\d{3}\)?[-. ]?\d{3}[-. ]\d{4}\b"),
    (
        "Amount",
        r"\$\s?\d{1,3}(?:,\d{3})*(?:\.\d{2})?|\b\d{1,3}(?:,\d{3})+\.\d{2}\b",
    ),
    ("Date", r"\b\d{1,2}/\d{1,2}/\d{2,4}\b"),
];

pub struct PatternSearch {
    pub input: Entity<TextInput>,
    pub _events: Subscription,
    /// Source of the pattern the current results are for.
    pub source: String,
    pub error: Option<String>,
    /// Hits in primary-source page order.
    pub hits: Vec<SearchHit>,
    pub current: Option<usize>,
    pub scanned: u32,
    pub done: bool,
    pub started: Option<Instant>,
    pub _task: Option<Task<()>>,
    pub scroll: gpui::ScrollHandle,
}

enum Row {
    Header { page: u32, count: usize },
    Hit(usize),
}

impl DocView {
    fn ensure_pattern(&mut self, cx: &mut Context<Self>) {
        if self.pattern.is_some() {
            return;
        }
        let input = cx.new(|cx| TextInput::new("Regular expression", cx));
        let events = cx.subscribe(&input, |this, input, ev: &TextInputEvent, cx| match ev {
            TextInputEvent::Confirm => {
                let src = input.read(cx).text().to_string();
                this.start_pattern_search(src, cx);
            }
            TextInputEvent::Cancel => this.pending_focus = true,
        });
        self.pattern = Some(PatternSearch {
            input,
            _events: events,
            source: String::new(),
            error: None,
            hits: Vec::new(),
            current: None,
            scanned: 0,
            done: true,
            started: None,
            _task: None,
            scroll: gpui::ScrollHandle::new(),
        });
    }

    /// Ctrl+Shift+F: make sure the panel exists and put the cursor in it.
    pub fn focus_pattern_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_pattern(cx);
        let input = self.pattern.as_ref().unwrap().input.clone();
        input.update(cx, |i, cx| i.select_all(cx));
        window.focus(&input.read(cx).focus_handle(cx), cx);
        cx.notify();
    }

    /// `--pattern`: run a pattern once the document is ready.
    pub fn set_initial_pattern(&mut self, src: String) {
        self.initial_pattern = Some(src);
    }

    pub(crate) fn run_initial_pattern(&mut self, src: String, cx: &mut Context<Self>) {
        self.ensure_pattern(cx);
        if let Some(p) = &self.pattern {
            let input = p.input.clone();
            input.update(cx, |i, cx| i.set_text(src.clone(), cx));
        }
        self.start_pattern_search(src, cx);
    }

    pub(crate) fn start_pattern_search(&mut self, src: String, cx: &mut Context<Self>) {
        self.ensure_pattern(cx);
        let State::Ready(doc) = &self.state else {
            return;
        };
        let handle = doc.primary().clone();
        let Some(ps) = &mut self.pattern else { return };
        ps.source = src.clone();
        ps.hits.clear();
        ps.current = None;
        ps.scanned = 0;
        ps.error = None;
        ps._task = None;
        if src.trim().is_empty() {
            ps.done = true;
            cx.notify();
            return;
        }
        let re = match regex::Regex::new(&src) {
            Ok(re) => Arc::new(re),
            Err(e) => {
                ps.error = Some(e.to_string());
                ps.done = true;
                cx.notify();
                return;
            }
        };
        ps.done = false;
        ps.started = Some(Instant::now());
        let mut rx = handle.pattern_search(re);
        let timing = self.timing;
        let Some(ps) = &mut self.pattern else { return };
        ps._task =
            Some(cx.spawn(async move |this, cx| {
                while let Some(ev) = rx.next().await {
                    let stop = this
                    .update(cx, |view, cx| {
                        let Some(ps) = &mut view.pattern else { return true };
                        match ev {
                            SearchEvent::Hits(mut hits) => {
                                let first = ps.hits.is_empty();
                                ps.hits.append(&mut hits);
                                if first {
                                    view.go_to_pattern_hit(0, cx);
                                }
                            }
                            SearchEvent::Progress(n) => ps.scanned = n,
                            SearchEvent::Done => {
                                ps.done = true;
                                if timing && let Some(t) = ps.started.take() {
                                    eprintln!(
                                        "[timing] pattern {:?}: {} hits across {} pages in {:?}",
                                        ps.source,
                                        ps.hits.len(),
                                        ps.scanned,
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

    pub(crate) fn go_to_pattern_hit(&mut self, ix: usize, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let Some(ps) = &mut self.pattern else { return };
        if ps.hits.is_empty() {
            return;
        }
        let ix = ix % ps.hits.len();
        ps.current = Some(ix);
        let hit = &ps.hits[ix];
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
            .unwrap_or(sheaf_core::PageSize {
                width: 612.0,
                height: 792.0,
            });
        if let Some(r) = hit.rects.first() {
            let r = crate::docview::rotate_rect(*r, src, pref.rotation);
            let ps2 = src.rotated(pref.rotation);
            doc.pending_scroll = Some(ScrollTo::Reveal {
                page: index,
                fx: (r.x + r.w / 2.0) / ps2.width.max(1.0),
                fy: (r.y + r.h / 2.0) / ps2.height.max(1.0),
            });
        }
        cx.notify();
    }

    /// The Patterns sidebar panel.
    /// `wide`: the panel sits in a bottom dock, so the controls go in a
    /// column on the left with the results beside them.
    pub fn render_patterns(
        &mut self,
        wide: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.ensure_pattern(cx);
        let ps = self.pattern.as_ref().unwrap();
        let input = ps.input.clone();

        let chips: Vec<_> = PRESETS
            .iter()
            .map(|(name, re)| {
                let re = re.to_string();
                let input = input.clone();
                div()
                    .id(*name)
                    .px_2()
                    .py_0p5()
                    .rounded_sm()
                    .text_xs()
                    .cursor_pointer()
                    .bg(rgb(0x3a3a3a))
                    .hover(|s| s.bg(rgb(0x4a4a4a)))
                    .on_click(cx.listener(move |v, _, _, cx| {
                        input.update(cx, |i, cx| i.set_text(re.clone(), cx));
                        v.start_pattern_search(re.clone(), cx);
                    }))
                    .child(*name)
            })
            .collect();

        let status = if let Some(e) = &ps.error {
            format!("Invalid pattern: {e}")
        } else if ps.source.trim().is_empty() {
            "Enter a pattern and press Enter, or pick a preset".to_string()
        } else {
            let pages: std::collections::HashSet<u32> = ps.hits.iter().map(|h| h.page).collect();
            let n = ps.hits.len();
            if ps.done {
                format!(
                    "{n} match{} on {} page{}",
                    if n == 1 { "" } else { "es" },
                    pages.len(),
                    if pages.len() == 1 { "" } else { "s" }
                )
            } else {
                format!("{n} so far — scanning page {}…", ps.scanned)
            }
        };

        // Rows: a header per page, then its hits.
        let mut rows: Vec<Row> = Vec::new();
        let mut i = 0;
        while i < ps.hits.len() {
            let page = ps.hits[i].page;
            let start = i;
            while i < ps.hits.len() && ps.hits[i].page == page {
                i += 1;
            }
            rows.push(Row::Header {
                page,
                count: i - start,
            });
            for k in start..i {
                rows.push(Row::Hit(k));
            }
        }
        let total_h = rows.len() as f32 * ROW_H + PAD;
        let bounds = ps.scroll.bounds();
        let view_h = f32::from(bounds.size.height).max(1.0);
        let scroll_y = (-f32::from(ps.scroll.offset().y)).max(0.0);
        let r0 = ((scroll_y / ROW_H) as usize).min(rows.len());
        let r1 = (((scroll_y + view_h) / ROW_H) as usize + 1).min(rows.len());
        let current = ps.current;
        let hits = &ps.hits;
        let mut els = Vec::new();
        for (r, row) in rows.iter().enumerate().take(r1).skip(r0) {
            let base = div()
                .absolute()
                .top(px(r as f32 * ROW_H))
                .left(px(0.0))
                .w_full()
                .h(px(ROW_H))
                .flex()
                .items_center();
            els.push(match row {
                Row::Header { page, count } => base
                    .px_2()
                    .text_xs()
                    .text_color(rgb(0x9a9a9a))
                    .bg(rgb(0x202020))
                    .child(format!("Page {}  ·  {count}", page + 1))
                    .into_any_element(),
                Row::Hit(ix) => {
                    let ix = *ix;
                    base.id(("pattern-hit", ix))
                        .pl_4()
                        .pr_2()
                        .text_sm()
                        .cursor_pointer()
                        .when(current == Some(ix), |d| d.bg(rgb(0x4a2f6f)))
                        .hover(|s| s.bg(rgb(0x3a3a3a)))
                        .on_click(cx.listener(move |v, _, _, cx| v.go_to_pattern_hit(ix, cx)))
                        .child(
                            div()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_color(rgb(0xd0d0d0))
                                .child(hits[ix].text.clone()),
                        )
                        .into_any_element()
                }
            });
        }

        let controls = div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .child(input)
            .child(div().flex().flex_wrap().gap_1().children(chips))
            .child(
                div()
                    .text_xs()
                    .text_color(if ps.error.is_some() {
                        rgb(0xe08080)
                    } else {
                        rgb(0x9a9a9a)
                    })
                    .child(status),
            );
        let results = div()
            .id("pattern-results")
            .track_scroll(&ps.scroll)
            .overflow_y_scroll()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .child(div().relative().w_full().h(px(total_h)).children(els));
        if wide {
            div()
                .size_full()
                .flex()
                .child(
                    controls
                        .w(px(340.0))
                        .border_r_1()
                        .border_color(rgb(0x111111)),
                )
                .child(results)
                .into_any_element()
        } else {
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(controls.border_b_1().border_color(rgb(0x111111)))
                .child(results)
                .into_any_element()
        }
    }
}
