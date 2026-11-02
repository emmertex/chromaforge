//! The Folders header's + ▸ Add Folder… (#734): the folder dialog and the scan run off the UI
//! thread, as File ▸ Import from Folder… does, and the photos found open in the import review.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::state::Dialog;
use crate::{LccApp, Services};

const T: Duration = Duration::from_secs(10);

type Answers = Arc<Mutex<Vec<mpsc::Sender<crate::pick::PickResult>>>>;

/// A folder with one photo, removed when the test ends (also when an assert fails).
struct TempFolder(PathBuf);

impl TempFolder {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("lc-folder-add-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let img = lcc_raster::Rgba8 { width: 8, height: 8, data: vec![[90, 120, 160, 255]; 64] };
        let png = lcc_codecs::encode_png(&lcc_codecs::EncodeImage::rgba8(&img), &lcc_codecs::EncodeMeta::default()).unwrap();
        std::fs::write(dir.join("beach.png"), png).unwrap();
        Self(dir)
    }
}

impl Drop for TempFolder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    for _ in 0..3 {
        h.step();
    }
}

/// The app in the grid with the Folders panel open, a background picker whose dialogs wait in
/// the returned list for their answer.
fn app_with_picker(services: Services) -> (Headless, Answers) {
    let mut h = Headless::new(LccApp::new(lcc_engine::Session::new().with_fs(), services), [1300.0, 1600.0], 1.0);
    let answers: Answers = Default::default();
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
    (h, answers)
}

/// Answer the folder dialog with `dir` and check the review: Add (photos stay where they are),
/// nothing in the library before it.
fn answer_and_check_review(h: &mut Headless, answers: &Answers, dir: &TempFolder) {
    assert_eq!(answers.lock().unwrap().len(), 1, "the folder dialog is up, shown elsewhere");
    answers.lock().unwrap()[0].send(Ok(vec![dir.0.to_string_lossy().into()])).unwrap();
    h.settle(T);
    let Some(Dialog::Import { opts }) = &h.app.ui.dialog else {
        panic!("the photos found open in the import review");
    };
    assert!(!opts.copy, "the photos are added where they are, not copied");
    assert_eq!(h.app.session.catalog.photos().count(), 0, "nothing joins the library before the review");
}

#[test]
fn add_folder_asks_for_its_folder_and_scans_it_off_the_ui_thread() {
    let dir = TempFolder::new("desktop");
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
    let (mut h, answers) = app_with_picker(services);

    click(&mut h, "icon:folderAdd");
    click(&mut h, "button:folderAddFolder");
    assert!(!blocked.load(Ordering::Relaxed), "the folder dialog blocked the UI thread");
    answer_and_check_review(&mut h, &answers, &dir);
}

#[test]
fn add_folder_is_offered_with_only_the_background_picker() {
    // Android: the system picker runs in the background, there's no synchronous folder dialog
    let dir = TempFolder::new("android");
    let (mut h, answers) = app_with_picker(Services { png: None, ..Default::default() });
    assert!(h.app.services.pick_folder.is_none());

    click(&mut h, "icon:folderAdd");
    click(&mut h, "button:folderAddFolder");
    answer_and_check_review(&mut h, &answers, &dir);
}
