//! The bodies of the fuzz targets in `fuzz/` (cargo-fuzz), here so a
//! stable test runs them too. Each reads what it's given, and must
//! neither panic nor hang whatever the bytes.

use std::path::Path;
use std::sync::Arc;

use lopdf::LazyPdf;

use super::Limits;
use crate::store::{BlobKind, Store};

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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::{Duration, Instant};

    use lopdf::encryption::crypt_filters::{Aes128CryptFilter, CryptFilter};
    use lopdf::xref::XrefType;
    use lopdf::{EncryptionState, EncryptionVersion, Permissions};

    use super::*;
    use crate::forms::fixture::{every_kind, hybrid_reference};
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

    /// The fixtures, as the fuzz targets' first corpus, into the directory
    /// `SHEAF_FUZZ_SEEDS` names: `fuzz.yml` seeds each night's run with
    /// them, so the repository holds no PDF.
    #[test]
    #[ignore = "fuzz.yml's seeds"]
    fn write_fuzz_seeds() {
        let dir = std::env::var_os("SHEAF_FUZZ_SEEDS").expect("SHEAF_FUZZ_SEEDS names a directory");
        let dir = Path::new(&dir);
        std::fs::create_dir_all(dir).unwrap();
        for (n, fixture) in fixtures().iter().enumerate() {
            std::fs::write(dir.join(format!("fixture-{n}.pdf")), fixture).unwrap();
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
        let took = started.elapsed();
        assert!(took < Duration::from_secs(60), "{took:?}");
    }
}
