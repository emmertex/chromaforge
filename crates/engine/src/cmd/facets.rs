//! `library.facets`: the distinct values of metadata fields among the photos in view, with how many photos have
//! each (issue #218's first Metadata columns, and issue #777's full Classic-style set). Each field is counted
//! with its own constraint left out of the filter, so a field already narrowed to one value still lists the
//! others it could take; the other fields' constraints and the source apply.

use std::collections::HashMap;

use lcc_catalog::{ColorLabel, Filter, Flag, MediaKind, Photo, RatingOp, Sort};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd};
use crate::{Result, Session};

/// The fields `library.facets` counts (and every Metadata column can pick).
pub use lcc_catalog::facets::FIELDS;
/// Values one field returns without a `limit`.
pub const LIMIT_DEFAULT: usize = 200;
/// Most values one field returns.
pub const LIMIT_MAX: usize = 2000;
/// Most values a caller may accumulate in one field's selection.
const MAX_CHOSEN: usize = 1000;
/// Every field can be picked; the older fields keep their own `Filter` fields, the newer ones use
/// `Filter::facets`.
pub const PICKABLE: &[&str] = FIELDS;

/// `f` without the constraint of `field` (the ones the filter bar and sidebar set for it).
fn without(field: &str, f: &Filter) -> Filter {
    let mut f = f.clone();
    match field {
        "camera" => {
            f.camera = None;
            f.cameras.clear();
        }
        "lens" => {
            f.lens = None;
            f.lenses.clear();
        }
        "keyword" => {
            f.keyword = None;
            f.keywords.clear();
        }
        "person" => {
            f.person = None;
            f.people.clear();
        }
        "label" => {
            f.label = None;
            f.labels.clear();
        }
        "flag" => f.flag = None,
        "rating" => f.rating = 0,
        "kind" => f.kind = None,
        "year" => {
            f.date = None;
            f.date_from = None;
            f.date_to = None;
        }
        _ => {}
    }
    f.facets.remove(field);
    f
}

