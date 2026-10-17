//! A document's comments as the Comments panel lists them: threaded,
//! filtered and sorted. Written out as a summary PDF ([`summary_pdf`]),
//! CSV ([`csv`]) or text ([`text`]), the same list in the same order.
//!
//! A comment is one of sheaf's marks or another tool's annotation
//! ([`ForeignAnnot`]); replies and review statuses thread under the
//! comment they're on, by `/NM`.

mod summary;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use anyhow::Result;
use sheaf_doc::annot::{COLORS, MARKED};
use sheaf_doc::file::ForeignAnnot;
use sheaf_doc::{Annot, AnnotId, AnnotKind, Box2, DocState, Document, PageId, PageRef, SourceId};

pub use summary::{Layout, summary_pdf};

/// A comment: one of our marks, or another tool's (an index into the
/// document's list of them).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Item {
    Ours(PageId, AnnotId),
    Foreign(usize),
}

/// What the list shows. Each set lists what's hidden, so everything shows
/// until something is turned off.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CommentFilter {
    pub types: BTreeSet<String>,
    pub authors: BTreeSet<String>,
    /// Review states, "None" included.
    pub statuses: BTreeSet<String>,
    pub colors: BTreeSet<[u8; 3]>,
    /// Only checked (true) or only unchecked (false) ones.
    pub checked: Option<bool>,
    /// Display pages, from 0, inclusive.
    pub pages: Option<(usize, usize)>,
    /// PDFlyer's Stamp Summary or Note Summary.
    pub summary: Option<Summary>,
    pub sort: Sort,
}

impl CommentFilter {
    /// Whether it hides anything.
    pub fn is_active(&self) -> bool {
        !self.types.is_empty()
            || !self.authors.is_empty()
            || !self.statuses.is_empty()
            || !self.colors.is_empty()
            || self.checked.is_some()
            || self.pages.is_some()
            || self.summary.is_some()
    }

    /// What it hides, in a few words: "pages 3–10; Stamp Summary; 2
    /// authors hidden". Empty when it hides nothing.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some((a, b)) = self.pages {
            parts.push(if a == b {
                format!("page {}", a + 1)
            } else {
                format!("pages {}–{}", a + 1, b + 1)
            });
        }
        if let Some(s) = self.summary {
            parts.push(s.label().to_string());
        }
        match self.checked {
            Some(true) => parts.push("checked only".into()),
            Some(false) => parts.push("unchecked only".into()),
            None => {}
        }
        for (set, one, many) in [
            (self.types.len(), "type", "types"),
            (self.authors.len(), "author", "authors"),
            (self.statuses.len(), "status", "statuses"),
            (self.colors.len(), "color", "colors"),
        ] {
            if set > 0 {
                parts.push(format!(
                    "{set} {} hidden",
                    if set == 1 { one } else { many }
                ));
            }
        }
        parts.join("; ")
    }
}

/// PDFlyer's summaries, as filter presets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Summary {
    /// Tickmarks, stamps and connectors.
    Stamps,
    /// Notes and calculator tapes.
    Notes,
}

impl Summary {
    pub fn types(self) -> &'static [&'static str] {
        match self {
            Summary::Stamps => &["Tickmark", "Stamp", "Connector"],
            Summary::Notes => &["Note", "Calculator tape"],
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Summary::Stamps => "Stamp Summary",
            Summary::Notes => "Note Summary",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Page,
    Author,
    Date,
    Type,
    Color,
}

impl Sort {
    pub const ALL: [Sort; 5] = [
        Sort::Page,
        Sort::Author,
        Sort::Date,
        Sort::Type,
        Sort::Color,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Sort::Page => "Page",
            Sort::Author => "Author",
            Sort::Date => "Date",
            Sort::Type => "Type",
            Sort::Color => "Color",
        }
    }

    /// "page", "Author" … as typed.
    pub fn from_name(name: &str) -> Option<Sort> {
        Sort::ALL
            .into_iter()
            .find(|s| s.label().eq_ignore_ascii_case(name.trim()))
    }
}

