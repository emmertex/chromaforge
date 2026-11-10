//! The built-in profiles: TactileBridge's curated layouts (Default, Wedding, Portrait, Fashion,
//! Landscape, Real Estate) for the MX Creative Keypad and Dialpad, with every action mapped to
//! the app, plus the pages that follow the app's tools.
//!
//! Portions © RAD Films - Roman Alurkoff (from TactileBridge), MIT: the profile names, pages and
//! their key and dial layouts are TactileBridge's (`tools/GenerateProfiles.cs`); its tool-context
//! layouts (`TactileBridgeSettings.CreateDefaultToolContexts`) shaped the tool pages.
//!
//! Each TactileBridge profile came in three Logi device variants: `…70` for the Keypad (key
//! pages), `…71` for the Dialpad (four buttons and dial pages) and `…72` for the Actions Ring
//! (a copy of the Keypad's first pages and the Dialpad's dial pages). Here the three are one
//! profile: key pages from `70`, dial pages and buttons from `71` (`72` adds nothing new).
//! One change: the Dialpad's fourth button turns the dial pages (TactileBridge left that to Logi
//! Options+); the action it had is on a key page too.
//!
//! The profiles that ship are the TactileBridge author's own working layouts
//! (`data/default-profiles.json`, by RAD Films - Roman Alurkoff, MIT): the same six profiles,
//! tuned through daily use on weddings, with a Loupedeck+ layout on every profile. Keys that applied the
//! author's own presets were left out. [`tactilebridge_profiles`] builds the original layouts in
//! code; it serves if the file can't be read.

use serde_json::json;

/// The shipped profiles (see the module docs).
const DEFAULT_PROFILES: &str = include_str!("../data/default-profiles.json");

use crate::binding::{Binding, Modifier};
use crate::devices::PageGroup;
use crate::profile::{Page, Profile};
use crate::tactilebridge::binding_of;

/// A key page from TactileBridge action keys (key 1 … key 9).
fn keys(name: &str, tb: [&str; 9]) -> Page {
    let mut p = Page { name: name.to_string(), groups: vec![PageGroup::Keys], ..Default::default() };
    for (i, k) in tb.iter().enumerate() {
        if let Some(b) = binding_of(k) {
            p.bindings.insert(format!("key.{}", i + 1), b);
        }
    }
    p
}

/// A dial page from TactileBridge dial values in its order: encoder 0 (the roller), then
/// encoder 1 (the big dial) — TactileBridge's Wedding layout keeps Exposure on the roller and
/// Kelvin on the dial.
fn dials(name: &str, roller: &str, dial: &str) -> Page {
    let mut p = Page { name: name.to_string(), groups: vec![PageGroup::Dials], ..Default::default() };
    for (slot, k) in [("roller", roller), ("dial", dial)] {
        if let Some(b) = binding_of(k) {
            p.bindings.insert(slot.to_string(), b);
        }
    }
    p
}

/// A dial page from the app targets.
fn lc_dials(name: &str, dial: &str, roller: &str) -> Page {
    Page::new(name, &[PageGroup::Dials], [("dial", Binding::value(dial)), ("roller", Binding::value(roller))])
}

/// The Dialpad's buttons: three TactileBridge actions and the dial-page switch.
fn pads(p: &mut Profile, tb: [&str; 3]) {
    for (i, k) in tb.iter().enumerate() {
        if let Some(b) = binding_of(k) {
            p.base.insert(format!("pad.{}", i + 1), b);
        }
    }
    p.base.insert("pad.4".into(), Binding::PageNext { group: Some(PageGroup::Dials) });
}

fn cmd(c: &str) -> Binding {
    Binding::command(c)
}
fn cmdp(c: &str, p: serde_json::Value) -> Binding {
    Binding::command_with(c, p)
}
fn val(t: &str) -> Binding {
    Binding::value(t)
}

