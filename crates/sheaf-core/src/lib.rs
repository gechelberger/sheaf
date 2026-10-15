//! Shared primitive types: page identity, geometry in PDF user space, units.
//!
//! Everything here is plain data with no engine dependency, so `sheaf-doc`,
//! `sheaf-render`, and `sheaf-ui` can all speak the same vocabulary.

/// sheaf's quick checksum: xxh3-128 of `bytes`. Fast, and stable across
/// builds and platforms, so what's named by it (cache files, comments
/// without `/NM`) keeps its name. Not a security boundary.
pub fn checksum(bytes: &[u8]) -> u128 {
    twox_hash::XxHash3_128::oneshot(bytes)
}

/// [`checksum`] as 32 lowercase hex digits.
pub fn checksum_hex(bytes: &[u8]) -> String {
    format!("{:032x}", checksum(bytes))
}

#[cfg(test)]
mod checksum_tests {
    #[test]
    fn checksum_is_xxh3_128() {
        // XXH3_128bits of nothing, from the reference implementation.
        assert_eq!(super::checksum(b""), 0x99aa06d3014798d86001c324468d497f);
        assert_eq!(super::checksum_hex(b"").len(), 32);
        assert_ne!(super::checksum(b"a"), super::checksum(b"b"));
    }
}

/// Zero-based index of a page within a *source file* (what the engine sees).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PageIndex(pub u32);

use serde::{Deserialize, Serialize};

/// A size in PDF points (1/72 inch).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PageSize {
    pub width: f32,
    pub height: f32,
}

impl PageSize {
    /// The size after rotating the page by `r` (quarter turns swap axes).
    pub fn rotated(self, r: Rotation) -> Self {
        if r.is_quarter() {
            Self {
                width: self.height,
                height: self.width,
            }
        } else {
            self
        }
    }
}

/// A clockwise quarter-turn rotation, as in the `/Rotate` entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Rotation {
    #[default]
    R0,
    R90,
    R180,
    R270,
}

impl Rotation {
    pub fn from_degrees(deg: i32) -> Self {
        match deg.rem_euclid(360) {
            90 => Self::R90,
            180 => Self::R180,
            270 => Self::R270,
            _ => Self::R0,
        }
    }

    pub fn degrees(self) -> i32 {
        match self {
            Self::R0 => 0,
            Self::R90 => 90,
            Self::R180 => 180,
            Self::R270 => 270,
        }
    }

    /// Rotate a further 90° clockwise (the PDFlyer "rotate this page" action).
    pub fn cw(self) -> Self {
        Self::from_degrees(self.degrees() + 90)
    }

    pub fn ccw(self) -> Self {
        Self::from_degrees(self.degrees() - 90)
    }

    pub fn plus(self, other: Rotation) -> Self {
        Self::from_degrees(self.degrees() + other.degrees())
    }

    pub fn minus(self, other: Rotation) -> Self {
        Self::from_degrees(self.degrees() - other.degrees())
    }

    /// 90° or 270°: width and height swap.
    pub fn is_quarter(self) -> bool {
        matches!(self, Self::R90 | Self::R270)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_wraps() {
        assert_eq!(Rotation::R270.cw(), Rotation::R0);
        assert_eq!(Rotation::R0.ccw(), Rotation::R270);
        assert_eq!(Rotation::from_degrees(-90), Rotation::R270);
        assert_eq!(Rotation::R90.plus(Rotation::R270), Rotation::R0);
        assert_eq!(Rotation::R0.minus(Rotation::R90), Rotation::R270);
    }

    #[test]
    fn size_rotates() {
        let s = PageSize {
            width: 612.0,
            height: 792.0,
        };
        assert_eq!(s.rotated(Rotation::R90).width, 792.0);
        assert_eq!(s.rotated(Rotation::R180).width, 612.0);
    }
}

/// Where a bookmark lands on its page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DestView {
    /// PDF `/XYZ`: show this point at the window's top-left, at this zoom
    /// (1.0 = 100%). Coordinates are in the page's displayed space (points,
    /// top-left origin, after the file's own `/Rotate`), like annotations.
    /// `None` keeps the viewer's current value.
    Xyz {
        left: Option<f32>,
        top: Option<f32>,
        zoom: Option<f32>,
    },
    /// Any other kind (`Fit`, `FitH`, `FitR`, …), kept as the file had it:
    /// its name and its numbers in user space (`None` for null), written
    /// back unchanged.
    Other {
        kind: String,
        params: Vec<Option<f32>>,
    },
}

/// A page's geometry for mapping between user space (PDF coordinates,
/// bottom-left origin, before `/Rotate`) and displayed space (top-left
/// origin, after `/Rotate`): its visible box (x0, y0, x1, y1; the CropBox, else
/// the MediaBox) and `/Rotate`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageFrame {
    pub media: [f32; 4],
    pub rotate: i32,
}

impl PageFrame {
    /// Displayed (x, y) → user (x, y), one axis at a time, so a missing
    /// coordinate stays missing. Each displayed axis maps to exactly one
    /// user axis (which one depends on the rotation).
    pub fn to_user(&self, x: Option<f32>, y: Option<f32>) -> (Option<f32>, Option<f32>) {
        let [x0, y0, x1, y1] = self.media;
        match self.rotate.rem_euclid(360) {
            90 => (y.map(|y| x0 + y), x.map(|x| y0 + x)),
            180 => (x.map(|x| x1 - x), y.map(|y| y0 + y)),
            270 => (y.map(|y| x1 - y), x.map(|x| y1 - x)),
            _ => (x.map(|x| x0 + x), y.map(|y| y1 - y)),
        }
    }

    /// User (x, y) → displayed (x, y), one axis at a time.
    pub fn to_display(&self, ux: Option<f32>, uy: Option<f32>) -> (Option<f32>, Option<f32>) {
        let [x0, y0, x1, y1] = self.media;
        match self.rotate.rem_euclid(360) {
            90 => (uy.map(|uy| uy - y0), ux.map(|ux| ux - x0)),
            180 => (ux.map(|ux| x1 - ux), uy.map(|uy| uy - y0)),
            270 => (uy.map(|uy| y1 - uy), ux.map(|ux| x1 - ux)),
            _ => (ux.map(|ux| ux - x0), uy.map(|uy| y1 - uy)),
        }
    }
}

#[cfg(test)]
mod frame_tests {
    use super::*;

    #[test]
    fn axes_round_trip_in_every_rotation() {
        for rotate in [0, 90, 180, 270] {
            let f = PageFrame {
                media: [10.0, 20.0, 622.0, 812.0],
                rotate,
            };
            let (ux, uy) = f.to_user(Some(100.0), Some(50.0));
            assert_eq!(f.to_display(ux, uy), (Some(100.0), Some(50.0)), "{rotate}");
            // A missing coordinate stays missing on the matching axis.
            let (ux, uy) = f.to_user(None, Some(50.0));
            assert_eq!(f.to_display(ux, uy), (None, Some(50.0)), "{rotate}");
        }
        // Unrotated: displayed top 0 is the top of the MediaBox.
        let f = PageFrame {
            media: [0.0, 0.0, 612.0, 792.0],
            rotate: 0,
        };
        assert_eq!(f.to_user(Some(0.0), Some(0.0)), (Some(0.0), Some(792.0)));
    }
}
