//! The map canvas: tiles, tracks, saved-location circles, pins and clusters, attribution; pan,
//! zoom, pin hover / click / drag, and drops of photos dragged from the filmstrip or grid.

use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use serde_json::json;

use lcc_geo::LatLon;
use lcc_geo::mercator::{TILE, project, unproject, visible_tiles, world_px};

use super::MapUi;
use crate::LccApp;
use crate::module::{Edge, edge_visible};
use crate::theme::Tokens;
use crate::widgets::register;

/// Height of the filter / search bar.
const BAR_H: f32 = 34.0;

/// Screen position of a place (the copy of the world nearest the centre).
pub fn geo_to_screen(m: &MapUi, r: Rect, p: LatLon) -> Pos2 {
    let world = world_px(m.prefs.zoom);
    let (cx, cy) = project(m.centre());
    let (x, y) = project(p);
    let mut dx = x - cx;
    dx -= dx.round();
    pos2(r.center().x + (dx * world) as f32, r.center().y + ((y - cy) * world) as f32)
}

pub fn screen_to_geo(m: &MapUi, r: Rect, s: Pos2) -> LatLon {
    let world = world_px(m.prefs.zoom);
    let (cx, cy) = project(m.centre());
    unproject(cx + f64::from(s.x - r.center().x) / world, cy + f64::from(s.y - r.center().y) / world)
}

/// The Map module's centre: filter bar, map canvas, side panel; the filmstrip below.
pub fn center(ui: &mut egui::Ui, app: &mut LccApp) {
    app.map.start(app.headless_host);
    let mut area = ui.available_rect_before_wrap();
    if edge_visible(app, Edge::Bottom) {
        let film = Rect::from_min_max(pos2(area.left(), area.bottom() - app.ui.film_height()), area.max);
        area.max.y = film.top();
        crate::panels::detail::filmstrip(app, ui, film);
    }
    app.canvas_rect = Some(area);
    register(ui.ctx(), "view:module:map", area);
    super::side::poll_jobs(app, ui.ctx());
    let side = crate::panels::split_side(ui, &mut area, false, "map:side", &mut app.ui.right_width, crate::state::RIGHT_WIDTH);
    let bar = Rect::from_min_max(area.min, pos2(side.left(), area.top() + BAR_H));
    let map = Rect::from_min_max(pos2(area.left(), bar.bottom()), pos2(side.left(), area.bottom()));
    super::side::bar(app, ui, bar);
    super::side::panel(app, ui, side);
    if map.width() > 10.0 && map.height() > 10.0 {
        canvas(app, ui, map);
    }
}

