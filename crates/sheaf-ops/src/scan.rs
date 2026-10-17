//! Photos and scans of paper to a clean PDF (`sheaf scan`, File > Create >
//! From Scans/Photos): each image is cleaned up (`sheaf_image::clean`), becomes a
//! page, and can be OCRed so the text is searchable. OCR reads the very
//! image that becomes the page, so the text lines up with it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sheaf_core::PageSize;
use sheaf_doc::images::placement;
use sheaf_doc::{Box2, Document, Edit, SourceInfo, TextLayer};
use sheaf_image::clean::ScanParams;
use sheaf_image::io::PageImage;
use sheaf_ocr::{OcrConfig, OcrImage, OcrWord};
use sheaf_progress::{Progress, Unit};

use crate::images::Paper;

/// Color and gray pages are stored as JPEG at this quality.
const JPEG_QUALITY: u8 = 80;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ScanOptions {
    #[serde(flatten)]
    pub clean: ScanParams,
    pub page: Paper,
    /// Output resolution.
    pub dpi: u32,
    /// Recognize text with the configured OCR engine.
    pub ocr: bool,
    /// Leave out pages that come out blank (duplex backs, separators).
    /// Only clearly blank ones: a nearly blank page stays.
    pub drop_blanks: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            clean: ScanParams::default(),
            page: Paper::Letter,
            dpi: 300,
            ocr: false,
            drop_blanks: false,
        }
    }
}

impl ScanOptions {
    /// The cleanup parameters, with the pixel size the paper and
    /// resolution allow.
    pub fn params(&self) -> ScanParams {
        let inches = match self.page {
            Paper::Letter => Some((8.5, 11.0)),
            Paper::A4 => Some((8.27, 11.69)),
            Paper::Original => None,
        };
        ScanParams {
            fit: inches.map(|(w, h): (f32, f32)| {
                (
                    (w * self.dpi as f32).round() as u32,
                    (h * self.dpi as f32).round() as u32,
                )
            }),
            ..self.clean.clone()
        }
    }
}

/// What a scan did, page by page.
#[derive(Debug, Default)]
pub struct ScanReport {
    pub pages: usize,
    /// Pages found in the photo and cut out.
    pub cropped: usize,
    /// Words recognized, when OCR ran.
    pub words: usize,
    /// Blank pages left out.
    pub dropped: usize,
}

/// Clean up one photo.
pub fn clean_one(
    path: &Path,
    params: &ScanParams,
    progress: &dyn Progress,
) -> Result<sheaf_image::clean::Cleaned> {
    let image = sheaf_image::io::load(path)?;
    clean_image(image, params, progress, &mut |_, _| {})
        .with_context(|| format!("cleaning up {}", path.display()))
}

/// Clean up an image, with the orientation model when it's there,
/// showing `stage` each step.
pub fn clean_image(
    image: image::RgbImage,
    params: &ScanParams,
    progress: &dyn Progress,
    stage: &mut dyn FnMut(sheaf_image::clean::Stage, &image::RgbImage),
) -> Result<sheaf_image::clean::Cleaned> {
    let orienter = |img: &image::RgbImage| orientation_votes(img);
    let unwarper = |img: &image::RgbImage| sheaf_ocr::rectify::unwarp(img);
    let models = sheaf_image::clean::Models {
        orient: (params.orient
            && (sheaf_ocr::orient::available() || sheaf_ocr::orient::osd_available()))
        .then_some(&orienter as sheaf_image::clean::Orienter),
        unwarp: (params.flatten_curl && sheaf_ocr::rectify::available())
            .then_some(&unwarper as sheaf_image::clean::Unwarper),
    };
    if params.flatten_curl && models.unwarp.is_none() {
        anyhow::bail!(
            "can't flatten curled pages: {}",
            sheaf_ocr::rectify::unavailable_reason().unwrap_or_default()
        );
    }
    sheaf_image::clean::clean_with(image, params, models, progress, stage)
}

