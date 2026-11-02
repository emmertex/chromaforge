//! Generative fill in the UI (`docs/generative-fill.md`): the Remove panel's Generative section
//! (provider, prompt, variations, the estimate, Generate, the variation picker and warnings), the
//! consent dialog before a provider is first used, and Settings → Generative Fill (your own API
//! keys, consent per provider).
//!
//! Nothing is sent until Generate is clicked, and never to a provider whose terms the user has
//! not accepted in the consent dialog, which shows the crop that will be uploaded.

use egui::RichText;
use serde_json::{Value, json};

use crate::LccApp;
use crate::theme::Tokens;
use crate::widgets::{register, text_button};

/// Remove panel choices that are not develop settings.
#[derive(Clone, Default)]
pub struct GenfillUi {
    /// Provider id (empty: the default).
    pub provider: String,
    pub prompt: String,
    pub variations: u32,
    /// What Generate would send for (photo, spot, provider, variations, stroke), from `genfill.plan`.
    pub plan: Option<(u64, Value)>,
    /// The API key being typed in Settings, per provider (never shown back: a password field).
    pub key_input: std::collections::BTreeMap<String, String>,
    /// The consent dialog's preview of the crop: (dialog key, texture, size).
    pub preview: Option<(u64, egui::TextureHandle, [usize; 2])>,
}

// (no derived Debug: a key being typed must never end up in a debug dump)
impl std::fmt::Debug for GenfillUi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GenfillUi")
            .field("provider", &self.provider)
            .field("prompt", &self.prompt)
            .field("variations", &self.variations)
            .finish_non_exhaustive()
    }
}

fn providers(app: &mut LccApp) -> Vec<Value> {
    app.session.execute("genfill.providers", &json!({})).ok().and_then(|v| v["providers"].as_array().cloned()).unwrap_or_default()
}

fn provider_id(app: &LccApp) -> String {
    if app.ui.genfill.provider.is_empty() { "openai".to_string() } else { app.ui.genfill.provider.clone() }
}

