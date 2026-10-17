//! Markup annotations: the model and their PDF serialization (dictionary +
//! appearance stream).
//!
//! A mark's geometry is in PageSpace (M10.5 §5): its page's default user
//! space, points, y up, before `/Rotate`, the space `/Rect` and
//! `/QuadPoints` are in, so nothing moves it but an edit of it. Its
//! [`turn`](Annot::turn) is the rotation it reads upright in. Its layout
//! (bounds, hit tests, where its text and lines go) is written y down, for
//! the mark in a frame that reads upright: [`Annot::upright`], or a view's.
//! The writer lays a mark out upright and maps that into PageSpace.
//!
//! Every annotation we write carries `/NM sheaf:<id>` and a `/SheafAnnot`
//! JSON copy of the model ([`Payload`]), so a file we saved can be reopened
//! with the marks editable and PDFium told to hide its rendering of them.
//! Beside the mark, the copy holds a digest of the dictionary's standard
//! keys ([`staleness`]): a mark another tool has edited since no longer
//! matches it, and opens as that tool's comment (M10.5 §3.6).
//!
//! The JSON is a file format. Changes to it are additive: a new kind is a
//! new variant and a new field has `#[serde(default)]`, so older files keep
//! reading. Version 1, from before M10.5, held the mark in its page's
//! displayed space; it's read from the standard keys (`file::sheaf_mark`).
//! A mark this build can't read (a kind from a newer sheaf) isn't ours to
//! rewrite: it's kept and shown like another tool's (see `file::is_ours`).
//! Every mark also writes a full appearance, so other viewers, and older
//! sheaf builds, draw it. Appearances are drawn with the content toolkit's
//! writer, their text in the standard 14 fonts; a character those lack
//! draws `?`, which [`Annot::unencodable`] tells without drawing anything.
//! A save shares what its marks draw with through a [`WriteCx`].

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context as _;
use lopdf::{Dictionary, Object, Stream, StringFormat, dictionary};
use serde::{Deserialize, Serialize};
use sheaf_core::Rotation;
use unicode_normalization::UnicodeNormalization;

use crate::assets::{self, MarkAssets};
use crate::attachments::{Objects, resolve};
use crate::content::{
    Blend, Color, ContentWriter, FillRule, Font, GState, LineCap, LineJoin, Paint, Res, ResObjects,
    Reserved, ResourceNeeds, Std14,
};
use crate::geom::{Affine, Geom, Rect};
use crate::store::BlobId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AnnotId(pub u64);

impl AnnotId {
    /// What a mark with this id and no name of its own is called in the
    /// file (`/NM`), and the StrikeOut of a replacement: the names PDFium
    /// is told to hide.
    pub fn sheaf_names(self) -> [String; 2] {
        [
            format!("sheaf:{}", self.0),
            format!("sheaf:{}:strike", self.0),
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

/// A box, `w` and `h` not negative, with `(x, y)` its low corner: its
/// bottom-left in PageSpace, its top-left in a frame that's y down. So
/// [`bottom`](Self::bottom) is its top in PageSpace.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Box2 {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Box2 {
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn contains(&self, p: Pt) -> bool {
        p.x >= self.x && p.x <= self.right() && p.y >= self.y && p.y <= self.bottom()
    }
    pub fn union(self, o: Box2) -> Box2 {
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        Box2 {
            x,
            y,
            w: self.right().max(o.right()) - x,
            h: self.bottom().max(o.bottom()) - y,
        }
    }
    pub fn from_corners(a: Pt, b: Pt) -> Box2 {
        Box2 {
            x: a.x.min(b.x),
            y: a.y.min(b.y),
            w: (a.x - b.x).abs(),
            h: (a.y - b.y).abs(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AnnotKind {
    /// Text markup over glyph boxes.
    Highlight {
        rects: Vec<Box2>,
    },
    /// Sticky note: an icon at `pos` (its box's low corner), text in
    /// `contents`.
    Note {
        pos: Pt,
    },
    /// Text drawn in a box.
    FreeText {
        rect: Box2,
        size: f32,
    },
    Square {
        rect: Box2,
    },
    Ellipse {
        rect: Box2,
    },
    Line {
        a: Pt,
        b: Pt,
        /// An arrowhead at `b`.
        arrow: bool,
        /// An arrowhead at `a`.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        arrow_start: bool,
    },
    Ink {
        paths: Vec<Vec<Pt>>,
    },
    /// A stamp from the tickmark library: `glyph` is what it shows.
    Stamp {
        rect: Box2,
        glyph: String,
    },
    /// PDFlyer-style connector: one end of a linked pair. `label` is the
    /// pair's number; `partner` is the other end, on any page. Written as a
    /// Link annotation whose `/Dest` is the partner's page, so it jumps in
    /// other viewers too.
    Connector {
        rect: Box2,
        label: String,
        partner: Option<AnnotId>,
    },
    /// Calculator tape, as PDFlyer prints it (see `tape`): `tape`'s rows,
    /// `size` points tall, drawn on paper as `look` says. `contents` holds
    /// its text for other viewers; a tape from before rows were kept has
    /// only that, and is read from it (`Annot::tape`).
    Tape {
        rect: Box2,
        size: f32,
        #[serde(default, skip_serializing_if = "crate::tape::Tape::is_empty")]
        tape: crate::tape::Tape,
        #[serde(default)]
        look: crate::tape::TapeLook,
    },
    /// Text markup: a line under the text.
    Underline {
        rects: Vec<Box2>,
    },
    /// Text markup: a line through the text.
    StrikeOut {
        rects: Vec<Box2>,
    },
    /// Text markup: a wavy line under the text.
    Squiggly {
        rects: Vec<Box2>,
    },
    /// A suggested replacement (Acrobat's Replace Text): the text `struck`,
    /// over `rects`, is crossed out, and a caret at `at` carries the new
    /// text in `contents`. Written as a Caret with the StrikeOut as its
    /// grouped companion, as Acrobat writes them.
    Replace {
        rects: Vec<Box2>,
        struck: String,
        at: Box2,
    },
    /// A suggested insertion (Insert Text): a caret at `at`, the text in
    /// `contents`.
    Insert {
        at: Box2,
    },
    /// A reply (`contents`) in the thread of the comment whose `/NM` is
    /// `parent`: one of ours (`sheaf:<id>`) or another tool's. Written as
    /// Acrobat writes replies: a Text annotation with `/IRT` to its parent,
    /// drawn by no one.
    Reply {
        parent: String,
    },
    /// A review state set on the comment `parent`: `model` is `Review`
    /// (`state` None, Accepted, Rejected, Cancelled or Completed) or
    /// `Marked` (the checkmark: Marked or Unmarked). Written as Acrobat
    /// writes them: a hidden Text annotation with `/IRT`, `/State` and
    /// `/StateModel`. Each change is a new one, so they keep the history.
    Status {
        parent: String,
        model: String,
        state: String,
    },
    /// A stamp from the library: a PDF page drawn in `rect`. `stamp` names
    /// its one-page PDF by its stem in the generated-file cache, which the
    /// document's [`MarkAssets`] find (a saved file holds the page as a
    /// form), `name` is what the library calls it, and `text` (a dynamic
    /// stamp's, filled in when placed) is drawn over it.
    PageStamp {
        rect: Box2,
        stamp: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    /// A text stamp in a frame, as Acrobat's dynamic stamps look: the first
    /// line large ("APPROVED"), the rest small ("GE 10/14/2026"), filled in
    /// when placed.
    TextStamp {
        rect: Box2,
        text: String,
        /// Placed by Set Status, so Reset Status can take it off.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        status: bool,
    },
    /// A border around a page, as Set Status draws it; `status` names the
    /// status, so Reset Status finds it.
    PageBorder {
        rect: Box2,
        status: String,
    },
    /// A closed shape through `points`.
    Polygon {
        points: Vec<Pt>,
    },
    /// An open line through `points`.
    Polyline {
        points: Vec<Pt>,
    },
    /// A text box (`contents`, `size` points) with a leader line from
    /// its edge through `knee` to an arrowhead at `point`: Acrobat's
    /// callout.
    Callout {
        rect: Box2,
        knee: Pt,
        point: Pt,
        size: f32,
    },
    /// A file attached at `pos` (Acrobat's Attach File), its icon's box's
    /// low corner as a note's is. `file` names its bytes by their stem in
    /// the cache (`generated::store_blob`), which the document's
    /// [`MarkAssets`] find (a saved file embeds them); `name` and `size`
    /// are the file's. `icon` is one of [`ATTACHMENT_ICONS`].
    FileAttachment {
        pos: Pt,
        icon: String,
        file: String,
        name: String,
        size: u64,
    },
    /// A measurement (Acrobat's Measure tools): what `points` come to at
    /// `scale` (see [`crate::measure`]); a distance has two. Its label is
    /// drawn with it and kept in `contents`.
    Measure {
        what: crate::measure::Measured,
        points: Vec<Pt>,
        /// Boxed, so every other mark isn't the bigger for it.
        scale: Box<crate::measure::Scale>,
    },
}

/// The icons an attachment can have, as PDF names them.
pub const ATTACHMENT_ICONS: [&str; 4] = ["PushPin", "Paperclip", "Graph", "Tag"];

/// What Previous/Next walks through: connectors by label, tickmarks by
/// glyph, stamps by name, as in PDFlyer.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Group {
    Connector(String),
    Tickmark(String),
    Stamp(String),
}

impl Group {
    /// "connector 3", "tickmark ✓", "stamp Approved".
    pub fn label(&self) -> String {
        match self {
            Group::Connector(l) => format!("connector {l}"),
            Group::Tickmark(g) => format!("tickmark {g}"),
            Group::Stamp(n) => format!("stamp {n}"),
        }
    }
}

/// The group a mark is in, if it's one that's walked through.
pub fn group_of(kind: &AnnotKind) -> Option<Group> {
    match kind {
        AnnotKind::Connector { label, .. } => Some(Group::Connector(label.clone())),
        AnnotKind::Stamp { glyph, .. } => Some(Group::Tickmark(glyph.clone())),
        AnnotKind::PageStamp { name, .. } => Some(Group::Stamp(name.clone())),
        AnnotKind::TextStamp { text, .. } => Some(Group::Stamp(
            text.lines().next().unwrap_or_default().to_string(),
        )),
        _ => None,
    }
}

/// Connector labels in order, as PDFlyer offers them: 1–50, then A–Z,
/// then AA–ZZ.
pub fn connector_labels() -> impl Iterator<Item = String> {
    let letters = || (b'A'..=b'Z').map(|c| c as char);
    (1..=50)
        .map(|n| n.to_string())
        .chain(letters().map(|c| c.to_string()))
        .chain(letters().flat_map(move |a| letters().map(move |b| format!("{a}{b}"))))
}

/// The markup colors the toolbar offers, with their names.
pub const COLORS: &[([u8; 3], &str)] = &[
    ([255, 230, 0], "Yellow"),
    ([220, 40, 40], "Red"),
    ([0, 120, 255], "Blue"),
    ([0, 150, 0], "Green"),
    ([255, 140, 0], "Orange"),
    ([160, 60, 220], "Purple"),
    ([20, 20, 20], "Black"),
];

/// The review state models (`/StateModel`).
pub const REVIEW: &str = "Review";
pub const MARKED: &str = "Marked";
/// The review states sheaf offers, in menu order.
pub const REVIEW_STATES: [&str; 5] = ["None", "Accepted", "Rejected", "Cancelled", "Completed"];

/// A box array as `[x0, y0, x1, y1]`, normalized.
pub fn box_of(arr: &[lopdf::Object]) -> Option<[f32; 4]> {
    if arr.len() != 4 {
        return None;
    }
    let v: Vec<f32> = arr
        .iter()
        .map(|o| o.as_float().unwrap_or(0.0) as f32)
        .collect();
    Some([
        v[0].min(v[2]),
        v[1].min(v[3]),
        v[0].max(v[2]),
        v[1].max(v[3]),
    ])
}

/// A note's or an attachment's icon box, its low corner at `pos`.
pub fn icon_box(pos: Pt) -> Box2 {
    Box2 {
        x: pos.x,
        y: pos.y,
        w: NOTE_SIZE,
        h: NOTE_SIZE,
    }
}

impl AnnotKind {
    /// The same shape with every point mapped by `mp`, every box by `mb`
    /// and an icon's corner by `icon_at`; `size` maps the text size of
    /// tapes.
    fn map_with(
        &self,
        mp: &dyn Fn(&Pt) -> Pt,
        mb: &dyn Fn(&Box2) -> Box2,
        icon_at: &dyn Fn(&Pt) -> Pt,
        size: &dyn Fn(f32) -> f32,
    ) -> AnnotKind {
        let rs = |rects: &Vec<Box2>| rects.iter().map(mb).collect();
        match self {
            AnnotKind::Highlight { rects } => AnnotKind::Highlight { rects: rs(rects) },
            AnnotKind::Underline { rects } => AnnotKind::Underline { rects: rs(rects) },
            AnnotKind::StrikeOut { rects } => AnnotKind::StrikeOut { rects: rs(rects) },
            AnnotKind::Squiggly { rects } => AnnotKind::Squiggly { rects: rs(rects) },
            AnnotKind::Replace { rects, struck, at } => AnnotKind::Replace {
                rects: rs(rects),
                struck: struck.clone(),
                at: mb(at),
            },
            AnnotKind::Insert { at } => AnnotKind::Insert { at: mb(at) },
            AnnotKind::Reply { .. } | AnnotKind::Status { .. } => self.clone(),
            AnnotKind::PageStamp {
                rect,
                stamp,
                name,
                text,
            } => AnnotKind::PageStamp {
                rect: mb(rect),
                stamp: stamp.clone(),
                name: name.clone(),
                text: text.clone(),
            },
            AnnotKind::TextStamp { rect, text, status } => AnnotKind::TextStamp {
                rect: mb(rect),
                text: text.clone(),
                status: *status,
            },
            AnnotKind::PageBorder { rect, status } => AnnotKind::PageBorder {
                rect: mb(rect),
                status: status.clone(),
            },
            AnnotKind::Polygon { points } => AnnotKind::Polygon {
                points: points.iter().map(mp).collect(),
            },
            AnnotKind::Polyline { points } => AnnotKind::Polyline {
                points: points.iter().map(mp).collect(),
            },
            AnnotKind::Measure {
                what,
                points,
                scale,
            } => AnnotKind::Measure {
                what: *what,
                points: points.iter().map(mp).collect(),
                scale: scale.clone(),
            },
            AnnotKind::Callout {
                rect,
                knee,
                point,
                size: s,
            } => AnnotKind::Callout {
                rect: mb(rect),
                knee: mp(knee),
                point: mp(point),
                size: *s,
            },
            AnnotKind::FileAttachment {
                pos,
                icon,
                file,
                name,
                size,
            } => AnnotKind::FileAttachment {
                pos: icon_at(pos),
                icon: icon.clone(),
                file: file.clone(),
                name: name.clone(),
                size: *size,
            },
            AnnotKind::Note { pos } => AnnotKind::Note { pos: icon_at(pos) },
            AnnotKind::FreeText { rect, size } => AnnotKind::FreeText {
                rect: mb(rect),
                size: *size,
            },
            AnnotKind::Square { rect } => AnnotKind::Square { rect: mb(rect) },
            AnnotKind::Ellipse { rect } => AnnotKind::Ellipse { rect: mb(rect) },
            AnnotKind::Line {
                a,
                b,
                arrow,
                arrow_start,
            } => AnnotKind::Line {
                a: mp(a),
                b: mp(b),
                arrow: *arrow,
                arrow_start: *arrow_start,
            },
            AnnotKind::Ink { paths } => AnnotKind::Ink {
                paths: paths.iter().map(|p| p.iter().map(mp).collect()).collect(),
            },
            AnnotKind::Stamp { rect, glyph } => AnnotKind::Stamp {
                rect: mb(rect),
                glyph: glyph.clone(),
            },
            AnnotKind::Connector {
                rect,
                label,
                partner,
            } => AnnotKind::Connector {
                rect: mb(rect),
                label: label.clone(),
                partner: *partner,
            },
            AnnotKind::Tape {
                rect,
                size: s,
                tape,
                look,
            } => AnnotKind::Tape {
                rect: mb(rect),
                size: size(*s),
                tape: tape.clone(),
                look: look.clone(),
            },
        }
    }

    /// Its bounding box, in a frame that's y down (see
    /// [`Annot::upright`]).
    pub fn bounds(&self) -> Box2 {
        let union = |rects: &[Box2]| {
            rects.iter().copied().reduce(Box2::union).unwrap_or(Box2 {
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 0.0,
            })
        };
        match self {
            AnnotKind::Highlight { rects }
            | AnnotKind::Underline { rects }
            | AnnotKind::StrikeOut { rects }
            | AnnotKind::Squiggly { rects } => union(rects),
            AnnotKind::Replace { rects, at, .. } => union(rects).union(*at),
            AnnotKind::Insert { at } => *at,
            // Not on the page: they're in their parent's thread.
            AnnotKind::Reply { .. } | AnnotKind::Status { .. } => union(&[]),
            AnnotKind::Polygon { points } | AnnotKind::Polyline { points } => union(
                &points
                    .iter()
                    .map(|p| Box2 {
                        x: p.x,
                        y: p.y,
                        w: 0.0,
                        h: 0.0,
                    })
                    .collect::<Vec<_>>(),
            ),
            AnnotKind::Callout {
                rect, knee, point, ..
            } => union(&[*rect, Box2::from_corners(*knee, *point)]),
            AnnotKind::Measure {
                what,
                points,
                scale,
            } => union(
                &points
                    .iter()
                    .map(|p| Box2 {
                        x: p.x,
                        y: p.y,
                        w: 0.0,
                        h: 0.0,
                    })
                    .chain(measure_label_box(*what, points, scale))
                    .collect::<Vec<_>>(),
            ),
            AnnotKind::Note { pos } | AnnotKind::FileAttachment { pos, .. } => icon_box(*pos),
            AnnotKind::FreeText { rect, .. }
            | AnnotKind::Tape { rect, .. }
            | AnnotKind::Square { rect }
            | AnnotKind::Ellipse { rect } => *rect,
            AnnotKind::Stamp { rect, .. }
            | AnnotKind::Connector { rect, .. }
            | AnnotKind::PageStamp { rect, .. }
            | AnnotKind::TextStamp { rect, .. }
            | AnnotKind::PageBorder { rect, .. } => *rect,
            AnnotKind::Line { a, b, .. } => Box2::from_corners(*a, *b),
            AnnotKind::Ink { paths } => paths
                .iter()
                .flatten()
                .map(|p| Box2 {
                    x: p.x,
                    y: p.y,
                    w: 0.0,
                    h: 0.0,
                })
                .reduce(Box2::union)
                .unwrap_or(Box2 {
                    x: 0.0,
                    y: 0.0,
                    w: 0.0,
                    h: 0.0,
                }),
        }
    }

    /// The same shape mapped by `m`, a quarter-turn map as a page's
    /// frames are: points pointwise, boxes through [`Affine::apply_box`],
    /// and a note's or an attachment's corner through its icon's box.
    pub fn mapped(&self, m: &Affine) -> AnnotKind {
        self.map_with(
            &|p| m.apply(*p),
            &|b| m.apply_box(*b),
            &|p| {
                let b = m.apply_box(icon_box(*p));
                Pt { x: b.x, y: b.y }
            },
            &|s| s,
        )
    }

    /// The same shape moved by (`dx`, `dy`) points.
    pub fn translated(&self, dx: f32, dy: f32) -> AnnotKind {
        let mp = |p: &Pt| Pt {
            x: p.x + dx,
            y: p.y + dy,
        };
        self.map_with(
            &mp,
            &|b| Box2 {
                x: b.x + dx,
                y: b.y + dy,
                w: b.w,
                h: b.h,
            },
            &mp,
            &|s| s,
        )
    }

    /// The same shape fitted from bounds `from` into bounds `to`: every
    /// point scaled and moved, as dragging a resize handle does. Text sizes
    /// scale with the height of tapes.
    pub fn resized(&self, from: Box2, to: Box2) -> AnnotKind {
        let sx = if from.w > 0.0 { to.w / from.w } else { 1.0 };
        let sy = if from.h > 0.0 { to.h / from.h } else { 1.0 };
        let mp = |p: &Pt| Pt {
            x: to.x + (p.x - from.x) * sx,
            y: to.y + (p.y - from.y) * sy,
        };
        // A turned tape's text runs across the box's height: it scales with
        // the width.
        let along = match self {
            AnnotKind::Tape { look, .. } if look.rotation % 180 == 90 => sx,
            _ => sy,
        };
        self.map_with(
            &mp,
            &|b| {
                let p = mp(&Pt { x: b.x, y: b.y });
                Box2 {
                    x: p.x,
                    y: p.y,
                    w: b.w * sx,
                    h: b.h * sy,
                }
            },
            &mp,
            &|s| (s * along).max(4.0),
        )
    }

    /// How the mark can be reshaped by dragging: its two ends, a box, or
    /// not at all (sticky notes and connectors keep their size; a
    /// highlight follows its text).
    pub fn handles(&self) -> Handles {
        match self {
            AnnotKind::Line { .. } => Handles::Ends,
            AnnotKind::Polygon { .. } | AnnotKind::Polyline { .. } | AnnotKind::Measure { .. } => {
                Handles::Vertices
            }
            AnnotKind::Callout { .. } => Handles::Callout,
            AnnotKind::FreeText { .. }
            | AnnotKind::Square { .. }
            | AnnotKind::Ellipse { .. }
            | AnnotKind::Ink { .. }
            | AnnotKind::Stamp { .. }
            | AnnotKind::PageStamp { .. }
            | AnnotKind::TextStamp { .. }
            | AnnotKind::Tape { .. } => Handles::Box,
            AnnotKind::Highlight { .. }
            | AnnotKind::Underline { .. }
            | AnnotKind::StrikeOut { .. }
            | AnnotKind::Squiggly { .. }
            | AnnotKind::Replace { .. }
            | AnnotKind::Insert { .. }
            | AnnotKind::Reply { .. }
            | AnnotKind::Status { .. }
            | AnnotKind::PageBorder { .. }
            | AnnotKind::FileAttachment { .. }
            | AnnotKind::Note { .. }
            | AnnotKind::Connector { .. } => Handles::None,
        }
    }

    /// Whether it has an inside to fill.
    pub fn fillable(&self) -> bool {
        matches!(
            self,
            AnnotKind::Square { .. } | AnnotKind::Ellipse { .. } | AnnotKind::Polygon { .. }
        )
    }

    /// Whether it can have a cloudy border.
    pub fn cloudable(&self) -> bool {
        self.fillable()
    }

    /// Whether it's drawn with a stroke (width and dashes apply).
    pub fn stroked(&self) -> bool {
        matches!(
            self,
            AnnotKind::Square { .. }
                | AnnotKind::Ellipse { .. }
                | AnnotKind::Line { .. }
                | AnnotKind::Ink { .. }
                | AnnotKind::Polygon { .. }
                | AnnotKind::Polyline { .. }
                | AnnotKind::Callout { .. }
                | AnnotKind::Measure { .. }
        )
    }

    /// The text size, for kinds whose text the user sizes.
    pub fn text_size(&self) -> Option<f32> {
        match self {
            AnnotKind::FreeText { size, .. }
            | AnnotKind::Tape { size, .. }
            | AnnotKind::Callout { size, .. } => Some(*size),
            _ => None,
        }
    }

    pub fn with_text_size(&self, to: f32) -> AnnotKind {
        let mut k = self.clone();
        if let AnnotKind::FreeText { size, .. }
        | AnnotKind::Tape { size, .. }
        | AnnotKind::Callout { size, .. } = &mut k
        {
            *size = to.clamp(4.0, 144.0);
        }
        k
    }
}

/// See [`AnnotKind::handles`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handles {
    None,
    Ends,
    Box,
    /// One on each vertex.
    Vertices,
    /// The text box's, the knee and the point.
    Callout,
}

/// Whether `p` is inside the polygon `pts` (even-odd).
fn inside(pts: &[Pt], p: Pt) -> bool {
    let mut odd = false;
    let n = pts.len();
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + n - 1) % n]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            odd = !odd;
        }
    }
    odd
}

/// Where a callout's leader meets its box: the box's edge nearest the
/// knee.
pub fn callout_end(rect: Box2, knee: Pt) -> Pt {
    let x = knee.x.clamp(rect.x, rect.right());
    let y = knee.y.clamp(rect.y, rect.bottom());
    if (x, y) != (knee.x, knee.y) {
        return Pt { x, y };
    }
    // The knee is inside: out to the nearest side.
    let sides = [
        (knee.x - rect.x, Pt { x: rect.x, y }),
        (rect.right() - knee.x, Pt { x: rect.right(), y }),
        (knee.y - rect.y, Pt { x, y: rect.y }),
        (
            rect.bottom() - knee.y,
            Pt {
                x,
                y: rect.bottom(),
            },
        ),
    ];
    sides
        .into_iter()
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, p)| p)
        .unwrap_or(knee)
}

