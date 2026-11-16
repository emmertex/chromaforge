use super::*;

fn job(printer: &str, pages: usize) -> Job {
    Job { printer: printer.into(), title: "test".into(), copies: 1, pages, paper_mm: [210.0, 297.0], output: None }
}

fn blank(w: usize, h: usize) -> PageImage {
    PageImage { rgba: vec![255; w * h * 4], width: w, height: h }
}

#[test]
fn hostile_jobs_are_errors_not_panics() {
    let cases = vec![
        job("", 1),
        job("p\0", 1),
        job(&"x".repeat(300), 1),
        job("p", 0),
        Job { paper_mm: [f32::NAN, 297.0], ..job("p", 1) },
        Job { paper_mm: [0.0, 297.0], ..job("p", 1) },
        Job { paper_mm: [f32::INFINITY, 1e9], ..job("p", 1) },
        Job { output: Some("a\0b".into()), ..job("p", 1) },
    ];
    for j in &cases {
        assert!(matches!(validate(j), Err(Error::Invalid(_))), "{j:?}");
        assert!(matches!(print(j, &mut |_| Ok(blank(2, 2)), &mut |_, _| true), Err(Error::Invalid(_))), "{j:?}");
    }
    let bad_pages = [
        PageImage { rgba: vec![0; 16], width: 0, height: 4 },
        PageImage { rgba: vec![0; 16], width: 5, height: 4 },
        PageImage { rgba: vec![], width: usize::MAX, height: usize::MAX },
        PageImage { rgba: vec![0; 3], width: 1, height: 1 },
    ];
    for p in &bad_pages {
        assert!(matches!(p.validate(), Err(Error::Invalid(_))), "{p:?}");
    }
    assert!(blank(3, 2).validate().is_ok());
}

#[test]
fn placement_fits_centres_and_rejects_empty_pages() {
    // A4 portrait page on an A4 device page at 600 dpi: fills it, offset subtracted
    let p = placement([2480, 3508], [4960, 7016], [100, 120]).unwrap();
    assert_eq!(p, [-100, -120, 4960, 7016]);
    // a landscape layout on a portrait device page: scaled to the width, centred vertically
    let [x, y, w, h] = placement([3000, 2000], [3000, 4000], [0, 0]).unwrap();
    assert_eq!((x, w), (0, 3000));
    assert!(y > 0 && y + h <= 4000 && (y - (4000 - y - h)).abs() <= 1);
    assert!(placement([0, 10], [10, 10], [0, 0]).is_none());
    assert!(placement([10, 10], [0, 10], [0, 0]).is_none());
    assert!(placement([10, 10], [-5, 10], [0, 0]).is_none());
}

#[test]
fn bands_are_bgr_padded_and_short_rasters_stay_white() {
    let px = [1u8, 2, 3, 255, 4, 5, 6, 255];
    let b = bgr_band(&px, 2, 0, 1);
    assert_eq!(b, vec![3, 2, 1, 6, 5, 4, 0xff, 0xff]);
    // rows past the end: white, no panic
    let b = bgr_band(&px, 2, 0, 3);
    assert_eq!(b.len(), 24);
    assert!(b.get(8..).unwrap().iter().all(|&v| v == 255));
    assert!(bgr_band(&[], 3, 5, 2).is_empty());
}

#[test]
fn printers_list_or_report_unsupported() {
    match printers() {
        Ok(list) => {
            assert!(list.iter().all(|p| !p.name.is_empty()));
            // the default (if any) comes first
            assert!(list.iter().skip(1).all(|p| !p.default));
        }
        Err(e) => assert!(!AVAILABLE && e == Error::Unsupported, "{e}"),
    }
}

/// Prints a two-page job through Microsoft Print to PDF into a file (no dialog), where it exists.
#[test]
fn printing_to_microsoft_print_to_pdf_writes_a_pdf() {
    const NAME: &str = "Microsoft Print to PDF";
    let has = printers().map(|l| l.iter().any(|p| p.name == NAME)).unwrap_or(false);
    if !has {
        lcc_testkit::skip!(System, "no {NAME} printer");
        return;
    }
    let dir = std::env::temp_dir().join(format!("winspool-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("out.pdf");
    let _ = std::fs::remove_file(&path);
    let (w, h) = (620, 877); // A4 at 75 dpi
    let mut j = job(NAME, 2);
    j.output = Some(path.clone());
    let mut asked = Vec::new();
    let mut seen = Vec::new();
    let r = print(
        &j,
        &mut |i| {
            asked.push(i);
            let mut p = blank(w, h);
            for (k, px) in p.rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                if (k % w) < w / 2 && i == 0 {
                    *px = [200, 30, 30, 255];
                }
            }
            Ok(p)
        },
        &mut |i, n| {
            seen.push((i, n));
            true
        },
    )
    .unwrap();
    assert_eq!(r.pages, 2);
    assert_eq!(asked, vec![0, 1]);
    assert_eq!(seen, vec![(0, 2), (1, 2)]);
    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.starts_with(b"%PDF"), "not a PDF");
    // cancelling, a page that fails to render and a malformed page are errors (the job is aborted)
    assert_eq!(print(&j, &mut |_| Ok(blank(w, h)), &mut |_, _| false), Err(Error::Cancelled));
    assert!(matches!(print(&j, &mut |_| Err("no photo".into()), &mut |_, _| true), Err(Error::Render(_))));
    let bad = PageImage { rgba: vec![0; 8], width: w, height: h };
    assert!(matches!(print(&j, &mut |_| Ok(bad.clone()), &mut |_, _| true), Err(Error::Invalid(_))));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_unknown_printer_is_a_system_error() {
    let r = print(&job("no such printer 7f3a", 1), &mut |_| Ok(blank(2, 2)), &mut |_, _| true);
    if AVAILABLE {
        assert!(matches!(r, Err(Error::System(_))), "{r:?}");
    } else {
        assert_eq!(r, Err(Error::Unsupported));
    }
}
