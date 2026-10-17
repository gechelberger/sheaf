//! A page's blocks (§13): its content classified by who drew what. The
//! running heads, watermarks and backgrounds marked as pagination
//! artifacts, and which tool drew each; sheaf's own blocks, by the marker
//! on their stream or in their property list; the streams that isolate
//! the page's content; the OCR layer sheaf wrote before M10.5; and the
//! content's `q`/`Q` balance. A source reads a page's once, when first
//! asked ([`Source::blocks`]), never at open.
//!
//! [`Source::blocks`]: crate::source::Source::blocks

use std::collections::{BTreeSet, HashMap};
use std::ops::Range;
use std::sync::{Arc, Mutex, PoisonError};

use lopdf::{Dictionary, Object, ObjectId};

use super::balance::Balance;
use super::lex::{self, LexError, ResCx, Step, Tok};
use super::marked::{self, Event, Marked, Nest, PropertiesDict, Props};
use super::page::{Concat, ContentFault, PageContent, ReadObjects, SpanAcross, direct};
use super::splice::SpliceError;
use crate::source::Limits;

/// What a pagination artifact is, by its `/Subtype`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PaginationKind {
    Header,
    Footer,
    Watermark,
    Background,
}

impl PaginationKind {
    pub const ALL: [PaginationKind; 4] = [
        PaginationKind::Header,
        PaginationKind::Footer,
        PaginationKind::Watermark,
        PaginationKind::Background,
    ];

    /// Its `/Subtype`, which Acrobat's `/Private` names too.
    pub fn name(self) -> &'static [u8] {
        match self {
            PaginationKind::Header => b"Header",
            PaginationKind::Footer => b"Footer",
            PaginationKind::Watermark => b"Watermark",
            PaginationKind::Background => b"Background",
        }
    }

    pub fn from_name(name: &[u8]) -> Option<PaginationKind> {
        PaginationKind::ALL.into_iter().find(|k| k.name() == name)
    }
}

/// Who drew a pagination artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PagOrigin {
    /// sheaf, as a generator's block: in a `/SheafBlock` stream, or with
    /// `/SheafGen` in its list.
    Sheaf,
    /// sheaf before M10.5: `/SheafStamp` or `/SheafSettings` in its list,
    /// drawn in the `/SheafHF` font, or in a `/SheafOcrLayer` stream.
    SheafLegacy,
    /// Acrobat: it draws Form XObjects only, each with `/PieceInfo
    /// /ADBE_CompoundType /Private` a pagination kind.
    Acrobat,
    /// Any other: the author's own running heads, as Word, LibreOffice and
    /// InDesign mark them.
    Authored,
}

/// A generator's id, as sheaf writes it in a block: `(g<hex>)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GenId(pub u64);

/// A layer's id, as sheaf writes it in a block: `(b<hex>)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockId(pub u64);

impl GenId {
    pub fn parse(s: &[u8]) -> Option<GenId> {
        hex_id(s, b'g').map(GenId)
    }
}

impl BlockId {
    pub fn parse(s: &[u8]) -> Option<BlockId> {
        hex_id(s, b'b').map(BlockId)
    }
}

/// `prefix` and up to 16 hex digits, the number they write.
fn hex_id(s: &[u8], prefix: u8) -> Option<u64> {
    let digits = s.strip_prefix(&[prefix])?;
    if digits.is_empty() || digits.len() > 16 || !digits.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    u64::from_str_radix(std::str::from_utf8(digits).ok()?, 16).ok()
}

/// A block of sheaf's (§11.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SheafBlock {
    /// A generator's output on the page.
    Gen(GenId),
    /// An OCR layer.
    Ocr(BlockId),
    /// Placements flattened into the page.
    Flat(BlockId),
}

/// Where a block of sheaf's is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockAt {
    /// A stream of its own, marked `/SheafBlock`.
    Stream(ObjectId),
    /// A marked range in a stream another tool merged it into.
    Marked(SpanAcross),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SheafBlockRef {
    pub block: SheafBlock,
    pub at: BlockAt,
}

