//! Measurements (M9 slice 19): distance, perimeter and area on a page at a
//! scale ("1 in = 10 ft"), as Acrobat's Measure tools make them. Written as
//! standard dimension annotations (decision 9): a Line, PolyLine or
//! Polygon with `/IT` `/LineDimension`, `/PolyLineDimension` or
//! `/PolygonDimension` and a `/Measure` dictionary, so Acrobat reads them
//! as measurements.

use lopdf::{Dictionary, Object, dictionary};
use serde::{Deserialize, Serialize};

use crate::annot::Pt;
use crate::attachments::Objects;

/// What a measurement measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Measured {
    /// Between two points.
    Distance,
    /// Along an open path.
    Perimeter,
    /// Inside a closed shape.
    Area,
}

impl Measured {
    pub fn label(self) -> &'static str {
        match self {
            Measured::Distance => "Distance",
            Measured::Perimeter => "Perimeter",
            Measured::Area => "Area",
        }
    }

    /// `/IT` as the PDF has it.
    pub fn intent(self) -> &'static [u8] {
        match self {
            Measured::Distance => b"LineDimension",
            Measured::Perimeter => b"PolyLineDimension",
            Measured::Area => b"PolygonDimension",
        }
    }
}

/// Lengths on the page, and what they stand for.
pub const PAGE_UNITS: [&str; 4] = ["in", "mm", "cm", "pt"];
/// Real-world units.
pub const REAL_UNITS: [&str; 9] = ["in", "ft", "yd", "mi", "mm", "cm", "m", "km", "pt"];

/// Points in one of a unit.
pub fn points_per(unit: &str) -> Option<f64> {
    Some(match unit {
        "pt" => 1.0,
        "in" => 72.0,
        "ft" => 72.0 * 12.0,
        "yd" => 72.0 * 36.0,
        "mi" => 72.0 * 63_360.0,
        "mm" => 72.0 / 25.4,
        "cm" => 72.0 / 2.54,
        "m" => 72.0 / 0.0254,
        "km" => 72.0 / 0.0000254,
        _ => return None,
    })
}

/// A scale: `page` `page_unit` on the page stands for `real` `unit`, and
/// values show `precision` decimals.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scale {
    pub page: f64,
    pub page_unit: String,
    pub real: f64,
    pub unit: String,
    pub precision: u8,
}

impl Default for Scale {
    /// Actual size, in inches.
    fn default() -> Self {
        Scale {
            page: 1.0,
            page_unit: "in".into(),
            real: 1.0,
            unit: "in".into(),
            precision: 2,
        }
    }
}

impl Scale {
    /// Real units for each point on the page.
    pub fn per_point(&self) -> f64 {
        let page_pt = points_per(&self.page_unit).unwrap_or(72.0) * self.page;
        if page_pt <= 0.0 {
            return 0.0;
        }
        self.real / page_pt
    }

    /// "1 in = 10 ft".
    pub fn ratio(&self) -> String {
        format!(
            "{} {} = {} {}",
            trim(self.page),
            self.page_unit,
            trim(self.real),
            self.unit
        )
    }

    /// Calibrated: a line `points` long on the page is `real` `unit`. Kept
    /// as so much per inch of page.
    pub fn calibrated(points: f64, real: f64, unit: &str, precision: u8) -> Scale {
        let per_point = if points > 0.0 { real / points } else { 1.0 };
        Scale {
            page: 1.0,
            page_unit: "in".into(),
            real: per_point * 72.0,
            unit: unit.into(),
            precision,
        }
    }

    /// `v` with the scale's decimals and `unit`: "12.50 ft".
    pub fn show(&self, v: f64, unit: &str) -> String {
        let p = self.precision.min(6) as usize;
        format!("{v:.p$} {unit}")
    }
}

