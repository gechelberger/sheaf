//! XFDF (ISO 19444-1): comments as XML, an element per annotation whose
//! attributes and children are its dictionary's keys under other names.
//! Read into dictionaries and written from them, so what's in a PDF and
//! what's in XFDF go through the same converter.
//!
//! Only `<annots>` (and `<f>`, the file they're on) for now; M10's form
//! data goes beside them as `<fields>`.

use anyhow::{Context, Result, bail};
use lopdf::{Dictionary, Document as Pdf, Object, StringFormat};
use quick_xml::escape::{escape, resolve_predefined_entity};
use quick_xml::events::Event;

use super::Comments;
use crate::file::pdf_text;

const NS: &str = "http://ns.adobe.com/xfdf/";

/// Subtypes and their elements.
const ELEMENTS: [(&str, &str); 17] = [
    ("Text", "text"),
    ("FreeText", "freetext"),
    ("Line", "line"),
    ("Square", "square"),
    ("Circle", "circle"),
    ("Polygon", "polygon"),
    ("PolyLine", "polyline"),
    ("Highlight", "highlight"),
    ("Underline", "underline"),
    ("Squiggly", "squiggly"),
    ("StrikeOut", "strikeout"),
    ("Stamp", "stamp"),
    ("Caret", "caret"),
    ("Ink", "ink"),
    ("FileAttachment", "fileattachment"),
    ("Sound", "sound"),
    ("Redact", "redact"),
];

/// `/F` bits by name.
const FLAGS: [(&str, i64); 9] = [
    ("invisible", 1),
    ("hidden", 2),
    ("print", 4),
    ("nozoom", 8),
    ("norotate", 16),
    ("noview", 32),
    ("readonly", 64),
    ("locked", 128),
    ("togglenoview", 256),
];

/// Border styles (`/BS /S`) by name.
const STYLES: [(&str, &[u8]); 5] = [
    ("solid", b"S"),
    ("dash", b"D"),
    ("bevelled", b"B"),
    ("inset", b"I"),
    ("underline", b"U"),
];

