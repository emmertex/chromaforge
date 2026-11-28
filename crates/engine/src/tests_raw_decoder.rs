//! The raw decoder per camera: which decoder a choice reaches, the fallbacks, the checks on a backend's frame, and
//! the `library.preferences` round trip.

use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;

use std::collections::BTreeMap;

use super::*;

/// A stand-in for LibRaw: hands back `frame` (or fails) and counts its calls.
struct Fake {
    frame: Result<Frame, String>,
    calls: AtomicUsize,
}

impl Fake {
    fn new(frame: Result<Frame, String>) -> Fake {
        Fake { frame, calls: AtomicUsize::new(0) }
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }
}

impl Backend for Fake {
    fn name(&self) -> String {
        "fake LibRaw".into()
    }
    fn decode(&self, _bytes: &[u8]) -> Result<Frame, String> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.frame.clone()
    }
}

/// An RGGB mosaic with a two-pixel masked border on the left, as LibRaw would report it.
fn frame() -> Frame {
    let (w, h) = (34usize, 24usize);
    let data = (0..w * h).map(|i| 600 + (i % 97) as u16 * 30).collect();
    Frame {
        width: w,
        height: h,
        data,
        cfa: lcc_raw::Cfa::bayer("RGGB"),
        bits: 14,
        black: lcc_raw::BlackLevel::uniform(512.0),
        white: 16383.0,
        active_area: lcc_raw::Rect::new(2, 0, 32, 24),
        wb_multipliers: Some([2.0, 1.0, 1.5]),
        xyz_to_camera: Some(lcc_raw::Mat3([[0.7, -0.1, -0.05], [-0.4, 1.2, 0.2], [-0.1, 0.2, 0.6]])),
    }
}

fn dng() -> Vec<u8> {
    crate::tests_xmp::synthetic_dng_with(None, Default::default())
}

/// Probes as a raw container this app doesn't decode yet (Sigma X3F).
fn unsupported() -> Vec<u8> {
    let mut b = b"FOVb".to_vec();
    b.resize(4096, 0);
    b
}

#[test]
fn ids_read_as_people_type_them() {
    assert_eq!(Decoder::parse("LibRaw"), Some(Decoder::Libraw));
    assert_eq!(Decoder::parse("lcc-raw"), Some(Decoder::Lcc));
    assert_eq!(Decoder::parse("lccraw"), Some(Decoder::Lcc));
    assert_eq!(Decoder::parse(" Auto "), Some(Decoder::Auto));
    assert_eq!(Decoder::parse("dcraw"), None);
    for d in Decoder::ALL {
        assert_eq!(Decoder::parse(d.id()), Some(d));
    }
}

#[test]
fn auto_and_lcc_read_with_lcc_raw_and_never_ask_libraw() {
    let bytes = dng();
    let fake = Fake::new(Ok(frame()));
    for choice in [Decoder::Auto, Decoder::Lcc] {
        let (raw, used) = decode_as(&bytes, choice, Some(&fake)).unwrap();
        assert_eq!(used, Used::Lcc);
        assert_eq!(raw, lcc_raw::decode(&bytes).unwrap(), "{choice:?} reads exactly what lcc-raw reads");
    }
    assert_eq!(fake.calls(), 0);
}

#[test]
fn libraw_reads_when_chosen_and_lcc_raw_takes_over_when_it_fails_or_is_missing() {
    let bytes = dng();
    let fake = Fake::new(Ok(frame()));
    let (raw, used) = decode_as(&bytes, Decoder::Libraw, Some(&fake)).unwrap();
    assert_eq!((used, fake.calls()), (Used::Libraw, 1));
    assert_eq!((raw.width, raw.height, raw.active_area), (34, 24, lcc_raw::Rect::new(2, 0, 32, 24)));
    assert_eq!(raw.wb_multipliers, Some([2.0, 1.0, 1.5]));
    // the file's metadata still comes with it
    assert_eq!(raw.metadata, lcc_raw::probe_info(&bytes).unwrap().metadata);
    // a failing LibRaw, and none at all (web, Android), leave the photo to lcc-raw
    let failing = Fake::new(Err("unsupported file format".into()));
    assert_eq!(decode_as(&bytes, Decoder::Libraw, Some(&failing)).unwrap().1, Used::Lcc);
    assert_eq!(decode_as(&bytes, Decoder::Libraw, None).unwrap().1, Used::Lcc);
}

