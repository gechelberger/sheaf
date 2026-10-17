//! Which way up a page image is, from PaddleOCR's document orientation
//! model (`pp-lcnet_x1_0_doc_ori`), for straightening photos before OCR.

use image::RgbImage;

/// Clockwise quarter turns that make `img` upright, or `None` when the
/// model isn't available (no `oar` feature, or its files are missing) or
/// isn't sure.
pub fn quarter_turns(img: &RgbImage) -> Option<u16> {
    let s = scores(img)?;
    let (best, score) = (0..4u16)
        .map(|k| (k, s[k as usize]))
        .max_by(|a, b| a.1.total_cmp(&b.1))?;
    // Four classes: chance is 0.25. Below this it's a guess.
    (score >= 0.45).then_some(best)
}

/// The model's confidence, for k = 0..4, that `img` needs k clockwise
/// quarter turns to be upright; `None` without the model. Averaged over
/// `img` turned all four ways: on its own the model has favourite
/// answers, and a page on its side can fall below [`quarter_turns`]'
/// threshold (it did on macOS).
pub fn scores(img: &RgbImage) -> Option<[f32; 4]> {
    #[cfg(feature = "oar")]
    {
        use image::imageops::{rotate90, rotate180, rotate270};
        let mut avg = [0f32; 4];
        let turned = [img.clone(), rotate90(img), rotate180(img), rotate270(img)];
        for (r, im) in turned.iter().enumerate() {
            // Turned r more clockwise, it needs r fewer turns.
            for (j, v) in model::scores(im)?.iter().enumerate() {
                avg[(j + r) % 4] += v / 4.0;
            }
        }
        Some(avg)
    }
    #[cfg(not(feature = "oar"))]
    {
        let _ = img;
        None
    }
}

/// Whether [`quarter_turns`] can answer.
pub fn available() -> bool {
    #[cfg(feature = "oar")]
    {
        model::predictor().is_some()
    }
    #[cfg(not(feature = "oar"))]
    {
        false
    }
}

#[cfg(feature = "oar")]
mod model {
    use std::sync::{Mutex, OnceLock};

    use image::RgbImage;
    use oar_ocr::predictors::DocumentOrientationPredictor;

    pub fn predictor() -> Option<&'static Mutex<DocumentOrientationPredictor>> {
        static CELL: OnceLock<Option<Mutex<DocumentOrientationPredictor>>> = OnceLock::new();
        CELL.get_or_init(|| {
            let model = crate::data::models_dir()?.join(crate::data::OAR_DOC_ORI);
            let runtime = crate::data::onnxruntime_lib()?;
            if !model.is_file() || !runtime.is_file() {
                return None;
            }
            if let Err(e) = crate::oar::load_runtime(&runtime) {
                log::warn!("orientation model: {e}");
                return None;
            }
            match DocumentOrientationPredictor::builder()
                .topk(4)
                .build(&model)
            {
                Ok(p) => Some(Mutex::new(p)),
                Err(e) => {
                    log::warn!("orientation model {}: {e}", model.display());
                    None
                }
            }
        })
        .as_ref()
    }

    pub fn scores(img: &RgbImage) -> Option<[f32; 4]> {
        let p = predictor()?.lock().ok()?;
        let out = p.predict(vec![img.clone()]).ok()?;
        let mut s = [0f32; 4];
        for c in out.orientations.first()? {
            // The label is how far the content is turned clockwise;
            // undoing it takes the rest of a full turn.
            let deg: u16 = c.label.parse().ok()?;
            s[((4 - (deg / 90) % 4) % 4) as usize] = c.score;
        }
        log::debug!("page orientation scores (turns 0–3): {s:?}");
        Some(s)
    }
}

/// Tesseract's orientation detection (OSD): the clockwise quarter turns
/// that make `img` upright and Tesseract's confidence, or `None` without
/// the `tesseract` feature or `osd.traineddata`. It reads the shapes of
/// the characters themselves, so it works on pages with little text.
pub fn osd(img: &image::GrayImage, dpi: u32) -> Option<(u16, f32)> {
    #[cfg(feature = "tesseract")]
    {
        tess::osd(img, dpi)
    }
    #[cfg(not(feature = "tesseract"))]
    {
        let _ = (img, dpi);
        None
    }
}

/// Whether [`osd`] can answer.
pub fn osd_available() -> bool {
    #[cfg(feature = "tesseract")]
    {
        tess::api().is_some()
    }
    #[cfg(not(feature = "tesseract"))]
    {
        false
    }
}

#[cfg(feature = "tesseract")]
mod tess {
    use std::sync::{Mutex, OnceLock};

    use tesseract_rs::{TessPageSegMode, TesseractAPI};

    pub fn api() -> Option<&'static Mutex<TesseractAPI>> {
        static CELL: OnceLock<Option<Mutex<TesseractAPI>>> = OnceLock::new();
        CELL.get_or_init(|| {
            let dir = crate::data::tessdata_dir(&Default::default())?;
            if !dir.join("osd.traineddata").is_file() {
                return None;
            }
            let api = TesseractAPI::new();
            if let Err(e) = api.init(&dir, "osd") {
                log::warn!("Tesseract OSD: {e}");
                return None;
            }
            let _ = api.set_page_seg_mode(TessPageSegMode::PSM_OSD_ONLY);
            let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
            let _ = api.set_variable("debug_file", null);
            Some(Mutex::new(api))
        })
        .as_ref()
    }

    pub fn osd(img: &image::GrayImage, dpi: u32) -> Option<(u16, f32)> {
        let api = api()?.lock().ok()?;
        let (w, h) = (img.width() as i32, img.height() as i32);
        api.set_image(img.as_raw(), w, h, 1, w).ok()?;
        api.set_source_resolution(dpi as i32).ok()?;
        let (deg, conf, _, _) = api.detect_os().ok()?;
        log::debug!("Tesseract OSD: {deg}° ({conf:.2})");
        // Tesseract reports how far the image is turned clockwise; undoing
        // it takes the rest of a full turn.
        let k = (deg.rem_euclid(360) / 90) as u16;
        Some(((4 - k) % 4, conf))
    }
}
