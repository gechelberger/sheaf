//! Combine Files (File > Create > Combine Files…, `sheaf combine`): one
//! new PDF from PDFs and images, each with an optional page subset, with a
//! bookmark per file and each PDF's own bookmarks nested under it.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use in_progress::{NoProgress, Progress, Unit};
use serde::{Deserialize, Serialize};
use sheaf_doc::open::Facts;
use sheaf_doc::{
    Document, Edit, FlatOutlineItem, NewOutlineItem, PageId, SourceId, SourceInfo, Store,
};

use crate::images::Paper;

/// One input: a file and, optionally, which of its pages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CombineItem {
    pub path: PathBuf,
    /// A page range like "1-3, 7, 10-"; none for every page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pages: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

impl CombineItem {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            pages: None,
            password: None,
        }
    }

    /// A command-line argument: `file` or `file:range`. A path that exists
    /// as given wins, so `C:\scans\a.pdf` and odd file names still work.
    pub fn parse_arg(arg: &str) -> Self {
        if !Path::new(arg).exists()
            && let Some((path, range)) = arg.rsplit_once(':')
            && !path.is_empty()
            && !range.contains(['/', '\\'])
        {
            return Self {
                path: path.into(),
                pages: Some(range.to_string()),
                password: None,
            };
        }
        Self::new(arg)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct CombineParams {
    /// A bookmark per file, titled with its name, holding its own bookmarks.
    pub bookmark_each_file: bool,
    /// Page size for images.
    pub image_page: Paper,
}

impl Default for CombineParams {
    fn default() -> Self {
        Self {
            bookmark_each_file: true,
            image_page: Paper::Original,
        }
    }
}

#[derive(Debug, Default)]
pub struct CombineReport {
    pub pages: usize,
    pub files: usize,
    pub bookmarks: usize,
    /// About the forms combined: fields put under a new parent since their
    /// names were taken, and scripts turned off (M10 proposal 5).
    pub notice: Option<String>,
}

/// Whether `path` is a PDF (by its first bytes, else its extension).
pub fn is_pdf(path: &Path) -> bool {
    let mut head = [0u8; 5];
    match std::fs::File::open(path).and_then(|mut f| std::io::Read::read_exact(&mut f, &mut head)) {
        Ok(()) => &head == b"%PDF-",
        Err(_) => path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pdf")),
    }
}

/// The file sheaf can take its pages from: the PDF itself, or a PDF made
/// from the image (kept in the generated-file cache).
pub fn page_source(path: &Path, image_page: Paper) -> Result<PathBuf> {
    if is_pdf(path) {
        return Ok(path.to_path_buf());
    }
    let pages = sheaf_image::io::read(path)?;
    let pdf = sheaf_doc::images::to_pdf(&pages, image_page.fit(0.0))?;
    sheaf_doc::generated::store(&pdf)
}

/// Build the combined document (not yet written).
pub fn build(
    items: &[CombineItem],
    params: &CombineParams,
    progress: &dyn Progress,
) -> Result<(Document, CombineReport)> {
    if items.is_empty() {
        bail!("nothing to combine");
    }
    progress.set_total(Some(items.len() as u64), Unit::ITEMS);
    let store = Store::new();
    let mut doc: Option<Document> = None;
    let mut report = CombineReport::default();
    let (mut renamed, mut signed) = (Vec::new(), Vec::new());
    for (n, item) in items.iter().enumerate() {
        progress.check()?;
        let name = item
            .path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let file = page_source(&item.path, params.image_page)
            .with_context(|| format!("reading {}", item.path.display()))?;
        let info = SourceInfo::read(&store, &file, item.password.as_deref())
            .with_context(|| format!("opening {name}"))?;
        let count = info.src.page_count() as usize;
        let wanted: Vec<u32> = match &item.pages {
            Some(r) if !r.trim().is_empty() => crate::pages::parse(r, count)
                .map_err(anyhow::Error::msg)
                .with_context(|| format!("{name}: pages"))?
                .into_iter()
                .map(|i| i as u32)
                .collect(),
            _ => (0..info.src.page_count()).collect(),
        };
        // Its marks and what they hold, its form (M10 slice 4) and its
        // bookmarks; an image's page has none.
        let Facts {
            marks,
            assets,
            form,
            outline,
            ..
        } = if is_pdf(&item.path) {
            sheaf_doc::open::finish(&info.src, &NoProgress)
        } else {
            Facts::default()
        };
        // Page ids of this file's pages, by index in the file.
        let ids: Vec<(u32, PageId)> = match doc.as_mut() {
            None => {
                // The first file opens as the document, with its marks and
                // form. Its bookmarks go under its own below, like every
                // file's, and its labels and header/footer aren't taken.
                let mut d = sheaf_doc::open::start(
                    store.clone(),
                    info,
                    sheaf_doc::history::default_policy(),
                );
                d.ready(Facts {
                    marks,
                    assets,
                    form,
                    ..Default::default()
                })?;
                let all: Vec<PageId> = d.state().pages.iter().map(|p| p.id).collect();
                let drop: Vec<PageId> = all
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| !wanted.contains(&(*i as u32)))
                    .map(|(_, id)| *id)
                    .collect();
                if !drop.is_empty() {
                    d.apply(Edit::DeletePages { ids: drop }, "Combine")?;
                }
                let ids = wanted.iter().map(|&i| (i, all[i as usize])).collect();
                doc = Some(d);
                ids
            }
            Some(d) => {
                let sid = d.add_source(info);
                d.set_source_form(sid, form);
                d.set_source_assets(sid, &assets);
                let ids = insert_runs(d, sid, &wanted, &mut renamed, &mut signed)?;
                for &(index, id) in &ids {
                    for annot in marks.get(&index).into_iter().flatten() {
                        d.apply(
                            Edit::AnnotAdd {
                                page: id,
                                annot: annot.clone(),
                            },
                            "Combine",
                        )?;
                    }
                }
                ids
            }
        };
        let d = doc.as_mut().expect("made above");
        let title = item
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or(name);
        report.bookmarks += add_bookmarks(d, &title, &ids, &outline, params.bookmark_each_file)?;
        report.pages += ids.len();
        report.files += 1;
        progress.set(n as u64 + 1);
    }
    let doc = doc.expect("at least one item");
    signed.dedup();
    let forms = sheaf_doc::EditOutcome {
        forms_renamed: renamed,
        signatures_in: signed,
        ..Default::default()
    };
    report.notice = doc.form_notice(&forms, false, false);
    Ok((doc, report))
}

