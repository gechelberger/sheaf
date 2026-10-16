//! A hybrid form's XFA copy (M10 proposal 6): the fields its template
//! draws, by the AcroForm full name each has, with the value its datasets
//! hold for it and its tooltip.
//!
//! sheaf fills the AcroForm and never the XFA. It reads the XFA to see
//! whether its datasets hold values the AcroForm lacks (a static form
//! filled by a tool that only writes XFA), and to keep its tooltips in
//! `/TU` when a save removes it: the IRS forms' tooltips are only there.
//!
//! The binding is XFA's normal one, by name and order: each named subform
//! is a data group unless it binds `none`; a field is a data value in the
//! group it's in (`topmostSubform[0].Page1[0].f1_01[0]` binds to
//! `topmostSubform/f1_01`, since `Page1` binds `none`). `global` binds by
//! name anywhere, and a `dataRef` of `$.…` or `$record.…` by its path.
//! Anything else isn't followed.

use std::collections::HashMap;

use lopdf::{Dictionary, Object};
use quick_xml::events::{BytesStart, Event};

use crate::attachments::{Objects, resolve};
use crate::source::FormLimits;

/// A field (or exclusion group) of the template.
#[derive(Debug, Clone, PartialEq)]
pub struct XfaField {
    /// Its AcroForm full name, as Designer writes it:
    /// `topmostSubform[0].Page1[0].f1_01[0]`.
    pub name: String,
    /// What the datasets hold for it, when they hold anything.
    pub value: Option<String>,
    /// Its `<toolTip>`, or else its `<speak>` text.
    pub tooltip: Option<String>,
}

/// An XFA copy's fields, to look up by AcroForm full name: as Designer
/// writes it (`form1[0].x[0]`), or else by its path without indexes, when
/// only one field has that path (a form whose AcroForm names have none).
pub struct Lookup {
    exact: HashMap<String, XfaField>,
    bare: HashMap<String, Option<String>>,
}

impl Lookup {
    pub fn new(fields: Vec<XfaField>) -> Lookup {
        let mut bare: HashMap<String, Option<String>> = HashMap::new();
        for f in &fields {
            bare.entry(without_indexes(&f.name))
                .and_modify(|v| *v = None)
                .or_insert_with(|| Some(f.name.clone()));
        }
        let exact = fields.into_iter().map(|f| (f.name.clone(), f)).collect();
        Lookup { exact, bare }
    }

    pub fn get(&self, name: &str) -> Option<&XfaField> {
        self.exact.get(name).or_else(|| {
            let only = self.bare.get(&without_indexes(name))?.as_ref()?;
            self.exact.get(only)
        })
    }
}

