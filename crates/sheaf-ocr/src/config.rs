//! OCR options. The app remembers the Recognize Text dialog's choices in
//! `state.json` under `"ocr"`; the paths and tuning set by hand come from
//! `[ocr]` in `settings.toml` (`sheaf_ops::prefs::OcrPrefs`).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::engine::EngineKind;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OcrConfig {
    pub engine: EngineKind,
    /// Pages are rendered at this resolution for recognition.
    pub dpi: u32,
    /// Leave pages that already have a text layer alone.
    pub skip_pages_with_text: bool,
    /// The page scope last chosen in the dialog (see [`Scope`]).
    pub last_scope: String,
    pub tesseract: TesseractConfig,
    pub oar: OarConfig,
}

impl Default for OcrConfig {
    fn default() -> Self {
        Self {
            engine: EngineKind::Tesseract,
            dpi: 300,
            skip_pages_with_text: true,
            last_scope: Scope::All.id().to_string(),
            tesseract: TesseractConfig::default(),
            oar: OarConfig::default(),
        }
    }
}

impl OcrConfig {
    pub const MIN_DPI: u32 = 150;
    pub const MAX_DPI: u32 = 600;

    pub fn clamped_dpi(&self) -> u32 {
        self.dpi.clamp(Self::MIN_DPI, Self::MAX_DPI)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TesseractConfig {
    /// Directory holding `<lang>.traineddata`; default is `tessdata/` in
    /// the data directory.
    pub tessdata_dir: Option<PathBuf>,
    /// Tesseract language code(s), e.g. "eng" or "eng+deu".
    pub lang: String,
    /// Page segmentation mode (3 = automatic).
    pub psm: u8,
}

impl Default for TesseractConfig {
    fn default() -> Self {
        Self {
            tessdata_dir: None,
            lang: "eng".into(),
            psm: 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OarConfig {
    /// Which bundled recognition model to use: "en" or "multi".
    pub model_set: String,
    /// Explicit model paths override the bundled set.
    pub det: Option<PathBuf>,
    pub rec: Option<PathBuf>,
    pub dict: Option<PathBuf>,
    pub textline_ori: Option<PathBuf>,
    pub doc_ori: Option<PathBuf>,
    /// Classify and correct whole-page orientation before detection.
    pub use_doc_orientation: bool,
}

impl Default for OarConfig {
    fn default() -> Self {
        Self {
            model_set: "en".into(),
            det: None,
            rec: None,
            dict: None,
            textline_ori: None,
            doc_ori: None,
            use_doc_orientation: true,
        }
    }
}

/// Which pages an OCR run covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scope {
    #[default]
    All,
    Current,
    Selected,
    Range,
}

impl Scope {
    pub const ALL: [Scope; 4] = [Scope::All, Scope::Current, Scope::Selected, Scope::Range];

    pub fn id(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Current => "current",
            Self::Selected => "selected",
            Self::Range => "range",
        }
    }

    pub fn from_id(id: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|s| s.id() == id)
            .unwrap_or_default()
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All pages",
            Self::Current => "Current page",
            Self::Selected => "Selected pages",
            Self::Range => "Pages",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_unknown_keys_round_trip() {
        let cfg = OcrConfig::default();
        assert_eq!(cfg.engine, EngineKind::Tesseract);
        assert_eq!(cfg.dpi, 300);
        assert!(cfg.skip_pages_with_text);
        let json = r#"{"engine":"oar","dpi":200,"future":1,"oar":{"model_set":"multi"}}"#;
        let parsed: OcrConfig = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.engine, EngineKind::Oar);
        assert_eq!(parsed.dpi, 200);
        assert_eq!(parsed.oar.model_set, "multi");
        assert_eq!(parsed.tesseract, TesseractConfig::default());
        let back: OcrConfig =
            serde_json::from_str(&serde_json::to_string(&parsed).unwrap()).unwrap();
        assert_eq!(back, parsed);
        assert_eq!(OcrConfig { dpi: 9999, ..cfg }.clamped_dpi(), 600);
    }

    #[test]
    fn scope_ids() {
        for s in Scope::ALL {
            assert_eq!(Scope::from_id(s.id()), s);
        }
        assert_eq!(Scope::from_id("bogus"), Scope::All);
    }
}
