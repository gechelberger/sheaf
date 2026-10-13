//! Plugins in the app: discovery at startup, what their manifests
//! contribute (commands, menu items, key bindings, presets, marks, panels),
//! and running their calls against the active document with the UI they
//! ask for. The runtime itself lives in `sheaf-plugin`; see
//! `todos/plugins.md` for the design.
//!
//! Plugins live in `<config dir>/plugins/`, one directory per plugin
//! holding a `sheaf-plugin.toml` and a `plugin.wasm`. The plugins that ship
//! with sheaf (the repo's `plugins/`, built by `scripts/build-plugins.sh`
//! into `plugins/` next to the executable) are installed there on first
//! run and are ordinary plugins from then on: the user can turn them off
//! (`plugins.disabled` in `settings.json`), replace or delete them like
//! any other. See [`install_bundled`].

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use futures::StreamExt;
use futures::channel::{mpsc, oneshot};
use gpui::{App, Context, Entity, Global, KeyBinding, KeyBindingContextPredicate, Window};
use sheaf_doc::SourceId;
use sheaf_plugin::ui::{FieldAnswer, Form, PanelRow, Ui};
use sheaf_plugin::{
    Call, CallContext, Limits, Manifest, Plugin, PluginError, Runtime, TextMatch, TextSource,
};
use sheaf_render::RenderHandle;

use crate::commands::{Command, CommandRegistry, Run, ShowPluginPanel, Target};
use crate::contrib::{Contributions, Marks, PluginPanel, TickSet};
use crate::docview::DocView;
use crate::patterns::Preset;
use crate::settings::Settings;
use crate::workspace::Workspace;

/// A plugin found on disk, loaded or not.
pub struct Installed {
    pub manifest: Manifest,
    pub dir: PathBuf,
    pub enabled: bool,
}

/// Every plugin found, the running ones by id, and what went wrong.
#[derive(Default)]
pub struct PluginHost {
    pub installed: Vec<Installed>,
    plugins: HashMap<String, Plugin>,
    pub problems: Vec<String>,
}

impl Global for PluginHost {}

/// Where plugins are installed.
pub fn dir() -> PathBuf {
    crate::settings::config_dir().join("plugins")
}

/// Where the plugins that ship with sheaf are: `SHEAF_BUNDLED_PLUGINS` if set, else
/// `plugins/` next to the executable; debug builds fall back to the
/// workspace's `target/plugins`, where `scripts/build-plugins.sh` puts them.
pub fn bundled_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("SHEAF_BUNDLED_PLUGINS") {
        return Some(PathBuf::from(dir));
    }
    let beside = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("plugins")))
        .filter(|d| d.is_dir());
    if beside.is_some() {
        return beside;
    }
    cfg!(debug_assertions)
        .then(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/plugins"))
        .filter(|d| d.is_dir())
}

/// What `install_bundled` has installed, so a bundled plugin the user
/// deleted stays deleted and one they replaced isn't overwritten.
const BUNDLED_RECORD: &str = ".bundled.json";