fn canvas(app: &mut LccApp, ui: &mut egui::Ui, r: Rect) {
    let t = Tokens::get(ui.ctx());
    let ctx = ui.ctx().clone();
    app.map.canvas = Some(r);
    register(&ctx, "map:canvas", r);
    // fork #741: a filmstrip selection whose pins lie off screen brings the map to them (pan only).
    // The first frame only records the selection, so entering the module never jumps.
    let sel: Vec<u64> = app.session.selection.ids.iter().map(|p| p.0).collect();
    if super::glide_step(&mut app.map, ui.input(|i| i.time)) {
        ctx.request_repaint();
    }
    if app.map.last_selection.as_ref().is_some_and(|prev| prev.as_slice() != sel.as_slice()) {
        super::reveal_selection(app, false);
    } else {
        let mut sorted = sel;
        sorted.sort_unstable();
        app.map.last_selection = Some(sorted);
    }
    let resp = ui.interact(r, egui::Id::new("map-canvas"), Sense::click_and_drag());
    crate::access::named(&resp, egui::WidgetType::Other, crate::i18n::tr("Map"));
    let painter = ui.painter_at(r);
    painter.rect_filled(r, 0.0, Color32::from_rgb(0xaa, 0xd3, 0xdf));

    // ---- input: pan, zoom (pointer-anchored), double-click zoom
    let hover = resp.hover_pos();
    let pointer_in_map = hover.or_else(|| ui.input(|i| i.multi_touch().map(|t| t.center_pos).or(i.pointer.hover_pos())).filter(|p| r.contains(*p)));
    if resp.dragged() && app.map.dragging_pin.is_none() && app.map.dragging_location.is_none() {
        let d = resp.drag_delta();
        let world = world_px(app.map.prefs.zoom);
        let (cx, cy) = project(app.map.centre());
        let c = unproject(cx - f64::from(d.x) / world, cy - f64::from(d.y) / world);
        let z = app.map.prefs.zoom;
        app.map.set_view(c, z);
    }
    if resp.drag_stopped() {
        app.map.save();
    }
    let scroll = if pointer_in_map.is_some() { ui.input(|i| i.smooth_scroll_delta.y + i.zoom_delta().log2() * 200.0) } else { 0.0 };
    if let Some(at) = pointer_in_map
        && scroll.abs() > 0.0
    {
        zoom_at(app, r, at, f64::from(scroll) / 200.0);
        ui.input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
    }
    if resp.double_clicked()
        && let Some(at) = hover
    {
        zoom_at(app, r, at, 1.0);
        app.map.save();
    }

    // ---- tiles: integer zoom level, scaled by the fraction
    let server = app.map.server();
    let zf = app.map.prefs.zoom;
    let z = zf.floor().clamp(0.0, f64::from(server.max_zoom)) as u8;
    let scale = 2f64.powf(zf - f64::from(z));
    let (w, h) = (f64::from(r.width()) / scale, f64::from(r.height()) / scale);
    let tiles = visible_tiles(app.map.centre(), z, w, h);
    let sync = app.headless_host;
    app.map.tiles.poll(&ctx);
    let size = (TILE * scale) as f32;
    for (k, ox, oy) in &tiles {
        let min = r.min + vec2((*ox * scale) as f32, (*oy * scale) as f32);
        let tr = Rect::from_min_size(min, vec2(size, size));
        app.map.tiles.want(&ctx, &server, *k, sync);
        if let Some(tex) = app.map.tiles.get(&server.id, *k) {
            painter.image(tex, tr.expand(0.25), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            continue;
        }
        // a coarser tile already loaded stands in while this one loads
        let mut drawn = false;
        let (mut pk, mut uv) = (*k, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)));
        for _ in 0..4 {
            let Some(parent) = pk.parent() else { break };
            let (qx, qy) = ((pk.x % 2) as f32 * 0.5, (pk.y % 2) as f32 * 0.5);
            uv = Rect::from_min_size(pos2(qx + uv.min.x * 0.5, qy + uv.min.y * 0.5), uv.size() * 0.5);
            pk = parent;
            if let Some(tex) = app.map.tiles.get(&server.id, pk) {
                painter.image(tex, tr.expand(0.25), uv, Color32::WHITE);
                drawn = true;
                break;
            }
        }
        if !drawn {
            painter.rect_filled(tr, 0.0, Color32::from_rgb(0xe8, 0xe4, 0xdc));
            painter.rect_stroke(tr, 0.0, Stroke::new(0.5, Color32::from_rgb(0xd0, 0xcc, 0xc4)), StrokeKind::Inside);
        }
    }
    if app.map.tiles.busy() {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }

    // ---- tracks
    for tr in app.map.tracks.iter().filter(|t| t.shown) {
        for seg in &tr.track.segments {
            let step = (seg.len() / 4000).max(1);
            let pts: Vec<Pos2> = seg.iter().step_by(step).map(|p| geo_to_screen(&app.map, r, *p)).collect();
            if pts.len() >= 2 {
                painter.add(egui::Shape::line(pts.clone(), Stroke::new(4.0, Color32::from_rgba_unmultiplied(255, 255, 255, 170))));
                painter.add(egui::Shape::line(pts, Stroke::new(2.0, super::info::TRACK)));
            } else if let Some(p) = pts.first() {
                painter.circle_filled(*p, 3.0, super::info::TRACK);
            }
        }
    }

    // ---- saved locations: circles, and the centre handles of the ones not active (under the pins)
    let mpp = lcc_geo::mercator::metres_per_px(app.map.prefs.lat, zf);
    location_drag_follow(app, ui, r);
    locations(app, ui, &painter, r, mpp, false);

    // ---- pins
    let pins = app.map.pins(&mut app.session);
    let selected: std::collections::HashSet<u64> = app.session.selection.ids.iter().map(|p| p.0).collect();
    app.map.drawn.clear();
    let mut hovered: Option<(Pos2, Vec<u64>)> = None;
    for c in pins.iter() {
        let at = geo_to_screen(&app.map, r, c.centre);
        if !r.expand(30.0).contains(at) {
            continue;
        }
        let n = c.items.len();
        let sel = c.items.iter().any(|i| selected.contains(i));
        let id = egui::Id::new(("map-pin", c.items.first().copied().unwrap_or(0)));
        let rad = if n == 1 { 7.0 } else { 11.0 + (n as f32).log10() * 4.0 };
        let hit = Rect::from_center_size(at, vec2(rad * 2.0 + 6.0, rad * 2.0 + 6.0));
        let pr = ui.interact(hit, id, Sense::click_and_drag());
        crate::access::named(&pr, egui::WidgetType::Button, &format!("{} ({n})", crate::i18n::tr("Photos at this place")));
        register(&ctx, format!("map:pin:{}", c.items.first().copied().unwrap_or(0)), hit);
        let fill = if sel { t.accent } else { super::info::PIN };
        painter.circle(at, rad, fill, Stroke::new(2.0, Color32::WHITE));
        if n > 1 {
            painter.text(at, Align2::CENTER_CENTER, n.to_string(), t.semibold(if n > 99 { 9.5 } else { 11.0 }), Color32::WHITE);
        }
        app.map.drawn.push((at, c.items.clone()));
        if pr.hovered() {
            hovered = Some((at, c.items.clone()));
        }
        if pr.clicked() {
            let mode = if ui.input(|i| i.modifiers.command) { "add" } else { "replace" };
            let _ = app.run("library.select", json!({"ids": c.items, "mode": mode}));
        }
        if pr.double_clicked() {
            zoom_at(app, r, at, 2.0);
        }
        if pr.drag_started() && !app.map.prefs.lock_markers {
            app.map.dragging_pin = Some(c.items.clone());
        }
    }
    // a pin being moved: follow the pointer; drop = new position for its photos
    if let Some(ids) = app.map.dragging_pin.clone() {
        let (pos, released) = ui.input(|i| (i.pointer.latest_pos(), i.pointer.any_released()));
        if let Some(p) = pos {
            painter.circle(p, 8.0, t.accent.gamma_multiply(0.7), Stroke::new(2.0, Color32::WHITE));
            if released {
                if r.contains(p) {
                    let at = screen_to_geo(&app.map, r, p);
                    // the overlay says what changed; only a refusal is a toast
                    if let Err(e) = app.run("map.geotag", json!({"ids": ids, "lat": at.lat, "lon": at.lon})) {
                        app.toast_error(&ctx, e);
                    }
                }
                app.map.dragging_pin = None;
            }
        } else if released {
            app.map.dragging_pin = None;
        }
    }
    // photos dragged from the filmstrip or the grid: drop to geotag
    if let Some(ids) = app.ui.dragging_photos.clone() {
        let (pos, released) = ui.input(|i| (i.pointer.latest_pos(), i.pointer.any_released()));
        if let Some(p) = pos.filter(|p| r.contains(*p)) {
            painter.rect_stroke(r.shrink(2.0), 0.0, Stroke::new(2.0, t.accent), StrokeKind::Inside);
            painter.circle_stroke(p, 10.0, Stroke::new(2.0, t.accent));
            if released {
                let at = screen_to_geo(&app.map, r, p);
                if let Err(e) = app.run("map.geotag", json!({"ids": ids, "lat": at.lat, "lon": at.lon})) {
                    app.toast_error(&ctx, e);
                }
                app.ui.dragging_photos = None;
            }
        }
    }
    // the active saved location's handles (centre and radius) and the Edit Location panel's draft: above the pins
    locations(app, ui, &painter, r, mpp, true);
    // the hover card: opened by a hovered pin, kept while the pointer is on the pin or the card
    let busy = app.map.dragging_pin.is_some()
        || app.map.dragging_location.is_some()
        || app.ui.dragging_photos.is_some()
        || resp.dragged()
        || scroll.abs() > 0.0;
    let pointer = ui.input(|i| i.pointer.hover_pos());
    app.map.hover_card = if busy { None } else { next_hover_card(app.map.hover_card.take(), hovered, &app.map.drawn, pointer) };
    if app.map.hover_card.is_some() {
        hover_card(app, ui, r);
    }

    // ---- context menu
    // the place right-clicked (or long-pressed on a touch screen: egui counts that as a secondary click), kept while
    // the menu is open: the pointer leaves the map for the menu
    if resp.secondary_clicked() {
        app.map.menu_at = resp.interact_pointer_pos().or(hover).map(|p| screen_to_geo(&app.map, r, p));
    }
    resp.context_menu(|ui| {
        let Some(at) = app.map.menu_at else { return };
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        ui.label(egui::RichText::new(format!("{:.5}, {:.5}", at.lat, at.lon)).weak());
        let has_sel = !app.session.selection.ids.is_empty();
        let b = ui.add_enabled(has_sel, egui::Button::new(crate::i18n::tr("Geotag Selected Photos Here")));
        register(ui.ctx(), "map:menu:geotag", b.rect);
        if b.clicked() {
            if let Err(e) = app.run("map.geotag", json!({"lat": at.lat, "lon": at.lon})) {
                app.toast_error(ui.ctx(), e);
            }
            ui.close();
        }
        let b = ui.button(crate::i18n::tr("New Location Here…"));
        register(ui.ctx(), "map:menu:newLocation", b.rect);
        if b.clicked() {
            super::open_new_location(app, at, None);
            ui.close();
        }
        let b = ui.button(crate::i18n::tr("Copy Coordinates"));
        register(ui.ctx(), "map:menu:copy", b.rect);
        if b.clicked() {
            ui.ctx().copy_text(format!("{:.6}, {:.6}", at.lat, at.lon));
            ui.close();
        }
    });

    // ---- the corner boxes View ▸ Show Map Info / Show Map Key turn on
    let (info, key) = super::info::paint(&mut app.map, &painter, &t, r);
    if let Some(b) = info {
        register(&ctx, "map:info", b);
    }
    if let Some(b) = key {
        register(&ctx, "map:key", b);
    }

    // ---- attribution (always on screen) and zoom buttons
    let attribution = format!("{}{}", server.attribution, app.map.tiles.last_error.as_ref().map(|e| format!("  ·  {e}")).unwrap_or_default());
    let g = painter.layout_no_wrap(attribution, t.font(10.0), Color32::from_gray(40));
    let ar = Rect::from_min_size(pos2(r.right() - g.size().x - 10.0, r.bottom() - g.size().y - 6.0), g.size() + vec2(8.0, 4.0));
    painter.rect_filled(ar, CornerRadius::same(3), Color32::from_rgba_unmultiplied(255, 255, 255, 200));
    painter.galley(ar.min + vec2(4.0, 2.0), g, Color32::from_gray(40));
    register(&ctx, "map:attribution", ar);
    for (i, (label, dz)) in [("+", 1.0), ("−", -1.0)].into_iter().enumerate() {
        let br = Rect::from_min_size(pos2(r.left() + 10.0, r.top() + 10.0 + i as f32 * 30.0), vec2(26.0, 26.0));
        let b = ui.interact(br, egui::Id::new(("map-zoom", i)), Sense::click());
        crate::access::button(&b, if dz > 0.0 { "Zoom In" } else { "Zoom Out" });
        register(&ctx, format!("map:zoom{}", if dz > 0.0 { "In" } else { "Out" }), br);
        painter.rect(
            br,
            CornerRadius::same(4),
            if b.hovered() { Color32::from_gray(235) } else { Color32::WHITE },
            Stroke::new(1.0, Color32::from_gray(160)),
            StrokeKind::Inside,
        );
        painter.text(br.center(), Align2::CENTER_CENTER, label, t.semibold(16.0), Color32::from_gray(40));
        if b.clicked() {
            let c = app.map.centre();
            app.map.set_view(c, app.map.prefs.zoom.round() + dz);
            app.map.save();
        }
    }
    // scale bar
    let metres = mpp * 100.0;
    let nice = [1.0, 2.0, 5.0].iter().flat_map(|m| (0..8).map(move |e| m * 10f64.powi(e))).filter(|v| *v <= metres).fold(1.0, f64::max);
    let px = (nice / mpp.max(1e-9)) as f32;
    let y = r.bottom() - 12.0;
    painter.line_segment([pos2(r.left() + 10.0, y), pos2(r.left() + 10.0 + px, y)], Stroke::new(2.0, Color32::from_gray(40)));
    let label = if nice >= 1000.0 { format!("{} km", nice / 1000.0) } else { format!("{nice} m") };
    painter.text(pos2(r.left() + 12.0, y - 3.0), Align2::LEFT_BOTTOM, label, t.font(10.0), Color32::from_gray(40));

    location_editor(app, &ctx, r);
}

