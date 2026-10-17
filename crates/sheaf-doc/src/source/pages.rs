//! The page table: every page the tree lists, numbered as PDFium numbers
//! them, with what each inherits. Walked when a source opens, without
//! reading any page's content.
//!
//! PDFium renders pages by index (until slice 13), so the walk follows
//! its traversal: depth first through `/Kids` from the catalog's `/Pages`,
//! a kid with `/Kids` a node whatever its `/Type`, any other kid a page. A
//! page object listed twice is two pages, which share what they hold. The
//! walk stops at the root's `/Count` when PDFium counts by it, since
//! PDFium loads no page past it. The inheritable keys come down the
//! `/Parent` chain from the page, as PDFium's `GetPageAttr` reads them,
//! which needn't be the path the walk took to the page, so a page object's
//! facts are the same wherever it's listed.
//!
//! Where PDFium stops quietly, the walk fails: a node met again on its
//! own path, a `/Parent` chain that loops, and a tree past the caps
//! ([`PageTreeLimits`]) are [`PageTreeError`]s. It's iterative, so a deep
//! tree is never deep on the stack.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use lopdf::{Dictionary, Error, LazyPdf, Object, ObjectId};

use super::{OpenError, PageTreeError, PageTreeLimits};
use crate::geom::{Geom, Rect};

/// A page, as the tree lists it.
#[derive(Debug, Clone)]
pub struct PageFacts {
    pub id: ObjectId,
    /// Its `/Parent`, which the inheritable keys come down.
    pub parent: Option<Arc<Parent>>,
    /// Its `/Resources`, else the nearest on its `/Parent` chain, as
    /// written: a reference stays one.
    pub resources: Option<Arc<Object>>,
    pub geom: Geom,
    /// The streams its `/Contents` names: one, or a direct array's. A
    /// reference to an array stays that one reference, since telling the
    /// two apart reads the stream it usually is.
    pub contents: Arc<[ObjectId]>,
    /// Its `/Annots` entries in order, as written (a reference each, in a
    /// well-formed file): an annotation's slot is its index here. Pages
    /// whose `/Annots` names one array share it.
    pub annots: Arc<[Object]>,
    pub struct_parents: Option<i64>,
    /// How often the tree listed this page object before: 0 the first
    /// time, 1 the second.
    pub repeat: u32,
}

impl PageFacts {
    /// A kid that names no dictionary, which keeps its place in the tree,
    /// as PDFium's traversal and lopdf's `get_pages` keep it: a Letter
    /// page with nothing on it.
    fn stand_in(id: ObjectId) -> PageFacts {
        PageFacts {
            id,
            parent: None,
            resources: None,
            geom: Geom::read(None, None, 0),
            contents: Arc::from([]),
            annots: Arc::from([]),
            struct_parents: None,
            repeat: 0,
        }
    }

    /// The nodes on its `/Parent` chain, nearest first.
    pub fn parents(&self) -> impl Iterator<Item = ObjectId> + '_ {
        std::iter::successors(self.parent.as_deref(), |p| p.up.as_deref()).map(|p| p.id)
    }
}

/// A node on a page's `/Parent` chain, and the chain above it: pages
/// under one node share it.
#[derive(Debug)]
pub struct Parent {
    pub id: ObjectId,
    pub up: Option<Arc<Parent>>,
}

/// PDFium's `kPageMaxNum`: it counts pages by the root's `/Count` when
/// that's positive and below it.
const PDFIUM_PAGE_MAX: i64 = 0xFFFFF;

/// `pdf`'s pages.
pub(super) fn table(pdf: &LazyPdf, limits: &PageTreeLimits) -> Result<Vec<PageFacts>, OpenError> {
    let mut walk = Walk {
        pdf,
        limits: *limits,
        count: None,
        nodes: HashMap::new(),
        arrays: HashMap::new(),
        listed: HashMap::new(),
        pages: Vec::new(),
        visits: 0,
    };
    let root = match pdf.catalog()?.get(b"Pages") {
        Ok(root) => walk.dict_at(root)?,
        Err(_) => None,
    };
    let Some((id, root)) = root else {
        return Err(PageTreeError::NoPages.into());
    };
    walk.count = walk.counted(root)?;
    walk.walk(id, root)?;
    if walk.pages.is_empty() {
        return Err(PageTreeError::NoPages.into());
    }
    Ok(walk.pages)
}