/// A comment as the list knows it.
#[derive(Clone, Debug)]
pub struct Comment {
    pub item: Item,
    pub name: String,
    /// What it's in the thread of, by name.
    pub parent: Option<String>,
    /// Part of its parent (`/RT /Group`), not listed.
    pub grouped: bool,
    /// When it was made: its creation date, else its last change.
    pub date: String,
    /// When it last changed.
    pub modified: String,
    /// A status: (model, state).
    pub state: Option<(String, String)>,
    pub author: String,
    /// What it says: its text, or what a text edit suggests. Empty when
    /// it says nothing.
    pub body: String,
    /// What it is, in a word or two ("Tickmark ✓", "Highlight").
    pub label: String,
    /// Its type for the filter and sort, coarser than its label (every
    /// tickmark is a "Tickmark").
    pub kind: String,
    pub color: Option<[u8; 3]>,
    /// Where it is on its page, in PageSpace.
    pub rect: Box2,
    /// What a measurement comes to ("12.50 ft"), and its scale ("1 in =
    /// 10 ft").
    pub measurement: Option<(String, String)>,
}

impl Comment {
    /// What its row says: its text, else what it is.
    pub fn row_text(&self) -> &str {
        if self.body.is_empty() {
            &self.label
        } else {
            &self.body
        }
    }

    /// "(no author)" for none.
    pub fn author_label(&self) -> String {
        if self.author.is_empty() {
            "(no author)".into()
        } else {
            self.author.clone()
        }
    }

    fn matches(&self, needle: &str) -> bool {
        needle.is_empty()
            || self.body.to_lowercase().contains(needle)
            || self.label.to_lowercase().contains(needle)
            || self.author.to_lowercase().contains(needle)
    }
}

/// Every comment on the page `pref`: our marks, then the other tools'
/// that are on it (`foreign`, indexes into `foreign_all`), less those
/// deleted in sheaf and their threads, and those one of our marks has
/// taken the place of (made editable) with their grouped parts, which
/// come back second.
pub fn comments_on(
    st: &DocState,
    foreign_all: &[(SourceId, ForeignAnnot)],
    pref: &PageRef,
    foreign: &[usize],
) -> (Vec<Comment>, Vec<usize>) {
    let page = pref.id;
    let mut out = Vec::new();
    for a in st.annots_on(page).into_iter().flatten() {
        out.push(Comment {
            item: Item::Ours(page, a.id),
            name: a.name(),
            parent: a.thread_parent().map(str::to_string),
            grouped: false,
            date: if a.created.is_empty() {
                a.modified.clone()
            } else {
                a.created.clone()
            },
            modified: a.modified.clone(),
            state: match &a.kind {
                AnnotKind::Status { model, state, .. } => Some((model.clone(), state.clone())),
                _ => None,
            },
            author: a.author.clone(),
            body: ours_body(a),
            label: kind_label(&a.kind),
            kind: ours_type(&a.kind).to_string(),
            color: Some(a.color),
            rect: a.page_bounds(),
            measurement: measurement_of(&a.kind),
        });
    }
    let theirs: Vec<(usize, &ForeignAnnot)> = foreign
        .iter()
        .filter_map(|&i| foreign_all.get(i).map(|(_, f)| (i, f)))
        .collect();
    let mut gone = match st.removed_comments.get(&page) {
        Some(removed) => sheaf_doc::file::removed_with_threads(
            theirs
                .iter()
                .map(|(_, f)| (f.name.as_str(), f.parent.as_deref())),
            removed,
        ),
        None => HashSet::new(),
    };
    let ours: HashSet<&str> = out.iter().map(|c| c.name.as_str()).collect();
    for (_, f) in &theirs {
        let part_of_ours = f.grouped && f.parent.as_deref().is_some_and(|p| ours.contains(p));
        if ours.contains(f.name.as_str()) || part_of_ours {
            gone.insert(f.name.clone());
        }
    }
    let mut deleted = Vec::new();
    for (i, f) in theirs {
        if gone.contains(&f.name) {
            deleted.push(i);
            continue;
        }
        out.push(Comment {
            item: Item::Foreign(i),
            name: f.name.clone(),
            parent: f.parent.clone(),
            grouped: f.grouped,
            date: if f.created.is_empty() {
                f.modified.clone()
            } else {
                f.created.clone()
            },
            modified: f.modified.clone(),
            state: f.state.clone(),
            author: f.author.clone(),
            body: f.contents.replace('\n', " "),
            label: subtype_label(&f.subtype).to_string(),
            kind: f
                .mark
                .as_ref()
                .filter(|m| matches!(m.kind, AnnotKind::Measure { .. }))
                .map_or_else(
                    || subtype_label(&f.subtype).to_string(),
                    |m| ours_type(&m.kind).to_string(),
                ),
            color: f.color,
            rect: f.rect,
            measurement: f.mark.as_ref().and_then(|m| measurement_of(&m.kind)),
        });
    }
    (out, deleted)
}

