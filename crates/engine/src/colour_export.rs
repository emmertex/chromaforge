//! The app's colour, taken to other editors:
//! - [`photo_dcp`]: the camera colour a raw renders with (its chosen `.dcp` or calibration look, the file's own
//!   matrices and profile tables, or the starting colour the app fits for a raw without them) as a DNG camera
//!   profile (`.dcp`) for Lightroom, Camera Raw or RawTherapee;
//! - [`look_cube`]: a photo's global look (white balance aside: Light, tone curve, colour, mixer, colour grading,
//!   profile look) as a 33³ `.cube` LUT from sRGB to sRGB, for editors and tools that apply LUTs (Capture One,
//!   darktable's lut 3D module, Photoshop, video graders);
//! - [`with_photo_colour`]: the same camera colour written into a raw before it is saved as a DNG.
//!
//! Sources: own design; DNG specification 1.7 chapter 6 (camera profiles); Cube LUT format.
use lcc_develop::DevelopSettings;
use lcc_raw::RawImage;

/// Give `raw` the camera colour a photo of it with camera-profile choice `choice` (`profile.camera`) renders with:
/// the chosen (or the camera's default) `.dcp` or calibration look, else the file's own matrices, else the starting
/// colour the app fits for a raw without matrices. Returns the profile's name when a `.dcp` or look was applied.
pub(crate) fn with_photo_colour(raw: &mut RawImage, bytes: &[u8], choice: &str) -> Option<String> {
    let meta = raw.metadata.clone();
    let applied = crate::camera_profiles::apply_choice(&mut raw.color, &meta, choice);
    crate::camera_preview::with_starting_colour(raw, bytes);
    applied
}

/// The name camera profiles use for a camera (`UniqueCameraModel`): the camera database's make and model when it
/// knows the camera (the naming raw converters' own profiles use, "Nikon D850"), else the file's make and model.
pub fn profile_model(make: Option<&str>, model: &str) -> String {
    if let Some(e) = lcc_camdb::find(make, model) {
        return format!("{} {}", e.make.trim(), e.model.trim());
    }
    let (make, model) = (make.unwrap_or_default().trim(), model.trim());
    if make.is_empty() || model.to_lowercase().starts_with(&make.to_lowercase()) { model.to_string() } else { format!("{make} {model}") }
}

/// A `.dcp` of the camera colour a photo of the raw in `bytes` renders with (see [`with_photo_colour`]), named
/// `name`, for camera `unique` (`UniqueCameraModel`; default [`profile_model`]).
pub fn photo_dcp(bytes: &[u8], choice: &str, name: &str, unique: Option<&str>) -> Result<Vec<u8>, String> {
    let mut raw = lcc_raw::decode(bytes).map_err(|e| format!("not a raw photo: {e}"))?;
    with_photo_colour(&mut raw, bytes, choice);
    if !lcc_raw::color::has_matrix(&raw.color) {
        return Err("the app has no colour model for this camera to export".into());
    }
    let model = raw.metadata.model.clone().unwrap_or_default();
    let unique =
        unique.map(str::trim).filter(|u| !u.is_empty()).map(str::to_string).unwrap_or_else(|| profile_model(raw.metadata.make.as_deref(), &model));
    if unique.is_empty() {
        return Err("the raw names no camera model: pass the camera's name".into());
    }
    // a profile carries colour, not a photo's white balance
    let c = &raw.color;
    let color = lcc_raw::ColorData {
        illuminant: c.illuminant,
        color_matrix: c.color_matrix,
        forward_matrix: c.forward_matrix,
        camera_calibration: [None, None],
        baseline_exposure: if c.baseline_exposure.is_finite() { c.baseline_exposure } else { 0.0 },
        profile: lcc_raw::profile::ProfileLook { gain_table_map: None, ..c.profile.clone() },
        ..Default::default()
    };
    let name = if name.trim().is_empty() { format!("{} {unique}", lcc_brand::DISPLAY_NAME) } else { name.trim().to_string() };
    lcc_raw::dcp::write(&lcc_raw::dcp::Dcp { model: unique, name, copyright: String::new(), color })
}

