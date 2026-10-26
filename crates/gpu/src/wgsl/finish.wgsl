// The per-pixel stage: a straight port of `lcc_pipeline::finish` (keep in step with it).
// Bindings: img (rgb, pre-exposure), log_l, base, clar, tex, dark, masks (NMASK planes, then the
// blurred chromaticity when HAS_CHROMA), aux
// (tone LUT | chroma curve | sRGB LUT | curve LUTs | mask terms), out (packed RGBA8; for a deep
// render, `DEEP`, also linear float R, G, B in the output primaries from word `DEEP_OFF`, and the
// packed pixel is their 8-bit reduction, `DeepImage::to_rgba8`). `PEAK` is the HDR render's peak
// (SDR white = 1; 1 for SDR renders).

fn tone_apply(y: f32) -> f32 {
    if (y <= 0.0) {
        return 0.0;
    }
    let ev = log2(y / GREY);
    let f = clamp((ev - TONE_MIN_EV) / (TONE_MAX_EV - TONE_MIN_EV), 0.0, 1.0) * f32(TONE_N - 1u);
    let i = min(u32(f), TONE_N - 2u);
    let t = f - f32(i);
    let v = aux[i] + (aux[i + 1u] - aux[i]) * t;
    if (ev < TONE_MIN_EV) {
        return v * (y / (GREY * TONE_MIN_GAIN));
    }
    return v;
}

// The camera chroma curve follows the tone LUT in `aux` (`ToneMap::chroma_scale`).
fn chroma_scale(o: f32) -> f32 {
    let f = clamp(o, 0.0, 1.0) * f32(CHROMA_N - 1u);
    let i = min(u32(f), CHROMA_N - 2u);
    let t = f - f32(i);
    let a = aux[TONE_N + i];
    let b = aux[TONE_N + i + 1u];
    if (a == b) {
        return a;
    }
    return a + (b - a) * t;
}

fn encode_srgb(v: f32) -> f32 {
    let o = pu(F_SRGB_OFF);
    let f = clamp(v, 0.0, 1.0) * f32(SRGB_N);
    let i = min(u32(f), SRGB_N - 1u);
    let t = f - f32(i);
    return aux[o + i] + (aux[o + i + 1u] - aux[o + i]) * t;
}

// The curve table at `o` in `aux` (`Lut1::eval`).
fn lut_at(o: u32, x: f32) -> f32 {
    let f = clamp(x, 0.0, 1.0) * f32(CURVE_N - 1u);
    let i = min(u32(f), CURVE_N - 2u);
    let t = f - f32(i);
    return aux[o + i] + (aux[o + i + 1u] - aux[o + i]) * t;
}

// `finish::eval_curves` on one channel: for HDR renders (`hdr`) a value above SDR white continues
// with slope 1 from the curve's end point.
fn lut_hdr(o: u32, x: f32, hdr: bool) -> f32 {
    if (hdr && x > 1.0) {
        return lut_at(o, 1.0) + (x - 1.0);
    }
    return lut_at(o, x);
}

fn curve(ch: u32, x: f32, hdr: bool) -> f32 {
    return lut_hdr(pu(F_CURVE_OFF) + ch * CURVE_N, x, hdr);
}

// The exact sRGB curve (`lcc_color::transfer`), unbounded above 1 as HDR renders encode.
fn lin_to_srgb(v: f32) -> f32 {
    if (v <= 0.0031308) {
        return v * 12.92;
    }
    return 1.055 * pow(v, 1.0 / 2.4) - 0.055;
}

fn srgb_to_lin(v: f32) -> f32 {
    if (v <= 0.04045) {
        return v / 12.92;
    }
    return pow((v + 0.055) / 1.055, 2.4);
}

fn band_weights(h: f32) -> array<f32, 8> {
    var w: array<f32, 8>;
    for (var i = 0u; i < 8u; i++) {
        let a = pf(F_BANDH + i);
        let b = pf(F_BANDH + (i + 1u) % 8u);
        let span = rem_euclid(wrap_angle(b - a), TAU);
        let d = rem_euclid(wrap_angle(h - a), TAU);
        if (d <= span) {
            let t = d / span;
            let s = 0.5 - 0.5 * cos(t * PI);
            w[i] += 1.0 - s;
            w[(i + 1u) % 8u] += s;
            break;
        }
    }
    return w;
}

