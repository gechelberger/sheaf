//! Cropping a page: the plan, the edit, and what a save writes.
//!
//! Needs libpdfium (`scripts/fetch-pdfium.sh`), like the app itself.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lopdf::{Dictionary, Document as Pdf, Stream, dictionary};
use sheaf_core::{PageIndex, Rotation};
use sheaf_doc::geom::Geom;
use sheaf_doc::{
    Annot, AnnotId, AnnotKind, Box2, DestView, Document, Edit, OcrWord, PageChars, PageId, Pt,
    TextLayer,
};
use sheaf_image::raster::{PixelFormat, content_bounds};
use sheaf_ops::crop::{CropSpec, Margins};
use sheaf_render::pdfium::PdfiumEngine;
use sheaf_render::{Job, Renderer};

/// A Letter page with a black square at user (300..400, 300..400).
fn pdf(dir: &Path) -> PathBuf {
    let page = dictionary! {
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
    };
    one_page(dir, "square.pdf", page, b"0 0 0 rg 300 300 100 100 re f")
}

/// A Letter-sized page whose media box is off the origin, with a line of
/// Helvetica at user (100, 500).
fn text_pdf(dir: &Path) -> PathBuf {
    let font = dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" };
    let page = dictionary! {
        "MediaBox" => vec![(-37).into(), 52.into(), 575.into(), 844.into()],
        "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } },
    };
    let text = b"BT /F1 24 Tf 100 500 Td (Glyphs under sheaf ink) Tj ET";
    one_page(dir, "text.pdf", page, text)
}

/// `dir/name`: one page, `page`'s keys and `content`.
fn one_page(dir: &Path, name: &str, mut page: Dictionary, content: &[u8]) -> PathBuf {
    let mut pdf = Pdf::with_version("1.5");
    let pages_id = pdf.new_object_id();
    let content = pdf.add_object(Stream::new(dictionary! {}, content.to_vec()));
    page.set("Type", "Page");
    page.set("Parent", pages_id);
    page.set("Contents", content);
    let page = pdf.add_object(page);
    pdf.objects.insert(
        pages_id,
        dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 }.into(),
    );
    let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    pdf.trailer.set("Root", catalog);
    let path = dir.join(name);
    pdf.save(&path).unwrap();
    path
}