/// The Generative section of the Remove panel, for spot `index` (selected) or the next one painted.
pub fn remove_section(app: &mut LccApp, ui: &mut egui::Ui, spot: Option<(usize, lcc_develop::Spot)>) {
    let t = Tokens::get(ui.ctx());
    let list = providers(app);
    let pid = provider_id(app);
    let current = list.iter().find(|p| p["id"] == pid.as_str()).cloned().unwrap_or(Value::Null);
    ui.label(RichText::new(crate::i18n::tr("Generative Fill")).font(t.semibold(12.5)).color(t.text));
    ui.label(
        RichText::new(crate::i18n::tr("Sends a crop around the spot to the image service you choose, with your own API key and account. Nothing is sent until you click Generate."))
            .size(11.0)
            .color(t.text_dim),
    );
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for p in &list {
            let id = p["id"].as_str().unwrap_or_default().to_string();
            let name = p["name"].as_str().unwrap_or_default();
            let short = name.split(" (").next().unwrap_or(name);
            let r = text_button(ui, &format!("genfillProvider-{id}"), short, id == pid);
            let r = r.on_hover_text(crate::i18n::tr(if p["configured"] == true {
                "API key set"
            } else {
                "No API key yet: add yours in Settings → Generative Fill"
            }));
            if r.clicked() {
                app.ui.genfill.provider = id;
                app.ui.genfill.plan = None;
            }
        }
    });
    let r = crate::text_field::TextField::singleline("genfillPrompt", &mut app.ui.genfill.prompt)
        .hint(crate::i18n::tr("What should be there (empty: remove)"))
        .label(crate::i18n::tr("Prompt"))
        .width(ui.available_width())
        .show(ui);
    if r.committed() {
        app.ui.genfill.plan = None;
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new(crate::i18n::tr("Variations")).color(t.text_label));
        let v = app.ui.genfill.variations.clamp(1, 4);
        for n in 1..=4u32 {
            if text_button(ui, &format!("genfillVariations-{n}"), &n.to_string(), v == n).clicked() {
                app.ui.genfill.variations = n;
                app.ui.genfill.plan = None;
            }
        }
    });
    let Some((index, sp)) = spot else {
        ui.label(
            RichText::new(crate::i18n::tr("Paint over what to replace with the Generative tool, then click Generate.")).size(11.0).color(t.text_dim),
        );
        return;
    };
    let Some(photo) = app.session.active() else { return };
    // the estimate, refreshed when what it depends on changes
    let n = app.ui.genfill.variations.clamp(1, 4);
    let key = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (photo.0, index, &pid, n, app.ui.genfill.prompt.trim().is_empty(), sp.stroke_key()).hash(&mut h);
        h.finish()
    };
    if app.ui.genfill.plan.as_ref().is_none_or(|(k, _)| *k != key) {
        let p = app
            .session
            .execute("genfill.plan", &json!({"index": index, "provider": pid, "variations": n, "prompt": app.ui.genfill.prompt}))
            .unwrap_or_else(|e| json!({"error": e.to_string()}));
        app.ui.genfill.plan = Some((key, p));
    }
    let plan = app.ui.genfill.plan.as_ref().map(|(_, p)| p.clone()).unwrap_or_default();
    if let Some(e) = plan["error"].as_str() {
        ui.label(RichText::new(e).size(11.0).color(t.text_dim));
    } else {
        let requests = plan["requests"].as_u64().unwrap_or(1);
        let usd = plan["usd"].as_f64().unwrap_or(0.0);
        let line = crate::i18n::tr_format!(
            "About ${usd} · {requests} request(s), billed to your {name} account",
            usd = format!("{usd:.2}"),
            requests = requests,
            name = current["name"].as_str().unwrap_or_default().split(" (").next().unwrap_or_default()
        );
        ui.label(RichText::new(line).size(11.0).color(t.text_label));
        for w in plan["warnings"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            ui.label(RichText::new(crate::i18n::tr(w)).size(11.0).color(t.caution));
        }
    }
    let running = app.session.genfill.running();
    ui.horizontal(|ui| {
        if running {
            let st = app.session.execute("genfill.status", &json!({})).unwrap_or_default();
            ui.spinner();
            ui.label(RichText::new(crate::i18n::tr_format!("Generating… {} of {}", st["done"], st["total"])).color(t.text_label));
            if text_button(ui, "genfillCancel", crate::i18n::tr("Cancel"), false).clicked() {
                let _ = app.run("genfill.cancel", json!({}));
            }
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
        } else {
            let label = if sp.generated.is_some() { "Generate Again" } else { "Generate" };
            if text_button(ui, "genfillGenerate", crate::i18n::tr(label), false).clicked() {
                generate(app, ui.ctx(), index, &current);
            }
        }
    });
    // the variations of a generated spot, and what to know about it
    if let Some(g) = &sp.generated {
        if g.variants > 1 {
            ui.horizontal(|ui| {
                if text_button(ui, "genfillPrev", "‹", false).clicked() {
                    let _ = app.run("spot.variation", json!({"index": index, "delta": -1}));
                }
                ui.label(RichText::new(crate::i18n::tr_format!("Variation {} of {}", g.variant + 1, g.variants)).color(t.text_label));
                if text_button(ui, "genfillNext", "›", false).clicked() {
                    let _ = app.run("spot.variation", json!({"index": index, "delta": 1}));
                }
            });
        }
        if sp.shown_gen().is_none() {
            ui.label(
                RichText::new(crate::i18n::tr(
                    "The spot was moved or resized since it was generated: it shows a content-aware fill until you generate it again.",
                ))
                .size(11.0)
                .color(t.caution),
            );
        } else if app.session.genfill_missing(photo, index) {
            ui.label(RichText::new(crate::i18n::tr("The generated pixels are missing from this library (moved or deleted): the spot shows a content-aware fill. Generate it again to restore it.")).size(11.0).color(t.caution));
        } else if g.scale < 0.95 {
            ui.label(
                RichText::new(crate::i18n::tr_format!(
                    "Generated at {}% of the photo's resolution: softer than the rest.",
                    format!("{:.0}", g.scale * 100.0)
                ))
                .size(11.0)
                .color(t.caution),
            );
        }
    }
}

/// Generate: straight away when the provider is set up and agreed to; else the consent dialog
/// (which shows what is sent) or a pointer to Settings for the key.
fn generate(app: &mut LccApp, ctx: &egui::Context, index: usize, provider: &Value) {
    let pid = provider["id"].as_str().unwrap_or("openai").to_string();
    if provider["configured"] != true {
        app.toast(
            ctx,
            crate::i18n::tr_format!("Add your {} API key in Settings → Generative Fill first.", provider["name"].as_str().unwrap_or_default()),
        );
        app.ui.dialog = Some(crate::state::Dialog::Settings { tab: "genfill".into() });
        return;
    }
    let (prompt, variations) = (app.ui.genfill.prompt.clone(), app.ui.genfill.variations.clamp(1, 4));
    if provider["consented"] != true {
        app.ui.genfill.preview = None;
        app.ui.dialog = Some(crate::state::Dialog::GenfillConsent { provider: pid, index, prompt, variations });
        return;
    }
    start(app, ctx, &pid, index, &prompt, variations);
}

