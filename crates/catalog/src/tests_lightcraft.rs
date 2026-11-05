//! Import LightCraft Library (test notes 2026-11-05, 19:33): a LightCraft library (`lightcraft-catalog` v10) is read
//! with its edits, masks, picks and collections, converted to this app's records, and never changed. The records
//! below are what LightCraft itself writes (made by its own catalog code for this test, a synthetic library).

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::lightcraft::{self, LightcraftReport};
use crate::{ColorLabel, Flag, PhotoId, transfer};

/// A photo's develop settings as LightCraft writes them: exposure 0.7 and three masks (a radial gradient, a
/// remembered "Wedding dress" with its segmentation, the subject), the dress linked across photos.
const FORK_DEVELOP: &str = r#"{"version":1,"process":3,"profile":{"id":"lc.color","amount":100.0},"treatment":"color","wb":{"mode":"asShot","temp":6500.0,"tint":0.0},"light":{"exposure":0.7,"contrast":0.0,"highlights":0.0,"shadows":0.0,"whites":0.0,"blacks":0.0},"curve":{"highlights":0.0,"lights":0.0,"darks":0.0,"shadows":0.0,"split_shadows":25.0,"split_mid":50.0,"split_highlights":75.0,"master":[],"red":[],"green":[],"blue":[],"refine_saturation":100.0},"color":{"vibrance":0.0,"saturation":0.0},"mixer":{"red":{"hue":0.0,"sat":0.0,"lum":0.0},"orange":{"hue":0.0,"sat":0.0,"lum":0.0},"yellow":{"hue":0.0,"sat":0.0,"lum":0.0},"green":{"hue":0.0,"sat":0.0,"lum":0.0},"aqua":{"hue":0.0,"sat":0.0,"lum":0.0},"blue":{"hue":0.0,"sat":0.0,"lum":0.0},"purple":{"hue":0.0,"sat":0.0,"lum":0.0},"magenta":{"hue":0.0,"sat":0.0,"lum":0.0}},"point_colors":[],"bw_mix":{"red":0.0,"orange":0.0,"yellow":0.0,"green":0.0,"aqua":0.0,"blue":0.0,"purple":0.0,"magenta":0.0},"grading":{"shadows":{"hue":0.0,"sat":0.0,"lum":0.0},"midtones":{"hue":0.0,"sat":0.0,"lum":0.0},"highlights":{"hue":0.0,"sat":0.0,"lum":0.0},"global":{"hue":0.0,"sat":0.0,"lum":0.0},"blending":50.0,"balance":0.0},"effects":{"texture":0.0,"clarity":0.0,"dehaze":0.0},"vignette":{"amount":0.0,"midpoint":50.0,"roundness":0.0,"feather":50.0,"highlights":0.0,"style":"highlightPriority"},"grain":{"amount":0.0,"size":25.0,"roughness":50.0,"seed":0,"color":0.0,"size_variation":40.0,"clumping":30.0},"detail":{"sharpen_amount":0.0,"sharpen_radius":1.0,"sharpen_detail":25.0,"sharpen_masking":0.0,"nr_luminance":0.0,"nr_detail":50.0,"nr_contrast":0.0,"nr_color":0.0,"nr_color_detail":50.0,"nr_color_smoothness":50.0},"optics":{"remove_ca":false,"lens_profile":false,"profile_distortion":100.0,"profile_vignetting":100.0,"defringe_purple_amount":0.0,"defringe_purple_hue_lo":30.0,"defringe_purple_hue_hi":70.0,"defringe_green_amount":0.0,"defringe_green_hue_lo":40.0,"defringe_green_hue_hi":60.0,"distortion":0.0,"vignetting":0.0,"vignetting_midpoint":50.0,"ca_red":0.0,"ca_blue":0.0},"geometry":{"upright":"off","guides":[],"upright_transform":null,"vertical":0.0,"horizontal":0.0,"rotate":0.0,"aspect":0.0,"scale":100.0,"offset_x":0.0,"offset_y":0.0,"constrain_crop":false},"crop":{"geometry":{"rect":{"x0":0.0,"y0":0.0,"x1":1.0,"y1":1.0},"angle":0.0},"aspect":null,"flip_h":false,"flip_v":false},"orientation":"Normal","masks":[{"id":1,"name":"Glow","visible":true,"invert":false,"components":[{"op":"add","invert":false,"shape":{"kind":"radial","center":{"x":0.5,"y":0.4},"rx":0.2,"ry":0.1,"angle":0.0,"feather":50.0,"invert":false}}],"adjust":{"temp":0.0,"tint":0.0,"exposure":0.5,"contrast":0.0,"highlights":0.0,"shadows":0.0,"whites":0.0,"blacks":0.0,"texture":0.0,"clarity":0.0,"dehaze":0.0,"hue":0.0,"saturation":0.0,"sharpness":0.0,"noise":0.0,"moire":0.0,"defringe":0.0,"color_hue":0.0,"color_sat":0.0,"amount":100.0}},{"id":2,"name":"Dress","visible":true,"invert":false,"components":[{"op":"add","invert":false,"shape":{"kind":"concept","name":"Wedding dress","seg":{"side":4,"data":"k5BABQA="}}}],"adjust":{"temp":0.0,"tint":0.0,"exposure":-0.3,"contrast":0.0,"highlights":0.0,"shadows":0.0,"whites":0.0,"blacks":0.0,"texture":0.0,"clarity":0.0,"dehaze":0.0,"hue":0.0,"saturation":0.0,"sharpness":0.0,"noise":0.0,"moire":0.0,"defringe":0.0,"color_hue":0.0,"color_sat":0.0,"amount":100.0},"link":"L1"},{"id":3,"name":"Couple","visible":true,"invert":false,"components":[{"op":"add","invert":false,"shape":{"kind":"subject","seg":{"side":4,"data":"k5BABQA="}}}],"adjust":{"temp":0.0,"tint":0.0,"exposure":0.0,"contrast":10.0,"highlights":0.0,"shadows":0.0,"whites":0.0,"blacks":0.0,"texture":0.0,"clarity":0.0,"dehaze":0.0,"hue":0.0,"saturation":0.0,"sharpness":0.0,"noise":0.0,"moire":0.0,"defringe":0.0,"color_hue":0.0,"color_sat":0.0,"amount":100.0}}],"spots":[],"red_eye":[],"lens_blur":{"enabled":false,"amount":0.0,"focal_distance":0.0,"focal_range":0.0},"enhance":{"denoise":0.0,"raw_details":false,"super_resolution":false},"calibration":{"shadows_tint":0.0,"red_hue":0.0,"red_sat":0.0,"green_hue":0.0,"green_sat":0.0,"blue_hue":0.0,"blue_sat":0.0},"disabled_sections":[]}"#;
/// The first photo's record (without its settings), as LightCraft writes it: the serial number is `serial`.
const FORK_PHOTO: &str = r#"{"id":1,"source":{"type":"file","path":"D:/Shoots/Event/wedding-0001.cr3"},"file_name":"wedding-0001.cr3","kind":"image","format":"CR3","width":6000,"height":4000,"file_size":0,"captured":null,"imported":"2026-10-03T10:00:00Z","meta":{"camera":"Canon EOS R5","serial":"012345678901","lens":"RF24-70mm F2.8 L IS USM","focal_mm":null,"aperture":null,"shutter":"","iso":800,"lens_range":[24.0,70.0],"location":"","city":"","state":"","country":"","gps":null,"title":"","caption":"","alt_text":"","extended_description":"","copyright":"","creator":"","keywords":[]},"rating":4,"flag":"pick","label":null,"edited":"2026-10-04T09:00:00Z","versions":[],"history":[],"deleted":false,"duration":null,"as_shot_wb":null}"#;

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lcc-lightcraft-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// `catalog.snap` as LightCraft writes it: the format first (what a quick look at its start reads).
fn snapshot(version: u32, catalog: &Value) -> String {
    format!("{{\"format\":\"lightcraft-catalog\",\"version\":{version},\"seq\":6,\"catalog\":{catalog}}}\n")
}

