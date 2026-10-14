//! Export pages to image or text files (File > Export Pages, `sheaf
//! export`). The pages go through the same file printing makes (`sheaf_doc::save::print_pdf`), so sheaf's own
//! marks come out as they look, then PDFium renders each at the chosen
//! resolution.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use image::{ImageEncoder, RgbaImage};
use sheaf_core::{PageIndex, Rotation};
use sheaf_doc::{PageId, Snapshot};
use sheaf_progress::{Progress, Unit};
use sheaf_render::pdfium::PdfiumEngine;
use sheaf_render::{Job, PixelFormat, Raster, Renderer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Png,
    /// Quality 1–100.
    Jpeg(u8),
    /// Every page in one multi-page file.
    Tiff(TiffMode),
    /// The pages' text, one file, a form feed between pages.
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TiffMode {
    /// RGB, LZW.
    Color,
    /// 8-bit gray, LZW.
    Gray,
    /// 1-bit, CCITT Group 4: what scanned binders and document systems use.
    BlackWhite,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpeg(_) => "jpg",
            Format::Tiff(_) => "tif",
            Format::Text => "txt",
        }
    }

    /// One file for the whole job (else one per page).
    pub fn single_file(self) -> bool {
        matches!(self, Format::Tiff(_) | Format::Text)
    }
}

/// One export: which pages, how, and where.
#[derive(Debug, Clone)]
pub struct ExportJob {
    pub pages: Vec<PageId>,
    pub markup: bool,
    pub dpi: u32,
    pub format: Format,
    /// One file per page: `<dir>/<stem>_Page_<n>.<ext>`; for a
    /// single-file format, `<dir>/<stem>.<ext>`.
    pub dir: PathBuf,
    pub stem: String,
}

/// `<stem>_Page_<n>`, numbers padded so files sort: `_Page_007` of 120.
pub fn file_name(stem: &str, n: usize, of: usize, ext: &str) -> String {
    let width = of.to_string().len();
    format!("{stem}_Page_{n:0width$}.{ext}")
}

/// Run an export on this thread, one page at a time. A cancel stops it
/// with `Cancelled` before the next page; per-page files already written
/// stay, a single-file export leaves nothing. Returns the files written.
pub fn run(snapshot: &Snapshot, job: &ExportJob, progress: &dyn Progress) -> Result<Vec<PathBuf>> {
    if job.pages.is_empty() {
        bail!("no pages to export");
    }
    progress.check()?;
    let engine = flattened(snapshot, &job.pages, job.markup)?;
    let n = engine.page_count() as usize;
    progress.set_total(Some(n as u64), Unit::Pages);
    std::fs::create_dir_all(&job.dir).with_context(|| format!("creating {}", job.dir.display()))?;
    if job.format == Format::Text {
        let path = job.dir.join(format!("{}.txt", job.stem));
        write_text(&engine, n, &path, progress)?;
        return Ok(vec![path]);
    }
    if let Format::Tiff(mode) = job.format {
        let path = job.dir.join(format!("{}.tif", job.stem));
        write_tiff(&engine, n, job.dpi, mode, &path, progress)?;
        return Ok(vec![path]);
    }
    let mut written = Vec::with_capacity(n);
    for i in 0..n {
        progress.check()?;
        let raster = render_at(&engine, i as u32, job.dpi)?;
        let bytes = encode(raster, job.format, job.dpi)?;
        let path = job
            .dir
            .join(file_name(&job.stem, i + 1, n, job.format.extension()));
        std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
        written.push(path);
        progress.set(i as u64 + 1);
    }
    Ok(written)
}

/// The pages as they print, opened in PDFium.
pub fn flattened(snapshot: &Snapshot, pages: &[PageId], markup: bool) -> Result<PdfiumEngine> {
    let bytes = sheaf_doc::save::print_pdf(snapshot, pages, markup)?;
    let tmp = std::env::temp_dir().join(format!(
        "sheaf-export-{}-{}.pdf",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&tmp, bytes)?;
    // PDFium reads the whole file when it opens.
    let engine = PdfiumEngine::open(&tmp, None);
    let _ = std::fs::remove_file(&tmp);
    engine
}

/// Page `i` at `dpi`, RGBA.
pub fn render_at(engine: &PdfiumEngine, i: u32, dpi: u32) -> Result<Raster> {
    let size = engine.page_size(PageIndex(i))?;
    let width = (size.width * dpi as f32 / 72.0).round().max(1.0) as u32;
    engine.render(
        PageIndex(i),
        Job::Page {
            width,
            rotation: Rotation::R0,
        },
        PixelFormat::Rgba,
    )
}

/// The text of all `n` pages, in reading order as PDFium extracts it,
/// into one UTF-8 file. Pages are separated by a form feed on a line of
/// its own, which `less` and printers treat as a page break.
fn write_text(engine: &PdfiumEngine, n: usize, path: &Path, progress: &dyn Progress) -> Result<()> {
    let mut out = String::new();
    for i in 0..n {
        progress.check()?;
        if i > 0 {
            out.push_str("\u{c}\n");
        }
        out.push_str(&page_text(&engine.page_text(PageIndex(i as u32))?));
        progress.set(i as u64 + 1);
    }
    std::fs::write(path, out).with_context(|| format!("writing {}", path.display()))
}

/// One page's extracted text with `\n` line ends, no trailing spaces, and
/// a final newline.
fn page_text(raw: &str) -> String {
    let mut t = raw.replace("\r\n", "\n").replace('\r', "\n");
    // PDFium marks hyphens it broke a word at with U+0002; keep the hyphen.
    t = t.replace('\u{2}', "-");
    // No trailing spaces on lines (PDFium leaves them at line breaks).
    let lines: Vec<&str> = t.lines().map(str::trim_end).collect();
    let mut s = lines.join("\n").trim_end().to_string();
    if !s.is_empty() {
        s.push('\n');
    }
    s
}

/// All `n` pages of `engine` into one TIFF at `path`.
fn write_tiff(
    engine: &PdfiumEngine,
    n: usize,
    dpi: u32,
    mode: TiffMode,
    path: &Path,
    progress: &dyn Progress,
) -> Result<()> {
    // Write beside the target and rename, so a failure or cancel leaves
    // no half file.
    let tmp = path.with_extension("tif.part");
    let result = write_tiff_to(engine, n, dpi, mode, &tmp, progress);
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result?;
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))
}

