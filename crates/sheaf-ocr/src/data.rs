//! Where the engines' data lives: Tesseract language files and ONNX
//! models. Same lookup order as libpdfium: `$SHEAF_OCR_DATA_DIR`, `ocr/`
//! beside the executable (how packaged builds ship it), `vendor/ocr`
//! under the current directory, and `vendor/ocr` in this source tree (for
//! tests run from a crate directory).

use std::path::{Path, PathBuf};

use crate::config::{OarConfig, TesseractConfig};
use crate::engine::EngineError;

pub const FETCH_HINT: &str = "run scripts/fetch-ocr-data.sh to download the OCR data";

/// Every directory that is searched, in order.
pub fn candidates() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(d) = std::env::var("SHEAF_OCR_DATA_DIR") {
        dirs.push(PathBuf::from(d));
    }
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        dirs.push(exe_dir.join("ocr"));
    }
    dirs.push(PathBuf::from("vendor/ocr"));
    dirs.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("vendor/ocr"),
    );
    dirs
}

/// The first candidate that exists.
pub fn data_dir() -> Option<PathBuf> {
    candidates().into_iter().find(|d| d.is_dir())
}

fn missing(what: &str) -> EngineError {
    let searched: Vec<String> = candidates()
        .iter()
        .map(|p| p.display().to_string())
        .collect();
    EngineError::MissingData(format!(
        "{what} not found (looked in {}); {FETCH_HINT}",
        searched.join(", ")
    ))
}

/// The directory holding `<lang>.traineddata`.
pub fn tessdata_dir(cfg: &TesseractConfig) -> Option<PathBuf> {
    cfg.tessdata_dir
        .clone()
        .or_else(|| data_dir().map(|d| d.join("tessdata")))
}

/// Check every language in `cfg.lang` ("eng+deu") has its data file.
pub fn tessdata_check(cfg: &TesseractConfig) -> Result<PathBuf, EngineError> {
    let dir = tessdata_dir(cfg).ok_or_else(|| missing("the OCR data directory"))?;
    for lang in cfg.lang.split('+').filter(|l| !l.is_empty()) {
        let file = dir.join(format!("{lang}.traineddata"));
        if !file.is_file() {
            return Err(EngineError::MissingData(format!(
                "Tesseract language file {} not found; {FETCH_HINT}",
                file.display()
            )));
        }
    }
    Ok(dir)
}

/// Language codes with data files in `dir`, sorted.
pub fn tessdata_languages(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| {
                    let name = e.file_name().into_string().ok()?;
                    name.strip_suffix(".traineddata").map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// A bundled PaddleOCR model set: id, label, recognition model, dictionary.
pub struct OarModelSet {
    pub id: &'static str,
    pub label: &'static str,
    pub rec: &'static str,
    pub dict: &'static str,
}

pub const OAR_MODEL_SETS: [OarModelSet; 2] = [
    OarModelSet {
        id: "en",
        label: "English",
        rec: "en_pp-ocrv5_mobile_rec.onnx",
        dict: "ppocrv5_en_dict.txt",
    },
    OarModelSet {
        id: "multi",
        label: "Multilingual",
        rec: "pp-ocrv5_mobile_rec.onnx",
        dict: "ppocrv5_dict.txt",
    },
];
pub const OAR_DET: &str = "pp-ocrv5_mobile_det.onnx";
pub const OAR_TEXTLINE_ORI: &str = "pp-lcnet_x0_25_textline_ori.onnx";
pub const OAR_DOC_ORI: &str = "pp-lcnet_x1_0_doc_ori.onnx";

/// The model files an oar engine will load.
#[derive(Debug, Clone, PartialEq)]
pub struct OarModels {
    pub det: PathBuf,
    pub rec: PathBuf,
    pub dict: PathBuf,
    pub textline_ori: Option<PathBuf>,
    pub doc_ori: Option<PathBuf>,
    pub set_label: String,
}

pub fn models_dir() -> Option<PathBuf> {
    data_dir().map(|d| d.join("models"))
}

/// Resolve the model paths for `cfg`: explicit overrides first, else the
/// bundled set. Required files must exist; orientation models are optional.
pub fn oar_models(cfg: &OarConfig) -> Result<OarModels, EngineError> {
    let set = OAR_MODEL_SETS
        .iter()
        .find(|s| s.id == cfg.model_set)
        .unwrap_or(&OAR_MODEL_SETS[0]);
    let dir = models_dir();
    let bundled = |name: &str| dir.as_ref().map(|d| d.join(name));
    let required =
        |over: &Option<PathBuf>, name: &str, what: &str| -> Result<PathBuf, EngineError> {
            let p = over
                .clone()
                .or_else(|| bundled(name))
                .ok_or_else(|| missing(what))?;
            if p.is_file() {
                Ok(p)
            } else {
                Err(EngineError::MissingData(format!(
                    "{what} {} not found; {FETCH_HINT}",
                    p.display()
                )))
            }
        };
    let optional = |over: &Option<PathBuf>, name: &str| {
        over.clone()
            .or_else(|| bundled(name))
            .filter(|p| p.is_file())
    };
    Ok(OarModels {
        det: required(&cfg.det, OAR_DET, "text detection model")?,
        rec: required(&cfg.rec, set.rec, "text recognition model")?,
        dict: required(&cfg.dict, set.dict, "character dictionary")?,
        textline_ori: optional(&cfg.textline_ori, OAR_TEXTLINE_ORI),
        doc_ori: if cfg.use_doc_orientation {
            optional(&cfg.doc_ori, OAR_DOC_ORI)
        } else {
            None
        },
        set_label: set.label.to_string(),
    })
}

/// Bundled model sets whose files are present (ids).
pub fn oar_model_sets(cfg: &OarConfig) -> Vec<String> {
    let Some(dir) = models_dir() else {
        return Vec::new();
    };
    let _ = cfg;
    OAR_MODEL_SETS
        .iter()
        .filter(|s| dir.join(s.rec).is_file() && dir.join(s.dict).is_file())
        .map(|s| s.id.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_override_comes_first() {
        let dir = std::env::temp_dir().join(format!("sheaf-ocr-data-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("tessdata")).unwrap();
        std::fs::write(dir.join("tessdata/eng.traineddata"), b"x").unwrap();
        std::fs::write(dir.join("tessdata/deu.traineddata"), b"x").unwrap();
        // SAFETY: tests in this crate don't run other env-dependent code concurrently.
        unsafe { std::env::set_var("SHEAF_OCR_DATA_DIR", &dir) };
        assert_eq!(candidates()[0], dir);
        assert_eq!(data_dir().as_deref(), Some(dir.as_path()));
        let cfg = TesseractConfig::default();
        assert_eq!(tessdata_check(&cfg).unwrap(), dir.join("tessdata"));
        assert_eq!(tessdata_languages(&dir.join("tessdata")), ["deu", "eng"]);
        let fra = TesseractConfig {
            lang: "eng+fra".into(),
            ..cfg
        };
        let err = tessdata_check(&fra).unwrap_err().to_string();
        assert!(err.contains("fra.traineddata"), "{err}");
        let err = oar_models(&OarConfig::default()).unwrap_err().to_string();
        assert!(err.contains("detection model"), "{err}");
        assert!(oar_model_sets(&OarConfig::default()).is_empty());
        unsafe { std::env::remove_var("SHEAF_OCR_DATA_DIR") };
        let _ = std::fs::remove_dir_all(&dir);
    }
}
