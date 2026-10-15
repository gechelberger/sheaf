//! What sheaf remembers for itself: window placement, docks, and each
//! dialog's last choices. Kept in `state.json` in the state folder
//! (`sheaf_ops::dirs`), local to the machine and disposable: a file that
//! doesn't parse is set aside as `state.json.bad` and sheaf starts fresh,
//! and deleting it loses no preference (those are in `settings.toml`; see
//! `crate::prefs`).
//!
//! The whole app shares one copy, in a GPUI global. Windows read it with
//! [`get`] and change it with [`update`], which applies to the current
//! value, so a window can't undo another's change by saving an old copy.
//! A background job takes the `Arc` when it starts and keeps it. Only the
//! process that owns the instance (`crate::instance`) writes the file, from
//! one writer thread, half a second after a burst of changes and once more
//! on quit.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use gpui::{App, Global};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct State {
    pub window: Option<WindowPlacement>,
    pub docks: Option<DockLayout>,
    /// OCR engine, resolution and last-used options (see `sheaf_ocr`).
    pub ocr: sheaf_ocr::OcrConfig,
    /// File > Print's last choices.
    pub print: crate::print_dialog::PrintPrefs,
    /// File > Create > From Scans/Photos' last choices.
    pub scan: sheaf_ops::scan::ScanOptions,
    /// File > Create > Combine Files' last choices.
    pub combine: sheaf_ops::combine::CombineParams,
    /// Document > Find Blank Pages' last choices.
    pub blanks: sheaf_ops::blank::BlankParams,
    /// Stamps placed lately, newest first (the Stamps panel's Recent).
    pub recent_stamps: Vec<sheaf_ops::stamps::StampRef>,
    /// Set Status's last choices, for Set Status Again.
    pub set_status: Option<crate::set_status::StatusSpec>,
    /// The comment CSV's columns, by name (`sheaf_ops::comments::Column`);
    /// none for the usual ones.
    pub comment_columns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WindowPlacement {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub maximized: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DockLayout {
    pub left: DockState,
    pub right: DockState,
    pub bottom: DockState,
    pub all_hidden: bool,
}

/// One dock: panel ids in tab order (see `Panel::id`), which is active,
/// whether it's open, and its width (sides) or height (bottom).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DockState {
    pub panels: Vec<String>,
    pub active: usize,
    pub open: bool,
    pub size: f32,
}

impl Default for DockState {
    fn default() -> Self {
        Self {
            panels: Vec::new(),
            active: 0,
            open: false,
            size: 240.0,
        }
    }
}

impl Default for DockLayout {
    fn default() -> Self {
        Self {
            left: DockState {
                panels: vec![
                    "pages".into(),
                    "bookmarks".into(),
                    "attachments".into(),
                    "layers".into(),
                ],
                active: 0,
                open: true,
                size: 220.0,
            },
            right: DockState {
                panels: vec!["comments".into(), "history".into(), "stamps".into()],
                active: 0,
                open: false,
                size: 300.0,
            },
            bottom: DockState {
                panels: vec!["patterns".into(), "calc".into()],
                active: 0,
                open: false,
                size: 240.0,
            },
            all_hidden: false,
        }
    }
}

pub const FILE: &str = "state.json";
/// What sheaf kept before M9, in the config folder; moved on first run.
const OLD_FILE: &str = "settings.json";
/// Changes closer together than this are written once.
const BATCH: Duration = Duration::from_millis(500);
/// How long quitting waits for the last write.
const FLUSH_WAIT: Duration = Duration::from_secs(2);

pub fn path() -> PathBuf {
    sheaf_ops::dirs::state_dir().join(FILE)
}

fn old_path() -> PathBuf {
    sheaf_ops::dirs::config_dir().join(OLD_FILE)
}

/// Read `path`. With no file yet, move `old` (the pre-M9 `settings.json`)
/// over. A file that doesn't parse is set aside. `writable`: whether this
/// process may change files (the owner).
fn load_from(path: &Path, old: &Path, writable: bool) -> State {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return migrate(old, path, writable).unwrap_or_default();
        }
        Err(e) => {
            log::warn!("reading {}: {e}", path.display());
            return State::default();
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("{}: {e}; starting fresh", path.display());
            if writable {
                let _ = std::fs::rename(path, path.with_extension("json.bad"));
            }
            State::default()
        }
    }
}

