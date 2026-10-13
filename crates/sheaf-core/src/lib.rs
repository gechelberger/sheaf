//! Shared primitive types: page identity, geometry in PDF user space, units.
//!
//! Everything here is plain data with no engine dependency, so `sheaf-doc`,
//! `sheaf-render`, and `sheaf-ui` can all speak the same vocabulary.

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
