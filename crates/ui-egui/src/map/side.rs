//! The map's filter / search bar and its side panel (style, saved locations, tracklog, reverse
//! geocoding, tile cache); the background jobs they start.

use crate::json_ext::JsonExt as _;
use egui::{Rect, RichText};
use serde_json::{Value, json};

use lcc_engine::cmd::map::LocationFilter;
use lcc_geo::LatLon;

use super::Job;
use crate::LccApp;
use crate::i18n::tr;
use crate::theme::Tokens;
use crate::widgets::register;

/// Parse `48.85, 2.35` (decimal degrees).
fn parse_coords(q: &str) -> Option<LatLon> {
    let mut it = q.split([',', ' ', ';']).filter(|s| !s.is_empty()).map(|s| s.trim().parse::<f64>());
    let (Some(Ok(lat)), Some(Ok(lon)), None) = (it.next(), it.next(), it.next()) else { return None };
    let p = LatLon::new(lat, lon);
    p.is_valid().then_some(p)
}

/// `map.search`: coordinates, then saved locations, then (with consent) the online geocoder.
/// Saved locations match by name, ignoring case and accents (`lcc_engine::text::search_key`). One
/// match, or a name typed in full, centres the map on it; several are listed under the search box
/// (`MapUi::location_matches`) to pick from.
pub fn search(app: &mut LccApp, q: &str) -> Result<Value, String> {
    let q = q.trim();
    app.map.location_matches.clear();
    if q.is_empty() {
        app.map.results.clear();
        return Ok(json!({"results": []}));
    }
    if let Some(p) = parse_coords(q) {
        let z = app.map.prefs.zoom.max(12.0);
        app.map.set_view(p, z);
        return Ok(json!({"found": "coordinates", "lat": p.lat, "lon": p.lon}));
    }
    let key = lcc_engine::text::search_key(q);
    let matches: Vec<(String, String)> = app
        .session
        .catalog
        .saved_locations()
        .map(|l| (lcc_engine::text::search_key(&l.name), l.name.clone()))
        .filter(|(k, _)| k.contains(&key))
        .collect();
    let exact = matches.iter().find(|(k, _)| *k == key).or(if matches.len() == 1 { matches.first() } else { None });
    if let Some((_, name)) = exact {
        let name = name.clone();
        go_to_location(app, &name);
        return Ok(json!({"found": "savedLocation", "name": name}));
    }
    if !matches.is_empty() {
        let names: Vec<String> = matches.into_iter().map(|(_, n)| n).collect();
        app.map.location_matches = names.clone();
        return Ok(json!({"found": "savedLocations", "names": names}));
    }
    search_online(app, q)
}

/// The online geocoder (Nominatim) for `q`, with consent: `map.search {online: true}`, the last
/// step of `search`, and the saved-location list's "Search online for …" row (so a place that
/// shares its name with saved locations can still be looked up).
pub fn search_online(app: &mut LccApp, q: &str) -> Result<Value, String> {
    let q = q.trim();
    app.map.location_matches.clear();
    if !app.map.prefs.online_consent {
        return Err(tr(
            "Place search needs online lookups: turn them on under Reverse Geocoding (they send the search text to the geocoding server)",
        )
        .to_string());
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let endpoint = app.map.prefs.endpoint.clone();
        let query = q.to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("map-search".into()).spawn(move || {
            let r = lcc_geo::geocode::Nominatim::new(&endpoint)
                .and_then(|n| n.search(lcc_geo::geocode::Consent::Granted, &query))
                .map_err(|e| e.to_string());
            let _ = tx.send(r);
        });
        spawned.map_err(|e| e.to_string())?;
        app.map.jobs.push(Job::Search(rx));
        Ok(json!({"pending": true}))
    }
    #[cfg(target_arch = "wasm32")]
    Err(tr("Place search isn't available in the web app").to_string())
}

