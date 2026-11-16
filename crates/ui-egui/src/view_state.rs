//! Library ▸ View's three view-only switches: Lock Zoom Position (`view.lockZoomPosition`), Scroll By
//! Row (`view.scrollByRow`) and Enable Mirror Image Mode (`view.mirrorImage`). None of them touches a
//! photo, its edit or an export.
//!
//! - **Lock Zoom Position** (on, as the loupe always behaved): one zoom and pan for every photo.
//!   Off: each photo keeps its own, remembered for the last [`MEMORY_LIMIT`] photos looked at and
//!   restored when it is active again; a photo not seen yet opens at Fit.
//! - **Scroll By Row**: a wheel notch moves the grid by whole rows ([`row_scroll`]).
//! - **Mirror Image Mode**: the centre view (grid, loupe, compare, survey) and the filmstrip are
//!   painted flipped left to right. [`around`] mirrors the shapes a pane painted about its centre;
//!   [`mirror_input`] mirrors the pointer the same way, so what is under the cursor on screen is what
//!   the pane (and its tools) act on, and a drag moves the way the hand moves.

use std::collections::VecDeque;

use egui::{Event, Pos2, Rect, Shape};
use lcc_catalog::PhotoId;

use crate::LccApp;
use crate::state::Zoom;

/// Photos whose zoom and pan are remembered while Lock Zoom Position is off.
pub const MEMORY_LIMIT: usize = 64;

/// Per-photo zoom and pan memory, and the photo it was last tracking.
#[derive(Debug, Default)]
pub struct ViewState {
    last: Option<PhotoId>,
    memory: VecDeque<(PhotoId, Zoom, (f32, f32))>,
    /// The rects painted mirrored since the last input, those the pointer is mirrored by, and
    /// whether a press that began in one of them is still held.
    painted: Vec<Rect>,
    active: Vec<Rect>,
    grabbed: bool,
}

impl ViewState {
    /// Remember `photo`'s view, newest last, dropping the oldest past [`MEMORY_LIMIT`].
    fn remember(&mut self, photo: PhotoId, zoom: Zoom, pan: (f32, f32)) {
        self.memory.retain(|(p, ..)| *p != photo);
        self.memory.push_back((photo, zoom, pan));
        while self.memory.len() > MEMORY_LIMIT {
            self.memory.pop_front();
        }
    }

    fn recall(&self, photo: PhotoId) -> Option<(Zoom, (f32, f32))> {
        self.memory.iter().find(|(p, ..)| *p == photo).map(|(_, z, pan)| (*z, *pan))
    }

    /// Number of photos remembered.
    pub fn remembered(&self) -> usize {
        self.memory.len()
    }
}

/// Once per frame: when the active photo changed and zoom positions are unlocked, store the one
/// left and restore the one arrived at.
pub fn track(app: &mut LccApp) {
    let now = app.session.active();
    if now == app.view_state.last {
        return;
    }
    let before = app.view_state.last;
    app.view_state.last = now;
    if app.ui.settings.lock_zoom {
        return;
    }
    if let Some(p) = before {
        let (zoom, pan) = (app.ui.zoom, app.ui.pan);
        app.view_state.remember(p, zoom, pan);
    }
    if let Some(p) = now {
        let (zoom, pan) = app.view_state.recall(p).unwrap_or((Zoom::Fit, (0.5, 0.5)));
        app.ui.zoom = zoom;
        app.ui.pan = pan;
    }
}

/// The scroll offset a wheel of `notches` (positive = down) from `top` lands on when the grid scrolls
/// by whole rows: the top of the row `notches` rows below (above) the row `top` is in. A `top` in
/// the middle of a row first snaps to that row's top when scrolling up. None for no rows or a
/// non-finite input.
pub fn row_scroll(rows: &[crate::panels::grid::GridRow], top: f32, notches: f32) -> Option<f32> {
    if rows.is_empty() || !top.is_finite() || !notches.is_finite() || notches == 0.0 {
        return None;
    }
    // the row `top` is inside: the last one starting at or above it (within half a point)
    let at = rows.partition_point(|r| r.top <= top + 0.5).saturating_sub(1);
    let at_top = rows.get(at).is_some_and(|r| (r.top - top).abs() <= 0.5);
    let n = notches.abs().ceil().min(1000.0) as usize;
    let target = if notches > 0.0 {
        at.saturating_add(n)
    } else if at_top {
        at.saturating_sub(n)
    } else {
        at.saturating_sub(n.saturating_sub(1))
    };
    rows.get(target.min(rows.len() - 1)).map(|r| r.top)
}

