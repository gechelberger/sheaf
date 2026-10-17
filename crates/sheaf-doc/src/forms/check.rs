//! The form check: what in a file's form other viewers would trip on, or
//! what a save would carry where it shouldn't (M10 slice 1, from the
//! planning probe). Every slice that writes forms asserts it on what it
//! writes, and `sheaf form check` prints it.
//!
//! It reads the file as it is, with no field model:
//! - widgets no field in `/Fields` reaches, widgets on no page, on more
//!   than one, or whose `/P` is wrong or null;
//! - kids whose `/Parent` isn't the field that lists them;
//! - fonts a `/DA` names that `/DR` lacks;
//! - full names more than one field has;
//! - values with no appearance (what PDFium's screen hides, bug 10);
//! - values an earlier revision holds that the current one doesn't;
//! - `/XFA`, `/Perms` and `/SigFlags`, and whether they still fit the file.
//!
//! The tree comes from untrusted files: it's read with a visited set,
//! within the form reader's caps (`FormLimits`): at most 32 deep and
//! 100,000 nodes, with full names of at most 64 KiB and 64 MiB in all, and
//! what can't be read is reported, not followed.
//! Nothing else is copied per field: values are compared by a hash of
//! their text, and an object many fields share is read once.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::path::Path;

use anyhow::{Context, Result};
use lopdf::{Dictionary, Document as Pdf, Object, ObjectId};
use quick_xml::events::{BytesStart, Event};

use super::read::FormClass;
use crate::attachments::resolve;
use crate::content::{self, ResCx, Tok};
use crate::file::pdf_text;
use crate::source::{FormLimits, Limits};

/// How much of a widget's name, read up its `/Parent` chain, a finding
/// keeps.
const SHOWN_NAME: usize = 256;
/// The caps the check reads within: the form reader's.
const LIMITS: FormLimits = Limits::DEFAULT.forms;
/// Earlier revisions read, the latest ones: each is a parse of the file
/// up to it.
const MAX_REVISIONS: usize = 256;
/// Names shown as examples in a finding, and the characters shown of each.
const EXAMPLES: usize = 3;
const EXAMPLE_CHARS: usize = 80;

/// What the check found in one file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    pub pages: usize,
    /// Terminal fields `/Fields` reaches.
    pub fields: usize,
    /// Widgets on pages.
    pub widgets: usize,
    /// Widgets on page templates (`/Names /Templates`), which a form's
    /// scripts spawn pages from. They're on no page, and that's fine.
    pub template_widgets: usize,
    pub xfa: Option<Xfa>,
    pub need_appearances: bool,
    /// `/Perms /UR3` (or `/UR`): Reader's usage rights.
    pub usage_rights: bool,
    /// `/Perms /DocMDP`: a certification.
    pub certified: bool,
    /// `/AcroForm /SigFlags`.
    pub sig_flags: i64,
    pub signature_fields: usize,
    /// Signature fields with a signature in them.
    pub signed: usize,
    /// The original and each incremental update that reads as a file (of
    /// the last 256).
    pub revisions: usize,
    pub findings: Vec<Finding>,
}

/// A form's XFA copy.
#[derive(Debug, Clone, PartialEq)]
pub struct Xfa {
    /// Its packets' names, in order; `xdp` when it's one stream.
    pub packets: Vec<String>,
    /// The catalog's `/NeedsRendering`: the XFA is the form, and the
    /// pages are a placeholder.
    pub dynamic: bool,
}

/// One problem. Lists of names hold full names, sorted: a widget's is its
/// field's.
#[derive(Debug, Clone, PartialEq)]
pub enum Finding {
    /// Widgets on pages that no field in `/Fields` reaches (bug 2).
    Orphans(Vec<String>),
    /// Widgets in the file on no page and no page template (bugs 2 and
    /// 5).
    OffPage(Vec<String>),
    /// Widgets in more than one page's `/Annots` (bug 4).
    SharedWidgets(Vec<String>),
    /// Widgets whose `/P` is another page.
    WrongPage(Vec<String>),
    /// Widgets whose `/P` is null, or an object the file doesn't have.
    NullPage(Vec<String>),
    /// Fields whose `/Kids` hold a field or widget whose `/Parent` isn't
    /// them: viewers that name fields up `/Parent` (pdf.js) and those that
    /// follow `/Kids` (PDFium) see different fields.
    WrongParent(Vec<String>),
    /// Fonts a `/DA` names that `/DR` lacks, each with how many widgets
    /// name it.
    MissingFonts(Vec<(String, usize)>),
    /// Full names more than one field has (bug 3).
    DuplicateNames(Vec<String>),
    /// Fields with a value and no appearance for it on their widgets.
    NoAppearance(Vec<String>),
    /// Values revision `revision` (from 1) holds that the current one
    /// doesn't, by field.
    EarlierValues { revision: usize, names: Vec<String> },
    /// The XFA copy describes fields the AcroForm lacks, or the other way
    /// round (bug 6).
    XfaMismatch {
        xfa_only: Vec<String>,
        acroform_only: Vec<String>,
    },
    /// A signature whose `/ByteRange` isn't a revision of this file
    /// (bug 7): it covers `covers` bytes of `size`.
    StaleSignature {
        what: String,
        covers: u64,
        size: u64,
    },
    /// `/SigFlags` says there are signatures, or signature fields, and
    /// there aren't.
    StaleSigFlags { flags: i64, why: &'static str },
    /// Why the field tree couldn't be read whole.
    Damaged(String),
}

impl Report {
    pub fn is_clean(&self) -> bool {
        self.findings.is_empty()
    }

    /// "AcroForm", "hybrid" (with an XFA copy), "XFA-only" (dynamic, or
    /// with no AcroForm fields) or "no form".
    pub fn class(&self) -> &'static str {
        let dynamic = self.xfa.as_ref().is_some_and(|x| x.dynamic);
        FormClass::of(self.xfa.is_some(), dynamic, self.fields).label()
    }
}

