//! `library.facets` (issue #218): the values each metadata field takes among the photos in view, with counts.

use lcc_catalog::{CopyrightStatus, Op};
use serde_json::json;

use crate::Session;

/// The demo library with three photos carrying the full set of Metadata-column values.
fn library() -> (Session, Vec<lcc_catalog::PhotoId>) {
    let mut s = Session::with_demo();
    let ids: Vec<_> = s.catalog.photos().filter(|p| p.in_library()).take(3).map(|p| p.id).collect();
    let rows = [
        ("2026-04-12T10:00:00+04:00", "SN-A", Some(true), "J-777", "Ann", CopyrightStatus::Copyrighted, "Studio", "Zayed City", "Abu Dhabi", "United Arab Emirates", Some((24.45, 54.6)), "1/250", 2.8, 50.0, 100),
        ("2026-04-12T11:00:00Z", "SN-A", Some(false), "J-778", "Bo", CopyrightStatus::PublicDomain, "Harbour", "Muscat", "Muscat", "Oman", Some((23.59, 58.41)), "1/125", 4.0, 35.0, 200),
        ("2026-05-03T12:00:00-05:00", "SN-B", None, "", "Cy", CopyrightStatus::Unknown, "Tram", "Lisbon", "Lisbon", "Portugal", None, "1/60", 5.6, 24.0, 400),
    ];
    for (id, (captured, serial, flash, job, creator, status, location, city, state, country, gps, shutter, aperture, focal, iso)) in ids.iter().copied().zip(rows) {
        let mut m = s.catalog.photo(id).unwrap().meta.clone();
        m.camera = if serial == "SN-A" { "Facet Cam A".into() } else { "Facet Cam B".into() };
        m.camera_serial = serial.into();
        m.flash = flash.map(|fired| lcc_meta::Flash { fired, raw: if fired { 1 } else { 0 } });
        m.job = job.into();
        m.creator = creator.into();
        m.copyright_status = status;
        m.location = location.into();
        m.city = city.into();
        m.state = state.into();
        m.country = country.into();
        m.gps = gps;
        m.shutter = shutter.into();
        m.aperture = Some(aperture);
        m.focal_mm = Some(focal);
        m.iso = Some(iso);
        m.keywords = if id == ids[0] {
            vec!["qq-one".into()]
        } else if id == ids[1] {
            vec!["qq-one".into(), "qq-two".into()]
        } else {
            vec!["qq-two".into()]
        };
        s.catalog.apply(Op::SetMeta { id, meta: Box::new(m) }).unwrap();
        s.catalog.apply(Op::SetCaptured { id, captured: Some(captured.into()) }).unwrap();
    }
    let home = lcc_geo::SavedLocation::new("Studio", 24.45, 54.6, 1000.0, false).unwrap();
    s.catalog.apply(Op::SetSavedLocation { name: home.name.clone(), location: Some(home) }).unwrap();
    (s, ids)
}

fn values(r: &serde_json::Value, field: &str) -> Vec<(String, u64)> {
    r[field]["values"].as_array().unwrap().iter().map(|v| (v["value"].as_str().unwrap().to_string(), v["count"].as_u64().unwrap())).collect()
}

#[test]
fn counts_the_values_of_the_photos_in_view() {
    let (mut s, _) = library();
    let shown = s.visible().len() as u64;
    let r = s.execute("library.facets", &json!({"fields": ["camera", "keyword", "rating"]})).unwrap();
    assert_eq!(r["camera"]["photos"], shown);
    let cams = values(&r, "camera");
    assert!(cams.contains(&("Facet Cam A".to_string(), 2)) && cams.contains(&("Facet Cam B".to_string(), 1)), "{cams:?}");
    let with_camera: u64 = cams.iter().map(|c| c.1).sum();
    assert_eq!(with_camera + r["camera"]["none"].as_u64().unwrap(), shown, "every photo once, with a camera or without");
    assert_eq!(r["camera"]["distinct"].as_u64().unwrap() as usize, cams.len());
    // a photo with several keywords counts once under each
    let kws = values(&r, "keyword");
    assert!(kws.contains(&("qq-one".to_string(), 2)) && kws.contains(&("qq-two".to_string(), 2)), "{kws:?}");
    // most photos first; rating by value
    assert!(cams.windows(2).all(|w| w[0].1 >= w[1].1));
    let ratings: Vec<u32> = values(&r, "rating").iter().map(|v| v.0.parse().unwrap()).collect();
    assert!(ratings.windows(2).all(|w| w[0] < w[1]), "{ratings:?}");
    assert!(r.get("lens").is_none(), "only the fields asked for");
}

