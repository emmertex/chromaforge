//! An optional scene segmentation model for the automatic Sky, Landscape (water, vegetation) and
//! People selections ([`crate::auto_mask`]).
//!
//! The engine installs one ([`set_scene_model`]) only when the user chose it and its weights
//! are in place (EoMT-S, COCO panoptic); otherwise nothing changes: the photo analysis of
//! `auto_mask.rs` stands. This layer only calls the model: given the whole uncropped, oriented
//! photo as 8-bit sRGB, it returns `0..1` grids over the same frame. Results are cached per photo
//! (pixels and framing; white balance left out, so a WB drag never re-runs the model).
//!
//! Sources: own design (the same hook as the Subject model's).

use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use lcc_develop::DevelopSettings;
use lcc_raster::{Plane, Rgb32f};

use crate::SourceInfo;
use crate::auto_mask::AutoMasks;

/// What a scene model selects, each a `0..1` grid of any size over the whole uncropped frame.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneSelection {
    pub sky: Plane,
    pub water: Plane,
    pub vegetation: Plane,
    /// Every person in the photo.
    pub person: Plane,
    /// The rest of Landscape ([`crate::auto_mask::LANDSCAPE_TERRAIN`] names): mountains,
    /// architecture, natural and artificial ground.
    pub terrain: Vec<(&'static str, Plane)>,
}

/// The model: `(rgb, w, h)` → its selection, or `None` when it can't (the analysis is used then).
pub type SceneModel = dyn Fn(&[u8], usize, usize) -> Option<SceneSelection> + Send + Sync;

static MODEL: RwLock<Option<Arc<SceneModel>>> = RwLock::new(None);
static CACHE: Mutex<Vec<(u64, Arc<SceneSelection>)>> = Mutex::new(Vec::new());
/// Held while the model runs: a second render of the same photo waits and takes the cached result.
static BUSY: Mutex<()> = Mutex::new(());
const CACHE_LEN: usize = 8;

/// Long edge of the image the scene model is given (EoMT-S works at 640 px).
pub const MODEL_EDGE: usize = 640;

/// Install (or with `None` remove) the scene model; cached selections are dropped.
pub fn set_scene_model(m: Option<Arc<SceneModel>>) {
    *MODEL.write().unwrap_or_else(PoisonError::into_inner) = m;
    CACHE.lock().unwrap_or_else(PoisonError::into_inner).clear();
    crate::auto_mask::clear_cache();
}

/// Whether a scene model is installed.
pub fn has_scene_model() -> bool {
    MODEL.read().unwrap_or_else(PoisonError::into_inner).is_some()
}

fn usable(p: &Plane) -> bool {
    p.width > 0 && p.height > 0 && p.data.len() == p.width.saturating_mul(p.height) && p.data.iter().all(|v| v.is_finite())
}

/// The installed model's selection of `src` under `s` (cached), if there is a model and it ran.
pub(crate) fn selection(src: &Rgb32f, info: &SourceInfo, s: &DevelopSettings) -> Option<Arc<SceneSelection>> {
    let model = MODEL.read().unwrap_or_else(PoisonError::into_inner).clone()?;
    let frame = crate::frame_for(src, info, s, false);
    let mut hs = std::collections::hash_map::DefaultHasher::new();
    (src.width, src.height, format!("{frame:?}")).hash(&mut hs);
    let step = (src.data.len() / 4096).max(1);
    for c in src.data.iter().step_by(step) {
        c.map(f32::to_bits).hash(&mut hs);
    }
    let key = hs.finish();
    let cached = || CACHE.lock().unwrap_or_else(PoisonError::into_inner).iter().find(|(k, _)| *k == key).map(|(_, p)| p.clone());
    if let Some(p) = cached() {
        return Some(p);
    }
    let _busy = BUSY.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(p) = cached() {
        return Some(p);
    }
    let (w, h) = frame.fit(MODEL_EDGE, MODEL_EDGE);
    let (w, h) = (w.max(1), h.max(1));
    let img = frame.sample(src, w, h);
    let rgb = crate::auto_mask::model_input(&img);
    let sel = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| model(&rgb, w, h))).ok().flatten()?;
    if ![&sel.sky, &sel.water, &sel.vegetation, &sel.person].into_iter().chain(sel.terrain.iter().map(|(_, p)| p)).all(usable) {
        return None;
    }
    let sel = Arc::new(sel);
    let mut c = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    if c.len() >= CACHE_LEN {
        c.remove(0);
    }
    c.push((key, sel.clone()));
    Some(sel)
}

