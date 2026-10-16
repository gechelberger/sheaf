//! PDFs sheaf makes for pages that have no file of their own (images, blank
//! pages), and the stamps and attachments marks name, written to a
//! content-addressed cache: the same bytes give the same file, and a file
//! outlives a crash so recovery can still use it. Blank and image pages
//! go in as files, read from here into the document's store; marks name
//! their stamps and attachments by the file's stem.

use std::path::PathBuf;

use anyhow::{Context, Result};

/// `$SHEAF_CACHE_DIR`, else the platform's cache directory plus `sheaf`.
pub fn cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("SHEAF_CACHE_DIR") {
        return PathBuf::from(dir);
    }
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        home().map(|h| h.join("Library/Caches"))
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| home().map(|h| h.join(".cache")))
    };
    base.unwrap_or_else(std::env::temp_dir).join("sheaf")
}

/// A file's name in the cache: its checksum and length. Files stored
/// before sheaf used xxh3 are named by FNV-1a (16 hex digits); they keep
/// those names, which marks saved since then refer to.
fn name_of(bytes: &[u8]) -> String {
    format!("{}-{}", sheaf_core::checksum_hex(bytes), bytes.len())
}

/// The name `bytes` had before xxh3 (FNV-1a, 64 bits), to recognize one.
pub(crate) fn legacy_name_of(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}-{}", bytes.len())
}

/// Whether `stem` is a name the cache gives ([`name_of`], or
/// [`legacy_name_of`]): hex, a dash, and digits. Marks carry these names
/// from whatever file they're read from, so nothing else is taken for
/// one; a name in a crafted file can't point outside the cache.
pub fn is_stem(stem: &str) -> bool {
    let Some((hash, len)) = stem.split_once('-') else {
        return false;
    };
    matches!(hash.len(), 16 | 32)
        && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        && (1..=20).contains(&len.len())
        && len.bytes().all(|b| b.is_ascii_digit())
}

/// Write `pdf` to the cache (once) and return its path.
pub fn store(pdf: &[u8]) -> Result<PathBuf> {
    store_in(&cache_dir().join("generated"), pdf)
}

/// [`store`], into `dir`.
fn store_in(dir: &std::path::Path, pdf: &[u8]) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(format!("{}.pdf", name_of(pdf)));
    let same = |path: &PathBuf| std::fs::read(path).is_ok_and(|b| b == pdf);
    if !same(&path) {
        write_new(&path, pdf)?;
    }
    Ok(path)
}

/// Write `bytes` to `path` by way of a temporary file of this writer's
/// own, so two writing the same file at once can't trip each other up.
fn write_new(path: &PathBuf, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension(format!(
        "{}-{:x}.part",
        std::process::id(),
        fastrand::u64(..)
    ));
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        // Another writer finishing first is as good.
        if !std::fs::read(path).is_ok_and(|b| b == bytes) {
            return Err(e).with_context(|| format!("writing {}", path.display()));
        }
    }
    Ok(())
}

/// The cache file named `stem` (what [`stem_of`] gives for a stored
/// file); None if `stem` isn't a cache name ([`is_stem`]).
pub fn path_of(stem: &str) -> Option<PathBuf> {
    is_stem(stem).then(|| cache_dir().join("generated").join(format!("{stem}.pdf")))
}

