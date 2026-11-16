// 2x2 maximum with stride 2 over an NHWC tensor read and written four channels at a time.

struct P {
    h: u32,
    w: u32,
    c4: u32,
    total: u32,
}

@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var<storage, read> src: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> dst: array<vec4<f32>>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let o = gid.x + gid.y * nwg.x * 64u;
    if (o >= p.total) {
        return;
    }
    let c = o % p.c4;
    let pix = o / p.c4;
    let ow = p.w / 2u;
    let oy = pix / ow;
    let ox = pix % ow;
    let i00 = ((2u * oy) * p.w + 2u * ox) * p.c4 + c;
    let i10 = i00 + p.w * p.c4;
    dst[o] = max(max(src[i00], src[i00 + p.c4]), max(src[i10], src[i10 + p.c4]));
}
