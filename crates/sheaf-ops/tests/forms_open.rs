//! A form opens into the document model as the app opens it (M10 slice
//! 2): its fields, and their widgets on the pages. A damaged field tree
//! opens with no fields, and the file still opens. PDFium's form type
//! agrees with the class sheaf reads.
//!
//! One test, so PDFium is only ever called from one thread (see the slice
//! notes in `todos/m10.md`, slice 1). Needs libpdfium
//! (`scripts/fetch-pdfium.sh`), like the app itself.

use std::path::{Path, PathBuf};

use lopdf::{Object, dictionary};
use sheaf_doc::forms::FormClass;
use sheaf_doc::forms::fixture::{Form, every_kind, row};
use sheaf_render::pdfium::PdfiumEngine;
use sheaf_render::{FormType, Renderer};

fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sheaf-forms-open-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, name: &str, form: &mut Form) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, form.bytes()).unwrap();
    path
}

/// A field tree that loops: `a`'s kid is `b`, whose kid is `a`.
fn cyclic() -> Form {
    let mut f = Form::new(1);
    f.text("ok", 0, row(0));
    let a = f
        .pdf
        .add_object(dictionary! { "T" => Object::string_literal("a") });
    let b = f.pdf.add_object(
        dictionary! { "T" => Object::string_literal("b"), "Kids" => vec![Object::Reference(a)] },
    );
    f.dict(a).set("Kids", vec![Object::Reference(b)]);
    f.dict(a).set("Parent", b);
    f.dict(b).set("Parent", a);
    let mut fields = f.fields();
    fields.push(a);
    f.set_fields(&fields);
    f
}

/// A field that's its own kid and parent.
fn own_parent() -> Form {
    let mut f = Form::new(1);
    let a = f.text("a", 0, row(0));
    f.dict(a).set("Kids", vec![Object::Reference(a)]);
    f.dict(a).set("Parent", a);
    f
}

/// A field 10,000 parents deep.
fn deep() -> Form {
    let mut f = Form::new(1);
    let mut below = f.text("leaf", 0, row(0));
    for i in 0..10_000 {
        let up = f.pdf.add_object(dictionary! {
            "T" => Object::string_literal(format!("n{i}")),
            "Kids" => vec![Object::Reference(below)],
        });
        f.dict(below).set("Parent", up);
        below = up;
    }
    f.set_fields(&[below]);
    f
}

#[test]
fn forms_open_into_the_model() {
    let dir = scratch();
    let path = write(&dir, "every-kind.pdf", &mut every_kind());
    let doc = sheaf_ops::open(&path, None).unwrap();
    let st = doc.state();
    assert_eq!(st.fields.len(), 9);
    let on_page: Vec<usize> = st
        .pages
        .iter()
        .map(|p| st.widgets_on(p.id).map_or(0, |w| w.len()))
        .collect();
    assert_eq!(on_page, [6, 4]);
    let (_, facts) = sheaf_doc::file::read_open(&path, None).unwrap();
    assert_eq!(facts.form.facts.class, FormClass::AcroForm);
    let engine = PdfiumEngine::open(&path, None).unwrap();
    assert_eq!(engine.form_type(), FormType::AcroForm);

    // PDFium tells hybrid and XFA-only forms apart as sheaf does.
    let mut hybrid = every_kind();
    hybrid.xfa();
    let mut only = every_kind();
    only.xfa_only();
    let mut none = Form::new(1);
    none.dict(none.catalog).remove(b"AcroForm");
    for (name, form, class, form_type) in [
        (
            "hybrid.pdf",
            hybrid,
            FormClass::Hybrid,
            FormType::XfaForeground,
        ),
        ("xfa-only.pdf", only, FormClass::XfaOnly, FormType::XfaFull),
        ("none.pdf", none, FormClass::None, FormType::None),
    ] {
        let path = write(&dir, name, &mut { form });
        let (_, facts) = sheaf_doc::file::read_open(&path, None).unwrap();
        assert_eq!(facts.form.facts.class, class, "{name}");
        let engine = PdfiumEngine::open(&path, None).unwrap();
        assert_eq!(engine.form_type(), form_type, "{name}");
    }

    // A damaged tree: the file opens, with no fields to fill.
    for (name, mut form, why) in [
        ("cyclic.pdf", cyclic(), "its own ancestor"),
        ("own-parent.pdf", own_parent(), "its own ancestor"),
        ("deep.pdf", deep(), "more than 32 deep"),
    ] {
        let path = write(&dir, name, &mut form);
        let doc = sheaf_ops::open(&path, None).unwrap();
        assert_eq!(doc.page_count(), 1, "{name}");
        assert!(doc.state().fields.is_empty(), "{name}");
        assert!(doc.state().widgets.is_empty(), "{name}");
        let (_, facts) = sheaf_doc::file::read_open(&path, None).unwrap();
        let facts = facts.form.facts;
        let damaged = facts.damaged.clone().unwrap_or_default();
        assert!(damaged.contains(why), "{name}: {damaged:?}");
        assert!(!facts.is_fillable());
        assert!(facts.to_string().ends_with("so not fillable"), "{facts}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
