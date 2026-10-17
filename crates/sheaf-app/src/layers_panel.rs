//! The Layers panel: optional content groups, shown or hidden for viewing.
//! PDFium renders the default configuration, so a change reopens the
//! primary file's renderer from a copy whose default configuration says
//! what to hide. Saving never writes it.

use std::collections::HashSet;

use gpui::{AnyElement, Context, Window, div, prelude::*, px};
use sheaf_doc::layers::LayerId;
use sheaf_render::RenderHandle;
use sheaf_ui::theme;

use crate::docview::{DocView, State};

impl crate::docview::Doc {
    /// Layers as the file has them, once its facts are read.
    pub(crate) fn set_layers(&mut self, layers: Vec<sheaf_doc::layers::Layer>) {
        let hidden: HashSet<LayerId> = layers
            .iter()
            .filter(|l| !l.on)
            .filter_map(|l| l.id)
            .collect();
        self.layers_default = hidden.clone();
        self.layers_hidden = hidden;
        self.layers = layers;
    }

    /// The display differs from the file's own layer settings.
    pub(crate) fn layers_changed(&self) -> bool {
        self.layers_hidden != self.layers_default
    }
}

impl DocView {
    fn toggle_layer(&mut self, id: LayerId, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        if !doc.layers_hidden.remove(&id) {
            doc.layers_hidden.insert(id);
        }
        self.apply_layers(cx);
    }

    fn show_all_layers(&mut self, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let locked: HashSet<LayerId> = doc
            .layers
            .iter()
            .filter(|l| l.locked)
            .filter_map(|l| l.id)
            .collect();
        doc.layers_hidden.retain(|id| locked.contains(id));
        self.apply_layers(cx);
    }

    /// Reopen the primary file's renderer with the hidden layers off.
    pub(crate) fn apply_layers(&mut self, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let sid = doc.model.primary();
        let Some(src) = doc.model.source(sid) else {
            return;
        };
        let src = src.src.clone();
        let hidden = doc.layers_hidden.clone();
        let generation = {
            doc.layers_generation += 1;
            doc.layers_generation
        };
        doc.notice = Some("Updating layers…".into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let opened = cx
                .background_executor()
                .spawn(async move {
                    let mut pdf = sheaf_doc::file::load_pdf_mem(src.bytes(), src.password())?;
                    sheaf_doc::layers::set_hidden(&mut pdf, &hidden);
                    let mut bytes = Vec::new();
                    pdf.save_to(&mut bytes)?;
                    RenderHandle::open_bytes(bytes.into(), None)
                })
                .await;
            this.update(cx, |v, cx| {
                let State::Ready(doc) = &mut v.state else {
                    return;
                };
                // A newer change, or a save, superseded this one.
                if doc.layers_generation != generation || doc.model.primary() != sid {
                    return;
                }
                match opened {
                    Ok(handle) => {
                        let pages = doc.drawn_marks.clone();
                        if !pages.is_empty() {
                            // Our own marks are drawn by sheaf, not PDFium.
                            drop(handle.hide_annots(pages));
                        }
                        doc.handles.insert(sid, handle);
                        // A new engine: hide in it what was hidden before.
                        doc.foreign_hidden.retain(|(s, _, _)| *s != sid);
                        doc.invalidate_tiles();
                        let off = doc.layers_hidden.len();
                        doc.notice = Some(match off {
                            0 => "All layers shown".into(),
                            1 => "1 layer hidden (display only; not saved)".into(),
                            n => format!("{n} layers hidden (display only; not saved)"),
                        });
                    }
                    Err(e) => doc.notice = Some(format!("Couldn't update layers: {e:#}")),
                }
                v.sync_foreign_visibility(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The Layers panel body.
    pub fn render_layers(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let State::Ready(doc) = &self.state else {
            return div().p_2().text_sm().child("…").into_any_element();
        };
        if doc.layers.is_empty() {
            return div()
                .p_2()
                .text_sm()
                .text_color(theme::TEXT_MUTED)
                .child(if doc.model.is_ready() {
                    "This document has no layers"
                } else {
                    "Reading layers…"
                })
                .into_any_element();
        }
        let rows = doc.layers.iter().enumerate().map(|(i, l)| {
            let indent = px(8.0 + 16.0 * l.depth as f32);
            let row = div()
                .id(("layer", i))
                .pl(indent)
                .pr_2()
                .py_1()
                .flex()
                .items_center()
                .gap_2()
                .text_sm();
            let Some(id) = l.id else {
                // A heading in the layer order.
                return row
                    .text_color(theme::TEXT_MUTED)
                    .child(l.name.clone())
                    .into_any_element();
            };
            let on = !doc.layers_hidden.contains(&id);
            let row = row
                .child(
                    div()
                        .w(px(16.0))
                        .text_color(if on { theme::ACCENT } else { theme::TEXT_DIM })
                        .child(if on { "☑" } else { "☐" }),
                )
                .child(
                    div()
                        .text_color(if on {
                            theme::TEXT_BRIGHT
                        } else {
                            theme::TEXT_MUTED
                        })
                        .child(l.name.clone()),
                );
            if l.locked {
                row.text_color(theme::TEXT_DIM)
                    .child(div().text_xs().child("locked"))
                    .into_any_element()
            } else {
                row.cursor_pointer()
                    .hover(|s| s.bg(theme::HOVER_SUBTLE))
                    .on_click(cx.listener(move |v, _, _, cx| v.toggle_layer(id, cx)))
                    .into_any_element()
            }
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .p_2()
                    .border_b_1()
                    .border_color(theme::DIVIDER)
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(theme::TEXT_MUTED)
                            .child("Click to show or hide; display only"),
                    )
                    .child(
                        sheaf_ui::button::Button::new("layers-all", "Show All")
                            .on_click(cx.listener(|v, _, _, cx| v.show_all_layers(cx))),
                    ),
            )
            .child(
                div()
                    .id("layers")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .children(rows),
            )
            .into_any_element()
    }
}
