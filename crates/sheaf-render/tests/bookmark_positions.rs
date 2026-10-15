//! Bookmark positions written by `sheaf-doc` come back through PDFium where
//! they were put, in the page's displayed space, whatever the file's own
//! `/Rotate`; other destination kinds come back as they were.
//!
//! Needs libpdfium (`scripts/fetch-pdfium.sh`), like the app itself.

use std::path::{Path, PathBuf};

use lopdf::{Document as Pdf, Object, Stream, dictionary};
use sheaf_core::{DestView, PageSize};
use sheaf_doc::{Document, FixedCount, FlatOutlineItem, SourceInfo};
use sheaf_render::Renderer;
use sheaf_render::pdfium::PdfiumEngine;

fn tempdir() -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "sheaf-render-bookmarks-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Two Letter pages: the first unrotated, the second with `/Rotate 90`.
fn two_pages(path: &Path) {
    let mut pdf = Pdf::with_version("1.5");
    let pages_id = pdf.new_object_id();
    let mut kids = Vec::new();
    for rotate in [0, 90] {
        let content = pdf.add_object(Stream::new(dictionary! {}, Vec::new()));
        let mut page = dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => content,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        };
        if rotate != 0 {
            page.set("Rotate", rotate);
        }
        kids.push(Object::Reference(pdf.add_object(page)));
    }
    pdf.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => 2 }),
    );
    let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    pdf.trailer.set("Root", catalog);
    pdf.save(path).unwrap();
}

fn xyz(left: f32, top: f32) -> Option<DestView> {
    Some(DestView::Xyz {
        left: Some(left),
        top: Some(top),
        zoom: None,
    })
}

#[test]
fn positions_round_trip_through_pdfium() {
    let dir = tempdir();
    let src = dir.join("src.pdf");
    two_pages(&src);
    let item = |title: &str, page, view| FlatOutlineItem {
        title: title.into(),
        page: Some(page),
        depth: 0,
        view,
        color: None,
        style: Default::default(),
    };
    let fit_h = Some(DestView::Other {
        kind: "FitH".into(),
        params: vec![Some(500.0)],
    });
    let model = Document::new(
        SourceInfo {
            path: src.clone(),
            page_count: 2,
            first_page: PageSize {
                width: 612.0,
                height: 792.0,
            },
            page_sizes: None,
            bytes: 0,
            password: None,
            data: None,
            derived: None,
        },
        &[
            item("Upright", 0, xyz(36.0, 100.0)),
            // Displayed space on a /Rotate 90 page is 792 wide, 612 tall.
            item("Turned", 1, xyz(400.0, 50.0)),
            item("Fit width", 1, fit_h.clone()),
            item("Top", 0, None),
        ],
        Box::new(FixedCount(5)),
    );
    let out = dir.join("out.pdf");
    std::fs::write(&out, sheaf_doc::save::rewrite(&model.snapshot()).unwrap()).unwrap();

    let engine = PdfiumEngine::open(&out, None).unwrap();
    let got: Vec<(String, Option<u32>, Option<DestView>)> = engine
        .outline()
        .unwrap()
        .into_iter()
        .map(|i| (i.title, i.page, i.view))
        .collect();
    let close = |v: &Option<DestView>, l: f32, t: f32| match v {
        Some(DestView::Xyz {
            left: Some(a),
            top: Some(b),
            zoom: None,
        }) => (a - l).abs() < 0.01 && (b - t).abs() < 0.01,
        _ => false,
    };
    assert_eq!(got.len(), 4, "{got:?}");
    assert!(close(&got[0].2, 36.0, 100.0), "{got:?}");
    assert!(close(&got[1].2, 400.0, 50.0), "{got:?}");
    assert_eq!(got[2].2, fit_h);
    assert_eq!(got[3].2, None, "a page-only bookmark has no position");
    assert_eq!(
        got.iter().map(|g| g.1).collect::<Vec<_>>(),
        [Some(0), Some(1), Some(1), Some(0)]
    );
    let _ = std::fs::remove_dir_all(&dir);
}