/// The Masks panel's page: the mask tools on the first keys, the mask's sliders on the next (press one to put it on
/// the dial), the overlay on key 7 (decision 2026-10-25, item 14; it was Temp), its exposure on the dial and its
/// amount on the roller. A layout saved with Temp there keeps it.
pub fn masks_page() -> Page {
    Page::new(
        MASKS,
        &[],
        [
            ("key.1", cmdp("tool.brush", json!({"new": true}))),
            ("key.2", cmd("tool.linear")),
            ("key.3", cmd("tool.radial")),
            ("key.4", val("mask.exposure")),
            ("key.5", val("mask.highlights")),
            ("key.6", val("mask.shadows")),
            ("key.7", cmd("view.maskOverlay")),
            ("key.8", cmd("surface.maskNext")),
            ("key.9", cmd("tool.done")),
            ("dial", val("mask.exposure")),
            ("roller", val("mask.amount")),
        ],
    )
}

/// The Masks panel's page name.
pub const MASKS: &str = "Masks";

/// The two pages format 3 had for the Masks panel, as built in: "Masking" (no mask selected) and "Mask" (a mask
/// selected), to tell a user's own changes from the defaults when they are merged (format 4).
fn old_mask_pages() -> (Page, Page) {
    let masking = Page::new(
        "Masking",
        &[],
        [
            ("key.1", cmdp("tool.brush", json!({"new": true}))),
            ("key.2", cmd("tool.linear")),
            ("key.3", cmd("tool.radial")),
            ("key.4", cmdp("mask.add", json!({"kind": "subject"}))),
            ("key.5", cmdp("mask.add", json!({"kind": "sky"}))),
            ("key.6", cmdp("mask.add", json!({"kind": "luminanceRange"}))),
            ("key.7", cmd("view.maskOverlay")),
            ("key.8", cmd("edit.undo")),
            ("key.9", cmd("tool.done")),
            ("dial", val("ui.zoom")),
        ],
    );
    let mask = Page::new(
        "Mask",
        &[],
        [
            ("key.1", val("mask.exposure")),
            ("key.2", val("mask.contrast")),
            ("key.3", val("mask.highlights")),
            ("key.4", val("mask.shadows")),
            ("key.5", val("mask.temp")),
            ("key.6", val("mask.saturation")),
            ("key.7", cmd("mask.invert")),
            ("key.8", cmd("surface.maskPrevious")),
            ("key.9", cmd("surface.maskNext")),
            ("dial", val("mask.exposure")),
            ("roller", val("mask.amount")),
        ],
    );
    (masking, mask)
}

/// What merging a profile's Masks pages did ([`merge_mask_pages`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaskPagesMerged {
    /// The profile's name.
    pub profile: String,
    /// The controls whose own (changed) bindings were carried into the Masks page.
    pub kept: Vec<String>,
}

