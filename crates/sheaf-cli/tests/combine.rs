//! `sheaf-cli combine` end to end, through the binary.
//!
//! Needs libpdfium (`scripts/fetch-pdfium.sh`), like the app itself.

use std::path::{Path, PathBuf};
use std::process::Command;

use lopdf::{Document as Pdf, Object, Stream, dictionary};

/// A PDF of `n` pages saying "<tag> page i", bookmarked "<tag> i" each.
fn pdf(dir: &Path, tag: &str, n: usize) -> PathBuf {
    let mut pdf = Pdf::with_version("1.5");
    let pages_id = pdf.new_object_id();
    let font = pdf.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
    });
    let mut page_ids = Vec::new();
    for i in 1..=n {
        let content = pdf.add_object(Stream::new(
            dictionary! {},
            format!("BT /F1 24 Tf 72 700 Td ({tag} page {i}) Tj ET").into_bytes(),
        ));
        page_ids.push(pdf.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => content,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } },
        }));
    }
    pdf.objects.insert(
        pages_id,
        dictionary! {
            "Type" => "Pages",
            "Kids" => page_ids.iter().map(|&p| p.into()).collect::<Vec<Object>>(),
            "Count" => n as i64,
        }
        .into(),
    );
    // A flat outline: one item per page.
    let outlines = pdf.new_object_id();
    let items: Vec<_> = (0..n).map(|_| pdf.new_object_id()).collect();
    for (i, &id) in items.iter().enumerate() {
        let mut d = dictionary! {
            "Title" => Object::string_literal(format!("{tag} {}", i + 1)),
            "Parent" => outlines,
            "Dest" => vec![page_ids[i].into(), "Fit".into()],
        };
        if i > 0 {
            d.set("Prev", items[i - 1]);
        }
        if i + 1 < n {
            d.set("Next", items[i + 1]);
        }
        pdf.objects.insert(id, d.into());
    }
    pdf.objects.insert(
        outlines,
        dictionary! {
            "Type" => "Outlines", "First" => items[0], "Last" => items[n - 1],
            "Count" => n as i64,
        }
        .into(),
    );
    let catalog = pdf.add_object(dictionary! {
        "Type" => "Catalog", "Pages" => pages_id, "Outlines" => outlines,
    });
    pdf.trailer.set("Root", catalog);
    let path = dir.join(format!("{tag}.pdf"));
    pdf.save(&path).unwrap();
    path
}

fn sheaf(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_sheaf-cli"))
        .args(args)
        .output()
        .unwrap()
}

/// Top-level outline titles and, for each, its children's titles.
fn outline(path: &Path) -> Vec<(String, Vec<String>)> {
    let pdf = Pdf::load(path).unwrap();
    let title =
        |d: &lopdf::Dictionary| lopdf::decode_text_string(d.get(b"Title").unwrap()).unwrap();
    let kids = |parent: &lopdf::Dictionary| {
        let mut out = Vec::new();
        let mut next = parent.get(b"First").ok().map(|o| o.as_reference().unwrap());
        while let Some(id) = next {
            let d = pdf.get_dictionary(id).unwrap().clone();
            next = d.get(b"Next").ok().map(|o| o.as_reference().unwrap());
            out.push(d);
        }
        out
    };
    let root = pdf.catalog().unwrap();
    let outlines = pdf
        .get_dictionary(root.get(b"Outlines").unwrap().as_reference().unwrap())
        .unwrap();
    kids(outlines)
        .iter()
        .map(|d| (title(d), kids(d).iter().map(title).collect()))
        .collect()
}

#[test]
fn combines_pdfs_and_images_with_bookmarks() {
    let dir = std::env::temp_dir().join(format!("sheaf-cli-combine-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let a = pdf(&dir, "A", 3);
    let b = pdf(&dir, "B", 4);
    let img = dir.join("photo.png");
    image::RgbImage::from_pixel(96, 48, image::Rgb([10, 200, 10]))
        .save(&img)
        .unwrap();
    let out = dir.join("out.pdf");
    let run = sheaf(&[
        "combine",
        "-q",
        "-o",
        out.to_str().unwrap(),
        a.to_str().unwrap(),
        &format!("{}:2,4", b.display()),
        img.to_str().unwrap(),
    ]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let pdf = Pdf::load(&out).unwrap();
    assert_eq!(pdf.get_pages().len(), 3 + 2 + 1);
    assert_eq!(
        outline(&out),
        vec![
            ("A".into(), vec!["A 1".into(), "A 2".into(), "A 3".into()]),
            ("B".into(), vec!["B 2".into(), "B 4".into()]),
            ("photo".into(), vec![]),
        ]
    );
    // The text is what was asked for, in order.
    let text = dir.join("out.txt");
    let t = sheaf(&[
        "export",
        "-q",
        out.to_str().unwrap(),
        "-f",
        "text",
        "-o",
        text.to_str().unwrap(),
    ]);
    assert!(t.status.success());
    let pages: Vec<String> = std::fs::read_to_string(&text)
        .unwrap()
        .split('\u{c}')
        .map(|p| p.trim().to_string())
        .collect();
    assert_eq!(
        pages,
        vec![
            "A page 1", "A page 2", "A page 3", "B page 2", "B page 4", ""
        ]
    );
    // Without per-file bookmarks, the files' own sit at the top.
    let flat = dir.join("flat.pdf");
    let run = sheaf(&[
        "combine",
        "-q",
        "--no-file-bookmarks",
        "-o",
        flat.to_str().unwrap(),
        &format!("{}:3", a.display()),
        b.to_str().unwrap(),
    ]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let top: Vec<String> = outline(&flat).into_iter().map(|(t, _)| t).collect();
    assert_eq!(top, vec!["A 3", "B 1", "B 2", "B 3", "B 4"]);
    let _ = std::fs::remove_dir_all(&dir);
}
