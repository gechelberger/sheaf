//! Materialize a [`Document`] snapshot into PDF bytes.
//!
//! The rewrite strategy: load the primary source with lopdf, rebuild its page
//! tree from the virtual page list (copying pages from other sources with
//! object renumbering), apply rotations, rebuild the outline, prune what's no
//! longer referenced, and serialize. Always correct, O(file). The incremental
//! strategy (append-only, preserving original bytes) lives alongside later.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result, bail};
use im::Vector;
use lopdf::{Dictionary, Document as Pdf, Object, ObjectId, StringFormat, dictionary};

use crate::annot::{Annot, AnnotId, AnnotKind, PageGeom, to_pdf};
use crate::model::{DocState, OutlineNode, PageId, Snapshot, SourceId};

/// Page-level keys a page may inherit from its ancestors in the page tree.
/// We flatten the tree, so these must be copied onto each page.
const INHERITED: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

/// Serialize a snapshot as a complete new PDF.
pub fn rewrite(model: &Snapshot) -> Result<Vec<u8>> {
    rewrite_with(model, false)
}

/// PDFlyer's "Export Clean Copy": the document as it stands, with every
/// markup annotation removed, ours and anyone else's. Links, form fields
/// and bookmarks stay. Refuses to overwrite the open file.
pub fn export_clean_copy(model: &Snapshot, path: &Path) -> Result<()> {
    let primary = model
        .source(model.primary)
        .context("primary source missing")?;
    if path.exists()
        && std::fs::canonicalize(path).ok() == std::fs::canonicalize(&primary.path).ok()
    {
        bail!("a clean copy can't replace the open file; choose another name");
    }
    let bytes = rewrite_with(model, true)?;
    write_atomic(path, &bytes)
}

/// `clean`: strip all markup instead of writing our annotations.
fn rewrite_with(model: &Snapshot, clean: bool) -> Result<Vec<u8>> {
    let primary_id = model.primary;
    let primary_info = model.source(primary_id).context("primary source missing")?;
    let mut pdf = crate::file::load_pdf(&primary_info.path, primary_info.password.as_deref())?;

    // Source page object ids and their inherited attributes, resolved against
    // the *original* trees before anything is re-parented.
    let mut sources: HashMap<SourceId, LoadedSource> = HashMap::new();
    sources.insert(primary_id, LoadedSource::from_pdf(&pdf)?);
    for (&sid, info) in &model.sources {
        if sid != primary_id && model.state.pages.iter().any(|p| p.source == sid) {
            let other = crate::file::load_pdf(&info.path, info.password.as_deref())?;
            sources.insert(sid, LoadedSource::from_pdf_owned(other)?);
        }
    }

    let state = &model.state;
    let pages_id = pdf.new_object_id();
    let mut kids: Vec<Object> = Vec::with_capacity(state.pages.len());
    let mut page_obj: HashMap<PageId, ObjectId> = HashMap::new();
    let mut used: HashSet<ObjectId> = HashSet::new();
    let mut copy_maps: HashMap<SourceId, HashMap<ObjectId, ObjectId>> = HashMap::new();
    let mut connectors = Vec::new();

    for pref in &state.pages {
        let src = sources.get(&pref.source).context("unknown source")?;
        let src_page = *src
            .pages
            .get(pref.index as usize)
            .context("source page index out of range")?;
        let inherited = src.inherited.get(&src_page).cloned().unwrap_or_default();

        let mut dict: Dictionary = if pref.source == primary_id {
            pdf.get_dictionary(src_page)?.clone()
        } else {
            let map = copy_maps.entry(pref.source).or_default();
            let src_pdf = src
                .pdf
                .as_ref()
                .expect("non-primary sources keep their pdf");
            copy_page_dict(src_pdf, src_page, &mut pdf, map)?
        };
        for (k, v) in &inherited {
            if !dict.has(k) {
                dict.set(k.clone(), v.clone());
            }
        }
        // Rotation: the source's own /Rotate plus our delta.
        let original = dict
            .get(b"Rotate")
            .ok()
            .and_then(|o| o.as_i64().ok())
            .unwrap_or(0);
        if clean {
            strip_markup_annots(&pdf, &mut dict);
        } else if let Some(list) = state.annots.get(&pref.id) {
            let geom = geom_of(&dict, original as i32);
            connectors.extend(write_page_annots(&mut pdf, &mut dict, list.iter(), &geom));
        } else {
            strip_sheaf_annots(&pdf, &mut dict);
        }
        let rotate = (original + pref.rotation.degrees() as i64).rem_euclid(360);
        if rotate == 0 {
            dict.remove(b"Rotate");
        } else {
            dict.set("Rotate", rotate);
        }
        dict.set("Parent", pages_id);
        dict.remove(b"Type");
        dict.set("Type", Object::Name(b"Page".to_vec()));

        // A page object may appear only once in the tree; duplicates get a
        // fresh object sharing the same content and resources by reference.
        let id = if pref.source == primary_id && used.insert(src_page) {
            *pdf.get_object_mut(src_page)? = Object::Dictionary(dict);
            src_page
        } else {
            pdf.add_object(dict)
        };
        kids.push(id.into());
        page_obj.insert(pref.id, id);
    }

    link_connectors(&mut pdf, &connectors, state, &page_obj);

    pdf.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => kids,
            "Count" => state.pages.len() as i64,
        }),
    );

    let catalog_id = pdf.trailer.get(b"Root")?.as_reference()?;
    let catalog = pdf.get_dictionary_mut(catalog_id)?;
    catalog.set("Pages", pages_id);
    // Page labels index by position; they'd be wrong after any reorder.
    if !is_identity(model, &sources[&primary_id]) {
        catalog.remove(b"PageLabels");
    }
    catalog.remove(b"Outlines");
    if !state.outline.is_empty() {
        let outlines_id = write_outline(&mut pdf, &state.outline, &page_obj);
        pdf.get_dictionary_mut(catalog_id)?
            .set("Outlines", outlines_id);
    }

    pdf.prune_objects();
    pdf.renumber_objects();
    pdf.compress();
    let mut out = Vec::new();
    pdf.save_to(&mut out)?;
    Ok(out)
}

/// Which strategy a save used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    Incremental,
    Rewrite,
}

