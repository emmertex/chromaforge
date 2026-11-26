//! Headless tests of the Map module against a fake tile server on 127.0.0.1 (no external
//! network): tiles load and draw with the attribution, pins appear for geotagged photos, a photo
//! dragged from the filmstrip onto the map is geotagged where it lands, a pin dragged moves its
//! photos, saved locations and the location filter work, and track logs load.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::Duration;

use serde_json::{Value, json};

use crate::headless::Headless;
use crate::{LccApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

/// A tile server answering every request with a solid red 256² PNG (until the test ends).
fn fake_tiles() -> u16 {
    let mut png = Vec::new();
    image::RgbaImage::from_pixel(256, 256, image::Rgba([220, 30, 30, 255]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { continue };
            let png = png.clone();
            std::thread::spawn(move || {
                let mut r = BufReader::new(s.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                        break;
                    }
                }
                let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\n\r\n", png.len()).as_bytes());
                let _ = s.write_all(&png);
            });
        }
    });
    port
}

fn demo() -> Headless {
    let services = Services { png: None, ..Default::default() };
    let app = LccApp::new(lcc_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    h.settle(SETTLE);
    h
}

fn run(h: &mut Headless, id: &str, params: Value) -> Value {
    let r = h.request("engine.execute", json!({"command": id, "params": params}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.step();
    r["result"].clone()
}

fn widget(h: &Headless, id: &str) -> Option<egui::Rect> {
    h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r)
}

fn map_on_fake_tiles() -> Headless {
    let port = fake_tiles();
    let mut h = demo();
    run(&mut h, "module.map", json!({}));
    run(
        &mut h,
        "map.addServer",
        json!({"name": "Fake", "url": format!("http://127.0.0.1:{port}/{{z}}/{{x}}/{{y}}.png"), "attribution": "© Fake Tiles"}),
    );
    run(&mut h, "map.view", json!({"lat": 48.85, "lon": 2.35, "zoom": 12}));
    h.settle(SETTLE);
    h
}

#[test]
fn tiles_draw_with_attribution_and_pins_cluster() {
    let mut h = map_on_fake_tiles();
    let img = h.snapshot(T);
    let canvas = widget(&h, "map:canvas").expect("the map canvas is on screen");
    assert!(widget(&h, "view:module:map").is_some() && widget(&h, "map:attribution").is_some() && widget(&h, "map:side").is_some());
    // a tile pixel (away from pins, buttons and the attribution) is the fake server's red
    let p = canvas.center() + egui::vec2(-150.0, 120.0);
    let c = img.pixels[p.y as usize * img.width() + p.x as usize];
    assert!(c.r() > 180 && c.g() < 80 && c.b() < 80, "tile pixel {c:?}");
    let v = run(&mut h, "map.view", json!({}));
    assert!(v["tiles"]["loaded"].as_u64().unwrap() > 0, "{v}");
    assert_eq!(v["attribution"], "© Fake Tiles");

    // two photos near each other and one far: two pins at zoom 4, three at zoom 16
    let ids: Vec<u64> = h.app.session.visible_cloned().iter().take(3).map(|p| p.0).collect();
    run(&mut h, "map.geotag", json!({"ids": [ids[0], ids[1]], "lat": 48.8566, "lon": 2.3522}));
    run(&mut h, "map.geotag", json!({"ids": [ids[2]], "lat": 48.8600, "lon": 2.3400}));
    run(&mut h, "map.view", json!({"lat": 48.858, "lon": 2.346, "zoom": 15}));
    h.settle(SETTLE);
    let v = run(&mut h, "map.view", json!({}));
    assert_eq!(v["pins"].as_array().unwrap().len(), 2, "{v}");
    run(&mut h, "map.view", json!({"zoom": 6}));
    h.settle(SETTLE);
    let v = run(&mut h, "map.view", json!({}));
    let pins = v["pins"].as_array().unwrap();
    assert_eq!(pins.len(), 1, "{v}");
    assert_eq!(pins[0]["ids"].as_array().unwrap().len(), 3);
    // clicking the cluster selects its photos
    h.request("ui.clickWidget", json!({"id": format!("map:pin:{}", pins[0]["ids"][0])}), T);
    h.settle(SETTLE);
    assert_eq!(h.app.session.selection.ids.len(), 3);
    // map.fit frames them
    let v = run(&mut h, "map.fit", json!({}));
    assert!(v["zoom"].as_f64().unwrap() > 10.0, "{v}");
    // the +/- keys' command: one level at a time, clamped at the ends
    run(&mut h, "map.view", json!({"lat": 48.85, "lon": 2.35, "zoom": 12}));
    assert_eq!(run(&mut h, "map.view", json!({"zoomBy": 1}))["zoom"].as_f64(), Some(13.0));
    assert_eq!(run(&mut h, "map.view", json!({"zoomBy": -2}))["zoom"].as_f64(), Some(11.0));
    assert_eq!(run(&mut h, "map.view", json!({"zoomBy": -100}))["zoom"].as_f64(), Some(1.0), "clamped");
}

#[test]
fn dragging_from_the_filmstrip_and_moving_pins_geotags() {
    let mut h = map_on_fake_tiles();
    let id = h.app.session.visible_cloned()[0].0;
    assert!(h.app.session.catalog.photo(lcc_catalog::PhotoId(id)).unwrap().meta.gps.is_none());
    let canvas = widget(&h, "map:canvas").unwrap();
    assert!(widget(&h, &format!("film:{id}")).is_some(), "the filmstrip shows the photo");
    let to = canvas.center();
    let r = h.request("ui.dragWidget", json!({"id": format!("film:{id}"), "toX": to.x, "toY": to.y, "steps": 12}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let gps = h.app.session.catalog.photo(lcc_catalog::PhotoId(id)).unwrap().meta.gps.expect("dropped on the map: geotagged");
    assert!((gps.0 - 48.85).abs() < 0.01 && (gps.1 - 2.35).abs() < 0.01, "{gps:?}");
    // drag its pin 100 px east: the photo moves with it
    let r = h.request("ui.dragWidget", json!({"id": format!("map:pin:{id}"), "dx": 100.0, "dy": 0.0, "steps": 12}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let moved = h.app.session.catalog.photo(lcc_catalog::PhotoId(id)).unwrap().meta.gps.unwrap();
    assert!(moved.1 > gps.1 + 0.005 && (moved.0 - gps.0).abs() < 0.005, "{gps:?} → {moved:?}");
    // undo puts it back
    run(&mut h, "edit.undo", json!({}));
    assert_eq!(h.app.session.catalog.photo(lcc_catalog::PhotoId(id)).unwrap().meta.gps, Some(gps));
    // geotagAt (control channel) at a screen point
    run(&mut h, "map.geotagAt", json!({"ids": [id], "x": to.x, "y": to.y}));
}

#[test]
fn saved_locations_filter_and_tracks() {
    let dir = lcc_testkit::TempDir::new("lcc-map-ui");
    let mut h = map_on_fake_tiles();
    let ids: Vec<u64> = h.app.session.visible_cloned().iter().take(2).map(|p| p.0).collect();
    run(&mut h, "map.geotag", json!({"ids": [ids[0]], "lat": 48.8566, "lon": 2.3522}));
    run(&mut h, "map.saveLocation", json!({"name": "Home", "lat": 48.8566, "lon": 2.3522, "radius": 300, "private": true}));
    run(&mut h, "map.filter", json!({"filter": "location:Home"}));
    h.settle(SETTLE);
    let v = run(&mut h, "map.view", json!({}));
    assert_eq!(v["filter"], "location:Home");
    assert_eq!(v["pins"].as_array().unwrap().len(), 1);
    let r = h.request("engine.execute", json!({"command": "map.filter", "params": {"filter": "location:Nowhere"}}), T);
    assert_eq!(r["ok"], false);
    run(&mut h, "map.filter", json!({"filter": "untagged"}));
    h.settle(SETTLE);
    assert!(run(&mut h, "map.view", json!({}))["pins"].as_array().unwrap().is_empty());
    run(&mut h, "map.filter", json!({"filter": "all"}));
    // search finds the saved location
    let r = run(&mut h, "map.search", json!({"query": "home"}));
    assert_eq!(r["found"], "savedLocation");
    // online search without consent is refused (nothing is sent)
    let r = h.request("engine.execute", json!({"command": "map.search", "params": {"query": "Paris"}}), T);
    assert_eq!(r["ok"], false, "{r}");
    // a GPX track loads, frames the view and draws
    std::fs::create_dir_all(&dir).unwrap();
    let gpx = dir.join("walk.gpx");
    std::fs::write(
        &gpx,
        r#"<gpx><trk><trkseg><trkpt lat="46.0" lon="7.0"><time>2026-05-01T10:00:00Z</time></trkpt>
        <trkpt lat="46.01" lon="7.02"><time>2026-05-01T10:10:00Z</time></trkpt></trkseg></trk></gpx>"#,
    )
    .unwrap();
    let r = run(&mut h, "map.loadTrack", json!({"path": gpx.to_string_lossy()}));
    assert_eq!((r["points"].as_u64(), r["timed"].as_bool()), (Some(2), Some(true)));
    let v = run(&mut h, "map.view", json!({}));
    assert!((v["lat"].as_f64().unwrap() - 46.005).abs() < 0.01, "{v}");
    h.settle(SETTLE);
    let _ = h.snapshot(T);
    // bad input: errors, not panics
    for (cmd, p) in [
        ("map.loadTrack", json!({"path": "/no/such.gpx"})),
        ("map.view", json!({"lat": 1000})),
        ("map.style", json!({"id": "nope"})),
        ("map.addServer", json!({"name": "x", "url": "ftp://x", "attribution": "a"})),
        ("map.geocoder", json!({"provider": "magic"})),
        ("map.lookup", json!({})),
    ] {
        let r = h.request("engine.execute", json!({"command": cmd, "params": p}), T);
        assert_eq!(r["ok"], false, "{cmd}: {r}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// P6.2: a damaged or hand-edited `map.json` (hostile numbers, bad tile servers, wrong types)
/// either fails to parse (defaults are used) or is sanitized into range; never a panic.
#[test]
fn hostile_map_json_is_sanitized() {
    use super::{MapPrefs, MapUi};
    let nums = ["0", "-0", "1e308", "-1e308", "90.0001", "-180.5", "1e-320", "22", "99999999999999999999", "-1"];
    let servers = [
        r#"[]"#,
        r#"[{"id":"x","name":"x","url":"file:///etc/passwd","attribution":"","maxZoom":19}]"#,
        r#"[{"id":"","name":"","url":"https://t/{z}/{x}/{y}.png","attribution":"","maxZoom":255}]"#,
        r#"[{"id":"osm","name":"dup","url":"https://t/{z}/{x}/{y}.png","attribution":"a","maxZoom":19,"kind":"satellite"}]"#,
        r#"{"not":"a list"}"#,
    ];
    let mut n = 0;
    for (i, lat) in nums.iter().enumerate() {
        for lon in nums.iter().step_by(3) {
            for zoom in nums.iter().skip(i % 3).step_by(2) {
                for (k, sv) in servers.iter().enumerate() {
                    let style = ["osm", "", "x", "\\u0000"][k % 4];
                    let text = format!(
                        r#"{{"lat":{lat},"lon":{lon},"zoom":{zoom},"style":"{style}","servers":{sv},"geocoder":"online","endpoint":"","onlineConsent":true,"cacheMb":{}}}"#,
                        nums[k]
                    );
                    let Ok(prefs) = serde_json::from_str::<MapPrefs>(&text) else { continue };
                    n += 1;
                    let mut m = MapUi { prefs, ..MapUi::default() };
                    m.sanitize();
                    let p = &m.prefs;
                    let max = f64::from(lcc_geo::mercator::MAX_ZOOM);
                    assert!(lcc_geo::LatLon::new(p.lat, p.lon).is_valid() && (1.0..=max).contains(&p.zoom), "{text}");
                    assert!(m.servers().iter().any(|s| s.id == p.style), "{text}");
                    assert!(p.servers.iter().all(|s| s.validate().is_ok()));
                    let _ = serde_json::to_string(p);
                }
            }
        }
    }
    assert!(n > 50, "only {n} variants parsed");
}

/// P6.1 budget probe (`cargo test --release -p lcc-ui-egui map_50k -- --ignored --nocapture`):
/// 50,000 geotagged photos spread over Europe; prints the clustering time per zoom step and the
/// steady-state frame time of the Map view (pins cached) at a few zooms. Numbers go in docs/perf.md.
#[test]
#[ignore = "perf probe"]
fn map_50k_pins_frame_time() {
    use lcc_catalog::{Op, Photo, Source};
    use std::time::Instant;
    let port = fake_tiles();
    let mut h = demo();
    for i in 0..50_000u64 {
        let id = h.app.session.catalog.alloc_photo_id();
        let mut p = Photo::new(id, Source::File { path: format!("/x/{i}.jpg") }, &format!("{i}.jpg"), "JPEG", 6000, 4000, "2026-01-01T00:00:00");
        // deterministic scatter over 36..60 N, -10..30 E
        let a = (i.wrapping_mul(2_654_435_761) % 10_000) as f64 / 10_000.0;
        let b = (i.wrapping_mul(40_503) % 10_007) as f64 / 10_007.0;
        p.meta.gps = Some((36.0 + 24.0 * a, -10.0 + 40.0 * b));
        h.app.session.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    }
    run(&mut h, "module.map", json!({}));
    run(&mut h, "map.addServer", json!({"name": "Fake", "url": format!("http://127.0.0.1:{port}/{{z}}/{{x}}/{{y}}.png"), "attribution": "© Fake"}));
    for zoom in [3.0, 5.0, 8.0, 12.0] {
        run(&mut h, "map.view", json!({"lat": 48.0, "lon": 10.0, "zoom": zoom}));
        h.settle(SETTLE);
        h.app.map.pins_cache = None;
        let t = Instant::now();
        let pins = h.app.map.pins(&mut h.app.session);
        let cluster_ms = t.elapsed().as_secs_f64() * 1e3;
        h.step();
        let frames = 30;
        let t = Instant::now();
        for _ in 0..frames {
            h.step();
        }
        let frame_ms = t.elapsed().as_secs_f64() * 1e3 / f64::from(frames);
        let t = Instant::now();
        for _ in 0..frames {
            drop(h.app.map.pins(&mut h.app.session));
        }
        let cached_us = t.elapsed().as_secs_f64() * 1e6 / f64::from(frames);
        println!(
            "map 50k zoom {zoom}: {} pins, {} drawn, cluster {cluster_ms:.1} ms, cached pins {cached_us:.1} µs, frame {frame_ms:.2} ms",
            pins.len(),
            h.app.map.drawn.len()
        );
    }
    // the same frame loop outside the Map (what the rest of the shell costs with 50k photos)
    run(&mut h, "module.library", json!({}));
    h.settle(SETTLE);
    let t = Instant::now();
    for _ in 0..30 {
        h.step();
    }
    println!("library 50k: frame {:.2} ms", t.elapsed().as_secs_f64() * 1e3 / 30.0);
}

#[test]
fn context_menu_geotags_where_it_was_opened() {
    // #323: the menu read its position from the hover each frame, so it emptied (and dropped the
    // click) once the pointer left the map for the menu
    let mut h = map_on_fake_tiles();
    let id = h.app.session.visible_cloned()[0].0;
    h.app.session.selection = lcc_engine::Selection::single(lcc_catalog::PhotoId(id));
    let canvas = widget(&h, "map:canvas").unwrap();
    let at = canvas.center() + egui::vec2(-120.0, -80.0);
    let r = h.request("ui.click", json!({"x": at.x, "y": at.y, "button": "right"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let item = widget(&h, "map:menu:geotag").expect("the context menu is open");
    // the pointer travels from the map onto the menu, then clicks
    for i in 1..=6 {
        let p = at + (item.center() - at) * (i as f32 / 6.0);
        h.request("ui.move", json!({"x": p.x, "y": p.y}), T);
        h.step();
    }
    assert!(widget(&h, "map:menu:geotag").is_some(), "the menu keeps its items under the pointer");
    let r = h.request("ui.clickWidget", json!({"id": "map:menu:geotag"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let gps = h.app.session.catalog.photo(lcc_catalog::PhotoId(id)).unwrap().meta.gps.expect("geotagged from the menu");
    let want = super::view::screen_to_geo(&h.app.map, canvas, at);
    assert!((gps.0 - want.lat).abs() < 1e-4 && (gps.1 - want.lon).abs() < 1e-4, "{gps:?} vs {want:?}");
}

#[test]
fn a_long_press_opens_the_context_menu_where_it_was_pressed() {
    // a touch long-press opens the menu too; egui counts it as a secondary click (`Response::secondary_clicked`),
    // so the place is kept then as well, not only on a right-click
    let mut h = map_on_fake_tiles();
    let canvas = widget(&h, "map:canvas").unwrap();
    let at = canvas.center() + egui::vec2(90.0, 60.0);
    let touch = |phase| egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(1), phase, pos: at, force: None };
    h.events.push(touch(egui::TouchPhase::Start));
    h.events.push(egui::Event::PointerMoved(at));
    h.events.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    // held still past egui's long-press time
    for _ in 0..90 {
        h.step();
    }
    h.events.push(touch(egui::TouchPhase::End));
    h.events.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.settle(SETTLE);
    let want = super::view::screen_to_geo(&h.app.map, canvas, at);
    let got = h.app.map.menu_at.expect("the long-press place is kept for the menu");
    assert!((got.lat - want.lat).abs() < 1e-6 && (got.lon - want.lon).abs() < 1e-6, "{got:?} vs {want:?}");
    assert!(widget(&h, "map:menu:geotag").is_some(), "the menu has its items");
}

#[test]
fn zooming_over_a_pin_keeps_zooming_and_suppresses_hover_preview() {
    // #740: hovering a pin stole hover from the map canvas, interrupting wheel / trackpad zoom
    let mut h = map_on_fake_tiles();
    let id = h.app.session.visible_cloned()[0].0;
    run(&mut h, "map.geotag", json!({"ids": [id], "lat": 48.85, "lon": 2.35}));
    h.settle(SETTLE);
    let pin = widget(&h, &format!("map:pin:{id}")).expect("the pin is on screen");
    let at = pin.center();
    h.events.push(egui::Event::PointerMoved(at));
    h.step();
    assert!(widget(&h, "map:hover").is_some(), "hovering a pin while idle shows its thumbnail preview");

    let z0 = h.app.map.prefs.zoom;
    h.events.push(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, 200.0),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
    });
    h.step();
    assert!(h.app.map.prefs.zoom > z0 + 0.1, "scroll zooms in over a pin: {z0} -> {}", h.app.map.prefs.zoom);
    assert!(widget(&h, "map:hover").is_none(), "thumbnail preview is suppressed while zooming");

    // Let smooth scroll decay before testing pinch zoom
    for _ in 0..30 {
        h.step();
    }
    assert!(widget(&h, "map:hover").is_some(), "hover preview reappears when scroll zooming stops");

    let z1 = h.app.map.prefs.zoom;
    h.events.push(egui::Event::Zoom(0.5));
    h.step();
    assert!(h.app.map.prefs.zoom < z1 - 0.5, "trackpad pinch zooms out over a pin: {z1} -> {}", h.app.map.prefs.zoom);
    assert!(widget(&h, "map:hover").is_none(), "thumbnail preview is suppressed during pinch zoom");

    // Once zooming stops, the hover preview returns.
    h.step();
    assert!(widget(&h, "map:hover").is_some(), "hover preview reappears when zooming stops");
}

/// Fork #741: a filmstrip selection whose pins lie off screen pans the map to them (without
/// zooming in), a selection already on screen leaves the view alone, `map.reveal {force}` (the
/// filmstrip's double-click) frames and zooms in even then, and photos without a location don't
/// move the map.
#[test]
fn selecting_offscreen_photos_reveals_their_pins() {
    let mut h = map_on_fake_tiles();
    let ids: Vec<u64> = h.app.session.visible_cloned().iter().take(3).map(|p| p.0).collect();
    run(&mut h, "map.geotag", json!({"ids": [ids[0]], "lat": 48.8566, "lon": 2.3522}));
    run(&mut h, "map.geotag", json!({"ids": [ids[1]], "lat": -33.87, "lon": 151.21}));
    run(&mut h, "map.view", json!({"lat": 48.85, "lon": 2.35, "zoom": 12}));
    h.settle(SETTLE);
    // the pin is already in view: selecting it doesn't move the map
    run(&mut h, "library.select", json!({"ids": [ids[0]]}));
    h.settle(SETTLE);
    let v = run(&mut h, "map.view", json!({}));
    assert!((v["lat"].as_f64().unwrap() - 48.85).abs() < 0.01, "{v}");
    // Sydney is off screen: selecting it pans there, at the same zoom
    run(&mut h, "library.select", json!({"ids": [ids[1]]}));
    h.settle(SETTLE);
    let v = run(&mut h, "map.view", json!({}));
    assert!((v["lat"].as_f64().unwrap() + 33.87).abs() < 0.05 && (v["lon"].as_f64().unwrap() - 151.21).abs() < 0.05, "{v}");
    assert!((v["zoom"].as_f64().unwrap() - 12.0).abs() < 0.01, "reveal pans without zooming in: {v}");
    // force (a double-clicked thumbnail): frames the pin and zooms in on it
    let v = run(&mut h, "map.reveal", json!({"force": true}));
    assert!((v["lat"].as_f64().unwrap() + 33.87).abs() < 0.05, "{v}");
    assert!(v["zoom"].as_f64().unwrap() > 12.0, "{v}");
    // a photo without a location: the map stays
    run(&mut h, "map.view", json!({"lat": 10.0, "lon": 10.0, "zoom": 5.0}));
    h.settle(SETTLE);
    run(&mut h, "library.select", json!({"ids": [ids[2]]}));
    h.settle(SETTLE);
    let v = run(&mut h, "map.view", json!({}));
    assert!((v["lat"].as_f64().unwrap() - 10.0).abs() < 0.01, "{v}");
    // nothing selected: reveal is a no-op, not an error
    run(&mut h, "library.select", json!({"ids": []}));
    let v = run(&mut h, "map.reveal", json!({}));
    assert_eq!(v["lat"].as_f64().unwrap(), 10.0);
}

/// Fork #784: the Map side panel shows the selected photos' location and IPTC fields, and edits
/// apply to every selected photo (the Library Metadata panel's widgets on `photo.setMeta`).
#[test]
fn the_side_panel_shows_and_edits_the_selection_metadata() {
    let mut h = map_on_fake_tiles();
    let ids: Vec<u64> = h.app.session.visible_cloned().iter().take(2).map(|p| p.0).collect();
    run(&mut h, "map.geotag", json!({"ids": ids, "lat": 48.8566, "lon": 2.3522}));
    run(&mut h, "library.select", json!({"ids": ids}));
    h.settle(SETTLE);
    for field in [
        "field:gps",
        "field:location",
        "field:city",
        "field:state",
        "field:country",
        "field:title",
        "field:headline",
        "field:caption",
        "field:keywords",
    ] {
        assert!(widget(&h, field).is_some(), "{field} is on the side panel");
    }
    // an edit reaches both photos of the pin (one undo step)
    h.request("ui.clickWidget", json!({"id": "field:city"}), T);
    h.settle(SETTLE);
    h.request("ui.text", json!({"text": "Paris"}), T);
    h.settle(SETTLE);
    h.request("ui.key", json!({"key": "Enter"}), T);
    h.settle(SETTLE);
    for id in ids {
        assert_eq!(h.app.session.catalog.photo(lcc_catalog::PhotoId(id)).unwrap().meta.city, "Paris", "the edit reached every photo at the pin");
    }
    // no selection: the section says so instead of showing fields
    run(&mut h, "library.select", json!({"ids": []}));
    h.settle(SETTLE);
    assert!(widget(&h, "field:city").is_none(), "no fields without a selection");
}
/// Fork #783: the Location Filter bar filters the filmstrip, as Lightroom Classic's does, not only
/// the pins: Untagged hides the geotagged photos, Tagged keeps only them, Visible on Map keeps the
/// photos positioned inside the map view, and None shows every photo again.
#[test]
fn location_filter_filters_the_filmstrip() {
    let mut h = map_on_fake_tiles();
    let ids: Vec<u64> = h.app.session.visible_cloned().iter().take(2).map(|p| p.0).collect();
    run(&mut h, "map.geotag", json!({"ids": [ids[0]], "lat": 48.8566, "lon": 2.3522}));
    h.settle(SETTLE);
    let film = |h: &Headless, id: u64| widget(h, &format!("film:{id}")).is_some();
    assert!(film(&h, ids[0]) && film(&h, ids[1]), "no filter: every photo is in the strip");
    run(&mut h, "map.filter", json!({"filter": "untagged"}));
    h.settle(SETTLE);
    assert!(!film(&h, ids[0]) && film(&h, ids[1]), "Untagged hides the geotagged photo");
    run(&mut h, "map.filter", json!({"filter": "tagged"}));
    h.settle(SETTLE);
    assert!(film(&h, ids[0]) && !film(&h, ids[1]), "Tagged keeps only the geotagged photo");
    // Visible on Map: the photo is in the strip while Paris is in view, not when the map shows Sydney
    run(&mut h, "map.filter", json!({"filter": "visible"}));
    run(&mut h, "map.view", json!({"lat": 48.85, "lon": 2.35, "zoom": 12}));
    h.settle(SETTLE);
    assert!(film(&h, ids[0]) && !film(&h, ids[1]), "Visible on Map keeps the photo in view");
    run(&mut h, "map.view", json!({"lat": -33.87, "lon": 151.21, "zoom": 12}));
    h.settle(SETTLE);
    assert!(!film(&h, ids[0]) && !film(&h, ids[1]), "Visible on Map drops the photo out of view");
    run(&mut h, "map.filter", json!({"filter": "all"}));
    h.settle(SETTLE);
    assert!(film(&h, ids[0]) && film(&h, ids[1]), "None shows every photo again");
}

#[test]
fn info_and_key_boxes_follow_their_menu_toggles() {
    let mut h = map_on_fake_tiles();
    // the offline place list names Paris for a view centred there
    let cities = "1\tParis\tParis\t\t48.85\t2.35\tP\tPPLC\tFR\t\t11\n";
    let names = lcc_geo::geocode::GeoNames::parse(cities, Some("FR.11\tIle-de-France\n"), Some("FR\tFRA\t250\tFR\tFrance\n"));
    h.app.map.names = Some(Some(std::sync::Arc::new(names)));
    h.app.map.centre_name = None;
    assert!(widget(&h, "map:info").is_none() && widget(&h, "map:key").is_none(), "both start hidden");
    assert_eq!(crate::menubar::checked(&h.app, "map.showInfo"), Some(false));

    run(&mut h, "map.showInfo", json!({}));
    run(&mut h, "map.showKey", json!({}));
    h.settle(SETTLE);
    assert_eq!(crate::menubar::checked(&h.app, "map.showInfo"), Some(true));
    assert_eq!(crate::menubar::checked(&h.app, "map.showKey"), Some(true));
    let canvas = widget(&h, "map:canvas").expect("canvas");
    for id in ["map:info", "map:key"] {
        let r = widget(&h, id).unwrap_or_else(|| panic!("{id} is drawn"));
        assert!(canvas.contains_rect(r), "{id} {r:?} inside {canvas:?}");
    }
    let rows: Vec<String> = h.app.map.info_rows().into_iter().map(|r| r.text).collect();
    assert!(rows.iter().any(|t| t.contains("Paris") && t.contains("France")), "{rows:?}");
    assert!(rows.iter().any(|t| t.contains("12.0")), "the zoom: {rows:?}");

    // the choice is part of the saved map preferences
    let saved = serde_json::to_string(&h.app.map.prefs).unwrap();
    let back: super::MapPrefs = serde_json::from_str(&saved).unwrap();
    assert!(back.show_info && back.show_key);
    assert!(!serde_json::from_str::<super::MapPrefs>("{}").unwrap().show_info, "off unless asked for");

    run(&mut h, "map.showInfo", json!({"on": false}));
    h.settle(SETTLE);
    assert!(widget(&h, "map:info").is_none() && widget(&h, "map:key").is_some());
}

/// Saved locations are edited on the map as in Lightroom Classic: drag the centre handle to move
/// one, the knob on its edge to change its radius (one undo step each); Lock Markers holds both.
/// Right-click ▸ New Location Here… opens the Edit Location panel with a draft circle there,
/// nothing is saved until Save, and Save refuses a name another location already has (it used to
/// overwrite that location silently). The panel also renames, makes private and deletes.
#[test]
fn saved_locations_are_moved_resized_and_edited_on_the_map() {
    let mut h = map_on_fake_tiles();
    run(&mut h, "map.view", json!({"lat": 48.8566, "lon": 2.3522, "zoom": 16}));
    run(&mut h, "map.saveLocation", json!({"name": "Home", "lat": 48.8566, "lon": 2.3522, "radius": 100}));
    h.settle(SETTLE);
    let home = |h: &Headless| h.app.session.catalog.saved_location("Home").cloned().expect("Home is saved");
    assert!(widget(&h, "map:location:Home:centre").is_some(), "every saved location has a centre handle");
    assert!(widget(&h, "map:location:Home:radius").is_none(), "the radius handle shows on the active location only");

    // drag the centre 80 px east: the location moves, its radius stays
    let r = h.request("ui.dragWidget", json!({"id": "map:location:Home:centre", "dx": 80.0, "dy": 0.0, "steps": 12}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let moved = home(&h);
    assert!(moved.lon > 2.3522 + 0.0005 && (moved.lat - 48.8566).abs() < 0.0003, "{moved:?}");
    assert_eq!(moved.radius, 100.0);
    assert_eq!(h.app.map.active_location.as_deref(), Some("Home"), "the location just moved is the active one");

    // drag the radius knob 40 px up, away from the centre: the circle grows
    let knob = widget(&h, "map:location:Home:radius").expect("the active location shows its radius handle");
    let r = h.request("ui.dragWidget", json!({"id": "map:location:Home:radius", "dx": 0.0, "dy": -40.0, "steps": 12}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let grown = home(&h);
    assert!(grown.radius > 120.0, "{grown:?} (knob was at {knob:?})");
    assert!((grown.lon - moved.lon).abs() < 1e-9, "resizing doesn't move it");

    // each drag is one undo step
    run(&mut h, "edit.undo", json!({}));
    assert_eq!(home(&h).radius, 100.0);
    run(&mut h, "edit.undo", json!({}));
    assert!((home(&h).lon - 2.3522).abs() < 1e-9);
    h.settle(SETTLE);

    // Lock Markers: the handles don't move it
    run(&mut h, "map.toggleLockMarkers", json!({}));
    h.settle(SETTLE);
    let before = home(&h);
    h.request("ui.dragWidget", json!({"id": "map:location:Home:centre", "dx": 60.0, "dy": 30.0, "steps": 12}), T);
    h.settle(SETTLE);
    assert_eq!(home(&h), before, "locked");
    run(&mut h, "map.toggleLockMarkers", json!({}));

    // right-click ▸ New Location Here…: a draft, nothing saved yet
    let canvas = widget(&h, "map:canvas").unwrap();
    let at = canvas.center() + egui::vec2(-200.0, 120.0);
    h.request("ui.click", json!({"x": at.x, "y": at.y, "button": "right"}), T);
    h.settle(SETTLE);
    let r = h.request("ui.clickWidget", json!({"id": "map:menu:newLocation"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let ed = h.app.map.editor.clone().expect("the Edit Location panel is open");
    let want = super::view::screen_to_geo(&h.app.map, canvas, at);
    assert!(ed.original.is_none() && (ed.centre.lat - want.lat).abs() < 1e-4 && (ed.centre.lon - want.lon).abs() < 1e-4, "{ed:?}");
    assert!(ed.radius >= 10.0, "a radius that suits the zoom: {}", ed.radius);
    assert!(widget(&h, "map:location:draft:centre").is_some() && widget(&h, "map:location:draft:radius").is_some(), "the draft has handles");
    assert_eq!(h.app.session.catalog.saved_locations().count(), 1, "nothing saved before Save");

    // the draft's radius handle changes the panel, not the library
    h.request("ui.dragWidget", json!({"id": "map:location:draft:radius", "dx": 0.0, "dy": -30.0, "steps": 10}), T);
    h.settle(SETTLE);
    assert!(h.app.map.editor.as_ref().unwrap().radius > ed.radius);
    assert_eq!(h.app.session.catalog.saved_locations().count(), 1);

    // a name another location has is refused, not overwritten
    run(&mut h, "map.locationEditor", json!({"name": "home"}));
    h.request("ui.clickWidget", json!({"id": "button:mapLocationSave"}), T);
    h.settle(SETTLE);
    assert!(h.app.map.editor.as_ref().is_some_and(|e| e.error.is_some()), "the panel says why");
    assert_eq!(home(&h), before, "Home is untouched");
    run(&mut h, "map.locationEditor", json!({"name": "Work"}));
    h.request("ui.clickWidget", json!({"id": "button:mapLocationSave"}), T);
    h.settle(SETTLE);
    assert!(h.app.map.editor.is_none(), "saved: the panel closes");
    assert!(h.app.session.catalog.saved_location("Work").is_some());

    // the panel renames and makes private (the filter follows the rename); Delete removes, undoably
    run(&mut h, "map.filter", json!({"filter": "location:Home"}));
    run(&mut h, "map.editLocation", json!({"name": "Home"}));
    run(&mut h, "map.locationEditor", json!({"name": "Casa", "private": true, "action": "save"}));
    assert!(h.app.session.catalog.saved_location("Home").is_none());
    let casa = h.app.session.catalog.saved_location("Casa").cloned().expect("renamed");
    assert!(casa.private && casa.radius == 100.0);
    assert_eq!(run(&mut h, "map.view", json!({}))["filter"], "location:Casa");
    // only the capitals change: not "already exists"
    run(&mut h, "map.editLocation", json!({"name": "Casa"}));
    run(&mut h, "map.locationEditor", json!({"name": "CASA", "action": "save"}));
    assert_eq!(h.app.session.catalog.saved_location("casa").map(|l| l.name.as_str()), Some("CASA"));
    run(&mut h, "map.editLocation", json!({"name": "Work"}));
    h.settle(SETTLE);
    h.request("ui.clickWidget", json!({"id": "button:mapLocationDelete"}), T);
    h.settle(SETTLE);
    assert!(h.app.session.catalog.saved_location("Work").is_none());
    run(&mut h, "edit.undo", json!({}));
    assert!(h.app.session.catalog.saved_location("Work").is_some());

    // bad input: errors, not panics
    for (cmd, p) in [
        ("map.editLocation", json!({"name": "Nowhere"})),
        ("map.newLocation", json!({"lat": 1000})),
        ("map.locationEditor", json!({"action": "explode"})),
    ] {
        let r = h.request("engine.execute", json!({"command": cmd, "params": p}), T);
        assert_eq!(r["ok"], false, "{cmd}: {r}");
    }
    run(&mut h, "map.newLocation", json!({}));
    let r = h.request("engine.execute", json!({"command": "map.locationEditor", "params": {"radius": -5}}), T);
    assert_eq!(r["ok"], false, "{r}");
    run(&mut h, "map.locationEditor", json!({"action": "cancel"}));
    assert!(h.app.map.editor.is_none());
}

/// Photos dragged from the filmstrip onto a saved location in the side panel's list take the
/// location's centre (one undo step), as dropping them on the map does at that point.
#[test]
fn photos_dropped_on_a_saved_location_in_the_list_take_its_centre() {
    let mut h = map_on_fake_tiles();
    run(&mut h, "map.saveLocation", json!({"name": "Home", "lat": 40.653957, "lon": -4.689763, "radius": 25}));
    h.settle(SETTLE);
    let id = h.app.session.visible_cloned()[0].0;
    assert!(h.app.session.catalog.photo(lcc_catalog::PhotoId(id)).unwrap().meta.gps.is_none());
    // the Saved Locations section sits below Metadata: scroll the side panel to it
    let side = widget(&h, "map:side").expect("the side panel");
    h.request("ui.move", json!({"x": side.center().x, "y": side.center().y}), T);
    for _ in 0..30 {
        if widget(&h, "map:locationRow:Home").is_some_and(|r| side.contains(r.center())) {
            break;
        }
        h.request("ui.scroll", json!({"dy": -300.0}), T);
        h.settle(SETTLE);
    }
    let row = widget(&h, "map:locationRow:Home").expect("the location's row is on screen");
    let r = h.request("ui.dragWidget", json!({"id": format!("film:{id}"), "toX": row.center().x, "toY": row.center().y, "steps": 12}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let gps = h.app.session.catalog.photo(lcc_catalog::PhotoId(id)).unwrap().meta.gps.expect("dropped on the row: geotagged");
    assert!((gps.0 - 40.653957).abs() < 1e-6 && (gps.1 + 4.689763).abs() < 1e-6, "{gps:?}");
    assert!(h.app.ui.dragging_photos.is_none(), "the drag ended");
    run(&mut h, "edit.undo", json!({}));
    assert!(h.app.session.catalog.photo(lcc_catalog::PhotoId(id)).unwrap().meta.gps.is_none(), "one undo step");
}

/// Lightroom Classic's card over a hovered pin: the photo, its name, date and exposure, and for a
/// pin of several photos "1 of N" with arrows. The card stays while the pointer moves onto it.
#[test]
fn a_pin_hover_card_steps_through_the_pins_photos_and_selects_one() {
    let mut h = map_on_fake_tiles();
    let ids: Vec<u64> = h.app.session.visible_cloned().iter().take(3).map(|p| p.0).collect();
    run(&mut h, "library.select", json!({"ids": ids}));
    run(&mut h, "map.geotag", json!({"ids": ids, "lat": 48.85, "lon": 2.35}));
    run(&mut h, "library.select", json!({"ids": []}));
    h.settle(SETTLE);
    let pin = widget(&h, &format!("map:pin:{}", ids[0])).expect("the pin is on screen");
    h.events.push(egui::Event::PointerMoved(pin.center()));
    h.step();
    h.step();
    assert!(widget(&h, "map:hover").is_some(), "hovering the pin opens the card");
    let card = h.app.map.hover_card.clone().expect("the card's state");
    assert_eq!((card.ids.len(), card.index), (3, 0));
    // over to the card (across the gap) and its arrow: the card stays and shows the next photo
    let next = widget(&h, "map:hover:next").expect("a pin of three photos has arrows");
    let r = h.request("ui.clickWidget", json!({"id": "map:hover:next"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    assert_eq!(h.app.map.hover_card.as_ref().map(|c| c.index), Some(1), "moving onto the card kept it; next: the second photo");
    assert!(widget(&h, "map:hover").is_some_and(|c| c.contains(next.center())));
    // back twice: round the start to the last photo
    for _ in 0..2 {
        h.request("ui.clickWidget", json!({"id": "map:hover:prev"}), T);
        h.step();
    }
    assert_eq!(h.app.map.hover_card.as_ref().map(|c| c.index), Some(2));
    // a click on the photo selects that one
    h.request("ui.clickWidget", json!({"id": "map:hover:photo"}), T);
    h.step();
    assert_eq!(h.app.session.selection.ids, vec![lcc_catalog::PhotoId(card.ids[2])]);
    // away from the pin and the card: it closes
    let canvas = widget(&h, "map:canvas").expect("the canvas");
    h.events.push(egui::Event::PointerMoved(canvas.left_bottom() + egui::vec2(40.0, -40.0)));
    h.step();
    h.step();
    assert!(h.app.map.hover_card.is_none() && widget(&h, "map:hover").is_none(), "the card closes");
    // a pin of one photo: no arrows
    run(&mut h, "map.geotag", json!({"ids": [ids[2]], "lat": 48.80, "lon": 2.20}));
    h.settle(SETTLE);
    let one = widget(&h, &format!("map:pin:{}", ids[2])).expect("the single pin");
    h.events.push(egui::Event::PointerMoved(one.center()));
    h.step();
    h.step();
    assert!(widget(&h, "map:hover").is_some() && widget(&h, "map:hover:next").is_none());
}