// ---------- saved locations

/// A saved location's circle as drawn this frame: the library's, the one being dragged (where the
/// pointer has it), or the Edit Location panel's draft.
struct Circle {
    /// The library name (`None`: the panel's draft of a new location).
    name: Option<String>,
    centre: LatLon,
    radius: f64,
    private: bool,
    /// Shows the radius handle and sits above the pins.
    active: bool,
    /// Drawn by the Edit Location panel (its changes wait for Save).
    draft: bool,
    /// The name shown above the circle (the panel's draft shows the name being typed).
    label: String,
}

fn key(name: &str) -> String {
    name.trim().to_lowercase()
}

fn circles(app: &LccApp) -> Vec<Circle> {
    let m = &app.map;
    let editing = m.editor.as_ref().and_then(|e| e.original.as_deref()).map(key);
    let active = m.active_location.as_deref().map(key);
    let drag = m.dragging_location.as_ref();
    let mut out: Vec<Circle> = app
        .session
        .catalog
        .saved_locations()
        .filter(|_| m.prefs.show_saved)
        .filter(|l| editing.as_deref() != Some(key(&l.name).as_str()))
        .map(|l| {
            let held = drag.filter(|d| d.name.as_deref().map(key) == Some(key(&l.name)));
            Circle {
                name: Some(l.name.clone()),
                centre: held.map_or(l.centre(), |d| d.centre),
                radius: held.map_or(l.radius, |d| d.radius),
                private: l.private,
                active: active.as_deref() == Some(key(&l.name).as_str()),
                draft: false,
                label: l.name.clone(),
            }
        })
        .collect();
    if let Some(e) = m.editor.as_ref() {
        let held = drag.filter(|d| d.name.is_none());
        out.push(Circle {
            name: e.original.clone(),
            centre: held.map_or(e.centre, |d| d.centre),
            radius: held.map_or(e.radius, |d| d.radius),
            private: e.private,
            active: true,
            draft: true,
            label: e.name.clone(),
        });
    }
    out
}

