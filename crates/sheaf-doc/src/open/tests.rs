//! The open: no fact after the first edit; the headless open; a file
//! whose pages' content is never read; trees that list themselves; and
//! over Greg's corpus, the old loaders' eager reading of each file. The
//! readers' own tests read their fixtures as sources with [`lazily`].

use lopdf::{Dictionary, Document as Pdf, Object, ObjectId, Stream, dictionary};
use sheaf_core::Rotation;

use super::*;
use crate::annot::{AnnotId, WriteCx};
use crate::forms::corpus::{Corpus, Entry, each_entry};
use crate::forms::fixture::{Form, every_fact, note, outline_tree};
use crate::labels::Style;
use crate::model::{DocState, Edit, NotReady, PageId};
use crate::store::{BlobKind, Src};

/// `pdf` written and read as a source, in a store of its own: what a
/// reader reads as a file opens, beside the loaded document. A fixture
/// with no pages is given a Letter page first, since a source has pages.
pub(crate) fn lazily(pdf: &mut Pdf) -> Src {
    if pdf.get_pages().is_empty() {
        let pages = pdf.new_object_id();
        let page = pdf.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        });
        pdf.objects.insert(
            pages,
            dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 }.into(),
        );
        pdf.catalog_mut().unwrap().set("Pages", pages);
    }
    let mut bytes = Vec::new();
    pdf.save_to(&mut bytes).unwrap();
    crate::forms::fixture::source(bytes)
}

/// `st` holds what [`every_fact`] says, on the pages it says it of.
pub(crate) fn assert_every_fact(st: &DocState) {
    let ids: Vec<PageId> = st.pages.iter().map(|p| p.id).collect();
    let marks: Vec<(PageId, Vec<Annot>)> = st
        .annots
        .iter()
        .map(|(page, list)| (*page, list.iter().cloned().collect()))
        .collect();
    assert_eq!(marks, [(ids[1], vec![note()])]);
    assert_eq!(st.fields.len(), 1);
    let widgets: Vec<(PageId, usize)> = st.widgets.iter().map(|(p, w)| (*p, w.len())).collect();
    assert_eq!(widgets, [(ids[0], 1)]);
    let labels = st.labels.as_ref().expect("labels");
    assert_eq!(labels.len(), 1);
    assert_eq!(
        (labels[0].start, labels[0].style),
        (ids[0], Style::LowerRoman)
    );
    let hf = st.header_footer.as_ref().expect("a header/footer");
    assert_eq!(
        (hf.texts[0].as_str(), &hf.pages),
        ("Draft", &Some(vec![ids[2]]))
    );
    let outline: Vec<(u16, &str, Option<PageId>, bool)> = st
        .outline_flat()
        .iter()
        .map(|(depth, n)| (*depth, n.title.as_str(), n.target, n.open))
        .collect();
    assert_eq!(
        outline,
        [(0, "A", Some(ids[0]), false), (1, "A1", Some(ids[2]), true)]
    );
}

/// No fact comes after the first edit: while the file opens an edit is
/// refused, and once `ready` takes the facts the first edit is made from
/// the version that holds them all.
#[test]
fn no_fact_comes_after_the_first_edit() {
    let store = Store::new();
    let kind = BlobKind::file(Path::new("facts.pdf"), None);
    let primary = SourceInfo {
        src: store.open(every_fact().bytes().into(), kind).unwrap(),
        name: "facts.pdf".into(),
        derived: None,
    };
    let src = primary.src.clone();
    let mut doc = start(store, primary, crate::history::default_policy());
    let page = doc.page(0).unwrap().id;
    let rotate = || Edit::RotatePages {
        ids: vec![page],
        by: Rotation::R90,
    };
    let refused = doc.apply(rotate(), "Rotate").unwrap_err();
    assert!(refused.is::<NotReady>(), "{refused:#}");
    assert!(!doc.can_undo());
    doc.ready(finish(&src, &NoProgress)).unwrap();
    let opened = doc.current_version();
    doc.apply(rotate(), "Rotate").unwrap();
    doc.undo().unwrap();
    assert_eq!(doc.current_version(), opened);
    assert!(!doc.can_undo());
    assert_every_fact(doc.state());
}

/// [`file`] opens a file with every fact, ready and with nothing to
/// save, and [`finish`] counts the pages of its first pass. (The app's
/// open is checked against it in sheaf-app's
/// `the_app_opens_as_the_command_line_does`.)
#[test]
fn a_file_opens_headless_with_every_fact() {
    let mut f = every_fact();
    let path = std::env::temp_dir().join(format!("sheaf-every-fact-{}.pdf", std::process::id()));
    std::fs::write(&path, f.bytes()).unwrap();
    let opened = file(&path, None);
    std::fs::remove_file(&path).ok();
    let doc = opened.unwrap().doc;
    assert!(doc.is_ready() && !doc.is_dirty() && !doc.can_undo());
    assert_every_fact(doc.state());
    let counted = in_progress::Tracker::new();
    finish(&f.source(), &counted);
    let s = counted.snapshot();
    assert_eq!((s.done, s.total, s.unit), (3, Some(3), crate::PAGES));
}

