//! Find blank pages (Document > Find Blank Pages…, `sheaf remove-blanks`):
//! duplex backs, scanner fillers, empty separators.
//!
//! Content is checked before pixels, since it's cheap and certain:
//! - a page with sheaf's marks, other annotations, links or form fields
//!   isn't blank;
//! - a page with text isn't either, unless the text says it's
//!   intentionally blank ("This page intentionally left blank", perhaps
//!   with a page number or header).
//!
//! The rest are rendered at about 60 dpi and judged by their pixels
//! ([`is_blank`], which the scan pipeline also uses on cleaned photos).
//! A false "blank" deletes content, so the pixel check is careful:
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

use std::collections::BTreeMap;
use std::sync::LazyLock;

use anyhow::{Context, Result};
use image::GrayImage;
use serde::{Deserialize, Serialize};
use sheaf_core::{PageIndex, Rotation};
use sheaf_doc::{PageId, Snapshot, SourceId};
use sheaf_progress::{Progress, Unit};
use sheaf_render::pdfium::PdfiumEngine;
use sheaf_render::{Job, PixelFormat, Renderer};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reason {
    Blank,
    /// A little ink in a few places: worth a look, not deleted by default.
    NearlyBlank,
    /// Says it's intentionally left blank.
    IntentionallyBlank,
}

impl Reason {
    pub fn label(self) -> &'static str {
        match self {
            Reason::Blank => "blank",
            Reason::NearlyBlank => "nearly blank",
            Reason::IntentionallyBlank => "intentionally blank",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    /// 0-based position in the document.
    pub index: usize,
    pub id: PageId,
    /// Share of the page that's ink (0 for an intentionally blank one).
    pub ink: f32,
    pub reason: Reason,
}

impl Candidate {
    /// Whether it's deleted unless someone says otherwise.
    pub fn drop_by_default(&self) -> bool {
        self.reason != Reason::NearlyBlank
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct BlankParams {
    pub sensitivity: Sensitivity,
    /// Keep pages that say they're intentionally blank.
    pub keep_intentional: bool,
}

static INTENTIONAL: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)(this\s+page\s+(is\s+|has\s+been\s+|was\s+)?)?(intentionally|deliberately|purposely|purposefully)\s+(left\s+)?blank|(this\s+)?page\s+left\s+blank",
    )
    .expect("valid")
});

/// What a page's text says about it.
#[derive(Debug, PartialEq)]
enum Says {
    /// No real text: the pixels decide.
    Nothing,
    IntentionallyBlank,
    Content,
}

fn text_says(text: &str) -> Says {
    let letters = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).count();
    if letters(text) < 3 {
        return Says::Nothing;
    }
    if INTENTIONAL.is_match(text) {
        // Room for a page number, a header or a footer, not a paragraph.
        let rest = INTENTIONAL.replace_all(text, "");
        if letters(&rest) <= 60 {
            return Says::IntentionallyBlank;
        }
    }
    Says::Content
}

/// The blank pages among `indexes` (0-based; none: every page).
pub fn find(
    snap: &Snapshot,
    indexes: Option<&[usize]>,
    params: BlankParams,
    progress: &dyn Progress,
) -> Result<Vec<Candidate>> {
    let pages = &snap.state.pages;
    let wanted: Vec<usize> = match indexes {
        Some(ix) => ix.iter().copied().filter(|&i| i < pages.len()).collect(),
        None => (0..pages.len()).collect(),
    };
    progress.set_total(Some(wanted.len() as u64), Unit::Pages);
    let mut by_source: BTreeMap<SourceId, Vec<usize>> = BTreeMap::new();
    for &i in &wanted {
        by_source.entry(pages[i].source).or_default().push(i);
    }
    let mut out = Vec::new();
    let mut done = 0;
    for (sid, list) in by_source {
        let info = snap.source(sid).context("unknown source")?;
        let engine = PdfiumEngine::open(&info.path, info.password.as_deref())?;
        let file = sheaf_doc::file::load_pdf(&info.path, info.password.as_deref())?;
        let objs: Vec<_> = file.get_pages().into_values().collect();
        for i in list {
            progress.check()?;
            done += 1;
            progress.set(done);
            let p = pages[i];
            // sheaf's marks, others' annotations, links, fields.
            if snap.state.annots_on(p.id).is_some_and(|a| !a.is_empty()) {
                continue;
            }
            let has_annots = objs
                .get(p.index as usize)
                .and_then(|&o| file.get_dictionary(o).ok())
                .and_then(|d| d.get(b"Annots").ok())
                .and_then(|a| file.dereference(a).ok())
                .and_then(|(_, a)| a.as_array().ok().map(|a| !a.is_empty()))
                .unwrap_or(false);
            if has_annots {
                continue;
            }
            // Text, from the file and any unsaved OCR.
            let mut text = engine.page_text(PageIndex(p.index)).unwrap_or_default();
            if let Some(layer) = snap.state.text_layers.get(&p.id) {
                text.push(' ');
                text.push_str(&layer.text());
            }
            match text_says(&text) {
                Says::IntentionallyBlank => {
                    if !params.keep_intentional {
                        out.push(Candidate {
                            index: i,
                            id: p.id,
                            ink: 0.0,
                            reason: Reason::IntentionallyBlank,
                        });
                    }
                    continue;
                }
                Says::Content => continue,
                Says::Nothing => {}
            }
            let size = info.page_size(p.index);
            let raster = engine.render(
                PageIndex(p.index),
                Job::Page {
                    width: (size.width * 60.0 / 72.0).round().max(16.0) as u32,
                    rotation: Rotation::R0,
                },
                PixelFormat::Rgba,
            )?;
            let gray = image::GrayImage::from_fn(raster.width, raster.height, |x, y| {
                let o = ((y * raster.width + x) * 4) as usize;
                let d = &raster.data[o..o + 3];
                image::Luma([
                    ((77 * d[0] as u32 + 150 * d[1] as u32 + 29 * d[2] as u32) >> 8) as u8,
                ])
            });
            let (reason, ink) = match is_blank(&gray, params.sensitivity) {
                Verdict::Blank { ink } => (Reason::Blank, ink),
                Verdict::NearlyBlank { ink } => (Reason::NearlyBlank, ink),
                Verdict::NotBlank { .. } => continue,
            };
            out.push(Candidate {
                index: i,
                id: p.id,
                ink,
                reason,
            });
        }
    }
    out.sort_by_key(|c| c.index);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intentionally_blank_phrases() {
        let says = |t: &str| text_says(t);
        assert_eq!(
            says("This page intentionally left blank"),
            Says::IntentionallyBlank
        );
        assert_eq!(
            says("Form 1040 (2025)   Page 4\nTHIS PAGE HAS BEEN INTENTIONALLY LEFT BLANK"),
            Says::IntentionallyBlank
        );
        assert_eq!(says("Intentionally blank"), Says::IntentionallyBlank);
        assert_eq!(says("  . , - "), Says::Nothing);
        // A paragraph that mentions it is content.
        let long = "Schedule B instructions: if a line does not apply, leave it blank. \
                    Lines intentionally left blank are not errors and need no explanation \
                    from the preparer or the taxpayer when filing.";
        assert_eq!(says(long), Says::Content);
        assert_eq!(says("Invoice 1234 total 56.00"), Says::Content);
    }
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
