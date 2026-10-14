//! Sum the numbers in the selected region. See README.md.

use std::collections::HashMap;
use std::sync::Mutex;

use sheaf_plugin_api::types::{Box2, DocEvent};
use sheaf_plugin_api::{Plugin, document, text, ui};

/// Fixed-point scale: 4 decimals.
const SCALE: i128 = 10_000;

/// A number as written: its value in 1/10,000ths and how many decimals it
/// showed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Amount {
    pub units: i128,
    pub decimals: u32,
}

/// Read a word as a number, accounting-style. None if it isn't one.
pub fn parse_amount(word: &str) -> Option<Amount> {
    let mut s = word.trim().trim_end_matches([',', ';', ':', '*']);
    if matches!(s, "-" | "–" | "—") {
        return Some(Amount {
            units: 0,
            decimals: 0,
        });
    }
    let mut negative = false;
    if let Some(inner) = s.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
        negative = true;
        s = inner;
    }
    for minus in ['-', '−'] {
        if let Some(r) = s.strip_prefix(minus) {
            negative = !negative;
            s = r;
        } else if let Some(r) = s.strip_suffix(minus) {
            negative = !negative;
            s = r;
        }
    }
    s = s.trim_start_matches(['$', '€', '£', '¥']).trim();
    if s.is_empty() || !s.starts_with(|c: char| c.is_ascii_digit() || c == '.') {
        return None;
    }
    let (whole, frac) = match s.split_once('.') {
        Some((w, f)) => (w, f),
        None => (s, ""),
    };
    if frac.contains(|c: char| !c.is_ascii_digit()) {
        return None;
    }
    // Commas only as thousands separators.
    let groups: Vec<&str> = whole.split(',').collect();
    let well_grouped = groups.len() == 1
        || (!groups[0].is_empty()
            && groups[0].len() <= 3
            && groups[1..].iter().all(|g| g.len() == 3));
    if !well_grouped
        || groups
            .iter()
            .any(|g| g.contains(|c: char| !c.is_ascii_digit()))
    {
        return None;
    }
    let digits: String = groups.concat();
    if digits.is_empty() && frac.is_empty() {
        return None;
    }
    let whole_units: i128 = if digits.is_empty() {
        0
    } else {
        digits.parse().ok()?
    };
    let decimals = frac.len() as u32;
    let mut frac_units: i128 = 0;
    for (i, c) in frac.chars().take(4).enumerate() {
        frac_units += (c as i128 - '0' as i128) * SCALE / 10i128.pow(i as u32 + 1);
    }
    // Round beyond four decimals.
    if frac.len() > 4 && frac.as_bytes()[4] >= b'5' {
        frac_units += 1;
    }
    let units = whole_units.checked_mul(SCALE)?.checked_add(frac_units)?;
    Some(Amount {
        units: if negative { -units } else { units },
        decimals: decimals.min(4),
    })
}

/// "12,345.67", with `decimals` places and thousands separators.
pub fn format_units(units: i128, decimals: u32) -> String {
    let neg = units < 0;
    let abs = units.unsigned_abs();
    let scale = SCALE as u128;
    // Round to the decimals shown.
    let step = 10u128.pow(4 - decimals);
    let rounded = (abs + step / 2) / step * step;
    let whole = (rounded / scale).to_string();
    let mut grouped = String::new();
    for (i, ch) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    let mut out = String::new();
    if neg && rounded > 0 {
        out.push('-');
    }
    out.push_str(&grouped);
    if decimals > 0 {
        let frac = (rounded % scale) / step;
        out.push_str(&format!(".{frac:0width$}", width = decimals as usize));
    }
    out
}

/// The status line for these amounts.
pub fn summary(amounts: &[Amount]) -> String {
    if amounts.is_empty() {
        return "Subtotal: no numbers in the selection".into();
    }
    let total: i128 = amounts.iter().map(|a| a.units).sum();
    let decimals = amounts.iter().map(|a| a.decimals).max().unwrap_or(0);
    let n = amounts.len();
    format!(
        "Sum {} · {n} number{}",
        format_units(total, decimals),
        if n == 1 { "" } else { "s" }
    )
}

