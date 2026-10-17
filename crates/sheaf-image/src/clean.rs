//! Cleaning up scans and phone photos of paper.
//!
//! [`clean`] runs the whole pipeline; each step is also a function of its
//! own so a preview can show them one by one:
//!
//! 1. [`find_page`]: the page's four corners in the photo.
//! 2. [`unwarp`]: the page cut out and made rectangular.
//! 3. [`clear_edges`]: what's left of the background whitened.
//! 4. [`flatten_light`]: shadows and color casts removed, paper made white.
//!    Before deskew, so the white the rotation fills corners with matches.
//!    Then [`quarter_turns`] (and a model, if the caller passes one): which
//!    way is up. Models read flattened pages much better.
//! 5. [`skew_angle`] and [`rotate`]: text lines made level.
//! 6. [`sauvola`] and [`despeckle`]: black and white, without the dust.
//!
//! Analysis runs on a copy about 1000 pixels across, where noise matters
//! less and it's fast; pixel work runs at full resolution.

use anyhow::Result;
use image::imageops::FilterType;
use image::{DynamicImage, GrayImage, Luma, Rgb, RgbImage};
use imageproc::geometric_transformations::{Border, Interpolation, Projection};
use imageproc::point::Point;
use in_progress::{Progress, Unit};
use serde::{Deserialize, Serialize};

/// Side length that analysis runs at.
const ANALYSIS: u32 = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    #[default]
    Color,
    Gray,
    /// Black and white (Sauvola thresholding, then despeckle).
    Bw,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ScanParams {
    /// Find the page in the photo and cut it out.
    pub crop: bool,
    /// Turn sideways or upside-down pages upright.
    pub orient: bool,
    pub deskew: bool,
    /// Even out lighting and make the paper white.
    pub flatten: bool,
    /// Flatten curled or bent pages with a model (see [`Models::unwarp`]).
    /// Off by default: it can make flat pages wobble slightly.
    pub flatten_curl: bool,
    pub mode: Mode,
    /// Sauvola's k: higher keeps less of faint strokes.
    pub threshold_k: f32,
    /// Drop specks in black-and-white output.
    pub despeckle: bool,
    /// Scale to fit within this many pixels (width, height; the larger
    /// goes with the longer side) before thresholding. At most doubles.
    pub fit: Option<(u32, u32)>,
}

impl Default for ScanParams {
    fn default() -> Self {
        Self {
            crop: true,
            orient: true,
            deskew: true,
            flatten: true,
            flatten_curl: false,
            mode: Mode::Color,
            threshold_k: 0.25,
            despeckle: true,
            fit: None,
        }
    }
}

/// Corners in image pixels: top-left, top-right, bottom-right, bottom-left.
pub type Quad = [(f32, f32); 4];

/// A model that flattens a curled page, or `None` when it can't.
pub type Unwarper<'a> = &'a (dyn Fn(&RgbImage) -> Option<RgbImage> + Sync);

/// The machine-learning models the pipeline can use, when the caller has
/// them. Without them it falls back to what it can do alone.
#[derive(Default, Clone, Copy)]
pub struct Models<'a> {
    pub orient: Option<Orienter<'a>>,
    /// Used only with [`ScanParams::flatten_curl`].
    pub unwarp: Option<Unwarper<'a>>,
}

/// Asks models which way is up: given the page at full resolution, votes
/// for k = 0..4 clockwise quarter turns making it upright (higher is
/// surer); `None` when they can't say.
pub type Orienter<'a> = &'a (dyn Fn(&RgbImage) -> Option<[f32; 4]> + Sync);

/// How much the text's direction (word shapes) adds to the two turns on
/// its axis: a tiebreak, since drawings have labels running both ways.
const AXIS_PRIOR: f32 = 0.15;

/// What [`clean`] did, with the result.
pub struct Cleaned {
    pub image: DynamicImage,
    /// Where the page was in the photo, if it was found.
    pub quad: Option<Quad>,
    /// Clockwise quarter turns applied.
    pub turns: u16,
    /// Degrees of skew removed (clockwise positive).
    pub skew: f32,
}

