//! The wasmtime side: one shared engine, one worker thread and `Store` per
//! plugin, and the host half of the WIT interfaces.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow};
use futures::channel::oneshot;
use sheaf_doc::{Edit, PageId, Snapshot, SourceId};
use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder, Trap, UpdateDeadline};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

use crate::api;
use crate::manifest::{Grants, Manifest, WASM_NAME};
use crate::storage::Storage;
use crate::ui::Ui;

/// How often the epoch advances; timeouts are measured in these.
const TICK: Duration = Duration::from_millis(10);

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Longest a single call (activate or a command) may run.
    pub call_timeout: Duration,
    /// Most linear memory a plugin may grow to.
    pub memory_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            call_timeout: Duration::from_secs(30),
            memory_bytes: 256 << 20,
        }
    }
}

/// Reads a page's text from the engine, for pages without an unsaved OCR
/// layer. The app implements it over its render workers.
pub trait TextSource: Send + Sync {
    fn page_text(&self, source: SourceId, index: u32) -> Result<String>;
    /// Every match of `re` on the page, with boxes in the page's displayed
    /// space: its source page as the file displays it, which is the API's
    /// page coordinates.
    fn pattern_search(
        &self,
        source: SourceId,
        index: u32,
        re: &Arc<regex::Regex>,
    ) -> Result<Vec<TextMatch>>;
    /// Every occurrence of `query`, like Find.
    fn search(
        &self,
        source: SourceId,
        index: u32,
        query: &str,
        match_case: bool,
    ) -> Result<Vec<TextMatch>>;
    /// Boxes of the text under `rect`, merged per line.
    fn text_boxes(
        &self,
        source: SourceId,
        index: u32,
        rect: sheaf_doc::Box2,
    ) -> Result<Vec<sheaf_doc::Box2>>;
}

/// Runs OCR for plugins. The app implements it over `sheaf_ocr`.
pub trait OcrSource: Send + Sync {
    /// Recognize one page as it shows, `geom` (`Snapshot::geom`), rendered
    /// turned `render` on top of its source page's own `/Rotate`
    /// (`Snapshot::render_delta`).
    fn recognize(
        &self,
        page: &sheaf_doc::PageRef,
        geom: &sheaf_doc::geom::Geom,
        render: sheaf_core::Rotation,
        language: Option<String>,
        dpi: Option<u32>,
    ) -> Result<sheaf_doc::TextLayer>;
}

/// A document event, for plugins that subscribe to it.
#[derive(Debug, Clone, PartialEq)]
pub enum DocEvent {
    Opened,
    /// The undo label.
    Edited(String),
    /// The path saved to.
    Saved(String),
    SelectionChanged,
    Closed,
    /// A region on a page, in PageSpace.
    Region {
        page: PageId,
        rect: sheaf_doc::Box2,
    },
    RegionCleared,
}

impl DocEvent {
    pub fn kind(&self) -> crate::manifest::EventKind {
        use crate::manifest::EventKind;
        match self {
            DocEvent::Opened => EventKind::Opened,
            DocEvent::Edited(_) => EventKind::Edited,
            DocEvent::Saved(_) => EventKind::Saved,
            DocEvent::SelectionChanged => EventKind::SelectionChanged,
            DocEvent::Closed => EventKind::Closed,
            DocEvent::Region { .. } | DocEvent::RegionCleared => EventKind::Region,
        }
    }
}

/// How to start a plugin beyond its manifest.
#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    /// Where its `storage` lives; None keeps it in memory.
    pub data_dir: Option<PathBuf>,
    /// The permissions the user granted (a subset of the manifest's).
    pub grants: Grants,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextMatch {
    pub text: String,
    pub rects: Vec<sheaf_doc::Box2>,
    /// A pattern's capture groups 1…n; empty for a literal search.
    pub groups: Vec<Option<String>>,
}

/// A pattern-search preset, for `patterns.presets`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preset {
    pub name: String,
    pub regex: String,
}

/// What a call can see and reach.
#[derive(Clone, Default)]
pub struct CallContext {
    /// None when no document is open.
    pub doc: Option<DocContext>,
    /// Where `ui` calls go. None: forms are cancelled, notices logged,
    /// panel updates dropped.
    pub ui: Option<Arc<dyn Ui>>,
    /// The user's pattern presets, then plugin-shipped ones.
    pub presets: Vec<Preset>,
    /// Where the plugin's `progress` tasks report, and how the call is
    /// cancelled. None: tasks report nowhere and are never cancelled.
    pub progress: Option<in_progress::Tracker>,
    /// The plugin's section of the user's `settings.toml`, as JSON (API
    /// 0.3's `storage.preferences`), taken when the call starts so it
    /// stays the same throughout. Empty for none.
    pub preferences: String,
}

