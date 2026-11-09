//! `library.facets`: the distinct values of metadata fields among the photos in view, with how many photos have
//! each (issue #218: the first piece of Lightroom Classic's Metadata filter columns, and of a table view's column
//! filters). Each field is counted with its own constraint left out of the filter, so a field already narrowed to
//! one value still lists the others it could take; the other fields' constraints and the source apply.

use std::collections::HashMap;

use lcc_catalog::facets::{flag_name, kind_name};
use lcc_catalog::{ColorLabel, FacetPick, Filter, Flag, MediaKind, Photo, RatingOp, Sort};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd};
use crate::{Result, Session};

/// The fields `library.facets` counts (and `library.facetPick` narrows the view by).
pub fn fields() -> Vec<&'static str> {
    lcc_catalog::facets::all_fields().collect()
}
/// Values one field returns without a `limit`.
pub const LIMIT_DEFAULT: usize = 200;
/// Most values one field returns.
pub const LIMIT_MAX: usize = 2000;

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
        "flag" => {
            f.flag = None;
            f.flags.clear();
        }
        "rating" => f.rating = 0,
        "kind" => f.kind = None,
        "year" => {
            f.date = None;
            f.date_from = None;
            f.date_to = None;
        }
        // the fields picked through `Filter::facets` only; the search text (`iso:>800`) stays
        _ => {}
    }
    f.facets.retain(|p| p.field != field);
    f
}

/// The filter's column pick on `field` in [`Filter::facets`], if any.
fn facet<'a>(f: &'a Filter, field: &str) -> Option<&'a FacetPick> {
    f.facets.iter().find(|p| p.field == field)
}

/// Whether the filter lets photos without a value for `field` through, as a column's "no value" row picks.
pub fn none_chosen(s: &Session, field: &str) -> bool {
    facet(&s.filter, field).is_some_and(|p| p.none)
}

/// The values of `field` the filter has chosen, spelled as `library.facets` lists them.
pub fn chosen(s: &Session, field: &str) -> Vec<String> {
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
    // a column whose no-value row is picked keeps its values in `Filter::facets`
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
        "rating" if f.rating_op == RatingOp::Exactly && f.rating == 0 => vec!["0".to_string()],
        "year" => f.date.iter().filter(|d| d.len() == 4).cloned().collect(),
        _ => facet(f, field).map(|p| p.values.clone()).unwrap_or_default(),
    };
    if let Some(pk) = facet(f, field) {
        for value in &pk.values {
            if !out.iter().any(|o| o.eq_ignore_ascii_case(value)) {
                out.push(value.clone());
            }
        }
    }
    out
}

/// `library.facetPick`: a click on a Metadata column's value. Fields that take several values (camera, lens,
/// keyword, person, label) toggle it among the chosen ones, any of which matches; the others hold one value, which
/// a second click clears. No value clears the field. `none: true` toggles the photos without a value instead
/// (the column's "no value" row), which combine with the chosen values (any matches).
fn pick(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "library.facetPick";
    let Some(field) = p.get("field").and_then(Value::as_str).filter(|f| lcc_catalog::facets::is_field(f)) else {
        return Err(bad(C, format!("field is one of {}", fields().join(", "))));
    };
    match p.get("none") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => {}
        Some(Value::Bool(true)) => {
            if matches!(field, "flag" | "rating" | "kind") {
                return Err(bad(C, format!("every photo has a {field}: pick its value (flag none, rating 0) instead")));
            }
            let on = !none_chosen(s, field);
            // turning the no-value row on unites it with values picked before: they move from the
            // dedicated fields into the `Filter::facets` entry (the union), the dedicated fields
            // would AND with it and the column would come out empty
            if on && matches!(field, "camera" | "lens" | "keyword" | "person" | "label") {
                let mut values = chosen(s, field);
                match field {
                    "camera" => (s.filter.camera, s.filter.cameras) = (None, Vec::new()),
                    "lens" => (s.filter.lens, s.filter.lenses) = (None, Vec::new()),
                    "keyword" => (s.filter.keyword, s.filter.keywords) = (None, Vec::new()),
                    "person" => (s.filter.person, s.filter.people) = (None, Vec::new()),
                    _ => (s.filter.label, s.filter.labels) = (None, Vec::new()),
                }
                match s.filter.facets.iter_mut().find(|p| p.field == field) {
                    Some(pk) => {
                        pk.values.clear();
                        pk.values.append(&mut values);
                        pk.none = true;
                    }
                    None => s.filter.facets.push(FacetPick { field: field.to_string(), values, none: true }),
                }
                s.filter.facets.retain(|p| !p.is_empty());
                return Ok(json!({"field": field, "chosen": chosen(s, field), "none": none_chosen(s, field), "count": s.visible().len()}));
            }
            match s.filter.facets.iter_mut().find(|p| p.field == field) {
                Some(pk) => pk.none = on,
                None => s.filter.facets.push(FacetPick { field: field.to_string(), values: Vec::new(), none: on }),
            }
            s.filter.facets.retain(|p| !p.is_empty());
            return Ok(json!({"field": field, "chosen": chosen(s, field), "none": none_chosen(s, field), "count": s.visible().len()}));
        }
        Some(_) => return Err(bad(C, "none is true or false")),
    }
    let value = match p.get("value") {
        None | Some(Value::Null) => None,
        Some(Value::String(v)) if !v.trim().is_empty() => Some(v.trim().to_string()),
        Some(_) => return Err(bad(C, "value is a non-empty string (or null to clear the field)")),
    };
    let before = chosen(s, field);
    let on = value.as_ref().is_some_and(|v| before.iter().any(|b| b.eq_ignore_ascii_case(v)));
    // the chosen values once the click is done
    let next: Vec<String> = match &value {
        None => Vec::new(),
        Some(v) if on => before.iter().filter(|b| !b.eq_ignore_ascii_case(v)).cloned().collect(),
        Some(v) => before.iter().cloned().chain([v.clone()]).collect(),
    };
    // a single-valued field keeps only the last value clicked; clicking it again clears it
    let single = if on { None } else { value.clone() };
    if value.is_none() {
        s.filter.facets.retain(|p| p.field != field);
    }
    match field {
        // with the column's no-value row picked, the values go into the `Filter::facets` entry
        // (which unites them with the photos without a value): the dedicated fields would AND
        // with it and the union would come out empty
        "camera" | "lens" | "keyword" | "person" | "label" if none_chosen(s, field) => {
            let none = none_chosen(s, field);
            s.filter.facets.retain(|p| p.field != field);
            let pk = FacetPick { field: field.to_string(), values: next, none };
            if !pk.is_empty() {
                s.filter.facets.push(pk);
            }
        }
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
                // 0: exactly unrated
                let r: u8 = r.parse().ok().filter(|r| *r <= 5).ok_or_else(|| bad(C, format!("rating `{r}` is 0 (unrated) to 5")))?;
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
        "year" => {
            if single.as_ref().is_some_and(|y| y.len() != 4 || !y.chars().all(|c| c.is_ascii_digit())) {
                return Err(bad(C, "year is four digits"));
            }
            s.filter.date = single;
            (s.filter.date_from, s.filter.date_to) = (None, None);
        }
        // the fields picked through `Filter::facets`: several values, any of which matches
        _ => {
            let none = none_chosen(s, field);
            s.filter.facets.retain(|p| p.field != field);
            let pk = FacetPick { field: field.to_string(), values: next, none };
            if !pk.is_empty() {
                s.filter.facets.push(pk);
            }
        }
    }
    Ok(json!({"field": field, "chosen": chosen(s, field), "none": none_chosen(s, field), "count": s.visible().len()}))
}

