//! Unit tests of the on-canvas gradient tools: handles, the radial feather ring, the cursors.

use egui::{Rect, pos2, vec2};
use lcc_develop::{DevelopSettings, MaskShape};
use lcc_geom::Point;
use lcc_pipeline::geometry::Frame;

use super::*;

/// A 400 × 300 photo drawn at 400 × 300 points from (100, 50).
fn map() -> CanvasMap {
    let frame = Frame::new(400, 300, &DevelopSettings::default(), true);
    CanvasMap::new(&frame, Rect::from_min_size(pos2(100.0, 50.0), vec2(400.0, 300.0)))
}

fn close(a: Point, b: Point) -> bool {
    (a.x - b.x).abs() < 1e-3 && (a.y - b.y).abs() < 1e-3
}

#[test]
fn turning_a_linear_gradient_keeps_its_middle_and_length() {
    let m = map();
    let shape = MaskShape::Linear { start: Point::new(0.25, 0.5), end: Point::new(0.75, 0.5) };
    // from right of the middle to below it: a quarter turn clockwise on screen
    let (press, at) = (Point::new(0.75, 0.5), Point::new(0.5, 0.5 + 100.0 / 300.0));
    let MaskShape::Linear { start, end } = moved_shape(&shape, ROTATE, Point::new(at.x - press.x, at.y - press.y), at, &m, false) else { panic!() };
    let (a, b) = (m.screen(start), m.screen(end));
    assert!((a.lerp(b, 0.5) - pos2(300.0, 200.0)).length() < 0.1, "the middle stays");
    assert!((a.distance(b) - 200.0).abs() < 0.1, "the length stays");
    assert!((a.x - b.x).abs() < 0.1 && a.y < b.y, "now vertical, start above: {a:?} {b:?}");
}

#[test]
fn dragging_a_linear_line_widens_the_fade_without_turning_it() {
    let m = map();
    let shape = MaskShape::Linear { start: Point::new(0.25, 0.5), end: Point::new(0.75, 0.5) };
    // the start line dragged left and down: only the leftward part counts
    let (press, at) = (Point::new(0.25, 0.2), Point::new(0.1, 0.4));
    let MaskShape::Linear { start, end } = moved_shape(&shape, LINE_START, Point::new(at.x - press.x, at.y - press.y), at, &m, false) else {
        panic!()
    };
    assert!(close(start, Point::new(0.1, 0.5)), "{start:?}");
    assert!(close(end, Point::new(0.75, 0.5)), "the end stays");
    let MaskShape::Linear { end, .. } = moved_shape(&shape, LINE_END, Point::new(0.05, 0.3), Point::new(0.8, 0.8), &m, false) else { panic!() };
    assert!(close(end, Point::new(0.8, 0.5)), "{end:?}");
}

#[test]
fn which_part_of_a_gradient_a_press_grabs() {
    let m = map();
    let linear = MaskShape::Linear { start: Point::new(0.25, 0.5), end: Point::new(0.75, 0.5) };
    // the lines at x = 200 (start), 300 (middle), 400 (end) on screen
    assert_eq!(gradient_part(&linear, pos2(300.0, 120.0), &m), Some(ROTATE));
    assert_eq!(gradient_part(&linear, pos2(202.0, 300.0), &m), Some(LINE_START));
    assert_eq!(gradient_part(&linear, pos2(398.0, 60.0), &m), Some(LINE_END));
    assert_eq!(gradient_part(&linear, pos2(250.0, 200.0), &m), Some(0), "between the lines: moves");
    let radial = MaskShape::Radial { center: Point::new(0.5, 0.5), rx: 0.25, ry: 0.25, angle: 0.0, feather: 50.0, invert: false };
    // radius 100 points round (300, 200)
    assert_eq!(gradient_part(&radial, pos2(300.0, 150.0), &m), Some(0), "inside: moves");
    assert_eq!(gradient_part(&radial, pos2(408.0, 200.0), &m), Some(ROTATE), "just outside: turns");
    assert_eq!(gradient_part(&radial, pos2(480.0, 200.0), &m), None, "far outside: nothing");
}

#[test]
fn the_feather_ring_sets_the_feather_and_the_slider_moves_the_ring() {
    let m = map();
    let radial = MaskShape::Radial { center: Point::new(0.5, 0.5), rx: 0.25, ry: 0.25, angle: 0.0, feather: 50.0, invert: false };
    // the ring at half the radius; dragged out to 80 % of it: feather 20
    let at = Point::new(0.5 + 0.8 * 100.0 / 400.0, 0.5);
    let MaskShape::Radial { feather, rx, .. } = moved_shape(&radial, 6, Point::new(0.0, 0.0), at, &m, false) else { panic!() };
    assert!((feather - 20.0).abs() < 0.6 && rx == 0.25, "{feather}");
    // the ring is drawn where the feather says
    assert!((radial_inner(20.0) - 0.8).abs() < 1e-9);
    assert!((radial_inner(f64::NAN) - 0.5).abs() < 1e-9, "a hostile value draws a ring anyway");
    let q = radial_point(&m, Point::new(0.5, 0.5), 0.25, 0.25, 0.0, radial_inner(20.0), 0.0);
    assert!((q - pos2(380.0, 200.0)).length() < 0.1, "{q:?}");
}

#[test]
fn turning_a_radial_gradient_follows_the_pointer_round_its_centre() {
    let m = map();
    let radial = MaskShape::Radial { center: Point::new(0.5, 0.5), rx: 0.25, ry: 0.1, angle: 10.0, feather: 50.0, invert: false };
    // a quarter turn clockwise from anywhere round the centre: +90°, from where it was
    let (press, at) = (Point::new(0.75, 0.5), Point::new(0.5, 0.5 + 100.0 / 300.0));
    for handle in [5, ROTATE] {
        let MaskShape::Radial { angle, .. } = moved_shape(&radial, handle, Point::new(at.x - press.x, at.y - press.y), at, &m, false) else {
            panic!()
        };
        assert!((angle - 100.0).abs() < 0.01, "{handle}: {angle}");
    }
}

#[test]
fn resize_cursors_follow_the_direction() {
    use egui::CursorIcon::*;
    assert_eq!(resize_cursor(vec2(1.0, 0.0)), ResizeHorizontal);
    assert_eq!(resize_cursor(vec2(-1.0, 0.05)), ResizeHorizontal);
    assert_eq!(resize_cursor(vec2(0.0, 1.0)), ResizeVertical);
    assert_eq!(resize_cursor(vec2(1.0, 1.0)), ResizeNwSe);
    assert_eq!(resize_cursor(vec2(1.0, -1.0)), ResizeNeSw);
    let _ = resize_cursor(vec2(f32::NAN, 0.0));
}
