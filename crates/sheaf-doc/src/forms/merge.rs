//! Forms from other files, and copies of a form, in a saved file (M10
//! slice 4, proposal 5).
//!
//! Pages from other files bring their widgets, and through them their
//! fields; Duplicate Pages copies a page's widgets. The document model
//! says which fields each page's widgets are in (`Field::copy`,
//! `Field::under`). Both writers copy a page's annotations by the copy of
//! the form they're in ([`page_copy`]), then [`merge`] puts those fields in
//! the saved file's form: their roots in `/Fields` (under a new parent
//! field when their names clashed), with what they need from their own
//! form (`/DR` fonts, `/DA` and `/Q`, `/CO`, `NeedAppearances`, document
//! scripts). Only widgets on the pages written come along. An incoming
//! form's XFA copy, usage rights and `/SigFlags` describe its own file,
//! and stay there.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::Result;
use lopdf::{Dictionary, Document as Pdf, Object, ObjectId, dictionary};

use crate::forms::Field;
use crate::model::{DocState, PageId, Snapshot, SourceId};
use crate::save::{copy_object, remap};
use crate::source::Limits;

/// A copy of a source's form: its source, and `Field::copy`.
pub(crate) type FormCopy = (SourceId, u32);

/// Objects copied into the saved file, by the copy of a form they're for:
/// each source object and its copy.
pub(crate) type CopyMaps = HashMap<FormCopy, HashMap<ObjectId, ObjectId>>;

/// The most document scripts read from one file, as `forms::read` counts
/// them.
const MAX_SCRIPTS: usize = Limits::DEFAULT.forms.scripts;

/// The copy of a form `page`'s widgets are in, when they're in one.
pub(crate) fn page_copy(state: &DocState, page: PageId) -> Option<FormCopy> {
    let w = state.widgets_on(page)?.front()?;
    let f = state.fields.get(&w.field)?;
    Some((f.source, f.copy))
}

/// `page`'s annotations in `src`, copied into `dst` through `map`: what a
/// page showing a copy of a form has, so its widgets are its own.
pub(crate) fn copy_annots(
    src: &Pdf,
    page: ObjectId,
    dst: &mut Pdf,
    map: &mut HashMap<ObjectId, ObjectId>,
) -> Result<Option<Object>> {
    match src.get_dictionary(page)?.get(b"Annots") {
        Ok(a) => Ok(Some(remap(src, a, dst, map)?)),
        Err(_) => Ok(None),
    }
}

/// The annotations on `pages`, as written.
pub(crate) fn annots_on(
    pdf: &Pdf,
    pages: impl IntoIterator<Item = Dictionary>,
) -> HashSet<ObjectId> {
    let mut out = HashSet::new();
    for page in pages {
        if let Some(list) = page
            .get(b"Annots")
            .ok()
            .and_then(|a| deref(pdf, a))
            .and_then(|a| a.as_array().ok())
        {
            out.extend(list.iter().filter_map(|a| a.as_reference().ok()));
        }
    }
    out
}