/// The old file's contents, written to `to` and the old file removed. An
/// old file that doesn't parse is left where it is.
fn migrate(old: &Path, to: &Path, writable: bool) -> Option<State> {
    let state: State = serde_json::from_slice(&std::fs::read(old).ok()?).ok()?;
    if writable {
        match write(to, &state) {
            Ok(()) => {
                let _ = std::fs::remove_file(old);
            }
            Err(e) => log::warn!("moving {} to {}: {e:#}", old.display(), to.display()),
        }
    }
    Some(state)
}

/// Replace `path` atomically: a uniquely named temp file beside it, then a
/// rename.
fn write(path: &Path, state: &State) -> Result<()> {
    let dir = path.parent().context("state path has no parent")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".state.json.")
        .suffix(".tmp")
        .tempfile_in(dir)
        .with_context(|| format!("creating a temp file in {}", dir.display()))?;
    serde_json::to_writer_pretty(&mut tmp, state)?;
    tmp.persist(path)
        .with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

/// Temp files left by a crash mid-write.
fn clear_temps(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(".state.json.") && name.ends_with(".tmp") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

enum Job {
    Write(Arc<State>),
    /// Write anything pending now, then answer.
    Flush(Sender<()>),
}

/// The one thread that writes `path`: each change is held for [`BATCH`]
/// so a burst (a dock drag) is written once, as its last value.
fn spawn_writer(path: PathBuf) -> Option<Sender<Job>> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("sheaf-state".into())
        .spawn(move || writer(rx, &path))
        .map_err(|e| log::warn!("starting the state writer: {e}"))
        .ok()?;
    Some(tx)
}

fn writer(rx: Receiver<Job>, path: &Path) {
    let save = |state: &State| {
        if let Err(e) = write(path, state) {
            log::warn!("saving {}: {e:#}", path.display());
        }
    };
    let mut pending: Option<(Arc<State>, Instant)> = None;
    loop {
        let job = match &pending {
            None => match rx.recv() {
                Ok(job) => job,
                Err(_) => return,
            },
            Some((_, due)) => {
                match rx.recv_timeout(due.saturating_duration_since(Instant::now())) {
                    Ok(job) => job,
                    Err(RecvTimeoutError::Timeout) => {
                        if let Some((state, _)) = pending.take() {
                            save(&state);
                        }
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        if let Some((state, _)) = pending.take() {
                            save(&state);
                        }
                        return;
                    }
                }
            }
        };
        match job {
            Job::Write(state) => {
                let due = pending
                    .as_ref()
                    .map(|(_, due)| *due)
                    .unwrap_or_else(|| Instant::now() + BATCH);
                pending = Some((state, due));
            }
            Job::Flush(ack) => {
                if let Some((state, _)) = pending.take() {
                    save(&state);
                }
                let _ = ack.send(());
            }
        }
    }
}

struct Shared {
    state: Arc<State>,
    /// None in a process that doesn't own the instance: it writes nothing.
    writer: Option<Sender<Job>>,
}
impl Global for Shared {}

/// Load the state file into the app. Call once, at startup.
pub fn init(cx: &mut App) {
    let writable = crate::instance::owns_state();
    let path = path();
    let state = load_from(&path, &old_path(), writable);
    let writer = if writable {
        if let Some(dir) = path.parent() {
            clear_temps(dir);
        }
        spawn_writer(path)
    } else {
        None
    };
    cx.set_global(Shared {
        state: Arc::new(state),
        writer,
    });
}

/// The current state. Keep the `Arc` for as long as a job runs; it doesn't
/// change under you.
pub fn get(cx: &App) -> Arc<State> {
    cx.global::<Shared>().state.clone()
}

/// Change the state: `f` sees the current value, whatever any window did
/// since. Written about half a second later.
pub fn update(cx: &mut App, f: impl FnOnce(&mut State)) {
    let shared = cx.global_mut::<Shared>();
    let mut next = (*shared.state).clone();
    f(&mut next);
    if next == *shared.state {
        return;
    }
    shared.state = Arc::new(next);
    if let Some(writer) = &shared.writer {
        let _ = writer.send(Job::Write(shared.state.clone()));
    }
}

