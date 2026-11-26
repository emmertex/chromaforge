//! The messages at the bottom centre of the window: what a message is (its kind), how long it stays (one policy)
//! and what it looks like (one look). The panels only say a kind and a text; they never pick a time or a colour.

use egui::{Color32, Stroke};

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
}

impl ToastKind {
    /// The id a script says (`ui.toast`'s `kind`, `ui.inspect`'s `toasts`).
    pub const fn id(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Action => "action",
            Self::Error => "error",
        }
    }

    /// The seconds a short message of this kind stays.
    const fn base_seconds(self) -> f64 {
        match self {
            Self::Message => 1.4,
            Self::Error => 6.0,
            Self::Action => 10.0,
        }
    }
}

/// Characters of a message read within its base time; each one past them adds [`READING_SECONDS_PER_CHAR`].
const READING_FREE_CHARS: usize = 50;
const READING_SECONDS_PER_CHAR: f64 = 0.08;
/// The longest any message stays.
const LONGEST_SECONDS: f64 = 20.0;

/// How long a message of `kind` with `text` stays: the kind's time, and more for a long text so it can be read.
/// The one place a message's time is decided; a setting can replace it here.
pub fn stay_seconds(kind: ToastKind, text: &str) -> f64 {
    let extra = text.chars().count().saturating_sub(READING_FREE_CHARS) as f64 * READING_SECONDS_PER_CHAR;
    (kind.base_seconds() + extra).min(LONGEST_SECONDS)
}

/// A short message at the bottom of the photo view: its text, its kind and when it goes (app time, s).
#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub text: String,
    pub until: f64,
    pub kind: ToastKind,
}

impl Toast {
    /// A message of `kind` shown at app time `now`, for the time [`stay_seconds`] gives it.
    pub fn new(kind: ToastKind, text: impl Into<String>, now: f64) -> Self {
        let text = text.into();
        let until = now + stay_seconds(kind, &text);
        Self { text, until, kind }
    }
}

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

/// The look of a message of `kind`, `fade` (0 to 1) of the way back in: a message is dark text on a dark box, an
/// error has a red edge and a reddish box.
pub fn look(t: &Tokens, kind: ToastKind, fade: f32) -> Look {
    let (fill, stroke) = match kind {
        ToastKind::Error => (Color32::from_rgba_unmultiplied(60, 14, 14, 215), Stroke::new(1.0, t.reject.gamma_multiply(fade))),
        ToastKind::Message | ToastKind::Action => (Color32::from_black_alpha(200), Stroke::NONE),
    };
    Look {
        fill: fill.gamma_multiply(fade),
        text: t.text.gamma_multiply(fade),
        stroke,
        radius: 6.0,
        pad_x: 20.0,
        pad_y: 11.0,
        min_height: 40.0,
        font: 14.0,
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
