//! FDF: a PDF-syntax file of annotation dictionaries, each with `/Page`,
//! under the catalog's `/FDF /Annots` (ISO 32000 §12.7.8).

use anyhow::{Context, Result};
use lopdf::{Document as Pdf, Object, dictionary};

use super::Comments;

/// The comments in an FDF file.
pub fn read(bytes: &[u8]) -> Result<Comments> {
    // It's PDF syntax under another header; lopdf wants a PDF's.
    let mut bytes = bytes.to_vec();
    if let Some(at) = bytes.windows(5).position(|w| w == b"%FDF-") {
        bytes[at..at + 5].copy_from_slice(b"%PDF-");
    }
    let pdf = Pdf::load_mem(&bytes).context("reading the FDF")?;
    let fdf = pdf
        .trailer
        .get(b"Root")
        .ok()
        .and_then(|r| pdf.dereference(r).ok())
        .and_then(|(_, r)| r.as_dict().ok())
        .and_then(|r| r.get(b"FDF").ok())
        .and_then(|f| pdf.dereference(f).ok())
        .and_then(|(_, f)| f.as_dict().ok())
        .context("no /FDF dictionary")?
        .clone();
    let file = fdf
        .get(b"F")
        .ok()
        .and_then(|f| pdf.dereference(f).ok())
        .map(|(_, f)| match f {
            Object::Dictionary(spec) => spec
                .get(b"UF")
                .or_else(|_| spec.get(b"F"))
                .map(crate::file::pdf_text)
                .unwrap_or_default(),
            other => crate::file::pdf_text(other),
        })
        .filter(|f| !f.is_empty());
    let list = fdf
        .get(b"Annots")
        .ok()
        .and_then(|a| pdf.dereference(a).ok())
        .and_then(|(_, a)| a.as_array().ok())
        .cloned()
        .unwrap_or_default();
    let mut annots = Vec::new();
    for entry in list {
        let Ok(id) = entry.as_reference() else {
            continue;
        };
        let Ok(d) = pdf.get_dictionary(id) else {
            continue;
        };
        let page = d
            .get(b"Page")
            .ok()
            .and_then(|p| p.as_i64().ok())
            .unwrap_or(0);
        annots.push((page.max(0) as u32, id));
    }
    Ok(Comments { pdf, annots, file })
}

/// `comments` as an FDF file.
pub fn write(comments: &Comments) -> Result<Vec<u8>> {
    let mut pdf = comments.pdf.clone();
    pdf.version = "1.2".into();
    for &(page, id) in &comments.annots {
        if let Ok(d) = pdf.get_dictionary_mut(id) {
            d.set("Page", page as i64);
        }
    }
    let mut fdf = dictionary! {
        "Annots" => comments.annots.iter().map(|&(_, id)| Object::Reference(id)).collect::<Vec<_>>(),
    };
    if let Some(f) = &comments.file {
        fdf.set("F", crate::save::text_string(f));
    }
    let root = pdf.add_object(dictionary! { "FDF" => fdf });
    pdf.trailer = dictionary! { "Root" => root };
    let mut out = Vec::new();
    pdf.save_to(&mut out).context("writing the FDF")?;
    if out.starts_with(b"%PDF-") {
        out[..5].copy_from_slice(b"%FDF-");
    }
    Ok(out)
}
