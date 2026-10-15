//! `sheaf comments`: a file's comments as a summary PDF, CSV, text, XFDF
//! or FDF, as the Comments panel's Export menu writes them; and comments
//! from a data file or another PDF brought into it, as File > Import
//! Comments does.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use clap::{Args, Subcommand, ValueEnum};
use sheaf_doc::annot::REVIEW_STATES;
use sheaf_ops::comments::{self, Column, CommentFilter, DataFormat, Layout, Sort, Summary};

#[derive(Args)]
#[command(about = "Summarize or list a PDF's comments")]
pub struct CommentsArgs {
    #[command(subcommand)]
    what: What,
}

#[derive(Subcommand)]
enum What {
    /// A summary PDF: the comments alone, or each page beside its list.
    Summary(SummaryArgs),
    /// One row per comment, reply and status, as CSV.
    Csv(CsvArgs),
    /// The list as plain text.
    Text(TextArgs),
    /// The comments as XFDF or FDF, for Acrobat or another tool.
    Export(ExportArgs),
    /// Comments from FDF, XFDF or another PDF into a copy of the file:
    /// those it has already (by name) are updated, the rest added.
    Import(ImportArgs),
}

#[derive(Args)]
struct ExportArgs {
    #[command(flatten)]
    common: Common,
    /// Where to write it (default: standard output). Its extension
    /// (.xfdf, .fdf) picks the format unless --format does.
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(short, long, value_enum)]
    format: Option<FormatArg>,
}

#[derive(Args)]
struct ImportArgs {
    /// The PDF to bring the comments into.
    input: PathBuf,
    /// FDF, XFDF or PDF files with the comments.
    #[arg(required = true)]
    from: Vec<PathBuf>,
    /// Where to write the result (default: beside the input,
    /// "<name> - imported.pdf").
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(long)]
    password: Option<String>,
    /// The password of a PDF the comments come from.
    #[arg(long)]
    from_password: Option<String>,
    /// Replace the output file if it's there.
    #[arg(long)]
    force: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum FormatArg {
    Xfdf,
    Fdf,
}

#[derive(Args)]
struct SummaryArgs {
    #[command(flatten)]
    common: Common,
    /// Where to write it (default: beside the input, "<name> - comments.pdf").
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(short, long, value_enum, default_value_t = LayoutArg::Comments)]
    layout: LayoutArg,
    /// Replace the file if it's there.
    #[arg(long)]
    force: bool,
}

#[derive(Args)]
struct CsvArgs {
    #[command(flatten)]
    common: Common,
    /// Where to write it (default: standard output).
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// The columns, in order, e.g. "number,page,author,text". Any of:
    /// number, page, page-label, type, label, author, date, modified,
    /// status, checked, color, text, reply-to.
    #[arg(short, long, value_delimiter = ',')]
    columns: Vec<String>,
}

#[derive(Args)]
struct TextArgs {
    #[command(flatten)]
    common: Common,
    /// Where to write it (default: standard output).
    #[arg(short, long)]
    output: Option<PathBuf>,
}

/// What every listing takes: the file, and the panel's filter and sort.
#[derive(Args)]
struct Common {
    /// The PDF whose comments to list.
    input: PathBuf,
    #[arg(long)]
    password: Option<String>,
    /// A page or a range of them, e.g. "3-10" (default: all).
    #[arg(short, long)]
    pages: Option<String>,
    #[arg(short, long, value_enum, default_value_t = SortArg::Page)]
    sort: SortArg,
    /// Only comments of this type ("Note", "Highlight", "Tickmark"…);
    /// repeat for more.
    #[arg(long = "type")]
    types: Vec<String>,
    /// Only comments by this author; repeat for more.
    #[arg(long = "author")]
    authors: Vec<String>,
    /// Only comments in this review state (None, Accepted, Rejected,
    /// Cancelled, Completed); repeat for more.
    #[arg(long = "status")]
    statuses: Vec<String>,
    /// Only checked comments.
    #[arg(long, conflicts_with = "unchecked")]
    checked: bool,
    /// Only unchecked comments.
    #[arg(long)]
    unchecked: bool,
    /// PDFlyer's Stamp Summary or Note Summary.
    #[arg(long, value_enum)]
    summary: Option<SummaryArg>,
    /// Only threads that say this, or are by this author.
    #[arg(long)]
    search: Option<String>,
}

