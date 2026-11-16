//! The eye focus strip's results kept on disk (`eye_store`): they survive a new session, follow
//! the file, a corrupt record is looked at again, and `eyes.analyze` runs as an activity row.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use lcc_catalog::PhotoId;
use serde_json::json;

use crate::Session;
use crate::eye_store::tests::report;

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-eye-keep-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A folder with `n` small JPEGs, browsed (not imported) in a library on disk.
fn browsed(root: &Path, n: usize) -> (Session, PathBuf, Vec<PhotoId>) {
    let shoot = root.join("shoot");
    std::fs::create_dir_all(&shoot).unwrap();
    for i in 0..n {
        let img = lcc_raster::Rgba8 { width: 32, height: 24, data: vec![[(i * 40) as u8, 120, 60, 255]; 32 * 24] };
        std::fs::write(shoot.join(format!("p{i}.jpg")), lcc_preview::encode_jpeg(&img).unwrap()).unwrap();
    }
    let s = open(root, &shoot);
    let ids = ids(&s);
    assert_eq!(ids.len(), n);
    (s, shoot, ids)
}

fn open(root: &Path, shoot: &Path) -> Session {
    let mut s = Session::new().with_fs();
    s.open_library(root.join("lib"), false).unwrap();
    s.execute("library.browse", &json!({"path": shoot.to_string_lossy()})).unwrap();
    s
}

fn ids(s: &Session) -> Vec<PhotoId> {
    let mut v: Vec<(String, PhotoId)> = s
        .catalog
        .photos()
        .filter_map(|p| match &p.source {
            lcc_catalog::Source::File { path } => Some((path.clone(), p.id)),
            _ => None,
        })
        .collect();
    v.sort();
    v.into_iter().map(|x| x.1).collect()
}

/// Judge photo `id` as the strip and Reject Closed and Blurry Eyes do: (found, read from the cache).
fn judge(s: &Session, models: &Path, id: PhotoId) -> (bool, bool) {
    let job = s.eye_job(id).unwrap();
    let mut out = None;
    crate::eye_cull::judge_counted(models, &[job], &AtomicBool::new(false), false, |_, r, cached| out = Some((r.is_ok(), cached)));
    out.unwrap()
}

/// A result kept once is there for a new session (a restart) of the same library, read without
/// the model; a changed file is looked at again; a corrupt record is dropped and looked at again.
#[test]
fn kept_eyes_survive_a_restart_and_follow_the_file() {
    let _keys = crate::sensor_maps::keep_generation();
    let root = temp_dir("restart");
    // no model here: anything not kept fails to be looked at
    let models = root.join("models").join("mediapipe-face-landmarker");
    std::fs::create_dir_all(&models).unwrap();
    let (s, shoot, ids) = browsed(&root, 2);
    assert_eq!(judge(&s, &models, ids[0]), (false, false));
    let job = s.eye_job(ids[0]).unwrap();
    let store = job.store.clone().unwrap();
    let key = store.key(&models).unwrap();
    crate::eye_store::write(&store.cache, key, &report());
    assert_eq!(judge(&s, &models, ids[0]), (true, true));
    drop(s);

    // a new session: kept on disk, in the library's preview cache
    let s = open(&root, &shoot);
    let id = ids_of(&s, &shoot, "p0.jpg");
    assert_eq!(judge(&s, &models, id), (true, true), "kept across sessions");
    let job = s.eye_job(id).unwrap();
    let mut eyes = 0;
    crate::eye_cull::judge(&models, &[job], &AtomicBool::new(false), |_, r| eyes = r.map(|r| r.eyes.len()).unwrap_or(0));
    assert_eq!(eyes, 2);
    // the other photo was never looked at
    assert_eq!(judge(&s, &models, ids_of(&s, &shoot, "p1.jpg")), (false, false));

    // a corrupt record: dropped, the photo looked at again (never a crash)
    s.media.rendered.put_data(key, b"LCEY\x01\x00\x00\x00 not a record");
    assert_eq!(judge(&s, &models, id), (false, false));
    assert!(s.media.rendered.get_data(key).is_none(), "the corrupt record is gone");

    // the file changes: another key, looked at again
    crate::eye_store::write(&s.media.rendered, key, &report());
    assert_eq!(judge(&s, &models, id), (true, true));
    let f = shoot.join("p0.jpg");
    let mut bytes = std::fs::read(&f).unwrap();
    bytes.extend_from_slice(b"edited");
    std::fs::write(&f, bytes).unwrap();
    assert_eq!(judge(&s, &models, id), (false, false), "a changed file is looked at again");
    drop(s);
    let _ = std::fs::remove_dir_all(&root);
}

