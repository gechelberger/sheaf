//! Rasterization. The `Renderer` trait is the seam between the app and the
//! engine; `pdfium` is the only implementation for now.
//!
//! M0 scope: open a file, render one page to RGBA. Tiling and the cache come
//! in M1.

use std::path::Path;

use anyhow::{Context, Result};
use sheaf_core::{PageIndex, PageSize};

/// A rendered page or tile: tightly packed RGBA8, row-major.
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub trait Renderer {
    fn page_count(&self) -> u32;
    fn page_size(&self, page: PageIndex) -> Result<PageSize>;
    /// Render a whole page scaled so its width is `target_width` pixels.
    fn render_page(&self, page: PageIndex, target_width: u32) -> Result<Raster>;
}

pub mod pdfium {
    use super::*;
    use pdfium_render::prelude::*;

    /// Locate and bind libpdfium. Search order: `$PDFIUM_LIB_DIR`,
    /// `vendor/pdfium/<platform>/` relative to the cwd, then the system library.
    pub fn bind() -> Result<Pdfium> {
        let mut dirs: Vec<std::path::PathBuf> = Vec::new();
        if let Ok(d) = std::env::var("PDFIUM_LIB_DIR") {
            dirs.push(d.into());
        }
        dirs.push(format!("vendor/pdfium/{}-{}", std::env::consts::OS, std::env::consts::ARCH).into());

        let mut last_err = None;
        for d in &dirs {
            match Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(d)) {
                Ok(b) => return Ok(Pdfium::new(b)),
                Err(e) => last_err = Some(e),
            }
        }
        Pdfium::bind_to_system_library()
            .map(Pdfium::new)
            .with_context(|| {
                format!(
                    "no libpdfium found (searched {dirs:?} and system); run scripts/fetch-pdfium.sh. last error: {last_err:?}"
                )
            })
    }

    pub struct PdfiumRenderer {
        // Document borrows from Pdfium; keep both together and self-referential
        // via a leaked Pdfium for the spike. M1 will restructure this.
        doc: PdfDocument<'static>,
    }

    impl PdfiumRenderer {
        pub fn open(path: impl AsRef<Path>, password: Option<&str>) -> Result<Self> {
            let pdfium: &'static Pdfium = Box::leak(Box::new(bind()?));
            let doc = pdfium
                .load_pdf_from_file(path.as_ref(), password)
                .with_context(|| format!("pdfium loading {}", path.as_ref().display()))?;
            Ok(Self { doc })
        }
    }

    impl Renderer for PdfiumRenderer {
        fn page_count(&self) -> u32 {
            self.doc.pages().len() as u32
        }

        fn page_size(&self, page: PageIndex) -> Result<PageSize> {
            let p = self.doc.pages().get(page.0 as i32)?;
            Ok(PageSize {
                width: p.width().value,
                height: p.height().value,
            })
        }

        fn render_page(&self, page: PageIndex, target_width: u32) -> Result<Raster> {
            let p = self.doc.pages().get(page.0 as i32)?;
            let cfg = PdfRenderConfig::new().set_target_width(target_width as i32);
            let bmp = p.render_with_config(&cfg)?;
            Ok(Raster {
                width: bmp.width() as u32,
                height: bmp.height() as u32,
                rgba: bmp.as_rgba_bytes(),
            })
        }
    }
}
