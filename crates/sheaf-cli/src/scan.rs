//! `sheaf scan`: photos or scans of paper to a clean PDF.

use std::path::PathBuf;

use anyhow::bail;
use clap::{Args, ValueEnum};
use sheaf_ocr::{EngineKind, OcrConfig};
use sheaf_ops::scan::ScanOptions;
use sheaf_scan::Mode;

use crate::images::PaperArg;

#[derive(Args)]
#[command(
    about = "Clean up photos or scans of paper into a PDF",
    long_about = "Clean up photos or scans of paper into a PDF, a page each: find the \
                  page in the photo and straighten it, turn it upright, level the text, \
                  even out shadows and make the paper white."
)]
pub struct ScanArgs {
    /// The images, in page order.
    #[arg(required = true)]
    inputs: Vec<PathBuf>,
    /// The PDF to write.
    #[arg(short, long)]
    output: PathBuf,
    #[arg(long, value_enum, default_value_t = ModeArg::Color)]
    mode: ModeArg,
    /// Page size. `original` keeps the cleaned image's size at --dpi.
    #[arg(long, value_enum, default_value_t = PaperArg::Letter)]
    page: PaperArg,
    #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u32).range(72..=1200))]
    dpi: u32,
    /// Recognize the text, so the PDF can be searched and copied from.
    #[arg(long)]
    ocr: bool,
    /// OCR engine (default: tesseract).
    #[arg(long, value_enum)]
    engine: Option<EngineArg>,
    /// OCR language, e.g. "eng" or "eng+deu" (tesseract) or a PaddleOCR set.
    #[arg(long)]
    lang: Option<String>,
    /// Flatten curled or bent pages (book pages, folded letters) with the
    /// UVDoc model. Slower; can make flat pages wobble slightly. Needs
    /// scripts/fetch-ocr-data.sh --uvdoc.
    #[arg(long)]
    flatten_curl: bool,
    /// Leave out pages that come out blank (duplex backs, separators).
    #[arg(long)]
    drop_blanks: bool,
    /// Keep the whole photo: don't look for the page's edges.
    #[arg(long)]
    no_crop: bool,
    /// Don't turn pages upright.
    #[arg(long)]
    no_orient: bool,
    #[arg(long)]
    no_deskew: bool,
    /// Leave lighting and colors as they are.
    #[arg(long)]
    no_flatten: bool,
    /// Black-and-white threshold strength (Sauvola k, 0.05–0.6): higher
    /// drops more faint marks.
    #[arg(long, default_value_t = 0.25)]
    threshold: f32,
    /// Replace the output if it's already there.
    #[arg(long)]
    force: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum ModeArg {
    Color,
    Gray,
    /// Black and white.
    Bw,
}

#[derive(Clone, Copy, ValueEnum)]
enum EngineArg {
    Tesseract,
    #[value(alias = "paddle")]
    Oar,
}

pub fn run(args: ScanArgs, quiet: bool) -> anyhow::Result<()> {
    if args.output.exists() && !args.force {
        bail!(
            "{} already exists (--force replaces)",
            args.output.display()
        );
    }
    let mut opts = ScanOptions {
        page: args.page.into(),
        dpi: args.dpi,
        ocr: args.ocr,
        drop_blanks: args.drop_blanks,
        ..Default::default()
    };
    opts.clean.mode = match args.mode {
        ModeArg::Color => Mode::Color,
        ModeArg::Gray => Mode::Gray,
        ModeArg::Bw => Mode::Bw,
    };
    opts.clean.crop = !args.no_crop;
    opts.clean.flatten_curl = args.flatten_curl;
    opts.clean.orient = !args.no_orient;
    opts.clean.deskew = !args.no_deskew;
    opts.clean.flatten = !args.no_flatten;
    opts.clean.threshold_k = args.threshold.clamp(0.05, 0.6);
    let mut ocr = OcrConfig::default();
    if let Some(e) = args.engine {
        ocr.engine = match e {
            EngineArg::Tesseract => EngineKind::Tesseract,
            EngineArg::Oar => EngineKind::Oar,
        };
    }
    if let Some(lang) = &args.lang {
        sheaf_ocr::set_language(ocr.engine, &mut ocr, lang);
    }
    let report = crate::progress::run("scanning", quiet, |p| {
        sheaf_ops::scan::scan(&args.inputs, &opts, &ocr, &args.output, p)
    })?;
    if !quiet {
        eprintln!(
            "{} page{}, {} cropped from the photo{}{}",
            report.pages,
            if report.pages == 1 { "" } else { "s" },
            report.cropped,
            if report.dropped > 0 {
                format!(", {} blank left out", report.dropped)
            } else {
                String::new()
            },
            if args.ocr {
                format!(", {} words recognized", report.words)
            } else {
                String::new()
            }
        );
    }
    println!("{}", args.output.display());
    Ok(())
}