impl Finding {
    /// The kind of problem, for tallies.
    pub fn kind(&self) -> &'static str {
        match self {
            Finding::Orphans(_) => "widgets no field reaches",
            Finding::OffPage(_) => "widgets on no page",
            Finding::SharedWidgets(_) => "widgets on more than one page",
            Finding::WrongPage(_) => "widgets with the wrong /P",
            Finding::NullPage(_) => "widgets with a null /P",
            Finding::WrongParent(_) => "kids with the wrong /Parent",
            Finding::MissingFonts(_) => "fonts missing from /DR",
            Finding::DuplicateNames(_) => "duplicate full names",
            Finding::NoAppearance(_) => "values with no appearance",
            Finding::EarlierValues { .. } => "values in earlier revisions",
            Finding::XfaMismatch { .. } => "an XFA copy that doesn't fit",
            Finding::StaleSignature { .. } => "signatures that don't fit",
            Finding::StaleSigFlags { .. } => "SigFlags with nothing to flag",
            Finding::Damaged(_) => "damaged field trees",
        }
    }
}

/// Check the file in `bytes`, decrypted with `password` (or the empty
/// password) if it's encrypted.
pub fn check(bytes: &[u8], password: Option<&str>) -> Result<Report> {
    let pdf = crate::file::load_pdf_mem(bytes, password)?;
    Ok(check_pdf(&pdf, bytes, password))
}

/// [`check`] for a file on disk.
pub fn check_file(path: &Path, password: Option<&str>) -> Result<Report> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    check(&bytes, password).with_context(|| format!("checking {}", path.display()))
}

fn check_pdf(pdf: &Pdf, bytes: &[u8], password: Option<&str>) -> Report {
    let catalog = pdf.catalog().ok();
    let form = acroform(pdf);
    let tree = read_tree(pdf, form);
    let pages = on_pages(pdf);
    let templates = on_templates(pdf, &pages);
    let mut report = Report {
        pages: pages.count,
        fields: tree.fields.len(),
        widgets: pages.of.len(),
        template_widgets: templates.len(),
        need_appearances: form
            .and_then(|f| f.get(b"NeedAppearances").ok())
            .and_then(|o| o.as_bool().ok())
            .unwrap_or(false),
        sig_flags: form
            .and_then(|f| f.get(b"SigFlags").ok())
            .and_then(|o| resolve(pdf, o))
            .and_then(|o| o.as_i64().ok())
            .unwrap_or(0),
        ..Default::default()
    };
    let mut findings = Vec::new();

    // The widgets themselves.
    let orphans = pages.of.keys().filter(|w| !tree.widgets.contains(w));
    let off_page = pdf.objects.iter().filter(|(id, o)| {
        o.as_dict().is_ok_and(is_widget) && !pages.of.contains_key(id) && !templates.contains(id)
    });
    let shared = pages.of.iter().filter(|(_, p)| p.len() > 1);
    let widgets: [(Vec<ObjectId>, Named); 5] = [
        (orphans.copied().collect(), Finding::Orphans),
        (off_page.map(|(&id, _)| id).collect(), Finding::OffPage),
        (shared.map(|(&w, _)| w).collect(), Finding::SharedWidgets),
        (pages.wrong.clone(), Finding::WrongPage),
        (pages.null.clone(), Finding::NullPage),
    ];
    for (ids, finding) in widgets {
        let names = ids.into_iter().map(|w| name_up(pdf, w)).collect();
        push(&mut findings, names, finding);
    }
    push(&mut findings, tree.wrong_parent, Finding::WrongParent);

    // Fonts, names and appearances.
    let missing = missing_fonts(pdf, form, &tree.fields);
    if !missing.is_empty() {
        findings.push(Finding::MissingFonts(missing));
    }
    let mut by_name: BTreeMap<&str, usize> = BTreeMap::new();
    for f in tree.fields.iter().filter(|f| !f.name.is_empty()) {
        *by_name.entry(&f.name).or_default() += 1;
    }
    let duplicates = by_name.into_iter().filter(|&(_, n)| n > 1);
    let duplicates = duplicates.map(|(name, _)| name.to_string()).collect();
    push(&mut findings, duplicates, Finding::DuplicateNames);
    let bare = tree.fields.iter().filter(|f| !shows_value(pdf, f, &pages));
    let bare = bare.map(|f| f.name.clone()).collect();
    push(&mut findings, bare, Finding::NoAppearance);

    // Earlier revisions, from the original.
    let ends = revision_ends(bytes);
    let now = values(&tree.fields);
    report.revisions = 1;
    let earlier = &ends[..ends.len().saturating_sub(1)];
    for &(_, end) in &earlier[earlier.len().saturating_sub(MAX_REVISIONS)..] {
        let Ok(earlier) = crate::file::load_pdf_mem(&bytes[..end], password) else {
            continue;
        };
        if earlier.xref_start == 0 || earlier.get_pages().is_empty() {
            // Not a revision: lopdf rebuilt its cross-references (the cut
            // is inside a stream), or it's a linearized file's first-page
            // section.
            continue;
        }
        let then = read_tree(&earlier, acroform(&earlier));
        let names: Vec<String> = values(&then.fields)
            .into_iter()
            .filter(|(name, v)| now.get(name) != Some(v))
            .map(|(name, _)| name.to_string())
            .collect();
        if !names.is_empty() {
            findings.push(Finding::EarlierValues {
                revision: report.revisions,
                names,
            });
        }
        report.revisions += 1;
    }

    // XFA.
    if let Some((packets, template)) = form.and_then(|f| xfa_of(pdf, f)) {
        let dynamic = catalog
            .and_then(|c| c.get(b"NeedsRendering").ok())
            .and_then(|o| o.as_bool().ok())
            .unwrap_or(false);
        if !dynamic && let Some(xfa) = template.as_deref().and_then(xfa_paths) {
            let acroform: BTreeSet<String> =
                tree.fields.iter().map(|f| som_path(&f.name)).collect();
            let xfa_only: Vec<String> = xfa.difference(&acroform).cloned().collect();
            let acroform_only: Vec<String> = acroform.difference(&xfa).cloned().collect();
            if !xfa_only.is_empty() || !acroform_only.is_empty() {
                findings.push(Finding::XfaMismatch {
                    xfa_only,
                    acroform_only,
                });
            }
        }
        report.xfa = Some(Xfa { packets, dynamic });
    }

    // Signatures: usage rights, a certification, signed fields.
    let ends: HashSet<usize> = ends
        .iter()
        .flat_map(|&(at, end)| [at, end])
        .chain([bytes.len()])
        .collect();
    let perms = catalog
        .and_then(|c| c.get(b"Perms").ok())
        .and_then(|p| resolve(pdf, p))
        .and_then(|p| p.as_dict().ok());
    let perm = |key: &[u8]| {
        let o = perms?.get(key).ok()?;
        Some((o.as_reference().ok(), resolve(pdf, o)?.as_dict().ok()?))
    };
    let mut signatures: Vec<(String, &Dictionary)> = Vec::new();
    let mut seen = HashSet::new();
    if let Some((_, sig)) = perm(b"UR3").or_else(|| perm(b"UR")) {
        report.usage_rights = true;
        signatures.push(("usage rights (/Perms /UR3)".into(), sig));
    }
    if let Some((id, sig)) = perm(b"DocMDP") {
        report.certified = true;
        seen.extend(id);
        signatures.push(("the certification (/Perms /DocMDP)".into(), sig));
    }
    for f in tree.fields.iter().filter(|f| f.ft == b"Sig") {
        report.signature_fields += 1;
        let Some(v) = f.value else { continue };
        let (id, sig) = match v {
            Object::Reference(id) => (Some(*id), pdf.get_dictionary(*id).ok()),
            o => (None, o.as_dict().ok()),
        };
        let Some(sig) = sig.filter(|s| s.has(b"ByteRange")) else {
            continue;
        };
        report.signed += 1;
        if id.is_none_or(|id| seen.insert(id)) {
            signatures.push((format!("the signature in {:?}", f.name), sig));
        }
    }
    for (what, sig) in signatures {
        if let Err(covers) = byte_range_fits(sig, bytes, &ends) {
            findings.push(Finding::StaleSignature {
                what,
                covers,
                size: bytes.len() as u64,
            });
        }
    }
    let flags = report.sig_flags;
    if flags & 1 != 0 && report.signature_fields == 0 {
        findings.push(Finding::StaleSigFlags {
            flags,
            why: "no signature field",
        });
    } else if flags & 2 != 0 && report.signed == 0 && !report.usage_rights && !report.certified {
        findings.push(Finding::StaleSigFlags {
            flags,
            why: "nothing signed",
        });
    }

    for reason in tree.damaged {
        findings.push(Finding::Damaged(reason));
    }
    report.findings = findings;
    report
}

