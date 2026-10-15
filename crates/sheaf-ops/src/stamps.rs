//! The stamp library (M9 decision 6): collections of stamps, each a PDF in
//! the config folder's `stamps`, one stamp per page, the way Acrobat keeps
//! its Stamps folder. Sharing a collection is sending its PDF; an Acrobat
//! custom-stamp file imports the same way.
//!
//! The PDF side (names, pages, images) is `sheaf_doc::stamp_lib`. This is
//! the folder: listing it, and editing a collection by writing it anew.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use lopdf::Document as Pdf;
use serde::{Deserialize, Serialize};
use sheaf_doc::stamp_lib::{self, StampPage};

/// A collection in the library.
#[derive(Debug, Clone, PartialEq)]
pub struct Collection {
    pub path: PathBuf,
    pub title: String,
    pub stamps: Vec<StampPage>,
}

impl Collection {
    /// The file's name in the library, which is how a stamp is referred
    /// to (with its own name).
    pub fn file(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// A library stamp, by collection file and name: what Recent and
/// Favorites keep.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StampRef {
    pub file: String,
    pub name: String,
}

/// The library folder.
pub fn dir() -> PathBuf {
    crate::dirs::config_dir().join("stamps")
}

/// Every readable collection, by title. One that can't be read is skipped,
/// with a line in the log.
pub fn list() -> Vec<Collection> {
    list_in(&dir())
}

pub fn list_in(dir: &Path) -> Vec<Collection> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Collection> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")))
        .filter_map(|p| match load(&p) {
            Ok(c) => Some(c),
            Err(e) => {
                log::warn!("stamp collection {}: {e:#}", p.display());
                None
            }
        })
        .collect();
    out.sort_by_key(|c| c.title.to_lowercase());
    out
}

pub fn load(path: &Path) -> Result<Collection> {
    let pdf = Pdf::load(path).with_context(|| format!("reading {}", path.display()))?;
    let (title, stamps) = stamp_lib::read_collection(&pdf);
    Ok(Collection {
        path: path.to_path_buf(),
        title: title.unwrap_or_else(|| {
            path.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        }),
        stamps,
    })
}

/// Stamp `index` of the collection at `path`, alone: what goes into a
/// document when it's placed.
pub fn stamp_pdf(path: &Path, index: usize) -> Result<Vec<u8>> {
    let pdf = Pdf::load(path).with_context(|| format!("reading {}", path.display()))?;
    stamp_lib::one_page(&pdf, index)
}

impl StampRef {
    /// One of the dynamic stamps sheaf comes with, by its label.
    pub fn preset(label: &str) -> StampRef {
        StampRef {
            file: String::new(),
            name: label.to_string(),
        }
    }

    /// The template, when this is one of the dynamic presets.
    pub fn preset_template(&self) -> Option<&'static str> {
        if !self.file.is_empty() {
            return None;
        }
        PRESETS
            .iter()
            .find(|(label, _)| *label == self.name)
            .map(|(_, t)| *t)
    }
}

/// Find a stamp by reference: its collection and its index there.
pub fn find(library: &[Collection], r: &StampRef) -> Option<(usize, usize)> {
    let c = library.iter().position(|c| c.file() == r.file)?;
    let s = library[c].stamps.iter().position(|s| s.name == r.name)?;
    Some((c, s))
}

/// Every stamp of a collection alone, as one-page PDFs, in order.
pub fn each_stamp(c: &Collection) -> Result<Vec<Vec<u8>>> {
    Ok(pages(c)?.into_iter().map(|(_, _, pdf)| pdf).collect())
}

/// A stamp as a collection is rebuilt from: its name, dynamic text and
/// page (a one-page PDF).
type Page = (String, Option<String>, Vec<u8>);

/// Every stamp of a collection, as [`Page`]s.
fn pages(c: &Collection) -> Result<Vec<Page>> {
    let pdf = Pdf::load(&c.path).with_context(|| format!("reading {}", c.path.display()))?;
    c.stamps
        .iter()
        .enumerate()
        .map(|(i, s)| {
            Ok((
                s.name.clone(),
                s.text.clone(),
                stamp_lib::one_page(&pdf, i)?,
            ))
        })
        .collect()
}

