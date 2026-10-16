//! sheaf: a PDF read lazily (SHEAF.md): [`LazyPdf`], the bytes it reads ([`ByteSource`]) and its
//! caps ([`ReadLimits`]).

use std::borrow::Cow;
use std::cell::Cell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io;
use std::ops::Range;
use std::sync::{Arc, Mutex, OnceLock};

#[cfg(feature = "rayon")]
use rayon::prelude::*;

use crate::encryption::{self, DecryptionError, EncryptionState};
use crate::error::{DecompressError, ParseError};
use crate::object_stream::ObjectStream;
use crate::parser::{self, Resolve};
use crate::reader::{
    Resolved, XrefReader, decryption, encrypts, header_offset, length_value, normal_offsets, object_bound,
};
use crate::xref::{Xref, XrefEntry, XrefType};
use crate::{Dictionary, Document, Error, Limit, Object, ObjectId, Result, Stream};

/// A file's bytes, read by range.
#[allow(clippy::len_without_is_empty, reason = "nothing reads a source that holds no bytes")]
pub trait ByteSource: Send + Sync {
    fn len(&self) -> usize;

    /// The bytes in `range`, which is within `0..len()`.
    fn read_at(&self, range: Range<usize>) -> io::Result<Cow<'_, [u8]>>;

    /// How many bytes a parse at an offset reads first. It reads twice as many each time it
    /// needs more, so this changes only what a read costs. In memory, all of them.
    fn first_window(&self) -> usize {
        usize::MAX
    }
}

impl ByteSource for [u8] {
    fn len(&self) -> usize {
        <[u8]>::len(self)
    }

    fn read_at(&self, range: Range<usize>) -> io::Result<Cow<'_, [u8]>> {
        Ok(Cow::Borrowed(&self[range]))
    }
}

/// Bytes in memory: an object read sees all of them from its offset on, as the eager reader
/// parses.
impl ByteSource for Arc<[u8]> {
    fn len(&self) -> usize {
        <[u8]>::len(self)
    }

    fn read_at(&self, range: Range<usize>) -> io::Result<Cow<'_, [u8]>> {
        Ok(Cow::Borrowed(&self[range]))
    }
}

/// A file read by position, never all at once (unless the file must be rebuilt). It reads what
/// bytes in memory would give: a parse that fails, or reads to the end of what it was given,
/// is given twice as much, up to the end of the file.
pub struct FileBytes {
    file: File,
    len: usize,
}

impl FileBytes {
    /// What a parse reads first.
    const WINDOW: usize = 4096;

    /// `file`, as long as it is now.
    pub fn new(file: File) -> io::Result<FileBytes> {
        let len = usize::try_from(file.metadata()?.len()).map_err(io::Error::other)?;
        Ok(FileBytes { file, len })
    }
}

impl ByteSource for FileBytes {
    fn len(&self) -> usize {
        self.len
    }

    fn read_at(&self, range: Range<usize>) -> io::Result<Cow<'_, [u8]>> {
        let mut bytes = vec![0; range.len()];
        read_exact_at(&self.file, &mut bytes, range.start as u64)?;
        Ok(Cow::Owned(bytes))
    }

    fn first_window(&self) -> usize {
        Self::WINDOW
    }
}

#[cfg(unix)]
fn read_exact_at(file: &File, bytes: &mut [u8], offset: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, bytes, offset)
}

