//! The Folders header's + ▸ Add Folder… (#734): the folder dialog and the scan run off the UI
//! thread, as File ▸ Import from Folder… does, and the photos found open in the import review.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::state::Dialog;
use crate::{LccApp, Services};

const T: Duration = Duration::from_secs(10);

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    for _ in 0..3 {
        h.step();
    }
}

#[test]
fn add_folder_asks_for_its_folder_and_scans_it_off_the_ui_thread() {
    let dir = std::env::temp_dir().join(format!("lc-folder-add-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let img = lcc_raster::Rgba8 { width: 8, height: 8, data: vec![[90, 120, 160, 255]; 64] };
    let png = lcc_codecs::encode_png(&lcc_codecs::EncodeImage::rgba8(&img), &lcc_codecs::EncodeMeta::default()).unwrap();
    std::fs::write(dir.join("beach.png"), png).unwrap();

    let blocked = Arc::new(AtomicBool::new(false));
    let b = blocked.clone();
    let services = Services {
        png: None,
        pick_folder: Some(Box::new(move || {
            b.store(true, Ordering::Relaxed);
            None
        })),
        ..Default::default()
    };
    let mut h = Headless::new(LccApp::new(lcc_engine::Session::new().with_fs(), services), [1300.0, 1600.0], 1.0);
    let answers: Arc<Mutex<Vec<mpsc::Sender<crate::pick::PickResult>>>> = Default::default();
    let a = answers.clone();
    h.app.services.picker = Some(Box::new(move |_req| {
        let (tx, rx) = mpsc::channel();
        a.lock().unwrap().push(tx);
        Ok(rx)
    }));
    let r = h.request("ui.set", json!({"view": "photoGrid", "leftPanel": true, "sidebar": ["folders"]}), T);
    assert_eq!(r["ok"], true, "{r}");
    for _ in 0..6 {
        h.step();
    }

    click(&mut h, "icon:folderAdd");
    click(&mut h, "button:folderAddFolder");
    assert!(!blocked.load(Ordering::Relaxed), "the folder dialog blocked the UI thread");
    assert_eq!(answers.lock().unwrap().len(), 1, "the folder dialog is up, shown elsewhere");

    answers.lock().unwrap()[0].send(Ok(vec![dir.to_string_lossy().into()])).unwrap();
    h.settle(T);
    assert!(matches!(h.app.ui.dialog, Some(Dialog::Import { .. })), "the photos found open in the import review");
    assert_eq!(h.app.session.catalog.photos().count(), 0, "nothing joins the library before the review");
    let _ = std::fs::remove_dir_all(&dir);
}
