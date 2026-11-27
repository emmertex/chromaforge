//! Hostile input to the pixel-shift readers (the maker-note header reader and the series checks): truncated and damaged
//! notes, tags of the wrong type or count, looping and overlapping IFDs, indices that disagree, counters that wrap.
//! Every input gives a header or a clean refusal, never a panic, and takes bounded time. The seeded mutation tests use the
//! repo's `lcc-fuzzkit` (`LCC_FUZZ_ITERS` raises their length for a longer local run).

use lcc_raw::pixel_shift::{FrameHeader, HEADER_PREFIX_BYTES, SeriesError, check_subset, detect, read_header, read_header_with};
use lcc_tiff::{ByteOrder, IfdBuilder, TiffWriter, Value, tags as t, writer::rational};

/// A header-only synthetic NEF: Exif and a Nikon maker note with the pixel-shift numbers of frame `index` (all values
/// invented); `tweak` may change the maker note before it is written.
fn nef(order: ByteOrder, index: u8, count: u16, counter: u32, tweak: &dyn Fn(&mut IfdBuilder)) -> Vec<u8> {
    let mut note = IfdBuilder::new();
    // as the Z 8 writes them: 0x0034 says 16 in every mode, the mode is byte 4 of 0x005e (1, 2, 3, 4 = 4, 8, 16, 32 shots) and
    // its byte 5 is the frame's slot (the number, plus 12 for frames 5 to 8 of an 8-shot series)
    note.set(0x0034, Value::Short(vec![16]));
    let mut a = vec![0u8; 16];
    a[8] = index;
    note.set(0x0056, Value::Undefined(a));
    let mut b = vec![0u8; 28];
    b[4] = match count {
        4 => 1,
        8 => 2,
        16 => 3,
        32 => 4,
        _ => 0,
    };
    b[5] = if count == 8 && index > 4 { index.wrapping_add(12) } else { index };
    note.set(0x005e, Value::Undefined(b));
    note.set(0x00a7, Value::Long(vec![counter]));
    note.set(0x0045, Value::Short(vec![0, 0, 8256, 5504]));
    note.set(0x001b, Value::Short(vec![1, 2, 3, 4, 5, 6, 7]));
    note.set(0x0054, Value::Undefined(vec![0u8; 16]));
    tweak(&mut note);
    let mut maker = b"Nikon\0\x02\0\0\0".to_vec();
    maker.extend(TiffWriter::new(order, false).write(&[note]).unwrap());
    let mut exif = IfdBuilder::new();
    exif.set(t::MAKER_NOTE, Value::Undefined(maker));
    exif.set(t::EXPOSURE_TIME, Value::Rational(vec![rational(0.01)]));
    exif.set(t::DATE_TIME_ORIGINAL, Value::Ascii("2030:05:06 07:08:09".into()));
    exif.set(0x9291, Value::Ascii(format!("{:02}", u32::from(index) * 6)));
    let mut ifd0 = IfdBuilder::new();
    ifd0.set(t::MAKE, Value::Ascii("NIKON CORPORATION".into()));
    ifd0.set(t::MODEL, Value::Ascii("NIKON Z 8".into()));
    ifd0.set_child(t::EXIF_IFD, exif);
    TiffWriter::new(order, false).write(&[ifd0]).unwrap()
}

fn valid(index: u8) -> Vec<u8> {
    nef(ByteOrder::Little, index, 16, 5000 + u32::from(index), &|_| {})
}

fn header(index: u16, counter: u32, time_ms: i64) -> FrameHeader {
    FrameHeader {
        camera: "NIKON CORPORATION NIKON Z 8".into(),
        lens: String::new(),
        raw_size: (8280, 5520),
        image_area: Some([0, 0, 8256, 5504]),
        sensor_region: None,
        exposure_time: 0.01,
        f_number: 5.6,
        iso: 64,
        exposure_bias: 0.0,
        white_balance: None,
        shot_count: Some(16),
        index: Some(index),
        counter: Some(counter),
        time_ms: Some(time_ms),
        position: None,
    }
}

#[test]
fn a_valid_header_reads() {
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let h = read_header(&nef(order, 7, 16, 5007, &|_| {})).unwrap();
        assert_eq!((h.shot_count, h.index, h.counter), (Some(16), Some(7), Some(5007)));
    }
}

