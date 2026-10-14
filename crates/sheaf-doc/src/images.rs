//! Images as PDF pages: each image becomes one page showing it, sized
//! from its resolution or fitted to a paper size.
//!
//! The bytes are kept as small as the image allows: a JPEG goes in as it
//! is (DCTDecode, with its EXIF rotation as the page's `/Rotate`); a
//! black-and-white image as CCITT Group 4; anything else as 8-bit gray
//! or RGB under Flate. A multi-page TIFF gives one page per image in it.

use std::io::{Cursor, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, metadata::Orientation};
use lopdf::{Document as Pdf, Object, Stream, dictionary};

/// Resolution assumed when an image doesn't record one.
pub const DEFAULT_DPI: f32 = 96.0;

/// One image, ready to become a page.
#[derive(Clone)]
pub struct PageImage {
    pub width: u32,
    pub height: u32,
    /// Pixels per inch, horizontally and vertically.
    pub dpi: (f32, f32),
    /// Clockwise quarter turns to show it upright (JPEGs only; the rest are
    /// turned when decoded).
    pub rotate: u16,
    pub pixels: Pixels,
}

#[derive(Clone)]
pub enum Pixels {
    /// A JPEG file's bytes and its component count (1 gray, 3 color).
    Jpeg { bytes: Vec<u8>, components: u8 },
    /// 8-bit gray, row by row.
    Gray(Vec<u8>),
    /// 8-bit RGB, row by row.
    Rgb(Vec<u8>),
    /// Black and white: 8-bit gray with every pixel 0 or 255.
    Bilevel(Vec<u8>),
}

impl PageImage {
    /// Pixels the caller already has (a cleaned-up scan, say). Gray
    /// images whose pixels are all 0 or 255 are stored as black and white.
    pub fn from_image(image: DynamicImage, dpi: f32) -> PageImage {
        let (width, height) = (image.width(), image.height());
        let pixels = if image.color().has_color() {
            Pixels::Rgb(flatten_alpha(image).into_rgb8().into_raw())
        } else {
            let gray = flatten_alpha(image).into_luma8().into_raw();
            if gray.iter().all(|&v| v == 0 || v == 255) {
                Pixels::Bilevel(gray)
            } else {
                Pixels::Gray(gray)
            }
        };
        PageImage {
            width,
            height,
            dpi: (dpi, dpi),
            rotate: 0,
            pixels,
        }
    }

    /// Size in points as shown, after `rotate`.
    pub fn size_pt(&self) -> (f32, f32) {
        let w = self.width as f32 * 72.0 / self.dpi.0;
        let h = self.height as f32 * 72.0 / self.dpi.1;
        if self.rotate % 2 == 1 { (h, w) } else { (w, h) }
    }
}

/// How big each page is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PageFit {
    /// The image's own size at its resolution.
    Original,
    /// A sheet of this size in points (portrait; a landscape image turns
    /// it landscape), the image scaled to fit inside `margin` and centered.
    Paper {
        width: f32,
        height: f32,
        margin: f32,
    },
}

impl PageFit {
    pub const LETTER: PageFit = PageFit::Paper {
        width: 612.0,
        height: 792.0,
        margin: 0.0,
    };
    pub const A4: PageFit = PageFit::Paper {
        width: 595.28,
        height: 841.89,
        margin: 0.0,
    };
}

/// The pages in an image file: one, or one per image in a TIFF.
pub fn read(path: &Path) -> Result<Vec<PageImage>> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    read_bytes(bytes).with_context(|| format!("reading {}", path.display()))
}

pub fn read_bytes(bytes: Vec<u8>) -> Result<Vec<PageImage>> {
    let format = image::guess_format(&bytes).context("not an image sheaf knows")?;
    match format {
        ImageFormat::Jpeg => Ok(vec![jpeg(bytes)?]),
        ImageFormat::Tiff => tiff_pages(&bytes),
        _ => {
            let dpi = match format {
                ImageFormat::Png => png_dpi(&bytes),
                _ => None,
            };
            Ok(vec![decoded(&bytes, format, dpi)?])
        }
    }
}

/// Decode, turn upright, and keep the pixels.
fn decoded(bytes: &[u8], format: ImageFormat, dpi: Option<(f32, f32)>) -> Result<PageImage> {
    let mut decoder = ImageReader::with_format(Cursor::new(bytes), format).into_decoder()?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    let dpi = dpi.unwrap_or((DEFAULT_DPI, DEFAULT_DPI));
    let mut page = PageImage::from_image(image, dpi.0);
    page.dpi = if matches!(
        orientation,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    ) {
        (dpi.1, dpi.0)
    } else {
        dpi
    };
    Ok(page)
}