/// Format 4 (test notes 2026-10-24 note 24): the Masks panel had two near-identical pages, "Masking" (no mask
/// selected) and "Mask" (a mask selected), and a change made on one didn't show on the other. They become one
/// "Masks" page shown whenever the Masks panel is open (Brush keeps its own). Per control, a binding the user
/// changed on "Mask" is kept, else one changed on "Masking", else the Masks page's own. A page of the user's own
/// mapped there instead stays in charge ("Mask"'s first). `None` when the profile had nothing to merge.
pub fn merge_mask_pages(p: &mut Profile) -> Option<MaskPagesMerged> {
    let first = |p: &Profile, k: &str| p.contexts.get(k).and_then(|l| l.first()).cloned();
    let masking = first(p, "masking");
    let mask = first(p, "masking.mask").or_else(|| first(p, "masking.mask.gradient"));
    if mask.is_none() && masking.as_deref().is_none_or(|n| n == MASKS) {
        return None;
    }
    let (old_masking, old_mask) = old_mask_pages();
    // a page of the user's own (not one of the built-in mask pages)
    let own = |name: &Option<String>| name.as_deref().is_some_and(|n| n != old_masking.name && n != old_mask.name && n != MASKS);
    let mut kept = Vec::new();
    let chosen = if own(&mask) {
        mask.clone()
    } else if own(&masking) {
        masking.clone()
    } else {
        // merge the two built-in pages, as the user left them, into Masks
        let mut merged = p.page(MASKS).cloned().unwrap_or_else(|| Page { auto: true, ..masks_page() });
        let pm = masking.as_deref().filter(|n| *n == old_masking.name).and_then(|n| p.page(n)).cloned();
        let pk = mask.as_deref().filter(|n| *n == old_mask.name).and_then(|n| p.page(n)).cloned();
        let mut controls: std::collections::BTreeSet<String> = merged.bindings.keys().cloned().collect();
        for pg in [&pm, &pk].into_iter().flatten() {
            controls.extend(pg.bindings.keys().cloned());
        }
        for c in controls {
            let changed = |pg: &Option<Page>, old: &Page| {
                pg.as_ref().filter(|pg| pg.bindings.get(&c) != old.bindings.get(&c)).map(|pg| pg.bindings.get(&c).cloned())
            };
            let take = changed(&pk, &old_mask).or_else(|| changed(&pm, &old_masking));
            if let Some(b) = take {
                kept.push(c.clone());
                match b {
                    Some(b) => {
                        merged.bindings.insert(c, b);
                    }
                    None => {
                        merged.bindings.remove(&c);
                    }
                }
            }
        }
        match p.pages.iter_mut().find(|pg| pg.name == MASKS) {
            Some(pg) => *pg = merged,
            None => p.pages.push(merged),
        }
        Some(MASKS.to_string())
    };
    p.contexts.remove("masking.mask");
    p.contexts.remove("masking.mask.gradient");
    if let Some(name) = chosen {
        p.contexts.insert("masking".into(), vec![name]);
    }
    // the old built-in pages go when nothing shows them any more
    for old in [&old_masking.name, &old_mask.name] {
        let used = p.contexts.values().any(|l| l.contains(old));
        if !used {
            p.pages.retain(|pg| !(pg.auto && pg.name == *old));
        }
    }
    p.sanitize();
    Some(MaskPagesMerged { profile: p.name.clone(), kept })
}

