//! The content toolkit over Greg's corpus of real forms
//! ([`forms::corpus`](crate::forms::corpus)): every page's content, and
//! every form XObject it draws, reads, lexes into tokens that tile it and
//! splices back to each stream's own bytes, read apart; its marked content
//! balances; its `q`/`Q` balance is what lopdf's `Content::decode` counts;
//! its words decode through its fonts; its pagination artifacts are
//! classified as the tools that made the files drew them; and each of
//! them splices out, as a strip cuts it, or is refused, Acrobat's never.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;
use std::time::Instant;

use lopdf::{Object, ObjectId};

use super::tests::blank;
use super::*;
use crate::forms::corpus::{Corpus, Entry, each_entry};
use crate::source::Limits;
use crate::stamps;
use crate::store::{BlobKind, Store};

const LIM: &Limits = &Limits::DEFAULT;

/// What one file's content came to.
#[derive(Default)]
struct Outcome {
    pages: usize,
    forms: usize,
    bytes: usize,
    /// Content that can't be read or lexed, or whose tokens don't tile
    /// it, with its page: each a line.
    unread: Vec<String>,
    /// Pages whose marked content doesn't balance.
    unbalanced: Vec<String>,
    /// Streams the concatenation, or a splice of nothing, changed from
    /// their own bytes.
    changed: Vec<String>,
    /// Pages lopdf counts `q` and `Q` on (no inline images, and decoded),
    /// and those whose count differs.
    counted: usize,
    miscounted: Vec<String>,
    origins: BTreeMap<(PaginationKind, PagOrigin), usize>,
    /// Pagination artifacts spliced out alone, as a strip cuts them, by
    /// who drew them and whether the splice was made; Acrobat's refused,
    /// and pages whose strip of a kind (`stamps::stripped`) is refused.
    splices: BTreeMap<(PagOrigin, bool), usize>,
    acrobat_refused: Vec<String>,
    unstrippable: Vec<String>,
    nested: usize,
    /// The words pages draw, decoded through their fonts.
    words: usize,
    /// The marked ranges blocks keep, and the most on a page.
    kept: usize,
    most_kept: usize,
    /// The kinds the file's bytes say Acrobat drew, those classified as
    /// Acrobat's, and those found at all.
    acrobat_said: BTreeSet<PaginationKind>,
    acrobat_found: BTreeSet<PaginationKind>,
    kinds_found: BTreeSet<PaginationKind>,
    /// Whether Word or LibreOffice made it.
    office: bool,
}

