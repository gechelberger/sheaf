//! Another tool's annotation as a sheaf mark (M9 decision 5). One
//! converter serves both editing a third-party comment in place and
//! importing comments from FDF, XFDF or another PDF: each is, or is made
//! into, a PDF annotation dictionary first.
//!
//! The mark keeps the annotation's `/NM` ([`Annot::name`]), so replies to
//! it still find it, and a save puts the mark where the annotation was.

use lopdf::{Dictionary, Object};
use sheaf_core::Rotation;

use crate::annot::{Annot, AnnotId, AnnotKind, Box2, Pt, REVIEW};
use crate::attachments::Objects;
use crate::file::{pdf_text, thread_name};

/// What a conversion may do beyond reading the dictionary.
#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    /// Copy an attachment's embedded file into the cache (and so make it
    /// a mark). Off when converting a whole file ahead of need, where
    /// copying every attachment would be waste.
    pub files: bool,
}

/// Why an annotation didn't become a mark.
#[derive(Clone, Debug, PartialEq)]
pub enum NotAMark {
    /// Not a comment: a link, form field or popup. Passed over.
    NotComment,
    /// A comment sheaf can't hold, by subtype. Reported.
    Unsupported(String),
    /// Part of another, which became a mark with it: the crossed-out
    /// text of a replacement.
    Part,
}

/// Each of a page's annotations (`dicts`, in order) as a mark that reads
/// upright turned `turn`, as the page shows. A replacement's StrikeOut,
/// grouped under its Caret, is folded into the Caret's mark. `pdf`
/// resolves references (`/IRT`, `/AP`, `/FS`).
pub fn convert_page<P: Objects + ?Sized>(
    pdf: &P,
    dicts: &[&Dictionary],
    turn: Rotation,
    opts: Options,
) -> Vec<Result<Annot, NotAMark>> {
    let names: Vec<String> = dicts.iter().map(|d| thread_name(d)).collect();
    // Grouped parts, by the name of what they're grouped under.
    let mut parts: std::collections::HashMap<String, Vec<usize>> = Default::default();
    for (i, d) in dicts.iter().enumerate() {
        if is_grouped(d)
            && let Some(parent) = irt_name(pdf, d)
        {
            parts.entry(parent).or_default().push(i);
        }
    }
    let mut out: Vec<Result<Annot, NotAMark>> =
        dicts.iter().map(|d| convert(pdf, d, turn, opts)).collect();
    for (i, d) in dicts.iter().enumerate() {
        let Some(kids) = parts.get(&names[i]) else {
            continue;
        };
        if subtype(d) != "Caret" {
            continue;
        }
        let strike = kids
            .iter()
            .copied()
            .find(|&k| subtype(dicts[k]) == "StrikeOut");
        let Some(k) = strike else { continue };
        let rects = match &out[k] {
            Ok(Annot {
                kind: AnnotKind::StrikeOut { rects },
                ..
            }) => rects.clone(),
            _ => continue,
        };
        if let Ok(a) = &mut out[i]
            && let AnnotKind::Insert { at } = a.kind
        {
            a.kind = AnnotKind::Replace {
                rects,
                struck: String::new(),
                at,
            };
            out[k] = Err(NotAMark::Part);
        }
    }
    out
}