/// The widget id of a circle's handle (`centre` / `radius`), for scripts and tests.
fn handle_widget(c: &Circle, what: &str) -> String {
    match (&c.name, c.draft) {
        (Some(n), false) => format!("map:location:{n}:{what}"),
        _ => format!("map:location:draft:{what}"),
    }
}

fn label_text(c: &Circle) -> String {
    if c.private { format!("🔒 {}", c.label) } else { c.label.clone() }
}

/// Where each circle's name goes (its top-left and its laid-out text), or `None` where it would
/// overlap a name already placed: when zoomed out, nearby locations' names would otherwise pile up
/// into an unreadable smear. The active circle (or the Edit Location panel's draft) places first,
/// then the others, largest on screen first, so a town's name wins over the houses inside it.
/// Names off the map are not placed and take no room.
fn place_labels(
    app: &LccApp,
    painter: &egui::Painter,
    r: Rect,
    mpp: f64,
    all: &[Circle],
    t: &Tokens,
) -> Vec<Option<(Pos2, std::sync::Arc<egui::Galley>)>> {
    let rad = |c: &Circle| c.radius / mpp.max(1e-9);
    let mut order: Vec<(usize, &Circle)> = all.iter().enumerate().collect();
    order.sort_by(|(_, ca), (_, cb)| cb.active.cmp(&ca.active).then(rad(cb).total_cmp(&rad(ca))).then_with(|| ca.label.cmp(&cb.label)));
    let mut out = vec![None; all.len()];
    let mut taken: Vec<Rect> = Vec::new();
    for (i, c) in order {
        let col = if c.private { super::info::SAVED_PRIVATE } else { super::info::SAVED };
        let galley = painter.layout_no_wrap(label_text(c), t.semibold(11.0), col);
        let at = geo_to_screen(&app.map, r, c.centre);
        let px = (rad(c) as f32).clamp(3.0, 20_000.0);
        // the active circle's name sits clear of its radius handle
        let centre = at + vec2(0.0, -px - if c.active { 20.0 } else { 8.0 });
        let rect = Rect::from_center_size(centre, galley.size());
        if !r.intersects(rect) {
            continue;
        }
        if !c.active && taken.iter().any(|o| o.expand(2.0).intersects(rect)) {
            continue;
        }
        taken.push(rect);
        if let Some(slot) = out.get_mut(i) {
            *slot = Some((rect.min, galley));
        }
    }
    out
}

