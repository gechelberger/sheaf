//! Tesseract 5 through `tesseract-rs`, built from source and linked in.

use anyhow::Context;
use tesseract_rs::{TessPageSegMode, TesseractAPI};

use crate::config::OcrConfig;
use crate::engine::{EngineError, EngineInfo, EngineKind, InputKind, OcrEngine, OcrImage, OcrWord};

pub fn check(cfg: &OcrConfig) -> Result<(), EngineError> {
    crate::data::tessdata_check(&cfg.tesseract).map(|_| ())
}

pub fn build(cfg: &OcrConfig) -> Result<Box<dyn OcrEngine>, EngineError> {
    let dir = crate::data::tessdata_check(&cfg.tesseract)?;
    let api = TesseractAPI::new();
    let init = |e: tesseract_rs::TesseractError| EngineError::Init(format!("Tesseract: {e}"));
    api.init(&dir, &cfg.tesseract.lang).map_err(|e| {
        EngineError::Init(format!(
            "Tesseract could not load '{}' from {}: {e}",
            cfg.tesseract.lang,
            dir.display()
        ))
    })?;
    api.set_page_seg_mode(TessPageSegMode::from_int(cfg.tesseract.psm as i32))
        .map_err(init)?;
    // Tesseract writes diagnostics to a debug file (default stderr).
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let _ = api.set_variable("debug_file", null);
    let languages = crate::data::tessdata_languages(&dir);
    Ok(Box::new(Tesseract {
        api,
        info: EngineInfo {
            kind: EngineKind::Tesseract,
            name: format!("tesseract {}", TesseractAPI::version()),
            languages,
        },
    }))
}

struct Tesseract {
    api: TesseractAPI,
    info: EngineInfo,
}

impl OcrEngine for Tesseract {
    fn input(&self) -> InputKind {
        InputKind::Gray
    }

    fn recognize(&mut self, img: OcrImage, dpi: u32) -> anyhow::Result<Vec<OcrWord>> {
        let OcrImage::Gray(g) = img else {
            anyhow::bail!("Tesseract wants a grayscale image");
        };
        let (w, h) = (g.width() as i32, g.height() as i32);
        self.api
            .set_image(g.as_raw(), w, h, 1, w)
            .context("setting image")?;
        self.api
            .set_source_resolution(dpi as i32)
            .context("setting resolution")?;
        self.api.recognize().context("recognizing")?;
        let mut words = Vec::new();
        // A page with nothing on it has no iterator.
        if let Ok(it) = self.api.get_iterator() {
            loop {
                if let Ok((text, l, t, r, b, conf)) = it.get_word_with_bounds() {
                    let text = text.trim();
                    if !text.is_empty() {
                        words.push(OcrWord {
                            text: text.to_string(),
                            bbox_px: [l as f32, t as f32, r as f32, b as f32],
                            confidence: (conf / 100.0).clamp(0.0, 1.0),
                        });
                    }
                }
                if !it.next_word().unwrap_or(false) {
                    break;
                }
            }
        }
        // Free the page's image and results; adaptive state carries over
        // only if we let it.
        let _ = self.api.clear();
        Ok(words)
    }

    fn describe(&self) -> EngineInfo {
        self.info.clone()
    }
}
