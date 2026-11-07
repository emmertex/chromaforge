//! What plug-ins see of a photo's pixels and change of its develop settings: `photo.pixels`, a
//! downsized copy of the whole uncropped frame, and `develop.setCrop`, which sets its crop as
//! `crop.set` does. Together they let a plug-in find a crop (a film frame's picture area inside
//! its rebate, say) and set it as an ordinary, editable crop. The permission checks are in
//! `lcc-plugins`; this is what runs once they passed.

use std::sync::Arc;

use lcc_catalog::PhotoId;
use lcc_develop::DevelopSettings;
use serde_json::{Value, json};

use crate::Session;
use crate::media::{SourceLevel, SourceRef};

/// Photo `photo` (a plug-in's decimal id) of `s`'s library.
fn photo_of(s: &Session, photo: &str, method: &str) -> Result<PhotoId, String> {
    let id = photo.trim().parse::<u64>().map(PhotoId).map_err(|_| format!("{method}: {photo:?} is not a photo id"))?;
    s.catalog.photo(id).map(|_| id).ok_or_else(|| format!("{method}: no photo {photo}"))
}

const PIXELS: &str = "photo.pixels";

/// What `photo.pixels` decodes: found on the session's thread, which is quick, and decoded by
/// [`PixelsSource::decode`] on any thread, which takes a while (about 0.3 s for a 33 MP raw). A
/// hook or job on a worker decodes there, so the window doesn't wait for it.
pub(super) struct PixelsSource {
    source: SourceRef,
    settings: Arc<DevelopSettings>,
    long_edge: u32,
}

/// Photo `photo`'s source at the smallest level that covers `long_edge`, and its settings.
pub(super) fn pixels_source(s: &mut Session, photo: &str, long_edge: u32) -> Result<PixelsSource, String> {
    let id = photo_of(s, photo, PIXELS)?;
    let p = s.catalog.photo(id).cloned().ok_or_else(|| format!("{PIXELS}: no photo {photo}"))?;
    let settings = s.develop_of(id).unwrap_or_default();
    let source = s.media.source_ref(&p, SourceLevel::for_size(long_edge as usize));
    Ok(PixelsSource { source, settings, long_edge })
}

impl std::fmt::Debug for PixelsSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PixelsSource").field("long_edge", &self.long_edge).finish_non_exhaustive()
    }
}

impl PixelsSource {
    /// The photo as the crop tool shows it uncropped, long edge at most `long_edge`, 8-bit sRGB,
    /// interleaved RGB rows as base64.
    pub(super) fn decode(self) -> Result<Value, String> {
        let src = self.source.load_source().map_err(|e| format!("{PIXELS}: {e}"))?;
        let (linear, w, h) = lcc_pipeline::plugin_filters::uncropped_input(&src.image, &self.settings, self.long_edge as usize);
        let bytes: Vec<u8> = linear.iter().map(|v| lcc_color::transfer::encode_srgb8(*v)).collect();
        Ok(json!({
            "width": w,
            "height": h,
            "channels": 3,
            "encoding": "srgb8",
            "data": crate::base64::encode(&bytes),
        }))
    }
}

/// `photo.pixels` on the session's thread: found and decoded here.
pub(super) fn pixels(s: &mut Session, photo: &str, long_edge: u32) -> Result<Value, String> {
    pixels_source(s, photo, long_edge)?.decode()
}

/// `develop.setCrop`: photo `photo`'s crop, `rect` normalized in the frame [`pixels`] gives turned
/// by `angle` about its centre. `crop.set` reads a rect in the photo's straightened frame, so an
/// angle left out is 0, not the photo's: the rect then lands where it was found in the pixels,
/// whatever the photo was straightened by before.
pub(super) fn set_crop(s: &mut Session, photo: &str, rect: [f64; 4], angle: Option<f64>) -> Result<Value, String> {
    const M: &str = "develop.setCrop";
    let id = photo_of(s, photo, M)?;
    let g = super::develop::set_crop_of(s, id, M, Some(rect), Some(angle.unwrap_or(0.0))).map_err(|e| e.to_string())?;
    Ok(json!({"rect": [g.rect.x0, g.rect.y0, g.rect.x1, g.rect.y1], "angle": g.angle}))
}

/// `photo.virtualCopy`: a virtual copy of photo `photo`, as `photo.virtualCopy` makes one (stacked
/// after it, in its albums, "Copy N" or `name`), without touching the selection. One History step.
pub(super) fn virtual_copy(s: &mut Session, photo: &str, name: Option<&str>) -> Result<Value, String> {
    const M: &str = "photo.virtualCopy";
    let id = photo_of(s, photo, M)?;
    if s.catalog.photo(id).is_some_and(|p| p.deleted) {
        return Err(format!("{M}: photo {photo} is in Recently Deleted"));
    }
    let copy = super::organize::virtual_copy_one(s, id, name).map_err(|e| e.to_string())?.ok_or_else(|| format!("{M}: no photo {photo}"))?;
    Ok(json!({"photo": copy.0.to_string()}))
}
