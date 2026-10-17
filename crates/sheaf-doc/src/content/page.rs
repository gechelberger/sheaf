//! A page's content as one: the logical concatenation of its `/Contents`
//! streams. A marked range or a `q` may open in one stream and close in
//! the next, so a page's streams are lexed as PDFium reads them, joined
//! with a space between each ([`Concat`]), and what's found is mapped back
//! to a stream and a place in it ([`Pos`], [`SpanAcross`]).
//!
//! [`PageContent`] reads the streams through [`ReadObjects`]: a source,
//! which reads each without keeping it, or a document in memory (the old
//! writers' copies, a stamp collection). A stream that won't decode, or
//! streams that decode past [`ContentLimits::decompressed`] together, are
//! a [`ContentFault`], never skipped and never taken as raw bytes.
//!
//! [`ContentLimits::decompressed`]: crate::source::ContentLimits

use std::borrow::Cow;
use std::ops::Range;

use lopdf::{Dictionary, LazyPdf, Object, ObjectId, Stream};

use super::lex::{self, LexError, ResCx, Tok};
use crate::source::Limits;

/// A place in a page's content: a byte in one of its streams. The end of
/// a stream is the place after its last byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pos {
    pub stream: u32,
    pub at: u32,
}

/// Part of a page's content, which may start in one stream and end in
/// another: from `start` up to `end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpanAcross {
    pub start: Pos,
    pub end: Pos,
}

impl std::fmt::Display for Pos {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "stream {} byte {}", self.stream, self.at)
    }
}

impl std::fmt::Display for SpanAcross {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{} to {}", self.start, self.end)
    }
}

/// Streams as one: their bytes joined, a space between each, as PDFium
/// joins a page's. A token can still run from one into the next, as in
/// PDFium, when a string or an inline image does. Offsets are into the
/// joined bytes; streams are at most 4 GiB together, as a page's content
/// is under its cap.
#[derive(Debug, Clone)]
pub struct Concat<'a> {
    bytes: Cow<'a, [u8]>,
    /// Where each stream starts.
    starts: Box<[usize]>,
}

