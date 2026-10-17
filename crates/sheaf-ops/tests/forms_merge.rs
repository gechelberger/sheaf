//! Forms that come in with pages join the document's (M10 slice 4).
//! Insert, Replace and Combine add the incoming fields to `/Fields` with
//! what they need from their own form, and only the widgets on the pages
//! copied (bug 2). A form whose names clash with the document's, on any
//! node, goes under a new parent field named for its file (bug 3), and
//! Duplicate Pages gives the copy fields of its own the same way (bug 4).
//! Scripts from more than one form are off by default. Each by both save
//! strategies, with the form check clean on the output.
//!
//! Needs libpdfium (`scripts/fetch-pdfium.sh`), like the app itself.

use std::ops::Range;
use std::path::{Path, PathBuf};

use lopdf::{Document as Pdf, Object, ObjectId, dictionary};
use sheaf_core::PageIndex;
use sheaf_doc::forms::check;
use sheaf_doc::forms::fixture::Form;
use sheaf_doc::save::Strategy;
use sheaf_doc::{Document, Edit, EditOutcome, PageId, SourceInfo};
use sheaf_ops::combine::{CombineItem, CombineParams};
use sheaf_render::pdfium::render_appearances;

mod common;
use common::*;

/// Insert `range` of `path`'s pages (all, by default) before `before`, as
/// the app does: its form comes with them.
fn insert(
    doc: &mut Document,
    path: &Path,
    range: Option<Range<u32>>,
    before: Option<PageId>,
) -> EditOutcome {
    let info = SourceInfo::read(doc.store(), path, None).unwrap();
    let range = range.unwrap_or(0..info.src.page_count());
    let form = sheaf_doc::forms::read(&info.src);
    let sid = doc.add_source(info);
    doc.set_source_form(sid, form);
    doc.apply(
        Edit::InsertPages {
            source: sid,
            range,
            before,
            outline: None,
            annots: Vec::new(),
        },
        "Insert",
    )
    .unwrap()
}

/// A field `name` with two widgets, row 1 of pages 1 and 2, whose
/// appearances say `PAGEONE` and `PAGETWO`, holding `Both`.
fn shared_field(f: &mut Form, name: &str) -> ObjectId {
    let w1 = blue(f, "x", 0, 1, "PAGEONE");
    let w2 = blue(f, "x", 1, 1, "PAGETWO");
    for w in [w1, w2] {
        let d = f.dict(w);
        for k in [b"T".as_slice(), b"V", b"FT", b"DA"] {
            d.remove(k);
        }
    }
    let parent = f.group(name, &[w1, w2]);
    let d = f.dict(parent);
    d.set("FT", "Tx");
    d.set("V", Object::string_literal("Both"));
    d.set("DA", Object::string_literal("/Helv 10 Tf 0 g"));
    parent
}

/// The document's form: `a` (blue, "Alpha") on page 1, `a2` on page 2.
fn form_a() -> Form {
    let mut f = Form::new(2);
    blue(&mut f, "a", 0, 0, "Alpha");
    f.text("a2", 1, row(0));
    f
}

/// Another form: `b1` (blue, "Beta") on page 1, `both` on pages 1 and 2,
/// a Times font in `/DR` that `b1` uses, and `/CO` naming `b1`.
fn form_b() -> Form {
    let mut f = Form::new(2);
    let b1 = blue(&mut f, "b1", 0, 0, "Beta");
    shared_field(&mut f, "both");
    let times = f.pdf.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Times-Roman",
    });
    let form = f.acroform;
    let dr = f.dict(form).get(b"DR").unwrap().as_dict().unwrap().clone();
    let mut fonts = dr.get(b"Font").unwrap().as_dict().unwrap().clone();
    fonts.set("TiRo", times);
    let mut dr = dr;
    dr.set("Font", fonts);
    f.dict(form).set("DR", dr);
    f.dict(form).set("CO", vec![Object::Reference(b1)]);
    f.dict(b1)
        .set("DA", Object::string_literal("/TiRo 10 Tf 0 g"));
    f
}

