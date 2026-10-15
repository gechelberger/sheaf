//! How much undo history to keep.
//!
//! History is a tree of versions, each with the edits that made it (see
//! `model::Document`). A policy decides how many snapshots of the
//! document to keep; a version whose snapshot is dropped is rebuilt by
//! replaying edits when it's needed. The versions themselves (their edits)
//! are kept up to [`MAX_VERSIONS`].

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
    /// Estimated bytes each snapshot holds that its parent's doesn't, in
    /// the order they'd be dropped: those that may be first, then those
    /// that must stay.
    pub marginal_bytes: Vec<usize>,
    /// Bytes of source files referenced only by versions newer than the
    /// cursor (i.e. inserts that have been undone).
    pub undone_source_bytes: usize,
}

impl HistoryStats {
    pub fn total_bytes(&self) -> usize {
        self.marginal_bytes.iter().sum::<usize>() + self.undone_source_bytes
    }
}

pub trait HistoryPolicy: Send + Sync {
    /// How many snapshots to drop, from the front of `marginal_bytes`. The
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

/// Keep dropping the oldest versions until the estimate fits the budget.
pub struct MemoryBudget(pub usize);

impl HistoryPolicy for MemoryBudget {
    fn trim(&self, stats: &HistoryStats) -> usize {
        let mut total = stats.total_bytes();
        let mut drop = 0;
        while total > self.0 && drop + 1 < stats.versions {
            total -= stats.marginal_bytes[drop];
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

    fn stats(marginal: &[usize]) -> HistoryStats {
        HistoryStats {
            versions: marginal.len(),
            cursor: marginal.len() - 1,
            marginal_bytes: marginal.to_vec(),
            undone_source_bytes: 0,
        }
    }

    #[test]
    fn fixed_count() {
        assert_eq!(FixedCount(3).trim(&stats(&[1, 1, 1, 1, 1])), 2);
        assert_eq!(FixedCount(10).trim(&stats(&[1, 1])), 0);
    }

    #[test]
    fn memory_budget_never_drops_current() {
        assert_eq!(MemoryBudget(5).trim(&stats(&[10, 10, 10])), 2);
        assert_eq!(MemoryBudget(25).trim(&stats(&[10, 10, 10])), 1);
    }

    #[test]
    fn all_of_takes_max() {
        let p = AllOf(vec![Box::new(FixedCount(2)), Box::new(MemoryBudget(1000))]);
        assert_eq!(p.trim(&stats(&[1, 1, 1, 1])), 2);
    }
}
