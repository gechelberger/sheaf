//! Comments in and out of a document as data: FDF, XFDF (`fdf`, `xfdf`),
//! or another PDF's (Acrobat's Import/Export Data File, M9 slice 18).
//!
//! Whatever the format, comments travel as PDF annotation dictionaries
//! ([`Comments`]): XFDF's elements are read into the same keys. Coming in,
//! each becomes a mark through [`crate::import`], the converter that also
//! makes another tool's comment editable, and is matched to what's there
//! by `/NM` ([`import`]).

pub mod fdf;
pub mod xfdf;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use anyhow::{Context, Result};
use lopdf::{Dictionary, Document as Pdf, Object, ObjectId};

use crate::annot::{AnnotId, PageGeom};
use crate::file::{ForeignAnnot, thread_name};
use crate::import::{NotAMark, Options};
use crate::model::{DocState, Edit, PageId, PageRef, SourceId, SourceInfo};

/// Comments as dictionaries, each with its page (from 0). `pdf` holds
/// what they refer to: appearances, files, the comments their `/IRT`
/// names.
pub struct Comments {
    pub pdf: Pdf,
    pub annots: Vec<(u32, ObjectId)>,
    /// The PDF they're comments on, as the data file names it.
    pub file: Option<String>,
}

impl Comments {
    pub fn new() -> Comments {
        Comments {
            pdf: Pdf::with_version("1.2"),
            annots: Vec::new(),
            file: None,
        }
    }

    /// The `i`th comment's dictionary.
    pub fn dict(&self, i: usize) -> Option<&Dictionary> {
        self.pdf.get_dictionary(self.annots.get(i)?.1).ok()
    }

    pub fn len(&self) -> usize {
        self.annots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.annots.is_empty()
    }

    /// The name of what `dict` replies to, or is part of.
    pub fn parent_of(&self, dict: &Dictionary) -> Option<String> {
        crate::import::irt_name(&self.pdf, dict)
    }
}

impl Default for Comments {
    fn default() -> Self {
        Comments::new()
    }
}

/// A data file or PDF, by what it starts with.
pub fn read(bytes: &[u8], password: Option<&str>) -> Result<Comments> {
    let head: Vec<u8> = bytes
        .iter()
        .copied()
        .skip_while(|b| b.is_ascii_whitespace() || *b == 0xEF || *b == 0xBB || *b == 0xBF)
        .take(5)
        .collect();
    match head.as_slice() {
        b"%FDF-" => fdf::read(bytes),
        b"%PDF-" => from_pdf(bytes, password),
        [b'<', ..] => xfdf::read(bytes),
        _ => anyhow::bail!("not an FDF, XFDF or PDF file"),
    }
}

/// Another PDF's comments: every annotation but links, form fields and
/// popups, sheaf's own included (they come back exactly, from
/// `/SheafAnnot`).
pub fn from_pdf(bytes: &[u8], password: Option<&str>) -> Result<Comments> {
    let mut pdf = Pdf::load_mem(bytes).context("reading the PDF")?;
    if pdf.is_encrypted() {
        let options = lopdf::LoadOptions::with_password(password.unwrap_or(""));
        pdf = Pdf::load_mem_with_options(bytes, options)
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("decrypting the PDF")?;
    }
    let mut annots = Vec::new();
    for (i, page) in pdf.get_pages().into_values().enumerate() {
        let list = pdf
            .get_dictionary(page)
            .ok()
            .and_then(|d| d.get(b"Annots").ok())
            .and_then(|a| pdf.dereference(a).ok())
            .and_then(|(_, a)| a.as_array().ok())
            .cloned()
            .unwrap_or_default();
        for entry in list {
            let Ok(id) = entry.as_reference() else {
                continue;
            };
            let Ok(d) = pdf.get_dictionary(id) else {
                continue;
            };
            if matches!(
                crate::import::subtype(d).as_str(),
                "Link" | "Widget" | "Popup" | ""
            ) {
                continue;
            }
            annots.push((i as u32, id));
        }
    }
    Ok(Comments {
        pdf,
        annots,
        file: None,
    })
}

/// What [`import`] did, for the notice.
#[derive(Debug, Default, PartialEq)]
pub struct Imported {
    pub edits: Vec<Edit>,
    /// New comments.
    pub added: usize,
    /// Comments already here (by `/NM`) brought up to date.
    pub updated: usize,
    /// Comments sheaf can't hold, by subtype.
    pub skipped: BTreeMap<String, usize>,
    /// Comments on pages this document doesn't have.
    pub off_the_end: usize,
}

