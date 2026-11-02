//! Capture One catalog import (library data).
//!
//! A Capture One catalog (`Name.cocatalog`: a folder, a package on macOS) keeps its library in
//! `Name.cocatalogdb`, an SQLite database. This module reads it with the same read-only page reader
//! as the Lightroom import and turns it into the same [`CatalogImport`] model, so everything after the
//! read (the job, merge-or-new-library, one undo step, reimport without duplicates, the result window,
//! the archive) is shared with File ▸ Import Lightroom Catalog….
//!
//! What is read: originals (inside the catalog or referenced), variants (the first one is the photo,
//! the others become virtual copies), star ratings, colour tags, IPTC fields, keywords, and user albums
//! with the projects and groups around them. Develop settings are not read yet.
//!
//! Sources: own design, from the database's own table and column names (`sqlite_schema`) observed in
//! a catalog written by Capture One 15/16; no Capture One code or documentation was used.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use serde_json::{Value, json};

use crate::lightroom_catalog::{CatalogImport, CatalogPhoto, Original, Row, SkipReason, SkippedImage, ensure_read_active, open_catalog};

/// Deepest collection or keyword chain followed; deeper (or cyclic) chains are cut there.
const MAX_DEPTH: usize = 64;

/// Whether `path` is a Capture One catalog: its `.cocatalogdb` database, or a folder (a `.cocatalog`
/// package, or the plain folder Windows shows) that holds one.
pub fn is_capture_one(path: &Path) -> bool {
    database_file(path).is_some()
}

fn has_extension(path: &Path, ext: &str) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

/// The catalog database for `path`: the file itself, or the one `.cocatalogdb` in the folder (the one
/// named after the folder first, when there are several).
pub fn database_file(path: &Path) -> Option<PathBuf> {
    if has_extension(path, "cocatalogdb") {
        return path.is_file().then(|| path.to_path_buf());
    }
    if !path.is_dir() {
        return None;
    }
    let named = path.file_stem().map(|stem| path.join(format!("{}.cocatalogdb", stem.to_string_lossy())));
    if let Some(named) = named.filter(|p| p.is_file()) {
        return Some(named);
    }
    let mut found: Vec<PathBuf> = std::fs::read_dir(path)
        .ok()?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && has_extension(p, "cocatalogdb"))
        .collect();
    found.sort();
    found.into_iter().next()
}

fn text<'a>(r: &'a Row, k: &str) -> &'a str {
    r.get(k).and_then(Value::as_str).unwrap_or("")
}

fn int(r: &Row, k: &str) -> Option<i64> {
    let v = r.get(k)?;
    v.as_i64().or_else(|| v.as_f64().filter(|f| f.is_finite() && f.abs() < 9.0e15).map(|f| f.round() as i64))
}

fn number(r: &Row, k: &str) -> i64 {
    int(r, k).unwrap_or(0)
}

fn by_pk(rows: Vec<Row>) -> HashMap<i64, Row> {
    rows.into_iter().filter_map(|r| int(&r, "Z_PK").map(|pk| (pk, r))).collect()
}

/// Capture One's colour tags, by `ZCOLOR_TAG_INDEX`, as the library's label names. Orange (2) and pink
/// (6) have no label in the library.
fn colour_tag(index: i64) -> Result<&'static str, Option<&'static str>> {
    match index {
        1 => Ok("Red"),
        3 => Ok("Yellow"),
        4 => Ok("Green"),
        5 => Ok("Blue"),
        7 => Ok("Purple"),
        2 => Err(Some("orange")),
        6 => Err(Some("pink")),
        _ => Err(None),
    }
}

