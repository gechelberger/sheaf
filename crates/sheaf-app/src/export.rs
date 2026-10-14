//! File > Export Pages: pages to image files. The pages go through the
//! same file printing makes (`sheaf_doc::save::print_pdf`), so sheaf's own
//! marks come out as they look, then PDFium renders each at the chosen
//! resolution.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use image::{ImageEncoder, RgbaImage};
use sheaf_core::{PageIndex, Rotation};
use sheaf_doc::{PageId, Snapshot};
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

/// Progress as pages are written.
pub enum Progress {
    Page { done: usize, of: usize },
}

/// `<stem>_Page_<n>`, numbers padded so files sort: `_Page_007` of 120.
pub fn file_name(stem: &str, n: usize, of: usize, ext: &str) -> String {
    let width = of.to_string().len();
    format!("{stem}_Page_{n:0width$}.{ext}")
}

/// Run an export on this thread. `progress` is called after each page;
/// returning false stops early. Returns the files written.
pub fn run(
    snapshot: &Snapshot,
    job: &ExportJob,
    mut progress: impl FnMut(Progress) -> bool,
) -> Result<Vec<PathBuf>> {
    if job.pages.is_empty() {
        bail!("no pages to export");
    }
    let engine = flattened(snapshot, &job.pages, job.markup)?;
    let n = engine.page_count() as usize;
    std::fs::create_dir_all(&job.dir).with_context(|| format!("creating {}", job.dir.display()))?;
    if job.format == Format::Text {
        let path = job.dir.join(format!("{}.txt", job.stem));
        write_text(&engine, n, &path, &mut progress)?;
        return Ok(vec![path]);
    }
    if let Format::Tiff(mode) = job.format {
        let path = job.dir.join(format!("{}.tif", job.stem));
        write_tiff(&engine, n, job.dpi, mode, &path, &mut progress)?;
        return Ok(vec![path]);
    }
    let mut written = Vec::with_capacity(n);
    for i in 0..n {
        let raster = render_at(&engine, i as u32, job.dpi)?;
        let bytes = encode(raster, job.format, job.dpi)?;
        let path = job
            .dir
            .join(file_name(&job.stem, i + 1, n, job.format.extension()));
        std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
        written.push(path);
        if !progress(Progress::Page { done: i + 1, of: n }) {
            break;
        }
    }
    Ok(written)
}

