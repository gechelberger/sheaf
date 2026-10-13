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
//! run and are ordinary plugins from then on: the user can turn them off,
//! replace or remove them like any other. See [`install_bundled`].
//!
//! What's turned off and which permissions the user allowed are kept in
//! `<config dir>/plugins.json` ([`PluginState`]). A plugin asking for files
//! or the network runs without them until the user says yes.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use futures::StreamExt;
use futures::channel::{mpsc, oneshot};
use gpui::{App, Context, Entity, Global, KeyBinding, KeyBindingContextPredicate, Window};
use serde::{Deserialize, Serialize};
use sheaf_doc::SourceId;
use sheaf_plugin::ui::{FieldAnswer, Form, PanelRow, Ui};
use sheaf_plugin::{
    Call, CallContext, Limits, LoadOptions, Manifest, Permissions, Plugin, PluginError, Runtime,
    Status, TextMatch, TextSource,
};
use sheaf_render::RenderHandle;

use crate::commands::{Command, CommandRegistry, Run, ShowPluginPanel, Target};
use crate::contrib::{Contributions, Marks, PluginPanel, TickSet};
use crate::docview::DocView;
use crate::patterns::Preset;
use crate::workspace::Workspace;

/// A plugin found on disk, running or not.
pub struct Installed {
    pub manifest: Manifest,
    pub dir: PathBuf,
}

/// Every plugin found, the running ones by id, and what went wrong.
#[derive(Default)]
pub struct PluginHost {
    pub installed: Vec<Installed>,
    plugins: HashMap<String, Plugin>,
    pub problems: Vec<String>,
    pub state: PluginState,
    /// Plugins whose permissions the user hasn't been asked about yet.
    pending_consent: Vec<String>,
    runtime: Option<Arc<Runtime>>,
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

/// `<config>/plugins.json`: which plugins are off and what each may reach.
/// Its own file so windows saving `settings.json` can't clobber it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginState {
    pub disabled: BTreeSet<String>,
    pub consent: BTreeMap<String, Consent>,
}

/// The user's answer about a plugin's permissions.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Consent {
    /// What the plugin asked for when the user answered; asking for more
    /// later asks again.
    pub asked: Permissions,
    pub allowed: Permissions,
}

fn covers(have: &Permissions, want: &Permissions) -> bool {
    want.read.iter().all(|d| have.read.contains(d))
        && want.write.iter().all(|d| have.write.contains(d))
        && (!want.network || have.network)
}

fn intersect(a: &Permissions, b: &Permissions) -> Permissions {
    Permissions {
        read: a
            .read
            .iter()
            .filter(|d| b.read.contains(d))
            .cloned()
            .collect(),
        write: a
            .write
            .iter()
            .filter(|d| b.write.contains(d))
            .cloned()
            .collect(),
        network: a.network && b.network,
    }
}

fn is_empty(p: &Permissions) -> bool {
    p.read.is_empty() && p.write.is_empty() && !p.network
}

impl PluginState {
    fn path() -> PathBuf {
        crate::settings::config_dir().join("plugins.json")
    }