/// The folder an image's `ZPATHLOCATION` names, as a path string (it may be on another computer).
/// Relative locations are relative to the folder the catalog database is in.
fn location_path(loc: &Row, catalog_dir: &Path) -> Option<String> {
    let relative = text(loc, "ZRELATIVEPATH").replace(['\\', '/'], std::path::MAIN_SEPARATOR_STR);
    if number(loc, "ZISRELATIVE") != 0 {
        let mut dir = catalog_dir.to_path_buf();
        for part in relative.split(std::path::MAIN_SEPARATOR).filter(|p| !p.is_empty() && *p != ".") {
            if part == ".." {
                dir = dir.parent().map(Path::to_path_buf).unwrap_or(dir);
            } else {
                dir.push(part);
            }
        }
        return Some(dir.to_string_lossy().into());
    }
    // An absolute location: the root on the system this runs on first, then the other one, then the
    // volume (macOS).
    let roots = if cfg!(windows) { ["ZWINROOT", "ZMACROOT", "ZVOLUME"] } else { ["ZMACROOT", "ZWINROOT", "ZVOLUME"] };
    let root = roots.iter().map(|k| text(loc, k)).find(|r| !r.is_empty())?;
    let relative = relative.trim_start_matches(std::path::MAIN_SEPARATOR);
    Some(if relative.is_empty() { root.to_string() } else { Path::new(root).join(relative).to_string_lossy().into() })
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            // XML 1.0 has no other control characters
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}

/// IPTC fields of a variant as an XMP packet, which the sidecar reader merges like any other XMP.
fn iptc_xmp(meta: &dyn Fn(&str) -> Option<String>) -> String {
    let simple = [
        ("photoshop:Headline", "ZCONTENT_HEADLINE"),
        ("photoshop:CaptionWriter", "ZCONTENT_DESCRIPTIONWRITER"),
        ("photoshop:City", "ZIMAGE_CITY"),
        ("photoshop:State", "ZIMAGE_STATE"),
        ("photoshop:Country", "ZIMAGE_COUNTRY"),
        ("photoshop:Instructions", "ZSTATUS_INSTRUCTIONS"),
        ("photoshop:TransmissionReference", "ZSTATUS_JOBIDENTIFIER"),
        ("photoshop:Credit", "ZSTATUS_PROVIDER"),
        ("photoshop:Source", "ZSTATUS_SOURCE"),
        ("photoshop:Category", "ZCONTENT_CATEGORY"),
        ("photoshop:AuthorsPosition", "ZCONTACT_CREATORSTITLE"),
        ("Iptc4xmpCore:Location", "ZIMAGE_LOCATION"),
        ("Iptc4xmpCore:CountryCode", "ZIMAGE_ISOCOUNTRYCODE"),
        ("Iptc4xmpCore:IntellectualGenre", "ZIMAGE_INTELLECTUALGENRE"),
    ];
    let alt = [
        ("dc:title", "ZSTATUS_TITLE"),
        ("dc:description", "ZCONTENT_DESCRIPTION"),
        ("dc:rights", "ZSTATUS_COPYRIGHTNOTICE"),
        ("xmpRights:UsageTerms", "ZSTATUS_USAGETERMS"),
    ];
    let mut body = String::new();
    for (tag, col) in simple {
        if let Some(v) = meta(col) {
            body.push_str(&format!("<{tag}>{}</{tag}>", xml_escape(&v)));
        }
    }
    for (tag, col) in alt {
        if let Some(v) = meta(col) {
            body.push_str(&format!("<{tag}><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></{tag}>", xml_escape(&v)));
        }
    }
    if let Some(v) = meta("ZCONTACT_CREATOR") {
        let names: String =
            v.split([';', '\n']).map(str::trim).filter(|n| !n.is_empty()).map(|n| format!("<rdf:li>{}</rdf:li>", xml_escape(n))).collect();
        if !names.is_empty() {
            body.push_str(&format!("<dc:creator><rdf:Seq>{names}</rdf:Seq></dc:creator>"));
        }
    }
    if body.is_empty() {
        return String::new();
    }
    format!(
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
         <rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" \
         xmlns:photoshop=\"http://ns.adobe.com/photoshop/1.0/\" xmlns:Iptc4xmpCore=\"http://iptc.org/std/Iptc4xmpCore/1.0/xmlns/\" \
         xmlns:xmpRights=\"http://ns.adobe.com/xap/1.0/rights/\">{body}</rdf:Description></rdf:RDF></x:xmpmeta>"
    )
}