/// A point in the pipeline, for previews.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    /// The page cut out of the photo, edges cleared.
    Cropped,
    /// A curled page flattened by the model.
    Uncurled,
    /// Lighting evened out.
    Flattened,
    /// Turned upright.
    Upright,
    /// Text made level.
    Straightened,
}

/// The whole pipeline, as `params` says.
pub fn clean(
    image: RgbImage,
    params: &ScanParams,
    models: Models,
    progress: &dyn Progress,
) -> Result<Cleaned> {
    clean_with(image, params, models, progress, &mut |_, _| {})
}

/// [`clean`], showing `stage` the image after each step that ran.
pub fn clean_with(
    image: RgbImage,
    params: &ScanParams,
    models: Models,
    progress: &dyn Progress,
    stage: &mut dyn FnMut(Stage, &RgbImage),
) -> Result<Cleaned> {
    progress.set_total(Some(6), Unit::ITEMS);
    let mut image = image;
    let mut quad = None;
    if params.crop
        && let Some(q) = find_page(&image)
    {
        image = inset(unwarp(&image, q), 0.0075);
        quad = Some(q);
    }
    progress.set(1);
    progress.check()?;
    if params.flatten_curl
        && let Some(model) = models.unwarp
        && let Some(flat) = model(&image)
    {
        image = flat;
        stage(Stage::Uncurled, &image);
    }
    if quad.is_some() {
        clear_edges(&mut image, params.threshold_k);
        stage(Stage::Cropped, &image);
    }
    progress.set(2);
    progress.check()?;
    if params.flatten {
        image = flatten_light(&image);
        stage(Stage::Flattened, &image);
    }
    let mut turns = 0;
    if params.orient {
        // Word shapes give the axis; models, when there are any, vote
        // on all four turns, with the axis as a tiebreak.
        let axis = quarter_turns(&small_gray(&image));
        turns = match models.orient.and_then(|model| model(&image)) {
            Some(votes) => (0..4u16)
                .max_by(|&a, &b| {
                    let v =
                        |k: u16| votes[k as usize] + if k % 2 == axis { AXIS_PRIOR } else { 0.0 };
                    v(a).total_cmp(&v(b))
                })
                .unwrap_or(axis),
            None => axis,
        };
        image = turn(image, turns);
        stage(Stage::Upright, &image);
    }
    progress.set(3);
    progress.check()?;
    let mut skew = 0.0;
    if params.deskew {
        skew = skew_angle(&small_gray(&image));
        if skew.abs() >= 0.05 {
            image = rotate(&image, -skew);
        }
        stage(Stage::Straightened, &image);
    }
    progress.set(4);
    progress.check()?;
    if let Some(fit) = params.fit {
        image = fit_within(image, fit);
    }
    progress.set(5);
    progress.check()?;
    let image = match params.mode {
        Mode::Color => DynamicImage::ImageRgb8(image),
        Mode::Gray => DynamicImage::ImageLuma8(DynamicImage::ImageRgb8(image).into_luma8()),
        Mode::Bw => {
            let gray = DynamicImage::ImageRgb8(image).into_luma8();
            let mut bw = sauvola(&gray, window_for(&gray), params.threshold_k);
            if params.despeckle {
                let speck = speck_area(&bw);
                despeckle(&mut bw, speck);
            }
            DynamicImage::ImageLuma8(bw)
        }
    };
    progress.set(6);
    Ok(Cleaned {
        image,
        quad,
        turns,
        skew,
    })
}

/// The largest speck worth dropping: half a millimeter across at 300 dpi
/// on a Letter page, scaled to this one.
fn speck_area(img: &GrayImage) -> u32 {
    ((img.width() as u64 * img.height() as u64) / 200_000).max(4) as u32
}

fn scale_for(w: u32, h: u32) -> f32 {
    (ANALYSIS as f32 / w.max(h) as f32).min(1.0)
}

