// API 0.2's marks: the kinds and fields it had. A mark of a newer kind
// isn't shown to a 0.2 plugin (M9 decision 12), and one it updates keeps
// what 0.2 can't say (the app's `keep_unseen`).

/// A mark as API 0.2 shows it; None for kinds 0.2 doesn't have, which
/// plugins don't see until 0.3 (M9 decision 12).
pub(crate) fn annot_to_wit(a: &Annot) -> Option<w::Annot> {
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
        AnnotKind::Tape { rect, size, .. } => w::AnnotKind::Tape(w::TextBox {
            rect: box_to_wit(*rect),
            size: *size,
        }),
        AnnotKind::Underline { .. }
        | AnnotKind::StrikeOut { .. }
        | AnnotKind::Squiggly { .. }
        | AnnotKind::Replace { .. }
        | AnnotKind::Insert { .. }
        | AnnotKind::Reply { .. }
        | AnnotKind::Status { .. }
        | AnnotKind::PageStamp { .. }
        | AnnotKind::TextStamp { .. }
        | AnnotKind::PageBorder { .. }
        | AnnotKind::Polygon { .. }
        | AnnotKind::Polyline { .. }
        | AnnotKind::Callout { .. }
        | AnnotKind::FileAttachment { .. }
        | AnnotKind::Measure { .. } => return None,
    };
    Some(w::Annot {
        id: a.id.0,
        kind,
        color: rgb_to_wit(a.color),
        opacity: a.opacity,
        stroke: a.stroke,
        author: a.author.clone(),
        contents: a.contents.clone(),
    })
}

/// A plugin's annotation as sheaf stores it. The author defaults to the
/// user, and the modification time is now.
pub(crate) fn annot_from_wit(a: w::Annot) -> Annot {
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
        // A plugin sees a tape as its text (`contents`); its rows are read
        // back from that.
        w::AnnotKind::Tape(t) => AnnotKind::Tape {
            rect: box_from_wit(t.rect),
            size: t.size,
            tape: Default::default(),
            look: Default::default(),
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
        cloudy: false,
        turn: Rotation::R0,
        name: String::new(),
    }
}

impl api::storage::Host for HostState {
    fn get(&mut self, key: String) -> Option<String> {
        self.storage.get(&key)
    }

    fn set(&mut self, key: String, value: String) -> Result<(), String> {
        self.storage.set(key, value)
    }

    fn delete(&mut self, key: String) {
        if let Err(e) = self.storage.delete(&key) {
            log::warn!(target: "plugin", "[{}] {e}", self.plugin);
        }
    }

    fn keys(&mut self) -> Vec<String> {
        self.storage.keys()
    }

    fn settings(&mut self) -> String {
        self.storage.settings()
    }
}
