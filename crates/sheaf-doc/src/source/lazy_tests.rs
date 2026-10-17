//! lopdf's `LazyPdf` (vendor/lopdf/SHEAF.md), whose tests live here:
//! what it reads equals lopdf's eager load, from memory and from a file;
//! an encrypted file decrypts object by object, as it was before it was
//! encrypted; and a hostile file is a typed error within the caps, and
//! quickly.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::io::Write as _;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lopdf::encryption::DecryptionError;
use lopdf::encryption::crypt_filters::{Aes128CryptFilter, Aes256CryptFilter, CryptFilter};
use lopdf::xref::XrefType;
use lopdf::{
    ByteSource, Dictionary, Document as Pdf, EncryptionState, EncryptionVersion, Error, FileBytes,
    LazyPdf, Limit, LoadOptions, Object, ObjectId, Permissions, ReadLimits, Stream, StringFormat,
    XrefKind, dictionary,
};

use super::Limits;
use crate::forms::corpus::Corpus;
use crate::forms::fixture::{every_kind, hybrid_reference, hybrid_update};

/// How long a hostile file may take to fail.
pub(super) const QUICK: Duration = Duration::from_secs(5);

fn open(bytes: &[u8]) -> LazyPdf {
    LazyPdf::open(Arc::<[u8]>::from(bytes), None, &Limits::DEFAULT.reader).unwrap()
}

/// `bytes` read by position from a file of their own, opened with
/// `password`.
fn open_file(bytes: &[u8], password: Option<&str>) -> LazyPdf {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("sheaf-lazy-{}-{n}.pdf", std::process::id()));
    std::fs::File::create(&path)
        .and_then(|mut f| f.write_all(bytes))
        .unwrap();
    let file = FileBytes::new(std::fs::File::open(&path).unwrap()).unwrap();
    let pdf = LazyPdf::open(file, password, &Limits::DEFAULT.reader).unwrap();
    std::fs::remove_file(&path).unwrap();
    pdf
}

/// Whether the two readers read the same object. A stream's position in
/// the bytes aside: they count it from different places.
fn same(a: &Object, b: &Object) -> bool {
    match (a, b) {
        (Object::Stream(a), Object::Stream(b)) => {
            (&a.dict, &a.content, a.allows_compression)
                == (&b.dict, &b.content, b.allows_compression)
        }
        _ => a == b,
    }
}

/// How `LazyPdf` reads an object otherwise than the eager load.
#[derive(Debug)]
enum Difference {
    Differs,
    Fails(Error),
    /// The table lists it and it reads lazily, but the eager load hasn't
    /// it.
    OnlyLazy,
}

/// The objects of the eager load that `lazy` reads otherwise, or can't,
/// and those its table lists that it reads and the eager load hasn't:
/// `/Encrypt` aside, which the eager load drops.
fn differences(eager: &Pdf, lazy: &LazyPdf) -> Vec<(ObjectId, Difference)> {
    let otherwise = eager
        .objects
        .iter()
        .filter_map(|(&id, object)| match lazy.get(id) {
            Ok(read) if same(read, object) => None,
            Ok(_) => Some((id, Difference::Differs)),
            Err(e) => Some((id, Difference::Fails(e))),
        });
    let encrypt = lazy
        .trailer()
        .get(b"Encrypt")
        .and_then(Object::as_reference)
        .ok();
    let only_lazy = lazy
        .ids()
        .filter(|&id| Some(id) != encrypt && !eager.objects.contains_key(&id))
        .filter(|&id| lazy.get(id).is_ok())
        .map(|id| (id, Difference::OnlyLazy));
    otherwise.chain(only_lazy).collect()
}

fn assert_same(eager: &Pdf, lazy: &LazyPdf, name: &str) {
    let found = differences(eager, lazy);
    assert!(found.is_empty(), "{name}: {found:?}");
}

/// [`every_kind`]'s form, written with a cross-reference table
/// (`classic`) or, with object streams, a cross-reference stream.
fn written(classic: bool) -> Vec<u8> {
    let mut form = every_kind();
    let mut bytes = Vec::new();
    if classic {
        form.pdf.reference_table.cross_reference_type = XrefType::CrossReferenceTable;
        form.pdf.save_to(&mut bytes).unwrap();
    } else {
        form.pdf.save_modern(&mut bytes).unwrap();
    }
    bytes
}

#[test]
fn lazy_objects_equal_the_eager_load() {
    let first = hybrid_reference();
    let files = [
        ("every_kind", every_kind().bytes()),
        ("classic", written(true)),
        ("object streams", written(false)),
        ("hybrid_reference", first.clone()),
        ("hybrid_update", hybrid_update(&first)),
    ];
    for (name, bytes) in files {
        let eager = Pdf::load_mem(&bytes).unwrap();
        assert!(!eager.objects.is_empty());
        for lazy in [open(&bytes), open_file(&bytes, None)] {
            assert!(lazy.base_sound(), "{name}");
            assert_same(&eager, &lazy, name);
            assert_eq!(lazy.trailer(), &eager.trailer, "{name}");
            assert_eq!(lazy.version(), eager.version, "{name}");
        }
    }

    // The update is a table over a table, each with its stream at
    // /XRefStm, the first section's numbered 9 and the update's 13.
    let facts = open(&hybrid_update(&first)).xref().clone();
    assert_eq!(facts.kind, XrefKind::Table);
    assert!(facts.hybrid);
    assert_eq!(facts.sections, 2);
    assert_eq!(facts.max_stream, Some(13));
    let streams = open(&written(false)).xref().clone();
    assert_eq!((streams.kind, streams.hybrid), (XrefKind::Stream, false));
    assert_eq!(streams.sections, 1);
    let linearized = with_table(&[catalog(), pages(), (3, b"<< /Linearized 1 >>".to_vec())]);
    let linearized = String::from_utf8(linearized)
        .unwrap()
        .replace("/Root 1 0 R >>", "/Root 1 0 R /ID [<0102> <0304>] >>");
    let facts = open(linearized.as_bytes()).xref().clone();
    assert_eq!(
        (facts.size, facts.linearized, facts.hybrid),
        (4, true, false)
    );
    assert_eq!(facts.id, Some([vec![1, 2], vec![3, 4]]));
    assert!(!streams.linearized);

    // Threads reading at once read the same, the first read of each
    // object kept.
    let bytes = written(false);
    let (eager, lazy) = (Pdf::load_mem(&bytes).unwrap(), open(&bytes));
    std::thread::scope(|s| {
        for _ in 0..4 {
            s.spawn(|| assert_same(&eager, &lazy, "threads"));
        }
    });
}

