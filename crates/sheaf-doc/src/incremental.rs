//! Incremental save: append only what changed to the original file, with a
//! new cross-reference section chained to the previous one via `/Prev`.
//! The original bytes are preserved verbatim (what Acrobat does by default,
//! and what auditors like). Only possible when saving a document back to
//! its own primary file.

use std::collections::BTreeMap;
use std::io::Write;

use anyhow::{Context, Result, bail};
use lopdf::xref::{XrefEntry, XrefType};
use lopdf::{Dictionary, Document as Pdf, Object, ObjectId, Stream, StringFormat, dictionary};
use sheaf_core::Rotation;

use crate::model::Snapshot;
use crate::save::{
    LoadedSource, copy_page_dict, is_identity, link_connectors, strip_sheaf_annots, write_outline,
    write_page_annots,
};
use crate::text_layer;

/// Produce the full new file (original bytes + update section). Refuses
/// to replace a page's OCR text already in the file, which would stay in
/// the earlier revision (M9 bug 14). Whether anything else was removed is
/// `save::save_to_path`'s to check (`Snapshot::removes_content`).
pub fn incremental(snap: &Snapshot) -> Result<Vec<u8>> {
    Ok(build(snap, Mode::Save)?.1)
}

/// The update a signed file gets when the reader would rather append than
/// break its signatures (M10 proposal 7): what it takes out (a removed
/// page, a changed form's XFA copy and usage rights, a page's old OCR
/// text) stays in the earlier revision.
pub fn append(snap: &Snapshot) -> Result<Vec<u8>> {
    Ok(build(snap, Mode::Append)?.1)
}

/// Just the update section, with the length of the original it appends
/// to. Empty when there's nothing to write. Used for crash recovery.
pub fn incremental_section(snap: &Snapshot) -> Result<(usize, Vec<u8>)> {
    let (original_len, mut full) = build(snap, Mode::Sidecar)?;
    let section = full.split_off(original_len);
    Ok((original_len, section))
}

/// What an update is for, and so what it refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// A save, which a full one replaces when it refuses: what an update
    /// can't take out of the file (a page's OCR text replaced, a changed
    /// form's XFA copy and usage rights) or point back at (cross-references
    /// lopdf rebuilt).
    Save,
    /// An append chosen over breaking a file's signatures: what it takes
    /// out stays in the earlier revision. Damaged cross-references still
    /// refuse.
    Append,
    /// A recovery sidecar, which is never the file itself.
    Sidecar,
}

