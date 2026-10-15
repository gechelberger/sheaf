//! The form check on forms built in code: a clean one of every kind, then
//! one for each problem it looks for. And on the corpus's 1040-ES, when
//! the corpus is there.

use lopdf::{Document as Pdf, Object, ObjectId, Stream, dictionary};

use super::check::{Finding, Report, check};
use super::corpus::{Corpus, parse_manifest};
use super::fixture::{Form, append_revision};

/// Rows down the page, 30 pt apart.
fn row(i: usize) -> [f32; 4] {
    let top = 700.0 - 30.0 * i as f32;
    [72.0, top - 20.0, 272.0, top]
}

/// One of every kind of field, filled where it can be, under `form1[0]`
/// as Designer names them.
fn every_kind() -> Form {
    let mut f = Form::new(2);
    let name = f.text("name[0]", 0, row(0));
    f.fill(name, "Ann Lee");
    let empty = f.text("empty[0]", 0, row(1));
    let on = f.check_box("on[0]", 0, row(2), true);
    let off = f.check_box("off[0]", 0, row(3), false);
    let radio = f.radio("pick[0]", 0, &[row(4), row(5)], Some(1));
    let combo = f.choice("state[0]", 1, row(0), &["CA", "NY"], true);
    f.fill(combo, "NY");
    let list = f.choice("kind[0]", 1, row(1), &["W-2", "1099"], false);
    f.fill(list, "1099");
    let button = f.push_button("print[0]", 1, row(2), "Print");
    let sig = f.signature("sig[0]", 1, row(3));
    f.group(
        "form1[0]",
        &[name, empty, on, off, radio, combo, list, button, sig],
    );
    f
}

fn checked(bytes: &[u8]) -> Report {
    check(bytes, None).unwrap()
}

/// `report`'s findings, with what it printed when that isn't `want`.
fn assert_findings(report: &Report, want: &[Finding]) {
    assert_eq!(report.findings, want, "\n{report}");
}

#[test]
fn a_form_of_every_kind_is_clean() {
    let mut f = every_kind();
    f.xfa();
    f.usage_rights();
    f.tag();
    let report = checked(&f.bytes());
    assert_findings(&report, &[]);
    assert_eq!(report.class(), "hybrid");
    assert_eq!((report.fields, report.widgets, report.pages), (9, 10, 2));
    assert!(report.usage_rights && !report.certified);
    assert_eq!((report.signature_fields, report.signed), (1, 0));
    assert_eq!(report.sig_flags, 2);
    assert_eq!(report.revisions, 1);
    let xfa = report.xfa.as_ref().unwrap();
    assert_eq!(
        xfa.packets,
        ["preamble", "template", "datasets", "postamble"]
    );
    assert!(!xfa.dynamic);
    assert!(report.to_string().ends_with("No problems found.\n"));
}

#[test]
fn widgets_no_field_reaches() {
    let mut f = every_kind();
    let form1 = f.fields();
    f.text("lost", 0, row(6));
    // Off /Fields, still on the page: what Combine leaves (bug 2).
    f.set_fields(&form1);
    let report = checked(&f.bytes());
    assert_findings(&report, &[Finding::Orphans(vec!["lost".into()])]);
    assert_eq!(report.fields, 9);
}

#[test]
fn widgets_on_two_pages_or_with_the_wrong_page() {
    let mut f = every_kind();
    let shared = f.text("shared", 0, row(6));
    let mut annots = f.annots(1);
    annots.push(shared);
    f.set_annots(1, &annots);
    // Duplicate Pages' copy, today (bug 4): on both pages, its /P one of
    // them.
    assert_findings(
        &checked(&f.bytes()),
        &[
            Finding::SharedWidgets(vec!["shared".into()]),
            Finding::WrongPage(vec!["shared".into()]),
        ],
    );

    let mut f = every_kind();
    let wrong = f.text("wrong", 0, row(6));
    let page2 = f.pages[1];
    f.dict(wrong).set("P", page2);
    let null = f.text("null", 0, row(7));
    f.dict(null).set("P", Object::Null);
    let gone = f.text("gone", 0, row(8));
    f.dict(gone).set("P", (999, 0));
    // /P is optional.
    let unset = f.text("unset", 0, row(9));
    f.dict(unset).remove(b"P");
    assert_findings(
        &checked(&f.bytes()),
        &[
            Finding::WrongPage(vec!["wrong".into()]),
            Finding::NullPage(vec!["gone".into(), "null".into()]),
        ],
    );
}

