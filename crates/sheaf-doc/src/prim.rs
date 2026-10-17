//! What an edit changed, as history keeps it (M10.5 §6.2).
//!
//! A [`Prim`] puts one slot of the document, a [`Key`], from the value it
//! held before the edit to the value it holds after. `None` is absent: no
//! labels, no header/footer, nothing for that page or field. Before-values
//! cost nothing, since they are the parent's values (im and `Arc` clones),
//! and they make replay checked: [`DocState::apply`] refuses a prim whose
//! before isn't what the slot holds, so a damaged history fails instead of
//! quietly building another document.
//!
//! An edit writes through a recording context, [`Rec`], which turns what it
//! wrote into prims. A slot put back to the value it had makes none, so an
//! edit that changes nothing has none.
//!
//! History counts what it holds with [`HeapSize`] (§3.9).
//!
//! The keys are transitional, one per field of today's `DocState`; M10.5
//! slice 8 replaces them with the records' keys.

use std::collections::BTreeSet;
use std::ops::Deref;
use std::sync::Arc;

use im::{OrdSet, Vector};
use sheaf_core::DestView;

use crate::annot::{Annot, AnnotKind, Box2, Pt};
use crate::forms::field::{Action, Choice, Trigger, Value};
use crate::forms::{Field, FieldId, Widget};
use crate::labels::LabelSpec;
use crate::measure::Scale;
use crate::model::{
    DocState, Edit, FlatOutlineItem, NewOutlineItem, OutlineNode, PageId, PageRef, Rebase,
};
use crate::stamps::{Bates, HeaderFooter};
use crate::tape::{Row, Tape};
use crate::text_layer::TextLayer;

/// A slot of the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Key {
    Pages,
    Outline,
    /// A page's marks.
    AnnotsOf(PageId),
    TextLayer(PageId),
    /// Other tools' comments deleted from a page.
    RemovedOf(PageId),
    Labels,
    HeaderFooter,
    Field(FieldId),
    WidgetsOf(PageId),
}

/// What a slot holds. Pages and Outline always hold one.
#[derive(Debug, Clone)]
pub enum Val {
    Pages(Vector<PageRef>),
    Outline(Vector<OutlineNode>),
    Annots(Vector<Annot>),
    TextLayer(Arc<TextLayer>),
    Removed(OrdSet<String>),
    Labels(Vector<LabelSpec>),
    HeaderFooter(Arc<HeaderFooter>),
    /// A copy: `DocState` holds its fields by value.
    Field(Box<Field>),
    Widgets(Vector<Widget>),
}

/// One slot, from `before` to `after`.
#[derive(Debug, Clone)]
pub struct Prim {
    pub key: Key,
    pub before: Option<Val>,
    pub after: Option<Val>,
}

/// A prim that doesn't fit the state: `key` doesn't hold its before, or
/// its after isn't a value `key` can hold.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{key:?} doesn't hold what the history says it held")]
pub struct Mismatch {
    pub key: Key,
}

/// Whether two slot values are the same: the same im nodes or `Arc`, or
/// else equal.
pub(crate) fn same(a: Option<&Val>, b: Option<&Val>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(Val::Pages(a)), Some(Val::Pages(b))) => a.ptr_eq(b) || a == b,
        (Some(Val::Outline(a)), Some(Val::Outline(b))) => a.ptr_eq(b) || a == b,
        (Some(Val::Annots(a)), Some(Val::Annots(b))) => a.ptr_eq(b) || a == b,
        (Some(Val::TextLayer(a)), Some(Val::TextLayer(b))) => Arc::ptr_eq(a, b) || a == b,
        (Some(Val::Removed(a)), Some(Val::Removed(b))) => a.ptr_eq(b) || a == b,
        (Some(Val::Labels(a)), Some(Val::Labels(b))) => a.ptr_eq(b) || a == b,
        (Some(Val::HeaderFooter(a)), Some(Val::HeaderFooter(b))) => Arc::ptr_eq(a, b) || a == b,
        (Some(Val::Field(a)), Some(Val::Field(b))) => a == b,
        (Some(Val::Widgets(a)), Some(Val::Widgets(b))) => a.ptr_eq(b) || a == b,
        _ => false,
    }
}