/// Every tag the reader looks at, given a value of the wrong type, the wrong size or no size at all: a header or an error.
#[test]
fn tags_of_the_wrong_type_or_count_give_a_header_or_an_error() {
    let values: Vec<(&str, Value)> = vec![
        ("ascii", Value::Ascii("not a number".into())),
        ("rational", Value::Rational(vec![rational(1.5)])),
        ("float", Value::Float(vec![f32::NAN, f32::INFINITY])),
        ("empty bytes", Value::Undefined(vec![])),
        ("one byte", Value::Undefined(vec![9])),
        ("short bytes", Value::Undefined(vec![1, 2, 3])),
        ("empty shorts", Value::Short(vec![])),
        ("many shorts", Value::Short(vec![0xffff; 50_000])),
        ("many longs", Value::Long(vec![u32::MAX; 50_000])),
        ("big bytes", Value::Undefined(vec![0xff; 300_000])),
        ("rational over zero", Value::Rational(vec![(1, 0), (0, 0)])),
        ("double nan", Value::Double(vec![f64::NAN, f64::NEG_INFINITY])),
        ("long8 at the end", Value::Long8(vec![u64::MAX])),
        ("ifd pointer at the end", Value::Ifd(vec![u32::MAX])),
    ];
    for tag in [0x0034u16, 0x0056, 0x005e, 0x00a7, 0x0045, 0x001b, 0x0054, 0x000c] {
        for (what, value) in &values {
            let bytes = nef(ByteOrder::Little, 3, 16, 5003, &|note| {
                note.set(tag, value.clone());
            });
            // (a header or a refusal; the point is that it returns)
            let _ = read_header(&bytes);
            let _ = read_header_with(&bytes[..bytes.len().min(HEADER_PREFIX_BYTES)], bytes.len() as u64, || Some(bytes.clone()));
            let _ = (tag, what);
        }
    }
}

#[test]
fn indices_that_disagree_give_no_index_and_no_series() {
    let bytes = nef(ByteOrder::Little, 3, 16, 5003, &|note| {
        let mut b = vec![0u8; 28];
        b[4] = 3;
        b[5] = 4;
        note.set(0x005e, Value::Undefined(b));
    });
    let h = read_header(&bytes).unwrap();
    assert_eq!(h.index, None, "the number says 3 and the slot 4");
    assert_eq!(detect(std::slice::from_ref(&h)), Err(SeriesError::NoIndex(1)));
    assert_eq!(check_subset(&[h], 16), Err(SeriesError::NoIndex(1)));
}

/// Every mode byte, slot and frame number a maker note can hold: the modes a series has give a shot count, a mode that
/// is none of them is refused by name, and a number or slot outside the mode's gives no frame number; never a panic.
#[test]
fn every_mode_slot_and_number_is_judged_without_a_panic() {
    use lcc_raw::pixel_shift::HeaderError;
    for mode in 0..=255u8 {
        for (number, slot) in [(0u8, 0u8), (1, 1), (4, 4), (5, 17), (8, 20), (16, 16), (17, 17), (32, 32), (33, 33), (255, 255), (3, 0), (0, 3)] {
            let bytes = nef(ByteOrder::Little, number, 16, 5000, &|note| {
                let mut b = vec![0u8; 28];
                b[4] = mode;
                b[5] = slot;
                note.set(0x005e, Value::Undefined(b));
            });
            match read_header(&bytes) {
                Ok(h) => {
                    let shots = match mode {
                        1 => 4,
                        2 => 8,
                        3 => 16,
                        4 => 32,
                        _ => panic!("mode {mode} gave a header"),
                    };
                    assert_eq!(h.shot_count, Some(shots));
                    if let Some(i) = h.index {
                        assert!((1..=shots).contains(&i) && i == u16::from(number), "mode {mode}, number {number}, slot {slot}: {i}");
                    }
                }
                Err(e) => assert!(matches!(e, HeaderError::NotPixelShift | HeaderError::UnknownMode(_)), "mode {mode}: {e:?}"),
            }
        }
    }
}

/// Shot counts a file can say, the largest included: a refusal, in bounded work.
#[test]
fn any_shot_count_is_judged_without_a_panic() {
    for count in [0u16, 1, 2, 3, 15, 17, 32, 255, 256, 65_535] {
        let one: Vec<FrameHeader> =
            (1..=16u16).map(|i| FrameHeader { shot_count: Some(count), ..header(i, 100 + u32::from(i), i64::from(i) * 120) }).collect();
        let _ = detect(&one);
        let _ = check_subset(&one, count);
        let _ = check_subset(&one[..3], count);
    }
}

/// Exposure counters that wrap around are still one burst; the numbers at the ends of their range are no overflow.
#[test]
fn counters_that_wrap_are_one_burst_and_the_extremes_do_not_overflow() {
    let wrapping: Vec<FrameHeader> = (1..=16u16).map(|i| header(i, (u32::MAX - 7).wrapping_add(u32::from(i)), i64::from(i) * 120)).collect();
    assert_eq!(detect(&wrapping).map(|s| s.count), Ok(16));
    assert_eq!(check_subset(&wrapping, 16), Ok(()));
    assert_eq!(check_subset(&wrapping[3..9], 16), Ok(()));
    // times at the ends of their range
    for time in [i64::MIN, i64::MIN / 2, -1, 0, i64::MAX / 2, i64::MAX] {
        let hs: Vec<FrameHeader> = (1..=16u16).map(|i| header(i, 100 + u32::from(i), if i % 2 == 0 { time } else { 0 })).collect();
        let _ = detect(&hs);
        let _ = check_subset(&hs, 16);
    }
    // a gap in the counters, equal counters, backwards counters
    for counters in [[1u32, 3], [5, 5], [9, 2]] {
        let hs = [header(1, counters[0], 0), header(2, counters[1], 120)];
        assert_eq!(check_subset(&hs, 16), Err(SeriesError::CounterGap), "{counters:?}");
    }
}

