//! Document model: the editable object graph, page tree, outline, and
//! annotations, plus undo/redo and incremental save.
//!
//! M0 scope: wrap `lopdf` just enough to load, inspect, and round-trip.

use std::path::Path;

use anyhow::{Context, Result};
use sheaf_core::{PageIndex, PageSize, Rotation};

pub struct Document {
    inner: lopdf::Document,
}

impl Document {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let inner = lopdf::Document::load(path)
            .with_context(|| format!("loading {}", path.display()))?;
        Ok(Self { inner })
    }

    pub fn page_count(&self) -> u32 {
        self.inner.get_pages().len() as u32
    }

    /// Size and rotation of a page, resolving inherited attributes.
    pub fn page_geometry(&self, index: PageIndex) -> Result<(PageSize, Rotation)> {
        let pages = self.inner.get_pages();
        let id = *pages
            .get(&(index.0 + 1))
            .with_context(|| format!("no page {}", index.0))?;
        let media_box = self
            .inherited(id, b"MediaBox")?
            .and_then(|o| o.as_array().ok().cloned())
            .unwrap_or_default();
        let num = |i: usize| media_box.get(i).and_then(|o| o.as_float().ok()).unwrap_or(0.0);
        let size = PageSize {
            width: (num(2) - num(0)).abs(),
            height: (num(3) - num(1)).abs(),
        };
        let rot = self
            .inherited(id, b"Rotate")?
            .and_then(|o| o.as_i64().ok())
            .map(|d| Rotation::from_degrees(d as i32))
            .unwrap_or_default();
        Ok((size, rot))
    }

    /// Walk up the `/Parent` chain looking for `key`.
    fn inherited(&self, mut id: lopdf::ObjectId, key: &[u8]) -> Result<Option<lopdf::Object>> {
        loop {
            let dict = self.inner.get_dictionary(id)?;
            if let Ok(obj) = dict.get(key) {
                return Ok(Some(self.inner.dereference(obj)?.1.clone()));
            }
            match dict.get(b"Parent") {
                Ok(parent) => id = parent.as_reference()?,
                Err(_) => return Ok(None),
            }
        }
    }

    /// Full rewrite. Incremental save comes in M2.
    pub fn save_as(&mut self, path: impl AsRef<Path>) -> Result<()> {
        self.inner.save(path.as_ref())?;
        Ok(())
    }

    pub fn into_inner(self) -> lopdf::Document {
        self.inner
    }
}