/// Send the generation (consent and key are in place).
pub fn start(app: &mut LccApp, ctx: &egui::Context, provider: &str, index: usize, prompt: &str, variations: u32) {
    match app.run("spot.generate", json!({"index": index, "provider": provider, "prompt": prompt, "variations": variations})) {
        Ok(_) => {}
        Err(e) => app.toast(ctx, e),
    }
}

/// The consent dialog's body: who gets what, their policy, the cost, and the crop itself.
pub fn consent_body(app: &mut LccApp, ui: &mut egui::Ui, provider: &str, index: usize, prompt: &str, variations: u32) {
    let t = Tokens::get(ui.ctx());
    let list = providers(app);
    let Some(p) = list.iter().find(|p| p["id"] == provider).cloned() else { return };
    let name = p["name"].as_str().unwrap_or_default().to_string();
    let plan = app
        .session
        .execute("genfill.plan", &json!({"index": index, "provider": provider, "variations": variations, "prompt": prompt}))
        .unwrap_or_else(|e| json!({"error": e.to_string()}));
    ui.label(
        RichText::new(crate::i18n::tr_format!(
            "{app} will upload a crop of this photo to {name}, using your API key. This is the first time you use {name}.",
            name = name
        ))
        .color(t.text),
    );
    // the crop as it will be sent (from the preview-size picture: the real one has full resolution)
    let key = (index as u64) << 32 ^ provider.len() as u64 ^ app.session.active().map_or(0, |i| i.0.rotate_left(7));
    if app.ui.genfill.preview.as_ref().is_none_or(|(k, ..)| *k != key)
        && let Some(id) = app.session.active()
        && let Ok((w, h, rgb, mask)) = app.session.genfill_preview_by_id(id, index, provider)
    {
        // the area to fill tinted, so it is clear what the model is asked to change
        let rgba: Vec<u8> = rgb
            .as_chunks::<3>()
            .0
            .iter()
            .zip(&mask)
            .flat_map(|(c, m)| {
                if *m > 0 {
                    [((c[0] as u16 + 255) / 2) as u8, (c[1] as u16 / 2) as u8, (c[2] as u16 / 2) as u8, 255]
                } else {
                    [c[0], c[1], c[2], 255]
                }
            })
            .collect();
        let img = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
        let tex = ui.ctx().load_texture("genfill-consent-preview", img, egui::TextureOptions::LINEAR);
        app.ui.genfill.preview = Some((key, tex, [w, h]));
    }
    if let Some((_, tex, [w, h])) = &app.ui.genfill.preview {
        let max = egui::vec2(ui.available_width().min(420.0), 240.0);
        let k = (max.x / *w as f32).min(max.y / *h as f32).min(1.0);
        let r = ui.add(egui::Image::new((tex.id(), egui::vec2(*w as f32 * k, *h as f32 * k))));
        register(ui.ctx(), "genfill:consentPreview", r.rect);
    }
    let crops = plan["crops"].as_array().cloned().unwrap_or_default();
    let what = match crops.first() {
        Some(c) if crops.len() == 1 => crate::i18n::tr_format!(
            "Uploaded: only this {} × {} px crop (red: the area to fill) and its mask. Not the whole photo, not its metadata.",
            c["w"],
            c["h"]
        ),
        _ => crate::i18n::tr_format!(
            "Uploaded: {} crops along the area (red: the area to fill) and their masks. Not the whole photo, not its metadata.",
            crops.len()
        ),
    };
    ui.label(RichText::new(what).color(t.text_label));
    ui.label(RichText::new(crate::i18n::tr_format!("What {name} does with it:", name = name)).font(t.semibold(12.5)).color(t.text));
    ui.label(RichText::new(p["policy"]["summary"].as_str().unwrap_or_default()).color(t.text_label));
    if let Some(c) = p["caveat"].as_str().filter(|c| !c.is_empty()) {
        ui.label(RichText::new(c).color(t.caution));
    }
    let url = p["policy"]["url"].as_str().unwrap_or_default().to_string();
    let r = ui.link(crate::i18n::tr_format!("Read {name}'s policy", name = name));
    register(ui.ctx(), "link:genfillPolicy", r.rect);
    if r.clicked() {
        let _ = crate::links::open(app, &url);
    }
    if let Some(usd) = plan["usd"].as_f64() {
        ui.label(
            RichText::new(crate::i18n::tr_format!(
                "Cost: about ${usd} for {} request(s), billed by {name} to your account.",
                plan["requests"],
                usd = format!("{usd:.2}"),
                name = name
            ))
            .color(t.text_label),
        );
    }
    for w in plan["warnings"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        ui.label(RichText::new(crate::i18n::tr(w)).color(t.caution));
    }
    ui.label(
        RichText::new(crate::i18n::tr("You are asked once per service. Withdraw it any time in Settings → Generative Fill."))
            .size(11.0)
            .color(t.text_dim),
    );
}

