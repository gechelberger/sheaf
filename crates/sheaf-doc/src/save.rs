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

use crate::annot::{Annot, AnnotId, AnnotKind, PageGeom};
use crate::model::{DocState, OutlineNode, PageId, PageRef, Snapshot, SourceId, SourceInfo};
use crate::text_layer;
use sheaf_core::{DestView, PageFrame};

/// Page-level keys a page may inherit from its ancestors in the page tree.
/// We flatten the tree, so these must be copied onto each page.
const INHERITED: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

/// Serialize a snapshot as a complete new PDF.
pub fn rewrite(model: &Snapshot) -> Result<Vec<u8>> {
    Ok(rewrite_with(model, false)?.0)
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
    let (bytes, _) = rewrite_with(model, true)?;
    write_atomic(path, &bytes)
}

/// `clean`: strip all markup instead of writing our annotations. With what
/// the reader should be told (see `kept::keep_only`).
fn rewrite_with(model: &Snapshot, clean: bool) -> Result<(Vec<u8>, Vec<String>)> {
    let (pdf, notes) = build_rewrite(model, clean)?;
    Ok((finish(pdf)?, notes))
}

/// Prune, renumber, compress and serialize a rewritten document. It's
/// written as one section, so a `/Prev` or `/XRefStm` left in the trailer
/// (lopdf keeps them when it rebuilds a damaged file's cross-references)
/// would point into it at random.
fn finish(mut pdf: Pdf) -> Result<Vec<u8>> {
    pdf.trailer.remove(b"Prev");
    pdf.trailer.remove(b"XRefStm");
    pdf.prune_objects();
    pdf.renumber_objects();
    pdf.compress();
    let mut out = Vec::new();
    pdf.save_to(&mut out)?;
    Ok(out)
}

/// Acrobat's Extract Pages: `ids` (kept in document order) become a new
/// file at `path`, along with their markup and the bookmarks that point at
/// them. Returns how many pages were written.
pub fn extract_pages(model: &Snapshot, ids: &[PageId], path: &Path) -> Result<usize> {
    let bytes = extract_bytes(model, ids)?;
    write_atomic(path, &bytes)?;
    Ok(ids.len())
}

/// What [`extract_pages`] would write, in memory.
pub fn extract_bytes(model: &Snapshot, ids: &[PageId]) -> Result<Vec<u8>> {
    let snap = subset(model, ids, true).context("no pages to extract")?;
    Ok(rewrite_with(&snap, false)?.0)
}

/// `ids` (in document order) with their markup and text layers, and their
/// bookmarks when `outline`. None when no page is kept.
fn subset(model: &Snapshot, ids: &[PageId], outline: bool) -> Option<Snapshot> {
    let keep: HashSet<PageId> = ids.iter().copied().collect();
    let pages: Vector<PageRef> = model
        .state
        .pages
        .iter()
        .filter(|p| keep.contains(&p.id))
        .copied()
        .collect();
    if pages.is_empty() {
        return None;
    }
    let mut annots = model.state.annots.clone();
    annots.retain(|id, _| keep.contains(id));
    let mut text_layers = model.state.text_layers.clone();
    text_layers.retain(|id, _| keep.contains(id));
    let mut widgets = model.state.widgets.clone();
    widgets.retain(|id, _| keep.contains(id));
    Some(Snapshot {
        state: DocState {
            pages,
            outline: if outline {
                prune_outline(&model.state.outline, &keep)
            } else {
                Vector::new()
            },
            annots,
            text_layers,
            labels: if outline {
                model.state.labels.clone()
            } else {
                None
            },
            header_footer: model.state.header_footer.clone(),
            removed_comments: {
                let mut r = model.state.removed_comments.clone();
                r.retain(|id, _| keep.contains(id));
                r
            },
            fields: model.state.fields.clone(),
            widgets,
        },
        primary: model.primary,
        sources: model.sources.clone(),
        outline_dirty: true,
        labels_dirty: true,
        header_footer_dirty: model.header_footer_dirty,
        annots_dirty: HashSet::new(),
        fields_dirty: HashSet::new(),
        removes_content: false,
    })
}

/// The file sent to a printer: `ids` in document order, as they look
/// now. With `markup`, every comment and mark is drawn into the page (as
/// Export Flattened Copy does), so any printing system shows it; without,
/// markup is left out (Export Clean Copy). Form fields print either way.
pub fn print_pdf(model: &Snapshot, ids: &[PageId], markup: bool) -> Result<Vec<u8>> {
    let snap = subset(model, ids, false).context("no pages to print")?;
    let (mut pdf, _) = build_rewrite(&snap, !markup)?;
    if markup {
        flatten_annots(&mut pdf)?;
    }
    finish(pdf)
}

/// Bookmarks that still point somewhere: a node survives if its target is
/// kept or any descendant survives (then it becomes a plain heading).
fn prune_outline(nodes: &Vector<OutlineNode>, keep: &HashSet<PageId>) -> Vector<OutlineNode> {
    nodes
        .iter()
        .filter_map(|n| {
            let children = prune_outline(&n.children, keep);
            let target = n.target.filter(|t| keep.contains(t));
            if target.is_none() && children.is_empty() {
                return None;
            }
            Some(OutlineNode {
                id: n.id,
                title: n.title.clone(),
                view: target.and(n.view.clone()),
                target,
                color: n.color,
                style: n.style,
                children,
            })
        })
        .collect()
}

/// Export with every markup annotation's appearance drawn into its page's
/// content and the annotations themselves removed, so the marks show in
/// any viewer and can't be edited. Links and form fields stay, as in the
/// clean copy; popups and hidden annotations are dropped without drawing.
pub fn export_flattened(model: &Snapshot, path: &Path) -> Result<usize> {
    let primary = model
        .source(model.primary)
        .context("primary source missing")?;
    if path.exists()
        && std::fs::canonicalize(path).ok() == std::fs::canonicalize(&primary.path).ok()
    {
        bail!("a flattened copy can't replace the open file; choose another name");
    }
    let (mut pdf, _) = build_rewrite(model, false)?;
    let n = flatten_annots(&mut pdf)?;
    let bytes = finish(pdf)?;
    write_atomic(path, &bytes)?;
    Ok(n)
}