    pub fn load() -> Self {
        std::fs::read(Self::path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save(&self) -> Result<()> {
        let path = Self::path();
        let dir = path.parent().context("no config dir")?;
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(".plugins.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// What `m` may use now: what it asks for that the user allowed.
    pub fn granted(&self, m: &Manifest) -> Permissions {
        match self.consent.get(&m.id) {
            Some(c) => intersect(&m.permissions, &c.allowed),
            None => Permissions::default(),
        }
    }

    /// Whether `m` asks for something the user hasn't answered.
    pub fn needs_consent(&self, m: &Manifest) -> bool {
        !is_empty(&m.permissions)
            && !self
                .consent
                .get(&m.id)
                .is_some_and(|c| covers(&c.asked, &m.permissions))
    }
}

/// Where a plugin's `storage` lives.
fn data_dir(id: &str) -> PathBuf {
    crate::settings::config_dir().join("plugin-data").join(id)
}

/// Install any new bundled plugins, then discover, register and start
/// every enabled plugin. Call before the user keymap loads (so it can
/// override plugin bindings) and before the menus are set.
pub fn init(cx: &mut App) {
    let mut host = PluginHost {
        state: PluginState::load(),
        ..PluginHost::default()
    };
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
            Ok(f) => host.installed.push(Installed {
                manifest: f.manifest,
                dir: f.dir,
            }),
            Err((dir, e)) => host.problems.push(format!("{}: {e:#}", dir.display())),
        }
    }
    let enabled: Vec<String> = host
        .installed
        .iter()
        .map(|i| i.manifest.id.clone())
        .filter(|id| !host.state.disabled.contains(id))
        .collect();
    cx.set_global(host);
    for id in enabled {
        start(&id, cx);
    }
    for p in &cx.global::<PluginHost>().problems {
        log::warn!("plugins: {p}");
        eprintln!("plugins: {p}");
    }
}

impl PluginHost {
    pub fn is_enabled(&self, id: &str) -> bool {
        !self.state.disabled.contains(id)
    }

    pub fn status(&self, id: &str) -> Option<Status> {
        self.plugins.get(id).map(|p| p.status())
    }

    fn manifest(&self, id: &str) -> Option<&Manifest> {
        self.installed
            .iter()
            .find(|i| i.manifest.id == id)
            .map(|i| &i.manifest)
    }

    fn runtime(&mut self) -> Result<Arc<Runtime>> {
        if self.runtime.is_none() {
            self.runtime = Some(Runtime::new(Limits::default())?);
        }
        Ok(self.runtime.clone().unwrap())
    }

    /// (Re)start a plugin's runtime instance, with what it's allowed now.
    fn load(&mut self, id: &str) -> Result<()> {
        let i = self
            .installed
            .iter()
            .find(|i| i.manifest.id == id)
            .context("not installed")?;
        let (dir, manifest) = (i.dir.clone(), i.manifest.clone());
        let opts = LoadOptions {
            data_dir: Some(data_dir(id)),
            grants: self.state.granted(&manifest).grants(),
        };
        let runtime = self.runtime()?;
        log::info!("loading plugin {id} {}", manifest.version);
        // Dropping the old instance (if any) stops it once queued calls end.
        self.plugins
            .insert(id.to_string(), runtime.load(dir, manifest, opts));
        Ok(())
    }
}

/// Register a plugin's contributions and start it.
fn start(id: &str, cx: &mut App) {
    let Some(m) = cx.global::<PluginHost>().manifest(id).cloned() else {
        return;
    };
    let mut problems: Vec<String> = register(&m, cx)
        .into_iter()
        .map(|e| format!("{id}: {e}"))
        .collect();
    let host = cx.global_mut::<PluginHost>();
    if let Err(e) = host.load(id) {
        problems.push(format!("{id}: {e:#}"));
    }
    if host.state.needs_consent(&m) && !host.pending_consent.iter().any(|p| p == id) {
        host.pending_consent.push(id.to_string());
    }
    host.problems.extend(problems);
}

/// Stop a plugin and take its contributions back out.
fn stop(id: &str, cx: &mut App) {
    let host = cx.global_mut::<PluginHost>();
    host.plugins.remove(id);
    host.pending_consent.retain(|p| p != id);
    cx.global_mut::<CommandRegistry>().remove_plugin(id);
    cx.global_mut::<Contributions>().remove_plugin(id);
}

fn save_state(cx: &mut App) {
    if let Err(e) = cx.global::<PluginHost>().state.save() {
        log::warn!("saving plugin state: {e:#}");
    }
}

/// Turn a plugin on or off now. Key bindings it added stay until restart
/// (and report that it's off).
pub fn set_enabled(id: &str, on: bool, cx: &mut App) {
    let host = cx.global_mut::<PluginHost>();
    let changed = if on {
        host.state.disabled.remove(id)
    } else {
        host.state.disabled.insert(id.to_string())
    };
    if !changed {
        return;
    }
    save_state(cx);
    if on {
        start(id, cx);
    } else {
        stop(id, cx);
    }
    crate::menus::refresh(cx);
}

/// Record the user's answer to a plugin's permissions and restart it with
/// them.
pub fn set_allowed(id: &str, allow: bool, cx: &mut App) {
    let host = cx.global_mut::<PluginHost>();
    let Some(m) = host.manifest(id).cloned() else {
        return;
    };
    let consent = host.state.consent.entry(id.to_string()).or_default();
    consent.asked = m.permissions.clone();
    consent.allowed = if allow {
        m.permissions.clone()
    } else {
        Permissions::default()
    };
    host.pending_consent.retain(|p| p != id);
    let running = host.plugins.contains_key(id);
    if running && let Err(e) = host.load(id) {
        host.problems.push(format!("{id}: {e:#}"));
    }
    save_state(cx);
}

/// Remove a plugin from disk. Its storage stays, in case it comes back.
pub fn uninstall(id: &str, cx: &mut App) -> Result<()> {
    stop(id, cx);
    let host = cx.global_mut::<PluginHost>();
    let at = host
        .installed
        .iter()
        .position(|i| i.manifest.id == id)
        .context("not installed")?;
    let dir = host.installed.remove(at).dir;
    host.state.disabled.remove(id);
    save_state(cx);
    crate::menus::refresh(cx);
    std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))
}