/// Pages that follow the app's tools, shared by every built-in profile, with the contexts they
/// show in.
pub fn tool_pages() -> Vec<(Page, &'static [&'static str])> {
    let both: &[PageGroup] = &[];
    vec![
        (
            // TactileBridge's Crop context: crop orientation, zoom, overlays; the dial straightens
            Page::new(
                "Crop",
                both,
                [
                    ("key.1", cmd("crop.rotateAspect")),
                    ("key.2", cmd("crop.autoStraighten")),
                    ("key.3", cmd("crop.reset")),
                    ("key.4", cmd("view.zoomIn")),
                    ("key.5", cmd("view.zoomOut")),
                    ("key.6", cmd("view.zoomFit")),
                    ("key.7", cmd("view.cropOverlay")),
                    ("key.8", cmd("view.cropOverlayOrientation")),
                    ("key.9", cmd("tool.done")),
                    ("dial", val("crop.angle")),
                    ("roller", val("ui.zoom")),
                ],
            ),
            &["crop"],
        ),
        (
            // the Masks panel: one page (test notes 2026-10-24 note 24). With a mask selected its values adjust that
            // mask; with none selected they set what the next mask is made with (as the panel's sliders do)
            masks_page(),
            &["masking"],
        ),
        (
            // TactileBridge's Draw Mask context: mask actions on the keys, brush size and feather
            // on the dials
            Page::new(
                "Brush",
                both,
                [
                    ("key.1", cmd("mask.invert")),
                    ("key.2", cmd("brush.erase")),
                    ("key.3", cmdp("tool.brush", json!({"new": true}))),
                    // size and feather on keys too: one hand changes them while the other paints
                    ("key.4", val("ui.brushSize")),
                    ("key.5", val("ui.brushFeather")),
                    ("key.6", cmd("mask.visible")),
                    ("key.7", val("ui.brushFlow")),
                    ("key.8", cmd("edit.undo")),
                    ("key.9", cmd("tool.done")),
                    ("dial", val("ui.brushSize")),
                    ("roller", val("ui.brushFeather")),
                ],
            ),
            &["masking.mask.brush"],
        ),
        (
            // TactileBridge's Draw Healing Mask context
            Page::new(
                "Remove",
                both,
                [
                    ("key.1", cmd("view.visualizeSpots")),
                    ("key.2", cmd("view.zoomIn")),
                    ("key.3", cmd("view.zoomOut")),
                    ("key.4", cmd("view.zoomFit")),
                    ("key.5", cmd("view.zoom100")),
                    ("key.6", cmd("spot.delete")),
                    ("key.7", cmd("edit.undo")),
                    ("key.8", cmd("edit.redo")),
                    ("key.9", cmd("tool.done")),
                    ("dial", val("ui.removeSize")),
                    ("roller", val("ui.removeFeather")),
                ],
            ),
            &["remove"],
        ),
        (
            // TactileBridge's Import / Export / Process dialog contexts
            Page::new(
                "Dialog",
                &[PageGroup::Keys],
                [
                    ("key.1", cmd("surface.dialogConfirm")),
                    ("key.3", cmd("surface.dialogCancel")),
                    ("key.4", cmd("library.selectAll")),
                    ("key.6", cmd("library.selectNone")),
                ],
            ),
            &["dialog"],
        ),
        // the Edit panel's sections on the dials (TactileBridge's Direct Color Editor context put
        // colour on the dials): ready to map to `develop.<section>` in Settings > Controls; not
        // mapped by default, so the profile's own dial pages stay in charge while editing
        (lc_dials("Light", "light.exposure", "light.contrast"), &[]),
        (lc_dials("Color", "wb.temp", "wb.tint"), &[]),
        (lc_dials("Effects", "effects.clarity", "effects.texture"), &[]),
        (lc_dials("Detail", "detail.sharpenAmount", "detail.nrLuminance"), &[]),
        (lc_dials("Optics", "optics.distortion", "optics.vignetting"), &[]),
    ]
}

/// The Actions Ring's slots in every built-in profile: undo, redo, pick, reject, previous and
/// next photo, before / after and fit (nothing that deletes or resets).
pub fn ring() -> Vec<(String, Binding)> {
    [
        cmd("edit.undo"),
        cmd("edit.redo"),
        cmd("photo.pick"),
        cmd("photo.reject"),
        cmd("library.previous"),
        cmd("library.next"),
        cmd("view.beforeAfter"),
        cmd("view.zoomFit"),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, b)| (format!("ring.{}", i + 1), b))
    .collect()
}

fn finish(mut p: Profile) -> Profile {
    for (c, b) in ring() {
        p.base.entry(c).or_insert(b);
    }
    let first_keys = p.pages_in(PageGroup::Keys).next().map(|p| p.name.clone());
    let first_dials = p.pages_in(PageGroup::Dials).next().map(|p| p.name.clone());
    p.contexts.insert("default".into(), first_keys.into_iter().chain(first_dials).collect());
    for (page, contexts) in tool_pages() {
        for c in contexts {
            p.contexts.entry((*c).to_string()).or_default().push(page.name.clone());
        }
        // a profile's own page of the same name (Fashion's "Color" dial page) serves instead
        if p.page(&page.name).is_none() {
            p.pages.push(Page { auto: true, ..page });
        }
    }
    p.sanitize();
    p
}

/// The built-in profiles, in TactileBridge's order: the shipped layouts
/// (`data/default-profiles.json`), or [`tactilebridge_profiles`] if that file can't be read.
pub fn profiles() -> Vec<Profile> {
    match serde_json::from_str::<Vec<Profile>>(DEFAULT_PROFILES) {
        Ok(mut ps) if !ps.is_empty() => {
            for p in &mut ps {
                p.sanitize();
            }
            ps
        }
        _ => tactilebridge_profiles(),
    }
}

