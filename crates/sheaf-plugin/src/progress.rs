//! The `progress` interface: a plugin's `task` handles are trackers under
//! the call's own (`CallContext::progress`), which the app shows and can
//! cancel. Handles are freed when the call that made them returns.

use sheaf_progress::Tracker;
use wasmtime::component::Resource;

use crate::runtime::HostState;

/// A `task` handle.
pub struct Task(pub(crate) Tracker);

/// Most tasks alive at once in one call.
pub const MAX_TASKS: usize = 256;

impl HostState {
    pub(crate) fn push_task(&mut self, t: Tracker) -> Resource<Task> {
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

    pub(crate) fn task(&self, this: &Resource<Task>) -> Option<&Tracker> {
        self.table.get(this).ok().map(|t| &t.0)
    }

    /// Free every task handle the call made.
    pub(crate) fn drop_task_handles(&mut self) {
        for rep in std::mem::take(&mut self.task_handles) {
            let _ = self.table.delete::<Task>(Resource::new_own(rep));
        }
    }
}
