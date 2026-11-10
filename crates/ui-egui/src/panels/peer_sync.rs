//! Settings ▸ Sync: this device's name, finding devices on the network, pairing (both screens show
//! a code, both people confirm it) and the paired devices. Everything goes through the engine's
//! `sync.*` commands ([`lcc_engine::peers`]).
//!
//! While the tab is open the app looks for devices; it stops a moment after the tab closes
//! ([`pump`]). At launch, [`pump`] starts the service once if devices are paired, so they can
//! find each other.

use egui::RichText;
use serde_json::{Value, json};

use crate::LccApp;
use crate::i18n::tr;
use crate::theme::Tokens;
use crate::widgets::{register, text_button};

/// How long after the tab was last drawn the app stops looking for devices (seconds).
const STOP_AFTER: f64 = 1.0;
/// How often the tab asks for the status (seconds).
const POLL_EVERY: f64 = 0.4;
/// How often the status is asked for while the tab is closed, so `running` is known (seconds).
const POLL_CLOSED: f64 = 2.0;
/// How often `sync.tick` runs while sync is running (seconds).
const TICK_EVERY: f64 = 1.0;

/// What the tab keeps between frames.
#[derive(Default)]
pub struct PeerSyncUi {
    status: Value,
    polled: f64,
    /// When the tab was last drawn.
    seen: f64,
    /// `sync.find {on: true}` was sent and not yet undone.
    finding: bool,
    /// `sync.start` was tried this run.
    started: bool,
    name: String,
    name_loaded: bool,
    error: Option<String>,
    /// When `sync.tick` last ran.
    ticked: f64,
}

/// `tr(template)` with each `{}` filled in turn.
macro_rules! trf {
    ($t:literal $(, $a:expr)* $(,)?) => {
        fill(tr($t), &[$(&$a as &dyn std::fmt::Display),*])
    };
}

