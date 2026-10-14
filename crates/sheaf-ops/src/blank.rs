//! Find blank pages (Document > Find Blank Pages…, `sheaf remove-blanks`):
//! duplex backs, scanner fillers, empty separators.
//!
//! Content is checked before pixels, since it's cheap and certain:
//! - a page with sheaf's marks, other annotations, links or form fields
//!   isn't blank;
//! - a page with text isn't either, unless the text says it's
//!   intentionally blank ("This page intentionally left blank", perhaps
//!   with a page number or header).
//!
//! The rest are rendered at about 60 dpi and judged by their pixels with
//! [`sheaf_image::blank`], which the scan pipeline also uses on cleaned
//! photos.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sheaf_core::{PageIndex, Rotation};
use sheaf_doc::{Document, Edit, NewOutlineItem, PageId, Snapshot, SourceId};
use sheaf_image::blank::{Sensitivity, Verdict, is_blank};
use sheaf_progress::{Progress, Unit};
use sheaf_render::pdfium::PdfiumEngine;
use sheaf_render::{Job, PixelFormat, Renderer};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reason {
    Blank,
    /// A little ink in a few places: worth a look, not deleted by default.
    NearlyBlank,
    /// Says it's intentionally left blank.
    IntentionallyBlank,
}

impl Reason {
    pub fn label(self) -> &'static str {
        match self {
            Reason::Blank => "blank",
            Reason::NearlyBlank => "nearly blank",
            Reason::IntentionallyBlank => "intentionally blank",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    /// 0-based position in the document.
    pub index: usize,
    pub id: PageId,
    /// Share of the page that's ink (0 for an intentionally blank one).
    pub ink: f32,
    pub reason: Reason,
}

impl Candidate {
    /// Whether it's deleted unless someone says otherwise.
    pub fn drop_by_default(&self) -> bool {
        self.reason != Reason::NearlyBlank
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct BlankParams {
    pub sensitivity: Sensitivity,
    /// Keep pages that say they're intentionally blank.
    pub keep_intentional: bool,
}

static INTENTIONAL: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)(this\s+page\s+(is\s+|has\s+been\s+|was\s+)?)?(intentionally|deliberately|purposely|purposefully)\s+(left\s+)?blank|(this\s+)?page\s+left\s+blank",
    )
    .expect("valid")
});

/// What a page's text says about it.
#[derive(Debug, PartialEq)]
enum Says {
    /// No real text: the pixels decide.
    Nothing,
    IntentionallyBlank,
    Content,
}

fn text_says(text: &str) -> Says {
    let letters = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).count();
    if letters(text) < 3 {
        return Says::Nothing;
    }
    if INTENTIONAL.is_match(text) {
        // Room for a page number, a header or a footer, not a paragraph.
        let rest = INTENTIONAL.replace_all(text, "");
        if letters(&rest) <= 60 {
            return Says::IntentionallyBlank;
        }
    }
    Says::Content
}

