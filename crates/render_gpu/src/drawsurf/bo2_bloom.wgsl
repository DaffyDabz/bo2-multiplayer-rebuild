// bo2zm: Black Ops II's bloom and final colour (see bo2_bloom.rs): the
// frame's bright parts at quarter size, blurred into a high and a low level,
// put back as BO2's hdr_bloom_apply does (sqrt(scene^2 + bloom), the
// highlight roll-off), then looked up in the map's colour table.

struct BloomParams {
    // xy: one source texel in uv; zw: the blur direction (in texels).
    texel_dir: vec4<f32>,
    // The menu blur's: x, y the luminance where its shrink starts and is
    // full (-1, 0: everything); w: how much of the blur goes back in.
    threshold: vec4<f32>,
    // bo2mp: the bright pass's ramps on linear luminance: x, z where the
    // colour bloom starts and is full; y, w the luminance bloom's.
    ramp: vec4<f32>,
    // bo2mp: hdr_bloom_combine_hilo's tints (the vision's vc_RGBH, vc_RGBL,
    // vc_YH, vc_YL): the high and low levels' colour, then their luminance.
    rgb_hi: vec4<f32>,
    rgb_lo: vec4<f32>,
    y_hi: vec4<f32>,
    y_lo: vec4<f32>,
}

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
@group(0) @binding(2) var<uniform> params: BloomParams;
@group(0) @binding(3) var bloom: texture_2d<f32>;
@group(0) @binding(4) var bloom_lo: texture_2d<f32>;
@group(0) @binding(5) var grade_lut: texture_3d<f32>;

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

// BO2's highlight roll-off (hdr_bloom_apply): above 0.75,
// 0.75 + 0.25 * (1 - 2^(-5.77078 * (x - 0.75))).
fn roll_off(x: vec3<f32>) -> vec3<f32> {
    let rolled = 0.75 + 0.25 * (1.0 - exp2(-5.77078 * (x - 0.75)));
    return select(x, rolled, x > vec3(0.75));
}

// bo2mp: the surfaces roll their own highlights off (the frame holds 8-bit
// display values); this undoes it, so bloom goes in before the roll-off as
// in BO2. An 8-bit white reads back as about 1.96.
fn unroll(x: vec3<f32>) -> vec3<f32> {
    let rest = max((1.0 - x) * 4.0, vec3(0.5 / 255.0 * 4.0));
    return select(x, 0.75 - log2(rest) / 5.77078, x > vec3(0.75));
}

fn ramp(x: f32, start: f32, end: f32) -> f32 {
    let t = saturate((x - start) / max(end - start, 0.000001));
    return t * t * (3.0 - 2.0 * t);
}

// The menu blur's shrink: a 4x4 block of the full frame per quarter texel
// (four bilinear taps), weighted by a smooth threshold on its luminance.
@fragment
fn extract(in: FullscreenOut) -> @location(0) vec4<f32> {
    let t = params.texel_dir.xy;
    var c = vec3(0.0);
    for (var i = 0; i < 4; i++) {
        let o = vec2(f32(i & 1) * 2.0 - 1.0, f32(i >> 1u) * 2.0 - 1.0) * t;
        c += textureSample(source, source_sampler, in.uv + o).rgb;
    }
    c *= 0.25;
    let luma = dot(c, vec3(0.212585, 0.715195, 0.07222));
    let w = smoothstep(params.threshold.x, params.threshold.y, luma);
    return vec4(c * w, 1.0);
}

// bo2mp: BO2's bloom downsample (hdr_bloom_downsample): the same 4x4 block
// averaged in gamma, squared to linear; its luminance through two smooth
// ramps, one for the colour (rgb) and one for the luminance (alpha).
@fragment
fn bright(in: FullscreenOut) -> @location(0) vec4<f32> {
    let t = params.texel_dir.xy;
    var c = vec3(0.0);
    for (var i = 0; i < 4; i++) {
        let o = vec2(f32(i & 1) * 2.0 - 1.0, f32(i >> 1u) * 2.0 - 1.0) * t;
        c += unroll(textureSample(source, source_sampler, in.uv + o).rgb);
    }
    c *= 0.25;
    let lin = c * c;
    let luma = dot(lin, vec3(0.212585, 0.715195, 0.07222));
    let r = params.ramp;
    return vec4(lin * ramp(luma, r.x, r.z), luma * ramp(luma, r.y, r.w));
}

