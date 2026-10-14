//! `sheaf-cli images-to-pdf` end to end, through the binary.

use std::process::Command;

#[test]
fn images_become_pages() {
    let dir = std::env::temp_dir().join(format!("sheaf-cli-images-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.jpg");
    let b = dir.join("b.png");
    image::RgbImage::from_pixel(200, 100, image::Rgb([200, 30, 30]))
        .save(&a)
        .unwrap();
    image::GrayImage::from_pixel(50, 50, image::Luma([0]))
        .save(&b)
        .unwrap();
    let out = dir.join("out.pdf");
    let run = Command::new(env!("CARGO_BIN_EXE_sheaf-cli"))
        .args(["images-to-pdf", "-q", "--page", "letter", "-o"])
        .arg(&out)
        .args([&a, &b])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let pdf = lopdf::Document::load(&out).unwrap();
    assert_eq!(pdf.get_pages().len(), 2);
    // A second run won't overwrite.
    let again = Command::new(env!("CARGO_BIN_EXE_sheaf-cli"))
        .args(["images-to-pdf", "-o"])
        .arg(&out)
        .arg(&a)
        .output()
        .unwrap();
    assert_eq!(again.status.code(), Some(1));
    let _ = std::fs::remove_dir_all(&dir);
}
