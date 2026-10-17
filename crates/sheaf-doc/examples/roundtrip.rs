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
use sheaf_doc::store::{BlobKind, Store};

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
                Operation::new(
                    "Tj",
                    vec![Object::string_literal(format!("sheaf spike page {n}"))],
                ),
                Operation::new("ET", vec![]),
            ],
        };
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode()?));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => content_id,
            // Rotate the middle page so its geometry has something to show.
            "Rotate" => if n == 2 { 90 } else { 0 },
        });
        kids.push(page_id.into());
    }
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Kids" => kids.clone(), "Count" => 3, "Resources" => resources_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        }),
    );
    // A small bookmark tree: Part 1 (-> p1) with two children, Part 2 (-> p3).
    let outlines_id = doc.new_object_id();
    let (part1, part2, ch1, ch2) = (
        doc.new_object_id(),
        doc.new_object_id(),
        doc.new_object_id(),
        doc.new_object_id(),
    );
    let dest = |i: usize| {
        vec![
            kids[i].clone(),
            "XYZ".into(),
            Object::Null,
            Object::Null,
            Object::Null,
        ]
    };
    doc.objects.insert(part1, Object::Dictionary(dictionary! {
        "Title" => Object::string_literal("Part 1 — Opening"), "Parent" => outlines_id, "Next" => part2,
        "First" => ch1, "Last" => ch2, "Count" => 2, "Dest" => dest(0),
    }));
    doc.objects.insert(ch1, Object::Dictionary(dictionary! {
        "Title" => Object::string_literal("Section 1.1"), "Parent" => part1, "Next" => ch2, "Dest" => dest(0),
    }));
    doc.objects.insert(ch2, Object::Dictionary(dictionary! {
        "Title" => Object::string_literal("Section 1.2 (rotated page)"), "Parent" => part1, "Prev" => ch1, "Dest" => dest(1),
    }));
    doc.objects.insert(part2, Object::Dictionary(dictionary! {
        "Title" => Object::string_literal("Part 2 — Closing"), "Parent" => outlines_id, "Prev" => part1, "Dest" => dest(2),
    }));
    doc.objects.insert(
        outlines_id,
        Object::Dictionary(dictionary! {
            "Type" => "Outlines", "First" => part1, "Last" => part2, "Count" => 4,
        }),
    );
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog", "Pages" => pages_id, "Outlines" => outlines_id, "PageMode" => "UseOutlines",
    });
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
    let mut doc = sheaf_doc::file::PdfFile::open(&input)?;
    let load = t.elapsed();
    let n = doc.page_count();
    println!("{}: {n} pages, loaded in {load:?}", input.display());
    let src = Store::new().open(sheaf_core::read_file(&input)?, BlobKind::file(&input, None))?;
    for i in 0..n.min(5) {
        let geom = src.geom(i);
        let size = geom.shown();
        println!(
            "  page {i}: {:.0}x{:.0} pt, rotate {}",
            size.width,
            size.height,
            geom.rotate.degrees()
        );
    }

    let out = out_dir.join("roundtrip.pdf");
    let t = Instant::now();
    doc.save_as(&out)?;
    println!("resaved to {} in {:?}", out.display(), t.elapsed());

    // Prove the resave is loadable and has the same page count.
    let again = sheaf_doc::file::PdfFile::open(&out)?;
    assert_eq!(again.page_count(), n, "page count changed on round-trip");
    println!("round-trip OK");
    Ok(())
}