/// Put the fields of each copy of a form the document has in `pdf`'s
/// form, but the primary file's own, which are there already. `from`
/// gives each source's file as read (the primary's before the save changed
/// it); `maps`, what was copied from each, by copy; `on_pages`, the
/// annotations on the pages written. Returns the objects it changed that
/// `pdf` had: its catalog and its form.
pub(crate) fn merge<'a>(
    pdf: &mut Pdf,
    model: &Snapshot,
    from: &dyn Fn(SourceId) -> Option<&'a Pdf>,
    maps: &mut CopyMaps,
    on_pages: &HashSet<ObjectId>,
) -> Result<Vec<ObjectId>> {
    let mut copies: BTreeMap<FormCopy, Vec<&Field>> = BTreeMap::new();
    for f in model.state.fields.values() {
        if (f.source, f.copy) != (model.primary, 0) {
            copies.entry((f.source, f.copy)).or_default().push(f);
        }
    }
    if copies.is_empty() {
        return Ok(Vec::new());
    }
    let catalog_id = pdf.trailer.get(b"Root")?.as_reference()?;
    let form_id = own_form(pdf, catalog_id)?;
    let mut form = pdf.get_dictionary(form_id)?.clone();
    let mut fields: Vec<Object> = list(pdf, form.get(b"Fields").ok());
    let mut co: Vec<Object> = list(pdf, form.get(b"CO").ok());
    let mut dr = dict(pdf, form.get(b"DR").ok());
    let da = form
        .get(b"DA")
        .ok()
        .and_then(|d| d.as_str().ok())
        .map(<[u8]>::to_vec);
    let q = form
        .get(b"Q")
        .ok()
        .and_then(|q| q.as_i64().ok())
        .unwrap_or(0);
    let mut need_appearances = flag(&form, b"NeedAppearances");
    // Each source's fonts renamed in `/DR`, by old name.
    let mut renamed: HashMap<SourceId, HashMap<Vec<u8>, Vec<u8>>> = HashMap::new();
    let mut scripts = Vec::new();

    for ((source, copy), these) in copies {
        let Some(src) = from(source) else {
            continue;
        };
        let theirs = src
            .catalog()
            .ok()
            .and_then(|c| c.get(b"AcroForm").ok())
            .and_then(|f| deref(src, f))
            .and_then(|f| f.as_dict().ok())
            .cloned()
            .unwrap_or_default();
        let map = maps.entry((source, copy)).or_default();
        let fonts = match renamed.get(&source) {
            Some(r) => r.clone(),
            None => {
                let r = merge_resources(&mut dr, pdf, src, &theirs, map)?;
                renamed.insert(source, r.clone());
                scripts.extend(document_scripts(src, pdf, map)?);
                r
            }
        };
        // Their roots, copied with what's under them.
        let mut roots = Vec::new();
        let mut seen = HashSet::new();
        for f in &these {
            let root = root_of(src, f.object);
            if seen.insert(root)
                && let Some(id) = copy_object(src, root, pdf, map)?
            {
                roots.push(Object::Reference(id));
            }
        }
        let (left, removed) = crate::kept::prune_fields(pdf, &roots, on_pages.clone());
        if left.is_empty() {
            continue;
        }
        if !fonts.is_empty() {
            for id in map.values() {
                if let Some(Object::Dictionary(d)) = pdf.objects.get_mut(id) {
                    rename_fonts(d, &fonts);
                }
            }
        }
        // Their form's /DA and /Q, where they differ from this one's, go
        // where the fields inherit them: the new parent, or each root.
        let their_da = theirs
            .get(b"DA")
            .ok()
            .and_then(|d| d.as_str().ok())
            .map(|d| rename_in_da(d, &fonts))
            .filter(|d| Some(d) != da.as_ref());
        let their_q = theirs
            .get(b"Q")
            .ok()
            .and_then(|q| q.as_i64().ok())
            .unwrap_or(0);
        let push_down = |d: &mut Dictionary| {
            if let Some(da) = &their_da
                && !d.has(b"DA")
            {
                d.set(
                    "DA",
                    Object::String(da.clone(), lopdf::StringFormat::Literal),
                );
            }
            if their_q != q && !d.has(b"Q") {
                d.set("Q", their_q);
            }
        };
        match &these[0].under {
            Some(under) => {
                let mut parent = dictionary! {
                    "T" => crate::save::text_string(under),
                    "Kids" => left.clone(),
                };
                push_down(&mut parent);
                let parent = pdf.add_object(parent);
                for root in &left {
                    if let Ok(id) = root.as_reference()
                        && let Ok(d) = pdf.get_dictionary_mut(id)
                    {
                        d.set("Parent", parent);
                    }
                }
                fields.push(Object::Reference(parent));
            }
            None => {
                for root in &left {
                    if let Ok(id) = root.as_reference()
                        && let Ok(d) = pdf.get_dictionary_mut(id)
                    {
                        push_down(d);
                    }
                }
                fields.extend(left);
            }
        }
        for c in list(src, theirs.get(b"CO").ok()) {
            if let Some(&id) = c.as_reference().ok().and_then(|r| map.get(&r))
                && !removed.contains(&id)
            {
                co.push(Object::Reference(id));
            }
        }
        need_appearances |= flag(&theirs, b"NeedAppearances");
    }

    form.set("Fields", fields);
    if !co.is_empty() {
        form.set("CO", co);
    }
    if !dr.is_empty() {
        form.set("DR", dr);
    }
    if need_appearances {
        form.set("NeedAppearances", true);
    }
    pdf.objects.insert(form_id, Object::Dictionary(form));
    if !scripts.is_empty() {
        add_document_scripts(pdf, catalog_id, scripts);
    }
    Ok(vec![catalog_id, form_id])
}