/// A unit as typed ("feet", "\"", "metres") as one of ours.
pub fn unit_named(word: &str) -> Option<&'static str> {
    Some(match word.trim().to_lowercase().as_str() {
        "pt" | "pts" | "point" | "points" => "pt",
        "in" | "inch" | "inches" | "\"" => "in",
        "ft" | "foot" | "feet" | "'" => "ft",
        "yd" | "yard" | "yards" => "yd",
        "mi" | "mile" | "miles" => "mi",
        "mm" | "millimeter" | "millimeters" | "millimetre" | "millimetres" => "mm",
        "cm" | "centimeter" | "centimeters" | "centimetre" | "centimetres" => "cm",
        "m" | "meter" | "meters" | "metre" | "metres" => "m",
        "km" | "kilometer" | "kilometers" | "kilometre" | "kilometres" => "km",
        _ => return None,
    })
}

/// "10 ft", "1/8 in", "1 1/2 in", "2.5m": a length and its unit (none
/// when not given).
pub fn parse_length(text: &str) -> Option<(f64, Option<&'static str>)> {
    let t = text.trim();
    // The unit is what follows the number, spaced or not.
    let split = t
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_alphabetic() || *c == '"' || *c == '\'')
        .last()
        .map(|(i, _)| i)
        .unwrap_or(t.len());
    let (number, unit) = t.split_at(split);
    let unit = match unit.trim() {
        "" => None,
        u => Some(unit_named(u)?),
    };
    let fraction = |s: &str| -> Option<f64> {
        match s.split_once('/') {
            Some((a, b)) => {
                let b: f64 = b.trim().parse().ok()?;
                (b != 0.0).then_some(a.trim().parse::<f64>().ok()? / b)
            }
            None => s.trim().parse().ok(),
        }
    };
    let parts: Vec<&str> = number.split_whitespace().collect();
    let v = match parts.as_slice() {
        [one] => fraction(one)?,
        [whole, frac] => whole.parse::<f64>().ok()? + fraction(frac)?,
        _ => return None,
    };
    (v.is_finite() && v > 0.0).then_some((v, unit))
}

/// A scale as typed: "1 in = 10 ft", "1/8 in = 1 ft", or "1:100" (the
/// same unit both sides: `unit`).
pub fn parse_scale(text: &str, unit: &str, precision: u8) -> Option<Scale> {
    let (page, real) = text.split_once(['=', ':'])?;
    let (page, page_unit) = parse_length(page)?;
    let (real, real_unit) = parse_length(real)?;
    let real_unit = real_unit.or(page_unit).unwrap_or(unit);
    let page_unit = page_unit.filter(|u| PAGE_UNITS.contains(u)).unwrap_or(
        if PAGE_UNITS.contains(&real_unit) {
            real_unit
        } else {
            "in"
        },
    );
    Some(Scale {
        page,
        page_unit: page_unit.into(),
        real,
        unit: real_unit.into(),
        precision,
    })
}

/// A number without trailing zeros: 10, 0.25.
fn trim(v: f64) -> String {
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() { "0".into() } else { s.into() }
}

/// The length of the path through `points`, closed back to the first when
/// `closed`, in points.
pub fn length(points: &[Pt], closed: bool) -> f64 {
    let seg = |a: &Pt, b: &Pt| ((b.x - a.x) as f64).hypot((b.y - a.y) as f64);
    let open: f64 = points.windows(2).map(|w| seg(&w[0], &w[1])).sum();
    match (closed, points.first(), points.last()) {
        (true, Some(a), Some(b)) if points.len() > 2 => open + seg(b, a),
        _ => open,
    }
}

/// The area inside `points` (the shoelace), in square points.
pub fn area(points: &[Pt]) -> f64 {
    if points.len() < 3 {
        return 0.0;
    }
    let n = points.len();
    let twice: f64 = (0..n)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            a.x as f64 * b.y as f64 - b.x as f64 * a.y as f64
        })
        .sum();
    twice.abs() / 2.0
}

