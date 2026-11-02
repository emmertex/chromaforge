//! The Library Filter's Metadata columns (issue #777): one registry of fields, labels and value readers.
//!
//! [`crate::Filter`] keeps the chosen values of the newer columns in `Filter::facets`; the older
//! columns keep their own fields on `Filter`. Both the matcher and `library.facets` call [`values`],
//! so a value a column offers is exactly a value the filter understands. Values are canonical ids
//! (`fired`, `publicDomain`, `UTC+04:00`), while the UI translates them for display.

use crate::query::Filter;
use crate::{Catalog, CopyrightStatus, Flag, MediaKind, Photo};
use lcc_geo::LatLon;
use lcc_meta::{DatePrecision, DateTime};

/// A clickable "Unknown" row: photos with no value for a field.
pub const NO_VALUE: &str = "noValue";

/// Every field a Metadata column can list, in the order the column picker shows them.
pub const FIELDS: &[&str] = &[
    "year",
    "month",
    "day",
    "timezone",
    "kind",
    "flag",
    "rating",
    "exported",
    "label",
    "keyword",
    "person",
    "edit",
    "camera",
    "cameraSerial",
    "lens",
    "focal",
    "shutter",
    "aperture",
    "iso",
    "flash",
    "gpsData",
    "mapLocation",
    "location",
    "city",
    "state",
    "country",
    "creator",
    "copyrightStatus",
    "job",
];

/// Fields whose choices live in [`Filter::facets`]; the first nine have their own filter fields.
pub const GENERIC_FIELDS: &[&str] = &[
    "month",
    "day",
    "timezone",
    "exported",
    "edit",
    "cameraSerial",
    "focal",
    "shutter",
    "aperture",
    "iso",
    "flash",
    "gpsData",
    "mapLocation",
    "location",
    "city",
    "state",
    "country",
    "creator",
    "copyrightStatus",
    "job",
];

/// (field id, English label) in the order of [`FIELDS`]. The UI translates the labels.
pub const LABELS: &[(&str, &str)] = &[
    ("year", "Date"),
    ("month", "Month"),
    ("day", "Day"),
    ("timezone", "Timezone"),
    ("kind", "File Type"),
    ("flag", "Flag"),
    ("rating", "Rating"),
    ("exported", "Exported Files"),
    ("label", "Label"),
    ("keyword", "Keyword"),
    ("person", "People"),
    ("edit", "Edit"),
    ("camera", "Camera"),
    ("cameraSerial", "Camera Serial Number"),
    ("lens", "Lens"),
    ("focal", "Focal Length"),
    ("shutter", "Shutter Speed"),
    ("aperture", "Aperture"),
    ("iso", "ISO Speed"),
    ("flash", "Flash State"),
    ("gpsData", "GPS Data"),
    ("mapLocation", "Map Location"),
    ("location", "Location"),
    ("city", "City"),
    ("state", "State / Province"),
    ("country", "Country / Region"),
    ("creator", "Creator"),
    ("copyrightStatus", "Copyright Status"),
    ("job", "Job"),
];

/// The dropdown's groups; every field of [`FIELDS`] appears once.
pub const GROUPS: &[(&str, &[&str])] = &[
    ("Date & Time", &["year", "month", "day", "timezone"]),
    ("File & Status", &["kind", "flag", "rating", "exported", "label", "keyword", "person", "edit"]),
    ("Camera & EXIF", &["camera", "cameraSerial", "lens", "focal", "shutter", "aperture", "iso", "flash"]),
    ("Location & GPS", &["gpsData", "mapLocation", "location", "city", "state", "country"]),
    ("Workflow & Ownership", &["creator", "copyrightStatus", "job"]),
];