#[cfg(windows)]
fn read_exact_at(file: &File, mut bytes: &mut [u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !bytes.is_empty() {
        match file.seek_read(bytes, offset) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => {
                bytes = &mut bytes[n..];
                offset += n as u64;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// `parse` over `bytes` from `start` to the end. In memory it's given all of them; from a file,
/// a window of at least `min` bytes that doubles, up to the end, while the parse fails but for
/// a broken cap ([`Error::Limit`]). `parse`
/// is told whether its window ends the file, so it can fail a parse that reached the window's
/// end where more bytes could change it.
pub(crate) fn parse_from<S: ByteSource + ?Sized, T>(
    bytes: &S, start: usize, min: usize, mut parse: impl FnMut(&[u8], bool) -> Result<T>,
) -> Result<T> {
    let len = bytes.len();
    if start > len {
        return Err(Error::InvalidOffset(start));
    }
    let mut window = bytes.first_window().max(min);
    loop {
        let end = start.saturating_add(window).min(len);
        let last = end == len;
        match parse(&bytes.read_at(start..end)?, last) {
            // More bytes never mend a broken cap.
            Err(e) if !last && !matches!(e, Error::Limit(_)) => window = window.saturating_mul(2),
            parsed => return parsed,
        }
    }
}

/// A decompression bomb as the cap it breaks.
pub(crate) fn as_limit(e: Error) -> Error {
    match e {
        Error::Decompress(DecompressError::MemoryLimitExceeded { .. }) => Error::Limit(Limit::Decompressed),
        e => e,
    }
}

/// The caps on what [`LazyPdf`] reads. Breaking one is [`Error::Limit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadLimits {
    /// Bytes an object stream or a cross-reference stream may decompress to.
    pub max_decompressed: usize,
    /// Cross-reference sections in the `/Prev` chain.
    pub max_sections: usize,
    /// Object numbers: the cross-reference table's size (ISO 32000-1 Annex C's 8,388,607
    /// indirect objects). A cross-reference stream lists none past it, and the streams in the
    /// `/Prev` chain list twice as many entries at most, each stream refused from its dictionary
    /// before its data is decompressed.
    pub max_objects: u32,
    /// Objects on one read's resolution path (a stream's indirect `/Length`, an object stream
    /// that holds one), and references in one chain.
    pub max_depth: usize,
}

impl ReadLimits {
    pub const DEFAULT: ReadLimits = ReadLimits {
        max_decompressed: 256 << 20,
        max_sections: 1024,
        max_objects: 8_388_607,
        max_depth: 64,
    };
}

impl Default for ReadLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// How a cross-reference section is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XrefKind {
    Table,
    Stream,
}

/// What a file's cross-reference sections say, for writing after them. Offsets are as the file
/// gives them: from its header, at [`XrefFacts::header_offset`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XrefFacts {
    /// The newest section's kind.
    pub kind: XrefKind,
    /// Where the newest section starts, as `startxref` gives it, corrected when it's slightly off
    /// as readers do. 0 when the table was rebuilt.
    pub start: usize,
    /// The trailer's `/Size` (0 when it has none). Object numbers in use can reach past it.
    pub size: u32,
    /// The largest number of a cross-reference stream read: a section written later numbers
    /// above it as well as above `size`.
    pub max_stream: Option<u32>,
    /// The trailer's `/ID`.
    pub id: Option<[Vec<u8>; 2]>,
    /// Where the header, `%PDF-`, starts: bytes before it are junk some writers prepend.
    pub header_offset: usize,
    /// Whether a section has a cross-reference stream at `/XRefStm`.
    pub hybrid: bool,
    /// Whether the file says it's linearized: `/Linearized` within 1,024 bytes of the header.
    pub linearized: bool,
    /// The sections in the `/Prev` chain.
    pub sections: usize,
}

/// A PDF read lazily. [`LazyPdf::open`] reads the header, the cross-reference sections and the
/// trailer, and authenticates; each object is read, and decrypted, when it's first asked for,
/// then kept, and so is a failure to read it that the bytes alone decide. The bytes come from a [`ByteSource`]: in memory, or a file read by position. Every
/// read runs under one set of caps, [`ReadLimits`], so a hostile file is a typed error
/// ([`Error::Limit`]), never a panic, a hang or memory without bound.
///
/// It reads what the eager [`crate::Reader`] reads, with the code it shares: the parser, the
/// cross-reference sections, the bound on an object's bytes, the password and the decryption,
/// and object streams. Where they part:
/// - `/Encrypt` stays in the trailer, and objects are decrypted one at a time.
/// - Only what the cross-reference table lists is there. The eager reader also keeps an object
///   stream's members the table doesn't list, and an object under the number its header gives.
/// - A cycle of indirect `/Length`s is an error ([`Error::ReferenceCycle`]), where the eager
///   reader leaves the streams on it empty, and so is an object that doesn't decrypt, which the
///   eager reader keeps decrypted up to where it failed.
/// - When the sections can't be read, the table is rebuilt by scanning as the eager reader does,
///   and every object is read at once, an object stream's members included:
///   [`LazyPdf::base_sound`] is false.
pub struct LazyPdf {
    bytes: Box<dyn ByteSource>,
    limits: ReadLimits,
    version: String,
    trailer: Dictionary,
    xref: Xref,
    facts: XrefFacts,
    sound: bool,
    /// For [`object_bound`].
    normal_offsets: Vec<usize>,
    crypt: Option<EncryptionState>,
    /// The `/Encrypt` dictionary, which isn't decrypted.
    encrypt: Option<ObjectId>,
    /// What reading each object gave, a slot per object number, in chunks made when first used.
    slots: Box<[OnceLock<Chunk>]>,
    /// The object streams decoded, by number. Their members are in `slots`.
    unpacked: Mutex<HashMap<u32, Unpacked>>,
}

/// Slots per chunk.
const CHUNK: usize = 1024;

type Chunk = Box<[OnceLock<Kept>]>;

/// What a read of an object kept: the object, or why it can't be read when the bytes alone
/// decide that, so reading it again fails at once.
type Kept = std::result::Result<Arc<Object>, Failure>;

/// A failed read that's kept: one the bytes alone decide, never a [`Fatal`] one or an I/O error.
enum Failure {
    Offset(usize),
    Object(usize),
    Mismatch,
    CipherText,
    Padding,
    CryptFilter(Vec<u8>),
}

impl Failure {
    /// `e`, when it's one to keep.
    fn of(e: &Error) -> Option<Failure> {
        Some(match e {
            Error::InvalidOffset(offset) => Failure::Offset(*offset),
            Error::IndirectObject { offset } => Failure::Object(*offset),
            Error::ObjectIdMismatch => Failure::Mismatch,
            Error::Decryption(DecryptionError::InvalidCipherTextLength) => Failure::CipherText,
            Error::Decryption(DecryptionError::Padding) => Failure::Padding,
            Error::Decryption(DecryptionError::UndefinedCryptFilter(name)) => Failure::CryptFilter(name.clone()),
            _ => return None,
        })
    }

    /// The error the read gave.
    fn error(&self) -> Error {
        match self {
            Failure::Offset(offset) => Error::InvalidOffset(*offset),
            Failure::Object(offset) => Error::IndirectObject { offset: *offset },
            Failure::Mismatch => Error::ObjectIdMismatch,
            Failure::CipherText => Error::Decryption(DecryptionError::InvalidCipherTextLength),
            Failure::Padding => Error::Decryption(DecryptionError::Padding),
            Failure::CryptFilter(name) => Error::Decryption(DecryptionError::UndefinedCryptFilter(name.clone())),
        }
    }
}

/// What `slot` keeps, as a read gives it.
fn kept(slot: &Kept) -> Result<&Arc<Object>> {
    slot.as_ref().map_err(Failure::error)
}

/// What became of an object stream.
#[derive(Clone, Copy)]
enum Unpacked {
    /// Its members are in their slots.
    Done,
    /// It didn't decode.
    Failed,
    /// Decompressing it breaks a cap.
    Limit(Limit),
}

/// A failure that fails a whole read, and keeps what it read on the way from being memoized:
/// a cycle, which depends on where the read started, or a cap broken.
#[derive(Clone, Copy)]
enum Fatal {
    Cycle(ObjectId),
    Limit(Limit),
}

impl Fatal {
    fn error(self) -> Error {
        match self {
            Fatal::Cycle(id) => Error::ReferenceCycle(id),
            Fatal::Limit(limit) => Error::Limit(limit),
        }
    }
}

/// One read: the parser's resolver, and the first [`Fatal`] failure on the way. The parser
/// takes a stream whose `/Length` it can't read as having none, so a failure there would
/// otherwise be lost.
struct Cx<'p> {
    pdf: &'p LazyPdf,
    fatal: Cell<Option<Fatal>>,
}

