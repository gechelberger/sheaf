//! Headers, footers and Bates numbers: text sheaf draws on pages.
//!
//! In the model it's one [`HeaderFooter`] per document: six slots (left,
//! center and right of the header and of the footer) whose text can use
//! tokens for the page number, page count, date, file name and Bates
//! number. On screen it's drawn over the pages until saved. A save
//! writes it into each page's content as marked-content artifacts, as
//! Acrobat marks its headers and footers, `<</Type /Pagination /Subtype
//! /Header>>`, the list named in the page's `/Properties`:
//! `/Artifact /Sh… BDC … EMC`. [`strip`] removes those again, and
//! Acrobat's, so Update and Remove work on files Acrobat marked too; an
//! author's own running heads stay (proposal 31).

use std::borrow::Cow;
use std::sync::Arc;

use lopdf::{Dictionary, Document as Pdf, Object, Stream, dictionary};
use serde::{Deserialize, Serialize};
use sheaf_core::PageSize;

use crate::annot::Pt;
use crate::attachments::Objects;
use crate::content::{
    self, Color, ContentError, ContentWriter, DrawnForms, Font, PagOrigin, PageBlocks, PageContent,
    Pagination, PaginationKind, ResCx, ResObjects, Reserved, ResourceNeeds, Std14,
};
use crate::geom::{Affine, Geom};
use crate::model::PageId;
use crate::source::Limits;

/// Bates numbering: `<prefix><number, zero-padded to digits><suffix>`,
/// counting pages in scope in document order from `start`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bates {
    pub prefix: String,
    pub suffix: String,
    pub digits: u8,
    pub start: u64,
}

impl Default for Bates {
    fn default() -> Self {
        Self {
            prefix: String::new(),
            suffix: String::new(),
            digits: 6,
            start: 1,
        }
    }
}

/// The header/footer of a document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HeaderFooter {
    /// Header left, center, right; footer left, center, right.
    pub texts: [String; 6],
    pub font_size: f32,
    pub color: [u8; 3],
    /// Distance from the top (header) and bottom (footer) edge to the
    /// text, and from the sides, in points.
    pub top: f32,
    pub bottom: f32,
    pub left: f32,
    pub right: f32,
    /// What `<<1>>` is on the first page of the document.
    pub first_page_number: u32,
    /// The pages it's on; none for every page.
    pub pages: Option<Vec<PageId>>,
    /// `<<date>>` (fixed when added, as Acrobat does).
    pub date: String,
    /// `<<file>>`: the document's file name when added.
    #[serde(default)]
    pub file: String,
    pub bates: Option<Bates>,
}

impl Default for HeaderFooter {
    fn default() -> Self {
        Self {
            texts: Default::default(),
            font_size: 10.0,
            color: [0, 0, 0],
            top: 36.0,
            bottom: 36.0,
            left: 72.0,
            right: 72.0,
            first_page_number: 1,
            pages: None,
            date: String::new(),
            file: String::new(),
            bates: None,
        }
    }
}

/// The tokens slot text can use.
pub const TOKENS: [(&str, &str); 5] = [
    ("<<1>>", "page number"),
    ("<<N>>", "page count"),
    ("<<date>>", "date"),
    ("<<file>>", "file name"),
    ("<<bates>>", "Bates number"),
];

/// What one page's tokens are.
pub struct PageFacts {
    /// 0-based position in the document.
    pub index: usize,
    pub count: usize,
    /// The page's Bates number, when Bates applies.
    pub bates: Option<u64>,
}

/// One piece of text, placed: its baseline's left end in the page's shown
/// space (top-left origin, points).
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    /// In the header (else the footer).
    pub header: bool,
    pub text: String,
    pub x: f32,
    pub baseline: f32,
    pub size: f32,
    pub width: f32,
}

impl HeaderFooter {
    pub fn is_empty(&self) -> bool {
        self.texts.iter().all(|t| t.trim().is_empty())
    }

