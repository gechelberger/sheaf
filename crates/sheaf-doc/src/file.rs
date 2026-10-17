//! Direct access to a PDF file's object graph via lopdf. This is the
//! serialization backend (see docs/document-model.md), not the editing
//! model; the viewer never touches it.

use std::path::Path;

use anyhow::{Context, Result};

pub struct PdfFile {
    inner: lopdf::Document,
}

impl PdfFile {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let inner =
            lopdf::Document::load(path).with_context(|| format!("loading {}", path.display()))?;
        Ok(Self { inner })
    }

    pub fn page_count(&self) -> u32 {
        self.inner.get_pages().len() as u32
    }

    /// Full rewrite. Incremental save comes in M2.
    pub fn save_as(&mut self, path: impl AsRef<Path>) -> Result<()> {
        self.inner.save(path.as_ref())?;
        Ok(())
    }

    pub fn into_inner(self) -> lopdf::Document {
        self.inner
    }
}

use std::cell::OnceCell;
use std::collections::HashMap;

use lopdf::{Dictionary, Object, ObjectId};
use sheaf_core::Rotation;

use crate::annot::{Annot, AnnotKind, Box2, PART, Pt, from_json};
use crate::attachments::Objects;
use crate::geom::{Geom, Rect};
use crate::import::{inset, numbers, points, quads, user_box};

/// A page's `/Annots`, as [`sheaf_mark`] and [`is_ours`] read its
/// entries. A version 1 replacement's struck text is the StrikeOut grouped
/// under it (`/RT /Group`, its `/IRT` the replacement's entry), found in
/// an index of the page's StrikeOuts made the first time one is asked
/// for, so a page of replacements reads in linear time.
pub(crate) struct PageAnnots<'a> {
    list: &'a [Object],
    strikes: OnceCell<HashMap<ObjectId, &'a Dictionary>>,
}

impl<'a> PageAnnots<'a> {
    pub(crate) fn new(list: &'a [Object]) -> Self {
        Self {
            list,
            strikes: OnceCell::new(),
        }
    }

    /// The first StrikeOut grouped under entry `id`.
    fn strike<P: Objects + ?Sized>(&self, pdf: &'a P, id: ObjectId) -> Option<&'a Dictionary> {
        let strikes = self.strikes.get_or_init(|| {
            let mut by_parent = HashMap::new();
            for e in self.list {
                let Some(d) = pdf.dereference(e).ok().and_then(|(_, o)| o.as_dict().ok()) else {
                    continue;
                };
                let name = |key: &[u8], is: &[u8]| {
                    d.get(key).and_then(Object::as_name).is_ok_and(|n| n == is)
                };
                if let Ok(irt) = d.get(b"IRT").and_then(Object::as_reference)
                    && name(b"RT", b"Group")
                    && name(b"Subtype", b"StrikeOut")
                {
                    by_parent.entry(irt).or_insert(d);
                }
            }
            by_parent
        });
        strikes.get(&id).copied()
    }
}

/// The mark an `/Annots` entry carries, when this build can read it and
/// no other tool has changed it since sheaf wrote it (M10.5 §3.6). A
/// version 2 payload is fresh while its digest is the dictionary's
/// ([`staleness`](crate::annot::staleness)). A version 1 payload is fresh
/// while its `modified` is `/M`. Its geometry was in the page's displayed
/// space, which on a page sheaf had turned wasn't the `/Rotate` the save
/// wrote, so the geometry is read from the standard keys, which are
/// PageSpace, and the mark reads upright turned `turn`, the page's
/// `/Rotate`; a replacement's struck text is its StrikeOut's, among
/// `annots`, the page's. None for anything else, which is another tool's.
pub(crate) fn sheaf_mark<'a, P: Objects + ?Sized>(
    pdf: &'a P,
    entry: &Object,
    turn: Rotation,
    annots: &PageAnnots<'a>,
) -> Option<Annot> {
    let (id, dict) = pdf.dereference(entry).ok()?;
    let dict = dict.as_dict().ok()?;
    let (_, json) = pdf.dereference(dict.get(b"SheafAnnot").ok()?).ok()?;
    let payload = from_json(text_string(json.as_str().ok()?).as_bytes())?;
    let a = payload.mark;
    match payload.v {
        Some(crate::annot::VERSION) => {
            let digest = crate::annot::staleness(pdf, dict, a.thread_parent().is_some());
            (payload.std == digest).then_some(a)
        }
        None => {
            let modified = dict.get(b"M").map(|m| text(pdf, m)).unwrap_or_default();
            if a.modified != modified {
                return None;
            }
            let strike = || annots.strike(pdf, id?);
            Some(Annot {
                kind: from_standard_keys(pdf, dict, &a, strike)?,
                turn,
                ..a
            })
        }
        Some(_) => None,
    }
}

