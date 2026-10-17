//! The form reader on forms built in code, written and read back from
//! their sources: every kind of field and what it inherits, its widgets on
//! pages, the facts, and damaged trees, which read as not fillable; and
//! hybrid-reference files, built and from the corpus. Then any tree at
//! all, as a proptest.

use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use lopdf::{Object, ObjectId, Stream, StringFormat, dictionary};
use proptest::prelude::*;

use super::*;
use crate::SourceInfo;
use crate::forms::corpus::Corpus;
use crate::forms::field::{annot_flags, flags};
use crate::forms::fixture::{Form, every_kind, hybrid_reference, hybrid_update, row, source};
use crate::store::{BlobKind, Store};

/// The form as a file holds it, written and read back.
fn read_back(f: &mut Form) -> FileForm {
    read(&f.source())
}

/// [`read_back`] within `limits`.
fn read_within(f: &mut Form, limits: FormLimits) -> FileForm {
    let store = Store::with_limits(Limits {
        forms: limits,
        ..Limits::DEFAULT
    });
    let kind = BlobKind::file(Path::new("form.pdf"), None);
    read(&store.open(f.bytes().into(), kind).unwrap())
}

/// A placeholder for a real past f64's range, which lopdf can't write:
/// [`with_huge`] puts a 9, 318 zeros and a point in its place, which reads
/// as infinity.
fn huge() -> Object {
    Object::string_literal("~".repeat(320))
}

/// `bytes` with each [`huge`] placeholder swapped for its real, byte for
/// byte, so no offset moves. A string needs no space around it, so the
/// real brings its own.
fn with_huge(mut bytes: Vec<u8>) -> Vec<u8> {
    let placeholder = format!("({})", "~".repeat(320));
    let real = format!(" 9{}. ", "0".repeat(placeholder.len() - 4));
    while let Some(at) = bytes
        .windows(placeholder.len())
        .position(|w| w == placeholder.as_bytes())
    {
        bytes[at..at + real.len()].copy_from_slice(real.as_bytes());
    }
    bytes
}

fn field<'a>(form: &'a FileForm, name: &str) -> &'a Field {
    form.fields
        .iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("no field {name}"))
}

fn text(s: &str) -> Option<Value> {
    Some(Value::Text(s.into()))
}

fn state(s: &str) -> Option<Value> {
    Some(Value::State(s.into()))
}

fn js(script: &str) -> Object {
    Object::Dictionary(dictionary! {
        "S" => "JavaScript", "JS" => Object::string_literal(script),
    })
}

#[test]
fn every_kind_of_field() {
    let form = read_back(&mut every_kind());
    let fields: Vec<(&str, FieldKind, Option<Value>)> = form
        .fields
        .iter()
        .map(|f| (f.name.as_str(), f.kind, f.value.clone()))
        .collect();
    use FieldKind::*;
    assert_eq!(
        fields,
        [
            ("form1[0].name[0]", Text, text("Ann Lee")),
            ("form1[0].empty[0]", Text, None),
            ("form1[0].on[0]", CheckBox, state("Yes")),
            ("form1[0].off[0]", CheckBox, state("Off")),
            ("form1[0].pick[0]", Radio, state("1")),
            ("form1[0].state[0]", Combo, text("NY")),
            ("form1[0].kind[0]", List, text("1099")),
            ("form1[0].print[0]", PushButton, None),
            ("form1[0].sig[0]", Signature, None),
        ]
    );
    let ids: Vec<u64> = form.fields.iter().map(|f| f.id.0).collect();
    assert_eq!(ids, (0..9).collect::<Vec<_>>());
    let name = field(&form, "form1[0].name[0]");
    assert_eq!(name.da.as_deref(), Some("/Helv 10 Tf 0 g"));
    assert_eq!(
        (name.align, name.max_len, name.tooltip.as_ref()),
        (None, None, None)
    );
    let combo = field(&form, "form1[0].state[0]");
    let items: Vec<(&str, &str)> = combo
        .options
        .iter()
        .map(|c| (&*c.export, &*c.shown))
        .collect();
    assert_eq!(items, [("CA", "CA"), ("NY", "NY")]);
    assert!(combo.has(flags::COMBO));

    // Widgets by page, in /Annots order, each with its field.
    let on_page = |i: u32| -> Vec<(u64, usize)> {
        form.widgets[&i]
            .iter()
            .map(|w| (w.field.0, w.slot))
            .collect()
    };
    assert_eq!(on_page(0), [(0, 0), (1, 1), (2, 2), (3, 3), (4, 4), (4, 5)]);
    assert_eq!(on_page(1), [(5, 0), (6, 1), (7, 2), (8, 3)]);
    let first = &form.widgets[&0][0];
    assert_eq!(first.rect, row(0));
    assert_eq!(first.flags, annot_flags::PRINT);
    assert!(first.has_appearance && first.states.is_empty());
    let picks: Vec<(Option<&str>, &[String])> = form.widgets[&0][4..]
        .iter()
        .map(|w| (w.state.as_deref(), w.states.as_slice()))
        .collect();
    let (zero, one) = (
        ["0".to_string(), "Off".into()],
        ["1".to_string(), "Off".into()],
    );
    assert_eq!(picks, [(Some("Off"), &zero[..]), (Some("1"), &one[..])]);
    let button = &form.widgets[&1][2];
    assert_eq!(button.mk.caption.as_deref(), Some("Print"));

    let facts = &form.facts;
    assert_eq!(facts.class, FormClass::AcroForm);
    assert_eq!((facts.fields, facts.widgets), (9, 10));
    assert_eq!(facts.kinds[&Text], 2);
    assert_eq!(facts.kinds[&CheckBox], 2);
    assert_eq!((facts.signature_fields, facts.signed), (1, 0));
    assert_eq!(facts.fonts, ["Helv", "ZaDb"]);
    assert_eq!(facts.da.as_deref(), Some("/Helv 0 Tf 0 g"));
    assert!(facts.is_fillable() && !facts.has_scripts());
    assert_eq!(
        facts.to_string(),
        "AcroForm; 9 fields; no scripts; 1 signature field, 0 signed"
    );
}