/// `comments` as an XFDF document.
pub fn write(comments: &Comments) -> Result<Vec<u8>> {
    let pdf = &comments.pdf;
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<xfdf xmlns=\"{NS}\" xml:space=\"preserve\">\n<annots>\n"
    );
    for &(page, id) in &comments.annots {
        let Ok(d) = pdf.get_dictionary(id) else {
            continue;
        };
        let sub = crate::import::subtype(d);
        let Some(&(_, element)) = ELEMENTS.iter().find(|(s, _)| *s == sub) else {
            continue;
        };
        let mut attrs: Vec<(&str, String)> = vec![("page", page.to_string())];
        let nums = |key: &[u8]| numbers(pdf, d, key);
        let text = |key: &[u8]| d.get(key).ok().map(pdf_text).filter(|t| !t.is_empty());
        let name = |key: &[u8]| {
            d.get(key)
                .ok()
                .and_then(|o| o.as_name().ok())
                .map(|n| String::from_utf8_lossy(n).into_owned())
        };
        if let Some(r) = nums(b"Rect") {
            attrs.push(("rect", join(&r, ",")));
        }
        for (key, attr) in [
            (b"NM".as_slice(), "name"),
            (b"T", "title"),
            (b"Subj", "subject"),
            (b"M", "date"),
            (b"CreationDate", "creationdate"),
            (b"State", "state"),
            (b"StateModel", "statemodel"),
        ] {
            if let Some(v) = text(key) {
                attrs.push((attr, v));
            }
        }
        for (key, attr) in [(b"C".as_slice(), "color"), (b"IC", "interior-color")] {
            if let Some(c) = nums(key).and_then(|c| hex_color(&c)) {
                attrs.push((attr, c));
            }
        }
        if let Some(ca) = d.get(b"CA").ok().and_then(|o| o.as_float().ok()) {
            attrs.push(("opacity", num(ca as f32)));
        }
        if let Some(f) = d.get(b"F").ok().and_then(|o| o.as_i64().ok()) {
            let names: Vec<&str> = FLAGS
                .iter()
                .filter(|(_, bit)| f & bit != 0)
                .map(|(n, _)| *n)
                .collect();
            if !names.is_empty() {
                attrs.push(("flags", names.join(",")));
            }
        }
        let bs = d
            .get(b"BS")
            .ok()
            .and_then(|o| pdf.dereference(o).ok())
            .and_then(|(_, o)| o.as_dict().ok());
        if let Some(w) = bs
            .and_then(|bs| bs.get(b"W").ok())
            .and_then(|w| w.as_float().ok())
        {
            attrs.push(("width", num(w as f32)));
        }
        let cloudy = d
            .get(b"BE")
            .ok()
            .and_then(|o| pdf.dereference(o).ok())
            .and_then(|(_, o)| o.as_dict().ok())
            .filter(|be| be.get(b"S").ok().and_then(|s| s.as_name().ok()) == Some(b"C"));
        match cloudy {
            Some(be) => {
                attrs.push(("style", "cloudy".into()));
                let i = be
                    .get(b"I")
                    .ok()
                    .and_then(|i| i.as_float().ok())
                    .unwrap_or(1.0);
                attrs.push(("intensity", num(i as f32)));
            }
            None => {
                if let Some(style) = bs
                    .and_then(|bs| bs.get(b"S").ok())
                    .and_then(|s| s.as_name().ok())
                    .and_then(|s| STYLES.iter().find(|(_, k)| *k == s))
                {
                    attrs.push(("style", style.0.into()));
                }
            }
        }
        if let Some(dash) = bs
            .and_then(|bs| bs.get(b"D").ok())
            .and_then(|o| o.as_array().ok())
        {
            attrs.push(("dashes", join(&floats(pdf, dash), ",")));
        }
        if let Some(rd) = nums(b"RD") {
            attrs.push(("fringe", join(&rd, ",")));
        }
        if let Some(parent) = comments.parent_of(d) {
            attrs.push(("inreplyto", parent));
            if name(b"RT").as_deref() == Some("Group") {
                attrs.push(("replyType", "group".into()));
            }
        }
        if let Some(icon) = name(b"Name") {
            attrs.push(("icon", icon));
        }
        if let Some(open) = d.get(b"Open").ok().and_then(|o| o.as_bool().ok()) {
            attrs.push(("open", if open { "yes" } else { "no" }.into()));
        }
        if let Some(q) = nums(b"QuadPoints") {
            attrs.push(("coords", join(&q, ",")));
        }
        if let Some(l) = nums(b"L").filter(|l| l.len() == 4) {
            attrs.push(("start", join(&l[..2], ",")));
            attrs.push(("end", join(&l[2..], ",")));
        }
        match d
            .get(b"LE")
            .ok()
            .and_then(|o| pdf.dereference(o).ok())
            .map(|(_, o)| o)
        {
            Some(Object::Array(ends)) => {
                for (end, attr) in ends.iter().zip(["head", "tail"]) {
                    if let Ok(n) = end.as_name() {
                        attrs.push((attr, String::from_utf8_lossy(n).into_owned()));
                    }
                }
            }
            Some(Object::Name(n)) => attrs.push(("head", String::from_utf8_lossy(n).into_owned())),
            _ => {}
        }
        if let Some(it) = name(b"IT") {
            attrs.push(("intent", it));
        }
        if let Some(cl) = nums(b"CL") {
            attrs.push(("callout", join(&cl, ",")));
        }
        if let Some(sy) = name(b"Sy") {
            attrs.push(("symbol", sy));
        }
        if let Some(r) = d.get(b"Rotate").ok().and_then(|o| o.as_i64().ok()) {
            attrs.push(("rotation", r.to_string()));
        }

        out.push_str(&format!("<{element}"));
        for (k, v) in &attrs {
            out.push_str(&format!(" {k}=\"{}\"", escape(v.as_str())));
        }
        out.push('>');
        if let Some(c) = text(b"Contents") {
            out.push_str(&format!("<contents>{}</contents>", escape(c.as_str())));
        }
        for (key, child) in [
            (b"DA".as_slice(), "defaultappearance"),
            (b"DS", "defaultstyle"),
        ] {
            if let Some(v) = text(key) {
                out.push_str(&format!("<{child}>{}</{child}>", escape(v.as_str())));
            }
        }
        if let Some(v) = nums(b"Vertices") {
            out.push_str(&format!("<vertices>{}</vertices>", pairs(&v)));
        }
        if let Some(Object::Array(strokes)) = d
            .get(b"InkList")
            .ok()
            .and_then(|o| pdf.dereference(o).ok())
            .map(|(_, o)| o)
        {
            out.push_str("<inklist>");
            for s in strokes {
                if let Ok((_, Object::Array(s))) = pdf.dereference(s) {
                    out.push_str(&format!("<gesture>{}</gesture>", pairs(&floats(pdf, s))));
                }
            }
            out.push_str("</inklist>");
        }
        out.push_str(&format!("</{element}>\n"));
    }
    out.push_str("</annots>\n");
    if let Some(f) = &comments.file {
        out.push_str(&format!("<f href=\"{}\"/>\n", escape(f.as_str())));
    }
    out.push_str("</xfdf>\n");
    Ok(out.into_bytes())
}

