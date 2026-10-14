//! The engine interface and the registry of compiled-in engines.

use image::{GrayImage, RgbImage};
use serde::{Deserialize, Serialize};
use sheaf_image::raster::Raster;

use crate::config::OcrConfig;

/// One recognized word, in pixel coordinates of the image it came from
/// (top-left origin).
#[derive(Debug, Clone, PartialEq)]
pub struct OcrWord {
    pub text: String,
    /// left, top, right, bottom.
    pub bbox_px: [f32; 4],
    /// 0 to 1.
    pub confidence: f32,
}

/// What an engine wants to be fed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    Gray,
    Rgb,
}

/// A page image in the engine's preferred format.
#[derive(Debug, Clone)]
pub enum OcrImage {
    Gray(GrayImage),
    Rgb(RgbImage),
}

impl OcrImage {
    /// Convert a rendered page. The 4-byte raster is dropped by the caller
    /// right after, so a 300 dpi Letter page costs 8 MB (gray) instead of
    /// 34 MB while it waits for an engine.
    pub fn from_raster(r: &Raster, kind: InputKind) -> Option<Self> {
        match kind {
            InputKind::Gray => r.to_gray().map(Self::Gray),
            InputKind::Rgb => r.to_rgb().map(Self::Rgb),
        }
    }

    pub fn width(&self) -> u32 {
        match self {
            Self::Gray(i) => i.width(),
            Self::Rgb(i) => i.width(),
        }
    }

    pub fn height(&self) -> u32 {
        match self {
            Self::Gray(i) => i.height(),
            Self::Rgb(i) => i.height(),
        }
    }
}

/// The engines sheaf knows about, compiled in or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineKind {
    #[default]
    Tesseract,
    Oar,
}

impl EngineKind {
    pub const ALL: [EngineKind; 2] = [EngineKind::Tesseract, EngineKind::Oar];

    /// Stable id, as written to settings.
    pub fn id(self) -> &'static str {
        match self {
            Self::Tesseract => "tesseract",
            Self::Oar => "oar",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Tesseract => "Tesseract",
            Self::Oar => "PaddleOCR (oar-ocr)",
        }
    }

    pub fn compiled(self) -> bool {
        match self {
            Self::Tesseract => cfg!(feature = "tesseract"),
            Self::Oar => cfg!(feature = "oar"),
        }
    }
}

/// What a built engine says about itself.
#[derive(Debug, Clone, PartialEq)]
pub struct EngineInfo {
    pub kind: EngineKind,
    /// Engine and version, e.g. "tesseract 5.5.2".
    pub name: String,
    /// Languages or model sets it can use with the current data.
    pub languages: Vec<String>,
}

/// An OCR engine instance. One per worker thread; not shared.
pub trait OcrEngine: Send {
    fn input(&self) -> InputKind;
    /// Recognize `img`, rendered at `dpi`. Words come back in pixel
    /// coordinates of `img`.
    fn recognize(&mut self, img: OcrImage, dpi: u32) -> anyhow::Result<Vec<OcrWord>>;
    fn describe(&self) -> EngineInfo;
}

