//! The WIT worlds this host serves: 0.3, the current one, and 0.2 (N−1),
//! each bound on its own (their names differ by version, so both link
//! into one `Linker`). What they have in common is written once,
//! `shared.rs`, and included in each version's module, where `api` is
//! that version's bindings. Marks (0.2 has fewer kinds and fields) and
//! `storage` (0.3 adds preferences) differ: `v03.rs`, `v02.rs`.

pub(crate) mod bindings_v03 {
    wasmtime::component::bindgen!({
        world: "plugin",
        path: "../../wit",
        with: {
            "sheaf:plugin/render.page-renderer": crate::render::PageRenderer,
            "sheaf:plugin/render.image": crate::render::RenderedImage,
            "sheaf:plugin/progress.task": crate::progress::Task,
        },
    });
}

pub(crate) mod bindings_v02 {
    wasmtime::component::bindgen!({
        world: "plugin",
        path: "../../wit/0.2",
        with: {
            "sheaf:plugin/render.page-renderer": crate::render::PageRenderer,
            "sheaf:plugin/render.image": crate::render::RenderedImage,
            "sheaf:plugin/progress.task": crate::progress::Task,
        },
    });
}

/// API 0.3.
pub(crate) mod v03 {
    pub(crate) use super::bindings_v03::Plugin as World;
    use super::bindings_v03::sheaf::plugin as api;

    include!("shared.rs");
    include!("v03.rs");
}

/// API 0.2, still served.
pub(crate) mod v02 {
    pub(crate) use super::bindings_v02::Plugin as World;
    use super::bindings_v02::sheaf::plugin as api;

    include!("shared.rs");
    include!("v02.rs");
}

#[cfg(test)]
mod tests {
    use sheaf_core::Rotation;
    use sheaf_doc::measure::{Measured, Scale};
    use sheaf_doc::{Annot, AnnotId, AnnotKind, Edit, OutlineId, Pt};

    fn mark(kind: AnnotKind) -> Annot {
        Annot {
            id: AnnotId(7),
            kind,
            color: [1, 2, 3],
            opacity: 0.5,
            stroke: 2.0,
            author: "Greg".into(),
            contents: "c".into(),
            modified: "D:20260101000000".into(),
            created: "D:20260101000000".into(),
            fill: None,
            dashed: false,
            cloudy: false,
            turn: Rotation::R0,
            name: String::new(),
        }
    }

    /// What a plugin's mark comes back as, but for what the host sets.
    fn same(a: &Annot, back: Annot) {
        assert_eq!(
            back,
            Annot {
                modified: back.modified.clone(),
                created: back.created.clone(),
                ..a.clone()
            }
        );
    }

    #[test]
    fn marks_round_trip_in_both_apis() {
        let ink = mark(AnnotKind::Ink {
            paths: vec![vec![Pt { x: 1.0, y: 2.0 }, Pt { x: 3.0, y: 4.0 }]],
        });
        same(
            &ink,
            super::v02::annot_from_wit(super::v02::annot_to_wit(&ink).unwrap()),
        );
        same(
            &ink,
            super::v03::annot_from_wit(super::v03::annot_to_wit(&ink).unwrap()),
        );
        // 0.3 has every kind and field; 0.2 doesn't show the new kinds.
        let mut area = mark(AnnotKind::Measure {
            what: Measured::Area,
            points: vec![
                Pt { x: 0.0, y: 0.0 },
                Pt { x: 72.0, y: 0.0 },
                Pt { x: 72.0, y: 72.0 },
            ],
            scale: Box::new(Scale::default()),
        });
        area.dashed = true;
        same(
            &area,
            super::v03::annot_from_wit(super::v03::annot_to_wit(&area).unwrap()),
        );
        assert!(super::v02::annot_to_wit(&area).is_none());
        let mut poly = mark(AnnotKind::Polygon {
            points: vec![
                Pt { x: 0.0, y: 0.0 },
                Pt { x: 9.0, y: 0.0 },
                Pt { x: 9.0, y: 9.0 },
            ],
        });
        poly.fill = Some([9, 9, 9]);
        poly.cloudy = true;
        let wit = super::v03::annot_to_wit(&poly).unwrap();
        assert_eq!(wit.name, "sheaf:7");
        same(&poly, super::v03::annot_from_wit(wit));
    }

    /// A plugin's units are sheaf's, with the nouns the status bar shows.
    #[test]
    fn units_cross_both_apis() {
        use super::bindings_v02::sheaf::plugin::progress::Unit as V02;
        use super::bindings_v03::sheaf::plugin::progress::Unit as V03;
        use in_progress::Unit;
        let v02 = [V02::Items, V02::Pages, V02::Bytes].map(super::v02::unit);
        let v03 = [V03::Items, V03::Pages, V03::Bytes].map(super::v03::unit);
        for units in [v02, v03] {
            assert_eq!(units, [Unit::ITEMS, sheaf_doc::PAGES, Unit::Bytes]);
            assert_eq!(units.map(Unit::noun), ["items", "pages", "bytes"]);
        }
    }

    #[test]
    fn outline_edit_keeps_clear_vs_unchanged() {
        use super::bindings_v03::sheaf::plugin::types as w;
        let e = super::v03::edit_from_wit(
            None,
            w::Edit::OutlineEdit(w::OutlineEdit {
                id: 3,
                title: None,
                target: Some(None),
                color: None,
                view: None,
            }),
        );
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
