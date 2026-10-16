//! Optional content (layers): the groups in `/OCProperties`, listed in
//! the default configuration's `/Order`, and a way to change which are
//! shown for display. Visibility changes are never saved.

use std::collections::HashSet;

use lopdf::{Dictionary, Document as Pdf, Object, ObjectId};

use crate::attachments::{Objects, once, resolve};
use crate::file::pdf_text;

/// An optional content group: its object number in the file.
pub type LayerId = ObjectId;

/// One row of the layers list: a group, or a heading in `/Order`.
#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    /// The optional content group; None for a heading.
    pub id: Option<ObjectId>,
    pub name: String,
    /// Shown in the default configuration.
    pub on: bool,
    /// Listed in `/Locked`: the user may not change it.
    pub locked: bool,
    pub depth: u16,
}

/// The file's layers in display order: `/D /Order` if it has one (nested
/// arrays give depth; a string first in an array is a heading), then any
/// group the order leaves out. Empty when the file has no layers.
pub fn layers_in<P: Objects + ?Sized>(pdf: &P) -> Vec<Layer> {
    let Some(props) = oc_properties(pdf) else {
        return Vec::new();
    };
    let groups: Vec<ObjectId> = props
        .get(b"OCGs")
        .ok()
        .and_then(|o| resolve(pdf, o))
        .and_then(|o| o.as_array().ok())
        .map(|a| a.iter().filter_map(|o| o.as_reference().ok()).collect())
        .unwrap_or_default();
    let d = props
        .get(b"D")
        .ok()
        .and_then(|o| resolve(pdf, o))
        .and_then(|o| o.as_dict().ok());
    let ids = |key: &[u8]| -> HashSet<ObjectId> {
        d.and_then(|d| d.get(key).ok())
            .and_then(|o| resolve(pdf, o))
            .and_then(|o| o.as_array().ok())
            .map(|a| a.iter().filter_map(|o| o.as_reference().ok()).collect())
            .unwrap_or_default()
    };
    let base_off = d
        .and_then(|d| d.get(b"BaseState").ok())
        .and_then(|o| o.as_name().ok())
        == Some(b"OFF");
    let (on, off, locked) = (ids(b"ON"), ids(b"OFF"), ids(b"Locked"));
    let visible = |id: &ObjectId| {
        if base_off {
            on.contains(id)
        } else {
            !off.contains(id)
        }
    };
    let name = |id: ObjectId| {
        pdf.get_dictionary(id)
            .ok()
            .and_then(|g| g.get(b"Name").ok())
            .and_then(|o| resolve(pdf, o))
            .map(pdf_text)
            .unwrap_or_default()
    };

    let mut out = Vec::new();
    let mut listed = HashSet::new();
    let mut seen = HashSet::new();
    if let Some(order) = d
        .and_then(|d| d.get(b"Order").ok())
        .and_then(|o| once(pdf, &mut seen, o))
        .and_then(|o| o.as_array().ok())
    {
        walk_order(pdf, order, 0, &mut seen, &mut |id, label, depth| match id {
            Some(id) if groups.contains(&id) || pdf.get_dictionary(id).is_ok() => {
                if listed.insert(id) {
                    out.push(Layer {
                        id: Some(id),
                        name: name(id),
                        on: visible(&id),
                        locked: locked.contains(&id),
                        depth,
                    });
                }
            }
            Some(_) => {}
            None => out.push(Layer {
                id: None,
                name: label,
                on: true,
                locked: true,
                depth,
            }),
        });
    }
    for id in groups {
        if listed.insert(id) {
            out.push(Layer {
                id: Some(id),
                name: name(id),
                on: visible(&id),
                locked: locked.contains(&id),
                depth: 0,
            });
        }
    }
    out
}

/// `/Order`'s items below `depth`, each array reached by reference once
/// (an order may hold itself).
fn walk_order<P: Objects + ?Sized>(
    pdf: &P,
    items: &[Object],
    depth: u16,
    seen: &mut HashSet<ObjectId>,
    f: &mut impl FnMut(Option<ObjectId>, String, u16),
) {
    if depth > 32 {
        return;
    }
    let mut it = items.iter().peekable();
    // A label first in a nested array heads it.
    if depth > 0
        && let Some(o) = it.peek()
        && let Some(s) = resolve(pdf, o).filter(|o| o.as_str().is_ok())
    {
        f(None, pdf_text(s), depth - 1);
        it.next();
    }
    for o in it {
        match o {
            Object::Reference(id) => match pdf.get_object(*id) {
                Ok(Object::Array(a)) => {
                    if seen.insert(*id) {
                        walk_order(pdf, a, depth + 1, seen, f);
                    }
                }
                _ => f(Some(*id), String::new(), depth),
            },
            Object::Array(a) => walk_order(pdf, a, depth + 1, seen, f),
            _ => {}
        }
    }
}