/// One entry point into a plugin.
#[derive(Debug, Clone, PartialEq)]
pub enum Call {
    Command {
        id: String,
        args: Option<String>,
    },
    PanelAction {
        panel: String,
        row: String,
        action: String,
    },
    Event(DocEvent),
}

impl std::fmt::Display for Call {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Call::Command { id, .. } => write!(f, "command {id:?}"),
            Call::PanelAction { panel, action, .. } => {
                write!(f, "action {action:?} in panel {panel:?}")
            }
            Call::Event(e) => write!(f, "event {e:?}"),
        }
    }
}

#[derive(Clone)]
pub struct DocContext {
    pub path: PathBuf,
    pub snapshot: Snapshot,
    /// In document order.
    pub selection: Vec<PageId>,
    pub current_page: Option<PageId>,
    pub text: Arc<dyn TextSource>,
    /// None: `ocr.recognize` reports that OCR isn't available.
    pub ocr: Option<Arc<dyn OcrSource>>,
    /// None: `render` reports that rendering isn't available.
    pub render: Option<Arc<dyn crate::render::RenderSource>>,
}

/// Edits one `edits.submit` asked for: one undo step.
#[derive(Debug, Clone, PartialEq)]
pub struct EditBatch {
    pub label: String,
    pub edits: Vec<Edit>,
}

#[derive(Debug, Default)]
pub struct CallOutput {
    /// In submission order. Applied by the app after the call returns.
    pub batches: Vec<EditBatch>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginError {
    /// The plugin returned an error; it stays loaded.
    Failed(String),
    /// The plugin trapped (crashed, ran out of time or memory) and has been
    /// unloaded.
    Trapped(String),
    /// The plugin isn't running: it failed to load or trapped earlier.
    Unavailable(String),
}

impl std::fmt::Display for PluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PluginError::Failed(m) => write!(f, "{m}"),
            PluginError::Trapped(m) => write!(f, "crashed and was unloaded: {m}"),
            PluginError::Unavailable(m) => write!(f, "not running: {m}"),
        }
    }
}

impl std::error::Error for PluginError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Loading,
    Ready,
    /// Failed to load, or trapped; the reason.
    Failed(String),
}

/// The shared engine. Create one per process.
pub struct Runtime {
    engine: Engine,
    linker: Linker<HostState>,
    limits: Limits,
    /// The epoch ticker runs while this is alive.
    _ticker: Arc<()>,
}

impl Runtime {
    pub fn new(limits: Limits) -> Result<Arc<Self>> {
        let mut config = Config::new();
        config.wasm_component_model(true).epoch_interruption(true);
        let engine = Engine::new(&config).map_err(anyhow::Error::from)?;
        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker).map_err(anyhow::Error::from)?;
        wasmtime_wasi_http::p2::add_only_http_to_linker_sync(&mut linker)
            .map_err(anyhow::Error::from)?;
        // Both worlds: a plugin links to the one it was built for.
        api::v03::World::add_to_linker::<_, HasSelf<_>>(&mut linker, |s| s)
            .map_err(anyhow::Error::from)?;
        api::v02::World::add_to_linker::<_, HasSelf<_>>(&mut linker, |s| s)
            .map_err(anyhow::Error::from)?;
        let token = Arc::new(());
        let alive = Arc::downgrade(&token);
        let ticking = engine.clone();
        thread::Builder::new()
            .name("plugin-epoch".into())
            .spawn(move || {
                while alive.upgrade().is_some() {
                    thread::sleep(TICK);
                    ticking.increment_epoch();
                }
            })?;
        Ok(Arc::new(Self {
            engine,
            linker,
            limits,
            _ticker: token,
        }))
    }

    /// Start a plugin: its thread compiles and instantiates `plugin.wasm`
    /// from `dir` and calls `activate`. Returns at once; calls made before
    /// loading finishes wait for it.
    pub fn load(self: &Arc<Self>, dir: PathBuf, manifest: Manifest, opts: LoadOptions) -> Plugin {
        let (tx, rx) = mpsc::channel::<Request>();
        let status = Arc::new(Mutex::new(Status::Loading));
        let (rt, st, id) = (self.clone(), status.clone(), manifest.id.clone());
        let panels = manifest.panels.iter().map(|p| p.id.clone()).collect();
        let v02 = !manifest.api.matches(&crate::API_VERSION);
        let spawned = thread::Builder::new()
            .name(format!("plugin-{id}"))
            .spawn(move || worker(rt, dir, id, panels, v02, opts, st, rx));
        if let Err(e) = spawned {
            *status.lock().unwrap() = Status::Failed(format!("starting its thread: {e}"));
        }
        Plugin {
            manifest,
            tx,
            status,
        }
    }
}