/// A closed outline as points: a rectangle's corners, an ellipse in 32
/// steps, a polygon's vertices. For a cloudy border.
pub fn outline_of(kind: &AnnotKind) -> Option<Vec<Pt>> {
    match kind {
        AnnotKind::Square { rect } => Some(vec![
            Pt {
                x: rect.x,
                y: rect.y,
            },
            Pt {
                x: rect.right(),
                y: rect.y,
            },
            Pt {
                x: rect.right(),
                y: rect.bottom(),
            },
            Pt {
                x: rect.x,
                y: rect.bottom(),
            },
        ]),
        AnnotKind::Ellipse { rect } => {
            let (cx, cy, rx, ry) = (
                rect.x + rect.w / 2.0,
                rect.y + rect.h / 2.0,
                rect.w / 2.0,
                rect.h / 2.0,
            );
            Some(
                (0..32)
                    .map(|i| {
                        let t = i as f32 / 32.0 * std::f32::consts::TAU;
                        Pt {
                            x: cx + rx * t.cos(),
                            y: cy + ry * t.sin(),
                        }
                    })
                    .collect(),
            )
        }
        AnnotKind::Polygon { points } if points.len() >= 3 => Some(points.clone()),
        _ => None,
    }
}

/// A cloudy border around the closed outline `pts`: scallops about
/// `step` points long bulging outward, each a cubic (from, c1, c2, to), in
/// the same space as `pts`.
pub fn cloud_curves(pts: &[Pt], step: f32) -> Vec<[Pt; 4]> {
    let n = pts.len();
    if n < 3 {
        return Vec::new();
    }
    // Which side is out: the outline's turning, from its signed area.
    let area: f32 = (0..n)
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            a.x * b.y - b.x * a.y
        })
        .sum();
    let out = if area > 0.0 { 1.0 } else { -1.0 };
    let mut curves = Vec::new();
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        let len = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
        if len < 0.01 {
            continue;
        }
        let k = (len / step).round().max(1.0) as usize;
        for j in 0..k {
            let t0 = j as f32 / k as f32;
            let t1 = (j + 1) as f32 / k as f32;
            let p = Pt {
                x: a.x + (b.x - a.x) * t0,
                y: a.y + (b.y - a.y) * t0,
            };
            let q = Pt {
                x: a.x + (b.x - a.x) * t1,
                y: a.y + (b.y - a.y) * t1,
            };
            let seg = len / k as f32;
            let (nx, ny) = (out * (q.y - p.y) / seg, out * -(q.x - p.x) / seg);
            let h = seg * 0.55;
            curves.push([
                p,
                Pt {
                    x: p.x + nx * h,
                    y: p.y + ny * h,
                },
                Pt {
                    x: q.x + nx * h,
                    y: q.y + ny * h,
                },
                q,
            ]);
        }
    }
    curves
}

/// An attachment icon's strokes in the box `at`, in a frame that's y
/// down.
pub fn attachment_icon(icon: &str, at: Box2) -> Vec<Vec<Pt>> {
    let shape: Vec<Vec<(f32, f32)>> = match icon {
        "Paperclip" => vec![vec![
            (0.36, 0.72),
            (0.36, 0.24),
            (0.5, 0.1),
            (0.64, 0.24),
            (0.64, 0.82),
            (0.5, 0.94),
            (0.43, 0.84),
            (0.43, 0.32),
        ]],
        "Graph" => vec![
            vec![(0.14, 0.1), (0.14, 0.86), (0.9, 0.86)],
            vec![(0.32, 0.86), (0.32, 0.58)],
            vec![(0.52, 0.86), (0.52, 0.36)],
            vec![(0.72, 0.86), (0.72, 0.18)],
        ],
        "Tag" => vec![
            vec![
                (0.1, 0.3),
                (0.6, 0.3),
                (0.9, 0.5),
                (0.6, 0.7),
                (0.1, 0.7),
                (0.1, 0.3),
            ],
            vec![
                (0.62, 0.45),
                (0.7, 0.45),
                (0.7, 0.55),
                (0.62, 0.55),
                (0.62, 0.45),
            ],
        ],
        // PushPin, and anything this build doesn't know.
        _ => vec![
            vec![
                (0.3, 0.12),
                (0.7, 0.12),
                (0.6, 0.32),
                (0.68, 0.56),
                (0.32, 0.56),
                (0.4, 0.32),
                (0.3, 0.12),
            ],
            vec![(0.5, 0.56), (0.5, 0.94)],
        ],
    };
    shape
        .into_iter()
        .map(|path| {
            path.into_iter()
                .map(|(fx, fy)| Pt {
                    x: at.x + fx * at.w,
                    y: at.y + fy * at.h,
                })
                .collect()
        })
        .collect()
}

/// `/BE` for a cloudy border.
fn cloudy_style() -> Dictionary {
    dictionary! { "S" => Object::Name(b"C".to_vec()), "I" => 1 }
}