/// Centres the map on a saved location, zoomed so its circle fits, and shows its handles.
pub(crate) fn go_to_location(app: &mut LccApp, name: &str) {
    let Some(l) = app.session.catalog.saved_location(name).cloned() else { return };
    let z = (17.0 - (l.radius / 100.0).max(1.0).log2()).clamp(3.0, 17.0);
    app.map.set_view(l.centre(), z);
    app.map.active_location = Some(l.name);
    app.map.location_matches.clear();
}

/// `map.lookup {ids?}`: reverse-geocode the selected photos with the chosen provider (online in the
/// background, then applied as one undo step).
pub fn lookup(app: &mut LccApp, p: &Value) -> Result<Value, String> {
    let mut params = json!({"provider": app.map.prefs.geocoder, "overwrite": p.get("overwrite").and_then(Value::as_bool).unwrap_or(false)});
    if let (Some(o), Some(ids)) = (params.as_object_mut(), p.get("ids")) {
        o.insert("ids".into(), ids.clone());
    }
    match app.map.prefs.geocoder.as_str() {
        "offline" => app.run("map.reverseGeocode", params),
        "online" => {
            if !app.map.prefs.online_consent {
                return Err(tr("Allow online lookups first (Reverse Geocoding ▸ Online)").to_string());
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let ids: Vec<lcc_catalog::PhotoId> = app.session.targets(&params).map_err(|e| e.to_string())?;
                let todo: Vec<(u64, LatLon)> =
                    ids.iter().filter_map(|id| app.session.catalog.photo(*id)?.meta.gps.map(|(a, b)| (id.0, LatLon::new(a, b)))).take(500).collect();
                if todo.is_empty() {
                    return Err(tr("None of the selected photos has a location").to_string());
                }
                let endpoint = app.map.prefs.endpoint.clone();
                let (tx, rx) = std::sync::mpsc::channel();
                let spawned = std::thread::Builder::new().name("map-geocode".into()).spawn(move || {
                    let r = (|| -> Result<Value, String> {
                        let n = lcc_geo::geocode::Nominatim::new(&endpoint).map_err(|e| e.to_string())?;
                        let mut places = Vec::new();
                        let mut seen: Vec<(LatLon, Option<lcc_geo::geocode::Place>)> = Vec::new();
                        for (id, at) in todo {
                            let pl = match seen.iter().find(|(q, _)| lcc_geo::distance_m(*q, at) < 50.0) {
                                Some((_, pl)) => pl.clone(),
                                None => {
                                    let pl = n.reverse(lcc_geo::geocode::Consent::Granted, at).map_err(|e| e.to_string())?;
                                    seen.push((at, pl.clone()));
                                    pl
                                }
                            };
                            if let Some(pl) = pl {
                                places.push(
                                    json!({"id": id, "sublocation": pl.sublocation, "city": pl.city, "state": pl.state, "country": pl.country, "countryCode": pl.iso}),
                                );
                            }
                        }
                        Ok(json!({"provider": "given", "places": places}))
                    })();
                    let _ = tx.send(r);
                });
                spawned.map_err(|e| e.to_string())?;
                app.map.jobs.push(Job::Geocode(rx));
                Ok(json!({"pending": true}))
            }
            #[cfg(target_arch = "wasm32")]
            Err(tr("Online lookups aren't available in the web app").to_string())
        }
        _ => Err(tr("Reverse geocoding is off: choose Offline or Online first").to_string()),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn start_download(app: &mut LccApp) {
    let dir = lcc_engine::config::config_dir().map(|d| d.join("geonames"));
    let Some(dir) = dir else { return };
    let (tx, rx) = std::sync::mpsc::channel();
    let ok = std::thread::Builder::new()
        .name("geonames".into())
        .spawn(move || {
            let r = lcc_geo::geocode::download_geonames(lcc_geo::geocode::Consent::Granted, lcc_geo::geocode::GEONAMES_BASE, &dir);
            let _ = tx.send(r.map_err(|e| e.to_string()));
        })
        .is_ok();
    if ok {
        app.map.jobs.push(Job::Download(rx));
    }
}

/// Finish background jobs.
pub fn poll_jobs(app: &mut LccApp, ctx: &egui::Context) {
    use std::sync::mpsc::TryRecvError;
    let mut keep = Vec::new();
    for job in std::mem::take(&mut app.map.jobs) {
        match job {
            Job::Search(rx) => match rx.try_recv() {
                Ok(Ok(found)) => {
                    if let Some((_, p)) = found.first() {
                        let z = app.map.prefs.zoom.max(11.0);
                        app.map.set_view(*p, z);
                    }
                    if found.is_empty() {
                        app.toast(ctx, tr("No place found"));
                    }
                    app.map.results = found;
                }
                Ok(Err(e)) => app.toast_error(ctx, e),
                Err(TryRecvError::Empty) => keep.push(Job::Search(rx)),
                Err(TryRecvError::Disconnected) => {}
            },
            Job::Geocode(rx) => match rx.try_recv() {
                Ok(Ok(params)) => match app.run("map.reverseGeocode", params) {
                    // the overlay says "Reverse Geocode" when photos changed; only a lookup that filled none is a toast
                    Ok(v) if v.field("changed").as_u64().unwrap_or(0) == 0 => {
                        app.toast(ctx, crate::i18n::tr_format!("Filled the location of {n} photos", n = 0))
                    }
                    Ok(_) => {}
                    Err(e) => app.toast_error(ctx, e),
                },
                Ok(Err(e)) => app.toast_error(ctx, e),
                Err(TryRecvError::Empty) => keep.push(Job::Geocode(rx)),
                Err(TryRecvError::Disconnected) => {}
            },
            Job::Download(rx) => match rx.try_recv() {
                Ok(Ok(n)) => app.toast(ctx, crate::i18n::tr_format!("Downloaded {n} place names (GeoNames, CC-BY 4.0)", n = n)),
                Ok(Err(e)) => app.toast_error(ctx, e),
                Err(TryRecvError::Empty) => keep.push(Job::Download(rx)),
                Err(TryRecvError::Disconnected) => {}
            },
        }
    }
    if !keep.is_empty() {
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }
    app.map.jobs = keep;
}

/// The bar above the map: location filter and search.
pub fn bar(app: &mut LccApp, ui: &mut egui::Ui, r: Rect) {
    let t = Tokens::get(ui.ctx());
    ui.painter().rect_filled(r, 0.0, t.chrome);
    ui.painter().line_segment([r.left_bottom(), r.right_bottom()], egui::Stroke::new(1.0, t.divider));
    let mut child =
        ui.new_child(egui::UiBuilder::new().max_rect(r.shrink2(egui::vec2(10.0, 5.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let ui = &mut child;
    ui.label(RichText::new(tr("Location Filter:")).color(t.text_dim));
    let current = app.map.filter.clone();
    for (key, label) in [("visible", "Visible on Map"), ("tagged", "Tagged"), ("untagged", "Untagged"), ("all", "None")] {
        let on = current.key() == key;
        let b = ui.selectable_label(on, tr(label));
        register(ui.ctx(), format!("map:filter:{key}"), b.rect);
        if b.clicked() {
            let _ = app.run("map.filter", json!({"filter": key}));
        }
    }
    let names: Vec<String> = app.session.catalog.saved_locations().map(|l| l.name.clone()).collect();
    let shown = match &current {
        LocationFilter::Location(n) => n.clone(),
        _ => tr("Saved Location").to_string(),
    };
    egui::ComboBox::from_id_salt("map-filter-location").selected_text(shown).width(130.0).show_ui(ui, |ui| {
        for n in &names {
            if ui.selectable_label(current == LocationFilter::Location(n.clone()), n).clicked() {
                let _ = app.run("map.filter", json!({"filter": format!("location:{n}")}));
            }
        }
        if names.is_empty() {
            ui.label(RichText::new(tr("No saved locations")).weak());
        }
    });
    let counts = {
        let all = lcc_engine::cmd::map::filtered(&mut app.session, &LocationFilter::All);
        let tagged = all.iter().filter(|(_, g)| g.is_some()).count();
        (tagged, all.len() - tagged)
    };
    ui.label(RichText::new(crate::i18n::tr_format!("{a} on map · {b} untagged", a = counts.0, b = counts.1)).color(t.text_dim).size(11.0));
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let edit = egui::TextEdit::singleline(&mut app.map.search).hint_text(tr("Search place or lat, lon")).desired_width(200.0);
        let resp = ui.add(edit);
        crate::access::label(&resp, "Find Place");
        register(ui.ctx(), "map:search", resp.rect);
        if std::mem::take(&mut app.map.focus_search) {
            resp.request_focus();
        }
        if resp.has_focus() {
            app.text_focus = true;
        }
        if resp.changed() {
            app.map.location_matches.clear();
        }
        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            let q = app.map.search.clone();
            if let Err(e) = search(app, &q) {
                app.toast_error(ui.ctx(), e);
            }
        }
        location_matches(app, ui, resp.rect);
    });
}

/// The saved locations a search matched, when there were several: a list under the search box
/// (right-aligned with it); a click centres the map on that one. Esc or a click elsewhere closes it.
fn location_matches(app: &mut LccApp, ui: &mut egui::Ui, search: Rect) {
    if app.map.location_matches.is_empty() {
        return;
    }
    let names = app.map.location_matches.clone();
    let area = egui::Area::new(egui::Id::new("map:searchMatches"))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::RIGHT_TOP)
        .fixed_pos(search.right_bottom() + egui::vec2(0.0, 4.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                // one line per name, the list as wide as the longest (growing left, from the box's
                // right edge), and tall enough for ten before it scrolls: a ScrollArea in an Area
                // otherwise starts at its minimum height, a row or two
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                ui.set_min_width(search.width());
                let row = ui.spacing().interact_size.y + ui.spacing().item_spacing.y;
                let extra = if app.map.prefs.online_consent { 1.5 } else { 0.0 };
                let tall = ((names.len() as f32 + extra) * row).min(10.0 * row);
                egui::ScrollArea::vertical().min_scrolled_height(tall).max_height(tall).show(ui, |ui| {
                    ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                        for name in &names {
                            let b = ui.selectable_label(false, name);
                            register(ui.ctx(), format!("map:searchMatch:{name}"), b.rect);
                            if b.clicked() {
                                go_to_location(app, name);
                            }
                        }
                        // with online lookups allowed, the place itself may be what was meant
                        if app.map.prefs.online_consent {
                            ui.separator();
                            let q = app.map.search.trim().to_string();
                            let b = ui.selectable_label(false, crate::i18n::tr_format!("Search online for “{q}”…", q = q));
                            register(ui.ctx(), "map:searchOnline", b.rect);
                            if b.clicked()
                                && let Err(e) = search_online(app, &q)
                            {
                                app.toast_error(ui.ctx(), e);
                            }
                        }
                    });
                });
            });
        });
    register(ui.ctx(), "map:searchMatches", area.response.rect);
    let clicked_outside = ui.input(|i| i.pointer.any_pressed()) && !area.response.contains_pointer() && !ui.rect_contains_pointer(search);
    if clicked_outside || ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        app.map.location_matches.clear();
    }
}