/// `dict` as a mark that reads upright turned `turn`. Its geometry is the
/// standard keys', which are PageSpace.
pub fn convert<P: Objects + ?Sized>(
    pdf: &P,
    dict: &Dictionary,
    turn: Rotation,
    opts: Options,
) -> Result<Annot, NotAMark> {
    let sub = subtype(dict);
    if matches!(
        sub.as_str(),
        "" | "Link" | "Widget" | "Popup" | "PrinterMark" | "TrapNet" | "Watermark"
    ) {
        return Err(NotAMark::NotComment);
    }
    let unsupported = || NotAMark::Unsupported(sub.clone());
    let rect = numbers(pdf, dict, b"Rect")
        .filter(|r| r.len() == 4)
        .map(|r| user_box([r[0], r[1], r[2], r[3]]))
        .ok_or_else(unsupported)?;
    let text = |key: &[u8]| dict.get(key).map(pdf_text).unwrap_or_default();
    let stroke = border_width(pdf, dict);
    // The box inside /Rect: less /RD where there is one, else less
    // `border` (half a shape's line, which is drawn on the box).
    let within = |border: f32| match numbers(pdf, dict, b"RD").filter(|d| d.len() == 4) {
        Some(d) => {
            let r = numbers(pdf, dict, b"Rect").unwrap_or_default();
            user_box([r[0] + d[0], r[1] + d[1], r[2] - d[2], r[3] - d[3]])
        }
        None => inset(rect, border),
    };
    let inner = || within(stroke / 2.0);
    let point = |x: f32, y: f32| Pt { x, y };
    let mut color = color_of(pdf, dict, b"C");
    let kind = match sub.as_str() {
        "Text" => {
            let state = text(b"State");
            match irt_name(pdf, dict) {
                Some(parent) if !state.is_empty() => AnnotKind::Status {
                    parent,
                    model: Some(text(b"StateModel"))
                        .filter(|m| !m.is_empty())
                        .unwrap_or_else(|| REVIEW.to_string()),
                    state,
                },
                Some(parent) => AnnotKind::Reply { parent },
                None => AnnotKind::Note { pos: icon_at(rect) },
            }
        }
        "Highlight" | "Underline" | "StrikeOut" | "Squiggly" => {
            let rects = quads(pdf, dict)
                .filter(|q| !q.is_empty())
                .unwrap_or_else(|| vec![rect]);
            match sub.as_str() {
                "Highlight" => {
                    color = color.or(Some([255, 230, 0]));
                    AnnotKind::Highlight { rects }
                }
                "Underline" => AnnotKind::Underline { rects },
                "StrikeOut" => AnnotKind::StrikeOut { rects },
                _ => AnnotKind::Squiggly { rects },
            }
        }
        "FreeText" => {
            let (size, ink) = default_appearance(&text(b"DA"));
            // The text's color, not the box's.
            color = ink.or(Some([0, 0, 0]));
            let size = size.filter(|s| *s > 0.0).unwrap_or(12.0);
            let callout = dict
                .get(b"IT")
                .ok()
                .and_then(|o| o.as_name().ok())
                .is_some_and(|n| n == b"FreeTextCallout");
            match numbers(pdf, dict, b"CL").filter(|_| callout) {
                Some(cl) if cl.len() == 4 || cl.len() == 6 => {
                    let pts = points(&cl);
                    // [point, knee, end], or [point, end] with no knee.
                    AnnotKind::Callout {
                        rect: inner(),
                        point: pts[0],
                        knee: pts[1],
                        size,
                    }
                }
                _ => AnnotKind::FreeText {
                    rect: inner(),
                    size,
                },
            }
        }
        "Square" => AnnotKind::Square { rect: inner() },
        "Circle" => AnnotKind::Ellipse { rect: inner() },
        "Line" => {
            let l = numbers(pdf, dict, b"L")
                .filter(|l| l.len() == 4)
                .ok_or_else(unsupported)?;
            let ends: Vec<bool> = match dict.get(b"LE").ok().and_then(|o| pdf.dereference(o).ok()) {
                Some((_, Object::Array(a))) => a
                    .iter()
                    .map(|o| o.as_name().is_ok_and(|n| n.ends_with(b"Arrow")))
                    .collect(),
                _ => Vec::new(),
            };
            AnnotKind::Line {
                a: point(l[0], l[1]),
                b: point(l[2], l[3]),
                arrow: ends.get(1).copied().unwrap_or(false),
                arrow_start: ends.first().copied().unwrap_or(false),
            }
        }
        "Ink" => {
            let paths = ink(pdf, dict)
                .filter(|p| !p.is_empty())
                .ok_or_else(unsupported)?;
            AnnotKind::Ink { paths }
        }
        "Polygon" | "PolyLine" => {
            let pts = numbers(pdf, dict, b"Vertices")
                .map(|v| points(&v))
                .filter(|p| p.len() >= 2)
                .ok_or_else(unsupported)?;
            if sub == "Polygon" {
                AnnotKind::Polygon { points: pts }
            } else {
                AnnotKind::Polyline { points: pts }
            }
        }
        "Caret" => AnnotKind::Insert { at: within(0.0) },
        "Stamp" => stamp(pdf, dict, rect),
        "FileAttachment" if opts.files => attachment(pdf, dict, rect).ok_or_else(unsupported)?,
        _ => return Err(unsupported()),
    };
    // A dimension with a scale is a measurement (decision 9). Its value
    // leads its text, as Acrobat writes it; sheaf draws that itself.
    let mut contents = text(b"Contents");
    let kind = match (measured(dict), scale(pdf, dict), kind) {
        (Some(what), Some(scale), AnnotKind::Line { a, b, .. }) => {
            measure(what, vec![a, b], scale, &mut contents)
        }
        (
            Some(what),
            Some(scale),
            AnnotKind::Polyline { points } | AnnotKind::Polygon { points },
        ) => measure(what, points, scale, &mut contents),
        (_, _, kind) => kind,
    };
    let fillable = kind.fillable();
    Ok(Annot {
        id: AnnotId(0),
        kind,
        color: color.unwrap_or([0, 0, 0]),
        opacity: dict
            .get(b"CA")
            .ok()
            .and_then(|o| o.as_float().ok())
            .unwrap_or(1.0)
            .clamp(0.0, 1.0) as f32,
        stroke,
        author: text(b"T"),
        contents,
        modified: text(b"M"),
        created: text(b"CreationDate"),
        fill: color_of(pdf, dict, b"IC").filter(|_| fillable),
        dashed: border_style(pdf, dict).is_some_and(|s| s == b"D"),
        cloudy: dict
            .get(b"BE")
            .ok()
            .and_then(|o| pdf.dereference(o).ok())
            .and_then(|(_, o)| o.as_dict().ok())
            .and_then(|be| be.get(b"S").ok())
            .and_then(|s| s.as_name().ok())
            .is_some_and(|s| s == b"C"),
        turn,
        name: thread_name(dict),
    })
}