/// Draw annotation appearances into page content. Returns how many were
/// flattened.
fn flatten_annots(pdf: &mut Pdf) -> Result<usize> {
    const HIDDEN: i64 = 1 << 1;
    const NOVIEW: i64 = 1 << 5;
    let mut flattened = 0usize;
    let pages: Vec<ObjectId> = pdf.get_pages().into_values().collect();
    for page_id in pages {
        let page = pdf.get_dictionary(page_id)?.clone();
        let Some(annots) = page
            .get(b"Annots")
            .ok()
            .and_then(|a| pdf.dereference(a).ok())
            .and_then(|(_, o)| o.as_array().ok().cloned())
        else {
            continue;
        };
        let mut kept: Vec<Object> = Vec::new();
        let mut ops = String::new();
        let mut xobjects: Vec<(String, ObjectId)> = Vec::new();
        for entry in &annots {
            let Ok((_, obj)) = pdf.dereference(entry) else {
                continue;
            };
            let Ok(dict) = obj.as_dict() else { continue };
            let subtype = dict
                .get(b"Subtype")
                .ok()
                .and_then(|s| s.as_name().ok())
                .unwrap_or(b"");
            if (subtype == b"Link" || subtype == b"Widget") && !dict.has(b"SheafAnnot") {
                kept.push(entry.clone());
                continue;
            }
            if subtype == b"Popup" {
                continue;
            }
            let flags = dict
                .get(b"F")
                .ok()
                .and_then(|f| f.as_i64().ok())
                .unwrap_or(0);
            if flags & (HIDDEN | NOVIEW) != 0 {
                continue;
            }
            let Some(stream_id) = normal_appearance(pdf, dict) else {
                continue;
            };
            let Ok(stream) = pdf.get_object(stream_id).and_then(|o| o.as_stream()) else {
                continue;
            };
            let rect: Vec<f32> = dict
                .get(b"Rect")
                .ok()
                .and_then(|o| pdf.dereference(o).ok())
                .and_then(|(_, o)| o.as_array().ok())
                .map(|a| a.iter().map(|v| v.as_float().unwrap_or(0.0)).collect())
                .unwrap_or_default();
            if rect.len() != 4 {
                continue;
            }
            let (llx, lly, urx, ury) = (
                rect[0].min(rect[2]),
                rect[1].min(rect[3]),
                rect[0].max(rect[2]),
                rect[1].max(rect[3]),
            );
            // PDF 32000 12.5.5: map the form's (Matrix-transformed) BBox onto Rect.
            let bbox: Vec<f32> = stream
                .dict
                .get(b"BBox")
                .ok()
                .and_then(|o| o.as_array().ok())
                .map(|a| a.iter().map(|v| v.as_float().unwrap_or(0.0)).collect())
                .unwrap_or_else(|| vec![llx, lly, urx, ury]);
            let m: Vec<f32> = stream
                .dict
                .get(b"Matrix")
                .ok()
                .and_then(|o| o.as_array().ok())
                .map(|a| a.iter().map(|v| v.as_float().unwrap_or(0.0)).collect())
                .filter(|m: &Vec<f32>| m.len() == 6)
                .unwrap_or_else(|| vec![1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
            if bbox.len() != 4 {
                continue;
            }
            let xf = |x: f32, y: f32| (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]);
            let corners = [
                xf(bbox[0], bbox[1]),
                xf(bbox[2], bbox[1]),
                xf(bbox[2], bbox[3]),
                xf(bbox[0], bbox[3]),
            ];
            let bx0 = corners.iter().map(|c| c.0).fold(f32::MAX, f32::min);
            let bx1 = corners.iter().map(|c| c.0).fold(f32::MIN, f32::max);
            let by0 = corners.iter().map(|c| c.1).fold(f32::MAX, f32::min);
            let by1 = corners.iter().map(|c| c.1).fold(f32::MIN, f32::max);
            let sx = if bx1 - bx0 > 1e-6 {
                (urx - llx) / (bx1 - bx0)
            } else {
                1.0
            };
            let sy = if by1 - by0 > 1e-6 {
                (ury - lly) / (by1 - by0)
            } else {
                1.0
            };
            let (tx, ty) = (llx - bx0 * sx, lly - by0 * sy);
            let name = format!("SheafFlat{}", xobjects.len() + 1);
            ops.push_str(&format!(
                " q {sx:.4} 0 0 {sy:.4} {tx:.3} {ty:.3} cm /{name} Do Q"
            ));
            xobjects.push((name, stream_id));
            flattened += 1;
        }
        if xobjects.is_empty() && kept.len() == annots.len() {
            continue;
        }
        // Register the forms as page XObjects and append the draws after
        // the existing content, which is wrapped so its state can't leak.
        let mut resources = page
            .get(b"Resources")
            .ok()
            .and_then(|r| pdf.dereference(r).ok())
            .and_then(|(_, o)| o.as_dict().ok().cloned())
            .unwrap_or_default();
        let mut xo = resources
            .get(b"XObject")
            .ok()
            .and_then(|r| pdf.dereference(r).ok())
            .and_then(|(_, o)| o.as_dict().ok().cloned())
            .unwrap_or_default();
        for (name, id) in &xobjects {
            xo.set(name.clone(), Object::Reference(*id));
        }
        resources.set("XObject", Object::Dictionary(xo));
        let mut content = b"q\n".to_vec();
        content.extend_from_slice(&pdf.get_page_content(page_id));
        content.extend_from_slice(b"\nQ\n");
        content.extend_from_slice(ops.as_bytes());
        let content_id =
            pdf.add_object(Object::Stream(lopdf::Stream::new(dictionary! {}, content)));
        let page_mut = pdf.get_dictionary_mut(page_id)?;
        page_mut.set("Contents", Object::Reference(content_id));
        page_mut.set("Resources", Object::Dictionary(resources));
        if kept.is_empty() {
            page_mut.remove(b"Annots");
        } else {
            page_mut.set("Annots", Object::Array(kept));
        }
    }
    Ok(flattened)
}

/// The annotation's normal appearance stream, resolving appearance
/// sub-dictionaries through `/AS` (or a lone state).
fn normal_appearance(pdf: &Pdf, annot: &Dictionary) -> Option<ObjectId> {
    let ap = annot
        .get(b"AP")
        .ok()
        .and_then(|o| pdf.dereference(o).ok())
        .and_then(|(_, o)| o.as_dict().ok().cloned())?;
    let n = ap.get(b"N").ok()?;
    if let Ok(id) = n.as_reference()
        && pdf.get_object(id).ok()?.as_stream().is_ok()
    {
        return Some(id);
    }
    let (_, states) = pdf.dereference(n).ok()?;
    let states = states.as_dict().ok()?;
    let state = annot
        .get(b"AS")
        .ok()
        .and_then(|s| s.as_name().ok())
        .map(|s| s.to_vec())
        .or_else(|| states.iter().next().map(|(k, _)| k.clone()))?;
    states.get(&state).ok()?.as_reference().ok()
}

/// The rewritten document, before serialization, and what the reader
/// should be told about it.
fn build_rewrite(model: &Snapshot, clean: bool) -> Result<(Pdf, Vec<String>)> {
    let primary_id = model.primary;
    let primary_info = model.source(primary_id).context("primary source missing")?;
    let mut pdf = crate::file::load_source(primary_info)?;
    let state = &model.state;
    let origins = origins(model);

    // Source page object ids and their inherited attributes, resolved against
    // the *original* trees before anything is re-parented.
    let mut sources: HashMap<SourceId, LoadedSource> = HashMap::new();
    sources.insert(primary_id, LoadedSource::from_pdf(&pdf)?);
    for (&sid, info) in &model.sources {
        if sid != primary_id && needed(model, &origins, sid) {
            let other = crate::file::load_source(info)?;
            sources.insert(sid, LoadedSource::from_pdf_owned(other)?);
        }
    }

    let pages_id = pdf.new_object_id();
    let mut kids: Vec<Object> = Vec::with_capacity(state.pages.len());
    let mut page_obj: HashMap<PageId, ObjectId> = HashMap::new();
    // Each page's geometry before our rotation, for bookmark positions.
    let mut frames: HashMap<PageId, PageFrame> = HashMap::new();
    let mut used: HashSet<ObjectId> = HashSet::new();
    let mut copy_maps: HashMap<SourceId, HashMap<ObjectId, ObjectId>> = HashMap::new();
    let mut connectors = Vec::new();
    let mut stamp_forms = StampForms::new();
    let mut ocr_objects = None;
    let mut stamp_objects = None;
    // Pages from other files get their ids first, so links among them
    // follow (bug 14); the first use of each fills its id. A derived
    // copy's page is a use of the page it was made from.
    for pref in &state.pages {
        let (source, index) = origins
            .get(&pref.id)
            .copied()
            .unwrap_or((pref.source, pref.index));
        if source != primary_id
            && let Some(&p) = sources
                .get(&source)
                .and_then(|s| s.pages.get(index as usize))
        {
            reserve_pages([p], &mut pdf, copy_maps.entry(source).or_default());
        }
    }
    let mut filled: HashSet<(SourceId, ObjectId)> = HashSet::new();
    let page_ids: Vec<PageId> = state.pages.iter().map(|p| p.id).collect();
    let bates = state
        .header_footer
        .as_ref()
        .map(|hf| hf.bates_numbers(&page_ids))
        .unwrap_or_default();
    // Primary page dicts as loaded. A duplicated page is visited twice and
    // the first visit rewrites the object, so the second must not read it
    // back (it would see the rotation already applied).
    let mut originals: HashMap<ObjectId, Dictionary> = HashMap::new();

    for pref in &state.pages {
        let src = sources.get(&pref.source).context("unknown source")?;
        let shown_page = *src
            .pages
            .get(pref.index as usize)
            .context("source page index out of range")?;
        let inherited = src.inherited.get(&shown_page).cloned().unwrap_or_default();
        // A derived copy's page is written as the page it was made from:
        // its object, annotations and form fields, with the copy's content
        // (M10 bug 1).
        let origin = origins.get(&pref.id).copied();
        let source = origin.map_or(pref.source, |(s, _)| s);
        let src_page = match origin {
            Some((s, i)) => *sources
                .get(&s)
                .and_then(|o| o.pages.get(i as usize))
                .context("a derived page's original is missing")?,
            None => shown_page,
        };

        let mut dict: Dictionary = if pref.source == primary_id {
            match originals.get(&src_page) {
                Some(d) => d.clone(),
                None => {
                    let d = pdf.get_dictionary(src_page)?.clone();
                    originals.insert(src_page, d.clone());
                    d
                }
            }
        } else {
            let map = copy_maps.entry(pref.source).or_default();
            let src_pdf = src
                .pdf
                .as_ref()
                .expect("non-primary sources keep their pdf");
            if origin.is_some() {
                copy_derived_page(src_pdf, shown_page, &mut pdf, map)?
            } else {
                copy_page_dict(src_pdf, shown_page, &mut pdf, map)?
            }
        };
        if origin.is_some() {
            let annots = if source == primary_id {
                if let std::collections::hash_map::Entry::Vacant(e) = originals.entry(src_page) {
                    e.insert(pdf.get_dictionary(src_page)?.clone());
                }
                originals[&src_page].get(b"Annots").ok().cloned()
            } else {
                let from = sources[&source]
                    .pdf
                    .as_ref()
                    .expect("non-primary sources keep their pdf");
                let map = copy_maps.entry(source).or_default();
                match from.get_dictionary(src_page)?.get(b"Annots") {
                    Ok(a) => Some(remap(from, a, &mut pdf, map)?),
                    Err(_) => None,
                }
            };
            match annots {
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
        // Rotation: the source's own /Rotate plus our delta.
        let original = dict
            .get(b"Rotate")
            .ok()
            .and_then(|o| o.as_i64().ok())
            .unwrap_or(0);
        if clean {
            strip_markup_annots(&pdf, &mut dict);
        } else {
            match (
                state.annots.get(&pref.id),
                state.removed_comments.get(&pref.id),
            ) {
                (None, None) => strip_sheaf_annots(&pdf, &mut dict),
                (list, removed) => {
                    let geom = geom_of(&dict, original as i32);
                    let marks = list.into_iter().flatten();
                    connectors.extend(
                        write_page_annots(
                            &mut pdf,
                            &mut dict,
                            marks,
                            removed,
                            &geom,
                            &mut stamp_forms,
                        )
                        .connectors,
                    );
                }
            }
        }
        if let Some(layer) = state.text_layers.get(&pref.id) {
            let geom = geom_of(&dict, original as i32);
            text_layer::install(&mut pdf, &mut dict, layer, &geom, &mut ocr_objects);
        }
        // A header/footer the file doesn't have drawn yet.
        if model.header_footer_dirty
            && let Some(hf) = &state.header_footer
            && hf.covers(pref.id)
        {
            let geom = geom_of(&dict, original as i32);
            let extra = pref.rotation.degrees();
            let [x0, y0, x1, y1] = geom.media;
            let total = (geom.rotate + extra).rem_euclid(360);
            let size = if total % 180 == 0 {
                sheaf_core::PageSize {
                    width: x1 - x0,
                    height: y1 - y0,
                }
            } else {
                sheaf_core::PageSize {
                    width: y1 - y0,
                    height: x1 - x0,
                }
            };
            let position = kids.len();
            let facts = crate::stamps::PageFacts {
                index: position,
                count: state.pages.len(),
                bates: bates.get(position).copied().flatten(),
            };
            let placed = hf.layout(size, &facts);
            crate::stamps::install(
                &mut pdf,
                &mut dict,
                &placed,
                hf.color,
                &geom,
                extra,
                &mut stamp_objects,
            );
        }
        let g = geom_of(&dict, original as i32);
        frames.insert(
            pref.id,
            PageFrame {
                media: g.media,
                rotate: g.rotate,
            },
        );
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
        let id = if source == primary_id && used.insert(src_page) {
            *pdf.get_object_mut(src_page)? = Object::Dictionary(dict);
            src_page
        } else if source != primary_id && filled.insert((source, src_page)) {
            let id = copy_maps[&source][&src_page];
            pdf.objects.insert(id, Object::Dictionary(dict));
            id
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
    // (serialization happens in `finish`)

    let catalog_id = pdf.trailer.get(b"Root")?.as_reference()?;
    // Labels the model keeps follow their pages.
    let labels_tree = state
        .label_ranges()
        .filter(|r| !r.is_empty())
        .map(|r| pdf.add_object(crate::labels::number_tree(&r)));
    let identity = is_identity(model, &sources[&primary_id]);
    let catalog = pdf.get_dictionary_mut(catalog_id)?;
    catalog.set("Pages", pages_id);
    // The header/footer's settings, so Update can start from them.
    match &state.header_footer {
        Some(hf) => catalog.set(
            crate::stamps::CATALOG_KEY,
            crate::stamps::to_catalog(hf, &page_ids),
        ),
        None => {
            catalog.remove(crate::stamps::CATALOG_KEY.as_bytes());
        }
    }
    match labels_tree {
        Some(id) => catalog.set("PageLabels", id),
        // Removed.
        None if model.labels_dirty => {
            catalog.remove(b"PageLabels");
        }
        // Never read into the model: the file's index by position, so
        // they'd be wrong after any reorder.
        None => {
            if !identity {
                catalog.remove(b"PageLabels");
            }
        }
    }
    catalog.remove(b"Outlines");
    if !state.outline.is_empty() {
        let outlines_id = write_outline(&mut pdf, &state.outline, &page_obj, &|id| {
            frames.get(&id).copied()
        });
        pdf.get_dictionary_mut(catalog_id)?
            .set("Outlines", outlines_id);
    }
    // Nothing reaches a page that isn't kept (bug 14), so pruning takes it
    // out of the file.
    let kept: HashSet<ObjectId> = page_obj.values().copied().collect();
    let notes = crate::kept::keep_only(&mut pdf, &kept);
    for n in &notes {
        log::warn!("{n}");
    }
    Ok((pdf, notes))
}

/// Pages of `src`, each given a new `/CropBox` (`[x0, y0, x1, y1]` in its
/// user space), as a new PDF: what a cropped page becomes, since sources
/// are files. See [`derived_copy`].
pub fn cropped_copy(src: &SourceInfo, pages: &[(u32, [f32; 4])]) -> Result<Vec<u8>> {
    let pages: Vec<(u32, Option<[f32; 4]>)> = pages.iter().map(|&(i, c)| (i, Some(c))).collect();
    derived_copy(src, &pages, &mut |_, _, _, _| Ok(()))
}

/// Pages of `src` as a new PDF, each with a new `/CropBox` when given,
/// then passed to `fix` (with the new PDF, the page's dictionary, its
/// geometry, and its position in `pages`) to change its content. sheaf's
/// own marks are left out, since the model keeps them; other
/// annotations, links and fields come along, and the form the fields are
/// in, so the page shows them (see [`copy_form`]). A save writes the page
/// it was made from instead, with the copy's content
/// (`SourceInfo::derived`).
pub fn derived_copy(
    src: &SourceInfo,
    pages: &[(u32, Option<[f32; 4]>)],
    fix: &mut dyn FnMut(&mut Pdf, &mut Dictionary, &PageGeom, usize) -> Result<()>,
) -> Result<Vec<u8>> {
    let from = crate::file::load_source(src)?;
    let loaded = LoadedSource::from_pdf(&from)?;
    let mut pdf = Pdf::with_version("1.7");
    let pages_id = pdf.new_object_id();
    let mut map: HashMap<ObjectId, ObjectId> = HashMap::new();
    let mut kids: Vec<Object> = Vec::with_capacity(pages.len());
    reserve_pages(
        pages
            .iter()
            .filter_map(|&(index, _)| loaded.pages.get(index as usize).copied()),
        &mut pdf,
        &mut map,
    );
    let mut filled = HashSet::new();
    for (position, &(index, crop)) in pages.iter().enumerate() {
        let page = *loaded
            .pages
            .get(index as usize)
            .with_context(|| format!("no page {index} in {}", src.path.display()))?;
        let mut dict = copy_page_dict(&from, page, &mut pdf, &mut map)?;
        let id = if filled.insert(page) {
            map[&page]
        } else {
            pdf.new_object_id()
        };
        for (k, v) in loaded.inherited.get(&page).into_iter().flatten() {
            if !dict.has(k) {
                let v = remap(&from, v, &mut pdf, &mut map)?;
                dict.set(k.clone(), v);
            }
        }
        strip_sheaf_annots(&pdf, &mut dict);
        if let Some(crop) = crop {
            dict.set(
                "CropBox",
                crop.iter().map(|&v| Object::Real(v)).collect::<Vec<_>>(),
            );
        }
        let rotate = dict
            .get(b"Rotate")
            .ok()
            .and_then(|o| o.as_i64().ok())
            .unwrap_or(0) as i32;
        let geom = geom_of(&dict, rotate);
        fix(&mut pdf, &mut dict, &geom, position)?;
        dict.set("Parent", pages_id);
        dict.set("Type", Object::Name(b"Page".to_vec()));
        pdf.objects.insert(id, Object::Dictionary(dict));
        kids.push(id.into());
    }
    let count = kids.len() as i64;
    let kept: HashSet<ObjectId> = kids.iter().filter_map(|k| k.as_reference().ok()).collect();
    pdf.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count }),
    );
    let mut catalog = dictionary! { "Type" => "Catalog", "Pages" => pages_id };
    if let Some(form) = copy_form(&from, &mut pdf, &mut map)? {
        catalog.set("AcroForm", pdf.add_object(form));
    }
    let catalog = pdf.add_object(catalog);
    pdf.trailer.set("Root", catalog);
    // Links to pages that weren't copied lead nowhere, and widgets there
    // leave their fields: they go, and so does what only they reached.
    crate::kept::keep_only(&mut pdf, &kept);
    pdf.prune_objects();
    let mut out = Vec::new();
    pdf.save_to(&mut out)?;
    Ok(out)
}

/// The form of `from`, as far as the pages copied into `pdf` reach it
/// (their widgets' fields, copied with them through `map`): the roots
/// among `/Fields` and `/CO` that were copied, and `/DR`, `/DA`, `/Q` and
/// `NeedAppearances`. PDFium draws widgets only in a document with a form
/// (M10 bug 1). None when no field was copied. The XFA copy, usage rights
/// and `/SigFlags` describe the whole file, so they stay behind.
fn copy_form(
    from: &Pdf,
    pdf: &mut Pdf,
    map: &mut HashMap<ObjectId, ObjectId>,
) -> Result<Option<Dictionary>> {
    let Some(form) = from
        .catalog()
        .ok()
        .and_then(|c| c.get(b"AcroForm").ok())
        .and_then(|f| from.dereference(f).ok())
        .and_then(|(_, f)| f.as_dict().ok())
    else {
        return Ok(None);
    };
    let copied = |key: &[u8]| -> Vec<Object> {
        form.get(key)
            .ok()
            .and_then(|a| from.dereference(a).ok())
            .and_then(|(_, a)| a.as_array().ok())
            .into_iter()
            .flatten()
            .filter_map(|f| map.get(&f.as_reference().ok()?).copied())
            .map(Object::Reference)
            .collect()
    };
    let fields = copied(b"Fields");
    if fields.is_empty() {
        return Ok(None);
    }
    let co = copied(b"CO");
    let mut out = dictionary! { "Fields" => fields };
    if !co.is_empty() {
        out.set("CO", co);
    }
    for key in [b"DR".as_slice(), b"DA", b"Q", b"NeedAppearances"] {
        if let Ok(v) = form.get(key) {
            out.set(key.to_vec(), remap(from, v, pdf, map)?);
        }
    }
    Ok(Some(out))
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
    Ok(save_to_path_noting(model, path)?.0)
}

/// [`save_to_path`], and what the reader should be told about the saved
/// file (see `kept::keep_only`).
///
/// When something in the file was removed since it was saved (a page, a
/// mark, a bookmark: `Snapshot::removes_content`), it's a full rewrite:
/// an incremental save would keep the removed content in the earlier
/// revision (M9 bug 14).
pub fn save_to_path_noting(model: &Snapshot, path: &Path) -> Result<(Strategy, Vec<String>)> {
    let primary = model
        .source(model.primary)
        .context("primary source missing")?;
    let same_file = std::fs::canonicalize(path).ok() == std::fs::canonicalize(&primary.path).ok();
    let unchanged = primary.bytes == 0
        || std::fs::metadata(&primary.path)
            .map(|m| m.len() as usize == primary.bytes)
            .unwrap_or(false);
    if same_file && unchanged && !model.removes_content {
        match crate::incremental::incremental(model) {
            Ok(bytes) => {
                write_atomic(path, &bytes)?;
                return Ok((Strategy::Incremental, Vec::new()));
            }
            Err(e) => log::warn!("incremental save failed, rewriting instead: {e:#}"),
        }
    }
    let (bytes, notes) = rewrite_with(model, false)?;
    write_atomic(path, &bytes)?;
    Ok((Strategy::Rewrite, notes))
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

/// The page a derived copy's page was made from (see
/// `SourceInfo::derived`), through copies of copies; None for a page that
/// isn't a copy.
pub(crate) fn origin(model: &Snapshot, source: SourceId, index: u32) -> Option<(SourceId, u32)> {
    let mut at = None;
    let (mut source, mut index) = (source, index);
    // A chain longer than there are sources would be a loop.
    for _ in 0..model.sources.len() {
        let Some(derived) = model.source(source).and_then(|s| s.derived.as_ref()) else {
            break;
        };
        let Some(&i) = derived.pages.get(index as usize) else {
            break;
        };
        (source, index) = (derived.from, i);
        at = Some((source, index));
    }
    at
}

/// [`origin`] for each page of the document that's a derived copy's.
pub(crate) fn origins(model: &Snapshot) -> HashMap<PageId, (SourceId, u32)> {
    model
        .state
        .pages
        .iter()
        .filter_map(|p| Some((p.id, origin(model, p.source, p.index)?)))
        .collect()
}

/// Whether a save reads source `sid`: a page shows it, or was made from
/// one of its pages.
pub(crate) fn needed(
    model: &Snapshot,
    origins: &HashMap<PageId, (SourceId, u32)>,
    sid: SourceId,
) -> bool {
    model.state.pages.iter().any(|p| p.source == sid) || origins.values().any(|o| o.0 == sid)
}

/// A derived copy's page from `src` in `dst`, but for `/Annots`: those are
/// the page it was made from's (see [`origin`]), as is its object.
pub(crate) fn copy_derived_page(
    src: &Pdf,
    page: ObjectId,
    dst: &mut Pdf,
    map: &mut HashMap<ObjectId, ObjectId>,
) -> Result<Dictionary> {
    let mut out = Dictionary::new();
    for (k, v) in src.get_dictionary(page)?.iter() {
        if k == b"Parent" || k == b"Annots" {
            continue;
        }
        out.set(k.clone(), remap(src, v, dst, map)?);
    }
    Ok(out)
}

/// Clone a page dictionary from `src` into `dst`, deep-copying everything it
/// references (resources, content, annotations…) with fresh object numbers.
/// Other pages aren't copied (see [`remap`]): a reference to one the caller
/// is copying too goes through `map`, where [`reserve_pages`] put it.
pub(crate) fn copy_page_dict(
    src: &Pdf,
    page: ObjectId,
    dst: &mut Pdf,
    map: &mut HashMap<ObjectId, ObjectId>,
) -> Result<Dictionary> {
    let dict = src.get_dictionary(page)?.clone();
    // The page's own id must map to the object we're about to create so
    // annotations' /P back-references resolve; the caller inserts it.
    let new_id = match map.get(&page) {
        Some(&id) => id,
        None => {
            let id = dst.new_object_id();
            map.insert(page, id);
            id
        }
    };
    let mut out = Dictionary::new();
    for (k, v) in dict.iter() {
        if k == b"Parent" {
            continue;
        }
        out.set(k.clone(), remap(src, v, dst, map)?);
    }
    // Hold the id so later allocations don't collide; the caller may
    // replace this placeholder, or it stays as the page.
    dst.objects
        .entry(new_id)
        .or_insert_with(|| Object::Dictionary(out.clone()));
    Ok(out)
}

/// Ids in `dst` for the pages of `src` that are being copied, before any
/// is, so a link from one to another follows it instead of being dropped.
pub(crate) fn reserve_pages(
    pages: impl IntoIterator<Item = ObjectId>,
    dst: &mut Pdf,
    map: &mut HashMap<ObjectId, ObjectId>,
) {
    for page in pages {
        map.entry(page).or_insert_with(|| dst.new_object_id());
    }
}

/// A page or a node of the page tree.
pub(crate) fn is_page_node(pdf: &Pdf, id: ObjectId) -> bool {
    matches!(
        pdf.get_object(id),
        Ok(Object::Dictionary(d)) if d
            .get(b"Type")
            .ok()
            .and_then(|t| t.as_name().ok())
            .is_some_and(|t| t == b"Page" || t == b"Pages")
    )
}

/// `id` copied into `dst`, or None for a page that isn't being copied
/// (bug 14: following a link, or a page's `/Parent`, used to copy the
/// whole file it came from).
fn copy_object(
    src: &Pdf,
    id: ObjectId,
    dst: &mut Pdf,
    map: &mut HashMap<ObjectId, ObjectId>,
) -> Result<Option<ObjectId>> {
    if let Some(&m) = map.get(&id) {
        return Ok(Some(m));
    }
    if is_page_node(src, id) {
        return Ok(None);
    }
    let new_id = dst.new_object_id();
    map.insert(id, new_id);
    // Placeholder first so cycles terminate.
    dst.objects.insert(new_id, Object::Null);
    let obj = src.get_object(id)?.clone();
    let obj = remap(src, &obj, dst, map)?;
    dst.objects.insert(new_id, obj);
    Ok(Some(new_id))
}

/// `obj` from `src` in `dst`, copying what it references. A page is never
/// copied this way: one in `map` (being copied too) is referred to there,
/// and any other becomes null, for [`crate::kept::keep_only`] to tidy.
pub(crate) fn remap(
    src: &Pdf,
    obj: &Object,
    dst: &mut Pdf,
    map: &mut HashMap<ObjectId, ObjectId>,
) -> Result<Object> {
    Ok(match obj {
        Object::Reference(r) => match copy_object(src, *r, dst, map)? {
            Some(id) => Object::Reference(id),
            None => Object::Null,
        },
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
pub fn text_string(s: &str) -> Object {
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

/// A bookmark's `/Dest`: its page and, when it has one, its position (see
/// [`DestView`]); otherwise the top of the page.
fn outline_dest(page: ObjectId, view: Option<&DestView>, frame: Option<PageFrame>) -> Vec<Object> {
    let num = |v: Option<f32>| v.map(Object::Real).unwrap_or(Object::Null);
    let mut dest = vec![Object::Reference(page)];
    match view {
        Some(DestView::Xyz { left, top, zoom }) => {
            let (ux, uy) = match frame {
                Some(f) => f.to_user(*left, *top),
                None => (None, None),
            };
            dest.extend(["XYZ".into(), num(ux), num(uy), num(*zoom)]);
        }
        Some(DestView::Other { kind, params }) => {
            dest.push(Object::Name(kind.as_bytes().to_vec()));
            dest.extend(params.iter().map(|p| num(*p)));
        }
        None => dest.extend(["XYZ".into(), Object::Null, Object::Null, Object::Null]),
    }
    dest
}

/// Write the outline tree. `frame` gives a page's geometry (before any
/// rotation added in sheaf) for converting bookmark positions to user space.
pub(crate) fn write_outline(
    pdf: &mut Pdf,
    nodes: &Vector<OutlineNode>,
    page_obj: &HashMap<PageId, ObjectId>,
    frame: &dyn Fn(PageId) -> Option<PageFrame>,
) -> ObjectId {
    fn level(
        pdf: &mut Pdf,
        parent: ObjectId,
        nodes: &Vector<OutlineNode>,
        page_obj: &HashMap<PageId, ObjectId>,
        frame: &dyn Fn(PageId) -> Option<PageFrame>,
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
            if let Some(t) = node.target
                && let Some(obj) = page_obj.get(&t)
            {
                dict.set("Dest", outline_dest(*obj, node.view.as_ref(), frame(t)));
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
            let flags = node.style.italic as i64 | (node.style.bold as i64) << 1;
            if flags != 0 {
                dict.set("F", flags);
            }
            let (first, last, count) = level(pdf, ids[i], &node.children, page_obj, frame);
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
    let (first, last, count) = level(pdf, outlines_id, nodes, page_obj, frame);
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
    let boxed = |key: &[u8]| {
        dict.get(key)
            .ok()
            .and_then(|o| o.as_array().ok())
            .and_then(|a| crate::annot::box_of(a))
    };
    let media = boxed(b"MediaBox").unwrap_or([0.0, 0.0, 612.0, 792.0]);
    PageGeom {
        media: crate::annot::visible_box(media, boxed(b"CropBox")),
        rotate: rotate.rem_euclid(360),
    }
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
        .filter(|e| !crate::file::is_ours(pdf, e))
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
    pub label: String,
    /// Top-left of the marker in its page's user space.
    pub top_left: (f32, f32),
}

/// Stamp pages imported into the file being written, by stamp, so each is
/// in it once (see `annot::to_pdf_in`).
pub(crate) type StampForms = HashMap<String, Option<(ObjectId, sheaf_core::PageSize)>>;

/// What writing a page's marks did besides the page dict.
pub(crate) struct PageAnnotsWritten {
    /// The connectors written, for `link_connectors`.
    pub connectors: Vec<ConnectorRef>,
    /// Objects already in the file that it changed: another tool's replies
    /// to our marks, re-pointed at their new objects (bug 6), and comments
    /// replied to in sheaf that had no `/NM`, given the one the reply uses.
    /// An incremental save writes these again.
    pub touched: Vec<ObjectId>,
}

/// Replace the page's sheaf annotations with `annots`, as new objects with
/// appearance streams. `/P` is left unset; PDFium and Acrobat don't need it.
///
/// Replies and statuses (`Annot::thread_parent`) are written after what
/// they're in the thread of, with `/IRT` to it and its `/Rect`.
pub(crate) fn write_page_annots<'a>(
    pdf: &mut Pdf,
    page: &mut Dictionary,
    annots: impl Iterator<Item = &'a Annot>,
    removed: Option<&im::OrdSet<String>>,
    geom: &PageGeom,
    forms: &mut StampForms,
) -> PageAnnotsWritten {
    // Our marks as the file has them now, by object, so other tools'
    // replies to them can follow them to the objects this writes.
    let mut before: HashMap<ObjectId, String> = page
        .get(b"Annots")
        .ok()
        .and_then(|a| pdf.dereference(a).ok())
        .and_then(|(_, o)| o.as_array().ok().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|e| {
            let id = e.as_reference().ok()?;
            let d = pdf.get_dictionary(id).ok()?;
            (d.has(b"SheafAnnot") && crate::file::is_ours(pdf, e))
                .then(|| (id, crate::file::thread_name(d)))
        })
        .collect();
    strip_sheaf_annots(pdf, page);
    let mut list: Vec<Object> = page
        .get(b"Annots")
        .ok()
        .and_then(|a| pdf.dereference(a).ok())
        .and_then(|(_, o)| o.as_array().ok().cloned())
        .unwrap_or_default();
    // Other tools' comments deleted in sheaf go, with their threads, and
    // so do their replies to our marks that are gone.
    let annots: Vec<&Annot> = annots.collect();
    let writing: std::collections::HashSet<String> = annots.iter().map(|a| a.name()).collect();
    // Another tool's comment that one of our marks now is (made editable
    // in sheaf, or imported over it) gives way to it, with the parts
    // grouped under it (`/RT /Group`, which the mark draws itself); its
    // replies follow it as they follow our marks.
    let mut replaced = std::collections::HashSet::new();
    for e in &list {
        if let Ok(id) = e.as_reference()
            && let Ok(d) = pdf.get_dictionary(id)
            && let name = crate::file::thread_name(d)
            && writing.contains(&name)
        {
            before.insert(id, name);
            replaced.insert(id);
        }
    }
    list.retain(|e| {
        let Ok(id) = e.as_reference() else {
            return true;
        };
        let Ok(d) = pdf.get_dictionary(id) else {
            return true;
        };
        let grouped_under_replaced = d
            .get(b"RT")
            .ok()
            .and_then(|o| o.as_name().ok())
            .is_some_and(|n| n == b"Group")
            && d.get(b"IRT")
                .ok()
                .and_then(|o| o.as_reference().ok())
                .is_some_and(|irt| replaced.contains(&irt));
        !(writing.contains(&crate::file::thread_name(d)) || grouped_under_replaced)
    });
    let mut removed = removed.cloned().unwrap_or_default();
    removed.extend(before.values().filter(|n| !writing.contains(*n)).cloned());
    if !removed.is_empty() {
        let comments: Vec<(String, Option<String>)> = list
            .iter()
            .map(|e| {
                let d = e
                    .as_reference()
                    .ok()
                    .and_then(|id| pdf.get_dictionary(id).ok());
                let name = d.map(crate::file::thread_name).unwrap_or_default();
                let parent = d
                    .and_then(|d| d.get(b"IRT").ok())
                    .and_then(|o| pdf.dereference(o).ok())
                    .and_then(|(_, o)| o.as_dict().ok())
                    .map(crate::file::thread_name);
                (name, parent)
            })
            .collect();
        let gone = crate::file::removed_with_threads(
            comments.iter().map(|(n, p)| (n.as_str(), p.as_deref())),
            &removed,
        );
        let mut kept = comments.iter().map(|(n, _)| !gone.contains(n));
        list.retain(|_| kept.next().unwrap_or(true));
    }
    let theirs: Vec<ObjectId> = list.iter().filter_map(|e| e.as_reference().ok()).collect();
    // Every comment on the page by name: its object and its /Rect.
    let rect_of = |pdf: &Pdf, d: &Dictionary| {
        d.get(b"Rect")
            .ok()
            .and_then(|r| pdf.dereference(r).ok())
            .map(|(_, o)| o.clone())
            .unwrap_or_else(|| Object::Array(vec![0.into(), 0.into(), 0.into(), 0.into()]))
    };
    let mut named: HashMap<String, (ObjectId, Object)> = HashMap::new();
    for &id in &theirs {
        if let Ok(d) = pdf.get_dictionary(id) {
            named.insert(crate::file::thread_name(d), (id, rect_of(pdf, d)));
        }
    }
    let (threads, marks): (Vec<&Annot>, Vec<&Annot>) = annots
        .into_iter()
        .partition(|a| a.thread_parent().is_some());
    let mut connectors = Vec::new();
    for a in marks {
        let written = crate::annot::to_pdf_in(a, geom, pdf, forms);
        let rect = rect_of(pdf, &written.main);
        let id = pdf.add_object(Object::Dictionary(written.main));
        list.push(Object::Reference(id));
        named.insert(a.name(), (id, rect));
        for mut part in written.companions {
            part.set("IRT", id);
            part.set("RT", Object::Name(b"Group".to_vec()));
            part.set(crate::annot::PART, true);
            list.push(Object::Reference(pdf.add_object(Object::Dictionary(part))));
        }
        if let AnnotKind::Connector { rect, label, .. } = &a.kind {
            let [llx, _, _, ury] = geom.box_to_user(*rect);
            connectors.push(ConnectorRef {
                obj: id,
                id: a.id,
                label: label.clone(),
                top_left: (llx, ury),
            });
        }
    }
    // Replies after what they reply to, a reply to a reply after that one;
    // any whose parent isn't on the page, last and unthreaded.
    let wanted: std::collections::HashSet<String> = threads
        .iter()
        .filter_map(|a| a.thread_parent().map(str::to_string))
        .collect();
    let mut pending = threads;
    while !pending.is_empty() {
        let (ready, waiting): (Vec<&Annot>, Vec<&Annot>) = pending
            .iter()
            .partition(|a| named.contains_key(a.thread_parent().unwrap_or_default()));
        let stuck = ready.is_empty();
        for a in if stuck { &waiting } else { &ready } {
            let mut main = crate::annot::to_pdf_in(a, geom, pdf, forms).main;
            if let Some((parent, rect)) = a.thread_parent().and_then(|p| named.get(p)).cloned() {
                main.set("IRT", parent);
                main.set("Rect", rect);
            }
            let rect = rect_of(pdf, &main);
            let id = pdf.add_object(Object::Dictionary(main));
            list.push(Object::Reference(id));
            named.insert(a.name(), (id, rect));
        }
        pending = if stuck { Vec::new() } else { waiting };
    }
    // Other tools' replies to our marks follow them; a comment replied to
    // in sheaf takes the name the reply uses.
    let mut touched = Vec::new();
    for id in theirs {
        let Ok(d) = pdf.get_dictionary(id) else {
            continue;
        };
        let repoint = d
            .get(b"IRT")
            .ok()
            .and_then(|o| o.as_reference().ok())
            .and_then(|irt| before.get(&irt))
            .and_then(|name| named.get(name))
            .map(|(obj, _)| *obj);
        let name = (!d.has(b"NM"))
            .then(|| crate::file::anon_name(d))
            .filter(|n| wanted.contains(n));
        if repoint.is_none() && name.is_none() {
            continue;
        }
        if let Ok(d) = pdf.get_dictionary_mut(id) {
            if let Some(obj) = repoint {
                d.set("IRT", obj);
            }
            if let Some(n) = name {
                d.set("NM", Object::string_literal(n));
            }
            touched.push(id);
        }
    }
    if list.is_empty() {
        page.remove(b"Annots");
    } else {
        page.set("Annots", Object::Array(list));
    }
    PageAnnotsWritten {
        connectors,
        touched,
    }
}

/// Point each written connector's `/Dest` at the next connector with its
/// label, in page order and wrapping, so clicking walks the group in any
/// viewer (M9 decision 8): at its position when it was written in this
/// save, else just its page. A connector alone in its group goes nowhere.
pub(crate) fn link_connectors(
    pdf: &mut Pdf,
    connectors: &[ConnectorRef],
    state: &DocState,
    page_obj: &HashMap<PageId, ObjectId>,
) {
    let mut groups: HashMap<&str, Vec<(usize, PageId, Annot)>> = HashMap::new();
    for c in connectors {
        let members = groups.entry(c.label.as_str()).or_insert_with(|| {
            state.group_members(&crate::annot::Group::Connector(c.label.clone()))
        });
        if members.len() < 2 {
            continue;
        }
        let Some(i) = members.iter().position(|(_, _, a)| a.id == c.id) else {
            continue;
        };
        let (_, next_page, next) = &members[(i + 1) % members.len()];
        let Some(&page_ref) = page_obj.get(next_page) else {
            continue;
        };
        let mut dest = vec![Object::Reference(page_ref), Object::Name(b"XYZ".to_vec())];
        match connectors.iter().find(|g| g.id == next.id) {
            // Land with it a little in from the top-left corner.
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
    use crate::model::{Document, Edit, FlatOutlineItem};
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
            data: None,
            derived: None,
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

    /// Each bookmark's `/Dest` after its page reference, as text
    /// ("/XYZ 36 692 null"), in written order.
    fn read_dests(bytes: &[u8]) -> Vec<String> {
        let pdf = Pdf::load_mem(bytes).unwrap();
        let root = pdf.trailer.get(b"Root").unwrap().as_reference().unwrap();
        let outlines = pdf
            .get_dictionary(root)
            .unwrap()
            .get(b"Outlines")
            .unwrap()
            .as_reference()
            .unwrap();
        let mut cur = pdf
            .get_dictionary(outlines)
            .unwrap()
            .get(b"First")
            .ok()
            .and_then(|o| o.as_reference().ok());
        let mut out = Vec::new();
        while let Some(id) = cur {
            let d = pdf.get_dictionary(id).unwrap();
            let dest = d.get(b"Dest").unwrap().as_array().unwrap();
            let parts: Vec<String> = dest[1..]
                .iter()
                .map(|o| match o {
                    Object::Name(n) => format!("/{}", String::from_utf8_lossy(n)),
                    Object::Null => "null".into(),
                    o => format!("{}", o.as_float().unwrap()),
                })
                .collect();
            out.push(parts.join(" "));
            cur = d.get(b"Next").ok().and_then(|o| o.as_reference().ok());
        }
        out
    }

    #[test]
    fn bookmark_positions_survive_saves_and_our_rotation() {
        let dir = tempdir();
        let a = make_pdf(&dir, "a.pdf", "A", 2, &[]);
        let item = |title: &str, page, view| FlatOutlineItem {
            title: title.into(),
            page: Some(page),
            depth: 0,
            view,
            color: None,
            style: Default::default(),
        };
        let mut model = Document::new(
            info(&a, 2),
            &[
                item(
                    "At a spot",
                    0,
                    Some(DestView::Xyz {
                        left: Some(36.0),
                        top: Some(100.0),
                        zoom: None,
                    }),
                ),
                item(
                    "Fit width",
                    1,
                    Some(DestView::Other {
                        kind: "FitH".into(),
                        params: vec![Some(500.0)],
                    }),
                ),
                item("Page top", 1, None),
            ],
            Box::new(FixedCount(10)),
        );
        // Turning the page in sheaf mustn't move the stored position: it's
        // relative to the page as the file draws it.
        let p0 = model.page(0).unwrap().id;
        model
            .apply(
                Edit::RotatePages {
                    ids: vec![p0],
                    by: Rotation::R90,
                },
                "rotate",
            )
            .unwrap();
        let want = ["/XYZ 36 692 null", "/FitH 500", "/XYZ null null null"];
        assert_eq!(read_dests(&rewrite(&model.snapshot()).unwrap()), want);
        // The incremental path writes the outline when it changed.
        let p1 = model.page(1).unwrap().id;
        model
            .apply(
                Edit::OutlineInsert {
                    parent: None,
                    after: None,
                    title: "New".into(),
                    target: Some(p1),
                    view: Some(DestView::Xyz {
                        left: None,
                        top: Some(0.0),
                        zoom: Some(1.5),
                    }),
                },
                "add",
            )
            .unwrap();
        let mut want_inc = vec!["/XYZ null 792 1.5"];
        want_inc.extend(want);
        let bytes = crate::incremental::incremental(&model.snapshot()).unwrap();
        assert_eq!(read_dests(&bytes), want_inc);
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
                    view: None,
                    color: None,
                    style: Default::default(),
                },
                FlatOutlineItem {
                    title: "Three".into(),
                    page: Some(2),
                    depth: 0,
                    view: None,
                    color: None,
                    style: Default::default(),
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
                    view: None,
                    color: None,
                    style: Default::default(),
                },
                FlatOutlineItem {
                    title: "Child é".into(),
                    page: Some(2),
                    depth: 1,
                    view: None,
                    color: None,
                    style: Default::default(),
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
                    annots: Vec::new(),
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
                view: None,
                color: None,
                style: Default::default(),
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
                    annots: Vec::new(),
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
                    view: None,
                    style: None,
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

        // A page was deleted: saving the primary path is a full rewrite,
        // so the page isn't left in an earlier revision (bug 14).
        assert!(model.snapshot().removes_content);
        assert_eq!(
            save_to_path(&model.snapshot(), &a).unwrap(),
            Strategy::Rewrite
        );
        let saved = std::fs::read(&a).unwrap();
        assert!(!saved.starts_with(&original));
        assert!(!streams_say(&saved, "MARK A 0") && streams_say(&saved, "MARK A 1"));
        assert_eq!(
            save_to_path(&model.snapshot(), &dir.join("elsewhere.pdf")).unwrap(),
            Strategy::Rewrite
        );
    }

    /// Whether any stream in the file says `text`, decompressed.
    fn streams_say(bytes: &[u8], text: &str) -> bool {
        let pdf = Pdf::load_mem(bytes).unwrap();
        pdf.objects.values().any(|o| match o {
            Object::Stream(s) => {
                let data = s
                    .decompressed_content()
                    .unwrap_or_else(|_| s.content.clone());
                String::from_utf8_lossy(&data).contains(text)
            }
            _ => false,
        })
    }

    #[test]
    fn incremental_classic_xref() {
        incremental_case(false);
    }

    #[test]
    fn incremental_xref_stream() {
        incremental_case(true);
    }

    /// `path`'s bytes, written with a cross-reference table and a trailer.
    fn classic(path: &Path) -> Vec<u8> {
        let mut pdf = Pdf::load(path).unwrap();
        pdf.reference_table.cross_reference_type = lopdf::xref::XrefType::CrossReferenceTable;
        let mut bytes = Vec::new();
        pdf.save_to(&mut bytes).unwrap();
        bytes
    }

    /// `bytes` with `keys` written into its trailer, as lopdf writes one.
    fn with_trailer_keys(bytes: &[u8], keys: &str) -> Vec<u8> {
        let open = b"trailer\n<<";
        let at = bytes.windows(open.len()).rposition(|w| w == open).unwrap();
        let at = at + open.len();
        [&bytes[..at], keys.as_bytes(), &bytes[at..]].concat()
    }

    /// A one-page model of `path`, its page turned a quarter.
    fn rotated(path: &Path) -> Document {
        let mut i = info(path, 1);
        i.bytes = std::fs::metadata(path).unwrap().len() as usize;
        let mut model = Document::new(i, &[], Box::new(FixedCount(10)));
        let page = model.state().pages[0].id;
        let rotate = Edit::RotatePages {
            ids: vec![page],
            by: Rotation::R90,
        };
        model.apply(rotate, "rot").unwrap();
        model
    }

    #[test]
    fn a_stale_xrefstm_is_ignored() {
        // lopdf 0.45 kept a one-section hybrid-reference file's /XRefStm
        // when it rewrote it, pointing past the end of the new file.
        let dir = tempdir();
        let a = make_pdf(&dir, "a.pdf", "A", 1, &[]);
        let bytes = classic(&a);
        let table = Pdf::load_mem(&bytes).unwrap().xref_start;
        let stale = format!("/XRefStm {}", bytes.len() + 5000);
        std::fs::write(&a, with_trailer_keys(&bytes, &stale)).unwrap();
        let original = std::fs::read(&a).unwrap();
        let model = rotated(&a);

        // The update chains to the table, and a rewrite leaves the key out.
        let saved = crate::incremental::incremental(&model.snapshot()).unwrap();
        let update = String::from_utf8_lossy(&saved[original.len()..]);
        let prev = update.split("/Prev ").nth(1).unwrap();
        let prev = prev.split(|c: char| !c.is_ascii_digit()).next().unwrap();
        assert_eq!(prev.parse::<usize>().unwrap(), table);
        assert_eq!(read_pages(&saved), [("A 0".into(), 90)]);
        let rewritten = rewrite(&model.snapshot()).unwrap();
        assert!(!rewritten.windows(8).any(|w| w == b"/XRefStm"));
    }

    #[test]
    fn a_file_lopdf_rebuilds_is_rewritten() {
        // Its startxref points past the end, so lopdf rebuilds its
        // cross-references by scanning, and keeps its trailer's keys.
        let dir = tempdir();
        let a = make_pdf(&dir, "a.pdf", "A", 1, &[]);
        let mut bytes = with_trailer_keys(&classic(&a), "/Prev 9 /XRefStm 9");
        let end = bytes.windows(9).rposition(|w| w == b"startxref").unwrap();
        bytes.truncate(end);
        let past = bytes.len() + 5000;
        bytes.extend_from_slice(format!("startxref\n{past}\n%%EOF\n").as_bytes());
        std::fs::write(&a, &bytes).unwrap();
        assert_eq!(Pdf::load_mem(&bytes).unwrap().xref_start, 0);
        let model = rotated(&a);

        // An update would have no section for /Prev to point at.
        assert_eq!(
            save_to_path(&model.snapshot(), &a).unwrap(),
            Strategy::Rewrite
        );
        let saved = std::fs::read(&a).unwrap();
        assert_ne!(Pdf::load_mem(&saved).unwrap().xref_start, 0);
        assert_eq!(read_pages(&saved), [("A 0".into(), 90)]);
        let has = |key: &[u8]| saved.windows(key.len()).any(|w| w == key);
        assert!(!has(b"/Prev") && !has(b"/XRefStm"));
    }

    /// Put a mark of a kind no build knows on page 0 of `path`, as a newer
    /// sheaf would have written it.
    fn plant_unknown_mark(path: &Path) {
        let mut pdf = Pdf::load(path).unwrap();
        let page0 = pdf.get_pages()[&1];
        let json = r#"{"id":77,"kind":{"Hexagon":{"rect":{"x":1,"y":1,"w":5,"h":5}}},"color":[1,2,3],"opacity":1.0,"stroke":1.0,"author":"future","contents":"from a newer sheaf","modified":""}"#;
        let ap = pdf.add_object(lopdf::Stream::new(
            dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => vec![0.into(), 0.into(), 60.into(), 60.into()] },
            b"0 0 60 60 re S".to_vec(),
        ));
        let annot = pdf.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Polygon",
            "Rect" => vec![100.into(), 600.into(), 160.into(), 660.into()],
            "NM" => Object::string_literal("sheaf:77"),
            "Contents" => Object::string_literal("from a newer sheaf"),
            "SheafAnnot" => Object::string_literal(json),
            "AP" => dictionary! { "N" => ap },
        });
        pdf.get_dictionary_mut(page0)
            .unwrap()
            .set("Annots", vec![annot.into()]);
        pdf.save(path).unwrap();
    }

    #[test]
    fn a_mark_this_build_cant_read_survives_a_save() {
        let dir = tempdir();
        let path = make_pdf(&dir, "a.pdf", "A", 2, &[]);
        plant_unknown_mark(&path);
        let found = crate::file::read_annots(&path, None).unwrap();
        assert!(found.sheaf.is_empty());
        assert_eq!(found.foreign.len(), 1);
        assert_eq!(found.foreign[0].subtype, "Polygon");
        for incremental in [true, false] {
            let len = std::fs::metadata(&path).unwrap().len() as usize;
            let mut model = Document::new(
                {
                    let mut i = info(&path, 2);
                    i.bytes = len;
                    i
                },
                &[],
                Box::new(FixedCount(10)),
            );
            let p0 = model.page(0).unwrap().id;
            let ours = Annot {
                id: AnnotId(0),
                kind: AnnotKind::Square {
                    rect: crate::annot::Box2 {
                        x: 10.0,
                        y: 10.0,
                        w: 20.0,
                        h: 20.0,
                    },
                },
                color: [255, 0, 0],
                opacity: 1.0,
                stroke: 1.0,
                author: "t".into(),
                contents: String::new(),
                modified: String::new(),
                created: String::new(),
                fill: None,
                dashed: false,
                cloudy: false,
                name: String::new(),
            };
            model
                .apply(
                    Edit::AnnotAdd {
                        page: p0,
                        annot: ours,
                    },
                    "add",
                )
                .unwrap();
            let bytes = if incremental {
                crate::incremental::incremental(&model.snapshot()).unwrap()
            } else {
                rewrite(&model.snapshot()).unwrap()
            };
            let out = dir.join(format!("out-{incremental}.pdf"));
            std::fs::write(&out, &bytes).unwrap();
            let back = crate::file::read_annots(&out, None).unwrap();
            assert_eq!(back.sheaf[&0].len(), 1, "ours, once ({incremental})");
            assert_eq!(
                back.foreign.len(),
                1,
                "the unknown mark, kept ({incremental})"
            );
            assert_eq!(back.foreign[0].contents, "from a newer sheaf");
        }
    }

    #[test]
    fn bookmark_colors_and_styles_round_trip() {
        let dir = tempdir();
        let path = make_pdf(
            &dir,
            "a.pdf",
            "A",
            3,
            &[("Red", 0, 0), ("Plain", 1, 0), ("Both", 2, 0)],
        );
        // Colored and styled as another tool (PDFlyer, Acrobat) would.
        {
            let mut pdf = Pdf::load(&path).unwrap();
            let first = pdf
                .catalog()
                .unwrap()
                .get(b"Outlines")
                .unwrap()
                .as_reference()
                .unwrap();
            let first = pdf
                .get_dictionary(first)
                .unwrap()
                .get(b"First")
                .unwrap()
                .as_reference()
                .unwrap();
            let second = pdf
                .get_dictionary(first)
                .unwrap()
                .get(b"Next")
                .unwrap()
                .as_reference()
                .unwrap();
            let third = pdf
                .get_dictionary(second)
                .unwrap()
                .get(b"Next")
                .unwrap()
                .as_reference()
                .unwrap();
            pdf.get_dictionary_mut(first)
                .unwrap()
                .set("C", vec![1.0.into(), 0.0.into(), 0.0.into()]);
            pdf.get_dictionary_mut(third).unwrap().set("F", 3);
            pdf.save(&path).unwrap();
        }
        let styles = crate::file::outline_styles(&Pdf::load(&path).unwrap());
        assert_eq!(styles.len(), 3);
        // As PDFium hands them over: no colors or styles.
        let mut items: Vec<FlatOutlineItem> = ["Red", "Plain", "Both"]
            .iter()
            .enumerate()
            .map(|(i, t)| FlatOutlineItem {
                title: t.to_string(),
                page: Some(i as u32),
                depth: 0,
                view: None,
                color: None,
                style: Default::default(),
            })
            .collect();
        crate::file::style_flat_items(&mut items, &styles);
        assert_eq!(items[0].color, Some([255, 0, 0]));
        assert!(items[1].color.is_none() && items[1].style.is_plain());
        assert!(items[2].style.bold && items[2].style.italic);
        for incremental in [true, false] {
            let len = std::fs::metadata(&path).unwrap().len() as usize;
            let mut model = Document::new(
                {
                    let mut i = info(&path, 3);
                    i.bytes = len;
                    i
                },
                &items,
                Box::new(FixedCount(10)),
            );
            // A bookmark edit, so the incremental save rewrites the outline.
            let plain = model.state().outline[1].id;
            model
                .apply(
                    Edit::OutlineEdit {
                        id: plain,
                        title: Some("Renamed".into()),
                        target: None,
                        color: None,
                        view: None,
                        style: None,
                    },
                    "rename",
                )
                .unwrap();
            let bytes = if incremental {
                crate::incremental::incremental(&model.snapshot()).unwrap()
            } else {
                rewrite(&model.snapshot()).unwrap()
            };
            let back = crate::file::outline_styles(&Pdf::load_mem(&bytes).unwrap());
            assert_eq!(back.len(), 3, "{incremental}");
            assert_eq!(back[0].color, Some([255, 0, 0]), "{incremental}");
            assert_eq!(back[1].title, "Renamed");
            assert!(back[2].style.bold && back[2].style.italic, "{incremental}");
        }
    }

    /// A source as the app makes one: its bytes kept from when it opened.
    fn kept(path: &Path, n: u32) -> SourceInfo {
        let bytes = std::fs::read(path).unwrap();
        SourceInfo {
            bytes: bytes.len(),
            data: Some(crate::model::SourceData(std::sync::Arc::new(bytes))),
            derived: None,
            ..info(path, n)
        }
    }

    /// Bug 13 (todos/m9.md): after a save in place, a version from before
    /// it still means the old file, whatever the path holds now.
    #[test]
    fn undo_past_a_save_then_save_writes_the_right_pages() {
        for incremental_first in [true, false] {
            let dir = tempdir();
            let a = make_pdf(&dir, "a.pdf", "A", 3, &[]);
            let mut model = Document::new(kept(&a, 3), &[], Box::new(FixedCount(10)));
            let ids: Vec<PageId> = model.state().pages.iter().map(|p| p.id).collect();
            model
                .apply(
                    Edit::MovePages {
                        ids: vec![ids[2]],
                        before: Some(ids[0]),
                    },
                    "move",
                )
                .unwrap();
            if incremental_first {
                save_to_path(&model.snapshot(), &a).unwrap();
            } else {
                rewrite_to_path(&model.snapshot(), &a).unwrap();
            }
            let saved = read_pages(&std::fs::read(&a).unwrap());
            assert_eq!(saved[0].0, "A 2");
            model.rebase_to_saved(kept(&a, 3));
            model.undo();
            save_to_path(&model.snapshot(), &a).unwrap();
            let pages: Vec<String> = read_pages(&std::fs::read(&a).unwrap())
                .into_iter()
                .map(|(t, _)| t)
                .collect();
            assert_eq!(pages, vec!["A 0", "A 1", "A 2"], "{incremental_first}");
        }
    }

    /// A note of ours as the thread tests use it.
    fn thread_mark(kind: crate::annot::AnnotKind, contents: &str) -> Annot {
        Annot {
            id: crate::annot::AnnotId(0),
            kind,
            color: [255, 200, 0],
            opacity: 1.0,
            stroke: 1.0,
            author: "Greg".into(),
            contents: contents.into(),
            modified: "D:20261014000000".into(),
            created: "D:20261014000000".into(),
            fill: None,
            dashed: false,
            cloudy: false,
            name: String::new(),
        }
    }

    /// Page 1's annotations: (object, dict), in /Annots order.
    fn page_annots(bytes: &[u8]) -> Vec<(ObjectId, Dictionary)> {
        let pdf = Pdf::load_mem(bytes).unwrap();
        let page0 = pdf.get_pages()[&1];
        pdf.get_dictionary(page0)
            .unwrap()
            .get(b"Annots")
            .map(|a| a.as_array().unwrap().clone())
            .unwrap_or_default()
            .iter()
            .map(|o| {
                let id = o.as_reference().unwrap();
                (id, pdf.get_dictionary(id).unwrap().clone())
            })
            .collect()
    }

    fn opened(path: &Path, bytes: usize) -> Document {
        Document::new(
            {
                let mut i = info(path, 1);
                i.bytes = bytes;
                i
            },
            &[],
            Box::new(FixedCount(10)),
        )
    }

    #[test]
    fn replies_and_statuses_thread_under_their_comment() {
        use crate::annot::{AnnotKind, Pt};
        let dir = tempdir();
        let a_path = make_pdf(&dir, "a.pdf", "A", 1, &[]);
        let original = std::fs::read(&a_path).unwrap();
        let mut model = opened(&a_path, original.len());
        let p0 = model.page(0).unwrap().id;
        let note = thread_mark(
            AnnotKind::Note {
                pos: Pt { x: 100.0, y: 100.0 },
            },
            "Tie to the K-1",
        );
        model
            .apply(
                Edit::AnnotAdd {
                    page: p0,
                    annot: note,
                },
                "note",
            )
            .unwrap();
        let parent = model.state().annots_on(p0).unwrap()[0].name();
        for (kind, text) in [
            (
                AnnotKind::Reply {
                    parent: parent.clone(),
                },
                "Tied.",
            ),
            (
                AnnotKind::Status {
                    parent: parent.clone(),
                    model: crate::annot::REVIEW.into(),
                    state: "Accepted".into(),
                },
                "Accepted set by Greg",
            ),
        ] {
            model
                .apply(
                    Edit::AnnotAdd {
                        page: p0,
                        annot: thread_mark(kind, text),
                    },
                    "thread",
                )
                .unwrap();
        }
        let want: Vec<Annot> = model
            .state()
            .annots_on(p0)
            .unwrap()
            .iter()
            .cloned()
            .collect();
        let bytes = crate::incremental::incremental(&model.snapshot()).unwrap();
        let list = page_annots(&bytes);
        assert_eq!(list.len(), 3);
        let (note_id, note_dict) = &list[0];
        for (_, d) in &list[1..] {
            assert_eq!(d.get(b"Subtype").unwrap().as_name().unwrap(), b"Text");
            assert_eq!(d.get(b"IRT").unwrap().as_reference().unwrap(), *note_id);
            assert_eq!(d.get(b"Rect").unwrap(), note_dict.get(b"Rect").unwrap());
        }
        let status = &list[2].1;
        assert_eq!(status.get(b"State").unwrap().as_str().unwrap(), b"Accepted");
        assert_eq!(
            status.get(b"StateModel").unwrap().as_str().unwrap(),
            b"Review"
        );
        // Hidden, as Acrobat writes a state.
        assert_eq!(status.get(b"F").unwrap().as_i64().unwrap() & 2, 2);
        assert_eq!(
            crate::file::read_sheaf_annots_mem(&bytes).unwrap()[&0],
            want
        );

        // Deleting the note takes its thread.
        let id = want[0].id;
        model
            .apply(Edit::AnnotDelete { page: p0, id }, "rm")
            .unwrap();
        assert!(model.state().annots_on(p0).is_none());
    }

    /// A one-page file with a note of ours and, as Acrobat leaves them, a
    /// reply to it and a comment of its own with no /NM.
    fn file_with_their_comments(dir: &Path) -> (std::path::PathBuf, Vec<Annot>) {
        use crate::annot::{AnnotKind, Pt};
        let a_path = make_pdf(dir, "a.pdf", "A", 1, &[]);
        let original = std::fs::read(&a_path).unwrap();
        let mut model = opened(&a_path, original.len());
        let p0 = model.page(0).unwrap().id;
        let note = thread_mark(
            AnnotKind::Note {
                pos: Pt { x: 100.0, y: 100.0 },
            },
            "ours",
        );
        model
            .apply(
                Edit::AnnotAdd {
                    page: p0,
                    annot: note,
                },
                "note",
            )
            .unwrap();
        let ours: Vec<Annot> = model
            .state()
            .annots_on(p0)
            .unwrap()
            .iter()
            .cloned()
            .collect();
        let bytes = crate::incremental::incremental(&model.snapshot()).unwrap();
        let mut pdf = Pdf::load_mem(&bytes).unwrap();
        let page0 = pdf.get_pages()[&1];
        let note_obj = page_annots(&bytes)[0].0;
        let rect = || Object::Array(vec![300.into(), 300.into(), 320.into(), 320.into()]);
        let reply = pdf.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Text", "Rect" => rect(),
            "IRT" => note_obj, "NM" => Object::string_literal("acro-reply"),
            "T" => Object::string_literal("Pat"), "Contents" => Object::string_literal("Agreed"),
        });
        let theirs = pdf.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Text", "Rect" => rect(),
            "T" => Object::string_literal("Pat"), "Contents" => Object::string_literal("No name"),
        });
        let mut annots = pdf
            .get_dictionary(page0)
            .unwrap()
            .get(b"Annots")
            .unwrap()
            .as_array()
            .unwrap()
            .clone();
        annots.extend([Object::Reference(reply), Object::Reference(theirs)]);
        pdf.get_dictionary_mut(page0)
            .unwrap()
            .set("Annots", Object::Array(annots));
        let path = dir.join("theirs.pdf");
        pdf.save(&path).unwrap();
        (path, ours)
    }

    #[test]
    fn their_replies_follow_our_marks_across_saves() {
        // Bug 6: every save rewrites our marks as new objects.
        let dir = tempdir();
        let (path, ours) = file_with_their_comments(&dir);
        let found = crate::file::read_annots(&path, None).unwrap();
        let reply = found
            .foreign
            .iter()
            .find(|f| f.name == "acro-reply")
            .unwrap();
        assert_eq!(reply.parent.as_deref(), Some(ours[0].name().as_str()));

        for incremental in [true, false] {
            let bytes = std::fs::read(&path).unwrap();
            let mut model = opened(&path, bytes.len());
            model.set_initial_annots(found.sheaf.clone()).unwrap();
            let p0 = model.page(0).unwrap().id;
            let mut note = model.state().annots_on(p0).unwrap()[0].clone();
            note.contents = "ours, edited".into();
            model
                .apply(
                    Edit::AnnotUpdate {
                        page: p0,
                        annot: note,
                    },
                    "edit",
                )
                .unwrap();
            let out = if incremental {
                crate::incremental::incremental(&model.snapshot()).unwrap()
            } else {
                rewrite(&model.snapshot()).unwrap()
            };
            let list = page_annots(&out);
            let note_obj = list
                .iter()
                .find(|(_, d)| d.has(b"SheafAnnot"))
                .map(|(id, _)| *id)
                .unwrap();
            let (_, reply) = list
                .iter()
                .find(|(_, d)| {
                    d.get(b"NM").ok().map(crate::file::pdf_text).as_deref() == Some("acro-reply")
                })
                .unwrap();
            assert_eq!(
                reply.get(b"IRT").unwrap().as_reference().unwrap(),
                note_obj,
                "incremental {incremental}"
            );
            // Read back from what was saved, it's still in the note's thread.
            let again = crate::file::read_annots_mem(&out).unwrap();
            let reply = again
                .foreign
                .iter()
                .find(|f| f.name == "acro-reply")
                .unwrap();
            assert_eq!(
                reply.parent.as_deref(),
                Some(ours[0].name().as_str()),
                "incremental {incremental}"
            );
        }
    }

    /// A page with another tool's square ("sq", with Pat's reply), and an
    /// Acrobat replace-text edit (Caret "c1", its StrikeOut grouped under it).
    fn file_with_their_shapes(dir: &Path) -> std::path::PathBuf {
        let a_path = make_pdf(dir, "a.pdf", "A", 1, &[]);
        let mut pdf = Pdf::load(&a_path).unwrap();
        let page0 = pdf.get_pages()[&1];
        let square = pdf.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Square",
            "Rect" => vec![100.into(), 100.into(), 200.into(), 150.into()],
            "NM" => Object::string_literal("sq"), "T" => Object::string_literal("Pat"),
            "C" => vec![0.into(), 0.into(), 1.into()], "BS" => dictionary! { "W" => 2 },
        });
        let reply = pdf.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Text",
            "Rect" => vec![100.into(), 100.into(), 120.into(), 120.into()],
            "IRT" => square, "NM" => Object::string_literal("sq-reply"),
            "T" => Object::string_literal("Pat"), "Contents" => Object::string_literal("Why?"),
        });
        let caret = pdf.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Caret",
            "Rect" => vec![300.into(), 300.into(), 306.into(), 312.into()],
            "NM" => Object::string_literal("c1"), "Contents" => Object::string_literal("the"),
        });
        let strike = pdf.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "StrikeOut",
            "Rect" => vec![306.into(), 300.into(), 330.into(), 312.into()],
            "QuadPoints" => vec![306.into(), 312.into(), 330.into(), 312.into(), 306.into(), 300.into(), 330.into(), 300.into()],
            "IRT" => caret, "RT" => "Group", "NM" => Object::string_literal("c1-strike"),
        });
        pdf.get_dictionary_mut(page0).unwrap().set(
            "Annots",
            vec![square.into(), reply.into(), caret.into(), strike.into()],
        );
        let path = dir.join("shapes.pdf");
        pdf.save(&path).unwrap();
        path
    }

    #[test]
    fn a_comment_made_ours_takes_its_place_and_keeps_its_replies() {
        let dir = tempdir();
        let path = file_with_their_shapes(&dir);
        let found = crate::file::read_annots(&path, None).unwrap();
        let mark = |name: &str| {
            *found
                .foreign
                .iter()
                .find(|f| f.name == name)
                .unwrap()
                .mark
                .clone()
                .unwrap()
        };
        let mut square = mark("sq");
        assert_eq!(square.color, [0, 0, 255]);
        square.kind = square.kind.translated(10.0, 0.0);
        let replace = mark("c1");
        assert!(matches!(
            replace.kind,
            crate::annot::AnnotKind::Replace { .. }
        ));
        // The strikeout is part of the replacement, not a mark of its own.
        assert!(
            found
                .foreign
                .iter()
                .find(|f| f.name == "c1-strike")
                .unwrap()
                .mark
                .is_none()
        );
        for incremental in [true, false] {
            let bytes = std::fs::read(&path).unwrap();
            let mut model = opened(&path, bytes.len());
            let p0 = model.page(0).unwrap().id;
            for annot in [square.clone(), replace.clone()] {
                model
                    .apply(Edit::AnnotAdd { page: p0, annot }, "adopt")
                    .unwrap();
            }
            let out = if incremental {
                crate::incremental::incremental(&model.snapshot()).unwrap()
            } else {
                rewrite(&model.snapshot()).unwrap()
            };
            let list = page_annots(&out);
            let subtypes: Vec<String> = list
                .iter()
                .map(|(_, d)| crate::import::subtype(d))
                .collect();
            // Theirs are gone; ours (a Caret with its own StrikeOut) are
            // there instead, and Pat's reply.
            assert_eq!(
                subtypes.iter().filter(|s| *s == "Square").count(),
                1,
                "{subtypes:?}"
            );
            assert_eq!(subtypes.iter().filter(|s| *s == "StrikeOut").count(), 1);
            let (sq_obj, sq) = list
                .iter()
                .find(|(_, d)| crate::import::subtype(d) == "Square")
                .unwrap();
            assert!(sq.has(b"SheafAnnot"));
            assert_eq!(crate::file::thread_name(sq), "sq");
            let (_, reply) = list
                .iter()
                .find(|(_, d)| crate::file::thread_name(d) == "sq-reply")
                .unwrap();
            assert_eq!(reply.get(b"IRT").unwrap().as_reference().unwrap(), *sq_obj);
            // Read back, the square is ours, named as it was, and the reply
            // is still in its thread.
            let again = crate::file::read_annots_mem(&out).unwrap();
            let ours = &again.sheaf[&0];
            assert!(ours.iter().any(|a| a.name == "sq" && a.kind == square.kind));
            assert!(ours.iter().any(|a| a.name == "c1"));
            let names: Vec<(&str, Option<&str>)> = again
                .foreign
                .iter()
                .map(|f| (f.name.as_str(), f.parent.as_deref()))
                .collect();
            assert_eq!(
                names,
                [("sq-reply", Some("sq"))],
                "incremental {incremental}"
            );
        }
    }

    #[test]
    fn deleted_comments_and_their_threads_stay_out_of_the_file() {
        let dir = tempdir();
        let (path, ours) = file_with_their_comments(&dir);
        let found = crate::file::read_annots(&path, None).unwrap();
        let unnamed = found
            .foreign
            .iter()
            .find(|f| f.contents == "No name")
            .unwrap();
        let contents = |bytes: &[u8]| -> Vec<String> {
            page_annots(bytes)
                .iter()
                .map(|(_, d)| {
                    d.get(b"Contents")
                        .map(crate::file::pdf_text)
                        .unwrap_or_default()
                })
                .collect()
        };
        for incremental in [true, false] {
            let bytes = std::fs::read(&path).unwrap();
            let mut model = opened(&path, bytes.len());
            model.set_initial_annots(found.sheaf.clone()).unwrap();
            let p0 = model.page(0).unwrap().id;
            // Theirs, by name.
            model
                .apply(
                    Edit::CommentsDelete {
                        page: p0,
                        names: vec![unnamed.name.clone()],
                    },
                    "delete theirs",
                )
                .unwrap();
            assert!(model.snapshot().annots_dirty.contains(&p0));
            // Ours: Acrobat's reply to it goes too.
            model
                .apply(
                    Edit::AnnotDelete {
                        page: p0,
                        id: ours[0].id,
                    },
                    "delete ours",
                )
                .unwrap();
            let save = |m: &Document| {
                if incremental {
                    crate::incremental::incremental(&m.snapshot()).unwrap()
                } else {
                    rewrite(&m.snapshot()).unwrap()
                }
            };
            assert!(
                contents(&save(&model)).is_empty(),
                "incremental {incremental}"
            );
            // Undone, they're all written again.
            model.undo().unwrap();
            model.undo().unwrap();
            let mut back = contents(&save(&model));
            back.sort();
            assert_eq!(
                back,
                ["Agreed", "No name", "ours"],
                "incremental {incremental}"
            );
        }
    }

    #[test]
    fn a_reply_names_a_comment_that_had_no_name() {
        use crate::annot::AnnotKind;
        let dir = tempdir();
        let (path, _) = file_with_their_comments(&dir);
        let found = crate::file::read_annots(&path, None).unwrap();
        let unnamed = found
            .foreign
            .iter()
            .find(|f| f.contents == "No name")
            .unwrap();
        assert!(unnamed.name.starts_with("sheaf-anon:"));
        // The same name every time it's read.
        assert_eq!(
            crate::file::read_annots(&path, None).unwrap().foreign,
            found.foreign
        );
        let bytes = std::fs::read(&path).unwrap();
        let mut model = opened(&path, bytes.len());
        model.set_initial_annots(found.sheaf.clone()).unwrap();
        let p0 = model.page(0).unwrap().id;
        model
            .apply(
                Edit::AnnotAdd {
                    page: p0,
                    annot: thread_mark(
                        AnnotKind::Reply {
                            parent: unnamed.name.clone(),
                        },
                        "Answered",
                    ),
                },
                "reply",
            )
            .unwrap();
        let out = crate::incremental::incremental(&model.snapshot()).unwrap();
        let list = page_annots(&out);
        let (theirs_id, theirs) = list
            .iter()
            .find(|(_, d)| {
                d.get(b"Contents")
                    .ok()
                    .map(crate::file::pdf_text)
                    .as_deref()
                    == Some("No name")
            })
            .unwrap();
        assert_eq!(
            crate::file::pdf_text(theirs.get(b"NM").unwrap()),
            unnamed.name
        );
        let (_, reply) = list
            .iter()
            .find(|(_, d)| {
                d.get(b"Contents")
                    .ok()
                    .map(crate::file::pdf_text)
                    .as_deref()
                    == Some("Answered")
            })
            .unwrap();
        assert_eq!(
            reply.get(b"IRT").unwrap().as_reference().unwrap(),
            *theirs_id
        );
        // Read back, the reply is in its thread and theirs keeps the name.
        let back = crate::file::read_annots_mem(&out).unwrap();
        assert!(back.foreign.iter().any(|f| f.name == unnamed.name));
    }

    #[test]
    fn an_attachment_is_embedded_and_recovered() {
        // Named as sheaf names files now, and as it did before xxh3 (a
        // mark saved then keeps that name).
        for legacy in [false, true] {
            attachment_round_trip(legacy);
        }
    }

    fn attachment_round_trip(legacy: bool) {
        use crate::annot::{AnnotKind, Pt};
        let bytes = format!("workpaper notes {:x}", fastrand::u64(..)).into_bytes();
        let stem = if legacy {
            let stem = crate::generated::legacy_name_of(&bytes);
            crate::generated::store_blob_as(&stem, &bytes).unwrap();
            stem
        } else {
            crate::generated::store_blob(&bytes).unwrap()
        };
        let dir = tempdir();
        let a_path = make_pdf(&dir, "a.pdf", "A", 1, &[]);
        let original = std::fs::read(&a_path).unwrap();
        let mut model = opened(&a_path, original.len());
        let p0 = model.page(0).unwrap().id;
        model
            .apply(
                Edit::AnnotAdd {
                    page: p0,
                    annot: thread_mark(
                        AnnotKind::FileAttachment {
                            pos: Pt { x: 50.0, y: 50.0 },
                            icon: "Paperclip".into(),
                            file: stem.clone(),
                            name: "notes.txt".into(),
                            size: bytes.len() as u64,
                        },
                        "",
                    ),
                },
                "attach",
            )
            .unwrap();
        let out = crate::incremental::incremental(&model.snapshot()).unwrap();
        let pdf = Pdf::load_mem(&out).unwrap();
        let (_, d) = &page_annots(&out)[0];
        assert_eq!(
            d.get(b"Subtype").unwrap().as_name().unwrap(),
            b"FileAttachment"
        );
        assert_eq!(d.get(b"Name").unwrap().as_name().unwrap(), b"Paperclip");
        // The file is in it, byte for byte; ours aren't listed twice.
        let embedded = pdf
            .dereference(
                d.get(b"FS")
                    .unwrap()
                    .as_dict()
                    .unwrap()
                    .get(b"EF")
                    .unwrap()
                    .as_dict()
                    .unwrap()
                    .get(b"F")
                    .unwrap(),
            )
            .unwrap()
            .1
            .as_stream()
            .unwrap()
            .decompressed_content()
            .unwrap();
        assert_eq!(embedded, bytes);
        assert!(crate::attachments::attachments_in(&pdf).is_empty());
        // Elsewhere, without the cache: back from the file.
        let path = crate::generated::blob_path(&stem).unwrap();
        std::fs::remove_file(&path).unwrap();
        crate::file::recover_cached(&pdf);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_page_stamp_is_written_once_and_recovered_from_the_file() {
        use crate::annot::{AnnotKind, Box2};
        // A stamp page of its own, unlike any other test's.
        let stamp_pdf = {
            let mut out = Pdf::with_version("1.5");
            let pages_id = out.new_object_id();
            let tag = format!("{:x}", fastrand::u64(..));
            let content = out.add_object(lopdf::Stream::new(
                dictionary! {},
                format!("% {tag}\n1 0 0 rg 0 0 100 50 re f").into_bytes(),
            ));
            let page = out.add_object(dictionary! {
                "Type" => "Page", "Parent" => pages_id, "Contents" => content,
                "MediaBox" => vec![0.into(), 0.into(), 100.into(), 50.into()],
            });
            out.objects.insert(
                pages_id,
                Object::Dictionary(
                    dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 },
                ),
            );
            let cat = out.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
            out.trailer.set("Root", cat);
            let mut b = Vec::new();
            out.save_to(&mut b).unwrap();
            b
        };
        let path = crate::generated::store(&stamp_pdf).unwrap();
        let stem = crate::generated::stem_of(&path);

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
        let stamp = |page| Edit::AnnotAdd {
            page,
            annot: thread_mark(
                AnnotKind::PageStamp {
                    rect: Box2 {
                        x: 100.0,
                        y: 100.0,
                        w: 100.0,
                        h: 50.0,
                    },
                    stamp: stem.clone(),
                    name: "Paid".into(),
                    text: Some("GE 10/14/2026".into()),
                },
                "",
            ),
        };
        let (p0, p1) = (model.page(0).unwrap().id, model.page(1).unwrap().id);
        model.apply(stamp(p0), "stamp").unwrap();
        model.apply(stamp(p1), "stamp").unwrap();
        let bytes = crate::incremental::incremental(&model.snapshot()).unwrap();
        let pdf = Pdf::load_mem(&bytes).unwrap();
        // Both appearances draw the one imported page.
        let mut forms = std::collections::HashSet::new();
        for page in pdf.get_pages().into_values() {
            let annots = pdf
                .get_dictionary(page)
                .unwrap()
                .get(b"Annots")
                .unwrap()
                .as_array()
                .unwrap();
            let d = pdf
                .get_dictionary(annots[0].as_reference().unwrap())
                .unwrap();
            let n = d
                .get(b"AP")
                .unwrap()
                .as_dict()
                .unwrap()
                .get(b"N")
                .unwrap()
                .as_reference()
                .unwrap();
            let Object::Stream(ap) = pdf.get_object(n).unwrap() else {
                panic!()
            };
            let xo = ap
                .dict
                .get(b"Resources")
                .unwrap()
                .as_dict()
                .unwrap()
                .get(b"XObject")
                .unwrap()
                .as_dict()
                .unwrap();
            forms.insert(xo.get(b"SheafStamp").unwrap().as_reference().unwrap());
            let content = String::from_utf8_lossy(&ap.content).into_owned();
            assert!(
                content.contains("/SheafStamp Do") && content.contains("Tj"),
                "{content}"
            );
        }
        assert_eq!(forms.len(), 1);

        // On a machine without the stamp, it comes back from the file.
        std::fs::remove_file(&path).unwrap();
        crate::file::recover_cached(&pdf);
        let back = Pdf::load_mem(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(back.get_pages().len(), 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_text_replacement_is_a_caret_with_a_grouped_strikeout() {
        use crate::annot::{AnnotKind, Box2};
        let dir = tempdir();
        let a_path = make_pdf(&dir, "a.pdf", "A", 1, &[]);
        let original = std::fs::read(&a_path).unwrap();
        let mut model = Document::new(
            {
                let mut i = info(&a_path, 1);
                i.bytes = original.len();
                i
            },
            &[],
            Box::new(FixedCount(10)),
        );
        let p0 = model.page(0).unwrap().id;
        let words = Box2 {
            x: 72.0,
            y: 80.0,
            w: 120.0,
            h: 14.0,
        };
        let mk = |kind: AnnotKind, contents: &str| Annot {
            id: crate::annot::AnnotId(0),
            kind,
            color: [0, 0, 255],
            opacity: 1.0,
            stroke: 1.0,
            author: "greg".into(),
            contents: contents.into(),
            modified: "D:20261014000000".into(),
            created: "D:20261014000000".into(),
            fill: None,
            dashed: false,
            cloudy: false,
            name: String::new(),
        };
        let replace = mk(
            AnnotKind::Replace {
                rects: vec![words],
                struck: "old words".into(),
                at: crate::annot::caret_box(words.right(), words.bottom(), words.h),
            },
            "new words",
        );
        let underline = mk(
            AnnotKind::Underline {
                rects: vec![Box2 { y: 120.0, ..words }],
            },
            "",
        );
        for (a, label) in [(replace, "replace"), (underline, "underline")] {
            model
                .apply(Edit::AnnotAdd { page: p0, annot: a }, label)
                .unwrap();
        }
        let want: Vec<Annot> = model
            .state()
            .annots_on(p0)
            .unwrap()
            .iter()
            .cloned()
            .collect();

        let annots_of = |bytes: &[u8]| {
            let pdf = Pdf::load_mem(bytes).unwrap();
            let page0 = pdf.get_pages()[&1];
            let list: Vec<lopdf::Dictionary> = pdf
                .get_dictionary(page0)
                .unwrap()
                .get(b"Annots")
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .map(|o| pdf.dereference(o).unwrap().1.as_dict().unwrap().clone())
                .collect();
            let ids: Vec<lopdf::ObjectId> = pdf
                .get_dictionary(page0)
                .unwrap()
                .get(b"Annots")
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .map(|o| o.as_reference().unwrap())
                .collect();
            (list, ids)
        };
        let bytes = crate::incremental::incremental(&model.snapshot()).unwrap();
        let (list, ids) = annots_of(&bytes);
        // Caret, its StrikeOut, the underline.
        let subtypes: Vec<&[u8]> = list
            .iter()
            .map(|d| d.get(b"Subtype").unwrap().as_name().unwrap())
            .collect();
        assert_eq!(
            subtypes,
            [
                b"Caret".as_slice(),
                b"StrikeOut".as_slice(),
                b"Underline".as_slice()
            ]
        );
        let (caret, strike) = (&list[0], &list[1]);
        assert_eq!(
            caret.get(b"Contents").unwrap().as_str().unwrap(),
            b"new words"
        );
        assert_eq!(strike.get(b"IRT").unwrap().as_reference().unwrap(), ids[0]);
        assert_eq!(strike.get(b"RT").unwrap().as_name().unwrap(), b"Group");
        assert!(strike.has(crate::annot::PART.as_bytes()));
        assert!(!strike.has(b"SheafAnnot"));
        assert!(strike.has(b"QuadPoints"));
        // The names the renderer hides are exactly what was written.
        let names: Vec<String> = list
            .iter()
            .map(|d| crate::file::pdf_text(d.get(b"NM").unwrap()))
            .collect();
        let mut expected: Vec<String> = want.iter().flat_map(|a| a.pdf_names()).collect();
        expected.sort();
        let mut got = names.clone();
        got.sort();
        assert_eq!(got, expected);

        // Read back: two marks, nothing foreign.
        let out = dir.join("out.pdf");
        std::fs::write(&out, &bytes).unwrap();
        let back = crate::file::read_annots(&out, None).unwrap();
        assert_eq!(back.sheaf[&0], want);
        assert!(back.foreign.is_empty(), "{:?}", back.foreign);

        // Saved again from the reopened file: the companion isn't doubled.
        let mut model2 = Document::new(
            {
                let mut i = info(&out, 1);
                i.bytes = bytes.len();
                i
            },
            &[],
            Box::new(FixedCount(10)),
        );
        model2.set_initial_annots(back.sheaf).unwrap();
        let p = model2.page(0).unwrap().id;
        let mut changed = model2.state().annots_on(p).unwrap()[0].clone();
        changed.contents = "newer words".into();
        model2
            .apply(
                Edit::AnnotUpdate {
                    page: p,
                    annot: changed,
                },
                "edit",
            )
            .unwrap();
        let bytes2 = crate::incremental::incremental(&model2.snapshot()).unwrap();
        let (list2, _) = annots_of(&bytes2);
        assert_eq!(list2.len(), 3);
        assert_eq!(
            list2[0].get(b"Contents").unwrap().as_str().unwrap(),
            b"newer words"
        );
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
            created: String::new(),
            fill: None,
            dashed: false,
            cloudy: false,
            name: String::new(),
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

    /// A connector group's /Dest walk it in page order and wrap; a new
    /// member, saved later, is fitted into the cycle on every page.
    #[test]
    fn connector_groups_link_in_a_cycle() {
        use crate::annot::{AnnotKind, Box2};
        let dir = tempdir();
        let a_path = make_pdf(&dir, "g.pdf", "G", 3, &[]);
        let original = std::fs::read(&a_path).unwrap();
        let mut model = opened_n(&a_path, 3, original.len());
        let page = |i: usize| model.page(i).unwrap().id;
        let (p0, p1, p2) = (page(0), page(1), page(2));
        let connector = |y: f32| Annot {
            id: crate::annot::AnnotId(0),
            kind: AnnotKind::Connector {
                rect: Box2 {
                    x: 100.0,
                    y,
                    w: 22.0,
                    h: 22.0,
                },
                label: "C".into(),
                partner: None,
            },
            color: [220, 40, 40],
            opacity: 1.0,
            stroke: 1.0,
            author: "greg".into(),
            contents: String::new(),
            modified: String::new(),
            created: String::new(),
            fill: None,
            dashed: false,
            cloudy: false,
            name: String::new(),
        };
        for pg in [p0, p1, p2] {
            model
                .apply(
                    Edit::AnnotAdd {
                        page: pg,
                        annot: connector(100.0),
                    },
                    "c",
                )
                .unwrap();
        }
        // Each page's links, top to bottom, as the page number each goes to.
        let dests = |bytes: &[u8]| -> Vec<Vec<u32>> {
            let pdf = Pdf::load_mem(bytes).unwrap();
            let pages = pdf.get_pages();
            let number: HashMap<ObjectId, u32> = pages.iter().map(|(n, id)| (*id, *n)).collect();
            pages
                .values()
                .map(|id| {
                    let mut links: Vec<(f32, u32)> = pdf
                        .get_dictionary(*id)
                        .unwrap()
                        .get(b"Annots")
                        .map(|a| a.as_array().unwrap().clone())
                        .unwrap_or_default()
                        .iter()
                        .map(|o| {
                            let d = pdf.get_dictionary(o.as_reference().unwrap()).unwrap();
                            let top = d.get(b"Rect").unwrap().as_array().unwrap()[3]
                                .as_float()
                                .unwrap();
                            let to = d.get(b"Dest").unwrap().as_array().unwrap()[0]
                                .as_reference()
                                .unwrap();
                            (-top, number[&to])
                        })
                        .collect();
                    links.sort_by(|a, b| a.0.total_cmp(&b.0));
                    links.into_iter().map(|(_, n)| n).collect()
                })
                .collect()
        };
        let bytes = crate::incremental::incremental(&model.snapshot()).unwrap();
        assert_eq!(dests(&bytes), [vec![2], vec![3], vec![1]]);

        // Saved, reopened, and a fourth added below the first: page 1's
        // first now goes to its neighbour, and page 3 comes back to it.
        let out = dir.join("g2.pdf");
        std::fs::write(&out, &bytes).unwrap();
        let mut model = opened_n(&out, 3, bytes.len());
        model
            .set_initial_annots(crate::file::read_sheaf_annots(&out).unwrap())
            .unwrap();
        let p0 = model.page(0).unwrap().id;
        model
            .apply(
                Edit::AnnotAdd {
                    page: p0,
                    annot: connector(400.0),
                },
                "c",
            )
            .unwrap();
        let bytes = crate::incremental::incremental(&model.snapshot()).unwrap();
        assert_eq!(dests(&bytes), [vec![1, 2], vec![3], vec![1]]);
    }

    fn opened_n(path: &Path, n: u32, bytes: usize) -> Document {
        Document::new(
            {
                let mut i = info(path, n);
                i.bytes = bytes;
                i
            },
            &[],
            Box::new(FixedCount(10)),
        )
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
            created: String::new(),
            fill: None,
            dashed: false,
            cloudy: false,
            name: String::new(),
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
                    view: None,
                    color: None,
                    style: Default::default(),
                },
                FlatOutlineItem {
                    title: "Second".into(),
                    page: Some(1),
                    depth: 0,
                    view: None,
                    color: None,
                    style: Default::default(),
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
                        created: String::new(),
                        fill: None,
                        dashed: false,
                        cloudy: false,
                        name: String::new(),
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
            created: String::new(),
            fill: None,
            dashed: false,
            cloudy: false,
            name: String::new(),
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
                view: None,
                color: None,
                style: Default::default(),
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

    /// Extract keeps the chosen pages in order, their marks, and the
    /// bookmarks that point at them (parents of kept children become
    /// headings).
    #[test]
    fn extract_pages_keeps_marks_and_relevant_bookmarks() {
        use crate::annot::{AnnotKind, Box2};
        let dir = tempdir();
        let a = make_pdf(&dir, "x.pdf", "X", 4, &[]);
        let mut model = Document::new(
            info(&a, 4),
            &[
                FlatOutlineItem {
                    title: "Part".into(),
                    page: Some(0),
                    depth: 0,
                    view: None,
                    color: None,
                    style: Default::default(),
                },
                FlatOutlineItem {
                    title: "Second".into(),
                    page: Some(1),
                    depth: 1,
                    view: None,
                    color: None,
                    style: Default::default(),
                },
                FlatOutlineItem {
                    title: "Fourth".into(),
                    page: Some(3),
                    depth: 1,
                    view: None,
                    color: None,
                    style: Default::default(),
                },
                FlatOutlineItem {
                    title: "Third".into(),
                    page: Some(2),
                    depth: 0,
                    view: None,
                    color: None,
                    style: Default::default(),
                },
            ],
            Box::new(FixedCount(10)),
        );
        let ids: Vec<PageId> = (0..4).map(|i| model.page(i).unwrap().id).collect();
        for (i, kind) in [
            (
                1usize,
                AnnotKind::Square {
                    rect: Box2 {
                        x: 10.0,
                        y: 10.0,
                        w: 50.0,
                        h: 20.0,
                    },
                },
            ),
            (
                2usize,
                AnnotKind::Note {
                    pos: crate::annot::Pt { x: 5.0, y: 5.0 },
                },
            ),
        ] {
            model
                .apply(
                    Edit::AnnotAdd {
                        page: ids[i],
                        annot: Annot {
                            id: crate::annot::AnnotId(0),
                            kind,
                            color: [0, 0, 0],
                            opacity: 1.0,
                            stroke: 1.0,
                            author: "t".into(),
                            contents: String::new(),
                            modified: String::new(),
                            created: String::new(),
                            fill: None,
                            dashed: false,
                            cloudy: false,
                            name: String::new(),
                        },
                    },
                    "a",
                )
                .unwrap();
        }
        let out = dir.join("extract.pdf");
        // Pages 2 and 4 (ids given out of order: output keeps document order).
        let n = extract_pages(&model.snapshot(), &[ids[3], ids[1]], &out).unwrap();
        assert_eq!(n, 2);
        let pdf = Pdf::load(&out).unwrap();
        assert_eq!(pdf.get_pages().len(), 2);
        let text = |i: u32| {
            String::from_utf8_lossy(&pdf.get_page_content(pdf.get_pages()[&i])).into_owned()
        };
        assert!(text(1).contains("MARK X 1"), "{}", text(1));
        assert!(text(2).contains("MARK X 3"), "{}", text(2));
        // Only page 2's square came along.
        let marks = crate::file::read_sheaf_annots(&out).unwrap();
        assert_eq!(marks.len(), 1);
        assert!(matches!(marks[&0][0].kind, AnnotKind::Square { .. }));
        // Bookmarks: Part (now a heading) > Second, Fourth; Third dropped.
        let items = read_outline(&std::fs::read(&out).unwrap());
        let titles: Vec<(u16, String, Option<u32>)> = items;
        assert_eq!(
            titles,
            vec![
                (0, "Part".into(), None),
                (1, "Second".into(), Some(1)),
                (1, "Fourth".into(), Some(2)),
            ]
        );
        assert!(extract_pages(&model.snapshot(), &[], &out).is_err());
    }

    /// The print file has just the chosen pages, no bookmarks, and its marks
    /// drawn in or left out.
    /// A header/footer is drawn into page content as pagination
    /// artifacts on save, its settings kept in the catalog; a derived copy
    /// can strip it again.
    #[test]
    fn header_footer_is_drawn_stored_and_strippable() {
        use crate::stamps::{HeaderFooter, has_artifacts};
        let dir = tempdir();
        let a = make_pdf(&dir, "hf.pdf", "H", 2, &[]);
        let mut model = Document::new(info(&a, 2), &[], crate::history::default_policy());
        let hf = HeaderFooter {
            texts: [
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                "Page <<1>> of <<N>>".into(),
                String::new(),
            ],
            ..Default::default()
        };
        model
            .apply(
                Edit::SetHeaderFooter {
                    header_footer: Some(hf.clone()),
                },
                "hf",
            )
            .unwrap();
        assert!(model.header_footer_dirty());
        let out = dir.join("hf-out.pdf");
        // In place it falls back from incremental to a rewrite.
        rewrite_to_path(&model.snapshot(), &out).unwrap();
        let pdf = Pdf::load(&out).unwrap();
        let pages: Vec<ObjectId> = pdf.get_pages().into_values().collect();
        for (i, &p) in pages.iter().enumerate() {
            let dict = pdf.get_dictionary(p).unwrap();
            assert!(has_artifacts(&pdf, dict, crate::stamps::HEADER_FOOTER));
            let all: Vec<u8> = dict
                .get(b"Contents")
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|o| {
                    let s = pdf
                        .get_object(o.as_reference().unwrap())
                        .unwrap()
                        .as_stream()
                        .unwrap();
                    s.decompressed_content()
                        .unwrap_or_else(|_| s.content.clone())
                })
                .collect();
            let want = format!("(Page {} of 2)", i + 1);
            assert!(String::from_utf8_lossy(&all).contains(&want), "page {i}");
        }
        let (stored, scope) = crate::stamps::from_catalog(&pdf).unwrap();
        assert_eq!(stored.texts, hf.texts);
        assert_eq!(scope, None);
        // Stripped, both pages come back clean.
        let bytes = derived_copy(
            &info(&out, 2),
            &[(0, None), (1, None)],
            &mut |pdf, d, _, _| {
                crate::stamps::strip(pdf, d, crate::stamps::HEADER_FOOTER);
                Ok(())
            },
        )
        .unwrap();
        let clean = Pdf::load_mem(&bytes).unwrap();
        for p in clean.get_pages().into_values() {
            assert!(!has_artifacts(
                &clean,
                clean.get_dictionary(p).unwrap(),
                crate::stamps::HEADER_FOOTER
            ));
        }
    }

    /// Labels set in the model follow their pages through a reorder, by
    /// rewrite and by incremental save, and removing them removes them.
    #[test]
    fn page_labels_are_written_and_follow_pages() {
        use crate::labels::{LabelSpec, Style, label_ranges, labels_for};
        let dir = tempdir();
        let a = make_pdf(&dir, "labels.pdf", "L", 4, &[]);
        let bytes = std::fs::metadata(&a).unwrap().len() as usize;
        let mut model = Document::new(
            SourceInfo {
                bytes,
                ..info(&a, 4)
            },
            &[],
            crate::history::default_policy(),
        );
        let ids: Vec<PageId> = model.state().pages.iter().map(|p| p.id).collect();
        let spec = |id, style, first| LabelSpec {
            start: id,
            style,
            prefix: String::new(),
            first,
        };
        model
            .apply(
                Edit::SetPageLabels {
                    ranges: vec![
                        spec(ids[0], Style::LowerRoman, 1),
                        spec(ids[2], Style::Decimal, 1),
                    ],
                },
                "labels",
            )
            .unwrap();
        let read = |p: &Path| {
            let pdf = Pdf::load(p).unwrap();
            label_ranges(&pdf).map(|r| labels_for(&r, 4))
        };
        // Incremental, in place.
        assert_eq!(
            save_to_path(&model.snapshot(), &a).unwrap(),
            Strategy::Incremental
        );
        assert_eq!(read(&a).unwrap(), vec!["i", "ii", "1", "2"]);
        // Move the decimal range's first page to the front; rewrite.
        model
            .apply(
                Edit::MovePages {
                    ids: vec![ids[2]],
                    before: Some(ids[0]),
                },
                "move",
            )
            .unwrap();
        let out = dir.join("moved.pdf");
        rewrite_to_path(&model.snapshot(), &out).unwrap();
        assert_eq!(read(&out).unwrap(), vec!["1", "i", "ii", "iii"]);
        // Removed.
        model
            .apply(Edit::SetPageLabels { ranges: vec![] }, "none")
            .unwrap();
        let out = dir.join("none.pdf");
        rewrite_to_path(&model.snapshot(), &out).unwrap();
        assert_eq!(read(&out), None);
    }

    /// Displayed positions start at the CropBox's corner, as viewers draw.
    #[test]
    fn geometry_follows_the_crop_box() {
        let d = dictionary! {
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "CropBox" => vec![100.into(), 100.into(), 400.into(), 500.into()],
        };
        let g = geom_of(&d, 0);
        assert_eq!(g.media, [100.0, 100.0, 400.0, 500.0]);
        let u = g.to_user(crate::annot::Pt { x: 10.0, y: 10.0 });
        assert_eq!((u.x, u.y), (110.0, 490.0));
        // A crop outside the media box is ignored.
        let d = dictionary! {
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "CropBox" => vec![700.into(), 700.into(), 800.into(), 900.into()],
        };
        assert_eq!(geom_of(&d, 0).media, [0.0, 0.0, 612.0, 792.0]);
    }

    #[test]
    fn print_pdf_flattens_or_drops_markup() {
        use crate::annot::{AnnotKind, Box2};
        let dir = tempdir();
        let a = make_pdf(&dir, "p.pdf", "P", 3, &[]);
        let mut model = Document::new(
            info(&a, 3),
            &[FlatOutlineItem {
                title: "Top".into(),
                page: Some(1),
                depth: 0,
                view: None,
                color: None,
                style: Default::default(),
            }],
            Box::new(FixedCount(10)),
        );
        let ids: Vec<PageId> = (0..3).map(|i| model.page(i).unwrap().id).collect();
        model
            .apply(
                Edit::AnnotAdd {
                    page: ids[1],
                    annot: Annot {
                        id: crate::annot::AnnotId(0),
                        kind: AnnotKind::Square {
                            rect: Box2 {
                                x: 10.0,
                                y: 10.0,
                                w: 50.0,
                                h: 20.0,
                            },
                        },
                        color: [255, 0, 0],
                        opacity: 1.0,
                        stroke: 1.0,
                        author: "t".into(),
                        contents: String::new(),
                        modified: String::new(),
                        created: String::new(),
                        fill: None,
                        dashed: false,
                        cloudy: false,
                        name: String::new(),
                    },
                },
                "a",
            )
            .unwrap();
        let snap = model.snapshot();
        let with = Pdf::load_mem(&print_pdf(&snap, &[ids[2], ids[1]], true).unwrap()).unwrap();
        assert_eq!(with.get_pages().len(), 2);
        let first = with.get_pages()[&1];
        let content = String::from_utf8_lossy(&with.get_page_content(first)).into_owned();
        assert!(content.contains("MARK P 1"), "{content}");
        // The square is drawn into the page, not left as an annotation.
        let annots = with.get_page_annotations(first).unwrap_or_default();
        assert!(annots.is_empty());
        assert!(content.len() > "MARK P 1".len() + 20, "{content}");
        assert!(with.catalog().unwrap().get(b"Outlines").is_err());
        let without = Pdf::load_mem(&print_pdf(&snap, &[ids[1]], false).unwrap()).unwrap();
        let only = without.get_pages()[&1];
        assert!(
            without
                .get_page_annotations(only)
                .unwrap_or_default()
                .is_empty()
        );
        let plain = String::from_utf8_lossy(&without.get_page_content(only)).into_owned();
        assert!(plain.len() < content.len(), "{plain}");
        assert!(print_pdf(&snap, &[], true).is_err());
    }

    /// Flattening draws each mark's appearance into the page content and
    /// removes the annotation; links stay.
    #[test]
    fn flattened_copy_burns_marks_into_content() {
        use crate::annot::{AnnotKind, Box2};
        let dir = tempdir();
        let a = make_pdf(&dir, "f.pdf", "F", 1, &[]);
        {
            let mut pdf = Pdf::load(&a).unwrap();
            let page0 = pdf.get_pages()[&1];
            let link = pdf.add_object(dictionary! {
                "Type" => "Annot", "Subtype" => "Link",
                "Rect" => Object::Array(vec![0.into(), 0.into(), 50.into(), 20.into()]),
            });
            pdf.get_dictionary_mut(page0)
                .unwrap()
                .set("Annots", Object::Array(vec![link.into()]));
            pdf.save(&a).unwrap();
        }
        let mut model = Document::new(info(&a, 1), &[], Box::new(FixedCount(10)));
        let p0 = model.page(0).unwrap().id;
        model
            .apply(
                Edit::AnnotAdd {
                    page: p0,
                    annot: Annot {
                        id: crate::annot::AnnotId(0),
                        kind: AnnotKind::Square {
                            rect: Box2 {
                                x: 100.0,
                                y: 100.0,
                                w: 80.0,
                                h: 40.0,
                            },
                        },
                        color: [0, 120, 255],
                        opacity: 1.0,
                        stroke: 2.0,
                        author: "t".into(),
                        contents: String::new(),
                        modified: String::new(),
                        created: String::new(),
                        fill: None,
                        dashed: false,
                        cloudy: false,
                        name: String::new(),
                    },
                },
                "sq",
            )
            .unwrap();
        let out = dir.join("flat.pdf");
        assert_eq!(export_flattened(&model.snapshot(), &out).unwrap(), 1);
        let pdf = Pdf::load(&out).unwrap();
        let page = pdf.get_pages()[&1];
        let dict = pdf.get_dictionary(page).unwrap();
        // Only the link remains as an annotation.
        let annots = pdf
            .dereference(dict.get(b"Annots").unwrap())
            .unwrap()
            .1
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(annots.len(), 1);
        assert!(crate::file::read_sheaf_annots(&out).unwrap().is_empty());
        // The content draws the form XObject, which is registered in Resources.
        let content = String::from_utf8_lossy(&pdf.get_page_content(page)).into_owned();
        assert!(content.contains("/SheafFlat1 Do"), "{content}");
        assert!(content.starts_with("q\n"), "{content}");
        let (res, _) = pdf.get_page_resources(page).unwrap();
        let xo = res.unwrap().get(b"XObject").unwrap().as_dict().unwrap();
        let form = pdf
            .get_object(xo.get(b"SheafFlat1").unwrap().as_reference().unwrap())
            .unwrap()
            .as_stream()
            .unwrap();
        assert_eq!(
            form.dict.get(b"Subtype").unwrap().as_name().unwrap(),
            b"Form"
        );
        // The form content is the square's stroke.
        let form_ops = String::from_utf8_lossy(&form.decompressed_content().unwrap()).into_owned();
        assert!(form_ops.contains(" re S"), "{form_ops}");
    }

    // ----- OCR text layers ---------------------------------------------------

    fn ocr_layer(text: &str) -> std::sync::Arc<crate::text_layer::TextLayer> {
        use crate::annot::Box2;
        use crate::text_layer::{OcrWord, TextLayer};
        std::sync::Arc::new(TextLayer::from_engine(
            vec![OcrWord {
                text: text.into(),
                rect: Box2 {
                    x: 72.0,
                    y: 100.0,
                    w: 100.0,
                    h: 12.0,
                },
                confidence: 0.95,
            }],
            PageSize {
                width: 612.0,
                height: 792.0,
            },
            Rotation::R0,
            "test",
        ))
    }

    fn set_layer(model: &mut Document, page: PageId, text: &str) {
        model
            .apply(
                Edit::SetTextLayers {
                    layers: vec![(page, ocr_layer(text))],
                },
                "ocr",
            )
            .unwrap();
    }

    /// The page's /Contents as an array (a lone reference becomes one entry).
    fn contents_of(pdf: &Pdf, page: ObjectId) -> Vec<Object> {
        let dict = pdf.get_dictionary(page).unwrap();
        match pdf.dereference(dict.get(b"Contents").unwrap()).unwrap().1 {
            Object::Array(a) => a.clone(),
            _ => vec![dict.get(b"Contents").unwrap().clone()],
        }
    }

    fn stream_text(pdf: &Pdf, entry: &Object) -> String {
        let s = pdf
            .get_object(entry.as_reference().unwrap())
            .unwrap()
            .as_stream()
            .unwrap();
        String::from_utf8_lossy(&s.decompressed_content().unwrap()).into_owned()
    }

    fn fonts_of(pdf: &Pdf, page: ObjectId) -> Dictionary {
        // Inline on the page, or referenced (possibly inherited).
        let (res, ids) = pdf.get_page_resources(page).unwrap();
        let res: Dictionary = match res {
            Some(d) => d.clone(),
            None => pdf.get_dictionary(ids[0]).unwrap().clone(),
        };
        pdf.dereference(res.get(b"Font").unwrap())
            .unwrap()
            .1
            .as_dict()
            .unwrap()
            .clone()
    }

    fn glyphless_fonts(pdf: &Pdf) -> usize {
        pdf.objects
            .values()
            .filter(|o| {
                o.as_dict().is_ok_and(|d| {
                    d.get(b"Subtype")
                        .is_ok_and(|s| s.as_name().is_ok_and(|n| n == b"Type0"))
                        && d.get(b"BaseFont")
                            .is_ok_and(|b| b.as_name().is_ok_and(|n| n == b"GlyphLessFont"))
                })
            })
            .count()
    }

    const HELLO_HEX: &str = "<00480065006C006C006F>";

    #[test]
    fn rewrite_writes_invisible_text_layer() {
        let dir = tempdir();
        let a = make_pdf(&dir, "a.pdf", "A", 3, &[]);
        let mut model = Document::new(info(&a, 3), &[], Box::new(FixedCount(10)));
        let ids: Vec<PageId> = model.state().pages.iter().map(|p| p.id).collect();
        set_layer(&mut model, ids[1], "Hello");
        let bytes = rewrite(&model.snapshot()).unwrap();
        assert_eq!(
            read_pages(&bytes),
            vec![("A 0".into(), 0), ("A 1".into(), 0), ("A 2".into(), 0)]
        );
        let pdf = Pdf::load_mem(&bytes).unwrap();
        let pages: Vec<ObjectId> = pdf.get_pages().into_values().collect();
        let contents = contents_of(&pdf, pages[1]);
        assert_eq!(contents.len(), 3);
        assert_eq!(stream_text(&pdf, &contents[0]), "q\n");
        assert!(stream_text(&pdf, &contents[1]).contains("MARK A 1"));
        let layer = stream_text(&pdf, &contents[2]);
        assert!(layer.starts_with("Q\nBT\n3 Tr\n"), "{layer}");
        assert!(layer.contains(HELLO_HEX), "{layer}");
        // Inherited fonts survive next to ours, inline on the page.
        let fonts = fonts_of(&pdf, pages[1]);
        assert!(fonts.has(b"F1"));
        let type0 = pdf
            .get_dictionary(fonts.get(b"SheafOcr").unwrap().as_reference().unwrap())
            .unwrap();
        assert_eq!(type0.get(b"Subtype").unwrap().as_name().unwrap(), b"Type0");
        let cid = pdf
            .get_dictionary(
                type0.get(b"DescendantFonts").unwrap().as_array().unwrap()[0]
                    .as_reference()
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(cid.get(b"DW").unwrap().as_i64().unwrap(), 500);
        assert!(type0.has(b"ToUnicode"));
        // Other pages are untouched.
        for &p in [pages[0], pages[2]].iter() {
            assert_eq!(contents_of(&pdf, p).len(), 1);
            assert!(!fonts_of(&pdf, p).has(b"SheafOcr"));
        }
    }

    #[test]
    fn incremental_unchanged_order_writes_layer() {
        let dir = tempdir();
        let a = make_pdf(&dir, "a.pdf", "A", 2, &[]);
        let original = std::fs::read(&a).unwrap();
        let mut model = Document::new(
            {
                let mut i = info(&a, 2);
                i.bytes = original.len();
                i
            },
            &[],
            Box::new(FixedCount(10)),
        );
        let ids: Vec<PageId> = model.state().pages.iter().map(|p| p.id).collect();
        set_layer(&mut model, ids[0], "Hello");
        let bytes = crate::incremental::incremental(&model.snapshot()).unwrap();
        assert!(bytes.starts_with(&original), "original bytes preserved");
        assert_eq!(
            read_pages(&bytes),
            vec![("A 0".into(), 0), ("A 1".into(), 0)]
        );
        let pdf = Pdf::load_mem(&bytes).unwrap();
        let pages: Vec<ObjectId> = pdf.get_pages().into_values().collect();
        let contents = contents_of(&pdf, pages[0]);
        assert_eq!(contents.len(), 3);
        assert!(stream_text(&pdf, &contents[2]).contains(HELLO_HEX));
        // The rewritten page pulled its inherited attributes down.
        let dict = pdf.get_dictionary(pages[0]).unwrap();
        assert!(dict.has(b"MediaBox"));
        assert!(dict.get(b"Resources").unwrap().as_dict().is_ok(), "inline");
        let fonts = fonts_of(&pdf, pages[0]);
        assert!(fonts.has(b"F1") && fonts.has(b"SheafOcr"));
        assert!(!pdf.get_dictionary(pages[1]).unwrap().has(b"Resources"));
        assert_eq!(glyphless_fonts(&pdf), 1);
        // The chooser saves incrementally and the file reopens with the layer.
        assert_eq!(
            save_to_path(&model.snapshot(), &a).unwrap(),
            Strategy::Incremental
        );
        let saved = Pdf::load(&a).unwrap();
        let p0 = saved.get_pages()[&1];
        assert_eq!(contents_of(&saved, p0).len(), 3);
    }

    #[test]
    fn incremental_tree_changed_writes_layer() {
        let dir = tempdir();
        let a = make_pdf(&dir, "a.pdf", "A", 3, &[]);
        let original = std::fs::read(&a).unwrap();
        let mut model = Document::new(
            {
                let mut i = info(&a, 3);
                i.bytes = original.len();
                i
            },
            &[],
            Box::new(FixedCount(10)),
        );
        let ids: Vec<PageId> = model.state().pages.iter().map(|p| p.id).collect();
        set_layer(&mut model, ids[2], "Hello");
        model
            .apply(Edit::DeletePages { ids: vec![ids[0]] }, "del")
            .unwrap();
        let bytes = crate::incremental::incremental(&model.snapshot()).unwrap();
        assert!(bytes.starts_with(&original));
        assert_eq!(
            read_pages(&bytes),
            vec![("A 1".into(), 0), ("A 2".into(), 0)]
        );
        let pdf = Pdf::load_mem(&bytes).unwrap();
        let pages: Vec<ObjectId> = pdf.get_pages().into_values().collect();
        assert_eq!(contents_of(&pdf, pages[0]).len(), 1);
        let contents = contents_of(&pdf, pages[1]);
        assert_eq!(contents.len(), 3);
        assert!(stream_text(&pdf, &contents[2]).contains(HELLO_HEX));
        assert!(fonts_of(&pdf, pages[1]).has(b"F1"));
    }

    #[test]
    fn one_font_set_per_file() {
        let dir = tempdir();
        let a = make_pdf(&dir, "a.pdf", "A", 3, &[]);
        let mut model = Document::new(info(&a, 3), &[], Box::new(FixedCount(10)));
        let ids: Vec<PageId> = model.state().pages.iter().map(|p| p.id).collect();
        model
            .apply(
                Edit::SetTextLayers {
                    layers: vec![
                        (ids[0], ocr_layer("one")),
                        (ids[1], ocr_layer("two")),
                        (ids[2], ocr_layer("three")),
                    ],
                },
                "ocr",
            )
            .unwrap();
        let bytes = rewrite(&model.snapshot()).unwrap();
        let pdf = Pdf::load_mem(&bytes).unwrap();
        assert_eq!(glyphless_fonts(&pdf), 1);
        let q_streams = pdf
            .objects
            .values()
            .filter(|o| {
                o.as_stream()
                    .is_ok_and(|s| s.decompressed_content().unwrap() == b"q\n")
            })
            .count();
        assert_eq!(q_streams, 1);
    }

    #[test]
    fn duplicate_shares_layer_and_is_not_double_rotated() {
        let dir = tempdir();
        let a = make_pdf(&dir, "a.pdf", "A", 2, &[]);
        let mut model = Document::new(info(&a, 2), &[], Box::new(FixedCount(10)));
        let ids: Vec<PageId> = model.state().pages.iter().map(|p| p.id).collect();
        model
            .apply(
                Edit::RotatePages {
                    ids: vec![ids[0]],
                    by: Rotation::R90,
                },
                "rot",
            )
            .unwrap();
        set_layer(&mut model, ids[0], "Hello");
        model
            .apply(Edit::DuplicatePages { ids: vec![ids[0]] }, "dup")
            .unwrap();
        let bytes = rewrite(&model.snapshot()).unwrap();
        assert_eq!(
            read_pages(&bytes),
            vec![("A 0".into(), 90), ("A 0".into(), 90), ("A 1".into(), 0)]
        );
        let pdf = Pdf::load_mem(&bytes).unwrap();
        let pages: Vec<ObjectId> = pdf.get_pages().into_values().collect();
        for &p in &pages[..2] {
            let contents = contents_of(&pdf, p);
            assert_eq!(contents.len(), 3);
            assert!(stream_text(&pdf, &contents[2]).contains(HELLO_HEX));
        }
    }

    #[test]
    fn reinstall_replaces_previous_layer() {
        let dir = tempdir();
        let a = make_pdf(&dir, "a.pdf", "A", 1, &[]);
        let mut model = Document::new(info(&a, 1), &[], Box::new(FixedCount(10)));
        let p0 = model.page(0).unwrap().id;
        set_layer(&mut model, p0, "Hello");
        let b = dir.join("b.pdf");
        rewrite_to_path(&model.snapshot(), &b).unwrap();

        // Re-OCR the saved file: the old layer goes, the new one comes.
        let mut model = Document::new(info(&b, 1), &[], Box::new(FixedCount(10)));
        let p0 = model.page(0).unwrap().id;
        set_layer(&mut model, p0, "World");
        let bytes = rewrite(&model.snapshot()).unwrap();
        let pdf = Pdf::load_mem(&bytes).unwrap();
        let page = pdf.get_pages()[&1];
        let contents = contents_of(&pdf, page);
        assert_eq!(contents.len(), 3);
        let all = String::from_utf8_lossy(&pdf.get_page_content(page)).into_owned();
        assert!(all.contains("<0057006F0072006C0064>"), "{all}");
        assert!(!all.contains(HELLO_HEX), "{all}");
        assert_eq!(glyphless_fonts(&pdf), 1);

        // Saving the file again with no layer in the model keeps what's baked.
        let model = Document::new(info(&b, 1), &[], Box::new(FixedCount(10)));
        let bytes = rewrite(&model.snapshot()).unwrap();
        let pdf = Pdf::load_mem(&bytes).unwrap();
        let page = pdf.get_pages()[&1];
        assert_eq!(contents_of(&pdf, page).len(), 3);
    }

    #[test]
    fn extract_flatten_and_clean_keep_layer() {
        use crate::annot::{AnnotKind, Box2};
        let dir = tempdir();
        let a = make_pdf(&dir, "a.pdf", "A", 2, &[]);
        let mut model = Document::new(info(&a, 2), &[], Box::new(FixedCount(10)));
        let ids: Vec<PageId> = model.state().pages.iter().map(|p| p.id).collect();
        set_layer(&mut model, ids[1], "Hello");
        model
            .apply(
                Edit::AnnotAdd {
                    page: ids[1],
                    annot: Annot {
                        id: crate::annot::AnnotId(0),
                        kind: AnnotKind::Square {
                            rect: Box2 {
                                x: 100.0,
                                y: 100.0,
                                w: 80.0,
                                h: 40.0,
                            },
                        },
                        color: [255, 0, 0],
                        opacity: 1.0,
                        stroke: 2.0,
                        author: "t".into(),
                        contents: String::new(),
                        modified: String::new(),
                        created: String::new(),
                        fill: None,
                        dashed: false,
                        cloudy: false,
                        name: String::new(),
                    },
                },
                "sq",
            )
            .unwrap();
        let snap = model.snapshot();

        let out = dir.join("extract.pdf");
        assert_eq!(extract_pages(&snap, &[ids[1]], &out).unwrap(), 1);
        let pdf = Pdf::load(&out).unwrap();
        let page = pdf.get_pages()[&1];
        assert!(String::from_utf8_lossy(&pdf.get_page_content(page)).contains(HELLO_HEX));

        let out = dir.join("flat.pdf");
        assert_eq!(export_flattened(&snap, &out).unwrap(), 1);
        let pdf = Pdf::load(&out).unwrap();
        let page = pdf.get_pages()[&2];
        let all = String::from_utf8_lossy(&pdf.get_page_content(page)).into_owned();
        assert!(all.contains(HELLO_HEX), "{all}");
        assert!(all.contains("/SheafFlat1 Do"), "{all}");
        assert!(fonts_of(&pdf, page).has(b"SheafOcr"));

        let out = dir.join("clean.pdf");
        export_clean_copy(&snap, &out).unwrap();
        let pdf = Pdf::load(&out).unwrap();
        let page = pdf.get_pages()[&2];
        assert!(String::from_utf8_lossy(&pdf.get_page_content(page)).contains(HELLO_HEX));
        assert!(!pdf.get_dictionary(page).unwrap().has(b"Annots"));
    }

    #[test]
    fn empty_layer_writes_nothing() {
        let dir = tempdir();
        let a = make_pdf(&dir, "a.pdf", "A", 1, &[]);
        let mut model = Document::new(info(&a, 1), &[], Box::new(FixedCount(10)));
        let p0 = model.page(0).unwrap().id;
        let empty = std::sync::Arc::new(crate::text_layer::TextLayer::from_engine(
            vec![],
            PageSize {
                width: 612.0,
                height: 792.0,
            },
            Rotation::R0,
            "test",
        ));
        model
            .apply(
                Edit::SetTextLayers {
                    layers: vec![(p0, empty)],
                },
                "ocr",
            )
            .unwrap();
        let bytes = rewrite(&model.snapshot()).unwrap();
        let pdf = Pdf::load_mem(&bytes).unwrap();
        let page = pdf.get_pages()[&1];
        assert_eq!(contents_of(&pdf, page).len(), 1);
        assert_eq!(glyphless_fonts(&pdf), 0);
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