impl Imported {
    /// "12 added, 3 updated; 2 Sound comments skipped (sheaf can't hold
    /// them)".
    pub fn summary(&self) -> String {
        let mut out = format!("{} added, {} updated", self.added, self.updated);
        if !self.skipped.is_empty() {
            let kinds: Vec<String> = self
                .skipped
                .iter()
                .map(|(k, n)| format!("{n} {k}"))
                .collect();
            out.push_str(&format!(
                "; skipped {} (sheaf can't hold them)",
                kinds.join(", ")
            ));
        }
        if self.off_the_end > 0 {
            out.push_str(&format!(
                "; {} on pages this document doesn't have",
                self.off_the_end
            ));
        }
        out
    }
}

/// The edits that bring `comments` into the document `st` (with its
/// `sources`, and the other tools' comments read from them, `foreign`):
/// one comment whose `/NM` is one of our marks' updates it; one whose
/// `/NM` is another tool's comment here takes its place, as making it
/// editable does; the rest are added. Pages are counted in the document's
/// order.
pub fn import(
    st: &DocState,
    sources: &HashMap<SourceId, Arc<SourceInfo>>,
    foreign: &[(SourceId, ForeignAnnot)],
    comments: &Comments,
) -> Imported {
    let mut out = Imported::default();
    let ours: HashMap<String, (PageId, AnnotId)> = st
        .annots
        .iter()
        .flat_map(|(page, list)| list.iter().map(move |a| (a.name(), (*page, a.id))))
        .collect();
    let theirs: std::collections::HashSet<&str> =
        foreign.iter().map(|(_, f)| f.name.as_str()).collect();
    let mut geoms = Geoms::new(sources);
    let mut seen = std::collections::HashSet::new();
    // By page, so a replacement's parts meet.
    let mut by_page: BTreeMap<u32, Vec<&Dictionary>> = BTreeMap::new();
    for i in 0..comments.len() {
        if let Some(d) = comments.dict(i) {
            by_page.entry(comments.annots[i].0).or_default().push(d);
        }
    }
    for (page, dicts) in by_page {
        let Some(pref) = st.pages.get(page as usize).copied() else {
            out.off_the_end += dicts
                .iter()
                .filter(|d| crate::import::subtype(d) != "Popup")
                .count();
            continue;
        };
        let geom = geoms.of(&pref);
        let converted =
            crate::import::convert_page(&comments.pdf, &dicts, &geom, Options { files: true });
        for (dict, mark) in dicts.iter().zip(converted) {
            // One of sheaf's comes back exactly as it was, under the name
            // it had there, so what replies to it there does here.
            let mark = match crate::file::sheaf_mark(dict) {
                Some(mut a) => {
                    if a.name.is_empty() {
                        a.name = a.name();
                    }
                    if let crate::annot::AnnotKind::Connector { partner, .. } = &mut a.kind {
                        *partner = None;
                    }
                    Ok(a)
                }
                None => mark,
            };
            let mut a = match mark {
                Ok(a) => a,
                Err(NotAMark::Unsupported(kind)) => {
                    *out.skipped.entry(kind).or_default() += 1;
                    continue;
                }
                Err(_) => continue,
            };
            if a.name.is_empty() {
                a.name = thread_name(dict);
            }
            if !seen.insert(a.name.clone()) {
                continue;
            }
            match ours.get(&a.name) {
                Some(&(at, id)) => {
                    a.id = id;
                    out.edits.push(Edit::AnnotUpdate { page: at, annot: a });
                    out.updated += 1;
                }
                None => {
                    if theirs.contains(a.name.as_str()) {
                        out.updated += 1;
                    } else {
                        out.added += 1;
                    }
                    out.edits.push(Edit::AnnotAdd {
                        page: pref.id,
                        annot: a,
                    });
                }
            }
        }
    }
    out
}