/// Votes for which way up a page goes, from the models there are:
/// PaddleOCR's orientation classifier, averaged over the page turned all
/// four ways (on its own it has favourite answers), and Tesseract's
/// orientation detection on the gray page and a black-and-white copy,
/// each weighted by its confidence. On ten phone photos of engineering
/// drawings neither was right alone; together they were.
pub fn orientation_votes(img: &image::RgbImage) -> Option<[f32; 4]> {
    let mut votes = [0f32; 4];
    let mut any = false;
    if sheaf_ocr::orient::available() {
        let s = (1000.0 / img.width().max(img.height()) as f32).min(1.0);
        let small = image::imageops::resize(
            img,
            ((img.width() as f32 * s) as u32).max(1),
            ((img.height() as f32 * s) as u32).max(1),
            image::imageops::FilterType::Triangle,
        );
        if let Some(sc) = sheaf_ocr::orient::scores(&small) {
            votes = sc;
            any = true;
        }
    }
    if sheaf_ocr::orient::osd_available() {
        let gray = image::DynamicImage::ImageRgb8(img.clone()).into_luma8();
        let bw = sheaf_image::clean::sauvola(&gray, (gray.width() / 50).max(15) | 1, 0.25);
        for im in [&gray, &bw] {
            if let Some((k, conf)) = sheaf_ocr::orient::osd(im, 300) {
                votes[k as usize] += (conf / 8.0).clamp(0.0, 1.0);
                any = true;
            }
        }
    }
    any.then_some(votes)
}

/// Clean up `inputs` and write them to `out` as a PDF, a page each.
/// `ocr` is the engine setup to use when `opts.ocr` is on.
pub fn scan(
    inputs: &[PathBuf],
    opts: &ScanOptions,
    ocr: &OcrConfig,
    out: &Path,
    progress: &dyn Progress,
) -> Result<ScanReport> {
    if inputs.is_empty() {
        anyhow::bail!("no images given");
    }
    let mut engine = if opts.ocr {
        sheaf_ocr::check_engine(ocr.engine, ocr)?;
        Some(sheaf_ocr::build_engine(ocr.engine, ocr)?)
    } else {
        None
    };
    let engine_name = engine
        .as_ref()
        .map(|e| e.describe().name)
        .unwrap_or_default();
    let params = opts.params();
    let fit = opts.page.fit(0.0);
    let mut report = ScanReport::default();
    let mut pages = Vec::with_capacity(inputs.len());
    let mut words: Vec<Vec<sheaf_doc::OcrWord>> = Vec::new();
    progress.set_total(Some(inputs.len() as u64), Unit::Pages);
    for (i, path) in inputs.iter().enumerate() {
        progress.check()?;
        let share = 1.0 / inputs.len() as f32;
        let step = progress.child(&format!("page {}", i + 1), share);
        let cleaned = clean_one(path, &params, &*step.child("cleaning", 0.6))?;
        if cleaned.quad.is_some() {
            report.cropped += 1;
        }
        if cleaned.turns != 0 {
            step.message(
                sheaf_progress::Level::Info,
                &format!("{}: turned {}°", path.display(), cleaned.turns * 90),
            );
        }
        if opts.drop_blanks && is_blank_page(&cleaned.image) {
            step.message(
                sheaf_progress::Level::Info,
                &format!("{}: blank, left out", path.display()),
            );
            report.dropped += 1;
            progress.set(i as u64 + 1);
            continue;
        }
        let page = match opts.clean.mode {
            // Black and white goes in as CCITT G4.
            sheaf_image::clean::Mode::Bw => {
                PageImage::from_image(cleaned.image.clone(), opts.dpi as f32)
            }
            _ => PageImage::jpeg(&cleaned.image, opts.dpi as f32, JPEG_QUALITY)?,
        };
        if let Some(engine) = engine.as_mut() {
            step.check()?;
            let img = match engine.input() {
                sheaf_ocr::InputKind::Gray => OcrImage::Gray(cleaned.image.to_luma8()),
                sheaf_ocr::InputKind::Rgb => OcrImage::Rgb(cleaned.image.to_rgb8()),
            };
            let found = engine
                .recognize(img, opts.dpi)
                .with_context(|| format!("recognizing {}", path.display()))?;
            report.words += found.len();
            words.push(to_page(&found, &page, fit));
        }
        pages.push(page);
        report.pages += 1;
        progress.set(i as u64 + 1);
    }
    progress.check()?;
    if pages.is_empty() {
        anyhow::bail!("every page came out blank");
    }
    let bytes = sheaf_doc::images::to_pdf(&pages, fit)?;
    if engine.is_none() {
        crate::images::write_atomic(out, &bytes)?;
        return Ok(report);
    }
    // Add the text through the document model, as the app's OCR does.
    let mut tmp = out.as_os_str().to_owned();
    tmp.push(".images.part");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, &bytes).with_context(|| format!("writing {}", tmp.display()))?;
    let result = add_text(&tmp, &pages, fit, words, &engine_name, out);
    let _ = std::fs::remove_file(&tmp);
    result?;
    Ok(report)
}