/// The open never reads a page's content stream, let alone lexes it, and
/// pages whose content is no stream that decodes open with every fact.
#[test]
fn finish_never_lexes_content() {
    let mut f = every_fact();
    let mut contents = Vec::new();
    for page in f.pages.clone() {
        let garbage = f.pdf.add_object(Stream::new(
            dictionary! { "Filter" => "FlateDecode" },
            b"\x00\xff not deflate: BT ( ] >> q Tj".to_vec(),
        ));
        f.dict(page)
            .set("Contents", vec![garbage.into(), Object::Reference(garbage)]);
        contents.push(garbage);
    }
    let src = f.source();
    let facts = finish(&src, &NoProgress);
    let read: Vec<&ObjectId> = contents
        .iter()
        .filter(|&&id| src.pdf().is_read(id))
        .collect();
    assert!(read.is_empty(), "content read: {read:?}");
    assert_eq!(facts.marks[&1], vec![note()]);
    let roman = LabelRange {
        start: 0,
        style: Style::LowerRoman,
        prefix: String::new(),
        first: 1,
    };
    assert_eq!(facts.labels, Some(vec![roman]));
    assert_eq!(facts.form.fields.len(), 1);
    assert_eq!(facts.form.widgets[&0].len(), 1);
    let scope = facts.header_footer.and_then(|(_, scope)| scope);
    assert_eq!(scope, Some(vec![2]));
    let outline: Vec<(u16, Option<u32>)> =
        facts.outline.iter().map(|b| (b.depth, b.page)).collect();
    assert_eq!(outline, [(0, Some(0)), (1, Some(2))]);
}

/// Whether two values are the same, their numbers within `tolerance`.
fn near(a: &serde_json::Value, b: &serde_json::Value, tolerance: f64) -> bool {
    use serde_json::Value::*;
    match (a, b) {
        (Number(x), Number(y)) => (x.as_f64().unwrap() - y.as_f64().unwrap()).abs() <= tolerance,
        (Array(x), Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| near(a, b, tolerance))
        }
        (Object(x), Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| near(v, w, tolerance)))
        }
        _ => a == b,
    }
}

/// The boxes of the page the marks' tests write on: its media box off the
/// origin, and its crop box inset from it.
const MEDIA: [f32; 4] = [-37.0, 52.0, 575.0, 844.0];
const CROP: [f32; 4] = [-17.0, 62.0, 555.0, 824.0];

/// Every kind of mark, reading upright turned `turn`.
fn every_mark_turned(turn: Rotation) -> Vec<Annot> {
    crate::forms::fixture::every_mark_kind()
        .into_iter()
        .map(|a| Annot { turn, ..a })
        .collect()
}

/// A one-page form, its page turned `degrees` with [`MEDIA`] and
/// [`CROP`], and `marks` on it as a save writes them.
fn marked(degrees: i32, marks: &[Annot]) -> Form {
    let num = |v: f32| Object::from(v);
    let mut f = Form::new(1);
    let page = f.pages[0];
    let mut dict = f.dict(page).clone();
    dict.set("MediaBox", MEDIA.map(num).to_vec());
    dict.set("CropBox", CROP.map(num).to_vec());
    dict.set("Rotate", i64::from(degrees));
    let mut cx = WriteCx::new(crate::forms::fixture::mark_assets());
    crate::save::write_page_annots(&mut f.pdf, &mut dict, marks.iter(), None, &[], &mut cx);
    *f.dict(page) = dict;
    f
}

/// `read` is `want`, by id: each mark's numbers within `tolerance` and its
/// turn kept.
fn assert_marks(read: &[Annot], want: &[Annot], tolerance: f64, case: &str) {
    let mut read = read.to_vec();
    read.sort_by_key(|a| a.id);
    assert_eq!(read.len(), want.len(), "{case}");
    for (want, got) in want.iter().zip(&read) {
        let (w, g) = (
            serde_json::to_value(want).unwrap(),
            serde_json::to_value(got).unwrap(),
        );
        assert!(near(&w, &g, tolerance), "{case}\nwant {w}\n got {g}");
        assert_eq!(got.turn, want.turn, "{case}: {:?}", want.kind);
    }
}

/// `bytes` opened as a file opens, ready, with what the view is given.
fn opened(bytes: Vec<u8>) -> Opened {
    let store = Store::new();
    let kind = BlobKind::file(Path::new("marks.pdf"), None);
    let primary = SourceInfo {
        src: store.open(bytes.into(), kind).unwrap(),
        name: "marks.pdf".into(),
        derived: None,
    };
    let src = primary.src.clone();
    let mut doc = start(store, primary, crate::history::default_policy());
    let view = doc.ready(finish(&src, &NoProgress)).unwrap();
    Opened { doc, view }
}

