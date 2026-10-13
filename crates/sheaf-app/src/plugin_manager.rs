//! Plugins > Manage Plugins…: every installed plugin with its status and
//! permissions, to turn on or off, allow or revoke, or uninstall; and
//! installing from a folder or a `.sheafplugin` file. Changes apply at
//! once.

use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    AnyElement, App, Context, EventEmitter, FocusHandle, Focusable, MouseButton, PathPromptOptions,
    PromptLevel, ScrollHandle, Window, div, prelude::*, px, rgb, rgba,
};
use sheaf_plugin::Status;

use crate::DialogCancel;
use crate::plugins::{self, PluginHost};

pub const KEY_CONTEXT: &str = "PluginManager";

pub enum PluginManagerEvent {
    Dismiss,
}

pub struct PluginManager {
    focus: FocusHandle,
    scroll: ScrollHandle,
    /// The outcome of the last install, uninstall or change.
    message: Option<(bool, String)>,
    /// A redraw is queued while some plugin is still loading.
    polling: bool,
}

impl EventEmitter<PluginManagerEvent> for PluginManager {}

impl Focusable for PluginManager {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// One installed plugin, as shown.
struct Row {
    id: String,
    name: String,
    version: String,
    description: String,
    enabled: bool,
    status: Option<Status>,
    /// Permission lines, if it asks for any.
    asks: Vec<String>,
    allowed: bool,
}

fn button(
    id: impl Into<gpui::ElementId>,
    label: &str,
    primary: bool,
    on_click: impl Fn(&mut PluginManager, &mut Window, &mut Context<PluginManager>) + 'static,
    cx: &mut Context<PluginManager>,
) -> AnyElement {
    div()
        .id(id.into())
        .px_2()
        .py_0p5()
        .rounded_sm()
        .text_xs()
        .cursor_pointer()
        .when(primary, |d| d.bg(rgb(0x4a9eff)).text_color(rgb(0xffffff)))
        .when(!primary, |d| d.bg(rgb(0x3a3a3a)))
        .hover(|s| s.bg(rgb(0x5a5a5a)))
        .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
        .child(label.to_string())
        .into_any_element()
}

impl PluginManager {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            focus,
            scroll: ScrollHandle::new(),
            message: None,
            polling: false,
        }
    }

    fn rows(cx: &App) -> Vec<Row> {
        let Some(host) = cx.try_global::<PluginHost>() else {
            return Vec::new();
        };
        let mut rows: Vec<Row> = host
            .installed
            .iter()
            .map(|i| {
                let m = &i.manifest;
                let granted = host.state.granted(m);
                Row {
                    id: m.id.clone(),
                    name: m.name.clone(),
                    version: m.version.to_string(),
                    description: m.description.clone(),
                    enabled: host.is_enabled(&m.id),
                    status: host.status(&m.id),
                    asks: m.permissions.grants().describe(),
                    allowed: granted == m.permissions,
                }
            })
            .collect();
        rows.sort_by_key(|r| r.name.to_lowercase());
        rows
    }

    fn done(&mut self, result: anyhow::Result<String>, cx: &mut Context<Self>) {
        self.message = Some(match result {
            Ok(m) => (true, m),
            Err(e) => (false, format!("{e:#}")),
        });
        cx.notify();
    }

    fn install(&mut self, archive: bool, cx: &mut Context<Self>) {
        let rx = crate::file_dialog::open(
            cx,
            PathPromptOptions {
                files: archive,
                directories: !archive,
                multiple: false,
                prompt: None,
            },
        );
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = rx.await else {
                return;
            };
            if paths.is_empty() {
                return;
            }
            let path: PathBuf = paths.remove(0);
            this.update(cx, |m, cx| {
                let result = if archive {
                    plugins::install_archive(&path, cx)
                } else {
                    plugins::install_dir(&path, cx)
                };
                let result =
                    result.map(|id| format!("Installed {}", plugins::plugin_name(&id, cx)));
                m.done(result, cx);
            })
            .ok();
        })
        .detach();
    }

    fn uninstall(&mut self, id: String, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let rx = window.prompt(
            PromptLevel::Warning,
            &format!("Uninstall “{name}”?"),
            Some("Its files are removed. Data it saved stays, in case you reinstall it."),
            &["Uninstall", "Cancel"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if matches!(rx.await, Ok(0)) {
                this.update(cx, |m, cx| {
                    let r = plugins::uninstall(&id, cx).map(|()| format!("Uninstalled {name}"));
                    m.done(r, cx);
                })
                .ok();
            }
        })
        .detach();
    }

    /// Redraw shortly while a plugin is still loading, to show it finish.
    fn poll(&mut self, cx: &mut Context<Self>) {
        if self.polling {
            return;
        }
        self.polling = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            this.update(cx, |m, cx| {
                m.polling = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn render_row(&self, i: usize, r: &Row, cx: &mut Context<Self>) -> AnyElement {
        let (status, color) = match (&r.status, r.enabled) {
            (_, false) => ("Off".to_string(), 0x8a8a8a),
            (Some(Status::Ready), _) => ("On".to_string(), 0x6fcf6f),
            (Some(Status::Loading), _) => ("Loading…".to_string(), 0xd0b060),
            (Some(Status::Failed(_)), _) => ("Error".to_string(), 0xe08080),
            (None, true) => ("Not running".to_string(), 0xe08080),
        };
        let error = match &r.status {
            Some(Status::Failed(m)) if r.enabled => Some(m.clone()),
            _ => None,
        };
        let (id, name) = (r.id.clone(), r.name.clone());
        let enabled = r.enabled;
        let toggle = {
            let id = id.clone();
            button(
                ("plugin-toggle", i),
                if enabled { "Turn Off" } else { "Turn On" },
                false,
                move |m, _, cx| {
                    plugins::set_enabled(&id, !enabled, cx);
                    m.message = None;
                    cx.notify();
                },
                cx,
            )
        };
        let remove = {
            let (id, name) = (id.clone(), name.clone());
            button(
                ("plugin-uninstall", i),
                "Uninstall",
                false,
                move |m, window, cx| m.uninstall(id.clone(), name.clone(), window, cx),
                cx,
            )
        };
        let permissions = (!r.asks.is_empty()).then(|| {
            let allowed = r.allowed;
            let id = id.clone();
            div()
                .mt_1()
                .flex()
                .items_start()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .text_xs()
                        .text_color(if allowed {
                            rgb(0xb0b0b0)
                        } else {
                            rgb(0xd0b060)
                        })
                        .child(if allowed {
                            "Allowed to:"
                        } else {
                            "Asks to (not allowed):"
                        })
                        .children(r.asks.iter().map(|l| div().pl_2().child(format!("• {l}")))),
                )
                .child(button(
                    ("plugin-perm", i),
                    if allowed { "Revoke" } else { "Allow" },
                    !allowed,
                    move |m, _, cx| {
                        plugins::set_allowed(&id, !allowed, cx);
                        m.message = None;
                        cx.notify();
                    },
                    cx,
                ))
        });
        div()
            .py_2()
            .px_2()
            .border_b_1()
            .border_color(rgb(0x333333))
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xe8e8e8))
                            .child(r.name.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x7a7a7a))
                            .child(format!("{} · {}", r.version, r.id)),
                    )
                    .child(div().text_xs().text_color(rgb(color)).child(status))
                    .child(div().flex_1())
                    .child(toggle)
                    .child(remove),
            )
            .when(!r.description.is_empty(), |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(rgb(0x9a9a9a))
                        .child(r.description.clone()),
                )
            })
            .when_some(error, |d, e| {
                d.child(div().text_xs().text_color(rgb(0xe08080)).child(e))
            })
            .children(permissions)
            .into_any_element()
    }
}

