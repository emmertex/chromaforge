//! Generative fill in the UI (`docs/generative-fill.md`): the Remove panel's Generative section
//! (the services set up, prompt, variations, the estimate, Generate, the variation picker,
//! warnings and a failed generation with Retry), the short consent dialog before a service is
//! first used, Settings → Generative Fill (a card per service, your own API keys, consent), and
//! the spinner on the photo while a spot is generating.
//!
//! Nothing is sent until Generate is clicked, and never to a provider whose terms the user has
//! not accepted in the consent dialog, which shows the crop that will be uploaded.

use egui::RichText;
use serde_json::{Value, json};

use crate::LccApp;
use crate::theme::Tokens;
use crate::widgets::{register, text_button};

/// Remove panel choices that are not develop settings (the remembered ones are in
/// [`crate::state::UiState`]: `genfill_provider`, `genfill_variations`, `genfill_sharp`).
#[derive(Clone, Default)]
pub struct GenfillUi {
    pub prompt: String,
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
        f.debug_struct("GenfillUi").field("prompt", &self.prompt).finish_non_exhaustive()
    }
}

fn providers(app: &mut LccApp) -> Vec<Value> {
    app.session.execute("genfill.providers", &json!({})).ok().and_then(|v| v["providers"].as_array().cloned()).unwrap_or_default()
}

/// The provider's short name ("OpenAI" of "OpenAI (GPT Image)").
fn short_name(p: &Value) -> String {
    let name = p["name"].as_str().unwrap_or_default();
    name.split(" (").next().unwrap_or(name).to_string()
}

/// The provider to use: the one last chosen while it has a key, else the first with a key.
fn current(app: &LccApp, configured: &[Value]) -> Option<Value> {
    configured.iter().find(|p| p["id"] == app.ui.genfill_provider.as_str()).or_else(|| configured.first()).cloned()
}

/// The last generation's error, if it failed (cancelling is not a failure).
fn last_error(app: &LccApp) -> Option<String> {
    match &app.session.genfill.last {
        Some(Err(e)) if e != "cancelled" => Some(e.clone()),
        _ => None,
    }
}

fn percent(v: &Value) -> String {
    format!("{:.0}", v.as_f64().unwrap_or(1.0) * 100.0)
}

