//! Between the WIT types and sheaf's own.

use sheaf_core::{PageSize, Rotation};
use sheaf_doc::{
    Annot, AnnotId, AnnotKind, Box2, DestView, Edit, NewOutlineItem, OutlineId, OutlineNode,
    PageId, PageRef, Pt, Snapshot,
};

use crate::runtime::bindings::sheaf::plugin::types as w;
use crate::runtime::bindings::sheaf::plugin::ui as wu;
use crate::runtime::{DocEvent, TextMatch};
use crate::ui;

pub fn rotation_to_wit(r: Rotation) -> w::Rotation {
    match r {
        Rotation::R0 => w::Rotation::R0,
        Rotation::R90 => w::Rotation::R90,
        Rotation::R180 => w::Rotation::R180,
        Rotation::R270 => w::Rotation::R270,
    }
}

pub fn rotation_from_wit(r: w::Rotation) -> Rotation {
    match r {
        w::Rotation::R0 => Rotation::R0,
        w::Rotation::R90 => Rotation::R90,
        w::Rotation::R180 => Rotation::R180,
        w::Rotation::R270 => Rotation::R270,
    }
}

fn rgb_to_wit(c: [u8; 3]) -> w::Rgb {
    w::Rgb {
        r: c[0],
        g: c[1],
        b: c[2],
    }
}

fn rgb_from_wit(c: w::Rgb) -> [u8; 3] {
    [c.r, c.g, c.b]
}

fn pt_to_wit(p: Pt) -> w::Pt {
    w::Pt { x: p.x, y: p.y }
}

fn pt_from_wit(p: w::Pt) -> Pt {
    Pt { x: p.x, y: p.y }
}

pub fn box_to_wit(b: Box2) -> w::Box2 {
    w::Box2 {
        x: b.x,
        y: b.y,
        w: b.w,
        h: b.h,
    }
}

pub fn box_from_wit(b: w::Box2) -> Box2 {
    Box2 {
        x: b.x,
        y: b.y,
        w: b.w,
        h: b.h,
    }
}

/// A page's size as displayed: its source size turned by its rotation.
pub fn page_size(snap: &Snapshot, p: &PageRef) -> PageSize {
    snap.source(p.source)
        .map(|s| s.page_size(p.index))
        .unwrap_or(PageSize {
            width: 612.0,
            height: 792.0,
        })
        .rotated(p.rotation)
}

pub fn layer_to_wit(
    l: &sheaf_doc::TextLayer,
) -> crate::runtime::bindings::sheaf::plugin::ocr::TextLayer {
    use crate::runtime::bindings::sheaf::plugin::ocr as wo;
    wo::TextLayer {
        words: l
            .words
            .iter()
            .map(|w| wo::OcrWord {
                text: w.text.clone(),
                rect: box_to_wit(w.rect),
                confidence: w.confidence,
            })
            .collect(),
        lines: l
            .lines
            .iter()
            .map(|r| (r.start as u32, r.end as u32))
            .collect(),
        engine: l.engine.clone(),
    }
}

pub fn page_info(snap: &Snapshot, p: &PageRef) -> w::PageInfo {
    let size = page_size(snap, p);
    w::PageInfo {
        id: p.id.0,
        rotation: rotation_to_wit(p.rotation),
        width: size.width,
        height: size.height,
    }
}

pub fn dest_to_wit(v: &DestView) -> w::DestView {
    match v {
        DestView::Xyz { left, top, zoom } => w::DestView::Xyz(w::DestXyz {
            left: *left,
            top: *top,
            zoom: *zoom,
        }),
        DestView::Other { kind, params } => w::DestView::Other(w::DestOther {
            kind: kind.clone(),
            params: params.clone(),
        }),
    }
}

pub fn dest_from_wit(v: w::DestView) -> DestView {
    match v {
        w::DestView::Xyz(x) => DestView::Xyz {
            left: x.left,
            top: x.top,
            zoom: x.zoom.filter(|z| *z > 0.0),
        },
        w::DestView::Other(o) => DestView::Other {
            kind: o.kind,
            params: o.params,
        },
    }
}

