//! Sending a print to an IPP printer: pick the format the printer takes (PDF, else one JPEG job
//! per page), submit with the copy count, then follow the job until it finishes or `wait` runs out.
//! On Windows a printer named by its system name (what the Windows Print dialog shows, e.g.
//! `HP OfficeJet` or `\\server\printer`) goes through the print spooler instead
//! ([`lcc_winspool`]): each page is rendered at the job's resolution and drawn on the printer.
//!
//! Sources: RFC 8011 (Print-Job, Get-Job-Attributes, job states). Own design.

use std::time::{Duration, Instant};

use lcc_layout::render::{PhotoSource, RenderOptions, render_page};
use lcc_layout::{Document, PT_PER_INCH};

use crate::ipp::{self, JobState};
use crate::output::{PdfOptions, to_jpeg_icc, to_pdf};
use crate::text::ShapedText;
use crate::{PrintError, PrintSettings, Result};

/// How a job went to the printer.
#[derive(Clone, Debug, PartialEq)]
pub struct Sent {
    /// `application/pdf` or `image/jpeg`.
    pub format: String,
    /// One job id per submitted document.
    pub jobs: Vec<i32>,
    pub pages: usize,
    pub copies: u32,
    /// The last job's state when the wait ended (`None` when the printer did not say).
    pub state: Option<JobState>,
    pub warnings: Vec<String>,
}

/// Who sends, and how long to follow the job.
#[derive(Clone, Debug)]
pub struct SendOptions {
    pub user: String,
    pub title: String,
    pub pdf: PdfOptions,
    /// How long to poll the job state after submitting (zero: don't poll).
    pub wait: Duration,
    pub poll: Duration,
}

/// Whether `printer` is a system printer name (the Windows print spooler) rather than a URI.
pub fn is_system_printer(printer: &str) -> bool {
    let p = printer.trim();
    !p.is_empty() && !p.contains("://")
}

/// Checks `printer` before anything is rendered: an `ipp://` / `ipps://` URI, or on Windows a
/// system printer name.
pub fn check_printer(printer: &str) -> Result<()> {
    if is_system_printer(printer) && lcc_winspool::AVAILABLE {
        return Ok(());
    }
    ipp::IppUri::parse(printer).map(|_| ())
}

