//! A page keeps its form through the page tools (M10 slice 3). Crop,
//! watermark and Remove Header/Footer carry its fields into the copy they
//! make, so the page shows them, and a save writes them back as they were
//! (bug 1). A removed page's widgets leave the file, values and all, even
//! ones only the structure tree's parent tree reaches (bug 5). A form left
//! with no fields goes, and direct-object fields are pruned too (bug 10).
//! Each by both save strategies, with the form check clean on the output.
//!
//! Needs libpdfium (`scripts/fetch-pdfium.sh`), like the app itself.

use std::collections::BTreeMap;

use lopdf::{Document as Pdf, Object, ObjectId, dictionary};
use sheaf_core::PageIndex;
use sheaf_doc::forms::check;
use sheaf_doc::forms::corpus::Corpus;
use sheaf_doc::forms::fixture::Form;
use sheaf_doc::save::Strategy;
use sheaf_doc::{Document, Edit, PageId};
use sheaf_image::raster::PixelFormat;
use sheaf_ops::crop::{CropSpec, Margins};
use sheaf_render::Renderer;
use sheaf_render::pdfium::{PdfiumEngine, render_appearances};

mod common;
use common::*;

/// A page's `/CropBox`.
fn crop_box(bytes: &[u8], page: usize) -> Vec<f64> {
    let pdf = Pdf::load_mem(bytes).unwrap();
    let id = pdf.get_pages()[&(page as u32 + 1)];
    pdf.get_dictionary(id)
        .unwrap()
        .get(b"CropBox")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o.as_float().unwrap())
        .collect()
}

/// Two pages: on the first, text fields `a` (blue) and `c` and a check
/// box; on the second, `b` (blue).
fn two_page_form() -> Form {
    let mut f = Form::new(2);
    blue(&mut f, "a", 0, 0, "Alpha");
    f.text("c", 0, row(1));
    f.check_box("box", 0, row(2), true);
    blue(&mut f, "b", 1, 0, "Beta");
    f
}

/// Move `pages` of `doc` onto derived copies, as the app does.
fn rebase(doc: &mut Document, derived: Vec<sheaf_ops::derive::DerivedSource>, what: &str) {
    assert!(!derived.is_empty(), "{what} made no copy");
    for d in derived {
        let sid = doc.add_source(d.info.clone());
        doc.apply(
            Edit::RebasePages {
                pages: d.rebases(sid),
            },
            what,
        )
        .unwrap();
    }
}

