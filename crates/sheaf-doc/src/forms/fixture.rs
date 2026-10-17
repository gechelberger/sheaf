//! Forms built in code, for tests (M10 proposal 21: no PDF is committed
//! to sheaf): fields of each type with their widgets, `/DR`, an `/XFA`
//! copy, usage rights and signatures, a structure tree, and revisions.
//!
//! A [`Form`] is a plain lopdf document plus the ids a test needs to
//! break it on purpose. [`Form::bytes`] writes it, filling in each
//! signature's `/ByteRange` around its `/Contents` as signing would (the
//! contents are zeros: nothing is really signed), and [`Form::source`]
//! reads what it writes as `forms::read` reads a file. lopdf can't write a
//! hybrid-reference file, so [`hybrid_reference`] writes one by hand.
//! [`every_fact`] is a file with one of each fact a file opens with, and
//! [`outline_tree`] gives a form bookmarks.

use std::collections::BTreeMap;
use std::path::Path;

use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document as Pdf, Object, ObjectId, Stream, StringFormat, dictionary};
use sheaf_core::Rotation;

use crate::annot::{Annot, AnnotId, AnnotKind, Pt};
use crate::model::PageId;
use crate::stamps::HeaderFooter;
use crate::store::{BlobKind, Src, Store};

/// A rectangle in page space: left, bottom, right, top.
pub type Rect = [f32; 4];

/// `/Ff` bits.
pub const RADIO: i64 = 1 << 15;
pub const NO_TOGGLE_TO_OFF: i64 = 1 << 14;
pub const PUSH_BUTTON: i64 = 1 << 16;
pub const COMBO: i64 = 1 << 17;

/// What a signature's `/ByteRange` says until [`Form::bytes`] fills it in.
const RANGE_PLACEHOLDER: &[u8] = b"[0 9999999999 9999999999 9999999999]";
/// Bytes of zeros in a signature's `/Contents`.
const CONTENTS_LEN: usize = 512;

pub struct Form {
    pub pdf: Pdf,
    pub catalog: ObjectId,
    pub pages: Vec<ObjectId>,
    /// The `/AcroForm` dictionary, an object of its own.
    pub acroform: ObjectId,
    /// `/DR`'s Helvetica, which appearances use as `/Helv`.
    pub helv: ObjectId,
    /// Signature dictionaries, filled in by `bytes`.
    signatures: Vec<ObjectId>,
}

