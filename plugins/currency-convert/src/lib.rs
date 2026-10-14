//! Convert foreign-currency amounts. See README.md.

use std::collections::BTreeMap;

use serde::Deserialize;
use sheaf_plugin_api::types::{AnnotKind, Box2, Edit, PageAnnot, Rgb};
use sheaf_plugin_api::{Batch, FormBuilder, Plugin, document, http, storage, text, ui};

/// What the ECB publishes reference rates for (Frankfurter's list).
pub const CURRENCIES: &[&str] = &[
    "USD", "EUR", "GBP", "JPY", "CHF", "CAD", "AUD", "NZD", "CNY", "HKD", "SGD", "INR", "KRW",
    "MXN", "BRL", "ZAR", "SEK", "NOK", "DKK", "PLN", "CZK", "HUF", "RON", "BGN", "ISK", "TRY",
    "ILS", "IDR", "MYR", "PHP", "THB",
];

const DEFAULT_BASE: &str = "https://api.frankfurter.dev/v1";

/// Symbols and their currency, longest first so `CA$` wins over `$`.
const SYMBOLS: &[(&str, &str)] = &[
    ("US$", "USD"),
    ("CA$", "CAD"),
    ("AU$", "AUD"),
    ("NZ$", "NZD"),
    ("HK$", "HKD"),
    ("S$", "SGD"),
    ("C$", "CAD"),
    ("A$", "AUD"),
    ("€", "EUR"),
    ("£", "GBP"),
    ("¥", "JPY"),
    ("₹", "INR"),
    ("₩", "KRW"),
    ("$", "USD"),
];

/// Number as written in either style: 1,234.56 / 1.234,56 / 1 234,56 /
/// 1'234.56 / 1234.5.
const NUMBER: &str = r"\d{1,3}(?:[.,  ']\d{3})+(?:[.,]\d{1,2})?|\d+(?:[.,]\d{1,2})?";

/// The pattern for an amount: a symbol or code, then the number, or the
/// number, then the symbol or code.
pub fn amount_pattern() -> String {
    let mut markers: Vec<String> = SYMBOLS.iter().map(|(s, _)| regex_escape(s)).collect();
    markers.extend(CURRENCIES.iter().map(|c| format!(r"\b{c}\b")));
    let m = markers.join("|");
    format!(r"(?:(?P<pre>{m})\s?(?P<a>{NUMBER}))|(?:(?P<b>{NUMBER})\s?(?P<post>{m}))")
}

fn regex_escape(s: &str) -> String {
    s.chars()
        .map(|c| {
            if "$.^*+?()[]{}|\\".contains(c) {
                format!("\\{c}")
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// A currency code from a symbol or code as written.
pub fn currency_of(marker: &str) -> Option<&'static str> {
    let m = marker.trim();
    SYMBOLS
        .iter()
        .find(|(s, _)| *s == m)
        .map(|(_, c)| *c)
        .or_else(|| CURRENCIES.iter().copied().find(|c| *c == m))
}

/// A number in either thousands/decimal style. The last `.` or `,`
/// followed by one or two digits at the end is the decimal point; every
/// other separator groups thousands.
pub fn parse_number(s: &str) -> Option<f64> {
    let s = s.trim();
    let last_sep = s.rfind(['.', ',']);
    let (int_part, frac_part) = match last_sep {
        Some(i)
            if (1..=2).contains(&(s.len() - i - 1))
                && s[i + 1..].chars().all(|c| c.is_ascii_digit()) =>
        {
            (&s[..i], &s[i + 1..])
        }
        _ => (s, ""),
    };
    let digits: String = int_part.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    format!(
        "{digits}.{}",
        if frac_part.is_empty() { "0" } else { frac_part }
    )
    .parse()
    .ok()
}

/// Decimals a currency is shown with.
pub fn decimals(code: &str) -> usize {
    match code {
        "JPY" | "KRW" | "ISK" | "HUF" | "IDR" => 0,
        _ => 2,
    }
}

pub fn format_money(v: f64, code: &str) -> String {
    let d = decimals(code);
    let s = format!("{:.*}", d, v.abs());
    let (int, frac) = match s.split_once('.') {
        Some((i, f)) => (i.to_string(), format!(".{f}")),
        None => (s, String::new()),
    };
    let mut grouped = String::new();
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    format!("{code} {}{grouped}{frac}", if v < 0.0 { "-" } else { "" })
}

/// Today as YYYY-MM-DD (UTC), from the WASI clock.
fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    civil_date((secs / 86_400) as i64)
}

/// Days since 1970-01-01 → YYYY-MM-DD (Howard Hinnant's algorithm).
pub fn civil_date(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}")
}

pub fn valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && s.chars()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

#[derive(Debug, Deserialize)]
struct RateReply {
    date: String,
    rates: BTreeMap<String, f64>,
}

/// A rate and the date the ECB published it for.
#[derive(Debug, Clone, PartialEq)]
pub struct Rate {
    pub rate: f64,
    pub date: String,
}

fn base_url() -> String {
    serde_json::from_str::<serde_json::Value>(&storage::settings())
        .ok()
        .and_then(|v| v.get("base_url")?.as_str().map(str::to_string))
        .unwrap_or_else(|| DEFAULT_BASE.to_string())
        .trim_end_matches('/')
        .to_string()
}

/// `from` → `to` on `date` ("latest" or YYYY-MM-DD), from storage when
/// looked up before.
fn rate(from: &str, to: &str, date: &str) -> Result<Rate, String> {
    let key = format!("rate:{date}:{from}:{to}");
    if let Some(cached) = storage::get(&key)
        && let Some((r, d)) = cached.split_once(' ')
        && let Ok(rate) = r.parse()
    {
        return Ok(Rate {
            rate,
            date: d.to_string(),
        });
    }
    let url = format!("{}/{date}?from={from}&to={to}", base_url());
    let resp = http::get(&url)?;
    if resp.status != 200 {
        return Err(format!(
            "rates for {from}→{to} on {date}: HTTP {}",
            resp.status
        ));
    }
    let reply: RateReply =
        serde_json::from_slice(&resp.body).map_err(|e| format!("rates reply: {e}"))?;
    let rate = *reply
        .rates
        .get(to)
        .ok_or_else(|| format!("no {from}→{to} rate for {date}"))?;
    let _ = storage::set(&key, &format!("{rate} {}", reply.date));
    Ok(Rate {
        rate,
        date: reply.date,
    })
}

/// One amount found on a page.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub page: u64,
    pub currency: &'static str,
    pub value: f64,
    pub text: String,
    pub rects: Vec<Box2>,
}

