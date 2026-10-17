//! `render_appearances` shows what a viewer that draws only appearance
//! streams shows (M10 bug 10): a value with no appearance is blank there,
//! though the page view draws it; a check box shows its `/AS` state, and
//! a radio button with no `/AS` shows none; hidden and `NoView` widgets
//! stay out, and one that doesn't print still shows.
//!
//! Needs libpdfium (`scripts/fetch-pdfium.sh`), like the app itself.

use lopdf::{Object, Stream, dictionary};
use sheaf_core::{PageIndex, Rotation};
use sheaf_doc::forms::fixture::{Form, Rect};
use sheaf_image::raster::{PixelFormat, Raster};
use sheaf_render::pdfium::{PdfiumEngine, render_appearances};
use sheaf_render::{Job, Renderer};

fn row(i: usize) -> Rect {
    let top = 740.0 - 50.0 * i as f32;
    [100.0, top - 30.0, 300.0, top]
}

/// A text field drawn as a solid red box, with annotation flags `f`.
fn red(form: &mut Form, name: &str, i: usize, f: i64) {
    let id = form.text(name, 0, row(i));
    form.dict(id).set("V", Object::string_literal(name));
    form.dict(id).set("F", f);
    let ap = form.pdf.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Form",
            "BBox" => vec![0.into(), 0.into(), 200.into(), 30.into()],
        },
        b"1 0 0 rg 0 0 200 30 re f".to_vec(),
    ));
    form.dict(id).set("AP", dictionary! { "N" => ap });
}

/// Red and dark pixels inside row `i` (the page is 612 wide, so a pixel
/// is a point).
fn count(r: &Raster, i: usize) -> (usize, usize) {
    let [x0, y0, x1, y1] = row(i).map(|v| v as usize);
    let (mut red, mut dark) = (0, 0);
    for y in (792 - y1 + 2)..(792 - y0 - 2) {
        for x in (x0 + 2)..(x1 - 2) {
            let p = &r.data[(y * r.width as usize + x) * 4..][..3];
            if p[0] > 200 && p[1] < 60 && p[2] < 60 {
                red += 1;
            }
            if p[0] < 100 && p[1] < 100 && p[2] < 100 {
                dark += 1;
            }
        }
    }
    (red, dark)
}

#[test]
fn appearances_only() {
    let mut form = Form::new(1);
    red(&mut form, "drawn", 0, 4);
    let bare = form.text("bare", 0, row(1));
    form.dict(bare)
        .set("V", Object::string_literal("HELLO WORLD"));
    form.dict(bare)
        .set("DA", Object::string_literal("/Helv 20 Tf 0 g"));
    form.check_box("on", 0, row(2), true);
    form.check_box("off", 0, row(3), false);
    red(&mut form, "hidden", 4, 4 | 2);
    red(&mut form, "noview", 5, 4 | 32);
    red(&mut form, "screen", 6, 0);
    // States "0" and "1" sort before "Off": PDFium's flatten would draw
    // them.
    let pick = form.radio("pick", 0, &[row(7), row(8)], Some(0));
    for w in form.widgets_of(pick) {
        form.dict(w).remove(b"AS");
    }
    let acroform = form.acroform;
    form.dict(acroform).set("NeedAppearances", true);
    let bytes = form.bytes();

    let shown = render_appearances(&bytes, None, PageIndex(0), 612).unwrap();
    assert_eq!((shown.width, shown.height), (612, 792));
    let filled = 196 * 26;
    assert_eq!(count(&shown, 0), (filled, 0), "drawn");
    assert_eq!(count(&shown, 1), (0, 0), "a value with no appearance");
    assert_eq!(count(&shown, 2), (0, filled), "a check box that's on");
    assert_eq!(count(&shown, 3), (0, 0), "one that's off");
    assert_eq!(count(&shown, 4), (0, 0), "hidden");
    assert_eq!(count(&shown, 5), (0, 0), "NoView");
    assert_eq!(count(&shown, 6), (filled, 0), "on screen, not in print");
    assert_eq!(count(&shown, 7), (0, 0), "a radio button with no /AS");
    assert_eq!(count(&shown, 8), (0, 0), "another");

    // The page view draws the bare value, and redraws the others from
    // theirs (NeedAppearances).
    let engine = PdfiumEngine::open_bytes(bytes.into(), None).unwrap();
    let job = Job::Page {
        width: 612,
        rotation: Rotation::R0,
    };
    let screen = engine.render(PageIndex(0), job, PixelFormat::Rgba).unwrap();
    assert!(count(&screen, 1).1 > 100, "{:?}", count(&screen, 1));
}
