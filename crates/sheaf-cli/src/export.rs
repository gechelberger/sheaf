//! `sheaf export`: pages to image or text files, as File > Export Pages.

use std::path::PathBuf;

use anyhow::{Context, bail};
use clap::{Args, ValueEnum};
use sheaf_ops::export::{ExportJob, Format, TiffMode, existing, stem_of};

#[derive(Args)]
#[command(about = "Export pages as PNG, JPEG, TIFF or text")]
pub struct ExportArgs {
    /// The PDF to export.
    input: PathBuf,
    /// A folder to write into (default: beside the input). For tiff and
    /// text, which write one file, a file name works too.
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(short, long, value_enum, default_value_t = Kind::Png)]
    format: Kind,
    /// Pages to export, e.g. "1-3, 7, 10-" (default: all).
    #[arg(short, long)]
    pages: Option<String>,
    /// Resolution of image formats.
    #[arg(long, default_value_t = 150, value_parser = clap::value_parser!(u32).range(18..=2400))]
    dpi: u32,
    /// JPEG quality, 1–100.
    #[arg(long, default_value_t = 80, value_parser = clap::value_parser!(u8).range(1..=100))]
    quality: u8,
    /// TIFF color mode.
    #[arg(long, value_enum, default_value_t = Tiff::Bw)]
    tiff: Tiff,
    /// Leave comments and markups out.
    #[arg(long)]
    no_markup: bool,
    /// File name stem (default: the input's): `<name>_Page_<n>.<ext>`.
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    password: Option<String>,
    /// Replace files that are already there.
    #[arg(long)]
    force: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum Kind {
    Png,
    #[value(alias = "jpg")]
    Jpeg,
    #[value(alias = "tif")]
    Tiff,
    #[value(alias = "txt")]
    Text,
}

#[derive(Clone, Copy, ValueEnum)]
enum Tiff {
    Color,
    Gray,
    /// 1-bit, CCITT Group 4.
    Bw,
}

pub fn run(args: ExportArgs, quiet: bool) -> anyhow::Result<()> {
    let format = match args.format {
        Kind::Png => Format::Png,
        Kind::Jpeg => Format::Jpeg(args.quality),
        Kind::Tiff => Format::Tiff(match args.tiff {
            Tiff::Color => TiffMode::Color,
            Tiff::Gray => TiffMode::Gray,
            Tiff::Bw => TiffMode::BlackWhite,
        }),
        Kind::Text => Format::Text,
    };
    let beside = || {
        args.input
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    };
    let (dir, stem) = match &args.output {
        // One output file, named.
        Some(o) if format.single_file() && !o.is_dir() && o.extension().is_some() => (
            o.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from(".")),
            stem_of(o),
        ),
        Some(o) => (
            o.clone(),
            args.name.clone().unwrap_or_else(|| stem_of(&args.input)),
        ),
        None => (
            beside(),
            args.name.clone().unwrap_or_else(|| stem_of(&args.input)),
        ),
    };
    let doc = sheaf_doc::open::file(&args.input, args.password.as_deref())?.doc;
    let snapshot = doc.snapshot();
    let pages = sheaf_ops::pages::ids(&snapshot, args.pages.as_deref())
        .map_err(anyhow::Error::msg)
        .context("--pages")?;
    let job = ExportJob {
        pages,
        markup: !args.no_markup && format != Format::Text,
        dpi: args.dpi,
        format,
        dir,
        stem,
    };
    let clash = existing(&job);
    if !clash.is_empty() && !args.force {
        bail!(
            "{} already exists{} (--force replaces)",
            clash[0].display(),
            match clash.len() {
                1 => String::new(),
                n => format!(", and {} more", n - 1),
            }
        );
    }
    let files = crate::progress::run("exporting", quiet, |p| {
        sheaf_ops::export::run(&snapshot, &job, p)
    })?;
    for f in files {
        println!("{}", f.display());
    }
    Ok(())
}