fn small_rgb(img: &RgbImage) -> RgbImage {
    let s = scale_for(img.width(), img.height());
    if s >= 1.0 {
        return img.clone();
    }
    let (w, h) = (
        (img.width() as f32 * s).round().max(1.0) as u32,
        (img.height() as f32 * s).round().max(1.0) as u32,
    );
    image::imageops::resize(img, w, h, FilterType::Triangle)
}

fn small_gray(img: &RgbImage) -> GrayImage {
    DynamicImage::ImageRgb8(small_rgb(img)).into_luma8()
}

// ----- 1. the page ----------------------------------------------------------

/// The page's corners, when there's a clear four-sided page covering at
/// least a fifth of the photo. Tries the page's edges first, then its
/// brightness against the background.
pub fn find_page(img: &RgbImage) -> Option<Quad> {
    let s = scale_for(img.width(), img.height());
    let gray = small_gray(img);
    let frame = gray.width() as f64 * gray.height() as f64;
    let blurred = imageproc::filter::gaussian_blur_f32(&gray, 2.0);
    let (w, h) = (gray.width() as f32, gray.height() as f32);
    // A page that runs well out of the photo (a close-up of part of a
    // sheet) isn't one we can cut out: its "corners" are guesses.
    let slack = 0.02 * w.max(h);
    let inside = |q: &Quad| {
        q.iter()
            .all(|&(x, y)| x > -slack && y > -slack && x < w + slack && y < h + slack)
    };
    let mut best: Option<(f64, Quad)> = None;
    let mut consider = |mask: &GrayImage, shrink: f32| {
        for (area, q) in quads_in(mask, frame, shrink) {
            if inside(&q) && best.as_ref().is_none_or(|(a, _)| area > *a) {
                best = Some((area, q));
            }
        }
    };
    // Edges, joined up.
    let edges = imageproc::edges::canny(&blurred, 20.0, 60.0);
    let edges = imageproc::morphology::dilate(&edges, imageproc::distance_transform::Norm::LInf, 2);
    consider(&edges, 5.0);
    // Paper is brighter than what it lies on.
    let level = imageproc::contrast::otsu_level(&blurred);
    let mut bright =
        imageproc::contrast::threshold(&blurred, level, imageproc::contrast::ThresholdType::Binary);
    bright = imageproc::morphology::open(&bright, imageproc::distance_transform::Norm::LInf, 3);
    consider(&bright, 0.0);
    // No clean four-sided outline (a curled page's edges bow): the
    // smallest rectangle around the largest outline, and clear_edges
    // whitens the background it takes in.
    if best.is_none() {
        best = rect_fallback(&edges, frame)
            .or_else(|| rect_fallback(&bright, frame))
            .filter(|(_, q)| inside(q));
    }
    let (_, q) = best?;
    // A quad hugging the whole frame means the photo is the page already.
    let margin = 0.01 * w.max(h);
    let hugs = q
        .iter()
        .all(|&(x, y)| (x < margin || x > w - margin) && (y < margin || y > h - margin));
    if hugs {
        return None;
    }
    Some(q.map(|(x, y)| (x / s, y / s)))
}

/// Four-sided convex outlines in a mask covering 20%+ of the frame, with
/// their areas. `shrink` pulls corners in by that many pixels (the
/// dilation around edges).
fn quads_in(mask: &GrayImage, frame: f64, shrink: f32) -> Vec<(f64, Quad)> {
    use imageproc::contours::{BorderType, find_contours};
    use imageproc::geometry::{approximate_polygon_dp, arc_length, contour_area, convex_hull};
    let mut out = Vec::new();
    for c in find_contours::<i32>(mask) {
        if c.border_type != BorderType::Outer || c.points.len() < 4 {
            continue;
        }
        let hull = convex_hull(c.points);
        let area = contour_area(&hull).abs();
        if area < 0.2 * frame || hull.len() < 4 {
            continue;
        }
        let poly = approximate_polygon_dp(&hull, 0.02 * arc_length(&hull, true), true);
        if poly.len() != 4 {
            continue;
        }
        let q = order_corners(&poly);
        out.push((area, pull_in(q, shrink)));
    }
    out
}