impl Cx<'_> {
    fn fail(&self, fatal: Fatal) -> Error {
        if self.fatal.get().is_none() {
            self.fatal.set(Some(fatal));
        }
        fatal.error()
    }
}

impl Resolve for Cx<'_> {
    fn strict(&self) -> bool {
        false
    }

    fn max_decompressed_size(&self) -> Option<usize> {
        Some(self.pdf.limits.max_decompressed)
    }

    fn length(&self, id: ObjectId, already_seen: &mut HashSet<ObjectId>) -> Result<i64> {
        self.pdf.resolve(id, already_seen, self)?.as_i64()
    }
}

/// The bytes from the header on, which the file's offsets count from.
struct Body<'a> {
    bytes: &'a dyn ByteSource,
    header: usize,
}

impl ByteSource for Body<'_> {
    fn len(&self) -> usize {
        self.bytes.len() - self.header
    }

    fn read_at(&self, range: Range<usize>) -> io::Result<Cow<'_, [u8]>> {
        self.bytes.read_at(range.start + self.header..range.end + self.header)
    }

    fn first_window(&self) -> usize {
        self.bytes.first_window()
    }
}

impl LazyPdf {
    /// Reads the header, the cross-reference sections and the trailer, then authenticates the
    /// empty password, then `password`: a wrong one is [`Error::InvalidPassword`], and so is
    /// none when the empty one doesn't work. When the sections can't be read, the table is
    /// rebuilt by scanning and every object read at once ([`LazyPdf::base_sound`]).
    pub fn open(bytes: impl ByteSource + 'static, password: Option<&str>, limits: &ReadLimits) -> Result<LazyPdf> {
        let bytes: Box<dyn ByteSource> = Box::new(bytes);
        let header = header_offset(&*bytes)?;
        let body = Body { bytes: &*bytes, header };
        let version = parse_from(&body, 0, 0, |head, _| {
            parser::header(head, false).ok_or(Error::Parse(ParseError::InvalidFileHeader))
        })?;
        let sections = XrefReader {
            bytes: &body,
            strict: false,
            max_decompressed_size: Some(limits.max_decompressed),
            limits: Some(limits),
        };
        let (resolved, sound) = match sections.resolve() {
            Ok(resolved) => (resolved, true),
            Err(e @ Error::Limit(_)) => return Err(e),
            Err(e) => match sections.reconstruct() {
                Some((xref, trailer)) => (
                    Resolved {
                        xref,
                        trailer,
                        start: 0,
                        kind: XrefType::CrossReferenceTable,
                        max_stream: None,
                        hybrid: false,
                        sections: 0,
                    },
                    false,
                ),
                None => return Err(e),
            },
        };
        if resolved.xref.size > limits.max_objects {
            return Err(Error::Limit(Limit::Objects));
        }
        let head = body.read_at(0..body.len().min(1024))?;
        let linearized = head.windows(11).any(|w| w == b"/Linearized");
        let trailer = resolved.trailer;
        let facts = XrefFacts {
            kind: match resolved.kind {
                XrefType::CrossReferenceTable => XrefKind::Table,
                XrefType::CrossReferenceStream => XrefKind::Stream,
            },
            start: resolved.start,
            size: trailer
                .get(b"Size")
                .and_then(Object::as_i64)
                .ok()
                .and_then(|size| u32::try_from(size).ok())
                .unwrap_or(0),
            max_stream: resolved.max_stream,
            id: file_id(&trailer),
            header_offset: header,
            hybrid: resolved.hybrid,
            linearized,
            sections: resolved.sections,
        };
        let mut pdf = LazyPdf {
            limits: *limits,
            version,
            normal_offsets: normal_offsets(&resolved.xref),
            slots: empty_slots(resolved.xref.size),
            xref: resolved.xref,
            trailer,
            facts,
            sound,
            crypt: None,
            encrypt: None,
            unpacked: Mutex::default(),
            bytes,
        };
        pdf.authenticate(password)?;
        if !sound {
            pdf.read_all();
        }
        Ok(pdf)
    }

