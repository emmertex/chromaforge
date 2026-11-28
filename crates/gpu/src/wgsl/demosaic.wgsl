// The AHD demosaic of a Bayer mosaic (`lcc_raw::demosaic::ahd`, which is the reference; Hirakawa & Parks 2005),
// on one strip of the mosaic padded by AHD_MARGIN on every side by mirror reflection, as the CPU pads its tiles.
// Four passes over the padded strip, each as deep inside it as the CPU's tile computes it (the greens 3 from its
// edge, the candidates' Lab 4, the homogeneity 5, the choice 6), so every pixel sees the numbers the CPU's does.
//
// P: [padded width, padded height, colour of each (row & 1, column & 1) of the padded strip (2 bits each, row
// parity first: index (y & 1) * 2 + (x & 1))].
// mos: the padded mosaic; gr: the horizontal and vertical green of each pixel; lab: the two candidates' CIELab
// (6 per pixel); homo: the two homogeneity counts (horizontal in bits 0-7, vertical in 8-15); rgb_out: the
// strip's camera RGB, unpadded, 3 per pixel.

const AHD_MARGIN: u32 = 6u;

fn ahd_bw() -> u32 {
    return pu(0u);
}

fn ahd_col(i: u32) -> u32 {
    let w = ahd_bw();
    let k = ((i / w) & 1u) * 2u + ((i % w) & 1u);
    return (pu(2u) >> (k * 2u)) & 3u;
}

// `i` is at least `d` from every edge of the padded strip.
fn ahd_inside(i: u32, d: u32) -> bool {
    let w = ahd_bw();
    let h = pu(1u);
    if i >= w * h {
        return false;
    }
    let x = i % w;
    let y = i / w;
    return x >= d && x + d < w && y >= d && y + d < h;
}

// `demosaic::clamp_between`: `v` clamped between `a` and `b`, as is when they are NaN.
fn ahd_between(v: f32, a: f32, b: f32) -> f32 {
    let lo = min(a, b);
    let hi = max(a, b);
    return select(v, clamp(v, lo, hi), lo <= hi);
}

// 1. The directional greens.
@compute @workgroup_size(256)
fn ahd_green(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) n: vec3<u32>) {
    let i = lin_index(gid, n);
    if !ahd_inside(i, AHD_MARGIN - 3u) {
        return;
    }
    let w = ahd_bw();
    let c = mos[i];
    if ahd_col(i) == 1u {
        gr[2u * i] = c;
        gr[2u * i + 1u] = c;
        return;
    }
    let l = mos[i - 1u];
    let r = mos[i + 1u];
    let eh = (l + r) * 0.5 + (2.0 * c - mos[i - 2u] - mos[i + 2u]) * 0.25;
    let u = mos[i - w];
    let d = mos[i + w];
    let ev = (u + d) * 0.5 + (2.0 * c - mos[i - 2u * w] - mos[i + 2u * w]) * 0.25;
    gr[2u * i] = ahd_between(eh, l, r);
    gr[2u * i + 1u] = ahd_between(ev, u, d);
}

fn ahd_g(j: u32, v: u32) -> f32 {
    return gr[2u * j + v];
}

// mosaic minus green at `j`
fn ahd_d(j: u32, v: u32) -> f32 {
    return mos[j] - ahd_g(j, v);
}

fn ahd_put(p: vec3<f32>, c: u32, x: f32) -> vec3<f32> {
    return vec3<f32>(select(p.x, x, c == 0u), select(p.y, x, c == 1u), select(p.z, x, c >= 2u));
}

// 2. The full candidate of direction `v` (0 horizontal, 1 vertical) at `i`: red and blue by colour difference.
fn ahd_candidate(i: u32, v: u32) -> vec3<f32> {
    let w = ahd_bw();
    let own = ahd_col(i);
    let g = ahd_g(i, v);
    if own == 1u {
        let p = ahd_put(vec3<f32>(0.0, g, 0.0), ahd_col(i + 1u), g + (ahd_d(i - 1u, v) + ahd_d(i + 1u, v)) * 0.5);
        return ahd_put(p, ahd_col(i + w), g + (ahd_d(i - w, v) + ahd_d(i + w, v)) * 0.5);
    }
    let p = ahd_put(vec3<f32>(0.0, g, 0.0), own, mos[i]);
    let s = ahd_d(i - w - 1u, v) + ahd_d(i - w + 1u, v) + ahd_d(i + w - 1u, v) + ahd_d(i + w + 1u, v);
    return ahd_put(p, 2u - own, g + s * 0.25);
}

// `ahd::cube_root`: a third of the exponent from the bits, then two of Halley's steps.
fn ahd_cube_root(t: f32) -> f32 {
    var y = bitcast<f32>(bitcast<u32>(t) / 3u + 710235476u);
    for (var k = 0; k < 2; k++) {
        let y3 = y * y * y;
        y = y * ((y3 + 2.0 * t) / (2.0 * y3 + t));
    }
    return y;
}