#[test]
fn widgets_on_no_page() {
    let mut f = every_kind();
    let stray = f.text("stray", 0, row(6));
    f.fill(stray, "LEAKVALUE123");
    let annots: Vec<_> = f.annots(0).into_iter().filter(|&a| a != stray).collect();
    f.set_annots(0, &annots);
    f.dict(stray).set("P", Object::Null);
    // Bug 5's stray: in the file and in /Fields, on no page.
    assert_findings(
        &checked(&f.bytes()),
        &[Finding::OffPage(vec!["stray".into()])],
    );
}

#[test]
fn widgets_on_page_templates() {
    // A form that spawns pages keeps the template outside the page tree,
    // its widgets on no page (GA 500, OH IT 1040). That's not a leak.
    let mut f = every_kind();
    let spawned = f.text("spawned", 0, row(6));
    let annots: Vec<_> = f.annots(0).into_iter().filter(|&a| a != spawned).collect();
    f.set_annots(0, &annots);
    let template = f.pdf.add_object(dictionary! {
        "Type" => "Page",
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        "Annots" => vec![Object::Reference(spawned)],
    });
    f.dict(spawned).set("P", template);
    let templates =
        dictionary! { "Names" => vec![Object::string_literal("extra"), template.into()] };
    f.dict(f.catalog)
        .set("Names", dictionary! { "Templates" => templates });
    let report = checked(&f.bytes());
    assert_findings(&report, &[]);
    assert_eq!((report.widgets, report.template_widgets), (10, 1));
    assert!(report.to_string().contains("1 widget on page templates"));
}

#[test]
fn kids_whose_parent_is_another_field() {
    let mut f = Form::new(1);
    let a = f.radio("a", 0, &[row(0), row(1)], Some(0));
    let b = f.radio("b", 0, &[row(2), row(3)], Some(0));
    // In a's /Kids, but b's by /Parent; and one of b's with no /Parent.
    let [_, a2] = f.widgets_of(a)[..] else {
        panic!()
    };
    f.dict(a2).set("Parent", b);
    let [_, b2] = f.widgets_of(b)[..] else {
        panic!()
    };
    f.dict(b2).remove(b"Parent");
    // A field under a parent field, with no /Parent.
    let x = f.text("x", 0, row(4));
    f.group("g", &[x]);
    f.dict(x).remove(b"Parent");
    assert_findings(
        &checked(&f.bytes()),
        &[Finding::WrongParent(vec![
            "a".into(),
            "b".into(),
            "g".into(),
        ])],
    );
}

#[test]
fn fonts_da_names_that_dr_lacks() {
    let mut f = every_kind();
    let arial = f.text("arial", 0, row(6));
    f.dict(arial)
        .set("DA", Object::string_literal("/ArialMT 9 Tf 0 g"));
    // No /DA of its own: the form's names the font.
    let plain = f.text("plain", 0, row(7));
    f.dict(plain).remove(b"DA");
    let also = f.text("also", 0, row(8));
    f.dict(also).remove(b"DA");
    f.dict(f.acroform).set(
        "DA",
        Object::string_literal("/HelveticaLTStd-Bold 0 Tf 0 g"),
    );
    // Nor does a kid of a parent field that has one.
    let kid = f.text("kid", 1, row(5));
    f.dict(kid).remove(b"DA");
    let parent = f.group("parent", &[kid]);
    f.dict(parent)
        .set("DA", Object::string_literal("/Courier 9 Tf 0 g"));
    let report = checked(&f.bytes());
    assert_findings(
        &report,
        &[Finding::MissingFonts(vec![
            ("HelveticaLTStd-Bold".into(), 2),
            ("ArialMT".into(), 1),
            ("Courier".into(), 1),
        ])],
    );
    assert!(
        report
            .to_string()
            .contains("HelveticaLTStd-Bold ×2, ArialMT ×1, Courier ×1")
    );
}

