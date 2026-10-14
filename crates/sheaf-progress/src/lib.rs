//! Progress and cancellation for long-running operations.
//!
//! Ops take `&dyn Progress`, report how far they are, and call
//! `p.check()?` once per page or chunk so a cancel stops them cleanly.
//! [`Tracker`] is the shared implementation: the GUI polls its
//! [`Snapshot`] each frame and the CLI draws it as a bar; both cancel it
//! from another thread. [`NoProgress`] is for callers that don't care.
//!
//! A [`Progress::child`] covers a weighted share of its parent's span, so
//! a job made of steps ("render, then OCR") can hand each step its own
//! progress without the step knowing where it sits in the whole.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// What a count counts. A closed set, so it can cross into plugins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Unit {
    #[default]
    Items,
    Pages,
    Bytes,
}

impl Unit {
    /// The plural noun, for "3 of 10 pages".
    pub fn noun(self) -> &'static str {
        match self {
            Unit::Items => "items",
            Unit::Pages => "pages",
            Unit::Bytes => "bytes",
        }
    }

    fn from_u8(v: u8) -> Unit {
        match v {
            1 => Unit::Pages,
            2 => Unit::Bytes,
            _ => Unit::Items,
        }
    }

    fn to_u8(self) -> u8 {
        match self {
            Unit::Items => 0,
            Unit::Pages => 1,
            Unit::Bytes => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

/// The error an op returns when it stops because it was cancelled.
/// Callers that need to tell a cancel from a failure downcast to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cancelled")
    }
}

impl std::error::Error for Cancelled {}

pub trait Progress: Send + Sync {
    /// How many there are to do; `None` while that isn't known.
    fn set_total(&self, total: Option<u64>, unit: Unit);
    /// How many are done.
    fn set(&self, done: u64);
    fn inc(&self, n: u64);
    fn message(&self, level: Level, msg: &str);
    fn is_cancelled(&self) -> bool;
    /// `Err(Cancelled)` once cancelled, for `p.check()?`.
    fn check(&self) -> Result<(), Cancelled> {
        if self.is_cancelled() {
            Err(Cancelled)
        } else {
            Ok(())
        }
    }
    /// A progress that covers `weight` (0–1) of this one's span, starting
    /// where this one is now or where its last child's share ended,
    /// whichever is further. It shares this one's cancellation.
    fn child(&self, name: &str, weight: f32) -> Box<dyn Progress + '_>;
}

/// Reports nowhere and never cancels.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoProgress;

impl Progress for NoProgress {
    fn set_total(&self, _: Option<u64>, _: Unit) {}
    fn set(&self, _: u64) {}
    fn inc(&self, _: u64) {}
    fn message(&self, _: Level, _: &str) {}
    fn is_cancelled(&self) -> bool {
        false
    }
    fn child(&self, _: &str, _: f32) -> Box<dyn Progress + '_> {
        Box::new(NoProgress)
    }
}

/// What a [`Tracker`] looks like right now.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Snapshot {
    /// Overall, 0–1; `None` until some count is known.
    pub fraction: Option<f32>,
    /// The step that last reported a count: its name, done, total, unit.
    pub step: String,
    pub done: u64,
    pub total: Option<u64>,
    pub unit: Unit,
    /// The latest message.
    pub message: Option<(Level, String)>,
    pub cancelled: bool,
}

const UNKNOWN: u32 = u32::MAX;
const NO_TOTAL: u64 = u64::MAX;

struct Shared {
    cancelled: AtomicBool,
    /// `f32` bits of the overall fraction; `UNKNOWN` before any count.
    fraction: AtomicU32,
    status: Mutex<Status>,
}

#[derive(Default)]
struct Status {
    step: String,
    done: u64,
    total: Option<u64>,
    unit: Unit,
    message: Option<(Level, String)>,
}

/// The shared [`Progress`]: cheap to clone, every clone is the same
/// progress. Children share the root's cancellation and snapshot.
#[derive(Clone)]
pub struct Tracker {
    shared: Arc<Shared>,
    node: Arc<Node>,
}

/// One progress in the tree: its span of the root's 0–1, and its count.
struct Node {
    name: String,
    lo: f32,
    hi: f32,
    total: AtomicU64,
    done: AtomicU64,
    unit: AtomicU8,
    /// `f32` bits: where the last child's share ends, so the next child
    /// starts after it.
    next: AtomicU32,
}

impl Node {
    fn new(name: &str, lo: f32, hi: f32) -> Node {
        Node {
            name: name.to_string(),
            lo,
            hi,
            total: AtomicU64::new(NO_TOTAL),
            done: AtomicU64::new(0),
            unit: AtomicU8::new(0),
            next: AtomicU32::new(lo.to_bits()),
        }
    }

    fn total(&self) -> Option<u64> {
        match self.total.load(Ordering::Relaxed) {
            NO_TOTAL => None,
            t => Some(t),
        }
    }

    /// Where this node is, as a fraction of the root.
    fn position(&self) -> f32 {
        let done = self.done.load(Ordering::Relaxed);
        match self.total() {
            Some(0) => self.hi,
            Some(t) => self.lo + (self.hi - self.lo) * (done as f32 / t as f32).min(1.0),
            None => self.lo,
        }
    }
}

impl Default for Tracker {
    fn default() -> Self {
        Self::new()
    }
}

impl Tracker {
    pub fn new() -> Tracker {
        Tracker {
            shared: Arc::new(Shared {
                cancelled: AtomicBool::new(false),
                fraction: AtomicU32::new(UNKNOWN),
                status: Mutex::new(Status::default()),
            }),
            node: Arc::new(Node::new("", 0.0, 1.0)),
        }
    }

