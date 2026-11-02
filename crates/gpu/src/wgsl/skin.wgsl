// Smooth Skin (`lcc_pipeline::skin`): the bands' input, the reduced copies of the wide bands
// and the combine step, over one bucket's region of the scene-linear image. The coverage plane and
// the regions are worked out on the CPU (`skin::plan`); the blurs are the box-Gaussian of
// `blur.wgsl`. Each function mirrors its CPU twin (same constants, the same fast log and exp).
// Bindings: img (the image, 3 floats a pixel, changed in place by `skin_combine`), cov (the
// coverage over the box at its origin), lab (the region's bands' input, 4 floats a pixel; the
// output of `skin_reduce`), fine (the fine band; the input of `skin_reduce`), mid, low.

const SQRT2: f32 = 1.4142135;
const LOG2E: f32 = 1.442695;
const LN2: f32 = 0.6931472;

fn finite(x: f32) -> bool {
    return (bitcast<u32>(x) & 0x7f800000u) != 0x7f800000u;
}

// `skin::fast_log2` (x positive and finite).
fn fast_log2(x: f32) -> f32 {
    let bits = bitcast<u32>(x);
    var e = i32((bits >> 23u) & 0xffu) - 127;
    var m = bitcast<f32>((bits & 0x007fffffu) | 0x3f800000u);
    if (m > SQRT2) {
        m = m * 0.5;
        e = e + 1;
    }
    let s = (m - 1.0) / (m + 1.0);
    let s2 = s * s;
    return f32(e) + 2.88539 * s * (1.0 + s2 * (1.0 / 3.0 + s2 * (0.2 + s2 * (1.0 / 7.0))));
}

// `f32::round`: halves away from zero (WGSL's `round` takes them to even).
fn round_away(x: f32) -> f32 {
    return select(ceil(x - 0.5), floor(x + 0.5), x >= 0.0);
}

// `skin::fast_exp2`.
fn fast_exp2(x: f32) -> f32 {
    if ((bitcast<u32>(x) & 0x7fffffffu) > 0x7f800000u) {
        return 1.0; // NaN
    }
    if (x < -126.0) {
        return 0.0;
    }
    let y = min(x, 126.0);
    let i = round_away(y);
    let f = (y - i) * LN2;
    let p = 1.0 + f * (1.0 + f * (0.5 + f * (1.0 / 6.0 + f * (1.0 / 24.0 + f * (1.0 / 120.0)))));
    return bitcast<f32>(u32(i32(i) + 127) << 23u) * p;
}

// `skin::to_lab`: log luminance and log colour ratios; `ok` false for what the bands can't describe.
struct Lab {
    v: vec3<f32>,
    ok: bool,
}

fn to_lab(p: vec3<f32>, k: vec4<f32>) -> Lab {
    if (!(p.x > 0.0 && p.y > 0.0 && p.z > 0.0 && finite(p.x) && finite(p.y) && finite(p.z))) {
        return Lab(vec3<f32>(0.0), false);
    }
    let y = k.x * p.x + k.y * p.y + k.z * p.z;
    let lg = fast_log2(p.y + k.w);
    return Lab(vec3<f32>(fast_log2(y + k.w), fast_log2(p.x + k.w) - lg, fast_log2(p.z + k.w) - lg), true);
}

// `skin::shifted`.
fn shifted(p: vec3<f32>, dl: f32, da: f32, db: f32, k: vec4<f32>) -> vec3<f32> {
    let y = k.x * p.x + k.y * p.y + k.z * p.z;
    let rg = p.x / p.y * fast_exp2(da);
    let bg = p.z / p.y * fast_exp2(db);
    let y2 = y * fast_exp2(dl);
    let g2 = y2 / (k.x * rg + k.y + k.z * bg);
    let o = vec3<f32>(g2 * rg, g2, g2 * bg);
    if (finite(o.x) && finite(o.y) && finite(o.z)) {
        return o;
    }
    return p;
}

// `skin::small`.
fn small(x: f32, k: f32) -> f32 {
    return fast_exp2(-(x * x) / (2.0 * k * k) * LOG2E);
}

// `skin::norm`.
fn norm(v: vec4<f32>, fallback: vec3<f32>) -> vec3<f32> {
    if (v.w > 1e-3) {
        let r = 1.0 / v.w;
        return vec3<f32>(v.x * r, v.y * r, v.z * r);
    }
    return fallback;
}

fn lab4(i: u32) -> vec4<f32> {
    let j = i * 4u;
    return vec4<f32>(lab[j], lab[j + 1u], lab[j + 2u], lab[j + 3u]);
}

fn fine4(i: u32) -> vec4<f32> {
    let j = i * 4u;
    return vec4<f32>(fine[j], fine[j + 1u], fine[j + 2u], fine[j + 3u]);
}

