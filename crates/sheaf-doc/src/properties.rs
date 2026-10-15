//! File > Properties: what a file says about itself. Read when the dialog
//! opens (the font list walks every page), not when the file does.

use std::collections::HashSet;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use lopdf::{Dictionary, Document as Pdf, Object, ObjectId};

use crate::attachments::resolve;
use crate::file::pdf_text;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Properties {
    /// The header's version, or the catalog's `/Version` when newer.
    pub version: String,
    pub title: String,
    pub author: String,
    pub subject: String,
    pub keywords: String,
    /// The program that made the original document, and the one that
    /// made the PDF.
    pub creator: String,
    pub producer: String,
    /// PDF date strings.
    pub created: String,
    pub modified: String,
    pub pages: u32,
    /// `/MarkInfo /Marked`: a tagged (accessible) PDF.
    pub tagged: bool,
    /// Fast Web View.
    pub linearized: bool,
    pub security: Option<Security>,
    pub fonts: Vec<Font>,
    /// The font walk stopped early (a huge file).
    pub fonts_truncated: bool,
    /// What the file's form is: its class, fields, scripts, usage rights
    /// and signatures.
    pub form: crate::forms::FormFacts,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Security {
    /// "Password (AES-256)" and the like.
    pub method: String,
    /// What the file allows, as (what, allowed).
    pub permissions: Vec<(&'static str, bool)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Font {
    /// `/BaseFont`, without a subset tag.
    pub name: String,
    /// `/Subtype`: Type1, TrueType, Type0, Type3…
    pub kind: String,
    pub embedding: Embedding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Embedding {
    Embedded,
    Subset,
    NotEmbedded,
}

/// Pages walked for fonts before giving up.
const FONT_PAGE_LIMIT: usize = 5_000;

/// Read the properties of the file at `path`.
pub fn read(path: &Path, password: Option<&str>) -> Result<Properties> {
    let raw = Pdf::load(path).with_context(|| format!("loading {}", path.display()))?;
    let security = raw.is_encrypted().then(|| security_of(&raw));
    let linearized = linearized(path);
    let pdf = if raw.is_encrypted() {
        crate::file::load_pdf(path, password)?
    } else {
        raw
    };
    let mut p = properties_of(&pdf);
    p.security = security;
    p.linearized = linearized;
    Ok(p)
}

/// Everything but security and Fast Web View, which need the file itself.
pub fn properties_of(pdf: &Pdf) -> Properties {
    let info = pdf
        .trailer
        .get(b"Info")
        .ok()
        .and_then(|o| resolve(pdf, o))
        .and_then(|o| o.as_dict().ok());
    let text = |k: &[u8]| {
        info.and_then(|d| d.get(k).ok())
            .and_then(|o| resolve(pdf, o))
            .map(pdf_text)
            .unwrap_or_default()
    };
    let catalog = pdf.catalog().ok();
    let mut version = pdf.version.clone();
    if let Some(v) = catalog
        .and_then(|c| c.get(b"Version").ok())
        .and_then(|o| o.as_name().ok())
    {
        let v = String::from_utf8_lossy(v).into_owned();
        if v > version {
            version = v;
        }
    }
    let tagged = catalog
        .and_then(|c| c.get(b"MarkInfo").ok())
        .and_then(|o| resolve(pdf, o))
        .and_then(|o| o.as_dict().ok())
        .and_then(|m| m.get(b"Marked").ok())
        .and_then(|o| o.as_bool().ok())
        .unwrap_or(false);
    let pages = pdf.get_pages();
    let (fonts, fonts_truncated) = fonts_in(pdf, pages.values().copied());
    Properties {
        version,
        title: text(b"Title"),
        author: text(b"Author"),
        subject: text(b"Subject"),
        keywords: text(b"Keywords"),
        creator: text(b"Creator"),
        producer: text(b"Producer"),
        created: text(b"CreationDate"),
        modified: text(b"ModDate"),
        pages: pages.len() as u32,
        tagged,
        linearized: false,
        security: None,
        fonts,
        fonts_truncated,
        form: crate::forms::read(pdf).facts,
    }
}

fn security_of(pdf: &Pdf) -> Security {
    let enc = pdf
        .trailer
        .get(b"Encrypt")
        .ok()
        .and_then(|o| resolve(pdf, o))
        .and_then(|o| o.as_dict().ok());
    let int = |k: &[u8]| {
        enc.and_then(|d| d.get(k).ok())
            .and_then(|o| o.as_i64().ok())
    };
    let filter = enc
        .and_then(|d| d.get(b"Filter").ok())
        .and_then(|o| o.as_name().ok())
        .map(|n| String::from_utf8_lossy(n).into_owned())
        .unwrap_or_default();
    let cipher = match int(b"V") {
        Some(5) => "AES-256".to_string(),
        Some(4) => {
            let aes = enc
                .and_then(|d| d.get(b"CF").ok())
                .and_then(|o| o.as_dict().ok())
                .and_then(|cf| cf.get(b"StdCF").ok())
                .and_then(|o| o.as_dict().ok())
                .and_then(|s| s.get(b"CFM").ok())
                .and_then(|o| o.as_name().ok())
                == Some(b"AESV2");
            if aes { "AES-128" } else { "RC4 128-bit" }.to_string()
        }
        Some(2) => format!("RC4 {}-bit", int(b"Length").unwrap_or(40)),
        Some(1) => "RC4 40-bit".to_string(),
        _ => "unknown cipher".to_string(),
    };
    let method = if filter == "Standard" || filter.is_empty() {
        format!("Password ({cipher})")
    } else {
        format!("{filter} ({cipher})")
    };
    // `/P`: bit 3 print, 4 modify, 5 copy, 6 annotate, 9 fill forms,
    // 10 accessibility, 11 assemble, 12 high-quality print (1-based).
    let p = int(b"P").unwrap_or(-1) as i32;
    let bit = |n: u32| p & (1 << (n - 1)) != 0;
    Security {
        method,
        permissions: vec![
            ("Printing", bit(3)),
            ("High-quality printing", bit(3) && bit(12)),
            ("Changing the document", bit(4)),
            ("Assembling pages", bit(4) || bit(11)),
            ("Copying text and images", bit(5)),
            ("Text access for accessibility", bit(5) || bit(10)),
            ("Commenting", bit(6)),
            ("Filling form fields", bit(6) || bit(9)),
        ],
    }
}

/// Fast Web View: the first object in the file is a linearization
/// dictionary.
fn linearized(path: &Path) -> bool {
    let mut head = [0u8; 2048];
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let n = f.read(&mut head).unwrap_or(0);
    head[..n].windows(11).any(|w| w == b"/Linearized")
}

/// Fonts used by these pages (and the forms they draw), once each.
fn fonts_in(pdf: &Pdf, pages: impl Iterator<Item = ObjectId>) -> (Vec<Font>, bool) {
    let mut seen_fonts: HashSet<ObjectId> = HashSet::new();
    let mut seen_res: HashSet<ObjectId> = HashSet::new();
    let mut fonts: Vec<Font> = Vec::new();
    let mut truncated = false;
    for (n, page) in pages.enumerate() {
        if n >= FONT_PAGE_LIMIT {
            truncated = true;
            break;
        }
        let Ok(page) = pdf.get_dictionary(page) else {
            continue;
        };
        if let Some(res) = page_resources(pdf, page) {
            walk_resources(pdf, res, &mut seen_fonts, &mut seen_res, &mut fonts, 0);
        }
    }
    fonts.sort_by(|a, b| a.name.cmp(&b.name).then(a.kind.cmp(&b.kind)));
    fonts.dedup();
    (fonts, truncated)
}

/// A page's resources, inherited from the page tree if need be.
fn page_resources<'a>(pdf: &'a Pdf, page: &'a Dictionary) -> Option<&'a Dictionary> {
    let mut node = page;
    for _ in 0..32 {
        if let Some(r) = node
            .get(b"Resources")
            .ok()
            .and_then(|o| resolve(pdf, o))
            .and_then(|o| o.as_dict().ok())
        {
            return Some(r);
        }
        node = node
            .get(b"Parent")
            .ok()
            .and_then(|o| resolve(pdf, o))
            .and_then(|o| o.as_dict().ok())?;
    }
    None
}

fn walk_resources(
    pdf: &Pdf,
    res: &Dictionary,
    seen_fonts: &mut HashSet<ObjectId>,
    seen_res: &mut HashSet<ObjectId>,
    out: &mut Vec<Font>,
    depth: u8,
) {
    if depth > 8 {
        return;
    }
    if let Some(fonts) = res
        .get(b"Font")
        .ok()
        .and_then(|o| resolve(pdf, o))
        .and_then(|o| o.as_dict().ok())
    {
        for (_, f) in fonts.iter() {
            if let Object::Reference(id) = f
                && !seen_fonts.insert(*id)
            {
                continue;
            }
            if let Some(d) = resolve(pdf, f).and_then(|o| o.as_dict().ok()) {
                out.push(describe_font(pdf, d));
            }
        }
    }
    // Form XObjects (and their own resources) draw text too.
    if let Some(xobjects) = res
        .get(b"XObject")
        .ok()
        .and_then(|o| resolve(pdf, o))
        .and_then(|o| o.as_dict().ok())
    {
        for (_, x) in xobjects.iter() {
            let Object::Reference(id) = x else { continue };
            if !seen_res.insert(*id) {
                continue;
            }
            let Ok(Object::Stream(s)) = pdf.get_object(*id) else {
                continue;
            };
            if s.dict.get(b"Subtype").and_then(Object::as_name).ok() != Some(b"Form") {
                continue;
            }
            if let Some(r) = s
                .dict
                .get(b"Resources")
                .ok()
                .and_then(|o| resolve(pdf, o))
                .and_then(|o| o.as_dict().ok())
            {
                walk_resources(pdf, r, seen_fonts, seen_res, out, depth + 1);
            }
        }
    }
}

fn describe_font(pdf: &Pdf, d: &Dictionary) -> Font {
    let name_of = |d: &Dictionary, k: &[u8]| {
        d.get(k)
            .ok()
            .and_then(|o| resolve(pdf, o))
            .and_then(|o| o.as_name().ok())
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .unwrap_or_default()
    };
    let kind = name_of(d, b"Subtype");
    let base = name_of(d, b"BaseFont");
    // Type0 fonts keep their descriptor on the descendant.
    let desc_owner = if kind == "Type0" {
        d.get(b"DescendantFonts")
            .ok()
            .and_then(|o| resolve(pdf, o))
            .and_then(|o| o.as_array().ok())
            .and_then(|a| a.first())
            .and_then(|o| resolve(pdf, o))
            .and_then(|o| o.as_dict().ok())
            .unwrap_or(d)
    } else {
        d
    };
    let has_file = desc_owner
        .get(b"FontDescriptor")
        .ok()
        .and_then(|o| resolve(pdf, o))
        .and_then(|o| o.as_dict().ok())
        .is_some_and(|fd| {
            [b"FontFile".as_slice(), b"FontFile2", b"FontFile3"]
                .iter()
                .any(|k| fd.has(k))
        });
    let subset = base.len() > 7
        && base.as_bytes()[6] == b'+'
        && base[..6].bytes().all(|c| c.is_ascii_uppercase());
    let embedding = if kind == "Type3" || (has_file && !subset) {
        Embedding::Embedded
    } else if has_file || subset {
        Embedding::Subset
    } else {
        Embedding::NotEmbedded
    };
    let name = if subset { base[7..].to_string() } else { base };
    Font {
        name: if name.is_empty() {
            "(unnamed)".into()
        } else {
            name
        },
        kind,
        embedding,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{Stream, dictionary};

    fn sample() -> Pdf {
        let mut pdf = Pdf::with_version("1.4");
        let file = pdf.add_object(Stream::new(dictionary! {}, vec![0u8; 4]));
        let desc = pdf.add_object(dictionary! { "Type" => "FontDescriptor", "FontFile2" => file });
        let embedded = pdf.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "TrueType", "BaseFont" => "ABCDEF+Arial-Bold", "FontDescriptor" => desc,
        });
        let helv = pdf.add_object(
            dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" },
        );
        let form = pdf.add_object(Stream::new(
            dictionary! { "Subtype" => "Form", "Resources" => dictionary! { "Font" => dictionary! { "F9" => helv } } },
            Vec::new(),
        ));
        let pages_id = pdf.new_object_id();
        let page = pdf.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id,
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => embedded }, "XObject" => dictionary! { "X1" => form } },
        });
        pdf.objects.insert(
            pages_id,
            dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 }.into(),
        );
        let info = pdf.add_object(dictionary! {
            "Title" => Object::string_literal("Engagement letter"),
            "Producer" => Object::string_literal("sheaf test"),
        });
        let catalog = pdf.add_object(dictionary! {
            "Type" => "Catalog", "Pages" => pages_id, "Version" => "1.7",
            "MarkInfo" => dictionary! { "Marked" => true },
        });
        pdf.trailer.set("Root", catalog);
        pdf.trailer.set("Info", info);
        pdf
    }

    #[test]
    fn reads_info_version_and_tags() {
        let p = properties_of(&sample());
        assert_eq!(p.title, "Engagement letter");
        assert_eq!(p.producer, "sheaf test");
        assert_eq!(p.version, "1.7");
        assert_eq!(p.pages, 1);
        assert!(p.tagged);
    }

    #[test]
    fn lists_fonts_including_forms() {
        let p = properties_of(&sample());
        assert_eq!(
            p.fonts,
            [
                Font {
                    name: "Arial-Bold".into(),
                    kind: "TrueType".into(),
                    embedding: Embedding::Subset
                },
                Font {
                    name: "Helvetica".into(),
                    kind: "Type1".into(),
                    embedding: Embedding::NotEmbedded
                },
            ]
        );
    }

    #[test]
    fn permissions_decode() {
        let mut pdf = Pdf::with_version("1.7");
        let enc = pdf.add_object(dictionary! {
            "Filter" => "Standard", "V" => 5, "R" => 6,
            // Print and copy allowed; nothing else.
            "P" => (-4096i64 | 0b100 | 0b10000),
        });
        pdf.trailer.set("Encrypt", enc);
        let s = security_of(&pdf);
        assert_eq!(s.method, "Password (AES-256)");
        let allowed: Vec<&str> = s
            .permissions
            .iter()
            .filter(|(_, ok)| *ok)
            .map(|(w, _)| *w)
            .collect();
        assert_eq!(
            allowed,
            [
                "Printing",
                "Copying text and images",
                "Text access for accessibility"
            ]
        );
    }
}
