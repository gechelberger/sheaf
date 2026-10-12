//! Regenerate the app icon rasters in `crates/sheaf-app/resources/` from the
//! logo art in `assets/icons/`. The outputs are committed so ordinary builds
//! never rasterize; rerun this after changing the f2-dark artwork:
//!
//!     cargo run -p sheaf-app --example gen_icons
//!
//! The .ico takes 16/24/32 from the hand-tuned PNGs and renders the larger
//! sizes from the full-detail SVG.

use std::{fs, io::Cursor, path::Path};

use image::{
    ExtendedColorType, ImageEncoder, RgbaImage,
    codecs::ico::{IcoEncoder, IcoFrame},
};

const HAND_TUNED: [u32; 3] = [16, 24, 32];
const RENDERED: [u32; 4] = [48, 64, 128, 256];

fn main() -> anyhow::Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let icons = root.join("assets/icons");
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources");
    fs::create_dir_all(&out)?;

    let svg = fs::read(icons.join("sheaf-f2-dark.svg"))?;
    let tree = resvg::usvg::Tree::from_data(&svg, &Default::default())?;

    let mut frames = Vec::new();
    for size in HAND_TUNED {
        let png = image::open(icons.join(format!("png/sheaf-f2-dark-{size}.png")))?.to_rgba8();
        anyhow::ensure!(
            png.width() == size && png.height() == size,
            "{size}px PNG is the wrong size"
        );
        frames.push(frame(&png)?);
    }
    for size in RENDERED {
        let img = render(&tree, size)?;
        if size == 256 {
            img.save(out.join("sheaf-256.png"))?;
        }
        frames.push(frame(&img)?);
    }
    let mut ico = Vec::new();
    IcoEncoder::new(Cursor::new(&mut ico)).encode_images(&frames)?;
    fs::write(out.join("sheaf.ico"), ico)?;
    fs::write(out.join("sheaf.svg"), &svg)?;
    println!("wrote {}", out.display());
    Ok(())
}

fn render(tree: &resvg::usvg::Tree, size: u32) -> anyhow::Result<RgbaImage> {
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size).unwrap();
    let scale = size as f32 / tree.size().width();
    resvg::render(
        tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    // tiny-skia stores premultiplied alpha; image wants straight.
    let data = pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect();
    Ok(RgbaImage::from_raw(size, size, data).unwrap())
}

fn frame(img: &RgbaImage) -> anyhow::Result<IcoFrame<'static>> {
    // PNG-compressed frames keep the 256px entry small.
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png).write_image(
        img,
        img.width(),
        img.height(),
        ExtendedColorType::Rgba8,
    )?;
    Ok(IcoFrame::with_encoded(
        png,
        img.width(),
        img.height(),
        ExtendedColorType::Rgba8,
    )?)
}
