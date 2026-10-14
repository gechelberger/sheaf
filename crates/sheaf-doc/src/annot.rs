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
        arrow: bool,
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
    /// Calculator tape. `contents` holds one entry per line as
    /// `"<amount> <op>"` (see `tape_lines`); drawn as a paper tape with
    /// right-aligned amounts, totals (`T`) ruled and bold.
    Tape {
        rect: Box2,
        size: f32,
    },
}

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
    /// The same shape moved by (`dx`, `dy`) points.
    pub fn translated(&self, dx: f32, dy: f32) -> AnnotKind {
        let mv = |p: &Pt| Pt {
            x: p.x + dx,
            y: p.y + dy,
        };
        let mb = |b: &Box2| Box2 {
            x: b.x + dx,
            y: b.y + dy,
            w: b.w,
            h: b.h,
        };
        match self {
            AnnotKind::Highlight { rects } => AnnotKind::Highlight {
                rects: rects.iter().map(mb).collect(),
            },
            AnnotKind::Note { pos } => AnnotKind::Note { pos: mv(pos) },
            AnnotKind::FreeText { rect, size } => AnnotKind::FreeText {
                rect: mb(rect),
                size: *size,
            },
            AnnotKind::Square { rect } => AnnotKind::Square { rect: mb(rect) },
            AnnotKind::Ellipse { rect } => AnnotKind::Ellipse { rect: mb(rect) },
            AnnotKind::Line { a, b, arrow } => AnnotKind::Line {
                a: mv(a),
                b: mv(b),
                arrow: *arrow,
            },
            AnnotKind::Ink { paths } => AnnotKind::Ink {
                paths: paths.iter().map(|p| p.iter().map(mv).collect()).collect(),
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
            AnnotKind::Tape { rect, size } => AnnotKind::Tape {
                rect: mb(rect),
                size: *size,
            },
        }
    }
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
    /// PDF date string, e.g. `D:20261011153000`.
    pub modified: String,
}

