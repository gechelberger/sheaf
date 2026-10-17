//! Documents open, size and draw from several threads at once, as the app
//! opens each one on a render thread of its own. pdfium-render's
//! `thread_safe` lock leaves some calls out (one of them in every load),
//! and PDFium parsing on two threads at once leaves its parser broken for
//! the rest of the process: most loads failed with `FormatError` (M10
//! slice 1's note). Fixed by holding one lock across every use of PDFium.
//!
//! The documents are forms built in code, and some of the forms in Greg's
//! corpus when it's there (see `sheaf_doc::forms::corpus`).
//!
//! Needs libpdfium (`scripts/fetch-pdfium.sh`), like the app itself.

use std::sync::{Arc, Mutex};

use sheaf_core::PageIndex;
use sheaf_doc::forms::corpus::Corpus;
use sheaf_doc::forms::fixture::{self, Form};
use sheaf_image::raster::PixelFormat;
use sheaf_render::Renderer;
use sheaf_render::pdfium::{PdfiumEngine, render_appearances};

const THREADS: usize = 8;

/// Times each thread uses the forms built here: on two cores, one round
/// is over too soon for the threads to overlap much. (On one they hardly
/// overlap at all, and the test can't catch the bug.)
const ROUNDS: usize = 4;

/// Corpus forms used at most, so the test takes a second or two: the
/// threads take turns in PDFium.
const FROM_CORPUS: usize = 16;

type Docs = Vec<(String, Arc<[u8]>)>;

/// Forms of a few sizes and kinds.
fn fixtures() -> Docs {
    let mut out = vec![("every kind".into(), fixture::every_kind().bytes())];
    let mut hybrid = fixture::every_kind();
    hybrid.xfa();
    out.push(("hybrid".into(), hybrid.bytes()));
    for pages in [1, 4, 12] {
        let mut form = Form::new(pages);
        for page in 0..pages {
            for i in 0..20 {
                let name = format!("p{page}.f{i}");
                if i % 4 == 3 {
                    form.check_box(&name, page, fixture::row(i), i % 8 == 3);
                } else {
                    let id = form.text(&name, page, fixture::row(i));
                    form.fill(id, &name);
                }
            }
        }
        out.push((format!("{pages} pages"), form.bytes()));
    }
    out.into_iter().map(|(name, b)| (name, b.into())).collect()
}

/// Up to [`FROM_CORPUS`] of the corpus's forms, from across it, but none
/// of the SafeDocs sample; and only those that open and draw from one
/// thread, since the corpus keeps damaged files too.
fn corpus() -> Docs {
    let Some(corpus) = Corpus::find() else {
        return Vec::new();
    };
    let entries: Vec<_> = corpus
        .manifest()
        .unwrap()
        .into_iter()
        .filter(|e| !e.path.starts_with("forms/safedocs/"))
        .collect();
    entries
        .iter()
        .step_by(entries.len().div_ceil(FROM_CORPUS).max(1))
        .filter_map(|e| {
            Some((
                e.path.clone(),
                Arc::from(std::fs::read(corpus.path(e)).ok()?),
            ))
        })
        .filter(|(_, bytes)| use_document(bytes, true).is_ok())
        .collect()
}

/// What the app does with a document: open it, size its pages and read
/// the first page's text; and if `draw`, draw that page, as the page view
/// does and as a viewer of appearances only does.
fn use_document(bytes: &Arc<[u8]>, draw: bool) -> anyhow::Result<()> {
    let engine = PdfiumEngine::open_bytes(bytes.clone(), None)?;
    for i in 0..engine.page_count() {
        engine.page_size(PageIndex(i))?;
    }
    engine.page_text(PageIndex(0))?;
    if draw {
        engine.render_page(PageIndex(0), 40, PixelFormat::Bgra)?;
        render_appearances(bytes, None, PageIndex(0), 40)?;
    }
    Ok(())
}

#[test]
fn documents_open_from_several_threads() {
    let mut docs = fixtures();
    let built = docs.len();
    let found = corpus();
    eprintln!("{built} forms built here, {} from the corpus", found.len());
    docs.extend(found);
    // Which documents each thread uses, in turn: the built ones ROUNDS
    // times, those from the corpus once.
    let uses: Vec<usize> = (0..ROUNDS)
        .flat_map(|_| 0..built)
        .chain(built..docs.len())
        .collect();

    let failed = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for t in 0..THREADS {
            let (docs, uses, failed) = (&docs, &uses, &failed);
            s.spawn(move || {
                // Each thread starts at a different one, and draws an
                // eighth of them.
                for &i in uses.iter().cycle().skip(t).take(uses.len()) {
                    let (name, bytes) = &docs[i];
                    if let Err(e) = use_document(bytes, i % THREADS == t) {
                        failed.lock().unwrap().push(format!("{name}: {e:#}"));
                    }
                }
            });
        }
    });
    let failed = failed.into_inner().unwrap();
    assert!(
        failed.is_empty(),
        "{} of {} failed, first {:#?}",
        failed.len(),
        uses.len() * THREADS,
        &failed[..failed.len().min(5)]
    );

    // And PDFium still opens them from one thread.
    for (name, bytes) in &docs {
        use_document(bytes, false).unwrap_or_else(|e| panic!("{name}: {e:#}"));
    }
}
