//! Recognition commands: `faces.index` (embed the faces in the library), `faces.suggest` (who might an unnamed
//! face be?), `faces.person` (one person's faces, and the unnamed faces that look like them), `faces.unnamed` (every
//! unnamed face, look-alikes together) and `faces.setName` / `faces.nameFaces` (name one face, or many at once, which is
//! what teaches the suggestions). `faces.renamePerson` renames every in-library face of one person, or merges two
//! people when asked.
//!
//! Everything here is a suggestion for the user to confirm; nothing is ever named automatically. All of it is
//! catalog-only (no sidecar is written) and naming is undoable.

use lcc_catalog::Op;
use serde_json::{Value, json};

use super::{CommandSpec, bad, cmd};
use crate::{Result, Session};

/// The `name` parameter of the naming commands: a name, or none (`null`, empty) to clear it.
fn name_param(p: &Value, c: &str) -> Result<Option<String>> {
    let name = match p.get("name") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_str().ok_or_else(|| bad(c, "`name` must be text or null"))?.trim().to_string()).filter(|n| !n.is_empty()),
    };
    if let Some(n) = &name
        && (n.chars().count() > 200 || n.chars().any(char::is_control))
    {
        return Err(bad(c, "a name is at most 200 characters, without control characters"));
    }
    Ok(name)
}

/// `faces.ignore`: ignore faces (or restore them, `ignored: false`) in one undo step. Every face is checked before
/// anything changes, so a stale selection changes nothing.
fn ignore_faces(s: &mut Session, p: &Value) -> Result<Value> {
    use lcc_meta::RegionKind;
    const C: &str = "faces.ignore";
    let ignored = match p.get("ignored") {
        None => true,
        Some(v) => v.as_bool().ok_or_else(|| bad(C, "`ignored` must be true or false"))?,
    };
    let faces = p.get("faces").and_then(Value::as_array).ok_or_else(|| bad(C, "missing `faces`"))?;
    if faces.is_empty() || faces.len() > 100_000 {
        return Err(bad(C, "select between 1 and 100000 faces"));
    }
    let mut edits: std::collections::BTreeMap<lcc_catalog::PhotoId, lcc_catalog::Meta> = Default::default();
    let mut seen = std::collections::HashSet::new();
    for face in faces {
        let id = lcc_catalog::PhotoId(face.get("photo").and_then(Value::as_u64).ok_or_else(|| bad(C, "missing `photo`"))?);
        let index = face.get("index").and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok()).ok_or_else(|| bad(C, "invalid face `index`"))?;
        if !seen.insert((id, index)) {
            continue;
        }
        let photo = s.catalog.photo(id).filter(|p| p.in_library()).ok_or_else(|| bad(C, "photo is no longer in the catalog"))?;
        let region = photo.meta.regions.get(index).ok_or_else(|| bad(C, "face no longer exists"))?;
        let expected = if ignored { RegionKind::Face } else { RegionKind::IgnoredFace };
        if region.kind != expected {
            return Err(bad(C, "a selected face changed; reload the faces and try again"));
        }
        if let Some(rect) = face.get("rect") {
            let rect: lcc_geom::Rect = serde_json::from_value(rect.clone()).map_err(|_| bad(C, "invalid `rect`"))?;
            if rect != region.rect {
                return Err(bad(C, "a selected face changed; reload the faces and try again"));
            }
        }
        let meta = edits.entry(id).or_insert_with(|| photo.meta.clone());
        if let Some(region) = meta.regions.get_mut(index) {
            region.kind = if ignored { RegionKind::IgnoredFace } else { RegionKind::Face };
        }
    }
    let count = seen.len();
    s.commit(
        if ignored { "Ignore Faces" } else { "Restore Ignored Faces" },
        Op::Batch { ops: edits.into_iter().map(|(id, meta)| Op::SetMeta { id, meta: Box::new(meta) }).collect() },
    )?;
    // regions are not written to XMP, so a sidecar must not be rewritten for this
    s.skip_auto_write = true;
    Ok(json!({"changed": count, "ignored": ignored}))
}

/// `faces.ignored`: one page of the ignored faces, in catalog order (needs no recognition model).
fn ignored_faces(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "faces.ignored";
    let offset = match p.get("offset") {
        None => 0,
        Some(v) => v.as_u64().and_then(|n| usize::try_from(n).ok()).ok_or_else(|| bad(C, "`offset` must be a non-negative index"))?,
    };
    let limit = match p.get("limit") {
        None => 1000,
        Some(v) => v.as_u64().and_then(|n| usize::try_from(n).ok()).ok_or_else(|| bad(C, "`limit` must be between 1 and 5000"))?,
    };
    if !(1..=5000).contains(&limit) {
        return Err(bad(C, "`limit` must be between 1 and 5000"));
    }
    let mut faces = Vec::new();
    let mut total = 0usize;
    for photo in s.catalog.photos().filter(|p| p.in_library()) {
        for (index, region) in photo.meta.regions.iter().enumerate().filter(|(_, r)| r.kind == lcc_meta::RegionKind::IgnoredFace) {
            if total >= offset && faces.len() < limit {
                faces.push(json!({"photo": photo.id.0, "index": index, "rect": region.rect, "name": region.name, "view": s.face_view(photo.id, region.rect)}));
            }
            total = total.saturating_add(1);
        }
    }
    Ok(json!({"total": total, "offset": offset, "faces": faces}))
}

/// Give a face region its name. Naming makes the face yours: a later "Detect Faces" run no longer replaces it.
fn apply_name(region: &mut lcc_meta::Region, name: &Option<String>) {
    region.name = name.clone();
    if name.is_some() && region.description.as_deref().is_some_and(|d| d.starts_with(super::face_detect::MARK)) {
        region.description = None;
    }
}

/// `faces.nameFaces {faces: [{photo, index}], name}`: name (or clear) many face regions at once, as one undo step: what
/// naming a group of look-alikes does.
fn name_faces(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "faces.nameFaces";
    let name = name_param(p, C)?;
    let list = p.get("faces").and_then(Value::as_array).ok_or_else(|| bad(C, "missing `faces`"))?;
    if list.len() > 5000 {
        return Err(bad(C, "at most 5000 faces at a time"));
    }
    let mut by_photo: std::collections::BTreeMap<u64, Vec<usize>> = std::collections::BTreeMap::new();
    for f in list {
        let (Some(photo), Some(index)) =
            (f.get("photo").and_then(Value::as_u64), f.get("index").and_then(Value::as_u64).and_then(|i| usize::try_from(i).ok()))
        else {
            return Err(bad(C, "each face needs a `photo` and an `index`"));
        };
        by_photo.entry(photo).or_default().push(index);
    }
    let (mut ops, mut named) = (Vec::new(), 0usize);
    for (photo, indexes) in by_photo {
        let id = lcc_catalog::PhotoId(photo);
        let Some(photo) = s.catalog.photo(id) else { continue };
        let mut meta = photo.meta.clone();
        let before = named;
        for i in indexes {
            if let Some(region) = meta.regions.get_mut(i).filter(|r| r.kind == lcc_meta::RegionKind::Face) {
                apply_name(region, &name);
                named += 1;
            }
        }
        if named > before {
            ops.push(Op::SetMeta { id, meta: Box::new(meta) });
        }
    }
    if ops.is_empty() {
        return Err(bad(C, "none of those faces exist"));
    }
    let photos = ops.len();
    s.commit(if name.is_some() { "Name Faces" } else { "Clear Face Names" }, Op::Batch { ops })?;
    s.skip_auto_write = true;
    Ok(json!({"named": named, "photos": photos, "name": name}))
}

/// `faces.setName {id?, index, name}`: name (or, with `null` or an empty name, un-name) one face region. Naming
/// makes the face yours: a later "Detect Faces" run no longer replaces it.
fn set_name(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "faces.setName";
    let id = super::photo_id(p, C)?.or_else(|| s.active()).ok_or_else(|| bad(C, "no photo"))?;
    let index = p.get("index").and_then(Value::as_u64).and_then(|i| usize::try_from(i).ok()).ok_or_else(|| bad(C, "missing or invalid `index`"))?;
    let name = name_param(p, C)?;
    let mut meta = s.catalog.photo(id).ok_or_else(|| bad(C, "no such photo"))?.meta.clone();
    let region = meta.regions.get_mut(index).ok_or_else(|| bad(C, "no such region"))?;
    apply_name(region, &name);
    s.commit(if name.is_some() { "Name Face" } else { "Clear Face Name" }, Op::SetMeta { id, meta: Box::new(meta) })?;
    s.skip_auto_write = true;
    Ok(json!({"id": id.0, "index": index, "name": name}))
}

/// The faces a decision command was given, as `(photo, region index, the face's box)`, checked before anything changes:
/// each must be an unnamed Face region of a photo in the library (a stale list changes nothing). Reads nothing but the
/// catalog, and refuses more than 5,000 at a time.
fn unnamed_faces_param(s: &Session, cmd: &str, p: &Value) -> Result<Vec<(u64, usize, lcc_geom::Rect)>> {
    let list = p.get("faces").and_then(Value::as_array).ok_or_else(|| bad(cmd, "missing `faces`"))?;
    if list.is_empty() || list.len() > 5000 {
        return Err(bad(cmd, "select between 1 and 5000 faces"));
    }
    let mut out = Vec::with_capacity(list.len());
    let mut seen = std::collections::HashSet::new();
    for face in list {
        let id = lcc_catalog::PhotoId(face.get("photo").and_then(Value::as_u64).ok_or_else(|| bad(cmd, "missing `photo`"))?);
        let index =
            face.get("index").and_then(Value::as_u64).and_then(|i| usize::try_from(i).ok()).ok_or_else(|| bad(cmd, "invalid face `index`"))?;
        if !seen.insert((id, index)) {
            continue;
        }
        let photo = s.catalog.photo(id).filter(|p| p.in_library()).ok_or_else(|| bad(cmd, "photo is no longer in the catalog"))?;
        let region = photo.meta.regions.get(index).ok_or_else(|| bad(cmd, "face no longer exists"))?;
        if region.kind != lcc_meta::RegionKind::Face || region.name.is_some() {
            return Err(bad(cmd, "a selected face changed; reload the faces and try again"));
        }
        if let Some(rect) = face.get("rect") {
            let rect: lcc_geom::Rect = serde_json::from_value(rect.clone()).map_err(|_| bad(cmd, "invalid `rect`"))?;
            if rect != region.rect {
                return Err(bad(cmd, "a selected face changed; reload the faces and try again"));
            }
        }
        out.push((id.0, index, region.rect));
    }
    Ok(out)
}

/// Today, as `people.json` writes the day of a decision (`YYYY-MM-DD`, the day it is where the user is).
fn today() -> String {
    chrono::Local::now().date_naive().format("%Y-%m-%d").to_string()
}

/// `faces.confirmSuggestions {faces: [{photo, index, rect?}]}`: the user agreed with what the named faces suggest. Each
/// face is given the name that was suggested for it now, so confirming a stale list never names a face something it was
/// not seen to look like: a face with no suggestion is left alone. One undo step, catalog only; the decisions are kept
/// in `people.json` (see `faces.suggestionDecisions`).
fn confirm_suggestions(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "faces.confirmSuggestions";
    let chosen = unnamed_faces_param(s, C, p)?;
    // what the named faces suggest for these faces now (never a stale name from the list the user looked at, nor one
    // the library-wide scan has not judged again since)
    let suggestions = imp::suggest_for(s, &chosen);
    let mut by_photo: std::collections::BTreeMap<u64, Vec<(usize, String)>> = std::collections::BTreeMap::new();
    let mut decisions: Vec<(u64, usize, lcc_geom::Rect, String)> = Vec::new();
    {
        // a face the user rejected as this person is not suggested as them, so it is not confirmed as them either
        let store = s.people_store();
        for (photo, index, rect) in &chosen {
            let Some((name, _)) = suggestions.get(&(*photo, *index)) else { continue };
            let name = name.trim();
            if name.is_empty() || store.rejected_as(*photo, *index, *rect, name) {
                continue;
            }
            decisions.push((*photo, *index, *rect, name.to_string()));
        }
    }
    for (photo, index, _, name) in &decisions {
        by_photo.entry(*photo).or_default().push((*index, name.clone()));
    }
    let mut named = 0usize;
    let mut ops = Vec::new();
    for (photo, faces) in by_photo {
        let id = lcc_catalog::PhotoId(photo);
        let Some(p) = s.catalog.photo(id) else { continue };
        let mut meta = p.meta.clone();
        for (index, name) in faces {
            if let Some(region) = meta.regions.get_mut(index).filter(|r| r.kind == lcc_meta::RegionKind::Face) {
                apply_name(region, &Some(name));
                named += 1;
            }
        }
        ops.push(Op::SetMeta { id, meta: Box::new(meta) });
    }
    if named > 0 {
        s.commit("Confirm Faces", Op::Batch { ops })?;
        s.skip_auto_write = true;
    }
    // the decisions are remembered so the groups drop them before the face lists are next read
    let day = today();
    let kept = s
        .with_people(|store| {
            let mut kept = 0usize;
            for (photo, index, rect, name) in &decisions {
                if store.decide(*photo, *index, *rect, name, crate::people_store::Verdict::Confirmed, &day).is_ok() {
                    kept += 1;
                }
            }
            kept
        })
        .map_err(crate::EngineError::Other)?;
    Ok(json!({"confirmed": named, "suggested": chosen.len(), "kept": kept}))
}

/// `faces.rejectSuggestions {faces: [{photo, index, rect?}]}`: the user said a suggested face is not that person. The
/// face stays where it is (unnamed and unknown, not ignored, still detected and never named automatically); the person
/// it was suggested as is remembered, so it is not suggested as them again. Nothing on a photo changes, so this makes no
/// catalog undo step: the app offers a toast that calls `faces.undoSuggestionDecision`.
fn reject_suggestions(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "faces.rejectSuggestions";
    let chosen = unnamed_faces_param(s, C, p)?;
    // a rejected face is not suggested again, so what is suggested for these faces now is worked out here
    let suggestions = imp::suggest_for(s, &chosen);
    let day = today();
    // a face already rejected as the person it is suggested as is not suggested as them: there is nothing to reject
    let decisions: Vec<(u64, usize, lcc_geom::Rect, String)> = {
        let store = s.people_store();
        chosen
            .iter()
            .filter_map(|(photo, index, rect)| {
                let name = suggestions.get(&(*photo, *index))?.0.clone();
                (!store.rejected_as(*photo, *index, *rect, &name)).then_some((*photo, *index, *rect, name))
            })
            .collect()
    };
    let kept = s
        .with_people(|store| {
            let mut kept = 0usize;
            for (photo, index, rect, name) in &decisions {
                if store.decide(*photo, *index, *rect, name, crate::people_store::Verdict::Rejected, &day).is_ok() {
                    kept += 1;
                }
            }
            kept
        })
        .map_err(crate::EngineError::Other)?;
    Ok(json!({"rejected": kept, "faces": chosen.len()}))
}

/// `faces.undoSuggestionDecision {faces: [{photo, index}]}`: forget what was decided about these faces, so the person
/// they were rejected as is suggested again and they can be confirmed or rejected afresh. Nothing on a photo changes,
/// so this has no catalog undo step of its own (the app offers a toast for it).
fn undo_suggestion_decision(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "faces.undoSuggestionDecision";
    let list = p.get("faces").and_then(Value::as_array).ok_or_else(|| bad(C, "missing `faces`"))?;
    if list.is_empty() || list.len() > 5000 {
        return Err(bad(C, "select between 1 and 5000 faces"));
    }
    let mut keys = std::collections::BTreeSet::new();
    for face in list {
        let photo = face.get("photo").and_then(Value::as_u64).ok_or_else(|| bad(C, "missing `photo`"))?;
        let index = face.get("index").and_then(Value::as_u64).and_then(|i| usize::try_from(i).ok()).ok_or_else(|| bad(C, "invalid face `index`"))?;
        keys.insert((photo, index));
    }
    let removed = s
        .with_people(|store| {
            let before = store.decisions.len();
            store.decisions.retain(|d| !keys.contains(&(d.photo, d.index)));
            before - store.decisions.len()
        })
        .map_err(crate::EngineError::Other)?;
    Ok(json!({"removed": removed, "faces": keys.len()}))
}

/// Most decisions `faces.suggestionDecisions` lists at once.
const MAX_DECISIONS_LISTED: usize = 2000;

/// `faces.suggestionDecisions {photo?}`: what was decided about suggested faces (rejected, or confirmed), newest
/// decisions included, from `people.json`.
fn suggestion_decisions(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "faces.suggestionDecisions";
    let photo = match p.get("photo") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_u64().ok_or_else(|| bad(C, "`photo` must be a photo id"))?),
    };
    // a page at a time, as `faces.ignored` lists: at most `MAX_DECISIONS_LISTED` rows, newest first (the latest decisions
    // are the likeliest undone), after `after` of the previous page; a library can hold a decision about every face it knows
    let start = match p.get("after") {
        None | Some(Value::Null) => 0,
        Some(v) => usize::try_from(v.as_u64().ok_or_else(|| bad(C, "`after` must be a count"))?).map_err(|_| bad(C, "`after` is too large"))?,
    };
    let limit = match p.get("limit") {
        None | Some(Value::Null) => MAX_DECISIONS_LISTED,
        Some(v) => usize::try_from(v.as_u64().ok_or_else(|| bad(C, "`limit` must be a count"))?)
            .map_err(|_| bad(C, "`limit` is too large"))?
            .clamp(1, MAX_DECISIONS_LISTED),
    };
    let store = s.people_store();
    let mut listed: Vec<&crate::people_store::SuggestionDecision> = store.decisions.iter().filter(|d| photo.is_none_or(|id| d.photo == id)).collect();
    listed.sort_by(|a, b| b.decided.cmp(&a.decided).then_with(|| (b.photo, b.index).cmp(&(a.photo, a.index))));
    Ok(json!({"total": listed.len(), "decisions": decisions_page(&listed, start, limit)}))
}

/// One page of `listed` (already in the order to show), from `start`, `limit` long.
fn decisions_page(listed: &[&crate::people_store::SuggestionDecision], start: usize, limit: usize) -> Vec<Value> {
    listed
        .iter()
        .skip(start)
        .take(limit)
        .map(|d| {
            let verdict = match d.verdict {
                crate::people_store::Verdict::Rejected => "rejected",
                crate::people_store::Verdict::Confirmed => "confirmed",
            };
            json!({"photo": d.photo, "index": d.index, "person": d.person, "verdict": verdict, "decided": d.decided})
        })
        .collect()
}