/// A field narrowed by the filter still lists the values it could take; the other fields count only what matches.
#[test]
fn a_fields_own_constraint_is_left_out() {
    let (mut s, _) = library();
    s.execute("library.filter", &json!({"camera": "Facet Cam A"})).unwrap();
    let r = s.execute("library.facets", &json!({"fields": ["camera", "keyword"]})).unwrap();
    let cams = values(&r, "camera");
    assert!(cams.iter().any(|c| c.0 == "Facet Cam B"), "the other camera is still offered: {cams:?}");
    assert_eq!(r["keyword"]["photos"], 2, "keywords of the two Facet Cam A photos");
    assert_eq!(values(&r, "keyword"), vec![("qq-one".to_string(), 2), ("qq-two".to_string(), 1)]);
}

#[test]
fn rejects_unknown_fields_and_bounds_the_values() {
    let (mut s, _) = library();
    assert!(s.execute("library.facets", &json!({"fields": ["camera", "shoe size"]})).is_err());
    assert!(s.execute("library.facets", &json!({"fields": "camera"})).is_err());
    assert!(s.execute("library.facets", &json!({"limit": -1})).is_err());
    let r = s.execute("library.facets", &json!({"fields": ["keyword"], "limit": 1})).unwrap();
    assert_eq!(r["keyword"]["values"].as_array().unwrap().len(), 1);
    assert!(r["keyword"]["distinct"].as_u64().unwrap() > 1, "distinct counts them all");
    let all = s.execute("library.facets", &json!({})).unwrap();
    assert_eq!(all.as_object().unwrap().len(), crate::cmd::facets::FIELDS.len());
}

/// Issue #218: a Metadata column's click (`library.facetPick`) narrows the view; the next column counts only what
/// is left (the columns cascade), a field with several values chosen shows any of them, and the field picked keeps
/// listing its other values.
#[test]
fn picking_a_value_narrows_and_cascades() {
    let (mut s, ids) = library();
    let r = s.execute("library.facetPick", &json!({"field": "camera", "value": "Facet Cam A"})).unwrap();
    assert_eq!(r["count"], 2, "{r}");
    assert_eq!(r["chosen"], json!(["Facet Cam A"]));
    let f = s.execute("library.facets", &json!({"fields": ["camera", "keyword"]})).unwrap();
    assert_eq!(f["camera"]["chosen"], json!(["Facet Cam A"]));
    assert!(values(&f, "camera").iter().any(|c| c.0 == "Facet Cam B"), "the camera column keeps its other values");
    assert_eq!(values(&f, "keyword"), vec![("qq-one".to_string(), 2), ("qq-two".to_string(), 1)], "the keyword column cascades");
    // a second camera: either matches
    let r = s.execute("library.facetPick", &json!({"field": "camera", "value": "Facet Cam B"})).unwrap();
    assert_eq!(r["count"], 3);
    // the next column narrows further
    let r = s.execute("library.facetPick", &json!({"field": "keyword", "value": "qq-two"})).unwrap();
    assert_eq!(r["count"], 2);
    let mut shown = s.visible().to_vec();
    shown.sort();
    let mut want = vec![ids[1], ids[2]];
    want.sort();
    assert_eq!(shown, want);
    // a click on a chosen value takes it off (any case); no value clears the field
    let r = s.execute("library.facetPick", &json!({"field": "camera", "value": "facet cam b"})).unwrap();
    assert_eq!(r["chosen"], json!(["Facet Cam A"]));
    s.execute("library.facetPick", &json!({"field": "camera"})).unwrap();
    s.execute("library.facetPick", &json!({"field": "keyword", "value": null})).unwrap();
    assert_eq!(s.filter, lcc_catalog::Filter::default(), "nothing left of the picks");
}

/// Single-valued fields (flag, rating, kind, year) hold the last value clicked; a second click clears it.
#[test]
fn single_valued_fields_hold_one_value() {
    let (mut s, ids) = library();
    s.execute("photo.rate", &json!({"ids": [ids[0].0], "rating": 4})).unwrap();
    s.execute("library.facetPick", &json!({"field": "rating", "value": "4"})).unwrap();
    assert!(!s.visible().is_empty());
    let shown = s.visible().to_vec();
    assert!(shown.iter().all(|id| s.catalog.photo(*id).unwrap().rating == 4));
    assert_eq!(s.execute("library.facets", &json!({"fields": ["rating"]})).unwrap()["rating"]["chosen"], json!(["4"]));
    s.execute("library.facetPick", &json!({"field": "rating", "value": "4"})).unwrap();
    assert_eq!(s.filter, lcc_catalog::Filter::default(), "cleared");
    for (field, value) in [("flag", "pick"), ("kind", "image"), ("year", "2026")] {
        s.execute("library.facetPick", &json!({"field": field, "value": value})).unwrap();
        let f = s.execute("library.facets", &json!({"fields": [field]})).unwrap();
        assert_eq!(f[field]["chosen"], json!([value]), "{field}");
        s.execute("library.facetPick", &json!({"field": field, "value": value})).unwrap();
    }
    assert_eq!(s.filter, lcc_catalog::Filter::default());
}

