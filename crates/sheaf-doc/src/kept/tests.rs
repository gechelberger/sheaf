//! Bug 14's reproductions and their variants: what a save, an extract, a
//! crop or a stamp writes holds no page it doesn't keep.

use std::path::{Path, PathBuf};

use lopdf::content::{Content, Operation};
use lopdf::{Document as Pdf, Object, ObjectId, Stream, dictionary};
use sheaf_core::Rotation;

use crate::annot::{Annot, AnnotId, AnnotKind, Pt};
use crate::history::FixedCount;
use crate::model::{Document, Edit, SourceInfo};
use crate::save::{Strategy, rewrite, save_to_path};
use crate::store::Store;

fn tempdir() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let d = std::env::temp_dir().join(format!(
        "sheaf-kept-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A file of `n` pages saying "MARK <tag> <i>", with `extra` adding to it
/// (links, names…) given the page ids.
fn file(
    dir: &Path,
    name: &str,
    tag: &str,
    n: usize,
    extra: impl FnOnce(&mut Pdf, &[ObjectId]),
) -> PathBuf {
    let mut pdf = Pdf::with_version("1.5");
    let pages_id = pdf.new_object_id();
    let font = pdf.add_object(
        dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" },
    );
    let mut ids = Vec::new();
    for i in 0..n {
        let content = Content {
            operations: vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F1".into(), 24.into()]),
                Operation::new("Td", vec![72.into(), 700.into()]),
                Operation::new(
                    "Tj",
                    vec![Object::string_literal(format!("MARK {tag} {i}"))],
                ),
                Operation::new("ET", vec![]),
            ],
        };
        let c = pdf.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
        ids.push(pdf.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => c,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } },
        }));
    }
    pdf.objects.insert(
        pages_id,
        dictionary! {
            "Type" => "Pages", "Count" => n as i64,
            "Kids" => ids.iter().map(|&i| Object::Reference(i)).collect::<Vec<_>>(),
        }
        .into(),
    );
    let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    pdf.trailer.set("Root", catalog);
    extra(&mut pdf, &ids);
    let path = dir.join(name);
    pdf.save(&path).unwrap();
    path
}

/// A link on `from` to `to`.
fn link(pdf: &mut Pdf, from: ObjectId, to: ObjectId) {
    let l = pdf.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Link",
        "Rect" => vec![72.into(), 72.into(), 144.into(), 96.into()],
        "Dest" => vec![to.into(), "Fit".into()],
    });
    let page = pdf.get_dictionary_mut(from).unwrap();
    let mut annots = page
        .get(b"Annots")
        .and_then(|a| a.as_array())
        .cloned()
        .unwrap_or_default();
    annots.push(l.into());
    page.set("Annots", annots);
}

fn catalog(pdf: &mut Pdf) -> &mut lopdf::Dictionary {
    let id = pdf.trailer.get(b"Root").unwrap().as_reference().unwrap();
    pdf.get_dictionary_mut(id).unwrap()
}

fn opened(path: &Path) -> Document {
    let store = Store::new();
    let primary = SourceInfo::read(&store, path, None).unwrap();
    Document::new(store, primary, &[], Box::new(FixedCount(10)))
}

/// Whether any stream in the file says `text`, decompressed.
fn says(bytes: &[u8], text: &str) -> bool {
    let pdf = Pdf::load_mem(bytes).unwrap();
    pdf.objects.values().any(|o| match o {
        Object::Stream(s) => {
            let data = s
                .decompressed_content()
                .unwrap_or_else(|_| s.content.clone());
            String::from_utf8_lossy(&data).contains(text)
        }
        _ => false,
    })
}

fn page_annots(bytes: &[u8], page: u32) -> usize {
    let pdf = Pdf::load_mem(bytes).unwrap();
    let id = pdf.get_pages()[&(page + 1)];
    pdf.get_dictionary(id)
        .unwrap()
        .get(b"Annots")
        .ok()
        .and_then(|a| pdf.dereference(a).ok())
        .and_then(|(_, a)| a.as_array().ok().map(|a| a.len()))
        .unwrap_or(0)
}

/// `bytes` saved by `strategy`: through the chooser to the file itself, or
/// a plain rewrite.
fn saved(model: &Document, path: &Path, by_chooser: bool) -> Vec<u8> {
    if by_chooser {
        save_to_path(&model.snapshot(), path).unwrap();
        std::fs::read(path).unwrap()
    } else {
        rewrite(&model.snapshot()).unwrap()
    }
}

