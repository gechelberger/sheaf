//! What a save takes out of a form, and asks first about (M10 slice 5).
//! A changed hybrid form loses its XFA copy, its tooltips kept in `/TU`
//! (bug 6); a rewrite drops Reader's usage rights, and `/SigFlags` when
//! nothing signed needs it (bug 7); an XFA copy holding values the
//! AcroForm lacks is found; a save that would rewrite a signed file asks,
//! and appending leaves its signed bytes as they were, but for an
//! encrypted one.
//!
//! Needs libpdfium (`scripts/fetch-pdfium.sh`), like the app itself.

use std::path::Path;

use lopdf::{Document as Pdf, EncryptionState, EncryptionVersion, Object, Permissions};
use sheaf_doc::forms::fixture::Form;
use sheaf_doc::forms::{Finding, check};
use sheaf_doc::save::{Signed, Strategy, WouldBreakSignatures, save_to_path_noting};
use sheaf_doc::{Document, Edit, SourceInfo};

mod common;
use common::*;

/// A two-page hybrid form as Designer names one: `form1[0].x[0]` on page
/// 1 ("Ex") and `form1[0].y[0]` on page 2 ("Why"), with an XFA copy whose
/// tooltips the AcroForm lacks.
fn hybrid() -> Form {
    let mut f = Form::new(2);
    let x = blue(&mut f, "x[0]", 0, 0, "Ex");
    let y = blue(&mut f, "y[0]", 1, 0, "Why");
    f.group("form1[0]", &[x, y]);
    f.xfa();
    f
}

/// The form's `/XFA`, `/Perms` and `/SigFlags`, as there or not.
fn parts(bytes: &[u8]) -> (bool, bool, bool) {
    let pdf = Pdf::load_mem(bytes).unwrap();
    let catalog = pdf.catalog().unwrap();
    let form = catalog
        .get(b"AcroForm")
        .ok()
        .and_then(|f| pdf.dereference(f).ok())
        .and_then(|(_, f)| f.as_dict().ok());
    (
        form.is_some_and(|f| f.has(b"XFA")),
        catalog.has(b"Perms"),
        form.is_some_and(|f| f.has(b"SigFlags")),
    )
}

/// Each field's own `/TU`, by full name.
fn tooltips(bytes: &[u8]) -> Vec<(String, Option<String>)> {
    let pdf = Pdf::load_mem(bytes).unwrap();
    form_in(bytes)
        .fields
        .into_iter()
        .map(|f| {
            let tu = pdf
                .get_dictionary(f.object)
                .unwrap()
                .get(b"TU")
                .ok()
                .map(sheaf_doc::file::pdf_text);
            (f.name, tu)
        })
        .collect()
}