fn mid4(i: u32) -> vec4<f32> {
    let j = i * 4u;
    return vec4<f32>(mid[j], mid[j + 1u], mid[j + 2u], mid[j + 3u]);
}

fn low4(i: u32) -> vec4<f32> {
    let j = i * 4u;
    return vec4<f32>(low[j], low[j + 1u], low[j + 2u], low[j + 3u]);
}

// The skin's coverage at image pixel (gx, gy): 0 outside the box at (ox, oy) of cw × ch.
fn coverage(gx: u32, gy: u32, ox: u32, oy: u32, cw: u32, ch: u32) -> f32 {
    if (gx < ox || gy < oy) {
        return 0.0;
    }
    let cx = gx - ox;
    let cy = gy - oy;
    if (cx >= cw || cy >= ch) {
        return 0.0;
    }
    return cov[cy * cw + cx];
}

// `Reduced::at`: the band of reduction `f` (a w × h copy, 1: the region itself) at region pixel (x, y).
fn band_at(which: u32, x: u32, y: u32, f: u32, w: u32, h: u32) -> vec4<f32> {
    if (f == 1u) {
        let i = y * w + x;
        if (which == 0u) {
            return mid4(i);
        }
        return low4(i);
    }
    if (w == 0u || h == 0u) {
        return vec4<f32>(0.0);
    }
    let ff = f32(f);
    let fx = clamp((f32(x) + 0.5) / ff - 0.5, 0.0, f32(w - 1u));
    let fy = clamp((f32(y) + 0.5) / ff - 0.5, 0.0, f32(h - 1u));
    let x0 = u32(fx);
    let y0 = u32(fy);
    let x1 = min(x0 + 1u, w - 1u);
    let y1 = min(y0 + 1u, h - 1u);
    let tx = fx - f32(x0);
    let ty = fy - f32(y0);
    var a: vec4<f32>;
    var b: vec4<f32>;
    var c: vec4<f32>;
    var d: vec4<f32>;
    if (which == 0u) {
        a = mid4(y0 * w + x0);
        b = mid4(y0 * w + x1);
        c = mid4(y1 * w + x0);
        d = mid4(y1 * w + x1);
    } else {
        a = low4(y0 * w + x0);
        b = low4(y0 * w + x1);
        c = low4(y1 * w + x0);
        d = low4(y1 * w + x1);
    }
    return (a * (1.0 - tx) + b * tx) * (1.0 - ty) + (c * (1.0 - tx) + d * tx) * ty;
}

// The bands' input over the region: log luminance and colour ratios premultiplied by the
// coverage, and the coverage. P: w, rx0, ry0, rw, rh, ox, oy, cw, ch, min coverage, KR, KG, KB, EPS.
@compute @workgroup_size(16, 16)
fn skin_lab(@builtin(global_invocation_id) g: vec3<u32>) {
    let w = pu(0u);
    let rw = pu(3u);
    let rh = pu(4u);
    if (g.x >= rw || g.y >= rh) {
        return;
    }
    let gx = pu(1u) + g.x;
    let gy = pu(2u) + g.y;
    let o = (g.y * rw + g.x) * 4u;
    let m = coverage(gx, gy, pu(5u), pu(6u), pu(7u), pu(8u));
    var out = vec4<f32>(0.0);
    if (m >= pf(9u)) {
        let i = (gy * w + gx) * 3u;
        let l = to_lab(vec3<f32>(img[i], img[i + 1u], img[i + 2u]), vec4<f32>(pf(10u), pf(11u), pf(12u), pf(13u)));
        if (l.ok) {
            out = vec4<f32>(l.v * m, m);
        }
    }
    lab[o] = out.x;
    lab[o + 1u] = out.y;
    lab[o + 2u] = out.z;
    lab[o + 3u] = out.w;
}

// `skin::reduce`: f × f box means of the 4-channel `fine` (sw × sh) into `lab` (dw × dh); the last
// row and column average what they have. P: sw, sh, f, dw, dh.
@compute @workgroup_size(16, 16)
fn skin_reduce(@builtin(global_invocation_id) g: vec3<u32>) {
    let sw = pu(0u);
    let sh = pu(1u);
    let f = pu(2u);
    let dw = pu(3u);
    let dh = pu(4u);
    if (g.x >= dw || g.y >= dh) {
        return;
    }
    var acc = vec4<f32>(0.0);
    var n = 0.0;
    for (var sy = g.y * f; sy < min((g.y + 1u) * f, sh); sy++) {
        for (var sx = g.x * f; sx < min((g.x + 1u) * f, sw); sx++) {
            acc += fine4(sy * sw + sx);
            n += 1.0;
        }
    }
    if (n > 0.0) {
        acc = acc / n;
    }
    let o = (g.y * dw + g.x) * 4u;
    lab[o] = acc.x;
    lab[o + 1u] = acc.y;
    lab[o + 2u] = acc.z;
    lab[o + 3u] = acc.w;
}