/// What a dimension annotation measures, by its `/IT`.
fn measured(dict: &Dictionary) -> Option<crate::measure::Measured> {
    use crate::measure::Measured;
    match dict.get(b"IT").ok()?.as_name().ok()? {
        b"LineDimension" => Some(Measured::Distance),
        b"PolyLineDimension" => Some(Measured::Perimeter),
        b"PolygonDimension" => Some(Measured::Area),
        _ => None,
    }
}

/// Its `/Measure` as a scale.
fn scale<P: Objects + ?Sized>(pdf: &P, dict: &Dictionary) -> Option<crate::measure::Scale> {
    let (_, m) = pdf.dereference(dict.get(b"Measure").ok()?).ok()?;
    crate::measure::scale_of(pdf, m.as_dict().ok()?)
}

/// A measurement of `points`; the value its text starts with comes off,
/// since sheaf draws and writes it from the points.
fn measure(
    what: crate::measure::Measured,
    points: Vec<Pt>,
    scale: crate::measure::Scale,
    contents: &mut String,
) -> AnnotKind {
    let first = contents.lines().next().unwrap_or_default().trim();
    if !first.is_empty() && first.ends_with(scale.unit.as_str()) {
        *contents = contents
            .split_once('\n')
            .map(|(_, rest)| rest.trim_start().to_string())
            .unwrap_or_default();
    }
    AnnotKind::Measure {
        what,
        points,
        scale: Box::new(scale),
    }
}

/// A stamp: its appearance as one of ours, in the cache, when it has one
/// sheaf can read; else a text stamp saying its name.
fn stamp<P: Objects + ?Sized>(pdf: &P, dict: &Dictionary, rect: Box2) -> AnnotKind {
    let name = dict
        .get(b"Name")
        .ok()
        .and_then(|o| o.as_name().ok())
        .map(|n| String::from_utf8_lossy(n).into_owned())
        .unwrap_or_default();
    // Acrobat's own are "SBApproved", "SHSignHere"…; custom ones "#…".
    let shown = match name.as_str() {
        n if n.starts_with('#') || n.is_empty() => "Stamp".to_string(),
        n if n.len() > 2
            && n[..2].chars().all(|c| c.is_ascii_uppercase())
            && n[2..].starts_with(|c: char| c.is_ascii_uppercase()) =>
        {
            n[2..].to_string()
        }
        n => n.to_string(),
    };
    let form = dict
        .get(b"AP")
        .ok()
        .and_then(|o| pdf.dereference(o).ok())
        .and_then(|(_, o)| o.as_dict().ok())
        .and_then(|ap| ap.get(b"N").ok())
        .and_then(|n| n.as_reference().ok());
    let stored = form
        .and_then(|f| crate::stamp_lib::form_to_pdf(pdf, f).ok())
        .and_then(|bytes| crate::generated::store(&bytes).ok());
    match stored {
        Some(path) => AnnotKind::PageStamp {
            rect,
            stamp: crate::generated::stem_of(&path),
            name: shown,
            text: None,
        },
        None => AnnotKind::TextStamp {
            rect,
            text: shown.to_uppercase(),
            status: false,
        },
    }
}

