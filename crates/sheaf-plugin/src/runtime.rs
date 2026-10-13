//! The wasmtime side: one shared engine, one worker thread and `Store` per
//! plugin, and the host half of the WIT interfaces.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use futures::channel::oneshot;
use sheaf_doc::{Edit, PageId, Snapshot, SourceId};
use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder, Trap};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

use crate::convert;
use crate::manifest::{Manifest, WASM_NAME};

pub(crate) mod bindings {
    wasmtime::component::bindgen!({
        world: "plugin",
        path: "../../wit",
    });
}

use bindings::sheaf::plugin as api;

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
}

/// What a call can see.
#[derive(Clone, Default)]
pub struct CallContext {
    /// None when no document is open.
    pub doc: Option<DocContext>,
}

#[derive(Clone)]
pub struct DocContext {
    pub path: PathBuf,
    pub snapshot: Snapshot,
    /// In document order.
    pub selection: Vec<PageId>,
    pub current_page: Option<PageId>,
    pub text: Arc<dyn TextSource>,
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
        bindings::Plugin::add_to_linker::<_, HasSelf<_>>(&mut linker, |s| s)
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
    pub fn load(self: &Arc<Self>, dir: PathBuf, manifest: Manifest) -> Plugin {
        let (tx, rx) = mpsc::channel::<Request>();
        let status = Arc::new(Mutex::new(Status::Loading));
        let (rt, st, id) = (self.clone(), status.clone(), manifest.id.clone());
        let spawned = thread::Builder::new()
            .name(format!("plugin-{id}"))
            .spawn(move || worker(rt, dir, id, st, rx));
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
    command: String,
    args: Option<String>,
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
        let (reply, rx) = oneshot::channel();
        let req = Request {
            command: command.to_string(),
            args,
            ctx,
            reply,
        };
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

fn worker(
    rt: Arc<Runtime>,
    dir: PathBuf,
    id: String,
    status: Arc<Mutex<Status>>,
    rx: mpsc::Receiver<Request>,
) {
    let set = |s: Status| *status.lock().unwrap() = s;
    let mut inst = match Instance::start(&rt, &dir, &id) {
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
        let result = inst.run(&req.command, req.args.as_deref(), req.ctx);
        if let Err(PluginError::Trapped(m)) = &result {
            log::error!("plugin {id} trapped in {:?}: {m}", req.command);
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
    world: Option<bindings::Plugin>,
    timeout_ticks: u64,
    timeout: Duration,
}

impl Instance {
    fn start(rt: &Runtime, dir: &std::path::Path, id: &str) -> Result<Self> {
        let wasm = dir.join(WASM_NAME);
        let component = Component::from_file(&rt.engine, &wasm)
            .map_err(anyhow::Error::from)
            .with_context(|| format!("loading {}", wasm.display()))?;
        let wasi = WasiCtx::builder().inherit_stderr().build();
        let state = HostState {
            wasi,
            table: ResourceTable::new(),
            limits: StoreLimitsBuilder::new()
                .memory_size(rt.limits.memory_bytes)
                .build(),
            plugin: id.to_string(),
            call: None,
            batches: Vec::new(),
        };
        let mut store = Store::new(&rt.engine, state);
        store.limiter(|s| &mut s.limits);
        let timeout_ticks = (rt.limits.call_timeout.as_millis() / TICK.as_millis()).max(1) as u64;
        store.set_epoch_deadline(timeout_ticks);
        let world = bindings::Plugin::instantiate(&mut store, &component, &rt.linker)
            .map_err(anyhow::Error::from)
            .context("instantiating")?;
        store.set_epoch_deadline(timeout_ticks);
        world
            .call_activate(&mut store)
            .map_err(|e| anyhow!(describe_trap(&e, rt.limits.call_timeout)))
            .context("activate")?
            .map_err(|e| anyhow!("activate: {e}"))?;
        Ok(Self {
            store: Some(store),
            world: Some(world),
            timeout_ticks,
            timeout: rt.limits.call_timeout,
        })
    }

    fn unload(&mut self) {
        self.world = None;
        self.store = None;
    }

    fn run(
        &mut self,
        command: &str,
        args: Option<&str>,
        ctx: CallContext,
    ) -> Result<CallOutput, PluginError> {
        let (Some(store), Some(world)) = (&mut self.store, &self.world) else {
            return Err(PluginError::Unavailable("unloaded".into()));
        };
        store.data_mut().call = Some(ctx);
        store.data_mut().batches.clear();
        store.set_epoch_deadline(self.timeout_ticks);
        let result = world.call_run_command(&mut *store, command, args);
        let data = store.data_mut();
        data.call = None;
        let batches = std::mem::take(&mut data.batches);
        match result {
            Ok(Ok(())) => Ok(CallOutput { batches }),
            Ok(Err(msg)) => Err(PluginError::Failed(msg)),
            Err(e) => Err(PluginError::Trapped(describe_trap(&e, self.timeout))),
        }
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
    table: ResourceTable,
    limits: StoreLimits,
    plugin: String,
    call: Option<CallContext>,
    batches: Vec<EditBatch>,
}

impl HostState {
    fn doc(&self) -> Option<&DocContext> {
        self.call.as_ref()?.doc.as_ref()
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

impl api::types::Host for HostState {}

impl api::document::Host for HostState {
    fn path(&mut self) -> Option<String> {
        self.doc().map(|d| d.path.display().to_string())
    }

    fn pages(&mut self) -> Vec<api::types::PageInfo> {
        let Some(d) = self.doc() else {
            return Vec::new();
        };
        d.snapshot
            .state
            .pages
            .iter()
            .map(|p| convert::page_info(&d.snapshot, p))
            .collect()
    }

    fn outline(&mut self) -> Vec<api::types::OutlineItem> {
        let Some(d) = self.doc() else {
            return Vec::new();
        };
        d.snapshot
            .state
            .outline_flat()
            .into_iter()
            .map(|(depth, n)| convert::outline_item(depth, n))
            .collect()
    }

    fn annotations(&mut self, page: u64) -> Vec<api::types::Annot> {
        let Some(d) = self.doc() else {
            return Vec::new();
        };
        d.snapshot
            .state
            .annots_on(PageId(page))
            .map(|l| l.iter().map(convert::annot_to_wit).collect())
            .unwrap_or_default()
    }

    fn selection(&mut self) -> Vec<u64> {
        self.doc()
            .map(|d| d.selection.iter().map(|p| p.0).collect())
            .unwrap_or_default()
    }

    fn current_page(&mut self) -> Option<u64> {
        self.doc()?.current_page.map(|p| p.0)
    }
}

impl api::text::Host for HostState {
    fn page_text(&mut self, page: u64) -> Result<String, String> {
        let d = self.doc().ok_or("no document is open")?;
        let id = PageId(page);
        if let Some(layer) = d.snapshot.state.text_layer(id) {
            return Ok(layer.text());
        }
        let p = d
            .snapshot
            .state
            .pages
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| format!("no page {page}"))?;
        d.text
            .page_text(p.source, p.index)
            .map_err(|e| format!("{e:#}"))
    }
}

impl api::edits::Host for HostState {
    fn submit(&mut self, label: String, edits: Vec<api::types::Edit>) {
        let edits = edits.into_iter().map(convert::edit_from_wit).collect();
        self.batches.push(EditBatch { label, edits });
    }
}

impl api::log::Host for HostState {
    fn log(&mut self, level: api::log::Level, message: String) {
        use api::log::Level;
        let level = match level {
            Level::Trace => log::Level::Trace,
            Level::Debug => log::Level::Debug,
            Level::Info => log::Level::Info,
            Level::Warn => log::Level::Warn,
            Level::Error => log::Level::Error,
        };
        log::log!(target: "plugin", level, "[{}] {message}", self.plugin);
    }
}