/// Whether a cleaned page is blank enough to leave out.
pub fn is_blank_page(image: &image::DynamicImage) -> bool {
    matches!(
        sheaf_image::blank::judge_image(image, sheaf_image::blank::Sensitivity::Normal),
        sheaf_image::blank::Verdict::Blank { .. }
    )
}

/// An engine's words on a cleaned image, in points on its page.
fn to_page(
    found: &[OcrWord],
    page: &PageImage,
    fit: sheaf_doc::images::PageFit,
) -> Vec<sheaf_doc::OcrWord> {
    let place = placement(page, fit).image;
    let sx = place.w / page.width as f32;
    let sy = place.h / page.height as f32;
    found
        .iter()
        .map(|w| {
            let [l, t, r, b] = w.bbox_px;
            sheaf_doc::OcrWord {
                text: w.text.clone(),
                rect: Box2 {
                    x: place.x + l.min(r) * sx,
                    y: place.y + t.min(b) * sy,
                    w: (r - l).abs() * sx,
                    h: (b - t).abs() * sy,
                },
                confidence: w.confidence,
            }
        })
        .collect()
}

/// Open the image PDF at `src`, give each page its text layer, and write
/// the result to `out`.
fn add_text(
    src: &Path,
    pages: &[PageImage],
    fit: sheaf_doc::images::PageFit,
    words: Vec<Vec<sheaf_doc::OcrWord>>,
    engine: &str,
    out: &Path,
) -> Result<()> {
    let sizes: Vec<PageSize> = pages
        .iter()
        .map(|p| {
            let (width, height) = placement(p, fit).page;
            PageSize { width, height }
        })
        .collect();
    let info = SourceInfo {
        path: src.to_path_buf(),
        page_count: pages.len() as u32,
        first_page: sizes[0],
        page_sizes: Some(sizes.clone()),
        bytes: std::fs::metadata(src)
            .map(|m| m.len() as usize)
            .unwrap_or(0),
        password: None,
        data: None,
        derived: None,
    };
    let mut doc = Document::new(info, &[], sheaf_doc::history::default_policy());
    let ids: Vec<_> = doc.state().pages.iter().map(|p| p.id).collect();
    let layers = ids
        .into_iter()
        .zip(words)
        .zip(sizes)
        .map(|((id, w), size)| {
            let layer = TextLayer::from_engine(w, size, sheaf_core::Rotation::R0, engine);
            (id, Arc::new(layer))
        })
        .collect();
    doc.apply(Edit::SetTextLayers { layers }, "Recognize Text")?;
    sheaf_doc::save::rewrite_to_path(&doc.snapshot(), out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letter_at_300_dpi_fits_2550_by_3300() {
        let opts = ScanOptions::default();
        assert_eq!(opts.params().fit, Some((2550, 3300)));
        let opts: ScanOptions =
            serde_json::from_str(r#"{"page":"original","mode":"bw","ocr":true}"#).unwrap();
        assert_eq!(opts.params().fit, None);
        assert_eq!(opts.clean.mode, sheaf_image::clean::Mode::Bw);
        assert!(opts.ocr && opts.clean.deskew);
    }
}