// `PointK::apply` for Point Color sample `k` (OkLCh in, OkLCh out).
fn point_color(k: u32, lch: vec3<f32>) -> vec3<f32> {
    let o = F_PC + k * POINT_WORDS;
    let sl = pf(o);
    let sc = pf(o + 1u);
    let sh = pf(o + 2u);
    let var_k = pf(o + 6u);
    let wh_ = pf(o + 7u);
    let wc_ = pf(o + 8u);
    let wl_ = pf(o + 9u);
    var l = lch.x;
    var c = lch.y;
    var h = lch.z;
    var wh = 1.0;
    if (sc >= 0.02) {
        wh = (1.0 - sstep(0.5 * wh_, wh_, abs(wrap_angle(h - sh)))) * sstep(0.005, 0.025, c);
    }
    if (wh <= 0.0) {
        return lch;
    }
    let wc = 1.0 - sstep(0.5 * wc_, wc_, abs(c - sc));
    let wl = 1.0 - sstep(0.5 * wl_, wl_, abs(l - sl));
    let w = wh * wc * wl;
    if (w <= 0.0) {
        return lch;
    }
    h += w * (var_k * wrap_angle(h - sh) + pf(o + 3u));
    c += w * var_k * (c - sc);
    c = max(c * (1.0 + w * pf(o + 4u)), 0.0);
    l += w * (var_k * (l - sl) + pf(o + 5u));
    return vec3<f32>(l, c, h);
}

// `ColorOps::apply`.
fn color_ops(rgb: vec3<f32>, local_sat: f32, local_hue: f32) -> vec3<f32> {
    if (pu(F_OPS_IDENTITY) != 0u && local_sat == 0.0 && local_hue == 0.0) {
        return rgb;
    }
    let lab0 = oklab(rgb);
    var l = lab0.x;
    var c = sqrt(lab0.y * lab0.y + lab0.z * lab0.z);
    // a neutral (black or grey) pixel has no hue: atan2(0, 0) is undefined in WGSL (NaN on some
    // GPUs, which a later lightening such as the vignette then shows); the CPU's atan2 gives 0
    var h = select(atan2(lab0.z, lab0.y), 0.0, c == 0.0);
    if (pu(F_MIXER) != 0u) {
        let w = band_weights(h);
        var dh = 0.0;
        var ds = 0.0;
        var dl = 0.0;
        for (var i = 0u; i < 8u; i++) {
            dh += w[i] * pf(F_MIX_HUE + i);
            ds += w[i] * pf(F_MIX_SAT + i);
            dl += w[i] * pf(F_MIX_LUM + i);
        }
        let chroma_w = min(c / 0.12, 1.0);
        h += dh * chroma_w;
        c *= max(1.0 + ds, 0.0);
        l += dl * chroma_w * sqrt(max(l, 0.05));
    }
    for (var k = 0u; k < pu(F_NPC); k++) {
        let r = point_color(k, vec3<f32>(l, c, h));
        l = r.x;
        c = r.y;
        h = r.z;
    }
    let vib = pf(F_VIBRANCE);
    if (vib != 0.0) {
        let low = 1.0 - clamp(c / 0.22, 0.0, 1.0);
        var skin = 1.0;
        if (vib > 0.0) {
            let q = wrap_angle(h - pf(F_SKIN)) / 0.35;
            skin = 1.0 - 0.6 * exp(-(q * q));
        }
        c *= max(1.0 + vib * low * low * skin * 1.2, 0.0);
    }
    let sat = pf(F_SATURATION);
    if (sat != 0.0 || local_sat != 0.0) {
        c *= max(1.0 + sat + local_sat, 0.0);
    }
    h += local_hue;
    if (pu(F_BW) != 0u) {
        let w = band_weights(h);
        var mix = 0.0;
        for (var i = 0u; i < 8u; i++) {
            mix += w[i] * pf(F_BW_MIX + i);
        }
        l = max(l + mix * min(c / 0.2, 1.0) * 0.25, 0.0);
        c = 0.0;
    }
    var lab = vec3<f32>(l, c * cos(h), c * sin(h));
    if (pu(F_GRADING) != 0u) {
        let m = 0.5 - pf(F_BALANCE) * 0.25;
        let width = 0.15 + pf(F_BLENDING) * 0.5;
        let ws = 1.0 - sstep(m - width, m + width * 0.25, lab.x);
        let wh = sstep(m - width * 0.25, m + width, lab.x);
        let wm = max(1.0 - ws - wh, 0.0);
        let wts = array<f32, 4>(ws, wm, wh, 1.0);
        for (var k = 0u; k < 4u; k++) {
            let wt = wts[k];
            lab.y += pf(F_WHEELS + 3u * k) * wt;
            lab.z += pf(F_WHEELS + 3u * k + 1u) * wt;
            lab.x += pf(F_WHEELS + 3u * k + 2u) * wt;
        }
    }
    return oklab_inv(lab);
}

