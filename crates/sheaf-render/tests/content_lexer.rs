//! The content lexer against PDFium (M10.5 §13): over the planning pool
//! (`bench/pool-sample.txt`, in the corpus), each page's top-level
//! painting operators, by kind, against the page objects PDFium makes of
//! the same content, by type. A lexer that lost its place, in an inline
//! image or a string, shows as a page whose counts differ, or as a page
//! it can't read that PDFium reads.
//!
//! The lexer side counts what PDFium makes an object of: a `Tj`, `'` or
//! `"` of a string that isn't empty, or a `TJ` with a string, once a font
//! is set; a path painted with two points or more, as PDFium keeps them,
//! or a closed point stroked with round caps; an inline image, or an
//! image XObject drawn; a form XObject drawn; and a shading the resources
//! have, painted. Pages whose counts differ are listed, and fail it: on
//! the pool, at slice 6, none did, over 23,628 pages.
//!
//! Ignored: it reads every page of 402 files with PDFium. It binds PDFium
//! itself, since it reads page objects, which the engine doesn't show
//! (`scripts/fetch-pdfium.sh`):
//!
//! ```sh
//! cargo test -p sheaf-render --test content_lexer -- --ignored --nocapture
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use lopdf::{Dictionary, LazyPdf, Object, ObjectId};
use pdfium_render::prelude::*;
use sheaf_doc::content::{PageContent, ResCx, Tok};
use sheaf_doc::forms::corpus::{Corpus, Entry, each_entry};
use sheaf_doc::source::{Limits, PageFacts};
use sheaf_doc::store::{BlobKind, Store};

/// Top-level objects by type.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Kinds {
    text: u32,
    path: u32,
    image: u32,
    form: u32,
    shading: u32,
}

impl Kinds {
    fn add(&mut self, other: Kinds) {
        self.text += other.text;
        self.path += other.path;
        self.image += other.image;
        self.form += other.form;
        self.shading += other.shading;
    }

    /// The types whose counts differ from `other`'s, with both counts.
    fn differences(&self, other: &Kinds) -> Vec<(&'static str, u32, u32)> {
        [
            ("text", self.text, other.text),
            ("path", self.path, other.path),
            ("image", self.image, other.image),
            ("form", self.form, other.form),
            ("shading", self.shading, other.shading),
        ]
        .into_iter()
        .filter(|(_, a, b)| a != b)
        .collect()
    }
}

fn pdfium() -> Pdfium {
    let dir = std::env::var_os("PDFIUM_LIB_DIR").map_or_else(
        || {
            Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
                "../../vendor/pdfium/{}-{}",
                std::env::consts::OS,
                std::env::consts::ARCH
            ))
        },
        PathBuf::from,
    );
    Pdfium::new(
        Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(&dir))
            .expect("libpdfium (scripts/fetch-pdfium.sh)"),
    )
}

/// What PDFium makes of each page: its objects, or None when it can't
/// load the page.
fn pdfium_pages(pdfium: &Pdfium, bytes: &[u8]) -> Option<Vec<Option<Kinds>>> {
    let doc = pdfium.load_pdf_from_byte_slice(bytes, None).ok()?;
    let pages = doc.pages();
    Some(
        (0..pages.len())
            .map(|i| {
                let page = pages.get(i).ok()?;
                let mut kinds = Kinds::default();
                for object in page.objects().iter() {
                    match object.object_type() {
                        PdfPageObjectType::Text => kinds.text += 1,
                        PdfPageObjectType::Path => kinds.path += 1,
                        PdfPageObjectType::Image => kinds.image += 1,
                        PdfPageObjectType::XObjectForm => kinds.form += 1,
                        PdfPageObjectType::Shading => kinds.shading += 1,
                        PdfPageObjectType::Unsupported => {}
                    }
                }
                Some(kinds)
            })
            .collect(),
    )
}

fn direct<'a>(pdf: &'a LazyPdf, object: &'a Object) -> Option<&'a Object> {
    pdf.dereference(object).ok().map(|(_, o)| o)
}

/// A page's resource category (`/XObject`, `/Shading`).
fn category<'a>(pdf: &'a LazyPdf, page: &'a PageFacts, key: &[u8]) -> Option<&'a Dictionary> {
    let resources = direct(pdf, page.resources.as_deref()?)?.as_dict().ok()?;
    direct(pdf, resources.get(key).ok()?)?.as_dict().ok()
}

