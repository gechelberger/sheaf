//! The bookmarks, read from the source (M10.5 §3.8): each item's title,
//! the page it goes to and where on it, its color and style, and whether
//! the file shows it closed, flattened in pre-order with its depth.
//!
//! Positions are PageSpace (M10.5 §5), as the file has them, so a
//! bookmark sheaf saved is read back where it was put, and a crop or a
//! turn of its page leaves it on the same content. PDFium's bookmark walk,
//! which sheaf-render's tests keep, is the reference: the same caps, items
//! and destinations.

use std::collections::{HashMap, HashSet};

use lopdf::{Dictionary, LazyPdf, Object, ObjectId};
use sheaf_core::DestView;

use crate::Source;
use crate::attachments::{name_tree, resolve};
use crate::file::{pdf_text_bytes, text_string};
use crate::model::{FlatOutlineItem, TextStyle};

#[cfg(test)]
mod tests;

/// How deep the walk goes: an item this deep is read, its children
/// aren't. PDFium's walk stopped there too.
const MAX_DEPTH: u16 = 32;
/// How many items are read at most.
const MAX_ITEMS: usize = 200_000;

/// `src`'s bookmarks in pre-order: the catalog's `/Outlines`, each item
/// then its `/First` child's level, then its `/Next`. An item seen before
/// ends the run of siblings it's in, so a cycle reads each item once.
pub fn outline(src: &Source) -> Vec<FlatOutlineItem> {
    let pdf = src.pdf();
    let first = pdf
        .catalog()
        .ok()
        .and_then(|c| c.get(b"Outlines").ok())
        .and_then(|o| resolve(pdf, o))
        .and_then(|o| o.as_dict().ok())
        .and_then(|d| link(d, b"First"));
    let Some(first) = first else {
        return Vec::new();
    };
    let mut pages = HashMap::new();
    for (index, id) in src.page_ids().enumerate() {
        // A page the tree lists twice is where it's first listed, as
        // PDFium's page index has it.
        pages.entry(id).or_insert(index as u32);
    }
    let mut walk = Walk {
        src,
        pdf,
        pages,
        names: None,
        seen: HashSet::new(),
        out: Vec::new(),
    };
    walk.level(Some(first), 0);
    walk.out
}

/// A title's text, as PDFium shows it: the text string's
/// ([`text_string`]), control characters as spaces.
fn title(bytes: &[u8]) -> String {
    text_string(bytes)
        .chars()
        .map(|c| if c < ' ' { ' ' } else { c })
        .collect()
}

/// An item's `/First` or `/Next`.
fn link(item: &Dictionary, key: &[u8]) -> Option<ObjectId> {
    item.get(key).and_then(Object::as_reference).ok()
}

struct Walk<'a> {
    src: &'a Source,
    pdf: &'a LazyPdf,
    /// Each page object's index in the page table.
    pages: HashMap<ObjectId, u32>,
    /// `/Names /Dests` by key, read when a destination first names one.
    names: Option<HashMap<String, &'a Object>>,
    seen: HashSet<ObjectId>,
    out: Vec<FlatOutlineItem>,
}

impl<'a> Walk<'a> {
    /// The run of siblings from `first`, each followed by its children.
    fn level(&mut self, first: Option<ObjectId>, depth: u16) {
        let mut next = first;
        while let Some(id) = next {
            if self.out.len() >= MAX_ITEMS || !self.seen.insert(id) {
                return;
            }
            let Ok(item) = self.pdf.get_dictionary(id) else {
                return;
            };
            let (page, view) = self.target(item);
            let (color, style) = self.style(item);
            let count = item
                .get(b"Count")
                .ok()
                .and_then(|c| resolve(self.pdf, c))
                .and_then(|c| c.as_i64().ok());
            let title = item
                .get(b"Title")
                .ok()
                .and_then(|t| resolve(self.pdf, t))
                .and_then(|t| t.as_str().ok())
                .map(title)
                .unwrap_or_default();
            self.out.push(FlatOutlineItem {
                title,
                page,
                depth,
                view,
                color,
                style,
                open: count.is_none_or(|n| n >= 0),
            });
            if depth < MAX_DEPTH {
                self.level(link(item, b"First"), depth + 1);
            }
            next = link(item, b"Next");
        }
    }

    /// Where `item` goes: its `/Dest`, else its GoTo action's `/D`. No
    /// page, no position.
    fn target(&mut self, item: &'a Dictionary) -> (Option<u32>, Option<DestView>) {
        let dest = match item.get(b"Dest") {
            Ok(dest) => self.dest(dest),
            Err(_) => None,
        };
        let Some(dest) = dest.or_else(|| self.goto(item)) else {
            return (None, None);
        };
        let Some(page) = self.page(dest) else {
            return (None, None);
        };
        (Some(page), self.view(dest))
    }