/// A mark's cloudy border, laid out upright and drawn in user space
/// through `up`: a path, not painted.
fn cloud_path(w: &mut ContentWriter<'_>, kind: &AnnotKind, width: f32, up: &Affine) {
    let Some(outline) = outline_of(kind) else {
        return;
    };
    for (i, [a, c1, c2, b]) in cloud_curves(&outline, cloud_step(width))
        .into_iter()
        .enumerate()
    {
        let [a, c1, c2, b] = [a, c1, c2, b].map(|p| up.apply(p));
        if i == 0 {
            w.move_to(a.x, a.y);
        }
        w.curve_to(c1, c2, b);
    }
    w.close();
}

/// How long a cloud's scallops are for a mark of stroke `width`.
pub fn cloud_step(width: f32) -> f32 {
    (9.0 + width * 3.0).min(30.0)
}

/// Distance from `p` to the segment `a`–`b`.
fn segment_distance(p: Pt, a: Pt, b: Pt) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (qx, qy) = (a.x + t * dx, a.y + t * dy);
    ((p.x - qx).powi(2) + (p.y - qy).powi(2)).sqrt()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Annot {
    pub id: AnnotId,
    pub kind: AnnotKind,
    /// RGB 0–255.
    pub color: [u8; 3],
    /// 0–1.
    pub opacity: f32,
    /// Stroke width in points, where it applies.
    pub stroke: f32,
    pub author: String,
    /// Note/FreeText text, or a comment on any other kind.
    pub contents: String,
    /// PDF date string, e.g. `D:20261011153000`: when it last changed.
    pub modified: String,
    /// When it was made (`/CreationDate`); moving or editing it doesn't
    /// change this. Empty for marks made before sheaf kept it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub created: String,
    /// Interior color (`/IC`), for kinds with an inside
    /// ([`AnnotKind::fillable`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<[u8; 3]>,
    /// A dashed stroke (`/BS /S /D`), for kinds with one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dashed: bool,
    /// A cloudy border (`/BE /S /C`): scallops along the outline of a
    /// rectangle, ellipse or polygon.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cloudy: bool,
    /// The rotation it reads upright in, absolute: its page's when it was
    /// made. Turning the page later turns the mark with what's printed
    /// there.
    #[serde(default, skip_serializing_if = "is_upright")]
    pub turn: Rotation,
    /// The `/NM` it came with when it was another tool's comment (made
    /// editable, or imported), kept so the replies to it still find it.
    /// Empty for one made in sheaf: see [`Annot::name`].
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
}

fn is_upright(turn: &Rotation) -> bool {
    *turn == Rotation::R0
}

impl Annot {
    /// The mark with its geometry mapped by `m` (see
    /// [`AnnotKind::mapped`]), its turn kept.
    pub fn mapped(&self, m: &Affine) -> Annot {
        let mut a = self.clone();
        a.kind = self.kind.mapped(m);
        a
    }

    /// The mark made in `frame`, a view of its page, with its turn relative
    /// to the frame (R0 reads as drawn), placed on the page: in PageSpace,
    /// with its turn on the page. The one way a mark made on a page as it
    /// shows reaches the model (M10.5 §5).
    pub fn to_page(&self, frame: &Geom) -> Annot {
        Annot {
            turn: self.turn.plus(frame.rotate),
            ..self.mapped(&frame.from_view())
        }
    }

    /// The mark as `frame`, a view of its page, shows it, its turn made
    /// relative to the frame: what a drag moves, a handle reshapes and the
    /// clipboard holds. [`to_page`](Self::to_page) takes it back.
    pub fn to_frame(&self, frame: &Geom) -> Annot {
        Annot {
            turn: self.turn.minus(frame.rotate),
            ..self.mapped(&frame.to_view())
        }
    }

    /// Its shape in the frame it reads upright in: PageSpace turned by its
    /// [`turn`](Self::turn), y down, not moved.
    /// [`AnnotKind::bounds`], [`hit`](Self::hit), [`outline_of`],
    /// [`text_line_paths`], [`caret_points`], [`measure_label_box`] and the
    /// tape and stamp layouts are written y down, for a shape in this frame
    /// or in a view's.
    pub fn upright(&self) -> AnnotKind {
        self.kind.mapped(&Affine::upright(self.turn))
    }

    /// Its bounds in PageSpace: laid out upright, then mapped back.
    pub fn page_bounds(&self) -> Box2 {
        Affine::upright(self.turn)
            .inverse()
            .apply_box(self.upright().bounds())
    }
    /// What it's called in threads and in the file (`/NM`): the name it
    /// came with, else `sheaf:<id>`.
    pub fn name(&self) -> String {
        if self.name.is_empty() {
            let [own, _] = self.id.sheaf_names();
            own
        } else {
            self.name.clone()
        }
    }

    /// Whether a click at `p`, in the frame the mark reads upright in
    /// ([`upright`](Self::upright)), lands on the mark: on the stroke for
    /// lines and drawings (within `tolerance` points of it), anywhere in
    /// the box for everything else.
    pub fn hit(&self, p: Pt, tolerance: f32) -> bool {
        let near = |a: Pt, b: Pt| segment_distance(p, a, b) <= tolerance + self.stroke / 2.0;
        let shape = self.upright();
        match &shape {
            AnnotKind::Reply { .. } | AnnotKind::Status { .. } => false,
            // On its edge, not anywhere on the page it goes round.
            AnnotKind::PageBorder { rect, .. } => {
                let [a, b, c, d] = [
                    Pt {
                        x: rect.x,
                        y: rect.y,
                    },
                    Pt {
                        x: rect.right(),
                        y: rect.y,
                    },
                    Pt {
                        x: rect.right(),
                        y: rect.bottom(),
                    },
                    Pt {
                        x: rect.x,
                        y: rect.bottom(),
                    },
                ];
                near(a, b) || near(b, c) || near(c, d) || near(d, a)
            }
            AnnotKind::Line { a, b, .. } => near(*a, *b),
            AnnotKind::Polyline { points } => points.windows(2).any(|w| near(w[0], w[1])),
            // On a line, or its label.
            AnnotKind::Measure {
                what,
                points,
                scale,
            } => {
                let n = points.len();
                let closed = *what == crate::measure::Measured::Area && n > 2;
                points.windows(2).any(|w| near(w[0], w[1]))
                    || (closed && near(points[n - 1], points[0]))
                    || measure_label_box(*what, points, scale).is_some_and(|b| b.contains(p))
            }
            // On an edge, or inside when filled.
            AnnotKind::Polygon { points } => {
                let n = points.len();
                (0..n).any(|i| near(points[i], points[(i + 1) % n]))
                    || (self.fill.is_some() && inside(points, p))
            }
            AnnotKind::Callout {
                rect, knee, point, ..
            } => rect.contains(p) || near(*point, *knee) || near(*knee, callout_end(*rect, *knee)),
            AnnotKind::Ink { paths } => paths.iter().any(|path| match path.as_slice() {
                [one] => near(*one, *one),
                pts => pts.windows(2).any(|w| near(w[0], w[1])),
            }),
            _ => {
                let b = shape.bounds();
                Box2 {
                    x: b.x - tolerance,
                    y: b.y - tolerance,
                    w: b.w + 2.0 * tolerance,
                    h: b.h + 2.0 * tolerance,
                }
                .contains(p)
            }
        }
    }

    /// The `/NM` of every annotation this mark writes: its own, then any
    /// companions' (see [`Written`]). sheaf draws these itself, so PDFium
    /// is told to hide exactly these.
    pub fn pdf_names(&self) -> Vec<String> {
        match self.kind {
            AnnotKind::Replace { .. } => vec![self.name(), self.strike_name()],
            _ => vec![self.name()],
        }
    }

    /// A tape's rows: its own, or for a tape from before rows were kept,
    /// read from its text. None for any other mark.
    pub fn tape(&self) -> Option<crate::tape::Tape> {
        match &self.kind {
            AnnotKind::Tape { tape, .. } if tape.lines.is_empty() => {
                Some(crate::tape::Tape::from_text(&self.contents))
            }
            AnnotKind::Tape { tape, .. } => Some(tape.clone()),
            _ => None,
        }
    }

    /// The comment this is a reply or status in the thread of, by `/NM`.
    pub fn thread_parent(&self) -> Option<&str> {
        match &self.kind {
            AnnotKind::Reply { parent } | AnnotKind::Status { parent, .. } => Some(parent),
            _ => None,
        }
    }

    /// The `/NM` of a replacement's StrikeOut.
    fn strike_name(&self) -> String {
        let [_, strike] = self.id.sheaf_names();
        strike
    }
}

/// Marks a companion annotation: one sheaf writes beside a mark's own,
/// pointing at it with `/IRT` (the Caret of a replace-text edit, say). It
/// belongs to that mark, and goes when the mark is rewritten.
pub const PART: &str = "SheafPart";

/// A mark as PDF objects: its annotation dictionary, with `/AP` set, and
/// any companions. Objects they point at (appearance streams, images,
/// embedded files) are already in the document. The writer gives each
/// companion `/IRT` to the main annotation, `/RT /Group` and [`PART`].
pub struct Written {
    pub main: Dictionary,
    pub companions: Vec<Dictionary>,
}

/// What the marks one save writes share: the fonts and ExtGStates their
/// appearances draw with, each one object a save, and the stamps' pages
/// and attachments' files, each written once, from the document's
/// [`MarkAssets`].
pub struct WriteCx {
    pub(crate) res: ResObjects,
    assets: Arc<MarkAssets>,
    /// The source whose objects the file holds as its own, under their
    /// numbers: the primary a save writes over.
    own: Option<BlobId>,
    /// Each stamp's page as a form in the file, by stem: None when it
    /// can't be found.
    stamps: HashMap<String, Option<StampForm>>,
    /// Each attachment's file in the file, by stem.
    files: HashMap<String, Option<lopdf::ObjectId>>,
    /// What's been copied from each source: its objects' numbers in the
    /// file.
    copied: HashMap<BlobId, HashMap<lopdf::ObjectId, lopdf::ObjectId>>,
}

/// A stamp's page as a form in the file: its number, `/BBox` and
/// `/Matrix`.
#[derive(Clone, Copy)]
struct StampForm {
    id: lopdf::ObjectId,
    bbox: Rect,
    matrix: Affine,
}

impl WriteCx {
    /// For a save of marks whose stamps and files are `assets`.
    pub fn new(assets: Arc<MarkAssets>) -> WriteCx {
        WriteCx {
            res: ResObjects::default(),
            assets,
            own: None,
            stamps: HashMap::new(),
            files: HashMap::new(),
            copied: HashMap::new(),
        }
    }

    /// For a file that holds `src`'s objects as its own, under their
    /// numbers, as a save holds its primary's: a stamp's page or a file
    /// that source holds is drawn and embedded where it is, not copied.
    pub fn owning(self, src: BlobId) -> WriteCx {
        WriteCx {
            own: Some(src),
            ..self
        }
    }

    /// Stamp `stem`'s page, the library's `name`, as a form in `pdf`:
    /// imported from its one-page PDF, or the one the appearance of a
    /// source drew, copied unless `pdf` holds it. None, said once, when it
    /// can't be found.
    fn stamp(&mut self, stem: &str, name: &str, pdf: &mut lopdf::Document) -> Option<StampForm> {
        if let Some(&form) = self.stamps.get(stem) {
            return form;
        }
        let form = match self.assets.stamp(stem) {
            Some(assets::Stamp::Pdf(blob)) => lopdf::Document::load_mem(blob.bytes())
                .map_err(anyhow::Error::from)
                .and_then(|src| crate::stamp_lib::page_form(&src, pdf))
                .map(|f| StampForm {
                    id: f.id,
                    bbox: f.bbox,
                    matrix: f.matrix,
                }),
            Some(assets::Stamp::Form { src, form }) => {
                let id = if self.own == Some(src.id()) {
                    Ok(Some(form))
                } else {
                    let copied = self.copied.entry(src.id()).or_default();
                    crate::save::copy_object(src.pdf(), form, pdf, copied)
                };
                id.and_then(|id| {
                    let id = id.context("a page, not a form")?;
                    let (bbox, matrix) =
                        crate::stamp_lib::form_frame(&*pdf, id).context("a form with no box")?;
                    Ok(StampForm { id, bbox, matrix })
                })
            }
            None => Err(anyhow::anyhow!(
                "it isn't in the document or on this machine"
            )),
        };
        let form = form
            .inspect_err(|e| log::warn!("stamp {name} ({stem}) is drawn without its page: {e:#}"))
            .ok();
        self.stamps.insert(stem.to_string(), form);
        form
    }

    /// Attachment `stem`'s file, `name`, as an embedded file stream in
    /// `pdf`: a source's as it stores it, copied unless `pdf` holds it, or
    /// made from its bytes. None, said once, when it can't be found.
    fn file(
        &mut self,
        stem: &str,
        name: &str,
        pdf: &mut lopdf::Document,
    ) -> Option<lopdf::ObjectId> {
        if let Some(&file) = self.files.get(stem) {
            return file;
        }
        let file = match self.assets.file(stem) {
            Some(assets::File::Source { src, stream }) if self.own == Some(src.id()) => {
                match pdf.get_object(stream) {
                    Ok(Object::Stream(_)) => Ok(stream),
                    _ => Err(anyhow::anyhow!("the file's embedded file isn't a stream")),
                }
            }
            Some(assets::File::Source { src, stream }) => {
                let copied = self.copied.entry(src.id()).or_default();
                copy_stream(src.pdf(), stream, pdf, copied)
            }
            Some(assets::File::Blob(blob)) => {
                let mut stream = Stream::new(
                    dictionary! {
                        "Type" => "EmbeddedFile",
                        "Params" => dictionary! { "Size" => blob.len() as i64 },
                    },
                    blob.bytes().to_vec(),
                );
                let _ = stream.compress();
                Ok(pdf.add_object(Object::Stream(stream)))
            }
            None => Err(anyhow::anyhow!(
                "it isn't in the document or on this machine"
            )),
        };
        let file = file
            .inspect_err(|e| {
                log::warn!("attachment {name} ({stem}) is written without its file: {e:#}")
            })
            .ok();
        self.files.insert(stem.to_string(), file);
        file
    }
}

/// Stream `id` of `src` in `pdf`, as `src` stores it, filters and all:
/// read without the source keeping it, since an attached file can be
/// large. What its dictionary refers to is copied too.
fn copy_stream(
    src: &lopdf::LazyPdf,
    id: lopdf::ObjectId,
    pdf: &mut lopdf::Document,
    copied: &mut HashMap<lopdf::ObjectId, lopdf::ObjectId>,
) -> anyhow::Result<lopdf::ObjectId> {
    use crate::content::ReadObjects;
    if let Some(&id) = copied.get(&id) {
        return Ok(id);
    }
    let stream = src
        .read(id, |o| o.as_stream().cloned())?
        .context("not a stream")?;
    let dict = crate::save::remap(src, &Object::Dictionary(stream.dict), pdf, copied)?;
    let mut copy = Stream::new(dict.as_dict()?.clone(), stream.content);
    copy.allows_compression = stream.allows_compression;
    let new = pdf.add_object(Object::Stream(copy));
    copied.insert(id, new);
    Ok(new)
}

