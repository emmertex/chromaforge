use super::*;

fn shot(k: u64, minute: i64, exposure: f64, iso: u32) -> Shot {
    let t = lcc_catalog::dates::iso_seconds("2026-09-27T15:00:00").unwrap_or(0) + minute * 60;
    Shot { fingerprint: k, captured: Some(lcc_catalog::dates::civil(t)), exposure: Some(exposure), iso: Some(iso), f_number: Some(8.0) }
}

fn hot(x: u32, y: u32) -> Defect {
    Defect { x, y, s: 0, kind: DefectKind::Hot, level: 0.03 }
}

#[test]
fn body_keys_follow_the_camera_profile_rule() {
    assert_eq!(body_key(Some(" 653499300171 "), Some("_71_1377.CR3")).as_deref(), Some("653499300171"));
    assert_eq!(body_key(None, Some("_SA94454.ARW")).as_deref(), Some("file:SA"));
    assert_eq!(body_key(Some(""), Some("__71_4940.CR3")).as_deref(), Some("file:71"));
    assert_eq!(body_key(None, Some("_S.ARW")), None);
    assert_eq!(body_key(None, Some("_-x123.ARW")), None);
    assert_eq!(body_key(None, None), None);
    assert_eq!(body_label("file:SA"), "SA");
    assert_eq!(body_label("653499300171"), "…0171");
    assert_eq!(file_name("file:71"), "file_71.json");
    assert_eq!(file_name("../../etc"), "______etc.json");
}

#[test]
fn a_pixel_is_confirmed_in_photos_of_different_scenes_only() {
    let mut m = BodyMap::new("file:71", "Canon", "Canon EOS R5 C", [8352, 5586]);
    // a burst of one scene (same minute) can't confirm it, however many frames
    for k in 0..5 {
        assert!(m.record_pixels(&shot(k, 0, 0.01, 100), &[hot(100, 200)]));
    }
    assert!(!m.pixels[0].confirmed && m.pixels[0].hits == 5 && m.pixels[0].buckets == 1);
    // photos of two scenes in any order (parallel workers): two periods, however they alternate
    let mut m2 = BodyMap::new("order", "", "M", [100, 100]);
    for (k, minute) in [0i64, 20, 0, 20, 0, 20].into_iter().enumerate() {
        m2.record_pixels(&shot(100 + k as u64, minute, 0.01, 100), &[hot(1, 1)]);
    }
    assert_eq!((m2.pixels[0].hits, m2.pixels[0].buckets, m2.pixels[0].confirmed), (6, 2, false));
    // the same frame decoded again is not counted again
    assert!(!m.record_pixels(&shot(4, 0, 0.01, 100), &[hot(100, 200)]));
    assert_eq!(m.pixels[0].hits, 5);
    // two more scenes, 20 and 40 minutes later
    m.record_pixels(&shot(10, 20, 0.01, 100), &[hot(100, 200)]);
    assert!(!m.pixels[0].confirmed);
    m.record_pixels(&shot(11, 40, 0.02, 400), &[hot(100, 200), hot(5, 5)]);
    assert!(m.pixels[0].confirmed);
    assert_eq!(m.fixes(), vec![Fix { x: 100, y: 200, s: 0 }]);
    let p = &m.pixels[0];
    assert_eq!((p.exp_min, p.exp_max, p.iso_min, p.iso_max), (Some(0.01), Some(0.02), Some(100), Some(400)));
    assert!(p.first < p.last);
}