#[test]
fn a_deleted_page_leaves_the_file_whatever_points_at_it() {
    for by_chooser in [true, false] {
        let dir = tempdir();
        let path = file(&dir, "a.pdf", "A", 3, |pdf, p| {
            // Page 1 links to page 3, by a link, a name, the old /Dests
            // and the open action.
            link(pdf, p[0], p[2]);
            let names = pdf.add_object(dictionary! {
                "Names" => vec![Object::string_literal("end"), Object::Array(vec![p[2].into(), "Fit".into()]),
                                Object::string_literal("start"), Object::Array(vec![p[0].into(), "Fit".into()])],
            });
            let dests =
                pdf.add_object(dictionary! { "last" => vec![Object::from(p[2]), "Fit".into()] });
            let named = pdf.add_object(dictionary! {
                "Type" => "Annot", "Subtype" => "Link",
                "Rect" => vec![72.into(), 200.into(), 144.into(), 220.into()],
                "Dest" => Object::string_literal("end"),
            });
            let c = catalog(pdf);
            c.set("Names", dictionary! { "Dests" => names });
            c.set("Dests", dests);
            c.set("OpenAction", vec![Object::from(p[2]), "Fit".into()]);
            let page = pdf.get_dictionary_mut(p[0]).unwrap();
            let mut a = page.get(b"Annots").unwrap().as_array().unwrap().clone();
            a.push(named.into());
            page.set("Annots", a);
        });
        let mut model = opened(&path);
        let third = model.page(2).unwrap().id;
        model
            .apply(Edit::DeletePages { ids: vec![third] }, "del")
            .unwrap();
        let bytes = saved(&model, &path, by_chooser);
        assert!(!says(&bytes, "MARK A 2"), "chooser {by_chooser}");
        assert!(says(&bytes, "MARK A 0") && says(&bytes, "MARK A 1"));
        // Both links led to it: they're gone. So are the name and the open
        // action; the name for page 1 stays.
        assert_eq!(page_annots(&bytes, 0), 0);
        let pdf = Pdf::load_mem(&bytes).unwrap();
        let c = pdf.catalog().unwrap();
        assert!(c.get(b"OpenAction").is_err());
        let names = pdf
            .dereference(c.get(b"Names").unwrap())
            .unwrap()
            .1
            .as_dict()
            .unwrap()
            .get(b"Dests")
            .unwrap();
        let tree = pdf.dereference(names).unwrap().1.as_dict().unwrap();
        assert_eq!(tree.get(b"Names").unwrap().as_array().unwrap().len(), 2);
        let dests = pdf
            .dereference(c.get(b"Dests").unwrap())
            .unwrap()
            .1
            .as_dict()
            .unwrap();
        assert!(dests.is_empty());
        assert_eq!(super::page_count_reachable(&pdf), 2);
    }
}

/// Whether the file holds `text` anywhere: in its bytes, or a stream's.
fn holds(bytes: &[u8], text: &str) -> bool {
    bytes.windows(text.len()).any(|w| w == text.as_bytes()) || says(bytes, text)
}

#[test]
fn a_reply_keeps_no_comment_of_a_deleted_page() {
    // A note on page 2, and a reply to it on page 1: a reply's /IRT is the
    // one way left to reach the note once its page goes.
    for by_chooser in [true, false] {
        let dir = tempdir();
        let path = file(&dir, "a.pdf", "A", 2, |pdf, p| {
            let note = pdf.add_object(dictionary! {
                "Type" => "Annot", "Subtype" => "Text", "P" => p[1],
                "Rect" => vec![72.into(), 72.into(), 96.into(), 96.into()],
                "Contents" => Object::string_literal("SHEAFLEAK note"),
            });
            pdf.get_dictionary_mut(p[1])
                .unwrap()
                .set("Annots", vec![note.into()]);
            let reply = pdf.add_object(dictionary! {
                "Type" => "Annot", "Subtype" => "Text", "P" => p[0], "IRT" => note,
                "Rect" => vec![72.into(), 72.into(), 96.into(), 96.into()],
                "Contents" => Object::string_literal("a reply"),
            });
            pdf.get_dictionary_mut(p[0])
                .unwrap()
                .set("Annots", vec![reply.into()]);
        });
        let mut model = opened(&path);
        let second = model.page(1).unwrap().id;
        model
            .apply(Edit::DeletePages { ids: vec![second] }, "del")
            .unwrap();
        let bytes = saved(&model, &path, by_chooser);
        assert!(!holds(&bytes, "SHEAFLEAK"), "chooser {by_chooser}");
        assert!(holds(&bytes, "a reply"), "chooser {by_chooser}");
        assert_eq!(page_annots(&bytes, 0), 1);
    }
}