/// The printers to offer: on Windows the system's printers first (by name, the default first),
/// then those the CUPS / IPP `server` shares. A CUPS server that doesn't answer is an error only
/// when there are no system printers either.
pub fn find_printers(server: &str) -> Result<Vec<ipp::PrinterEntry>> {
    let mut out: Vec<ipp::PrinterEntry> = match lcc_winspool::printers() {
        Ok(list) => list
            .into_iter()
            .map(|p| ipp::PrinterEntry {
                info: if p.default { "(Windows, default)".into() } else { "(Windows)".into() },
                uri: p.name.clone(),
                name: p.name,
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    match ipp::cups_printers(server) {
        Ok(more) => out.extend(more),
        Err(e) if out.is_empty() => return Err(e),
        Err(_) => {}
    }
    Ok(out)
}

/// Renders `doc` and prints it on `uri`.
///
/// The printer is asked which formats it takes; when it can't be asked, PDF is sent. A job the
/// printer aborts or cancels is an error. On Windows a system printer name prints through the
/// spooler ([`send_to_system_printer`]).
pub fn send_to_printer(
    uri: &str,
    doc: &Document,
    settings: &PrintSettings,
    photos: &dyn PhotoSource,
    text: &ShapedText,
    opt: &SendOptions,
    progress: &mut dyn FnMut(usize, usize) -> bool,
) -> Result<Sent> {
    if is_system_printer(uri) && lcc_winspool::AVAILABLE {
        return send_to_system_printer(uri, doc, settings, photos, text, opt, None, progress);
    }
    ipp::IppUri::parse(uri)?;
    let copies = settings.job.copies.clamp(1, 999);
    let mut warnings = Vec::new();
    let formats = match ipp::printer_info(uri) {
        Ok(info) => Some(info),
        Err(e) => {
            warnings.push(format!("could not ask the printer for its formats ({e}); sending PDF"));
            None
        }
    };
    let pdf_ok = formats.as_ref().is_none_or(|i| i.accepts("application/pdf"));
    let jpeg_ok = formats.as_ref().is_some_and(|i| i.accepts("image/jpeg"));
    let (format, files, pages) = if pdf_ok {
        let out = to_pdf(doc, settings, photos, text, &opt.pdf, progress)?;
        warnings.extend(out.warnings);
        ("application/pdf", out.files, out.pages)
    } else if jpeg_ok {
        let out = to_jpeg_icc(doc, settings, photos, text, opt.pdf.photo_profile.as_deref().map(Vec::as_slice), progress)?;
        warnings.extend(out.warnings);
        ("image/jpeg", out.files, out.pages)
    } else {
        let list = formats.map(|i| i.formats.join(", ")).unwrap_or_default();
        return Err(PrintError::Ipp(format!("the printer takes neither PDF nor JPEG (it takes: {list})")));
    };
    // the printer's own options, less those it says it doesn't offer
    let driver = match &formats {
        Some(info) => {
            let (d, w) = settings.job.driver.supported_by(&info.options);
            warnings.extend(w);
            d
        }
        None => settings.job.driver.clone(),
    };
    let mut jobs = Vec::new();
    let n = files.len();
    for (i, bytes) in files.iter().enumerate() {
        let name = if n > 1 { format!("{} ({}/{n})", opt.title, i + 1) } else { opt.title.clone() };
        jobs.push(ipp::print_job_with(uri, &opt.user, &name, format, copies, &driver, bytes)?);
    }
    let state = match jobs.last() {
        Some(&id) if id > 0 && !opt.wait.is_zero() => follow(uri, &opt.user, id, opt.wait, opt.poll),
        _ => None,
    };
    if let Some(s) = &state
        && (s.state == JobState::ABORTED || s.state == JobState::CANCELED)
    {
        let why = if s.message.is_empty() { s.reasons.join(", ") } else { s.message.clone() };
        return Err(PrintError::Ipp(format!("the printer {} the job: {why}", s.label())));
    }
    Ok(Sent { format: format.to_string(), jobs, pages, copies, state, warnings })
}

/// Prints `doc` on the Windows printer `name` through the spooler: every page rendered at the
/// job's resolution (8 bits; the photos already in the printer profile's space when one is set)
/// and handed to the driver with the paper size, orientation and copy count. `output` prints to
/// a file instead of the printer's port (drivers such as Microsoft Print to PDF). The spooler
/// reports no job state, and the IPP-only printer options (paper type, tray, quality, sides) are
/// set in the printer's own preferences instead.
#[allow(clippy::too_many_arguments)]
pub fn send_to_system_printer(
    name: &str,
    doc: &Document,
    settings: &PrintSettings,
    photos: &dyn PhotoSource,
    text: &ShapedText,
    opt: &SendOptions,
    output: Option<std::path::PathBuf>,
    progress: &mut dyn FnMut(usize, usize) -> bool,
) -> Result<Sent> {
    doc.validate()?;
    let copies = settings.job.copies.clamp(1, 999);
    let pages = doc.pages.len();
    let mm = 25.4 / PT_PER_INCH;
    let job = lcc_winspool::Job {
        printer: name.trim().to_string(),
        title: opt.title.clone(),
        copies,
        pages,
        paper_mm: [doc.page.size.w * mm, doc.page.size.h * mm],
        output,
    };
    let mut warnings = Vec::new();
    if settings.job.color.bit16 {
        warnings.push("the Windows print spooler takes 8-bit pages; 16-bit output needs PDF".into());
    }
    if !settings.job.driver.is_empty() {
        warnings.push("paper type, tray, quality and two-sided printing are set in the printer's own preferences for a Windows printer".into());
    }
    let dpi = settings.job.dpi;
    let mut page_warnings = Vec::new();
    let r = lcc_winspool::print(
        &job,
        &mut |i| {
            let p = doc.pages.get(i).ok_or("no such page")?;
            let ro = RenderOptions { dpi, include_bleed: false, show_guides: false, page: i + 1, pages };
            let r = render_page(&doc.page, p, photos, text, &ro).map_err(|e| e.to_string())?;
            page_warnings.extend(r.warnings.into_iter().map(|w| format!("page {}: {w}", i + 1)));
            let (width, height) = (r.image.width, r.image.height);
            Ok(lcc_winspool::PageImage { rgba: r.image.data.into_flattened(), width, height })
        },
        progress,
    );
    let printed = match r {
        Ok(p) => p,
        Err(lcc_winspool::Error::Cancelled) => return Err(PrintError::Cancelled),
        Err(e) => return Err(PrintError::Ipp(format!("{}: {e}", name.trim()))),
    };
    warnings.extend(page_warnings);
    warnings.extend(printed.warnings);
    Ok(Sent { format: "windows-spooler".into(), jobs: Vec::new(), pages: printed.pages, copies, state: None, warnings })
}

/// Polls the job state until it is final or `wait` passes; `None` when the printer won't say.
fn follow(uri: &str, user: &str, id: i32, wait: Duration, poll: Duration) -> Option<JobState> {
    let until = Instant::now() + wait;
    let mut last = None;
    loop {
        match ipp::job_state(uri, user, id) {
            Ok(s) => {
                let done = s.is_final();
                last = Some(s);
                if done {
                    return last;
                }
            }
            Err(_) => return last,
        }
        if Instant::now() >= until {
            return last;
        }
        std::thread::sleep(poll.min(until.saturating_duration_since(Instant::now())));
    }
}