// `colorops::calibrate`: primaries matrix, then the shadows tint (luminance kept).
fn calibrate(c0: vec3<f32>) -> vec3<f32> {
    var c = c0;
    if (pu(F_CALIB) != 0u) {
        let m = array<vec3<f32>, 3>(
            vec3<f32>(pf(F_CALIB_M), pf(F_CALIB_M + 1u), pf(F_CALIB_M + 2u)),
            vec3<f32>(pf(F_CALIB_M + 3u), pf(F_CALIB_M + 4u), pf(F_CALIB_M + 5u)),
            vec3<f32>(pf(F_CALIB_M + 6u), pf(F_CALIB_M + 7u), pf(F_CALIB_M + 8u)),
        );
        c = max(mul3(m, c), vec3<f32>(0.0));
    }
    let st = pf(F_SHADOW_TINT);
    if (st != 0.0) {
        let y0 = lum2020(c);
        let w = 1.0 - sstep(-5.0, -0.5, log2(max(y0, 1e-7) / 0.18));
        c.y *= max(1.0 - SHADOW_TINT_K * st * w, 0.0);
        let y1 = max(lum2020(c), 1e-9);
        c = c * y0 / y1;
    }
    return c;
}

// `finish::refine_saturation`.
fn refine_saturation(before: vec3<f32>, after: vec3<f32>, refine: f32) -> vec3<f32> {
    let lw = vec3<f32>(0.2126, 0.7152, 0.0722);
    let y0 = dot(before, lw);
    let y1 = dot(after, lw);
    let s0 = (max(before.x, max(before.y, before.z)) - min(before.x, min(before.y, before.z))) / max(y0, 1e-4);
    let s1 = (max(after.x, max(after.y, after.z)) - min(after.x, min(after.y, after.z))) / max(y1, 1e-4);
    if (s1 <= 1e-6 || s0 <= 1e-6) {
        return after;
    }
    let k = clamp(pow(s0 / s1, 1.0 - refine), 0.0, 4.0);
    return y1 + (after - y1) * k;
}

fn ghash(i: i32, j: i32, seed: u32) -> f32 {
    var v = (bitcast<u32>(i) * GRAIN_H0) ^ (bitcast<u32>(j) * GRAIN_H1) ^ (seed * GRAIN_H2);
    v ^= v >> 13u;
    v = v * GRAIN_H3;
    v ^= v >> 15u;
    return f32(v & 0xffffu) / 32768.0 - 1.0;
}

fn grain_noise(x: f32, y: f32, seed: u32) -> f32 {
    let x0 = floor(x);
    let y0 = floor(y);
    let fx = x - x0;
    let fy = y - y0;
    let i = i32(x0);
    let j = i32(y0);
    let u = fx * fx * (3.0 - 2.0 * fx);
    let v = fy * fy * (3.0 - 2.0 * fy);
    let a = ghash(i, j, seed) + (ghash(i + 1, j, seed) - ghash(i, j, seed)) * u;
    let b = ghash(i, j + 1, seed) + (ghash(i + 1, j + 1, seed) - ghash(i, j + 1, seed)) * u;
    return a + (b - a) * v;
}

fn enc8(v: f32) -> u32 {
    return u32(clamp(v, 0.0, 1.0) * 255.0 + 0.5);
}

// sRGB-curve-encoded value → the output space's own curve (`OutputTrc`: 0 sRGB, 1 gamma, 2 Rec.709).
fn out_encode(v: f32) -> f32 {
    let kind = pu(F_OUT_TRC);
    if (kind == 0u) {
        return v;
    }
    let e = clamp(v, 0.0, 1.0);
    var l = e / 12.92;
    if (e > 0.04045) {
        l = pow((e + 0.055) / 1.055, 2.4);
    }
    if (kind == 1u) {
        return pow(l, 1.0 / pf(F_OUT_GAMMA));
    }
    if (l < 0.018) {
        return l * 4.5;
    }
    return 1.099 * pow(l, 0.45) - 0.099;
}

