//! A made-up camera photographs a ColorChecker (procedurally: no photograph involved); the chart is
//! found, a profile is built and written as a DCP, and it reads back and reproduces the chart.

use lcc_camprof::{BuildOptions, ChartCorners, RawFrame, Vec3, build, find_chart, orient, refine, synthetic};

fn frame(turns: u8) -> (RawFrame, ChartCorners) {
    let (w, h) = (900usize, 600usize);
    let corners = if turns.is_multiple_of(2) {
        ChartCorners::from_rect(220.0, 150.0, 420.0, 280.0)
    } else {
        ChartCorners::from_rect(300.0, 110.0, 280.0, 420.0)
    };
    let d55 = lcc_camprof::illuminants::white_point_by_name("1931_2", "D55").unwrap();
    let data = synthetic::chart_frame(w, h, &corners, turns, d55, 0.55);
    (RawFrame::from_rgb(w, h, data).unwrap(), corners)
}

fn close(a: &ChartCorners, b: &ChartCorners, tol: f64) -> bool {
    a.points().iter().zip(b.points()).all(|(p, q)| (p[0] - q[0]).abs() < tol && (p[1] - q[1]).abs() < tol)
}

#[test]
fn the_chart_is_found_profiled_and_the_dcp_reads_back() {
    for turns in [0u8, 1] {
        let (frame, truth) = frame(turns);
        let found = find_chart(&frame).expect("the chart is found");
        assert_eq!(found.turns, turns);
        assert_eq!(orient(&frame, &truth), turns, "hand-placed corners are oriented by the fit");
        assert!(close(&found.corners, &truth, 12.0), "{:?} vs {truth:?}", found.corners);
        let opts = BuildOptions { camera_model: "the app Test Camera".into(), name: "Chart D55".into(), ..Default::default() };
        let built = build(&frame, &found, &opts).unwrap();
        assert_eq!(built.patches.len(), 24 - (built.patches_read - built.patches_used));
        assert!(built.mean_delta_e < 1.0, "mean ΔE {}", built.mean_delta_e);
        assert!(built.max_delta_e < 3.0, "max ΔE {}", built.max_delta_e);
        // the table does work the matrix alone cannot
        assert!(built.matrix_mean_delta_e > built.mean_delta_e, "{} vs {}", built.matrix_mean_delta_e, built.mean_delta_e);

        let bytes = lcc_raw::dcp::write(&built.dcp).unwrap();
        let back = lcc_raw::dcp::parse(&bytes).unwrap();
        assert_eq!(back.name, "Chart D55");
        assert_eq!(back.model, "the app Test Camera");
        assert_eq!(back.color.illuminant[0], 20, "D55 (EXIF 20)");
        assert!(back.color.forward_matrix[0].is_some());
        let map = back.color.profile.hue_sat_map[0].as_ref().expect("a hue/saturation map");
        assert_eq!((map.hue_divisions, map.sat_divisions, map.val_divisions), (90, 30, 1));
    }
}

#[test]
fn corners_placed_by_hand_are_moved_onto_the_chart() {
    let (frame, truth) = frame(0);
    let p = truth.points();
    let nudged = ChartCorners {
        top_left: [p[0][0] + 9.0, p[0][1] - 7.0],
        top_right: [p[1][0] - 6.0, p[1][1] + 5.0],
        bottom_right: [p[2][0] + 8.0, p[2][1] + 6.0],
        bottom_left: [p[3][0] - 5.0, p[3][1] - 8.0],
    };
    let refined = refine(&frame, &nudged, 0).expect("close enough to refine");
    // (a sampling square is 60 % of a cell and a patch 84 %: anywhere within ~8 px samples only the patch)
    assert!(close(&refined.corners, &truth, 10.0), "{:?} vs {truth:?}", refined.corners);
    let opts = BuildOptions { camera_model: "the app Test Camera".into(), ..Default::default() };
    let built = build(&frame, &refined, &opts).unwrap();
    assert!(built.mean_delta_e < 1.0, "mean ΔE {}", built.mean_delta_e);
    assert_eq!(built.dcp.name, "the app Test Camera · Daylight · Photo", "the default name is the camera, the light and the mode");
}