/// Write `a` into `pdf`. Its geometry is PageSpace, which is the page's
/// user space, so it needs nothing of the page. The caller adds
/// [`Written::main`] (and the companions) to the page's `/Annots`. Its
/// stamp's page or file comes from the generated cache.
pub fn to_pdf(a: &Annot, pdf: &mut lopdf::Document) -> Written {
    to_pdf_in(
        a,
        pdf,
        &mut WriteCx::new(Arc::new(MarkAssets::new(crate::Store::new()))),
    )
}

/// [`to_pdf`], for one of the marks a save writes into `pdf` through
/// `cx`: what they draw with in common is in the file once, and their
/// stamps and files come from the document's.
pub fn to_pdf_in(a: &Annot, pdf: &mut lopdf::Document, cx: &mut WriteCx) -> Written {
    let stamp = match &a.kind {
        AnnotKind::PageStamp { stamp, name, .. } => cx.stamp(stamp, name, pdf),
        _ => None,
    };
    let (mut main, appearance) = annotation(a, stamp);
    let ap = appearance.write(pdf, &mut cx.res);
    main.set("AP", dictionary! { "N" => ap });
    if let AnnotKind::FileAttachment { file, name, .. } = &a.kind
        && let Some(ef) = cx.file(file, name, pdf)
    {
        main.set(
            "FS",
            dictionary! {
                "Type" => "Filespec",
                "F" => pdf_text_string(&name.chars().map(|c| if c.is_ascii() { c } else { '_' }).collect::<String>()),
                "UF" => pdf_text_string(name),
                "EF" => dictionary! { "F" => ef, "UF" => ef },
            },
        );
    }
    // The mark, and the digest of what its dictionary now says.
    on_grid(&mut main);
    let payload = Payload {
        v: Some(VERSION),
        std: staleness(&*pdf, &main, a.thread_parent().is_some()),
        mark: a,
    };
    main.set(
        "SheafAnnot",
        pdf_text_string(&serde_json::to_string(&payload).unwrap_or_default()),
    );
    let mut companions = Vec::new();
    // A replacement's crossed-out text: a StrikeOut of its own, grouped
    // under the Caret, which carries the mark.
    if let AnnotKind::Replace { rects, .. } = &a.kind {
        let strike = Annot {
            kind: AnnotKind::StrikeOut {
                rects: rects.clone(),
            },
            contents: String::new(),
            ..a.clone()
        };
        let (mut dict, appearance) = annotation(&strike, None);
        dict.set("NM", pdf_text_string(&a.strike_name()));
        let ap = appearance.write(pdf, &mut cx.res);
        dict.set("AP", dictionary! { "N" => ap });
        companions.push(dict);
    }
    Written { main, companions }
}

/// A caret's box at a point between characters `h` points tall: centred
/// on the point across, its lower half below the text.
pub fn caret_box(x: f32, bottom: f32, h: f32) -> Box2 {
    let size = (h * 0.6).max(4.0);
    Box2 {
        x: x - size / 2.0,
        y: bottom - size / 2.0,
        w: size,
        h: size,
    }
}

pub const NOTE_SIZE: f32 = 20.0;

/// A measurement's label size, in points.
pub const MEASURE_TEXT: f32 = 9.0;

/// Where a measurement's label is drawn: over a distance's middle, after a
/// path's end, on an area's centre.
pub fn measure_label_box(
    what: crate::measure::Measured,
    points: &[Pt],
    scale: &crate::measure::Scale,
) -> Option<Box2> {
    use crate::measure::Measured;
    let at = crate::measure::label_at(what, points)?;
    let text = crate::measure::label(what, points, scale);
    let w = Std14::Helvetica.width(&text, MEASURE_TEXT) + 4.0;
    let h = MEASURE_TEXT + 3.0;
    Some(match what {
        Measured::Distance => Box2 {
            x: at.x - w / 2.0,
            y: at.y - h - 2.0,
            w,
            h,
        },
        Measured::Perimeter => Box2 {
            x: at.x + 4.0,
            y: at.y - h / 2.0,
            w,
            h,
        },
        Measured::Area => Box2 {
            x: at.x - w / 2.0,
            y: at.y - h / 2.0,
            w,
            h,
        },
    })
}
/// Diameter of a connector marker, in points.
pub const CONNECTOR_SIZE: f32 = 22.0;
/// Tape layout: inner padding (pt), line height (em), operator column (em).
pub const TAPE_PAD: f32 = 4.0;
pub const TAPE_LINE: f32 = 1.25;
pub const TAPE_OP_W: f32 = 0.9;

/// Split tape contents into (amount, operator) per line. The operator is
/// whatever follows the last space; a line without one is an amount alone.
pub fn tape_lines(contents: &str) -> Vec<(String, String)> {
    contents
        .lines()
        .map(|l| {
            let l = l.trim_end();
            match l.rsplit_once(' ') {
                Some((v, op)) => (v.trim().to_string(), op.to_string()),
                None => (l.to_string(), String::new()),
            }
        })
        .collect()
}

impl Annot {
    /// The other end of a connector, if this is one and it's linked.
    pub fn partner(&self) -> Option<AnnotId> {
        match &self.kind {
            AnnotKind::Connector { partner, .. } => *partner,
            _ => None,
        }
    }

    /// Whether some of the text it draws can't be drawn in the built-in
    /// fonts: its appearance shows those characters as `?`, while its
    /// `/Contents` and its `/SheafAnnot` keep them (M10.5 proposal 22,
    /// until M10 slice 7's fallback font). Read from the text the writer
    /// draws, laid out, without drawing it.
    pub fn unencodable(&self) -> bool {
        runs(self, &self.upright()).iter().any(Run::unencodable)
    }
}

/// Current time as a PDF date string (`D:YYYYMMDDHHmmSS`, UTC).
pub fn pdf_date_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86400) as i64;
    let (h, m, sec) = ((secs % 86400) / 3600, (secs % 3600) / 60, secs % 60);
    // Civil-from-days (Howard Hinnant).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("D:{y:04}{mo:02}{d:02}{h:02}{m:02}{sec:02}Z")
}

/// A reasonable default author: the OS user name.
pub fn default_author() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "sheaf".into())
}

/// A mark's upright frame (see [`Annot::upright`]) to PageSpace, which
/// is the page's user space: where the writer draws what it lays out.
fn to_user(turn: Rotation) -> Affine {
    Affine::upright(turn).inverse()
}

/// The box `b` of the upright frame in user space, `[llx lly urx ury]`.
fn user_box(up: &Affine, b: Box2) -> [f32; 4] {
    let u = up.apply_box(b);
    [u.x, u.y, u.right(), u.bottom()]
}

fn pdf_rect(r: [f32; 4]) -> Object {
    Object::Array(r.iter().map(|v| Object::from(*v)).collect())
}

/// A path through `points`, not painted.
fn path(w: &mut ContentWriter<'_>, points: impl IntoIterator<Item = Pt>) {
    for (i, p) in points.into_iter().enumerate() {
        if i == 0 {
            w.move_to(p.x, p.y);
        } else {
            w.line_to(p.x, p.y);
        }
    }
}

/// The user-space box `[x0 y0 x1 y1]` as a path, not painted.
fn box_path(w: &mut ContentWriter<'_>, [x0, y0, x1, y1]: [f32; 4]) {
    w.rect(x0, y0, x1 - x0, y1 - y0);
}

/// An ellipse inscribed in the user-space box, as a path, not painted.
fn ellipse_path(w: &mut ContentWriter<'_>, [x0, y0, x1, y1]: [f32; 4]) {
    let (cx, cy, rx, ry) = (
        (x0 + x1) / 2.0,
        (y0 + y1) / 2.0,
        (x1 - x0) / 2.0,
        (y1 - y0) / 2.0,
    );
    let k = 0.5523;
    let at = |x: f32, y: f32| Pt { x, y };
    w.move_to(cx + rx, cy);
    w.curve_to(
        at(cx + rx, cy + ry * k),
        at(cx + rx * k, cy + ry),
        at(cx, cy + ry),
    );
    w.curve_to(
        at(cx - rx * k, cy + ry),
        at(cx - rx, cy + ry * k),
        at(cx - rx, cy),
    );
    w.curve_to(
        at(cx - rx, cy - ry * k),
        at(cx - rx * k, cy - ry),
        at(cx, cy - ry),
    );
    w.curve_to(
        at(cx + rx * k, cy - ry),
        at(cx + rx, cy - ry * k),
        at(cx + rx, cy),
    );
}

/// A text string: as is when PDFDocEncoding reads it back so
/// ([`crate::file::is_pdf_doc_ascii`]), else UTF-16BE with its BOM. The
/// `/SheafAnnot` JSON is one too: a DEL in a mark's text, written as is,
/// would read back as a NUL, which no JSON string holds.
fn pdf_text_string(s: &str) -> Object {
    if crate::file::is_pdf_doc_ascii(s) {
        Object::String(s.as_bytes().to_vec(), StringFormat::Literal)
    } else {
        let mut bytes = vec![0xFE, 0xFF];
        for u in s.encode_utf16() {
            bytes.extend_from_slice(&u.to_be_bytes());
        }
        Object::String(bytes, StringFormat::Hexadecimal)
    }
}

/// `/DA` for a mark that shows text, for viewers that lay it out again:
/// Helvetica at `size`, in `c`.
fn default_appearance(size: f32, c: [u8; 3]) -> Object {
    let [r, g, b] = c.map(|v| f32::from(v) / 255.0);
    pdf_text_string(&format!("/Helv {size} Tf {r:.3} {g:.3} {b:.3} rg"))
}

/// The dash pattern sheaf draws, in points: a mark's `/BS /D` and its
/// appearance's `d`.
pub const DASH: [f32; 2] = [4.0, 3.0];

/// `/BS` for a stroked mark: its width, and dashes if it has them.
fn border_style(a: &Annot) -> Dictionary {
    let mut bs = dictionary! { "W" => a.stroke };
    if a.dashed {
        bs.set("S", Object::Name(b"D".to_vec()));
        bs.set(
            "D",
            Object::Array(DASH.iter().map(|v| Object::from(*v)).collect()),
        );
    }
    bs
}

/// `/RD`: how far in from `/Rect` (`outer`) the box `inner` is, on each
/// side, so readers find the shape's own box.
fn inner_box(outer: [f32; 4], inner: [f32; 4]) -> Object {
    let [llx, lly, urx, ury] = outer;
    let [x0, y0, x1, y1] = inner;
    Object::Array(
        [x0 - llx, y0 - lly, urx - x1, ury - y1]
            .iter()
            .map(|v| Object::from(v.max(0.0)))
            .collect(),
    )
}

/// The stroke a mark's lines are drawn with: its color and width, and
/// dashes if it has them.
fn stroke_style(w: &mut ContentWriter<'_>, a: &Annot) {
    w.stroke_color(Color::rgb8(a.color));
    w.line_width(a.stroke);
    if a.dashed {
        w.dash(&DASH, 0.0);
    }
}

/// An open arrowhead at `tip`, pointing away from `from` (user space),
/// solid even on a dashed line.
fn arrowhead(w: &mut ContentWriter<'_>, from: Pt, tip: Pt, stroke: f32) {
    let (dx, dy) = (tip.x - from.x, tip.y - from.y);
    let len = (dx * dx + dy * dy).sqrt().max(0.001);
    let (ux, uy) = (dx / len, dy / len);
    let head = 6.0 + stroke * 2.0;
    let left = Pt {
        x: tip.x - head * (ux * 0.866 - uy * 0.5),
        y: tip.y - head * (uy * 0.866 + ux * 0.5),
    };
    let right = Pt {
        x: tip.x - head * (ux * 0.866 + uy * 0.5),
        y: tip.y - head * (uy * 0.866 - ux * 0.5),
    };
    w.dash(&[], 0.0);
    path(w, [left, tip, right]);
    w.paint(Paint::Stroke);
}

/// A piece of text a mark draws: `text` in `font` at `size`, from `at`,
/// its baseline's left end, in the frame the mark reads upright in (a
/// tape's in its own: see [`tape_runs`]).
struct Run {
    font: &'static Std14,
    size: f32,
    at: Pt,
    text: String,
}

impl Run {
    /// Whether its font draws some of it as `?`.
    fn unencodable(&self) -> bool {
        self.font.encode(&self.text).1.0 > 0
    }
}

/// The text `a` draws, `shown` its shape upright ([`Annot::upright`]),
/// laid out: what [`annotation`] draws and [`Annot::unencodable`] checks,
/// so the two can't disagree.
fn runs(a: &Annot, shown: &AnnotKind) -> Vec<Run> {
    let bold = &Std14::HelveticaBold;
    match shown {
        AnnotKind::FreeText { rect, size } | AnnotKind::Callout { rect, size, .. } => {
            let font = &Std14::Helvetica;
            font.wrap(&a.contents, *size, rect.w - 4.0)
                .into_iter()
                .enumerate()
                .map(|(i, text)| Run {
                    font,
                    size: *size,
                    at: Pt {
                        x: rect.x + 2.0,
                        y: rect.y + 2.0 + size * (1.0 + 1.2 * i as f32),
                    },
                    text,
                })
                .collect()
        }
        AnnotKind::Tape {
            rect, size, look, ..
        } => tape_runs(*rect, *size, look, &a.tape().unwrap_or_default()),
        AnnotKind::Measure {
            what,
            points,
            scale,
        } => measure_label_box(*what, points, scale)
            .map(|b| Run {
                font: &Std14::Helvetica,
                size: MEASURE_TEXT,
                at: Pt {
                    x: b.x + 2.0,
                    y: b.bottom() - 3.0,
                },
                text: crate::measure::label(*what, points, scale),
            })
            .into_iter()
            .collect(),
        // Centred on the marker.
        AnnotKind::Connector { rect, label, .. } => {
            let size = rect.h * 0.55;
            let width = bold.width(label, size);
            vec![Run {
                font: bold,
                size,
                at: Pt {
                    x: rect.x + (rect.w - width) / 2.0,
                    y: rect.y + rect.h / 2.0 + size * 0.36,
                },
                text: label.clone(),
            }]
        }
        // A dynamic stamp's text, over its page.
        AnnotKind::PageStamp { rect, text, .. } => text
            .iter()
            .filter(|t| !t.is_empty())
            .map(|t| {
                let size = (rect.h * 0.16).clamp(5.0, 24.0);
                let width = bold.width(t, size);
                Run {
                    font: bold,
                    size,
                    at: Pt {
                        x: rect.x + (rect.w - width).max(0.0) / 2.0,
                        y: rect.bottom() - rect.h * 0.08,
                    },
                    text: t.clone(),
                }
            })
            .collect(),
        AnnotKind::TextStamp { rect, text, .. } => text_stamp_lines(*rect, text)
            .into_iter()
            .map(|(line, size, baseline)| Run {
                font: bold,
                size,
                at: Pt {
                    x: rect.x + (rect.w - bold.width(&line, size)) / 2.0,
                    y: baseline,
                },
                text: line,
            })
            .collect(),
        // ZapfDingbats has the check and the cross (`tick_glyph`);
        // everything else is bold text.
        AnnotKind::Stamp { rect, glyph } => {
            let text: String = glyph.chars().map(tick_glyph).collect();
            let font = if text == *glyph {
                bold
            } else {
                &Std14::ZapfDingbats
            };
            vec![Run {
                font,
                size: rect.h * 0.8,
                at: Pt {
                    x: rect.x + rect.w * 0.1,
                    y: rect.y + rect.h * 0.85,
                },
                text,
            }]
        }
        _ => Vec::new(),
    }
}

