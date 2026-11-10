//! Generative fill commands (`docs/generative-fill.md`): the providers and their terms, the
//! user's own API keys (never echoed), consent, what a generation would send and cost, Generate,
//! and choosing a variation. Plus `spot.fromMask`, which turns a mask (an Object selection made
//! with a click, a brushed mask…) into a Fill or Generate spot.

use lcc_develop::{Spot, SpotMode};
use lcc_geom::Point;
use serde_json::{Value, json};

#[cfg(not(target_arch = "wasm32"))]
use super::f64_or;
use super::{CommandSpec, bad, bool_or, cmd, has_active, str_param};
use crate::{Result, Session};

/// `index` from the params, else the selected spot.
fn spot_index(s: &Session, p: &Value, c: &str) -> Result<usize> {
    let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
    let n = s.develop_of(id).map_or(0, |d| d.spots.len());
    let i = p.get("index").and_then(Value::as_u64).map(|v| v as usize).or(s.active_spot).ok_or_else(|| bad(c, "no spot selected (give `index`)"))?;
    if i >= n {
        return Err(bad(c, "no such spot"));
    }
    Ok(i)
}

#[cfg(not(target_arch = "wasm32"))]
fn provider(p: &Value, c: &str) -> Result<lcc_genfill::cloud::Provider> {
    match str_param(p, "provider") {
        None => Ok(lcc_genfill::cloud::DEFAULT),
        Some(id) => lcc_genfill::cloud::Provider::from_id(id).ok_or_else(|| {
            let ids: Vec<&str> = lcc_genfill::cloud::ALL.iter().map(|p| p.info().id).collect();
            bad(c, format!("unknown provider `{id}` (one of {})", ids.join(", ")))
        }),
    }
}

/// One provider as `genfill.providers` lists it (never the key).
#[cfg(not(target_arch = "wasm32"))]
pub fn provider_json(s: &Session, pr: lcc_genfill::cloud::Provider) -> Value {
    let i = pr.info();
    json!({
        "id": i.id,
        "name": i.name,
        "default": pr == lcc_genfill::cloud::DEFAULT,
        "configured": s.genfill.keys.status(pr) != lcc_genfill::cloud::keys::KeySource::None,
        "keySource": s.genfill.keys.status(pr).id(),
        "keyHint": s.genfill.keys.hint(pr),
        "keyEnv": lcc_genfill::cloud::keys::env_var(pr),
        "keyUrl": i.key_url,
        "consented": s.genfill.consented(i.id),
        "fillModel": i.fill_model,
        "removeModel": i.remove_model,
        "maxSide": i.max_side,
        "usdPerImage": i.usd_per_image,
        "policy": {"summary": i.policy.summary, "url": i.policy.url, "trainsByDefault": i.policy.trains_by_default, "checked": i.policy.checked, "provenance": i.policy.provenance},
        "caveat": i.caveat,
    })
}