/// Lattice points per axis of an exported look LUT.
pub const CUBE_SIZE: usize = lcc_pipeline::grade::LUT_SIZE;

/// `settings`' global look as a [`CUBE_SIZE`]³ `.cube` LUT: sRGB-encoded input to sRGB-encoded output, so it applies
/// to an sRGB rendering of a photo. White balance is left out (it belongs to the photo, not the look); local edits,
/// texture, clarity, dehaze, detail, effects, optics and geometry can't be a LUT and aren't in it.
pub fn look_cube(settings: &DevelopSettings, title: &str) -> Result<String, String> {
    use lcc_color::transfer::srgb_to_linear;
    let mut s = settings.clone();
    s.wb = Default::default();
    let lut = lcc_pipeline::grade::GradeLut::bake(&s)?;
    let to_working = lcc_color::WORKING.from_xyz().mul(&lcc_color::SRGB.to_xyz());
    let n = CUBE_SIZE;
    let at = |i: usize| srgb_to_linear(i as f32 / (n - 1) as f32);
    let mut data = Vec::with_capacity(n * n * n);
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                let lin = to_working.apply_f32([at(r), at(g), at(b)]).map(|v| v.max(0.0));
                data.push(lut.lookup(lin).map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }));
            }
        }
    }
    let title = Some(title.trim()).filter(|t| !t.is_empty()).map(str::to_string);
    Ok(lcc_pipeline::lut::Lut3d { size: n, data, domain_min: [0.0; 3], domain_max: [1.0; 3], title }.to_cube())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny RGGB DNG of a camera the database knows, with no matrices of its own.
    fn dng(model: &str) -> Vec<u8> {
        let (w, h) = (64usize, 48usize);
        let mut raw = lcc_raw::RawImage {
            format: lcc_raw::RawFormat::Dng,
            width: w,
            height: h,
            cpp: 1,
            data: lcc_raw::RawData::U16((0..w * h).map(|i| 1000 + (i % 97) as u16 * 40).collect()),
            cfa: lcc_raw::Cfa::bayer("RGGB"),
            bits: 14,
            black: lcc_raw::BlackLevel::uniform(512.0),
            white: vec![16000.0],
            active_area: lcc_raw::Rect::new(0, 0, w, h),
            crop: lcc_raw::Rect::new(0, 0, w, h),
            orientation: lcc_raw::Orientation::Normal,
            color: lcc_raw::ColorData {
                illuminant: [21, 0],
                color_matrix: [Some(lcc_raw::Mat3([[0.8, -0.2, -0.05], [-0.45, 1.3, 0.15], [-0.08, 0.22, 0.62]])), None],
                as_shot_neutral: Some([0.5, 1.0, 0.7]),
                ..Default::default()
            },
            wb_multipliers: None,
            linearized: false,
            pixel_aspect: 1.0,
            opcodes: Default::default(),
            metadata: Default::default(),
            lens_metadata: Default::default(),
        };
        raw.metadata.make = Some("NIKON CORPORATION".into());
        raw.metadata.model = Some(model.into());
        lcc_raw::write_dng(&raw, &Default::default()).unwrap()
    }

    #[test]
    fn a_photo_dcp_reads_back_with_the_colour_it_renders_with() {
        let bytes = dng("NIKON D3");
        let out = photo_dcp(&bytes, "", "", None).unwrap();
        let d = lcc_raw::dcp::parse(&out).unwrap();
        assert_eq!(d.model, "Nikon D3", "named as raw converters name the camera");
        assert!(d.name.contains("D3"));
        let raw = lcc_raw::decode(&bytes).unwrap();
        assert_eq!(
            d.color.color_matrix[0].map(|m| m.0.map(|r| r.map(|v| (v * 1e4).round()))),
            raw.color.color_matrix[0].map(|m| m.0.map(|r| r.map(|v| (v * 1e4).round())))
        );
        assert!(d.color.as_shot_neutral.is_none(), "a profile carries no white balance");
        let named = lcc_raw::dcp::parse(&photo_dcp(&bytes, "", "My Look", Some("Nikon D3 test")).unwrap()).unwrap();
        assert_eq!((named.name.as_str(), named.model.as_str()), ("My Look", "Nikon D3 test"));
    }

    /// A photo whose camera-profile choice names an imported `.dcp` gets that profile's colour (what DNG export
    /// writes), and the `.dcp` exported from it is that profile again.
    #[test]
    fn a_chosen_dcp_is_the_colour_written() {
        let bytes = dng("NIKON D3");
        let mine = lcc_raw::Mat3([[0.71, -0.11, -0.07], [-0.52, 1.33, 0.21], [-0.12, 0.28, 0.66]]);
        let profile = lcc_raw::dcp::Dcp {
            model: "Nikon D3".into(),
            name: "Mine".into(),
            copyright: String::new(),
            color: lcc_raw::ColorData { illuminant: [21, 0], color_matrix: [Some(mine), None], ..Default::default() },
        };
        let saved = lcc_raw::dcp::library();
        let mut list: Vec<(String, lcc_raw::dcp::Dcp)> = saved.iter().map(|(id, d)| (id.clone(), (**d).clone())).collect();
        list.push(("dcp:colour-export-test".into(), profile));
        lcc_raw::dcp::install_library(list);
        let mut raw = lcc_raw::decode(&bytes).unwrap();
        let name = with_photo_colour(&mut raw, &bytes, "dcp:colour-export-test");
        let out = photo_dcp(&bytes, "dcp:colour-export-test", "", None);
        lcc_raw::dcp::install_library(saved.iter().map(|(id, d)| (id.clone(), (**d).clone())).collect());
        assert_eq!(name.as_deref(), Some("Mine"));
        assert_eq!(raw.color.color_matrix[0], Some(mine));
        let back = lcc_raw::dcp::parse(&out.unwrap()).unwrap();
        let close = |a: lcc_raw::Mat3, b: lcc_raw::Mat3| a.0.iter().flatten().zip(b.0.iter().flatten()).all(|(x, y)| (x - y).abs() < 1e-4);
        assert!(close(back.color.color_matrix[0].unwrap(), mine));
    }

    #[test]
    fn hostile_input_is_an_error() {
        assert!(photo_dcp(b"", "", "", None).is_err());
        assert!(photo_dcp(b"II*\0garbage", "", "", None).is_err());
    }

    #[test]
    fn profile_names_follow_the_database_else_the_file() {
        assert_eq!(profile_model(Some("NIKON CORPORATION"), "NIKON D850"), "Nikon D850");
        assert_eq!(profile_model(Some("Nobody"), "Cam 1"), "Nobody Cam 1");
        assert_eq!(profile_model(Some("Nobody"), "Nobody Cam 1"), "Nobody Cam 1");
        assert_eq!(profile_model(None, "Cam 1"), "Cam 1");
    }

    #[test]
    fn a_look_cube_is_identity_at_defaults_and_carries_an_edit() {
        let parse = |s: &DevelopSettings| lcc_pipeline::lut::Lut3d::parse_cube(&look_cube(s, "t").unwrap()).unwrap();
        let mut neutral = DevelopSettings::default();
        // the default rendition of a rendered photo is the identity, up to the LUT's resolution
        let id = parse(&neutral);
        assert_eq!(id.size, CUBE_SIZE);
        let n = CUBE_SIZE;
        let mid = id.data[(n / 2) + n * ((n / 2) + n * (n / 2))];
        let want = (n / 2) as f32 / (n - 1) as f32;
        assert!(mid.iter().all(|v| (v - want).abs() < 0.02), "{mid:?} vs {want}");
        // an exposure push lifts mid grey; white balance never enters
        neutral.light.exposure = 1.0;
        let bright = parse(&neutral);
        assert!(bright.data[(n / 2) + n * ((n / 2) + n * (n / 2))][1] > mid[1] + 0.05);
        let mut warm = DevelopSettings::default();
        warm.wb.temp = 3000.0;
        warm.wb.mode = lcc_develop::WbMode::Custom;
        assert_eq!(parse(&warm).data, id.data);
    }
}
