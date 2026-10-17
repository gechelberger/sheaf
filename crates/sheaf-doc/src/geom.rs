//! A page's boxes and rotation (M10.5 §3.3), read as PDFium sizes the
//! page, so the layout's size for a page is the size the engine renders,
//! and the one conversion between the page's default user space
//! (PageSpace, §5) and the page as it's shown.

use sheaf_core::{PageSize, Rotation};

use crate::annot::{Box2, Pt};

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

/// An affine map of the plane, in PDF's order: `x' = a·x + c·y + e`,
/// `y' = b·x + d·y + f`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affine {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

impl Affine {
    pub const IDENTITY: Affine = Affine {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    /// PageSpace into the frame, y down, that reads upright when the page
    /// is turned `r`, with no translation: [`Geom::to_view`]'s linear part
    /// for a page rotated `r`.
    pub fn upright(r: Rotation) -> Affine {
        let (a, b, c, d) = match r {
            Rotation::R0 => (1.0, 0.0, 0.0, -1.0),
            Rotation::R90 => (0.0, 1.0, 1.0, 0.0),
            Rotation::R180 => (-1.0, 0.0, 0.0, 1.0),
            Rotation::R270 => (0.0, -1.0, -1.0, 0.0),
        };
        Affine {
            a,
            b,
            c,
            d,
            ..Affine::IDENTITY
        }
    }

    pub fn apply(self, p: Pt) -> Pt {
        Pt {
            x: self.a * p.x + self.c * p.y + self.e,
            y: self.b * p.x + self.d * p.y + self.f,
        }
    }

    /// The box with the images of `r`'s opposite corners as its corners.
    /// A quarter-turn map, as a page's frame is, keeps a box's edges on
    /// the axes, so this is its image.
    pub fn apply_box(self, r: Box2) -> Box2 {
        let near = self.apply(Pt { x: r.x, y: r.y });
        let far = self.apply(Pt {
            x: r.right(),
            y: r.bottom(),
        });
        Box2::from_corners(near, far)
    }

    /// The smallest rectangle that holds the image of `r`: its four
    /// corners' images, bounded. A form's box on the page is its
    /// `/Matrix`'s bounds of its `/BBox`.
    pub fn bounds(self, r: Rect) -> Rect {
        let corners = [(r.x0, r.y0), (r.x1, r.y0), (r.x0, r.y1), (r.x1, r.y1)]
            .map(|(x, y)| self.apply(Pt { x, y }));
        let low = |f: fn(&Pt) -> f32| corners.iter().map(f).fold(f32::INFINITY, f32::min);
        let high = |f: fn(&Pt) -> f32| corners.iter().map(f).fold(f32::NEG_INFINITY, f32::max);
        Rect {
            x0: low(|p| p.x),
            y0: low(|p| p.y),
            x1: high(|p| p.x),
            y1: high(|p| p.y),
        }
    }

    /// A point some of whose coordinates may be missing, as an `/XYZ`'s
    /// are: under a quarter-turn map each coordinate of the image comes
    /// from one of the point's, so it's missing where that one is.
    pub fn apply_axes(self, x: Option<f32>, y: Option<f32>) -> (Option<f32>, Option<f32>) {
        debug_assert!(
            self.a * self.c == 0.0 && self.b * self.d == 0.0,
            "not a quarter-turn map: {self:?}"
        );
        let along = |k: f32, from: Option<f32>, offset: f32| from.map(|v| k * v + offset);
        let x2 = if self.a != 0.0 {
            along(self.a, x, self.e)
        } else {
            along(self.c, y, self.e)
        };
        let y2 = if self.d != 0.0 {
            along(self.d, y, self.f)
        } else {
            along(self.b, x, self.f)
        };
        (x2, y2)
    }

    /// `self`, then `next`.
    pub fn then(self, next: Affine) -> Affine {
        Affine {
            a: self.a * next.a + self.b * next.c,
            b: self.a * next.b + self.b * next.d,
            c: self.c * next.a + self.d * next.c,
            d: self.c * next.b + self.d * next.d,
            e: self.e * next.a + self.f * next.c + next.e,
            f: self.e * next.b + self.f * next.d + next.f,
        }
    }

    /// The map that undoes this one. A page's frames are invertible.
    pub fn inverse(self) -> Affine {
        let det = self.a * self.d - self.b * self.c;
        Affine {
            a: self.d / det,
            b: -self.b / det,
            c: -self.c / det,
            d: self.a / det,
            e: (self.c * self.f - self.d * self.e) / det,
            f: (self.b * self.e - self.a * self.f) / det,
        }
    }
}

/// What PDFium makes of a `/Rotate` of `degrees`: whole quarter turns,
/// `(degrees / 90) % 4`, so 135 is 90 and -90 is 270.
pub fn quarter_turns(degrees: i64) -> Rotation {
    match (degrees / 90).rem_euclid(4) {
        1 => Rotation::R90,
        2 => Rotation::R180,
        3 => Rotation::R270,
        _ => Rotation::R0,
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
    /// [`quarter_turns`].
    pub fn read(media: Option<Rect>, crop: Option<Rect>, degrees: i64) -> Geom {
        let media = media.filter(|m| !m.is_empty()).unwrap_or(Rect::LETTER);
        let crop = crop
            .filter(|c| !c.is_empty())
            .map_or(media, |c| c.intersect(media));
        Geom {
            media,
            crop,
            trim: None,
            bleed: None,
            art: None,
            rotate: quarter_turns(degrees),
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

    /// PageSpace to the page as shown: points, y down, from the top-left
    /// corner of the visible box as it shows after `/Rotate`, so the
    /// visible box covers `(0, 0)` to [`shown`](Self::shown). It's PDFium's
    /// page-to-device mapping at one pixel a point.
    pub fn to_view(&self) -> Affine {
        let v = self.visible();
        let turn = Affine::upright(self.rotate);
        let shown = turn.apply_box(Box2 {
            x: v.x0,
            y: v.y0,
            w: v.width(),
            h: v.height(),
        });
        turn.then(Affine {
            e: -shown.x,
            f: -shown.y,
            ..Affine::IDENTITY
        })
    }

    /// The page as shown to PageSpace: [`to_view`](Self::to_view) undone.
    pub fn from_view(&self) -> Affine {
        self.to_view().inverse()
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const ROTATIONS: [Rotation; 4] = [Rotation::R0, Rotation::R90, Rotation::R180, Rotation::R270];

    /// Within 1/16 pt.
    fn near(a: Pt, b: Pt) -> bool {
        (a.x - b.x).abs() <= 1.0 / 16.0 && (a.y - b.y).abs() <= 1.0 / 16.0
    }

    /// Where each corner of the visible box lands as the page is shown,
    /// per rotation: the convention itself, which a round trip alone
    /// wouldn't catch (the identity passes it).
    #[test]
    fn visible_corners_land_on_the_shown_page() {
        let media = Rect::new(-37.0, 52.0, 575.0, 844.0);
        let crop = Rect::new(-17.0, 62.0, 555.0, 824.0);
        let (x0, y0, x1, y1) = (crop.x0, crop.y0, crop.x1, crop.y1);
        let (w, h) = (crop.width(), crop.height());
        // The visible box's bottom-left, top-left, top-right and
        // bottom-right in user space, and where each shows.
        let table = [
            (0, [(0.0, h), (0.0, 0.0), (w, 0.0), (w, h)]),
            (90, [(0.0, 0.0), (h, 0.0), (h, w), (0.0, w)]),
            (180, [(w, 0.0), (w, h), (0.0, h), (0.0, 0.0)]),
            (270, [(h, w), (0.0, w), (0.0, 0.0), (h, 0.0)]),
        ];
        let corners = [(x0, y0), (x0, y1), (x1, y1), (x1, y0)];
        for (degrees, shown) in table {
            let g = Geom::read(Some(media), Some(crop), degrees);
            let to_view = g.to_view();
            for (&(x, y), &(vx, vy)) in corners.iter().zip(&shown) {
                let got = to_view.apply(Pt { x, y });
                assert!(near(got, Pt { x: vx, y: vy }), "{degrees}: {got:?}");
            }
            let size = g.shown();
            let all = Box2 { x: x0, y: y0, w, h };
            let got = to_view.apply_box(all);
            let want = Box2 {
                x: 0.0,
                y: 0.0,
                w: size.width,
                h: size.height,
            };
            assert_eq!(got, want, "{degrees}");
        }
    }

    /// An `/XYZ` with only a top keeps only the shown coordinate that
    /// comes from it: on a /Rotate 90 page that's the left.
    #[test]
    fn a_missing_axis_stays_missing() {
        let g = Geom::read(None, None, 90);
        assert_eq!(
            g.to_view().apply_axes(None, Some(700.0)),
            (Some(700.0), None)
        );
        assert_eq!(
            g.to_view().apply_axes(Some(100.0), None),
            (None, Some(100.0))
        );
        let upright = Geom::read(None, None, 0);
        assert_eq!(
            upright.to_view().apply_axes(None, Some(700.0)),
            (None, Some(92.0))
        );
        assert_eq!(upright.to_view().apply_axes(None, None), (None, None));
    }

    /// A box's image under a turn or a shear is bounded by its corners'.
    #[test]
    fn bounds_hold_every_corner() {
        let r = Rect::new(0.0, 0.0, 100.0, 50.0);
        let turn = Affine {
            a: 0.0,
            b: 1.0,
            c: -1.0,
            d: 0.0,
            e: 10.0,
            f: 20.0,
        };
        assert_eq!(turn.bounds(r), Rect::new(-40.0, 20.0, 10.0, 120.0));
        let shear = Affine {
            c: 1.0,
            ..Affine::IDENTITY
        };
        assert_eq!(shear.bounds(r), Rect::new(0.0, 0.0, 150.0, 50.0));
    }

    fn geoms() -> impl Strategy<Value = Geom> {
        (
            0..4usize,
            0..4usize,
            (-5000.0f32..5000.0, -5000.0f32..5000.0),
            (1.0f32..14400.0, 1.0f32..14400.0),
            (0.0f32..1.0, 0.0f32..1.0, 0.0f32..1.0, 0.0f32..1.0),
        )
            .prop_map(|(rotate, view, (x, y), (w, h), (l, b, r, t))| {
                let media = Rect::new(x, y, x + w, y + h);
                // A crop box inside it, inset from each side by a part of
                // what's left.
                let (left, bottom) = (x + l * w / 2.0, y + b * h / 2.0);
                let crop = Rect::new(
                    left,
                    bottom,
                    left + (x + w - left) * (1.0 - r / 2.0),
                    bottom + (y + h - bottom) * (1.0 - t / 2.0),
                );
                let g = Geom::read(Some(media), Some(crop), rotate as i64 * 90);
                // What a pane with rotate view shows.
                Geom {
                    rotate: g.rotate.plus(ROTATIONS[view]),
                    ..g
                }
            })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        /// Over every `/Rotate` and view rotation, with boxes off the
        /// origin, `to_view` and `from_view` undo each other within
        /// 1/16 pt, both ways.
        #[test]
        fn to_view_and_from_view_undo_each_other(
            g in geoms(),
            (x, y) in (-20000.0f32..20000.0, -20000.0f32..20000.0),
        ) {
            let p = Pt { x, y };
            let (to, from) = (g.to_view(), g.from_view());
            let there = from.apply(to.apply(p));
            prop_assert!(near(there, p), "{g:?}: {p:?} came back {there:?}");
            let back = to.apply(from.apply(p));
            prop_assert!(near(back, p), "{g:?}: {p:?} came back {back:?}");
        }
    }
}