/// Settings → Generative Fill: your own API keys and consent, per provider.
pub fn settings_tab(app: &mut LccApp, ui: &mut egui::Ui, t: &Tokens) {
    super::settings::hint(
        ui,
        t,
        crate::i18n::tr(
            "Generative Fill uses image services you pay for yourself. Create an API key in your account with the service and paste it here. Keys stay on this computer, in {app}'s settings folder: never in the catalog, XMP sidecars, exports or logs.",
        ),
    );
    for p in providers(app) {
        let id = p["id"].as_str().unwrap_or_default().to_string();
        let name = p["name"].as_str().unwrap_or_default().to_string();
        super::settings::heading(ui, t, &name);
        let status = match p["keySource"].as_str() {
            Some("settings") => crate::i18n::tr_format!("Key saved ({})", p["keyHint"].as_str().unwrap_or_default()),
            Some("env") => crate::i18n::tr_format!("Key from the environment variable {}", p["keyEnv"].as_str().unwrap_or_default()),
            _ => crate::i18n::tr("No key").to_string(),
        };
        ui.label(RichText::new(status).color(t.text_label));
        ui.horizontal(|ui| {
            let text = app.ui.genfill.key_input.entry(id.clone()).or_default();
            let r = crate::text_field::TextField::singleline(&format!("genfillKey-{id}"), text)
                .hint(crate::i18n::tr("Paste your API key"))
                .label(crate::i18n::tr_format!("{name} API key", name = name))
                .password(true)
                .width(240.0)
                .show(ui);
            let save = text_button(ui, &format!("genfillSaveKey-{id}"), crate::i18n::tr("Save Key"), false).clicked()
                || r.ending == Some(crate::text_field::Ending::Return);
            if save {
                let key = app.ui.genfill.key_input.get(&id).cloned().unwrap_or_default();
                if !key.trim().is_empty() {
                    match app.run("genfill.setKey", json!({"provider": id, "key": key})) {
                        Ok(_) => app.toast(ui.ctx(), crate::i18n::tr("API key saved")),
                        Err(e) => app.toast(ui.ctx(), e),
                    }
                }
                // the typed key is not kept in the UI
                app.ui.genfill.key_input.remove(&id);
            }
            if p["keySource"] == "settings" && text_button(ui, &format!("genfillForgetKey-{id}"), crate::i18n::tr("Remove Key"), false).clicked() {
                let _ = app.run("genfill.setKey", json!({"provider": id, "key": null}));
            }
        });
        ui.horizontal(|ui| {
            let r = ui.link(crate::i18n::tr("Get an API key"));
            register(ui.ctx(), format!("link:genfillKeyUrl-{id}"), r.rect);
            if r.clicked() {
                let _ = crate::links::open(app, p["keyUrl"].as_str().unwrap_or_default());
            }
            let r = ui.link(crate::i18n::tr("Data policy"));
            if r.clicked() {
                let _ = crate::links::open(app, p["policy"]["url"].as_str().unwrap_or_default());
            }
            if p["consented"] == true && text_button(ui, &format!("genfillWithdraw-{id}"), crate::i18n::tr("Withdraw Consent"), false).clicked() {
                let _ = app.run("genfill.consent", json!({"provider": id, "accept": false}));
            }
        });
        if p["policy"]["trainsByDefault"] == true {
            super::settings::hint(ui, t, p["policy"]["summary"].as_str().unwrap_or_default());
        }
    }
}

/// Apply a finished background generation and say how it went (every frame; cheap when idle).
pub fn frame(app: &mut LccApp, ctx: &egui::Context) {
    if app.session.genfill.running() {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
    match app.session.genfill_poll() {
        Some(Ok(g)) => {
            let mut msg = if g.fill.variants > 1 {
                crate::i18n::tr_format!("Generated {} variations", g.fill.variants)
            } else {
                crate::i18n::tr("Generated").to_string()
            };
            if let Some(w) = g.warnings.first() {
                msg = format!("{msg}: {w}");
            }
            app.toast(ctx, msg);
            ctx.request_repaint();
        }
        Some(Err(e)) => app.toast(ctx, crate::i18n::tr_format!("Generative Fill failed: {e}", e = e)),
        None => {}
    }
}
