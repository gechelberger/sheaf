//! Images as PDF pages: each image becomes one page showing it, sized
//! from its resolution or fitted to a paper size.
//!
//! The bytes are kept as small as the image allows: a JPEG goes in as it
//! is (DCTDecode, with its EXIF rotation as the page's `/Rotate`); a
//! black-and-white image as CCITT Group 4; anything else as 8-bit gray
//! or RGB under Flate. Reading and decoding the files is
//! [`sheaf_image::io`]'s.

use std::io::Write;
use std::path::Path;

use anyhow::{Result, bail};
use lopdf::{Document as Pdf, Object, Stream, dictionary};
use sheaf_image::encode::g4;
use sheaf_image::io::{PageImage, Pixels};

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

/// Where an image goes on its page, as shown (upright, top-left origin,
/// points).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// Page width and height.
    pub page: (f32, f32),
    pub image: crate::Box2,
}

pub fn placement(page: &PageImage, fit: PageFit) -> Placement {
    let (iw, ih) = page.size_pt();
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
    Placement {
        page: (pw, ph),
        image: crate::Box2 { x, y, w, h },
    }
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
        let place = placement(page, fit);
        let (pw, ph) = place.page;
        let (x, w, h) = (place.image.x, place.image.w, place.image.h);
        // PDF space runs bottom up.
        let y = ph - place.image.y - h;
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
            "MediaBox" => vec![0.into(), 0.into(), Object::from(pw), Object::from(ph)],
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

pub(crate) fn image_stream(page: &PageImage) -> Result<Stream> {
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

/// Read every file and make one PDF of all their pages.
pub fn files_to_pdf(paths: &[impl AsRef<Path>], fit: PageFit) -> Result<Vec<u8>> {
    let mut pages = Vec::new();
    for p in paths {
        pages.extend(sheaf_image::io::read(p.as_ref())?);
    }
    to_pdf(&pages, fit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageFormat};
    use sheaf_image::io::read_bytes;
    use std::io::Cursor;

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
                    .map(|o| o.as_float().unwrap() as f32)
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
    fn encodes_jpeg_by_color() {
        let rgb = PageImage::jpeg(&gradient(40, 30), 150.0, 80).unwrap();
        assert!(matches!(rgb.pixels, Pixels::Jpeg { components: 3, .. }));
        let gray = PageImage::jpeg(&gradient(40, 30).grayscale(), 150.0, 80).unwrap();
        assert!(matches!(gray.pixels, Pixels::Jpeg { components: 1, .. }));
        assert_eq!(
            page_facts(&to_pdf(&[gray], PageFit::Original).unwrap())[0].2,
            "DCTDecode"
        );
    }

    #[test]
    fn fits_paper_turning_it_for_landscape() {
        let page = PageImage::from_image(gradient(200, 100), 100.0);
        let pdf = to_pdf(&[page], PageFit::LETTER).unwrap();
        assert_eq!(page_facts(&pdf)[0].0, [0.0, 0.0, 792.0, 612.0]);
    }
}
