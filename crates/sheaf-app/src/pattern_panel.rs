//! The Patterns panel: regex search as its own tool, separate from the
//! literal Find bar. Presets cover the things a workpaper preparer scans a
//! binder for (SSNs, EINs, amounts, dates) and are editable (see
//! `patterns.rs`); results are listed by page and highlighted in a
//! different color than Find's.

use std::sync::Arc;
use std::time::Instant;

use futures::StreamExt;
use gpui::{
    AnchoredPositionMode, AnyElement, Context, Entity, Focusable, MouseButton, Pixels, Point,
    Subscription, Task, Window, anchored, deferred, div, prelude::*, px,
};
use sheaf_render::SearchEvent;
use sheaf_ui::list::ListLayout;
use sheaf_ui::text_input::{TextInput, TextInputEvent};
use sheaf_ui::theme;

use crate::docview::{DocView, Hit, ScrollTo, State};
use crate::patterns::{self, Preset};

const ROW_H: f32 = 22.0;
const PAD: f32 = 8.0;

/// Saving or editing a preset: a name field; the regex comes from the
/// pattern field when Save is pressed.
pub struct PresetEdit {
    /// Which preset is being edited, or None for a new one.
    pub index: Option<usize>,
    pub name: Entity<TextInput>,
    pub _sub: Subscription,
}

pub struct PatternSearch {
    pub presets: Vec<Preset>,
    /// Right-click menu on a preset chip: (index, where).
    pub preset_menu: Option<(usize, Point<Pixels>)>,
    pub editing: Option<PresetEdit>,
    pub preset_error: Option<String>,
    pub input: Entity<TextInput>,
    pub _events: Subscription,
    /// Source of the pattern the current results are for.
    pub source: String,
    pub error: Option<String>,
    /// Hits in document order.
    pub hits: Vec<Hit>,
    pub current: Option<usize>,
    pub scanned: u32,
    pub done: bool,
    pub started: Option<Instant>,
    pub _task: Option<Task<()>>,
    pub scroll: gpui::ScrollHandle,
}

