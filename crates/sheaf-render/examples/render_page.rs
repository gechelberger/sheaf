//! M0 spike: render pages of a PDF to PNG via PDFium and report timings.
//!
//!     cargo run -p sheaf-render --example render_page [input.pdf] [max_pages]
//!
//! Defaults to target/spike/synth.pdf (produced by the sheaf-doc roundtrip
//! spike). PNGs land in target/spike/.

use std::time::Instant;

use anyhow::Result;
use sheaf_core::PageIndex;
use sheaf_render::{PixelFormat, Renderer, pdfium::PdfiumEngine};

fn main() -> Result<()> {
    env_logger::init();
    let input = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/spike/synth.pdf".into());
    let max_pages: u32 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(3);
    let out_dir = std::path::Path::new("target/spike");
    std::fs::create_dir_all(out_dir)?;

    let t = Instant::now();
    let r = PdfiumEngine::open(&input, None)?;
    let opened = t.elapsed();
    let t = Instant::now();
    let sizes = r.page_sizes()?;
    println!(
        "{input}: {} pages, opened in {opened:?}, page sizes in {:?}",
        r.page_count(),
        t.elapsed()
    );

    for i in 0..r.page_count().min(max_pages) {
        let t = Instant::now();
        let raster = r.render_page(PageIndex(i), 1200, PixelFormat::Rgba)?;
        let render = t.elapsed();
        let path = out_dir.join(format!("page-{i:04}.png"));
        image::save_buffer(
            &path,
            &raster.data,
            raster.width,
            raster.height,
            image::ColorType::Rgba8,
        )?;
        println!(
            "  page {i} ({:.0}x{:.0} pt): {}x{} rendered in {render:?} (+{:?} png) -> {}",
            sizes[i as usize].width,
            sizes[i as usize].height,
            raster.width,
            raster.height,
            t.elapsed() - render,
            path.display()
        );
    }
    Ok(())
}
