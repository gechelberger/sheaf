//! Dev: `--script steps.json` drives the first window from a list of
//! steps, for UI tests where the OS won't take synthetic input. Each step is
//! an object with one key (plus options):
//!
//! - `{"action": "workspace::CommandPalette", "args": {...}}`: dispatch an
//!   action by name, as `keymap.json` names it.
//! - `{"keys": "ctrl-shift-k escape"}`: keystrokes, space-separated.
//! - `{"type": "text"}`: type text into whatever has focus.
//! - `{"click": [x, y]}`, `{"drag": [x1, y1, x2, y2], "shift": true}`: in
//!   window coordinates (logical px); `"right": true` clicks the right
//!   button. `{"hover": [x, y]}` only moves. `{"scroll": [x, y, dy]}`
//!   turns the wheel there by `dy` px (negative goes down).
//! - `{"drag_page": [x1, y1, x2, y2], "page": 0, "shift": true}`: in page
//!   points (the page as the file draws it), on page index `page`.
//!   `{"hover_page": [x, y], "page": 0}` only moves there, as a pointer
//!   does before it presses.
//! - `{"form": {"field-id": value, ...}}`: fill the open plugin form and
//!   submit it (choices by index, page ranges as text).
//! - `{"wait": 500}`: milliseconds.
//! - `{"checkpoint": "name"}`: print `SCRIPT checkpoint name` and wait (up to
//!   two minutes) until `<script>.go` contains that name, so a harness can
//!   take a screenshot first.
//! - `{"dump": "outline" | "notice" | "selection" | "annots" | "pages" | "panels" |
//!   "plugins" | "region" | "view" | "layout" | "toolbar" | "focus" | "form"}`: print state as `SCRIPT <what>: …` lines.
//! - `{"quit": true}`
//!
//! File pickers can't be driven; set `SHEAF_SCRIPT_PICK=<path>` and every
//! open, folder and save picker answers with that path. `SHEAF_SCRIPT_SAVE`
//! answers save pickers instead, when a script opens one file and saves
//! another.
//!
//! Output goes to stderr. Never part of normal use.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::{
    App, AppContext as _, AsyncApp, Keystroke, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PlatformInput, ScrollDelta, ScrollWheelEvent, TouchPhase,
    WindowHandle, point, px,
};
use serde_json::Value;

use crate::workspace::Workspace;

pub fn start(path: PathBuf, handle: WindowHandle<Workspace>, cx: &mut App) {
    let steps: Vec<Value> = match std::fs::read(&path)
        .map_err(|e| e.to_string())
        .and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()))
    {
        Ok(s) => s,
        Err(e) => {
            eprintln!("SCRIPT error: {}: {e}", path.display());
            return;
        }
    };
    let go = path.with_extension("go");
    cx.spawn(async move |cx| {
        // Let the document open and lay out first.
        cx.background_executor()
            .timer(Duration::from_millis(1500))
            .await;
        for (i, step) in steps.iter().enumerate() {
            eprintln!("SCRIPT step {i}: {step}");
            if let Err(e) = run_step(step, handle, &go, cx).await {
                eprintln!("SCRIPT error at step {i}: {e}");
            }
        }
        eprintln!("SCRIPT done");
    })
    .detach();
}