/// Write a collection anew (through a temporary file).
fn write(path: &Path, title: &str, stamps: &[Page]) -> Result<()> {
    let bytes = stamp_lib::build_collection(title, stamps)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let tmp = path.with_extension("pdf.part");
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// A file name for a new collection titled `title` in `dir`, not taken.
fn new_path(dir: &Path, title: &str) -> PathBuf {
    let base: String = title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim()
        .to_string();
    let base = if base.is_empty() {
        "Stamps".into()
    } else {
        base
    };
    let mut path = dir.join(format!("{base}.pdf"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{base} {n}.pdf"));
        n += 1;
    }
    path
}

/// A name for a new stamp in `c`, from `want`, not taken.
fn free_name(c: Option<&Collection>, want: &str) -> String {
    let want = if want.trim().is_empty() {
        "Stamp"
    } else {
        want.trim()
    };
    let taken = |n: &str| c.is_some_and(|c| c.stamps.iter().any(|s| s.name == n));
    if !taken(want) {
        return want.to_string();
    }
    (2..)
        .map(|i| format!("{want} {i}"))
        .find(|n| !taken(n))
        .expect("some name is free")
}

/// Add a stamp (a one-page PDF) to the collection titled `title` in `dir`,
/// making the collection if there's none. Returns the collection's path
/// and the stamp's name, which gets a number if `name` is taken.
pub fn add_in(dir: &Path, title: &str, name: &str, stamp: Vec<u8>) -> Result<(PathBuf, String)> {
    let existing = list_in(dir).into_iter().find(|c| c.title == title);
    let name = free_name(existing.as_ref(), name);
    let (path, mut stamps) = match &existing {
        Some(c) => (c.path.clone(), pages(c)?),
        None => (new_path(dir, title), Vec::new()),
    };
    stamps.push((name.clone(), None, stamp));
    write(&path, title, &stamps)?;
    Ok((path, name))
}

/// Add from Image…: a picture (transparency kept) as a stamp.
pub fn add_image(dir: &Path, title: &str, image: &Path) -> Result<(PathBuf, String)> {
    let name = image
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    add_in(dir, title, &name, stamp_lib::image_stamp(image)?)
}

/// Add from PDF Page…: page `index` (from 0) of a PDF as a stamp.
pub fn add_pdf_page(
    dir: &Path,
    title: &str,
    pdf: &Path,
    index: usize,
    password: Option<&str>,
) -> Result<(PathBuf, String)> {
    let doc = sheaf_doc::file::load_pdf(pdf, password)?;
    let name = format!(
        "{} p{}",
        pdf.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        index + 1
    );
    add_in(dir, title, &name, stamp_lib::one_page(&doc, index)?)
}

/// Rename stamp `index` of the collection at `path`.
pub fn rename(path: &Path, index: usize, to: &str) -> Result<String> {
    let c = load(path)?;
    let mut stamps = pages(&c)?;
    if index >= stamps.len() {
        bail!("no such stamp");
    }
    let others = Collection {
        stamps: c
            .stamps
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != index)
            .map(|(_, s)| s.clone())
            .collect(),
        ..c.clone()
    };
    let name = free_name(Some(&others), to);
    stamps[index].0 = name.clone();
    write(path, &c.title, &stamps)?;
    Ok(name)
}

/// Set (or, with an empty one, clear) the dynamic text stamp `index` puts
/// over its page.
pub fn set_text(path: &Path, index: usize, text: &str) -> Result<()> {
    let c = load(path)?;
    let mut stamps = pages(&c)?;
    let s = stamps.get_mut(index).context("no such stamp")?;
    s.1 = (!text.trim().is_empty()).then(|| text.to_string());
    write(path, &c.title, &stamps)
}

/// Remove stamp `index`; the last one takes the collection with it.
pub fn remove(path: &Path, index: usize) -> Result<()> {
    let c = load(path)?;
    let mut stamps = pages(&c)?;
    if index >= stamps.len() {
        bail!("no such stamp");
    }
    stamps.remove(index);
    if stamps.is_empty() {
        std::fs::remove_file(path).with_context(|| format!("removing {}", path.display()))?;
        return Ok(());
    }
    write(path, &c.title, &stamps)
}

/// Rename a collection (its title; the file keeps its name).
pub fn retitle(path: &Path, title: &str) -> Result<()> {
    let c = load(path)?;
    let stamps = pages(&c)?;
    write(path, title.trim(), &stamps)
}

/// Import a collection: a stamp PDF (sheaf's, or Acrobat's) copied into
/// the library. Returns where it went.
pub fn import_in(dir: &Path, from: &Path) -> Result<PathBuf> {
    let c = load(from)?;
    if c.stamps.is_empty() {
        bail!("{} has no pages to use as stamps", from.display());
    }
    // Written anew in sheaf's shape, so its names read the same here.
    let stamps = pages(&c)?;
    let path = new_path(dir, &c.title);
    write(&path, &c.title, &stamps)?;
    Ok(path)
}

/// Export a collection: its PDF, copied to `to`.
pub fn export(path: &Path, to: &Path) -> Result<()> {
    std::fs::copy(path, to)
        .with_context(|| format!("copying {} to {}", path.display(), to.display()))?;
    Ok(())
}

/// Favorites, kept in the library folder (they're the user's, like the
/// stamps; Recent is in sheaf's state).
pub fn favorites_in(dir: &Path) -> Vec<StampRef> {
    std::fs::read(dir.join("favorites.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn set_favorites_in(dir: &Path, favs: &[StampRef]) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join("favorites.json");
    std::fs::write(&path, serde_json::to_vec_pretty(favs)?)
        .with_context(|| format!("writing {}", path.display()))
}

/// Fill in a dynamic stamp's tokens: `<<name>>`, `<<initials>>`,
/// `<<date>>` and `<<time>>`, as of `now`.
pub fn fill(template: &str, name: &str, initials: &str, date: &str, time: &str) -> String {
    template
        .replace("<<name>>", name)
        .replace("<<initials>>", initials)
        .replace("<<date>>", date)
        .replace("<<time>>", time)
}

/// The dynamic text stamps sheaf comes with: (label, template).
pub const PRESETS: [(&str, &str); 8] = [
    ("Approved", "APPROVED\n<<initials>> <<date>>"),
    ("Reviewed", "REVIEWED\n<<initials>> <<date>>"),
    ("Received", "RECEIVED\n<<initials>> <<date>>"),
    ("Prepared", "PREPARED\n<<initials>> <<date>>"),
    ("Approved (name)", "APPROVED\n<<name>> <<date>>"),
    ("Reviewed (name)", "REVIEWED\n<<name>> <<date>>"),
    ("Received (name)", "RECEIVED\n<<name>> <<date>>"),
    ("Prepared (name)", "PREPARED\n<<name>> <<date>>"),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "sheaf-stamps-{}-{:x}",
            std::process::id(),
            fastrand::u64(..)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn png(dir: &Path, name: &str) -> PathBuf {
        let mut img = image::RgbaImage::new(40, 20);
        for (x, _, p) in img.enumerate_pixels_mut() {
            // Left half red, right half clear.
            *p = if x < 20 {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 0, 0, 0])
            };
        }
        let path = dir.join(name);
        img.save(&path).unwrap();
        path
    }

    #[test]
    fn a_collection_is_made_edited_and_shared() {
        let lib = tempdir().join("stamps");
        let pics = tempdir();
        let (path, name) = add_image(&lib, "Signatures", &png(&pics, "Pat.png")).unwrap();
        assert_eq!(name, "Pat");
        // The same name again gets a number.
        let (same, name2) = add_image(&lib, "Signatures", &png(&pics, "Pat.png")).unwrap();
        assert_eq!((same.clone(), name2.as_str()), (path.clone(), "Pat 2"));
        let all = list_in(&lib);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].title, "Signatures");
        assert_eq!(all[0].stamps.len(), 2);
        // 40 × 20 pixels at 72 to the inch.
        assert_eq!(all[0].stamps[0].size.width, 40.0);

        assert_eq!(rename(&path, 1, "Lee").unwrap(), "Lee");
        set_text(&path, 1, "<<initials>> <<date>>").unwrap();
        let c = load(&path).unwrap();
        assert_eq!(c.stamps[1].name, "Lee");
        assert_eq!(c.stamps[1].text.as_deref(), Some("<<initials>> <<date>>"));
        let r = StampRef {
            file: c.file(),
            name: "Lee".into(),
        };
        assert_eq!(find(&list_in(&lib), &r), Some((0, 1)));

        // Shared: exported, imported elsewhere, the same stamps.
        let out = pics.join("shared.pdf");
        export(&path, &out).unwrap();
        let other = tempdir().join("stamps");
        let imported = import_in(&other, &out).unwrap();
        let back = load(&imported).unwrap();
        assert_eq!(back.title, "Signatures");
        let names: Vec<&str> = back.stamps.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Pat", "Lee"]);

        // Removing the last stamp removes the collection.
        remove(&path, 0).unwrap();
        remove(&path, 0).unwrap();
        assert!(list_in(&lib).is_empty());
    }

    #[test]
    fn favorites_keep_and_tokens_fill() {
        let lib = tempdir();
        assert!(favorites_in(&lib).is_empty());
        let favs = vec![StampRef {
            file: "Review.pdf".into(),
            name: "Approved".into(),
        }];
        set_favorites_in(&lib, &favs).unwrap();
        assert_eq!(favorites_in(&lib), favs);
        assert_eq!(
            fill(PRESETS[0].1, "Pat Smith", "PS", "10/14/2026", "3:15 PM"),
            "APPROVED\nPS 10/14/2026"
        );
    }
}
