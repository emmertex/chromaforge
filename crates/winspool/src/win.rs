//! The Win32 calls. Every pointer handed to Windows points into a buffer this module owns for the
//! whole call; every handle is closed by a guard on every path.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDCW, DEVMODEW, DIB_RGB_COLORS, DM_COPIES, DM_IN_BUFFER, DM_ORIENTATION, DM_OUT_BUFFER,
    DM_PAPERLENGTH, DM_PAPERSIZE, DM_PAPERWIDTH, DMORIENT_LANDSCAPE, DMORIENT_PORTRAIT, DeleteDC, GetDeviceCaps, HALFTONE, HDC, PHYSICALHEIGHT,
    PHYSICALOFFSETX, PHYSICALOFFSETY, PHYSICALWIDTH, SRCCOPY, SetStretchBltMode, StretchDIBits,
};
use windows_sys::Win32::Graphics::Printing::{
    ClosePrinter, DocumentPropertiesW, EnumPrintersW, GetDefaultPrinterW, OpenPrinterW, PRINTER_ENUM_CONNECTIONS, PRINTER_ENUM_LOCAL, PRINTER_HANDLE,
    PRINTER_INFO_4W,
};
use windows_sys::Win32::Storage::Xps::{AbortDoc, DOCINFOW, EndDoc, EndPage, StartDocW, StartPage};

use crate::{BAND_ROWS, Error, Job, PageImage, Printed, Printer, Result, bgr_band, placement};

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

fn last_error(what: &str) -> Error {
    Error::System(format!("{what} failed: {}", std::io::Error::last_os_error()))
}

/// Reads a NUL-terminated UTF-16 string Windows wrote into a buffer we own.
///
/// # Safety
/// `p` is null or points to a NUL-terminated UTF-16 string inside a live buffer.
unsafe fn from_wide(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0usize;
    // SAFETY: the caller guarantees a NUL terminator inside the live buffer.
    while unsafe { *p.add(n) } != 0 && n < 32_768 {
        n += 1;
    }
    // SAFETY: the `n` units before the terminator are in the buffer.
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, n) })
}

pub fn default_printer() -> Option<String> {
    let mut len = 0u32;
    // SAFETY: a null buffer with a length out-pointer asks for the size only.
    unsafe { GetDefaultPrinterW(null_mut(), &mut len) };
    if len == 0 || len > 32_768 {
        return None;
    }
    let mut buf = vec![0u16; len as usize];
    // SAFETY: `buf` holds `len` units, as `len` says.
    let ok = unsafe { GetDefaultPrinterW(buf.as_mut_ptr(), &mut len) };
    if ok == 0 {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(buf.get(..end).unwrap_or(&[])))
}

pub fn printers() -> Result<Vec<Printer>> {
    let flags = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
    let (mut needed, mut count) = (0u32, 0u32);
    // SAFETY: a null buffer of size 0 asks for the needed size.
    unsafe { EnumPrintersW(flags, null(), 4, null_mut(), 0, &mut needed, &mut count) };
    if needed == 0 {
        return Ok(Vec::new());
    }
    if needed > 64 << 20 {
        return Err(Error::System("printer list too large".into()));
    }
    // u64 elements: aligned for PRINTER_INFO_4W
    let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
    let bytes = u32::try_from(buf.len() * 8).map_err(|_| Error::System("printer list too large".into()))?;
    // SAFETY: `buf` is `bytes` long and 8-byte aligned; Windows fills it with `count` records and
    // the strings they point to, all inside the buffer.
    let ok = unsafe { EnumPrintersW(flags, null(), 4, buf.as_mut_ptr().cast(), bytes, &mut needed, &mut count) };
    if ok == 0 {
        return Err(last_error("EnumPrinters"));
    }
    let max = buf.len() * 8 / size_of::<PRINTER_INFO_4W>();
    let default = default_printer();
    let mut out = Vec::new();
    for i in 0..(count as usize).min(max) {
        // SAFETY: record `i` lies inside `buf` (checked against `max`) and is properly aligned.
        let rec = unsafe { &*buf.as_ptr().cast::<PRINTER_INFO_4W>().add(i) };
        // SAFETY: the name points to a NUL-terminated string Windows wrote into `buf`.
        let name = unsafe { from_wide(rec.pPrinterName) };
        if !name.is_empty() {
            out.push(Printer { default: default.as_deref() == Some(name.as_str()), name });
        }
    }
    Ok(out)
}