/// What a tick's character draws as: a check or a cross is ZapfDingbats'
/// heavy one (✔ ✘), as ticks always have been, wherever a tick is drawn;
/// anything else is itself.
pub fn tick_glyph(c: char) -> char {
    match c {
        '✓' => '✔',
        '✗' => '✘',
        c => c,
    }
}

/// `runs` drawn in `color`, each from where the text matrix `at` makes
/// of its point puts it.
fn draw_runs(w: &mut ContentWriter<'_>, runs: &[Run], color: Color, at: impl Fn(Pt) -> Affine) {
    if runs.is_empty() {
        return;
    }
    w.begin_text();
    w.fill_color(color);
    let mut set = None;
    for run in runs {
        if set != Some((run.font, run.size)) {
            w.font(run.font, run.size);
            set = Some((run.font, run.size));
        }
        w.text_matrix(at(run.at));
        w.text(&run.text);
    }
    w.end_text();
}

/// A mark's appearance, drawn: its box in user space, its content, and
/// the resources that draws with.
struct Appearance {
    bbox: [f32; 4],
    content: Vec<u8>,
    needs: ResourceNeeds,
}

impl Appearance {
    /// It as a form in `pdf`, the resources written through `res`, once a
    /// save each.
    fn write(self, pdf: &mut lopdf::Document, res: &mut ResObjects) -> lopdf::ObjectId {
        let mut resources = Dictionary::new();
        res.bind(pdf, &self.needs, &mut resources);
        pdf.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Form",
                "BBox" => pdf_rect(self.bbox),
                "Resources" => resources,
            },
            self.content,
        ))
    }
}

/// The main annotation dictionary (without `/AP`, `/P` or `/SheafAnnot`)
/// and its appearance. A page stamp's draws `stamp`, its page as a form,
/// when it has one.
fn annotation(a: &Annot, stamp: Option<StampForm>) -> (Dictionary, Appearance) {
    // Laid out as it reads, then drawn in user space through `user`, so
    // it's upright whatever the page's /Rotate.
    let shown = a.upright();
    let user = to_user(a.turn);
    let up = |p: Pt| user.apply(p);
    // A replacement's Caret covers the caret; its StrikeOut is separate.
    let bounds = match &shown {
        AnnotKind::Replace { at, .. } => *at,
        _ => shown.bounds(),
    };
    // Pad so strokes aren't clipped by the BBox.
    let pad = a.stroke.max(1.0) + 1.0;
    let padded = Box2 {
        x: bounds.x - pad,
        y: bounds.y - pad,
        w: bounds.w + 2.0 * pad,
        h: bounds.h + 2.0 * pad,
    };
    let rect = user_box(&user, padded);
    let rect_user = rect;
    let [llx, lly, urx, ury] = rect;

    let mut w = ContentWriter::new();
    // A page stamp's page first, under its text and its opacity: fitted
    // into the mark as it reads, in the frame that reads upright, turned
    // y up, where a form's box is placed.
    if let (AnnotKind::PageStamp { rect, .. }, Some(form)) = (&shown, stamp) {
        let upright = Affine::upright(Rotation::R0).then(user);
        let turned = upright != Affine::IDENTITY;
        if turned {
            w.save();
            w.transform(upright);
        }
        let target = Box2 {
            y: -rect.bottom(),
            ..*rect
        };
        w.place_form(Res::written(form.id), form.bbox, form.matrix, target);
        if turned {
            w.restore();
        }
    }
    let alpha = a.opacity.clamp(0.0, 1.0);
    let blend = if matches!(a.kind, AnnotKind::Highlight { .. }) {
        Blend::Multiply
    } else {
        Blend::Normal
    };
    w.gstate(GState {
        fill_alpha: Some(alpha),
        stroke_alpha: Some(alpha),
        blend: Some(blend),
    });
    let mut dict = dictionary! {
        "Type" => "Annot",
        "Rect" => pdf_rect(rect),
        "NM" => pdf_text_string(&a.name()),
        "T" => pdf_text_string(&a.author),
        "M" => pdf_text_string(&a.modified),
        "F" => 4i64, // Print
        "CA" => alpha,
    };
    if !a.contents.is_empty() {
        dict.set("Contents", pdf_text_string(&a.contents));
    }
    if !a.created.is_empty() {
        dict.set("CreationDate", pdf_text_string(&a.created));
    }
    let c = a.color;
    let color = Color::rgb8(c);
    let color_arr = Object::Array(c.iter().map(|v| Object::from(*v as f32 / 255.0)).collect());
    let interior =
        |f: [u8; 3]| Object::Array(f.iter().map(|v| Object::from(*v as f32 / 255.0)).collect());

    match &shown {
        AnnotKind::Highlight { rects } => {
            dict.set("Subtype", Object::Name(b"Highlight".to_vec()));
            dict.set("C", color_arr);
            let mut quads = Vec::new();
            w.fill_color(color);
            for r in rects {
                let b = user_box(&user, *r);
                let [x0, y0, x1, y1] = b;
                // QuadPoints order: x1 y1 x2 y2 (upper), x3 y3 x4 y4 (lower).
                quads.extend([x0, y1, x1, y1, x0, y0, x1, y0].map(Object::from));
                box_path(&mut w, b);
                w.paint(Paint::Fill);
            }
            dict.set("QuadPoints", Object::Array(quads));
        }
        AnnotKind::Underline { rects }
        | AnnotKind::StrikeOut { rects }
        | AnnotKind::Squiggly { rects } => {
            let subtype: &[u8] = match a.kind {
                AnnotKind::Underline { .. } => b"Underline",
                AnnotKind::StrikeOut { .. } => b"StrikeOut",
                _ => b"Squiggly",
            };
            dict.set("Subtype", Object::Name(subtype.to_vec()));
            dict.set("C", color_arr);
            let mut quads = Vec::new();
            for r in rects {
                let [x0, y0, x1, y1] = user_box(&user, *r);
                quads.extend([x0, y1, x1, y1, x0, y0, x1, y0].map(Object::from));
            }
            dict.set("QuadPoints", Object::Array(quads));
            // Drawn as the text reads, then taken to user space, so the
            // line follows the text whatever the page's /Rotate.
            w.stroke_color(color);
            w.line_width(text_line_width(rects));
            for line in text_line_paths(&shown) {
                path(&mut w, line.into_iter().map(up));
                w.paint(Paint::Stroke);
            }
        }
        AnnotKind::Replace { at, .. } | AnnotKind::Insert { at } => {
            dict.set("Subtype", Object::Name(b"Caret".to_vec()));
            dict.set("RD", inner_box(rect_user, user_box(&user, *at)));
            dict.set("C", color_arr);
            dict.set("Sy", Object::Name(b"None".to_vec()));
            w.fill_color(color);
            path(&mut w, caret_points(*at).map(up));
            w.close();
            w.paint(Paint::Fill);
        }
        AnnotKind::Reply { .. } | AnnotKind::Status { .. } => {
            // The writer sets /IRT and /Rect from the parent. Nothing to
            // draw: a thread is read in a comments list.
            dict.set("Subtype", Object::Name(b"Text".to_vec()));
            dict.set("Name", Object::Name(b"Comment".to_vec()));
            dict.set("C", color_arr);
            dict.set("Open", false);
            // Print, NoZoom, NoRotate, as Acrobat writes them; a status is
            // hidden too.
            let hidden = matches!(a.kind, AnnotKind::Status { .. });
            dict.set("F", if hidden { 30i64 } else { 28i64 });
            if let AnnotKind::Status { model, state, .. } = &a.kind {
                dict.set("StateModel", pdf_text_string(model));
                dict.set("State", pdf_text_string(state));
            }
        }
        AnnotKind::FileAttachment {
            pos, icon, name, ..
        } => {
            dict.set("Subtype", Object::Name(b"FileAttachment".to_vec()));
            dict.set("Name", Object::Name(icon.clone().into_bytes()));
            dict.set("C", color_arr);
            if a.contents.is_empty() {
                dict.set("Contents", pdf_text_string(name));
            }
            w.stroke_color(color);
            w.line_width(1.5);
            w.line_cap(LineCap::Round);
            w.line_join(LineJoin::Round);
            for line in attachment_icon(icon, icon_box(*pos)) {
                path(&mut w, line.into_iter().map(up));
                w.paint(Paint::Stroke);
            }
        }
        AnnotKind::Note { pos } => {
            dict.set("Subtype", Object::Name(b"Text".to_vec()));
            dict.set("Name", Object::Name(b"Comment".to_vec()));
            dict.set("C", color_arr);
            dict.set("Open", false);
            let b = user_box(&user, icon_box(*pos));
            let [x0, y0, x1, y1] = b;
            w.fill_color(color);
            w.stroke_color(Color::Gray(0.0));
            w.line_width(1.0);
            box_path(&mut w, b);
            w.paint(Paint::FillStroke);
            // Three "text lines" inside the icon.
            w.line_width(0.5);
            w.stroke_color(Color::Gray(1.0));
            for k in 1..=3 {
                let y = y0 + (y1 - y0) * (k as f32 / 4.0);
                w.move_to(x0 + 4.0, y);
                w.line_to(x1 - 4.0, y);
                w.paint(Paint::Stroke);
            }
        }
        AnnotKind::FreeText { rect, size } => {
            dict.set("Subtype", Object::Name(b"FreeText".to_vec()));
            dict.set("RD", inner_box(rect_user, user_box(&user, *rect)));
            dict.set("DA", default_appearance(*size, c));
            dict.set("C", Object::Array(vec![]));
        }
        AnnotKind::Tape {
            rect, size, look, ..
        } => {
            dict.set("Subtype", Object::Name(b"FreeText".to_vec()));
            dict.set("DA", default_appearance(*size, c));
            dict.set("C", Object::Array(vec![Object::Real(1.0); 3]));
            let tape = a.tape().unwrap_or_default();
            tape_content(&mut w, *rect, *size, look, &tape, c, &user);
        }
        AnnotKind::Square { rect } | AnnotKind::Ellipse { rect } => {
            let square = matches!(a.kind, AnnotKind::Square { .. });
            let subtype: &[u8] = if square { b"Square" } else { b"Circle" };
            dict.set("Subtype", Object::Name(subtype.to_vec()));
            dict.set("C", color_arr);
            dict.set("BS", border_style(a));
            stroke_style(&mut w, a);
            if let Some(f) = a.fill {
                dict.set("IC", interior(f));
                w.fill_color(Color::rgb8(f));
            }
            let b = user_box(&user, *rect);
            dict.set("RD", inner_box(rect_user, b));
            if a.cloudy {
                dict.set("BE", cloudy_style());
                cloud_path(&mut w, &shown, a.stroke, &user);
            } else if square {
                box_path(&mut w, b);
            } else {
                ellipse_path(&mut w, b);
            }
            w.paint(if a.fill.is_some() {
                Paint::FillStroke
            } else {
                Paint::Stroke
            });
        }
        AnnotKind::Polygon { points } | AnnotKind::Polyline { points } => {
            let closed = matches!(a.kind, AnnotKind::Polygon { .. });
            let subtype: &[u8] = if closed { b"Polygon" } else { b"PolyLine" };
            dict.set("Subtype", Object::Name(subtype.to_vec()));
            dict.set("C", color_arr);
            dict.set("BS", border_style(a));
            dict.set(
                "Vertices",
                Object::Array(
                    points
                        .iter()
                        .flat_map(|p| {
                            let u = up(*p);
                            [Object::from(u.x), Object::from(u.y)]
                        })
                        .collect(),
                ),
            );
            stroke_style(&mut w, a);
            let filled = closed && a.fill.is_some();
            if let (true, Some(f)) = (closed, a.fill) {
                dict.set("IC", interior(f));
                w.fill_color(Color::rgb8(f));
            }
            if closed && a.cloudy {
                dict.set("BE", cloudy_style());
                cloud_path(&mut w, &shown, a.stroke, &user);
            } else {
                path(&mut w, points.iter().map(|p| up(*p)));
                if closed {
                    w.close();
                }
            }
            w.paint(if filled {
                Paint::FillStroke
            } else {
                Paint::Stroke
            });
        }
        AnnotKind::Measure {
            what,
            points,
            scale,
        } => {
            use crate::measure::Measured;
            let subtype: &[u8] = match what {
                Measured::Distance => b"Line",
                Measured::Perimeter => b"PolyLine",
                Measured::Area => b"Polygon",
            };
            dict.set("Subtype", Object::Name(subtype.to_vec()));
            dict.set("IT", Object::Name(what.intent().to_vec()));
            dict.set("C", color_arr);
            dict.set("BS", border_style(a));
            dict.set("Measure", crate::measure::measure_dict(scale));
            let label = crate::measure::label(*what, points, scale);
            // The value first, as Acrobat keeps it; then any note.
            dict.set(
                "Contents",
                pdf_text_string(&if a.contents.is_empty() {
                    label
                } else {
                    format!("{label}\n{}", a.contents)
                }),
            );
            let users: Vec<Pt> = points.iter().map(|p| up(*p)).collect();
            let coords = |pts: &[Pt]| {
                Object::Array(
                    pts.iter()
                        .flat_map(|u| [Object::from(u.x), Object::from(u.y)])
                        .collect(),
                )
            };
            stroke_style(&mut w, a);
            match (what, users.as_slice()) {
                (Measured::Distance, [ua, .., ub]) => {
                    dict.set("L", coords(&[*ua, *ub]));
                    let arrow = || Object::Name(b"OpenArrow".to_vec());
                    dict.set("LE", Object::Array(vec![arrow(), arrow()]));
                    path(&mut w, [*ua, *ub]);
                    w.paint(Paint::Stroke);
                    arrowhead(&mut w, *ua, *ub, a.stroke);
                    arrowhead(&mut w, *ub, *ua, a.stroke);
                }
                (_, pts) if !pts.is_empty() => {
                    dict.set("Vertices", coords(pts));
                    path(&mut w, pts.iter().copied());
                    if *what == Measured::Area {
                        w.close();
                    }
                    w.paint(Paint::Stroke);
                }
                _ => {}
            }
        }
        AnnotKind::Callout {
            rect,
            knee,
            point,
            size,
        } => {
            dict.set("Subtype", Object::Name(b"FreeText".to_vec()));
            dict.set("IT", Object::Name(b"FreeTextCallout".to_vec()));
            dict.set("DA", default_appearance(*size, c));
            dict.set("C", Object::Array(vec![]));
            dict.set("BS", border_style(a));
            let end = callout_end(*rect, *knee);
            let (up_point, up_knee, up_end) = (up(*point), up(*knee), up(end));
            dict.set(
                "CL",
                Object::Array(
                    [up_point, up_knee, up_end]
                        .iter()
                        .flat_map(|p| [Object::from(p.x), Object::from(p.y)])
                        .collect(),
                ),
            );
            dict.set("LE", Object::Name(b"OpenArrow".to_vec()));
            // The box inside /Rect, for viewers that lay the text out.
            let b = user_box(&user, *rect);
            let [bx0, by0, bx1, by1] = b;
            dict.set(
                "RD",
                Object::Array(
                    [bx0 - llx, by0 - lly, urx - bx1, ury - by1]
                        .iter()
                        .map(|v| Object::from(v.max(0.0)))
                        .collect(),
                ),
            );
            stroke_style(&mut w, a);
            w.fill_color(Color::Gray(1.0));
            box_path(&mut w, b);
            w.paint(Paint::FillStroke);
            path(&mut w, [up_end, up_knee, up_point]);
            w.paint(Paint::Stroke);
            arrowhead(&mut w, up_knee, up_point, a.stroke);
        }
        AnnotKind::Connector { rect, .. } => {
            // A Link so other viewers jump too; the writer fills in /Dest once
            // the partner's page object is known.
            dict.set("Subtype", Object::Name(b"Link".to_vec()));
            dict.set(
                "Border",
                Object::Array(vec![0i64.into(), 0i64.into(), 0i64.into()]),
            );
            dict.set("H", Object::Name(b"I".to_vec()));
            // A disc in the mark's color; its label goes on it in white.
            w.fill_color(color);
            ellipse_path(&mut w, user_box(&user, *rect));
            w.paint(Paint::Fill);
        }
        AnnotKind::Line {
            a: pa,
            b: pb,
            arrow,
            arrow_start,
        } => {
            dict.set("Subtype", Object::Name(b"Line".to_vec()));
            dict.set("C", color_arr);
            dict.set("BS", border_style(a));
            let (ua, ub) = (up(*pa), up(*pb));
            dict.set(
                "L",
                Object::Array(vec![ua.x.into(), ua.y.into(), ub.x.into(), ub.y.into()]),
            );
            if *arrow || *arrow_start {
                let end = |on: bool| {
                    Object::Name(if on {
                        b"OpenArrow".to_vec()
                    } else {
                        b"None".to_vec()
                    })
                };
                dict.set("LE", Object::Array(vec![end(*arrow_start), end(*arrow)]));
            }
            stroke_style(&mut w, a);
            path(&mut w, [ua, ub]);
            w.paint(Paint::Stroke);
            if *arrow {
                arrowhead(&mut w, ua, ub, a.stroke);
            }
            if *arrow_start {
                arrowhead(&mut w, ub, ua, a.stroke);
            }
        }
        AnnotKind::Ink { paths } => {
            dict.set("Subtype", Object::Name(b"Ink".to_vec()));
            dict.set("C", color_arr);
            dict.set("BS", border_style(a));
            let mut ink_list = Vec::new();
            stroke_style(&mut w, a);
            w.line_cap(LineCap::Round);
            w.line_join(LineJoin::Round);
            for line in paths {
                let users: Vec<Pt> = line.iter().map(|p| up(*p)).collect();
                ink_list.push(Object::Array(
                    users
                        .iter()
                        .flat_map(|u| [Object::from(u.x), Object::from(u.y)])
                        .collect(),
                ));
                path(&mut w, users);
                w.paint(Paint::Stroke);
            }
            dict.set("InkList", Object::Array(ink_list));
        }
        AnnotKind::PageStamp { name, .. } => {
            dict.set("Subtype", Object::Name(b"Stamp".to_vec()));
            if a.contents.is_empty() {
                dict.set("Contents", pdf_text_string(name));
            }
        }
        AnnotKind::PageBorder { rect, status } => {
            dict.set("Subtype", Object::Name(b"Square".to_vec()));
            dict.set("C", color_arr);
            dict.set("BS", border_style(a));
            if a.contents.is_empty() {
                dict.set("Contents", pdf_text_string(&format!("Status: {status}")));
            }
            stroke_style(&mut w, a);
            box_path(&mut w, user_box(&user, *rect));
            w.paint(Paint::Stroke);
        }
        AnnotKind::TextStamp { rect, text, .. } => {
            dict.set("Subtype", Object::Name(b"Stamp".to_vec()));
            if a.contents.is_empty() {
                dict.set("Contents", pdf_text_string(text));
            }
            // A rounded frame, inset by half its line.
            let lw = text_stamp_line(*rect);
            let r = Box2 {
                x: rect.x + lw / 2.0,
                y: rect.y + lw / 2.0,
                w: rect.w - lw,
                h: rect.h - lw,
            };
            let rad = (r.h * 0.18).min(r.w / 2.0);
            let k = 0.5523 * rad;
            let pt = |x: f32, y: f32| up(Pt { x, y });
            let (l, t, rr, b) = (r.x, r.y, r.right(), r.bottom());
            let segs = [
                (pt(l + rad, t), None),
                (pt(rr - rad, t), None),
                (
                    pt(rr, t + rad),
                    Some((pt(rr - rad + k, t), pt(rr, t + rad - k))),
                ),
                (pt(rr, b - rad), None),
                (
                    pt(rr - rad, b),
                    Some((pt(rr, b - rad + k), pt(rr - rad + k, b))),
                ),
                (pt(l + rad, b), None),
                (
                    pt(l, b - rad),
                    Some((pt(l + rad - k, b), pt(l, b - rad + k))),
                ),
                (pt(l, t + rad), None),
                (
                    pt(l + rad, t),
                    Some((pt(l, t + rad - k), pt(l + rad - k, t))),
                ),
            ];
            w.stroke_color(color);
            w.line_width(lw);
            for (i, (p, ctrl)) in segs.into_iter().enumerate() {
                match (i, ctrl) {
                    (0, _) => w.move_to(p.x, p.y),
                    (_, None) => w.line_to(p.x, p.y),
                    (_, Some((c1, c2))) => w.curve_to(c1, c2, p),
                }
            }
            w.close();
            w.paint(Paint::Stroke);
        }
        AnnotKind::Stamp { .. } => {
            dict.set("Subtype", Object::Name(b"Stamp".to_vec()));
            // Which tick it is is the mark's id, in /SheafAnnot.
            dict.set("Name", Object::Name(b"Tick".to_vec()));
        }
    }

    // Its text last, turned with the frame it reads upright in; a tape's
    // is drawn in its own frame, with the rest of it.
    if !matches!(shown, AnnotKind::Tape { .. }) {
        let ink = if matches!(shown, AnnotKind::Connector { .. }) {
            Color::Gray(1.0)
        } else {
            color
        };
        let theta = (a.turn.degrees() as f32).to_radians();
        let (cs, sn) = (theta.cos(), theta.sin());
        draw_runs(&mut w, &runs(a, &shown), ink, |p| {
            let o = up(p);
            Affine {
                a: cs,
                b: sn,
                c: -sn,
                d: cs,
                e: o.x,
                f: o.y,
            }
        });
    }
    let (content, needs) = w.finish(&Reserved::default(), &[]);
    (
        dict,
        Appearance {
            bbox: [llx, lly, urx, ury],
            content,
            needs,
        },
    )
}