/// TactileBridge's original layouts, built in code (see the module docs).
pub fn tactilebridge_profiles() -> Vec<Profile> {
    let mut out = Vec::new();

    let mut d = Profile { name: "Default".into(), description: "Browse, rate and edit: TactileBridge's default layout".into(), ..Default::default() };
    d.pages.push(keys(
        "Page 1",
        [
            "PreviousImage",
            "NextImage",
            "ZoomToFit",
            "Zoom100",
            "ToggleBeforeAfter",
            "CopyAdjustments",
            "ApplyAdjustments",
            "AutoAdjust",
            "ResetAdjustments",
        ],
    ));
    d.pages.push(keys(
        "Page 2",
        ["ClearRating", "Rate1Star", "Rate2Stars", "Rate3Stars", "Rate4Stars", "Rate5Stars", "ColorTagRed", "ColorTagGreen", "ColorTagBlue"],
    ));
    d.pages.push(keys(
        "Page 3",
        [
            "SelectTool",
            "CropTool",
            "SpotRemovalTool",
            "WBPickerTool",
            "DrawMaskTool",
            "EraseMaskTool",
            "LinearGradientTool",
            "RadialGradientTool",
            "LoupeTool",
        ],
    ));
    d.pages.push(dials("Dial Page", "Exposure", "Contrast"));
    // TactileBridge's Actions Ring variant turned white balance
    d.pages.push(dials("White Balance", "Kelvin", "Tint"));
    pads(&mut d, ["CopyAdjustments", "ApplyAdjustments", "Redo"]);
    out.push(finish(d));

    let mut w = Profile {
        name: "Wedding".into(),
        description: "Fast culling, white balance, exposure, copy and paste, skin retouching".into(),
        ..Default::default()
    };
    w.pages.push(keys(
        "Cull",
        ["PreviousImage", "NextImage", "ToggleBeforeAfter", "Rate1Star", "Rate3Stars", "Rate5Stars", "ColorTagGreen", "ColorTagRed", "ColorTagNone"],
    ));
    w.pages.push(keys(
        "Edit",
        ["CopyAdjustments", "ApplyAdjustments", "AutoAdjust", "WBPickerTool", "CropTool", "StraightenTool", "Undo", "Redo", "ResetAdjustments"],
    ));
    w.pages.push(keys(
        "Retouch",
        ["SpotRemovalTool", "DrawMaskTool", "EraseMaskTool", "NewEmptyLayer", "NewHealLayer", "ToggleLayerOnOff", "ZoomToFit", "Zoom100", "Process"],
    ));
    w.pages.push(dials("Exposure/WB", "Exposure", "Kelvin"));
    w.pages.push(dials("HDR", "Highlight", "Shadow"));
    pads(&mut w, ["CopyAdjustments", "ApplyAdjustments", "AutoAdjust"]);
    out.push(finish(w));

    let mut po = Profile {
        name: "Portrait".into(),
        description: "Skin tones, white balance, gentle adjustments, black and white".into(),
        ..Default::default()
    };
    po.pages.push(keys(
        "Cull",
        ["PreviousImage", "NextImage", "ToggleBeforeAfter", "Rate1Star", "Rate3Stars", "Rate5Stars", "ColorTagGreen", "ColorTagRed", "ColorTagNone"],
    ));
    po.pages.push(keys(
        "Tone",
        ["CopyAdjustments", "ApplyAdjustments", "AutoAdjust", "WBPickerTool", "CropTool", "RotateRight", "Undo", "Redo", "ResetAdjustments"],
    ));
    po.pages.push(keys(
        "Skin & BW",
        [
            "SpotRemovalTool",
            "DrawHealTool",
            "DirectColorEditor",
            "DrawMaskTool",
            "EraseMaskTool",
            "NewEmptyLayer",
            "BWEnable",
            "ZoomToFit",
            "Process",
        ],
    ));
    po.pages.push(dials("Exposure/WB", "Exposure", "Kelvin"));
    po.pages.push(dials("Tone (dials)", "Contrast", "Saturation"));
    pads(&mut po, ["CopyAdjustments", "ApplyAdjustments", "AutoAdjust"]);
    out.push(finish(po));

    let mut f = Profile {
        name: "Fashion".into(),
        description: "Layer-heavy retouching, colour grading, skin tone, creative colour".into(),
        ..Default::default()
    };
    f.pages.push(keys(
        "Cull",
        ["PreviousImage", "NextImage", "ToggleBeforeAfter", "Rate1Star", "Rate3Stars", "Rate5Stars", "ColorTagGreen", "ColorTagRed", "ColorTagNone"],
    ));
    f.pages.push(keys(
        "Retouch",
        [
            "SpotRemovalTool",
            "DrawHealTool",
            "DrawCloneTool",
            "DrawMaskTool",
            "EraseMaskTool",
            "NewEmptyLayer",
            "NewHealLayer",
            "SelectLayerUp",
            "SelectLayerDown",
        ],
    ));
    f.pages.push(keys(
        "Grade",
        ["Undo", "Redo", "ToggleBeforeAfter", "CropTool", "WBPickerTool", "DirectColorEditor", "CopyAdjustments", "ApplyAdjustments", "Process"],
    ));
    f.pages.push(dials("Exposure", "Exposure", "Contrast"));
    f.pages.push(dials("Color", "Saturation", "Kelvin"));
    f.pages.push(dials("Detail", "Clarity", "SharpeningAmount"));
    pads(&mut f, ["SpotRemovalTool", "DrawMaskTool", "NewEmptyLayer"]);
    out.push(finish(f));

    let mut l = Profile {
        name: "Landscape".into(),
        description: "Highlight and shadow recovery, clarity and texture, graduated masks, vivid colour".into(),
        ..Default::default()
    };
    l.pages.push(keys(
        "Navigate",
        ["PreviousImage", "NextImage", "ZoomToFit", "ToggleBeforeAfter", "Zoom100", "Zoom200", "Rate1Star", "Rate5Stars", "ColorTagGreen"],
    ));
    l.pages.push(keys(
        "Tone",
        ["CopyAdjustments", "ApplyAdjustments", "AutoAdjust", "WBPickerTool", "CropTool", "StraightenTool", "Undo", "Redo", "ResetAdjustments"],
    ));
    l.pages.push(keys(
        "Layers",
        [
            "LinearGradientTool",
            "RadialGradientTool",
            "DrawMaskTool",
            "EraseMaskTool",
            "NewEmptyLayer",
            "ToggleLayerOnOff",
            "SelectLayerUp",
            "SelectLayerDown",
            "Process",
        ],
    ));
    l.pages.push(dials("Exposure/WB", "Exposure", "Kelvin"));
    l.pages.push(dials("HDR", "Highlight", "Shadow"));
    l.pages.push(dials("Detail", "Clarity", "Structure"));
    pads(&mut l, ["CopyAdjustments", "ApplyAdjustments", "AutoAdjust"]);
    out.push(finish(l));

    let mut r = Profile {
        name: "Real Estate".into(),
        description: "Perspective correction, white balance, highlights and shadows, batch work".into(),
        ..Default::default()
    };
    r.pages.push(keys(
        "Navigate",
        ["PreviousImage", "NextImage", "ZoomToFit", "ToggleBeforeAfter", "Rate1Star", "Rate5Stars", "ColorTagGreen", "ColorTagRed", "ColorTagNone"],
    ));
    r.pages.push(keys(
        "Correct",
        [
            "CropTool",
            "StraightenTool",
            "KeystoneTool",
            "KeystoneVertical",
            "KeystoneHorizontal",
            "WBPickerTool",
            "AutoAdjust",
            "RotateLeft",
            "RotateRight",
        ],
    ));
    r.pages.push(keys(
        "Process",
        ["CopyAdjustments", "ApplyAdjustments", "ResetAdjustments", "SelectAll", "EditAllSelected", "Process", "Undo", "Redo", "ToggleExposureWarn"],
    ));
    r.pages.push(dials("Exposure/WB", "Exposure", "Kelvin"));
    r.pages.push(dials("HDR", "Highlight", "Shadow"));
    pads(&mut r, ["CopyAdjustments", "ApplyAdjustments", "AutoAdjust"]);
    out.push(finish(r));

    out
}