/// One log line as LightCraft writes it.
fn record(seq: u64, op: &Value) -> String {
    let body = op.to_string();
    format!("{{\"seq\":{seq},\"crc\":{},\"op\":{body}}}\n", crc32fast::hash(body.as_bytes()))
}

fn develop() -> Value {
    serde_json::from_str(FORK_DEVELOP).unwrap()
}

/// A LightCraft library: two photos (the first picked, 4 stars, edited with masks) and a collection in the snapshot
/// (`version`); after it, in the log, the second photo rejected, edited, given its import state and a red label.
fn write_library(dir: &Path, version: u32) {
    let mut p1: Value = serde_json::from_str(FORK_PHOTO).unwrap();
    p1["develop"] = develop();
    let mut p2 = p1.clone();
    p2["id"] = json!(2);
    p2["file_name"] = json!("wedding-0002.cr3");
    p2["source"]["path"] = json!("D:/Shoots/Event/wedding-0002.cr3");
    p2["rating"] = json!(0);
    p2["flag"] = json!("none");
    p2["edited"] = Value::Null;
    p2["develop"]["light"]["exposure"] = json!(0.0);
    p2["develop"]["masks"] = json!([]);
    let catalog = json!({
        "photos": {"1": p1, "2": p2},
        "albums": {"1": {"id": 1, "name": "Ceremony", "parent": null, "folder": false, "photos": [1, 2], "cover": null}},
        "next_photo": 3, "next_album": 2, "next_stack": 1
    });
    std::fs::write(dir.join("catalog.snap"), snapshot(version, &catalog)).unwrap();
    let mut edited = develop();
    edited["light"]["exposure"] = json!(1.1);
    let mut state = develop();
    state["light"]["exposure"] = json!(0.0);
    state["masks"] = json!([]);
    let log = [
        // already in the snapshot: skipped
        record(6, &json!({"op": "setRating", "id": 1, "rating": 1})),
        record(7, &json!({"op": "setFlag", "id": 2, "flag": "reject"})),
        record(8, &json!({"op": "setDevelop", "id": 2, "settings": edited, "label": "Exposure", "edited": "2026-10-04T09:05:00Z"})),
        record(9, &json!({"op": "setImportState", "id": 2, "state": state})),
        record(10, &json!({"op": "setLabel", "id": 2, "label": "red"})),
    ]
    .concat();
    std::fs::write(dir.join("catalog.log"), log).unwrap();
    std::fs::write(dir.join("catalog.lock"), b"held by LightCraft").unwrap();
}