#[test]
fn fields_inherit_from_their_parents() {
    let mut f = Form::new(1);
    let a = f.text("a", 0, row(0));
    let b = f.text("b", 0, row(1));
    for k in [a, b] {
        let d = f.dict(k);
        d.remove(b"FT");
        d.remove(b"DA");
    }
    // b's own keys win over its parent's.
    f.dict(b).set("FT", "Ch");
    f.dict(b).set("Q", 1);
    f.dict(b).set("TU", Object::string_literal("Choose one"));
    let parent = f.group("p", &[a, b]);
    let opt = vec![
        Object::string_literal("x"),
        vec![Object::string_literal("e"), Object::string_literal("Shown")].into(),
    ];
    let p = f.dict(parent);
    p.set("FT", "Tx");
    p.set("Ff", flags::MULTILINE as i64);
    p.set("V", Object::string_literal("shared"));
    p.set("DV", Object::string_literal("default"));
    p.set("DA", Object::string_literal("/Cour 9 Tf 0 0 1 rg"));
    p.set("Q", 2);
    p.set("MaxLen", 5000);
    p.set("Opt", opt);
    p.set("TU", Object::string_literal("not inherited"));
    let form = read_back(&mut f);
    let (a, b) = (field(&form, "p.a"), field(&form, "p.b"));
    assert_eq!(a.kind, FieldKind::Text);
    assert_eq!(b.kind, FieldKind::List);
    assert!(a.has(flags::MULTILINE) && b.has(flags::MULTILINE));
    assert_eq!(
        (a.value.clone(), a.default.clone()),
        (text("shared"), text("default"))
    );
    assert_eq!(a.da.as_deref(), Some("/Cour 9 Tf 0 0 1 rg"));
    assert_eq!((a.align, b.align), (Some(2), Some(1)));
    assert_eq!(a.max_len, Some(5000));
    assert_eq!(a.options.len(), 2);
    assert_eq!(
        (&*a.options[1].export, &*a.options[1].shown),
        ("e", "Shown")
    );
    assert_eq!(a.tooltip, None);
    assert_eq!(b.tooltip.as_deref(), Some("Choose one"));
    // Read once, and shared.
    let (Some(Value::Text(va)), Some(Value::Text(vb))) = (&a.value, &b.value) else {
        panic!()
    };
    assert!(Arc::ptr_eq(va, vb));
    assert!(Arc::ptr_eq(&a.options, &b.options));
}

#[test]
fn the_forms_own_keys() {
    let mut f = every_kind();
    let form1 = f.fields()[0];
    let kids = f.widgets_of(form1);
    let co = vec![
        Object::Reference(kids[1]),
        // A parent isn't a field that calculates.
        Object::Reference(form1),
        Object::Reference(kids[0]),
    ];
    let d = f.dict(f.acroform);
    d.set("NeedAppearances", true);
    d.set("Q", 1);
    d.set("CO", co);
    let facts = read_back(&mut f).facts;
    assert!(facts.need_appearances);
    assert_eq!(facts.align, 1);
    assert_eq!(facts.calculation_order, [FieldId(1), FieldId(0)]);
}

#[test]
fn the_class_of_a_form() {
    let class = |f: &mut Form| read_back(f).facts.class;
    assert_eq!(class(&mut every_kind()), FormClass::AcroForm);
    let mut hybrid = every_kind();
    hybrid.xfa();
    assert_eq!(class(&mut hybrid), FormClass::Hybrid);
    // Dynamic: the XFA is the form, fields or not.
    hybrid.dict(hybrid.catalog).set("NeedsRendering", true);
    let form = read_back(&mut hybrid);
    assert_eq!(
        (form.facts.class, form.fields.len()),
        (FormClass::XfaOnly, 9)
    );
    assert!(!form.facts.is_fillable());
    let mut only = every_kind();
    only.xfa_only();
    let facts = read_back(&mut only).facts;
    assert_eq!(facts.class, FormClass::XfaOnly);
    assert!(facts.xfa && facts.dynamic && facts.fields == 0);
    assert_eq!(facts.to_string(), "XFA only, which sheaf can't fill");
    // A static XFA copy with no AcroForm fields.
    let mut stat = every_kind();
    stat.xfa();
    stat.set_fields(&[]);
    assert_eq!(class(&mut stat), FormClass::XfaOnly);
    // A damaged hybrid form is hybrid, though none of its fields were read
    // before the damage.
    let mut damaged = every_kind();
    damaged.xfa();
    let mut chain = damaged.text("leaf", 0, row(9));
    for i in 0..40 {
        chain = damaged.group(&format!("n{i}"), &[chain]);
    }
    let mut fields = damaged.fields();
    fields.rotate_right(1);
    damaged.set_fields(&fields);
    let facts = read_back(&mut damaged).facts;
    assert!(facts.damaged.is_some() && facts.xfa);
    assert_eq!((facts.class, facts.fields), (FormClass::Hybrid, 0));

    let mut empty = Form::new(1);
    assert_eq!(class(&mut empty), FormClass::None);
    empty.dict(empty.catalog).remove(b"AcroForm");
    let facts = read_back(&mut empty).facts;
    assert_eq!(facts.class, FormClass::None);
    assert_eq!(facts.to_string(), "None");
    let labels = [
        FormClass::None,
        FormClass::AcroForm,
        FormClass::Hybrid,
        FormClass::XfaOnly,
    ]
    .map(FormClass::label);
    assert_eq!(labels, ["no form", "AcroForm", "hybrid", "XFA-only"]);
}