/// The OCR layer sheaf wrote before M10.5: a stream of `q` before the
/// page's content, and the layer after it, which starts with `Q`. Both are
/// marked `/SheafOcrLayer`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LegacyOcr {
    pub q: ObjectId,
    pub layer: ObjectId,
}

/// A pagination artifact: an `/Artifact` range whose `/Subtype` is a
/// pagination kind, outside any other. Acrobat, Word and sheaf write
/// `/Type /Pagination` with it; without a `/Subtype` there's no kind to
/// remove it by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Pagination {
    pub kind: PaginationKind,
    pub origin: PagOrigin,
    pub span: SpanAcross,
}

impl Pagination {
    /// What to take out of `content`, lexed with `res`, to remove it: its
    /// span, but for a run of `Q`s right after its `BDC`, which end a `q`
    /// from before it and stay. Acrobat opens its first artifact on a page
    /// with the `Q` that ends the `q` it put before the page's content;
    /// kept, it leaves the page as sheaf's own header does, its `Q` outside
    /// its range. The two cuts either side are spliced together, the
    /// marked content one opens closed by the other.
    pub fn cuts(
        &self,
        content: &Concat,
        res: &ResCx,
        lim: &Limits,
    ) -> Result<Vec<SpanAcross>, LexError> {
        let Some(range) = content.offsets(self.span) else {
            return Ok(vec![self.span]);
        };
        let at = |e: LexError| LexError {
            at: e.at + range.start,
            ..e
        };
        let mut ops = lex::lex(&content.bytes()[range.clone()], res, lim)
            .filter(|t| !matches!(t, Ok((_, Tok::Operand(_)))));
        // Its BMC or BDC, then the run of `Q`s.
        ops.next().transpose().map_err(at)?;
        let mut kept: Option<Range<usize>> = None;
        for token in ops {
            match token.map_err(at)? {
                (r, Tok::Op(b"Q")) => kept = Some(kept.map_or(r.clone(), |k| k.start..r.end)),
                _ => break,
            }
        }
        Ok(match kept {
            None => vec![self.span],
            Some(q) => vec![
                content.span(range.start..range.start + q.start),
                content.span(range.start + q.end..range.end),
            ],
        })
    }
}

/// A page's content, classified.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageBlocks {
    /// The artifact ranges, at any depth, and sheaf's marked blocks, in
    /// the order they open; not the structure's ranges, which a tagged
    /// page has one of for every run of text.
    pub marked: Arc<[Marked]>,
    pub pagination: Arc<[Pagination]>,
    /// sheaf's blocks: each stream marked `/SheafBlock`, and each range
    /// whose list has `/SheafGen` or `/SheafOcrBlock` in a stream that
    /// isn't.
    pub sheaf: Arc<[SheafBlockRef]>,
    /// The streams marked `/SheafBlock << /K /Isolate >>`.
    pub isolation: Arc<[ObjectId]>,
    pub legacy_ocr: Option<LegacyOcr>,
    pub balance: Balance,
    /// Whether a Form XObject the page draws outside its pagination
    /// artifacts has one inside: removing the page's leaves it in place.
    /// Forms are read one level deep, each once, within what's left of
    /// the page's [`ContentLimits::decompressed`] after its streams: one
    /// that can't be read draws nothing, as in PDFium, and has none, but
    /// one past the cap, or met once the page's share is spent, may have
    /// one, and counts as having one.
    ///
    /// [`ContentLimits::decompressed`]: crate::source::ContentLimits
    pub nested_in_forms: bool,
}