/// The Generative section of the Remove panel, for spot `index` (selected) or the next one painted.
pub fn remove_section(app: &mut LccApp, ui: &mut egui::Ui, spot: Option<(usize, lcc_develop::Spot)>) {
    let t = Tokens::get(ui.ctx());
    let configured: Vec<Value> = providers(app).into_iter().filter(|p| p["configured"] == true).collect();
    ui.label(RichText::new(crate::i18n::tr("Generative Fill")).font(t.semibold(12.5)).color(t.text));
    // only services with a key show here; Settings → Generative Fill sets them up
    let Some(current) = current(app, &configured) else {
        ui.label(
            RichText::new(crate::i18n::tr("Replaces the painted area using an online image service, with your own account. No service is set up yet."))
                .size(11.0)
                .color(t.text_dim),
        );
        if text_button(ui, "genfillSetUp", crate::i18n::tr("Set Up a Service…"), false).clicked() {
            app.ui.dialog = Some(crate::state::Dialog::Settings { tab: "genfill".into() });
        }
        return;
    };
    let pid = current["id"].as_str().unwrap_or("openai").to_string();
    let name = short_name(&current);
    if configured.len() > 1 {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for p in &configured {
                let id = p["id"].as_str().unwrap_or_default().to_string();
                if text_button(ui, &format!("genfillProvider-{id}"), &short_name(p), id == pid).clicked() {
                    app.ui.genfill_provider = id;
                    app.ui.genfill.plan = None;
                }
            }
        });
    } else {
        ui.label(RichText::new(crate::i18n::tr_format!("Service: {name}", name = name)).size(11.0).color(t.text_label));
    }
    let r = crate::text_field::TextField::singleline("genfillPrompt", &mut app.ui.genfill.prompt)
        .hint(crate::i18n::tr("What should be there (empty: remove)"))
        .label(crate::i18n::tr("Prompt"))
        .width(ui.available_width())
        .show(ui);
    if r.committed() {
        app.ui.genfill.plan = None;
    }
    // each variation is a paid result: 1 unless the user asks for more (remembered)
    let usd_each = current["usdPerImage"].as_f64().unwrap_or(0.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(crate::i18n::tr("Variations")).color(t.text_label));
        let v = app.ui.genfill_variations.clamp(1, 4);
        for n in 1..=4u32 {
            if text_button(ui, &format!("genfillVariations-{n}"), &n.to_string(), v == n).clicked() {
                app.ui.genfill_variations = n;
                app.ui.genfill.plan = None;
            }
        }
    });
    let Some((index, sp)) = spot else {
        ui.label(
            RichText::new(crate::i18n::tr_format!("Paint over what to replace, then click Generate. About ${usd} per variation.", usd = format!("{usd_each:.2}")))
                .size(11.0)
                .color(t.text_dim),
        );
        error_box(app, ui, &t, None);
        return;
    };
    let Some(photo) = app.session.active() else { return };
    // the estimate, refreshed when what it depends on changes
    let n = app.ui.genfill_variations.clamp(1, 4);
    let sharp = app.ui.genfill_sharp;
    let key = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (photo.0, index, &pid, n, sharp, app.ui.genfill.prompt.trim().is_empty(), sp.stroke_key()).hash(&mut h);
        h.finish()
    };
    if app.ui.genfill.plan.as_ref().is_none_or(|(k, _)| *k != key) {
        let p = app
            .session
            .execute("genfill.plan", &json!({"index": index, "provider": pid, "variations": n, "prompt": app.ui.genfill.prompt, "sharp": sharp}))
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
            name = name
        );
        ui.label(RichText::new(line).size(11.0).color(t.text_label));
        // a large area: sharper tiles (more requests) or one softer crop
        if plan["other"].is_object() {
            let mut v = app.ui.genfill_sharp;
            if ui.checkbox(&mut v, crate::i18n::tr("Sharper result for a large area")).changed() {
                app.ui.genfill_sharp = v;
                app.ui.genfill.plan = None;
            }
            let pct = percent(&plan["scale"]);
            let note = if sharp && plan["tiles"].as_u64().unwrap_or(1) > 1 {
                crate::i18n::tr_format!("Sent as {} pieces at {}% of the photo's resolution.", plan["tiles"], pct)
            } else {
                crate::i18n::tr_format!("Sent as one piece at {}% of the photo's resolution: softer than the rest.", pct)
            };
            ui.label(RichText::new(note).size(11.0).color(if plan["soft"] == true { t.caution } else { t.text_dim }));
        } else if plan["soft"] == true {
            ui.label(
                RichText::new(crate::i18n::tr_format!("Generated at {}% of the photo's resolution: softer than the rest.", percent(&plan["scale"])))
                    .size(11.0)
                    .color(t.caution),
            );
        }
        if requests > u64::from(lcc_engine::genfill::MAX_REQUESTS) {
            ui.label(RichText::new(crate::i18n::tr("Too many requests: ask for fewer variations or paint a smaller area.")).size(11.0).color(t.caution));
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
    if !running {
        error_box(app, ui, &t, Some((index, &current)));
    }
    if sp.generated.is_none() && !running && last_error(app).is_none() {
        ui.label(
            RichText::new(crate::i18n::tr("Not generated yet: the area is only marked. Nothing is sent until you click Generate."))
                .size(11.0)
                .color(t.text_dim),
        );
    }
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
                RichText::new(crate::i18n::tr("The spot was moved or resized since it was generated: the photo shows through until you generate it again."))
                    .size(11.0)
                    .color(t.caution),
            );
        } else if app.session.genfill_missing(photo, index) {
            ui.label(
                RichText::new(crate::i18n::tr(
                    "The generated pixels are missing from this library (moved or deleted): the photo shows through. Generate it again to restore it.",
                ))
                .size(11.0)
                .color(t.caution),
            );
        } else if g.scale < 0.95 {
            ui.label(
                RichText::new(crate::i18n::tr_format!("Generated at {}% of the photo's resolution: softer than the rest.", format!("{:.0}", g.scale * 100.0)))
                    .size(11.0)
                    .color(t.caution),
            );
        }
    }
}

