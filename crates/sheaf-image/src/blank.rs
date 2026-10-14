//! Whether a page image is blank, by its pixels. `sheaf_ops::blank` puts
//! this together with what a PDF page's content says.
//!
//! A false "blank" deletes content, so the check is careful:
//! - a margin is ignored (scanner edge shadows, punch holes, staples);
//! - the paper level is a high percentile, so off-white and gray paper
//!   count as paper;
//! - only pixels well below the paper count as ink, so faint
//!   bleed-through from the back doesn't;
//! - ink counts only in cells with more than a speck of it, so dust
//!   doesn't;
//! - a page is blank only when both its ink and the number of cells with
//!   ink are tiny. One short line (a signature) is "nearly blank", which a
//!   UI leaves unticked.

use image::GrayImage;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Sensitivity {
    /// Only pages with next to nothing on them.
    Strict,
    #[default]
    Normal,
    /// Also pages with a little more ink (heavy scanner noise).
    Lenient,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Verdict {
    Blank {
        ink: f32,
    },
    /// A little ink in a few places: a signature, a stray line.
    NearlyBlank {
        ink: f32,
    },
    NotBlank {
        ink: f32,
    },
}

impl Verdict {
    /// Fraction of the page (inside the margin) that's ink.
    pub fn ink(self) -> f32 {
        match self {
            Verdict::Blank { ink } | Verdict::NearlyBlank { ink } | Verdict::NotBlank { ink } => {
                ink
            }
        }
    }
}

/// Share of each side ignored.
const MARGIN: f32 = 0.04;
/// Cells across and down.
const GRID: u32 = 32;
/// A cell counts as inked above this share of ink.
const CELL_FLOOR: f32 = 0.01;
/// Ink is darker than this share of the paper level.
const INK: f32 = 0.6;

struct Limits {
    /// Blank: at most this ink and this many inked cells.
    blank_ink: f32,
    blank_cells: usize,
    /// Nearly blank: at most this much.
    nearly_ink: f32,
    nearly_cells: usize,
}

impl Sensitivity {
    fn limits(self) -> Limits {
        match self {
            Sensitivity::Strict => Limits {
                blank_ink: 0.0002,
                blank_cells: 1,
                nearly_ink: 0.002,
                nearly_cells: 8,
            },
            Sensitivity::Normal => Limits {
                blank_ink: 0.0005,
                blank_cells: 2,
                nearly_ink: 0.003,
                nearly_cells: 16,
            },
            Sensitivity::Lenient => Limits {
                blank_ink: 0.0015,
                blank_cells: 6,
                nearly_ink: 0.006,
                nearly_cells: 28,
            },
        }
    }
}

/// Judge a page image of about 60 dpi (see [`judge_image`] for any size).
pub fn is_blank(gray: &GrayImage, sensitivity: Sensitivity) -> Verdict {
    let (w, h) = gray.dimensions();
    let (mx, my) = ((w as f32 * MARGIN) as u32, (h as f32 * MARGIN) as u32);
    let (x0, y0, x1, y1) = (mx, my, w.saturating_sub(mx), h.saturating_sub(my));
    if x1 <= x0 + GRID || y1 <= y0 + GRID {
        return Verdict::NotBlank { ink: 1.0 };
    }
    // The paper: the 90th percentile of brightness.
    let mut hist = [0u64; 256];
    for y in y0..y1 {
        for x in x0..x1 {
            hist[gray.get_pixel(x, y)[0] as usize] += 1;
        }
    }
    let total: u64 = hist.iter().sum();
    let mut seen = 0;
    let mut paper = 255u32;
    for (v, n) in hist.iter().enumerate() {
        seen += n;
        if seen * 10 >= total * 9 {
            paper = v as u32;
            break;
        }
    }
    // A dark page (a photo, a black separator) isn't blank paper.
    if paper < 90 {
        return Verdict::NotBlank { ink: 1.0 };
    }
    let cut = (paper as f32 * INK) as u8;
    let (iw, ih) = (x1 - x0, y1 - y0);
    let mut cells = vec![(0u32, 0u32); (GRID * GRID) as usize];
    for y in y0..y1 {
        let cy = ((y - y0) * GRID / ih).min(GRID - 1);
        for x in x0..x1 {
            let cx = ((x - x0) * GRID / iw).min(GRID - 1);
            let c = &mut cells[(cy * GRID + cx) as usize];
            c.1 += 1;
            if gray.get_pixel(x, y)[0] < cut {
                c.0 += 1;
            }
        }
    }
    let mut ink_px = 0u64;
    let mut inked = 0usize;
    for &(ink, n) in &cells {
        if n > 0 && ink as f32 / n as f32 > CELL_FLOOR {
            ink_px += ink as u64;
            inked += 1;
        }
    }
    let ink = ink_px as f32 / total.max(1) as f32;
    let l = sensitivity.limits();
    if ink <= l.blank_ink && inked <= l.blank_cells {
        Verdict::Blank { ink }
    } else if ink <= l.nearly_ink && inked <= l.nearly_cells {
        Verdict::NearlyBlank { ink }
    } else {
        Verdict::NotBlank { ink }
    }
}

/// [`is_blank`] on a page image of any size: shrunk to about 60 dpi
/// first, as `sheaf_ops::blank::find` renders, so a resolution doesn't change the verdict.
pub fn judge_image(image: &image::DynamicImage, sensitivity: Sensitivity) -> Verdict {
    // A Letter page's long side at 60 dpi.
    const LONG: u32 = 660;
    let (w, h) = (image.width(), image.height());
    let gray = if w.max(h) > LONG {
        let s = LONG as f32 / w.max(h) as f32;
        image
            .resize_exact(
                ((w as f32 * s).round() as u32).max(1),
                ((h as f32 * s).round() as u32).max(1),
                image::imageops::FilterType::Triangle,
            )
            .to_luma8()
    } else {
        image.to_luma8()
    };
    is_blank(&gray, sensitivity)
}

#[cfg(test)]
mod pixel_tests {
    use super::*;
    use image::Luma;

    /// A Letter page at 60 dpi.
    fn paper(v: u8) -> GrayImage {
        GrayImage::from_pixel(510, 660, Luma([v]))
    }

    fn rect(g: &mut GrayImage, x: u32, y: u32, w: u32, h: u32, v: u8) {
        for yy in y..y + h {
            for xx in x..x + w {
                g.put_pixel(xx, yy, Luma([v]));
            }
        }
    }

    fn noise(g: &mut GrayImage, amp: i32) {
        let mut s = 9u32;
        for p in g.pixels_mut() {
            s = s.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let d = ((s >> 16) % (2 * amp as u32 + 1)) as i32 - amp;
            p[0] = (p[0] as i32 + d).clamp(0, 255) as u8;
        }
    }

    fn blank(g: &GrayImage) -> bool {
        matches!(is_blank(g, Sensitivity::Normal), Verdict::Blank { .. })
    }

    #[test]
    fn white_and_off_white_with_noise() {
        assert!(blank(&paper(255)));
        let mut g = paper(215);
        noise(&mut g, 8);
        assert!(blank(&g));
    }

    #[test]
    fn bleed_through_is_paper() {
        // Text from the back, faint: 85% of the paper level.
        let mut g = paper(230);
        for line in 0..30 {
            rect(&mut g, 60, 60 + line * 18, 380, 6, 196);
        }
        assert!(blank(&g));
    }

    #[test]
    fn dark_scanner_border_and_dust() {
        let mut g = paper(245);
        // A black edge inside the margin, and punch holes.
        rect(&mut g, 0, 0, 12, 660, 10);
        rect(&mut g, 4, 100, 14, 14, 20);
        // Specks.
        for (x, y) in [(100, 200), (300, 420), (250, 90), (400, 600)] {
            rect(&mut g, x, y, 1, 1, 30);
        }
        assert!(blank(&g));
    }

    #[test]
    fn a_signature_is_nearly_blank() {
        let mut g = paper(250);
        rect(&mut g, 300, 520, 120, 3, 20);
        assert!(matches!(
            is_blank(&g, Sensitivity::Normal),
            Verdict::NearlyBlank { .. }
        ));
    }

    #[test]
    fn text_is_not_blank() {
        let mut g = paper(250);
        // One line of words.
        for word in 0..8 {
            rect(&mut g, 60 + word * 48, 80, 38, 7, 30);
        }
        assert!(!blank(&g));
        // A full page.
        let mut g = paper(250);
        for line in 0..35 {
            for word in 0..8 {
                rect(&mut g, 60 + word * 48, 60 + line * 16, 38, 7, 30);
            }
        }
        assert!(matches!(
            is_blank(&g, Sensitivity::Lenient),
            Verdict::NotBlank { .. }
        ));
        // A dark page isn't blank paper.
        assert!(!blank(&paper(30)));
    }
}