impl DocState {
    /// What `key` holds.
    pub(crate) fn get(&self, key: &Key) -> Option<Val> {
        match *key {
            Key::Pages => Some(Val::Pages(self.pages.clone())),
            Key::Outline => Some(Val::Outline(self.outline.clone())),
            Key::AnnotsOf(page) => self.annots.get(&page).cloned().map(Val::Annots),
            Key::TextLayer(page) => self.text_layers.get(&page).cloned().map(Val::TextLayer),
            Key::RemovedOf(page) => self.removed_comments.get(&page).cloned().map(Val::Removed),
            Key::Labels => self.labels.clone().map(Val::Labels),
            Key::HeaderFooter => self.header_footer.clone().map(Val::HeaderFooter),
            Key::Field(id) => self
                .fields
                .get(&id)
                .map(|f| Val::Field(Box::new(f.clone()))),
            Key::WidgetsOf(page) => self.widgets.get(&page).cloned().map(Val::Widgets),
        }
    }

    /// Puts `val` in `key`'s slot. A value `key` can't hold leaves the
    /// state as it was.
    fn set(&mut self, key: &Key, val: Option<Val>) -> Result<(), Mismatch> {
        let misfit = Err(Mismatch { key: *key });
        match *key {
            Key::Pages => match val {
                Some(Val::Pages(v)) => self.pages = v,
                _ => return misfit,
            },
            Key::Outline => match val {
                Some(Val::Outline(v)) => self.outline = v,
                _ => return misfit,
            },
            Key::AnnotsOf(page) => match val {
                Some(Val::Annots(v)) => _ = self.annots.insert(page, v),
                None => _ = self.annots.remove(&page),
                _ => return misfit,
            },
            Key::TextLayer(page) => match val {
                Some(Val::TextLayer(v)) => _ = self.text_layers.insert(page, v),
                None => _ = self.text_layers.remove(&page),
                _ => return misfit,
            },
            Key::RemovedOf(page) => match val {
                Some(Val::Removed(v)) => _ = self.removed_comments.insert(page, v),
                None => _ = self.removed_comments.remove(&page),
                _ => return misfit,
            },
            Key::Labels => match val {
                Some(Val::Labels(v)) => self.labels = Some(v),
                None => self.labels = None,
                _ => return misfit,
            },
            Key::HeaderFooter => match val {
                Some(Val::HeaderFooter(v)) => self.header_footer = Some(v),
                None => self.header_footer = None,
                _ => return misfit,
            },
            Key::Field(id) => match val {
                Some(Val::Field(v)) => _ = self.fields.insert(id, *v),
                None => _ = self.fields.remove(&id),
                _ => return misfit,
            },
            Key::WidgetsOf(page) => match val {
                Some(Val::Widgets(v)) => _ = self.widgets.insert(page, v),
                None => _ = self.widgets.remove(&page),
                _ => return misfit,
            },
        }
        Ok(())
    }

    /// Applies `p`: when its key holds its before, puts its after there.
    /// Otherwise the state is left as it was.
    pub fn apply(&mut self, p: &Prim) -> Result<(), Mismatch> {
        if !same(self.get(&p.key).as_ref(), p.before.as_ref()) {
            return Err(Mismatch { key: p.key });
        }
        self.set(&p.key, p.after.clone())
    }
}

/// The state an edit is computing, and what it started from. The edit
/// reads its own writes through `Deref`, and writes only with
/// [`put`](Self::put), so [`finish`](Self::finish) knows each slot it
/// changed.
pub(crate) struct Rec {
    base: DocState,
    st: DocState,
    touched: BTreeSet<Key>,
}