/// A stored file's name without `.pdf`: how marks refer to one.
pub fn stem_of(path: &std::path::Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Write `pdf` to the cache as `stem`, for a file recovered from a saved
/// document whose original isn't on this machine.
pub fn store_as(stem: &str, pdf: &[u8]) -> Result<PathBuf> {
    let path = path_of(stem).with_context(|| format!("{stem:?} isn't a cache name"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    write_new(&path, pdf)?;
    Ok(path)
}

/// Keep any file's bytes in the cache (an attachment until it's saved
/// into the document), named by content; returns the name.
pub fn store_blob(bytes: &[u8]) -> Result<String> {
    let dir = cache_dir().join("files");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let stem = name_of(bytes);
    let path = dir.join(&stem);
    if !std::fs::read(&path).is_ok_and(|b| b == bytes) {
        write_new(&path, bytes)?;
    }
    Ok(stem)
}

/// Write `bytes` to the cache as `stem`, for an attachment recovered from
/// a saved document: under the name its mark has, as long as that's the
/// name of these bytes (now or before xxh3). Anything else could put one
/// file's bytes in another's place, for every document that has it.
pub fn store_blob_as(stem: &str, bytes: &[u8]) -> Result<PathBuf> {
    if stem != name_of(bytes) && stem != legacy_name_of(bytes) {
        anyhow::bail!("the file embedded isn't the one {stem:?} names");
    }
    let path = blob_path(stem).with_context(|| format!("{stem:?} isn't a cache name"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    write_new(&path, bytes)?;
    Ok(path)
}

/// Where the cache keeps the file named `stem` (see [`store_blob`]).
/// None if `stem` isn't a cache name ([`is_stem`]).
pub fn blob_path(stem: &str) -> Option<PathBuf> {
    is_stem(stem).then(|| cache_dir().join("files").join(stem))
}

/// One blank page of `width` × `height` points.
pub fn blank_pdf(width: f32, height: f32) -> Result<Vec<u8>> {
    blank_pages(width, height, 1)
}

/// `count` blank pages of `width` × `height` points.
pub fn blank_pages(width: f32, height: f32, count: usize) -> Result<Vec<u8>> {
    use lopdf::{Document as Pdf, Object, Stream, dictionary};
    let mut pdf = Pdf::with_version("1.5");
    let pages_id = pdf.new_object_id();
    let content = pdf.add_object(Stream::new(dictionary! {}, Vec::new()));
    let kids: Vec<Object> = (0..count.max(1))
        .map(|_| {
            pdf.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "MediaBox" => vec![0.into(), 0.into(), Object::from(width), Object::from(height)],
                "Contents" => content,
                "Resources" => dictionary! {},
            })
            .into()
        })
        .collect();
    let n = kids.len() as i64;
    pdf.objects.insert(
        pages_id,
        dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => n }.into(),
    );
    let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    pdf.trailer.set("Root", catalog);
    let mut out = Vec::new();
    pdf.save_to(&mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_once_by_content() {
        // A folder of its own: other tests use the real cache meanwhile.
        let dir = std::env::temp_dir().join(format!("sheaf-gen-{}", std::process::id()));
        let a = blank_pdf(612.0, 792.0).unwrap();
        let b = blank_pdf(595.0, 842.0).unwrap();
        let pa = store_in(&dir, &a).unwrap();
        assert_eq!(store_in(&dir, &a).unwrap(), pa);
        let pb = store_in(&dir, &b).unwrap();
        assert_ne!(pa, pb);
        assert_eq!(std::fs::read(&pa).unwrap(), a);
        let doc = lopdf::Document::load(&pb).unwrap();
        assert_eq!(doc.get_pages().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_cache_names_are_paths() {
        let bytes = b"notes";
        assert!(is_stem(&name_of(bytes)));
        assert!(is_stem(&legacy_name_of(bytes)));
        assert_eq!(legacy_name_of(b""), "cbf29ce484222325-0");
        for bad in [
            "",
            "../../.bashrc",
            "/etc/passwd",
            "0123456789abcdef-12/../x",
            "0123456789ABCDEF-12",
            "0123456789abcdef",
            "0123456789abcdef-",
            "0123456789abcde-12",
            "C:\\x",
        ] {
            assert!(!is_stem(bad), "{bad:?}");
            assert!(
                path_of(bad).is_none() && blob_path(bad).is_none(),
                "{bad:?}"
            );
            assert!(store_as(bad, b"%PDF").is_err(), "{bad:?}");
        }
        // A recovered file is kept only under its own name.
        assert!(store_blob_as(&name_of(b"other"), bytes).is_err());
        assert!(store_blob_as("../x", bytes).is_err());
    }
}