#[test]
fn a_file_with_a_100_byte_prefix_reads_identically() {
    let junk: Vec<u8> = (0..100u8).map(|i| b'a' + i % 26).collect();
    for classic in [true, false] {
        let bytes = written(classic);
        let prefixed = [junk.as_slice(), &bytes].concat();
        let eager = Pdf::load_mem(&bytes).unwrap();
        let (plain_pdf, shifted) = (open(&bytes), open(&prefixed));
        assert_eq!(plain_pdf.xref().header_offset, 0);
        assert_eq!(shifted.xref().header_offset, 100);
        assert_eq!(shifted.xref().start, plain_pdf.xref().start);
        assert_same(&eager, &shifted, "prefixed");
        assert_same(&eager, &open_file(&prefixed, None), "prefixed, from a file");
        let mut spans = 0;
        for &id in eager.objects.keys() {
            let span = plain_pdf.raw_span(id);
            assert_eq!(
                shifted.raw_span(id),
                span.clone().map(|s| s.start + 100..s.end + 100),
                "{id:?}"
            );
            if let Some(span) = span {
                spans += 1;
                let text = &bytes[span];
                assert!(text.starts_with(format!("{} {} obj", id.0, id.1).as_bytes()));
                assert!(text.ends_with(b"endobj"), "{id:?}");
            }
        }
        // Only the members of object streams have none.
        assert_eq!(spans == eager.objects.len(), classic);
        assert!(spans > 0);
    }
}

/// A file of `objects` (number, body) in a cross-reference table, the
/// first of them the catalog.
pub(super) fn with_table(objects: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = BTreeMap::new();
    for (number, body) in objects {
        offsets.insert(*number, out.len());
        out.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let size = offsets.keys().max().unwrap() + 1;
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for number in 1..size {
        match offsets.get(&number) {
            Some(offset) => out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes()),
            None => out.extend_from_slice(b"0000000000 65535 f \n"),
        }
    }
    let root = objects[0].0;
    let tail = format!("trailer\n<< /Size {size} /Root {root} 0 R >>\nstartxref\n{xref}\n%%EOF\n");
    out.extend_from_slice(tail.as_bytes());
    out
}

pub(super) fn catalog() -> (u32, Vec<u8>) {
    (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec())
}

fn pages() -> (u32, Vec<u8>) {
    (2, b"<< /Type /Pages /Kids [] /Count 0 >>".to_vec())
}

/// `n` zero bytes, deflated.
fn deflated_zeros(n: usize) -> Vec<u8> {
    let mut deflated = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    deflated.write_all(&vec![0; n]).unwrap();
    deflated.finish().unwrap()
}

/// A stream object's body: `dict` and `data`.
pub(super) fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    [
        format!("<< {dict} >>\nstream\n").as_bytes(),
        data,
        b"\nendstream",
    ]
    .concat()
}