/// `faces.renamePerson {from, to, merge?}`: give every in-library face of `from` the exact name `to`, as one undo
/// step. Names match the People view (trimmed, case-insensitive). A `to` that is already someone else is refused
/// unless `merge` is set, and then the two people become one. The sidecar is not touched, and a refusal changes nothing.
fn rename_person(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "faces.renamePerson";
    let from = match p.get("from") {
        Some(Value::String(v)) => {
            let t = v.trim();
            if t.is_empty() {
                return Err(bad(C, "missing `from`"));
            }
            t.to_string()
        }
        Some(_) => return Err(bad(C, "`from` must be text")),
        None => return Err(bad(C, "missing `from`")),
    };
    let to = match p.get("to") {
        Some(Value::String(v)) => {
            let t = v.trim();
            if t.is_empty() {
                return Err(bad(C, "`to` must be a name"));
            }
            if t.chars().count() > 200 || t.chars().any(char::is_control) {
                return Err(bad(C, "a name is at most 200 characters, without control characters"));
            }
            t.to_string()
        }
        Some(_) => return Err(bad(C, "`to` must be text")),
        None => return Err(bad(C, "missing `to`")),
    };
    let merge = super::bool_or(p, "merge", false);
    let from_key = from.to_lowercase();
    let to_key = to.to_lowercase();
    let mut matched = 0usize;
    let mut other = false;
    let mut faces = 0usize;
    let mut ops = Vec::new();
    let ids: Vec<_> = s.catalog.photos().filter(|ph| ph.in_library()).map(|ph| ph.id).collect();
    for id in ids {
        let Some(ph) = s.catalog.photo(id) else { continue };
        let mut indexes = Vec::new();
        for (i, r) in ph.meta.regions.iter().enumerate() {
            if !matches!(r.kind, lcc_meta::RegionKind::Face | lcc_meta::RegionKind::IgnoredFace) {
                continue;
            }
            let Some(n) = r.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) else { continue };
            let key = n.to_lowercase();
            if key == from_key {
                matched += 1;
                if r.name.as_deref() != Some(to.as_str()) {
                    indexes.push(i);
                }
            } else if key == to_key {
                other = true;
            }
        }
        if indexes.is_empty() {
            continue;
        }
        let mut meta = ph.meta.clone();
        for i in indexes {
            if let Some(region) = meta.regions.get_mut(i) {
                region.name = Some(to.clone());
                faces += 1;
            }
        }
        ops.push(Op::SetMeta { id, meta: Box::new(meta) });
    }
    if matched == 0 {
        return Err(bad(C, format!("no person named {from}")));
    }
    if faces == 0 {
        return Err(bad(C, format!("{to} is already their name")));
    }
    if other && !merge {
        return Err(bad(C, format!("{to} is already another person; pass merge: true to combine them")));
    }
    let photos = ops.len();
    s.commit(if other { "Merge People" } else { "Rename Person" }, Op::Batch { ops })?;
    s.skip_auto_write = true;
    if s.filter.person.as_deref().is_some_and(|n| n.trim().to_lowercase() == from_key) {
        s.filter.person = Some(to.clone());
    }
    if let Some(f) = s.shelved_filter.as_mut()
        && f.person.as_deref().is_some_and(|n| n.trim().to_lowercase() == from_key)
    {
        f.person = Some(to.clone());
    }
    Ok(json!({"faces": faces, "photos": photos, "from": from, "to": to, "merged": other}))
}

/// Photos to look in, from `scope`: absent, the whole library; `"filter"`, the photos the library shows (a year, a
/// folder, a collection picked in the sidebar), any person filter aside, so People and a person's page follow the sidebar.
type Scope = Option<std::collections::HashSet<u64>>;

fn scope_of(s: &Session, cmd: &str, p: &Value) -> Result<Scope> {
    match p.get("scope") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(v)) if v == "filter" => {
            // what the People view shows, the sidebar's source included: a folder or a collection is not in `s.filter`
            // (issue #685)
            let f = s.people_filter();
            Ok(Some(s.catalog.query(&f, &lcc_catalog::Sort::default()).into_iter().map(|id| id.0).collect()))
        }
        Some(_) => Err(bad(cmd, "`scope` must be \"filter\" or left out")),
    }
}

fn in_scope(scope: &Scope, photo: u64) -> bool {
    scope.as_ref().is_none_or(|v| v.contains(&photo))
}

/// Most faces `faces.unnamed` lists.
const MAX_UNNAMED: usize = 3000;

/// How the library-wide look for suggestions is going ([`imp::scan_suggestions`]): a suggestion is the library's whole
/// answer only once `complete`, and the Suggested category says so while it is not (issue #1365).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Progress {
    /// Unnamed faces with an embedding in the library.
    total: usize,
    /// How many of them have been judged against the named faces.
    judged: usize,
    /// Every one of them was judged, so nothing is left to find.
    complete: bool,
}

impl Progress {
    fn json(&self) -> Value {
        json!({"total": self.total, "judged": self.judged, "complete": self.complete})
    }
}

/// How the unnamed faces were laid out.
struct Arranged {
    /// Faces that look alike are next to each other (only the ones already embedded can be).
    ordered: bool,
    /// A recognition model is chosen and its face index is available.
    ready: bool,
    /// A name for some faces, when the faces already named say so with confidence: `(photo, index)` to `(name, score)`.
    suggestions: std::collections::HashMap<(u64, usize), (String, f32)>,
    /// Best similarity to any named person for each embedded, listed face, including faces below the suggestion bars.
    similarities: std::collections::HashMap<(u64, usize), (String, f32)>,
    /// Look-alike groups (#259): faces next to each other in the order that are as alike as a match needs share a
    /// group number, so a frontend can select (and name or ignore) a group at once. `(photo, index)` to the group.
    groups: std::collections::HashMap<(u64, usize), usize>,
}

/// The unnamed faces the named ones recognise, grouped by the name they suggest (issue #1050): one entry per person
/// suggested, the people with the most suggested faces first (then by name), and within a group the faces that look
/// most like them first (then in photo order). Each face is in one group, the best name it was given.
///
/// Every suggested face the scope holds is in a group, not only those of the page `faces.unnamed` lists (issue
/// #1365): which page a face falls on is no reason to hide the person it looks like, and a group says how many faces
/// the library has for that person rather than how many happened to be listed. A face whose suggestion the user
/// rejected as that person is left out, and so is a key that is no longer an unnamed face (named, ignored or removed
/// since the scan judged it).
#[cfg(not(target_arch = "wasm32"))]
fn suggestion_groups(
    s: &mut Session,
    suggestions: &std::collections::HashMap<(u64, usize), (String, f32)>,
    scope: &Scope,
    people: &std::collections::HashSet<String>,
) -> Value {
    /// One suggested face that is still unnamed.
    struct Suggested {
        key: (u64, usize),
        name: String,
        score: f32,
        rect: lcc_geom::Rect,
        view: lcc_geom::Rect,
    }
    let mut found: Vec<Suggested> = Vec::new();
    for ((photo, index), (name, score)) in suggestions {
        if !in_scope(scope, *photo) || !people.contains(&name.trim().to_lowercase()) {
            continue;
        }
        let id = lcc_catalog::PhotoId(*photo);
        let Some(ph) = s.catalog.photo(id).filter(|ph| ph.in_library()) else { continue };
        let Some(region) = ph.meta.regions.get(*index) else { continue };
        if region.kind != lcc_meta::RegionKind::Face || region.name.is_some() {
            continue;
        }
        found.push(Suggested { key: (*photo, *index), name: name.clone(), score: *score, rect: region.rect, view: s.face_view(id, region.rect) });
    }
    // what the user rejected as the person suggested is not put under them again (issue #1050)
    let rejected: std::collections::HashSet<(u64, usize)> = {
        let store = s.people_store();
        found.iter().filter(|f| store.rejected_as(f.key.0, f.key.1, f.rect, &f.name)).map(|f| f.key).collect()
    };
    // the name each suggested person was first shown as, by lower-cased name (`Ann` and `ann` are one person)
    let mut names: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut groups: std::collections::HashMap<String, Vec<Suggested>> = std::collections::HashMap::new();
    for face in found {
        if rejected.contains(&face.key) {
            continue;
        }
        let key = face.name.trim().to_lowercase();
        if key.is_empty() {
            continue;
        }
        names.entry(key.clone()).or_insert_with(|| face.name.trim().to_string());
        groups.entry(key).or_default().push(face);
    }
    let mut out: Vec<Value> = groups
        .into_iter()
        .filter_map(|(key, mut faces)| {
            let name = names.get(&key)?.clone();
            faces.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.key.0.cmp(&b.key.0)).then(a.key.1.cmp(&b.key.1)));
            let list: Vec<Value> = faces
                .iter()
                .map(|f| {
                    json!({
                        "photo": f.key.0, "index": f.key.1, "rect": rect_json(&f.rect), "view": rect_json(&f.view), "score": f.score
                    })
                })
                .collect();
            Some(json!({"name": name, "total": faces.len(), "faces": list}))
        })
        .collect();
    // the most suggested faces first, then by name, so the groups do not move between frames
    out.sort_by(|a, b| {
        let count = |v: &Value| v["total"].as_u64().unwrap_or(0);
        let name = |v: &Value| v["name"].as_str().unwrap_or("").to_lowercase();
        count(b).cmp(&count(a)).then_with(|| name(a).cmp(&name(b)))
    });
    Value::Array(out)
}

/// `faces.unnamed {limit?: 600, bySuggestion?: false, budgetMs?: 15}`: the faces nobody has named, to be named a
/// group at a time. With a recognition model chosen, faces that look alike are next to each other, and a face the
/// named faces recognise carries the name they suggest. With `bySuggestion: true`, `groups` holds those suggested
/// faces under the person suggested (issue #1050), so one person's suggestions are reviewed together.
///
/// The page is not where suggestions are found: every unnamed face with an embedding is judged against the named ones,
/// a slice of the work per call (`budgetMs`, so a big library does not stall a frame), and `scan` says how far that is
/// (issue #1365). `groups` therefore holds the library's suggestions within the scope, not only the page's.
fn unnamed(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "faces.unnamed";
    let limit = p.get("limit").and_then(Value::as_u64).map_or(600, |n| usize::try_from(n).unwrap_or(600)).clamp(1, MAX_UNNAMED);
    let by_suggestion = match p.get("bySuggestion") {
        None => false,
        Some(v) => v.as_bool().ok_or_else(|| bad(C, "`bySuggestion` must be true or false"))?,
    };
    // how long this call may spend judging faces; zero only asks how far the judging is
    let budget = std::time::Duration::from_millis(p.get("budgetMs").and_then(Value::as_u64).unwrap_or(15).min(250));
    let scope = scope_of(s, C, p)?;
    let mut all: Vec<(u64, usize, lcc_geom::Rect)> = Vec::new();
    for ph in s.catalog.photos().filter(|ph| ph.in_library() && in_scope(&scope, ph.id.0)) {
        for (index, r) in ph.meta.regions.iter().enumerate() {
            if r.kind == lcc_meta::RegionKind::Face && r.name.is_none() {
                all.push((ph.id.0, index, r.rect));
            }
        }
    }
    all.sort_by_key(|(photo, index, _)| (*photo, *index));
    let total = all.len();
    // the library is judged a slice at a time, this page's faces first, so that a suggestion never depends on which
    // page a face falls on (issue #1365)
    let people = current_people(s);
    let progress = imp::scan_suggestions(s, budget, &all);
    let arranged = imp::arrange_unnamed(s, &mut all, limit, &people);
    all.truncate(limit);
    let faces: Vec<Value> = all
        .iter()
        .map(|(photo, index, rect)| {
            let view = s.face_view(lcc_catalog::PhotoId(*photo), *rect);
            let suggestion = arranged.suggestions.get(&(*photo, *index)).map(|(name, score)| json!({"name": name, "score": score}));
            let similarity = arranged.similarities.get(&(*photo, *index)).map(|(name, score)| json!({"name": name, "score": score}));
            let alike = arranged.groups.get(&(*photo, *index));
            json!({"photo": photo, "index": index, "rect": rect_json(rect), "view": rect_json(&view), "suggestion": suggestion, "similarity": similarity, "group": alike})
        })
        .collect();
    // what the user said about a face's suggestion holds: a face rejected as the person it was suggested as is not put
    // under them again (issue #1050); the face itself stays in the list, unknown
    let (mut faces, mut rejected, mut suggestions) = (faces, 0usize, arranged.suggestions.clone());
    // (`faces` is one entry per listed face, in `all`'s order; the decisions are read in one borrow of the store, which
    // ends before the session is read again for the groups)
    let decided: Vec<bool> = {
        let store = s.people_store();
        all.iter()
            .map(|(photo, index, rect)| suggestions.get(&(*photo, *index)).is_some_and(|(name, _)| store.rejected_as(*photo, *index, *rect, name)))
            .collect()
    };
    for ((photo, index, _), (face, was_rejected)) in all.iter().zip(faces.iter_mut().zip(decided)) {
        if !was_rejected {
            continue;
        }
        suggestions.remove(&(*photo, *index));
        rejected += 1;
        if let Value::Object(map) = face {
            map.insert("suggestion".into(), Value::Null);
        }
    }
    let mut out =
        json!({"total": total, "faces": faces, "ordered": arranged.ordered, "ready": arranged.ready, "rejected": rejected, "scan": progress.json()});
    if by_suggestion && let Value::Object(map) = &mut out {
        // every suggestion the library holds within the scope, not only those of the faces this page lists (#1365)
        map.insert("groups".into(), imp::suggestion_groups(s, &scope, &people));
    }
    Ok(out)
}

/// The people the library knows: the lower-cased names of its named faces. A suggestion naming anyone else was made
/// before that person was renamed or removed, and is not shown (the judging has still to reach that face again).
fn current_people(s: &Session) -> std::collections::HashSet<String> {
    s.catalog
        .photos()
        .filter(|ph| ph.in_library())
        .flat_map(|ph| ph.meta.regions.iter())
        .filter(|r| r.kind == lcc_meta::RegionKind::Face)
        .filter_map(|r| r.name.as_deref())
        .map(|n| n.trim().to_lowercase())
        .filter(|n| !n.is_empty())
        .collect()
}

/// What looking for faces that resemble a person found.
struct Similar {
    /// `{photo, index, rect, score}` of unnamed faces, the most alike first.
    items: Vec<serde_json::Value>,
    /// A recognition model is chosen and its face index is available, so an empty list means "none yet" rather than "not set up".
    ready: bool,
    /// Photos the background scan has still to do.
    pending: usize,
}

fn rect_json(r: &lcc_geom::Rect) -> Value {
    json!({"x0": r.x0, "y0": r.y0, "x1": r.x1, "y1": r.y1})
}

/// Most confirmed faces `faces.person` lists.
const MAX_CONFIRMED: usize = 5000;

