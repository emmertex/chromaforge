//! The Witchcraft grading mode's palettes (DaVinci Resolve style colour grading, beyond
//! Lightroom): the colourist primaries — Lift, Gamma, Gain and Offset wheels, each with its
//! master wheel beneath — in the Color variant (wide tonal overlap) and the Log variant (narrow
//! ranges, `primaries.low`/`high`). The wheels drag the engine's own `primaries.*` controls, so
//! everything else (history, presets, XMP, MCP, the GPU) sees one edit like any other.

use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, pos2, vec2};
use lcc_develop::DevelopSettings;
use serde_json::json;

use super::edit::{
    WHEEL_CAPTURE, WheelCapture, WheelGrab, WheelLock, WheelMods, control_as, give_pointer_back, paint_wheel, wheel_pick, wheel_pointer,
};
use crate::LccApp;
use crate::theme::Tokens;
use crate::widgets::register;

/// The wheels, in the palette's order: the key names the `primaries.<key>.*` controls; the label
/// is the Color variant's (the Log variant renames the first three).
const WHEELS: [(&str, &str, &str); 4] =
    [("lift", "Lift", "Shadows"), ("gamma", "Gamma", "Midtones"), ("gain", "Gain", "Highlights"), ("offset", "Offset", "Offset")];

/// The node strip at the head of the Witchcraft panel: the grade's stages as Resolve lays a node
/// tree out — the panels' own sections in the grade's order, the photo's masks in the middle, the
/// output at the end. Presentation only (the pipeline's order is the renderer's): a click opens a
/// node's panel, its number badge bypasses the stage (the section's eye / the mask's visibility).
pub(crate) fn node_strip(app: &mut LccApp, ui: &mut egui::Ui, d: &DevelopSettings) {
    let t = Tokens::get(ui.ctx());
    /// A strip node: the section it opens (`output`: none) and whether its stage contributes.
    struct Node {
        id: String,
        label: String,
        on: bool,
        mask: Option<u32>,
    }
    let mut nodes: Vec<Node> =
        [("light", "Balance"), ("primaries", "Primaries"), ("hsl", "HSL Curves"), ("curve", "Curve"), ("mixer", "Color"), ("grading", "Grade")]
            .iter()
            .map(|(id, label)| Node { id: (*id).into(), label: crate::i18n::tr(label).to_string(), on: d.section_enabled(id), mask: None })
            .collect();
    for m in &d.masks {
        let name = if m.name.is_empty() { format!("{} {}", crate::i18n::tr("Mask"), m.id) } else { m.name.clone() };
        nodes.push(Node { id: format!("mask{}", m.id), label: name, on: m.visible, mask: Some(m.id) });
    }
    nodes.push(Node { id: "effects".into(), label: crate::i18n::tr("Effects").to_string(), on: d.section_enabled("effects"), mask: None });
    nodes.push(Node { id: "output".into(), label: crate::i18n::tr("Output").to_string(), on: true, mask: None });
    egui::Frame::NONE.inner_margin(crate::widgets::row_margin(ui, egui::Margin { left: 12, right: 16, top: 6, bottom: 2 }, 3, 2)).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
            for (i, node) in nodes.iter().enumerate() {
                let (r, _) = ui.allocate_exact_size(vec2(64.0, 28.0), Sense::click());
                register(ui.ctx(), format!("node:{}", node.id), r);
                let terminal = node.id == "output";
                let resp = ui.interact(r, egui::Id::new(("node", &node.id)), Sense::click());
                let resp = if terminal {
                    resp
                } else {
                    resp.on_hover_text(crate::i18n::tr("Click a node to open its panel; click its number to bypass that stage."))
                };
                let p = ui.painter();
                let fill = if node.on { t.button } else { Color32::from_gray(24) };
                p.rect(r, 4.0, fill, Stroke::new(1.0, if node.on { t.button_border } else { Color32::from_gray(40) }), egui::StrokeKind::Inside);
                // the connector to the node before it
                if i > 0 {
                    let prev = r.left_top() + vec2(-8.0, r.height() / 2.0);
                    p.line_segment([prev, r.left_top() + vec2(0.0, r.height() / 2.0)], Stroke::new(1.0, t.button_border));
                }
                // the label, cut to the node (a long mask name ends in an ellipsis)
                let color = if node.on { t.text_label } else { t.text_disabled };
                let font = t.font(10.5);
                let max_w = r.width() - 8.0;
                let mut label = node.label.clone();
                let mut galley = p.layout_no_wrap(label.clone(), font.clone(), color);
                if galley.size().x > max_w {
                    while !label.is_empty() {
                        label.pop();
                        galley = p.layout_no_wrap(format!("{label}…"), font.clone(), color);
                        if galley.size().x <= max_w {
                            break;
                        }
                    }
                }
                let text_pos = pos2(r.center().x - galley.size().x / 2.0, r.center().y - galley.size().y / 2.0);
                p.galley(text_pos, galley, Color32::WHITE);
                // the numbered badge: bypass the stage
                let badge = Rect::from_center_size(r.left_top() + vec2(2.0, 2.0), vec2(15.0, 15.0));
                let bc = badge.center();
                p.circle_filled(bc, 8.0, if node.on { t.pressed } else { Color32::from_gray(40) });
                p.circle_stroke(bc, 8.0, Stroke::new(1.0, t.button_border));
                p.text(bc, Align2::CENTER_CENTER, format!("{:02}", i + 1), t.font(9.0), t.text_label);
                register(ui.ctx(), format!("nodeBadge:{}", node.id), badge);
                if !terminal {
                    let br = ui.interact(badge, egui::Id::new(("nodeBadge", &node.id)), Sense::click());
                    let br = br.on_hover_text(crate::i18n::tr(if node.on { "Bypass this stage" } else { "Include this stage" }));
                    if br.clicked() {
                        match node.mask {
                            Some(id) => {
                                let _ = app.run("mask.visible", json!({"id": id}));
                            }
                            None => {
                                let _ = app.run("develop.sectionEnabled", json!({"section": &node.id, "enabled": !node.on}));
                            }
                        }
                    } else if resp.clicked() && node.mask.is_none() {
                        let _ = app.run(format!("section.{}", node.id).as_str(), json!({}));
                    }
                }
            }
        });
    });
}

