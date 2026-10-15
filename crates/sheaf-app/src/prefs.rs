//! Preferences from `settings.toml` (`sheaf_ops::prefs`) for the whole
//! app: read at startup, and again when a window comes forward if the file
//! changed. Problems show in a banner in every window until the file is
//! fixed or the banner is dismissed. Edit > Open Settings File opens it,
//! writing a commented template first if there's none.

use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use gpui::{App, Global};
use sheaf_ops::prefs::{self, Prefs, Problem};

/// Enough to tell that a file changed without reading it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
}

/// `path`'s stamp, or None when there's no file.
pub fn stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some(Stamp {
        modified: meta.modified().ok(),
        len: meta.len(),
    })
}

struct Shared {
    prefs: Arc<Prefs>,
    problems: Vec<Problem>,
    dismissed: bool,
    stamp: Option<Stamp>,
}
impl Global for Shared {}

fn read() -> (Prefs, Vec<Problem>) {
    let (p, problems) = prefs::load();
    for e in &problems {
        log::warn!("{}: {e}", prefs::FILE);
    }
    (p, problems)
}

pub fn init(cx: &mut App) {
    let stamp = stamp(&prefs::path());
    let (p, problems) = read();
    for e in &problems {
        eprintln!("{}: {e}", prefs::FILE);
    }
    cx.set_global(Shared {
        prefs: Arc::new(p),
        problems,
        dismissed: false,
        stamp,
    });
}

/// The current preferences. A job keeps the `Arc` it started with.
pub fn get(cx: &App) -> Arc<Prefs> {
    cx.global::<Shared>().prefs.clone()
}

/// The OCR options a run uses: the Recognize Text dialog's remembered
/// choices, with `[ocr]` from `settings.toml` on top.
pub fn ocr_config(cx: &App) -> sheaf_ocr::OcrConfig {
    let mut config = crate::state::get(cx).ocr.clone();
    get(cx).ocr.apply(&mut config);
    config
}

/// Re-read `settings.toml` if it changed since it was last read. A file
/// that now has a syntax error keeps the settings from before it.
pub fn reload_if_changed(cx: &mut App) {
    let now = stamp(&prefs::path());
    let shared = cx.global_mut::<Shared>();
    if now == shared.stamp {
        return;
    }
    shared.stamp = now;
    let (p, problems) = read();
    let broken = problems.iter().any(|p| p.whole_file);
    if !broken {
        shared.prefs = Arc::new(p);
    }
    shared.problems = problems;
    shared.dismissed = false;
    cx.refresh_windows();
}

/// What the banner says, if anything: the first problem, and how many more.
pub fn banner(cx: &App) -> Option<String> {
    let shared = cx.try_global::<Shared>()?;
    let first = shared.problems.first().filter(|_| !shared.dismissed)?;
    let more = match shared.problems.len() - 1 {
        0 => String::new(),
        1 => " (and 1 more)".into(),
        n => format!(" (and {n} more)"),
    };
    Some(format!("{}, {first}{more}", prefs::FILE))
}

pub fn dismiss(cx: &mut App) {
    cx.global_mut::<Shared>().dismissed = true;
    cx.refresh_windows();
}

/// Open `settings.toml` in an editor, writing the commented template
/// first if there's no file yet.
pub fn open_settings_file(cx: &mut App) {
    let path = prefs::path();
    if !path.exists() {
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&path, prefs::TEMPLATE));
        if let Err(e) = written {
            log::warn!("writing {}: {e}", path.display());
        }
        // Our own write isn't a change to reload.
        cx.global_mut::<Shared>().stamp = stamp(&path);
    }
    // Windows has no editor for .toml by default; Notepad is always there.
    let opened = if cfg!(target_os = "windows") {
        std::process::Command::new("notepad")
            .arg(&path)
            .spawn()
            .map(|_| ())
            .map_err(anyhow::Error::from)
    } else {
        crate::attachments_panel::open_with_system(&path)
    };
    if let Err(e) = opened {
        log::warn!("opening {}: {e:#}", path.display());
    }
}
