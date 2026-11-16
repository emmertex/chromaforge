//! A touch device's on-screen keyboard, as egui input.
//!
//! A host whose keyboard talks to the app through an input-method connection (Android) hands
//! what it sends here as [`SoftInput`]s; [`SoftKeyboard::events`] turns them into egui events for
//! the frame's raw input: typed text, the composition an input method is still working on
//! (pre-edit, e.g. a word being predicted or kana being converted), and editing keys.
//! [`SoftKeyboard::wanted`] says whether a text field has focus, so the host shows the keyboard.

use egui::{Event, ImeEvent, Key, Modifiers};

/// The most deletions one request turns into (an input method never asks for more in one go;
/// a hostile count must not flood the frame).
const MAX_DELETE: usize = 256;

/// What the on-screen keyboard sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SoftInput {
    /// Final text, at the cursor (replaces the composition, if any).
    Commit(String),
    /// The composition so far, shown at the cursor; empty clears it.
    Preedit(String),
    /// Keep the composition as it is, as final text.
    FinishComposing,
    /// Delete this many characters before and after the cursor.
    DeleteSurrounding {
        before: usize,
        after: usize,
    },
    Backspace,
    /// Forward delete.
    Delete,
    Enter,
}

/// Turns [`SoftInput`] into egui events, remembering the composition in progress.
#[derive(Debug, Default)]
pub struct SoftKeyboard {
    preedit: String,
}

impl SoftKeyboard {
    /// Appends the egui events for `input` to `out`.
    pub fn events(&mut self, input: SoftInput, out: &mut Vec<Event>) {
        match input {
            SoftInput::Commit(text) => {
                if self.preedit.is_empty() {
                    if !text.is_empty() {
                        out.push(Event::Text(text));
                    }
                } else {
                    self.preedit.clear();
                    out.push(Event::Ime(ImeEvent::Commit(text)));
                }
            }
            SoftInput::Preedit(text) => {
                if text.is_empty() && self.preedit.is_empty() {
                    return;
                }
                self.preedit.clone_from(&text);
                out.push(Event::Ime(ImeEvent::Preedit { text, active_range_chars: None }));
            }
            SoftInput::FinishComposing => {
                if !self.preedit.is_empty() {
                    out.push(Event::Ime(ImeEvent::Commit(std::mem::take(&mut self.preedit))));
                }
            }
            SoftInput::DeleteSurrounding { before, after } => {
                // the composition is the text the input method deletes into: settle it first
                self.events(SoftInput::FinishComposing, out);
                for _ in 0..before.min(MAX_DELETE) {
                    key(out, Key::Backspace);
                }
                for _ in 0..after.min(MAX_DELETE) {
                    key(out, Key::Delete);
                }
            }
            SoftInput::Backspace => {
                if self.preedit.is_empty() {
                    key(out, Key::Backspace);
                } else {
                    // a backspace while composing shortens the composition
                    let mut text = std::mem::take(&mut self.preedit);
                    text.pop();
                    self.events(SoftInput::Preedit(text), out);
                }
            }
            SoftInput::Delete => {
                self.events(SoftInput::FinishComposing, out);
                key(out, Key::Delete);
            }
            SoftInput::Enter => {
                self.events(SoftInput::FinishComposing, out);
                key(out, Key::Enter);
            }
        }
    }

    /// Forgets the composition (the keyboard went away, or focus moved).
    pub fn reset(&mut self) {
        self.preedit.clear();
    }

    /// Whether a text field wants typing this frame (call after the frame's UI ran).
    pub fn wanted(ctx: &egui::Context) -> bool {
        ctx.output(|o| o.ime.is_some())
    }
}

fn key(out: &mut Vec<Event>, key: Key) {
    for pressed in [true, false] {
        out.push(Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs frames of a focused single-line text field fed by `inputs` (one frame each), and
    /// returns its text and whether a keyboard was wanted in the last frame.
    fn type_into(start: &str, inputs: Vec<SoftInput>) -> (String, bool) {
        let ctx = egui::Context::default();
        let mut text = start.to_string();
        let mut kb = SoftKeyboard::default();
        let id = egui::Id::new("field");
        let mut wanted = false;
        let mut frame = |events: Vec<Event>, text: &mut String| {
            let raw = egui::RawInput { events, ..Default::default() };
            let mut out = ctx.run_ui(raw, |ui| {
                let r = ui.add(egui::TextEdit::singleline(text).id(id));
                if !r.has_focus() {
                    r.request_focus();
                }
                wanted = SoftKeyboard::wanted(ui.ctx());
            });
            out.textures_delta.clear();
        };
        frame(Vec::new(), &mut text);
        frame(Vec::new(), &mut text);
        for input in inputs {
            let mut events = Vec::new();
            kb.events(input, &mut events);
            frame(events, &mut text);
        }
        (text, wanted)
    }

    #[test]
    fn committed_text_is_typed() {
        let (text, wanted) = type_into("", vec![SoftInput::Commit("hello".into()), SoftInput::Commit(" world".into())]);
        assert_eq!(text, "hello world");
        assert!(wanted, "a focused text field wants the keyboard");
    }

    #[test]
    fn a_composition_shows_and_then_settles() {
        let (text, _) = type_into(
            "a",
            vec![SoftInput::Preedit("w".into()), SoftInput::Preedit("wo".into()), SoftInput::Commit("word".into()), SoftInput::Commit("!".into())],
        );
        assert_eq!(text, "aword!");
        let (text, _) = type_into("", vec![SoftInput::Preedit("ka".into()), SoftInput::FinishComposing]);
        assert_eq!(text, "ka");
    }

    #[test]
    fn deletions_and_backspace_edit_the_text() {
        let (text, _) = type_into("abcd", vec![SoftInput::Backspace, SoftInput::DeleteSurrounding { before: 2, after: 0 }]);
        assert_eq!(text, "a");
        let (text, _) = type_into("x", vec![SoftInput::Preedit("ab".into()), SoftInput::Backspace, SoftInput::FinishComposing]);
        assert_eq!(text, "xa");
    }

    #[test]
    fn a_hostile_delete_count_is_bounded() {
        let mut out = Vec::new();
        SoftKeyboard::default().events(SoftInput::DeleteSurrounding { before: usize::MAX, after: usize::MAX }, &mut out);
        assert!(out.len() <= 4 * MAX_DELETE);
    }

    #[test]
    fn nothing_wants_the_keyboard_without_a_text_field() {
        let ctx = egui::Context::default();
        let mut wanted = true;
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let _ = ui.button("ok");
            wanted = SoftKeyboard::wanted(ui.ctx());
        });
        out.textures_delta.clear();
        assert!(!wanted);
    }
}
