use super::*;

/// A Bayer sensor as LibRaw describes one: `rows` names the 2 × 2 layout at the image area's top-left, with the
/// second green written as 3 (LibRaw's way).
fn facts(rows: [[u8; 2]; 2]) -> Facts {
    let mut pattern = [[0u8; 48]; 48];
    for (r, row) in pattern.iter_mut().enumerate() {
        for (c, v) in row.iter_mut().enumerate() {
            *v = rows[r % 2][c % 2];
        }
    }
    Facts {
        raw_width: 40,
        raw_height: 30,
        left_margin: 3,
        top_margin: 1,
        width: 36,
        height: 28,
        filters: 0x9494_9494,
        colors: 3,
        is_mosaic: true,
        fuji_width: 0,
        pattern,
        black: 500,
        cblack: vec![0; 6 + 256],
        maximum: 16000,
        raw_bps: 14,
        cam_mul: [2.0, 1.0, 1.5, 1.0],
        cam_xyz: [[0.7, -0.1, -0.05], [-0.4, 1.2, 0.2], [-0.1, 0.2, 0.6], [0.0; 3]],
    }
}

const RGGB: [[u8; 2]; 2] = [[0, 1], [3, 2]];

fn data(f: &Facts) -> Vec<u16> {
    vec![1000; f.raw_width * f.raw_height]
}

#[test]
fn the_pattern_is_anchored_at_the_first_sample_of_the_data() {
    let f = facts(RGGB);
    let (p, _) = describe(&f).unwrap();
    let p = p.unwrap();
    assert_eq!((p.width, p.height), (2, 2));
    // the image area starts at (3, 1) with red: the data's (0, 0) is one row up and three columns left
    let at = |x: usize, y: usize| p.colors[(y % 2) * 2 + x % 2];
    assert_eq!(at(3, 1), 0, "red at the image area's first pixel");
    assert_eq!(at(4, 1), 1);
    assert_eq!(at(4, 2), 2);
    assert!(p.colors.iter().all(|&c| c <= 2), "LibRaw's second green (3) is green");
}

#[test]
fn an_xtrans_layout_repeats_every_six_pixels() {
    let rows = ["GGRGGB", "GGBGGR", "BRGRBG", "GGBGGR", "GGRGGB", "RBGBRG"];
    let mut f = facts(RGGB);
    for (r, row) in f.pattern.iter_mut().enumerate() {
        for (c, v) in row.iter_mut().enumerate() {
            *v = match rows[r % 6].as_bytes()[c % 6] {
                b'R' => 0,
                b'G' => 1,
                _ => 2,
            };
        }
    }
    let (p, black) = describe(&f).unwrap();
    let p = p.unwrap();
    assert_eq!((p.width, p.height), (6, 6));
    assert_eq!((black.rows, black.cols), (6, 6));
}

#[test]
fn the_black_level_adds_lib_raws_per_colour_and_pattern_parts() {
    let mut f = facts(RGGB);
    f.cblack[0] = 10; // red
    f.cblack[3] = 3; // second green
    let (_, b) = describe(&f).unwrap();
    assert_eq!((b.rows, b.cols), (2, 2));
    assert_eq!(b.values, vec![510.0, 500.0, 503.0, 500.0]);
    // a 2 × 2 black pattern on top
    f.cblack[4] = 2;
    f.cblack[5] = 2;
    f.cblack[6..10].copy_from_slice(&[1, 2, 3, 4]);
    let (_, b) = describe(&f).unwrap();
    assert_eq!(b.values, vec![511.0, 502.0, 506.0, 504.0]);
}

#[test]
fn a_monochrome_sensor_has_no_pattern() {
    let mut f = facts(RGGB);
    f.filters = 0;
    f.colors = 1;
    f.pattern = [[6; 48]; 48];
    let u = f.unpacked(data(&f), "LibRaw".into()).unwrap();
    assert!(u.pattern.is_none());
    assert_eq!(u.black.values, vec![500.0]);
    assert!(u.xyz_to_camera.is_none());
}

#[test]
fn levels_and_colour_numbers_are_read() {
    let f = facts(RGGB);
    let u = f.unpacked(data(&f), "LibRaw".into()).unwrap();
    assert_eq!(u.image_area, [3, 1, 36, 28]);
    assert_eq!((u.white, u.bits), (16000.0, 14));
    assert_eq!(u.as_shot, Some([2.0, 1.0, 1.5]));
    assert!(u.xyz_to_camera.is_some());
    // missing multipliers and matrix are left out, not invented
    let mut f = facts(RGGB);
    f.cam_mul = [0.0; 4];
    f.cam_xyz = [[0.0; 3]; 4];
    let u = f.unpacked(data(&f), "LibRaw".into()).unwrap();
    assert_eq!((u.as_shot, u.xyz_to_camera), (None, None));
}

/// Hostile numbers from LibRaw are errors, never panics or huge allocations.
#[test]
fn hostile_descriptions_are_refused() {
    let cases: Vec<(&str, Spoil)> = vec![
        ("no data", Box::new(|f| f.raw_width = 0)),
        ("overflowing size", Box::new(|f| (f.raw_width, f.raw_height) = (usize::MAX, 2))),
        ("oversized", Box::new(|f| (f.raw_width, f.raw_height) = (1 << 20, 1 << 20))),
        ("area past the data", Box::new(|f| f.width = 40)),
        ("area overflows", Box::new(|f| f.top_margin = usize::MAX)),
        ("empty area", Box::new(|f| f.height = 0)),
        ("SuperCCD", Box::new(|f| f.fuji_width = 1000)),
        ("not a mosaic", Box::new(|f| f.is_mosaic = false)),
        ("four colours", Box::new(|f| f.colors = 4)),
        ("unknown colour", Box::new(|f| f.pattern[5][7] = 255)),
        ("no blue", Box::new(|f| f.pattern = [[1; 48]; 48])),
        ("irregular pattern", Box::new(|f| f.pattern[40][40] = 2)),
        ("black pattern too big", Box::new(|f| (f.cblack[4], f.cblack[5]) = (17, 1))),
        ("patterns together too big", Box::new(|f| (f.cblack[4], f.cblack[5]) = (9, 1))),
        ("black past 16 bits", Box::new(|f| f.black = 70_000)),
        ("white below black", Box::new(|f| f.maximum = 400)),
        ("white zero", Box::new(|f| f.maximum = 0)),
        ("white past 16 bits", Box::new(|f| f.maximum = 1 << 20)),
        ("short cblack", Box::new(|f| f.cblack.clear())),
    ];
    for (what, spoil) in cases {
        let mut f = facts(RGGB);
        spoil(&mut f);
        let n = f.raw_width.saturating_mul(f.raw_height).min(1 << 20);
        let r = f.unpacked(vec![0; n], "LibRaw".into());
        if what == "short cblack" {
            assert!(r.is_ok(), "{what}: a missing cblack reads as zero");
        } else {
            assert!(r.is_err(), "{what}");
        }
    }
    let f = facts(RGGB);
    assert!(f.unpacked(vec![0; 10], "LibRaw".into()).is_err(), "sample count");
}

#[test]
fn without_libraw_the_stub_says_so() {
    if !crate::available() {
        assert!(crate::decode(b"anything").is_err());
        assert!(crate::version().is_none());
    } else {
        assert!(crate::decode(b"").is_err());
        assert!(crate::decode(b"not a raw file at all").is_err());
        assert!(crate::version().is_some_and(|v| v.starts_with("LibRaw")));
    }
}

/// A change that makes a description hostile.
type Spoil = Box<dyn Fn(&mut Facts)>;