#[test]
fn scripts_by_trigger_and_at_document_level() {
    let mut f = Form::new(1);
    let a = f.text("a", 0, row(0));
    f.dict(a).set(
        "AA",
        dictionary! {
            "K" => js("AFNumber_Keystroke(2,0,0,0,\"\",true);"),
            "F" => js("AFNumber_Format(2,0,0,0,\"\",true);"),
            "Fo" => js("app.alert('hi');"),
        },
    );
    let b = f.text("b", 0, row(1));
    f.dict(b).set(
        "AA",
        dictionary! {
            "C" => js("AFSimple_Calculate(\"SUM\", new Array (\"a\"));"),
            // Blank scripts don't count.
            "V" => js("  "),
        },
    );
    let c = f.push_button("c", 0, row(2), "Print");
    f.dict(c).set("A", js("this.print();"));
    let d = f.text("d", 0, row(3));
    f.dict(d).set(
        "AA",
        dictionary! { "K" => dictionary! { "S" => "ResetForm" } },
    );
    // A script in a stream, compressed.
    let script = "event.value = util.printx(\"999-99-9999\", event.value);";
    let mut stream = Stream::new(dictionary! {}, script.as_bytes().to_vec());
    stream.compress().unwrap();
    let stream = f.pdf.add_object(stream);
    let e = f.text("e", 0, row(4));
    f.dict(e).set(
        "AA",
        dictionary! { "F" => dictionary! { "S" => "JavaScript", "JS" => stream } },
    );
    // Document scripts: Adobe's version check, and one in a kid.
    let kid = f.pdf.add_object(dictionary! {
        "Names" => vec![Object::string_literal("init"), js("var x = 1;")],
    });
    f.dict(f.catalog).set(
        "Names",
        dictionary! { "JavaScript" => dictionary! {
            "Names" => vec![Object::string_literal("!ADBE::0100_VersChkStrings"), js("")],
            "Kids" => vec![Object::Reference(kid)],
        } },
    );
    let form = read_back(&mut f);
    let facts = &form.facts;
    assert_eq!(
        facts.scripts,
        ScriptCounts {
            keystroke: 1,
            format: 2,
            validate: 0,
            calculate: 1,
            fields: 4,
        }
    );
    assert_eq!((facts.document_scripts, facts.version_checks), (1, 1));
    assert!(facts.has_scripts());
    assert_eq!(
        facts.to_string(),
        "AcroForm; 5 fields; scripts on 4 fields and 1 at document level"
    );
    assert_eq!(field(&form, "e").script(Trigger::Format), Some(script));
    assert_eq!(field(&form, "b").script(Trigger::Validate), None);
    let reset = &field(&form, "d").actions[0];
    assert_eq!(reset.0, Trigger::Keystroke);
    assert_eq!(
        (reset.1.kind.as_str(), reset.1.script.is_none()),
        ("ResetForm", true)
    );
    // Widget triggers are the widget's, even when it's the field's
    // dictionary too.
    let widget = &form.widgets[&0][0];
    assert_eq!(widget.actions.len(), 1);
    assert_eq!(widget.actions[0].0, Trigger::Focus);
    assert_eq!(form.widgets[&0][2].actions[0].0, Trigger::Activate);
}

#[test]
fn shared_document_scripts_count_once() {
    // Two nodes share one /Names array, and a third, a direct object in a
    // /Kids array, lists that array as its own kids.
    let mut f = Form::new(1);
    let names = f.pdf.add_object(vec![
        Object::string_literal("a"),
        js("var a;"),
        Object::string_literal("b"),
        js("var b;"),
    ]);
    let kids = f.pdf.new_object_id();
    f.pdf.objects.insert(
        kids,
        Object::Array(vec![
            Object::Dictionary(dictionary! { "Names" => names }),
            Object::Dictionary(dictionary! { "Names" => names }),
            Object::Dictionary(dictionary! { "Kids" => kids }),
        ]),
    );
    f.dict(f.catalog).set(
        "Names",
        dictionary! { "JavaScript" => dictionary! { "Kids" => kids } },
    );
    let facts = read_back(&mut f).facts;
    assert_eq!((facts.document_scripts, facts.version_checks), (2, 0));
}

#[test]
fn usage_rights_signatures_and_a_certification() {
    let mut f = every_kind();
    f.usage_rights();
    let facts = read_back(&mut f).facts;
    assert!(facts.usage_rights && !facts.certified);
    assert_eq!(
        (facts.sig_flags, facts.signature_fields, facts.signed),
        (2, 1, 0)
    );
    assert!(facts.to_string().contains("; Reader usage rights; "));

    let mut f = Form::new(1);
    let sig = f.signature("Signature1", 0, row(0));
    f.sign(sig);
    let form = read_back(&mut f);
    assert_eq!(form.fields[0].value, Some(Value::Signed));
    assert_eq!((form.facts.signature_fields, form.facts.signed), (1, 1));

    let mut f = Form::new(1);
    let sig = f.signature("Author", 0, row(0));
    f.certify(sig);
    let facts = read_back(&mut f).facts;
    assert!(facts.certified && !facts.usage_rights);
    assert_eq!(
        facts.to_string(),
        "AcroForm; 1 field; no scripts; certified; 1 signature field, 1 signed"
    );
}