/// The values `field` takes for photo `p` (several for keywords and people; none when it has no value).
fn values(s: &Session, field: &str, p: &Photo) -> Vec<String> {
    lcc_catalog::facets::values(p, &s.catalog, field)
}

/// Numeric fields sort by value; the rest by count, most first, then by name.
fn numeric(field: &str) -> bool {
    matches!(field, "rating" | "year" | "iso" | "focal" | "aperture" | "shutter")
}

fn facets(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "library.facets";
    let fields: Vec<String> = match p.get("fields") {
        None | Some(Value::Null) => fields().into_iter().map(str::to_string).collect(),
        Some(Value::Array(a)) => {
            let mut out = Vec::new();
            for v in a {
                let Some(name) = v.as_str().filter(|n| lcc_catalog::facets::is_field(n)) else {
                    return Err(bad(C, format!("fields are among {}", fields().join(", "))));
                };
                if !out.iter().any(|o| o == name) {
                    out.push(name.to_string());
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
    for field in &fields {
        let f = s.view_filter(s.source, &without(field, &s.filter));
        let ids = s.catalog.query(&f, &Sort::default());
        let mut counts: HashMap<String, usize> = HashMap::new();
        let mut none = 0usize;
        for photo in ids.iter().filter_map(|id| s.catalog.photo(*id)) {
            let mut vals = values(s, field, photo);
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
        if numeric(field) {
            rows.sort_by(|a, b| {
                let key = |v: &str| lcc_catalog::facets::sort_key(field, v).unwrap_or(f64::MAX);
                key(&a.0).total_cmp(&key(&b.0)).then_with(|| a.0.cmp(&b.0))
            });
        } else {
            rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
        }
        let shown: Vec<Value> = rows.into_iter().take(limit).map(|(value, count)| json!({"value": value, "count": count})).collect();
        out.insert(field.clone(), json!({"photos": ids.len(), "none": none, "distinct": distinct, "values": shown, "chosen": chosen(s, field), "noneChosen": none_chosen(s, field)}));
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
            "{field: a library.facets field, value?: a value as library.facets lists it (null: clear the field), none?: true (toggle the photos without a value)} → {field, chosen, none, count} — a Metadata filter column's click: flag, rating (exactly; 0 = unrated), kind and year take the one value, and a second click clears it; the other fields toggle the value among the chosen ones (any matches). `none` adds or removes the photos without a value (no camera, keyword, label, capture date, title…), which any chosen value also lets through",
            always,
            pick
        ),
        cmd!(
        query "library.facets",
        "Metadata Facets",
        [],
        None,
        "{fields?: [camera | lens | keyword | person | label | flag | rating | kind | year | iso | focal | format | title | headline | caption | creator | copyright | location | city | country | aspect | preset | ai] (default: all), limit? (values per field, default 200, at most 2000)} → {<field>: {photos, none, distinct, values: [{value, count}], chosen: [the values the filter picked], noneChosen}} — format: the file format (JPG, CR3, TIFF…); aspect: the developed aspect ratio (3:2, 4:5, 16:9…); preset: develop presets applied (from History); ai: AI tools in use (Denoise, Subject mask…) — the values each field takes among the photos in view (the source, and the filter without that field's own constraint), with how many photos have each; `none`: photos without one. Counts by most photos first; rating, year, ISO and focal length by value",
            always,
            facets
        ),
    ]
}
