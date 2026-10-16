//! Crash recovery. Unsaved edits are kept as an incremental update section
//! in a sidecar next to the file (`<name>.pdf.sheaf-recovery`); appending
//! that section to the original yields a complete PDF with the edits
//! applied. Writing it costs only the size of the edits, so it can run a
//! few seconds after every change. The sidecar records the original's
//! length so a file changed by something else is never patched.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::model::Snapshot;

const MAGIC: &[u8] = b"sheaf-recovery 1\n";
pub const SUFFIX: &str = ".sheaf-recovery";

pub fn sidecar_path(pdf: &Path) -> PathBuf {
    let mut name = pdf
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
    name.push(SUFFIX);
    pdf.with_file_name(name)
}

/// Write the sidecar for `snap`'s unsaved state. Returns the section size,
/// or None (and no sidecar) when there's nothing to record.
pub fn write(snap: &Snapshot) -> Result<Option<usize>> {
    let info = snap
        .source(snap.primary)
        .context("primary source missing")?;
    let (original_len, section) = crate::incremental::incremental_section(snap)?;
    let sidecar = sidecar_path(&info.name);
    if section.is_empty() {
        let _ = std::fs::remove_file(&sidecar);
        return Ok(None);
    }
    let mut bytes = Vec::with_capacity(MAGIC.len() + 24 + section.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(format!("{original_len}\n").as_bytes());
    bytes.extend_from_slice(&section);
    crate::save::write_atomic(&sidecar, &bytes)?;
    Ok(Some(section.len()))
}

/// Recovery data found next to a file.
#[derive(Debug, Clone, PartialEq)]
pub struct Recovery {
    pub sidecar: PathBuf,
    pub original_len: usize,
    pub section: Vec<u8>,
}

/// A usable sidecar for `pdf`: present, well-formed, and written against a
/// file of the current size. A stale or malformed one is removed.
pub fn find(pdf: &Path) -> Option<Recovery> {
    let sidecar = sidecar_path(pdf);
    let bytes = std::fs::read(&sidecar).ok()?;
    let current_len = std::fs::metadata(pdf).map(|m| m.len() as usize).ok()?;
    match parse(&bytes) {
        Some((original_len, section)) if original_len == current_len && !section.is_empty() => {
            Some(Recovery {
                sidecar,
                original_len,
                section: section.to_vec(),
            })
        }
        _ => {
            let _ = std::fs::remove_file(&sidecar);
            None
        }
    }
}

fn parse(bytes: &[u8]) -> Option<(usize, &[u8])> {
    let rest = bytes.strip_prefix(MAGIC)?;
    let nl = rest.iter().position(|&b| b == b'\n')?;
    let len: usize = std::str::from_utf8(&rest[..nl]).ok()?.parse().ok()?;
    Some((len, &rest[nl + 1..]))
}

/// Write `pdf` plus the recovered section as a complete PDF at `out`.
pub fn materialize(pdf: &Path, rec: &Recovery, out: &Path) -> Result<()> {
    let mut bytes = std::fs::read(pdf).with_context(|| format!("reading {}", pdf.display()))?;
    if bytes.len() != rec.original_len {
        bail!(
            "{} changed since the recovery data was written",
            pdf.display()
        );
    }
    bytes.extend_from_slice(&rec.section);
    crate::save::write_atomic(out, &bytes)
}

/// Remove the sidecar, if any.
pub fn discard(pdf: &Path) {
    let _ = std::fs::remove_file(sidecar_path(pdf));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::FixedCount;
    use crate::model::{Document, Edit, SourceInfo};
    use crate::store::Store;
    use lopdf::{Document as Pdf, Object, Stream, dictionary};
    use sheaf_core::{PageSize, Rotation};

    fn two_page_pdf(path: &Path) {
        let mut pdf = Pdf::with_version("1.7");
        let pages_id = pdf.new_object_id();
        let mut kids = Vec::new();
        for _ in 0..2 {
            let content = pdf.add_object(Stream::new(dictionary! {}, b"".to_vec()));
            let page = pdf.add_object(dictionary! {
                "Type" => "Page", "Parent" => pages_id,
                "MediaBox" => Object::Array(vec![0.into(), 0.into(), 612.into(), 792.into()]),
                "Contents" => content,
            });
            kids.push(Object::Reference(page));
        }
        pdf.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => 2 }),
        );
        let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        pdf.trailer.set("Root", catalog);
        pdf.save(path).unwrap();
    }

    #[test]
    fn sidecar_round_trips_and_detects_staleness() {
        let dir = std::env::temp_dir().join(format!("sheaf-recovery-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pdf = dir.join("doc.pdf");
        two_page_pdf(&pdf);
        let bytes = std::fs::metadata(&pdf).unwrap().len() as usize;
        let store = Store::new();
        let primary = SourceInfo::read(&store, &pdf, None).unwrap();
        let mut model = Document::new(store, primary, &[], Box::new(FixedCount(10)));
        // Nothing unsaved: no sidecar.
        assert_eq!(write(&model.snapshot()).unwrap(), None);
        assert!(find(&pdf).is_none());

        let p1 = model.page(1).unwrap().id;
        model
            .apply(
                Edit::RotatePages {
                    ids: vec![p1],
                    by: Rotation::R90,
                },
                "rot",
            )
            .unwrap();
        let n = write(&model.snapshot()).unwrap().unwrap();
        assert!(n > 0);
        assert!(sidecar_path(&pdf).exists());
        // The original is untouched.
        assert_eq!(std::fs::metadata(&pdf).unwrap().len() as usize, bytes);

        let rec = find(&pdf).expect("sidecar found");
        assert_eq!(rec.original_len, bytes);
        let out = dir.join("recovered.pdf");
        materialize(&pdf, &rec, &out).unwrap();
        let recovered = Pdf::load(&out).unwrap();
        let pages = recovered.get_pages();
        let p2 = recovered.get_dictionary(pages[&2]).unwrap();
        assert_eq!(p2.get(b"Rotate").unwrap().as_i64().unwrap(), 90);
        assert!(!recovered.get_dictionary(pages[&1]).unwrap().has(b"Rotate"));

        // The file changed underneath: the sidecar is stale and goes away.
        std::fs::write(
            &pdf,
            [std::fs::read(&pdf).unwrap(), b"\n%extra\n".to_vec()].concat(),
        )
        .unwrap();
        assert!(find(&pdf).is_none());
        assert!(!sidecar_path(&pdf).exists());

        discard(&pdf); // no-op without a sidecar
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sidecar_carries_ocr_layers() {
        use crate::annot::Box2;
        use crate::text_layer::{OcrWord, TextLayer};
        let dir = std::env::temp_dir().join(format!("sheaf-recovery-ocr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pdf = dir.join("doc.pdf");
        two_page_pdf(&pdf);
        let store = Store::new();
        let primary = SourceInfo::read(&store, &pdf, None).unwrap();
        let mut model = Document::new(store, primary, &[], Box::new(FixedCount(10)));
        let p1 = model.page(1).unwrap().id;
        let layer = TextLayer::from_engine(
            vec![OcrWord {
                text: "Hello".into(),
                rect: Box2 {
                    x: 72.0,
                    y: 100.0,
                    w: 100.0,
                    h: 12.0,
                },
                confidence: 1.0,
            }],
            PageSize {
                width: 612.0,
                height: 792.0,
            },
            Rotation::R0,
            "test",
        );
        model
            .apply(
                Edit::SetTextLayers {
                    layers: vec![(p1, std::sync::Arc::new(layer))],
                },
                "ocr",
            )
            .unwrap();
        assert!(write(&model.snapshot()).unwrap().is_some());
        let rec = find(&pdf).expect("sidecar found");
        let out = dir.join("recovered.pdf");
        materialize(&pdf, &rec, &out).unwrap();
        let recovered = Pdf::load(&out).unwrap();
        let pages = recovered.get_pages();
        let p2 = recovered.get_dictionary(pages[&2]).unwrap();
        let contents = recovered
            .dereference(p2.get(b"Contents").unwrap())
            .unwrap()
            .1
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(contents.len(), 3);
        let text = String::from_utf8_lossy(&recovered.get_page_content(pages[&2])).into_owned();
        assert!(
            text.contains("3 Tr") && text.contains("<00480065006C006C006F>"),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
