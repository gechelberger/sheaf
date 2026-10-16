//! The open: no fact after the first edit; the headless open; a file
//! whose pages' content is never read; trees that list themselves; and
//! over Greg's corpus, the old loaders' eager reading of each file. The
//! readers' own tests read their fixtures as sources with [`lazily`].

use lopdf::{Dictionary, Document as Pdf, Object, ObjectId, Stream, dictionary};
use sheaf_core::Rotation;

use super::*;
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
    let counted = sheaf_progress::Tracker::new();
    finish(&f.source(), &counted);
    let s = counted.snapshot();
    assert_eq!((s.done, s.total, s.unit), (3, Some(3), Unit::Pages));
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
