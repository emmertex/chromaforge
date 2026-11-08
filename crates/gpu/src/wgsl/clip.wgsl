// CLIP transformer blocks (semantic search): layer norm, linear layers (with GELU and the residual add fused in) and
// multi-head attention, on `[rows, width]` row-major f32 matrices. A head `h` is columns `h*head_dim ..` of q, k and v.
// Own design, following the CLIP paper's pre-norm transformer (Radford et al. 2021); `clip.rs` is the host side.

struct Params {
    rows: u32,     // matrix rows (batch * sequence)
    k: u32,        // input width (linear, layernorm)
    n: u32,        // output width (linear)
    flags: u32,    // linear: 1 = GELU, 2 = add into `o` (residual)
    heads: u32,
    seq: u32,
    head_dim: u32,
    causal: u32,
    eps: f32,
    scale: f32,
    _a: u32,
    _b: u32,
}

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read> a: array<f32>;
@group(0) @binding(2) var<storage, read> b: array<f32>;
@group(0) @binding(3) var<storage, read> c: array<f32>;
@group(0) @binding(4) var<storage, read_write> o: array<f32>;

const NEG: f32 = -3.0e38;
const MAX_SEQ: u32 = 1024u;

var<workgroup> ta: array<f32, 256>;
var<workgroup> tb: array<f32, 256>;
var<workgroup> red: array<f32, 256>;
var<workgroup> sc: array<f32, 1024>;

// Abramowitz & Stegun 7.1.26 (|error| < 1.5e-7)
fn erf(x: f32) -> f32 {
    let s = sign(x);
    let ax = abs(x);
    let t = 1.0 / (1.0 + 0.3275911 * ax);
    let y = 1.0 - (((((1.061405429 * t - 1.453152027) * t) + 1.421413741) * t - 0.284496736) * t + 0.254829592) * t * exp(-ax * ax);
    return s * y;
}

fn gelu(x: f32) -> f32 {
    return 0.5 * x * (1.0 + erf(x * 0.70710678118654752));
}

// o[r, j] (+)= act(sum_i a[r, i] * b[j, i] + c[j]); b is the weight in `[out, in]` order. 16x16 tiles.
@compute @workgroup_size(16, 16)
fn linear(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) l: vec3<u32>) {
    let row = wg.y * 16u + l.y;
    let col = wg.x * 16u + l.x;
    let wrow = wg.x * 16u + l.y;
    var acc = 0.0;
    let tiles = (p.k + 15u) / 16u;
    for (var t = 0u; t < tiles; t = t + 1u) {
        let kk = t * 16u + l.x;
        var av = 0.0;
        if (row < p.rows && kk < p.k) {
            av = a[row * p.k + kk];
        }
        ta[l.y * 16u + l.x] = av;
        var bv = 0.0;
        if (wrow < p.n && kk < p.k) {
            bv = b[wrow * p.k + kk];
        }
        tb[l.y * 16u + l.x] = bv;
        workgroupBarrier();
        for (var i = 0u; i < 16u; i = i + 1u) {
            acc = acc + ta[l.y * 16u + i] * tb[l.x * 16u + i];
        }
        workgroupBarrier();
    }
    if (row < p.rows && col < p.n) {
        var v = acc + c[col];
        if ((p.flags & 1u) != 0u) {
            v = gelu(v);
        }
        let idx = row * p.n + col;
        if ((p.flags & 2u) != 0u) {
            v = v + o[idx];
        }
        o[idx] = v;
    }
}

fn reduce_sum(l: u32) -> f32 {
    for (var s = 128u; s > 0u; s = s >> 1u) {
        if (l < s) {
            red[l] = red[l] + red[l + s];
        }
        workgroupBarrier();
    }
    return red[0];
}

fn reduce_max(l: u32) -> f32 {
    for (var s = 128u; s > 0u; s = s >> 1u) {
        if (l < s) {
            red[l] = max(red[l], red[l + s]);
        }
        workgroupBarrier();
    }
    return red[0];
}

// o[r] = (a[r] - mean) / sqrt(var + eps) * b + c, one row per workgroup.
@compute @workgroup_size(256)
fn layernorm(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) l: u32) {
    let r = wg.x;
    let base = r * p.k;
    var s = 0.0;
    for (var i = l; i < p.k; i = i + 256u) {
        s = s + a[base + i];
    }
    red[l] = s;
    workgroupBarrier();
    let mean = reduce_sum(l) / f32(p.k);
    workgroupBarrier();
    var v = 0.0;
    for (var i = l; i < p.k; i = i + 256u) {
        let d = a[base + i] - mean;
        v = v + d * d;
    }
    red[l] = v;
    workgroupBarrier();
    let inv = inverseSqrt(reduce_sum(l) / f32(p.k) + p.eps);
    for (var i = l; i < p.k; i = i + 256u) {
        o[base + i] = (a[base + i] - mean) * inv * b[i] + c[i];
    }
}

// Softmax attention for one query (workgroup x) of one batch item and head (workgroup y = item * heads + head):
// a = q, b = k, c = v, o = the heads' outputs side by side.
@compute @workgroup_size(256)
fn attention(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) l: u32) {
    let qi = wg.x;
    let item = wg.y / p.heads;
    let h = wg.y % p.heads;
    let width = p.heads * p.head_dim;
    let off = h * p.head_dim;
    let qbase = (item * p.seq + qi) * width + off;
    var m = NEG;
    for (var j = l; j < p.seq; j = j + 256u) {
        var s = NEG;
        if (p.causal == 0u || j <= qi) {
            let kbase = (item * p.seq + j) * width + off;
            var dot = 0.0;
            for (var d = 0u; d < p.head_dim; d = d + 1u) {
                dot = dot + a[qbase + d] * b[kbase + d];
            }
            s = dot * p.scale;
        }
        sc[j] = s;
        m = max(m, s);
    }
    red[l] = m;
    workgroupBarrier();
    let mx = reduce_max(l);
    workgroupBarrier();
    var sum = 0.0;
    for (var j = l; j < p.seq; j = j + 256u) {
        var e = 0.0;
        if (sc[j] > NEG) {
            e = exp(sc[j] - mx);
        }
        sc[j] = e;
        sum = sum + e;
    }
    red[l] = sum;
    workgroupBarrier();
    let total = max(reduce_sum(l), 1e-30);
    for (var d = l; d < p.head_dim; d = d + 256u) {
        var acc = 0.0;
        for (var j = 0u; j < p.seq; j = j + 1u) {
            acc = acc + sc[j] * c[(item * p.seq + j) * width + off + d];
        }
        o[qbase + d] = acc / total;
    }
}
