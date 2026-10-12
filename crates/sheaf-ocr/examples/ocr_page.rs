//! Recognize one page of a PDF and print the words with their boxes, in
//! pixels and in points. For checking an engine's output by eye.
//!
//! Usage: cargo run -p sheaf-ocr --features tesseract,oar --example ocr_page -- file.pdf [page] [tesseract|oar]

use std::path::PathBuf;

use sheaf_core::{PageIndex, Rotation};
use sheaf_ocr::geom::PxToPt;
use sheaf_ocr::{EngineKind, OcrConfig, OcrImage, build_engine};
use sheaf_render::pdfium::PdfiumEngine;
use sheaf_render::{Job, PixelFormat, Renderer};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: ocr_page file.pdf [page] [tesseract|oar]");
        std::process::exit(2);
    };
    let page: u32 = args.next().map(|p| p.parse()).transpose()?.unwrap_or(1);
    let kind = match args.next().as_deref() {
        Some("oar") => EngineKind::Oar,
        _ => EngineKind::Tesseract,
    };
    let cfg = OcrConfig::default();
    let mut engine = build_engine(kind, &cfg).map_err(|e| anyhow::anyhow!("{e}"))?;
    let pdf = PdfiumEngine::open(PathBuf::from(&path), None)?;
    let size = pdf.page_size(PageIndex(page - 1))?;
    let width = (size.width * cfg.dpi as f32 / 72.0).round() as u32;
    let raster = pdf.render(
        PageIndex(page - 1),
        Job::Page {
            width,
            rotation: Rotation::R0,
        },
        PixelFormat::Rgba,
    )?;
    let m = PxToPt::new(size, raster.width, raster.height);
    let img = OcrImage::from_raster(&raster, engine.input()).unwrap();
    let words = engine.recognize(img, cfg.dpi)?;
    println!(
        "page {page}: {:.0}×{:.0} pt, raster {}×{}, {} words via {}",
        size.width,
        size.height,
        raster.width,
        raster.height,
        words.len(),
        engine.describe().name
    );
    for w in &words {
        let r = m.rect(w.bbox_px);
        println!(
            "  {:<16} px [{:.0},{:.0} {:.0}×{:.0}]  pt [{:.1},{:.1} {:.1}×{:.1}]  conf {:.2}",
            format!("{:?}", w.text),
            w.bbox_px[0],
            w.bbox_px[1],
            w.bbox_px[2] - w.bbox_px[0],
            w.bbox_px[3] - w.bbox_px[1],
            r.x,
            r.y,
            r.w,
            r.h,
            w.confidence
        );
    }
    Ok(())
}