/// A file of `objects` (number, body), the first of them the catalog,
/// and `members` (number, object stream), in a cross-reference stream
/// numbered last: its dictionary has `xref_dict` too, and its data is
/// `xref_data` instead of the entries when that's given.
fn with_stream(
    objects: &[(u32, Vec<u8>)],
    members: &[(u32, u32)],
    xref_dict: &str,
    xref_data: Option<Vec<u8>>,
) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut entries = BTreeMap::new();
    for (number, body) in objects {
        entries.insert(*number, (1u8, out.len() as u32));
        out.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    for &(number, container) in members {
        entries.insert(number, (2, container));
    }
    let number = entries.keys().max().unwrap() + 1;
    entries.insert(number, (1, out.len() as u32));
    let size = number + 1;
    let data = xref_data.unwrap_or_else(|| {
        let mut data = vec![0, 0, 0, 0, 0, 0xff, 0xff];
        for n in 1..size {
            let (kind, field) = entries.get(&n).copied().unwrap_or((0, 0));
            data.push(kind);
            data.extend_from_slice(&field.to_be_bytes());
            data.extend_from_slice(&[0, 0]);
        }
        data
    });
    let dict = format!(
        "/Type /XRef /Size {size} /W [1 4 2] /Root 1 0 R /Length {} {xref_dict}",
        data.len()
    );
    let xref = out.len();
    out.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
    out.extend_from_slice(&stream(&dict, &data));
    out.extend_from_slice(format!("\nendobj\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    out
}

#[test]
fn an_objstm_bomb_is_a_typed_error() {
    let limits = ReadLimits {
        max_decompressed: 1 << 20,
        ..ReadLimits::DEFAULT
    };
    // The pages are in an object stream that says it holds a billion
    // objects and inflates to 16 MiB.
    let bomb = deflated_zeros(16 << 20);
    let objstm = stream(
        &format!(
            "/Type /ObjStm /N 1000000000 /First 0 /Filter /FlateDecode /Length {}",
            bomb.len()
        ),
        &bomb,
    );
    let bytes = with_stream(&[catalog(), (3, objstm)], &[(2, 3)], "", None);
    let started = Instant::now();
    let pdf = LazyPdf::open(Arc::<[u8]>::from(bytes), None, &limits).unwrap();
    assert!(pdf.get((1, 0)).is_ok());
    for _ in 0..2 {
        assert!(matches!(
            pdf.get((2, 0)),
            Err(Error::Limit(Limit::Decompressed))
        ));
        assert!(matches!(
            pdf.get_dictionary((2, 0)),
            Err(Error::Limit(Limit::Decompressed))
        ));
    }
    assert!(started.elapsed() < QUICK);

    // A cross-reference stream as a bomb fails the open: it isn't a
    // damaged table to rebuild.
    let bytes = with_stream(
        &[catalog(), pages()],
        &[],
        "/Filter /FlateDecode",
        Some(bomb),
    );
    let started = Instant::now();
    assert!(matches!(
        LazyPdf::open(Arc::<[u8]>::from(bytes), None, &limits),
        Err(Error::Limit(Limit::Decompressed))
    ));
    assert!(started.elapsed() < QUICK);
}

/// `entries` cross-reference stream entries, each `[1 0 0]` under
/// `/W [1 1 1]`, deflated as they're written.
fn xref_entries(entries: usize) -> Vec<u8> {
    const CHUNK: usize = 1 << 16;
    let chunk = [1, 0, 0].repeat(CHUNK);
    let mut deflated = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut left = entries;
    while left > 0 {
        let n = left.min(CHUNK);
        deflated.write_all(&chunk[..3 * n]).unwrap();
        left -= n;
    }
    deflated.finish().unwrap()
}

/// A file whose cross-reference sections are `sections` streams chained by
/// `/Prev`, each listing `entries` objects from 0 with `data`.
fn stream_chain(sections: usize, entries: usize, data: &[u8]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut prev = None;
    for n in 1..=sections {
        let at = out.len();
        let dict = format!(
            "/Type /XRef /Size {entries} /W [1 1 1] /Root 1 0 R /Filter /FlateDecode /Length {}{}",
            data.len(),
            prev.map_or(String::new(), |prev| format!(" /Prev {prev}"))
        );
        out.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
        out.extend_from_slice(&stream(&dict, data));
        out.extend_from_slice(b"\nendobj\n");
        prev = Some(at);
    }
    let start = prev.unwrap();
    out.extend_from_slice(format!("startxref\n{start}\n%%EOF\n").as_bytes());
    out
}

#[test]
fn a_cross_reference_stream_bomb_is_refused_from_its_dictionary() {
    // 50 million entries, 150 MB unpacked: past the object cap, so refused
    // before they're decompressed, in one section or the first of four.
    let entries = 50_000_000;
    let data = xref_entries(entries);
    for sections in [1, 4] {
        let bytes = stream_chain(sections, entries, &data);
        let started = Instant::now();
        assert!(matches!(
            LazyPdf::open(Arc::<[u8]>::from(bytes), None, &Limits::DEFAULT.reader),
            Err(Error::Limit(Limit::Objects))
        ));
        assert!(started.elapsed() < QUICK, "{:?}", started.elapsed());
    }
    // Told from the dictionary alone: data that doesn't decompress is
    // refused alike, where it would have the table rebuilt.
    let bytes = stream_chain(1, entries, b"not deflated");
    assert!(matches!(
        LazyPdf::open(Arc::<[u8]>::from(bytes), None, &Limits::DEFAULT.reader),
        Err(Error::Limit(Limit::Objects))
    ));

    // Sections relisting the same numbers stay within the cap one by one,
    // but the streams list twice as many entries as there are numbers at
    // most, together.
    let limits = ReadLimits {
        max_objects: 100_000,
        ..ReadLimits::DEFAULT
    };
    let data = xref_entries(100_000);
    let open = |sections| {
        LazyPdf::open(
            Arc::<[u8]>::from(stream_chain(sections, 100_000, &data)),
            None,
            &limits,
        )
    };
    assert_eq!(open(2).unwrap().xref().sections, 2);
    let started = Instant::now();
    for sections in [3, 20] {
        assert!(matches!(open(sections), Err(Error::Limit(Limit::Objects))));
    }
    assert!(started.elapsed() < QUICK, "{:?}", started.elapsed());
}

/// Bytes in memory, read a window at a time as a file is, counting the
/// bytes read.
struct Counted {
    bytes: Arc<[u8]>,
    read: Arc<AtomicUsize>,
}

impl ByteSource for Counted {
    fn len(&self) -> usize {
        self.bytes.len()
    }

    fn read_at(&self, range: Range<usize>) -> std::io::Result<Cow<'_, [u8]>> {
        self.read.fetch_add(range.len(), Ordering::Relaxed);
        Ok(Cow::Borrowed(&self.bytes[range]))
    }

    fn first_window(&self) -> usize {
        4096
    }
}

/// A file's only section near its start, as a linearized file's first
/// page's is, and `startxref` a few bytes off it: read by position, the
/// probe that finds the section reads a few bytes, not the 8 MB stream
/// after it.
#[test]
fn a_slightly_wrong_startxref_reads_little_of_a_file() {
    let header = b"%PDF-1.7\n";
    let data = vec![b'x'; 8 << 20];
    let objects = [
        catalog(),
        pages(),
        (3, stream(&format!("/Length {}", data.len()), &data)),
    ];
    let trailer = b"trailer\n<< /Size 4 /Root 1 0 R >>\n";
    let table_len = b"xref\n0 4\n".len() + 4 * 20 + trailer.len();
    let mut body = Vec::new();
    let mut table = b"xref\n0 4\n0000000000 65535 f \n".to_vec();
    for (number, object) in objects {
        let offset = header.len() + table_len + body.len();
        table.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        body.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
        body.extend_from_slice(&object);
        body.extend_from_slice(b"\nendobj\n");
    }
    table.extend_from_slice(trailer);
    assert_eq!(table.len(), table_len);
    let off = header.len() + 3;
    let tail = format!("startxref\n{off}\n%%EOF\n");
    let bytes = [&header[..], &table, &body, tail.as_bytes()].concat();
    let read = Arc::new(AtomicUsize::new(0));
    let counted = Counted {
        bytes: bytes.into(),
        read: read.clone(),
    };
    let pdf = LazyPdf::open(counted, None, &Limits::DEFAULT.reader).unwrap();
    assert!(pdf.base_sound());
    assert_eq!(pdf.xref().start, header.len());
    assert!(pdf.catalog().is_ok());
    let read = read.load(Ordering::Relaxed);
    assert!(read < 64 << 10, "{read} bytes read");
}

#[test]
fn a_4096_section_chain_is_a_typed_error() {
    let mut bytes = with_table(&[catalog(), pages()]);
    let tail = bytes.windows(9).rposition(|w| w == b"startxref").unwrap();
    let mut prev: usize = String::from_utf8_lossy(&bytes[tail + 9..])
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    for _ in 1..4096 {
        let section = bytes.len();
        let trailer = format!("<< /Size 3 /Root 1 0 R /Prev {prev} >>");
        bytes.extend_from_slice(format!("xref\n0 0\ntrailer\n{trailer}\n").as_bytes());
        prev = section;
    }
    bytes.extend_from_slice(format!("startxref\n{prev}\n%%EOF\n").as_bytes());
    let started = Instant::now();
    assert!(matches!(
        LazyPdf::open(Arc::<[u8]>::from(bytes), None, &Limits::DEFAULT.reader),
        Err(Error::Limit(Limit::Sections))
    ));
    assert!(started.elapsed() < QUICK);
}

#[test]
fn a_10000_deep_reference_chain_is_a_typed_error() {
    // Objects 3 on are each a reference to the next, the last of them 42.
    let mut objects = vec![catalog(), pages()];
    for n in 3..10_003 {
        objects.push((n, format!("{} 0 R", n + 1).into_bytes()));
    }
    objects.push((10_003, b"42".to_vec()));
    let pdf = open(&with_table(&objects));
    let started = Instant::now();
    assert!(matches!(
        pdf.get_object((3, 0)),
        Err(Error::Limit(Limit::Depth))
    ));
    assert!(matches!(
        pdf.dereference(&Object::Reference((3, 0))),
        Err(Error::Limit(Limit::Depth))
    ));
    assert_eq!(pdf.get_object((10_002, 0)).unwrap(), &Object::Integer(42));
    assert!(started.elapsed() < QUICK);

    // Streams from 3 on, each with the next as its /Length.
    let mut objects = vec![catalog(), pages()];
    for n in 3..10_003 {
        objects.push((n, stream(&format!("/Length {} 0 R", n + 1), b"")));
    }
    objects.push((10_003, b"0".to_vec()));
    let pdf = open(&with_table(&objects));
    let started = Instant::now();
    for _ in 0..2 {
        assert!(matches!(pdf.get((3, 0)), Err(Error::Limit(Limit::Depth))));
    }
    // Near its end the chain is short enough to read.
    assert!(pdf.get((10_000, 0)).is_ok());
    assert!(started.elapsed() < QUICK);
}

/// A stream whose `/Length` the parser can't read, a chain of references
/// or an integral real, reads as the eager reader reads it, and its span
/// ends after `endobj`, past its data.
#[test]
fn a_stream_whose_length_the_parser_cant_read_spans_to_endobj() {
    let bytes = with_table(&[
        catalog(),
        pages(),
        (3, stream("/Length 6 0 R", b"abc")),
        (4, stream("/Length 3.0", b"abc")),
        // A length that names nothing: read empty, as eagerly, with no
        // end to its span.
        (5, stream("/Length 9 0 R", b"abc")),
        (6, b"7 0 R".to_vec()),
        (7, b"3".to_vec()),
    ]);
    let eager = Pdf::load_mem(&bytes).unwrap();
    for pdf in [open(&bytes), open_file(&bytes, None)] {
        assert_same(&eager, &pdf, "lengths");
        for id in [(3, 0), (4, 0)] {
            assert_eq!(pdf.get(id).unwrap().as_stream().unwrap().content, b"abc");
            let text = &bytes[pdf.raw_span(id).unwrap()];
            assert!(text.starts_with(format!("{} 0 obj", id.0).as_bytes()));
            assert!(text.ends_with(b"abc\nendstream\nendobj"), "{id:?}");
        }
        assert!(
            pdf.get((5, 0))
                .unwrap()
                .as_stream()
                .unwrap()
                .content
                .is_empty()
        );
        assert_eq!(pdf.raw_span((5, 0)), None);
    }
}

/// `object_unkept` reads what `object` reads without keeping it, but for
/// what it resolved on the way, and gives what's kept as it's kept.
#[test]
fn an_unkept_read_keeps_only_what_it_met() {
    let bytes = with_table(&[
        catalog(),
        pages(),
        (3, stream("/Length 4 0 R", b"abc")),
        (4, b"3".to_vec()),
        (5, b"<< /Kept true >>".to_vec()),
        (6, b"<< /Broken".to_vec()),
    ]);
    let pdf = open(&bytes);
    let unkept = pdf.object_unkept((3, 0)).unwrap();
    assert_eq!(unkept.as_stream().unwrap().content, b"abc");
    assert!(!pdf.is_read((3, 0)) && pdf.is_read((4, 0)));
    assert_eq!(*unkept, *pdf.object((3, 0)).unwrap());
    let kept = pdf.object((5, 0)).unwrap();
    assert!(Arc::ptr_eq(&kept, &pdf.object_unkept((5, 0)).unwrap()));
    // A failure the bytes decide is kept, as `get` keeps it.
    assert!(pdf.object_unkept((6, 0)).is_err());
    assert!(pdf.is_read((6, 0)));
    assert!(matches!(
        pdf.object_unkept((9, 0)),
        Err(Error::MissingXrefEntry)
    ));
}

#[test]
fn a_length_cycle_is_a_typed_error() {
    // 3's /Length is 4, a stream whose /Length is 3; 5's is itself; and 6's
    // runs through a cycle of 60 streams, 7 to 66.
    let mut objects = vec![
        catalog(),
        pages(),
        (3, stream("/Length 4 0 R", b"abc")),
        (4, stream("/Length 3 0 R", b"abc")),
        (5, stream("/Length 5 0 R", b"abc")),
        (6, stream("/Length 7 0 R", b"abc")),
    ];
    for n in 7..67 {
        let next = if n == 66 { 7 } else { n + 1 };
        objects.push((n, stream(&format!("/Length {next} 0 R"), b"abc")));
    }
    let pdf = open(&with_table(&objects));
    let started = Instant::now();
    // Threads reading the cycles at once don't wait on each other.
    std::thread::scope(|s| {
        for n in 3..67 {
            let pdf = &pdf;
            s.spawn(move || assert!(matches!(pdf.get((n, 0)), Err(Error::ReferenceCycle(_)))));
        }
    });
    for _ in 0..2 {
        assert!(matches!(pdf.get((3, 0)), Err(Error::ReferenceCycle(_))));
        assert!(matches!(pdf.get((4, 0)), Err(Error::ReferenceCycle(_))));
        assert!(matches!(
            pdf.get((5, 0)),
            Err(Error::ReferenceCycle((5, 0)))
        ));
        assert!(matches!(pdf.get((6, 0)), Err(Error::ReferenceCycle(_))));
    }
    assert!(pdf.get((1, 0)).is_ok());
    assert!(started.elapsed() < QUICK);
}

#[test]
fn a_broken_xref_falls_back_to_an_eager_load() {
    // startxref points into the header: the table is rebuilt from the
    // objects' headers, which miss the object stream's members.
    let mut bytes = hybrid_reference();
    let tail = bytes.windows(9).rposition(|w| w == b"startxref").unwrap();
    bytes.truncate(tail);
    bytes.extend_from_slice(b"startxref\n1\n%%EOF\n");
    let eager = Pdf::load_mem(&bytes).unwrap();
    for lazy in [open(&bytes), open_file(&bytes, None)] {
        assert!(!lazy.base_sound());
        assert_eq!(lazy.xref().start, 0);
        assert_same(&eager, &lazy, "rebuilt");
        for member in [5, 6, 7] {
            assert!(lazy.get((member, 0)).is_ok(), "{member}");
        }
    }

    // Junk before the header, and the table's offsets counted from the
    // start of the file: its first object isn't where it says, so it's
    // rebuilt, as PDFium rebuilds it.
    let plain = with_table(&[catalog(), pages()]);
    let junk = b"BT /F1 11 Tf ET\n".repeat(4);
    let mut shifted = String::from_utf8([junk.as_slice(), &plain].concat()).unwrap();
    for number in [1, 2] {
        let header = format!("{number} 0 obj");
        let offset = plain
            .windows(header.len())
            .position(|w| w == header.as_bytes())
            .unwrap();
        let entry = format!("{offset:010} 00000 n");
        let absolute = format!("{:010} 00000 n", offset + junk.len());
        shifted = shifted.replace(&entry, &absolute);
    }
    let eager = Pdf::load_mem(&plain).unwrap();
    for lazy in [
        open(shifted.as_bytes()),
        open_file(shifted.as_bytes(), None),
    ] {
        assert!(!lazy.base_sound());
        assert_eq!(lazy.xref().header_offset, junk.len());
        assert_same(&eager, &lazy, "counted from the start of the file");
    }
    assert!(open(&plain).base_sound());

    // A file with only cross-reference streams has no `trailer` keyword:
    // rebuilt, its trailer is its newest stream's dictionary, as PDFium
    // takes it, without the keys that describe the stream.
    let mut bytes = written(false);
    let tail = bytes.windows(9).rposition(|w| w == b"startxref").unwrap();
    bytes.truncate(tail);
    bytes.extend_from_slice(b"startxref\n1\n%%EOF\n");
    assert!(!holds(&bytes, b"trailer"));
    let eager = Pdf::load_mem(&bytes).unwrap();
    for lazy in [open(&bytes), open_file(&bytes, None)] {
        assert!(!lazy.base_sound());
        assert_same(&eager, &lazy, "streams only");
        let trailer = lazy.trailer();
        assert!(trailer.has_type(b"XRef") && lazy.catalog().is_ok());
        for key in ["Length", "W", "Index", "Prev"] {
            assert!(!trailer.has(key.as_bytes()), "{key}");
        }
    }
}

const OWNER: &str = "owner";
const USER: &str = "user";

/// A signature's `/Contents`, which is never encrypted, and how it's
/// written.
const SIGNATURE: &[u8] = &[0x30, 0x82, 0x01, 0x0a, 0xde, 0xad, 0xbe, 0xef];
const SIGNATURE_WRITTEN: &[u8] = b"<3082010ADEADBEEF>";

/// What [`plaintext`] holds in a string or a stream, each once.
const PAGE_STRING: &[u8] = b"a page's string";
const FORM_STRING: &[u8] = b"a form's string";
const CONTENT: &[u8] = b"q /Fx Do Q BT (the page's content) Tj ET";
const XMP: &[u8] = b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">an XMP title</x:xmpmeta>";
const ATTACHMENT: &[u8] = b"an attachment's data";
const FIELD_NAME: &[u8] = b"a signature's field";
const TITLE: &[u8] = b"an Info title";

/// A file to encrypt, built here: a page with a string in its
/// dictionary, whose content draws a form XObject with strings in its
/// stream's dictionary; XMP metadata; an embedded file with its
/// `/Params`; a signature; an Info title; and the `/ID` the RC4 and
/// AES-128 keys are made from.
pub(super) fn plaintext() -> Pdf {
    let piece = |private: &[u8]| {
        dictionary! { "Sheaf" => dictionary! {
            "LastModified" => Object::string_literal("D:20261016120000Z"),
            "Private" => Object::string_literal(private),
        } }
    };
    let mut pdf = Pdf::with_version("1.7");
    let pages = pdf.new_object_id();
    let form = pdf.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => vec![0.into(), 0.into(), 100.into(), 100.into()],
            "PieceInfo" => piece(FORM_STRING),
            "LastModified" => Object::string_literal("D:20261016120000Z"),
        },
        b"0 0 100 100 re f".to_vec(),
    ));
    let content = pdf.add_object(Stream::new(dictionary! {}, CONTENT.to_vec()));
    let page = pdf.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages,
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        "Resources" => dictionary! { "XObject" => dictionary! { "Fx" => form } },
        "Contents" => content,
        "PieceInfo" => piece(PAGE_STRING),
        "LastModified" => Object::string_literal("D:20261016120000Z"),
    });
    let kids = dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 };
    pdf.objects.insert(pages, kids.into());
    let metadata = pdf.add_object(Stream::new(
        dictionary! { "Type" => "Metadata", "Subtype" => "XML" },
        XMP.to_vec(),
    ));
    let attachment = pdf.add_object(Stream::new(
        dictionary! {
            "Type" => "EmbeddedFile",
            "Subtype" => "text/plain",
            "Params" => dictionary! {
                "Size" => ATTACHMENT.len() as i64,
                "ModDate" => Object::string_literal("D:20261016120000Z"),
                "CheckSum" => Object::String(vec![0x5a; 16], StringFormat::Hexadecimal),
            },
        },
        ATTACHMENT.to_vec(),
    ));
    let spec = pdf.add_object(dictionary! {
        "Type" => "Filespec",
        "F" => Object::string_literal("notes.txt"),
        "EF" => dictionary! { "F" => attachment },
    });
    let signature = pdf.add_object(dictionary! {
        "Type" => "Sig",
        "Filter" => "Adobe.PPKLite",
        "SubFilter" => "adbe.pkcs7.detached",
        "Contents" => Object::String(SIGNATURE.to_vec(), StringFormat::Hexadecimal),
        "M" => Object::string_literal("D:20261016120000Z"),
    });
    let field = pdf.add_object(dictionary! {
        "FT" => "Sig",
        "T" => Object::string_literal(FIELD_NAME),
        "V" => signature,
    });
    let catalog = pdf.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages,
        "Metadata" => metadata,
        "Names" => dictionary! { "EmbeddedFiles" => dictionary! {
            "Names" => vec![Object::string_literal("notes.txt"), spec.into()],
        } },
        "AcroForm" => dictionary! { "Fields" => vec![field.into()] },
    });
    let info = pdf.add_object(dictionary! { "Title" => Object::string_literal(TITLE) });
    pdf.trailer.set("Root", catalog);
    pdf.trailer.set("Info", info);
    let id = Object::String(b"0123456789abcdef".to_vec(), StringFormat::Hexadecimal);
    pdf.trailer.set("ID", vec![id.clone(), id]);
    pdf
}