/// Paint the saved locations. `top = false`: every circle and the centre handles of the inactive
/// ones (drawn before the pins, so a pin wins where they overlap); `top = true`: the active one's
/// centre and radius handles, above the pins.
fn locations(app: &mut LccApp, ui: &mut egui::Ui, painter: &egui::Painter, r: Rect, mpp: f64, top: bool) {
    let t = Tokens::get(ui.ctx());
    let locked = app.map.prefs.lock_markers;
    let all = circles(app);
    let labels = if top { Vec::new() } else { place_labels(app, painter, r, mpp, &all, &t) };
    for (i, c) in all.into_iter().enumerate() {
        // an active circle is drawn with the others (under the pins); only its handles go on top
        if top && !c.active {
            continue;
        }
        let at = geo_to_screen(&app.map, r, c.centre);
        let rad = (c.radius / mpp.max(1e-9)).clamp(3.0, 20_000.0) as f32;
        let col = if c.private { super::info::SAVED_PRIVATE } else { super::info::SAVED };
        // a name left out for want of room shows when the centre handle is hovered
        let mut hidden_label = None;
        if !top {
            let (fill, width) = if c.active { (0.22, 2.5) } else { (0.15, 1.5) };
            painter.circle(at, rad, col.gamma_multiply(fill), Stroke::new(width, col));
            match labels.get(i).and_then(Option::as_ref) {
                Some((pos, galley)) => {
                    if let Some(n) = c.name.as_deref().filter(|_| !c.draft) {
                        register(ui.ctx(), format!("map:location:{n}:label"), Rect::from_min_size(*pos, galley.size()));
                    }
                    painter.galley(*pos, galley.clone(), col);
                }
                None => hidden_label = Some(label_text(&c)),
            }
            if c.active {
                continue;
            }
        }
        if !r.expand(10.0).contains(at) {
            continue;
        }
        // centre handle
        let dot = if c.active { 6.0 } else { 4.5 };
        let hit = Rect::from_center_size(at, vec2(18.0, 18.0));
        let wid = handle_widget(&c, "centre");
        let id = egui::Id::new(("map-location", wid.clone()));
        let h = ui.interact(hit, id, Sense::click_and_drag());
        let h = match hidden_label {
            Some(text) => h.on_hover_text(text),
            None => h,
        };
        crate::access::named(&h, egui::WidgetType::Button, &crate::i18n::tr_format!("Move saved location {name}", name = c.label.clone()));
        register(ui.ctx(), wid, hit);
        painter.circle(at, if h.hovered() { dot + 1.5 } else { dot }, col, Stroke::new(2.0, Color32::WHITE));
        if h.hovered() && !locked {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        if h.clicked() && !c.draft {
            app.map.active_location = c.name.clone();
        }
        if h.double_clicked()
            && !c.draft
            && let Some(n) = c.name.as_deref()
            && let Err(e) = super::open_edit_location(app, n)
        {
            app.toast_error(ui.ctx(), e);
        }
        if h.drag_started() && !locked {
            let press = ui.input(|i| i.pointer.press_origin()).unwrap_or(at);
            app.map.dragging_location = Some(super::LocationDrag {
                name: if c.draft { None } else { c.name.clone() },
                handle: super::LocationHandle::Centre,
                grab: at - press,
                centre: c.centre,
                radius: c.radius,
            });
        }
        // radius handle: on the circle's top edge, for the active circle drawn large enough to grab
        if !c.active || rad < 14.0 {
            continue;
        }
        let knob = at + vec2(0.0, -rad);
        let hit = Rect::from_center_size(knob, vec2(18.0, 18.0));
        let wid = handle_widget(&c, "radius");
        let id = egui::Id::new(("map-location", wid.clone()));
        let h = ui.interact(hit, id, Sense::click_and_drag());
        crate::access::named(&h, egui::WidgetType::Button, &crate::i18n::tr_format!("Change the radius of {name}", name = c.label.clone()));
        register(ui.ctx(), wid, hit);
        painter.circle(knob, if h.hovered() { 7.0 } else { 5.5 }, Color32::WHITE, Stroke::new(2.0, col));
        if h.hovered() && !locked {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        if h.drag_started() && !locked {
            app.map.dragging_location = Some(super::LocationDrag {
                name: if c.draft { None } else { c.name.clone() },
                handle: super::LocationHandle::Edge,
                grab: vec2(0.0, 0.0),
                centre: c.centre,
                radius: c.radius,
            });
        }
        let dragging_this =
            app.map.dragging_location.as_ref().is_some_and(|d| d.handle == super::LocationHandle::Edge && (d.name.is_none() == c.draft));
        if dragging_this || h.hovered() {
            let text = super::radius_label(c.radius);
            let g = painter.layout_no_wrap(text, t.semibold(11.0), Color32::WHITE);
            let br = Rect::from_min_size(knob + vec2(11.0, -g.size().y / 2.0 - 2.0), g.size() + vec2(8.0, 4.0));
            painter.rect_filled(br, CornerRadius::same(3), col);
            painter.galley(br.min + vec2(4.0, 2.0), g, Color32::WHITE);
        }
    }
}

/// A saved-location handle being dragged: follow the pointer; on release outside the map cancel,
/// else save the new centre or radius (one undo step), or hand it to the Edit Location panel when
/// it is the panel's circle.
fn location_drag_follow(app: &mut LccApp, ui: &mut egui::Ui, r: Rect) {
    let Some(mut d) = app.map.dragging_location.clone() else { return };
    let (pos, released) = ui.input(|i| (i.pointer.latest_pos(), i.pointer.any_released()));
    let Some(p) = pos else {
        if released {
            app.map.dragging_location = None;
        }
        return;
    };
    match d.handle {
        super::LocationHandle::Centre => d.centre = screen_to_geo(&app.map, r, p + d.grab),
        super::LocationHandle::Edge => {
            let at = screen_to_geo(&app.map, r, p);
            d.radius = lcc_geo::distance_m(d.centre, at).clamp(lcc_geo::places::MIN_RADIUS_M, lcc_geo::places::MAX_RADIUS_M);
        }
    }
    if !released {
        app.map.dragging_location = Some(d);
        ui.ctx().set_cursor_icon(if d_is_centre(&app.map) { egui::CursorIcon::Grabbing } else { egui::CursorIcon::ResizeVertical });
        return;
    }
    app.map.dragging_location = None;
    if !r.contains(p) {
        return;
    }
    let editing_it = app.map.editor.as_ref().is_some_and(|e| match (&d.name, &e.original) {
        (None, _) => true,
        (Some(n), Some(o)) => key(n) == key(o),
        _ => false,
    });
    if editing_it {
        if let Some(e) = app.map.editor.as_mut() {
            e.centre = d.centre;
            e.radius = d.radius;
        }
        return;
    }
    let Some(name) = d.name else { return };
    let params = match d.handle {
        super::LocationHandle::Centre => json!({"name": name, "lat": d.centre.lat, "lon": d.centre.lon}),
        super::LocationHandle::Edge => json!({"name": name, "radius": d.radius}),
    };
    app.map.active_location = Some(name);
    if let Err(e) = app.run("map.saveLocation", params) {
        app.toast_error(ui.ctx(), e);
    }
}

fn d_is_centre(m: &MapUi) -> bool {
    m.dragging_location.as_ref().is_some_and(|d| d.handle == super::LocationHandle::Centre)
}

/// The Edit Location panel, over the map's top-left corner: name, radius, private; Save, Cancel and
/// (for a saved location) Delete. Not modal: the map stays usable, so the circle can be dragged
/// while the panel is open.
fn location_editor(app: &mut LccApp, ctx: &egui::Context, r: Rect) {
    let Some(mut ed) = app.map.editor.clone() else { return };
    let title = if ed.original.is_some() { crate::i18n::tr("Edit Location") } else { crate::i18n::tr("New Location") };
    let window_id = egui::Id::new("map-location-editor");
    let mut action: Option<&str> = None;
    let mut typing = false;
    egui::Window::new(title)
        .id(window_id)
        .title_bar(false)
        .frame(crate::dialog_frame::card_frame(ctx))
        .collapsible(false)
        .resizable(false)
        .default_width(300.0)
        .fixed_pos(r.min + vec2(48.0, 12.0))
        .show(ctx, |ui| {
            if crate::dialog_frame::card_header(ui, title, Some("mapLocation:close")) {
                action = Some("cancel");
            }
            egui::Grid::new("map-location-editor-grid").num_columns(2).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
                ui.label(crate::i18n::tr("Location Name"));
                let f = crate::text_field::TextField::singleline("field:mapLocationName", &mut ed.name)
                    .label(crate::i18n::tr("Location Name"))
                    .width(190.0)
                    .select_on_focus(true)
                    .show(ui);
                typing |= f.response.has_focus();
                ui.end_row();
                ui.label(crate::i18n::tr("Radius"));
                ui.horizontal(|ui| {
                    let mut v = if ed.km { ed.radius / 1000.0 } else { ed.radius };
                    let (lo, hi) = if ed.km { (0.001, 1000.0) } else { (1.0, 1_000_000.0) };
                    let speed = if ed.km { 0.01 } else { 1.0 };
                    let dv = ui.add(egui::DragValue::new(&mut v).range(lo..=hi).speed(speed).max_decimals(if ed.km { 3 } else { 0 }));
                    crate::access::label(&dv, "Radius");
                    register(ui.ctx(), "field:mapLocationRadius", dv.rect);
                    typing |= dv.has_focus();
                    if dv.changed() {
                        ed.radius = if ed.km { v * 1000.0 } else { v };
                    }
                    let unit_name = if ed.km { "km" } else { "m" };
                    let cb = egui::ComboBox::from_id_salt("map-location-unit").selected_text(unit_name).width(56.0).show_ui(ui, |ui| {
                        ui.selectable_value(&mut ed.km, false, "m");
                        ui.selectable_value(&mut ed.km, true, "km");
                    });
                    register(ui.ctx(), "combo:mapLocationUnit", cb.response.rect);
                });
                ui.end_row();
            });
            let c = ui.checkbox(&mut ed.private, crate::i18n::tr("Private"));
            register(ui.ctx(), "check:mapLocationPrivate", c.rect);
            ui.label(egui::RichText::new(crate::i18n::tr("Photos inside a private location are exported without their location.")).weak().size(11.0));
            ui.label(
                egui::RichText::new(crate::i18n::tr("Drag the centre to move the location and the handle on its edge to change the radius."))
                    .weak()
                    .size(11.0),
            );
            if let Some(e) = ed.error.as_deref() {
                ui.colored_label(Tokens::get(ui.ctx()).caution, e);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ed.original.is_some() && crate::button::Button::new("Delete").danger().widget("button:mapLocationDelete").show(ui).clicked() {
                    action = Some("delete");
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if crate::button::Button::new("Save").primary().widget("button:mapLocationSave").show(ui).clicked() {
                        action = Some("save");
                    }
                    if crate::button::Button::new("Cancel").widget("button:mapLocationCancel").show(ui).clicked() {
                        action = Some("cancel");
                    }
                });
            });
        });
    if typing {
        app.text_focus = true;
        if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
            action = action.or(Some("save"));
        }
    }
    if !typing && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        action = action.or(Some("cancel"));
    }
    // the fields typed this frame go to the panel (a drag of its circle may have moved it meanwhile)
    if let Some(e) = app.map.editor.as_mut() {
        e.name = ed.name;
        e.radius = ed.radius;
        e.private = ed.private;
        e.km = ed.km;
    }
    match action {
        Some("save") => {
            let _ = super::save_location_editor(app);
        }
        Some("delete") => {
            if let Err(e) = super::delete_location_editor(app) {
                app.toast_error(ctx, e);
            }
        }
        Some(_) => app.map.editor = None,
        None => {}
    }
}

