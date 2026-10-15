//! Markup annotations: the model, their PDF serialization (dictionary +
//! appearance stream), and the coordinate mapping between the page's
//! displayed space and PDF user space.
//!
//! Coordinates in the model are in the source page's *displayed* space:
//! points, top-left origin, after the page's own `/Rotate` — the same space
//! search hits use. They're converted to user space (bottom-left origin,
//! before `/Rotate`) only when written to a file.
//!
//! Every annotation we write carries `/NM sheaf:<id>` and a `/SheafAnnot`
//! JSON copy of the model, so a file we saved can be reopened with the marks
//! editable and PDFium told to hide its rendering of them.
//!
//! The JSON is a file format. Changes to it are additive: a new kind is a
//! new variant and a new field has `#[serde(default)]`, so older files keep
//! reading. A mark this build can't read (a kind from a newer sheaf) isn't
//! ours to rewrite: it's kept and shown like another tool's (see
//! `file::is_ours`). Every mark also writes a full appearance, so other
//! viewers, and older sheaf builds, draw it.

use lopdf::{Dictionary, Object, Stream, StringFormat, dictionary};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AnnotId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

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
    /// Sticky note: an icon at `pos` (top-left), text in `contents`.
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
    /// its one-page PDF in the generated-file cache (`generated::path_of`),
    /// `name` is what the library calls it, and `text` (a dynamic stamp's,
    /// filled in when placed) is drawn over it.
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
}

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
    let v: Vec<f32> = arr.iter().map(|o| o.as_float().unwrap_or(0.0)).collect();
    Some([
        v[0].min(v[2]),
        v[1].min(v[3]),
        v[0].max(v[2]),
        v[1].max(v[3]),
    ])
}

/// What a viewer shows: `crop` clipped to `media` (PDF 32000 §14.11.2),
/// or `media` when there's no crop or it doesn't overlap.
pub fn visible_box(media: [f32; 4], crop: Option<[f32; 4]>) -> [f32; 4] {
    match crop {
        Some(c) => {
            let b = [
                c[0].max(media[0]),
                c[1].max(media[1]),
                c[2].min(media[2]),
                c[3].min(media[3]),
            ];
            if b[2] - b[0] > 0.5 && b[3] - b[1] > 0.5 {
                b
            } else {
                media
            }
        }
        None => media,
    }
}