/// `plain` encrypted with `state`, written with a cross-reference table
/// and with a stream. The `/Encrypt` dictionary gets `edit`'s changes
/// first.
pub(super) fn encrypted(
    plain: &Pdf,
    state: &EncryptionState,
    edit: impl Fn(&mut Dictionary),
) -> [Vec<u8>; 2] {
    [
        XrefType::CrossReferenceTable,
        XrefType::CrossReferenceStream,
    ]
    .map(|kind| {
        let mut pdf = plain.clone();
        pdf.encrypt(state).unwrap();
        let id = pdf.trailer.get(b"Encrypt").unwrap().as_reference().unwrap();
        edit(pdf.get_dictionary_mut(id).unwrap());
        pdf.reference_table.cross_reference_type = kind;
        let mut bytes = Vec::new();
        pdf.save_to(&mut bytes).unwrap();
        bytes
    })
}

fn holds(bytes: &[u8], text: &[u8]) -> bool {
    bytes.windows(text.len()).any(|w| w == text)
}

/// `bytes` read with the user password is `plain`, object by object:
/// eagerly, and lazily as eagerly, from memory and from a file. Returns
/// the lazy read from memory.
fn assert_decrypts_to(plain: &Pdf, bytes: &[u8], name: &str) -> LazyPdf {
    let eager = Pdf::load_mem_with_options(bytes, LoadOptions::with_password(USER)).unwrap();
    let lazy = LazyPdf::open(
        Arc::<[u8]>::from(bytes),
        Some(USER),
        &Limits::DEFAULT.reader,
    )
    .unwrap();
    for (id, object) in &plain.objects {
        assert!(same(&eager.objects[id], object), "{name}, eagerly: {id:?}");
    }
    assert_same(&eager, &lazy, name);
    assert_same(&eager, &open_file(bytes, Some(USER)), name);
    // And the cross-reference stream, when there is one, as written: its
    // /ID isn't decrypted.
    for (&id, object) in &eager.objects {
        if !plain.objects.contains_key(&id) {
            for xref in [object, lazy.get(id).unwrap()] {
                let dict = &xref.as_stream().unwrap().dict;
                assert!(dict.has_type(b"XRef"), "{name}: {id:?}");
                assert_eq!(
                    dict.get(b"ID").ok(),
                    plain.trailer.get(b"ID").ok(),
                    "{name}"
                );
            }
        }
    }
    assert!(matches!(
        LazyPdf::open(Arc::<[u8]>::from(bytes), None, &Limits::DEFAULT.reader),
        Err(Error::InvalidPassword)
    ));
    lazy
}

