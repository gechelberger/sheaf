//! Page labels (`/PageLabels` in the catalog): the "iv" or "A-3" a page
//! shows instead of its position. Read from files, kept in the model by
//! page id ([`LabelSpec`]) so they survive reorders, and written back.

use std::collections::HashSet;

use lopdf::{Dictionary, Object, ObjectId};
use serde::{Deserialize, Serialize};

use crate::attachments::{Objects, once, resolve};
use crate::model::PageId;

/// How a range numbers its pages (`/S`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Style {
    Decimal,
    UpperRoman,
    LowerRoman,
    UpperLetters,
    LowerLetters,
    /// No number, only the prefix.
    None,
}

/// One entry of the number tree: pages from `start` on, until the next.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabelRange {
    pub start: u32,
    pub style: Style,
    pub prefix: String,
    /// The number of the range's first page (`/St`, default 1).
    pub first: u32,
}

/// The label ranges in the catalog's `/PageLabels`, sorted by start page.
pub fn label_ranges<P: Objects + ?Sized>(pdf: &P) -> Option<Vec<LabelRange>> {
    let catalog = pdf.catalog().ok()?;
    let mut seen = HashSet::new();
    let tree = once(pdf, &mut seen, catalog.get(b"PageLabels").ok()?)?
        .as_dict()
        .ok()?;
    let mut nums = Vec::new();
    collect_nums(pdf, tree, &mut nums, 0, &mut seen);
    let mut ranges: Vec<LabelRange> = nums
        .into_iter()
        .filter_map(|(start, o)| {
            let d = resolve(pdf, o)?.as_dict().ok()?;
            Some(range_from(pdf, start, d))
        })
        .collect();
    if ranges.is_empty() {
        return None;
    }
    ranges.sort_by_key(|r| r.start);
    ranges.dedup_by_key(|r| r.start);
    Some(ranges)
}

/// Labels for `page_count` pages. Pages before the first range (a
/// malformed tree) get plain numbers.
pub fn labels_for(ranges: &[LabelRange], page_count: u32) -> Vec<String> {
    (0..page_count)
        .map(|p| {
            let at = ranges.partition_point(|r| r.start <= p);
            match at.checked_sub(1).map(|i| &ranges[i]) {
                Some(r) => format_label(r, p - r.start),
                None => (p + 1).to_string(),
            }
        })
        .collect()
}

/// A label range anchored to the page it starts at, as the model keeps
/// it: reordering pages carries the range start along.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabelSpec {
    pub start: PageId,
    pub style: Style,
    pub prefix: String,
    pub first: u32,
}

/// `specs` as ranges over page positions (`pages` in document order):
/// sorted, a start whose page is gone dropped, and plain numbers from the
/// first page when no range starts there (the PDF needs one at 0).
pub fn resolve_specs(specs: &[LabelSpec], pages: &[PageId]) -> Vec<LabelRange> {
    let mut ranges: Vec<LabelRange> = specs
        .iter()
        .filter_map(|s| {
            let start = pages.iter().position(|p| *p == s.start)? as u32;
            Some(LabelRange {
                start,
                style: s.style,
                prefix: s.prefix.clone(),
                first: s.first.max(1),
            })
        })
        .collect();
    ranges.sort_by_key(|r| r.start);
    ranges.dedup_by_key(|r| r.start);
    if !ranges.is_empty() && ranges[0].start != 0 {
        ranges.insert(
            0,
            LabelRange {
                start: 0,
                style: Style::Decimal,
                prefix: String::new(),
                first: 1,
            },
        );
    }
    ranges
}

/// Back to specs, by the page at each start.
pub fn to_specs(ranges: &[LabelRange], pages: &[PageId]) -> Vec<LabelSpec> {
    ranges
        .iter()
        .filter_map(|r| {
            Some(LabelSpec {
                start: *pages.get(r.start as usize)?,
                style: r.style,
                prefix: r.prefix.clone(),
                first: r.first,
            })
        })
        .collect()
}

