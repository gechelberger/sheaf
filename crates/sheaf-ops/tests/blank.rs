//! Finding blank pages in a mixed document.
//!
//! Needs libpdfium (`scripts/fetch-pdfium.sh`), like the app itself.

use std::path::{Path, PathBuf};

use lopdf::{Document as Pdf, Object, Stream, dictionary};
use sheaf_doc::Document;
use sheaf_ops::blank::{BlankParams, Reason};

/// Pages: 0 text, 1 empty, 2 "intentionally left blank", 3 a link on an
/// empty page, 4 a lone signature stroke, 5 another empty page.
fn mixed(dir: &std::path::Path) -> PathBuf {
    let mut pdf = Pdf::with_version("1.5");
    let pages_id = pdf.new_object_id();
    let font = pdf.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
    });
    let contents = [
        "BT /F1 12 Tf 72 700 Td (Statement of account for the year) Tj 0 -16 Td (More lines of real content here) Tj ET".to_string(),
        String::new(),
        "BT /F1 12 Tf 200 400 Td (This page intentionally left blank) Tj ET".to_string(),
        String::new(),
        "0 0 0 RG 2 w 380 120 m 520 128 l S".to_string(),
        String::new(),
    ];
    let kids: Vec<Object> = contents
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let content = pdf.add_object(Stream::new(dictionary! {}, c.clone().into_bytes()));
            let mut page = dictionary! {
                "Type" => "Page", "Parent" => pages_id, "Contents" => content,
                "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
                "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } },
            };
            if i == 3 {
                let link = pdf.add_object(dictionary! {
                    "Type" => "Annot", "Subtype" => "Link",
                    "Rect" => vec![100.into(), 100.into(), 200.into(), 120.into()],
                });
                page.set("Annots", vec![link.into()]);
            }
            pdf.add_object(page).into()
        })
        .collect();
    let n = kids.len() as i64;
    pdf.objects.insert(
        pages_id,
        dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => n }.into(),
    );
    let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    pdf.trailer.set("Root", catalog);
    let path = dir.join("mixed.pdf");
    pdf.save(&path).unwrap();
    path
}

/// `path` opened as the app opens it.
fn open(path: &Path) -> Document {
    sheaf_doc::open::file(path, None).unwrap().doc
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sheaf-blank-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn finds_blanks_and_spares_content() {
    let dir = scratch("mixed");
    let doc = open(&mixed(&dir));
    let found = sheaf_ops::blank::find(
        &doc.snapshot(),
        None,
        BlankParams::default(),
        &sheaf_ops::NoProgress,
    )
    .unwrap();
    let got: Vec<(usize, Reason)> = found.iter().map(|c| (c.index, c.reason)).collect();
    assert_eq!(
        got,
        vec![
            (1, Reason::Blank),
            (2, Reason::IntentionallyBlank),
            (4, Reason::NearlyBlank),
            (5, Reason::Blank),
        ]
    );
    assert!(!found[2].drop_by_default());
    // Keeping intentional ones, and a range.
    let params = BlankParams {
        keep_intentional: true,
        ..Default::default()
    };
    let found = sheaf_ops::blank::find(
        &doc.snapshot(),
        Some(&[0, 1, 2, 3]),
        params,
        &sheaf_ops::NoProgress,
    )
    .unwrap();
    assert_eq!(found.iter().map(|c| c.index).collect::<Vec<_>>(), vec![1]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_noisy_scanned_blank_is_blank() {
    let dir = scratch("scan");
    // An off-white "scan" with noise and faint bleed-through.
    let mut img = image::GrayImage::from_pixel(1275, 1650, image::Luma([228]));
    let mut s = 3u32;
    for p in img.pixels_mut() {
        s = s.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        p[0] = (p[0] as i32 + ((s >> 16) % 13) as i32 - 6) as u8;
    }
    for line in 0..40u32 {
        for x in 150..1100 {
            for y in 0..10 {
                img.put_pixel(x, 150 + line * 34 + y, image::Luma([200]));
            }
        }
    }
    let page = sheaf_image::io::PageImage::from_image(image::DynamicImage::ImageLuma8(img), 150.0);
    let bytes = sheaf_doc::images::to_pdf(&[page], sheaf_doc::images::PageFit::Original).unwrap();
    let path = dir.join("scan.pdf");
    std::fs::write(&path, bytes).unwrap();
    let doc = open(&path);
    let found = sheaf_ops::blank::find(
        &doc.snapshot(),
        None,
        BlankParams::default(),
        &sheaf_ops::NoProgress,
    )
    .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].reason, Reason::Blank);
    let _ = std::fs::remove_dir_all(&dir);
}