#[test]
fn auto_hands_libraw_only_the_variants_lcc_raw_refuses() {
    let bytes = unsupported();
    assert!(matches!(lcc_raw::decode(&bytes), Err(lcc_raw::RawError::Unsupported(_))), "the fixture is an unsupported variant");
    let fake = Fake::new(Ok(frame()));
    assert_eq!(decode_as(&bytes, Decoder::Auto, Some(&fake)).unwrap().1, Used::Libraw);
    // without LibRaw (or when it can't either) the refusal stands, so the embedded preview is shown as before
    assert!(matches!(decode_as(&bytes, Decoder::Auto, None), Err(lcc_raw::RawError::Unsupported(_))));
    let failing = Fake::new(Err("no".into()));
    assert!(matches!(decode_as(&bytes, Decoder::Auto, Some(&failing)), Err(lcc_raw::RawError::Unsupported(_))));
    // lcc-raw only: never LibRaw
    let before = fake.calls();
    assert!(decode_as(&bytes, Decoder::Lcc, Some(&fake)).is_err());
    assert_eq!(fake.calls(), before);
}

#[test]
fn a_frame_without_app_headers_takes_libraws_matrix_and_the_whole_image_area() {
    let raw = frame().into_raw_image(&unsupported()).unwrap();
    assert_eq!(raw.crop, lcc_raw::Rect::new(0, 0, 32, 24));
    assert_eq!(raw.color.color_matrix[0], frame().xyz_to_camera);
    assert_eq!(raw.color.illuminant[0], 21);
    // and it develops like any other mosaic
    assert!(raw.normalized().unwrap().data.iter().all(|v| v.is_finite()));
}

/// Hostile frames from a backend are errors, never panics or huge allocations.
#[test]
fn hostile_frames_are_refused() {
    let bytes = dng();
    let cases: Vec<(&str, Spoil)> = vec![
        ("no samples", Box::new(|f| f.data.clear())),
        ("short data", Box::new(|f| f.data.truncate(10))),
        ("zero size", Box::new(|f| f.width = 0)),
        ("overflowing size", Box::new(|f| (f.width, f.height) = (usize::MAX, 3))),
        ("oversized", Box::new(|f| (f.width, f.height) = (100_000, 100_000))),
        ("area past the data", Box::new(|f| f.active_area = lcc_raw::Rect::new(10, 0, 32, 24))),
        ("area overflows", Box::new(|f| f.active_area = lcc_raw::Rect::new(usize::MAX, 0, 2, 2))),
        ("empty area", Box::new(|f| f.active_area = lcc_raw::Rect::new(0, 0, 0, 24))),
        ("bad colour index", Box::new(|f| f.cfa = Some(lcc_raw::Cfa { width: 2, height: 2, pattern: vec![0, 1, 1, 7] }))),
        ("pattern size", Box::new(|f| f.cfa = Some(lcc_raw::Cfa { width: 2, height: 2, pattern: vec![0, 1] }))),
        ("huge pattern", Box::new(|f| f.cfa = Some(lcc_raw::Cfa { width: 64, height: 1, pattern: vec![1; 64] }))),
        ("white below black", Box::new(|f| f.white = 100.0)),
        ("white not a number", Box::new(|f| f.white = f32::NAN)),
        ("white past 16 bits", Box::new(|f| f.white = 1e9)),
        ("black pattern", Box::new(|f| f.black = lcc_raw::BlackLevel { repeat_rows: 2, repeat_cols: 2, values: vec![1.0], ..Default::default() })),
        ("black zero repeat", Box::new(|f| f.black = lcc_raw::BlackLevel { repeat_rows: 0, repeat_cols: 0, values: vec![], ..Default::default() })),
        ("black infinite", Box::new(|f| f.black = lcc_raw::BlackLevel::uniform(f32::INFINITY))),
    ];
    for (what, spoil) in cases {
        let mut f = frame();
        spoil(&mut f);
        assert!(f.into_raw_image(&bytes).is_err(), "{what}");
    }
    // odd but harmless values are cleaned instead
    let mut f = frame();
    f.wb_multipliers = Some([f32::NAN, 1.0, 1.0]);
    f.xyz_to_camera = Some(lcc_raw::Mat3([[f64::NAN; 3]; 3]));
    let raw = f.into_raw_image(&unsupported()).unwrap();
    assert_eq!(raw.wb_multipliers, None);
    assert_eq!(raw.color.color_matrix[0], None);
}

