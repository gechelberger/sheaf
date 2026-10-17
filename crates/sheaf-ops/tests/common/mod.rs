//! Helpers the forms tests share: fixtures, saving both ways, and what a
//! saved file holds.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lopdf::{Document as Pdf, Object, ObjectId, Stream, dictionary};
use sheaf_doc::Document;
use sheaf_doc::forms::corpus::Corpus;
use sheaf_doc::forms::fixture::{Form, Rect, source};
use sheaf_doc::forms::{FileForm, Finding, Report, Value, check};
use sheaf_doc::save::{Strategy, save_to_path};
use sheaf_image::raster::Raster;

/// Half of a row's inside, in pixels.
pub const HALF_A_ROW: usize = 196 * 26 / 2;

pub fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sheaf-forms-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // SAFETY: each test sets the same value before any reads it.
    unsafe {
        std::env::set_var(
            "SHEAF_CACHE_DIR",
            std::env::temp_dir().join(format!("sheaf-forms-cache-{}", std::process::id())),
        )
    };
    dir
}

/// `path` opened as the app opens it.
pub fn open(path: &Path) -> Document {
    sheaf_doc::open::file(path, None).unwrap().doc
}

pub fn write(dir: &Path, name: &str, form: &mut Form) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, form.bytes()).unwrap();
    path
}

/// Row `i` of a page, 30 pt high, from the top.
pub fn row(i: usize) -> Rect {
    let top = 740.0 - 50.0 * i as f32;
    [100.0, top - 30.0, 300.0, top]
}

/// A text field filled with `value`, whose appearance is a solid blue box.
pub fn blue(form: &mut Form, name: &str, page: usize, i: usize, value: &str) -> ObjectId {
    let id = form.text(name, page, row(i));
    form.dict(id).set("V", Object::string_literal(value));
    let ap = form.pdf.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Form",
            "BBox" => vec![0.into(), 0.into(), 200.into(), 30.into()],
        },
        format!("0 0 1 rg 0 0 200 30 re f BT /Helv 10 Tf 2 10 Td ({value}) Tj ET").into_bytes(),
    ));
    form.dict(id).set("AP", dictionary! { "N" => ap });
    id
}

/// Blue pixels inside row `i` of a 612-pixel-wide render of a page whose
/// shown area's top is at `top` in user space.
pub fn blue_in(r: &Raster, i: usize, top: f32) -> usize {
    let [x0, y0, x1, y1] = row(i).map(|v| v as usize);
    let top = top as usize;
    let mut n = 0;
    for y in (top - y1 + 2)..(top - y0 - 2) {
        for x in (x0 + 2)..(x1 - 2) {
            let p = &r.data[(y * r.width as usize + x) * 4..][..3];
            if p[2] > 200 && p[0] < 60 && p[1] < 60 {
                n += 1;
            }
        }
    }
    n
}

/// Whether `needle` is anywhere in the file: in its bytes, in a string
/// (decoded from hex), or in a stream once decoded.
pub fn holds(bytes: &[u8], needle: &[u8]) -> bool {
    let found = |hay: &[u8]| hay.windows(needle.len()).any(|w| w == needle);
    if found(bytes) {
        return true;
    }
    // Strings the file writes in hex, and streams, decoded.
    fn any(o: &Object, found: &dyn Fn(&[u8]) -> bool) -> bool {
        match o {
            Object::String(s, _) => found(s),
            Object::Array(a) => a.iter().any(|o| any(o, found)),
            Object::Dictionary(d) => d.iter().any(|(_, o)| any(o, found)),
            Object::Stream(s) => {
                s.dict.iter().any(|(_, o)| any(o, found))
                    || found(
                        &s.decompressed_content()
                            .unwrap_or_else(|_| s.content.clone()),
                    )
            }
            _ => false,
        }
    }
    let pdf = Pdf::load_mem(bytes).unwrap();
    pdf.objects.values().any(|o| any(o, &found))
}

/// The form in `bytes`, read as a file's is when it opens.
pub fn form_in(bytes: &[u8]) -> FileForm {
    sheaf_doc::forms::read(&source(bytes.to_vec()))
}

/// The text fields' values, by full name.
pub fn values(bytes: &[u8]) -> BTreeMap<String, String> {
    form_in(bytes)
        .fields
        .into_iter()
        .filter_map(|f| match f.value {
            Some(Value::Text(t)) => Some((f.name, t.to_string())),
            _ => None,
        })
        .collect()
}

/// The form check on `bytes`, which must be clean.
pub fn clean(bytes: &[u8], what: &str) -> Report {
    let report = check(bytes, None).unwrap();
    assert!(report.is_clean(), "{what}: {:?}", report.findings);
    report
}