/// An attachment, its embedded file copied into the cache.
fn attachment<P: Objects + ?Sized>(pdf: &P, dict: &Dictionary, rect: Box2) -> Option<AnnotKind> {
    let deref_dict = |o: &Object| pdf.dereference(o).ok()?.1.as_dict().ok().cloned();
    let fs = deref_dict(dict.get(b"FS").ok()?)?;
    let ef = deref_dict(fs.get(b"EF").ok()?)?;
    let file = ef
        .get(b"UF")
        .or_else(|_| ef.get(b"F"))
        .ok()?
        .as_reference()
        .ok()?;
    let Ok(Object::Stream(s)) = pdf.get_object(file) else {
        return None;
    };
    let bytes = s
        .decompressed_content()
        .unwrap_or_else(|_| s.content.clone());
    let stem = crate::generated::store_blob(&bytes).ok()?;
    let name = [b"UF".as_slice(), b"F"]
        .iter()
        .find_map(|k| fs.get(k).ok().map(pdf_text).filter(|n| !n.is_empty()))
        .unwrap_or_else(|| "attachment".into());
    let icon = dict
        .get(b"Name")
        .ok()
        .and_then(|o| o.as_name().ok())
        .map(|n| String::from_utf8_lossy(n).into_owned())
        .filter(|n| crate::annot::ATTACHMENT_ICONS.contains(&n.as_str()))
        .unwrap_or_else(|| "PushPin".into());
    Some(AnnotKind::FileAttachment {
        pos: icon_at(rect),
        icon,
        file: stem,
        name,
        size: bytes.len() as u64,
    })
}

pub fn subtype(dict: &Dictionary) -> String {
    dict.get(b"Subtype")
        .ok()
        .and_then(|o| o.as_name().ok())
        .map(|n| String::from_utf8_lossy(n).into_owned())
        .unwrap_or_default()
}

/// `/RT /Group`: part of what its `/IRT` is, not a reply to it.
fn is_grouped(dict: &Dictionary) -> bool {
    dict.get(b"RT")
        .ok()
        .and_then(|o| o.as_name().ok())
        .is_some_and(|n| n == b"Group")
}

/// What `/IRT` names: the annotation it refers to, by its name, or (as
/// XFDF gives it) the name itself.
pub fn irt_name<P: Objects + ?Sized>(pdf: &P, dict: &Dictionary) -> Option<String> {
    match dict.get(b"IRT").ok()? {
        Object::Reference(id) => pdf.get_dictionary(*id).ok().map(thread_name),
        o @ Object::String(..) => Some(pdf_text(o)).filter(|n| !n.is_empty()),
        _ => None,
    }
}

/// `/InkList`'s strokes, those with points.
pub(crate) fn ink<P: Objects + ?Sized>(pdf: &P, dict: &Dictionary) -> Option<Vec<Vec<Pt>>> {
    let (_, list) = pdf.dereference(dict.get(b"InkList").ok()?).ok()?;
    Some(
        list.as_array()
            .ok()?
            .iter()
            .filter_map(|s| pdf.dereference(s).ok())
            .filter_map(|(_, s)| s.as_array().ok())
            .map(|s| points(&floats(pdf, s)))
            .filter(|p| !p.is_empty())
            .collect(),
    )
}

/// `/QuadPoints` as the box each quad bounds.
pub(crate) fn quads<P: Objects + ?Sized>(pdf: &P, dict: &Dictionary) -> Option<Vec<Box2>> {
    let q = numbers(pdf, dict, b"QuadPoints")?;
    let quads = q.as_chunks::<8>().0;
    Some(quads.iter().map(|c| bounds(&points(c))).collect())
}

/// Coordinates `[x0 y0 x1 y1 …]` as points.
pub(crate) fn points(v: &[f32]) -> Vec<Pt> {
    let pairs = v.as_chunks::<2>().0;
    pairs.iter().map(|&[x, y]| Pt { x, y }).collect()
}

fn floats<P: Objects + ?Sized>(pdf: &P, a: &[Object]) -> Vec<f32> {
    a.iter()
        .map(|o| {
            pdf.dereference(o)
                .ok()
                .and_then(|(_, o)| o.as_float().ok())
                .unwrap_or(0.0) as f32
        })
        .collect()
}