/// A failed generation: what went wrong, Retry (for spot `retry`) and Dismiss. It stays until
/// dismissed or the next generation, so a failure is never missed.
fn error_box(app: &mut LccApp, ui: &mut egui::Ui, t: &Tokens, retry: Option<(usize, &Value)>) {
    let Some(e) = last_error(app) else { return };
    egui::Frame::new().stroke(egui::Stroke::new(1.0, t.caution)).corner_radius(4.0).inner_margin(6.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let r = ui.add(egui::Label::new(RichText::new(crate::i18n::tr_format!("Generative Fill failed: {e}", e = e)).size(11.0).color(t.caution)).wrap());
        register(ui.ctx(), "genfill:error", r.rect);
        ui.horizontal(|ui| {
            if let Some((index, provider)) = retry
                && text_button(ui, "genfillRetry", crate::i18n::tr("Retry"), false).clicked()
            {
                generate(app, ui.ctx(), index, provider);
            }
            if text_button(ui, "genfillDismiss", crate::i18n::tr("Dismiss"), false).clicked() {
                app.session.genfill.last = None;
            }
        });
    });
}

/// Generate: straight away when the provider is set up and agreed to; else the consent dialog
/// (which shows what is sent) or a pointer to Settings for the key.
fn generate(app: &mut LccApp, ctx: &egui::Context, index: usize, provider: &Value) {
    let pid = provider["id"].as_str().unwrap_or("openai").to_string();
    if provider["configured"] != true {
        app.toast(ctx, crate::i18n::tr_format!("Add your {} API key in Settings → Generative Fill first.", provider["name"].as_str().unwrap_or_default()));
        app.ui.dialog = Some(crate::state::Dialog::Settings { tab: "genfill".into() });
        return;
    }
    app.ui.genfill_provider = pid.clone();
    let (prompt, variations) = (app.ui.genfill.prompt.clone(), app.ui.genfill_variations.clamp(1, 4));
    if provider["consented"] != true {
        app.ui.genfill.preview = None;
        app.ui.dialog = Some(crate::state::Dialog::GenfillConsent { provider: pid, index, prompt, variations });
        return;
    }
    start(app, ctx, &pid, index, &prompt, variations);
}

/// Send the generation (consent and key are in place). A refusal before anything is sent (no
/// key, one already running) shows like a failed generation, with Retry.
pub fn start(app: &mut LccApp, ctx: &egui::Context, provider: &str, index: usize, prompt: &str, variations: u32) {
    if let Err(e) = send(app, provider, index, prompt, variations) {
        app.toast_error(ctx, crate::i18n::tr_format!("Generative Fill failed: {e}", e = e));
    }
}

/// [`start`] without the toast (the consent dialog's Generate): the error is also kept for the
/// Remove panel's Retry.
pub fn send(app: &mut LccApp, provider: &str, index: usize, prompt: &str, variations: u32) -> Result<Value, String> {
    app.session.genfill.last = None;
    let sharp = app.ui.genfill_sharp;
    app.run("spot.generate", json!({"index": index, "provider": provider, "prompt": prompt, "variations": variations, "sharp": sharp})).map_err(|e| {
        let e = e.to_string();
        app.session.genfill.last = Some(Err(e.clone()));
        e
    })
}