/// A full name without its `[n]` indexes.
fn without_indexes(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut depth = 0usize;
    for c in name.chars() {
        match c {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            c if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// The fields of `form`'s XFA copy (`/AcroForm /XFA`), read within
/// `limits`: a copy decompressed to at most [`FormLimits::xfa`], with as
/// many fields and data values as a field tree's nodes, and names as long
/// as its full names. Empty when it has none, or it isn't XML.
pub(crate) fn read<P: Objects + ?Sized>(
    pdf: &P,
    form: &Dictionary,
    limits: &FormLimits,
) -> Vec<XfaField> {
    let Some((template, datasets)) = packets(pdf, form, limits) else {
        return Vec::new();
    };
    let data = datasets
        .as_deref()
        .map(|xml| data_values(xml, limits))
        .unwrap_or_default();
    fields(&template, &data, limits).unwrap_or_default()
}

/// The template packet, and the datasets packet when there is one. A copy
/// in one stream is both.
fn packets<P: Objects + ?Sized>(
    pdf: &P,
    form: &Dictionary,
    limits: &FormLimits,
) -> Option<(Vec<u8>, Option<Vec<u8>>)> {
    let content = |o: &Object| {
        let s = resolve(pdf, o)?.as_stream().ok()?;
        s.decompressed_content_with_limit(limits.xfa).ok()
    };
    match resolve(pdf, form.get(b"XFA").ok()?)? {
        Object::Array(items) => {
            let (mut template, mut datasets) = (None, None);
            for pair in items.chunks(2) {
                match (pair[0].as_str().ok(), pair.get(1)) {
                    (Some(b"template"), Some(o)) => template = content(o),
                    (Some(b"datasets"), Some(o)) => datasets = content(o),
                    _ => {}
                }
            }
            Some((template?, datasets))
        }
        o @ Object::Stream(_) => {
            let all = content(o)?;
            Some((all.clone(), Some(all)))
        }
        _ => None,
    }
}

fn attr(e: &BytesStart, key: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == key)
        .map(|a| {
            a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .map(|v| v.into_owned())
                .unwrap_or_else(|_| String::from_utf8_lossy(&a.value).into_owned())
        })
}

/// The data values under `<xfa:data>`, by path (`topmostSubform/f1_01`),
/// each path's in document order.
fn data_values(xml: &[u8], limits: &FormLimits) -> HashMap<String, Vec<String>> {
    let mut reader = quick_xml::Reader::from_reader(xml);
    let mut buf = Vec::new();
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    // Open elements under <data>: their names, whether they're groups (a
    // child element, or marked so), and their text.
    let mut open: Vec<(String, bool, String)> = Vec::new();
    let mut in_data = false;
    let mut count = 0;
    while let Ok(event) = reader.read_event_into(&mut buf) {
        match &event {
            Event::Start(e) | Event::Empty(e) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                if !in_data {
                    in_data = name == "data" && matches!(event, Event::Start(_));
                    buf.clear();
                    continue;
                }
                if let Some(parent) = open.last_mut() {
                    parent.1 = true;
                }
                count += 1;
                if count > limits.nodes {
                    break;
                }
                let group = attr(e, b"dataNode").as_deref() == Some("dataGroup");
                if matches!(event, Event::Empty(_)) {
                    if !group {
                        let path = data_path(&open, &name);
                        out.entry(path).or_default().push(String::new());
                    }
                } else {
                    open.push((name, group, String::new()));
                }
            }
            Event::Text(t) => {
                if let Some(top) = open.last_mut()
                    && let Ok(text) = t.decode()
                    && top.2.len() < limits.name
                {
                    top.2.push_str(&unescape(&text));
                }
            }
            Event::CData(t) => {
                if let Some(top) = open.last_mut() {
                    top.2.push_str(&String::from_utf8_lossy(t));
                }
            }
            Event::GeneralRef(r) => {
                if let Some(top) = open.last_mut() {
                    top.2.extend(referenced(r));
                }
            }
            Event::End(_) => {
                if !in_data {
                    buf.clear();
                    continue;
                }
                match open.pop() {
                    Some((name, group, text)) => {
                        if !group {
                            let path = data_path(&open, &name);
                            out.entry(path).or_default().push(text);
                        }
                    }
                    // </data>
                    None => break,
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

/// The character a reference (`&amp;`, `&#38;`) stands for: one of XML's
/// own entities or a character reference.
fn referenced(r: &quick_xml::events::BytesRef) -> Option<char> {
    if let Ok(Some(c)) = r.resolve_char_ref() {
        return Some(c);
    }
    match r.decode().ok()?.as_ref() {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => None,
    }
}

/// Text with XML's entities and character references replaced.
fn unescape(text: &str) -> String {
    quick_xml::escape::unescape(text)
        .map(|t| t.into_owned())
        .unwrap_or_else(|_| text.to_string())
}

fn data_path(open: &[(String, bool, String)], name: &str) -> String {
    let mut path: Vec<&str> = open.iter().map(|(n, ..)| n.as_str()).collect();
    path.push(name);
    path.join("/")
}

/// How a container or field binds to data.
#[derive(Debug, Clone, PartialEq)]
enum Bind {
    /// By name: a container is a data group, a field a value in it.
    Normal,
    /// Not at all: a container's fields bind in its parent's group.
    None,
    /// By name, anywhere in the data.
    Global,
    /// By a path from the root.
    Ref(Vec<String>),
}

/// An open element of the template that has a place in it.
struct Frame {
    /// A container, a field or an exclusion group (or something else, with
    /// no name in the field tree).
    kind: Kind,
    name: String,
    /// Its full name: its parents', then its own with its index.
    path: String,
    bind: Bind,
    /// How many of each name it holds so far, and of each kind of
    /// element, for their indexes: a named object's counts its name, an
    /// unnamed one's its kind (`#subform[9]` is the tenth subform).
    names: HashMap<String, usize>,
    classes: HashMap<String, usize>,
    tooltip: Option<String>,
    speak: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Container,
    Field,
    Group,
    Other,
}

/// The template's fields, bound to `data`. None when it isn't XML or is
/// past the field tree's limits.
fn fields(
    xml: &[u8],
    data: &HashMap<String, Vec<String>>,
    limits: &FormLimits,
) -> Option<Vec<XfaField>> {
    const CONTAINERS: [&[u8]; 5] = [b"subform", b"area", b"subformSet", b"pageSet", b"pageArea"];
    let mut reader = quick_xml::Reader::from_reader(xml);
    let mut buf = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();
    let mut in_template = false;
    // How deep inside something left out (a prototype) it is.
    let mut skip = 0usize;
    // The text element being read for a field's tooltip: which, and how
    // deep the stack was when it opened.
    let mut reading: Option<(&'static str, usize, String)> = None;
    // Values handed out, by data path, so the next field of the same path
    // gets the next one.
    let mut taken: HashMap<String, usize> = HashMap::new();
    let mut out = Vec::new();
    loop {
        let event = reader.read_event_into(&mut buf).ok()?;
        match &event {
            Event::Start(e) | Event::Empty(e) => {
                let empty = matches!(event, Event::Empty(_));
                let tag = e.local_name().as_ref().to_vec();
                if !in_template {
                    in_template = tag == b"template" && !empty;
                    buf.clear();
                    continue;
                }
                if skip > 0 {
                    skip += usize::from(!empty);
                    buf.clear();
                    continue;
                }
                if tag == b"proto" {
                    skip = usize::from(!empty);
                    buf.clear();
                    continue;
                }
                // A <bind> is its parent's.
                if tag == b"bind"
                    && let Some(top) = stack.last_mut()
                {
                    top.bind = match (attr(e, b"match").as_deref(), attr(e, b"ref")) {
                        (Some("none"), _) => Bind::None,
                        (Some("global"), _) => Bind::Global,
                        (Some("dataRef"), Some(r)) => match reference(&r) {
                            Some(path) => Bind::Ref(path),
                            None => Bind::None,
                        },
                        _ => Bind::Normal,
                    };
                }
                // A tooltip, in the field's own <assist>.
                if (tag == b"toolTip" || tag == b"speak")
                    && !empty
                    && stack.len() >= 2
                    && stack[stack.len() - 1].kind == Kind::Other
                    && stack[stack.len() - 1].name == "assist"
                    && matches!(stack[stack.len() - 2].kind, Kind::Field | Kind::Group)
                {
                    let which = if tag == b"toolTip" {
                        "toolTip"
                    } else {
                        "speak"
                    };
                    reading = Some((which, stack.len(), String::new()));
                }
                let kind = match tag.as_slice() {
                    b"field" => Kind::Field,
                    b"exclGroup" => Kind::Group,
                    t if CONTAINERS.contains(&t) => Kind::Container,
                    _ => Kind::Other,
                };
                // Fields in an exclusion group are its; nothing in a field
                // is a field.
                let inside = stack
                    .iter()
                    .any(|f| matches!(f.kind, Kind::Field | Kind::Group));
                let kind = if inside && kind != Kind::Other {
                    Kind::Other
                } else {
                    kind
                };
                let tag_name = String::from_utf8_lossy(&tag).into_owned();
                let named = attr(e, b"name").filter(|n| !n.is_empty());
                // Its index among its siblings: by name, or by kind when it
                // has none. A draw or content area isn't a field, but takes
                // an index too (the 2848's `RetentionRevocation[1]` is a
                // field after a draw).
                let counted = kind != Kind::Other || tag == b"draw" || tag == b"contentArea";
                // An unnamed subform is transparent to names: what's in it
                // counts its name in the nearest named container (the
                // 990-EZ's `#subform[2].help[2]` is Page1's third `help`).
                let mut place = None;
                if counted && !inside {
                    let containers: Vec<usize> = (0..stack.len())
                        .filter(|&i| stack[i].kind == Kind::Container)
                        .collect();
                    if let Some(&parent) = containers.last() {
                        let class = stack[parent].classes.entry(tag_name.clone()).or_default();
                        *class += 1;
                        let by_class = *class - 1;
                        let index = match &named {
                            Some(name) => {
                                // Only an unnamed subform: a page set has
                                // names of its own (the 4868's master page
                                // `Page1` isn't its body's).
                                let scope = containers
                                    .iter()
                                    .rev()
                                    .copied()
                                    .find(|&i| stack[i].name != "#subform")
                                    .unwrap_or(parent);
                                let n = stack[scope].names.entry(name.clone()).or_default();
                                *n += 1;
                                *n - 1
                            }
                            None => by_class,
                        };
                        place = Some((index, stack[parent].path.clone()));
                    }
                }
                let (name, path) = if kind == Kind::Other {
                    (tag_name, String::new())
                } else {
                    let name = named.unwrap_or_else(|| format!("#{tag_name}"));
                    let (index, path) = place.unwrap_or((0, String::new()));
                    // A dot in a name is `\.` in an AcroForm's.
                    let segment = name.replace('.', "\\.");
                    let path = if path.is_empty() {
                        format!("{segment}[{index}]")
                    } else {
                        format!("{path}.{segment}[{index}]")
                    };
                    if path.len() > limits.name || out.len() >= limits.nodes {
                        return None;
                    }
                    (name, path)
                };
                let frame = Frame {
                    kind,
                    name,
                    path,
                    bind: Bind::Normal,
                    names: HashMap::new(),
                    classes: HashMap::new(),
                    tooltip: None,
                    speak: None,
                };
                if !empty {
                    stack.push(frame);
                } else if matches!(kind, Kind::Field | Kind::Group) {
                    out.push(bound(&stack, &frame, data, &mut taken));
                }
            }
            Event::Text(t) => {
                if let Some((_, _, text)) = reading.as_mut()
                    && let Ok(t) = t.decode()
                    && text.len() < limits.name
                {
                    text.push_str(&unescape(&t));
                }
            }
            Event::GeneralRef(r) => {
                if let Some((_, _, text)) = reading.as_mut() {
                    text.extend(referenced(r));
                }
            }
            Event::End(e) => {
                if !in_template {
                    buf.clear();
                    continue;
                }
                if skip > 0 {
                    skip -= 1;
                    buf.clear();
                    continue;
                }
                if e.local_name().as_ref() == b"template" && stack.is_empty() {
                    break;
                }
                let Some(frame) = stack.pop() else {
                    break;
                };
                if let Some((which, depth, text)) = reading.take() {
                    if depth == stack.len() {
                        // Its field is two up, past <assist>.
                        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
                        if let Some(field) = stack.len().checked_sub(2).map(|i| &mut stack[i])
                            && !text.is_empty()
                        {
                            match which {
                                "toolTip" => field.tooltip = Some(text),
                                _ => field.speak = Some(text),
                            }
                        }
                    } else {
                        reading = Some((which, depth, text));
                    }
                }
                if matches!(frame.kind, Kind::Field | Kind::Group) {
                    out.push(bound(&stack, &frame, data, &mut taken));
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Some(out)
}

/// `$.a.b` or `$record.a.b` as a path from the data's root (`$.` is taken
/// from the root too: the IRS forms bind only by name).
fn reference(r: &str) -> Option<Vec<String>> {
    let rest = r
        .strip_prefix("$record.")
        .or_else(|| r.strip_prefix("$data."))
        .or_else(|| r.strip_prefix("$."))?;
    let parts: Vec<String> = rest
        .split('.')
        .map(|p| p.split('[').next().unwrap_or(p).to_string())
        .filter(|p| !p.is_empty())
        .collect();
    (!parts.is_empty()).then_some(parts)
}

/// `field` (closed, with `stack` its parents) as found in `data`.
fn bound(
    stack: &[Frame],
    field: &Frame,
    data: &HashMap<String, Vec<String>>,
    taken: &mut HashMap<String, usize>,
) -> XfaField {
    let path = match &field.bind {
        Bind::None => None,
        Bind::Ref(parts) => Some(parts.join("/")),
        Bind::Global => data
            .keys()
            .filter(|k| k.rsplit('/').next() == Some(field.name.as_str()))
            .min()
            .cloned(),
        Bind::Normal => {
            let mut parts: Vec<&str> = stack
                .iter()
                .filter(|f| f.kind == Kind::Container && f.bind == Bind::Normal)
                .filter(|f| !f.name.starts_with('#'))
                .map(|f| f.name.as_str())
                .collect();
            (!field.name.starts_with('#')).then(|| {
                parts.push(&field.name);
                parts.join("/")
            })
        }
    };
    let value = path.and_then(|p| {
        let n = taken.entry(p.clone()).or_default();
        let v = data.get(&p)?.get(*n).cloned();
        *n += 1;
        v.filter(|v| !v.trim().is_empty())
    });
    XfaField {
        name: field.path.clone(),
        value,
        tooltip: field.tooltip.clone().or_else(|| field.speak.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::Limits;

    const LIMITS: &FormLimits = &Limits::DEFAULT.forms;

    const TEMPLATE: &str = r#"<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/"><template xmlns="http://www.xfa.org/schema/xfa-template/3.0/">
<subform name="topmostSubform"><subform name="Page1"><bind match="none"/>
  <draw name="c1_1"/>
  <field name="f1_01"><assist><speak disable="1">Your first name</speak></assist></field>
  <field name="f1_01"><assist><toolTip>Second &amp; last</toolTip><speak>not this</speak></assist></field>
  <field name="c1_1"><items><integer>1</integer><integer>0</integer></items></field>
  <exclGroup name="status"><field name="single"/><field name="married"/></exclGroup>
  <subform name="Row"><field name="cell"/></subform>
  <subform><field name="inner"/><field name="f1_01"/></subform>
  <field name="nowhere"><bind match="none"/></field>
  <field name="byref"><bind match="dataRef" ref="$record.other.thing"/></field>
  <field name="e.g."/>
  <proto><field name="ghost"/></proto>
</subform></subform></template></xdp:xdp>"#;

    const DATA: &str = r#"<xfa:datasets xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/"><xfa:data>
<topmostSubform><f1_01>Ann</f1_01><f1_01/><c1_1>0</c1_1><status>2</status>
<inner>in</inner><Row><cell>c</cell></Row><nowhere>x</nowhere><Empty xfa:dataNode="dataGroup"/></topmostSubform>
<other><thing>ref'd</thing></other></xfa:data></xfa:datasets>"#;

    #[test]
    fn fields_bind_by_name_and_order() {
        let data = data_values(DATA.as_bytes(), LIMITS);
        assert_eq!(data["topmostSubform/f1_01"], ["Ann", ""]);
        assert!(!data.contains_key("topmostSubform/Empty"));
        let fields = fields(TEMPLATE.as_bytes(), &data, LIMITS).unwrap();
        let got: Vec<(&str, Option<&str>, Option<&str>)> = fields
            .iter()
            .map(|f| (f.name.as_str(), f.value.as_deref(), f.tooltip.as_deref()))
            .collect();
        assert_eq!(
            got,
            [
                (
                    "topmostSubform[0].Page1[0].f1_01[0]",
                    Some("Ann"),
                    Some("Your first name")
                ),
                (
                    "topmostSubform[0].Page1[0].f1_01[1]",
                    None,
                    Some("Second & last")
                ),
                // A draw of the same name came first.
                ("topmostSubform[0].Page1[0].c1_1[1]", Some("0"), None),
                ("topmostSubform[0].Page1[0].status[0]", Some("2"), None),
                ("topmostSubform[0].Page1[0].Row[0].cell[0]", Some("c"), None),
                // An unnamed subform binds nothing of its own, and is the
                // second subform.
                (
                    "topmostSubform[0].Page1[0].#subform[1].inner[0]",
                    Some("in"),
                    None
                ),
                // It's transparent to names: this is Page1's third f1_01.
                (
                    "topmostSubform[0].Page1[0].#subform[1].f1_01[2]",
                    None,
                    None
                ),
                ("topmostSubform[0].Page1[0].nowhere[0]", None, None),
                ("topmostSubform[0].Page1[0].byref[0]", Some("ref'd"), None),
                ("topmostSubform[0].Page1[0].e\\.g\\.[0]", None, None),
            ]
        );
    }

    #[test]
    fn what_isnt_xml_reads_as_nothing() {
        let empty = HashMap::new();
        assert_eq!(fields(b"<template><subform", &empty, LIMITS), None);
        assert!(data_values(b"not xml at all", LIMITS).is_empty());
    }
}