/// The minimum-area rectangle around the largest outline covering 20%+
/// of the frame.
fn rect_fallback(mask: &GrayImage, frame: f64) -> Option<(f64, Quad)> {
    use imageproc::contours::{BorderType, find_contours};
    use imageproc::geometry::{contour_area, convex_hull, min_area_rect};
    find_contours::<i32>(mask)
        .into_iter()
        .filter(|c| c.border_type == BorderType::Outer && c.points.len() >= 4)
        .map(|c| convex_hull(c.points))
        .map(|hull| (contour_area(&hull).abs(), hull))
        .filter(|(area, _)| *area >= 0.2 * frame)
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(area, hull)| (area, order_corners(&min_area_rect(&hull))))
}

/// Top-left, top-right, bottom-right, bottom-left.
fn order_corners(p: &[Point<i32>]) -> Quad {
    let pts: Vec<(f32, f32)> = p.iter().map(|p| (p.x as f32, p.y as f32)).collect();
    let by = |f: &dyn Fn(&(f32, f32)) -> f32, max: bool| {
        *pts.iter()
            .max_by(|a, b| {
                let (fa, fb) = (f(a), f(b));
                if max {
                    fa.total_cmp(&fb)
                } else {
                    fb.total_cmp(&fa)
                }
            })
            .unwrap()
    };
    [
        by(&|p| p.0 + p.1, false),
        by(&|p| p.0 - p.1, true),
        by(&|p| p.0 + p.1, true),
        by(&|p| p.0 - p.1, false),
    ]
}

fn pull_in(q: Quad, by: f32) -> Quad {
    if by == 0.0 {
        return q;
    }
    let cx = q.iter().map(|p| p.0).sum::<f32>() / 4.0;
    let cy = q.iter().map(|p| p.1).sum::<f32>() / 4.0;
    q.map(|(x, y)| {
        let (dx, dy) = (cx - x, cy - y);
        let d = (dx * dx + dy * dy).sqrt().max(1.0);
        (x + dx / d * by, y + dy / d * by)
    })
}

/// The page inside `quad`, made rectangular. Its size is the mean of
/// opposite edge lengths.
pub fn unwarp(img: &RgbImage, quad: Quad) -> RgbImage {
    let dist = |a: (f32, f32), b: (f32, f32)| ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
    let [tl, tr, br, bl] = quad;
    let w = ((dist(tl, tr) + dist(bl, br)) / 2.0).round().max(1.0);
    let h = ((dist(tl, bl) + dist(tr, br)) / 2.0).round().max(1.0);
    let Some(proj) =
        Projection::from_control_points(quad, [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)])
    else {
        return img.clone();
    };
    let mut out = RgbImage::new(w as u32, h as u32);
    imageproc::geometric_transformations::warp_into(
        img,
        proj,
        Interpolation::Bilinear,
        Border::Constant(Rgb([255, 255, 255])),
        &mut out,
    );
    out
}

// ----- 2. which way is up -----------------------------------------------------

/// 0, or 1 when the text runs up and down (the page is sideways): with
/// letters smeared together into words, words lie wide. Can't tell 90°
/// from 270°, or 0° from 180°: that takes a model.
pub fn quarter_turns(gray: &GrayImage) -> u16 {
    let (wide, tall) = word_shapes(gray);
    if tall > wide * 3 / 2 && tall >= 5 {
        1
    } else {
        0
    }
}

