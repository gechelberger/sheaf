//! Where the stamps and attachments sheaf's marks name are, while marks
//! name them by the generated cache's stems: transitional until slice 8,
//! whose marks hold a page stamp's form and an attachment's blob
//! themselves (M10.5 §3.6).
//!
//! A document keeps one [`MarkAssets`], outside history. Each file whose
//! marks it holds, the one it opened and those whose pages come in,
//! registers what its marks draw and embed ([`SourceAssets`]); placing a
//! stamp and pasting a mark intern their bytes; and a save writes each
//! from here, once (`annot::WriteCx`), and the file it wrote answers for
//! them from then on. What none of those has, the generated cache may
//! still: a stamp a plugin or another tool's comment named (until slice
//! 19), or an attachment not yet saved (until slice 8). It's read from
//! there once, and kept.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use anyhow::{Context, Result};
use lopdf::{Object, ObjectId};

use crate::content::ReadObjects;
use crate::store::{Blob, BlobKind, MemoryKind, Origin, Src, Store};

/// A page stamp's page.
#[derive(Clone)]
pub enum Stamp {
    /// A one-page PDF.
    Pdf(Blob),
    /// The page form a source's saved appearance of the stamp draws.
    Form { src: Src, form: ObjectId },
}

/// An attachment's file.
#[derive(Clone)]
pub enum File {
    /// An embedded file a source holds, as it's stored there.
    Source { src: Src, stream: ObjectId },
    /// Its bytes.
    Blob(Blob),
}

/// The stamps' pages and attachments' files a file holds for its marks,
/// by the stems they name them by: what `file::annots_in` found.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SourceAssets {
    /// Each stamp's page form.
    pub stamps: HashMap<String, ObjectId>,
    /// Each attachment's embedded file stream.
    pub files: HashMap<String, ObjectId>,
}

/// A document's stamps and attachments, by stem. Shared by the document
/// and its snapshots, so a save on another thread finds what was
/// registered while it ran. A stem is trusted to name the same bytes
/// wherever it's found, but nothing checks it: a file whose pages come in
/// can name a stem the document knows with other bytes. So what's known
/// stays, whoever registers or interns the stem next, but for the file a
/// save wrote, which holds what the save took from here.
pub struct MarkAssets {
    /// Where bytes read or pasted are interned.
    store: Arc<Store>,
    stamps: RwLock<HashMap<String, Stamp>>,
    files: RwLock<HashMap<String, File>>,
    /// Stamps' pages as one-page PDFs, made once each.
    pdfs: Mutex<HashMap<String, Arc<[u8]>>>,
    /// How many times what's known has changed.
    generation: AtomicU64,
}

impl MarkAssets {
    /// None yet, interning into `store`.
    pub fn new(store: Arc<Store>) -> MarkAssets {
        MarkAssets {
            store,
            stamps: RwLock::default(),
            files: RwLock::default(),
            pdfs: Mutex::default(),
            generation: AtomicU64::new(0),
        }
    }

    /// What `src`, a file opened or whose pages come in, holds for its
    /// marks, `found` in it: kept for the stems not known yet.
    pub fn register(&self, src: &Src, found: &SourceAssets) {
        self.take(src, found, false);
    }

    /// What `src`, the file a save just wrote, holds for its marks, `found`
    /// in it. It takes the place of what was known, since the save wrote
    /// each from here: the file it replaced, and a stamp placed or pasted,
    /// aren't held for them, and the next save keeps the saved file's
    /// where they are.
    pub fn register_saved(&self, src: &Src, found: &SourceAssets) {
        self.take(src, found, true);
    }

    fn take(&self, src: &Src, found: &SourceAssets, replace: bool) {
        let mut stamps = write(&self.stamps);
        for (stem, &form) in &found.stamps {
            let stamp = Stamp::Form {
                src: src.clone(),
                form,
            };
            put(&mut stamps, stem, stamp, replace);
        }
        drop(stamps);
        let mut files = write(&self.files);
        for (stem, &stream) in &found.files {
            let file = File::Source {
                src: src.clone(),
                stream,
            };
            put(&mut files, stem, file, replace);
        }
        self.changed();
    }

    /// How many times what it knows has changed: what it couldn't answer
    /// for before it may now.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    fn changed(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Stamp `stem`'s one-page PDF, placed from the library or pasted:
    /// kept unless the stamp is known already.
    pub fn intern_stamp(&self, stem: &str, pdf: Arc<[u8]>) {
        if read(&self.stamps).contains_key(stem) {
            return;
        }
        let kind = BlobKind::Pdf {
            password: None,
            origin: Origin::Memory(MemoryKind::Library),
        };
        if let Some(blob) = self.intern(stem, pdf, kind) {
            write(&self.stamps)
                .entry(stem.to_string())
                .or_insert(Stamp::Pdf(blob));
            self.changed();
        }
    }