impl Rec {
    pub(crate) fn new(base: DocState) -> Self {
        Self {
            st: base.clone(),
            base,
            touched: BTreeSet::new(),
        }
    }

    pub(crate) fn put(&mut self, key: Key, val: Option<Val>) {
        self.st
            .set(&key, val)
            .expect("an edit puts values its keys can hold");
        self.touched.insert(key);
    }

    /// The state the edit made, and its prims from the state it started
    /// from, in key order: none for a slot that ended as it was.
    pub(crate) fn finish(self) -> (DocState, Vec<Prim>) {
        let prims = self
            .touched
            .into_iter()
            .filter_map(|key| {
                let before = self.base.get(&key);
                let after = self.st.get(&key);
                (!same(before.as_ref(), after.as_ref())).then_some(Prim { key, before, after })
            })
            .collect();
        (self.st, prims)
    }
}

impl Deref for Rec {
    type Target = DocState;

    fn deref(&self) -> &DocState {
        &self.st
    }
}

/// The bytes a value holds beyond its own `size_of`, as history counts
/// them. An estimate: an im collection counts as its elements, and an
/// `Arc` as if nothing else held it.
pub trait HeapSize {
    fn heap_size(&self) -> usize;
}

/// What a `Vec` holds for its elements, not counting what they hold.
fn buf<T>(v: &Vec<T>) -> usize {
    v.capacity() * size_of::<T>()
}

/// Plain data: nothing beyond its own size.
macro_rules! no_heap {
    ($($t:ty),*) => {
        $(impl HeapSize for $t {
            fn heap_size(&self) -> usize {
                0
            }
        })*
    };
}

no_heap!(u32, PageId, PageRef, Pt, Box2, Trigger, Rebase);

impl HeapSize for String {
    fn heap_size(&self) -> usize {
        self.capacity()
    }
}

impl HeapSize for Arc<str> {
    fn heap_size(&self) -> usize {
        2 * size_of::<usize>() + self.len()
    }
}

impl<T: HeapSize> HeapSize for Arc<[T]> {
    fn heap_size(&self) -> usize {
        2 * size_of::<usize>()
            + self.len() * size_of::<T>()
            + self.iter().map(T::heap_size).sum::<usize>()
    }
}

impl<T: HeapSize> HeapSize for Box<T> {
    fn heap_size(&self) -> usize {
        size_of::<T>() + T::heap_size(self)
    }
}

impl<T: HeapSize> HeapSize for Option<T> {
    fn heap_size(&self) -> usize {
        self.as_ref().map_or(0, T::heap_size)
    }
}

impl<A: HeapSize, B: HeapSize> HeapSize for (A, B) {
    fn heap_size(&self) -> usize {
        self.0.heap_size() + self.1.heap_size()
    }
}

impl<T: HeapSize> HeapSize for Vec<T> {
    fn heap_size(&self) -> usize {
        buf(self) + self.iter().map(T::heap_size).sum::<usize>()
    }
}

impl<T: HeapSize + Clone> HeapSize for Vector<T> {
    fn heap_size(&self) -> usize {
        self.len() * size_of::<T>() + self.iter().map(T::heap_size).sum::<usize>()
    }
}

impl<T: HeapSize + Ord + Clone> HeapSize for OrdSet<T> {
    fn heap_size(&self) -> usize {
        self.len() * size_of::<T>() + self.iter().map(T::heap_size).sum::<usize>()
    }
}

impl HeapSize for DestView {
    fn heap_size(&self) -> usize {
        match self {
            DestView::Xyz { .. } => 0,
            DestView::Other { kind, params } => kind.heap_size() + buf(params),
        }
    }
}

impl HeapSize for OutlineNode {
    fn heap_size(&self) -> usize {
        self.title.heap_size() + self.view.heap_size() + self.children.heap_size()
    }
}

impl HeapSize for FlatOutlineItem {
    fn heap_size(&self) -> usize {
        self.title.heap_size() + self.view.heap_size()
    }
}