impl Annot {
    /// Bounding box in displayed page space.
    pub fn bounds(&self) -> Box2 {
        match &self.kind {
            AnnotKind::Highlight { rects } => {
                rects.iter().copied().reduce(Box2::union).unwrap_or(Box2 {
                    x: 0.0,
                    y: 0.0,
                    w: 0.0,
                    h: 0.0,
                })
            }
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
            AnnotKind::Stamp { rect, .. } | AnnotKind::Connector { rect, .. } => *rect,
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

/// The annotation dictionary (without `/P`) and its appearance stream,
/// ready to be added to a page's `/Annots`. The caller adds `/AP << /N ref >>`
/// after inserting the stream and sets `/P`.
pub fn to_pdf(a: &Annot, geom: &PageGeom) -> (Dictionary, Stream) {
    let bounds = a.bounds();
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
        AnnotKind::Tape { rect, size } => {
            dict.set("Subtype", Object::Name(b"FreeText".to_vec()));
            dict.set(
                "DA",
                pdf_text_string(&format!("{HELV} {size} Tf {}", color_ops(c, true))),
            );
            dict.set("C", Object::Array(vec![Object::Real(1.0); 3]));
            // Paper: white, thin grey border.
            let [x0, y0, x1, y1] = geom.box_to_user(*rect);
            content.push_str(&format!(
                " 1 1 1 rg 0.6 G 0.5 w {x0:.2} {y0:.2} {:.2} {:.2} re B",
                x1 - x0,
                y1 - y0
            ));
            let lines = tape_lines(&a.contents);
            let op_w = size * TAPE_OP_W;
            let right = rect.right() - TAPE_PAD;
            let baseline = |i: usize| rect.y + TAPE_PAD + size * (1.0 + TAPE_LINE * i as f32);
            // Rules above totals (paths can't go inside BT/ET).
            content.push_str(" 0.3 G 0.5 w");
            for (i, (_, op)) in lines.iter().enumerate() {
                if op == "T" {
                    let y = baseline(i) - size * 1.05;
                    let p = up(Pt {
                        x: rect.x + TAPE_PAD,
                        y,
                    });
                    let q = up(Pt { x: right, y });
                    content.push_str(&format!(
                        " {:.2} {:.2} m {:.2} {:.2} l S",
                        p.x, p.y, q.x, q.y
                    ));
                }
            }
            content.push_str(&format!(" BT {}", color_ops(c, true)));
            for (i, (val, op)) in lines.iter().enumerate() {
                let font = if op == "T" { HELV_BOLD } else { HELV };
                let origin = Pt {
                    x: right - op_w - helv_width(val, *size),
                    y: baseline(i),
                };
                content.push_str(&format!(
                    " {font} {size:.2} Tf {} {} Tj",
                    text_matrix(origin),
                    content_literal(val)
                ));
                if !op.is_empty() {
                    let origin = Pt {
                        x: right - op_w + size * 0.2,
                        y: baseline(i),
                    };
                    content.push_str(&format!(
                        " {} {} Tj",
                        text_matrix(origin),
                        content_literal(op)
                    ));
                }
            }
            content.push_str(" ET");
        }
        AnnotKind::Square { rect } => {
            dict.set("Subtype", Object::Name(b"Square".to_vec()));
            dict.set("C", color_arr);
            dict.set("BS", dictionary! { "W" => a.stroke });
            let [x0, y0, x1, y1] = geom.box_to_user(*rect);
            content.push_str(&format!(
                " {} {:.2} w {x0:.2} {y0:.2} {:.2} {:.2} re S",
                color_ops(c, false),
                a.stroke,
                x1 - x0,
                y1 - y0
            ));
        }
        AnnotKind::Ellipse { rect } => {
            dict.set("Subtype", Object::Name(b"Circle".to_vec()));
            dict.set("C", color_arr);
            dict.set("BS", dictionary! { "W" => a.stroke });
            content.push_str(&format!(" {} {:.2} w", color_ops(c, false), a.stroke));
            content.push_str(&ellipse_path(geom.box_to_user(*rect)));
            content.push_str(" S");
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
        } => {
            dict.set("Subtype", Object::Name(b"Line".to_vec()));
            dict.set("C", color_arr);
            dict.set("BS", dictionary! { "W" => a.stroke });
            let (ua, ub) = (up(*pa), up(*pb));
            dict.set(
                "L",
                Object::Array(vec![ua.x.into(), ua.y.into(), ub.x.into(), ub.y.into()]),
            );
            if *arrow {
                dict.set(
                    "LE",
                    Object::Array(vec![
                        Object::Name(b"None".to_vec()),
                        Object::Name(b"OpenArrow".to_vec()),
                    ]),
                );
            }
            content.push_str(&format!(
                " {} {:.2} w {:.2} {:.2} m {:.2} {:.2} l S",
                color_ops(c, false),
                a.stroke,
                ua.x,
                ua.y,
                ub.x,
                ub.y
            ));
            if *arrow {
                let (dx, dy) = (ub.x - ua.x, ub.y - ua.y);
                let len = (dx * dx + dy * dy).sqrt().max(0.001);
                let (ux, uy) = (dx / len, dy / len);
                let head = 6.0 + a.stroke * 2.0;
                let (lx, ly) = (
                    ub.x - head * (ux * 0.866 - uy * 0.5),
                    ub.y - head * (uy * 0.866 + ux * 0.5),
                );
                let (rx, ry) = (
                    ub.x - head * (ux * 0.866 + uy * 0.5),
                    ub.y - head * (uy * 0.866 - ux * 0.5),
                );
                content.push_str(&format!(
                    " {lx:.2} {ly:.2} m {:.2} {:.2} l {rx:.2} {ry:.2} l S",
                    ub.x, ub.y
                ));
            }
        }
        AnnotKind::Ink { paths } => {
            dict.set("Subtype", Object::Name(b"Ink".to_vec()));
            dict.set("C", color_arr);
            dict.set("BS", dictionary! { "W" => a.stroke });
            let mut ink_list = Vec::new();
            content.push_str(&format!(
                " {} {:.2} w 1 J 1 j",
                color_ops(c, false),
                a.stroke
            ));
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
        };
        let (dict, stream) = to_pdf(&a, &geom(0));
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
}