/// Whether a string operand holds a byte.
fn filled(string: &[u8]) -> bool {
    match string.first() {
        Some(b'(') => string.len() > 2,
        Some(b'<') => string.iter().any(u8::is_ascii_hexdigit),
        _ => false,
    }
}

/// Whether a `TJ` array holds a string.
fn has_string(array: &[u8]) -> bool {
    let inner = &array[1..array.len().saturating_sub(1).max(1)];
    sheaf_doc::content::lex(inner, &ResCx::none(), &Limits::DEFAULT)
        .map_while(Result::ok)
        .any(|(_, t)| match t {
            Tok::Operand(o) => o[0] == b'(' || (o[0] == b'<' && o.get(1) != Some(&b'<')),
            _ => false,
        })
}

/// What the lexer finds on a page, by what PDFium would make an object of.
/// `subtypes` keeps each XObject's `/Subtype`, by object, for the file.
fn lexed(
    pdf: &LazyPdf,
    page: &PageFacts,
    subtypes: &mut HashMap<ObjectId, Option<Vec<u8>>>,
) -> Result<Kinds, String> {
    let content =
        PageContent::read(pdf, &page.contents, &Limits::DEFAULT).map_err(|e| e.to_string())?;
    let res = ResCx::read(pdf, page.resources.as_deref());
    let xobjects = category(pdf, page, b"XObject");
    let shadings = category(pdf, page, b"Shading");
    let mut subtype = |name: &[u8]| -> Option<Vec<u8>> {
        let id = xobjects?.get(name).ok()?.as_reference().ok()?;
        subtypes
            .entry(id)
            .or_insert_with(|| {
                let object = pdf.object_unkept(id).ok()?;
                let dict = &object.as_stream().ok()?.dict;
                Some(dict.get(b"Subtype").ok()?.as_name().ok()?.to_vec())
            })
            .clone()
    };
    let mut kinds = Kinds::default();
    let mut operands: Vec<&[u8]> = Vec::new();
    let mut path = Pending::default();
    // What PDFium's objects depend on of the graphics state, at each `q`
    // level.
    let mut states = vec![State::default()];
    for token in content.concat().lex(&res, &Limits::DEFAULT) {
        let (_, token) = token.map_err(|e| content.fault(e).to_string())?;
        let op = match token {
            Tok::Operand(o) => {
                operands.push(o);
                continue;
            }
            Tok::InlineImage(_) => {
                kinds.image += 1;
                operands.clear();
                continue;
            }
            Tok::Op(op) => op,
        };
        let last = operands.last().copied();
        let state = *states.last().unwrap();
        let name = || last.and_then(sheaf_doc::content::name);
        match op {
            b"q" => states.push(state),
            b"Q" if states.len() > 1 => {
                states.pop();
            }
            b"Tf" => states.last_mut().unwrap().font = true,
            b"J" => states.last_mut().unwrap().round = last == Some(b"1"),
            b"Tj" | b"'" | b"\"" if state.font && last.is_some_and(filled) => kinds.text += 1,
            b"TJ" if state.font && last.is_some_and(|a| a[0] == b'[' && has_string(a)) => {
                kinds.text += 1
            }
            b"m" if operands.len() == 2 => path.point(true),
            b"l" if operands.len() == 2 => path.point(false),
            b"c" | b"v" | b"y" => path.point(false),
            b"re" => {
                path.point(true);
                path.point(false);
            }
            b"h" => path.close(),
            b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" => {
                // `b*` closes with a line whether or not it ends where it
                // started, `s` and `b` as `h` does.
                match op {
                    b"b*" => path.point(false),
                    b"s" | b"b" => path.close(),
                    _ => {}
                }
                let stroked = matches!(op, b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*");
                if path.points >= 2 || (path.closed && stroked && state.round) {
                    kinds.path += 1;
                }
                path = Pending::default();
            }
            b"n" => path = Pending::default(),
            b"Do" => match name().and_then(|n| subtype(&n)).as_deref() {
                Some(b"Image") => kinds.image += 1,
                Some(b"Form") => kinds.form += 1,
                _ => {}
            },
            b"sh" if name().is_some_and(|n| shadings.is_some_and(|s| s.has(&n))) => {
                kinds.shading += 1
            }
            _ => {}
        }
        operands.clear();
    }
    Ok(kinds)
}