/// Number pages `from..=to` (positions) with `new`, as Acrobat's Page
/// Labels dialog does: ranges starting inside are replaced, and the page
/// after keeps the label it had, so numbering past the change is
/// unchanged.
pub fn apply(
    ranges: &[LabelRange],
    page_count: u32,
    from: u32,
    to: u32,
    style: Style,
    prefix: &str,
    first: u32,
) -> Vec<LabelRange> {
    let covering = |p: u32| {
        let at = ranges.partition_point(|r| r.start <= p);
        at.checked_sub(1).map(|i| ranges[i].clone())
    };
    let after = (to + 1 < page_count).then(|| {
        let next = to + 1;
        match covering(next) {
            Some(r) => LabelRange {
                start: next,
                first: r.first.saturating_add(next - r.start),
                ..r
            },
            // No labels there before: plain numbers, as they showed.
            None => LabelRange {
                start: next,
                style: Style::Decimal,
                prefix: String::new(),
                first: next + 1,
            },
        }
    });
    let mut out: Vec<LabelRange> = ranges
        .iter()
        .filter(|r| r.start < from || r.start > to + 1)
        .cloned()
        .collect();
    if from > 0 && !out.iter().any(|r| r.start == 0) {
        out.push(LabelRange {
            start: 0,
            style: Style::Decimal,
            prefix: String::new(),
            first: 1,
        });
    }
    out.push(LabelRange {
        start: from,
        style,
        prefix: prefix.to_string(),
        first: first.max(1),
    });
    if let Some(a) = after
        && !ranges.iter().any(|r| r.start == to + 1)
    {
        out.push(a);
    } else if let Some(r) = ranges.iter().find(|r| r.start == to + 1) {
        out.push(r.clone());
    }
    out.sort_by_key(|r| r.start);
    out.dedup_by_key(|r| r.start);
    out
}

/// The `/PageLabels` number tree for `ranges`.
pub fn number_tree(ranges: &[LabelRange]) -> Dictionary {
    let mut nums = Vec::with_capacity(ranges.len() * 2);
    for r in ranges {
        let mut d = Dictionary::new();
        let s: Option<&[u8]> = match r.style {
            Style::Decimal => Some(b"D"),
            Style::UpperRoman => Some(b"R"),
            Style::LowerRoman => Some(b"r"),
            Style::UpperLetters => Some(b"A"),
            Style::LowerLetters => Some(b"a"),
            Style::None => None,
        };
        if let Some(s) = s {
            d.set("S", Object::Name(s.to_vec()));
        }
        if !r.prefix.is_empty() {
            d.set("P", crate::save::text_string(&r.prefix));
        }
        if r.first != 1 {
            d.set("St", r.first as i64);
        }
        nums.push(Object::Integer(r.start as i64));
        nums.push(Object::Dictionary(d));
    }
    let mut tree = Dictionary::new();
    tree.set("Nums", nums);
    tree
}

/// The label of the page `offset` pages into range `r`.
pub fn format_label(r: &LabelRange, offset: u32) -> String {
    let n = r.first.saturating_add(offset);
    let number = match r.style {
        Style::Decimal => n.to_string(),
        Style::UpperRoman => roman(n),
        Style::LowerRoman => roman(n).to_lowercase(),
        Style::UpperLetters => letters(n),
        Style::LowerLetters => letters(n).to_lowercase(),
        Style::None => String::new(),
    };
    format!("{}{number}", r.prefix)
}

fn range_from<P: Objects + ?Sized>(pdf: &P, start: u32, d: &Dictionary) -> LabelRange {
    let style = match d.get(b"S").ok().and_then(|o| resolve(pdf, o)) {
        Some(Object::Name(n)) => match n.as_slice() {
            b"D" => Style::Decimal,
            b"R" => Style::UpperRoman,
            b"r" => Style::LowerRoman,
            b"A" => Style::UpperLetters,
            b"a" => Style::LowerLetters,
            _ => Style::None,
        },
        _ => Style::None,
    };
    let prefix = d
        .get(b"P")
        .ok()
        .and_then(|o| resolve(pdf, o))
        .map(crate::file::pdf_text)
        .unwrap_or_default();
    let first = d
        .get(b"St")
        .ok()
        .and_then(|o| resolve(pdf, o))
        .and_then(|o| o.as_i64().ok())
        .filter(|&n| n >= 1)
        .map_or(1, |n| n.min(u32::MAX as i64) as u32);
    LabelRange {
        start,
        style,
        prefix,
        first,
    }
}