#[test]
fn bad_input_is_an_error_not_a_panic() {
    let (frame, truth) = frame(0);
    let located = lcc_camprof::Located { corners: truth, turns: 0, score: None };
    let mut opts = BuildOptions { camera_model: "X".into(), illuminant: "Moonlight".into(), ..Default::default() };
    assert!(build(&frame, &located, &opts).is_err());
    opts.illuminant = "D65".into();
    opts.camera_model = " ".into();
    assert!(build(&frame, &located, &opts).is_err());
    opts.camera_model = "X".into();
    // a grid off the picture, a collapsed one, NaN corners
    for corners in [
        ChartCorners::from_rect(5000.0, 5000.0, 100.0, 80.0),
        ChartCorners::from_rect(10.0, 10.0, 0.0, 0.0),
        ChartCorners::from_rect(f64::NAN, 3.0, 100.0, 80.0),
    ] {
        let l = lcc_camprof::Located { corners, turns: 7, score: None };
        assert!(build(&frame, &l, &opts).is_err(), "{corners:?}");
        let _ = refine(&frame, &corners, 3);
    }
    // a frame with no chart: nothing is found
    let blank = RawFrame::from_rgb(300, 200, vec![[0.2, 0.2, 0.2]; 60_000]).unwrap();
    assert!(find_chart(&blank).is_none());
    // one pixel frames
    let tiny = RawFrame::from_rgb(1, 1, vec![[0.5; 3]]).unwrap();
    assert!(find_chart(&tiny).is_none());
    let _ = Vec3::ONE;
}

fn frame_under(light: &str) -> (RawFrame, ChartCorners) {
    let (w, h) = (900usize, 600usize);
    let corners = ChartCorners::from_rect(220.0, 150.0, 420.0, 280.0);
    let white = lcc_camprof::illuminants::white_point_by_name("1931_2", light).unwrap();
    let data = synthetic::chart_frame(w, h, &corners, 0, white, 0.55);
    (RawFrame::from_rgb(w, h, data).unwrap(), corners)
}

#[test]
fn two_lights_make_a_dual_illuminant_profile() {
    use lcc_camprof::{Located, Shot, build_dual};
    let (day, dc) = frame_under("D65");
    let (tung, tc) = frame_under("A");
    let opts = BuildOptions { camera_model: "the app Test Camera".into(), ..Default::default() };
    // given daylight first: calibration 1 is still the lower colour temperature (DNG order)
    let dual = build_dual(
        &Shot { frame: &day, located: Located { corners: dc, turns: 0, score: None }, illuminant: "D65" },
        &Shot { frame: &tung, located: Located { corners: tc, turns: 0, score: None }, illuminant: "A" },
        &opts,
    )
    .unwrap();
    assert_eq!(dual.dcp.name, "the app Test Camera · Tungsten + D65 · Photo");
    assert_eq!(dual.calibrations[0].illuminant, "A");
    assert!(dual.calibrations.iter().all(|c| c.mean_delta_e < 1.0), "{} / {}", dual.calibrations[0].mean_delta_e, dual.calibrations[1].mean_delta_e);
    let back = lcc_raw::dcp::parse(&lcc_raw::dcp::write(&dual.dcp).unwrap()).unwrap();
    assert_eq!(back.color.illuminant, [17, 21], "StdA, then D65");
    let (cm1, cm2) = (back.color.color_matrix[0].unwrap(), back.color.color_matrix[1].unwrap());
    let (fm1, fm2) = (back.color.forward_matrix[0].unwrap(), back.color.forward_matrix[1].unwrap());
    let differ = |a: &lcc_raw::Mat3, b: &lcc_raw::Mat3| a.0.iter().flatten().zip(b.0.iter().flatten()).any(|(x, y)| (x - y).abs() > 1e-3);
    assert!(differ(&cm1, &cm2) && differ(&fm1, &fm2), "each light has its own calibration");
    let (m1, m2) = (back.color.profile.hue_sat_map[0].as_ref().unwrap(), back.color.profile.hue_sat_map[1].as_ref().unwrap());
    assert_eq!(
        (m1.hue_divisions, m1.sat_divisions, m1.val_divisions, m1.srgb_value),
        (m2.hue_divisions, m2.sat_divisions, m2.val_divisions, m2.srgb_value)
    );
    assert_ne!(m1.data, m2.data);
}

#[test]
fn the_same_light_twice_is_refused() {
    use lcc_camprof::{Located, Shot, build_dual};
    let (a, ac) = frame_under("D65");
    let (b, bc) = frame_under("D65");
    let opts = BuildOptions { camera_model: "X".into(), ..Default::default() };
    let shot = |f, c, l| Shot { frame: f, located: Located { corners: c, turns: 0, score: None }, illuminant: l };
    // declared alike
    let e = build_dual(&shot(&a, ac, "D65"), &shot(&b, bc, "D75"), &opts).unwrap_err();
    assert!(e.to_string().contains("same light"), "{e}");
    // declared apart, but the camera saw one light
    let e = build_dual(&shot(&a, ac, "D65"), &shot(&b, bc, "A"), &opts).unwrap_err();
    assert!(e.to_string().contains("same light"), "{e}");
    assert_eq!(lcc_camprof::nearest_illuminant(2900.0), "A");
    assert_eq!(lcc_camprof::nearest_illuminant(5600.0), "D55");
    assert_eq!(lcc_camprof::nearest_illuminant(f64::NAN), "D55");
}