#[derive(Clone, Copy, ValueEnum)]
enum LayoutArg {
    /// The comments alone.
    Comments,
    /// Each page that has comments beside a numbered list of them.
    Pages,
}

#[derive(Clone, Copy, ValueEnum)]
enum SortArg {
    Page,
    Author,
    Date,
    Type,
    Color,
}

#[derive(Clone, Copy, ValueEnum)]
enum SummaryArg {
    /// Tickmarks, stamps and connectors.
    Stamps,
    /// Notes and calculator tapes.
    Notes,
}

/// The file's comments, and the filter as asked.
struct Opened {
    doc: sheaf_doc::Document,
    foreign: Vec<(sheaf_doc::SourceId, sheaf_doc::file::ForeignAnnot)>,
    filter: CommentFilter,
    needle: String,
}

impl Common {
    fn open(&self) -> anyhow::Result<Opened> {
        let (doc, foreign) = comments::open(&self.input, self.password.as_deref())?;
        let mut filter = CommentFilter {
            sort: match self.sort {
                SortArg::Page => Sort::Page,
                SortArg::Author => Sort::Author,
                SortArg::Date => Sort::Date,
                SortArg::Type => Sort::Type,
                SortArg::Color => Sort::Color,
            },
            summary: self.summary.map(|s| match s {
                SummaryArg::Stamps => Summary::Stamps,
                SummaryArg::Notes => Summary::Notes,
            }),
            checked: match (self.checked, self.unchecked) {
                (true, _) => Some(true),
                (_, true) => Some(false),
                _ => None,
            },
            ..Default::default()
        };
        if let Some(p) = &self.pages {
            let (a, b) = comments::parse_pages(p)
                .with_context(|| format!("--pages {p:?}: a page or a range, e.g. 3-10"))?;
            if a >= doc.page_count() {
                bail!(
                    "--pages {p:?}: out of range (the file has {} pages)",
                    doc.page_count()
                );
            }
            filter.pages = Some((a, b.min(doc.page_count() - 1)));
        }
        // The filter lists what's hidden; these say what's shown.
        let all = comments::list(doc.state(), &foreign, &CommentFilter::default(), "");
        filter.types = hidden(&all.types, &self.types, "type")?;
        filter.authors = hidden(&all.authors, &self.authors, "author")?;
        let states = REVIEW_STATES.iter().map(|s| s.to_string()).collect();
        filter.statuses = hidden(&states, &self.statuses, "status")?;
        Ok(Opened {
            doc,
            foreign,
            filter,
            needle: self.search.as_deref().unwrap_or("").trim().to_lowercase(),
        })
    }
}

/// Of `all`, the ones not `wanted` (any case); none when nothing is.
fn hidden(
    all: &std::collections::BTreeSet<String>,
    wanted: &[String],
    what: &str,
) -> anyhow::Result<std::collections::BTreeSet<String>> {
    if wanted.is_empty() {
        return Ok(Default::default());
    }
    for w in wanted {
        if !all.iter().any(|a| a.eq_ignore_ascii_case(w)) {
            let known: Vec<&str> = all.iter().map(String::as_str).collect();
            bail!(
                "no comments with {what} {w:?}{}",
                if known.is_empty() {
                    String::new()
                } else {
                    format!(" (there are: {})", known.join(", "))
                }
            );
        }
    }
    Ok(all
        .iter()
        .filter(|a| !wanted.iter().any(|w| a.eq_ignore_ascii_case(w)))
        .cloned()
        .collect())
}

