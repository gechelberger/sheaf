//! Windows printing through GDI. The printer's driver settings (paper,
//! orientation, copies, duplex) go in its DEVMODE; each page of the
//! print PDF is rasterized by PDFium at the device's resolution, in bands,
//! and blitted where [`crate::place`] puts it.

use std::path::Path;

use anyhow::{Context, Result, bail};
use sheaf_core::{PageIndex, Rotation};
use sheaf_image::raster::PixelFormat;
use sheaf_render::{Job, Renderer, pdfium::PdfiumEngine};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDCW, DEVMODE_COLLATE, DEVMODE_DUPLEX, DEVMODEW,
    DIB_RGB_COLORS, DM_COLLATE, DM_COPIES, DM_DUPLEX, DM_IN_BUFFER, DM_ORIENTATION, DM_OUT_BUFFER,
    DM_PAPERSIZE, DeleteDC, GetDeviceCaps, HDC, LOGPIXELSX, LOGPIXELSY, PHYSICALHEIGHT,
    PHYSICALOFFSETX, PHYSICALOFFSETY, PHYSICALWIDTH, SRCCOPY, StretchDIBits,
};
use windows::Win32::Graphics::Printing::{
    ClosePrinter, DocumentPropertiesW, EnumPrintersW, GetDefaultPrinterW, OpenPrinterW,
    PRINTER_ENUM_CONNECTIONS, PRINTER_ENUM_LOCAL, PRINTER_HANDLE, PRINTER_INFO_4W,
};
use windows::Win32::Storage::Xps::{
    DC_PAPERNAMES, DC_PAPERS, DC_PAPERSIZE, DOCINFOW, DeviceCapabilitiesW, EndDoc, EndPage,
    StartDocW, StartPage,
};
use windows::core::{PCWSTR, PWSTR};

use crate::{Duplex, JobSettings, Orientation, Paper, Printer};

/// Rows rasterized at a time: a 600 dpi letter page is ~5100 px wide, so
/// 512 rows is ~10 MB.
const BAND: u32 = 512;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn from_wide(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { p.to_string().unwrap_or_default() }
}

pub fn printers() -> Result<Vec<Printer>> {
    let flags = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
    let (mut needed, mut count) = (0u32, 0u32);
    // The first call sizes the buffer (and fails with "insufficient buffer").
    let _ = unsafe { EnumPrintersW(flags, PCWSTR::null(), 4, None, &mut needed, &mut count) };
    let mut buf = vec![0u8; needed as usize];
    if needed > 0 {
        unsafe {
            EnumPrintersW(
                flags,
                PCWSTR::null(),
                4,
                Some(&mut buf),
                &mut needed,
                &mut count,
            )
        }
        .context("listing printers")?;
    }
    let infos = unsafe {
        std::slice::from_raw_parts(buf.as_ptr() as *const PRINTER_INFO_4W, count as usize)
    };
    let default = default_printer();
    Ok(infos
        .iter()
        .map(|i| {
            let name = from_wide(i.pPrinterName);
            let (papers, default_paper) = papers(&name);
            Printer {
                is_default: default.as_deref() == Some(name.as_str()),
                name,
                papers,
                default_paper,
            }
        })
        .collect())
}

fn default_printer() -> Option<String> {
    let mut len = 0u32;
    unsafe {
        let _ = GetDefaultPrinterW(None, &mut len);
        if len == 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize];
        if !GetDefaultPrinterW(Some(PWSTR(buf.as_mut_ptr())), &mut len).as_bool() {
            return None;
        }
        Some(String::from_utf16_lossy(
            &buf[..len.saturating_sub(1) as usize],
        ))
    }
}

