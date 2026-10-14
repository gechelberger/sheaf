//! Progress for long commands: shown in sheaf's status bar while the call
//! runs, with Escape to stop it. (API 0.2.1; declare `api = "^0.2.1"`.)
//!
//! ```ignore
//! use sheaf_plugin_api::{document, progress};
//!
//! let pages = document::pages();
//! let task = progress::pages("Checking totals", pages.len() as u64);
//! for (i, page) in pages.iter().enumerate() {
//!     progress::check(&task)?;
//!     // … work on `page` …
//!     task.set(i as u64 + 1);
//! }
//! ```

pub use crate::bindings::sheaf::plugin::progress::{Level, Task, Unit};

/// A task counting `total` pages.
pub fn pages(label: &str, total: u64) -> Task {
    Task::new(label, Some(total), Unit::Pages)
}

/// A task counting `total` items (rows, files, …).
pub fn items(label: &str, total: u64) -> Task {
    Task::new(label, Some(total), Unit::Items)
}

/// `Err("Stopped")` once the user has asked the command to stop, so a
/// loop can end with `progress::check(&task)?`.
pub fn check(task: &Task) -> Result<(), String> {
    if task.is_cancelled() {
        Err("Stopped".into())
    } else {
        Ok(())
    }
}
