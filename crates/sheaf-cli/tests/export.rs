//! `sheaf-cli export` end to end, through the binary.
//!
//! Needs libpdfium (`scripts/fetch-pdfium.sh`), like the app itself.

use std::path::{Path, PathBuf};
use std::process::Command;

use lopdf::{Document as Pdf, Object, Stream, dictionary};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sheaf-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A letter-size PDF whose pages say "Page 1", "Page 2", ….
fn pages_pdf(dir: &Path, n: usize) -> PathBuf {
    let mut pdf = Pdf::with_version("1.5");
    let pages_id = pdf.new_object_id();
    let font = pdf.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
    });
    let kids: Vec<Object> = (1..=n)
        .map(|i| {
            let content = pdf.add_object(Stream::new(
                dictionary! {},
                format!("BT /F1 24 Tf 72 700 Td (Page {i}) Tj ET").into_bytes(),
            ));
            pdf.add_object(dictionary! {
                "Type" => "Page", "Parent" => pages_id, "Contents" => content,
                "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
                "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } },
            })
            .into()
        })
        .collect();
    pdf.objects.insert(
        pages_id,
        dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => n as i64 }.into(),
    );
    let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    pdf.trailer.set("Root", catalog);
    let path = dir.join("doc.pdf");
    pdf.save(&path).unwrap();
    path
}

fn sheaf(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_sheaf-cli"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn exports_chosen_pages_as_png() {
    let dir = scratch("png");
    let pdf = pages_pdf(&dir, 3);
    let out = dir.join("out");
    let run = sheaf(&[
        "export",
        pdf.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
        "--pages",
        "2-",
        "--dpi",
        "72",
        "-q",
    ]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let listed: Vec<String> = String::from_utf8(run.stdout)
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    assert_eq!(
        listed,
        vec![
            out.join("doc_Page_1.png").display().to_string(),
            out.join("doc_Page_2.png").display().to_string(),
        ]
    );
    let im = image_size(&out.join("doc_Page_1.png"));
    assert_eq!(im, (612, 792));
    // A second run refuses to overwrite without --force.
    let again = sheaf(&["export", pdf.to_str().unwrap(), "-o", out.to_str().unwrap()]);
    assert_eq!(again.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&again.stderr).contains("--force"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn exports_text_to_a_named_file() {
    let dir = scratch("text");
    let pdf = pages_pdf(&dir, 2);
    let out = dir.join("words.txt");
    let run = sheaf(&[
        "export",
        pdf.to_str().unwrap(),
        "-f",
        "txt",
        "-o",
        out.to_str().unwrap(),
    ]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        "Page 1\n\u{c}\nPage 2\n"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bad_arguments_fail_with_2() {
    assert_eq!(sheaf(&["export"]).status.code(), Some(2));
    assert_eq!(sheaf(&["frobnicate"]).status.code(), Some(2));
    let dir = scratch("range");
    let pdf = pages_pdf(&dir, 2);
    let run = sheaf(&["export", pdf.to_str().unwrap(), "--pages", "5"]);
    assert_eq!(run.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&run.stderr).contains("out of range"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// PNG width and height from the IHDR chunk.
fn image_size(path: &Path) -> (u32, u32) {
    let b = std::fs::read(path).unwrap();
    assert_eq!(&b[1..4], b"PNG");
    let be = |o: usize| u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    (be(16), be(20))
}