/// Every kind of mark, written by the writer a save uses on a page turned
/// each quarter, whose media box is off the origin and whose crop box is
/// inset from it, opens as the mark it was: its PageSpace geometry and its
/// turn kept, and none of it read as another tool's. Its `/Rect` is its
/// PageSpace bounds, padded for its stroke.
#[test]
fn every_mark_kind_round_trips_on_every_rotate() {
    use crate::annot::AnnotKind;
    for degrees in [0, 90, 180, 270] {
        let turn = Rotation::from_degrees(degrees);
        let marks = every_mark_turned(turn);
        let mut f = marked(degrees, &marks);
        let rects: HashMap<String, Vec<f32>> = f
            .annots(0)
            .into_iter()
            .map(|id| {
                let d = f.pdf.get_dictionary(id).unwrap();
                let r = d.get(b"Rect").unwrap().as_array().unwrap();
                let r = r.iter().map(|v| v.as_float().unwrap() as f32).collect();
                (crate::file::thread_name(d), r)
            })
            .collect();
        for a in &marks {
            if matches!(
                a.kind,
                AnnotKind::Replace { .. } | AnnotKind::Reply { .. } | AnnotKind::Status { .. }
            ) {
                continue;
            }
            let (b, pad) = (a.page_bounds(), a.stroke.max(1.0) + 1.0);
            let want = [b.x - pad, b.y - pad, b.right() + pad, b.bottom() + pad];
            let r = &rects[&a.name()];
            let near = r.iter().zip(want).all(|(r, w)| (r - w).abs() <= 1.0 / 64.0);
            assert!(near, "{degrees}: {:?}: /Rect {r:?}, want {want:?}", a.kind);
        }
        let src = f.source();
        assert_eq!(src.geom(0).rotate, turn);
        let facts = finish(&src, &NoProgress);
        assert!(facts.foreign.is_empty(), "{degrees}: {:?}", facts.foreign);
        // Replies and statuses are written after what they're in the
        // thread of.
        assert_marks(&facts.marks[&0], &marks, 1.0 / 64.0, &format!("{degrees}°"));
    }
}

/// A mark an older sheaf wrote reads as the same mark written now. Its
/// `/SheafAnnot` is version 1's: the JSON in the page's displayed space,
/// with no version or digest, and a tick named `/SheafTick<id>`. Its
/// geometry comes from the standard keys, within 0.01 pt, and it reads
/// upright as the page shows. So does one written on a page sheaf had
/// turned, whose JSON is in the displayed space of the `/Rotate` the page
/// had before the save.
#[test]
fn a_v1_mark_reads_as_its_v2_self() {
    use crate::annot::AnnotKind;
    let rect = |b: [f32; 4]| crate::geom::Rect::new(b[0], b[1], b[2], b[3]);
    // The page's /Rotate, and the one the JSON's frame has.
    for (degrees, frame) in [(0, 0), (90, 90), (180, 180), (270, 270), (90, 0), (0, 270)] {
        let marks = every_mark_turned(Rotation::from_degrees(degrees));
        let mut f = marked(degrees, &marks);
        let shown = crate::geom::Geom::read(Some(rect(MEDIA)), Some(rect(CROP)), frame).to_view();
        for id in f.annots(0) {
            let d = f.dict(id);
            let Ok(json) = d.get(b"SheafAnnot").and_then(Object::as_str) else {
                continue;
            };
            let a = crate::annot::from_json(crate::file::text_string(json).as_bytes())
                .unwrap()
                .mark;
            let old = Annot {
                turn: Rotation::R0,
                ..a.mapped(&shown)
            };
            let json = serde_json::to_string(&old).unwrap();
            d.set("SheafAnnot", crate::save::text_string(&json));
            if matches!(a.kind, AnnotKind::Stamp { .. }) {
                let tick = format!("SheafTick{}", a.id.0);
                d.set("Name", Object::Name(tick.into_bytes()));
            }
        }
        let facts = finish(&f.source(), &NoProgress);
        let case = format!("{degrees}°, its JSON in {frame}°'s frame");
        assert!(facts.foreign.is_empty(), "{case}: {:?}", facts.foreign);
        assert_marks(&facts.marks[&0], &marks, 0.01, &case);
    }
}

