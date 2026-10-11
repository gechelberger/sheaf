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

use crate::model::{OutlineNode, PageId, Snapshot, SourceId};

/// Page-level keys a page may inherit from its ancestors in the page tree.
/// We flatten the tree, so these must be copied onto each page.
const INHERITED: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

/// Serialize a snapshot as a complete new PDF.
pub fn rewrite(model: &Snapshot) -> Result<Vec<u8>> {
    let primary_id = model.primary;
    let primary_info = model.source(primary_id).context("primary source missing")?;
    let mut pdf = Pdf::load(&primary_info.path)
        .with_context(|| format!("loading {}", primary_info.path.display()))?;

    // Source page object ids and their inherited attributes, resolved against
    // the *original* trees before anything is re-parented.
    let mut sources: HashMap<SourceId, LoadedSource> = HashMap::new();
    sources.insert(primary_id, LoadedSource::from_pdf(&pdf)?);
    for (&sid, info) in &model.sources {
        if sid != primary_id && model.state.pages.iter().any(|p| p.source == sid) {
            let other = Pdf::load(&info.path)
                .with_context(|| format!("loading {}", info.path.display()))?;
            sources.insert(sid, LoadedSource::from_pdf_owned(other)?);
        }
    }

    let state = &model.state;
    let pages_id = pdf.new_object_id();
    let mut kids: Vec<Object> = Vec::with_capacity(state.pages.len());
    let mut page_obj: HashMap<PageId, ObjectId> = HashMap::new();
    let mut used: HashSet<ObjectId> = HashSet::new();
    let mut copy_maps: HashMap<SourceId, HashMap<ObjectId, ObjectId>> = HashMap::new();

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

/// Rewrite to `path` atomically (temp file + rename).
pub fn rewrite_to_path(model: &Snapshot, path: &Path) -> Result<()> {
    let bytes = rewrite(model)?;
    let dir = path.parent().unwrap_or(Path::new("."));
    let tmp = dir.join(format!(
        ".{}.sheaf-tmp",
        path.file_name().and_then(|s| s.to_str()).unwrap_or("save")
    ));
    std::fs::write(&tmp, &bytes).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

struct LoadedSource {
    /// Kept for non-primary sources so pages can be copied out of them.
    pdf: Option<Pdf>,
    /// Page object ids in page order.
    pages: Vec<ObjectId>,
    /// Inherited attributes per page, as found in the original tree.
    inherited: HashMap<ObjectId, Vec<(Vec<u8>, Object)>>,
}

impl LoadedSource {
    fn from_pdf(pdf: &Pdf) -> Result<Self> {
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
    fn from_pdf_owned(pdf: Pdf) -> Result<Self> {
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

fn is_identity(model: &Snapshot, primary: &LoadedSource) -> bool {
    let pages = &model.state.pages;
    pages.len() == primary.pages.len()
        && pages
            .iter()
            .enumerate()
            .all(|(i, p)| p.source == model.primary && p.index as usize == i)
}

/// Clone a page dictionary from `src` into `dst`, deep-copying everything it
/// references (resources, content, annotations…) with fresh object numbers.
fn copy_page_dict(
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

fn write_outline(
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
                let dict = pdf.get_dictionary(id).unwrap();
                let rot = dict
                    .get(b"Rotate")
                    .ok()
                    .and_then(|o| o.as_i64().ok())
                    .unwrap_or(0);
                // Inherited resources must have been flattened onto the page.
                assert!(dict.has(b"Resources"), "page lost its resources");
                assert!(dict.has(b"MediaBox"), "page lost its media box");
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