struct Request {
    call: Call,
    ctx: CallContext,
    reply: oneshot::Sender<Result<CallOutput, PluginError>>,
}

/// A loaded (or loading) plugin. Dropping it stops its thread once queued
/// calls finish.
pub struct Plugin {
    manifest: Manifest,
    tx: mpsc::Sender<Request>,
    status: Arc<Mutex<Status>>,
}

impl Plugin {
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    /// Queue a command. The reply arrives when it finishes.
    pub fn run_command(
        &self,
        command: &str,
        args: Option<String>,
        ctx: CallContext,
    ) -> oneshot::Receiver<Result<CallOutput, PluginError>> {
        self.call(
            Call::Command {
                id: command.to_string(),
                args,
            },
            ctx,
        )
    }

    /// Queue a call. The reply arrives when it finishes.
    pub fn call(
        &self,
        call: Call,
        ctx: CallContext,
    ) -> oneshot::Receiver<Result<CallOutput, PluginError>> {
        let (reply, rx) = oneshot::channel();
        let req = Request { call, ctx, reply };
        if let Err(mpsc::SendError(req)) = self.tx.send(req) {
            let why = match self.status() {
                Status::Failed(m) => m,
                _ => "its thread has stopped".into(),
            };
            let _ = req.reply.send(Err(PluginError::Unavailable(why)));
        }
        rx
    }
}

#[allow(clippy::too_many_arguments)]
fn worker(
    rt: Arc<Runtime>,
    dir: PathBuf,
    id: String,
    panels: Vec<String>,
    v02: bool,
    opts: LoadOptions,
    status: Arc<Mutex<Status>>,
    rx: mpsc::Receiver<Request>,
) {
    let set = |s: Status| *status.lock().unwrap() = s;
    let mut inst = match Instance::start(&rt, &dir, &id, panels, v02, opts) {
        Ok(i) => {
            set(Status::Ready);
            i
        }
        Err(e) => {
            let msg = format!("{e:#}");
            log::error!("plugin {id}: {msg}");
            set(Status::Failed(msg.clone()));
            for req in rx {
                let _ = req.reply.send(Err(PluginError::Unavailable(msg.clone())));
            }
            return;
        }
    };
    let mut dead: Option<String> = None;
    for req in rx {
        if let Some(why) = &dead {
            let _ = req.reply.send(Err(PluginError::Unavailable(why.clone())));
            continue;
        }
        let result = inst.run(&req.call, req.ctx);
        if let Err(PluginError::Trapped(m)) = &result {
            log::error!("plugin {id} trapped in {}: {m}", req.call);
            set(Status::Failed(m.clone()));
            dead = Some(m.clone());
        }
        let _ = req.reply.send(result);
        if dead.is_some() {
            // Free the instance's memory now rather than at shutdown.
            inst.unload();
        }
    }
}

struct Instance {
    store: Option<Store<HostState>>,
    world: Option<World>,
    timeout: Duration,
}

/// A plugin's instance, in the API it was built for.
enum World {
    V03(api::v03::World),
    V02(api::v02::World),
}

impl World {
    fn activate(&self, store: &mut Store<HostState>) -> wasmtime::Result<Result<(), String>> {
        match self {
            World::V03(w) => w.call_activate(store),
            World::V02(w) => w.call_activate(store),
        }
    }

    fn run(
        &self,
        store: &mut Store<HostState>,
        call: &Call,
    ) -> wasmtime::Result<Result<(), String>> {
        match (self, call) {
            (World::V03(w), Call::Command { id, args }) => {
                w.call_run_command(store, id, args.as_deref())
            }
            (World::V02(w), Call::Command { id, args }) => {
                w.call_run_command(store, id, args.as_deref())
            }
            (World::V03(w), Call::PanelAction { panel, row, action }) => {
                w.call_panel_action(store, panel, row, action)
            }
            (World::V02(w), Call::PanelAction { panel, row, action }) => {
                w.call_panel_action(store, panel, row, action)
            }
            (World::V03(w), Call::Event(e)) => {
                let e = api::v03::event_to_wit(store.data().doc().map(|d| &d.snapshot), e);
                w.call_on_event(store, &e)
            }
            (World::V02(w), Call::Event(e)) => {
                let e = api::v02::event_to_wit(store.data().doc().map(|d| &d.snapshot), e);
                w.call_on_event(store, &e)
            }
        }
    }
}

