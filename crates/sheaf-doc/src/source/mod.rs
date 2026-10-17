//! The files a document's pages come from, read lazily (M10.5 §3.1). A
//! [`Source`] is a PDF blob parsed: lopdf's [`LazyPdf`] reads the file's
//! cross-reference sections and trailer when it opens, and each object
//! when it's first asked for, under one set of caps, [`Limits`]: a hostile
//! file is a typed error, never a panic, a hang or memory without bound.
//!
//! What a source says about itself is a pure function of its bytes, read
//! once: the page table when it opens, its conformance claims when first
//! asked. Sources come from the [`Store`](crate::store::Store), one per
//! distinct file.

mod conformance;
#[doc(hidden)]
pub mod fuzz;
#[cfg(test)]
mod lazy_tests;
mod limits;
mod pages;
#[cfg(test)]
mod tests;

use std::ops::Range;
use std::sync::{Arc, OnceLock};

use lopdf::{LazyPdf, Object, ObjectId, XrefFacts};
use sheaf_core::PageSize;

use crate::content::{ContentError, DrawnForms, PageBlocks};
use crate::geom::Geom;
use crate::store::{Blob, BlobCollision, BlobId};

pub use conformance::{Conformance, PdfA};
pub use limits::{ContentLimits, FormLimits, Limits, PageTreeLimits};
pub use pages::{PageFacts, Parent};

/// A PDF, parsed: its blob, read lazily, and its page table.
pub struct Source {
    blob: Blob,
    pdf: LazyPdf,
    limits: Limits,
    pages: Arc<[PageFacts]>,
    /// Each page's blocks, once asked for, and what's known of the forms
    /// its pages draw.
    blocks: Box<[OnceLock<Arc<PageBlocks>>]>,
    forms: DrawnForms,
    conformance: OnceLock<Conformance>,
}

/// Why a source didn't open.
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("the password is wrong, or the file needs one")]
    Password,
    #[error("reading the file: {0}")]
    Pdf(lopdf::Error),
    #[error(transparent)]
    PageTree(#[from] PageTreeError),
    #[error(transparent)]
    Collision(#[from] BlobCollision),
}

impl OpenError {
    /// Whether `err`, anywhere in its chain, is a wrong or missing
    /// password.
    pub fn is_password(err: &anyhow::Error) -> bool {
        err.chain()
            .any(|c| matches!(c.downcast_ref::<OpenError>(), Some(OpenError::Password)))
    }
}

impl From<lopdf::Error> for OpenError {
    fn from(e: lopdf::Error) -> OpenError {
        match e {
            lopdf::Error::InvalidPassword => OpenError::Password,
            e => OpenError::Pdf(e),
        }
    }
}

/// What's wrong with a page tree that has no page table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PageTreeError {
    #[error("the page tree lists a node inside itself")]
    Cycle,
    #[error("a page's /Parent chain loops")]
    ParentLoop,
    #[error("the page tree is too deep")]
    TooDeep,
    #[error("the page tree lists too many pages")]
    TooManyPages,
    #[error("the file has no pages")]
    NoPages,
}

impl Source {
    /// `blob` read under `limits`: its cross-reference sections, trailer
    /// and page table.
    pub(crate) fn open(blob: Blob, limits: &Limits) -> Result<Source, OpenError> {
        let pdf = LazyPdf::open(blob.bytes().clone(), blob.password(), &limits.reader)?;
        let pages: Arc<[PageFacts]> = pages::table(&pdf, &limits.page_tree)?.into();
        Ok(Source {
            blob,
            pdf,
            limits: *limits,
            blocks: pages.iter().map(|_| OnceLock::new()).collect(),
            forms: DrawnForms::default(),
            pages,
            conformance: OnceLock::new(),
        })
    }

    pub fn id(&self) -> BlobId {
        self.blob.id()
    }

    pub fn blob(&self) -> &Blob {
        &self.blob
    }

    pub fn bytes(&self) -> &Arc<[u8]> {
        self.blob.bytes()
    }

    /// The password it was opened with, if it needed one.
    pub fn password(&self) -> Option<&str> {
        self.blob.password()
    }

    pub fn pdf(&self) -> &LazyPdf {
        &self.pdf
    }

    /// The caps it's read under.
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// False when lopdf rebuilt the cross-reference table: such a file is
    /// never appended to.
    pub fn base_sound(&self) -> bool {
        self.pdf.base_sound()
    }

    pub fn xref(&self) -> &XrefFacts {
        self.pdf.xref()
    }

    pub fn object(&self, id: ObjectId) -> lopdf::Result<Arc<Object>> {
        self.pdf.object(id)
    }

    /// Where object `id` is in the bytes. None for an object stream's
    /// member.
    pub fn raw_span(&self, id: ObjectId) -> Option<Range<usize>> {
        self.pdf.raw_span(id)
    }

    pub fn page_count(&self) -> u32 {
        self.pages.len() as u32
    }

    /// Page `index`, in the order PDFium numbers them.
    pub fn page(&self, index: u32) -> Option<&PageFacts> {
        self.pages.get(index as usize)
    }

    /// Every page, in that order.
    pub fn pages(&self) -> &[PageFacts] {
        &self.pages
    }

    /// Every page's object, in that order: how a reader that walks pages
    /// numbers them.
    pub fn page_ids(&self) -> impl ExactSizeIterator<Item = ObjectId> + Clone + '_ {
        self.pages.iter().map(|page| page.id)
    }

    /// Page `index`'s boxes and its own `/Rotate`; past the last page, a
    /// page's with no boxes, Letter.
    pub fn geom(&self, index: u32) -> Geom {
        self.page(index)
            .map_or_else(|| Geom::read(None, None, 0), |page| page.geom)
    }

    /// Page `index`'s size as shown.
    pub fn page_size(&self, index: u32) -> PageSize {
        self.geom(index).shown()
    }

    /// Page `index`'s content classified (§13): its pagination artifacts
    /// and who drew them, sheaf's blocks, its balance. Read once, its
    /// streams without keeping them; never at open. Past the last page, a
    /// page's with nothing on it.
    pub fn blocks(&self, index: u32) -> Result<Arc<PageBlocks>, ContentError> {
        let (Some(slot), Some(page)) = (self.blocks.get(index as usize), self.page(index)) else {
            return Ok(Arc::default());
        };
        if let Some(blocks) = slot.get() {
            return Ok(blocks.clone());
        }
        let blocks = PageBlocks::read(
            &self.pdf,
            &page.contents,
            page.resources.as_deref(),
            &self.limits,
            &self.forms,
        )
        .map_err(|fault| ContentError::Read { page: index, fault })?;
        Ok(slot.get_or_init(|| Arc::new(blocks)).clone())
    }

    /// Whether page `index`'s blocks have been read.
    #[cfg(test)]
    pub(crate) fn blocks_known(&self, index: u32) -> bool {
        self.blocks
            .get(index as usize)
            .is_some_and(|slot| slot.get().is_some())
    }

    /// What its XMP metadata claims it conforms to: PDF/A and PDF/UA.
    pub fn conformance(&self) -> anyhow::Result<Conformance> {
        if let Some(&claims) = self.conformance.get() {
            return Ok(claims);
        }
        let claims = conformance::read(&self.pdf, &self.limits)?;
        Ok(*self.conformance.get_or_init(|| claims))
    }
}