/// The values of `field` the filter has chosen, spelled as `library.facets` lists them.
pub fn chosen(s: &Session, field: &str) -> Vec<String> {
    let field = lcc_catalog::facets::canonical(field).unwrap_or(field);
    let f = &s.filter;
    let list = |one: &Option<String>, many: &[String]| {
        let mut out: Vec<String> = Vec::new();
        for v in one.iter().chain(many.iter()) {
            if !out.iter().any(|o| o.eq_ignore_ascii_case(v)) {
                out.push(v.clone());
            }
        }
        out
    };
    let mut out = match field {
        "camera" => list(&f.camera, &f.cameras),
        "lens" => list(&f.lens, &f.lenses),
        "keyword" => list(&f.keyword, &f.keywords),
        "person" => list(&f.person, &f.people),
        "label" => {
            let mut v: Vec<ColorLabel> = f.labels.clone();
            v.extend(f.label.filter(|l| !f.labels.contains(l)));
            v.into_iter().map(|l| s.catalog.label_name(l)).collect()
        }
        "flag" => f.flag.map(flag_name).map(str::to_string).into_iter().collect(),
        "rating" if f.rating_op == RatingOp::Exactly && f.rating > 0 => vec![f.rating.to_string()],
        "kind" => f.kind.map(kind_name).map(str::to_string).into_iter().collect(),
        "year" => f.date.iter().filter(|d| d.len() == 4).cloned().collect(),
        _ => Vec::new(),
    };
    if let Some(extra) = f.facets.get(field) {
        for v in extra {
            if !out.iter().any(|o| o.eq_ignore_ascii_case(v)) {
                out.push(v.clone());
            }
        }
    }
    out
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

/// `library.facetPick`: a click on a Metadata column's value. Camera, lens, keyword, person, label
/// and every newer field toggle a value among the chosen ones, any of which matches; flag, exact
/// rating, type and year hold one value, which a second click clears. No value clears the field.
fn pick(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "library.facetPick";
    let asked = p.get("field").and_then(Value::as_str).map(str::trim).filter(|f| !f.is_empty()).unwrap_or("");
    let Some(field) = lcc_catalog::facets::canonical(asked).filter(|f| PICKABLE.contains(f)) else {
        return Err(bad(C, format!("field is one of {}", PICKABLE.join(", "))));
    };
    let value = match p.get("value") {
        None | Some(Value::Null) => None,
        Some(Value::String(v)) if !v.trim().is_empty() => {
            let v = v.trim();
            if v.len() > 512 {
                return Err(bad(C, "value is too long"));
            }
            Some(v.to_string())
        }
        Some(_) => return Err(bad(C, "value is a non-empty string (or null to clear the field)")),
    };
    let before = chosen(s, field);
    let on = value.as_ref().is_some_and(|v| before.iter().any(|b| b.eq_ignore_ascii_case(v)));
    // the chosen values once the click is done
    let next_all: Vec<String> = match &value {
        None => Vec::new(),
        Some(v) if on => before.iter().filter(|b| !b.eq_ignore_ascii_case(v)).cloned().collect(),
        Some(v) => before.iter().cloned().chain([v.clone()]).collect(),
    };
    if next_all.len() > MAX_CHOSEN {
        return Err(bad(C, "too many chosen values for one field"));
    }
    // the newer columns keep their choices on `Filter::facets`
    if lcc_catalog::facets::is_generic(field) {
        if let Some(v) = value.as_deref()
            && !lcc_catalog::facets::valid_value(field, v)
        {
            return Err(bad(C, format!("value `{v}` is not one {field} lists")));
        }
        if next_all.is_empty() {
            s.filter.facets.remove(field);
        } else {
            s.filter.facets.insert(field.to_string(), next_all);
        }
        return Ok(json!({"field": asked, "chosen": chosen(s, field), "count": s.visible().len()}));
    }
    if value.as_deref() == Some(lcc_catalog::facets::NO_VALUE) {
        return Err(bad(C, "the Unknown row is only for the newer Metadata columns"));
    }
    let next: Vec<String> = next_all.into_iter().filter(|v| v.as_str() != lcc_catalog::facets::NO_VALUE).collect();
    s.filter.facets.remove(field);
    // a single-valued field keeps only the last value clicked; clicking it again clears it
    let single = if on { None } else { value.clone() };
    match field {
        "camera" => (s.filter.camera, s.filter.cameras) = (None, next),
        "lens" => (s.filter.lens, s.filter.lenses) = (None, next),
        "keyword" => (s.filter.keyword, s.filter.keywords) = (None, next),
        "person" => (s.filter.person, s.filter.people) = (None, next),
        "label" => {
            let mut labels = Vec::new();
            for name in &next {
                let Some(l) = ColorLabel::ALL.into_iter().find(|l| s.catalog.label_name(*l).eq_ignore_ascii_case(name)) else {
                    return Err(bad(C, format!("no colour label is named `{name}`")));
                };
                labels.push(l);
            }
            labels.sort();
            labels.dedup();
            (s.filter.label, s.filter.labels) = (None, labels);
        }
        "flag" => {
            s.filter.flag = match single.as_deref() {
                None => None,
                Some("pick") => Some(Flag::Pick),
                Some("reject") => Some(Flag::Reject),
                Some("none") => Some(Flag::None),
                Some(other) => return Err(bad(C, format!("flag `{other}` is pick, reject or none"))),
            }
        }
        "rating" => match single {
            None => (s.filter.rating, s.filter.rating_op) = (0, RatingOp::AtLeast),
            Some(r) => {
                // (the filter has no "exactly unrated": a rating of 0 asks nothing)
                let r: u8 = r.parse().ok().filter(|r| (1..=5).contains(r)).ok_or_else(|| bad(C, format!("rating `{r}` is 1 to 5")))?;
                (s.filter.rating, s.filter.rating_op) = (r, RatingOp::Exactly);
            }
        },
        "kind" => {
            s.filter.kind = match single.as_deref() {
                None => None,
                Some("image") => Some(MediaKind::Image),
                Some("raw") => Some(MediaKind::Raw),
                Some("video") => Some(MediaKind::Video),
                Some(other) => return Err(bad(C, format!("kind `{other}` is image, raw or video"))),
            }
        }
        // "year"
        _ => {
            if single.as_ref().is_some_and(|y| y.len() != 4 || !y.chars().all(|c| c.is_ascii_digit())) {
                return Err(bad(C, "year is four digits"));
            }
            s.filter.date = single;
            (s.filter.date_from, s.filter.date_to) = (None, None);
        }
    }
    Ok(json!({"field": asked, "chosen": chosen(s, field), "count": s.visible().len()}))
}

/// The values `field` takes for photo `p` (several for keywords, people and overlapping saved
/// locations; none when it has no value).
fn values(s: &Session, field: &str, p: &Photo) -> Vec<String> {
    lcc_catalog::facets::values(field, p, &s.catalog, &s.filter)
}

fn number(s: &str) -> f64 {
    s.parse().unwrap_or(f64::MAX)
}

fn shutter_number(s: &str) -> f64 {
    lcc_catalog::parse_shutter_seconds(s).unwrap_or(f64::MAX)
}

/// Numeric and calendar fields sort by value; the rest by count, most first, then by name.
fn sort_rows(field: &str, rows: &mut Vec<(String, usize)>) {
    if matches!(field, "rating" | "iso" | "focal" | "aperture") {
        rows.sort_by(|a, b| number(&a.0).total_cmp(&number(&b.0)).then_with(|| a.0.cmp(&b.0)));
    } else if field == "shutter" {
        rows.sort_by(|a, b| shutter_number(&a.0).total_cmp(&shutter_number(&b.0)).then_with(|| a.0.cmp(&b.0)));
    } else if matches!(field, "year" | "month" | "day" | "timezone") {
        rows.sort_by(|a, b| a.0.cmp(&b.0));
    } else {
        rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
    }
}

fn facets(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "library.facets";
    // (output key, canonical field id): Classic's own field names are accepted as aliases.
    let fields: Vec<(String, &'static str)> = match p.get("fields") {
        None | Some(Value::Null) => FIELDS.iter().map(|f| ((*f).to_string(), *f)).collect(),
        Some(Value::Array(a)) => {
            let mut out: Vec<(String, &'static str)> = Vec::new();
            for v in a {
                let asked = v.as_str().map(str::trim).filter(|f| !f.is_empty()).unwrap_or("");
                let canonical = lcc_catalog::facets::canonical(asked)
                    .filter(|n| FIELDS.contains(n))
                    .ok_or_else(|| bad(C, format!("fields are among {}", FIELDS.join(", "))))?;
                if !out.iter().any(|(name, _)| name.as_str() == asked) {
                    out.push((asked.to_string(), canonical));
                }
            }
            out
        }
        Some(_) => return Err(bad(C, "fields must be a list of field names")),
    };
    let limit = match p.get("limit") {
        None | Some(Value::Null) => LIMIT_DEFAULT,
        Some(v) => {
            v.as_u64().map(|n| usize::try_from(n).unwrap_or(usize::MAX).min(LIMIT_MAX)).ok_or_else(|| bad(C, "limit must be a whole number"))?
        }
    };
    let mut out = serde_json::Map::new();
    for (name, field) in &fields {
        let f = s.view_filter(s.source, &without(*field, &s.filter));
        let ids = s.catalog.query(&f, &Sort::default());
        let mut counts: HashMap<String, usize> = HashMap::new();
        let mut none = 0usize;
        for photo in ids.iter().filter_map(|id| s.catalog.photo(*id)) {
            let mut vals = values(s, *field, photo);
            vals.sort();
            vals.dedup();
            if vals.is_empty() {
                none += 1;
            }
            for v in vals {
                *counts.entry(v).or_default() += 1;
            }
        }
        let distinct = counts.len();
        let mut rows: Vec<(String, usize)> = counts.into_iter().collect();
        sort_rows(*field, &mut rows);
        let shown: Vec<Value> = rows.into_iter().take(limit).map(|(value, count)| json!({"value": value, "count": count})).collect();
        out.insert(name.clone(), json!({"photos": ids.len(), "none": none, "distinct": distinct, "values": shown, "chosen": chosen(s, *field)}));
    }
    Ok(Value::Object(out))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "library.facetPick",
            "Pick Metadata Value",
            [],
            None,
            "{field: a Metadata column id or Classic name (see view.filterColumns), value?: a value as library.facets lists it, noValue for a column's Unknown row (null: clear the field)} → {field, chosen, count} — a Metadata filter column's click: camera, lens, keyword, person, label and every newer field toggle the value among the chosen ones (any matches); flag, rating (exactly), kind and year take the one value, and a second click clears it",
            always,
            pick
        ),
        cmd!(
        query "library.facets",
        "Metadata Facets",
        [],
        None,
        "{fields?: [year | month | day | timezone | kind | flag | rating | exported | label | keyword | person | edit | camera | cameraSerial | lens | focal | shutter | aperture | iso | flash | gpsData | mapLocation | location | city | state | country | creator | copyrightStatus | job, or a Classic name / alias] (default: all), limit? (values per field, default 200, at most 2000)} → {<field>: {photos, none, distinct, values: [{value, count}], chosen: [the values the filter picked]}} — the values each field takes among the photos in view (the source, and the filter without that field's own constraint), with how many photos have each; `none`: photos without one. Counts by most photos first; rating, year, month, day, timezone, ISO, focal length, aperture and shutter speed by value",
            always,
            facets
        ),
    ]
}
