//! Cross-reference amounts that appear on more than one page. See README.md.

use std::collections::BTreeMap;
use std::sync::Mutex;

use sheaf_plugin_api::types::{
    Annot, AnnotKind, Box2, ConnectorPair, ConnectorShape, Edit, PageAnnot, Rgb,
};
use sheaf_plugin_api::ui::{
    self, Field, FieldKind, FieldValue, Form, NumberField, PanelRow, RowAction,
};
use sheaf_plugin_api::{Guest, document, edits, text};

/// Money as written in returns and statements: "$1,234.56", "(1,234)",
/// "12,500", "980.10". Bare integers without a separator are too noisy
/// (line numbers, years) to count.
const AMOUNT: &str = r"\(?\$?\s?\d{1,3}(?:,\d{3})+(?:\.\d{2})?\)?|\(?\$?\s?\d+\.\d{2}\)?";

const PANEL: &str = "pairs";
/// Connector side, as sheaf's own connector tool draws them.
const CONNECTOR: f32 = 22.0;
const BLUE: Rgb = Rgb {
    r: 0,
    g: 120,
    b: 255,
};

/// An amount in cents, from its text. Parentheses mean negative.
pub fn parse_amount(s: &str) -> Option<i64> {
    let negative = s.contains('(') && s.contains(')');
    let digits: String = s
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let (whole, cents) = match digits.split_once('.') {
        Some((w, c)) if c.len() == 2 => (w, c.parse::<i64>().ok()?),
        Some(_) => return None,
        None => (digits.as_str(), 0),
    };
    let v = whole
        .parse::<i64>()
        .ok()?
        .checked_mul(100)?
        .checked_add(cents)?;
    Some(if negative { -v } else { v })
}

pub fn format_amount(cents: i64) -> String {
    let abs = cents.unsigned_abs();
    let whole = (abs / 100).to_string();
    let mut grouped = String::new();
    for (i, ch) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    let s = format!("${grouped}.{:02}", abs % 100);
    if cents < 0 { format!("({s})") } else { s }
}

/// Where an amount was seen.
#[derive(Debug, Clone, PartialEq)]
pub struct Seen {
    pub page: u64,
    /// Display index, for labels.
    pub index: usize,
    pub rect: Box2,
}

/// A candidate: the same amount on two pages.
#[derive(Debug, Clone, PartialEq)]
pub struct Pair {
    pub cents: i64,
    pub a: Seen,
    pub b: Seen,
}

impl Pair {
    pub fn key(&self) -> String {
        format!("{}:{}:{}", self.cents, self.a.page, self.b.page)
    }
}

/// Pair each amount's first sighting on a page with its first sighting on
/// the next page it appears on, in page order. Amounts below `min_cents`,
/// and multiples of $100 when `skip_round`, are left out.
pub fn find_pairs(seen: &[(i64, Seen)], min_cents: i64, skip_round: bool) -> Vec<Pair> {
    let mut by_amount: BTreeMap<i64, Vec<&Seen>> = BTreeMap::new();
    for (cents, s) in seen {
        if cents.abs() < min_cents || (skip_round && cents % 10_000 == 0) {
            continue;
        }
        let list = by_amount.entry(*cents).or_default();
        if !list.iter().any(|x| x.page == s.page) {
            list.push(s);
        }
    }
    let mut pairs = Vec::new();
    for (cents, mut list) in by_amount {
        list.sort_by_key(|s| s.index);
        for w in list.windows(2) {
            pairs.push(Pair {
                cents,
                a: w[0].clone(),
                b: w[1].clone(),
            });
        }
    }
    pairs.sort_by_key(|p| (p.a.index, p.b.index, p.cents));
    pairs
}

fn overlaps(a: &Box2, b: &Box2) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

/// The connector square just right of an amount.
pub fn connector_rect(amount: &Box2) -> Box2 {
    Box2 {
        x: amount.x + amount.w + 3.0,
        y: amount.y + amount.h / 2.0 - CONNECTOR / 2.0,
        w: CONNECTOR,
        h: CONNECTOR,
    }
}