#[test]
fn a_cropped_form_page_keeps_its_fields() {
    let dir = scratch("crop");
    let path = write(&dir, "form.pdf", &mut two_page_form());
    let mut doc = open(&path);
    let page: PageId = doc.state().pages[0].id;
    // 100 pt off the bottom: the fields stay in view.
    let margins = Margins {
        top: 0.0,
        left: 0.0,
        right: 0.0,
        bottom: 100.0,
    };
    let plan = sheaf_ops::crop::plan(
        doc.store(),
        &doc.snapshot(),
        &[page],
        CropSpec::Margins(margins),
        &sheaf_ops::NoProgress,
    )
    .unwrap();
    // On screen: the copy is a form, so PDFium draws its widgets (before,
    // it had no /AcroForm and the field was blank).
    let copy = plan[0].info.src.bytes().clone();
    let shown = PdfiumEngine::open_bytes(copy.clone(), None).unwrap();
    let r = shown
        .render_page(PageIndex(0), 612, PixelFormat::Rgba)
        .unwrap();
    assert!(
        blue_in(&r, 0, 792.0) > HALF_A_ROW,
        "the copy draws the field"
    );
    assert_eq!(values(&copy)["a"], "Alpha");
    // Only the copied page's fields came along.
    assert!(!values(&copy).contains_key("b"));
    clean(&copy, "the cropped copy");

    let sid = doc.add_source(plan[0].info.clone());
    doc.apply(
        Edit::RebasePages {
            pages: plan[0].rebases(sid),
        },
        "Crop",
    )
    .unwrap();
    for (strategy, bytes) in written_both_ways(&doc) {
        let what = format!("{strategy:?}");
        let report = clean(&bytes, &what);
        assert_eq!((report.fields, report.widgets), (4, 4), "{what}");
        let v = values(&bytes);
        assert_eq!(
            (v["a"].as_str(), v["b"].as_str()),
            ("Alpha", "Beta"),
            "{what}"
        );
        assert_eq!(
            crop_box(&bytes, 0),
            vec![0.0, 100.0, 612.0, 792.0],
            "{what}"
        );
        // Other viewers see the value: its appearance is on the page.
        let r = render_appearances(&bytes, None, PageIndex(0), 612).unwrap();
        assert!(
            blue_in(&r, 0, 792.0) > HALF_A_ROW,
            "{what}: the value shows"
        );
        // The copy's own widgets didn't come along: the page has its own.
        let pdf = Pdf::load_mem(&bytes).unwrap();
        let first = pdf.get_pages()[&1];
        let annots = pdf.get_dictionary(first).unwrap().get(b"Annots").unwrap();
        let annots = pdf.dereference(annots).unwrap().1.as_array().unwrap().len();
        assert_eq!(annots, 3, "{what}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A watermark on both pages, and with `remove`, taken off again (a
/// copy of a copy), saved both ways.
fn watermarked(remove: bool) {
    let name = if remove { "unmarked" } else { "marked" };
    let dir = scratch(name);
    let path = write(&dir, "form.pdf", &mut two_page_form());
    let mut doc = open(&path);
    let pages: Vec<PageId> = doc.state().pages.iter().map(|p| p.id).collect();
    let wm = sheaf_doc::watermark::Watermark::default();
    let snap = doc.snapshot();
    let derived = sheaf_ops::derive::watermark(
        doc.store(),
        &snap,
        false,
        Some(&wm),
        &pages,
        &sheaf_ops::NoProgress,
    )
    .unwrap();
    let copy = derived[0].info.src.bytes().clone();
    let shown = PdfiumEngine::open_bytes(copy.clone(), None).unwrap();
    let r = shown
        .render_page(PageIndex(1), 612, PixelFormat::Rgba)
        .unwrap();
    assert!(
        blue_in(&r, 0, 792.0) > HALF_A_ROW,
        "the copy draws the field"
    );
    clean(&copy, "the watermarked copy");
    rebase(&mut doc, derived, "Watermark");
    if remove {
        let snap = doc.snapshot();
        let derived = sheaf_ops::derive::watermark(
            doc.store(),
            &snap,
            false,
            None,
            &pages,
            &sheaf_ops::NoProgress,
        )
        .unwrap();
        rebase(&mut doc, derived, "Remove Watermark");
    }
    for (strategy, bytes) in written_both_ways(&doc) {
        let what = format!("{name}, {strategy:?}");
        let report = clean(&bytes, &what);
        assert_eq!((report.fields, report.widgets), (4, 4), "{what}");
        let v = values(&bytes);
        assert_eq!(
            (v["a"].as_str(), v["b"].as_str()),
            ("Alpha", "Beta"),
            "{what}"
        );
        for page in 0..2 {
            let r = render_appearances(&bytes, None, PageIndex(page), 612).unwrap();
            assert!(blue_in(&r, 0, 792.0) > HALF_A_ROW, "{what}: page {page}");
        }
        assert_eq!(holds(&bytes, b"DRAFT"), !remove, "{what}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_watermarked_form_keeps_its_fields() {
    watermarked(false);
}

#[test]
fn a_form_with_its_watermark_removed_keeps_its_fields() {
    watermarked(true);
}

/// Tag `form` as Acrobat tags some forms: beside each widget's `/Form`
/// element in the tree, a second one with no `/P`, which only the parent
/// tree reaches (M10 bug 5).
fn tag_with_strays(form: &mut Form) {
    form.tag();
    let root = form
        .pdf
        .catalog()
        .unwrap()
        .get(b"StructTreeRoot")
        .unwrap()
        .as_reference()
        .unwrap();
    let nums = form
        .dict(root)
        .get(b"ParentTree")
        .unwrap()
        .as_dict()
        .unwrap()
        .get(b"Nums")
        .unwrap()
        .as_array()
        .unwrap()
        .clone();
    let mut strays = Vec::new();
    for pair in nums.chunks(2) {
        let element = pair[1].as_reference().unwrap();
        let k = form.dict(element).get(b"K").unwrap().clone();
        let page = form.dict(element).get(b"Pg").unwrap().clone();
        let stray = form.pdf.add_object(dictionary! {
            "Type" => "StructElem", "S" => "Form", "Pg" => page, "K" => k,
        });
        strays.push(pair[0].clone());
        strays.push(Object::Reference(stray));
    }
    form.dict(root)
        .set("ParentTree", dictionary! { "Nums" => strays });
}

#[test]
fn a_form_with_its_header_and_footer_removed_keeps_its_fields() {
    let dir = scratch("header-footer");
    let path = write(&dir, "form.pdf", &mut two_page_form());
    // A footer, saved into the file.
    let mut doc = open(&path);
    let hf = sheaf_doc::stamps::HeaderFooter {
        texts: [
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            "SHEAFFOOTER <<1>>".into(),
            String::new(),
        ],
        ..Default::default()
    };
    doc.apply(
        Edit::SetHeaderFooter {
            header_footer: Some(hf),
        },
        "Header & footer",
    )
    .unwrap();
    let footed = dir.join("footed.pdf");
    sheaf_doc::save::rewrite_to_path(&doc.snapshot(), &footed).unwrap();
    assert!(holds(&std::fs::read(&footed).unwrap(), b"SHEAFFOOTER"));

    // Removed, as the app does it.
    let mut doc = open(&footed);
    let derived =
        sheaf_ops::derive::strip_stamps(doc.store(), &doc.snapshot(), &sheaf_ops::NoProgress)
            .unwrap();
    assert_eq!(derived.len(), 1);
    let sid = doc.add_source(derived[0].info.clone());
    doc.apply_batch(
        vec![
            Edit::RebasePages {
                pages: derived[0].rebases(sid),
            },
            Edit::SetHeaderFooter {
                header_footer: None,
            },
        ],
        "Remove header & footer",
    )
    .unwrap();
    // A header/footer change is only ever a rewrite: the incremental writer
    // refuses it.
    assert!(sheaf_doc::incremental::incremental(&doc.snapshot()).is_err());
    let saved = saved_both_ways(&doc, &footed, &dir.join("rewritten.pdf"));
    assert_eq!(saved[1].0, Strategy::Rewrite);
    for (strategy, bytes) in saved {
        let what = format!("{strategy:?}");
        assert!(!holds(&bytes, b"SHEAFFOOTER"), "{what}");
        let report = clean(&bytes, &what);
        assert_eq!((report.fields, report.widgets), (4, 4), "{what}");
        let v = values(&bytes);
        assert_eq!(
            (v["a"].as_str(), v["b"].as_str()),
            ("Alpha", "Beta"),
            "{what}"
        );
        for page in 0..2 {
            let r = render_appearances(&bytes, None, PageIndex(page), 612).unwrap();
            assert!(blue_in(&r, 0, 792.0) > HALF_A_ROW, "{what}: page {page}");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Two tagged pages whose widgets the parent tree reaches only through
/// strays; on the second, a field holding `LEAKVALUE123`.
fn leaky_form() -> Form {
    let mut f = Form::new(2);
    blue(&mut f, "keep", 0, 0, "KEPT");
    blue(&mut f, "gone", 1, 0, "LEAKVALUE123");
    tag_with_strays(&mut f);
    f
}

/// Tag `form`, then point its parent tree into a second tree the root
/// doesn't hold, as the I-9's is: a page element per page over a
/// `/Figure` (with alt text, "SHEAFALT <page>") over a `/Form` element per
/// widget. An element only the ID tree reaches ("SHEAFID <page>") points
/// at each page's widgets too.
fn tag_with_a_second_tree(form: &mut Form) {
    form.tag();
    let root = form
        .pdf
        .catalog()
        .unwrap()
        .get(b"StructTreeRoot")
        .unwrap()
        .as_reference()
        .unwrap();
    let stale = form.pdf.new_object_id();
    let (mut tops, mut nums, mut ids) = (Vec::new(), Vec::new(), Vec::new());
    for (i, page) in form.pages.clone().into_iter().enumerate() {
        let top = form.pdf.new_object_id();
        let figure = form.pdf.new_object_id();
        let mut kids = vec![Object::Integer(0)];
        let mut objrs = Vec::new();
        for w in form.annots(i) {
            let key = form.dict(w).get(b"StructParent").unwrap().as_i64().unwrap();
            let objr = dictionary! { "Type" => "OBJR", "Obj" => w, "Pg" => page };
            objrs.push(Object::Dictionary(objr.clone()));
            let element = form.pdf.add_object(dictionary! {
                "Type" => "StructElem", "S" => "Form", "P" => figure, "K" => objr,
            });
            kids.push(Object::Reference(element));
            nums.push(Object::Integer(key));
            nums.push(Object::Reference(element));
        }
        form.pdf.objects.insert(
            figure,
            dictionary! {
                "Type" => "StructElem", "S" => "Figure", "P" => top, "Pg" => page, "K" => kids,
                "Alt" => Object::string_literal(format!("SHEAFALT {i}")),
            }
            .into(),
        );
        form.pdf.objects.insert(
            top,
            dictionary! { "Type" => "StructElem", "S" => "Part", "P" => stale, "K" => vec![figure.into()] }
                .into(),
        );
        tops.push(Object::Reference(top));
        let only = form.pdf.add_object(dictionary! {
            "Type" => "StructElem", "S" => "Form", "ID" => Object::string_literal(format!("id{i}")),
            "Alt" => Object::string_literal(format!("SHEAFID {i}")), "K" => objrs,
        });
        ids.push(Object::string_literal(format!("id{i}")));
        ids.push(Object::Reference(only));
    }
    form.pdf.objects.insert(
        stale,
        dictionary! { "Type" => "StructTreeRoot", "K" => tops }.into(),
    );
    let r = form.dict(root);
    r.set("ParentTree", dictionary! { "Nums" => nums });
    r.set("IDTree", dictionary! { "Names" => ids });
}

#[test]
fn a_deleted_pages_value_leaves_a_second_tree() {
    let dir = scratch("second-tree");
    let mut f = Form::new(2);
    blue(&mut f, "keep", 0, 0, "KEPT");
    blue(&mut f, "gone", 1, 0, "LEAKVALUE123");
    tag_with_a_second_tree(&mut f);
    let path = write(&dir, "form.pdf", &mut f);
    let mut doc = open(&path);
    let second = doc.state().pages[1].id;
    doc.apply(Edit::DeletePages { ids: vec![second] }, "Delete")
        .unwrap();
    for (strategy, bytes) in saved_both_ways(&doc, &path, &dir.join("rewritten.pdf")) {
        let what = format!("{strategy:?}");
        assert!(!holds(&bytes, b"LEAKVALUE123"), "{what}: the value stayed");
        // What described the page went with it; the first page's stays.
        assert!(!holds(&bytes, b"SHEAFALT 1"), "{what}");
        assert!(!holds(&bytes, b"SHEAFID 1"), "{what}");
        assert!(
            holds(&bytes, b"SHEAFALT 0") && holds(&bytes, b"SHEAFID 0"),
            "{what}"
        );
        assert_eq!(dangling_structure(&bytes), 0, "{what}");
        let report = clean(&bytes, &what);
        assert_eq!(
            (report.pages, report.fields, report.widgets),
            (1, 1, 1),
            "{what}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_deleted_pages_value_leaves_the_file() {
    let dir = scratch("delete");
    let path = write(&dir, "form.pdf", &mut leaky_form());
    let mut doc = open(&path);
    let second = doc.state().pages[1].id;
    doc.apply(Edit::DeletePages { ids: vec![second] }, "Delete")
        .unwrap();
    let saved = saved_both_ways(&doc, &path, &dir.join("rewritten.pdf"));
    // A removal is never incremental (M9 bug 14).
    assert_eq!(saved[1].0, Strategy::Rewrite);
    for (strategy, bytes) in saved {
        let what = format!("{strategy:?}");
        assert!(!holds(&bytes, b"LEAKVALUE123"), "{what}: the value stayed");
        let report = clean(&bytes, &what);
        assert_eq!(
            (report.pages, report.fields, report.widgets),
            (1, 1, 1),
            "{what}"
        );
        assert_eq!(values(&bytes)["keep"], "KEPT", "{what}");
        assert_eq!(dangling_structure(&bytes), 0, "{what}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_extracted_page_brings_no_other_pages_value() {
    let dir = scratch("extract");
    let path = write(&dir, "form.pdf", &mut leaky_form());
    let doc = open(&path);
    let first = doc.state().pages[0].id;
    let bytes = sheaf_doc::save::extract_bytes(&doc.snapshot(), &[first]).unwrap();
    assert!(!holds(&bytes, b"LEAKVALUE123"));
    let report = clean(&bytes, "extracted");
    assert_eq!((report.pages, report.fields, report.widgets), (1, 1, 1));
    assert_eq!(dangling_structure(&bytes), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_form_left_with_no_fields_goes() {
    // Fields on the first page only, as on the W-9 (its second page is
    // instructions).
    let dir = scratch("empty");
    let mut f = Form::new(2);
    blue(&mut f, "a", 0, 0, "Alpha");
    f.check_box("box", 0, row(1), true);
    f.xfa();
    let path = write(&dir, "form.pdf", &mut f);
    let mut doc = open(&path);
    let (first, second) = (doc.state().pages[0].id, doc.state().pages[1].id);
    let bytes = sheaf_doc::save::extract_bytes(&doc.snapshot(), &[second]).unwrap();
    assert_eq!(fields_of(&bytes), None, "extracted");
    assert!(!holds(&bytes, b"Alpha"), "extracted");
    let report = clean(&bytes, "extracted");
    assert_eq!(report.class(), "no form");

    doc.apply(Edit::DeletePages { ids: vec![first] }, "Delete")
        .unwrap();
    for (strategy, bytes) in saved_both_ways(&doc, &path, &dir.join("rewritten.pdf")) {
        let what = format!("{strategy:?}");
        assert_eq!(fields_of(&bytes), None, "{what}");
        assert!(!holds(&bytes, b"Alpha"), "{what}");
        assert_eq!(clean(&bytes, &what).class(), "no form", "{what}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn direct_object_fields_are_pruned() {
    // `/Fields` holds a parent field as a direct dictionary, whose kids
    // are widgets on the first page; a text field is on the second.
    let dir = scratch("direct");
    let mut f = Form::new(2);
    let a = blue(&mut f, "a", 0, 0, "LEAKVALUE123");
    let b = f.text("b", 0, row(1));
    let keep = blue(&mut f, "keep", 1, 0, "KEPT");
    for id in [a, b] {
        f.dict(id).remove(b"T");
    }
    let direct = dictionary! {
        "T" => Object::string_literal("group"),
        "Kids" => vec![Object::Reference(a), Object::Reference(b)],
    };
    f.dict(f.acroform).set(
        "Fields",
        vec![Object::Dictionary(direct), Object::Reference(keep)],
    );
    let path = write(&dir, "form.pdf", &mut f);
    let mut doc = open(&path);
    let first = doc.state().pages[0].id;
    doc.apply(Edit::DeletePages { ids: vec![first] }, "Delete")
        .unwrap();
    for (strategy, bytes) in saved_both_ways(&doc, &path, &dir.join("rewritten.pdf")) {
        let what = format!("{strategy:?}");
        let fields = fields_of(&bytes).unwrap();
        assert_eq!(fields.len(), 1, "{what}: {fields:?}");
        assert!(fields[0].as_reference().is_ok(), "{what}");
        assert!(!holds(&bytes, b"LEAKVALUE123"), "{what}");
        assert_eq!(values(&bytes)["keep"], "KEPT", "{what}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn page_templates_stay() {
    // A form that spawns pages keeps a template page outside the page
    // tree, with its own field.
    let dir = scratch("templates");
    let mut f = two_page_form();
    let template = f.pdf.add_object(dictionary! {
        "Type" => "Page", "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
    });
    let widget = f.pdf.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx", "P" => template,
        "T" => Object::string_literal("spawned"),
        "Rect" => vec![100.into(), 700.into(), 300.into(), 730.into()],
    });
    f.dict(template)
        .set("Annots", vec![Object::Reference(widget)]);
    let mut fields = f.fields();
    fields.push(widget);
    f.set_fields(&fields);
    let catalog = f.catalog;
    f.dict(catalog).set(
        "Names",
        dictionary! { "Templates" => dictionary! {
            "Names" => vec![Object::string_literal("extra"), Object::Reference(template)],
        } },
    );
    let path = write(&dir, "form.pdf", &mut f);
    let mut doc = open(&path);
    let second = doc.state().pages[1].id;
    doc.apply(Edit::DeletePages { ids: vec![second] }, "Delete")
        .unwrap();
    for (strategy, bytes) in saved_both_ways(&doc, &path, &dir.join("rewritten.pdf")) {
        let what = format!("{strategy:?}");
        let report = clean(&bytes, &what);
        assert_eq!((report.template_widgets, report.fields), (1, 4), "{what}");
        let pdf = Pdf::load_mem(&bytes).unwrap();
        let names = pdf.catalog().unwrap().get(b"Names").unwrap();
        let names = pdf.dereference(names).unwrap().1.as_dict().unwrap();
        let templates = pdf.dereference(names.get(b"Templates").unwrap()).unwrap().1;
        let entry = &templates
            .as_dict()
            .unwrap()
            .get(b"Names")
            .unwrap()
            .as_array()
            .unwrap()[1];
        let page = pdf.get_dictionary(entry.as_reference().unwrap()).unwrap();
        assert_eq!(
            page.get(b"Annots").unwrap().as_array().unwrap().len(),
            1,
            "{what}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Bug 5 as planning reproduced it: CA 540's `540-5007` (on the 2025
/// form, `540_form_5007`) filled with `LEAKVALUE123`, then page 5 deleted,
/// or page 6 extracted.
#[test]
fn corpus_ca_540_leaks_no_removed_value() {
    let Some(form) = corpus_form("forms/ca-ftb/2025-540.pdf") else {
        return;
    };
    let dir = scratch("ca540");
    let path = dir.join("540.pdf");
    std::fs::write(&path, filled(&form, "540_form_5007", "LEAKVALUE123")).unwrap();
    let mut doc = open(&path);
    let pages: Vec<PageId> = doc.state().pages.iter().map(|p| p.id).collect();
    let extracted = sheaf_doc::save::extract_bytes(&doc.snapshot(), &[pages[5]]).unwrap();
    assert!(!holds(&extracted, b"LEAKVALUE123"), "extracted");
    assert_eq!(findings(&extracted), vec![], "extracted");

    doc.apply(
        Edit::DeletePages {
            ids: vec![pages[4]],
        },
        "Delete",
    )
    .unwrap();
    for (strategy, bytes) in saved_both_ways(&doc, &path, &dir.join("rewritten.pdf")) {
        let what = format!("{strategy:?}");
        assert!(!holds(&bytes, b"LEAKVALUE123"), "{what}");
        assert_eq!(findings(&bytes), vec![], "{what}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Bug 1 on real forms: the 1040's first page and the W-9's first page
/// cropped, saved both ways, keep every field and its value.
#[test]
fn corpus_cropped_forms_keep_their_fields() {
    for (file, field) in [
        ("forms/irs/f1040.pdf", "topmostSubform[0].Page1[0].f1_14[0]"),
        ("forms/irs/fw9.pdf", "topmostSubform[0].Page1[0].f1_01[0]"),
    ] {
        let Some(form) = corpus_form(file) else {
            return;
        };
        let dir = scratch("corpus-crop");
        let path = dir.join("form.pdf");
        std::fs::write(&path, filled(&form, field, "Filled In")).unwrap();
        let input = std::fs::read(&path).unwrap();
        let before = check(&input, None).unwrap();
        // The fill has no appearance yet (slice 9 makes them).
        let expected = vec![sheaf_doc::forms::Finding::NoAppearance(vec![
            field.to_string(),
        ])];
        let mut doc = open(&path);
        let page = doc.state().pages[0].id;
        let margins = Margins {
            top: 10.0,
            left: 10.0,
            right: 10.0,
            bottom: 10.0,
        };
        let snap = doc.snapshot();
        let plan = sheaf_ops::crop::plan(
            doc.store(),
            &snap,
            &[page],
            CropSpec::Margins(margins),
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
        for (strategy, bytes) in written_both_ways(&doc) {
            let what = format!("{file}, {strategy:?}");
            let report = check(&bytes, None).unwrap();
            assert_eq!(
                (report.fields, report.widgets),
                (before.fields, before.widgets),
                "{what}"
            );
            assert_eq!(findings(&bytes), expected, "{what}");
            assert_eq!(values(&bytes)[field], "Filled In", "{what}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Bug 5 on every curated form with more than one page: each text field
/// whose widgets are all on the page with most of them is filled with a
/// marker, the page is deleted, and no marker is left in the rewrite.
#[test]
fn corpus_deleted_pages_leave_no_values() {
    let Some(corpus) = Corpus::or_skip() else {
        return;
    };
    let dir = scratch("corpus-delete");
    let (mut tried, mut failed) = (0, Vec::new());
    for entry in corpus.manifest().unwrap() {
        if entry.path.starts_with("forms/safedocs/") || entry.pages < 2 {
            continue;
        }
        let bytes = std::fs::read(corpus.path(&entry)).unwrap();
        let mut pdf = Pdf::load_mem(&bytes).unwrap();
        let form = form_in(&bytes);
        // Each text field's pages.
        let mut pages: BTreeMap<_, Vec<u32>> = BTreeMap::new();
        for (&page, widgets) in &form.widgets {
            for w in widgets {
                pages.entry(w.field).or_default().push(page);
            }
        }
        let pages = &pages;
        let alone = |page: u32| {
            form.fields.iter().filter(move |f| {
                f.kind == sheaf_doc::forms::FieldKind::Text
                    && pages
                        .get(&f.id)
                        .is_some_and(|p| p.iter().all(|&q| q == page))
            })
        };
        let Some(page) = (0..entry.pages).max_by_key(|&p| alone(p).count()) else {
            continue;
        };
        let objects: Vec<ObjectId> = alone(page).map(|f| f.object).collect();
        if objects.is_empty() {
            continue;
        }
        for (i, &object) in objects.iter().enumerate() {
            pdf.get_dictionary_mut(object)
                .unwrap()
                .set("V", Object::string_literal(format!("SHEAFLEAK{i}")));
        }
        let path = dir.join("form.pdf");
        pdf.save(&path).unwrap();
        let mut doc = open(&path);
        let id = doc.state().pages[page as usize].id;
        doc.apply(Edit::DeletePages { ids: vec![id] }, "Delete")
            .unwrap();
        let bytes = sheaf_doc::save::rewrite(&doc.snapshot()).unwrap();
        let dangling = dangling_structure(&bytes);
        if holds(&bytes, b"SHEAFLEAK") || dangling > 0 {
            // Kept, to look at.
            let name = entry.path.replace('/', "_");
            std::fs::write(dir.join(&name), &bytes).unwrap();
            failed.push(format!(
                "{}: page {} deleted, a value stayed or {dangling} structure references \
                 lead nowhere (in {})",
                entry.path,
                page + 1,
                dir.join(&name).display()
            ));
        }
        tried += 1;
    }
    println!("{tried} forms, a page of each deleted");
    assert!(failed.is_empty(), "{failed:#?}");
    let _ = std::fs::remove_dir_all(&dir);
}
