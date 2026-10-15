//! The calculator tape, as PDFlyer prints it: sections of numbered rows.
//!
//! A section starts with an optional title row (`1-0`), runs through its
//! entries (`1-1`, `1-2`…), may print subtotals (◊) along the way, and
//! closes with its total (`1-T`, marked `*`); the next section is `2-`.
//! A grand total (`GT`) adds up the section totals so far. Any row can
//! carry a note.
//!
//! Totals are worked out from the entries ([`Tape::retotal`]), so lines
//! can be inserted and deleted anywhere. A placed tape keeps its rows in
//! the mark, and a text version in `/Contents` for other viewers; a tape
//! placed before rows existed is read back from that text
//! ([`Tape::from_text`]).

use serde::{Deserialize, Serialize};

/// What an entry does to its section's running total.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
}

impl Op {
    pub fn symbol(self) -> &'static str {
        match self {
            Op::Add => "+",
            Op::Sub => "−",
            Op::Mul => "×",
            Op::Div => "÷",
        }
    }

    fn apply(self, running: f64, v: f64) -> f64 {
        match self {
            Op::Add => running + v,
            Op::Sub => running - v,
            Op::Mul => running * v,
            Op::Div if v != 0.0 => running / v,
            Op::Div => running,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Row {
    /// A section's title (`1-0`).
    Title(String),
    /// An amount and what it does.
    Entry { value: f64, op: Op },
    /// The section's running total so far; the section goes on.
    Subtotal(f64),
    /// The section's total, closing it.
    Total(f64),
    /// The sum of the section totals so far.
    GrandTotal(f64),
}

impl Row {
    /// The amount it shows (a title shows none).
    pub fn value(&self) -> Option<f64> {
        match self {
            Row::Title(_) => None,
            Row::Entry { value, .. }
            | Row::Subtotal(value)
            | Row::Total(value)
            | Row::GrandTotal(value) => Some(*value),
        }
    }

    /// The symbol printed after the amount.
    pub fn symbol(&self) -> &'static str {
        match self {
            Row::Title(_) => "",
            Row::Entry { op, .. } => op.symbol(),
            Row::Subtotal(_) => "◊",
            Row::Total(_) => "*",
            Row::GrandTotal(_) => "GT",
        }
    }

    /// Printed bold, with a rule above.
    pub fn is_total(&self) -> bool {
        matches!(self, Row::Total(_) | Row::GrandTotal(_))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub row: Row,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

impl Line {
    pub fn new(row: Row) -> Self {
        Self {
            row,
            note: String::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Tape {
    pub lines: Vec<Line>,
}

impl Tape {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Work every subtotal, total and grand total out again from the
    /// entries, after lines are inserted or deleted.
    pub fn retotal(&mut self) {
        let mut running = 0.0;
        let mut started = false;
        let mut totals = 0.0;
        for line in &mut self.lines {
            match &mut line.row {
                Row::Title(_) => {}
                Row::Entry { value, op } => {
                    running = if started {
                        op.apply(running, *value)
                    } else {
                        // The first entry starts the section: ×5 or ÷5 of
                        // nothing would be nothing.
                        match op {
                            Op::Sub => -*value,
                            _ => *value,
                        }
                    };
                    started = true;
                }
                Row::Subtotal(v) => *v = running,
                Row::Total(v) => {
                    *v = running;
                    totals += running;
                    running = 0.0;
                    started = false;
                }
                Row::GrandTotal(v) => *v = totals,
            }
        }
    }

    /// The running total of the open section (what a total would print).
    pub fn running(&self) -> f64 {
        let mut t = self.clone();
        t.lines.push(Line::new(Row::Total(0.0)));
        t.retotal();
        t.lines.last().and_then(|l| l.row.value()).unwrap_or(0.0)
    }

    /// Whether the last section is still open (has rows after its last
    /// total).
    pub fn open(&self) -> bool {
        self.lines
            .iter()
            .rev()
            .take_while(|l| !matches!(l.row, Row::Total(_)))
            .any(|l| matches!(l.row, Row::Entry { .. }))
    }

    /// Each line's number, as PDFlyer prints them: `1-0` for a title,
    /// `1-1`… for the rows, `1-T` for the total, `GT` for a grand total.
    pub fn numbers(&self) -> Vec<String> {
        let mut section = 1;
        let mut n = 0;
        self.lines
            .iter()
            .map(|l| match &l.row {
                Row::Title(_) => format!("{section}-0"),
                Row::Entry { .. } | Row::Subtotal(_) => {
                    n += 1;
                    format!("{section}-{n}")
                }
                Row::Total(_) => {
                    let s = format!("{section}-T");
                    section += 1;
                    n = 0;
                    s
                }
                Row::GrandTotal(_) => "GT".into(),
            })
            .collect()
    }

    /// The tape as text, for `/Contents` and copying: number, amount (or
    /// title), symbol, and note, a line each.
    pub fn text(&self, decimals: usize) -> String {
        self.numbers()
            .iter()
            .zip(&self.lines)
            .map(|(n, l)| {
                let body = match &l.row {
                    Row::Title(t) => t.clone(),
                    row => format!(
                        "{} {}",
                        fmt_amount(row.value().unwrap_or(0.0), decimals),
                        row.symbol()
                    ),
                };
                if l.note.is_empty() {
                    format!("{n} {body}")
                } else {
                    format!("{n} {body}  ; {}", l.note)
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A tape from text: this tape's own (`1-1 1,234.00 +  ; note`) or a
    /// tape placed before rows were kept (`1,234.00 +`, `T` for a total).
    pub fn from_text(text: &str) -> Tape {
        let mut lines = Vec::new();
        for raw in text.lines() {
            let (body, note) = match raw.split_once("  ; ") {
                Some((b, n)) => (b.trim_end(), n.trim().to_string()),
                None => (raw.trim_end(), String::new()),
            };
            let mut words: Vec<&str> = body.split_whitespace().collect();
            // A leading row number: "1-0", "2-T", "GT".
            let number = words
                .first()
                .filter(|w| {
                    **w == "GT"
                        || w.split_once('-')
                            .is_some_and(|(s, _)| s.parse::<u32>().is_ok())
                })
                .map(|w| w.to_string());
            if number.is_some() {
                words.remove(0);
            }
            if number.as_deref().is_some_and(|n| n.ends_with("-0")) {
                lines.push(Line {
                    row: Row::Title(words.join(" ")),
                    note,
                });
                continue;
            }
            let (amount, symbol) = match words.as_slice() {
                [] => continue,
                [a] => (*a, ""),
                [a @ .., s] => (a.last().copied().unwrap_or_default(), *s),
            };
            let Some(value) = parse_amount(amount) else {
                continue;
            };
            let row = match symbol {
                "*" | "T" => Row::Total(value),
                "GT" => Row::GrandTotal(value),
                "◊" => Row::Subtotal(value),
                "-" | "−" => Row::Entry { value, op: Op::Sub },
                "×" | "x" => Row::Entry { value, op: Op::Mul },
                "÷" | "/" => Row::Entry { value, op: Op::Div },
                _ => Row::Entry { value, op: Op::Add },
            };
            lines.push(Line { row, note });
        }
        let mut tape = Tape { lines };
        tape.retotal();
        tape
    }
}

/// `decimals` places with thousands separators; negatives get a minus.
pub fn fmt_amount(v: f64, decimals: usize) -> String {
    let neg = v < 0.0 && v.abs() >= 0.5 * 10f64.powi(-(decimals as i32));
    let scale = 10f64.powi(decimals as i32);
    let units = (v.abs() * scale).round() as u128;
    let whole = units / scale as u128;
    let frac = units % scale as u128;
    let digits = whole.to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    if decimals > 0 {
        out.push('.');
        out.push_str(&format!("{frac:0decimals$}"));
    }
    format!("{}{out}", if neg { "-" } else { "" })
}

/// What people paste from workpapers: commas, `$`, spaces, and
/// accounting-style `(negative)`.
pub fn parse_amount(s: &str) -> Option<f64> {
    let cleaned: String = s
        .chars()
        .filter(|c| !matches!(c, ',' | '$' | ' '))
        .collect();
    let cleaned = cleaned.trim_start_matches('+');
    let (neg, body) = match cleaned.strip_prefix('(').and_then(|b| b.strip_suffix(')')) {
        Some(b) => (true, b),
        None => (false, cleaned),
    };
    let v: f64 = body.parse().ok()?;
    Some(if neg { -v } else { v })
}

/// How a placed tape looks (PDFlyer's options).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TapeLook {
    /// The `1-1` column.
    pub numbers: bool,
    /// Title rows.
    pub titles: bool,
    /// Turned a quarter: 0, 90 or 270 degrees clockwise.
    pub rotation: u16,
    /// The paper's color; the text is the mark's.
    pub paper: [u8; 3],
    /// Places after the point.
    pub decimals: usize,
}

impl Default for TapeLook {
    fn default() -> Self {
        Self {
            numbers: true,
            titles: true,
            rotation: 0,
            paper: [255, 255, 255],
            decimals: 2,
        }
    }
}

impl TapeLook {
    /// The lines it shows, with their numbers; titles are left out when
    /// they're hidden.
    pub fn shown<'a>(&self, tape: &'a Tape) -> Vec<(String, &'a Line)> {
        tape.numbers()
            .into_iter()
            .zip(&tape.lines)
            .filter(|(_, l)| self.titles || !matches!(l.row, Row::Title(_)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(value: f64, op: Op) -> Line {
        Line::new(Row::Entry { value, op })
    }

    fn sample() -> Tape {
        let mut t = Tape {
            lines: vec![
                Line::new(Row::Title("Cash".into())),
                entry(100.0, Op::Add),
                entry(50.0, Op::Add),
                Line::new(Row::Subtotal(0.0)),
                entry(30.0, Op::Sub),
                Line::new(Row::Total(0.0)),
                entry(10.0, Op::Add),
                entry(3.0, Op::Mul),
                Line::new(Row::Total(0.0)),
                Line::new(Row::GrandTotal(0.0)),
            ],
        };
        t.retotal();
        t
    }

    #[test]
    fn sections_total_and_number_as_pdflyer_prints_them() {
        let t = sample();
        let values: Vec<Option<f64>> = t.lines.iter().map(|l| l.row.value()).collect();
        assert_eq!(values[3], Some(150.0));
        assert_eq!(values[5], Some(120.0));
        assert_eq!(values[8], Some(30.0));
        assert_eq!(values[9], Some(150.0));
        assert_eq!(
            t.numbers(),
            [
                "1-0", "1-1", "1-2", "1-3", "1-4", "1-T", "2-1", "2-2", "2-T", "GT"
            ]
        );
        assert!(!t.open());
    }

    #[test]
    fn inserting_and_deleting_retotal() {
        let mut t = sample();
        t.lines.insert(2, entry(25.0, Op::Add));
        t.retotal();
        assert_eq!(t.lines[4].row.value(), Some(175.0));
        assert_eq!(t.lines[6].row.value(), Some(145.0));
        t.lines.remove(1);
        t.retotal();
        assert_eq!(t.lines[5].row.value(), Some(45.0));
    }

    #[test]
    fn the_text_reads_back_and_old_tapes_still_read() {
        let mut t = sample();
        t.lines[1].note = "petty cash".into();
        let text = t.text(2);
        assert!(
            text.starts_with("1-0 Cash\n1-1 100.00 +  ; petty cash"),
            "{text}"
        );
        assert_eq!(Tape::from_text(&text), t);
        // A tape from before rows: "<amount> <op>", T for a total.
        let old = Tape::from_text("100.00 +\n50.00 -\n50.00 T");
        assert_eq!(old.lines.len(), 3);
        assert_eq!(old.lines[2].row, Row::Total(50.0));
        assert_eq!(old.numbers(), ["1-1", "1-2", "1-T"]);
    }

    #[test]
    fn amounts_format_to_any_places() {
        assert_eq!(fmt_amount(1234.5, 2), "1,234.50");
        assert_eq!(fmt_amount(-1234567.891, 2), "-1,234,567.89");
        assert_eq!(fmt_amount(1234.5, 0), "1,235");
        assert_eq!(fmt_amount(-0.001, 2), "0.00");
        assert_eq!(fmt_amount(0.12345, 4), "0.1235");
        assert_eq!(parse_amount("(84.50)"), Some(-84.5));
    }
}