/// What a node gives the pages under it by `/Parent`: the nearest value
/// of each inheritable key, on it or above it.
struct Inherited<'a> {
    chain: Arc<Parent>,
    /// Nodes on the chain.
    depth: usize,
    media: Option<&'a Object>,
    crop: Option<&'a Object>,
    rotate: Option<&'a Object>,
    resources: Option<Arc<Object>>,
}

/// A node being walked: its kids, and the next to visit.
struct Frame<'a> {
    id: ObjectId,
    kids: &'a [Object],
    next: usize,
}

struct Walk<'a> {
    pdf: &'a LazyPdf,
    limits: PageTreeLimits,
    /// The pages PDFium counts by the root's `/Count`, when it does.
    count: Option<usize>,
    /// What each node met on a `/Parent` chain gives.
    nodes: HashMap<ObjectId, Rc<Inherited<'a>>>,
    /// The `/Annots` arrays read, by the reference pages name them by.
    arrays: HashMap<ObjectId, Arc<[Object]>>,
    /// Each page object listed: where the table first has it, and how
    /// often it's been listed.
    listed: HashMap<ObjectId, (usize, u32)>,
    pages: Vec<PageFacts>,
    /// Nodes visited under the root.
    visits: usize,
}

impl<'a> Walk<'a> {
    /// The tree under the root `/Pages`, `id`. A root without `/Kids` is
    /// the one page, as PDFium reads it.
    fn walk(&mut self, id: ObjectId, root: &'a Dictionary) -> Result<(), OpenError> {
        if !root.has(b"Kids") {
            return self.page(id, root);
        }
        let mut stack = vec![Frame {
            id,
            kids: self.kids(root)?,
            next: 0,
        }];
        let mut path = HashSet::from([id]);
        while let Some(frame) = stack.last_mut() {
            if self.count.is_some_and(|count| self.pages.len() >= count) {
                break;
            }
            let Some(kid) = frame.kids.get(frame.next) else {
                path.remove(&frame.id);
                stack.pop();
                continue;
            };
            frame.next += 1;
            // A dictionary written in place has no object number: lopdf's
            // get_pages skips it too.
            let Ok(id) = kid.as_reference() else {
                continue;
            };
            // A page listed again: the facts it was read with, shared.
            if let Some(&(first, _)) = self.listed.get(&id) {
                self.push(self.pages[first].clone())?;
                continue;
            }
            let Some(kid) = self.dict(id)? else {
                self.push(PageFacts::stand_in(id))?;
                continue;
            };
            if !kid.has(b"Kids") {
                self.page(id, kid)?;
                continue;
            }
            if path.contains(&id) {
                return Err(PageTreeError::Cycle.into());
            }
            if stack.len() >= self.limits.depth {
                return Err(PageTreeError::TooDeep.into());
            }
            self.visits += 1;
            if self.visits > self.limits.pages {
                return Err(PageTreeError::TooManyPages.into());
            }
            path.insert(id);
            stack.push(Frame {
                id,
                kids: self.kids(kid)?,
                next: 0,
            });
        }
        Ok(())
    }