/// A page whose content can't be read, or can't be changed as asked: what
/// an op that needs it reports, naming the page.
#[derive(Debug, thiserror::Error)]
pub enum ContentError {
    #[error("page {}'s content can't be read; nothing was changed", .page + 1)]
    Read {
        /// From 0.
        page: u32,
        #[source]
        fault: ContentFault,
    },
    #[error(
        "page {}'s content can't be changed without changing what else it draws; nothing was changed",
        .page + 1
    )]
    Unsafe {
        page: u32,
        #[source]
        error: SpliceError,
    },
}

impl ContentError {
    /// The page it names, from 0.
    pub fn page(&self) -> u32 {
        match *self {
            ContentError::Read { page, .. } | ContentError::Unsafe { page, .. } => page,
        }
    }

    /// The same error naming page `page`: a document's, where a source's
    /// page was named.
    pub fn on_page(mut self, to: u32) -> ContentError {
        match &mut self {
            ContentError::Read { page, .. } | ContentError::Unsafe { page, .. } => *page = to,
        }
        self
    }
}

impl PageBlocks {
    /// The blocks of a page whose `/Contents` names `contents`, drawn with
    /// `resources`, its streams read without keeping them; what's known
    /// of the forms it draws is in `known`.
    pub fn read(
        objects: &impl ReadObjects,
        contents: &[ObjectId],
        resources: Option<&Object>,
        lim: &Limits,
        known: &DrawnForms,
    ) -> Result<PageBlocks, ContentFault> {
        let content = PageContent::read(objects, contents, lim)?;
        PageBlocks::of(objects, &content, resources, lim, known)
    }

    /// The blocks of `content`, drawn with `resources`; what's known of
    /// the forms it draws is in `known`.
    pub fn of(
        objects: &impl ReadObjects,
        content: &PageContent,
        resources: Option<&Object>,
        lim: &Limits,
        known: &DrawnForms,
    ) -> Result<PageBlocks, ContentFault> {
        let concat = content.concat();
        let res = ResCx::read(objects, resources);
        let props = PropertiesDict::read(objects, resources);
        let mut scan = Scan {
            nest: Nest::new(&props, lim),
            balance: Balance::default(),
            open: None,
            drawn: Vec::new(),
            forms: BTreeSet::new(),
        };
        lex::steps(concat.bytes(), &res, lim, |step| scan.step(concat, &step))
            .map_err(|e| content.fault(e))?;
        let Scan {
            nest,
            balance,
            drawn,
            forms,
            ..
        } = scan;
        let marked = nest.done().map_err(|e| content.fault(e))?;
        let marks: Vec<StreamMark> = content.dicts().iter().map(StreamMark::of).collect();
        let mark_of = |m: &Marked| marks.get(m.span.start.stream as usize);
        let pagination = drawn
            .iter()
            .map(|d| {
                let range = &marked[d.index];
                let mark = mark_of(range).unwrap_or(&StreamMark::Plain);
                Pagination {
                    kind: d.kind,
                    origin: d.origin(range, mark, |form| {
                        xobject(objects, resources, form)
                            .is_some_and(|id| known.acrobat(objects, id))
                    }),
                    span: range.span,
                }
            })
            .collect();
        let ids = content.ids();
        let mut sheaf: Vec<SheafBlockRef> = ids
            .iter()
            .zip(&marks)
            .filter_map(|(&id, mark)| match *mark {
                StreamMark::Block(block) => Some(SheafBlockRef {
                    block,
                    at: BlockAt::Stream(id),
                }),
                _ => None,
            })
            .collect();
        sheaf.extend(marked.iter().filter_map(|m| {
            if matches!(mark_of(m), Some(StreamMark::Block(_))) {
                return None;
            }
            Some(SheafBlockRef {
                block: marked_block(m)?,
                at: BlockAt::Marked(m.span),
            })
        }));
        let isolation = ids
            .iter()
            .zip(&marks)
            .filter(|(_, mark)| matches!(mark, StreamMark::Isolate))
            .map(|(&id, _)| id)
            .collect();
        let forms: BTreeSet<ObjectId> = forms
            .iter()
            .filter_map(|form| xobject(objects, resources, form))
            .collect();
        let mut budget = lim
            .content
            .decompressed
            .saturating_sub(concat.bytes().len());
        let nested_in_forms = forms
            .into_iter()
            .any(|form| known.nested(objects, form, resources, lim, &mut budget));
        Ok(PageBlocks {
            marked: marked
                .into_iter()
                .filter(|m| m.tag == b"Artifact" || marked_block(m).is_some())
                .collect(),
            pagination,
            sheaf: sheaf.into(),
            isolation,
            legacy_ocr: legacy_ocr(content, &marks),
            balance,
            nested_in_forms,
        })
    }