/// A finding that names fields or widgets.
type Named = fn(Vec<String>) -> Finding;

/// `finding(names)`, sorted, unless there are none.
fn push(findings: &mut Vec<Finding>, mut names: Vec<String>, finding: Named) {
    if !names.is_empty() {
        names.sort();
        findings.push(finding(names));
    }
}

fn acroform(pdf: &Pdf) -> Option<&Dictionary> {
    let form = pdf.catalog().ok()?.get(b"AcroForm").ok()?;
    resolve(pdf, form)?.as_dict().ok()
}

/// A terminal field, as the check reads it: inherited keys resolved.
struct Field<'a> {
    name: String,
    ft: &'a [u8],
    /// `/V`, resolved, but a signature stays a reference (see
    /// [`resolve_once`]).
    value: Option<&'a Object>,
    /// A hash of the value's text (see [`Tree::digest`]).
    digest: Option<u64>,
    /// Its own `/DA`, or one it inherits from a parent field.
    da: Option<&'a [u8]>,
    widgets: Vec<ObjectId>,
}

impl Field<'_> {
    /// The value's hash; None when it's empty, or `Off` for a button.
    fn value(&self) -> Option<u64> {
        if self.ft == b"Btn" && self.state() == Some(&b"Off"[..]) {
            return None;
        }
        self.digest
    }

    /// A button's value: the state its widgets show it in.
    fn state(&self) -> Option<&[u8]> {
        match self.value? {
            Object::Name(n) => Some(n.as_slice()),
            Object::String(s, _) => Some(s.as_slice()),
            _ => None,
        }
    }
}

#[derive(Default)]
struct Tree<'a> {
    fields: Vec<Field<'a>>,
    /// Every widget the tree reaches.
    widgets: HashSet<ObjectId>,
    /// Fields with kids whose `/Parent` isn't them.
    wrong_parent: Vec<String>,
    damaged: BTreeSet<String>,
    seen: HashSet<ObjectId>,
    nodes: usize,
    /// Bytes of full names so far.
    names: usize,
    /// Value hashes by object, for values and for list items.
    values: HashMap<ObjectId, Option<u64>>,
    texts: HashMap<ObjectId, Option<u64>>,
}

/// What a field passes down to its kids.
#[derive(Clone, Copy, Default)]
struct Inherited<'a> {
    ft: &'a [u8],
    value: Option<&'a Object>,
    digest: Option<u64>,
    da: Option<&'a [u8]>,
}

fn read_tree<'a>(pdf: &'a Pdf, form: Option<&'a Dictionary>) -> Tree<'a> {
    let mut tree = Tree::default();
    let roots = form
        .and_then(|f| f.get(b"Fields").ok())
        .and_then(|f| resolve(pdf, f))
        .and_then(|f| f.as_array().ok());
    for root in roots.into_iter().flatten() {
        if let Ok(id) = root.as_reference() {
            tree.node(pdf, id, Inherited::default(), "", 1);
        }
    }
    tree
}

