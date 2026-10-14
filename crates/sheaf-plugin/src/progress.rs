//! The `progress` interface: a plugin's `task` handles are trackers under
//! the call's own (`CallContext::progress`), which the app shows and can
//! cancel. Handles are freed when the call that made them returns.

use sheaf_progress::{Level, Progress, Tracker, Unit};
use wasmtime::component::Resource;

use crate::runtime::HostState;
use crate::runtime::bindings::sheaf::plugin::progress as api;

/// A `task` handle.
pub struct Task(pub(crate) Tracker);

/// Most tasks alive at once in one call.
pub const MAX_TASKS: usize = 256;

fn unit(u: api::Unit) -> Unit {
    match u {
        api::Unit::Items => Unit::Items,
        api::Unit::Pages => Unit::Pages,
        api::Unit::Bytes => Unit::Bytes,
    }
}

impl HostState {
    fn push_task(&mut self, t: Tracker) -> Resource<Task> {
        // Past the cap, a detached tracker: it works, nobody sees it.
        let t = if self.task_handles.len() >= MAX_TASKS {
            Tracker::new()
        } else {
            t
        };
        let r = self.table.push(Task(t)).expect("resource table full");
        self.task_handles.push(r.rep());
        r
    }

    fn task(&self, this: &Resource<Task>) -> Option<&Tracker> {
        self.table.get(this).ok().map(|t| &t.0)
    }

    /// Free every task handle the call made.
    pub(crate) fn drop_task_handles(&mut self) {
        for rep in std::mem::take(&mut self.task_handles) {
            let _ = self.table.delete::<Task>(Resource::new_own(rep));
        }
    }
}

impl api::Host for HostState {}

impl api::HostTask for HostState {
    fn new(&mut self, label: String, total: Option<u64>, u: api::Unit) -> Resource<Task> {
        let t = match self.progress() {
            Some(root) => root.child_tracker(&label, 1.0),
            None => Tracker::new(),
        };
        t.set_total(total, unit(u));
        self.push_task(t)
    }

    fn set_total(&mut self, this: Resource<Task>, total: Option<u64>, u: api::Unit) {
        if let Some(t) = self.task(&this) {
            t.set_total(total, unit(u));
        }
    }

    fn set(&mut self, this: Resource<Task>, done: u64) {
        if let Some(t) = self.task(&this) {
            t.set(done);
        }
    }

    fn inc(&mut self, this: Resource<Task>, n: u64) {
        if let Some(t) = self.task(&this) {
            t.inc(n);
        }
    }

    fn message(&mut self, this: Resource<Task>, level: api::Level, message: String) {
        if let Some(t) = self.task(&this) {
            let level = match level {
                api::Level::Info => Level::Info,
                api::Level::Warn => Level::Warn,
                api::Level::Error => Level::Error,
            };
            // A status line, not a document.
            let message: String = message.chars().take(500).collect();
            t.message(level, &message);
        }
    }

    fn is_cancelled(&mut self, this: Resource<Task>) -> bool {
        self.task(&this).is_some_and(|t| t.is_cancelled())
    }

    fn child(&mut self, this: Resource<Task>, label: String, weight: f32) -> Resource<Task> {
        let child = match self.task(&this) {
            Some(t) => t.child_tracker(&label, if weight.is_finite() { weight } else { 0.0 }),
            None => Tracker::new(),
        };
        self.push_task(child)
    }

    fn drop(&mut self, this: Resource<Task>) -> wasmtime::Result<()> {
        let rep = this.rep();
        self.task_handles.retain(|r| *r != rep);
        let _ = self.table.delete(this);
        Ok(())
    }
}
