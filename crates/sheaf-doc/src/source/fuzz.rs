//! The bodies of the fuzz targets in `fuzz/` (cargo-fuzz), here so a
//! stable test runs them too. Each reads what it's given, and must
//! neither panic nor hang whatever the bytes.

use std::borrow::Cow;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use lopdf::{LazyPdf, Object, Stream, dictionary};

use super::Limits;
use crate::content::{
    self, Concat, DrawnForms, PageBlocks, PageContent, PropertiesDict, ResCx, SpanAcross,
};
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

/// `data` as a page's content, lexed under the default caps, every token
/// taken: whole, and as two streams split at its middle. The resources
/// name a color space of each size an inline image may be drawn in.
pub fn lexer(data: &[u8]) {
    let res = ResCx::new([
        (b"CS1".to_vec(), 1),
        (b"CS3".to_vec(), 3),
        (b"CS4".to_vec(), 4),
    ]);
    let (first, second) = data.split_at(data.len() / 2);
    let split = Concat::new(&[first, second]);
    for content in [Concat::new(&[data]), split] {
        for token in content.lex(&res, &Limits::DEFAULT) {
            let Ok((range, _)) = token else { break };
            let _ = content.span(range);
        }
    }
}

/// `data` as a page's content, two streams split at its middle, drawn
/// with resources that name a property list, a color space, a form of
/// Acrobat's and one with an artifact in it: its marked ranges, names and
/// blocks, and a splice of each range at the top, of all of them at once
/// widened to touch ([`touching`]), and of its pagination artifacts as a
/// strip cuts them. Where a splice is made, what it keeps lexes as the
/// tokens it kept. Returns how many were made.
pub fn splice(data: &[u8]) -> usize {
    let lim = &Limits::DEFAULT;
    let mut pdf = lopdf::Document::with_version("1.7");
    let (first, second) = data.split_at(data.len() / 2);
    let streams = [first, second].map(|s| pdf.add_object(Stream::new(dictionary! {}, s.to_vec())));
    let piece = dictionary! { "ADBE_CompoundType" => dictionary! { "Private" => "Header" } };
    let acrobat = pdf.add_object(Stream::new(
        dictionary! { "Subtype" => "Form", "PieceInfo" => piece },
        b"BT (h) Tj ET".to_vec(),
    ));
    let nested = pdf.add_object(Stream::new(
        dictionary! { "Subtype" => "Form" },
        b"/Artifact <</Subtype /Footer>> BDC EMC".to_vec(),
    ));
    let resources = Object::Dictionary(dictionary! {
        "Properties" => dictionary! { "P0" => dictionary! {
            "Type" => "Pagination", "Subtype" => "Header", "SheafGen" => Object::string_literal("g1"),
        } },
        "XObject" => dictionary! { "X0" => acrobat, "X1" => nested },
        "ColorSpace" => dictionary! { "CS3" => "DeviceRGB" },
    });
    let Ok(content) = PageContent::read(&pdf, &streams, lim) else {
        return 0;
    };
    let concat = content.concat();
    let res = ResCx::read(&pdf, Some(&resources));
    let _ = content::balance(concat, &res, lim);
    let _ = content::names_used(concat, &res, lim, &[]);
    let props = PropertiesDict::read(&pdf, Some(&resources));
    let mut drops: Vec<Vec<SpanAcross>> = Vec::new();
    if let Ok(found) = content::marked(concat, &props, &res, lim) {
        let top: Vec<SpanAcross> = found
            .iter()
            .filter(|m| m.depth == 0)
            .map(|m| m.span)
            .collect();
        drops.extend(top.iter().map(|&span| vec![span]));
        if top.len() > 1 {
            drops.push(touching(concat, &top));
        }
    }
    let known = DrawnForms::default();
    if let Ok(blocks) = PageBlocks::of(&pdf, &content, Some(&resources), lim, &known)
        && let Ok(cuts) = blocks
            .pagination
            .iter()
            .map(|p| p.cuts(concat, &res, lim))
            .collect::<Result<Vec<_>, _>>()
        && !cuts.is_empty()
    {
        drops.push(cuts.concat());
    }
    let mut made = 0;
    for drop in drops {
        if let Ok(kept) = content::splice(concat, &res, lim, &drop) {
            assert_kept(concat, &res, &kept, &drop);
            made += 1;
        }
    }
    made
}

/// `spans`, in order and apart, each widened over the white-space either
/// side of it, white-space between two split at its middle: cuts that
/// touch, so whether one needs a space in its place depends on bytes its
/// neighbour takes.
fn touching(content: &Concat, spans: &[SpanAcross]) -> Vec<SpanAcross> {
    let s = content.bytes();
    let space = |c: &&u8| matches!(c, b'\0' | b'\t' | b'\n' | b'\x0c' | b'\r' | b' ');
    let ranges: Vec<Range<usize>> = spans
        .iter()
        .filter_map(|&span| content.offsets(span))
        .collect();
    let mut cuts: Vec<Range<usize>> = ranges
        .iter()
        .map(|r| {
            let before = s[..r.start].iter().rev().take_while(space).count();
            r.start - before..r.end + s[r.end..].iter().take_while(space).count()
        })
        .collect();
    for i in 1..cuts.len() {
        if cuts[i].start < cuts[i - 1].end {
            let middle = (ranges[i - 1].end + ranges[i].start) / 2;
            (cuts[i - 1].end, cuts[i].start) = (middle, middle);
        }
    }
    cuts.into_iter().map(|r| content.span(r)).collect()
}

