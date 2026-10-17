//! The pipeline on synthetic photos: a page of "text" (rows of word-sized
//! bars), photographed at an angle, skewed, sideways, or under a shadow
//! with a warm color cast.

use super::*;
use in_progress::NoProgress;

const PAGE: (u32, u32) = (850, 1100);

/// A white page with lines of black word bars.
fn page() -> RgbImage {
    let (w, h) = PAGE;
    let mut img = RgbImage::from_pixel(w, h, Rgb([255, 255, 255]));
    let mut seed = 7u32;
    let mut rand = move |n: u32| {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        (seed >> 16) % n
    };
    let mut y = 80;
    while y + 10 < h - 80 {
        let mut x = 70;
        while x < w - 120 {
            let len = 20 + rand(70);
            for yy in y..y + 9 {
                for xx in x..(x + len).min(w - 70) {
                    img.put_pixel(xx, yy, Rgb([20, 20, 20]));
                }
            }
            x += len + 12;
        }
        y += 26;
    }
    img
}

/// `page` seen in a photo of `size`, its corners at `quad`, on `bg`.
fn photo(page: &RgbImage, quad: Quad, size: (u32, u32), bg: [u8; 3]) -> RgbImage {
    let (w, h) = (page.width() as f32, page.height() as f32);
    let proj =
        Projection::from_control_points([(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)], quad).unwrap();
    let mut out = RgbImage::new(size.0, size.1);
    imageproc::geometric_transformations::warp_into(
        page,
        proj,
        Interpolation::Bilinear,
        Border::Constant(Rgb(bg)),
        &mut out,
    );
    out
}

/// Darken left to right down to `low`, and tint warm.
fn shadow(img: &mut RgbImage, low: f32) {
    let w = img.width() as f32;
    for (x, _, p) in img.enumerate_pixels_mut() {
        let f = 1.0 - (1.0 - low) * (x as f32 / w);
        let tint = [1.0, 0.92, 0.78];
        for c in 0..3 {
            p[c] = (p[c] as f32 * f * tint[c]) as u8;
        }
    }
}

/// A page seen from a little below and to the left: proportions near
/// the page's own, as a phone held over it sees.
const ANGLED: Quad = [
    (350.0, 120.0),
    (1130.0, 160.0),
    (1180.0, 1170.0),
    (300.0, 1130.0),
];

#[test]
fn finds_a_page_shot_at_an_angle() {
    let img = photo(&page(), ANGLED, (1600, 1300), [90, 80, 70]);
    let q = find_page(&img).expect("a page");
    for (got, want) in q.iter().zip(ANGLED.iter()) {
        let d = ((got.0 - want.0).powi(2) + (got.1 - want.1).powi(2)).sqrt();
        assert!(d < 16.0, "corner {got:?} vs {want:?}");
    }
    let flat = unwarp(&img, q);
    let aspect = flat.width() as f32 / flat.height() as f32;
    let want = PAGE.0 as f32 / PAGE.1 as f32;
    assert!((aspect - want).abs() < 0.06, "aspect {aspect} vs {want}");
}

#[test]
fn a_photo_that_is_all_page_is_left_alone() {
    assert_eq!(find_page(&page()), None);
}

#[test]
fn measures_and_removes_skew() {
    let skewed = rotate(&page(), 4.0);
    let a = skew_angle(&small_gray(&skewed));
    assert!((a - 4.0).abs() <= 0.3, "measured {a}");
    let level = rotate(&skewed, -a);
    let b = skew_angle(&small_gray(&level));
    assert!(b.abs() <= 0.3, "left {b}");
    assert_eq!(skew_angle(&small_gray(&page())), 0.0);
}

#[test]
fn notices_a_sideways_page() {
    assert_eq!(quarter_turns(&small_gray(&page())), 0);
    assert_eq!(quarter_turns(&small_gray(&turn(page(), 1))), 1);
    assert_eq!(quarter_turns(&small_gray(&turn(page(), 3))), 1);
}

#[test]
fn flattening_whitens_shadowed_paper() {
    let mut img = page();
    shadow(&mut img, 0.45);
    // Before: the shadowed margin is dark and warm.
    assert!(img.get_pixel(800, 40)[2] < 120);
    let flat = flatten_light(&img);
    for &(x, y) in &[(20, 40), (800, 40), (800, 1060), (430, 1080)] {
        let p = flat.get_pixel(x, y);
        assert!(p.0.iter().all(|&v| v >= 235), "paper at {x},{y} is {p:?}");
    }
    // Ink on the dark side stays ink.
    let p = flat.get_pixel(75, 84);
    assert!(p.0.iter().all(|&v| v <= 90), "ink is {p:?}");
}