#[test]
fn rc4_128_aes_128_and_aes_256_decrypt_per_object_identically() {
    let plain = plaintext();
    let std_cf = |filter: Arc<dyn CryptFilter>| BTreeMap::from([(b"StdCF".to_vec(), filter)]);
    let versions = [
        (
            "RC4-128",
            EncryptionVersion::V2 {
                document: &plain,
                owner_password: OWNER,
                user_password: USER,
                key_length: 128,
                permissions: Permissions::all(),
            },
        ),
        (
            "AES-128",
            EncryptionVersion::V4 {
                document: &plain,
                encrypt_metadata: true,
                crypt_filters: std_cf(Arc::new(Aes128CryptFilter)),
                stream_filter: b"StdCF".to_vec(),
                string_filter: b"StdCF".to_vec(),
                owner_password: OWNER,
                user_password: USER,
                permissions: Permissions::all(),
            },
        ),
        (
            "AES-256",
            EncryptionVersion::V5 {
                encrypt_metadata: true,
                crypt_filters: std_cf(Arc::new(Aes256CryptFilter)),
                file_encryption_key: &[7; 32],
                stream_filter: b"StdCF".to_vec(),
                string_filter: b"StdCF".to_vec(),
                owner_password: OWNER,
                user_password: USER,
                permissions: Permissions::all(),
            },
        ),
    ];
    for (name, version) in versions {
        let state = EncryptionState::try_from(version).unwrap();
        for bytes in encrypted(&plain, &state, |_| {}) {
            // Every string and stream is encrypted, those in a stream's
            // dictionary too, but a signature's /Contents.
            for text in [
                PAGE_STRING,
                FORM_STRING,
                CONTENT,
                XMP,
                ATTACHMENT,
                FIELD_NAME,
                TITLE,
            ] {
                assert!(
                    !holds(&bytes, text),
                    "{name}: {:?}",
                    String::from_utf8_lossy(text)
                );
            }
            assert!(holds(&bytes, SIGNATURE_WRITTEN), "{name}");
            assert_decrypts_to(&plain, &bytes, name);
        }
    }
}