/// Install the plugins that ship with sheaf into `dest`: each one not
/// installed before, and newer versions of ones still as installed. Returns
/// one message per plugin that couldn't be installed.
pub fn install_bundled(from: &Path, dest: &Path) -> Vec<String> {
    let record_path = dest.join(BUNDLED_RECORD);
    let mut record: BTreeMap<String, semver::Version> = std::fs::read(&record_path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let mut errors = Vec::new();
    let mut changed = false;
    for found in sheaf_plugin::discover(from) {
        let f = match found {
            Ok(f) => f,
            Err((dir, e)) => {
                errors.push(format!("bundled {}: {e:#}", dir.display()));
                continue;
            }
        };
        let id = &f.manifest.id;
        let target = dest.join(id);
        let install = match record.get(id) {
            // Never installed: install.
            None => true,
            // Installed before: upgrade only our own, unchanged copy. A
            // missing directory means the user deleted it.
            Some(v) => {
                *v < f.manifest.version && Manifest::load(&target).is_ok_and(|m| m.version == *v)
            }
        };
        if !install {
            continue;
        }
        if !record.contains_key(id) && target.exists() {
            // The user installed their own copy first; it wins.
            record.insert(id.clone(), f.manifest.version.clone());
            changed = true;
            continue;
        }
        match copy_plugin(&f.dir, &target) {
            Ok(()) => {
                log::info!("installed bundled plugin {id} {}", f.manifest.version);
                record.insert(id.clone(), f.manifest.version.clone());
                changed = true;
            }
            Err(e) => errors.push(format!("installing bundled {id}: {e:#}")),
        }
    }
    if changed {
        let write = serde_json::to_vec_pretty(&record)
            .map_err(anyhow::Error::from)
            .and_then(|json| Ok(std::fs::write(&record_path, json)?));
        if let Err(e) = write {
            errors.push(format!("writing {}: {e:#}", record_path.display()));
        }
    }
    errors
}

/// Copy a plugin directory's files (not subdirectories) into `to`,
/// replacing what's there.
fn copy_plugin(from: &Path, to: &Path) -> Result<()> {
    let tmp = to.with_extension("installing");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            std::fs::copy(entry.path(), tmp.join(entry.file_name()))?;
        }
    }
    if to.exists() {
        std::fs::remove_dir_all(to)?;
    }
    std::fs::rename(&tmp, to)?;
    Ok(())
}