/// What generating spot `index` with `pr` would send: the plan for the photo's native size.
#[cfg(not(target_arch = "wasm32"))]
pub fn plan_json(
    s: &Session,
    id: crate::PhotoId,
    index: usize,
    pr: lcc_genfill::cloud::Provider,
    variations: u32,
    remove: bool,
) -> std::result::Result<(Value, crate::genfill::Plan), String> {
    let p = s.catalog.photo(id).ok_or("no such photo")?;
    let spot = p.develop.spots.get(index).ok_or("no such spot")?;
    let lim = crate::genfill::Limits::for_provider(pr);
    let plan = crate::genfill::plan(spot, &p.develop, p.embedded_lens.as_deref(), p.width as usize, p.height as usize, lim)?;
    let n = variations.clamp(1, crate::genfill::MAX_VARIATIONS);
    let requests = plan.requests(pr, n, remove);
    let images = n;
    let mut warnings = Vec::new();
    if plan.scale < 0.95 {
        warnings.push(format!(
            "The area is larger than {} sends at full resolution: it is generated at {:.0} % and comes out softer than the rest of the photo.",
            pr.info().name,
            plan.scale * 100.0
        ));
    }
    if requests > crate::genfill::MAX_REQUESTS {
        warnings.push(format!(
            "This would take {requests} requests (at most {}): paint a smaller area or ask for fewer variations.",
            crate::genfill::MAX_REQUESTS
        ));
    }
    let t = &plan.tile;
    let crop = json!({"x": t.crop[0], "y": t.crop[1], "w": t.crop[2] - t.crop[0], "h": t.crop[3] - t.crop[1], "sentW": t.send.0, "sentH": t.send.1});
    let v = json!({
        "provider": pr.info().id,
        "requests": requests,
        "images": images,
        "usd": (f64::from(images) * pr.info().usd_per_image * 100.0).round() / 100.0,
        "scale": plan.scale,
        "soft": plan.scale < 0.95,
        "crop": crop,
        "photo": {"w": p.width, "h": p.height},
        "sends": "only this crop, in one request, with the mask of the area to fill; never the whole photo unless it is smaller than the crop, never its metadata",
        "warnings": warnings,
    });
    Ok((v, plan))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "genfill.providers", "Generative Fill Providers", [], None, "{} → {providers: [{id, name, default, configured, keySource: none|settings|env, keyHint (last 4 characters), keyEnv, keyUrl, consented, maxSide, usdPerImage, policy: {summary, url, trainsByDefault, checked, provenance}, caveat}], mock: bool, running}: never the keys", |_| Ok(()), |s, _| {
            #[cfg(not(target_arch = "wasm32"))]
            {
                let list: Vec<Value> = lcc_genfill::cloud::ALL.iter().map(|p| provider_json(s, *p)).collect();
                let mock = lcc_brand::env("GENFILL_MOCK").is_some_and(|v| !v.trim().is_empty());
                Ok(json!({"providers": list, "mock": mock, "running": s.genfill.running()}))
            }
            #[cfg(target_arch = "wasm32")]
            {
                let _ = s;
                Ok(json!({"providers": [], "mock": false, "running": false}))
            }
        }),
        cmd!(query "genfill.setKey", "Set Generative Fill API Key", [], None, "{provider, key: string | null (forget it)}: store your own API key for a provider in the app's settings folder (outside the catalog, never in XMP, logs or exports) → {provider, configured, keySource, keyHint}; the key is never returned", |_| Ok(()), |s, p| {
            let c = "genfill.setKey";
            #[cfg(not(target_arch = "wasm32"))]
            {
                let pr = provider(p, c)?;
                let key = match p.get("key") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(k)) if k.trim().is_empty() => None,
                    Some(Value::String(k)) => Some(k.as_str()),
                    Some(_) => return Err(bad(c, "`key` must be a string or null")),
                };
                s.genfill.keys.set(pr, key).map_err(|e| bad(c, e))?;
                Ok(json!({"provider": pr.info().id, "configured": s.genfill.keys.status(pr) != lcc_genfill::cloud::keys::KeySource::None, "keySource": s.genfill.keys.status(pr).id(), "keyHint": s.genfill.keys.hint(pr)}))
            }
            #[cfg(target_arch = "wasm32")]
            {
                let _ = (s, p);
                Err(bad(c, "generative fill is not available in the web version"))
            }
        }),
        cmd!(query "genfill.consent", "Generative Fill Consent", [], None, "{provider, accept: bool}: record that you read the provider's data policy and agree to send crops of your photos to it (the consent dialog); `accept: false` withdraws it → {provider, consented}", |_| Ok(()), |s, p| {
            let c = "genfill.consent";
            #[cfg(not(target_arch = "wasm32"))]
            {
                let pr = provider(p, c)?;
                let accept = p.get("accept").and_then(Value::as_bool).ok_or_else(|| bad(c, "missing bool `accept`"))?;
                s.genfill_consent(pr.info().id, accept).map_err(|e| bad(c, e))?;
                Ok(json!({"provider": pr.info().id, "consented": s.genfill.consented(pr.info().id)}))
            }
            #[cfg(target_arch = "wasm32")]
            {
                let _ = (s, p);
                Err(bad(c, "generative fill is not available in the web version"))
            }
        }),
        cmd!(query "genfill.plan", "Generative Fill Estimate", [], None, "{index? (the selected spot), provider?, variations?: 1..4 (1), prompt?, previewPath?: write the crop as it would be sent (PNG) and its mask (…-mask.png)}: what Generate would send and cost (always one crop in one request; an area too large for one is an error, sending nothing); sends nothing → {provider, requests, images, usd, scale, soft, crop: {x, y, w, h, sentW, sentH}, warnings, consented, configured}", has_active, |s, p| {
            let c = "genfill.plan";
            #[cfg(not(target_arch = "wasm32"))]
            {
                let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
                let i = spot_index(s, p, c)?;
                let pr = provider(p, c)?;
                let remove = str_param(p, "prompt").is_none_or(|t| t.trim().is_empty());
                let (mut v, _) = plan_json(s, id, i, pr, f64_or(p, "variations", 1.0) as u32, remove).map_err(|e| bad(c, e))?;
                v["consented"] = json!(s.genfill.consented(pr.info().id));
                v["configured"] = json!(s.genfill.keys.status(pr) != lcc_genfill::cloud::keys::KeySource::None);
                if let Some(path) = str_param(p, "previewPath") {
                    let (w, h, rgb, mask) = s.genfill_preview(id, i, pr).map_err(|e| bad(c, e))?;
                    let png = lcc_genfill::cloud::util::png_encode(w, h, 3, &rgb).map_err(|e| bad(c, e))?;
                    std::fs::write(path, png).map_err(|e| bad(c, format!("could not write the preview: {e}")))?;
                    let mpath = std::path::Path::new(path).with_extension("mask.png");
                    let m = lcc_genfill::cloud::util::png_encode(w, h, 1, &mask).map_err(|e| bad(c, e))?;
                    std::fs::write(&mpath, m).map_err(|e| bad(c, format!("could not write the preview: {e}")))?;
                    v["previewPath"] = json!(path);
                    v["previewSize"] = json!([w, h]);
                }
                Ok(v)
            }
            #[cfg(target_arch = "wasm32")]
            {
                let _ = (s, p);
                Err(bad(c, "generative fill is not available in the web version"))
            }
        }),
        cmd!(query "spot.generate", "Generate Fill", [], None, "{index? (the selected spot), provider? (openai), variations?: 1..4 (1), prompt? (empty: remove)}: send the spot's context crop, in one request (an area too large for one is refused before anything is sent), to the provider with YOUR key (after you accepted its terms) and store the results as patches of the photo's own pixels; the spot becomes Generative (one undo step). In the app it runs in the background (activity row with ✕) → {started}; elsewhere it waits → {generated: {id, variants, scale}, warnings}. Never journaled or replayed.", has_active, |s, p| {
            let c = "spot.generate";
            #[cfg(not(target_arch = "wasm32"))]
            {
                let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
                let i = spot_index(s, p, c)?;
                let pr = provider(p, c)?;
                let prompt = str_param(p, "prompt").unwrap_or_default().to_string();
                let n = f64_or(p, "variations", 1.0).clamp(1.0, f64::from(crate::genfill::MAX_VARIATIONS)) as u32;
                match s.genfill_start(id, i, pr, &prompt, n).map_err(|e| bad(c, e))? {
                    None => Ok(json!({"started": true})),
                    Some(g) => Ok(json!({"generated": {"id": g.fill.id, "variants": g.fill.variants, "scale": g.fill.scale}, "warnings": g.warnings})),
                }
            }
            #[cfg(target_arch = "wasm32")]
            {
                let _ = (s, p);
                Err(bad(c, "generative fill is not available in the web version"))
            }
        }),
        cmd!(query "genfill.status", "Generative Fill Status", [], None, "{} → {running, done, total, provider, last: {ok, warnings?, error?}}", |_| Ok(()), |s, _| {
            let (running, done, total, provider) = match &s.genfill.job {
                Some(j) => match j.state() {
                    crate::genfill::JobState::Running { done, total } => (true, done, total, Some(j.provider.clone())),
                    _ => (false, 0, 0, Some(j.provider.clone())),
                },
                None => (false, 0, 0, None),
            };
            let last = match &s.genfill.last {
                Some(Ok(g)) => json!({"ok": true, "warnings": g.warnings, "id": g.fill.id}),
                Some(Err(e)) => json!({"ok": false, "error": e}),
                None => Value::Null,
            };
            Ok(json!({"running": running, "done": done, "total": total, "provider": provider, "last": last}))
        }),
        cmd!(query "genfill.cancel", "Cancel Generative Fill", [], None, "{} → {cancelled}", |_| Ok(()), |s, _| Ok(json!({"cancelled": s.genfill_cancel()}))),
        cmd!(
            "spot.variation",
            "Choose Generated Variation",
            [],
            None,
            "{index? (the selected spot), variant?: 0-based (of the generation shown), result?: 0-based (of every generation's results: Generate Again keeps the earlier ones), delta?: ±1 (through every result)}: which generated result a Generative spot shows → {variant, variants, result, results, id}",
            has_active,
            |s, p| {
                let c = "spot.variation";
                let i = spot_index(s, p, c)?;
                let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
                let mut d = (*s.develop_of(id).unwrap_or_default()).clone();
                let g = d.spots.get_mut(i).and_then(|sp| sp.generated.as_mut()).ok_or_else(|| bad(c, "this spot has no generated variations"))?;
                let (total, at) = g.results();
                let k =
                    match (p.get("variant").and_then(Value::as_i64), p.get("result").and_then(Value::as_i64), p.get("delta").and_then(Value::as_i64))
                    {
                        (Some(v), _, _) => {
                            let n = i64::from(g.variants.max(1));
                            if !(0..n).contains(&v) {
                                return Err(bad(c, format!("variant must be 0..{}", n - 1)));
                            }
                            i64::from(at) - i64::from(g.variant) + v
                        }
                        (None, Some(r), _) => r,
                        (None, None, Some(dl)) => (i64::from(at) + dl).rem_euclid(i64::from(total.max(1))),
                        (None, None, None) => return Err(bad(c, "give `variant`, `result` or `delta`")),
                    };
                if !(0..i64::from(total)).contains(&k) || !g.choose(k as u32) {
                    return Err(bad(c, format!("result must be 0..{}", total.saturating_sub(1))));
                }
                let (results, result) = g.results();
                let out = json!({"variant": g.variant, "variants": g.variants, "result": result, "results": results, "id": g.id});
                s.set_develop(id, d, "Choose Variation")?;
                Ok(out)
            }
        ),
        cmd!(
            "spot.putBack",
            "Put Back Generated Fill",
            [],
            None,
            "{index? (the selected spot)}: put a Generative spot that was moved or resized since its fill was generated back where the fill was made, so it shows again (one undo step) → {index, shown}",
            has_active,
            |s, p| {
                let c = "spot.putBack";
                let i = spot_index(s, p, c)?;
                let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
                s.genfill_put_back(id, i).map_err(|e| bad(c, e))?;
                let shown = s.develop_of(id).and_then(|d| d.spots.get(i).map(|sp| sp.shown_gen().is_some())).unwrap_or(false);
                Ok(json!({"index": i, "shown": shown}))
            }
        ),
        cmd!(query "genfill.results", "Generated Fills of This Photo", [], None, "{}: every generated fill kept in the library for the active photo, newest first (each a paid result; none is ever deleted by the app) → {results: [{id, variants, provider, prompt, made (seconds since 1970), spot: index | null (the spot that keeps it), shown}]}", has_active, |s, _| {
            let c = "genfill.results";
            let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
            let list: Vec<Value> = s
                .genfill_results(id)
                .into_iter()
                .map(|f| json!({"id": f.id, "variants": f.variants, "provider": f.provider, "prompt": f.prompt, "made": f.made, "spot": f.spot, "shown": f.shown}))
                .collect();
            Ok(json!({"results": list}))
        }),
        cmd!(
            "spot.restoreGenerated",
            "Restore Generated Fill",
            [],
            None,
            "{id: a generated fill's id (genfill.results)}: show that generated fill on the photo again: on the spot that keeps it, else on a new spot where it was made (one undo step) → {index}",
            has_active,
            |s, p| {
                let c = "spot.restoreGenerated";
                let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
                let which = str_param(p, "id").ok_or_else(|| bad(c, "missing `id`"))?.to_string();
                let index = s.genfill_restore(id, &which).map_err(|e| bad(c, e))?;
                Ok(json!({"index": index}))
            }
        ),
        cmd!(
            "spot.fromMask",
            "Remove Masked Area",
            [],
            None,
            "{mask: id (the selected mask), mode?: fill|generate (fill), deleteMask?: bool (true)}: turn a mask (an Object selection, a brushed mask…) into a Fill or Generative spot covering it (slightly grown to take in edges and shadows); one undo step → {index, dabs}",
            has_active,
            |s, p| {
                let c = "spot.fromMask";
                let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
                let mid = p
                    .get("mask")
                    .and_then(Value::as_u64)
                    .map(|v| v as u32)
                    .or(s.active_mask)
                    .ok_or_else(|| bad(c, "no mask selected (give `mask`)"))?;
                let mode = match str_param(p, "mode").unwrap_or("fill") {
                    "fill" => SpotMode::Fill,
                    "generate" => SpotMode::Generate,
                    m => return Err(bad(c, format!("mode must be fill or generate, not `{m}`"))),
                };
                let d = s.develop_of(id).ok_or_else(|| bad(c, "no photo"))?;
                if !d.masks.iter().any(|m| m.id == mid) {
                    return Err(bad(c, "no such mask"));
                }
                let spot = spot_from_mask(s, id, mid, mode).map_err(|e| bad(c, e))?;
                let mut d = (*d).clone();
                if bool_or(p, "deleteMask", true) {
                    d.masks.retain(|m| m.id != mid);
                }
                let dabs = spot.points.len();
                d.spots.push(spot);
                let n = d.spots.len();
                s.set_develop(id, d, "Remove Masked Area")?;
                if bool_or(p, "deleteMask", true) && s.active_mask == Some(mid) {
                    s.active_mask = None;
                }
                s.active_spot = Some(n - 1);
                Ok(json!({"index": n - 1, "dabs": dabs}))
            }
        ),
    ]
}