/// How thick the line of an underline, strikeout or squiggly is: a
/// fraction of the text's height.
pub fn text_line_width(rects: &[Box2]) -> f32 {
    let h = rects.iter().map(|r| r.h).fold(0.0, f32::max);
    (h * 0.07).clamp(0.75, 3.0)
}

/// The lines an underline, strikeout or squiggly draws, for the mark in
/// a frame that's y down: one path per rect, under it, through its
/// middle, or a wave under it.
pub fn text_line_paths(kind: &AnnotKind) -> Vec<Vec<Pt>> {
    let (rects, at): (&[Box2], f32) = match kind {
        AnnotKind::Underline { rects } => (rects, 0.92),
        AnnotKind::StrikeOut { rects } => (rects, 0.55),
        AnnotKind::Squiggly { rects } => (rects, 0.9),
        _ => return Vec::new(),
    };
    let wavy = matches!(kind, AnnotKind::Squiggly { .. });
    rects
        .iter()
        .map(|r| {
            let y = r.y + r.h * at;
            if !wavy {
                return vec![Pt { x: r.x, y }, Pt { x: r.right(), y }];
            }
            // A zigzag a few points high, with a step of about a quarter of
            // the text's height.
            let amp = (r.h * 0.08).max(0.75);
            let step = (r.h * 0.25).max(2.0);
            let n = ((r.w / step).ceil() as usize).max(1);
            (0..=n)
                .map(|i| Pt {
                    x: (r.x + i as f32 * step).min(r.right()),
                    y: if i % 2 == 0 { y - amp } else { y + amp },
                })
                .collect()
        })
        .collect()
}

/// A caret's triangle in a frame that's y down: its tip at the top
/// centre.
pub fn caret_points(at: Box2) -> [Pt; 3] {
    [
        Pt {
            x: at.x,
            y: at.bottom(),
        },
        Pt {
            x: at.x + at.w / 2.0,
            y: at.y,
        },
        Pt {
            x: at.right(),
            y: at.bottom(),
        },
    ]
}

/// A text stamp's frame line, from its height.
pub fn text_stamp_line(rect: Box2) -> f32 {
    (rect.h * 0.05).clamp(1.0, 4.0)
}

/// A text stamp's lines as drawn: (text, size, baseline, y down). The
/// first is large, the rest small, centred as a block.
pub fn text_stamp_lines(rect: Box2, text: &str) -> Vec<(String, f32, f32)> {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let (big, small) = match lines.len() {
        0 | 1 => (rect.h * 0.5, 0.0),
        n => (rect.h * 0.38, rect.h * 0.42 / (n - 1) as f32 / 1.15),
    };
    let block = big * 1.0 + small * 1.15 * lines.len().saturating_sub(1) as f32;
    let mut y = rect.y + (rect.h - block) / 2.0;
    lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let size = if i == 0 { big } else { small };
            y += if i == 0 { size * 0.85 } else { size * 1.15 };
            (l.to_string(), size, y)
        })
        .collect()
}

/// A text stamp's size for `text` at `height`: wide enough for its lines.
pub fn text_stamp_size(text: &str, height: f32) -> (f32, f32) {
    let r = Box2 {
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: height,
    };
    let widest = text_stamp_lines(r, text)
        .iter()
        .map(|(l, size, _)| Std14::HelveticaBold.width(l, *size))
        .fold(0.0, f32::max);
    (widest + height * 0.6, height)
}

/// Helvetica, or Helvetica-Bold: what a tape's rows are drawn in.
fn helvetica(bold: bool) -> &'static Std14 {
    if bold {
        &Std14::HelveticaBold
    } else {
        &Std14::Helvetica
    }
}

/// A tape's frame as it reads (width, height): the mark's box, turned
/// back when the tape is turned.
fn tape_frame(rect: Box2, look: &crate::tape::TapeLook) -> (f32, f32) {
    if look.rotation % 180 == 90 {
        (rect.h, rect.w)
    } else {
        (rect.w, rect.h)
    }
}

/// Where a point of the tape's frame (x right, y down, as it reads) is
/// in the mark's upright frame, the tape turned as `look` says.
fn tape_to_shown(rect: Box2, look: &crate::tape::TapeLook, u: f32, v: f32) -> Pt {
    match look.rotation % 360 {
        90 => Pt {
            x: rect.right() - v,
            y: rect.y + u,
        },
        270 => Pt {
            x: rect.x + v,
            y: rect.bottom() - u,
        },
        180 => Pt {
            x: rect.right() - u,
            y: rect.bottom() - v,
        },
        _ => Pt {
            x: rect.x + u,
            y: rect.y + v,
        },
    }
}

/// The columns of a tape at `size`: (number, amount, symbol, note) widths.
pub fn tape_columns(
    tape: &crate::tape::Tape,
    size: f32,
    look: &crate::tape::TapeLook,
) -> (f32, f32, f32, f32) {
    use crate::tape::{Row, fmt_amount};
    let shown = look.shown(tape);
    let width = |s: &str, bold: bool| helvetica(bold).width(s, size);
    let number = if look.numbers {
        shown
            .iter()
            .map(|(n, _)| width(n, false))
            .fold(0.0, f32::max)
            + size * 0.8
    } else {
        0.0
    };
    let amount = shown
        .iter()
        .map(|(_, l)| match &l.row {
            Row::Title(t) => width(t, true) - size * 1.8,
            row => width(
                &fmt_amount(row.value().unwrap_or(0.0), look.decimals),
                row.is_total(),
            ),
        })
        .fold(size * 3.0, f32::max);
    let symbol = size * 1.8;
    let note = shown
        .iter()
        .map(|(_, l)| {
            if l.note.is_empty() {
                0.0
            } else {
                width(&l.note, false) + size
            }
        })
        .fold(0.0, f32::max);
    (number, amount, symbol, note)
}

/// A tape's natural size at `size` (as it reads, before it's turned).
pub fn tape_size(tape: &crate::tape::Tape, size: f32, look: &crate::tape::TapeLook) -> (f32, f32) {
    let (n, a, s, note) = tape_columns(tape, size, look);
    let rows = look.shown(tape).len().max(1) as f32;
    (
        n + a + s + note + 2.0 * TAPE_PAD,
        rows * size * TAPE_LINE + 2.0 * TAPE_PAD + size * 0.3,
    )
}

/// The baseline of a tape's `row`th shown row at `size`, in its frame
/// `height` tall, y up from its bottom.
fn tape_baseline(height: f32, size: f32, row: usize) -> f32 {
    height - (TAPE_PAD + size * (1.0 + TAPE_LINE * row as f32))
}

/// A tape's text, laid out in its own frame, y up from its bottom left:
/// each shown row's number, amount or title, symbol and note. A symbol
/// its row's font lacks (the minus, the subtotal's lozenge) is Symbol's.
fn tape_runs(
    rect: Box2,
    size: f32,
    look: &crate::tape::TapeLook,
    tape: &crate::tape::Tape,
) -> Vec<Run> {
    use crate::tape::{Row, fmt_amount};
    let (_, h) = tape_frame(rect, look);
    let (number_w, amount_w, symbol_w, _) = tape_columns(tape, size, look);
    let amount_right = TAPE_PAD + number_w + amount_w;
    let note_x = amount_right + symbol_w;
    let mut runs = Vec::new();
    let mut run = |font: &'static Std14, x: f32, y: f32, text: &str| {
        runs.push(Run {
            font,
            size,
            at: Pt { x, y },
            text: text.to_string(),
        });
    };
    for (i, (number, l)) in look.shown(tape).iter().enumerate() {
        let y = tape_baseline(h, size, i);
        if look.numbers {
            run(helvetica(false), TAPE_PAD, y, number);
        }
        match &l.row {
            Row::Title(t) => run(helvetica(true), TAPE_PAD + number_w, y, t),
            row => {
                let font = helvetica(row.is_total());
                let v = fmt_amount(row.value().unwrap_or(0.0), look.decimals);
                run(font, amount_right - font.width(&v, size), y, &v);
                let symbol = row.symbol();
                let font = if symbol.chars().all(|c| font.covers(c)) {
                    font
                } else {
                    &Std14::Symbol
                };
                run(font, amount_right + size * 0.3, y, symbol);
            }
        }
        if !l.note.is_empty() {
            run(helvetica(false), note_x + size * 0.4, y, &l.note);
        }
    }
    runs
}