async fn pause(cx: &AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

async fn run_step(
    step: &Value,
    handle: WindowHandle<Workspace>,
    go: &std::path::Path,
    cx: &mut AsyncApp,
) -> Result<(), String> {
    let shift = step.get("shift").and_then(Value::as_bool).unwrap_or(false);
    if let Some(ms) = step.get("wait").and_then(Value::as_u64) {
        pause(cx, ms).await;
        return Ok(());
    }
    if let Some(name) = step.get("checkpoint").and_then(Value::as_str) {
        // Synthetic input doesn't always schedule a frame; make sure the
        // screenshot sees the current state.
        // A grab can show the frame before the last; draw twice.
        for _ in 0..2 {
            cx.update_window(handle.into(), |_, window, _| window.refresh())
                .map_err(|e| e.to_string())?;
            pause(cx, 300).await;
        }
        eprintln!("SCRIPT checkpoint {name}");
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(120) {
            if std::fs::read_to_string(go).is_ok_and(|s| s.trim() == name) {
                return Ok(());
            }
            pause(cx, 100).await;
        }
        return Err(format!("checkpoint {name} timed out"));
    }
    if step.get("quit").is_some() {
        cx.update(crate::instance::quit);
        return Ok(());
    }
    if step.get("dump").and_then(Value::as_str) == Some("focus") {
        return cx
            .update_window(handle.into(), |_, window, cx| {
                let contexts: Vec<String> = window
                    .context_stack()
                    .iter()
                    .map(|c| format!("{c:?}"))
                    .collect();
                eprintln!(
                    "SCRIPT focus: {} {}",
                    if window.focused(cx).is_some() {
                        "something"
                    } else {
                        "nothing"
                    },
                    contexts.join(" > ")
                );
            })
            .map_err(|e| e.to_string());
    }
    if let Some(what) = step.get("dump").and_then(Value::as_str) {
        let what = what.to_string();
        return handle
            .update(cx, |ws, _, cx| ws.script_dump(&what, cx))
            .map_err(|e| e.to_string());
    }
    if let Some(values) = step.get("form").and_then(Value::as_object) {
        let values = values.clone();
        let ok = handle
            .update(cx, |ws, window, cx| ws.script_form(&values, window, cx))
            .map_err(|e| e.to_string())?;
        pause(cx, 200).await;
        return if ok {
            Ok(())
        } else {
            Err("no plugin form is open".into())
        };
    }
    if let Some(name) = step.get("action").and_then(Value::as_str) {
        let args = step.get("args").cloned();
        let name = name.to_string();
        cx.update_window(handle.into(), |_, window, cx| {
            let action = cx.build_action(&name, args).map_err(|e| e.to_string())?;
            window.dispatch_action(action, cx);
            Ok::<_, String>(())
        })
        .map_err(|e| e.to_string())??;
        pause(cx, 250).await;
        return Ok(());
    }
    if let Some(keys) = step.get("keys").and_then(Value::as_str) {
        for k in keys.split_whitespace() {
            let ks = Keystroke::parse(k).map_err(|e| format!("{k}: {e:?}"))?;
            cx.update_window(handle.into(), |_, window, cx| {
                window.dispatch_keystroke(ks, cx)
            })
            .map_err(|e| e.to_string())?;
            pause(cx, 60).await;
        }
        pause(cx, 150).await;
        return Ok(());
    }
    if let Some(text) = step.get("type").and_then(Value::as_str) {
        for ch in text.chars() {
            let ks = Keystroke::parse(
                if ch == ' ' {
                    "space".to_string()
                } else {
                    ch.to_string()
                }
                .as_str(),
            )
            .map_err(|e| format!("{ch}: {e:?}"))?;
            cx.update_window(handle.into(), |_, window, cx| {
                window.dispatch_keystroke(ks, cx)
            })
            .map_err(|e| e.to_string())?;
            pause(cx, 20).await;
        }
        pause(cx, 150).await;
        return Ok(());
    }
    let coords = |key: &str| -> Option<Vec<f32>> {
        step.get(key)?
            .as_array()?
            .iter()
            .map(|v| v.as_f64().map(|f| f as f32))
            .collect()
    };
    let page_point = |c: &[f32], cx: &mut AsyncApp| {
        let page = step.get("page").and_then(Value::as_u64).unwrap_or(0) as usize;
        handle
            .update(cx, |ws, _, cx| {
                let doc = ws.doc()?.clone();
                doc.read(cx).window_point(page, c[0], c[1])
            })
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "that page isn't laid out".to_string())
    };
    let hover_at = if let Some(c) = coords("hover_page") {
        let p = page_point(&c, cx)?;
        Some(vec![p.0, p.1])
    } else {
        coords("hover")
    };
    if let Some(c) = coords("scroll").filter(|c| c.len() == 3) {
        cx.update_window(handle.into(), |_, window, cx| {
            window.dispatch_event(
                PlatformInput::ScrollWheel(ScrollWheelEvent {
                    position: point(px(c[0]), px(c[1])),
                    delta: ScrollDelta::Pixels(point(px(0.0), px(c[2]))),
                    modifiers: Modifiers::default(),
                    touch_phase: TouchPhase::Moved,
                }),
                cx,
            );
        })
        .map_err(|e| e.to_string())?;
        pause(cx, 100).await;
        return Ok(());
    }
    if let Some(c) = hover_at {
        cx.update_window(handle.into(), |_, window, cx| {
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: point(px(c[0]), px(c[1])),
                    pressed_button: None,
                    modifiers: Modifiers::default(),
                }),
                cx,
            );
        })
        .map_err(|e| e.to_string())?;
        pause(cx, 100).await;
        Ok(())
    } else if let Some(c) = coords("click") {
        let button = if step.get("right").and_then(Value::as_bool) == Some(true) {
            MouseButton::Right
        } else {
            MouseButton::Left
        };
        mouse(handle, cx, (c[0], c[1]), (c[0], c[1]), shift, button).await
    } else if let Some(c) = coords("drag") {
        mouse(
            handle,
            cx,
            (c[0], c[1]),
            (c[2], c[3]),
            shift,
            MouseButton::Left,
        )
        .await
    } else if let Some(c) = coords("drag_page") {
        let page = step.get("page").and_then(Value::as_u64).unwrap_or(0) as usize;
        let (a, b) = handle
            .update(cx, |ws, _, cx| {
                let doc = ws.doc()?.clone();
                let a = doc.read(cx).window_point(page, c[0], c[1])?;
                let b = doc.read(cx).window_point(page, c[2], c[3])?;
                Some((a, b))
            })
            .map_err(|e| e.to_string())?
            .ok_or("that page isn't laid out")?;
        mouse(handle, cx, a, b, shift, MouseButton::Left).await
    } else {
        Err(format!("unknown step {step}"))
    }
}