    /// [`Progress::child`] as an owned `Tracker`, for holders that can't
    /// borrow this one (a plugin's handle, say).
    pub fn child_tracker(&self, name: &str, weight: f32) -> Tracker {
        let node = &self.node;
        let lo = node
            .position()
            .max(f32::from_bits(node.next.load(Ordering::Relaxed)))
            .min(node.hi);
        let hi = (lo + (node.hi - node.lo) * weight.clamp(0.0, 1.0)).min(node.hi);
        node.next.store(hi.to_bits(), Ordering::Relaxed);
        Tracker {
            shared: self.shared.clone(),
            node: Arc::new(Node::new(name, lo, hi)),
        }
    }

    /// Ask the work to stop; it does at its next `check()`.
    pub fn cancel(&self) {
        self.shared.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> Snapshot {
        let bits = self.shared.fraction.load(Ordering::Relaxed);
        let st = self.status();
        Snapshot {
            fraction: (bits != UNKNOWN).then(|| f32::from_bits(bits)),
            step: st.step.clone(),
            done: st.done,
            total: st.total,
            unit: st.unit,
            message: st.message.clone(),
            cancelled: self.is_cancelled(),
        }
    }

    fn status(&self) -> std::sync::MutexGuard<'_, Status> {
        // A panic while holding the lock leaves plain data; keep going.
        self.shared.status.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Publish this node's count and position.
    fn report(&self) {
        let node = &self.node;
        if node.total().is_some() {
            let pos = node.position().clamp(0.0, 1.0);
            self.shared.fraction.store(pos.to_bits(), Ordering::Relaxed);
        }
        let mut st = self.status();
        st.step.clone_from(&node.name);
        st.done = node.done.load(Ordering::Relaxed);
        st.total = node.total();
        st.unit = Unit::from_u8(node.unit.load(Ordering::Relaxed));
    }
}

impl Progress for Tracker {
    fn set_total(&self, total: Option<u64>, unit: Unit) {
        self.node
            .total
            .store(total.unwrap_or(NO_TOTAL), Ordering::Relaxed);
        self.node.unit.store(unit.to_u8(), Ordering::Relaxed);
        self.report();
    }

    fn set(&self, done: u64) {
        self.node.done.store(done, Ordering::Relaxed);
        self.report();
    }

    fn inc(&self, n: u64) {
        self.node.done.fetch_add(n, Ordering::Relaxed);
        self.report();
    }

    fn message(&self, level: Level, msg: &str) {
        self.status().message = Some((level, msg.to_string()));
    }

    fn is_cancelled(&self) -> bool {
        self.shared.cancelled.load(Ordering::Relaxed)
    }

    fn child(&self, name: &str, weight: f32) -> Box<dyn Progress + '_> {
        Box::new(self.child_tracker(name, weight))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frac(t: &Tracker) -> f32 {
        t.snapshot().fraction.unwrap()
    }

    #[test]
    fn counts_and_fraction() {
        let t = Tracker::new();
        assert_eq!(t.snapshot().fraction, None);
        t.set_total(Some(4), Unit::Pages);
        assert_eq!(frac(&t), 0.0);
        t.inc(1);
        t.inc(1);
        assert_eq!(frac(&t), 0.5);
        let s = t.snapshot();
        assert_eq!((s.done, s.total, s.unit), (2, Some(4), Unit::Pages));
        t.set(4);
        assert_eq!(frac(&t), 1.0);
        // Overshoot stays at the end.
        t.inc(3);
        assert_eq!(frac(&t), 1.0);
    }

    #[test]
    fn indeterminate_leaves_the_fraction() {
        let t = Tracker::new();
        t.set_total(None, Unit::Items);
        t.inc(5);
        assert_eq!(t.snapshot().fraction, None);
        assert_eq!(t.snapshot().done, 5);
    }

    #[test]
    fn children_fill_their_share() {
        let t = Tracker::new();
        t.set_total(Some(1), Unit::Items);
        {
            let render = t.child("render", 0.25);
            render.set_total(Some(10), Unit::Pages);
            render.set(10);
        }
        assert_eq!(frac(&t), 0.25);
        let ocr = t.child("ocr", 0.75);
        ocr.set_total(Some(2), Unit::Pages);
        ocr.inc(1);
        assert_eq!(frac(&t), 0.25 + 0.75 / 2.0);
        // Grandchildren nest inside their parent's share.
        let page = ocr.child("page 2", 0.5);
        page.set_total(Some(2), Unit::Items);
        page.set(1);
        assert_eq!(frac(&t), 0.625 + 0.75 * 0.5 * 0.5);
        let s = t.snapshot();
        assert_eq!(s.step, "page 2");
    }

    #[test]
    fn cancel_reaches_every_child() {
        let t = Tracker::new();
        let handle = t.clone();
        let child = t.child("a", 0.5);
        let grandchild = child.child("b", 0.5);
        assert_eq!(grandchild.check(), Ok(()));
        std::thread::spawn(move || handle.cancel()).join().unwrap();
        assert_eq!(grandchild.check(), Err(Cancelled));
        assert!(t.snapshot().cancelled);
    }

    #[test]
    fn messages_keep_the_latest() {
        let t = Tracker::new();
        t.child("x", 1.0).message(Level::Warn, "page 3 is blank");
        assert_eq!(
            t.snapshot().message,
            Some((Level::Warn, "page 3 is blank".into()))
        );
    }

    #[test]
    fn no_progress_never_cancels() {
        let p = NoProgress;
        p.set_total(Some(3), Unit::Pages);
        assert!(p.child("c", 1.0).check().is_ok());
    }
}
