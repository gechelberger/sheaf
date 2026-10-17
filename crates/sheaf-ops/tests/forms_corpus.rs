//! Every form in Greg's corpus opens, and the form check runs on it (M10
//! slice 1). What it finds is the baseline later slices compare with:
//! printed here (`--nocapture`), and recorded in `todos/m10.md`'s slice
//! notes. And every form reads as its manifest row says (slice 2). Skips
//! when there's no corpus (see `sheaf_doc::forms::corpus`).
//!
//! Needs libpdfium (`scripts/fetch-pdfium.sh`), like the app itself.

use std::collections::BTreeMap;
use std::sync::Mutex;

use sheaf_core::PageIndex;
use sheaf_doc::forms::corpus::{Corpus, Entry};
use sheaf_doc::forms::{FieldKind, FormFacts, Report, check};
use sheaf_doc::{SourceInfo, Store};
use sheaf_render::pdfium::{PdfiumEngine, render_appearances};
use sheaf_render::{FormType, Renderer};

/// What became of one file.
struct Outcome {
    entry: Entry,
    /// Its pages and its kind of form, as PDFium opened it.
    opened: Result<(u32, FormType), String>,
    /// The check, and the form as the field model reads it.
    checked: Result<(Report, FormFacts), String>,
}

/// Open the file in PDFium as the app does and, if `draw`, draw its first
/// page's appearances. PDFium is called from one thread only: sheaf
/// holds one lock across every use of it (slice 1's note in
/// `todos/m10.md`), so more threads wouldn't open files faster.
fn open(bytes: &[u8], draw: bool) -> Result<(u32, FormType), String> {
    let engine = PdfiumEngine::open_bytes(bytes.into(), None);
    engine
        .and_then(|e| {
            for i in 0..e.page_count() {
                e.page_size(PageIndex(i))?;
            }
            if draw {
                render_appearances(bytes, None, PageIndex(0), 100)?;
            }
            Ok((e.page_count(), e.form_type()))
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
                    let path = corpus.path(entry);
                    let result = std::fs::read(&path)
                        .map_err(anyhow::Error::from)
                        .and_then(|bytes| {
                            let report = check(&bytes, None)?;
                            let info = SourceInfo::read(&Store::new(), &path, None)?;
                            Ok((report, sheaf_doc::forms::read(&info.src).facts))
                        })
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
    let mut xfa_differs = Vec::new();
    let mut clean = 0;
    let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
    for o in &outcomes {
        match &o.opened {
            // pypdf can't read these either: a sample of damaged files.
            Err(e) if o.entry.kind == "unparsed" => unparsed.push(format!("{}: {e}", o.entry.path)),
            Err(e) => unopened.push(format!("{}: {e}", o.entry.path)),
            Ok((n, _)) if o.entry.pages > 0 && *n != o.entry.pages => {
                pages_differ.push(format!("{}: {n}, not {}", o.entry.path, o.entry.pages))
            }
            Ok(_) => {}
        }
        // PDFium's form type says what the field model's reading does of
        // XFA.
        if let (Ok((_, form_type)), Ok((_, facts))) = (&o.opened, &o.checked) {
            let xfa = match (facts.xfa, facts.dynamic) {
                (true, true) => FormType::XfaFull,
                (true, false) => FormType::XfaForeground,
                (false, _) => FormType::None,
            };
            let pdfium = match form_type {
                FormType::AcroForm => FormType::None,
                t => *t,
            };
            if xfa != pdfium {
                let class = facts.class.label();
                xfa_differs.push(format!("{}: {form_type:?}, {class}", o.entry.path));
            }
        }
        match &o.checked {
            Err(e) => unchecked.push(format!("{} ({}): {e}", o.entry.path, o.entry.kind)),
            Ok((r, _)) if r.is_clean() => clean += 1,
            Ok((r, _)) => {
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
        if let Ok((r, _)) = &o.checked
            && !r.is_clean()
        {
            eprintln!("{}:\n{r}", o.entry.path);
        }
    }
    for line in pages_differ
        .iter()
        .chain(&unchecked)
        .chain(&unparsed)
        .chain(&xfa_differs)
    {
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
    assert!(
        xfa_differs.is_empty(),
        "PDFium's form type differs:\n{}",
        xfa_differs.join("\n")
    );
    // lopdf can't read some of the SafeDocs sample, which is the web's
    // damaged files too; it reads every curated form.
    let curated: Vec<&String> = unchecked
        .iter()
        .filter(|l| !l.starts_with("forms/safedocs/"))
        .collect();
    assert!(curated.is_empty(), "not checked: {curated:#?}");
}

/// The manifest's columns from what sheaf reads: `kind` (by the manifest's
/// rule: XFA and no fields is `xfa-only`), `fields`, `field_types` and
/// `field_scripts`.
fn as_manifest(facts: &FormFacts) -> (String, u32, Counts, Counts) {
    let kind = match (facts.xfa, facts.fields) {
        (true, 0) => "xfa-only",
        (true, _) => "hybrid",
        (false, _) => "acroform",
    };
    let mut types = Counts::new();
    for (kind, n) in &facts.kinds {
        if *kind != FieldKind::Unknown {
            *types.entry(kind.short().to_string()).or_default() += *n as u32;
        }
    }
    let s = &facts.scripts;
    let scripts = [
        ("calculate", s.calculate),
        ("format", s.format),
        ("keystroke", s.keystroke),
        ("validate", s.validate),
    ];
    let scripts = scripts
        .into_iter()
        .filter(|&(_, n)| n > 0)
        .map(|(k, n)| (k.to_string(), n as u32))
        .collect();
    (kind.into(), facts.fields as u32, types, scripts)
}

type Counts = BTreeMap<String, u32>;

/// Every file's form, read as sheaf reads it when it opens (M10 slice 2),
/// has the manifest's kind, field count, fields by type and fields with
/// scripts. The manifest was made with pypdf, which counts a field listed
/// twice twice, and lopdf misreads a few of the SafeDocs sample (a lost
/// catalog, a dropped object); so every curated form must read,
/// undamaged, as its row, and 99% of the sample. What differs is listed,
/// and so are damaged trees, which sheaf doesn't read.
#[test]
fn every_form_reads_as_its_manifest_row() {
    let Some(corpus) = Corpus::or_skip() else {
        return;
    };
    let entries: Vec<Entry> = corpus
        .manifest()
        .unwrap()
        .into_iter()
        .filter(|e| e.kind != "unparsed")
        .collect();
    let queue = Mutex::new(entries.iter());
    let differ = Mutex::new(Vec::new());
    let damaged = Mutex::new(Vec::new());
    let unread = Mutex::new(Vec::new());
    let classes = Mutex::new(BTreeMap::<&str, usize>::new());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(16));
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let Some(entry) = queue.lock().unwrap().next() else {
                        break;
                    };
                    let info = SourceInfo::read(&Store::new(), &corpus.path(entry), None);
                    let info = match info {
                        Ok(info) => info,
                        Err(e) => {
                            unread
                                .lock()
                                .unwrap()
                                .push(format!("{}: {e:#}", entry.path));
                            continue;
                        }
                    };
                    let facts = sheaf_doc::forms::read(&info.src).facts;
                    *classes
                        .lock()
                        .unwrap()
                        .entry(facts.class.label())
                        .or_default() += 1;
                    if let Some(why) = &facts.damaged {
                        damaged
                            .lock()
                            .unwrap()
                            .push(format!("{}: {why}", entry.path));
                        continue;
                    }
                    let want = (
                        entry.kind.clone(),
                        entry.fields,
                        entry.field_types.clone(),
                        entry.field_scripts.clone(),
                    );
                    let got = as_manifest(&facts);
                    if got != want {
                        differ.lock().unwrap().push((
                            entry.path.clone(),
                            format!("  manifest {want:?}\n  sheaf    {got:?}"),
                        ));
                    }
                }
            });
        }
    });
    let (differ, damaged, unread) = (
        differ.into_inner().unwrap(),
        damaged.into_inner().unwrap(),
        unread.into_inner().unwrap(),
    );
    eprintln!(
        "{} files read: {} as their manifest rows, {} damaged, {} unread",
        entries.len() - unread.len(),
        entries.len() - unread.len() - damaged.len() - differ.len(),
        damaged.len(),
        unread.len()
    );
    for (class, n) in classes.into_inner().unwrap() {
        eprintln!("  {n} {class}");
    }
    for line in damaged.iter().chain(&unread) {
        eprintln!("{line}");
    }
    for (path, how) in &differ {
        eprintln!("{path}:\n{how}");
    }
    let sample = |path: &str| path.starts_with("forms/safedocs/");
    let curated: Vec<&str> = differ
        .iter()
        .map(|(p, _)| p.as_str())
        .chain(damaged.iter().chain(&unread).map(String::as_str))
        .filter(|p| !sample(p))
        .collect();
    assert!(
        curated.is_empty(),
        "differ from the manifest, damaged or unread: {curated:#?}"
    );
    let read = entries.iter().filter(|e| sample(&e.path)).count() - unread.len();
    let off = differ.len() + damaged.len();
    assert!(
        off * 100 <= read,
        "{off} of {read} SafeDocs files differ from the manifest or are damaged"
    );
}