/// A JPEG goes in as is unless it's mirrored or CMYK.
fn jpeg(bytes: Vec<u8>) -> Result<PageImage> {
    let info = jpeg_info(&bytes).context("not a readable JPEG")?;
    let orientation = ImageReader::with_format(Cursor::new(&bytes), ImageFormat::Jpeg)
        .into_decoder()?
        .orientation()
        .unwrap_or(Orientation::NoTransforms);
    let rotate = match orientation {
        Orientation::NoTransforms => Some(0),
        Orientation::Rotate90 => Some(1),
        Orientation::Rotate180 => Some(2),
        Orientation::Rotate270 => Some(3),
        _ => None,
    };
    let dpi = info.dpi.unwrap_or((DEFAULT_DPI, DEFAULT_DPI));
    match (rotate, info.components) {
        (Some(rotate), c @ (1 | 3)) => Ok(PageImage {
            width: info.width,
            height: info.height,
            dpi,
            rotate,
            pixels: Pixels::Jpeg {
                bytes,
                components: c,
            },
        }),
        _ => decoded(&bytes, ImageFormat::Jpeg, info.dpi),
    }
}

struct JpegInfo {
    width: u32,
    height: u32,
    components: u8,
    dpi: Option<(f32, f32)>,
}

/// Size, components and JFIF density, from the markers.
fn jpeg_info(b: &[u8]) -> Option<JpegInfo> {
    if b.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut dpi = None;
    let mut i = 2;
    while i + 4 <= b.len() {
        if b[i] != 0xFF {
            return None;
        }
        let marker = b[i + 1];
        if marker == 0xFF {
            i += 1;
            continue;
        }
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        let seg = b.get(i + 4..i + 2 + len)?;
        match marker {
            // APP0 JFIF: units, x density, y density.
            0xE0 if seg.starts_with(b"JFIF\0") && seg.len() >= 12 => {
                let x = u16::from_be_bytes([seg[8], seg[9]]) as f32;
                let y = u16::from_be_bytes([seg[10], seg[11]]) as f32;
                dpi = match seg[7] {
                    1 if x > 0.0 && y > 0.0 => Some((x, y)),
                    2 if x > 0.0 && y > 0.0 => Some((x * 2.54, y * 2.54)),
                    _ => None,
                };
            }
            // Start of frame (any but DHT, JPG and DAC).
            0xC0..=0xCF if !matches!(marker, 0xC4 | 0xC8 | 0xCC) => {
                return Some(JpegInfo {
                    height: u16::from_be_bytes([*seg.get(1)?, *seg.get(2)?]) as u32,
                    width: u16::from_be_bytes([*seg.get(3)?, *seg.get(4)?]) as u32,
                    components: *seg.get(5)?,
                    dpi,
                });
            }
            _ => {}
        }
        i += 2 + len;
    }
    None
}

/// The `pHYs` resolution, when it's in pixels per meter.
fn png_dpi(b: &[u8]) -> Option<(f32, f32)> {
    let mut i = 8;
    while i + 8 <= b.len() {
        let len = u32::from_be_bytes(b[i..i + 4].try_into().ok()?) as usize;
        let kind = &b[i + 4..i + 8];
        if kind == b"pHYs" {
            let d = b.get(i + 8..i + 17)?;
            let x = u32::from_be_bytes(d[0..4].try_into().ok()?) as f32;
            let y = u32::from_be_bytes(d[4..8].try_into().ok()?) as f32;
            return (d[8] == 1 && x > 0.0 && y > 0.0).then_some((x * 0.0254, y * 0.0254));
        }
        if kind == b"IDAT" {
            return None;
        }
        i += 12 + len;
    }
    None
}

