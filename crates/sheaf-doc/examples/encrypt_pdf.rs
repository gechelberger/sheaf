//! Dev tool: write a password-protected copy of a PDF (RC4 128-bit, which
//! every reader including PDFium opens) for testing the password prompt
//! and decrypt-on-save.
//!
//! Usage: cargo run -p sheaf-doc --example encrypt_pdf <in.pdf> <out.pdf> <user-password> [owner-password]

use lopdf::{Document, EncryptionState, EncryptionVersion, Object, Permissions, StringFormat};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, output, user, rest @ ..] = args.as_slice() else {
        anyhow::bail!("usage: encrypt_pdf <in.pdf> <out.pdf> <user-password> [owner-password]");
    };
    let owner = rest
        .first()
        .cloned()
        .unwrap_or_else(|| format!("{user}-owner"));
    let mut pdf = Document::load(input)?;
    if !pdf.trailer.has(b"ID") {
        // The standard security handler keys off the file /ID.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let id = Object::String(
            format!("{nanos:032x}").into_bytes(),
            StringFormat::Hexadecimal,
        );
        pdf.trailer.set("ID", Object::Array(vec![id.clone(), id]));
    }
    let state = EncryptionState::try_from(EncryptionVersion::V2 {
        document: &pdf,
        owner_password: &owner,
        user_password: user,
        key_length: 128,
        permissions: Permissions::default(),
    })?;
    pdf.encrypt(&state)?;
    pdf.save(output)?;
    println!("wrote {output} (user password {user:?})");
    Ok(())
}