/// `path` opened as the app opens it.
fn open(path: &Path) -> Document {
    sheaf_doc::open::file(path, None).unwrap().doc
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

/// Crop `page` of `doc` by `m`, as Document > Crop Pages does.
fn crop(doc: &mut Document, page: PageId, m: Margins) {
    let plan = sheaf_ops::crop::plan(
        doc.store(),
        &doc.snapshot(),
        &[page],
        CropSpec::Margins(m),
        &sheaf_ops::NoProgress,
    )
    .unwrap();
    let sid = doc.add_source(plan[0].info.clone());
    doc.apply(
        Edit::RebasePages {
            pages: plan[0].rebases(sid),
        },
        "Crop",
    )
    .unwrap();
}

/// Margins off every side, none alike.
const UNEVEN: Margins = Margins {
    top: 50.0,
    right: 30.0,
    bottom: 20.0,
    left: 72.0,
};

/// A red mark of `kind` for the document to give an id, reading upright.
fn mark(kind: AnnotKind) -> Annot {
    Annot {
        id: AnnotId(0),
        kind,
        color: [255, 0, 0],
        opacity: 1.0,
        stroke: 1.0,
        author: String::new(),
        contents: String::new(),
        modified: String::new(),
        created: String::new(),
        fill: None,
        dashed: false,
        cloudy: false,
        turn: Rotation::R0,
        name: String::new(),
    }
}

/// PDFium's characters on the first page, boxes in its view frame.
fn engine_chars(engine: &PdfiumEngine) -> Vec<(char, Option<Box2>)> {
    let chars = engine.page_chars(PageIndex(0)).unwrap();
    chars
        .into_iter()
        .map(|(c, r)| {
            let r = r.map(|r| Box2 {
                x: r.x,
                y: r.y,
                w: r.w,
                h: r.h,
            });
            (c, r)
        })
        .collect()
}

/// PDFium's glyph boxes for `word` on the first page, into PageSpace
/// through the page's `geom`.
fn glyphs(engine: &PdfiumEngine, geom: &Geom, word: &str) -> Vec<Box2> {
    let chars = engine_chars(engine);
    let n = word.chars().count();
    let at = chars
        .windows(n)
        .position(|w| w.iter().map(|(c, _)| *c).eq(word.chars()))
        .unwrap_or_else(|| panic!("{word:?} isn't on the page"));
    chars[at..at + n]
        .iter()
        .map(|(_, r)| geom.from_view().apply_box(r.unwrap()))
        .collect()
}

fn crop_box(path: &Path) -> Vec<f64> {
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

/// A crop is a box edit: the mark over the square, in PageSpace, is
/// what it was, and the cropped page, saved and drawn by PDFium, shows the
/// square where the page's frame puts the mark.
#[test]
fn margins_crop_and_marks_follow() {
    let dir = scratch("margins");
    let src = pdf(&dir);
    let mut doc = open(&src);
    let page = doc.state().pages[0].id;
    let over = Box2 {
        x: 325.0,
        y: 325.0,
        w: 50.0,
        h: 50.0,
    };
    let annot = mark(AnnotKind::Square { rect: over });
    doc.apply(Edit::AnnotAdd { page, annot }, "mark").unwrap();
    let m = Margins {
        top: 72.0,
        left: 72.0,
        right: 0.0,
        bottom: 0.0,
    };
    crop(&mut doc, page, m);
    let p = doc.state().pages[0];
    let size = doc.page_size(&p);
    assert_eq!((size.width, size.height), (540.0, 720.0));
    let kept = &doc.state().annots_on(page).unwrap()[0];
    assert_eq!(kept.kind, AnnotKind::Square { rect: over });
    let shown = doc.geom(&p).to_view().apply_box(over);
    assert_eq!((shown.x, shown.y), (253.0, 345.0));
    // Saved, the page's crop box is the shown area.
    let out = dir.join("out.pdf");
    sheaf_doc::save::rewrite_to_path(&doc.snapshot(), &out).unwrap();
    assert_eq!(crop_box(&out), vec![72.0, 0.0, 612.0, 720.0]);
    let engine = PdfiumEngine::open(&out, None).unwrap();
    let job = Job::Page {
        width: 540,
        rotation: Rotation::R0,
    };
    let raster = engine.render(PageIndex(0), job, PixelFormat::Rgba).unwrap();
    let (cx, cy) = (shown.x + shown.w / 2.0, shown.y + shown.h / 2.0);
    let i = ((cy as u32 * raster.width + cx as u32) * 4) as usize;
    let px = &raster.data[i..i + 3];
    assert!(
        px.iter().all(|&c| c < 60),
        "the square under the mark: {px:?}"
    );
    // Undo puts it back.
    doc.undo().unwrap().unwrap();
    assert_eq!(doc.page_size(&doc.state().pages[0]).width, 612.0);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A highlight made over a word as the app makes one, from PDFium's glyph
/// boxes, still covers the word once its page is cropped, turned and
/// saved either way: reopened, its rects are as they were, and PDFium's
/// boxes for the word on the saved page, through that page's Geom, lie
/// inside them.
#[test]
fn a_cropped_rotated_highlight_still_covers_its_glyphs() {
    let dir = scratch("highlight");
    let src = text_pdf(&dir);
    let mut doc = open(&src);
    let p = doc.state().pages[0];
    let engine = PdfiumEngine::open(&src, None).unwrap();
    let chars = PageChars::from_engine(engine_chars(&engine), &doc.geom(&p));
    let words = chars.words();
    let (_, (a, b), _) = words.iter().find(|(w, ..)| w == "sheaf").unwrap();
    let rects = chars.rects(*a, *b);
    let annot = Annot {
        opacity: 0.6,
        turn: chars.upright,
        ..mark(AnnotKind::Highlight {
            rects: rects.clone(),
        })
    };
    doc.apply(Edit::AnnotAdd { page: p.id, annot }, "highlight")
        .unwrap();
    crop(&mut doc, p.id, UNEVEN);
    let turn = Edit::RotatePages {
        ids: vec![p.id],
        by: Rotation::R90,
    };
    doc.apply(turn, "rotate").unwrap();
    let now = doc.geom(&doc.state().pages[0]);
    let shown = (now.visible(), now.rotate);
    assert_eq!(shown.1, Rotation::R90);
    let snap = doc.snapshot();
    let written = [
        ("rewrite", sheaf_doc::save::rewrite(&snap).unwrap()),
        (
            "incremental",
            sheaf_doc::incremental::incremental(&snap).unwrap(),
        ),
    ];
    for (how, bytes) in written {
        let out = dir.join(format!("{how}.pdf"));
        std::fs::write(&out, bytes).unwrap();
        let saved = open(&out);
        let page = saved.state().pages[0];
        let geom = saved.geom(&page);
        assert_eq!((geom.visible(), geom.rotate), shown, "{how}");
        let marks = saved.state().annots_on(page.id).unwrap();
        let AnnotKind::Highlight { rects: kept } = &marks[0].kind else {
            panic!("{how}: {:?}", marks[0].kind);
        };
        let near = |a: Box2, b: Box2, by: f32| {
            (a.x - b.x).abs() <= by
                && (a.y - b.y).abs() <= by
                && (a.right() - b.right()).abs() <= by
                && (a.bottom() - b.bottom()).abs() <= by
        };
        assert_eq!(kept.len(), rects.len(), "{how}");
        for (k, r) in kept.iter().zip(&rects) {
            assert!(near(*k, *r, 1.0 / 16.0), "{how}: {k:?} was {r:?}");
        }
        // PDFium's boxes on the saved page fall inside, to its 1/16 pt grid.
        let engine = PdfiumEngine::open(&out, None).unwrap();
        for g in glyphs(&engine, &geom, "sheaf") {
            assert!(
                kept.iter().any(|k| near(k.union(g), *k, 1.0 / 16.0)),
                "{how}: {g:?} outside {kept:?}"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn trim_white_finds_the_content() {
    let dir = scratch("trim");
    let src = pdf(&dir);
    let doc = open(&src);
    let page = doc.state().pages[0].id;
    let plan = sheaf_ops::crop::plan(
        doc.store(),
        &doc.snapshot(),
        &[page],
        CropSpec::TrimWhite { pad: 0.0 },
        &sheaf_ops::NoProgress,
    )
    .unwrap();
    let size = plan[0].info.src.page_size(0);
    assert!((size.width - 100.0).abs() <= 2.0, "{size:?}");
    assert!((size.height - 100.0).abs() <= 2.0, "{size:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A bookmark's `/XYZ`, in PageSpace, stays on the same content through a
/// crop: saved and reopened it's unchanged, and the cropped page's frame
/// puts it on the square's top-left corner as PDFium draws the page.
#[test]
fn a_bookmark_on_a_cropped_page_lands_on_the_same_content() {
    let dir = scratch("bookmark");
    let src = pdf(&dir);
    let mut doc = open(&src);
    let page = doc.state().pages[0].id;
    // The square's top-left in user space.
    let view = DestView::Xyz {
        left: Some(300.0),
        top: Some(400.0),
        zoom: None,
    };
    let insert = Edit::OutlineInsert {
        parent: None,
        after: None,
        title: "Square".into(),
        target: Some(page),
        view: Some(view.clone()),
    };
    doc.apply(insert, "bookmark").unwrap();
    crop(&mut doc, page, UNEVEN);
    let out = dir.join("out.pdf");
    sheaf_doc::save::rewrite_to_path(&doc.snapshot(), &out).unwrap();
    let saved = open(&out);
    assert_eq!(saved.state().outline[0].view, Some(view));
    let p = saved.state().pages[0];
    let at = saved.geom(&p).to_view().apply(Pt { x: 300.0, y: 400.0 });
    // Where PDFium draws the square's top-left on the saved page.
    let size = saved.page_size(&p);
    let engine = PdfiumEngine::open(&out, None).unwrap();
    let job = Job::Page {
        width: size.width.round() as u32,
        rotation: Rotation::R0,
    };
    let raster = engine.render(PageIndex(0), job, PixelFormat::Rgba).unwrap();
    let (sx, sy) = (
        size.width / raster.width as f32,
        size.height / raster.height as f32,
    );
    let (x0, y0, _, _) = content_bounds(&raster.to_gray().unwrap(), 128).unwrap();
    let corner = Pt {
        x: x0 as f32 * sx,
        y: y0 as f32 * sy,
    };
    assert!(
        (corner.x - at.x).abs() <= 1.0 && (corner.y - at.y).abs() <= 1.0,
        "the bookmark shows at {at:?}, the square at {corner:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A crop leaves the page's OCR text as it was, in PageSpace, and PDFium
/// finds the saved text on the words it was recognized on.
#[test]
fn a_crop_leaves_ocr_text_on_its_words() {
    let dir = scratch("ocr");
    let src = pdf(&dir);
    let mut doc = open(&src);
    let p = doc.state().pages[0];
    // A word along the square's top as shown.
    let word = OcrWord {
        text: "square".into(),
        rect: Box2 {
            x: 300.0,
            y: 392.0,
            w: 100.0,
            h: 20.0,
        },
        confidence: 1.0,
    };
    let layer = Arc::new(TextLayer::from_engine(vec![word], &doc.geom(&p), "test"));
    let set = Edit::SetTextLayers {
        layers: vec![(p.id, layer.clone())],
    };
    doc.apply(set, "ocr").unwrap();
    crop(&mut doc, p.id, UNEVEN);
    assert!(Arc::ptr_eq(doc.state().text_layer(p.id).unwrap(), &layer));
    let out = dir.join("out.pdf");
    sheaf_doc::save::rewrite_to_path(&doc.snapshot(), &out).unwrap();
    let saved = open(&out);
    let geom = saved.geom(&saved.state().pages[0]);
    let engine = PdfiumEngine::open(&out, None).unwrap();
    let hits = engine.search_page(PageIndex(0), "square", false).unwrap();
    assert_eq!(hits.len(), 1, "{hits:?}");
    let hit = hits[0]
        .rects
        .iter()
        .map(|r| Box2 {
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
        })
        .reduce(Box2::union)
        .unwrap();
    let (got, want) = (geom.from_view().apply_box(hit), layer.words[0].rect);
    let near = |a: f32, b: f32| (a - b).abs() <= 2.0;
    assert!(
        near(got.x, want.x)
            && near(got.y, want.y)
            && near(got.right(), want.right())
            && near(got.bottom(), want.bottom()),
        "found at {got:?}, recognized at {want:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