/// Save `model` to `path`, incrementally when that's its own primary file
/// (and the file on disk is still what we opened), otherwise by rewrite.
/// An incremental failure falls back to a rewrite.
pub fn save_to_path(model: &Snapshot, path: &Path) -> Result<Strategy> {
    let primary = model
        .source(model.primary)
        .context("primary source missing")?;
    let same_file = std::fs::canonicalize(path).ok() == std::fs::canonicalize(&primary.path).ok();
    let unchanged = primary.bytes == 0
        || std::fs::metadata(&primary.path)
            .map(|m| m.len() as usize == primary.bytes)
            .unwrap_or(false);
    if same_file && unchanged {
        match crate::incremental::incremental(model) {
            Ok(bytes) => {
                write_atomic(path, &bytes)?;
                return Ok(Strategy::Incremental);
            }
            Err(e) => log::warn!("incremental save failed, rewriting instead: {e:#}"),
        }
    }
    rewrite_to_path(model, path)?;
    Ok(Strategy::Rewrite)
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let tmp = dir.join(format!(
        ".{}.sheaf-tmp",
        path.file_name().and_then(|s| s.to_str()).unwrap_or("save")
    ));
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

/// Rewrite to `path` atomically (temp file + rename).
pub fn rewrite_to_path(model: &Snapshot, path: &Path) -> Result<()> {
    let bytes = rewrite(model)?;
    write_atomic(path, &bytes)
}

pub(crate) struct LoadedSource {
    /// Kept for non-primary sources so pages can be copied out of them.
    pub(crate) pdf: Option<Pdf>,
    /// Page object ids in page order.
    pub(crate) pages: Vec<ObjectId>,
    /// Inherited attributes per page, as found in the original tree.
    pub(crate) inherited: HashMap<ObjectId, Vec<(Vec<u8>, Object)>>,
}

impl LoadedSource {
    pub(crate) fn from_pdf(pdf: &Pdf) -> Result<Self> {
        let pages: Vec<ObjectId> = pdf.get_pages().into_values().collect();
        let mut inherited = HashMap::new();
        for &page in &pages {
            inherited.insert(page, inherited_attrs(pdf, page));
        }
        Ok(Self {
            pdf: None,
            pages,
            inherited,
        })
    }
}

impl LoadedSource {
    pub(crate) fn from_pdf_owned(pdf: Pdf) -> Result<Self> {
        let mut s = Self::from_pdf(&pdf)?;
        s.pdf = Some(pdf);
        Ok(s)
    }
}

/// Values of the inheritable keys, resolved by walking up `/Parent`.
fn inherited_attrs(pdf: &Pdf, page: ObjectId) -> Vec<(Vec<u8>, Object)> {
    let mut out = Vec::new();
    let mut found: HashSet<&[u8]> = HashSet::new();
    let mut id = page;
    for _ in 0..64 {
        let Ok(dict) = pdf.get_dictionary(id) else {
            break;
        };
        for key in INHERITED {
            if !found.contains(key)
                && let Ok(v) = dict.get(key)
            {
                found.insert(key);
                if id != page {
                    out.push((key.to_vec(), v.clone()));
                }
            }
        }
        match dict.get(b"Parent").and_then(|p| p.as_reference()) {
            Ok(parent) => id = parent,
            Err(_) => break,
        }
    }
    out
}

pub(crate) fn is_identity(model: &Snapshot, primary: &LoadedSource) -> bool {
    let pages = &model.state.pages;
    pages.len() == primary.pages.len()
        && pages
            .iter()
            .enumerate()
            .all(|(i, p)| p.source == model.primary && p.index as usize == i)
}

/// Clone a page dictionary from `src` into `dst`, deep-copying everything it
/// references (resources, content, annotations…) with fresh object numbers.
pub(crate) fn copy_page_dict(
    src: &Pdf,
    page: ObjectId,
    dst: &mut Pdf,
    map: &mut HashMap<ObjectId, ObjectId>,
) -> Result<Dictionary> {
    let dict = src.get_dictionary(page)?.clone();
    // The page's own id must map to the object we're about to create so
    // annotations' /P back-references resolve; the caller inserts it.
    let new_id = dst.new_object_id();
    map.insert(page, new_id);
    let mut out = Dictionary::new();
    for (k, v) in dict.iter() {
        if k == b"Parent" {
            continue;
        }
        out.set(k.clone(), remap(src, v, dst, map)?);
    }
    // Reserve the id so later allocations don't collide; the caller may
    // replace this placeholder, or it stays as the page.
    dst.objects.insert(new_id, Object::Dictionary(out.clone()));
    Ok(out)
}

fn copy_object(
    src: &Pdf,
    id: ObjectId,
    dst: &mut Pdf,
    map: &mut HashMap<ObjectId, ObjectId>,
) -> Result<ObjectId> {
    if let Some(&m) = map.get(&id) {
        return Ok(m);
    }
    let new_id = dst.new_object_id();
    map.insert(id, new_id);
    // Placeholder first so cycles terminate.
    dst.objects.insert(new_id, Object::Null);
    let obj = src.get_object(id)?.clone();
    let obj = remap(src, &obj, dst, map)?;
    dst.objects.insert(new_id, obj);
    Ok(new_id)
}

fn remap(
    src: &Pdf,
    obj: &Object,
    dst: &mut Pdf,
    map: &mut HashMap<ObjectId, ObjectId>,
) -> Result<Object> {
    Ok(match obj {
        Object::Reference(r) => Object::Reference(copy_object(src, *r, dst, map)?),
        Object::Array(items) => Object::Array(
            items
                .iter()
                .map(|o| remap(src, o, dst, map))
                .collect::<Result<_>>()?,
        ),
        Object::Dictionary(d) => {
            let mut out = Dictionary::new();
            for (k, v) in d.iter() {
                out.set(k.clone(), remap(src, v, dst, map)?);
            }
            Object::Dictionary(out)
        }
        Object::Stream(s) => {
            let mut dict = Dictionary::new();
            for (k, v) in s.dict.iter() {
                dict.set(k.clone(), remap(src, v, dst, map)?);
            }
            let mut stream = lopdf::Stream::new(dict, s.content.clone());
            stream.allows_compression = s.allows_compression;
            Object::Stream(stream)
        }
        other => other.clone(),
    })
}

/// PDF text string: PDFDocEncoding when ASCII, otherwise UTF-16BE with BOM.
fn text_string(s: &str) -> Object {
    if s.is_ascii() {
        Object::String(s.as_bytes().to_vec(), StringFormat::Literal)
    } else {
        let mut bytes = vec![0xFE, 0xFF];
        for u in s.encode_utf16() {
            bytes.extend_from_slice(&u.to_be_bytes());
        }
        Object::String(bytes, StringFormat::Hexadecimal)
    }
}

pub(crate) fn write_outline(
    pdf: &mut Pdf,
    nodes: &Vector<OutlineNode>,
    page_obj: &HashMap<PageId, ObjectId>,
) -> ObjectId {
    fn level(
        pdf: &mut Pdf,
        parent: ObjectId,
        nodes: &Vector<OutlineNode>,
        page_obj: &HashMap<PageId, ObjectId>,
    ) -> (Option<ObjectId>, Option<ObjectId>, i64) {
        let ids: Vec<ObjectId> = nodes.iter().map(|_| pdf.new_object_id()).collect();
        let mut total = 0;
        for (i, node) in nodes.iter().enumerate() {
            let mut dict = dictionary! { "Title" => text_string(&node.title), "Parent" => parent };
            if i > 0 {
                dict.set("Prev", ids[i - 1]);
            }
            if i + 1 < ids.len() {
                dict.set("Next", ids[i + 1]);
            }
            if let Some(obj) = node.target.and_then(|t| page_obj.get(&t)) {
                dict.set(
                    "Dest",
                    vec![
                        Object::Reference(*obj),
                        "XYZ".into(),
                        Object::Null,
                        Object::Null,
                        Object::Null,
                    ],
                );
            }
            if let Some([r, g, b]) = node.color {
                dict.set(
                    "C",
                    vec![
                        (r as f32 / 255.0).into(),
                        (g as f32 / 255.0).into(),
                        (b as f32 / 255.0).into(),
                    ],
                );
            }
            let (first, last, count) = level(pdf, ids[i], &node.children, page_obj);
            if let (Some(f), Some(l)) = (first, last) {
                dict.set("First", f);
                dict.set("Last", l);
                dict.set("Count", count);
            }
            total += 1 + count;
            pdf.objects.insert(ids[i], Object::Dictionary(dict));
        }
        (ids.first().copied(), ids.last().copied(), total)
    }
    let outlines_id = pdf.new_object_id();
    let (first, last, count) = level(pdf, outlines_id, nodes, page_obj);
    let mut dict = dictionary! { "Type" => "Outlines" };
    if let (Some(f), Some(l)) = (first, last) {
        dict.set("First", f);
        dict.set("Last", l);
        dict.set("Count", count);
    }
    pdf.objects.insert(outlines_id, Object::Dictionary(dict));
    outlines_id
}

/// Sanity check used by tests and the app: the page refs must point at
/// pages that exist in their sources.
pub fn validate(model: &Snapshot) -> Result<()> {
    for p in &model.state.pages {
        let info = model.source(p.source).context("unknown source")?;
        if p.index >= info.page_count {
            bail!("page ref {:?} out of range for {}", p, info.path.display());
        }
    }
    Ok(())
}

/// Page geometry from a (flattened) page dictionary and its original rotation.
pub(crate) fn geom_of(dict: &Dictionary, rotate: i32) -> PageGeom {
    let media = dict
        .get(b"MediaBox")
        .ok()
        .and_then(|o| o.as_array().ok())
        .filter(|a| a.len() == 4)
        .map(|a| {
            let v: Vec<f32> = a.iter().map(|o| o.as_float().unwrap_or(0.0)).collect();
            [
                v[0].min(v[2]),
                v[1].min(v[3]),
                v[0].max(v[2]),
                v[1].max(v[3]),
            ]
        })
        .unwrap_or([0.0, 0.0, 612.0, 792.0]);
    PageGeom {
        media,
        rotate: rotate.rem_euclid(360),
    }
}

/// Is this /Annots entry one of ours?
fn is_sheaf_annot(pdf: &Pdf, entry: &Object) -> bool {
    pdf.dereference(entry)
        .ok()
        .and_then(|(_, o)| o.as_dict().ok().map(|d| d.has(b"SheafAnnot")))
        .unwrap_or(false)
}

/// Is this /Annots entry navigation or a form field rather than markup?
/// Our connectors are links too, but they're markup.
fn is_link_or_field(pdf: &Pdf, entry: &Object) -> bool {
    pdf.dereference(entry)
        .ok()
        .and_then(|(_, o)| o.as_dict().ok())
        .filter(|d| !d.has(b"SheafAnnot"))
        .and_then(|d| d.get(b"Subtype").ok())
        .and_then(|s| s.as_name().ok())
        .is_some_and(|n| n == b"Link" || n == b"Widget")
}

/// Drop every markup annotation from a page, keeping links and form fields.
pub(crate) fn strip_markup_annots(pdf: &Pdf, page: &mut Dictionary) {
    let Some(existing) = page
        .get(b"Annots")
        .ok()
        .and_then(|a| pdf.dereference(a).ok())
        .and_then(|(_, o)| o.as_array().ok().cloned())
    else {
        return;
    };
    let kept: Vec<Object> = existing
        .into_iter()
        .filter(|e| is_link_or_field(pdf, e))
        .collect();
    if kept.is_empty() {
        page.remove(b"Annots");
    } else {
        page.set("Annots", Object::Array(kept));
    }
}

/// Drop sheaf-written entries from a page's /Annots, keeping third-party ones.
pub(crate) fn strip_sheaf_annots(pdf: &Pdf, page: &mut Dictionary) {
    let Some(existing) = page
        .get(b"Annots")
        .ok()
        .and_then(|a| pdf.dereference(a).ok())
        .and_then(|(_, o)| o.as_array().ok().cloned())
    else {
        return;
    };
    let kept: Vec<Object> = existing
        .into_iter()
        .filter(|e| !is_sheaf_annot(pdf, e))
        .collect();
    if kept.is_empty() {
        page.remove(b"Annots");
    } else {
        page.set("Annots", Object::Array(kept));
    }
}

/// A connector written during a save, so its `/Dest` can be filled in once
/// every page has an object id (see `link_connectors`).
pub(crate) struct ConnectorRef {
    pub obj: ObjectId,
    pub id: AnnotId,
    pub partner: Option<AnnotId>,
    /// Top-left of the marker in its page's user space.
    pub top_left: (f32, f32),
}

/// Replace the page's sheaf annotations with `annots`, as new objects with
/// appearance streams. `/P` is left unset; PDFium and Acrobat don't need it.
/// Returns the connectors written, for `link_connectors`.
pub(crate) fn write_page_annots<'a>(
    pdf: &mut Pdf,
    page: &mut Dictionary,
    annots: impl Iterator<Item = &'a Annot>,
    geom: &PageGeom,
) -> Vec<ConnectorRef> {
    strip_sheaf_annots(pdf, page);
    let mut list: Vec<Object> = page
        .get(b"Annots")
        .ok()
        .and_then(|a| pdf.dereference(a).ok())
        .and_then(|(_, o)| o.as_array().ok().cloned())
        .unwrap_or_default();
    let mut connectors = Vec::new();
    for a in annots {
        let (mut dict, stream) = to_pdf(a, geom);
        let ap_id = pdf.add_object(Object::Stream(stream));
        dict.set("AP", dictionary! { "N" => ap_id });
        let id = pdf.add_object(Object::Dictionary(dict));
        list.push(Object::Reference(id));
        if let AnnotKind::Connector { rect, partner, .. } = &a.kind {
            let [llx, _, _, ury] = geom.box_to_user(*rect);
            connectors.push(ConnectorRef {
                obj: id,
                id: a.id,
                partner: *partner,
                top_left: (llx, ury),
            });
        }
    }
    if list.is_empty() {
        page.remove(b"Annots");
    } else {
        page.set("Annots", Object::Array(list));
    }
    connectors
}