/// A mark another tool has edited since sheaf wrote it is that tool's
/// comment: Acrobat's edit of its text (a new `/Contents` and `/M`) opens
/// among the other tools' comments, as a mark that can be made editable,
/// and not among the marks, whether its `/SheafAnnot` is version 2, whose
/// digest no longer matches, or version 1, whose `modified` isn't `/M`. A
/// rewrite and an incremental save of an edit of the page's other marks
/// keep it as that tool left it: not put back as sheaf wrote it, and not
/// dropped.
#[test]
fn a_stale_mark_is_another_tools_comment() {
    use crate::annot::AnnotKind;
    for v1 in [false, true] {
        let marks = every_mark_turned(Rotation::R90)[1..4].to_vec();
        let mut f = marked(90, &marks);
        let square = f.annots(0)[2];
        let case = if v1 { "version 1" } else { "version 2" };
        if v1 {
            // Its JSON as an older sheaf wrote it: no version or digest.
            let json = f.dict(square).get(b"SheafAnnot").and_then(Object::as_str);
            let json = crate::file::text_string(json.unwrap());
            let mut payload: serde_json::Value = serde_json::from_str(&json).unwrap();
            let payload = payload.as_object_mut().unwrap();
            payload.remove("v");
            payload.remove("std");
            let json = serde_json::to_string(payload).unwrap();
            f.dict(square)
                .set("SheafAnnot", crate::save::text_string(&json));
        }
        let edited = crate::save::text_string("Edited in Acrobat");
        f.dict(square).set("Contents", edited.clone());
        f.dict(square)
            .set("M", Object::string_literal("D:20261017090000Z"));
        let bytes = f.bytes();
        let as_left = Pdf::load_mem(&bytes)
            .unwrap()
            .get_dictionary(square)
            .unwrap()
            .clone();
        let Opened { mut doc, view } = opened(bytes);
        let page = doc.page(0).unwrap().id;
        let ids: Vec<AnnotId> = doc
            .state()
            .annots_on(page)
            .unwrap()
            .iter()
            .map(|a| a.id)
            .collect();
        assert_eq!(ids, [AnnotId(2), AnnotId(3)], "{case}");
        let [theirs] = view.foreign.as_slice() else {
            panic!("{case}: {:?}", view.foreign);
        };
        assert_eq!(
            (theirs.name.as_str(), theirs.contents.as_str()),
            ("sheaf:4", "Edited in Acrobat"),
            "{case}"
        );
        assert!(
            matches!(
                theirs.mark.as_deref(),
                Some(Annot {
                    kind: AnnotKind::Square { .. },
                    ..
                })
            ),
            "{case}"
        );
        let mut note = doc.state().annots_on(page).unwrap()[0].clone();
        note.contents = "Edited in sheaf".into();
        doc.apply(Edit::AnnotUpdate { page, annot: note }, "Edit")
            .unwrap();
        let snap = doc.snapshot();
        let saves = [
            crate::save::rewrite(&snap).unwrap(),
            crate::incremental::incremental(&snap).unwrap(),
        ];
        for (saved, how) in saves.iter().zip(["rewrite", "incremental"]) {
            let how = format!("{case}, {how}");
            let pdf = Pdf::load_mem(saved).unwrap();
            let page = pdf.page_iter().next().unwrap();
            let found = crate::file::annots_in(&pdf, [page]);
            let contents: Vec<&str> = found.sheaf[&0]
                .iter()
                .map(|a| a.contents.as_str())
                .collect();
            assert_eq!(contents, ["Edited in sheaf", "About this"], "{how}");
            let [theirs] = found.foreign.as_slice() else {
                panic!("{how}: {:?}", found.foreign);
            };
            assert_eq!(theirs.name, "sheaf:4", "{how}");
            let list = pdf.get_dictionary(page).unwrap().get(b"Annots").unwrap();
            let kept = list.as_array().unwrap().iter().find_map(|e| {
                let d = pdf.get_dictionary(e.as_reference().ok()?).ok()?;
                (crate::file::thread_name(d) == "sheaf:4").then_some(d)
            });
            let keys = |d: &Dictionary| -> Vec<(Vec<u8>, Object)> {
                d.iter()
                    .filter(|(k, _)| k.as_slice() != b"AP")
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            };
            assert_eq!(kept.map(keys), Some(keys(&as_left)), "{how}");
        }
    }
}

/// A page stamp written where its page couldn't be found, and an
/// attachment written without its file, hold neither: each opens as
/// another tool's comment, as a stale mark does, and a save of an edit of
/// the page's other mark keeps them as they are.
#[test]
fn a_mark_without_its_page_or_file_is_another_tools_comment() {
    let every = crate::forms::fixture::every_mark_kind();
    // The note (2), the page stamp (18) and the attachment (24).
    let marks = [every[1].clone(), every[17].clone(), every[23].clone()];
    let mut f = Form::new(1);
    let mut page = f.dict(f.pages[0]).clone();
    // A document that has neither.
    let assets = crate::assets::MarkAssets::new(Store::new());
    let mut cx = WriteCx::new(std::sync::Arc::new(assets));
    crate::save::write_page_annots(&mut f.pdf, &mut page, marks.iter(), None, &[], &mut cx);
    *f.dict(f.pages[0]) = page;
    let Opened { mut doc, view } = opened(f.bytes());
    let page = doc.page(0).unwrap().id;
    let mut note = doc.state().annots_on(page).unwrap()[0].clone();
    assert_eq!(note.id, AnnotId(2));
    assert_eq!(doc.state().annots_on(page).unwrap().len(), 1);
    let theirs = |foreign: &[crate::file::ForeignAnnot]| -> Vec<(String, String)> {
        foreign
            .iter()
            .map(|f| (f.name.clone(), f.subtype.clone()))
            .collect()
    };
    let want = [
        ("sheaf:18".to_string(), "Stamp".to_string()),
        ("sheaf:24".to_string(), "FileAttachment".to_string()),
    ];
    assert_eq!(theirs(&view.foreign), want);
    note.contents = "Edited in sheaf".into();
    doc.apply(Edit::AnnotUpdate { page, annot: note }, "Edit")
        .unwrap();
    let snap = doc.snapshot();
    for saved in [
        crate::save::rewrite(&snap).unwrap(),
        crate::incremental::incremental(&snap).unwrap(),
    ] {
        let pdf = Pdf::load_mem(&saved).unwrap();
        let found = crate::file::annots_in(&pdf, pdf.page_iter());
        assert_eq!(found.sheaf[&0].len(), 1);
        assert_eq!(theirs(&found.foreign), want);
        assert!(found.assets.stamps.is_empty() && found.assets.files.is_empty());
    }
}