fn section(ui: &mut egui::Ui, title: &str, open: bool, body: impl FnOnce(&mut egui::Ui)) {
    crate::widgets::folding_section(ui, &format!("map:{title}"), RichText::new(tr(title)).strong(), open, body);
}

/// The side panel.
pub fn panel(app: &mut LccApp, ui: &mut egui::Ui, r: Rect) {
    let t = Tokens::get(ui.ctx());
    ui.painter().rect_filled(r, 0.0, t.chrome);
    ui.painter().line_segment([r.left_top(), r.left_bottom()], egui::Stroke::new(1.0, t.divider));
    register(ui.ctx(), "map:side", r);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r.shrink(10.0)).layout(egui::Layout::top_down(egui::Align::Min)));
    egui::ScrollArea::vertical().id_salt("map-side-scroll").auto_shrink([false, false]).show(&mut child, |ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        style_section(app, ui);
        selected_section(app, ui);
        locations_section(app, ui);
        track_section(app, ui);
        geocode_section(app, ui);
        let results = app.map.results.clone();
        if !results.is_empty() {
            section(ui, "Search Results", true, |ui| {
                for (name, p) in results {
                    if ui.link(&name).clicked() {
                        let z = app.map.prefs.zoom.max(11.0);
                        app.map.set_view(p, z);
                    }
                }
            });
        }
    });
}