impl Form {
    /// `pages` Letter pages, each saying "Page n", and an empty form whose
    /// `/DR` has Helvetica (`/Helv`) and ZapfDingbats (`/ZaDb`).
    pub fn new(pages: usize) -> Form {
        let mut pdf = Pdf::with_version("1.7");
        let pages_id = pdf.new_object_id();
        let helv = pdf.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
            "Encoding" => "WinAnsiEncoding",
        });
        let zadb = pdf.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "ZapfDingbats",
        });
        let mut ids = Vec::new();
        for i in 0..pages {
            let content = Content {
                operations: vec![
                    Operation::new("BT", vec![]),
                    Operation::new("Tf", vec!["F1".into(), 18.into()]),
                    Operation::new("Td", vec![72.into(), 740.into()]),
                    Operation::new(
                        "Tj",
                        vec![Object::string_literal(format!("Page {}", i + 1))],
                    ),
                    Operation::new("ET", vec![]),
                ],
            };
            let c = pdf.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
            ids.push(pdf.add_object(dictionary! {
                "Type" => "Page", "Parent" => pages_id, "Contents" => c,
                "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
                "Resources" => dictionary! { "Font" => dictionary! { "F1" => helv } },
            }));
        }
        pdf.objects.insert(
            pages_id,
            dictionary! {
                "Type" => "Pages", "Count" => pages as i64,
                "Kids" => ids.iter().map(|&i| Object::Reference(i)).collect::<Vec<_>>(),
            }
            .into(),
        );
        let acroform = pdf.add_object(dictionary! {
            "Fields" => Vec::<Object>::new(),
            "DR" => dictionary! { "Font" => dictionary! { "Helv" => helv, "ZaDb" => zadb } },
            "DA" => Object::string_literal("/Helv 0 Tf 0 g"),
        });
        let catalog = pdf.add_object(dictionary! {
            "Type" => "Catalog", "Pages" => pages_id, "AcroForm" => acroform,
        });
        pdf.trailer.set("Root", catalog);
        Form {
            pdf,
            catalog,
            pages: ids,
            acroform,
            helv,
            signatures: Vec::new(),
        }
    }

    /// An object's dictionary, to change.
    pub fn dict(&mut self, id: ObjectId) -> &mut Dictionary {
        self.pdf.get_dictionary_mut(id).expect("a dictionary")
    }

    /// A text field, its own widget, at `rect` on page `page` (from 0).
    pub fn text(&mut self, name: &str, page: usize, rect: Rect) -> ObjectId {
        let id = self.widget(page, rect);
        let d = self.dict(id);
        d.set("FT", "Tx");
        d.set("T", Object::string_literal(name));
        d.set("DA", Object::string_literal("/Helv 10 Tf 0 g"));
        self.add_field(id);
        id
    }

    /// A check box, on (state `/Yes`) or off, with both appearances.
    pub fn check_box(&mut self, name: &str, page: usize, rect: Rect, on: bool) -> ObjectId {
        let id = self.widget(page, rect);
        let ap = self.states(rect, "Yes");
        let state = if on { "Yes" } else { "Off" };
        let d = self.dict(id);
        d.set("FT", "Btn");
        d.set("T", Object::string_literal(name));
        d.set("DA", Object::string_literal("/ZaDb 0 Tf 0 g"));
        d.set("AP", ap);
        d.set("V", Object::Name(state.into()));
        d.set("AS", Object::Name(state.into()));
        self.add_field(id);
        id
    }

    /// A radio group: a button per rect on page `page`, in states "0",
    /// "1"…, with `chosen` on. Returns the field; its widgets are its
    /// `/Kids`.
    pub fn radio(
        &mut self,
        name: &str,
        page: usize,
        rects: &[Rect],
        chosen: Option<usize>,
    ) -> ObjectId {
        let value = chosen.map_or("Off".to_string(), |i| i.to_string());
        let field = self.pdf.add_object(dictionary! {
            "FT" => "Btn", "T" => Object::string_literal(name),
            "Ff" => RADIO | NO_TOGGLE_TO_OFF,
            "DA" => Object::string_literal("/ZaDb 0 Tf 0 g"),
            "V" => Object::Name(value.into_bytes()),
        });
        let mut kids = Vec::new();
        for (i, &rect) in rects.iter().enumerate() {
            let w = self.widget(page, rect);
            let ap = self.states(rect, &i.to_string());
            let state = if chosen == Some(i) {
                i.to_string()
            } else {
                "Off".into()
            };
            let d = self.dict(w);
            d.set("Parent", field);
            d.set("AP", ap);
            d.set("AS", Object::Name(state.into_bytes()));
            kids.push(Object::Reference(w));
        }
        self.dict(field).set("Kids", kids);
        self.add_field(field);
        field
    }

    /// A combo box (or, with `combo` false, a list box) of `options`.
    pub fn choice(
        &mut self,
        name: &str,
        page: usize,
        rect: Rect,
        options: &[&str],
        combo: bool,
    ) -> ObjectId {
        let id = self.widget(page, rect);
        let d = self.dict(id);
        d.set("FT", "Ch");
        d.set("T", Object::string_literal(name));
        d.set("DA", Object::string_literal("/Helv 10 Tf 0 g"));
        let options: Vec<Object> = options.iter().map(|o| Object::string_literal(*o)).collect();
        d.set("Opt", options);
        if combo {
            d.set("Ff", COMBO);
        }
        self.add_field(id);
        id
    }

    /// A push button that says `caption`.
    pub fn push_button(&mut self, name: &str, page: usize, rect: Rect, caption: &str) -> ObjectId {
        let id = self.widget(page, rect);
        let ap = self.text_appearance(rect, caption);
        let d = self.dict(id);
        d.set("FT", "Btn");
        d.set("Ff", PUSH_BUTTON);
        d.set("T", Object::string_literal(name));
        d.set("DA", Object::string_literal("/Helv 10 Tf 0 g"));
        d.set(
            "MK",
            dictionary! { "CA" => Object::string_literal(caption) },
        );
        d.set("AP", dictionary! { "N" => ap });
        self.add_field(id);
        id
    }

    /// An unsigned signature field (an empty appearance).
    pub fn signature(&mut self, name: &str, page: usize, rect: Rect) -> ObjectId {
        let id = self.widget(page, rect);
        let ap = self.appearance(rect, Vec::new());
        let d = self.dict(id);
        d.set("FT", "Sig");
        d.set("T", Object::string_literal(name));
        d.set("AP", dictionary! { "N" => ap });
        self.add_field(id);
        id
    }

    /// Fill a text or choice field: `/V`, and on each widget an appearance
    /// that draws it.
    pub fn fill(&mut self, field: ObjectId, value: &str) {
        self.dict(field).set("V", Object::string_literal(value));
        for w in self.widgets_of(field) {
            let rect = self.rect_of(w);
            let ap = self.text_appearance(rect, value);
            self.dict(w).set("AP", dictionary! { "N" => ap });
        }
    }

    /// `/Fields`.
    pub fn fields(&self) -> Vec<ObjectId> {
        refs(self.pdf.get_dictionary(self.acroform).ok(), b"Fields")
    }

    pub fn set_fields(&mut self, fields: &[ObjectId]) {
        let fields: Vec<Object> = fields.iter().map(|&f| Object::Reference(f)).collect();
        self.dict(self.acroform).set("Fields", fields);
    }

    /// Page `page`'s `/Annots`.
    pub fn annots(&self, page: usize) -> Vec<ObjectId> {
        refs(self.pdf.get_dictionary(self.pages[page]).ok(), b"Annots")
    }

    pub fn set_annots(&mut self, page: usize, annots: &[ObjectId]) {
        let annots: Vec<Object> = annots.iter().map(|&a| Object::Reference(a)).collect();
        self.dict(self.pages[page]).set("Annots", annots);
    }

    /// The widgets of `field`: its kids, or itself.
    pub fn widgets_of(&self, field: ObjectId) -> Vec<ObjectId> {
        let d = self.pdf.get_dictionary(field).expect("a field");
        match d.get(b"Kids").and_then(|k| k.as_array()) {
            Ok(kids) => kids.iter().filter_map(|k| k.as_reference().ok()).collect(),
            Err(_) => vec![field],
        }
    }

    /// Put `fields` under a new field `name` (no `/T` clash checks): they
    /// leave `/Fields`, and it joins.
    pub fn group(&mut self, name: &str, fields: &[ObjectId]) -> ObjectId {
        let kids: Vec<Object> = fields.iter().map(|&f| Object::Reference(f)).collect();
        let parent = self.pdf.add_object(dictionary! {
            "T" => Object::string_literal(name), "Kids" => kids,
        });
        for &f in fields {
            self.dict(f).set("Parent", parent);
        }
        let left: Vec<ObjectId> = self
            .fields()
            .into_iter()
            .filter(|f| !fields.contains(f))
            .chain([parent])
            .collect();
        self.set_fields(&left);
        parent
    }

    /// An `/XFA` copy that names the fields as they are now, as LiveCycle
    /// Designer would for a static form: a template with a subform per
    /// parent field and a field per terminal one (an exclusion group for a
    /// radio group), each with a tooltip ("Tip for name"), and datasets
    /// with their values. Field names should
    /// carry Designer's `[0]` indexes (`form1[0].name[0]`).
    pub fn xfa(&mut self) {
        let mut template = String::new();
        let mut data = String::new();
        for f in self.fields() {
            self.xfa_node(f, &mut template, &mut data);
        }
        let packet = |pdf: &mut Pdf, xml: String| {
            Object::Reference(pdf.add_object(Stream::new(dictionary! {}, xml.into_bytes())))
        };
        let preamble = packet(
            &mut self.pdf,
            r#"<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">"#.into(),
        );
        let template = packet(
            &mut self.pdf,
            format!(
                r#"<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">{template}</template>"#
            ),
        );
        let datasets = packet(
            &mut self.pdf,
            format!(
                r#"<xfa:datasets xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/"><xfa:data>{data}</xfa:data></xfa:datasets>"#
            ),
        );
        let postamble = packet(&mut self.pdf, "</xdp:xdp>".into());
        let xfa = vec![
            Object::string_literal("preamble"),
            preamble,
            Object::string_literal("template"),
            template,
            Object::string_literal("datasets"),
            datasets,
            Object::string_literal("postamble"),
            postamble,
        ];
        self.dict(self.acroform).set("XFA", xfa);
    }

    /// A dynamic XFA form: the XFA copy as [`Form::xfa`] makes it, then no
    /// AcroForm fields or widgets, and `/NeedsRendering`.
    pub fn xfa_only(&mut self) {
        self.xfa();
        self.set_fields(&[]);
        for page in 0..self.pages.len() {
            self.set_annots(page, &[]);
        }
        self.dict(self.catalog).set("NeedsRendering", true);
        self.pdf.prune_objects();
    }

    /// Reader usage rights: `/Perms /UR3`, a signature over the whole
    /// file, and `/SigFlags 2`.
    pub fn usage_rights(&mut self) {
        let sig = self.signature_dict(dictionary! {
            "Type" => "SigRef", "TransformMethod" => "UR3",
            "TransformParams" => dictionary! {
                "Type" => "TransformParams", "V" => "2.2",
                "Form" => vec![Object::Name(b"FillIn".to_vec())],
                "Document" => vec![Object::Name(b"FullSave".to_vec())],
            },
        });
        self.dict(self.catalog)
            .set("Perms", dictionary! { "UR3" => sig });
        self.dict(self.acroform).set("SigFlags", 2);
    }

    /// Sign signature field `field`: a `/V` over the whole file, and
    /// `/SigFlags 3`.
    pub fn sign(&mut self, field: ObjectId) {
        let sig = self.signature_dict(dictionary! {});
        self.dict(field).set("V", sig);
        self.dict(self.acroform).set("SigFlags", 3);
    }

    /// Certify with signature field `field`: [`Form::sign`], with a
    /// DocMDP reference that allows filling, and `/Perms /DocMDP`.
    pub fn certify(&mut self, field: ObjectId) {
        let sig = self.signature_dict(dictionary! {
            "Type" => "SigRef", "TransformMethod" => "DocMDP",
            "TransformParams" => dictionary! { "Type" => "TransformParams", "P" => 2, "V" => "1.2" },
        });
        self.dict(field).set("V", sig);
        self.dict(self.catalog)
            .set("Perms", dictionary! { "DocMDP" => sig });
        self.dict(self.acroform).set("SigFlags", 3);
    }

    /// A structure tree, as Acrobat tags a form: a `/Form` element per
    /// widget, with an object reference to it, in a parent tree; and
    /// `/Tabs /S` on every page.
    pub fn tag(&mut self) {
        let root = self.pdf.new_object_id();
        let document = self.pdf.new_object_id();
        let mut elements = Vec::new();
        let mut nums = Vec::new();
        for (i, page) in self.pages.clone().into_iter().enumerate() {
            self.dict(page).set("Tabs", "S");
            for w in self.annots(i) {
                let key = nums.len() as i64 / 2;
                let element = self.pdf.add_object(dictionary! {
                    "Type" => "StructElem", "S" => "Form", "P" => document, "Pg" => page,
                    "K" => dictionary! { "Type" => "OBJR", "Obj" => w, "Pg" => page },
                });
                self.dict(w).set("StructParent", key);
                elements.push(Object::Reference(element));
                nums.push(Object::Integer(key));
                nums.push(Object::Reference(element));
            }
        }
        let next = nums.len() as i64 / 2;
        self.pdf.objects.insert(
            document,
            dictionary! { "Type" => "StructElem", "S" => "Document", "P" => root, "K" => elements }
                .into(),
        );
        self.pdf.objects.insert(
            root,
            dictionary! {
                "Type" => "StructTreeRoot", "K" => document,
                "ParentTree" => dictionary! { "Nums" => nums },
                "ParentTreeNextKey" => next,
            }
            .into(),
        );
        let catalog = self.dict(self.catalog);
        catalog.set("StructTreeRoot", root);
        catalog.set("MarkInfo", dictionary! { "Marked" => true });
    }

    /// The file. Each signature's `/ByteRange` covers it all but the
    /// signature's `/Contents`.
    pub fn bytes(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        self.pdf.save_to(&mut out).expect("writing the fixture");
        let mut from = 0;
        for _ in &self.signatures {
            let at =
                from + find(&out[from..], RANGE_PLACEHOLDER).expect("a signature's /ByteRange");
            let contents = at + find(&out[at..], b"/Contents").expect("its /Contents");
            let lt = contents + find(&out[contents..], b"<").expect("a hex string");
            let gt = lt + find(&out[lt..], b">").expect("its end");
            let range = format!("[0 {:010} {:010} {:010}]", lt, gt + 1, out.len() - (gt + 1));
            out[at..at + RANGE_PLACEHOLDER.len()].copy_from_slice(range.as_bytes());
            from = gt;
        }
        out
    }

    /// The file, as a source in a store of its own.
    pub fn source(&mut self) -> Src {
        source(self.bytes())
    }

    fn widget(&mut self, page: usize, rect: Rect) -> ObjectId {
        let r: Vec<Object> = rect.iter().map(|&v| Object::from(v)).collect();
        let id = self.pdf.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "Rect" => r, "F" => 4,
            "P" => self.pages[page],
        });
        let mut annots = self.annots(page);
        annots.push(id);
        self.set_annots(page, &annots);
        id
    }

    fn add_field(&mut self, id: ObjectId) {
        let mut fields = self.fields();
        fields.push(id);
        self.set_fields(&fields);
    }

    fn rect_of(&self, w: ObjectId) -> Rect {
        let r: Vec<f32> = self
            .pdf
            .get_dictionary(w)
            .ok()
            .and_then(|d| d.get(b"Rect").ok())
            .and_then(|r| r.as_array().ok())
            .map(|r| {
                r.iter()
                    .filter_map(|v| v.as_float().ok())
                    .map(|v| v as f32)
                    .collect()
            })
            .unwrap_or_default();
        [r[0], r[1], r[2], r[3]]
    }

    /// A form XObject the size of `rect` drawing `content`, with `/DR`'s
    /// Helvetica as `/Helv`.
    fn appearance(&mut self, rect: Rect, content: Vec<u8>) -> ObjectId {
        let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
        self.pdf.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject", "Subtype" => "Form",
                "BBox" => vec![0.into(), 0.into(), Object::from(w), Object::from(h)],
                "Resources" => dictionary! { "Font" => dictionary! { "Helv" => self.helv } },
            },
            content,
        ))
    }

    /// An appearance that draws `text` in 10 pt Helvetica, as Acrobat
    /// marks a text field's.
    fn text_appearance(&mut self, rect: Rect, text: &str) -> ObjectId {
        let h = rect[3] - rect[1];
        let content = Content {
            operations: vec![
                Operation::new("BMC", vec![Object::Name(b"Tx".to_vec())]),
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["Helv".into(), 10.into()]),
                Operation::new("g", vec![0.into()]),
                Operation::new("Td", vec![2.into(), Object::from((h - 7.0) / 2.0)]),
                Operation::new("Tj", vec![Object::string_literal(text)]),
                Operation::new("ET", vec![]),
                Operation::new("EMC", vec![]),
            ],
        };
        self.appearance(rect, content.encode().unwrap())
    }

    /// `/N` and `/D` for a button: `on` draws a filled square, `/Off`
    /// nothing.
    fn states(&mut self, rect: Rect, on: &str) -> Dictionary {
        let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
        let mark = format!("0 g 2 2 {} {} re f", w - 4.0, h - 4.0).into_bytes();
        let on_ap = self.appearance(rect, mark);
        let off_ap = self.appearance(rect, Vec::new());
        let states = dictionary! { on => on_ap, "Off" => off_ap };
        dictionary! { "N" => states.clone(), "D" => states }
    }

    /// A signature dictionary with placeholder `/ByteRange` and
    /// `/Contents`, and `reference` (a `/SigRef`) if it isn't empty.
    fn signature_dict(&mut self, reference: Dictionary) -> ObjectId {
        let mut sig = dictionary! {
            "Type" => "Sig", "Filter" => "Adobe.PPKLite", "SubFilter" => "adbe.pkcs7.detached",
            "ByteRange" => vec![0.into(), 9_999_999_999i64.into(), 9_999_999_999i64.into(), 9_999_999_999i64.into()],
            "Contents" => Object::String(vec![0; CONTENTS_LEN], StringFormat::Hexadecimal),
            "M" => Object::string_literal("D:20261015120000Z"),
        };
        if !reference.is_empty() {
            sig.set("Reference", vec![Object::Dictionary(reference)]);
        }
        let id = self.pdf.add_object(sig);
        self.signatures.push(id);
        id
    }

    /// One field's part of the XFA template and data.
    fn xfa_node(&self, id: ObjectId, template: &mut String, data: &mut String) {
        let Ok(d) = self.pdf.get_dictionary(id) else {
            return;
        };
        let name = d.get(b"T").map(crate::file::pdf_text).unwrap_or_default();
        let bare = name.split('[').next().unwrap_or_default().to_string();
        // Designer's names are XML names: they name the data's elements.
        assert!(
            bare.starts_with(|c: char| c.is_alphabetic() || c == '_')
                && bare
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '-'),
            "not a name Designer would give a field: {name:?}"
        );
        let kids = refs(Some(d), b"Kids");
        let field_kids: Vec<ObjectId> = kids
            .iter()
            .copied()
            .filter(|&k| self.pdf.get_dictionary(k).is_ok_and(|k| k.has(b"T")))
            .collect();
        if !field_kids.is_empty() {
            template.push_str(&format!(r#"<subform name="{bare}">"#));
            data.push_str(&format!("<{bare}>"));
            for k in field_kids {
                self.xfa_node(k, template, data);
            }
            template.push_str("</subform>");
            data.push_str(&format!("</{bare}>"));
            return;
        }
        let value = match d.get(b"V") {
            Ok(Object::Name(n)) if n != b"Off" => String::from_utf8_lossy(n).into_owned(),
            Ok(v @ Object::String(..)) => crate::file::pdf_text(v),
            _ => String::new(),
        };
        let flags = d.get(b"Ff").and_then(|f| f.as_i64()).unwrap_or(0);
        // A tooltip, as the IRS forms' XFA copies have where the AcroForm
        // has none.
        let assist = format!("<assist><toolTip>Tip for {bare}</toolTip></assist>");
        if flags & RADIO != 0 {
            template.push_str(&format!(r#"<exclGroup name="{bare}">{assist}"#));
            for i in 0..kids.len() {
                template.push_str(&format!(r#"<field name="item{i}"/>"#));
            }
            template.push_str("</exclGroup>");
        } else {
            template.push_str(&format!(r#"<field name="{bare}">{assist}</field>"#));
        }
        let value = quick_xml::escape::escape(value);
        data.push_str(&format!("<{bare}>{value}</{bare}>"));
    }
}

/// A file's `bytes` as a source in a store of its own, named `form.pdf`.
pub fn source(bytes: Vec<u8>) -> Src {
    let kind = BlobKind::file(Path::new("form.pdf"), None);
    Store::new()
        .open(bytes.into(), kind)
        .expect("the file opens as a source")
}

/// Rows down a page, 30 pt apart.
pub fn row(i: usize) -> Rect {
    let top = 700.0 - 30.0 * i as f32;
    [72.0, top - 20.0, 272.0, top]
}

/// Two pages with one of every kind of field, filled where it can be,
/// under `form1[0]` as Designer names them: on page 1 `name[0]` (filled),
/// `empty[0]`, check boxes `on[0]` and `off[0]`, and the radio group
/// `pick[0]` (its second button on); on page 2 a combo box `state[0]`, a
/// list box `kind[0]` (both filled), a push button `print[0]` and the
/// signature field `sig[0]`.
pub fn every_kind() -> Form {
    let mut f = Form::new(2);
    let name = f.text("name[0]", 0, row(0));
    f.fill(name, "Ann Lee");
    let empty = f.text("empty[0]", 0, row(1));
    let on = f.check_box("on[0]", 0, row(2), true);
    let off = f.check_box("off[0]", 0, row(3), false);
    let radio = f.radio("pick[0]", 0, &[row(4), row(5)], Some(1));
    let combo = f.choice("state[0]", 1, row(0), &["CA", "NY"], true);
    f.fill(combo, "NY");
    let list = f.choice("kind[0]", 1, row(1), &["W-2", "1099"], false);
    f.fill(list, "1099");
    let button = f.push_button("print[0]", 1, row(2), "Print");
    let sig = f.signature("sig[0]", 1, row(3));
    f.group(
        "form1[0]",
        &[name, empty, on, off, radio, combo, list, button, sig],
    );
    f
}

/// A file that says one of each thing the opened version holds, on three
/// pages: a field on the first, a mark on the second, roman page labels,
/// a header/footer on the third alone, and a closed bookmark to the first
/// with one under it to the third.
pub fn every_fact() -> Form {
    let mut f = Form::new(3);
    f.text("name", 0, row(0));
    let main = crate::annot::to_pdf(&note(), &mut f.pdf).main;
    let main = f.pdf.add_object(main);
    f.set_annots(1, &[main]);
    let catalog = f.catalog;
    f.dict(catalog).set(
        "PageLabels",
        dictionary! { "Nums" => vec![0.into(), dictionary! { "S" => "r" }.into()] },
    );
    let mut texts: [String; 6] = Default::default();
    texts[0] = "Draft".into();
    let hf = HeaderFooter {
        texts,
        pages: Some(vec![PageId(2)]),
        ..Default::default()
    };
    let scope = crate::stamps::to_catalog(&hf, &[PageId(0), PageId(1), PageId(2)]);
    f.dict(catalog).set(crate::stamps::CATALOG_KEY, scope);
    let pages = f.pages.clone();
    let item = |title: &str, page: ObjectId| {
        dictionary! {
            "Title" => Object::string_literal(title),
            "Dest" => vec![page.into(), "Fit".into()],
        }
    };
    let mut closed = item("A", pages[0]);
    closed.set("Count", -1);
    outline_tree(&mut f, vec![(0, closed), (1, item("A1", pages[2]))]);
    f
}

/// `items` as `f`'s outline: each (depth, item) in pre-order, a depth at
/// most one below the item before's. The links between them are filled
/// in, and `/Count` where an item has none: how many items are below it.
/// The items' ids, in order.
pub fn outline_tree(f: &mut Form, items: Vec<(u16, Dictionary)>) -> Vec<ObjectId> {
    let root = f.pdf.new_object_id();
    let ids: Vec<ObjectId> = items.iter().map(|_| f.pdf.new_object_id()).collect();
    let depths: Vec<u16> = items.iter().map(|(depth, _)| *depth).collect();
    let mut dicts: Vec<Dictionary> = items.into_iter().map(|(_, item)| item).collect();
    // The items below `at` (None: the root), and those of them a level down.
    let below = |at: Option<usize>| {
        let (from, depth) = at.map_or((0, 0), |i| (i + 1, depths[i] + 1));
        let all: Vec<usize> = (from..depths.len())
            .take_while(|&j| depths[j] >= depth)
            .collect();
        let kids: Vec<usize> = all
            .iter()
            .copied()
            .filter(|&j| depths[j] == depth)
            .collect();
        (all.len(), kids)
    };
    let link = |dicts: &mut [Dictionary], parent: Option<usize>, root_dict: &mut Dictionary| {
        let (count, kids) = below(parent);
        for pair in kids.windows(2) {
            dicts[pair[0]].set("Next", ids[pair[1]]);
            dicts[pair[1]].set("Prev", ids[pair[0]]);
        }
        let parent_id = parent.map_or(root, |i| ids[i]);
        for &k in &kids {
            dicts[k].set("Parent", parent_id);
        }
        let (Some(&first), Some(&last)) = (kids.first(), kids.last()) else {
            return;
        };
        let at = match parent {
            Some(i) => &mut dicts[i],
            None => root_dict,
        };
        at.set("First", ids[first]);
        at.set("Last", ids[last]);
        if !at.has(b"Count") {
            at.set("Count", count as i64);
        }
    };
    let mut outlines = dictionary! { "Type" => "Outlines" };
    link(&mut dicts, None, &mut outlines);
    for i in 0..ids.len() {
        link(&mut dicts, Some(i), &mut outlines);
    }
    f.pdf.objects.insert(root, outlines.into());
    for (&id, item) in ids.iter().zip(dicts) {
        f.pdf.objects.insert(id, item.into());
    }
    let catalog = f.catalog;
    f.dict(catalog).set("Outlines", root);
    ids
}

/// The note [`every_fact`] puts on its second page, as sheaf reads it.
pub fn note() -> Annot {
    Annot {
        id: AnnotId(4),
        kind: AnnotKind::Note {
            pos: Pt { x: 1.0, y: 2.0 },
        },
        color: [1, 2, 3],
        opacity: 1.0,
        stroke: 1.0,
        author: "me".into(),
        contents: "mine".into(),
        modified: String::new(),
        created: String::new(),
        fill: None,
        dashed: false,
        cloudy: false,
        turn: Rotation::R0,
        name: String::new(),
    }
}

/// The stem [`every_mark_kind`]'s page stamp names its page by: no name
/// the generated cache gives, so writing and reading it never touches the
/// cache. [`mark_assets`] holds it.
pub const MARK_STAMP: &str = "fixture-stamp";

/// The stem [`every_mark_kind`]'s attachment names its file by, as
/// [`MARK_STAMP`] is.
pub const MARK_FILE: &str = "fixture-file";

/// The bytes of [`every_mark_kind`]'s attachment.
pub const MARK_FILE_BYTES: &[u8] = b"fixture notes\n";

/// What [`every_mark_kind`]'s stamp and attachment name, as a document
/// holds them: a page 100 by 50 points with a blue square in its corner
/// and [`MARK_FILE_BYTES`].
pub fn mark_assets() -> std::sync::Arc<crate::assets::MarkAssets> {
    let mut pdf = Pdf::with_version("1.7");
    let pages_id = pdf.new_object_id();
    let content = pdf.add_object(Stream::new(
        dictionary! {},
        b"0 0 1 rg 0 0 20 20 re f".to_vec(),
    ));
    let page = pdf.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => content,
        "MediaBox" => vec![0.into(), 0.into(), 100.into(), 50.into()],
    });
    pdf.objects.insert(
        pages_id,
        dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 }.into(),
    );
    let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    pdf.trailer.set("Root", catalog);
    let mut bytes = Vec::new();
    pdf.save_to(&mut bytes).unwrap();
    let assets = crate::assets::MarkAssets::new(Store::new());
    assets.intern_stamp(MARK_STAMP, bytes.into());
    assets.intern_file(MARK_FILE, MARK_FILE_BYTES.into());
    std::sync::Arc::new(assets)
}

/// One mark of every kind, not turned, in PageSpace within a visible box
/// of (−17, 62)–(555, 824), and after them the measurements of a
/// perimeter and an area, which a file holds as `/Vertices` where a
/// distance is `/L`: ids from 1, the reply and the status in the note's
/// thread, and the stamp and the attachment naming [`MARK_STAMP`] and
/// [`MARK_FILE`], which [`mark_assets`] holds.
pub fn every_mark_kind() -> Vec<Annot> {
    use crate::annot::{Box2, REVIEW};
    use crate::measure::{Measured, Scale};
    use crate::tape::{Tape, TapeLook};
    let b = |x: f32, y: f32, w: f32, h: f32| Box2 { x, y, w, h };
    let p = |x: f32, y: f32| Pt { x, y };
    let lines = vec![b(60.0, 700.0, 200.0, 14.0), b(60.0, 684.0, 120.0, 14.0)];
    let scale = || {
        Box::new(Scale {
            page: 1.0,
            page_unit: "in".into(),
            real: 10.0,
            unit: "ft".into(),
            precision: 2,
        })
    };
    let mut kinds = vec![
        AnnotKind::Highlight { rects: lines },
        AnnotKind::Note {
            pos: p(400.0, 700.0),
        },
        AnnotKind::FreeText {
            rect: b(60.0, 600.0, 180.0, 40.0),
            size: 11.0,
        },
        AnnotKind::Square {
            rect: b(300.0, 600.0, 80.0, 50.0),
        },
        AnnotKind::Ellipse {
            rect: b(400.0, 600.0, 60.0, 40.0),
        },
        AnnotKind::Line {
            a: p(60.0, 560.0),
            b: p(200.0, 520.0),
            arrow: true,
            arrow_start: true,
        },
        AnnotKind::Ink {
            paths: vec![vec![p(250.0, 560.0), p(270.0, 540.0), p(300.0, 555.0)]],
        },
        AnnotKind::Stamp {
            rect: b(320.0, 530.0, 24.0, 24.0),
            glyph: "✓".into(),
        },
        AnnotKind::Connector {
            rect: b(380.0, 530.0, 22.0, 22.0),
            label: "1".into(),
            partner: None,
        },
        AnnotKind::Tape {
            rect: b(450.0, 420.0, 70.0, 100.0),
            size: 10.0,
            tape: Tape::from_text("1.00 +\n2.00 +\n3.00 T"),
            look: TapeLook {
                rotation: 90,
                ..TapeLook::default()
            },
        },
        AnnotKind::Underline {
            rects: vec![b(60.0, 480.0, 150.0, 12.0)],
        },
        AnnotKind::StrikeOut {
            rects: vec![b(60.0, 460.0, 150.0, 12.0)],
        },
        AnnotKind::Squiggly {
            rects: vec![b(60.0, 440.0, 150.0, 12.0)],
        },
        AnnotKind::Replace {
            rects: vec![b(60.0, 420.0, 60.0, 12.0)],
            struck: "teh".into(),
            at: b(117.0, 416.0, 7.0, 7.0),
        },
        AnnotKind::Insert {
            at: b(200.0, 416.0, 7.0, 7.0),
        },
        AnnotKind::Reply {
            parent: "sheaf:2".into(),
        },
        AnnotKind::Status {
            parent: "sheaf:2".into(),
            model: REVIEW.into(),
            state: "Accepted".into(),
        },
        AnnotKind::PageStamp {
            rect: b(60.0, 300.0, 100.0, 50.0),
            stamp: MARK_STAMP.into(),
            name: "Approved".into(),
            text: Some("GE".into()),
        },
        AnnotKind::TextStamp {
            rect: b(200.0, 300.0, 120.0, 50.0),
            text: "APPROVED\nGE 10/16/2026".into(),
            status: false,
        },
        AnnotKind::PageBorder {
            rect: b(-12.0, 67.0, 562.0, 752.0),
            status: "Reviewed".into(),
        },
        AnnotKind::Polygon {
            points: vec![p(350.0, 300.0), p(420.0, 310.0), p(380.0, 370.0)],
        },
        AnnotKind::Polyline {
            points: vec![p(430.0, 300.0), p(470.0, 330.0), p(520.0, 300.0)],
        },
        AnnotKind::Callout {
            rect: b(60.0, 200.0, 150.0, 36.0),
            knee: p(240.0, 210.0),
            point: p(300.0, 180.0),
            size: 12.0,
        },
        AnnotKind::FileAttachment {
            pos: p(500.0, 700.0),
            icon: "Paperclip".into(),
            file: MARK_FILE.into(),
            name: "notes.txt".into(),
            size: MARK_FILE_BYTES.len() as u64,
        },
        AnnotKind::Measure {
            what: Measured::Distance,
            points: vec![p(250.0, 200.0), p(400.0, 250.0)],
            scale: scale(),
        },
    ];
    for (i, kind) in kinds.iter().enumerate() {
        // Every kind, in this order: a new one doesn't compile until it's
        // here too.
        let at = match kind {
            AnnotKind::Highlight { .. } => 0,
            AnnotKind::Note { .. } => 1,
            AnnotKind::FreeText { .. } => 2,
            AnnotKind::Square { .. } => 3,
            AnnotKind::Ellipse { .. } => 4,
            AnnotKind::Line { .. } => 5,
            AnnotKind::Ink { .. } => 6,
            AnnotKind::Stamp { .. } => 7,
            AnnotKind::Connector { .. } => 8,
            AnnotKind::Tape { .. } => 9,
            AnnotKind::Underline { .. } => 10,
            AnnotKind::StrikeOut { .. } => 11,
            AnnotKind::Squiggly { .. } => 12,
            AnnotKind::Replace { .. } => 13,
            AnnotKind::Insert { .. } => 14,
            AnnotKind::Reply { .. } => 15,
            AnnotKind::Status { .. } => 16,
            AnnotKind::PageStamp { .. } => 17,
            AnnotKind::TextStamp { .. } => 18,
            AnnotKind::PageBorder { .. } => 19,
            AnnotKind::Polygon { .. } => 20,
            AnnotKind::Polyline { .. } => 21,
            AnnotKind::Callout { .. } => 22,
            AnnotKind::FileAttachment { .. } => 23,
            AnnotKind::Measure { .. } => 24,
        };
        assert_eq!(at, i, "{kind:?} is out of place");
    }
    kinds.extend([
        AnnotKind::Measure {
            what: Measured::Perimeter,
            points: vec![p(250.0, 120.0), p(330.0, 160.0), p(400.0, 110.0)],
            scale: scale(),
        },
        AnnotKind::Measure {
            what: Measured::Area,
            points: vec![p(420.0, 120.0), p(520.0, 120.0), p(500.0, 180.0)],
            scale: scale(),
        },
    ]);
    kinds
        .into_iter()
        .enumerate()
        .map(|(i, kind)| Annot {
            id: AnnotId(i as u64 + 1),
            fill: kind.fillable().then_some([255, 255, 0]),
            dashed: matches!(kind, AnnotKind::Square { .. }),
            cloudy: matches!(kind, AnnotKind::Ellipse { .. } | AnnotKind::Polygon { .. }),
            kind,
            color: [220, 40, 40],
            opacity: 0.8,
            stroke: 1.5,
            author: "Ann".into(),
            contents: "About this".into(),
            modified: "D:20261016120000".into(),
            created: "D:20261015120000".into(),
            turn: Rotation::R0,
            name: String::new(),
        })
        .collect()
}

/// `bytes` with an incremental update appended: `change` edits a copy of
/// each object in `ids` (and anything it adds) in the new revision.
pub fn append_revision(bytes: &[u8], ids: &[ObjectId], change: impl FnOnce(&mut Pdf)) -> Vec<u8> {
    let prev = Pdf::load_mem(bytes).expect("the previous revision");
    let mut doc = lopdf::IncrementalDocument::create_from(bytes.to_vec(), prev);
    for &id in ids {
        doc.opt_clone_object_to_new_document(id)
            .expect("an object in the previous revision");
    }
    change(&mut doc.new_document);
    let mut out = Vec::new();
    doc.save_to(&mut out).expect("writing the update");
    out
}

/// A one-page form written as a hybrid-reference file (ISO 32000
/// 7.5.8.4), in one section with no `/Prev`: the cross-reference table
/// lists the catalog (1), the pages (2), the page (3) and an appearance
/// (4), and the form's objects, the `/AcroForm` (5) and text fields `name`
/// filled with "Ann Lee" (6) and `city` filled with "Hull" (7), are in an
/// object stream that only the cross-reference stream at the trailer's
/// `/XRefStm` lists.
pub fn hybrid_reference() -> Vec<u8> {
    let mut out = b"%PDF-1.5\n%\xe2\xe3\xcf\xd3\n".to_vec();
    hybrid_section(
        &mut out,
        &[
            (1, "<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R >>"),
            (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
            (3, &hybrid_page("6 0 R 7 0 R")),
            (
                4,
                "<< /Type /XObject /Subtype /Form /BBox [0 0 200 20] /Length 0 >>\n\
                 stream\n\nendstream",
            ),
        ],
        &[],
        &[
            (5, "<< /Fields [6 0 R 7 0 R] >>"),
            (6, &hybrid_text("name", "Ann Lee", 0)),
            (7, &hybrid_text("city", "Hull", 1)),
        ],
        None,
    );
    out
}

/// [`hybrid_reference`]'s bytes, `first`, with an update appended that
/// fills `name` with "Bo Ray" and adds a signature field `sig` (10,
/// unsigned), as the update in the SafeDocs files lopdf 0.45 misread adds
/// a signed one. The update's table lists the page, with the new widget.
/// Its cross-reference stream lists a new catalog, which the first table
/// lists too, and in a new object stream `name`, `sig` and a new
/// `/AcroForm` (11). Its trailer's `/Prev` is the first section. `city`
/// is only in the first section's object stream, so the update reads
/// whole only through the first section's `/XRefStm` too.
pub fn hybrid_update(first: &[u8]) -> Vec<u8> {
    let tail = first.windows(9).rposition(|w| w == b"startxref").unwrap();
    let prev = String::from_utf8_lossy(&first[tail + 9..]);
    let prev = prev.split_whitespace().next().unwrap().parse().unwrap();
    let mut out = first.to_vec();
    hybrid_section(
        &mut out,
        &[(3, &hybrid_page("6 0 R 7 0 R 10 0 R"))],
        &[(1, "<< /Type /Catalog /Pages 2 0 R /AcroForm 11 0 R >>")],
        &[
            (6, &hybrid_text("name", "Bo Ray", 0)),
            (
                10,
                "<< /Type /Annot /Subtype /Widget /FT /Sig /T (sig) \
                 /Rect [72 620 272 640] /F 4 /P 3 0 R >>",
            ),
            (11, "<< /Fields [6 0 R 7 0 R 10 0 R] >>"),
        ],
        Some(prev),
    );
    out
}

fn hybrid_page(annots: &str) -> String {
    format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [{annots}] >>")
}

/// A text field `name` filled with `value`, its widget in [`row`] `i`.
fn hybrid_text(name: &str, value: &str, i: usize) -> String {
    let [x0, y0, x1, y1] = row(i);
    format!(
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T ({name}) /V ({value}) /AP << /N 4 0 R >> \
         /Rect [{x0} {y0} {x1} {y1}] /F 4 /P 3 0 R >>"
    )
}

/// One section of a hybrid-reference file, appended to `out`: `listed`
/// in its table, `hidden` only in its cross-reference stream, and
/// `packed` into an object stream that only the cross-reference stream
/// lists. The two streams take the next two object numbers.
fn hybrid_section(
    out: &mut Vec<u8>,
    listed: &[(u32, &str)],
    hidden: &[(u32, &str)],
    packed: &[(u32, &str)],
    prev: Option<usize>,
) {
    let all = listed.iter().chain(hidden).chain(packed);
    let objstm = all.map(|&(id, _)| id).max().unwrap() + 1;
    let xrefstm = objstm + 1;
    let object = |out: &mut Vec<u8>, id: u32, body: &str| {
        out.extend_from_slice(format!("{id} 0 obj\n{body}\nendobj\n").as_bytes());
    };
    let mut table = Vec::new();
    for &(id, body) in listed {
        table.push((id, out.len()));
        object(out, id, body);
    }
    // The stream's entries, /W [1 4 2]: a type, then an offset and a
    // generation, or an object stream and an index in it.
    let mut entries = BTreeMap::new();
    for &(id, body) in hidden {
        entries.insert(id, (1u8, out.len() as u32, 0u16));
        object(out, id, body);
    }
    let (mut head, mut objects) = (String::new(), String::new());
    for (i, &(id, body)) in packed.iter().enumerate() {
        head.push_str(&format!("{id} {} ", objects.len()));
        objects.push_str(body);
        objects.push('\n');
        entries.insert(id, (2, objstm, i as u16));
    }
    entries.insert(objstm, (1, out.len() as u32, 0));
    let n = packed.len();
    let first = head.len();
    let length = first + objects.len();
    let header = format!("<< /Type /ObjStm /N {n} /First {first} /Length {length} >>");
    object(
        out,
        objstm,
        &format!("{header}\nstream\n{head}{objects}\nendstream"),
    );

    let at = out.len();
    entries.insert(xrefstm, (1, at as u32, 0));
    let mut data = Vec::new();
    let mut index = String::new();
    for (id, (kind, a, b)) in entries {
        data.push(kind);
        data.extend_from_slice(&a.to_be_bytes());
        data.extend_from_slice(&b.to_be_bytes());
        index.push_str(&format!("{id} 1 "));
    }
    let size = xrefstm + 1;
    let dict = format!(
        "/Type /XRef /Size {size} /W [1 4 2] /Index [{index}] /Length {}",
        data.len()
    );
    out.extend_from_slice(format!("{xrefstm} 0 obj\n<< {dict} >>\nstream\n").as_bytes());
    out.extend_from_slice(&data);
    out.extend_from_slice(b"\nendstream\nendobj\n");

    let xref = out.len();
    out.extend_from_slice(b"xref\n");
    if prev.is_none() {
        out.extend_from_slice(b"0 1\n0000000000 65535 f \n");
    }
    for (id, offset) in table {
        out.extend_from_slice(format!("{id} 1\n{offset:010} 00000 n \n").as_bytes());
    }
    let prev = prev.map(|p| format!(" /Prev {p}")).unwrap_or_default();
    let trailer = format!("<< /Size {size} /Root 1 0 R /XRefStm {at}{prev} >>");
    out.extend_from_slice(format!("trailer\n{trailer}\nstartxref\n{xref}\n%%EOF\n").as_bytes());
}

/// The references in an array under `key`.
fn refs(d: Option<&Dictionary>, key: &[u8]) -> Vec<ObjectId> {
    d.and_then(|d| d.get(key).ok())
        .and_then(|a| a.as_array().ok())
        .map(|a| a.iter().filter_map(|o| o.as_reference().ok()).collect())
        .unwrap_or_default()
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