#[test]
fn names_are_compared_as_bytes() {
    // A check box's state and a font in Latin-1, not UTF-8.
    let oui = b"Oui\xE9".to_vec();
    let mut f = Form::new(1);
    let cb = f.check_box("cb", 0, row(0), true);
    let d = f.dict(cb);
    let states = d.get_mut(b"AP").and_then(|ap| ap.as_dict_mut()).unwrap();
    let states = states.get_mut(b"N").and_then(|n| n.as_dict_mut()).unwrap();
    let on = states.remove(b"Yes").unwrap();
    states.set(oui.clone(), on);
    d.set("V", Object::Name(oui.clone()));
    d.set("AS", Object::Name(oui));
    let text = f.text("text", 0, row(1));
    f.dict(text)
        .set("DA", Object::string_literal("/F#E9 10 Tf 0 g"));
    let helv = f.helv;
    let dr = f
        .dict(f.acroform)
        .get_mut(b"DR")
        .and_then(|d| d.as_dict_mut());
    let fonts = dr.unwrap().get_mut(b"Font").and_then(|d| d.as_dict_mut());
    fonts.unwrap().set(b"F\xE9".to_vec(), helv);
    assert_findings(&checked(&f.bytes()), &[]);
}

#[test]
fn full_names_more_than_one_field_has() {
    let mut f = Form::new(1);
    f.text("a", 0, row(0));
    f.text("a", 0, row(1));
    f.text("b", 0, row(2));
    let report = checked(&f.bytes());
    assert_findings(&report, &[Finding::DuplicateNames(vec!["a".into()])]);
}

#[test]
fn values_with_no_appearance() {
    let mut f = Form::new(1);
    let bare = f.text("bare", 0, row(0));
    f.dict(bare)
        .set("V", Object::string_literal("shown only by PDFium"));
    let drawn = f.text("drawn", 0, row(1));
    f.fill(drawn, "drawn");
    // Checked, and showing its off state.
    let off = f.check_box("box", 0, row(2), false);
    f.dict(off).set("V", Object::Name(b"Yes".to_vec()));
    // An empty value needs nothing.
    let empty = f.text("empty", 0, row(3));
    f.dict(empty).set("V", Object::string_literal(""));
    // Nor does a hidden widget (scripts keep values there); one that
    // isn't viewed is still printed.
    let hidden = f.text("hidden", 0, row(4));
    f.dict(hidden)
        .set("V", Object::string_literal("for scripts"));
    f.dict(hidden).set("F", 4 | 2);
    let noview = f.text("noview", 0, row(5));
    f.dict(noview).set("V", Object::string_literal("printed"));
    f.dict(noview).set("F", 4 | 32);
    let report = checked(&f.bytes());
    assert_findings(
        &report,
        &[Finding::NoAppearance(vec![
            "bare".into(),
            "box".into(),
            "noview".into(),
        ])],
    );
}

#[test]
fn values_an_earlier_revision_holds() {
    let mut f = Form::new(1);
    let name = f.text("name", 0, row(0));
    f.fill(name, "Last client");
    let empty = f.text("empty", 0, row(1));
    let gone = f.text("gone", 0, row(2));
    f.fill(gone, "Cleared");
    let original = f.bytes();
    assert_eq!(checked(&original).revisions, 1);

    // Reader appends: a replaced value, and a cleared one.
    let saved = append_revision(&original, &[name, gone], |pdf| {
        let d = pdf.get_dictionary_mut(name).unwrap();
        d.set("V", Object::string_literal("This client"));
        pdf.get_dictionary_mut(gone).unwrap().remove(b"V");
    });
    let report = checked(&saved);
    assert_eq!(report.revisions, 2);
    assert_findings(
        &report,
        &[Finding::EarlierValues {
            revision: 1,
            names: vec!["gone".into(), "name".into()],
        }],
    );

    // Filling only empty fields leaves nothing behind. (The appearances
    // are stale: the check doesn't look inside them.)
    let filled = append_revision(&original, &[empty], |pdf| {
        let d = pdf.get_dictionary_mut(empty).unwrap();
        d.set("V", Object::string_literal("New"));
    });
    let report = checked(&filled);
    assert_findings(&report, &[Finding::NoAppearance(vec!["empty".into()])]);
}

