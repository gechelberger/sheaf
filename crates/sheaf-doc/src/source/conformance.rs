//! What a file claims to conform to, from its XMP metadata: PDF/A (ISO
//! 19005) and PDF/UA (ISO 14289), each claim an attribute or an element.

use anyhow::{Context, Result};
use lopdf::LazyPdf;
use quick_xml::NsReader;
use quick_xml::events::Event;
use quick_xml::name::{LocalName, Namespace, ResolveResult};

use super::Limits;

const PDFA: &[u8] = b"http://www.aiim.org/pdfa/ns/id/";
const PDFUA: &[u8] = b"http://www.aiim.org/pdfua/ns/id/";

/// A file's conformance claims.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Conformance {
    pub pdfa: Option<PdfA>,
    /// `pdfuaid:part`: 1 for PDF/UA-1.
    pub pdfua: Option<u32>,
}

/// A PDF/A claim: `pdfaid:part` and `pdfaid:conformance`. PDF/A-2b is part
/// 2, level `B`; PDF/A-4 may give no level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PdfA {
    pub part: u32,
    pub level: Option<char>,
}

/// One of the values a claim is made of.
#[derive(Clone, Copy)]
enum Claim {
    Part,
    Level,
    UaPart,
}

impl Claim {
    fn of(ns: ResolveResult, name: LocalName) -> Option<Claim> {
        let ResolveResult::Bound(Namespace(ns)) = ns else {
            return None;
        };
        match (ns, name.as_ref()) {
            (PDFA, b"part") => Some(Claim::Part),
            (PDFA, b"conformance") => Some(Claim::Level),
            (PDFUA, b"part") => Some(Claim::UaPart),
            _ => None,
        }
    }
}

/// The claims in `pdf`'s catalog `/Metadata`; none without one.
pub(super) fn read(pdf: &LazyPdf, limits: &Limits) -> Result<Conformance> {
    let Ok(metadata) = pdf.catalog()?.get(b"Metadata") else {
        return Ok(Conformance::default());
    };
    let (_, metadata) = pdf.dereference(metadata)?;
    let xml = metadata
        .as_stream()
        .context("the catalog's /Metadata isn't a stream")?
        .get_plain_content_with_limit(limits.reader.max_decompressed)
        .context("decoding the XMP metadata")?;
    claims(&xml).context("reading the XMP metadata")
}

/// The claims in an XMP packet: the first value of each.
fn claims(xml: &[u8]) -> Result<Conformance> {
    let mut reader = NsReader::from_reader(xml);
    let mut buf = Vec::new();
    let (mut part, mut level, mut ua) = (None, None, None);
    // The claim an element being read holds, and its text so far.
    let mut open: Option<(Claim, String)> = None;
    let mut set = |claim, value: &str| -> Result<()> {
        let value = value.trim();
        match claim {
            Claim::Part if part.is_none() => part = Some(number(value)?),
            Claim::UaPart if ua.is_none() => ua = Some(number(value)?),
            Claim::Level if level.is_none() => level = Some(letter(value)?),
            _ => {}
        }
        Ok(())
    };
    loop {
        let event = reader.read_event_into(&mut buf)?;
        match &event {
            Event::Start(e) | Event::Empty(e) => {
                for attribute in e.attributes() {
                    let attribute = attribute?;
                    let (ns, name) = reader.resolver().resolve_attribute(attribute.key);
                    if let Some(claim) = Claim::of(ns, name) {
                        set(
                            claim,
                            &attribute.normalized_value(quick_xml::XmlVersion::Implicit1_0)?,
                        )?;
                    }
                }
                let (ns, name) = reader.resolver().resolve_element(e.name());
                if matches!(event, Event::Start(_)) {
                    open = Claim::of(ns, name).map(|claim| (claim, String::new()));
                }
            }
            Event::Text(t) => {
                if let Some((_, text)) = &mut open {
                    text.push_str(&t.xml10_content()?);
                }
            }
            Event::End(_) => {
                if let Some((claim, text)) = open.take() {
                    set(claim, &text)?;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(Conformance {
        pdfa: part.map(|part| PdfA { part, level }),
        pdfua: ua,
    })
}

fn number(value: &str) -> Result<u32> {
    value
        .parse()
        .with_context(|| format!("a part number {value:?} isn't a number"))
}

/// A conformance level: one letter, upper case.
fn letter(value: &str) -> Result<char> {
    let mut chars = value.chars();
    chars
        .next()
        .filter(|c| c.is_ascii_alphabetic() && chars.next().is_none())
        .map(|c| c.to_ascii_uppercase())
        .with_context(|| format!("pdfaid:conformance {value:?} isn't a letter"))
}