#[test]
fn inserting_one_page_brings_only_that_page() {
    for incremental in [true, false] {
        let dir = tempdir();
        let a = file(&dir, "a.pdf", "A", 1, |_, _| {});
        let b = file(&dir, "b.pdf", "B", 3, |pdf, p| {
            link(pdf, p[0], p[2]);
            link(pdf, p[0], p[1]);
        });
        let mut model = opened(&a);
        let sb = model.add_source(SourceInfo::read(model.store(), &b, None).unwrap());
        // B's first and third pages: the link between them stays, the one
        // to its second goes, and the second isn't in the file.
        for range in [0..1, 2..3] {
            model
                .apply(
                    Edit::InsertPages {
                        source: sb,
                        range,
                        before: None,
                        outline: None,
                        annots: Vec::new(),
                    },
                    "ins",
                )
                .unwrap();
        }
        assert!(!model.snapshot().removes_content);
        let bytes = if incremental {
            crate::incremental::incremental(&model.snapshot()).unwrap()
        } else {
            rewrite(&model.snapshot()).unwrap()
        };
        assert!(!says(&bytes, "MARK B 1"), "incremental {incremental}");
        assert!(says(&bytes, "MARK B 0") && says(&bytes, "MARK B 2"));
        let pdf = Pdf::load_mem(&bytes).unwrap();
        let pages = pdf.get_pages();
        let b0 = pages[&2];
        let annots = pdf
            .get_dictionary(b0)
            .unwrap()
            .get(b"Annots")
            .unwrap()
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(annots.len(), 1, "incremental {incremental}");
        let link = pdf.dereference(&annots[0]).unwrap().1.as_dict().unwrap();
        let to = link.get(b"Dest").unwrap().as_array().unwrap()[0]
            .as_reference()
            .unwrap();
        assert_eq!(to, pages[&3]);
        assert_eq!(super::page_count_reachable(&pdf), 3);
    }
}