/// A field id or one of Classic's names / aliases → its canonical id.
pub fn canonical(field: &str) -> Option<&'static str> {
    let f = field.trim();
    if f.len() > 128 {
        return None;
    }
    if let Some(k) = FIELDS.iter().copied().find(|k| *k == f) {
        return Some(k);
    }
    let lower = f.to_ascii_lowercase();
    let space = |s: &str| s.chars().filter(|c| !matches!(c, ' ' | '/' | '&' | '_' | '-')).collect::<String>();
    let compact = space(&lower);
    Some(match compact.as_str() {
        "date" => "year",
        "filetype" | "type" => "kind",
        "exportedfiles" | "exportstatus" => "exported",
        "colorlabel" => "label",
        "keywords" => "keyword",
        "people" => "person",
        "edited" => "edit",
        "cameraserial" | "cameraserialnumber" | "bodyserialnumber" | "serialnumber" => "cameraSerial",
        "focallength" => "focal",
        "shutterspeed" => "shutter",
        "isospeed" | "isospeedrating" => "iso",
        "flashstate" => "flash",
        "gps" | "hasgps" | "gpsdata" | "coordinates" => "gpsData",
        "gpslocation" | "maplocation" => "mapLocation",
        "sublocation" => "location",
        "stateprovince" => "state",
        "countryregion" => "country",
        "copyright" | "copyrightstatus" => "copyrightStatus",
        "jobid" | "jobidentifier" | "transmissionreference" => "job",
        "month" => "month",
        "day" => "day",
        "timezone" | "timezoneoffset" => "timezone",
        "camera" => "camera",
        "lens" => "lens",
        "aperture" => "aperture",
        "iso" => "iso",
        "focal" => "focal",
        "shutter" => "shutter",
        "flash" => "flash",
        "location" => "location",
        "city" => "city",
        "state" => "state",
        "country" => "country",
        "creator" => "creator",
        "exported" => "exported",
        "edit" => "edit",
        "job" => "job",
        "label" => "label",
        "flag" => "flag",
        "rating" => "rating",
        "year" => "year",
        "kind" => "kind",
        _ => return None,
    })
}

/// Whether `field` names a column (an alias is fine too).
pub fn is_field(field: &str) -> bool {
    canonical(field).is_some()
}

/// Whether `field`'s choices live in [`Filter::facets`].
pub fn is_generic(field: &str) -> bool {
    GENERIC_FIELDS.contains(&field)
}

/// A column's English label; an unknown field reads as its id.
pub fn label<'a>(field: &'a str) -> &'a str {
    let key = canonical(field).unwrap_or(field);
    LABELS.iter().find(|(k, _)| *k == key).map(|(_, l)| *l).unwrap_or(key)
}

/// Whether a value a command or filter preset asked for can mean something for `field`.
pub fn valid_value(field: &str, value: &str) -> bool {
    let value = value.trim();
    if value == NO_VALUE {
        return true;
    }
    match field {
        "month" => DateTime::parse_iso(value).is_some_and(|d| d.precision == DatePrecision::Month),
        "day" => DateTime::parse_iso(value).is_some_and(|d| d.is_full()),
        "timezone" => timezone_offset(value).is_some(),
        "exported" => matches!(value, "exported" | "notExported"),
        "edit" => matches!(value, "edited" | "unedited"),
        "flash" => matches!(value, "fired" | "didNotFire"),
        "gpsData" => matches!(value, "coordinates" | "noCoordinates"),
        "copyrightStatus" => CopyrightStatus::parse(value).is_some(),
        "rating" => value.parse::<u8>().is_ok_and(|n| (1..=5).contains(&n)),
        "flag" => matches!(value, "pick" | "reject" | "none"),
        "kind" => matches!(value, "image" | "raw" | "video"),
        "year" => value.len() == 4 && value.chars().all(|c| c.is_ascii_digit()),
        _ => !value.is_empty(),
    }
}

