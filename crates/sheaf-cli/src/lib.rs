//! sheaf's command line. Each subcommand parses its flags into an op's
//! parameters (`sheaf_ops`) and runs it with a progress bar on stderr;
//! Ctrl-C cancels the op cleanly.
//!
//! Two binaries run it: `sheaf-cli`, which has no GUI at all, and the app
//! itself, which hands off here when its first argument is a subcommand
//! (`sheaf export …`).

use clap::{Parser, Subcommand};
use std::ffi::{OsStr, OsString};

mod blanks;
mod combine;
mod comments;
mod export;
mod form;
mod images;
mod progress;
mod scan;
mod split;

/// The subcommands, for the app to recognize its hand-off.
pub const SUBCOMMANDS: &[&str] = &[
    "combine",
    "comments",
    "export",
    "form",
    "images-to-pdf",
    "remove-blanks",
    "scan",
    "split",
];

/// Whether `arg` (the first argument after the program name) names a
/// subcommand. A file of that name in the current directory still opens
/// in the app as `./export`.
pub fn is_subcommand(arg: &OsStr) -> bool {
    arg.to_str().is_some_and(|a| SUBCOMMANDS.contains(&a))
}

#[derive(Parser)]
#[command(version, about = "sheaf: PDF tools", propagate_version = true)]
struct Cli {
    /// No progress bar.
    #[arg(short, long, global = true)]
    quiet: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Combine(combine::CombineArgs),
    Comments(comments::CommentsArgs),
    Export(export::ExportArgs),
    Form(form::FormArgs),
    ImagesToPdf(images::ImagesArgs),
    RemoveBlanks(blanks::BlanksArgs),
    Scan(scan::ScanArgs),
    Split(split::SplitArgs),
}

/// Exit status when an op was cancelled (as for SIGINT).
const CANCELLED: u8 = 130;

/// Parse `args` (program name first) and run the subcommand. Returns the
/// exit status: 0, 1 on failure, 2 for bad arguments, 130 if cancelled.
pub fn run<I, T>(args: I) -> u8
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(e) => {
            let _ = e.print();
            return if e.use_stderr() { 2 } else { 0 };
        }
    };
    let result = match cli.command {
        Command::Combine(args) => combine::run(args, cli.quiet),
        Command::Comments(args) => comments::run(args, cli.quiet),
        Command::Export(args) => export::run(args, cli.quiet),
        Command::Form(args) => form::run(args, cli.quiet),
        Command::ImagesToPdf(args) => images::run(args, cli.quiet),
        Command::RemoveBlanks(args) => blanks::run(args, cli.quiet),
        Command::Scan(args) => scan::run(args, cli.quiet),
        Command::Split(args) => split::run(args, cli.quiet),
    };
    match result {
        Ok(()) => 0,
        Err(e) if sheaf_ops::was_cancelled(&e) => {
            eprintln!("cancelled");
            CANCELLED
        }
        Err(e) => {
            eprintln!("error: {e:#}");
            1
        }
    }
}
