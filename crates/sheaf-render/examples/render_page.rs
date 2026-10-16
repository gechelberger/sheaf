//! M0 spike: render pages of a PDF to PNG via PDFium and report timings.
//!
//!     cargo run -p sheaf-render --example render_page [input.pdf] [max_pages] [query [--re]]
//!
//! Defaults to target/spike/synth.pdf (produced by the sheaf-doc roundtrip
//! spike). PNGs land in target/spike/.

use std::time::Instant;

use anyhow::Result;
use sheaf_core::PageIndex;
use sheaf_image::raster::PixelFormat;
use sheaf_render::{Renderer, pdfium::PdfiumEngine};

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
    let sizes = (0..r.page_count())
        .map(|i| r.page_size(PageIndex(i)))
        .collect::<anyhow::Result<Vec<_>>>()?;
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

    // Search check: `render_page <pdf> <max_pages> <query>` draws hit boxes
    // in red on each rendered page's PNG.
    if let Some(query) = std::env::args().nth(3) {
        for i in 0..r.page_count().min(max_pages) {
            let t = Instant::now();
            let hits = match std::env::args().nth(4).as_deref() {
                Some("--re") => r.pattern_search_page(PageIndex(i), &regex::Regex::new(&query)?)?,
                _ => r.search_page(PageIndex(i), &query, false)?,
            };
            println!(
                "  page {i}: {} hits for {query:?} in {:?}",
                hits.len(),
                t.elapsed()
            );
            let path = out_dir.join(format!("page-{i:04}.png"));
            let mut im = image::open(&path)?.to_rgba8();
            let k = im.width() as f32 / sizes[i as usize].width;
            for h in &hits {
                for r in &h.rects {
                    let (x0, y0) = ((r.x * k) as u32, (r.y * k) as u32);
                    let (x1, y1) = (((r.x + r.w) * k) as u32, ((r.y + r.h) * k) as u32);
                    for x in x0..x1.min(im.width()) {
                        for y in [y0, y1.saturating_sub(1)] {
                            if y < im.height() {
                                im.put_pixel(x, y, image::Rgba([255, 0, 0, 255]));
                            }
                        }
                    }
                    for y in y0..y1.min(im.height()) {
                        for x in [x0, x1.saturating_sub(1)] {
                            if x < im.width() {
                                im.put_pixel(x, y, image::Rgba([255, 0, 0, 255]));
                            }
                        }
                    }
                }
            }
            im.save(&path)?;
        }
    }

    // Tile check: page 0 at 2400px wide, the tile one step down and right.
    let job = sheaf_render::Job::Tile {
        page_width: 2400,
        page_height: 3106,
        x: 1024,
        y: 1024,
        w: 1024,
        h: 1024,
        rotation: sheaf_core::Rotation::R0,
    };
    let t = Instant::now();
    let tile = r.render(PageIndex(0), job, PixelFormat::Rgba)?;
    let path = out_dir.join("tile-0-1024-1024.png");
    image::save_buffer(
        &path,
        &tile.data,
        tile.width,
        tile.height,
        image::ColorType::Rgba8,
    )?;
    println!(
        "  tile {}x{} rendered in {:?} -> {}",
        tile.width,
        tile.height,
        t.elapsed(),
        path.display()
    );
    Ok(())
}