#[test]
fn maps_grow_and_entries_expire() {
    let mut m = BodyMap::new("b", "", "M", [100, 100]);
    let mut k = 0u64;
    let mut take = |m: &mut BodyMap, minute: i64, exposure: f64, found: &[Defect]| {
        k += 1;
        m.record_pixels(&shot(k, minute, exposure, 100), found)
    };
    for i in 0..3 {
        take(&mut m, i * 30, 0.01, &[hot(10, 10)]);
    }
    assert!(m.pixels[0].confirmed);
    // a new hot pixel develops later: the map grows
    for i in 3..6 {
        take(&mut m, i * 30, 0.01, &[hot(10, 10), hot(50, 60)]);
    }
    assert_eq!(m.fixes().len(), 2);
    // a one-off candidate (a star the guard let through) is dropped after SINGLETON_TTL photos without it
    take(&mut m, 400, 0.01, &[hot(70, 70)]);
    assert!(m.pixels.iter().any(|p| p.x == 70));
    for i in 0..=SINGLETON_TTL as i64 {
        take(&mut m, 500 + i * 15, 2.0, &[hot(10, 10), hot(50, 60)]);
    }
    assert!(!m.pixels.iter().any(|p| p.x == 70));
    // photos at exposures where (10, 10) never showed don't count against it…
    assert_eq!(m.pixels.iter().find(|p| p.x == 50).map(|p| p.misses), Some(0));
    for i in 0..400 {
        take(&mut m, 20_000 + i * 15, 30.0, &[hot(50, 60)]);
    }
    assert!(m.pixels.iter().any(|p| p.x == 10 && p.confirmed), "never tested in its range (exposures 0.01–2 s)");
    // …photos in its range without it do: after EXPIRE_MISSES (and 20 × hits) it expires
    let hits = m.pixels.iter().find(|p| p.x == 10).map_or(0, |p| p.hits);
    for i in 0..EXPIRE_MISSES.max(hits * 20) as i64 {
        take(&mut m, 40_000 + i * 15, 0.5, &[hot(50, 60)]);
    }
    assert!(!m.pixels.iter().any(|p| p.x == 10), "{:?}", m.pixels.iter().find(|p| p.x == 10));
    assert!(m.pixels.iter().any(|p| p.x == 50 && p.confirmed));
}

#[test]
fn dark_frames_confirm_at_once_and_never_expire_by_misses() {
    let mut m = BodyMap::new("b", "", "M", [100, 100]);
    let df = lcc_raw::darkframe::DarkFrame {
        version: 1,
        make: None,
        model: Some("M".into()),
        serial: None,
        sensor: [100, 100],
        active: [0, 0, 100, 100],
        cfa: Some("RGGB".into()),
        iso: Some(800),
        exposure: Some(30.0),
        captured: None,
        range: 15_000.0,
        offset: [0.0; 3],
        noise: [0.001; 3],
        row_noise: 0.0,
        column_noise: 0.0,
        hot: vec![hot(3, 4), hot(7, 8)],
    };
    assert_eq!(m.record_dark_frame(&df), 2);
    assert_eq!(m.fixes().len(), 2);
    for k in 0..400 {
        m.record_pixels(&shot(k, k as i64 * 30, 30.0, 800), &[]);
    }
    assert_eq!(m.fixes().len(), 2);
}

#[test]
fn the_map_file_round_trips_and_damaged_files_are_set_aside() {
    let mut m = BodyMap::new("roundtrip", "Sony", "ILCE-9", [6048, 4024]);
    m.record_pixels(&shot(1, 0, 0.01, 100), &[hot(1, 2)]);
    let j = serde_json::to_string(&m).unwrap();
    let back: BodyMap = serde_json::from_str(&j).unwrap();
    assert_eq!(back, m);
    assert!(back.valid());
    // hostile values are refused
    let mut bad = m.clone();
    bad.pixels[0].x = 999_999;
    assert!(!bad.valid());
    let mut bad = m.clone();
    bad.dust.push(DustEntry { x: 7.0, y: 0.5, windows: vec![], apertures: vec![], confirmed: true });
    assert!(!bad.valid());
    // a damaged file: renamed .bad, a new map starts, nothing panics
    let d = dir().unwrap();
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join(file_name("damaged-body")), b"{not json").unwrap();
    assert!(get("damaged-body").is_none());
    assert!(d.join("damaged-body.json.bad").exists());
}

#[test]
fn records_through_the_registry_change_the_generation_on_confirmation_only() {
    let body = "registry-body";
    let g0 = generation();
    for k in 0..2 {
        assert_eq!(record_pixels(body, "Canon", "R5", [100, 80], &shot(100 + k, k as i64 * 20, 0.01, 100), &[hot(9, 9)]), Some(false));
    }
    assert!(fixes_for(body, "R5", [100, 80]).is_empty());
    assert_eq!(record_pixels(body, "Canon", "R5", [100, 80], &shot(102, 40, 0.01, 100), &[hot(9, 9)]), Some(true));
    assert_ne!(generation(), g0);
    assert_eq!(fixes_for(body, "R5", [100, 80]).as_slice(), &[Fix { x: 9, y: 9, s: 0 }]);
    // another model or sensor size under the same key is not touched or corrected
    assert!(fixes_for(body, "R6", [100, 80]).is_empty());
    assert_eq!(record_pixels(body, "Canon", "R5", [90, 80], &shot(103, 60, 0.01, 100), &[hot(9, 9)]), None);
    // it was saved: a file exists and reads back
    assert!(dir().unwrap().join(file_name(body)).exists());
    assert!(bodies().contains(&body.to_string()));
    clear(body, true, true).unwrap();
    assert!(fixes_for(body, "R5", [100, 80]).is_empty());
}