/// A version 1 mark's geometry, as the standard keys sheaf wrote have it:
/// a box is `/Rect` less `/RD`, or less the writer's pad where it wrote
/// none; text markup is its `/QuadPoints`, lines and paths their `/L`,
/// `/Vertices`, `/InkList` and `/CL`; a note's or an attachment's corner
/// is its icon box's. Everything else is `a`'s.
fn from_standard_keys<'s, P: Objects + ?Sized>(
    pdf: &P,
    dict: &Dictionary,
    a: &Annot,
    strike: impl FnOnce() -> Option<&'s Dictionary>,
) -> Option<AnnotKind> {
    let rect = numbers(pdf, dict, b"Rect").filter(|r| r.len() == 4)?;
    let padded = user_box([rect[0], rect[1], rect[2], rect[3]]);
    let less_pad = || inset(padded, a.stroke.max(1.0) + 1.0);
    let less_rd = || {
        let d = numbers(pdf, dict, b"RD").filter(|d| d.len() == 4)?;
        let (x0, y0) = (padded.x + d[0], padded.y + d[1]);
        let (x1, y1) = (padded.right() - d[2], padded.bottom() - d[3]);
        Some(user_box([x0, y0, x1, y1]))
    };
    let corner = |b: Box2| Pt { x: b.x, y: b.y };
    let line = || {
        let l = numbers(pdf, dict, b"L").filter(|l| l.len() == 4)?;
        Some(points(&l))
    };
    let vertices = || numbers(pdf, dict, b"Vertices").map(|v| points(&v));
    Some(match a.kind.clone() {
        AnnotKind::Highlight { .. } => AnnotKind::Highlight {
            rects: quads(pdf, dict)?,
        },
        AnnotKind::Underline { .. } => AnnotKind::Underline {
            rects: quads(pdf, dict)?,
        },
        AnnotKind::StrikeOut { .. } => AnnotKind::StrikeOut {
            rects: quads(pdf, dict)?,
        },
        AnnotKind::Squiggly { .. } => AnnotKind::Squiggly {
            rects: quads(pdf, dict)?,
        },
        AnnotKind::Replace { struck, .. } => AnnotKind::Replace {
            rects: quads(pdf, strike()?)?,
            struck,
            at: less_rd()?,
        },
        AnnotKind::Insert { .. } => AnnotKind::Insert { at: less_rd()? },
        kind @ (AnnotKind::Reply { .. } | AnnotKind::Status { .. }) => kind,
        AnnotKind::Note { .. } => AnnotKind::Note {
            pos: corner(less_pad()),
        },
        AnnotKind::FileAttachment {
            icon,
            file,
            name,
            size,
            ..
        } => AnnotKind::FileAttachment {
            pos: corner(less_pad()),
            icon,
            file,
            name,
            size,
        },
        AnnotKind::FreeText { size, .. } => AnnotKind::FreeText {
            rect: less_rd()?,
            size,
        },
        AnnotKind::Square { .. } => AnnotKind::Square { rect: less_rd()? },
        AnnotKind::Ellipse { .. } => AnnotKind::Ellipse { rect: less_rd()? },
        AnnotKind::Tape {
            size, tape, look, ..
        } => AnnotKind::Tape {
            rect: less_pad(),
            size,
            tape,
            look,
        },
        AnnotKind::Line {
            arrow, arrow_start, ..
        } => {
            let [a, b] = line()?.try_into().ok()?;
            AnnotKind::Line {
                a,
                b,
                arrow,
                arrow_start,
            }
        }
        AnnotKind::Ink { .. } => AnnotKind::Ink {
            paths: crate::import::ink(pdf, dict)?,
        },
        AnnotKind::Stamp { glyph, .. } => AnnotKind::Stamp {
            rect: less_pad(),
            glyph,
        },
        AnnotKind::Connector { label, partner, .. } => AnnotKind::Connector {
            rect: less_pad(),
            label,
            partner,
        },
        AnnotKind::PageStamp {
            stamp, name, text, ..
        } => AnnotKind::PageStamp {
            rect: less_pad(),
            stamp,
            name,
            text,
        },
        AnnotKind::TextStamp { text, status, .. } => AnnotKind::TextStamp {
            rect: less_pad(),
            text,
            status,
        },
        AnnotKind::PageBorder { status, .. } => AnnotKind::PageBorder {
            rect: less_pad(),
            status,
        },
        AnnotKind::Polygon { .. } => AnnotKind::Polygon {
            points: vertices()?,
        },
        AnnotKind::Polyline { .. } => AnnotKind::Polyline {
            points: vertices()?,
        },
        AnnotKind::Callout { size, .. } => {
            // [point, knee, where it meets the box].
            let cl = numbers(pdf, dict, b"CL").filter(|cl| cl.len() >= 4)?;
            let cl = points(&cl);
            AnnotKind::Callout {
                rect: less_rd()?,
                knee: cl[1],
                point: cl[0],
                size,
            }
        }
        AnnotKind::Measure { what, scale, .. } => AnnotKind::Measure {
            points: match what {
                crate::measure::Measured::Distance => line()?,
                _ => vertices()?,
            },
            what,
            scale,
        },
    })
}

/// Is this `/Annots` entry one of ours to rewrite: a mark this build can
/// read, fresh, or a companion of one (see `annot::PART`)? `annots` is the
/// page's. A stale mark isn't, nor is a `/SheafAnnot` this build can't
/// read, from a newer sheaf: each is kept like any other tool's
/// annotation, so a save here never loses it or reverts its edit. It
/// answers as [`sheaf_mark`] does, so what the open lists as another
/// tool's comment is what a save keeps.
pub(crate) fn is_ours<'a, P: Objects + ?Sized>(
    pdf: &'a P,
    entry: &Object,
    annots: &PageAnnots<'a>,
) -> bool {
    let Some(dict) = pdf
        .dereference(entry)
        .ok()
        .and_then(|(_, o)| o.as_dict().ok())
    else {
        return false;
    };
    // The turn doesn't decide it.
    let mark = |entry: &Object| sheaf_mark(pdf, entry, Rotation::R0, annots).is_some();
    if dict.has(b"SheafAnnot") {
        return mark(entry);
    }
    // One step only: a companion's /IRT is its mark, never another companion.
    dict.has(PART.as_bytes()) && dict.get(b"IRT").is_ok_and(mark)
}

/// A text string's text, decoded as [`text_string`] decodes it, after its
/// references.
fn text<P: Objects + ?Sized>(pdf: &P, o: &Object) -> String {
    pdf.dereference(o)
        .ok()
        .and_then(|(_, o)| o.as_str().ok())
        .map(text_string)
        .unwrap_or_default()
}