/// The values `field` takes for photo `p` (several for keywords, people and overlapping saved
/// locations; none when the photo has no value). `filter` carries the last export's ids.
pub fn values(field: &str, p: &Photo, cat: &Catalog, filter: &Filter) -> Vec<String> {
    let field = canonical(field).unwrap_or(field);
    let text = |s: &str| {
        let s = s.trim();
        (!s.is_empty()).then(|| vec![s.to_string()]).unwrap_or_default()
    };
    match field {
        "camera" => text(&p.meta.camera),
        "lens" => text(&p.meta.lens),
        "keyword" => p.meta.keywords.iter().filter(|k| !k.trim().is_empty()).cloned().collect(),
        "person" => p.people().into_iter().map(str::to_string).collect(),
        "label" => p.label.map(|l| cat.label_name(l)).into_iter().collect(),
        "flag" => vec![flag_name(p.flag).to_string()],
        "rating" => vec![p.rating.to_string()],
        "kind" => vec![kind_name(p.kind).to_string()],
        "year" => p.captured.as_deref().and_then(|c| c.get(..4)).filter(|y| y.chars().all(|c| c.is_ascii_digit())).map(str::to_string).into_iter().collect(),
        "month" => date_parts(p.captured.as_deref(), "month"),
        "day" => date_parts(p.captured.as_deref(), "day"),
        "timezone" => date_parts(p.captured.as_deref(), "timezone"),
        "exported" => vec![if filter.exported_ids.contains(&p.id) { "exported".to_string() } else { "notExported".to_string() }],
        "edit" => vec![if p.is_edited() { "edited".to_string() } else { "unedited".to_string() }],
        "cameraSerial" => text(&p.meta.camera_serial),
        "focal" => decimal(p.meta.focal_mm).into_iter().collect(),
        "shutter" => text(&p.meta.shutter),
        "aperture" => decimal(p.meta.aperture).into_iter().collect(),
        "iso" => p.meta.iso.map(|i| i.to_string()).into_iter().collect(),
        "flash" => p.meta.flash.map(|f| if f.fired { "fired".to_string() } else { "didNotFire".to_string() }).into_iter().collect(),
        "gpsData" => vec![if p.meta.gps.is_some() { "coordinates".to_string() } else { "noCoordinates".to_string() }],
        "mapLocation" => {
            let Some((lat, lon)) = p.meta.gps else { return Vec::new() };
            let at = LatLon::new(lat, lon);
            let saved: Vec<String> = cat.saved_locations().filter(|l| l.contains(at)).map(|l| l.name.clone()).collect();
            if saved.is_empty() { text(&p.meta.country) } else { saved }
        }
        "location" => text(&p.meta.location),
        "city" => text(&p.meta.city),
        "state" => text(&p.meta.state),
        "country" => text(&p.meta.country),
        "creator" => text(&p.meta.creator),
        "copyrightStatus" => vec![p.meta.copyright_status.id().to_string()],
        "job" => text(&p.meta.job),
        _ => Vec::new(),
    }
}

/// Year / month / day / zone parts of a capture time; partial dates only give the parts they state.
fn date_parts(captured: Option<&str>, part: &str) -> Vec<String> {
    let Some(d) = captured.and_then(DateTime::parse_iso) else { return Vec::new() };
    match part {
        "month" if d.precision != DatePrecision::Year => vec![format!("{:04}-{:02}", d.year, d.month)],
        "day" if d.precision == DatePrecision::Full => vec![format!("{:04}-{:02}-{:02}", d.year, d.month, d.day)],
        "timezone" => d.offset_minutes.map(offset_label).into_iter().collect(),
        _ => Vec::new(),
    }
}

/// A zone as the column lists it (`UTC`, `UTC+04:00`, `UTC-05:30`).
fn offset_label(minutes: i16) -> String {
    if minutes == 0 {
        return "UTC".to_string();
    }
    let sign = if minutes < 0 { '-' } else { '+' };
    let m = minutes.unsigned_abs();
    format!("UTC{sign}{:02}:{:02}", m / 60, m % 60)
}

/// Read a zone written as `UTC`, `UTC+04:00`, `Z`, `+04:00` or `-0530`.
fn timezone_offset(value: &str) -> Option<i16> {
    if value.eq_ignore_ascii_case("UTC") {
        return Some(0);
    }
    let rest = if value.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("UTC")) {
        value.get(3..).unwrap_or("")
    } else {
        value
    };
    DateTime::parse_offset(rest.trim())
}

/// One decimal place, as stored `f32` values display and sort.
fn decimal(v: Option<f32>) -> Option<String> {
    let v = v.filter(|n| n.is_finite() && *n > 0.0)?;
    let rounded = (f64::from(v) * 10.0).round() / 10.0;
    Some(format!("{rounded}"))
}

fn flag_name(f: Flag) -> &'static str {
    match f {
        Flag::Pick => "pick",
        Flag::Reject => "reject",
        Flag::None => "none",
    }
}