impl Instance {
    fn start(
        rt: &Runtime,
        dir: &std::path::Path,
        id: &str,
        panels: Vec<String>,
        v02: bool,
        opts: LoadOptions,
    ) -> Result<Self> {
        let wasm = dir.join(WASM_NAME);
        let component = Component::from_file(&rt.engine, &wasm)
            .map_err(anyhow::Error::from)
            .with_context(|| format!("loading {}", wasm.display()))?;
        let wasi = wasi_ctx(&opts.grants)?;
        let credit = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let state = HostState {
            wasi,
            table: ResourceTable::new(),
            limits: StoreLimitsBuilder::new()
                .memory_size(rt.limits.memory_bytes)
                .build(),
            plugin: id.to_string(),
            panels,
            storage: Storage::new(opts.data_dir),
            http: wasmtime_wasi_http::WasiHttpCtx::new(),
            net: NetHooks {
                grants: opts.grants.clone(),
                credit: credit.clone(),
            },
            credit,
            render_handles: Default::default(),
            task_handles: Vec::new(),
            call: None,
            batches: Vec::new(),
            deadline: Instant::now() + rt.limits.call_timeout,
        };
        let mut store = Store::new(&rt.engine, state);
        store.limiter(|s| &mut s.limits);
        // Checked every tick against a wall-clock deadline, which host calls
        // that wait for the user push back.
        store.epoch_deadline_callback(|cx| {
            let d = cx.data();
            let waited = Duration::from_nanos(d.credit.load(std::sync::atomic::Ordering::Relaxed));
            Ok(if Instant::now() < d.deadline + waited {
                UpdateDeadline::Continue(1)
            } else {
                UpdateDeadline::Interrupt
            })
        });
        store.set_epoch_deadline(1);
        let world = if v02 {
            api::v02::World::instantiate(&mut store, &component, &rt.linker).map(World::V02)
        } else {
            api::v03::World::instantiate(&mut store, &component, &rt.linker).map(World::V03)
        }
        .map_err(anyhow::Error::from)
        .context("instantiating")?;
        store.data_mut().deadline = Instant::now() + rt.limits.call_timeout;
        store.set_epoch_deadline(1);
        world
            .activate(&mut store)
            .map_err(|e| anyhow!(describe_trap(&e, rt.limits.call_timeout)))
            .context("activate")?
            .map_err(|e| anyhow!("activate: {e}"))?;
        Ok(Self {
            store: Some(store),
            world: Some(world),
            timeout: rt.limits.call_timeout,
        })
    }

    fn unload(&mut self) {
        self.world = None;
        self.store = None;
    }

    fn run(&mut self, call: &Call, ctx: CallContext) -> Result<CallOutput, PluginError> {
        let (Some(store), Some(world)) = (&mut self.store, &self.world) else {
            return Err(PluginError::Unavailable("unloaded".into()));
        };
        let data = store.data_mut();
        data.call = Some(ctx);
        data.batches.clear();
        data.deadline = Instant::now() + self.timeout;
        data.credit.store(0, std::sync::atomic::Ordering::Relaxed);
        store.set_epoch_deadline(1);
        let result = world.run(store, call);
        let data = store.data_mut();
        data.drop_render_handles();
        data.drop_task_handles();
        data.call = None;
        let batches = std::mem::take(&mut data.batches);
        match result {
            Ok(Ok(())) => Ok(CallOutput { batches }),
            Ok(Err(msg)) => Err(PluginError::Failed(msg)),
            Err(e) => Err(PluginError::Trapped(describe_trap(&e, self.timeout))),
        }
    }
}

/// WASI with only what the user granted: the listed directories (seen by
/// the plugin at their own paths, with `/` separators). No sockets: the
/// network is HTTP only, through `wasi:http` and [`NetHooks`]. Stderr goes
/// to ours, for panic messages.
fn wasi_ctx(grants: &Grants) -> Result<WasiCtx> {
    use wasmtime_wasi::{FsPerms, WasiCtxBuilder};
    let mut b = WasiCtxBuilder::new();
    b.inherit_stderr();
    for (dirs, perms) in [
        (&grants.read, FsPerms::ReadOnly),
        (&grants.write, FsPerms::ReadWrite),
    ] {
        for d in dirs {
            if !d.is_dir() {
                log::warn!("plugin permission: {} isn't a directory", d.display());
                continue;
            }
            let guest = d.to_string_lossy().replace('\\', "/");
            b.preopened_dir(d, guest, perms)
                .map_err(anyhow::Error::from)
                .with_context(|| format!("opening {}", d.display()))?;
        }
    }
    Ok(b.build())
}

