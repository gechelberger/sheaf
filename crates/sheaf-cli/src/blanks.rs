//! `sheaf remove-blanks`: a PDF without its blank pages, as Document >
//! Find Blank Pages… does after review.

use std::path::PathBuf;

use anyhow::bail;
use clap::{Args, ValueEnum};
use sheaf_image::blank::Sensitivity;
use sheaf_ops::blank::BlankParams;
use sheaf_ops::export::stem_of;

#[derive(Args)]
#[command(about = "Remove blank pages from a PDF")]
pub struct BlanksArgs {
    /// The PDF to clean up.
    input: PathBuf,
    /// Where the kept pages go (default: `<input>_no_blanks.pdf` beside it).
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Also write the removed pages here, bookmarked "p. 4 — blank".
    #[arg(long)]
    rejects: Option<PathBuf>,
    /// Pages to look at, e.g. "1-3, 7, 10-" (default: all).
    #[arg(short, long)]
    pages: Option<String>,
    #[arg(long, value_enum, default_value_t = Level::Normal)]
    sensitivity: Level,
    /// Keep pages that say "this page intentionally left blank".
    #[arg(long)]
    keep_intentional: bool,
    /// Also remove nearly blank pages (a signature, a stray mark).
    #[arg(long)]
    nearly_blank: bool,
    /// List what would go, and write nothing.
    #[arg(short = 'n', long)]
    dry_run: bool,
    #[arg(long)]
    password: Option<String>,
    /// Replace files that are already there.
    #[arg(long)]
    force: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum Level {
    /// Only pages with next to nothing on them.
    Strict,
    Normal,
    /// Also pages with heavier scanner noise.
    Lenient,
}

pub fn run(args: BlanksArgs, quiet: bool) -> anyhow::Result<()> {
    let output = args.output.clone().unwrap_or_else(|| {
        args.input
            .with_file_name(format!("{}_no_blanks.pdf", stem_of(&args.input)))
    });
    if !args.dry_run && !args.force {
        for p in std::iter::once(&output).chain(&args.rejects) {
            if p.exists() {
                bail!("{} already exists (--force replaces)", p.display());
            }
        }
    }
    let mut doc = sheaf_doc::open::file(&args.input, args.password.as_deref())?.doc;
    let snap = doc.snapshot();
    let pages = match &args.pages {
        Some(r) => Some(
            sheaf_ops::pages::parse(r, snap.state.pages.len())
                .map_err(|e| anyhow::anyhow!("--pages: {e}"))?,
        ),
        None => None,
    };
    let params = BlankParams {
        sensitivity: match args.sensitivity {
            Level::Strict => Sensitivity::Strict,
            Level::Normal => Sensitivity::Normal,
            Level::Lenient => Sensitivity::Lenient,
        },
        keep_intentional: args.keep_intentional,
    };
    let found = crate::progress::run("looking", quiet, |p| {
        sheaf_ops::blank::find(&snap, pages.as_deref(), params, p)
    })?;
    let (drop, keep): (Vec<_>, Vec<_>) = found
        .into_iter()
        .partition(|c| args.nearly_blank || c.drop_by_default());
    // One line per page found: number, why, and what happens to it.
    for (c, gone) in drop
        .iter()
        .map(|c| (c, true))
        .chain(keep.iter().map(|c| (c, false)))
    {
        let why = match c.reason {
            sheaf_ops::blank::Reason::NearlyBlank => {
                format!("nearly blank ({:.2}% ink)", c.ink * 100.0)
            }
            r => r.label().to_string(),
        };
        println!(
            "{}\t{why}\t{}",
            c.index + 1,
            if gone { "removed" } else { "kept" }
        );
    }
    if args.dry_run {
        return Ok(());
    }
    if drop.is_empty() {
        eprintln!("no blank pages; nothing written");
        return Ok(());
    }
    let n = sheaf_ops::blank::write(&mut doc, &drop, &output, args.rejects.as_deref())?;
    if !quiet {
        eprintln!("{}: {n} pages, {} removed", output.display(), drop.len());
    }
    Ok(())
}