/// Press at `from`, move to `to` in steps, release.
async fn mouse(
    handle: WindowHandle<Workspace>,
    cx: &mut AsyncApp,
    from: (f32, f32),
    to: (f32, f32),
    shift: bool,
    button: MouseButton,
) -> Result<(), String> {
    let modifiers = Modifiers {
        shift,
        ..Modifiers::default()
    };
    let at = |(x, y): (f32, f32)| point(px(x), px(y));
    // Through the window, not the workspace: dispatch updates the
    // workspace itself, so it mustn't already be borrowed.
    let send = |input: PlatformInput, cx: &mut AsyncApp| {
        cx.update_window(handle.into(), |_, window, cx| {
            window.dispatch_event(input, cx);
        })
        .map_err(|e| e.to_string())
    };
    send(
        PlatformInput::MouseMove(MouseMoveEvent {
            position: at(from),
            pressed_button: None,
            modifiers,
        }),
        cx,
    )?;
    // Let a frame paint, so hover and hitboxes are up to date.
    pause(cx, 100).await;
    send(
        PlatformInput::MouseDown(MouseDownEvent {
            button,
            position: at(from),
            modifiers,
            click_count: 1,
            first_mouse: false,
        }),
        cx,
    )?;
    // A click is a press and release in place: moving with the button
    // down, even by nothing, can start a drag on draggable rows.
    let steps = if from == to { 0 } else { 12 };
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        let p = (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
        send(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: at(p),
                pressed_button: Some(button),
                modifiers,
            }),
            cx,
        )?;
        pause(cx, 25).await;
    }
    // GPUI listens for a click's release only from the frame after the
    // press, so let one paint.
    pause(cx, 100).await;
    send(
        PlatformInput::MouseUp(MouseUpEvent {
            button,
            position: at(to),
            modifiers,
            click_count: 1,
        }),
        cx,
    )?;
    pause(cx, 250).await;
    Ok(())
}

impl crate::docview::DocView {
    /// Window position (logical px) of point (`x`, `y`) on page `page`, on
    /// the page as its file shows it (points from its top-left, y down).
    pub(crate) fn window_point(&self, page: usize, x: f32, y: f32) -> Option<(f32, f32)> {
        let crate::docview::State::Ready(doc) = &self.state else {
            return None;
        };
        let pane = doc.pane();
        let r = pane.layout.as_ref()?.rect(page)?;
        let pref = doc.page_ref(page)?;
        let file = doc.model.source(pref.source)?.src.geom(pref.index);
        let geom = doc.pane_geom(page)?;
        let shown = file
            .from_view()
            .then(geom.to_view())
            .apply(sheaf_doc::Pt { x, y });
        let k = r.w / geom.shown().width.max(1.0);
        let b = pane.scroll.bounds();
        let off = pane.scroll.offset();
        Some((
            f32::from(b.origin.x) + f32::from(off.x) + r.left + shown.x * k,
            f32::from(b.origin.y) + f32::from(off.y) + r.top + shown.y * k,
        ))
    }
}