/// An annotation some other tool left in the file. Listed read-only in the
/// Comments panel; PDFium draws it.
#[derive(Debug, Clone, PartialEq)]
pub struct ForeignAnnot {
    /// Page index in the file.
    pub page: u32,
    /// `/Subtype`, e.g. `Text`, `Highlight`.
    pub subtype: String,
    pub author: String,
    pub contents: String,
    /// `/M`, a PDF date string, or empty.
    pub modified: String,
    /// Its `/Rect`, normalized: PageSpace.
    pub rect: Box2,
    /// `/NM`, or for one without, a stand-in made from what it says
    /// ([`anon_name`]), so replies can still name it.
    pub name: String,
    /// The comment whose thread it's in (`/IRT`), by name.
    pub parent: Option<String>,
    /// `/RT /Group`: part of its parent (the StrikeOut of Acrobat's
    /// replace-text edit, say) rather than a reply to it.
    pub grouped: bool,
    /// A review state it sets on its parent: (`/StateModel`, `/State`).
    pub state: Option<(String, String)>,
    /// `/CreationDate`, or empty.
    pub created: String,
    /// Its position in the page's `/Annots`, which is how PDFium counts
    /// them too, so sheaf can hide it on screen.
    pub slot: usize,
    /// The file hides it (`/F` bit 2): sheaf never shows it.
    pub hidden: bool,
    /// `/C`, when it's RGB.
    pub color: Option<[u8; 3]>,
    /// What it is as one of our marks, when sheaf can hold it: what
    /// editing it makes it (M9 decision 5). Read when the file is.
    pub mark: Option<Box<Annot>>,
}

/// The names deleting `removed` takes off a page: those, and every
/// comment in their threads. `comments` is each comment's name and its
/// parent's.
pub fn removed_with_threads<'a>(
    comments: impl Iterator<Item = (&'a str, Option<&'a str>)> + Clone,
    removed: &im::OrdSet<String>,
) -> std::collections::HashSet<String> {
    let mut gone: std::collections::HashSet<String> = removed.iter().cloned().collect();
    loop {
        let more: Vec<String> = comments
            .clone()
            .filter(|(name, parent)| {
                !gone.contains(*name) && parent.is_some_and(|p| gone.contains(p))
            })
            .map(|(name, _)| name.to_string())
            .collect();
        if more.is_empty() {
            return gone;
        }
        gone.extend(more);
    }
}

/// A name for an annotation without `/NM`, from what it says: the same
/// every time it's read, so a reply made in sheaf can name it, and a save
/// writes it in as its `/NM`.
pub fn anon_name(dict: &lopdf::Dictionary) -> String {
    // The fields one after another, each ended by a byte no text has, and
    // their checksum: stable across builds, unlike std's hasher.
    let mut fields: Vec<u8> = Vec::new();
    let mut eat = |bytes: &[u8]| {
        fields.extend_from_slice(bytes);
        fields.push(0xff);
    };
    for key in [
        b"Subtype".as_slice(),
        b"Rect",
        b"T",
        b"M",
        b"CreationDate",
        b"Contents",
    ] {
        let v = dict.get(key).ok();
        match v {
            Some(lopdf::Object::Array(a)) => {
                for o in a {
                    // As an f32, as reals were read before, so names stay.
                    eat(format!("{:.2}", o.as_float().unwrap_or(0.0) as f32).as_bytes());
                }
            }
            Some(lopdf::Object::Name(n)) => eat(n),
            Some(o) => eat(pdf_text(o).as_bytes()),
            None => eat(b""),
        }
    }
    format!("sheaf-anon:{}", sheaf_core::checksum_hex(&fields))
}

/// What an annotation is called in threads: its `/NM`, else [`anon_name`].
pub fn thread_name(dict: &lopdf::Dictionary) -> String {
    dict.get(b"NM")
        .ok()
        .map(pdf_text)
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| anon_name(dict))
}

/// Everything annotation-shaped in a file: our marks (editable, keyed by
/// page index) and everyone else's.
#[derive(Debug, Default)]
pub struct AnnotsInFile {
    pub sheaf: HashMap<u32, Vec<Annot>>,
    pub foreign: Vec<ForeignAnnot>,
}

/// A source as lopdf sees it, decrypted as [`load_pdf`] does: its blob's
/// bytes, opened with its password, so a file replaced since (by a save in
/// place) doesn't matter.
pub fn load_source(info: &crate::SourceInfo) -> Result<lopdf::Document> {
    load_pdf_mem(info.src.bytes(), info.src.password())
        .with_context(|| format!("loading {}", info.name.display()))
}

/// [`load_pdf`] for a file in memory.
pub fn load_pdf_mem(bytes: &[u8], password: Option<&str>) -> Result<lopdf::Document> {
    let mut pdf = lopdf::Document::load_mem(bytes)?;
    if pdf.is_encrypted() {
        let options = lopdf::LoadOptions::with_password(password.unwrap_or(""));
        pdf = lopdf::Document::load_mem_with_options(bytes, options)
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("decrypting")?;
        pdf.trailer.remove(b"Encrypt");
        pdf.encryption_state = None;
    }
    Ok(pdf)
}

/// Load a file with lopdf and, if it's encrypted, decrypt it in memory with
/// `password` (or the empty user password many files use). The result is
/// plain: strings and streams readable, `/Encrypt` gone, so writing it
/// produces an unencrypted file.
pub fn load_pdf(path: &Path, password: Option<&str>) -> Result<lopdf::Document> {
    let mut pdf =
        lopdf::Document::load(path).with_context(|| format!("loading {}", path.display()))?;
    if pdf.is_encrypted() {
        // lopdf only parses an encrypted file's objects when it has the
        // password at load time, so load again with it.
        pdf = lopdf::Document::load_with_password(path, password.unwrap_or(""))
            .map_err(|e| anyhow::anyhow!("{e}"))
            .with_context(|| format!("decrypting {}", path.display()))?;
        pdf.trailer.remove(b"Encrypt");
        pdf.encryption_state = None;
    }
    Ok(pdf)
}