#[test]
fn black_and_white_without_specks() {
    let mut gray = DynamicImage::ImageRgb8(page()).into_luma8();
    // Dust: lone dark pixels in the margin.
    for i in 0..40 {
        gray.put_pixel(10 + i * 3, 30, Luma([30]));
    }
    let mut bw = sauvola(&gray, 31, 0.25);
    assert_eq!(bw.get_pixel(13, 30)[0], 0);
    despeckle(&mut bw, 4);
    assert_eq!(bw.get_pixel(13, 30)[0], 255);
    // A word bar survives, and the paper is white.
    assert_eq!(bw.get_pixel(75, 84)[0], 0);
    assert_eq!(bw.get_pixel(30, 600)[0], 255);
}

#[test]
fn the_whole_pipeline() {
    let mut img = photo(&rotate(&page(), 3.0), ANGLED, (1600, 1300), [90, 80, 70]);
    shadow(&mut img, 0.55);
    let params = ScanParams {
        mode: Mode::Bw,
        fit: Some((850, 1100)),
        ..Default::default()
    };
    let out = clean(img, &params, Models::default(), &NoProgress).unwrap();
    assert!(out.quad.is_some());
    assert_eq!(out.turns, 0);
    assert!((out.skew - 3.0).abs() <= 0.5, "skew {}", out.skew);
    let bw = out.image.into_luma8();
    let (w, h) = bw.dimensions();
    assert!(w <= 850 && h <= 1100 && (w == 850 || h == 1100), "{w}×{h}");
    let ink = bw.pixels().filter(|p| p[0] == 0).count() as f32 / (w * h) as f32;
    // The page is about a quarter ink; no black borders or shadows.
    assert!((0.1..0.4).contains(&ink), "ink {ink}");
}

#[test]
fn params_from_json() {
    let p: ScanParams = serde_json::from_str(r#"{"mode":"bw","deskew":false}"#).unwrap();
    assert_eq!(p.mode, Mode::Bw);
    assert!(!p.deskew && p.crop && p.flatten);
}

#[test]
fn a_curled_page_is_cropped_and_its_edges_cleared() {
    // Each column of the page dropped by a sine: edges bow, lines arc.
    let p = page();
    let (w, h) = p.dimensions();
    let lift = 50;
    let mut img = RgbImage::from_pixel(w + 300, h + 2 * lift + 300, Rgb([80, 60, 45]));
    for x in 0..w {
        let dy = (lift as f32 * (std::f32::consts::PI * x as f32 / w as f32).sin()) as u32;
        for y in 0..h {
            img.put_pixel(x + 150, y + 150 + lift - dy, *p.get_pixel(x, y));
        }
    }
    assert!(find_page(&img).is_some(), "falls back to a rectangle");
    let params = ScanParams {
        mode: Mode::Bw,
        deskew: false,
        ..Default::default()
    };
    let bw = clean(img, &params, Models::default(), &NoProgress)
        .unwrap()
        .image
        .into_luma8();
    let (w, h) = bw.dimensions();
    // No background left along the edges.
    let border = (0..w)
        .flat_map(|x| [(x, 0), (x, h - 1)])
        .chain((0..h).flat_map(|y| [(0, y), (w - 1, y)]));
    let dark = border.filter(|&(x, y)| bw.get_pixel(x, y)[0] == 0).count();
    assert_eq!(dark, 0);
}

#[test]
fn clearing_edges_keeps_content_that_runs_off_the_page() {
    // A close-up: a table whose rules run off the photo on every side,
    // and a strip of dark background along the top.
    let (w, h) = (800u32, 600u32);
    let mut img = RgbImage::from_pixel(w, h, Rgb([235, 230, 220]));
    for y in (40..h).step_by(60) {
        for x in 0..w {
            for t in 0..3 {
                img.put_pixel(x, y + t, Rgb([20, 20, 20]));
            }
        }
    }
    for x in (50..w).step_by(120) {
        for y in 40..h {
            for t in 0..3 {
                img.put_pixel(x + t, y, Rgb([20, 20, 20]));
            }
        }
    }
    for y in 0..14 {
        for x in 0..w {
            img.put_pixel(x, y, Rgb([70, 60, 50]));
        }
    }
    clear_edges(&mut img, 0.25);
    // The table's rules are still there, the strip is gone, and what
    // replaced it is the paper's color, not white.
    assert!(img.get_pixel(400, 101)[0] < 60, "rule erased");
    assert!(img.get_pixel(171, 300)[0] < 60, "rule erased");
    let strip = img.get_pixel(400, 5);
    assert!(strip[0] > 200 && strip[0] < 250, "strip is {strip:?}");
}