/// The pages as they print, opened in PDFium.
pub(crate) fn flattened(
    snapshot: &Snapshot,
    pages: &[PageId],
    markup: bool,
) -> Result<PdfiumEngine> {
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
pub(crate) fn render_at(engine: &PdfiumEngine, i: u32, dpi: u32) -> Result<Raster> {
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
fn write_text(
    engine: &PdfiumEngine,
    n: usize,
    path: &Path,
    progress: &mut impl FnMut(Progress) -> bool,
) -> Result<()> {
    let mut out = String::new();
    for i in 0..n {
        if i > 0 {
            out.push_str("\u{c}\n");
        }
        out.push_str(&page_text(&engine.page_text(PageIndex(i as u32))?));
        if !progress(Progress::Page { done: i + 1, of: n }) {
            break;
        }
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
    progress: &mut impl FnMut(Progress) -> bool,
) -> Result<()> {
    use tiff::encoder::{Compression, Rational, TiffEncoder, colortype};
    use tiff::tags::{ResolutionUnit, Tag};
    // Write beside the target and rename, so a failure leaves no half file.
    let tmp = path.with_extension("tif.part");
    let file =
        std::fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    let mut enc =
        TiffEncoder::new(std::io::BufWriter::new(file))?.with_compression(Compression::Lzw);
    let res = || Rational { n: dpi, d: 1 };
    for i in 0..n {
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
                let data = g4(gray.as_raw(), w, h);
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
        if !progress(Progress::Page { done: i + 1, of: n }) {
            break;
        }
    }
    drop(enc);
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// 8-bit gray rows as CCITT Group 4, thresholded at mid-gray.
fn g4(gray: &[u8], w: u32, h: u32) -> Vec<u8> {
    use fax::{Color, VecWriter, encoder::Encoder};
    let mut enc = Encoder::new(VecWriter::new());
    for row in gray.chunks_exact(w as usize).take(h as usize) {
        let pels = row
            .iter()
            .map(|&v| if v < 128 { Color::Black } else { Color::White });
        // VecWriter can't fail.
        let _ = enc.encode_line(pels, w as u16);
    }
    match enc.finish() {
        Ok(writer) => writer.finish(),
        Err(never) => match never {},
    }
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

impl crate::docview::DocView {
    /// What File > Export Pages needs from the view.
    pub(crate) fn export_context(&self) -> Option<crate::export_dialog::ExportContext> {
        let crate::docview::State::Ready(doc) = &self.state else {
            return None;
        };
        let st = doc.model.state();
        Some(crate::export_dialog::ExportContext {
            page_count: st.pages.len(),
            current: doc.current_page(),
            selected: st
                .pages
                .iter()
                .enumerate()
                .filter(|(_, p)| doc.selection.contains(&p.id))
                .map(|(i, _)| i)
                .collect(),
        })
    }

    /// Ask for a folder, then export there.
    pub(crate) fn export_pages(
        &mut self,
        spec: crate::export_dialog::ExportSpec,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let crate::docview::State::Ready(doc) = &self.state else {
            return;
        };
        let st = doc.model.state();
        let pages: Vec<PageId> = spec
            .pages
            .iter()
            .filter_map(|&i| st.pages.get(i))
            .map(|p| p.id)
            .collect();
        let job = ExportJob {
            pages,
            markup: spec.markup,
            dpi: spec.dpi,
            format: spec.format,
            dir: PathBuf::new(),
            stem: stem_of(&self.path),
        };
        let window_handle = window.window_handle();
        if job.format.single_file() {
            // One file: ask for its name (the picker confirms replacing).
            let dir = self
                .path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_default();
            let suggested = format!("{}.{}", job.stem, job.format.extension());
            let rx = crate::file_dialog::save(cx, &dir, Some(&suggested));
            cx.spawn(async move |this, cx| {
                let Ok(Ok(Some(path))) = rx.await else {
                    return;
                };
                let job = ExportJob {
                    dir: path.parent().map(|p| p.to_path_buf()).unwrap_or_default(),
                    stem: stem_of(&path),
                    ..job
                };
                this.update(cx, |v, cx| v.run_export(job, cx)).ok();
            })
            .detach();
            return;
        }
        let rx = crate::file_dialog::open(
            cx,
            gpui::PathPromptOptions {
                files: false,
                directories: true,
                multiple: false,
                prompt: Some("Export Here".into()),
            },
        );
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(mut dirs))) = rx.await else {
                return;
            };
            let Some(dir) = dirs.pop() else { return };
            let job = ExportJob { dir, ..job };
            let clash = existing(&job);
            if !clash.is_empty() {
                let answer = window_handle
                    .update(cx, |_, window, cx| {
                        sheaf_ui::prompts::ask(
                            window,
                            cx,
                            gpui::PromptLevel::Warning,
                            &format!(
                                "Replace {} existing file{}?",
                                clash.len(),
                                if clash.len() == 1 { "" } else { "s" }
                            ),
                            Some(&format!(
                                "{} already has {}.",
                                job.dir.display(),
                                clash
                                    .iter()
                                    .take(3)
                                    .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                                    + if clash.len() > 3 { ", …" } else { "" }
                            )),
                            &[
                                gpui::PromptButton::ok("Replace"),
                                gpui::PromptButton::cancel("Cancel"),
                            ],
                        )
                    })
                    .ok();
                match answer {
                    Some(rx) => {
                        if rx.await != Ok(0) {
                            return;
                        }
                    }
                    None => return,
                }
            }
            this.update(cx, |v, cx| v.run_export(job, cx)).ok();
        })
        .detach();
    }

    fn run_export(&mut self, job: ExportJob, cx: &mut gpui::Context<Self>) {
        use futures::StreamExt;
        let crate::docview::State::Ready(doc) = &mut self.state else {
            return;
        };
        let snapshot = doc.model.snapshot();
        doc.notice = Some(format!("Exporting to {}…", job.dir.display()));
        cx.notify();
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<Progress>();
        let dir = job.dir.clone();
        let work = cx
            .background_executor()
            .spawn(async move { run(&snapshot, &job, |p| tx.unbounded_send(p).is_ok()) });
        cx.spawn(async move |this, cx| {
            while let Some(Progress::Page { done, of }) = rx.next().await {
                this.update(cx, |v, cx| {
                    if let crate::docview::State::Ready(doc) = &mut v.state {
                        doc.notice = Some(format!("Exporting page {done} of {of}…"));
                    }
                    cx.notify();
                })
                .ok();
            }
            let result = work.await;
            this.update(cx, |v, cx| {
                if let crate::docview::State::Ready(doc) = &mut v.state {
                    doc.notice = Some(match result {
                        Ok(files) => format!(
                            "Exported {} file{} to {}",
                            files.len(),
                            if files.len() == 1 { "" } else { "s" },
                            dir.display()
                        ),
                        Err(e) => format!("Export failed: {e:#}"),
                    });
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
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
    fn g4_round_trips_through_a_decoder() {
        // A 40×10 page: white with a black bar in rows 3–5, columns 8–30.
        let (w, h) = (40u32, 10u32);
        let mut gray = vec![255u8; (w * h) as usize];
        for y in 3..6 {
            for x in 8..31 {
                gray[(y * w + x) as usize] = 0;
            }
        }
        let data = g4(&gray, w, h);
        let mut rows = Vec::new();
        fax::decoder::decode_g4(data.into_iter(), w as u16, Some(h as u16), |t| {
            rows.push(fax::decoder::pels(t, w as u16).collect::<Vec<_>>());
        });
        assert_eq!(rows.len(), h as usize);
        for (y, row) in rows.iter().enumerate() {
            for (x, c) in row.iter().enumerate() {
                let black = (3..6).contains(&y) && (8..31).contains(&x);
                assert_eq!(*c == fax::Color::Black, black, "pel {x},{y}");
            }
        }
    }

    #[test]
    fn jpeg_decodes_to_the_same_size() {
        let bytes = encode(raster(30, 20), Format::Jpeg(80), 150).unwrap();
        assert_eq!(&bytes[..2], &[0xFF, 0xD8]);
        let im = image::load_from_memory(&bytes).unwrap();
        assert_eq!((im.width(), im.height()), (30, 20));
    }
}