/// A page's comments as threads: the top-level ones in order, and the
/// replies and statuses in each one's thread (by its name) in date order.
/// One whose parent isn't on the page is top-level; grouped parts are
/// left out.
pub fn threads(comments: &[Comment]) -> (Vec<usize>, HashMap<String, Vec<usize>>) {
    let names: HashSet<&str> = comments.iter().map(|n| n.name.as_str()).collect();
    let mut roots = Vec::new();
    let mut children: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, n) in comments.iter().enumerate() {
        match &n.parent {
            Some(p) if names.contains(p.as_str()) && n.grouped => {}
            Some(p) if names.contains(p.as_str()) => children.entry(p.clone()).or_default().push(i),
            _ => roots.push(i),
        }
    }
    for list in children.values_mut() {
        list.sort_by(|&a, &b| comments[a].date.cmp(&comments[b].date));
    }
    (roots, children)
}

/// The status a thread's statuses come to: the last review state, and
/// whether the last checkmark change checked it.
pub fn thread_status(comments: &[Comment], thread: &[usize]) -> (Option<String>, bool) {
    let mut review = None;
    let mut marked = false;
    for &i in thread {
        match &comments[i].state {
            Some((m, s)) if m == MARKED => marked = s == "Marked",
            Some((_, s)) => review = (s != "None").then(|| s.clone()),
            None => {}
        }
    }
    (review, marked)
}

/// The list: the threads shown, grouped and in order, and what the
/// filter needs to offer and hide.
#[derive(Debug, Default)]
pub struct Listing {
    pub groups: Vec<Group>,
    /// Each thread's status, by its comment, shown or not.
    pub statuses: HashMap<Item, (Option<String>, bool)>,
    /// Threads listed, and threads the filter hides.
    pub count: usize,
    pub filtered: usize,
    /// What there is to filter by.
    pub types: BTreeSet<String>,
    pub authors: BTreeSet<String>,
    pub colors: BTreeSet<[u8; 3]>,
    /// Comments the filter (or a deletion) hides on the page.
    pub hidden_ours: HashSet<AnnotId>,
    pub hidden_foreign: HashSet<usize>,
}

impl Listing {
    pub fn threads(&self) -> impl Iterator<Item = &Thread> {
        self.groups.iter().flat_map(|g| &g.threads)
    }
}

/// Threads under one heading: a page, an author, a day, a type or a
/// color, by the sort.
#[derive(Debug)]
pub struct Group {
    pub label: String,
    pub swatch: Option<[u8; 3]>,
    pub threads: Vec<Thread>,
    /// What it sorts by; threads with the same go under one heading.
    order: String,
}

/// A comment and its replies and statuses.
#[derive(Debug)]
pub struct Thread {
    /// Its display page, from 0.
    pub page: usize,
    pub page_id: PageId,
    /// (review state, checked).
    pub status: (Option<String>, bool),
    /// The comment first, then its thread, depth first.
    pub entries: Vec<Entry>,
}

#[derive(Debug)]
pub struct Entry {
    pub comment: Comment,
    /// 0 for the comment, 1 for a reply to it, and so on.
    pub depth: usize,
}

/// `st`'s comments as the panel lists them, filtered by `cf` and by
/// `needle` (lowercase; a thread shows if any of it matches).
pub fn list(
    st: &DocState,
    foreign_all: &[(SourceId, ForeignAnnot)],
    cf: &CommentFilter,
    needle: &str,
) -> Listing {
    let mut foreign_on: HashMap<(SourceId, u32), Vec<usize>> = HashMap::new();
    for (i, (source, f)) in foreign_all.iter().enumerate() {
        foreign_on.entry((*source, f.page)).or_default().push(i);
    }
    let mut out = Listing::default();
    // Each thread shown, and what it sorts by.
    let mut kept: Vec<(GroupKey, Thread)> = Vec::new();
    for (index, pref) in st.pages.iter().enumerate() {
        let foreign = foreign_on
            .get(&(pref.source, pref.index))
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let (comments, deleted) = comments_on(st, foreign_all, pref, foreign);
        out.hidden_foreign.extend(deleted);
        if comments.is_empty() {
            continue;
        }
        let (roots, children) = threads(&comments);
        for &r in &roots {
            let root = &comments[r];
            let mut order = Vec::new();
            walk(&children, &comments, r, 0, &mut order);
            out.types.insert(root.kind.clone());
            out.authors.insert(root.author_label());
            out.colors.extend(root.color);
            let status = thread_status(&comments, children.get(&root.name).map_or(&[][..], |v| v));
            out.statuses.insert(root.item, status.clone());
            if !passes(cf, root, &status, index) {
                // Hidden here and on the page, thread and all.
                out.filtered += 1;
                for &(n, _) in &order {
                    match comments[n].item {
                        Item::Ours(_, id) => {
                            out.hidden_ours.insert(id);
                        }
                        Item::Foreign(i) => {
                            out.hidden_foreign.insert(i);
                        }
                    }
                }
                continue;
            }
            if !order.iter().any(|&(n, _)| comments[n].matches(needle)) {
                continue;
            }
            out.count += 1;
            kept.push((
                group_key(cf.sort, root, index),
                Thread {
                    page: index,
                    page_id: pref.id,
                    status,
                    entries: order
                        .into_iter()
                        .map(|(n, depth)| Entry {
                            comment: comments[n].clone(),
                            depth,
                        })
                        .collect(),
                },
            ));
        }
    }
    kept.sort_by(|a, b| a.0.order.cmp(&b.0.order).then(a.1.page.cmp(&b.1.page)));
    for (key, thread) in kept {
        match out.groups.last_mut() {
            Some(g) if g.order == key.order => g.threads.push(thread),
            _ => out.groups.push(Group {
                label: key.label,
                swatch: key.swatch,
                order: key.order,
                threads: vec![thread],
            }),
        }
    }
    out
}