/// Keyword paths (`a|b|c`) for the names in a variant's keyword field. A name that is one keyword of
/// the catalog's keyword tree gets its parents; anything else is kept as written.
fn keyword_paths(field: &str, tree: &HashMap<i64, Row>, by_name: &HashMap<String, Vec<i64>>) -> Vec<String> {
    let mut out = Vec::new();
    for name in field.split([',', ';', '\n', '\r']).map(str::trim).filter(|n| !n.is_empty()) {
        if name.contains('|') {
            out.push(name.split('|').map(str::trim).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("|"));
            continue;
        }
        let path = match by_name.get(&name.to_lowercase()).map(Vec::as_slice) {
            Some([only]) => {
                let mut parts = Vec::new();
                let mut seen = HashSet::new();
                let mut at = *only;
                while let Some(k) = tree.get(&at) {
                    if parts.len() >= MAX_DEPTH || !seen.insert(at) {
                        break;
                    }
                    let part = text(k, "ZNAME").trim();
                    if !part.is_empty() {
                        parts.push(part.to_string());
                    }
                    at = number(k, "ZPARENT");
                }
                parts.reverse();
                parts.join("|")
            }
            _ => name.to_string(),
        };
        if !path.is_empty() && !out.contains(&path) {
            out.push(path);
        }
    }
    out
}

