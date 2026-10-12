//! The sheaf logo (f2 navy family, dark tile; art in `assets/icons/`).

use std::sync::{Arc, LazyLock};

use gpui::{Image, ImageFormat, Img, Pixels, Styled, Window, img};

/// The window icon for platforms that take one at runtime (X11). Windows
/// reads it from the exe's resources instead (build.rs).
pub fn window_icon() -> Option<Arc<image::RgbaImage>> {
    image::load_from_memory(include_bytes!("../resources/sheaf-256.png"))
        .map(|i| Arc::new(i.to_rgba8()))
        .inspect_err(|e| log::warn!("window icon: {e}"))
        .ok()
}

macro_rules! asset {
    ($format:ident, $path:literal) => {
        LazyLock::new(|| {
            Arc::new(Image::from_bytes(
                ImageFormat::$format,
                include_bytes!(concat!("../../../assets/icons/", $path)).to_vec(),
            ))
        })
    };
}

// Hand-rasterized for small sizes; scaling the SVG down loses the S.
static MARK_16: LazyLock<Arc<Image>> = asset!(Png, "png/sheaf-f2-dark-16.png");
static MARK_24: LazyLock<Arc<Image>> = asset!(Png, "png/sheaf-f2-dark-24.png");
static MARK_32: LazyLock<Arc<Image>> = asset!(Png, "png/sheaf-f2-dark-32.png");
static LOGO: LazyLock<Arc<Image>> = asset!(Svg, "sheaf-f2-dark.svg");

/// A small square mark, using the hand-tuned raster nearest the device size.
pub fn mark(size: Pixels, window: &Window) -> Img {
    let device = f32::from(size) * window.scale_factor();
    let image = if device <= 20.0 {
        &MARK_16
    } else if device <= 28.0 {
        &MARK_24
    } else {
        &MARK_32
    };
    img(Arc::clone(image)).size(size).flex_none()
}

/// The full-detail logo, for display sizes well above 32px.
pub fn logo(size: Pixels) -> Img {
    img(Arc::clone(&LOGO)).size(size).flex_none()
}
