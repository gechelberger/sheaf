//! Edits in words, for the History panel: "Delete pages 4–6", "Add
//! highlight on page 3". Page numbers come from the document as it was
//! just before the edit: a step's line is made when it's applied, so a
//! batch's later edits count pages after its earlier ones.

use crate::annot::AnnotKind;
use crate::model::{DocState, Edit, PageId};

/// What `edit` does to `before`, the document it's applied to, in a short
/// line.
pub fn describe(edit: &Edit, before: &DocState) -> String {
    let pages = |ids: &[PageId]| pages_text(ids, before);
    match edit {
        Edit::MovePages { ids, .. } => format!("Move {}", pages(ids)),
        Edit::RotatePages { ids, by } => {
            format!("Rotate {} {}°", pages(ids), by.degrees())
        }
        Edit::DeletePages { ids } => format!("Delete {}", pages(ids)),
        Edit::InsertPages { range, .. } => format!("Insert {}", count(range.len(), "page")),
        Edit::DuplicatePages { ids } => format!("Duplicate {}", pages(ids)),
        Edit::ReplacePages { ids, range, .. } => {
            format!("Replace {} with {}", pages(ids), count(range.len(), "page"))
        }
        Edit::OutlineInsert { title, .. } => format!("Add bookmark “{title}”"),
        Edit::OutlineInsertTree { items, .. } => match items.as_slice() {
            [one] => format!("Add bookmark “{}”", one.title),
            _ => format!("Add {}", count(items.len(), "bookmark")),
        },
        Edit::OutlineMove { with_pages, .. } => if *with_pages {
            "Move a bookmark with its pages"
        } else {
            "Move a bookmark"
        }
        .into(),
        Edit::OutlineEdit {
            title,
            target,
            color,
            view,
            style,
            ..
        } => {
            let mut what = Vec::new();
            if let Some(t) = title {
                what.push(format!("rename it “{t}”"));
            }
            if target.is_some() || view.is_some() {
                what.push("change where it goes".into());
            }
            if let Some(c) = color {
                what.push(
                    if c.is_some() {
                        "color it"
                    } else {
                        "clear its color"
                    }
                    .into(),
                );
            }
            if style.is_some() {
                what.push("change its style".into());
            }
            match what.as_slice() {
                [] => "Change a bookmark".into(),
                _ => format!("Bookmark: {}", what.join(", ")),
            }
        }
        Edit::OutlineDelete { .. } => "Delete a bookmark".into(),
        Edit::AnnotAdd { page, annot } => match &annot.kind {
            AnnotKind::Reply { .. } => format!("Reply{}", on_page(*page, before)),
            AnnotKind::Status { model, state, .. } if model == crate::annot::MARKED => format!(
                "{} a comment{}",
                if state == "Marked" {
                    "Check"
                } else {
                    "Uncheck"
                },
                on_page(*page, before)
            ),
            AnnotKind::Status { state, .. } => {
                format!("Set status {state}{}", on_page(*page, before))
            }
            kind => format!("Add {}{}", noun(kind), on_page(*page, before)),
        },
        Edit::AnnotUpdate { page, annot } => {
            format!("Change {}{}", noun(&annot.kind), on_page(*page, before))
        }
        Edit::AnnotDelete { page, id } => {
            let kind = before
                .annots_on(*page)
                .and_then(|list| list.iter().find(|a| a.id == *id))
                .map(|a| noun(&a.kind))
                .unwrap_or_else(|| "a mark".into());
            format!("Delete {kind}{}", on_page(*page, before))
        }
        Edit::CommentsDelete { page, names } => format!(
            "Delete {}{}",
            match names.len() {
                1 => "a comment".to_string(),
                n => format!("{n} comments"),
            },
            on_page(*page, before)
        ),
        Edit::ConnectorAdd { a, .. } => match &a.1.kind {
            AnnotKind::Connector { label, .. } => format!("Add connector pair {label}"),
            _ => "Add a connector pair".into(),
        },
        Edit::SetTextLayers { layers } => {
            let ids: Vec<PageId> = layers.iter().map(|(id, _)| *id).collect();
            format!("Recognize text on {}", pages(&ids))
        }
        Edit::ClearTextLayers { ids } => format!("Remove recognized text from {}", pages(ids)),
        Edit::RebasePages { pages: list } => {
            let ids: Vec<PageId> = list.iter().map(|r| r.id).collect();
            format!("Change the content of {}", pages(&ids))
        }
        Edit::SetPageLabels { .. } => "Change page labels".into(),
        Edit::SetHeaderFooter { header_footer } => match header_footer {
            Some(_) => "Set the header and footer".into(),
            None => "Remove the header and footer".into(),
        },
    }
}