/// Amounts in a pattern match's capture groups: (pre, a, b, post).
pub fn found_from(
    page: u64,
    text: &str,
    groups: &[Option<String>],
    rects: Vec<Box2>,
) -> Option<Found> {
    let g = |i: usize| groups.get(i).and_then(|g| g.as_deref());
    let (marker, number) = match (g(0), g(1), g(2), g(3)) {
        (Some(m), Some(n), _, _) => (m, n),
        (_, _, Some(n), Some(m)) => (m, n),
        _ => return None,
    };
    Some(Found {
        page,
        currency: currency_of(marker)?,
        value: parse_number(number)?,
        text: text.trim().to_string(),
        rects,
    })
}

fn overlaps(a: &Box2, b: &Box2) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

const GREEN: Rgb = Rgb {
    r: 120,
    g: 220,
    b: 140,
};

fn run() -> Result<(), String> {
    let pages = document::pages();
    if pages.is_empty() {
        return Err("open a document first".into());
    }
    let last_target = storage::get("target").unwrap_or_else(|| "USD".into());
    let default = CURRENCIES
        .iter()
        .position(|c| *c == last_target)
        .unwrap_or(0);
    let Some(a) = FormBuilder::new("Convert Currency")
        .choice("target", "Convert to", CURRENCIES, default)
        .text("date", "Rate date", "")
        .pages("pages", "Pages")
        .submit_label("Convert")
        .show()
    else {
        return Ok(());
    };
    let target = CURRENCIES[a
        .choice("target")
        .unwrap_or(default)
        .min(CURRENCIES.len() - 1)];
    let _ = storage::set("target", target);
    let date = match a.text("date").map(str::trim) {
        None | Some("") => "latest".to_string(),
        Some("today") => today(),
        Some(d) if valid_date(d) => d.to_string(),
        Some(d) => return Err(format!("“{d}” isn't a date like 2025-12-31")),
    };
    let scan: Vec<u64> = match a.pages("pages") {
        Some(p) => p.to_vec(),
        None => pages.iter().map(|p| p.id).collect(),
    };

    let pattern = amount_pattern();
    let mut found = Vec::new();
    for page in pages.iter().filter(|p| scan.contains(&p.id)) {
        // Highlights this plugin already left for the target currency.
        let done: Vec<Box2> = document::annotations(page.id)
            .into_iter()
            .filter(|an| an.contents.starts_with(&format!("≈ {target} ")))
            .filter_map(|an| match an.kind {
                AnnotKind::Highlight(r) => r.first().copied(),
                _ => None,
            })
            .collect();
        for m in text::pattern_search(page.id, &pattern)? {
            let Some(f) = found_from(page.id, &m.text, &m.groups, m.rects.clone()) else {
                continue;
            };
            if f.currency == target || f.rects.iter().any(|r| done.iter().any(|d| overlaps(r, d))) {
                continue;
            }
            found.push(f);
        }
    }
    if found.is_empty() {
        ui::notice(&format!("No amounts to convert to {target}"));
        return Ok(());
    }

    let mut rates: BTreeMap<&str, Rate> = BTreeMap::new();
    for f in &found {
        if !rates.contains_key(f.currency) {
            rates.insert(f.currency, rate(f.currency, target, &date)?);
        }
    }
    let n = found.len();
    let mut batch = Batch::new(format!(
        "Convert {n} amount{} to {target}",
        if n == 1 { "" } else { "s" }
    ));
    for f in &found {
        let r = &rates[f.currency];
        let mut annot = sheaf_plugin_api::annot(AnnotKind::Highlight(f.rects.clone()));
        annot.color = GREEN;
        annot.opacity = 0.5;
        annot.contents = format!(
            "≈ {} ({} × {}, ECB {})",
            format_money(f.value * r.rate, target),
            format_money(f.value, f.currency),
            r.rate,
            r.date
        );
        batch = batch.push(Edit::AnnotAdd(PageAnnot {
            page: f.page,
            annot,
        }));
    }
    batch.submit();
    let dates: Vec<&str> = {
        let mut d: Vec<&str> = rates.values().map(|r| r.date.as_str()).collect();
        d.dedup();
        d
    };
    ui::notice(&format!(
        "Converted {n} amount{} to {target} at ECB rates for {}",
        if n == 1 { "" } else { "s" },
        dates.join(", ")
    ));
    Ok(())
}

