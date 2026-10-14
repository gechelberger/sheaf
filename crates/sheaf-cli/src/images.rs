//! `sheaf images-to-pdf`: image files to a PDF, one page each.

use std::path::PathBuf;

use anyhow::bail;
use clap::{Args, ValueEnum};
use sheaf_ops::images::{ImagesParams, Paper};

#[derive(Args)]
#[command(about = "Make a PDF from images (JPEG, PNG, TIFF, BMP, GIF, WebP), one page each")]
pub struct ImagesArgs {
    /// The images, in page order. A multi-page TIFF gives all its pages.
    #[arg(required = true)]
    inputs: Vec<PathBuf>,
    /// The PDF to write.
    #[arg(short, long)]
    output: PathBuf,
    /// Page size: the image's own size, or the image fitted to a sheet.
    #[arg(long, value_enum, default_value_t = PaperArg::Original)]
    page: PaperArg,
    /// Resolution to use instead of what the images record.
    #[arg(long)]
    dpi: Option<f32>,
    /// White margin around a fitted image, in points.
    #[arg(long, default_value_t = 0.0)]
    margin: f32,
    /// Replace the output if it's already there.
    #[arg(long)]
    force: bool,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum PaperArg {
    Original,
    Letter,
    A4,
}

impl From<PaperArg> for Paper {
    fn from(p: PaperArg) -> Paper {
        match p {
            PaperArg::Original => Paper::Original,
            PaperArg::Letter => Paper::Letter,
            PaperArg::A4 => Paper::A4,
        }
    }
}

pub fn run(args: ImagesArgs, quiet: bool) -> anyhow::Result<()> {
    if args.output.exists() && !args.force {
        bail!(
            "{} already exists (--force replaces)",
            args.output.display()
        );
    }
    let params = ImagesParams {
        page: args.page.into(),
        dpi: args.dpi,
        margin: args.margin,
    };
    crate::progress::run("reading", quiet, |p| {
        sheaf_ops::images::images_to_pdf(&args.inputs, &params, &args.output, p)
    })?;
    println!("{}", args.output.display());
    Ok(())
}