#[test]
fn an_eof_in_a_stream_or_a_trailer_written_twice_isnt_a_revision() {
    // An attached PDF, not compressed, ends in its own trailer.
    let mut f = Form::new(1);
    let name = f.text("name", 0, row(0));
    f.fill(name, "Ann Lee");
    let attached = Form::new(1).bytes();
    let attached = f.pdf.add_object(Stream::new(
        dictionary! { "Type" => "EmbeddedFile" },
        attached,
    ));
    f.dict(f.catalog).set("Attached", attached);
    let bytes = f.bytes();
    let report = checked(&bytes);
    assert_eq!(report.revisions, 1, "{report}");
    assert_findings(&report, &[]);

    // Some writers append the same trailer again.
    let tail = bytes.windows(9).rposition(|w| w == b"startxref").unwrap();
    let mut twice = bytes.clone();
    twice.extend_from_slice(&bytes[tail..]);
    twice.extend_from_slice(&bytes[tail..]);
    assert_eq!(checked(&twice).revisions, 1);
}

#[test]
fn an_xfa_copy_that_no_longer_fits() {
    let mut f = every_kind();
    f.xfa();
    // The combo box goes with its page (bug 6), and a field comes that
    // the XFA copy doesn't have.
    let form1 = f.fields()[0];
    let combo = f.widgets_of(form1)[5];
    let kids: Vec<Object> = f
        .widgets_of(form1)
        .into_iter()
        .filter(|&k| k != combo)
        .map(Object::Reference)
        .collect();
    f.dict(form1).set("Kids", kids);
    let annots: Vec<_> = f.annots(1).into_iter().filter(|&a| a != combo).collect();
    f.set_annots(1, &annots);
    f.text("extra", 0, row(6));
    assert_findings(
        &checked(&f.bytes()),
        &[
            Finding::OffPage(vec!["form1[0].state[0]".into()]),
            Finding::XfaMismatch {
                xfa_only: vec!["form1.state".into()],
                acroform_only: vec!["extra".into()],
            },
        ],
    );

    // No fields left at all (bug 6's W-9 page 2).
    let mut f = every_kind();
    f.xfa();
    f.set_fields(&[]);
    let report = checked(&f.bytes());
    let Some(Finding::XfaMismatch { xfa_only, .. }) = report.findings.last() else {
        panic!("{report}");
    };
    assert_eq!(xfa_only.len(), 9);
}

#[test]
fn the_fixtures_xfa_escapes_values() {
    let mut f = every_kind();
    let name = f.widgets_of(f.fields()[0])[0];
    f.fill(name, "Lee & <Co>");
    f.xfa();
    let acroform = f.pdf.get_dictionary(f.acroform).unwrap();
    let xfa = acroform.get(b"XFA").unwrap().as_array().unwrap();
    let datasets = f.pdf.get_object(xfa[5].as_reference().unwrap());
    let datasets = &datasets.unwrap().as_stream().unwrap().content;
    let datasets = String::from_utf8_lossy(datasets);
    assert!(
        datasets.contains("<name>Lee &amp; &lt;Co&gt;</name>"),
        "{datasets}"
    );
    assert_findings(&checked(&f.bytes()), &[]);
}

#[test]
fn xfa_only_forms() {
    let mut f = every_kind();
    f.xfa_only();
    let report = checked(&f.bytes());
    assert_eq!(report.class(), "XFA-only");
    assert!(report.xfa.as_ref().unwrap().dynamic);
    assert_findings(&report, &[]);
}