#[test]
fn values_of_every_shape() {
    let mut f = Form::new(1);
    // One value object, many fields.
    let shared = f.pdf.add_object(Object::string_literal("same"));
    let many: Vec<ObjectId> = (0..1000)
        .map(|i| {
            let t = f.text(&format!("t{i}"), 0, [0.0, 0.0, 10.0, 10.0]);
            f.dict(t).set("V", shared);
            t
        })
        .collect();
    let list = f.choice("list", 0, row(1), &["a", "b", "c"], false);
    let items = vec![Object::string_literal("a"), Object::string_literal("c")];
    f.dict(list).set("V", items);
    f.dict(list).set("Ff", flags::MULTI_SELECT as i64);
    // A check box's state written as a string; a text field's as a name.
    let cb = f.check_box("cb", 0, row(2), false);
    f.dict(cb).set("V", Object::string_literal("Yes"));
    let named = f.text("named", 0, row(3));
    f.dict(named).set("V", Object::Name(b"Plain".to_vec()));
    // UTF-16, and text in a stream.
    let utf16 = f.text("utf16", 0, row(4));
    let mut bytes = vec![0xFE, 0xFF];
    bytes.extend("Łukasz".encode_utf16().flat_map(u16::to_be_bytes));
    f.dict(utf16)
        .set("V", Object::String(bytes, StringFormat::Hexadecimal));
    let streamed = f.text("streamed", 0, row(5));
    let s = f
        .pdf
        .add_object(Stream::new(dictionary! {}, b"long text".to_vec()));
    f.dict(streamed).set("V", s);
    let number = f.text("number", 0, row(6));
    f.dict(number).set("V", 42);
    let form = read_back(&mut f);
    assert_eq!(form.fields.len(), many.len() + 6);
    let Some(Value::Text(first)) = &form.fields[0].value else {
        panic!()
    };
    assert!(form.fields[..1000].iter().all(|f| matches!(
        &f.value, Some(Value::Text(t)) if Arc::ptr_eq(t, first)
    )));
    let Some(Value::Items(picked)) = &field(&form, "list").value else {
        panic!()
    };
    assert_eq!(picked.iter().map(|i| &**i).collect::<Vec<_>>(), ["a", "c"]);
    assert_eq!(field(&form, "cb").value, state("Yes"));
    assert_eq!(field(&form, "named").value, text("Plain"));
    assert_eq!(field(&form, "utf16").value, text("Łukasz"));
    assert_eq!(field(&form, "streamed").value, text("long text"));
    assert_eq!(field(&form, "number").value, None);
}

#[test]
fn widgets_are_finite_and_clamped() {
    let mut f = Form::new(1);
    // A link first: slots count every annotation, as PDFium does.
    let link = f
        .pdf
        .add_object(dictionary! { "Type" => "Annot", "Subtype" => "Link" });
    f.set_annots(0, &[link]);
    let a = f.text("a", 0, row(0));
    let rect = [300.into(), huge(), (-2e9).into(), 10.into()];
    let d = f.dict(a);
    d.set("Rect", rect.to_vec());
    d.set(
        "MK",
        dictionary! {
            "BG" => vec![Object::Real(2.0)],
            "BC" => vec![Object::Real(0.5), huge(), Object::Integer(-1)],
            "R" => -90,
            "CA" => Object::string_literal("4"),
        },
    );
    d.set("BS", dictionary! { "W" => Object::Real(1e9), "S" => "U" });
    d.set("F", 4 | 32);
    let b = f.text("b", 0, row(1));
    let d = f.dict(b);
    d.set("Rect", vec![1.into(), 2.into(), 3.into()]);
    d.set(
        "MK",
        dictionary! {
            "BG" => Vec::<Object>::new(),
            "BC" => vec![1.into(), 0.into()],
            "R" => 45,
        },
    );
    d.set("Border", vec![0.into(), 0.into(), 2.into()]);
    // A radio button's own /DA, apart from its field's.
    let pick = f.radio("pick", 0, &[row(2)], Some(0));
    let kid = f.widgets_of(pick)[0];
    f.dict(kid)
        .set("DA", Object::string_literal("/ZaDb 12 Tf 1 0 0 rg"));
    let form = read(&source(with_huge(f.bytes())));
    let [wa, wb, wk] = &form.widgets[&0][..] else {
        panic!("{:?}", form.widgets)
    };
    assert_eq!((wa.slot, wb.slot, wk.slot), (1, 2, 3));
    assert_eq!(wa.rect, [-MAX_COORD, 0.0, 300.0, 10.0]);
    assert_eq!(wa.flags, annot_flags::PRINT | annot_flags::NO_VIEW);
    assert_eq!(wa.mk.background, Some(Color::Gray(1.0)));
    assert_eq!(wa.mk.border_color, Some(Color::Rgb([0.5, 0.0, 0.0])));
    assert_eq!(wa.mk.rotation, 270);
    assert_eq!(wa.mk.caption.as_deref(), Some("4"));
    assert_eq!(
        wa.border,
        Some(Border {
            width: 100.0,
            style: BorderStyle::Underline
        })
    );
    assert_eq!(wa.da, None);
    assert_eq!(wb.rect, [0.0; 4]);
    assert_eq!(
        (wb.mk.background, wb.mk.border_color, wb.mk.rotation),
        (None, None, 0)
    );
    assert_eq!(
        wb.border,
        Some(Border {
            width: 2.0,
            style: BorderStyle::Solid
        })
    );
    assert_eq!(wk.da.as_deref(), Some("/ZaDb 12 Tf 1 0 0 rg"));
    assert_eq!(wk.field, field(&form, "pick").id);
}

/// What damage reads as: no fields, and a reason.
fn assert_damaged(f: &mut Form, why: &str) {
    assert_damaged_within(f, Limits::DEFAULT.forms, why);
}

/// [`assert_damaged`] within `limits`.
fn assert_damaged_within(f: &mut Form, limits: FormLimits, why: &str) {
    let form = read_within(f, limits);
    let facts = &form.facts;
    let reason = facts
        .damaged
        .as_deref()
        .unwrap_or_else(|| panic!("not damaged: {why}"));
    assert!(reason.contains(why), "{reason:?}, not {why:?}");
    assert!(form.fields.is_empty() && form.widgets.is_empty());
    assert!(!facts.is_fillable());
    assert!(facts.to_string().ends_with("so not fillable"), "{facts}");
}

