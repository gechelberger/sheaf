//! Cropping a page: the plan, the edit, and what a save writes.
//!
//! Needs libpdfium (`scripts/fetch-pdfium.sh`), like the app itself.

use std::path::{Path, PathBuf};

use lopdf::{Document as Pdf, Stream, dictionary};
use sheaf_doc::{Annot, AnnotId, AnnotKind, Box2, Edit};
use sheaf_ops::crop::{CropSpec, Margins};

/// A Letter page with a black square at user (300..400, 300..400).
fn pdf(dir: &Path) -> PathBuf {
    let mut pdf = Pdf::with_version("1.5");
    let pages_id = pdf.new_object_id();
    let content = pdf.add_object(Stream::new(
        dictionary! {},
        b"0 0 0 rg 300 300 100 100 re f".to_vec(),
    ));
    let page = pdf.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => content,
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
    });
    pdf.objects.insert(
        pages_id,
        dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 }.into(),
    );
    let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    pdf.trailer.set("Root", catalog);
    let path = dir.join("square.pdf");
    pdf.save(&path).unwrap();
    path
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sheaf-crop-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // SAFETY: each test sets the same value before any reads it.
    unsafe {
        std::env::set_var(
            "SHEAF_CACHE_DIR",
            std::env::temp_dir().join("sheaf-crop-cache"),
        )
    };
    dir
}

fn crop_box(path: &Path) -> Vec<f32> {
    let pdf = Pdf::load(path).unwrap();
    let id = *pdf.get_pages().values().next().unwrap();
    pdf.get_dictionary(id)
        .unwrap()
        .get(b"CropBox")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o.as_float().unwrap())
        .collect()
}

#[test]
fn margins_crop_and_marks_follow() {
    let dir = scratch("margins");
    let src = pdf(&dir);
    let mut doc = sheaf_ops::open(&src, None).unwrap();
    let page = doc.state().pages[0].id;
    let mark = Annot {
        id: AnnotId(0),
        kind: AnnotKind::Square {
            rect: Box2 {
                x: 100.0,
                y: 100.0,
                w: 50.0,
                h: 50.0,
            },
        },
        color: [255, 0, 0],
        opacity: 1.0,
        stroke: 1.0,
        author: String::new(),
        contents: String::new(),
        modified: String::new(),
        created: String::new(),
        fill: None,
        dashed: false,
    };
    doc.apply(Edit::AnnotAdd { page, annot: mark }, "mark")
        .unwrap();
    let m = Margins {
        top: 72.0,
        left: 72.0,
        right: 0.0,
        bottom: 0.0,
    };
    let plan = sheaf_ops::crop::plan(
        &doc.snapshot(),
        &[page],
        CropSpec::Margins(m),
        &sheaf_ops::NoProgress,
    )
    .unwrap();
    assert_eq!(plan.len(), 1);
    let sid = doc.add_source(plan[0].info.clone());
    doc.apply(
        Edit::RebasePages {
            pages: plan[0].rebases(sid),
        },
        "Crop",
    )
    .unwrap();
    let p = doc.state().pages[0];
    let size = doc.page_size(&p);
    assert_eq!((size.width, size.height), (540.0, 720.0));
    let moved = doc.state().annots_on(page).unwrap()[0].bounds();
    assert_eq!((moved.x, moved.y), (28.0, 28.0));
    // Saved, the page's crop box is the shown area.
    let out = dir.join("out.pdf");
    sheaf_doc::save::rewrite_to_path(&doc.snapshot(), &out).unwrap();
    assert_eq!(crop_box(&out), vec![72.0, 0.0, 612.0, 720.0]);
    // Undo puts it back.
    doc.undo().unwrap();
    assert_eq!(doc.page_size(&doc.state().pages[0]).width, 612.0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn trim_white_finds_the_content() {
    let dir = scratch("trim");
    let src = pdf(&dir);
    let doc = sheaf_ops::open(&src, None).unwrap();
    let page = doc.state().pages[0].id;
    let plan = sheaf_ops::crop::plan(
        &doc.snapshot(),
        &[page],
        CropSpec::TrimWhite { pad: 0.0 },
        &sheaf_ops::NoProgress,
    )
    .unwrap();
    let size = plan[0].info.page_sizes.as_ref().unwrap()[0];
    assert!((size.width - 100.0).abs() <= 2.0, "{size:?}");
    assert!((size.height - 100.0).abs() <= 2.0, "{size:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
