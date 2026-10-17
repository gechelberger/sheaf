//! Split a document into several files (Document > Split Document…,
//! `sheaf split`): every N pages, by page ranges, at each top-level
//! bookmark, or into files no bigger than a size. Each part keeps its
//! pages' markup and the bookmarks that point into it.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use in_progress::{Level, Progress, Unit};
use serde::{Deserialize, Serialize};
use sheaf_doc::{PAGES, PageId, Snapshot};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SplitBy {
    /// Parts of this many pages (the last may be shorter).
    EveryN(usize),
    /// One part per range, e.g. ["1-3", "4-10", "11-"].
    Ranges(Vec<String>),
    /// A part per top-level bookmark, from its page to the next one's.
    TopBookmarks,
    /// Parts as large as fit in this many bytes.
    MaxBytes(u64),
}

/// One file to write: its name (no extension) and pages.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub name: String,
    pub pages: Vec<PageId>,
}

/// The parts `by` makes of `snap`, named after `stem`. Size splits need
/// to measure, so they're planned by [`split`] instead.
pub fn plan(snap: &Snapshot, by: &SplitBy, stem: &str) -> Result<Vec<Part>> {
    let pages: Vec<PageId> = snap.state.pages.iter().map(|p| p.id).collect();
    if pages.is_empty() {
        bail!("the document has no pages");
    }
    let numbered = |groups: Vec<Vec<PageId>>| {
        let width = groups.len().to_string().len();
        groups
            .into_iter()
            .enumerate()
            .map(|(i, pages)| Part {
                name: format!("{stem}_Part_{:0width$}", i + 1),
                pages,
            })
            .collect::<Vec<_>>()
    };
    Ok(match by {
        SplitBy::EveryN(n) => {
            if *n == 0 {
                bail!("parts need at least one page");
            }
            numbered(pages.chunks(*n).map(|c| c.to_vec()).collect())
        }
        SplitBy::Ranges(ranges) => {
            let mut groups = Vec::new();
            for r in ranges.iter().filter(|r| !r.trim().is_empty()) {
                let idx = crate::pages::parse(r, pages.len()).map_err(anyhow::Error::msg)?;
                groups.push(idx.into_iter().map(|i| pages[i]).collect());
            }
            if groups.is_empty() {
                bail!("no page ranges given");
            }
            numbered(groups)
        }
        SplitBy::TopBookmarks => by_bookmarks(snap, &pages, stem)?,
        SplitBy::MaxBytes(_) => bail!("a size split is planned while it's written"),
    })
}

fn by_bookmarks(snap: &Snapshot, pages: &[PageId], stem: &str) -> Result<Vec<Part>> {
    // (first page index, title) per top-level bookmark with a target.
    let mut starts: Vec<(usize, String)> = snap
        .state
        .outline
        .iter()
        .filter_map(|o| {
            let at = pages.iter().position(|p| Some(*p) == o.target)?;
            Some((at, o.title.clone()))
        })
        .collect();
    starts.sort_by_key(|(at, _)| *at);
    starts.dedup_by_key(|(at, _)| *at);
    if starts.is_empty() {
        bail!("the document has no top-level bookmarks to split at");
    }
    let mut parts = Vec::new();
    let mut used: Vec<String> = Vec::new();
    let mut name = |title: &str| {
        let base = format!("{stem}_{}", file_safe(title));
        let mut n = base.clone();
        let mut k = 2;
        while used.iter().any(|u| u.eq_ignore_ascii_case(&n)) {
            n = format!("{base}_{k}");
            k += 1;
        }
        used.push(n.clone());
        n
    };
    if starts[0].0 > 0 {
        parts.push(Part {
            name: name("Start"),
            pages: pages[..starts[0].0].to_vec(),
        });
    }
    for (i, (at, title)) in starts.iter().enumerate() {
        let end = starts.get(i + 1).map(|(e, _)| *e).unwrap_or(pages.len());
        parts.push(Part {
            name: name(title),
            pages: pages[*at..end].to_vec(),
        });
    }
    Ok(parts)
}

/// A bookmark title as a file name: no path separators or characters
/// Windows refuses, spaces kept, at most 80 characters.
pub fn file_safe(title: &str) -> String {
    let s: String = title
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let s = s.trim().trim_matches('.').trim();
    let s: String = s.chars().take(80).collect();
    if s.is_empty() {
        "Untitled".into()
    } else {
        s.trim_end().to_string()
    }
}

/// Split `snap` into files in `dir` named after `stem`. Returns them in
/// order. With `force`, existing files are replaced; otherwise that's an
/// error before anything is written.
pub fn split(
    snap: &Snapshot,
    by: &SplitBy,
    dir: &Path,
    stem: &str,
    force: bool,
    progress: &dyn Progress,
) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir)?;
    let parts = match by {
        SplitBy::MaxBytes(max) => by_size(snap, *max, stem, &*progress.child("measuring", 0.5))?,
        _ => plan(snap, by, stem)?,
    };
    let paths: Vec<PathBuf> = parts
        .iter()
        .map(|p| dir.join(format!("{}.pdf", p.name)))
        .collect();
    if !force && let Some(p) = paths.iter().find(|p| p.exists()) {
        bail!("{} already exists", p.display());
    }
    let step = progress.child("writing", 1.0);
    step.set_total(Some(parts.len() as u64), Unit::ITEMS);
    for (i, (part, path)) in parts.iter().zip(&paths).enumerate() {
        step.check()?;
        sheaf_doc::save::extract_pages(snap, &part.pages, path)?;
        step.set(i as u64 + 1);
    }
    Ok(paths)
}