fn zoom_at(app: &mut LccApp, r: Rect, at: Pos2, dz: f64) {
    let before = screen_to_geo(&app.map, r, at);
    let c = app.map.centre();
    let z = app.map.prefs.zoom + dz;
    app.map.set_view(c, z);
    // keep the place under the pointer where it was
    let after = screen_to_geo(&app.map, r, at);
    let (bx, by) = project(before);
    let (ax, ay) = project(after);
    let (cx, cy) = project(app.map.centre());
    let zz = app.map.prefs.zoom;
    app.map.set_view(unproject(cx + (bx - ax), cy + (by - ay)), zz);
}

/// The hover card after this frame: a hovered pin opens it (or keeps it, on the photo it shows);
/// otherwise it stays while the pointer is on the card or on its pin, which may have moved.
pub(super) fn next_hover_card(
    card: Option<super::HoverCard>,
    hovered: Option<(Pos2, Vec<u64>)>,
    drawn: &[(Pos2, Vec<u64>)],
    pointer: Option<Pos2>,
) -> Option<super::HoverCard> {
    if let Some((at, ids)) = hovered {
        return Some(match card {
            Some(c) if c.ids == ids => super::HoverCard { anchor: at, ..c },
            _ => super::HoverCard { ids, index: 0, anchor: at, rect: None },
        });
    }
    let mut c = card?;
    c.anchor = drawn.iter().find(|(_, ids)| *ids == c.ids)?.0;
    // the pin, the card and the gap between them: the pointer crosses it to reach the arrows
    let zone =
        c.rect.map_or_else(|| Rect::from_center_size(c.anchor, vec2(28.0, 28.0)), |r| r.union(Rect::from_center_size(c.anchor, vec2(28.0, 28.0))));
    pointer.is_some_and(|p| zone.contains(p)).then_some(c)
}