impl Workspace {
    /// Fill the open plugin form with `values` (by field id) and submit it.
    pub(crate) fn script_form(
        &mut self,
        values: &serde_json::Map<String, Value>,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> bool {
        #[cfg(feature = "plugins")]
        if let Some((form, _)) = &self.plugin_form {
            let form = form.clone();
            form.update(cx, |f, cx| f.script_fill(values, cx));
            return true;
        }
        let _ = (values, cx);
        false
    }

    pub(crate) fn script_dump(&mut self, what: &str, cx: &mut gpui::Context<Self>) {
        use crate::docview::State;
        if what == "plugins" {
            #[cfg(feature = "plugins")]
            if let Some(host) = cx.try_global::<crate::plugins::PluginHost>() {
                for i in &host.installed {
                    eprintln!(
                        "SCRIPT plugins: {} enabled={} status={:?}",
                        i.manifest.id,
                        host.is_enabled(&i.manifest.id),
                        host.status(&i.manifest.id)
                    );
                }
                for p in &host.problems {
                    eprintln!("SCRIPT plugins: problem {p}");
                }
            }
            return;
        }
        let Some(doc) = self.doc().cloned() else {
            eprintln!("SCRIPT {what}: no document");
            return;
        };
        let view = doc.read(cx);
        let State::Ready(d) = &view.state else {
            eprintln!("SCRIPT {what}: not ready");
            return;
        };
        let st = d.model.state();
        match what {
            "notice" => eprintln!("SCRIPT notice: {:?}", d.notice),
            "form" => {
                let widgets: usize = st.widgets.values().map(|w| w.len()).sum();
                eprintln!(
                    "SCRIPT form: {} fields, {widgets} widgets; bar {:?}",
                    st.fields.len(),
                    d.form_bar()
                );
                match &d.forms.primary {
                    Some(facts) => eprintln!("SCRIPT form: {facts}"),
                    None => eprintln!("SCRIPT form: not read yet"),
                }
                for (source, facts) in &d.forms.incoming {
                    eprintln!("SCRIPT form: source {} {facts}", source.0);
                }
            }
            "selection" => eprintln!(
                "SCRIPT selection: {:?}",
                view.selected_text().map(|(_, text, _)| text)
            ),
            "pages" => eprintln!("SCRIPT pages: {}", st.pages.len()),
            "view" => eprintln!(
                "SCRIPT view: page {} {:?} selected {:?}",
                view.current_page() + 1,
                view.current_view().map(|(_, v)| v),
                d.outline_selected
                    .and_then(|id| st.outline_flat().into_iter().find(|(_, n)| n.id == id))
                    .map(|(_, n)| n.title.clone())
            ),
            "toolbar" => {
                for line in self.toolbar_dump(cx) {
                    eprintln!("SCRIPT toolbar: {line}");
                }
            }
            "layout" => {
                for (i, pane) in d.panes.iter().enumerate() {
                    eprintln!(
                        "SCRIPT layout: pane {i}{} {} row {} rotate {} page {} zoom {:.0}%",
                        if i == d.active { " (active)" } else { "" },
                        pane.mode.label(),
                        pane.row,
                        pane.rotation.degrees(),
                        pane.current_page + 1,
                        pane.layout.as_ref().map_or(0.0, |l| l.zoom_percent()),
                    );
                }
            }
            "region" => eprintln!(
                "SCRIPT region: {:?}",
                view.region.as_ref().map(|r| (r.page, r.rect()))
            ),
            "outline" => {
                for (depth, n) in st.outline_flat() {
                    eprintln!(
                        "SCRIPT outline: {}{} -> page {:?} view {:?}",
                        "  ".repeat(depth as usize),
                        n.title,
                        n.target.and_then(|t| st.index_of(t)).map(|i| i + 1),
                        n.view
                    );
                }
            }
            "annots" => {
                for (i, p) in st.pages.iter().enumerate() {
                    for a in st.annots_on(p.id).into_iter().flatten() {
                        let kind = format!("{:?}", a.kind);
                        let kind = kind.split([' ', '{', '(']).next().unwrap_or("");
                        eprintln!(
                            "SCRIPT annots: page {} {kind} {:?} color {:?} fill {:?} width {} dashed {} at {:?}",
                            i + 1,
                            a.contents,
                            a.color,
                            a.fill,
                            a.stroke,
                            a.dashed,
                            a.page_bounds()
                        );
                    }
                }
            }
            "panels" =>
            {
                #[cfg(feature = "plugins")]
                for (id, rows) in view.plugin_panel_rows() {
                    for r in rows {
                        eprintln!(
                            "SCRIPT panels: {id}: {}{} | {}",
                            "  ".repeat(r.depth as usize),
                            r.label,
                            r.detail
                        );
                    }
                }
            }
            other => eprintln!("SCRIPT {other}: unknown dump"),
        }
    }
}