/// Greedy parts no bigger than `max` bytes: each takes as many pages as
/// fit, found by doubling then bisecting. A page too big on its own is a
/// part by itself, with a warning.
fn by_size(snap: &Snapshot, max: u64, stem: &str, progress: &dyn Progress) -> Result<Vec<Part>> {
    let pages: Vec<PageId> = snap.state.pages.iter().map(|p| p.id).collect();
    if pages.is_empty() {
        bail!("the document has no pages");
    }
    if max == 0 {
        bail!("the size limit must be more than zero");
    }
    progress.set_total(Some(pages.len() as u64), PAGES);
    let fits = |from: usize, to: usize| -> Result<bool> {
        progress.check()?;
        Ok(sheaf_doc::save::extract_bytes(snap, &pages[from..to])?.len() as u64 <= max)
    };
    let mut groups = Vec::new();
    let mut start = 0;
    while start < pages.len() {
        let n = pages.len() - start;
        let take = if !fits(start, start + 1)? {
            progress.message(
                Level::Warn,
                &format!("page {} alone is over the limit", start + 1),
            );
            1
        } else if fits(start, pages.len())? {
            n
        } else {
            // good fits, bad doesn't.
            let (mut good, mut bad) = (1, 2.min(n));
            while bad < n && fits(start, start + bad)? {
                good = bad;
                bad = (bad * 2).min(n);
            }
            while bad - good > 1 {
                let mid = (good + bad) / 2;
                if fits(start, start + mid)? {
                    good = mid;
                } else {
                    bad = mid;
                }
            }
            good
        };
        groups.push(pages[start..start + take].to_vec());
        start += take;
        progress.set(start as u64);
    }
    let width = groups.len().to_string().len();
    Ok(groups
        .into_iter()
        .enumerate()
        .map(|(i, pages)| Part {
            name: format!("{stem}_Part_{:0width$}", i + 1),
            pages,
        })
        .collect())
}

/// "10MB", "500 kB", "2.5M", or plain bytes.
pub fn parse_size(s: &str) -> Result<u64, String> {
    let t = s.trim().to_ascii_lowercase().replace(' ', "");
    let digits = t
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(t.len());
    let (num, unit) = t.split_at(digits);
    let n: f64 = num.parse().map_err(|_| format!("'{s}' isn't a size"))?;
    let mult = match unit {
        "" | "b" => 1.0,
        "k" | "kb" => 1e3,
        "m" | "mb" => 1e6,
        "g" | "gb" => 1e9,
        "kib" => 1024.0,
        "mib" => 1024.0 * 1024.0,
        "gib" => 1024.0 * 1024.0 * 1024.0,
        _ => return Err(format!("'{s}': use B, KB, MB or GB")),
    };
    let bytes = (n * mult).round();
    if bytes < 1.0 {
        return Err(format!("'{s}' is too small"));
    }
    Ok(bytes as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sheaf_doc::{Document, Edit, SourceInfo, Store};

    fn doc(n: u32) -> Document {
        let store = Store::new();
        let primary = SourceInfo {
            name: "x.pdf".into(),
            ..SourceInfo::blank(&store, n)
        };
        Document::new(store, primary, &[], sheaf_doc::history::default_policy())
    }

    fn sizes(parts: &[Part]) -> Vec<usize> {
        parts.iter().map(|p| p.pages.len()).collect()
    }

    #[test]
    fn every_n_and_ranges() {
        let d = doc(10);
        let s = d.snapshot();
        let parts = plan(&s, &SplitBy::EveryN(4), "Return").unwrap();
        assert_eq!(sizes(&parts), vec![4, 4, 2]);
        assert_eq!(parts[0].name, "Return_Part_1");
        let parts = plan(
            &s,
            &SplitBy::Ranges(vec!["1-2".into(), "5, 9-".into()]),
            "R",
        )
        .unwrap();
        assert_eq!(sizes(&parts), vec![2, 3]);
        assert!(plan(&s, &SplitBy::Ranges(vec!["11".into()]), "R").is_err());
        let parts = plan(&doc(12).snapshot(), &SplitBy::EveryN(1), "R").unwrap();
        assert_eq!(parts[0].name, "R_Part_01");
    }

    #[test]
    fn top_bookmarks() {
        let mut d = doc(6);
        let ids: Vec<_> = d.state().pages.iter().map(|p| p.id).collect();
        for (title, page) in [("Form 1040", 1), ("Schedule A/B", 3), ("Form 1040", 4)] {
            let after = d.state().outline.last().map(|o| o.id);
            d.apply(
                Edit::OutlineInsert {
                    parent: None,
                    after,
                    title: title.into(),
                    target: Some(ids[page]),
                    view: None,
                },
                "b",
            )
            .unwrap();
        }
        let parts = plan(&d.snapshot(), &SplitBy::TopBookmarks, "Ret").unwrap();
        let names: Vec<_> = parts.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "Ret_Start",
                "Ret_Form 1040",
                "Ret_Schedule A-B",
                "Ret_Form 1040_2"
            ]
        );
        assert_eq!(sizes(&parts), vec![1, 2, 1, 2]);
    }

    #[test]
    fn sizes_parse() {
        assert_eq!(parse_size("10MB"), Ok(10_000_000));
        assert_eq!(parse_size("2.5 m"), Ok(2_500_000));
        assert_eq!(parse_size("1MiB"), Ok(1_048_576));
        assert_eq!(parse_size("500"), Ok(500));
        assert!(parse_size("ten").is_err());
        assert!(parse_size("5 TB").is_err());
    }
}