pub(crate) fn numbers<P: Objects + ?Sized>(
    pdf: &P,
    dict: &Dictionary,
    key: &[u8],
) -> Option<Vec<f32>> {
    let (_, o) = pdf.dereference(dict.get(key).ok()?).ok()?;
    Some(floats(pdf, o.as_array().ok()?))
}

/// A user-space box `[x0 y0 x1 y1]`, in either order, as a mark's.
pub(crate) fn user_box(r: [f32; 4]) -> Box2 {
    Box2::from_corners(Pt { x: r[0], y: r[1] }, Pt { x: r[2], y: r[3] })
}

fn bounds(points: &[Pt]) -> Box2 {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in points {
        (x0, y0, x1, y1) = (x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y));
    }
    Box2 {
        x: x0,
        y: y0,
        w: (x1 - x0).max(0.0),
        h: (y1 - y0).max(0.0),
    }
}

/// Where an icon's corner goes for it to sit in the middle of `rect`.
fn icon_at(rect: Box2) -> Pt {
    Pt {
        x: rect.x + (rect.w - crate::annot::NOTE_SIZE) / 2.0,
        y: rect.y + (rect.h - crate::annot::NOTE_SIZE) / 2.0,
    }
}

/// `r` with `d` off each side, but never past its middle.
pub(crate) fn inset(r: Box2, d: f32) -> Box2 {
    let d = d.min(r.w / 2.0).min(r.h / 2.0).max(0.0);
    Box2 {
        x: r.x + d,
        y: r.y + d,
        w: r.w - 2.0 * d,
        h: r.h - 2.0 * d,
    }
}

/// `/BS /W`, else `/Border`'s width, else 1.
fn border_width<P: Objects + ?Sized>(pdf: &P, dict: &Dictionary) -> f32 {
    let bs = dict
        .get(b"BS")
        .ok()
        .and_then(|o| pdf.dereference(o).ok())
        .and_then(|(_, o)| o.as_dict().ok());
    if let Some(w) = bs
        .and_then(|bs| bs.get(b"W").ok())
        .and_then(|w| w.as_float().ok())
    {
        return w as f32;
    }
    numbers(pdf, dict, b"Border")
        .and_then(|b| b.get(2).copied())
        .unwrap_or(1.0)
}

fn border_style<P: Objects + ?Sized>(pdf: &P, dict: &Dictionary) -> Option<Vec<u8>> {
    let (_, bs) = pdf.dereference(dict.get(b"BS").ok()?).ok()?;
    Some(bs.as_dict().ok()?.get(b"S").ok()?.as_name().ok()?.to_vec())
}

/// `/C` or `/IC` as RGB: gray, RGB or CMYK; none when empty.
fn color_of<P: Objects + ?Sized>(pdf: &P, dict: &Dictionary, key: &[u8]) -> Option<[u8; 3]> {
    let c = numbers(pdf, dict, key)?;
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    match c.as_slice() {
        [g] => Some([byte(*g); 3]),
        [r, g, b] => Some([byte(*r), byte(*g), byte(*b)]),
        [c, m, y, k] => Some([
            byte((1.0 - c) * (1.0 - k)),
            byte((1.0 - m) * (1.0 - k)),
            byte((1.0 - y) * (1.0 - k)),
        ]),
        _ => None,
    }
}

/// A `/DA` string's font size and fill color: `/Helv 12 Tf 1 0 0 rg`.
fn default_appearance(da: &str) -> (Option<f32>, Option<[u8; 3]>) {
    let tokens: Vec<&str> = da.split_whitespace().collect();
    let num = |i: usize| tokens.get(i).and_then(|t| t.parse::<f32>().ok());
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let (mut size, mut color) = (None, None);
    for (i, t) in tokens.iter().enumerate() {
        match *t {
            "Tf" if i >= 1 => size = num(i - 1),
            "rg" if i >= 3 => {
                if let (Some(r), Some(g), Some(b)) = (num(i - 3), num(i - 2), num(i - 1)) {
                    color = Some([byte(r), byte(g), byte(b)]);
                }
            }
            "g" if i >= 1 => color = num(i - 1).map(|v| [byte(v); 3]),
            _ => {}
        }
    }
    (size, color)
}

#[cfg(test)]
mod tests;