/// The decoder choices are one per process: the tests that set them take turns.
static CHOICES: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn choices_are_set_cleared_and_saved_with_the_library() {
    let _turn = CHOICES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = crate::Session::with_demo();
    let set = |s: &mut crate::Session, v: serde_json::Value| s.execute("library.preferences", &json!({"rawDecoder": v}));
    assert!(set(&mut s, json!({"camera": "", "decoder": "libraw"})).is_err(), "a camera is required");
    assert!(set(&mut s, json!({"camera": "Example X1", "decoder": "dcraw"})).is_err());
    let out = set(&mut s, json!({"camera": "Example X1", "decoder": "libraw"})).unwrap();
    assert_eq!(out["rawDecoders"]["Example X1"], "libraw");
    // (the decoder's own copy is one per process and any library opened by a test running alongside replaces it, so
    // the lookup is checked on the session's map)
    let installed = |s: &crate::Session| s.raw_decoders.iter().map(|(c, d)| (camera_key(c), *d)).collect::<BTreeMap<_, _>>();
    assert_eq!(choice_in(&installed(&s), "EXAMPLE  x1"), Decoder::Libraw, "matched as Info shows the camera, any case");
    // the same camera spelled differently replaces the entry; auto removes it
    set(&mut s, json!({"camera": "EXAMPLE X1", "decoder": "lcc-raw"})).unwrap();
    assert_eq!(s.raw_decoders.len(), 1);
    assert_eq!(choice_in(&installed(&s), "Example X1"), Decoder::Lcc);
    set(&mut s, json!({"camera": "Example X1", "decoder": "auto"})).unwrap();
    assert!(s.raw_decoders.is_empty());
    assert_eq!(choice_in(&installed(&s), "Example X1"), Decoder::Auto);
}

#[test]
fn choices_persist_with_the_library_and_reinstall_on_reopen() {
    let _turn = CHOICES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let lib = std::env::temp_dir().join(format!("lc-raw-decoder-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&lib);
    std::fs::create_dir_all(&lib).unwrap();
    {
        let mut s = crate::Session::new().with_fs();
        s.open_library(&lib, true).unwrap();
        s.execute("library.preferences", &json!({"rawDecoder": {"camera": "Example X2", "decoder": "libraw"}})).unwrap();
    }
    {
        let mut s = crate::Session::new().with_fs();
        s.open_library(&lib, true).unwrap();
        assert_eq!(s.raw_decoders.get("Example X2"), Some(&Decoder::Libraw));
        s.execute("library.preferences", &json!({"rawDecoder": {"camera": "Example X2", "decoder": null}})).unwrap();
    }
    install_choices(&std::collections::BTreeMap::new());
    let _ = std::fs::remove_dir_all(&lib);
}

/// A change that makes a description hostile.
type Spoil = Box<dyn Fn(&mut Frame)>;

/// A TIFF container with nothing in it but its Exif make and model: no raw data and no colour tags of its own.
fn tiff_of(make: &str, model: &str) -> Vec<u8> {
    let (make, model) = (format!("{make}\0"), format!("{model}\0"));
    let ifd = 8usize;
    let strings = ifd + 2 + 2 * 12 + 4;
    let mut b = b"II*\0".to_vec();
    b.extend((ifd as u32).to_le_bytes());
    b.extend(2u16.to_le_bytes());
    for (tag, at, s) in [(0x010Fu16, strings, &make), (0x0110, strings + make.len(), &model)] {
        b.extend(tag.to_le_bytes());
        b.extend(2u16.to_le_bytes());
        b.extend((s.len() as u32).to_le_bytes());
        b.extend((at as u32).to_le_bytes());
    }
    b.extend(0u32.to_le_bytes());
    b.extend(make.as_bytes());
    b.extend(model.as_bytes());
    b
}

/// LibRaw's matrix is the last resort: a body the app's camera table has matrices for keeps the table's (applied when the
/// file has none), exactly as when lcc-raw reads it; a body without any gets LibRaw's.
#[test]
fn libraws_matrix_is_used_only_when_neither_the_file_nor_the_camera_table_has_one() {
    // the table knows this body (the app's matrices, never the file's)
    assert!(app_table_has_matrix(Some("NIKON CORPORATION"), Some("NIKON Z 8")));
    let known = tiff_of("NIKON CORPORATION", "NIKON Z 8");
    let raw = frame().into_raw_image(&known).unwrap();
    assert_eq!(raw.metadata.model.as_deref(), Some("NIKON Z 8"), "the synthetic file reads as that body");
    assert!(!lcc_raw::color::has_matrix(&raw.color), "the table's matrices are applied later, LibRaw's must not stand in for them");

    // a body the table has nothing for takes LibRaw's
    assert!(!app_table_has_matrix(Some("NOMAKE"), Some("NO SUCH CAMERA 1")));
    let unknown = tiff_of("NOMAKE", "NO SUCH CAMERA 1");
    let raw = frame().into_raw_image(&unknown).unwrap();
    assert_eq!(raw.color.color_matrix[0], frame().xyz_to_camera);
    assert_eq!(raw.color.illuminant[0], 21);

    // a file that is no more than a name has no model at all
    assert!(!app_table_has_matrix(Some("NIKON CORPORATION"), None));
}