#[test]
fn damaged_trees_are_not_fillable() {
    // A cycle through /Kids.
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
    assert_damaged(&mut f, "its own ancestor");

    // A field that's its own kid and parent; and a widget that's its own
    // kid.
    let mut f = Form::new(1);
    let ok = f.text("ok", 0, row(0));
    let a = f
        .pdf
        .add_object(dictionary! { "T" => Object::string_literal("a") });
    f.dict(a).set("Kids", vec![Object::Reference(a)]);
    f.dict(a).set("Parent", a);
    f.set_fields(&[ok, a]);
    assert_damaged(&mut f, "its own ancestor");
    let mut f = Form::new(1);
    let w = f.text("w", 0, row(0));
    f.dict(w).set("Kids", vec![Object::Reference(w)]);
    assert_damaged(&mut f, "its own ancestor");

    // Smaller caps, for trees past them that would take long to build.
    let small = FormLimits {
        nodes: 1000,
        entries: 2000,
        widgets: 1000,
        texts: 4 << 20,
        ..Limits::DEFAULT.forms
    };

    // A hundred fields that share one /Kids array of a hundred: each kid
    // is read once, but the lists come to ten thousand entries.
    let mut f = Form::new(1);
    let kids: Vec<Object> = (0..100)
        .map(|i| {
            let t = Object::string_literal(format!("k{i}"));
            Object::Reference(f.pdf.add_object(dictionary! { "FT" => "Tx", "T" => t }))
        })
        .collect();
    let kids = f.pdf.add_object(kids);
    let parents: Vec<ObjectId> = (0..100)
        .map(|i| {
            let t = Object::string_literal(format!("p{i}"));
            f.pdf.add_object(dictionary! { "T" => t, "Kids" => kids })
        })
        .collect();
    f.set_fields(&parents);
    assert_damaged_within(&mut f, small, "more than 2000 entries");

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
    assert_damaged(&mut f, "more than 32 deep");

    // More fields than the cap.
    let mut f = Form::new(1);
    let fields: Vec<ObjectId> = (0..=small.nodes)
        .map(|i| {
            let t = Object::string_literal(format!("f{i}"));
            f.pdf.add_object(dictionary! { "T" => t })
        })
        .collect();
    f.set_fields(&fields);
    assert_damaged_within(&mut f, small, "more than 1000 nodes");

    // More widgets on pages than the cap, under one field.
    let mut f = Form::new(1);
    let widgets: Vec<ObjectId> = (0..=small.widgets)
        .map(|_| f.pdf.add_object(dictionary! { "Subtype" => "Widget" }))
        .collect();
    let kids: Vec<Object> = widgets.iter().map(|&w| Object::Reference(w)).collect();
    let many = f.pdf.add_object(dictionary! {
        "FT" => "Btn", "T" => Object::string_literal("many"), "Kids" => kids,
    });
    f.set_fields(&[many]);
    f.set_annots(0, &widgets);
    assert_damaged_within(&mut f, small, "more than 1000 widgets");

    // A full name over 64 KiB.
    let mut f = Form::new(1);
    let kid = f.text("k", 0, row(0));
    f.group(&"n".repeat(70_000), &[kid]);
    assert_damaged(&mut f, "longer than 64 KiB");

    // Full names over 64 MiB in all: a 65,000-byte name in each of 1,100
    // kids' names.
    let mut f = Form::new(1);
    let kids: Vec<ObjectId> = (0..1100)
        .map(|i| f.text(&format!("k{i}"), 0, row(0)))
        .collect();
    f.group(&"n".repeat(65_000), &kids);
    assert_damaged(&mut f, "full names come to more than 64 MiB");

    let caps = Limits::DEFAULT.forms;
    // An /Opt with more than 10,000 items.
    let mut f = Form::new(1);
    let items: Vec<String> = (0..=caps.items).map(|i| i.to_string()).collect();
    let items: Vec<&str> = items.iter().map(String::as_str).collect();
    f.choice("big", 0, row(0), &items, true);
    assert_damaged(&mut f, "an /Opt has more than 10000 items");
    // And a list box's value.
    let mut f = Form::new(1);
    let list = f.choice("list", 0, row(0), &["a"], false);
    let items = vec![Object::string_literal("a"); caps.items + 1];
    f.dict(list).set("V", items);
    assert_damaged(&mut f, "a value has more than 10000 items");

    // A value over 1 MiB, as a string.
    let mut f = Form::new(1);
    let t = f.text("t", 0, row(0));
    f.dict(t)
        .set("V", Object::string_literal(vec![b'x'; caps.text + 1]));
    assert_damaged(&mut f, "longer than 1 MiB");

    // More text in all than the cap: 5 values of 1 MiB.
    let mut f = Form::new(1);
    for i in 0..5 {
        let t = f.text(&format!("t{i}"), 0, row(0));
        f.dict(t)
            .set("V", Object::string_literal(vec![b'x'; caps.text]));
    }
    assert_damaged_within(&mut f, small, "its text comes to more than 4 MiB");

    // A value that inflates past 1 MiB.
    let mut f = Form::new(1);
    let mut zeros = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    zeros.write_all(&vec![b'0'; caps.text + 1]).unwrap();
    let bomb = f.pdf.add_object(Stream::new(
        dictionary! { "Filter" => "FlateDecode" },
        zeros.finish().unwrap(),
    ));
    let t = f.text("t", 0, row(0));
    f.dict(t).set("V", bomb);
    assert_damaged(&mut f, "longer than 1 MiB");
}

#[test]
fn fields_listed_twice_are_read_once() {
    // A field in /Fields twice; a kid in /Fields before its parent; a
    // widget two fields list; and one listed twice by its field.
    let mut f = Form::new(1);
    let twice = f.text("twice", 0, row(0));
    let kid = f.text("kid", 0, row(1));
    let parent = f.group("parent", &[kid]);
    let x = f.radio("x", 0, &[row(2)], Some(0));
    let y = f.radio("y", 0, &[row(3)], Some(0));
    let shared = f.widgets_of(x)[0];
    f.dict(y).set(
        "Kids",
        vec![Object::Reference(shared), Object::Reference(shared)],
    );
    f.set_fields(&[twice, kid, parent, x, y, twice]);
    let form = read_back(&mut f);
    assert_eq!(form.facts.damaged, None);
    let names: Vec<&str> = form.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["twice", "parent.kid", "x", "y"]);
    let owners: Vec<(u64, usize)> = form.widgets[&0]
        .iter()
        .map(|w| (w.field.0, w.slot))
        .collect();
    // y's own widget isn't its kid any more: no field has it.
    assert_eq!(owners, [(0, 0), (1, 1), (2, 2)]);
}

