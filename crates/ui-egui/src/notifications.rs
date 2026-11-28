//! Notifications: what Settings ▸ Notifications shows and changes. The overlay pill's settings live with the control
//! surfaces (`control-surfaces.json`) and the messages' in `ui.json`; the page reads and writes both through
//! [`NotificationSettings`], and scripts reach the messages' half through `messages.settings`.

use lcc_surface::settings::OsdSettings;
use serde_json::{Value, json};

use crate::LccApp;
use crate::toast::{self, MessageSettings, Toast, ToastKind};

/// Every notification setting: the pill's and the messages'.
#[derive(Clone, Debug, PartialEq)]
pub struct NotificationSettings {
    pub pill: OsdSettings,
    pub messages: MessageSettings,
}

impl NotificationSettings {
    /// The settings as they are now.
    pub fn read(app: &mut LccApp) -> Self {
        crate::surface::ensure_loaded(app);
        Self { pill: app.surface.settings().osd.clone(), messages: app.ui.settings.messages.clone() }
    }

    /// Make `self` the settings: each half is saved where it lives, and only if it changed. A half-typed colour
    /// stays in the field without being applied (the pill's, as its page always did; the messages' the same).
    pub fn write(self, app: &mut LccApp) {
        let mut pill = self.pill;
        if lcc_surface::settings::rgb(&pill.background).is_none() {
            // keep the half-typed colour in the field without applying it
            app.surface.runtime.settings_mut().osd.background = pill.background;
        } else if app.surface.settings().osd != pill {
            pill = OsdSettings { background: lcc_surface::settings::hex_color(&pill.background, "#202020"), ..pill };
            app.surface.runtime.settings_mut().osd = pill;
            app.surface.changed();
        }
        let typed = self.messages;
        if lcc_surface::settings::rgb(&typed.background).is_none() {
            app.ui.settings.messages.background = typed.background;
        } else {
            let messages = typed.clamped();
            if app.ui.settings.messages != messages {
                app.ui.settings.messages = messages;
            }
        }
    }
}

fn messages_json(m: &MessageSettings) -> Value {
    json!({
        "size": m.size,
        "opacity": m.opacity,
        "displayMs": m.display_ms,
        "position": crate::panels::surface_osd::position_name(m.position),
        "background": m.background,
    })
}

/// `messages.settings {size?, opacity?, displayMs?, position?, background?}`: how messages look and how long an
/// Information message stays (the others stay a multiple of it); replies with all of them. The overlay pill's are
/// `osd.settings`.
pub fn messages_command(app: &mut LccApp, p: &Value) -> Result<Value, String> {
    let mut m = app.ui.settings.messages.clone();
    let number = |key: &str, what: &str| -> Result<Option<f64>, String> {
        match p.get(key) {
            None => Ok(None),
            Some(v) => v.as_f64().filter(|v| v.is_finite()).map(Some).ok_or_else(|| format!("messages.settings: `{key}` must be a number ({what})")),
        }
    };
    if let Some(v) = number("size", "0.5–2")? {
        m.size = v.clamp(0.5, 2.0);
    }
    if let Some(v) = number("opacity", "0.1–1")? {
        m.opacity = v.clamp(0.1, 1.0);
    }
    if let Some(v) = number("displayMs", "500–10000")? {
        m.display_ms = v.clamp(500.0, 10_000.0) as u32;
    }
    if let Some(v) = p.get("position") {
        m.position =
            v.as_str().and_then(crate::panels::surface_osd::parse_position).ok_or("messages.settings: `position` is down, downLow, up or upHigh")?;
    }
    if let Some(v) = p.get("background") {
        let c = v.as_str().filter(|c| lcc_surface::settings::rgb(c).is_some()).ok_or("messages.settings: `background` is a colour like #202020")?;
        m.background = lcc_surface::settings::hex_color(c, "#000000");
    }
    if app.ui.settings.messages != m {
        app.ui.settings.messages = m.clone();
    }
    Ok(messages_json(&m))
}