/// Hostile arguments are errors that leave the filter alone, never a panic.
#[test]
fn bad_picks_are_refused() {
    let (mut s, _) = library();
    for p in [
        json!({}),
        json!({"field": "shoe size", "value": "9"}),
        json!({"field": "exported", "value": "maybe"}),
        json!({"field": "flash", "value": "maybe"}),
        json!({"field": "gpsData", "value": "maybe"}),
        json!({"field": "copyrightStatus", "value": "maybe"}),
        json!({"field": "month", "value": "2026-13"}),
        json!({"field": "day", "value": "2026-02-30"}),
        json!({"field": "timezone", "value": "Mars"}),
        json!({"field": "camera", "value": 3}),
        json!({"field": "camera", "value": "  "}),
        json!({"field": "rating", "value": "9"}),
        json!({"field": "rating", "value": "0"}),
        json!({"field": "rating", "value": "-1"}),
        json!({"field": "flag", "value": "maybe"}),
        json!({"field": "kind", "value": "hologram"}),
        json!({"field": "year", "value": "20x6"}),
        json!({"field": "year", "value": "é202"}),
        json!({"field": "label", "value": "Ultraviolet"}),
    ] {
        assert!(s.execute("library.facetPick", &p).is_err(), "{p}");
        assert_eq!(s.filter, lcc_catalog::Filter::default(), "{p} left the filter alone");
    }
}

/// Issue #777: every group of Metadata columns counts its values and narrows the grid, including
/// combinations of EXIF, copyright and place values, the export status and a column's Unknown row.
#[test]
fn comprehensive_metadata_columns_filter_together() {
    let (mut s, ids) = library();
    s.record_export(&[ids[0], ids[1]]);
    let fields = [
        "month", "day", "timezone", "exported", "edit", "cameraSerial", "focal", "shutter", "aperture", "iso", "flash", "gpsData",
        "mapLocation", "location", "city", "state", "country", "creator", "copyrightStatus", "job",
    ];
    let r = s.execute("library.facets", &json!({"fields": fields})).unwrap();
    for (field, value, count) in [
        ("month", "2026-04", 2),
        ("day", "2026-04-12", 2),
        ("timezone", "UTC+04:00", 1),
        ("cameraSerial", "SN-A", 2),
        ("shutter", "1/250", 1),
        ("aperture", "2.8", 1),
        ("iso", "100", 1),
        ("focal", "50", 1),
        ("flash", "fired", 1),
        ("gpsData", "coordinates", 2),
        ("mapLocation", "Studio", 1),
        ("city", "Zayed City", 1),
        ("copyrightStatus", "copyrighted", 1),
        ("job", "J-777", 1),
        ("exported", "exported", 2),
        ("edit", "unedited", 3),
    ] {
        assert!(values(&r, field).contains(&(value.to_string(), count)), "{field}: {value} / {count}: {:?}", values(&r, field));
    }
    assert_eq!(r["job"]["none"], 1, "one photo has no job identifier");

    // Classic's own field name is accepted; choices in two columns AND together.
    let r = s.execute("library.facetPick", &json!({"field": "shutterSpeed", "value": "1/250"})).unwrap();
    assert_eq!(r["count"], 1, "{r}");
    let r = s.execute("library.facetPick", &json!({"field": "city", "value": "Zayed City"})).unwrap();
    assert_eq!(r["count"], 1, "{r}");
    assert_eq!(s.visible().first().copied(), Some(ids[0]));
    s.execute("library.facetPick", &json!({"field": "shutter"})).unwrap();
    s.execute("library.facetPick", &json!({"field": "city"})).unwrap();
    assert_eq!(s.filter, lcc_catalog::Filter::default());

    // Exported Files, a numeric column and the Unknown row are real filters too.
    let r = s.execute("library.facetPick", &json!({"field": "exported", "value": "exported"})).unwrap();
    assert_eq!(r["count"], 2, "{r}");
    s.execute("library.facetPick", &json!({"field": "exported"})).unwrap();
    let r = s.execute("library.facetPick", &json!({"field": "iso", "value": "100"})).unwrap();
    assert_eq!(r["count"], 1, "{r}");
    s.execute("library.facetPick", &json!({"field": "iso"})).unwrap();
    let r = s.execute("library.facetPick", &json!({"field": "job", "value": lcc_catalog::facets::NO_VALUE})).unwrap();
    assert_eq!(r["count"], 1, "{r}");
    assert_eq!(s.visible().first().copied(), Some(ids[2]));
    s.execute("library.facetPick", &json!({"field": "job"})).unwrap();

    // A filter patch and its removable chips use the same canonical field ids.
    s.execute("library.filter", &json!({"facets": {"city": ["Zayed City"], "copyrightStatus": ["copyrighted"]}})).unwrap();
    assert_eq!(s.visible().first().copied(), Some(ids[0]));
    assert_eq!(crate::filter_chips(&s.filter, &s.catalog).len(), 2);
    s.execute("library.clearFilter", &json!({})).unwrap();
    assert_eq!(s.filter, lcc_catalog::Filter::default());
}
