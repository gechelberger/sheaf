//! Watermarks and backgrounds: text or an image drawn across pages, with
//! opacity, rotation and position, in front of the page's content
//! (watermark) or behind it (background).
//!
//! They're written into a copy of each page (see `sheaf_ops::derive`), as
//! marked-content artifacts the way Acrobat writes them:
//! `/Artifact <</Type /Pagination /Subtype /Watermark>> BDC … EMC`, or
//! `/Background`. The settings ride along in the property list, so the
//! dialog can offer them again after a reopen.

use std::path::PathBuf;

use anyhow::{Context, Result};
use lopdf::content::Content;
use lopdf::{Dictionary, Document as Pdf, Object, Stream, dictionary};
use serde::{Deserialize, Serialize};
use sheaf_core::PageSize;

use crate::annot::Pt;
use crate::content::{Font, Std14, write_string};
use crate::geom::Geom;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mark {
    Text {
        text: String,
        font_size: f32,
        color: [u8; 3],
    },
    /// An image file, `scale` of the page's width wide.
    Image { path: PathBuf, scale: f32 },
}

/// Where on the page: 0, 1, 2 for left/top, center, right/bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    pub horizontal: u8,
    pub vertical: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Watermark {
    pub mark: Mark,
    /// 0–1.
    pub opacity: f32,
    /// Degrees counterclockwise, as the page is shown.
    pub rotation: f32,
    pub anchor: Anchor,
    /// Behind the page's content (a background) rather than in front.
    pub behind: bool,
}

impl Default for Watermark {
    fn default() -> Self {
        Self {
            mark: Mark::Text {
                text: "DRAFT".into(),
                font_size: 72.0,
                color: [200, 30, 30],
            },
            opacity: 0.3,
            rotation: 45.0,
            anchor: Anchor {
                horizontal: 1,
                vertical: 1,
            },
            behind: false,
        }
    }
}

impl Watermark {
    /// Whether some of its text can't be drawn in [`FONT`]: it shows as
    /// `?` in the file.
    pub fn unencodable(&self) -> bool {
        match &self.mark {
            Mark::Text { text, .. } => FONT.encode(text).1.0 > 0,
            Mark::Image { .. } => false,
        }
    }

    /// The artifact subtypes this one's kind is written as.
    pub fn kinds(&self) -> &'static [crate::content::PaginationKind] {
        if self.behind {
            crate::stamps::BACKGROUND
        } else {
            crate::stamps::WATERMARK
        }
    }
}

/// Space kept from the edge for a mark placed at a side.
const EDGE: f32 = 36.0;
const GS: &str = "SheafWMgs";
const FONT_NAME: &str = "SheafWMf";
const IMAGE: &str = "SheafWMim";
/// What a text watermark is drawn in.
pub const FONT: Std14 = Std14::HelveticaBold;

/// The mark's size before rotation, in points, on a page `size` wide.
fn extent(wm: &Watermark, size: PageSize, image: Option<(u32, u32)>) -> (f32, f32) {
    match &wm.mark {
        Mark::Text {
            text, font_size, ..
        } => (FONT.width(text, *font_size), font_size * 0.72),
        Mark::Image { scale, .. } => {
            let w = size.width * scale.clamp(0.01, 2.0);
            let (iw, ih) = image.unwrap_or((1, 1));
            (w, w * ih.max(1) as f32 / iw.max(1) as f32)
        }
    }
}