/// The original bytes with the update appended, and where the original ends.
fn build(snap: &Snapshot, mode: Mode) -> Result<(usize, Vec<u8>)> {
    let primary_id = snap.primary;
    let info = snap.source(primary_id).context("primary source missing")?;
    let original = info.src.bytes();
    let original_len = original.len();
    let mut pdf =
        Pdf::load_mem(original).with_context(|| format!("parsing {}", info.name.display()))?;
    if snap.header_footer_dirty {
        // Drawing it touches every page's content: a rewrite does that.
        bail!("a new header/footer is written by a full save");
    }
    if pdf.encryption_state.is_some() || pdf.trailer.has(b"Encrypt") {
        bail!("incremental save of encrypted files is not supported yet");
    }
    let prev_startxref = pdf.xref_start;
    if mode != Mode::Sidecar && prev_startxref == 0 {
        // lopdf rebuilt a damaged file's cross-references by scanning it:
        // there's no section for /Prev to point at. A rewrite writes new ones.
        bail!("the file's cross-references are damaged; a full save writes new ones");
    }
    let orig_max_id = pdf.max_id;
    let xref_type = pdf.reference_table.cross_reference_type;

    let primary = LoadedSource::from_pdf(&pdf)?;
    let origins = crate::save::origins(snap);
    let mut others: std::collections::HashMap<_, LoadedSource> = std::collections::HashMap::new();
    for (&sid, sinfo) in &snap.sources {
        if sid != primary_id && crate::save::needed(snap, &origins, sid) {
            let other = crate::file::load_source(sinfo)?;
            others.insert(sid, LoadedSource::from_pdf_owned(other)?);
        }
    }

    // Objects to (re)write, by id. New objects are allocated through `pdf`
    // so ids never collide; everything above the original max is new.
    let mut changed: BTreeMap<ObjectId, Object> = BTreeMap::new();
    let catalog_id = pdf.trailer.get(b"Root")?.as_reference()?;
    let mut catalog = pdf.get_dictionary(catalog_id)?.clone();
    let mut catalog_changed = false;

    let tree_changed = !is_identity(snap, &primary);
    let mut page_obj: std::collections::HashMap<crate::model::PageId, ObjectId> =
        std::collections::HashMap::new();
    let mut copy_maps: crate::forms::merge::CopyMaps = std::collections::HashMap::new();
    // A copy of a primary page's form is copied from the file as it was.
    let mut pristine = None;

    let mut connectors = Vec::new();
    // OCR's font and its isolating `q`, and what the marks draw with and
    // embed, once a save; the original's where it has them.
    let mut cx = crate::annot::WriteCx::new(snap.assets.clone()).owning(info.src.id());
    if tree_changed {
        let pages_id = pdf.new_object_id();
        let mut kids = Vec::with_capacity(snap.state.pages.len());
        let mut used = std::collections::HashSet::new();
        // Pages from other files get their ids first, so links among them
        // follow and links to their other pages don't copy those (bug 14),
        // and so does each page showing a copy of a form (M10 bug 4).
        let own_objects = crate::save::reserve_page_objects(
            snap,
            &origins,
            &|s, i| {
                let from = if s == primary_id {
                    &primary
                } else {
                    others.get(&s)?
                };
                from.pages.get(i as usize).copied()
            },
            &mut pdf,
            &mut copy_maps,
        );
        if own_objects
            .keys()
            .any(|p| crate::save::copy_of(snap, &origins, *p).0 == primary_id)
        {
            pristine = Some(pdf.clone());
        }
        for pref in &snap.state.pages {
            // A derived copy's page is written as the page it was made
            // from: its object, annotations and form fields, with the
            // copy's content (M10 bug 1).
            let origin = origins.get(&pref.id).copied();
            let source = origin.map_or(pref.source, |(s, _)| s);
            let (src_page, inherited, mut dict) = if let Some((_, index)) = origin {
                let shown = others.get(&pref.source).context("unknown source")?;
                let id = *shown
                    .pages
                    .get(pref.index as usize)
                    .context("page index out of range")?;
                let map = copy_maps.entry((pref.source, 0)).or_default();
                let mut dict =
                    crate::save::copy_derived_page(shown.pdf.as_ref().unwrap(), id, &mut pdf, map)?;
                let own = own_objects.contains_key(&pref.id);
                let (src_page, annots) = if source == primary_id {
                    let page = *primary
                        .pages
                        .get(index as usize)
                        .context("page index out of range")?;
                    (page, pdf.get_dictionary(page)?.get(b"Annots").ok().cloned())
                } else {
                    let from = others.get(&source).context("unknown source")?;
                    let page = *from
                        .pages
                        .get(index as usize)
                        .context("page index out of range")?;
                    let from = from.pdf.as_ref().unwrap();
                    let map = copy_maps.entry((source, 0)).or_default();
                    let annots = match from.get_dictionary(page)?.get(b"Annots") {
                        // A copy of a form gets its own, below.
                        Ok(a) if !own => Some(crate::save::remap(from, a, &mut pdf, map)?),
                        _ => None,
                    };
                    (page, annots)
                };
                if let Some(a) = annots {
                    dict.set("Annots", a);
                }
                if source != primary_id {
                    crate::kept::drop_dead_links(&pdf, &mut dict);
                }
                (
                    src_page,
                    shown.inherited.get(&id).cloned().unwrap_or_default(),
                    dict,
                )
            } else if pref.source == primary_id {
                let id = *primary
                    .pages
                    .get(pref.index as usize)
                    .context("page index out of range")?;
                (
                    id,
                    primary.inherited.get(&id).cloned().unwrap_or_default(),
                    pdf.get_dictionary(id)?.clone(),
                )
            } else {
                let src = others.get(&pref.source).context("unknown source")?;
                let id = *src
                    .pages
                    .get(pref.index as usize)
                    .context("page index out of range")?;
                let map = copy_maps.entry((pref.source, 0)).or_default();
                let src_pdf = src.pdf.as_ref().unwrap();
                let mut dict = if own_objects.contains_key(&pref.id) {
                    crate::save::copy_derived_page(src_pdf, id, &mut pdf, map)?
                } else {
                    copy_page_dict(src_pdf, id, &mut pdf, map)?
                };
                crate::kept::drop_dead_links(&pdf, &mut dict);
                (
                    id,
                    src.inherited.get(&id).cloned().unwrap_or_default(),
                    dict,
                )
            };
            if own_objects.contains_key(&pref.id) {
                // A copy of a form: its annotations are its own (M10 bug 4).
                let form = crate::save::copy_of(snap, &origins, pref.id);
                let from = if form.0 == primary_id {
                    pristine.as_ref()
                } else {
                    others.get(&form.0).and_then(|o| o.pdf.as_ref())
                }
                .context("a copy's source isn't loaded")?;
                let map = copy_maps.entry(form).or_default();
                match crate::forms::merge::copy_annots(from, src_page, &mut pdf, map)? {
                    Some(a) => dict.set("Annots", a),
                    None => {
                        dict.remove(b"Annots");
                    }
                }
            }
            for (k, v) in &inherited {
                if !dict.has(k) {
                    dict.set(k.clone(), v.clone());
                }
            }
            match (
                snap.state.annots.get(&pref.id),
                snap.state.removed_comments.get(&pref.id),
            ) {
                (None, None) => strip_sheaf_annots(&pdf, &mut dict),
                (list, removed) => {
                    let marks = list.into_iter().flatten();
                    let index = origin.map_or(pref.index, |(_, i)| i);
                    let renamed = snap.renamed_on(source, index);
                    let w =
                        write_page_annots(&mut pdf, &mut dict, marks, removed, renamed, &mut cx);
                    connectors.extend(w.connectors);
                    for id in w.touched {
                        changed.insert(id, pdf.get_object(id)?.clone());
                    }
                }
            }
            if let Some(layer) = snap.state.text_layers.get(&pref.id) {
                if mode == Mode::Save && text_layer::has_layer(&pdf, &dict) {
                    bail!(
                        "a page's OCR text is replaced: a full save takes the old out of the file"
                    );
                }
                text_layer::install(&mut pdf, &mut dict, layer, &mut cx.res);
            }
            // The new tree's root has no /Rotate for it to inherit.
            if pref.rotation == Rotation::R0 {
                dict.remove(b"Rotate");
            } else {
                dict.set("Rotate", pref.rotation.degrees());
            }
            dict.set("Parent", pages_id);
            dict.set("Type", Object::Name(b"Page".to_vec()));
            let first = !own_objects.contains_key(&pref.id) && used.insert((source, src_page));
            let id = if let Some(&id) = own_objects.get(&pref.id) {
                id
            } else if source == primary_id && first {
                src_page
            } else if source != primary_id && first {
                // Its id was reserved above; a second use is a copy.
                copy_maps[&(source, 0)][&src_page]
            } else {
                pdf.new_object_id()
            };
            changed.insert(id, Object::Dictionary(dict));
            kids.push(Object::Reference(id));
            page_obj.insert(pref.id, id);
        }
        changed.insert(
            pages_id,
            Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => snap.state.pages.len() as i64 }),
        );
        catalog.set("Pages", pages_id);
        catalog.remove(b"PageLabels");
        catalog_changed = true;
    } else {
        for pref in &snap.state.pages {
            let id = primary.pages[pref.index as usize];
            page_obj.insert(pref.id, id);
            let layer = snap.state.text_layers.get(&pref.id);
            let turned = snap.render_delta(pref) != Rotation::R0;
            if turned || snap.annots_dirty.contains(&pref.id) || layer.is_some() {
                let mut dict = pdf.get_dictionary(id)?.clone();
                // The rewritten dict stands alone: pull down what it
                // inherited (a layer inlines Resources, which would
                // otherwise hide the inherited ones).
                if let Some(attrs) = primary.inherited.get(&id) {
                    for (k, v) in attrs {
                        if !dict.has(k) {
                            dict.set(k.clone(), v.clone());
                        }
                    }
                }
                if let Some(layer) = layer {
                    if mode == Mode::Save && text_layer::has_layer(&pdf, &dict) {
                        bail!(
                            "a page's OCR text is replaced: a full save takes the old out of the file"
                        );
                    }
                    text_layer::install(&mut pdf, &mut dict, layer, &mut cx.res);
                }
                if snap.annots_dirty.contains(&pref.id) {
                    match (
                        snap.state.annots.get(&pref.id),
                        snap.state.removed_comments.get(&pref.id),
                    ) {
                        (None, None) => strip_sheaf_annots(&pdf, &mut dict),
                        (list, removed) => {
                            let marks = list.into_iter().flatten();
                            let w = write_page_annots(
                                &mut pdf,
                                &mut dict,
                                marks,
                                removed,
                                snap.renamed_on(primary_id, pref.index),
                                &mut cx,
                            );
                            connectors.extend(w.connectors);
                            for id in w.touched {
                                changed.insert(id, pdf.get_object(id)?.clone());
                            }
                        }
                    }
                }
                // A turned page says its rotation, 0 too: it stays under
                // its parents, any of which may have a /Rotate. Any other
                // keeps the one it has, its own or the one pulled down.
                if turned {
                    dict.set("Rotate", pref.rotation.degrees());
                }
                changed.insert(id, Object::Dictionary(dict));
            }
        }
    }

    // Only a page-tree rewrite has changed the catalog so far.
    let reordered = catalog_changed;
    let labels_now = snap.state.label_ranges().filter(|r| !r.is_empty());
    if snap.labels_dirty || (reordered && labels_now.is_some()) {
        match labels_now {
            Some(r) => {
                let id = pdf.add_object(crate::labels::number_tree(&r));
                catalog.set("PageLabels", id);
            }
            None => {
                catalog.remove(b"PageLabels");
            }
        }
        catalog_changed = true;
    }
    if snap.outline_dirty {
        catalog.remove(b"Outlines");
        if !snap.state.outline.is_empty() {
            let outlines_id = write_outline(&mut pdf, &snap.state.outline, &page_obj);
            catalog.set("Outlines", outlines_id);
        }
        catalog_changed = true;
    }
    // Forms from other files, and copies of forms, join the file's.
    let written: Vec<Dictionary> = page_obj
        .values()
        .filter_map(|id| match changed.get(id) {
            Some(Object::Dictionary(d)) => Some(d.clone()),
            _ => pdf.get_dictionary(*id).ok().cloned(),
        })
        .collect();
    let on_pages = crate::forms::merge::annots_on(&pdf, written);
    pdf.objects
        .insert(catalog_id, Object::Dictionary(catalog.clone()));
    let touched = crate::forms::merge::merge(
        &mut pdf,
        snap,
        &|sid| {
            if sid == primary_id {
                pristine.as_ref()
            } else {
                others.get(&sid).and_then(|o| o.pdf.as_ref())
            }
        },
        &mut copy_maps,
        &on_pages,
    )?;
    let mut touched = touched;
    // A changed form's XFA copy and usage rights describe the file as it
    // was: a save takes them out by a full rewrite, so the earlier revision
    // doesn't keep them; a recovery sidecar takes them out here.
    if snap.form_changed {
        let (xfa, rights) = crate::forms::rules::has(&pdf);
        if xfa || rights {
            if mode == Mode::Save {
                bail!("the form changed: a full save takes out its XFA copy and usage rights");
            }
            touched.extend(crate::forms::rules::take_out(&mut pdf, true).0);
        }
    }
    for id in touched {
        if id == catalog_id {
            catalog = pdf.get_dictionary(catalog_id)?.clone();
            catalog_changed = true;
        } else {
            changed.insert(id, pdf.get_object(id)?.clone());
        }
    }
    if catalog_changed {
        changed.insert(catalog_id, Object::Dictionary(catalog));
    }
    link_connectors(&mut pdf, &connectors, snap, &page_obj);
    // What's new and reachable from what's written is written too: objects
    // a copy brought that nothing keeps (a widget of a page left behind)
    // stay out.
    let mut queue: Vec<ObjectId> = changed.keys().copied().collect();
    let mut reached = std::collections::HashSet::new();
    while let Some(id) = queue.pop() {
        let Some(obj) = changed.get(&id).or_else(|| pdf.objects.get(&id)) else {
            continue;
        };
        refs(obj, &mut |r| {
            if r.0 > orig_max_id && reached.insert(r) {
                queue.push(r);
            }
        });
    }
    for id in reached {
        if let Some(obj) = pdf.objects.get(&id) {
            changed.entry(id).or_insert_with(|| obj.clone());
        }
    }
    if changed.is_empty() {
        return Ok((original_len, original.to_vec()));
    }

    // ----- serialize the update section ---------------------------------------
    let generation = |id: ObjectId, pdf: &Pdf| -> u16 {
        match pdf.reference_table.entries.get(&id.0) {
            Some(XrefEntry::Normal { generation, .. }) => *generation,
            _ => id.1,
        }
    };
    let mut out = original.to_vec();
    if !out.ends_with(b"\n") {
        out.push(b'\n');
    }
    let mut offsets: BTreeMap<u32, (usize, u16)> = BTreeMap::new();
    for (id, obj) in &changed {
        let generation = generation(*id, &pdf);
        offsets.insert(id.0, (out.len(), generation));
        writeln!(out, "{} {} obj", id.0, generation)?;
        write_object(&mut out, obj)?;
        out.extend_from_slice(b"\nendobj\n");
    }

    let mut trailer = Dictionary::new();
    trailer.set("Root", Object::Reference(catalog_id));
    trailer.set("Prev", prev_startxref as i64);
    for key in [b"Info".as_slice(), b"ID".as_slice()] {
        if let Ok(v) = pdf.trailer.get(key) {
            trailer.set(key.to_vec(), v.clone());
        }
    }

    match xref_type {
        XrefType::CrossReferenceTable => {
            let xref_offset = out.len();
            trailer.set("Size", (pdf.max_id + 1) as i64);
            out.extend_from_slice(b"xref\n");
            for (start, run) in runs(offsets.keys().copied()) {
                writeln!(out, "{} {}", start, run.len())?;
                for id in run {
                    let (off, generation) = offsets[&id];
                    writeln!(out, "{:010} {:05} n ", off, generation)?;
                }
            }
            out.extend_from_slice(b"trailer\n");
            write_object(&mut out, &Object::Dictionary(trailer))?;
            write!(out, "\nstartxref\n{}\n%%EOF\n", xref_offset)?;
        }
        XrefType::CrossReferenceStream => {
            // The stream describes itself too.
            let xref_id = pdf.new_object_id();
            let xref_offset = out.len();
            offsets.insert(xref_id.0, (xref_offset, 0));
            let mut index = Vec::new();
            let mut data = Vec::new();
            for (start, run) in runs(offsets.keys().copied()) {
                index.push(Object::Integer(start as i64));
                index.push(Object::Integer(run.len() as i64));
                for id in run {
                    let (off, generation) = offsets[&id];
                    data.push(1u8);
                    data.extend_from_slice(&(off as u64).to_be_bytes());
                    data.extend_from_slice(&generation.to_be_bytes());
                }
            }
            trailer.set("Type", Object::Name(b"XRef".to_vec()));
            trailer.set("Size", (xref_id.0 + 1) as i64);
            trailer.set(
                "W",
                vec![Object::Integer(1), Object::Integer(8), Object::Integer(2)],
            );
            trailer.set("Index", index);
            let mut stream = Stream::new(trailer, data);
            stream.allows_compression = false;
            writeln!(out, "{} 0 obj", xref_id.0)?;
            write_object(&mut out, &Object::Stream(stream))?;
            write!(out, "\nendobj\nstartxref\n{}\n%%EOF\n", xref_offset)?;
        }
    }
    Ok((original_len, out))
}