/// Insert `wanted` pages of `sid` at the end, a contiguous run at a time,
/// and return each one's id by its index in the file. The forms renamed
/// go in `renamed`, and files whose signed fields came in in `signed`.
fn insert_runs(
    d: &mut Document,
    sid: SourceId,
    wanted: &[u32],
    renamed: &mut Vec<(String, SourceId)>,
    signed: &mut Vec<SourceId>,
) -> Result<Vec<(u32, PageId)>> {
    let mut out = Vec::with_capacity(wanted.len());
    let mut i = 0;
    while i < wanted.len() {
        let mut j = i + 1;
        while j < wanted.len() && wanted[j] == wanted[j - 1] + 1 {
            j += 1;
        }
        let before = d.state().pages.len();
        let done = d.apply(
            Edit::InsertPages {
                source: sid,
                range: wanted[i]..wanted[j - 1] + 1,
                before: None,
                outline: None,
                // `combine` adds each file's marks itself, afterwards.
                annots: Vec::new(),
            },
            "Combine",
        )?;
        renamed.extend(done.forms_renamed);
        signed.extend(done.signatures_in);
        let pages = &d.state().pages;
        out.extend(
            (before..pages.len())
                .zip(wanted[i]..)
                .map(|(at, index)| (index, pages[at].id)),
        );
        i = j;
    }
    Ok(out)
}

/// This file's bookmarks at the end of the outline: under one titled with
/// the file's name (when `per_file`), else at the top level. Bookmarks to
/// pages left out are dropped unless something under them stays, which
/// makes them headings. Returns how many were added.
fn add_bookmarks(
    d: &mut Document,
    title: &str,
    ids: &[(u32, PageId)],
    outline: &[FlatOutlineItem],
    per_file: bool,
) -> Result<usize> {
    let target =
        |page: Option<u32>| page.and_then(|p| ids.iter().find(|(i, _)| *i == p).map(|(_, id)| *id));
    // Keep an item if it points at a kept page or a descendant does.
    let mut keep = vec![false; outline.len()];
    for i in (0..outline.len()).rev() {
        let child_kept = (i + 1..outline.len())
            .take_while(|&j| outline[j].depth > outline[i].depth)
            .any(|j| keep[j]);
        keep[i] = child_kept || target(outline[i].page).is_some();
    }
    let base = u16::from(per_file);
    let mut items: Vec<NewOutlineItem> = Vec::new();
    if per_file {
        items.push(NewOutlineItem {
            title: title.to_string(),
            target: ids.first().map(|(_, id)| *id),
            view: None,
            depth: 0,
        });
    }
    // Depths renumbered so a dropped parent's children move up a level.
    let mut stack: Vec<u16> = Vec::new();
    for (i, o) in outline.iter().enumerate() {
        while stack.last().is_some_and(|&d| d >= o.depth) {
            stack.pop();
        }
        if !keep[i] {
            continue;
        }
        let t = target(o.page);
        items.push(NewOutlineItem {
            title: o.title.clone(),
            target: t,
            view: t.and(o.view.clone()),
            depth: base + stack.len() as u16,
        });
        stack.push(o.depth);
    }
    if items.is_empty() {
        return Ok(0);
    }
    let n = items.len();
    let after = d.state().outline.last().map(|o| o.id);
    d.apply(
        Edit::OutlineInsertTree {
            parent: None,
            after,
            items,
        },
        "Combine",
    )?;
    Ok(n)
}