/// The catalog's `/AcroForm` as an object of its own: the one it has, or
/// a direct one moved out, or a new empty form.
fn own_form(pdf: &mut Pdf, catalog_id: ObjectId) -> Result<ObjectId> {
    let current = pdf
        .get_dictionary(catalog_id)?
        .get(b"AcroForm")
        .ok()
        .cloned();
    let form = match current {
        Some(Object::Reference(id)) if pdf.get_dictionary(id).is_ok() => return Ok(id),
        Some(Object::Dictionary(d)) => d,
        _ => dictionary! { "Fields" => Vec::<Object>::new() },
    };
    let id = pdf.add_object(form);
    pdf.get_dictionary_mut(catalog_id)?.set("AcroForm", id);
    Ok(id)
}

/// The top of `id`'s `/Parent` chain in `src` (at most 32 up).
fn root_of(src: &Pdf, mut id: ObjectId) -> ObjectId {
    let mut seen = HashSet::from([id]);
    for _ in 0..32 {
        match src
            .get_dictionary(id)
            .ok()
            .and_then(|d| d.get(b"Parent").ok())
            .and_then(|p| p.as_reference().ok())
        {
            Some(up) if src.get_dictionary(up).is_ok() && seen.insert(up) => id = up,
            _ => break,
        }
    }
    id
}

/// Their `/DR` into `dr`: what it lacks is copied in; a font of the same
/// name that isn't the same font comes in under a new name. The fonts
/// renamed, by old name.
fn merge_resources(
    dr: &mut Dictionary,
    pdf: &mut Pdf,
    src: &Pdf,
    theirs: &Dictionary,
    map: &mut HashMap<ObjectId, ObjectId>,
) -> Result<HashMap<Vec<u8>, Vec<u8>>> {
    let mut renamed = HashMap::new();
    let their_dr = dict(src, theirs.get(b"DR").ok());
    for (kind, entries) in their_dr.iter() {
        let Some(entries) = deref(src, entries).and_then(|e| e.as_dict().ok()) else {
            continue;
        };
        let mut ours = dict(pdf, dr.get(kind).ok());
        for (name, value) in entries.iter() {
            match ours.get(name).ok().cloned() {
                None => {
                    let v = remap(src, value, pdf, map)?;
                    ours.set(name.clone(), v);
                }
                Some(mine) if kind == b"Font" && !same_font(pdf, &mine, src, value) => {
                    let new = (2..)
                        .map(|n| {
                            let mut n2 = name.clone();
                            n2.extend_from_slice(format!("_{n}").as_bytes());
                            n2
                        })
                        .find(|n| !ours.has(n) && !entries.has(n))
                        .expect("a free name");
                    let v = remap(src, value, pdf, map)?;
                    ours.set(new.clone(), v);
                    renamed.insert(name.clone(), new);
                }
                Some(_) => {}
            }
        }
        dr.set(kind.clone(), ours);
    }
    Ok(renamed)
}