impl<'a> Tree<'a> {
    /// Read field `id`, a kid of the field with full name `up_name`.
    fn node(&mut self, pdf: &'a Pdf, id: ObjectId, up: Inherited<'a>, up_name: &str, depth: usize) {
        if depth > LIMITS.depth {
            let depth = LIMITS.depth;
            self.damaged
                .insert(format!("the field tree is more than {depth} deep"));
            return;
        }
        self.nodes += 1;
        if self.nodes > LIMITS.nodes {
            let nodes = LIMITS.nodes;
            self.damaged
                .insert(format!("the field tree has more than {nodes} nodes"));
            return;
        }
        if self.names > LIMITS.names {
            let mib = LIMITS.names >> 20;
            self.damaged
                .insert(format!("the full names come to more than {mib} MiB"));
            return;
        }
        if !self.seen.insert(id) {
            self.damaged.insert(twice());
            return;
        }
        let Ok(dict) = pdf.get_dictionary(id) else {
            return;
        };
        let name = match dict.get(b"T").ok().and_then(|t| resolve(pdf, t)) {
            Some(t) if t.as_str().is_ok_and(|s| s.len() > LIMITS.name) => None,
            Some(t) if up_name.is_empty() => Some(pdf_text(t)),
            Some(t) => Some(format!("{up_name}.{}", pdf_text(t))),
            None => Some(up_name.to_string()),
        };
        let Some(name) = name.filter(|n| n.len() <= LIMITS.name) else {
            let kib = LIMITS.name >> 10;
            self.damaged
                .insert(format!("a full name is longer than {kib} KiB"));
            return;
        };
        self.names += name.len();
        let mut here = up;
        if let Some(ft) = dict.get(b"FT").ok().and_then(|o| o.as_name().ok()) {
            here.ft = ft;
        }
        if let Ok(v) = dict.get(b"V")
            && let Some(value) = resolve_once(pdf, v)
        {
            here.value = Some(value);
            here.digest = self.digest(pdf, v);
        }
        if let Some(da) = da_of(pdf, dict) {
            here.da = Some(da);
        }
        let kids: Vec<ObjectId> = dict
            .get(b"Kids")
            .ok()
            .and_then(|k| resolve(pdf, k))
            .and_then(|k| k.as_array().ok())
            .map(|k| k.iter().filter_map(|o| o.as_reference().ok()).collect())
            .unwrap_or_default();
        let mut widgets = Vec::new();
        if is_widget(dict) {
            widgets.push(id);
        }
        let mut field_kids = false;
        let mut wrong_parent = false;
        for kid in kids {
            let Ok(k) = pdf.get_dictionary(kid) else {
                continue;
            };
            wrong_parent |= parent_of(k) != Some(id);
            // A kid with a name is a field; one without is a widget,
            // unless it has kids of its own.
            if k.has(b"T") || (!is_widget(k) && k.has(b"Kids")) {
                field_kids = true;
                self.node(pdf, kid, here, &name, depth + 1);
            } else if self.seen.insert(kid) {
                widgets.push(kid);
            } else {
                self.damaged.insert(twice());
            }
        }
        if wrong_parent {
            self.wrong_parent.push(name.clone());
        }
        self.widgets.extend(&widgets);
        if !widgets.is_empty() || !field_kids {
            self.fields.push(Field {
                name,
                ft: here.ft,
                value: here.value,
                digest: here.digest,
                da: here.da,
                widgets,
            });
        }
    }

    /// A hash of a value's text (each item's, for a list), to compare
    /// values across revisions by; None when it's empty.
    fn digest(&mut self, pdf: &Pdf, v: &Object) -> Option<u64> {
        if let Ok(id) = v.as_reference()
            && let Some(&digest) = self.values.get(&id)
        {
            return digest;
        }
        let items = match resolve(pdf, v) {
            Some(Object::Array(items)) => items.as_slice(),
            _ => std::slice::from_ref(v),
        };
        let mut h = DefaultHasher::new();
        let mut empty = true;
        for item in items {
            if let Some(text) = self.text(pdf, item) {
                text.hash(&mut h);
                empty = false;
            }
        }
        let digest = (!empty).then(|| h.finish());
        if let Ok(id) = v.as_reference() {
            self.values.insert(id, digest);
        }
        digest
    }

    /// A string's, name's or stream's text, hashed; None when it's empty.
    fn text(&mut self, pdf: &Pdf, o: &Object) -> Option<u64> {
        if let Ok(id) = o.as_reference()
            && let Some(&text) = self.texts.get(&id)
        {
            return text;
        }
        let text = match resolve(pdf, o) {
            Some(s @ Object::String(..)) => hash(pdf_text(s).as_bytes()),
            Some(Object::Name(n)) => hash(n),
            Some(Object::Stream(s)) => hash(&s.content),
            _ => None,
        };
        if let Ok(id) = o.as_reference() {
            self.texts.insert(id, text);
        }
        text
    }
}

fn hash(bytes: &[u8]) -> Option<u64> {
    if bytes.is_empty() {
        return None;
    }
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    Some(h.finish())
}

fn twice() -> String {
    "a field or widget is reached twice (a cycle, or kids shared)".into()
}

fn parent_of(d: &Dictionary) -> Option<ObjectId> {
    d.get(b"Parent").ok()?.as_reference().ok()
}

/// The widgets on pages, with the pages each is on, and those with a
/// wrong or null `/P`.
#[derive(Default)]
struct OnPages {
    count: usize,
    of: BTreeMap<ObjectId, Vec<usize>>,
    wrong: Vec<ObjectId>,
    null: Vec<ObjectId>,
}

fn on_pages(pdf: &Pdf) -> OnPages {
    let mut out = OnPages::default();
    for (i, page) in pdf.get_pages().into_values().enumerate() {
        out.count += 1;
        for (id, w) in widgets_on(pdf, page) {
            out.of.entry(id).or_default().push(i);
            match w.get(b"P") {
                Err(_) => {}
                Ok(Object::Reference(p)) if *p == page => {}
                Ok(Object::Reference(p)) if pdf.get_object(*p).is_ok() => out.wrong.push(id),
                Ok(_) => out.null.push(id),
            }
        }
    }
    out
}