/// V4 with AES-128 for embedded files only: `/StmF` and `/StrF` are
/// `/Identity`, and `/EFF` names `/CF`'s `/StdCF`.
fn attachment_only(plain: &Pdf) -> EncryptionState {
    let mut state = EncryptionState::try_from(EncryptionVersion::V4 {
        document: plain,
        encrypt_metadata: true,
        crypt_filters: BTreeMap::from([(
            b"StdCF".to_vec(),
            Arc::new(Aes128CryptFilter) as Arc<dyn CryptFilter>,
        )]),
        stream_filter: b"Identity".to_vec(),
        string_filter: b"Identity".to_vec(),
        owner_password: OWNER,
        user_password: USER,
        permissions: Permissions::all(),
    })
    .unwrap();
    state.set_embedded_file_filter(b"StdCF".to_vec());
    state
}

#[test]
fn an_attachment_only_eff_file_decrypts_its_attachment() {
    let plain = plaintext();
    for bytes in encrypted(&plain, &attachment_only(&plain), |_| {}) {
        // Only the attachment's data is encrypted.
        assert!(!holds(&bytes, ATTACHMENT));
        for text in [
            PAGE_STRING,
            FORM_STRING,
            CONTENT,
            XMP,
            FIELD_NAME,
            TITLE,
            SIGNATURE_WRITTEN,
        ] {
            assert!(holds(&bytes, text), "{:?}", String::from_utf8_lossy(text));
        }
        let lazy = assert_decrypts_to(&plain, &bytes, "attachment only");
        let (_, encrypt) = lazy
            .dereference(lazy.trailer().get(b"Encrypt").unwrap())
            .unwrap();
        let encrypt = encrypt.as_dict().unwrap();
        for (key, name) in [("StmF", "Identity"), ("StrF", "Identity"), ("EFF", "StdCF")] {
            assert_eq!(
                encrypt.get(key.as_bytes()).unwrap(),
                &Object::from(name),
                "{key}"
            );
        }
        // The attachment, found as a reader finds it.
        let file = lazy
            .catalog()
            .and_then(|catalog| catalog.get(b"Names"))
            .and_then(Object::as_dict)
            .and_then(|names| names.get(b"EmbeddedFiles"))
            .and_then(Object::as_dict)
            .and_then(|tree| tree.get(b"Names"))
            .and_then(Object::as_array)
            .and_then(|tree| lazy.get_dictionary(tree[1].as_reference()?))
            .and_then(|spec| spec.get(b"EF"))
            .and_then(Object::as_dict)
            .and_then(|files| files.get(b"F"))
            .and_then(Object::as_reference)
            .and_then(|file| lazy.get(file))
            .and_then(Object::as_stream)
            .unwrap();
        assert_eq!(file.content, ATTACHMENT);
    }
}

