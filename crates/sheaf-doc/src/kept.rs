//! A saved file reaches only the pages it keeps (M9 bug 14, slice 21).
//!
//! A rewrite edits the primary file in place, and a copy follows
//! references. Either way, anything that still points at a page that isn't
//! kept keeps it in the file, and through its `/Parent` the old page tree
//! and every page in it: a link, a named destination, the open action, a
//! form field's widget, the structure tree, an article thread.
//! [`keep_only`] takes those out (or prunes them), then cuts whatever else
//! still points at such a page, so pruning unreachable objects removes it.

use std::collections::{HashMap, HashSet, VecDeque};

use lopdf::{Document as Pdf, Object, ObjectId};

use crate::save::is_page_node;

/// Make `pdf` reach no page but `kept` and its page tree (the catalog's
/// `/Pages`). References that a copy couldn't follow (null, see
/// `save::remap`) count as gone too. Returns what a reader should be told:
/// structure dropped rather than pruned.
pub(crate) fn keep_only(pdf: &mut Pdf, kept: &HashSet<ObjectId>) -> Vec<String> {
    let Some(catalog_id) = pdf
        .trailer
        .get(b"Root")
        .ok()
        .and_then(|r| r.as_reference().ok())
    else {
        return Vec::new();
    };
    let tree = pdf
        .get_dictionary(catalog_id)
        .ok()
        .and_then(|c| c.get(b"Pages").ok())
        .and_then(|p| p.as_reference().ok());
    let gone = Gone { kept, tree };
    let mut notes = Vec::new();

    let names = clean_named_dests(pdf, catalog_id, &gone);
    clean_links(pdf, &gone, &names);
    clean_open_action(pdf, catalog_id, &gone, &names);
    clean_fields(pdf, catalog_id, kept);
    if !prune_structure(pdf, catalog_id, &gone) {
        if let Ok(c) = pdf.get_dictionary_mut(catalog_id) {
            c.remove(b"StructTreeRoot");
            c.remove(b"MarkInfo");
        }
        notes.push(
            "The document's tags (its structure for screen readers) were dropped: they \
             couldn't be separated from the pages taken out."
                .to_string(),
        );
    }
    if drop_threads(pdf, catalog_id, &gone) {
        notes.push("Article threads through pages taken out were dropped.".to_string());
    }
    sweep(pdf, &gone);
    notes
}

/// What isn't kept.
struct Gone<'a> {
    kept: &'a HashSet<ObjectId>,
    tree: Option<ObjectId>,
}

impl Gone<'_> {
    /// A page, or a node of an old page tree, that isn't kept.
    fn page(&self, pdf: &Pdf, id: ObjectId) -> bool {
        Some(id) != self.tree && !self.kept.contains(&id) && is_page_node(pdf, id)
    }

    /// Whether `o` points at a page that isn't kept: a reference to one,
    /// or null where a copy dropped one.
    fn target(&self, pdf: &Pdf, o: &Object) -> bool {
        match o {
            Object::Reference(id) => self.page(pdf, *id),
            Object::Null => true,
            _ => false,
        }
    }

    /// Whether a destination (an array, a dictionary with `/D`, or a
    /// name in `names`) lands on a page that isn't kept, or nowhere.
    fn dest(&self, pdf: &Pdf, d: &Object, names: &HashMap<Vec<u8>, Object>) -> bool {
        let d = match d {
            Object::Reference(id) => match pdf.get_object(*id) {
                Ok(o) => o,
                Err(_) => return true,
            },
            o => o,
        };
        match d {
            Object::Array(a) => a.first().is_none_or(|p| self.target(pdf, p)),
            Object::Dictionary(dict) => dict
                .get(b"D")
                .ok()
                .is_some_and(|inner| self.dest(pdf, inner, names)),
            Object::String(n, _) | Object::Name(n) => match names.get(n) {
                Some(v) => self.dest(pdf, v, &HashMap::new()),
                None => true,
            },
            _ => false,
        }
    }
}

/// An object, through a reference.
fn deref<'a>(pdf: &'a Pdf, o: &'a Object) -> Option<&'a Object> {
    match o {
        Object::Reference(id) => pdf.get_object(*id).ok(),
        o => Some(o),
    }
}