fn seen(x: f64, y: f64, radius: f64, strength: f32) -> DustSeen {
    DustSeen { x, y, radius, strength, dip: strength * 1.5 }
}

#[test]
fn dust_is_confirmed_across_photos_and_remembered_per_aperture() {
    let mut m = BodyMap::new("dusty", "", "M", [100, 100]);
    m.aspect = 1.5;
    let none = |_: f64, _: f64, _: f64| None;
    let mut s16 = shot(1, 0, 0.01, 100);
    s16.f_number = Some(16.0);
    assert!(m.record_dust(&s16, &[seen(0.30, 0.40, 0.003, 0.08)], none));
    assert!(!m.record_dust(&s16, &[seen(0.30, 0.40, 0.003, 0.08)], none), "the same photo again");
    let mut s8 = shot(2, 30, 0.01, 100);
    s8.f_number = Some(8.0);
    // found a little off (detection jitter): the same spot
    m.record_dust(&s8, &[seen(0.302, 0.401, 0.005, 0.05)], none);
    assert_eq!(m.dust.len(), 1);
    assert!(!m.dust[0].confirmed);
    let mut s11 = shot(3, 60, 0.01, 100);
    s11.f_number = Some(11.0);
    m.record_dust(&s11, &[seen(0.30, 0.40, 0.004, 0.06), seen(0.8, 0.2, 0.003, 0.05)], none);
    assert!(m.dust[0].confirmed && !m.dust[1].confirmed);
    assert_eq!(m.dust[0].apertures.len(), 3);
    // its look at f/16 is the f/16 one; at f/4 it is wider and fainter than at f/8
    let at16 = m.dust[0].look_at(Some(16.0)).unwrap();
    assert!((at16.0 - 0.003).abs() < 1e-9);
    let (a8, a4) = (m.dust[0].look_at(Some(8.0)).unwrap(), m.dust[0].look_at(Some(4.0)).unwrap());
    assert!(a4.0 > a8.0 && a4.1 < a8.1);
    // what a photo an hour later gets: the confirmed spot only (the other was seen once)
    let later = s11.time().map(|t| t + 3600);
    let spots = m.dust_for(later, Some(11.0));
    assert_eq!(spots.len(), 1);
    assert!(spots[0].4);
}

#[test]
fn dust_windows_close_when_the_spot_is_gone_and_reopen_when_it_is_back() {
    let mut m = BodyMap::new("windows", "", "M", [100, 100]);
    m.aspect = 1.5;
    let none = |_: f64, _: f64, _: f64| None;
    let mut k = 0;
    let mut at = |m: &mut BodyMap, minute: i64, found: &[DustSeen], gone: bool| {
        k += 1;
        let mut s = shot(k, minute, 0.01, 100);
        s.f_number = Some(11.0);
        if gone { m.record_dust(&s, found, |_, _, _| Some(DustCheck { dip: 0.0, testable: true })) } else { m.record_dust(&s, found, none) }
    };
    let spot = [seen(0.5, 0.5, 0.004, 0.06)];
    for i in 0..4 {
        at(&mut m, i * 20, &spot, false);
    }
    assert!(m.dust[0].confirmed);
    // a day later the sensor was cleaned: three smooth, bright photos at f/11 without it close the window
    for i in 0..3 {
        at(&mut m, 1440 + i * 20, &[], true);
    }
    assert!(m.dust[0].windows[0].closed);
    let t = |minute: i64| shot(0, minute, 0.01, 100).time();
    assert_eq!(m.dust_for(t(30), Some(11.0)).len(), 1, "inside the window");
    assert!(m.dust_for(t(2000), Some(11.0)).is_empty(), "after it closed");
    // photos where the place is textured or the aperture too wide don't close a window
    let mut m2 = m.clone();
    m2.dust[0].windows[0].closed = false;
    m2.dust[0].windows[0].misses = 0;
    let mut s = shot(999, 3000, 0.01, 100);
    s.f_number = Some(2.8);
    m2.record_dust(&s, &[], |_, _, _| Some(DustCheck { dip: 0.0, testable: true }));
    assert_eq!(m2.dust[0].windows[0].misses, 0);
    // it comes back a week later: a new window
    for i in 0..3 {
        at(&mut m, 10_080 + i * 30, &spot, false);
    }
    assert_eq!(m.dust[0].windows.len(), 2);
    assert_eq!(m.dust_for(t(10_200), Some(11.0)).len(), 1);
    assert!(m.dust_for(t(5000), Some(11.0)).is_empty(), "between the windows");
}
