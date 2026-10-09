#import bevy_ui::ui_vertex_output::UiVertexOutput

struct Tint {
    value: vec4<f32>,
};

@group(1) @binding(0) var<uniform> tint: Tint;
@group(1) @binding(1) var picture: texture_2d<f32>;
@group(1) @binding(2) var picture_sampler: sampler;
@group(1) @binding(3) var<uniform> uv_window: vec4<f32>;
@group(1) @binding(4) var<uniform> encoded: f32;
@group(1) @binding(5) var<uniform> color_add: f32;

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    let texel = textureSample(picture, picture_sampler, uv_window.xy + in.uv * uv_window.zw);
    // Added onto the scene: the colour scaled by the fade. The UI target
    // needs a non-zero alpha to show anything, so the colour is carried as
    // (c / m, m) with m its largest channel.
    var c = texel.rgb * tint.value.rgb * tint.value.a;
    if (color_add > 0.5) {
        // BO2's sw4_2d_color_add: the colour map's alpha lit by the
        // element's colour, plus the AddMap's own colour, times the fade.
        c = (texel.a * tint.value.rgb + texel.rgb) * tint.value.a;
    }
    if (encoded > 0.5) {
        // The match HUD is drawn straight into the encoded main texture:
        // ONE/ONE adds the colour itself, as the game does in gamma space.
        return vec4<f32>(c, 0.0);
    }
    let m = max(c.r, max(c.g, c.b));
    // A faint picture (its largest channel under 0.1: a hex mesh, a glow's
    // tail) would, normalised, take the whole alpha of whatever picture is
    // under it (the target blends straight), so it is carried fainter.
    return vec4<f32>(c / max(m, 0.1), m);
}