fn ahd_f(t: f32) -> f32 {
    let root = ahd_cube_root(clamp(t, 0.008856, 1.0e30));
    return select(7.787 * t + 16.0 / 116.0, root, t > 0.008856);
}

// `ahd::lab`: linear camera RGB (read as sRGB / D65 for the homogeneity metric only) → CIELab.
fn ahd_lab(p: vec3<f32>) -> vec3<f32> {
    let x = (0.4124 * p.x + 0.3576 * p.y + 0.1805 * p.z) / 0.95047;
    let y = 0.2126 * p.x + 0.7152 * p.y + 0.0722 * p.z;
    let z = (0.0193 * p.x + 0.1192 * p.y + 0.9505 * p.z) / 1.08883;
    let fx = ahd_f(x);
    let fy = ahd_f(y);
    let fz = ahd_f(z);
    return vec3<f32>(116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz));
}

// 3. Both candidates' Lab.
@compute @workgroup_size(256)
fn ahd_to_lab(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) n: vec3<u32>) {
    let i = lin_index(gid, n);
    if !ahd_inside(i, AHD_MARGIN - 2u) {
        return;
    }
    let h = ahd_lab(ahd_candidate(i, 0u));
    let v = ahd_lab(ahd_candidate(i, 1u));
    let b = 6u * i;
    lab[b] = h.x;
    lab[b + 1u] = h.y;
    lab[b + 2u] = h.z;
    lab[b + 3u] = v.x;
    lab[b + 4u] = v.y;
    lab[b + 5u] = v.z;
}

fn ahd_lab_at(j: u32, v: u32) -> vec3<f32> {
    let b = 6u * j + 3u * v;
    return vec3<f32>(lab[b], lab[b + 1u], lab[b + 2u]);
}

fn ahd_dl(a: vec3<f32>, b: vec3<f32>) -> f32 {
    return abs(a.x - b.x);
}

fn ahd_dc(a: vec3<f32>, b: vec3<f32>) -> f32 {
    let p = a.y - b.y;
    let q = a.z - b.z;
    return p * p + q * q;
}

// 4. The homogeneity of each candidate: of its 4 neighbours (left, right, up, down), those within the adaptive
// luminance and chrominance tolerances.
@compute @workgroup_size(256)
fn ahd_homogeneity(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) n: vec3<u32>) {
    let i = lin_index(gid, n);
    if !ahd_inside(i, AHD_MARGIN - 1u) {
        return;
    }
    let w = ahd_bw();
    let nb = array<u32, 4>(i - 1u, i + 1u, i - w, i + w);
    let a = ahd_lab_at(i, 0u);
    let b = ahd_lab_at(i, 1u);
    var dlh: array<f32, 4>;
    var dch: array<f32, 4>;
    var dlv: array<f32, 4>;
    var dcv: array<f32, 4>;
    for (var k = 0u; k < 4u; k++) {
        let h = ahd_lab_at(nb[k], 0u);
        let v = ahd_lab_at(nb[k], 1u);
        dlh[k] = ahd_dl(a, h);
        dch[k] = ahd_dc(a, h);
        dlv[k] = ahd_dl(b, v);
        dcv[k] = ahd_dc(b, v);
    }
    let eps_l = min(max(dlh[0], dlh[1]), max(dlv[2], dlv[3]));
    let eps_c = min(max(dch[0], dch[1]), max(dcv[2], dcv[3]));
    var hh = 0u;
    var hv = 0u;
    for (var k = 0u; k < 4u; k++) {
        hh += select(0u, 1u, dlh[k] <= eps_l && dch[k] <= eps_c);
        hv += select(0u, 1u, dlv[k] <= eps_l && dcv[k] <= eps_c);
    }
    homo[i] = hh | (hv << 8u);
}

// 5. The choice: the candidate more homogeneous over the 3 × 3 around the pixel, their mean on a tie.
@compute @workgroup_size(256)
fn ahd_choose(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) n: vec3<u32>) {
    let i = lin_index(gid, n);
    if !ahd_inside(i, AHD_MARGIN) {
        return;
    }
    let w = ahd_bw();
    var sh = 0u;
    var sv = 0u;
    for (var dy = 0u; dy < 3u; dy++) {
        for (var dx = 0u; dx < 3u; dx++) {
            let t = homo[i + dy * w + dx - w - 1u];
            sh += t & 255u;
            sv += t >> 8u;
        }
    }
    let rh = ahd_candidate(i, 0u);
    let rv = ahd_candidate(i, 1u);
    var p = (rh + rv) * 0.5;
    if sh > sv {
        p = rh;
    } else if sv > sh {
        p = rv;
    }
    let o = ((i / w - AHD_MARGIN) * (w - 2u * AHD_MARGIN) + (i % w - AHD_MARGIN)) * 3u;
    rgb_out[o] = p.x;
    rgb_out[o + 1u] = p.y;
    rgb_out[o + 2u] = p.z;
}