/// Each reference in `o`.
fn refs(o: &Object, f: &mut dyn FnMut(ObjectId)) {
    match o {
        Object::Reference(id) => f(*id),
        Object::Array(a) => a.iter().for_each(|o| refs(o, f)),
        Object::Dictionary(d) => d.iter().for_each(|(_, o)| refs(o, f)),
        Object::Stream(s) => s.dict.iter().for_each(|(_, o)| refs(o, f)),
        _ => {}
    }
}

/// Contiguous runs of ascending ids.
fn runs(ids: impl Iterator<Item = u32>) -> Vec<(u32, Vec<u32>)> {
    let mut out: Vec<(u32, Vec<u32>)> = Vec::new();
    for id in ids {
        match out.last_mut() {
            Some((start, run)) if *start + run.len() as u32 == id => run.push(id),
            _ => out.push((id, vec![id])),
        }
    }
    out
}

// ----- minimal PDF object serializer (lopdf's writer is private) ----------

fn write_object(out: &mut Vec<u8>, obj: &Object) -> Result<()> {
    match obj {
        Object::Null => out.extend_from_slice(b"null"),
        Object::Boolean(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Object::Integer(i) => write!(out, "{i}")?,
        Object::Real(r) => {
            // Shortest form that round-trips reasonably; PDF has no exponent syntax.
            let s = format!("{:.6}", r);
            let s = s.trim_end_matches('0').trim_end_matches('.');
            out.extend_from_slice(if s.is_empty() || s == "-" {
                b"0"
            } else {
                s.as_bytes()
            });
        }
        Object::Name(n) => write_name(out, n),
        Object::String(bytes, _) => {
            // Hex form is always valid regardless of content or the original
            // format flag.
            let _ = StringFormat::Hexadecimal;
            out.push(b'<');
            for b in bytes {
                write!(out, "{:02X}", b)?;
            }
            out.push(b'>');
        }
        Object::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b' ');
                }
                write_object(out, item)?;
            }
            out.push(b']');
        }
        Object::Dictionary(d) => write_dict(out, d)?,
        Object::Stream(st) => {
            let mut dict = st.dict.clone();
            dict.set("Length", st.content.len() as i64);
            write_dict(out, &dict)?;
            out.extend_from_slice(b"\nstream\n");
            out.extend_from_slice(&st.content);
            out.extend_from_slice(b"\nendstream");
        }
        Object::Reference((n, g)) => write!(out, "{n} {g} R")?,
    }
    Ok(())
}

fn write_dict(out: &mut Vec<u8>, d: &Dictionary) -> Result<()> {
    out.extend_from_slice(b"<<");
    for (k, v) in d.iter() {
        write_name(out, k);
        out.push(b' ');
        write_object(out, v)?;
        out.push(b' ');
    }
    out.extend_from_slice(b">>");
    Ok(())
}

fn write_name(out: &mut Vec<u8>, name: &[u8]) {
    out.push(b'/');
    for &b in name {
        let regular = b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'_'
                    | b'.'
                    | b'+'
                    | b'@'
                    | b'*'
                    | b'$'
                    | b','
                    | b':'
                    | b';'
                    | b'='
                    | b'?'
                    | b'!'
                    | b'^'
                    | b'~'
                    | b'|'
                    | b'&'
                    | b'\''
            );
        if regular && b > b' ' && b < 0x7f {
            out.push(b);
        } else {
            out.extend_from_slice(format!("#{:02X}", b).as_bytes());
        }
    }
}