/// The pagination kinds `bytes` say Acrobat drew: `/ADBE_CompoundType`
/// dictionaries' `/Private` names, found by a scan.
fn acrobat_said(bytes: &[u8]) -> BTreeSet<PaginationKind> {
    let mut said = BTreeSet::new();
    let mut at = 0;
    while let Some(i) = find(&bytes[at..], b"ADBE_CompoundType") {
        at += i + 1;
        let near = &bytes[at..(at + 300).min(bytes.len())];
        if let Some(p) = find(near, b"/Private") {
            let name = near[p + 8..]
                .iter()
                .skip_while(|c| c.is_ascii_whitespace())
                .skip(1)
                .take_while(|c| c.is_ascii_alphanumeric())
                .copied()
                .collect::<Vec<_>>();
            said.extend(PaginationKind::from_name(&name));
        }
    }
    said
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Whether Word or LibreOffice made `src`, by its Info's `/Producer` or
/// `/Creator`, or its XMP.
fn office(src: &crate::store::Src) -> bool {
    let pdf = src.pdf();
    let made_by = |s: &[u8]| {
        let s = String::from_utf8_lossy(s);
        [
            "Microsoft® Word",
            "Microsoft Word",
            "LibreOffice",
            "OpenOffice",
        ]
        .iter()
        .any(|m| s.contains(m))
    };
    let info = pdf
        .trailer()
        .get(b"Info")
        .ok()
        .and_then(|i| pdf.dereference(i).ok())
        .and_then(|(_, i)| i.as_dict().ok());
    let in_info = info.is_some_and(|info| {
        [b"Producer".as_slice(), b"Creator"].iter().any(|key| {
            info.get(key)
                .ok()
                .and_then(|v| v.as_str().ok())
                .is_some_and(|v| made_by(&crate::file::text_string(v).into_bytes()))
        })
    });
    let in_xmp = || {
        let catalog = pdf.catalog().ok()?;
        let (_, xmp) = pdf.dereference(catalog.get(b"Metadata").ok()?).ok()?;
        let xmp = xmp.as_stream().ok()?;
        Some(made_by(&xmp.decompressed_content().ok()?))
    };
    in_info || in_xmp() == Some(true)
}

/// XObject `name` of `resources`.
fn xobject(src: &crate::store::Src, resources: Option<&Object>, name: &[u8]) -> Option<ObjectId> {
    let pdf = src.pdf();
    let (_, resources) = pdf.dereference(resources?).ok()?;
    let (_, xobjects) = pdf
        .dereference(resources.as_dict().ok()?.get(b"XObject").ok()?)
        .ok()?;
    xobjects.as_dict().ok()?.get(name).ok()?.as_reference().ok()
}

/// Where `s`'s tokens, lexed with `res`, don't tile it: an error, a
/// token out of order, or something between or after them that isn't
/// white-space or a comment.
fn untiled(s: &[u8], res: &ResCx) -> Option<String> {
    let mut at = 0;
    for token in lex(s, res, LIM) {
        let r = match token {
            Ok((r, _)) => r,
            Err(e) => return Some(e.to_string()),
        };
        if !(at <= r.start && r.start < r.end) || !blank(&s[at..r.start]) {
            return Some(format!("bytes {at} to {}", r.start));
        }
        at = r.end;
    }
    (!blank(&s[at..])).then(|| format!("bytes after {at}"))
}

/// lopdf's count of `bytes`' `q`s and `Q`s, the reference.
fn decoded_balance(bytes: &[u8]) -> Option<Balance> {
    let content = lopdf::content::Content::decode(bytes).ok()?;
    let mut balance = Balance::default();
    for op in &content.operations {
        balance.take(op.operator.as_bytes());
    }
    Some(balance)
}

fn read_file(corpus: &Corpus, entry: &Entry) -> Outcome {
    let mut out = Outcome::default();
    let path = corpus.path(entry);
    let Ok(bytes) = std::fs::read(&path) else {
        return out;
    };
    out.acrobat_said = acrobat_said(&bytes);
    let kind = BlobKind::file(Path::new(&entry.path), None);
    let Ok(src) = Store::new().open(bytes.into(), kind) else {
        return out;
    };
    out.office = office(&src);
    let mut forms_seen = HashSet::new();
    let known = DrawnForms::default();
    for (i, page) in src.pages().iter().enumerate() {
        out.pages += 1;
        let at = format!("{} page {}", entry.path, i + 1);
        let content = match PageContent::read(src.pdf(), &page.contents, LIM) {
            Ok(content) => content,
            Err(fault) => {
                out.unread.push(format!("{at}: {fault}"));
                continue;
            }
        };
        let resources = page.resources.as_deref();
        let res = ResCx::read(src.pdf(), resources);
        let concat = content.concat();
        out.bytes += concat.bytes().len();
        if let Some(wrong) = untiled(concat.bytes(), &res) {
            out.unread.push(format!("{at}: tokens don't tile {wrong}"));
            continue;
        }
        // Each stream's own bytes, read apart from the concatenation.
        let own: Vec<Vec<u8>> = content
            .ids()
            .iter()
            .map(|&id| {
                let stream = src.pdf().get_object(id).unwrap().as_stream().unwrap();
                stream.decompressed_content().unwrap()
            })
            .collect();
        match splice(concat, &res, LIM, &[]) {
            Ok(streams) => {
                for (n, (stream, own)) in streams.iter().zip(&own).enumerate() {
                    if **stream != **own || concat.stream(n) != own {
                        out.changed.push(format!("{at}, stream {n}"));
                    }
                }
            }
            Err(e) => {
                out.unread
                    .push(format!("{at}: {}", content.fault(lex_error(e))));
                continue;
            }
        }
        let lexed: Vec<_> = concat.lex(&res, LIM).map(Result::unwrap).collect();
        let has_image = lexed.iter().any(|(_, t)| matches!(t, Tok::InlineImage(_)));
        let ours = balance(concat, &res, LIM).unwrap();
        if !has_image && let Some(reference) = decoded_balance(concat.bytes()) {
            out.counted += 1;
            if ours != reference {
                out.miscounted
                    .push(format!("{at}: {ours:?}, lopdf {reference:?}"));
            }
        }
        let fonts = FontDecoders::read(src.pdf(), resources, LIM);
        out.words += text_bag(concat, &res, LIM, &fonts).unwrap().len();
        // The forms it draws, each once a file.
        let names = names_used(concat, &res, LIM, &[]).unwrap();
        for name in names.of(ResKind::XObject) {
            let Some(id) = xobject(&src, resources, name) else {
                continue;
            };
            if !forms_seen.insert(id) {
                continue;
            }
            let form = src
                .pdf()
                .get_object(id)
                .ok()
                .and_then(|o| o.as_stream().ok());
            let Some(form) = form
                .filter(|f| f.dict.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Form"))
            else {
                continue;
            };
            out.forms += 1;
            let at = format!("{at}, form {} {} R", id.0, id.1);
            let Ok(data) = form.decompressed_content_with_limit(LIM.content.decompressed) else {
                out.unread.push(format!("{at}: won't decode"));
                continue;
            };
            let own = form.dict.get(b"Resources").ok();
            let res = ResCx::read(src.pdf(), own.or(resources));
            if let Some(wrong) = untiled(&data, &res) {
                out.unread.push(format!("{at}: tokens don't tile {wrong}"));
                continue;
            }
            let one = Concat::new(&[&data]);
            match splice(&one, &res, LIM, &[]) {
                Ok(streams) if *streams[0] == *data => {}
                Ok(_) => out.changed.push(at),
                Err(e) => out.unread.push(format!("{at}: {e}")),
            }
        }
        match PageBlocks::of(src.pdf(), &content, resources, LIM, &known) {
            Ok(blocks) => {
                for p in blocks.pagination.iter() {
                    *out.origins.entry((p.kind, p.origin)).or_default() += 1;
                    out.kinds_found.insert(p.kind);
                    if p.origin == PagOrigin::Acrobat {
                        out.acrobat_found.insert(p.kind);
                    }
                    let cuts = p.cuts(concat, &res, LIM).unwrap();
                    let made = splice(concat, &res, LIM, &cuts).is_ok();
                    *out.splices.entry((p.origin, made)).or_default() += 1;
                    if p.origin == PagOrigin::Acrobat && !made {
                        out.acrobat_refused.push(format!("{at}: {:?}", p.kind));
                    }
                }
                for kinds in [stamps::HEADER_FOOTER, stamps::WATERMARK, stamps::BACKGROUND] {
                    let cuts: Vec<SpanAcross> = stamps::stripped(&blocks, kinds)
                        .flat_map(|p| p.cuts(concat, &res, LIM).unwrap())
                        .collect();
                    if let Err(e) = splice(concat, &res, LIM, &cuts) {
                        out.unstrippable.push(format!("{at}, {kinds:?}: {e}"));
                    }
                }
                out.nested += usize::from(blocks.nested_in_forms);
                out.kept += blocks.marked.len();
                out.most_kept = out.most_kept.max(blocks.marked.len());
            }
            Err(fault) => out.unbalanced.push(format!("{at}: {fault}")),
        }
    }
    out
}

fn lex_error(e: SpliceError) -> LexError {
    match e {
        SpliceError::Lex(e) => e,
        e => panic!("a splice of nothing: {e}"),
    }
}

/// Every page of every form in the corpus, its content and the forms it
/// draws: read, lexed, spliced with nothing taken out, counted,
/// classified, and spliced without each pagination artifact.
#[test]
fn corpus_content_lexes_splices_and_classifies() {
    let Some(corpus) = Corpus::or_skip() else {
        return;
    };
    let entries = corpus.manifest().unwrap();
    let started = Instant::now();
    let outcomes = each_entry(&entries, |entry| read_file(&corpus, entry));
    let took = started.elapsed();
    let sum = |f: fn(&Outcome) -> usize| outcomes.iter().map(f).sum::<usize>();
    let all = |f: fn(&Outcome) -> &Vec<String>| outcomes.iter().flat_map(f).collect::<Vec<_>>();
    let (unread, unbalanced) = (all(|o| &o.unread), all(|o| &o.unbalanced));
    let (changed, miscounted) = (all(|o| &o.changed), all(|o| &o.miscounted));
    let (refused, unstrippable) = (all(|o| &o.acrobat_refused), all(|o| &o.unstrippable));
    let mut origins: BTreeMap<(PaginationKind, PagOrigin), usize> = BTreeMap::new();
    let mut splices: BTreeMap<(PagOrigin, bool), usize> = BTreeMap::new();
    for o in &outcomes {
        for (key, n) in &o.origins {
            *origins.entry(*key).or_default() += n;
        }
        for (key, n) in &o.splices {
            *splices.entry(*key).or_default() += n;
        }
    }
    eprintln!(
        "{} pages ({} MB of content) and {} forms in {} files, in {took:?}: \
         {} unread, {} with marked content unbalanced, {} lopdf counted, {} nested in forms",
        sum(|o| o.pages),
        sum(|o| o.bytes) >> 20,
        sum(|o| o.forms),
        entries.len(),
        unread.len(),
        unbalanced.len(),
        sum(|o| o.counted),
        sum(|o| o.nested),
    );
    eprintln!(
        "  {} words drawn; {} marked ranges kept, at most {} a page",
        sum(|o| o.words),
        sum(|o| o.kept),
        outcomes.iter().map(|o| o.most_kept).max().unwrap_or(0)
    );
    for line in unread.iter().chain(&unbalanced).take(20) {
        eprintln!("  {line}");
    }
    for ((kind, origin), n) in &origins {
        eprintln!("  {kind:?} by {origin:?}: {n}");
    }
    for origin in [
        PagOrigin::Sheaf,
        PagOrigin::SheafLegacy,
        PagOrigin::Acrobat,
        PagOrigin::Authored,
    ] {
        let n = |made| splices.get(&(origin, made)).copied().unwrap_or(0);
        eprintln!(
            "  {origin:?}'s spliced out alone: {} made, {} refused",
            n(true),
            n(false)
        );
    }
    assert!(unread.is_empty(), "{unread:#?}");
    assert!(unbalanced.is_empty(), "{unbalanced:#?}");
    assert!(changed.is_empty(), "{changed:#?}");
    assert!(miscounted.is_empty(), "{miscounted:#?}");
    // One page of Acrobat's watermark has the body's `q` and `BT` inside
    // its range, which no splice can take without the body.
    let opens_the_body = "forms/safedocs/0112530.pdf page 1";
    assert_eq!(refused, [&format!("{opens_the_body}: Watermark")]);
    assert_eq!(unstrippable.len(), 1, "{unstrippable:#?}");
    assert!(unstrippable[0].starts_with(&format!("{opens_the_body}, [Watermark]")));

    // Acrobat's files have Acrobat's artifacts of the kinds they say, but
    // where its forms are drawn unmarked, which is no artifact; Word's and
    // LibreOffice's have their authors' only.
    let (mut missed, mut unmarked) = (Vec::new(), 0);
    let mut office = (0, 0);
    for (entry, o) in entries.iter().zip(&outcomes) {
        let said: BTreeSet<_> = o.acrobat_said.difference(&o.acrobat_found).collect();
        if said.iter().any(|kind| o.kinds_found.contains(kind)) {
            missed.push(format!(
                "{}: says {:?}, found {:?}",
                entry.path, o.acrobat_said, o.origins
            ));
        } else if !said.is_empty() {
            unmarked += 1;
        }
        if o.office && o.acrobat_said.is_empty() && !o.origins.is_empty() {
            office.0 += 1;
            office.1 += o.origins.values().sum::<usize>();
            let not_authored: Vec<_> = o
                .origins
                .keys()
                .filter(|(_, origin)| *origin != PagOrigin::Authored)
                .collect();
            assert!(not_authored.is_empty(), "{}: {not_authored:?}", entry.path);
        }
    }
    let acrobat = outcomes
        .iter()
        .filter(|o| !o.acrobat_said.is_empty())
        .count();
    eprintln!(
        "{acrobat} files Acrobat drew in: {} as said, {unmarked} its forms unmarked; \
         {} Word or LibreOffice files mark {} artifacts, all their authors'",
        acrobat - missed.len() - unmarked,
        office.0,
        office.1
    );
    assert!(missed.is_empty(), "{missed:#?}");
}