/// The Primaries palette: the Color | Log variant switch, the four wheels in two rows, the master
/// wheel of each under its column, and the Log variant's range boundaries.
pub(crate) fn primaries(app: &mut LccApp, ui: &mut egui::Ui, d: &DevelopSettings) {
    // Color (the default) or Log, as the palette's own segmented switch (Resolve's Primaries palette)
    egui::Frame::NONE.inner_margin(crate::widgets::row_margin(ui, egui::Margin { left: 12, right: 16, top: 6, bottom: 2 }, 3, 2)).show(ui, |ui| {
        ui.horizontal(|ui| {
            let active = usize::from(d.primaries.log);
            if let Some(i) = crate::widgets::segmented(ui, "primariesVariant", &[("Color", "color"), ("Log", "log")], Some(active), 2) {
                let _ = app.run("develop.primaries", json!({"log": i == 1}));
            }
        });
    });
    // four wheels in two rows, the master wheels under their columns (as the 3-Way grading lays
    // its wheels and luminance sliders out)
    let w = ui.available_width();
    let rad = 44.0;
    let (area, _) = ui.allocate_exact_size(vec2(w, rad * 4.0 + 64.0), Sense::hover());
    let at = [
        pos2(area.left() + w * 0.28, area.top() + rad + 8.0),
        pos2(area.left() + w * 0.72, area.top() + rad + 8.0),
        pos2(area.left() + w * 0.28, area.top() + rad * 3.0 + 36.0),
        pos2(area.left() + w * 0.72, area.top() + rad * 3.0 + 36.0),
    ];
    let labels: Vec<&str> = WHEELS.iter().map(|(_, color, log)| if d.primaries.log { *log } else { *color }).collect();
    for ((key, ..), c) in WHEELS.iter().zip(at) {
        wheel(app, ui, d, key, c, rad);
    }
    for ((key, ..), label) in WHEELS.iter().zip(&labels) {
        control_as(app, ui, d, &format!("primaries.{key}.lum"), true, Some(label));
    }
    // the Log variant's range boundaries (they move where the Shadow and Highlight wheels stop)
    if d.primaries.log {
        control_as(app, ui, d, "primaries.low", true, Some("Low Range"));
        control_as(app, ui, d, "primaries.high", true, Some("High Range"));
    }
    ui.add_space(8.0);
}