/// `n` and its thread below it, depth first.
fn walk(
    children: &HashMap<String, Vec<usize>>,
    comments: &[Comment],
    n: usize,
    depth: usize,
    out: &mut Vec<(usize, usize)>,
) {
    out.push((n, depth));
    if let Some(kids) = children.get(&comments[n].name) {
        for &k in kids {
            walk(children, comments, k, depth + 1, out);
        }
    }
}

/// What a thread is grouped and sorted under.
pub struct GroupKey {
    pub order: String,
    pub label: String,
    pub swatch: Option<[u8; 3]>,
}

pub fn group_key(sort: Sort, n: &Comment, page: usize) -> GroupKey {
    let simple = |order: String, label: String| GroupKey {
        order,
        label,
        swatch: None,
    };
    match sort {
        Sort::Page => simple(format!("{page:08}"), format!("Page {}", page + 1)),
        Sort::Author => {
            let a = n.author_label();
            simple(a.to_lowercase(), a)
        }
        Sort::Date => {
            let day = fmt_pdf_date(&n.date).chars().take(10).collect::<String>();
            let label = if day.is_empty() {
                "No date".into()
            } else {
                day
            };
            simple(n.date.chars().take(10).collect(), label)
        }
        Sort::Type => simple(n.kind.to_lowercase(), n.kind.clone()),
        Sort::Color => match n.color {
            Some(c) => GroupKey {
                order: format!("{:02x}{:02x}{:02x}", c[0], c[1], c[2]),
                label: color_name(c),
                swatch: Some(c),
            },
            None => simple("~".into(), "No color".into()),
        },
    }
}

/// A palette color's name, else its hex.
pub fn color_name(c: [u8; 3]) -> String {
    COLORS
        .iter()
        .find(|(k, _)| *k == c)
        .map(|(_, name)| name.to_string())
        .unwrap_or_else(|| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]))
}

/// Whether the filter shows a thread whose comment is `n`, with thread
/// status `status`, on display page `page`.
pub fn passes(
    cf: &CommentFilter,
    n: &Comment,
    status: &(Option<String>, bool),
    page: usize,
) -> bool {
    if let Some(sm) = cf.summary
        && !sm.types().contains(&n.kind.as_str())
    {
        return false;
    }
    if cf.types.contains(&n.kind) || cf.authors.contains(&n.author_label()) {
        return false;
    }
    if cf.statuses.contains(status.0.as_deref().unwrap_or("None")) {
        return false;
    }
    if n.color.is_some_and(|c| cf.colors.contains(&c)) {
        return false;
    }
    if cf.checked.is_some_and(|want| status.1 != want) {
        return false;
    }
    !cf.pages.is_some_and(|(a, b)| page < a || page > b)
}

/// "3-10" → pages 3 to 10 (from 0: 2 to 9); "5" → page 5; blank or
/// unreadable → all pages.
pub fn parse_pages(text: &str) -> Option<(usize, usize)> {
    let t = text.trim();
    let num = |s: &str| s.trim().parse::<usize>().ok().filter(|n| *n > 0);
    let (a, b) = match t.split_once(['-', '–']) {
        Some((a, b)) => (num(a)?, num(b)?),
        None => {
            let n = num(t)?;
            (n, n)
        }
    };
    Some((a.min(b) - 1, a.max(b) - 1))
}

