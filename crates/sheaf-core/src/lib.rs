//! Shared primitive types: page identity, geometry in PDF user space, units.
//!
//! Everything here is plain data with no engine dependency, so `sheaf-doc`,
//! `sheaf-render`, and `sheaf-ui` can all speak the same vocabulary.

/// Zero-based index of a page within a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PageIndex(pub u32);

/// A size in PDF points (1/72 inch).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageSize {
    pub width: f32,
    pub height: f32,
}

/// Page rotation as stored in the `/Rotate` entry, clockwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_wraps() {
        assert_eq!(Rotation::R270.cw(), Rotation::R0);
        assert_eq!(Rotation::from_degrees(-90), Rotation::R270);
    }
}