fn write_tiff_to(
    engine: &PdfiumEngine,
    n: usize,
    dpi: u32,
    mode: TiffMode,
    tmp: &Path,
    progress: &dyn Progress,
) -> Result<()> {
    use tiff::encoder::{Compression, Rational, TiffEncoder, colortype};
    use tiff::tags::{ResolutionUnit, Tag};
    let file = std::fs::File::create(tmp).with_context(|| format!("creating {}", tmp.display()))?;
    let mut enc =
        TiffEncoder::new(std::io::BufWriter::new(file))?.with_compression(Compression::Lzw);
    let res = || Rational { n: dpi, d: 1 };
    for i in 0..n {
        progress.check()?;
        let raster = render_at(engine, i as u32, dpi)?;
        let (w, h) = (raster.width, raster.height);
        let rgba = RgbaImage::from_raw(w, h, raster.data).context("raster size")?;
        let page = [i as u16, n.min(u16::MAX as usize) as u16];
        match mode {
            TiffMode::Color => {
                let rgb = image::DynamicImage::ImageRgba8(rgba).into_rgb8();
                let mut im = enc.new_image::<colortype::RGB8>(w, h)?;
                im.resolution(ResolutionUnit::Inch, res());
                im.encoder().write_tag(Tag::Unknown(297), &page[..])?;
                im.write_data(rgb.as_raw())?;
            }
            TiffMode::Gray => {
                let gray = image::DynamicImage::ImageRgba8(rgba).into_luma8();
                let mut im = enc.new_image::<colortype::Gray8>(w, h)?;
                im.resolution(ResolutionUnit::Inch, res());
                im.encoder().write_tag(Tag::Unknown(297), &page[..])?;
                im.write_data(gray.as_raw())?;
            }
            TiffMode::BlackWhite => {
                let gray = image::DynamicImage::ImageRgba8(rgba).into_luma8();
                let data = sheaf_doc::images::g4(gray.as_raw(), w, h);
                let mut dir = enc.image_directory()?;
                let offset = dir.write_data(&data[..])?;
                dir.write_tag(Tag::NewSubfileType, 2u32)?;
                dir.write_tag(Tag::ImageWidth, w)?;
                dir.write_tag(Tag::ImageLength, h)?;
                dir.write_tag(Tag::BitsPerSample, 1u16)?;
                // CCITT T.6.
                dir.write_tag(Tag::Compression, 4u16)?;
                // WhiteIsZero, as fax data is.
                dir.write_tag(Tag::PhotometricInterpretation, 0u16)?;
                dir.write_tag(Tag::StripOffsets, u32::try_from(offset)?)?;
                dir.write_tag(Tag::SamplesPerPixel, 1u16)?;
                dir.write_tag(Tag::RowsPerStrip, h)?;
                dir.write_tag(Tag::StripByteCounts, data.len() as u32)?;
                dir.write_tag(Tag::XResolution, res())?;
                dir.write_tag(Tag::YResolution, res())?;
                dir.write_tag(Tag::ResolutionUnit, 2u16)?;
                // T6Options: no uncompressed mode.
                dir.write_tag(Tag::Unknown(293), 0u32)?;
                dir.write_tag(Tag::Unknown(297), &page[..])?;
                dir.finish()?;
            }
        }
        progress.set(i as u64 + 1);
    }
    drop(enc);
    Ok(())
}

