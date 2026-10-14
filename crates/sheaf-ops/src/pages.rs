//! Acrobat-style page ranges: "1-3, 7, 10-" (1-based, inclusive, an open
//! end runs to the last page, an open start from the first).

use sheaf_doc::{PageId, Snapshot};

/// Zero-based page indices, sorted and unique.
pub fn parse(s: &str, page_count: usize) -> Result<Vec<usize>, String> {
    let mut out = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (a, b) = match part.split_once('-') {
            Some((a, b)) => (a.trim(), Some(b.trim())),
            None => (part, None),
        };
        let num = |t: &str| -> Result<usize, String> {
            let n: usize = t
                .parse()
                .map_err(|_| format!("'{part}' is not a page or range"))?;
            if n == 0 || n > page_count {
                return Err(format!("page {n} is out of range (1–{page_count})"));
            }
            Ok(n)
        };
        let start = match (a, b) {
            ("", Some(_)) => 1,
            _ => num(a)?,
        };
        let end = match b {
            Some("") => page_count,
            Some(b) => num(b)?,
            None => start,
        };
        if end < start {
            return Err(format!("'{part}' runs backwards"));
        }
        out.extend((start - 1)..end);
    }
    if out.is_empty() {
        return Err("no pages given".into());
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// The pages a range picks in `snapshot`; `None` means all of them.
pub fn ids(snapshot: &Snapshot, range: Option<&str>) -> Result<Vec<PageId>, String> {
    let pages = &snapshot.state.pages;
    Ok(match range {
        None => pages.iter().map(|p| p.id).collect(),
        Some(r) => parse(r, pages.len())?
            .into_iter()
            .map(|i| pages[i].id)
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn parses_ranges() {
        assert_eq!(
            parse("1-3, 7, 10-", 12).unwrap(),
            vec![0, 1, 2, 6, 9, 10, 11]
        );
        assert_eq!(parse("-2", 6).unwrap(), vec![0, 1]);
        assert_eq!(parse("3,1,3", 5).unwrap(), vec![0, 2]);
        assert!(parse("", 5).is_err());
        assert!(parse("0", 5).is_err());
        assert!(parse("6", 5).is_err());
        assert!(parse("4-2", 5).is_err());
        assert!(parse("x", 5).is_err());
    }
}
