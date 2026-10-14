//! This process's place among sheaf processes: the one owner per user and
//! machine opens every window and alone writes sheaf's state; a later
//! launch hands its files over and exits (see `sheaf_instance`).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures::StreamExt;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use gpui::{App, Global};
use sheaf_instance::{Mode, Request, Started};

/// How long a launch keeps trying an owner that holds the lock but doesn't
/// answer (still starting up, or quitting) before running alone.
const WAIT: Duration = Duration::from_secs(3);

static OWNS_STATE: AtomicBool = AtomicBool::new(true);

/// Whether this process may write sheaf's state (`state.json`, recent
/// files).
/// Only the owner does; a process running alone beside it writes nothing.
pub fn owns_state() -> bool {
    OWNS_STATE.load(Ordering::Relaxed)
}

/// The lock, held for the life of the process.
struct Instance(Option<sheaf_instance::Owner>);
impl Global for Instance {}

/// Decided before GPUI starts.
pub struct Launch {
    owner: Option<sheaf_instance::Owner>,
    /// Files from later launches (and, on macOS, from Finder).
    pub requests: UnboundedReceiver<Request>,
    pub sink: UnboundedSender<Request>,
}

/// The runtime folder: under `SHEAF_CONFIG_DIR` when that's set, so a
/// scratch config (tests, scripts) gets an instance of its own.
fn runtime_dir() -> Option<PathBuf> {
    if std::env::var_os("SHEAF_CONFIG_DIR").is_some() {
        return Some(sheaf_ops::dirs::config_dir().join("run"));
    }
    sheaf_instance::default_dir()
}

/// Join the running sheaf, or become it. Exits the process when another
/// sheaf took `paths`.
pub fn start(mode: Mode, paths: &[PathBuf]) -> Launch {
    let (sink, requests) = unbounded();
    let request = Request::new(paths.iter().cloned());
    let started = match runtime_dir() {
        Some(dir) => sheaf_instance::start(&dir, mode, &request, sink.clone(), WAIT),
        None => Started::Standalone("no runtime folder".into()),
    };
    let owner = match started {
        Started::Owner(o) => Some(o),
        Started::HandedOff => std::process::exit(0),
        Started::Standalone(why) => {
            OWNS_STATE.store(false, Ordering::Relaxed);
            log::warn!("running alone, keeping no settings: {why}");
            if mode == Mode::Join {
                eprintln!("sheaf: running alone, keeping no settings: {why}");
            }
            None
        }
    };
    Launch {
        owner,
        requests,
        sink,
    }
}

impl Launch {
    /// Hold the lock for the app's life and open what later launches send.
    pub fn serve(self, cx: &mut App) {
        let mut requests = self.requests;
        cx.set_global(Instance(self.owner));
        cx.spawn(async move |cx| {
            while let Some(req) = requests.next().await {
                cx.update(|cx| crate::workspace::open_from_launch(req.paths, cx));
            }
        })
        .detach();
    }
}

/// Quit: tell later launches to wait for the lock rather than hand files
/// to a process on its way out, and write the state's last changes.
pub fn quit(cx: &mut App) {
    if let Some(Instance(Some(owner))) = cx.try_global::<Instance>() {
        owner.stop();
    }
    crate::state::flush(cx);
    cx.quit();
}

/// A `file://` URL as a path (macOS hands opened files over as these).
pub fn file_url_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    // An empty host or "localhost"; anything else is another machine.
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        match (bytes[i], bytes.get(i + 1), bytes.get(i + 2)) {
            (b'%', Some(&h), Some(&l)) if hex(h).is_some() && hex(l).is_some() => {
                out.push((hex(h)? * 16 + hex(l)?) as u8);
                i += 3;
            }
            (b, _, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    Some(PathBuf::from(String::from_utf8(out).ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_urls_decode() {
        assert_eq!(
            file_url_path("file:///Users/g/My%20Binder.pdf"),
            Some(PathBuf::from("/Users/g/My Binder.pdf"))
        );
        assert_eq!(
            file_url_path("file://localhost/tmp/a.pdf"),
            Some(PathBuf::from("/tmp/a.pdf"))
        );
        assert_eq!(file_url_path("file://server/share/a.pdf"), None);
        assert_eq!(file_url_path("https://example.com/a.pdf"), None);
        // A stray percent is kept as it is.
        assert_eq!(
            file_url_path("file:///tmp/100%.pdf"),
            Some(PathBuf::from("/tmp/100%.pdf"))
        );
    }
}
