//! Smoke tests for the real engines. They need the OCR data
//! (`scripts/fetch-ocr-data.sh`) and libpdfium, so they are ignored by
//! default; CI runs them with `cargo test -p sheaf-ocr -- --ignored`.
//!
//! The page image comes from PDFium rendering a PDF made here, which is
//! exactly the path the app uses.

#[cfg(any(feature = "tesseract", feature = "oar"))]
use lopdf::{Document as Pdf, Object, Stream, dictionary};
#[cfg(any(feature = "tesseract", feature = "oar"))]
use sheaf_core::{PageIndex, Rotation};
use sheaf_image::raster::PixelFormat;
#[cfg(any(feature = "tesseract", feature = "oar"))]
use sheaf_ocr::{EngineKind, OcrConfig, OcrWord, build_engine};
use sheaf_ocr::{InputKind, OcrImage};
#[cfg(any(feature = "tesseract", feature = "oar"))]
use sheaf_render::{Job, Renderer};

/// A Letter page saying "Hello World 1234" in 24 pt Helvetica.
#[cfg(any(feature = "tesseract", feature = "oar"))]
fn sample_pdf() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("sheaf-ocr-smoke-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("sample.pdf");
    let mut pdf = Pdf::with_version("1.5");
    let pages_id = pdf.new_object_id();
    let font = pdf.add_object(
        dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" },
    );
    let content = pdf.add_object(Stream::new(
        dictionary! {},
        b"BT /F1 24 Tf 72 700 Td (Hello World 1234) Tj ET".to_vec(),
    ));
    let page = pdf.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => content,
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } },
    });
    pdf.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1,
        }),
    );
    let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    pdf.trailer.set("Root", catalog);
    pdf.save(&path).unwrap();
    path
}

#[cfg(any(feature = "tesseract", feature = "oar"))]
fn recognize_sample(kind: EngineKind) -> (Vec<OcrWord>, u32, u32) {
    let cfg = OcrConfig::default();
    let mut engine = build_engine(kind, &cfg).unwrap_or_else(|e| panic!("{e}"));
    let path = sample_pdf();
    let pdfium = sheaf_render::pdfium::PdfiumEngine::open(&path, None).unwrap();
    let raster = pdfium
        .render(
            PageIndex(0),
            Job::Page {
                width: 2550,
                rotation: Rotation::R0,
            },
            PixelFormat::Rgba,
        )
        .unwrap();
    let (w, h) = (raster.width, raster.height);
    let img = OcrImage::from_raster(&raster, engine.input()).unwrap();
    let words = engine.recognize(img, 300).unwrap();
    (words, w, h)
}

#[cfg(any(feature = "tesseract", feature = "oar"))]
fn check(kind: EngineKind) {
    let (words, w, h) = recognize_sample(kind);
    let joined: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
    assert!(
        joined.iter().any(|t| t.contains("Hello")),
        "{kind:?}: {joined:?}"
    );
    assert!(
        joined.iter().any(|t| t.contains("1234")),
        "{kind:?}: {joined:?}"
    );
    for word in &words {
        let [l, t, r, b] = word.bbox_px;
        assert!(
            l >= 0.0 && t >= 0.0 && r <= w as f32 && b <= h as f32,
            "{word:?}"
        );
        assert!(r > l && b > t, "{word:?}");
        assert!((0.0..=1.0).contains(&word.confidence), "{word:?}");
    }
    // 24 pt at 300 dpi is 100 px tall. Tesseract's glyph boxes are a bit
    // shorter; PaddleOCR's detector pads its line boxes, so a bit taller.
    let hello = words.iter().find(|w| w.text.contains("Hello")).unwrap();
    let height = hello.bbox_px[3] - hello.bbox_px[1];
    assert!((40.0..=160.0).contains(&height), "{hello:?}");
    assert!(words.len() >= 3, "words, not one line: {words:?}");
    // The text starts at 72 pt = 300 px from the left (detectors pad a bit).
    assert!((250.0..=330.0).contains(&hello.bbox_px[0]), "{hello:?}");
}

#[test]
#[ignore = "needs OCR data and libpdfium"]
#[cfg(feature = "tesseract")]
fn tesseract_reads_a_rendered_page() {
    check(EngineKind::Tesseract);
}