/// Drop named destinations (`/Names /Dests`, the old `/Dests`) to pages
/// that aren't kept. The ones left, by name.
fn clean_named_dests(pdf: &mut Pdf, catalog_id: ObjectId, gone: &Gone) -> HashMap<Vec<u8>, Object> {
    let catalog = pdf.get_dictionary(catalog_id).cloned().unwrap_or_default();
    // The old way: a dictionary in the catalog.
    if let Ok(dests) = catalog.get(b"Dests") {
        let (id, dict) = match dests {
            Object::Reference(id) => (Some(*id), pdf.get_dictionary(*id).ok().cloned()),
            Object::Dictionary(d) => (None, Some(d.clone())),
            _ => (None, None),
        };
        if let Some(mut dict) = dict {
            let dead: Vec<Vec<u8>> = dict
                .iter()
                .filter(|(_, v)| gone.dest(pdf, v, &HashMap::new()))
                .map(|(k, _)| k.clone())
                .collect();
            for k in dead {
                dict.remove(&k);
            }
            match id {
                Some(id) => {
                    pdf.objects.insert(id, Object::Dictionary(dict));
                }
                None => {
                    if let Ok(c) = pdf.get_dictionary_mut(catalog_id) {
                        c.set("Dests", dict);
                    }
                }
            }
        }
    }
    if let Some(tree) = catalog
        .get(b"Names")
        .ok()
        .and_then(|n| deref(pdf, n))
        .and_then(|n| n.as_dict().ok())
        .and_then(|n| n.get(b"Dests").ok())
        .cloned()
    {
        clean_tree(
            pdf,
            &tree,
            b"Names",
            &|pdf, v| gone.dest(pdf, v, &HashMap::new()),
            0,
        );
    }
    // What's left, for links that name their destination.
    let mut out = HashMap::new();
    let catalog = pdf.get_dictionary(catalog_id).cloned().unwrap_or_default();
    if let Some(d) = catalog
        .get(b"Dests")
        .ok()
        .and_then(|d| deref(pdf, d))
        .and_then(|d| d.as_dict().ok())
    {
        for (k, v) in d.iter() {
            out.insert(k.clone(), v.clone());
        }
    }
    if let Some(tree) = catalog
        .get(b"Names")
        .ok()
        .and_then(|n| deref(pdf, n))
        .and_then(|n| n.as_dict().ok())
        .and_then(|n| n.get(b"Dests").ok())
    {
        collect_tree(pdf, tree, &mut out, 0);
    }
    out
}

/// Remove from a name tree (`key` `Names`) or number tree (`Nums`) the
/// entries `dead` says to, in place.
fn clean_tree(
    pdf: &mut Pdf,
    node: &Object,
    key: &[u8],
    dead: &dyn Fn(&Pdf, &Object) -> bool,
    depth: u32,
) {
    let Object::Reference(id) = node else {
        return;
    };
    if depth > 32 {
        return;
    }
    let Ok(mut dict) = pdf.get_dictionary(*id).cloned() else {
        return;
    };
    if let Some(pairs) = dict
        .get(key)
        .ok()
        .and_then(|p| deref(pdf, p))
        .and_then(|p| p.as_array().ok())
        .cloned()
    {
        let kept: Vec<Object> = pairs
            .chunks(2)
            .filter(|c| c.len() == 2 && !dead(pdf, &c[1]))
            .flat_map(|c| c.to_vec())
            .collect();
        if kept.len() != pairs.len() {
            dict.set(key.to_vec(), Object::Array(kept));
            pdf.objects.insert(*id, Object::Dictionary(dict.clone()));
        }
    }
    if let Some(kids) = dict
        .get(b"Kids")
        .ok()
        .and_then(|k| deref(pdf, k))
        .and_then(|k| k.as_array().ok())
        .cloned()
    {
        for kid in &kids {
            clean_tree(pdf, kid, key, dead, depth + 1);
        }
    }
}