/// Comments to write out: our marks and other tools' comments, as
/// dictionaries in a [`Comments`], replies pointing at what they reply to
/// when it's written too. `ours` and `foreign` (indexes into
/// `foreign_all`) say which; pages are counted in the document's order.
pub fn gather(
    st: &DocState,
    sources: &HashMap<SourceId, Arc<SourceInfo>>,
    foreign_all: &[(SourceId, ForeignAnnot)],
    ours: &[(PageId, AnnotId)],
    foreign: &[usize],
) -> Result<Comments> {
    let mut out = Comments::new();
    let mut geoms = Geoms::new(sources);
    let mut forms = HashMap::new();
    // Each written comment's name and parent's name, to link them after.
    let mut names: HashMap<String, ObjectId> = HashMap::new();
    let mut parents: Vec<(ObjectId, String)> = Vec::new();
    for &(page, id) in ours {
        let Some(index) = st.index_of(page) else {
            continue;
        };
        let Some(a) = st
            .annots_on(page)
            .and_then(|l| l.iter().find(|a| a.id == id))
        else {
            continue;
        };
        let geom = geoms.of(&st.pages[index]);
        let written = crate::annot::to_pdf_in(a, &geom, &mut out.pdf, &mut forms);
        let mut main = written.main;
        main.set("Page", index as i64);
        let main_id = out.pdf.add_object(Object::Dictionary(main));
        out.annots.push((index as u32, main_id));
        names.insert(a.name(), main_id);
        if let Some(p) = a.thread_parent() {
            parents.push((main_id, p.to_string()));
        }
        for mut part in written.companions {
            part.set("IRT", main_id);
            part.set("RT", Object::Name(b"Group".to_vec()));
            part.set("Page", index as i64);
            let part_id = out.pdf.add_object(Object::Dictionary(part));
            out.annots.push((index as u32, part_id));
        }
    }
    let mut maps: HashMap<SourceId, HashMap<ObjectId, ObjectId>> = HashMap::new();
    for &ix in foreign {
        let Some((source, f)) = foreign_all.get(ix) else {
            continue;
        };
        let Some(index) = st
            .pages
            .iter()
            .position(|p| p.source == *source && p.index == f.page)
        else {
            continue;
        };
        let Some(src) = geoms.pdf(*source) else {
            continue;
        };
        let Some(dict) = annot_in(src, f.page, f.slot) else {
            continue;
        };
        let mut dict = dict.clone();
        // Not what points back into the file it came from.
        for key in [
            b"P".as_slice(),
            b"Popup",
            b"Parent",
            b"IRT",
            b"StructParent",
        ] {
            dict.remove(key);
        }
        let map = maps.entry(*source).or_default();
        let Ok(Object::Dictionary(mut copied)) =
            crate::save::remap(src, &Object::Dictionary(dict), &mut out.pdf, map)
        else {
            continue;
        };
        copied.set("Page", index as i64);
        if !copied.has(b"NM") {
            copied.set("NM", crate::save::text_string(&f.name));
        }
        let id = out.pdf.add_object(Object::Dictionary(copied));
        out.annots.push((index as u32, id));
        names.insert(f.name.clone(), id);
        if let Some(p) = &f.parent {
            parents.push((id, p.clone()));
        }
    }
    for (child, parent) in parents {
        let to = match names.get(&parent) {
            Some(&p) => Object::Reference(p),
            // Not written: by name, as XFDF says it.
            None => crate::save::text_string(&parent),
        };
        if let Ok(d) = out.pdf.get_dictionary_mut(child) {
            d.set("IRT", to);
        }
    }
    out.file = st
        .pages
        .front()
        .and_then(|p| sources.get(&p.source))
        .and_then(|s| s.name.file_name())
        .map(|n| n.to_string_lossy().into_owned());
    Ok(out)
}

/// Page `page`'s annotation in `/Annots` slot `slot`.
fn annot_in(pdf: &Pdf, page: u32, slot: usize) -> Option<&Dictionary> {
    let id = *pdf.get_pages().get(&(page + 1))?;
    let list = pdf
        .dereference(pdf.get_dictionary(id).ok()?.get(b"Annots").ok()?)
        .ok()?
        .1
        .as_array()
        .ok()?;
    pdf.dereference(list.get(slot)?).ok()?.1.as_dict().ok()
}

/// The sources' files, loaded when first needed, and their pages' boxes.
struct Geoms<'a> {
    sources: &'a HashMap<SourceId, Arc<SourceInfo>>,
    loaded: HashMap<SourceId, Option<Pdf>>,
}

impl<'a> Geoms<'a> {
    fn new(sources: &'a HashMap<SourceId, Arc<SourceInfo>>) -> Self {
        Geoms {
            sources,
            loaded: HashMap::new(),
        }
    }

    fn pdf(&mut self, source: SourceId) -> Option<&Pdf> {
        let sources = self.sources;
        self.loaded
            .entry(source)
            .or_insert_with(|| {
                let info = sources.get(&source)?;
                crate::file::load_source(info)
                    .map_err(|e| log::warn!("{}: {e:#}", info.name.display()))
                    .ok()
            })
            .as_ref()
    }

    /// A page's box and rotation in its file, else its size, upright.
    fn of(&mut self, pref: &PageRef) -> PageGeom {
        if let Some(pdf) = self.pdf(pref.source)
            && let Some(&id) = pdf.get_pages().get(&(pref.index + 1))
        {
            return crate::file::page_geom(pdf, id);
        }
        let size = self
            .sources
            .get(&pref.source)
            .map(|s| s.src.page_size(pref.index))
            .unwrap_or(sheaf_core::PageSize {
                width: 612.0,
                height: 792.0,
            });
        PageGeom {
            media: [0.0, 0.0, size.width, size.height],
            rotate: 0,
        }
    }
}

#[cfg(test)]
mod tests;