fn oc_properties<P: Objects + ?Sized>(pdf: &P) -> Option<&Dictionary> {
    pdf.catalog()
        .ok()?
        .get(b"OCProperties")
        .ok()
        .and_then(|o| resolve(pdf, o))?
        .as_dict()
        .ok()
}

/// Make exactly `off` hidden in the default configuration (and every
/// other group shown), for rendering.
pub fn set_hidden(pdf: &mut Pdf, off: &HashSet<ObjectId>) {
    let Some(props) = oc_properties(pdf) else {
        return;
    };
    let groups: Vec<ObjectId> = props
        .get(b"OCGs")
        .ok()
        .and_then(|o| o.as_array().ok())
        .map(|a| a.iter().filter_map(|o| o.as_reference().ok()).collect())
        .unwrap_or_default();
    let d_ref = props.get(b"D").ok().and_then(|o| o.as_reference().ok());
    let refs = |set: Vec<ObjectId>| -> Object {
        Object::Array(set.into_iter().map(Object::Reference).collect())
    };
    let on: Vec<ObjectId> = groups
        .iter()
        .copied()
        .filter(|g| !off.contains(g))
        .collect();
    let hidden: Vec<ObjectId> = groups.iter().copied().filter(|g| off.contains(g)).collect();
    let update = |d: &mut Dictionary| {
        d.set("BaseState", Object::Name(b"ON".to_vec()));
        d.set("ON", refs(on.clone()));
        d.set("OFF", refs(hidden.clone()));
        // Usage-based auto states (/AS) would override the lists.
        d.remove(b"AS");
    };
    if let Some(id) = d_ref {
        if let Ok(d) = pdf.get_dictionary_mut(id) {
            update(d);
        }
        return;
    }
    // /D held directly (or /OCProperties itself is indirect).
    let props_ref = pdf
        .catalog()
        .ok()
        .and_then(|c| c.get(b"OCProperties").ok())
        .and_then(|o| o.as_reference().ok());
    let props = match props_ref {
        Some(id) => pdf.get_dictionary_mut(id).ok(),
        None => pdf
            .catalog_mut()
            .ok()
            .and_then(|c| c.get_mut(b"OCProperties").ok())
            .and_then(|o| o.as_dict_mut().ok()),
    };
    if let Some(props) = props {
        match props.get_mut(b"D").ok().and_then(|o| o.as_dict_mut().ok()) {
            Some(d) => update(d),
            None => {
                let mut d = Dictionary::new();
                update(&mut d);
                props.set("D", d);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::dictionary;

    fn sample() -> (Pdf, ObjectId, ObjectId, ObjectId) {
        let mut pdf = Pdf::with_version("1.7");
        let a = pdf
            .add_object(dictionary! { "Type" => "OCG", "Name" => Object::string_literal("Grid") });
        let b = pdf
            .add_object(dictionary! { "Type" => "OCG", "Name" => Object::string_literal("Notes") });
        let c = pdf.add_object(
            dictionary! { "Type" => "OCG", "Name" => Object::string_literal("Orphan") },
        );
        let d = pdf.add_object(dictionary! {
            "OFF" => vec![b.into()],
            "Locked" => vec![a.into()],
            "Order" => vec![
                a.into(),
                vec![Object::string_literal("Review"), b.into()].into(),
            ],
        });
        let catalog = pdf.add_object(dictionary! {
            "Type" => "Catalog",
            "OCProperties" => dictionary! { "OCGs" => vec![a.into(), b.into(), c.into()], "D" => d },
        });
        pdf.trailer.set("Root", catalog);
        (pdf, a, b, c)
    }

    #[test]
    fn lists_in_order_with_headings_and_states() {
        let (mut pdf, a, b, c) = sample();
        let l = layers_in(&pdf);
        // Read from a source, as a file opens: the same rows.
        assert_eq!(layers_in(crate::open::tests::lazily(&mut pdf).pdf()), l);
        let rows: Vec<(Option<ObjectId>, &str, bool, bool, u16)> = l
            .iter()
            .map(|l| (l.id, l.name.as_str(), l.on, l.locked, l.depth))
            .collect();
        assert_eq!(
            rows,
            [
                (Some(a), "Grid", true, true, 0),
                (None, "Review", true, true, 0),
                (Some(b), "Notes", false, false, 1),
                (Some(c), "Orphan", true, false, 0),
            ]
        );
    }

    #[test]
    fn set_hidden_rewrites_the_default_config() {
        let (mut pdf, a, _, c) = sample();
        set_hidden(&mut pdf, &[a, c].into());
        let on: Vec<(String, bool)> = layers_in(&pdf)
            .into_iter()
            .filter(|l| l.id.is_some())
            .map(|l| (l.name, l.on))
            .collect();
        assert_eq!(
            on,
            [
                ("Grid".to_string(), false),
                ("Notes".to_string(), true),
                ("Orphan".to_string(), false)
            ]
        );
    }

    #[test]
    fn files_without_layers_list_none() {
        let pdf = crate::attachments::tests::sample();
        assert!(layers_in(&pdf).is_empty());
    }
}
