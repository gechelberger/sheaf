use super::*;
use sheaf_core::Rotation;
use sheaf_doc::annot::REVIEW;
use sheaf_doc::{FixedCount, Pt, SourceInfo, Store};

#[test]
fn dates_format() {
    assert_eq!(fmt_pdf_date("D:20261011153000Z"), "2026-10-11 15:30");
    assert_eq!(fmt_pdf_date("D:20261011"), "2026-10-11");
    assert_eq!(fmt_pdf_date(""), "");
    assert_eq!(fmt_pdf_date("garbage"), "");
}

fn mark(kind: AnnotKind, contents: &str) -> Annot {
    Annot {
        id: AnnotId(1),
        kind,
        color: [0, 0, 255],
        opacity: 1.0,
        stroke: 1.0,
        author: String::new(),
        contents: contents.into(),
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
fn text_edits_read_as_what_they_suggest() {
    let at = Box2 {
        x: 0.0,
        y: 0.0,
        w: 6.0,
        h: 6.0,
    };
    let replace = |contents| {
        mark(
            AnnotKind::Replace {
                rects: vec![at],
                struck: "teh".into(),
                at,
            },
            contents,
        )
    };
    assert_eq!(ours_text(&replace("the")), "Replace “teh” with “the”");
    assert_eq!(ours_text(&replace("")), "Cross out “teh”");
    assert_eq!(
        ours_text(&mark(AnnotKind::Insert { at }, "not")),
        "Insert “not”"
    );
    // One that says nothing reads as what it is.
    assert_eq!(
        ours_text(&mark(
            AnnotKind::Note {
                pos: Pt { x: 0.0, y: 0.0 }
            },
            ""
        )),
        "Note"
    );
}

fn comment(i: usize, name: &str, parent: Option<&str>, date: &str) -> Comment {
    Comment {
        item: Item::Foreign(i),
        name: name.into(),
        parent: parent.map(str::to_string),
        grouped: false,
        date: date.into(),
        modified: date.into(),
        state: None,
        author: "Pat".into(),
        body: String::new(),
        label: "Note".into(),
        kind: "Note".into(),
        color: None,
        rect: Box2 {
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
        },
        measurement: None,
    }
}

#[test]
fn replies_thread_under_their_comment_in_date_order() {
    let mut strike = comment(4, "s", Some("a"), "D:3");
    strike.grouped = true;
    let comments = vec![
        comment(0, "a", None, "D:1"),
        comment(1, "late", Some("a"), "D:5"),
        comment(2, "early", Some("a"), "D:2"),
        comment(3, "lost", Some("gone"), "D:4"),
        strike,
    ];
    let (roots, children) = threads(&comments);
    // A reply whose comment isn't here stands alone; a grouped part
    // isn't listed at all.
    assert_eq!(roots, vec![0, 3]);
    assert_eq!(children["a"], vec![2, 1]);
}

#[test]
fn a_thread_shows_its_latest_status_and_checkmark() {
    let status = |i, model: &str, state: &str, date| {
        let mut n = comment(i, &format!("s{i}"), Some("a"), date);
        n.state = Some((model.into(), state.into()));
        n
    };
    let comments = vec![
        comment(0, "a", None, "D:1"),
        status(1, REVIEW, "Accepted", "D:2"),
        status(2, MARKED, "Marked", "D:3"),
        status(3, REVIEW, "Rejected", "D:4"),
    ];
    let (_, children) = threads(&comments);
    assert_eq!(
        thread_status(&comments, &children["a"]),
        (Some("Rejected".to_string()), true)
    );
    let comments = vec![
        comment(0, "a", None, "D:1"),
        status(1, REVIEW, "Accepted", "D:2"),
        status(2, REVIEW, "None", "D:3"),
        status(3, MARKED, "Marked", "D:4"),
        status(4, MARKED, "Unmarked", "D:5"),
    ];
    let (_, children) = threads(&comments);
    assert_eq!(thread_status(&comments, &children["a"]), (None, false));
}

#[test]
fn page_ranges_read_as_typed() {
    assert_eq!(parse_pages("3-10"), Some((2, 9)));
    assert_eq!(parse_pages(" 10 – 3 "), Some((2, 9)));
    assert_eq!(parse_pages("5"), Some((4, 4)));
    assert_eq!(parse_pages(""), None);
    assert_eq!(parse_pages("0"), None);
    assert_eq!(parse_pages("x-2"), None);
}

#[test]
fn the_filter_hides_by_each_facet() {
    let mut n = comment(0, "a", None, "D:1");
    n.kind = "Tickmark".into();
    n.color = Some([255, 0, 0]);
    let open = (None, false);
    let mut cf = CommentFilter::default();
    assert!(passes(&cf, &n, &open, 0));
    cf.summary = Some(Summary::Notes);
    assert!(!passes(&cf, &n, &open, 0));
    cf.summary = Some(Summary::Stamps);
    assert!(passes(&cf, &n, &open, 0));
    cf.colors.insert([255, 0, 0]);
    assert!(!passes(&cf, &n, &open, 0));
    cf.colors.clear();
    cf.authors.insert("Pat".into());
    assert!(!passes(&cf, &n, &open, 0));
    cf.authors.clear();
    cf.statuses.insert("None".into());
    assert!(!passes(&cf, &n, &open, 0));
    assert!(passes(&cf, &n, &(Some("Accepted".into()), false), 0));
    cf.statuses.clear();
    cf.checked = Some(true);
    assert!(!passes(&cf, &n, &open, 0));
    assert!(passes(&cf, &n, &(None, true), 0));
    cf.checked = None;
    cf.pages = Some((3, 5));
    assert!(!passes(&cf, &n, &open, 2) && passes(&cf, &n, &open, 4));
    assert_eq!(cf.describe(), "pages 4–6; Stamp Summary");
}

/// A file of `pages` blank pages opened, holding `marks`, sheaf's own.
fn doc(pages: u32, marks: HashMap<u32, Vec<Annot>>) -> Document {
    let store = Store::new();
    let primary = SourceInfo {
        name: "big.pdf".into(),
        ..SourceInfo::blank(&store, pages)
    };
    let mut d = sheaf_doc::open::start(store, primary, Box::new(FixedCount(10)));
    let facts = sheaf_doc::open::Facts {
        marks,
        ..Default::default()
    };
    d.ready(facts).unwrap();
    d
}

fn theirs(
    source: SourceId,
    page: u32,
    name: &str,
    parent: Option<&str>,
) -> (SourceId, ForeignAnnot) {
    (
        source,
        ForeignAnnot {
            page,
            subtype: "Text".into(),
            author: "Ann".into(),
            contents: format!("says {name}"),
            modified: "D:20261001120000".into(),
            rect: Box2 {
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 10.0,
            },
            name: name.into(),
            parent: parent.map(str::to_string),
            grouped: false,
            state: None,
            created: String::new(),
            slot: 0,
            hidden: false,
            color: Some([0, 0, 255]),
            mark: None,
        },
    )
}

/// The case Acrobat crawled on (acrobat.md §3.2): 1,000 comments over
/// 900 pages, filtered and sorted.
#[test]
fn a_thousand_comments_list_quickly() {
    let authors = ["Pat", "Greg", "Lee"];
    let mut ours: HashMap<u32, Vec<Annot>> = HashMap::new();
    for i in 0..600u32 {
        let mut a = mark(
            if i % 2 == 0 {
                AnnotKind::Note {
                    pos: Pt { x: 10.0, y: 10.0 },
                }
            } else {
                AnnotKind::Stamp {
                    rect: Box2 {
                        x: 10.0,
                        y: 10.0,
                        w: 20.0,
                        h: 20.0,
                    },
                    glyph: "✓".into(),
                }
            },
            &format!("comment {i}"),
        );
        a.id = AnnotId(i as u64 + 1);
        a.color = [255, 0, 0];
        a.author = authors[i as usize % 3].into();
        a.modified = format!("D:2026101{}000000", i % 10);
        ours.entry(i * 3 % 900).or_default().push(a);
    }
    let d = doc(900, ours);
    let source = d.state().pages[0].source;
    let foreign: Vec<(SourceId, ForeignAnnot)> = (0..400u32)
        .map(|i| {
            let mut f = theirs(source, i * 2 % 900, &format!("t{i}"), None);
            f.1.author = authors[i as usize % 3].into();
            f.1.contents = format!("theirs {i}");
            f
        })
        .collect();
    let started = std::time::Instant::now();
    let all = list(d.state(), &foreign, &CommentFilter::default(), "");
    assert_eq!(all.count, 1000);
    let mut cf = CommentFilter {
        sort: Sort::Author,
        ..Default::default()
    };
    cf.authors.insert("Lee".into());
    let some = list(d.state(), &foreign, &cf, "comment");
    let elapsed = started.elapsed();
    // Lee's are hidden on the page too; the rest are grouped by author.
    assert_eq!(some.filtered, 200 + 133);
    assert!(some.hidden_ours.len() == 200 && some.hidden_foreign.len() == 133);
    let headings: Vec<&str> = some.groups.iter().map(|g| g.label.as_str()).collect();
    assert_eq!(headings, ["Greg", "Pat"]);
    let summary = list(
        d.state(),
        &foreign,
        &CommentFilter {
            summary: Some(Summary::Stamps),
            ..Default::default()
        },
        "",
    );
    assert_eq!(summary.count, 300);
    assert!(
        elapsed < std::time::Duration::from_millis(250),
        "two lists took {elapsed:?}"
    );
}

/// Two threads on page 2: one with a reply that has its own reply and a
/// status, and one alone.
fn two_threads() -> Listing {
    let d = doc(3, HashMap::new());
    let source = d.state().pages[0].source;
    let mut status = theirs(source, 1, "st", Some("a"));
    status.1.state = Some((REVIEW.into(), "Accepted".into()));
    status.1.modified = "D:20261003".into();
    let mut reply = theirs(source, 1, "r", Some("a"));
    reply.1.contents = "Yes, \"quoted\", and, commas".into();
    reply.1.modified = "D:20261002".into();
    let mut deeper = theirs(source, 1, "rr", Some("r"));
    deeper.1.modified = "D:20261004".into();
    let foreign = vec![
        theirs(source, 1, "a", None),
        reply,
        status,
        deeper,
        theirs(source, 1, "b", None),
    ];
    list(d.state(), &foreign, &CommentFilter::default(), "")
}

#[test]
fn threads_list_depth_first_and_number_by_place() {
    let l = two_threads();
    assert_eq!(l.count, 2);
    let t = &l.groups[0].threads[0];
    let names: Vec<(&str, usize)> = t
        .entries
        .iter()
        .map(|e| (e.comment.name.as_str(), e.depth))
        .collect();
    assert_eq!(names, [("a", 0), ("r", 1), ("rr", 2), ("st", 1)]);
    assert_eq!(numbers(t, 4), ["4", "4.1", "4.1.1", "4.2"]);
    assert_eq!(t.status, (Some("Accepted".into()), false));
}

#[test]
fn the_csv_has_the_columns_asked_for_quoted() {
    let l = two_threads();
    let out = csv(
        &l,
        Some(&["i".into(), "ii".into(), "iii".into()]),
        &[
            Column::Number,
            Column::PageLabel,
            Column::Status,
            Column::Text,
            Column::ReplyTo,
        ],
    );
    let lines: Vec<&str> = out.split("\r\n").collect();
    assert_eq!(lines[0], "Number,Page label,Status,Text,Reply to");
    assert_eq!(lines[1], "1,ii,Accepted,says a,");
    assert_eq!(lines[2], "1.1,ii,,\"Yes, \"\"quoted\"\", and, commas\",1");
    assert_eq!(lines[3], "1.1.1,ii,,says rr,1.1");
    assert_eq!(lines[4], "1.2,ii,,Accepted,1");
    assert_eq!(lines[5], "2,ii,,says b,");
    assert_eq!(lines.len(), 7, "{out}");
}

#[test]
fn columns_read_as_typed() {
    assert_eq!(Column::from_name("page-label"), Some(Column::PageLabel));
    assert_eq!(Column::from_name("Reply to"), Some(Column::ReplyTo));
    assert_eq!(Column::from_name("nope"), None);
    assert_eq!(Sort::from_name("author"), Some(Sort::Author));
}

#[test]
fn the_text_export_reads_as_an_outline() {
    let l = two_threads();
    let out = text(&l, None, "report.pdf", Sort::Page);
    let expect = "report.pdf\n\
        2 comments, by page\n\
        \n\
        Page 2\n\
        \x20 1  Note · Ann · 2026-10-01 12:00 · Accepted\n\
        \x20    says a\n\
        \x20   1.1  Note · Ann · 2026-10-02\n\
        \x20        Yes, \"quoted\", and, commas\n\
        \x20     1.1.1  Note · Ann · 2026-10-04\n\
        \x20            says rr\n\
        \x20   1.2  Set to Accepted · Ann · 2026-10-03\n\
        \x20 2  Note · Ann · 2026-10-01 12:00\n\
        \x20    says b\n";
    assert_eq!(out, expect);
}