/// A crypt filter that `/StmF`, `/StrF` or `/EFF` names but neither is
/// `/Identity` nor `/CF` has fails the open, eagerly too.
#[test]
fn an_undefined_crypt_filter_is_a_typed_error() {
    let plain = plaintext();
    let state = attachment_only(&plain);
    for key in ["StmF", "StrF", "EFF"] {
        let [bytes, _] = encrypted(&plain, &state, |encrypt| encrypt.set(key, "Undefined"));
        let bytes = Arc::<[u8]>::from(bytes);
        let Err(Error::Decryption(DecryptionError::UndefinedCryptFilter(name))) =
            LazyPdf::open(bytes.clone(), Some(USER), &Limits::DEFAULT.reader)
        else {
            panic!("{key}");
        };
        assert_eq!(name, b"Undefined");
        assert!(
            matches!(
                Pdf::load_mem_with_options(&bytes, LoadOptions::with_password(USER)),
                Err(Error::Decryption(DecryptionError::UndefinedCryptFilter(_)))
            ),
            "{key}"
        );
    }
}

/// Why a corpus file's objects read otherwise lazily than eagerly, where
/// lopdf's eager load is what's wrong, or the file is.
#[derive(Debug, Clone, Copy)]
enum Explained {
    /// The eager load keeps an object stream's members that the
    /// cross-reference table doesn't list in use: lazily they aren't there.
    Unlisted,
    /// Another entry's offset holds an object with the same number, which
    /// the eager load keeps instead, keying objects by their headers: an
    /// earlier revision's Info dictionary. Lazily the table's object is
    /// read.
    Shadowed,
    /// A stream or string doesn't decrypt (AES padding): an XMP stream
    /// stored in the clear though `/EncryptMetadata` is true, or a
    /// `/SigRef`'s `/DigestValue`. Lazily a typed error; eagerly what was
    /// there, half decrypted.
    Undecryptable,
}