/// The paper a printer offers, and which is its default.
fn papers(name: &str) -> (Vec<Paper>, usize) {
    let dev = wide(name);
    let dev = PCWSTR(dev.as_ptr());
    unsafe {
        let n = DeviceCapabilitiesW(dev, PCWSTR::null(), DC_PAPERS, None, None);
        if n <= 0 {
            return (vec![Paper::letter()], 0);
        }
        let n = n as usize;
        let mut ids = vec![0u16; n];
        let mut names = vec![0u16; n * 64];
        // POINTs, in tenths of a millimetre.
        let mut sizes = vec![0i32; n * 2];
        DeviceCapabilitiesW(
            dev,
            PCWSTR::null(),
            DC_PAPERS,
            Some(PWSTR(ids.as_mut_ptr())),
            None,
        );
        DeviceCapabilitiesW(
            dev,
            PCWSTR::null(),
            DC_PAPERNAMES,
            Some(PWSTR(names.as_mut_ptr())),
            None,
        );
        DeviceCapabilitiesW(
            dev,
            PCWSTR::null(),
            DC_PAPERSIZE,
            Some(PWSTR(sizes.as_mut_ptr() as *mut u16)),
            None,
        );
        let default_id = devmode(name).map(|dm| dm.Anonymous1.Anonymous1.dmPaperSize as u16);
        let mut papers = Vec::with_capacity(n);
        let mut default = 0;
        for i in 0..n {
            let raw = &names[i * 64..(i + 1) * 64];
            let end = raw.iter().position(|&c| c == 0).unwrap_or(64);
            let label = String::from_utf16_lossy(&raw[..end]);
            let (w, h) = (sizes[i * 2] as f32, sizes[i * 2 + 1] as f32);
            if w <= 0.0 || h <= 0.0 {
                continue;
            }
            let to_pt = 72.0 / 254.0;
            if Some(ids[i]) == default_id {
                default = papers.len();
            }
            papers.push(Paper {
                id: ids[i].to_string(),
                label,
                width: (w * to_pt).min(h * to_pt),
                height: (w * to_pt).max(h * to_pt),
            });
        }
        if papers.is_empty() {
            (vec![Paper::letter()], 0)
        } else {
            (papers, default)
        }
    }
}

/// The printer's default settings, as a buffer DEVMODEW plus its driver's
/// private bytes.
struct DevMode {
    buf: Vec<u8>,
}

impl std::ops::Deref for DevMode {
    type Target = DEVMODEW;
    fn deref(&self) -> &DEVMODEW {
        unsafe { &*(self.buf.as_ptr() as *const DEVMODEW) }
    }
}

impl std::ops::DerefMut for DevMode {
    fn deref_mut(&mut self) -> &mut DEVMODEW {
        unsafe { &mut *(self.buf.as_mut_ptr() as *mut DEVMODEW) }
    }
}

fn devmode(name: &str) -> Option<DevMode> {
    let w = wide(name);
    unsafe {
        let mut h = PRINTER_HANDLE::default();
        OpenPrinterW(PCWSTR(w.as_ptr()), &mut h, None).ok()?;
        let size = DocumentPropertiesW(None, h, PCWSTR(w.as_ptr()), None, None, 0);
        let result = if size > 0 {
            // u64 storage keeps the DEVMODEW aligned.
            let mut store = vec![0u64; (size as usize).div_ceil(8)];
            let out = store.as_mut_ptr() as *mut DEVMODEW;
            let r = DocumentPropertiesW(
                None,
                h,
                PCWSTR(w.as_ptr()),
                Some(out),
                None,
                DM_OUT_BUFFER.0,
            );
            (r >= 0).then(|| DevMode {
                buf: std::slice::from_raw_parts(store.as_ptr() as *const u8, size as usize)
                    .to_vec(),
            })
        } else {
            None
        };
        let _ = ClosePrinter(h);
        result
    }
}

pub fn submit(job: &JobSettings, pdf: &Path) -> Result<String> {
    let engine = PdfiumEngine::open(pdf, None)?;
    let pages = engine.page_count();
    if pages == 0 {
        bail!("nothing to print");
    }
    let mut dm = devmode(&job.printer).context("the printer's settings can't be read")?;
    {
        let d = &mut *dm;
        let fields = &mut d.dmFields;
        unsafe {
            let s = &mut d.Anonymous1.Anonymous1;
            if let Ok(id) = job.paper.id.parse::<i16>() {
                s.dmPaperSize = id;
                fields.0 |= DM_PAPERSIZE.0;
            }
            s.dmCopies = job.copies.clamp(1, i16::MAX as u32) as i16;
            fields.0 |= DM_COPIES.0;
            if job.orientation != Orientation::Auto {
                s.dmOrientation = if job.orientation == Orientation::Landscape {
                    2
                } else {
                    1
                };
                fields.0 |= DM_ORIENTATION.0;
            }
        }
        d.dmDuplex = DEVMODE_DUPLEX(match job.duplex {
            Duplex::Off => 1,
            Duplex::LongEdge => 2,
            Duplex::ShortEdge => 3,
        });
        d.dmCollate = DEVMODE_COLLATE(job.collate as i16);
        fields.0 |= DM_DUPLEX.0 | DM_COLLATE.0;
    }
    // Let the driver check and merge what we asked for.
    let name = wide(&job.printer);
    unsafe {
        let mut h = PRINTER_HANDLE::default();
        if OpenPrinterW(PCWSTR(name.as_ptr()), &mut h, None).is_ok() {
            let ptr = dm.buf.as_mut_ptr() as *mut DEVMODEW;
            DocumentPropertiesW(
                None,
                h,
                PCWSTR(name.as_ptr()),
                Some(ptr),
                Some(ptr as *const DEVMODEW),
                DM_IN_BUFFER.0 | DM_OUT_BUFFER.0,
            );
            let _ = ClosePrinter(h);
        }
    }

    let driver = wide("WINSPOOL");
    let hdc = unsafe {
        CreateDCW(
            PCWSTR(driver.as_ptr()),
            PCWSTR(name.as_ptr()),
            PCWSTR::null(),
            Some(&*dm as *const DEVMODEW),
        )
    };
    if hdc.is_invalid() {
        bail!("couldn't open the printer {}", job.printer);
    }
    let result = print_pages(hdc, &engine, pages, job);
    unsafe {
        let _ = DeleteDC(hdc);
    }
    result.map(|n| format!("{n} page{} sent", if n == 1 { "" } else { "s" }))
}

