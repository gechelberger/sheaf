//! Images to a PDF, one page each (`sheaf images-to-pdf`).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sheaf_doc::images::{PageFit, PageImage};
use sheaf_progress::{Progress, Unit};

/// Page size for pages made from images.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Paper {
    /// The image's own size at its resolution.
    #[default]
    Original,
    Letter,
    A4,
}

impl Paper {
    /// The fit, with `margin` points around a fitted image.
    pub fn fit(self, margin: f32) -> PageFit {
        let with = |f: PageFit| match f {
            PageFit::Paper { width, height, .. } => PageFit::Paper {
                width,
                height,
                margin,
            },
            f => f,
        };
        match self {
            Paper::Original => PageFit::Original,
            Paper::Letter => with(PageFit::LETTER),
            Paper::A4 => with(PageFit::A4),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ImagesParams {
    pub page: Paper,
    /// Resolution to use instead of what the images record.
    pub dpi: Option<f32>,
    /// Points of white around each image on a fitted page.
    pub margin: f32,
}

/// Read each image file (a TIFF may hold several pages), in order.
pub fn read_all(
    inputs: &[PathBuf],
    dpi: Option<f32>,
    progress: &dyn Progress,
) -> Result<Vec<PageImage>> {
    if inputs.is_empty() {
        bail!("no images given");
    }
    progress.set_total(Some(inputs.len() as u64), Unit::Items);
    let mut pages = Vec::new();
    for (i, path) in inputs.iter().enumerate() {
        progress.check()?;
        let mut read = sheaf_doc::images::read(path)?;
        if let Some(d) = dpi.filter(|d| *d > 0.0) {
            for p in &mut read {
                p.dpi = (d, d);
            }
        }
        pages.extend(read);
        progress.set(i as u64 + 1);
    }
    Ok(pages)
}

/// Write `out` (atomically) as a PDF of `inputs`, one page per image.
pub fn images_to_pdf(
    inputs: &[PathBuf],
    params: &ImagesParams,
    out: &Path,
    progress: &dyn Progress,
) -> Result<()> {
    let pages = read_all(inputs, params.dpi, &*progress.child("reading", 0.8))?;
    progress.check()?;
    let bytes = sheaf_doc::images::to_pdf(&pages, params.page.fit(params.margin))?;
    write_atomic(out, &bytes)
}

/// Write beside `path` and rename, so a failure leaves no half file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".part");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_from_json() {
        let p: ImagesParams = serde_json::from_str(r#"{"page":"a4","margin":36}"#).unwrap();
        assert_eq!(p.page, Paper::A4);
        assert_eq!(p.margin, 36.0);
        assert_eq!(p.dpi, None);
        assert!(matches!(
            p.page.fit(36.0),
            PageFit::Paper { margin: 36.0, .. }
        ));
    }
}