#[test]
fn usage_rights_after_a_rewrite() {
    let mut f = every_kind();
    f.usage_rights();
    let bytes = f.bytes();
    assert!(checked(&bytes).is_clean());
    // A full rewrite keeps /Perms with a /ByteRange for the old bytes
    // (bug 7).
    let mut pdf = Pdf::load_mem(&bytes).unwrap();
    let mut rewritten = Vec::new();
    pdf.save_to(&mut rewritten).unwrap();
    let report = checked(&rewritten);
    assert!(report.usage_rights);
    let [Finding::StaleSignature { what, covers, size }] = &report.findings[..] else {
        panic!("{report}");
    };
    assert!(what.contains("/UR3"));
    assert_eq!(*covers, bytes.len() as u64);
    assert_eq!(*size, rewritten.len() as u64);
}

#[test]
fn signatures_fit_their_revision() {
    let mut f = Form::new(1);
    let field = f.signature("Signature1", 0, row(0));
    let name = f.text("name", 0, row(1));
    f.sign(field);
    let signed = f.bytes();
    let report = checked(&signed);
    assert_findings(&report, &[]);
    assert_eq!(
        (report.signature_fields, report.signed, report.sig_flags),
        (1, 1, 3)
    );

    // An append leaves the signed revision whole.
    let appended = append_revision(&signed, &[name], |pdf| {
        let d = pdf.get_dictionary_mut(name).unwrap();
        d.set("TU", Object::string_literal("Your name"));
    });
    assert_findings(&checked(&appended), &[]);

    // A certification is one signature, in /Perms and in its field.
    let mut f = Form::new(1);
    let field = f.signature("Author", 0, row(0));
    f.certify(field);
    let certified = f.bytes();
    let report = checked(&certified);
    assert!(report.certified);
    assert_findings(&report, &[]);
    let mut pdf = Pdf::load_mem(&certified).unwrap();
    let mut rewritten = Vec::new();
    pdf.save_to(&mut rewritten).unwrap();
    let report = checked(&rewritten);
    assert!(
        matches!(&report.findings[..], [Finding::StaleSignature { what, .. }] if what.contains("DocMDP")),
        "{report}"
    );
}

#[test]
fn sig_flags_with_nothing_to_flag() {
    let mut f = Form::new(1);
    f.text("name", 0, row(0));
    f.dict(f.acroform).set("SigFlags", 3);
    assert_findings(
        &checked(&f.bytes()),
        &[Finding::StaleSigFlags {
            flags: 3,
            why: "no signature field",
        }],
    );
    // Usage rights removed, SigFlags left (what slice 5 must not do).
    let mut f = Form::new(1);
    f.signature("Signature1", 0, row(0));
    f.dict(f.acroform).set("SigFlags", 2);
    assert_findings(
        &checked(&f.bytes()),
        &[Finding::StaleSigFlags {
            flags: 2,
            why: "nothing signed",
        }],
    );
}

#[test]
fn damaged_trees_are_reported_not_followed() {
    // A cycle through /Kids and /Parent.
    let mut f = Form::new(1);
    let a = f
        .pdf
        .add_object(dictionary! { "T" => Object::string_literal("a") });
    let b = f.pdf.add_object(
        dictionary! { "T" => Object::string_literal("b"), "Kids" => vec![Object::Reference(a)] },
    );
    f.dict(a).set("Kids", vec![Object::Reference(b)]);
    f.dict(a).set("Parent", b);
    f.dict(b).set("Parent", a);
    f.set_fields(&[a]);
    let report = checked(&f.bytes());
    assert!(
        matches!(&report.findings[..], [Finding::Damaged(r)] if r.contains("twice")),
        "{report}"
    );

    // A field that is its own kid and parent.
    let mut f = Form::new(1);
    let a = f
        .pdf
        .add_object(dictionary! { "T" => Object::string_literal("a") });
    f.dict(a).set("Kids", vec![Object::Reference(a)]);
    f.dict(a).set("Parent", a);
    f.set_fields(&[a]);
    assert!(matches!(
        &checked(&f.bytes()).findings[..],
        [Finding::Damaged(_)]
    ));

    // A chain 10,000 deep.
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
    let report = checked(&f.bytes());
    assert!(
        report
            .findings
            .iter()
            .any(|x| matches!(x, Finding::Damaged(r) if r.contains("32 deep"))),
        "{report}"
    );

    // More than 100,000 fields.
    let mut f = Form::new(1);
    let fields: Vec<ObjectId> = (0..=100_000)
        .map(|i| {
            let t = Object::string_literal(format!("f{i}"));
            f.pdf.add_object(dictionary! { "T" => t })
        })
        .collect();
    f.set_fields(&fields);
    let report = checked(&f.bytes());
    assert_eq!(report.fields, 100_000);
    assert_findings(
        &report,
        &[Finding::Damaged(
            "the field tree has more than 100000 nodes".into(),
        )],
    );
}