/// Install any new bundled plugins, then discover, register and start
/// every enabled plugin. Call before the user keymap loads (so it can
/// override plugin bindings) and before the menus are set.
pub fn init(settings: &Settings, cx: &mut App) {
    let mut host = PluginHost::default();
    let dest = dir();
    if let Some(bundled) = bundled_dir() {
        if let Err(e) = std::fs::create_dir_all(&dest) {
            host.problems
                .push(format!("creating {}: {e}", dest.display()));
        } else {
            host.problems.extend(install_bundled(&bundled, &dest));
        }
    }
    for found in sheaf_plugin::discover(&dest) {
        match found {
            Ok(f) => {
                let enabled = !settings.plugins.disabled.contains(&f.manifest.id);
                host.installed.push(Installed {
                    manifest: f.manifest,
                    dir: f.dir,
                    enabled,
                });
            }
            Err((dir, e)) => host.problems.push(format!("{}: {e:#}", dir.display())),
        }
    }
    if host.installed.iter().any(|i| i.enabled) {
        match Runtime::new(Limits::default()) {
            Ok(runtime) => {
                for i in host.installed.iter().filter(|i| i.enabled) {
                    let id = i.manifest.id.clone();
                    let errors = register(&i.manifest, cx);
                    host.problems
                        .extend(errors.into_iter().map(|e| format!("{id}: {e}")));
                    log::info!("loading plugin {id} {}", i.manifest.version);
                    let plugin = runtime.load(i.dir.clone(), i.manifest.clone());
                    host.plugins.insert(id, plugin);
                }
            }
            Err(e) => host.problems.push(format!("plugin runtime: {e:#}")),
        }
    }
    for p in &host.problems {
        log::warn!("plugins: {p}");
        eprintln!("plugins: {p}");
    }
    cx.set_global(host);
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
    for p in &m.panels {
        let id = format!("{}.{}", m.id, p.id);
        cx.global_mut::<Contributions>().add_panel(PluginPanel {
            id: id.clone(),
            plugin: m.id.clone(),
            label: p.label.clone(),
        });
        let shown = cx.global_mut::<CommandRegistry>().add_placed(
            Command {
                id: format!("{}::show-panel-{}", m.id, p.id),
                label: p.label.clone(),
                category: "Panels".into(),
                target: Target::Builtin(Box::new(ShowPluginPanel { panel: id })),
            },
            Some("View/Plugin Panels"),
        );
        if let Err(e) = shown {
            errors.push(e.to_string());
        }
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

/// Page text and pattern search from the document's render workers.
struct EngineText(HashMap<SourceId, RenderHandle>);

impl EngineText {
    fn handle(&self, source: SourceId) -> Result<&RenderHandle> {
        self.0.get(&source).context("source not open")
    }
}

impl TextSource for EngineText {
    fn page_text(&self, source: SourceId, index: u32) -> Result<String> {
        futures::executor::block_on(self.handle(source)?.page_text(index))
            .context("render thread is gone")?
    }

    fn pattern_search(
        &self,
        source: SourceId,
        index: u32,
        re: &Arc<regex::Regex>,
    ) -> Result<Vec<TextMatch>> {
        let rx = self.handle(source)?.pattern_search_page(index, re.clone());
        let hits = futures::executor::block_on(rx).context("render thread is gone")??;
        Ok(hits
            .into_iter()
            .map(|h| TextMatch {
                text: h.text,
                rects: h
                    .rects
                    .into_iter()
                    .map(|r| sheaf_doc::Box2 {
                        x: r.x,
                        y: r.y,
                        w: r.w,
                        h: r.h,
                    })
                    .collect(),
            })
            .collect())
    }
}

pub fn text_source(handles: &HashMap<SourceId, RenderHandle>) -> Arc<dyn TextSource> {
    Arc::new(EngineText(handles.clone()))
}

/// A `ui` call from a plugin's thread, for the workspace to carry out.
pub enum UiRequest {
    Form(Form, oneshot::Sender<Option<Vec<FieldAnswer>>>),
    Notice(String),
    /// Panel id within the plugin, and its rows.
    Panel(String, Vec<PanelRow>),
}

/// Forwards `ui` calls to the workspace that started the call.
struct AppUi(mpsc::UnboundedSender<UiRequest>);

impl Ui for AppUi {
    fn show_form(&self, form: Form) -> Option<Vec<FieldAnswer>> {
        let (tx, rx) = oneshot::channel();
        self.0.unbounded_send(UiRequest::Form(form, tx)).ok()?;
        futures::executor::block_on(rx).ok().flatten()
    }

    fn notice(&self, message: String) {
        let _ = self.0.unbounded_send(UiRequest::Notice(message));
    }

    fn set_panel(&self, panel: String, rows: Vec<PanelRow>) {
        let _ = self.0.unbounded_send(UiRequest::Panel(panel, rows));
    }
}

/// The plugin's display name, or its id.
pub fn plugin_name(id: &str, cx: &App) -> String {
    cx.try_global::<PluginHost>()
        .and_then(|h| h.installed.iter().find(|i| i.manifest.id == id))
        .map(|i| i.manifest.name.clone())
        .unwrap_or_else(|| id.to_string())
}

/// Run a plugin call against `doc` (the active document, if any): its UI
/// requests go to the workspace, and the edits it submits are applied when
/// it returns, one undo step per batch.
pub fn run(
    plugin_id: &str,
    call: Call,
    doc: Option<Entity<DocView>>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let notice = |doc: &Option<Entity<DocView>>, msg: String, cx: &mut App| {
        log::info!("{msg}");
        if let Some(doc) = doc {
            doc.update(cx, |d, cx| d.set_notice(msg, cx));
        }
    };
    let label = match &call {
        Call::Command { id, .. } => cx
            .global::<CommandRegistry>()
            .get(&format!("{plugin_id}::{id}"))
            .map(|c| crate::menus::parse_label(&c.label).0),
        Call::PanelAction { .. } => None,
    }
    .unwrap_or_else(|| plugin_name(plugin_id, cx));
    let Some(plugin) = cx
        .try_global::<PluginHost>()
        .and_then(|h| h.plugins.get(plugin_id))
    else {
        notice(
            &doc,
            format!("Plugin “{plugin_id}” isn't installed or is turned off"),
            cx,
        );
        return;
    };
    if let Call::Command { id, .. } = &call
        && cx
            .global::<CommandRegistry>()
            .get(&format!("{plugin_id}::{id}"))
            .is_none()
    {
        notice(
            &doc,
            format!("Plugin “{plugin_id}” has no command “{id}”"),
            cx,
        );
        return;
    }
    let (ui_tx, mut ui_rx) = mpsc::unbounded();
    let ctx = CallContext {
        doc: doc.as_ref().and_then(|d| d.read(cx).plugin_context()),
        ui: Some(Arc::new(AppUi(ui_tx))),
    };
    let what = call.to_string();
    let rx = plugin.call(call, ctx);
    if matches!(
        plugin.status(),
        sheaf_plugin::Status::Loading | sheaf_plugin::Status::Ready
    ) {
        notice(&doc, format!("{label}…"), cx);
    }

    // UI requests arrive while the call runs; the channel closes with it.
    let ui_doc = doc.clone();
    let source = plugin_id.to_string();
    cx.spawn_in(window, async move |ws, cx| {
        while let Some(req) = ui_rx.next().await {
            ws.update_in(cx, |ws, window, cx| {
                ws.plugin_ui(&source, ui_doc.clone(), req, window, cx)
            })
            .ok();
        }
    })
    .detach();

    let weak = doc.as_ref().map(|d| d.downgrade());
    let plugin_id = plugin_id.to_string();
    cx.spawn(async move |_, cx| {
        let result = rx
            .await
            .unwrap_or_else(|_| Err(PluginError::Unavailable("its thread stopped".into())));
        cx.update(|cx| {
            let doc = weak.and_then(|w| w.upgrade());
            match result {
                Ok(out) if out.batches.is_empty() => {
                    // Leave any notice the plugin set; just clear "Running…".
                    if let Some(d) = &doc {
                        d.update(cx, |d, cx| d.clear_notice_if(&format!("{label}…"), cx));
                    }
                }
                Ok(out) => match &doc {
                    Some(d) => d.update(cx, |d, cx| {
                        for b in out.batches {
                            d.apply_batch(b.edits, &b.label, cx);
                        }
                    }),
                    None => notice(&doc, format!("{label}: no document to edit"), cx),
                },
                Err(e) => {
                    log::warn!("{plugin_id} {what}: {e}");
                    notice(&doc, format!("{label}: {e}"), cx);
                }
            }
        });
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin(dir: &Path, id: &str, version: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("sheaf-plugin.toml"),
            format!("id = \"{id}\"\nname = \"{id}\"\nversion = \"{version}\"\napi = \"^0.1\"\n"),
        )
        .unwrap();
        std::fs::write(dir.join("plugin.wasm"), version).unwrap();
    }

    fn installed(dest: &Path, id: &str) -> Option<String> {
        std::fs::read_to_string(dest.join(id).join("plugin.wasm")).ok()
    }

    #[test]
    fn bundled_plugins_install_once_and_upgrade_only_untouched_copies() {
        let root = std::env::temp_dir().join(format!("sheaf-bundled-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (from, dest) = (root.join("ship"), root.join("plugins"));
        std::fs::create_dir_all(&dest).unwrap();
        plugin(&from.join("a"), "a", "1.0.0");
        plugin(&from.join("b"), "b", "1.0.0");
        plugin(&from.join("c"), "c", "1.0.0");
        // The user already has their own "c".
        plugin(&dest.join("c"), "c", "0.5.0");
        assert!(install_bundled(&from, &dest).is_empty());
        assert_eq!(installed(&dest, "a").as_deref(), Some("1.0.0"));
        assert_eq!(
            installed(&dest, "c").as_deref(),
            Some("0.5.0"),
            "user copy wins"
        );

        // The user deletes "a" and replaces "b"; sheaf ships 2.0 of all three.
        std::fs::remove_dir_all(dest.join("a")).unwrap();
        plugin(&dest.join("b"), "b", "1.5.0");
        for id in ["a", "b", "c"] {
            plugin(&from.join(id), id, "2.0.0");
        }
        assert!(install_bundled(&from, &dest).is_empty());
        assert_eq!(installed(&dest, "a"), None, "deleted stays deleted");
        assert_eq!(
            installed(&dest, "b").as_deref(),
            Some("1.5.0"),
            "replaced stays"
        );
        assert_eq!(installed(&dest, "c").as_deref(), Some("0.5.0"));

        // An untouched copy upgrades.
        plugin(&from.join("d"), "d", "1.0.0");
        install_bundled(&from, &dest);
        plugin(&from.join("d"), "d", "1.1.0");
        install_bundled(&from, &dest);
        assert_eq!(installed(&dest, "d").as_deref(), Some("1.1.0"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