/// "a highlight", "a sticky note", "tickmark ✓".
fn noun(kind: &AnnotKind) -> String {
    match kind {
        AnnotKind::Highlight { .. } => "a highlight".into(),
        AnnotKind::Note { .. } => "a sticky note".into(),
        AnnotKind::FreeText { .. } => "a text box".into(),
        AnnotKind::Square { .. } => "a rectangle".into(),
        AnnotKind::Ellipse { .. } => "an ellipse".into(),
        AnnotKind::Line { arrow: true, .. } => "an arrow".into(),
        AnnotKind::Line { .. } => "a line".into(),
        AnnotKind::Ink { .. } => "a drawing".into(),
        AnnotKind::Stamp { glyph, .. } => format!("tickmark {glyph}"),
        AnnotKind::Connector { label, .. } => format!("connector {label}"),
        AnnotKind::Tape { .. } => "a calculator tape".into(),
        AnnotKind::Underline { .. } => "an underline".into(),
        AnnotKind::StrikeOut { .. } => "a strikethrough".into(),
        AnnotKind::Squiggly { .. } => "a squiggly underline".into(),
        AnnotKind::Replace { .. } => "a text replacement".into(),
        AnnotKind::Insert { .. } => "a text insertion".into(),
        AnnotKind::PageStamp { name, .. } => format!("stamp {name}"),
        AnnotKind::TextStamp { text, .. } => {
            format!("stamp {}", text.lines().next().unwrap_or_default())
        }
        AnnotKind::PageBorder { status, .. } => format!("a {status} border"),
        AnnotKind::Polygon { .. } => "a polygon".into(),
        AnnotKind::Polyline { .. } => "a polyline".into(),
        AnnotKind::Callout { .. } => "a callout".into(),
        AnnotKind::FileAttachment { name, .. } => format!("attachment {name}"),
        AnnotKind::Measure { what, .. } => format!("a {} measurement", what.label().to_lowercase()),
        AnnotKind::Reply { .. } => "a reply".into(),
        AnnotKind::Status { .. } => "a status".into(),
    }
}

fn count(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

/// " on page 3", or nothing when the page isn't in the document.
fn on_page(page: PageId, st: &DocState) -> String {
    st.index_of(page)
        .map(|i| format!(" on page {}", i + 1))
        .unwrap_or_default()
}

/// "page 3", "pages 2, 5", "pages 4–6", or a count when one of them isn't
/// in the document.
fn pages_text(ids: &[PageId], st: &DocState) -> String {
    // One walk of the pages, not a search for each id: their numbers come
    // out sorted, each once. A sorted list finds the ids faster than a hash
    // set does, from one id to thousands.
    let mut wanted = ids.to_vec();
    wanted.sort_unstable();
    wanted.dedup();
    let numbers: Vec<usize> = st
        .pages
        .iter()
        .enumerate()
        .filter(|(_, p)| wanted.binary_search(&p.id).is_ok())
        .map(|(i, _)| i + 1)
        .collect();
    if numbers.len() != wanted.len() || numbers.is_empty() {
        return count(wanted.len(), "page");
    }
    // Runs: 4–6, 9.
    let mut runs: Vec<String> = Vec::new();
    let mut i = 0;
    while i < numbers.len() {
        let mut j = i;
        while j + 1 < numbers.len() && numbers[j + 1] == numbers[j] + 1 {
            j += 1;
        }
        runs.push(match j - i {
            0 => numbers[i].to_string(),
            1 => format!("{}, {}", numbers[i], numbers[j]),
            _ => format!("{}–{}", numbers[i], numbers[j]),
        });
        i = j + 1;
    }
    let word = if numbers.len() == 1 { "page" } else { "pages" };
    // A long list is just its count.
    if runs.len() > 4 {
        return count(numbers.len(), "page");
    }
    format!("{word} {}", runs.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::FixedCount;
    use crate::model::{Document, SourceInfo};
    use crate::store::Store;
    use sheaf_core::Rotation;

    fn doc(n: u32) -> Document {
        let store = Store::new();
        let primary = SourceInfo::blank(&store, n);
        Document::new(store, primary, &[], Box::new(FixedCount(10)))
    }

    #[test]
    fn pages_read_as_numbers_and_runs() {
        let d = doc(10);
        let id = |i: usize| d.page(i).unwrap().id;
        let st = d.state();
        assert_eq!(pages_text(&[id(2)], st), "page 3");
        assert_eq!(pages_text(&[id(5), id(4), id(3)], st), "pages 4–6");
        assert_eq!(pages_text(&[id(1), id(4)], st), "pages 2, 5");
        assert_eq!(pages_text(&[id(0), id(1), id(8)], st), "pages 1, 2, 9");
        let every_other: Vec<PageId> = (0..10).step_by(2).map(id).collect();
        assert_eq!(pages_text(&every_other, st), "5 pages", "a long list");
        assert_eq!(
            pages_text(&[id(3), PageId(999)], st),
            "2 pages",
            "one isn't there"
        );
        assert_eq!(pages_text(&[id(2), id(2)], st), "page 3", "named twice");
        assert_eq!(
            pages_text(&[id(3), id(3), PageId(999)], st),
            "2 pages",
            "named twice, and one isn't there"
        );
    }

    #[test]
    fn edits_read_plainly() {
        let d = doc(6);
        let id = |i: usize| d.page(i).unwrap().id;
        let st = d.state();
        assert_eq!(
            describe(
                &Edit::DeletePages {
                    ids: vec![id(3), id(4), id(5)]
                },
                st
            ),
            "Delete pages 4–6"
        );
        assert_eq!(
            describe(
                &Edit::RotatePages {
                    ids: vec![id(0)],
                    by: Rotation::R90
                },
                st
            ),
            "Rotate page 1 90°"
        );
    }
}
