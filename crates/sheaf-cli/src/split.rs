//! `sheaf split`: one PDF into several.

use std::path::PathBuf;

use anyhow::Context;
use clap::{ArgGroup, Args};
use sheaf_ops::export::stem_of;
use sheaf_ops::split::SplitBy;

#[derive(Args)]
#[command(
    about = "Split a PDF into several files",
    group(ArgGroup::new("how").required(true).args(["every", "ranges", "bookmarks", "max_size"]))
)]
pub struct SplitArgs {
    /// The PDF to split.
    input: PathBuf,
    /// Parts of this many pages.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    every: Option<u32>,
    /// A part per range, separated by semicolons: "1-3; 4-10; 11-".
    #[arg(long)]
    ranges: Option<String>,
    /// A part per top-level bookmark, named after it.
    #[arg(long)]
    bookmarks: bool,
    /// Parts no bigger than this: "10MB", "500KB".
    #[arg(long)]
    max_size: Option<String>,
    /// The folder to write into (default: beside the input).
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// File name stem (default: the input's).
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    password: Option<String>,
    /// Replace files that are already there.
    #[arg(long)]
    force: bool,
}

pub fn run(args: SplitArgs, quiet: bool) -> anyhow::Result<()> {
    let by = if let Some(n) = args.every {
        SplitBy::EveryN(n as usize)
    } else if let Some(r) = &args.ranges {
        SplitBy::Ranges(r.split(';').map(|s| s.trim().to_string()).collect())
    } else if args.bookmarks {
        SplitBy::TopBookmarks
    } else {
        let s = args.max_size.as_deref().unwrap_or_default();
        SplitBy::MaxBytes(
            sheaf_ops::split::parse_size(s)
                .map_err(anyhow::Error::msg)
                .context("--max-size")?,
        )
    };
    let dir = args.output.clone().unwrap_or_else(|| {
        args.input
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    });
    let stem = args.name.clone().unwrap_or_else(|| stem_of(&args.input));
    let doc = sheaf_doc::open::file(&args.input, args.password.as_deref())?.doc;
    let snap = doc.snapshot();
    let files = crate::progress::run("splitting", quiet, |p| {
        sheaf_ops::split::split(&snap, &by, &dir, &stem, args.force, p)
    })?;
    for f in files {
        println!("{}", f.display());
    }
    Ok(())
}
