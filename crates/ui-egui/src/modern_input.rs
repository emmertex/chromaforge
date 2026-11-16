//! Touch scrolling for the Modern layout (fork-owned): a finger that swipes scrolls, a tap still
//! taps, and a press and hold still holds.
//!
//! egui gives a press to the widget under it at once, so a swipe that starts on a grid photo or a
//! slider drags that widget instead of scrolling the list it is in. This sits between the window's
//! input and egui (`LccApp::raw_input_hook`) and holds a press back until the finger shows what it
//! means:
//! - it moves past [`SLOP`] up or down (or either way inside a strip of photos or buttons): a
//!   scroll. The movement goes to egui as touch-phase wheel events at the press, so the scroll area
//!   under the finger scrolls and no widget sees a press. On release it keeps gliding (momentum)
//!   and slows down; a new touch stops it.
//! - it moves sideways outside those strips: the press goes through where it started, followed by
//!   the movement (a slider's drag, a text selection).
//! - it lifts without moving: the press and, on the next frame, the release (a tap).
//! - it stays put for [`LONG_PRESS`]: the press goes through (press and hold, then drag), except
//!   on a grid of photos, where it is reported as a long press (it starts a selection) and only a
//!   following move goes through, as a drag of the selection.
//!
//! Own design.

use egui::{Event, Modifiers, MouseWheelUnit, PointerButton, Pos2, RawInput, TouchPhase, Vec2};

/// How far a finger may move and still be a tap or a hold (points).
pub const SLOP: f32 = 10.0;
/// How long a still finger waits before it is a press and hold (s).
pub const LONG_PRESS: f64 = 0.45;
/// Glides slower than this (points a second) stop.
const FLING_MIN: f32 = 40.0;
/// How quickly a glide slows down: its speed falls by e every 1 / FRICTION seconds.
const FRICTION: f32 = 3.0;
/// Longest frame a glide steps over (s): after a pause it does not jump.
const MAX_DT: f32 = 0.05;

/// What lies under a press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    /// A swipe up or down scrolls; sideways goes to the widget.
    Free,
    /// A strip of buttons: any swipe scrolls; a hold goes to the widget.
    Strip,
    /// A grid or strip of photos: any swipe scrolls; a hold is a long press.
    Photos,
}

/// The scroller's state between frames (kept in `ModernUi`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scroller {
    state: State,
    /// Events for the coming frames, one batch a frame (a tap's release, a scroll's end).
    later: Vec<Vec<Event>>,
    time: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
enum State {
    #[default]
    Idle,
    /// Held back: not yet a tap, a swipe or a hold.
    Pending { origin: Pos2, t0: f64, region: Region, modifiers: Modifiers },
    /// Scrolling with the finger; `vel` in points a second.
    Scrolling { origin: Pos2, last: Pos2, moved: Vec2, vel: Vec2 },
    /// Gliding on after the finger lifted.
    Fling { at: Pos2, vel: Vec2 },
    /// A long press on photos: a move drags (the press goes through then), a release ends it.
    Held { origin: Pos2, modifiers: Modifiers },
    /// The press went to the widgets; everything passes until the release.
    Through,
}

/// What a frame's input amounted to.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Outcome {
    /// A long press on photos, where it started.
    pub long_press: Option<Pos2>,
    /// More frames are needed without input (a held press timing, a glide, a delayed release).
    pub busy: bool,
}

fn press(pos: Pos2, pressed: bool, modifiers: Modifiers) -> Event {
    Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers }
}

fn wheel(phase: TouchPhase, delta: Vec2) -> Event {
    Event::MouseWheel { unit: MouseWheelUnit::Point, delta, phase, modifiers: Modifiers::NONE }
}