/// Save `doc` by rewrite to `copy`, then in place: the bytes each gave.
/// A page that's gone makes the second a rewrite too (M9 bug 14).
pub fn saved_both_ways(doc: &Document, path: &Path, copy: &Path) -> Vec<(Strategy, Vec<u8>)> {
    sheaf_doc::save::rewrite_to_path(&doc.snapshot(), copy).unwrap();
    let rewritten = std::fs::read(copy).unwrap();
    let strategy = save_to_path(&doc.snapshot(), path).unwrap();
    vec![
        (Strategy::Rewrite, rewritten),
        (strategy, std::fs::read(path).unwrap()),
    ]
}

/// What each strategy writes for `doc`. A page shown from a copy counts
/// as removed content, so saving in place rewrites (M9 bug 14), but the
/// incremental writer still writes it: for crash recovery's sidecar. So
/// does a changed form with an XFA copy or usage rights.
pub fn written_both_ways(doc: &Document) -> Vec<(Strategy, Vec<u8>)> {
    let snap = doc.snapshot();
    let incremental = sheaf_doc::incremental::incremental(&snap).unwrap_or_else(|e| {
        // A changed form with an XFA copy or usage rights is saved in full
        // (M10 slice 5); a recovery sidecar still appends, taking them out.
        assert!(snap.form_changed, "{e:#}");
        let (len, section) = sheaf_doc::incremental::incremental_section(&snap).unwrap();
        let info = snap.source(snap.primary).unwrap();
        let mut bytes = info.src.bytes()[..len].to_vec();
        bytes.extend(section);
        bytes
    });
    vec![
        (Strategy::Rewrite, sheaf_doc::save::rewrite(&snap).unwrap()),
        (Strategy::Incremental, incremental),
    ]
}

/// Object references in the file's structure that lead nowhere: an
/// `/OBJR` whose `/Obj` is null or an object the file doesn't have. None
/// may, once a page is gone: what was there leaves the tree with it.
pub fn dangling_structure(bytes: &[u8]) -> usize {
    let pdf = Pdf::load_mem(bytes).unwrap();
    let objr = |o: &Object| {
        let o = match o {
            Object::Reference(id) => pdf.get_object(*id).ok(),
            o => Some(o),
        };
        o.and_then(|o| o.as_dict().ok())
            .filter(|d| d.get(b"Type").and_then(|t| t.as_name()).ok() == Some(b"OBJR"))
            .is_some_and(|d| match d.get(b"Obj") {
                Ok(Object::Reference(id)) => pdf.get_object(*id).is_err(),
                _ => true,
            })
    };
    pdf.objects
        .values()
        .filter_map(|o| o.as_dict().ok())
        .filter(|d| d.get(b"Type").and_then(|t| t.as_name()).ok() == Some(b"StructElem"))
        .map(|d| match d.get(b"K") {
            Ok(Object::Array(kids)) => kids.iter().filter(|k| objr(k)).count(),
            Ok(k) => usize::from(objr(k)),
            Err(_) => 0,
        })
        .sum()
}

/// The catalog's `/AcroForm /Fields`, or None with no `/AcroForm`.
pub fn fields_of(bytes: &[u8]) -> Option<Vec<Object>> {
    let pdf = Pdf::load_mem(bytes).unwrap();
    let form = pdf.catalog().unwrap().get(b"AcroForm").ok()?;
    let form = pdf.dereference(form).unwrap().1.as_dict().unwrap();
    let fields = pdf.dereference(form.get(b"Fields").unwrap()).unwrap().1;
    Some(fields.as_array().unwrap().clone())
}

/// A corpus form, or None (and the corpus tests skip) without one.
pub fn corpus_form(path: &str) -> Option<Vec<u8>> {
    let corpus = Corpus::or_skip()?;
    let entry = corpus.entry(path).unwrap().expect("in the manifest");
    Some(std::fs::read(corpus.path(&entry)).unwrap())
}

/// `bytes` with field `name`'s `/V` set to `value` (its appearance as it
/// was), as a fill sheaf can't make yet (slice 9) would leave it.
pub fn filled(bytes: &[u8], name: &str, value: &str) -> Vec<u8> {
    let field = form_in(bytes)
        .fields
        .into_iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("no field {name}"));
    // In an update, as Reader saves a fill: the original's bytes, and the
    // usage rights' signature over them, stay sound.
    sheaf_doc::forms::fixture::append_revision(bytes, &[field.object], |pdf| {
        pdf.get_dictionary_mut(field.object)
            .unwrap()
            .set("V", Object::string_literal(value));
    })
}

/// What the form check finds in `bytes`.
pub fn findings(bytes: &[u8]) -> Vec<Finding> {
    check(bytes, None).unwrap().findings
}
