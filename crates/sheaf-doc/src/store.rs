//! The bytes a document uses, named by their content (M10.5 §3.1): a
//! [`Blob`] is a byte string, a [`Src`] a PDF blob parsed. The [`Store`]
//! hands out one blob for the same bytes, however often they're interned,
//! and one source for a blob while anything holds it.
//!
//! Liveness is the `Arc`: a blob lives while a [`Blob`] or [`Src`] holds
//! it, and the store keeps only weak references, so nothing has to be
//! freed by hand. A [`Source`] holds its blob, and the blob only a weak
//! reference to its source, so there's no cycle. Interning is outside
//! undo: undoing an insert leaves its file interned, and the bytes go when
//! the last version that holds them does.

use std::collections::HashMap;
use std::fmt;
use std::ops::Deref;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock, Weak};

use zeroize::Zeroizing;

use crate::source::{Limits, OpenError, Source};

/// A blob's name: its checksum and its length. Two blobs with one id are
/// compared byte for byte before they're taken as one.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct BlobId {
    /// [`sheaf_core::checksum`] of the bytes.
    pub hash: u128,
    pub len: u64,
}

impl BlobId {
    pub fn of(bytes: &[u8]) -> BlobId {
        BlobId {
            hash: sheaf_core::checksum(bytes),
            len: bytes.len() as u64,
        }
    }
}

/// What a blob is: a PDF, or a file a mark attaches. Images come with
/// what interns them.
pub enum BlobKind {
    Pdf {
        /// What it was opened with, if it needed one.
        password: Option<Zeroizing<String>>,
        origin: Origin,
    },
    /// The bytes of a file an attachment mark holds.
    Attachment,
}

impl BlobKind {
    /// A PDF read from the file at `path`, opened with `password`.
    pub fn file(path: &Path, password: Option<&str>) -> BlobKind {
        BlobKind::Pdf {
            password: password.map(|p| Zeroizing::new(p.to_owned())),
            origin: Origin::File(path.into()),
        }
    }
}

/// Where a blob was first interned from: a hint, never a name to show.
/// Interning the same bytes again keeps the first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    File(Arc<Path>),
    Memory(MemoryKind),
}

/// What made a blob in memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryKind {
    /// A copy sheaf made of another source's pages: cropped, watermarked,
    /// a header and footer stripped. Gone when those are edits (slice 17).
    Derived,
    /// A page stamp's one-page PDF, from the stamp library or made from a
    /// placed stamp's page, as one placed or pasted is.
    Library,
}

/// Two different byte strings with one [`BlobId`]: the second isn't
/// interned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("two different files have the same checksum ({id:?})")]
pub struct BlobCollision {
    pub id: BlobId,
}

/// The blobs of one document, and the caps its sources are read under.
pub struct Store {
    blobs: RwLock<HashMap<BlobId, Weak<BlobInner>>>,
    /// The bytes of the live blobs, each counted once.
    live: Arc<AtomicUsize>,
    limits: Limits,
}

/// A byte string the document uses. Clones share it.
#[derive(Clone)]
pub struct Blob(Arc<BlobInner>);

struct BlobInner {
    id: BlobId,
    bytes: Arc<[u8]>,
    kind: BlobKind,
    /// Its source while one lives.
    pdf: Mutex<Weak<Source>>,
    /// The store's count of live bytes.
    live: Arc<AtomicUsize>,
}

impl Drop for BlobInner {
    /// Never touches the store's map: a blob's last handle can drop while
    /// the map is locked. Its dead entry goes when the bytes are interned
    /// again, or when the map would grow.
    fn drop(&mut self) {
        self.live.fetch_sub(self.bytes.len(), Ordering::Relaxed);
    }
}

impl Store {
    /// An empty store, reading sources under [`Limits::DEFAULT`].
    pub fn new() -> Arc<Store> {
        Store::with_limits(Limits::DEFAULT)
    }

    pub(crate) fn with_limits(limits: Limits) -> Arc<Store> {
        Arc::new(Store {
            blobs: RwLock::default(),
            live: Arc::default(),
            limits,
        })
    }