/// The selected photos' location and IPTC fields (fork #784), in the Library Metadata panel's
/// widgets (`photo.setMeta`): GPS, Sublocation, City, State / Region, Country, Title, Headline,
/// Caption and Keywords. Every edit applies to every selected photo (the photos sharing a pin, say).
/// The ISO country code has no catalog field (see LR-MAP-REVGEO), so it has no row here.
fn selected_section(app: &mut LccApp, ui: &mut egui::Ui) {
    let photos: Vec<std::sync::Arc<lcc_catalog::Photo>> =
        app.session.selected().iter().filter_map(|i| app.session.catalog.photo(*i).cloned()).collect();
    section(ui, "Metadata", true, |ui| {
        if photos.is_empty() {
            ui.label(RichText::new(tr("Select a photo on the map or in the filmstrip to see and edit its location and metadata.")).weak().size(11.0));
            return;
        }
        let ids: Vec<u64> = photos.iter().map(|p| p.id.0).collect();
        if photos.len() > 1 {
            ui.label(
                RichText::new(format!(
                    "{} {} · {} {}",
                    photos.len(),
                    tr("selected photos"),
                    tr("differing values show"),
                    crate::panels::metadata::MIXED
                ))
                .weak()
                .size(11.0),
            );
        }
        crate::panels::metadata::gps(app, ui, &photos, &ids);
        for (key, label, lines) in [
            ("location", "Sublocation", 1usize),
            ("city", "City", 1),
            ("state", "State / Region", 1),
            ("country", "Country", 1),
            ("countryCode", "Country Code", 1),
        ] {
            crate::panels::metadata::text_field(app, ui, &photos, &ids, label, key, lines);
        }
        ui.separator();
        for (key, label, lines) in [("title", "Title", 1usize), ("headline", "Headline", 1), ("caption", "Caption", 3), ("keywords", "Keywords", 2)] {
            crate::panels::metadata::text_field(app, ui, &photos, &ids, label, key, lines);
        }
    });
}