/// Every image in a TIFF.
fn tiff_pages(bytes: &[u8]) -> Result<Vec<PageImage>> {
    use tiff::ColorType as C;
    use tiff::decoder::{Decoder, DecodingResult as D};
    use tiff::tags::Tag;
    let mut dec = Decoder::new(Cursor::new(bytes))?;
    let mut out = Vec::new();
    loop {
        let (w, h) = dec.dimensions()?;
        let color = dec.colortype()?;
        let res = |dec: &mut Decoder<_>, tag| dec.get_tag_f32(tag).ok().filter(|v| *v > 0.0);
        let unit = dec
            .find_tag_unsigned::<u16>(Tag::ResolutionUnit)?
            .unwrap_or(2);
        let dpi = match (
            res(&mut dec, Tag::XResolution),
            res(&mut dec, Tag::YResolution),
        ) {
            (Some(x), y) => {
                let y = y.unwrap_or(x);
                match unit {
                    3 => (x * 2.54, y * 2.54),
                    2 => (x, y),
                    _ => (DEFAULT_DPI, DEFAULT_DPI),
                }
            }
            _ => (DEFAULT_DPI, DEFAULT_DPI),
        };
        let n = (w * h) as usize;
        let image = match (color, dec.read_image()?) {
            (C::Gray(1), D::U8(packed)) => {
                let row = w.div_ceil(8) as usize;
                let mut gray = Vec::with_capacity(n);
                for r in packed.chunks(row).take(h as usize) {
                    gray.extend((0..w as usize).map(|x| {
                        if r[x / 8] & (0x80 >> (x % 8)) != 0 {
                            255
                        } else {
                            0
                        }
                    }));
                }
                DynamicImage::ImageLuma8(image::GrayImage::from_raw(w, h, gray).context("size")?)
            }
            (C::Gray(8), D::U8(v)) => {
                DynamicImage::ImageLuma8(image::GrayImage::from_raw(w, h, v).context("size")?)
            }
            (C::Gray(16), D::U16(v)) => {
                DynamicImage::ImageLuma16(image::ImageBuffer::from_raw(w, h, v).context("size")?)
            }
            (C::GrayA(8), D::U8(v)) => {
                DynamicImage::ImageLumaA8(image::ImageBuffer::from_raw(w, h, v).context("size")?)
            }
            (C::RGB(8), D::U8(v)) => {
                DynamicImage::ImageRgb8(image::RgbImage::from_raw(w, h, v).context("size")?)
            }
            (C::RGB(16), D::U16(v)) => {
                DynamicImage::ImageRgb16(image::ImageBuffer::from_raw(w, h, v).context("size")?)
            }
            (C::RGBA(8), D::U8(v)) => {
                DynamicImage::ImageRgba8(image::RgbaImage::from_raw(w, h, v).context("size")?)
            }
            (C::RGBA(16), D::U16(v)) => {
                DynamicImage::ImageRgba16(image::ImageBuffer::from_raw(w, h, v).context("size")?)
            }
            (C::CMYK(8), D::U8(v)) => {
                let rgb: Vec<u8> = v
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .flat_map(|p| {
                        let k = 255 - p[3] as u16;
                        [0, 1, 2].map(|c| ((255 - p[c] as u16) * k / 255) as u8)
                    })
                    .collect();
                DynamicImage::ImageRgb8(image::RgbImage::from_raw(w, h, rgb).context("size")?)
            }
            (c, _) => bail!(
                "TIFF image {} is {c:?}, which sheaf can't read",
                out.len() + 1
            ),
        };
        let mut page = PageImage::from_image(image, dpi.0);
        page.dpi = dpi;
        out.push(page);
        if !dec.more_images() {
            return Ok(out);
        }
        dec.next_image()?;
    }
}

/// Alpha composited onto white: pages are paper.
fn flatten_alpha(image: DynamicImage) -> DynamicImage {
    if !image.color().has_alpha() {
        return image;
    }
    let mut rgba = image.into_rgba8();
    for p in rgba.pixels_mut() {
        let a = p[3] as u16;
        for c in 0..3 {
            p[c] = ((p[c] as u16 * a + 255 * (255 - a)) / 255) as u8;
        }
        p[3] = 255;
    }
    DynamicImage::ImageRgba8(rgba).into_rgb8().into()
}

