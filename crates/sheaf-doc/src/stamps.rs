//! Headers, footers and Bates numbers: text sheaf draws on pages.
//!
//! In the model it's one [`HeaderFooter`] per document: six slots (left,
//! center and right of the header and of the footer) whose text can use
//! tokens for the page number, page count, date, file name and Bates
//! number. On screen it's drawn over the pages until saved. A save
//! writes it into each page's content as marked-content artifacts, the
//! way Acrobat writes its headers and footers:
//! `/Artifact <</Type /Pagination /Subtype /Header>> BDC … EMC`.
//! [`strip`] removes those again, ours or Acrobat's, so Update and
//! Remove work on files from either.

use std::sync::Arc;

use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document as Pdf, Object, ObjectId, Stream, dictionary};
use serde::{Deserialize, Serialize};
use sheaf_core::PageSize;

use crate::annot::{PageGeom, Pt};
use crate::model::PageId;

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

    /// The texts on a page of `size` (as shown).
    pub fn layout(&self, size: PageSize, f: &PageFacts) -> Vec<Placed> {
        let fs = self.font_size;
        let mut out = Vec::new();
        for (slot, text) in self.texts.iter().enumerate() {
            let text = self.expand(text, f);
            if text.trim().is_empty() {
                continue;
            }
            let width = helvetica_width(&text, fs);
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

/// Helvetica's advance widths (AFM, per 1000 em) for printable ASCII;
/// anything else counts as an "n".
const HELVETICA: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278,
    278, // ␠ … /
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, // 0-9
    278, 278, 584, 584, 584, 556, 1015, // : … @
    667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667,
    611, 722, 667, 944, 667, 667, 611, // A-Z
    278, 278, 278, 469, 556, 333, // [ … `
    556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, 556, 556, 333, 500,
    278, 556, 500, 722, 500, 500, 500, // a-z
    334, 260, 334, 584, // { … ~
];

pub fn helvetica_width(s: &str, size: f32) -> f32 {
    s.chars()
        .map(|c| match c as u32 {
            32..=126 => HELVETICA[c as usize - 32] as f32,
            _ => 556.0,
        })
        .sum::<f32>()
        * size
        / 1000.0
}

/// Text for a WinAnsi string: printable ASCII as is, Latin-1 bytes, the
/// rest as "?".
fn win_ansi(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| match c as u32 {
            0x20..=0x7e | 0xa0..=0xff => c as u8,
            _ => b'?',
        })
        .collect()
}

/// The resource name our font goes by on a page.
pub const FONT_NAME: &str = "SheafHF";
/// Marks our artifacts so they can be found again.
const MARKER: &str = "SheafStamp";

/// The artifact stream for `placed` on a page with geometry `geom` whose
/// shown orientation includes `extra` degrees of rotation beyond the
/// file's own (sheaf's page rotation).
pub fn to_content(placed: &[Placed], color: [u8; 3], geom: &PageGeom, extra: i32) -> Vec<u8> {
    // Draw in the shown frame: map shown points to user space.
    let rotate = (geom.rotate + extra).rem_euclid(360);
    let g = PageGeom {
        media: geom.media,
        rotate,
    };
    let theta = (rotate as f32).to_radians();
    let (cs, sn) = (theta.cos(), theta.sin());
    let [r, gr, b] = color.map(|v| v as f32 / 255.0);
    let mut s = String::from("Q\n");
    for p in placed {
        let subtype = if p.header { "Header" } else { "Footer" };
        let o = g.to_user(Pt {
            x: p.x,
            y: p.baseline,
        });
        let text: String = win_ansi(&p.text)
            .iter()
            .flat_map(|&b| match b {
                b'(' | b')' | b'\\' => vec![b'\\', b],
                b => vec![b],
            })
            .map(|b| b as char)
            .collect();
        s.push_str(&format!(
            "/Artifact <</Type /Pagination /Subtype /{subtype} /{MARKER} true>> BDC\n\
             q BT {r:.3} {gr:.3} {b:.3} rg /{FONT_NAME} {:.2} Tf \
             {cs:.4} {sn:.4} {:.4} {cs:.4} {:.3} {:.3} Tm ({text}) Tj ET Q\nEMC\n",
            p.size, -sn, o.x, o.y
        ));
    }
    s.into_bytes()
}

