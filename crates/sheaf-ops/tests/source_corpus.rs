//! Every corpus file PDFium opens opens as a source, and every page's
//! size, as the source's page table reads it, is the size PDFium gives it
//! (M10.5 slice 3): the app opens a file through its source, the layout
//! takes its sizes from the table, and PDFium renders the pages, by index,
//! until slice 13. Skips when there's no corpus (see
//! `sheaf_doc::forms::corpus`).
//!
//! Needs libpdfium (`scripts/fetch-pdfium.sh`), like the app itself.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

use sheaf_core::{PageIndex, PageSize};
use sheaf_doc::Store;
use sheaf_doc::forms::corpus::Corpus;
use sheaf_doc::model::SourceInfo;
use sheaf_render::Renderer;
use sheaf_render::pdfium::PdfiumEngine;

/// Each page's size as the page table has it.
fn table_sizes(path: &Path) -> Result<Vec<PageSize>, String> {
    let src = SourceInfo::read(&Store::new(), path, None)
        .map_err(|e| format!("{e:#}"))?
        .src;
    Ok((0..src.page_count()).map(|i| src.page_size(i)).collect())
}

/// Each page's size as PDFium has it; None when PDFium doesn't open the
/// file.
fn pdfium_sizes(path: &Path) -> Option<Vec<Result<PageSize, String>>> {
    let bytes = std::fs::read(path).ok()?;
    let engine = PdfiumEngine::open_bytes(bytes.into(), None).ok()?;
    Some(
        (0..engine.page_count())
            .map(|i| engine.page_size(PageIndex(i)).map_err(|e| format!("{e:#}")))
            .collect(),
    )
}

#[test]
fn every_corpus_page_s_geom_is_pdfium_s_size() {
    let Some(corpus) = Corpus::or_skip() else {
        return;
    };
    let entries = corpus.manifest().unwrap();
    // The sources open on other threads, PDFium on this one: sheaf holds
    // one lock across every use of it.
    let queue = Mutex::new(entries.iter().enumerate());
    let tables = Mutex::new(BTreeMap::new());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(16));
    let mut engines = Vec::new();
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let Some((i, entry)) = queue.lock().unwrap().next() else {
                        break;
                    };
                    let sizes = table_sizes(&corpus.path(entry));
                    tables.lock().unwrap().insert(i, sizes);
                }
            });
        }
        for entry in &entries {
            engines.push(pdfium_sizes(&corpus.path(entry)));
        }
    });
    let tables = tables.into_inner().unwrap();

    let (mut opened, mut pages) = (0, 0);
    let mut differ: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for ((entry, engine), table) in entries.iter().zip(engines).zip(tables.into_values()) {
        let Some(engine) = engine else {
            continue;
        };
        opened += 1;
        let table = match table {
            Ok(table) => table,
            Err(e) => {
                differ.insert(
                    &entry.path,
                    vec![format!("PDFium opens it, the source doesn't: {e}")],
                );
                continue;
            }
        };
        let mut found = Vec::new();
        if table.len() != engine.len() {
            found.push(format!(
                "{} pages in the table, {} in PDFium",
                table.len(),
                engine.len()
            ));
        }
        for (page, (table, pdfium)) in table.into_iter().zip(engine).enumerate() {
            pages += 1;
            if pdfium.as_ref() != Ok(&table) {
                found.push(format!(
                    "page {page}: {table:?} in the table, {pdfium:?} in PDFium"
                ));
            }
        }
        if !found.is_empty() {
            differ.insert(&entry.path, found);
        }
    }
    println!(
        "{} files, {opened} PDFium opens, {pages} pages compared, {} files differ",
        entries.len(),
        differ.len()
    );
    for (path, found) in &differ {
        println!(
            "{path}: {} differences: {}",
            found.len(),
            found[..found.len().min(3)].join("; ")
        );
    }
    assert!(opened > 0, "PDFium opened nothing");
    assert!(differ.is_empty(), "{} files differ", differ.len());
}