/// A mark another tool has only written out again stays sheaf's: its
/// reals reformatted (whole ones as integers, integers as reals, the rest
/// to three decimals, as many writers write them), `/Contents` as UTF-16BE
/// in another Unicode normalization, `/T` in PDFDocEncoding, and `/Rect`'s
/// corners swapped. The marks sit off the digest's steps, where three
/// decimals would move a coordinate near the middle of two into the next,
/// had the writer not put them on the steps.
#[test]
fn a_reserialized_mark_stays_a_mark() {
    use unicode_normalization::UnicodeNormalization;
    fn reformat(o: &mut Object) {
        match o {
            Object::Real(r) if r.fract() == 0.0 => *o = Object::Integer(*r as i64),
            Object::Real(r) => *r = (*r * 1000.0).round() / 1000.0,
            Object::Integer(i) => *o = Object::Real(*i as f64),
            Object::Array(items) => items.iter_mut().for_each(reformat),
            Object::Dictionary(d) => d.iter_mut().for_each(|(_, v)| reformat(v)),
            _ => {}
        }
    }
    let marks: Vec<Annot> = every_mark_turned(Rotation::R90)
        .into_iter()
        .map(|a| Annot {
            kind: a.kind.translated(0.0077, 0.0077),
            author: "Ann’s".into(),
            contents: "Café".into(),
            ..a
        })
        .collect();
    let mut f = marked(90, &marks);
    for id in f.annots(0) {
        let d = f.dict(id);
        for (_, v) in d.iter_mut() {
            reformat(v);
        }
        if let Ok(Object::Array(r)) = d.get_mut(b"Rect") {
            r.rotate_left(2);
        }
        if let Ok(contents) = d.get(b"Contents").and_then(Object::as_str) {
            let mut utf16 = vec![0xFE, 0xFF];
            for u in crate::file::text_string(contents)
                .nfd()
                .collect::<String>()
                .encode_utf16()
            {
                utf16.extend_from_slice(&u.to_be_bytes());
            }
            d.set(
                "Contents",
                Object::String(utf16, lopdf::StringFormat::Hexadecimal),
            );
        }
        if d.has(b"T") {
            // ’ is 0x90 in PDFDocEncoding.
            d.set("T", Object::string_literal(b"Ann\x90s".to_vec()));
        }
    }
    let facts = finish(&f.source(), &NoProgress);
    assert!(facts.foreign.is_empty(), "{:?}", facts.foreign);
    assert_marks(&facts.marks[&0], &marks, 0.0, "reserialized");
}

/// A mark's text with characters PDFDocEncoding gives other meanings (a
/// DEL, the accents at 0x18–0x1F) is written so that it reads back as
/// written: the mark opens as itself, and its `/Contents` and `/T` decode
/// to what it says.
#[test]
fn a_mark_s_text_reads_back_as_written() {
    let n = Annot {
        contents: "a\u{7f}b\u{18}c".into(),
        author: "x\u{1f}y".into(),
        ..note()
    };
    let mut f = marked(0, std::slice::from_ref(&n));
    let dict = f.dict(f.annots(0)[0]).clone();
    for (key, want) in [(&b"Contents"[..], &n.contents), (b"T", &n.author)] {
        let text = dict.get(key).and_then(Object::as_str).unwrap();
        assert_eq!(crate::file::text_string(text), *want);
    }
    let facts = finish(&f.source(), &NoProgress);
    assert!(facts.foreign.is_empty(), "{:?}", facts.foreign);
    assert_eq!(facts.marks[&0], [n]);
}

/// A page of version 1 replacements whose StrikeOuts are gone, each of
/// which looks for its own, opens and saves in linear time: the page's
/// StrikeOuts are indexed once, where a search of the page for each took
/// 22 s at 12,000.
#[test]
fn a_page_of_v1_replacements_reads_in_linear_time() {
    const N: usize = 10_000;
    let mut f = Form::new(1);
    let ids: Vec<ObjectId> = (1..=N)
        .map(|i| {
            let json = format!(
                r#"{{"id":{i},"kind":{{"Replace":{{"rects":[],"struck":"x","at":{{"x":1.0,"y":1.0,"w":2.0,"h":2.0}}}}}},"color":[1,2,3],"opacity":1.0,"stroke":1.0,"author":"me","contents":"","modified":"D:1"}}"#
            );
            f.pdf.add_object(dictionary! {
                "Type" => "Annot", "Subtype" => "Caret",
                "Rect" => vec![0.into(), 0.into(), 4.into(), 4.into()],
                "M" => Object::string_literal("D:1"),
                "SheafAnnot" => Object::string_literal(json),
            })
        })
        .collect();
    f.set_annots(0, &ids);
    let started = std::time::Instant::now();
    let facts = finish(&f.source(), &NoProgress);
    let mut page = f.dict(f.pages[0]).clone();
    crate::save::strip_sheaf_annots(&f.pdf, &mut page);
    let took = started.elapsed();
    // None has its StrikeOut, so each is another tool's, and kept.
    assert!(facts.marks.is_empty());
    assert_eq!(facts.foreign.len(), N);
    assert_eq!(page.get(b"Annots").unwrap().as_array().unwrap().len(), N);
    assert!(took < std::time::Duration::from_secs(5), "{took:?}");
}