struct PrinterGuard(PRINTER_HANDLE);
impl Drop for PrinterGuard {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful OpenPrinterW and is closed once.
        unsafe { ClosePrinter(self.0) };
    }
}

struct DcGuard {
    dc: HDC,
    doc_open: bool,
}
impl Drop for DcGuard {
    fn drop(&mut self) {
        // SAFETY: `dc` came from a successful CreateDCW; a document left open is aborted first.
        unsafe {
            if self.doc_open {
                AbortDoc(self.dc);
            }
            DeleteDC(self.dc);
        }
    }
}

/// The driver's DEVMODE for `name`, with paper size, orientation and copies asked for.
fn devmode(name: &[u16], paper_mm: [f32; 2], copies: u32) -> Result<Vec<u64>> {
    let mut h = PRINTER_HANDLE { Value: null_mut() };
    // SAFETY: `name` is NUL-terminated; `h` receives the handle.
    if unsafe { OpenPrinterW(name.as_ptr(), &mut h, null()) } == 0 {
        return Err(last_error("OpenPrinter"));
    }
    let g = PrinterGuard(h);
    // SAFETY: mode 0 with null buffers returns the DEVMODE size.
    let size = unsafe { DocumentPropertiesW(null_mut(), g.0, name.as_ptr(), null_mut(), null(), 0) };
    if size < size_of::<DEVMODEW>() as i32 || size > 1 << 20 {
        return Err(last_error("DocumentProperties"));
    }
    let mut buf = vec![0u64; (size as usize).div_ceil(8)];
    let dm = buf.as_mut_ptr().cast::<DEVMODEW>();
    // SAFETY: `buf` holds `size` bytes, 8-byte aligned, as the driver asked.
    if unsafe { DocumentPropertiesW(null_mut(), g.0, name.as_ptr(), dm, null(), DM_OUT_BUFFER) } < 0 {
        return Err(last_error("DocumentProperties"));
    }
    let [w, hgt] = paper_mm;
    let landscape = w > hgt;
    let (short, long) = if landscape { (hgt, w) } else { (w, hgt) };
    // SAFETY: `dm` points at the DEVMODEW the driver just wrote; the printer fields are the
    // `Anonymous1.Anonymous1` arm of the union.
    unsafe {
        let d = &mut *dm;
        let p = &mut d.Anonymous1.Anonymous1;
        p.dmOrientation = if landscape { DMORIENT_LANDSCAPE } else { DMORIENT_PORTRAIT } as i16;
        p.dmPaperSize = 0;
        p.dmPaperWidth = (short * 10.0).round().clamp(1.0, f32::from(i16::MAX)) as i16;
        p.dmPaperLength = (long * 10.0).round().clamp(1.0, f32::from(i16::MAX)) as i16;
        p.dmCopies = copies.clamp(1, 999) as i16;
        d.dmFields = (d.dmFields & !DM_PAPERSIZE) | DM_ORIENTATION | DM_PAPERWIDTH | DM_PAPERLENGTH | DM_COPIES;
    }
    // SAFETY: input and output are the same valid DEVMODE buffer, which the driver merges in place.
    if unsafe { DocumentPropertiesW(null_mut(), g.0, name.as_ptr(), dm, dm, DM_IN_BUFFER | DM_OUT_BUFFER) } < 0 {
        return Err(last_error("DocumentProperties"));
    }
    Ok(buf)
}

