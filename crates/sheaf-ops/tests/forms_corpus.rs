//! Every form in Greg's corpus opens, and the form check runs on it (M10
//! slice 1). What it finds is the baseline later slices compare with:
//! printed here (`--nocapture`), and recorded in `todos/m10.md`'s slice
//! notes. Skips when there's no corpus (see `sheaf_doc::forms::corpus`).
//!
//! Needs libpdfium (`scripts/fetch-pdfium.sh`), like the app itself.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use sheaf_core::PageIndex;
use sheaf_doc::forms::corpus::{Corpus, Entry};
use sheaf_doc::forms::{Report, check};
use sheaf_render::Renderer;
use sheaf_render::pdfium::{PdfiumEngine, render_appearances};

/// What became of one file.
struct Outcome {
    entry: Entry,
    /// Its pages, as PDFium opened it.
    opened: Result<u32, String>,
    checked: Result<Report, String>,
}

/// Open the file in PDFium as the app does and, if `draw`, draw its first
/// page's appearances. PDFium is called from one thread only: from
/// several, most loads fail (found in slice 1, not fixed; see the slice
/// notes in `todos/m10.md`).
fn open(bytes: &[u8], draw: bool) -> Result<u32, String> {
    let engine = PdfiumEngine::open_bytes(Arc::new(bytes.to_vec()), None);
    engine
        .and_then(|e| {
            e.page_sizes()?;
            if draw {
                render_appearances(bytes, None, PageIndex(0), 100)?;
            }
            Ok(e.page_count())
        })
        .map_err(|e| format!("{e:#}"))
}

#[test]
fn every_form_opens_and_is_checked() {
    let Some(corpus) = Corpus::or_skip() else {
        return;
    };
    let entries = corpus.manifest().unwrap();
    // The checks run on other threads, the opens on this one.
    let queue = Mutex::new(entries.iter().enumerate());
    let checked = Mutex::new(BTreeMap::new());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(16));
    let mut opened = Vec::new();
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let Some((i, entry)) = queue.lock().unwrap().next() else {
                        break;
                    };
                    let result = std::fs::read(corpus.path(entry))
                        .map_err(anyhow::Error::from)
                        .and_then(|bytes| check(&bytes, None))
                        .map_err(|e| format!("{e:#}"));
                    checked.lock().unwrap().insert(i, result);
                }
            });
        }
        for entry in &entries {
            let bytes = std::fs::read(corpus.path(entry)).map_err(|e| e.to_string());
            // Drawing the SafeDocs sample's 4,316 would take most of a
            // minute; the curated forms, a second.
            let draw = !entry.path.starts_with("forms/safedocs/");
            opened.push(bytes.and_then(|b| open(&b, draw)));
        }
    });
    let outcomes: Vec<Outcome> = entries
        .into_iter()
        .zip(opened)
        .zip(checked.into_inner().unwrap().into_values())
        .map(|((entry, opened), checked)| Outcome {
            entry,
            opened,
            checked,
        })
        .collect();

    let mut unopened = Vec::new();
    let mut unparsed = Vec::new();
    let mut pages_differ = Vec::new();
    let mut unchecked = Vec::new();
    let mut clean = 0;
    let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
    for o in &outcomes {
        match &o.opened {
            // pypdf can't read these either: a sample of damaged files.
            Err(e) if o.entry.kind == "unparsed" => unparsed.push(format!("{}: {e}", o.entry.path)),
            Err(e) => unopened.push(format!("{}: {e}", o.entry.path)),
            Ok(n) if o.entry.pages > 0 && *n != o.entry.pages => {
                pages_differ.push(format!("{}: {n}, not {}", o.entry.path, o.entry.pages))
            }
            Ok(_) => {}
        }
        match &o.checked {
            Err(e) => unchecked.push(format!("{} ({}): {e}", o.entry.path, o.entry.kind)),
            Ok(r) if r.is_clean() => clean += 1,
            Ok(r) => {
                let mut seen: Vec<&str> = r.findings.iter().map(|f| f.kind()).collect();
                seen.dedup();
                for k in seen {
                    *kinds.entry(k).or_default() += 1;
                }
            }
        }
    }
    eprintln!(
        "{} files: {} opened, {} checked, {clean} with no problems",
        outcomes.len(),
        outcomes.len() - unopened.len() - unparsed.len(),
        outcomes.len() - unchecked.len(),
    );
    for (kind, n) in &kinds {
        eprintln!("  {n} with {kind}");
    }
    for o in outcomes
        .iter()
        .filter(|o| !o.entry.path.contains("/safedocs/"))
    {
        if let Ok(r) = &o.checked
            && !r.is_clean()
        {
            eprintln!("{}:\n{r}", o.entry.path);
        }
    }
    for line in pages_differ.iter().chain(&unchecked).chain(&unparsed) {
        eprintln!("{line}");
    }
    assert!(
        unopened.is_empty(),
        "PDFium can't open:\n{}",
        unopened.join("\n")
    );
    assert!(
        pages_differ.is_empty(),
        "page counts differ from the manifest:\n{}",
        pages_differ.join("\n")
    );
    // lopdf can't read some of the SafeDocs sample, which is the web's
    // damaged files too; it reads every curated form.
    let curated: Vec<&String> = unchecked
        .iter()
        .filter(|l| !l.starts_with("forms/safedocs/"))
        .collect();
    assert!(curated.is_empty(), "not checked: {curated:#?}");
}