/// The widgets in page `page`'s `/Annots`.
fn widgets_on(pdf: &Pdf, page: ObjectId) -> Vec<(ObjectId, &Dictionary)> {
    let annots = pdf
        .get_dictionary(page)
        .ok()
        .and_then(|d| d.get(b"Annots").ok())
        .and_then(|a| resolve(pdf, a))
        .and_then(|a| a.as_array().ok());
    annots
        .into_iter()
        .flatten()
        .filter_map(|a| a.as_reference().ok())
        .filter_map(|id| Some((id, pdf.get_dictionary(id).ok().filter(|d| is_widget(d))?)))
        .collect()
}

/// The widgets on page templates that aren't on pages too: on the pages
/// in the catalog's `/Names /Templates` and `/Names /Pages` name trees.
fn on_templates(pdf: &Pdf, pages: &OnPages) -> HashSet<ObjectId> {
    let names = pdf
        .catalog()
        .ok()
        .and_then(|c| c.get(b"Names").ok())
        .and_then(|n| resolve(pdf, n))
        .and_then(|n| n.as_dict().ok());
    let mut todo: Vec<&Object> = names
        .into_iter()
        .flat_map(|n| [n.get(b"Templates").ok(), n.get(b"Pages").ok()])
        .flatten()
        .collect();
    let mut seen = HashSet::new();
    let mut out = HashSet::new();
    while let Some(node) = todo.pop() {
        if let Ok(id) = node.as_reference()
            && (!seen.insert(id) || seen.len() > LIMITS.nodes)
        {
            continue;
        }
        let Some(d) = resolve(pdf, node).and_then(|n| n.as_dict().ok()) else {
            continue;
        };
        let array = |key: &[u8]| {
            let a = d.get(key).ok().and_then(|a| resolve(pdf, a));
            a.and_then(|a| a.as_array().ok()).map(Vec::as_slice)
        };
        todo.extend(array(b"Kids").unwrap_or_default());
        // Pairs of a name and a page.
        let named = array(b"Names")
            .unwrap_or_default()
            .iter()
            .skip(1)
            .step_by(2);
        for page in named.filter_map(|p| p.as_reference().ok()) {
            if seen.insert(page) {
                let widgets = widgets_on(pdf, page).into_iter().map(|(id, _)| id);
                out.extend(widgets.filter(|id| !pages.of.contains_key(id)));
            }
        }
    }
    out
}

fn is_widget(d: &Dictionary) -> bool {
    d.get(b"Subtype")
        .ok()
        .and_then(|s| s.as_name().ok())
        .is_some_and(|s| s == b"Widget")
}

/// Whether a widget has the Hidden flag: no viewer draws it.
fn hidden(pdf: &Pdf, id: ObjectId) -> bool {
    let flags = pdf.get_dictionary(id).ok().and_then(|w| w.get(b"F").ok());
    flags.and_then(|f| f.as_i64().ok()).unwrap_or(0) & 2 != 0
}

/// A widget's full name, from `/T` up its `/Parent` chain. Past
/// [`SHOWN_NAME`] bytes, its first parts are `…`.
fn name_up(pdf: &Pdf, id: ObjectId) -> String {
    let mut parts = Vec::new();
    let mut len = 0;
    let mut seen = HashSet::new();
    let mut at = Some(id);
    while let Some(id) = at.filter(|&id| seen.insert(id) && seen.len() <= LIMITS.depth) {
        let Ok(d) = pdf.get_dictionary(id) else { break };
        if let Some(t) = d.get(b"T").ok().and_then(|t| resolve(pdf, t)) {
            let part = match t.as_str() {
                Ok(s) if s.len() > SHOWN_NAME => "…".to_string(),
                _ => pdf_text(t),
            };
            len += part.len() + 1;
            parts.push(part);
            if len > SHOWN_NAME {
                parts.push("…".into());
                break;
            }
        }
        at = parent_of(d);
    }
    parts.reverse();
    parts.join(".")
}

/// A `/V`, resolved; but a reference to a dictionary (a signature) stays
/// one, so a certification's signature is known in its field.
fn resolve_once<'a>(pdf: &'a Pdf, o: &'a Object) -> Option<&'a Object> {
    match o {
        Object::Reference(id) if pdf.get_dictionary(*id).is_ok() => Some(o),
        o => resolve(pdf, o),
    }
}

/// A dictionary's `/DA`, unless it's longer than [`FormLimits::da`].
fn da_of<'a>(pdf: &'a Pdf, d: &'a Dictionary) -> Option<&'a [u8]> {
    let da = resolve(pdf, d.get(b"DA").ok()?)?.as_str().ok()?;
    (da.len() <= LIMITS.da).then_some(da)
}

/// The font a `/DA` sets: its last `Tf`'s first operand, when that's a
/// name, up to what can't be lexed.
fn da_font(da: &[u8]) -> Option<Vec<u8>> {
    let mut operands = Vec::new();
    let mut font = None;
    for token in content::lex(da, &ResCx::none(), &Limits::DEFAULT) {
        let Ok((_, token)) = token else { break };
        match token {
            Tok::Operand(operand) => operands.push(operand),
            Tok::Op(op) => {
                if op == b"Tf" {
                    font = operands.first().and_then(|&o| content::name(o));
                }
                operands.clear();
            }
            Tok::InlineImage(_) => operands.clear(),
        }
    }
    Some(font?.into_owned())
}