#[test]
fn fields_tags_and_threads_let_go_of_a_deleted_page() {
    let dir = tempdir();
    let path = file(&dir, "a.pdf", "A", 3, |pdf, p| {
        // A field with a widget on pages 1 and 3, and one only on page 3.
        let widget = |pdf: &mut Pdf, page: ObjectId, parent: Option<ObjectId>| {
            let mut w = dictionary! {
                "Type" => "Annot", "Subtype" => "Widget", "P" => page,
                "Rect" => vec![72.into(), 300.into(), 200.into(), 320.into()],
            };
            if let Some(parent) = parent {
                w.set("Parent", parent);
            } else {
                w.set("FT", "Tx");
                w.set("T", Object::string_literal("only3"));
            }
            let id = pdf.add_object(w);
            let d = pdf.get_dictionary_mut(page).unwrap();
            let mut a = d
                .get(b"Annots")
                .and_then(|a| a.as_array())
                .cloned()
                .unwrap_or_default();
            a.push(id.into());
            d.set("Annots", a);
            id
        };
        let both = pdf.new_object_id();
        let w0 = widget(pdf, p[0], Some(both));
        let w2 = widget(pdf, p[2], Some(both));
        pdf.objects.insert(
            both,
            dictionary! { "FT" => "Tx", "T" => Object::string_literal("both"), "Kids" => vec![w0.into(), w2.into()] }.into(),
        );
        let only = widget(pdf, p[2], None);
        // Tags: a paragraph on each page, under a document element.
        let root = pdf.new_object_id();
        let doc = pdf.new_object_id();
        let para = |pdf: &mut Pdf, page: ObjectId| {
            pdf.add_object(dictionary! { "Type" => "StructElem", "S" => "P", "P" => doc, "Pg" => page, "K" => 0 })
        };
        let e0 = para(pdf, p[0]);
        let e2 = para(pdf, p[2]);
        pdf.objects.insert(
            doc,
            dictionary! { "Type" => "StructElem", "S" => "Document", "P" => root, "K" => vec![e0.into(), e2.into()] }.into(),
        );
        let parents = pdf.add_object(dictionary! {
            "Nums" => vec![0.into(), Object::Array(vec![e0.into()]), 1.into(), Object::Array(vec![e2.into()])],
        });
        pdf.objects.insert(
            root,
            dictionary! { "Type" => "StructTreeRoot", "K" => doc, "ParentTree" => parents }.into(),
        );
        // An article thread through pages 2 and 3.
        let thread = pdf.new_object_id();
        let (b1, b2) = (pdf.new_object_id(), pdf.new_object_id());
        let r = || vec![0.into(), 0.into(), 100.into(), 100.into()];
        pdf.objects.insert(
            b1,
            dictionary! { "T" => thread, "N" => b2, "V" => b2, "P" => p[1], "R" => r() }.into(),
        );
        pdf.objects.insert(
            b2,
            dictionary! { "N" => b1, "V" => b1, "P" => p[2], "R" => r() }.into(),
        );
        pdf.objects
            .insert(thread, dictionary! { "Type" => "Thread", "F" => b1 }.into());
        pdf.get_dictionary_mut(p[1])
            .unwrap()
            .set("B", vec![Object::from(b1)]);
        pdf.get_dictionary_mut(p[2])
            .unwrap()
            .set("B", vec![Object::from(b2)]);
        for (i, page) in p.iter().enumerate() {
            pdf.get_dictionary_mut(*page)
                .unwrap()
                .set("StructParents", i as i64);
        }
        let c = catalog(pdf);
        c.set(
            "AcroForm",
            dictionary! { "Fields" => vec![both.into(), only.into()] },
        );
        c.set("StructTreeRoot", root);
        c.set("MarkInfo", dictionary! { "Marked" => true });
        c.set("Threads", vec![Object::from(thread)]);
    });
    let mut model = opened(&path);
    let third = model.page(2).unwrap().id;
    model
        .apply(Edit::DeletePages { ids: vec![third] }, "del")
        .unwrap();
    let bytes = rewrite(&model.snapshot()).unwrap();
    assert!(!says(&bytes, "MARK A 2"));
    let pdf = Pdf::load_mem(&bytes).unwrap();
    assert_eq!(super::page_count_reachable(&pdf), 2);
    let c = pdf.catalog().unwrap();
    // The field on both keeps page 1's widget; the one only on page 3 is gone.
    let form = pdf
        .dereference(c.get(b"AcroForm").unwrap())
        .unwrap()
        .1
        .as_dict()
        .unwrap();
    let fields = form.get(b"Fields").unwrap().as_array().unwrap();
    assert_eq!(fields.len(), 1);
    let both = pdf.dereference(&fields[0]).unwrap().1.as_dict().unwrap();
    assert_eq!(both.get(b"Kids").unwrap().as_array().unwrap().len(), 1);
    // The tags keep page 1's paragraph; page 3's is gone from the tree and
    // the parent tree.
    let root = pdf
        .dereference(c.get(b"StructTreeRoot").unwrap())
        .unwrap()
        .1
        .as_dict()
        .unwrap();
    let doc = pdf
        .dereference(root.get(b"K").unwrap())
        .unwrap()
        .1
        .as_dict()
        .unwrap();
    assert_eq!(doc.get(b"K").unwrap().as_array().unwrap().len(), 1);
    let parents = pdf
        .dereference(root.get(b"ParentTree").unwrap())
        .unwrap()
        .1
        .as_dict()
        .unwrap();
    assert_eq!(parents.get(b"Nums").unwrap().as_array().unwrap().len(), 2);
    // The thread through page 3 is dropped, and page 2's bead of it.
    assert!(c.get(b"Threads").is_err());
    let page2 = pdf.get_dictionary(pdf.get_pages()[&2]).unwrap();
    assert!(page2.get(b"B").is_err());
}

#[test]
fn copies_hold_only_the_pages_copied() {
    let dir = tempdir();
    let path = file(&dir, "a.pdf", "A", 3, |pdf, p| {
        link(pdf, p[0], p[2]);
        link(pdf, p[0], p[1]);
    });
    let mut model = opened(&path);
    let (p0, p1) = (model.page(0).unwrap().id, model.page(1).unwrap().id);
    // Extract pages 1 and 2: page 3 doesn't come, nor does the link to it.
    let bytes = crate::save::extract_bytes(&model.snapshot(), &[p0, p1]).unwrap();
    assert!(!says(&bytes, "MARK A 2") && says(&bytes, "MARK A 1"));
    assert_eq!(page_annots(&bytes, 0), 1);
    // A crop is a copy of the page alone.
    let src = model.snapshot().sources[&model.primary()].clone();
    let bytes = crate::save::cropped_copy(&src, &[(0, [0.0, 0.0, 300.0, 300.0])]).unwrap();
    assert!(!says(&bytes, "MARK A 1") && !says(&bytes, "MARK A 2"));
    assert_eq!(page_annots(&bytes, 0), 0);
    // So is a stamp made from a page, and the forms the summary draws.
    let pdf = Pdf::load(&path).unwrap();
    let stamp = crate::stamp_lib::one_page(&pdf, 0).unwrap();
    assert!(!says(&stamp, "MARK A 1") && !says(&stamp, "MARK A 2"));
    let mut dst = Pdf::with_version("1.5");
    let first = pdf.get_pages()[&1];
    crate::stamp_lib::page_forms(&pdf, &[first], &mut dst).unwrap();
    let mut out = Vec::new();
    dst.save_to(&mut out).unwrap();
    assert!(says(&out, "MARK A 0") && !says(&out, "MARK A 2"));
    // Duplicating a page doesn't make anything go: an incremental save.
    model
        .apply(Edit::DuplicatePages { ids: vec![p0] }, "dup")
        .unwrap();
    assert!(!model.snapshot().removes_content);
}