// The face pixel (x, y) belongs to (`skin::owner`): the nearest in units of its own size, the
// first of equals. Faces from word `base`: x, y, d each.
fn owner(x: f32, y: f32, n: u32, base: u32) -> u32 {
    var best = 0u;
    var best_v = 3.4e38;
    for (var i = 0u; i < n; i++) {
        let fx = pf(base + i * 3u);
        let fy = pf(base + i * 3u + 1u);
        let fd = pf(base + i * 3u + 2u);
        let v = sqrt((x - fx) * (x - fx) + (y - fy) * (y - fy)) / fd;
        if (v < best_v) {
            best_v = v;
            best = i;
        }
    }
    return best;
}

// The combine step over the region, in place on `img` (`skin::smooth_bucket`). P: w, rx0, ry0,
// rw, rh, ox, oy, cw, ch, mid f, mid w, mid h, low f, low w, low h, all, faces, members' mask (2
// words), then floats from word 19: amount, k_mid, k_low, k_chroma, tex, even, even_c, red,
// shine, min coverage, shine margin, shine strength, KR, KG, KB, EPS; then the faces (x, y, d).
@compute @workgroup_size(16, 16)
fn skin_combine(@builtin(global_invocation_id) g: vec3<u32>) {
    let w = pu(0u);
    let rw = pu(3u);
    let rh = pu(4u);
    if (g.x >= rw || g.y >= rh) {
        return;
    }
    let gx = pu(1u) + g.x;
    let gy = pu(2u) + g.y;
    let m = coverage(gx, gy, pu(5u), pu(6u), pu(7u), pu(8u));
    let f0 = 19u;
    if (m < pf(f0 + 9u)) {
        return;
    }
    if (pu(15u) == 0u) {
        let o = owner(f32(gx) + 0.5, f32(gy) + 0.5, pu(16u), f0 + 16u);
        if (o >= 64u) {
            return;
        }
        if (((pu(17u + o / 32u) >> (o % 32u)) & 1u) == 0u) {
            return;
        }
    }
    let ri = g.y * rw + g.x;
    let v = lab4(ri);
    if (v.w <= 0.0) {
        return;
    }
    let ii = (gy * w + gx) * 3u;
    let px = vec3<f32>(img[ii], img[ii + 1u], img[ii + 2u]);
    let amount = pf(f0);
    let k_mid = pf(f0 + 1u);
    let k_low = pf(f0 + 2u);
    let k_chroma = pf(f0 + 3u);
    let tex = pf(f0 + 4u);
    let even = pf(f0 + 5u);
    let even_c = pf(f0 + 6u);
    let red = pf(f0 + 7u);
    let shine_amt = pf(f0 + 8u);
    let margin = pf(f0 + 10u);
    let strength = pf(f0 + 11u);
    let k = vec4<f32>(pf(f0 + 12u), pf(f0 + 13u), pf(f0 + 14u), pf(f0 + 15u));
    let r = 1.0 / v.w;
    let v0 = vec3<f32>(v.x * r, v.y * r, v.z * r);
    let v1 = norm(fine4(ri), v0);
    let v2 = norm(band_at(0u, g.x, g.y, pu(9u), pu(10u), pu(11u)), v1);
    let v3 = norm(band_at(1u, g.x, g.y, pu(12u), pu(13u), pu(14u)), v2);
    let hi = v0.x - v1.x;
    let md = v1.x - v2.x;
    let lo = v2.x - v3.x;
    var dl = -amount * md * small(md, k_mid) - even * lo * small(lo, k_low) + tex * hi;
    let hot = max(md + lo - margin, 0.0);
    let shine = shine_amt * min(hot * 3.0, 1.0);
    dl -= shine_amt * strength * hot;
    let va = v1.y - v3.y;
    let vb = v1.z - v3.z;
    let ma = v1.y - v2.y;
    let mb = v1.z - v2.z;
    let da = -even_c * va * small(va, k_chroma) - red * max(va, 0.0) * (1.0 - even_c) - 0.5 * amount * ma - shine * 0.6 * va;
    let db = -even_c * vb * small(vb, k_chroma) - 0.5 * amount * mb - shine * 0.6 * vb;
    let out = shifted(px, dl, da, db, k);
    let res = px + m * (out - px);
    img[ii] = res.x;
    img[ii + 1u] = res.y;
    img[ii + 2u] = res.z;
}