fn ids_of(s: &Session, shoot: &Path, name: &str) -> PhotoId {
    let want = shoot.join(name);
    s.catalog.photos().find(|p| matches!(&p.source, lcc_catalog::Source::File { path } if Path::new(path) == want)).map(|p| p.id).unwrap()
}

/// A library in memory keeps nothing on disk (and nothing breaks).
#[test]
fn a_library_in_memory_keeps_nothing() {
    let root = temp_dir("memory");
    let shoot = root.join("shoot");
    std::fs::create_dir_all(&shoot).unwrap();
    let img = lcc_raster::Rgba8 { width: 8, height: 8, data: vec![[9, 9, 9, 255]; 64] };
    std::fs::write(shoot.join("a.jpg"), lcc_preview::encode_jpeg(&img).unwrap()).unwrap();
    let mut s = Session::new().with_fs();
    s.execute("library.browse", &json!({"path": shoot.to_string_lossy()})).unwrap();
    let id = ids(&s)[0];
    let job = s.eye_job(id).unwrap();
    assert!(job.store.unwrap().key(&root).is_none());
    let _ = std::fs::remove_dir_all(&root);
}

/// `eyes.analyze` over a browsed folder: a batch row in the activity stack, photos kept already
/// are skipped (no model loaded for them), the others fail without the model; `status` reports.
#[cfg(feature = "sam")]
#[test]
fn analyze_runs_as_an_activity_row_and_skips_kept_photos() {
    let root = temp_dir("command");
    let (mut s, shoot, ids) = browsed(&root, 3);
    // a stand-in for the installed face model (right names and sizes; it can't be loaded)
    let models = root.join("models");
    let m = lcc_segment::models::by_key("face").unwrap();
    let dir = models.join(m.dir_name);
    std::fs::create_dir_all(&dir).unwrap();
    for f in m.files.iter().chain(m.companions.iter().map(|c| &c.file)) {
        std::fs::File::create(dir.join(f.name)).unwrap().set_len(f.size.unwrap_or(1)).unwrap();
    }
    s.segmenter.models_root = Some(models);
    let model_dir = s.eye_model_dir().unwrap();
    for id in &ids[..2] {
        let st = s.eye_job(*id).unwrap().store.unwrap();
        crate::eye_store::write(&st.cache, st.key(&model_dir).unwrap(), &report());
    }
    // now, as the CLI's prep run does
    let r = s.execute("eyes.analyze", &json!({"path": shoot.to_string_lossy(), "wait": true})).unwrap();
    assert_eq!((r["photos"].as_u64(), r["kept"].as_u64(), r["analyzed"].as_u64(), r["failed"].as_u64()), (Some(3), Some(2), Some(0), Some(1)), "{r}");
    let r = s.execute("eyes.analyze", &json!({"path": shoot.to_string_lossy(), "wait": true, "lanes": 2})).unwrap();
    assert_eq!((r["kept"].as_u64(), r["failed"].as_u64(), r["lanes"].as_u64()), (Some(2), Some(1), Some(2)), "{r}");
    // in the background: a batch row, and its summary when it ends
    let r = s.execute("eyes.analyze", &json!({"path": shoot.to_string_lossy()})).unwrap();
    assert_eq!(r["photos"], 3);
    let t0 = std::time::Instant::now();
    while s.execute("eyes.analyze", &json!({"status": true})).unwrap()["done"] != true {
        assert!(t0.elapsed().as_secs() < 30, "the run never ended");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let st = s.execute("eyes.analyze", &json!({"status": true})).unwrap();
    assert_eq!((st["kept"].as_u64(), st["failed"].as_u64()), (Some(2), Some(1)), "{st}");
    let t0 = std::time::Instant::now();
    let row = loop {
        if let Some(b) = s.activity.finished().into_iter().find(|b| b.kind == "eyes") {
            break b;
        }
        assert!(t0.elapsed().as_secs() < 10, "no activity row");
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    assert_eq!((row.label.as_str(), row.done, row.failed, row.total, row.cancelled), (crate::eye_prep::LABEL, 3, 1, 3, false));
    // cancelling when nothing runs is no error
    assert_eq!(s.execute("eyes.analyze", &json!({"cancel": true})).unwrap()["cancelled"], false);
    drop(s);
    let _ = std::fs::remove_dir_all(&root);
}
