//! Flattening curled or bent pages in photos with the UVDoc model, for
//! scan cleanup. Opt-in: it can add a slight wobble to pages that were
//! flat already, and the model (`uvdoc.onnx`, about 31 MB) is only there
//! after `scripts/fetch-ocr-data.sh --uvdoc`.
//!
//! The network predicts a coarse sampling grid, which is upsampled and
//! applied to the full photo, so detail is kept; sharp creases don't fully
//! flatten.

use image::RgbImage;

/// Whether [`unwarp`] can run.
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

/// Why it can't, for the UI.
pub fn unavailable_reason() -> Option<String> {
    if available() {
        return None;
    }
    Some(if cfg!(feature = "oar") {
        format!(
            "needs the UVDoc model ({}); run scripts/fetch-ocr-data.sh --uvdoc",
            crate::data::OAR_UVDOC
        )
    } else {
        "this build has no PaddleOCR (oar) support".into()
    })
}

/// `img` with its page flattened, or `None` when the model isn't there
/// or fails.
pub fn unwarp(img: &RgbImage) -> Option<RgbImage> {
    #[cfg(feature = "oar")]
    {
        model::unwarp(img)
    }
    #[cfg(not(feature = "oar"))]
    {
        let _ = img;
        None
    }
}

#[cfg(feature = "oar")]
mod model {
    use std::sync::{Mutex, OnceLock};

    use image::RgbImage;
    use oar_ocr::predictors::DocumentRectificationPredictor;

    pub fn predictor() -> Option<&'static Mutex<DocumentRectificationPredictor>> {
        static CELL: OnceLock<Option<Mutex<DocumentRectificationPredictor>>> = OnceLock::new();
        CELL.get_or_init(|| {
            let model = crate::data::models_dir()?.join(crate::data::OAR_UVDOC);
            let runtime = crate::data::onnxruntime_lib()?;
            if !model.is_file() || !runtime.is_file() {
                return None;
            }
            if let Err(e) = crate::oar::load_runtime(&runtime) {
                log::warn!("UVDoc: {e}");
                return None;
            }
            match DocumentRectificationPredictor::builder().build(&model) {
                Ok(p) => Some(Mutex::new(p)),
                Err(e) => {
                    log::warn!("UVDoc model {}: {e}", model.display());
                    None
                }
            }
        })
        .as_ref()
    }

    pub fn unwarp(img: &RgbImage) -> Option<RgbImage> {
        let p = predictor()?.lock().ok()?;
        match p.predict(vec![img.clone()]) {
            Ok(mut out) if !out.images.is_empty() => Some(out.images.swap_remove(0)),
            Ok(_) => None,
            Err(e) => {
                log::warn!("UVDoc: {e}");
                None
            }
        }
    }
}