pub fn outline_item(depth: u16, n: &OutlineNode) -> w::OutlineItem {
    w::OutlineItem {
        id: n.id.0,
        title: n.title.clone(),
        target: n.target.map(|p| p.0),
        view: n.view.as_ref().map(dest_to_wit),
        color: n.color.map(rgb_to_wit),
        depth,
    }
}

pub fn annot_to_wit(a: &Annot) -> w::Annot {
    let kind = match &a.kind {
        AnnotKind::Highlight { rects } => {
            w::AnnotKind::Highlight(rects.iter().copied().map(box_to_wit).collect())
        }
        AnnotKind::Note { pos } => w::AnnotKind::Note(pt_to_wit(*pos)),
        AnnotKind::FreeText { rect, size } => w::AnnotKind::FreeText(w::TextBox {
            rect: box_to_wit(*rect),
            size: *size,
        }),
        AnnotKind::Square { rect } => w::AnnotKind::Square(box_to_wit(*rect)),
        AnnotKind::Ellipse { rect } => w::AnnotKind::Ellipse(box_to_wit(*rect)),
        AnnotKind::Line { a, b, arrow, .. } => w::AnnotKind::Line(w::LineShape {
            a: pt_to_wit(*a),
            b: pt_to_wit(*b),
            arrow: *arrow,
        }),
        AnnotKind::Ink { paths } => w::AnnotKind::Ink(
            paths
                .iter()
                .map(|p| p.iter().copied().map(pt_to_wit).collect())
                .collect(),
        ),
        AnnotKind::Stamp { rect, glyph } => w::AnnotKind::Stamp(w::StampShape {
            rect: box_to_wit(*rect),
            glyph: glyph.clone(),
        }),
        AnnotKind::Connector {
            rect,
            label,
            partner,
        } => w::AnnotKind::Connector(w::ConnectorShape {
            rect: box_to_wit(*rect),
            label: label.clone(),
            partner: partner.map(|p| p.0),
        }),
        AnnotKind::Tape { rect, size } => w::AnnotKind::Tape(w::TextBox {
            rect: box_to_wit(*rect),
            size: *size,
        }),
    };
    w::Annot {
        id: a.id.0,
        kind,
        color: rgb_to_wit(a.color),
        opacity: a.opacity,
        stroke: a.stroke,
        author: a.author.clone(),
        contents: a.contents.clone(),
    }
}

/// A plugin's annotation as sheaf stores it. The author defaults to the
/// user, and the modification time is now.
pub fn annot_from_wit(a: w::Annot) -> Annot {
    let kind = match a.kind {
        w::AnnotKind::Highlight(rects) => AnnotKind::Highlight {
            rects: rects.into_iter().map(box_from_wit).collect(),
        },
        w::AnnotKind::Note(pos) => AnnotKind::Note {
            pos: pt_from_wit(pos),
        },
        w::AnnotKind::FreeText(t) => AnnotKind::FreeText {
            rect: box_from_wit(t.rect),
            size: t.size,
        },
        w::AnnotKind::Square(r) => AnnotKind::Square {
            rect: box_from_wit(r),
        },
        w::AnnotKind::Ellipse(r) => AnnotKind::Ellipse {
            rect: box_from_wit(r),
        },
        w::AnnotKind::Line(l) => AnnotKind::Line {
            a: pt_from_wit(l.a),
            b: pt_from_wit(l.b),
            arrow: l.arrow,
            arrow_start: false,
        },
        w::AnnotKind::Ink(paths) => AnnotKind::Ink {
            paths: paths
                .into_iter()
                .map(|p| p.into_iter().map(pt_from_wit).collect())
                .collect(),
        },
        w::AnnotKind::Stamp(s) => AnnotKind::Stamp {
            rect: box_from_wit(s.rect),
            glyph: s.glyph,
        },
        w::AnnotKind::Connector(c) => AnnotKind::Connector {
            rect: box_from_wit(c.rect),
            label: c.label,
            partner: c.partner.map(AnnotId),
        },
        w::AnnotKind::Tape(t) => AnnotKind::Tape {
            rect: box_from_wit(t.rect),
            size: t.size,
        },
    };
    Annot {
        id: AnnotId(a.id),
        kind,
        color: rgb_from_wit(a.color),
        opacity: a.opacity.clamp(0.0, 1.0),
        stroke: a.stroke.max(0.0),
        // Empty means the user, whom the app names (`settings.toml`).
        author: a.author.trim().to_string(),
        contents: a.contents,
        modified: sheaf_doc::pdf_date_now(),
        created: sheaf_doc::pdf_date_now(),
        fill: None,
        dashed: false,
    }
}