/// Fonts named by the `/DA` each widget uses (its own, its field's, or
/// the form's) that `/DR /Font` doesn't have, with how many widgets name
/// each, most first.
fn missing_fonts(pdf: &Pdf, form: Option<&Dictionary>, fields: &[Field]) -> Vec<(String, usize)> {
    let form_da = form.and_then(|f| da_of(pdf, f));
    let fonts = form
        .and_then(|f| f.get(b"DR").ok())
        .and_then(|d| resolve(pdf, d))
        .and_then(|d| d.as_dict().ok())
        .and_then(|d| d.get(b"Font").ok())
        .and_then(|f| resolve(pdf, f))
        .and_then(|f| f.as_dict().ok());
    let mut missing: BTreeMap<Vec<u8>, usize> = BTreeMap::new();
    for f in fields.iter().filter(|f| f.ft != b"Sig") {
        for &w in &f.widgets {
            let own = pdf.get_dictionary(w).ok().and_then(|d| da_of(pdf, d));
            let Some(font) = own.or(f.da).or(form_da).and_then(da_font) else {
                continue;
            };
            if !fonts.is_some_and(|d| d.has(&font)) {
                *missing.entry(font).or_default() += 1;
            }
        }
    }
    let mut missing: Vec<(String, usize)> = missing
        .into_iter()
        .map(|(font, n)| (String::from_utf8_lossy(&font).into_owned(), n))
        .collect();
    missing.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    missing
}

/// Each field's value hash, by full name (the first, for names more than
/// one field has).
fn values<'t>(fields: &'t [Field]) -> BTreeMap<&'t str, u64> {
    let mut out = BTreeMap::new();
    for f in fields.iter().filter(|f| f.ft != b"Sig") {
        if let Some(v) = f.value() {
            out.entry(f.name.as_str()).or_insert(v);
        }
    }
    out
}

/// Whether a field with a value shows it on the widgets a viewer draws
/// (on a page, and not hidden): each of a text or choice field's has a
/// normal appearance, and one of a check box's or radio group's is in
/// the value's state, with an appearance for it. Fields with no value,
/// or no such widgets, pass.
fn shows_value(pdf: &Pdf, f: &Field, pages: &OnPages) -> bool {
    if f.ft == b"Sig" || f.value().is_none() {
        return true;
    }
    let shown: Vec<ObjectId> = f
        .widgets
        .iter()
        .copied()
        .filter(|&w| pages.of.contains_key(&w) && !hidden(pdf, w))
        .collect();
    if shown.is_empty() {
        return true;
    }
    if f.ft == b"Btn" {
        shown.iter().any(|&w| appears(pdf, w, f.state()))
    } else {
        shown.iter().all(|&w| appears(pdf, w, None))
    }
}

/// Whether widget `id` has a normal appearance: a stream, or the state
/// `/AS` picks. With `state`, `/AS` must be that state.
fn appears(pdf: &Pdf, id: ObjectId, state: Option<&[u8]>) -> bool {
    let Ok(w) = pdf.get_dictionary(id) else {
        return false;
    };
    let Some(normal) = w
        .get(b"AP")
        .ok()
        .and_then(|ap| resolve(pdf, ap))
        .and_then(|ap| ap.as_dict().ok())
        .and_then(|ap| ap.get(b"N").ok())
        .and_then(|n| resolve(pdf, n))
    else {
        return false;
    };
    let current = w.get(b"AS").ok().and_then(|s| s.as_name().ok());
    match (normal, state) {
        (Object::Stream(_), None) => true,
        (Object::Dictionary(states), None) => current.is_some_and(|s| states.has(s)),
        (Object::Dictionary(states), Some(want)) => current == Some(want) && states.has(want),
        _ => false,
    }
}

/// Where each revision may end: after a `%%EOF` that follows `startxref
/// n`, for an `n` no `%%EOF` before it gave (so a trailer written twice
/// ends one revision), as where the `%%EOF` ends and where its line does.
/// A `%%EOF` in a stream can pass; reading the file up to it tells.
fn revision_ends(bytes: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut xrefs = HashSet::new();
    let mut from = 0;
    while let Some(at) = bytes[from..].windows(5).position(|w| w == b"%%EOF") {
        let at = from + at;
        let end = at + 5;
        from = end;
        if !startxref(&bytes[..at]).is_some_and(|n| xrefs.insert(n)) {
            continue;
        }
        let eol = match &bytes[end..] {
            [b'\r', b'\n', ..] => 2,
            [b'\r' | b'\n', ..] => 1,
            _ => 0,
        };
        out.push((end, end + eol));
    }
    out
}

/// The `n` of the `startxref n` that `bytes` ends with, before white space.
fn startxref(bytes: &[u8]) -> Option<u64> {
    let bytes = bytes.trim_ascii_end();
    let digits = bytes
        .iter()
        .rev()
        .take_while(|b| b.is_ascii_digit())
        .count();
    let (head, n) = bytes.split_at(bytes.len() - digits);
    if digits == 0 || !head.trim_ascii_end().ends_with(b"startxref") {
        return None;
    }
    std::str::from_utf8(n).ok()?.parse().ok()
}

/// Whether a signature's `/ByteRange` is `[0 a b c]` with `b + c` where a
/// revision ends, around a hex string (the `/Contents` the signature
/// leaves out). Otherwise, how many bytes it says it covers.
fn byte_range_fits(sig: &Dictionary, bytes: &[u8], ends: &HashSet<usize>) -> Result<(), u64> {
    let range: Vec<i64> = sig
        .get(b"ByteRange")
        .ok()
        .and_then(|r| r.as_array().ok())
        .map(|r| r.iter().filter_map(|n| n.as_i64().ok()).collect())
        .unwrap_or_default();
    let [start, a, b, c] = range[..] else {
        return Err(0);
    };
    let covers = b.saturating_add(c).max(0) as u64;
    let fits = start == 0
        && 0 < a
        && a < b
        && c >= 0
        && ends.contains(&(covers as usize))
        && bytes.get(a as usize) == Some(&b'<')
        && bytes.get(b as usize - 1) == Some(&b'>');
    if fits { Ok(()) } else { Err(covers) }
}