/// One colourist wheel (`key` of [`WHEELS`]) centred on `c`: drag to pick hue and saturation,
/// double-click to reset the puck — the same holds and readouts as the Color Grading wheels.
fn wheel(app: &mut LccApp, ui: &mut egui::Ui, d: &DevelopSettings, key: &str, c: Pos2, rad: f32) {
    let t = Tokens::get(ui.ctx());
    let wh = match key {
        "lift" => d.primaries.lift,
        "gamma" => d.primaries.gamma,
        "gain" => d.primaries.gain,
        _ => d.primaries.offset,
    };
    let label = WHEELS.iter().find(|(k, ..)| *k == key).map_or(key, |(_, l, _)| *l);
    let log_label = WHEELS.iter().find(|(k, ..)| *k == key).map_or(key, |(_, _, l)| *l);
    let r = Rect::from_center_size(c, vec2(rad * 2.0, rad * 2.0));
    let resp = ui.interact(r, egui::Id::new(("primariesWheel", key)), Sense::click_and_drag());
    crate::widgets::announce(&resp, format!("{} {}", crate::i18n::tr(label), crate::i18n::tr("Color wheel")));
    let resp = resp.on_hover_text(crate::i18n::tr(
        "Drag to set hue and saturation. Hold Shift to keep the hue, Cmd/Ctrl to keep the saturation, Alt/Option to fine-adjust. Double-click to reset.",
    ));
    register(ui.ctx(), format!("wheel:primaries-{key}"), r);
    paint_wheel(ui.painter(), c, rad);
    let a = (wh.hue as f32).to_radians();
    let dot = c + vec2(a.cos(), -a.sin()) * rad * (wh.sat as f32 / 100.0);
    // the puck's modifiers and mid-drag re-grab, as the Color Grading wheels
    let grab_id = egui::Id::new(("primariesWheelGrab", key));
    let mods = ui.input(|i| WheelMods::from_modifiers(i.modifiers));
    let pointer = wheel_pointer(ui, &resp);
    let grab = match pointer {
        Some(q) if resp.dragged() || resp.clicked() => {
            let held = ui.data(|m| m.get_temp::<WheelGrab>(grab_id)).filter(|g| g.mods == mods && !resp.drag_started());
            Some(held.unwrap_or_else(|| {
                let origin = if resp.drag_started() { ui.input(|i| i.pointer.press_origin()).unwrap_or(q) } else { q };
                let g = WheelGrab::new(mods, wh.hue as f32, wh.sat as f32, origin - c, rad);
                ui.data_mut(|m| m.insert_temp(grab_id, g));
                g
            }))
        }
        _ => {
            ui.data_mut(|m| m.remove::<WheelGrab>(grab_id));
            None
        }
    };
    let guide = Stroke::new(1.5, Color32::from_white_alpha(170));
    match grab.as_ref().and_then(|g| g.lock()) {
        Some(WheelLock::Hue(h)) => {
            let a = h.to_radians();
            ui.painter().line_segment([c, c + vec2(a.cos(), -a.sin()) * rad], guide);
        }
        Some(WheelLock::Sat(s)) => {
            ui.painter().circle_stroke(c, rad * s / 100.0, guide);
        }
        None => {}
    }
    ui.painter().circle_filled(c, 2.0, Color32::WHITE);
    ui.painter().circle_filled(dot, 5.0, Color32::WHITE);
    ui.painter().circle_stroke(dot, 5.0, Stroke::new(1.0, Color32::BLACK));
    ui.painter().text(
        pos2(c.x, c.y + rad + 12.0),
        Align2::CENTER_CENTER,
        crate::i18n::tr(if d.primaries.log { log_label } else { label }),
        t.font(12.0),
        t.text_label,
    );
    if resp.drag_started() {
        let _ = app.run("develop.beginInteraction", json!({"label": crate::i18n::tr(if d.primaries.log { log_label } else { label })}));
    }
    if (resp.dragged() || resp.clicked())
        && let Some(q) = pointer
    {
        let (hue, sat) = grab.map_or_else(|| wheel_pick(q - c, rad, None), |g| g.pick(q - c, rad));
        let _ = app.run("develop.set", json!({"values": {format!("primaries.{key}.hue"): hue.round(), format!("primaries.{key}.sat"): sat.round()}}));
    }
    if resp.drag_stopped() || resp.clicked() {
        let _ = app.run("develop.endInteraction", json!({}));
    }
    if resp.drag_stopped()
        && let Some(held) = ui.ctx().data_mut(|m| m.remove_temp::<WheelCapture>(egui::Id::new(WHEEL_CAPTURE)))
        && !held.dropped
    {
        give_pointer_back(ui.ctx(), held.origin);
    }
    if resp.double_clicked() {
        let _ = app.run("develop.set", json!({"values": {format!("primaries.{key}.hue"): 0, format!("primaries.{key}.sat"): 0}}));
    }
}

