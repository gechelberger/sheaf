//! What the old writers do with a [`ContentWriter`]'s
//! [`ResourceNeeds`]: write each resource into the lopdf document being
//! saved, once a save, and bind it in a resources dict. Transitional:
//! slice 9's emitter names and writes resources itself (§11.5).
//!
//! [`ContentWriter`]: super::ContentWriter

use std::collections::HashMap;

use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

use super::res::{Res, ResBody, ResId};
use super::writer::ResourceNeeds;

/// The objects a save has written for resources, by [`ResId`]: a font, an
/// ExtGState or a property list that many pages or marks draw with is one
/// object a save. For one document only.
#[derive(Debug, Default)]
pub struct ResObjects {
    written: HashMap<ResId, ObjectId>,
}

impl ResObjects {
    /// `res`'s object in `pdf`, written the first time it's asked for.
    pub fn object(&mut self, pdf: &mut Document, res: &Res) -> ObjectId {
        match &res.body {
            ResBody::Written(id) => *id,
            ResBody::Objects(objects) => *self
                .written
                .entry(res.id)
                .or_insert_with(|| add(pdf, objects)),
        }
    }

    /// A stream written once a save, however many pages it's on: the
    /// isolating `q` the old writers put before a page's content.
    pub fn stream(&mut self, pdf: &mut Document, dict: Dictionary, content: Vec<u8>) -> ObjectId {
        self.object(pdf, &Res::of([Stream::new(dict, content).into()]))
    }

    /// `needs` written into `pdf` and bound in `resources`, a resources
    /// dict about to be written: each under its name in its category. A
    /// category that's a reference is copied in first, so a dict other
    /// pages share is never changed.
    pub fn bind(&mut self, pdf: &mut Document, needs: &ResourceNeeds, resources: &mut Dictionary) {
        for need in &needs.0 {
            let id = self.object(pdf, &need.res);
            let key = need.kind.key();
            let mut category = match resources.get(key) {
                Ok(Object::Reference(r)) => pdf.get_dictionary(*r).cloned().unwrap_or_default(),
                Ok(Object::Dictionary(d)) => d.clone(),
                _ => Dictionary::new(),
            };
            category.set(need.name.clone(), id);
            resources.set(key, category);
        }
    }
}

/// `page`'s `/Resources`, a copy to change: a page shares its dict with
/// others by reference.
pub fn resources_of(pdf: &Document, page: &Dictionary) -> Dictionary {
    page.get(b"Resources")
        .ok()
        .and_then(|o| pdf.dereference(o).ok())
        .and_then(|(_, o)| o.as_dict().ok().cloned())
        .unwrap_or_default()
}

/// `objects`, a resource's ([`ResBody::Objects`]), added to `pdf`, each
/// with a number of its own and its streams compressed; the first's
/// number.
fn add(pdf: &mut Document, objects: &[Object]) -> ObjectId {
    let ids: Vec<ObjectId> = objects.iter().map(|_| pdf.new_object_id()).collect();
    for (object, &id) in objects.iter().zip(&ids) {
        let mut object = object.clone();
        renumber(&mut object, &ids);
        if let Object::Stream(stream) = &mut object {
            let _ = stream.compress();
        }
        pdf.objects.insert(id, object);
    }
    ids[0]
}

/// `object` with each reference `(i, 0)` made `ids[i]`.
fn renumber(object: &mut Object, ids: &[ObjectId]) {
    match object {
        Object::Reference(id) => *id = ids[id.0 as usize],
        Object::Array(items) => items.iter_mut().for_each(|o| renumber(o, ids)),
        Object::Dictionary(dict) => dict.iter_mut().for_each(|(_, o)| renumber(o, ids)),
        Object::Stream(stream) => stream.dict.iter_mut().for_each(|(_, o)| renumber(o, ids)),
        _ => {}
    }
}
