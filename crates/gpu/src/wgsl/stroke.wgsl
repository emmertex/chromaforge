// Auto Mask strokes refined over the rectangle they can reach and kept in 16 bits between renders
// (`render::auto_stroke`, `strokes`; the CPU's twin: `masks::auto_stroke`).

// Cut the rectangle at P[1], P[2] (x0, y0), P[3] × P[4] (rw × rh) from a plane P[0] pixels wide:
// src → dst.
@compute @workgroup_size(16, 16)
fn crop(@builtin(global_invocation_id) g: vec3<u32>) {
    let w = pu(0u);
    let rw = pu(3u);
    let rh = pu(4u);
    if (g.x >= rw || g.y >= rh) {
        return;
    }
    dst[g.y * rw + g.x] = src[(pu(2u) + g.y) * w + pu(1u) + g.x];
}

// P[0] coverage values of src in 16 bits, two per word (pixel 2k in the low half):
// round(clamp(v, 0, 1) · 65535), as the CPU keeps them.
@compute @workgroup_size(256)
fn pack16(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    let n = pu(0u);
    let j = 2u * i;
    if (j >= n) {
        return;
    }
    var b = 0.0;
    if (j + 1u < n) {
        b = src[j + 1u];
    }
    packed[i] = pack2x16unorm(vec2<f32>(src[j], b));
}

// Combine packed coverage into the plane dst (P[0] wide) over the rectangle P[1], P[2] (x0, y0),
// P[3] × P[4] (rw × rh): P[5] density, P[6] the op of `combine` (1 add, 2 subtract, 4 add up to the
// density). The coverage is read from a packed rectangle P[7] pixels wide, from its pixel P[8], P[9].
@compute @workgroup_size(16, 16)
fn combine16(@builtin(global_invocation_id) g: vec3<u32>) {
    let w = pu(0u);
    let rw = pu(3u);
    let rh = pu(4u);
    if (g.x >= rw || g.y >= rh) {
        return;
    }
    let k = (pu(9u) + g.y) * pu(7u) + pu(8u) + g.x;
    let pair = unpack2x16unorm(packed[k / 2u]);
    var v = pair.x;
    if (k % 2u == 1u) {
        v = pair.y;
    }
    let j = (pu(2u) + g.y) * w + pu(1u) + g.x;
    let a = dst[j];
    let op = pu(6u);
    if (op == 2u) {
        dst[j] = a * (1.0 - v);
    } else if (op == 4u) {
        dst[j] = max(a, min(a + v - a * v, pf(5u)));
    } else {
        dst[j] = max(a, v);
    }
}
