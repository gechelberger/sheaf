//! Rasters come back in the byte order they're asked for: a red page is
//! B=0,G=0,R=255 as BGRA and R=255,G=0,B=0 as RGBA.
//!
//! Needs libpdfium (`scripts/fetch-pdfium.sh`), like the app itself.

use lopdf::{Document as Pdf, Stream, dictionary};
use sheaf_core::{PageIndex, Rotation};
use sheaf_image::raster::PixelFormat;
use sheaf_render::pdfium::PdfiumEngine;
use sheaf_render::{Job, Renderer};

fn red_page() -> std::path::PathBuf {
    let mut pdf = Pdf::with_version("1.5");
    let pages_id = pdf.new_object_id();
    let content = pdf.add_object(Stream::new(
        dictionary! {},
        b"1 0 0 rg 0 0 100 100 re f".to_vec(),
    ));
    let page = pdf.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => content,
        "MediaBox" => vec![0.into(), 0.into(), 100.into(), 100.into()],
    });
    pdf.objects.insert(
        pages_id,
        dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 }.into(),
    );
    let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    pdf.trailer.set("Root", catalog);
    let path = std::env::temp_dir().join(format!("sheaf-red-{}.pdf", std::process::id()));
    pdf.save(&path).unwrap();
    path
}

#[test]
fn bgra_and_rgba_are_what_they_say() {
    let path = red_page();
    let engine = PdfiumEngine::open(&path, None).unwrap();
    let page = Job::Page {
        width: 20,
        rotation: Rotation::R0,
    };
    let tile = Job::Tile {
        page_width: 20,
        page_height: 20,
        x: 5,
        y: 5,
        w: 4,
        h: 4,
        rotation: Rotation::R0,
    };
    for job in [page, tile] {
        let bgra = engine.render(PageIndex(0), job, PixelFormat::Bgra).unwrap();
        assert_eq!(&bgra.data[..4], &[0, 0, 255, 255], "{job:?} as BGRA");
        let rgba = engine.render(PageIndex(0), job, PixelFormat::Rgba).unwrap();
        assert_eq!(&rgba.data[..4], &[255, 0, 0, 255], "{job:?} as RGBA");
    }
    let _ = std::fs::remove_file(path);
}
