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

use anyhow::{Context, Result};
use lopdf::{Dictionary, Document as Pdf, Object, ObjectId};

use crate::annot::AnnotId;
use crate::assets::MarkAssets;
use crate::file::{ForeignAnnot, thread_name};
use crate::import::{NotAMark, Options};
use crate::model::{Edit, PageId, Snapshot, SourceId};

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

/// The edits that bring `comments` into the document `snap` (with the
/// other tools' comments read from its sources, `foreign`): one comment
/// whose `/NM` is one of our marks' updates it; one whose `/NM` is another
/// tool's comment here takes its place, as making it editable does; the
/// rest are added. Pages are counted in the document's order, and a
/// comment reads upright as its page shows. What our marks hold, a page
/// stamp's page and an attachment's file, goes into the document's
/// stamps and files where it has none of its own.
pub fn import(
    snap: &Snapshot,
    foreign: &[(SourceId, ForeignAnnot)],
    comments: &Comments,
) -> Imported {
    let st = &snap.state;
    let mut out = Imported::default();
    let ours: HashMap<String, (PageId, AnnotId)> = st
        .annots
        .iter()
        .flat_map(|(page, list)| list.iter().map(move |a| (a.name(), (*page, a.id))))
        .collect();
    let theirs: std::collections::HashSet<&str> =
        foreign.iter().map(|(_, f)| f.name.as_str()).collect();
    let mut seen = std::collections::HashSet::new();
    // By page, so a replacement's parts meet.
    let mut by_page: BTreeMap<u32, Vec<ObjectId>> = BTreeMap::new();
    for &(page, id) in &comments.annots {
        if comments.pdf.get_dictionary(id).is_ok() {
            by_page.entry(page).or_default().push(id);
        }
    }
    for (page, ids) in by_page {
        let entries: Vec<Object> = ids.iter().map(|&id| Object::Reference(id)).collect();
        let dicts: Vec<&Dictionary> = ids
            .iter()
            .filter_map(|&id| comments.pdf.get_dictionary(id).ok())
            .collect();
        let Some(pref) = st.pages.get(page as usize).copied() else {
            out.off_the_end += dicts
                .iter()
                .filter(|d| crate::import::subtype(d) != "Popup")
                .count();
            continue;
        };
        let converted = crate::import::convert_page(
            &comments.pdf,
            &dicts,
            pref.rotation,
            Options { files: true },
        );
        let page_annots = crate::file::PageAnnots::new(&entries);
        for ((dict, entry), mark) in dicts.iter().zip(&entries).zip(converted) {
            // One of sheaf's comes back exactly as it was, under the name
            // it had there, so what replies to it there does here; one
            // another tool has edited since is that tool's.
            let own = crate::file::sheaf_mark(&comments.pdf, entry, pref.rotation, &page_annots);
            let mark = match own {
                Some(mut a) => {
                    bring_held(&comments.pdf, dict, &a, &snap.assets);
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

/// What `a`, one of our marks, holds in `pdf`, its dictionary `dict`, put
/// in `assets` where they have no copy: its stamp's page, as a one-page
/// PDF, or its file's bytes.
fn bring_held(pdf: &Pdf, dict: &Dictionary, a: &crate::Annot, assets: &MarkAssets) {
    use crate::annot::AnnotKind;
    match &a.kind {
        AnnotKind::PageStamp { stamp, .. } if assets.stamp(stamp).is_none() => {
            let page = crate::file::stamp_page(pdf, dict)
                .map(|form| crate::stamp_lib::form_to_pdf(pdf, form));
            match page {
                Some(Ok(page)) => assets.intern_stamp(stamp, page.into()),
                Some(Err(e)) => log::warn!("stamp {stamp}: {e:#}"),
                None => {}
            }
        }
        AnnotKind::FileAttachment { file, .. } if assets.file(file).is_none() => {
            let bytes = crate::file::attached_file(pdf, dict)
                .and_then(|id| pdf.get_object(id).ok()?.as_stream().ok())
                .map(crate::attachments::file_bytes);
            match bytes {
                Some(Ok(bytes)) => assets.intern_file(file, bytes.into()),
                Some(Err(e)) => log::warn!("attachment {file}: {e:#}"),
                None => {}
            }
        }
        _ => {}
    }
}

/// Comments to write out of `snap`: our marks and other tools' comments,
/// as dictionaries in a [`Comments`], replies pointing at what they reply
/// to when it's written too. `ours` and `foreign` (indexes into
/// `foreign_all`) say which; pages are counted in the document's order.
pub fn gather(
    snap: &Snapshot,
    foreign_all: &[(SourceId, ForeignAnnot)],
    ours: &[(PageId, AnnotId)],
    foreign: &[usize],
) -> Result<Comments> {
    let (st, sources) = (&snap.state, &snap.sources);
    let mut out = Comments::new();
    let mut cx = crate::annot::WriteCx::new(snap.assets.clone());
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
        let written = crate::annot::to_pdf_in(a, &mut out.pdf, &mut cx);
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
        let Some(src) = sources.get(source).map(|s| &s.src) else {
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
            crate::save::remap(src.pdf(), &Object::Dictionary(dict), &mut out.pdf, map)
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
fn annot_in(src: &crate::Source, page: u32, slot: usize) -> Option<&Dictionary> {
    let entry = src.page(page)?.annots.get(slot)?;
    src.pdf().dereference(entry).ok()?.1.as_dict().ok()
}

#[cfg(test)]
mod tests;
