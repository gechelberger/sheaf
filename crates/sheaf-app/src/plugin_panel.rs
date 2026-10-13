//! A plugin's dock panel: rows the plugin supplies with `ui.set-panel`,
//! kept per document. Clicking a row shows its page; its action buttons
//! dispatch `plugin::PanelAction` back to the plugin.

use gpui::{AnyElement, Context, ScrollHandle, Window, div, prelude::*, px, rgb};
use sheaf_doc::PageId;
use sheaf_plugin::ui::PanelRow;

use crate::commands::PanelAction;
use crate::docview::{DocView, State};

/// One panel's rows for this document.
pub struct PluginPanelState {
    rows: Vec<PanelRow>,
    scroll: ScrollHandle,
}

impl DocView {
    /// Replace a panel's rows (`id` is `<plugin>.<panel>`).
    pub(crate) fn set_plugin_panel(
        &mut self,
        id: String,
        rows: Vec<PanelRow>,
        cx: &mut Context<Self>,
    ) {
        self.plugin_panels
            .entry(id)
            .or_insert_with(|| PluginPanelState {
                rows: Vec::new(),
                scroll: ScrollHandle::new(),
            })
            .rows = rows;
        cx.notify();
    }

    fn page_index(&self, id: PageId) -> Option<usize> {
        match &self.state {
            State::Ready(doc) => doc.model.index_of(id),
            _ => None,
        }
    }

    pub fn render_plugin_panel(
        &mut self,
        id: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(panel) = self.plugin_panels.get(id) else {
            return div()
                .p_3()
                .text_sm()
                .text_color(rgb(0x8a8a8a))
                .child("Nothing here yet. Run one of the plugin's commands.")
                .into_any_element();
        };
        if panel.rows.is_empty() {
            return div()
                .p_3()
                .text_sm()
                .text_color(rgb(0x8a8a8a))
                .child("No items.")
                .into_any_element();
        }
        let rows: Vec<AnyElement> = panel
            .rows
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let page = r.page.and_then(|p| self.page_index(p));
                let actions = r.actions.iter().enumerate().map(|(k, a)| {
                    let action = PanelAction {
                        panel: id.to_string(),
                        row: r.id.clone(),
                        action: a.id.clone(),
                    };
                    div()
                        .id(("plugin-row-action", i * 64 + k))
                        .px_2()
                        .py_0p5()
                        .rounded_sm()
                        .text_xs()
                        .cursor_pointer()
                        .bg(rgb(0x3a3a3a))
                        .hover(|s| s.bg(rgb(0x4a9eff)).text_color(rgb(0xffffff)))
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            window.dispatch_action(Box::new(action.clone()), cx);
                        })
                        .child(a.label.clone())
                });
                div()
                    .id(("plugin-row", i))
                    .flex()
                    .items_center()
                    .gap_2()
                    .py_1()
                    .pr_2()
                    .pl(px(8.0 + 14.0 * r.depth as f32))
                    .border_b_1()
                    .border_color(rgb(0x2a2a2a))
                    .when(page.is_some(), |d| {
                        d.cursor_pointer().hover(|s| s.bg(rgb(0x303030)))
                    })
                    .on_click(cx.listener(move |v, _, _, cx| {
                        if let Some(p) = page {
                            v.go_to_page(p, cx);
                        }
                    }))
                    .when_some(r.icon.glyph(), |d, g| {
                        d.child(
                            div()
                                .w(px(14.0))
                                .text_sm()
                                .text_color(rgb(0x9ac0ff))
                                .child(g),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(0xe0e0e0))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(r.label.clone()),
                            )
                            .when(!r.detail.is_empty(), |d| {
                                d.child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(0x8a8a8a))
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(r.detail.clone()),
                                )
                            }),
                    )
                    .when_some(r.badge.clone(), |d, b| {
                        d.child(
                            div()
                                .px_1p5()
                                .rounded_sm()
                                .bg(rgb(0x3a3a3a))
                                .text_xs()
                                .text_color(rgb(0xc0c0c0))
                                .child(b),
                        )
                    })
                    .children(actions)
                    .into_any_element()
            })
            .collect();
        div()
            .id("plugin-panel")
            .size_full()
            .track_scroll(&panel.scroll)
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .children(rows)
            .into_any_element()
    }
}