/// Point each written connector's `/Dest` at its partner: at the partner's
/// position when it was written in this save, else just its page.
pub(crate) fn link_connectors(
    pdf: &mut Pdf,
    connectors: &[ConnectorRef],
    state: &DocState,
    page_obj: &HashMap<PageId, ObjectId>,
) {
    for c in connectors {
        let Some(pid) = c.partner else { continue };
        let Some((ppage, _)) = state.find_annot(pid) else {
            continue;
        };
        let Some(&page_ref) = page_obj.get(&ppage) else {
            continue;
        };
        let mut dest = vec![Object::Reference(page_ref), Object::Name(b"XYZ".to_vec())];
        match connectors.iter().find(|g| g.id == pid) {
            // Land with the partner a little in from the top-left corner.
            Some(g) => dest.extend([
                Object::Real(g.top_left.0 - 36.0),
                Object::Real(g.top_left.1 + 36.0),
                Object::Null,
            ]),
            None => dest.extend([Object::Null, Object::Null, Object::Null]),
        }
        if let Ok(Object::Dictionary(d)) = pdf.get_object_mut(c.obj) {
            d.set("Dest", Object::Array(dest));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::FixedCount;
    use crate::model::{Document, Edit, FlatOutlineItem, SourceInfo};
    use lopdf::content::{Content, Operation};
    use sheaf_core::{PageSize, Rotation};

    /// A PDF whose page i has content "MARK <tag> <i>" and /Rotate from `rot`.
    fn make_pdf(
        dir: &Path,
        name: &str,
        tag: &str,
        n: usize,
        outline: &[(&str, usize, u16)],
    ) -> std::path::PathBuf {
        let mut doc = Pdf::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(
            dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" },
        );
        // Resources on the Pages node, so inheritance is exercised.
        let resources_id =
            doc.add_object(dictionary! { "Font" => dictionary! { "F1" => font_id } });
        let mut kids = Vec::new();
        for i in 0..n {
            let content = Content {
                operations: vec![
                    Operation::new("BT", vec![]),
                    Operation::new("Tf", vec!["F1".into(), 24.into()]),
                    Operation::new("Td", vec![72.into(), 700.into()]),
                    Operation::new(
                        "Tj",
                        vec![Object::string_literal(format!("MARK {tag} {i}"))],
                    ),
                    Operation::new("ET", vec![]),
                ],
            };
            let content_id = doc.add_object(lopdf::Stream::new(
                dictionary! {},
                content.encode().unwrap(),
            ));
            let page_id = doc.add_object(
                dictionary! { "Type" => "Page", "Parent" => pages_id, "Contents" => content_id },
            );
            kids.push(Object::Reference(page_id));
        }
        let mut items: Vec<ObjectId> = Vec::new();
        let outlines_id = doc.new_object_id();
        if !outline.is_empty() {
            // Flat list only (depth ignored beyond 0) is enough for the source side.
            let ids: Vec<ObjectId> = outline.iter().map(|_| doc.new_object_id()).collect();
            for (i, (title, page, _)) in outline.iter().enumerate() {
                let mut d = dictionary! {
                    "Title" => Object::string_literal(*title), "Parent" => outlines_id,
                    "Dest" => vec![kids[*page].clone(), "XYZ".into(), Object::Null, Object::Null, Object::Null],
                };
                if i > 0 {
                    d.set("Prev", ids[i - 1]);
                }
                if i + 1 < ids.len() {
                    d.set("Next", ids[i + 1]);
                }
                doc.objects.insert(ids[i], Object::Dictionary(d));
            }
            items = ids;
        }
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages", "Kids" => kids, "Count" => n as i64, "Resources" => resources_id,
                "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            }),
        );
        let mut catalog = dictionary! { "Type" => "Catalog", "Pages" => pages_id };
        if !items.is_empty() {
            doc.objects.insert(
                outlines_id,
                Object::Dictionary(dictionary! {
                    "Type" => "Outlines", "First" => items[0], "Last" => *items.last().unwrap(), "Count" => items.len() as i64,
                }),
            );
            catalog.set("Outlines", outlines_id);
        }
        let catalog_id = doc.add_object(catalog);
        doc.trailer.set("Root", catalog_id);
        let path = dir.join(name);
        doc.save(&path).unwrap();
        path
    }

    fn info(path: &Path, n: u32) -> SourceInfo {
        SourceInfo {
            path: path.to_path_buf(),
            page_count: n,
            first_page: PageSize {
                width: 612.0,
                height: 792.0,
            },
            page_sizes: None,
            bytes: 0,
            password: None,
        }
    }

    /// (marker text, /Rotate) per page of a written PDF, in order.
    fn read_pages(bytes: &[u8]) -> Vec<(String, i64)> {
        let pdf = Pdf::load_mem(bytes).unwrap();
        pdf.get_pages()
            .into_values()
            .map(|id| {
                let content = pdf.get_page_content(id);
                let text = String::from_utf8_lossy(&content);
                let mark = text
                    .split("MARK ")
                    .nth(1)
                    .unwrap()
                    .split(')')
                    .next()
                    .unwrap()
                    .to_string();
                // Attributes may live on the page or be inherited; both are valid.
                let resolved = inherited_attrs(&pdf, id);
                let dict = pdf.get_dictionary(id).unwrap();
                let has = |k: &[u8]| dict.has(k) || resolved.iter().any(|(kk, _)| kk == k);
                let rot = dict
                    .get(b"Rotate")
                    .ok()
                    .cloned()
                    .or_else(|| {
                        resolved
                            .iter()
                            .find(|(k, _)| k == b"Rotate")
                            .map(|(_, v)| v.clone())
                    })
                    .and_then(|o| o.as_i64().ok())
                    .unwrap_or(0);
                assert!(has(b"Resources"), "page lost its resources");
                assert!(has(b"MediaBox"), "page lost its media box");
                (mark, rot)
            })
            .collect()
    }

    /// Outline as (depth, title, page number) by walking the written tree.
    fn read_outline(bytes: &[u8]) -> Vec<(u16, String, Option<u32>)> {
        let pdf = Pdf::load_mem(bytes).unwrap();
        let page_no: HashMap<ObjectId, u32> =
            pdf.get_pages().into_iter().map(|(n, id)| (id, n)).collect();
        fn walk(
            pdf: &Pdf,
            first: Option<ObjectId>,
            depth: u16,
            page_no: &HashMap<ObjectId, u32>,
            out: &mut Vec<(u16, String, Option<u32>)>,
        ) {
            let mut cur = first;
            while let Some(id) = cur {
                let d = pdf.get_dictionary(id).unwrap();
                let raw = d.get(b"Title").unwrap().as_str().unwrap();
                let title = if raw.starts_with(&[0xFE, 0xFF]) {
                    let units: Vec<u16> = raw[2..]
                        .chunks(2)
                        .map(|c| u16::from_be_bytes([c[0], c[1]]))
                        .collect();
                    String::from_utf16_lossy(&units)
                } else {
                    String::from_utf8_lossy(raw).to_string()
                };
                let page = d
                    .get(b"Dest")
                    .ok()
                    .and_then(|o| o.as_array().ok())
                    .and_then(|a| a.first())
                    .and_then(|o| o.as_reference().ok())
                    .and_then(|r| page_no.get(&r).copied());
                out.push((depth, title, page));
                walk(
                    pdf,
                    d.get(b"First").ok().and_then(|o| o.as_reference().ok()),
                    depth + 1,
                    page_no,
                    out,
                );
                cur = d.get(b"Next").ok().and_then(|o| o.as_reference().ok());
            }
        }
        let catalog = pdf.trailer.get(b"Root").unwrap().as_reference().unwrap();
        let catalog = pdf.get_dictionary(catalog).unwrap();
        let mut out = Vec::new();
        if let Ok(o) = catalog.get(b"Outlines").and_then(|o| o.as_reference()) {
            let od = pdf.get_dictionary(o).unwrap();
            walk(
                &pdf,
                od.get(b"First").ok().and_then(|o| o.as_reference().ok()),
                0,
                &page_no,
                &mut out,
            );
        }
        out
    }

    #[test]
    fn identity_round_trip() {
        let dir = tempdir();
        let a = make_pdf(&dir, "a.pdf", "A", 3, &[("One", 0, 0), ("Three", 2, 0)]);
        let model = Document::new(
            info(&a, 3),
            &[
                FlatOutlineItem {
                    title: "One".into(),
                    page: Some(0),
                    depth: 0,
                },
                FlatOutlineItem {
                    title: "Three".into(),
                    page: Some(2),
                    depth: 0,
                },
            ],
            Box::new(FixedCount(10)),
        );
        let bytes = rewrite(&model.snapshot()).unwrap();
        assert_eq!(
            read_pages(&bytes),
            vec![("A 0".into(), 0), ("A 1".into(), 0), ("A 2".into(), 0)]
        );
        assert_eq!(
            read_outline(&bytes),
            vec![(0, "One".into(), Some(1)), (0, "Three".into(), Some(3))]
        );
    }

    #[test]
    fn edits_materialize() {
        let dir = tempdir();
        let a = make_pdf(&dir, "a.pdf", "A", 4, &[]);
        let b = make_pdf(&dir, "b.pdf", "B", 2, &[]);
        let mut model = Document::new(
            info(&a, 4),
            &[
                FlatOutlineItem {
                    title: "Top".into(),
                    page: Some(0),
                    depth: 0,
                },
                FlatOutlineItem {
                    title: "Child é".into(),
                    page: Some(2),
                    depth: 1,
                },
            ],
            Box::new(FixedCount(10)),
        );
        let sb = model.add_source(info(&b, 2));
        let ids: Vec<PageId> = model.state().pages.iter().map(|p| p.id).collect();
        model
            .apply(Edit::DeletePages { ids: vec![ids[1]] }, "del")
            .unwrap(); // A0 A2 A3
        model
            .apply(
                Edit::RotatePages {
                    ids: vec![ids[2]],
                    by: Rotation::R90,
                },
                "rot",
            )
            .unwrap();
        model
            .apply(
                Edit::MovePages {
                    ids: vec![ids[3]],
                    before: Some(ids[0]),
                },
                "mv",
            )
            .unwrap(); // A3 A0 A2
        model
            .apply(
                Edit::InsertPages {
                    source: sb,
                    range: 0..2,
                    before: Some(ids[2]),
                    outline: None,
                },
                "ins",
            )
            .unwrap(); // A3 A0 B0 B1 A2
        model
            .apply(Edit::DuplicatePages { ids: vec![ids[0]] }, "dup")
            .unwrap(); // A3 A0 A0 B0 B1 A2

        let bytes = rewrite(&model.snapshot()).unwrap();
        assert_eq!(
            read_pages(&bytes),
            vec![
                ("A 3".into(), 0),
                ("A 0".into(), 0),
                ("A 0".into(), 0),
                ("B 0".into(), 0),
                ("B 1".into(), 0),
                ("A 2".into(), 90),
            ]
        );
        // "Top" -> A0 is now page 2; "Child é" -> A2 is page 6; written as UTF-16.
        let outline = read_outline(&bytes);
        assert_eq!(outline.len(), 2);
        assert_eq!((outline[0].0, outline[0].2), (0, Some(2)));
        assert_eq!((outline[1].0, outline[1].2), (1, Some(6)));
        assert_eq!(outline[0].1, "Top");
        assert_eq!(outline[1].1, "Child é");

        // The written file re-opens cleanly and renders the same page count.
        let reopened = Pdf::load_mem(&bytes).unwrap();
        assert_eq!(reopened.get_pages().len(), 6);
    }

    #[test]
    fn rewrite_to_path_is_atomic() {
        let dir = tempdir();
        let a = make_pdf(&dir, "a.pdf", "A", 2, &[]);
        let model = Document::new(info(&a, 2), &[], Box::new(FixedCount(10)));
        let out = dir.join("out.pdf");
        rewrite_to_path(&model.snapshot(), &out).unwrap();
        assert!(out.exists());
        assert!(!dir.join(".out.pdf.sheaf-tmp").exists());
        assert_eq!(read_pages(&std::fs::read(&out).unwrap()).len(), 2);
    }

    fn save_modern(path: &Path) {
        let mut pdf = Pdf::load(path).unwrap();
        let mut bytes = Vec::new();
        pdf.save_modern(&mut bytes).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn incremental_case(modern: bool) {
        let dir = tempdir();
        let a = make_pdf(&dir, "a.pdf", "A", 4, &[("One", 2, 0)]);
        let b = make_pdf(&dir, "b.pdf", "B", 1, &[]);
        if modern {
            save_modern(&a);
        }
        let original = std::fs::read(&a).unwrap();
        let mut model = Document::new(
            {
                let mut i = info(&a, 4);
                i.bytes = original.len();
                i
            },
            &[FlatOutlineItem {
                title: "One".into(),
                page: Some(2),
                depth: 0,
            }],
            Box::new(FixedCount(10)),
        );
        let ids: Vec<PageId> = model.state().pages.iter().map(|p| p.id).collect();

        // Rotation only: tree untouched, one page object rewritten.
        model
            .apply(
                Edit::RotatePages {
                    ids: vec![ids[1]],
                    by: Rotation::R180,
                },
                "rot",
            )
            .unwrap();
        let bytes = crate::incremental::incremental(&model.snapshot()).unwrap();
        assert!(bytes.starts_with(&original), "original bytes preserved");
        assert_eq!(
            read_pages(&bytes),
            vec![
                ("A 0".into(), 0),
                ("A 1".into(), 180),
                ("A 2".into(), 0),
                ("A 3".into(), 0)
            ]
        );
        assert_eq!(read_outline(&bytes), vec![(0, "One".into(), Some(3))]);

        // Structural: delete, insert from another file, bookmark rename.
        let sb = model.add_source(info(&b, 1));
        model
            .apply(Edit::DeletePages { ids: vec![ids[0]] }, "del")
            .unwrap();
        model
            .apply(
                Edit::InsertPages {
                    source: sb,
                    range: 0..1,
                    before: None,
                    outline: None,
                },
                "ins",
            )
            .unwrap();
        let one = model.state().outline_flat()[0].1.id;
        model
            .apply(
                Edit::OutlineEdit {
                    id: one,
                    title: Some("Uno".into()),
                    target: Some(Some(ids[2])),
                    color: None,
                },
                "bm",
            )
            .unwrap();
        let bytes = crate::incremental::incremental(&model.snapshot()).unwrap();
        assert!(bytes.starts_with(&original));
        assert_eq!(
            read_pages(&bytes),
            vec![
                ("A 1".into(), 180),
                ("A 2".into(), 0),
                ("A 3".into(), 0),
                ("B 0".into(), 0)
            ]
        );
        assert_eq!(read_outline(&bytes), vec![(0, "Uno".into(), Some(2))]);

        // Saving through the chooser picks incremental for the primary path.
        assert_eq!(
            save_to_path(&model.snapshot(), &a).unwrap(),
            Strategy::Incremental
        );
        assert!(std::fs::read(&a).unwrap().starts_with(&original));
        assert_eq!(
            save_to_path(&model.snapshot(), &dir.join("elsewhere.pdf")).unwrap(),
            Strategy::Rewrite
        );
    }

    #[test]
    fn incremental_classic_xref() {
        incremental_case(false);
    }

    #[test]
    fn incremental_xref_stream() {
        incremental_case(true);
    }

    #[test]
    fn annots_written_and_read_back() {
        use crate::annot::{AnnotKind, Box2, Pt};
        let dir = tempdir();
        let a_path = make_pdf(&dir, "a.pdf", "A", 2, &[]);
        let original = std::fs::read(&a_path).unwrap();
        let mut model = Document::new(
            {
                let mut i = info(&a_path, 2);
                i.bytes = original.len();
                i
            },
            &[],
            Box::new(FixedCount(10)),
        );
        let p0 = model.page(0).unwrap().id;
        let mk = |kind: AnnotKind| Annot {
            id: crate::annot::AnnotId(0),
            kind,
            color: [255, 200, 0],
            opacity: 0.5,
            stroke: 1.5,
            author: "greg".into(),
            contents: "note".into(),
            modified: "D:20261011000000".into(),
        };
        model
            .apply(
                Edit::AnnotAdd {
                    page: p0,
                    annot: mk(AnnotKind::Highlight {
                        rects: vec![Box2 {
                            x: 72.0,
                            y: 80.0,
                            w: 200.0,
                            h: 14.0,
                        }],
                    }),
                },
                "hl",
            )
            .unwrap();
        model
            .apply(
                Edit::AnnotAdd {
                    page: p0,
                    annot: mk(AnnotKind::Note {
                        pos: Pt { x: 300.0, y: 80.0 },
                    }),
                },
                "note",
            )
            .unwrap();
        model
            .apply(
                Edit::AnnotAdd {
                    page: p0,
                    annot: mk(AnnotKind::Stamp {
                        rect: Box2 {
                            x: 400.0,
                            y: 80.0,
                            w: 24.0,
                            h: 24.0,
                        },
                        glyph: "✓".into(),
                    }),
                },
                "tick",
            )
            .unwrap();

        let bytes = crate::incremental::incremental(&model.snapshot()).unwrap();
        assert!(bytes.starts_with(&original));
        let out = dir.join("inc.pdf");
        std::fs::write(&out, &bytes).unwrap();
        let back = crate::file::read_sheaf_annots(&out).unwrap();
        assert_eq!(back.len(), 1);
        let want: Vec<Annot> = model
            .state()
            .annots_on(p0)
            .unwrap()
            .iter()
            .cloned()
            .collect();
        assert_eq!(back[&0], want);
        let pdf = Pdf::load_mem(&bytes).unwrap();
        let page0 = pdf.get_pages()[&1];
        let annots = pdf
            .get_dictionary(page0)
            .unwrap()
            .get(b"Annots")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(annots.len(), 3);

        // Reopen, delete one, save again: no duplicates.
        let out2 = dir.join("inc2.pdf");
        std::fs::copy(&out, &out2).unwrap();
        let mut model2 = Document::new(
            {
                let mut i = info(&out2, 2);
                i.bytes = bytes.len();
                i
            },
            &[],
            Box::new(FixedCount(10)),
        );
        model2.set_initial_annots(back).unwrap();
        let p0b = model2.page(0).unwrap().id;
        let first = model2.state().annots_on(p0b).unwrap()[0].clone();
        model2
            .apply(
                Edit::AnnotDelete {
                    page: p0b,
                    id: first.id,
                },
                "rm",
            )
            .unwrap();
        let bytes2 = crate::incremental::incremental(&model2.snapshot()).unwrap();
        let pdf = Pdf::load_mem(&bytes2).unwrap();
        let page0 = pdf.get_pages()[&1];
        let annots = pdf
            .get_dictionary(page0)
            .unwrap()
            .get(b"Annots")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(annots.len(), 2);

        let rw = rewrite(&model.snapshot()).unwrap();
        let out3 = dir.join("rw.pdf");
        std::fs::write(&out3, &rw).unwrap();
        assert_eq!(crate::file::read_sheaf_annots(&out3).unwrap()[&0], want);
    }

    /// Connectors come out as Link annotations whose /Dest is the partner's
    /// page, in both save strategies.
    #[test]
    fn connectors_link_to_partner_page() {
        use crate::annot::{AnnotKind, Box2};
        let dir = tempdir();
        let a_path = make_pdf(&dir, "c.pdf", "C", 3, &[]);
        let original = std::fs::read(&a_path).unwrap();
        let mut model = Document::new(
            {
                let mut i = info(&a_path, 3);
                i.bytes = original.len();
                i
            },
            &[],
            Box::new(FixedCount(10)),
        );
        let (p0, p2) = (model.page(0).unwrap().id, model.page(2).unwrap().id);
        let mk = || Annot {
            id: crate::annot::AnnotId(0),
            kind: AnnotKind::Connector {
                rect: Box2 {
                    x: 100.0,
                    y: 100.0,
                    w: 22.0,
                    h: 22.0,
                },
                label: "1".into(),
                partner: None,
            },
            color: [220, 40, 40],
            opacity: 1.0,
            stroke: 1.0,
            author: "greg".into(),
            contents: String::new(),
            modified: "D:20261011000000".into(),
        };
        model
            .apply(
                Edit::ConnectorAdd {
                    a: (p0, mk()),
                    b: (p2, mk()),
                },
                "c",
            )
            .unwrap();

        let check = |bytes: &[u8]| {
            let pdf = Pdf::load_mem(bytes).unwrap();
            let pages = pdf.get_pages();
            let link_on = |n: u32| -> Dictionary {
                let page = pdf.get_dictionary(pages[&n]).unwrap();
                let annots = pdf
                    .dereference(page.get(b"Annots").unwrap())
                    .unwrap()
                    .1
                    .as_array()
                    .unwrap()
                    .clone();
                assert_eq!(annots.len(), 1);
                pdf.get_dictionary(annots[0].as_reference().unwrap())
                    .unwrap()
                    .clone()
            };
            let a = link_on(1);
            let b = link_on(3);
            assert_eq!(a.get(b"Subtype").unwrap().as_name().unwrap(), b"Link");
            let dest_page = |d: &Dictionary| {
                d.get(b"Dest").unwrap().as_array().unwrap()[0]
                    .as_reference()
                    .unwrap()
            };
            assert_eq!(dest_page(&a), pages[&3]);
            assert_eq!(dest_page(&b), pages[&1]);
            // Round trip keeps the pairing.
            let back = crate::file::read_sheaf_annots_mem(bytes).unwrap();
            assert_eq!(back[&0][0].partner(), Some(back[&2][0].id));
        };
        check(&crate::incremental::incremental(&model.snapshot()).unwrap());
        check(&rewrite(&model.snapshot()).unwrap());
    }

    /// A clean copy keeps pages, bookmarks, links and form fields, and drops
    /// every markup annotation — ours and third-party — along with their
    /// now-unreferenced objects.
    #[test]
    fn clean_copy_strips_markup_only() {
        use crate::annot::{AnnotKind, Box2};
        let dir = tempdir();
        let a_path = make_pdf(&dir, "m.pdf", "M", 2, &[("Top", 0, 0), ("Second", 1, 0)]);
        // Plant third-party annotations on page 1: a note with its popup, a
        // link and a form field.
        {
            let mut pdf = Pdf::load(&a_path).unwrap();
            let page0 = pdf.get_pages()[&1];
            let rect = |a: f32, b: f32, c: f32, d: f32| {
                Object::Array(vec![a.into(), b.into(), c.into(), d.into()])
            };
            let note = pdf.new_object_id();
            let popup = pdf.add_object(dictionary! {
                "Type" => "Annot", "Subtype" => "Popup", "Rect" => rect(0.0, 0.0, 100.0, 100.0), "Parent" => note,
            });
            pdf.objects.insert(
                note,
                Object::Dictionary(dictionary! {
                    "Type" => "Annot", "Subtype" => "Text", "Rect" => rect(10.0, 700.0, 30.0, 720.0),
                    "Contents" => Object::String(b"reviewer note".to_vec(), StringFormat::Literal),
                    "Popup" => popup,
                }),
            );
            let link = pdf.add_object(dictionary! {
                "Type" => "Annot", "Subtype" => "Link", "Rect" => rect(0.0, 0.0, 50.0, 20.0),
                "Dest" => Object::Array(vec![page0.into(), Object::Name(b"Fit".to_vec())]),
            });
            let field = pdf.add_object(dictionary! {
                "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx", "T" => Object::String(b"name".to_vec(), StringFormat::Literal),
                "Rect" => rect(100.0, 100.0, 300.0, 120.0),
            });
            pdf.get_dictionary_mut(page0).unwrap().set(
                "Annots",
                Object::Array(vec![note.into(), popup.into(), link.into(), field.into()]),
            );
            pdf.save(&a_path).unwrap();
        }
        let original = std::fs::read(&a_path).unwrap();
        let mut model = Document::new(
            {
                let mut i = info(&a_path, 2);
                i.bytes = original.len();
                i
            },
            &[
                FlatOutlineItem {
                    title: "Top".into(),
                    page: Some(0),
                    depth: 0,
                },
                FlatOutlineItem {
                    title: "Second".into(),
                    page: Some(1),
                    depth: 0,
                },
            ],
            Box::new(FixedCount(10)),
        );
        let p0 = model.page(0).unwrap().id;
        model
            .apply(
                Edit::AnnotAdd {
                    page: p0,
                    annot: Annot {
                        id: crate::annot::AnnotId(0),
                        kind: AnnotKind::Stamp {
                            rect: Box2 {
                                x: 400.0,
                                y: 80.0,
                                w: 24.0,
                                h: 24.0,
                            },
                            glyph: "✓".into(),
                        },
                        color: [0, 150, 0],
                        opacity: 1.0,
                        stroke: 1.0,
                        author: "greg".into(),
                        contents: String::new(),
                        modified: "D:20261011000000".into(),
                    },
                },
                "tick",
            )
            .unwrap();
        // A connector pair: links in the file, but ours, so they go too.
        let p1 = model.page(1).unwrap().id;
        let conn = || Annot {
            id: crate::annot::AnnotId(0),
            kind: AnnotKind::Connector {
                rect: Box2 {
                    x: 50.0,
                    y: 50.0,
                    w: 22.0,
                    h: 22.0,
                },
                label: "1".into(),
                partner: None,
            },
            color: [220, 40, 40],
            opacity: 1.0,
            stroke: 1.0,
            author: "greg".into(),
            contents: String::new(),
            modified: "D:20261011000000".into(),
        };
        model
            .apply(
                Edit::ConnectorAdd {
                    a: (p0, conn()),
                    b: (p1, conn()),
                },
                "conn",
            )
            .unwrap();
        // Also rotate page 2 so the copy reflects edits, not just the file.
        model
            .apply(
                Edit::RotatePages {
                    ids: vec![p1],
                    by: Rotation::R90,
                },
                "rot",
            )
            .unwrap();

        let out = dir.join("clean.pdf");
        export_clean_copy(&model.snapshot(), &out).unwrap();
        let pdf = Pdf::load(&out).unwrap();
        let pages = pdf.get_pages();
        assert_eq!(pages.len(), 2);
        let annots = pdf
            .dereference(
                pdf.get_dictionary(pages[&1])
                    .unwrap()
                    .get(b"Annots")
                    .unwrap(),
            )
            .unwrap()
            .1
            .as_array()
            .unwrap()
            .clone();
        let mut subtypes: Vec<String> = annots
            .iter()
            .map(|a| {
                String::from_utf8_lossy(
                    pdf.get_dictionary(a.as_reference().unwrap())
                        .unwrap()
                        .get(b"Subtype")
                        .unwrap()
                        .as_name()
                        .unwrap(),
                )
                .into_owned()
            })
            .collect();
        subtypes.sort();
        assert_eq!(subtypes, ["Link", "Widget"]);
        assert!(!pdf.get_dictionary(pages[&2]).unwrap().has(b"Annots"));
        assert_eq!(
            pdf.get_dictionary(pages[&2])
                .unwrap()
                .get(b"Rotate")
                .unwrap()
                .as_i64()
                .unwrap(),
            90
        );
        // Nothing markup-shaped survives anywhere in the file.
        for obj in pdf.objects.values() {
            if let Ok(d) = obj.as_dict()
                && let Ok(s) = d.get(b"Subtype").and_then(|s| s.as_name())
            {
                assert!(
                    !matches!(s, b"Text" | b"Popup" | b"Stamp"),
                    "leftover {}",
                    String::from_utf8_lossy(s)
                );
            }
            if let Ok(d) = obj.as_dict() {
                assert!(!d.has(b"SheafAnnot"));
            }
        }
        assert!(crate::file::read_sheaf_annots(&out).unwrap().is_empty());
        // Bookmarks kept.
        assert!(
            pdf.get_dictionary(pdf.trailer.get(b"Root").unwrap().as_reference().unwrap())
                .unwrap()
                .has(b"Outlines")
        );
        // The open file itself can't be the target.
        assert!(export_clean_copy(&model.snapshot(), &a_path).is_err());
        assert_eq!(std::fs::read(&a_path).unwrap(), original);
    }

    /// An encrypted original opens with its password and saves as a plain
    /// file with the edits applied.
    #[test]
    fn encrypted_source_saves_decrypted() {
        use lopdf::{EncryptionState, EncryptionVersion, Permissions};
        let dir = tempdir();
        let plain = make_pdf(&dir, "plain.pdf", "E", 2, &[("Top", 0, 0)]);
        let enc = dir.join("enc.pdf");
        {
            let mut pdf = Pdf::load(&plain).unwrap();
            // The standard security handler keys off the file /ID.
            let id = Object::String(b"0123456789abcdef".to_vec(), StringFormat::Hexadecimal);
            pdf.trailer.set("ID", Object::Array(vec![id.clone(), id]));
            let state = EncryptionState::try_from(EncryptionVersion::V2 {
                document: &pdf,
                owner_password: "owner",
                user_password: "secret",
                key_length: 128,
                permissions: Permissions::default(),
            })
            .unwrap();
            pdf.encrypt(&state).unwrap();
            pdf.save(&enc).unwrap();
        }
        assert!(Pdf::load(&enc).unwrap().is_encrypted());
        assert!(crate::file::load_pdf(&enc, Some("wrong")).is_err());
        assert!(
            !crate::file::load_pdf(&enc, Some("secret"))
                .unwrap()
                .is_encrypted()
        );

        let mut model = Document::new(
            {
                let mut i = info(&enc, 2);
                i.password = Some("secret".into());
                i
            },
            &[FlatOutlineItem {
                title: "Top".into(),
                page: Some(0),
                depth: 0,
            }],
            Box::new(FixedCount(10)),
        );
        let p1 = model.page(1).unwrap().id;
        model
            .apply(
                Edit::RotatePages {
                    ids: vec![p1],
                    by: Rotation::R90,
                },
                "rot",
            )
            .unwrap();
        let out = dir.join("saved.pdf");
        assert_eq!(
            save_to_path(&model.snapshot(), &out).unwrap(),
            Strategy::Rewrite
        );
        let saved = Pdf::load(&out).unwrap();
        assert!(!saved.is_encrypted());
        assert!(!saved.trailer.has(b"Encrypt"));
        let pages = saved.get_pages();
        assert_eq!(pages.len(), 2);
        assert_eq!(
            saved
                .get_dictionary(pages[&2])
                .unwrap()
                .get(b"Rotate")
                .unwrap()
                .as_i64()
                .unwrap(),
            90
        );
        // The content survived decryption: the page's text is readable.
        let content = saved.get_page_content(pages[&1]);
        assert!(
            String::from_utf8_lossy(&content).contains("E"),
            "content stream not decrypted"
        );
        // Outline came through too.
        let items = read_outline(&std::fs::read(&out).unwrap());
        assert_eq!(items[0].1, "Top");
    }

    fn tempdir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "sheaf-save-{}-{}",
            std::process::id(),
            rand_suffix()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn rand_suffix() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        t ^ N.fetch_add(1, Ordering::Relaxed)
    }
}
