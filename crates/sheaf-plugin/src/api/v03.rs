// API 0.3's marks: every kind and field there is. And `storage`, which
// adds the plugin's preferences.

/// A mark as API 0.3 shows it: all of it.
pub(crate) fn annot_to_wit(a: &Annot) -> Option<w::Annot> {
    let boxes = |r: &[Box2]| r.iter().copied().map(box_to_wit).collect::<Vec<_>>();
    let points = |p: &[Pt]| p.iter().copied().map(pt_to_wit).collect::<Vec<_>>();
    let kind = match &a.kind {
        AnnotKind::Highlight { rects } => w::AnnotKind::Highlight(boxes(rects)),
        AnnotKind::Note { pos } => w::AnnotKind::Note(pt_to_wit(*pos)),
        AnnotKind::FreeText { rect, size } => w::AnnotKind::FreeText(w::TextBox {
            rect: box_to_wit(*rect),
            size: *size,
        }),
        AnnotKind::Square { rect } => w::AnnotKind::Square(box_to_wit(*rect)),
        AnnotKind::Ellipse { rect } => w::AnnotKind::Ellipse(box_to_wit(*rect)),
        AnnotKind::Line {
            a,
            b,
            arrow,
            arrow_start,
        } => w::AnnotKind::Line(w::LineShape {
            a: pt_to_wit(*a),
            b: pt_to_wit(*b),
            arrow: *arrow,
            arrow_start: *arrow_start,
        }),
        AnnotKind::Ink { paths } => w::AnnotKind::Ink(paths.iter().map(|p| points(p)).collect()),
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
        AnnotKind::Underline { rects } => w::AnnotKind::Underline(boxes(rects)),
        AnnotKind::StrikeOut { rects } => w::AnnotKind::StrikeOut(boxes(rects)),
        AnnotKind::Squiggly { rects } => w::AnnotKind::Squiggly(boxes(rects)),
        AnnotKind::Replace { rects, struck, at } => w::AnnotKind::Replace(w::ReplaceShape {
            rects: boxes(rects),
            struck: struck.clone(),
            at: box_to_wit(*at),
        }),
        AnnotKind::Insert { at } => w::AnnotKind::Insert(box_to_wit(*at)),
        AnnotKind::Reply { parent } => w::AnnotKind::Reply(parent.clone()),
        AnnotKind::Status {
            parent,
            model,
            state,
        } => w::AnnotKind::Status(w::StatusShape {
            parent: parent.clone(),
            model: model.clone(),
            state: state.clone(),
        }),
        AnnotKind::PageStamp {
            rect,
            stamp,
            name,
            text,
        } => w::AnnotKind::PageStamp(w::PageStampShape {
            rect: box_to_wit(*rect),
            stamp: stamp.clone(),
            name: name.clone(),
            text: text.clone(),
        }),
        AnnotKind::TextStamp { rect, text, status } => w::AnnotKind::TextStamp(w::TextStampShape {
            rect: box_to_wit(*rect),
            text: text.clone(),
            status: *status,
        }),
        AnnotKind::PageBorder { rect, status } => w::AnnotKind::PageBorder(w::PageBorderShape {
            rect: box_to_wit(*rect),
            status: status.clone(),
        }),
        AnnotKind::Polygon { points: p } => w::AnnotKind::Polygon(points(p)),
        AnnotKind::Polyline { points: p } => w::AnnotKind::Polyline(points(p)),
        AnnotKind::Callout {
            rect,
            knee,
            point,
            size,
        } => w::AnnotKind::Callout(w::CalloutShape {
            rect: box_to_wit(*rect),
            knee: pt_to_wit(*knee),
            point: pt_to_wit(*point),
            size: *size,
        }),
        AnnotKind::FileAttachment {
            pos,
            icon,
            file,
            name,
            size,
        } => w::AnnotKind::FileAttachment(w::AttachmentShape {
            pos: pt_to_wit(*pos),
            icon: icon.clone(),
            file: file.clone(),
            name: name.clone(),
            size: *size,
        }),
        AnnotKind::Measure {
            what,
            points: p,
            scale,
        } => w::AnnotKind::Measure(w::MeasureShape {
            what: match what {
                sheaf_doc::measure::Measured::Distance => w::Measured::Distance,
                sheaf_doc::measure::Measured::Perimeter => w::Measured::Perimeter,
                sheaf_doc::measure::Measured::Area => w::Measured::Area,
            },
            points: points(p),
            scale: w::Scale {
                page: scale.page,
                page_unit: scale.page_unit.clone(),
                real: scale.real,
                unit: scale.unit.clone(),
                precision: scale.precision,
            },
        }),
    };
    Some(w::Annot {
        id: a.id.0,
        kind,
        color: rgb_to_wit(a.color),
        opacity: a.opacity,
        stroke: a.stroke,
        author: a.author.clone(),
        contents: a.contents.clone(),
        fill: a.fill.map(rgb_to_wit),
        dashed: a.dashed,
        cloudy: a.cloudy,
        created: a.created.clone(),
        modified: a.modified.clone(),
        name: a.name(),
    })
}