/// Install (or replace) the plugin in directory `src` and start it.
/// Returns its id.
pub fn install_dir(src: &Path, cx: &mut App) -> Result<String> {
    let manifest = Manifest::load(src)?;
    let wasm = src.join(sheaf_plugin::manifest::WASM_NAME);
    anyhow::ensure!(wasm.is_file(), "{} is missing", wasm.display());
    let id = manifest.id.clone();
    let target = dir().join(&id);
    if same_dir(src, &target) {
        anyhow::bail!("that is the installed copy");
    }
    std::fs::create_dir_all(dir())?;
    let was_running = cx.global::<PluginHost>().plugins.contains_key(&id);
    if was_running {
        stop(&id, cx);
    }
    copy_plugin(src, &target)?;
    let host = cx.global_mut::<PluginHost>();
    host.installed.retain(|i| i.manifest.id != id);
    host.installed.push(Installed {
        manifest,
        dir: target,
    });
    host.installed.sort_by(|a, b| {
        a.manifest
            .name
            .to_lowercase()
            .cmp(&b.manifest.name.to_lowercase())
    });
    if host.is_enabled(&id) {
        start(&id, cx);
    }
    crate::menus::refresh(cx);
    Ok(id)
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Install a `.sheafplugin` file: a zip holding `sheaf-plugin.toml` and
/// `plugin.wasm` (at its root or in one folder). Returns the plugin's id.
pub fn install_archive(path: &Path, cx: &mut App) -> Result<String> {
    let tmp = std::env::temp_dir().join(format!(
        "sheaf-plugin-install-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    extract_archive(path, &tmp)?;
    let result = install_dir(&tmp, cx);
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

/// Unpack the files a plugin needs, by name only: entry paths never pick
/// where anything is written.
fn extract_archive(path: &Path, to: &Path) -> Result<()> {
    const WANTED: &[&str] = &["sheaf-plugin.toml", "plugin.wasm", "README.md", "LICENSE"];
    const MAX_BYTES: u64 = 64 << 20;
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut zip = zip::ZipArchive::new(file).context("not a plugin archive (zip)")?;
    std::fs::create_dir_all(to)?;
    let mut seen: Vec<String> = Vec::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        if !entry.is_file() {
            continue;
        }
        let name = entry
            .name()?
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or_default()
            .to_string();
        if !WANTED.contains(&name.as_str()) {
            continue;
        }
        anyhow::ensure!(entry.size() <= MAX_BYTES, "{name} is too large");
        anyhow::ensure!(
            !seen.contains(&name),
            "the archive has more than one {name}"
        );
        seen.push(name.clone());
        let mut out = std::fs::File::create(to.join(&name))?;
        std::io::copy(&mut (&mut entry).take(MAX_BYTES), &mut out)?;
    }
    Ok(())
}

/// Ask about plugins whose permissions the user hasn't answered, one at a
/// time, in `window`. Called when a window opens; asks once per run.
pub fn ask_consent(window: &mut Window, cx: &mut Context<Workspace>) {
    let Some(host) = cx.try_global::<PluginHost>() else {
        return;
    };
    let pending: Vec<(String, String, Vec<String>)> = host
        .pending_consent
        .iter()
        .filter_map(|id| {
            let m = host.manifest(id)?;
            Some((
                id.clone(),
                m.name.clone(),
                m.permissions.grants().describe(),
            ))
        })
        .collect();
    if pending.is_empty() {
        return;
    }
    cx.global_mut::<PluginHost>().pending_consent.clear();
    cx.spawn_in(window, async move |_, cx| {
        for (id, name, lines) in pending {
            let detail = format!(
                "{}\n\nYou can change this later in Plugins → Manage Plugins.",
                lines.join("\n")
            );
            let answer = cx.update(|window, cx| {
                window.prompt(
                    gpui::PromptLevel::Info,
                    &format!("The “{name}” plugin wants to:"),
                    Some(&detail),
                    &["Allow", "Don't Allow"],
                    cx,
                )
            });
            let Ok(rx) = answer else { return };
            let allow = matches!(rx.await, Ok(0));
            cx.update(|_, cx| set_allowed(&id, allow, cx)).ok();
        }
    })
    .detach();
}

/// Send a document event to the plugins that subscribe to it.
pub fn dispatch_event(
    doc: &Entity<DocView>,
    event: &crate::docview::DocEvent,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    use crate::docview::DocEvent as E;
    use sheaf_plugin::DocEvent as P;
    let (event, from) = match event {
        E::Opened => (P::Opened, None),
        E::Edited { label, plugin, .. } => (P::Edited(label.clone()), plugin.clone()),
        E::Saved { path } => (P::Saved(path.display().to_string()), None),
        E::SelectionChanged => (P::SelectionChanged, None),
        E::Closed => (P::Closed, None),
    };
    let Some(host) = cx.try_global::<PluginHost>() else {
        return;
    };
    let kind = event.kind();
    let to: Vec<String> = host
        .plugins
        .iter()
        .filter(|(id, p)| {
            p.manifest().events.contains(&kind) && from.as_deref() != Some(id.as_str())
        })
        .map(|(id, _)| id.clone())
        .collect();
    for id in to {
        run(
            &id,
            Call::Event(event.clone()),
            Some(doc.clone()),
            window,
            cx,
        );
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
        Ok(Self::hits(hits))
    }

    fn search(
        &self,
        source: SourceId,
        index: u32,
        query: &str,
        match_case: bool,
    ) -> Result<Vec<TextMatch>> {
        let rx = self
            .handle(source)?
            .search_page(index, query.to_string(), match_case);
        let hits = futures::executor::block_on(rx).context("render thread is gone")??;
        Ok(Self::hits(hits))
    }

    fn text_boxes(
        &self,
        source: SourceId,
        index: u32,
        rect: sheaf_doc::Box2,
    ) -> Result<Vec<sheaf_doc::Box2>> {
        let rect = sheaf_render::Rect {
            x: rect.x,
            y: rect.y,
            w: rect.w,
            h: rect.h,
        };
        let rx = self.handle(source)?.text_boxes_in_rect(index, rect);
        let boxes = futures::executor::block_on(rx).context("render thread is gone")??;
        Ok(boxes.into_iter().map(to_box).collect())
    }
}

impl EngineText {
    fn hits(hits: Vec<sheaf_render::SearchHit>) -> Vec<TextMatch> {
        hits.into_iter()
            .map(|h| TextMatch {
                text: h.text,
                rects: h.rects.into_iter().map(to_box).collect(),
            })
            .collect()
    }
}

fn to_box(r: sheaf_render::Rect) -> sheaf_doc::Box2 {
    sheaf_doc::Box2 {
        x: r.x,
        y: r.y,
        w: r.w,
        h: r.h,
    }
}

/// OCR for plugins: one page at a time through `sheaf_ocr::job`, with the
/// user's engine and settings.
struct EngineOcr(HashMap<SourceId, RenderHandle>);

impl sheaf_plugin::OcrSource for EngineOcr {
    fn recognize(
        &self,
        page: &sheaf_doc::PageRef,
        size: sheaf_core::PageSize,
        language: Option<String>,
        dpi: Option<u32>,
    ) -> Result<sheaf_doc::TextLayer> {
        use sheaf_ocr::{OcrEvent, OcrPage, OcrRequest, PageSource};
        let mut config = crate::settings::Settings::load().ocr;
        if let Some(l) = language {
            sheaf_ocr::set_language(config.engine, &mut config, &l);
        }
        if let Some(d) = dpi {
            config.dpi = d;
        }
        let sources: HashMap<SourceId, Arc<dyn PageSource>> = self
            .0
            .iter()
            .map(|(sid, h)| (*sid, Arc::new(h.clone()) as Arc<dyn PageSource>))
            .collect();
        let req = OcrRequest {
            pages: vec![OcrPage {
                id: page.id,
                source: page.source,
                index: page.index,
                rotation: page.rotation,
                size,
            }],
            skip_pages_with_text: false,
            engine: config.engine,
            config,
        };
        let (_job, mut rx) = sheaf_ocr::job::start(req, sources);
        futures::executor::block_on(async {
            while let Some(ev) = rx.next().await {
                match ev {
                    OcrEvent::Page { layer, .. } => return Ok(layer),
                    OcrEvent::Failed { error, .. } => anyhow::bail!(error),
                    OcrEvent::Aborted(e) => anyhow::bail!(e),
                    OcrEvent::Finished { .. } => break,
                    _ => {}
                }
            }
            anyhow::bail!("OCR produced no result")
        })
    }
}

pub fn ocr_source(handles: &HashMap<SourceId, RenderHandle>) -> Arc<dyn sheaf_plugin::OcrSource> {
    Arc::new(EngineOcr(handles.clone()))
}

pub fn text_source(handles: &HashMap<SourceId, RenderHandle>) -> Arc<dyn TextSource> {
    Arc::new(EngineText(handles.clone()))
}

enum CallKind {
    /// The user asked for it: say it's running.
    User,
    /// A document event: work quietly unless it fails.
    Event,
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
        Call::PanelAction { .. } | Call::Event(_) => None,
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
    let call_kind = match &call {
        Call::Event(_) => CallKind::Event,
        _ => CallKind::User,
    };
    let rx = plugin.call(call, ctx);
    let quiet = matches!(call_kind, CallKind::Event);
    if !quiet && matches!(plugin.status(), Status::Loading | Status::Ready) {
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
                Ok(out) if out.batches.is_empty() && quiet => {}
                Ok(out) if out.batches.is_empty() => {
                    // Leave any notice the plugin set; just clear "Running…".
                    if let Some(d) = &doc {
                        d.update(cx, |d, cx| d.clear_notice_if(&format!("{label}…"), cx));
                    }
                }
                Ok(out) => match &doc {
                    Some(d) => d.update(cx, |d, cx| {
                        d.edit_source = Some(plugin_id.clone());
                        for b in out.batches {
                            d.apply_batch(b.edits, &b.label, cx);
                        }
                        d.edit_source = None;
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

    fn manifest(perms: &str) -> Manifest {
        Manifest::parse(&format!(
            "id = \"p\"\nname = \"P\"\nversion = \"1.0.0\"\napi = \"^0.1\"\n[permissions]\n{perms}"
        ))
        .unwrap()
    }

    #[test]
    fn consent_covers_what_was_asked_and_asks_again_for_more() {
        let mut st = PluginState::default();
        let none = manifest("");
        assert!(!st.needs_consent(&none), "nothing to ask");
        let m = manifest("read = [\"/data\"]\nnetwork = true");
        assert!(st.needs_consent(&m));
        assert_eq!(st.granted(&m), Permissions::default());
        st.consent.insert(
            "p".into(),
            Consent {
                asked: m.permissions.clone(),
                allowed: m.permissions.clone(),
            },
        );
        assert!(!st.needs_consent(&m));
        assert_eq!(st.granted(&m), m.permissions);
        // A new version wants more: ask again; meanwhile it gets only what
        // was allowed and it still asks for.
        let more = manifest("read = [\"/data\", \"/other\"]");
        assert!(st.needs_consent(&more));
        assert_eq!(
            st.granted(&more),
            Permissions {
                read: vec!["/data".into()],
                write: vec![],
                network: false,
            }
        );
        // "Don't allow" is an answer too.
        st.consent.insert(
            "p".into(),
            Consent {
                asked: more.permissions.clone(),
                allowed: Permissions::default(),
            },
        );
        assert!(!st.needs_consent(&more));
        assert_eq!(st.granted(&more), Permissions::default());
    }

    #[test]
    fn archives_extract_only_known_files_by_name() {
        use std::io::Write as _;
        let root = std::env::temp_dir().join(format!("sheaf-archive-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let zip_path = root.join("p.sheafplugin");
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for (name, body) in [
                ("p/sheaf-plugin.toml", "id = \"p\""),
                ("p/plugin.wasm", "wasm"),
                ("../../escape/README.md", "outside"),
                ("p/other.txt", "ignored"),
            ] {
                z.start_file(name, opts).unwrap();
                z.write_all(body.as_bytes()).unwrap();
            }
            z.finish().unwrap();
        }
        let out = root.join("out");
        extract_archive(&zip_path, &out).unwrap();
        let mut names: Vec<String> = std::fs::read_dir(&out)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["README.md", "plugin.wasm", "sheaf-plugin.toml"]);
        assert_eq!(
            std::fs::read_to_string(out.join("plugin.wasm")).unwrap(),
            "wasm"
        );
        assert!(!root.join("escape").exists());
        assert!(!std::env::temp_dir().join("escape").exists());

        // Two plugin.wasm entries: refuse rather than pick one.
        let dup = root.join("dup.sheafplugin");
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&dup).unwrap());
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for name in ["a/plugin.wasm", "b/plugin.wasm"] {
                z.start_file(name, opts).unwrap();
                z.write_all(b"x").unwrap();
            }
            z.finish().unwrap();
        }
        let err = extract_archive(&dup, &root.join("dup")).unwrap_err();
        assert!(err.to_string().contains("more than one plugin.wasm"));
        let _ = std::fs::remove_dir_all(&root);
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
