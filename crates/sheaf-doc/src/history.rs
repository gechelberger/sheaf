//! How much undo history to keep.

/// What a policy gets to look at. All byte figures are estimates.
#[derive(Debug, Clone)]
pub struct HistoryStats {
    /// Number of versions currently held (including the current one).
    pub versions: usize,
    /// Index of the current version.
    pub cursor: usize,
    /// Estimated bytes each version holds that its predecessor doesn't.
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
    /// How many of the oldest versions to drop. The document never drops the
    /// current version regardless of what this returns.
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