fn page_annot(pa: w::PageAnnot) -> (PageId, Annot) {
    (PageId(pa.page), annot_from_wit(pa.annot))
}

pub fn edit_from_wit(e: w::Edit) -> Edit {
    let pages = |ids: Vec<u64>| ids.into_iter().map(PageId).collect::<Vec<_>>();
    match e {
        w::Edit::MovePages(m) => Edit::MovePages {
            ids: pages(m.ids),
            before: m.before.map(PageId),
        },
        w::Edit::RotatePages(r) => Edit::RotatePages {
            ids: pages(r.ids),
            by: rotation_from_wit(r.by),
        },
        w::Edit::DeletePages(ids) => Edit::DeletePages { ids: pages(ids) },
        w::Edit::DuplicatePages(ids) => Edit::DuplicatePages { ids: pages(ids) },
        w::Edit::OutlineInsert(o) => Edit::OutlineInsert {
            parent: o.parent.map(OutlineId),
            after: o.after.map(OutlineId),
            title: o.title,
            target: o.target.map(PageId),
            view: o.view.map(dest_from_wit),
        },
        w::Edit::OutlineInsertTree(t) => Edit::OutlineInsertTree {
            parent: t.parent.map(OutlineId),
            after: t.after.map(OutlineId),
            items: t
                .items
                .into_iter()
                .map(|i| NewOutlineItem {
                    title: i.title,
                    target: i.target.map(PageId),
                    view: i.view.map(dest_from_wit),
                    depth: i.depth,
                })
                .collect(),
        },
        w::Edit::OutlineMove(o) => Edit::OutlineMove {
            id: OutlineId(o.id),
            parent: o.parent.map(OutlineId),
            after: o.after.map(OutlineId),
            with_pages: o.with_pages,
        },
        w::Edit::OutlineEdit(o) => Edit::OutlineEdit {
            id: OutlineId(o.id),
            title: o.title,
            target: o.target.map(|t| t.map(PageId)),
            color: o.color.map(|c| c.map(rgb_from_wit)),
            view: o.view.map(|v| v.map(dest_from_wit)),
            style: None,
        },
        w::Edit::OutlineDelete(id) => Edit::OutlineDelete { id: OutlineId(id) },
        w::Edit::AnnotAdd(pa) => {
            let (page, annot) = page_annot(pa);
            Edit::AnnotAdd { page, annot }
        }
        w::Edit::AnnotUpdate(pa) => {
            let (page, annot) = page_annot(pa);
            Edit::AnnotUpdate { page, annot }
        }
        w::Edit::AnnotDelete(r) => Edit::AnnotDelete {
            page: PageId(r.page),
            id: AnnotId(r.id),
        },
        w::Edit::ConnectorAdd(c) => Edit::ConnectorAdd {
            a: page_annot(c.a),
            b: page_annot(c.b),
        },
    }
}

pub fn event_to_wit(e: &DocEvent) -> w::DocEvent {
    match e {
        DocEvent::Opened => w::DocEvent::Opened,
        DocEvent::Edited(l) => w::DocEvent::Edited(l.clone()),
        DocEvent::Saved(p) => w::DocEvent::Saved(p.clone()),
        DocEvent::SelectionChanged => w::DocEvent::SelectionChanged,
        DocEvent::Closed => w::DocEvent::Closed,
        DocEvent::Region { page, rect } => w::DocEvent::Region(w::Region {
            page: page.0,
            rect: box_to_wit(*rect),
        }),
        DocEvent::RegionCleared => w::DocEvent::RegionCleared,
    }
}

pub fn text_match_to_wit(m: TextMatch) -> w::TextMatch {
    w::TextMatch {
        text: m.text,
        rects: m.rects.into_iter().map(box_to_wit).collect(),
        groups: m.groups,
    }
}

