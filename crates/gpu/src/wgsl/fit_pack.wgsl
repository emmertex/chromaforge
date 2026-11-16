// The display fit's resampled floats (3 per pixel) packed back into opaque RGBA byte words, rounded
// as the CPU's `fit_rgba8` rounds (half away from zero, then clamped). P[0] = pixel count.
// Bindings: src, dst.

fn byte_of(v: f32) -> u32 {
    return u32(clamp(floor(v + 0.5), 0.0, 255.0));
}

@compute @workgroup_size(256)
fn pack_rgb8(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    let r = byte_of(src[3u * i]);
    let gg = byte_of(src[3u * i + 1u]);
    let b = byte_of(src[3u * i + 2u]);
    dst[i] = r | (gg << 8u) | (b << 16u) | (255u << 24u);
}