impl Explained {
    /// Whether it explains object `id` reading as `difference`, in a file
    /// whose trailer's `/Info` is `info`.
    fn covers(self, id: ObjectId, difference: &Difference, info: Option<ObjectId>) -> bool {
        match self {
            Explained::Unlisted => matches!(difference, Difference::Fails(Error::MissingXrefEntry)),
            Explained::Shadowed => matches!(difference, Difference::Differs) && info == Some(id),
            Explained::Undecryptable => {
                matches!(difference, Difference::Fails(Error::Decryption(_)))
            }
        }
    }
}

/// The corpus files whose objects read otherwise lazily, why, and how
/// many objects do.
const EXPLAINED: &[(&str, Explained, usize)] = &[
    ("forms/safedocs/0054508.pdf", Explained::Shadowed, 1),
    ("forms/safedocs/0078699.pdf", Explained::Shadowed, 1),
    ("forms/safedocs/0155165.pdf", Explained::Shadowed, 1),
    ("forms/safedocs/0169467.pdf", Explained::Undecryptable, 1),
    ("forms/safedocs/0270171.pdf", Explained::Shadowed, 1),
    ("forms/safedocs/0372984.pdf", Explained::Shadowed, 1),
    ("forms/safedocs/0845477.pdf", Explained::Unlisted, 70),
    ("forms/safedocs/1156137.pdf", Explained::Unlisted, 1),
    ("forms/safedocs/1263925.pdf", Explained::Undecryptable, 1),
    ("forms/safedocs/1530261.pdf", Explained::Unlisted, 86),
    ("forms/safedocs/2280585.pdf", Explained::Unlisted, 80),
    ("forms/safedocs/3401937.pdf", Explained::Unlisted, 6),
];

/// Every manifest file lopdf loads (with the empty password, as
/// `load_mem` does) reads lazily as it loads, from memory and from the
/// file: each object but `/Encrypt`, which the eager load drops, both
/// ways. The differences are [`EXPLAINED`]'s, as many as it says, and
/// only theirs. Skips without the corpus.
#[test]
fn corpus_objects_read_lazily_equal_the_eager_load() {
    let Some(corpus) = Corpus::or_skip() else {
        return;
    };
    let entries = corpus.manifest().unwrap();
    #[derive(Default)]
    struct Counts {
        files: usize,
        objects: usize,
        encrypted: usize,
        rebuilt: usize,
        needs_password: usize,
        unloadable: usize,
    }
    let counts = Mutex::new(Counts::default());
    let differ = Mutex::new(BTreeMap::new());
    let queue = Mutex::new(entries.iter());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(16));
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let Some(entry) = queue.lock().unwrap().next() else {
                        break;
                    };
                    let path = corpus.path(entry);
                    let bytes = std::fs::read(&path).unwrap();
                    let Ok(eager) = Pdf::load_mem(&bytes) else {
                        counts.lock().unwrap().unloadable += 1;
                        continue;
                    };
                    // Still encrypted: the empty password didn't work.
                    if eager
                        .trailer
                        .get(b"Encrypt")
                        .is_ok_and(|e| *e != Object::Null)
                    {
                        counts.lock().unwrap().needs_password += 1;
                        continue;
                    }
                    let file = FileBytes::new(std::fs::File::open(&path).unwrap()).unwrap();
                    let opened = [
                        LazyPdf::open(Arc::<[u8]>::from(bytes), None, &Limits::DEFAULT.reader),
                        LazyPdf::open(file, None, &Limits::DEFAULT.reader),
                    ];
                    let mut found = Vec::new();
                    let mut rebuilt = false;
                    for lazy in opened {
                        match lazy {
                            Ok(lazy) => {
                                rebuilt |= !lazy.base_sound();
                                found.extend(differences(&eager, &lazy));
                            }
                            Err(e) => found.push(((0, 0), Difference::Fails(e))),
                        }
                    }
                    let mut c = counts.lock().unwrap();
                    c.files += 1;
                    c.objects += eager.objects.len();
                    c.encrypted += usize::from(eager.encryption_state.is_some());
                    c.rebuilt += usize::from(rebuilt);
                    drop(c);
                    if !found.is_empty() {
                        let info = eager.trailer.get(b"Info").and_then(Object::as_reference);
                        let found = (info.ok(), found);
                        differ.lock().unwrap().insert(entry.path.clone(), found);
                    }
                }
            });
        }
    });
    let c = counts.into_inner().unwrap();
    let differ = differ.into_inner().unwrap();
    eprintln!(
        "{} files, {} objects, each read lazily from memory and from the file; {} decrypted, \
         {} with their tables rebuilt; {} need a password, {} lopdf can't load; {} differ",
        c.files,
        c.objects,
        c.encrypted,
        c.rebuilt,
        c.needs_password,
        c.unloadable,
        differ.len()
    );
    let mut unexplained = Vec::new();
    for (path, (info, found)) in &differ {
        let why = EXPLAINED.iter().find(|(p, ..)| p == path);
        // Each difference is found twice: from memory and from the file.
        eprintln!("{path}: {} objects, {why:?}", found.len() / 2);
        let explained = why.is_some_and(|&(_, why, objects)| {
            found.len() == 2 * objects && found.iter().all(|(id, d)| why.covers(*id, d, *info))
        });
        if !explained {
            unexplained.push((path, found));
        }
    }
    assert!(unexplained.is_empty(), "{unexplained:#?}");
    let stale: Vec<&str> = EXPLAINED
        .iter()
        .map(|&(path, ..)| path)
        .filter(|path| !differ.contains_key(*path))
        .collect();
    assert!(stale.is_empty(), "no longer differ: {stale:?}");
}