/// How many word-sized blobs of ink are wide, and how many tall.
fn word_shapes(gray: &GrayImage) -> (usize, usize) {
    use imageproc::region_labelling::{Connectivity, connected_components};
    let ink = ink_mask(gray);
    let merged = imageproc::morphology::dilate(&ink, imageproc::distance_transform::Norm::LInf, 2);
    let labels = connected_components(&merged, Connectivity::Eight, Luma([0u8]));
    let mut boxes: Vec<[u32; 4]> = Vec::new();
    for (x, y, l) in labels.enumerate_pixels() {
        let l = l[0] as usize;
        if l == 0 {
            continue;
        }
        if l > boxes.len() {
            boxes.resize(l, [u32::MAX, u32::MAX, 0, 0]);
        }
        let b = &mut boxes[l - 1];
        *b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
    }
    let limit = gray.width().max(gray.height()) / 3;
    let (mut wide, mut tall) = (0, 0);
    for [x0, y0, x1, y1] in boxes {
        let (w, h) = (x1 + 1 - x0, y1 + 1 - y0);
        // Dots and specks say nothing; rules and pictures aren't words.
        if w.max(h) < 8 || w.max(h) > limit {
            continue;
        }
        if w * 2 > h * 3 {
            wide += 1;
        } else if h * 2 > w * 3 {
            tall += 1;
        }
    }
    (wide, tall)
}

/// Dark pixels (ink) as 255, the rest 0.
fn ink_mask(gray: &GrayImage) -> GrayImage {
    let flat = flatten_gray(gray);
    let level = imageproc::contrast::otsu_level(&flat).min(200);
    GrayImage::from_fn(flat.width(), flat.height(), |x, y| {
        Luma([if flat.get_pixel(x, y)[0] < level {
            255
        } else {
            0
        }])
    })
}

/// Turn clockwise by `turns` quarters.
pub fn turn(img: RgbImage, turns: u16) -> RgbImage {
    match turns % 4 {
        1 => image::imageops::rotate90(&img),
        2 => image::imageops::rotate180(&img),
        3 => image::imageops::rotate270(&img),
        _ => img,
    }
}

// ----- 3. skew ---------------------------------------------------------------

/// How far text lines slope, in degrees clockwise (−15 to 15): the angle
/// at which ink lines up into the sharpest rows.
pub fn skew_angle(gray: &GrayImage) -> f32 {
    let ink = ink_mask(gray);
    let mut pts: Vec<(f32, f32)> = ink
        .enumerate_pixels()
        .filter(|(_, _, p)| p[0] != 0)
        .map(|(x, y, _)| (x as f32, y as f32))
        .collect();
    if pts.len() < 50 {
        return 0.0;
    }
    // Plenty of ink: a regular sample keeps it quick.
    let step = pts.len().div_ceil(60_000);
    if step > 1 {
        pts = pts.into_iter().step_by(step).collect();
    }
    let diag = ((ink.width().pow(2) + ink.height().pow(2)) as f32).sqrt();
    let score = |deg: f32| -> f64 {
        let (s, c) = deg.to_radians().sin_cos();
        let mut bins = vec![0u32; (2.0 * diag) as usize + 2];
        for &(x, y) in &pts {
            let r = y * c - x * s + diag;
            bins[r as usize] += 1;
        }
        bins.iter().map(|&n| (n as f64).powi(2)).sum()
    };
    let best_in = |from: f32, to: f32, step: f32| {
        let mut best = (f64::MIN, 0.0);
        let mut a = from;
        while a <= to + 1e-4 {
            let v = score(a);
            if v > best.0 {
                best = (v, a);
            }
            a += step;
        }
        best.1
    };
    let coarse = best_in(-15.0, 15.0, 1.0);
    let fine = best_in(coarse - 1.0, coarse + 1.0, 0.1);
    (fine * 10.0).round() / 10.0
}

/// Rotate clockwise by `degrees` about the center, white where the
/// corners were.
pub fn rotate(img: &RgbImage, degrees: f32) -> RgbImage {
    imageproc::geometric_transformations::rotate_about_center(
        img,
        degrees.to_radians(),
        Interpolation::Bilinear,
        Border::Constant(Rgb([255, 255, 255])),
    )
}

// ----- 4. light -----------------------------------------------------------------

