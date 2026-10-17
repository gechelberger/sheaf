use super::*;
use crate::annot::{Annot, AnnotKind, Pt};
use crate::history::FixedCount;
use crate::model::{Document, SourceInfo};
use crate::store::Store;
use lopdf::{Stream, dictionary};
use sheaf_core::Rotation;

/// Comments of most kinds, as another tool writes them.
fn sample() -> Comments {
    let mut c = Comments::new();
    let add = |c: &mut Comments, page: u32, d: Dictionary| {
        let id = c.pdf.add_object(Object::Dictionary(d));
        c.annots.push((page, id));
        id
    };
    let r = |x: i64, y: i64| vec![x.into(), y.into(), (x + 20).into(), (y + 20).into()];
    let note = add(
        &mut c,
        0,
        dictionary! {
            "Type" => "Annot", "Subtype" => "Text", "Rect" => r(100, 600), "NM" => Object::string_literal("n1"),
            "T" => Object::string_literal("Ann"), "Contents" => crate::save::text_string("Tie <&> “foot”"),
            "M" => Object::string_literal("D:20261001120000"), "C" => vec![1.into(), 1.into(), 0.into()],
            "F" => 28, "Name" => "Comment", "Open" => false,
        },
    );
    add(
        &mut c,
        0,
        dictionary! {
            "Type" => "Annot", "Subtype" => "Text", "Rect" => r(100, 600), "NM" => Object::string_literal("r1"),
            "IRT" => note, "T" => Object::string_literal("Bo"), "Contents" => Object::string_literal("Done"),
        },
    );
    add(
        &mut c,
        0,
        dictionary! {
            "Type" => "Annot", "Subtype" => "Text", "Rect" => r(100, 600), "NM" => Object::string_literal("s1"),
            "IRT" => note, "State" => Object::string_literal("Accepted"),
            "StateModel" => Object::string_literal("Review"),
        },
    );
    add(
        &mut c,
        0,
        dictionary! {
            "Type" => "Annot", "Subtype" => "Highlight", "Rect" => r(100, 680),
            "NM" => Object::string_literal("h1"), "C" => vec![1.into(), 1.into(), 0.into()], "CA" => 0.5,
            "QuadPoints" => vec![100.into(), 700.into(), 200.into(), 700.into(), 100.into(), 688.into(), 200.into(), 688.into()],
        },
    );
    add(
        &mut c,
        1,
        dictionary! {
            "Type" => "Annot", "Subtype" => "Line", "Rect" => r(10, 10), "NM" => Object::string_literal("l1"),
            "L" => vec![10.into(), 10.into(), 200.into(), 150.into()],
            "LE" => vec![Object::Name(b"None".to_vec()), Object::Name(b"OpenArrow".to_vec())],
            "C" => vec![1.into(), 0.into(), 0.into()], "BS" => dictionary! { "W" => 3, "S" => "D", "D" => vec![3.into(), 3.into()] },
        },
    );
    add(
        &mut c,
        1,
        dictionary! {
            "Type" => "Annot", "Subtype" => "Ink", "Rect" => r(10, 10), "NM" => Object::string_literal("i1"),
            "InkList" => vec![Object::Array(vec![10.into(), 10.into(), 20.into(), 25.into(), 40.into(), 30.into()])],
        },
    );
    add(
        &mut c,
        1,
        dictionary! {
            "Type" => "Annot", "Subtype" => "Polygon", "Rect" => r(300, 300), "NM" => Object::string_literal("p1"),
            "Vertices" => vec![300.into(), 300.into(), 400.into(), 310.into(), 350.into(), 400.into()],
            "BE" => dictionary! { "S" => "C", "I" => 1 }, "IC" => vec![0.into(), 0.into(), 1.into()],
        },
    );
    add(
        &mut c,
        1,
        dictionary! {
            "Type" => "Annot", "Subtype" => "FreeText", "IT" => "FreeTextCallout",
            "Rect" => vec![200.into(), 200.into(), 400.into(), 260.into()],
            "RD" => vec![50.into(), 0.into(), 0.into(), 0.into()],
            "CL" => vec![205.into(), 205.into(), 230.into(), 230.into(), 250.into(), 230.into()],
            "DA" => Object::string_literal("/Helv 10 Tf 0 0 1 rg"), "LE" => "OpenArrow",
            "NM" => Object::string_literal("f1"), "Contents" => Object::string_literal("Why?"),
        },
    );
    c.file = Some("review.pdf".into());
    c
}

