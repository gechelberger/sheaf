//! `sheaf-cli comments` end to end, through the binary.
//!
//! Needs libpdfium (`scripts/fetch-pdfium.sh`), like the app itself.

use std::path::{Path, PathBuf};
use std::process::Command;

use lopdf::{Document as Pdf, Object, Stream, dictionary};

fn scratch(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("sheaf-cli-comments-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Two pages: Ann's note on page 1 with Bo's reply, and Bo's highlight
/// on page 2.
fn pdf(dir: &Path) -> PathBuf {
    let mut pdf = Pdf::with_version("1.5");
    let pages_id = pdf.new_object_id();
    let mut kids = Vec::new();
    let mut annots: Vec<Vec<Object>> = vec![Vec::new(), Vec::new()];
    let mut page_ids = Vec::new();
    for _ in 0..2 {
        let content = pdf.add_object(Stream::new(dictionary! {}, Vec::new()));
        let id = pdf.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => content,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        });
        page_ids.push(id);
        kids.push(Object::Reference(id));
    }
    let comment = |pdf: &mut Pdf, subtype: &str, who: &str, name: &str, text: &str| {
        pdf.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => subtype,
            "Rect" => vec![72.into(), 600.into(), 200.into(), 620.into()],
            "Contents" => Object::string_literal(text),
            "T" => Object::string_literal(who),
            "NM" => Object::string_literal(name),
            "M" => Object::string_literal("D:20261001120000"),
        })
    };
    let note = comment(&mut pdf, "Text", "Ann", "n1", "Tie to the ledger, please");
    let reply = comment(&mut pdf, "Text", "Bo", "r1", "Tied");
    pdf.get_dictionary_mut(reply).unwrap().set("IRT", note);
    let mark = comment(&mut pdf, "Highlight", "Bo", "h1", "");
    annots[0].extend([note.into(), reply.into()]);
    annots[1].push(mark.into());
    for (id, list) in page_ids.iter().zip(annots) {
        pdf.get_dictionary_mut(*id).unwrap().set("Annots", list);
    }
    pdf.objects.insert(
        pages_id,
        dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => 2 }.into(),
    );
    let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    pdf.trailer.set("Root", catalog);
    let path = dir.join("review.pdf");
    pdf.save(&path).unwrap();
    path
}

fn sheaf(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_sheaf-cli"))
        .args(args)
        .output()
        .unwrap()
}

fn ok(run: &std::process::Output) -> String {
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    String::from_utf8(run.stdout.clone()).unwrap()
}

#[test]
fn lists_comments_as_csv_and_text() {
    let dir = scratch("csv");
    let pdf = pdf(&dir);
    let p = pdf.to_str().unwrap();
    let csv = ok(&sheaf(&["comments", "csv", p]));
    assert_eq!(
        csv.lines().collect::<Vec<_>>(),
        [
            "Number,Page,Type,Author,Date,Status,Text",
            "1,1,Note,Ann,2026-10-01 12:00,,\"Tie to the ledger, please\"",
            "1.1,1,Note,Bo,2026-10-01 12:00,,Tied",
            "2,2,Highlight,Bo,2026-10-01 12:00,,Highlight",
        ]
    );
    // Only notes, chosen columns, sorted by author.
    let csv = ok(&sheaf(&[
        "comments",
        "csv",
        p,
        "--type",
        "note",
        "-c",
        "number,author,reply-to",
        "--sort",
        "author",
    ]));
    assert_eq!(csv, "Number,Author,Reply to\r\n1,Ann,\r\n1.1,Bo,1\r\n");
    let text = ok(&sheaf(&["comments", "text", p, "--pages", "2"]));
    assert!(
        text.starts_with("review.pdf\n1 comment, by page\n\nPage 2\n"),
        "{text}"
    );
    let run = sheaf(&["comments", "csv", p, "--type", "stamp"]);
    assert_eq!(run.status.code(), Some(1));
    let err = String::from_utf8_lossy(&run.stderr);
    assert!(err.contains("there are: Highlight, Note"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn writes_the_summary_beside_the_file() {
    let dir = scratch("summary");
    let pdf = pdf(&dir);
    let p = pdf.to_str().unwrap();
    let out = ok(&sheaf(&["comments", "summary", p, "-l", "pages"]));
    let written = dir.join("review - comments.pdf");
    assert_eq!(out.trim(), written.to_str().unwrap());
    assert_eq!(Pdf::load(&written).unwrap().get_pages().len(), 2);
    // It's there now: only --force replaces it.
    let again = sheaf(&["comments", "summary", p]);
    assert_eq!(again.status.code(), Some(1));
    ok(&sheaf(&["comments", "summary", p, "--force"]));
    assert_eq!(Pdf::load(&written).unwrap().get_pages().len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Two blank pages.
fn blank(dir: &Path) -> PathBuf {
    let mut pdf = Pdf::with_version("1.5");
    let pages_id = pdf.new_object_id();
    let kids: Vec<Object> = (0..2)
        .map(|_| {
            let content = pdf.add_object(Stream::new(dictionary! {}, Vec::new()));
            pdf.add_object(dictionary! {
                "Type" => "Page", "Parent" => pages_id, "Contents" => content,
                "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            })
            .into()
        })
        .collect();
    pdf.objects.insert(
        pages_id,
        dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => 2 }.into(),
    );
    let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    pdf.trailer.set("Root", catalog);
    let path = dir.join("blank.pdf");
    pdf.save(&path).unwrap();
    path
}

#[test]
fn comments_go_out_as_xfdf_and_come_back_in() {
    let dir = scratch("xfdf");
    let pdf = pdf(&dir);
    let xfdf = dir.join("review.xfdf");
    ok(&sheaf(&[
        "comments",
        "export",
        pdf.to_str().unwrap(),
        "-o",
        xfdf.to_str().unwrap(),
    ]));
    let text = std::fs::read_to_string(&xfdf).unwrap();
    assert!(
        text.contains("<xfdf") && text.contains(r#"inreplyto="n1""#),
        "{text}"
    );

    let blank = blank(&dir);
    let out = dir.join("with.pdf");
    let run = sheaf(&[
        "comments",
        "import",
        blank.to_str().unwrap(),
        xfdf.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
    ]);
    ok(&run);
    assert!(
        String::from_utf8_lossy(&run.stderr).contains("3 added, 0 updated"),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let csv = ok(&sheaf(&[
        "comments",
        "csv",
        out.to_str().unwrap(),
        "-c",
        "number,page,author,text",
    ]));
    assert_eq!(
        csv.lines().collect::<Vec<_>>(),
        [
            "Number,Page,Author,Text",
            "1,1,Ann,\"Tie to the ledger, please\"",
            "1.1,1,Bo,Tied",
            "2,2,Bo,Highlight",
        ]
    );
    // Again, as FDF: the same comments, matched by name.
    let fdf = dir.join("review.fdf");
    ok(&sheaf(&[
        "comments",
        "export",
        pdf.to_str().unwrap(),
        "-o",
        fdf.to_str().unwrap(),
    ]));
    assert!(std::fs::read(&fdf).unwrap().starts_with(b"%FDF-"));
    let run = sheaf(&[
        "comments",
        "import",
        out.to_str().unwrap(),
        fdf.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
        "--force",
    ]);
    ok(&run);
    assert!(String::from_utf8_lossy(&run.stderr).contains("0 added, 3 updated"));
    let _ = std::fs::remove_dir_all(&dir);
}