/// `wasi:http` for a plugin: requests only to the hosts it was granted,
/// sent by the host (which does TLS). Time waiting on a response is added
/// to `credit`, which the call's time limit doesn't count.
pub(crate) struct NetHooks {
    grants: Grants,
    credit: Arc<std::sync::atomic::AtomicU64>,
}

impl wasmtime_wasi_http::WasiHttpHooks for NetHooks {
    fn send_request(
        &mut self,
        request: http::Request<wasmtime_wasi_http::WasiBody>,
        options: Option<wasmtime_wasi_http::RequestOptions>,
        fut: Box<dyn std::future::Future<Output = Result<(), wasmtime_wasi_http::Error>> + Send>,
    ) -> Box<
        dyn std::future::Future<
                Output = Result<
                    (
                        http::Response<wasmtime_wasi_http::WasiBody>,
                        Box<
                            dyn std::future::Future<Output = Result<(), wasmtime_wasi_http::Error>>
                                + Send,
                        >,
                    ),
                    wasmtime_wasi_http::Error,
                >,
            > + Send,
    > {
        _ = fut;
        let uri = request.uri();
        let host = uri.host().unwrap_or("").to_string();
        let allowed = self.grants.allows_host(&host, uri.port_u16());
        if !allowed {
            log::warn!(target: "plugin", "HTTP request to {host} denied");
        }
        let credit = self.credit.clone();
        Box::new(async move {
            use http_body_util::BodyExt;
            if !allowed {
                return Err(wasmtime_wasi_http::Error::HttpRequestDenied);
            }
            let started = Instant::now();
            let sent = wasmtime_wasi_http::default_send_request(request, options).await;
            credit.fetch_add(
                started.elapsed().as_nanos() as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
            let (res, io) = sent?;
            Ok((
                res.map(BodyExt::boxed_unsync),
                Box::new(io) as Box<dyn std::future::Future<Output = _> + Send>,
            ))
        })
    }
}

fn describe_trap(e: &wasmtime::Error, timeout: Duration) -> String {
    match e.downcast_ref::<Trap>() {
        Some(Trap::Interrupt) => format!("took longer than {timeout:?}"),
        Some(t) => t.to_string(),
        None => format!("{e:#}"),
    }
}

/// Per-plugin data the host functions reach.
pub(crate) struct HostState {
    wasi: WasiCtx,
    pub(crate) table: ResourceTable,
    limits: StoreLimits,
    pub(crate) plugin: String,
    /// Panel ids the manifest declares.
    pub(crate) panels: Vec<String>,
    pub(crate) storage: Storage,
    http: wasmtime_wasi_http::WasiHttpCtx,
    net: NetHooks,
    /// Nanoseconds this call spent waiting on the network (not charged).
    credit: Arc<std::sync::atomic::AtomicU64>,
    /// Render handles this call made, freed when it returns.
    pub(crate) render_handles: crate::render::CallHandles,
    /// `progress.task` handles this call made, freed when it returns.
    pub(crate) task_handles: Vec<u32>,
    pub(crate) call: Option<CallContext>,
    pub(crate) batches: Vec<EditBatch>,
    /// When the running call times out.
    pub(crate) deadline: Instant,
}

impl HostState {
    pub(crate) fn doc(&self) -> Option<&DocContext> {
        self.call.as_ref()?.doc.as_ref()
    }

    pub(crate) fn progress(&self) -> Option<&in_progress::Tracker> {
        self.call.as_ref()?.progress.as_ref()
    }

    pub(crate) fn ui(&self) -> Option<Arc<dyn Ui>> {
        self.call.as_ref()?.ui.clone()
    }
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl wasmtime_wasi_http::WasiHttpView for HostState {
    fn http(&mut self) -> wasmtime_wasi_http::WasiHttpCtxView<'_> {
        wasmtime_wasi_http::WasiHttpCtxView {
            ctx: &mut self.http,
            table: &mut self.table,
            hooks: &mut self.net,
        }
    }
}

impl HostState {
    /// The page, for text calls.
    pub(crate) fn page_ref(&self, page: u64) -> Result<(&DocContext, sheaf_doc::PageRef), String> {
        let d = self.doc().ok_or("no document is open")?;
        let p = d
            .snapshot
            .state
            .pages
            .iter()
            .find(|p| p.id == PageId(page))
            .copied()
            .ok_or_else(|| format!("no page {page}"))?;
        Ok((d, p))
    }
}