pub fn run(args: CommentsArgs, _quiet: bool) -> anyhow::Result<()> {
    match args.what {
        What::Summary(a) => summary(a),
        What::Csv(a) => {
            let columns = if a.columns.is_empty() {
                Column::DEFAULT.to_vec()
            } else {
                a.columns
                    .iter()
                    .map(|c| {
                        Column::from_name(c).with_context(|| format!("--columns: no column {c:?}"))
                    })
                    .collect::<anyhow::Result<_>>()?
            };
            let o = a.common.open()?;
            let listing = comments::list(o.doc.state(), &o.foreign, &o.filter, &o.needle);
            let labels = o.doc.state().page_labels();
            let out = comments::csv(&listing, labels.as_deref(), &columns);
            write_out(a.output.as_deref(), out.as_bytes())
        }
        What::Export(a) => {
            let format = match (a.format, a.output.as_deref().and_then(DataFormat::of)) {
                (Some(FormatArg::Xfdf), _) => DataFormat::Xfdf,
                (Some(FormatArg::Fdf), _) => DataFormat::Fdf,
                (None, Some(f)) => f,
                (None, None) => DataFormat::Xfdf,
            };
            let o = a.common.open()?;
            let listing = comments::list(o.doc.state(), &o.foreign, &o.filter, &o.needle);
            let out = comments::export_data(
                &o.doc.snapshot(),
                &o.foreign,
                &comments::items(&listing),
                format,
            )?;
            write_out(a.output.as_deref(), &out)
        }
        What::Import(a) => import(a),
        What::Text(a) => {
            let o = a.common.open()?;
            let listing = comments::list(o.doc.state(), &o.foreign, &o.filter, &o.needle);
            let labels = o.doc.state().page_labels();
            let out = comments::text(
                &listing,
                labels.as_deref(),
                &file_name(&a.common.input),
                o.filter.sort,
            );
            write_out(a.output.as_deref(), out.as_bytes())
        }
    }
}

fn import(a: ImportArgs) -> anyhow::Result<()> {
    let output = a.output.clone().unwrap_or_else(|| {
        let stem = sheaf_ops::export::stem_of(&a.input);
        a.input.with_file_name(format!("{stem} - imported.pdf"))
    });
    if output.exists() && !a.force {
        bail!("{} already exists (--force replaces)", output.display());
    }
    let (mut doc, foreign) = comments::open(&a.input, a.password.as_deref())?;
    for from in &a.from {
        let bytes = std::fs::read(from).with_context(|| format!("reading {}", from.display()))?;
        let got = comments::import_data(
            &doc.snapshot(),
            &foreign,
            &bytes,
            a.from_password.as_deref(),
        )
        .with_context(|| from.display().to_string())?;
        eprintln!("{}: {}", from.display(), got.summary());
        if !got.edits.is_empty() {
            doc.apply_batch(got.edits, "Import comments")?;
        }
    }
    sheaf_doc::save::save_to_path(&doc.snapshot(), &output)
        .with_context(|| format!("writing {}", output.display()))?;
    println!("{}", output.display());
    Ok(())
}

fn summary(a: SummaryArgs) -> anyhow::Result<()> {
    let output = a.output.clone().unwrap_or_else(|| {
        let stem = sheaf_ops::export::stem_of(&a.common.input);
        a.common
            .input
            .with_file_name(format!("{stem} - comments.pdf"))
    });
    if output.exists() && !a.force {
        bail!("{} already exists (--force replaces)", output.display());
    }
    let o = a.common.open()?;
    let layout = match a.layout {
        LayoutArg::Comments => Layout::CommentsOnly,
        LayoutArg::Pages => Layout::Pages,
    };
    let bytes = comments::summary_pdf(
        &o.doc.snapshot(),
        &o.foreign,
        &o.filter,
        &o.needle,
        layout,
        &file_name(&a.common.input),
    )?;
    std::fs::write(&output, bytes).with_context(|| format!("writing {}", output.display()))?;
    println!("{}", output.display());
    Ok(())
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `bytes` to the file, else to standard output.
fn write_out(output: Option<&Path>, bytes: &[u8]) -> anyhow::Result<()> {
    match output {
        Some(path) => {
            std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
        }
        None => {
            let mut out = std::io::stdout().lock();
            out.write_all(bytes)?;
            out.flush()?;
            Ok(())
        }
    }
}
