//! Photo ▸ Enhance… (the dialog runs these): `enhance.rawDetails`, `enhance.superResolution`, `enhance.aiSharpen` and the per-frame `enhance.pump`. The work is
//! `crate::enhance`.

use serde_json::Value;

use super::{CommandSpec, bad, cmd};
use crate::enhance::EnhanceKind;
use crate::{Result, Session};

fn can_enhance(s: &Session) -> std::result::Result<(), String> {
    if cfg!(target_arch = "wasm32") && s.media.file_bytes.is_none() {
        return Err("Enhance needs access to the photo files".into());
    }
    // the targets are checked when run (an explicit `ids` parameter may name them)
    if s.catalog.photos().next().is_none() { Err("the library has no photos".into()) } else { Ok(()) }
}

/// `enhance.superResolution`: the classical method, or the installed upscaler (`method`, `scale`).
fn super_resolution(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "enhance.superResolution";
    let scale = match p.get("scale") {
        None | Some(Value::Null) => 2,
        Some(v) => v.as_u64().filter(|f| [2, 4].contains(f)).ok_or_else(|| bad(C, "`scale` must be 2 or 4"))? as u8,
    };
    let ai = match p.get("method").and_then(Value::as_str) {
        None | Some("classical") => false,
        Some("ai") => true,
        Some("auto") => s.enhance_upscaler_ready(),
        Some(other) => return Err(bad(C, format!("unknown method `{other}`: use classical, ai or auto"))),
    };
    if !ai && scale != 2 {
        return Err(bad(C, "the classical method doubles the size: `scale: 4` needs `method: \"ai\"`"));
    }
    s.enhance_command(if ai { EnhanceKind::AiSuperResolution(scale) } else { EnhanceKind::SuperResolution }, p)
}

fn pump(s: &mut Session, _: &Value) -> Result<Value> {
    Ok(s.enhance_pump())
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "enhance.rawDetails",
            "Raw Details",
            [],
            None,
            "{ids?, stack?: true, wait?: false} → {queued} or with `wait` {made: [{id, source, path, size, bytes}], failed: [{id, error}]} — demosaic each raw again with the slower edge-aware method (finer detail, less false colour and zipper) into a new linear DNG `<name>-Enhanced-RD.dng` next to it, imported with the source's develop settings and stacked with it. Runs in the background with a row in the activity stack; `wait: true` returns when done (scripts)",
            can_enhance,
            |s, p| s.enhance_command(EnhanceKind::RawDetails, p)
        ),
        cmd!(
            "enhance.superResolution",
            "Super Resolution",
            [],
            None,
            "{ids?, stack?: true, wait?: false, method?: classical | ai | auto, scale?: 2 | 4} → as `enhance.rawDetails` — `classical` (the default): Raw Details, then twice the width and height (four times the pixels) by iterative back-projection, into `<name>-Enhanced-SR.dng`; also takes JPEG, TIFF and other photos; at most 400 MP out. `ai`: the photo with its import defaults (none of its edits), enlarged `scale` times across by the installed upscaler (`denoise.models.select {kind: \"upscale\"}`), into a 16-bit TIFF `<name>-Enhanced-SR.tif` stacked with it; at most 110 MP out. `auto`: `ai` when an upscaler is chosen, else `classical`",
            can_enhance,
            super_resolution
        ),
        cmd!(
            "enhance.aiSharpen",
            "AI Sharpen",
            [],
            None,
            "{ids?, stack?: true, wait?: false} → as `enhance.rawDetails` — like `enhance.superResolution {method: \"ai\"}`, but the upscaler's answer is averaged back down to the photo's own size: finer detail at the same pixel dimensions, into a 16-bit TIFF `<name>-Enhanced-SR.tif`",
            can_enhance,
            |s, p| s.enhance_command(EnhanceKind::AiSharpen, p)
        ),
        cmd!(query "enhance.pump", "Enhance Pump", [], None, "{} → {active, running, made, failed} — called every frame by the app: imports the photos Enhance has finished", super::always, pump),
    ]
}