/// Divide each channel by its estimated background (the paper without
/// the ink), which takes out shadows, uneven light and color casts and
/// leaves the paper white; then stretch so the ink is black.
pub fn flatten_light(img: &RgbImage) -> RgbImage {
    let (w, h) = img.dimensions();
    let small = small_rgb(img);
    let r = (small.width().max(small.height()) / 100).max(3);
    let mut bgs = Vec::with_capacity(3);
    for c in 0..3 {
        let ch = GrayImage::from_fn(small.width(), small.height(), |x, y| {
            Luma([small.get_pixel(x, y)[c]])
        });
        let bg = background(&ch, r);
        bgs.push(image::imageops::resize(&bg, w, h, FilterType::Triangle));
    }
    let mut out = RgbImage::new(w, h);
    for (x, y, p) in out.enumerate_pixels_mut() {
        let src = img.get_pixel(x, y);
        for c in 0..3 {
            let bg = bgs[c].get_pixel(x, y)[0].max(1) as u32;
            p[c] = (src[c] as u32 * 255 / bg).min(255) as u8;
        }
    }
    stretch(&mut out);
    out
}

/// The same for one gray channel (used for analysis).
fn flatten_gray(gray: &GrayImage) -> GrayImage {
    let r = (gray.width().max(gray.height()) / 100).max(3);
    let bg = background(gray, r);
    GrayImage::from_fn(gray.width(), gray.height(), |x, y| {
        let b = bg.get_pixel(x, y)[0].max(1) as u32;
        Luma([(gray.get_pixel(x, y)[0] as u32 * 255 / b).min(255) as u8])
    })
}

/// Paper without ink: a max filter wipes out strokes thinner than `r`,
/// then a median smooths what's left.
fn background(ch: &GrayImage, r: u32) -> GrayImage {
    let maxed = max_filter(ch, r);
    imageproc::filter::median_filter(&maxed, r * 2, r * 2)
}

/// Separable maximum over a (2r+1)² square.
fn max_filter(img: &GrayImage, r: u32) -> GrayImage {
    let (w, h) = img.dimensions();
    let r = r as i64;
    let mut rows = GrayImage::new(w, h);
    for y in 0..h {
        for x in 0..w as i64 {
            let (a, b) = ((x - r).max(0), (x + r).min(w as i64 - 1));
            let m = (a..=b)
                .map(|i| img.get_pixel(i as u32, y)[0])
                .max()
                .unwrap_or(0);
            rows.put_pixel(x as u32, y, Luma([m]));
        }
    }
    let mut out = GrayImage::new(w, h);
    for x in 0..w {
        for y in 0..h as i64 {
            let (a, b) = ((y - r).max(0), (y + r).min(h as i64 - 1));
            let m = (a..=b)
                .map(|i| rows.get_pixel(x, i as u32)[0])
                .max()
                .unwrap_or(0);
            out.put_pixel(x, y as u32, Luma([m]));
        }
    }
    out
}

/// Map the darkest half percent of brightness to black and the median
/// (paper, on a page of text) to white.
fn stretch(img: &mut RgbImage) {
    let mut hist = [0u64; 256];
    for p in img.pixels() {
        let l = crate::raster::luma(p[0], p[1], p[2]);
        hist[l as usize] += 1;
    }
    let total: u64 = hist.iter().sum();
    let at = |frac_x1000: u64| {
        let mut seen = 0;
        for (i, n) in hist.iter().enumerate() {
            seen += n;
            if seen * 1000 >= total * frac_x1000 {
                return i as u32;
            }
        }
        255
    };
    let lo = at(5).min(128);
    // A page mostly of pictures has a darker median: don't blow it out.
    let hi = at(500).clamp(200, 255);
    if lo == 0 && hi == 255 {
        return;
    }
    let span = (hi - lo).max(1);
    for p in img.pixels_mut() {
        for c in 0..3 {
            p[c] = ((p[c] as u32).saturating_sub(lo) * 255 / span).min(255) as u8;
        }
    }
}