#[test]
fn a_changed_hybrid_form_loses_its_xfa_copy() {
    // Bug 6: a page deleted, so the XFA describes a field that's gone.
    let dir = scratch("saves-xfa");
    let path = write(&dir, "form.pdf", &mut hybrid());
    let mut doc = open(&path);
    let second = doc.state().pages[1].id;
    doc.apply(Edit::DeletePages { ids: vec![second] }, "Delete")
        .unwrap();
    assert!(doc.form_changed());
    let mut saved = written_both_ways(&doc);
    saved.extend(saved_both_ways(&doc, &path, &dir.join("rewritten.pdf")));
    for (strategy, bytes) in saved {
        let what = format!("{strategy:?}");
        if strategy == Strategy::Rewrite {
            clean(&bytes, &what);
        }
        assert!(!parts(&bytes).0, "{what}: the XFA copy stayed");
        // Its tooltip is the AcroForm's now. (A recovery sidecar can't
        // take `y` out: a save rewrites.)
        let tips = tooltips(&bytes);
        assert!(
            tips.contains(&("form1[0].x[0]".into(), Some("Tip for x".into()))),
            "{what}: {tips:?}"
        );
        if strategy == Strategy::Rewrite {
            assert_eq!(tips.len(), 1, "{what}");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_unchanged_hybrid_form_keeps_its_xfa_copy() {
    // A page turned: the form is as it was.
    let dir = scratch("saves-xfa-kept");
    let path = write(&dir, "form.pdf", &mut hybrid());
    let mut doc = open(&path);
    let first = doc.state().pages[0].id;
    doc.apply(
        Edit::RotatePages {
            ids: vec![first],
            by: sheaf_core::Rotation::R90,
        },
        "Rotate",
    )
    .unwrap();
    assert!(!doc.form_changed());
    for (strategy, bytes) in saved_both_ways(&doc, &path, &dir.join("rewritten.pdf")) {
        let what = format!("{strategy:?}");
        clean(&bytes, &what);
        assert!(parts(&bytes).0, "{what}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_form_brought_into_a_hybrid_one_is_saved_in_full() {
    // An insert changes the form: the incremental save refuses, and the
    // full one takes the XFA copy out.
    let dir = scratch("saves-xfa-insert");
    let path = write(&dir, "form.pdf", &mut hybrid());
    let other = write(&dir, "other.pdf", &mut Form::new(1));
    let mut doc = open(&path);
    {
        let mut f = Form::new(1);
        blue(&mut f, "z", 0, 0, "Zed");
        std::fs::write(&other, f.bytes()).unwrap();
    }
    let info = SourceInfo::read(doc.store(), &other, None).unwrap();
    let form = sheaf_doc::forms::read(&info.src);
    let sid = doc.add_source(info);
    doc.set_source_form(sid, form);
    doc.apply(
        Edit::InsertPages {
            source: sid,
            range: 0..1,
            before: None,
            outline: None,
            annots: Vec::new(),
        },
        "Insert",
    )
    .unwrap();
    assert!(sheaf_doc::incremental::incremental(&doc.snapshot()).is_err());
    let (strategy, notes) = save_to_path_noting(&doc.snapshot(), &path, Signed::Ask).unwrap();
    assert_eq!(strategy, Strategy::Rewrite);
    assert!(
        notes.contains(&sheaf_doc::forms::rules::XFA_REMOVED.to_string()),
        "{notes:?}"
    );
    let bytes = std::fs::read(&path).unwrap();
    let report = clean(&bytes, "saved");
    assert_eq!(report.fields, 3);
    assert!(!parts(&bytes).0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn usage_rights_go_with_a_rewrite() {
    // Bug 7: they'd still sign the original's bytes.
    let dir = scratch("saves-rights");
    let mut f = hybrid();
    f.usage_rights();
    let path = write(&dir, "form.pdf", &mut f);
    assert_eq!(parts(&std::fs::read(&path).unwrap()), (true, true, true));
    let mut doc = open(&path);
    let first = doc.state().pages[0].id;
    doc.apply(
        Edit::RotatePages {
            ids: vec![first],
            by: sheaf_core::Rotation::R90,
        },
        "Rotate",
    )
    .unwrap();
    // Saved as it is: an update keeps them, still signing the original.
    let saved = saved_both_ways(&doc, &path, &dir.join("rewritten.pdf"));
    assert_eq!(saved[1].0, Strategy::Incremental);
    for (strategy, bytes) in saved {
        let what = format!("{strategy:?}");
        clean(&bytes, &what);
        let want = match strategy {
            // No signature needs `/SigFlags` now; the form is as it was.
            Strategy::Rewrite => (true, false, false),
            Strategy::Incremental => (true, true, true),
        };
        assert_eq!(parts(&bytes), want, "{what}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_xfa_copy_holding_values_the_acroform_lacks_is_found() {
    // `y` filled in the XFA's datasets only, as a tool that writes only XFA
    // leaves it.
    let mut f = hybrid();
    let y = f.fields()[0];
    let y = f.widgets_of(y)[1];
    f.xfa();
    f.dict(y).remove(b"V");
    let facts = form_in(&f.bytes()).facts;
    assert_eq!(facts.xfa_values, 1);
    // With its value back in the AcroForm, nothing's missing.
    f.dict(y).set("V", Object::string_literal("Why"));
    assert_eq!(form_in(&f.bytes()).facts.xfa_values, 0);
}

/// A two-page form with a signed signature field on page 1, and a text
/// field on each page.
fn signed() -> Form {
    let mut f = Form::new(2);
    blue(&mut f, "a", 0, 0, "Alpha");
    blue(&mut f, "b", 1, 0, "LEAKVALUE123");
    let sig = f.signature("sig", 0, row(2));
    f.sign(sig);
    f
}

/// Whether the file's signature fits it: its `/ByteRange` ends where a
/// revision ends.
fn signature_fits(bytes: &[u8]) -> bool {
    !check(bytes, None)
        .unwrap()
        .findings
        .iter()
        .any(|f| matches!(f, Finding::StaleSignature { .. }))
}

#[test]
fn a_save_that_would_rewrite_a_signed_file_asks() {
    let dir = scratch("saves-signed");
    let path = write(&dir, "signed.pdf", &mut signed());
    let original = std::fs::read(&path).unwrap();
    assert!(signature_fits(&original));
    let mut doc = open(&path);
    let second = doc.state().pages[1].id;
    doc.apply(Edit::DeletePages { ids: vec![second] }, "Delete")
        .unwrap();
    let snap = doc.snapshot();
    // Asked: nothing's written.
    let err = save_to_path_noting(&snap, &path, Signed::Ask).unwrap_err();
    let e = err.downcast_ref::<WouldBreakSignatures>().expect("asks");
    assert_eq!(
        *e,
        WouldBreakSignatures {
            signatures: 1,
            encrypted: false
        }
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
    // Rewritten elsewhere: the signature breaks, and the value goes.
    let copy = dir.join("rewritten.pdf");
    save_to_path_noting(&snap, &copy, Signed::Rewrite).unwrap();
    let rewritten = std::fs::read(&copy).unwrap();
    assert!(!signature_fits(&rewritten));
    assert!(!holds(&rewritten, b"LEAKVALUE123"));
    // Appended: the signed bytes are as they were, and the signature fits;
    // the deleted page's value stays in the earlier revision, as said.
    let (strategy, _) = save_to_path_noting(&snap, &path, Signed::Append).unwrap();
    assert_eq!(strategy, Strategy::Incremental);
    let appended = std::fs::read(&path).unwrap();
    assert!(appended.starts_with(&original));
    assert!(signature_fits(&appended));
    let report = check(&appended, None).unwrap();
    assert_eq!((report.pages, report.signed), (1, 1));
    // An unsigned file isn't asked about.
    let plain = write(&dir, "plain.pdf", &mut hybrid());
    let mut doc = open(&plain);
    let second = doc.state().pages[1].id;
    doc.apply(Edit::DeletePages { ids: vec![second] }, "Delete")
        .unwrap();
    save_to_path_noting(&doc.snapshot(), &plain, Signed::Ask).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_encrypted_signed_file_asks_with_no_append() {
    // Opened with its empty user password; sheaf can't append to it yet.
    let dir = scratch("saves-signed-encrypted");
    let mut f = signed();
    let id = Object::string_literal("0123456789abcdef");
    f.pdf.trailer.set("ID", vec![id.clone(), id]);
    let state = EncryptionState::try_from(EncryptionVersion::V2 {
        document: &f.pdf,
        owner_password: "owner",
        user_password: "",
        key_length: 128,
        permissions: Permissions::all(),
    })
    .unwrap();
    f.pdf.encrypt(&state).unwrap();
    let path = write(&dir, "signed.pdf", &mut f);
    let mut doc = open(&path);
    let second = doc.state().pages[1].id;
    doc.apply(Edit::DeletePages { ids: vec![second] }, "Delete")
        .unwrap();
    let err = save_to_path_noting(&doc.snapshot(), &path, Signed::Ask).unwrap_err();
    assert_eq!(
        err.downcast_ref::<WouldBreakSignatures>(),
        Some(&WouldBreakSignatures {
            signatures: 1,
            encrypted: true
        })
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_certified_file_asks_too() {
    let dir = scratch("saves-certified");
    let mut f = Form::new(2);
    let sig = f.signature("cert", 0, row(0));
    f.certify(sig);
    let path = write(&dir, "certified.pdf", &mut f);
    let mut doc = open(&path);
    let second = doc.state().pages[1].id;
    doc.apply(Edit::DeletePages { ids: vec![second] }, "Delete")
        .unwrap();
    let err = save_to_path_noting(&doc.snapshot(), &path, Signed::Ask).unwrap_err();
    assert_eq!(
        err.downcast_ref::<WouldBreakSignatures>()
            .map(|e| e.signatures),
        Some(1)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn signed_fields_brought_in_are_kept_and_said_so() {
    let dir = scratch("saves-signed-in");
    let a = write(&dir, "a.pdf", &mut Form::new(1));
    let s = write(&dir, "signed.pdf", &mut signed());
    let mut doc = open(&a);
    let info = SourceInfo::read(doc.store(), &s, None).unwrap();
    let form = sheaf_doc::forms::read(&info.src);
    let sid = doc.add_source(info);
    doc.set_source_form(sid, form);
    let out = doc
        .apply(
            Edit::InsertPages {
                source: sid,
                range: 0..1,
                before: None,
                outline: None,
                annots: Vec::new(),
            },
            "Insert",
        )
        .unwrap();
    assert_eq!(
        doc.form_notice(&out, false, false).as_deref(),
        Some("The signature from signed.pdf is no longer valid here")
    );
    let bytes = sheaf_doc::save::rewrite(&doc.snapshot()).unwrap();
    let report = check(&bytes, None).unwrap();
    assert_eq!(report.signed, 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The corpus's `path`, or None (and the test skips) without a corpus.
fn corpus_path(path: &str) -> Option<std::path::PathBuf> {
    let corpus = sheaf_doc::forms::corpus::Corpus::or_skip()?;
    let entry = corpus.entry(path).unwrap().expect("in the manifest");
    Some(corpus.path(&entry))
}

/// The file at `path` opened, its page `index` deleted, and rewritten.
fn without_page(path: &Path, index: usize) -> (Document, Vec<u8>) {
    let mut doc = open(path);
    let id = doc.state().pages[index].id;
    doc.apply(Edit::DeletePages { ids: vec![id] }, "Delete")
        .unwrap();
    let bytes = sheaf_doc::save::rewrite(&doc.snapshot()).unwrap();
    (doc, bytes)
}

#[test]
fn corpus_a_1040_page_deleted_keeps_the_others_tooltips() {
    let Some(f1040) = corpus_path("forms/irs/f1040.pdf") else {
        return;
    };
    let (_, bytes) = without_page(&f1040, 1);
    // Bug 6: the XFA for two pages in a one-page file; bug 7: usage rights
    // over the original's bytes.
    assert_eq!(parts(&bytes), (false, false, false));
    clean(&bytes, "1040 without page 2");
    let tips = tooltips(&bytes);
    let with = tips.iter().filter(|(_, t)| t.is_some()).count();
    assert!(
        with * 10 >= tips.len() * 9,
        "{with} of {} have /TU",
        tips.len()
    );
    let (name, tip) = tips
        .iter()
        .find(|(n, _)| n == "topmostSubform[0].Page1[0].f1_01[0]")
        .unwrap();
    assert!(
        tip.as_deref().is_some_and(|t| t.starts_with("Page 1.")),
        "{name}: {tip:?}"
    );
}

#[test]
fn corpus_the_w9s_instructions_page_alone_has_no_form() {
    // Bug 6: its page 2, extracted, kept empty `/Fields` with the full XFA
    // and `/Perms`.
    let Some(fw9) = corpus_path("forms/irs/fw9.pdf") else {
        return;
    };
    let doc = open(&fw9);
    let second = doc.state().pages[1].id;
    let bytes = sheaf_doc::save::extract_bytes(&doc.snapshot(), &[second]).unwrap();
    let report = clean(&bytes, "W-9 page 2");
    assert_eq!(report.class(), "no form");
    assert_eq!(parts(&bytes), (false, false, false));
}