fn style_section(app: &mut LccApp, ui: &mut egui::Ui) {
    section(ui, "Map Style", true, |ui| {
        let servers = app.map.servers();
        let cur = app.map.server();
        egui::ComboBox::from_id_salt("map-style").selected_text(cur.name.clone()).width(240.0).show_ui(ui, |ui| {
            for s in &servers {
                if ui.selectable_label(s.id == cur.id, &s.name).clicked() {
                    let _ = app.run("map.style", json!({"id": s.id}));
                }
            }
        });
        egui::CollapsingHeader::new(tr("Add Tile Server")).id_salt("map-add-server").show(ui, |ui| {
            let (n, u, a) = &mut app.map.new_server;
            let r = ui.add(egui::TextEdit::singleline(n).hint_text(tr("Name")));
            crate::access::label(&r, "Name");
            let r = ui.add(egui::TextEdit::singleline(u).hint_text("https://…/{z}/{x}/{y}.png"));
            crate::access::label(&r, "Tile URL");
            let r = ui.add(egui::TextEdit::singleline(a).hint_text(tr("Attribution (shown on the map)")));
            crate::access::label(&r, "Attribution (shown on the map)");
            if ui.button(tr("Add")).clicked() {
                let (n, u, a) = app.map.new_server.clone();
                match app.run("map.addServer", json!({"name": n, "url": u, "attribution": a})) {
                    Ok(_) => app.map.new_server = Default::default(),
                    Err(e) => app.toast_error(ui.ctx(), e),
                }
            }
        });
        if let Some((bytes, files)) = app.map.tiles.cache_usage() {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(crate::i18n::tr_format!("Tile cache: {mb} MB, {n} tiles", mb = bytes / (1024 * 1024), n = files)).weak().size(11.0),
                );
                if ui.small_button(tr("Clear")).clicked() {
                    let _ = app.run("map.tileCache", json!({"clear": true}));
                }
            });
        }
    });
}