impl<'a> Concat<'a> {
    /// `streams` as one; one stream is borrowed as it is.
    pub fn new(streams: &[&'a [u8]]) -> Concat<'a> {
        if let [one] = streams {
            return Concat {
                bytes: Cow::Borrowed(one),
                starts: Box::new([0]),
            };
        }
        let mut join = Join::default();
        for stream in streams {
            join.push(stream.to_vec());
        }
        join.done()
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// How many streams there are.
    pub fn stream_count(&self) -> usize {
        self.starts.len()
    }

    /// Stream `i`'s bytes.
    pub fn stream(&self, i: usize) -> &[u8] {
        &self.bytes[self.range(i)]
    }

    /// Where stream `i` is in the joined bytes.
    pub fn range(&self, i: usize) -> Range<usize> {
        let end = self
            .starts
            .get(i + 1)
            .map_or(self.bytes.len(), |next| next - 1);
        self.starts[i]..end
    }

    /// Where offset `at` is: past a stream's last byte, the space after it
    /// is that stream's end.
    pub fn pos(&self, at: usize) -> Pos {
        let stream = self
            .starts
            .partition_point(|&start| start <= at)
            .saturating_sub(1);
        let start = self.starts.get(stream).copied().unwrap_or(0);
        Pos {
            stream: small(stream),
            at: small(at - start),
        }
    }

    /// Where `pos` is in the joined bytes; None past its stream's end, or
    /// past the last stream.
    pub fn offset(&self, pos: Pos) -> Option<usize> {
        let stream = pos.stream as usize;
        let range = (stream < self.starts.len()).then(|| self.range(stream))?;
        let at = range.start + pos.at as usize;
        (at <= range.end).then_some(at)
    }

    /// Where `span` is in the joined bytes; None when either end is
    /// nowhere, or the end is before the start.
    pub fn offsets(&self, span: SpanAcross) -> Option<Range<usize>> {
        let (start, end) = (self.offset(span.start)?, self.offset(span.end)?);
        (start <= end).then_some(start..end)
    }

    /// Where `range` is.
    pub fn span(&self, range: Range<usize>) -> SpanAcross {
        SpanAcross {
            start: self.pos(range.start),
            end: self.pos(range.end),
        }
    }

    /// The joined bytes' tokens ([`lex::lex`]), at offsets into them.
    pub fn lex<'s>(
        &'s self,
        res: &ResCx,
        lim: &Limits,
    ) -> impl Iterator<Item = Result<(Range<usize>, Tok<'s>), LexError>> {
        lex::lex(&self.bytes, res, lim)
    }
}

fn small(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Streams joined as they're read.
#[derive(Default)]
struct Join {
    bytes: Vec<u8>,
    starts: Vec<usize>,
}

impl Join {
    fn push(&mut self, stream: Vec<u8>) {
        if self.starts.is_empty() {
            self.starts.push(0);
            self.bytes = stream;
        } else {
            self.bytes.push(b' ');
            self.starts.push(self.bytes.len());
            self.bytes.extend_from_slice(&stream);
        }
    }

    /// What `len` more bytes would make the joined bytes, a space before
    /// them.
    fn with(&self, len: usize) -> usize {
        self.bytes.len() + usize::from(!self.starts.is_empty()) + len
    }

    fn done(self) -> Concat<'static> {
        Concat {
            bytes: Cow::Owned(self.bytes),
            starts: self.starts.into(),
        }
    }
}

/// What a page's content is read from: a source, or a document in memory.
/// The toolkit's own, so it outlives the eager documents.
pub trait ReadObjects {
    /// Object `id`, its references followed, read once and kept where the
    /// reader keeps what it reads: resources, which pages share.
    fn object(&self, id: ObjectId) -> lopdf::Result<&Object>;

    /// Object `id`, its references followed, handed to `read`; a source
    /// reads it without keeping it: a page's streams, each read once to
    /// lex.
    fn read<R>(&self, id: ObjectId, read: impl FnOnce(&Object) -> R) -> lopdf::Result<R>;
}

impl ReadObjects for LazyPdf {
    fn object(&self, id: ObjectId) -> lopdf::Result<&Object> {
        self.get_object(id)
    }

    fn read<R>(&self, id: ObjectId, read: impl FnOnce(&Object) -> R) -> lopdf::Result<R> {
        let mut object = self.object_unkept(id)?;
        let mut chain = 0;
        while let Object::Reference(next) = *object {
            chain += 1;
            if chain > Limits::DEFAULT.reader.max_depth {
                return Err(lopdf::Error::Limit(lopdf::Limit::Depth));
            }
            object = self.object_unkept(next)?;
        }
        Ok(read(&object))
    }
}

impl ReadObjects for lopdf::Document {
    fn object(&self, id: ObjectId) -> lopdf::Result<&Object> {
        self.get_object(id)
    }

    fn read<R>(&self, id: ObjectId, read: impl FnOnce(&Object) -> R) -> lopdf::Result<R> {
        self.get_object(id).map(read)
    }
}

/// Why a page's content can't be read.
#[derive(Debug, thiserror::Error)]
pub enum ContentFault {
    #[error("stream {} {} R can't be read: {error}", .stream.0, .stream.1)]
    Stream {
        stream: ObjectId,
        error: lopdf::Error,
    },
    #[error("its streams decode to more than {0} bytes")]
    TooLarge(usize),
    /// `error`'s offset is into `stream`.
    #[error("stream {} {} R: {error}", .stream.0, .stream.1)]
    Lex { stream: ObjectId, error: LexError },
}

/// A page's content: its streams, decoded, as one.
#[derive(Debug, Clone)]
pub struct PageContent {
    /// Each stream's object, in order.
    ids: Box<[ObjectId]>,
    /// Each stream's dictionary: what marks it as one of sheaf's.
    dicts: Box<[Dictionary]>,
    concat: Concat<'static>,
}

impl PageContent {
    /// The streams `contents` names, decoded: what [`PageFacts::contents`]
    /// holds, where a reference to an array is that array's streams. What
    /// isn't a stream (null, a missing object) draws nothing, as in
    /// PDFium.
    ///
    /// [`PageFacts::contents`]: crate::source::PageFacts
    pub fn read(
        objects: &impl ReadObjects,
        contents: &[ObjectId],
        lim: &Limits,
    ) -> Result<PageContent, ContentFault> {
        let cap = lim.content.decompressed;
        let mut ids = Vec::new();
        let mut dicts = Vec::new();
        let mut join = Join::default();
        let mut take = |id: ObjectId, stream: &Stream| -> Result<(), ContentFault> {
            let left = cap.saturating_sub(join.with(0));
            let data = stream
                .decompressed_content_with_limit(left)
                .map_err(|e| match e {
                    lopdf::Error::Decompress(lopdf::DecompressError::MemoryLimitExceeded {
                        ..
                    }) => ContentFault::TooLarge(cap),
                    e => ContentFault::Stream {
                        stream: id,
                        error: e,
                    },
                })?;
            if join.with(data.len()) > cap {
                return Err(ContentFault::TooLarge(cap));
            }
            ids.push(id);
            dicts.push(stream.dict.clone());
            join.push(data);
            Ok(())
        };
        for &id in contents {
            let entries = objects
                .read(id, |object| match object {
                    Object::Stream(stream) => take(id, stream).map(|()| Vec::new()),
                    Object::Array(entries) => Ok(entries
                        .iter()
                        .filter_map(|e| e.as_reference().ok())
                        .collect()),
                    _ => Ok(Vec::new()),
                })
                .or_else(|e| missing(id, e).map(|()| Ok(Vec::new())))??;
            for entry in entries {
                objects
                    .read(entry, |object| match object {
                        Object::Stream(stream) => take(entry, stream),
                        _ => Ok(()),
                    })
                    .or_else(|e| missing(entry, e).map(Ok))??;
            }
        }
        Ok(PageContent {
            ids: ids.into(),
            dicts: dicts.into(),
            concat: join.done(),
        })
    }

    /// Each stream's object, in order.
    pub fn ids(&self) -> &[ObjectId] {
        &self.ids
    }

    /// Each stream's dictionary, in order.
    pub fn dicts(&self) -> &[Dictionary] {
        &self.dicts
    }

    /// The streams, decoded, as one.
    pub fn concat(&self) -> &Concat<'static> {
        &self.concat
    }

    /// The joined bytes, to draw as they are.
    pub fn into_bytes(self) -> Vec<u8> {
        self.concat.bytes.into_owned()
    }

    /// The fault `error`, from lexing [`PageContent::concat`], is: in
    /// which stream, and where in it.
    pub fn fault(&self, error: LexError) -> ContentFault {
        let pos = self.concat.pos(error.at);
        ContentFault::Lex {
            stream: self.ids.get(pos.stream as usize).copied().unwrap_or((0, 0)),
            error: LexError {
                at: pos.at as usize,
                ..error
            },
        }
    }
}

/// A read of `id` that failed: Ok when there's no such object, which
/// reads as null.
fn missing(id: ObjectId, e: lopdf::Error) -> Result<(), ContentFault> {
    match e {
        lopdf::Error::MissingXrefEntry | lopdf::Error::ObjectNotFound(_) => Ok(()),
        error => Err(ContentFault::Stream { stream: id, error }),
    }
}

/// The streams a page's `/Contents` names: one, or a direct array's. A
/// reference to an array stays that one reference, since telling the two
/// apart reads the stream it usually is.
pub fn contents(page: &Dictionary) -> Vec<ObjectId> {
    match page.get(b"Contents") {
        Ok(Object::Reference(id)) => vec![*id],
        Ok(Object::Array(streams)) => streams
            .iter()
            .filter_map(|s| s.as_reference().ok())
            .collect(),
        _ => Vec::new(),
    }
}

impl ResCx {
    /// The named color spaces of `resources`, a page's or a form's
    /// `/Resources` as written.
    pub fn read(objects: &impl ReadObjects, resources: Option<&Object>) -> ResCx {
        let spaces = resources
            .and_then(|r| direct(objects, r))
            .and_then(|r| r.as_dict().ok())
            .and_then(|r| direct(objects, r.get(b"ColorSpace").ok()?))
            .and_then(|s| s.as_dict().ok());
        ResCx::new(spaces.into_iter().flat_map(|spaces| {
            spaces
                .iter()
                .map(|(name, space)| (name.clone(), components(objects, space)))
        }))
    }
}

/// The components a color space in the resources has, as PDFium counts
/// them, 3 where it can't make one out.
fn components(objects: &impl ReadObjects, space: &Object) -> u32 {
    let direct = |o| direct(objects, o);
    let Some(space) = direct(space) else {
        return 3;
    };
    let parts = match space {
        Object::Name(family) => return lex::family_components(family),
        Object::Array(parts) => parts,
        _ => return 3,
    };
    let family = parts
        .first()
        .and_then(direct)
        .and_then(|f| f.as_name().ok());
    let second = || parts.get(1).and_then(direct);
    match family {
        Some(b"ICCBased") => second()
            .and_then(|s| s.as_stream().ok())
            .and_then(|s| s.dict.get(b"N").ok()?.as_i64().ok())
            .filter(|n| matches!(n, 1 | 3 | 4))
            .map_or(3, |n| n as u32),
        Some(b"DeviceN") => second()
            .and_then(|names| names.as_array().ok())
            .map(|names| names.len() as u32)
            .filter(|&n| n > 0)
            .unwrap_or(3),
        Some(family) => lex::family_components(family),
        None => 3,
    }
}

/// `object`, or what the reference it is names.
pub(super) fn direct<'a>(objects: &'a impl ReadObjects, object: &'a Object) -> Option<&'a Object> {
    match object {
        Object::Reference(id) => objects.object(*id).ok(),
        o => Some(o),
    }
}
