//! Writing content (§13): operators with their operands written the same
//! way every time, text through a [`Font`], and resources by what they
//! are ([`Res`]), named only when the content is finished
//! ([`ContentWriter::finish`]), so the names fit the resources it goes
//! with: `/Sh` and six characters of the resource's digest, bumped past a
//! name already bound to something else (§11.5).
//!
//! Numbers have at most four decimals, trailing zeros trimmed, and never
//! an exponent or `-0`. Identical calls give identical bytes, on any
//! thread.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

use lopdf::{Dictionary, Object};

use super::font::{Font, Unencoded};
use super::names::ResKind;
use super::page::{ReadObjects, direct};
use super::res::{Res, ResId};
use crate::annot::{Box2, Pt};
use crate::geom::{Affine, Rect};

/// A color, in a device space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Color {
    Gray(f32),
    Rgb([f32; 3]),
    Cmyk([f32; 4]),
}

impl Color {
    /// An RGB color from 0–255 components.
    pub fn rgb8(c: [u8; 3]) -> Color {
        Color::Rgb(c.map(|v| f32::from(v) / 255.0))
    }
}

/// A blend mode (`/BM`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blend {
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl Blend {
    fn name(self) -> &'static str {
        match self {
            Blend::Normal => "Normal",
            Blend::Multiply => "Multiply",
            Blend::Screen => "Screen",
            Blend::Overlay => "Overlay",
            Blend::Darken => "Darken",
            Blend::Lighten => "Lighten",
            Blend::ColorDodge => "ColorDodge",
            Blend::ColorBurn => "ColorBurn",
            Blend::HardLight => "HardLight",
            Blend::SoftLight => "SoftLight",
            Blend::Difference => "Difference",
            Blend::Exclusion => "Exclusion",
            Blend::Hue => "Hue",
            Blend::Saturation => "Saturation",
            Blend::Color => "Color",
            Blend::Luminosity => "Luminosity",
        }
    }
}

/// The graphics state an ExtGState sets: opacity and blending.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GState {
    /// `/ca`, 0–1.
    pub fill_alpha: Option<f32>,
    /// `/CA`, 0–1.
    pub stroke_alpha: Option<f32>,
    pub blend: Option<Blend>,
}

impl GState {
    /// It as an ExtGState resource.
    fn res(self) -> Res {
        let mut dict = lopdf::dictionary! { "Type" => "ExtGState" };
        if let Some(a) = self.fill_alpha {
            dict.set("ca", a.clamp(0.0, 1.0));
        }
        if let Some(a) = self.stroke_alpha {
            dict.set("CA", a.clamp(0.0, 1.0));
        }
        if let Some(b) = self.blend {
            dict.set("BM", Object::Name(b.name().into()));
        }
        Res::dict(dict)
    }
}

/// How a line's ends are drawn (`J`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineCap {
    Butt,
    Round,
    Square,
}

/// How a path's corners are drawn (`j`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineJoin {
    Miter,
    Round,
    Bevel,
}

/// What ends a path: how it's painted, if it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paint {
    /// `f`. (`F`, its old synonym, is never written.)
    Fill,
    /// `f*`.
    FillEvenOdd,
    /// `S`.
    Stroke,
    /// `s`: closed, then stroked.
    CloseStroke,
    /// `B`.
    FillStroke,
    /// `B*`.
    FillStrokeEvenOdd,
    /// `b`: closed, filled and stroked.
    CloseFillStroke,
    /// `b*`.
    CloseFillStrokeEvenOdd,
    /// `n`: not painted, as after a clip.
    None,
}

impl Paint {
    fn op(self) -> &'static str {
        match self {
            Paint::Fill => "f",
            Paint::FillEvenOdd => "f*",
            Paint::Stroke => "S",
            Paint::CloseStroke => "s",
            Paint::FillStroke => "B",
            Paint::FillStrokeEvenOdd => "B*",
            Paint::CloseFillStroke => "b",
            Paint::CloseFillStrokeEvenOdd => "b*",
            Paint::None => "n",
        }
    }
}