/// `faces.person {name, more?}`: the faces named `name` (case-insensitively) across the library, and, when a recognition
/// model is chosen, the unnamed faces that look most like them, to be confirmed one by one.
fn person(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "faces.person";
    let name = p.get("name").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(C, "missing `name`"))?;
    let key = name.to_lowercase();
    let limit = p.get("more").and_then(Value::as_u64).map_or(120, |n| usize::try_from(n).unwrap_or(120)).min(1000);
    let scope = scope_of(s, C, p)?;
    let (mut shown, mut total, mut confirmed) = (name.to_string(), 0usize, Vec::new());
    for ph in s.catalog.photos().filter(|ph| ph.in_library()) {
        for (index, r) in ph.meta.regions.iter().enumerate() {
            let Some(n) = r.name.as_deref().filter(|n| r.kind == lcc_meta::RegionKind::Face && n.trim().to_lowercase() == key) else {
                continue;
            };
            if total == 0 {
                shown = n.trim().to_string();
            }
            total += 1;
            // the count is the whole library's; the faces shown follow the scope
            if confirmed.len() < MAX_CONFIRMED && in_scope(&scope, ph.id.0) {
                confirmed.push(json!({"photo": ph.id.0, "index": index, "rect": rect_json(&r.rect), "view": rect_json(&s.face_view(ph.id, r.rect))}));
            }
        }
    }
    let similar = if total > 0 { imp::similar(s, &key, limit, &scope) } else { Similar { items: Vec::new(), ready: false, pending: 0 } };
    Ok(
        json!({"name": shown, "total": total, "confirmed": confirmed, "more": similar.items, "ready": similar.ready, "pendingPhotos": similar.pending}),
    )
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use lcc_catalog::{AlbumId, Op, Photo, PhotoId, Source};
    use lcc_develop::DevelopSettings;
    use lcc_faces::matching;
    use lcc_faces::runtime::Embedder;
    use lcc_geom::Rect;
    use lcc_meta::{Region, RegionKind};
    use rayon::prelude::*;
    use serde_json::{Value, json};

    use super::super::bad;
    use super::super::face_detect::{LABEL, MARK};
    use super::super::face_models::{finish_downloads, installed_models, read_settings, set_paused, switches, top_faces_value};
    use super::Similar;
    use crate::faces_index::{FaceScanItem, FaceScanMode, FaceScanRun, FaceScanStatus};
    use crate::faces_worker::{Done, EDGE, PREFETCH, Pace, Prepared, Worker, max_workers, process, target_workers};
    use crate::{Result, Session};

    /// Defaults for suggestions, until a model's own threshold is known.
    const DEFAULT_THRESHOLD: f32 = 0.45;
    const DEFAULT_MARGIN: f32 = 0.06;
    /// How long a look at the models folder is trusted by the per-frame `faces.pump`.
    const RECHECK: Duration = Duration::from_secs(1);
    /// How often the background pump writes new embeddings to the cache file (appending is cheap; this keeps it to a few
    /// writes a minute, and a session that ends writes the rest).
    const SAVE_EVERY: Duration = Duration::from_secs(3);

    /// Stop an active scoped scan before changing the library, model or index out from under its queued photos.
    fn abort_maintenance(s: &mut Session) {
        if let Some(mut run) = s.faces.maintenance.take() {
            run.queue.clear();
            run.in_flight.clear();
            run.cancelled = true;
            if let Some(cancel) = run.task.cancel_flag() {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            s.faces.last_maintenance = Some(run.status("cancelled"));
        }
    }

    /// The chosen recognition model's manifest, with the face index reset to it when it changed, without loading the
    /// ONNX network. `fresh` looks at the models folder again; otherwise a look made in the last second is trusted.
    fn current_manifest(s: &mut Session, cmd: &str, fresh: bool) -> Result<lcc_faces::ModelManifest> {
        if !fresh && s.faces.checked.is_some_and(|t| t.elapsed() < RECHECK) {
            return s
                .faces
                .manifest
                .as_ref()
                .map(|(_, m)| m.clone())
                .ok_or_else(|| bad(cmd, "no recognition model is chosen: add one in Settings > Faces and press Use"));
        }
        s.faces.checked = Some(Instant::now());
        let dir = s.face_models_dir.clone().ok_or_else(|| {
            s.faces.manifest = None;
            bad(cmd, "this build has nowhere to keep face models")
        })?;
        let id = read_settings(&dir).embedder.ok_or_else(|| {
            s.faces.manifest = None;
            bad(cmd, "no recognition model is chosen: add one in Settings > Faces and press Use")
        })?;
        let installed = installed_models(&dir).into_iter().find(|i| i.manifest.id == id).ok_or_else(|| {
            s.faces.manifest = None;
            bad(cmd, "the chosen recognition model is not installed")
        })?;
        let dim = match installed.manifest.output {
            lcc_faces::OutputSpec::Embedding { dim } => dim as usize,
            _ => {
                s.faces.manifest = None;
                return Err(bad(cmd, "the chosen model is not a recognition model"));
            }
        };
        let mirror = if read_settings(&dir).mirror_average() { crate::faces_index::MIRROR_TAG } else { "" };
        let tag = format!("{id}@{}{mirror}", installed.manifest.sha256.as_deref().unwrap_or("unknown"));
        // which photos the detector has searched is kept beside the library (or in memory when it is not on disk)
        let scan_path = s.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.join("face-scanned.bin"));
        if s.faces.scanned.path() != scan_path.as_deref() {
            s.faces.scanned.reset(scan_path);
        }
        let cache = s.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.join(format!("face-embeddings-{id}.bin")));
        if s.faces.index.tag != tag || s.faces.index.path != cache {
            if s.faces.maintenance.is_some() {
                abort_maintenance(s);
            }
            s.faces.index.reset(&tag, dim, cache);
            s.faces.queue.clear();
            s.faces.queue_stamp = None;
            s.faces.in_flight.clear();
        }
        s.faces.manifest = Some((tag, installed.manifest.clone()));
        Ok(installed.manifest)
    }

    /// The chosen recognition model, loaded, with the index reset to it when it changed. `fresh` looks at the models
    /// folder again; otherwise a look made in the last second is trusted (the UI asks every frame).
    fn current_with(s: &mut Session, cmd: &str, fresh: bool) -> Result<Arc<Embedder>> {
        let manifest = current_manifest(s, cmd, fresh)?;
        let tag = s.faces.index.tag.clone();
        if let Some((have, e)) = &s.faces.embedder
            && *have == tag
        {
            return Ok(e.clone());
        }
        let dir = s.face_models_dir.clone().ok_or_else(|| bad(cmd, "this build has nowhere to keep face models"))?;
        let embedder = Arc::new(Embedder::load(&dir.join(&manifest.id).join("model.onnx"), &manifest).map_err(|e| bad(cmd, e.to_string()))?);
        let net = embedder.clone();
        super::super::face_detect::offer_gpu(s, "recognition", move |gpu| net.accelerate(gpu, true).map_err(|e| e.to_string()));
        s.faces.embedder = Some((tag, embedder.clone()));
        Ok(embedder)
    }

    fn current(s: &mut Session, cmd: &str) -> Result<Arc<Embedder>> {
        current_with(s, cmd, true)
    }

    /// Whether the background scan should look for faces in this photo: it is in the library, has no face regions at all
    /// (nothing from XMP, nothing drawn or named, no earlier detection) and has not been searched yet.
    fn needs_search(s: &Session, photo: &Photo) -> bool {
        photo.in_library()
            && !photo.meta.regions.iter().any(|r| matches!(r.kind, RegionKind::Face | RegionKind::IgnoredFace))
            && !s.faces.scanned.contains(photo.id.0)
            && !s.faces.scan_failed.contains(&photo.id.0)
    }

    /// The photo's faces that still need embedding (and, with `search`, the faces to find in it first), with the render job
    /// for its picture. `None` when there is nothing to do.
    fn prepare(
        s: &mut Session,
        id: PhotoId,
        search: bool,
        redetect: bool,
        replace_detections: bool,
        min_score: f32,
        named_only: bool,
    ) -> Option<Prepared> {
        let tag = s.faces.index.tag.clone();
        let photo = s.catalog.photo(id)?.clone();
        let todo: Vec<Rect> = photo
            .meta
            .regions
            .iter()
            .filter(|r| {
                r.kind == RegionKind::Face
                    && (!named_only || r.name.as_deref().is_some_and(|name| !name.trim().is_empty()))
                    && s.faces.index.needs(id.0, &r.rect)
            })
            .map(|r| r.rect)
            .collect();
        // Finding faces needs the detector the user may not have downloaded: without it a photo is left unsearched.
        let detector = (search || redetect).then(|| crate::cmd::face_detect::detector(s).ok()).flatten();
        let detect = detector.is_some() && (redetect || (search && needs_search(s, &photo)));
        if todo.is_empty() && !detect {
            return None;
        }
        let job = s.preview_job(id, EDGE, EDGE, false, &DevelopSettings::default())?;
        let preview = s.scan_preview_of(&photo);
        Some(Prepared {
            tag,
            epoch: s.faces.epoch,
            id,
            content: crate::media::content_key(&photo),
            job,
            preview,
            todo,
            detect,
            redetect,
            replace_detections,
            min_score,
            detector,
        })
    }

    /// Faces found in a photo with no boxes become unnamed regions. A requested rescan may remove only earlier automatic
    /// detections, or keep and merge them; named, ignored and manually drawn regions are never replaced.
    fn add_found(s: &mut Session, id: PhotoId, found: &[Rect], replace_detections: bool) {
        let Some(photo) = s.catalog.photo(id) else { return };
        let old_regions = photo.meta.regions.clone();
        let mut meta = photo.meta.clone();
        if replace_detections {
            for region in meta.regions.iter().filter(|r| crate::cmd::face_detect::is_detected(r)) {
                s.faces.index.forget_region(id.0, &region.rect);
            }
            meta.regions.retain(|r| !crate::cmd::face_detect::is_detected(r));
        }
        for rect in found {
            let face = lcc_faces::yunet::Face {
                x0: rect.x0 as f32,
                y0: rect.y0 as f32,
                x1: rect.x1 as f32,
                y1: rect.y1 as f32,
                score: 1.0,
                landmarks: [(0.0, 0.0); 5],
            };
            if crate::cmd::face_detect::overlaps_existing(&face, &meta.regions) {
                continue;
            }
            meta.regions.push(Region { rect: *rect, kind: RegionKind::Face, name: None, description: Some(format!("{MARK}{LABEL}")) });
        }
        if meta.regions != old_regions && s.apply_system(Op::SetMeta { id, meta: Box::new(meta) }).is_ok() {
            s.skip_auto_write = true;
        }
    }

    /// Take a finished photo into the index (ignored when it was made for another model or another library, or from
    /// a file the photo no longer has: issue #1524). Returns (embedded, failed).
    fn ingest(s: &mut Session, d: Done) -> (usize, usize) {
        s.faces.in_flight.remove(&d.id);
        if d.epoch != s.faces.epoch || d.tag != s.faces.index.tag {
            return (0, 0);
        }
        if s.catalog.photo(d.id).is_none_or(|p| crate::media::content_key(p) != d.content) {
            return (0, 0);
        }
        if d.detect_wanted {
            // A user or another import may have added regions while this background scan was in flight.
            // Never apply its stale detections to a photo that no longer qualifies for the initial search.
            let still_needs_search = s.catalog.photo(d.id).is_some_and(|photo| needs_search(s, photo));
            match &d.found {
                Some(found) => {
                    s.faces.scanned.insert(d.id.0);
                    if d.redetect || still_needs_search {
                        add_found(s, d.id, found, d.redetect && d.replace_detections);
                    }
                }
                None => {
                    s.faces.scan_failed.insert(d.id.0);
                }
            }
        }
        let current: std::collections::HashSet<[i32; 4]> = s
            .catalog
            .photo(d.id)
            .into_iter()
            .flat_map(|photo| photo.meta.regions.iter().filter(|r| r.kind == RegionKind::Face).map(|r| crate::faces_index::region_key(&r.rect)))
            .collect();
        let (mut embedded, mut failed) = (0, 0);
        for (rect, v, view) in d.results {
            // A rescan can replace automatic boxes while a worker is embedding them. Do not retain an embedding whose
            // region was removed or changed by the new detector result.
            if !current.contains(&crate::faces_index::region_key(&rect)) {
                continue;
            }
            match v {
                Some(v) if v.len() == s.faces.index.dim => {
                    s.faces.index.insert_with_view(d.id.0, &rect, v, &view);
                    embedded += 1;
                }
                _ => {
                    s.faces.index.skip(d.id.0, &rect);
                    failed += 1;
                }
            }
        }
        (embedded, failed)
    }

    /// Take in whatever the worker has finished, advancing a scoped maintenance task when its photos return.
    fn collect(s: &mut Session) -> (usize, usize) {
        let finished = s.faces.worker.as_ref().map(Worker::finished).unwrap_or_default();
        let (mut embedded, mut failed) = (0usize, 0usize);
        for d in finished {
            let id = d.id;
            let photo_failed = (d.detect_wanted && d.found.is_none()) || d.results.iter().any(|(_, e, _)| e.is_none());
            let follow_detection = d.detect_wanted && d.found.is_some();
            let current_job = d.epoch == s.faces.epoch && d.tag == s.faces.index.tag;
            let maintenance = s.faces.maintenance.as_mut().is_some_and(|run| run.in_flight.remove(&id));
            let (done_embeddings, failed_embeddings) = ingest(s, d);
            embedded += done_embeddings;
            failed += failed_embeddings;
            if !maintenance {
                continue;
            }

            let cancelled = s.faces.maintenance.as_ref().is_some_and(|run| run.task.is_cancelled());
            let named_only = s.faces.maintenance.as_ref().is_some_and(|run| run.named_only);
            let needs_follow_up = follow_detection
                && current_job
                && !cancelled
                && s.catalog.photo(id).is_some_and(|photo| {
                    photo.meta.regions.iter().any(|r| {
                        r.kind == RegionKind::Face
                            && (!named_only || r.name.as_deref().is_some_and(|name| !name.trim().is_empty()))
                            && s.faces.index.needs(id.0, &r.rect)
                    })
                });
            if let Some(run) = s.faces.maintenance.as_mut() {
                if run.task.is_cancelled() {
                    run.cancelled = true;
                    run.queue.clear();
                }
                if needs_follow_up {
                    run.queue.push_back(crate::faces_index::FaceScanItem { photo: id, search: false, redetect: false, replace_detections: false });
                    continue;
                }
                run.done = run.done.saturating_add(1).min(run.total);
                if photo_failed || failed_embeddings > 0 || !current_job {
                    run.failed = run.failed.saturating_add(1);
                    run.task.item_failed(&format!("photo {}: face data could not be read", id.0));
                } else {
                    run.task.item_done();
                }
            }
        }
        (embedded, failed)
    }

    /// Embed every face region of `id` the index does not have yet, here and now. Returns (embedded, failed).
    fn embed_photo(s: &mut Session, id: PhotoId, e: &Embedder) -> Result<(usize, usize)> {
        let Some(job) = prepare(s, id, false, false, false, crate::faces_worker::REGION_SCORE, false) else { return Ok((0, 0)) };
        Ok(ingest(s, process(job, e)))
    }

    /// Photos with a face that needs embedding (and, with `search`, photos to look for faces in): those the caller asked
    /// for first, then those with named faces (they are the gallery everything else is compared with), then the rest, and
    /// last the photos that only need searching.
    fn pending(s: &Session, first: &[PhotoId], search: bool) -> Vec<PhotoId> {
        let mut out: Vec<(u8, PhotoId)> = Vec::new();
        for p in s.catalog.photos().filter(|p| p.in_library()) {
            let faces: Vec<_> = p.meta.regions.iter().filter(|r| r.kind == RegionKind::Face).collect();
            let embed = faces.iter().any(|r| s.faces.index.needs(p.id.0, &r.rect));
            if embed || (search && needs_search(s, p)) {
                let rank = if first.contains(&p.id) {
                    0
                } else if faces.iter().any(|r| r.name.is_some()) {
                    1
                } else if embed {
                    2
                } else {
                    3
                };
                out.push((rank, p.id));
            }
        }
        out.sort_by_key(|(rank, id)| (*rank, id.0));
        out.into_iter().map(|(_, id)| id).collect()
    }

    fn parse_scan_mode(p: &Value) -> Result<FaceScanMode> {
        const C: &str = "faces.maintain";
        let raw = match p.get("mode") {
            None => "detect",
            Some(value) => value.as_str().ok_or_else(|| bad(C, "`mode` must be text"))?,
        }
        .trim();
        match raw.to_ascii_lowercase().as_str() {
            "detect" | "detection" => Ok(FaceScanMode::Detect),
            "recognize" | "recognition" => Ok(FaceScanMode::Recognize),
            "new" | "newitems" | "new-only" => Ok(FaceScanMode::New),
            "rebuild" | "all" => Ok(FaceScanMode::Rebuild),
            "rebuildnamed" | "named" | "confirmed" => Ok(FaceScanMode::RebuildNamed),
            "rescan" | "redetect" | "re-detect" => Ok(FaceScanMode::Rescan),
            _ => Err(bad(C, "`mode` is `detect`, `recognize`, `new`, `rebuild`, `rebuildNamed`, or `rescan`")),
        }
    }

    fn selector_texts(p: &Value, singular: &str, plural: &str, cmd: &str) -> Result<Vec<String>> {
        let mut out = Vec::new();
        if let Some(value) = p.get(singular) {
            let text = value.as_str().ok_or_else(|| bad(cmd, format!("`{singular}` must be text")))?.trim();
            if text.is_empty() || text.chars().any(char::is_control) || text.len() > 4096 {
                return Err(bad(cmd, format!("`{singular}` must be a non-empty path of at most 4096 bytes")));
            }
            out.push(text.to_string());
        }
        if let Some(value) = p.get(plural) {
            let values = value.as_array().ok_or_else(|| bad(cmd, format!("`{plural}` must be a list of paths")))?;
            if values.len() > 64 {
                return Err(bad(cmd, format!("at most 64 `{plural}` may be selected")));
            }
            for value in values {
                let text = value.as_str().ok_or_else(|| bad(cmd, format!("every `{plural}` entry must be text")))?.trim();
                if text.is_empty() || text.chars().any(char::is_control) || text.len() > 4096 {
                    return Err(bad(cmd, format!("every `{plural}` entry must be a non-empty path of at most 4096 bytes")));
                }
                out.push(text.to_string());
            }
        }
        out.sort();
        out.dedup();
        Ok(out)
    }

    fn selected_album_photos(
        s: &Session,
        id: AlbumId,
        seen_albums: &mut std::collections::HashSet<AlbumId>,
        photos: &mut std::collections::HashSet<PhotoId>,
    ) -> Result<()> {
        const C: &str = "faces.maintain";
        if !seen_albums.insert(id) {
            return Ok(());
        }
        let album = s.catalog.album(id).ok_or_else(|| bad(C, format!("album {} no longer exists", id.0)))?;
        if album.folder {
            // A selected collection set always means its complete nested collection tree. The
            // `includeSubfolders` option applies only to filesystem folder selectors.
            for child in s.catalog.album_children(Some(id)) {
                selected_album_photos(s, child.id, seen_albums, photos)?;
            }
        } else {
            photos.extend(s.catalog.album_photos(id));
        }
        Ok(())
    }

    /// Resolve explicit ids, visible/selected photos, folder paths and regular, smart or folder albums into one
    /// deduplicated in-library photo list. Selectors are unioned; absent any selector the scope is the whole catalog.
    fn maintenance_scope(s: &mut Session, p: &Value) -> Result<(Vec<PhotoId>, String)> {
        const C: &str = "faces.maintain";
        let scope = match p.get("scope") {
            None => "all",
            Some(value) => value.as_str().ok_or_else(|| bad(C, "`scope` must be text"))?,
        }
        .trim();
        if !matches!(scope, "all" | "visible" | "selected") {
            return Err(bad(C, "`scope` is `all`, `visible`, or `selected`"));
        }
        let include_subfolders = match p.get("includeSubfolders") {
            None => true,
            Some(value) => value.as_bool().ok_or_else(|| bad(C, "`includeSubfolders` must be true or false"))?,
        };
        let folders = selector_texts(p, "folder", "folders", C)?;
        let mut albums = Vec::new();
        if let Some(value) = p.get("album") {
            let id = value.as_u64().ok_or_else(|| bad(C, "`album` must be a non-negative album id"))?;
            albums.push(AlbumId(id));
        }
        if let Some(value) = p.get("albums") {
            let values = value.as_array().ok_or_else(|| bad(C, "`albums` must be a list of album ids"))?;
            if values.len() > 256 {
                return Err(bad(C, "at most 256 albums may be selected"));
            }
            for value in values {
                albums.push(AlbumId(value.as_u64().ok_or_else(|| bad(C, "every `albums` entry must be a non-negative id"))?));
            }
        }
        albums.sort_by_key(|a| a.0);
        albums.dedup();

        let explicit_ids = super::super::photo_ids(p, C)?;
        let has_ids_selector = explicit_ids.is_some();
        let has_explicit = has_ids_selector || !folders.is_empty() || !albums.is_empty();
        let mut photos = std::collections::HashSet::new();
        if let Some(ids) = explicit_ids {
            for id in ids {
                if !s.catalog.photo(id).is_some_and(|photo| photo.in_library()) {
                    return Err(bad(C, format!("photo {} is no longer in the library", id.0)));
                }
                photos.insert(id);
            }
        }
        for root in &folders {
            for photo in s.catalog.photos().filter(|photo| photo.in_library()) {
                let Source::File { path } = &photo.source else { continue };
                let within = if include_subfolders {
                    lcc_catalog::query::folder_within(path, root)
                } else {
                    lcc_catalog::folder_of(photo)
                        .is_some_and(|folder| lcc_catalog::query::folder_key(&folder) == lcc_catalog::query::folder_key(root))
                };
                if within {
                    photos.insert(photo.id);
                }
            }
        }
        let mut seen_albums = std::collections::HashSet::new();
        for album in &albums {
            selected_album_photos(s, *album, &mut seen_albums, &mut photos)?;
        }
        match scope {
            "visible" => {
                let visible = s.visible_cloned();
                photos.extend(visible.into_iter().filter(|id| s.catalog.photo(*id).is_some_and(|photo| photo.in_library())));
            }
            "selected" => {
                let selected = s.selected();
                photos.extend(selected.into_iter().filter(|id| s.catalog.photo(*id).is_some_and(|photo| photo.in_library())));
            }
            "all" if !has_explicit => photos.extend(s.catalog.photos().filter(|photo| photo.in_library()).map(|photo| photo.id)),
            _ => {}
        }
        let mut ids: Vec<PhotoId> = photos.into_iter().collect();
        ids.sort_by_key(|id| id.0);
        let description = if has_explicit {
            let mut parts = Vec::new();
            if has_ids_selector {
                parts.push("photo ids".to_string());
            }
            if !folders.is_empty() {
                parts.push(format!("{} folder(s)", folders.len()));
            }
            if !albums.is_empty() {
                parts.push(format!("{} album(s)", albums.len()));
            }
            if scope != "all" {
                parts.push(scope.to_string());
            }
            parts.join(" + ")
        } else {
            scope.to_string()
        };
        Ok((ids, description))
    }

    fn mode_needs_photo(s: &Session, mode: FaceScanMode, id: PhotoId) -> bool {
        let Some(photo) = s.catalog.photo(id).filter(|photo| photo.in_library()) else { return false };
        let has_face = photo.meta.regions.iter().any(|region| region.kind == RegionKind::Face);
        let has_named_face = photo
            .meta
            .regions
            .iter()
            .any(|region| region.kind == RegionKind::Face && region.name.as_deref().is_some_and(|name| !name.trim().is_empty()));
        let needs_embedding = photo.meta.regions.iter().any(|region| region.kind == RegionKind::Face && s.faces.index.needs(id.0, &region.rect));
        match mode {
            FaceScanMode::Detect => needs_embedding || needs_search(s, photo),
            FaceScanMode::Recognize => needs_embedding,
            FaceScanMode::New => !s.faces.scanned.contains(id.0) && (needs_embedding || needs_search(s, photo)),
            FaceScanMode::Rebuild => has_face,
            FaceScanMode::RebuildNamed => has_named_face,
            FaceScanMode::Rescan => true,
        }
    }

    /// Start a scoped, cancellable maintenance pass. It uses the existing recognition worker and detector; the ordinary
    /// library-wide indexer remains responsible for continuous background discovery and the existing suggestion judge.
    pub fn maintain(s: &mut Session, p: &Value) -> Result<Value> {
        const C: &str = "faces.maintain";
        if s.faces.maintenance.is_some() {
            return Err(bad(C, "a face maintenance scan is already running"));
        }
        let mode = parse_scan_mode(p)?;
        current_with(s, C, true)?;
        if mode.detects() {
            super::super::face_detect::detector(s).map_err(|e| e.into_error(C))?;
        }
        let (scope_ids, scope) = maintenance_scope(s, p)?;
        if mode.detects() {
            // A deliberate user run retries photos the continuous indexer could not read earlier in this session.
            for id in &scope_ids {
                s.faces.scan_failed.remove(&id.0);
            }
        }
        let min_score = match p.get("minScore") {
            None => crate::faces_worker::REGION_SCORE,
            Some(value) => {
                let score = value.as_f64().ok_or_else(|| bad(C, "`minScore` must be a number from 0 to 1"))?;
                if !score.is_finite() || !(0.0..=1.0).contains(&score) {
                    return Err(bad(C, "`minScore` must be a number from 0 to 1"));
                }
                score as f32
            }
        };
        let behavior = match p.get("behavior") {
            None => "merge",
            Some(value) => value.as_str().ok_or_else(|| bad(C, "`behavior` must be `merge` or `replace`"))?,
        }
        .trim()
        .to_ascii_lowercase();
        let replace_detections = match p.get("replaceDetections") {
            Some(value) => value.as_bool().ok_or_else(|| bad(C, "`replaceDetections` must be true or false"))?,
            None => match behavior.as_str() {
                "merge" => false,
                "replace" | "overwrite" => true,
                _ => return Err(bad(C, "`behavior` is `merge` or `replace`")),
            },
        };
        if mode != FaceScanMode::Rescan && (replace_detections || p.get("behavior").is_some()) {
            return Err(bad(C, "`behavior` and `replaceDetections` apply only to `mode: rescan`"));
        }

        let mut queue = std::collections::VecDeque::new();
        for id in &scope_ids {
            if !mode_needs_photo(s, mode, *id) {
                continue;
            }
            if mode == FaceScanMode::Rebuild {
                s.faces.index.forget_photo(id.0);
            } else if mode == FaceScanMode::RebuildNamed
                && let Some(photo) = s.catalog.photo(*id)
            {
                for region in photo
                    .meta
                    .regions
                    .iter()
                    .filter(|region| region.kind == RegionKind::Face && region.name.as_deref().is_some_and(|name| !name.trim().is_empty()))
                {
                    s.faces.index.forget_region(id.0, &region.rect);
                }
            }
            queue.push_back(FaceScanItem {
                photo: *id,
                search: matches!(mode, FaceScanMode::Detect | FaceScanMode::New),
                redetect: mode.redetects(),
                replace_detections,
            });
        }
        let total = queue.len() as u64;
        let task = s.activity.start_batch("faces", "Face Maintenance Scan", crate::activity::Cancel::Yes);
        task.progress(0, total);
        task.set_controls(Some(crate::activity::Controls {
            paused: switches_cached(s).1,
            pause: "faces.pause",
            resume: "faces.resume",
            settings_tab: "faces",
            settings_section: None,
        }));
        s.faces.last_maintenance = None;
        s.faces.maintenance = Some(FaceScanRun {
            mode,
            scope,
            queue,
            min_score,
            named_only: mode.named_only(),
            in_flight: std::collections::HashSet::new(),
            total,
            done: 0,
            failed: 0,
            cancelled: false,
            task,
        });
        finish_maintenance_if_done(s);
        maintenance_status(s, p)
    }

    fn finish_maintenance_if_done(s: &mut Session) {
        if let Some(run) = s.faces.maintenance.as_mut()
            && run.task.is_cancelled()
        {
            run.cancelled = true;
            run.queue.clear();
        }
        let done = s.faces.maintenance.as_ref().is_some_and(|run| run.queue.is_empty() && run.in_flight.is_empty());
        if done && let Some(run) = s.faces.maintenance.take() {
            let state = if run.cancelled || run.task.is_cancelled() { "cancelled" } else { "complete" };
            s.faces.last_maintenance = Some(run.status(state));
        }
    }

    /// The scoped run's status, separate from the always-on whole-library indexer's progress.
    pub fn maintenance_status(s: &mut Session, _: &Value) -> Result<Value> {
        if let Some(run) = s.faces.maintenance.as_ref() {
            let state = if run.cancelled || run.task.is_cancelled() { "cancelling" } else { "scanning" };
            return Ok(run.status(state).value());
        }
        Ok(s.faces.last_maintenance.as_ref().map(FaceScanStatus::value).unwrap_or_else(|| {
            json!({"state": "idle", "mode": null, "scope": null, "done": 0, "total": 0, "pendingPhotos": 0, "failed": 0, "cancelled": false, "taskId": null})
        }))
    }

    fn scan_item_needs_work(s: &Session, item: FaceScanItem, named_only: bool) -> bool {
        let Some(photo) = s.catalog.photo(item.photo).filter(|photo| photo.in_library()) else { return false };
        let embed = photo.meta.regions.iter().any(|region| {
            region.kind == RegionKind::Face
                && (!named_only || region.name.as_deref().is_some_and(|name| !name.trim().is_empty()))
                && s.faces.index.needs(item.photo.0, &region.rect)
        });
        item.redetect || embed || (item.search && needs_search(s, photo))
    }

    fn finish_scan_photo(run: &mut FaceScanRun, id: PhotoId, failed: bool, reason: &str) {
        run.done = run.done.saturating_add(1).min(run.total);
        if failed {
            run.failed = run.failed.saturating_add(1);
            run.task.item_failed(&format!("photo {}: {reason}", id.0));
        } else {
            run.task.item_done();
        }
    }

    /// Hand the scoped queue to the shared face worker, alongside any ordinary indexing jobs already in flight.
    fn queue_maintenance(s: &mut Session, embedder: &Arc<Embedder>, pace: Pace, target: usize) {
        if s.faces.maintenance.as_ref().is_some_and(|run| run.task.is_cancelled())
            && let Some(run) = s.faces.maintenance.as_mut()
        {
            run.cancelled = true;
            run.queue.clear();
        }
        let cancelling = s.faces.maintenance.as_ref().is_some_and(|run| run.cancelled);
        if cancelling {
            // Jobs already handed to the shared worker are bounded by the batch's in-flight set. Let them drain even if
            // the continuous indexer was paused; the cancelled batch has no queued work left to start.
            if let Some(worker) = &s.faces.worker {
                worker.allow(max_workers(), Pace::Light);
            }
            return;
        }
        if target == 0 || s.faces.maintenance.is_none() {
            if let Some(worker) = &s.faces.worker {
                worker.allow(target, pace);
            }
            return;
        }
        if s.faces.worker.is_none() && s.faces.maintenance.as_ref().is_some_and(|run| !run.queue.is_empty()) {
            s.faces.worker = Worker::start(max_workers());
        }
        if let Some(worker) = &s.faces.worker {
            worker.allow(target, pace);
        }
        let queued_at_start = s.faces.maintenance.as_ref().map_or(0, |run| run.queue.len());
        let limit = target;
        let mut looked = 0;
        while s.faces.in_flight.len() < limit && looked < queued_at_start {
            let Some(item) = s.faces.maintenance.as_mut().and_then(|run| run.queue.pop_front()) else { break };
            looked += 1;
            if s.faces.in_flight.contains(&item.photo) {
                if let Some(run) = s.faces.maintenance.as_mut() {
                    run.queue.push_back(item);
                }
                continue;
            }
            let (min_score, named_only) =
                s.faces.maintenance.as_ref().map_or((crate::faces_worker::REGION_SCORE, false), |run| (run.min_score, run.named_only));
            let needs_work = scan_item_needs_work(s, item, named_only);
            let Some(job) = prepare(s, item.photo, item.search, item.redetect, item.replace_detections, min_score, named_only) else {
                if let Some(run) = s.faces.maintenance.as_mut() {
                    finish_scan_photo(run, item.photo, needs_work, "photo could not be prepared for scanning");
                }
                continue;
            };
            let detail = s.catalog.photo(item.photo).map(|photo| photo.file_name.clone()).unwrap_or_else(|| item.photo.0.to_string());
            if let Some(run) = s.faces.maintenance.as_ref() {
                run.task.detail(&detail);
            }
            if s.faces.worker.as_ref().is_some_and(|worker| worker.submit(embedder.clone(), job)) {
                s.faces.in_flight.insert(item.photo);
                if let Some(run) = s.faces.maintenance.as_mut() {
                    run.in_flight.insert(item.photo);
                }
            } else {
                if let Some(run) = s.faces.maintenance.as_mut() {
                    run.queue.push_front(item);
                }
                break;
            }
        }
    }

    pub fn index(s: &mut Session, p: &Value) -> Result<Value> {
        const C: &str = "faces.index";
        let embedder = current(s, C)?;
        collect(s);
        let budget = Duration::from_millis(p.get("budgetMs").and_then(Value::as_u64).unwrap_or(200).min(600_000));
        let first: Vec<PhotoId> = super::super::photo_ids(p, C)?.unwrap_or_default();
        let todo = pending(s, &first, false);
        let started = Instant::now();
        let (mut embedded, mut failed, mut done) = (0, 0, 0);
        for id in &todo {
            // at least one photo per call, so a slow model still makes progress; none when only the status is asked for
            if budget.is_zero() || (done > 0 && started.elapsed() >= budget) {
                break;
            }
            let (e, f) = embed_photo(s, *id, &embedder)?;
            (embedded, failed, done) = (embedded + e, failed + f, done + 1);
        }
        if let Err(e) = s.faces.index.save() {
            log::warn!("could not save the face embeddings: {e}");
        }
        Ok(json!({
            "model": s.faces.index.tag,
            "embedded": embedded,
            "failed": failed,
            "photosDone": done,
            "pendingPhotos": todo.len().saturating_sub(done),
            "indexedFaces": s.faces.index.len(),
            "ms": started.elapsed().as_secs_f64() * 1000.0,
        }))
    }

    fn str_scope(p: &Value) -> Option<&str> {
        p.get("scope").and_then(Value::as_str)
    }

    /// Seconds since 1970 of an ISO capture time (`2024-09-20T10:48:45…`, any zone suffix ignored): only for telling
    /// shots taken close together, so the zone does not matter.
    fn capture_secs(iso: &str) -> Option<i64> {
        let n = |a: usize, b: usize| iso.get(a..b)?.parse::<i64>().ok();
        let (y, m, d, hh, mm, ss) = (n(0, 4)?, n(5, 7)?, n(8, 10)?, n(11, 13)?, n(14, 16)?, n(17, 19)?);
        if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
            return None;
        }
        // Days since 1970-01-01 (proleptic Gregorian); a day past the month's length rolls over as
        // the old Hinnant arithmetic did.
        let first = chrono::NaiveDate::from_ymd_opt(i32::try_from(y).ok()?, u32::try_from(m).ok()?, 1)?;
        let date = first.checked_add_days(chrono::Days::new(u64::try_from(d - 1).ok()?))?;
        let days = (date - chrono::NaiveDate::from_ymd_opt(1970, 1, 1)?).num_days();
        Some(days * 86_400 + hh * 3600 + mm * 60 + ss)
    }

    /// `faces.evaluate`: leave-one-out over the library's named faces, to see how well recognition works on *your*
    /// photos and which threshold to trust. Each named face in turn is treated as unknown and ranked against the
    /// other named faces, ignoring those from shots taken within `burstSecs` of it (near-duplicates would make
    /// everything look easy). Reports counts only.
    pub fn evaluate(s: &mut Session, p: &Value) -> Result<Value> {
        const C: &str = "faces.evaluate";
        let embedder = current(s, C)?;
        collect(s);
        let burst = p.get("burstSecs").and_then(Value::as_i64).unwrap_or(5).clamp(0, 86_400 * 7);
        let margin = p.get("margin").and_then(Value::as_f64).filter(|v| v.is_finite()).map_or(DEFAULT_MARGIN, |v| v as f32);
        let thresholds: Vec<f32> = p
            .get("thresholds")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_f64).filter(|v| v.is_finite()).map(|v| v as f32).take(64).collect())
            .filter(|v: &Vec<f32>| !v.is_empty())
            .unwrap_or_else(|| (4..=16).map(|i| i as f32 * 0.05).collect());
        let budget = Duration::from_millis(p.get("budgetMs").and_then(Value::as_u64).unwrap_or(0).min(3_600_000));
        let top = top_faces_param(s, C, p)?;
        let started = Instant::now();
        // `scope: "visible"` looks only at the photos the current filter shows (say one year, or one album)
        let scope: Option<std::collections::HashSet<PhotoId>> = (str_scope(p) == Some("visible")).then(|| s.visible_cloned().into_iter().collect());
        let in_scope = |id: PhotoId| scope.as_ref().is_none_or(|v| v.contains(&id));
        let todo: Vec<PhotoId> = pending(s, &[], false).into_iter().filter(|id| in_scope(*id)).collect();
        let mut done = 0;
        for id in &todo {
            if started.elapsed() >= budget {
                break;
            }
            embed_photo(s, *id, &embedder)?;
            done += 1;
        }
        // every named face with its embedding, the photo it is in, and when that was taken
        struct Named {
            name: String,
            emb: Arc<[f32]>,
            photo: PhotoId,
            when: Option<i64>,
        }
        let named: Vec<Named> = s
            .catalog
            .photos()
            .filter(|ph| ph.in_library() && in_scope(ph.id))
            .flat_map(|ph| {
                let when = ph.captured.as_deref().and_then(capture_secs);
                ph.meta
                    .regions
                    .iter()
                    .filter(|r| r.kind == RegionKind::Face)
                    .filter_map(|r| Some(Named { name: r.name.clone()?, emb: s.faces.index.get(ph.id.0, &r.rect)?.clone(), photo: ph.id, when }))
                    .collect::<Vec<_>>()
            })
            .collect();
        let near = |a: &Named, b: &Named| a.photo == b.photo || matches!((a.when, b.when), (Some(x), Some(y)) if (x - y).abs() <= burst);
        let mut sweep: Vec<(f32, usize, usize)> = thresholds.iter().map(|t| (*t, 0, 0)).collect();
        let (mut queries, mut top1, mut unmatchable) = (0usize, 0usize, 0usize);
        for q in &named {
            let others: Vec<&Named> = named.iter().filter(|g| !near(q, g)).collect();
            if !others.iter().any(|g| g.name.eq_ignore_ascii_case(&q.name)) {
                unmatchable += 1;
                continue;
            }
            let refs: Vec<(&str, &[f32])> = others.iter().map(|g| (g.name.as_str(), &g.emb[..])).collect();
            let ranked = matching::rank_top(&q.emb, &refs, top);
            queries += 1;
            if ranked.first().is_some_and(|r| r.name.eq_ignore_ascii_case(&q.name)) {
                top1 += 1;
            }
            for (t, suggested, correct) in sweep.iter_mut() {
                if let Some(sg) = matching::suggest(&ranked, *t, margin) {
                    *suggested += 1;
                    if sg.name.eq_ignore_ascii_case(&q.name) {
                        *correct += 1;
                    }
                }
            }
        }
        let pct = |n: usize, d: usize| if d == 0 { Value::Null } else { json!((n as f64 / d as f64 * 1000.0).round() / 1000.0) };
        Ok(json!({
            "model": s.faces.index.tag,
            "namedFaces": named.len(),
            "queries": queries,
            "unmatchable": unmatchable,
            "pendingPhotos": todo.len().saturating_sub(done),
            "top1": pct(top1, queries),
            "margin": margin,
            "topFaces": top,
            "burstSecs": burst,
            "sweep": sweep.iter().map(|(t, sg, ok)| json!({"threshold": t, "suggested": sg, "correct": ok, "precision": pct(*ok, *sg), "recall": pct(*ok, queries)})).collect::<Vec<_>>(),
        }))
    }

    /// The scan's row in the activity stack: there while `pending` photos are left (also while paused), its whole the
    /// most that were left at once since the scan last finished (which it returns; 0 when none are left). It has no ✕:
    /// its pause button pauses it (resume while `paused`), and its gear opens Settings ▸ Faces, where it is switched off.
    fn track(s: &mut Session, pending: u64, paused: bool) -> u64 {
        if pending == 0 {
            s.faces.task = None;
            s.faces.peak = 0;
            return 0;
        }
        s.faces.peak = s.faces.peak.max(pending);
        let task = s.faces.task.get_or_insert_with(|| s.activity.start("faces", "Finding faces", crate::activity::Cancel::No));
        task.progress(s.faces.peak.saturating_sub(pending), s.faces.peak);
        task.detail(if paused { "Paused" } else { "" });
        task.set_controls(Some(crate::activity::Controls {
            paused,
            pause: "faces.pause",
            resume: "faces.resume",
            settings_tab: "faces",
            settings_section: None,
        }));
        s.faces.peak
    }

    /// Whether recognition is on and the scan paused, looked up at most once a second.
    fn switches_cached(s: &mut Session) -> (bool, bool) {
        let Some(dir) = s.face_models_dir.clone() else { return (false, false) };
        if let Some((t, on, paused)) = s.faces.switches_seen
            && t.elapsed() < RECHECK
        {
            return (on, paused);
        }
        let (on, paused) = switches(&dir);
        s.faces.switches_seen = Some((Instant::now(), on, paused));
        (on, paused)
    }

    /// Where the scan stands: `off` (recognition switched off), `paused`, `indexing` (photos left), `complete` (none
    /// left) or `idle` (on, but not started yet: no model chosen, or not looked at yet).
    fn state(s: &mut Session) -> (&'static str, bool, bool) {
        let (on, paused) = switches_cached(s);
        let pending = s.faces.queue.len() + s.faces.in_flight.len();
        let state = if !on {
            "off"
        } else if paused {
            "paused"
        } else if pending > 0 {
            "indexing"
        } else if s.faces.queue_stamp.is_some() {
            "complete"
        } else {
            "idle"
        };
        (state, on, paused)
    }

    /// `faces.status`: how far the background face scan is.
    pub fn status(s: &mut Session, _: &Value) -> Result<Value> {
        let (state, enabled, paused) = state(s);
        let pending = (s.faces.queue.len() + s.faces.in_flight.len()) as u64;
        let total = s.faces.peak.max(pending);
        Ok(json!({
            "enabled": enabled,
            "paused": paused,
            "state": state,
            "done": total.saturating_sub(pending),
            "total": total,
            "pendingPhotos": pending,
            "indexedFaces": s.faces.index.len(),
            "searchedPhotos": s.faces.scanned.len(),
        }))
    }

    /// Drop what the scan has learned for this library: every face embedding (of every recognition model) and the list
    /// of photos already searched, in memory and on disk. Faces already in the catalog (named, or found by an earlier
    /// scan) stay. What a worker is still making is dropped when it comes back.
    fn drop_index(s: &mut Session, cmd: &str) {
        abort_maintenance(s);
        // the chosen model's index is loaded first, so its file is the one written fresh (none chosen: only the files go)
        let _ = current_manifest(s, cmd, true);
        s.faces.epoch = s.faces.epoch.wrapping_add(1);
        s.faces.index.clear();
        s.faces.scanned.clear();
        s.faces.scan_failed.clear();
        s.faces.queue.clear();
        s.faces.queue_stamp = None;
        s.faces.in_flight.clear();
        s.faces.task = None;
        s.faces.peak = 0;
        if let Some(dir) = s.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.clone())
            && let Ok(entries) = std::fs::read_dir(&dir)
        {
            for e in entries.flatten() {
                let name = e.file_name();
                let name = name.to_string_lossy();
                if name == "face-scanned.bin" || (name.starts_with("face-embeddings-") && name.ends_with(".bin")) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        // the chosen model's file and the searched list come back empty, so what is learned next is appended to them
        if let Err(e) = s.faces.index.save() {
            log::warn!("could not save the face embeddings: {e}");
        }
        if let Err(e) = s.faces.scanned.save() {
            log::warn!("could not save the list of searched photos: {e}");
        }
    }

    /// `faces.clearIndex`: drop the face index (see [`drop_index`]) and pause the scan, so it does not start over at once.
    pub fn clear(s: &mut Session, _: &Value) -> Result<Value> {
        const C: &str = "faces.clearIndex";
        set_paused(s, C, true)?;
        drop_index(s, C);
        status(s, &Value::Null)
    }

    /// `faces.restart`: drop the face index and scan the library again from the start.
    pub fn restart(s: &mut Session, _: &Value) -> Result<Value> {
        const C: &str = "faces.restart";
        set_paused(s, C, false)?;
        drop_index(s, C);
        status(s, &Value::Null)
    }

    /// `faces.reset`: confirmed, destructive clean slate. Remove face-region assignments and all recognition caches,
    /// while leaving other region kinds and unrelated people-role bindings alone.
    pub fn reset(s: &mut Session, p: &Value) -> Result<Value> {
        const C: &str = "faces.reset";
        if p.get("confirmed").and_then(Value::as_bool) != Some(true) {
            return Err(bad(C, "set `confirmed: true` to remove every face region, name, decision and recognition cache"));
        }
        // Keep the clean slate: if the continuous recogniser remained enabled and unpaused, its next pump would start
        // finding and embedding faces again immediately after this reset.
        let paused = s.face_models_dir.is_some();
        if paused {
            set_paused(s, C, true)?;
        }
        abort_maintenance(s);
        let mut edits = Vec::new();
        let (mut regions, mut photos_changed) = (0usize, 0usize);
        for photo in s.catalog.photos() {
            let mut meta = photo.meta.clone();
            let before = meta.regions.len();
            meta.regions.retain(|region| !matches!(region.kind, RegionKind::Face | RegionKind::IgnoredFace));
            let removed = before - meta.regions.len();
            if removed > 0 {
                regions += removed;
                photos_changed += 1;
                edits.push(Op::SetMeta { id: photo.id, meta: Box::new(meta) });
            }
        }
        // This file also holds unrelated AI-mask role bindings, which a face reset does not erase. Remove the actual
        // stored appearance examples and face-suggestion decisions, so a clean scan cannot inherit stale identities.
        s.with_people(|store| {
            store.people.clear();
            store.decisions.clear();
        })
        .map_err(|error| bad(C, error))?;
        drop_index(s, C);
        if !edits.is_empty() {
            s.commit("Reset Face Data", Op::Batch { ops: edits })?;
            s.skip_auto_write = true;
        }
        Ok(json!({
            "reset": true,
            "paused": paused,
            "photos": photos_changed,
            "faceRegions": regions,
            "indexedFaces": s.faces.index.len(),
            "searchedPhotos": s.faces.scanned.len()
        }))
    }

    /// `faces.pump`: cheap to call every frame. Takes in what the background worker has finished and, when it is idle,
    /// hands it the next photo with faces to embed. Does nothing (and says so) until recognition is switched on in
    /// Settings with a model chosen.
    pub fn pump(s: &mut Session, p: &Value) -> Result<Value> {
        const C: &str = "faces.pump";
        // how hard the scan may work now, from what the user is doing (the app says); nothing new is started while paused,
        // and what is already running finishes
        // paused by the user (`faces.pause`): nothing new is started until `faces.resume`
        let (enabled, paused) = switches_cached(s);
        let pace = if paused { Pace::Pause } else { Pace::parse(p.get("pace").and_then(Value::as_str)) };
        let target = target_workers(pace);
        // a model that has finished downloading is installed and chosen here, whether or not recognition was on
        finish_downloads(s);
        if !enabled && s.faces.maintenance.is_none() {
            track(s, 0, false);
            if let Some(w) = &s.faces.worker {
                w.allow(0, Pace::Pause);
            }
            let _ = current_manifest(s, C, false);
            collect(s);
            if (s.faces.index.has_unsaved() || s.faces.scanned.has_unsaved()) && s.faces.saved_at.is_none_or(|t| t.elapsed() >= SAVE_EVERY) {
                if let Err(e) = s.faces.index.save() {
                    log::warn!("could not save the face embeddings: {e}");
                }
                if let Err(e) = s.faces.scanned.save() {
                    log::warn!("could not save the list of searched photos: {e}");
                }
                s.faces.saved_at = Some(Instant::now());
            }
            return Ok(json!({
                "active": false,
                "paused": paused,
                "state": "off",
                "inFlight": 0,
                "pendingPhotos": 0,
                "peak": 0,
                "indexedFaces": s.faces.index.len(),
            }));
        }
        if s.faces.maintenance.is_none() && s.faces.retry_at.is_some_and(|t| Instant::now() < t) {
            track(s, 0, paused);
            return Ok(json!({"active": false, "paused": paused, "state": "idle"}));
        }
        let Ok(embedder) = current_with(s, C, false) else {
            // the chosen model is gone or broken: look again in a few seconds, not every frame; an explicit scoped run
            // has nowhere to make progress, so finish it as cancelled instead of leaving a stuck activity row.
            s.faces.retry_at = Some(Instant::now() + Duration::from_secs(3));
            if s.faces.maintenance.is_some() {
                abort_maintenance(s);
                finish_maintenance_if_done(s);
                return Ok(json!({"active": false, "paused": paused, "state": "idle", "maintenance": maintenance_status(s, &Value::Null)?}));
            }
            track(s, 0, paused);
            return Ok(json!({"active": false, "paused": paused, "state": "idle"}));
        };
        s.faces.retry_at = None;
        // the card the model ran on stopped working mid-scan: the rest goes to the other card, or the CPU
        {
            let net = embedder.clone();
            super::super::face_detect::move_off_lost_card(s, "recognition", embedder.net().accelerator().is_some(), move |gpu| {
                net.accelerate(gpu, true).map_err(|e| e.to_string())
            });
        }
        let (embedded, _) = collect(s);
        if (s.faces.index.has_unsaved() || s.faces.scanned.has_unsaved()) && s.faces.saved_at.is_none_or(|t| t.elapsed() >= SAVE_EVERY) {
            if let Err(e) = s.faces.index.save() {
                log::warn!("could not save the face embeddings: {e}");
            }
            if let Err(e) = s.faces.scanned.save() {
                log::warn!("could not save the list of searched photos: {e}");
            }
            s.faces.saved_at = Some(Instant::now());
        }
        finish_maintenance_if_done(s);
        if !enabled && s.faces.maintenance.is_none() {
            track(s, 0, false);
            if let Some(worker) = &s.faces.worker {
                worker.allow(0, Pace::Pause);
            }
            return Ok(json!({
                "active": false,
                "paused": paused,
                "state": "off",
                "inFlight": 0,
                "pendingPhotos": 0,
                "peak": 0,
                "indexedFaces": s.faces.index.len(),
                "maintenance": maintenance_status(s, &Value::Null)?,
            }));
        }
        if s.faces.maintenance.is_some() {
            let (runs, share, second_share) = crate::faces_worker::gpu_pace(pace);
            lcc_gpu::faces::set_pace(runs, share, second_share);
            if let Some(run) = s.faces.maintenance.as_ref() {
                run.task.set_controls(Some(crate::activity::Controls {
                    paused,
                    pause: "faces.pause",
                    resume: "faces.resume",
                    settings_tab: "faces",
                    settings_section: None,
                }));
            }
            queue_maintenance(s, &embedder, pace, target);
            finish_maintenance_if_done(s);
            if !enabled {
                track(s, 0, false);
            } else {
                let maintenance_in_flight = s.faces.maintenance.as_ref().map_or(0, |run| run.in_flight.len());
                let regular_pending = s.faces.queue.len() + s.faces.in_flight.len().saturating_sub(maintenance_in_flight);
                track(s, regular_pending as u64, paused);
            }
            let maintenance = maintenance_status(s, &Value::Null)?;
            let total_pending = s.faces.queue.len() + s.faces.in_flight.len();
            return Ok(json!({
                "active": true,
                "paused": paused,
                "state": if paused { "paused" } else { "maintenance" },
                "inFlight": s.faces.in_flight.len(),
                "pendingPhotos": total_pending,
                "indexedFaces": s.faces.index.len(),
                "searchedPhotos": s.faces.scanned.len(),
                "workers": target,
                "embedded": embedded,
                "maintenance": maintenance,
            }));
        }
        if s.faces.in_flight.is_empty() && s.faces.queue.is_empty() && s.faces.queue_stamp != Some(s.catalog.revision) {
            // the queue is taken from the end, so it is made last first
            let mut queue = pending(s, &[], true);
            queue.reverse();
            s.faces.queue = queue;
            s.faces.queue_stamp = Some(s.catalog.revision);
        }
        if target > 0 && s.faces.worker.is_none() && !s.faces.queue.is_empty() {
            s.faces.worker = Worker::start(max_workers());
        }
        // the workers are allowed `target` photos at once; a couple more wait behind them, so a worker that finishes has its
        // next photo at once instead of waiting for the next frame to hand it one
        if let Some(w) = &s.faces.worker {
            w.allow(target, pace);
        }
        // the GPU is held back the same way: a light pace leaves it mostly free for the window's own renders
        let (runs, share, second_share) = crate::faces_worker::gpu_pace(pace);
        lcc_gpu::faces::set_pace(runs, share, second_share);
        while target > 0 && s.faces.in_flight.len() < target + PREFETCH {
            let Some(id) = s.faces.queue.pop() else { break };
            let Some(job) = prepare(s, id, true, false, false, crate::faces_worker::REGION_SCORE, false) else { continue };
            if s.faces.worker.as_ref().is_some_and(|w| w.submit(embedder.clone(), job)) {
                s.faces.in_flight.insert(id);
            } else {
                break;
            }
        }
        let pending = s.faces.queue.len() + s.faces.in_flight.len();
        let peak = track(s, pending as u64, paused);
        let (state, _, _) = state(s);
        Ok(json!({
            "active": true,
            "paused": paused,
            "state": state,
            "inFlight": s.faces.in_flight.len(),
            "pendingPhotos": pending,
            "peak": peak,
            "indexedFaces": s.faces.index.len(),
            "searchedPhotos": s.faces.scanned.len(),
            "workers": target,
            "embedded": embedded,
        }))
    }

    /// The unnamed faces that look most like the person whose lower-cased name is `key`, best first, at most `limit`: each
    /// is scored like a suggestion (the mean of its two closest faces of theirs) and kept above four fifths of the
    /// model's suggestion bar, since a person looks at these before anything is named; a face that does not look clearly
    /// more like them than like anyone else already named (by [`DEFAULT_MARGIN`]) is left out. Only faces already embedded
    /// can be found, and only named faces already embedded count as someone else: while the scan runs, a face can be
    /// offered until the faces of the person it looks more like are embedded. [`RIVAL_CHECKS`] times `limit` faces are
    /// checked against everyone else, and up to [`RIVAL_CHECKS`] times as many until one is kept; `ready` says whether a
    /// recognition model is chosen and its index is available.
    pub fn similar(s: &mut Session, key: &str, limit: usize, scope: &super::Scope) -> Similar {
        let none = Similar { items: Vec::new(), ready: false, pending: 0 };
        let Some(_) = s.face_models_dir.clone() else { return none };
        let Ok(manifest) = current_manifest(s, "faces.person", false) else { return none };
        let pending = s.faces.queue.len() + s.faces.in_flight.len();
        let (mut gallery, mut candidates): (Vec<Arc<[f32]>>, Vec<(u64, usize, Rect, Arc<[f32]>)>) = (Vec::new(), Vec::new());
        // everyone else's named faces, by person (anywhere in the library, like theirs)
        let mut others: std::collections::HashMap<String, Vec<Arc<[f32]>>> = std::collections::HashMap::new();
        for ph in s.catalog.photos().filter(|ph| ph.in_library()) {
            for (index, r) in ph.meta.regions.iter().enumerate().filter(|(_, r)| r.kind == RegionKind::Face) {
                let Some(e) = s.faces.index.get(ph.id.0, &r.rect) else { continue };
                match r.name.as_deref().map(|n| n.trim().to_lowercase()) {
                    Some(n) if n == key => gallery.push(e.clone()),
                    Some(n) if !n.is_empty() => others.entry(n).or_default().push(e.clone()),
                    Some(_) => {}
                    // their faces anywhere teach the match; only faces in scope are offered
                    None if super::in_scope(scope, ph.id.0) => candidates.push((ph.id.0, index, r.rect, e.clone())),
                    None => {}
                }
            }
        }
        let ready = Similar { items: Vec::new(), ready: true, pending };
        if gallery.is_empty() {
            return ready;
        }
        let bar = manifest.thresholds.match_cosine.unwrap_or(DEFAULT_THRESHOLD) * 0.8;
        // with Settings ▸ Faces ▸ Ignore a person's odd faces, only their typical faces: a face from behind named after
        // them would bring every other back of a head along
        let skip_odd = s.face_models_dir.as_deref().is_some_and(|d| read_settings(d).skip_odd);
        fn faces_of(faces: &[Arc<[f32]>], skip_odd: bool) -> Vec<&[f32]> {
            let all: Vec<&[f32]> = faces.iter().map(|e| &e[..]).collect();
            if skip_odd { matching::typical(&all) } else { all }
        }
        let refs: Vec<&[f32]> = faces_of(&gallery, skip_odd);
        let top = top_faces(s);
        // every unnamed face against every face of theirs: hundreds of millions of comparisons in a large
        // library, so on every core (minutes on one)
        let mut scored: Vec<(f32, u64, usize, Rect, Arc<[f32]>)> = candidates
            .into_par_iter()
            .filter_map(|(photo, index, rect, e)| {
                let score = matching::person_score(&e, &refs, top)?;
                (score >= bar).then_some((score, photo, index, rect, e))
            })
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        // a face is offered only when it looks clearly more like this person than like anyone else already named, by the
        // margin name suggestions ask for: one that looks more like someone else is theirs (two people who look alike),
        // and one that looks about as much like everybody is a poor face (tiny, blurred, from behind) that would fill
        // the page. Best first, a batch at a time until the page is full
        let others: Vec<Vec<&[f32]>> = others.values().map(|faces| faces_of(faces, skip_odd)).collect();
        let mut kept = Vec::with_capacity(limit.min(scored.len()));
        let window = limit.saturating_mul(RIVAL_CHECKS);
        scored.truncate(window.saturating_mul(RIVAL_CHECKS));
        let mut checked = 0usize;
        for batch in scored.chunks(256) {
            if checked >= window && !kept.is_empty() {
                break;
            }
            checked = checked.saturating_add(batch.len());
            let theirs: Vec<bool> = batch
                .par_iter()
                .map(|(score, .., e)| others.iter().any(|g| matching::person_score(e, g, top).is_some_and(|o| o > *score - DEFAULT_MARGIN)))
                .collect();
            kept.extend(batch.iter().zip(theirs).filter(|(_, theirs)| !theirs).map(|(c, _)| (c.0, c.1, c.2, c.3)));
            if kept.len() >= limit {
                break;
            }
        }
        kept.truncate(limit);
        let scored = kept;
        let items = scored
            .into_iter()
            .map(|(score, photo, index, rect)| {
                let view = s.face_view(PhotoId(photo), rect);
                json!({"photo": photo, "index": index, "rect": super::rect_json(&rect), "view": super::rect_json(&view), "score": score})
            })
            .collect();
        Similar { items, ..ready }
    }

    /// How many times the page's size in faces a person's page checks against everyone else already named, best first:
    /// each check is a pass over every named face in the library, and it runs while the window waits, so a person whose
    /// look-alikes are mostly someone else's gets a shorter page rather than a frozen window. When none of those is kept,
    /// up to that many times as many are checked, so a page is not left empty while faces further down would do. The
    /// next page (after a face is named) checks the next ones.
    const RIVAL_CHECKS: usize = 4;

    /// Most faces put in order of likeness (the order is a pass over every pair of them).
    const MAX_ARRANGED: usize = 1500;

    /// Faces in an order that puts look-alikes together: start anywhere, then always go to the closest face not yet placed.
    fn nearest_chain(embeddings: &[&[f32]]) -> Vec<usize> {
        let n = embeddings.len();
        let (mut placed, mut order) = (vec![false; n], Vec::with_capacity(n));
        let mut current = 0;
        if n == 0 {
            return order;
        }
        placed[0] = true;
        order.push(0);
        for _ in 1..n {
            let mut best: Option<(usize, f32)> = None;
            for (j, e) in embeddings.iter().enumerate() {
                if placed[j] {
                    continue;
                }
                let c = matching::cosine(embeddings[current], e).unwrap_or(-1.0);
                if best.is_none_or(|(_, b)| c > b) {
                    best = Some((j, c));
                }
            }
            let Some((j, _)) = best else { break };
            placed[j] = true;
            order.push(j);
            current = j;
        }
        order
    }

    /// How many faces one chunk of the judging covers: they are scored in parallel, and the clock is looked at between
    /// chunks, so a call overshoots its budget by about one chunk rather than by the library.
    const JUDGE_CHUNK: usize = 16;
    /// The most faces one call judges, whatever the clock says (a coarse timer must not let a call run away).
    const JUDGE_PER_CALL: usize = 200_000;

    /// What the model and the bars of a suggestion are, as one number: the model's tag (which carries its identity,
    /// version and mirror setting), which library it is, the threshold and how many of a person's faces a score
    /// averages. A different one makes every judged face wrong, so the scan starts over.
    fn model_stamp(tag: &str, epoch: u64, threshold: f32, top: usize) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        tag.hash(&mut h);
        epoch.hash(&mut h);
        threshold.to_bits().hash(&mut h);
        top.hash(&mut h);
        h.finish()
    }

    /// The named faces the suggestions are made from, as one number: each one's name, photo and region. A different one
    /// has every face judged again, over the answer the last one gave (a view that emptied on every name would be no
    /// use on a large library).
    fn gallery_stamp(gallery: &[(String, u64, usize, Arc<[f32]>)]) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        gallery.len().hash(&mut h);
        for (name, photo, index, _) in gallery {
            name.trim().to_lowercase().hash(&mut h);
            photo.hash(&mut h);
            index.hash(&mut h);
        }
        h.finish()
    }

    /// The unnamed faces a suggestion is made from, and the named ones it is compared with: the named faces of the
    /// library with an embedding, each with the photo and region it is (so one gallery is told from another).
    fn suggestion_gallery(s: &Session) -> Vec<(String, u64, usize, Arc<[f32]>)> {
        s.catalog
            .photos()
            .filter(|ph| ph.in_library())
            .flat_map(|ph| {
                ph.meta
                    .regions
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.kind == RegionKind::Face)
                    .filter_map(|(index, r)| Some((r.name.clone()?, ph.id.0, index, s.faces.index.get(ph.id.0, &r.rect)?.clone())))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// The name the named faces suggest for each of `faces`, as `faces.suggest` would, worked out here and now: a
    /// decision acts on this, never on what a page (or an older scan) showed.
    pub fn suggest_for(s: &mut Session, faces: &[(u64, usize, Rect)]) -> std::collections::HashMap<(u64, usize), (String, f32)> {
        let mut out = std::collections::HashMap::new();
        let Some(_) = s.face_models_dir.clone() else { return out };
        let Ok(manifest) = current_manifest(s, "faces.unnamed", false) else { return out };
        let gallery = suggestion_gallery(s);
        if gallery.is_empty() {
            return out;
        }
        let refs: Vec<(&str, &[f32])> = gallery.iter().map(|(name, _, _, e)| (name.as_str(), &e[..])).collect();
        let threshold = manifest.thresholds.match_cosine.unwrap_or(DEFAULT_THRESHOLD);
        let top = top_faces(s);
        for (photo, index, rect) in faces {
            let Some(e) = s.faces.index.get(*photo, rect) else { continue };
            if let Some(x) = matching::suggest(&matching::rank_top(e, &refs, top), threshold, DEFAULT_MARGIN) {
                out.insert((*photo, *index), (x.name, x.score));
            }
        }
        out
    }

    /// Judge every unnamed face with an embedding against the named ones, a slice per call, so a suggestion is the
    /// library's answer and not a page's (issue #1365): `faces.unnamed` lists a page, and which page a face falls on
    /// must not decide whether the person it looks like is offered. The work is kept in [`crate::face_suggestions`],
    /// so it resumes where the last call stopped; it starts over only when what a suggestion depends on changed (the
    /// model, the named faces, the bars). The faces of `first` (the page about to be listed) are judged first, so a
    /// page keeps its names and hover scores while the rest of the library is looked at again.
    ///
    /// With `budget` zero nothing is judged: the caller only asks how far the judging is.
    pub fn scan_suggestions(s: &mut Session, budget: Duration, first: &[(u64, usize, Rect)]) -> super::Progress {
        let done = super::Progress { total: 0, judged: 0, complete: true };
        let Some(_) = s.face_models_dir.clone() else { return done };
        let Ok(manifest) = current_manifest(s, "faces.unnamed", false) else { return done };
        let threshold = manifest.thresholds.match_cosine.unwrap_or(DEFAULT_THRESHOLD);
        let top = top_faces(s);
        let gallery = suggestion_gallery(s);
        let (model, named) = (model_stamp(&s.faces.index.tag, s.faces.epoch, threshold, top), gallery_stamp(&gallery));
        {
            let scan = &mut s.faces.suggestions;
            if scan.model != model {
                scan.reset(model, named);
            } else if scan.gallery != named {
                scan.named_again(named);
            }
        }
        // nothing is named (or nothing named is embedded): no face has a person to be suggested as
        if gallery.is_empty() {
            let scan = &mut s.faces.suggestions;
            scan.complete = true;
            scan.indexed = s.faces.index.len();
            return done;
        }
        // nothing was embedded since every face was judged: the walk over the library can be skipped
        let judged_scan = &s.faces.suggestions;
        if judged_scan.complete && judged_scan.indexed == s.faces.index.len() {
            return super::Progress { total: judged_scan.judged.len(), judged: judged_scan.judged.len(), complete: true };
        }
        // the faces still to judge, and how many of the library's are judged under this stamp already
        let (mut todo, mut judged) = {
            let mut todo: Vec<(u64, usize, Arc<[f32]>)> = Vec::new();
            let mut judged = 0usize;
            for ph in s.catalog.photos().filter(|ph| ph.in_library()) {
                for (index, r) in ph.meta.regions.iter().enumerate() {
                    if r.kind != RegionKind::Face || r.name.is_some() {
                        continue;
                    }
                    let Some(e) = s.faces.index.get(ph.id.0, &r.rect) else { continue };
                    if judged_scan.judged.contains(&(ph.id.0, index)) {
                        judged += 1;
                    } else {
                        todo.push((ph.id.0, index, e.clone()));
                    }
                }
            }
            (todo, judged)
        };
        let total = judged + todo.len();
        if !budget.is_zero() && !todo.is_empty() {
            // the page's faces first: they are what the caller is about to list
            let page: std::collections::HashSet<(u64, usize)> = first.iter().map(|(photo, index, _)| (*photo, *index)).collect();
            todo.sort_by_key(|(photo, index, _)| !page.contains(&(*photo, *index)));
            let refs: Vec<(&str, &[f32])> = gallery.iter().map(|(name, _, _, e)| (name.as_str(), &e[..])).collect();
            let started = Instant::now();
            let mut looked = 0usize;
            for chunk in todo.chunks(JUDGE_CHUNK) {
                let found: Vec<((u64, usize), Option<(String, f32)>, Option<(String, f32)>)> = chunk
                    .par_iter()
                    .map(|(photo, index, e)| {
                        let ranked = matching::rank_top(e, &refs, top);
                        let similarity = ranked.first().map(|b| (b.name.clone(), b.score));
                        let suggestion = matching::suggest(&ranked, threshold, DEFAULT_MARGIN).map(|x| (x.name, x.score));
                        ((*photo, *index), similarity, suggestion)
                    })
                    .collect();
                let scan = &mut s.faces.suggestions;
                for (key, similarity, suggestion) in found {
                    scan.judge(key, similarity, suggestion);
                }
                judged += chunk.len();
                looked += chunk.len();
                if started.elapsed() >= budget || looked >= JUDGE_PER_CALL {
                    break;
                }
            }
        }
        let complete = judged == total;
        let scan = &mut s.faces.suggestions;
        scan.complete = complete;
        if complete {
            scan.indexed = s.faces.index.len();
        }
        super::Progress { total, judged, complete }
    }

    /// The grouped suggestions the scope holds ([`super::suggestion_groups`]), from what the scan has judged so far.
    pub fn suggestion_groups(s: &mut Session, scope: &super::Scope, people: &std::collections::HashSet<String>) -> Value {
        let found = s.faces.suggestions.suggestions.clone();
        super::suggestion_groups(s, &found, scope, people)
    }

    /// Put the unnamed faces `all` in order of likeness (the ones already embedded; the rest follow in photo order) and
    /// give the page's faces what the named faces recognise them as, as the library-wide scan found it.
    ///
    /// The scan ([`scan_suggestions`]) has already judged the library, so which faces carry a suggestion no longer
    /// depends on their place in this order or on `limit` (issue #1365): that is only how many are listed.
    pub fn arrange_unnamed(
        s: &mut Session,
        all: &mut Vec<(u64, usize, Rect)>,
        limit: usize,
        people: &std::collections::HashSet<String>,
    ) -> super::Arranged {
        let none = super::Arranged {
            ordered: false,
            ready: false,
            suggestions: Default::default(),
            similarities: Default::default(),
            groups: Default::default(),
        };
        let Some(_) = s.face_models_dir.clone() else { return none };
        let Ok(manifest) = current_manifest(s, "faces.unnamed", false) else { return none };
        let (mut embedded, mut plain) = (Vec::new(), Vec::new());
        for f in all.drain(..) {
            match s.faces.index.get(f.0, &f.2) {
                Some(e) => embedded.push((f, e.clone())),
                None => plain.push(f),
            }
        }
        let beyond = embedded.split_off(embedded.len().min(MAX_ARRANGED));
        let order = nearest_chain(&embedded.iter().map(|(_, e)| &e[..]).collect::<Vec<_>>());
        let ordered: Vec<((u64, usize, Rect), Arc<[f32]>)> = order.into_iter().filter_map(|i| embedded.get(i).cloned()).collect();
        let threshold = manifest.thresholds.match_cosine.unwrap_or(DEFAULT_THRESHOLD);
        let groups = look_alike_groups(ordered.iter().map(|((photo, index, _), e)| ((*photo, *index), &e[..])), threshold);
        let grouped = !ordered.is_empty();
        all.extend(ordered.into_iter().map(|(f, _)| f));
        all.extend(beyond.into_iter().map(|(f, _)| f));
        all.extend(plain);
        // a suggestion naming someone since renamed or removed is not shown: the judging has still to reach this face
        let known = |v: &(String, f32)| people.contains(&v.0.trim().to_lowercase());
        let (mut suggestions, mut similarities) = (std::collections::HashMap::new(), std::collections::HashMap::new());
        for (photo, index, _) in all.iter().take(limit) {
            if let Some(v) = s.faces.suggestions.similarities.get(&(*photo, *index)).filter(|v| known(v)) {
                similarities.insert((*photo, *index), v.clone());
            }
            if let Some(v) = s.faces.suggestions.suggestions.get(&(*photo, *index)).filter(|v| known(v)) {
                suggestions.insert((*photo, *index), v.clone());
            }
        }
        super::Arranged { ordered: grouped, ready: true, suggestions, similarities, groups }
    }

    /// Number the runs of `chain` (faces in order of likeness) whose neighbours are at least `threshold` alike: each
    /// run is one look-alike group. A pair that cannot be compared starts a new group.
    pub(super) fn look_alike_groups<'a>(
        chain: impl Iterator<Item = ((u64, usize), &'a [f32])>,
        threshold: f32,
    ) -> std::collections::HashMap<(u64, usize), usize> {
        let mut groups = std::collections::HashMap::new();
        let mut group = 0usize;
        let mut prev: Option<&[f32]> = None;
        for (key, e) in chain {
            if let Some(p) = prev
                && !matching::cosine(p, e).is_some_and(|c| c >= threshold)
            {
                group = group.saturating_add(1);
            }
            groups.insert(key, group);
            prev = Some(e);
        }
        groups
    }

    /// How many of a person's best matching faces a match is based on (Settings ▸ Faces), looked up at most once a second.
    fn top_faces(s: &mut Session) -> usize {
        if let Some((t, n)) = s.faces.top_seen
            && t.elapsed() < RECHECK
        {
            return n;
        }
        let n = s.face_models_dir.as_deref().map_or(matching::TOP_FACES, |d| read_settings(d).top_faces());
        s.faces.top_seen = Some((Instant::now(), n));
        n
    }

    /// `topFaces` asked for in a command (1 to 8, refused otherwise, as `faces.matching` refuses it), else the setting.
    fn top_faces_param(s: &mut Session, cmd: &str, p: &Value) -> Result<usize> {
        let asked = match p.get("topFaces") {
            Some(v) => top_faces_value(cmd, v, "the count chosen in Settings")?,
            None => None,
        };
        Ok(asked.unwrap_or_else(|| top_faces(s)))
    }

    pub fn suggest(s: &mut Session, p: &Value) -> Result<Value> {
        const C: &str = "faces.suggest";
        let budget = Duration::from_millis(p.get("budgetMs").and_then(Value::as_u64).unwrap_or(1500).min(600_000));
        let manifest = current_manifest(s, C, !budget.is_zero())?;
        collect(s);
        let targets = s.targets(p)?;
        let model_threshold = manifest.thresholds.match_cosine;
        let num = |k: &str| p.get(k).and_then(Value::as_f64).filter(|v| v.is_finite()).map(|v| v as f32);
        let threshold = num("threshold").or(model_threshold).unwrap_or(DEFAULT_THRESHOLD);
        let margin = num("margin").unwrap_or(DEFAULT_MARGIN);
        let top = top_faces_param(s, C, p)?;
        // the photos asked about are embedded first, then the named faces they are compared with, within the budget
        let started = Instant::now();
        let todo = pending(s, &targets, false);
        let mut done = 0;
        if !budget.is_zero() && !todo.is_empty() {
            let embedder = current(s, C)?;
            for id in &todo {
                if done > 0 && started.elapsed() >= budget {
                    break;
                }
                embed_photo(s, *id, &embedder)?;
                done += 1;
            }
        }
        let pending_photos: Vec<u64> = todo.iter().skip(done).map(|id| id.0).collect();
        let gallery: Vec<(String, Arc<[f32]>)> = s
            .catalog
            .photos()
            .filter(|ph| ph.in_library())
            .flat_map(|ph| {
                ph.meta
                    .regions
                    .iter()
                    .filter(|r| r.kind == RegionKind::Face)
                    .filter_map(|r| Some((r.name.clone()?, s.faces.index.get(ph.id.0, &r.rect)?.clone())))
                    .collect::<Vec<_>>()
            })
            .collect();
        let refs: Vec<(&str, &[f32])> = gallery.iter().map(|(n, e)| (n.as_str(), &e[..])).collect();
        let mut photos = Vec::new();
        for id in &targets {
            let Some(photo) = s.catalog.photo(*id) else { continue };
            let mut faces = Vec::new();
            for (index, r) in photo.meta.regions.iter().enumerate().filter(|(_, r)| r.kind == RegionKind::Face && r.name.is_none()) {
                let Some(q) = s.faces.index.get(id.0, &r.rect) else {
                    faces.push(json!({"index": index, "pending": true}));
                    continue;
                };
                let ranked = matching::rank_top(q, &refs, top);
                let suggestion = matching::suggest(&ranked, threshold, margin);
                faces.push(json!({
                    "index": index,
                    "suggestion": suggestion.as_ref().map(|x| json!({"name": x.name, "score": x.score, "runnerUp": x.runner_up.as_ref().map(|(n, sc)| json!({"name": n, "score": sc}))})),
                    "candidates": ranked.iter().take(3).map(|c| json!({"name": c.name, "score": c.score, "faces": c.faces})).collect::<Vec<_>>(),
                }));
            }
            photos.push(json!({"id": id.0, "faces": faces}));
        }
        Ok(
            json!({"model": s.faces.index.tag, "threshold": threshold, "margin": margin, "topFaces": top, "galleryFaces": gallery.len(), "pendingPhotos": pending_photos, "photos": photos}),
        )
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::tests_face_recognize::{install_face_model, region, set_regions, setup, temp, two_photos};

        /// #259: runs of look-alikes share a group; an unlike neighbour, or one that cannot be compared, starts a new one.
        #[test]
        fn look_alikes_next_to_each_other_form_groups() {
            let (a, a2, b, bad) = (unit(&[(0, 1.0)]), unit(&[(0, 1.0), (1, 0.1)]), unit(&[(5, 1.0)]), vec![f32::NAN; 64]);
            let chain = [((1, 0), &a), ((2, 0), &a2), ((3, 0), &b), ((4, 0), &bad), ((5, 0), &b)];
            let g = look_alike_groups(chain.iter().map(|(k, e)| (*k, &e[..])), 0.5);
            assert_eq!(g[&(1, 0)], g[&(2, 0)]);
            assert_ne!(g[&(2, 0)], g[&(3, 0)]);
            assert_ne!(g[&(3, 0)], g[&(4, 0)]);
            assert_ne!(g[&(4, 0)], g[&(5, 0)]);
            assert!(look_alike_groups(std::iter::empty(), 0.5).is_empty());
        }

        fn unit(parts: &[(usize, f32)]) -> Vec<f32> {
            let mut v = vec![0.0; 64];
            for (i, x) in parts {
                v[*i] = *x;
            }
            let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            v.iter().map(|x| x / n).collect()
        }

        /// A session with a tiny model loaded (so the index has its model's tag).
        fn loaded(name: &str) -> (Session, std::path::PathBuf) {
            let d = temp(name);
            let (mut s, _) = setup(&d, 64);
            s.execute("faces.index", &json!({"budgetMs": 0})).unwrap();
            (s, d)
        }

        fn finished(s: &Session, id: PhotoId, found: Option<Vec<Rect>>, results: Vec<(Rect, Option<Vec<f32>>, Rect)>) -> Done {
            let content = s.catalog.photo(id).map(|p| crate::media::content_key(p)).unwrap_or_default();
            Done {
                tag: s.faces.index.tag.clone(),
                epoch: s.faces.epoch,
                id,
                content,
                results,
                detect_wanted: true,
                redetect: false,
                replace_detections: false,
                found,
            }
        }

        #[test]
        fn the_background_scan_finds_faces_once_and_only_in_photos_without_any() {
            // faces found by the scan become unnamed regions without an undo step
            {
                let (mut s, d) = loaded("found");
                let (a, b) = two_photos(&s);
                set_regions(&mut s, b, vec![region(0.1, Some("Ann"))]);
                let (undo, redo) = (s.undo.len(), s.redo.len());
                let found = region(0.3, None).rect;
                let done = finished(&s, a, Some(vec![found]), vec![(found, Some(unit(&[(0, 1.0)])), found)]);
                assert_eq!(ingest(&mut s, done), (1, 0));
                let regions = &s.catalog.photo(a).unwrap().meta.regions;
                assert_eq!(regions.len(), 1);
                assert!(regions[0].kind == RegionKind::Face && regions[0].name.is_none() && regions[0].rect == found);
                assert!(regions[0].description.as_deref().unwrap().starts_with(MARK), "marked as detected");
                // not something the user did, and its embedding is already there
                assert_eq!((s.undo.len(), s.redo.len()), (undo, redo));
                assert!(s.skip_auto_write, "no sidecar is rewritten for it");
                assert!(s.faces.scanned.contains(a.0) && s.faces.index.contains(a.0, &found));
                // a photo that has faces by now (the user, an import) keeps exactly those
                let other = region(0.6, None).rect;
                let done = finished(&s, b, Some(vec![other]), vec![(other, None, other)]);
                ingest(&mut s, done);
                assert_eq!(s.catalog.photo(b).unwrap().meta.regions.len(), 1);
                assert!(s.faces.scanned.contains(b.0));
                let _ = std::fs::remove_dir_all(&d);
            }
            // a photo that could not be searched is left alone for the session
            {
                let (mut s, d) = loaded("scanfail");
                let (a, _) = two_photos(&s);
                assert!(needs_search(&s, s.catalog.photo(a).unwrap()));
                let done = finished(&s, a, None, vec![]);
                ingest(&mut s, done);
                assert!(!s.faces.scanned.contains(a.0), "not remembered as searched: the file may come back");
                assert!(!needs_search(&s, s.catalog.photo(a).unwrap()), "but not retried on every pass");
                assert!(!pending(&s, &[], true).contains(&a));
                let _ = std::fs::remove_dir_all(&d);
            }
            // only the background scan looks for faces and only in photos without any
            {
                let (mut s, d) = loaded("search");
                // finding faces needs the detector, which is not bundled; without it the scan only embeds
                let detector = install_face_model(&mut s);
                let (a, b) = two_photos(&s);
                set_regions(&mut s, b, vec![region(0.1, Some("Ann")), region(0.5, None)]);
                // `faces.index` embeds what is boxed; only the scan (search = true) also searches the rest, last
                assert_eq!(pending(&s, &[], false), vec![b]);
                let scan = pending(&s, &[], true);
                assert_eq!(scan.first(), Some(&b), "named faces first");
                assert!(scan.contains(&a) && scan.len() > 2, "the unsearched photos follow");
                assert!(
                    prepare(&mut s, a, false, false, false, crate::faces_worker::REGION_SCORE, false).is_none(),
                    "nothing to embed in a photo without boxes"
                );
                if detector {
                    assert!(
                        prepare(&mut s, a, true, false, false, crate::faces_worker::REGION_SCORE, false)
                            .is_some_and(|p| p.detect && p.todo.is_empty())
                    );
                } else {
                    assert!(
                        prepare(&mut s, a, true, false, false, crate::faces_worker::REGION_SCORE, false).is_none(),
                        "no detector: nothing to do, and not marked as searched"
                    );
                }
                assert!(
                    prepare(&mut s, b, true, false, false, crate::faces_worker::REGION_SCORE, false).is_some_and(|p| !p.detect && p.todo.len() == 2),
                    "a photo with boxes is not searched"
                );
                // once searched, a photo is not searched again
                s.faces.scanned.insert(a.0);
                assert!(!pending(&s, &[], true).contains(&a));
                let _ = std::fs::remove_dir_all(&d);
            }
        }

        #[test]
        fn faces_from_another_library_or_a_replaced_file_are_dropped() {
            // opening another library forgets the old ones faces
            {
                let (mut s, d) = loaded("newlib");
                let (a, _) = two_photos(&s);
                let rect = region(0.2, None).rect;
                s.faces.index.insert(a.0, &rect, unit(&[(0, 1.0)]));
                s.faces.scanned.insert(a.0);
                let epoch = s.faces.epoch;
                let old = finished(&s, a, Some(vec![rect]), vec![(rect, Some(unit(&[(1, 1.0)])), rect)]);
                s.open_library(d.join("second"), true).unwrap();
                assert_eq!((s.faces.index.len(), s.faces.scanned.len(), s.faces.queue.len(), s.faces.in_flight.len()), (0, 0, 0, 0));
                assert!(s.faces.epoch != epoch && s.faces.embedder.is_none());
                // a worker that was still busy with the old library cannot put its faces into the new one
                assert_eq!(ingest(&mut s, old), (0, 0));
                assert!(s.faces.index.len() == 0 && !s.faces.scanned.contains(a.0));
                let _ = std::fs::remove_dir_all(&d);
            }
            // faces made from a replaced file are dropped
            // A Reload that finds the file replaced (an outside edit) gives the photo other pixels: what a worker made from
            // the old file is dropped when it comes back, the faces already embedded from it are forgotten and the photo is
            // searched again (issue #1524).
            {
                let (mut s, d) = loaded("replaced");
                let (a, b) = two_photos(&s);
                let rect = region(0.2, None).rect;
                s.faces.index.insert(a.0, &rect, unit(&[(0, 1.0)]));
                s.faces.scanned.insert(a.0);
                s.faces.index.insert(b.0, &rect, unit(&[(1, 1.0)]));
                s.faces.scanned.insert(b.0);
                let old = finished(&s, a, Some(vec![rect]), vec![(rect, Some(unit(&[(2, 1.0)])), rect)]);
                let (width, height) = s.catalog.photo(a).map(|p| (p.width, p.height)).unwrap();
                let replace = Op::SetContent {
                    id: a,
                    width,
                    height,
                    file_size: 1234,
                    content_hash: Some("edited".into()),
                    preview_only: None,
                    preview_checked: String::new(),
                };
                s.commit("Reload Changed Files", replace).unwrap();
                assert!(s.faces.index.get(a.0, &rect).is_none() && !s.faces.scanned.contains(a.0), "the old file's faces are forgotten");
                assert!(needs_search(&s, s.catalog.photo(a).unwrap()), "and the photo is searched again");
                assert!(s.faces.index.get(b.0, &rect).is_some() && s.faces.scanned.contains(b.0), "other photos keep theirs");
                assert_eq!(ingest(&mut s, old), (0, 0), "a result made from the old file is dropped");
                assert!(s.catalog.photo(a).unwrap().meta.regions.is_empty() && !s.faces.scanned.contains(a.0));
                // what is made from the new file is taken in
                let new = finished(&s, a, Some(vec![rect]), vec![(rect, Some(unit(&[(3, 1.0)])), rect)]);
                assert_eq!(ingest(&mut s, new), (1, 0));
                assert!(s.faces.scanned.contains(a.0) && s.faces.index.get(a.0, &rect).is_some());
                let _ = std::fs::remove_dir_all(&d);
            }
        }

        #[test]
        fn the_unnamed_faces_that_look_like_a_person_come_best_first() {
            let (mut s, d) = loaded("similar");
            let (a, b) = two_photos(&s);
            let c = s.catalog.photos().map(|p| p.id).find(|id| *id != a && *id != b).unwrap();
            set_regions(&mut s, a, vec![region(0.1, Some("Ann")), region(0.5, Some("Bob"))]);
            set_regions(&mut s, b, vec![region(0.1, None), region(0.5, None)]);
            set_regions(&mut s, c, vec![region(0.3, None)]);
            let put = |s: &mut Session, id: PhotoId, x: f64, v: Vec<f32>| s.faces.index.insert(id.0, &region(x, None).rect, v);
            put(&mut s, a, 0.1, unit(&[(0, 1.0)])); // Ann
            put(&mut s, a, 0.5, unit(&[(1, 1.0)])); // Bob
            put(&mut s, b, 0.1, unit(&[(0, 0.95), (1, 0.05)])); // looks just like Ann
            put(&mut s, b, 0.5, unit(&[(1, 1.0)])); // looks like Bob, not Ann
            put(&mut s, c, 0.3, unit(&[(0, 0.8), (1, 0.6)])); // somewhat like Ann
            let r = similar(&mut s, "ann", 10, &None);
            assert!(r.ready);
            let found: Vec<(u64, u64)> = r.items.iter().map(|i| (i["photo"].as_u64().unwrap(), i["index"].as_u64().unwrap())).collect();
            assert_eq!(found, vec![(b.0, 0), (c.0, 0)], "best first; the face that looks like Bob is left out");
            assert!(r.items[0]["score"].as_f64().unwrap() > r.items[1]["score"].as_f64().unwrap());
            assert_eq!(similar(&mut s, "ann", 1, &None).items.len(), 1, "capped");
            assert!(similar(&mut s, "nobody", 10, &None).items.is_empty());
            let _ = std::fs::remove_dir_all(&d);
        }

        #[test]
        fn a_page_whose_first_faces_all_look_more_like_someone_else_checks_further() {
            let (mut s, d) = loaded("similar-further");
            let (a, b) = two_photos(&s);
            let c = s.catalog.photos().map(|p| p.id).find(|id| *id != a && *id != b).unwrap();
            set_regions(&mut s, a, vec![region(0.1, Some("Ann")), region(0.5, Some("Bob"))]);
            // a full first window of faces like Ann but more like Bob, then ten a little less like her and nothing like him
            let (limit, window) = (64, 64 * RIVAL_CHECKS);
            set_regions(&mut s, b, (0..window).map(|i| region(i as f64 * 0.001, None)).collect());
            set_regions(&mut s, c, (0..10).map(|i| region(i as f64 * 0.01, None)).collect());
            let put = |s: &mut Session, id: PhotoId, x: f64, v: Vec<f32>| s.faces.index.insert(id.0, &region(x, None).rect, v);
            put(&mut s, a, 0.1, unit(&[(0, 1.0)])); // Ann
            put(&mut s, a, 0.5, unit(&[(0, 0.6), (1, 0.8)])); // Bob
            for i in 0..window {
                put(&mut s, b, i as f64 * 0.001, unit(&[(0, 0.75), (1, 0.66)]));
            }
            for i in 0..10 {
                put(&mut s, c, i as f64 * 0.01, unit(&[(0, 0.7), (2, 0.714)]));
            }
            let r = similar(&mut s, "ann", limit, &None);
            let photos: Vec<u64> = r.items.iter().map(|i| i["photo"].as_u64().unwrap()).collect();
            assert_eq!(photos, vec![c.0; 10], "the faces past the first window are found, the ones like Bob are not");
            let _ = std::fs::remove_dir_all(&d);
        }

        fn file_scope_session() -> (Session, PhotoId, PhotoId, PhotoId, PhotoId) {
            let mut s = Session::new();
            let rows = [
                (PhotoId(1), "/photos/Trip/a.jpg", "a.jpg"),
                (PhotoId(2), "/photos/Trip/Day1/b.jpg", "b.jpg"),
                (PhotoId(3), "/photos/Trip-old/c.jpg", "c.jpg"),
                (PhotoId(4), "/photos/Elsewhere/d.jpg", "d.jpg"),
            ];
            for (id, path, name) in rows {
                let photo = Photo::new(id, Source::File { path: path.into() }, name, "JPEG", 60, 40, "2026-01-01");
                s.catalog.apply(Op::AddPhoto { photo: Box::new(photo) }).unwrap();
            }
            (s, rows[0].0, rows[1].0, rows[2].0, rows[3].0)
        }

        fn add_album(s: &mut Session, name: &str, parent: Option<u64>, folder: bool) -> u64 {
            s.execute("album.create", &json!({"name": name, "parent": parent, "folder": folder})).unwrap()["id"].as_u64().unwrap()
        }

        fn add_to_album(s: &mut Session, album: u64, photos: &[PhotoId]) {
            s.execute("library.select", &json!({"ids": photos.iter().map(|id| id.0).collect::<Vec<_>>()})).unwrap();
            s.execute("album.addPhotos", &json!({"id": album})).unwrap();
        }

        #[test]
        fn maintenance_folder_scope_resolves_direct_children_and_optional_subfolders() {
            let (mut s, direct, nested, sibling, outside) = file_scope_session();
            let (all, _) = maintenance_scope(&mut s, &json!({"folder": "/photos/Trip"})).unwrap();
            assert_eq!(all, vec![direct, nested], "subfolders are included by default; a similarly named sibling is not");
            let (only_direct, _) = maintenance_scope(&mut s, &json!({"folder": "/photos/Trip", "includeSubfolders": false})).unwrap();
            assert_eq!(only_direct, vec![direct], "only files directly in the chosen folder are included");
            let (many, _) =
                maintenance_scope(&mut s, &json!({"folders": ["/photos/Trip", "/photos/Elsewhere"], "includeSubfolders": false})).unwrap();
            assert_eq!(many, vec![direct, outside], "multiple folder roots are unioned and deduplicated");
            assert!(!many.contains(&sibling));
        }

        #[test]
        fn maintenance_collection_scope_always_recurses_nested_sets_and_unions_multiple_collections() {
            let (mut s, direct, nested, sibling, outside) = file_scope_session();
            let trip = add_album(&mut s, "Trip", None, true);
            let picks = add_album(&mut s, "Picks", Some(trip), false);
            let day = add_album(&mut s, "Day 1", Some(trip), true);
            let day_picks = add_album(&mut s, "Day picks", Some(day), false);
            let extra = add_album(&mut s, "Extra", None, false);
            add_to_album(&mut s, picks, &[direct]);
            add_to_album(&mut s, day_picks, &[nested]);
            add_to_album(&mut s, extra, &[sibling, outside]);

            let (with_subfolders, _) = maintenance_scope(&mut s, &json!({"albums": [trip]})).unwrap();
            assert_eq!(with_subfolders, vec![direct, nested], "a collection set includes all nested collections");
            let (folder_toggle_off, _) = maintenance_scope(&mut s, &json!({"albums": [trip], "includeSubfolders": false})).unwrap();
            assert_eq!(folder_toggle_off, vec![direct, nested], "the filesystem-folder toggle does not trim a collection set");
            let (multiple, _) = maintenance_scope(&mut s, &json!({"albums": [picks, extra]})).unwrap();
            assert_eq!(multiple, vec![direct, sibling, outside], "several collections are unioned and returned in photo order");
        }

        #[test]
        fn new_item_mode_skips_searched_photos_but_recognition_mode_keeps_existing_boxes() {
            let d = temp("maintenance-new");
            let (mut s, _) = setup(&d, 64);
            let (with_face, without_faces) = two_photos(&s);
            set_regions(&mut s, with_face, vec![region(0.1, None)]);
            assert!(mode_needs_photo(&s, FaceScanMode::Recognize, with_face), "existing unindexed face needs its embedding");
            assert!(mode_needs_photo(&s, FaceScanMode::New, with_face), "an unsearched photo with a face is new work");
            s.faces.scanned.insert(with_face.0);
            assert!(!mode_needs_photo(&s, FaceScanMode::New, with_face), "the new-items pass does not repeat a searched photo");
            assert!(mode_needs_photo(&s, FaceScanMode::Recognize, with_face), "recognition-only can still rebuild an existing box");
            assert!(mode_needs_photo(&s, FaceScanMode::Detect, without_faces), "detection considers unsearched photos without boxes");
            assert!(!mode_needs_photo(&s, FaceScanMode::Recognize, without_faces), "recognition-only does not search for boxes");
            let _ = std::fs::remove_dir_all(&d);
        }

        #[test]
        fn reset_requires_confirmation_and_preserves_other_region_kinds_and_people_roles() {
            let (mut s, d) = loaded("maintenance-reset");
            let (photo, _) = two_photos(&s);
            let face = region(0.1, Some("Ann"));
            let ignored = Region { kind: RegionKind::IgnoredFace, ..region(0.4, None) };
            let pet = Region { kind: RegionKind::Pet, ..region(0.55, Some("Milo")) };
            let focus = Region { kind: RegionKind::Focus, ..region(0.7, None) };
            let other = Region { kind: RegionKind::Other("RegionOfInterest".into()), ..region(0.8, None) };
            set_regions(&mut s, photo, vec![face.clone(), ignored, pet.clone(), focus.clone(), other.clone()]);
            s.faces.index.insert(photo.0, &face.rect, unit(&[(0, 1.0)]));
            s.faces.scanned.insert(photo.0);
            s.with_people(|store| {
                store.roles.push(crate::people_store::RoleBinding {
                    role: "Subject".into(),
                    person: "Ann".into(),
                    scope: crate::people_store::Scope::Folder("/photos/Trip".into()),
                });
                store.people.push(crate::people_store::PersonData {
                    name: "Ann".into(),
                    examples: vec![crate::people_store::Example {
                        photo: photo.0,
                        rect: [face.rect.x0, face.rect.y0, face.rect.x1, face.rect.y1],
                        signature: json!([1.0, 0.0]),
                    }],
                });
                store.decisions.push(crate::people_store::SuggestionDecision {
                    photo: photo.0,
                    index: 0,
                    person: "Ann".into(),
                    verdict: crate::people_store::Verdict::Rejected,
                    rect: Some(face.rect),
                    decided: "2026-01-01".into(),
                });
            })
            .unwrap();

            assert!(s.execute("faces.reset", &json!({})).is_err(), "a reset without explicit confirmation is refused");
            assert_eq!(s.catalog.photo(photo).unwrap().meta.regions.len(), 5, "the refused reset changes nothing");
            assert_eq!(s.faces.index.len(), 1);
            assert!(s.faces.scanned.contains(photo.0));

            let result = s.execute("faces.reset", &json!({"confirmed": true})).unwrap();
            assert_eq!(result["faceRegions"], 2, "Face and IgnoredFace regions are removed");
            assert_eq!(result["paused"], true, "the continuous scan stays paused after reset");
            assert_eq!(s.execute("faces.status", &json!({})).unwrap()["paused"], true, "the saved scan status is paused too");
            assert_eq!(s.catalog.photo(photo).unwrap().meta.regions, vec![pet, focus, other]);
            assert_eq!(s.faces.index.len(), 0);
            assert_eq!(s.faces.scanned.len(), 0);
            let people = s.people_store().clone();
            assert_eq!(people.roles.len(), 1, "role bindings remain intact");
            assert_eq!(people.roles[0].person, "Ann");
            assert!(people.people.is_empty(), "stored appearance examples are cleared");
            assert!(people.decisions.is_empty(), "face suggestion decisions are cleared");
            let _ = std::fs::remove_dir_all(&d);
        }

        #[test]
        fn replace_rescan_replaces_only_earlier_automatic_detections() {
            let (mut s, d) = loaded("maintenance-rescan");
            let (photo, _) = two_photos(&s);
            let mut automatic = region(0.1, None);
            automatic.description = Some(format!("{MARK}{LABEL}"));
            let mut named = region(0.4, Some("Ann"));
            named.description = Some(format!("{MARK}{LABEL}"));
            let manual = region(0.7, None);
            let ignored = Region { kind: RegionKind::IgnoredFace, ..region(0.82, None) };
            let mut pet = Region { kind: RegionKind::Pet, ..region(0.9, None) };
            pet.description = Some(format!("{MARK}{LABEL}"));
            let old_automatic = automatic.rect;
            set_regions(&mut s, photo, vec![automatic, named.clone(), manual.clone(), ignored.clone(), pet.clone()]);
            let fresh = region(0.2, None).rect;
            let mut done = finished(&s, photo, Some(vec![fresh]), vec![]);
            done.redetect = true;
            done.replace_detections = true;
            ingest(&mut s, done);
            let regions = &s.catalog.photo(photo).unwrap().meta.regions;
            assert_eq!(regions.len(), 5);
            assert!(
                regions.iter().any(|r| r.rect == fresh && r.name.is_none() && r.description.as_deref().is_some_and(|d| d.starts_with(MARK))),
                "the new automatic detection is kept"
            );
            assert!(regions.iter().any(|r| r.rect == named.rect && r.name.as_deref() == Some("Ann")), "named faces are kept");
            assert!(
                regions.iter().any(|r| r.rect == named.rect && r.description.as_deref().is_some_and(|d| d.starts_with(MARK))),
                "a named face with an automatic-detection marker is kept"
            );
            assert!(regions.iter().any(|r| r.rect == manual.rect && r.description.is_none()), "manual regions are kept");
            assert!(regions.iter().any(|r| r.kind == RegionKind::IgnoredFace), "ignored faces are kept");
            assert!(regions.iter().any(|r| r.kind == RegionKind::Pet), "other region kinds are kept");
            assert!(
                regions.iter().any(|r| r.kind == RegionKind::Pet && r.description.as_deref().is_some_and(|d| d.starts_with(MARK))),
                "non-face regions are not treated as automatic detections"
            );
            assert!(
                !regions.iter().any(|r| r.rect == old_automatic
                    && r.kind == RegionKind::Face
                    && r.name.as_deref().is_none_or(|name| name.trim().is_empty())
                    && r.description.as_deref().is_some_and(|d| d.starts_with(MARK))),
                "old unnamed automatic detections are replaced"
            );
            let _ = std::fs::remove_dir_all(&d);
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use serde_json::Value;

    use super::super::bad;
    use crate::{Result, Session};

    pub fn index(_: &mut Session, _: &Value) -> Result<Value> {
        Err(bad("faces.index", "this build cannot run face recognition"))
    }

    pub fn maintain(_: &mut Session, _: &Value) -> Result<Value> {
        Err(bad("faces.maintain", "this build cannot run face recognition"))
    }

    pub fn maintenance_status(_: &mut Session, _: &Value) -> Result<Value> {
        Ok(
            serde_json::json!({"state": "idle", "mode": null, "scope": null, "done": 0, "total": 0, "pendingPhotos": 0, "failed": 0, "cancelled": false, "taskId": null}),
        )
    }

    pub fn reset(_: &mut Session, _: &Value) -> Result<Value> {
        Err(bad("faces.reset", "this build cannot run face recognition"))
    }

    pub fn suggest(_: &mut Session, _: &Value) -> Result<Value> {
        Err(bad("faces.suggest", "this build cannot run face recognition"))
    }

    pub fn evaluate(_: &mut Session, _: &Value) -> Result<Value> {
        Err(bad("faces.evaluate", "this build cannot run face recognition"))
    }

    pub fn pump(_: &mut Session, _: &Value) -> Result<Value> {
        Ok(serde_json::json!({"active": false}))
    }

    pub fn status(_: &mut Session, _: &Value) -> Result<Value> {
        Ok(
            serde_json::json!({"enabled": false, "paused": false, "state": "off", "done": 0, "total": 0, "pendingPhotos": 0, "indexedFaces": 0, "searchedPhotos": 0}),
        )
    }

    pub fn clear(_: &mut Session, _: &Value) -> Result<Value> {
        Err(bad("faces.clearIndex", "this build cannot run face recognition"))
    }

    pub fn restart(_: &mut Session, _: &Value) -> Result<Value> {
        Err(bad("faces.restart", "this build cannot run face recognition"))
    }

    pub fn similar(_: &mut Session, _: &str, _: usize, _: &super::Scope) -> super::Similar {
        super::Similar { items: Vec::new(), ready: false, pending: 0 }
    }

    pub fn suggest_for(_: &mut Session, _: &[(u64, usize, lcc_geom::Rect)]) -> std::collections::HashMap<(u64, usize), (String, f32)> {
        std::collections::HashMap::new()
    }

    pub fn scan_suggestions(_: &mut Session, _: std::time::Duration, _: &[(u64, usize, lcc_geom::Rect)]) -> super::Progress {
        super::Progress { total: 0, judged: 0, complete: true }
    }

    pub fn suggestion_groups(_: &mut Session, _: &super::Scope, _: &std::collections::HashSet<String>) -> Value {
        Value::Array(Vec::new())
    }

    pub fn arrange_unnamed(
        _: &mut Session,
        _: &mut Vec<(u64, usize, lcc_geom::Rect)>,
        _: usize,
        _: &std::collections::HashSet<String>,
    ) -> super::Arranged {
        super::Arranged {
            ordered: false,
            ready: false,
            suggestions: Default::default(),
            similarities: Default::default(),
            groups: Default::default(),
        }
    }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "faces.ignore",
            "Ignore Faces",
            [],
            None,
            "{faces: [{photo, index, rect?}], ignored?: true}: ignore blurry or false faces (kept, never detected again, left out of People), or restore them with ignored: false; one undo step; catalog only",
            super::always,
            ignore_faces
        ),
        cmd!(query "faces.ignored", "Ignored Faces", [], None,
            "{offset?: 0, limit?: 1000} → {total, offset, faces: [{photo, index, rect, name, view}]}: the ignored faces in catalog order; limit 1..5000", super::always, ignored_faces),
        cmd!(
            "faces.index",
            "Index Faces",
            [],
            None,
            "{budgetMs?: 200, ids?: [photo ids to do first]} → {embedded, photosDone, pendingPhotos, indexedFaces, ms}: embed the faces in the library with the chosen recognition model, as many photos as fit in the time (at least one); call again until `pendingPhotos` is 0. `budgetMs: 0` only reports",
            super::always,
            imp::index
        ),
        cmd!(
            "faces.maintain",
            "Maintain Face Index",
            [],
            None,
            "{mode?: detect | recognize | new | rebuild | rebuildNamed | rescan, scope?: all | visible | selected, ids?, folder?, folders?, album?, albums?, includeSubfolders?: bool (filesystem folders only; collection sets include nested collections), minScore?: 0..1, behavior?: merge | replace} → {state, mode, scope, done, total, pendingPhotos, failed, taskId}: start a cancellable background maintenance pass. `detect` finds and embeds missing faces; `recognize` embeds existing boxes only; `new` skips indexed/searched photos; `rebuild` re-embeds existing boxes; `rebuildNamed` rebuilds the named-face gallery; `rescan` runs detection again and merges, or replaces earlier automatic detections with `behavior: replace`. Select folders/albums/ids, or the whole library by default. Pause with `faces.pause`, cancel its `taskId` with `activity.cancel`, and poll `faces.scanStatus`",
            super::always,
            imp::maintain
        ),
        cmd!(
            query "faces.scanStatus",
            "Face Maintenance Status",
            [],
            None,
            "{} → {state: idle | scanning | cancelling | complete | cancelled, mode, scope, done, total, pendingPhotos, failed, taskId}: status of the separate folder/selection maintenance scan, not the continuous library indexer",
            super::always,
            imp::maintenance_status
        ),
        cmd!(
            "faces.suggest",
            "Suggest Names for Faces",
            [],
            None,
            "{ids?, threshold?, margin?, topFaces?: 1..8 | null, budgetMs?} → {topFaces, photos: [{id, faces: [{index, suggestion: {name, score, runnerUp} | null, candidates: [{name, score}]}]}]}: who the unnamed faces in these photos might be, from the faces already named in the library. Only suggestions: nothing is named",
            super::always,
            imp::suggest
        ),
        cmd!(
            query "faces.pump",
            "Index Faces in the Background",
            [],
            None,
            "{pace?: pause | light | normal | full} → {active, inFlight, pendingPhotos, indexedFaces, searchedPhotos, workers}: cheap to call every frame: takes in what the background workers have finished and gives them the next photos, as many at once as the pace allows (none when paused, one when light, half the machine's threads when normal, four fifths when full); inactive until face recognition is on in Settings with a model chosen",
            super::always,
            imp::pump
        ),
        cmd!(
            query "faces.status",
            "Face Scan Status",
            [],
            None,
            "{} → {enabled, paused, state: off | idle | indexing | paused | complete, done, total, pendingPhotos, indexedFaces, searchedPhotos}: how far the background face scan is (`done` of `total`: photos since it last had nothing to do)",
            super::always,
            imp::status
        ),
        cmd!(
            "faces.pause",
            "Pause Face Scan",
            [],
            None,
            "{} → {enabled, paused}: pause the background face scan (what is running finishes, nothing new starts); it stays paused across restarts until `faces.resume`",
            super::always,
            |s, _| super::face_models::set_paused(s, "faces.pause", true)
        ),
        cmd!(
            "faces.resume",
            "Resume Face Scan",
            [],
            None,
            "{} → {enabled, paused}: resume a paused background face scan",
            super::always,
            |s, _| super::face_models::set_paused(s, "faces.resume", false)
        ),
        cmd!(
            "faces.clearIndex",
            "Clear Face Index",
            [],
            None,
            "{} → as faces.status: drop what the face scan learned for this library (every face embedding, and which photos were searched) and pause the scan. Faces already in the catalog, named or found, stay. Destructive: frontends ask first",
            super::always,
            imp::clear
        ),
        cmd!(
            "faces.restart",
            "Restart Face Scan",
            [],
            None,
            "{} → as faces.status: drop the face index (as faces.clearIndex) and scan the library again from the start (resumes a paused scan)",
            super::always,
            imp::restart
        ),
        cmd!(
            "faces.reset",
            "Reset All Face Data",
            [],
            None,
            "{confirmed: true} → {reset, paused, photos, faceRegions}: remove every Face and IgnoredFace region and its name, clear suggestion decisions and stored face examples, delete all recognition indexes, and pause the continuous scan so the clean slate stays empty. Other region kinds and people-role bindings are preserved. Destructive; callers must ask for confirmation",
            super::always,
            imp::reset
        ),
        cmd!(
            "faces.evaluate",
            "Evaluate Face Recognition",
            [],
            None,
            "{budgetMs?: 0, burstSecs?: 5, margin?, thresholds?: [..], topFaces?: 1..8 | null, scope?: \"visible\"} → {queries, top1, topFaces, sweep: [{threshold, suggested, correct, precision, recall}]}: leave-one-out over your named faces: each is ranked against the others (ignoring shots within burstSecs of it) to show how well the chosen model recognises people in *your* photos and which threshold to trust. Faces not yet embedded are done within budgetMs (`faces.index` does them all); counts only",
            super::always,
            imp::evaluate
        ),
        cmd!(
            query "faces.unnamed",
            "Unnamed Faces",
            [],
            None,
            "{limit?: 600, scope?: \"filter\", bySuggestion?: false, budgetMs?: 15} → {total, faces: [{photo, index, rect, view, suggestion: {name, score} | null, similarity: {name, score} | null}], ordered, ready, rejected, scan: {total, judged, complete}, groups?}: the faces nobody has named (with `scope: \"filter\"`, only in the photos the library shows: the sidebar's source and the filter, any person filter aside); with a recognition model chosen, look-alikes are next to each other and a face the named ones recognise carries the name they suggest. `similarity` is the best confirmed-person match, including below-threshold matches. Every unnamed face with an embedding is judged against the named ones, `budgetMs` of that per call at most, so a suggestion does not depend on the page a face falls on: `scan` says how many of the library's faces were judged, and whether all of them were (`budgetMs: 0` only reports). With `bySuggestion: true`, `groups` lists every suggested face in the scope under the person suggested ([{name, total, faces: [{photo, index, rect, view, score}]}], the most faces first), not only the listed page's. `view` is the box to show the face by. Name them with `faces.nameFaces`, ignore them with `faces.ignore`",
            super::always,
            unnamed
        ),
        cmd!(
            "faces.nameFaces",
            "Name Faces",
            [],
            None,
            "{faces: [{photo, index}], name} → {named, photos}: name (or, with a null or empty name, clear) many face regions as one undoable step; the sidecar is not touched",
            super::always,
            name_faces
        ),
        cmd!(
            "faces.confirmSuggestions",
            "Confirm Suggested Faces",
            [],
            None,
            "{faces: [{photo, index, rect?}]} → {confirmed, suggested, kept}: the user agreed with what the named faces suggest for these unnamed faces: each is named the person suggested for it now (a stale list never names a face something it was not seen to look like), one undo step, catalog only; what was decided is kept in people.json (people.forgetAll empties that)",
            super::always,
            confirm_suggestions
        ),
        cmd!(
            "faces.rejectSuggestions",
            "Reject Suggested Faces",
            [],
            None,
            "{faces: [{photo, index, rect?}]} → {rejected, faces}: the user said a suggested face is not the person it was suggested as: the face stays unnamed and is not ignored, and that person is never suggested for it again; kept in people.json (people.forgetAll empties that)",
            super::always,
            reject_suggestions
        ),
        cmd!(
            "faces.undoSuggestionDecision",
            "Undo Face Decision",
            [],
            None,
            "{faces: [{photo, index}]} → {removed, faces}: forget what was decided about these faces, so a rejected person is suggested again and the face can be reviewed afresh; nothing on a photo changes",
            super::always,
            undo_suggestion_decision
        ),
        cmd!(
            query "faces.suggestionDecisions",
            "Face Decisions",
            [],
            None,
            "{photo?, limit?: 2000, after?} → {total, decisions: [{photo, index, person, verdict: \"rejected\" | \"confirmed\", decided}]}: what was decided about suggested faces (faces.rejectSuggestions / faces.confirmSuggestions), newest first, a page of at most `limit` (2000) after `after` of the last page; `total` counts them all",
            super::always,
            suggestion_decisions
        ),
        cmd!(
            query "faces.person",
            "Faces of a Person",
            [],
            None,
            "{name, more?: 120, scope?: \"filter\"} → {name, total, confirmed: [{photo, index, rect}], more: [{photo, index, rect, score}], ready, pendingPhotos}: the faces named `name` across the library, and the unnamed faces that look most like them (best first, already embedded ones only, once a recognition model is chosen); confirm one with `faces.setName`. `scope: \"filter\"` shows only the faces in the photos the library shows (the sidebar's source and the filter, any person filter aside); `total` still counts the whole library",
            super::always,
            person
        ),
        cmd!(
            "faces.setName",
            "Name Face",
            [],
            None,
            "{id?, index, name}: name a face region (null or empty clears it); undoable; the sidecar is not touched",
            super::always,
            set_name
        ),
        cmd!(
            "faces.renamePerson",
            "Rename Person",
            [],
            None,
            "{from, to, merge?: bool} → {faces, photos, from, to, merged}: give every in-library face of `from` the name `to` (trimmed, any capitalization), one undo step; refused when `to` is already someone else unless `merge` is true, which combines the two; the sidecar is not touched",
            super::always,
            rename_person
        ),
    ]
}