/// "Accepted", "Checked", "Unchecked".
pub fn status_words(model: &str, state: &str) -> String {
    match (model, state) {
        (MARKED, "Marked") => "Checked".into(),
        (MARKED, _) => "Unchecked".into(),
        (_, s) => s.to_string(),
    }
}

/// A measurement's value and scale.
fn measurement_of(kind: &AnnotKind) -> Option<(String, String)> {
    match kind {
        AnnotKind::Measure {
            what,
            points,
            scale,
        } => Some((
            sheaf_doc::measure::label(*what, points, scale),
            scale.ratio(),
        )),
        _ => None,
    }
}

/// A mark's type for the filter and sort: coarser than its label (every
/// tickmark is a "Tickmark").
pub fn ours_type(kind: &AnnotKind) -> &'static str {
    match kind {
        AnnotKind::Highlight { .. } => "Highlight",
        AnnotKind::Underline { .. } => "Underline",
        AnnotKind::StrikeOut { .. } => "Strikethrough",
        AnnotKind::Squiggly { .. } => "Squiggly",
        AnnotKind::Replace { .. } => "Replace text",
        AnnotKind::Insert { .. } => "Insert text",
        AnnotKind::Note { .. } => "Note",
        AnnotKind::FreeText { .. } => "Text",
        AnnotKind::Square { .. } => "Rectangle",
        AnnotKind::Ellipse { .. } => "Ellipse",
        AnnotKind::Line { arrow: true, .. } => "Arrow",
        AnnotKind::Line { .. } => "Line",
        AnnotKind::Ink { .. } => "Pencil",
        AnnotKind::Stamp { .. } => "Tickmark",
        AnnotKind::Connector { .. } => "Connector",
        AnnotKind::Tape { .. } => "Calculator tape",
        AnnotKind::Reply { .. } => "Reply",
        AnnotKind::Status { .. } => "Status",
        AnnotKind::PageStamp { .. } | AnnotKind::TextStamp { .. } => "Stamp",
        AnnotKind::PageBorder { .. } => "Page border",
        AnnotKind::Polygon { .. } => "Polygon",
        AnnotKind::Polyline { .. } => "Polyline",
        AnnotKind::Callout { .. } => "Callout",
        AnnotKind::FileAttachment { .. } => "Attachment",
        AnnotKind::Measure { what, .. } => what.label(),
    }
}

/// Short label for one of our marks.
pub fn kind_label(kind: &AnnotKind) -> String {
    match kind {
        AnnotKind::Highlight { .. } => "Highlight".into(),
        AnnotKind::Note { .. } => "Note".into(),
        AnnotKind::FreeText { .. } => "Text".into(),
        AnnotKind::Square { .. } => "Rectangle".into(),
        AnnotKind::Ellipse { .. } => "Ellipse".into(),
        AnnotKind::Line { arrow: true, .. } => "Arrow".into(),
        AnnotKind::Line { .. } => "Line".into(),
        AnnotKind::Ink { .. } => "Pencil".into(),
        AnnotKind::Stamp { glyph, .. } => format!("Tickmark {glyph}"),
        AnnotKind::Connector { label, .. } => format!("Connector {label}"),
        AnnotKind::Tape { .. } => "Calculator tape".into(),
        AnnotKind::Underline { .. } => "Underline".into(),
        AnnotKind::StrikeOut { .. } => "Strikethrough".into(),
        AnnotKind::Squiggly { .. } => "Squiggly".into(),
        AnnotKind::Replace { .. } => "Replace text".into(),
        AnnotKind::Insert { .. } => "Insert text".into(),
        AnnotKind::Reply { .. } => "Reply".into(),
        AnnotKind::Status { state, .. } => state.clone(),
        AnnotKind::PageStamp { name, .. } => format!("Stamp {name}"),
        AnnotKind::TextStamp { text, .. } => {
            format!("Stamp {}", text.lines().next().unwrap_or_default())
        }
        AnnotKind::PageBorder { status, .. } => format!("{status} border"),
        AnnotKind::Polygon { .. } => "Polygon".into(),
        AnnotKind::Polyline { .. } => "Polyline".into(),
        AnnotKind::Callout { .. } => "Callout".into(),
        AnnotKind::FileAttachment { name, .. } => format!("Attachment {name}"),
        // Its value is what it says (`ours_body`).
        AnnotKind::Measure { what, .. } => what.label().into(),
    }
}