/// The form's `/CO`, as full names.
fn calculation_order(bytes: &[u8]) -> Vec<String> {
    let form = form_in(bytes);
    form.facts
        .calculation_order
        .iter()
        .filter_map(|id| form.fields.iter().find(|f| f.id == *id))
        .map(|f| f.name.clone())
        .collect()
}

/// The form's `/DR /Font` names.
fn fonts(bytes: &[u8]) -> Vec<String> {
    form_in(bytes).facts.fonts
}

/// A field's own `/DA`, as text.
fn da_of(bytes: &[u8], name: &str) -> String {
    let f = form_in(bytes)
        .fields
        .into_iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("no field {name}"));
    f.da.as_deref().unwrap_or_default().to_string()
}

/// The full names of the fields in the model.
fn names(doc: &Document) -> Vec<String> {
    doc.state()
        .fields
        .values()
        .map(|f| f.name.clone())
        .collect()
}

#[test]
fn an_inserted_form_joins_the_documents() {
    let dir = scratch("merge-insert");
    let a = write(&dir, "a.pdf", &mut form_a());
    let b = write(&dir, "b.pdf", &mut form_b());
    let mut doc = open(&a);
    let out = insert(&mut doc, &b, None, None);
    // No name is taken: nothing's renamed.
    assert!(out.forms_renamed.is_empty());
    assert_eq!(names(&doc), ["a", "a2", "b1", "both"]);
    assert_eq!(doc.form_notice(&out, false, false), None);
    let saved = saved_both_ways(&doc, &a, &dir.join("rewritten.pdf"));
    assert_eq!(saved[1].0, Strategy::Incremental);
    for (strategy, bytes) in saved {
        let what = format!("{strategy:?}");
        let report = clean(&bytes, &what);
        assert_eq!((report.fields, report.widgets), (4, 5), "{what}");
        let v = values(&bytes);
        assert_eq!(
            (v["a"].as_str(), v["b1"].as_str(), v["both"].as_str()),
            ("Alpha", "Beta", "Both"),
            "{what}"
        );
        // What the incoming fields needed of their form came too.
        assert!(fonts(&bytes).contains(&"TiRo".to_string()), "{what}");
        assert_eq!(calculation_order(&bytes), ["b1"], "{what}");
        let r = render_appearances(&bytes, None, PageIndex(2), 612).unwrap();
        assert!(blue_in(&r, 0, 792.0) > HALF_A_ROW, "{what}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn one_page_brings_only_its_widgets() {
    // Page 2 of B: `both`, with its widget there and not the one on page 1.
    let dir = scratch("merge-one-page");
    let a = write(&dir, "a.pdf", &mut form_a());
    let b = write(&dir, "b.pdf", &mut form_b());
    let mut doc = open(&a);
    insert(&mut doc, &b, Some(1..2), None);
    assert_eq!(names(&doc), ["a", "a2", "both"]);
    for (strategy, bytes) in written_both_ways(&doc) {
        let what = format!("{strategy:?}");
        let report = clean(&bytes, &what);
        assert_eq!((report.fields, report.widgets), (3, 3), "{what}");
        assert!(holds(&bytes, b"PAGETWO"), "{what}");
        assert!(
            !holds(&bytes, b"PAGEONE"),
            "{what}: a widget of a page left behind"
        );
        assert!(!holds(&bytes, b"Beta"), "{what}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_form_inserted_into_itself_goes_under_its_files_name() {
    let dir = scratch("merge-itself");
    let path = write(&dir, "form.pdf", &mut form_a());
    let mut doc = open(&path);
    let out = insert(&mut doc, &path, None, None);
    assert_eq!(out.forms_renamed.len(), 1);
    assert_eq!(out.forms_renamed[0].0, "form");
    assert_eq!(
        doc.form_notice(&out, false, false).as_deref(),
        Some("Fields from form.pdf are under “form”, since their names were taken")
    );
    // And again: the name is taken too.
    let out = insert(&mut doc, &path, None, None);
    assert_eq!(out.forms_renamed[0].0, "form_2");
    assert_eq!(
        names(&doc),
        ["a", "a2", "form.a", "form.a2", "form_2.a", "form_2.a2"]
    );
    for (strategy, bytes) in saved_both_ways(&doc, &path, &dir.join("rewritten.pdf")) {
        let what = format!("{strategy:?}");
        let report = clean(&bytes, &what);
        assert_eq!(
            (report.pages, report.fields, report.widgets),
            (6, 6, 6),
            "{what}"
        );
        let v = values(&bytes);
        assert_eq!(v["a"], "Alpha", "{what}");
        assert_eq!(v["form.a"], "Alpha", "{what}");
        assert_eq!(v["form_2.a"], "Alpha", "{what}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_clash_on_a_parent_renames_the_whole_form() {
    // `top.x` here and `top.y` coming in: only the parents' names clash.
    let dir = scratch("merge-parent");
    let mut a = Form::new(1);
    let x = a.text("x", 0, row(0));
    a.group("top", &[x]);
    let mut b = Form::new(1);
    let y = b.text("y", 0, row(1));
    b.group("top", &[y]);
    let pa = write(&dir, "a.pdf", &mut a);
    let pb = write(&dir, "b.pdf", &mut b);
    let mut doc = open(&pa);
    insert(&mut doc, &pb, None, None);
    assert_eq!(names(&doc), ["top.x", "b.top.y"]);
    for (strategy, bytes) in written_both_ways(&doc) {
        let what = format!("{strategy:?}");
        let report = clean(&bytes, &what);
        assert_eq!(report.fields, 2, "{what}");
        let read: Vec<String> = form_in(&bytes).fields.into_iter().map(|f| f.name).collect();
        assert_eq!(read, ["top.x", "b.top.y"], "{what}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_font_of_the_same_name_comes_in_renamed() {
    // B's `/Helv` is Times: its fields use it as `/Helv_2`.
    let dir = scratch("merge-fonts");
    let mut b = Form::new(1);
    blue(&mut b, "b", 0, 0, "Beta");
    let helv = b.helv;
    b.dict(helv).set("BaseFont", "Times-Roman");
    let pa = write(&dir, "a.pdf", &mut form_a());
    let pb = write(&dir, "b.pdf", &mut b);
    let mut doc = open(&pa);
    insert(&mut doc, &pb, None, None);
    for (strategy, bytes) in written_both_ways(&doc) {
        let what = format!("{strategy:?}");
        clean(&bytes, &what);
        assert_eq!(da_of(&bytes, "b"), "/Helv_2 10 Tf 0 g", "{what}");
        assert_eq!(da_of(&bytes, "a"), "/Helv 10 Tf 0 g", "{what}");
        let f = fonts(&bytes);
        assert!(
            f.contains(&"Helv".into()) && f.contains(&"Helv_2".into()),
            "{what}: {f:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Save `doc` in place as the app does: then it shows the saved file.
fn save_in_place(doc: &mut Document, path: &Path) -> Strategy {
    let strategy = sheaf_doc::save::save_to_path(&doc.snapshot(), path).unwrap();
    let info = SourceInfo::read(doc.store(), path, None).unwrap();
    doc.rebase_to_saved(info);
    strategy
}

#[test]
fn a_duplicated_page_gets_fields_of_its_own() {
    let dir = scratch("merge-duplicate");
    let path = write(&dir, "form.pdf", &mut form_a());
    let mut doc = open(&path);
    let first = doc.state().pages[0].id;
    let out = doc
        .apply(Edit::DuplicatePages { ids: vec![first] }, "Duplicate")
        .unwrap();
    assert_eq!(out.forms_renamed.len(), 1);
    assert_eq!(
        doc.form_notice(&out, true, true).as_deref(),
        Some("The copies' fields are under “form”, apart from the originals")
    );
    assert_eq!(names(&doc), ["a", "a2", "form.a"]);
    let saved = saved_both_ways(&doc, &path, &dir.join("rewritten.pdf"));
    assert_eq!(saved[1].0, Strategy::Incremental);
    for (strategy, bytes) in saved {
        let what = format!("{strategy:?}");
        let report = clean(&bytes, &what);
        assert_eq!(
            (report.pages, report.fields, report.widgets),
            (3, 3, 3),
            "{what}"
        );
        assert_eq!(values(&bytes)["form.a"], "Alpha", "{what}");
        let r = render_appearances(&bytes, None, PageIndex(1), 612).unwrap();
        assert!(blue_in(&r, 0, 792.0) > HALF_A_ROW, "{what}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn saving_twice_writes_each_field_once() {
    // A duplicate and an insert, saved in place, then saved again: the
    // fields are the saved file's own now.
    let dir = scratch("merge-twice");
    let path = write(&dir, "form.pdf", &mut form_a());
    let b = write(&dir, "b.pdf", &mut form_b());
    let mut doc = open(&path);
    let first = doc.state().pages[0].id;
    doc.apply(Edit::DuplicatePages { ids: vec![first] }, "Duplicate")
        .unwrap();
    insert(&mut doc, &b, None, None);
    assert_eq!(save_in_place(&mut doc, &path), Strategy::Incremental);
    let once = std::fs::read(&path).unwrap();
    let report = clean(&once, "first save");
    assert_eq!((report.fields, report.widgets), (5, 6));
    // Then a page turned, and saved again both ways.
    let last = doc.state().pages[4].id;
    doc.apply(
        Edit::RotatePages {
            ids: vec![last],
            by: sheaf_core::Rotation::R90,
        },
        "Rotate",
    )
    .unwrap();
    for (strategy, bytes) in saved_both_ways(&doc, &path, &dir.join("rewritten.pdf")) {
        let what = format!("{strategy:?}");
        let report = clean(&bytes, &what);
        assert_eq!((report.fields, report.widgets), (5, 6), "{what}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn duplicating_a_copy_or_a_cropped_page_gets_fields_of_its_own() {
    let dir = scratch("merge-duplicate-copies");
    let path = write(&dir, "form.pdf", &mut form_a());
    let b = write(&dir, "b.pdf", &mut form_b());
    let mut doc = open(&path);
    insert(&mut doc, &b, None, None);
    // B's first page, duplicated; then form.pdf's first page cropped and
    // duplicated.
    let b_first = doc.state().pages[2].id;
    doc.apply(Edit::DuplicatePages { ids: vec![b_first] }, "Duplicate")
        .unwrap();
    let first = doc.state().pages[0].id;
    let plan = sheaf_ops::crop::plan(
        doc.store(),
        &doc.snapshot(),
        &[first],
        sheaf_ops::crop::CropSpec::Margins(sheaf_ops::crop::Margins {
            top: 0.0,
            left: 0.0,
            right: 0.0,
            bottom: 100.0,
        }),
        &sheaf_ops::NoProgress,
    )
    .unwrap();
    let sid = doc.add_source(plan[0].info.clone());
    doc.apply(
        Edit::RebasePages {
            pages: plan[0].rebases(sid),
        },
        "Crop",
    )
    .unwrap();
    doc.apply(Edit::DuplicatePages { ids: vec![first] }, "Duplicate")
        .unwrap();
    assert_eq!(
        names(&doc),
        ["a", "a2", "b1", "both", "b.b1", "b.both", "form.a"]
    );
    for (strategy, bytes) in written_both_ways(&doc) {
        let what = format!("{strategy:?}");
        let report = clean(&bytes, &what);
        // `both` has a widget on each of B's pages; only the first's is
        // copied.
        assert_eq!(
            (report.pages, report.fields, report.widgets),
            (6, 7, 8),
            "{what}"
        );
        let v = values(&bytes);
        assert_eq!(
            (v["b.b1"].as_str(), v["form.a"].as_str()),
            ("Beta", "Alpha"),
            "{what}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn replaced_pages_bring_their_form() {
    // Page 2 of A (with `a2`) replaced by B's first page.
    let dir = scratch("merge-replace");
    let a = write(&dir, "a.pdf", &mut form_a());
    let b = write(&dir, "b.pdf", &mut form_b());
    let mut doc = open(&a);
    let second = doc.state().pages[1].id;
    let info = SourceInfo::read(doc.store(), &b, None).unwrap();
    let form = sheaf_doc::forms::read(&info.src);
    let sid = doc.add_source(info);
    doc.set_source_form(sid, form);
    doc.apply(
        Edit::ReplacePages {
            ids: vec![second],
            source: sid,
            range: 0..1,
            annots: Vec::new(),
        },
        "Replace",
    )
    .unwrap();
    // The incremental writer's first: saving in place changes the file it
    // reads.
    let mut saved = written_both_ways(&doc);
    let in_place = saved_both_ways(&doc, &a, &dir.join("rewritten.pdf"));
    assert_eq!(in_place[1].0, Strategy::Rewrite);
    saved.extend(in_place);
    for (strategy, bytes) in saved {
        let what = format!("{strategy:?}");
        let v = values(&bytes);
        assert_eq!(
            (v["a"].as_str(), v["b1"].as_str()),
            ("Alpha", "Beta"),
            "{what}"
        );
        if strategy == Strategy::Rewrite {
            // `a2` was only on the page replaced: it's gone.
            let report = clean(&bytes, &what);
            assert_eq!((report.fields, report.widgets), (3, 3), "{what}");
        } else {
            // The incremental writer (a recovery sidecar's) can't take it
            // out: a save is a rewrite (M9 bug 14).
            let findings = check(&bytes, None).unwrap().findings;
            assert_eq!(
                findings,
                [sheaf_doc::forms::Finding::OffPage(vec!["a2".into()])],
                "{what}"
            );
        }
        assert!(!holds(&bytes, b"PAGETWO"), "{what}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn undo_takes_the_form_out_again() {
    let dir = scratch("merge-undo");
    let a = write(&dir, "a.pdf", &mut form_a());
    let b = write(&dir, "b.pdf", &mut form_b());
    let mut doc = open(&a);
    insert(&mut doc, &b, None, None);
    doc.undo().unwrap().unwrap();
    assert_eq!(names(&doc), ["a", "a2"]);
    for (strategy, bytes) in written_both_ways(&doc) {
        let what = format!("{strategy:?}");
        assert!(!holds(&bytes, b"Beta"), "{what}");
        assert_eq!(clean(&bytes, &what).fields, 2, "{what}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A document script named `name`.
fn document_script(f: &mut Form, scripts: &[(&str, &str)]) {
    let mut pairs = Vec::new();
    for (name, js) in scripts {
        let action = f.pdf.add_object(dictionary! {
            "S" => "JavaScript", "JS" => Object::string_literal(*js),
        });
        pairs.push(Object::string_literal(*name));
        pairs.push(Object::Reference(action));
    }
    let catalog = f.catalog;
    f.dict(catalog).set(
        "Names",
        dictionary! { "JavaScript" => dictionary! { "Names" => pairs } },
    );
}

/// The document scripts' names in the file.
fn script_names(bytes: &[u8]) -> Vec<String> {
    let pdf = Pdf::load_mem(bytes).unwrap();
    let names = pdf.catalog().unwrap().get(b"Names").unwrap();
    let names = pdf.dereference(names).unwrap().1.as_dict().unwrap();
    let tree = pdf
        .dereference(names.get(b"JavaScript").unwrap())
        .unwrap()
        .1;
    tree.as_dict()
        .unwrap()
        .get(b"Names")
        .unwrap()
        .as_array()
        .unwrap()
        .chunks(2)
        .map(|c| String::from_utf8_lossy(c[0].as_str().unwrap()).to_string())
        .collect()
}

#[test]
fn scripts_from_two_forms_are_off_and_kept_apart() {
    // A: a keystroke script on `a`, and document scripts; B: document
    // scripts with a name A's have.
    let dir = scratch("merge-scripts");
    let mut a = form_a();
    let first = a.fields()[0];
    a.dict(first).set(
        "AA",
        dictionary! { "K" => dictionary! { "S" => "JavaScript", "JS" => Object::string_literal("AFNumber_Keystroke(2,0,0,0,\"\",true);") } },
    );
    document_script(
        &mut a,
        &[
            ("!ADBE::0200_VersChkStrings", "var v;"),
            ("Init", "var a = 1;"),
        ],
    );
    let mut b = form_b();
    document_script(
        &mut b,
        &[
            ("!ADBE::0200_VersChkStrings", "var v;"),
            ("Init", "var b = 2;"),
        ],
    );
    let pa = write(&dir, "a.pdf", &mut a);
    let pb = write(&dir, "b.pdf", &mut b);

    let mut doc = open(&pa);
    assert!(!doc.scripts_mixed());
    let out = insert(&mut doc, &pb, None, None);
    assert!(doc.scripts_mixed());
    assert_eq!(doc.script_sources().len(), 2);
    assert_eq!(
        doc.form_notice(&out, false, false).as_deref(),
        Some("Scripts are off: fields from more than one form have them")
    );
    for (strategy, bytes) in written_both_ways(&doc) {
        let what = format!("{strategy:?}");
        clean(&bytes, &what);
        assert_eq!(
            script_names(&bytes),
            ["!ADBE::0200_VersChkStrings", "Init", "Init 2"],
            "{what}"
        );
        assert!(holds(&bytes, b"var b = 2;"), "{what}");
    }
    // Combine says the same.
    let items = [pa.clone(), pb.clone()].map(|path| CombineItem {
        path,
        pages: None,
        password: None,
    });
    let (combined, _) =
        sheaf_ops::combine::build(&items, &CombineParams::default(), &sheaf_ops::NoProgress)
            .unwrap();
    assert!(combined.scripts_mixed());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_files_pages_in_runs_join_one_form() {
    // B's pages 1 and 3 combined after A: `both` has widgets on both, as
    // one field, and nothing's renamed.
    let dir = scratch("merge-runs");
    let mut b = Form::new(3);
    let w1 = blue(&mut b, "x", 0, 1, "PAGEONE");
    let w3 = blue(&mut b, "x", 2, 1, "PAGETHREE");
    for w in [w1, w3] {
        let d = b.dict(w);
        for k in [b"T".as_slice(), b"V", b"FT", b"DA"] {
            d.remove(k);
        }
    }
    let parent = b.group("both", &[w1, w3]);
    b.dict(parent).set("FT", "Tx");
    b.dict(parent).set("V", Object::string_literal("Both"));
    blue(&mut b, "middle", 1, 0, "MIDDLE");
    let pa = write(&dir, "a.pdf", &mut form_a());
    let pb = write(&dir, "b.pdf", &mut b);
    let items = [
        CombineItem {
            path: pa,
            pages: None,
            password: None,
        },
        CombineItem {
            path: pb,
            pages: Some("1,3".into()),
            password: None,
        },
    ];
    let (doc, _) =
        sheaf_ops::combine::build(&items, &CombineParams::default(), &sheaf_ops::NoProgress)
            .unwrap();
    assert_eq!(names(&doc), ["a", "a2", "both"]);
    let bytes = sheaf_doc::save::rewrite(&doc.snapshot()).unwrap();
    let report = clean(&bytes, "combined");
    assert_eq!((report.pages, report.fields, report.widgets), (4, 3, 4));
    assert!(!holds(&bytes, b"MIDDLE"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The corpus's `path`, or None (and the test skips) without a corpus.
fn corpus_path(path: &str) -> Option<PathBuf> {
    let corpus = sheaf_doc::forms::corpus::Corpus::or_skip()?;
    let entry = corpus.entry(path).unwrap().expect("in the manifest");
    Some(corpus.path(&entry))
}

/// Combine `paths` as the app does, and the rewrite.
fn combined(paths: &[PathBuf]) -> (Document, Vec<u8>) {
    let items: Vec<CombineItem> = paths
        .iter()
        .map(|p| CombineItem {
            path: p.clone(),
            pages: None,
            password: None,
        })
        .collect();
    let (doc, _) =
        sheaf_ops::combine::build(&items, &CombineParams::default(), &sheaf_ops::NoProgress)
            .unwrap();
    let bytes = sheaf_doc::save::rewrite(&doc.snapshot()).unwrap();
    (doc, bytes)
}

/// Fields and widgets the form check counts in a file.
fn counts(path: &Path) -> (usize, usize) {
    let r = check(&std::fs::read(path).unwrap(), None).unwrap();
    (r.fields, r.widgets)
}

#[test]
fn corpus_the_1040_and_two_schedules_combine_with_unique_names() {
    let (Some(f1040), Some(s1), Some(sb)) = (
        corpus_path("forms/irs/f1040.pdf"),
        corpus_path("forms/irs/f1040s1.pdf"),
        corpus_path("forms/irs/f1040sb.pdf"),
    ) else {
        return;
    };
    let paths = [f1040, s1, sb];
    let (doc, bytes) = combined(&paths);
    // Every IRS form's root is `topmostSubform[0]`: the schedules are
    // renamed for their files.
    let renamed: Vec<&str> = doc
        .state()
        .fields
        .values()
        .filter_map(|f| f.under.as_deref())
        .collect();
    assert!(renamed.contains(&"f1040s1") && renamed.contains(&"f1040sb"));
    let want = paths
        .iter()
        .map(|p| counts(p))
        .fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
    let report = check(&bytes, None).unwrap();
    assert_eq!((report.fields, report.widgets), want);
    assert_eq!(findings(&bytes), vec![]);
}

#[test]
fn corpus_a_form_combined_with_itself_is_two_forms() {
    // Bug 3: the second copy's widgets were orphans with the first's
    // names.
    let Some(f1040) = corpus_path("forms/irs/f1040.pdf") else {
        return;
    };
    let (_, bytes) = combined(&[f1040.clone(), f1040.clone()]);
    let (fields, widgets) = counts(&f1040);
    let report = check(&bytes, None).unwrap();
    assert_eq!((report.fields, report.widgets), (2 * fields, 2 * widgets));
    assert_eq!(findings(&bytes), vec![]);
}

#[test]
fn corpus_scripted_forms_combined_have_scripts_off() {
    // GA 500 and OH IT 1040 in the plan; the corpus's scripted forms are
    // CA 540 and NY IT-201.
    let (Some(ca), Some(ny)) = (
        corpus_path("forms/ca-ftb/2025-540.pdf"),
        corpus_path("forms/ny-tax/it201_2025_fill_in.pdf"),
    ) else {
        return;
    };
    let (doc, bytes) = combined(&[ca.clone(), ny.clone()]);
    assert!(doc.scripts_mixed());
    let want = [ca, ny]
        .iter()
        .map(|p| counts(p))
        .fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
    let report = check(&bytes, None).unwrap();
    assert_eq!((report.fields, report.widgets), want);
    assert_eq!(findings(&bytes), vec![]);
}

#[test]
fn corpus_a_duplicated_page_is_a_form_of_its_own() {
    let Some(f1040) = corpus_path("forms/irs/f1040.pdf") else {
        return;
    };
    let mut doc = open(&f1040);
    let first = doc.state().pages[0].id;
    let on_first = doc.state().widgets_on(first).map_or(0, |w| w.len());
    doc.apply(Edit::DuplicatePages { ids: vec![first] }, "Duplicate")
        .unwrap();
    let (fields, widgets) = counts(&f1040);
    for (strategy, bytes) in written_both_ways(&doc) {
        let what = format!("{strategy:?}");
        let report = check(&bytes, None).unwrap();
        assert_eq!(report.widgets, widgets + on_first, "{what}");
        assert!(report.fields > fields, "{what}");
        assert_eq!(findings(&bytes), vec![], "{what}");
    }
}
