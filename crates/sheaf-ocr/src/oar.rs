//! PaddleOCR (PP-OCRv5) models on ONNX Runtime through `oar-ocr`.

use oar_ocr::core::config::OrtSessionConfig;
use oar_ocr::prelude::{OAROCR, OAROCRBuilder};

use crate::config::OcrConfig;
use crate::engine::{EngineError, EngineInfo, EngineKind, InputKind, OcrEngine, OcrImage, OcrWord};
use crate::geom::words_from_line;

pub fn check(cfg: &OcrConfig) -> Result<(), EngineError> {
    crate::data::oar_models(&cfg.oar).map(|_| ())
}

/// Load Microsoft's ONNX Runtime from `path` once per process. ort keeps
/// one global environment; a second call is a no-op.
pub(crate) fn load_runtime(path: &std::path::Path) -> Result<(), EngineError> {
    use std::sync::OnceLock;
    static LOADED: OnceLock<Result<(), String>> = OnceLock::new();
    LOADED
        .get_or_init(|| {
            ort::init_from(path)
                .map(|env| {
                    env.commit();
                })
                .map_err(|e| format!("loading ONNX Runtime from {}: {e}", path.display()))
        })
        .clone()
        .map_err(EngineError::Init)
}

pub fn build(cfg: &OcrConfig) -> Result<Box<dyn OcrEngine>, EngineError> {
    let models = crate::data::oar_models(&cfg.oar)?;
    load_runtime(&models.runtime)?;
    let mut builder =
        OAROCRBuilder::new(models.det.clone(), models.rec.clone(), models.dict.clone())
            .return_word_box(true)
            .image_batch_size(1)
            // Two threads per session; the job runs up to two sessions.
            .ort_session(
                OrtSessionConfig::new()
                    .with_intra_threads(2)
                    .with_log_severity_level(3),
            );
    if let Some(p) = &models.textline_ori {
        builder = builder.with_text_line_orientation_classification(p.clone());
    }
    if let Some(p) = &models.doc_ori {
        builder = builder.with_document_image_orientation_classification(p.clone());
    }
    // Rectification (UVDoc) is deliberately not enabled: its boxes are in
    // the unwarped image's coordinates and can't be mapped back to the page.
    let ocr = builder
        .build()
        .map_err(|e| EngineError::Init(format!("loading PaddleOCR models: {e}")))?;
    Ok(Box::new(Oar {
        ocr,
        info: EngineInfo {
            kind: EngineKind::Oar,
            name: format!("oar-ocr PP-OCRv5 {}", models.set_label),
            languages: crate::data::oar_model_sets(&cfg.oar),
        },
    }))
}

struct Oar {
    ocr: OAROCR,
    info: EngineInfo,
}

fn aabb(b: &oar_ocr::processors::BoundingBox) -> [f32; 4] {
    b.points.iter().fold(
        [f32::MAX, f32::MAX, f32::MIN, f32::MIN],
        |[l, t, r, btm], p| [l.min(p.x), t.min(p.y), r.max(p.x), btm.max(p.y)],
    )
}

impl OcrEngine for Oar {
    fn input(&self) -> InputKind {
        InputKind::Rgb
    }

    fn recognize(&mut self, img: OcrImage, _dpi: u32) -> anyhow::Result<Vec<OcrWord>> {
        let OcrImage::Rgb(rgb) = img else {
            anyhow::bail!("PaddleOCR wants an RGB image");
        };
        let results = self
            .ocr
            .predict(vec![rgb])
            .map_err(|e| anyhow::anyhow!("PaddleOCR: {e}"))?;
        let mut words = Vec::new();
        for region in results.iter().flat_map(|r| r.text_regions.iter()) {
            let Some(text) = &region.text else { continue };
            if region.bounding_box.points.is_empty() {
                continue;
            }
            let line = aabb(&region.bounding_box);
            let boxes: Option<Vec<[f32; 4]>> = region.word_boxes.as_ref().map(|v| {
                v.iter()
                    .filter(|b| !b.points.is_empty())
                    .map(aabb)
                    .collect()
            });
            let conf = region.confidence.unwrap_or(0.0).clamp(0.0, 1.0);
            words.extend(words_from_line(text, line, boxes.as_deref(), conf));
        }
        Ok(words)
    }

    fn describe(&self) -> EngineInfo {
        self.info.clone()
    }
}