/// Lightroom Classic's card over a hovered pin: the photo, its file name, capture time and
/// exposure, and for a pin of several photos "2 of 5" with arrows to step through them. A click on
/// the photo selects it.
fn hover_card(app: &mut LccApp, ui: &mut egui::Ui, r: Rect) {
    let Some(card) = app.map.hover_card.clone() else { return };
    let n = card.ids.len();
    let index = card.index.min(n.saturating_sub(1));
    let Some(id) = card.ids.get(index).copied() else { return };
    let pid = lcc_catalog::PhotoId(id);
    crate::panels::grid::request_thumb(app, pid, 256, 4);
    let t = Tokens::get(ui.ctx());
    let size = card.rect.map_or(vec2(206.0, 230.0), |c| c.size());
    let mut pos = card.anchor + vec2(14.0, -size.y - 10.0);
    pos.x = pos.x.min(r.right() - size.x - 4.0).max(r.left() + 4.0);
    pos.y = pos.y.max(r.top() + 4.0);
    let (name, date, exposure) = app
        .session
        .catalog
        .photo(pid)
        .map(|p| {
            let date = p.captured.as_deref().map(crate::i18n::display_time).unwrap_or_else(|| crate::i18n::tr("No date").into());
            (p.file_name.clone(), date, crate::panels::grid::exposure_line(&p.meta))
        })
        .unwrap_or_default();
    let thumb = app.renderer.thumb(pid);
    let mut step: Option<isize> = None;
    let mut pick = false;
    let shown = egui::Area::new(egui::Id::new("map-hover-card")).order(egui::Order::Foreground).fixed_pos(pos).show(ui.ctx(), |ui| {
        egui::Frame::new().fill(t.chrome).stroke(Stroke::new(1.0, t.divider)).corner_radius(CornerRadius::same(4)).inner_margin(6.0).show(ui, |ui| {
            ui.set_width(194.0);
            let (img, resp) = ui.allocate_exact_size(vec2(194.0, 130.0), Sense::click());
            register(ui.ctx(), "map:hover:photo", img);
            crate::access::named(&resp, egui::WidgetType::Button, &name);
            if let Some(tex) = thumb {
                let [tw, th] = tex.size;
                let s = (img.width() / tw.max(1) as f32).min(img.height() / th.max(1) as f32);
                let fr = Rect::from_center_size(img.center(), vec2(tw as f32 * s, th as f32 * s));
                ui.painter().image(tex.id(), fr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            } else {
                ui.painter().rect_filled(img, 0.0, t.skeleton);
            }
            pick = resp.clicked();
            ui.add(egui::Label::new(egui::RichText::new(&name).size(11.0).color(t.text)).truncate());
            ui.label(egui::RichText::new(&date).size(10.5).color(t.text_dim));
            if let Some(e) = &exposure {
                ui.add(egui::Label::new(egui::RichText::new(e).size(10.5).color(t.text_dim)).truncate());
            }
            if n > 1 {
                ui.horizontal(|ui| {
                    let b = ui.small_button("‹").on_hover_text(crate::i18n::tr("Previous"));
                    register(ui.ctx(), "map:hover:prev", b.rect);
                    if b.clicked() {
                        step = Some(-1);
                    }
                    ui.label(egui::RichText::new(crate::i18n::tr_format!("{i} of {n}", i = index + 1, n = n)).size(11.0).color(t.text));
                    let b = ui.small_button("›").on_hover_text(crate::i18n::tr("Next"));
                    register(ui.ctx(), "map:hover:next", b.rect);
                    if b.clicked() {
                        step = Some(1);
                    }
                });
            }
        });
    });
    register(ui.ctx(), "map:hover", shown.response.rect);
    if let Some(c) = app.map.hover_card.as_mut() {
        c.rect = Some(shown.response.rect);
        c.index = match step {
            // the arrows wrap round at both ends
            Some(d) => (index as isize + d).rem_euclid(n.max(1) as isize) as usize,
            None => index,
        };
    }
    if pick {
        let _ = app.run("library.select", json!({"ids": [id]}));
    }
}