/// A page another program copied, as Acrobat's Insert Pages copies one,
/// carries copies of its marks, which say the same ids: the first page's
/// keep theirs, and the copies get new ones, the same each time the file
/// opens, all given for the view's one notice (`renamed`). A copied reply
/// stays in the copied note's thread, and so does another tool's reply to
/// the copy; a save of either kind gives the copies `/NM`s of their own,
/// and the other tool's reply follows its note there. Deleting the copied
/// note takes that reply out with it.
#[test]
fn a_page_copied_by_another_tool_gets_fresh_ids() {
    let every = crate::forms::fixture::every_mark_kind();
    // The note (2), the square (4) and the note's reply (16).
    let marks = [every[1].clone(), every[3].clone(), every[15].clone()];
    let mut f = Form::new(2);
    let mut page = f.dict(f.pages[0]).clone();
    let mut cx = WriteCx::new(crate::forms::fixture::mark_assets());
    crate::save::write_page_annots(&mut f.pdf, &mut page, marks.iter(), None, &[], &mut cx);
    *f.dict(f.pages[0]) = page;
    let originals = f.annots(0);
    let mut copies: Vec<ObjectId> = originals
        .iter()
        .map(|&id| {
            let d = f.pdf.get_dictionary(id).unwrap().clone();
            f.pdf.add_object(d)
        })
        .collect();
    for &copy in &copies {
        if let Ok(irt) = f.dict(copy).get(b"IRT").and_then(Object::as_reference) {
            let at = originals.iter().position(|&o| o == irt).unwrap();
            f.dict(copy).set("IRT", copies[at]);
        }
    }
    let theirs = f.pdf.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Text",
        "Rect" => vec![0.into(), 0.into(), 20.into(), 20.into()],
        "IRT" => copies[0], "NM" => Object::string_literal("acro-reply"),
        "T" => Object::string_literal("Acrobat"),
        "Contents" => Object::string_literal("their reply"),
    });
    copies.push(theirs);
    f.set_annots(1, &copies);
    let bytes = f.bytes();

    let Opened { mut doc, view } = opened(bytes.clone());
    let st = doc.state();
    let [first, copy] = [0, 1].map(|i| st.pages[i].id);
    let ids = |page| -> Vec<AnnotId> { st.annots_on(page).unwrap().iter().map(|a| a.id).collect() };
    assert_eq!(ids(first), [AnnotId(2), AnnotId(4), AnnotId(16)]);
    let new = ids(copy);
    assert!(
        new.iter()
            .all(|id| !ids(first).contains(id) && id.0 < 1 << 53),
        "{new:?}"
    );
    assert!(new[0] != new[1] && new[1] != new[2] && new[0] != new[2]);
    let was = [2, 4, 16].map(|id| (copy, AnnotId(id)));
    assert_eq!(view.renamed, was);
    let note = format!("sheaf:{}", new[0].0);
    let reply = &st.annots_on(copy).unwrap()[2];
    assert_eq!(reply.thread_parent(), Some(note.as_str()));
    let threads = |view: &ViewFacts| -> Vec<(String, Option<String>)> {
        view.foreign
            .iter()
            .map(|f| (f.name.clone(), f.parent.clone()))
            .collect()
    };
    let in_thread = [("acro-reply".to_string(), Some(note.clone()))];
    assert_eq!(threads(&view), in_thread);
    let again = opened(bytes);
    assert!(again.doc.state() == st && again.view == view);
    assert!(!doc.is_dirty());

    // Nothing else changed, and a save of either kind writes them.
    let snap = doc.snapshot();
    let saves = [
        crate::save::rewrite(&snap).unwrap(),
        crate::incremental::incremental(&snap).unwrap(),
    ];
    // Other tools' first, as a save writes a page's annotations.
    let mut want = vec!["acro-reply".to_string()];
    want.extend(new.iter().map(|id| format!("sheaf:{}", id.0)));
    for (saved, how) in saves.into_iter().zip(["rewrite", "incremental"]) {
        let pdf = Pdf::load_mem(&saved).unwrap();
        let second = pdf.page_iter().nth(1).unwrap();
        let list = pdf.get_dictionary(second).unwrap().get(b"Annots").unwrap();
        let names: Vec<String> = list
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                crate::file::thread_name(pdf.get_dictionary(e.as_reference().unwrap()).unwrap())
            })
            .collect();
        assert_eq!(names, want, "{how}");
        let reopened = opened(saved);
        assert!(reopened.view.renamed.is_empty(), "{how}");
        assert_eq!(threads(&reopened.view), in_thread, "{how}");
        let st = reopened.doc.state();
        let marks = st.annots_on(st.pages[1].id).unwrap();
        assert_eq!(marks.iter().map(|a| a.id).collect::<Vec<_>>(), new, "{how}");
    }

    // The copied note deleted, the other tool's reply to it goes too.
    doc.apply(
        Edit::AnnotDelete {
            page: copy,
            id: new[0],
        },
        "Delete",
    )
    .unwrap();
    let snap = doc.snapshot();
    let saves = [
        crate::save::rewrite(&snap).unwrap(),
        crate::incremental::incremental(&snap).unwrap(),
    ];
    for (saved, how) in saves.into_iter().zip(["rewrite", "incremental"]) {
        let reopened = opened(saved);
        assert!(
            reopened.view.foreign.is_empty(),
            "{how}: {:?}",
            reopened.view.foreign
        );
    }
}