    /// Attachment `stem`'s bytes, pasted: kept unless the file is known
    /// already.
    pub fn intern_file(&self, stem: &str, bytes: Arc<[u8]>) {
        if read(&self.files).contains_key(stem) {
            return;
        }
        if let Some(blob) = self.intern(stem, bytes, BlobKind::Attachment) {
            write(&self.files)
                .entry(stem.to_string())
                .or_insert(File::Blob(blob));
            self.changed();
        }
    }

    /// Stamp `stem`'s page, else the generated cache's file of that name.
    pub fn stamp(&self, stem: &str) -> Option<Stamp> {
        if let Some(stamp) = read(&self.stamps).get(stem) {
            return Some(stamp.clone());
        }
        let path = crate::generated::path_of(stem)?;
        let bytes = sheaf_core::read_file(&path).ok()?;
        let kind = BlobKind::Pdf {
            password: None,
            origin: Origin::File(path.into()),
        };
        let blob = self.intern(stem, bytes, kind)?;
        Some(
            write(&self.stamps)
                .entry(stem.to_string())
                .or_insert(Stamp::Pdf(blob))
                .clone(),
        )
    }

    /// Attachment `stem`'s file, else the generated cache's file of that
    /// name.
    pub fn file(&self, stem: &str) -> Option<File> {
        if let Some(file) = read(&self.files).get(stem) {
            return Some(file.clone());
        }
        let bytes = sheaf_core::read_file(&crate::generated::blob_path(stem)?).ok()?;
        let blob = self.intern(stem, bytes, BlobKind::Attachment)?;
        Some(
            write(&self.files)
                .entry(stem.to_string())
                .or_insert(File::Blob(blob))
                .clone(),
        )
    }

    /// Stamp `stem`'s page as a one-page PDF, to show or copy: a form a
    /// source holds is made one once.
    pub fn stamp_pdf(&self, stem: &str) -> Option<Arc<[u8]>> {
        let (src, form) = match self.stamp(stem)? {
            Stamp::Pdf(blob) => return Some(blob.bytes().clone()),
            Stamp::Form { src, form } => (src, form),
        };
        let made = lock(&self.pdfs).get(stem).cloned();
        if made.is_some() {
            return made;
        }
        let pdf: Arc<[u8]> = match crate::stamp_lib::form_to_pdf(src.pdf(), form) {
            Ok(pdf) => pdf.into(),
            Err(e) => {
                log::warn!("stamp {stem}: {e:#}");
                return None;
            }
        };
        Some(
            lock(&self.pdfs)
                .entry(stem.to_string())
                .or_insert(pdf)
                .clone(),
        )
    }

    /// Attachment `stem`'s bytes, decoded.
    pub fn file_bytes(&self, stem: &str) -> Result<Arc<[u8]>> {
        match self
            .file(stem)
            .with_context(|| format!("no file {stem:?}"))?
        {
            File::Blob(blob) => Ok(blob.bytes().clone()),
            File::Source { src, stream } => {
                let bytes = src
                    .pdf()
                    .read(stream, |o| match o {
                        Object::Stream(s) => crate::attachments::file_bytes(s),
                        _ => anyhow::bail!("the embedded file isn't a stream"),
                    })
                    .context("reading the embedded file")??;
                Ok(bytes.into())
            }
        }
    }

    /// `bytes` in the store as `kind`; None, said, when two files share
    /// their checksum.
    fn intern(&self, stem: &str, bytes: Arc<[u8]>, kind: BlobKind) -> Option<Blob> {
        self.store
            .intern(bytes, kind)
            .inspect_err(|e| log::warn!("{stem}: {e}"))
            .ok()
    }
}

/// `value` as `stem`'s in `known`: in place of what's there with
/// `replace`, else only where nothing is.
fn put<T>(known: &mut HashMap<String, T>, stem: &str, value: T, replace: bool) {
    if replace {
        known.insert(stem.to_string(), value);
    } else {
        known.entry(stem.to_string()).or_insert(value);
    }
}

fn read<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

fn write<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