/// Which rule says what's inside a path, for a clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillRule {
    NonZero,
    EvenOdd,
}

/// How text is drawn (`Tr`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextMode {
    Fill,
    Stroke,
    FillStroke,
    /// Neither: text to find and select, as OCR's is.
    Invisible,
    FillClip,
    StrokeClip,
    FillStrokeClip,
    Clip,
}

/// The resources content draws with, each named: what the resources it
/// goes with must bind. In the order the content first uses them, one
/// entry a resource.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResourceNeeds(pub Vec<Need>);

/// A resource content draws with, under its name in its category.
#[derive(Debug, Clone, PartialEq)]
pub struct Need {
    pub kind: ResKind,
    pub name: Vec<u8>,
    pub res: Res,
}

/// The names a resources dict already binds, which
/// [`ContentWriter::finish`] keeps clear of: each with what it binds, as
/// far as that can be told by content (a direct object that reaches no
/// other) or by number (a reference). A name bound to the very resource
/// being named is used again, not bumped, so content written again for
/// the same resources gets the same names.
#[derive(Debug, Clone, Default)]
pub struct Reserved {
    names: BTreeMap<(ResKind, Vec<u8>), Vec<ResId>>,
}

impl Reserved {
    /// The names `resources` binds.
    pub fn read(objects: &impl ReadObjects, resources: Option<&Object>) -> Reserved {
        let mut names = BTreeMap::new();
        let resources = resources
            .and_then(|r| direct(objects, r))
            .and_then(|r| r.as_dict().ok());
        for kind in ResKind::ALL {
            let Some(category) = resources
                .and_then(|r| direct(objects, r.get(kind.key()).ok()?))
                .and_then(|c| c.as_dict().ok())
            else {
                continue;
            };
            for (name, bound) in category.iter() {
                let mut ids = Vec::new();
                if let Object::Reference(id) = bound {
                    ids.push(Res::written(*id).id);
                }
                if let Some(resolved) = direct(objects, bound).filter(|o| !reaches(o)) {
                    ids.push(ResId::of(std::slice::from_ref(resolved)));
                }
                names.insert((kind, name.clone()), ids);
            }
        }
        Reserved { names }
    }

    /// Whether `name` may name `id` in `kind`.
    fn allows(&self, kind: ResKind, name: &[u8], id: ResId) -> bool {
        self.names
            .get(&(kind, name.to_vec()))
            .is_none_or(|bound| bound.contains(&id))
    }
}

/// Whether `object` refers to another.
fn reaches(object: &Object) -> bool {
    match object {
        Object::Reference(_) => true,
        Object::Array(items) => items.iter().any(reaches),
        Object::Dictionary(dict) => dict.iter().any(|(_, o)| reaches(o)),
        Object::Stream(stream) => stream.dict.iter().any(|(_, o)| reaches(o)),
        _ => false,
    }
}

/// Content being written.
#[derive(Default)]
pub struct ContentWriter<'f> {
    out: Vec<u8>,
    /// Where a resource's name goes in `out`, in order: just past its
    /// `/`, and which of `needs` it names.
    slots: Vec<(usize, usize)>,
    needs: Vec<(ResKind, Res)>,
    /// The font text draws in, and the ones `q`s saved.
    font: Option<&'f dyn Font>,
    saved: Vec<Option<&'f dyn Font>>,
}