    /// Whether it's on page `id`.
    pub fn covers(&self, id: PageId) -> bool {
        self.pages.as_ref().is_none_or(|p| p.contains(&id))
    }

    pub fn bates_text(&self, n: u64) -> String {
        match &self.bates {
            Some(b) => format!(
                "{}{:0width$}{}",
                b.prefix,
                n,
                b.suffix,
                width = b.digits as usize
            ),
            None => String::new(),
        }
    }

    fn expand(&self, text: &str, f: &PageFacts) -> String {
        let page = (f.index as u64 + self.first_page_number as u64).to_string();
        text.replace("<<1>>", &page)
            .replace("<<N>>", &f.count.to_string())
            .replace("<<date>>", &self.date)
            .replace("<<file>>", &self.file)
            .replace(
                "<<bates>>",
                &f.bates.map(|n| self.bates_text(n)).unwrap_or_default(),
            )
    }

    /// Each slot's text on the page `f` says, by slot, the empty ones left
    /// out: what [`layout`](Self::layout) places and
    /// [`unencodable`](Self::unencodable) reads.
    fn slots<'a>(&'a self, f: &'a PageFacts) -> impl Iterator<Item = (usize, String)> + 'a {
        self.texts
            .iter()
            .enumerate()
            .map(|(slot, text)| (slot, self.expand(text, f)))
            .filter(|(_, text)| !text.trim().is_empty())
    }

    /// Whether some of its text can't be drawn in [`FONT`]: it shows as
    /// `?` in the file. Its tokens draw the same characters on every page
    /// but the digits, which every font has, so the first page answers
    /// for all.
    pub fn unencodable(&self) -> bool {
        let first = PageFacts {
            index: 0,
            count: 1,
            bates: self.bates.as_ref().map(|b| b.start),
        };
        self.slots(&first)
            .any(|(_, text)| FONT.encode(&text).1.0 > 0)
    }

    /// The texts on a page of `size` (as shown).
    pub fn layout(&self, size: PageSize, f: &PageFacts) -> Vec<Placed> {
        let fs = self.font_size;
        let mut out = Vec::new();
        for (slot, text) in self.slots(f) {
            let width = FONT.width(&text, fs);
            let x = match slot % 3 {
                0 => self.left,
                1 => (size.width - width) / 2.0,
                _ => size.width - self.right - width,
            };
            // The cap height of Helvetica is about 0.72 em: the header's
            // tops sit `top` below the edge, the footer's baselines
            // `bottom` above it.
            let baseline = if slot < 3 {
                self.top + fs * 0.72
            } else {
                size.height - self.bottom
            };
            out.push(Placed {
                header: slot < 3,
                text,
                x,
                baseline,
                size: fs,
                width,
            });
        }
        out
    }

    /// Bates numbers by page, for the pages it covers, in order.
    pub fn bates_numbers(&self, pages: &[PageId]) -> Vec<Option<u64>> {
        let Some(b) = &self.bates else {
            return vec![None; pages.len()];
        };
        let mut n = b.start;
        pages
            .iter()
            .map(|id| {
                self.covers(*id).then(|| {
                    n += 1;
                    n - 1
                })
            })
            .collect()
    }
}

/// What the header/footer is drawn in.
pub const FONT: Std14 = Std14::Helvetica;
/// The resource name its font goes by on a page, until slice 17's
/// generator.
pub const FONT_NAME: &str = "SheafHF";
/// Marks our artifacts so they can be found again.
const MARKER: &str = "SheafStamp";

