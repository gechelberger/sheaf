//! Plugins in the app: discovery at startup, what their manifests
//! contribute (commands, menu items, key bindings, presets, marks), and
//! running their commands against the active document. The runtime itself
//! lives in `sheaf-plugin`; see `todos/plugins.md` for the design.
//!
//! Plugins install to `<config dir>/plugins/<id>/`, each with a
//! `sheaf-plugin.toml` and a `plugin.wasm`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use gpui::{App, Entity, Global, KeyBinding, KeyBindingContextPredicate};
use sheaf_doc::SourceId;
use sheaf_plugin::{Limits, Manifest, Plugin, PluginError, Runtime, TextSource};
use sheaf_render::RenderHandle;

use crate::commands::{CommandRegistry, Run};
use crate::contrib::{Contributions, Marks, TickSet};
use crate::docview::DocView;
use crate::patterns::Preset;
use crate::settings::Settings;

/// Loaded plugins by id, and what went wrong loading the rest.
#[derive(Default)]
pub struct PluginHost {
    plugins: HashMap<String, Plugin>,
    pub problems: Vec<String>,
}

impl Global for PluginHost {}

pub fn dir() -> PathBuf {
    crate::settings::config_dir().join("plugins")
}

/// Discover, register and start every enabled plugin. Call before the
/// user keymap loads (so it can override plugin bindings) and before the
/// menus are set.
pub fn init(settings: &Settings, cx: &mut App) {
    let mut host = PluginHost::default();
    let found = sheaf_plugin::discover(&dir());
    if found.is_empty() {
        cx.set_global(host);
        return;
    }
    let runtime = match Runtime::new(Limits::default()) {
        Ok(r) => r,
        Err(e) => {
            host.problems.push(format!("plugin runtime: {e:#}"));
            report(&host);
            cx.set_global(host);
            return;
        }
    };
    for f in found {
        let f = match f {
            Ok(f) => f,
            Err((dir, e)) => {
                host.problems.push(format!("{}: {e:#}", dir.display()));
                continue;
            }
        };
        let id = f.manifest.id.clone();
        if settings.plugins.disabled.contains(&id) {
            log::info!("plugin {id} is disabled");
            continue;
        }
        host.problems.extend(
            register(&f.manifest, cx)
                .into_iter()
                .map(|e| format!("{id}: {e}")),
        );
        log::info!("loading plugin {id} {}", f.manifest.version);
        host.plugins.insert(id, runtime.load(f.dir, f.manifest));
    }
    report(&host);
    cx.set_global(host);
}

fn report(host: &PluginHost) {
    for p in &host.problems {
        log::warn!("plugins: {p}");
        eprintln!("plugins: {p}");
    }
}

/// Add a manifest's static contributions. Returns one message per item
/// that couldn't be added; the rest still are.
fn register(m: &Manifest, cx: &mut App) -> Vec<String> {
    let mut errors = Vec::new();
    let mut bindings = Vec::new();
    let workspace = KeyBindingContextPredicate::parse(crate::workspace::KEY_CONTEXT)
        .ok()
        .map(Rc::new);
    for c in &m.commands {
        let added = cx.global_mut::<CommandRegistry>().add_plugin_command(
            &m.id,
            &c.id,
            &c.label,
            m.category(c),
            c.menu.as_deref(),
        );
        if let Err(e) = added {
            errors.push(e.to_string());
            continue;
        }
        if let Some(keys) = &c.keys {
            let action = Box::new(Run {
                plugin: m.id.clone(),
                command: c.id.clone(),
                args: None,
            });
            match KeyBinding::load(
                keys,
                action,
                workspace.clone(),
                false,
                None,
                cx.keyboard_mapper().as_ref(),
            ) {
                Ok(b) => bindings.push(b),
                Err(e) => errors.push(format!("command {:?}: keys {keys:?}: {e}", c.id)),
            }
        }
    }
    if !bindings.is_empty() {
        cx.bind_keys(bindings);
    }
    let presets = m
        .presets
        .iter()
        .map(|p| Preset {
            name: p.name.clone(),
            regex: p.regex.clone(),
        })
        .collect();
    let marks = Marks {
        tickmarks: m
            .marks
            .tickmarks
            .iter()
            .map(|t| TickSet {
                name: t.name.clone(),
                marks: t.marks.clone(),
            })
            .collect(),
        stamps: m.marks.stamps.clone(),
    };
    cx.global_mut::<Contributions>()
        .add_plugin(&m.id, presets, marks);
    errors
}

/// Page text from the document's render workers.
struct EngineText(HashMap<SourceId, RenderHandle>);

impl TextSource for EngineText {
    fn page_text(&self, source: SourceId, index: u32) -> Result<String> {
        let handle = self.0.get(&source).context("source not open")?;
        futures::executor::block_on(handle.page_text(index)).context("render thread is gone")?
    }
}

pub fn text_source(handles: &HashMap<SourceId, RenderHandle>) -> Arc<dyn TextSource> {
    Arc::new(EngineText(handles.clone()))
}

/// Run a plugin command against `doc` (the active document, if any) and
/// apply the edits it submits, one undo step per batch.
pub fn run(run: &Run, doc: Option<Entity<DocView>>, cx: &mut App) {
    let id = format!("{}::{}", run.plugin, run.command);
    let label = cx
        .global::<CommandRegistry>()
        .get(&id)
        .map(|c| crate::menus::parse_label(&c.label).0)
        .unwrap_or_else(|| id.clone());
    let notice = |doc: &Option<Entity<DocView>>, msg: String, cx: &mut App| {
        log::info!("{msg}");
        if let Some(doc) = doc {
            doc.update(cx, |d, cx| d.set_notice(msg, cx));
        }
    };
    let Some(plugin) = cx
        .try_global::<PluginHost>()
        .and_then(|h| h.plugins.get(&run.plugin))
    else {
        notice(
            &doc,
            format!("Plugin “{}” isn't installed or is disabled", run.plugin),
            cx,
        );
        return;
    };
    if cx.global::<CommandRegistry>().get(&id).is_none() {
        notice(
            &doc,
            format!("Plugin “{}” has no command “{}”", run.plugin, run.command),
            cx,
        );
        return;
    }
    let ctx = sheaf_plugin::CallContext {
        doc: doc.as_ref().and_then(|d| d.read(cx).plugin_context()),
    };
    let args = run.args.as_ref().map(|a| a.to_string());
    let rx = plugin.run_command(&run.command, args, ctx);
    notice(&doc, format!("{label}…"), cx);
    let weak = doc.as_ref().map(|d| d.downgrade());
    cx.spawn(async move |cx| {
        let result = rx
            .await
            .unwrap_or_else(|_| Err(PluginError::Unavailable("its thread stopped".into())));
        cx.update(|cx| {
            let doc = weak.and_then(|w| w.upgrade());
            match result {
                Ok(out) if out.batches.is_empty() => notice(&doc, format!("{label}: done"), cx),
                Ok(out) => match &doc {
                    Some(d) => d.update(cx, |d, cx| {
                        for b in out.batches {
                            d.apply_batch(b.edits, &b.label, cx);
                        }
                    }),
                    None => notice(&doc, format!("{label}: no document to edit"), cx),
                },
                Err(e) => {
                    log::warn!("{id}: {e}");
                    notice(&doc, format!("{label}: {e}"), cx);
                }
            }
        });
    })
    .detach();
}