/// Whether two fonts of a `/DR` are the same: the same `/Subtype`,
/// `/BaseFont` and named `/Encoding` (a font with an encoding of its own
/// isn't taken for another).
fn same_font(pdf: &Pdf, mine: &Object, src: &Pdf, theirs: &Object) -> bool {
    let (Some(a), Some(b)) = (
        deref(pdf, mine).and_then(|o| o.as_dict().ok()),
        deref(src, theirs).and_then(|o| o.as_dict().ok()),
    ) else {
        return false;
    };
    let name = |d: &Dictionary, k: &[u8]| {
        d.get(k)
            .ok()
            .and_then(|v| v.as_name().ok())
            .map(<[u8]>::to_vec)
    };
    let plain = |d: &Dictionary| d.get(b"Encoding").map_or(true, |e| e.as_name().is_ok());
    plain(a)
        && plain(b)
        && [b"Subtype".as_slice(), b"BaseFont", b"Encoding"]
            .iter()
            .all(|k| name(a, k) == name(b, k))
        && name(a, b"BaseFont").is_some()
}

/// `d`'s `/DA` with the fonts in `renamed` renamed.
fn rename_fonts(d: &mut Dictionary, renamed: &HashMap<Vec<u8>, Vec<u8>>) {
    if let Some(da) = d.get(b"DA").ok().and_then(|v| v.as_str().ok()) {
        let new = rename_in_da(da, renamed);
        if new != da {
            d.set("DA", Object::String(new, lopdf::StringFormat::Literal));
        }
    }
}

/// A `/DA` string with each name in `renamed` replaced.
fn rename_in_da(da: &[u8], renamed: &HashMap<Vec<u8>, Vec<u8>>) -> Vec<u8> {
    let delimiter = |b: u8| b.is_ascii_whitespace() || b"()<>[]{}/%".contains(&b);
    let mut out = Vec::with_capacity(da.len());
    let mut i = 0;
    while i < da.len() {
        if da[i] == b'/' {
            let start = i + 1;
            let mut end = start;
            while end < da.len() && !delimiter(da[end]) {
                end += 1;
            }
            out.push(b'/');
            match renamed.get(&da[start..end]) {
                Some(new) => out.extend_from_slice(new),
                None => out.extend_from_slice(&da[start..end]),
            }
            i = end;
        } else {
            out.push(da[i]);
            i += 1;
        }
    }
    out
}

/// The document scripts in `src` (`/Names /JavaScript`), copied into
/// `pdf`, by name.
fn document_scripts(
    src: &Pdf,
    pdf: &mut Pdf,
    map: &mut HashMap<ObjectId, ObjectId>,
) -> Result<Vec<(Vec<u8>, Object)>> {
    let mut found = Vec::new();
    if let Some(tree) = src
        .catalog()
        .ok()
        .and_then(|c| c.get(b"Names").ok())
        .and_then(|n| deref(src, n))
        .and_then(|n| n.as_dict().ok())
        .and_then(|n| n.get(b"JavaScript").ok())
    {
        name_tree(src, tree, &mut found, &mut HashSet::new(), 0);
    }
    found
        .into_iter()
        .map(|(k, v)| Ok((k, remap(src, &v, pdf, map)?)))
        .collect()
}

