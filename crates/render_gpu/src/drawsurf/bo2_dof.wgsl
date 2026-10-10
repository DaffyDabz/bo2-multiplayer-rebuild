// bo2mp: depth of field on a Black Ops II map (see bo2_dof.rs): the frame
// at quarter size with each block's near blur amount, blurred, then mixed
// back in by each pixel's distance (near, far and the gun's own range).

struct DofParams {
    // xy: one source texel in uv; zw: the blur direction (in texels).
    texel_dir: vec4<f32>,
    // near start, near end, far start, far end (world units).
    range: vec4<f32>,
    // x: how much the far range blurs against the near one.
    blur: vec4<f32>,
    // x: the scene's near plane; y: where the gun's depth band starts;
    // z: the band's size (reversed depth); w: the gun's own near plane.
    depth: vec4<f32>,
    // the gun's own near start and end.
    gun: vec4<f32>,
}

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
@group(0) @binding(2) var<uniform> params: DofParams;
@group(0) @binding(3) var blurred: texture_2d<f32>;
@group(0) @binding(4) var depth_tex: texture_depth_2d;

struct FullscreenOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> FullscreenOut {
    let uv = vec2(f32((index << 1u) & 2u), f32(index & 2u));
    var out: FullscreenOut;
    out.position = vec4(uv * vec2(2.0, -2.0) + vec2(-1.0, 1.0), 0.0, 1.0);
    out.uv = uv;
    return out;
}

fn near_amount(z: f32, start: f32, end: f32) -> f32 {
    return saturate((end - z) / max(end - start, 0.001));
}

// How blurred one pixel is: the scene by its distance (near, then far), the
// gun (drawn in its own depth band) by its own range.
fn amount(coords: vec2<i32>) -> f32 {
    let d = textureLoad(depth_tex, coords, 0);
    let p = params.depth;
    if d > p.y {
        let z = p.w / max((d - p.y) / p.z, 1.0e-7);
        return near_amount(z, params.gun.x, params.gun.y) * step(params.gun.x + 0.5, params.gun.y);
    }
    let z = p.x / max(d / p.y, 1.0e-7);
    let r = params.range;
    let near = near_amount(z, r.x, r.y) * step(r.x + 1.0, r.y);
    let far = saturate((z - r.z) / max(r.w - r.z, 0.001)) * params.blur.x;
    return max(near, far);
}

// The shrink: a 4x4 block of the frame per quarter texel (four bilinear
// taps); alpha the block's largest near blur amount, so a near object's blur
// spreads over what is behind it when blurred.
@fragment
fn shrink(in: FullscreenOut) -> @location(0) vec4<f32> {
    let t = params.texel_dir.xy;
    var c = vec3(0.0);
    for (var i = 0; i < 4; i++) {
        let o = vec2(f32(i & 1) * 2.0 - 1.0, f32(i >> 1u) * 2.0 - 1.0) * t;
        c += textureSample(source, source_sampler, in.uv + o).rgb;
    }
    let size = vec2<i32>(textureDimensions(depth_tex));
    let base = vec2<i32>(floor(in.position.xy)) * 4;
    var a = 0.0;
    for (var y = 0; y < 4; y++) {
        for (var x = 0; x < 4; x++) {
            a = max(a, amount(min(base + vec2(x, y), size - vec2(1))));
        }
    }
    return vec4(c * 0.25, a);
}

// A 9-tap gaussian along one direction (colour and amount).
@fragment
fn blur(in: FullscreenOut) -> @location(0) vec4<f32> {
    let step = params.texel_dir.xy * params.texel_dir.zw;
    let w = array<f32, 5>(0.2270270, 0.1945946, 0.1216216, 0.0540541, 0.0162162);
    var c = textureSample(source, source_sampler, in.uv) * w[0];
    for (var i = 1; i < 5; i++) {
        let o = step * f32(i);
        c += textureSample(source, source_sampler, in.uv + o) * w[i];
        c += textureSample(source, source_sampler, in.uv - o) * w[i];
    }
    return c;
}

// Put back: the sharp pixel or the blurred one by how blurred it is (its own
// amount, or a near object's spread over it).
@fragment
fn composite(in: FullscreenOut) -> @location(0) vec4<f32> {
    let s = textureSample(source, source_sampler, in.uv).rgb;
    let b = textureSample(blurred, source_sampler, in.uv);
    let own = amount(vec2<i32>(floor(in.position.xy)));
    let k = max(own, saturate(b.a * 2.0 - own));
    return vec4(mix(s, b.rgb, smoothstep(0.0, 1.0, k)), 1.0);
}
