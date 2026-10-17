//! What the app and the command line read from an op's tracker: a cancel
//! told from a failure, and where the bar is when a step ends.

use std::path::PathBuf;

use sheaf_doc::Document;
use sheaf_ops::combine::{CombineItem, CombineParams, combine};
use sheaf_ops::split::{SplitBy, split};
use sheaf_ops::{Cancelled, Tracker, was_cancelled};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sheaf-tracker-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn blank(pages: u32) -> Document {
    let store = sheaf_doc::Store::new();
    let primary = sheaf_doc::SourceInfo::blank(&store, pages);
    Document::new(store, primary, &[], sheaf_doc::history::default_policy())
}

/// "Stopped" and exit status 130, not "failed": however the op passed
/// the cancel back, with context or inside an `io::Error`.
#[test]
fn a_cancel_is_told_from_a_failure() {
    let io = || std::io::Error::from(Cancelled);
    assert!(was_cancelled(&Cancelled.into()));
    assert!(was_cancelled(
        &anyhow::Error::new(Cancelled).context("reading a.pdf")
    ));
    assert!(was_cancelled(&io().into()));
    assert!(was_cancelled(
        &anyhow::Error::new(io()).context("writing out.pdf")
    ));
    assert!(!was_cancelled(&std::io::Error::other("x").into()));
    assert!(!was_cancelled(&anyhow::anyhow!("x")));
}

/// Once the files are read, the combine dialog says "Writing…": its
/// threshold is the reading step's share, 0.7.
#[test]
fn combining_reads_to_the_dialog_s_threshold() {
    let dir = scratch("combine");
    let items: Vec<CombineItem> = ["a", "b"]
        .iter()
        .map(|name| {
            let path = dir.join(format!("{name}.pdf"));
            let bytes = sheaf_doc::save::rewrite(&blank(1).snapshot()).unwrap();
            std::fs::write(&path, bytes).unwrap();
            CombineItem::new(path)
        })
        .collect();
    let tracker = Tracker::new();
    let out = dir.join("out.pdf");
    combine(&items, &CombineParams::default(), &out, &tracker).unwrap();
    let f = tracker.snapshot().fraction;
    assert!(f >= Some(0.7), "{f:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A size split measures, then writes: when it returns, both steps have
/// filled their shares and the status is the job's own again.
#[test]
fn a_size_split_ends_full() {
    let dir = scratch("split");
    let tracker = Tracker::new();
    let by = SplitBy::MaxBytes(u64::MAX);
    let parts = split(&blank(3).snapshot(), &by, &dir, "a", false, &tracker).unwrap();
    assert_eq!(parts.len(), 1);
    let s = tracker.snapshot();
    assert_eq!((s.fraction, s.step.as_str()), (Some(1.0), ""));
    let _ = std::fs::remove_dir_all(&dir);
}