/// Resolve's HSL curves — "the first word prompts the selection, and the second word affects the
/// change" — as free points anywhere over the hue circle (colour photos; the B&W mix has its own
/// panel): click to add a point and drag it where the edit belongs; the monotone curve through the
/// points (neutral at both ends of the hue axis) is what renders, the same spline the engine's LUTs
/// sample. A double-click removes the point under it. `develop.hslCurves` writes the curve's whole
/// point list, one drag one undo step.
pub(crate) fn hsl_curves(app: &mut LccApp, ui: &mut egui::Ui, d: &DevelopSettings) {
    let t = Tokens::get(ui.ctx());
    const CURVES: [(&str, &str); 3] = [("Hue vs Hue", "hue"), ("Hue vs Sat", "sat"), ("Hue vs Lum", "lum")];
    let channel = if CURVES.iter().any(|(_, k)| *k == app.ui.hsl_curve) { app.ui.hsl_curve.clone() } else { "hue".into() };
    egui::Frame::NONE.inner_margin(crate::widgets::row_margin(ui, egui::Margin { left: 12, right: 16, top: 6, bottom: 2 }, 3, 2)).show(ui, |ui| {
        ui.horizontal(|ui| {
            let active = CURVES.iter().position(|(_, k)| *k == channel);
            if let Some((_, k)) = crate::widgets::segmented(ui, "hslCurves", &CURVES, active, 3).and_then(|i| CURVES.get(i)) {
                app.ui.hsl_curve = k.to_string();
            }
        });
    });
    let points: Vec<lcc_geom::Point> = d.hsl_curves.channel(&channel).cloned().unwrap_or_default();
    let w = ui.available_width();
    let (r, resp) = ui.allocate_exact_size(vec2(w, 128.0), Sense::click_and_drag());
    let resp = resp.on_hover_text(crate::i18n::tr("Click to add a point and drag it where the edit belongs. Double-click a point to remove it."));
    crate::widgets::announce(&resp, crate::i18n::tr("HSL Curves"));
    register(ui.ctx(), "curve:hsl", r);
    let p = ui.painter();
    let plot = Rect::from_min_max(r.min + vec2(4.0, 4.0), r.max - vec2(4.0, 16.0));
    p.rect_filled(plot, 4.0, t.button);
    let at = |x: f32| lcc_pipeline::colorops::hsl_curve_at(&points, x as f64) as f32;
    let y_of = |v: f32| plot.bottom() - (v.clamp(-1.0, 1.0) + 1.0) / 2.0 * plot.height();
    let x_of = |x: f32| plot.left() + x * plot.width();
    // the neutral line and ±0.5 guides
    for (v, strong) in [(0.0, true), (0.5, false), (-0.5, false)] {
        let y = y_of(v);
        let color = if strong { Color32::from_white_alpha(60) } else { Color32::from_white_alpha(22) };
        p.line_segment([pos2(plot.left(), y), pos2(plot.right(), y)], egui::Stroke::new(1.0, color));
    }
    // the hue strip under the axis: each cell the colour of its own OkLCh hue, so the curve's
    // x axis is the hue it edits
    let cells = 48;
    for k in 0..cells {
        let x0 = k as f32 / cells as f32;
        let lab = lcc_color::perceptual::lch_to_lab([0.62, 0.13, x0 * std::f32::consts::TAU]);
        let wide = lcc_color::perceptual::oklab_to_2020(lab);
        let srgb = lcc_color::REC2020.to_space(&lcc_color::SRGB).apply_f32(wide).map(|v| (v.clamp(0.0, 1.0) * 255.0) as u8);
        p.rect_filled(
            Rect::from_min_max(pos2(x_of(x0), plot.bottom() + 2.0), pos2(x_of((k + 1) as f32 / cells as f32), r.bottom())),
            0.0,
            Color32::from_rgb(srgb[0], srgb[1], srgb[2]),
        );
    }
    // the curve, sampled exactly as the engine's LUT samples it
    let curve: Vec<Pos2> = (0..=64).map(|k| pos2(x_of(k as f32 / 64.0), y_of(at(k as f32 / 64.0)))).collect();
    p.add(egui::Shape::line(curve, egui::Stroke::new(1.5, t.text_label)));
    // the points themselves
    let pos_of = |q: &lcc_geom::Point| pos2(x_of(q.x as f32), y_of(q.y as f32));
    for (i, q) in points.iter().enumerate() {
        let c = pos_of(q);
        p.circle_filled(c, 4.5, Color32::WHITE);
        p.circle_stroke(c, 4.5, egui::Stroke::new(1.0, Color32::BLACK));
        register(ui.ctx(), format!("hslPoint:{channel}-{i}"), Rect::from_center_size(c, vec2(14.0, 14.0)));
    }
    // the pointer's curve coordinates
    let to_point = |q: Pos2| {
        lcc_geom::Point::new(
            ((q.x - plot.left()) / plot.width()).clamp(0.0, 1.0) as f64,
            ((plot.bottom() - q.y) / plot.height() * 2.0 - 1.0).clamp(-1.0, 1.0) as f64,
        )
    };
    let nearest = |q: Pos2| {
        points
            .iter()
            .enumerate()
            .min_by(|(i, a), (j, b)| pos_of(a).distance(q).total_cmp(&pos_of(b).distance(q)).then(i.cmp(j)))
            .filter(|(_, a)| pos_of(a).distance(q) < 12.0)
    };
    // the point a drag holds, and whether this interaction opened an undo window
    let grab_id = egui::Id::new(("hslPointGrab", channel.as_str()));
    let write = |app: &mut LccApp, pts: &[lcc_geom::Point]| {
        let _ = app.run("develop.hslCurves", json!({"channel": channel, "points": pts.iter().map(|q| [q.x, q.y]).collect::<Vec<_>>()}));
    };
    let pointer = resp.interact_pointer_pos();
    // right-click on a point: Delete Control Point, as the Tone Curve's editor (the point under
    // the pointer read when the menu opens and kept while it is open)
    let menu_point_id = egui::Id::new(("hslMenuPoint", channel.as_str()));
    if resp.secondary_clicked() {
        let hit = pointer.and_then(nearest).map(|(i, _)| i);
        ui.data_mut(|m| m.insert_temp(menu_point_id, hit));
    }
    let menu_point: Option<usize> = ui.data(|m| m.get_temp::<Option<usize>>(menu_point_id)).flatten().filter(|i| *i < points.len());
    crate::widgets::context_menu(&resp, |ui| {
        if let Some(i) = menu_point {
            let r = ui.button(crate::i18n::tr("Delete Control Point"));
            register(ui.ctx(), "hslMenu:deletePoint", r.rect);
            if r.clicked() {
                let pts: Vec<lcc_geom::Point> = points.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, q)| *q).collect();
                write(app, &pts);
                ui.close();
            }
        }
    });
    if resp.drag_started() {
        // an empty spot grows a point (never within the Tone Curve's POINT_GAP of another); an
        // existing one is grabbed
        let mut pts = points.clone();
        let held = match pointer.map(nearest) {
            Some(Some((i, _))) => Some(i),
            Some(None) => {
                let at = to_point(pointer.unwrap_or(plot.left_top()));
                super::edit::with_point(&pts, at.x, at.y).map(|(i, v)| {
                    pts = v;
                    i
                })
            }
            None => None,
        };
        ui.data_mut(|m| m.insert_temp(grab_id, (held, held.is_some())));
        if held.is_some() {
            write(app, &pts);
            let _ = app.run("develop.beginInteraction", json!({"label": crate::i18n::tr("HSL Curves")}));
        }
    }
    if resp.dragged()
        && let Some(held) = ui.data(|m| m.get_temp::<(Option<usize>, bool)>(grab_id)).and_then(|(h, _)| h)
        && held < points.len()
    {
        let mut pts = points.clone();
        let prev = if held > 0 { pts[held - 1].x + super::edit::POINT_GAP } else { 0.0 };
        let next = if held + 1 < pts.len() { pts[held + 1].x - super::edit::POINT_GAP } else { 1.0 };
        let to = to_point(pointer.unwrap_or(pos_of(&pts[held])));
        pts[held].x = to.x.clamp(prev.min(next), prev.max(next));
        pts[held].y = to.y;
        write(app, &pts);
    }
    if resp.drag_stopped() {
        if ui.data(|m| m.get_temp::<(Option<usize>, bool)>(grab_id)).is_some_and(|(_, began)| began) {
            let _ = app.run("develop.endInteraction", json!({}));
        }
        ui.data_mut(|m| m.remove::<(Option<usize>, bool)>(grab_id));
    }
    // a click on an empty spot plants one point (one undo step of its own); a click on a point
    // does nothing — drags, double-clicks and the right-click menu are the point's tools
    if resp.clicked()
        && pointer.is_some_and(|q| nearest(q).is_none())
        && ui.data(|m| m.get_temp::<(Option<usize>, bool)>(grab_id)).is_none()
        && let Some((_, pts)) = pointer.map(to_point).and_then(|at| super::edit::with_point(&points, at.x, at.y))
    {
        write(app, &pts);
    }
    if resp.double_clicked()
        && let Some((i, _)) = pointer.and_then(nearest)
    {
        let mut pts = points.clone();
        pts.remove(i);
        write(app, &pts);
    }
}
