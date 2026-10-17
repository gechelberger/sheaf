//! Greg's corpus of real forms (M10 proposal 21): where it is, and its
//! `manifest.tsv`, a row per file under `forms/`.
//!
//! Tests look in `$SHEAF_CORPUS`, then in `../pdf-corpus` beside the
//! workspace, and skip, passing with a message, when neither is there
//! (CI has no corpus). Nothing is assumed about what it holds. Files
//! marked `restricted` are read in place: no copy, excerpt or rendering
//! of them is committed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};

pub struct Corpus {
    pub root: PathBuf,
}

/// One row of `manifest.tsv`. Columns it lacks are left empty.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Entry {
    /// Relative to the corpus root (`forms/irs/f1040.pdf`).
    pub path: String,
    pub form: String,
    pub title: String,
    pub pages: u32,
    /// `acroform`, `hybrid`, `xfa-only`, or `unparsed` (pypdf couldn't
    /// read it).
    pub kind: String,
    /// Terminal AcroForm fields.
    pub fields: u32,
    /// Fields by type: `text`, `check`, `radio`, `choice`, `push`, `sig`.
    pub field_types: BTreeMap<String, u32>,
    /// Fields with a script, by event: `calculate`, `format`,
    /// `keystroke`, `validate`.
    pub field_scripts: BTreeMap<String, u32>,
    pub source: String,
    /// `public-domain`, `restricted`, `unstated` or `unknown`.
    pub license: String,
    pub sha256: String,
}

impl Entry {
    pub fn restricted(&self) -> bool {
        self.license == "restricted"
    }
}

impl Corpus {
    /// `$SHEAF_CORPUS`, else `../pdf-corpus` beside the workspace: the
    /// first that has a `manifest.tsv`.
    pub fn find() -> Option<Corpus> {
        let beside = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../pdf-corpus");
        let env = std::env::var_os("SHEAF_CORPUS").map(PathBuf::from);
        Corpus::first_of(env.into_iter().chain([beside]))
    }

    /// [`Corpus::find`], or a line saying the test skips.
    pub fn or_skip() -> Option<Corpus> {
        Corpus::find().or_else(|| {
            eprintln!("skipped: no corpus (set SHEAF_CORPUS, or check out ../pdf-corpus)");
            None
        })
    }

    /// The first of `roots` with a `manifest.tsv`.
    pub(crate) fn first_of(roots: impl IntoIterator<Item = PathBuf>) -> Option<Corpus> {
        roots
            .into_iter()
            .find(|root| root.join("manifest.tsv").is_file())
            .map(|root| Corpus { root })
    }

    pub fn manifest(&self) -> Result<Vec<Entry>> {
        let path = self.root.join("manifest.tsv");
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        parse_manifest(&text).with_context(|| path.display().to_string())
    }

    /// The row for `path` (`forms/irs/f1040es.pdf`), if the corpus has it.
    pub fn entry(&self, path: &str) -> Result<Option<Entry>> {
        Ok(self.manifest()?.into_iter().find(|e| e.path == path))
    }

    /// Where an entry's file is.
    pub fn path(&self, entry: &Entry) -> PathBuf {
        self.root.join(&entry.path)
    }
}

/// `read` of each of `entries`, in their order, on a thread per core (16
/// at most).
pub fn each_entry<T: Send>(entries: &[Entry], read: impl Fn(&Entry) -> T + Sync) -> Vec<T> {
    let outcomes = Mutex::new(BTreeMap::new());
    let queue = Mutex::new(entries.iter().enumerate());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(16));
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let Some((i, entry)) = queue.lock().unwrap().next() else {
                        break;
                    };
                    let outcome = read(entry);
                    outcomes.lock().unwrap().insert(i, outcome);
                }
            });
        }
    });
    outcomes.into_inner().unwrap().into_values().collect()
}

/// `manifest.tsv`'s rows, by its header's column names.
pub fn parse_manifest(text: &str) -> Result<Vec<Entry>> {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let header: Vec<&str> = lines.next().context("no header")?.split('\t').collect();
    if !header.contains(&"path") {
        anyhow::bail!("no path column");
    }
    let mut out = Vec::new();
    for (n, line) in lines.enumerate() {
        let cells: BTreeMap<&str, &str> = header.iter().copied().zip(line.split('\t')).collect();
        let get = |key: &str| cells.get(key).copied().unwrap_or("").trim().to_string();
        let number = |key: &str| -> Result<u32> {
            let v = get(key);
            if v.is_empty() || v == "-" {
                return Ok(0);
            }
            v.parse()
                .with_context(|| format!("row {}: {key} {v:?}", n + 2))
        };
        out.push(Entry {
            path: get("path"),
            form: get("form"),
            title: get("title"),
            pages: number("pages")?,
            kind: get("kind"),
            fields: number("fields")?,
            field_types: counts(&get("field_types"))
                .with_context(|| format!("row {}: field_types", n + 2))?,
            field_scripts: counts(&get("field_scripts"))
                .with_context(|| format!("row {}: field_scripts", n + 2))?,
            source: get("source"),
            license: get("license"),
            sha256: get("sha256"),
        });
    }
    Ok(out)
}

/// `check:82,push:10,text:66` as counts; `-` or nothing as none.
fn counts(cell: &str) -> Result<BTreeMap<String, u32>> {
    let mut out = BTreeMap::new();
    if cell.is_empty() || cell == "-" {
        return Ok(out);
    }
    for item in cell.split(',') {
        let (key, n) = item.split_once(':').context(item.to_string())?;
        out.insert(key.trim().to_string(), n.trim().parse()?);
    }
    Ok(out)
}