fn fit_within(img: RgbImage, (fw, fh): (u32, u32)) -> RgbImage {
    let (w, h) = img.dimensions();
    // The longer side goes with the longer limit.
    let (fw, fh) = if (w > h) == (fw > fh) {
        (fw, fh)
    } else {
        (fh, fw)
    };
    let s = (fw as f32 / w as f32).min(fh as f32 / h as f32).min(2.0);
    if (s - 1.0).abs() < 0.01 {
        return img;
    }
    let (nw, nh) = (
        (w as f32 * s).round().max(1.0) as u32,
        (h as f32 * s).round().max(1.0) as u32,
    );
    image::imageops::resize(&img, nw, nh, FilterType::CatmullRom)
}

// ----- 5. black and white ---------------------------------------------------

/// Sauvola's local threshold: a pixel is ink when darker than
/// `m·(1 + k·(s/128 − 1))` over the `window`-square around it (`m` mean,
/// `s` standard deviation). Returns 0 for ink, 255 for paper.
pub fn sauvola(gray: &GrayImage, window: u32, k: f32) -> GrayImage {
    let (w, h) = (gray.width() as usize, gray.height() as usize);
    // Integral images of v and v², one row and column of zeros in front.
    let stride = w + 1;
    let mut sum = vec![0u64; stride * (h + 1)];
    let mut sq = vec![0u64; stride * (h + 1)];
    for y in 0..h {
        let (mut rs, mut rq) = (0u64, 0u64);
        for x in 0..w {
            let v = gray.get_pixel(x as u32, y as u32)[0] as u64;
            rs += v;
            rq += v * v;
            sum[(y + 1) * stride + x + 1] = sum[y * stride + x + 1] + rs;
            sq[(y + 1) * stride + x + 1] = sq[y * stride + x + 1] + rq;
        }
    }
    let r = (window / 2) as usize;
    let k = k as f64;
    GrayImage::from_fn(w as u32, h as u32, |x, y| {
        let (x, y) = (x as usize, y as usize);
        let (x0, x1) = (x.saturating_sub(r), (x + r + 1).min(w));
        let (y0, y1) = (y.saturating_sub(r), (y + r + 1).min(h));
        let n = ((x1 - x0) * (y1 - y0)) as f64;
        let area = |t: &[u64]| {
            (t[y1 * stride + x1] + t[y0 * stride + x0] - t[y0 * stride + x1] - t[y1 * stride + x0])
                as f64
        };
        let m = area(&sum) / n;
        let s = (area(&sq) / n - m * m).max(0.0).sqrt();
        let t = m * (1.0 + k * (s / 128.0 - 1.0));
        Luma([if (gray.get_pixel(x as u32, y as u32)[0] as f64) <= t {
            0
        } else {
            255
        }])
    })
}

/// Cut `frac` of the width and height off each side: the last of the
/// background a crop can leave at the page's edges.
fn inset(img: RgbImage, frac: f32) -> RgbImage {
    let (w, h) = img.dimensions();
    let (dx, dy) = ((w as f32 * frac) as u32, (h as f32 * frac) as u32);
    if dx == 0 && dy == 0 {
        return img;
    }
    image::imageops::crop_imm(&img, dx, dy, w - 2 * dx, h - 2 * dy).to_image()
}

/// How far in from the edges leftover background can reach: 6% of the
/// shorter side.
const EDGE_BAND: f32 = 0.06;

/// Sauvola's window: 2% of the page's width.
fn window_for(gray: &GrayImage) -> u32 {
    (gray.width() / 50).max(15) | 1
}