/// The artifact stream for `placed`, laid out on the page as it shows
/// with geometry `geom`, its rotation the page's, absolute; and the
/// resources it draws with, named clear of `reserved`. Each piece of text
/// is an `/Artifact` whose property list is named in the page's
/// `/Properties`. It starts with the `Q` that closes the `q` [`install`]
/// puts before the page's own content.
pub fn to_content(
    placed: &[Placed],
    color: [u8; 3],
    geom: &Geom,
    reserved: &Reserved,
) -> (Vec<u8>, ResourceNeeds) {
    // Draw in the shown frame: map shown points to user space.
    let to_user = geom.from_view();
    let theta = (geom.rotate.degrees() as f32).to_radians();
    let (cs, sn) = (theta.cos(), theta.sin());
    let mut w = ContentWriter::new();
    w.restore();
    for p in placed {
        let subtype = if p.header { "Header" } else { "Footer" };
        let o = to_user.apply(Pt {
            x: p.x,
            y: p.baseline,
        });
        w.begin_marked(
            "Artifact",
            dictionary! {
                "Type" => "Pagination",
                "Subtype" => Object::Name(subtype.into()),
                MARKER => true,
            },
        );
        w.save();
        w.begin_text();
        w.fill_color(Color::rgb8(color));
        w.font(&FONT, p.size);
        w.text_matrix(Affine {
            a: cs,
            b: sn,
            c: -sn,
            d: cs,
            e: o.x,
            f: o.y,
        });
        w.text(&p.text);
        w.end_text();
        w.restore();
        w.end_marked();
    }
    w.finish(reserved, &[(FONT.resource().id, FONT_NAME)])
}

/// Draw `placed` on `page`, after its own content, as [`to_content`]
/// lays it out: `/Contents` becomes `[q, original…, ours]`, the `q` and
/// the resources written once a save through `res`.
pub fn install(
    pdf: &mut Pdf,
    page: &mut Dictionary,
    placed: &[Placed],
    color: [u8; 3],
    geom: &Geom,
    res: &mut ResObjects,
) {
    if placed.is_empty() {
        return;
    }
    let reserved = Reserved::read(pdf, page.get(b"Resources").ok());
    let (content, needs) = to_content(placed, color, geom, &reserved);
    let q = res.stream(pdf, dictionary! {}, b"q\n".to_vec());
    let mut stream = Stream::new(dictionary! {}, content);
    let _ = stream.compress();
    let ours = pdf.add_object(stream);
    let mut entries = vec![Object::Reference(q)];
    match page.get(b"Contents").ok().cloned() {
        None => {}
        Some(Object::Array(a)) => entries.extend(a),
        Some(Object::Reference(r)) => match pdf.get_object(r) {
            Ok(Object::Array(a)) => entries.extend(a.iter().cloned()),
            _ => entries.push(Object::Reference(r)),
        },
        Some(other) => entries.push(other),
    }
    entries.push(Object::Reference(ours));
    page.set("Contents", Object::Array(entries));
    let mut resources = content::resources_of(pdf, page);
    res.bind(pdf, &needs, &mut resources);
    page.set("Resources", Object::Dictionary(resources));
}

/// Pagination kinds, by what's removed together.
pub const HEADER_FOOTER: &[PaginationKind] = &[PaginationKind::Header, PaginationKind::Footer];
pub const WATERMARK: &[PaginationKind] = &[PaginationKind::Watermark];
pub const BACKGROUND: &[PaginationKind] = &[PaginationKind::Background];

/// The pagination artifacts of `kinds` that [`strip`] removes from a page
/// whose blocks are `blocks`: Acrobat's and sheaf's, never an author's own
/// (proposal 31), which slice 17's `include_authored` asks for.
pub fn stripped<'a>(
    blocks: &'a PageBlocks,
    kinds: &'a [PaginationKind],
) -> impl Iterator<Item = &'a Pagination> + 'a {
    blocks
        .pagination
        .iter()
        .filter(|p| kinds.contains(&p.kind) && p.origin != PagOrigin::Authored)
}