fn print_pages(hdc: HDC, engine: &PdfiumEngine, pages: u32, job: &JobSettings) -> Result<u32> {
    let title = wide(&job.title);
    let doc = DOCINFOW {
        cbSize: std::mem::size_of::<DOCINFOW>() as i32,
        lpszDocName: PCWSTR(title.as_ptr()),
        ..Default::default()
    };
    if unsafe { StartDocW(hdc, &doc) } <= 0 {
        bail!("the printer refused the job");
    }
    let caps = |i| unsafe { GetDeviceCaps(Some(hdc), i) } as f32;
    let (dpi_x, dpi_y) = (caps(LOGPIXELSX).max(1.0), caps(LOGPIXELSY).max(1.0));
    let (phys_w, phys_h) = (caps(PHYSICALWIDTH), caps(PHYSICALHEIGHT));
    let (off_x, off_y) = (caps(PHYSICALOFFSETX), caps(PHYSICALOFFSETY));
    let mut done = 0;
    for i in 0..pages {
        let size = engine.page_size(PageIndex(i))?;
        // The driver turned the sheet (or not); place on the sheet it gave us.
        let sheet = Paper {
            width: phys_w / dpi_x * 72.0,
            height: phys_h / dpi_y * 72.0,
            ..job.paper.clone()
        };
        let orientation = if sheet.width > sheet.height {
            Orientation::Landscape
        } else {
            Orientation::Portrait
        };
        let p = crate::place(size.width, size.height, &sheet, job.sizing, orientation);
        let (dw, dh) = (
            (p.w / 72.0 * dpi_x).round().max(1.0) as u32,
            (p.h / 72.0 * dpi_y).round().max(1.0) as u32,
        );
        let (dx, dy) = (
            (p.x / 72.0 * dpi_x - off_x).round() as i32,
            (p.y / 72.0 * dpi_y - off_y).round() as i32,
        );
        if unsafe { StartPage(hdc) } <= 0 {
            bail!("the printer stopped at page {}", i + 1);
        }
        let mut y = 0;
        while y < dh {
            let h = BAND.min(dh - y);
            let raster = engine.render(
                PageIndex(i),
                Job::Tile {
                    page_width: dw,
                    page_height: dh,
                    x: 0,
                    y,
                    w: dw,
                    h,
                    rotation: Rotation::R0,
                },
                PixelFormat::Bgra,
            )?;
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: raster.width as i32,
                    // Negative: rows run top to bottom.
                    biHeight: -(raster.height as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            unsafe {
                StretchDIBits(
                    hdc,
                    dx,
                    dy + y as i32,
                    raster.width as i32,
                    raster.height as i32,
                    0,
                    0,
                    raster.width as i32,
                    raster.height as i32,
                    Some(raster.data.as_ptr() as *const _),
                    &info,
                    DIB_RGB_COLORS,
                    SRCCOPY,
                );
            }
            y += h;
        }
        if unsafe { EndPage(hdc) } <= 0 {
            bail!("the printer stopped after page {}", i + 1);
        }
        done += 1;
    }
    unsafe {
        EndDoc(hdc);
    }
    Ok(done)
}
