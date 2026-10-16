//! How much undo history to keep.
//!
//! History is a tree of versions, each with the edits that made it and
//! what they changed, as prims (see `model::Document` and `prim`). A
//! policy decides how many snapshots of the document to keep; a version
//! whose snapshot is dropped is rebuilt when it's needed, by applying the
//! prims from the nearest version that kept one. The versions themselves
//! (their edits and prims) are kept up to [`MAX_VERSIONS`].
//!
//! Trimming snapshots frees only their bytes. What the steps hold is freed
//! when versions are pruned: by count today, and by bytes too from M10.5
//! slice 12.

/// Versions kept, with or without snapshots. Past this, versions on other
/// branches go first, then the oldest before the saved one.
pub const MAX_VERSIONS: usize = 1000;

/// What a policy gets to look at. All byte figures are estimates.
#[derive(Debug, Clone)]
pub struct HistoryStats {
    /// Snapshots currently held (the current version's included).
    pub versions: usize,
    /// Index of the current version's snapshot: the last.
    pub cursor: usize,
    /// Estimated bytes each snapshot holds that no other version does, in
    /// the order they'd be dropped: those that may be first, then those
    /// that must stay.
    pub snapshot_bytes: Vec<usize>,
    /// Estimated bytes all versions' steps hold: their edits and what they
    /// changed. Dropping snapshots doesn't free these.
    pub step_bytes: usize,
    /// Bytes of source files referenced only by versions newer than the
    /// cursor (i.e. inserts that have been undone).
    pub undone_source_bytes: usize,
}

impl HistoryStats {
    pub fn total_bytes(&self) -> usize {
        self.snapshot_bytes.iter().sum::<usize>() + self.step_bytes + self.undone_source_bytes
    }
}

pub trait HistoryPolicy: Send + Sync {
    /// How many snapshots to drop, from the front of `snapshot_bytes`. The
    /// document never drops more than it may (the current version's, the
    /// opened one's and saved ones' stay).
    fn trim(&self, stats: &HistoryStats) -> usize;
}

/// Keep at most this many versions.
pub struct FixedCount(pub usize);

impl HistoryPolicy for FixedCount {
    fn trim(&self, stats: &HistoryStats) -> usize {
        stats.versions.saturating_sub(self.0.max(1))
    }
}

/// Keep dropping the oldest snapshots until the estimate fits the budget.
/// Only their own bytes go, so when the steps alone are over it, every
/// snapshot that may be dropped is.
pub struct MemoryBudget(pub usize);

impl HistoryPolicy for MemoryBudget {
    fn trim(&self, stats: &HistoryStats) -> usize {
        let mut total = stats.total_bytes();
        let mut drop = 0;
        while total > self.0 && drop + 1 < stats.versions {
            total -= stats.snapshot_bytes[drop];
            drop += 1;
        }
        drop
    }
}

/// Drop enough to satisfy every policy.
pub struct AllOf(pub Vec<Box<dyn HistoryPolicy>>);

impl HistoryPolicy for AllOf {
    fn trim(&self, stats: &HistoryStats) -> usize {
        self.0.iter().map(|p| p.trim(stats)).max().unwrap_or(0)
    }
}

/// 200 versions and 256 MB.
pub fn default_policy() -> Box<dyn HistoryPolicy> {
    Box::new(AllOf(vec![
        Box::new(FixedCount(200)),
        Box::new(MemoryBudget(256 << 20)),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(snapshots: &[usize], steps: usize) -> HistoryStats {
        HistoryStats {
            versions: snapshots.len(),
            cursor: snapshots.len() - 1,
            snapshot_bytes: snapshots.to_vec(),
            step_bytes: steps,
            undone_source_bytes: 0,
        }
    }

    #[test]
    fn fixed_count() {
        assert_eq!(FixedCount(3).trim(&stats(&[1, 1, 1, 1, 1], 0)), 2);
        assert_eq!(FixedCount(10).trim(&stats(&[1, 1], 0)), 0);
    }

    #[test]
    fn memory_budget_never_drops_current() {
        assert_eq!(MemoryBudget(5).trim(&stats(&[10, 10, 10], 0)), 2);
        assert_eq!(MemoryBudget(25).trim(&stats(&[10, 10, 10], 0)), 1);
    }

    #[test]
    fn memory_budget_counts_steps_but_drops_only_snapshots() {
        assert_eq!(MemoryBudget(35).trim(&stats(&[10, 10, 10], 10)), 1);
        // Steps alone over the budget: every snapshot but the current one.
        assert_eq!(MemoryBudget(100).trim(&stats(&[10, 10, 10], 200)), 2);
    }

    #[test]
    fn all_of_takes_max() {
        let p = AllOf(vec![Box::new(FixedCount(2)), Box::new(MemoryBudget(1000))]);
        assert_eq!(p.trim(&stats(&[1, 1, 1, 1], 0)), 2);
    }
}