#[test]
fn a_subset_of_a_series_is_checked_against_the_frame_numbers() {
    let hs: Vec<FrameHeader> = [2u16, 5, 9].iter().map(|&i| header(i, 5000 + u32::from(i), i64::from(i) * 120)).collect();
    assert_eq!(check_subset(&hs, 16), Ok(()));
    assert_eq!(check_subset(&[], 16), Ok(()));
    // a frame twice, a frame past the shots, another camera, another exposure, times going backwards
    assert_eq!(check_subset(&[hs[0].clone(), hs[0].clone()], 16), Err(SeriesError::Duplicate(2)));
    assert!(check_subset(&hs, 4).is_err());
    let other_camera = FrameHeader { camera: "NIKON CORPORATION NIKON Z 9".into(), ..hs[1].clone() };
    assert_eq!(check_subset(&[hs[0].clone(), other_camera], 16), Err(SeriesError::MixedCamera));
    let other_exposure = FrameHeader { iso: 800, ..hs[1].clone() };
    assert_eq!(check_subset(&[hs[0].clone(), other_exposure], 16), Err(SeriesError::MixedExposure));
    let backwards = FrameHeader { time_ms: Some(0), ..hs[2].clone() };
    assert_eq!(check_subset(&[hs[0].clone(), backwards], 16), Err(SeriesError::TimeGap));
}

#[test]
fn looping_overlapping_and_cut_tiffs_end() {
    // IFD0 with no entries whose next-IFD pointer points back at itself
    let looping = [b'I', b'I', 42, 0, 8, 0, 0, 0, 0, 0, 8, 0, 0, 0];
    assert!(read_header(&looping).is_err());
    // an IFD that claims more entries than the file has, and one whose entries point outside it
    let claims = [b'I', b'I', 42, 0, 8, 0, 0, 0, 0xff, 0xff, 1, 2, 3];
    assert!(read_header(&claims).is_err());
    let mut outside = vec![b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0];
    // tag 0x8769 (Exif IFD) of type LONG, count 1, pointing at the end of the 32-bit range, no next IFD
    outside.extend_from_slice(&[0x69, 0x87, 4, 0, 1, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0]);
    assert!(read_header(&outside).is_err());
    // a valid file cut at every length: always a header or an error
    let bytes = valid(5);
    for n in (0..bytes.len()).step_by(7) {
        let _ = read_header(&bytes[..n]);
        let _ = read_header_with(&bytes[..n], bytes.len() as u64, || None);
        let _ = read_header_with(&bytes[..n], bytes.len() as u64, || Some(bytes.clone()));
    }
    // nothing, a few bytes, text
    for junk in [&b""[..], b"I", b"II*", b"II*\0", b"not a tiff at all"] {
        assert!(read_header(junk).is_err());
    }
}

#[test]
fn mutated_headers_never_panic() {
    let seeds = [valid(1), valid(9), nef(ByteOrder::Big, 16, 16, 5016, &|_| {})];
    let refs: Vec<&[u8]> = seeds.iter().map(Vec::as_slice).collect();
    lcc_fuzzkit::run("pixel_shift.header", &refs, 4000, |b| {
        if let Ok(h) = read_header(b) {
            let _ = detect(std::slice::from_ref(&h));
            let _ = check_subset(std::slice::from_ref(&h), 16);
        }
        let _ = read_header_with(b.get(..b.len().min(300)).unwrap_or_default(), b.len() as u64, || Some(b.to_vec()));
    });
}

/// Series of headers with random fields (counters, times, numbers and counts at the ends of their ranges): a series or a
/// refusal.
#[test]
fn random_header_sets_never_panic() {
    let mut rng = lcc_fuzzkit::Rng::new(0x5eed);
    for _ in 0..lcc_fuzzkit::iterations(4000) {
        let n = 1 + rng.below(20);
        let hs: Vec<FrameHeader> = (0..n)
            .map(|_| {
                let pick = |rng: &mut lcc_fuzzkit::Rng, small: u64| match rng.below(5) {
                    0 => u64::MAX,
                    1 => 0,
                    2 => u64::from(u32::MAX),
                    _ => rng.next_u64() % small,
                };
                let mut h = header(pick(&mut rng, 40) as u16, pick(&mut rng, 100) as u32, pick(&mut rng, 5000) as i64);
                h.shot_count = if rng.chance(8) { None } else { Some([4u16, 8, 16, 32, 0, u16::MAX][rng.below(6)]) };
                if rng.chance(10) {
                    h.index = None;
                }
                if rng.chance(10) {
                    h.counter = None;
                }
                if rng.chance(10) {
                    h.time_ms = None;
                }
                if rng.chance(10) {
                    h.camera = "OTHER".into();
                }
                h
            })
            .collect();
        let _ = detect(&hs);
        let _ = check_subset(&hs, [4u16, 8, 16, 32, 0, u16::MAX][rng.below(6)]);
    }
}
