//! The Map's two corner boxes (View ▸ Show Map Info / Show Map Key), painted with the shared
//! [`crate::overlay`] box: the info box names the place at the centre of the view (from the
//! offline place list when it is downloaded), the zoom and the style; the key explains the
//! symbols the canvas draws.

use egui::{Color32, Painter, Rect, vec2};

use lcc_geo::LatLon;
use lcc_geo::geocode::GeoNames;

use super::MapUi;
use crate::i18n::tr;
use crate::overlay::{self, Corner, Row, Symbol};
use crate::theme::Tokens;

/// Pin colours, shared with the canvas so the key never drifts from what it explains.
pub const PIN: Color32 = Color32::from_rgb(0x20, 0x20, 0x24);
pub const TRACK: Color32 = Color32::from_rgb(0xe0, 0x40, 0x30);
pub const SAVED: Color32 = Color32::from_rgb(0x30, 0x80, 0xe0);
pub const SAVED_PRIVATE: Color32 = Color32::from_rgb(0xd0, 0x40, 0x40);

/// "City, State, Country" for the nearest place, whichever parts are known.
pub fn place_text(names: &GeoNames, at: LatLon) -> Option<String> {
    let p = names.reverse(at)?;
    let parts: Vec<&str> = [p.city.as_str(), p.state.as_str(), p.country.as_str()].into_iter().filter(|s| !s.is_empty()).collect();
    (!parts.is_empty()).then(|| parts.join(", "))
}

impl MapUi {
    /// The offline place list, read the first time it is wanted (`None`: not downloaded).
    #[cfg(not(target_arch = "wasm32"))]
    fn place_names(&mut self) -> Option<std::sync::Arc<GeoNames>> {
        if self.names.is_none() {
            let loaded = super::settings_dir().and_then(|d| GeoNames::load(&d.join("geonames")).ok()).filter(|g| !g.is_empty());
            self.names = Some(loaded.map(std::sync::Arc::new));
        }
        self.names.clone().flatten()
    }

    #[cfg(target_arch = "wasm32")]
    fn place_names(&mut self) -> Option<std::sync::Arc<GeoNames>> {
        self.names.clone().flatten()
    }

    /// The name of the place at the centre of the view, looked up again only after the centre moves.
    fn centre_place(&mut self) -> Option<String> {
        let c = self.centre();
        if let Some((at, text)) = &self.centre_name
            && (at.lat - c.lat).abs() < 1e-4
            && (at.lon - c.lon).abs() < 1e-4
        {
            return text.clone();
        }
        let text = self.place_names().and_then(|g| place_text(&g, c));
        self.centre_name = Some((c, text.clone()));
        text
    }

    /// The info box's rows: the centre's place, its position, the zoom and the style.
    pub fn info_rows(&mut self) -> Vec<Row> {
        let c = self.centre();
        let mut rows = Vec::new();
        if let Some(place) = self.centre_place() {
            rows.push(Row::text(place));
        }
        rows.push(Row::text(format!("{:.4}, {:.4}", c.lat, c.lon)));
        rows.push(Row::text(format!("{}: {:.1}", tr("Zoom"), self.prefs.zoom)));
        rows.push(Row::text(format!("{}: {}", tr("Style"), self.server().name)));
        rows
    }
}

/// The key's rows: what each symbol on the map means (the selected pin in `accent`).
pub fn key_rows(accent: Color32) -> Vec<Row> {
    vec![
        Row::symbol(Symbol::Disc { fill: PIN, label: None }, tr("Photos at this place")),
        Row::symbol(Symbol::Disc { fill: PIN, label: Some("12") }, tr("Photos close together")),
        Row::symbol(Symbol::Disc { fill: accent, label: None }, tr("Selected photos")),
        Row::symbol(Symbol::Line { color: TRACK }, tr("Tracklog")),
        Row::symbol(Symbol::Ring { color: SAVED }, tr("Saved Locations")),
        Row::symbol(Symbol::Ring { color: SAVED_PRIVATE }, tr("Private")),
    ]
}

/// Paint the boxes the View menu turned on: the info in the top right, the key in the bottom
/// left above the scale bar. Returns their rectangles (info, key).
pub fn paint(m: &mut MapUi, painter: &Painter, t: &Tokens, r: Rect) -> (Option<Rect>, Option<Rect>) {
    let info = if m.prefs.show_info { overlay::paint(painter, t, r, Corner::TopRight, vec2(0.0, 0.0), &m.info_rows()) } else { None };
    let key = if m.prefs.show_key { overlay::paint(painter, t, r, Corner::BottomLeft, vec2(0.0, 22.0), &key_rows(t.accent)) } else { None };
    (info, key)
}