/// Trees whose nodes list themselves, twice each, are read a node at a
/// time: the `/Dests` a bookmark names, `/EmbeddedFiles`, `/PageLabels`,
/// the layers' `/Order` and a portfolio's folders. Walked by depth alone,
/// each was 2^32 visits, and the file never opened.
#[test]
fn a_tree_that_lists_itself_is_read_once() {
    let mut f = Form::new(1);
    let (catalog, page) = (f.catalog, f.pages[0]);
    let mut looped = |mut node: Dictionary| {
        let id = f.pdf.new_object_id();
        node.set("Kids", vec![id.into(), id.into()]);
        f.pdf.objects.insert(id, node.into());
        id
    };
    let there = vec![page.into(), "Fit".into()];
    let dests =
        looped(dictionary! { "Names" => vec![Object::string_literal("there"), there.into()] });
    let spec = dictionary! { "Type" => "Filespec", "F" => Object::string_literal("a.txt") };
    let files =
        looped(dictionary! { "Names" => vec![Object::string_literal("a.txt"), spec.into()] });
    let labels =
        looped(dictionary! { "Nums" => vec![0.into(), dictionary! { "S" => "r" }.into()] });
    // A folder that's its own first child and its own next sibling.
    let folder = f.pdf.new_object_id();
    f.pdf.objects.insert(
        folder,
        dictionary! {
            "Name" => Object::string_literal("Tax"), "ID" => 1,
            "Child" => folder, "Next" => folder,
        }
        .into(),
    );
    let ocg = f
        .pdf
        .add_object(dictionary! { "Type" => "OCG", "Name" => "Ink" });
    let order = f.pdf.new_object_id();
    f.pdf
        .objects
        .insert(order, vec![order.into(), order.into(), ocg.into()].into());
    let root = f.dict(catalog);
    root.set(
        "Names",
        dictionary! { "Dests" => dests, "EmbeddedFiles" => files },
    );
    root.set("PageLabels", labels);
    root.set(
        "OCProperties",
        dictionary! {
            "OCGs" => vec![ocg.into()],
            "D" => dictionary! { "Order" => order },
        },
    );
    root.set(
        "Collection",
        dictionary! { "Folders" => dictionary! { "ID" => 0, "Child" => folder } },
    );
    let named = dictionary! {
        "Title" => Object::string_literal("There"),
        "Dest" => Object::string_literal("there"),
    };
    outline_tree(&mut f, vec![(0, named)]);
    let src = f.source();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn({
        let src = src.clone();
        move || tx.send(finish(&src, &NoProgress)).ok()
    });
    let facts = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the open never ended");
    assert_eq!(facts.outline[0].page, Some(0));
    let names: Vec<&str> = facts.attachments.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["a.txt"]);
    assert_eq!(facts.labels.map(|l| l.len()), Some(1));
    let layers: Vec<(Option<ObjectId>, u16)> =
        facts.layers.iter().map(|l| (l.id, l.depth)).collect();
    assert_eq!(layers, [(Some(ocg), 0)]);
    let folders = crate::portfolio::folder_paths(src.pdf());
    assert_eq!(
        folders,
        HashMap::from([(0, String::new()), (1, "Tax".to_string())])
    );
}

/// A fact the open reads, as the corpus test compares it: the pages and
/// fields with the manifest row, the others with the old loaders' eager
/// reading of the same bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Fact {
    Pages,
    Fields,
    Marks,
    Foreign,
    Labels,
    HeaderFooter,
    Attachments,
    Collection,
    Layers,
}

/// How a corpus file read.
#[derive(Default)]
struct Outcome {
    /// The source didn't open (no facts), or lopdf didn't load the file
    /// eagerly (none to compare but the manifest's).
    unopened: bool,
    unloaded: bool,
    /// The page table numbers the pages otherwise than `get_pages`.
    renumbered: bool,
    differ: Vec<Fact>,
    /// The fields read, and whether the field tree is damaged.
    fields: usize,
    damaged: bool,
    /// What was read: marks, other tools' comments, attachments, layers.
    counts: [usize; 4],
    /// Whether it has labels, a header/footer, a portfolio schema,
    /// bookmark styles.
    with: [bool; 4],
}

/// Why a corpus file's facts aren't its manifest row's, where the
/// manifest is what's wrong, or the file is: its fields, as sheaf-ops'
/// `every_form_reads_as_its_manifest_row` finds them too.
#[derive(Debug, Clone, Copy)]
enum Explained {
    /// Fewer fields than the row: pypdf counts a field listed twice
    /// twice, and lopdf loses an object of a few damaged files.
    Fewer,
    /// A damaged field tree, which sheaf doesn't read.
    Damaged,
}

impl Explained {
    /// Whether it explains how `o`, with `entry`'s row, differs.
    fn covers(self, o: &Outcome, entry: &Entry) -> bool {
        o.differ == [Fact::Fields]
            && match self {
                Explained::Fewer => !o.damaged && (o.fields as u32) < entry.fields,
                Explained::Damaged => o.damaged,
            }
    }
}

/// The corpus files whose facts aren't their rows', and why. Every other
/// fact of every file is the old loaders'.
const EXPLAINED: &[(&str, Explained)] = &[
    ("forms/safedocs/0051572.pdf", Explained::Fewer),
    ("forms/safedocs/0053329.pdf", Explained::Fewer),
    ("forms/safedocs/0055217.pdf", Explained::Fewer),
    ("forms/safedocs/0092321.pdf", Explained::Fewer),
    ("forms/safedocs/0218989.pdf", Explained::Fewer),
    ("forms/safedocs/0304368.pdf", Explained::Fewer),
    ("forms/safedocs/0619933.pdf", Explained::Damaged),
    ("forms/safedocs/0630899.pdf", Explained::Fewer),
    ("forms/safedocs/0799563.pdf", Explained::Fewer),
    ("forms/safedocs/0982696.pdf", Explained::Fewer),
    ("forms/safedocs/1089075.pdf", Explained::Fewer),
    ("forms/safedocs/1210120.pdf", Explained::Fewer),
];