fn note(text: &str) -> Annot {
    Annot {
        id: AnnotId(0),
        kind: AnnotKind::Note {
            pos: Pt { x: 100.0, y: 100.0 },
        },
        color: [255, 230, 0],
        opacity: 1.0,
        stroke: 1.0,
        author: "Greg".into(),
        contents: text.into(),
        modified: String::new(),
        created: String::new(),
        fill: None,
        dashed: false,
        cloudy: false,
        turn: Rotation::R0,
        name: String::new(),
    }
}

#[test]
fn a_deleted_mark_is_gone_from_the_saved_bytes() {
    let dir = tempdir();
    let path = file(&dir, "a.pdf", "A", 1, |_, _| {});
    let mut model = opened(&path);
    let p0 = model.page(0).unwrap().id;
    model
        .apply(
            Edit::AnnotAdd {
                page: p0,
                annot: note("SECRET-NOTE-TEXT"),
            },
            "note",
        )
        .unwrap();
    assert_eq!(
        save_to_path(&model.snapshot(), &path).unwrap(),
        Strategy::Incremental
    );
    model.rebase_to_saved(SourceInfo::read(model.store(), &path, None).unwrap());
    let id = model.state().annots_on(p0).unwrap()[0].id;
    model
        .apply(Edit::AnnotDelete { page: p0, id }, "delete")
        .unwrap();
    assert!(model.snapshot().removes_content);
    // Undone, it's the saved version again: nothing removed.
    model.undo().unwrap();
    assert!(!model.snapshot().removes_content);
    model.redo().unwrap();
    assert_eq!(
        save_to_path(&model.snapshot(), &path).unwrap(),
        Strategy::Rewrite
    );
    let bytes = std::fs::read(&path).unwrap();
    let needle = b"SECRET-NOTE-TEXT";
    assert!(!bytes.windows(needle.len()).any(|w| w == needle));
    assert!(!says(&bytes, "SECRET-NOTE-TEXT"));
}

#[test]
fn replacing_saved_ocr_text_takes_the_old_out() {
    use crate::text_layer::{OcrWord, TextLayer};
    let dir = tempdir();
    let path = file(&dir, "a.pdf", "A", 1, |_, _| {});
    let mut model = opened(&path);
    let p0 = model.page(0).unwrap().id;
    let layer = |text: &str| {
        std::sync::Arc::new(TextLayer::from_engine(
            vec![OcrWord {
                text: text.into(),
                rect: crate::annot::Box2 {
                    x: 72.0,
                    y: 100.0,
                    w: 60.0,
                    h: 12.0,
                },
                confidence: 0.9,
            }],
            &crate::geom::Geom::read(None, None, 0),
            "test",
        ))
    };
    let layers = |pdf: &[u8]| {
        Pdf::load_mem(pdf)
            .unwrap()
            .objects
            .values()
            .filter(|o| {
                // The layer's text, not the `q` that sets it apart.
                o.as_stream().is_ok_and(|s| {
                    s.dict.has(b"SheafOcrLayer")
                        && s.decompressed_content()
                            .unwrap_or_else(|_| s.content.clone())
                            != b"q\n"
                })
            })
            .count()
    };
    model
        .apply(
            Edit::SetTextLayers {
                layers: vec![(p0, layer("first"))],
            },
            "ocr",
        )
        .unwrap();
    assert_eq!(
        save_to_path(&model.snapshot(), &path).unwrap(),
        Strategy::Incremental
    );
    let saved = std::fs::read(&path).unwrap();
    assert_eq!(layers(&saved), 1);
    model.rebase_to_saved(SourceInfo::read(model.store(), &path, None).unwrap());
    // Recognized again: the old text would stay in the earlier revision,
    // so it's a full save.
    model
        .apply(
            Edit::SetTextLayers {
                layers: vec![(p0, layer("second"))],
            },
            "ocr",
        )
        .unwrap();
    assert_eq!(
        save_to_path(&model.snapshot(), &path).unwrap(),
        Strategy::Rewrite
    );
    assert_eq!(layers(&std::fs::read(&path).unwrap()), 1);
}