    /// Whether the cross-reference sections were read as the file wrote them. When false, the
    /// table was rebuilt by scanning, and a section written after this file's would describe
    /// another file than the one readers see.
    pub fn base_sound(&self) -> bool {
        self.sound
    }

    pub fn xref(&self) -> &XrefFacts {
        &self.facts
    }

    /// The header's version.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// The newest trailer, without `/Prev` and `/XRefStm`, and with `/Encrypt`.
    pub fn trailer(&self) -> &Dictionary {
        &self.trailer
    }

    /// Whether the file is encrypted: its trailer has an `/Encrypt` that isn't null, and the
    /// open authenticated.
    pub fn is_encrypted(&self) -> bool {
        self.crypt.is_some()
    }

    /// The objects the cross-reference table lists as in use, by number.
    pub fn ids(&self) -> impl Iterator<Item = ObjectId> + '_ {
        self.xref.entries.iter().filter_map(|(&number, entry)| match *entry {
            XrefEntry::Normal { generation, .. } => Some((number, generation)),
            XrefEntry::Compressed { .. } => Some((number, 0)),
            XrefEntry::Free | XrefEntry::UnusableFree => None,
        })
    }

    pub fn catalog(&self) -> Result<&Dictionary> {
        self.trailer
            .get(b"Root")
            .and_then(Object::as_reference)
            .and_then(|id| self.get_dictionary(id))
    }

    /// The object `id`, read and decrypted the first time it's asked for. A stream's data is as
    /// the file has it, filters and all.
    pub fn get(&self, id: ObjectId) -> Result<&Object> {
        self.read(id).map(|object| &**object)
    }

    /// [`LazyPdf::get`], to keep.
    pub fn object(&self, id: ObjectId) -> Result<Arc<Object>> {
        self.read(id).cloned()
    }

    /// The object `id`, following references, as [`Document::get_object`] does.
    pub fn get_object(&self, id: ObjectId) -> Result<&Object> {
        self.dereference(self.get(id)?).map(|(_, object)| object)
    }

    pub fn get_dictionary(&self, id: ObjectId) -> Result<&Dictionary> {
        self.get_object(id).and_then(Object::as_dict)
    }

    /// `object`, or what its chain of references ends at, with the last reference's id.
    pub fn dereference<'a>(&'a self, object: &'a Object) -> Result<(Option<ObjectId>, &'a Object)> {
        let mut object = object;
        let mut id = None;
        let mut chain = 0;
        while let Ok(next) = object.as_reference() {
            chain += 1;
            if chain > self.limits.max_depth {
                return Err(Error::Limit(Limit::Depth));
            }
            id = Some(next);
            object = self.get(next)?;
        }
        Ok((id, object))
    }

    /// Whether a read of the object `id` was kept: it was asked for, or is in an object stream
    /// that was decoded, and it read, or failed for a reason the bytes alone decide. A reader's
    /// tests can tell from it what the reader read.
    pub fn is_read(&self, id: ObjectId) -> bool {
        let number = id.0 as usize;
        self.slots
            .get(number / CHUNK)
            .and_then(OnceLock::get)
            .is_some_and(|chunk| chunk[number % CHUNK].get().is_some())
    }

    /// Where the object `id` is in the bytes, from its header to after `endobj`. None for an
    /// object stream's member, for an object that can't be read, and for a stream whose data
    /// has no length to end by, or no `endstream` after it.
    pub fn raw_span(&self, id: ObjectId) -> Option<Range<usize>> {
        let XrefEntry::Normal { offset, generation } = *self.xref.get(id.0)? else {
            return None;
        };
        if generation != id.1 {
            return None;
        }
        self.get(id).ok()?;
        let (offset, cx, mut path) = (offset as usize, self.cx(), HashSet::new());
        let parsed = self.parse_at(id, offset, &mut path, &cx).ok()?;
        let end = match &parsed.object {
            // The parser left the data to `read_content`: the object ends after it.
            Object::Stream(Stream {
                dict,
                start_position: Some(data),
                ..
            }) => {
                let data_end = self.content_end(dict, offset + data, &mut path, &cx).ok()??;
                data_end + self.stream_end(data_end)? - offset
            }
            _ => parsed.end,
        };
        let start = offset + self.facts.header_offset;
        Some(start..start + end)
    }

    fn body(&self) -> Body<'_> {
        Body {
            bytes: &*self.bytes,
            header: self.facts.header_offset,
        }
    }

    fn cx(&self) -> Cx<'_> {
        Cx {
            pdf: self,
            fatal: Cell::new(None),
        }
    }

    fn read(&self, id: ObjectId) -> Result<&Arc<Object>> {
        let cx = self.cx();
        let object = self.resolve(id, &mut HashSet::new(), &cx);
        match cx.fatal.get() {
            Some(fatal) => Err(fatal.error()),
            None => object,
        }
    }

    /// The object `id`, memoized, on the resolution path `path`. A failure the bytes alone
    /// decide is memoized too: a broken object that every page names is parsed once.
    fn resolve(&self, id: ObjectId, path: &mut HashSet<ObjectId>, cx: &Cx) -> Result<&Arc<Object>> {
        let entry = self.xref.get(id.0).ok_or(Error::MissingXrefEntry)?;
        let slot = self.slot(id.0)?;
        match *entry {
            XrefEntry::Normal { offset, generation } if generation == id.1 => {
                if let Some(read) = slot.get() {
                    return kept(read);
                }
                // Two threads may have read it: the first one kept wins.
                match self.enter(id, path, cx, |path| self.parse(id, offset as usize, path, cx)) {
                    Ok(object) => kept(slot.get_or_init(|| Ok(Arc::new(object)))),
                    Err(e) => match Failure::of(&e) {
                        Some(failure) => kept(slot.get_or_init(|| Err(failure))),
                        None => Err(e),
                    },
                }
            }
            XrefEntry::Compressed { container, .. } if id.1 == 0 => {
                if let Some(read) = slot.get() {
                    return kept(read);
                }
                self.enter(id, path, cx, |path| self.unpack(container, path, cx))?;
                slot.get().ok_or(Error::MissingXrefEntry).and_then(kept)
            }
            _ => Err(Error::MissingXrefEntry),
        }
    }

    /// `read` with `id` on the resolution path. A cycle, a path deeper than the cap, or such a
    /// failure further on fails it, and once one has, every read after it fails at once: what
    /// the parser makes of a failed `/Length` would read again what failed.
    fn enter<T>(
        &self, id: ObjectId, path: &mut HashSet<ObjectId>, cx: &Cx,
        read: impl FnOnce(&mut HashSet<ObjectId>) -> Result<T>,
    ) -> Result<T> {
        if let Some(fatal) = cx.fatal.get() {
            return Err(fatal.error());
        }
        if path.contains(&id) {
            return Err(cx.fail(Fatal::Cycle(id)));
        }
        if path.len() >= self.limits.max_depth {
            return Err(cx.fail(Fatal::Limit(Limit::Depth)));
        }
        path.insert(id);
        let read = read(path);
        path.remove(&id);
        match cx.fatal.get() {
            Some(fatal) => Err(fatal.error()),
            None => read,
        }
    }

    /// The object at `offset`, decrypted.
    fn parse(&self, id: ObjectId, offset: usize, path: &mut HashSet<ObjectId>, cx: &Cx) -> Result<Object> {
        let mut object = self.parse_at(id, offset, path, cx)?.object;
        if let Object::Stream(stream) = &mut object
            && let Some(start) = stream.start_position
        {
            let start = offset + start;
            if stream.content.is_empty() {
                self.read_content(stream, start, path, cx)?;
            }
            stream.start_position = Some(start + self.facts.header_offset);
        }
        if let Some(crypt) = &self.crypt
            && self.encrypt != Some(id)
        {
            encryption::decrypt_object(crypt, id, &mut object).map_err(Error::Decryption)?;
        }
        Ok(object)
    }

    /// The object at `offset` as the parser reads it, its stream's position from `offset`. It's
    /// given at least a byte past where it can end, so a well-formed object takes one read.
    fn parse_at(&self, id: ObjectId, offset: usize, path: &mut HashSet<ObjectId>, cx: &Cx) -> Result<parser::Parsed> {
        let body = self.body();
        let bound = object_bound(&self.normal_offsets, self.facts.start, body.len(), offset);
        if offset > bound {
            return Err(Error::InvalidOffset(offset));
        }
        parse_from(&body, offset, bound - offset + 1, |window, last| {
            let parsed = parser::indirect_object_in(window, 0, Some(id), cx, path, Some(bound - offset)).map_err(
                |e| match e {
                    Error::IndirectObject { .. } => Error::IndirectObject { offset },
                    e => e,
                },
            )?;
            if parsed.rest == 0 && !last {
                return Err(Error::Parse(ParseError::EndOfInput));
            }
            Ok(parsed)
        })
    }

    /// The data of a stream whose `/Length` the parser couldn't read, from `start` in the body,
    /// as the eager reader fills it in after loading. Left empty when [`LazyPdf::content_end`]
    /// finds no end.
    fn read_content(&self, stream: &mut Stream, start: usize, path: &mut HashSet<ObjectId>, cx: &Cx) -> Result<()> {
        if let Some(end) = self.content_end(&stream.dict, start, path, cx)? {
            stream.set_content(self.body().read_at(start..end)?.into_owned());
        }
        Ok(())
    }

    /// Where the data from `start` in the body of a stream with `dict` ends, by a `/Length` that
    /// is a chain of references, or an integral real, as the eager reader reads it. None when
    /// there's none, or it ends past the bytes.
    fn content_end(
        &self, dict: &Dictionary, start: usize, path: &mut HashSet<ObjectId>, cx: &Cx,
    ) -> Result<Option<usize>> {
        let Ok(mut length) = dict.get(b"Length") else {
            return Ok(None);
        };
        let mut chain = 0;
        while let Ok(id) = length.as_reference() {
            chain += 1;
            if chain > self.limits.max_depth {
                return Err(cx.fail(Fatal::Limit(Limit::Depth)));
            }
            length = match self.resolve(id, path, cx) {
                Ok(length) => length,
                Err(_) => return Ok(None),
            };
        }
        Ok(length_value(length)
            .ok()
            .and_then(|length| usize::try_from(length).ok())
            .and_then(|length| start.checked_add(length))
            .filter(|&end| end <= self.body().len()))
    }

    /// How far a stream's `endstream`, and its object's `endobj`, reach past its data's end,
    /// `data_end` in the body. None when `endstream` isn't there.
    fn stream_end(&self, data_end: usize) -> Option<usize> {
        // An end of line and the keyword: fewer bytes can't tell.
        const KEYWORD: usize = b"\r\nendstream".len();
        parse_from(&self.body(), data_end, 0, |input, last| {
            match parser::stream_end(input) {
                Some(end) if end < input.len() || last => Ok(Some(end)),
                None if input.len() >= KEYWORD || last => Ok(None),
                _ => Err(Error::Parse(ParseError::EndOfInput)),
            }
        })
        .ok()?
    }

    /// Decodes object stream `container` once, putting its members in their slots: those the
    /// table says it holds.
    fn unpack(&self, container: u32, path: &mut HashSet<ObjectId>, cx: &Cx) -> Result<()> {
        let done = self.unpacked.lock().unwrap().get(&container).copied();
        let decoded = match done {
            Some(done) => return self.unpacked_result(container, done, cx),
            None => self
                .resolve((container, 0), path, cx)
                .and_then(|object| {
                    ObjectStream::new_with_limit(object.as_stream()?, Some(self.limits.max_decompressed))
                })
                .map_err(as_limit),
        };
        // A cycle or a path too deep depends on where this read started: another may decode it.
        if cx.fatal.get().is_some() {
            return decoded.map(|_| ());
        }
        let (done, result) = match decoded {
            Ok(stream) => {
                for (member, object) in stream.objects {
                    let listed = matches!(
                        self.xref.get(member.0),
                        Some(XrefEntry::Compressed { container: c, .. }) if *c == container
                    );
                    if listed && member.1 == 0 {
                        let _ = self.slot(member.0)?.set(Ok(Arc::new(object)));
                    }
                }
                (Unpacked::Done, Ok(()))
            }
            Err(Error::Limit(limit)) => (Unpacked::Limit(limit), Err(cx.fail(Fatal::Limit(limit)))),
            Err(e) => (Unpacked::Failed, Err(e)),
        };
        self.unpacked.lock().unwrap().insert(container, done);
        result
    }

    fn unpacked_result(&self, container: u32, done: Unpacked, cx: &Cx) -> Result<()> {
        match done {
            Unpacked::Done => Ok(()),
            Unpacked::Failed => Err(Error::InvalidObjectStream(format!(
                "object stream {container} 0 R failed to decode"
            ))),
            Unpacked::Limit(limit) => Err(cx.fail(Fatal::Limit(limit))),
        }
    }

    fn slot(&self, number: u32) -> Result<&OnceLock<Kept>> {
        let number = number as usize;
        let chunk = self.slots.get(number / CHUNK).ok_or(Error::MissingXrefEntry)?;
        Ok(&chunk.get_or_init(|| (0..CHUNK).map(|_| OnceLock::new()).collect())[number % CHUNK])
    }

    /// Reads `/Encrypt` and authenticates, keeping the decryption: [`decryption`]'s choice of
    /// password, as the eager reader makes it.
    fn authenticate(&mut self, password: Option<&str>) -> Result<()> {
        if !encrypts(&self.trailer) {
            return Ok(());
        }
        let mut document = Document::new();
        document.trailer = self.trailer.clone();
        let encrypt = self.trailer.get(b"Encrypt").and_then(Object::as_reference).ok();
        if let Some(id) = encrypt {
            document.objects.insert(id, self.get(id)?.clone());
        }
        let crypt = decryption(&document, password)?.ok_or(Error::InvalidPassword)?;
        self.crypt = Some(crypt);
        self.encrypt = encrypt;
        // What was read on the way wasn't decrypted.
        self.slots = empty_slots(self.xref.size);
        self.unpacked = Mutex::default();
        Ok(())
    }

    /// Every object, as the eager reader loads a file whose table it rebuilt: the objects the
    /// scan found, then the members of their object streams, which the scan can't list. A
    /// member is listed by the last object stream in the file that holds it, as incremental
    /// updates append, unless the scan found its number.
    fn read_all(&mut self) {
        let mut found: Vec<(u32, ObjectId)> = self
            .xref
            .entries
            .iter()
            .filter_map(|(&number, entry)| match *entry {
                XrefEntry::Normal { offset, generation } => Some((offset, (number, generation))),
                _ => None,
            })
            .collect();
        found.sort_unstable();
        let ids: Vec<ObjectId> = found.into_iter().map(|(_, id)| id).collect();
        #[cfg(feature = "rayon")]
        ids.par_iter().for_each(|&id| {
            let _ = self.get(id);
        });
        #[cfg(not(feature = "rayon"))]
        for &id in &ids {
            let _ = self.get(id);
        }

        let mut members = BTreeMap::new();
        for &id in ids.iter().rev() {
            let Ok(Object::Stream(stream)) = self.get(id) else {
                continue;
            };
            if !stream.dict.has_type(b"ObjStm") {
                continue;
            }
            let Ok(objects) = ObjectStream::new_with_limit(stream, Some(self.limits.max_decompressed)) else {
                continue;
            };
            for ((number, generation), object) in objects.objects {
                if generation == 0 && number < self.limits.max_objects && !self.xref.entries.contains_key(&number) {
                    members.entry(number).or_insert((id.0, object));
                }
            }
        }
        let Some(&last) = members.keys().next_back() else {
            return;
        };
        self.xref.size = self.xref.size.max(last + 1);
        let mut slots = std::mem::take(&mut self.slots).into_vec();
        slots.resize_with((self.xref.size as usize).div_ceil(CHUNK), OnceLock::new);
        self.slots = slots.into_boxed_slice();
        for (number, (container, object)) in members {
            // The index isn't read: members are found by number.
            self.xref.insert(number, XrefEntry::Compressed { container, index: 0 });
            if let Ok(slot) = self.slot(number) {
                let _ = slot.set(Ok(Arc::new(object)));
            }
        }
    }
}

fn empty_slots(size: u32) -> Box<[OnceLock<Chunk>]> {
    (0..(size as usize).div_ceil(CHUNK)).map(|_| OnceLock::new()).collect()
}

/// The trailer's `/ID`: two strings.
fn file_id(trailer: &Dictionary) -> Option<[Vec<u8>; 2]> {
    match trailer.get(b"ID").and_then(Object::as_array).ok()?.as_slice() {
        [Object::String(first, _), Object::String(second, _)] => Some([first.clone(), second.clone()]),
        _ => None,
    }
}