/// What our marks keep in this machine's cache, when it isn't there (a
/// file from someone else): a stamp's page, made again from its saved
/// appearance, and an attachment's file, from its embedded copy. `marks`
/// are the ones [`annots_and_cached`] found, with the dictionaries they
/// were read from.
pub(crate) fn recover_cached<P: Objects + ?Sized>(pdf: &P, marks: &[(Annot, &lopdf::Dictionary)]) {
    for (mark, dict) in marks {
        match &mark.kind {
            AnnotKind::FileAttachment { file, .. } => {
                if crate::generated::blob_path(file).is_some_and(|p| p.exists()) {
                    continue;
                }
                let bytes = dict
                    .get(b"FS")
                    .ok()
                    .and_then(|o| pdf.dereference(o).ok())
                    .and_then(|(_, o)| o.as_dict().ok())
                    .and_then(|fs| fs.get(b"EF").ok())
                    .and_then(|o| pdf.dereference(o).ok())
                    .and_then(|(_, o)| o.as_dict().ok())
                    .and_then(|ef| ef.get(b"F").ok())
                    .and_then(|o| o.as_reference().ok())
                    .and_then(|id| match pdf.get_object(id) {
                        Ok(lopdf::Object::Stream(s)) => Some(
                            s.decompressed_content()
                                .unwrap_or_else(|_| s.content.clone()),
                        ),
                        _ => None,
                    });
                match bytes.map(|b| crate::generated::store_blob_as(file, &b)) {
                    Some(Ok(_)) => {}
                    Some(Err(e)) => log::warn!("recovering attachment {file}: {e:#}"),
                    None => log::warn!("attachment {file} has no embedded file"),
                }
            }
            AnnotKind::PageStamp { stamp, .. } => {
                if crate::generated::path_of(stamp).is_some_and(|p| p.exists()) {
                    continue;
                }
                // /AP /N → its /Resources /XObject /SheafStamp.
                let form = dict
                    .get(b"AP")
                    .ok()
                    .and_then(|o| pdf.dereference(o).ok())
                    .and_then(|(_, o)| o.as_dict().ok())
                    .and_then(|ap| ap.get(b"N").ok())
                    .and_then(|n| n.as_reference().ok())
                    .and_then(|n| match pdf.get_object(n) {
                        Ok(lopdf::Object::Stream(s)) => Some(s.dict.clone()),
                        _ => None,
                    })
                    .and_then(|n| {
                        let res = pdf
                            .dereference(n.get(b"Resources").ok()?)
                            .ok()?
                            .1
                            .as_dict()
                            .ok()?
                            .clone();
                        let xo = pdf
                            .dereference(res.get(b"XObject").ok()?)
                            .ok()?
                            .1
                            .as_dict()
                            .ok()?
                            .clone();
                        xo.get(b"SheafStamp").ok()?.as_reference().ok()
                    });
                let made = form
                    .ok_or_else(|| anyhow::anyhow!("no saved appearance"))
                    .and_then(|f| crate::stamp_lib::form_to_pdf(pdf, f))
                    .and_then(|bytes| crate::generated::store_as(stamp, &bytes));
                if let Err(e) = made {
                    log::warn!("recovering stamp {stamp}: {e:#}");
                }
            }
            _ => {}
        }
    }
}

/// Annotations sheaf wrote into `path` earlier, keyed by page index.
#[cfg(test)]
pub(crate) fn read_sheaf_annots(path: impl AsRef<Path>) -> Result<HashMap<u32, Vec<Annot>>> {
    Ok(read_annots(path, None)?.sheaf)
}

/// Same as `read_sheaf_annots`, from bytes already in memory.
#[cfg(test)]
pub(crate) fn read_sheaf_annots_mem(bytes: &[u8]) -> Result<HashMap<u32, Vec<Annot>>> {
    Ok(read_annots_mem(bytes)?.sheaf)
}

/// All annotations in a file's bytes.
#[cfg(test)]
pub(crate) fn read_annots_mem(bytes: &[u8]) -> Result<AnnotsInFile> {
    let pdf = lopdf::Document::load_mem(bytes)?;
    Ok(annots_in(&pdf, pdf.page_iter()))
}

/// All annotations in `path`.
#[cfg(test)]
pub(crate) fn read_annots(path: impl AsRef<Path>, password: Option<&str>) -> Result<AnnotsInFile> {
    let pdf = load_pdf(path.as_ref(), password)?;
    Ok(annots_in(&pdf, pdf.page_iter()))
}

/// A text string's text, as PDFium decodes one: UTF-16 (a last odd byte
/// left out) or UTF-8 after their byte order marks, PDFDocEncoding
/// without one. ([`pdf_text`] reads PDFDocEncoding as Latin-1, which most
/// of it is, but not its quotes and dashes.)
pub fn text_string(bytes: &[u8]) -> String {
    let utf16 = |rest: &[u8], unit: fn([u8; 2]) -> u16| {
        let units: Vec<u16> = rest.as_chunks::<2>().0.iter().map(|&c| unit(c)).collect();
        String::from_utf16_lossy(&units)
    };
    match bytes {
        [0xFE, 0xFF, rest @ ..] => utf16(rest, u16::from_be_bytes),
        [0xFF, 0xFE, rest @ ..] => utf16(rest, u16::from_le_bytes),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => bytes.iter().map(|&b| pdf_doc_char(b)).collect(),
    }
}

