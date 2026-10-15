//! The `render` interface: read-only page images as WIT resources. Pixels
//! stay on the host; plugins read strips or tiles. Every handle is freed
//! when the call that made it returns.

use anyhow::Result;
use sheaf_doc::{PageRef, SourceId};
use wasmtime::component::Resource;

use crate::runtime::HostState;

/// Most pixels one call may render, all images together.
pub const PIXEL_BUDGET: u64 = 400_000_000;
/// Most images alive at once.
pub const MAX_LIVE_IMAGES: usize = 16;
pub const MAX_DPI: u32 = 600;

/// Renders pages for plugins. The app implements it over its render
/// workers: `rect` is a pixel tile of the page drawn `page_w`×`page_h` in
/// the file's own orientation; the result is RGBA, `w`×`h`.
pub trait RenderSource: Send + Sync {
    #[allow(clippy::too_many_arguments)]
    fn render(
        &self,
        source: SourceId,
        index: u32,
        page_w: u32,
        page_h: u32,
        rect: (u32, u32, u32, u32),
    ) -> Result<Vec<u8>>;
}

/// A `page-renderer` handle.
pub struct PageRenderer {
    pub(crate) page: PageRef,
    /// Points, as the file draws it.
    pub(crate) size: (f32, f32),
}

/// An `image` handle.
pub struct RenderedImage {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) gray: bool,
    pub(crate) data: Vec<u8>,
    /// Page point of pixel (0, 0), and pixels per point.
    pub(crate) origin: (f32, f32),
    pub(crate) scale: f32,
}

impl RenderedImage {
    pub(crate) fn bpp(&self) -> u32 {
        if self.gray { 1 } else { 4 }
    }
}

/// Handles one call created, freed when it returns.
#[derive(Default)]
pub(crate) struct CallHandles {
    pub renderers: Vec<u32>,
    pub images: Vec<u32>,
    pub pixels: u64,
}

impl HostState {
    /// Free every render handle the call made.
    pub(crate) fn drop_render_handles(&mut self) {
        let handles = std::mem::take(&mut self.render_handles);
        for rep in handles.images {
            let _ = self.table.delete::<RenderedImage>(Resource::new_own(rep));
        }
        for rep in handles.renderers {
            let _ = self.table.delete::<PageRenderer>(Resource::new_own(rep));
        }
    }
}
