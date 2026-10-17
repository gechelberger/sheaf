//! Plugins in the app: discovery at startup, what their manifests
//! contribute (commands, menu items, key bindings, presets, marks, panels),
//! and running their calls against the active document with the UI they
//! ask for. The runtime itself lives in `sheaf-plugin`; see
//! `todos/plugins.md` for the design.
//!
//! Plugins live in `<config dir>/plugins/`, one directory per plugin
//! holding a `sheaf-plugin.toml`, a `plugin.wasm` and its `icons/`. The plugins that ship
//! with sheaf (the repo's `plugins/`, built by `scripts/build-plugins.sh`
//! into `plugins/` next to the executable) are installed there on first
//! run and are ordinary plugins from then on: the user can turn them off,
//! replace or remove them like any other. See [`install_bundled`].
//!
//! What's turned off and which permissions the user allowed are kept in
//! `<config dir>/plugins.json` ([`PluginState`]). A plugin asking for files
//! or the network runs without them until the user says yes.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
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
use sheaf_plugin::ui::{FieldAnswer, Form, PanelRow, ShowPanel, Ui};
use sheaf_plugin::{
    Call, CallContext, Limits, LoadOptions, Manifest, Permissions, Plugin, PluginError, Runtime,
    Status, TextMatch, TextSource,
};
use sheaf_render::RenderHandle;

use crate::availability::When;
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
    /// Region events, latest wins per plugin (see `dispatch_event`).
    regions: Latest<(gpui::WeakEntity<DocView>, sheaf_plugin::DocEvent)>,
    runtime: Option<Arc<Runtime>>,
}

impl Global for PluginHost {}

/// Where plugins are installed.
pub fn dir() -> PathBuf {
    sheaf_ops::dirs::config_dir().join("plugins")
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
            Some(v) => *v < f.manifest.version && Manifest::version_in(&target).as_ref() == Some(v),
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
    copy_tree(from, &tmp)?;
    if to.exists() {
        std::fs::remove_dir_all(to)?;
    }
    std::fs::rename(&tmp, to)?;
    Ok(())
}

/// Copy a plugin folder: its files and its `icons/` folder. Links and
/// other folders (a build's `target/`) are left behind.
fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_file() {
            std::fs::copy(entry.path(), to.join(entry.file_name()))?;
        } else if kind.is_dir() && entry.file_name() == "icons" {
            let icons = to.join("icons");
            std::fs::create_dir_all(&icons)?;
            for icon in std::fs::read_dir(entry.path())? {
                let icon = icon?;
                if icon.file_type()?.is_file() {
                    std::fs::copy(icon.path(), icons.join(icon.file_name()))?;
                }
            }
        }
    }
    Ok(())
}

/// `<config>/plugins.json`: which plugins are off and what each may reach.
/// Its own file, apart from the state sheaf remembers for itself.
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
        && want.network.iter().all(|h| have.network.contains(h))
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
        network: a
            .network
            .iter()
            .filter(|h| b.network.contains(h))
            .cloned()
            .collect(),
    }
}

fn is_empty(p: &Permissions) -> bool {
    p.read.is_empty() && p.write.is_empty() && p.network.is_empty()
}