/// Pairs waiting in the panel, for the document they were found in.
static PENDING: Mutex<Option<(Option<String>, Vec<Pair>)>> = Mutex::new(None);

fn pending() -> Vec<Pair> {
    let path = document::path();
    match &*PENDING.lock().unwrap() {
        Some((p, pairs)) if *p == path => pairs.clone(),
        _ => Vec::new(),
    }
}

fn set_pending(pairs: Vec<Pair>) {
    let rows: Vec<PanelRow> = pairs
        .iter()
        .map(|p| PanelRow {
            id: p.key(),
            label: format_amount(p.cents),
            detail: format!("page {} ⇄ page {}", p.a.index + 1, p.b.index + 1),
            badge: None,
            icon: ui::Icon::Link,
            depth: 0,
            page: Some(p.a.page),
            actions: vec![
                RowAction {
                    id: "link".into(),
                    label: "Link".into(),
                },
                RowAction {
                    id: "dismiss".into(),
                    label: "Dismiss".into(),
                },
            ],
        })
        .collect();
    ui::set_panel(PANEL, &rows);
    *PENDING.lock().unwrap() = Some((document::path(), pairs));
}

/// The next free connector number in the document.
fn next_label() -> u32 {
    document::pages()
        .iter()
        .flat_map(|p| document::annotations(p.id))
        .filter_map(|a| match a.kind {
            AnnotKind::Connector(c) => c.label.trim().parse::<u32>().ok(),
            _ => None,
        })
        .max()
        .unwrap_or(0)
        + 1
}

fn connector(page: u64, amount: &Box2, label: &str) -> PageAnnot {
    PageAnnot {
        page,
        annot: Annot {
            id: 0,
            kind: AnnotKind::Connector(ConnectorShape {
                rect: connector_rect(amount),
                label: label.into(),
                partner: None,
            }),
            color: BLUE,
            opacity: 1.0,
            stroke: 1.5,
            author: String::new(),
            contents: format!("Amount cross-reference {label}"),
        },
    }
}

fn link(pairs: &[Pair]) -> Vec<Edit> {
    let first = next_label();
    pairs
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let label = (first + i as u32).to_string();
            Edit::ConnectorAdd(ConnectorPair {
                a: connector(p.a.page, &p.a.rect, &label),
                b: connector(p.b.page, &p.b.rect, &label),
            })
        })
        .collect()
}

fn find() -> Result<(), String> {
    let pages = document::pages();
    if pages.is_empty() {
        return Err("open a document first".into());
    }
    let form = Form {
        title: "Find Matching Amounts".into(),
        fields: vec![
            Field {
                id: "min".into(),
                label: "Smallest amount".into(),
                kind: FieldKind::Number(NumberField {
                    default: 100.0,
                    min: Some(0.0),
                    max: None,
                }),
            },
            Field {
                id: "pages".into(),
                label: "Pages".into(),
                kind: FieldKind::PageRange(String::new()),
            },
            Field {
                id: "round".into(),
                label: "Skip round hundreds".into(),
                kind: FieldKind::Checkbox(true),
            },
        ],
        submit_label: Some("Find".into()),
    };
    let Some(answers) = ui::show_form(&form) else {
        return Ok(());
    };
    let mut min_cents = 10_000;
    let mut skip_round = true;
    let mut scan: Vec<u64> = pages.iter().map(|p| p.id).collect();
    for a in answers {
        match (a.id.as_str(), a.value) {
            ("min", FieldValue::Number(n)) => min_cents = (n * 100.0).round() as i64,
            ("round", FieldValue::Checkbox(b)) => skip_round = b,
            ("pages", FieldValue::PageRange(p)) => scan = p,
            _ => {}
        }
    }
    let mut seen = Vec::new();
    for (index, p) in pages.iter().enumerate() {
        if !scan.contains(&p.id) {
            continue;
        }
        let connectors: Vec<Box2> = document::annotations(p.id)
            .into_iter()
            .filter_map(|a| match a.kind {
                AnnotKind::Connector(c) => Some(c.rect),
                _ => None,
            })
            .collect();
        for m in text::pattern_search(p.id, AMOUNT)? {
            let (Some(cents), Some(rect)) = (parse_amount(&m.text), m.rects.first().copied())
            else {
                continue;
            };
            // Already cross-referenced: a connector sits where ours would.
            let ours = connector_rect(&rect);
            if connectors.iter().any(|c| overlaps(c, &ours)) {
                continue;
            }
            seen.push((
                cents,
                Seen {
                    page: p.id,
                    index,
                    rect,
                },
            ));
        }
    }
    let pairs = find_pairs(&seen, min_cents, skip_round);
    let n = pairs.len();
    set_pending(pairs);
    ui::notice(&match n {
        0 => "No matching amounts found".to_string(),
        1 => "1 matching pair — see Amount Matches".to_string(),
        n => format!("{n} matching pairs — see Amount Matches"),
    });
    Ok(())
}

