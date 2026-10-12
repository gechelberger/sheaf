//! Turn a PDF into an image-only "scan": every page is rendered with
//! PDFium and written back as a grayscale image with no text, the kind of
//! file OCR exists for. Handy for trying Recognize Text on something with
//! known content.
//!
//! Usage: cargo run -p sheaf-ocr --example scanify -- in.pdf out.pdf [dpi]

use std::path::PathBuf;

use lopdf::{Dictionary, Document as Pdf, Object, Stream, dictionary};
use sheaf_core::{PageIndex, Rotation};
use sheaf_render::pdfium::PdfiumEngine;
use sheaf_render::{Job, PixelFormat, Renderer};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let (Some(input), Some(output)) = (args.next(), args.next()) else {
        eprintln!("usage: scanify in.pdf out.pdf [dpi]");
        std::process::exit(2);
    };
    let dpi: u32 = args.next().map(|d| d.parse()).transpose()?.unwrap_or(200);
    let engine = PdfiumEngine::open(PathBuf::from(&input), None)?;

    let mut pdf = Pdf::with_version("1.5");
    let pages_id = pdf.new_object_id();
    let mut kids = Vec::new();
    for i in 0..engine.page_count() {
        let size = engine.page_size(PageIndex(i))?;
        let width = (size.width * dpi as f32 / 72.0).round() as u32;
        let raster = engine.render(
            PageIndex(i),
            Job::Page {
                width,
                rotation: Rotation::R0,
            },
            PixelFormat::Rgba,
        )?;
        let gray: Vec<u8> = raster
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| ((77 * p[0] as u32 + 150 * p[1] as u32 + 29 * p[2] as u32) >> 8) as u8)
            .collect();
        let mut image = Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => raster.width as i64,
                "Height" => raster.height as i64,
                "ColorSpace" => "DeviceGray",
                "BitsPerComponent" => 8i64,
            },
            gray,
        );
        image.compress()?;
        let image_id = pdf.add_object(Object::Stream(image));
        let content = format!(
            "q {:.2} 0 0 {:.2} 0 0 cm /Im0 Do Q",
            size.width, size.height
        );
        let content_id = pdf.add_object(Stream::new(dictionary! {}, content.into_bytes()));
        let mut resources = Dictionary::new();
        resources.set("XObject", dictionary! { "Im0" => image_id });
        let page_id = pdf.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), size.width.into(), size.height.into()],
            "Resources" => resources,
            "Contents" => content_id,
        });
        kids.push(Object::Reference(page_id));
        eprintln!(
            "page {} rendered at {}×{}",
            i + 1,
            raster.width,
            raster.height
        );
    }
    let count = kids.len() as i64;
    pdf.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count }),
    );
    let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    pdf.trailer.set("Root", catalog);
    pdf.save(&output)?;
    eprintln!("wrote {output}");
    Ok(())
}
