//! `sheaf-cli remove-blanks` end to end, through the binary.
//!
//! Needs libpdfium (`scripts/fetch-pdfium.sh`).

use std::path::PathBuf;
use std::process::Command;

use lopdf::{Document as Pdf, Object, Stream, dictionary};

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

fn sheaf(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_sheaf-cli"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn removes_blanks_and_keeps_the_rejects() {
    let dir = std::env::temp_dir().join(format!("sheaf-cli-blanks-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let input = mixed(&dir);
    let input = input.to_str().unwrap();
    let kept = dir.join("kept.pdf");
    let rejects = dir.join("rejects.pdf");

    // A dry run lists and writes nothing.
    let run = sheaf(&[
        "remove-blanks",
        "-q",
        input,
        "-o",
        kept.to_str().unwrap(),
        "--dry-run",
    ]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let listed = String::from_utf8(run.stdout).unwrap();
    assert_eq!(
        listed
            .lines()
            .map(|l| l.split('\t').collect::<Vec<_>>())
            .map(|v| (v[0].to_string(), v[2].to_string()))
            .collect::<Vec<_>>(),
        [
            ("2", "removed"),
            ("3", "removed"),
            ("6", "removed"),
            ("5", "kept")
        ]
        .map(|(a, b)| (a.to_string(), b.to_string()))
    );
    assert!(!kept.exists());

    let run = sheaf(&[
        "remove-blanks",
        "-q",
        input,
        "-o",
        kept.to_str().unwrap(),
        "--rejects",
        rejects.to_str().unwrap(),
    ]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(Pdf::load(&kept).unwrap().get_pages().len(), 3);
    let rej = Pdf::load(&rejects).unwrap();
    assert_eq!(rej.get_pages().len(), 3);
    let titles: Vec<String> = rej
        .get_toc()
        .unwrap()
        .toc
        .into_iter()
        .map(|t| t.title)
        .collect();
    assert_eq!(
        titles,
        ["p. 2 — blank", "p. 3 — intentionally blank", "p. 6 — blank"]
    );

    // Won't overwrite; keeps intentional pages when asked.
    let again = sheaf(&["remove-blanks", "-q", input, "-o", kept.to_str().unwrap()]);
    assert_eq!(again.status.code(), Some(1));
    let run = sheaf(&[
        "remove-blanks",
        "-q",
        input,
        "-o",
        kept.to_str().unwrap(),
        "--force",
        "--keep-intentional",
        "--nearly-blank",
    ]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(Pdf::load(&kept).unwrap().get_pages().len(), 3);
    let _ = std::fs::remove_dir_all(&dir);
}