struct AmountCrossref;

impl Guest for AmountCrossref {
    fn activate() -> Result<(), String> {
        Ok(())
    }

    fn run_command(id: String, _args_json: Option<String>) -> Result<(), String> {
        match id.as_str() {
            "find" => find(),
            "link-all" => {
                let pairs = pending();
                if pairs.is_empty() {
                    return Err("nothing to link; run Find Matching Amounts first".into());
                }
                let n = pairs.len();
                edits::submit(
                    &format!("Link {n} amount{}", if n == 1 { "" } else { "s" }),
                    &link(&pairs),
                );
                set_pending(Vec::new());
                Ok(())
            }
            other => Err(format!("unknown command {other}")),
        }
    }

    fn panel_action(panel: String, row: String, action: String) -> Result<(), String> {
        if panel != PANEL {
            return Err(format!("unknown panel {panel}"));
        }
        let mut pairs = pending();
        let at = pairs
            .iter()
            .position(|p| p.key() == row)
            .ok_or("that match is gone; run Find Matching Amounts again")?;
        let pair = pairs.remove(at);
        match action.as_str() {
            "link" => edits::submit(
                &format!("Link {}", format_amount(pair.cents)),
                &link(std::slice::from_ref(&pair)),
            ),
            "dismiss" => {}
            other => return Err(format!("unknown action {other}")),
        }
        set_pending(pairs);
        Ok(())
    }
}

sheaf_plugin_api::export!(AmountCrossref);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_parse_and_format() {
        assert_eq!(parse_amount("$1,234.56"), Some(123456));
        assert_eq!(parse_amount("(1,234)"), Some(-123400));
        assert_eq!(parse_amount("12,500"), Some(1250000));
        assert_eq!(parse_amount("980.10"), Some(98010));
        assert_eq!(parse_amount("1.5"), None);
        assert_eq!(format_amount(123456), "$1,234.56");
        assert_eq!(format_amount(-1250000), "($12,500.00)");
        assert_eq!(format_amount(5), "$0.05");
    }

    fn seen(page: u64, index: usize) -> Seen {
        Seen {
            page,
            index,
            rect: Box2 {
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 10.0,
            },
        }
    }

    #[test]
    fn pairs_chain_across_pages_once_per_page() {
        let s = vec![
            (123456, seen(10, 0)),
            (123456, seen(10, 0)), // twice on one page: one sighting
            (123456, seen(30, 2)),
            (123456, seen(20, 1)),
            (5000, seen(10, 0)), // below the minimum
            (5000, seen(20, 1)),
            (2000000, seen(10, 0)), // round
            (2000000, seen(20, 1)),
            (777777, seen(40, 3)), // only one page
        ];
        let pairs = find_pairs(&s, 10_000, true);
        let got: Vec<(i64, usize, usize)> = pairs
            .iter()
            .map(|p| (p.cents, p.a.index, p.b.index))
            .collect();
        assert_eq!(got, [(123456, 0, 1), (123456, 1, 2)]);
        let with_round = find_pairs(&s, 10_000, false);
        assert_eq!(with_round.len(), 3);
    }

    #[test]
    fn connectors_sit_right_of_the_amount() {
        let r = connector_rect(&seen(1, 0).rect);
        assert_eq!((r.x, r.y, r.w), (13.0, -6.0, CONNECTOR));
    }
}