/// Read a Capture One catalog for import, counting the tables read in `total` / `done` and stopping
/// when `cancel` is set.
pub fn read_with_progress(path: &Path, cancel: &AtomicBool, total: &AtomicUsize, done: &AtomicUsize) -> Result<CatalogImport, String> {
    total.store(11, Ordering::Relaxed);
    done.store(0, Ordering::Relaxed);
    let db_path = database_file(path).ok_or_else(|| format!("{}: no Capture One catalog database (.cocatalogdb) found", path.display()))?;
    let catalog_dir = db_path.parent().map(Path::to_path_buf).unwrap_or_default();
    let (reader, guard) = open_catalog(&db_path, cancel)?;
    let table = |name: &str, required: bool| -> Result<Vec<Row>, String> {
        ensure_read_active(cancel)?;
        let rows = reader.table(name, required, cancel).map_err(|e| e.replace("Lightroom", "Capture One"))?;
        done.fetch_add(1, Ordering::Relaxed);
        Ok(rows)
    };
    let mut warnings = Vec::new();
    let entities: HashMap<i64, String> =
        table("ZENTITIES", false)?.iter().filter_map(|r| Some((int(r, "Z_ENT")?, text(r, "ZNAME").to_string()))).collect();
    let images = table("ZIMAGE", true)?;
    let locations = by_pk(table("ZPATHLOCATION", true)?);
    let variants = table("ZVARIANT", true)?;
    let metadata: HashMap<i64, Row> = table("ZVARIANTMETADATA", false)?.into_iter().filter_map(|r| Some((int(&r, "ZLAYER")?, r))).collect();
    let keyword_tree = by_pk(table("ZKEYWORD", false)?);
    let collections = table("ZCOLLECTION", false)?;
    let image_members = table("ZIMAGEINCOLLECTION", false)?;
    let variant_members = table("ZVARIANTINCOLLECTION", false)?;
    let positions = table("ZIMAGEINCOLLECTIONPROPERTIES", false)?;
    let documents = table("ZDOCUMENTCONTENT", false)?;
    let mut keyword_names: HashMap<String, Vec<i64>> = HashMap::new();
    for (pk, k) in &keyword_tree {
        keyword_names.entry(text(k, "ZNAME").trim().to_lowercase()).or_default().push(*pk);
    }

    // ---- photos: one per variant; the first variant of an image is the photo, the others its virtual copies
    let mut by_image: BTreeMap<i64, Vec<&Row>> = BTreeMap::new();
    for v in &variants {
        by_image.entry(number(v, "ZIMAGE")).or_default().push(v);
    }
    for list in by_image.values_mut() {
        list.sort_by_key(|v| (number(v, "ZINDEX"), number(v, "Z_PK")));
    }
    let mut primary: HashMap<i64, i64> = HashMap::new();
    let mut photos = Vec::new();
    let mut skipped = Vec::new();
    let mut trashed = 0usize;
    let mut unlabelled: BTreeMap<&'static str, usize> = BTreeMap::new();
    for image in &images {
        ensure_read_active(cancel)?;
        let Some(image_pk) = int(image, "Z_PK") else { continue };
        if number(image, "ZISTRASHED") != 0 {
            trashed += 1;
            continue;
        }
        let file_name = text(image, "ZIMAGEFILENAME");
        let folder = locations.get(&number(image, "ZIMAGELOCATION")).and_then(|loc| location_path(loc, &catalog_dir));
        let Some(folder) = folder else {
            skipped.push(SkippedImage { image: image_pk, reason: SkipReason::MissingFolder });
            continue;
        };
        if file_name.is_empty() || file_name.contains(['/', '\\']) {
            skipped.push(SkippedImage { image: image_pk, reason: SkipReason::IncompletePath });
            continue;
        }
        let stored = Path::new(&folder).join(file_name).to_string_lossy().to_string();
        let (path, original) = match crate::doc_path::resolve(&stored, Some(&db_path)) {
            Ok(found) => (found.to_string_lossy().to_string(), Original::Local),
            Err(crate::doc_path::NotFollowed::NotHere) => (stored, Original::NotHere),
            Err(crate::doc_path::NotFollowed::Refused(why)) => (stored, Original::Refused(why.to_string())),
        };
        let format = Path::new(file_name).extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default();
        let width = int(image, "ZPIXELWIDTH").or_else(|| int(image, "ZWIDTH")).unwrap_or(0).max(0);
        let height = int(image, "ZPIXELHEIGHT").or_else(|| int(image, "ZHEIGHT")).unwrap_or(0).max(0);
        // The file's own capture time is read when it is added; an offline original has only the catalog's.
        let captured = match (&original, image.get("ZEXP_DATE").and_then(Value::as_f64)) {
            (Original::Local, _) | (_, None) => String::new(),
            (_, Some(seconds)) => chrono::DateTime::from_timestamp(seconds.trunc() as i64, 0)
                .map(|t| t.naive_utc().format("%Y-%m-%dT%H:%M:%S").to_string())
                .unwrap_or_default(),
        };
        let image_variants = by_image.get(&image_pk).cloned().unwrap_or_default();
        if image_variants.is_empty() {
            skipped.push(SkippedImage { image: image_pk, reason: SkipReason::MissingFile });
            continue;
        }
        let mut master = 0;
        for (n, variant) in image_variants.iter().enumerate() {
            let Some(variant_pk) = int(variant, "Z_PK") else { continue };
            // Metadata hangs off a layer: the combined layer is what Capture One shows, the
            // adjustment and default layers fill in what it leaves empty.
            let chain: Vec<&Row> =
                ["ZCOMBINEDSETTINGS", "ZADJUSTMENTLAYER", "ZDEFAULTLAYER"].iter().filter_map(|k| metadata.get(&number(variant, k))).collect();
            let meta = |col: &str| -> Option<String> {
                chain.iter().filter_map(|m| m.get(col)).find_map(|v| match v {
                    Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
                    Value::Number(n) => Some(n.to_string()),
                    _ => None,
                })
            };
            let meta_int = |col: &str| chain.iter().find_map(|m| int(m, col));
            let stars = meta_int("ZBASIC_RATING").unwrap_or(0);
            let label = match meta_int("ZCOLOR_TAG_INDEX").map(colour_tag) {
                Some(Ok(name)) => name,
                Some(Err(Some(colour))) => {
                    *unlabelled.entry(colour).or_default() += 1;
                    ""
                }
                _ => "",
            };
            let mut row = json!({
                "id_local": variant_pk,
                "masterImage": master,
                "fileFormat": format,
                "fileWidth": width,
                "fileHeight": height,
                "rating": stars.clamp(0, 5),
                "pick": if stars < 0 { -1 } else { 0 },
                "colorLabels": label,
                "captureTime": captured,
            });
            if master != 0
                && let Some(obj) = row.as_object_mut()
            {
                obj.insert("copyName".into(), json!(format!("Variant {}", n + 1)));
            }
            let image_row: Row = serde_json::from_value(row).map_err(|e| e.to_string())?;
            let uuid = text(variant, "ZVARIANTUUID");
            photos.push(CatalogPhoto {
                source_id: variant_pk,
                uuid: if uuid.is_empty() { String::new() } else { format!("c1:{uuid}") },
                path: path.clone(),
                image: image_row,
                settings: String::new(),
                xmp: iptc_xmp(&meta),
                keywords: meta("ZCONTENT_KEYWORDS").map(|k| keyword_paths(&k, &keyword_tree, &keyword_names)).unwrap_or_default(),
                history: Vec::new(),
                snapshots: Vec::new(),
                original: original.clone(),
            });
            if master == 0 {
                master = variant_pk;
                primary.insert(image_pk, variant_pk);
            }
        }
    }
    if trashed > 0 {
        warnings.push(format!("{trashed} image(s) in Capture One's catalog trash were left out"));
    }
    for (colour, count) in unlabelled {
        warnings.push(format!("{count} photo(s) had Capture One's {colour} colour tag, which has no label here; imported without a label"));
    }

    // ---- albums, with the projects and groups that hold them
    let kinds: HashMap<i64, String> = collections
        .iter()
        .filter_map(|c| {
            let ent = int(c, "Z_ENT")?;
            Some((int(c, "Z_PK")?, entities.get(&ent).cloned().unwrap_or_default()))
        })
        .collect();
    let roots: HashSet<i64> = documents.iter().filter_map(|d| int(d, "ZROOTCOLLECTION")).collect();
    let by_id: HashMap<i64, &Row> = collections.iter().filter_map(|c| Some((int(c, "Z_PK")?, c))).collect();
    let is_group = |id: i64| matches!(kinds.get(&id).map(String::as_str), Some("ProjectCollection" | "Collection"));
    // A collection is imported when it is an album or a group and every collection above it, up to
    // the catalog's root, is a group: albums under Recent Imports and other system folders stay out.
    let wanted = |id: i64| -> bool {
        let mut at = id;
        let mut seen = HashSet::new();
        loop {
            if roots.contains(&at) || at <= 0 {
                return at != id;
            }
            if seen.len() >= MAX_DEPTH || !seen.insert(at) {
                return false;
            }
            let ok = if at == id { is_group(at) || kinds.get(&at).is_some_and(|k| k == "AlbumCollection") } else { is_group(at) };
            if !ok {
                return false;
            }
            at = by_id.get(&at).map_or(0, |c| number(c, "ZPARENT"));
        }
    };
    let mut out_collections = Vec::new();
    let mut kept = HashSet::new();
    for c in &collections {
        let Some(id) = int(c, "Z_PK") else { continue };
        let kind = kinds.get(&id).map(String::as_str).unwrap_or("");
        if kind == "SmartCollection" {
            // only the user's own (under the root or their projects and groups), not the built-in ones
            let parent = number(c, "ZPARENT");
            if roots.contains(&parent) || (is_group(parent) && wanted(parent)) {
                warnings.push(format!("smart album “{}”: its search rules aren't read yet; not imported", text(c, "ZNAME")));
            }
            continue;
        }
        if !wanted(id) {
            continue;
        }
        let parent = number(c, "ZPARENT");
        let parent = if roots.contains(&parent) || parent < 0 { 0 } else { parent };
        let creation = if is_group(id) { "com.adobe.ag.library.group" } else { "com.adobe.ag.library.collection" };
        let name = text(c, "ZNAME").trim();
        let name = if name.is_empty() { "Untitled" } else { name };
        out_collections.push(
            serde_json::from_value::<Row>(json!({"id_local": id, "name": name, "parent": parent, "creationId": creation}))
                .map_err(|e| e.to_string())?,
        );
        kept.insert(id);
    }
    let position: HashMap<(i64, i64), i64> =
        positions.iter().filter_map(|p| Some(((number(p, "ZCOLLECTION"), number(p, "ZIMAGE")), int(p, "ZMANUALSORTINDEX")?))).collect();
    let mut members = Vec::new();
    let mut seen_members = HashSet::new();
    let member = |collection: i64, variant: i64, pos: i64| -> Result<Row, String> {
        // positions sort as text: pad so 10 follows 9
        serde_json::from_value(json!({"collection": collection, "image": variant, "positionInCollection": format!("{:020}", pos.max(0))}))
            .map_err(|e| e.to_string())
    };
    for m in &image_members {
        let (collection, image) = (number(m, "ZCOLLECTION"), number(m, "ZIMAGE"));
        if let (true, Some(variant)) = (kept.contains(&collection), primary.get(&image))
            && seen_members.insert((collection, *variant))
        {
            members.push(member(collection, *variant, position.get(&(collection, image)).copied().unwrap_or(number(m, "Z_PK")))?);
        }
    }
    let photo_ids: HashSet<i64> = photos.iter().map(|p| p.source_id).collect();
    for m in &variant_members {
        let (collection, variant) = (number(m, "ZCOLLECTION"), number(m, "ZVARIANT"));
        if kept.contains(&collection) && photo_ids.contains(&variant) && seen_members.insert((collection, variant)) {
            members.push(member(collection, variant, number(m, "Z_PK"))?);
        }
    }
    guard.verify().map_err(|e| e.replace("Lightroom", "Capture One"))?;
    done.store(total.load(Ordering::Relaxed), Ordering::Relaxed);
    Ok(CatalogImport {
        source: db_path.to_string_lossy().into(),
        photos,
        collections: out_collections,
        members,
        collection_content: Vec::new(),
        stacks: Vec::new(),
        warnings,
        skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_tags_map_to_library_labels_or_say_why_not() {
        assert_eq!(colour_tag(1), Ok("Red"));
        assert_eq!(colour_tag(7), Ok("Purple"));
        assert_eq!(colour_tag(2), Err(Some("orange")));
        assert_eq!(colour_tag(0), Err(None));
        assert_eq!(colour_tag(-5), Err(None));
    }

    #[test]
    fn relative_locations_resolve_against_the_catalog_folder() {
        let dir = Path::new("/photos/My Catalog");
        let loc: Row = serde_json::from_value(json!({"ZRELATIVEPATH": "./Originals", "ZISRELATIVE": 1})).unwrap();
        assert_eq!(location_path(&loc, dir), Some(dir.join("Originals").to_string_lossy().into()));
        let up: Row = serde_json::from_value(json!({"ZRELATIVEPATH": "..\\..\\Shoots", "ZISRELATIVE": 1})).unwrap();
        assert_eq!(location_path(&up, dir), Some(Path::new("/Shoots").to_string_lossy().into()));
        let nothing: Row = serde_json::from_value(json!({"ZISRELATIVE": 0})).unwrap();
        assert_eq!(location_path(&nothing, dir), None);
    }

    #[test]
    fn keywords_get_their_parents_from_the_tree() {
        let tree = by_pk(vec![
            serde_json::from_value(json!({"Z_PK": 1, "ZNAME": "Nature", "ZPARENT": 0})).unwrap(),
            serde_json::from_value(json!({"Z_PK": 2, "ZNAME": "Trees", "ZPARENT": 1})).unwrap(),
            // a cycle must not hang
            serde_json::from_value(json!({"Z_PK": 3, "ZNAME": "Loop", "ZPARENT": 3})).unwrap(),
        ]);
        let mut names: HashMap<String, Vec<i64>> = HashMap::new();
        for (pk, k) in &tree {
            names.entry(text(k, "ZNAME").to_lowercase()).or_default().push(*pk);
        }
        assert_eq!(keyword_paths("trees, Rivers; Loop", &tree, &names), vec!["Nature|Trees", "Rivers", "Loop"]);
        assert_eq!(keyword_paths("a | b|", &tree, &names), vec!["a|b"]);
    }

    #[test]
    fn iptc_fields_become_escaped_xmp() {
        let x = iptc_xmp(&|c: &str| match c {
            "ZSTATUS_TITLE" => Some("Owls <night> & more".into()),
            "ZCONTACT_CREATOR" => Some("A; B".into()),
            _ => None,
        });
        assert!(x.contains("Owls &lt;night&gt; &amp; more"));
        assert!(x.contains("<rdf:li>A</rdf:li><rdf:li>B</rdf:li>"));
        assert!(iptc_xmp(&|_| None).is_empty());
    }

    #[test]
    fn hostile_files_are_errors_not_panics() {
        let dir = std::env::temp_dir().join(format!("c1-hostile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (n, bytes) in [b"".as_slice(), b"SQLite format 3\0".as_slice(), &[0xffu8; 4096]].iter().enumerate() {
            let p = dir.join(format!("bad{n}.cocatalogdb"));
            std::fs::write(&p, bytes).unwrap();
            assert!(is_capture_one(&p));
            let cancel = AtomicBool::new(false);
            assert!(read_with_progress(&p, &cancel, &AtomicUsize::new(0), &AtomicUsize::new(0)).is_err());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