/// Paint white whatever dark touches the edges of a cropped page: what's
/// left of the background around it. Dark means ink to Sauvola, or well
/// below the paper's brightness nearby: Sauvola alone misses a wide
/// uniform strip, since its window then sees nothing else.
pub fn clear_edges(img: &mut RgbImage, k: f32) {
    let gray = DynamicImage::ImageRgb8(img.clone()).into_luma8();
    let mut bw = sauvola(&gray, window_for(&gray), k);
    // The paper's own color nearby (a wide max filter, so gradual shadows
    // count as paper and thin strips of background don't).
    let small = small_rgb(img);
    let r = (small.width().max(small.height()) / 25).max(3);
    let channels: Vec<GrayImage> = (0..3)
        .map(|c| {
            let ch = GrayImage::from_fn(small.width(), small.height(), |x, y| {
                Luma([small.get_pixel(x, y)[c]])
            });
            let bg = imageproc::filter::median_filter(&max_filter(&ch, r), r, r);
            image::imageops::resize(&bg, gray.width(), gray.height(), FilterType::Triangle)
        })
        .collect();
    let paper = |x: u32, y: u32| Rgb([0, 1, 2].map(|c| channels[c].get_pixel(x, y)[0]));
    for (x, y, p) in bw.enumerate_pixels_mut() {
        let pp = paper(x, y);
        let lum = crate::raster::luma(pp[0], pp[1], pp[2]) as u32;
        if (gray.get_pixel(x, y)[0] as u32) * 10 < lum * 6 {
            p[0] = 0;
        }
    }
    // Paint it the paper's color, not white: flattening then whitens it
    // with the paper around it instead of leaving a halo.
    for (x, y) in edge_ink(&bw) {
        img.put_pixel(x, y, paper(x, y));
    }
}

/// Ink pixels (0 in `bw`) in shapes that touch the image's edge: what's
/// left of the background after a crop, not the page's content.
pub fn edge_ink(bw: &GrayImage) -> Vec<(u32, u32)> {
    use imageproc::region_labelling::{Connectivity, connected_components};
    let (w, h) = bw.dimensions();
    let labels = connected_components(bw, Connectivity::Eight, Luma([255u8]));
    let mut touching = std::collections::HashSet::new();
    for x in 0..w {
        touching.insert(labels.get_pixel(x, 0)[0]);
        touching.insert(labels.get_pixel(x, h - 1)[0]);
    }
    for y in 0..h {
        touching.insert(labels.get_pixel(0, y)[0]);
        touching.insert(labels.get_pixel(w - 1, y)[0]);
    }
    touching.remove(&0);
    if touching.is_empty() {
        return Vec::new();
    }
    // Background left by a crop hugs the edges; a table or a drawing's
    // frame that runs off the photo reaches well inside. Keep only shapes
    // that are nearly all within a band along the edges.
    let band = (w.min(h) as f32 * EDGE_BAND) as u32;
    let near = |x: u32, y: u32| x < band || y < band || x >= w - band || y >= h - band;
    let mut counts: std::collections::HashMap<u32, (u64, u64)> = Default::default();
    for (x, y, l) in labels.enumerate_pixels() {
        if touching.contains(&l[0]) {
            let c = counts.entry(l[0]).or_default();
            c.0 += 1;
            if near(x, y) {
                c.1 += 1;
            }
        }
    }
    let background: std::collections::HashSet<u32> = counts
        .into_iter()
        .filter(|(_, (all, inside))| *inside * 10 >= *all * 9)
        .map(|(l, _)| l)
        .collect();
    labels
        .enumerate_pixels()
        .filter(|(_, _, l)| background.contains(&l[0]))
        .map(|(x, y, _)| (x, y))
        .collect()
}

/// Turn black specks of fewer than `max_area` pixels white.
pub fn despeckle(bw: &mut GrayImage, max_area: u32) {
    use imageproc::region_labelling::{Connectivity, connected_components};
    let labels = connected_components(bw, Connectivity::Eight, Luma([255u8]));
    let mut sizes: Vec<u32> = Vec::new();
    for p in labels.pixels() {
        let l = p[0] as usize;
        if l >= sizes.len() {
            sizes.resize(l + 1, 0);
        }
        sizes[l] += 1;
    }
    for (x, y, p) in labels.enumerate_pixels() {
        let l = p[0] as usize;
        if l != 0 && sizes[l] < max_area {
            bw.put_pixel(x, y, Luma([255]));
        }
    }
}

#[cfg(test)]
mod tests;
