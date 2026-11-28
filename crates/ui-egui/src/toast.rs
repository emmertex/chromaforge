//! The messages at the bottom centre of the window: what a message is (its kind), how long it stays (one policy)
//! and what it looks like (one look). The panels only say a kind and a text; they never pick a time or a colour.

use egui::{Color32, Stroke};
use lcc_surface::settings::OsdPosition;
use serde::{Deserialize, Serialize};

use crate::theme::Tokens;

/// What a message is, which decides how it looks and how long it stays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToastKind {
    /// Information: a result or a note ("Catalog saved").
    #[default]
    Message,
    /// A message with a button (Undo…).
    Action,
    /// A failure or a warning the user has to read.
    Error,
    /// A condition that is still true ("changes not written to disk"): it stays until the person dismisses it or
    /// the code that raised it clears it (see [`Toast::sticky`]).
    Sticky,
}

impl ToastKind {
    /// The id a script says (`ui.toast`'s `kind`, `ui.inspect`'s `toasts`).
    pub const fn id(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Action => "action",
            Self::Error => "error",
            Self::Sticky => "sticky",
        }
    }

    /// The seconds a short message of this kind stays, given the seconds an Information message stays: a message with
    /// a button 7 times that, a warning 4 times it but at least 6 s.
    fn base_seconds(self, info: f64) -> f64 {
        match self {
            Self::Message => info,
            Self::Error => (4.0 * info).max(6.0),
            Self::Action => 7.0 * info,
            Self::Sticky => f64::INFINITY,
        }
    }
}

/// Characters of a message read within its base time; each one past them adds [`READING_SECONDS_PER_CHAR`].
const READING_FREE_CHARS: usize = 50;
const READING_SECONDS_PER_CHAR: f64 = 0.08;
/// The longest any message stays.
const LONGEST_SECONDS: f64 = 30.0;

/// How long a message of `kind` with `text` stays when an Information message stays `info` seconds: the kind's time,
/// and more for a long text so it can be read. The one place a message's time is decided. A sticky message never ends.
pub fn stay_seconds(kind: ToastKind, text: &str, info: f64) -> f64 {
    if kind == ToastKind::Sticky {
        return f64::INFINITY;
    }
    let extra = text.chars().count().saturating_sub(READING_FREE_CHARS) as f64 * READING_SECONDS_PER_CHAR;
    (kind.base_seconds(info) + extra).min(LONGEST_SECONDS)
}

/// How messages look and where they sit (Settings ▸ Notifications ▸ Messages), saved in `ui.json`. The same five
/// controls as the overlay pill's (`lcc_surface::settings::OsdSettings`), which is saved with the control surfaces.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MessageSettings {
    /// 0.5–2 times the standard size.
    pub size: f64,
    /// 0.1–1.
    pub opacity: f64,
    /// `#RRGGBB`: the box of an information message or one with a button (a warning keeps its red).
    pub background: String,
    /// How long an Information message stays (ms, 500–10000); the others stay a multiple of it.
    pub display_ms: u32,
    pub position: OsdPosition,
}

impl Default for MessageSettings {
    fn default() -> Self {
        MessageSettings { size: 1.0, opacity: 0.78, background: "#000000".into(), display_ms: 1400, position: OsdPosition::Bottom }
    }
}

impl MessageSettings {
    /// The settings with every value inside its range (a hand-edited file).
    pub fn clamped(mut self) -> Self {
        let d = Self::default();
        let num = |v: f64, lo: f64, hi: f64, or: f64| if v.is_finite() { v.clamp(lo, hi) } else { or };
        self.size = num(self.size, 0.5, 2.0, d.size);
        self.opacity = num(self.opacity, 0.1, 1.0, d.opacity);
        self.display_ms = self.display_ms.clamp(500, 10_000);
        if lcc_surface::settings::rgb(&self.background).is_none() {
            self.background = d.background;
        }
        self
    }

    /// How long an Information message stays (s).
    pub fn info_seconds(&self) -> f64 {
        f64::from(self.display_ms.clamp(500, 10_000)) / 1000.0
    }

    /// The distance of the stack's first message from the canvas edge it grows from.
    pub fn edge_gap(&self) -> f32 {
        let s = self.size.clamp(0.5, 2.0) as f32;
        let gap = match self.position {
            OsdPosition::Bottom | OsdPosition::Top => 40.0,
            OsdPosition::BottomEdge | OsdPosition::TopEdge => 16.0,
        };
        gap * s.max(1.0)
    }