/// Most dabs a spot made from a mask gets.
const MAX_DABS: usize = 900;
/// Long edge of the render the mask is read from.
const MASK_EDGE: usize = 1024;

/// A spot whose dabs cover mask `mid` of photo `id` (read at [`MASK_EDGE`] px, uncropped).
fn spot_from_mask(s: &mut Session, id: crate::PhotoId, mid: u32, mode: SpotMode) -> std::result::Result<Spot, String> {
    use lcc_pipeline::{MaskView, Overlay};
    let mut job = s.render_job(id, MASK_EDGE, MASK_EDGE, false, false).ok_or("no photo")?;
    job.request.overlay =
        Overlay::Mask { id: u16::try_from(mid).map_err(|_| "unusable mask id")?, view: MaskView::WhiteOnBlack, color: [255, 255, 255], opacity: 100 };
    job.key ^= 0x6d61_736b_7370_6f74;
    let img = job.run().rendered?.image;
    let (w, h) = (img.width, img.height);
    let inside: Vec<bool> = img.data.iter().map(|p| p[0] > 127).collect();
    let area = inside.iter().filter(|v| **v).count();
    if area == 0 {
        return Err("the mask is empty: select something first".into());
    }
    // a grid of dabs over the mask, close enough that their discs leave no gap
    let step = ((area as f64 / MAX_DABS as f64).sqrt().ceil() as usize).max(3);
    let r = step as f64 * 0.75;
    let p = s.catalog.photo(id).ok_or("no photo")?;
    let frame =
        lcc_pipeline::geometry::Frame::with_lens(p.width.max(1) as usize, p.height.max(1) as usize, &p.develop, false, p.embedded_lens.as_deref());
    let to_norm = frame.out_to_norm(w, h);
    let mut points = Vec::new();
    for y in (step / 2..h).step_by(step) {
        for x in (step / 2..w).step_by(step) {
            // a dab where any pixel of its cell is in the mask
            let hit = (y.saturating_sub(step / 2)..(y + step / 2 + 1).min(h))
                .any(|yy| (x.saturating_sub(step / 2)..(x + step / 2 + 1).min(w)).any(|xx| inside.get(yy * w + xx).copied().unwrap_or(false)));
            if hit {
                points.push(to_norm.apply(Point::new(x as f64 + 0.5, y as f64 + 0.5)));
            }
        }
    }
    let size = (r / w.max(h) as f64).clamp(super::masks::SPOT_SIZE.0, super::masks::SPOT_SIZE.1);
    Ok(Spot { mode, points, size, feather: 30.0, opacity: 100.0, source_offset: None, auto: None, generated: None })
}