/// A tape's appearance: paper, then its rows ([`tape_runs`]), drawn in
/// the tape's own frame and mapped onto the page through `up`, from
/// `rect`'s frame, so it reads the right way whichever way it's turned.
fn tape_content(
    w: &mut ContentWriter<'_>,
    rect: Box2,
    size: f32,
    look: &crate::tape::TapeLook,
    tape: &crate::tape::Tape,
    c: [u8; 3],
    up: &Affine,
) {
    let (width, h) = tape_frame(rect, look);
    // The frame, PDF-style (y up from its bottom left), to user space.
    let user = |x: f32, y: f32| up.apply(tape_to_shown(rect, look, x, h - y));
    let o = user(0.0, 0.0);
    let ex = user(1.0, 0.0);
    let ey = user(0.0, 1.0);
    w.save();
    w.transform(Affine {
        a: ex.x - o.x,
        b: ex.y - o.y,
        c: ey.x - o.x,
        d: ey.y - o.y,
        e: o.x,
        f: o.y,
    });
    // Paper, a thin grey border, and nothing drawn outside it.
    w.fill_color(Color::rgb8(look.paper));
    w.stroke_color(Color::Gray(0.6));
    w.line_width(0.5);
    w.rect(0.0, 0.0, width, h);
    w.paint(Paint::FillStroke);
    w.rect(0.0, 0.0, width, h);
    w.clip(FillRule::NonZero);
    w.paint(Paint::None);
    // Rules above totals first: paths can't go inside BT/ET.
    let (number_w, amount_w, symbol_w, _) = tape_columns(tape, size, look);
    w.stroke_color(Color::Gray(0.3));
    for (i, (_, l)) in look.shown(tape).iter().enumerate() {
        if l.row.is_total() {
            let y = tape_baseline(h, size, i) + size * 1.05;
            w.move_to(TAPE_PAD + number_w, y);
            w.line_to(TAPE_PAD + number_w + amount_w + symbol_w, y);
            w.paint(Paint::Stroke);
        }
    }
    draw_runs(w, &tape_runs(rect, size, look, tape), Color::rgb8(c), |p| {
        Affine {
            e: p.x,
            f: p.y,
            ..Affine::IDENTITY
        }
    });
    w.restore();
}

/// A tape alone, as it reads, as a one-page PDF its size: how sheaf shows
/// a turned tape on screen, from a picture of it.
pub fn tape_pdf(a: &Annot) -> Option<Vec<u8>> {
    let AnnotKind::Tape {
        rect, size, look, ..
    } = &a.upright()
    else {
        return None;
    };
    let tape = a.tape()?;
    let at = Box2 {
        x: 0.0,
        y: 0.0,
        w: rect.w,
        h: rect.h,
    };
    let page = Geom::read(Some(Rect::new(0.0, 0.0, rect.w, rect.h)), None, 0);
    let mut w = ContentWriter::new();
    tape_content(&mut w, at, *size, look, &tape, a.color, &page.from_view());
    let (content, needs) = w.finish(&Reserved::default(), &[]);
    let mut pdf = lopdf::Document::with_version("1.5");
    let mut resources = Dictionary::new();
    ResObjects::default().bind(&mut pdf, &needs, &mut resources);
    let pages_id = pdf.new_object_id();
    let content = pdf.add_object(Stream::new(dictionary! {}, content));
    let page = pdf.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => content,
        "MediaBox" => vec![0.into(), 0.into(), Object::from(rect.w), Object::from(rect.h)],
        "Resources" => resources,
    });
    pdf.objects.insert(
        pages_id,
        Object::Dictionary(
            dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 },
        ),
    );
    let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    pdf.trailer.set("Root", catalog);
    let mut bytes = Vec::new();
    pdf.save_to(&mut bytes).ok()?;
    Some(bytes)
}

/// The `/SheafAnnot` version this build writes.
pub const VERSION: u32 = 2;

/// A `/SheafAnnot` payload: the mark's JSON, and beside its fields the
/// version and the [`staleness`] digest of the dictionary it was written
/// into. Version 1 had neither, and its geometry was in the page's
/// displayed space. A field of [`Annot`] is never called `v` or `std`.
#[derive(Serialize, Deserialize)]
pub struct Payload<M> {
    /// [`VERSION`]; none in version 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub v: Option<u32>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub std: String,
    #[serde(flatten)]
    pub mark: M,
}

/// Parse a `/SheafAnnot` payload.
pub fn from_json(s: &[u8]) -> Option<Payload<Annot>> {
    serde_json::from_slice(s).ok()
}

/// How many of the steps [`staleness`] counts a key's numbers in make
/// one: 64 a point for geometry, 1000 for colors and `/CA`.
fn steps_per_unit(key: &[u8]) -> f64 {
    match key {
        b"C" | b"IC" | b"CA" => 1000.0,
        _ => 64.0,
    }
}

/// The step `v` is on, counted as the digest counts it: as an f32, which
/// is what the writer computes.
fn steps(v: f64, scale: f64) -> f64 {
    (f64::from(v as f32) * scale).round()
}

/// `v` on its step.
fn snapped(v: f64, scale: f64) -> f64 {
    steps(v, scale) / scale
}

/// Put the numbers [`staleness`] counts on its steps, before it counts
/// them. A value off them can sit by the middle of two, where another
/// tool writing it again with fewer digits moves it into the next and
/// stales the mark; one on them needs at most six decimals, and comes
/// back to its step from any rewrite to two or more (three for colors).
/// `/RD` is the inset to an inner box that's put on the steps itself, so
/// a box read as `/Rect` less `/RD` is off by half a step at most.
fn on_grid(dict: &mut Dictionary) {
    let numbers = |o: &Object| -> Option<Vec<f64>> {
        o.as_array()
            .ok()?
            .iter()
            .map(|v| v.as_float().ok())
            .collect()
    };
    let rect = dict.get(b"Rect").ok().and_then(numbers);
    let rd = dict.get(b"RD").ok().and_then(numbers);
    if let (Some([x0, y0, x1, y1]), Some([l, b, r, t])) = (
        rect.as_deref().and_then(|v| <[f64; 4]>::try_from(v).ok()),
        rd.as_deref().and_then(|v| <[f64; 4]>::try_from(v).ok()),
    ) {
        let s = |v| snapped(v, steps_per_unit(b"RD"));
        let (outer, inner) = (
            [s(x0), s(y0), s(x1), s(y1)],
            [s(x0 + l), s(y0 + b), s(x1 - r), s(y1 - t)],
        );
        let inset = [
            inner[0] - outer[0],
            inner[1] - outer[1],
            outer[2] - inner[2],
            outer[3] - inner[3],
        ];
        dict.set("RD", inset.map(Object::Real).to_vec());
    }
    fn walk(o: &mut Object, scale: f64) {
        match o {
            Object::Real(v) => *v = snapped(*v, scale),
            Object::Array(items) => items.iter_mut().for_each(|v| walk(v, scale)),
            _ => {}
        }
    }
    for key in [
        &b"Rect"[..],
        b"QuadPoints",
        b"InkList",
        b"L",
        b"Vertices",
        b"CL",
        b"C",
        b"IC",
        b"CA",
    ] {
        if let Ok(o) = dict.get_mut(key) {
            walk(o, steps_per_unit(key));
        }
    }
}