/// Encode an RGBA raster, recording its resolution where the format can.
pub fn encode(raster: Raster, format: Format, dpi: u32) -> Result<Vec<u8>> {
    debug_assert_eq!(raster.format, PixelFormat::Rgba);
    let (w, h) = (raster.width, raster.height);
    let rgba = RgbaImage::from_raw(w, h, raster.data).context("raster size")?;
    let mut out = Vec::new();
    match format {
        Format::Png => {
            // Pages are opaque: RGB is a quarter smaller.
            let rgb = image::DynamicImage::ImageRgba8(rgba).into_rgb8();
            let mut enc = png_with_dpi(&mut out, w, h, dpi);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            enc.set_compression(png::Compression::Balanced);
            enc.write_header()?.write_image_data(rgb.as_raw())?;
        }
        Format::Tiff(_) | Format::Text => bail!("this format goes into one file"),
        Format::Jpeg(q) => {
            let rgb = image::DynamicImage::ImageRgba8(rgba).into_rgb8();
            let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(
                Cursor::new(&mut out),
                q.clamp(1, 100),
            );
            enc.set_pixel_density(image::codecs::jpeg::PixelDensity::dpi(
                dpi.min(u16::MAX as u32) as u16,
            ));
            enc.write_image(rgb.as_raw(), w, h, image::ExtendedColorType::Rgb8)?;
        }
    }
    Ok(out)
}

/// A PNG encoder that writes `pHYs` with the resolution.
fn png_with_dpi(out: &mut Vec<u8>, w: u32, h: u32, dpi: u32) -> png::Encoder<'_, &mut Vec<u8>> {
    let mut enc = png::Encoder::new(out, w, h);
    let per_meter = (dpi as f64 / 0.0254).round() as u32;
    enc.set_pixel_dims(Some(png::PixelDimensions {
        xppu: per_meter,
        yppu: per_meter,
        unit: png::Unit::Meter,
    }));
    enc
}

/// Where a single-page export would land, to warn before overwriting.
pub fn existing(job: &ExportJob) -> Vec<PathBuf> {
    if job.format.single_file() {
        let p = job
            .dir
            .join(format!("{}.{}", job.stem, job.format.extension()));
        return if p.exists() { vec![p] } else { Vec::new() };
    }
    let n = job.pages.len();
    (1..=n)
        .map(|i| {
            job.dir
                .join(file_name(&job.stem, i, n, job.format.extension()))
        })
        .filter(|p| p.exists())
        .collect()
}

/// The document's file stem, for naming exports.
pub fn stem_of(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("document")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raster(w: u32, h: u32) -> Raster {
        let mut data = Vec::with_capacity((w * h * 4) as usize);
        for i in 0..w * h {
            data.extend_from_slice(&[(i % 256) as u8, 0, 255, 255]);
        }
        Raster {
            width: w,
            height: h,
            format: PixelFormat::Rgba,
            data,
        }
    }

    #[test]
    fn page_text_normalizes_line_ends() {
        assert_eq!(
            page_text("Form 1040\r\nwage-\u{2}\r\nearner \r\n\r\n"),
            "Form 1040\nwage--\nearner\n"
        );
        assert_eq!(page_text("  \r\n"), "");
    }

    #[test]
    fn names_sort() {
        assert_eq!(file_name("Return", 7, 120, "png"), "Return_Page_007.png");
        assert_eq!(file_name("Return", 1, 1, "jpg"), "Return_Page_1.jpg");
    }

    #[test]
    fn png_keeps_size_and_resolution() {
        let bytes = encode(raster(30, 20), Format::Png, 300).unwrap();
        let dec = png::Decoder::new(Cursor::new(bytes));
        let reader = dec.read_info().unwrap();
        let info = reader.info();
        assert_eq!((info.width, info.height), (30, 20));
        let dims = info.pixel_dims.unwrap();
        assert_eq!(dims.xppu, 11811);
    }

    #[test]
    fn a_cancelled_export_writes_nothing() {
        let dir = std::env::temp_dir().join(format!("sheaf-export-cancel-{}", std::process::id()));
        let job = ExportJob {
            pages: vec![sheaf_doc::PageId(0)],
            markup: false,
            dpi: 72,
            format: Format::Png,
            dir: dir.clone(),
            stem: "x".into(),
        };
        let snapshot = sheaf_doc::Document::new(
            sheaf_doc::SourceInfo {
                path: "none.pdf".into(),
                page_count: 1,
                first_page: sheaf_core::PageSize {
                    width: 612.0,
                    height: 792.0,
                },
                page_sizes: None,
                bytes: 0,
                password: None,
            },
            &[],
            sheaf_doc::history::default_policy(),
        )
        .snapshot();
        let tracker = sheaf_progress::Tracker::new();
        tracker.cancel();
        let err = run(&snapshot, &job, &tracker).unwrap_err();
        assert!(err.is::<sheaf_progress::Cancelled>());
        assert!(!dir.exists());
    }

    #[test]
    fn jpeg_decodes_to_the_same_size() {
        let bytes = encode(raster(30, 20), Format::Jpeg(80), 150).unwrap();
        assert_eq!(&bytes[..2], &[0xFF, 0xD8]);
        let im = image::load_from_memory(&bytes).unwrap();
        assert_eq!((im.width(), im.height()), (30, 20));
    }
}