/// A PDF with one page per image.
pub fn to_pdf(pages: &[PageImage], fit: PageFit) -> Result<Vec<u8>> {
    if pages.is_empty() {
        bail!("no images");
    }
    let mut pdf = Pdf::with_version("1.5");
    let pages_id = pdf.new_object_id();
    let mut kids = Vec::with_capacity(pages.len());
    for page in pages {
        let image = pdf.add_object(image_stream(page)?);
        let (iw, ih) = page.size_pt();
        // Page box and image rect as shown (upright).
        let (pw, ph, x, y, w, h) = match fit {
            PageFit::Original => (iw, ih, 0.0, 0.0, iw, ih),
            PageFit::Paper {
                width,
                height,
                margin,
            } => {
                let (pw, ph) = if (iw > ih) == (width > height) {
                    (width, height)
                } else {
                    (height, width)
                };
                let s = ((pw - 2.0 * margin) / iw).min((ph - 2.0 * margin) / ih);
                let (w, h) = (iw * s, ih * s);
                (pw, ph, (pw - w) / 2.0, (ph - h) / 2.0, w, h)
            }
        };
        // The file's page is the unrotated one; `/Rotate` turns it upright.
        // The image is centered, so its rect only swaps axes.
        let (pw, ph, x, y, w, h) = if page.rotate % 2 == 1 {
            (ph, pw, y, x, h, w)
        } else {
            (pw, ph, x, y, w, h)
        };
        let content = pdf.add_object(Stream::new(
            dictionary! {},
            format!("q {w:.4} 0 0 {h:.4} {x:.4} {y:.4} cm /Im0 Do Q").into_bytes(),
        ));
        let mut dict = dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), Object::Real(pw), Object::Real(ph)],
            "Contents" => content,
            "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => image } },
        };
        if page.rotate != 0 {
            dict.set("Rotate", page.rotate as i64 * 90);
        }
        kids.push(pdf.add_object(dict).into());
    }
    let count = kids.len() as i64;
    pdf.objects.insert(
        pages_id,
        dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count }.into(),
    );
    let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    pdf.trailer.set("Root", catalog);
    let mut out = Vec::new();
    pdf.save_to(&mut out)?;
    Ok(out)
}

fn image_stream(page: &PageImage) -> Result<Stream> {
    let (w, h) = (page.width as i64, page.height as i64);
    let base = |cs: &str, bits: i64| {
        dictionary! {
            "Type" => "XObject", "Subtype" => "Image",
            "Width" => w, "Height" => h,
            "ColorSpace" => cs, "BitsPerComponent" => bits,
        }
    };
    Ok(match &page.pixels {
        Pixels::Jpeg { bytes, components } => {
            let mut d = base(
                if *components == 1 {
                    "DeviceGray"
                } else {
                    "DeviceRGB"
                },
                8,
            );
            d.set("Filter", "DCTDecode");
            Stream::new(d, bytes.clone()).with_compression(false)
        }
        Pixels::Gray(v) => flate(base("DeviceGray", 8), v)?,
        Pixels::Rgb(v) => flate(base("DeviceRGB", 8), v)?,
        Pixels::Bilevel(v) => {
            let mut d = base("DeviceGray", 1);
            d.set("Filter", "CCITTFaxDecode");
            d.set(
                "DecodeParms",
                dictionary! { "K" => -1, "Columns" => w, "Rows" => h },
            );
            Stream::new(d, g4(v, page.width, page.height)).with_compression(false)
        }
    })
}