/// That `kept`, `content` spliced without `drop`, lexes as the tokens of
/// `content` outside `drop`.
fn assert_kept(content: &Concat, res: &ResCx, kept: &[Cow<[u8]>], drop: &[SpanAcross]) {
    let lim = &Limits::DEFAULT;
    let drop: Vec<_> = drop.iter().filter_map(|&d| content.offsets(d)).collect();
    let before: Vec<&[u8]> = content
        .lex(res, lim)
        .map(|t| t.expect("a splice lexed it"))
        .filter(|(r, _)| !drop.iter().any(|d| d.start <= r.start && r.end <= d.end))
        .map(|(r, _)| &content.bytes()[r])
        .collect();
    let kept: Vec<&[u8]> = kept.iter().map(|s| &**s).collect();
    let kept = Concat::new(&kept);
    let after: Vec<&[u8]> = kept
        .lex(res, lim)
        .map(|t| t.expect("what a splice keeps lexes"))
        .map(|(r, _)| &kept.bytes()[r])
        .collect();
    assert_eq!(before, after, "the tokens a splice kept");
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

    /// What a mutation of content may write: its tokens, and the bytes
    /// that end an inline image's data.
    const CONTENT_TOKENS: &[&[u8]] = &[
        b" ",
        b"(",
        b")",
        b"\\",
        b"<",
        b">",
        b"<<",
        b">>",
        b"[",
        b"]",
        b"%",
        b"\n",
        b" BI ",
        b" ID ",
        b" EI ",
        b"/W 4 /H 1 /BPC 8 /CS /CS3 ",
        b"/F /DCT ",
        b"/L 9 ",
        b"~>",
        b"\xff\xd9",
        b"\x80",
        b" BDC ",
        b" EMC ",
        b" q ",
        b" Q ",
        b" BT ",
        b" ET ",
        b" /F1 9 Tf ",
        b" 0 -14 Td ",
        b" (x) Tj ",
        b" /X0 Do ",
        b" /Artifact /P0 BDC ",
        b" /Artifact <</Type /Pagination /Subtype /Footer>> BDC ",
        b"4294967296",
    ];

    fn mutate(rng: &mut fastrand::Rng, data: &mut Vec<u8>, tokens: &[&[u8]]) {
        if data.is_empty() {
            data.extend_from_slice(tokens[rng.usize(..tokens.len())]);
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
                let token = tokens[rng.usize(..tokens.len())];
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
    /// files they refer to, but for what makes a page stamp and an
    /// attachment marks: the stamp's appearance, after them, which names
    /// its page, and the attachment's file specification, which names its
    /// file. Neither the page nor the file is in it.
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
            &mut crate::annot::WriteCx::new(crate::forms::fixture::mark_assets()),
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
        // The appearance that draws a form: the page stamp's.
        let draws_a_form = |d: &lopdf::Dictionary| {
            let n = d
                .get(b"AP")
                .and_then(Object::as_dict)
                .ok()?
                .get(b"N")
                .ok()?;
            let n = n.as_reference().ok()?;
            let s = f.pdf.get_object(n).ok()?.as_stream().ok()?;
            let resources = s.dict.get(b"Resources").and_then(Object::as_dict).ok()?;
            resources.has(b"XObject").then(|| s.clone())
        };
        [false, true]
            .map(|v1| {
                let mut out = Pdf::with_version("1.7");
                out.reference_table.cross_reference_type = XrefType::CrossReferenceTable;
                let mut next = 4 + entries.len() as u32;
                for id in &entries {
                    let mut d = f.pdf.get_dictionary(*id).unwrap().clone();
                    let ap = draws_a_form(&d);
                    for key in [b"AP".as_slice(), b"P"] {
                        d.remove(key);
                    }
                    if let Some(stream) = ap {
                        out.objects.insert((next, 0), stream.into());
                        d.set("AP", dictionary! { "N" => (next, 0) });
                        next += 1;
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

    /// [`lexer`]'s fixtures: page content with an inline image found by
    /// each of PDFium's rules, ` EI ` in its data where the data is
    /// counted, and strings, arrays, dictionaries and marked content.
    fn content_fixtures() -> Vec<Vec<u8>> {
        let image = |dict: &str, data: &[u8]| {
            [format!("q BI {dict} ID ").as_bytes(), data, b"\nEI Q\n"].concat()
        };
        vec![
            image("/W 4 /H 1 /BPC 8 /CS /G", b" EI "),
            image("/W 2 /H 1 /BPC 8 /CS /CS3", b"a EI b"),
            image("/IM true /W 10 /H 2", b" EI "),
            image("/W 2 /H 2 /BPC 8 /CS [/I /RGB 1 <000000FFFFFF>]", b"EI E"),
            image("/F /AHx /W 2 /H 2 /BPC 8 /CS /G", b"0E EI 0>"),
            image("/F [/A85 /Fl] /W 9 /H 9", b"9jqo^ EI ~>"),
            image("/F /RL /W 4 /H 1 /BPC 8 /CS /G", b"\x03 EI \x80"),
            image("/F /DCT /W 9 /H 9 /BPC 8 /CS /RGB", b"\xff\xd8 EI \xff\xd9"),
            image("/F /Fl /L 6", b"x EI y"),
            image("/F /CCF /DP <</K -1>>", b"xEI EIy"),
            b"%c\nq /Artifact <</Type /Pagination /Subtype /Header /Attached [/Top]>> BDC\n\
              q BT /F1 12 Tf 72 720 Td [(a\\)(b) c) -250 <4142>] TJ (\\\\) Tj ET Q EMC\n\
              [1 [2 [3]] <</A <</B (c)>>>>] true false null Q { } ) ]"
                .to_vec(),
        ]
    }

    /// [`splice`]'s fixtures, beside the lexer's: pagination artifacts
    /// of each origin, Acrobat's opening with the `Q` that ends the page's
    /// `q`, a range across the two streams, ranges inside text objects,
    /// ranges with white-space between, and forms drawn in and out of
    /// artifacts.
    fn splice_fixtures() -> Vec<Vec<u8>> {
        [
            &b"q /Artifact /P0 BDC q BT /F1 9 Tf (Head) Tj ET Q EMC Q 0 0 9 9 re f"[..],
            b"q 0 g 0 0 9 9 re f 1 1 8 8 re f 2 2 7 7 re f 3 3 6 6 re f 0 0 1 rg 9 9 m 4 4 l h S  \
              /Artifact <</Type /Pagination /Subtype /Header>> BDC Q q 1 0 0 1 9 9 cm /X0 Do Q EMC",
            b"0 g/A <<>> BDC EMC  /B <<>> BDC EMC 1 g 0 0 9 9 re f 1 1 8 8 re f 2 2 7 7 re f",
            b"/Artifact <</Type /Pagination /Subtype /Header>> BDC q /X0 Do Q EMC /X1 Do",
            b"BT /F1 12 Tf 72 700 Td /Artifact <</Subtype /Footer>> BDC (f) Tj EMC \
              0 -14 Td (Body) Tj ET",
            b"/Artifact <</Subtype /Watermark /SheafStamp true>> BDC q 0.5 g \
              BT /SheafHF 9 Tf (wm) Tj ET Q EMC /Span <</MCID 0>> BDC (x) Tj EMC",
            b"1 g /Artifact <</Subtype /Background>> BDC 0 0 m 9 9 l S \
              BI /W 2 /H 1 /BPC 8 /CS /CS3 ID ab EIxyz EI EMC Q",
        ]
        .map(<[u8]>::to_vec)
        .into_iter()
        .chain(content_fixtures())
        .collect()
    }

    /// The fixtures, as the fuzz targets' first corpus, into the directory
    /// `SHEAF_FUZZ_SEEDS` names: `fuzz.yml` seeds each night's run with
    /// them, so the repository holds no PDF. The target
    /// `SHEAF_FUZZ_TARGET` names takes the marks' fixtures when it's
    /// `sheaf_annot`, the content when it's `lexer` or `splice`, and the
    /// files otherwise.
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
            Ok("lexer") => named(content_fixtures(), |n| format!("content-{n}")).collect(),
            Ok("splice") => named(splice_fixtures(), |n| format!("content-{n}")).collect(),
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
                    mutate(&mut rng, &mut data, TOKENS);
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
                    mutate(&mut rng, &mut data, TOKENS);
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
        for (n, fixture) in splice_fixtures().iter().enumerate() {
            // Unmutated, each lexes, and those with marked content splice.
            let res = ResCx::new([(b"CS3".to_vec(), 3)]);
            assert!(crate::content::lex(fixture, &res, &Limits::DEFAULT).all(|t| t.is_ok()));
            let marks = fixture.windows(3).any(|w| w == b"BDC");
            assert_eq!(splice(fixture) > 0, marks, "fixture {n}");
            for case in 0..CASES {
                let mut data = fixture.clone();
                for _ in 0..rng.usize(1..=4) {
                    mutate(&mut rng, &mut data, CONTENT_TOKENS);
                }
                let ran = std::panic::catch_unwind(|| {
                    lexer(&data);
                    splice(&data);
                });
                if let Err(panic) = ran {
                    let kept = std::env::temp_dir().join(format!("sheaf-fuzz-content-{n}-{case}"));
                    std::fs::write(&kept, &data).unwrap();
                    eprintln!("content {n}, case {case}: the input is {}", kept.display());
                    std::panic::resume_unwind(panic);
                }
            }
        }
        let took = started.elapsed();
        assert!(took < Duration::from_secs(60), "{took:?}");
    }
}