    /// Whether it has a pagination artifact of one of `kinds`.
    pub fn has(&self, kinds: &[PaginationKind]) -> bool {
        self.pagination.iter().any(|p| kinds.contains(&p.kind))
    }
}

/// What a stream's dictionary marks it as.
enum StreamMark {
    Plain,
    Block(SheafBlock),
    Isolate,
    OcrLayer,
}

impl StreamMark {
    fn of(dict: &Dictionary) -> StreamMark {
        if dict.has(b"SheafOcrLayer") {
            return StreamMark::OcrLayer;
        }
        let Ok(Object::Dictionary(block)) = dict.get(b"SheafBlock") else {
            return StreamMark::Plain;
        };
        let string = |key: &[u8]| block.get(key).ok()?.as_str().ok();
        let mark = match block.get(b"K").and_then(Object::as_name) {
            Ok(b"Gen") => string(b"G")
                .and_then(GenId::parse)
                .map(|g| StreamMark::Block(SheafBlock::Gen(g))),
            Ok(b"Ocr") => string(b"B")
                .and_then(BlockId::parse)
                .map(|b| StreamMark::Block(SheafBlock::Ocr(b))),
            Ok(b"Flat") => string(b"B")
                .and_then(BlockId::parse)
                .map(|b| StreamMark::Block(SheafBlock::Flat(b))),
            Ok(b"Isolate") => Some(StreamMark::Isolate),
            _ => None,
        };
        mark.unwrap_or(StreamMark::Plain)
    }
}

/// The block of sheaf's a range's list marks: `/SheafGen` on a
/// generator's, `/SheafOcrBlock` on an OCR layer's.
fn marked_block(m: &Marked) -> Option<SheafBlock> {
    let list = m.props.as_ref()?.dict()?;
    let string = |key: &[u8]| list.get(key).ok()?.as_str().ok();
    string(b"SheafGen")
        .and_then(GenId::parse)
        .map(SheafBlock::Gen)
        .or_else(|| {
            string(b"SheafOcrBlock")
                .and_then(BlockId::parse)
                .map(SheafBlock::Ocr)
        })
}

/// The kind of pagination artifact `m` is, if it's one.
fn pagination(m: &Marked) -> Option<PaginationKind> {
    if m.tag != b"Artifact" {
        return None;
    }
    let list = m.props.as_ref()?.dict()?;
    PaginationKind::from_name(list.get(b"Subtype").ok()?.as_name().ok()?)
}

/// The legacy OCR layer's two streams, when the page has both.
fn legacy_ocr(content: &PageContent, marks: &[StreamMark]) -> Option<LegacyOcr> {
    let mut ocr = marks
        .iter()
        .enumerate()
        .filter(|(_, m)| matches!(m, StreamMark::OcrLayer))
        .map(|(i, _)| (i, content.concat().stream(i).trim_ascii() == b"q"));
    let q = ocr.find(|&(_, q)| q)?.0;
    let layer = ocr.rfind(|&(_, q)| !q)?.0;
    Some(LegacyOcr {
        q: content.ids()[q],
        layer: content.ids()[layer],
    })
}

/// A pagination artifact being lexed, outside any other: what it draws.
struct Drawn {
    /// Where it is in the marked ranges.
    index: usize,
    kind: PaginationKind,
    /// Whether it sets the font `/SheafHF`.
    sheaf_hf: bool,
    /// Whether it paints anything but XObjects.
    paints: bool,
    /// The XObjects it draws.
    forms: BTreeSet<Vec<u8>>,
}