// The 3×3 matrix stored row by row from field `o`.
fn mat_at(o: u32) -> array<vec3<f32>, 3> {
    return array<vec3<f32>, 3>(
        vec3<f32>(pf(o), pf(o + 1u), pf(o + 2u)),
        vec3<f32>(pf(o + 3u), pf(o + 4u), pf(o + 5u)),
        vec3<f32>(pf(o + 6u), pf(o + 7u), pf(o + 8u)),
    );
}

// `finish::gamut_map` (SDR): the chroma scale that brings `r` into 0..1 towards its luminance (1: in gamut) ...
fn gamut_scale(r: vec3<f32>, luma: vec3<f32>, top: f32) -> f32 {
    let yy = clamp(dot(luma, r), 0.0, top);
    var tg = 1.0;
    for (var k = 0u; k < 3u; k++) {
        let cc = r[k];
        if (cc < 0.0) {
            tg = min(tg, yy / max(yy - cc, 1e-9));
        } else if (cc > top) {
            tg = min(tg, (top - yy) / max(cc - yy, 1e-9));
        }
    }
    return tg;
}

// ... and `r` scaled by it.
fn gamut_apply(r: vec3<f32>, luma: vec3<f32>, tg: f32, top: f32) -> vec3<f32> {
    if (tg < 1.0) {
        let yy = clamp(dot(luma, r), 0.0, top);
        return yy + (r - yy) * tg;
    }
    return r;
}

