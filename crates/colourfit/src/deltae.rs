//! CIEDE2000 (Sharma, Wu, Dalal 2005) and Lab helpers.

/// CIELAB of linear XYZ relative to D50 (Y = 1 white).
pub fn lab(xyz: [f64; 3]) -> [f64; 3] {
    lcc_color::lab::xyz_d50_to_lab(xyz)
}

/// CIEDE2000 colour difference between two Lab colours (kL = kC = kH = 1).
pub fn de2000(a: [f64; 3], b: [f64; 3]) -> f64 {
    let [l1, a1, b1] = a;
    let [l2, a2, b2] = b;
    let c1 = a1.hypot(b1);
    let c2 = a2.hypot(b2);
    let cm = (c1 + c2) / 2.0;
    let cm7 = cm.powi(7);
    let g = 0.5 * (1.0 - (cm7 / (cm7 + 25f64.powi(7))).sqrt());
    let a1p = (1.0 + g) * a1;
    let a2p = (1.0 + g) * a2;
    let c1p = a1p.hypot(b1);
    let c2p = a2p.hypot(b2);
    let hue = |b: f64, a: f64| if b == 0.0 && a == 0.0 { 0.0 } else { b.atan2(a).to_degrees().rem_euclid(360.0) };
    let h1p = hue(b1, a1p);
    let h2p = hue(b2, a2p);
    let dl = l2 - l1;
    let dc = c2p - c1p;
    let dh = if c1p * c2p == 0.0 {
        0.0
    } else {
        let d = h2p - h1p;
        if d > 180.0 {
            d - 360.0
        } else if d < -180.0 {
            d + 360.0
        } else {
            d
        }
    };
    let dhh = 2.0 * (c1p * c2p).sqrt() * (dh.to_radians() / 2.0).sin();
    let lm = (l1 + l2) / 2.0;
    let cmp = (c1p + c2p) / 2.0;
    let hm = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        (h1p + h2p) / 2.0
    } else if h1p + h2p < 360.0 {
        (h1p + h2p + 360.0) / 2.0
    } else {
        (h1p + h2p - 360.0) / 2.0
    };
    let t = 1.0 - 0.17 * (hm - 30.0).to_radians().cos() + 0.24 * (2.0 * hm).to_radians().cos() + 0.32 * (3.0 * hm + 6.0).to_radians().cos()
        - 0.20 * (4.0 * hm - 63.0).to_radians().cos();
    let dtheta = 30.0 * (-((hm - 275.0) / 25.0).powi(2)).exp();
    let cmp7 = cmp.powi(7);
    let rc = 2.0 * (cmp7 / (cmp7 + 25f64.powi(7))).sqrt();
    let l50 = (lm - 50.0).powi(2);
    let sl = 1.0 + 0.015 * l50 / (20.0 + l50).sqrt();
    let sc = 1.0 + 0.045 * cmp;
    let sh = 1.0 + 0.015 * cmp * t;
    let rt = -(2.0 * dtheta).to_radians().sin() * rc;
    let (x, y, z) = (dl / sl, dc / sc, dhh / sh);
    let d2 = x * x + y * y + z * z + rt * y * z;
    d2.max(0.0).sqrt()
}

/// The `q` quantile (0..=1, nearest rank on the sorted values) of the finite values; 0 when there are none.
pub fn quantile(values: &[f64], q: f64) -> f64 {
    let mut v: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    let i = ((v.len() - 1) as f64 * q.clamp(0.0, 1.0)).round() as usize;
    v.get(i).copied().unwrap_or(0.0)
}

pub fn median(values: &[f64]) -> f64 {
    quantile(values, 0.5)
}

pub fn p95(values: &[f64]) -> f64 {
    quantile(values, 0.95)
}

/// Median and 95th percentile of a set of ΔE00 values (capped at 200, the calibration format's bound).
pub fn summary(values: &[f64]) -> lcc_camdb::calibration::DeltaE {
    lcc_camdb::calibration::DeltaE { median: median(values).min(200.0) as f32, p95: p95(values).min(200.0) as f32 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deltae_matches_sharma_test_pairs() {
        // pairs 1, 7, 17, 25 and 31 of the published test data
        let pairs = [
            ([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485], 2.0425),
            ([50.0, 0.0, 0.0], [50.0, -1.0, 2.0], 2.3669),
            ([50.0, 2.5, 0.0], [73.0, 25.0, -18.0], 27.1492),
            ([60.2574, -34.0099, 36.2677], [60.4626, -34.1751, 39.4387], 1.2644),
            ([90.8027, -2.0831, 1.4410], [91.1528, -1.6435, 0.0447], 1.4441),
        ];
        for (a, b, want) in pairs {
            assert!((de2000(a, b) - want).abs() < 1e-3, "{a:?} {b:?}: {} vs {want}", de2000(a, b));
            assert!((de2000(b, a) - want).abs() < 1e-3);
        }
    }

    #[test]
    fn deltae_quantiles_ignore_non_finite() {
        let v = [3.0, 1.0, f64::NAN, 2.0];
        assert_eq!(median(&v), 2.0);
        assert_eq!(quantile(&[], 0.5), 0.0);
    }
}