/// Objects shared by every page's stamp: a font and the `q` that
/// isolates the page's own content.
#[derive(Clone, Copy)]
pub struct StampObjects {
    font: ObjectId,
    q: ObjectId,
}

fn objects(pdf: &mut Pdf, cache: &mut Option<StampObjects>) -> StampObjects {
    if let Some(o) = *cache {
        return o;
    }
    let font = pdf.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding",
    });
    let q = pdf.add_object(Stream::new(dictionary! {}, b"q\n".to_vec()));
    let o = StampObjects { font, q };
    *cache = Some(o);
    o
}

/// Draw `placed` on `page`, after its own content.
pub fn install(
    pdf: &mut Pdf,
    page: &mut Dictionary,
    placed: &[Placed],
    color: [u8; 3],
    geom: &PageGeom,
    extra: i32,
    cache: &mut Option<StampObjects>,
) {
    if placed.is_empty() {
        return;
    }
    let o = objects(pdf, cache);
    let mut stream = Stream::new(dictionary! {}, to_content(placed, color, geom, extra));
    let _ = stream.compress();
    let ours = pdf.add_object(stream);
    let mut entries = vec![Object::Reference(o.q)];
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
    let deref_dict = |pdf: &Pdf, o: Option<&Object>| -> Dictionary {
        o.and_then(|o| pdf.dereference(o).ok())
            .and_then(|(_, o)| o.as_dict().ok().cloned())
            .unwrap_or_default()
    };
    let mut resources = deref_dict(pdf, page.get(b"Resources").ok());
    let mut fonts = deref_dict(pdf, resources.get(b"Font").ok());
    fonts.set(FONT_NAME, Object::Reference(o.font));
    resources.set("Font", Object::Dictionary(fonts));
    page.set("Resources", Object::Dictionary(resources));
}

/// Is this a header/footer artifact's property list (ours or Acrobat's)?
fn is_pagination(props: &Object, pdf: &Pdf) -> bool {
    let Ok((_, Object::Dictionary(d))) = pdf.dereference(props) else {
        return false;
    };
    let name = |k: &[u8]| d.get(k).ok().and_then(|o| o.as_name().ok());
    d.has(MARKER.as_bytes())
        || (name(b"Type") == Some(b"Pagination")
            && matches!(name(b"Subtype"), Some(b"Header") | Some(b"Footer")))
}

/// Remove header/footer artifacts from one content stream's operations.
/// Returns whether any went.
fn strip_ops(ops: &mut Vec<Operation>, pdf: &Pdf) -> bool {
    let mut out = Vec::with_capacity(ops.len());
    // Depth of marked content inside a stripped artifact, or none.
    let mut skipping: Option<usize> = None;
    let mut removed = false;
    for op in ops.drain(..) {
        match (&mut skipping, op.operator.as_str()) {
            (Some(d), "BDC" | "BMC") => *d += 1,
            (Some(d), "EMC") => {
                if *d == 0 {
                    skipping = None;
                } else {
                    *d -= 1;
                }
            }
            (Some(_), _) => {}
            (None, "BDC")
                if op.operands.first().and_then(|o| o.as_name().ok()) == Some(b"Artifact")
                    && op.operands.get(1).is_some_and(|p| is_pagination(p, pdf)) =>
            {
                skipping = Some(0);
                removed = true;
            }
            (None, _) => out.push(op),
        }
    }
    *ops = out;
    removed
}

