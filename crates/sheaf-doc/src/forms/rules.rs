//! What a save takes out of a form because it described the file as it was
//! (M10 proposals 6 and 7): the XFA copy, once the AcroForm changed, and
//! Reader's usage rights, whose signature covers the original bytes.
//!
//! A full rewrite always takes the usage rights out, and `/SigFlags` with
//! them when nothing signed needs it; a certification stays. When the form
//! changed (`Snapshot::form_changed`), the XFA copy goes too, its tooltips
//! copied into `/TU` first: Acrobat would show the XFA's fields and
//! values, not the ones sheaf wrote. An incremental save of a changed form
//! is a full one instead (`incremental`), but for a recovery sidecar, which
//! takes them out in its update.

use lopdf::{Dictionary, Document as Pdf, Object, ObjectId};

use super::field::{FieldKind, Value};
use crate::attachments::resolve;
use crate::source::Limits;

/// What a save took out, for the reader.
pub const XFA_REMOVED: &str =
    "Removed the form's XFA copy, so Acrobat shows the form as it is here";
pub const RIGHTS_REMOVED: &str =
    "Removed Reader's usage rights: their signature covers the file as it was";

/// Whether `pdf`'s form has an XFA copy, and whether it has usage rights.
pub(crate) fn has(pdf: &Pdf) -> (bool, bool) {
    let catalog = pdf.catalog().ok();
    let xfa = catalog
        .and_then(|c| c.get(b"AcroForm").ok())
        .and_then(|f| resolve(pdf, f))
        .and_then(|f| f.as_dict().ok())
        .is_some_and(|f| f.has(b"XFA"));
    let rights = catalog
        .and_then(|c| c.get(b"Perms").ok())
        .and_then(|p| resolve(pdf, p))
        .and_then(|p| p.as_dict().ok())
        .is_some_and(|p| p.has(b"UR3") || p.has(b"UR"));
    (xfa, rights)
}

/// Take the usage rights out of `pdf`, and when `form_changed` its XFA
/// copy. The objects changed (that `pdf` had), and what to tell the
/// reader.
pub(crate) fn take_out(pdf: &mut Pdf, form_changed: bool) -> (Vec<ObjectId>, Vec<String>) {
    let mut touched = Vec::new();
    let mut notes = Vec::new();
    let Some(catalog_id) = pdf
        .trailer
        .get(b"Root")
        .ok()
        .and_then(|r| r.as_reference().ok())
    else {
        return (touched, notes);
    };
    if form_changed && drop_xfa(pdf, catalog_id, &mut touched) {
        notes.push(XFA_REMOVED.to_string());
    }
    if drop_rights(pdf, catalog_id, &mut touched) {
        notes.push(RIGHTS_REMOVED.to_string());
    }
    touched.sort();
    touched.dedup();
    (touched, notes)
}

/// The form dictionary and where it lives: its own object, or the
/// catalog's.
fn form_of(pdf: &Pdf, catalog_id: ObjectId) -> Option<(ObjectId, Dictionary)> {
    match pdf.get_dictionary(catalog_id).ok()?.get(b"AcroForm").ok()? {
        Object::Reference(id) => Some((*id, pdf.get_dictionary(*id).ok()?.clone())),
        Object::Dictionary(d) => Some((catalog_id, d.clone())),
        _ => None,
    }
}

/// Write `form` back where [`form_of`] found it.
fn put_form(pdf: &mut Pdf, catalog_id: ObjectId, at: ObjectId, form: Dictionary) {
    if at == catalog_id {
        if let Ok(c) = pdf.get_dictionary_mut(catalog_id) {
            c.set("AcroForm", form);
        }
    } else {
        pdf.objects.insert(at, Object::Dictionary(form));
    }
}

/// Take `/XFA` out of the form, its tooltips into `/TU` where a field has
/// none. False when there was none.
fn drop_xfa(pdf: &mut Pdf, catalog_id: ObjectId, touched: &mut Vec<ObjectId>) -> bool {
    let Some((at, mut form)) = form_of(pdf, catalog_id) else {
        return false;
    };
    if !form.has(b"XFA") {
        return false;
    }
    let xfa = super::xfa::read(pdf, &form, &Limits::DEFAULT.forms);
    if xfa.iter().any(|f| f.tooltip.is_some()) {
        let xfa = super::xfa::Lookup::new(xfa);
        for f in super::read::fields_of(pdf, &form).unwrap_or_default() {
            if let Some(tip) = xfa.get(&f.name).and_then(|x| x.tooltip.as_deref())
                && let Ok(d) = pdf.get_dictionary_mut(f.object)
                && !d.has(b"TU")
            {
                d.set("TU", crate::save::text_string(tip));
                touched.push(f.object);
            }
        }
    }
    form.remove(b"XFA");
    put_form(pdf, catalog_id, at, form);
    touched.push(at);
    true
}

/// Take `/Perms /UR3` (and the older `/UR`) out, and `/Perms` when that
/// leaves it empty; `/SigFlags` too when no signed field or certification
/// needs it. False when there were none.
fn drop_rights(pdf: &mut Pdf, catalog_id: ObjectId, touched: &mut Vec<ObjectId>) -> bool {
    let Ok(catalog) = pdf.get_dictionary(catalog_id) else {
        return false;
    };
    let (perms_at, mut perms) = match catalog.get(b"Perms") {
        Ok(Object::Reference(id)) => match pdf.get_dictionary(*id) {
            Ok(d) => (Some(*id), d.clone()),
            Err(_) => return false,
        },
        Ok(Object::Dictionary(d)) => (None, d.clone()),
        _ => return false,
    };
    if !perms.has(b"UR3") && !perms.has(b"UR") {
        return false;
    }
    perms.remove(b"UR3");
    perms.remove(b"UR");
    let certified = perms.has(b"DocMDP");
    match (perms_at, perms.is_empty()) {
        (_, true) => {
            if let Ok(c) = pdf.get_dictionary_mut(catalog_id) {
                c.remove(b"Perms");
            }
            touched.push(catalog_id);
        }
        (Some(id), false) => {
            pdf.objects.insert(id, Object::Dictionary(perms));
            touched.push(id);
        }
        (None, false) => {
            if let Ok(c) = pdf.get_dictionary_mut(catalog_id) {
                c.set("Perms", perms);
            }
            touched.push(catalog_id);
        }
    }
    if !certified
        && let Some((at, mut form)) = form_of(pdf, catalog_id)
        && form.has(b"SigFlags")
        // No signed field needs it; a damaged tree may hold one.
        && super::read::fields_of(pdf, &form).is_some_and(|fields| {
            !fields
                .iter()
                .any(|f| f.kind == FieldKind::Signature && f.value == Some(Value::Signed))
        })
    {
        form.remove(b"SigFlags");
        put_form(pdf, catalog_id, at, form);
        touched.push(at);
    }
    true
}