    /// Whether the stack grows down from the top (the position is Up or Up High, or a slideshow is on).
    pub fn from_top(&self, slideshow: bool) -> bool {
        slideshow || matches!(self.position, OsdPosition::Top | OsdPosition::TopEdge)
    }
}

/// A short message at the bottom of the photo view: its text, its kind and when it goes (app time, s).
#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub text: String,
    pub until: f64,
    pub kind: ToastKind,
    /// The condition a sticky message reports (`save-failed`, `pairing:<client>`); `None` for the other kinds.
    pub key: Option<String>,
    /// When it was raised (app time, s): its end is this plus the policy's time, worked out when it is shown.
    pub at: f64,
}

impl Toast {
    /// A message of `kind` shown at app time `now`, for the time [`stay_seconds`] gives it.
    pub fn new(kind: ToastKind, text: impl Into<String>, now: f64) -> Self {
        let text = text.into();
        let until = now + stay_seconds(kind, &text, MessageSettings::default().info_seconds());
        Self { text, until, kind, key: None, at: now }
    }

    /// A message that stays until dismissed or until the code that raised it clears `key`.
    pub fn sticky(key: impl Into<String>, text: impl Into<String>) -> Self {
        Self { text: text.into(), until: f64::INFINITY, kind: ToastKind::Sticky, key: Some(key.into()), at: 0.0 }
    }
}

/// How many sticky messages show beside `transients` messages that are not sticky out of `stickies`: the stack has
/// [`crate::state::MAX_TOASTS`] slots, a sticky message uses one that is free, and one always shows, so a sticky
/// message never pushes a transient one out and is never hidden by them.
pub fn sticky_slots(transients: usize, stickies: usize) -> usize {
    crate::state::MAX_TOASTS.saturating_sub(transients).max(1).min(stickies)
}

/// The width of the dismiss button of a sticky message, at the standard size.
pub const DISMISS_WIDTH: f32 = 22.0;

/// How a message is drawn. Every message, whatever its kind, is measured and painted from this.
#[derive(Clone, Copy, Debug)]
pub struct Look {
    pub fill: Color32,
    pub text: Color32,
    pub stroke: Stroke,
    pub radius: f32,
    pub pad_x: f32,
    pub pad_y: f32,
    pub min_height: f32,
    pub font: f32,
}

/// The look of a message of `kind`, `fade` (0 to 1) of the way back in, with the message settings `m`: dark text on a
/// box of the chosen colour, an error with a red edge and a reddish box.
pub fn look(t: &Tokens, kind: ToastKind, fade: f32, m: &MessageSettings) -> Look {
    let alpha = (m.opacity.clamp(0.1, 1.0) * 255.0).round() as u8;
    let (fill, stroke) = match kind {
        ToastKind::Error | ToastKind::Sticky => (Color32::from_rgba_unmultiplied(60, 14, 14, alpha), Stroke::new(1.0, t.reject.gamma_multiply(fade))),
        ToastKind::Message | ToastKind::Action => {
            let (r, g, b) = lcc_surface::settings::rgb(&m.background).unwrap_or((0, 0, 0));
            (Color32::from_rgba_unmultiplied(r, g, b, alpha), Stroke::NONE)
        }
    };
    let s = m.size.clamp(0.5, 2.0) as f32;
    Look {
        fill: fill.gamma_multiply(fade),
        text: t.text.gamma_multiply(fade),
        stroke,
        radius: 6.0 * s,
        pad_x: 20.0 * s,
        pad_y: 11.0 * s,
        min_height: 40.0 * s,
        font: 14.0 * s,
    }
}

impl Look {
    /// The box of a message whose text measures `text`: its padding and least height.
    pub fn size(&self, text: egui::Vec2) -> egui::Vec2 {
        egui::vec2(text.x + 2.0 * self.pad_x, (text.y + 2.0 * self.pad_y).max(self.min_height))
    }

    /// Paint the box.
    pub fn paint(&self, painter: &egui::Painter, rect: egui::Rect) {
        painter.rect(rect, self.radius, self.fill, self.stroke, egui::StrokeKind::Inside);
    }

    /// The same box as a frame, for a message that holds a button.
    pub fn frame(&self) -> egui::Frame {
        egui::Frame::NONE
            .fill(self.fill)
            .stroke(self.stroke)
            .corner_radius(self.radius)
            .inner_margin(egui::Margin::symmetric(self.pad_x as i8, self.pad_y as i8))
    }
}