    fn page(&mut self, id: ObjectId, dict: &'a Dictionary) -> Result<(), OpenError> {
        let up = match dict.get(b"Parent").and_then(Object::as_reference) {
            Ok(parent) => self.inherited(parent)?,
            Err(_) => None,
        };
        let up = up.as_deref();
        let media = self.found(dict, b"MediaBox")?.or(up.and_then(|u| u.media));
        let crop = self.found(dict, b"CropBox")?.or(up.and_then(|u| u.crop));
        let rotate = self.found(dict, b"Rotate")?.or(up.and_then(|u| u.rotate));
        let resources = match self.found(dict, b"Resources")? {
            Some(_) => dict.get(b"Resources").ok().cloned().map(Arc::new),
            None => up.and_then(|u| u.resources.clone()),
        };
        let degrees = match rotate {
            Some(Object::Integer(i)) => *i,
            Some(Object::Real(r)) => *r as i64,
            _ => 0,
        };
        let page_box = |key: &[u8]| self.rect(self.found(dict, key)?);
        let user_unit = match self.found(dict, b"UserUnit")? {
            Some(&Object::Integer(i)) if i > 0 => i as f32,
            Some(&Object::Real(r)) if r > 0.0 => r as f32,
            _ => 1.0,
        };
        let geom = Geom {
            trim: page_box(b"TrimBox")?,
            bleed: page_box(b"BleedBox")?,
            art: page_box(b"ArtBox")?,
            user_unit,
            ..Geom::read(self.rect(media)?, self.rect(crop)?, degrees)
        };
        let contents = crate::content::contents(dict);
        let annots = self.annots(dict)?;
        let page = PageFacts {
            id,
            parent: up.map(|u| u.chain.clone()),
            resources,
            geom,
            contents: contents.into(),
            annots,
            struct_parents: dict.get(b"StructParents").and_then(Object::as_i64).ok(),
            repeat: 0,
        };
        self.push(page)
    }

    /// `page` added to the table, numbered by how often its object was
    /// listed before.
    fn push(&mut self, mut page: PageFacts) -> Result<(), OpenError> {
        if self.pages.len() >= self.limits.pages {
            return Err(PageTreeError::TooManyPages.into());
        }
        let (_, listed) = self.listed.entry(page.id).or_insert((self.pages.len(), 0));
        page.repeat = *listed;
        *listed += 1;
        self.pages.push(page);
        Ok(())
    }

    /// How many pages PDFium counts by the root's `/Count`, as its
    /// `CountPages` reads it: None when it counts the tree instead.
    fn counted(&self, root: &'a Dictionary) -> Result<Option<usize>, OpenError> {
        let count = match self.found(root, b"Count")? {
            Some(&Object::Integer(i)) => i,
            Some(&Object::Real(r)) => r as i64,
            _ => 0,
        };
        Ok((1..PDFIUM_PAGE_MAX)
            .contains(&count)
            .then_some(count as usize))
    }

    /// A page's `/Annots` entries: an array that pages name by reference
    /// is read once, and shared.
    fn annots(&mut self, dict: &'a Dictionary) -> Result<Arc<[Object]>, OpenError> {
        let shared = dict.get(b"Annots").and_then(Object::as_reference).ok();
        if let Some(annots) = shared.and_then(|id| self.arrays.get(&id)) {
            return Ok(annots.clone());
        }
        let annots: Arc<[Object]> = match self.found(dict, b"Annots")? {
            Some(Object::Array(annots)) => annots.as_slice().into(),
            _ => Arc::from([]),
        };
        if let Some(id) = shared {
            self.arrays.insert(id, annots.clone());
        }
        Ok(annots)
    }