/// Every file in `dir`, with its bytes.
fn files(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> =
        std::fs::read_dir(dir).unwrap().flatten().map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap())).collect();
    out.sort();
    out
}

#[test]
fn a_lightcraft_library_comes_in_with_its_edits_masks_and_picks_and_is_never_changed() {
    let dir = temp("read");
    write_library(&dir, 10);
    let before = files(&dir);
    assert!(lightcraft::is_lightcraft(&dir));
    let (cat, report) = lightcraft::load(&dir).unwrap();
    assert_eq!(report, LightcraftReport { version: Some(10), photos: 2, replayed: 4, ..Default::default() }, "every record read, none left out");

    let p1 = cat.photo(PhotoId(1)).unwrap();
    assert_eq!((p1.flag, p1.rating), (Flag::Pick, 4), "the pick and the stars (the stale log record skipped)");
    assert_eq!(p1.develop.light.exposure, 0.7);
    assert_eq!(p1.develop.masks.len(), 3, "every mask: gradient, remembered object, subject");
    assert_eq!(p1.develop.masks[1].name, "Dress");
    assert_eq!(p1.develop.masks[1].link, "L1", "linked masks stay linked");
    assert!(
        matches!(&p1.develop.masks[1].components[0].shape, lcc_develop::MaskShape::Concept { name, seg: Some(_) } if name == "Wedding dress"),
        "the remembered object keeps its segmentation: {:?}",
        p1.develop.masks[1].components[0].shape
    );
    assert_eq!(p1.develop.masks[1].adjust.exposure, -0.3);
    assert_eq!(p1.meta.camera_serial, "012345678901", "LightCraft's `serial`");
    assert_eq!(p1.meta.lens_range, Some((24.0, 70.0)));
    assert_eq!(p1.edited.as_deref(), Some("2026-10-04T09:00:00Z"));

    let p2 = cat.photo(PhotoId(2)).unwrap();
    assert_eq!(p2.flag, Flag::Reject, "the reject from the log");
    assert_eq!(p2.label, Some(ColorLabel::Red));
    assert_eq!(p2.develop.light.exposure, 1.1, "the edit from the log");
    assert!(p2.import_state.as_ref().is_some_and(|s| s.light.exposure == 0.0), "the import state from the log");

    let album = cat.albums().find(|a| a.name == "Ceremony").expect("the collection");
    assert_eq!(album.photos, vec![PhotoId(1), PhotoId(2)]);

    // Import from Another Catalog takes it the same way, and the library is exactly as it was
    let other = transfer::load_readonly(&dir).unwrap();
    assert_eq!(other.photos().count(), 2);
    let mut here = crate::Catalog::new();
    let plan = transfer::plan_import(&here, &other);
    assert_eq!(plan.new_photos.len(), 2);
    let op = plan.ops(&mut here, &other, transfer::ConflictRule::Keep);
    here.apply(op).unwrap();
    let mine = here.photos().find(|p| p.file_name == "wedding-0001.cr3").unwrap();
    assert_eq!((mine.flag, mine.develop.masks.len()), (Flag::Pick, 3), "merged with its pick and masks");
    assert_eq!(files(&dir), before, "nothing in the LightCraft library changed, nothing added");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_newer_lightcraft_library_is_refused_and_left_alone() {
    let dir = temp("newer");
    write_library(&dir, lightcraft::NEWEST + 1);
    let before = files(&dir);
    let e = lightcraft::load(&dir).unwrap_err();
    assert!(matches!(e, crate::CatalogError::Newer(_)), "{e}");
    assert_eq!(files(&dir), before);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A mask this app can't read is left out, the photo's other edits kept; a photo whose settings can't be read at all
/// comes in unedited with its pick; a damaged record mid-log ends the replay, as LightCraft does.
#[test]
fn what_cant_be_read_costs_only_itself() {
    let dir = temp("partial");
    write_library(&dir, 10);
    let mut snap: Value = serde_json::from_slice(&std::fs::read(dir.join("catalog.snap")).unwrap()).unwrap();
    let photos = &mut snap["catalog"]["photos"];
    photos["1"]["develop"]["masks"][0]["components"][0]["shape"] = json!({"kind": "hologram", "depth": 3});
    photos["2"]["develop"]["light"] = json!("not settings");
    photos["2"]["flag"] = json!("pick");
    std::fs::write(dir.join("catalog.snap"), snapshot(10, &snap["catalog"])).unwrap();
    let mut log = std::fs::read_to_string(dir.join("catalog.log")).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    // the third record damaged: it and every one after it are not applied
    log = format!("{}\n{}\n{}x\n{}\n{}\n", lines[0], lines[1], lines[2], lines[3], lines[4]);
    std::fs::write(dir.join("catalog.log"), log).unwrap();

    let (cat, report) = lightcraft::load(&dir).unwrap();
    assert_eq!(report.masks_left_out, 1, "{report:?}");
    assert_eq!(report.edits_unreadable, 1, "{report:?}");
    assert_eq!(report.photos, 2);
    let p1 = cat.photo(PhotoId(1)).unwrap();
    assert_eq!((p1.develop.masks.len(), p1.develop.light.exposure), (2, 0.7), "the other masks and edits kept");
    let p2 = cat.photo(PhotoId(2)).unwrap();
    assert_eq!(p2.develop.light.exposure, 0.0, "unedited");
    assert_eq!(p2.flag, Flag::Reject, "its pick came in, then the log's reject (record 7, before the damage)");
    assert_eq!(p2.label, None, "after the damage: not applied");
    assert_eq!(report.replayed, 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// This app's own catalogs and other folders aren't taken for LightCraft libraries; one without a snapshot or log
/// is refused.
#[test]
fn only_a_lightcraft_library_is_read_as_one() {
    let dir = temp("other");
    std::fs::write(dir.join("catalog.snap"), br#"{"format":"lcc-catalog","version":16,"seq":0,"catalog":{}}"#).unwrap();
    assert!(!lightcraft::is_lightcraft(&dir));
    assert!(matches!(lightcraft::load(&dir), Err(crate::CatalogError::Foreign(_))));
    let empty = temp("empty");
    assert!(!lightcraft::is_lightcraft(&empty));
    assert!(lightcraft::load(&empty).is_err());
    std::fs::write(dir.join("catalog.snap"), b"{\"format\":\"lightcraft-catalog\",\"version\":10,\"seq\":0,\"catalog\":42}").unwrap();
    assert!(lightcraft::load(&dir).is_err(), "a snapshot without a catalog");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&empty);
}
