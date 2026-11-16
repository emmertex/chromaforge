// An HDR view's texture on the device (`lcc_engine::hdr_texture::encode_pixel`): the deep render's
// linear floats (SDR white = 1) as `Rgba16Float` texels for the window's headroom. Negatives and
// non-numbers to 0, brighter than the headroom scaled down to it keeping their hue, sRGB-encoded
// and continued above 1, alpha 1; two words per pixel (red and green, blue and alpha), as the
// texture's bytes lie. P: n (pixels), offset (word of the first float in src), headroom (bits).
// Bindings: src (the render's output words), dst.

fn srgb_encode_ext(v: f32) -> f32 {
    if (v <= 0.0031308) {
        return 12.92 * v;
    }
    return 1.055 * pow(v, 1.0 / 2.4) - 0.055;
}

// (a non-number or an infinity reads as 0, as on the CPU)
fn finite_or_0(v: f32) -> f32 {
    if (v != v || abs(v) > 3.4028234e38) {
        return 0.0;
    }
    return max(v, 0.0);
}

@compute @workgroup_size(256)
fn hdr_texels(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    let o = pu(1u) + 3u * i;
    var hr = pf(2u);
    if (hr != hr || abs(hr) > 3.4028234e38) {
        hr = 1.0;
    }
    let h = max(hr, 1.0);
    let c = vec3<f32>(finite_or_0(bitcast<f32>(src[o])), finite_or_0(bitcast<f32>(src[o + 1u])), finite_or_0(bitcast<f32>(src[o + 2u])));
    let m = max(c.x, max(c.y, c.z));
    var k = 1.0;
    if (m > h) {
        k = h / m;
    }
    let e = vec3<f32>(srgb_encode_ext(c.x * k), srgb_encode_ext(c.y * k), srgb_encode_ext(c.z * k));
    dst[2u * i] = pack2x16float(e.xy);
    dst[2u * i + 1u] = pack2x16float(vec2<f32>(e.z, 1.0));
}