/// Run `body` (a pane painting into `ui` inside `rect`); with Mirror Image Mode on, flip what it
/// painted left to right about the centre of `rect` and remember the rect for [`mirror_input`].
pub fn around<R>(app: &mut LccApp, ui: &mut egui::Ui, rect: Rect, body: impl FnOnce(&mut LccApp, &mut egui::Ui) -> R) -> R {
    if !app.ui.settings.mirror_image || !rect.is_finite() {
        return body(app, ui);
    }
    let layer = ui.layer_id();
    let start = ui.ctx().graphics_mut(|g| g.entry(layer).next_idx());
    let out = body(app, ui);
    let end = ui.ctx().graphics_mut(|g| g.entry(layer).next_idx());
    let mid = rect.center().x;
    ui.ctx().graphics_mut(|g| {
        let list = g.entry(layer);
        let mut i = start.0;
        while i < end.0 {
            list.mutate_shape(egui::layers::ShapeIdx(i), |s| flip(&mut s.shape, mid));
            i += 1;
        }
    });
    app.view_state.painted.push(rect);
    out
}

/// Start of a frame: forget the rects mirrored two frames ago, mirror this frame's pointer events
/// by the ones the last frame painted.
pub fn mirror_input(app: &mut LccApp, raw: &mut egui::RawInput) {
    if !app.ui.settings.mirror_image {
        app.view_state.grabbed = false;
        app.view_state.painted.clear();
        app.view_state.active.clear();
        return;
    }
    // (the panes painted since the last input: a pane that stops being mirrored stops flipping)
    app.view_state.active = std::mem::take(&mut app.view_state.painted);
    let rects = app.view_state.active.clone();
    let mut grabbed = app.view_state.grabbed;
    for event in &mut raw.events {
        match event {
            Event::PointerMoved(pos) => *pos = flip_pos(&rects, *pos, grabbed),
            Event::PointerButton { pos, pressed, .. } => {
                let inside = rects.iter().any(|r| r.contains(*pos));
                *pos = flip_pos(&rects, *pos, grabbed);
                grabbed = *pressed && inside;
            }
            Event::Touch { pos, .. } => *pos = flip_pos(&rects, *pos, grabbed),
            _ => {}
        }
    }
    app.view_state.grabbed = grabbed;
}

/// Where the pointer is for a pane mirrored in `rects`: mirrored about the centre of the rect it is
/// in (or of the nearest one while a press that began inside is held).
fn flip_pos(rects: &[Rect], pos: Pos2, grabbed: bool) -> Pos2 {
    let hit = rects
        .iter()
        .find(|r| r.contains(pos))
        .or_else(|| if grabbed { rects.iter().min_by(|a, b| a.distance_sq_to_pos(pos).total_cmp(&b.distance_sq_to_pos(pos))) } else { None });
    match hit {
        Some(r) => Pos2::new(2.0 * r.center().x - pos.x, pos.y),
        None => pos,
    }
}

fn flip_x(x: f32, mid: f32) -> f32 {
    2.0 * mid - x
}

fn flip_pt(p: &mut Pos2, mid: f32) {
    p.x = flip_x(p.x, mid);
}

fn flip_rect(r: &mut Rect, mid: f32) {
    *r = Rect::from_min_max(Pos2::new(flip_x(r.max.x, mid), r.min.y), Pos2::new(flip_x(r.min.x, mid), r.max.y));
}