impl HeapSize for NewOutlineItem {
    fn heap_size(&self) -> usize {
        self.title.heap_size() + self.view.heap_size()
    }
}

impl HeapSize for LabelSpec {
    fn heap_size(&self) -> usize {
        self.prefix.heap_size()
    }
}

impl HeapSize for Bates {
    fn heap_size(&self) -> usize {
        self.prefix.heap_size() + self.suffix.heap_size()
    }
}

impl HeapSize for HeaderFooter {
    fn heap_size(&self) -> usize {
        self.texts.iter().map(String::heap_size).sum::<usize>()
            + self.pages.heap_size()
            + self.date.heap_size()
            + self.file.heap_size()
            + self.bates.heap_size()
    }
}

impl HeapSize for Tape {
    fn heap_size(&self) -> usize {
        buf(&self.lines)
            + self
                .lines
                .iter()
                .map(|l| {
                    l.note.heap_size()
                        + match &l.row {
                            Row::Title(t) => t.heap_size(),
                            _ => 0,
                        }
                })
                .sum::<usize>()
    }
}

impl HeapSize for Scale {
    fn heap_size(&self) -> usize {
        self.page_unit.heap_size() + self.unit.heap_size()
    }
}

impl HeapSize for AnnotKind {
    fn heap_size(&self) -> usize {
        match self {
            AnnotKind::Highlight { rects }
            | AnnotKind::Underline { rects }
            | AnnotKind::StrikeOut { rects }
            | AnnotKind::Squiggly { rects } => rects.heap_size(),
            AnnotKind::Replace { rects, struck, .. } => rects.heap_size() + struck.heap_size(),
            AnnotKind::Ink { paths } => paths.heap_size(),
            AnnotKind::Polygon { points } | AnnotKind::Polyline { points } => points.heap_size(),
            AnnotKind::Measure { points, scale, .. } => points.heap_size() + scale.heap_size(),
            AnnotKind::Tape { tape, .. } => tape.heap_size(),
            AnnotKind::Stamp { glyph: s, .. }
            | AnnotKind::Connector { label: s, .. }
            | AnnotKind::Reply { parent: s }
            | AnnotKind::TextStamp { text: s, .. }
            | AnnotKind::PageBorder { status: s, .. } => s.heap_size(),
            AnnotKind::Status {
                parent,
                model,
                state,
            } => parent.heap_size() + model.heap_size() + state.heap_size(),
            AnnotKind::PageStamp {
                stamp, name, text, ..
            } => stamp.heap_size() + name.heap_size() + text.heap_size(),
            AnnotKind::FileAttachment {
                icon, file, name, ..
            } => icon.heap_size() + file.heap_size() + name.heap_size(),
            AnnotKind::Note { .. }
            | AnnotKind::FreeText { .. }
            | AnnotKind::Square { .. }
            | AnnotKind::Ellipse { .. }
            | AnnotKind::Line { .. }
            | AnnotKind::Insert { .. }
            | AnnotKind::Callout { .. } => 0,
        }
    }
}

impl HeapSize for Annot {
    fn heap_size(&self) -> usize {
        self.kind.heap_size()
            + self.author.heap_size()
            + self.contents.heap_size()
            + self.modified.heap_size()
            + self.created.heap_size()
            + self.name.heap_size()
    }
}

impl HeapSize for Value {
    fn heap_size(&self) -> usize {
        match self {
            Value::Text(s) | Value::State(s) => s.heap_size(),
            Value::Items(items) => items.heap_size(),
            Value::Signed => 0,
        }
    }
}

impl HeapSize for Choice {
    fn heap_size(&self) -> usize {
        self.export.heap_size() + self.shown.heap_size()
    }
}

impl HeapSize for Action {
    fn heap_size(&self) -> usize {
        self.kind.heap_size() + self.script.heap_size()
    }
}