/// Why an engine can't be used. `Display` is what the user sees.
#[derive(Debug, Clone, thiserror::Error)]
pub enum EngineError {
    #[error("{0} is not compiled into this build")]
    NotCompiled(&'static str),
    #[error("{0}")]
    MissingData(String),
    #[error("{0}")]
    Init(String),
}

/// The engines compiled into this build.
pub fn engines() -> Vec<EngineKind> {
    EngineKind::ALL
        .into_iter()
        .filter(|k| k.compiled())
        .collect()
}

/// Cheap check that `kind` could be built with `cfg`: compiled in, its
/// data files present, the CPU able to run it. No engine is loaded.
pub fn check_engine(kind: EngineKind, cfg: &OcrConfig) -> Result<(), EngineError> {
    match kind {
        EngineKind::Tesseract => {
            #[cfg(feature = "tesseract")]
            {
                crate::tesseract::check(cfg)
            }
            #[cfg(not(feature = "tesseract"))]
            {
                let _ = cfg;
                Err(EngineError::NotCompiled(kind.label()))
            }
        }
        EngineKind::Oar => {
            #[cfg(feature = "oar")]
            {
                crate::oar::check(cfg)
            }
            #[cfg(not(feature = "oar"))]
            {
                let _ = cfg;
                Err(EngineError::NotCompiled(kind.label()))
            }
        }
    }
}

/// Load an engine. Expensive (models are read); one per worker thread.
pub fn build_engine(kind: EngineKind, cfg: &OcrConfig) -> Result<Box<dyn OcrEngine>, EngineError> {
    check_engine(kind, cfg)?;
    match kind {
        EngineKind::Tesseract => {
            #[cfg(feature = "tesseract")]
            {
                crate::tesseract::build(cfg)
            }
            #[cfg(not(feature = "tesseract"))]
            unreachable!("checked above")
        }
        EngineKind::Oar => {
            #[cfg(feature = "oar")]
            {
                crate::oar::build(cfg)
            }
            #[cfg(not(feature = "oar"))]
            unreachable!("checked above")
        }
    }
}

/// The languages (Tesseract) or model sets (oar) available to `kind` with
/// the current data directory, without loading anything.
pub fn languages(kind: EngineKind, cfg: &OcrConfig) -> Vec<String> {
    match kind {
        EngineKind::Tesseract => crate::data::tessdata_dir(&cfg.tesseract)
            .map(|d| crate::data::tessdata_languages(&d))
            .unwrap_or_default(),
        EngineKind::Oar => crate::data::oar_model_sets(&cfg.oar),
    }
}

/// [`languages`] as `(id, label)` pairs for a picker: Tesseract codes label
/// themselves, oar model sets get their names.
pub fn language_options(kind: EngineKind, cfg: &OcrConfig) -> Vec<(String, String)> {
    languages(kind, cfg)
        .into_iter()
        .map(|id| {
            let label = match kind {
                EngineKind::Tesseract => id.clone(),
                EngineKind::Oar => crate::data::OAR_MODEL_SETS
                    .iter()
                    .find(|s| s.id == id)
                    .map(|s| s.label.to_string())
                    .unwrap_or_else(|| id.clone()),
            };
            (id, label)
        })
        .collect()
}

/// The language/model-set choice that `cfg` holds for `kind`.
pub fn configured_language(kind: EngineKind, cfg: &OcrConfig) -> &str {
    match kind {
        EngineKind::Tesseract => &cfg.tesseract.lang,
        EngineKind::Oar => &cfg.oar.model_set,
    }
}

/// Store a language/model-set choice for `kind` in `cfg`.
pub fn set_language(kind: EngineKind, cfg: &mut OcrConfig, id: &str) {
    match kind {
        EngineKind::Tesseract => cfg.tesseract.lang = id.to_string(),
        EngineKind::Oar => cfg.oar.model_set = id.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sheaf_image::raster::PixelFormat;

    #[test]
    fn raster_converts_to_gray_and_rgb() {
        let r = Raster {
            width: 2,
            height: 1,
            format: PixelFormat::Bgra,
            data: vec![0, 0, 255, 255, 255, 255, 255, 255], // red, white (BGRA)
        };
        let OcrImage::Gray(g) = OcrImage::from_raster(&r, InputKind::Gray).unwrap() else {
            panic!()
        };
        assert_eq!(g.as_raw(), &[76, 255]);
        let OcrImage::Rgb(c) = OcrImage::from_raster(&r, InputKind::Rgb).unwrap() else {
            panic!()
        };
        assert_eq!(c.as_raw(), &[255, 0, 0, 255, 255, 255]);
        let short = Raster {
            width: 2,
            height: 2,
            format: PixelFormat::Rgba,
            data: vec![0; 4],
        };
        assert!(OcrImage::from_raster(&short, InputKind::Gray).is_none());
    }

    #[test]
    fn kinds_round_trip_through_serde() {
        for k in EngineKind::ALL {
            let s = serde_json::to_string(&k).unwrap();
            assert_eq!(s, format!("\"{}\"", k.id()));
            assert_eq!(serde_json::from_str::<EngineKind>(&s).unwrap(), k);
        }
    }

    #[test]
    fn uncompiled_engine_is_reported_not_crashed() {
        let cfg = OcrConfig::default();
        for k in EngineKind::ALL {
            if !k.compiled() {
                assert!(matches!(
                    check_engine(k, &cfg),
                    Err(EngineError::NotCompiled(_))
                ));
                assert!(build_engine(k, &cfg).is_err());
            }
        }
    }
}