/// Draw `wm` on `page`, laid out on the page as it shows with geometry
/// `geom`, its rotation the page's, absolute.
pub fn install(pdf: &mut Pdf, page: &mut Dictionary, wm: &Watermark, geom: &Geom) -> Result<()> {
    let size = geom.shown();
    let image = match &wm.mark {
        Mark::Image { path, .. } => {
            let img = sheaf_image::io::read(path)?
                .into_iter()
                .next()
                .context("the image has no pages")?;
            let dims = (img.width, img.height);
            Some((crate::images::image_stream(&img)?, dims))
        }
        Mark::Text { .. } => None,
    };
    let (w, h) = extent(wm, size, image.as_ref().map(|(_, d)| *d));
    // The mark's frame in shown space (y down): x along the text,
    // y up from it.
    let a = wm.rotation.to_radians();
    let ux = (a.cos(), -a.sin());
    let uy = (-a.sin(), -a.cos());
    // Its rotated bounding box, to keep a side-placed mark on the page.
    let bw = (w * a.cos()).abs() + (h * a.sin()).abs();
    let bh = (w * a.sin()).abs() + (h * a.cos()).abs();
    let place = |slot: u8, page: f32, extent: f32| match slot {
        0 => EDGE + extent / 2.0,
        2 => page - EDGE - extent / 2.0,
        _ => page / 2.0,
    };
    let cx = place(wm.anchor.horizontal, size.width, bw);
    let cy = place(wm.anchor.vertical, size.height, bh);
    // Bottom-left of the mark (text: its baseline start).
    let ox = cx - ux.0 * w / 2.0 - uy.0 * h / 2.0;
    let oy = cy - ux.1 * w / 2.0 - uy.1 * h / 2.0;
    // Shown space to user space is affine.
    let to_user = geom.from_view();
    let o = to_user.apply(Pt { x: 0.0, y: 0.0 });
    let ex = to_user.apply(Pt { x: 1.0, y: 0.0 });
    let ey = to_user.apply(Pt { x: 0.0, y: 1.0 });
    let map = |(vx, vy): (f32, f32)| {
        (
            (ex.x - o.x) * vx + (ey.x - o.x) * vy,
            (ex.y - o.y) * vx + (ey.y - o.y) * vy,
        )
    };
    let origin = to_user.apply(Pt { x: ox, y: oy });
    let (xa, xb) = map(ux);
    let (ya, yb) = map(uy);
    let settings = serde_json::to_string(wm).unwrap_or_default();
    let subtype = if wm.behind { "Background" } else { "Watermark" };
    let body = match &wm.mark {
        Mark::Text {
            text,
            font_size,
            color,
        } => {
            let [r, gr, b] = color.map(|v| v as f32 / 255.0);
            let mut body = format!(
                "{xa:.4} {xb:.4} {ya:.4} {yb:.4} {:.3} {:.3} cm \
                 BT {r:.3} {gr:.3} {b:.3} rg /{FONT_NAME} {font_size:.2} Tf 0 0 Td ",
                origin.x, origin.y
            )
            .into_bytes();
            write_string(&mut body, &FONT.encode(text).0);
            body.extend_from_slice(b" Tj ET");
            body
        }
        Mark::Image { .. } => format!(
            "{:.4} {:.4} {:.4} {:.4} {:.3} {:.3} cm /{IMAGE} Do",
            xa * w,
            xb * w,
            ya * h,
            yb * h,
            origin.x,
            origin.y
        )
        .into_bytes(),
    };
    let mut mark = format!("/{GS} gs ").into_bytes();
    mark.extend(body);
    // The settings go in as a string object in the property list.
    let props = dictionary! {
        "Type" => "Pagination",
        "Subtype" => Object::Name(subtype.as_bytes().to_vec()),
        "SheafStamp" => true,
        "SheafSettings" => Object::String(settings.into_bytes(), lopdf::StringFormat::Literal),
    };
    let ops = Content {
        operations: vec![
            lopdf::content::Operation::new(
                "BDC",
                vec![
                    Object::Name(b"Artifact".to_vec()),
                    Object::Dictionary(props),
                ],
            ),
            lopdf::content::Operation::new("q", vec![]),
        ],
    };
    let mut bytes = ops.encode()?;
    bytes.extend(mark);
    bytes.extend_from_slice(b"\nQ\nEMC\n");

    // Resources: transparency, font or image.
    let deref = |pdf: &Pdf, o: Option<&Object>| -> Dictionary {
        o.and_then(|o| pdf.dereference(o).ok())
            .and_then(|(_, o)| o.as_dict().ok().cloned())
            .unwrap_or_default()
    };
    let mut resources = deref(pdf, page.get(b"Resources").ok());
    let opacity = wm.opacity.clamp(0.0, 1.0);
    let gs = pdf.add_object(dictionary! {
        "Type" => "ExtGState", "CA" => Object::from(opacity), "ca" => Object::from(opacity),
    });
    let mut gstates = deref(pdf, resources.get(b"ExtGState").ok());
    gstates.set(GS, gs);
    resources.set("ExtGState", Object::Dictionary(gstates));
    match image {
        Some((stream, _)) => {
            let im = pdf.add_object(stream);
            let mut xo = deref(pdf, resources.get(b"XObject").ok());
            xo.set(IMAGE, im);
            resources.set("XObject", Object::Dictionary(xo));
        }
        None => {
            // The dictionary the text was encoded for.
            let font = pdf.add_object(FONT.resource().objects[0].clone());
            let mut fonts = deref(pdf, resources.get(b"Font").ok());
            fonts.set(FONT_NAME, font);
            resources.set("Font", Object::Dictionary(fonts));
        }
    }
    page.set("Resources", Object::Dictionary(resources));

    let mut current: Vec<Object> = match page.get(b"Contents").ok().cloned() {
        None => Vec::new(),
        Some(Object::Array(a)) => a,
        Some(Object::Reference(r)) => match pdf.get_object(r) {
            Ok(Object::Array(a)) => a.clone(),
            _ => vec![Object::Reference(r)],
        },
        Some(other) => vec![other],
    };
    let entries = if wm.behind {
        // Ours first: balanced q/Q, so the page's content starts clean.
        let mut s = Stream::new(dictionary! {}, bytes);
        let _ = s.compress();
        let mut v = vec![Object::Reference(pdf.add_object(s))];
        v.append(&mut current);
        v
    } else {
        // The page's content isolated in q … Q, then ours.
        let q = pdf.add_object(Stream::new(dictionary! {}, b"q\n".to_vec()));
        let mut ours = b"Q\n".to_vec();
        ours.extend(bytes);
        let mut s = Stream::new(dictionary! {}, ours);
        let _ = s.compress();
        let mut v = vec![Object::Reference(q)];
        v.append(&mut current);
        v.push(Object::Reference(pdf.add_object(s)));
        v
    };
    page.set("Contents", Object::Array(entries));
    Ok(())
}

