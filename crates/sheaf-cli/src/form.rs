//! `sheaf form`: a PDF's form. So far, `check`: what in the form other
//! viewers would trip on, or a save would carry where it shouldn't.

use std::path::PathBuf;

use clap::{Args, Subcommand};

#[derive(Args)]
#[command(about = "Check a PDF's form")]
pub struct FormArgs {
    #[command(subcommand)]
    what: What,
}

#[derive(Subcommand)]
enum What {
    /// The form's fields and widgets, its XFA copy, usage rights and
    /// signatures, and the problems found in them.
    Check(CheckArgs),
}

#[derive(Args)]
struct CheckArgs {
    /// The PDF to check.
    input: PathBuf,
    #[arg(long)]
    password: Option<String>,
}

pub fn run(args: FormArgs, _quiet: bool) -> anyhow::Result<()> {
    match args.what {
        What::Check(a) => {
            let report = sheaf_doc::forms::check_file(&a.input, a.password.as_deref())?;
            print!("{report}");
            Ok(())
        }
    }
}