impl HeapSize for Field {
    fn heap_size(&self) -> usize {
        self.name.heap_size()
            + self.value.heap_size()
            + self.default.heap_size()
            + self.da.heap_size()
            + self.options.heap_size()
            + self.tooltip.heap_size()
            + self.actions.heap_size()
            + self.under.heap_size()
    }
}

impl HeapSize for Widget {
    fn heap_size(&self) -> usize {
        self.da.heap_size()
            + self.mk.caption.heap_size()
            + self.state.heap_size()
            + self.states.heap_size()
            + self.actions.heap_size()
    }
}

impl HeapSize for TextLayer {
    fn heap_size(&self) -> usize {
        self.byte_estimate()
    }
}

impl HeapSize for Val {
    fn heap_size(&self) -> usize {
        match self {
            Val::Pages(v) => v.heap_size(),
            Val::Outline(v) => v.heap_size(),
            Val::Annots(v) => v.heap_size(),
            Val::TextLayer(l) => size_of::<TextLayer>() + l.heap_size(),
            Val::Removed(s) => s.heap_size(),
            Val::Labels(v) => v.heap_size(),
            Val::HeaderFooter(h) => size_of::<HeaderFooter>() + h.heap_size(),
            Val::Field(f) => f.heap_size(),
            Val::Widgets(v) => v.heap_size(),
        }
    }
}