fn locations_section(app: &mut LccApp, ui: &mut egui::Ui) {
    section(ui, "Saved Locations", true, |ui| {
        let list = app.run("map.locations", json!({})).ok().and_then(|v| v.field("locations").as_array().cloned()).unwrap_or_default();
        let picked: Vec<u64> = app.session.selected().iter().map(|id| id.0).collect();
        for l in &list {
            let name = l["name"].as_str().unwrap_or("").to_string();
            let private = l["private"].as_bool().unwrap_or(false);
            let row = ui.horizontal(|ui| {
                // as in Lightroom Classic: ticked when the selected photos are all inside, mixed when
                // some are; ticking it moves the selected photos to the location's centre
                let here = l["selected"].as_u64().unwrap_or(0) as usize;
                let all = !picked.is_empty() && here == picked.len();
                let mut ticked = all;
                let tip = if picked.is_empty() {
                    tr("Select photos to move them to this location")
                } else if all {
                    tr("The selected photos are in this location")
                } else {
                    tr("Move the selected photos to this location")
                };
                let b = ui
                    .add_enabled(!picked.is_empty(), egui::Checkbox::without_text(&mut ticked).indeterminate(here > 0 && !all))
                    .on_hover_text(tip)
                    .on_disabled_hover_text(tip);
                crate::widgets::register(ui.ctx(), format!("map:locationHere:{name}"), b.rect);
                if b.clicked() && !all {
                    place_at_location(app, ui.ctx(), picked.clone(), &name);
                }
                let label = format!("{}{}  ({})", if private { "🔒 " } else { "" }, name, l["photos"].as_u64().unwrap_or(0));
                let link = ui.link(label);
                let link = if private { link.on_hover_text(tr("Private: photos here are exported without their location")) } else { link };
                if link.clicked() {
                    app.map.active_location = Some(name.clone());
                    let _ = app.run("map.search", json!({"query": name}));
                }
                let b = ui.small_button(tr("Edit…")).on_hover_text(tr("Edit this saved location"));
                crate::widgets::register(ui.ctx(), format!("map:locationEdit:{name}"), b.rect);
                if b.clicked() {
                    match super::open_edit_location(app, &name) {
                        Ok(()) => {
                            let _ = app.run("map.search", json!({"query": name}));
                        }
                        Err(e) => app.toast_error(ui.ctx(), e),
                    }
                }
                if ui.small_button("×").on_hover_text(tr("Delete this saved location")).clicked() {
                    let _ = app.run("map.deleteLocation", json!({"name": name}));
                }
            });
            crate::widgets::register(ui.ctx(), format!("map:locationRow:{name}"), row.response.rect);
            location_drop(app, ui, row.response.rect, &name);
        }
        if list.is_empty() {
            ui.label(RichText::new(tr("Right-click the map and choose New Location Here…, or add one at the map's centre.")).weak().size(11.0));
        }
        ui.separator();
        let b = ui.button(tr("New Location at Map Centre…"));
        crate::widgets::register(ui.ctx(), "map:newLocationAtCentre", b.rect);
        if b.clicked() {
            let c = app.map.centre();
            super::open_new_location(app, c, None);
        }
    });
}