impl Scroller {
    /// Rewrites this frame's pointer events. `region` says what lies under a press (`None`: give
    /// it straight to the widgets, e.g. the photo in Develop, which has gestures of its own);
    /// `toggle` makes a tap on photos a Ctrl/⌘ tap (selection mode).
    pub fn process(&mut self, raw: &mut RawInput, region: impl Fn(Pos2) -> Option<Region>, toggle: bool) -> Outcome {
        let now = raw.time.unwrap_or_else(|| self.time.map_or(0.0, |t| t + 1.0 / 60.0));
        let dt = self.time.map_or(0.0, |t| (now - t).clamp(0.0, f64::from(MAX_DT)) as f32);
        self.time = Some(now);
        let mut out: Vec<Event> = if self.later.is_empty() { Vec::new() } else { self.later.remove(0) };
        let mut outcome = Outcome::default();
        for e in std::mem::take(&mut raw.events) {
            // a position that is not a number is egui's to refuse, not something to scroll by
            if let Event::PointerMoved(p) | Event::PointerButton { pos: p, .. } = &e
                && !(p.x.is_finite() && p.y.is_finite())
            {
                out.push(e);
                continue;
            }
            match e {
                Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers } => {
                    if matches!(self.state, State::Fling { .. } | State::Scrolling { .. }) {
                        out.push(wheel(TouchPhase::End, Vec2::ZERO));
                    }
                    match region(pos) {
                        Some(r) => {
                            self.state = State::Pending { origin: pos, t0: now, region: r, modifiers };
                            // hover stays here: the scroll goes to the scroll area under the press
                            out.push(Event::PointerMoved(pos));
                        }
                        None => {
                            self.state = State::Through;
                            out.push(e);
                        }
                    }
                }
                Event::PointerMoved(p) => match self.state.clone() {
                    State::Pending { origin, region, modifiers, .. } => {
                        let d = p - origin;
                        if d.length() >= SLOP {
                            if region != Region::Free || d.y.abs() > d.x.abs() {
                                out.push(wheel(TouchPhase::Start, Vec2::ZERO));
                                out.push(wheel(TouchPhase::Move, d));
                                self.state = State::Scrolling { origin, last: p, moved: d, vel: Vec2::ZERO };
                            } else {
                                out.push(press(origin, true, modifiers));
                                out.push(e);
                                self.state = State::Through;
                            }
                        }
                    }
                    State::Scrolling { origin, last, moved, vel } => {
                        let d = p - last;
                        if d != Vec2::ZERO {
                            out.push(wheel(TouchPhase::Move, d));
                        }
                        self.state = State::Scrolling { origin, last: p, moved: moved + d, vel };
                    }
                    State::Held { origin, modifiers } => {
                        if (p - origin).length() >= SLOP {
                            out.push(press(origin, true, modifiers));
                            out.push(e);
                            self.state = State::Through;
                        }
                    }
                    State::Fling { .. } => {}
                    State::Idle | State::Through => out.push(e),
                },
                Event::PointerButton { button: PointerButton::Primary, pressed: false, pos, modifiers } => match self.state.clone() {
                    State::Pending { origin, region, modifiers: m, .. } => {
                        let m = if toggle && region == Region::Photos { m | Modifiers::COMMAND } else { m };
                        if m != modifiers {
                            out.push(Event::ModifiersChanged(m));
                        }
                        out.push(press(origin, true, m));
                        self.later.push(vec![press(origin, false, m)]);
                        if m != modifiers {
                            self.later.push(vec![Event::ModifiersChanged(modifiers)]);
                        }
                        self.state = State::Idle;
                    }
                    State::Scrolling { origin, vel, .. } => {
                        if vel.length() >= FLING_MIN && vel.is_finite() {
                            self.state = State::Fling { at: origin, vel };
                        } else {
                            self.later.push(vec![wheel(TouchPhase::End, Vec2::ZERO)]);
                            self.state = State::Idle;
                        }
                    }
                    State::Held { .. } => self.state = State::Idle,
                    State::Fling { .. } => {}
                    State::Idle | State::Through => {
                        out.push(e);
                        self.state = State::Idle;
                        let _ = pos;
                    }
                },
                // the finger lifted: while scrolling or gliding the hover must stay put
                Event::PointerGone
                    if matches!(self.state, State::Scrolling { .. } | State::Fling { .. } | State::Pending { .. } | State::Held { .. }) => {}
                // a tap's release waits for the next frame: the finger's lifting goes after it, or
                // egui loses the pointer between the press and the release and sees no click
                Event::PointerGone if !self.later.is_empty() => {
                    if let Some(last) = self.later.last_mut() {
                        last.push(Event::PointerGone);
                    }
                }
                other => out.push(other),
            }
        }
        // the frame's end: velocity, holds and glides
        match self.state.clone() {
            State::Scrolling { origin, last, moved, vel } => {
                let vel = if dt > 0.0 { vel * 0.3 + moved / dt * 0.7 } else { vel };
                self.state = State::Scrolling { origin, last, moved: Vec2::ZERO, vel };
            }
            State::Pending { origin, t0, region, modifiers } if now - t0 >= LONG_PRESS => {
                if region == Region::Photos {
                    outcome.long_press = Some(origin);
                    self.state = State::Held { origin, modifiers };
                } else {
                    out.push(press(origin, true, modifiers));
                    self.state = State::Through;
                }
            }
            State::Fling { at, vel } => {
                let vel = vel * (-FRICTION * dt).exp();
                if vel.length() < FLING_MIN || !vel.is_finite() {
                    out.push(wheel(TouchPhase::End, Vec2::ZERO));
                    self.state = State::Idle;
                } else {
                    out.push(Event::PointerMoved(at));
                    out.push(wheel(TouchPhase::Move, vel * dt));
                    self.state = State::Fling { at, vel };
                }
            }
            _ => {}
        }
        raw.events = out;
        outcome.busy = self.busy();
        outcome
    }

    /// Whether frames are due without new input (a held press timing, a glide, a delayed
    /// release): the frame asks for the next one, since the host may otherwise wait for input.
    pub fn busy(&self) -> bool {
        !self.later.is_empty() || matches!(self.state, State::Pending { .. } | State::Fling { .. })
    }

    /// Scrolling with a finger, or gliding on.
    #[cfg(test)]
    fn scrolling(&self) -> bool {
        matches!(self.state, State::Scrolling { .. } | State::Fling { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(s: &mut Scroller, t: f64, events: Vec<Event>, region: Option<Region>) -> (Vec<Event>, Outcome) {
        let mut raw = RawInput { time: Some(t), events, ..Default::default() };
        let o = s.process(&mut raw, |_| region, false);
        (raw.events, o)
    }

    fn pressed(events: &[Event]) -> bool {
        events.iter().any(|e| matches!(e, Event::PointerButton { pressed: true, .. }))
    }

    fn wheel_y(events: &[Event]) -> f32 {
        events.iter().map(|e| if let Event::MouseWheel { delta, .. } = e { delta.y } else { 0.0 }).sum()
    }

    #[test]
    fn a_vertical_swipe_scrolls_and_glides_without_pressing_anything() {
        let mut s = Scroller::default();
        let a = Pos2::new(100.0, 400.0);
        let (e, _) = frame(&mut s, 0.0, vec![press(a, true, Modifiers::NONE)], Some(Region::Free));
        assert!(!pressed(&e), "held back");
        let mut scrolled = 0.0;
        for i in 1..=6 {
            let (e, _) = frame(&mut s, f64::from(i) / 60.0, vec![Event::PointerMoved(a - Vec2::new(0.0, 30.0 * i as f32))], Some(Region::Free));
            assert!(!pressed(&e));
            scrolled += wheel_y(&e);
        }
        assert!((scrolled + 180.0).abs() < 1e-3, "the content follows the finger ({scrolled})");
        let (e, o) = frame(&mut s, 7.0 / 60.0, vec![press(a, false, Modifiers::NONE)], Some(Region::Free));
        assert!(!e.iter().any(|e| matches!(e, Event::PointerButton { .. })), "no release either");
        assert!(o.busy && s.scrolling(), "a fast swipe glides on");
        let mut glide = 0.0;
        for i in 8..400 {
            let (e, _) = frame(&mut s, f64::from(i) / 60.0, vec![], None);
            glide += wheel_y(&e);
            if !s.scrolling() {
                break;
            }
        }
        assert!(glide < -10.0 && !s.scrolling(), "it glides the same way, then stops ({glide})");
    }

    #[test]
    fn a_tap_is_a_press_then_a_release_and_sideways_goes_to_the_widget() {
        let mut s = Scroller::default();
        let a = Pos2::new(50.0, 50.0);
        frame(&mut s, 0.0, vec![press(a, true, Modifiers::NONE)], Some(Region::Free));
        let (e, _) = frame(&mut s, 0.05, vec![press(a, false, Modifiers::NONE)], Some(Region::Free));
        assert!(pressed(&e));
        let (e, _) = frame(&mut s, 0.07, vec![], Some(Region::Free));
        assert!(e.iter().any(|e| matches!(e, Event::PointerButton { pressed: false, .. })), "released next frame");
        // sideways on a slider: the press goes through where it started
        frame(&mut s, 1.0, vec![press(a, true, Modifiers::NONE)], Some(Region::Free));
        let (e, _) = frame(&mut s, 1.02, vec![Event::PointerMoved(a + Vec2::new(30.0, 2.0))], Some(Region::Free));
        assert!(matches!(e.first(), Some(Event::PointerButton { pos, pressed: true, .. }) if *pos == a), "{e:?}");
        assert_eq!(wheel_y(&e), 0.0);
    }

    #[test]
    fn a_hold_on_photos_is_a_long_press_and_elsewhere_a_press() {
        let mut s = Scroller::default();
        let a = Pos2::new(10.0, 10.0);
        frame(&mut s, 0.0, vec![press(a, true, Modifiers::NONE)], Some(Region::Photos));
        let (e, o) = frame(&mut s, LONG_PRESS + 0.01, vec![], Some(Region::Photos));
        assert_eq!(o.long_press, Some(a));
        assert!(!pressed(&e), "a long press on photos selects; nothing is pressed");
        let (e, _) = frame(&mut s, 1.0, vec![Event::PointerMoved(a + Vec2::new(40.0, 0.0))], Some(Region::Photos));
        assert!(pressed(&e), "moving after it drags the selection");
        let mut s = Scroller::default();
        frame(&mut s, 0.0, vec![press(a, true, Modifiers::NONE)], Some(Region::Strip));
        let (e, o) = frame(&mut s, LONG_PRESS + 0.01, vec![], Some(Region::Strip));
        assert!(o.long_press.is_none() && pressed(&e), "a hold elsewhere is a press and hold");
    }

    #[test]
    fn a_quick_tap_keeps_the_pointer_until_its_release() {
        // Android: the press in one frame; the release and the finger's lifting in the next
        let mut s = Scroller::default();
        let a = Pos2::new(80.0, 40.0);
        frame(&mut s, 0.0, vec![press(a, true, Modifiers::NONE)], Some(Region::Free));
        let (e, o) = frame(&mut s, 0.01, vec![press(a, false, Modifiers::NONE), Event::PointerGone], Some(Region::Free));
        assert!(pressed(&e) && o.busy, "the press now, the release later");
        assert!(!e.iter().any(|e| matches!(e, Event::PointerGone)), "the pointer stays for the release");
        let (e, _) = frame(&mut s, 0.02, vec![], Some(Region::Free));
        let release = e.iter().position(|e| matches!(e, Event::PointerButton { pressed: false, .. }));
        let gone = e.iter().position(|e| matches!(e, Event::PointerGone));
        assert!(matches!((release, gone), (Some(r), Some(g)) if r < g), "the release, then the lifting: {e:?}");
    }

    #[test]
    fn hostile_input_never_panics() {
        let mut s = Scroller::default();
        let weird = [f32::NAN, f32::INFINITY, -f32::INFINITY, 1e30, -1e30, 0.0];
        let mut t = 0.0;
        for x in weird {
            for y in weird {
                let p = Pos2::new(x, y);
                for e in [press(p, true, Modifiers::NONE), Event::PointerMoved(p), press(p, false, Modifiers::NONE), Event::PointerGone] {
                    t += 0.01;
                    frame(&mut s, t, vec![e], Some(Region::Photos));
                }
            }
        }
        // time running backwards, or no time at all
        frame(&mut s, -5.0, vec![], None);
        let mut raw = RawInput::default();
        s.process(&mut raw, |_| None, true);
    }
}