impl AnnotKind {
    /// The same shape with every point mapped by `mp` and every box by
    /// `mb`; `size` maps the text size of tapes.
    fn mapped(
        &self,
        mp: &dyn Fn(&Pt) -> Pt,
        mb: &dyn Fn(&Box2) -> Box2,
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
            AnnotKind::Note { pos } => AnnotKind::Note { pos: mp(pos) },
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

    /// The same shape moved by (`dx`, `dy`) points.
    pub fn translated(&self, dx: f32, dy: f32) -> AnnotKind {
        self.mapped(
            &|p| Pt {
                x: p.x + dx,
                y: p.y + dy,
            },
            &|b| Box2 {
                x: b.x + dx,
                y: b.y + dy,
                w: b.w,
                h: b.h,
            },
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
        self.mapped(
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
            &|s| (s * along).max(4.0),
        )
    }

    /// How the mark can be reshaped by dragging: its two ends, a box, or
    /// not at all (sticky notes and connectors keep their size; a
    /// highlight follows its text).
    pub fn handles(&self) -> Handles {
        match self {
            AnnotKind::Line { .. } => Handles::Ends,
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
            | AnnotKind::Note { .. }
            | AnnotKind::Connector { .. } => Handles::None,
        }
    }

    /// Whether it has an inside to fill.
    pub fn fillable(&self) -> bool {
        matches!(self, AnnotKind::Square { .. } | AnnotKind::Ellipse { .. })
    }

    /// Whether it's drawn with a stroke (width and dashes apply).
    pub fn stroked(&self) -> bool {
        matches!(
            self,
            AnnotKind::Square { .. }
                | AnnotKind::Ellipse { .. }
                | AnnotKind::Line { .. }
                | AnnotKind::Ink { .. }
        )
    }

    /// The text size, for kinds whose text the user sizes.
    pub fn text_size(&self) -> Option<f32> {
        match self {
            AnnotKind::FreeText { size, .. } | AnnotKind::Tape { size, .. } => Some(*size),
            _ => None,
        }
    }

    pub fn with_text_size(&self, to: f32) -> AnnotKind {
        let mut k = self.clone();
        if let AnnotKind::FreeText { size, .. } | AnnotKind::Tape { size, .. } = &mut k {
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
}

impl Annot {
    /// Bounding box in displayed page space.
    pub fn bounds(&self) -> Box2 {
        let union = |rects: &[Box2]| {
            rects.iter().copied().reduce(Box2::union).unwrap_or(Box2 {
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 0.0,
            })
        };
        match &self.kind {
            AnnotKind::Highlight { rects }
            | AnnotKind::Underline { rects }
            | AnnotKind::StrikeOut { rects }
            | AnnotKind::Squiggly { rects } => union(rects),
            AnnotKind::Replace { rects, at, .. } => union(rects).union(*at),
            AnnotKind::Insert { at } => *at,
            // Not on the page: they're in their parent's thread.
            AnnotKind::Reply { .. } | AnnotKind::Status { .. } => union(&[]),
            AnnotKind::Note { pos } => Box2 {
                x: pos.x,
                y: pos.y,
                w: NOTE_SIZE,
                h: NOTE_SIZE,
            },
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

    pub fn name(&self) -> String {
        format!("sheaf:{}", self.id.0)
    }

    /// Whether a click at `p` lands on the mark: on the stroke for lines
    /// and drawings (within `tolerance` points of it), anywhere in the box
    /// for everything else.
    pub fn hit(&self, p: Pt, tolerance: f32) -> bool {
        let near = |a: Pt, b: Pt| segment_distance(p, a, b) <= tolerance + self.stroke / 2.0;
        match &self.kind {
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
            AnnotKind::Ink { paths } => paths.iter().any(|path| match path.as_slice() {
                [one] => near(*one, *one),
                pts => pts.windows(2).any(|w| near(w[0], w[1])),
            }),
            _ => {
                let b = self.bounds();
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
        format!("sheaf:{}:strike", self.id.0)
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

/// Write `a` into `pdf` as it appears on a page with geometry `geom`. The
/// caller adds [`Written::main`] (and the companions) to the page's
/// `/Annots`.
pub fn to_pdf(a: &Annot, geom: &PageGeom, pdf: &mut lopdf::Document) -> Written {
    to_pdf_in(a, geom, pdf, &mut std::collections::HashMap::new())
}

/// [`to_pdf`], with the stamp pages already imported into `pdf` in this
/// save (by `stamp`), so a stamp used on every page is in the file once.
pub fn to_pdf_in(
    a: &Annot,
    geom: &PageGeom,
    pdf: &mut lopdf::Document,
    forms: &mut std::collections::HashMap<String, Option<(lopdf::ObjectId, sheaf_core::PageSize)>>,
) -> Written {
    let (mut main, mut appearance) = annotation(a, geom);
    if let AnnotKind::PageStamp {
        rect, stamp, name, ..
    } = &a.kind
    {
        let form = *forms.entry(stamp.clone()).or_insert_with(|| {
            let path = crate::generated::path_of(stamp);
            let loaded = std::fs::read(&path)
                .ok()
                .and_then(|b| lopdf::Document::load_mem(&b).ok());
            match loaded {
                Some(src) => crate::stamp_lib::page_form(&src, pdf).ok(),
                None => {
                    log::warn!("stamp {name} ({}) isn't on this machine", path.display());
                    None
                }
            }
        });
        match form {
            Some((id, size)) => {
                // The page fitted into the mark, upright as shown.
                let up = |p: Pt| geom.to_user(p);
                let o = up(Pt {
                    x: rect.x,
                    y: rect.bottom(),
                });
                let u = up(Pt {
                    x: rect.x + rect.w / size.width.max(0.001),
                    y: rect.bottom(),
                });
                let v = up(Pt {
                    x: rect.x,
                    y: rect.bottom() - rect.h / size.height.max(0.001),
                });
                let (ex, ey) = ((u.x - o.x, u.y - o.y), (v.x - o.x, v.y - o.y));
                let mut content = format!(
                    "q {:.5} {:.5} {:.5} {:.5} {:.3} {:.3} cm /SheafStamp Do Q\n",
                    ex.0, ex.1, ey.0, ey.1, o.x, o.y
                )
                .into_bytes();
                content.extend_from_slice(&appearance.content);
                appearance.set_content(content);
                if let Ok(Object::Dictionary(r)) = appearance.dict.get_mut(b"Resources") {
                    r.set("XObject", dictionary! { "SheafStamp" => id });
                }
            }
            None => {
                // Without its page, a box with its name, so it's not lost.
                let mut content = appearance.content.clone();
                let [x0, y0, x1, y1] = geom.box_to_user(*rect);
                content.extend_from_slice(
                    format!(
                        " 0.5 g 1 w {x0:.2} {y0:.2} {:.2} {:.2} re S",
                        x1 - x0,
                        y1 - y0
                    )
                    .as_bytes(),
                );
                appearance.set_content(content);
            }
        }
    }
    let ap = pdf.add_object(Object::Stream(appearance));
    main.set("AP", dictionary! { "N" => ap });
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
        let (mut dict, appearance) = annotation(&strike, geom);
        dict.remove(b"SheafAnnot");
        dict.set("NM", pdf_text_string(&a.strike_name()));
        let ap = pdf.add_object(Object::Stream(appearance));
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

/// Advance of `s` in Helvetica at `size`, from the AFM widths of the
/// characters a tape uses (digits are all 556). Good enough to right-align.
pub fn helv_width(s: &str, size: f32) -> f32 {
    s.chars()
        .map(|c| match c {
            '.' | ',' | ' ' => 278.0,
            '-' | '+' | '×' | '÷' => 584.0,
            '(' | ')' => 333.0,
            'T' => 611.0,
            _ => 556.0,
        })
        .sum::<f32>()
        * size
        / 1000.0
}

impl Annot {
    /// The other end of a connector, if this is one and it's linked.
    pub fn partner(&self) -> Option<AnnotId> {
        match &self.kind {
            AnnotKind::Connector { partner, .. } => *partner,
            _ => None,
        }
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

/// Geometry of a page needed to map between displayed and user space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageGeom {
    /// The visible box, x0, y0, x1, y1 in user space: the CropBox clipped
    /// to the MediaBox, or the MediaBox when there's no CropBox. It's what
    /// viewers show, so "displayed" positions start at its corner.
    pub media: [f32; 4],
    /// `/Rotate`, normalized to 0/90/180/270.
    pub rotate: i32,
}

impl PageGeom {
    /// Displayed point → user space.
    pub fn to_user(&self, p: Pt) -> Pt {
        let [x0, y0, x1, y1] = self.media;
        match self.rotate.rem_euclid(360) {
            90 => Pt {
                x: x0 + p.y,
                y: y0 + p.x,
            },
            180 => Pt {
                x: x1 - p.x,
                y: y0 + p.y,
            },
            270 => Pt {
                x: x1 - p.y,
                y: y1 - p.x,
            },
            _ => Pt {
                x: x0 + p.x,
                y: y1 - p.y,
            },
        }
    }

    /// User space → displayed point.
    pub fn to_display(&self, u: Pt) -> Pt {
        let [x0, y0, x1, y1] = self.media;
        match self.rotate.rem_euclid(360) {
            90 => Pt {
                x: u.y - y0,
                y: u.x - x0,
            },
            180 => Pt {
                x: x1 - u.x,
                y: u.y - y0,
            },
            270 => Pt {
                x: y1 - u.y,
                y: x1 - u.x,
            },
            _ => Pt {
                x: u.x - x0,
                y: y1 - u.y,
            },
        }
    }

    /// Displayed box → user-space [llx lly urx ury].
    pub fn box_to_user(&self, b: Box2) -> [f32; 4] {
        let p = self.to_user(Pt { x: b.x, y: b.y });
        let q = self.to_user(Pt {
            x: b.right(),
            y: b.bottom(),
        });
        [p.x.min(q.x), p.y.min(q.y), p.x.max(q.x), p.y.max(q.y)]
    }
}

fn pdf_rect(r: [f32; 4]) -> Object {
    Object::Array(r.iter().map(|v| Object::Real(*v)).collect())
}

/// Path operators for an ellipse inscribed in the user-space box (no
/// painting operator).
fn ellipse_path([x0, y0, x1, y1]: [f32; 4]) -> String {
    let (cx, cy, rx, ry) = (
        (x0 + x1) / 2.0,
        (y0 + y1) / 2.0,
        (x1 - x0) / 2.0,
        (y1 - y0) / 2.0,
    );
    let k = 0.5523;
    let mut s = format!(" {:.2} {:.2} m", cx + rx, cy);
    s.push_str(&format!(
        " {:.2} {:.2} {:.2} {:.2} {:.2} {:.2} c",
        cx + rx,
        cy + ry * k,
        cx + rx * k,
        cy + ry,
        cx,
        cy + ry
    ));
    s.push_str(&format!(
        " {:.2} {:.2} {:.2} {:.2} {:.2} {:.2} c",
        cx - rx * k,
        cy + ry,
        cx - rx,
        cy + ry * k,
        cx - rx,
        cy
    ));
    s.push_str(&format!(
        " {:.2} {:.2} {:.2} {:.2} {:.2} {:.2} c",
        cx - rx,
        cy - ry * k,
        cx - rx * k,
        cy - ry,
        cx,
        cy - ry
    ));
    s.push_str(&format!(
        " {:.2} {:.2} {:.2} {:.2} {:.2} {:.2} c",
        cx + rx * k,
        cy - ry,
        cx + rx,
        cy - ry * k,
        cx + rx,
        cy
    ));
    s
}

fn color_ops(c: [u8; 3], fill: bool) -> String {
    let (r, g, b) = (
        c[0] as f32 / 255.0,
        c[1] as f32 / 255.0,
        c[2] as f32 / 255.0,
    );
    format!("{r:.3} {g:.3} {b:.3} {}", if fill { "rg" } else { "RG" })
}

fn pdf_text_string(s: &str) -> Object {
    if s.is_ascii() {
        Object::String(s.as_bytes().to_vec(), StringFormat::Literal)
    } else {
        let mut bytes = vec![0xFE, 0xFF];
        for u in s.encode_utf16() {
            bytes.extend_from_slice(&u.to_be_bytes());
        }
        Object::String(bytes, StringFormat::Hexadecimal)
    }
}

/// Escape a string for a content-stream `(...) Tj` literal (WinAnsi; non-Latin
/// characters degrade to '?').
fn content_literal(s: &str) -> String {
    let mut out = String::from("(");
    for ch in s.chars() {
        match ch {
            '(' | ')' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            c if (c as u32) < 128 => out.push(c),
            c => match c as u32 {
                0xA0..=0xFF => out.push_str(&format!("\\{:03o}", c as u32)),
                _ => out.push('?'),
            },
        }
    }
    out.push(')');
    out
}

/// Rough word wrap for FreeText using Helvetica's average advance.
fn wrap_lines(text: &str, size: f32, width: f32) -> Vec<String> {
    let avg = size * 0.5;
    let max_chars = ((width / avg).floor() as usize).max(1);
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ') {
            if line.is_empty() {
                line = word.to_string();
            } else if line.chars().count() + 1 + word.chars().count() <= max_chars {
                line.push(' ');
                line.push_str(word);
            } else {
                lines.push(std::mem::take(&mut line));
                line = word.to_string();
            }
        }
        lines.push(line);
    }
    lines
}

const HELV: &str = "/Helv";
const HELV_BOLD: &str = "/HeBo";
const ZADB: &str = "/ZaDb";

fn font_resources() -> Dictionary {
    dictionary! {
        "Font" => dictionary! {
            "Helv" => dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding" },
            "HeBo" => dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica-Bold", "Encoding" => "WinAnsiEncoding" },
            "ZaDb" => dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "ZapfDingbats" },
        }
    }
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
            Object::Array(DASH.iter().map(|v| Object::Real(*v)).collect()),
        );
    }
    bs
}

/// Content operators that set the stroke: color, width, and dashes.
fn stroke_ops(a: &Annot) -> String {
    let mut ops = format!(" {} {:.2} w", color_ops(a.color, false), a.stroke);
    if a.dashed {
        ops.push_str(&format!(" [{} {}] 0 d", DASH[0], DASH[1]));
    }
    ops
}

/// An open arrowhead at `tip`, pointing away from `from` (user space).
fn arrowhead(from: Pt, tip: Pt, stroke: f32) -> String {
    let (dx, dy) = (tip.x - from.x, tip.y - from.y);
    let len = (dx * dx + dy * dy).sqrt().max(0.001);
    let (ux, uy) = (dx / len, dy / len);
    let head = 6.0 + stroke * 2.0;
    let (lx, ly) = (
        tip.x - head * (ux * 0.866 - uy * 0.5),
        tip.y - head * (uy * 0.866 + ux * 0.5),
    );
    let (rx, ry) = (
        tip.x - head * (ux * 0.866 + uy * 0.5),
        tip.y - head * (uy * 0.866 - ux * 0.5),
    );
    // Solid even on a dashed line.
    format!(
        " [] 0 d {lx:.2} {ly:.2} m {:.2} {:.2} l {rx:.2} {ry:.2} l S",
        tip.x, tip.y
    )
}

/// The main annotation dictionary (without `/AP` or `/P`) and its
/// appearance stream.
fn annotation(a: &Annot, geom: &PageGeom) -> (Dictionary, Stream) {
    // A replacement's Caret covers the caret; its StrikeOut is separate.
    let bounds = match &a.kind {
        AnnotKind::Replace { at, .. } => *at,
        _ => a.bounds(),
    };
    // Pad so strokes aren't clipped by the BBox.
    let pad = a.stroke.max(1.0) + 1.0;
    let padded = Box2 {
        x: bounds.x - pad,
        y: bounds.y - pad,
        w: bounds.w + 2.0 * pad,
        h: bounds.h + 2.0 * pad,
    };
    let rect = geom.box_to_user(padded);
    let [llx, lly, urx, ury] = rect;

    // Appearance content is drawn in user space, so it works for any /Rotate;
    // text is placed upright relative to the displayed page by rotating with
    // the page.
    let up = |p: Pt| geom.to_user(p);
    let mut content = String::new();
    let alpha = a.opacity.clamp(0.0, 1.0);
    let ext = dictionary! { "GS" => dictionary! { "Type" => "ExtGState", "CA" => alpha, "ca" => alpha,
    "BM" => Object::Name(if matches!(a.kind, AnnotKind::Highlight { .. }) { b"Multiply".to_vec() } else { b"Normal".to_vec() }) } };
    content.push_str("/GS gs ");
    let mut dict = dictionary! {
        "Type" => "Annot",
        "Rect" => pdf_rect(rect),
        "NM" => pdf_text_string(&a.name()),
        "T" => pdf_text_string(&a.author),
        "M" => pdf_text_string(&a.modified),
        "F" => 4i64, // Print
        "CA" => alpha,
        "SheafAnnot" => pdf_text_string(&serde_json::to_string(a).unwrap_or_default()),
    };
    if !a.contents.is_empty() {
        dict.set("Contents", pdf_text_string(&a.contents));
    }
    if !a.created.is_empty() {
        dict.set("CreationDate", pdf_text_string(&a.created));
    }
    let c = a.color;
    let color_arr = Object::Array(c.iter().map(|v| Object::Real(*v as f32 / 255.0)).collect());

    // Rotation of upright text, in degrees, to counter the page rotation.
    let text_rot = geom.rotate.rem_euclid(360) as f32;
    let (cs, sn) = (text_rot.to_radians().cos(), text_rot.to_radians().sin());
    let text_matrix = |origin: Pt| {
        let o = up(origin);
        format!(
            "{cs:.4} {sn:.4} {:.4} {cs:.4} {:.3} {:.3} Tm",
            -sn, o.x, o.y
        )
    };

    match &a.kind {
        AnnotKind::Highlight { rects } => {
            dict.set("Subtype", Object::Name(b"Highlight".to_vec()));
            dict.set("C", color_arr);
            let mut quads = Vec::new();
            content.push_str(&color_ops(c, true));
            for r in rects {
                let [x0, y0, x1, y1] = geom.box_to_user(*r);
                // QuadPoints order: x1 y1 x2 y2 (upper), x3 y3 x4 y4 (lower).
                quads.extend([x0, y1, x1, y1, x0, y0, x1, y0].map(Object::Real));
                content.push_str(&format!(
                    " {x0:.2} {y0:.2} {:.2} {:.2} re f",
                    x1 - x0,
                    y1 - y0
                ));
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
                let [x0, y0, x1, y1] = geom.box_to_user(*r);
                quads.extend([x0, y1, x1, y1, x0, y0, x1, y0].map(Object::Real));
            }
            dict.set("QuadPoints", Object::Array(quads));
            // Drawn in displayed space, then taken to user space, so the
            // line follows the text whatever the page's /Rotate.
            let width = text_line_width(rects);
            content.push_str(&format!(" {} {width:.2} w", color_ops(c, false)));
            for path in text_line_paths(&a.kind) {
                let mut first = true;
                for p in path {
                    let u = up(p);
                    content.push_str(&format!(
                        " {:.2} {:.2} {}",
                        u.x,
                        u.y,
                        if first { "m" } else { "l" }
                    ));
                    first = false;
                }
                content.push_str(" S");
            }
        }
        AnnotKind::Replace { at, .. } | AnnotKind::Insert { at } => {
            dict.set("Subtype", Object::Name(b"Caret".to_vec()));
            dict.set("C", color_arr);
            dict.set("Sy", Object::Name(b"None".to_vec()));
            let [p0, p1, p2] = caret_points(*at).map(up);
            content.push_str(&format!(
                " {} {:.2} {:.2} m {:.2} {:.2} l {:.2} {:.2} l h f",
                color_ops(c, true),
                p0.x,
                p0.y,
                p1.x,
                p1.y,
                p2.x,
                p2.y
            ));
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
        AnnotKind::Note { pos } => {
            dict.set("Subtype", Object::Name(b"Text".to_vec()));
            dict.set("Name", Object::Name(b"Comment".to_vec()));
            dict.set("C", color_arr);
            dict.set("Open", false);
            let [x0, y0, x1, y1] = geom.box_to_user(Box2 {
                x: pos.x,
                y: pos.y,
                w: NOTE_SIZE,
                h: NOTE_SIZE,
            });
            content.push_str(&format!(
                " {} {} {x0:.2} {y0:.2} {:.2} {:.2} re B 0.5 w 1 1 1 RG",
                color_ops(c, true),
                "0 0 0 RG 1 w",
                x1 - x0,
                y1 - y0
            ));
            // Three "text lines" inside the icon.
            for k in 1..=3 {
                let yy = y0 + (y1 - y0) * (k as f32 / 4.0);
                content.push_str(&format!(
                    " {:.2} {yy:.2} m {:.2} {yy:.2} l S",
                    x0 + 4.0,
                    x1 - 4.0
                ));
            }
        }
        AnnotKind::FreeText { rect, size } => {
            dict.set("Subtype", Object::Name(b"FreeText".to_vec()));
            dict.set(
                "DA",
                pdf_text_string(&format!("{HELV} {size} Tf {}", color_ops(c, true))),
            );
            dict.set("C", Object::Array(vec![]));
            let lines = wrap_lines(&a.contents, *size, rect.w - 4.0);
            content.push_str(&format!(
                " BT {HELV} {size} Tf {} {:.2} TL",
                color_ops(c, true),
                size * 1.2
            ));
            for (i, line) in lines.iter().enumerate() {
                let origin = Pt {
                    x: rect.x + 2.0,
                    y: rect.y + 2.0 + size * (1.0 + 1.2 * i as f32),
                };
                content.push_str(&format!(
                    " {} {} Tj",
                    text_matrix(origin),
                    content_literal(line)
                ));
            }
            content.push_str(" ET");
        }
        AnnotKind::Tape {
            rect, size, look, ..
        } => {
            dict.set("Subtype", Object::Name(b"FreeText".to_vec()));
            dict.set(
                "DA",
                pdf_text_string(&format!("{HELV} {size} Tf {}", color_ops(c, true))),
            );
            dict.set("C", Object::Array(vec![Object::Real(1.0); 3]));
            let tape = a.tape().unwrap_or_default();
            content.push_str(&tape_content(*rect, *size, look, &tape, c, geom));
        }
        AnnotKind::Square { rect } | AnnotKind::Ellipse { rect } => {
            let square = matches!(a.kind, AnnotKind::Square { .. });
            let subtype: &[u8] = if square { b"Square" } else { b"Circle" };
            dict.set("Subtype", Object::Name(subtype.to_vec()));
            dict.set("C", color_arr);
            dict.set("BS", border_style(a));
            content.push_str(&stroke_ops(a));
            if let Some(f) = a.fill {
                dict.set(
                    "IC",
                    Object::Array(f.iter().map(|v| Object::Real(*v as f32 / 255.0)).collect()),
                );
                content.push_str(&format!(" {}", color_ops(f, true)));
            }
            let b = geom.box_to_user(*rect);
            if square {
                let [x0, y0, x1, y1] = b;
                content.push_str(&format!(
                    " {x0:.2} {y0:.2} {:.2} {:.2} re",
                    x1 - x0,
                    y1 - y0
                ));
            } else {
                content.push_str(&ellipse_path(b));
            }
            content.push_str(if a.fill.is_some() { " B" } else { " S" });
        }
        AnnotKind::Connector { rect, label, .. } => {
            // A Link so other viewers jump too; the writer fills in /Dest once
            // the partner's page object is known.
            dict.set("Subtype", Object::Name(b"Link".to_vec()));
            dict.set(
                "Border",
                Object::Array(vec![0i64.into(), 0i64.into(), 0i64.into()]),
            );
            dict.set("H", Object::Name(b"I".to_vec()));
            // Filled disc in the mark's color, label in white.
            content.push_str(&format!(" {}", color_ops(c, true)));
            content.push_str(&ellipse_path(geom.box_to_user(*rect)));
            content.push_str(" f");
            let size = rect.h * 0.55;
            let chars = label.chars().count().max(1) as f32;
            let origin = Pt {
                x: rect.x + rect.w / 2.0 - size * 0.3 * chars,
                y: rect.y + rect.h / 2.0 + size * 0.36,
            };
            content.push_str(&format!(
                " BT {HELV_BOLD} {size:.2} Tf 1 1 1 rg {} {} Tj ET",
                text_matrix(origin),
                content_literal(label)
            ));
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
            content.push_str(&stroke_ops(a));
            content.push_str(&format!(
                " {:.2} {:.2} m {:.2} {:.2} l S",
                ua.x, ua.y, ub.x, ub.y
            ));
            if *arrow {
                content.push_str(&arrowhead(ua, ub, a.stroke));
            }
            if *arrow_start {
                content.push_str(&arrowhead(ub, ua, a.stroke));
            }
        }
        AnnotKind::Ink { paths } => {
            dict.set("Subtype", Object::Name(b"Ink".to_vec()));
            dict.set("C", color_arr);
            dict.set("BS", border_style(a));
            let mut ink_list = Vec::new();
            content.push_str(&stroke_ops(a));
            content.push_str(" 1 J 1 j");
            for path in paths {
                let mut coords = Vec::new();
                for (i, p) in path.iter().enumerate() {
                    let u = up(*p);
                    coords.push(Object::Real(u.x));
                    coords.push(Object::Real(u.y));
                    content.push_str(&format!(
                        " {:.2} {:.2} {}",
                        u.x,
                        u.y,
                        if i == 0 { "m" } else { "l" }
                    ));
                }
                content.push_str(" S");
                ink_list.push(Object::Array(coords));
            }
            dict.set("InkList", Object::Array(ink_list));
        }
        AnnotKind::PageStamp {
            rect, name, text, ..
        } => {
            dict.set("Subtype", Object::Name(b"Stamp".to_vec()));
            if a.contents.is_empty() {
                dict.set("Contents", pdf_text_string(name));
            }
            // The page itself goes in when it's written (`to_pdf_in`); a
            // dynamic stamp's text over it is drawn here.
            if let Some(t) = text.as_ref().filter(|t| !t.is_empty()) {
                let size = (rect.h * 0.16).clamp(5.0, 24.0);
                let width = helv_text_width(t, size, true);
                let origin = Pt {
                    x: rect.x + (rect.w - width).max(0.0) / 2.0,
                    y: rect.bottom() - rect.h * 0.08,
                };
                content.push_str(&format!(
                    " BT {HELV_BOLD} {size:.2} Tf {} {} {} Tj ET",
                    color_ops(c, true),
                    text_matrix(origin),
                    content_literal(t)
                ));
            }
        }
        AnnotKind::PageBorder { rect, status } => {
            dict.set("Subtype", Object::Name(b"Square".to_vec()));
            dict.set("C", color_arr);
            dict.set("BS", border_style(a));
            if a.contents.is_empty() {
                dict.set("Contents", pdf_text_string(&format!("Status: {status}")));
            }
            content.push_str(&stroke_ops(a));
            let [x0, y0, x1, y1] = geom.box_to_user(*rect);
            content.push_str(&format!(
                " {x0:.2} {y0:.2} {:.2} {:.2} re S",
                x1 - x0,
                y1 - y0
            ));
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
            content.push_str(&format!(" {} {lw:.2} w", color_ops(c, false)));
            for (i, (p, ctrl)) in segs.iter().enumerate() {
                match (i, ctrl) {
                    (0, _) => content.push_str(&format!(" {:.2} {:.2} m", p.x, p.y)),
                    (_, None) => content.push_str(&format!(" {:.2} {:.2} l", p.x, p.y)),
                    (_, Some((c1, c2))) => content.push_str(&format!(
                        " {:.2} {:.2} {:.2} {:.2} {:.2} {:.2} c",
                        c1.x, c1.y, c2.x, c2.y, p.x, p.y
                    )),
                }
            }
            content.push_str(" h S");
            for (line, size, baseline) in text_stamp_lines(*rect, text) {
                let width = helv_text_width(&line, size, true);
                let origin = Pt {
                    x: rect.x + (rect.w - width) / 2.0,
                    y: baseline,
                };
                content.push_str(&format!(
                    " BT {HELV_BOLD} {size:.2} Tf {} {} {} Tj ET",
                    color_ops(c, true),
                    text_matrix(origin),
                    content_literal(&line)
                ));
            }
        }
        AnnotKind::Stamp { rect, glyph } => {
            dict.set("Subtype", Object::Name(b"Stamp".to_vec()));
            dict.set(
                "Name",
                Object::Name(format!("SheafTick{}", a.id.0).into_bytes()),
            );
            // ZapfDingbats has the check and cross; everything else is bold text.
            let (font, text): (&str, String) = match glyph.as_str() {
                "✓" => (ZADB, "4".into()),
                "✗" => (ZADB, "8".into()),
                other => (HELV_BOLD, other.to_string()),
            };
            let size = rect.h * 0.8;
            let origin = Pt {
                x: rect.x + rect.w * 0.1,
                y: rect.y + rect.h * 0.85,
            };
            content.push_str(&format!(
                " BT {font} {size:.2} Tf {} {} {} Tj ET",
                color_ops(c, true),
                text_matrix(origin),
                content_literal(&text)
            ));
        }
    }

    let stream = Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => pdf_rect([llx, lly, urx, ury]),
            "Resources" => {
                let mut r = font_resources();
                r.set("ExtGState", ext);
                r
            },
        },
        content.into_bytes(),
    );
    (dict, stream)
}

/// How thick the line of an underline, strikeout or squiggly is: a
/// fraction of the text's height.
pub fn text_line_width(rects: &[Box2]) -> f32 {
    let h = rects.iter().map(|r| r.h).fold(0.0, f32::max);
    (h * 0.07).clamp(0.75, 3.0)
}

/// The lines an underline, strikeout or squiggly draws, in displayed
/// space: one path per rect, under it, through its middle, or a wave
/// under it.
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

/// A caret's triangle in displayed space: its tip at the top centre.
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

/// A text stamp's lines as drawn: (text, size, baseline in displayed
/// space). The first is large, the rest small, centred as a block.
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
        .map(|(l, size, _)| helv_text_width(l, *size, true))
        .fold(0.0, f32::max);
    (widest + height * 0.6, height)
}

/// Advance of `s` in Helvetica (bold or not) at `size`, from the AFM widths
/// of ASCII; other characters count as a capital.
pub fn helv_text_width(s: &str, size: f32, bold: bool) -> f32 {
    s.chars()
        .map(|c| {
            let (regular, b) = match c {
                ' ' => (278.0, 278.0),
                '.' | ',' => (278.0, 278.0),
                ':' | ';' => (278.0, 333.0),
                '/' => (278.0, 278.0),
                '-' => (333.0, 333.0),
                'i' | 'j' | 'l' => (222.0, 278.0),
                'f' | 't' => (278.0, 333.0),
                'r' => (333.0, 389.0),
                'm' => (833.0, 889.0),
                'w' => (722.0, 778.0),
                'I' => (278.0, 278.0),
                'J' => (500.0, 556.0),
                'M' => (833.0, 833.0),
                'W' => (944.0, 944.0),
                '0'..='9' => (556.0, 556.0),
                'a'..='z' => (556.0, 611.0),
                'A'..='Z' => (667.0, 722.0),
                _ => (667.0, 722.0),
            };
            if bold { b } else { regular }
        })
        .sum::<f32>()
        * size
        / 1000.0
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
/// in the page's displayed space, the tape turned as `look` says.
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
    let width = |s: &str, bold: bool| helv_text_width(s, size, bold);
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

/// A tape's appearance: paper, then its rows (number, amount or title,
/// symbol, note), drawn in the tape's own frame and mapped onto the page,
/// so it reads the right way whichever way it's turned.
fn tape_content(
    rect: Box2,
    size: f32,
    look: &crate::tape::TapeLook,
    tape: &crate::tape::Tape,
    c: [u8; 3],
    geom: &PageGeom,
) -> String {
    use crate::tape::{Row, fmt_amount};
    let (w, h) = tape_frame(rect, look);
    // The frame, PDF-style (y up from its bottom left), to user space.
    let user = |x: f32, y: f32| geom.to_user(tape_to_shown(rect, look, x, h - y));
    let o = user(0.0, 0.0);
    let ex = user(1.0, 0.0);
    let ey = user(0.0, 1.0);
    let mut out = format!(
        " q {:.5} {:.5} {:.5} {:.5} {:.3} {:.3} cm",
        ex.x - o.x,
        ex.y - o.y,
        ey.x - o.x,
        ey.y - o.y,
        o.x,
        o.y
    );
    // Paper, a thin grey border, and nothing drawn outside it.
    out.push_str(&format!(
        " {} 0.6 G 0.5 w 0 0 {w:.2} {h:.2} re B 0 0 {w:.2} {h:.2} re W n",
        color_ops(look.paper, true)
    ));
    let (number_w, amount_w, symbol_w, _) = tape_columns(tape, size, look);
    let amount_right = TAPE_PAD + number_w + amount_w;
    let note_x = amount_right + symbol_w;
    let shown = look.shown(tape);
    let baseline = |i: usize| h - (TAPE_PAD + size * (1.0 + TAPE_LINE * i as f32));
    // Rules above totals first: paths can't go inside BT/ET.
    out.push_str(" 0.3 G 0.5 w");
    for (i, (_, l)) in shown.iter().enumerate() {
        if l.row.is_total() {
            let y = baseline(i) + size * 1.05;
            out.push_str(&format!(
                " {:.2} {y:.2} m {:.2} {y:.2} l S",
                TAPE_PAD + number_w,
                note_x
            ));
        }
    }
    out.push_str(&format!(" BT {}", color_ops(c, true)));
    let text = |out: &mut String, font: &str, x: f32, y: f32, s: &str| {
        out.push_str(&format!(
            " {font} {size:.2} Tf 1 0 0 1 {x:.2} {y:.2} Tm {} Tj",
            content_literal(s)
        ));
    };
    for (i, (number, l)) in shown.iter().enumerate() {
        let y = baseline(i);
        if look.numbers {
            text(&mut out, HELV, TAPE_PAD, y, number);
        }
        match &l.row {
            Row::Title(t) => text(&mut out, HELV_BOLD, TAPE_PAD + number_w, y, t),
            row => {
                let font = if row.is_total() { HELV_BOLD } else { HELV };
                let v = fmt_amount(row.value().unwrap_or(0.0), look.decimals);
                let x = amount_right - helv_text_width(&v, size, row.is_total());
                text(&mut out, font, x, y, &v);
                text(&mut out, font, amount_right + size * 0.3, y, row.symbol());
            }
        }
        if !l.note.is_empty() {
            text(&mut out, HELV, note_x + size * 0.4, y, &l.note);
        }
    }
    out.push_str(" ET Q");
    out
}

/// A tape alone as a one-page PDF its size: how sheaf shows a turned tape
/// on screen, from a picture of it.
pub fn tape_pdf(a: &Annot) -> Option<Vec<u8>> {
    let AnnotKind::Tape {
        rect, size, look, ..
    } = &a.kind
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
    let geom = PageGeom {
        media: [0.0, 0.0, rect.w, rect.h],
        rotate: 0,
    };
    let content = tape_content(at, *size, look, &tape, a.color, &geom);
    let mut pdf = lopdf::Document::with_version("1.5");
    let pages_id = pdf.new_object_id();
    let content = pdf.add_object(Stream::new(dictionary! {}, content.into_bytes()));
    let page = pdf.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => content,
        "MediaBox" => vec![0.into(), 0.into(), Object::Real(rect.w), Object::Real(rect.h)],
        "Resources" => font_resources(),
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

/// Parse a `/SheafAnnot` payload.
pub fn from_json(s: &[u8]) -> Option<Annot> {
    serde_json::from_slice(s).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geom(rot: i32) -> PageGeom {
        PageGeom {
            media: [0.0, 0.0, 612.0, 792.0],
            rotate: rot,
        }
    }

    #[test]
    fn display_user_round_trip_all_rotations() {
        for rot in [0, 90, 180, 270] {
            let g = geom(rot);
            for p in [
                Pt { x: 0.0, y: 0.0 },
                Pt { x: 100.0, y: 50.0 },
                Pt { x: 300.0, y: 700.0 },
            ] {
                let back = g.to_display(g.to_user(p));
                assert!(
                    (back.x - p.x).abs() < 1e-3 && (back.y - p.y).abs() < 1e-3,
                    "rot {rot} {p:?} -> {back:?}"
                );
            }
        }
    }

    #[test]
    fn display_corners_map_sensibly() {
        // Unrotated: displayed top-left is user (x0, y1).
        let g = geom(0);
        let u = g.to_user(Pt { x: 0.0, y: 0.0 });
        assert_eq!((u.x, u.y), (0.0, 792.0));
        // 90° CW: displayed page is 792 wide; displayed top-left is user bottom-left.
        let g = geom(90);
        let u = g.to_user(Pt { x: 0.0, y: 0.0 });
        assert_eq!((u.x, u.y), (0.0, 0.0));
        let u = g.to_user(Pt { x: 792.0, y: 0.0 });
        assert_eq!((u.x, u.y), (0.0, 792.0));
    }

    #[test]
    fn marks_saved_before_creation_dates_still_read() {
        let old = br#"{"id":3,"kind":{"Note":{"pos":{"x":1.0,"y":2.0}}},"color":[1,2,3],"opacity":1.0,"stroke":1.0,"author":"me","contents":"hi","modified":"D:20261011000000"}"#;
        let a = from_json(old).unwrap();
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
                    w: 200.0,
                    h: 40.0,
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
        };
        let mut pdf = lopdf::Document::with_version("1.7");
        let Written { main: dict, .. } = to_pdf(&a, &geom(0), &mut pdf);
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
        assert_eq!(from_json(&json).unwrap(), a);
        let content = String::from_utf8_lossy(&stream.content);
        assert!(content.contains("Tj"), "{content}");
        assert!(content.contains("\\(world\\)"), "{content}");
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
        }
    }

    fn bx(x: f32, y: f32, w: f32, h: f32) -> Box2 {
        Box2 { x, y, w, h }
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
        let square = mark(AnnotKind::Square {
            rect: bx(0.0, 0.0, 100.0, 100.0),
        });
        assert!(square.hit(Pt { x: 50.0, y: 50.0 }, 3.0));
        assert!(square.hit(Pt { x: -2.0, y: 50.0 }, 3.0));
        assert!(!square.hit(Pt { x: -5.0, y: 50.0 }, 3.0));
    }

    #[test]
    fn fills_dashes_and_arrowheads_reach_the_file() {
        let mut pdf = lopdf::Document::with_version("1.7");
        let mut square = mark(AnnotKind::Square {
            rect: bx(10.0, 10.0, 100.0, 50.0),
        });
        square.fill = Some([255, 0, 0]);
        square.dashed = true;
        let main = to_pdf(&square, &geom(0), &mut pdf).main;
        let ic = main.get(b"IC").unwrap().as_array().unwrap();
        assert_eq!(ic.len(), 3);
        assert_eq!(ic[0].as_float().unwrap(), 1.0);
        let bs = main.get(b"BS").unwrap().as_dict().unwrap();
        assert_eq!(bs.get(b"S").unwrap().as_name().unwrap(), b"D");
        assert_eq!(bs.get(b"D").unwrap().as_array().unwrap().len(), 2);

        // Unfilled and solid: neither key.
        square.fill = None;
        square.dashed = false;
        let main = to_pdf(&square, &geom(0), &mut pdf).main;
        assert!(main.get(b"IC").is_err());
        let bs = main.get(b"BS").unwrap().as_dict().unwrap();
        assert!(bs.get(b"S").is_err());

        let line = mark(AnnotKind::Line {
            a: Pt { x: 0.0, y: 0.0 },
            b: Pt { x: 100.0, y: 0.0 },
            arrow: false,
            arrow_start: true,
        });
        let main = to_pdf(&line, &geom(0), &mut pdf).main;
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
        assert_eq!(from_json(&json).unwrap(), a);
        assert_eq!(
            a.pdf_names(),
            vec!["sheaf:1".to_string(), "sheaf:1:strike".to_string()]
        );
        let ins = mark(AnnotKind::Insert {
            at: caret_box(50.0, 22.0, 12.0),
        });
        assert_eq!(from_json(&serde_json::to_vec(&ins).unwrap()).unwrap(), ins);
        assert_eq!(ins.bounds().w, ins.bounds().h);
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
}