/// Whether `s` reads back as itself written byte for byte, without a BOM:
/// ASCII but for the bytes PDFDocEncoding gives other characters, the
/// accents at 0x18–0x1F and the undefined 0x7F (see [`pdf_doc_char`]).
/// Anything else is written as UTF-16BE.
pub(crate) fn is_pdf_doc_ascii(s: &str) -> bool {
    s.bytes()
        .all(|b| b.is_ascii() && !matches!(b, 0x18..=0x1F | 0x7F))
}

/// A PDFDocEncoding byte's character: Latin-1's, but for the accents at
/// 0x18–0x1F and the punctuation and letters at 0x80–0xA0, and NUL for
/// the three bytes it leaves undefined (0x7F, 0x9F and 0xAD), as PDFium's
/// table has them.
fn pdf_doc_char(b: u8) -> char {
    const ACCENTS: [char; 8] = ['˘', 'ˇ', 'ˆ', '˙', '˝', '˛', '˚', '˜'];
    const HIGH: [char; 33] = [
        '•', '†', '‡', '…', '—', '–', 'ƒ', '⁄', '‹', '›', '−', '‰', '„', '“', '”', '‘', '’', '‚',
        '™', 'ﬁ', 'ﬂ', 'Ł', 'Œ', 'Š', 'Ÿ', 'Ž', 'ı', 'ł', 'œ', 'š', 'ž', '\0', '€',
    ];
    match b {
        0x18..=0x1F => ACCENTS[usize::from(b - 0x18)],
        0x80..=0xA0 => HIGH[usize::from(b - 0x80)],
        0x7F | 0xAD => '\0',
        b => char::from(b),
    }
}

/// Decode a PDF text string: UTF-16BE with BOM, else PDFDoc/Latin-1.
pub fn pdf_text(o: &lopdf::Object) -> String {
    match o.as_str() {
        Ok(bytes) => pdf_text_bytes(bytes),
        Err(_) => String::new(),
    }
}

/// [`pdf_text`] for a string's bytes (or a stream's, which hold text the
/// same way).
pub fn pdf_text_bytes(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = bytes[2..]
            .chunks(2)
            .map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        bytes.iter().map(|&b| b as char).collect()
    }
}

/// Subtypes that aren't comments: links, form fields, and the popup
/// windows that belong to other annotations.
fn is_comment_subtype(s: &str) -> bool {
    !matches!(
        s,
        "Link"
            | "Widget"
            | "Popup"
            | "Screen"
            | "PrinterMark"
            | "TrapNet"
            | "Watermark"
            | "3D"
            | "Movie"
            | "Sound"
            | "RichMedia"
    )
}

/// Every annotation on `pages`, the file's page objects by index.
pub fn annots_in<P: Objects + ?Sized>(
    pdf: &P,
    pages: impl IntoIterator<Item = lopdf::ObjectId>,
) -> AnnotsInFile {
    annots_and_cached(pdf, pages).0
}

/// [`annots_in`], and the marks it found that keep something in this
/// machine's cache (a stamp's page, an attachment's file), with the
/// dictionaries they were read from, for [`recover_cached`].
pub(crate) fn annots_and_cached<P: Objects + ?Sized>(
    pdf: &P,
    pages: impl IntoIterator<Item = lopdf::ObjectId>,
) -> (AnnotsInFile, Vec<(Annot, &lopdf::Dictionary)>) {
    let mut out = AnnotsInFile::default();
    let mut cached = Vec::new();
    for (i, page_id) in pages.into_iter().enumerate() {
        let Ok(page) = pdf.get_dictionary(page_id) else {
            continue;
        };
        let Ok(annots) = page
            .get(b"Annots")
            .and_then(|a| pdf.dereference(a).map(|(_, o)| o))
        else {
            continue;
        };
        let Ok(list) = annots.as_array() else {
            continue;
        };
        if list.is_empty() {
            continue;
        }
        let turn = page_geom(pdf, page_id).rotate;
        let entries = PageAnnots::new(list);
        // This page's, and their dictionaries, to convert together.
        let first = out.foreign.len();
        let mut dicts: Vec<&lopdf::Dictionary> = Vec::new();
        for (slot, entry) in list.iter().enumerate() {
            let Ok((_, obj)) = pdf.dereference(entry) else {
                continue;
            };
            let Ok(dict) = obj.as_dict() else { continue };
            if let Some(a) = sheaf_mark(pdf, entry, turn, &entries) {
                if matches!(
                    a.kind,
                    AnnotKind::FileAttachment { .. } | AnnotKind::PageStamp { .. }
                ) {
                    cached.push((a.clone(), dict));
                }
                out.sheaf.entry(i as u32).or_default().push(a);
                continue;
            }
            // A companion of one of ours is part of it. Anything else,
            // including a mark another tool has edited since sheaf wrote it
            // and one this build can't read, is listed as another tool's,
            // with what it is as one of our marks.
            if dict.has(PART.as_bytes()) && is_ours(pdf, entry, &entries) {
                continue;
            }
            let subtype = dict
                .get(b"Subtype")
                .ok()
                .and_then(|o| o.as_name().ok())
                .map(|n| String::from_utf8_lossy(n).into_owned())
                .unwrap_or_default();
            if subtype.is_empty() || !is_comment_subtype(&subtype) {
                continue;
            }
            let rect: Vec<f32> = dict
                .get(b"Rect")
                .ok()
                .and_then(|o| pdf.dereference(o).ok())
                .and_then(|(_, o)| o.as_array().ok())
                .map(|a| {
                    a.iter()
                        .map(|v| v.as_float().unwrap_or(0.0) as f32)
                        .collect()
                })
                .unwrap_or_default();
            if rect.len() != 4 {
                continue;
            }
            let text = |key: &[u8]| dict.get(key).map(pdf_text).unwrap_or_default();
            let name_of = |key: &[u8]| -> Option<String> {
                let (_, o) = pdf.dereference(dict.get(key).ok()?).ok()?;
                Some(thread_name(o.as_dict().ok()?))
            };
            let state = match (text(b"StateModel"), text(b"State")) {
                (_, s) if s.is_empty() => None,
                // Acrobat's default model.
                (m, s) if m.is_empty() => Some((crate::annot::REVIEW.to_string(), s)),
                (m, s) => Some((m, s)),
            };
            dicts.push(dict);
            out.foreign.push(ForeignAnnot {
                page: i as u32,
                subtype,
                author: text(b"T"),
                contents: text(b"Contents"),
                modified: text(b"M"),
                rect: Box2::from_corners(
                    Pt {
                        x: rect[0],
                        y: rect[1],
                    },
                    Pt {
                        x: rect[2],
                        y: rect[3],
                    },
                ),
                name: thread_name(dict),
                parent: name_of(b"IRT"),
                grouped: dict
                    .get(b"RT")
                    .ok()
                    .and_then(|o| o.as_name().ok())
                    .is_some_and(|n| n == b"Group"),
                state,
                created: text(b"CreationDate"),
                slot,
                hidden: dict
                    .get(b"F")
                    .ok()
                    .and_then(|f| f.as_i64().ok())
                    .is_some_and(|f| f & 2 != 0),
                color: dict
                    .get(b"C")
                    .ok()
                    .and_then(|c| pdf.dereference(c).ok())
                    .and_then(|(_, c)| c.as_array().ok())
                    .filter(|c| c.len() == 3)
                    .map(|c| {
                        let v = |i: usize| {
                            (c[i].as_float().unwrap_or(0.0).clamp(0.0, 1.0) * 255.0).round() as u8
                        };
                        [v(0), v(1), v(2)]
                    }),
                mark: None,
            });
        }
        if !dicts.is_empty() {
            let marks = crate::import::convert_page(pdf, &dicts, turn, Default::default());
            for (f, m) in out.foreign[first..].iter_mut().zip(marks) {
                f.mark = m.ok().map(Box::new);
            }
        }
    }
    (out, cached)
}