/// What an edit holds beyond its own size. Ids are plain data.
impl HeapSize for Edit {
    fn heap_size(&self) -> usize {
        match self {
            Edit::MovePages { ids, .. }
            | Edit::RotatePages { ids, .. }
            | Edit::DeletePages { ids }
            | Edit::DuplicatePages { ids }
            | Edit::ClearTextLayers { ids } => ids.heap_size(),
            Edit::InsertPages {
                outline, annots, ..
            } => outline.heap_size() + annots.heap_size(),
            Edit::ReplacePages { ids, annots, .. } => ids.heap_size() + annots.heap_size(),
            Edit::OutlineInsert { title, view, .. } => title.heap_size() + view.heap_size(),
            Edit::OutlineInsertTree { items, .. } => items.heap_size(),
            Edit::OutlineMove { .. } | Edit::OutlineDelete { .. } | Edit::AnnotDelete { .. } => 0,
            Edit::OutlineEdit { title, view, .. } => title.heap_size() + view.heap_size(),
            Edit::AnnotAdd { annot, .. } | Edit::AnnotUpdate { annot, .. } => annot.heap_size(),
            Edit::CommentsDelete { names, .. } => names.heap_size(),
            Edit::ConnectorAdd { a, b } => a.heap_size() + b.heap_size(),
            // The layers are the ones its prims put, and counted there.
            Edit::SetTextLayers { layers } => buf(layers),
            Edit::RebasePages { pages } => pages.heap_size(),
            Edit::SetPageLabels { ranges } => ranges.heap_size(),
            Edit::SetHeaderFooter { header_footer } => header_footer.heap_size(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::labels::Style;
    use crate::model::SourceId;
    use sheaf_core::Rotation;

    fn state() -> DocState {
        DocState {
            pages: (0..3)
                .map(|i| PageRef {
                    id: PageId(i),
                    source: SourceId(0),
                    index: i as u32,
                    rotation: Rotation::R0,
                })
                .collect(),
            outline: Vector::new(),
            annots: im::HashMap::new(),
            text_layers: im::HashMap::new(),
            labels: None,
            header_footer: None,
            removed_comments: im::HashMap::new(),
            fields: im::OrdMap::new(),
            widgets: im::HashMap::new(),
        }
    }

    fn labels(prefix: &str) -> Val {
        Val::Labels(Vector::unit(LabelSpec {
            start: PageId(0),
            style: Style::Decimal,
            prefix: prefix.into(),
            first: 1,
        }))
    }

    fn reversed(st: &DocState) -> Vector<PageRef> {
        st.pages.iter().rev().copied().collect()
    }

    #[test]
    fn apply_sets_and_removes() {
        let mut st = state();
        let pages = reversed(&st);
        st.apply(&Prim {
            key: Key::Pages,
            before: st.get(&Key::Pages),
            after: Some(Val::Pages(pages.clone())),
        })
        .unwrap();
        assert_eq!(st.pages, pages);
        st.apply(&Prim {
            key: Key::Labels,
            before: None,
            after: Some(labels("A-")),
        })
        .unwrap();
        assert_eq!(st.labels.as_ref().unwrap()[0].prefix, "A-");
        st.apply(&Prim {
            key: Key::Labels,
            before: Some(labels("A-")),
            after: None,
        })
        .unwrap();
        assert_eq!(st.labels, None);
    }

    #[test]
    fn a_wrong_before_is_a_mismatch_and_changes_nothing() {
        let mut st = state();
        let was = st.clone();
        let wrong = Prim {
            key: Key::Pages,
            before: Some(Val::Pages(reversed(&st))),
            after: Some(Val::Pages(Vector::new())),
        };
        assert_eq!(st.apply(&wrong), Err(Mismatch { key: Key::Pages }));
        let absent = Prim {
            key: Key::Labels,
            before: Some(labels("A-")),
            after: None,
        };
        assert_eq!(st.apply(&absent), Err(Mismatch { key: Key::Labels }));
        // Pages always hold a value, and only pages.
        let misfit = Prim {
            key: Key::Pages,
            before: st.get(&Key::Pages),
            after: Some(labels("A-")),
        };
        assert_eq!(st.apply(&misfit), Err(Mismatch { key: Key::Pages }));
        assert_eq!(st, was);
    }

    #[test]
    fn same_sees_equal_values_as_the_same() {
        let st = state();
        let (a, b) = (Val::Pages(reversed(&st)), Val::Pages(reversed(&st)));
        let (Val::Pages(va), Val::Pages(vb)) = (&a, &b) else {
            unreachable!()
        };
        assert!(!va.ptr_eq(vb), "built apart");
        assert!(same(Some(&a), Some(&b)));
        assert!(!same(Some(&a), st.get(&Key::Pages).as_ref()));
        assert!(same(None, None));
        assert!(!same(Some(&labels("A-")), None));
        assert!(!same(Some(&labels("A-")), Some(&labels("B-"))));
        assert!(
            !same(Some(&a), Some(&labels("A-"))),
            "another kind of value"
        );
    }

    #[test]
    fn heap_size_counts_buffers_and_what_they_hold() {
        let names = vec!["abc".to_string(), String::with_capacity(10)];
        let strings = names.iter().map(String::capacity).sum::<usize>();
        assert_eq!(
            names.heap_size(),
            names.capacity() * size_of::<String>() + strings
        );
        // An im vector counts as its elements.
        let pages = Val::Pages(state().pages);
        assert_eq!(pages.heap_size(), 3 * size_of::<PageRef>());
        assert_eq!(labels("A-").heap_size(), size_of::<LabelSpec>() + 2);
    }

    #[test]
    fn finish_drops_a_key_put_back() {
        let st = state();
        let mut rec = Rec::new(st.clone());
        rec.put(Key::Labels, Some(labels("A-")));
        rec.put(Key::TextLayer(PageId(1)), None);
        rec.put(Key::Pages, Some(Val::Pages(reversed(&st))));
        assert_eq!(rec.pages[0].id, PageId(2), "it reads its own writes");
        rec.put(Key::Labels, None);
        let (after, prims) = rec.finish();
        let keys: Vec<Key> = prims.iter().map(|p| p.key).collect();
        assert_eq!(keys, [Key::Pages]);
        assert_eq!(after.pages, reversed(&st));
        // The prims take the state it started from to the one it made.
        let mut replayed = st;
        for p in &prims {
            replayed.apply(p).unwrap();
        }
        assert_eq!(replayed, after);
    }
}
