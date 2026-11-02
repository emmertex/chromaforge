//! Write presets as Lightroom-style XMP preset files (`preset.exportXmp`), masks included:
//! Subject, Sky, Background and People parts become Lightroom AI masks (`Mask/Image`, which
//! Lightroom recomputes on each photo, as we do), gradients `Mask/Gradient` /
//! `Mask/CircularGradient`, luminance ranges `Mask/RangeMask`, with the `Local*` sliders, the
//! mask's Curve (`MainCurve`, `RedCurve`, …) and Point Color (`LocalPointColors`).
//!
//! What Lightroom has no equivalent for — People by name or role, anchored gradients, Landscape,
//! colour ranges, brushes, object selections, and the global settings (no writer for Lightroom's
//! global `crs:` fields yet) — is listed in the report, and the whole preset is kept in our own
//! namespace (`lc:PresetSettings`, JSON), which Lightroom ignores and we read back: an exported
//! preset imports unchanged.
//!
//! Ported from OneSlips' fork (own design there).

use lcc_develop::{LocalAdjustments, Mask, MaskOp, MaskShape, PointCurves, Preset};
use serde_json::Value;

/// The property holding our own preset in an exported file.
pub const LC_SETTINGS: &str = "lc:PresetSettings";

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            c if (c as u32) < 0x20 && c != '\t' && c != '\n' && c != '\r' => {}
            c => o.push(c),
        }
    }
    o
}