impl Drawn {
    /// Who drew it, `range` in a stream marked `mark`; `acrobat` says
    /// whether a form it draws is Acrobat's (§13's classification).
    fn origin(
        &self,
        range: &Marked,
        mark: &StreamMark,
        acrobat: impl Fn(&[u8]) -> bool,
    ) -> PagOrigin {
        let list = range.props.as_ref().and_then(Props::dict);
        let has = |key: &[u8]| list.is_some_and(|l| l.has(key));
        match mark {
            StreamMark::Block(_) => PagOrigin::Sheaf,
            _ if has(b"SheafGen") => PagOrigin::Sheaf,
            StreamMark::OcrLayer => PagOrigin::SheafLegacy,
            _ if has(b"SheafStamp") || has(b"SheafSettings") || self.sheaf_hf => {
                PagOrigin::SheafLegacy
            }
            _ if !self.paints
                && !self.forms.is_empty()
                && self.forms.iter().all(|f| acrobat(f)) =>
            {
                PagOrigin::Acrobat
            }
            _ => PagOrigin::Authored,
        }
    }
}

/// A page's content as it's stepped through.
struct Scan<'p> {
    nest: Nest<'p>,
    balance: Balance,
    open: Option<Drawn>,
    drawn: Vec<Drawn>,
    /// The XObjects drawn outside pagination artifacts.
    forms: BTreeSet<Vec<u8>>,
}

impl Scan<'_> {
    fn step(&mut self, concat: &Concat, step: &Step) -> Result<(), LexError> {
        match self.nest.step(concat, step)? {
            Some(Event::Opened(index)) if self.open.is_none() => {
                if let Some(kind) = pagination(self.nest.get(index)) {
                    self.open = Some(Drawn {
                        index,
                        kind,
                        sheaf_hf: false,
                        paints: false,
                        forms: BTreeSet::new(),
                    });
                }
            }
            Some(Event::Ended(index)) => {
                if let Some(drawn) = self.open.take_if(|d| d.index == index) {
                    self.drawn.push(drawn);
                }
            }
            _ => {}
        }
        let op = match step.tok {
            Tok::Op(op) => op,
            Tok::InlineImage(_) => b"BI",
            Tok::Operand(_) => return Ok(()),
        };
        self.balance.take(op);
        let Some(drawn) = &mut self.open else {
            if op == b"Do"
                && let Some(form) = step.name(0)
            {
                self.forms.insert(form.into_owned());
            }
            return Ok(());
        };
        match op {
            b"Do" => drawn.forms.extend(step.name(0).map(|f| f.into_owned())),
            b"Tf" => drawn.sheaf_hf |= step.name(1).as_deref() == Some(b"SheafHF"),
            op => drawn.paints |= paints(op),
        }
        Ok(())
    }
}

/// Whether `op` paints: a path, text, a shading or an inline image (`BI`).
fn paints(op: &[u8]) -> bool {
    matches!(
        op,
        b"S" | b"s"
            | b"f"
            | b"F"
            | b"f*"
            | b"B"
            | b"B*"
            | b"b"
            | b"b*"
            | b"Tj"
            | b"TJ"
            | b"'"
            | b"\""
            | b"sh"
            | b"BI"
    )
}

/// XObject `name` of `resources`.
fn xobject(
    objects: &impl ReadObjects,
    resources: Option<&Object>,
    name: &[u8],
) -> Option<ObjectId> {
    let resources = direct(objects, resources?)?.as_dict().ok()?;
    let xobjects = direct(objects, resources.get(b"XObject").ok()?)?;
    xobjects.as_dict().ok()?.get(name).ok()?.as_reference().ok()
}