/// Acrobat's names for the common subtypes.
pub fn subtype_label(subtype: &str) -> &str {
    match subtype {
        "Text" => "Note",
        "FreeText" => "Text",
        "Square" => "Rectangle",
        "Circle" => "Ellipse",
        "Ink" => "Pencil",
        "StrikeOut" => "Strikethrough",
        other => other,
    }
}

/// `D:20261011153000Z` → `2026-10-11 15:30`.
pub fn fmt_pdf_date(d: &str) -> String {
    let s = d.strip_prefix("D:").unwrap_or(d);
    let digit = |i: usize| s.as_bytes().get(i).filter(|b| b.is_ascii_digit()).is_some();
    if !(0..8).all(digit) {
        return String::new();
    }
    let mut out = format!("{}-{}-{}", &s[0..4], &s[4..6], &s[6..8]);
    if (8..12).all(digit) {
        out.push_str(&format!(" {}:{}", &s[8..10], &s[10..12]));
    }
    out
}

/// What one of our marks says, on one line: its text, or what a text
/// edit suggests. Empty when it says nothing.
fn ours_body(a: &Annot) -> String {
    let one_line = |s: &str| s.replace('\n', " ");
    match &a.kind {
        AnnotKind::Replace { struck, .. } if a.contents.is_empty() => {
            format!("Cross out “{}”", one_line(struck))
        }
        AnnotKind::Replace { struck, .. } => format!(
            "Replace “{}” with “{}”",
            one_line(struck),
            one_line(&a.contents)
        ),
        AnnotKind::Insert { .. } => format!("Insert “{}”", one_line(&a.contents)),
        AnnotKind::Measure {
            what,
            points,
            scale,
        } => {
            let value = sheaf_doc::measure::label(*what, points, scale);
            if a.contents.is_empty() {
                value
            } else {
                format!("{value} — {}", one_line(&a.contents))
            }
        }
        _ => one_line(&a.contents),
    }
}

/// A mark's text as its row shows it. Text edits read as what they
/// suggest.
pub fn ours_text(a: &Annot) -> String {
    let body = ours_body(a);
    if body.is_empty() {
        kind_label(&a.kind)
    } else {
        body
    }
}

/// Another tool's comment's text as its row shows it.
pub fn foreign_text(f: &ForeignAnnot) -> String {
    if f.contents.is_empty() {
        subtype_label(&f.subtype).to_string()
    } else {
        f.contents.replace('\n', " ")
    }
}

/// The numbers a thread's entries go by: the comment's `n`, its replies
/// `n.1`, `n.2`, theirs `n.1.1`.
pub fn numbers(thread: &Thread, n: usize) -> Vec<String> {
    let mut path: Vec<usize> = Vec::new();
    thread
        .entries
        .iter()
        .map(|e| {
            if e.depth == 0 {
                path.clear();
                return n.to_string();
            }
            path.truncate(e.depth);
            path.resize(e.depth, 0);
            path[e.depth - 1] += 1;
            std::iter::once(n.to_string())
                .chain(path.iter().map(usize::to_string))
                .collect::<Vec<_>>()
                .join(".")
        })
        .collect()
}

/// A document from a file, with the other tools' comments in it: what
/// [`list`] needs.
pub fn open(
    path: &Path,
    password: Option<&str>,
) -> Result<(Document, Vec<(SourceId, ForeignAnnot)>)> {
    let opened = sheaf_doc::open::file(path, password)?;
    let primary = opened.doc.primary();
    let foreign = opened.view.foreign.into_iter();
    Ok((opened.doc, foreign.map(|f| (primary, f)).collect()))
}

/// A comments data file's format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataFormat {
    Xfdf,
    Fdf,
}

impl DataFormat {
    pub fn extension(self) -> &'static str {
        match self {
            DataFormat::Xfdf => "xfdf",
            DataFormat::Fdf => "fdf",
        }
    }

    /// By a file's extension.
    pub fn of(path: &Path) -> Option<DataFormat> {
        match path.extension()?.to_str()?.to_lowercase().as_str() {
            "xfdf" | "xml" => Some(DataFormat::Xfdf),
            "fdf" => Some(DataFormat::Fdf),
            _ => None,
        }
    }
}