fn fill(template: &str, args: &[&dyn std::fmt::Display]) -> String {
    let mut out = String::new();
    let mut it = args.iter();
    let mut rest = template;
    while let Some(i) = rest.find("{}") {
        out.push_str(rest.get(..i).unwrap_or_default());
        if let Some(a) = it.next() {
            out.push_str(&a.to_string());
        }
        rest = rest.get(i + 2..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}

impl PeerSyncUi {
    /// Show `status` and don't poll for a while (headless tests).
    #[cfg(test)]
    pub(crate) fn set_status_for_test(&mut self, status: Value) {
        self.status = status;
        self.polled = f64::INFINITY;
        self.finding = true;
    }
}

fn run(app: &mut LccApp, cmd: &str, params: Value) -> Option<Value> {
    match app.session.execute(cmd, &params) {
        Ok(v) => {
            app.peer_sync.error = None;
            Some(v)
        }
        Err(e) => {
            app.peer_sync.error = Some(e.to_string());
            None
        }
    }
}

/// Every frame: stop looking once the tab is closed; start listening for paired devices once.
pub fn pump(app: &mut LccApp, ctx: &egui::Context) {
    let now = ctx.input(|i| i.time);
    if app.peer_sync.finding && now - app.peer_sync.seen > STOP_AFTER {
        app.peer_sync.finding = false;
        let _ = app.session.execute("sync.find", &json!({"on": false}));
    }
    if !app.peer_sync.started {
        app.peer_sync.started = true;
        match app.session.execute("sync.start", &json!({})) {
            Ok(v) => {
                if running(&v) {
                    app.peer_sync.status["running"] = json!(true);
                    app.peer_sync.ticked = f64::NEG_INFINITY;
                }
            }
            Err(e) => log::info!("sync: {e}"),
        }
    }
    // the tab polls often while open; closed, a slow poll keeps `running` known
    if now - app.peer_sync.seen > STOP_AFTER && now - app.peer_sync.polled >= POLL_CLOSED {
        app.peer_sync.polled = now;
        if let Ok(v) = app.session.execute("sync.status", &json!({})) {
            app.peer_sync.status = v;
        }
    }
    if running(&app.peer_sync.status) && now - app.peer_sync.ticked >= TICK_EVERY {
        app.peer_sync.ticked = now;
        if let Err(e) = app.session.execute("sync.tick", &json!({})) {
            log::info!("sync tick: {e}");
        }
    }
    if running(&app.peer_sync.status) {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(TICK_EVERY));
    }
}

fn running(v: &Value) -> bool {
    v.get("running").and_then(Value::as_bool).unwrap_or(false)
}

/// "Last synced …" for a unix time, relative to now.
fn last_synced(last: i64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let ago = now.saturating_sub(last).max(0);
    if ago < 60 {
        tr("Last synced just now").to_string()
    } else if ago < 3600 {
        trf!("Last synced {} min ago", ago / 60)
    } else if ago < 86_400 {
        trf!("Last synced {} h ago", ago / 3600)
    } else {
        // the calendar day of `YYYY-MM-DDTHH:MM:SS`
        let stamp = lcc_catalog::dates::local(last).unwrap_or_else(|| lcc_catalog::dates::civil(last));
        let date = stamp.get(..10).unwrap_or(&stamp).to_string();
        trf!("Last synced on {}", date)
    }
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

pub fn settings_tab(app: &mut LccApp, ui: &mut egui::Ui, t: &Tokens) {
    let now = ui.input(|i| i.time);
    app.peer_sync.seen = now;
    if !app.peer_sync.finding {
        app.peer_sync.finding = true;
        if let Some(v) = run(app, "sync.find", json!({"on": true})) {
            app.peer_sync.status = v;
            app.peer_sync.polled = now;
        }
    }
    if now - app.peer_sync.polled >= POLL_EVERY {
        app.peer_sync.polled = now;
        if let Ok(v) = app.session.execute("sync.status", &json!({})) {
            app.peer_sync.status = v;
        }
    }
    ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(POLL_EVERY));
    let st = app.peer_sync.status.clone();
    if !app.peer_sync.name_loaded && !s(&st, "name").is_empty() {
        app.peer_sync.name = s(&st, "name").to_string();
        app.peer_sync.name_loaded = true;
    }

    super::settings::heading(ui, t, "This Device");
    let mut rename = false;
    super::settings::row(ui, t, "Name", |ui| {
        let r = crate::text_field::TextField::singleline("field:syncDeviceName", &mut app.peer_sync.name).width(240.0).show(ui);
        rename = r.committed();
    });
    super::settings::hint(ui, t, "Other devices see this name.");
    if rename && app.peer_sync.name.trim() != s(&st, "name") {
        let name = app.peer_sync.name.clone();
        if let Some(v) = run(app, "sync.rename", json!({"name": name})) {
            app.peer_sync.name = s(&v, "name").to_string();
            app.peer_sync.status = v;
        }
    }
    if let Some(e) = app.peer_sync.error.clone() {
        crate::dialog_frame::note(ui, RichText::new(e).size(11.0).color(t.reject));
    }

    // a pairing under way takes the place of the device list
    let pairing = st.get("pairing").filter(|p| !p.is_null()).cloned();
    super::settings::heading(ui, t, "Find Devices");
    match &pairing {
        Some(p) => pairing_box(app, ui, t, p),
        None => found_list(app, ui, t, &st),
    }

    super::settings::heading(ui, t, "Paired Devices");
    let paired = st.get("paired").and_then(Value::as_array).cloned().unwrap_or_default();
    if paired.is_empty() {
        super::settings::hint(ui, t, "No paired devices yet.");
    } else {
        super::settings::hint(
            ui,
            t,
            "A shared catalog exchanges the edits, ratings, flags and colour labels of the photos both devices have. A photo edited on both devices gets a copy in the same stack.",
        );
    }
    for d in &paired {
        let id = s(d, "deviceId").to_string();
        crate::dialog_frame::control_row(ui, |ui| {
            let online = d.get("online").and_then(Value::as_bool).unwrap_or(false);
            ui.label(RichText::new(if online { "●" } else { "○" }).color(if online { t.accent } else { t.text_dim }));
            ui.label(RichText::new(s(d, "name")).color(t.text));
            ui.label(RichText::new(if online { tr("Online") } else { tr("Offline") }).size(11.0).color(t.text_dim));
            if text_button(ui, &format!("syncUnpair-{id}"), tr("Remove"), false).clicked()
                && let Some(v) = run(app, "sync.unpair", json!({"device": id}))
            {
                app.peer_sync.status = v;
            }
        });
        device_library(app, ui, t, &st, &id);
    }

    let offers = st.get("offers").and_then(Value::as_array).cloned().unwrap_or_default();
    if !offers.is_empty() {
        super::settings::heading(ui, t, "Offers");
        for o in &offers {
            let id = s(o, "deviceId").to_string();
            crate::dialog_frame::note(
                ui,
                RichText::new(trf!("{} wants to sync its catalog “{}” with this catalog.", s(o, "name"), s(o, "library"))).color(t.text),
            );
            crate::dialog_frame::control_row(ui, |ui| {
                for (key, label, accept, primary) in
                    [("syncOfferAccept", "Sync With This Catalog", true, true), ("syncOfferDecline", "Decline", false, false)]
                {
                    if text_button(ui, &format!("{key}-{id}"), tr(label), primary).clicked()
                        && let Some(v) = run(app, "sync.answerOffer", json!({"device": id, "accept": accept}))
                    {
                        app.peer_sync.status = v;
                    }
                }
            });
        }
    }
}

/// Under a paired device: whether the open library is shared with it, and its last exchange.
fn device_library(app: &mut LccApp, ui: &mut egui::Ui, t: &Tokens, st: &Value, id: &str) {
    let Some(lib) = st.get("library").filter(|l| !l.is_null()) else { return };
    let shared = lib.get("sharedWith").and_then(Value::as_array).is_some_and(|a| a.iter().any(|d| d.as_str() == Some(id)));
    crate::dialog_frame::in_control_column(ui, |ui| {
        ui.indent(format!("syncDev-{id}"), |ui| {
            let mut on = shared;
            let r = ui.checkbox(&mut on, tr("Sync this catalog"));
            register(ui.ctx(), format!("check:syncShare-{id}"), r.rect);
            if r.changed()
                && let Some(v) = run(app, "sync.share", json!({"device": id, "on": on}))
            {
                app.peer_sync.status = v;
            }
            let Some(x) = st.get("exchanges").and_then(|e| e.get(id)) else { return };
            if x.get("running").and_then(Value::as_bool).unwrap_or(false) {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(10.0));
                    ui.label(RichText::new(tr("Syncing…")).size(11.0).color(t.text_dim));
                });
            } else if let Some(last) = x.get("last").and_then(Value::as_i64).filter(|l| *l > 0) {
                ui.label(RichText::new(last_synced(last)).size(11.0).color(t.text_dim));
            }
            let err = s(x, "error");
            if !err.is_empty() {
                ui.label(RichText::new(err).size(11.0).color(t.reject));
            }
            let taken = x.get("taken").and_then(Value::as_u64).unwrap_or(0);
            let copies = x.get("copies").and_then(Value::as_u64).unwrap_or(0);
            if taken > 0 || copies > 0 {
                ui.label(RichText::new(trf!("{} photos updated, {} copies", taken, copies)).size(11.0).color(t.text_dim));
            }
        });
    });
}

