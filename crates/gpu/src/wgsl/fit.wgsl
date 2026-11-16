// The loupe's display fit (`lcc_raster::resample::fit_rgba8`) on the device: the 8-bit output
// unpacked to 3 floats per pixel for the `resize_*` kernels between these two. Words are RGBA
// bytes, red lowest. P[0] = pixel count. Bindings: src, dst.

@compute @workgroup_size(256)
fn unpack_rgb8(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    let w = src[i];
    dst[3u * i] = f32(w & 255u);
    dst[3u * i + 1u] = f32((w >> 8u) & 255u);
    dst[3u * i + 2u] = f32((w >> 16u) & 255u);
}