    /// The blob of `bytes`: the one already interned when it's alive, and
    /// its bytes are these, compared byte for byte, with the origin and
    /// password it was first given; otherwise a new one of `kind`.
    /// Different bytes under a live blob's id are a collision.
    pub fn intern(&self, bytes: Arc<[u8]>, kind: BlobKind) -> Result<Blob, BlobCollision> {
        let id = BlobId::of(&bytes);
        let same = |blob: Arc<BlobInner>| {
            if Arc::ptr_eq(&blob.bytes, &bytes) || blob.bytes == bytes {
                Ok(Blob(blob))
            } else {
                Err(BlobCollision { id })
            }
        };
        // Compared outside the lock: the bytes never change.
        let found = self.read().get(&id).and_then(Weak::upgrade);
        if let Some(blob) = found {
            return same(blob);
        }
        let mut blobs = self.blobs.write().unwrap_or_else(PoisonError::into_inner);
        if let Some(blob) = blobs.get(&id).and_then(Weak::upgrade) {
            drop(blobs);
            return same(blob);
        }
        if blobs.len() == blobs.capacity() {
            // Before the map grows, the dead go.
            blobs.retain(|_, blob| blob.strong_count() > 0);
        }
        self.live.fetch_add(bytes.len(), Ordering::Relaxed);
        let blob = Arc::new(BlobInner {
            id,
            bytes,
            kind,
            pdf: Mutex::default(),
            live: self.live.clone(),
        });
        blobs.insert(id, Arc::downgrade(&blob));
        Ok(Blob(blob))
    }

    /// The blob `id`, while anything holds it.
    pub fn get(&self, id: BlobId) -> Option<Blob> {
        self.read().get(&id).and_then(Weak::upgrade).map(Blob)
    }

    /// `blob` parsed: its cross-reference sections and trailer, and its
    /// page table. Parsed once while a [`Src`] of it lives, and again only
    /// after every one has dropped. The page table walks the whole tree,
    /// so this runs off the UI thread.
    pub fn source(&self, blob: &Blob) -> Result<Src, OpenError> {
        let mut pdf = blob.0.pdf.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(source) = pdf.upgrade() {
            return Ok(Src(source));
        }
        let source = Arc::new(Source::open(blob.clone(), &self.limits)?);
        *pdf = Arc::downgrade(&source);
        Ok(Src(source))
    }

    /// `bytes` interned as a blob of `kind`, as [`intern`](Self::intern)
    /// does, and parsed, as [`source`](Self::source) does.
    pub fn open(&self, bytes: Arc<[u8]>, kind: BlobKind) -> Result<Src, OpenError> {
        let blob = self.intern(bytes, kind)?;
        self.source(&blob)
    }

    /// Whether `blob` is this store's.
    pub fn holds(&self, blob: &Blob) -> bool {
        self.get(blob.id())
            .is_some_and(|ours| Arc::ptr_eq(&ours.0, &blob.0))
    }

    /// The bytes of every live blob, each counted once.
    pub fn bytes(&self) -> usize {
        self.live.load(Ordering::Relaxed)
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashMap<BlobId, Weak<BlobInner>>> {
        self.blobs.read().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Blob {
    pub fn id(&self) -> BlobId {
        self.0.id
    }

    pub fn bytes(&self) -> &Arc<[u8]> {
        &self.0.bytes
    }

    pub fn len(&self) -> usize {
        self.0.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.bytes.is_empty()
    }

    pub fn kind(&self) -> &BlobKind {
        &self.0.kind
    }

    /// The password a PDF blob was opened with.
    pub fn password(&self) -> Option<&str> {
        match &self.0.kind {
            BlobKind::Pdf { password, .. } => password.as_deref().map(String::as_str),
            BlobKind::Attachment => None,
        }
    }
}

impl fmt::Debug for Blob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Blob").field(&self.0.id).finish()
    }
}

/// A PDF source: pins its blob. Equal, ordered and hashed by its blob's
/// id.
#[derive(Clone)]
pub struct Src(Arc<Source>);

impl Deref for Src {
    type Target = Source;

    fn deref(&self) -> &Source {
        &self.0
    }
}

impl PartialEq for Src {
    fn eq(&self, other: &Src) -> bool {
        self.id() == other.id()
    }
}

impl Eq for Src {}

impl std::hash::Hash for Src {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id().hash(state);
    }
}

impl PartialOrd for Src {
    fn partial_cmp(&self, other: &Src) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Src {
    fn cmp(&self, other: &Src) -> std::cmp::Ordering {
        self.id().cmp(&other.id())
    }
}

impl fmt::Debug for Src {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Src").field(&self.id()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forms::fixture::Form;