/// `items` (a listing's, or a selection) as a data file: each with its
/// thread, as the listing has them.
pub fn export_data(
    snap: &sheaf_doc::Snapshot,
    foreign: &[(SourceId, ForeignAnnot)],
    items: &[Item],
    format: DataFormat,
) -> Result<Vec<u8>> {
    let ours: Vec<(PageId, AnnotId)> = items
        .iter()
        .filter_map(|i| match *i {
            Item::Ours(p, a) => Some((p, a)),
            Item::Foreign(_) => None,
        })
        .collect();
    let theirs: Vec<usize> = items
        .iter()
        .filter_map(|i| match *i {
            Item::Foreign(f) => Some(f),
            Item::Ours(..) => None,
        })
        .collect();
    let comments = sheaf_doc::exchange::gather(snap, foreign, &ours, &theirs)?;
    match format {
        DataFormat::Xfdf => sheaf_doc::exchange::xfdf::write(&comments),
        DataFormat::Fdf => sheaf_doc::exchange::fdf::write(&comments),
    }
}

/// Every comment a listing shows, replies and statuses included.
pub fn items(listing: &Listing) -> Vec<Item> {
    listing
        .threads()
        .flat_map(|t| t.entries.iter().map(|e| e.comment.item))
        .collect()
}

/// The comments in an FDF, XFDF or PDF file (`bytes`) as the edits that
/// bring them into `snap`, matched by `/NM` (see
/// `sheaf_doc::exchange::import`). `password` opens a PDF.
pub fn import_data(
    snap: &sheaf_doc::Snapshot,
    foreign: &[(SourceId, ForeignAnnot)],
    bytes: &[u8],
    password: Option<&str>,
) -> Result<sheaf_doc::exchange::Imported> {
    let comments = sheaf_doc::exchange::read(bytes, password)?;
    Ok(sheaf_doc::exchange::import(snap, foreign, &comments))
}

/// A column of the CSV export.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Column {
    /// "3", or "3.1" for a reply.
    Number,
    /// The page's number, from 1.
    Page,
    /// The page's label, else its number.
    PageLabel,
    Type,
    Label,
    Author,
    /// When it was made.
    Date,
    Modified,
    /// The thread's review state.
    Status,
    Checked,
    Color,
    Text,
    /// The number of the comment it replies to.
    ReplyTo,
    /// What a measurement comes to.
    Measurement,
    /// A measurement's scale.
    Scale,
}

impl Column {
    pub const ALL: [Column; 15] = [
        Column::Number,
        Column::Page,
        Column::PageLabel,
        Column::Type,
        Column::Label,
        Column::Author,
        Column::Date,
        Column::Modified,
        Column::Status,
        Column::Checked,
        Column::Color,
        Column::Text,
        Column::ReplyTo,
        Column::Measurement,
        Column::Scale,
    ];

    /// What a CSV has with no columns chosen.
    pub const DEFAULT: [Column; 7] = [
        Column::Number,
        Column::Page,
        Column::Type,
        Column::Author,
        Column::Date,
        Column::Status,
        Column::Text,
    ];

    /// Its heading, and its name on the command line.
    pub fn name(self) -> &'static str {
        match self {
            Column::Number => "Number",
            Column::Page => "Page",
            Column::PageLabel => "Page label",
            Column::Type => "Type",
            Column::Label => "Label",
            Column::Author => "Author",
            Column::Date => "Date",
            Column::Modified => "Modified",
            Column::Status => "Status",
            Column::Checked => "Checked",
            Column::Color => "Color",
            Column::Text => "Text",
            Column::ReplyTo => "Reply to",
            Column::Measurement => "Measurement",
            Column::Scale => "Scale",
        }
    }

    /// "page-label", "Page label" or "pagelabel".
    pub fn from_name(name: &str) -> Option<Column> {
        let squash = |s: &str| {
            s.chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>()
                .to_lowercase()
        };
        let want = squash(name);
        Column::ALL.into_iter().find(|c| squash(c.name()) == want)
    }
}

/// One line of the export per entry, in list order.
struct Line<'a> {
    number: String,
    reply_to: String,
    page: usize,
    thread: &'a Thread,
    entry: &'a Entry,
}

fn lines(listing: &Listing) -> Vec<Line<'_>> {
    let mut out = Vec::new();
    for (i, thread) in listing.threads().enumerate() {
        let numbers = numbers(thread, i + 1);
        let mut at_depth: Vec<&str> = Vec::new();
        for (entry, number) in thread.entries.iter().zip(&numbers) {
            at_depth.truncate(entry.depth);
            let reply_to = at_depth.last().copied().unwrap_or_default().to_string();
            at_depth.push(number);
            out.push(Line {
                number: number.clone(),
                reply_to,
                page: thread.page,
                thread,
                entry,
            });
        }
    }
    out
}

/// An entry's text in an export: what it says, else what it is; a status,
/// in words.
fn entry_text(c: &Comment) -> String {
    match &c.state {
        Some((model, state)) => status_words(model, state),
        None => c.row_text().to_string(),
    }
}