/// Remove the pagination artifacts of `kinds` ([`HEADER_FOOTER`],
/// [`WATERMARK`], [`BACKGROUND`]) that Acrobat or sheaf drew from `page`'s
/// content ([`stripped`]); `index` is the page's, from 0, for errors. The
/// page's streams are spliced: what isn't removed stays byte for byte, and
/// each stream that changes is written as a new object, so a page that
/// shares it keeps it. Content that can't be read, or an artifact that
/// can't be removed without changing what else the page draws, is an
/// error, and nothing is changed. Returns whether any were found.
pub fn strip(
    pdf: &mut Pdf,
    page: &mut Dictionary,
    index: u32,
    kinds: &[PaginationKind],
) -> Result<bool, ContentError> {
    let lim = Limits::DEFAULT;
    let read = |fault| ContentError::Read { page: index, fault };
    let content = PageContent::read(pdf, &content::contents(page), &lim).map_err(read)?;
    let resources = page.get(b"Resources").ok();
    let blocks =
        PageBlocks::of(pdf, &content, resources, &lim, &DrawnForms::default()).map_err(read)?;
    let res = ResCx::read(pdf, resources);
    let mut drop = Vec::new();
    for p in stripped(&blocks, kinds) {
        let cuts = p.cuts(content.concat(), &res, &lim);
        drop.extend(cuts.map_err(|e| read(content.fault(e)))?);
    }
    if drop.is_empty() {
        return Ok(false);
    }
    let streams = content::splice(content.concat(), &res, &lim, &drop)
        .map_err(|error| ContentError::Unsafe { page: index, error })?;
    let entries: Vec<Object> = content
        .ids()
        .iter()
        .zip(streams)
        .map(|(&id, stream)| match stream {
            Cow::Borrowed(_) => id.into(),
            Cow::Owned(bytes) => {
                let mut stream = Stream::new(dictionary! {}, bytes);
                let _ = stream.compress();
                pdf.add_object(stream).into()
            }
        })
        .collect();
    page.set("Contents", entries);
    Ok(true)
}

/// The catalog key sheaf keeps its header/footer settings under, so
/// Update can start from them after a reopen (page scope by index).
pub const CATALOG_KEY: &str = "SheafHeaderFooter";

/// Settings as stored in a file: page scope by position.
#[derive(Serialize, Deserialize)]
struct Stored {
    #[serde(flatten)]
    hf: HeaderFooter,
    page_indexes: Option<Vec<usize>>,
}

pub fn to_catalog(hf: &HeaderFooter, pages: &[PageId]) -> Object {
    let page_indexes = hf.pages.as_ref().map(|scope| {
        pages
            .iter()
            .enumerate()
            .filter(|(_, id)| scope.contains(id))
            .map(|(i, _)| i)
            .collect()
    });
    let stored = Stored {
        hf: HeaderFooter {
            pages: None,
            ..hf.clone()
        },
        page_indexes,
    };
    let json = serde_json::to_string(&stored).unwrap_or_default();
    Object::String(json.into_bytes(), lopdf::StringFormat::Literal)
}

/// Settings read back from a file's catalog: the header/footer and the
/// page indexes it was on (none: every page).
pub fn from_catalog<P: Objects + ?Sized>(pdf: &P) -> Option<(HeaderFooter, Option<Vec<usize>>)> {
    let catalog = pdf.catalog().ok()?;
    let s = catalog.get(CATALOG_KEY.as_bytes()).ok()?.as_str().ok()?;
    let stored: Stored = serde_json::from_slice(s).ok()?;
    Some((stored.hf, stored.page_indexes))
}

/// Shared ownership for snapshots.
pub type Shared = Arc<HeaderFooter>;

#[cfg(test)]
mod tests {
    use lopdf::ObjectId;

    use super::*;
    use crate::content::{Res, ResKind};