/// The comments in an XFDF document.
pub fn read(bytes: &[u8]) -> Result<Comments> {
    let text = std::str::from_utf8(bytes).context("XFDF that isn't UTF-8")?;
    let mut reader = quick_xml::Reader::from_str(text);
    let mut out = Comments::new();
    // The annotation being read: its page and keys; the child being read
    // and its text so far; the strokes of an ink.
    let mut current: Option<(u32, Dictionary)> = None;
    let mut child: Option<String> = None;
    let mut buf = String::new();
    let mut strokes: Vec<Object> = Vec::new();
    let mut in_annots = false;
    let mut saw_xfdf = false;
    loop {
        let event = reader.read_event().context("reading the XFDF")?;
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let empty = matches!(event, Event::Empty(_));
                let tag = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                let attrs: Vec<(String, String)> = e
                    .attributes()
                    .flatten()
                    .map(|a| {
                        (
                            String::from_utf8_lossy(a.key.local_name().as_ref()).into_owned(),
                            a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                                .map(|v| v.into_owned())
                                .unwrap_or_default(),
                        )
                    })
                    .collect();
                match (tag.as_str(), &current) {
                    ("xfdf", _) => saw_xfdf = true,
                    ("annots", _) => in_annots = !empty,
                    ("f", None) => {
                        out.file = attrs
                            .iter()
                            .find(|(k, _)| k == "href")
                            .map(|(_, v)| v.clone());
                    }
                    (_, None) if in_annots => {
                        if let Some(&(sub, _)) = ELEMENTS.iter().find(|(_, el)| *el == tag) {
                            let (page, dict) = annotation(sub, &attrs);
                            if empty {
                                let id = out.pdf.add_object(Object::Dictionary(dict));
                                out.annots.push((page, id));
                            } else {
                                current = Some((page, dict));
                            }
                        }
                    }
                    (_, Some(_)) => {
                        buf.clear();
                        child = (!empty).then_some(tag);
                    }
                    _ => {}
                }
            }
            Event::Text(t) => {
                if child.is_some() {
                    buf.push_str(&t.xml10_content().unwrap_or_default());
                }
            }
            Event::CData(t) => {
                if child.is_some() {
                    buf.push_str(&String::from_utf8_lossy(&t));
                }
            }
            Event::GeneralRef(r) => {
                if child.is_some() {
                    if let Ok(Some(c)) = r.resolve_char_ref() {
                        buf.push(c);
                    } else if let Ok(name) = r.decode()
                        && let Some(s) = resolve_predefined_entity(&name)
                    {
                        buf.push_str(s);
                    }
                }
            }
            Event::End(e) => {
                let tag = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                if child.as_deref() == Some(tag.as_str()) {
                    if let Some((_, dict)) = &mut current {
                        let value = std::mem::take(&mut buf);
                        match tag.as_str() {
                            "contents" => dict.set("Contents", crate::save::text_string(&value)),
                            "defaultappearance" => dict.set(
                                "DA",
                                Object::String(value.into_bytes(), StringFormat::Literal),
                            ),
                            "defaultstyle" => dict.set("DS", crate::save::text_string(&value)),
                            "vertices" => dict.set("Vertices", reals(&value)),
                            "gesture" => strokes.push(reals(&value)),
                            _ => {}
                        }
                    }
                    child = None;
                } else if tag == "inklist" {
                    if let Some((_, dict)) = &mut current {
                        dict.set("InkList", Object::Array(std::mem::take(&mut strokes)));
                    }
                } else if tag == "annots" {
                    in_annots = false;
                } else if let Some((page, dict)) = current.take_if(|(_, d)| {
                    ELEMENTS
                        .iter()
                        .any(|(sub, el)| *el == tag && crate::import::subtype(d) == *sub)
                }) {
                    let id = out.pdf.add_object(Object::Dictionary(dict));
                    out.annots.push((page, id));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !saw_xfdf {
        bail!("not XFDF: no <xfdf> element");
    }
    Ok(out)
}

/// An annotation's dictionary, from its element's attributes.
fn annotation(subtype: &str, attrs: &[(String, String)]) -> (u32, Dictionary) {
    let mut d = Dictionary::new();
    d.set("Type", Object::Name(b"Annot".to_vec()));
    d.set("Subtype", Object::Name(subtype.as_bytes().to_vec()));
    let mut page = 0;
    let mut bs = Dictionary::new();
    let mut ends: (Option<String>, Option<String>) = (None, None);
    let (mut start, mut end) = (None, None);
    let text = |v: &str| crate::save::text_string(v);
    let name = |v: &str| Object::Name(v.as_bytes().to_vec());
    for (k, v) in attrs {
        match k.as_str() {
            "page" => page = v.trim().parse().unwrap_or(0),
            "rect" => d.set("Rect", reals(v)),
            "name" => d.set("NM", text(v)),
            "title" => d.set("T", text(v)),
            "subject" => d.set("Subj", text(v)),
            "date" => d.set("M", text(v)),
            "creationdate" => d.set("CreationDate", text(v)),
            "state" => d.set("State", text(v)),
            "statemodel" => d.set("StateModel", text(v)),
            "color" => {
                if let Some(c) = rgb(v) {
                    d.set("C", c);
                }
            }
            "interior-color" => {
                if let Some(c) = rgb(v) {
                    d.set("IC", c);
                }
            }
            "opacity" => {
                if let Ok(o) = v.trim().parse::<f32>() {
                    d.set("CA", Object::from(o));
                }
            }
            "flags" => {
                let f: i64 = v
                    .split([',', ' '])
                    .filter_map(|n| FLAGS.iter().find(|(f, _)| f.eq_ignore_ascii_case(n.trim())))
                    .map(|(_, bit)| bit)
                    .sum();
                d.set("F", f);
            }
            "width" => {
                if let Ok(w) = v.trim().parse::<f32>() {
                    bs.set("W", Object::from(w));
                }
            }
            "style" if v == "cloudy" => {
                let mut be = Dictionary::new();
                be.set("S", name("C"));
                let i = attrs
                    .iter()
                    .find(|(k, _)| k == "intensity")
                    .and_then(|(_, i)| i.trim().parse::<f32>().ok())
                    .unwrap_or(1.0);
                be.set("I", Object::from(i));
                d.set("BE", be);
            }
            "style" => {
                if let Some((_, s)) = STYLES.iter().find(|(n, _)| *n == v) {
                    bs.set("S", Object::Name(s.to_vec()));
                }
            }
            "dashes" => bs.set("D", reals(v)),
            "fringe" => d.set("RD", reals(v)),
            "inreplyto" => d.set("IRT", text(v)),
            "replyType" if v.eq_ignore_ascii_case("group") => d.set("RT", name("Group")),
            "icon" => d.set("Name", name(v)),
            "open" => d.set("Open", v == "yes" || v == "true"),
            "coords" => d.set("QuadPoints", reals(v)),
            "start" => start = Some(v.clone()),
            "end" => end = Some(v.clone()),
            "head" => ends.0 = Some(v.clone()),
            "tail" => ends.1 = Some(v.clone()),
            "intent" => d.set("IT", name(v)),
            "callout" => d.set("CL", reals(v)),
            "symbol" => d.set("Sy", name(v)),
            "rotation" => {
                if let Ok(r) = v.trim().parse::<i64>() {
                    d.set("Rotate", r);
                }
            }
            _ => {}
        }
    }
    if !bs.is_empty() {
        d.set("BS", bs);
    }
    if let (Some(a), Some(b)) = (start, end) {
        d.set("L", reals(&format!("{a},{b}")));
    }
    match (subtype, ends) {
        ("Line" | "PolyLine", (h, t)) if h.is_some() || t.is_some() => {
            let end = |e: Option<String>| name(e.as_deref().unwrap_or("None"));
            d.set("LE", Object::Array(vec![end(h), end(t)]));
        }
        (_, (Some(h), _)) => d.set("LE", name(&h)),
        _ => {}
    }
    (page, d)
}

fn numbers(pdf: &Pdf, d: &Dictionary, key: &[u8]) -> Option<Vec<f32>> {
    let (_, o) = pdf.dereference(d.get(key).ok()?).ok()?;
    Some(floats(pdf, o.as_array().ok()?))
}

fn floats(pdf: &Pdf, a: &[Object]) -> Vec<f32> {
    a.iter()
        .map(|o| {
            pdf.dereference(o)
                .ok()
                .and_then(|(_, o)| o.as_float().ok())
                .unwrap_or(0.0) as f32
        })
        .collect()
}

/// "1,2.5;3,4" or "1 2.5 3 4" as numbers.
fn reals(s: &str) -> Object {
    Object::Array(
        s.split([',', ';', ' ', '\n', '\r', '\t'])
            .filter_map(|v| v.trim().parse::<f32>().ok())
            .map(Object::from)
            .collect(),
    )
}

fn num(v: f32) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" {
        "0".into()
    } else {
        s.to_string()
    }
}

fn join(v: &[f32], sep: &str) -> String {
    v.iter().map(|&x| num(x)).collect::<Vec<_>>().join(sep)
}

/// "x,y;x,y", as vertices and gestures are written.
fn pairs(v: &[f32]) -> String {
    v.chunks(2)
        .map(|c| join(c, ","))
        .collect::<Vec<_>>()
        .join(";")
}

fn hex_color(c: &[f32]) -> Option<String> {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let [r, g, b] = match *c {
        [g] => [byte(g); 3],
        [r, g, b] => [byte(r), byte(g), byte(b)],
        _ => return None,
    };
    Some(format!("#{r:02X}{g:02X}{b:02X}"))
}

fn rgb(hex: &str) -> Option<Object> {
    let h = hex.trim().strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let part = |i: usize| {
        u8::from_str_radix(&h[i..i + 2], 16)
            .ok()
            .map(|v| Object::from(v as f32 / 255.0))
    };
    Some(Object::Array(vec![part(0)?, part(2)?, part(4)?]))
}
