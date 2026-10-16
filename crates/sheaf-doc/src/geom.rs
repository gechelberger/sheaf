//! A page's boxes and rotation (M10.5 §3.3), read as PDFium sizes the
//! page, so the layout's size for a page is the size the engine renders.

use sheaf_core::{PageSize, Rotation};

/// A rectangle in a page's default user space: points, y up. Normalized,
/// `x0 <= x1` and `y0 <= y1`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    /// PDFium's size for a page without a usable `/MediaBox`.
    pub const LETTER: Rect = Rect {
        x0: 0.0,
        y0: 0.0,
        x1: 612.0,
        y1: 792.0,
    };

    /// The rectangle with corners `(a, b)` and `(c, d)`, in either order.
    pub fn new(a: f32, b: f32, c: f32, d: f32) -> Rect {
        Rect {
            x0: a.min(c),
            y0: b.min(d),
            x1: a.max(c),
            y1: b.max(d),
        }
    }

    pub fn width(self) -> f32 {
        self.x1 - self.x0
    }

    pub fn height(self) -> f32 {
        self.y1 - self.y0
    }

    /// Whether it has no area.
    pub fn is_empty(self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }

    /// What `self` and `other` share, or the zero rectangle where they
    /// don't meet, as PDFium intersects them.
    pub fn intersect(self, other: Rect) -> Rect {
        let shared = Rect {
            x0: self.x0.max(other.x0),
            y0: self.y0.max(other.y0),
            x1: self.x1.min(other.x1),
            y1: self.y1.min(other.y1),
        };
        if shared.x0 > shared.x1 || shared.y0 > shared.y1 {
            Rect::default()
        } else {
            shared
        }
    }
}

/// A page's boxes and rotation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geom {
    pub media: Rect,
    /// `/CropBox`, clipped to `media`.
    pub crop: Rect,
    pub trim: Option<Rect>,
    pub bleed: Option<Rect>,
    pub art: Option<Rect>,
    /// `/Rotate`: absolute, as it's written.
    pub rotate: Rotation,
    /// `/UserUnit`, kept for writing; sizes don't apply it, as PDFium's
    /// don't.
    pub user_unit: f32,
}

impl Geom {
    /// What PDFium makes of a page's `/MediaBox`, `/CropBox` (each None
    /// when missing or not four numbers) and `/Rotate`: an empty media box
    /// is Letter; an empty crop box is the media box, any other is clipped
    /// to it, and one outside it leaves nothing shown. The rotation is
    /// whole quarter turns, `(degrees / 90) % 4`: 135 is 90, and -90 is
    /// 270.
    pub fn read(media: Option<Rect>, crop: Option<Rect>, degrees: i64) -> Geom {
        let media = media.filter(|m| !m.is_empty()).unwrap_or(Rect::LETTER);
        let crop = crop
            .filter(|c| !c.is_empty())
            .map_or(media, |c| c.intersect(media));
        let rotate = match (degrees / 90).rem_euclid(4) {
            1 => Rotation::R90,
            2 => Rotation::R180,
            3 => Rotation::R270,
            _ => Rotation::R0,
        };
        Geom {
            media,
            crop,
            trim: None,
            bleed: None,
            art: None,
            rotate,
            user_unit: 1.0,
        }
    }

    /// The box the page shows: the crop box within the media box.
    pub fn visible(&self) -> Rect {
        self.crop.intersect(self.media)
    }

    /// The page's size as shown: the visible box, turned by `/Rotate`.
    pub fn shown(&self) -> PageSize {
        let v = self.visible();
        PageSize {
            width: v.width(),
            height: v.height(),
        }
        .rotated(self.rotate)
    }
}