impl PluginState {
    fn path() -> PathBuf {
        sheaf_ops::dirs::config_dir().join("plugins.json")
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
    sheaf_ops::dirs::config_dir().join("plugin-data").join(id)
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

    fn dir(&self, id: &str) -> Option<PathBuf> {
        self.installed
            .iter()
            .find(|i| i.manifest.id == id)
            .map(|i| i.dir.clone())
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
    let Some(dir) = cx.global::<PluginHost>().dir(id) else {
        return;
    };
    let mut problems: Vec<String> = register(&m, &dir, cx)
        .into_iter()
        .map(|e| format!("{id}: {e}"))
        .collect();
    // A setting in its section of settings.toml that it doesn't name.
    if let Some(section) = crate::prefs::get(cx).plugins.get(id) {
        for key in m.unknown_preferences(section.keys()) {
            problems.push(format!(
                "{id}: settings.toml [plugins.{id}] has `{key}`, which it doesn't read (it reads {})",
                m.preferences.join(", ")
            ));
        }
    }
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
    crate::assets::remove_plugin_icons(id);
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

/// Install a `.sheafplugin` file: a zip holding `sheaf-plugin.toml`,
/// `plugin.wasm` and an `icons/` folder (at its root or in one folder).
/// Returns the plugin's id.
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
/// where anything is written. Icons are the `.svg` files in a folder
/// named `icons`, and land in `icons/`.
fn extract_archive(path: &Path, to: &Path) -> Result<()> {
    const WANTED: &[&str] = &["sheaf-plugin.toml", "plugin.wasm", "README.md", "LICENSE"];
    const MAX_BYTES: u64 = 64 << 20;
    const MAX_FILES: usize = 512;
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut zip = zip::ZipArchive::new(file).context("not a plugin archive (zip)")?;
    std::fs::create_dir_all(to)?;
    let mut seen: Vec<String> = Vec::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        if !entry.is_file() {
            continue;
        }
        let path = entry.name()?.to_string();
        let mut parts = path.rsplit(['/', '\\']);
        let base = parts.next().unwrap_or_default().to_string();
        let in_icons = parts.next() == Some("icons");
        let name = if in_icons && is_icon_name(&base) {
            format!("icons/{base}")
        } else if WANTED.contains(&base.as_str()) {
            base
        } else {
            continue;
        };
        anyhow::ensure!(entry.size() <= MAX_BYTES, "{name} is too large");
        anyhow::ensure!(
            !seen.contains(&name),
            "the archive has more than one {name}"
        );
        seen.push(name.clone());
        anyhow::ensure!(seen.len() <= MAX_FILES, "the archive has too many files");
        if in_icons {
            std::fs::create_dir_all(to.join("icons"))?;
        }
        let mut out = std::fs::File::create(to.join(&name))?;
        std::io::copy(&mut (&mut entry).take(MAX_BYTES), &mut out)?;
    }
    Ok(())
}

/// A plain `.svg` file name: no path, nothing hidden.
fn is_icon_name(name: &str) -> bool {
    name.len() > ".svg".len()
        && name.ends_with(".svg")
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
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
            log::info!("plugin {id}: asking for permission");
            let answer = cx.update(|window, cx| {
                sheaf_ui::prompts::ask(
                    window,
                    cx,
                    gpui::PromptLevel::Info,
                    &format!("The “{name}” plugin wants to:"),
                    Some(&detail),
                    &[
                        gpui::PromptButton::new("Allow"),
                        gpui::PromptButton::cancel("Don't Allow"),
                    ],
                )
            });
            let Ok(rx) = answer else { return };
            let answer = rx.await;
            log::info!("plugin {id}: permission answer {answer:?}");
            let allow = matches!(answer, Ok(0));
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
        E::Region { page, rect } => (
            P::Region {
                page: *page,
                rect: *rect,
            },
            None,
        ),
        E::RegionCleared => (P::RegionCleared, None),
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
    let calls: Vec<(String, Call)> = if kind == sheaf_plugin::EventKind::Opened {
        // `opened` first, then the calls that waited for the file.
        doc.update(cx, |d, _| d.release_plugin_calls(to))
    } else {
        to.into_iter()
            .map(|id| (id, Call::Event(event.clone())))
            .collect()
    };
    for (id, call) in calls {
        // Live while dragging: a plugin still busy with one region gets
        // only the newest next, not every mouse move.
        if let Call::Event(region) = &call
            && is_region(&call)
            && cx
                .global_mut::<PluginHost>()
                .regions
                .offer(&id, (doc.downgrade(), region.clone()))
                .is_none()
        {
            continue;
        }
        run(&id, call, Some(doc.clone()), window, cx);
    }
}

/// Whether `call` is a region event (a region, or its clearing), which
/// the host's `regions` coalesces.
fn is_region(call: &Call) -> bool {
    matches!(call, Call::Event(e) if e.kind() == sheaf_plugin::EventKind::Region)
}

/// Add a manifest's static contributions, reading command icons from the
/// plugin's folder `dir`. Returns one message per item that couldn't be
/// added; the rest still are.
fn register(m: &Manifest, dir: &Path, cx: &mut App) -> Vec<String> {
    let mut errors = Vec::new();
    let mut bindings = Vec::new();
    let workspace = KeyBindingContextPredicate::parse(crate::workspace::KEY_CONTEXT)
        .ok()
        .map(Rc::new);
    for c in &m.commands {
        let icon = match m.icon(dir, c) {
            Ok(svg) => crate::assets::add_plugin_icon(&m.id, &c.id, svg),
            Err(e) => {
                errors.push(format!("command {:?}: {e:#}", c.id));
                continue;
            }
        };
        let added = cx.global_mut::<CommandRegistry>().add_placed(
            Command {
                id: format!("{}::{}", m.id, c.id),
                label: c.label.clone(),
                category: m.category(c).to_string(),
                icon,
                help: c.help.clone(),
                keywords: c.keywords.clone(),
                when: c.when.iter().filter_map(|w| When::parse(w)).collect(),
                enabled: None,
                checked: None,
                target: Target::plugin(&m.id, &c.id),
            },
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
            let context = match &c.keys_context {
                Some(text) => match KeyBindingContextPredicate::parse(text) {
                    Ok(p) => Some(Rc::new(p)),
                    Err(e) => {
                        errors.push(format!("command {:?}: keys-context {text:?}: {e}", c.id));
                        continue;
                    }
                },
                None => workspace.clone(),
            };
            match KeyBinding::load(
                keys,
                action,
                context,
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
                icon: crate::command_info::icon_path("right-dock"),
                help: format!("Show the {} panel from {}.", p.label, m.name),
                keywords: Vec::new(),
                when: Vec::new(),
                enabled: None,
                checked: None,
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
                groups: h.groups,
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
/// user's engine and options as they were when the plugin started.
struct EngineOcr(HashMap<SourceId, RenderHandle>, sheaf_ocr::OcrConfig);

impl sheaf_plugin::OcrSource for EngineOcr {
    fn recognize(
        &self,
        page: &sheaf_doc::PageRef,
        geom: &sheaf_doc::geom::Geom,
        render: sheaf_core::Rotation,
        language: Option<String>,
        dpi: Option<u32>,
    ) -> Result<sheaf_doc::TextLayer> {
        use sheaf_ocr::{OcrEvent, OcrPage, OcrRequest, PageSource};
        let mut config = self.1.clone();
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
                geom: *geom,
                render,
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

/// Page images for plugins, from the render workers at the lowest priority
/// (behind on-screen tiles).
struct EngineRender(HashMap<SourceId, RenderHandle>);

impl sheaf_plugin::render::RenderSource for EngineRender {
    fn render(
        &self,
        source: SourceId,
        index: u32,
        page_w: u32,
        page_h: u32,
        (x, y, w, h): (u32, u32, u32, u32),
    ) -> Result<Vec<u8>> {
        use sheaf_image::raster::PixelFormat;
        use sheaf_render::{Job, RenderRequest};
        let handle = self.0.get(&source).context("source not open")?;
        let rx = handle.render(RenderRequest {
            page: sheaf_core::PageIndex(index),
            job: Job::Tile {
                page_width: page_w,
                page_height: page_h,
                x,
                y,
                w,
                h,
                rotation: sheaf_core::Rotation::R0,
            },
            format: PixelFormat::Rgba,
            priority: u8::MAX,
        });
        let rendered = futures::executor::block_on(rx).context("render thread is gone")??;
        let r = rendered.raster;
        anyhow::ensure!(
            r.width == w && r.height == h,
            "rendered {}×{}, wanted {w}×{h}",
            r.width,
            r.height
        );
        Ok(r.data)
    }
}

pub fn render_source(
    handles: &HashMap<SourceId, RenderHandle>,
) -> Arc<dyn sheaf_plugin::render::RenderSource> {
    Arc::new(EngineRender(handles.clone()))
}

/// Marks a plugin adds without an author are the user's.
/// Who the user is, and the date and time now, as dynamic stamps write
/// them: what a plugin's marks are signed and filled in with.
struct Signer {
    name: String,
    initials: String,
    date: String,
    time: String,
}

impl Signer {
    fn now(prefs: &sheaf_ops::prefs::Prefs) -> Signer {
        let now = chrono::Local::now();
        Signer {
            name: prefs.identity.name(),
            initials: prefs.identity.initials(),
            date: now.format(prefs.stamps.date_format()).to_string(),
            time: now.format(prefs.stamps.time_format()).to_string(),
        }
    }

    fn fill(&self, text: &mut String) {
        if text.contains("<<") {
            *text =
                sheaf_ops::stamps::fill(text, &self.name, &self.initials, &self.date, &self.time);
        }
    }
}

/// A plugin's marks with no author are the user's, and `<<name>>`,
/// `<<initials>>`, `<<date>>` and `<<time>>` in their text are filled in,
/// as dynamic stamps are: the plugin never sees who the user is.
fn sign_marks(edits: &mut [sheaf_doc::Edit], me: &Signer) {
    let sign = |a: &mut sheaf_doc::Annot| {
        if a.author.is_empty() {
            a.author = me.name.clone();
        }
        me.fill(&mut a.contents);
        if let sheaf_doc::AnnotKind::TextStamp { text, .. } = &mut a.kind {
            me.fill(text);
        }
    };
    for e in edits {
        match e {
            sheaf_doc::Edit::AnnotAdd { annot, .. }
            | sheaf_doc::Edit::AnnotUpdate { annot, .. } => sign(annot),
            sheaf_doc::Edit::ConnectorAdd { a, b } => {
                sign(&mut a.1);
                sign(&mut b.1);
            }
            _ => {}
        }
    }
}

/// A plugin rewrites a mark from what its API shows it. Every plugin's
/// update keeps the mark's creation date, name, turn and a tape's rows and
/// look, which no API shows; a 0.2 plugin's (`api02`) keeps its fill,
/// dashes, cloud and start arrowhead too, which 0.2 can't say.
fn keep_unseen(edits: &mut [sheaf_doc::Edit], st: &sheaf_doc::DocState, api02: bool) {
    for e in edits {
        let sheaf_doc::Edit::AnnotUpdate { page, annot } = e else {
            continue;
        };
        let Some(was) = st
            .annots_on(*page)
            .and_then(|l| l.iter().find(|a| a.id == annot.id))
        else {
            continue;
        };
        annot.name = was.name.clone();
        annot.created = was.created.clone();
        annot.turn = was.turn;
        if api02 {
            annot.fill = was.fill;
            annot.dashed = was.dashed;
            annot.cloudy = was.cloudy;
        }
        if api02
            && let (
                sheaf_doc::AnnotKind::Line { arrow_start, .. },
                sheaf_doc::AnnotKind::Line {
                    arrow_start: before,
                    ..
                },
            ) = (&mut annot.kind, &was.kind)
        {
            *arrow_start = *before;
        }
        // A tape's look; and its rows, unless the plugin changed its text.
        if let (
            sheaf_doc::AnnotKind::Tape { tape, look, .. },
            sheaf_doc::AnnotKind::Tape {
                tape: tape_before,
                look: look_before,
                ..
            },
        ) = (&mut annot.kind, &was.kind)
        {
            *look = look_before.clone();
            if annot.contents == was.contents {
                *tape = tape_before.clone();
            }
        }
    }
}

pub fn ocr_source(
    handles: &HashMap<SourceId, RenderHandle>,
    config: sheaf_ocr::OcrConfig,
) -> Arc<dyn sheaf_plugin::OcrSource> {
    Arc::new(EngineOcr(handles.clone(), config))
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
    ShowPanel(ShowPanel),
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

    fn show_panel(&self, panel: ShowPanel) {
        let _ = self.0.unbounded_send(UiRequest::ShowPanel(panel));
    }
}

/// Pattern presets for `patterns.presets`: the user's, then plugins'.
fn presets(cx: &App) -> Vec<sheaf_plugin::Preset> {
    let mut out: Vec<sheaf_plugin::Preset> = crate::patterns::list(cx)
        .into_iter()
        .map(|p| sheaf_plugin::Preset {
            name: p.name,
            regex: p.regex,
        })
        .collect();
    out.extend(
        cx.global::<Contributions>()
            .plugin_presets()
            .iter()
            .map(|p| sheaf_plugin::Preset {
                name: p.preset.name.clone(),
                regex: p.preset.regex.clone(),
            }),
    );
    out
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
/// it returns, one undo step per batch. While `doc` is still opening, the
/// call waits for it (see [`Waiting`]).
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
            .map(|c| {
                sheaf_ui::menu_bar::parse_label(&c.label)
                    .0
                    .trim_end_matches('…')
                    .to_string()
            }),
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
    if let Some(d) = &doc
        && d.read(cx).is_opening()
    {
        if matches!(call, Call::Event(sheaf_plugin::DocEvent::Closed)) {
            // Closed before it opened: what waited goes with the view, and
            // `closed` only follows an `opened` sent before (a recovery
            // opens the file again).
            if !d.read(cx).plugin_calls.opened() {
                return;
            }
        } else {
            if !matches!(call, Call::Event(_)) {
                notice(&doc, format!("{label} runs once the file is open"), cx);
            }
            let region = is_region(&call);
            d.update(cx, |d, _| d.plugin_calls.hold(plugin_id, call));
            // Held, not running: the plugin is free for the next region,
            // here or in another document.
            if region {
                region_done(plugin_id, window, cx);
            }
            return;
        }
    }
    let (ui_tx, mut ui_rx) = mpsc::unbounded();
    // Commands the user ran report progress to the document's status bar,
    // and Escape there stops them.
    let tracker = matches!(call, Call::Command { .. } | Call::PanelAction { .. })
        .then(sheaf_ops::Tracker::new);
    let ctx = CallContext {
        doc: doc
            .as_ref()
            .and_then(|d| d.read(cx).plugin_context(crate::prefs::ocr_config(cx))),
        ui: Some(Arc::new(AppUi(ui_tx))),
        presets: presets(cx),
        progress: tracker.clone(),
        // Its own settings, as they are now, for the whole call.
        preferences: crate::prefs::get(cx).plugin_json(plugin_id),
    };
    let what = call.to_string();
    let region_call = is_region(&call);
    let call_kind = match &call {
        Call::Event(_) => CallKind::Event,
        _ => CallKind::User,
    };
    let rx = plugin.call(call, ctx);
    let quiet = matches!(call_kind, CallKind::Event);
    if !quiet && matches!(plugin.status(), Status::Loading | Status::Ready) {
        notice(&doc, format!("{label}…"), cx);
    }
    if let (Some(d), Some(t)) = (&doc, &tracker) {
        let (label, t) = (label.clone(), t.clone());
        d.update(cx, |d, _| d.plugin_run = Some((label, t)));
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
    cx.spawn_in(window, async move |ws, cx| {
        use futures::FutureExt;
        let mut rx = rx.fuse();
        // The last progress line shown, to clear when the call ends: by
        // then its tasks are gone, and the snapshot no longer makes it.
        // The final snapshot still makes "stopping…", which Escape may
        // have put up since the last tick, so that goes too.
        let mut shown: Option<String> = None;
        let result = loop {
            let mut tick = cx
                .background_executor()
                .timer(std::time::Duration::from_millis(150))
                .fuse();
            futures::select_biased! {
                r = rx => break r,
                _ = tick => {}
            }
            if let (Some(t), Some(d)) = (&tracker, weak.as_ref().and_then(|w| w.upgrade())) {
                let line = progress_line(&label, &t.snapshot());
                if let Some(line) = line {
                    d.update(cx, |d, cx| d.set_notice(line.clone(), cx));
                    shown = Some(line);
                }
            }
        }
        .unwrap_or_else(|_| Err(PluginError::Unavailable("its thread stopped".into())));
        ws.update_in(cx, |_, window, cx| {
            let doc = weak.and_then(|w| w.upgrade());
            if let Some(d) = &doc {
                d.update(cx, |d, cx| {
                    d.plugin_run = None;
                    // The last progress line goes with the run.
                    let now = tracker
                        .as_ref()
                        .and_then(|t| progress_line(&label, &t.snapshot()));
                    for line in shown.iter().chain(&now) {
                        d.clear_notice_if(line, cx);
                    }
                });
            }
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
                        // A message the plugin left outlasts the undo label
                        // applying shows, but not a failure or a warning.
                        let running = format!("{label}…");
                        let left = d.notice().filter(|n| *n != running).map(str::to_string);
                        let mut last = None;
                        d.edit_source = Some(plugin_id.clone());
                        let me = Signer::now(&crate::prefs::get(cx));
                        // A 0.2 plugin can't see the newer fields, so an
                        // update keeps them.
                        let api02 = cx
                            .global::<PluginHost>()
                            .manifest(&plugin_id)
                            .is_some_and(|m| !m.api.matches(&sheaf_plugin::API_VERSION));
                        for mut b in out.batches {
                            sign_marks(&mut b.edits, &me);
                            if let crate::docview::State::Ready(doc) = &d.state {
                                keep_unseen(&mut b.edits, doc.model.state(), api02);
                            }
                            d.apply_batch(b.edits, &b.label, cx);
                            last = Some(b.label);
                        }
                        d.edit_source = None;
                        if let Some(msg) = left
                            && d.notice() == last.as_deref()
                        {
                            d.set_notice(msg, cx);
                        }
                    }),
                    None => notice(&doc, format!("{label}: no document to edit"), cx),
                },
                Err(e) => {
                    log::warn!("{plugin_id} {what}: {e}");
                    notice(&doc, format!("{label}: {e}"), cx);
                }
            }
            if region_call {
                region_done(&plugin_id, window, cx);
            }
        })
        .ok();
    })
    .detach();
}

/// The status line for a running command's progress, once it has any.
fn progress_line(label: &str, s: &sheaf_ops::ProgressSnapshot) -> Option<String> {
    if s.cancelled {
        return Some(format!("{label}: stopping…"));
    }
    if let Some((_, msg)) = &s.message {
        return Some(format!("{label}: {msg}"));
    }
    let total = s.total?;
    let step = if s.step.is_empty() { label } else { &s.step };
    Some(format!(
        "{step}: {} of {total} {}…",
        s.done.min(total),
        s.unit.noun()
    ))
}

/// A plugin finished a region call: send it the newest region event that
/// arrived meanwhile, if any.
fn region_done(plugin: &str, window: &mut Window, cx: &mut Context<Workspace>) {
    let Some((doc, event)) = cx.global_mut::<PluginHost>().regions.done(plugin) else {
        return;
    };
    match doc.upgrade() {
        Some(doc) => run(plugin, Call::Event(event), Some(doc), window, cx),
        // The document is gone: nothing more to send.
        None => {
            cx.global_mut::<PluginHost>().regions.done(plugin);
        }
    }
}

/// A document's plugin calls while it opens (M10.5 proposal 28). A
/// command, a panel action or an event given before the file's facts are
/// read waits here; once they are, `opened` goes out first and the calls
/// follow in the order given, each made from the document as it is then.
#[derive(Default)]
pub struct Waiting {
    calls: Vec<(String, Call)>,
    /// `opened` has gone out for the view (a recovery opens it again).
    opened: bool,
}

impl Waiting {
    /// Keep `call` for `plugin` until the document is ready. A region
    /// event takes the place of the one the plugin had waiting, as the
    /// newest does while it's busy.
    pub fn hold(&mut self, plugin: &str, call: Call) {
        if is_region(&call) {
            self.calls.retain(|(p, c)| p != plugin || !is_region(c));
        }
        self.calls.push((plugin.to_string(), call));
    }

    /// The calls to make now, by plugin: none while `doc` is still
    /// opening; once it's ready, `opened` to each of `subscribers`, then
    /// every call held, in order.
    pub fn release(
        &mut self,
        doc: &sheaf_doc::Document,
        subscribers: Vec<String>,
    ) -> Vec<(String, Call)> {
        if !doc.is_ready() {
            return Vec::new();
        }
        self.opened = true;
        subscribers
            .into_iter()
            .map(|id| (id, Call::Event(sheaf_plugin::DocEvent::Opened)))
            .chain(std::mem::take(&mut self.calls))
            .collect()
    }

    /// Whether `opened` has gone out for the view.
    pub fn opened(&self) -> bool {
        self.opened
    }
}

/// Per-key "latest wins" queue: while a key is busy, newer items replace
/// the one waiting instead of piling up.
pub struct Latest<T> {
    busy: HashSet<String>,
    waiting: HashMap<String, T>,
}

impl<T> Default for Latest<T> {
    fn default() -> Self {
        Self {
            busy: HashSet::new(),
            waiting: HashMap::new(),
        }
    }
}

impl<T> Latest<T> {
    /// Offer an item: Some(item) to send now (the key is now busy), or
    /// None if it's waiting behind a call in progress.
    pub fn offer(&mut self, key: &str, item: T) -> Option<T> {
        if self.busy.contains(key) {
            self.waiting.insert(key.to_string(), item);
            None
        } else {
            self.busy.insert(key.to_string());
            Some(item)
        }
    }

    /// The key's call finished: the waiting item to send next (the key
    /// stays busy), or None (the key is free).
    pub fn done(&mut self, key: &str) -> Option<T> {
        match self.waiting.remove(key) {
            Some(item) => Some(item),
            None => {
                self.busy.remove(key);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin(dir: &Path, id: &str, version: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("sheaf-plugin.toml"),
            format!("id = \"{id}\"\nname = \"{id}\"\nversion = \"{version}\"\napi = \"^0.2\"\n"),
        )
        .unwrap();
        std::fs::write(dir.join("plugin.wasm"), version).unwrap();
    }

    fn installed(dest: &Path, id: &str) -> Option<String> {
        std::fs::read_to_string(dest.join(id).join("plugin.wasm")).ok()
    }

    fn manifest(perms: &str) -> Manifest {
        Manifest::parse(&format!(
            "id = \"p\"\nname = \"P\"\nversion = \"1.0.0\"\napi = \"^0.2\"\n[permissions]\n{perms}"
        ))
        .unwrap()
    }

    fn command(id: &str) -> Call {
        Call::Command {
            id: id.into(),
            args: None,
        }
    }

    /// A one-page document, still opening.
    fn opening() -> sheaf_doc::Document {
        let store = sheaf_doc::Store::new();
        let primary = sheaf_doc::SourceInfo::blank(&store, 1);
        sheaf_doc::open::start(store, primary, sheaf_doc::history::default_policy())
    }

    #[test]
    fn nothing_leaves_the_queue_before_ready() {
        let mut doc = opening();
        let mut waiting = Waiting::default();
        let held = [
            ("tax".to_string(), command("scan")),
            (
                "tax".to_string(),
                Call::PanelAction {
                    panel: "found".into(),
                    row: "1".into(),
                    action: "go".into(),
                },
            ),
            (
                "notes".to_string(),
                Call::Event(sheaf_plugin::DocEvent::SelectionChanged),
            ),
        ];
        for (plugin, call) in held.clone() {
            waiting.hold(&plugin, call);
            assert!(waiting.release(&doc, vec!["tax".into()]).is_empty());
        }
        doc.ready(Default::default()).unwrap();
        // All of them waited, and none was lost.
        let out = waiting.release(&doc, Vec::new());
        assert_eq!(out, held);
        assert!(waiting.release(&doc, Vec::new()).is_empty());
    }

    #[test]
    fn a_queued_call_runs_after_opened() {
        let mut doc = opening();
        let mut waiting = Waiting::default();
        waiting.hold("tax", command("scan"));
        waiting.hold("notes", command("list"));
        waiting.hold("tax", command("tick"));
        doc.ready(Default::default()).unwrap();
        let opened = || Call::Event(sheaf_plugin::DocEvent::Opened);
        assert_eq!(
            waiting.release(&doc, vec!["notes".into(), "tax".into()]),
            [
                ("notes".into(), opened()),
                ("tax".into(), opened()),
                ("tax".into(), command("scan")),
                ("notes".into(), command("list")),
                ("tax".into(), command("tick")),
            ]
        );
    }

    /// `opened` goes out with the facts: while the view's open is reading
    /// them, and once it has read them but not yet taken them in, calls
    /// wait and nothing is released; once it has, `opened` leads, and the
    /// document the calls are made from holds the file's mark and field.
    #[test]
    fn opened_goes_out_with_the_facts() {
        use sheaf_doc::{SourceInfo, Store};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("facts.pdf");
        std::fs::write(&path, sheaf_doc::forms::fixture::every_fact().bytes()).unwrap();
        let store = Store::new();
        let primary = SourceInfo::read(&store, &path, None).unwrap();
        let (src, blob) = (primary.src.clone(), primary.src.id());
        let mut doc = crate::docview::opening_model(store, primary, "Me".into());
        let mut waiting = Waiting::default();
        let wants = || vec!["notes".to_string()];
        waiting.hold("tax", command("scan"));
        assert!(waiting.release(&doc, wants()).is_empty(), "reading");
        let facts = sheaf_doc::open::finish(&src, &sheaf_ops::NoProgress);
        waiting.hold("tax", command("tick"));
        assert!(waiting.release(&doc, wants()).is_empty(), "read, not taken");
        assert!(!waiting.opened());
        crate::docview::take_facts(&mut doc, blob, facts, false).unwrap();
        let opened = Call::Event(sheaf_plugin::DocEvent::Opened);
        assert_eq!(
            waiting.release(&doc, wants()),
            [
                ("notes".into(), opened),
                ("tax".into(), command("scan")),
                ("tax".into(), command("tick")),
            ]
        );
        assert!(waiting.opened(), "a close now says `closed`");
        // What a plugin's context is made from when `opened` goes out.
        let seen = doc.snapshot();
        assert_eq!(
            seen.state.annots.values().map(|l| l.len()).sum::<usize>(),
            1
        );
        assert_eq!(seen.state.fields.len(), 1);
    }

    /// While the file opens, a plugin keeps only its newest region event,
    /// as it would while busy with one; other calls, and other plugins'
    /// regions, keep their places.
    #[test]
    fn a_held_region_gives_way_to_the_newest() {
        use sheaf_plugin::DocEvent;
        let mut doc = opening();
        let region = |x: f32| {
            Call::Event(DocEvent::Region {
                page: doc.page(0).unwrap().id,
                rect: sheaf_doc::Box2 {
                    x,
                    y: 0.0,
                    w: 10.0,
                    h: 10.0,
                },
            })
        };
        let (first, second, theirs) = (region(1.0), region(2.0), region(3.0));
        let mut waiting = Waiting::default();
        waiting.hold("tax", first);
        waiting.hold("tax", command("scan"));
        waiting.hold("sum", theirs.clone());
        waiting.hold("tax", Call::Event(DocEvent::RegionCleared));
        waiting.hold("tax", second.clone());
        doc.ready(Default::default()).unwrap();
        assert_eq!(
            waiting.release(&doc, Vec::new()),
            [
                ("tax".into(), command("scan")),
                ("sum".into(), theirs),
                ("tax".into(), second),
            ]
        );
    }

    #[test]
    fn latest_wins_while_busy() {
        let mut q = Latest::default();
        assert_eq!(q.offer("p", 1), Some(1), "idle: send now");
        assert_eq!(q.offer("p", 2), None, "busy: wait");
        assert_eq!(q.offer("p", 3), None, "replaces 2");
        assert_eq!(q.offer("other", 9), Some(9), "keys are independent");
        assert_eq!(q.done("p"), Some(3), "only the newest");
        assert_eq!(q.done("p"), None, "now idle");
        assert_eq!(q.offer("p", 4), Some(4));
    }

    #[test]
    fn consent_covers_what_was_asked_and_asks_again_for_more() {
        let mut st = PluginState::default();
        let none = manifest("");
        assert!(!st.needs_consent(&none), "nothing to ask");
        let m = manifest("read = [\"~/data\"]\nnetwork = [\"api.example.com\"]");
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
        let more = manifest("read = [\"~/data\", \"~/other\"]");
        assert!(st.needs_consent(&more));
        assert_eq!(
            st.granted(&more),
            Permissions {
                read: vec!["~/data".into()],
                write: vec![],
                network: vec![],
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
                ("p/icons/scan.svg", "<svg/>"),
                ("p/icons/.hidden.svg", "no"),
                ("p/notes/icons.svg", "no"),
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
        assert_eq!(
            names,
            ["README.md", "icons", "plugin.wasm", "sheaf-plugin.toml"]
        );
        let icons: Vec<_> = std::fs::read_dir(out.join("icons"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(icons, ["scan.svg"]);
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
    fn copies_take_the_icons_folder_and_nothing_else() {
        let root = std::env::temp_dir().join(format!("sheaf-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let from = root.join("src");
        plugin(&from, "p", "1.0.0");
        std::fs::create_dir_all(from.join("icons")).unwrap();
        std::fs::write(from.join("icons/scan.svg"), "<svg/>").unwrap();
        std::fs::create_dir_all(from.join("target/debug")).unwrap();
        std::fs::write(from.join("target/debug/big"), "x").unwrap();
        let to = root.join("dest");
        copy_plugin(&from, &to).unwrap();
        assert_eq!(
            std::fs::read_to_string(to.join("icons/scan.svg")).unwrap(),
            "<svg/>"
        );
        assert!(to.join("plugin.wasm").is_file());
        assert!(!to.join("target").exists());
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

        // A copy built for an older API no longer loads, but still upgrades.
        plugin(&from.join("e"), "e", "1.0.0");
        install_bundled(&from, &dest);
        let old = dest.join("e").join("sheaf-plugin.toml");
        let text = std::fs::read_to_string(&old).unwrap();
        std::fs::write(&old, text.replace("^0.2", "^0.1")).unwrap();
        assert!(Manifest::load(&dest.join("e")).is_err());
        plugin(&from.join("e"), "e", "1.1.0");
        assert!(install_bundled(&from, &dest).is_empty());
        assert_eq!(installed(&dest, "e").as_deref(), Some("1.1.0"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_plugin_update_keeps_what_it_cant_see() {
        use sheaf_doc::{
            Annot, AnnotId, AnnotKind, Document, Edit, FixedCount, Pt, SourceInfo, Store,
        };
        let store = Store::new();
        let primary = SourceInfo::blank(&store, 1);
        let mut d = Document::new(store, primary, &[], Box::new(FixedCount(10)));
        let page = d.page(0).unwrap().id;
        let line = Annot {
            id: AnnotId(1),
            kind: AnnotKind::Line {
                a: Pt { x: 0.0, y: 0.0 },
                b: Pt { x: 10.0, y: 0.0 },
                arrow: true,
                arrow_start: true,
            },
            color: [0, 0, 0],
            opacity: 1.0,
            stroke: 1.0,
            author: "me".into(),
            contents: String::new(),
            modified: "D:1".into(),
            created: "D:0".into(),
            fill: None,
            dashed: true,
            cloudy: false,
            turn: sheaf_core::Rotation::R90,
            name: String::new(),
        };
        d.apply(Edit::AnnotAdd { page, annot: line }, "Line")
            .unwrap();
        let mut placed = d.state().annots_on(page).unwrap()[0].clone();
        // What a 0.2 plugin hands back: the styles it can't see are lost,
        // and it reads upright as the file shows the page.
        let mut back = placed.clone();
        back.contents = "checked".into();
        back.dashed = false;
        back.created = "D:2".into();
        back.turn = sheaf_core::Rotation::R0;
        if let AnnotKind::Line { arrow_start, .. } = &mut back.kind {
            *arrow_start = false;
        }
        let mut edits = vec![Edit::AnnotUpdate { page, annot: back }];
        keep_unseen(&mut edits, d.state(), true);
        let Edit::AnnotUpdate { annot, .. } = &edits[0] else {
            panic!()
        };
        placed.contents = "checked".into();
        placed.modified = annot.modified.clone();
        assert_eq!(*annot, placed);
        // A 0.3 plugin sees them, so what it says goes; but not the
        // creation date, which no plugin sets.
        let mut back = placed.clone();
        back.dashed = false;
        back.created = "D:2".into();
        back.turn = sheaf_core::Rotation::R0;
        let mut edits = vec![Edit::AnnotUpdate { page, annot: back }];
        keep_unseen(&mut edits, d.state(), false);
        let Edit::AnnotUpdate { annot, .. } = &edits[0] else {
            panic!()
        };
        assert!(!annot.dashed);
        assert_eq!(annot.created, "D:0");
        assert_eq!(annot.turn, sheaf_core::Rotation::R90);
    }

    #[test]
    fn a_plugins_text_is_signed_like_a_dynamic_stamp() {
        use sheaf_doc::{Annot, AnnotId, AnnotKind, Box2, Edit, PageId};
        let me = Signer {
            name: "Pat Q. Smith".into(),
            initials: "PQS".into(),
            date: "10/15/2026".into(),
            time: "9:00 AM".into(),
        };
        let stamp = Annot {
            id: AnnotId(0),
            kind: AnnotKind::TextStamp {
                rect: Box2 {
                    x: 0.0,
                    y: 0.0,
                    w: 9.0,
                    h: 9.0,
                },
                text: "REVIEWED\n<<initials>> <<date>>".into(),
                status: false,
            },
            color: [0, 0, 0],
            opacity: 1.0,
            stroke: 1.0,
            author: String::new(),
            contents: "Checked by <<name>> at <<time>>".into(),
            modified: String::new(),
            created: String::new(),
            fill: None,
            dashed: false,
            cloudy: false,
            turn: sheaf_core::Rotation::R0,
            name: String::new(),
        };
        let mut edits = vec![Edit::AnnotAdd {
            page: PageId(1),
            annot: stamp,
        }];
        sign_marks(&mut edits, &me);
        let Edit::AnnotAdd { annot, .. } = &edits[0] else {
            panic!()
        };
        assert_eq!(annot.author, "Pat Q. Smith");
        assert_eq!(annot.contents, "Checked by Pat Q. Smith at 9:00 AM");
        assert!(
            matches!(&annot.kind, AnnotKind::TextStamp { text, .. } if text == "REVIEWED\nPQS 10/15/2026")
        );
    }

    /// A step's line goes with it: the snapshot's step goes back to the
    /// call's tracker, which counts nothing. So the poll loop remembers the
    /// last line it showed, to clear it when the call ends. "Stopping…"
    /// needs no step, so the final snapshot still makes that one.
    #[test]
    fn a_finished_step_leaves_no_progress_line() {
        use sheaf_ops::{Progress, Tracker};
        let t = Tracker::new();
        let step = t.child("Counting", 1.0);
        step.set_total(Some(10), Default::default());
        step.set(3);
        assert_eq!(
            progress_line("Count", &t.snapshot()).as_deref(),
            Some("Counting: 3 of 10 items…")
        );
        drop(step);
        assert_eq!(progress_line("Count", &t.snapshot()), None);
        t.cancel();
        assert_eq!(
            progress_line("Count", &t.snapshot()).as_deref(),
            Some("Count: stopping…")
        );
    }
}