fn lock<T>(lock: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use lopdf::{Document as Pdf, Stream, dictionary};

    use super::*;

    /// A one-page PDF, its page `w` by 50 points.
    fn page(w: i64) -> Vec<u8> {
        let mut pdf = Pdf::with_version("1.7");
        let pages_id = pdf.new_object_id();
        let content = pdf.add_object(Stream::new(dictionary! {}, b"0 0 10 10 re f".to_vec()));
        let page = pdf.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => content,
            "MediaBox" => vec![0.into(), 0.into(), w.into(), 50.into()],
        });
        pdf.objects.insert(
            pages_id,
            dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 }.into(),
        );
        let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        pdf.trailer.set("Root", catalog);
        let mut out = Vec::new();
        pdf.save_to(&mut out).unwrap();
        out
    }

    /// A file holding a stamp's page as a form, 80 by 40 points, and an
    /// embedded file, compressed, as a save writes them; and their ids.
    fn holding(file: &[u8]) -> (Src, ObjectId, ObjectId) {
        let mut pdf = Pdf::load_mem(&page(612)).unwrap();
        let form = pdf.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject", "Subtype" => "Form",
                "BBox" => vec![0.into(), 0.into(), 80.into(), 40.into()],
            },
            b"0 0 1 rg 0 0 80 40 re f".to_vec(),
        ));
        let mut embedded = Stream::new(dictionary! { "Type" => "EmbeddedFile" }, file.to_vec());
        embedded.compress().unwrap();
        let stream = pdf.add_object(embedded);
        let mut bytes = Vec::new();
        pdf.save_to(&mut bytes).unwrap();
        (crate::forms::fixture::source(bytes), form, stream)
    }

    /// What a file holds answers for its marks: its form made a one-page
    /// PDF once, the size it draws, and its embedded file decoded.
    #[test]
    fn a_file_answers_for_what_its_marks_hold() {
        let (src, form, stream) = holding(b"notes");
        let assets = MarkAssets::new(Store::new());
        let found = SourceAssets {
            stamps: [("s".to_string(), form)].into(),
            files: [("f".to_string(), stream)].into(),
        };
        assets.register(&src, &found);
        assert!(matches!(assets.stamp("s"), Some(Stamp::Form { form: f, .. }) if f == form));
        let pdf = assets.stamp_pdf("s").unwrap();
        assert!(Arc::ptr_eq(&pdf, &assets.stamp_pdf("s").unwrap()));
        let pdf = Pdf::load_mem(&pdf).unwrap();
        let page = pdf.get_pages()[&1];
        let media = pdf.get_dictionary(page).unwrap().get(b"MediaBox").unwrap();
        let media: Vec<f32> = media
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_float().unwrap() as f32)
            .collect();
        assert_eq!(media, [0.0, 0.0, 80.0, 40.0]);
        assert_eq!(&*assets.file_bytes("f").unwrap(), b"notes");
        assert!(assets.file_bytes("g").is_err());
    }

    /// What's known stays, whatever's placed, pasted or registered for its
    /// stem next, but for what the file a save wrote holds, which takes
    /// its place. Each change counts a generation.
    #[test]
    fn what_is_known_stays_but_for_a_saved_file() {
        let assets = MarkAssets::new(Store::new());
        let first: Arc<[u8]> = page(100).into();
        assets.intern_stamp("s", first.clone());
        let placed = assets.generation();
        assets.intern_stamp("s", page(200).into());
        assert_eq!(assets.stamp_pdf("s").unwrap(), first);
        let Some(Stamp::Pdf(blob)) = assets.stamp("s") else {
            panic!("a PDF")
        };
        assert!(matches!(
            blob.kind(),
            BlobKind::Pdf {
                origin: Origin::Memory(MemoryKind::Library),
                ..
            }
        ));
        assets.intern_file("f", b"one".as_slice().into());
        assets.intern_file("f", b"two".as_slice().into());
        assert_eq!(&*assets.file_bytes("f").unwrap(), b"one");
        assert!(assets.generation() > placed);
        // A file whose pages come in, naming the same stems.
        let (src, form, stream) = holding(b"three");
        let found = SourceAssets {
            stamps: [("s".to_string(), form)].into(),
            files: [("f".to_string(), stream)].into(),
        };
        assets.register(&src, &found);
        assert!(matches!(assets.stamp("s"), Some(Stamp::Pdf(_))));
        assert_eq!(&*assets.file_bytes("f").unwrap(), b"one");
        // The file a save wrote.
        assets.register_saved(&src, &found);
        assert!(matches!(assets.stamp("s"), Some(Stamp::Form { form: f, .. }) if f == form));
        assert_eq!(&*assets.file_bytes("f").unwrap(), b"three");
    }

    /// What nothing else has, the generated cache answers for, once: by
    /// its path, and kept. A name the cache can't give is never looked
    /// for there.
    #[test]
    fn the_cache_answers_last() {
        let pdf = page(fastrand::i64(300..100_000));
        let path = crate::generated::store(&pdf).unwrap();
        let stem = crate::generated::stem_of(&path);
        let bytes = format!("notes {:x}", fastrand::u64(..)).into_bytes();
        let file = crate::generated::store_blob(&bytes).unwrap();
        let assets = MarkAssets::new(Store::new());
        let Some(Stamp::Pdf(blob)) = assets.stamp(&stem) else {
            panic!("the cache's")
        };
        assert_eq!(**blob.bytes(), *pdf);
        assert!(matches!(
            blob.kind(),
            BlobKind::Pdf { origin: Origin::File(p), .. } if **p == *path
        ));
        assert!(
            matches!(assets.file(&file), Some(File::Blob(b)) if matches!(b.kind(), BlobKind::Attachment))
        );
        // Kept: gone from the cache, still known.
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_file(crate::generated::blob_path(&file).unwrap()).unwrap();
        assert_eq!(*assets.stamp_pdf(&stem).unwrap(), *pdf);
        assert_eq!(*assets.file_bytes(&file).unwrap(), *bytes);
        let fresh = MarkAssets::new(Store::new());
        assert!(fresh.stamp(&stem).is_none() && fresh.file(&file).is_none());
        assert!(fresh.stamp("../../etc/passwd").is_none());
    }
}
