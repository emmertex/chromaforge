//! Holding Space never repeats: its auto-repeat presses are dropped, typed text stays.

use egui::{Event, Key, Modifiers, RawInput};

fn key(key: Key, pressed: bool, repeat: bool) -> Event {
    Event::Key { key, physical_key: None, pressed, repeat, modifiers: Modifiers::NONE }
}

#[test]
fn space_repeats_are_dropped_other_keys_and_text_stay() {
    let mut raw = RawInput {
        events: vec![
            key(Key::Space, true, false),
            key(Key::Space, true, true),
            Event::Text(" ".into()),
            key(Key::Space, true, true),
            key(Key::ArrowRight, true, true),
            key(Key::Space, false, false),
        ],
        ..Default::default()
    };
    crate::drop_space_repeats(&mut raw);
    let spaces_pressed = raw.events.iter().filter(|e| matches!(e, Event::Key { key: Key::Space, pressed: true, .. })).count();
    assert_eq!(spaces_pressed, 1, "one press, no repeats");
    assert!(raw.events.iter().any(|e| matches!(e, Event::Text(t) if t == " ")), "typed text stays");
    assert!(raw.events.iter().any(|e| matches!(e, Event::Key { key: Key::ArrowRight, repeat: true, .. })), "other keys still repeat");
    assert!(raw.events.iter().any(|e| matches!(e, Event::Key { key: Key::Space, pressed: false, .. })), "the release stays");
}
