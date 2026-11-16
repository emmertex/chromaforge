// The Histogram panel's bins, scope and waveform of the finished 8-bit image, the GPU twin of
// `lcc_raster::Histogram::of_srgb8_covered` (the oracle: the same samples, the same integer maths).
// Bindings: img (packed RGBA8, one word a pixel), cov (coverage words, or unused), hist (counts).
// P: 0 image width, 1–2 window origin, 3–4 window size, 5 sample step, 6 samples per row,
// 7 sample rows, 8 coverage present, 9 coverage words per row.
// hist layout: r, g, b, luma, chroma (256 each), total, the 65×65 scope, 4 waveform planes of 128×64.

const H_TOTAL: u32 = 1280u;
const H_SCOPE: u32 = 1281u;
const H_WAVE: u32 = 5506u; // H_SCOPE + 65 * 65

var<workgroup> local_bins: array<atomic<u32>, 1281>;

fn scope_step(v: i32) -> i32 {
    if (v >= 0) {
        return (v + 2187) / 4375;
    }
    return (v - 2187) / 4375;
}

@compute @workgroup_size(256)
fn hist_k(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    for (var k = li; k < 1281u; k += 256u) {
        atomicStore(&local_bins[k], 0u);
    }
    workgroupBarrier();
    let i = lin_index(g, nw);
    let across = max(pu(6u), 1u);
    var live = i < across * pu(7u);
    let sx = i % across;
    let x = sx * pu(5u);
    let y = (i / across) * pu(5u);
    if (live && pu(8u) != 0u) {
        let word = cov[y * pu(9u) + x / 32u];
        live = ((word >> (x % 32u)) & 1u) == 1u;
    }
    if (live) {
        let p = img[(pu(2u) + y) * pu(0u) + pu(1u) + x];
        let r = p & 255u;
        let gg = (p >> 8u) & 255u;
        let b = (p >> 16u) & 255u;
        let l = min((r * 54u + gg * 183u + b * 19u) >> 8u, 255u);
        let mx = max(max(r, gg), b);
        let mn = min(min(r, gg), b);
        var sat = 0u;
        if (mx > 0u) {
            sat = min(((mx - mn) * ((255u << 16u) / mx)) >> 16u, 255u);
        }
        atomicAdd(&local_bins[r], 1u);
        atomicAdd(&local_bins[256u + gg], 1u);
        atomicAdd(&local_bins[512u + b], 1u);
        atomicAdd(&local_bins[768u + l], 1u);
        atomicAdd(&local_bins[1024u + sat], 1u);
        atomicAdd(&local_bins[H_TOTAL], 1u);
        let ri = i32(r);
        let gi = i32(gg);
        let bi = i32(b);
        let bx = u32(clamp(32 + scope_step(-169 * ri - 331 * gi + 500 * bi), 0, 64));
        let by = u32(clamp(32 - scope_step(500 * ri - 419 * gi - 81 * bi), 0, 64));
        atomicAdd(&hist[H_SCOPE + by * 65u + bx], 1u);
        let col = min(sx * 128u / across, 127u);
        let base = H_WAVE + col * 64u;
        atomicAdd(&hist[base + l / 4u], 1u);
        atomicAdd(&hist[base + 8192u + r / 4u], 1u);
        atomicAdd(&hist[base + 16384u + gg / 4u], 1u);
        atomicAdd(&hist[base + 24576u + b / 4u], 1u);
    }
    workgroupBarrier();
    for (var k = li; k < 1281u; k += 256u) {
        let c = atomicLoad(&local_bins[k]);
        if (c > 0u) {
            atomicAdd(&hist[k], c);
        }
    }
}