/// The list as CSV (RFC 4180, CRLF), one row per comment, reply and
/// status, with a heading row. `labels` are the page labels, if any.
pub fn csv(listing: &Listing, labels: Option<&[String]>, columns: &[Column]) -> String {
    let field = |s: &str| {
        if s.contains([',', '"', '\n', '\r']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_string()
        }
    };
    let mut out = String::new();
    let row = |cells: Vec<String>, out: &mut String| {
        out.push_str(&cells.iter().map(|c| field(c)).collect::<Vec<_>>().join(","));
        out.push_str("\r\n");
    };
    row(
        columns.iter().map(|c| c.name().to_string()).collect(),
        &mut out,
    );
    for l in lines(listing) {
        let c = &l.entry.comment;
        let root = l.entry.depth == 0;
        row(
            columns
                .iter()
                .map(|col| match col {
                    Column::Number => l.number.clone(),
                    Column::Page => (l.page + 1).to_string(),
                    Column::PageLabel => page_name(labels, l.page),
                    Column::Type => c.kind.clone(),
                    Column::Label => c.label.clone(),
                    Column::Author => c.author.clone(),
                    Column::Date => fmt_pdf_date(&c.date),
                    Column::Modified => fmt_pdf_date(&c.modified),
                    Column::Status if root => l.thread.status.0.clone().unwrap_or_default(),
                    Column::Checked if root => if l.thread.status.1 { "Yes" } else { "No" }.into(),
                    Column::Status | Column::Checked => String::new(),
                    Column::Color => c
                        .color
                        .map(|c| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]))
                        .unwrap_or_default(),
                    Column::Text => entry_text(c),
                    Column::ReplyTo => l.reply_to.clone(),
                    Column::Measurement => c
                        .measurement
                        .as_ref()
                        .map(|m| m.0.clone())
                        .unwrap_or_default(),
                    Column::Scale => c
                        .measurement
                        .as_ref()
                        .map(|m| m.1.clone())
                        .unwrap_or_default(),
                })
                .collect(),
            &mut out,
        );
    }
    out
}

/// A page's label, else its number.
fn page_name(labels: Option<&[String]>, page: usize) -> String {
    labels
        .and_then(|l| l.get(page))
        .filter(|l| !l.is_empty())
        .cloned()
        .unwrap_or_else(|| (page + 1).to_string())
}

/// The list as plain text, as PDFlyer's summary export reads: a heading
/// per group, each comment numbered with what it is, who and when, then
/// what it says; replies indented under it.
pub fn text(listing: &Listing, labels: Option<&[String]>, title: &str, sort: Sort) -> String {
    let mut out = format!("{title}\n");
    out.push_str(&format!(
        "{} comment{}, by {}\n",
        listing.count,
        if listing.count == 1 { "" } else { "s" },
        sort.label().to_lowercase()
    ));
    let mut n = 0;
    for g in &listing.groups {
        out.push_str(&format!("\n{}\n", g.label));
        for thread in &g.threads {
            n += 1;
            for (e, number) in thread.entries.iter().zip(numbers(thread, n)) {
                let pad = "  ".repeat(e.depth + 1);
                out.push_str(&format!(
                    "{pad}{number}  {}\n",
                    meta(e, thread, labels, sort)
                ));
                let text = entry_text(&e.comment);
                if e.comment.state.is_none() && !e.comment.body.is_empty() {
                    let indent = " ".repeat(pad.len() + number.len() + 2);
                    out.push_str(&format!("{indent}{text}\n"));
                }
            }
        }
    }
    out
}

/// "Note · Ann · 2026-10-11 15:30 · Page 3 · Accepted · Checked": what an
/// entry is, who made it and when; its page when the list isn't by page;
/// the thread's status on the comment itself.
fn meta(e: &Entry, thread: &Thread, labels: Option<&[String]>, sort: Sort) -> String {
    let c = &e.comment;
    let what = match &c.state {
        Some((model, state)) => format!("Set to {}", status_words(model, state)),
        None => c.label.clone(),
    };
    let mut parts = vec![what, c.author_label()];
    let date = fmt_pdf_date(&c.date);
    if !date.is_empty() {
        parts.push(date);
    }
    if e.depth == 0 {
        if sort != Sort::Page {
            parts.push(format!("Page {}", page_name(labels, thread.page)));
        }
        if let Some(s) = &thread.status.0 {
            parts.push(s.clone());
        }
        if thread.status.1 {
            parts.push("Checked".into());
        }
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests;
