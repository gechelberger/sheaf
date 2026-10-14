//! The `render` interface: read-only page images as WIT resources. Pixels
//! stay on the host; plugins read strips or tiles. Every handle is freed
//! when the call that made it returns.

use std::time::Instant;

use anyhow::Result;
use sheaf_doc::{PageId, PageRef, SourceId};
use wasmtime::component::Resource;

use crate::convert;
use crate::runtime::HostState;
use crate::runtime::bindings::sheaf::plugin::{render as api, types as w};

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
    fn bpp(&self) -> u32 {
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

impl api::Host for HostState {}

impl api::HostPageRenderer for HostState {
    fn new(&mut self, page: u64) -> Resource<PageRenderer> {
        // An unknown page still gets a handle; `render` reports it.
        let found = self.doc().and_then(|d| {
            let p = d
                .snapshot
                .state
                .pages
                .iter()
                .find(|p| p.id == PageId(page))
                .copied()?;
            let s = d.snapshot.source(p.source).map(|s| s.page_size(p.index))?;
            Some((p, (s.width, s.height)))
        });
        let (page, size) = found.unwrap_or((
            PageRef {
                id: PageId(page),
                source: SourceId(u32::MAX),
                index: 0,
                rotation: sheaf_core::Rotation::R0,
            },
            (0.0, 0.0),
        ));
        let r = self
            .table
            .push(PageRenderer { page, size })
            .expect("resource table full");
        self.render_handles.renderers.push(r.rep());
        r
    }

    fn size(&mut self, this: Resource<PageRenderer>) -> (f32, f32) {
        self.table.get(&this).map(|r| r.size).unwrap_or((0.0, 0.0))
    }

    fn render(
        &mut self,
        this: Resource<PageRenderer>,
        options: api::RenderOptions,
    ) -> Result<Resource<RenderedImage>, String> {
        let (page, (pw, ph)) = {
            let r = self.table.get(&this).map_err(|e| e.to_string())?;
            (r.page, r.size)
        };
        if pw <= 0.0 {
            return Err(format!("no page {}", page.id.0));
        }
        let d = self.doc().ok_or("no document is open")?;
        let source = d.render.clone().ok_or("rendering isn't available")?;
        let dpi = options.dpi.clamp(1, MAX_DPI);
        let scale = dpi as f32 / 72.0;
        let (page_w, page_h) = (
            (pw * scale).round().max(1.0) as u32,
            (ph * scale).round().max(1.0) as u32,
        );
        let (x0, y0, x1, y1) = match options.region {
            Some(r) => {
                let r = convert::box_from_wit(r);
                (
                    (r.x.max(0.0) * scale).floor() as u32,
                    (r.y.max(0.0) * scale).floor() as u32,
                    ((r.x + r.w) * scale).ceil().min(page_w as f32) as u32,
                    ((r.y + r.h) * scale).ceil().min(page_h as f32) as u32,
                )
            }
            None => (0, 0, page_w, page_h),
        };
        if x1 <= x0 || y1 <= y0 {
            return Err("the region is outside the page".into());
        }
        let (w, h) = (x1 - x0, y1 - y0);
        let pixels = w as u64 * h as u64;
        if self.render_handles.pixels + pixels > PIXEL_BUDGET {
            return Err(format!(
                "this call has rendered its limit of {} million pixels",
                PIXEL_BUDGET / 1_000_000
            ));
        }
        if self.render_handles.images.len() >= MAX_LIVE_IMAGES {
            return Err(format!(
                "at most {MAX_LIVE_IMAGES} images can be alive at once; drop some first"
            ));
        }
        let started = Instant::now();
        let rgba = source
            .render(page.source, page.index, page_w, page_h, (x0, y0, w, h))
            .map_err(|e| format!("{e:#}"));
        // Rendering is the host's time, not the plugin's.
        self.deadline += started.elapsed();
        let rgba = rgba?;
        self.render_handles.pixels += pixels;
        let gray = matches!(options.format, api::PixelFormat::Gray8);
        let data = if gray {
            rgba.as_chunks::<4>()
                .0
                .iter()
                .map(|p| ((p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000) as u8)
                .collect()
        } else {
            rgba
        };
        let image = RenderedImage {
            width: w,
            height: h,
            gray,
            data,
            origin: (x0 as f32 / scale, y0 as f32 / scale),
            scale,
        };
        let r = self.table.push(image).map_err(|e| e.to_string())?;
        self.render_handles.images.push(r.rep());
        Ok(r)
    }

    fn drop(&mut self, this: Resource<PageRenderer>) -> wasmtime::Result<()> {
        let rep = this.rep();
        self.render_handles.renderers.retain(|r| *r != rep);
        self.table.delete(this)?;
        Ok(())
    }
}

impl api::HostImage for HostState {
    fn width(&mut self, this: Resource<RenderedImage>) -> u32 {
        self.table.get(&this).map(|i| i.width).unwrap_or(0)
    }

    fn height(&mut self, this: Resource<RenderedImage>) -> u32 {
        self.table.get(&this).map(|i| i.height).unwrap_or(0)
    }

    fn format(&mut self, this: Resource<RenderedImage>) -> api::PixelFormat {
        match self.table.get(&this) {
            Ok(i) if !i.gray => api::PixelFormat::Rgba8,
            _ => api::PixelFormat::Gray8,
        }
    }

    fn to_page(&mut self, this: Resource<RenderedImage>, x: f32, y: f32) -> w::Pt {
        match self.table.get(&this) {
            Ok(i) => w::Pt {
                x: i.origin.0 + x / i.scale,
                y: i.origin.1 + y / i.scale,
            },
            Err(_) => w::Pt { x: 0.0, y: 0.0 },
        }
    }

    fn read_rows(&mut self, this: Resource<RenderedImage>, y: u32, count: u32) -> Vec<u8> {
        let Ok(i) = self.table.get(&this) else {
            return Vec::new();
        };
        let stride = (i.width * i.bpp()) as usize;
        let start = (y.min(i.height) as usize) * stride;
        let end = ((y.saturating_add(count)).min(i.height) as usize) * stride;
        i.data[start..end].to_vec()
    }

    fn read_rect(
        &mut self,
        this: Resource<RenderedImage>,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
    ) -> Vec<u8> {
        let Ok(i) = self.table.get(&this) else {
            return Vec::new();
        };
        let bpp = i.bpp() as usize;
        let (x0, x1) = (
            x.min(i.width) as usize,
            x.saturating_add(w).min(i.width) as usize,
        );
        let (y0, y1) = (y.min(i.height), y.saturating_add(h).min(i.height));
        let stride = i.width as usize * bpp;
        let mut out = Vec::with_capacity((x1 - x0) * bpp * (y1 - y0) as usize);
        for row in y0..y1 {
            let base = row as usize * stride;
            out.extend_from_slice(&i.data[base + x0 * bpp..base + x1 * bpp]);
        }
        out
    }

    fn drop(&mut self, this: Resource<RenderedImage>) -> wasmtime::Result<()> {
        let rep = this.rep();
        self.render_handles.images.retain(|r| *r != rep);
        self.table.delete(this)?;
        Ok(())
    }
}
