//! PDF Portfolios (the catalog's `/Collection`): a PDF whose real content
//! is its embedded files, with folders, metadata columns and a cover
//! sheet. Read only.

use std::collections::{HashMap, HashSet};

use lopdf::{Dictionary, Object, ObjectId};

use crate::attachments::{Objects, once, resolve};
use crate::file::pdf_text;

/// One metadata column of the portfolio (`/Schema`).
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    /// The key in each file's `/CI`, or for the built-ins `/F` (file name),
    /// `/Desc`, `/Size`, `/ModDate`, `/CreationDate`.
    pub key: String,
    /// Column heading (`/N`).
    pub name: String,
    pub kind: FieldKind,
    /// Column order (`/O`), and whether it's shown (`/V`).
    pub order: i64,
    pub visible: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    Date,
    Number,
    FileName,
    Description,
    Size,
    Modified,
    Created,
    CompressedSize,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Collection {
    /// Visible columns in display order.
    pub fields: Vec<Field>,
    /// The file shown first (`/D`), by its `/EmbeddedFiles` key.
    pub initial: Option<String>,
}

/// The catalog's `/Collection`, if this file is a portfolio.
pub fn collection<P: Objects + ?Sized>(pdf: &P) -> Option<Collection> {
    let c = pdf
        .catalog()
        .ok()?
        .get(b"Collection")
        .ok()
        .and_then(|o| resolve(pdf, o))?
        .as_dict()
        .ok()?;
    let mut fields: Vec<Field> = c
        .get(b"Schema")
        .ok()
        .and_then(|o| resolve(pdf, o))
        .and_then(|o| o.as_dict().ok())
        .map(|schema| {
            schema
                .iter()
                .filter(|(k, _)| k.as_slice() != b"Type")
                .filter_map(|(k, v)| {
                    let d = resolve(pdf, v)?.as_dict().ok()?;
                    Some(field(pdf, String::from_utf8_lossy(k).into_owned(), d))
                })
                .filter(|f| f.visible)
                .collect()
        })
        .unwrap_or_default();
    fields.sort_by_key(|f| f.order);
    let initial = c
        .get(b"D")
        .ok()
        .and_then(|o| resolve(pdf, o))
        .map(pdf_text)
        .filter(|s| !s.is_empty());
    Some(Collection { fields, initial })
}

fn field<P: Objects + ?Sized>(pdf: &P, key: String, d: &Dictionary) -> Field {
    let kind = match d.get(b"Subtype").ok().and_then(|o| o.as_name().ok()) {
        Some(b"D") => FieldKind::Date,
        Some(b"N") => FieldKind::Number,
        Some(b"F") => FieldKind::FileName,
        Some(b"Desc") => FieldKind::Description,
        Some(b"Size") => FieldKind::Size,
        Some(b"ModDate") => FieldKind::Modified,
        Some(b"CreationDate") => FieldKind::Created,
        Some(b"CompressedSize") => FieldKind::CompressedSize,
        _ => FieldKind::Text,
    };
    let name = d
        .get(b"N")
        .ok()
        .and_then(|o| resolve(pdf, o))
        .map(pdf_text)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| key.clone());
    let order = d
        .get(b"O")
        .ok()
        .and_then(|o| resolve(pdf, o))
        .and_then(|o| o.as_i64().ok())
        .unwrap_or(i64::MAX);
    let visible = d
        .get(b"V")
        .ok()
        .and_then(|o| resolve(pdf, o))
        .and_then(|o| o.as_bool().ok())
        .unwrap_or(true);
    Field {
        key,
        name,
        kind,
        order,
        visible,
    }
}

/// Folder paths by folder `/ID`: "Tax/2024" for a folder two levels down;
/// the root folder is "".
pub fn folder_paths<P: Objects + ?Sized>(pdf: &P) -> HashMap<i64, String> {
    let mut out = HashMap::new();
    let mut seen = HashSet::new();
    let root = pdf
        .catalog()
        .ok()
        .and_then(|c| c.get(b"Collection").ok())
        .and_then(|o| resolve(pdf, o))
        .and_then(|o| o.as_dict().ok())
        .and_then(|c| c.get(b"Folders").ok())
        .and_then(|o| once(pdf, &mut seen, o))
        .and_then(|o| o.as_dict().ok());
    if let Some(root) = root {
        if let Some(id) = folder_id(pdf, root) {
            out.insert(id, String::new());
        }
        walk_folders(pdf, root, "", &mut out, 0, &mut seen);
    }
    out
}