/// A plugin's mark as sheaf stores it. The author defaults to the user;
/// the dates are now, and the name is the host's (an update keeps the
/// mark's own: the app's `keep_unseen`).
pub(crate) fn annot_from_wit(a: w::Annot) -> Annot {
    let boxes = |r: Vec<w::Box2>| r.into_iter().map(box_from_wit).collect::<Vec<_>>();
    let points = |p: Vec<w::Pt>| p.into_iter().map(pt_from_wit).collect::<Vec<_>>();
    let kind = match a.kind {
        w::AnnotKind::Highlight(rects) => AnnotKind::Highlight {
            rects: boxes(rects),
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
            arrow_start: l.arrow_start,
        },
        w::AnnotKind::Ink(paths) => AnnotKind::Ink {
            paths: paths.into_iter().map(points).collect(),
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
        w::AnnotKind::Underline(r) => AnnotKind::Underline { rects: boxes(r) },
        w::AnnotKind::StrikeOut(r) => AnnotKind::StrikeOut { rects: boxes(r) },
        w::AnnotKind::Squiggly(r) => AnnotKind::Squiggly { rects: boxes(r) },
        w::AnnotKind::Replace(r) => AnnotKind::Replace {
            rects: boxes(r.rects),
            struck: r.struck,
            at: box_from_wit(r.at),
        },
        w::AnnotKind::Insert(at) => AnnotKind::Insert {
            at: box_from_wit(at),
        },
        w::AnnotKind::Reply(parent) => AnnotKind::Reply { parent },
        w::AnnotKind::Status(s) => AnnotKind::Status {
            parent: s.parent,
            model: s.model,
            state: s.state,
        },
        w::AnnotKind::PageStamp(s) => AnnotKind::PageStamp {
            rect: box_from_wit(s.rect),
            stamp: s.stamp,
            name: s.name,
            text: s.text,
        },
        w::AnnotKind::TextStamp(s) => AnnotKind::TextStamp {
            rect: box_from_wit(s.rect),
            text: s.text,
            status: s.status,
        },
        w::AnnotKind::PageBorder(b) => AnnotKind::PageBorder {
            rect: box_from_wit(b.rect),
            status: b.status,
        },
        w::AnnotKind::Polygon(p) => AnnotKind::Polygon { points: points(p) },
        w::AnnotKind::Polyline(p) => AnnotKind::Polyline { points: points(p) },
        w::AnnotKind::Callout(c) => AnnotKind::Callout {
            rect: box_from_wit(c.rect),
            knee: pt_from_wit(c.knee),
            point: pt_from_wit(c.point),
            size: c.size,
        },
        w::AnnotKind::FileAttachment(f) => AnnotKind::FileAttachment {
            pos: pt_from_wit(f.pos),
            icon: f.icon,
            file: f.file,
            name: f.name,
            size: f.size,
        },
        w::AnnotKind::Measure(m) => AnnotKind::Measure {
            what: match m.what {
                w::Measured::Distance => sheaf_doc::measure::Measured::Distance,
                w::Measured::Perimeter => sheaf_doc::measure::Measured::Perimeter,
                w::Measured::Area => sheaf_doc::measure::Measured::Area,
            },
            points: points(m.points),
            scale: Box::new(sheaf_doc::measure::Scale {
                page: m.scale.page,
                page_unit: m.scale.page_unit,
                real: m.scale.real,
                unit: m.scale.unit,
                precision: m.scale.precision,
            }),
        },
    };
    let now = sheaf_doc::pdf_date_now();
    Annot {
        id: AnnotId(a.id),
        fill: a.fill.map(rgb_from_wit).filter(|_| kind.fillable()),
        cloudy: a.cloudy && kind.cloudable(),
        kind,
        color: rgb_from_wit(a.color),
        opacity: a.opacity.clamp(0.0, 1.0),
        stroke: a.stroke.max(0.0),
        // Empty means the user, whom the app names (`settings.toml`).
        author: a.author.trim().to_string(),
        contents: a.contents,
        modified: now.clone(),
        created: now,
        dashed: a.dashed,
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

    fn preferences(&mut self) -> String {
        self.call
            .as_ref()
            .map(|c| c.preferences.clone())
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| "{}".into())
    }
}
