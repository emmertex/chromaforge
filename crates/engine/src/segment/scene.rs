//! The scene model for Sky, Landscape and People masks: EoMT-S COCO panoptic
//! (`lcc_segment::scene`, MIT weights, trained on COCO), an on-request download of
//! [`lcc_segment::models::SCENE`].
//!
//! It is the user's choice ([`super::models::SceneModel`], `ai.models.setSceneModel`): by
//! default those masks keep the photo analysis of `auto_mask.rs`, as before. Once EoMT is
//! chosen and its files are in place, it is installed as the pipeline's scene model
//! ([`lcc_pipeline::scene_model::set_scene_model`]): Sky, Landscape (water, vegetation) and
//! People then use its selections, computed once per photo (cached) with an activity row while
//! it runs. A failure to load or run it is logged and the photo analysis stands in.

use std::path::{Path, PathBuf};

use crate::activity::Activity;

/// Which folder the installed pipeline model was loaded from.
#[derive(Default)]
pub struct SceneMasks {
    installed_from: Option<PathBuf>,
}

impl SceneMasks {
    /// Install the model from `want` (idempotent), or remove it with `None`.
    pub fn sync(&mut self, want: Option<PathBuf>, activity: &Activity) {
        if want == self.installed_from {
            return;
        }
        match &want {
            Some(dir) => install(dir, activity.clone()),
            None => lcc_pipeline::scene_model::set_scene_model(None),
        }
        self.installed_from = want;
    }

    /// Whether the pipeline uses the scene model now.
    pub fn active(&self) -> bool {
        self.installed_from.is_some() && lcc_pipeline::scene_model::has_scene_model()
    }
}

/// The COCO panoptic classes each selection sums.
#[cfg(feature = "sam")]
fn is_sky(l: &str) -> bool {
    l.starts_with("sky")
}
#[cfg(feature = "sam")]
fn is_water(l: &str) -> bool {
    matches!(l, "sea" | "river" | "water-other")
}
#[cfg(feature = "sam")]
fn is_vegetation(l: &str) -> bool {
    matches!(l, "tree-merged" | "grass-merged" | "flower" | "pottedplant")
}

/// A scene segmentation as the pipeline's selections.
#[cfg(feature = "sam")]
pub(crate) fn selection(s: &lcc_segment::scene::Scene) -> lcc_pipeline::scene_model::SceneSelection {
    use lcc_segment::scene::class_ids;
    let plane = |pred: fn(&str) -> bool| lcc_raster::Plane { width: s.width, height: s.height, data: s.class_probability(&class_ids(pred)) };
    lcc_pipeline::scene_model::SceneSelection {
        sky: plane(is_sky),
        water: plane(is_water),
        vegetation: plane(is_vegetation),
        person: plane(|l| l == "person"),
    }
}

/// Install the model in `dir` as the pipeline's scene model (loaded on first use).
#[cfg(feature = "sam")]
fn install(dir: &Path, activity: Activity) {
    use std::sync::{Arc, Mutex, PoisonError};

    use lcc_segment::scene::SceneModel;

    let dir = dir.to_path_buf();
    let model: Mutex<Option<SceneModel>> = Mutex::new(None);
    let run = move |rgb: &[u8], w: usize, h: usize| -> Option<lcc_pipeline::scene_model::SceneSelection> {
        let _task = activity.start("segment", "Finding sky, water, plants and people", crate::activity::Cancel::No);
        let mut m = model.lock().unwrap_or_else(PoisonError::into_inner);
        if m.is_none() {
            match SceneModel::load(&dir).or_else(|e| {
                log::warn!("scene model on the GPU: {e}; using the CPU");
                SceneModel::load_cpu(&dir)
            }) {
                Ok(s) => *m = Some(s),
                Err(e) => {
                    log::warn!("scene model: {e}");
                    return None;
                }
            }
        }
        let started = std::time::Instant::now();
        match m.as_ref()?.segment(rgb, w, h) {
            Ok(s) => {
                log::info!("scene model: {w}×{h} in {:?}", started.elapsed());
                Some(selection(&s))
            }
            Err(e) => {
                log::warn!("scene model: {e}");
                None
            }
        }
    };
    lcc_pipeline::scene_model::set_scene_model(Some(Arc::new(run)));
}

#[cfg(not(feature = "sam"))]
fn install(_: &Path, _: Activity) {}

#[cfg(all(test, feature = "sam"))]
mod tests {
    use super::*;
    use lcc_segment::scene::{CLASSES, Scene, class_ids};

    #[test]
    fn the_selections_sum_their_classes() {
        let (w, h) = (2usize, 1usize);
        let n = w * h;
        let mut probs = vec![0f32; CLASSES * n];
        let id = |name: &str| class_ids(|l| l == name)[0];
        probs[id("sky-other-merged") * n] = 0.9;
        probs[id("sea") * n + 1] = 0.5;
        probs[id("river") * n + 1] = 0.3;
        probs[id("tree-merged") * n + 1] = 0.4;
        probs[id("person") * n + 1] = 0.8;
        let sel = selection(&Scene { width: w, height: h, probs, instances: Vec::new() });
        assert_eq!((sel.sky.width, sel.sky.height), (w, h));
        assert!(sel.sky.data[0] > 0.89 && sel.sky.data[1] < 0.01);
        assert!((sel.water.data[1] - 0.8).abs() < 1e-6);
        assert!(sel.vegetation.data[1] > 0.39 && sel.vegetation.data[0] < 0.01);
        assert!(sel.person.data[1] > 0.79);
    }

    #[test]
    fn a_damaged_model_leaves_the_photo_analysis() {
        let dir = std::env::temp_dir().join(format!("lcc-scene-engine-damaged-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("config.json"), br#"{"model_type": "eomt"}"#).unwrap();
        std::fs::write(dir.join("model.safetensors"), b"not weights").unwrap();
        let activity = Activity::default();
        let mut s = SceneMasks::default();
        s.sync(Some(dir.clone()), &activity);
        assert!(s.active());
        let src = lcc_raster::Rgb32f::new(64, 48);
        let info = lcc_pipeline::SourceInfo::default();
        let set = lcc_develop::DevelopSettings::default();
        // the model fails to load: no panic, and the analysis gives People no person grid
        let a = lcc_pipeline::auto_mask::for_source(&src, &info, &set);
        assert!(a.person.is_none());
        s.sync(None, &activity);
        assert!(!s.active());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
