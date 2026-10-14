//! Page labels (`/PageLabels` in the catalog): the "iv" or "A-3" a page
//! shows instead of its position. Read only; writing them is M8.

use lopdf::{Dictionary, Document as Pdf, Object};

/// How a range numbers its pages (`/S`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq)]
pub struct LabelRange {
    pub start: u32,
    pub style: Style,
    pub prefix: String,
    /// The number of the range's first page (`/St`, default 1).
    pub first: u32,
}

/// The label of every page, or None when the file has no page labels.
pub fn page_labels(pdf: &Pdf, page_count: u32) -> Option<Vec<String>> {
    let ranges = label_ranges(pdf)?;
    Some(labels_for(&ranges, page_count))
}

/// The label ranges in the catalog's `/PageLabels`, sorted by start page.
pub fn label_ranges(pdf: &Pdf) -> Option<Vec<LabelRange>> {
    let catalog = pdf.catalog().ok()?;
    let tree = resolve(pdf, catalog.get(b"PageLabels").ok()?)?
        .as_dict()
        .ok()?;
    let mut nums = Vec::new();
    collect_nums(pdf, tree, &mut nums, 0);
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

fn range_from(pdf: &Pdf, start: u32, d: &Dictionary) -> LabelRange {
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

/// Walk a number tree: `/Nums [key value …]` at leaves, `/Kids` above.
fn collect_nums<'a>(
    pdf: &'a Pdf,
    node: &'a Dictionary,
    out: &mut Vec<(u32, &'a Object)>,
    depth: u8,
) {
    if depth > 32 {
        return;
    }
    if let Some(Object::Array(nums)) = node.get(b"Nums").ok().and_then(|o| resolve(pdf, o)) {
        for pair in nums.as_chunks::<2>().0 {
            if let Some(k) = resolve(pdf, &pair[0]).and_then(|o| o.as_i64().ok())
                && (0..=u32::MAX as i64).contains(&k)
            {
                out.push((k as u32, &pair[1]));
            }
        }
    }
    if let Some(Object::Array(kids)) = node.get(b"Kids").ok().and_then(|o| resolve(pdf, o)) {
        for kid in kids {
            if let Some(Ok(d)) = resolve(pdf, kid).map(Object::as_dict) {
                collect_nums(pdf, d, out, depth + 1);
            }
        }
    }
}

fn resolve<'a>(pdf: &'a Pdf, o: &'a Object) -> Option<&'a Object> {
    match o {
        Object::Reference(id) => pdf.get_object(*id).ok(),
        o => Some(o),
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
    use lopdf::dictionary;

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
        assert_eq!(page_labels(&pdf, 4).unwrap(), ["i", "ii", "Tab 5", "Tab 6"]);
    }

    #[test]
    fn no_labels_is_none() {
        let mut pdf = Pdf::with_version("1.7");
        let catalog = pdf.add_object(dictionary! { "Type" => "Catalog" });
        pdf.trailer.set("Root", catalog);
        assert!(page_labels(&pdf, 3).is_none());
    }
}