/// A fine-steps modifier on a key, for profiles the user builds (offered by the editor).
pub fn fine_modifier() -> Binding {
    Binding::Modifier { modifier: Modifier::Fine }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn six_profiles_every_key_page_full_every_binding_known() {
        let ps = profiles();
        let names: Vec<_> = ps.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Default", "Wedding", "Portrait", "Fashion", "Landscape", "Real Estate"]);
        for p in &ps {
            for page in p.pages.iter().filter(|pg| pg.groups == [PageGroup::Keys] && !pg.auto) {
                // Wedding's Edit page had a key for the author's own preset: left out
                let want = if (p.name.as_str(), page.name.as_str()) == ("Wedding", "Edit") { 8 } else { 9 };
                assert_eq!(page.bindings.len(), want, "{} / {}: every key is mapped", p.name, page.name);
            }
            for b in p.pages.iter().flat_map(|pg| pg.bindings.values()).chain(p.base.values()) {
                if let Binding::Value { target } = b {
                    assert!(crate::targets::target_spec(target).is_some(), "{} → {target}", p.name);
                }
                if let Binding::Command { params, .. } = b {
                    let id = params.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    assert!(!id.starts_with("user."), "{}: no key applies someone's own preset ({id})", p.name);
                }
            }
            let slots = |prefix: &str| p.base.keys().filter(|k| k.starts_with(prefix)).count();
            assert_eq!((slots("pad."), slots("ring.")), (4, 8), "{}: four Dialpad buttons, eight Actions Ring slots", p.name);
            assert!(slots("ld.") > 100, "{}: a Loupedeck+ layout", p.name);
            assert!(p.contexts.contains_key("default") && p.contexts.contains_key("crop") && p.contexts.contains_key("masking.mask.brush"));
            let mut clone = p.clone();
            clone.sanitize();
            assert_eq!(&clone, p, "built-ins are already clean");
        }
        // Real Estate's Keystone keys became Upright
        let re = &ps[5];
        assert_eq!(re.page("Correct").unwrap().bindings["key.3"], Binding::command("tool.guidedUpright"));
    }

    /// A profile as format 3 had it: the "Masking" and "Mask" pages for the Masks panel.
    pub(crate) fn format_3(name: &str) -> Profile {
        let mut p = profiles().into_iter().find(|p| p.name == name).unwrap();
        p.pages.retain(|pg| pg.name != MASKS);
        let (masking, mask) = old_mask_pages();
        p.pages.push(Page { auto: true, ..masking });
        p.pages.push(Page { auto: true, ..mask });
        p.contexts.insert("masking".into(), vec!["Masking".into()]);
        p.contexts.insert("masking.mask".into(), vec!["Mask".into()]);
        p.contexts.insert("masking.mask.gradient".into(), vec!["Mask".into()]);
        p
    }

    /// Decision 2026-10-25 (item 14): key 7 of the built-in Masks page shows / hides the mask overlay (it was Temp);
    /// a profile saved with Temp there keeps it.
    #[test]
    fn masks_page_key_7_is_the_overlay_and_a_saved_layout_keeps_its_own() {
        assert_eq!(masks_page().bindings.get("key.7"), Some(&cmd("view.maskOverlay")));
        let p = profiles().into_iter().next().expect("a built-in profile");
        assert_eq!(p.page(MASKS).and_then(|pg| pg.bindings.get("key.7")), Some(&cmd("view.maskOverlay")));
        // a settings file saved with Temp on key 7 loads as saved
        let mut saved = crate::settings::SurfaceSettings::default();
        for p in &mut saved.profiles {
            if let Some(pg) = p.pages.iter_mut().find(|pg| pg.name == MASKS) {
                pg.bindings.insert("key.7".into(), val("mask.temp"));
            }
        }
        let back = crate::settings::SurfaceSettings::from_json(&saved.to_json()).unwrap();
        let back = back.profiles.first().expect("a profile");
        assert_eq!(back.page(MASKS).and_then(|pg| pg.bindings.get("key.7")), Some(&val("mask.temp")));
    }

    /// Test notes 2026-10-24 note 24: one Masks page for the Masks panel (Brush keeps its own); the old pair is
    /// merged, the user's own changes on either kept ("Mask"'s first).
    #[test]
    fn the_masks_panel_has_one_page_and_old_pairs_merge_keeping_the_users_changes() {
        for p in profiles() {
            assert_eq!(p.contexts.get("masking"), Some(&vec![MASKS.to_string()]), "{}", p.name);
            assert!(!p.contexts.contains_key("masking.mask") && !p.contexts.contains_key("masking.mask.gradient"), "{}", p.name);
            assert!(p.page("Masking").is_none() && p.page("Mask").is_none(), "{}", p.name);
            assert_eq!(p.contexts.get("masking.mask.brush"), Some(&vec!["Brush".to_string()]));
        }
        // the photographer's case: the dial and the roller changed on "Masking", "Mask" left alone
        let mut p = format_3("Wedding");
        let pg = p.pages.iter_mut().find(|pg| pg.name == "Masking").unwrap();
        pg.bindings.insert("dial".into(), Binding::value("mask.temp"));
        pg.bindings.insert("roller".into(), Binding::value("mask.exposure"));
        let merged = merge_mask_pages(&mut p).expect("merged");
        assert_eq!(merged.kept, ["dial", "roller"]);
        let masks = p.page(MASKS).expect("a Masks page");
        assert_eq!(masks.bindings.get("dial"), Some(&Binding::value("mask.temp")));
        assert_eq!(masks.bindings.get("roller"), Some(&Binding::value("mask.exposure")));
        assert_eq!(masks.bindings.get("key.1"), masks_page().bindings.get("key.1"), "the rest as built in");
        assert!(masks.auto);
        assert_eq!(p.contexts.get("masking"), Some(&vec![MASKS.to_string()]));
        assert!(!p.contexts.contains_key("masking.mask") && !p.contexts.contains_key("masking.mask.gradient"));
        assert!(p.page("Masking").is_none() && p.page("Mask").is_none(), "the old pages go");
        // both changed: "Mask"'s wins for the same control
        let mut p = format_3("Default");
        for (page, b) in [("Masking", "mask.temp"), ("Mask", "mask.tint")] {
            p.pages.iter_mut().find(|pg| pg.name == page).unwrap().bindings.insert("dial".into(), Binding::value(b));
        }
        merge_mask_pages(&mut p).unwrap();
        assert_eq!(p.page(MASKS).unwrap().bindings.get("dial"), Some(&Binding::value("mask.tint")));
        // nothing changed: the Masks page as built in; and once merged, nothing more to do
        let mut p = format_3("Portrait");
        assert_eq!(merge_mask_pages(&mut p).unwrap().kept, Vec::<String>::new());
        assert_eq!(p.page(MASKS).unwrap().bindings, masks_page().bindings);
        assert_eq!(merge_mask_pages(&mut p), None);
        // a page of the user's own for a selected mask stays in charge
        let mut p = format_3("Landscape");
        p.pages.push(Page::new("My Mask", &[], [("dial", Binding::value("mask.clarity"))]));
        p.contexts.insert("masking.mask".into(), vec!["My Mask".into()]);
        merge_mask_pages(&mut p).unwrap();
        assert_eq!(p.contexts.get("masking"), Some(&vec!["My Mask".to_string()]));
        assert!(p.page("My Mask").is_some());
    }
}
