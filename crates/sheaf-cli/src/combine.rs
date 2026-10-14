//! `sheaf combine`: PDFs and images into one PDF.

use std::path::PathBuf;

use anyhow::bail;
use clap::Args;
use sheaf_ops::combine::{CombineItem, CombineParams};

use crate::images::PaperArg;

#[derive(Args)]
#[command(
    about = "Combine PDFs and images into one PDF",
    long_about = "Combine PDFs and images into one PDF, in the order given. Take a subset \
                  of a file's pages with file:range, e.g. a.pdf:1-3,7 or b.pdf:10-. Each \
                  file gets a bookmark holding its own bookmarks."
)]
pub struct CombineArgs {
    /// Files in order, each optionally with `:pages`.
    #[arg(required = true)]
    inputs: Vec<String>,
    /// The PDF to write.
    #[arg(short, long)]
    output: PathBuf,
    /// No bookmark per file; each PDF's own bookmarks go at the top level.
    #[arg(long)]
    no_file_bookmarks: bool,
    /// Page size for images.
    #[arg(long, value_enum, default_value_t = PaperArg::Original)]
    image_page: PaperArg,
    /// Replace the output if it's already there.
    #[arg(long)]
    force: bool,
}

pub fn run(args: CombineArgs, quiet: bool) -> anyhow::Result<()> {
    if args.output.exists() && !args.force {
        bail!(
            "{} already exists (--force replaces)",
            args.output.display()
        );
    }
    let items: Vec<CombineItem> = args
        .inputs
        .iter()
        .map(|a| CombineItem::parse_arg(a))
        .collect();
    for i in &items {
        if !i.path.exists() {
            bail!("{} doesn't exist", i.path.display());
        }
        if args.output.exists() && i.path.canonicalize().ok() == args.output.canonicalize().ok() {
            bail!(
                "{} is an input; write the result elsewhere",
                i.path.display()
            );
        }
    }
    let params = CombineParams {
        bookmark_each_file: !args.no_file_bookmarks,
        image_page: args.image_page.into(),
    };
    let report = crate::progress::run("combining", quiet, |p| {
        sheaf_ops::combine::combine(&items, &params, &args.output, p)
    })?;
    if !quiet {
        eprintln!(
            "{} pages from {} files, {} bookmarks",
            report.pages, report.files, report.bookmarks
        );
    }
    println!("{}", args.output.display());
    Ok(())
}