    fn from_file(name: &str) -> BlobKind {
        BlobKind::file(Path::new(name), None)
    }

    fn intern(store: &Store, bytes: &[u8]) -> Blob {
        store.intern(bytes.into(), from_file("a.pdf")).unwrap()
    }

    #[test]
    fn interning_the_same_bytes_twice_gives_one_blob() {
        let store = Store::new();
        let bytes = Form::new(2).bytes();
        let first = store
            .intern(bytes.as_slice().into(), from_file("a.pdf"))
            .unwrap();
        // Another buffer of the same bytes, from another file.
        let second = store.intern(bytes.into(), from_file("b.pdf")).unwrap();
        assert!(Arc::ptr_eq(&first.0, &second.0));
        assert_eq!(store.bytes(), first.len());
        let BlobKind::Pdf { origin, .. } = second.kind() else {
            panic!("a PDF")
        };
        assert_eq!(origin, &Origin::File(Path::new("a.pdf").into()));
        assert!(Arc::ptr_eq(&store.get(first.id()).unwrap().0, &first.0));
        let other = intern(&store, b"other bytes");
        assert_ne!(other.id(), first.id());
        assert_eq!(store.bytes(), first.len() + other.len());
    }

    #[test]
    fn a_forced_blob_collision_is_refused() {
        let store = Store::new();
        let held = intern(&store, b"a blob");
        let bytes: Arc<[u8]> = Arc::from(&b"other bytes"[..]);
        let id = BlobId::of(&bytes);
        // As if `held`'s bytes had the other bytes' checksum and length.
        store
            .blobs
            .write()
            .unwrap()
            .insert(id, Arc::downgrade(&held.0));
        let refused = store.intern(bytes, from_file("b.pdf")).unwrap_err();
        assert_eq!(refused, BlobCollision { id });
        assert_eq!(store.bytes(), held.len());
        assert!(Arc::ptr_eq(&store.get(id).unwrap().0, &held.0));
    }

    #[test]
    fn the_store_counts_live_bytes_once_and_forgets_dead_blobs() {
        let store = Store::new();
        let a = intern(&store, b"four");
        let again = intern(&store, b"four");
        let b = intern(&store, b"eight by");
        assert_eq!(store.bytes(), 12);
        let id = b.id();
        drop(b);
        assert_eq!(store.bytes(), 4);
        assert!(store.get(id).is_none());
        drop(a);
        assert_eq!(store.bytes(), 4);
        drop(again);
        assert_eq!(store.bytes(), 0);

        // A source holds its blob.
        let blob = intern(&store, &Form::new(1).bytes());
        let (id, len) = (blob.id(), blob.len());
        let src = store.source(&blob).unwrap();
        drop(blob);
        assert_eq!(store.bytes(), len);
        assert_eq!(store.get(id).unwrap().id(), id);
        drop(src);
        assert_eq!(store.bytes(), 0);
        assert!(store.get(id).is_none());

        // Interned again, the dead are replaced; and they go before the
        // map grows.
        let blob = intern(&store, b"four");
        assert_eq!(store.bytes(), 4);
        drop(blob);
        for i in 0..1000u32 {
            drop(intern(&store, &i.to_le_bytes()));
        }
        assert!(store.blobs.read().unwrap().len() < 100);
        assert_eq!(store.bytes(), 0);
    }

    #[test]
    fn a_source_is_parsed_once_while_a_src_lives() {
        let store = Store::new();
        let blob = intern(&store, &Form::new(3).bytes());
        let first = store.source(&blob).unwrap();
        let second = store.source(&blob).unwrap();
        let elsewhere =
            std::thread::scope(|s| s.spawn(|| store.source(&blob).unwrap()).join()).unwrap();
        assert!(std::ptr::eq(&*first, &*second));
        assert!(std::ptr::eq(&*first, &*elsewhere));
        assert_eq!(first, elsewhere);
        assert_eq!(first.page_count(), 3);
        let source = Arc::downgrade(&first.0);
        drop((first, second, elsewhere));
        assert!(source.upgrade().is_none());
        let parsed_again = store.source(&blob).unwrap();
        assert_eq!(parsed_again.id(), blob.id());
        assert_eq!(parsed_again.page_count(), 3);
    }
}