/// The settings of the first watermark (or background, with `behind`)
/// on `page`, if sheaf wrote one.
pub fn read(pdf: &Pdf, page: &Dictionary, behind: bool) -> Option<Watermark> {
    let subtype: &[u8] = if behind { b"Background" } else { b"Watermark" };
    let ids: Vec<lopdf::ObjectId> = match page.get(b"Contents").ok()? {
        Object::Reference(r) => match pdf.get_object(*r).ok()? {
            Object::Array(a) => a.iter().filter_map(|o| o.as_reference().ok()).collect(),
            _ => vec![*r],
        },
        Object::Array(a) => a.iter().filter_map(|o| o.as_reference().ok()).collect(),
        _ => return None,
    };
    for id in ids {
        let Ok(s) = pdf.get_object(id).and_then(|o| o.as_stream()) else {
            continue;
        };
        let bytes = s
            .decompressed_content()
            .unwrap_or_else(|_| s.content.clone());
        if !bytes.windows(13).any(|w| w == b"SheafSettings") {
            continue;
        }
        let Ok(content) = Content::decode(&bytes) else {
            continue;
        };
        for op in content.operations {
            if op.operator == "BDC"
                && let Some(Object::Dictionary(d)) = op.operands.get(1)
                && d.get(b"Subtype").ok().and_then(|o| o.as_name().ok()) == Some(subtype)
                && let Ok(json) = d.get(b"SheafSettings").and_then(|o| o.as_str())
                && let Ok(wm) = serde_json::from_slice(json)
            {
                return Some(wm);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{PagOrigin, PageBlocks, PaginationKind};

    fn page(pdf: &mut Pdf) -> Dictionary {
        let c = pdf.add_object(Stream::new(
            dictionary! {},
            b"0 0 1 rg 10 10 50 50 re f".to_vec(),
        ));
        dictionary! {
            "Type" => "Page",
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Contents" => c,
        }
    }

    fn blocks(pdf: &Pdf, page: &Dictionary) -> PageBlocks {
        let contents = crate::content::contents(page);
        let resources = page.get(b"Resources").ok();
        let (lim, known) = (&crate::source::Limits::DEFAULT, Default::default());
        PageBlocks::read(pdf, &contents, resources, lim, &known).unwrap()
    }

    /// The page's streams, decoded, joined by `|`.
    fn content_bytes(pdf: &Pdf, page: &Dictionary) -> Vec<u8> {
        let streams: Vec<Vec<u8>> = page
            .get(b"Contents")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|o| {
                let s = pdf
                    .get_object(o.as_reference().unwrap())
                    .unwrap()
                    .as_stream()
                    .unwrap();
                s.decompressed_content().unwrap_or(s.content.clone())
            })
            .collect();
        streams.join(&b'|')
    }

    fn all_content(pdf: &Pdf, page: &Dictionary) -> String {
        String::from_utf8_lossy(&content_bytes(pdf, page)).into_owned()
    }

    /// The text is in the WinAnsi of the Helvetica-Bold it names, one byte
    /// a character, escaped: "é" is 0xE9, not UTF-8's two bytes.
    #[test]
    fn text_is_encoded_for_its_font() {
        let mut pdf = Pdf::with_version("1.7");
        let mut p = page(&mut pdf);
        let wm = Watermark {
            mark: Mark::Text {
                text: "Café (é)".into(),
                font_size: 40.0,
                color: [0; 3],
            },
            ..Default::default()
        };
        install(&mut pdf, &mut p, &wm, &Geom::read(None, None, 0)).unwrap();
        let bytes = content_bytes(&pdf, &p);
        let drawn = b"(Caf\xe9 \\(\xe9\\)) Tj";
        assert!(bytes.windows(drawn.len()).any(|w| w == drawn), "{bytes:?}");
        // The settings keep the text as it was typed.
        assert_eq!(read(&pdf, &p, false), Some(wm));
    }

    #[test]
    fn text_watermark_in_front_and_read_back() {
        let mut pdf = Pdf::with_version("1.7");
        let mut p = page(&mut pdf);
        let wm = Watermark::default();
        install(&mut pdf, &mut p, &wm, &Geom::read(None, None, 0)).unwrap();
        let text = all_content(&pdf, &p);
        // Page content first, the mark after.
        let at_page = text.find("re f").unwrap();
        let at_mark = text.find("(DRAFT) Tj").unwrap();
        assert!(at_page < at_mark, "{text}");
        assert!(text.contains("/Watermark"));
        assert_eq!(read(&pdf, &p, false), Some(wm));
        assert_eq!(read(&pdf, &p, true), None);
        // A watermark of sheaf's before M10.5.
        let found: Vec<_> = blocks(&pdf, &p)
            .pagination
            .iter()
            .map(|p| (p.kind, p.origin))
            .collect();
        assert_eq!(found, [(PaginationKind::Watermark, PagOrigin::SheafLegacy)]);
        assert!(crate::stamps::strip(&mut pdf, &mut p, 0, crate::stamps::WATERMARK).unwrap());
        assert_eq!(read(&pdf, &p, false), None);
        assert!(blocks(&pdf, &p).pagination.is_empty());
    }

    #[test]
    fn background_goes_first() {
        let mut pdf = Pdf::with_version("1.7");
        let mut p = page(&mut pdf);
        let wm = Watermark {
            behind: true,
            ..Default::default()
        };
        install(&mut pdf, &mut p, &wm, &Geom::read(None, None, 0)).unwrap();
        let text = all_content(&pdf, &p);
        assert!(
            text.find("(DRAFT)").unwrap() < text.find("re f").unwrap(),
            "{text}"
        );
        assert!(text.contains("/Background"));
        assert!(read(&pdf, &p, true).is_some());
        let found: Vec<_> = blocks(&pdf, &p)
            .pagination
            .iter()
            .map(|p| (p.kind, p.origin))
            .collect();
        assert_eq!(
            found,
            [(PaginationKind::Background, PagOrigin::SheafLegacy)]
        );
    }
}