// A 9-tap gaussian along one direction.
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

// BO2's final image (hdr_bloom_combine_hilo, hdr_bloom_apply): the two
// bloom levels tinted by the vision, added to the frame in linear, the
// highlight roll-off, then the map's colour table (32^3, trilinear like the
// game's 2D lookup).
@fragment
fn composite(in: FullscreenOut) -> @location(0) vec4<f32> {
    let s = unroll(textureSample(source, source_sampler, in.uv).rgb);
    let hi = textureSample(bloom, source_sampler, in.uv);
    let lo = textureSample(bloom_lo, source_sampler, in.uv);
    let add = hi.rgb * params.rgb_hi.rgb + lo.rgb * params.rgb_lo.rgb
        + hi.a * params.y_hi.rgb + lo.a * params.y_lo.rgb;
    let g = roll_off(sqrt(s * s + max(add, vec3(0.0))));
    let at = saturate(g) * (31.0 / 32.0) + vec3(0.5 / 32.0);
    return vec4(textureSampleLevel(grade_lut, source_sampler, at, 0.0).rgb, 1.0);
}

// bo2zm M4: the menus' world blur put back in the frame's place
// (threshold.w: how much).
fn srgb_enc(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(max(c, vec3(0.0)), vec3(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3(0.0031308));
}

fn srgb_dec(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((max(c, vec3(0.0)) + 0.055) / 1.055, vec3(2.4));
    return select(hi, lo, c <= vec3(0.04045));
}

// bo2mp: under a popup in a match the UI stacks two 0.8 black dims over the
// world. BO2 blends them in gamma space (each x0.2 on the encoded colour);
// ours blend in linear light with a remapped alpha (x0.029 on the light),
// which matches for bright pixels but crushes the dark world to black under
// the sRGB toe. Lift the world so the stacked result is the gamma blend's
// (threshold.y: the dims up; the UI then draws none of them over the world.)
fn dim_lift(c: vec3<f32>, dims: f32, z: f32) -> vec3<f32> {
    // (z: the class select before the first spawn keeps its own look.)
    if (dims < 1.5 || z > 0.5) {
        return c;
    }
    // (the world is stored as drawn on screen; the UI dim blends its light.)
    let k1 = 0.02899;
    let after_pause = srgb_enc(srgb_dec(c) * k1);
    return after_pause * pow(0.2, dims - 1.0);
}

@fragment
fn blurred(in: FullscreenOut) -> @location(0) vec4<f32> {
    let s = textureSample(source, source_sampler, in.uv).rgb;
    let b = textureSample(bloom, source_sampler, in.uv).rgb;
    // bo2mp: BO2's class select before the first spawn (threshold.z = 1) shows the
    // world blurred and grey and dim: the colour and the light go out as the blur
    // comes in. The pause menu in a live match (z = 0) only blurs it; the menu's
    // own dim layer darkens it and the colour stays.
    // (threshold.x = 1: the class select at the first spawn, real 28, is not blurred.)
    let sharp = select(0.0, 1.0, params.threshold.x > 0.5);
    let m = mix(s, b, params.threshold.w * 0.6 * (1.0 - sharp));
    let grey = vec3(dot(m, vec3(0.212585, 0.715195, 0.07222)));
    let k = params.threshold.w * params.threshold.z;
    // (Every menu before the first spawn takes the x0.5: the class select at the first spawn,
    // real 28, included. Its backing is the menu's own full-screen black 0.8 image, which the
    // match UI now blends in the target's gamma space, so the dim is the plain x0.2 on the
    // encoded colour and the x0.5 no longer stacks on a crushed dark world (it did while the dim
    // was blended in linear light: that left the strip at 0.29 x real's light). Real's saturated
    // white reads 51 in the pause menu (x0.2 alone); the class select strip's mean reads 11.3 against
    // 11.3 here with the x0.5, 22.3 without.)
    let level = mix(1.0, 0.5, k);
    return vec4(dim_lift(mix(m, grey, k) * level, params.threshold.y, params.threshold.z), 1.0);
}
