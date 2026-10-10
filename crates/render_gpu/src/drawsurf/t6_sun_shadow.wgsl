// bo2mp: Black Ops II's sun shadow casters: depth only, from the sun, one
// partition per pass.

@group(0) @binding(0) var<uniform> shadow_from_world: mat4x4<f32>;

struct CasterIn {
    @location(0) position: vec3<f32>,
    @location(6) world_from_local_0: vec4<f32>,
    @location(7) world_from_local_1: vec4<f32>,
    @location(8) world_from_local_2: vec4<f32>,
    @location(9) world_from_local_3: vec4<f32>,
}

@vertex
fn vertex_caster(in: CasterIn) -> @builtin(position) vec4<f32> {
    let world_from_local = mat4x4<f32>(
        in.world_from_local_0,
        in.world_from_local_1,
        in.world_from_local_2,
        in.world_from_local_3,
    );
    return shadow_from_world * (world_from_local * vec4(in.position, 1.0));
}
