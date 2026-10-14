//! Images out: page images as PNG, JPEG or (multi-page) TIFF, each with
//! its resolution recorded, and CCITT Group 4 for black and white.

use std::io::{Cursor, Seek, Write};

use anyhow::Result;
use image::{GrayImage, ImageEncoder, RgbImage};
use tiff::encoder::{Compression, Rational, TiffEncoder, colortype};
use tiff::tags::{ResolutionUnit, Tag};

/// PNG, with `pHYs` set to `dpi`.
pub fn png(rgb: &RgbImage, dpi: u32) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, rgb.width(), rgb.height());
    let per_meter = (dpi as f64 / 0.0254).round() as u32;
    enc.set_pixel_dims(Some(png::PixelDimensions {
        xppu: per_meter,
        yppu: per_meter,
        unit: png::Unit::Meter,
    }));
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Balanced);
    enc.write_header()?.write_image_data(rgb.as_raw())?;
    Ok(out)
}

/// JPEG at `quality` (1–100), with its JFIF density set to `dpi`.
pub fn jpeg(rgb: &RgbImage, quality: u8, dpi: u32) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(
        Cursor::new(&mut out),
        quality.clamp(1, 100),
    );
    enc.set_pixel_density(image::codecs::jpeg::PixelDensity::dpi(
        dpi.min(u16::MAX as u32) as u16,
    ));
    enc.write_image(
        rgb.as_raw(),
        rgb.width(),
        rgb.height(),
        image::ExtendedColorType::Rgb8,
    )?;
    Ok(out)
}