    /// What node `id` gives the pages under it, walking its `/Parent`
    /// chain up to a node already met, then back down. None when `id`
    /// isn't a dictionary.
    fn inherited(&mut self, id: ObjectId) -> Result<Option<Rc<Inherited<'a>>>, OpenError> {
        let mut chain = Vec::new();
        let mut on_chain = HashSet::new();
        let mut at = Some(id);
        let mut above = None;
        while let Some(node) = at {
            if let Some(known) = self.nodes.get(&node) {
                above = Some(known.clone());
                break;
            }
            if !on_chain.insert(node) {
                return Err(PageTreeError::ParentLoop.into());
            }
            if chain.len() >= self.limits.depth {
                return Err(PageTreeError::TooDeep.into());
            }
            let Some(dict) = self.dict(node)? else {
                break;
            };
            chain.push((node, dict));
            at = dict.get(b"Parent").and_then(Object::as_reference).ok();
        }
        for (node, dict) in chain.into_iter().rev() {
            let up = above.as_deref();
            let depth = up.map_or(0, |u| u.depth) + 1;
            if depth > self.limits.depth {
                return Err(PageTreeError::TooDeep.into());
            }
            let resources = match self.found(dict, b"Resources")? {
                Some(_) => dict.get(b"Resources").ok().cloned().map(Arc::new),
                None => up.and_then(|u| u.resources.clone()),
            };
            let node_inherits = Rc::new(Inherited {
                chain: Arc::new(Parent {
                    id: node,
                    up: up.map(|u| u.chain.clone()),
                }),
                depth,
                media: self.found(dict, b"MediaBox")?.or(up.and_then(|u| u.media)),
                crop: self.found(dict, b"CropBox")?.or(up.and_then(|u| u.crop)),
                rotate: self.found(dict, b"Rotate")?.or(up.and_then(|u| u.rotate)),
                resources,
            });
            self.nodes.insert(node, node_inherits.clone());
            above = Some(node_inherits);
        }
        Ok(above)
    }

    /// A node's kids: none when its `/Kids` isn't an array.
    fn kids(&self, node: &'a Dictionary) -> Result<&'a [Object], OpenError> {
        Ok(match self.found(node, b"Kids")? {
            Some(Object::Array(kids)) => kids,
            _ => &[],
        })
    }

    /// The value of `key` in `dict`, references followed: None when it's
    /// missing or names no object, as PDFium takes it.
    fn found(&self, dict: &'a Dictionary, key: &[u8]) -> Result<Option<&'a Object>, OpenError> {
        match dict.get(key) {
            Ok(value) => self.resolve(value),
            Err(_) => Ok(None),
        }
    }

    fn resolve(&self, value: &'a Object) -> Result<Option<&'a Object>, OpenError> {
        let pdf = self.pdf;
        match pdf.dereference(value) {
            Ok((_, object)) => Ok(Some(object)),
            Err(e) if fatal(&e) => Err(e.into()),
            Err(_) => Ok(None),
        }
    }

    /// The dictionary a reference names, with its id.
    fn dict_at(&self, entry: &'a Object) -> Result<Option<(ObjectId, &'a Dictionary)>, OpenError> {
        let Ok(id) = entry.as_reference() else {
            return Ok(None);
        };
        Ok(self.dict(id)?.map(|dict| (id, dict)))
    }

    /// Object `id`'s dictionary, or a stream's, as PDFium takes a kid.
    fn dict(&self, id: ObjectId) -> Result<Option<&'a Dictionary>, OpenError> {
        let pdf = self.pdf;
        match pdf.get(id) {
            Ok(Object::Dictionary(dict)) => Ok(Some(dict)),
            Ok(Object::Stream(stream)) => Ok(Some(&stream.dict)),
            Ok(_) => Ok(None),
            Err(e) if fatal(&e) => Err(e.into()),
            Err(_) => Ok(None),
        }
    }

    /// A box: four numbers, normalized. None for anything else.
    fn rect(&self, value: Option<&'a Object>) -> Result<Option<Rect>, OpenError> {
        let Some(Object::Array(numbers)) = value else {
            return Ok(None);
        };
        let [a, b, c, d] = numbers.as_slice() else {
            return Ok(None);
        };
        let mut corners = [0.0; 4];
        for (n, value) in corners.iter_mut().zip([a, b, c, d]) {
            *n = match self.resolve(value)? {
                Some(Object::Integer(i)) => *i as f32,
                Some(Object::Real(r)) => *r as f32,
                _ => 0.0,
            };
        }
        let [a, b, c, d] = corners;
        Ok(Some(Rect::new(a, b, c, d)))
    }
}

/// Whether reading an object failed in a way that fails the open: a cap
/// broken, a cycle of references, or an object that doesn't decrypt.
/// Any other failure is an object that isn't there, as PDFium takes it.
fn fatal(e: &Error) -> bool {
    matches!(
        e,
        Error::Limit(_) | Error::ReferenceCycle(_) | Error::Decryption(_)
    )
}
