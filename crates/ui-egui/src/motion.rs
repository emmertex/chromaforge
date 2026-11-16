//! The Reduce motion policy, in one place. With it on, things that move or turn stop moving (the
//! click-zoom glide, egui's widget easing, a spinner's turning arc); opacity fades that tell a
//! state change stay. The app sets the flag each frame from Settings ▸ Interface; drawing code
//! asks [`reduced`].

use egui::{Context, Id};

/// egui's widget easing time when motion is not reduced (what the theme sets).
const EASING_SECS: f32 = 0.08;

fn flag_id() -> Id {
    Id::new("reduce_motion")
}

/// Record this frame's setting and apply it to egui's own easing.
pub fn set(ctx: &Context, on: bool) {
    if ctx.data(|d| d.get_temp::<bool>(flag_id())) == Some(on) {
        return;
    }
    ctx.data_mut(|d| d.insert_temp(flag_id(), on));
    ctx.all_styles_mut(|s| s.animation_time = if on { 0.0 } else { EASING_SECS });
}

/// Whether motion is reduced (false until the app has said so).
pub fn reduced(ctx: &Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(flag_id())).unwrap_or(false)
}