/// The folders below `parent`, each reached by reference once: a folder
/// that's its own child or sibling, or another's again, ends the run.
fn walk_folders<P: Objects + ?Sized>(
    pdf: &P,
    parent: &Dictionary,
    path: &str,
    out: &mut HashMap<i64, String>,
    depth: u8,
    seen: &mut HashSet<ObjectId>,
) {
    if depth > 32 {
        return;
    }
    let mut next = parent
        .get(b"Child")
        .ok()
        .and_then(|o| once(pdf, seen, o))
        .and_then(|o| o.as_dict().ok());
    while let Some(f) = next {
        let name = f
            .get(b"Name")
            .ok()
            .and_then(|o| resolve(pdf, o))
            .map(pdf_text)
            .unwrap_or_default();
        let here = if path.is_empty() {
            name
        } else {
            format!("{path}/{name}")
        };
        if let Some(id) = folder_id(pdf, f) {
            out.insert(id, here.clone());
        }
        walk_folders(pdf, f, &here, out, depth + 1, seen);
        next = f
            .get(b"Next")
            .ok()
            .and_then(|o| once(pdf, seen, o))
            .and_then(|o| o.as_dict().ok());
    }
}

fn folder_id<P: Objects + ?Sized>(pdf: &P, f: &Dictionary) -> Option<i64> {
    f.get(b"ID")
        .ok()
        .and_then(|o| resolve(pdf, o))
        .and_then(|o| o.as_i64().ok())
}

/// A `/CI` value as text: strings, numbers, dates, or a collection
/// sub-item (`/D` data with an optional `/P` prefix).
pub fn field_text<P: Objects + ?Sized>(pdf: &P, v: &Object) -> String {
    let Some(v) = resolve(pdf, v) else {
        return String::new();
    };
    match v {
        Object::Integer(n) => n.to_string(),
        Object::Real(r) => r.to_string(),
        Object::String(..) => pdf_text(v),
        Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
        Object::Dictionary(d) => {
            let prefix = d.get(b"P").map(|o| field_text(pdf, o)).unwrap_or_default();
            let data = d.get(b"D").map(|o| field_text(pdf, o)).unwrap_or_default();
            format!("{prefix}{data}")
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{Document as Pdf, dictionary};

    #[test]
    fn reads_schema_folders_and_initial_file() {
        let mut pdf = crate::attachments::tests::sample();
        let sub = pdf.add_object(
            dictionary! { "Type" => "Folder", "ID" => 2, "Name" => Object::string_literal("2024") },
        );
        let tax = pdf.add_object(dictionary! { "Type" => "Folder", "ID" => 1, "Name" => Object::string_literal("Tax"), "Child" => sub });
        let root = pdf.add_object(dictionary! { "Type" => "Folder", "ID" => 0, "Name" => Object::string_literal("root"), "Child" => tax });
        let coll = dictionary! {
            "Type" => "Collection",
            "D" => Object::string_literal("w2"),
            "Folders" => root,
            "Schema" => dictionary! {
                "client" => dictionary! { "Subtype" => "S", "N" => Object::string_literal("Client"), "O" => 2 },
                "name" => dictionary! { "Subtype" => "F", "N" => Object::string_literal("Name"), "O" => 1 },
                "hidden" => dictionary! { "Subtype" => "S", "V" => false },
            },
        };
        let root_id = pdf.trailer.get(b"Root").unwrap().as_reference().unwrap();
        pdf.get_object_mut(root_id)
            .unwrap()
            .as_dict_mut()
            .unwrap()
            .set("Collection", coll);
        let c = collection(&pdf).unwrap();
        let names: Vec<&str> = c.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["Name", "Client"]);
        assert_eq!(c.fields[0].kind, FieldKind::FileName);
        assert_eq!(c.initial.as_deref(), Some("w2"));
        let paths = folder_paths(&pdf);
        assert_eq!(paths[&0], "");
        assert_eq!(paths[&1], "Tax");
        assert_eq!(paths[&2], "Tax/2024");
        // Read from a source, as a file opens: the same.
        let src = crate::open::tests::lazily(&mut pdf);
        assert_eq!(collection(src.pdf()), Some(c));
        assert_eq!(folder_paths(src.pdf()), paths);
    }

    #[test]
    fn plain_files_are_not_portfolios() {
        assert!(collection(&crate::attachments::tests::sample()).is_none());
    }

    #[test]
    fn sub_items_join_prefix_and_data() {
        let pdf = Pdf::with_version("1.7");
        let v: Object = dictionary! { "D" => 42, "P" => Object::string_literal("#") }.into();
        assert_eq!(field_text(&pdf, &v), "#42");
    }
}