// `finish::out_of_gamut`: `r` needed desaturating by `t` to fit and isn't just a neutral beyond white or below black.
fn out_of_gamut(r: vec3<f32>, t: f32) -> bool {
    let lo = min(r.x, min(r.y, r.z));
    let hi = max(r.x, max(r.y, r.z));
    return t < 0.995 && (hi - lo) > 0.02 * max(abs(hi), 1e-3) && hi - lo > 1e-3 && (lo < -1e-3 || (hi > 1.0 && lo < 1.0));
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = pu(F_W);
    let h = pu(F_H);
    let x = gid.x;
    let y = gid.y + pu(F_Y0);
    if (x >= w || y >= h) {
        return;
    }
    let i = y * w + x;
    let n = w * h;
    let gain = pf(F_GAIN);
    let ev = pf(F_EV);
    var c = vec3<f32>(img[3u * i], img[3u * i + 1u], img[3u * i + 2u]);
    if (gain != 1.0) {
        c = c * gain;
    }
    let l_pre = log_l[i];
    let l0 = l_pre + ev;

    // --- local (mask) contributions
    var lt: array<f32, MASK_SUMS>;
    var tint_on = false;
    var tint_dir = vec2<f32>(0.0, 0.0);
    var tint_amt = 0.0;
    let nm = pu(F_NMASK);
    for (var m = 0u; m < nm; m++) {
        let a = masks[m * n + i];
        if (a <= 0.0) {
            continue;
        }
        let t = pu(F_MASK_OFF) + m * MASK_TERMS;
        for (var k = 0u; k < MASK_SUMS; k++) {
            lt[k] += a * aux[t + k];
        }
        if (aux[t + MASK_SUMS] > 0.0) {
            tint_on = true;
            tint_dir = vec2<f32>(aux[t + MASK_SUMS + 1u], aux[t + MASK_SUMS + 2u]);
            tint_amt = a * aux[t + MASK_SUMS + 3u];
        }
    }
    let l_exp = lt[0];
    let l_temp = lt[1];
    let l_tint = lt[2];
    let l_noise = lt[14];

    // --- local Moiré (and the colour part of Noise): chromaticity towards its blur
    let mt = clamp(lt[15] + 0.5 * max(l_noise, 0.0), -1.0, 1.0);
    if (mt != 0.0 && pu(F_HAS_CHROMA) != 0u) {
        let raw = vec3<f32>(img[3u * i], img[3u * i + 1u], img[3u * i + 2u]);
        let y = lum2020(c);
        let ch0 = raw / max(lum2020(raw), 1e-6);
        let o = nm * n + 3u * i;
        let cb = vec3<f32>(masks[o], masks[o + 1u], masks[o + 2u]);
        c = max((ch0 + (cb - ch0) * mt) * y, vec3<f32>(0.0));
    }
    // --- local Defringe: desaturate purple / green fringes along edges
    let df = clamp(lt[16], 0.0, 1.0);
    if (df > 0.0 && pu(F_HAS_TEX) != 0u) {
        let yd = max(lum2020(c), 1e-6);
        let purple = (min(c.x, c.z) - c.y) / yd;
        let green = (c.y - max(c.x, c.z)) / yd;
        let k = df * sstep(0.04, 0.3, abs(l_pre - tex[i])) * max(sstep(0.02, 0.2, purple), sstep(0.02, 0.2, green));
        if (k > 0.0) {
            let y = lum2020(c);
            c = c + (y - c) * k;
        }
    }

    // --- dehaze (scene linear)
    let dz = pf(F_DEHAZE) + lt[10];
    if (dz != 0.0 && pu(F_HAS_DARK) != 0u) {
        // the dark channel is a blur, not a patch minimum: never attribute more veil than the
        // pixel's own minimum channel, or dark pixels next to bright ones crush to black (#2504)
        let air = pf(F_AIR);
        let cmin = min(c.x, min(c.y, c.z));
        let d = clamp(min(select(0.0, dark[i] / pf(F_AIR_PRE), pf(F_AIR_PRE) > 0.0), select(0.0, cmin / air, air > 0.0)), 0.0, 1.0);
        if (dz > 0.0 && pu(F_DEHAZE_V3) != 0u) {
            // process V3 (`finish::dehaze_v3_pixel`)
            let k0 = clamp(dz, 0.0, 1.0);
            let k = k0 * (0.4 + 0.6 * k0);
            let t = max(1.0 - 0.95 * k * d, 0.3);
            let veil = min(air * (1.0 - t), 0.9 * max(cmin, 0.0));
            c = max((c - veil) / t, vec3<f32>(0.0));
        } else if (dz > 0.0) {
            let t = max(1.0 - 0.95 * min(dz, 1.0) * d, 0.12);
            c = max((c - air * (1.0 - t)) / t, vec3<f32>(0.0));
        } else {
            let k = min(-dz, 1.0) * 0.7 * (0.35 + 0.65 * d);
            c = c + (air * 0.9 - c) * k;
        }
    }

    // --- local exposure / temp / tint
    if (l_exp != 0.0) {
        c = c * exp2(l_exp);
    }
    if (l_temp != 0.0 || l_tint != 0.0) {
        let y0 = lum2020(c);
        c = vec3<f32>(c.x * (1.0 + 0.3 * l_temp), c.y * (1.0 - 0.22 * l_tint), c.z * max(1.0 - 0.3 * l_temp, 0.0));
        let y1 = max(lum2020(c), 1e-9);
        c = c * y0 / y1;
    }

    // --- local tone in log luminance
    var l1 = l0;
    if (dz != 0.0 || l_exp != 0.0) {
        l1 = log_lum(c);
    }
    let shift = l1 - l0;
    let base = base_p[i] + ev + shift;
    var delta = 0.0;
    let hh = pf(F_HL) + lt[4];
    let ss = pf(F_SH) + lt[5];
    if (hh != 0.0 || ss != 0.0) {
        let ws = 1.0 - sstep(-4.8, 0.3, base);
        let wh = sstep(-1.0, 2.8, base);
        delta += ss * 1.7 * ws * sqrt(ws) + hh * 1.7 * wh;
    }
    if (lt[6] != 0.0) {
        delta += lt[6] * 0.8 * sstep(0.5, 3.0, l1);
    }
    if (lt[7] != 0.0) {
        delta += lt[7] * 0.8 * (1.0 - sstep(-6.0, -1.5, l1));
    }
    if (lt[3] != 0.0) {
        delta += lt[3] * 0.14 * clamp(l1, -6.0, 4.0);
    }
    let cl = pf(F_CLAR) + lt[9];
    if (cl != 0.0 && pu(F_HAS_CLAR) != 0u) {
        let det = clamp(l_pre - clar[i], -2.5, 2.5);
        let q = base / 3.2;
        let mid = exp(-(q * q));
        delta += cl * 0.85 * det * (0.35 + 0.65 * mid);
    }
    let tx = pf(F_TEX) + lt[8];
    let v4 = pu(F_SHARPEN_V4) != 0u;
    // V1-V3 sharpen by boosting the texture band; V4 has its own pixel-scale band
    var sp_old = 0.0;
    if (!v4) {
        sp_old = lt[13] * 0.6 + pf(F_SHARPEN);
    }
    if ((tx != 0.0 || sp_old != 0.0) && pu(F_HAS_TEX) != 0u) {
        let det = l_pre - tex[i];
        let tame = 1.0 - 0.6 * sstep(0.4, 1.6, abs(det));
        delta += tx * 1.1 * clamp(det, -1.0, 1.0) * tame;
        if (sp_old != 0.0) {
            let sm = pf(F_SHARPEN_MASK);
            var mk = 1.0;
            if (sm > 0.0) {
                mk = sstep(sm * 0.25, sm * 0.25 + 0.15, abs(det));
            }
            delta += sp_old * 1.3 * clamp(det, -0.8, 0.8) * mk;
        }
    }
    // V4 sharpening: unsharp mask on the pixel-scale band (`sharpen_term` on the CPU)
    var sp = 0.0;
    if (v4) {
        sp = (lt[13] * 0.6 + pf(F_SHARPEN)) * pf(F_SHARPEN_FADE);
    }
    if (sp != 0.0 && pu(F_HAS_SHP) != 0u) {
        let det = l_pre - tex[pu(F_SHP_OFF) + i];
        let sm = pf(F_SHARPEN_MASK);
        var mk = 1.0;
        if (sm > 0.0) {
            mk = sstep(sm * 0.1, sm * 0.1 + 0.06, abs(det));
        }
        delta += sp * 7.0 * mk * det / (1.0 + abs(det) / pf(F_SHARPEN_LIMIT));
    }
    // local Noise: smooth (or, negative, boost) small-amplitude detail, keep edges
    if (l_noise != 0.0 && pu(F_HAS_TEX) != 0u) {
        let det = l_pre - tex[i];
        delta -= clamp(l_noise, -1.0, 1.0) * 0.9 * det * (1.0 - sstep(0.1, 0.5, abs(det)));
    }
    if (delta != 0.0) {
        c = c * exp2(delta);
    }

    // --- calibration (scene linear, before the tone map)
    if (pu(F_CALIB) != 0u || pf(F_SHADOW_TINT) != 0.0) {
        c = calibrate(c);
    }

    // --- tone map on luminance, highlight desaturation
    let yl = lum2020(c);
    let o = tone_apply(yl);
    var d = vec3<f32>(0.0);
    if (yl > 1e-9) {
        d = c * o / yl;
    }
    let k = chroma_scale(o);
    if (k != 1.0) {
        d = vec3<f32>(o) + (d - vec3<f32>(o)) * k;
    }
    let peak = pf(F_PEAK);
    let hdr = peak > 1.0;
    let deep = pu(F_DEEP) != 0u;
    let mx = max(d.x, max(d.y, d.z));
    if (mx > peak) {
        let t = clamp((mx - peak) / max(mx - o, 1e-6), 0.0, 1.0);
        let grey = d + (o - d) * t;
        if (pu(F_HUE_CLIP) != 0u) {
            let kept = hue_clip(d, peak);
            d = kept + (grey - kept) * hue_fade(o, mx, peak);
        } else {
            d = grey;
        }
    }

    // --- colour
    d = color_ops(d, lt[11], lt[12]);
    if (tint_on) {
        let lab = oklab(d);
        d = oklab_inv(vec3<f32>(lab.x, lab.y + tint_dir.x * 0.08 * tint_amt, lab.z + tint_dir.y * 0.08 * tint_amt));
    }

    // --- vignette (display linear, post-crop)
    if (pu(F_VIG) != 0u) {
        // the vignette belongs to the whole output frame, not to the window being drawn
        let fw = pf(F_VIG_VIEW + 2u);
        let fh = pf(F_VIG_VIEW + 3u);
        let aspect = fw / fh;
        let amount = pf(F_VIG_AMOUNT);
        let mixa = pf(F_VIG_ASPECT_MIX);
        let power = pf(F_VIG_POWER);
        let u = (pf(F_VIG_VIEW) + f32(x) + 0.5) / fw * 2.0 - 1.0;
        let vv = (pf(F_VIG_VIEW + 1u) + f32(y) + 0.5) / fh * 2.0 - 1.0;
        let sx = 1.0 + (aspect - 1.0) * mixa;
        let sy = 1.0 + (1.0 / aspect - 1.0) * mixa;
        let ax = abs(u * max(sx, 1.0) / max(sx, sy));
        let ay = abs(vv * max(sy, 1.0) / max(sx, sy));
        let dist = pow(pow(ax, power) + pow(ay, power), 1.0 / power);
        let start = pf(F_VIG_START);
        let t = sstep(start, start + pf(F_VIG_WIDTH), dist);
        if (t > 0.0) {
            let lum = clamp(lum2020(d), 0.0, 1.0);
            if (amount < 0.0) {
                var f = 1.0 + amount * t;
                let style = pu(F_VIG_STYLE);
                if (style == 1u) {
                    f += (1.0 - f) * pf(F_VIG_HL) * sstep(0.4, 1.0, lum);
                }
                if (style == 2u) {
                    d = d * (1.0 - (-amount) * t) + 0.0;
                } else {
                    d = d * f;
                }
            } else if (hdr) {
                // lighten towards SDR white; never pull HDR highlights down to it
                d = d + max(vec3<f32>(1.0) - d, vec3<f32>(0.0)) * amount * t * 0.85;
            } else {
                d = d + (1.0 - d) * amount * t * 0.85;
            }
        }
    }

    // --- gamut map to the output space (desaturate towards luminance until in range);
    // soft proofing maps into the proof space first and shows that in the output space
    // (warn: 0 none, 1 destination gamut, 2 display gamut)
    var warn = 0u;
    var r: vec3<f32>;
    if (pu(F_PROOF) != 0u) {
        let q0 = mul3(mat_at(F_PROOF_M), d);
        let tq = gamut_scale(q0, vec3<f32>(pf(F_PROOF_Y), pf(F_PROOF_Y + 1u), pf(F_PROOF_Y + 2u)), 1.0);
        if (pu(F_PROOF_DEST_WARN) != 0u && out_of_gamut(q0, tq)) {
            warn = 1u;
        }
        let py = vec3<f32>(pf(F_PROOF_Y), pf(F_PROOF_Y + 1u), pf(F_PROOF_Y + 2u));
        r = mul3(mat_at(F_PROOF_OUT), keep_hue_in_gamut(q0, gamut_apply(q0, py, tq, 1.0), py, 1.0));
    } else {
        r = mul3(mat_at(F_OUT_M), d);
    }
    let oy = vec3<f32>(pf(F_OUT_Y), pf(F_OUT_Y + 1u), pf(F_OUT_Y + 2u));
    let tg = gamut_scale(r, oy, peak);
    if (pu(F_PROOF) != 0u && pu(F_PROOF_DISP_WARN) != 0u && warn == 0u && out_of_gamut(r, tg)) {
        warn = 2u;
    }
    r = keep_hue_in_gamut(r, gamut_apply(r, oy, tg, peak), oy, peak);

    // --- encode, curves, grain
    var e: vec3<f32>;
    if (hdr) {
        // HDR: the (unbounded) sRGB curve, so values above SDR white encode above 1
        let q = max(r, vec3<f32>(0.0));
        e = vec3<f32>(lin_to_srgb(q.x), lin_to_srgb(q.y), lin_to_srgb(q.z));
    } else if (deep) {
        let q = clamp(r, vec3<f32>(0.0), vec3<f32>(1.0));
        e = vec3<f32>(lin_to_srgb(q.x), lin_to_srgb(q.y), lin_to_srgb(q.z));
    } else {
        e = vec3<f32>(encode_srgb(r.x), encode_srgb(r.y), encode_srgb(r.z));
    }
    if (pu(F_CURVES) != 0u) {
        let e0 = e;
        e = vec3<f32>(curve(0u, e.x, hdr), curve(1u, e.y, hdr), curve(2u, e.z, hdr));
        let rs = pf(F_REFINE_SAT);
        if (rs < 1.0) {
            e = refine_saturation(e0, e, rs);
        }
    }
    // --- the masks' local Curves, in mask order, each weighted by its mask (`finish::local_curve_luts`)
    if (pu(F_LCURVES) != 0u) {
        let so = pu(F_LCURVE_OFF);
        for (var m = 0u; m < nm; m++) {
            let slot = aux[so + m];
            let a = masks[m * n + i];
            if (slot < 0.0 || a <= 0.0) {
                continue;
            }
            let o = so + nm + u32(slot) * 3u * CURVE_N;
            let c = vec3<f32>(lut_hdr(o, e.x, hdr), lut_hdr(o + CURVE_N, e.y, hdr), lut_hdr(o + 2u * CURVE_N, e.z, hdr));
            e = e + (c - e) * a;
        }
    }
    if (pu(F_GRAIN) != 0u) {
        let px = f32(x) + 0.5;
        let py = f32(y) + 0.5;
        let gx = pf(F_GRAIN_AFF) * px + pf(F_GRAIN_AFF + 2u) * py + pf(F_GRAIN_AFF + 4u);
        let gy = pf(F_GRAIN_AFF + 1u) * px + pf(F_GRAIN_AFF + 3u) * py + pf(F_GRAIN_AFF + 5u);
        let sc = pf(F_GRAIN_SC);
        let rough = pf(F_GRAIN_ROUGH);
        let seed = pu(F_GRAIN_SEED);
        var g = grain_noise(gx * sc, gy * sc, seed);
        g = g * (1.0 - rough * 0.5) + grain_noise(gx * sc * 2.3, gy * sc * 2.3, seed ^ 0x55u) * rough * 0.7;
        var lum = 0.2126 * e.x + 0.7152 * e.y + 0.0722 * e.z;
        if (hdr) {
            lum = clamp(lum, 0.0, 1.0);
        }
        let k = pf(F_GRAIN_AMT) * g * (0.35 + 2.6 * lum * (1.0 - lum));
        e = e + k;
    }
    if (warn == 1u) {
        e = vec3<f32>(1.0, 0.0, 0.0);
    } else if (warn == 2u) {
        e = vec3<f32>(0.0, 0.25, 1.0);
    }
    if (deep) {
        // linear in the output primaries (`finish::finish_deep`): up to `peak` (HDR) or 1
        let q = max(e, vec3<f32>(0.0));
        let lin = clamp(vec3<f32>(srgb_to_lin(q.x), srgb_to_lin(q.y), srgb_to_lin(q.z)), vec3<f32>(0.0), vec3<f32>(peak));
        let o = pu(F_DEEP_OFF) + 3u * i;
        out[o] = bitcast<u32>(lin.x);
        out[o + 1u] = bitcast<u32>(lin.y);
        out[o + 2u] = bitcast<u32>(lin.z);
        let sdr = clamp(lin, vec3<f32>(0.0), vec3<f32>(1.0));
        e = vec3<f32>(lin_to_srgb(sdr.x), lin_to_srgb(sdr.y), lin_to_srgb(sdr.z));
    }
    e = vec3<f32>(out_encode(e.x), out_encode(e.y), out_encode(e.z));
    out[i] = enc8(e.x) | (enc8(e.y) << 8u) | (enc8(e.z) << 16u) | (255u << 24u);
}