/// Walk a number tree: `/Nums [key value …]` at leaves, `/Kids` above,
/// each node or array reached by reference once.
fn collect_nums<'a, P: Objects + ?Sized>(
    pdf: &'a P,
    node: &'a Dictionary,
    out: &mut Vec<(u32, &'a Object)>,
    depth: u8,
    seen: &mut HashSet<ObjectId>,
) {
    if depth > 32 {
        return;
    }
    if let Some(Object::Array(nums)) = node.get(b"Nums").ok().and_then(|o| once(pdf, seen, o)) {
        for pair in nums.as_chunks::<2>().0 {
            if let Some(k) = resolve(pdf, &pair[0]).and_then(|o| o.as_i64().ok())
                && (0..=u32::MAX as i64).contains(&k)
            {
                out.push((k as u32, &pair[1]));
            }
        }
    }
    if let Some(Object::Array(kids)) = node.get(b"Kids").ok().and_then(|o| once(pdf, seen, o)) {
        for kid in kids {
            if let Some(Ok(d)) = once(pdf, seen, kid).map(Object::as_dict) {
                collect_nums(pdf, d, out, depth + 1, seen);
            }
        }
    }
}

fn roman(mut n: u32) -> String {
    const DIGITS: [(u32, &str); 13] = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut s = String::new();
    for (v, d) in DIGITS {
        while n >= v {
            s.push_str(d);
            n -= v;
        }
    }
    s
}

/// A, B, …, Z, AA, BB, …, ZZ, AAA: the PDF spec's letter numbering.
fn letters(n: u32) -> String {
    if n == 0 {
        return String::new();
    }
    let ch = (b'A' + ((n - 1) % 26) as u8) as char;
    std::iter::repeat_n(ch, ((n - 1) / 26 + 1) as usize).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{Document as Pdf, dictionary};

    fn range(start: u32, style: Style, prefix: &str, first: u32) -> LabelRange {
        LabelRange {
            start,
            style,
            prefix: prefix.into(),
            first,
        }
    }

    #[test]
    fn styles_format() {
        assert_eq!(roman(1994), "MCMXCIV");
        assert_eq!(letters(1), "A");
        assert_eq!(letters(27), "AA");
        assert_eq!(letters(53), "AAA");
        let r = range(0, Style::LowerRoman, "", 1);
        assert_eq!(format_label(&r, 3), "iv");
        let r = range(0, Style::Decimal, "A-", 7);
        assert_eq!(format_label(&r, 0), "A-7");
        let r = range(0, Style::None, "Cover", 1);
        assert_eq!(format_label(&r, 0), "Cover");
    }

    #[test]
    fn ranges_cover_the_document() {
        let ranges = [
            range(0, Style::LowerRoman, "", 1),
            range(3, Style::Decimal, "", 1),
        ];
        assert_eq!(labels_for(&ranges, 5), ["i", "ii", "iii", "1", "2"]);
        // A tree that doesn't start at 0 numbers the leading pages plainly.
        let ranges = [range(2, Style::UpperLetters, "", 1)];
        assert_eq!(labels_for(&ranges, 4), ["1", "2", "A", "B"]);
    }

    #[test]
    fn reads_a_number_tree_with_kids() {
        let mut pdf = Pdf::with_version("1.7");
        let leaf_a = pdf.add_object(dictionary! {
            "Nums" => vec![0.into(), dictionary! { "S" => "r" }.into()],
        });
        let roman_style = pdf.add_object(
            dictionary! { "S" => "D", "St" => 5, "P" => Object::string_literal("Tab ") },
        );
        let leaf_b = pdf.add_object(dictionary! {
            "Nums" => vec![2.into(), roman_style.into()],
        });
        let tree = pdf.add_object(dictionary! { "Kids" => vec![leaf_a.into(), leaf_b.into()] });
        let pages = pdf.add_object(
            dictionary! { "Type" => "Pages", "Kids" => Vec::<Object>::new(), "Count" => 0 },
        );
        let catalog = pdf.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages,
            "PageLabels" => tree,
        });
        pdf.trailer.set("Root", catalog);
        let ranges = label_ranges(&pdf).unwrap();
        assert_eq!(labels_for(&ranges, 4), ["i", "ii", "Tab 5", "Tab 6"]);
        // Read from a source, as a file opens: the same ranges.
        let src = crate::open::tests::lazily(&mut pdf);
        assert_eq!(label_ranges(src.pdf()), label_ranges(&pdf));
    }

    #[test]
    fn no_labels_is_none() {
        let mut pdf = Pdf::with_version("1.7");
        let catalog = pdf.add_object(dictionary! { "Type" => "Catalog" });
        pdf.trailer.set("Root", catalog);
        assert!(label_ranges(&pdf).is_none());
    }
}