fn found_list(app: &mut LccApp, ui: &mut egui::Ui, t: &Tokens, st: &Value) {
    crate::dialog_frame::control_row(ui, |ui| {
        ui.add(egui::Spinner::new().size(12.0));
        ui.label(RichText::new(tr("Looking for devices on this network…")).color(t.text_label));
    });
    super::settings::hint(ui, t, "Open the Sync tab of Settings on the other device too. Both need to be on the same network.");
    let found = st.get("found").and_then(Value::as_array).cloned().unwrap_or_default();
    if found.is_empty() {
        super::settings::hint(ui, t, "No devices found yet.");
    }
    for d in &found {
        let id = s(d, "deviceId").to_string();
        crate::dialog_frame::control_row(ui, |ui| {
            ui.label(RichText::new(s(d, "name")).color(t.text));
            if text_button(ui, &format!("syncPair-{id}"), tr("Pair"), false).clicked()
                && let Some(v) = run(app, "sync.pair", json!({"device": id}))
            {
                app.peer_sync.status = v;
            }
        });
    }
}

fn pairing_box(app: &mut LccApp, ui: &mut egui::Ui, t: &Tokens, p: &Value) {
    let stage = s(p, "stage");
    ui.horizontal(|ui| {
        ui.label(RichText::new(tr("Pairing with")).color(t.text_label));
        ui.label(RichText::new(s(p, "name")).color(t.text));
    });
    match stage {
        "connecting" => {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(12.0));
                ui.label(tr("Connecting…"));
            });
        }
        "confirm" | "waiting" => {
            ui.label(tr("Check that both screens show this code:"));
            let code = s(p, "code");
            let shown = match (code.get(..3), code.get(3..)) {
                (Some(a), Some(b)) if code.len() == 6 => format!("{a} {b}"),
                _ => code.to_string(),
            };
            let r = ui.label(RichText::new(shown).font(t.semibold(28.0)).color(t.text));
            register(ui.ctx(), "syncCode", r.rect);
            if stage == "confirm" {
                ui.horizontal(|ui| {
                    if text_button(ui, "syncCodesMatch", tr("Codes Match"), true).clicked()
                        && let Some(v) = run(app, "sync.confirm", json!({"accept": true}))
                    {
                        app.peer_sync.status = v;
                    }
                    if text_button(ui, "syncCancelPairing", tr("Cancel"), false).clicked()
                        && let Some(v) = run(app, "sync.confirm", json!({"accept": false}))
                    {
                        app.peer_sync.status = v;
                    }
                });
            } else {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(12.0));
                    ui.label(tr("Waiting for the other device…"));
                });
            }
        }
        "paired" | "failed" => {
            if stage == "paired" {
                ui.label(RichText::new(tr("Paired.")).color(t.accent));
            } else {
                ui.label(RichText::new(tr("Pairing failed.")).color(t.reject));
                let why = s(p, "message");
                if !why.is_empty() {
                    ui.label(RichText::new(why).size(11.0).color(t.text_dim));
                }
            }
            if text_button(ui, "syncPairingDone", tr("OK"), false).clicked()
                && let Some(v) = run(app, "sync.clearPairing", json!({}))
            {
                app.peer_sync.status = v;
            }
        }
        _ => {}
    }
}