/// What `points` measure, in the scale's units (squared for an area).
pub fn value(what: Measured, points: &[Pt], scale: &Scale) -> f64 {
    let k = scale.per_point();
    match what {
        Measured::Distance | Measured::Perimeter => length(points, false) * k,
        Measured::Area => area(points) * k * k,
    }
}

/// "12.50 ft", or "150.00 sq ft".
pub fn label(what: Measured, points: &[Pt], scale: &Scale) -> String {
    let v = value(what, points, scale);
    match what {
        Measured::Area => scale.show(v, &format!("sq {}", scale.unit)),
        _ => scale.show(v, &scale.unit),
    }
}

/// The point a drawn label goes by: a distance's middle, a path's end, an
/// area's centre.
pub fn label_at(what: Measured, points: &[Pt]) -> Option<Pt> {
    let first = *points.first()?;
    Some(match what {
        Measured::Distance => {
            let last = *points.last()?;
            Pt {
                x: (first.x + last.x) / 2.0,
                y: (first.y + last.y) / 2.0,
            }
        }
        Measured::Perimeter => *points.last()?,
        Measured::Area => {
            let n = points.len() as f32;
            Pt {
                x: points.iter().map(|p| p.x).sum::<f32>() / n,
                y: points.iter().map(|p| p.y).sum::<f32>() / n,
            }
        }
    })
}

/// `p` moved to the nearest 45° around `from` (Shift while drawing).
pub fn at_45(from: Pt, p: Pt) -> Pt {
    let (dx, dy) = (p.x - from.x, p.y - from.y);
    let len = (dx * dx + dy * dy).sqrt();
    let step = std::f32::consts::FRAC_PI_4;
    let ang = (dy.atan2(dx) / step).round() * step;
    Pt {
        x: from.x + len * ang.cos(),
        y: from.y + len * ang.sin(),
    }
}

/// The `/Measure` dictionary for `scale`: rectilinear, a number format for
/// x (points to the unit), and for distance and area.
pub fn measure_dict(scale: &Scale) -> Dictionary {
    let denominator = 10i64.pow(scale.precision.min(6) as u32);
    let format = |unit: &str, c: f64| {
        dictionary! {
            "Type" => "NumberFormat",
            "U" => Object::string_literal(unit),
            "C" => Object::Real(c),
            "F" => "D",
            "D" => denominator,
        }
    };
    dictionary! {
        "Type" => "Measure",
        "Subtype" => "RL",
        "R" => Object::string_literal(scale.ratio()),
        "X" => vec![Object::Dictionary(format(&scale.unit, scale.per_point()))],
        "D" => vec![Object::Dictionary(format(&scale.unit, 1.0))],
        "A" => vec![Object::Dictionary(format(&format!("sq {}", scale.unit), 1.0))],
    }
}