/// The consent dialog's body, short and calm: the crop that is sent, its size and the cost. The
/// service's data policy and provenance marks are behind Details.
pub fn consent_body(app: &mut LccApp, ui: &mut egui::Ui, provider: &str, index: usize, prompt: &str, variations: u32) {
    let t = Tokens::get(ui.ctx());
    let list = providers(app);
    let Some(p) = list.iter().find(|p| p["id"] == provider).cloned() else { return };
    let name = short_name(&p);
    let sharp = app.ui.genfill_sharp;
    let plan = app
        .session
        .execute("genfill.plan", &json!({"index": index, "provider": provider, "variations": variations, "prompt": prompt, "sharp": sharp}))
        .unwrap_or_else(|e| json!({"error": e.to_string()}));
    // the crop as it will be sent (from the preview-size picture: the real one has full resolution)
    let key = (index as u64) << 32 ^ provider.len() as u64 ^ app.session.active().map_or(0, |i| i.0.rotate_left(7)) ^ u64::from(sharp) << 40;
    if app.ui.genfill.preview.as_ref().is_none_or(|(k, ..)| *k != key)
        && let Some(id) = app.session.active()
        && let Ok((w, h, rgb, mask)) = app.session.genfill_preview_by_id(id, index, provider, sharp)
    {
        // the area to fill lightly tinted, so it is clear what the service is asked to change
        let rgba: Vec<u8> = rgb
            .as_chunks::<3>()
            .0
            .iter()
            .zip(&mask)
            .flat_map(|(c, m)| {
                if *m > 0 {
                    [((c[0] as u16 * 3 + 60) / 4) as u8, ((c[1] as u16 * 3 + 140) / 4) as u8, ((c[2] as u16 * 3 + 255) / 4) as u8, 255]
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
        let max = egui::vec2(ui.available_width().min(340.0), 200.0);
        let k = (max.x / *w as f32).min(max.y / *h as f32).min(1.0);
        ui.vertical_centered(|ui| {
            let r = ui.add(egui::Image::new((tex.id(), egui::vec2(*w as f32 * k, *h as f32 * k))).corner_radius(3.0));
            register(ui.ctx(), "genfill:consentPreview", r.rect);
        });
    }
    let crops = plan["crops"].as_array().cloned().unwrap_or_default();
    let what = match crops.first() {
        Some(c) if crops.len() == 1 => crate::i18n::tr_format!("Sends this {} × {} px area to {name}.", c["w"], c["h"], name = name),
        _ => crate::i18n::tr_format!("Sends this area to {name}, in {} pieces.", crops.len(), name = name),
    };
    ui.label(RichText::new(what).color(t.text));
    if let Some(usd) = plan["usd"].as_f64() {
        ui.label(RichText::new(crate::i18n::tr_format!("About ${usd}, billed to your {name} account.", usd = format!("{usd:.2}"), name = name)).color(t.text_label));
    }
    // a warning only when it matters
    if plan["soft"] == true {
        ui.label(
            RichText::new(crate::i18n::tr_format!(
                "The area is larger than {name} takes at full resolution: it will be softer than the rest of the photo ({}%).",
                percent(&plan["scale"]),
                name = name
            ))
            .size(11.0)
            .color(t.caution),
        );
    }
    // a service that may train on what it gets: said up front, not only in Details
    if p["policy"]["trainsByDefault"] == true
        && let Some(c) = p["caveat"].as_str().filter(|c| !c.is_empty())
    {
        ui.label(RichText::new(c).size(11.0).color(t.caution));
    }
    ui.add_space(2.0);
    egui::CollapsingHeader::new(crate::i18n::tr("Details")).id_salt("genfill-consent-details").default_open(false).show(ui, |ui| {
        ui.label(
            RichText::new(crate::i18n::tr("Only this area and the outline of what to fill are sent: not the whole photo, not its metadata."))
                .size(11.0)
                .color(t.text_label),
        );
        ui.label(RichText::new(p["policy"]["summary"].as_str().unwrap_or_default()).size(11.0).color(t.text_label));
        if let Some(pv) = p["policy"]["provenance"].as_str().filter(|v| !v.is_empty()) {
            ui.label(RichText::new(pv).size(11.0).color(t.text_label));
        }
        ui.label(
            RichText::new(crate::i18n::tr(
                "{app} keeps only the pixels of the area you fill: provenance metadata such as C2PA does not reach your photo or its exports. A watermark hidden in the pixels may.",
            ))
            .size(11.0)
            .color(t.text_dim),
        );
        if p["policy"]["trainsByDefault"] != true
            && let Some(c) = p["caveat"].as_str().filter(|c| !c.is_empty())
        {
            ui.label(RichText::new(c).size(11.0).color(t.text_label));
        }
        let url = p["policy"]["url"].as_str().unwrap_or_default().to_string();
        let r = ui.link(crate::i18n::tr_format!("Read {name}'s policy", name = name));
        register(ui.ctx(), "link:genfillPolicy", r.rect);
        if r.clicked() {
            let _ = crate::links::open(app, &url);
        }
        ui.label(RichText::new(crate::i18n::tr("You are asked once per service. Withdraw it any time in Settings → Generative Fill.")).size(11.0).color(t.text_dim));
    });
}

/// What a service is good and less good at, in plain words, for its card in Settings (by id).
fn strengths(id: &str) -> (&'static str, &'static str) {
    match id {
        "openai" => (
            "Following a written prompt: adding or changing something, and understanding the whole scene.",
            "Redraws the whole crop, so details can drift from the photo; large areas come out softer (at most 1536 px per request); slower (30 to 60 seconds).",
        ),
        "stability" => (
            "Removing things without a prompt (Erase), and large areas: up to 3072 px per request, so big areas stay sharper. Fast and cheap.",
            "Follows written prompts less well. Uses uploads for training unless you switch it off in your account.",
        ),
        "bfl" => (
            "Photographic realism: re-creating textures, extending a scene. Up to 2048 px per request.",
            "Its terms let it train on uploads, with no opt-out. Not for photos of children.",
        ),
        "fal" => (
            "FLUX Fill and object removal through one pay-as-you-go account. Up to 2048 px per request.",
            "Does not say whether it trains on uploads. FLUX's terms apply: not for photos of children.",
        ),
        _ => ("", ""),
    }
}

/// A service's card: what it is good at, its weak spots, size, price, training and marks.
fn provider_card(ui: &mut egui::Ui, t: &Tokens, p: &Value) {
    let (good, weak) = strengths(p["id"].as_str().unwrap_or_default());
    let row = |ui: &mut egui::Ui, label: &str, text: String| {
        ui.scope(|ui| {
            ui.set_max_width(ui.available_width().min(680.0));
            let mut job = egui::text::LayoutJob::default();
            job.append(
                &format!("{} ", crate::i18n::tr(label)),
                0.0,
                egui::TextFormat { font_id: t.semibold(11.0), color: t.text_label, ..Default::default() },
            );
            job.append(&text, 0.0, egui::TextFormat { font_id: egui::FontId::proportional(11.0), color: t.text_label, ..Default::default() });
            ui.add(egui::Label::new(job).wrap());
        });
    };
    if !good.is_empty() {
        row(ui, "Best at:", crate::i18n::tr(good).to_string());
        row(ui, "Weak spots:", crate::i18n::tr(weak).to_string());
    }
    row(
        ui,
        "Size and price:",
        crate::i18n::tr_format!(
            "up to {} px at full resolution, about ${usd} per fill",
            p["maxSide"],
            usd = format!("{:.2}", p["usdPerImage"].as_f64().unwrap_or(0.0))
        ),
    );
    row(
        ui,
        "Trains on uploads:",
        crate::i18n::tr(if p["policy"]["trainsByDefault"] == true { "Yes, unless you opt out where the service allows it (see Data policy)" } else { "No" }).to_string(),
    );
    if let Some(pv) = p["policy"]["provenance"].as_str().filter(|v| !v.is_empty()) {
        row(ui, "Marks added:", pv.to_string());
    }
}

/// Settings → Generative Fill: a card per service, your own API keys and consent.
pub fn settings_tab(app: &mut LccApp, ui: &mut egui::Ui, t: &Tokens) {
    super::settings::hint(
        ui,
        t,
        crate::i18n::tr(
            "Generative Fill uses image services you pay for yourself. Create an API key in your account with the service and paste it here. Keys stay on this computer, in {app}'s settings folder: never in the catalog, XMP sidecars, exports or logs.",
        ),
    );
    super::settings::hint(
        ui,
        t,
        crate::i18n::tr(
            "Recommended for removing distractions from client photos: OpenAI (it does not train on what you send) or Stability AI with training switched off in your account. For realistic textures: FLUX Fill, but its terms allow training on uploads.",
        ),
    );
    for p in providers(app) {
        let id = p["id"].as_str().unwrap_or_default().to_string();
        let name = p["name"].as_str().unwrap_or_default().to_string();
        super::settings::heading(ui, t, &name);
        provider_card(ui, t, &p);
        let status = match p["keySource"].as_str() {
            Some("settings") => crate::i18n::tr_format!("Key saved (ends in {})", p["keyHint"].as_str().unwrap_or_default()),
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
        // (the Remove panel keeps the error, with Retry, until it is dismissed)
        Some(Err(e)) if e == "cancelled" => app.toast(ctx, crate::i18n::tr("Generative Fill cancelled")),
        Some(Err(e)) => app.toast_error(ctx, crate::i18n::tr_format!("Generative Fill failed: {e}", e = e)),
        None => {}
    }
}

/// On the photo while a spot is generating: a spinner and "Generating…" over the area, so it is
/// clear the result is on its way (the photo shows through until it arrives; the outline is the
/// Remove tool's own). `screen` maps normalized photo coordinates to the screen.
pub fn canvas(app: &LccApp, ui: &egui::Ui, painter: &egui::Painter, screen: &dyn Fn(lcc_geom::Point) -> egui::Pos2, d: &lcc_develop::DevelopSettings) {
    let Some(job) = app.session.genfill.job.as_ref().filter(|_| app.session.genfill.running()) else { return };
    if app.session.active() != Some(job.photo) {
        return;
    }
    let Some(sp) = d.spots.get(job.spot).filter(|s| s.stroke_key() == job.stroke) else { return };
    let pts: Vec<egui::Pos2> = sp.points.iter().map(|p| screen(*p)).collect();
    let Some(first) = pts.first() else { return };
    let c = pts.iter().fold(egui::Rect::from_min_max(*first, *first), |r, p| r.union(egui::Rect::from_min_max(*p, *p))).center();
    // a turning arc on a dark disc
    let rad = 14.0;
    let turn = ui.input(|i| i.time) as f32 * 4.0;
    let arc: Vec<egui::Pos2> = (0..=24)
        .map(|i| {
            let a = turn + i as f32 / 24.0 * std::f32::consts::PI * 1.5;
            c + egui::vec2(a.cos(), a.sin()) * rad
        })
        .collect();
    painter.circle_filled(c, rad + 6.0, egui::Color32::from_black_alpha(140));
    painter.add(egui::Shape::line(arc, egui::Stroke::new(2.5, egui::Color32::WHITE)));
    let galley = painter.layout_no_wrap(crate::i18n::tr("Generating…").to_string(), egui::FontId::proportional(12.0), egui::Color32::WHITE);
    let at = egui::pos2(c.x - galley.size().x / 2.0, c.y + rad + 10.0);
    painter.rect_filled(egui::Rect::from_min_size(at, galley.size()).expand2(egui::vec2(6.0, 3.0)), 4.0, egui::Color32::from_black_alpha(150));
    painter.galley(at, galley, egui::Color32::WHITE);
    register(ui.ctx(), "genfill:canvasSpinner", egui::Rect::from_center_size(c, egui::vec2(rad * 2.0, rad * 2.0)));
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
}