impl Render for PluginManager {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = Self::rows(cx);
        if rows
            .iter()
            .any(|r| r.enabled && matches!(r.status, Some(Status::Loading)))
        {
            self.poll(cx);
        }
        let problems: Vec<String> = cx
            .try_global::<PluginHost>()
            .map(|h| h.problems.clone())
            .unwrap_or_default();
        let list: Vec<AnyElement> = rows
            .iter()
            .enumerate()
            .map(|(i, r)| self.render_row(i, r, cx))
            .collect();
        let empty = list.is_empty();
        let dir = plugins::dir();

        div()
            .id("plugins-backdrop")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(
                cx.listener(|_, _: &DialogCancel, _, cx| cx.emit(PluginManagerEvent::Dismiss)),
            )
            .absolute()
            .inset_0()
            .bg(rgba(0x00000060))
            .flex()
            .justify_center()
            .items_start()
            .pt(px(60.0))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(PluginManagerEvent::Dismiss)),
            )
            .child(
                div()
                    .id("plugins")
                    .w(px(640.0))
                    .max_h(px(620.0))
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .bg(rgb(0x262626))
                    .border_1()
                    .border_color(rgb(0x4a4a4a))
                    .rounded_md()
                    .shadow_lg()
                    .text_color(rgb(0xd0d0d0))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .pb_2()
                            .border_b_1()
                            .border_color(rgb(0x111111))
                            .flex()
                            .items_baseline()
                            .gap_2()
                            .child(div().text_sm().text_color(rgb(0xe8e8e8)).child("Plugins"))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x7a7a7a))
                                    .child(dir.display().to_string()),
                            ),
                    )
                    .children(problems.into_iter().map(|p| {
                        div()
                            .px_2()
                            .text_xs()
                            .text_color(rgb(0xe08080))
                            .child(p)
                            .into_any_element()
                    }))
                    .child(
                        div()
                            .id("plugins-list")
                            .flex_1()
                            .min_h(px(0.0))
                            .track_scroll(&self.scroll)
                            .overflow_y_scroll()
                            .when(empty, |d| {
                                d.child(
                                    div()
                                        .p_3()
                                        .text_sm()
                                        .text_color(rgb(0x8a8a8a))
                                        .child("No plugins installed."),
                                )
                            })
                            .children(list),
                    )
                    .when_some(self.message.clone(), |d, (ok, m)| {
                        d.child(
                            div()
                                .px_2()
                                .pt_1()
                                .text_xs()
                                .text_color(if ok { rgb(0x9ad09a) } else { rgb(0xe08080) })
                                .child(m),
                        )
                    })
                    .child(
                        div()
                            .pt_2()
                            .flex()
                            .gap_2()
                            .child(button(
                                "install-folder",
                                "Install from Folder…",
                                false,
                                |m, _, cx| m.install(false, cx),
                                cx,
                            ))
                            .child(button(
                                "install-file",
                                "Install from File…",
                                false,
                                |m, _, cx| m.install(true, cx),
                                cx,
                            ))
                            .child(div().flex_1())
                            .child(button(
                                "plugins-close",
                                "Close",
                                true,
                                |_, _, cx| cx.emit(PluginManagerEvent::Dismiss),
                                cx,
                            )),
                    ),
            )
    }
}
