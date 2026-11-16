//! The eyedropper pointer of the colour pickers (the white balance selector, Point Color and the
//! Color / Luminance Range pickers). egui has no eyedropper cursor, so over the photo the system
//! pointer is hidden and this one is painted where it is, its tip on the pixel that is picked.
//! Drawn in code: original work (see `assets/ATTRIBUTION.md`).

use egui::{Color32, Id, LayerId, Order, Pos2, Rect, Shape, Stroke, pos2, vec2};

/// How far the eyedropper reaches from its tip, up and to the right (points).
pub const REACH: f32 = 22.0;

/// Show the eyedropper at `tip` (the hot spot) instead of the system pointer, above everything.
pub fn show(ctx: &egui::Context, tip: Pos2) {
    if !tip.x.is_finite() || !tip.y.is_finite() {
        return;
    }
    ctx.set_cursor_icon(egui::CursorIcon::None);
    let painter = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("eyedropper-pointer")));
    paint(&painter, tip);
    // (where it is, for tests and screen readers' hit maps: the tip is the box's bottom-left corner)
    crate::widgets::register(ctx, "cursor:eyedropper", Rect::from_min_max(pos2(tip.x, tip.y - REACH), pos2(tip.x + REACH, tip.y)));
}

/// Paint an eyedropper whose tip is at `tip`: a thin glass tube rising at 45° to the upper right, a
/// collar, then the rubber bulb. White with a dark edge, so it reads on light and dark photos.
pub fn paint(p: &egui::Painter, tip: Pos2) {
    // along the tool (towards the bulb) and across it
    let u = vec2(1.0, -1.0) / std::f32::consts::SQRT_2;
    let v = vec2(u.y, -u.x);
    let at = |along: f32, across: f32| tip + u * along + v * across;
    // a band of the tool from `a` to `b` along it, `wa` / `wb` wide (half widths)
    let band = |a: f32, wa: f32, b: f32, wb: f32| vec![at(a, -wa), at(b, -wb), at(b, wb), at(a, wa)];
    let edge = Stroke::new(1.0, Color32::from_black_alpha(220));
    let halo = Stroke::new(3.0, Color32::from_black_alpha(90));
    // the tube: a fine point widening to the collar
    let tube = band(0.6, 0.8, 13.0, 2.2);
    // the collar and the bulb
    let collar = band(13.0, 3.6, 15.5, 3.6);
    let bulb = band(15.5, 2.8, 22.0, 2.8);
    for (shape, fill) in [(&tube, Color32::from_gray(245)), (&bulb, Color32::from_gray(200)), (&collar, Color32::WHITE)] {
        p.add(Shape::closed_line(shape.clone(), halo));
        p.add(Shape::convex_polygon(shape.clone(), fill, edge));
    }
    // the bulb's rounded end
    p.circle(at(22.0, 0.0), 2.8, Color32::from_gray(200), edge);
    // the liquid in the tube's tip, and the point itself
    p.add(Shape::convex_polygon(band(0.6, 0.8, 6.0, 1.4), Color32::from_gray(90), Stroke::NONE));
    p.circle_filled(tip, 0.9, Color32::BLACK);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tip is the hot spot: everything painted lies up and to the right of it, within reach.
    #[test]
    fn the_eyedropper_is_drawn_up_and_right_of_its_tip() {
        let ctx = egui::Context::default();
        let tip = pos2(100.0, 200.0);
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let p = ui.ctx().layer_painter(LayerId::new(Order::Tooltip, Id::new("t")));
            paint(&p, tip);
        });
        out.textures_delta.clear();
        let mut bounds = Rect::NOTHING;
        for s in &out.shapes {
            bounds = bounds.union(s.shape.visual_bounding_rect());
        }
        assert!(bounds.is_positive(), "something is drawn");
        assert!(bounds.left() >= tip.x - 3.5 && bounds.bottom() <= tip.y + 3.5, "nothing below or left of the tip: {bounds:?}");
        assert!(bounds.right() <= tip.x + REACH + 4.0 && bounds.top() >= tip.y - REACH - 4.0, "within reach: {bounds:?}");
        assert!(bounds.width() > 12.0 && bounds.height() > 12.0, "big enough to see: {bounds:?}");
    }

    /// A pointer position that is not a number draws nothing and keeps the system pointer.
    #[test]
    fn a_bad_position_draws_nothing() {
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show(ui.ctx(), pos2(f32::NAN, 3.0)));
        out.textures_delta.clear();
        assert!(out.shapes.is_empty());
        assert_eq!(out.platform_output.cursor_icon, egui::CursorIcon::Default);
    }
}