/// One page of a TIFF.
pub enum TiffPage<'a> {
    Rgb(&'a RgbImage),
    Gray(&'a GrayImage),
    /// Thresholded at mid-gray and stored 1-bit, CCITT Group 4.
    Bilevel(&'a GrayImage),
}

/// A multi-page TIFF, a page at a time (so a long export never holds
/// more than one page). Color and gray are LZW; each page records its
/// resolution and its page number.
pub struct TiffWriter<W: Write + Seek> {
    enc: TiffEncoder<W>,
    dpi: u32,
    pages: u16,
    written: u16,
}

impl<W: Write + Seek> TiffWriter<W> {
    /// `pages` is the total, for each page's PageNumber tag.
    pub fn new(out: W, dpi: u32, pages: usize) -> Result<Self> {
        Ok(Self {
            enc: TiffEncoder::new(out)?.with_compression(Compression::Lzw),
            dpi,
            pages: pages.min(u16::MAX as usize) as u16,
            written: 0,
        })
    }

    pub fn add(&mut self, page: TiffPage) -> Result<()> {
        let res = Rational { n: self.dpi, d: 1 };
        let number = [self.written, self.pages];
        match page {
            TiffPage::Rgb(rgb) => {
                let mut im = self
                    .enc
                    .new_image::<colortype::RGB8>(rgb.width(), rgb.height())?;
                im.resolution(ResolutionUnit::Inch, res);
                im.encoder().write_tag(Tag::Unknown(297), &number[..])?;
                im.write_data(rgb.as_raw())?;
            }
            TiffPage::Gray(gray) => {
                let mut im = self
                    .enc
                    .new_image::<colortype::Gray8>(gray.width(), gray.height())?;
                im.resolution(ResolutionUnit::Inch, res);
                im.encoder().write_tag(Tag::Unknown(297), &number[..])?;
                im.write_data(gray.as_raw())?;
            }
            TiffPage::Bilevel(gray) => {
                let (w, h) = gray.dimensions();
                let data = g4(gray.as_raw(), w, h);
                let mut dir = self.enc.image_directory()?;
                let offset = dir.write_data(&data[..])?;
                dir.write_tag(Tag::NewSubfileType, 2u32)?;
                dir.write_tag(Tag::ImageWidth, w)?;
                dir.write_tag(Tag::ImageLength, h)?;
                dir.write_tag(Tag::BitsPerSample, 1u16)?;
                // CCITT T.6.
                dir.write_tag(Tag::Compression, 4u16)?;
                // WhiteIsZero, as fax data is.
                dir.write_tag(Tag::PhotometricInterpretation, 0u16)?;
                dir.write_tag(Tag::StripOffsets, u32::try_from(offset)?)?;
                dir.write_tag(Tag::SamplesPerPixel, 1u16)?;
                dir.write_tag(Tag::RowsPerStrip, h)?;
                dir.write_tag(Tag::StripByteCounts, data.len() as u32)?;
                dir.write_tag(Tag::XResolution, res)?;
                dir.write_tag(Tag::YResolution, Rational { n: self.dpi, d: 1 })?;
                dir.write_tag(Tag::ResolutionUnit, 2u16)?;
                // T6Options: no uncompressed mode.
                dir.write_tag(Tag::Unknown(293), 0u32)?;
                dir.write_tag(Tag::Unknown(297), &number[..])?;
                dir.finish()?;
            }
        }
        self.written += 1;
        Ok(())
    }
}

/// 8-bit gray rows as CCITT Group 4, thresholded at mid-gray.
pub fn g4(gray: &[u8], w: u32, h: u32) -> Vec<u8> {
    use fax::{Color, VecWriter, encoder::Encoder};
    let mut enc = Encoder::new(VecWriter::new());
    for row in gray.chunks_exact(w as usize).take(h as usize) {
        let pels = row
            .iter()
            .map(|&v| if v < 128 { Color::Black } else { Color::White });
        // VecWriter can't fail.
        let _ = enc.encode_line(pels, w as u16);
    }
    match enc.finish() {
        Ok(writer) => writer.finish(),
        Err(never) => match never {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn g4_round_trips_through_a_decoder() {
        // A 40×10 page: white with a black bar in rows 3–5, columns 8–30.
        let (w, h) = (40u32, 10u32);
        let mut gray = vec![255u8; (w * h) as usize];
        for y in 3..6 {
            for x in 8..31 {
                gray[(y * w + x) as usize] = 0;
            }
        }
        let data = g4(&gray, w, h);
        let mut rows = Vec::new();
        fax::decoder::decode_g4(data.into_iter(), w as u16, Some(h as u16), |t| {
            rows.push(fax::decoder::pels(t, w as u16).collect::<Vec<_>>());
        });
        assert_eq!(rows.len(), h as usize);
        for (y, row) in rows.iter().enumerate() {
            for (x, c) in row.iter().enumerate() {
                let black = (3..6).contains(&y) && (8..31).contains(&x);
                assert_eq!(*c == fax::Color::Black, black, "pel {x},{y}");
            }
        }
    }

    fn page() -> GrayImage {
        GrayImage::from_fn(40, 30, |x, y| {
            image::Luma([if (8..31).contains(&x) && (10..20).contains(&y) {
                0
            } else {
                255
            }])
        })
    }

    #[test]
    fn png_keeps_size_and_resolution() {
        let rgb = image::DynamicImage::ImageLuma8(page()).into_rgb8();
        let bytes = png(&rgb, 300).unwrap();
        let info = png::Decoder::new(Cursor::new(bytes)).read_info().unwrap();
        let info = info.info();
        assert_eq!((info.width, info.height), (40, 30));
        assert_eq!(info.pixel_dims.unwrap().xppu, 11811);
    }

    #[test]
    fn jpeg_records_its_density() {
        let rgb = image::DynamicImage::ImageLuma8(page()).into_rgb8();
        let pages = crate::io::read_bytes(jpeg(&rgb, 80, 150).unwrap()).unwrap();
        assert_eq!((pages[0].width, pages[0].height), (40, 30));
        assert_eq!(pages[0].dpi, (150.0, 150.0));
    }

    #[test]
    fn tiff_round_trips_through_the_reader() {
        let gray = page();
        let rgb = image::DynamicImage::ImageLuma8(gray.clone()).into_rgb8();
        let mut out = Cursor::new(Vec::new());
        let mut tiff = TiffWriter::new(&mut out, 200, 3).unwrap();
        tiff.add(TiffPage::Rgb(&rgb)).unwrap();
        tiff.add(TiffPage::Gray(&gray)).unwrap();
        tiff.add(TiffPage::Bilevel(&gray)).unwrap();
        let pages = crate::io::read_bytes(out.into_inner()).unwrap();
        assert_eq!(pages.len(), 3);
        for p in &pages {
            assert_eq!((p.width, p.height, p.dpi), (40, 30, (200.0, 200.0)));
        }
        let crate::io::Pixels::Bilevel(px) = &pages[2].pixels else {
            panic!("the G4 page should read back black and white");
        };
        assert_eq!(px, gray.as_raw());
    }
}