pub fn form_from_wit(f: wu::Form) -> ui::Form {
    ui::Form {
        title: f.title,
        submit_label: f
            .submit_label
            .filter(|l| !l.trim().is_empty())
            .unwrap_or_else(|| "OK".into()),
        fields: f
            .fields
            .into_iter()
            .map(|f| ui::Field {
                id: f.id,
                label: f.label,
                kind: match f.kind {
                    wu::FieldKind::Text(t) => ui::FieldKind::Text {
                        default: t.default,
                        placeholder: t.placeholder,
                    },
                    wu::FieldKind::Number(n) => ui::FieldKind::Number {
                        default: n.default,
                        min: n.min,
                        max: n.max,
                    },
                    wu::FieldKind::Choice(c) => ui::FieldKind::Choice {
                        default: (c.default as usize).min(c.options.len().saturating_sub(1)),
                        options: c.options,
                    },
                    wu::FieldKind::Checkbox(b) => ui::FieldKind::Checkbox(b),
                    wu::FieldKind::File(f) => ui::FieldKind::File {
                        save: f.save,
                        extensions: f.extensions,
                    },
                    wu::FieldKind::PageRange(r) => ui::FieldKind::PageRange(r),
                },
            })
            .collect(),
    }
}

pub fn answer_to_wit(a: ui::FieldAnswer) -> wu::FieldAnswer {
    wu::FieldAnswer {
        id: a.id,
        value: match a.value {
            ui::FieldValue::Text(t) => wu::FieldValue::Text(t),
            ui::FieldValue::Number(n) => wu::FieldValue::Number(n),
            ui::FieldValue::Choice(i) => wu::FieldValue::Choice(i as u32),
            ui::FieldValue::Checkbox(b) => wu::FieldValue::Checkbox(b),
            ui::FieldValue::File(p) => wu::FieldValue::File(p),
            ui::FieldValue::PageRange(ids) => {
                wu::FieldValue::PageRange(ids.into_iter().map(|p| p.0).collect())
            }
        },
    }
}

pub fn row_from_wit(r: wu::PanelRow) -> ui::PanelRow {
    ui::PanelRow {
        id: r.id,
        label: r.label,
        detail: r.detail,
        badge: r.badge,
        icon: match r.icon {
            wu::Icon::None => ui::Icon::None,
            wu::Icon::Check => ui::Icon::Check,
            wu::Icon::Cross => ui::Icon::Cross,
            wu::Icon::Link => ui::Icon::Link,
            wu::Icon::Warning => ui::Icon::Warning,
            wu::Icon::Info => ui::Icon::Info,
            wu::Icon::Page => ui::Icon::Page,
        },
        depth: r.depth,
        page: r.page.map(PageId),
        actions: r
            .actions
            .into_iter()
            .map(|a| ui::RowAction {
                id: a.id,
                label: a.label,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annots_round_trip_except_host_fields() {
        let a = Annot {
            id: AnnotId(7),
            kind: AnnotKind::Ink {
                paths: vec![vec![Pt { x: 1.0, y: 2.0 }, Pt { x: 3.0, y: 4.0 }]],
            },
            color: [1, 2, 3],
            opacity: 0.5,
            stroke: 2.0,
            author: "Greg".into(),
            contents: "c".into(),
            modified: "D:20260101000000".into(),
            created: "D:20260101000000".into(),
            fill: None,
            dashed: false,
        };
        let back = annot_from_wit(annot_to_wit(&a));
        assert_eq!(
            back,
            Annot {
                modified: back.modified.clone(),
                created: back.created.clone(),
                ..a
            }
        );
    }

    #[test]
    fn outline_edit_keeps_clear_vs_unchanged() {
        let e = edit_from_wit(w::Edit::OutlineEdit(w::OutlineEdit {
            id: 3,
            title: None,
            target: Some(None),
            color: None,
            view: None,
        }));
        assert_eq!(
            e,
            Edit::OutlineEdit {
                id: OutlineId(3),
                title: None,
                target: Some(None),
                color: None,
                view: None,
                style: None,
            }
        );
    }
}
