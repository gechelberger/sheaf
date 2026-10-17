//! The `progress` interface: a plugin's `task` handles are trackers under
//! the call's own (`CallContext::progress`), which the app shows and can
//! cancel. Handles are freed when the call that made them returns.

use in_progress::Tracker;
use wasmtime::component::Resource;

use crate::runtime::HostState;

/// A `task` handle.
pub struct Task(pub(crate) Tracker);

/// Most tasks alive at once in one call.
pub const MAX_TASKS: usize = 256;

impl HostState {
    /// Whether a new task can go under the call's tracker. Past the cap a
    /// task gets a detached tracker instead: nobody sees it, and
    /// `is-cancelled` asks the call. Ask before placing one, since dropping
    /// a placed task fills its share.
    pub(crate) fn has_task_room(&self) -> bool {
        self.task_handles.len() < MAX_TASKS
    }

    pub(crate) fn push_task(&mut self, t: Tracker) -> Resource<Task> {
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