struct CurrencyConvert;

impl Plugin for CurrencyConvert {
    fn run_command(id: String, _args: Option<String>) -> Result<(), String> {
        match id.as_str() {
            "convert" => run(),
            other => Err(format!("unknown command {other}")),
        }
    }
}

sheaf_plugin_api::export!(CurrencyConvert);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_in_both_styles() {
        assert_eq!(parse_number("1,234.56"), Some(1234.56));
        assert_eq!(parse_number("1.234,56"), Some(1234.56));
        assert_eq!(parse_number("1 234,5"), Some(1234.5));
        assert_eq!(parse_number("1'234.56"), Some(1234.56));
        assert_eq!(
            parse_number("1.234"),
            Some(1234.0),
            "three digits: thousands"
        );
        assert_eq!(parse_number("12,000"), Some(12000.0));
        assert_eq!(parse_number("980"), Some(980.0));
        assert_eq!(parse_number("75.5"), Some(75.5));
        assert_eq!(parse_number("x"), None);
    }

    #[test]
    fn markers_and_patterns() {
        assert_eq!(currency_of("€"), Some("EUR"));
        assert_eq!(currency_of("C$"), Some("CAD"));
        assert_eq!(currency_of("$"), Some("USD"));
        assert_eq!(currency_of("GBP"), Some("GBP"));
        assert_eq!(currency_of("XYZ"), None);
        let pattern = amount_pattern();
        let re = regex_lite(&pattern);
        let hits: Vec<(String, Vec<Option<String>>)> =
            re("Paid €1.234,56 and GBP 980.00, then 1 200,50 CHF and C$75; ref 2025")
                .into_iter()
                .collect();
        let got: Vec<(&str, f64)> = hits
            .iter()
            .filter_map(|(t, g)| found_from(0, t, g, vec![]))
            .map(|f| (f.currency, f.value))
            .collect();
        assert_eq!(
            got,
            [
                ("EUR", 1234.56),
                ("GBP", 980.0),
                ("CHF", 1200.5),
                ("CAD", 75.0)
            ]
        );
    }

    /// The host runs the pattern with Rust's `regex`; tests use the same.
    fn regex_lite(p: &str) -> impl Fn(&str) -> Vec<(String, Vec<Option<String>>)> {
        let re = regex::Regex::new(p).unwrap();
        move |s: &str| {
            re.captures_iter(s)
                .map(|c| {
                    (
                        c[0].to_string(),
                        c.iter()
                            .skip(1)
                            .map(|g| g.map(|g| g.as_str().to_string()))
                            .collect(),
                    )
                })
                .collect()
        }
    }

    #[test]
    fn money_formats_per_currency() {
        assert_eq!(format_money(1352.104, "USD"), "USD 1,352.10");
        assert_eq!(format_money(150000.4, "JPY"), "JPY 150,000");
        assert_eq!(format_money(-5.0, "EUR"), "EUR -5.00");
    }

    #[test]
    fn dates() {
        assert_eq!(civil_date(0), "1970-01-01");
        assert_eq!(civil_date(20_454), "2026-01-01");
        assert!(valid_date("2025-12-31"));
        assert!(!valid_date("12/31/2025"));
    }
}