enum Row {
    /// `page` is a display index.
    Header {
        page: usize,
        count: usize,
    },
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
            presets: patterns::load(),
            preset_menu: None,
            editing: None,
            preset_error: None,
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
        // Unsaved OCR layers are invisible to PDFium; search them here.
        let seeded = doc.layer_hits(|l| sheaf_doc::text_layer::pattern_search(l, &re));
        let has_seed = !seeded.is_empty();
        ps.hits = seeded;
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
                            SearchEvent::Hits(hits) => {
                                let first = ps.hits.is_empty();
                                if let State::Ready(doc) = &view.state {
                                    doc.merge_source_hits(&mut ps.hits, hits);
                                }
                                if first && !ps.hits.is_empty() {
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
        if has_seed {
            self.go_to_pattern_hit(0, cx);
        }
        cx.notify();
    }

    // ----- presets -------------------------------------------------------------

    /// Start saving the current pattern as a new preset, or editing preset
    /// `index` (its regex goes into the pattern field).
    fn edit_preset(&mut self, index: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_pattern(cx);
        let ps = self.pattern.as_mut().unwrap();
        ps.preset_menu = None;
        ps.preset_error = None;
        let existing = index.and_then(|i| ps.presets.get(i).cloned());
        if let Some(p) = &existing {
            let input = ps.input.clone();
            input.update(cx, |i, cx| i.set_text(p.regex.clone(), cx));
        }
        let name = cx.new(|cx| {
            let mut i = TextInput::new("Preset name", cx);
            if let Some(p) = &existing {
                i.set_text(p.name.clone(), cx);
            }
            i
        });
        let sub = cx.subscribe(&name, |this, _, ev: &TextInputEvent, cx| match ev {
            TextInputEvent::Confirm => this.save_preset(cx),
            TextInputEvent::Cancel => {
                if let Some(ps) = &mut this.pattern {
                    ps.editing = None;
                }
                this.pending_focus = true;
                cx.notify();
            }
        });
        window.focus(&name.read(cx).focus_handle(cx), cx);
        let ps = self.pattern.as_mut().unwrap();
        ps.editing = Some(PresetEdit {
            index,
            name,
            _sub: sub,
        });
        cx.notify();
    }

    /// Commit the preset being edited: name from its field, regex from the
    /// pattern field, both validated.
    fn save_preset(&mut self, cx: &mut Context<Self>) {
        let Some(ps) = &mut self.pattern else { return };
        let Some(edit) = &ps.editing else { return };
        let name = edit.name.read(cx).text().trim().to_string();
        let regex = ps.input.read(cx).text().trim().to_string();
        if name.is_empty() {
            ps.preset_error = Some("Give the preset a name".into());
            cx.notify();
            return;
        }
        if let Err(e) = regex::Regex::new(&regex) {
            ps.preset_error = Some(format!("Invalid pattern: {e}"));
            cx.notify();
            return;
        }
        let preset = Preset { name, regex };
        match edit.index {
            Some(i) if i < ps.presets.len() => ps.presets[i] = preset,
            _ => ps.presets.push(preset),
        }
        ps.editing = None;
        ps.preset_error = patterns::save(&ps.presets)
            .err()
            .map(|e| format!("Couldn't save presets: {e:#}"));
        self.pending_focus = true;
        cx.notify();
    }

    fn delete_preset(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(ps) = &mut self.pattern else { return };
        ps.preset_menu = None;
        if index < ps.presets.len() {
            ps.presets.remove(index);
            ps.preset_error = patterns::save(&ps.presets)
                .err()
                .map(|e| format!("Couldn't save presets: {e:#}"));
        }
        cx.notify();
    }

    /// Put back any built-in preset that's missing (by name), keeping the
    /// user's own and their edits to the rest.
    fn restore_default_presets(&mut self, cx: &mut Context<Self>) {
        let Some(ps) = &mut self.pattern else { return };
        ps.preset_menu = None;
        for d in patterns::defaults() {
            if !ps.presets.iter().any(|p| p.name == d.name) {
                ps.presets.push(d);
            }
        }
        ps.preset_error = patterns::save(&ps.presets)
            .err()
            .map(|e| format!("Couldn't save presets: {e:#}"));
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

        let mut chips: Vec<AnyElement> = ps
            .presets
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let re = p.regex.clone();
                let input = input.clone();
                div()
                    .id(("preset", i))
                    .px_2()
                    .py_0p5()
                    .rounded_sm()
                    .text_xs()
                    .cursor_pointer()
                    .bg(theme::CONTROL)
                    .hover(|s| s.bg(theme::CONTROL_HOVER))
                    .on_click(cx.listener(move |v, _, _, cx| {
                        input.update(cx, |i, cx| i.set_text(re.clone(), cx));
                        v.start_pattern_search(re.clone(), cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |v, ev: &gpui::MouseDownEvent, _, cx| {
                            if let Some(ps) = &mut v.pattern {
                                ps.preset_menu = Some((i, ev.position));
                                cx.notify();
                            }
                        }),
                    )
                    .child(p.name.clone())
                    .into_any_element()
            })
            .collect();
        // Plugin presets: usable, but edited in the plugin, not here.
        let plugin_presets = cx
            .global::<crate::contrib::Contributions>()
            .plugin_presets()
            .to_vec();
        chips.extend(plugin_presets.into_iter().enumerate().map(|(i, p)| {
            let re = p.preset.regex.clone();
            let input = input.clone();
            div()
                .id(("plugin-preset", i))
                .px_2()
                .py_0p5()
                .rounded_sm()
                .text_xs()
                .cursor_pointer()
                .border_1()
                .border_color(theme::BORDER_CARD)
                .hover(|s| s.bg(theme::CONTROL_HOVER))
                .tooltip(sheaf_ui::tooltip::Tooltip::text(format!(
                    "From the {} plugin",
                    p.plugin
                )))
                .on_click(cx.listener(move |v, _, _, cx| {
                    input.update(cx, |i, cx| i.set_text(re.clone(), cx));
                    v.start_pattern_search(re.clone(), cx);
                }))
                .child(p.preset.name.clone())
                .into_any_element()
        }));
        chips.push(
            div()
                .id("preset-save")
                .px_2()
                .py_0p5()
                .rounded_sm()
                .text_xs()
                .cursor_pointer()
                .text_color(theme::TEXT_MUTED)
                .hover(|s| s.bg(theme::CONTROL_HOVER).text_color(theme::TEXT))
                .on_click(cx.listener(|v, _, window, cx| v.edit_preset(None, window, cx)))
                .child("+ Save…")
                .into_any_element(),
        );
        let edit_row = ps.editing.as_ref().map(|e| {
            let title = match e.index {
                Some(i) => format!("Edit preset {}", i + 1),
                None => "Save the pattern above as".to_string(),
            };
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_xs().text_color(theme::TEXT_MUTED).child(title))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(div().flex_1().child(e.name.clone()))
                        .child(
                            div()
                                .id("preset-ok")
                                .px_2()
                                .py_0p5()
                                .rounded_sm()
                                .text_xs()
                                .cursor_pointer()
                                .bg(theme::ACCENT)
                                .text_color(theme::TEXT_BRIGHT)
                                .on_click(cx.listener(|v, _, _, cx| v.save_preset(cx)))
                                .child("Save"),
                        )
                        .child(
                            div()
                                .id("preset-cancel")
                                .px_2()
                                .py_0p5()
                                .rounded_sm()
                                .text_xs()
                                .cursor_pointer()
                                .bg(theme::CONTROL)
                                .on_click(cx.listener(|v, _, _, cx| {
                                    if let Some(ps) = &mut v.pattern {
                                        ps.editing = None;
                                    }
                                    cx.notify();
                                }))
                                .child("Cancel"),
                        ),
                )
        });
        let preset_error = ps.preset_error.clone();
        let menu = ps.preset_menu.map(|(i, pos)| preset_menu(i, pos, cx));

