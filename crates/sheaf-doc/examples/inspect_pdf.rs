//! Dev tool: what lopdf sees in a file — page count, encryption, object
//! count — optionally after decrypting with a password.
//!
//! Usage: cargo run -p sheaf-doc --example inspect_pdf <file.pdf> [password]

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [path, rest @ ..] = args.as_slice() else {
        anyhow::bail!("usage: inspect_pdf <file.pdf> [password]");
    };
    let raw = lopdf::Document::load(path)?;
    println!(
        "raw: encrypted={} objects={} pages={} trailer_keys={:?}",
        raw.is_encrypted(),
        raw.objects.len(),
        raw.get_pages().len(),
        raw.trailer
            .iter()
            .map(|(k, _)| String::from_utf8_lossy(k).into_owned())
            .collect::<Vec<_>>()
    );
    let pdf =
        sheaf_doc::file::load_pdf(std::path::Path::new(path), rest.first().map(|s| s.as_str()))?;
    println!(
        "decrypted: encrypted={} objects={} pages={}",
        pdf.is_encrypted(),
        pdf.objects.len(),
        pdf.get_pages().len()
    );
    for (n, id) in pdf.get_pages() {
        let d = pdf.get_dictionary(id)?;
        println!(
            "  page {n}: {id:?} keys={:?}",
            d.iter()
                .map(|(k, _)| String::from_utf8_lossy(k).into_owned())
                .collect::<Vec<_>>()
        );
    }
    Ok(())
}
