//! M0 spike: build a PDF with lopdf, write it, reload it, and resave — or do the
//! reload/resave on a real file passed as the first argument.
//!
//!     cargo run -p sheaf-doc --example roundtrip [input.pdf]
//!
//! Output lands in target/spike/.

use std::time::Instant;

use anyhow::Result;
use lopdf::content::{Content, Operation};
use lopdf::{Document, Object, Stream, dictionary};
use sheaf_core::PageIndex;

fn synth_pdf(path: &std::path::Path) -> Result<()> {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
    });
    let resources_id = doc.add_object(dictionary! { "Font" => dictionary! { "F1" => font_id } });

    let mut kids = Vec::new();
    for n in 1..=3 {
        let content = Content {
            operations: vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F1".into(), 36.into()]),
                Operation::new("Td", vec![72.into(), 720.into()]),
                Operation::new("Tj", vec![Object::string_literal(format!("sheaf spike page {n}"))]),
                Operation::new("ET", vec![]),
            ],
        };
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode()?));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => content_id,
            // Rotate the middle page so page_geometry has something to find.
            "Rotate" => if n == 2 { 90 } else { 0 },
        });
        kids.push(page_id.into());
    }
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Kids" => kids, "Count" => 3, "Resources" => resources_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        }),
    );
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);
    doc.compress();
    doc.save(path)?;
    Ok(())
}

fn main() -> Result<()> {
    env_logger::init();
    let out_dir = std::path::Path::new("target/spike");
    std::fs::create_dir_all(out_dir)?;

    let input = match std::env::args().nth(1) {
        Some(p) => p.into(),
        None => {
            let p = out_dir.join("synth.pdf");
            synth_pdf(&p)?;
            println!("wrote synthetic {}", p.display());
            p
        }
    };

    let t = Instant::now();
    let mut doc = sheaf_doc::Document::open(&input)?;
    let load = t.elapsed();
    let n = doc.page_count();
    println!("{}: {n} pages, loaded in {load:?}", input.display());
    for i in 0..n.min(5) {
        let (size, rot) = doc.page_geometry(PageIndex(i))?;
        println!("  page {i}: {:.0}x{:.0} pt, rotate {}", size.width, size.height, rot.degrees());
    }

    let out = out_dir.join("roundtrip.pdf");
    let t = Instant::now();
    doc.save_as(&out)?;
    println!("resaved to {} in {:?}", out.display(), t.elapsed());

    // Prove the resave is loadable and has the same page count.
    let again = sheaf_doc::Document::open(&out)?;
    assert_eq!(again.page_count(), n, "page count changed on round-trip");
    println!("round-trip OK");
    Ok(())
}