/// A stable 32-hex-digit id from `seed` (Lightroom's UUIDs and sync ids).
fn hex_id(seed: &str) -> String {
    let mut a: u64 = 0xcbf2_9ce4_8422_2325;
    let mut b: u64 = 0x8422_2325_cbf2_9ce4;
    for byte in seed.bytes() {
        a = (a ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        b = (b ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3).rotate_left(17);
    }
    format!("{a:016X}{b:016X}")
}

fn f(v: f64) -> String {
    let v = if v.is_finite() { v } else { 0.0 };
    format!("{v:.6}")
}

/// Lightroom's People part ids (`MaskSubCategoryID`) we know.
fn people_part_id(part: &str) -> Option<u32> {
    Some(match part.to_ascii_lowercase().as_str() {
        "face skin" | "faceskin" => 2,
        "iris and pupil" | "iris" => 3,
        "body skin" | "bodyskin" => 4,
        "hair" => 5,
        "lips" => 6,
        "eye sclera" | "sclera" => 8,
        "clothes" | "clothing" => 11,
        "teeth" => 12,
        _ => return None,
    })
}

/// Our 0..1 range scale (log luminance on −8..+4 EV) → Lightroom's lightness 0..1.
fn range_to_lightness(r: f64) -> f64 {
    let y = (r.clamp(0.0, 1.0) * 12.0 - 8.0).exp2();
    y.powf(1.0 / 2.2).clamp(0.0, 1.0)
}

/// The `Local*` attributes of a mask's adjustments (the importer's table, read backwards).
fn local_attrs(a: &LocalAdjustments) -> String {
    let ours = serde_json::to_value(a).unwrap_or_default();
    let mut s = String::new();
    let mut done: Vec<&str> = Vec::new();
    for (crs, field, scale) in crate::crs_masks::local_fields() {
        if done.contains(&field) {
            continue;
        }
        done.push(field);
        let v = ours.get(field).and_then(Value::as_f64).unwrap_or(0.0);
        s.push_str(&format!("\n       crs:{crs}=\"{}\"", f(v / scale)));
    }
    s
}

/// A local curve channel as Lightroom's `"x, y"` points on 0..255 (`None` while straight).
fn curve_seq(tag: &str, pts: &[lcc_geom::Point]) -> Option<String> {
    if pts.len() < 2 {
        return None;
    }
    let items: Vec<String> = pts
        .iter()
        .take(64)
        .map(|p| {
            let c = |v: f64| if v.is_finite() { (v.clamp(0.0, 1.0) * 255.0).round() } else { 0.0 };
            format!("\n         <rdf:li>{}, {}</rdf:li>", c(p.x), c(p.y))
        })
        .collect();
    Some(format!("\n      <crs:{tag}>\n       <rdf:Seq>{}\n       </rdf:Seq>\n      </crs:{tag}>", items.concat()))
}

/// The mask's Curve and Point Color as child elements of its correction.
fn local_children(m: &Mask) -> String {
    let PointCurves { master, red, green, blue } = &m.curve;
    let mut s: String = [("MainCurve", master), ("RedCurve", red), ("GreenCurve", green), ("BlueCurve", blue)]
        .into_iter()
        .filter_map(|(tag, pts)| curve_seq(tag, pts))
        .collect();
    if !m.point_colors.is_empty() {
        let (lines, vars): (Vec<String>, Vec<String>) = m
            .point_colors
            .iter()
            .map(|p| {
                let (lr, var) = crate::crs_point_color::from_ours(p);
                (format!("\n         <rdf:li>{}</rdf:li>", crate::crs_point_color::format(&lr)), format!("\n         <rdf:li>{}</rdf:li>", f(var)))
            })
            .unzip();
        s.push_str(&format!("\n      <crs:LocalPointColors>\n       <rdf:Seq>{}\n       </rdf:Seq>\n      </crs:LocalPointColors>", lines.concat()));
        s.push_str(&format!(
            "\n      <crs:LocalColorVariance>\n       <rdf:Seq>{}\n       </rdf:Seq>\n      </crs:LocalColorVariance>",
            vars.concat()
        ));
    }
    s
}

/// One mask component as a `CorrectionMasks` item; `Err` names what Lightroom can't hold.
fn component(m: &Mask, k: usize, aspect: f64, notes: &mut Vec<String>) -> Option<String> {
    let c = m.components.get(k)?;
    let (mode, inverted) = match c.op {
        MaskOp::Add => (0, c.invert),
        MaskOp::Subtract => (1, c.invert),
        // Lightroom writes "intersect with" as subtracting the inverted component
        MaskOp::Intersect => (1, !c.invert),
    };
    if c.anchor.is_some() {
        notes.push(format!("{}: the gradient follows the people on each photo here; Lightroom gets it fixed on the photo", m.name));
    }
    let sync = hex_id(&format!("{}-{}-{k}", m.name, m.id));
    let head = |what: &str, name: &str| {
        format!(
            "<rdf:li\n         crs:What=\"{what}\"\n         crs:MaskActive=\"true\"\n         crs:MaskName=\"{}\"\n         crs:MaskBlendMode=\"{mode}\"\n         crs:MaskInverted=\"{}\"\n         crs:MaskSyncID=\"{sync}\"\n         crs:MaskValue=\"1\"",
            esc(name),
            if inverted { "true" } else { "false" }
        )
    };
    let image = |sub: u32, cat: Option<u32>, name: &str| {
        let cat = cat.map(|c| format!("\n         crs:MaskSubCategoryID=\"{c}\"")).unwrap_or_default();
        format!(
            "{}\n         crs:MaskVersion=\"1\"\n         crs:MaskSubType=\"{sub}\"{cat}\n         crs:ReferencePoint=\"0.500000 0.500000\"\n         crs:ErrorReason=\"0\"/>",
            head("Mask/Image", name)
        )
    };
    let (sx, sy) = if aspect >= 1.0 { (1.0, 1.0 / aspect) } else { (aspect, 1.0) };
    Some(match &c.shape {
        MaskShape::Subject => image(1, None, "Subject 1"),
        MaskShape::Sky => image(2, None, "Sky 1"),
        MaskShape::Background => image(0, Some(22), "Background 1"),
        MaskShape::People { person, parts, who, .. } => {
            if who.is_some() || *person > 0 {
                notes.push(format!(
                    "{}: Lightroom selects every person, not {}",
                    m.name,
                    who.as_ref().map_or_else(|| format!("person {person}"), |w| w.label())
                ));
            }
            let parts: Vec<&String> = if parts.is_empty() { Vec::new() } else { parts.iter().collect() };
            let ids: Vec<(u32, &String)> = parts.iter().filter_map(|p| people_part_id(p).map(|i| (i, *p))).collect();
            if ids.len() != parts.len() || parts.is_empty() {
                notes.push(format!("{}: People parts Lightroom's presets don't name (entire person, eyebrows) are left out", m.name));
            }
            // one Lightroom component per part
            let items: Vec<String> = ids.iter().map(|(i, p)| image(3, Some(*i), p)).collect();
            if items.is_empty() {
                return None;
            }
            items.join("\n        ")
        }
        MaskShape::Linear { start, end } => format!(
            "{}\n         crs:ZeroX=\"{}\"\n         crs:ZeroY=\"{}\"\n         crs:FullX=\"{}\"\n         crs:FullY=\"{}\"/>",
            head("Mask/Gradient", "Linear Gradient 1"),
            f(end.x),
            f(end.y),
            f(start.x),
            f(start.y)
        ),
        MaskShape::Radial { center, rx, ry, angle, feather, invert } => {
            let (hw, hh) = (rx / sx, ry / sy);
            format!(
                "{}\n         crs:Top=\"{}\"\n         crs:Left=\"{}\"\n         crs:Bottom=\"{}\"\n         crs:Right=\"{}\"\n         crs:Angle=\"{}\"\n         crs:Midpoint=\"50\"\n         crs:Roundness=\"0\"\n         crs:Feather=\"{}\"\n         crs:Flipped=\"{}\"/>",
                head("Mask/CircularGradient", "Radial Gradient 1"),
                f(center.y - hh),
                f(center.x - hw),
                f(center.y + hh),
                f(center.x + hw),
                f(*angle),
                f(feather.clamp(0.0, 100.0)),
                if *invert { "true" } else { "false" }
            )
        }
        MaskShape::LuminanceRange { lo, hi, lo_feather, hi_feather } => {
            let q = [lo - lo_feather, *lo, *hi, hi + hi_feather].map(|v| f(range_to_lightness(v)));
            format!(
                "<rdf:li>\n        <rdf:Description\n         crs:What=\"Mask/RangeMask\"\n         crs:MaskActive=\"true\"\n         crs:MaskName=\"Luminance Range 1\"\n         crs:MaskBlendMode=\"{mode}\"\n         crs:MaskInverted=\"{}\"\n         crs:MaskSyncID=\"{sync}\"\n         crs:MaskValue=\"1\">\n        <crs:CorrectionRangeMask\n         crs:Version=\"3\"\n         crs:Type=\"2\"\n         crs:Invert=\"false\"\n         crs:LumRange=\"{} {} {} {}\"/>\n        </rdf:Description>\n        </rdf:li>",
                if inverted { "true" } else { "false" },
                q[0],
                q[1],
                q[2],
                q[3]
            )
        }
        other => {
            let what = match other {
                MaskShape::Landscape { class, .. } => format!("Landscape ({class})"),
                MaskShape::ColorRange { .. } => "a colour range".into(),
                MaskShape::Brush { .. } => "a brush".into(),
                MaskShape::Prompt { text, .. } => format!("the description “{text}”"),
                MaskShape::Object { .. } => "an object selection".into(),
                MaskShape::DepthRange { .. } => "a depth range".into(),
                _ => "a component".into(),
            };
            notes.push(format!("{}: {what} has no Lightroom equivalent in presets and is left out", m.name));
            return None;
        }
    })
}

/// Preset `p` as a Lightroom XMP preset file, and notes on what Lightroom won't get.
pub fn to_xmp(p: &Preset) -> (String, Vec<String>) {
    let mut notes = Vec::new();
    let aspect = lcc_develop::recipe::RECIPE_ASPECT;
    let masks: Vec<Mask> = p
        .settings
        .get("masks")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|m| serde_json::from_value(m.clone()).ok()).collect())
        .unwrap_or_default();
    if p.settings.as_object().is_some_and(|o| o.keys().any(|k| k != "masks")) {
        notes.push("Global settings are kept for this app only (Lightroom gets the masks)".into());
    }
    let mut corrections = String::new();
    for m in &masks {
        let items: Vec<String> = (0..m.components.len()).filter_map(|k| component(m, k, aspect, &mut notes)).collect();
        if items.is_empty() {
            continue;
        }
        corrections.push_str(&format!(
            "\n     <rdf:li>\n      <rdf:Description\n       crs:What=\"Correction\"\n       crs:CorrectionAmount=\"{}\"\n       crs:CorrectionActive=\"{}\"\n       crs:CorrectionName=\"{}\"\n       crs:CorrectionSyncID=\"{}\"{}>\n      <crs:CorrectionMasks>\n       <rdf:Seq>\n        {}\n       </rdf:Seq>\n      </crs:CorrectionMasks>{}\n      </rdf:Description>\n     </rdf:li>",
            f((m.adjust.amount / 100.0).clamp(0.0, 2.0)),
            if m.visible { "true" } else { "false" },
            esc(&m.name),
            hex_id(&format!("{}-{}", p.id, m.id)),
            local_attrs(&m.adjust),
            items.join("\n        "),
            local_children(m)
        ));
        if m.invert || m.feather > 0.0 || m.refine > 0.0 {
            notes.push(format!("{}: the mask's Invert, Feather and Refine Edges stay here; Lightroom gets the mask without them", m.name));
        }
    }
    let mask_block = if corrections.is_empty() {
        String::new()
    } else {
        format!("\n   <crs:MaskGroupBasedCorrections>\n    <rdf:Seq>{corrections}\n    </rdf:Seq>\n   </crs:MaskGroupBasedCorrections>")
    };
    let lc = serde_json::to_string(&p.settings).unwrap_or_default();
    let alt = |tag: &str, v: &str| {
        format!("\n   <crs:{tag}>\n    <rdf:Alt>\n     <rdf:li xml:lang=\"x-default\">{}</rdf:li>\n    </rdf:Alt>\n   </crs:{tag}>", esc(v))
    };
    let xmp = format!(
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  <rdf:Description rdf:about=\"\"\n    xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n    xmlns:lc=\"{}\"\n   crs:PresetType=\"Normal\"\n   crs:UUID=\"{}\"\n   crs:SupportsAmount2=\"True\"\n   crs:SupportsAmount=\"True\"\n   crs:SupportsColor=\"True\"\n   crs:SupportsMonochrome=\"True\"\n   crs:SupportsHighDynamicRange=\"True\"\n   crs:SupportsNormalDynamicRange=\"True\"\n   crs:SupportsSceneReferred=\"True\"\n   crs:SupportsOutputReferred=\"True\"\n   crs:RequiresRGBTables=\"False\"\n   crs:Version=\"16.4\"\n   crs:ProcessVersion=\"15.4\"\n   crs:HasSettings=\"True\"\n   {LC_SETTINGS}=\"{}\">{}{}{}\n  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n",
        lcc_meta::LC_NS,
        hex_id(&p.id),
        esc(&lc),
        alt("Name", &p.name),
        alt("Group", &p.group),
        mask_block
    );
    notes.sort();
    notes.dedup();
    (xmp, notes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn preset() -> Preset {
        let masks = json!([
            {"id": 1, "name": "Sun & \"warm\"", "components": [
                {"op": "add", "shape": {"kind": "background"}},
                {"op": "subtract", "anchor": {"to": "people", "shape": {"kind": "linear", "start": {"x": 0.5, "y": 1.0}, "end": {"x": 0.5, "y": 0.55}}},
                 "shape": {"kind": "linear", "start": {"x": 0.5, "y": 1.0}, "end": {"x": 0.5, "y": 0.55}}}],
             "adjust": {"temp": 18.0, "exposure": 0.4, "amount": 100.0}},
            {"id": 2, "name": "Suit", "components": [
                {"op": "add", "shape": {"kind": "people", "person": 0, "parts": ["Clothes"], "who": {"role": "Groom"}}},
                {"op": "intersect", "shape": {"kind": "luminanceRange", "lo": 0.2, "hi": 0.6, "lo_feather": 0.1, "hi_feather": 0.1}},
                {"op": "add", "shape": {"kind": "prompt", "text": "Wedding dress"}}],
             "adjust": {"saturation": 30.0, "amount": 100.0},
             "curve": {"master": [{"x": 0.0, "y": 0.0}, {"x": 0.5, "y": 0.6}, {"x": 1.0, "y": 1.0}]},
             "point_colors": [{"lum": 0.7, "chroma": 0.08, "hue": 60.0, "hue_shift": 10.0, "sat_shift": -20.0, "lum_shift": 5.0}]}
        ]);
        Preset {
            id: "user.t".into(),
            name: "Sun <kissed>".into(),
            group: "Smart".into(),
            settings: json!({"masks": masks, "light": {"contrast": 10.0}}),
            favorite: false,
            builtin: false,
        }
    }

    #[test]
    fn exported_presets_read_back_in_lightroom_terms_and_in_full() {
        let (xmp, notes) = to_xmp(&preset());
        let d = lcc_meta::parse_xmp(&xmp).expect("well-formed XMP");
        // Lightroom's view: the crs: masks, as our own importer reads them
        let (masks, unmapped) = crate::crs_masks::masks(&d.values, 1.5);
        assert!(unmapped.is_empty(), "{unmapped:?}");
        assert_eq!(masks.len(), 2);
        assert_eq!(masks[0]["components"][0]["shape"]["kind"], "background");
        assert_eq!(masks[0]["components"][1]["op"], "subtract");
        assert!((masks[0]["adjust"]["exposure"].as_f64().unwrap() - 0.4).abs() < 1e-6);
        assert_eq!(masks[1]["components"][0]["shape"]["parts"], json!(["Clothes"]));
        // intersect survives as Lightroom writes it (subtract the inverted range)
        assert_eq!((masks[1]["components"][1]["op"].as_str(), masks[1]["components"][1]["invert"].as_bool()), (Some("subtract"), Some(true)));
        let lo = masks[1]["components"][1]["shape"]["lo"].as_f64().unwrap();
        assert!((lo - 0.2).abs() < 0.01, "luminance range round trip: {lo}");
        // the mask's Curve and Point Color travel with it
        let master = masks[1]["curve"]["master"].as_array().expect("local curve");
        assert_eq!(master.len(), 3);
        assert!((master[1]["y"].as_f64().unwrap() - 0.6).abs() < 0.01, "{master:?}");
        let pc = masks[1]["point_colors"].as_array().expect("point colours");
        assert_eq!(pc.len(), 1);
        assert!((pc[0]["hue_shift"].as_f64().unwrap() - 10.0).abs() < 1.0, "{pc:?}");
        // what Lightroom can't hold is said
        assert!(notes.iter().any(|n| n.contains("Groom")), "{notes:?}");
        assert!(notes.iter().any(|n| n.contains("Wedding dress")), "{notes:?}");
        assert!(notes.iter().any(|n| n.contains("Global settings")), "{notes:?}");
        // our own: the whole preset
        let lc = d.properties.get(LC_SETTINGS).and_then(|v| v.first()).expect("lc settings");
        let back: Value = serde_json::from_str(lc).unwrap();
        assert_eq!(back, preset().settings);
        assert!(xmp.contains("Sun &lt;kissed&gt;"));
    }
}

#[cfg(test)]
mod hostile {
    use super::*;
    use serde_json::json;

    #[test]
    fn hostile_presets_export_without_panicking() {
        for settings in [
            json!({"masks": "nope"}),
            json!({"masks": [{"name": "<&>", "components": [{"shape": {"kind": "radial", "center": {"x": 1e308, "y": -1e308}, "rx": 0.0, "ry": 0.0, "angle": 0.0, "feather": 1e9, "invert": true}}]}]}),
            json!({"masks": [{"curve": {"master": [{"x": -5.0, "y": 9.0}, {"x": 2.0, "y": -1.0}]}, "components": [{"shape": {"kind": "sky"}}]}]}),
            json!(null),
        ] {
            let p = Preset { id: "x".into(), name: "\u{0}bad\u{7}".into(), group: String::new(), settings, favorite: false, builtin: false };
            let (xmp, _) = to_xmp(&p);
            assert!(lcc_meta::parse_xmp(&xmp).is_ok(), "{xmp}");
        }
    }
}