pub fn print(
    job: &Job,
    render: &mut dyn FnMut(usize) -> std::result::Result<PageImage, String>,
    progress: &mut dyn FnMut(usize, usize) -> bool,
) -> Result<Printed> {
    let name = wide(&job.printer);
    let dm = devmode(&name, job.paper_mm, job.copies)?;
    // SAFETY: `name` is NUL-terminated; `dm` is the driver's DEVMODE, alive for the call.
    let dc = unsafe { CreateDCW(null(), name.as_ptr(), null(), dm.as_ptr().cast()) };
    if dc.is_null() {
        return Err(last_error("CreateDC"));
    }
    let mut g = DcGuard { dc, doc_open: false };
    let title = wide(if job.title.is_empty() { "Print" } else { &job.title });
    let output = job.output.as_ref().map(|p| wide(&p.to_string_lossy()));
    let info = DOCINFOW {
        cbSize: size_of::<DOCINFOW>() as i32,
        lpszDocName: title.as_ptr(),
        lpszOutput: output.as_ref().map_or(null(), |o| o.as_ptr()),
        lpszDatatype: null(),
        fwType: 0,
    };
    // SAFETY: `dc` is a printer DC; `info` and its strings live across the call.
    if unsafe { StartDocW(g.dc, &info) } <= 0 {
        return Err(last_error("StartDoc"));
    }
    g.doc_open = true;
    // SAFETY: plain queries on a valid DC.
    let caps = |i| unsafe { GetDeviceCaps(g.dc, i as i32) };
    let phys = [caps(PHYSICALWIDTH), caps(PHYSICALHEIGHT)];
    let offset = [caps(PHYSICALOFFSETX), caps(PHYSICALOFFSETY)];
    let mut out = Printed::default();
    let n = job.pages;
    for i in 0..n {
        if !progress(i, n) {
            return Err(Error::Cancelled);
        }
        let page = render(i).map_err(|e| Error::Render(format!("page {}: {e}", i + 1)))?;
        page.validate().map_err(|e| Error::Invalid(format!("page {}: {e}", i + 1)))?;
        let Some([x, y, w, h]) = placement([page.width, page.height], phys, offset) else {
            return Err(Error::System(format!("the printer reports no page size ({}×{})", phys[0], phys[1])));
        };
        let src_aspect = page.width as f64 / page.height as f64;
        let dev_aspect = f64::from(phys[0]) / f64::from(phys[1].max(1));
        if i == 0 && (src_aspect / dev_aspect - 1.0).abs() > 0.02 {
            out.warnings.push("the printer's paper differs from the layout's: pages are scaled to fit".into());
        }
        // SAFETY: valid DC with an open document.
        if unsafe { StartPage(g.dc) } <= 0 {
            return Err(last_error("StartPage"));
        }
        // SAFETY: valid DC.
        unsafe { SetStretchBltMode(g.dc, HALFTONE) };
        let scale = f64::from(h) / page.height as f64;
        let mut y0 = 0usize;
        while y0 < page.height {
            let y1 = (y0 + BAND_ROWS).min(page.height);
            let band = bgr_band(&page.rgba, page.width, y0, y1);
            let rows = (y1 - y0) as i32;
            let dy0 = y + (y0 as f64 * scale).round() as i32;
            let dy1 = y + (y1 as f64 * scale).round() as i32;
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: page.width as i32,
                    biHeight: -rows,
                    biPlanes: 1,
                    biBitCount: 24,
                    biCompression: BI_RGB,
                    ..Default::default()
                },
                ..Default::default()
            };
            // SAFETY: `band` holds `rows` top-down 24-bit rows of `page.width` pixels, each padded
            // to 4 bytes, exactly what `bmi` describes; both live across the call.
            let r = unsafe {
                StretchDIBits(g.dc, x, dy0, w, (dy1 - dy0).max(1), 0, 0, page.width as i32, rows, band.as_ptr().cast(), &bmi, DIB_RGB_COLORS, SRCCOPY)
            };
            if r == 0 {
                return Err(last_error("StretchDIBits"));
            }
            y0 = y1;
        }
        // SAFETY: valid DC with an open page.
        if unsafe { EndPage(g.dc) } <= 0 {
            return Err(last_error("EndPage"));
        }
        out.pages += 1;
    }
    // SAFETY: valid DC with an open document.
    if unsafe { EndDoc(g.dc) } <= 0 {
        return Err(last_error("EndDoc"));
    }
    g.doc_open = false;
    Ok(out)
}