/// The analysis `a` with the scene model's selections in place of its own.
pub(crate) fn apply(a: &mut AutoMasks, sel: &SceneSelection) {
    a.sky = sel.sky.clone();
    a.water = sel.water.clone();
    a.vegetation = sel.vegetation.clone();
    a.person = Some(sel.person.clone());
    a.terrain = sel.terrain.clone();
}

#[cfg(test)]
mod tests {
    use super::*;
    use lcc_develop::MaskShape;

    fn plane(w: usize, h: usize, v: f32) -> Plane {
        Plane { width: w, height: h, data: vec![v; w * h] }
    }

    #[test]
    fn the_model_replaces_sky_landscape_and_people_and_keeps_subject() {
        let mut a = crate::auto_mask::analyze(&Rgb32f::new(16, 12));
        let subject = a.subject.clone();
        let sel = SceneSelection { sky: plane(4, 3, 0.9), water: plane(4, 3, 0.2), vegetation: plane(4, 3, 0.3), person: plane(4, 3, 0.7), terrain: vec![("mountains", plane(4, 3, 0.6))] };
        apply(&mut a, &sel);
        assert_eq!(a.for_shape(&MaskShape::Sky), Some(&sel.sky));
        assert_eq!(a.for_shape(&MaskShape::Landscape { class: "Water".into() }), Some(&sel.water));
        assert_eq!(a.for_shape(&MaskShape::Landscape { class: "vegetation".into() }), Some(&sel.vegetation));
        assert_eq!(a.for_shape(&MaskShape::People { person: 0, parts: vec![], who: None, seg: None }), Some(&sel.person));
        assert_eq!(a.for_shape(&MaskShape::Subject), Some(&subject));
        // the other Landscape classes by their loose names; none the model didn't compute
        assert_eq!(a.for_shape(&MaskShape::Landscape { class: "Mountains".into() }), sel.terrain.first().map(|(_, p)| p));
        assert_eq!(a.for_shape(&MaskShape::Landscape { class: "rock".into() }), sel.terrain.first().map(|(_, p)| p));
        assert_eq!(a.for_shape(&MaskShape::Landscape { class: "road".into() }), None);
    }

    #[test]
    fn installing_a_model_moves_the_keys_of_masks_on_automatic_selections_only() {
        use lcc_develop::{Mask, MaskComponent};
        let mask = |shape| {
            vec![Mask {
                id: 1,
                components: vec![MaskComponent { anchor: None, name: None, op: Default::default(), invert: false, shape }],
                ..Default::default()
            }]
        };
        let sky = mask(MaskShape::Sky);
        let radial = mask(MaskShape::Radial { center: lcc_geom::Point::new(0.5, 0.5), rx: 0.2, ry: 0.2, angle: 0.0, feather: 50.0, invert: false });
        let (k_sky, k_radial) = (crate::masks::alpha_key(&sky, 0.0, 64, 48, None), crate::masks::alpha_key(&radial, 0.0, 64, 48, None));
        let before = crate::auto_mask::model_generation();
        set_scene_model(Some(Arc::new(|_: &[u8], _: usize, _: usize| None)));
        set_scene_model(None);
        assert!(crate::auto_mask::model_generation() > before);
        assert_ne!(crate::masks::alpha_key(&sky, 0.0, 64, 48, None), k_sky, "a Sky mask re-renders");
        assert_eq!(crate::masks::alpha_key(&radial, 0.0, 64, 48, None), k_radial, "a radial mask keeps its cache");
    }

    #[test]
    fn malformed_selections_are_not_used() {
        assert!(!usable(&Plane { width: 4, height: 3, data: vec![0.0; 5] }));
        assert!(!usable(&plane(0, 0, 0.0)));
        assert!(!usable(&plane(2, 2, f32::NAN)));
        assert!(usable(&plane(2, 2, 0.5)));
    }
}