/// The blank pages among `indexes` (0-based; none: every page).
pub fn find(
    snap: &Snapshot,
    indexes: Option<&[usize]>,
    params: BlankParams,
    progress: &dyn Progress,
) -> Result<Vec<Candidate>> {
    let pages = &snap.state.pages;
    let wanted: Vec<usize> = match indexes {
        Some(ix) => ix.iter().copied().filter(|&i| i < pages.len()).collect(),
        None => (0..pages.len()).collect(),
    };
    progress.set_total(Some(wanted.len() as u64), Unit::Pages);
    let mut by_source: BTreeMap<SourceId, Vec<usize>> = BTreeMap::new();
    for &i in &wanted {
        by_source.entry(pages[i].source).or_default().push(i);
    }
    let mut out = Vec::new();
    let mut done = 0;
    for (sid, list) in by_source {
        let info = snap.source(sid).context("unknown source")?;
        let engine = PdfiumEngine::open(&info.path, info.password.as_deref())?;
        let file = sheaf_doc::file::load_pdf(&info.path, info.password.as_deref())?;
        let objs: Vec<_> = file.get_pages().into_values().collect();
        for i in list {
            progress.check()?;
            done += 1;
            progress.set(done);
            let p = pages[i];
            // sheaf's marks, others' annotations, links, fields.
            if snap.state.annots_on(p.id).is_some_and(|a| !a.is_empty()) {
                continue;
            }
            let has_annots = objs
                .get(p.index as usize)
                .and_then(|&o| file.get_dictionary(o).ok())
                .and_then(|d| d.get(b"Annots").ok())
                .and_then(|a| file.dereference(a).ok())
                .and_then(|(_, a)| a.as_array().ok().map(|a| !a.is_empty()))
                .unwrap_or(false);
            if has_annots {
                continue;
            }
            // Text, from the file and any unsaved OCR.
            let mut text = engine.page_text(PageIndex(p.index)).unwrap_or_default();
            if let Some(layer) = snap.state.text_layers.get(&p.id) {
                text.push(' ');
                text.push_str(&layer.text());
            }
            match text_says(&text) {
                Says::IntentionallyBlank => {
                    if !params.keep_intentional {
                        out.push(Candidate {
                            index: i,
                            id: p.id,
                            ink: 0.0,
                            reason: Reason::IntentionallyBlank,
                        });
                    }
                    continue;
                }
                Says::Content => continue,
                Says::Nothing => {}
            }
            let size = info.page_size(p.index);
            let raster = engine.render(
                PageIndex(p.index),
                Job::Page {
                    width: (size.width * 60.0 / 72.0).round().max(16.0) as u32,
                    rotation: Rotation::R0,
                },
                PixelFormat::Rgba,
            )?;
            let gray = image::GrayImage::from_fn(raster.width, raster.height, |x, y| {
                let o = ((y * raster.width + x) * 4) as usize;
                let d = &raster.data[o..o + 3];
                image::Luma([
                    ((77 * d[0] as u32 + 150 * d[1] as u32 + 29 * d[2] as u32) >> 8) as u8,
                ])
            });
            let (reason, ink) = match is_blank(&gray, params.sensitivity) {
                Verdict::Blank { ink } => (Reason::Blank, ink),
                Verdict::NearlyBlank { ink } => (Reason::NearlyBlank, ink),
                Verdict::NotBlank { .. } => continue,
            };
            out.push(Candidate {
                index: i,
                id: p.id,
                ink,
                reason,
            });
        }
    }
    out.sort_by_key(|c| c.index);
    Ok(out)
}

/// Write `doc` without the `drop` pages to `kept`, and, with `rejects`,
/// the dropped pages to a file of their own, each bookmarked with where it
/// was and why ("p. 4 — blank"), so nothing is lost for good. Returns how
/// many pages `kept` has.
pub fn write(
    doc: &mut Document,
    drop: &[Candidate],
    kept: &Path,
    rejects: Option<&Path>,
) -> Result<usize> {
    let gone: HashSet<PageId> = drop.iter().map(|c| c.id).collect();
    let snap = doc.snapshot();
    let keep: Vec<PageId> = snap
        .state
        .pages
        .iter()
        .map(|p| p.id)
        .filter(|id| !gone.contains(id))
        .collect();
    if keep.is_empty() {
        anyhow::bail!("every page is blank; nothing to keep");
    }
    sheaf_doc::save::extract_pages(&snap, &keep, kept)?;
    let Some(rejects) = rejects.filter(|_| !drop.is_empty()) else {
        return Ok(keep.len());
    };
    // The rejects' own bookmarks replace the document's.
    let mut edits: Vec<Edit> = snap
        .state
        .outline
        .iter()
        .map(|n| Edit::OutlineDelete { id: n.id })
        .collect();
    edits.push(Edit::DeletePages { ids: keep.clone() });
    edits.push(Edit::OutlineInsertTree {
        parent: None,
        after: None,
        items: drop
            .iter()
            .map(|c| NewOutlineItem {
                title: format!("p. {} — {}", c.index + 1, c.reason.label()),
                target: Some(c.id),
                view: None,
                depth: 0,
            })
            .collect(),
    });
    doc.apply_batch(edits, "Rejected pages")?;
    sheaf_doc::save::rewrite_to_path(&doc.snapshot(), rejects)?;
    Ok(keep.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intentionally_blank_phrases() {
        let says = |t: &str| text_says(t);
        assert_eq!(
            says("This page intentionally left blank"),
            Says::IntentionallyBlank
        );
        assert_eq!(
            says("Form 1040 (2025)   Page 4\nTHIS PAGE HAS BEEN INTENTIONALLY LEFT BLANK"),
            Says::IntentionallyBlank
        );
        assert_eq!(says("Intentionally blank"), Says::IntentionallyBlank);
        assert_eq!(says("  . , - "), Says::Nothing);
        // A paragraph that mentions it is content.
        let long = "Schedule B instructions: if a line does not apply, leave it blank. \
                    Lines intentionally left blank are not errors and need no explanation \
                    from the preparer or the taxpayer when filing.";
        assert_eq!(says(long), Says::Content);
        assert_eq!(says("Invoice 1234 total 56.00"), Says::Content);
    }
}