        let ps = self.pattern.as_ref().unwrap();
        let status = if let Some(e) = preset_error {
            e
        } else if let Some(e) = &ps.error {
            format!("Invalid pattern: {e}")
        } else if ps.source.trim().is_empty() {
            "Enter a pattern and press Enter, or pick a preset".to_string()
        } else {
            let pages: std::collections::HashSet<_> = ps.hits.iter().map(|h| h.page).collect();
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
                page: ps.hits[start].order,
                count: i - start,
            });
            for k in start..i {
                rows.push(Row::Hit(k));
            }
        }
        let layout = ListLayout::fixed(rows.len(), ROW_H).padded(0.0, PAD);
        let current = ps.current;
        let hits = &ps.hits;
        let mut els = Vec::new();
        for r in layout.visible(&ps.scroll) {
            let base = layout.place(r).flex().items_center();
            els.push(match &rows[r] {
                Row::Header { page, count } => base
                    .px_2()
                    .text_xs()
                    .text_color(theme::TEXT_MUTED)
                    .bg(theme::LIST_HEADER)
                    .child(format!("Page {}  ·  {count}", page + 1))
                    .into_any_element(),
                Row::Hit(ix) => {
                    let ix = *ix;
                    base.id(("pattern-hit", ix))
                        .pl_4()
                        .pr_2()
                        .text_sm()
                        .cursor_pointer()
                        .when(current == Some(ix), |d| d.bg(theme::PATTERN_ROW_CURRENT))
                        .hover(|s| s.bg(theme::HOVER))
                        .on_click(cx.listener(move |v, _, _, cx| v.go_to_pattern_hit(ix, cx)))
                        .child(
                            div()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_color(theme::TEXT_SOFT)
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
            .children(edit_row)
            .child(
                div()
                    .text_xs()
                    .text_color(if ps.error.is_some() {
                        theme::ERROR
                    } else {
                        theme::TEXT_MUTED
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
            .child(layout.body().children(els));
        if wide {
            div()
                .size_full()
                .flex()
                .child(
                    controls
                        .w(px(340.0))
                        .border_r_1()
                        .border_color(theme::DIVIDER),
                )
                .child(results)
                .children(menu)
                .into_any_element()
        } else {
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(controls.border_b_1().border_color(theme::DIVIDER))
                .child(results)
                .children(menu)
                .into_any_element()
        }
    }
}

/// Right-click menu on a preset chip.
fn preset_menu(index: usize, pos: Point<Pixels>, cx: &mut Context<DocView>) -> AnyElement {
    let item = |id: &'static str,
                label: &'static str,
                cx: &mut Context<DocView>,
                f: fn(&mut DocView, usize, &mut Window, &mut Context<DocView>)| {
        div()
            .id(id)
            .px_3()
            .py_1()
            .text_sm()
            .cursor_pointer()
            .hover(|s| s.bg(theme::HOVER))
            .on_click(cx.listener(move |v, _, window, cx| f(v, index, window, cx)))
            .child(label)
    };
    deferred(
        anchored()
            .position(pos)
            .position_mode(AnchoredPositionMode::Window)
            .child(
                div()
                    .id("preset-menu")
                    .occlude()
                    .on_mouse_down_out(cx.listener(|v, _, _, cx| {
                        if let Some(ps) = &mut v.pattern {
                            ps.preset_menu = None;
                            cx.notify();
                        }
                    }))
                    .min_w(px(180.0))
                    .py_1()
                    .rounded_md()
                    .bg(theme::POPOVER)
                    .border_1()
                    .border_color(theme::DIVIDER)
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .child(item("preset-edit", "Edit…", cx, |v, i, w, cx| {
                        v.edit_preset(Some(i), w, cx)
                    }))
                    .child(item("preset-delete", "Delete", cx, |v, i, _, cx| {
                        v.delete_preset(i, cx)
                    }))
                    .child(div().h(px(1.0)).my_1().bg(theme::DIVIDER))
                    .child(item(
                        "preset-restore",
                        "Restore default presets",
                        cx,
                        |v, _, _, cx| v.restore_default_presets(cx),
                    )),
            ),
    )
    .with_priority(1)
    .into_any_element()
}