/// What's known of the XObjects pages draw, by object, so each is read
/// once however many names and pages draw it: whether it's a form Acrobat
/// drew a pagination artifact in, and whether one is inside it. A source
/// keeps one for its pages. Whether a form that draws with its page's
/// resources, having none of its own, has one depends on the page, so
/// it's read for each page that draws it.
#[derive(Debug, Default)]
pub struct DrawnForms {
    acrobat: Mutex<HashMap<ObjectId, bool>>,
    nested: Mutex<HashMap<ObjectId, bool>>,
}

impl DrawnForms {
    /// Whether XObject `id` is a form Acrobat drew a pagination artifact
    /// in: its `/PieceInfo /ADBE_CompoundType /Private` names a pagination
    /// kind.
    fn acrobat(&self, objects: &impl ReadObjects, id: ObjectId) -> bool {
        if let Some(&known) = lock(&self.acrobat).get(&id) {
            return known;
        }
        let kind = objects.read(id, |form| {
            let dict = &form.as_stream().ok()?.dict;
            let get = |d: &Dictionary, key: &[u8]| direct(objects, d.get(key).ok()?).cloned();
            let piece = get(dict, b"PieceInfo")?;
            let compound = get(piece.as_dict().ok()?, b"ADBE_CompoundType")?;
            let private = get(compound.as_dict().ok()?, b"Private")?;
            PaginationKind::from_name(private.as_name().ok()?)
        });
        let acrobat = matches!(kind, Ok(Some(_)));
        lock(&self.acrobat).insert(id, acrobat);
        acrobat
    }

    /// Whether XObject `id` is a form with a pagination artifact in it,
    /// drawn on a page with `resources`. Reading it takes its size from
    /// `budget`, what's left of the page's; with none left, it counts as
    /// having one.
    fn nested(
        &self,
        objects: &impl ReadObjects,
        id: ObjectId,
        resources: Option<&Object>,
        lim: &Limits,
        budget: &mut usize,
    ) -> bool {
        if let Some(&known) = lock(&self.nested).get(&id) {
            return known;
        }
        if *budget == 0 {
            return true;
        }
        let read = objects.read(id, |form| {
            let form = form.as_stream().ok()?;
            if form.dict.get(b"Subtype").and_then(Object::as_name).ok()? != b"Form" {
                return None;
            }
            let own = form.dict.get(b"Resources").ok().cloned();
            Some(
                match form.decompressed_content_with_limit(lim.content.decompressed) {
                    Ok(data) => Ok((data, own)),
                    Err(lopdf::Error::Decompress(
                        lopdf::DecompressError::MemoryLimitExceeded { .. },
                    )) => Err(true),
                    Err(_) => Err(false),
                },
            )
        });
        let (data, own) = match read {
            Ok(Some(Ok(form))) => form,
            // Past the cap, it may have one; not a form, or unreadable,
            // it draws nothing.
            Ok(Some(Err(too_large))) => return self.knows(id, too_large),
            Ok(None) | Err(_) => return self.knows(id, false),
        };
        *budget = budget.saturating_sub(data.len());
        // A form without resources of its own draws with its page's.
        let resources = own.as_ref().or(resources);
        let content = Concat::new(&[&data]);
        let res = ResCx::read(objects, resources);
        let props = PropertiesDict::read(objects, resources);
        let nested = marked::marked(&content, &props, &res, lim)
            .is_ok_and(|found| found.iter().any(|m| pagination(m).is_some()));
        if own.is_some() {
            self.knows(id, nested);
        }
        nested
    }

    /// `nested` for form `id`, kept.
    fn knows(&self, id: ObjectId, nested: bool) -> bool {
        lock(&self.nested).insert(id, nested);
        nested
    }

    /// Whether form `id` is known to have a pagination artifact in it, or
    /// not.
    #[cfg(test)]
    pub(crate) fn nested_known(&self, id: ObjectId) -> Option<bool> {
        lock(&self.nested).get(&id).copied()
    }
}

fn lock<T>(lock: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(PoisonError::into_inner)
}
