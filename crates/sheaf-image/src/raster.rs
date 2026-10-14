//! Rendered pages as pixels: [`Raster`], what a renderer hands back, and
//! its conversions to `image` buffers. Every gray conversion of a render
//! goes through [`Raster::to_gray`], so they all agree.

use image::{GrayImage, RgbImage, RgbaImage};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PixelFormat {
    /// Straight RGBA8, what the `image` crate and most encoders want.
    Rgba,
    /// BGRA8 — PDFium's native output and what GPUI uploads without conversion.
    Bgra,
}

/// A rendered page or tile: tightly packed 4 bytes/pixel, row-major.
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub data: Vec<u8>,
}

/// BT.601 luma in integers: (77 R + 150 G + 29 B) / 256.
pub fn luma(r: u8, g: u8, b: u8) -> u8 {
    ((77 * r as u32 + 150 * g as u32 + 29 * b as u32) >> 8) as u8
}

impl Raster {
    /// Indexes of red, green and blue in a pixel.
    fn order(&self) -> (usize, usize, usize) {
        match self.format {
            PixelFormat::Rgba => (0, 1, 2),
            PixelFormat::Bgra => (2, 1, 0),
        }
    }

    fn pixels(&self) -> &[[u8; 4]] {
        let n = self.width as usize * self.height as usize;
        let (px, _) = self.data.as_chunks::<4>();
        &px[..n.min(px.len())]
    }

    /// Whether `data` holds every pixel.
    fn complete(&self) -> bool {
        self.data.len() >= self.width as usize * self.height as usize * 4
    }

    /// 8-bit gray ([`luma`]). None when the data is short.
    pub fn to_gray(&self) -> Option<GrayImage> {
        if !self.complete() {
            return None;
        }
        let (r, g, b) = self.order();
        let data = self
            .pixels()
            .iter()
            .map(|p| luma(p[r], p[g], p[b]))
            .collect();
        GrayImage::from_raw(self.width, self.height, data)
    }

    /// 8-bit RGB, alpha dropped (pages are opaque). None when the data is
    /// short.
    pub fn to_rgb(&self) -> Option<RgbImage> {
        if !self.complete() {
            return None;
        }
        let (r, g, b) = self.order();
        let mut data = Vec::with_capacity(self.pixels().len() * 3);
        for p in self.pixels() {
            data.extend_from_slice(&[p[r], p[g], p[b]]);
        }
        RgbImage::from_raw(self.width, self.height, data)
    }

    /// RGBA, reusing the buffer (channels swapped in place when it's
    /// BGRA). None when the data is short.
    pub fn into_rgba(mut self) -> Option<RgbaImage> {
        if self.format == PixelFormat::Bgra {
            let (px, _) = self.data.as_chunks_mut::<4>();
            for p in px {
                p.swap(0, 2);
            }
        }
        RgbaImage::from_raw(self.width, self.height, self.data)
    }
}

/// The box `(x0, y0, x1, y1)` (end exclusive) around pixels darker than
/// `below`; none when there aren't any.
pub fn content_bounds(gray: &GrayImage, below: u8) -> Option<(u32, u32, u32, u32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for (x, y, p) in gray.enumerate_pixels() {
        if p[0] < below {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x + 1);
            y1 = y1.max(y + 1);
        }
    }
    (x0 != u32::MAX).then_some((x0, y0, x1, y1))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2×2: red, green / blue, white, in `format`.
    fn raster(format: PixelFormat) -> Raster {
        let rgba = [
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [255; 4],
        ];
        let data = rgba
            .iter()
            .flat_map(|p| match format {
                PixelFormat::Rgba => *p,
                PixelFormat::Bgra => [p[2], p[1], p[0], p[3]],
            })
            .collect();
        Raster {
            width: 2,
            height: 2,
            format,
            data,
        }
    }

    #[test]
    fn conversions_agree_across_formats() {
        for f in [PixelFormat::Rgba, PixelFormat::Bgra] {
            let r = raster(f);
            assert_eq!(r.to_gray().unwrap().into_raw(), vec![76, 149, 28, 255]);
            assert_eq!(
                r.to_rgb().unwrap().into_raw(),
                vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255]
            );
            let rgba = r.into_rgba().unwrap();
            assert_eq!(rgba.get_pixel(0, 1).0, [0, 0, 255, 255]);
        }
    }

    #[test]
    fn short_data_is_refused() {
        let mut r = raster(PixelFormat::Rgba);
        r.data.truncate(12);
        assert!(r.to_gray().is_none() && r.to_rgb().is_none());
        assert!(r.into_rgba().is_none());
    }

    #[test]
    fn bounds_of_dark_pixels() {
        let mut g = GrayImage::from_pixel(10, 8, image::Luma([255]));
        assert_eq!(content_bounds(&g, 235), None);
        g.put_pixel(2, 3, image::Luma([10]));
        g.put_pixel(6, 5, image::Luma([200]));
        assert_eq!(content_bounds(&g, 235), Some((2, 3, 7, 6)));
    }
}