impl<'f> ContentWriter<'f> {
    pub fn new() -> ContentWriter<'f> {
        ContentWriter::default()
    }

    /// `q`.
    pub fn save(&mut self) {
        self.saved.push(self.font);
        self.op(&[], "q");
    }

    /// `Q`. One with no `q` of this writer's before it closes one written
    /// before the content: the legacy shapes' isolating `q`.
    pub fn restore(&mut self) {
        if let Some(font) = self.saved.pop() {
            self.font = font;
        }
        self.op(&[], "Q");
    }

    /// `cm`.
    pub fn transform(&mut self, m: Affine) {
        self.op(&[m.a, m.b, m.c, m.d, m.e, m.f], "cm");
    }

    /// `g`, `rg` or `k`.
    pub fn fill_color(&mut self, color: Color) {
        self.color(color, false);
    }

    /// `G`, `RG` or `K`.
    pub fn stroke_color(&mut self, color: Color) {
        self.color(color, true);
    }

    fn color(&mut self, color: Color, stroke: bool) {
        let (values, fill_op, stroke_op): (&[f32], _, _) = match &color {
            Color::Gray(v) => (std::slice::from_ref(v), "g", "G"),
            Color::Rgb(v) => (v, "rg", "RG"),
            Color::Cmyk(v) => (v, "k", "K"),
        };
        self.op(values, if stroke { stroke_op } else { fill_op });
    }

    /// `gs`, with `state` an ExtGState resource.
    pub fn gstate(&mut self, state: GState) {
        self.name(ResKind::ExtGState, state.res());
        self.out.extend_from_slice(b" gs\n");
    }

    /// `w`.
    pub fn line_width(&mut self, width: f32) {
        self.op(&[width], "w");
    }

    /// `d`: `dashes` on and off in turn, from `phase` into them; none for
    /// a solid line.
    pub fn dash(&mut self, dashes: &[f32], phase: f32) {
        self.out.push(b'[');
        for (i, &d) in dashes.iter().enumerate() {
            if i > 0 {
                self.out.push(b' ');
            }
            number(&mut self.out, d);
        }
        self.out.extend_from_slice(b"] ");
        self.op(&[phase], "d");
    }

    /// `J`.
    pub fn line_cap(&mut self, cap: LineCap) {
        self.op(&[cap as u8 as f32], "J");
    }

    /// `j`.
    pub fn line_join(&mut self, join: LineJoin) {
        self.op(&[join as u8 as f32], "j");
    }

    /// `m`.
    pub fn move_to(&mut self, x: f32, y: f32) {
        self.op(&[x, y], "m");
    }

    /// `l`.
    pub fn line_to(&mut self, x: f32, y: f32) {
        self.op(&[x, y], "l");
    }

    /// `c`: a Bézier through control points `c1` and `c2` to `to`.
    pub fn curve_to(&mut self, c1: Pt, c2: Pt, to: Pt) {
        self.op(&[c1.x, c1.y, c2.x, c2.y, to.x, to.y], "c");
    }

    /// `re`.
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.op(&[x, y, w, h], "re");
    }

    /// `h`.
    pub fn close(&mut self) {
        self.op(&[], "h");
    }

    /// The path painted, as `paint` says, and ended.
    pub fn paint(&mut self, paint: Paint) {
        self.op(&[], paint.op());
    }

    /// `W` or `W*`: the path clips what follows, once it's ended.
    pub fn clip(&mut self, rule: FillRule) {
        let op = match rule {
            FillRule::NonZero => "W",
            FillRule::EvenOdd => "W*",
        };
        self.op(&[], op);
    }

    /// `BT`.
    pub fn begin_text(&mut self) {
        self.op(&[], "BT");
    }

    /// `ET`.
    pub fn end_text(&mut self) {
        self.op(&[], "ET");
    }

    /// `Tf`: text draws in `font` at `size`, its resource named at the
    /// end.
    pub fn font(&mut self, font: &'f dyn Font, size: f32) {
        self.font = Some(font);
        self.name(ResKind::Font, font.resource().into());
        self.out.push(b' ');
        self.op(&[size], "Tf");
    }

    /// `Tm`.
    pub fn text_matrix(&mut self, m: Affine) {
        self.op(&[m.a, m.b, m.c, m.d, m.e, m.f], "Tm");
    }

    /// `Td`.
    pub fn text_move(&mut self, x: f32, y: f32) {
        self.op(&[x, y], "Td");
    }

    /// `T*`.
    pub fn next_line(&mut self) {
        self.op(&[], "T*");
    }

    /// `Tr`.
    pub fn text_mode(&mut self, mode: TextMode) {
        self.op(&[mode as u8 as f32], "Tr");
    }

    /// `Tz`, in percent.
    pub fn horizontal_scale(&mut self, percent: f32) {
        self.op(&[percent], "Tz");
    }

    /// `TL`.
    pub fn leading(&mut self, leading: f32) {
        self.op(&[leading], "TL");
    }

    /// `Tj`: `s` in the font [`font`](Self::font) set, and the characters
    /// it couldn't encode, drawn as `?`. With no font set, nothing is
    /// drawn and every character is unencoded.
    pub fn text(&mut self, s: &str) -> Unencoded {
        let Some(font) = self.font else {
            return Unencoded(s.chars().count());
        };
        let (codes, unencoded) = font.encode(s);
        write_string(&mut self.out, &codes);
        self.out.extend_from_slice(b" Tj\n");
        unencoded
    }

    /// `Do`, as the current matrix places it.
    pub fn draw(&mut self, xobject: Res) {
        self.name(ResKind::XObject, xobject);
        self.out.extend_from_slice(b" Do\n");
    }

    /// Image `image` filling `target`.
    pub fn image(&mut self, image: Res, target: Box2) {
        self.save();
        self.transform(Affine {
            a: target.w,
            d: target.h,
            e: target.x,
            f: target.y,
            ..Affine::IDENTITY
        });
        self.draw(image);
        self.restore();
    }

    /// Form `form`, its `/BBox` `bbox` and `/Matrix` `matrix`, drawn so
    /// the box its matrix makes of its bbox fills `target`. A form whose
    /// box has no area draws nothing, and isn't drawn.
    pub fn place_form(&mut self, form: Res, bbox: Rect, matrix: Affine, target: Box2) {
        let frame = matrix.bounds(bbox);
        let (w, h) = (frame.width(), frame.height());
        let draws = w > 0.0 && h > 0.0;
        if !draws {
            return;
        }
        let (sx, sy) = (target.w / w, target.h / h);
        self.save();
        self.transform(Affine {
            a: sx,
            d: sy,
            e: target.x - frame.x0 * sx,
            f: target.y - frame.y0 * sy,
            ..Affine::IDENTITY
        });
        self.draw(form);
        self.restore();
    }

    /// `BDC`: content tagged `tag`, with property list `props`, named in
    /// the resources' `/Properties`. Lists are never written inline.
    pub fn begin_marked(&mut self, tag: &str, props: Dictionary) {
        write_name(&mut self.out, tag.as_bytes());
        self.out.push(b' ');
        self.name(ResKind::Properties, Res::dict(props));
        self.out.extend_from_slice(b" BDC\n");
    }

    /// `EMC`.
    pub fn end_marked(&mut self) {
        self.op(&[], "EMC");
    }

    /// The content, and the resources it draws with, named: `given` names
    /// a resource by its id (the legacy `/SheafHF` and `/SheafOcr`); any
    /// other is `/Sh` and its digest's first six base32 characters, the
    /// next name up where `reserved`, a name given, or one made before
    /// has it.
    pub fn finish(self, reserved: &Reserved, given: &[(ResId, &str)]) -> (Vec<u8>, ResourceNeeds) {
        let given_name = |id: ResId| {
            let (_, name) = given.iter().find(|(given, _)| *given == id)?;
            Some(name.as_bytes().to_vec())
        };
        let mut taken: BTreeSet<_> = self
            .needs
            .iter()
            .filter_map(|(kind, res)| Some((*kind, given_name(res.id)?)))
            .collect();
        let needs: Vec<Need> = self
            .needs
            .into_iter()
            .map(|(kind, res)| {
                let name =
                    given_name(res.id).unwrap_or_else(|| fresh(kind, res.id, reserved, &taken));
                taken.insert((kind, name.clone()));
                Need { kind, name, res }
            })
            .collect();
        let mut out = Vec::with_capacity(self.out.len() + 8 * self.slots.len());
        let mut at = 0;
        for &(slot, need) in &self.slots {
            out.extend_from_slice(&self.out[at..slot]);
            out.extend_from_slice(&needs[need].name);
            at = slot;
        }
        out.extend_from_slice(&self.out[at..]);
        (out, ResourceNeeds(needs))
    }

    /// `operands` and `op`, a line of its own.
    fn op(&mut self, operands: &[f32], op: &str) {
        for &n in operands {
            number(&mut self.out, n);
            self.out.push(b' ');
        }
        self.out.extend_from_slice(op.as_bytes());
        self.out.push(b'\n');
    }

    /// `/` and the place `res`'s name goes, once it has one.
    fn name(&mut self, kind: ResKind, res: Res) {
        let need = match self.needs.iter().position(|(_, r)| r.id == res.id) {
            Some(i) => i,
            None => {
                self.needs.push((kind, res));
                self.needs.len() - 1
            }
        };
        self.out.push(b'/');
        self.slots.push((self.out.len(), need));
    }
}

