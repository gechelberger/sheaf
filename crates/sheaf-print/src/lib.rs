//! Printing: the system's printers and their paper, where a page lands on
//! the sheet, and handing a print-ready PDF to the spooler. See
//! `todos/m7.md` for why sheaf draws its own dialog.
//!
//! - Linux and macOS: CUPS, through `lpstat`, `lpoptions` and `lp`.
//! - Windows: GDI, with PDFium rasterizing each page at the printer's
//!   resolution.
//!
//! The PDF a job prints is made by the caller (`sheaf_doc::save::print_pdf`)
//! at the page's own size; scaling and orientation are the backend's job,
//! done the way [`place`] says so the preview matches the paper.

use std::path::Path;

use anyhow::Result;

#[cfg(unix)]
mod cups;
#[cfg(windows)]
mod gdi;

/// A size of paper, portrait, in points.
#[derive(Debug, Clone, PartialEq)]
pub struct Paper {
    /// What the backend calls it: a CUPS media name (`Letter`,
    /// `iso_a4_210x297mm`) or a Windows paper number.
    pub id: String,
    /// What to show: "Letter", "A4".
    pub label: String,
    pub width: f32,
    pub height: f32,
}

impl Paper {
    pub fn letter() -> Paper {
        Paper {
            id: "Letter".into(),
            label: "Letter".into(),
            width: 612.0,
            height: 792.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Printer {
    /// The name the system knows it by.
    pub name: String,
    pub is_default: bool,
    /// Paper it takes; the first is used when the default isn't known.
    pub papers: Vec<Paper>,
    /// Index into `papers` of its default.
    pub default_paper: usize,
}

/// Acrobat's Page Sizing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sizing {
    /// Scale every page up or down to the paper.
    Fit,
    /// 100%, centred; what's off the paper is cut off.
    ActualSize,
    /// Pages bigger than the paper shrink to fit; the rest print at 100%.
    #[default]
    ShrinkOversized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Orientation {
    /// Each sheet turned to match its page.
    #[default]
    Auto,
    Portrait,
    Landscape,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Duplex {
    #[default]
    Off,
    /// Flip on the long edge (book style).
    LongEdge,
    /// Flip on the short edge (tablet style).
    ShortEdge,
}

/// Everything about a job except the pages.
#[derive(Debug, Clone, PartialEq)]
pub struct JobSettings {
    pub printer: String,
    /// Shown in the print queue.
    pub title: String,
    pub copies: u32,
    pub collate: bool,
    pub paper: Paper,
    pub sizing: Sizing,
    pub orientation: Orientation,
    pub duplex: Duplex,
}

/// Where a page goes on its sheet, in points from the sheet's top-left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// The sheet as it comes out: paper turned for landscape.
    pub sheet_w: f32,
    pub sheet_h: f32,
    pub landscape: bool,
    pub scale: f32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// How a `page_w`×`page_h` page (as shown, points) lands on `paper`.
pub fn place(
    page_w: f32,
    page_h: f32,
    paper: &Paper,
    sizing: Sizing,
    orientation: Orientation,
) -> Placement {
    let (pw, ph) = (page_w.max(1.0), page_h.max(1.0));
    let landscape = match orientation {
        Orientation::Auto => pw > ph,
        Orientation::Portrait => false,
        Orientation::Landscape => true,
    };
    let (short, long) = (paper.width.min(paper.height), paper.width.max(paper.height));
    let (sheet_w, sheet_h) = if landscape {
        (long, short)
    } else {
        (short, long)
    };
    let fit = (sheet_w / pw).min(sheet_h / ph);
    let scale = match sizing {
        Sizing::Fit => fit,
        Sizing::ActualSize => 1.0,
        Sizing::ShrinkOversized => fit.min(1.0),
    };
    let (w, h) = (pw * scale, ph * scale);
    Placement {
        sheet_w,
        sheet_h,
        landscape,
        scale,
        x: (sheet_w - w) / 2.0,
        y: (sheet_h - h) / 2.0,
        w,
        h,
    }
}

/// The printers the system offers, the default first.
pub fn printers() -> Result<Vec<Printer>> {
    #[cfg(unix)]
    let mut list = cups::printers()?;
    #[cfg(windows)]
    let mut list = gdi::printers()?;
    #[cfg(not(any(unix, windows)))]
    let mut list: Vec<Printer> = Vec::new();
    list.sort_by_key(|p| !p.is_default);
    Ok(list)
}

/// Send the PDF at `pdf` to the printer. Blocks until the spooler has it;
/// returns what it said (a CUPS request id, for one). The file may be
/// deleted afterwards.
pub fn submit(job: &JobSettings, pdf: &Path) -> Result<String> {
    #[cfg(unix)]
    return cups::submit(job, pdf);
    #[cfg(windows)]
    return gdi::submit(job, pdf);
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (job, pdf);
        anyhow::bail!("printing isn't supported on this system")
    }
}

/// Common paper by name, in points: the sizes `lpoptions` and Windows
/// drivers mostly offer under these names.
pub fn known_paper(name: &str) -> Option<(f32, f32)> {
    let n = name.to_ascii_lowercase();
    let n = n
        .trim_end_matches(".fullbleed")
        .trim_end_matches(".borderless");
    Some(match n {
        "letter" | "na_letter" | "us letter" => (612.0, 792.0),
        "legal" | "na_legal" | "us legal" => (612.0, 1008.0),
        "tabloid" | "ledger" | "11x17" | "na_ledger" => (792.0, 1224.0),
        "executive" | "na_executive" => (522.0, 756.0),
        "statement" | "na_invoice" => (396.0, 612.0),
        "a3" | "iso_a3" => (842.0, 1191.0),
        "a4" | "iso_a4" => (595.0, 842.0),
        "a5" | "iso_a5" => (420.0, 595.0),
        "a6" | "iso_a6" => (297.0, 420.0),
        "b4" | "jis_b4" => (729.0, 1032.0),
        "b5" | "jis_b5" => (516.0, 729.0),
        "env10" | "com10" | "na_number-10" => (297.0, 684.0),
        "envdl" | "dl" | "iso_dl" => (312.0, 624.0),
        "envc5" | "iso_c5" => (459.0, 649.0),
        _ => return None,
    })
}

/// IPP media names carry their size: `na_letter_8.5x11in`,
/// `iso_a4_210x297mm`, `custom_100x200mm`.
pub fn ipp_media_size(name: &str) -> Option<(f32, f32)> {
    let last = name.rsplit('_').next()?;
    let (dims, per) = match last.strip_suffix("in") {
        Some(d) => (d, 72.0),
        None => (last.strip_suffix("mm")?, 72.0 / 25.4),
    };
    let (w, h) = dims.split_once('x')?;
    let (w, h): (f32, f32) = (w.parse().ok()?, h.parse().ok()?);
    (w > 0.0 && h > 0.0).then_some((w * per, h * per))
}

/// A friendlier name for a paper id: "na_letter_8.5x11in" → "Letter".
pub fn paper_label(id: &str) -> String {
    // Drop a borderless variant's suffix (`Letter.Fullbleed`), not a
    // decimal in a size (`8.5x11in`).
    let base = match id.rsplit_once('.') {
        Some((head, tail))
            if tail.chars().all(|c| c.is_ascii_alphabetic())
                && !tail.ends_with("in")
                && !tail.ends_with("mm") =>
        {
            head
        }
        _ => id,
    };
    let mut parts: Vec<&str> = base.split('_').collect();
    if parts.len() >= 3 && ipp_media_size(base).is_some() {
        // class_name_size: keep the name.
        parts = vec![parts[1]];
    }
    let name = parts.join(" ");
    match name.to_ascii_lowercase().as_str() {
        "letter" => "Letter".into(),
        "legal" => "Legal".into(),
        "ledger" | "tabloid" => "Tabloid".into(),
        "executive" => "Executive".into(),
        "a3" | "a4" | "a5" | "a6" | "b4" | "b5" => name.to_ascii_uppercase(),
        _ => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LETTER: (f32, f32) = (612.0, 792.0);

    #[test]
    fn fit_scales_both_ways() {
        let paper = Paper::letter();
        // A half-letter page grows to fill the sheet.
        let p = place(306.0, 396.0, &paper, Sizing::Fit, Orientation::Portrait);
        assert!((p.scale - 2.0).abs() < 1e-3);
        assert!((p.x).abs() < 1e-3 && (p.y).abs() < 1e-3);
        // A tabloid page shrinks.
        let p = place(792.0, 1224.0, &paper, Sizing::Fit, Orientation::Portrait);
        assert!(p.scale < 0.8);
    }

    #[test]
    fn shrink_only_shrinks_and_actual_size_centres() {
        let paper = Paper::letter();
        let p = place(
            306.0,
            396.0,
            &paper,
            Sizing::ShrinkOversized,
            Orientation::Portrait,
        );
        assert_eq!(p.scale, 1.0);
        assert!((p.x - 153.0).abs() < 1e-3);
        let big = place(
            792.0,
            1224.0,
            &paper,
            Sizing::ShrinkOversized,
            Orientation::Portrait,
        );
        assert!(big.scale < 1.0);
        let actual = place(
            792.0,
            1224.0,
            &paper,
            Sizing::ActualSize,
            Orientation::Portrait,
        );
        assert_eq!(actual.scale, 1.0);
        // Hangs off both sides equally.
        assert!((actual.x + 90.0).abs() < 1e-3);
    }

    #[test]
    fn auto_orientation_turns_the_sheet_for_wide_pages() {
        let paper = Paper::letter();
        let p = place(LETTER.1, LETTER.0, &paper, Sizing::Fit, Orientation::Auto);
        assert!(p.landscape);
        assert_eq!((p.sheet_w, p.sheet_h), (792.0, 612.0));
        assert!((p.scale - 1.0).abs() < 1e-3);
        let p = place(
            LETTER.1,
            LETTER.0,
            &paper,
            Sizing::Fit,
            Orientation::Portrait,
        );
        assert!(!p.landscape);
        assert!(p.scale < 0.8);
    }

    #[test]
    fn paper_names_and_sizes() {
        assert_eq!(known_paper("A4"), Some((595.0, 842.0)));
        assert_eq!(known_paper("Letter.Fullbleed"), Some(LETTER));
        let (w, h) = ipp_media_size("na_letter_8.5x11in").unwrap();
        assert!((w - 612.0).abs() < 0.01 && (h - 792.0).abs() < 0.01);
        let (w, _) = ipp_media_size("iso_a4_210x297mm").unwrap();
        assert!((w - 595.3).abs() < 0.1);
        assert_eq!(ipp_media_size("Letter"), None);
        assert_eq!(paper_label("na_letter_8.5x11in"), "Letter");
        assert_eq!(paper_label("iso_a4_210x297mm"), "A4");
        assert_eq!(paper_label("Legal"), "Legal");
    }
}
