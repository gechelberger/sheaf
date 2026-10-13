//! Bookmark every tax form in the document. See README.md.

use std::sync::LazyLock;

use regex::Regex;
use sheaf_plugin_api::types::{Edit, OutlineInsert};
use sheaf_plugin_api::{Guest, document, edits, log, text};

/// Form headers, most specific first. Group 1 is the form's name.
static FORMS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)\bSchedule\s+(K-1)\b",
        r"(?i)\b(Schedule\s+(?:[A-Z]{1,2}|\d)(?:-[A-Z]+)?)\b(?:\s*\(Form\s+\d{4}\))?",
        r"(?i)\b(W-2G?)\b",
        r"(?i)\b(1099-[A-Z]+)\b",
        r"(?i)\b(1098(?:-[A-Z]+)?)\b",
        r"(?i)\bForm\s+(\d{4}(?:-[A-Z]+)?)\b",
    ]
    .iter()
    .map(|r| Regex::new(r).unwrap())
    .collect()
});

/// Labels of the box holding the issuer's name on information returns.
static PAYER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:payer|employer|filer|lender|partnership|corporation|estate or trust|recipient's lender)'?s?\s+name\b",
    )
    .unwrap()
});

/// The bookmark title for a page, if it starts a tax form: the form name,
/// normalized ("Schedule C", "1099-INT", "Form 1040"), plus " — <payer>"
/// for information returns when the payer's name can be found. Only the
/// first few lines count as a header, so a form merely mentioned in body
/// text doesn't match.
pub fn detect(page_text: &str) -> Option<String> {
    let header: Vec<&str> = page_text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(8)
        .collect();
    let name = header.iter().find_map(|line| {
        FORMS.iter().find_map(|re| {
            let m = re.captures(line)?.get(1)?.as_str();
            Some(normalize(m, re.as_str().contains("Form\\s+(")))
        })
    })?;
    let informational = ["W-2", "1099", "1098", "K-1"]
        .iter()
        .any(|p| name.starts_with(p) || name == "Schedule K-1");
    match informational.then(|| payer(page_text)).flatten() {
        Some(p) => Some(format!("{name} — {p}")),
        None => Some(name),
    }
}

fn normalize(m: &str, numbered_form: bool) -> String {
    let m = m.split_whitespace().collect::<Vec<_>>().join(" ");
    if numbered_form {
        return format!("Form {}", m.to_uppercase());
    }
    match m.split_once(' ') {
        // "SCHEDULE c" -> "Schedule C"
        Some((_, rest)) => format!("Schedule {}", rest.to_uppercase()),
        None => m.to_uppercase(),
    }
}

/// The line after a "PAYER'S name"-style label, trimmed of address-ish
/// noise.
fn payer(page_text: &str) -> Option<String> {
    let lines: Vec<&str> = page_text.lines().map(str::trim).collect();
    let at = lines.iter().position(|l| PAYER.is_match(l))?;
    // The name is often on the label's own line after a comma or colon.
    let label_line = lines[at];
    let after_label = PAYER
        .find(label_line)
        .map(|m| label_line[m.end()..].trim_start_matches([',', ':', ' ']))
        .filter(|rest| {
            let lower = rest.to_lowercase();
            rest.len() > 2
                && !["address", "street", "city", "zip", "telephone"]
                    .iter()
                    .any(|w| lower.contains(w))
        });
    let name = match after_label {
        Some(n) => n,
        None => lines[at + 1..].iter().find(|l| !l.is_empty())?,
    };
    let name = name.split("  ").next().unwrap_or(name).trim();
    (name.len() >= 2 && name.len() <= 60).then(|| name.to_string())
}

/// Titles to bookmark: (page index, title), skipping pages that continue
/// the previous page's form.
pub fn plan(pages: &[Option<String>]) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = Vec::new();
    let mut prev: Option<&String> = None;
    for (i, t) in pages.iter().enumerate() {
        if let Some(t) = t
            && prev != Some(t)
        {
            out.push((i, t.clone()));
        }
        prev = t.as_ref().or(prev);
    }
    out
}

struct TaxFormBookmarks;

impl Guest for TaxFormBookmarks {
    fn activate() -> Result<(), String> {
        Ok(())
    }