/// The staleness digest of a mark's dictionary (M10.5 §3.6): the keys the
/// writer sets from the mark, made canonical, so that a re-serialization
/// (`100` written `100.0`, reals written to three decimals or more, a
/// text string in another encoding, `/Rect`'s corners swapped) keeps it
/// and another tool's edit changes it. Geometry counts in steps of 1/64
/// pt, colors and `/CA` (1 when absent) in steps of 1/1000, as f32s, which
/// is what the writer computes, and the writer puts them on those steps
/// ([`on_grid`]); text strings decode as PDFium decodes them, then NFC. A
/// reply's or a status's `/Rect` (`threaded`) is its parent's, which the
/// writer sets later, and `/IRT`, `/RT` and `/AP` aren't the mark's: none
/// of them counts.
pub fn staleness<P: Objects + ?Sized>(pdf: &P, dict: &Dictionary, threaded: bool) -> String {
    use std::io::Write as _;
    const KEYS: [&[u8]; 15] = [
        b"Subtype",
        b"Rect",
        b"Contents",
        b"T",
        b"M",
        b"NM",
        b"C",
        b"IC",
        b"CA",
        b"QuadPoints",
        b"InkList",
        b"L",
        b"Vertices",
        b"CL",
        b"RD",
    ];
    let number = |o| resolve(pdf, o).and_then(|o| o.as_float().ok());
    // In steps of 1/`scale`.
    let numbers = |out: &mut Vec<u8>, items: &mut dyn Iterator<Item = Option<f64>>, scale: f64| {
        for v in items {
            match v {
                Some(v) => {
                    let _ = write!(out, "{} ", steps(v, scale) as i64);
                }
                None => out.extend_from_slice(b"? "),
            }
        }
    };
    let mut out = Vec::new();
    for key in KEYS {
        let scale = steps_per_unit(key);
        match (key, dict.get(key).ok().and_then(|o| resolve(pdf, o))) {
            (b"Rect", _) if threaded => {}
            (b"Subtype", Some(Object::Name(n))) => out.extend_from_slice(n),
            (b"Rect", Some(Object::Array(r))) => {
                let c: Vec<Option<f64>> = r.iter().map(number).collect();
                match c.as_slice() {
                    [Some(x0), Some(y0), Some(x1), Some(y1)] => {
                        let corners = [x0.min(*x1), y0.min(*y1), x0.max(*x1), y0.max(*y1)];
                        numbers(&mut out, &mut corners.into_iter().map(Some), scale);
                    }
                    _ => numbers(&mut out, &mut c.into_iter(), scale),
                }
            }
            (b"Contents" | b"T" | b"M" | b"NM", Some(Object::String(text, _))) => {
                let text: String = crate::file::text_string(text).nfc().collect();
                out.extend_from_slice(text.as_bytes());
            }
            (b"C" | b"IC", Some(Object::Array(c))) => {
                numbers(&mut out, &mut c.iter().map(number), scale);
            }
            (b"CA", ca) => {
                let ca = ca.map_or(Some(1.0), |o| o.as_float().ok());
                numbers(&mut out, &mut std::iter::once(ca), scale);
            }
            (b"InkList", Some(Object::Array(paths))) => {
                for path in paths {
                    if let Some(Object::Array(p)) = resolve(pdf, path) {
                        numbers(&mut out, &mut p.iter().map(number), scale);
                    }
                    out.push(b';');
                }
            }
            (_, Some(Object::Array(v))) => numbers(&mut out, &mut v.iter().map(number), scale),
            _ => {}
        }
        // Each key's value ends with a byte no text or number has.
        out.push(0xFF);
    }
    sheaf_core::checksum_hex(&out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A text stamp is as wide as its widest line in Helvetica-Bold, by
    /// Adobe's widths, with room either side.
    #[test]
    fn a_text_stamp_fits_its_widest_line() {
        let (w, h) = text_stamp_size("APPROVED\nby Greg", 40.0);
        // A P P R O V E D at 0.38 of the height; "by Greg" is narrower.
        let approved = 722.0 + 667.0 + 667.0 + 722.0 + 778.0 + 667.0 + 667.0 + 722.0;
        assert!(
            (w - (approved * 40.0 * 0.38 / 1000.0 + 24.0)).abs() < 1e-3,
            "{w}"
        );
        assert_eq!(h, 40.0);
    }

    /// The same marks give the same file, twice and on two threads: every
    /// kind a save writes, a turned tape among them, and each turned with
    /// its page too.
    #[test]
    fn identical_marks_write_identical_bytes() {
        fn write() -> Vec<u8> {
            let mut marks = crate::forms::fixture::every_mark_kind();
            let turned: Vec<Annot> = marks
                .iter()
                .map(|a| Annot {
                    id: AnnotId(a.id.0 + 100),
                    turn: Rotation::R90,
                    ..a.clone()
                })
                .collect();
            marks.extend(turned);
            let mut f = crate::forms::fixture::Form::new(1);
            let mut page = f.dict(f.pages[0]).clone();
            let mut cx = WriteCx::new(crate::forms::fixture::mark_assets());
            crate::save::write_page_annots(&mut f.pdf, &mut page, marks.iter(), None, &[], &mut cx);
            *f.dict(f.pages[0]) = page;
            f.bytes()
        }
        let first = write();
        assert_eq!(write(), first);
        let threads: Vec<_> = (0..2).map(|_| std::thread::spawn(write)).collect();
        for t in threads {
            assert_eq!(t.join().unwrap(), first);
        }
    }

    /// The fonts and ExtGStates marks draw with are one object a save,
    /// however many marks draw with them (M10.5 bug 19).
    #[test]
    fn marks_in_a_save_share_their_resources() {
        let marks = crate::forms::fixture::every_mark_kind();
        let mut pdf = lopdf::Document::with_version("1.7");
        let mut cx = WriteCx::new(crate::forms::fixture::mark_assets());
        let mut fonts = std::collections::BTreeSet::new();
        let mut states = std::collections::BTreeSet::new();
        for a in &marks {
            let main = to_pdf_in(a, &mut pdf, &mut cx).main;
            let ap = main.get(b"AP").unwrap().as_dict().unwrap();
            let ap = ap.get(b"N").unwrap().as_reference().unwrap();
            let form = pdf.get_object(ap).unwrap().as_stream().unwrap();
            let resources = form.dict.get(b"Resources").unwrap().as_dict().unwrap();
            for (key, into) in [(&b"Font"[..], &mut fonts), (b"ExtGState", &mut states)] {
                if let Ok(category) = resources.get(key) {
                    for (_, r) in category.as_dict().unwrap() {
                        into.insert(r.as_reference().unwrap());
                    }
                }
            }
        }
        // Helvetica, Helvetica-Bold and ZapfDingbats (the tick); every
        // mark at 0.8, all Normal but the highlight's Multiply.
        assert_eq!(fonts.len(), 3, "{fonts:?}");
        assert_eq!(states.len(), 2, "{states:?}");
    }

    /// A tape's symbols draw in the font that has them: its minus and
    /// subtotal lozenge, which WinAnsi lacks, in Symbol, so a tape draws
    /// no `?`.
    #[test]
    fn a_tape_draws_its_symbols_in_a_font_that_has_them() {
        let tape = crate::tape::Tape::from_text("5.00 +\n2.00 -\n3.00 ◊\n4.00 *");
        let a = mark(AnnotKind::Tape {
            rect: bx(10.0, 10.0, 120.0, 80.0),
            size: 10.0,
            tape: tape.clone(),
            look: Default::default(),
        });
        assert!(!a.unencodable());
        let runs = runs(&a, &a.upright());
        let symbol = |s: &str| runs.iter().find(|r| r.text == s).map(|r| r.font);
        assert_eq!(symbol("−"), Some(&Std14::Symbol));
        assert_eq!(symbol("◊"), Some(&Std14::Symbol));
        assert_eq!(symbol("+"), Some(&Std14::Helvetica));
    }

    #[test]
    fn marks_saved_before_creation_dates_still_read() {
        let old = br#"{"id":3,"kind":{"Note":{"pos":{"x":1.0,"y":2.0}}},"color":[1,2,3],"opacity":1.0,"stroke":1.0,"author":"me","contents":"hi","modified":"D:20261011000000"}"#;
        let a = from_json(old).unwrap().mark;
        assert_eq!(a.created, "");
        // And an empty one isn't written.
        assert!(!serde_json::to_string(&a).unwrap().contains("created"));
    }

    #[test]
    fn to_pdf_round_trips_through_json() {
        let a = Annot {
            id: AnnotId(7),
            kind: AnnotKind::FreeText {
                rect: Box2 {
                    x: 10.0,
                    y: 20.0,
                    w: 40.0,
                    h: 200.0,
                },
                size: 12.0,
            },
            color: [200, 30, 30],
            opacity: 1.0,
            stroke: 1.0,
            author: "greg".into(),
            contents: "Hello (world) é".into(),
            modified: "D:20261011000000".into(),
            created: "D:20261010000000".into(),
            fill: None,
            dashed: false,
            cloudy: false,
            turn: Rotation::R90,
            name: String::new(),
        };
        let mut pdf = lopdf::Document::with_version("1.7");
        let Written { main: dict, .. } = to_pdf(&a, &mut pdf);
        let ap = dict.get(b"AP").unwrap().as_dict().unwrap();
        let stream = pdf
            .get_object(ap.get(b"N").unwrap().as_reference().unwrap())
            .unwrap()
            .as_stream()
            .unwrap()
            .clone();
        assert_eq!(
            dict.get(b"CreationDate").unwrap().as_str().unwrap(),
            b"D:20261010000000"
        );
        assert_eq!(
            dict.get(b"Subtype").unwrap().as_name().unwrap(),
            b"FreeText"
        );
        let json = dict.get(b"SheafAnnot").unwrap().as_str().unwrap();
        // Non-ASCII contents force UTF-16 for the JSON string too; decode it.
        let json = if json.starts_with(&[0xFE, 0xFF]) {
            let units: Vec<u16> = json[2..]
                .chunks(2)
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect();
            String::from_utf16_lossy(&units).into_bytes()
        } else {
            json.to_vec()
        };
        let payload = from_json(&json).unwrap();
        assert_eq!(payload.mark, a);
        assert_eq!(payload.v, Some(VERSION));
        assert_eq!(payload.std, staleness(&pdf, &dict, false));
        // On one line, turned with the mark, in WinAnsi.
        let content = String::from_utf8_lossy(&stream.content);
        assert!(content.contains("0 1 -1 0 "), "{content}");
        let line = b"(Hello \\(world\\) \xE9) Tj";
        assert!(
            stream.content.windows(line.len()).any(|w| w == line),
            "{content}"
        );
    }

    fn mark(kind: AnnotKind) -> Annot {
        Annot {
            id: AnnotId(1),
            kind,
            color: [0, 0, 255],
            opacity: 1.0,
            stroke: 2.0,
            author: String::new(),
            contents: String::new(),
            modified: String::new(),
            created: String::new(),
            fill: None,
            dashed: false,
            cloudy: false,
            turn: Rotation::R0,
            name: String::new(),
        }
    }

    fn bx(x: f32, y: f32, w: f32, h: f32) -> Box2 {
        Box2 { x, y, w, h }
    }

    /// A mark turned with its page is laid out as it reads there: a
    /// note's icon box maps whole, and a text box's lines run up the
    /// page's user space.
    #[test]
    fn a_turned_mark_lays_out_as_it_reads() {
        let mut note = mark(AnnotKind::Note {
            pos: Pt { x: 10.0, y: 20.0 },
        });
        note.turn = Rotation::R90;
        let upright = note.upright();
        assert_eq!(
            upright,
            AnnotKind::Note {
                pos: Pt { x: 20.0, y: 10.0 }
            }
        );
        let back = upright.mapped(&Affine::upright(Rotation::R90).inverse());
        assert_eq!(back, note.kind);
        assert_eq!(note.page_bounds(), bx(10.0, 20.0, NOTE_SIZE, NOTE_SIZE));
        let mut text = mark(AnnotKind::FreeText {
            rect: bx(100.0, 100.0, 40.0, 200.0),
            size: 12.0,
        });
        text.contents = "Up".into();
        text.turn = Rotation::R90;
        assert_eq!(text.upright().bounds().w, 200.0, "it reads wide");
        let mut pdf = lopdf::Document::with_version("1.7");
        let main = to_pdf(&text, &mut pdf).main;
        let ap = main.get(b"AP").unwrap().as_dict().unwrap();
        let id = ap.get(b"N").unwrap().as_reference().unwrap();
        let content = &pdf.get_object(id).unwrap().as_stream().unwrap().content;
        let content = String::from_utf8_lossy(content);
        // Along user y, from its first line's start: (102, 114) upright.
        let tm = "0 1 -1 0 114 102 Tm";
        assert!(content.contains(tm), "{content}");
    }

    #[test]
    fn resizing_fits_the_shape_to_the_new_bounds() {
        let from = bx(10.0, 10.0, 100.0, 50.0);
        let to = bx(20.0, 30.0, 200.0, 25.0);
        let line = AnnotKind::Line {
            a: Pt { x: 10.0, y: 60.0 },
            b: Pt { x: 110.0, y: 10.0 },
            arrow: true,
            arrow_start: true,
        };
        match line.resized(from, to) {
            AnnotKind::Line {
                a,
                b,
                arrow,
                arrow_start,
            } => {
                assert_eq!((a.x, a.y), (20.0, 55.0));
                assert_eq!((b.x, b.y), (220.0, 30.0));
                assert!(arrow && arrow_start);
            }
            k => panic!("{k:?}"),
        }
        // A tape's text follows its height; a text box's doesn't.
        let tape = AnnotKind::Tape {
            rect: from,
            size: 10.0,
            tape: Default::default(),
            look: Default::default(),
        };
        assert_eq!(tape.resized(from, to).text_size(), Some(5.0));
        let text = AnnotKind::FreeText {
            rect: from,
            size: 10.0,
        };
        assert_eq!(text.resized(from, to).text_size(), Some(10.0));
        assert_eq!(text.with_text_size(1000.0).text_size(), Some(144.0));
    }

    /// A click is in the frame the mark reads upright in, y down: an
    /// upright mark's PageSpace with y negated, a turned one's turned too.
    #[test]
    fn clicks_hit_strokes_and_boxes() {
        let line = mark(AnnotKind::Line {
            a: Pt { x: 0.0, y: 0.0 },
            b: Pt { x: 100.0, y: 0.0 },
            arrow: false,
            arrow_start: false,
        });
        assert!(line.hit(Pt { x: 50.0, y: 3.0 }, 3.0));
        assert!(!line.hit(Pt { x: 50.0, y: 10.0 }, 3.0));
        assert!(!line.hit(Pt { x: 110.0, y: 0.0 }, 3.0));
        let mut square = mark(AnnotKind::Square {
            rect: bx(0.0, -100.0, 100.0, 100.0),
        });
        assert!(square.hit(Pt { x: 50.0, y: 50.0 }, 3.0));
        assert!(square.hit(Pt { x: -2.0, y: 50.0 }, 3.0));
        assert!(!square.hit(Pt { x: -5.0, y: 50.0 }, 3.0));
        square.turn = Rotation::R90;
        assert!(square.hit(Pt { x: -50.0, y: 50.0 }, 3.0));
        assert!(!square.hit(Pt { x: 50.0, y: 50.0 }, 3.0));
    }

    #[test]
    fn fills_dashes_and_arrowheads_reach_the_file() {
        let mut pdf = lopdf::Document::with_version("1.7");
        let mut square = mark(AnnotKind::Square {
            rect: bx(10.0, 10.0, 100.0, 50.0),
        });
        square.fill = Some([255, 0, 0]);
        square.dashed = true;
        let main = to_pdf(&square, &mut pdf).main;
        let ic = main.get(b"IC").unwrap().as_array().unwrap();
        assert_eq!(ic.len(), 3);
        assert_eq!(ic[0].as_float().unwrap(), 1.0);
        let bs = main.get(b"BS").unwrap().as_dict().unwrap();
        assert_eq!(bs.get(b"S").unwrap().as_name().unwrap(), b"D");
        assert_eq!(bs.get(b"D").unwrap().as_array().unwrap().len(), 2);

        // Unfilled and solid: neither key.
        square.fill = None;
        square.dashed = false;
        let main = to_pdf(&square, &mut pdf).main;
        assert!(main.get(b"IC").is_err());
        let bs = main.get(b"BS").unwrap().as_dict().unwrap();
        assert!(bs.get(b"S").is_err());

        let line = mark(AnnotKind::Line {
            a: Pt { x: 0.0, y: 0.0 },
            b: Pt { x: 100.0, y: 0.0 },
            arrow: false,
            arrow_start: true,
        });
        let main = to_pdf(&line, &mut pdf).main;
        let le: Vec<&[u8]> = main
            .get(b"LE")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o.as_name().unwrap())
            .collect();
        assert_eq!(le, [b"OpenArrow".as_slice(), b"None".as_slice()]);
    }

    #[test]
    fn text_lines_sit_under_through_and_under_wavily() {
        let r = bx(100.0, 100.0, 50.0, 20.0);
        let under = text_line_paths(&AnnotKind::Underline { rects: vec![r] });
        assert_eq!(under[0][0].y, under[0][1].y);
        assert!(under[0][0].y > 115.0);
        let strike = text_line_paths(&AnnotKind::StrikeOut { rects: vec![r] });
        assert!((strike[0][0].y - 111.0).abs() < 0.5);
        let wavy = text_line_paths(&AnnotKind::Squiggly { rects: vec![r] });
        assert!(wavy[0].len() > 5);
        assert!(wavy[0].iter().all(|p| p.x >= 100.0 && p.x <= 150.0));
        assert_eq!(wavy[0].last().unwrap().x, 150.0);
    }

    #[test]
    fn text_edits_read_back_from_json() {
        let mut a = mark(AnnotKind::Replace {
            rects: vec![bx(10.0, 10.0, 40.0, 12.0)],
            struck: "teh".into(),
            at: caret_box(50.0, 22.0, 12.0),
        });
        a.contents = "the".into();
        let json = serde_json::to_vec(&a).unwrap();
        assert_eq!(from_json(&json).unwrap().mark, a);
        assert_eq!(
            a.pdf_names(),
            vec!["sheaf:1".to_string(), "sheaf:1:strike".to_string()]
        );
        let ins = mark(AnnotKind::Insert {
            at: caret_box(50.0, 22.0, 12.0),
        });
        assert_eq!(
            from_json(&serde_json::to_vec(&ins).unwrap()).unwrap().mark,
            ins
        );
        assert_eq!(ins.kind.bounds().w, ins.kind.bounds().h);
    }

    /// A tick is a Stamp named `/Tick`: which one it is is its mark's id,
    /// which its `/SheafAnnot` holds.
    #[test]
    fn a_tick_is_named_tick() {
        let tick = mark(AnnotKind::Stamp {
            rect: bx(10.0, 10.0, 24.0, 24.0),
            glyph: "✓".into(),
        });
        let mut pdf = lopdf::Document::with_version("1.7");
        let main = to_pdf(&tick, &mut pdf).main;
        assert_eq!(main.get(b"Subtype").unwrap().as_name().unwrap(), b"Stamp");
        assert_eq!(main.get(b"Name").unwrap().as_name().unwrap(), b"Tick");
        let json = main.get(b"SheafAnnot").unwrap().as_str().unwrap();
        let json = crate::file::text_string(json);
        assert_eq!(from_json(json.as_bytes()).unwrap().mark.id, tick.id);
    }

    #[test]
    fn connector_labels_run_numbers_then_letters() {
        let labels: Vec<String> = connector_labels().collect();
        assert_eq!(labels.len(), 50 + 26 + 26 * 26);
        assert_eq!((labels[0].as_str(), labels[49].as_str()), ("1", "50"));
        assert_eq!((labels[50].as_str(), labels[75].as_str()), ("A", "Z"));
        assert_eq!(
            (labels[76].as_str(), labels.last().unwrap().as_str()),
            ("AA", "ZZ")
        );
        assert_eq!(
            group_of(&AnnotKind::Stamp {
                rect: bx(0.0, 0.0, 1.0, 1.0),
                glyph: "✓".into()
            }),
            Some(Group::Tickmark("✓".into()))
        );
    }

    #[test]
    fn clouds_bulge_outward_whichever_way_round() {
        let square = outline_of(&AnnotKind::Square {
            rect: bx(100.0, 100.0, 100.0, 100.0),
        })
        .unwrap();
        for pts in [square.clone(), square.iter().rev().copied().collect()] {
            let curves = cloud_curves(&pts, 20.0);
            assert_eq!(curves.len(), 20);
            // Every control point is outside the square.
            for [_, c1, c2, _] in curves {
                for c in [c1, c2] {
                    let inside = c.x > 100.0 && c.x < 200.0 && c.y > 100.0 && c.y < 200.0;
                    assert!(!inside, "{c:?}");
                }
            }
        }
    }

    #[test]
    fn polygons_and_callouts_write_their_keys() {
        let mut pdf = lopdf::Document::with_version("1.7");
        let mut poly = mark(AnnotKind::Polygon {
            points: vec![
                Pt { x: 10.0, y: -10.0 },
                Pt { x: 110.0, y: -10.0 },
                Pt { x: 60.0, y: -90.0 },
            ],
        });
        poly.cloudy = true;
        poly.fill = Some([255, 255, 0]);
        let main = to_pdf(&poly, &mut pdf).main;
        assert_eq!(main.get(b"Subtype").unwrap().as_name().unwrap(), b"Polygon");
        assert_eq!(main.get(b"Vertices").unwrap().as_array().unwrap().len(), 6);
        let be = main.get(b"BE").unwrap().as_dict().unwrap();
        assert_eq!(be.get(b"S").unwrap().as_name().unwrap(), b"C");
        assert!(
            poly.hit(Pt { x: 60.0, y: 40.0 }, 2.0),
            "filled: inside counts"
        );

        let callout = mark(AnnotKind::Callout {
            rect: bx(200.0, 100.0, 120.0, 40.0),
            knee: Pt { x: 180.0, y: 160.0 },
            point: Pt { x: 150.0, y: 200.0 },
            size: 12.0,
        });
        let main = to_pdf(&callout, &mut pdf).main;
        assert_eq!(
            main.get(b"IT").unwrap().as_name().unwrap(),
            b"FreeTextCallout"
        );
        let cl = main.get(b"CL").unwrap().as_array().unwrap();
        assert_eq!(cl.len(), 6);
        // The leader starts at the point.
        assert_eq!(cl[1].as_float().unwrap(), 200.0);
        // It meets the box at its left side, level with the knee.
        assert_eq!(
            callout_end(bx(200.0, 100.0, 120.0, 40.0), Pt { x: 180.0, y: 120.0 }),
            Pt { x: 200.0, y: 120.0 }
        );
        assert!(
            callout.hit(
                Pt {
                    x: 165.0,
                    y: -180.0
                },
                3.0
            ),
            "on the leader"
        );
    }
}