#[test]
fn a_page_listed_twice_has_its_widgets_at_both_indexes() {
    // Widgets are found through the page table, which numbers pages as
    // PDFium does: the page object twice is two pages.
    let mut f = Form::new(1);
    let a = f.text("a", 0, row(0));
    let page = Object::Reference(f.pages[0]);
    let root = f
        .dict(f.catalog)
        .get(b"Pages")
        .unwrap()
        .as_reference()
        .unwrap();
    f.dict(root).set("Kids", vec![page.clone(), page]);
    f.dict(root).set("Count", 2);
    let form = read_back(&mut f);
    let on: Vec<(u32, ObjectId)> = form
        .widgets
        .iter()
        .flat_map(|(&i, ws)| ws.iter().map(move |w| (i, w.object)))
        .collect();
    assert_eq!(on, [(0, a), (1, a)]);
    assert_eq!(form.facts.widgets, 2);
}

#[test]
fn a_parent_loop_alone_isnt_damage() {
    // /Parent isn't followed, so a field that's its own parent, but not its
    // own kid, reads like any other.
    let mut f = Form::new(1);
    let a = f.text("a", 0, row(0));
    f.dict(a).set("Parent", a);
    let form = read_back(&mut f);
    assert_eq!(form.facts.damaged, None);
    let names: Vec<&str> = form.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["a"]);
    assert_eq!(form.widgets[&0].len(), 1);
}

#[test]
fn layout_uses_at_most_a_thousand_of_max_len() {
    // Kept as the file has it; capped where it's used.
    let mut f = Form::new(1);
    let t = f.text("t", 0, row(0));
    f.dict(t).set("MaxLen", 1_000_000);
    let u = f.text("u", 0, row(1));
    f.dict(u).set("MaxLen", -3);
    let form = read_back(&mut f);
    assert_eq!(form.fields[0].max_len, Some(1_000_000));
    assert_eq!(form.fields[0].layout_len(), Some(MAX_LAYOUT_LEN));
    assert_eq!(form.fields[1].max_len, Some(0));
}

#[test]
fn hybrid_reference_files_read_their_newest_revision() {
    let fields = |form: &FileForm| -> Vec<(String, FieldKind, Option<Value>)> {
        form.fields
            .iter()
            .map(|f| (f.name.clone(), f.kind, f.value.clone()))
            .collect()
    };
    // The form is in an object stream only /XRefStm lists, and the
    // trailer has no /Prev.
    let first = hybrid_reference();
    let src = source(first.clone());
    let form = read(&src);
    assert_eq!(
        fields(&form),
        [
            ("name".into(), FieldKind::Text, text("Ann Lee")),
            ("city".into(), FieldKind::Text, text("Hull")),
        ]
    );
    assert!(src.xref().hybrid && src.xref().sections == 1);

    // The update's catalog is only in its /XRefStm, and the first table
    // lists the old one. The field it changes is in both sections' object
    // streams, and `city` only in the first section's.
    let src = source(hybrid_update(&first));
    let acroform = src.pdf().catalog().unwrap().get(b"AcroForm").unwrap();
    assert_eq!(acroform.as_reference().unwrap(), (11, 0));
    let form = read(&src);
    assert_eq!(
        fields(&form),
        [
            ("name".into(), FieldKind::Text, text("Bo Ray")),
            ("city".into(), FieldKind::Text, text("Hull")),
            ("sig".into(), FieldKind::Signature, None),
        ]
    );
    assert_eq!(form.widgets[&0].len(), 3);
    assert_eq!(form.facts.signature_fields, 1);
}

/// A crafted file can point thousands of sections' /XRefStm at one large
/// cross-reference stream, by offsets that differ only in the white-space,
/// comments and zeros before its header. lopdf reads it once
/// (vendor/lopdf/SHEAF.md): read for each section, this file takes
/// seconds to load, not milliseconds. Its 1,003 sections are within the
/// lazy reader's cap, and its source opens and its form reads as quickly.
#[test]
fn a_cross_reference_stream_many_sections_share_is_read_once() {
    let mut bytes = hybrid_reference();
    let mut prev = source(bytes.clone()).xref().start;
    // A million free entries, 7 MB unpacked.
    let mut entries = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    entries.write_all(&vec![0; 7_000_000]).unwrap();
    let entries = entries.finish().unwrap();
    let padding = [" ".repeat(300), "%\n".repeat(300), "0".repeat(100)].concat();
    let at = bytes.len();
    bytes.extend_from_slice(padding.as_bytes());
    let header = bytes.len();
    let dict = "/Type /XRef /Size 1000000 /W [1 4 2] /Filter /FlateDecode";
    let head = format!("99 0 obj\n<< {dict} /Length {} >>\nstream\n", entries.len());
    bytes.extend_from_slice(head.as_bytes());
    bytes.extend_from_slice(&entries);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    // Every offset in the padding reaches the stream, and so do both
    // digits of its number.
    for stream in at..header + 2 {
        let section = bytes.len();
        let trailer = format!("<< /Size 10 /Root 1 0 R /XRefStm {stream} /Prev {prev} >>");
        bytes.extend_from_slice(format!("xref\n0 0\ntrailer\n{trailer}\n").as_bytes());
        prev = section;
    }
    bytes.extend_from_slice(format!("startxref\n{prev}\n%%EOF\n").as_bytes());

    // Eagerly, as the old writers load it.
    let start = Instant::now();
    let pdf = crate::file::load_pdf_mem(&bytes, None).unwrap();
    let took = start.elapsed();
    let acroform = pdf.catalog().unwrap().get(b"AcroForm").unwrap();
    let acroform = resolve(&pdf, acroform).unwrap().as_dict().unwrap();
    assert_eq!(fields_of(&pdf, acroform).unwrap().len(), 2);
    assert!(took.as_secs_f32() < 1.0, "{took:?}");

    let start = Instant::now();
    let src = source(bytes);
    let form = read(&src);
    let took = start.elapsed();
    assert_eq!(form.fields.len(), 2);
    assert_eq!(src.xref().sections, 1003);
    assert!(src.xref().sections <= src.limits().reader.max_sections);
    assert!(took.as_secs_f32() < 1.0, "{took:?}");
}