/// Each comment in `c` as the converter makes it, by name.
fn marks(c: &Comments) -> Vec<(u32, String)> {
    let mut out = Vec::new();
    for i in 0..c.len() {
        let d = c.dict(i).unwrap();
        let a = crate::import::convert(&c.pdf, d, Rotation::R0, Default::default()).unwrap();
        out.push((c.annots[i].0, serde_json::to_string(&a).unwrap()));
    }
    out
}

#[test]
fn xfdf_and_fdf_carry_every_kind_through() {
    let c = sample();
    let want = marks(&c);
    let xfdf = xfdf::write(&c).unwrap();
    let text = String::from_utf8(xfdf.clone()).unwrap();
    assert!(text.contains(r#"<text page="0""#), "{text}");
    assert!(
        text.contains("<contents>Tie &lt;&amp;&gt; “foot”</contents>"),
        "{text}"
    );
    assert!(text.contains(r#"inreplyto="n1""#) && text.contains(r#"<f href="review.pdf"/>"#));
    let back = xfdf::read(&xfdf).unwrap();
    assert_eq!(back.file.as_deref(), Some("review.pdf"));
    assert_eq!(marks(&back), want);

    let fdf = fdf::write(&c).unwrap();
    assert!(fdf.starts_with(b"%FDF-1.2"));
    let back = read(&fdf, None).unwrap();
    assert_eq!(back.file.as_deref(), Some("review.pdf"));
    assert_eq!(marks(&back), want);
}

#[test]
fn acrobat_xfdf_reads() {
    let xml = r##"<?xml version="1.0" encoding="UTF-8" ?>
<xfdf xmlns="http://ns.adobe.com/xfdf/" xml:space="preserve"><annots>
<text color="#FFFF00" creationdate="D:20261001120000-07'00'" flags="print,nozoom,norotate" date="D:20261001120500-07'00'" name="a1b2" icon="Comment" page="0" rect="100,600,120,620" subject="Sticky Note" title="Ann"><contents-richtext><body xmlns="http://www.w3.org/1999/xhtml"><p>Tie &amp; foot</p></body></contents-richtext><contents>Tie &amp; foot&#x2014;now</contents><popup flags="print,nozoom,norotate" open="no" page="0" rect="120,500,300,620"/></text>
<text color="#FFFF00" name="r1" inreplyto="a1b2" page="0" rect="100,600,120,620" title="Bo"><contents>Done</contents></text>
<ink color="#FF0000" width="2" page="1" name="i1" rect="0,0,50,50"><inklist><gesture>10,10;20,20;30,15</gesture></inklist></ink>
</annots><f href="workpapers.pdf"/><ids original="ABC" modified="DEF"/></xfdf>"##;
    let c = read(xml.as_bytes(), None).unwrap();
    assert_eq!(c.len(), 3);
    assert_eq!(c.file.as_deref(), Some("workpapers.pdf"));
    let note = crate::import::convert(&c.pdf, c.dict(0).unwrap(), Rotation::R0, Default::default())
        .unwrap();
    assert_eq!(note.contents, "Tie & foot—now");
    assert_eq!((note.name.as_str(), note.author.as_str()), ("a1b2", "Ann"));
    assert_eq!(note.color, [255, 255, 0]);
    assert_eq!(note.created, "D:20261001120000-07'00'");
    let reply =
        crate::import::convert(&c.pdf, c.dict(1).unwrap(), Rotation::R0, Default::default())
            .unwrap();
    assert!(matches!(reply.kind, AnnotKind::Reply { ref parent } if parent == "a1b2"));
    let ink = crate::import::convert(&c.pdf, c.dict(2).unwrap(), Rotation::R0, Default::default())
        .unwrap();
    assert_eq!(c.annots[2].0, 1);
    assert!(matches!(&ink.kind, AnnotKind::Ink { paths } if paths[0].len() == 3));
    assert_eq!(ink.stroke, 2.0);
}

#[test]
fn not_a_data_file_says_so() {
    assert!(read(b"hello", None).is_err());
    assert!(xfdf::read(b"<html></html>").is_err());
}

/// A two-page Letter file with another tool's square ("sq") on page 1.
fn pdf_with_square(dir: &std::path::Path) -> std::path::PathBuf {
    let mut pdf = Pdf::with_version("1.5");
    let pages_id = pdf.new_object_id();
    let mut kids = Vec::new();
    for i in 0..2 {
        let content = pdf.add_object(Stream::new(dictionary! {}, Vec::new()));
        let mut page = dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => content,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        };
        if i == 0 {
            let sq = pdf.add_object(dictionary! {
                "Type" => "Annot", "Subtype" => "Square", "NM" => Object::string_literal("sq"),
                "Rect" => vec![100.into(), 100.into(), 200.into(), 150.into()],
                "T" => Object::string_literal("Pat"), "Contents" => Object::string_literal("Box"),
            });
            page.set("Annots", vec![sq.into()]);
        }
        kids.push(Object::Reference(pdf.add_object(page)));
    }
    pdf.objects.insert(
        pages_id,
        dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => 2 }.into(),
    );
    let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    pdf.trailer.set("Root", catalog);
    let path = dir.join("two.pdf");
    pdf.save(&path).unwrap();
    path
}

fn opened(path: &std::path::Path) -> (Document, Vec<(SourceId, ForeignAnnot)>) {
    let found = crate::file::read_annots(path, None).unwrap();
    let store = Store::new();
    let primary = SourceInfo::read(&store, path, None).unwrap();
    let mut doc = Document::new(store, primary, &[], Box::new(FixedCount(10)));
    let note = Annot {
        id: AnnotId(0),
        kind: AnnotKind::Note {
            pos: Pt { x: 50.0, y: 50.0 },
        },
        color: [255, 230, 0],
        opacity: 1.0,
        stroke: 1.0,
        author: "Greg".into(),
        contents: "ours".into(),
        modified: String::new(),
        created: String::new(),
        fill: None,
        dashed: false,
        cloudy: false,
        turn: Rotation::R0,
        name: String::new(),
    };
    let p0 = doc.page(0).unwrap().id;
    doc.apply(
        Edit::AnnotAdd {
            page: p0,
            annot: note,
        },
        "note",
    )
    .unwrap();
    let primary = doc.primary();
    (
        doc,
        found.foreign.into_iter().map(|f| (primary, f)).collect(),
    )
}

#[test]
fn imports_match_by_name_and_report_the_rest() {
    let dir = std::env::temp_dir().join(format!("sheaf-exchange-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = pdf_with_square(&dir);
    let (doc, foreign) = opened(&path);
    let snap = doc.snapshot();
    let p0 = doc.page(0).unwrap().id;
    let ours = doc.state().annots_on(p0).unwrap()[0].clone();

    let mut c = Comments::new();
    let mut add = |page: u32, d: Dictionary| {
        let id = c.pdf.add_object(Object::Dictionary(d));
        c.annots.push((page, id));
    };
    let r = || vec![100.into(), 100.into(), 120.into(), 120.into()];
    // Ours, changed elsewhere; theirs, changed elsewhere; a new one; one
    // sheaf can't hold; one past the last page.
    add(
        0,
        dictionary! { "Subtype" => "Text", "Rect" => r(), "NM" => crate::save::text_string(&ours.name()), "Contents" => Object::string_literal("ours, edited") },
    );
    add(
        0,
        dictionary! { "Subtype" => "Square", "Rect" => r(), "NM" => Object::string_literal("sq"), "Contents" => Object::string_literal("Box, edited") },
    );
    add(
        1,
        dictionary! { "Subtype" => "Text", "Rect" => r(), "NM" => Object::string_literal("new"), "Contents" => Object::string_literal("Hello") },
    );
    add(1, dictionary! { "Subtype" => "Sound", "Rect" => r() });
    add(1, dictionary! { "Subtype" => "Popup", "Rect" => r() });
    add(5, dictionary! { "Subtype" => "Text", "Rect" => r() });
    let got = import(&snap, &foreign, &c);
    assert_eq!((got.added, got.updated, got.off_the_end), (1, 2, 1));
    assert_eq!(got.skipped, BTreeMap::from([("Sound".to_string(), 1)]));
    assert_eq!(
        got.summary(),
        "1 added, 2 updated; skipped 1 Sound (sheaf can't hold them); 1 on pages this document doesn't have"
    );
    match &got.edits[0] {
        Edit::AnnotUpdate { page, annot } => {
            assert_eq!((*page, annot.id), (p0, ours.id));
            assert_eq!(annot.contents, "ours, edited");
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(&got.edits[1], Edit::AnnotAdd { annot, .. } if annot.name == "sq"));

    // All of it, ours and theirs, out and back.
    let ix: Vec<usize> = (0..foreign.len()).collect();
    let out = gather(&snap, &foreign, &[(p0, ours.id)], &ix).unwrap();
    assert_eq!(out.file.as_deref(), Some("two.pdf"));
    let back = read(&fdf::write(&out).unwrap(), None).unwrap();
    let names: Vec<(u32, String)> = (0..back.len())
        .map(|i| (back.annots[i].0, thread_name(back.dict(i).unwrap())))
        .collect();
    assert_eq!(names, [(0, ours.name()), (0, "sq".to_string())]);
    // Ours come back exactly, from /SheafAnnot.
    let again = import(&snap, &foreign, &back);
    assert_eq!((again.added, again.updated), (0, 2));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A page stamp's page and an attachment's file go out with our marks in
/// an FDF and come back with them into a document that has neither.
#[test]
fn our_marks_bring_their_stamps_and_files() {
    use crate::forms::fixture::{MARK_FILE, MARK_FILE_BYTES, MARK_STAMP, every_mark_kind};
    let dir = std::env::temp_dir().join(format!(
        "sheaf-exchange-held-{}-{:x}",
        std::process::id(),
        fastrand::u64(..)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = pdf_with_square(&dir);
    let (mut doc, foreign) = opened(&path);
    let held = crate::forms::fixture::mark_assets();
    let assets = doc.assets();
    assets.intern_stamp(MARK_STAMP, held.stamp_pdf(MARK_STAMP).unwrap());
    assets.intern_file(MARK_FILE, held.file_bytes(MARK_FILE).unwrap());
    let p0 = doc.page(0).unwrap().id;
    let every = every_mark_kind();
    for annot in [every[17].clone(), every[23].clone()] {
        doc.apply(Edit::AnnotAdd { page: p0, annot }, "mark")
            .unwrap();
    }
    // After the note.
    let ours: Vec<(PageId, AnnotId)> = doc
        .state()
        .annots_on(p0)
        .unwrap()
        .iter()
        .skip(1)
        .map(|a| (p0, a.id))
        .collect();
    let out = gather(&doc.snapshot(), &foreign, &ours, &[]).unwrap();
    let back = read(&fdf::write(&out).unwrap(), None).unwrap();
    let (other, foreign) = opened(&path);
    assert!(other.assets().stamp(MARK_STAMP).is_none());
    let got = import(&other.snapshot(), &foreign, &back);
    assert_eq!(got.added, 2);
    assert!(other.assets().stamp_pdf(MARK_STAMP).is_some());
    assert_eq!(
        &*other.assets().file_bytes(MARK_FILE).unwrap(),
        MARK_FILE_BYTES
    );
    let _ = std::fs::remove_dir_all(&dir);
}
