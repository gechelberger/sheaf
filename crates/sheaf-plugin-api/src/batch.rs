//! Build a list of edits and submit it as one undo step.

use crate::edits;
use crate::types::{
    Annot, AnnotKind, Box2, ConnectorPair, ConnectorShape, DestView, DestXyz, Edit, NewOutlineItem,
    OutlineInsert, OutlineInsertTree, PageAnnot, Rgb, StampShape,
};

/// A bookmark position: this point of the page at the window's top-left.
pub fn at(left: f32, top: f32) -> DestView {
    DestView::Xyz(DestXyz {
        left: Some(left),
        top: Some(top),
        zoom: None,
    })
}

/// Edits that undo together. Ids of things a batch creates aren't known
/// until sheaf applies it, after your call returns.
///
/// ```ignore
/// Batch::new("Bookmark forms")
///     .bookmark("Form 1040", Some(page))
///     .stamp(page, Box2 { x: 10.0, y: 10.0, w: 24.0, h: 24.0 }, "✓")
///     .submit();
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Batch {
    label: String,
    edits: Vec<Edit>,
}

impl Batch {
    /// `label` is what Undo shows.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            edits: Vec::new(),
        }
    }

    pub fn push(mut self, edit: Edit) -> Self {
        self.edits.push(edit);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    pub fn len(&self) -> usize {
        self.edits.len()
    }

    /// A top-level bookmark at the start of the outline.
    pub fn bookmark(self, title: &str, target: Option<u64>) -> Self {
        self.bookmark_at(title, target, None)
    }

    /// A top-level bookmark at the start of the outline that lands at
    /// `view` on its page (see [`at`]).
    pub fn bookmark_at(self, title: &str, target: Option<u64>, view: Option<DestView>) -> Self {
        self.push(Edit::OutlineInsert(OutlineInsert {
            parent: None,
            after: None,
            title: title.to_string(),
            target,
            view,
        }))
    }

    /// Several bookmarks nested by depth, after `after` under `parent`:
    /// a new heading and its children in one edit, say.
    pub fn bookmark_tree(
        self,
        parent: Option<u64>,
        after: Option<u64>,
        items: Vec<NewOutlineItem>,
    ) -> Self {
        self.push(Edit::OutlineInsertTree(OutlineInsertTree {
            parent,
            after,
            items,
        }))
    }

    /// A tickmark or stamp showing `glyph` in `rect`, in blue.
    pub fn stamp(self, page: u64, rect: Box2, glyph: &str) -> Self {
        self.push(Edit::AnnotAdd(PageAnnot {
            page,
            annot: annot(AnnotKind::Stamp(StampShape {
                rect,
                glyph: glyph.to_string(),
            })),
        }))
    }

    /// A numbered connector pair: one square on each page, linked.
    pub fn connector(self, a: (u64, Box2), b: (u64, Box2), label: &str) -> Self {
        let end = |(page, rect): (u64, Box2)| PageAnnot {
            page,
            annot: annot(AnnotKind::Connector(ConnectorShape {
                rect,
                label: label.to_string(),
                partner: None,
            })),
        };
        self.push(Edit::ConnectorAdd(ConnectorPair {
            a: end(a),
            b: end(b),
        }))
    }

    /// Submit unless empty.
    pub fn submit(self) {
        if !self.edits.is_empty() {
            edits::submit(&self.label, &self.edits);
        }
    }
}

/// sheaf's blue.
pub const BLUE: Rgb = Rgb {
    r: 0,
    g: 120,
    b: 255,
};

/// An annotation with sheaf's defaults: blue, opaque, the user as author.
pub fn annot(kind: AnnotKind) -> Annot {
    Annot {
        id: 0,
        kind,
        color: BLUE,
        opacity: 1.0,
        stroke: 1.5,
        author: String::new(),
        contents: String::new(),
        fill: None,
        dashed: false,
        cloudy: false,
        created: String::new(),
        modified: String::new(),
        name: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batches_collect_edits() {
        let r = Box2 {
            x: 1.0,
            y: 2.0,
            w: 3.0,
            h: 4.0,
        };
        let b = Batch::new("L")
            .bookmark("Form 1040", Some(7))
            .stamp(7, r, "✓")
            .connector((7, r), (9, r), "3");
        assert_eq!(b.len(), 3);
        let Edit::ConnectorAdd(pair) = &b.edits[2] else {
            panic!()
        };
        assert_eq!((pair.a.page, pair.b.page), (7, 9));
        assert!(matches!(&pair.b.annot.kind, AnnotKind::Connector(c) if c.label == "3"));
        assert!(Batch::new("empty").is_empty());
    }
}