/// Every corpus file with a hybrid-reference section (a trailer with
/// `/XRefStm`) has the manifest's fields by type, as pypdf read them.
/// lopdf 0.45 read an earlier revision of 9 SafeDocs files, without the
/// signature field their update added. As in `forms_corpus.rs`, since
/// pypdf counts a field listed twice twice, every curated form must match
/// and 99% of the SafeDocs sample; damaged trees, which sheaf doesn't
/// read, are left out. Skips without the corpus.
#[test]
fn corpus_hybrid_reference_files_read_their_newest_revision() {
    let Some(corpus) = Corpus::or_skip() else {
        return;
    };
    let sample = |path: &str| path.starts_with("forms/safedocs/");
    let (mut files, mut sampled) = (0, 0);
    let mut differ = Vec::new();
    // pypdf couldn't read the `unparsed` ones: their rows say nothing.
    for entry in corpus.manifest().unwrap() {
        if entry.kind == "unparsed" {
            continue;
        }
        let bytes = std::fs::read(corpus.path(&entry)).unwrap();
        if !bytes.windows(8).any(|w| w == b"/XRefStm") {
            continue;
        }
        // Some damaged files don't open at all (slice 1's note).
        let Ok(info) = SourceInfo::read(&Store::new(), &corpus.path(&entry), None) else {
            continue;
        };
        let facts = read(&info.src).facts;
        if facts.damaged.is_some() {
            continue;
        }
        files += 1;
        if sample(&entry.path) {
            sampled += 1;
        }
        let kinds: BTreeMap<String, u32> = facts
            .kinds
            .iter()
            .filter(|(k, _)| **k != FieldKind::Unknown)
            .map(|(k, n)| (k.short().into(), *n as u32))
            .collect();
        if (facts.fields as u32, &kinds) != (entry.fields, &entry.field_types) {
            differ.push((
                entry.path.clone(),
                format!("{kinds:?}, not {:?}", entry.field_types),
            ));
        }
    }
    eprintln!("{files} files with /XRefStm read, {} differ", differ.len());
    let curated: Vec<_> = differ.iter().filter(|(p, _)| !sample(p)).collect();
    assert!(curated.is_empty(), "{curated:#?}");
    assert!(differ.len() * 100 <= sampled, "{differ:#?}");
}

/// A node of a random field tree: its keys, with references by index
/// into the tree (some of them to objects that aren't there).
#[derive(Debug, Clone)]
struct Node {
    name: Option<String>,
    ft: Option<&'static str>,
    ff: Option<i64>,
    value: Option<Val>,
    kids: Option<Vec<usize>>,
    parent: Option<usize>,
    widget: bool,
    rect: Option<Vec<Num>>,
    mk: Option<Vec<Num>>,
    opt: Option<usize>,
    on_page: bool,
}

/// A number in a rect or a color: one lopdf writes as it is, or one past
/// f64's range, which reads as infinity (see [`huge`]).
#[derive(Debug, Clone, Copy)]
enum Num {
    Real(f32),
    Huge,
}

impl From<Num> for Object {
    fn from(n: Num) -> Object {
        match n {
            Num::Real(v) => v.into(),
            Num::Huge => huge(),
        }
    }
}

#[derive(Debug, Clone)]
enum Val {
    Text(String),
    Name(String),
    Items(Vec<String>),
    Number(i64),
    Ref(usize),
}

fn node() -> impl Strategy<Value = Node> {
    let value = prop_oneof![
        "[a-z]{0,8}".prop_map(Val::Text),
        "[A-Za-z]{1,4}".prop_map(Val::Name),
        prop::collection::vec("[a-z]{0,3}", 0..4).prop_map(Val::Items),
        any::<i64>().prop_map(Val::Number),
        (0usize..24).prop_map(Val::Ref),
    ];
    // lopdf writes a real past i64's range as digits it can't read back.
    let float = prop_oneof![
        (-1e18f32..1e18).prop_map(Num::Real),
        Just(Num::Huge),
        (-1e7f32..1e7).prop_map(Num::Real),
    ];
    (
        (
            prop::option::of("[a-c]{1,3}"),
            prop::option::of(prop::sample::select(vec!["Tx", "Btn", "Ch", "Sig", "Zz"])),
            prop::option::of(any::<i64>()),
            prop::option::of(value),
            prop::option::of(prop::collection::vec(0usize..24, 0..5)),
            prop::option::of(0usize..24),
        ),
        (
            any::<bool>(),
            prop::option::of(prop::collection::vec(float.clone(), 0..6)),
            prop::option::of(prop::collection::vec(float, 0..6)),
            prop::option::of(0usize..5),
            any::<bool>(),
        ),
    )
        .prop_map(
            |((name, ft, ff, value, kids, parent), (widget, rect, mk, opt, on_page))| Node {
                name,
                ft,
                ff,
                value,
                kids,
                parent,
                widget,
                rect,
                mk,
                opt,
                on_page,
            },
        )
}