/// The kinds a test-fire shows, in the order Settings lists them.
pub const TEST_KINDS: &[&str] = &["develop", "elsewhere", "information", "confirmation", "warning", "persistent"];

/// The key the test of a persistent message uses; it is cleared before each fire so it shows again after a dismiss.
const TEST_KEY: &str = "test-fire";

/// `notifications.testFire {kind}`: shows a sample of `kind` (`develop` or `elsewhere` for the pill; `information`,
/// `confirmation`, `warning` or `persistent` for a message) as it is configured, with its time in the text, through
/// the calls real ones use. Replies `{kind, shown, seconds}`: `shown` is false when a setting hides it (the pill's
/// mode or option). `none` does nothing: it is what the test message with a button runs.
pub fn test_fire(app: &mut LccApp, p: &Value) -> Result<Value, String> {
    use crate::i18n::{tr, tr_format};
    const KINDS: &str = "notifications.testFire: `kind` is develop, elsewhere, information, confirmation, warning or persistent";
    let kind = p.get("kind").and_then(Value::as_str).ok_or(KINDS)?;
    if kind == "none" {
        return Ok(json!({"kind": kind}));
    }
    crate::surface::ensure_loaded(app);
    let ctx = app.tasks.repaint.clone().unwrap_or_default();
    let info = app.ui.settings.messages.info_seconds();
    // the time said in the text is the one the message gets, which depends on the text's length: say it twice
    let said = |kind: ToastKind, text: &dyn Fn(&str) -> String| {
        let first = text(&format!("{:.1}", toast::stay_seconds(kind, "", info)));
        let secs = toast::stay_seconds(kind, &first, info);
        (text(&format!("{secs:.1}")), secs)
    };
    let reply = |shown: bool, seconds: Option<f64>| json!({"kind": kind, "shown": shown, "seconds": seconds});
    match kind {
        "develop" | "elsewhere" => {
            use lcc_engine::change_report::{ChangeKind, ChangeReport};
            let (command, label, kind_of, control, photos) = if kind == "develop" {
                ("develop.set", "Exposure", ChangeKind::Develop, Some("light.exposure".to_string()), 0)
            } else {
                ("photo.rate", "3 stars", ChangeKind::Rating(3), None, 1)
            };
            let report = ChangeReport {
                seq: 0,
                command: command.into(),
                params: json!({}),
                label: label.into(),
                photos,
                kind: kind_of,
                control,
                detail: None,
                live: false,
            };
            let shown = app.surface.settings().osd.allows(crate::panels::surface_osd::report_area(&report), true);
            app.surface.action = Some(crate::panels::surface_osd::ActionShown { report, at: app.surface.now(), test: true });
            Ok(reply(shown, Some(f64::from(app.surface.settings().osd.display_ms) / 1000.0)))
        }
        "information" => {
            let (text, secs) = said(ToastKind::Message, &|s| tr_format!("A test information message. It stays {secs} s.", secs = s));
            app.toast_as(&ctx, ToastKind::Message, text);
            Ok(reply(true, Some(secs)))
        }
        "confirmation" => {
            let (text, secs) = said(ToastKind::Action, &|s| tr_format!("A test message with a button. It stays {secs} s.", secs = s));
            app.toast_with_action(&ctx, text, tr("OK"), "notifications.testFire", json!({"kind": "none"}));
            Ok(reply(true, Some(secs)))
        }
        "warning" => {
            let (text, secs) = said(ToastKind::Error, &|s| tr_format!("A test warning. It stays {secs} s.", secs = s));
            app.toast_as(&ctx, ToastKind::Error, text);
            Ok(reply(true, Some(secs)))
        }
        "persistent" => {
            app.ui.clear_toast(TEST_KEY);
            app.ui.show_toast(Toast::sticky(TEST_KEY, tr("A test persistent warning. It stays until you dismiss it.")));
            Ok(reply(true, None))
        }
        _ => Err(KINDS.into()),
    }
}