    fn run_command(id: String, _args_json: Option<String>) -> Result<(), String> {
        if id != "scan" {
            return Err(format!("unknown command {id}"));
        }
        let pages = document::pages();
        if pages.is_empty() {
            return Err("open a document first".into());
        }
        let titles: Vec<Option<String>> = pages
            .iter()
            .map(|p| match text::page_text(p.id) {
                Ok(t) => detect(&t),
                Err(e) => {
                    log::log(log::Level::Warn, &format!("page {}: {e}", p.id));
                    None
                }
            })
            .collect();
        let outline = document::outline();
        let exists = |title: &str, page: u64| {
            outline
                .iter()
                .any(|o| o.title == title && o.target == Some(page))
        };
        let found = plan(&titles);
        let new: Vec<(u64, String)> = found
            .into_iter()
            .map(|(i, t)| (pages[i].id, t))
            .filter(|(page, t)| !exists(t, *page))
            .collect();
        if new.is_empty() {
            return Err("no new tax forms found".into());
        }
        // Append after the last top-level bookmark. Every insert names the
        // same sibling, so insert in reverse to end up in page order.
        let last_top = outline.iter().filter(|o| o.depth == 0).last().map(|o| o.id);
        let batch: Vec<Edit> = new
            .into_iter()
            .rev()
            .map(|(page, title)| {
                Edit::OutlineInsert(OutlineInsert {
                    parent: None,
                    after: last_top,
                    title,
                    target: Some(page),
                })
            })
            .collect();
        let label = format!(
            "Bookmark {} tax form{}",
            batch.len(),
            if batch.len() == 1 { "" } else { "s" }
        );
        edits::submit(&label, &batch);
        Ok(())
    }
}

sheaf_plugin_api::export!(TaxFormBookmarks);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_common_forms() {
        let cases = [
            (
                "Form 1040 U.S. Individual Income Tax Return 2025",
                "Form 1040",
            ),
            ("form 1040-x\nAmended", "Form 1040-X"),
            (
                "SCHEDULE C\n(Form 1040)\nProfit or Loss From Business",
                "Schedule C",
            ),
            ("Schedule SE (Form 1040) 2025", "Schedule SE"),
            ("Schedule 1 (Form 1040)\nAdditional Income", "Schedule 1"),
            ("Form 8949 Sales and Other Dispositions", "Form 8949"),
        ];
        for (text, want) in cases {
            assert_eq!(detect(text).as_deref(), Some(want), "{text:?}");
        }
    }

    #[test]
    fn information_returns_carry_the_payer() {
        let t = "CORRECTED (if checked)\nPAYER'S name, street address, city\nJPMorgan Chase Bank, N.A.\nPO Box 659754\nForm 1099-INT Interest Income";
        assert_eq!(
            detect(t).as_deref(),
            Some("1099-INT — JPMorgan Chase Bank, N.A.")
        );
        let w2 = "Form W-2 Wage and Tax Statement 2025\nc Employer's name, address, and ZIP code\nAcme Corp\n123 Main St";
        assert_eq!(detect(w2).as_deref(), Some("W-2 — Acme Corp"));
        let k1 = "Schedule K-1\n(Form 1065)\nPartnership's name, address\nSmith Ventures LP";
        assert_eq!(detect(k1).as_deref(), Some("K-1 — Smith Ventures LP"));
        // No payer label: just the form.
        assert_eq!(detect("1099-DIV Dividends").as_deref(), Some("1099-DIV"));
    }

    #[test]
    fn body_mentions_dont_count() {
        let filler = "line\n".repeat(10);
        let t = format!("Dear client,\n{filler}See the Form 1040 instructions.");
        assert_eq!(detect(&t), None);
        assert_eq!(detect(""), None);
    }

    #[test]
    fn continuation_pages_are_skipped() {
        let pages = vec![
            Some("Form 1040".to_string()),
            Some("Form 1040".to_string()),
            None,
            Some("Schedule C".to_string()),
            None,
            Some("Schedule C".to_string()),
            Some("Form 1040".to_string()),
        ];
        assert_eq!(
            plan(&pages),
            [
                (0, "Form 1040".to_string()),
                (3, "Schedule C".to_string()),
                (6, "Form 1040".to_string())
            ]
        );
    }
}
