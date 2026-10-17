//! Resources by content: a digest of what a resource is, so equal ones
//! are written once and named alike (§11.5).

use std::sync::Arc;

use lopdf::{Dictionary, Object, ObjectId};

use super::FontRes;

/// A resource's name by content: a digest of its objects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResId(pub u128);

impl ResId {
    /// The digest of `objects`, a resource's objects as
    /// [`ResBody::Objects`] holds them. A dictionary's entries count in key order, and a
    /// stream's data as it's held.
    pub fn of(objects: &[Object]) -> ResId {
        let mut bytes = Vec::new();
        for object in objects {
            feed(&mut bytes, object);
        }
        ResId(sheaf_core::checksum(&bytes))
    }
}

/// A resource content draws with: a font, an ExtGState, a property list,
/// an image or a form.
#[derive(Debug, Clone, PartialEq)]
pub struct Res {
    pub id: ResId,
    pub body: ResBody,
}

/// What a resource is.
#[derive(Debug, Clone, PartialEq)]
pub enum ResBody {
    /// Its objects: the resource first, then those it reaches, streams
    /// decoded. A reference `(i, 0)` among them is to the `i`th.
    Objects(Arc<[Object]>),
    /// An object the caller has already written, by its number in the
    /// document being written: a page made a form, say. Transitional
    /// until slice 9's emitter, whose forms and images are resources by
    /// content too.
    Written(ObjectId),
}

impl Res {
    /// The resource `objects` are ([`ResBody::Objects`]), named by them.
    pub fn of(objects: impl Into<Arc<[Object]>>) -> Res {
        let objects = objects.into();
        Res {
            id: ResId::of(&objects),
            body: ResBody::Objects(objects),
        }
    }

    /// A dictionary that reaches nothing else: an ExtGState or a property
    /// list.
    pub fn dict(dict: Dictionary) -> Res {
        Res::of([Object::Dictionary(dict)])
    }

    /// Object `id`, already written: named by its number.
    pub fn written(id: ObjectId) -> Res {
        Res {
            id: ResId::of(&[Object::Reference(id)]),
            body: ResBody::Written(id),
        }
    }
}

impl From<FontRes> for Res {
    fn from(font: FontRes) -> Res {
        Res {
            id: font.id,
            body: ResBody::Objects(font.objects),
        }
    }
}

/// `object` written into `out` so that two objects write the same bytes
/// only when they're equal: each part tagged with its kind, and each run
/// of bytes and each list with its length.
fn feed(out: &mut Vec<u8>, object: &Object) {
    match object {
        Object::Null => out.push(b'n'),
        Object::Boolean(b) => out.extend([b'b', u8::from(*b)]),
        Object::Integer(i) => {
            out.push(b'i');
            out.extend(i.to_le_bytes());
        }
        Object::Real(r) => {
            out.push(b'r');
            out.extend(r.to_bits().to_le_bytes());
        }
        Object::Name(name) => {
            out.push(b'/');
            run(out, name);
        }
        Object::String(s, _) => {
            out.push(b'(');
            run(out, s);
        }
        Object::Array(items) => {
            out.push(b'[');
            out.extend((items.len() as u64).to_le_bytes());
            for item in items {
                feed(out, item);
            }
        }
        Object::Dictionary(d) => {
            out.push(b'<');
            dict(out, d);
        }
        Object::Stream(s) => {
            out.push(b's');
            dict(out, &s.dict);
            run(out, &s.content);
        }
        Object::Reference((number, generation)) => {
            out.push(b'R');
            out.extend(number.to_le_bytes());
            out.extend(generation.to_le_bytes());
        }
    }
}

fn run(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend((bytes.len() as u64).to_le_bytes());
    out.extend(bytes);
}

fn dict(out: &mut Vec<u8>, dict: &Dictionary) {
    let mut entries: Vec<_> = dict.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    out.extend((entries.len() as u64).to_le_bytes());
    for (key, value) in entries {
        run(out, key);
        feed(out, value);
    }
}
