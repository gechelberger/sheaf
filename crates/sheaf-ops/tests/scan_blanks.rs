//! Scanning with blank pages left out.

use image::{Rgb, RgbImage};
use lopdf::Document as Pdf;
use sheaf_ocr::OcrConfig;
use sheaf_ops::scan::{ScanOptions, scan};
use sheaf_ops::{NoProgress, Tracker};

/// An off-white page with a little sensor noise, and lines of "text"
/// when `text`.
fn page(text: bool, seed: u32) -> RgbImage {
    let mut seed = seed;
    let mut img = RgbImage::from_fn(850, 1100, |_, _| {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        let v = 238 + ((seed >> 16) % 9) as u8;
        Rgb([v, v, v.saturating_sub(4)])
    });
    if text {
        for line in 0..30 {
            for x in 100..700 {
                for y in 0..10 {
                    if (x / 40) % 5 != 4 {
                        img.put_pixel(x, 120 + line * 28 + y, Rgb([30, 30, 30]));
                    }
                }
            }
        }
    }
    img
}

#[test]
fn blank_pages_are_left_out() {
    let dir = std::env::temp_dir().join(format!("sheaf-scan-blanks-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let inputs: Vec<_> = [true, false, true]
        .iter()
        .enumerate()
        .map(|(i, &text)| {
            let p = dir.join(format!("{i}.png"));
            page(text, i as u32 + 7).save(&p).unwrap();
            p
        })
        .collect();
    let mut opts = ScanOptions {
        drop_blanks: true,
        ..Default::default()
    };
    opts.clean.crop = false;
    opts.clean.orient = false;
    opts.clean.deskew = false;
    let out = dir.join("out.pdf");
    let tracker = Tracker::new();
    let report = scan(&inputs, &opts, &OcrConfig::default(), &out, &tracker).unwrap();
    assert_eq!((report.pages, report.dropped), (2, 1));
    // Each page's step reports to the whole scan's bar and status.
    let s = tracker.snapshot();
    assert_eq!((s.fraction, s.message_count), (Some(1.0), 1));
    assert!(s.message.unwrap().1.ends_with("1.png: blank, left out"));
    assert_eq!(Pdf::load(&out).unwrap().get_pages().len(), 2);

    // Off, every page stays.
    opts.drop_blanks = false;
    let report = scan(&inputs, &opts, &OcrConfig::default(), &out, &NoProgress).unwrap();
    assert_eq!((report.pages, report.dropped), (3, 0));

    // All blank is an error, not an empty file.
    opts.drop_blanks = true;
    assert!(
        scan(
            &inputs[1..2],
            &opts,
            &OcrConfig::default(),
            &out,
            &NoProgress
        )
        .is_err()
    );
    let _ = std::fs::remove_dir_all(&dir);
}