// `finish::hue_clip` (process V5): the brightest channel limited to `peak`, the darkest kept (all three `peak` when it
// is above too), the middle one in its place between them
fn hue_clip(c: vec3<f32>, peak: f32) -> vec3<f32> {
    let hi = max(c.x, max(c.y, c.z));
    if (!(hi > peak)) {
        return c;
    }
    let lo = min(c.x, min(c.y, c.z));
    if (lo >= peak) {
        return vec3<f32>(peak);
    }
    return vec3<f32>(lo) + (c - vec3<f32>(lo)) * ((peak - lo) / (hi - lo));
}

// `finish::HUE_FADE_FROM` and `HUE_FADE_EV` (keep in step)
const HUE_FADE_FROM: f32 = 0.35;
const HUE_FADE_EV: f32 = 2.0;

// `finish::hue_fade`
fn hue_fade(y: f32, hi: f32, peak: f32) -> f32 {
    let over = select(0.0, log2(hi / peak), hi > peak && peak > 0.0);
    return max(sstep(HUE_FADE_FROM * peak, peak, y), sstep(0.0, HUE_FADE_EV, over));
}

// `finish::keep_hue_in_gamut`, or `mapped` itself before process V5
fn keep_hue_in_gamut(r: vec3<f32>, mapped: vec3<f32>, luma: vec3<f32>, peak: f32) -> vec3<f32> {
    if (pu(F_HUE_CLIP) == 0u) {
        return mapped;
    }
    let c = hue_clip(r, peak);
    // `finish::lift_negatives`
    let yy = clamp(dot(luma, c), 0.0, peak);
    var t = 1.0;
    for (var k = 0u; k < 3u; k++) {
        if (c[k] < 0.0) {
            t = min(t, yy / max(yy - c[k], 1e-9));
        }
    }
    var kept = c;
    if (t < 1.0) {
        kept = yy + (c - yy) * t;
    }
    return kept + (mapped - kept) * hue_fade(dot(luma, r), max(r.x, max(r.y, r.z)), peak);
}