/// A name for `id` in `kind`: `Sh` and six base32 characters, its
/// digest's first thirty bits, counted up past any name `reserved` keeps
/// for something else or this content has given already.
fn fresh(
    kind: ResKind,
    id: ResId,
    reserved: &Reserved,
    taken: &BTreeSet<(ResKind, Vec<u8>)>,
) -> Vec<u8> {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    const BITS: u32 = 30;
    let mut n = (id.0 >> (128 - BITS)) as u32;
    loop {
        let mut name = b"Sh".to_vec();
        name.extend((0..6).rev().map(|i| ALPHABET[(n >> (5 * i)) as usize & 31]));
        if !taken.contains(&(kind, name.clone())) && reserved.allows(kind, &name, id) {
            return name;
        }
        n = (n + 1) & ((1 << BITS) - 1);
    }
}

/// `x` as an operand: at most four decimals, trailing zeros trimmed, with
/// no exponent and no `-0`. Anything but a finite number is 0.
fn number(out: &mut Vec<u8>, x: f32) {
    let x = if x.is_finite() { x } else { 0.0 };
    let start = out.len();
    let _ = write!(out, "{x:.4}");
    while out.last() == Some(&b'0') {
        out.pop();
    }
    if out.last() == Some(&b'.') {
        out.pop();
    }
    if out[start..] == *b"-0" {
        out.truncate(start);
        out.push(b'0');
    }
}

/// Codes `codes` as a string operand: literal, `(`, `)` and `\` escaped,
/// unless one is a control code (a two-byte font's codes), when it's
/// written in hex.
pub fn write_string(out: &mut Vec<u8>, codes: &[u8]) {
    if codes.iter().any(|&b| b < 0x20 || b == 0x7f) {
        out.push(b'<');
        for b in codes {
            let _ = write!(out, "{b:02X}");
        }
        out.push(b'>');
        return;
    }
    out.push(b'(');
    for &b in codes {
        if matches!(b, b'(' | b')' | b'\\') {
            out.push(b'\\');
        }
        out.push(b);
    }
    out.push(b')');
}

/// `/` and `name`, a byte that would end it written `#xx`.
fn write_name(out: &mut Vec<u8>, name: &[u8]) {
    out.push(b'/');
    for &b in name {
        let regular = b.is_ascii_graphic() && !b"#()<>[]{}/%".contains(&b);
        if regular {
            out.push(b);
        } else {
            let _ = write!(out, "#{b:02X}");
        }
    }
}