/// Mirror `shape` about the vertical line at `mid`. Text keeps its letters facing the right way and
/// only moves; pictures (meshes) flip.
fn flip(shape: &mut Shape, mid: f32) {
    match shape {
        Shape::Vec(v) => v.iter_mut().for_each(|s| flip(s, mid)),
        Shape::Circle(c) => flip_pt(&mut c.center, mid),
        Shape::Ellipse(e) => flip_pt(&mut e.center, mid),
        Shape::LineSegment { points, .. } => points.iter_mut().for_each(|p| flip_pt(p, mid)),
        Shape::Path(p) => p.points.iter_mut().for_each(|p| flip_pt(p, mid)),
        Shape::Rect(r) => flip_rect(&mut r.rect, mid),
        Shape::Text(t) => {
            let w = t.galley.size().x;
            t.pos.x = flip_x(t.pos.x, mid) - w;
        }
        Shape::Mesh(m) => {
            let m = std::sync::Arc::make_mut(m);
            m.vertices.iter_mut().for_each(|v| flip_pt(&mut v.pos, mid));
            // (mirrored triangles wind the other way; egui does not cull)
        }
        Shape::QuadraticBezier(b) => b.points.iter_mut().for_each(|p| flip_pt(p, mid)),
        Shape::CubicBezier(b) => b.points.iter_mut().for_each(|p| flip_pt(p, mid)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::grid::GridRow;

    fn rows() -> Vec<GridRow> {
        (0..5).map(|i| GridRow { top: i as f32 * 100.0, bottom: i as f32 * 100.0 + 90.0, start: i, end: i + 1 }).collect()
    }

    #[test]
    fn row_scroll_lands_on_whole_rows() {
        let r = rows();
        assert_eq!(row_scroll(&r, 0.0, 1.0), Some(100.0));
        assert_eq!(row_scroll(&r, 130.0, 1.0), Some(200.0));
        assert_eq!(row_scroll(&r, 130.0, -1.0), Some(100.0), "mid-row, up: this row's top");
        assert_eq!(row_scroll(&r, 200.0, -1.0), Some(100.0));
        assert_eq!(row_scroll(&r, 400.0, 3.0), Some(400.0), "clamped to the last row");
        assert_eq!(row_scroll(&r, 0.0, -1.0), Some(0.0));
        assert_eq!(row_scroll(&[], 0.0, 1.0), None);
        assert_eq!(row_scroll(&r, f32::NAN, 1.0), None);
        assert_eq!(row_scroll(&r, 0.0, f32::INFINITY), None);
    }

    #[test]
    fn flipping_twice_is_the_identity() {
        let mut s = Shape::rect_filled(Rect::from_min_max(Pos2::new(10.0, 5.0), Pos2::new(40.0, 25.0)), 0.0, egui::Color32::RED);
        let before = s.clone();
        flip(&mut s, 100.0);
        assert_ne!(s, before);
        flip(&mut s, 100.0);
        assert_eq!(s, before);
    }

    #[test]
    fn pointer_flips_inside_a_mirrored_rect_only() {
        let rects = [Rect::from_min_max(Pos2::new(100.0, 0.0), Pos2::new(300.0, 200.0))];
        assert_eq!(flip_pos(&rects, Pos2::new(120.0, 50.0), false), Pos2::new(280.0, 50.0));
        assert_eq!(flip_pos(&rects, Pos2::new(20.0, 50.0), false), Pos2::new(20.0, 50.0));
        assert_ne!(flip_pos(&rects, Pos2::new(20.0, 50.0), true), Pos2::new(20.0, 50.0), "a held drag keeps flipping");
        assert_eq!(flip_pos(&[], Pos2::new(20.0, 50.0), true), Pos2::new(20.0, 50.0));
    }

    #[test]
    fn zoom_memory_is_bounded_and_recalls_the_latest() {
        let mut s = ViewState::default();
        for i in 0..(MEMORY_LIMIT as u64 + 10) {
            s.remember(PhotoId(i), Zoom::Percent(100.0), (0.1, 0.2));
        }
        assert_eq!(s.remembered(), MEMORY_LIMIT);
        assert!(s.recall(PhotoId(0)).is_none());
        s.remember(PhotoId(70), Zoom::Fill, (0.9, 0.9));
        assert_eq!(s.recall(PhotoId(70)), Some((Zoom::Fill, (0.9, 0.9))));
    }
}