fn kind_name(k: MediaKind) -> &'static str {
    match k {
        MediaKind::Image => "image",
        MediaKind::Raw => "raw",
        MediaKind::Video => "video",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Op, PhotoId, Source};
    use std::collections::BTreeMap;

    fn photo(cat: &mut Catalog) -> PhotoId {
        let id = cat.alloc_photo_id();
        let mut p = Photo::new(id, Source::Demo { scene: 0 }, "a.jpg", "JPEG", 4, 3, "2026-01-01T00:00:00");
        p.captured = Some("2026-04-12T10:00:00+04:00".into());
        p.meta.camera = "Facet Cam".into();
        p.meta.lens = "Facet Lens".into();
        p.meta.camera_serial = "SN-777".into();
        p.meta.flash = Some(lcc_meta::Flash { fired: true, raw: 1 });
        p.meta.job = "J-777".into();
        p.meta.creator = "Ann Lee".into();
        p.meta.copyright_status = CopyrightStatus::Copyrighted;
        p.meta.location = "Studio".into();
        p.meta.city = "Zayed City".into();
        p.meta.state = "Abu Dhabi".into();
        p.meta.country = "United Arab Emirates".into();
        p.meta.gps = Some((24.45, 54.6));
        p.meta.focal_mm = Some(50.0);
        p.meta.aperture = Some(2.8);
        p.meta.shutter = "1/250".into();
        p.meta.iso = Some(100);
        cat.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
        id
    }

    #[test]
    fn labels_groups_and_canonical_names_cover_every_field_once() {
        let grouped: Vec<&str> = GROUPS.iter().flat_map(|(_, fields)| fields.iter().copied()).collect();
        assert_eq!(grouped.len(), FIELDS.len());
        for field in FIELDS {
            assert_eq!(grouped.iter().filter(|f| **f == *field).count(), 1, "{field}");
            assert!(LABELS.iter().any(|(k, _)| k == field), "{field}");
            assert_eq!(canonical(field), Some(*field));
        }
        assert_eq!(canonical("Camera Serial Number"), Some("cameraSerial"));
        assert_eq!(canonical("GPS Data"), Some("gpsData"));
        assert_eq!(canonical("shutterSpeed"), Some("shutter"));
        assert_eq!(canonical("shoe size"), None);
    }

    #[test]
    fn values_describe_a_photo_and_the_filter_matches_them() {
        let mut cat = Catalog::new();
        let id = photo(&mut cat);
        if let Ok(home) = lcc_geo::SavedLocation::new("Studio", 24.45, 54.6, 1000.0, false) {
            cat.apply(Op::SetSavedLocation { name: home.name.clone(), location: Some(home) }).unwrap();
        }
        let p = cat.photo(id).unwrap();
        for (field, want) in [
            ("month", "2026-04"),
            ("day", "2026-04-12"),
            ("timezone", "UTC+04:00"),
            ("cameraSerial", "SN-777"),
            ("focal", "50"),
            ("shutter", "1/250"),
            ("aperture", "2.8"),
            ("iso", "100"),
            ("flash", "fired"),
            ("gpsData", "coordinates"),
            ("mapLocation", "Studio"),
            ("city", "Zayed City"),
            ("copyrightStatus", "copyrighted"),
            ("job", "J-777"),
            ("exported", "notExported"),
        ] {
            assert!(values(field, p, &cat, &Filter::default()).contains(&want.to_string()), "{field}: {want}");
        }
        let mut facets = BTreeMap::new();
        for (field, want) in [("city", "Zayed City"), ("copyrightStatus", "copyrighted"), ("job", "J-777"), ("flash", "fired")] {
            facets.insert(field.to_string(), vec![want.to_string()]);
        }
        let mut f = Filter { facets, ..Default::default() };
        f.exported_ids.set(vec![]);
        assert!(f.matches(p, &cat));
        f.facets.insert("city".into(), vec!["Paris".into()]);
        assert!(!f.matches(p, &cat));
    }

    #[test]
    fn no_value_matches_photos_without_that_metadata() {
        let mut cat = Catalog::new();
        let id = photo(&mut cat);
        let mut p = cat.photo(id).unwrap().clone();
        p.meta.job.clear();
        let mut facets = BTreeMap::new();
        facets.insert("job".to_string(), vec![NO_VALUE.to_string()]);
        let f = Filter { facets, ..Default::default() };
        assert!(f.matches(&p, &cat));
        p.meta.job = "J-777".into();
        assert!(!f.matches(&p, &cat));
    }

    #[test]
    fn hostile_filter_facets_are_errors_not_empty_grids() {
        assert!(serde_json::from_value::<Filter>(serde_json::json!({"facets": {"shoe size": ["x"]}})).is_err());
        assert!(serde_json::from_value::<Filter>(serde_json::json!({"facets": {"city": 3}})).is_err());
        assert!(serde_json::from_value::<Filter>(serde_json::json!({"facets": {"month": ["2026-13"]}})).is_err());
        let f: Filter = serde_json::from_value(serde_json::json!({"facets": {"Camera Serial Number": [" SN-777 "], "city": []}})).unwrap();
        assert_eq!(f.facets.get("cameraSerial").map(Vec::as_slice), Some(&["SN-777".to_string()][..]));
        assert!(f.facets.get("city").is_none());
    }

    #[test]
    fn exported_ids_stay_out_of_filter_equality() {
        let mut f = Filter::default();
        f.exported_ids.set(vec![PhotoId(1)]);
        assert_eq!(f, Filter::default());
    }
}