fn collect_tree(pdf: &Pdf, node: &Object, out: &mut HashMap<Vec<u8>, Object>, depth: u32) {
    let Some(dict) = deref(pdf, node).and_then(|n| n.as_dict().ok()) else {
        return;
    };
    if depth > 32 {
        return;
    }
    if let Some(pairs) = dict
        .get(b"Names")
        .ok()
        .and_then(|p| deref(pdf, p))
        .and_then(|p| p.as_array().ok())
    {
        for c in pairs.chunks(2) {
            if let [Object::String(k, _), v] = c {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    if let Some(kids) = dict
        .get(b"Kids")
        .ok()
        .and_then(|k| deref(pdf, k))
        .and_then(|k| k.as_array().ok())
    {
        for kid in kids {
            collect_tree(pdf, kid, out, depth + 1);
        }
    }
}

/// A copied page's links whose target wasn't copied (null: see
/// `save::remap`) go, as they'd lead nowhere. For copies that aren't
/// followed by [`keep_only`] (an incremental save's new pages).
pub(crate) fn drop_dead_links(pdf: &Pdf, page: &mut lopdf::Dictionary) {
    fn dead(pdf: &Pdf, d: &Object) -> bool {
        match deref(pdf, d) {
            Some(Object::Array(a)) => matches!(a.first(), Some(Object::Null)),
            Some(Object::Dictionary(dict)) => dict.get(b"D").ok().is_some_and(|d| dead(pdf, d)),
            _ => false,
        }
    }
    let Some(annots) = page
        .get(b"Annots")
        .ok()
        .and_then(|a| deref(pdf, a))
        .and_then(|a| a.as_array().ok())
        .cloned()
    else {
        return;
    };
    let kept: Vec<Object> = annots
        .iter()
        .filter(|a| {
            let Some(d) = deref(pdf, a).and_then(|d| d.as_dict().ok()) else {
                return true;
            };
            let to = d.get(b"Dest").ok().or_else(|| {
                deref(pdf, d.get(b"A").ok()?)?
                    .as_dict()
                    .ok()?
                    .get(b"D")
                    .ok()
            });
            !to.is_some_and(|t| dead(pdf, t))
        })
        .cloned()
        .collect();
    if kept.len() != annots.len() {
        if kept.is_empty() {
            page.remove(b"Annots");
        } else {
            page.set("Annots", Object::Array(kept));
        }
    }
}

/// Every kept page's annotations: the list, as the page has it.
fn kept_pages(pdf: &Pdf, gone: &Gone) -> Vec<ObjectId> {
    let mut pages: Vec<ObjectId> = gone.kept.iter().copied().collect();
    pages.sort();
    pages.retain(|&p| pdf.get_dictionary(p).is_ok());
    pages
}

fn annots_of(pdf: &Pdf, page: ObjectId) -> Vec<Object> {
    pdf.get_dictionary(page)
        .ok()
        .and_then(|d| d.get(b"Annots").ok())
        .and_then(|a| deref(pdf, a))
        .and_then(|a| a.as_array().ok())
        .cloned()
        .unwrap_or_default()
}

/// Links on kept pages to pages that aren't kept go: they'd lead nowhere.
fn clean_links(pdf: &mut Pdf, gone: &Gone, names: &HashMap<Vec<u8>, Object>) {
    for page in kept_pages(pdf, gone) {
        let annots = annots_of(pdf, page);
        let kept: Vec<Object> = annots
            .iter()
            .filter(|a| {
                let Some(d) = deref(pdf, a).and_then(|d| d.as_dict().ok()) else {
                    return true;
                };
                let link = d
                    .get(b"Subtype")
                    .ok()
                    .and_then(|s| s.as_name().ok())
                    .is_some_and(|s| s == b"Link");
                if !link {
                    return true;
                }
                let dest = d.get(b"Dest").ok().cloned().or_else(|| {
                    let action = deref(pdf, d.get(b"A").ok()?)?.as_dict().ok()?;
                    let goto = action
                        .get(b"S")
                        .ok()
                        .and_then(|s| s.as_name().ok())
                        .is_some_and(|s| s == b"GoTo");
                    goto.then(|| action.get(b"D").ok().cloned()).flatten()
                });
                !dest.is_some_and(|d| gone.dest(pdf, &d, names))
            })
            .cloned()
            .collect();
        if kept.len() != annots.len()
            && let Ok(d) = pdf.get_dictionary_mut(page)
        {
            if kept.is_empty() {
                d.remove(b"Annots");
            } else {
                d.set("Annots", Object::Array(kept));
            }
        }
    }
}

/// The open action, when it goes to a page that isn't kept.
fn clean_open_action(
    pdf: &mut Pdf,
    catalog_id: ObjectId,
    gone: &Gone,
    names: &HashMap<Vec<u8>, Object>,
) {
    let Some(action) = pdf
        .get_dictionary(catalog_id)
        .ok()
        .and_then(|c| c.get(b"OpenAction").ok())
        .cloned()
    else {
        return;
    };
    let dead = match deref(pdf, &action) {
        Some(Object::Array(_)) => gone.dest(pdf, &action, names),
        Some(Object::Dictionary(a)) => {
            let goto = a
                .get(b"S")
                .ok()
                .and_then(|s| s.as_name().ok())
                .is_some_and(|s| s == b"GoTo");
            goto && a.get(b"D").ok().is_some_and(|d| gone.dest(pdf, d, names))
        }
        _ => false,
    };
    if dead && let Ok(c) = pdf.get_dictionary_mut(catalog_id) {
        c.remove(b"OpenAction");
    }
}

/// Widgets on pages that aren't kept leave their fields; fields left with
/// no widgets go.
fn clean_fields(pdf: &mut Pdf, catalog_id: ObjectId, kept: &HashSet<ObjectId>) {
    let Some(form) = pdf
        .get_dictionary(catalog_id)
        .ok()
        .and_then(|c| c.get(b"AcroForm").ok())
        .cloned()
    else {
        return;
    };
    let on_kept: HashSet<ObjectId> = kept
        .iter()
        .flat_map(|&p| annots_of(pdf, p))
        .filter_map(|a| a.as_reference().ok())
        .collect();
    let (form_id, mut form_dict) = match &form {
        Object::Reference(id) => match pdf.get_dictionary(*id) {
            Ok(d) => (Some(*id), d.clone()),
            Err(_) => return,
        },
        Object::Dictionary(d) => (None, d.clone()),
        _ => return,
    };
    let Some(fields) = form_dict
        .get(b"Fields")
        .ok()
        .and_then(|f| deref(pdf, f))
        .and_then(|f| f.as_array().ok())
        .cloned()
    else {
        return;
    };
    let mut removed = HashSet::new();
    let left: Vec<Object> = fields
        .into_iter()
        .filter(|f| match f.as_reference() {
            Ok(id) => keep_field(pdf, id, &on_kept, &mut removed, 0),
            Err(_) => true,
        })
        .collect();
    if removed.is_empty() {
        return;
    }
    form_dict.set("Fields", Object::Array(left));
    // The calculation order names fields too.
    if let Some(co) = form_dict
        .get(b"CO")
        .ok()
        .and_then(|c| deref(pdf, c))
        .and_then(|c| c.as_array().ok())
        .cloned()
    {
        let co: Vec<Object> = co
            .into_iter()
            .filter(|f| !f.as_reference().is_ok_and(|id| removed.contains(&id)))
            .collect();
        form_dict.set("CO", Object::Array(co));
    }
    match form_id {
        Some(id) => {
            pdf.objects.insert(id, Object::Dictionary(form_dict));
        }
        None => {
            if let Ok(c) = pdf.get_dictionary_mut(catalog_id) {
                c.set("AcroForm", form_dict);
            }
        }
    }
}

/// Whether field (or widget) `id` stays: a widget if it's on a kept page,
/// a field if any of its widgets stay (or it never had any). Removed ones
/// go into `removed`.
fn keep_field(
    pdf: &mut Pdf,
    id: ObjectId,
    on_kept: &HashSet<ObjectId>,
    removed: &mut HashSet<ObjectId>,
    depth: u32,
) -> bool {
    let Ok(dict) = pdf.get_dictionary(id).cloned() else {
        return false;
    };
    if depth > 32 {
        return true;
    }
    let widget = dict
        .get(b"Subtype")
        .ok()
        .and_then(|s| s.as_name().ok())
        .is_some_and(|s| s == b"Widget");
    let kids = dict
        .get(b"Kids")
        .ok()
        .and_then(|k| deref(pdf, k))
        .and_then(|k| k.as_array().ok())
        .cloned();
    let keep = match kids {
        Some(kids) if !kids.is_empty() => {
            let left: Vec<Object> = kids
                .into_iter()
                .filter(|k| match k.as_reference() {
                    Ok(kid) => keep_field(pdf, kid, on_kept, removed, depth + 1),
                    Err(_) => true,
                })
                .collect();
            let any = !left.is_empty();
            if let Ok(d) = pdf.get_dictionary_mut(id) {
                d.set("Kids", Object::Array(left));
            }
            any
        }
        _ if widget => on_kept.contains(&id),
        _ => true,
    };
    if !keep {
        removed.insert(id);
    }
    keep
}

/// Prune the structure tree of what's on pages that aren't kept. False
/// when it can't be done cleanly; the caller drops the tree.
fn prune_structure(pdf: &mut Pdf, catalog_id: ObjectId, gone: &Gone) -> bool {
    let Some(root) = pdf
        .get_dictionary(catalog_id)
        .ok()
        .and_then(|c| c.get(b"StructTreeRoot").ok())
        .and_then(|r| r.as_reference().ok())
    else {
        return true;
    };
    let mut removed: HashSet<ObjectId> = HashSet::new();
    let mut seen = HashSet::new();
    let Some(k) = pdf
        .get_dictionary(root)
        .ok()
        .and_then(|r| r.get(b"K").ok())
        .cloned()
    else {
        return true;
    };
    let pruned = prune_kids(pdf, &k, None, gone, &mut removed, &mut seen, 0);
    let Some(pruned) = pruned else {
        return false;
    };
    if removed.is_empty() {
        return true;
    }
    if let Ok(r) = pdf.get_dictionary_mut(root) {
        r.set("K", pruned);
    }
    let root_dict = pdf.get_dictionary(root).cloned().unwrap_or_default();
    // The parent tree: each page's marked content (an array of elements)
    // or an object's element. Entries for what's gone go.
    if let Ok(pt) = root_dict.get(b"ParentTree") {
        let pt = pt.clone();
        // An entry for an object's element, or a page's whose elements
        // are all gone.
        let dead = |pdf: &Pdf, v: &Object| {
            let gone_elem = |o: &Object| match o {
                Object::Reference(id) => removed.contains(id),
                Object::Null => true,
                _ => false,
            };
            match v {
                Object::Reference(id) if removed.contains(id) => true,
                v => deref(pdf, v)
                    .and_then(|a| a.as_array().ok())
                    .is_some_and(|a| !a.is_empty() && a.iter().all(gone_elem)),
            }
        };
        clean_tree(pdf, &pt, b"Nums", &dead, 0);
        // Arrays (a page's) can hold some that are gone among some that
        // aren't: those become null.
        null_removed_in_tree(pdf, &pt, &removed, 0);
    }
    if let Ok(ids) = root_dict.get(b"IDTree") {
        let ids = ids.clone();
        clean_tree(
            pdf,
            &ids,
            b"Names",
            &|_, v| v.as_reference().is_ok_and(|id| removed.contains(&id)),
            0,
        );
    }
    true
}

/// A structure element's kids (`k`) with what's on pages that aren't kept
/// taken out. `page` is the element's page, which an MCID is on. None
/// when the tree isn't shaped as it should be.
fn prune_kids(
    pdf: &mut Pdf,
    k: &Object,
    page: Option<ObjectId>,
    gone: &Gone,
    removed: &mut HashSet<ObjectId>,
    seen: &mut HashSet<ObjectId>,
    depth: u32,
) -> Option<Object> {
    if depth > 64 {
        return None;
    }
    let on_gone = |pdf: &Pdf, p: Option<&Object>, inherited: Option<ObjectId>| match p {
        Some(o) => gone.target(pdf, o),
        None => inherited.is_some_and(|id| gone.page(pdf, id)),
    };
    match k {
        Object::Array(items) => {
            let mut left = Vec::new();
            for item in items {
                let kept = prune_kids(pdf, item, page, gone, removed, seen, depth + 1)?;
                if !matches!(kept, Object::Null) {
                    left.push(kept);
                }
            }
            Some(Object::Array(left))
        }
        // Marked content on the element's page.
        Object::Integer(_) => Some(if on_gone(pdf, None, page) {
            Object::Null
        } else {
            k.clone()
        }),
        // Marked content (MCR) or an object (OBJR) on a page of its own.
        Object::Dictionary(d) => {
            let dead = on_gone(pdf, d.get(b"Pg").ok(), page)
                || d.get(b"Obj").ok().is_some_and(|o| gone.target(pdf, o));
            Some(if dead { Object::Null } else { k.clone() })
        }
        Object::Reference(id) => {
            let Ok(Object::Dictionary(d)) = pdf.get_object(*id).cloned() else {
                return Some(k.clone());
            };
            let is_ref = d
                .get(b"Type")
                .ok()
                .and_then(|t| t.as_name().ok())
                .is_some_and(|t| t == b"MCR" || t == b"OBJR");
            if is_ref {
                let dead = on_gone(pdf, d.get(b"Pg").ok(), page)
                    || d.get(b"Obj").ok().is_some_and(|o| gone.target(pdf, o));
                return Some(if dead { Object::Null } else { k.clone() });
            }
            if !seen.insert(*id) {
                return Some(k.clone());
            }
            // A structure element: its page, then its kids.
            let own = d.get(b"Pg").ok().cloned();
            let own_gone = own.as_ref().is_some_and(|p| gone.target(pdf, p));
            let Ok(kids) = d.get(b"K").cloned() else {
                if own_gone {
                    removed.insert(*id);
                    return Some(Object::Null);
                }
                return Some(k.clone());
            };
            let page = own.as_ref().and_then(|p| p.as_reference().ok()).or(page);
            let had = !is_empty(&kids);
            let left = prune_kids(pdf, &kids, page, gone, removed, seen, depth + 1)?;
            if had && is_empty(&left) {
                removed.insert(*id);
                return Some(Object::Null);
            }
            if let Ok(e) = pdf.get_dictionary_mut(*id) {
                e.set("K", left);
                // Its page is gone, but it has content on others, each
                // naming its own (an MCID on its page went with it).
                if own_gone {
                    e.remove(b"Pg");
                }
            }
            Some(k.clone())
        }
        _ => Some(k.clone()),
    }
}

fn is_empty(o: &Object) -> bool {
    match o {
        Object::Array(a) => a.iter().all(is_empty),
        Object::Null => true,
        _ => false,
    }
}

/// In a number tree's values, null each element in `removed`.
fn null_removed_in_tree(pdf: &mut Pdf, node: &Object, removed: &HashSet<ObjectId>, depth: u32) {
    let Object::Reference(id) = node else {
        return;
    };
    if depth > 32 {
        return;
    }
    let Ok(mut dict) = pdf.get_dictionary(*id).cloned() else {
        return;
    };
    let null = |o: &Object| match o {
        Object::Reference(r) if removed.contains(r) => Object::Null,
        o => o.clone(),
    };
    if let Ok(Object::Array(nums)) = dict.get(b"Nums").cloned() {
        let mut changed = false;
        let mut out = nums.clone();
        for v in out.iter_mut().skip(1).step_by(2) {
            match v {
                Object::Array(a) => {
                    let n: Vec<Object> = a.iter().map(null).collect();
                    if n != *a {
                        *a = n;
                        changed = true;
                    }
                }
                Object::Reference(r) => {
                    if let Ok(Object::Array(a)) = pdf.get_object(*r).cloned() {
                        let n: Vec<Object> = a.iter().map(null).collect();
                        if n != a {
                            pdf.objects.insert(*r, Object::Array(n));
                        }
                    }
                }
                _ => {}
            }
        }
        if changed {
            dict.set("Nums", Object::Array(out));
            pdf.objects.insert(*id, Object::Dictionary(dict.clone()));
        }
    }
    if let Ok(Object::Array(kids)) = dict.get(b"Kids").cloned() {
        for kid in &kids {
            null_removed_in_tree(pdf, kid, removed, depth + 1);
        }
    }
}

/// Article threads with a bead on a page that isn't kept go, and the kept
/// pages' beads of them. Whether any did.
fn drop_threads(pdf: &mut Pdf, catalog_id: ObjectId, gone: &Gone) -> bool {
    let Some(threads) = pdf
        .get_dictionary(catalog_id)
        .ok()
        .and_then(|c| c.get(b"Threads").ok())
        .and_then(|t| deref(pdf, t))
        .and_then(|t| t.as_array().ok())
        .cloned()
    else {
        return false;
    };
    let mut dropped = HashSet::new();
    let mut left = Vec::new();
    for t in threads {
        let Ok(tid) = t.as_reference() else {
            left.push(t);
            continue;
        };
        // Walk the beads from the first, around the ring.
        let first = pdf
            .get_dictionary(tid)
            .ok()
            .and_then(|d| d.get(b"F").ok())
            .and_then(|f| f.as_reference().ok());
        let mut bead = first;
        let mut through_gone = false;
        let mut steps = 0;
        while let Some(b) = bead {
            let Ok(d) = pdf.get_dictionary(b) else { break };
            if d.get(b"P").ok().is_some_and(|p| gone.target(pdf, p)) {
                through_gone = true;
                break;
            }
            bead = d.get(b"N").ok().and_then(|n| n.as_reference().ok());
            steps += 1;
            if bead == first || steps > 10_000 {
                break;
            }
        }
        if through_gone {
            dropped.insert(tid);
        } else {
            left.push(t);
        }
    }
    if dropped.is_empty() {
        return false;
    }
    for page in kept_pages(pdf, gone) {
        let beads = pdf
            .get_dictionary(page)
            .ok()
            .and_then(|d| d.get(b"B").ok())
            .and_then(|b| deref(pdf, b))
            .and_then(|b| b.as_array().ok())
            .cloned();
        let Some(beads) = beads else { continue };
        let kept: Vec<Object> = beads
            .into_iter()
            .filter(|b| {
                !b.as_reference()
                    .ok()
                    .and_then(|b| pdf.get_dictionary(b).ok())
                    .and_then(|d| d.get(b"T").ok())
                    .and_then(|t| t.as_reference().ok())
                    .is_some_and(|t| dropped.contains(&t))
            })
            .collect();
        if let Ok(d) = pdf.get_dictionary_mut(page) {
            if kept.is_empty() {
                d.remove(b"B");
            } else {
                d.set("B", Object::Array(kept));
            }
        }
    }
    if let Ok(c) = pdf.get_dictionary_mut(catalog_id) {
        if left.is_empty() {
            c.remove(b"Threads");
        } else {
            c.set("Threads", Object::Array(left));
        }
    }
    true
}

/// Whatever still points at a page that isn't kept, anywhere the file
/// reaches, points at nothing (null) instead.
fn sweep(pdf: &mut Pdf, gone: &Gone) {
    let mut queue: VecDeque<ObjectId> = VecDeque::new();
    refs(&Object::Dictionary(pdf.trailer.clone()), &mut |id| {
        queue.push_back(id)
    });
    let mut seen = HashSet::new();
    while let Some(id) = queue.pop_front() {
        if !seen.insert(id) || gone.page(pdf, id) {
            continue;
        }
        let Some(obj) = pdf.objects.get(&id) else {
            continue;
        };
        let mut hits = false;
        refs(obj, &mut |r| hits |= gone.page(pdf, r));
        if hits {
            let mut obj = obj.clone();
            cut(&mut obj, &|r| gone.page(pdf, r));
            pdf.objects.insert(id, obj);
        }
        if let Some(obj) = pdf.objects.get(&id) {
            refs(obj, &mut |r| queue.push_back(r));
        }
    }
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

/// Null each reference in `o` that `dead` says to.
fn cut(o: &mut Object, dead: &dyn Fn(ObjectId) -> bool) {
    match o {
        Object::Reference(id) if dead(*id) => *o = Object::Null,
        Object::Array(a) => a.iter_mut().for_each(|o| cut(o, dead)),
        Object::Dictionary(d) => d.iter_mut().for_each(|(_, o)| cut(o, dead)),
        Object::Stream(s) => s.dict.iter_mut().for_each(|(_, o)| cut(o, dead)),
        _ => {}
    }
}

/// The pages a page-tree rewrite didn't keep, for tests.
#[cfg(test)]
pub(crate) fn page_count_reachable(pdf: &Pdf) -> usize {
    let mut queue: VecDeque<ObjectId> = VecDeque::new();
    refs(&Object::Dictionary(pdf.trailer.clone()), &mut |id| {
        queue.push_back(id)
    });
    let mut seen = HashSet::new();
    let mut pages = 0;
    while let Some(id) = queue.pop_front() {
        if !seen.insert(id) {
            continue;
        }
        let Some(obj) = pdf.objects.get(&id) else {
            continue;
        };
        if let Object::Dictionary(d) = obj
            && d.get(b"Type").ok().and_then(|t| t.as_name().ok()) == Some(b"Page")
        {
            pages += 1;
        }
        refs(obj, &mut |r| queue.push_back(r));
    }
    pages
}

#[cfg(test)]
mod tests;
