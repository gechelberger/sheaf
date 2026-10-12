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

use crate::model::Snapshot;
use crate::save::{
    LoadedSource, copy_page_dict, geom_of, is_identity, link_connectors, strip_sheaf_annots,
    write_outline, write_page_annots,
};

/// Produce the full new file (original bytes + update section).
pub fn incremental(snap: &Snapshot) -> Result<Vec<u8>> {
    Ok(build(snap)?.1)
}

/// Just the update section, with the length of the original it appends
/// to. Empty when there's nothing to write. Used for crash recovery.
pub fn incremental_section(snap: &Snapshot) -> Result<(usize, Vec<u8>)> {
    let (original_len, mut full) = build(snap)?;
    let section = full.split_off(original_len);
    Ok((original_len, section))
}

/// The original bytes with the update appended, and where the original ends.
fn build(snap: &Snapshot) -> Result<(usize, Vec<u8>)> {
    let primary_id = snap.primary;
    let info = snap.source(primary_id).context("primary source missing")?;
    let original =
        std::fs::read(&info.path).with_context(|| format!("reading {}", info.path.display()))?;
    let original_len = original.len();
    let mut pdf =
        Pdf::load_mem(&original).with_context(|| format!("parsing {}", info.path.display()))?;
    if pdf.encryption_state.is_some() || pdf.trailer.has(b"Encrypt") {
        bail!("incremental save of encrypted files is not supported yet");
    }
    let prev_startxref = pdf.xref_start;
    let orig_max_id = pdf.max_id;
    let xref_type = pdf.reference_table.cross_reference_type;

    let primary = LoadedSource::from_pdf(&pdf)?;
    let mut others: std::collections::HashMap<_, LoadedSource> = std::collections::HashMap::new();
    for (&sid, sinfo) in &snap.sources {
        if sid != primary_id && snap.state.pages.iter().any(|p| p.source == sid) {
            let other = Pdf::load(&sinfo.path)
                .with_context(|| format!("loading {}", sinfo.path.display()))?;
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
    let mut copy_maps: std::collections::HashMap<_, std::collections::HashMap<ObjectId, ObjectId>> =
        std::collections::HashMap::new();

    let mut connectors = Vec::new();
    if tree_changed {
        let pages_id = pdf.new_object_id();
        let mut kids = Vec::with_capacity(snap.state.pages.len());
        let mut used = std::collections::HashSet::new();
        for pref in &snap.state.pages {
            let (src_page, inherited, mut dict) = if pref.source == primary_id {
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
                let map = copy_maps.entry(pref.source).or_default();
                let dict = copy_page_dict(src.pdf.as_ref().unwrap(), id, &mut pdf, map)?;
                (
                    id,
                    src.inherited.get(&id).cloned().unwrap_or_default(),
                    dict,
                )
            };
            for (k, v) in &inherited {
                if !dict.has(k) {
                    dict.set(k.clone(), v.clone());
                }
            }
            let original_rot = dict
                .get(b"Rotate")
                .ok()
                .and_then(|o| o.as_i64().ok())
                .unwrap_or(0) as i32;
            if let Some(list) = snap.state.annots.get(&pref.id) {
                let geom = geom_of(&dict, original_rot);
                connectors.extend(write_page_annots(&mut pdf, &mut dict, list.iter(), &geom));
            } else {
                strip_sheaf_annots(&pdf, &mut dict);
            }
            apply_rotation(&mut dict, pref.rotation.degrees());
            dict.set("Parent", pages_id);
            dict.set("Type", Object::Name(b"Page".to_vec()));
            let id = if pref.source == primary_id && used.insert(src_page) {
                src_page
            } else if pref.source != primary_id {
                // copy_page_dict reserved an id for the page itself.
                copy_maps[&pref.source][&src_page]
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
            if pref.rotation.degrees() != 0 || snap.annots_dirty.contains(&pref.id) {
                let mut dict = pdf.get_dictionary(id)?.clone();
                if !dict.has(b"Rotate")
                    && let Some((_, v)) = primary
                        .inherited
                        .get(&id)
                        .and_then(|a| a.iter().find(|(k, _)| k == b"Rotate"))
                {
                    dict.set("Rotate", v.clone());
                }
                if snap.annots_dirty.contains(&pref.id) {
                    let geom = crate::file::page_geom(&pdf, id);
                    match snap.state.annots.get(&pref.id) {
                        Some(list) => connectors.extend(write_page_annots(
                            &mut pdf,
                            &mut dict,
                            list.iter(),
                            &geom,
                        )),
                        None => strip_sheaf_annots(&pdf, &mut dict),
                    }
                }
                apply_rotation(&mut dict, pref.rotation.degrees());
                changed.insert(id, Object::Dictionary(dict));
            }
        }
    }

    if snap.outline_dirty {
        catalog.remove(b"Outlines");
        if !snap.state.outline.is_empty() {
            let outlines_id = write_outline(&mut pdf, &snap.state.outline, &page_obj);
            catalog.set("Outlines", outlines_id);
        }
        catalog_changed = true;
    }
    if catalog_changed {
        changed.insert(catalog_id, Object::Dictionary(catalog));
    }
    link_connectors(&mut pdf, &connectors, &snap.state, &page_obj);
    // Everything allocated above the original max id is new and must be written.
    for (&id, obj) in pdf.objects.range((orig_max_id + 1, 0)..) {
        changed.entry(id).or_insert_with(|| obj.clone());
    }
    if changed.is_empty() {
        return Ok((original_len, original));
    }

    // ----- serialize the update section ---------------------------------------
    let generation = |id: ObjectId, pdf: &Pdf| -> u16 {
        match pdf.reference_table.entries.get(&id.0) {
            Some(XrefEntry::Normal { generation, .. }) => *generation,
            _ => id.1,
        }
    };
    let mut out = original;
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

fn apply_rotation(dict: &mut Dictionary, delta: i32) {
    let original = dict
        .get(b"Rotate")
        .ok()
        .and_then(|o| o.as_i64().ok())
        .unwrap_or(0);
    let rotate = (original + delta as i64).rem_euclid(360);
    if rotate == 0 {
        dict.remove(b"Rotate");
    } else {
        dict.set("Rotate", rotate);
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