/// A path being built, as PDFium keeps its points: a move straight after a
/// move takes its place, and a line before any move is dropped. A curve
/// or a rectangle counts as a point: one is enough to make two. Closing a
/// path that ends where it started adds no point, but closes a lone move,
/// which stroked with round caps is a dot.
#[derive(Default)]
struct Pending {
    points: u32,
    /// Whether the last point is a move.
    moved: bool,
    /// Whether it's a lone move, closed.
    closed: bool,
}

impl Pending {
    fn close(&mut self) {
        self.closed = self.points == 1;
    }

    fn point(&mut self, move_to: bool) {
        if move_to && self.moved {
            return;
        }
        if move_to || self.points > 0 {
            self.points += 1;
            self.moved = move_to;
        }
    }
}

/// What PDFium's objects depend on of the graphics state: whether a font
/// is set, and whether lines have round caps, which make a stroked
/// single point a dot.
#[derive(Debug, Default, Clone, Copy)]
struct State {
    font: bool,
    round: bool,
}

/// One file's pages: where the lexer and PDFium differ, and the pages the
/// lexer can't read that PDFium can.
#[derive(Default)]
struct Outcome {
    pages: usize,
    lexed: Kinds,
    pdfium: Kinds,
    differ: Vec<String>,
    unread: Vec<String>,
    skipped: Option<String>,
}

#[test]
#[ignore = "every page of the 402-file pool, through PDFium"]
fn lexer_boundaries_match_pdfium_page_objects() {
    let Some(corpus) = Corpus::or_skip() else {
        return;
    };
    let list = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bench/pool-sample.txt");
    let entries: Vec<Entry> = std::fs::read_to_string(list)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|path| Entry {
            path: path.trim().into(),
            ..Entry::default()
        })
        .collect();
    let started = Instant::now();
    // PDFium on one thread at a time; the lexer beside it.
    let pdfium = Mutex::new(pdfium());
    let outcomes = each_entry(&entries, |entry| {
        let mut out = Outcome::default();
        let Ok(bytes) = std::fs::read(corpus.path(entry)) else {
            out.skipped = Some(format!("{}: not in the corpus", entry.path));
            return out;
        };
        let bytes: Arc<[u8]> = bytes.into();
        let shown = {
            let pdfium = pdfium.lock().unwrap();
            pdfium_pages(&pdfium, &bytes)
        };
        let kind = BlobKind::file(Path::new(&entry.path), None);
        let (Some(shown), Ok(src)) = (shown, Store::new().open(bytes.clone(), kind)) else {
            out.skipped = Some(format!(
                "{}: PDFium or the source didn't open it",
                entry.path
            ));
            return out;
        };
        if shown.len() != src.pages().len() {
            out.skipped = Some(format!(
                "{}: PDFium has {} pages, the source {}",
                entry.path,
                shown.len(),
                src.pages().len()
            ));
            return out;
        }
        let mut subtypes = HashMap::new();
        for (i, (page, shown)) in src.pages().iter().zip(shown).enumerate() {
            let Some(shown) = shown else { continue };
            out.pages += 1;
            out.pdfium.add(shown);
            match lexed(src.pdf(), page, &mut subtypes) {
                Ok(kinds) => {
                    out.lexed.add(kinds);
                    let differ = kinds.differences(&shown);
                    if !differ.is_empty() {
                        out.differ
                            .push(format!("{} page {}: {differ:?}", entry.path, i + 1));
                    }
                }
                Err(e) => out
                    .unread
                    .push(format!("{} page {}: {e}", entry.path, i + 1)),
            }
        }
        out
    });
    let (mut pages, mut lexed, mut shown) = (0, Kinds::default(), Kinds::default());
    for outcome in &outcomes {
        pages += outcome.pages;
        lexed.add(outcome.lexed);
        shown.add(outcome.pdfium);
    }
    let differ: Vec<&String> = outcomes.iter().flat_map(|o| &o.differ).collect();
    let unread: Vec<&String> = outcomes.iter().flat_map(|o| &o.unread).collect();
    for skipped in outcomes.iter().filter_map(|o| o.skipped.as_ref()) {
        eprintln!("skipped {skipped}");
    }
    for line in &differ {
        eprintln!("differs: {line} (lexer, PDFium)");
    }
    eprintln!(
        "{pages} pages of {} files in {:?}: lexer {lexed:?}, PDFium {shown:?}; \
         {} pages differ, {} the lexer can't read",
        entries.len(),
        started.elapsed(),
        differ.len(),
        unread.len()
    );
    assert!(unread.is_empty(), "pages PDFium reads: {unread:#?}");
    assert!(differ.is_empty(), "{} pages differ", differ.len());
}
