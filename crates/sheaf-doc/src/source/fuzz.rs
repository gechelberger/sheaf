//! The bodies of the fuzz targets in `fuzz/` (cargo-fuzz), here so a
//! stable test runs them too. Each reads what it's given, and must
//! neither panic nor hang whatever the bytes.

use std::path::Path;
use std::sync::Arc;

use lopdf::LazyPdf;

use super::Limits;
use crate::store::{BlobKind, Src, Store};

/// `data` read lazily: every object the cross-reference table lists, and
/// where each is in the bytes.
pub fn lazy_reader(data: &[u8]) {
    let Ok(pdf) = LazyPdf::open(Arc::<[u8]>::from(data), None, &Limits::DEFAULT.reader) else {
        return;
    };
    let _ = pdf.catalog();
    for id in pdf.ids() {
        let _ = pdf.get(id);
        let _ = pdf.raw_span(id);
    }
}

/// `data` interned and opened as a source: its page table, every page's
/// size, and its conformance claims.
pub fn page_tree(data: &[u8]) {
    let kind = BlobKind::file(Path::new("fuzzed.pdf"), None);
    let Ok(src) = Store::new().open(data.into(), kind) else {
        return;
    };
    for i in 0..src.page_count() {
        let _ = src.page_size(i);
    }
    let _ = src.conformance();
}

/// `data` as the objects of a page's annotations, `n 0 obj … endobj` from
/// 4 up, read as the open reads sheaf's marks: each entry's `/SheafAnnot`
/// and digest, or its geometry from the standard keys, and whether it's
/// ours to rewrite. The page, turned 90°, lists an annotation for each
/// `endobj`, in a file with no cross-reference table, which the reader
/// rebuilds by scanning.
pub fn sheaf_annot(data: &[u8]) {
    let Some(src) = annotated_page(data) else {
        return;
    };
    let (pdf, Some(page)) = (src.pdf(), src.page(0)) else {
        return;
    };
    let annots = crate::file::PageAnnots::new(&page.annots);
    for entry in page.annots.iter() {
        let _ = crate::file::sheaf_mark(pdf, entry, page.geom.rotate, &annots);
        let _ = crate::file::is_ours(pdf, entry, &annots);
        if let Ok((_, lopdf::Object::Dictionary(d))) = pdf.dereference(entry) {
            let _ = crate::annot::staleness(pdf, d, false);
        }
    }
}