#[test]
#[ignore = "needs OCR data and libpdfium"]
#[cfg(feature = "oar")]
fn oar_reads_a_rendered_page() {
    check(EngineKind::Oar);
}

#[test]
fn image_kinds_match_engines() {
    // Sanity for the harness itself, always on.
    assert_eq!(
        OcrImage::from_raster(
            &sheaf_image::raster::Raster {
                width: 1,
                height: 1,
                format: PixelFormat::Rgba,
                data: vec![1, 2, 3, 4]
            },
            InputKind::Rgb
        )
        .unwrap()
        .width(),
        1
    );
}

/// A page of text lines, rendered at 100 dpi.
#[cfg(feature = "oar")]
fn text_page() -> image::RgbImage {
    let dir = std::env::temp_dir().join(format!("sheaf-ocr-orient-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("lines.pdf");
    let mut pdf = Pdf::with_version("1.5");
    let pages_id = pdf.new_object_id();
    let font = pdf.add_object(
        dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" },
    );
    let mut ops = String::from("BT /F1 14 Tf 72 720 Td 18 TL\n");
    for i in 0..30 {
        ops.push_str(&format!(
            "(Line {i}: the quick brown fox jumps over the lazy dog, again.) '\n"
        ));
    }
    ops.push_str("ET");
    let content = pdf.add_object(Stream::new(dictionary! {}, ops.into_bytes()));
    let page = pdf.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => content,
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } },
    });
    pdf.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1,
        }),
    );
    let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    pdf.trailer.set("Root", catalog);
    pdf.save(&path).unwrap();
    let pdfium = sheaf_render::pdfium::PdfiumEngine::open(&path, None).unwrap();
    let raster = pdfium
        .render(
            PageIndex(0),
            Job::Page {
                width: 850,
                rotation: Rotation::R0,
            },
            PixelFormat::Rgba,
        )
        .unwrap();
    raster.to_rgb().unwrap()
}

#[test]
#[ignore = "needs OCR data and libpdfium"]
#[cfg(feature = "oar")]
fn oar_orientation_undoes_a_turn() {
    use image::imageops::{rotate90, rotate180, rotate270};
    assert!(sheaf_ocr::orient::available());
    let page = text_page();
    let check = |img: &image::RgbImage, want: u16| {
        use sheaf_ocr::orient::{quarter_turns, scores};
        assert_eq!(quarter_turns(img), Some(want), "scores {:?}", scores(img));
    };
    check(&page, 0);
    // Turned clockwise once, it takes three more to come back.
    check(&rotate90(&page), 3);
    check(&rotate180(&page), 2);
    check(&rotate270(&page), 1);
}

#[test]
#[ignore = "needs libpdfium; skips without the UVDoc model (fetch-ocr-data.sh --uvdoc)"]
#[cfg(feature = "oar")]
fn uvdoc_returns_a_page() {
    // The model is opt-in (fetch-ocr-data.sh --uvdoc); CI doesn't fetch it.
    if !sheaf_ocr::rectify::available() {
        eprintln!("skipped: no UVDoc model");
        return;
    }
    let page = text_page();
    let flat = sheaf_ocr::rectify::unwarp(&page).expect("an image");
    // Same page, roughly the same size.
    let ratio = flat.width() as f32 / page.width() as f32;
    assert!(
        (0.5..2.0).contains(&ratio),
        "{}×{}",
        flat.width(),
        flat.height()
    );
}

#[test]
#[ignore = "needs libpdfium; skips without osd.traineddata"]
#[cfg(all(feature = "oar", feature = "tesseract"))]
fn tesseract_osd_undoes_a_turn() {
    use image::imageops::{rotate90, rotate270};
    if !sheaf_ocr::orient::osd_available() {
        eprintln!("skipped: no osd.traineddata");
        return;
    }
    let gray = |im: image::RgbImage| image::DynamicImage::ImageRgb8(im).into_luma8();
    let page = text_page();
    let turns = |im| sheaf_ocr::orient::osd(&im, 100).map(|(k, _)| k);
    assert_eq!(turns(gray(page.clone())), Some(0));
    // Turned clockwise once, it takes three more to come back.
    assert_eq!(turns(gray(rotate90(&page))), Some(3));
    assert_eq!(turns(gray(rotate270(&page))), Some(1));
}