/// Write any pending change now, waiting a moment for it: before quitting.
pub fn flush(cx: &App) {
    let Some(Some(writer)) = cx.try_global::<Shared>().map(|s| s.writer.as_ref()) else {
        return;
    };
    let (ack, done) = mpsc::channel();
    if writer.send(Job::Flush(ack)).is_ok() {
        let _ = done.recv_timeout(FLUSH_WAIT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> State {
        let mut layout = DockLayout::default();
        layout.bottom.open = true;
        layout.bottom.active = 1;
        State {
            window: Some(WindowPlacement {
                x: 10.0,
                y: 20.0,
                w: 1000.0,
                h: 700.0,
                maximized: false,
            }),
            docks: Some(layout),
            ocr: sheaf_ocr::OcrConfig {
                engine: sheaf_ocr::EngineKind::Oar,
                dpi: 200,
                ..Default::default()
            },
            print: crate::print_dialog::PrintPrefs {
                printer: Some("Office".into()),
                markup: false,
                ..Default::default()
            },
            scan: sheaf_ops::scan::ScanOptions {
                dpi: 200,
                ..Default::default()
            },
            combine: sheaf_ops::combine::CombineParams {
                bookmark_each_file: false,
                ..Default::default()
            },
            blanks: sheaf_ops::blank::BlankParams {
                keep_intentional: true,
                ..Default::default()
            },
            recent_stamps: vec![sheaf_ops::stamps::StampRef::preset("Approved")],
            set_status: Some(crate::set_status::StatusSpec::default()),
            comment_columns: vec!["Number".into(), "Text".into()],
        }
    }

    #[test]
    fn round_trips_through_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let (path, old) = (
            tmp.path().join("state/state.json"),
            tmp.path().join("settings.json"),
        );
        assert_eq!(load_from(&path, &old, true), State::default());
        write(&path, &sample()).unwrap();
        assert_eq!(load_from(&path, &old, true), sample());
        // Unknown keys and missing fields are tolerated.
        std::fs::write(
            &path,
            br#"{"future": 1, "docks": {"left": {"panels": ["bookmarks"]}}}"#,
        )
        .unwrap();
        let loaded = load_from(&path, &old, true);
        assert_eq!(
            loaded.docks.as_ref().unwrap().left.panels,
            vec!["bookmarks"]
        );
        // Docks the file leaves out come from the default layout.
        assert_eq!(
            loaded.docks.as_ref().unwrap().right,
            DockLayout::default().right
        );
    }

    #[test]
    fn a_broken_file_is_set_aside() {
        let tmp = tempfile::tempdir().unwrap();
        let (path, old) = (
            tmp.path().join("state.json"),
            tmp.path().join("settings.json"),
        );
        std::fs::write(&path, b"{ not json").unwrap();
        assert_eq!(load_from(&path, &old, true), State::default());
        assert!(!path.exists());
        assert_eq!(
            std::fs::read(path.with_extension("json.bad")).unwrap(),
            b"{ not json"
        );
        // A process that doesn't own the state leaves it alone.
        std::fs::write(&path, b"{ not json").unwrap();
        assert_eq!(load_from(&path, &old, false), State::default());
        assert!(path.exists());
    }

    #[test]
    fn the_old_settings_file_moves_over() {
        let tmp = tempfile::tempdir().unwrap();
        let (path, old) = (
            tmp.path().join("state/state.json"),
            tmp.path().join("settings.json"),
        );
        std::fs::write(&old, serde_json::to_vec(&sample()).unwrap()).unwrap();
        // Not the owner: read it, move nothing.
        assert_eq!(load_from(&path, &old, false), sample());
        assert!(old.exists() && !path.exists());
        assert_eq!(load_from(&path, &old, true), sample());
        assert!(!old.exists());
        assert_eq!(load_from(&path, &old, true), sample());
    }

    #[test]
    fn the_writer_batches_and_flushes() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state.json");
        let writer = spawn_writer(path.clone()).unwrap();
        let mut a = sample();
        a.blanks.keep_intentional = false;
        writer.send(Job::Write(Arc::new(a))).unwrap();
        writer.send(Job::Write(Arc::new(sample()))).unwrap();
        // Held for the batch: nothing yet.
        assert!(!path.exists());
        let (ack, done) = mpsc::channel();
        writer.send(Job::Flush(ack)).unwrap();
        done.recv_timeout(Duration::from_secs(5)).unwrap();
        // Only the last of the burst is written.
        assert_eq!(load_from(&path, &path, true), sample());
        // And a lone change is written after the batch delay.
        let mut b = sample();
        b.scan.dpi = 300;
        writer.send(Job::Write(Arc::new(b.clone()))).unwrap();
        std::thread::sleep(BATCH * 3);
        assert_eq!(load_from(&path, &path, true), b);
    }

    #[test]
    fn crash_leftovers_are_cleared() {
        let tmp = tempfile::tempdir().unwrap();
        let stray = tmp.path().join(".state.json.abc123.tmp");
        let other = tmp.path().join("state.json");
        std::fs::write(&stray, b"x").unwrap();
        std::fs::write(&other, b"{}").unwrap();
        clear_temps(tmp.path());
        assert!(!stray.exists() && other.exists());
    }
}