/// [`sheaf_annot`]'s file, opened as a source.
fn annotated_page(data: &[u8]) -> Option<Src> {
    let n = data.windows(6).filter(|w| w == b"endobj").count();
    let annots: String = (0..n).map(|i| format!("{} 0 R ", i + 4)).collect();
    let mut file = format!(
        "%PDF-1.7\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
         2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n\
         3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [-37 52 575 844] /Rotate 90 \
         /Annots [{annots}] >>\nendobj\n"
    )
    .into_bytes();
    file.extend_from_slice(data);
    file.extend_from_slice(b"\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n");
    let kind = BlobKind::file(Path::new("fuzzed.pdf"), None);
    Store::new().open(file.into(), kind).ok()
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};
    use std::time::{Duration, Instant};

    use lopdf::encryption::crypt_filters::{Aes128CryptFilter, CryptFilter};
    use lopdf::xref::XrefType;
    use lopdf::{EncryptionState, EncryptionVersion, Permissions};

    use lopdf::{Document as Pdf, Object, ObjectId};

    use super::*;
    use crate::forms::fixture::{Form, every_kind, every_mark_kind, hybrid_reference};
    use crate::source::lazy_tests::{encrypted, plaintext, stream, with_table};

    /// Mutations of each fixture.
    const CASES: usize = 600;

    /// What a mutation may write: the tokens files are made of, and
    /// numbers that are offsets, lengths and counts.
    const TOKENS: &[&[u8]] = &[
        b" 0 R ",
        b" 0 obj ",
        b"endobj",
        b"<<",
        b">>",
        b"[",
        b"]",
        b"(",
        b")",
        b"/Kids [2 0 R] ",
        b"/Parent 3 0 R ",
        b"/Type /Pages ",
        b"/Count 1048576 ",
        b"/MediaBox [0 0 1e30 -7] ",
        b"/Rotate -2147483648 ",
        b"/Length 4 0 R ",
        b"/Type /ObjStm /N 9 /First 0 ",
        b"/Filter /FlateDecode ",
        b"stream\n",
        b"endstream",
        b"xref\n0 3\n",
        b"trailer",
        b"startxref\n0\n",
        b"%%EOF",
        b"4294967296",
        b"-1",
    ];

    fn mutate(rng: &mut fastrand::Rng, data: &mut Vec<u8>) {
        if data.is_empty() {
            data.extend_from_slice(TOKENS[rng.usize(..TOKENS.len())]);
            return;
        }
        let at = rng.usize(..data.len());
        match rng.u8(..5) {
            0 => data[at] = rng.u8(..),
            1 => {
                let end = (at + rng.usize(1..=64)).min(data.len());
                data.drain(at..end);
            }
            2 => {
                let token = TOKENS[rng.usize(..TOKENS.len())];
                data.splice(at..at, token.iter().copied());
            }
            3 => {
                let end = (at + rng.usize(1..=256)).min(data.len());
                let copy = data[at..end].to_vec();
                let to = rng.usize(..=data.len());
                data.splice(to..to, copy);
            }
            _ => {
                // A number written over a number.
                let end = at + data[at..].iter().take_while(|c| c.is_ascii_digit()).count();
                let number = rng.u64(..).to_string();
                let number = &number[..rng.usize(1..=number.len())];
                data.splice(at..end, number.bytes());
            }
        }
    }

    const XMP: &[u8] = b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" \
        xmlns:a=\"http://www.aiim.org/pdfa/ns/id/\" a:part=\"1\"/>";

    fn fixtures() -> Vec<Vec<u8>> {
        let tree = with_table(&[
            (
                1,
                b"<< /Type /Catalog /Pages 2 0 R /Metadata 7 0 R >>".to_vec(),
            ),
            (
                2,
                b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 3 /MediaBox [0 0 600 800] >>".to_vec(),
            ),
            (
                3,
                b"<< /Type /Pages /Parent 2 0 R /Kids [5 0 R 5 0 R] /Rotate 90 >>".to_vec(),
            ),
            (
                4,
                b"<< /Type /Page /Parent 2 0 R /CropBox [10 10 500 700] /Annots [6 0 R] >>"
                    .to_vec(),
            ),
            (
                5,
                b"<< /Type /Page /Parent 3 0 R /Contents 6 0 R >>".to_vec(),
            ),
            (6, stream("/Length 8", b"0 0 m S\n")),
            (
                7,
                stream(&format!("/Type /Metadata /Length {}", XMP.len()), XMP),
            ),
        ]);
        let mut modern = every_kind();
        let mut streams = Vec::new();
        modern.pdf.save_modern(&mut streams).unwrap();
        let mut classic = every_kind();
        classic.pdf.reference_table.cross_reference_type = XrefType::CrossReferenceTable;
        let std_cf = BTreeMap::from([(
            b"StdCF".to_vec(),
            std::sync::Arc::new(Aes128CryptFilter) as std::sync::Arc<dyn CryptFilter>,
        )]);
        let plain = plaintext();
        // The empty user password: the bodies open it, and decrypt.
        let aes = EncryptionState::try_from(EncryptionVersion::V4 {
            document: &plain,
            encrypt_metadata: true,
            crypt_filters: std_cf,
            stream_filter: b"StdCF".to_vec(),
            string_filter: b"StdCF".to_vec(),
            owner_password: "owner",
            user_password: "",
            permissions: Permissions::all(),
        })
        .unwrap();
        let [aes_table, aes_stream] = encrypted(&plain, &aes, |_| {});
        vec![
            tree,
            classic.bytes(),
            streams,
            hybrid_reference(),
            aes_table,
            aes_stream,
        ]
    }

    /// [`sheaf_annot`]'s fixtures: every kind of mark as a save writes it,
    /// and the same with version 1's payloads, which have no version or
    /// digest. Each is the annotations' objects as lopdf writes them,
    /// numbered from 4 in `/Annots` order, without the appearances and
    /// files they refer to.
    fn mark_fixtures() -> Vec<Vec<u8>> {
        let mut f = Form::new(1);
        let mut page = f.dict(f.pages[0]).clone();
        let marks = every_mark_kind();
        crate::save::write_page_annots(
            &mut f.pdf,
            &mut page,
            marks.iter(),
            None,
            &[],
            &mut Default::default(),
        );
        let entries: Vec<ObjectId> = page
            .get(b"Annots")
            .and_then(Object::as_array)
            .unwrap()
            .iter()
            .map(|e| e.as_reference().unwrap())
            .collect();
        let number: HashMap<ObjectId, ObjectId> =
            (4..).zip(&entries).map(|(n, id)| (*id, (n, 0))).collect();
        [false, true]
            .map(|v1| {
                let mut out = Pdf::with_version("1.7");
                out.reference_table.cross_reference_type = XrefType::CrossReferenceTable;
                for id in &entries {
                    let mut d = f.pdf.get_dictionary(*id).unwrap().clone();
                    for key in [b"AP".as_slice(), b"FS", b"P"] {
                        d.remove(key);
                    }
                    if let Ok(irt) = d.get(b"IRT").and_then(Object::as_reference) {
                        d.set("IRT", number[&irt]);
                    }
                    if v1 && let Ok(json) = d.get(b"SheafAnnot").and_then(Object::as_str) {
                        let mut json: serde_json::Value =
                            serde_json::from_str(&crate::file::text_string(json)).unwrap();
                        let fields = json.as_object_mut().unwrap();
                        fields.remove("v");
                        fields.remove("std");
                        d.set("SheafAnnot", crate::save::text_string(&json.to_string()));
                    }
                    out.objects.insert(number[id], d.into());
                }
                let mut bytes = Vec::new();
                out.save_to(&mut bytes).unwrap();
                let at =
                    |token: &[u8]| bytes.windows(token.len()).position(|w| w == token).unwrap();
                bytes[at(b"4 0 obj")..at(b"xref")].to_vec()
            })
            .to_vec()
    }

    /// The fixtures, as the fuzz targets' first corpus, into the directory
    /// `SHEAF_FUZZ_SEEDS` names: `fuzz.yml` seeds each night's run with
    /// them, so the repository holds no PDF. The target
    /// `SHEAF_FUZZ_TARGET` names takes the marks' fixtures when it's
    /// `sheaf_annot`, and the files otherwise.
    #[test]
    #[ignore = "fuzz.yml's seeds"]
    fn write_fuzz_seeds() {
        let dir = std::env::var_os("SHEAF_FUZZ_SEEDS").expect("SHEAF_FUZZ_SEEDS names a directory");
        let dir = Path::new(&dir);
        std::fs::create_dir_all(dir).unwrap();
        let named = |seeds: Vec<Vec<u8>>, name: fn(usize) -> String| {
            seeds
                .into_iter()
                .enumerate()
                .map(move |(n, seed)| (name(n), seed))
        };
        let seeds: Vec<(String, Vec<u8>)> = match std::env::var("SHEAF_FUZZ_TARGET").as_deref() {
            Ok("sheaf_annot") => named(mark_fixtures(), |n| format!("marks-{n}")).collect(),
            _ => named(fixtures(), |n| format!("fixture-{n}.pdf")).collect(),
        };
        for (name, seed) in seeds {
            std::fs::write(dir.join(name), seed).unwrap();
        }
    }

    /// Unmutated, the marks' fixtures read as marks, every one: version 2's
    /// by their digests, version 1's from the standard keys, a
    /// replacement's struck text from its StrikeOut.
    #[test]
    fn the_mark_fixtures_read_as_marks() {
        for fixture in mark_fixtures() {
            let src = annotated_page(&fixture).unwrap();
            let page = src.page(0).unwrap();
            let annots = crate::file::PageAnnots::new(&page.annots);
            let read = page
                .annots
                .iter()
                .filter_map(|e| crate::file::sheaf_mark(src.pdf(), e, page.geom.rotate, &annots));
            assert_eq!(read.count(), every_mark_kind().len());
        }
    }

    #[test]
    fn fuzz_bodies_survive_mutated_fixtures() {
        let fixtures = fixtures();
        let started = Instant::now();
        let mut rng = fastrand::Rng::with_seed(0x5eaf_f022);
        for (n, fixture) in fixtures.iter().enumerate() {
            // Unmutated, each opens: the mutations start from a file.
            let kind = BlobKind::file(Path::new("fixture.pdf"), None);
            let src = Store::new().open(fixture.as_slice().into(), kind).unwrap();
            assert!(src.page_count() > 0);
            for case in 0..CASES {
                let mut data = fixture.clone();
                for _ in 0..rng.usize(1..=4) {
                    mutate(&mut rng, &mut data);
                }
                let ran = std::panic::catch_unwind(|| {
                    lazy_reader(&data);
                    page_tree(&data);
                });
                // The seed alone can't replay a case of the AES fixtures,
                // whose IVs are drawn afresh each run: the input is kept.
                if let Err(panic) = ran {
                    let kept = std::env::temp_dir().join(format!("sheaf-fuzz-{n}-{case}.pdf"));
                    std::fs::write(&kept, &data).unwrap();
                    eprintln!("fixture {n}, case {case}: the input is {}", kept.display());
                    std::panic::resume_unwind(panic);
                }
            }
        }
        for (n, fixture) in mark_fixtures().iter().enumerate() {
            for case in 0..CASES {
                let mut data = fixture.clone();
                for _ in 0..rng.usize(1..=4) {
                    mutate(&mut rng, &mut data);
                }
                let ran = std::panic::catch_unwind(|| sheaf_annot(&data));
                if let Err(panic) = ran {
                    let kept = std::env::temp_dir().join(format!("sheaf-fuzz-marks-{n}-{case}"));
                    std::fs::write(&kept, &data).unwrap();
                    eprintln!("marks {n}, case {case}: the input is {}", kept.display());
                    std::panic::resume_unwind(panic);
                }
            }
        }
        let took = started.elapsed();
        assert!(took < Duration::from_secs(60), "{took:?}");
    }
}