/// The tree as a file: node `i` is object `i + 1`; index 20 and up point
/// at objects that aren't there.
fn tree_file(nodes: &[Node], roots: &[usize]) -> Vec<u8> {
    let mut f = Form::new(1);
    let base = f.pdf.max_id + 1;
    let id = |i: usize| -> ObjectId { (base + i as u32, 0) };
    let mut on_page = Vec::new();
    for (i, n) in nodes.iter().enumerate() {
        let mut d = lopdf::Dictionary::new();
        if let Some(name) = &n.name {
            d.set("T", Object::string_literal(name.as_str()));
        }
        if let Some(ft) = n.ft {
            d.set("FT", ft);
        }
        if let Some(ff) = n.ff {
            d.set("Ff", ff);
        }
        let v = match &n.value {
            Some(Val::Text(t)) => Some(Object::string_literal(t.as_str())),
            Some(Val::Name(t)) => Some(Object::Name(t.clone().into_bytes())),
            Some(Val::Items(items)) => Some(
                items
                    .iter()
                    .map(|t| Object::string_literal(t.as_str()))
                    .collect::<Vec<_>>()
                    .into(),
            ),
            Some(Val::Number(v)) => Some((*v).into()),
            Some(Val::Ref(r)) => Some(Object::Reference(id(*r))),
            None => None,
        };
        if let Some(v) = v {
            d.set("V", v);
        }
        if let Some(kids) = &n.kids {
            let kids: Vec<Object> = kids.iter().map(|&k| Object::Reference(id(k))).collect();
            d.set("Kids", kids);
        }
        if let Some(p) = n.parent {
            d.set("Parent", id(p));
        }
        if n.widget {
            d.set("Subtype", "Widget");
        }
        if let Some(rect) = &n.rect {
            d.set(
                "Rect",
                rect.iter().map(|&v| Object::from(v)).collect::<Vec<_>>(),
            );
        }
        if let Some(mk) = &n.mk {
            let c: Vec<Object> = mk.iter().map(|&v| Object::from(v)).collect();
            d.set("MK", dictionary! { "BG" => c.clone(), "BC" => c });
        }
        if let Some(n) = n.opt {
            let opt: Vec<Object> = (0..n).map(|i| Object::Integer(i as i64)).collect();
            d.set("Opt", opt);
        }
        f.pdf.objects.insert(id(i), Object::Dictionary(d));
        if n.on_page {
            on_page.push(id(i));
        }
    }
    f.pdf.max_id = base + 24;
    f.set_annots(0, &on_page);
    f.set_fields(&roots.iter().map(|&r| id(r)).collect::<Vec<_>>());
    with_huge(f.bytes())
}

proptest! {
    /// Any tree reads without a panic, and what's read hangs together:
    /// fields are distinct objects, each widget belongs to one of them,
    /// and every number is finite.
    #[test]
    fn any_field_tree_reads(
        nodes in prop::collection::vec(node(), 1..20),
        roots in prop::collection::vec(0usize..24, 0..6),
    ) {
        let form = read(&source(tree_file(&nodes, &roots)));
        let facts = &form.facts;
        if facts.damaged.is_some() {
            prop_assert!(form.fields.is_empty() && form.widgets.is_empty());
        } else {
            prop_assert_eq!(facts.fields, form.fields.len());
        }
        let objects: HashSet<ObjectId> = form.fields.iter().map(|f| f.object).collect();
        prop_assert_eq!(objects.len(), form.fields.len());
        let ids: HashSet<FieldId> = form.fields.iter().map(|f| f.id).collect();
        let mut owner = HashMap::new();
        for w in form.widgets.values().flatten() {
            prop_assert!(ids.contains(&w.field));
            prop_assert_eq!(*owner.entry(w.object).or_insert(w.field), w.field);
            prop_assert!(w.rect.iter().all(|v| v.is_finite() && v.abs() <= MAX_COORD));
            prop_assert!(w.rect[0] <= w.rect[2] && w.rect[1] <= w.rect[3]);
            for c in [w.mk.background, w.mk.border_color].into_iter().flatten() {
                let parts = match c {
                    Color::Gray(g) => vec![g],
                    Color::Rgb(p) => p.to_vec(),
                    Color::Cmyk(p) => p.to_vec(),
                };
                prop_assert!(parts.iter().all(|v| (0.0..=1.0).contains(v)));
            }
        }
    }
}

/// Every curated hybrid form's fields find their XFA fields by name, most
/// have its tooltips, and a blank form's XFA copy holds nothing the
/// AcroForm lacks (its check boxes' `0`s aren't values).
#[test]
fn corpus_hybrid_forms_bind_their_xfa() {
    let Some(corpus) = Corpus::or_skip() else {
        return;
    };
    let mut forms = 0;
    for entry in corpus.manifest().unwrap() {
        if entry.kind != "hybrid" || entry.path.starts_with("forms/safedocs/") {
            continue;
        }
        let src = SourceInfo::read(&Store::new(), &corpus.path(&entry), None)
            .unwrap()
            .src;
        let form = read(&src);
        let pdf = src.pdf();
        let acroform = pdf
            .catalog()
            .unwrap()
            .get(b"AcroForm")
            .ok()
            .and_then(|f| resolve(pdf, f))
            .and_then(|f| f.as_dict().ok())
            .unwrap();
        let xfa: HashSet<String> = crate::forms::xfa::read(pdf, acroform, &src.limits().forms)
            .into_iter()
            .map(|f| f.name)
            .collect();
        let unbound: Vec<&str> = form
            .fields
            .iter()
            .map(|f| f.name.as_str())
            .filter(|n| !xfa.contains(*n))
            .collect();
        assert!(unbound.is_empty(), "{}: {unbound:?}", entry.path);
        assert_eq!(form.facts.xfa_values, 0, "{}", entry.path);
        let tooltips = form.fields.iter().filter(|f| f.tooltip.is_some()).count();
        assert!(
            tooltips * 2 > form.fields.len(),
            "{}: {tooltips} tooltips",
            entry.path
        );
        forms += 1;
    }
    println!("{forms} hybrid forms");
}