/// Remove header/footer artifacts from `page`'s content, rewriting the
/// streams that had any. Returns whether any were found.
pub fn strip(pdf: &mut Pdf, page: &Dictionary) -> bool {
    let ids: Vec<ObjectId> = match page.get(b"Contents") {
        Ok(Object::Reference(r)) => match pdf.get_object(*r) {
            Ok(Object::Array(a)) => a.iter().filter_map(|o| o.as_reference().ok()).collect(),
            _ => vec![*r],
        },
        Ok(Object::Array(a)) => a.iter().filter_map(|o| o.as_reference().ok()).collect(),
        _ => Vec::new(),
    };
    let mut any = false;
    for id in ids {
        let Ok(Object::Stream(s)) = pdf.get_object(id) else {
            continue;
        };
        let bytes = s
            .decompressed_content()
            .unwrap_or_else(|_| s.content.clone());
        // Cheap test before parsing.
        if !contains(&bytes, b"Artifact") {
            continue;
        }
        let Ok(mut content) = Content::decode(&bytes) else {
            continue;
        };
        if strip_ops(&mut content.operations, pdf)
            && let Ok(encoded) = content.encode()
        {
            let mut stream = Stream::new(dictionary! {}, encoded);
            let _ = stream.compress();
            pdf.objects.insert(id, Object::Stream(stream));
            any = true;
        }
    }
    any
}

/// Whether `page`'s content has header/footer artifacts.
pub fn has_artifacts(pdf: &Pdf, page: &Dictionary) -> bool {
    let streams: Vec<&Stream> = match page.get(b"Contents") {
        Ok(Object::Reference(r)) => match pdf.get_object(*r) {
            Ok(Object::Array(a)) => a
                .iter()
                .filter_map(|o| o.as_reference().ok())
                .filter_map(|id| pdf.get_object(id).ok()?.as_stream().ok())
                .collect(),
            Ok(Object::Stream(s)) => vec![s],
            _ => Vec::new(),
        },
        Ok(Object::Array(a)) => a
            .iter()
            .filter_map(|o| o.as_reference().ok())
            .filter_map(|id| pdf.get_object(id).ok()?.as_stream().ok())
            .collect(),
        _ => Vec::new(),
    };
    streams.iter().any(|s| {
        let bytes = s
            .decompressed_content()
            .unwrap_or_else(|_| s.content.clone());
        contains(&bytes, b"/Pagination") || contains(&bytes, MARKER.as_bytes())
    })
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
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
pub fn from_catalog(pdf: &Pdf) -> Option<(HeaderFooter, Option<Vec<usize>>)> {
    let catalog = pdf.catalog().ok()?;
    let s = catalog.get(CATALOG_KEY.as_bytes()).ok()?.as_str().ok()?;
    let stored: Stored = serde_json::from_slice(s).ok()?;
    Some((stored.hf, stored.page_indexes))
}

/// Shared ownership for snapshots.
pub type Shared = Arc<HeaderFooter>;

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn widths_are_helvetica() {
        assert_eq!(helvetica_width("Il", 1000.0), 278.0 + 222.0);
    }

    #[test]
    fn strip_removes_ours_and_acrobats() {
        let mut pdf = Pdf::with_version("1.7");
        let content = b"q 1 0 0 rg 0 0 10 10 re f Q\n\
            /Artifact <</Type /Pagination /Subtype /Header>> BDC BT /F1 9 Tf (Acrobat) Tj ET EMC\n\
            /Artifact <</Type /Layout>> BDC 0 0 m EMC\n\
            /Artifact <</Type /Pagination /Subtype /Footer /SheafStamp true>> BDC \
            /Span <</ActualText (x)>> BDC (nested) Tj EMC EMC\n\
            0 0 1 rg"
            .to_vec();
        let id = pdf.add_object(Stream::new(dictionary! {}, content));
        let page = dictionary! { "Contents" => id };
        assert!(has_artifacts(&pdf, &page));
        assert!(strip(&mut pdf, &page));
        let s = pdf.get_object(id).unwrap().as_stream().unwrap();
        let out = String::from_utf8(s.decompressed_content().unwrap()).unwrap();
        assert!(!out.contains("Acrobat") && !out.contains("nested"), "{out}");
        // Other artifacts and the page's own drawing stay.
        assert!(out.contains("Layout") && out.contains("re") && out.contains("rg"));
        assert!(!has_artifacts(&pdf, &page));
    }
}