/// A page's boxes and `/Rotate`, its inherited attributes resolved, as
/// [`Geom::read`] reads them: for the old writers and readers of lopdf
/// objects, until slice 9.
pub fn page_geom<P: Objects + ?Sized>(pdf: &P, page: lopdf::ObjectId) -> Geom {
    let mut media = None;
    let mut crop = None;
    let mut rotate = None;
    let mut id = page;
    for _ in 0..64 {
        let Ok(dict) = pdf.get_dictionary(id) else {
            break;
        };
        let boxed = |key: &[u8]| {
            let (_, o) = pdf.dereference(dict.get(key).ok()?).ok()?;
            crate::annot::box_of(o.as_array().ok()?)
        };
        if media.is_none() {
            media = boxed(b"MediaBox");
        }
        if crop.is_none() {
            crop = boxed(b"CropBox");
        }
        if rotate.is_none()
            && let Ok(r) = dict.get(b"Rotate").and_then(|o| o.as_i64())
        {
            rotate = Some(r);
        }
        match dict.get(b"Parent").and_then(|p| p.as_reference()) {
            Ok(parent) => id = parent,
            Err(_) => break,
        }
    }
    let rect = |b: [f32; 4]| Rect::new(b[0], b[1], b[2], b[3]);
    Geom::read(media.map(rect), crop.map(rect), rotate.unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{Object, Stream, dictionary};
    use sheaf_core::Rotation;

    /// Reals are f64s (vendor/lopdf/SHEAF.md). Read and written back by
    /// lopdf, a fraction, a large number and a 16-digit coordinate keep
    /// their digits, in a dictionary and in content; and an f32 that sheaf
    /// computes is written as the digits it always was.
    #[test]
    fn reals_keep_their_digits() {
        use lopdf::content::{Content, Operation};
        const TEXTS: [&str; 3] = ["0.1", "123456.789", "306.1234567890123"];
        let reals = || TEXTS.map(|t| Object::Real(t.parse().unwrap())).to_vec();
        let mut pdf = lopdf::Document::with_version("1.7");
        let pages_id = pdf.new_object_id();
        let ops = format!("{} {} m\n{} w", TEXTS[1], TEXTS[2], TEXTS[0]);
        let content = pdf.add_object(Stream::new(dictionary! {}, ops.clone().into_bytes()));
        let page = pdf.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => content,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "SheafReals" => reals(),
        });
        pdf.objects.insert(
            pages_id,
            dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 }.into(),
        );
        let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        pdf.trailer.set("Root", catalog);
        let mut first = Vec::new();
        pdf.save_to(&mut first).unwrap();

        let mut read = lopdf::Document::load_mem(&first).unwrap();
        let mut written = Vec::new();
        read.save_to(&mut written).unwrap();
        let array = format!("[{}]", TEXTS.join(" "));
        assert!(
            written.windows(array.len()).any(|w| w == array.as_bytes()),
            "{}",
            String::from_utf8_lossy(&written)
        );
        let again = lopdf::Document::load_mem(&written).unwrap();
        let dict = again.get_dictionary(page).unwrap();
        assert_eq!(
            dict.get(b"SheafReals").unwrap().as_array().unwrap(),
            &reals()
        );
        let content = Content::decode(&again.get_page_content(page)).unwrap();
        let operands: Vec<Object> = content
            .operations
            .iter()
            .flat_map(|op| op.operands.clone())
            .collect();
        let [a, b, c] = reals().try_into().unwrap();
        assert_eq!(operands, vec![b, c, a]);
        assert_eq!(String::from_utf8(content.encode().unwrap()).unwrap(), ops);

        // An f32 writes its shortest digits, a whole one too.
        let reals = [0.1f32, 612.0, -3.0, 16_777_216.0, 1e-7].map(Object::from);
        let width = Content {
            operations: vec![Operation::new("w", reals.to_vec())],
        };
        assert_eq!(width.encode().unwrap(), b"0.1 612 -3 16777216 0.0000001 w");
    }

    #[test]
    fn their_threads_read_with_states_and_groups() {
        use lopdf::{Object, Stream, dictionary};
        let mut pdf = lopdf::Document::with_version("1.7");
        let pages_id = pdf.new_object_id();
        let content = pdf.add_object(Stream::new(dictionary! {}, b"".to_vec()));
        let rect = || Object::Array(vec![10.into(), 10.into(), 20.into(), 20.into()]);
        let text = |s: &str| Object::string_literal(s);
        let parent = pdf.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Highlight", "Rect" => rect(), "NM" => text("p"),
        });
        let accepted = pdf.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Text", "Rect" => rect(), "IRT" => parent,
            "State" => text("Accepted"), "StateModel" => text("Review"), "F" => 30,
        });
        // No /StateModel: Review, as Acrobat reads it.
        let completed = pdf.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Text", "Rect" => rect(), "IRT" => parent,
            "State" => text("Completed"),
        });
        let part = pdf.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "StrikeOut", "Rect" => rect(), "IRT" => parent,
            "RT" => Object::Name(b"Group".to_vec()),
        });
        let list: Vec<Object> = [parent, accepted, completed, part]
            .into_iter()
            .map(Object::Reference)
            .collect();
        let page = pdf.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => content,
            "MediaBox" => Object::Array(vec![0.into(), 0.into(), 612.into(), 792.into()]),
            "Annots" => list,
        });
        pdf.objects.insert(
            pages_id,
            Object::Dictionary(
                dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 },
            ),
        );
        let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        pdf.trailer.set("Root", catalog);
        let found = annots_in(&pdf, pdf.page_iter()).foreign;
        assert_eq!(found.len(), 4);
        assert_eq!(found[0].name, "p");
        assert_eq!(found[0].parent, None);
        assert_eq!(
            found[1].state,
            Some(("Review".to_string(), "Accepted".to_string()))
        );
        assert_eq!(found[1].parent.as_deref(), Some("p"));
        assert_eq!(found[2].state.as_ref().unwrap().0, "Review");
        assert!(found[3].grouped && !found[1].grouped);
    }

    #[test]
    fn a_companion_follows_its_mark() {
        use lopdf::{Object, Stream, dictionary};
        let mut pdf = lopdf::Document::with_version("1.7");
        let pages_id = pdf.new_object_id();
        let content = pdf.add_object(Stream::new(dictionary! {}, b"".to_vec()));
        let rect = || Object::Array(vec![10.into(), 10.into(), 20.into(), 20.into()]);
        let ours = Annot {
            id: crate::annot::AnnotId(5),
            kind: crate::annot::AnnotKind::Note {
                pos: Pt { x: 1.0, y: 2.0 },
            },
            color: [1, 2, 3],
            opacity: 1.0,
            stroke: 1.0,
            author: "me".into(),
            contents: String::new(),
            modified: String::new(),
            created: String::new(),
            fill: None,
            dashed: false,
            cloudy: false,
            turn: Rotation::R0,
            name: String::new(),
        };
        let main = crate::annot::to_pdf(&ours, &mut pdf).main;
        let main = pdf.add_object(main);
        let part = pdf.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Caret", "Rect" => rect(),
            "IRT" => main, PART => true,
        });
        // A mark from a newer sheaf, and its companion.
        let unknown = pdf.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "StrikeOut", "Rect" => rect(),
            "SheafAnnot" => Object::string_literal(r#"{"id":6,"kind":{"Future":{}}}"#),
        });
        let unknown_part = pdf.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Caret", "Rect" => rect(),
            "IRT" => unknown, PART => true,
        });
        let list: Vec<Object> = [main, part, unknown, unknown_part]
            .into_iter()
            .map(Object::Reference)
            .collect();
        let page = pdf.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => content,
            "MediaBox" => Object::Array(vec![0.into(), 0.into(), 612.into(), 792.into()]),
            "Annots" => list.clone(),
        });
        pdf.objects.insert(
            pages_id,
            Object::Dictionary(
                dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 },
            ),
        );
        let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        pdf.trailer.set("Root", catalog);

        let page = PageAnnots::new(&list);
        let mine: Vec<bool> = list.iter().map(|e| is_ours(&pdf, e, &page)).collect();
        assert_eq!(mine, vec![true, true, false, false]);
        let found = annots_in(&pdf, pdf.page_iter());
        assert_eq!(found.sheaf[&0], vec![ours]);
        // The newer sheaf's mark and its companion are listed like another
        // tool's, and kept.
        let subtypes: Vec<&str> = found.foreign.iter().map(|f| f.subtype.as_str()).collect();
        assert_eq!(subtypes, vec!["StrikeOut", "Caret"]);
    }

    /// One page carrying a third-party sticky note, a link (not a comment),
    /// a popup (not a comment), and one of ours.
    #[test]
    fn reads_foreign_and_sheaf_annotations() {
        let mut pdf = lopdf::Document::with_version("1.7");
        let pages_id = pdf.new_object_id();
        let content = pdf.add_object(Stream::new(dictionary! {}, b"".to_vec()));
        let note = pdf.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Text",
            "Rect" => Object::Array(vec![100.0.into(), 700.0.into(), 120.0.into(), 720.0.into()]),
            "T" => Object::String(b"Reviewer".to_vec(), lopdf::StringFormat::Literal),
            "Contents" => Object::String(vec![0xFE, 0xFF, 0x00, b'H', 0x00, b'i', 0x20, 0x14], lopdf::StringFormat::Hexadecimal),
            "M" => Object::String(b"D:20260101120000Z".to_vec(), lopdf::StringFormat::Literal),
        });
        let link = pdf.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Link",
            "Rect" => Object::Array(vec![0.0.into(), 0.0.into(), 10.0.into(), 10.0.into()]),
        });
        let popup = pdf.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Popup",
            "Rect" => Object::Array(vec![0.0.into(), 0.0.into(), 10.0.into(), 10.0.into()]),
        });
        let ours = Annot {
            id: crate::annot::AnnotId(3),
            kind: crate::annot::AnnotKind::Note {
                pos: Pt { x: 1.0, y: 2.0 },
            },
            color: [1, 2, 3],
            opacity: 1.0,
            stroke: 1.0,
            author: "me".into(),
            contents: "mine".into(),
            modified: String::new(),
            created: String::new(),
            fill: None,
            dashed: false,
            cloudy: false,
            turn: Rotation::R0,
            name: String::new(),
        };
        let dict = crate::annot::to_pdf(&ours, &mut pdf).main;
        let mine = pdf.add_object(dict);
        let page = pdf.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => Object::Array(vec![0.into(), 0.into(), 612.into(), 792.into()]),
            "Contents" => content,
            "Annots" => Object::Array(vec![note.into(), link.into(), popup.into(), mine.into()]),
        });
        pdf.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1,
            }),
        );
        let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        pdf.trailer.set("Root", catalog);

        let found = annots_in(&pdf, pdf.page_iter());
        assert_eq!(found.sheaf[&0], vec![ours]);
        assert_eq!(found.foreign.len(), 1, "{:?}", found.foreign);
        let f = &found.foreign[0];
        assert_eq!(f.subtype, "Text");
        assert_eq!(f.author, "Reviewer");
        assert_eq!(f.contents, "Hi\u{2014}");
        assert_eq!(f.modified, "D:20260101120000Z");
        // Its /Rect, which is PageSpace.
        assert_eq!(
            (f.rect.x, f.rect.y, f.rect.w, f.rect.h),
            (100.0, 700.0, 20.0, 20.0)
        );
    }

    /// One page with a mark and its companion, another tool's comment and
    /// a reply to it, a stamp with an appearance and a file attachment:
    /// read from a source, as a file opens, they read as they do loaded,
    /// the stamp converted through its appearance.
    #[test]
    fn annotations_read_from_a_source_as_loaded() {
        let mut pdf = lopdf::Document::with_version("1.7");
        let pages_id = pdf.new_object_id();
        let rect = || Object::Array(vec![10.into(), 10.into(), 20.into(), 20.into()]);
        let text = |s: &str| Object::string_literal(s);
        let ours = Annot {
            id: crate::annot::AnnotId(9),
            kind: crate::annot::AnnotKind::Note {
                pos: Pt { x: 1.0, y: 2.0 },
            },
            color: [1, 2, 3],
            opacity: 1.0,
            stroke: 1.0,
            author: "me".into(),
            contents: "mine".into(),
            modified: String::new(),
            created: String::new(),
            fill: None,
            dashed: false,
            cloudy: false,
            turn: Rotation::R0,
            name: String::new(),
        };
        let main = crate::annot::to_pdf(&ours, &mut pdf).main;
        let main = pdf.add_object(main);
        let part = pdf.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Caret", "Rect" => rect(),
            "IRT" => main, PART => true,
        });
        let note = pdf.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Text", "Rect" => rect(), "NM" => text("n"),
            "T" => text("Reviewer"), "Contents" => text("Why?"),
        });
        let reply = pdf.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Text", "Rect" => rect(), "IRT" => note,
            "Contents" => text("Because."),
        });
        // The same content every run, so its stamp page is one cache file.
        let form = pdf.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject", "Subtype" => "Form",
                "BBox" => vec![0.into(), 0.into(), 100.into(), 40.into()],
            },
            b"0 0 1 rg 0 0 100 40 re f".to_vec(),
        ));
        let stamp = pdf.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Stamp", "Name" => "SBApproved",
            "Rect" => vec![100.into(), 600.into(), 200.into(), 640.into()],
            "AP" => dictionary! { "N" => form },
        });
        let file = pdf.add_object(Stream::new(dictionary! {}, b"attached".to_vec()));
        let attached = pdf.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "FileAttachment", "Rect" => rect(),
            "FS" => dictionary! {
                "Type" => "Filespec", "F" => text("a.txt"), "EF" => dictionary! { "F" => file },
            },
        });
        let list: Vec<Object> = [main, part, note, reply, stamp, attached]
            .into_iter()
            .map(Object::Reference)
            .collect();
        let page = pdf.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Annots" => list,
        });
        pdf.objects.insert(
            pages_id,
            dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 }.into(),
        );
        let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        pdf.trailer.set("Root", catalog);

        let src = crate::open::tests::lazily(&mut pdf);
        let loaded = annots_in(&pdf, pdf.page_iter());
        let read = annots_in(src.pdf(), src.page_ids());
        assert_eq!(read.sheaf, loaded.sheaf);
        assert_eq!(read.foreign, loaded.foreign);
        assert_eq!(loaded.sheaf[&0], vec![ours]);
        let subtypes: Vec<&str> = loaded.foreign.iter().map(|f| f.subtype.as_str()).collect();
        assert_eq!(subtypes, ["Text", "Text", "Stamp", "FileAttachment"]);
        assert_eq!(loaded.foreign[1].parent.as_deref(), Some("n"));
        assert!(matches!(
            loaded.foreign[2].mark.as_deref(),
            Some(Annot {
                kind: crate::annot::AnnotKind::PageStamp { .. },
                ..
            })
        ));
    }
}