/// `entry`'s file opened, and its facts compared.
fn compare(corpus: &Corpus, entry: &Entry) -> Outcome {
    let Ok(info) = crate::SourceInfo::read(&Store::new(), &corpus.path(entry), None) else {
        return Outcome {
            unopened: true,
            ..Default::default()
        };
    };
    let src = &info.src;
    let facts = finish(src, &NoProgress);
    let mut out = Outcome {
        fields: facts.form.facts.fields,
        damaged: facts.form.facts.damaged.is_some(),
        counts: [
            facts.marks.values().map(Vec::len).sum(),
            facts.foreign.len(),
            facts.attachments.len(),
            facts.layers.len(),
        ],
        with: [
            facts.labels.is_some(),
            facts.header_footer.is_some(),
            facts.collection.is_some(),
            facts
                .outline
                .iter()
                .any(|b| b.color.is_some() || !b.style.is_plain()),
        ],
        ..Default::default()
    };
    // pypdf couldn't read an unparsed one: its row has no counts.
    if entry.kind != "unparsed" {
        if src.page_count() != entry.pages {
            out.differ.push(Fact::Pages);
        }
        if out.fields as u32 != entry.fields {
            out.differ.push(Fact::Fields);
        }
    }
    let Ok(pdf) = crate::file::load_pdf_mem(src.bytes(), None) else {
        out.unloaded = true;
        return out;
    };
    out.renumbered = !src.page_ids().eq(pdf.page_iter());
    let old = crate::file::annots_in(&pdf, pdf.page_iter());
    let same = [
        (Fact::Marks, facts.marks == old.sheaf),
        (Fact::Foreign, facts.foreign == old.foreign),
        (
            Fact::Labels,
            facts.labels == crate::labels::label_ranges(&pdf),
        ),
        (
            Fact::HeaderFooter,
            facts.header_footer == crate::stamps::from_catalog(&pdf),
        ),
        (
            Fact::Attachments,
            facts.attachments == crate::attachments::attachments_in(&pdf, pdf.page_iter()),
        ),
        (
            Fact::Collection,
            facts.collection == crate::portfolio::collection(&pdf),
        ),
        (Fact::Layers, facts.layers == crate::layers::layers_in(&pdf)),
    ];
    out.differ
        .extend(same.into_iter().filter(|(_, same)| !same).map(|(f, _)| f));
    out
}

/// Every manifest file's facts, read from its source as it opens, are
/// what the old loaders read: its pages and fields are its manifest
/// row's, and every other fact is what the same readers read from lopdf's
/// eager load of the same bytes, numbering pages by `get_pages`. What
/// differs is [`EXPLAINED`]'s, and only that. (The outline is PDFium's,
/// as sheaf-render's `the_source_outline_is_pdfium_s` finds.) Skips
/// without the corpus.
#[test]
fn the_open_s_facts_from_the_source_equal_the_eager_reading() {
    let Some(corpus) = Corpus::or_skip() else {
        return;
    };
    let entries = corpus.manifest().unwrap();
    let outcomes = each_entry(&entries, |entry| compare(&corpus, entry));
    let count = |f: fn(&Outcome) -> bool| outcomes.iter().filter(|o| f(o)).count();
    let total = |i: usize| outcomes.iter().map(|o| o.counts[i]).sum::<usize>();
    let with = |i: usize| outcomes.iter().filter(|o| o.with[i]).count();
    eprintln!(
        "{} files: {} sources don't open, {} don't load eagerly, {} number their pages \
         otherwise; {} marks, {} other tools' comments, {} attachments, {} layers; {} with \
         labels, {} with a header/footer, {} portfolios, {} with bookmark styles",
        outcomes.len(),
        count(|o| o.unopened),
        count(|o| o.unloaded),
        count(|o| o.renumbered),
        total(0),
        total(1),
        total(2),
        total(3),
        with(0),
        with(1),
        with(2),
        with(3),
    );
    let differ: Vec<(&Entry, &Outcome)> = entries
        .iter()
        .zip(&outcomes)
        .filter(|(_, o)| !o.differ.is_empty())
        .collect();
    let mut unexplained = Vec::new();
    for (entry, o) in &differ {
        let why = EXPLAINED
            .iter()
            .find(|(path, _)| *path == entry.path)
            .map(|&(_, why)| why);
        eprintln!(
            "{}: {:?} ({} fields, the row {}), {why:?}",
            entry.path, o.differ, o.fields, entry.fields
        );
        if !why.is_some_and(|why| why.covers(o, entry)) {
            unexplained.push((&entry.path, &o.differ));
        }
    }
    assert!(unexplained.is_empty(), "{unexplained:#?}");
    let stale: Vec<&str> = EXPLAINED
        .iter()
        .map(|&(path, _)| path)
        .filter(|path| !differ.iter().any(|(e, _)| e.path == *path))
        .collect();
    assert!(stale.is_empty(), "no longer differ: {stale:?}");
}