/// Combine `items` into a new PDF at `out`.
pub fn combine(
    items: &[CombineItem],
    params: &CombineParams,
    out: &Path,
    progress: &dyn Progress,
) -> Result<CombineReport> {
    let (doc, report) = build(items, params, &*progress.child("reading", 0.7))?;
    progress.check()?;
    sheaf_doc::save::rewrite_to_path(&doc.snapshot(), out)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each file's page stamps and attachments come with its marks: the
    /// combined document finds them in the files, whose stems no cache
    /// has, and writes them, and every mark reopens as sheaf's.
    #[test]
    fn marks_bring_what_they_hold() {
        use sheaf_doc::AnnotKind;
        use sheaf_doc::annot::{WriteCx, to_pdf_in};
        use sheaf_doc::assets::Stamp;
        use sheaf_doc::forms::fixture::{self, Form};
        let dir = std::env::temp_dir().join(format!(
            "sheaf-combine-{}-{:x}",
            std::process::id(),
            fastrand::u64(..)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let every = fixture::every_mark_kind();
        let held = fixture::mark_assets();
        let pdf = held.stamp_pdf(fixture::MARK_STAMP).unwrap();
        let file = held.file_bytes(fixture::MARK_FILE).unwrap();
        // The page stamp and the attachment, each file's named its own.
        let items = ["a", "b"].map(|name| {
            let (stamp, attached) = (format!("{name}-stamp"), format!("{name}-file"));
            held.intern_stamp(&stamp, pdf.clone());
            held.intern_file(&attached, file.clone());
            let mut marks = [every[17].clone(), every[23].clone()];
            if let AnnotKind::PageStamp { stamp: s, .. } = &mut marks[0].kind {
                *s = stamp;
            }
            if let AnnotKind::FileAttachment { file: f, .. } = &mut marks[1].kind {
                *f = attached;
            }
            let mut f = Form::new(1);
            let mut cx = WriteCx::new(held.clone());
            let mut annots = Vec::new();
            for a in &marks {
                let main = to_pdf_in(a, &mut f.pdf, &mut cx).main;
                annots.push(lopdf::Object::Reference(f.pdf.add_object(main)));
            }
            let page = f.pages[0];
            f.dict(page).set("Annots", annots);
            let path = dir.join(format!("{name}.pdf"));
            std::fs::write(&path, f.bytes()).unwrap();
            CombineItem {
                path,
                pages: None,
                password: None,
            }
        });
        let (doc, _) = build(&items, &CombineParams::default(), &NoProgress).unwrap();
        let assets = doc.assets();
        for name in ["a", "b"] {
            let stamp = assets.stamp(&format!("{name}-stamp"));
            assert!(matches!(stamp, Some(Stamp::Form { .. })), "{name}");
            let file = assets.file_bytes(&format!("{name}-file")).unwrap();
            assert_eq!(&*file, fixture::MARK_FILE_BYTES, "{name}");
        }
        let out = dir.join("combined.pdf");
        sheaf_doc::save::rewrite_to_path(&doc.snapshot(), &out).unwrap();
        let back = sheaf_doc::open::file(&out, None).unwrap();
        assert!(back.view.foreign.is_empty(), "{:?}", back.view.foreign);
        let st = back.doc.state();
        for p in st.pages.iter() {
            assert_eq!(st.annots_on(p.id).map(|l| l.len()), Some(2));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn args_split_off_a_range() {
        let a = CombineItem::parse_arg("a.pdf:1-3");
        assert_eq!(a.path, PathBuf::from("a.pdf"));
        assert_eq!(a.pages.as_deref(), Some("1-3"));
        assert_eq!(CombineItem::parse_arg("scan.tif").pages, None);
        // A path with a colon in a directory stays whole.
        assert_eq!(
            CombineItem::parse_arg("C:\\scans\\b.png").path,
            PathBuf::from("C:\\scans\\b.png")
        );
    }
}