    fn hf() -> HeaderFooter {
        HeaderFooter {
            texts: [
                "<<file>>".into(),
                String::new(),
                "<<date>>".into(),
                String::new(),
                "Page <<1>> of <<N>>".into(),
                "<<bates>>".into(),
            ],
            date: "10/14/2026".into(),
            file: "Return.pdf".into(),
            bates: Some(Bates {
                prefix: "ABC".into(),
                digits: 6,
                start: 100,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn tokens_and_layout() {
        let h = hf();
        let size = PageSize {
            width: 612.0,
            height: 792.0,
        };
        let f = PageFacts {
            index: 2,
            count: 10,
            bates: Some(101),
        };
        let placed = h.layout(size, &f);
        let texts: Vec<&str> = placed.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(
            texts,
            vec!["Return.pdf", "10/14/2026", "Page 3 of 10", "ABC000101"]
        );
        // Left at the margin, centred, right flush to its margin.
        assert_eq!(placed[0].x, 72.0);
        assert!((placed[2].x + placed[2].width / 2.0 - 306.0).abs() < 0.01);
        assert!((placed[3].x + placed[3].width - 540.0).abs() < 0.01);
        assert_eq!(placed[2].baseline, 792.0 - 36.0);
    }

    #[test]
    fn bates_counts_pages_in_scope() {
        let mut h = hf();
        let ids: Vec<PageId> = (0..4).map(PageId).collect();
        h.pages = Some(vec![ids[1], ids[3]]);
        assert_eq!(
            h.bates_numbers(&ids),
            vec![None, Some(100), None, Some(101)]
        );
        assert_eq!(h.bates_text(7), "ABC000007");
    }

    /// A slot is as wide as Helvetica draws it, whatever its characters.
    #[test]
    fn slots_are_as_wide_as_helvetica_draws_them() {
        let h = HeaderFooter {
            texts: ["Il “€”", "Œuvre", "Ω", "", "", ""].map(String::from),
            ..Default::default()
        };
        let size = PageSize {
            width: 612.0,
            height: 792.0,
        };
        let f = PageFacts {
            index: 0,
            count: 1,
            bates: None,
        };
        let widths: Vec<f32> = h.layout(size, &f).iter().map(|p| p.width).collect();
        // I l space “ € ” at 10 pt; Œ u v r e; Ω draws `?`.
        let at_10 = |w: f32| w * 10.0 / 1000.0;
        assert_eq!(
            widths[0],
            at_10(278.0 + 222.0 + 278.0 + 333.0 + 556.0 + 333.0)
        );
        assert_eq!(widths[1], at_10(1000.0 + 556.0 + 500.0 + 333.0 + 556.0));
        assert_eq!(widths[2], at_10(556.0));
    }

    fn placed(text: &str) -> Placed {
        Placed {
            header: true,
            text: text.into(),
            x: 10.0,
            baseline: 20.0,
            size: 9.0,
            width: 20.0,
        }
    }

    /// Text goes out in WinAnsi, one byte a character, escaped, in
    /// Helvetica named `/SheafHF`; the artifact's list is named.
    #[test]
    fn a_header_goes_out_in_win_ansi_with_a_named_list() {
        let (out, needs) = to_content(
            &[placed("Café “x” – € (a\\b)")],
            [0, 0, 0],
            &Geom::read(None, None, 0),
            &Reserved::default(),
        );
        let text = b"(Caf\xe9 \x93x\x94 \x96 \x80 \\(a\\\\b\\)) Tj";
        assert!(out.windows(text.len()).any(|w| w == text), "{out:?}");
        assert!(out.starts_with(b"Q\n/Artifact /Sh"), "{out:?}");
        assert!(!out.windows(2).any(|w| w == b"<<"), "no list inline");
        let [list, font] = &needs.0[..] else {
            panic!("{needs:?}")
        };
        assert_eq!((list.kind, font.kind), (ResKind::Properties, ResKind::Font));
        assert_eq!(font.name, b"SheafHF");
        assert_eq!(font.res, Std14::Helvetica.resource().into());
        assert_eq!(
            list.res,
            Res::dict(dictionary! {
                "Type" => "Pagination", "Subtype" => "Header", "SheafStamp" => true,
            })
        );
        let name = format!("/{} BDC", String::from_utf8_lossy(&list.name));
        assert!(String::from_utf8_lossy(&out).contains(&name), "{out:?}");
    }

    /// The same header gives the same bytes and names, twice and on two
    /// threads.
    #[test]
    fn identical_headers_write_identical_bytes() {
        fn write() -> (Vec<u8>, ResourceNeeds) {
            let hf = HeaderFooter {
                texts: [
                    "Café “x”".into(),
                    "Page <<1>> of <<N>>".into(),
                    "<<bates>>".into(),
                    "<<file>>".into(),
                    String::new(),
                    "<<date>>".into(),
                ],
                file: "report.pdf".into(),
                date: "2026-10-17".into(),
                bates: Some(Bates::default()),
                ..Default::default()
            };
            let geom = Geom::read(None, None, 90);
            let facts = PageFacts {
                index: 4,
                count: 9,
                bates: Some(17),
            };
            let placed = hf.layout(geom.shown(), &facts);
            to_content(&placed, [200, 30, 30], &geom, &Reserved::default())
        }
        let first = write();
        assert_eq!(write(), first);
        let threads: Vec<_> = (0..2).map(|_| std::thread::spawn(write)).collect();
        for t in threads {
            assert_eq!(t.join().unwrap(), first);
        }
    }

    /// A header installed and then stripped leaves the page drawing what it
    /// drew: its own stream, untouched, between the isolating `q` and `Q`.
    /// Pages share the `q`, the font and the list, and a page that has the
    /// list already names it as before.
    #[test]
    fn a_header_installed_then_stripped_leaves_the_page_as_it_was() {
        let mut pdf = Pdf::with_version("1.7");
        let body = pdf.add_object(Stream::new(dictionary! {}, b"0 0 9 9 re f".to_vec()));
        let page = || dictionary! { "Contents" => body, "Resources" => dictionary! {} };
        let (mut first, mut second) = (page(), page());
        let geom = Geom::read(None, None, 0);
        let mut res = ResObjects::default();
        for p in [&mut first, &mut second] {
            install(&mut pdf, p, &[placed("Head")], [0; 3], &geom, &mut res);
        }
        let entries = |p: &Dictionary| content::contents(p);
        let names = |p: &Dictionary, key: &[u8]| -> Vec<(Vec<u8>, Object)> {
            let resources = p.get(b"Resources").unwrap().as_dict().unwrap();
            let category = resources.get(key).unwrap().as_dict().unwrap();
            category
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        };
        assert_eq!(entries(&first)[0], entries(&second)[0], "one q");
        for key in [&b"Font"[..], b"Properties"] {
            assert_eq!(names(&first, key), names(&second, key), "one {key:?}");
        }
        let blocks = |pdf: &Pdf, p: &Dictionary| {
            let resources = p.get(b"Resources").ok();
            let (lim, known) = (&Limits::DEFAULT, DrawnForms::default());
            PageBlocks::read(pdf, &content::contents(p), resources, lim, &known).unwrap()
        };
        let found: Vec<_> = blocks(&pdf, &first)
            .pagination
            .iter()
            .map(|p| (p.kind, p.origin))
            .collect();
        assert_eq!(
            found,
            [(PaginationKind::Header, content::PagOrigin::SheafLegacy)]
        );

        assert!(strip(&mut pdf, &mut first, 0, HEADER_FOOTER).unwrap());
        assert!(!blocks(&pdf, &first).has(HEADER_FOOTER));
        let after = streams(&pdf, &first);
        let kept: Vec<&[u8]> = after.iter().map(|(_, s)| s.trim_ascii()).collect();
        assert_eq!(kept, [&b"q"[..], b"0 0 9 9 re f", b"Q"]);
        assert_eq!(after[1].0, body);
        assert_eq!(blocks(&pdf, &first).balance, Default::default());

        // Written again, the list keeps its name rather than being bumped.
        let before = names(&first, b"Properties");
        install(
            &mut pdf,
            &mut first,
            &[placed("Head")],
            [0; 3],
            &geom,
            &mut res,
        );
        assert_eq!(names(&first, b"Properties"), before);
    }

    /// A page of `streams`, each its own object in `pdf`.
    fn page_of(pdf: &mut Pdf, streams: &[&[u8]]) -> Dictionary {
        let ids: Vec<Object> = streams
            .iter()
            .map(|s| {
                pdf.add_object(Stream::new(dictionary! {}, s.to_vec()))
                    .into()
            })
            .collect();
        dictionary! { "Contents" => ids }
    }

    /// `page`'s streams, by object, decoded.
    fn streams(pdf: &Pdf, page: &Dictionary) -> Vec<(ObjectId, Vec<u8>)> {
        let content = PageContent::read(pdf, &content::contents(page), &Limits::DEFAULT).unwrap();
        let c = content.concat();
        let ids = content.ids().iter().copied();
        ids.zip((0..c.stream_count()).map(|i| c.stream(i).to_vec()))
            .collect()
    }

    /// A header sheaf drew before M10.5.
    const HEADER: &[u8] = b"/Artifact <</Type /Pagination /Subtype /Header /SheafStamp true>> BDC \
        q BT /F1 9 Tf (Head) Tj ET Q EMC";

    /// Strip takes sheaf's and Acrobat's artifacts of its kinds, and leaves
    /// the rest byte for byte: other kinds, other artifacts, and an
    /// author's own running heads (proposal 31).
    #[test]
    fn strip_removes_its_kinds_and_nothing_else() {
        let mut pdf = Pdf::with_version("1.7");
        let authored = b"/Artifact <</Type /Pagination /Subtype /Header>> BDC (author) Tj EMC";
        let content = [
            b"q 1 0 0 rg 0 0 10 10 re f Q\n".as_slice(),
            HEADER,
            b"\n/Artifact <</Type /Layout>> BDC 0 0 m 9 9 l S EMC\n\
            /Artifact <</Type /Pagination /Subtype /Footer /SheafStamp true>> BDC \
            /Span <</ActualText (x)>> BDC (nested) Tj EMC EMC\n",
            authored,
            b"\n/Artifact <</Type /Pagination /Subtype /Watermark /SheafStamp true>> BDC \
            (wm) Tj EMC\n0 0 1 rg",
        ]
        .concat();
        let mut page = page_of(&mut pdf, &[&content]);
        assert!(strip(&mut pdf, &mut page, 0, HEADER_FOOTER).unwrap());
        // Byte for byte, but for sheaf's header and footer.
        let [(_, out)] = &streams(&pdf, &page)[..] else {
            panic!("one stream")
        };
        assert_eq!(
            String::from_utf8_lossy(out),
            "q 1 0 0 rg 0 0 10 10 re f Q\n\
             \n/Artifact <</Type /Layout>> BDC 0 0 m 9 9 l S EMC\n\
             \n\
             /Artifact <</Type /Pagination /Subtype /Header>> BDC (author) Tj EMC\n\
             /Artifact <</Type /Pagination /Subtype /Watermark /SheafStamp true>> BDC \
             (wm) Tj EMC\n0 0 1 rg"
        );
        assert!(!strip(&mut pdf, &mut page, 0, HEADER_FOOTER).unwrap());
        assert!(strip(&mut pdf, &mut page, 0, WATERMARK).unwrap());
    }

    /// Unfiltered data holding ` EI ` is counted, not scanned for: a strip
    /// leaves the image as it was.
    #[test]
    fn an_inline_image_holding_ei_survives_a_strip() {
        let image: &[u8] = b"BI /W 6 /H 1 /BPC 8 /CS /G ID  EI EI\nEI";
        let content = [HEADER, b"\nq 6 0 0 1 0 0 cm ", image, b" Q"].concat();
        let mut pdf = Pdf::with_version("1.7");
        let mut page = page_of(&mut pdf, &[&content]);
        assert!(strip(&mut pdf, &mut page, 0, HEADER_FOOTER).unwrap());
        let [(_, out)] = &streams(&pdf, &page)[..] else {
            panic!("one stream")
        };
        assert_eq!(out, &[b"\nq 6 0 0 1 0 0 cm ", image, b" Q"].concat());
    }

    /// A header that opens in one stream and ends in the next goes: both
    /// streams are written anew, and the rest stay as they were.
    #[test]
    fn a_header_across_two_streams_goes() {
        let mut pdf = Pdf::with_version("1.7");
        let mut page = page_of(
            &mut pdf,
            &[
                b"q 0 0 1 rg /Artifact <</Subtype /Header /SheafStamp true>> BDC q BT /F1 9 Tf",
                b"(Head) Tj ET Q EMC 0 0 9 9 re f Q",
                b"0 g",
            ],
        );
        let before = streams(&pdf, &page);
        assert!(strip(&mut pdf, &mut page, 0, HEADER_FOOTER).unwrap());
        let after = streams(&pdf, &page);
        assert_eq!(after[0].1, b"q 0 0 1 rg ");
        assert_eq!(after[1].1, b" 0 0 9 9 re f Q");
        assert_eq!(after[2], before[2]);
        assert!(after[0].0 != before[0].0 && after[1].0 != before[1].0);
        // The old streams are as they were.
        for (id, bytes) in &before {
            let s = pdf.get_object(*id).unwrap().as_stream().unwrap();
            assert_eq!(&s.content, bytes);
        }
    }

    /// Blank pages share a stream: stripping one page leaves the stream,
    /// and so the other page, as it was.
    #[test]
    fn a_shared_stream_is_never_changed_in_place() {
        let mut pdf = Pdf::with_version("1.7");
        let shared = pdf.add_object(Stream::new(dictionary! {}, HEADER.to_vec()));
        let mut first = dictionary! { "Contents" => shared };
        let second = dictionary! { "Contents" => shared };
        assert!(strip(&mut pdf, &mut first, 0, HEADER_FOOTER).unwrap());
        assert_ne!(content::contents(&first), [shared]);
        assert_eq!(streams(&pdf, &first)[0].1, b"");
        assert_eq!(streams(&pdf, &second), [(shared, HEADER.to_vec())]);
    }

    #[test]
    fn content_that_cant_be_read_is_an_error_naming_the_page() {
        let mut pdf = Pdf::with_version("1.7");
        let bad = pdf.add_object(Stream::new(
            dictionary! { "Filter" => "JBIG2Decode" },
            b"q".to_vec(),
        ));
        let unended = [HEADER, b" /Artifact <</Subtype /Footer>> BDC (x) Tj"].concat();
        for contents in [
            vec![bad.into()],
            page_of(&mut pdf, &[&unended])
                .get(b"Contents")
                .unwrap()
                .as_array()
                .unwrap()
                .clone(),
        ] {
            let mut page = dictionary! { "Contents" => contents };
            let before = page.clone();
            let e = strip(&mut pdf, &mut page, 11, HEADER_FOOTER).unwrap_err();
            assert!(matches!(e, ContentError::Read { page: 11, .. }), "{e}");
            assert_eq!(
                e.to_string(),
                "page 12's content can't be read; nothing was changed"
            );
            assert_eq!(page, before);
        }
    }

    /// A header drawn inside the body's text object that sets the font the
    /// body text goes on in can't be taken out.
    #[test]
    fn a_header_that_cant_go_safely_is_refused() {
        let mut pdf = Pdf::with_version("1.7");
        let mut page = page_of(
            &mut pdf,
            &[b"BT /F1 12 Tf 72 700 Td \
                /Artifact <</Subtype /Header /SheafStamp true>> BDC /F2 9 Tf (Head) Tj EMC \
                (Body) Tj ET"],
        );
        let before = page.clone();
        let e = strip(&mut pdf, &mut page, 2, HEADER_FOOTER).unwrap_err();
        assert!(matches!(e, ContentError::Unsafe { page: 2, .. }), "{e}");
        assert_eq!(page, before);
    }
}