#[test]
fn names_past_the_limit_are_damage() {
    // A full name longer than 64 KiB isn't read.
    let mut f = Form::new(1);
    let kids: Vec<ObjectId> = (0..3)
        .map(|i| f.text(&format!("k{i}"), 0, row(i)))
        .collect();
    f.group(&"n".repeat(70_000), &kids);
    assert_findings(
        &checked(&f.bytes()),
        &[
            Finding::Orphans(vec!["….k0".into(), "….k1".into(), "….k2".into()]),
            Finding::Damaged("a full name is longer than 64 KiB".into()),
        ],
    );

    // A parent's name is in each of its kids': names stop at 64 MiB in
    // all, not at memory's end.
    let mut f = Form::new(1);
    let kids: Vec<ObjectId> = (0..1100)
        .map(|i| f.text(&format!("k{i}"), 0, [0.0, 0.0, 10.0, 10.0]))
        .collect();
    f.group(&"n".repeat(65_000), &kids);
    let report = checked(&f.bytes());
    let damaged = Finding::Damaged("the full names come to more than 64 MiB".into());
    assert!(report.findings.contains(&damaged), "{report}");
    assert!(report.fields < 1100);

    // Nor is an XFA path that long compared.
    let mut f = every_kind();
    f.xfa();
    let xfa = f.pdf.get_dictionary(f.acroform).unwrap().get(b"XFA");
    let template = xfa.unwrap().as_array().unwrap()[3].as_reference().unwrap();
    let Object::Stream(s) = f.pdf.get_object_mut(template).unwrap() else {
        panic!()
    };
    let xml = String::from_utf8(s.content.clone()).unwrap();
    let long = format!(r#"<subform name="{}">"#, "n".repeat(70_000));
    s.set_content(xml.replace(r#"<subform name="form1">"#, &long).into_bytes());
    assert_findings(&checked(&f.bytes()), &[]);
}

#[test]
fn text_from_the_file_prints_escaped() {
    let mut f = Form::new(1);
    let sig = f.signature("sig\u{1b}[2J", 0, row(0));
    f.sign(sig);
    let text = f.text("text", 0, row(1));
    f.dict(text)
        .set("DA", Object::string_literal("/F#1B 10 Tf 0 g"));
    let packet = f
        .pdf
        .add_object(Stream::new(dictionary! {}, b"<x/>".to_vec()));
    let xfa = vec![Object::string_literal("pre\u{1b}[31mamble"), packet.into()];
    f.dict(f.acroform).set("XFA", xfa);
    // Rewritten, so the signature is stale and named.
    let mut pdf = Pdf::load_mem(&f.bytes()).unwrap();
    let mut rewritten = Vec::new();
    pdf.save_to(&mut rewritten).unwrap();
    let out = checked(&rewritten).to_string();
    assert!(!out.contains('\u{1b}'), "{out:?}");
    for shown in [
        r"XFA: pre\u{1b}[31mamble",
        r#"the signature in "sig\u{1b}[2J""#,
        r"F\u{1b} ×1",
    ] {
        assert!(out.contains(shown), "{shown} in {out}");
    }
}

#[test]
fn encrypted_forms_are_read_with_their_password() {
    use lopdf::{EncryptionState, EncryptionVersion, Permissions, StringFormat};
    let mut f = every_kind();
    let bare = f.text("bare", 0, row(6));
    f.dict(bare)
        .set("V", Object::string_literal("no appearance"));
    let mut pdf = Pdf::load_mem(&f.bytes()).unwrap();
    let id = Object::String(b"0123456789abcdef".to_vec(), StringFormat::Hexadecimal);
    pdf.trailer.set("ID", vec![id.clone(), id]);
    let state = EncryptionState::try_from(EncryptionVersion::V2 {
        document: &pdf,
        owner_password: "owner",
        user_password: "secret",
        key_length: 128,
        permissions: Permissions::default(),
    })
    .unwrap();
    pdf.encrypt(&state).unwrap();
    let mut encrypted = Vec::new();
    pdf.save_to(&mut encrypted).unwrap();

    let report = check(&encrypted, Some("secret")).unwrap();
    assert_eq!(report.fields, 10);
    assert_findings(&report, &[Finding::NoAppearance(vec!["bare".into()])]);
    assert!(check(&encrypted, Some("wrong")).is_err());
    assert!(check(&encrypted, None).is_err());
}

#[test]
fn the_manifest_by_its_header() {
    let text = "path\tpages\tkind\tfields\tfield_types\tfield_scripts\tlicense\textra\n\
                forms/irs/f1040.pdf\t2\thybrid\t199\tcheck:73,text:126\t-\tpublic-domain\tx\n\
                forms/ny-tax/it201.pdf\t\tacroform\t5\ttext:5\tformat:2,keystroke:2\trestricted\n";
    let rows = parse_manifest(text).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].path, "forms/irs/f1040.pdf");
    assert_eq!((rows[0].pages, rows[0].fields), (2, 199));
    assert_eq!(rows[0].field_types["text"], 126);
    assert!(rows[0].field_scripts.is_empty());
    assert!(!rows[0].restricted() && rows[1].restricted());
    assert_eq!(rows[1].pages, 0);
    assert_eq!(rows[1].field_scripts["keystroke"], 2);
    assert_eq!(rows[1].sha256, "");
    assert!(parse_manifest("form\ttitle\n").is_err());
}

/// Where the corpus is: the first place with a manifest. With none, the
/// corpus tests skip.
#[test]
fn the_corpus_is_where_its_manifest_is() {
    let dir = std::env::temp_dir().join(format!("sheaf-corpus-{}", std::process::id()));
    let (none, some) = (dir.join("none"), dir.join("some"));
    std::fs::create_dir_all(&none).unwrap();
    std::fs::create_dir_all(&some).unwrap();
    assert!(Corpus::first_of([dir.join("missing"), none.clone()]).is_none());
    std::fs::write(some.join("manifest.tsv"), "path\tpages\nforms/a.pdf\t1\n").unwrap();
    let found = Corpus::first_of([none, some.clone()]).unwrap();
    assert_eq!(found.root, some);
    let rows = found.manifest().unwrap();
    assert_eq!(found.path(&rows[0]), some.join("forms/a.pdf"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The planning probe's 1040-ES finding (slice 1's check): fonts its
/// fields name that its `/DR` doesn't have.
#[test]
fn corpus_1040_es_fonts_missing_from_dr() {
    let Some(corpus) = Corpus::or_skip() else {
        return;
    };
    let Some(entry) = corpus.entry("forms/irs/f1040es.pdf").unwrap() else {
        eprintln!("skipped: the corpus has no forms/irs/f1040es.pdf");
        return;
    };
    let report = super::check_file(&corpus.path(&entry), None).unwrap();
    assert_eq!(report.fields, entry.fields as usize);
    assert_findings(
        &report,
        &[Finding::MissingFonts(vec![
            ("HelveticaLTStd-Bold".into(), 110),
            ("ArialMT".into(), 4),
        ])],
    );
}
