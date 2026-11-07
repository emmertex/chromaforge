//! What plug-ins see of a photo's pixels and change of its develop settings: `photo.pixels`, a
//! downsized copy of the whole uncropped frame, and `develop.setCrop`, which sets its crop as
//! `crop.set` does. Together they let a plug-in find a crop (a film frame's picture area inside
//! its rebate, say) and set it as an ordinary, editable crop. The permission checks are in
//! `lcc-plugins`; this is what runs once they passed.

use lcc_catalog::PhotoId;
use serde_json::{Value, json};

use crate::Session;
use crate::media::SourceLevel;

/// Photo `photo` (a plug-in's decimal id) of `s`'s library.
fn photo_of(s: &Session, photo: &str, method: &str) -> Result<PhotoId, String> {
    let id = photo.trim().parse::<u64>().map(PhotoId).map_err(|_| format!("{method}: {photo:?} is not a photo id"))?;
    s.catalog.photo(id).map(|_| id).ok_or_else(|| format!("{method}: no photo {photo}"))
}

/// `photo.pixels`: the photo as the crop tool shows it uncropped, long edge at most `long_edge`,
/// 8-bit sRGB, interleaved RGB rows as base64.
pub(super) fn pixels(s: &mut Session, photo: &str, long_edge: u32) -> Result<Value, String> {
    const M: &str = "photo.pixels";
    let id = photo_of(s, photo, M)?;
    let p = s.catalog.photo(id).cloned().ok_or_else(|| format!("{M}: no photo {photo}"))?;
    let settings = s.develop_of(id).unwrap_or_default();
    let source = s.media.source_ref(&p, SourceLevel::for_size(long_edge as usize));
    let src = source.load_source().map_err(|e| format!("{M}: {e}"))?;
    let (linear, w, h) = lcc_pipeline::plugin_filters::uncropped_input(&src.image, &settings, long_edge as usize);
    let bytes: Vec<u8> = linear.iter().map(|v| lcc_color::transfer::encode_srgb8(*v)).collect();
    Ok(json!({
        "width": w,
        "height": h,
        "channels": 3,
        "encoding": "srgb8",
        "data": crate::base64::encode(&bytes),
    }))
}

/// `develop.setCrop`: photo `photo`'s crop, `rect` normalized in the frame [`pixels`] gives.
pub(super) fn set_crop(s: &mut Session, photo: &str, rect: [f64; 4], angle: Option<f64>) -> Result<Value, String> {
    const M: &str = "develop.setCrop";
    let id = photo_of(s, photo, M)?;
    let g = super::develop::set_crop_of(s, id, M, Some(rect), angle).map_err(|e| e.to_string())?;
    Ok(json!({"rect": [g.rect.x0, g.rect.y0, g.rect.x1, g.rect.y1], "angle": g.angle}))
}