/// Photos dragged from the filmstrip or the grid and dropped on a saved location's row take its
/// centre as their position (one undo step), as dropping them on the map does at that point.
fn location_drop(app: &mut LccApp, ui: &mut egui::Ui, rect: egui::Rect, name: &str) {
    let Some(ids) = app.ui.dragging_photos.clone() else { return };
    let (pos, released) = ui.input(|i| (i.pointer.latest_pos(), i.pointer.any_released()));
    if !pos.is_some_and(|p| rect.contains(p)) {
        return;
    }
    if app.session.catalog.saved_location(name).is_none() {
        return;
    }
    let t = crate::theme::Tokens::get(ui.ctx());
    ui.painter().rect_stroke(rect.expand(2.0), 4.0, egui::Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
    crate::panels::grid::drag_hint(ui.ctx(), crate::i18n::tr_format!("Place at {name}", name = name));
    if released {
        app.ui.dragging_photos = None;
        place_at_location(app, ui.ctx(), ids, name);
    }
}

/// Puts the photos at a saved location's centre (one undo step). The overlay says what changed; only
/// a refusal is a toast.
fn place_at_location(app: &mut LccApp, ctx: &egui::Context, ids: Vec<u64>, name: &str) {
    let Some(centre) = app.session.catalog.saved_location(name).map(|l| l.centre()) else { return };
    if let Err(e) = app.run("map.geotag", json!({"ids": ids, "lat": centre.lat, "lon": centre.lon})) {
        app.toast_error(ctx, e);
    }
}

fn track_section(app: &mut LccApp, ui: &mut egui::Ui) {
    section(ui, "Tracklog", true, |ui| {
        if ui.button(tr("Load Tracklog…")).clicked()
            && let Err(e) = app.run("map.loadTrack", json!({}))
        {
            app.toast_error(ui.ctx(), e);
        }
        let mut remove = None;
        for (i, tr_) in app.map.tracks.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.checkbox(&mut tr_.shown, "");
                ui.label(format!("{} ({})", tr_.track.name, tr_.track.points()));
                if ui.small_button("×").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove
            && i < app.map.tracks.len()
        {
            app.map.tracks.remove(i);
        }
        if app.map.tracks.iter().any(|t| t.track.timed) {
            ui.horizontal(|ui| {
                ui.label(tr("Camera time zone"));
                let r = ui.add(egui::TextEdit::singleline(&mut app.map.offset).hint_text("+02:00").desired_width(70.0));
                crate::access::label(&r, "Camera time zone");
                if r.has_focus() {
                    app.text_focus = true;
                }
            });
            if ui
                .button(tr("Set Offset from Active Photo"))
                .on_hover_text(tr("The active photo was taken where it is geotagged (or at the map's centre): match it to the track"))
                .clicked()
            {
                let path = app.map.tracks.iter().rev().find(|t| t.track.timed).map(|t| t.path.clone()).unwrap_or_default();
                let mut p = json!({"path": path});
                if let Some(id) = app.session.active() {
                    let has_gps = app.session.catalog.photo(id).is_some_and(|ph| ph.meta.gps.is_some());
                    *p.field_mut("id") = json!(id.0);
                    if !has_gps {
                        let c = app.map.centre();
                        *p.field_mut("lat") = json!(c.lat);
                        *p.field_mut("lon") = json!(c.lon);
                    }
                }
                match app.run("map.trackOffset", p) {
                    Ok(v) => app.map.offset = v.field("offset").as_str().unwrap_or("").to_string(),
                    Err(e) => app.toast_error(ui.ctx(), e),
                }
            }
            let has_sel = !app.session.selection.ids.is_empty();
            if ui.add_enabled(has_sel, egui::Button::new(tr("Auto-Tag Selected Photos"))).clicked() {
                match app.run("map.autoTag", json!({})) {
                    Ok(v) => {
                        let msg = crate::i18n::tr_format!("Tagged {n} photos from the tracklog", n = v.field("tagged").as_u64().unwrap_or(0))
                            + &crate::menus::unplaceable_note(v.field("skipped"));
                        app.toast(ui.ctx(), msg)
                    }
                    Err(e) => app.toast_error(ui.ctx(), e),
                }
            }
        }
    });
}

fn geocode_section(app: &mut LccApp, ui: &mut egui::Ui) {
    section(ui, "Reverse Geocoding", false, |ui| {
        ui.label(RichText::new(tr("Fills Sublocation, City, State and Country from GPS. Off until you choose a provider.")).weak().size(11.0));
        let cur = app.map.prefs.geocoder.clone();
        for (key, label) in [("off", "Off"), ("offline", "Offline (GeoNames place list)"), ("online", "Online (Nominatim-compatible server)")] {
            if ui.radio(cur == key, tr(label)).clicked() {
                let _ = app.run("map.geocoder", json!({"provider": key}));
            }
        }
        match cur.as_str() {
            "offline" => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let st = app.run("map.geocodeStatus", json!({})).unwrap_or_default();
                    let have = st.field("offline").field("downloaded").as_bool().unwrap_or(false);
                    ui.label(RichText::new(lcc_geo::geocode::GEONAMES_ATTRIBUTION).weak().size(11.0));
                    let busy = app.map.jobs.iter().any(|j| matches!(j, Job::Download(_)));
                    let label =
                        if have { tr("Update Place Names (from geonames.org)") } else { tr("Download Place Names (from geonames.org, ~3 MB)") };
                    if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
                        start_download(app);
                    }
                }
            }
            "online" => {
                let r = ui.add(egui::TextEdit::singleline(&mut app.map.prefs.endpoint).desired_width(240.0));
                crate::access::label(&r, "Geocoding server");
                if r.has_focus() {
                    app.text_focus = true;
                }
                if r.lost_focus() {
                    app.map.save();
                }
                let mut consent = app.map.prefs.online_consent;
                if ui.checkbox(&mut consent, tr("Allow sending photo positions and searches to this server")).changed() {
                    let _ = app.run("map.geocoder", json!({"consent": consent}));
                }
                ui.label(
                    RichText::new(format!("{} {}", tr("At most one request per second."), lcc_geo::geocode::NOMINATIM_ATTRIBUTION)).weak().size(11.0),
                );
            }
            _ => {}
        }
        let busy = app.map.jobs.iter().any(|j| matches!(j, Job::Geocode(_)));
        let can = cur != "off" && !app.session.selection.ids.is_empty() && !busy;
        if ui.add_enabled(can, egui::Button::new(tr("Look Up Selected Photos"))).clicked() {
            match app.run("map.lookup", json!({})) {
                Ok(v) if v.get("pending").is_some() => app.toast(ui.ctx(), tr("Looking up locations…")),
                Ok(v) if v.field("found").as_u64().unwrap_or(0) == 0 => {
                    app.toast(ui.ctx(), crate::i18n::tr_format!("Filled the location of {n} photos", n = 0))
                }
                Ok(_) => {}
                Err(e) => app.toast_error(ui.ctx(), e),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinates_parse() {
        assert_eq!(parse_coords("48.85, 2.35"), Some(LatLon::new(48.85, 2.35)));
        assert_eq!(parse_coords("-33.9 151.2"), Some(LatLon::new(-33.9, 151.2)));
        assert_eq!(parse_coords("Paris"), None);
        assert_eq!(parse_coords("91, 0"), None);
        assert_eq!(parse_coords("1, 2, 3"), None);
    }
}
