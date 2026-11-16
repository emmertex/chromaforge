//! The decode as the app does it with a camera profile (`Dcp::apply` → `camera_transform_of` → profile tables),
//! staged so the fit can read the colour going into each table.
use lcc_color::{D50, D65, Mat3, PROPHOTO, REC2020, bradford};
use lcc_raw::ColorData;
use lcc_raw::profile::{HsvTable, ProfileLook};

/// Linear Rec.2020 D65 → linear ProPhoto (D50), as `ProfileTables`.
pub fn to_prophoto() -> Mat3 {
    PROPHOTO.from_xyz().mul(&bradford(D65, D50)).mul(&REC2020.to_xyz())
}

/// The camera model being fitted (DNG profile form).
#[derive(Clone, Debug)]
pub struct Model {
    pub cm: [Mat3; 2],
    /// `None`: the colour matrices alone (what a file without forward matrices decodes with).
    pub fm: Option<[Mat3; 2]>,
    pub hsm: Option<[HsvTable; 2]>,
}

/// A look on top of the camera model.
#[derive(Clone, Debug, Default)]
pub struct LookModel {
    pub offset: f64,
    pub table: Option<HsvTable>,
}

/// One photo's decode, resolved for its white.
#[derive(Clone, Debug)]
pub struct PhotoDecode {
    /// White-balanced camera RGB → linear Rec.2020 (white balance folded in: `m · (wb ⊙ camera)`).
    m: [[f64; 3]; 3],
    wb: [f64; 3],
    gain: f32,
    hsm: Option<HsvTable>,
    look: Option<HsvTable>,
    to: [[f32; 3]; 3],
    from: [[f32; 3]; 3],
    /// Calibration-1 weight of the photo's white.
    pub g: f64,
    pub white: lcc_color::Xy,
}

/// The stages of one colour through the decode (ProPhoto linear where named so).
#[derive(Clone, Copy, Debug)]
pub struct Stages {
    /// Into the hue/saturation map.
    pub hsm_in: [f32; 3],
    /// Into the look table (after the map and the exposure gain).
    pub look_in: [f32; 3],
    /// Out: scene-linear Rec.2020.
    pub scene: [f64; 3],
}

fn mat_apply_f32(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

impl Model {
    pub fn color_data(&self, look: &LookModel, file_baseline: f64, neutral: [f64; 3]) -> ColorData {
        ColorData {
            illuminant: [17, 21],
            color_matrix: self.cm.map(Some),
            forward_matrix: self.fm.map_or([None, None], |f| f.map(Some)),
            baseline_exposure: file_baseline + look.offset,
            as_shot_neutral: Some(neutral),
            profile: ProfileLook {
                hue_sat_map: self.hsm.clone().map(|[a, b]| [Some(a), Some(b)]).unwrap_or_default(),
                look_table: look.table.clone(),
                tone_curve: None,
                gain_table_map: None,
            },
            ..Default::default()
        }
    }

    pub fn photo(&self, look: &LookModel, file_baseline: f64, neutral: [f64; 3]) -> PhotoDecode {
        let color = self.color_data(look, file_baseline, neutral);
        let white = lcc_raw::color::as_shot_white_of(&color, None);
        let t = lcc_raw::color::camera_transform_of(&color, white);
        let g = lcc_raw::color::illuminant_weight(&color, white);
        let to = to_prophoto();
        let from = to.inverse().unwrap_or(Mat3::IDENTITY);
        PhotoDecode {
            m: t.matrix.0,
            wb: t.wb.map(f64::from),
            gain: 2f32.powf(t.baseline_exposure as f32),
            hsm: color.profile.hue_sat_for(g),
            look: look.table.clone(),
            to: to.to_f32(),
            from: from.to_f32(),
            g,
            white,
        }
    }
}

impl PhotoDecode {
    pub fn stages(&self, camera: [f32; 3]) -> Stages {
        let c = [0, 1, 2].map(|i| f64::from(camera[i]) * self.wb[i]);
        let rec: [f32; 3] = std::array::from_fn(|i| (self.m[i][0] * c[0] + self.m[i][1] * c[1] + self.m[i][2] * c[2]) as f32);
        let hsm_in = mat_apply_f32(&self.to, rec);
        let p1 = self.hsm.as_ref().map_or(hsm_in, |t| t.apply(hsm_in));
        let look_in = p1.map(|v| v * self.gain);
        let p3 = self.look.as_ref().map_or(look_in, |t| t.apply(look_in));
        let out = mat_apply_f32(&self.from, p3);
        Stages { hsm_in, look_in, scene: out.map(|v| if v.is_finite() { f64::from(v) } else { 0.0 }) }
    }

    pub fn scene(&self, camera: [f32; 3]) -> [f64; 3] {
        self.stages(camera).scene
    }

    /// ProPhoto (pre-gain) of a scene-linear Rec.2020 colour: the inverse of the last steps without the look table.
    pub fn gain(&self) -> f32 {
        self.gain
    }

    pub fn to_prophoto(&self, scene: [f64; 3]) -> [f32; 3] {
        mat_apply_f32(&self.to, scene.map(|v| v as f32))
    }
}