/// The form's `/XFA`: its packets' names, and its template (all of it,
/// when it's one stream).
fn xfa_of(pdf: &Pdf, form: &Dictionary) -> Option<(Vec<String>, Option<Vec<u8>>)> {
    let content = |o: &Object| {
        let s = resolve(pdf, o)?.as_stream().ok()?;
        s.decompressed_content_with_limit(LIMITS.xfa).ok()
    };
    match resolve(pdf, form.get(b"XFA").ok()?)? {
        Object::Array(items) => {
            let mut packets = Vec::new();
            let mut template = None;
            for pair in items.chunks(2) {
                let name = pair[0]
                    .as_str()
                    .map(|n| String::from_utf8_lossy(n).into_owned());
                let name = name.unwrap_or_default();
                if name == "template"
                    && let Some(o) = pair.get(1)
                {
                    template = content(o);
                }
                packets.push(name);
            }
            Some((packets, template))
        }
        o @ Object::Stream(_) => Some((vec!["xdp".into()], content(o))),
        _ => None,
    }
}

/// The SOM paths of the fields an XFA template draws, as an AcroForm
/// names them (`topmostSubform.Page1.f1_01`): containers by name, or
/// `#subform` and the like when unnamed; an exclusion group as one field;
/// nothing hidden, and no prototypes. None when it isn't XML, or its
/// paths are past the limits the field tree's names have.
fn xfa_paths(xml: &[u8]) -> Option<BTreeSet<String>> {
    fn attr(e: &BytesStart, key: &[u8]) -> Option<String> {
        e.attributes()
            .flatten()
            .find(|a| a.key.as_ref() == key)
            .map(|a| String::from_utf8_lossy(&a.value).into_owned())
    }
    fn gone(e: &BytesStart) -> bool {
        matches!(attr(e, b"presence").as_deref(), Some("hidden" | "inactive"))
    }
    const CONTAINERS: [&[u8]; 5] = [b"subform", b"area", b"subformSet", b"pageSet", b"pageArea"];
    let mut reader = quick_xml::Reader::from_reader(xml);
    let mut buf = Vec::new();
    // The open containers' path, and its length before each open element.
    let mut path = String::new();
    let mut stack: Vec<usize> = Vec::new();
    // How deep inside something left out (a prototype, a field) it is.
    let mut skip = 0usize;
    let mut in_template = 0usize;
    let mut out = BTreeSet::new();
    let mut bytes = 0;
    loop {
        let event = reader.read_event_into(&mut buf).ok()?;
        match &event {
            Event::Start(e) | Event::Empty(e) => {
                let tag = e.local_name().as_ref().to_vec();
                let empty = matches!(event, Event::Empty(_));
                if !empty {
                    stack.push(path.len());
                }
                if skip > 0 || in_template == 0 {
                    if !empty {
                        if skip > 0 {
                            skip += 1;
                        } else if tag == b"template" {
                            in_template = stack.len();
                        }
                    }
                    buf.clear();
                    continue;
                }
                // Unnamed, an element is `#` and its kind.
                let name = attr(e, b"name")
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| format!("#{}", String::from_utf8_lossy(&tag)));
                match tag.as_slice() {
                    b"field" | b"exclGroup" => {
                        if !gone(e) {
                            let len = path.len() + 1 + name.len();
                            bytes += len;
                            if len > LIMITS.name
                                || bytes > LIMITS.names
                                || out.len() >= LIMITS.nodes
                            {
                                return None;
                            }
                            out.insert(if path.is_empty() {
                                name
                            } else {
                                format!("{path}.{name}")
                            });
                        }
                        skip = usize::from(!empty);
                    }
                    b"proto" => skip = usize::from(!empty),
                    t if CONTAINERS.contains(&t) => {
                        if gone(e) {
                            skip = usize::from(!empty);
                        }
                        if !empty {
                            if !path.is_empty() {
                                path.push('.');
                            }
                            path.push_str(&name);
                        }
                    }
                    _ => {}
                }
            }
            Event::End(_) => {
                if stack.len() == in_template {
                    in_template = 0;
                }
                if let Some(len) = stack.pop() {
                    path.truncate(len);
                }
                skip = skip.saturating_sub(1);
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Some(out)
}

/// An AcroForm full name as a SOM path: no `[n]` indexes, and `\.` (a dot
/// within a name) as a dot.
fn som_path(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut depth = 0usize;
    for c in name.chars() {
        match c {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            '\\' => {}
            c if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "{}: {}, {}, {}",
            self.class(),
            count(self.fields, "field"),
            count(self.widgets, "widget"),
            count(self.pages, "page")
        )?;
        if self.template_widgets > 0 {
            let n = count(self.template_widgets, "widget");
            writeln!(f, "{n} on page templates")?;
        }
        if let Some(x) = &self.xfa {
            let kind = if x.dynamic { "dynamic, " } else { "" };
            let packets: Vec<String> = x
                .packets
                .iter()
                .map(|p| p.escape_debug().to_string())
                .collect();
            writeln!(f, "XFA: {kind}{}", packets.join(" "))?;
        }
        if self.need_appearances {
            writeln!(f, "NeedAppearances is set")?;
        }
        if self.usage_rights {
            writeln!(f, "Reader usage rights (/Perms /UR3)")?;
        }
        if self.certified {
            writeln!(f, "Certified (/Perms /DocMDP)")?;
        }
        if self.signature_fields > 0 {
            writeln!(
                f,
                "{}, {} signed",
                count(self.signature_fields, "signature field"),
                self.signed
            )?;
        }
        if self.sig_flags != 0 {
            writeln!(f, "SigFlags {}", self.sig_flags)?;
        }
        if self.revisions > 1 {
            writeln!(f, "{}", count(self.revisions, "revision"))?;
        }
        if self.findings.is_empty() {
            return writeln!(f, "No problems found.");
        }
        writeln!(f, "Problems:")?;
        for finding in &self.findings {
            writeln!(f, "- {finding}")?;
        }
        Ok(())
    }
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let widgets = |names: &[String], what: &str| {
            format!("{} {what}{}", count(names.len(), "widget"), examples(names))
        };
        match self {
            Finding::Orphans(names) => {
                write!(f, "{}", widgets(names, "no field in /Fields reaches"))
            }
            Finding::OffPage(names) => write!(f, "{}", widgets(names, "in the file on no page")),
            Finding::SharedWidgets(names) => {
                write!(f, "{}", widgets(names, "on more than one page"))
            }
            Finding::WrongPage(names) => {
                write!(f, "{}", widgets(names, "whose /P is another page"))
            }
            Finding::NullPage(names) => {
                write!(
                    f,
                    "{}",
                    widgets(names, "whose /P is null or not in the file")
                )
            }
            Finding::WrongParent(names) => write!(
                f,
                "{} listing kids whose /Parent is elsewhere{}",
                count(names.len(), "field"),
                examples(names)
            ),
            Finding::MissingFonts(fonts) => {
                let list: Vec<String> = fonts
                    .iter()
                    .map(|(n, k)| format!("{} ×{k}", n.escape_debug()))
                    .collect();
                write!(f, "fonts /DA names that /DR lacks: {}", list.join(", "))
            }
            Finding::DuplicateNames(names) => write!(
                f,
                "{} held by more than one field{}",
                count(names.len(), "full name"),
                examples(names)
            ),
            Finding::NoAppearance(names) => write!(
                f,
                "{} with a value and no appearance for it{}",
                count(names.len(), "field"),
                examples(names)
            ),
            Finding::EarlierValues { revision, names } => write!(
                f,
                "revision {revision} holds {} the current one doesn't{}",
                count(names.len(), "value"),
                examples(names)
            ),
            Finding::XfaMismatch {
                xfa_only,
                acroform_only,
            } => {
                let both: Vec<String> = xfa_only.iter().chain(acroform_only).cloned().collect();
                write!(
                    f,
                    "the XFA copy names {} the AcroForm lacks, and lacks {} it has{}",
                    count(xfa_only.len(), "field"),
                    acroform_only.len(),
                    examples(&both)
                )
            }
            Finding::StaleSignature { what, covers, size } => write!(
                f,
                "{what} signs {covers} bytes, which isn't where a revision of this \
                 {size}-byte file ends"
            ),
            Finding::StaleSigFlags { flags, why } => write!(f, "SigFlags {flags} with {why}"),
            Finding::Damaged(reason) => write!(f, "damaged: {reason}"),
        }
    }
}

fn count(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

/// ` (e.g. "a", "b", "c")`, from the first few of `names`, each cut to
/// [`EXAMPLE_CHARS`] characters.
fn examples(names: &[String]) -> String {
    if names.is_empty() {
        return String::new();
    }
    let shown: Vec<String> = names
        .iter()
        .take(EXAMPLES)
        .map(|n| match n.char_indices().nth(EXAMPLE_CHARS) {
            Some((cut, _)) => format!("{:?}…", &n[..cut]),
            None => format!("{n:?}"),
        })
        .collect();
    format!(" (e.g. {})", shown.join(", "))
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::forms::corpus::{Corpus, each_entry};

    /// The font a `/DA` sets, as `da_font` read it through lopdf's
    /// `Content::decode` before it lexed: what it must still find.
    fn decoded_font(da: &[u8]) -> Option<Vec<u8>> {
        let content = lopdf::content::Content::decode(da).ok()?;
        let tf = content
            .operations
            .iter()
            .rev()
            .find(|op| op.operator == "Tf")?;
        Some(tf.operands.first()?.as_name().ok()?.to_vec())
    }

    /// Every `/DA` in `pdf` the check would read: a widget's, a field's
    /// and the form's among them.
    fn every_da(pdf: &Pdf) -> Vec<&[u8]> {
        let mut out = Vec::new();
        let mut todo: Vec<&Object> = pdf.objects.values().collect();
        while let Some(object) = todo.pop() {
            let dict = match object {
                Object::Dictionary(d) => d,
                Object::Stream(s) => &s.dict,
                Object::Array(items) => {
                    todo.extend(items);
                    continue;
                }
                _ => continue,
            };
            out.extend(da_of(pdf, dict));
            todo.extend(dict.iter().map(|(_, v)| v));
        }
        out
    }

    #[test]
    fn da_fonts_are_the_last_tf_names() {
        for (da, font) in [
            (&b"/Helv 12 Tf 0 g"[..], Some(&b"Helv"[..])),
            (b"0 g /F1 9 Tf /F#202 10 Tf", Some(b"F 2")),
            (b"/F1 0 Tf 12 Tf", None),
            (b"0 0 1 rg", None),
            (b"/Helv 12 Tf (open", Some(b"Helv")),
        ] {
            assert_eq!(
                da_font(da).as_deref(),
                font,
                "{}",
                String::from_utf8_lossy(da)
            );
            assert_eq!(da_font(da), decoded_font(da));
        }
    }

    /// The lexer finds the font lopdf's `Content::decode` found in every
    /// `/DA` the corpus's forms hold.
    #[test]
    fn corpus_da_fonts_are_as_decoded() {
        let Some(corpus) = Corpus::or_skip() else {
            return;
        };
        let entries = corpus.manifest().unwrap();
        let started = Instant::now();
        let outcomes = each_entry(&entries, |entry| {
            let Ok(bytes) = std::fs::read(corpus.path(entry)) else {
                return (0, Vec::new());
            };
            let Ok(pdf) = crate::file::load_pdf_mem(&bytes, None) else {
                return (0, Vec::new());
            };
            let das = every_da(&pdf);
            let differ: Vec<String> = das
                .iter()
                .filter(|da| da_font(da) != decoded_font(da))
                .map(|da| format!("{}: {:?}", entry.path, String::from_utf8_lossy(da)))
                .collect();
            (das.len(), differ)
        });
        let read: usize = outcomes.iter().map(|(n, _)| n).sum();
        let differ: Vec<&String> = outcomes.iter().flat_map(|(_, d)| d).collect();
        eprintln!(
            "{read} /DA strings in {} forms, {} differ, in {:?}",
            entries.len(),
            differ.len(),
            started.elapsed()
        );
        assert!(differ.is_empty(), "{differ:#?}");
    }
}