    /// The destination of `item`'s `/A`, when it's a GoTo.
    fn goto(&mut self, item: &'a Dictionary) -> Option<&'a [Object]> {
        let action = resolve(self.pdf, item.get(b"A").ok()?)?.as_dict().ok()?;
        let kind = resolve(self.pdf, action.get(b"S").ok()?)?.as_name().ok()?;
        if kind != b"GoTo" {
            return None;
        }
        self.dest(action.get(b"D").ok()?)
    }

    /// A destination's array: written out, or by name, in the `/Names
    /// /Dests` tree or else the catalog's `/Dests` (PDF 1.1), where the
    /// array may be a dictionary's `/D`.
    fn dest(&mut self, dest: &'a Object) -> Option<&'a [Object]> {
        let name = match resolve(self.pdf, dest)? {
            Object::Array(dest) => return Some(dest),
            Object::Name(name) | Object::String(name, _) => name,
            _ => return None,
        };
        let pdf = self.pdf;
        let named = self
            .names()
            .get(&pdf_text_bytes(name))
            .copied()
            .or_else(|| {
                let dests = resolve(pdf, pdf.catalog().ok()?.get(b"Dests").ok()?)?;
                dests.as_dict().ok()?.get(name).ok()
            })?;
        match resolve(pdf, named)? {
            Object::Array(dest) => Some(dest),
            Object::Dictionary(d) => resolve(pdf, d.get(b"D").ok()?)?
                .as_array()
                .ok()
                .map(Vec::as_slice),
            _ => None,
        }
    }

    fn names(&mut self) -> &HashMap<String, &'a Object> {
        let pdf = self.pdf;
        self.names.get_or_insert_with(|| {
            let mut names = HashMap::new();
            for (key, dest) in name_tree(pdf, b"Dests") {
                // The first of a key listed twice.
                names.entry(key).or_insert(dest);
            }
            names
        })
    }

    /// The page a destination names: a page object, by its index in the
    /// page table, or an index itself.
    fn page(&self, dest: &[Object]) -> Option<u32> {
        match dest.first()? {
            Object::Reference(id) => self.pages.get(id).copied(),
            Object::Integer(index) => u32::try_from(*index)
                .ok()
                .filter(|&index| index < self.src.page_count()),
            _ => None,
        }
    }

    /// Where on its page a destination lands. An `/XYZ` position is read
    /// as is: null stays None, a zoom of 0 is none, and with nothing set
    /// it's just the page. Other kinds keep their numbers as written, as
    /// many as the kind takes.
    fn view(&self, dest: &[Object]) -> Option<DestView> {
        let kind = resolve(self.pdf, dest.get(1)?)?.as_name().ok()?;
        let takes = match kind {
            b"XYZ" => 3,
            b"Fit" | b"FitB" => 0,
            b"FitH" | b"FitV" | b"FitBH" | b"FitBV" => 1,
            b"FitR" => 4,
            _ => return None,
        };
        let params: Vec<Option<f32>> = (2..2 + takes)
            .map(|i| {
                let n = resolve(self.pdf, dest.get(i)?)?.as_float().ok()?;
                Some(n as f32)
            })
            .collect();
        if kind != b"XYZ" {
            return Some(DestView::Other {
                kind: String::from_utf8_lossy(kind).into_owned(),
                params,
            });
        }
        let (left, top) = (params[0], params[1]);
        let zoom = params[2].filter(|&z| z != 0.0);
        if left.is_none() && top.is_none() && zoom.is_none() {
            return None;
        }
        Some(DestView::Xyz { left, top, zoom })
    }

    /// `/C` (black is no color) and `/F` (italic 1, bold 2).
    fn style(&self, item: &Dictionary) -> (Option<[u8; 3]>, TextStyle) {
        let color = item
            .get(b"C")
            .ok()
            .and_then(|c| resolve(self.pdf, c))
            .and_then(|c| c.as_array().ok())
            .filter(|a| a.len() == 3)
            .map(|a| {
                let v = |i: usize| {
                    (a[i].as_float().unwrap_or(0.0).clamp(0.0, 1.0) * 255.0).round() as u8
                };
                [v(0), v(1), v(2)]
            })
            .filter(|c| *c != [0, 0, 0]);
        let flags = item
            .get(b"F")
            .ok()
            .and_then(|f| f.as_i64().ok())
            .unwrap_or(0);
        let style = TextStyle {
            italic: flags & 1 != 0,
            bold: flags & 2 != 0,
        };
        (color, style)
    }
}