/// `scripts` beside the document scripts `pdf` has. A name it has already
/// is renamed ("Init 2"), but for Adobe's version checks (`!ADBE::`),
/// which are the same everywhere and never run in sheaf.
fn add_document_scripts(pdf: &mut Pdf, catalog_id: ObjectId, scripts: Vec<(Vec<u8>, Object)>) {
    let Ok(catalog) = pdf.get_dictionary(catalog_id) else {
        return;
    };
    let mut names = dict(pdf, catalog.get(b"Names").ok());
    let mut all = Vec::new();
    if let Ok(tree) = names.get(b"JavaScript") {
        name_tree(pdf, tree, &mut all, &mut HashSet::new(), 0);
    }
    let mut taken: HashSet<Vec<u8>> = all.iter().map(|(k, _)| k.clone()).collect();
    for (key, script) in scripts {
        let key = if !taken.contains(&key) {
            key
        } else if key.starts_with(b"!ADBE::") {
            continue;
        } else {
            (2..)
                .map(|n| {
                    let mut k = key.clone();
                    k.extend_from_slice(format!(" {n}").as_bytes());
                    k
                })
                .find(|k| !taken.contains(k))
                .expect("a free name")
        };
        taken.insert(key.clone());
        all.push((key, script));
    }
    all.sort_by(|a, b| a.0.cmp(&b.0));
    let pairs: Vec<Object> = all
        .into_iter()
        .flat_map(|(k, v)| [Object::String(k, lopdf::StringFormat::Literal), v])
        .collect();
    let tree = pdf.add_object(dictionary! { "Names" => pairs });
    names.set("JavaScript", tree);
    if let Ok(c) = pdf.get_dictionary_mut(catalog_id) {
        c.set("Names", names);
    }
}

/// A name tree's entries, at most [`MAX_SCRIPTS`], each node read once.
fn name_tree(
    pdf: &Pdf,
    node: &Object,
    out: &mut Vec<(Vec<u8>, Object)>,
    seen: &mut HashSet<ObjectId>,
    depth: u32,
) {
    if depth > 32 || out.len() >= MAX_SCRIPTS {
        return;
    }
    if let Ok(id) = node.as_reference()
        && !seen.insert(id)
    {
        return;
    }
    let Some(d) = deref(pdf, node).and_then(|n| n.as_dict().ok()) else {
        return;
    };
    if let Some(pairs) = d
        .get(b"Names")
        .ok()
        .and_then(|n| deref(pdf, n))
        .and_then(|n| n.as_array().ok())
    {
        for pair in pairs.chunks(2) {
            if let [Object::String(k, _), v] = pair
                && out.len() < MAX_SCRIPTS
            {
                out.push((k.clone(), v.clone()));
            }
        }
    }
    if let Some(kids) = d
        .get(b"Kids")
        .ok()
        .and_then(|k| deref(pdf, k))
        .and_then(|k| k.as_array().ok())
    {
        for kid in kids {
            name_tree(pdf, kid, out, seen, depth + 1);
        }
    }
}

fn deref<'a>(pdf: &'a Pdf, o: &'a Object) -> Option<&'a Object> {
    match o {
        Object::Reference(id) => pdf.get_object(*id).ok(),
        o => Some(o),
    }
}

/// An array (or a reference to one), copied out; empty when it isn't one.
fn list(pdf: &Pdf, o: Option<&Object>) -> Vec<Object> {
    o.and_then(|o| deref(pdf, o))
        .and_then(|o| o.as_array().ok())
        .cloned()
        .unwrap_or_default()
}

/// A dictionary (or a reference to one), copied out; empty when it isn't
/// one.
fn dict(pdf: &Pdf, o: Option<&Object>) -> Dictionary {
    o.and_then(|o| deref(pdf, o))
        .and_then(|o| o.as_dict().ok())
        .cloned()
        .unwrap_or_default()
}

fn flag(d: &Dictionary, key: &[u8]) -> bool {
    d.get(key)
        .ok()
        .and_then(|v| v.as_bool().ok())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn da_fonts_are_renamed_by_name() {
        let renamed = HashMap::from([(b"Helv".to_vec(), b"Helv_2".to_vec())]);
        assert_eq!(
            rename_in_da(b"/Helv 0 Tf 0 g", &renamed),
            b"/Helv_2 0 Tf 0 g"
        );
        // Only whole names.
        assert_eq!(rename_in_da(b"/HelvB 9 Tf", &renamed), b"/HelvB 9 Tf");
        assert_eq!(
            rename_in_da(b"0 g /Helv 12 Tf", &renamed),
            b"0 g /Helv_2 12 Tf"
        );
    }
}