/// A scale from a `/Measure` dictionary: its x number format's unit and
/// conversion (`/C`, real units per point), and decimals (`/D`).
pub fn scale_of<P: Objects + ?Sized>(pdf: &P, measure: &Dictionary) -> Option<Scale> {
    let deref = |o: &Object| match o {
        Object::Reference(id) => pdf.get_object(*id).ok().cloned(),
        o => Some(o.clone()),
    };
    let x = deref(measure.get(b"X").ok()?)?;
    let first = deref(x.as_array().ok()?.first()?)?;
    let f = first.as_dict().ok()?;
    let unit = f
        .get(b"U")
        .ok()
        .map(crate::file::pdf_text)
        .unwrap_or_else(|| "pt".into());
    let c = f.get(b"C").ok()?.as_float().ok()?;
    let precision = f
        .get(b"D")
        .ok()
        .and_then(|d| d.as_i64().ok())
        .filter(|d| *d > 0)
        .map(|d| (d as f64).log10().round() as u8)
        .unwrap_or(2);
    Some(Scale {
        page: 1.0,
        page_unit: "in".into(),
        real: c * 72.0,
        unit,
        precision,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::Document as Pdf;

    fn p(x: f32, y: f32) -> Pt {
        Pt { x, y }
    }

    #[test]
    fn scales_convert_points_to_real_units() {
        let s = Scale {
            page: 1.0,
            page_unit: "in".into(),
            real: 10.0,
            unit: "ft".into(),
            precision: 2,
        };
        assert_eq!(s.ratio(), "1 in = 10 ft");
        // Two inches on the page are twenty feet.
        assert_eq!(
            label(Measured::Distance, &[p(0.0, 0.0), p(144.0, 0.0)], &s),
            "20.00 ft"
        );
        // A one-inch square is a hundred square feet.
        let square = [p(0.0, 0.0), p(72.0, 0.0), p(72.0, 72.0), p(0.0, 72.0)];
        assert_eq!(label(Measured::Area, &square, &s), "100.00 sq ft");
        assert!((length(&square, true) - 288.0).abs() < 1e-9);
        let mm = Scale {
            page: 1.0,
            page_unit: "mm".into(),
            real: 100.0,
            unit: "mm".into(),
            precision: 0,
        };
        assert_eq!(
            label(
                Measured::Perimeter,
                &[p(0.0, 0.0), p(72.0 / 25.4, 0.0)],
                &mm
            ),
            "100 mm"
        );
    }

    #[test]
    fn calibration_takes_a_known_length() {
        // A 180-point line is 25 feet.
        let s = Scale::calibrated(180.0, 25.0, "ft", 1);
        assert_eq!(
            label(Measured::Distance, &[p(0.0, 0.0), p(0.0, 360.0)], &s),
            "50.0 ft"
        );
        assert_eq!(s.ratio(), "1 in = 10 ft");
    }

    #[test]
    fn the_measure_dictionary_reads_back() {
        let s = Scale {
            page: 0.5,
            page_unit: "in".into(),
            real: 3.0,
            unit: "m".into(),
            precision: 3,
        };
        let pdf = Pdf::with_version("1.7");
        let back = scale_of(&pdf, &measure_dict(&s)).unwrap();
        assert!((back.per_point() - s.per_point()).abs() < 1e-6);
        assert_eq!((back.unit.as_str(), back.precision), ("m", 3));
    }

    #[test]
    fn scales_and_lengths_read_as_typed() {
        let s = parse_scale("1/8 in = 1 ft", "ft", 2).unwrap();
        assert_eq!(
            (s.page, s.page_unit.as_str(), s.real, s.unit.as_str()),
            (0.125, "in", 1.0, "ft")
        );
        assert!((s.per_point() - 1.0 / 9.0).abs() < 1e-9);
        let s = parse_scale("1:100", "m", 2).unwrap();
        assert_eq!((s.page_unit.as_str(), s.unit.as_str()), ("in", "m"));
        let s = parse_scale("1 cm = 2.5 metres", "ft", 1).unwrap();
        assert_eq!(
            (s.page_unit.as_str(), s.real, s.unit.as_str()),
            ("cm", 2.5, "m")
        );
        assert_eq!(parse_length("1 1/2in"), Some((1.5, Some("in"))));
        assert_eq!(parse_length("25 feet"), Some((25.0, Some("ft"))));
        assert_eq!(parse_length("12"), Some((12.0, None)));
        assert_eq!(parse_length("ten ft"), None);
        assert_eq!(parse_scale("1 in", "ft", 2), None);
    }

    #[test]
    fn shift_keeps_to_45_degrees() {
        let q = at_45(p(0.0, 0.0), p(10.0, 1.0));
        assert!((q.y).abs() < 1e-4 && (q.x - 101f32.sqrt()).abs() < 1e-4);
        let q = at_45(p(0.0, 0.0), p(10.0, 9.0));
        assert!((q.x - q.y).abs() < 1e-4);
    }
}