fn flate(mut dict: lopdf::Dictionary, data: &[u8]) -> Result<Stream> {
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(data)?;
    dict.set("Filter", "FlateDecode");
    Ok(Stream::new(dict, enc.finish()?).with_compression(false))
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

/// Read every file and make one PDF of all their pages.
pub fn files_to_pdf(paths: &[impl AsRef<Path>], fit: PageFit) -> Result<Vec<u8>> {
    let mut pages = Vec::new();
    for p in paths {
        pages.extend(read(p.as_ref())?);
    }
    to_pdf(&pages, fit)
}

/// Whether `path`'s extension is one of the image types `read` takes.
pub fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .and_then(ImageFormat::from_extension)
        .is_some_and(|f| {
            matches!(
                f,
                ImageFormat::Jpeg
                    | ImageFormat::Png
                    | ImageFormat::Tiff
                    | ImageFormat::Bmp
                    | ImageFormat::Gif
                    | ImageFormat::WebP
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x * 255 / w) as u8, (y * 255 / h) as u8, 128])
        }))
    }

    fn encode(image: &DynamicImage, format: ImageFormat) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        image.write_to(&mut out, format).unwrap();
        out.into_inner()
    }

    /// The single page's MediaBox, /Rotate, and image filter.
    fn page_facts(bytes: &[u8]) -> Vec<([f32; 4], i64, String)> {
        let pdf = Pdf::load_mem(bytes).unwrap();
        pdf.get_pages()
            .values()
            .map(|&id| {
                let page = pdf.get_dictionary(id).unwrap();
                let mb: Vec<f32> = page
                    .get(b"MediaBox")
                    .unwrap()
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|o| o.as_float().unwrap())
                    .collect();
                let rot = page.get(b"Rotate").and_then(|o| o.as_i64()).unwrap_or(0);
                let xo = page
                    .get(b"Resources")
                    .and_then(|r| r.as_dict())
                    .unwrap()
                    .get(b"XObject")
                    .and_then(|x| x.as_dict())
                    .unwrap();
                let im = pdf
                    .get_object(xo.get(b"Im0").unwrap().as_reference().unwrap())
                    .unwrap()
                    .as_stream()
                    .unwrap();
                let filter =
                    String::from_utf8(im.dict.get(b"Filter").unwrap().as_name().unwrap().to_vec())
                        .unwrap();
                ([mb[0], mb[1], mb[2], mb[3]], rot, filter)
            })
            .collect()
    }

    #[test]
    fn jpeg_passes_through() {
        let bytes = encode(&gradient(192, 96), ImageFormat::Jpeg);
        let pages = read_bytes(bytes.clone()).unwrap();
        assert!(
            matches!(&pages[0].pixels, Pixels::Jpeg { bytes: b, components: 3 } if *b == bytes)
        );
        let pdf = to_pdf(&pages, PageFit::Original).unwrap();
        // 192 px at 96 dpi is 2 in.
        assert_eq!(
            page_facts(&pdf),
            vec![([0.0, 0.0, 144.0, 72.0], 0, "DCTDecode".into())]
        );
    }

    #[test]
    fn jfif_density_sets_the_size() {
        let mut bytes = encode(&gradient(300, 600), ImageFormat::Jpeg);
        // The encoder writes a JFIF APP0 right after SOI: set 300 dpi.
        assert_eq!(&bytes[6..11], b"JFIF\0");
        bytes[13] = 1;
        bytes[14..16].copy_from_slice(&300u16.to_be_bytes());
        bytes[16..18].copy_from_slice(&300u16.to_be_bytes());
        let pages = read_bytes(bytes).unwrap();
        assert_eq!(pages[0].size_pt(), (72.0, 144.0));
    }

    #[test]
    fn png_gray_and_bilevel() {
        let gray = DynamicImage::ImageLuma8(image::GrayImage::from_fn(40, 20, |x, _| {
            image::Luma([(x * 6) as u8])
        }));
        let pages = read_bytes(encode(&gray, ImageFormat::Png)).unwrap();
        assert!(matches!(pages[0].pixels, Pixels::Gray(_)));
        let bw = DynamicImage::ImageLuma8(image::GrayImage::from_fn(40, 20, |x, y| {
            image::Luma([if (x + y) % 3 == 0 { 0 } else { 255 }])
        }));
        let pages = read_bytes(encode(&bw, ImageFormat::Png)).unwrap();
        assert!(matches!(pages[0].pixels, Pixels::Bilevel(_)));
        let pdf = to_pdf(&pages, PageFit::Original).unwrap();
        assert_eq!(page_facts(&pdf)[0].2, "CCITTFaxDecode");
        let rgb = read_bytes(encode(&gradient(10, 10), ImageFormat::Png)).unwrap();
        assert_eq!(
            page_facts(&to_pdf(&rgb, PageFit::Original).unwrap())[0].2,
            "FlateDecode"
        );
    }

    #[test]
    fn fits_paper_turning_it_for_landscape() {
        let page = PageImage::from_image(gradient(200, 100), 100.0);
        let pdf = to_pdf(&[page], PageFit::LETTER).unwrap();
        assert_eq!(page_facts(&pdf)[0].0, [0.0, 0.0, 792.0, 612.0]);
    }

    #[test]
    fn multi_page_tiff() {
        use tiff::encoder::{TiffEncoder, colortype};
        let mut out = Cursor::new(Vec::new());
        let mut enc = TiffEncoder::new(&mut out).unwrap();
        let a = gradient(30, 20).into_rgb8();
        enc.write_image::<colortype::RGB8>(30, 20, a.as_raw())
            .unwrap();
        let b = vec![200u8; 50 * 40];
        enc.write_image::<colortype::Gray8>(50, 40, &b).unwrap();
        let pages = read_bytes(out.into_inner()).unwrap();
        assert_eq!(pages.len(), 2);
        assert_eq!((pages[1].width, pages[1].height), (50, 40));
        let pdf = to_pdf(&pages, PageFit::A4).unwrap();
        assert_eq!(page_facts(&pdf).len(), 2);
    }

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
}