#[cfg(test)]
mod edit_tests {
    use super::*;
    use lopdf::Document as Pdf;

    fn r(start: u32, style: Style, prefix: &str, first: u32) -> LabelRange {
        LabelRange {
            start,
            style,
            prefix: prefix.into(),
            first,
        }
    }

    #[test]
    fn apply_keeps_numbering_after_the_change() {
        // 10 pages, plain numbers.
        let out = apply(&[], 10, 0, 3, Style::LowerRoman, "", 1);
        assert_eq!(
            labels_for(&out, 10),
            vec!["i", "ii", "iii", "iv", "5", "6", "7", "8", "9", "10"]
        );
        // Then restart at 1 from page 5.
        let out = apply(&out, 10, 4, 9, Style::Decimal, "", 1);
        assert_eq!(labels_for(&out, 10)[4..7], ["1", "2", "3"]);
        // A prefixed run in the middle leaves the rest counting on.
        let out = apply(&out, 10, 6, 7, Style::UpperLetters, "Ex-", 1);
        assert_eq!(
            labels_for(&out, 10),
            vec!["i", "ii", "iii", "iv", "1", "2", "Ex-A", "Ex-B", "5", "6"]
        );
    }

    #[test]
    fn specs_follow_their_pages() {
        let pages: Vec<PageId> = (0..5).map(PageId).collect();
        let ranges = vec![r(0, Style::LowerRoman, "", 1), r(2, Style::Decimal, "", 1)];
        let specs = to_specs(&ranges, &pages);
        // Move the page that starts the decimal range to the front.
        let moved = vec![pages[2], pages[0], pages[1], pages[3], pages[4]];
        let back = resolve_specs(&specs, &moved);
        assert_eq!(labels_for(&back, 5), vec!["1", "i", "ii", "iii", "iv"]);
        // The roman range's start page deleted: it goes, and the first
        // page gets plain numbers, since the PDF needs a range there.
        let gone = vec![pages[1], pages[2], pages[3]];
        let back = resolve_specs(&specs, &gone);
        assert_eq!(back[0].start, 0);
        assert_eq!(back[0].style, Style::Decimal);
        assert_eq!(labels_for(&back, 3), vec!["1", "1", "2"]);
    }

    #[test]
    fn number_tree_round_trips() {
        let ranges = vec![
            r(0, Style::LowerRoman, "", 1),
            r(3, Style::Decimal, "A-", 5),
        ];
        let tree = number_tree(&ranges);
        let mut pdf = Pdf::with_version("1.7");
        let tree_id = pdf.add_object(tree);
        let catalog =
            pdf.add_object(lopdf::dictionary! { "Type" => "Catalog", "PageLabels" => tree_id });
        pdf.trailer.set("Root", catalog);
        assert_eq!(label_ranges(&pdf).unwrap(), ranges);
    }
}