fn centre_in(b: &Box2, r: &Box2) -> bool {
    let (cx, cy) = (b.x + b.w / 2.0, b.y + b.h / 2.0);
    cx >= r.x && cx <= r.x + r.w && cy >= r.y && cy <= r.y + r.h
}

/// Numbers on a page with their boxes.
type PageNumbers = Vec<(Amount, Box2)>;

/// Each page's numbers, for the document (path) they came from.
type Cache = Option<(Option<String>, HashMap<u64, PageNumbers>)>;

static CACHE: Mutex<Cache> = Mutex::new(None);

fn page_numbers(page: u64) -> Result<PageNumbers, String> {
    let path = document::path();
    let mut cache = CACHE.lock().unwrap();
    if cache.as_ref().is_none_or(|(p, _)| *p != path) {
        *cache = Some((path, HashMap::new()));
    }
    let pages = &mut cache.as_mut().unwrap().1;
    if let Some(n) = pages.get(&page) {
        return Ok(n.clone());
    }
    let numbers: PageNumbers = text::words(page, None)?
        .into_iter()
        .filter_map(|w| {
            let a = parse_amount(&w.text)?;
            let b = *w.rects.first()?;
            Some((a, b))
        })
        .collect();
    pages.insert(page, numbers.clone());
    Ok(numbers)
}

struct Subtotal;

impl Plugin for Subtotal {
    fn run_command(id: String, _args: Option<String>) -> Result<(), String> {
        Err(format!("unknown command {id}"))
    }

    fn on_event(event: DocEvent) -> Result<(), String> {
        match event {
            DocEvent::Region(r) => {
                let amounts: Vec<Amount> = page_numbers(r.page)?
                    .into_iter()
                    .filter(|(_, b)| centre_in(b, &r.rect))
                    .map(|(a, _)| a)
                    .collect();
                ui::notice(&summary(&amounts));
            }
            DocEvent::RegionCleared => ui::notice(""),
            // OCR results or page changes can change a page's numbers.
            DocEvent::Edited(_) => *CACHE.lock().unwrap() = None,
            _ => {}
        }
        Ok(())
    }
}

sheaf_plugin_api::export!(Subtotal);

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Option<String> {
        parse_amount(s).map(|a| format_units(a.units, a.decimals))
    }

    #[test]
    fn reads_accounting_numbers() {
        assert_eq!(v("$1,234.56"), Some("1,234.56".into()));
        assert_eq!(v("1,234"), Some("1,234".into()));
        assert_eq!(v("(1,234.00)"), Some("-1,234.00".into()));
        assert_eq!(v("1,234-"), Some("-1,234".into()));
        assert_eq!(v("-12.5"), Some("-12.5".into()));
        assert_eq!(v("—"), Some("0".into()));
        assert_eq!(v(".75"), Some("0.75".into()));
        assert_eq!(v("2025"), Some("2,025".into()), "every number counts");
        assert_eq!(v("1,234.56,"), Some("1,234.56".into()));
        for not in ["W-2", "1099-INT", "Form", "12%", "1,23,4", "1.2.3", "$", ""] {
            assert_eq!(v(not), None, "{not}");
        }
    }

    #[test]
    fn sums_exactly() {
        let a: Vec<Amount> = ["0.10", "0.20", "1,000", "(0.30)"]
            .iter()
            .map(|s| parse_amount(s).unwrap())
            .collect();
        assert_eq!(summary(&a), "Sum 1,000.00 · 4 numbers");
        let one = [parse_amount("5").unwrap()];
        assert_eq!(summary(&one), "Sum 5 · 1 number");
        assert_eq!(summary(&[]), "Subtotal: no numbers in the selection");
        // More than four decimals round at the fourth.
        assert_eq!(v("0.123456"), Some("0.1235".into()));
    }

    #[test]
    fn region_membership_by_centre() {
        let r = Box2 {
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
        };
        let inside = Box2 {
            x: 8.0,
            y: 2.0,
            w: 3.0,
            h: 2.0,
        };
        let outside = Box2 {
            x: 9.0,
            y: 2.0,
            w: 4.0,
            h: 2.0,
        };
        assert!(centre_in(&inside, &r));
        assert!(!centre_in(&outside, &r));
    }
}
