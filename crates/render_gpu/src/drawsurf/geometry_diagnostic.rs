use crate::drawsurf::scene_depth::{SCENE_DEPTH_FORMAT, SceneDepthTexture};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use bevy::core_pipeline::core_3d::main_opaque_pass_3d;
use bevy::core_pipeline::{Core3d, Core3dSystems};
use bevy::mesh::VertexBufferLayout;
use bevy::prelude::*;
use bevy::render::RenderStartup;
use bevy::render::render_resource::binding_types::{
    sampler, texture_2d, texture_cube, texture_cube_array, texture_depth_2d, uniform_buffer,
    uniform_buffer_sized,
};
use bevy::render::render_resource::{
    AddressMode, BindGroup, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries,
    BlendComponent, BlendFactor, BlendOperation, BlendState, Buffer, BufferInitDescriptor, BufferUsages, CachedRenderPipelineId, ColorTargetState,
    ColorWrites, CompareFunction, DepthBiasState, DepthStencilState, Face, FilterMode,
    FragmentState, FrontFace,
    IndexFormat, MipmapFilterMode, MultisampleState, PipelineCache, PrimitiveState,
    RenderPipelineDescriptor, Sampler, SamplerBindingType, SamplerDescriptor, ShaderStages,
    SpecializedRenderPipeline, SpecializedRenderPipelines, StoreOp, TextureFormat,
    TextureSampleType, TextureViewDimension, VertexAttribute, VertexFormat, VertexState,
    VertexStepMode,
};
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::view::{
    ExtractedView, Msaa, ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms,
};
use bevy::render::{Render, RenderSystems};
use bevy::shader::{Shader, ShaderDefVal};

use super::gpu_resources::RuntimeUploadedImageRegistry;
use super::depth_range::{
    GFX_DEPTH_RANGE_VIEWMODEL, depth_range_type_for_draw, reverse_z_viewport_depth,
};
use crate::diag::render_frame_diag::{SharedRenderStages, SharedRenderStagesSlot};
use render_frame::{
    FrameProductKind, RetainedDrawItem, RetainedDrawKind, SmodelVertex, WorldVertex,
};
use render_material::MaterialGenerationId;

const SHADER_PATH: &str = "embedded://render_gpu/drawsurf/geometry_diagnostic.wgsl";

fn geometry_diagnostic_covers(kind: &RetainedDrawKind, key: u64) -> bool {
    !matches!(
        kind,
        RetainedDrawKind::CodeMesh { .. }
            | RetainedDrawKind::ParticleCloud { .. }
            | RetainedDrawKind::MarkMesh { .. }
            | RetainedDrawKind::Glass { .. }
    ) && depth_range_type_for_draw(kind, key) != GFX_DEPTH_RANGE_VIEWMODEL
}

struct DiagnosticStamp {
    started: Instant,
    slot: Option<Arc<Mutex<SharedRenderStages>>>,
    pass: u32,
    draw_n: u32,
    world_n: u32,
    smodel_n: u32,
    xmodel_n: u32,
}

impl Drop for DiagnosticStamp {
    fn drop(&mut self) {
        let Some(slot) = &self.slot else {
            return;
        };
        let Ok(mut guard) = slot.lock() else {
            return;
        };
        guard.diag_ms = Some(self.started.elapsed().as_secs_f32() * 1000.0);
        guard.diag_pass = Some(self.pass);
        guard.diag_draw_n = Some(self.draw_n);
        guard.diag_world_n = Some(self.world_n);
        guard.diag_smodel_n = Some(self.smodel_n);
        guard.diag_xmodel_n = Some(self.xmodel_n);
    }
}

static NO_DIAGNOSTIC: OnceLock<bool> = OnceLock::new();

pub fn geometry_diagnostic_enabled() -> bool {
    !*NO_DIAGNOSTIC.get_or_init(|| std::env::var_os("IW4L_NO_DIAGNOSTIC").is_some())
}

#[derive(Default)]
struct DiagnosticDrawScratch {
    submitted: HashSet<u64>,
    exec_ready: HashSet<u64>,
    draws: Vec<(RetainedDrawItem, bool)>,
    smodel_instances: Vec<DiagnosticSmodelInstance>,
}

fn diagnostic_would_draw(item: &RetainedDrawItem, ready: bool, submitted: &HashSet<u64>) -> bool {
    if ready && matches!(item.kind, RetainedDrawKind::World { .. }) {
        return false;
    }
    if submitted.contains(&item.key) {
        return false;
    }
    geometry_diagnostic_covers(&item.kind, item.key)
}

/// bo2zm: frame pacing and prop counts over a window, logged every 5 s.
#[derive(Default)]
struct DiagnosticPerf {
    window_start: Option<Instant>,
    last_frame: Option<Instant>,
    frames: u32,
    worst_ms: f32,
    props_drawn: u64,
    props_culled: u64,
    last_census_at: Option<Instant>,
}

#[derive(Resource, Clone, Debug, Default)]
pub struct ExtractedDiagnosticGeometry {
    pub generation: MaterialGenerationId,
    pub overlay_gpu_wait: bool,
    pub world_vertices: Arc<Vec<WorldVertex>>,
    pub world_indices: Arc<Vec<u32>>,
    pub world_surface_ranges: Arc<Vec<(u32, u32)>>,
    pub smodel_vertices: Arc<Vec<SmodelVertex>>,
    pub smodel_indices: Arc<Vec<u32>>,
    pub smodel_surface_ranges: Arc<Vec<(u32, u32)>>,
    pub xmodel_vertices: Arc<Vec<SmodelVertex>>,
    pub xmodel_indices: Arc<Vec<u32>>,
    pub xmodel_surface_ranges: Arc<Vec<(u32, u32)>>,
    pub xmodel_revision: u64,
    pub g0_world_surfs: Vec<u16>,
    /// bo2zm: per world surface, its colour map's uploaded image slot
    /// (`u32::MAX` = none); the fallback draw samples it when present.
    pub world_surface_color: Arc<Vec<u32>>,
    /// bo2zm: per world surface, its Black Ops II draw code
    /// (`asset_core::T6Draw::code`; `u32::MAX` = none).
    pub world_surface_draw: Arc<Vec<u32>>,
    /// bo2zm: per world surface, its lightmap page (`u8::MAX` = none).
    pub world_surface_lightmap: Arc<Vec<u8>>,
    /// bo2zm: the T6 sun direction and exposure scale, the sun colour, then
    /// the sky's rotation and brightness.
    pub t6_lighting: Option<[[f32; 4]; 3]>,
    /// bo2zm: Black Ops II props: `[surface, colour slot, draw code,
    /// instance]` per first-LOD surface of each placement.
    pub t6_props: Arc<Vec<[u32; 4]>>,
    /// bo2zm: per prop draw, its normal and specular map slots (shine).
    pub t6_prop_shine: Arc<Vec<[u32; 3]>>,
    /// bo2zm: per prop instance, world-from-local columns, light, then
    /// primary light and its visibility.
    pub t6_prop_instances: Arc<Vec<[f32; 24]>>,
    /// bo2zm: what the props were built from (generation, placements).
    pub t6_props_key: Option<(MaterialGenerationId, usize)>,
    /// bo2zm: the uploaded slot of the Black Ops II sky's cube map.
    pub t6_sky_slot: Option<u32>,
    /// bo2zm: per world surface, its primary light index.
    pub world_surface_primary: Arc<Vec<u8>>,
    /// bo2zm: the map's primary lights (four vec4 each).
    pub t6_lights: Arc<Vec<[f32; 16]>>,
    /// bo2zm: per world surface, its layer 1 and 2 colour map slots.
    pub world_surface_layers: Arc<Vec<[u32; 2]>>,
    /// bo2zm: per world vertex, layer 1 and 2 texcoords (or empty).
    pub world_layer_uvs: Arc<Vec<[f32; 4]>>,
    /// bo2zm: per world surface, its normal and specular map slots
    /// (`u32::MAX` = none) and its reflection probe (the shine).
    pub world_surface_shine: Arc<Vec<[u32; 3]>>,
    /// bo2zm: the reflection probes' cube array slot, and per probe its
    /// `lightingSH`.
    pub t6_probe_slot: Option<u32>,
    pub t6_probes: Arc<Vec<[[f32; 4]; 3]>>,
}

/// bo2zm: one Black Ops II thing that moves, drawn by the fallback pass
/// with the props' pipelines: vertices in its own space, indices, per draw
/// `[first index, index count, colour slot, draw code, instance]`, and the
/// instance rows (world-from-local columns, light, primary light,
/// visibility).
#[derive(Clone, Debug, Default)]
pub struct T6DynamicDraw {
    pub vertices: Vec<SmodelVertex>,
    pub indices: Vec<u32>,
    /// (first index, count, colour slot, draw code, instance, normal map
    /// slot, specular map slot)
    pub draws: Vec<[u32; 7]>,
    pub instances: Vec<[f32; 24]>,
}

/// bo2zm M3: the map's script models that do not animate (mannequins, perk
/// machines, the box, wall guns): their shapes in one geometry uploaded
/// once (`revision` changes when a new model joins), placed by this frame's
/// instance rows. Re-sending ~150 models' vertices every frame cost ~12 ms.
#[derive(Clone, Debug, Default)]
pub struct T6StaticDraw {
    pub geometry: Arc<(Vec<SmodelVertex>, Vec<u32>)>,
    pub revision: u64,
    pub draws: Vec<[u32; 7]>,
    pub instances: Vec<[f32; 24]>,
}

/// bo2zm: Black Ops II things that move, extracted every frame: the
/// first-person gun and arms. `materials` lists every T6 material's colour
/// slot and draw code, so pipelines and texture bind groups exist for
/// whatever a gun or effect draws with.
#[derive(Resource, Clone, Debug, Default)]
pub struct ExtractedT6Dynamic {
    pub generation: MaterialGenerationId,
    pub materials: Arc<Vec<(u32, u32)>>,
    pub viewmodel: Option<T6DynamicDraw>,
    /// Bullet holes and other decal marks, in world space.
    pub marks: Option<T6DynamicDraw>,
    /// Projectiles in flight (grenades, rockets), each in its own space.
    pub missiles: Option<T6DynamicDraw>,
    /// bo2zm M3: the script models that hold still.
    pub script_static: Option<T6StaticDraw>,
    /// Lights effects throw (origin, radius; colour), nearest first.
    pub dlights: Vec<[f32; 8]>,
    /// Effect sprites, in world space.
    pub effects: Option<T6DynamicDraw>,
    /// Effect sprites drawn with the first-person gun (muzzle flashes).
    pub viewmodel_effects: Option<T6DynamicDraw>,
}

/// bo2zm: one uploaded `T6DynamicDraw`: vertex, index and instance
/// buffers, then the draws.
type DynamicBuffers = (Buffer, Buffer, Buffer, Vec<[u32; 7]>);

/// bo2zm: the GPU side of `ExtractedT6Dynamic`.
#[derive(Resource, Default)]
struct DiagnosticDynamic {
    marks: Option<DynamicBuffers>,
    missiles: Option<DynamicBuffers>,
    script_static: Option<DynamicBuffers>,
    /// The still script models' shapes as uploaded (revision, vertices,
    /// indices).
    script_static_geometry: Option<(u64, Buffer, Buffer)>,
    viewmodel: Option<DynamicBuffers>,
    effects: Option<DynamicBuffers>,
    viewmodel_effects: Option<DynamicBuffers>,
}

fn upload_dynamic_draw(device: &RenderDevice, label: &str, draw: Option<&T6DynamicDraw>) -> Option<DynamicBuffers> {
    let draw = draw?;
    if draw.vertices.is_empty()
        || draw.indices.is_empty()
        || draw.draws.is_empty()
        || draw.instances.is_empty()
    {
        return None;
    }
    let vb = device.create_buffer_with_data(&BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(draw.vertices.as_slice()),
        usage: BufferUsages::VERTEX,
    });
    let ib = device.create_buffer_with_data(&BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(draw.indices.as_slice()),
        usage: BufferUsages::INDEX,
    });
    let inst = device.create_buffer_with_data(&BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(draw.instances.as_slice()),
        usage: BufferUsages::VERTEX,
    });
    Some((vb, ib, inst, draw.draws.clone()))
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct DiagnosticSmodelInstance {
    world_from_local: [[f32; 4]; 4],
}

#[derive(Resource, Default)]
struct DiagnosticGeometry {
    generation: MaterialGenerationId,
    world_vertex: Option<Buffer>,
    world_index: Option<Buffer>,
    world_surface_ranges: Arc<Vec<(u32, u32)>>,
    smodel_vertex: Option<Buffer>,
    smodel_index: Option<Buffer>,
    smodel_surface_ranges: Arc<Vec<(u32, u32)>>,
    world_vertex_count: usize,
    world_index_count: usize,
    smodel_vertex_count: usize,
    smodel_index_count: usize,
    xmodel_vertex: Option<Buffer>,
    xmodel_index: Option<Buffer>,
    xmodel_surface_ranges: Arc<Vec<(u32, u32)>>,
    xmodel_vertex_count: usize,
    xmodel_index_count: usize,
    xmodel_revision: u64,

    g0_world_surfs: Vec<u16>,
    world_surface_color: Arc<Vec<u32>>,
    world_surface_draw: Arc<Vec<u32>>,
    world_surface_lightmap: Arc<Vec<u8>>,
    t6_lighting: Option<[[f32; 4]; 3]>,
    t6_props: Arc<Vec<[u32; 4]>>,
    t6_prop_shine: Arc<Vec<[u32; 3]>>,
    world_surface_primary: Arc<Vec<u8>>,
    t6_lights: Arc<Vec<[f32; 16]>>,
    world_surface_layers: Arc<Vec<[u32; 2]>>,
    /// bo2zm: per world surface, normal and specular map slots and probe.
    world_surface_shine: Arc<Vec<[u32; 3]>>,
    t6_probe_slot: Option<u32>,
    t6_probes: Arc<Vec<[[f32; 4]; 3]>>,
    /// bo2zm: the world's layer texcoords, parallel to its vertices.
    world_layer_vertex: Option<Buffer>,
    world_layer_uvs: Arc<Vec<[f32; 4]>>,
    /// bo2zm: every textured pipeline the world surfaces and props need.
    t6_styles: Vec<DiagnosticTess>,
    /// bo2zm: the blended props' draw order (indices into `t6_props`, by
    /// sort key).
    t6_blended_props: Vec<usize>,
    t6_prop_instances: Arc<Vec<[f32; 24]>>,
    t6_prop_instance_buffer: Option<Buffer>,
    t6_sky_slot: Option<u32>,
}

/// bo2zm: one bind group per lightmap page the world surfaces read (page
/// view, sampler, T6 lighting constants), over the uploaded lightmap views.
#[derive(Resource, Default)]
struct DiagnosticWorldLightmaps {
    generation: MaterialGenerationId,
    sampler: Option<Sampler>,
    constants: Option<Buffer>,
    written: Option<[[f32; 4]; 3]>,
    bind_groups: HashMap<u8, BindGroup>,
    /// The sky: its cube map's slot and bind group (cube, sampler, lighting).
    sky: Option<(u32, BindGroup)>,
    /// The primary lights uniform (`T6_LIGHTS_BYTES`) and what it holds.
    lights: Option<Buffer>,
    lights_written: Option<Arc<Vec<[f32; 16]>>>,
    /// Group 2 for surfaces without a lightmap: lighting and lights only.
    lighting_group: Option<BindGroup>,
    /// The lights effects throw (`T6_DYN_BYTES`), rewritten every frame.
    dyn_lights: Option<Buffer>,
}

/// bo2zm: the effect lights uniform: a count, then 16 lights of two vec4.
const T6_DYN_LIGHTS: usize = 16;
const T6_DYN_BYTES: u64 = (16 + T6_DYN_LIGHTS * 32) as u64;

/// bo2zm: the primary lights uniform: 32 lights of four vec4.
const T6_MAX_LIGHTS: usize = 32;
const T6_LIGHTS_BYTES: u64 = (T6_MAX_LIGHTS * 64) as u64;

/// bo2zm: Black Ops II draw codes (see `asset_core::T6Draw::code`).
const T6_BLEND_MASK: u32 = 0x0f;
const T6_UNLIT: u32 = 0x10;
const T6_SHADOW_ONLY: u32 = 4;
const T6_NONE: u32 = u32::MAX;
/// Draw-code bit: a layered material; its vertex colour is layer weights,
/// not a tint (lightmapped styles only).
const T6_LAYERED: u32 = 0x40;
/// Draw-code bit: alpha tested (opaque styles only).
const T6_ALPHA_TEST: u32 = 0x80;
/// Draw-code bits: cull (8..9) and polygon offset (10..11).
const T6_RASTER_MASK: u32 = 0x0f00;

/// Style bit: the surface reads its lightmap (lit codes only).
const STYLE_LIGHTMAPPED: u16 = 0x20;
/// Style bit: a lit lightmapped opaque surface with a normal or specular
/// map draws with BO2's shine: normal map, gloss highlight, reflection
/// probe (group 1 bindings 8, 9; group 3).
const STYLE_SHINE: u16 = 0x08;
/// The reflection probe table: per probe its `lightingSH`, three vec4.
const T6_MAX_PROBES: usize = 32;
const T6_PROBES_BYTES: u64 = (T6_MAX_PROBES * 48) as u64;
/// Draw-code bits: the unlit colour scale's log2 (12..14), extra layers
/// 1 and 2 (24..25, 26..27).
const T6_UNLIT_SCALE_MASK: u32 = 0x7000;
const T6_LAYER_SHIFT: u32 = 24;

/// The sort key a draw code carries (blended surfaces draw in its order).
fn t6_sort_key(code: u32) -> u32 {
    if code == T6_NONE { 0 } else { (code >> 16) & 0xff }
}

/// A draw code blends (alpha, add or multiply).
fn t6_blended(code: u32) -> bool {
    code != T6_NONE && matches!(code & T6_BLEND_MASK, 1..=3)
}

/// The pipeline style of a world surface: blend (bits 0..1), unlit (4),
/// lightmapped (5), layered (6), alpha tested (7), cull (8..9), polygon
/// offset (10..11). A surface with no BO2 draw is style 0.
fn world_style(code: u32, lightmapped: bool) -> u16 {
    if code == T6_NONE {
        return 0;
    }
    let mut style = (code & (3 | T6_UNLIT | T6_RASTER_MASK)) as u16;
    if code & T6_UNLIT != 0 {
        style |= (code & T6_UNLIT_SCALE_MASK) as u16;
    }
    if lightmapped && code & T6_UNLIT == 0 {
        style |= STYLE_LIGHTMAPPED | (code & T6_LAYERED) as u16;
        // Lit lightmapped styles keep the extra layers in bits 12..15.
        style |= (((code >> T6_LAYER_SHIFT) & 0xf) as u16) << 12;
    }
    if code & T6_ALPHA_TEST != 0 && code & 3 == 0 {
        style |= T6_ALPHA_TEST as u16;
    }
    // bo2mp: water and emissive flow mark their lit lightmapped style with
    // bit 2 (free in world styles: the blend keeps bits 0..1), the kind in
    // the layer bits.
    if code & T6_WATER != 0 && style & STYLE_LIGHTMAPPED != 0 {
        style = (style & !STYLE_LAYER_MASK) | STYLE_WATER;
    }
    // bo2mp: emissive flow (lava), likewise.
    if code & T6_FLOW != 0 && style & STYLE_LIGHTMAPPED != 0 && code & T6_UNLIT == 0 {
        style = (style & !STYLE_LAYER_MASK) | STYLE_FLOW;
    }
    // bo2mp: a test layer 1 (`t1c1`): bit 2 with layer 1's kind blend.
    if code & T6_TEST1 != 0 && style_layers(style) && (style >> 12) & 3 == 1 {
        style |= STYLE_WORLD_OWN;
    }
    style
}

/// Draw-code bit of a lit code: layer 1 is a test layer (`t1c1`).
const T6_TEST1: u32 = 0x2000;

/// A style without its extra layers (the base layer alone).
fn strip_layers(style: u16) -> u16 {
    if style_layers(style) {
        style & !(STYLE_LAYER_MASK | STYLE_WORLD_OWN)
    } else {
        style
    }
}

/// Draw-code bit: BO2's emissive flow (`cod7emissiveflow`: molten lava).
const T6_FLOW: u32 = 0x8000_0000;
/// Style bits of a lit lightmapped emissive flow surface.
const STYLE_FLOW: u16 = STYLE_WORLD_OWN | 0x2000;

/// A lit lightmapped emissive flow style.
fn style_flow(style: u16) -> bool {
    style & STYLE_LIGHTMAPPED != 0
        && style & T6_UNLIT as u16 == 0
        && style & (STYLE_WORLD_OWN | STYLE_LAYER_MASK) == STYLE_FLOW
}

/// Draw-code bit: BO2's water (`cod7water`, `cod7watershore`).
const T6_WATER: u32 = 0x4000_0000;
/// Style bit of a lit lightmapped world surface drawn by a shader of its
/// own (water: no layer bits; emissive flow: layer bits 0x2000), or of a
/// layered one whose layer 1 is a test layer (layer 1's kind blend).
const STYLE_WORLD_OWN: u16 = 0x04;
/// Style bits of a lit lightmapped water surface.
const STYLE_WATER: u16 = STYLE_WORLD_OWN;

/// A lit lightmapped water style.
fn style_water(style: u16) -> bool {
    style & STYLE_LIGHTMAPPED != 0
        && style & T6_UNLIT as u16 == 0
        && style & (STYLE_WORLD_OWN | STYLE_LAYER_MASK) == STYLE_WATER
}

/// Draw-code bit / prop style bit: a BO2 effect technique (props are never
/// lightmapped, so the bit is free in prop styles).
const T6_EFFECT: u32 = 0x20;
/// Draw-code bit / prop style bit: a BO2 particle cloud (bit 15 is free in
/// both).
const T6_CLOUD: u32 = 0x8000;
/// Prop style bit: an effect drawn with soft edges (BO2's zfeather pixel
/// shaders), reading the scene depth (group 3). Props are never layered,
/// so the layered bit is free in prop styles.
const STYLE_SOFT: u16 = 0x40;
/// Draw-code bit: BO2's objective technique (the box's question marks).
const T6_OBJECTIVE: u32 = 0x1000_0000;
/// Prop style bit: an objective surface (bit 2 is free in prop styles: the
/// blend keeps bits 0..1).
const STYLE_OBJECTIVE: u16 = 0x04;

/// Draw-code bit: BO2's foliage technique (`treecanopy`): the vertex colour
/// is wind data and a grid-light scale, not a tint.
const T6_FOLIAGE: u32 = 0x2000_0000;
/// Prop style bit: a lit foliage surface (bit 12 is the unlit colour scale,
/// unused by lit styles).
const STYLE_FOLIAGE: u16 = 0x1000;

/// The pipeline style of a prop surface (never lightmapped or layered).
fn prop_style(code: u32) -> u16 {
    if code == T6_NONE {
        return 0;
    }
    let mut style = (code & (3 | T6_UNLIT | T6_RASTER_MASK | T6_EFFECT | T6_CLOUD)) as u16;
    if code & T6_UNLIT != 0 {
        style |= (code & T6_UNLIT_SCALE_MASK) as u16;
    }
    if code & T6_ALPHA_TEST != 0 && code & 3 == 0 {
        style |= T6_ALPHA_TEST as u16;
    }
    if code & T6_OBJECTIVE != 0 {
        style |= STYLE_OBJECTIVE;
    }
    if code & T6_FOLIAGE != 0 && code & T6_UNLIT == 0 {
        style |= STYLE_FOLIAGE;
    }
    if code & T6_FLOW != 0 && code & T6_UNLIT == 0 {
        style |= STYLE_PROP_FLOW;
    }
    if code & T6_TILE != 0 && code & T6_UNLIT == 0 {
        style |= STYLE_PROP_TILE;
    }
    // bo2mp: a tattered flag tests its own alpha (the mixed edge), not the
    // diffuse's (gloss).
    if code & T6_FLAG != 0 && code & T6_UNLIT == 0 {
        style = (style & !(T6_ALPHA_TEST as u16)) | STYLE_PROP_FLAG;
    }
    style
}

/// Draw-code bit of a lit code: BO2's tattered flag (`flag_tatters`).
const T6_FLAG: u32 = 0x4000;
/// Prop style bit: a lit tattered flag (bit 6 is the soft bit of effects
/// only, which are unlit).
const STYLE_PROP_FLAG: u16 = 0x40;

/// Draw-code bit of a lit code: BO2's emissive tile (`cod7_emissive_tile`;
/// bit 12 is the unlit colour scale in unlit codes).
const T6_TILE: u32 = 0x1000;
/// Prop style bit: a lit emissive tile surface (bit 14, likewise free).
const STYLE_PROP_TILE: u16 = 0x4000;

/// Prop style bit: a lit emissive flow surface (lava; bit 13 is the unlit
/// colour scale, unused by lit styles).
const STYLE_PROP_FLOW: u16 = 0x2000;

/// bo2zm: one dynamic list (marks, projectiles, effects, the gun) into a
/// pass: the draws `keep` admits, opaque first, then blended by sort key;
/// effects with soft edges when `soft` (the scene depth) is given. Returns
/// the draws made.
#[allow(clippy::too_many_arguments)]
fn draw_dynamic_list<'w>(
    pass: &mut bevy::render::render_phase::TrackedRenderPass<'w>,
    buffers: Option<&'w DynamicBuffers>,
    lighting_group: Option<&'w BindGroup>,
    depth_range: i32,
    viewport: [f32; 4],
    keep: &dyn Fn(u32) -> bool,
    soft: Option<&'w BindGroup>,
    cache: &'w PipelineCache,
    pipelines: &HashMap<DiagnosticTess, CachedRenderPipelineId>,
    world_textures: &'w DiagnosticWorldTextures,
) -> u32 {
    let (Some((vb, ib, inst, draws)), Some(lighting_group)) = (buffers, lighting_group) else {
        return 0;
    };
    let [vp_x, vp_y, vp_w, vp_h] = viewport;
    if vp_w > 0.0 && vp_h > 0.0 {
        let (depth_min, depth_max) = reverse_z_viewport_depth(depth_range);
        pass.set_viewport(vp_x, vp_y, vp_w, vp_h, depth_min, depth_max);
    }
    pass.set_bind_group(2, lighting_group, &[]);
    pass.set_vertex_buffer(0, vb.slice(..));
    pass.set_vertex_buffer(1, inst.slice(..));
    pass.set_index_buffer(ib.slice(..), IndexFormat::Uint32);
    let mut order: Vec<usize> = (0..draws.len()).filter(|&i| keep(draws[i][3])).collect();
    order.sort_by_key(|&i| (t6_blended(draws[i][3]), t6_sort_key(draws[i][3])));
    let mut bound: Option<DiagnosticTess> = None;
    let mut bound_slot = None;
    let mut drawn = 0u32;
    for i in order {
        let [start, count, slot, code, instance, normal, specular] = draws[i];
        if count == 0 || (code != T6_NONE && code & T6_BLEND_MASK >= T6_SHADOW_ONLY) {
            continue;
        }
        let shine_group = (prop_shine_code(code) && (normal != u32::MAX || specular != u32::MAX))
            .then(|| world_textures.probes.as_ref())
            .flatten()
            .and_then(|_| {
                world_textures
                    .shine
                    .get(&(slot, normal, specular, u32::MAX))
            });
        let soft_here = soft.filter(|_| soft_code(code));
        let style = prop_style(code)
            | if shine_group.is_some() { STYLE_SHINE } else { 0 }
            | if soft_here.is_some() { STYLE_SOFT } else { 0 };
        let key = DiagnosticTess::PropTextured(style);
        let pipeline = pipelines.get(&key).and_then(|&id| cache.get_render_pipeline(id));
        let (Some(pipeline), Some(bind_group)) = (
            pipeline,
            shine_group.or_else(|| world_textures.bind_groups.get(&slot)),
        ) else {
            // IW4L_T6_DRAWLOG=1: why a draw was left out.
            static LOG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            if *LOG.get_or_init(|| std::env::var_os("IW4L_T6_DRAWLOG").is_some()) {
                diag::info!(
                    World,
                    "bo2zm draw skipped: slot {slot} code {code:#x} style {style:#x} pipeline {} ready {} texture {}",
                    pipelines.contains_key(&key),
                    pipeline.is_some(),
                    world_textures.bind_groups.contains_key(&slot)
                );
            }
            continue;
        };
        if bound != Some(key) {
            pass.set_render_pipeline(pipeline);
            if shine_group.is_some()
                && let Some((_, probes)) = world_textures.probes.as_ref()
            {
                pass.set_bind_group(3, probes, &[]);
            }
            if let Some(soft) = soft_here {
                pass.set_bind_group(3, soft, &[]);
            }
            bound = Some(key);
        }
        let slot_key = (slot, shine_group.map(|_| [normal, specular]));
        if bound_slot != Some(slot_key) {
            pass.set_bind_group(1, bind_group, &[]);
            bound_slot = Some(slot_key);
        }
        pass.draw_indexed(start..start.saturating_add(count), 0, instance..instance + 1);
        drawn += 1;
    }
    drawn
}

/// bo2zm: an effect code that draws with soft edges in the world (BO2's
/// zfeather techniques; a material without `featherParms` reads none of
/// it): blended or added, not a particle cloud.
fn soft_code(code: u32) -> bool {
    code != T6_NONE && code & T6_EFFECT != 0 && code & T6_CLOUD == 0 && t6_blended(code)
}

/// bo2zm: a prop or dynamic code that can draw with shine: lit, opaque or
/// blended (glass), not an effect.
fn prop_shine_code(code: u32) -> bool {
    code != T6_NONE && code & T6_UNLIT == 0 && code & 3 <= 1 && code & T6_EFFECT == 0
}

/// Style bits of a lit lightmapped style's extra layers (12..15).
const STYLE_LAYER_MASK: u16 = 0xf000;

/// A style draws extra layers (lit, lightmapped, with layer kinds).
fn style_layers(style: u16) -> bool {
    style & STYLE_LIGHTMAPPED != 0
        && style & T6_UNLIT as u16 == 0
        && style & STYLE_LAYER_MASK != 0
        && !style_water(style)
        && !style_flow(style)
}

/// Every textured pipeline the codes need: each lit world code with and
/// without its lightmap, each prop code.
fn t6_styles(world: &[u32], props: &[[u32; 4]]) -> Vec<DiagnosticTess> {
    let mut styles = std::collections::BTreeSet::new();
    for &code in world {
        if code != T6_NONE && code & T6_BLEND_MASK >= T6_SHADOW_ONLY {
            continue;
        }
        styles.insert(DiagnosticTess::WorldTextured(world_style(code, false)));
        styles.insert(DiagnosticTess::WorldTextured(world_style(code, true)));
        // Layered: also the base-layer-only style (layer maps missing).
        styles.insert(DiagnosticTess::WorldTextured(strip_layers(world_style(
            code, true,
        ))));
        // Opaque lit lightmapped: also with shine.
        let lit = strip_layers(world_style(code, true));
        if lit & STYLE_LIGHTMAPPED != 0 && lit & 3 <= 1 {
            styles.insert(DiagnosticTess::WorldTextured(lit | STYLE_SHINE));
        }
        // bo2mp: water and emissive flow draw only with their shine maps
        // and probes.
        if style_water(world_style(code, true)) || style_flow(world_style(code, true)) {
            styles.insert(DiagnosticTess::WorldTextured(
                world_style(code, true) | STYLE_SHINE,
            ));
        }
    }
    for draw in props {
        let code = draw[2];
        if code != T6_NONE && code & T6_BLEND_MASK >= T6_SHADOW_ONLY {
            continue;
        }
        if prop_shine_code(code) {
            styles.insert(DiagnosticTess::PropTextured(prop_style(code) | STYLE_SHINE));
        }
        styles.insert(DiagnosticTess::PropTextured(prop_style(code)));
    }
    styles.into_iter().collect()
}

/// bo2zm: one bind group per colour map slot the world surfaces sample,
/// over the uploaded material image views; rebuilt when the catalog
/// generation changes.
#[derive(Resource, Default)]
struct DiagnosticWorldTextures {
    generation: MaterialGenerationId,
    sampler: Option<Sampler>,
    bind_groups: HashMap<u32, BindGroup>,
    /// bo2zm: layered surfaces' group 1, by (colour, layer 1, layer 2)
    /// slots; a missing layer reuses the colour map (its weight is still
    /// applied, so the layer kind decides whether one is bound).
    layered: HashMap<(u32, u32, u32), BindGroup>,
    /// bo2zm: shine surfaces' group 1, by (colour, normal, specular)
    /// slots; a missing map binds a flat normal / black specular.
    shine: HashMap<(u32, u32, u32, u32), BindGroup>,
    /// bo2zm: 1x1 stand-ins: a flat normal, a black specular map.
    flat_normal: Option<bevy::render::render_resource::TextureView>,
    no_specular: Option<bevy::render::render_resource::TextureView>,
    /// bo2zm: group 3 of shine surfaces: the probes' cube array, its
    /// sampler and their lighting table, for the cube array's slot.
    probes: Option<(u32, BindGroup)>,
    probe_table: Option<Buffer>,
    probe_written: Option<Arc<Vec<[[f32; 4]; 3]>>>,
}

#[derive(Resource)]
struct DiagnosticPipeline {
    view_layout: BindGroupLayoutDescriptor,
    texture_layout: BindGroupLayoutDescriptor,
    lightmap_layout: BindGroupLayoutDescriptor,
    /// bo2zm: group 2 without a lightmap (bindings 2, 3).
    lighting_layout: BindGroupLayoutDescriptor,
    /// bo2zm: group 1 of layered surfaces: colour map, sampler, layer 1
    /// and 2 colour maps (bindings 0, 1, 6, 7).
    layered_texture_layout: BindGroupLayoutDescriptor,
    sky_layout: BindGroupLayoutDescriptor,
    /// bo2zm: group 1 of shine surfaces: colour map, sampler, normal and
    /// specular maps (bindings 0, 1, 8, 9).
    shine_texture_layout: BindGroupLayoutDescriptor,
    /// bo2zm: group 3 of shine surfaces: the reflection probes' cube
    /// array, its sampler, their lighting table (bindings 0..2).
    probe_layout: BindGroupLayoutDescriptor,
    /// bo2zm: group 3 of soft effects: the scene depth (binding 0).
    soft_layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct DiagnosticPipelineKey {
    target: TextureFormat,
    samples: u32,
    tess: DiagnosticTess,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum DiagnosticTess {
    World,
    /// bo2zm: world surfaces with a colour map (group 1 = colour map), by
    /// style (`world_style`).
    WorldTextured(u16),
    /// bo2zm: Black Ops II props with a colour map, by style (`prop_style`).
    PropTextured(u16),
    /// bo2zm: the Black Ops II sky, a full-screen triangle at the far plane
    /// sampling its cube map (group 1, bindings 3..5).
    Sky,
    Smodel,
    XModel,
}

impl SpecializedRenderPipeline for DiagnosticPipeline {
    type Key = DiagnosticPipelineKey;

    fn specialize(&self, key: Self::Key) -> RenderPipelineDescriptor {
        // bo2zm: one shader-def list for both stages (the layered
        // vertex input and the fragment variants read it).
        let defs: Vec<ShaderDefVal> = match key.tess {
                    DiagnosticTess::WorldTextured(style) | DiagnosticTess::PropTextured(style) => {
                        // The unlit colour scale (`scaleRGB`), a power of two.
                        let mut defs = vec![ShaderDefVal::UInt(
                            "UNLIT_SCALE".into(),
                            1u32 << ((style >> 12) & 7),
                        )];
                        if style & T6_UNLIT as u16 != 0 {
                            defs.push("UNLIT".into());
                            if style & 3 == 3 {
                                defs.push("MULTIPLY".into());
                            }
                        }
                        if style & T6_ALPHA_TEST as u16 != 0 {
                            defs.push("ALPHA_TEST".into());
                        }
                        if style & STYLE_LIGHTMAPPED != 0 && style & T6_LAYERED as u16 != 0 {
                            defs.push("UNTINTED".into());
                        }
                        if style & STYLE_SHINE != 0 {
                            defs.push("SHINE".into());
                            if style_water(style) {
                                defs.push("WATER".into());
                            }
                            if style_flow(style) {
                                defs.push("FLOW".into());
                            }
                        }
                        if matches!(key.tess, DiagnosticTess::PropTextured(_))
                            && style & STYLE_OBJECTIVE != 0
                        {
                            defs.push("OBJECTIVE".into());
                        }
                        if matches!(key.tess, DiagnosticTess::PropTextured(_))
                            && style & T6_UNLIT as u16 == 0
                            && style & STYLE_FOLIAGE != 0
                        {
                            defs.push("FOLIAGE".into());
                        }
                        if matches!(key.tess, DiagnosticTess::PropTextured(_))
                            && style & T6_UNLIT as u16 == 0
                            && style & STYLE_PROP_FLOW != 0
                            && style & STYLE_SHINE != 0
                        {
                            defs.push("FLOW".into());
                        }
                        if matches!(key.tess, DiagnosticTess::PropTextured(_))
                            && style & T6_UNLIT as u16 == 0
                            && style & STYLE_PROP_TILE != 0
                            && style & STYLE_SHINE != 0
                        {
                            defs.push("TILE".into());
                        }
                        if matches!(key.tess, DiagnosticTess::PropTextured(_))
                            && style & (T6_UNLIT | T6_EFFECT) as u16 == 0
                            && style & STYLE_PROP_FLAG != 0
                            && style & STYLE_SHINE != 0
                        {
                            defs.push("FLAG".into());
                        }
                        if matches!(key.tess, DiagnosticTess::PropTextured(_))
                            && style & T6_EFFECT as u16 != 0
                        {
                            defs.push("EFFECT".into());
                            match style & 3 {
                                2 => defs.push("EFFECT_ADD".into()),
                                3 => defs.push("EFFECT_MULTIPLY".into()),
                                _ => {}
                            }
                            if u32::from(style) & T6_CLOUD != 0 {
                                defs.push("CLOUD".into());
                            }
                            if style & STYLE_SOFT != 0 {
                                defs.push("SOFT".into());
                            }
                        }
                        if style_layers(style) {
                            defs.push("LAYERS".into());
                            for (k, shift) in [(1, 12), (2, 14)] {
                                match (style >> shift) & 3 {
                                    1 if k == 1 && style & STYLE_WORLD_OWN != 0 => {
                                        defs.push("LAYER1_TEST".into())
                                    }
                                    1 => defs.push(format!("LAYER{k}_BLEND").into()),
                                    2 => defs.push(format!("LAYER{k}_MULTIPLY").into()),
                                    3 => defs.push(format!("LAYER{k}_ADD").into()),
                                    _ => {}
                                }
                            }
                        }
                        defs
                    }
                    _ => Vec::new(),
                };
        RenderPipelineDescriptor {
            label: Some("iw4_geometry_diagnostic".into()),
            layout: match key.tess {
                DiagnosticTess::Sky => vec![self.view_layout.clone(), self.sky_layout.clone()],
                DiagnosticTess::WorldTextured(style) if style_layers(style) => vec![
                    self.view_layout.clone(),
                    self.layered_texture_layout.clone(),
                    self.lightmap_layout.clone(),
                ],
                DiagnosticTess::WorldTextured(style) if style & STYLE_SHINE != 0 => vec![
                    self.view_layout.clone(),
                    self.shine_texture_layout.clone(),
                    self.lightmap_layout.clone(),
                    self.probe_layout.clone(),
                ],
                DiagnosticTess::WorldTextured(style) if style & STYLE_LIGHTMAPPED != 0 => vec![
                    self.view_layout.clone(),
                    self.texture_layout.clone(),
                    self.lightmap_layout.clone(),
                ],
                DiagnosticTess::PropTextured(style)
                    if style & STYLE_SOFT != 0 && style & T6_EFFECT as u16 != 0 =>
                {
                    vec![
                        self.view_layout.clone(),
                        self.texture_layout.clone(),
                        self.lighting_layout.clone(),
                        self.soft_layout.clone(),
                    ]
                }
                DiagnosticTess::PropTextured(style) if style & STYLE_SHINE != 0 => vec![
                    self.view_layout.clone(),
                    self.shine_texture_layout.clone(),
                    self.lighting_layout.clone(),
                    self.probe_layout.clone(),
                ],
                DiagnosticTess::WorldTextured(_) | DiagnosticTess::PropTextured(_) => vec![
                    self.view_layout.clone(),
                    self.texture_layout.clone(),
                    self.lighting_layout.clone(),
                ],
                _ => vec![self.view_layout.clone()],
            },
            immediate_size: 0,
            vertex: VertexState {
                shader: self.shader.clone(),
                shader_defs: defs.clone(),
                entry_point: Some(match key.tess {
                    DiagnosticTess::World | DiagnosticTess::WorldTextured(_) => "vertex".into(),
                    DiagnosticTess::PropTextured(_) => "vertex_prop".into(),
                    DiagnosticTess::Sky => "vertex_sky".into(),
                    DiagnosticTess::Smodel | DiagnosticTess::XModel => "vertex_smodel".into(),
                }),
                buffers: match key.tess {
                    DiagnosticTess::WorldTextured(style) if style_layers(style) => {
                        vec![world_vertex_layout(), world_layer_vertex_layout()]
                    }
                    DiagnosticTess::World | DiagnosticTess::WorldTextured(_) => {
                        vec![world_vertex_layout()]
                    }
                    DiagnosticTess::PropTextured(_) => {
                        vec![smodel_vertex_layout(), prop_instance_layout()]
                    }
                    DiagnosticTess::Sky => Vec::new(),
                    DiagnosticTess::Smodel | DiagnosticTess::XModel => {
                        vec![smodel_vertex_layout(), smodel_instance_layout()]
                    }
                },
            },
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                shader_defs: defs,
                entry_point: Some(match key.tess {
                    DiagnosticTess::Sky => "fragment_sky".into(),
                    DiagnosticTess::PropTextured(_) => "fragment_prop".into(),
                    DiagnosticTess::WorldTextured(style) if style & STYLE_LIGHTMAPPED != 0 => {
                        "fragment_lightmapped".into()
                    }
                    DiagnosticTess::WorldTextured(_) => "fragment_textured".into(),
                    _ => "fragment".into(),
                }),
                targets: vec![Some(ColorTargetState {
                    format: key.target,
                    blend: match key.tess {
                        DiagnosticTess::WorldTextured(code) | DiagnosticTess::PropTextured(code) => {
                            t6_blend_state(code)
                        }
                        _ => None,
                    },
                    write_mask: ColorWrites::ALL,
                })],
            }),
            // bo2zm: the material's own cull (as the engine maps IW
            // GFXS_CULL_*: back = wgpu back with clockwise front faces).
            primitive: PrimitiveState {
                front_face: FrontFace::Cw,
                cull_mode: match key.tess {
                    DiagnosticTess::WorldTextured(style) | DiagnosticTess::PropTextured(style) => {
                        match (style >> 8) & 3 {
                            1 => Some(Face::Back),
                            2 => Some(Face::Front),
                            _ => None,
                        }
                    }
                    _ => None,
                },
                ..Default::default()
            },
            depth_stencil: Some(DepthStencilState {
                format: SCENE_DEPTH_FORMAT,
                depth_write_enabled: Some(match key.tess {
                    // bo2mp: water blends here (the game refracts instead)
                    // but stays solid in depth, as the game's: the sky, drawn
                    // last where nothing is, must not cover an open sea.
                    DiagnosticTess::WorldTextured(style) if style_water(style) => true,
                    DiagnosticTess::WorldTextured(style) | DiagnosticTess::PropTextured(style) => {
                        style & 3 == 0
                    }
                    DiagnosticTess::Sky => false,
                    _ => true,
                }),
                depth_compare: Some(CompareFunction::GreaterEqual),
                stencil: Default::default(),
                // bo2zm: decals' polygon offset, the engine's IW defaults
                // turned for reverse Z.
                bias: match key.tess {
                    DiagnosticTess::WorldTextured(style) | DiagnosticTess::PropTextured(style)
                        if (style >> 10) & 3 != 0 =>
                    {
                        let (slope_scale, constant) =
                            asset_iw4::polygon_offset_wgpu_defaults(u32::from((style >> 10) & 3));
                        DepthBiasState {
                            constant: -constant,
                            slope_scale: -slope_scale,
                            clamp: 0.0,
                        }
                    }
                    _ => Default::default(),
                },
            }),
            multisample: MultisampleState {
                count: key.samples,
                ..Default::default()
            },
            zero_initialize_workgroup_memory: false,
        }
    }
}

/// bo2zm: the colour blend for a Black Ops II draw code: alpha blend, add,
/// or multiply what is behind; opaque has none.
fn t6_blend_state(code: u16) -> Option<BlendState> {
    let keep_alpha = BlendComponent {
        src_factor: BlendFactor::Zero,
        dst_factor: BlendFactor::One,
        operation: BlendOperation::Add,
    };
    let color = match code & 3 {
        1 => BlendComponent {
            src_factor: BlendFactor::SrcAlpha,
            dst_factor: BlendFactor::OneMinusSrcAlpha,
            operation: BlendOperation::Add,
        },
        2 => BlendComponent {
            src_factor: BlendFactor::SrcAlpha,
            dst_factor: BlendFactor::One,
            operation: BlendOperation::Add,
        },
        3 => BlendComponent {
            src_factor: BlendFactor::Dst,
            dst_factor: BlendFactor::Zero,
            operation: BlendOperation::Add,
        },
        _ => return None,
    };
    Some(BlendState {
        color,
        alpha: keep_alpha,
    })
}

#[derive(Component)]
struct DiagnosticViewBindGroup {
    bind_group: BindGroup,
    world_pipeline: CachedRenderPipelineId,
    /// bo2zm: the textured world and prop pipelines, by key.
    t6_pipelines: HashMap<DiagnosticTess, CachedRenderPipelineId>,
    sky_pipeline: CachedRenderPipelineId,
    smodel_pipeline: CachedRenderPipelineId,
    xmodel_pipeline: CachedRenderPipelineId,
}

fn world_vertex_layout() -> VertexBufferLayout {
    VertexBufferLayout {
        array_stride: size_of::<WorldVertex>() as u64,
        step_mode: VertexStepMode::Vertex,
        attributes: vec![
            VertexAttribute {
                format: VertexFormat::Float32x3,
                offset: 0,
                shader_location: 0,
            },
            VertexAttribute {
                format: VertexFormat::Float32x3,
                offset: 12,
                shader_location: 1,
            },
            VertexAttribute {
                format: VertexFormat::Float32x4,
                offset: 24,
                shader_location: 2,
            },
            VertexAttribute {
                format: VertexFormat::Float32x4,
                offset: 40,
                shader_location: 3,
            },
            VertexAttribute {
                format: VertexFormat::Float32x2,
                offset: 56,
                shader_location: 4,
            },
            VertexAttribute {
                format: VertexFormat::Float32x2,
                offset: 64,
                shader_location: 5,
            },
        ],
    }
}

/// bo2zm: a world vertex's layer 1 and 2 texcoords (location 6), from a
/// second buffer parallel to the world vertices.
fn world_layer_vertex_layout() -> VertexBufferLayout {
    VertexBufferLayout {
        array_stride: 16,
        step_mode: VertexStepMode::Vertex,
        attributes: vec![VertexAttribute {
            format: VertexFormat::Float32x4,
            offset: 0,
            shader_location: 6,
        }],
    }
}

fn smodel_vertex_layout() -> VertexBufferLayout {
    VertexBufferLayout {
        array_stride: size_of::<SmodelVertex>() as u64,
        step_mode: VertexStepMode::Vertex,
        attributes: vec![
            VertexAttribute {
                format: VertexFormat::Float32x3,
                offset: 0,
                shader_location: 0,
            },
            VertexAttribute {
                format: VertexFormat::Float32x3,
                offset: 12,
                shader_location: 1,
            },
            VertexAttribute {
                format: VertexFormat::Float32x4,
                offset: 24,
                shader_location: 2,
            },
            VertexAttribute {
                format: VertexFormat::Float32x2,
                offset: 40,
                shader_location: 3,
            },
        ],
    }
}

fn smodel_instance_layout() -> VertexBufferLayout {
    VertexBufferLayout {
        array_stride: size_of::<DiagnosticSmodelInstance>() as u64,
        step_mode: VertexStepMode::Instance,
        attributes: (0..4)
            .map(|column| VertexAttribute {
                format: VertexFormat::Float32x4,
                offset: u64::from(column) * 16,
                shader_location: 6 + column,
            })
            .collect(),
    }
}

/// bo2zm: a prop instance: world-from-local columns (locations 6..9), its
/// light (location 10), then its primary light and visibility (11).
fn prop_instance_layout() -> VertexBufferLayout {
    let mut attributes: Vec<VertexAttribute> = (0..4)
        .map(|column| VertexAttribute {
            format: VertexFormat::Float32x4,
            offset: u64::from(column) * 16,
            shader_location: 6 + column,
        })
        .collect();
    attributes.push(VertexAttribute {
        format: VertexFormat::Float32x4,
        offset: 64,
        shader_location: 10,
    });
    attributes.push(VertexAttribute {
        format: VertexFormat::Float32x4,
        offset: 80,
        shader_location: 11,
    });
    VertexBufferLayout {
        array_stride: 96,
        step_mode: VertexStepMode::Instance,
        attributes,
    }
}

fn init_pipeline(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(DiagnosticPipeline {
        view_layout: BindGroupLayoutDescriptor::new(
            "iw4_geometry_diagnostic_view",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::VERTEX_FRAGMENT,
                (uniform_buffer::<ViewUniform>(true),),
            ),
        ),
        texture_layout: BindGroupLayoutDescriptor::new(
            "bo2zm_geometry_diagnostic_colour_map",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::FRAGMENT,
                (
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    sampler(SamplerBindingType::Filtering),
                ),
            ),
        ),
        sky_layout: BindGroupLayoutDescriptor::new(
            "bo2zm_geometry_diagnostic_sky",
            &BindGroupLayoutEntries::with_indices(
                ShaderStages::FRAGMENT,
                (
                    (3, texture_cube(TextureSampleType::Float { filterable: true })),
                    (4, sampler(SamplerBindingType::Filtering)),
                    (5, uniform_buffer_sized(false, std::num::NonZeroU64::new(48))),
                    (
                        6,
                        uniform_buffer_sized(false, std::num::NonZeroU64::new(T6_LIGHTS_BYTES)),
                    ),
                ),
            ),
        ),
        lightmap_layout: BindGroupLayoutDescriptor::new(
            "bo2zm_geometry_diagnostic_lightmap",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::FRAGMENT,
                (
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    sampler(SamplerBindingType::Filtering),
                    uniform_buffer_sized(false, std::num::NonZeroU64::new(48)),
                    uniform_buffer_sized(false, std::num::NonZeroU64::new(T6_LIGHTS_BYTES)),
                    uniform_buffer_sized(false, std::num::NonZeroU64::new(T6_DYN_BYTES)),
                ),
            ),
        ),
        layered_texture_layout: BindGroupLayoutDescriptor::new(
            "bo2zm_geometry_diagnostic_layered_colour_maps",
            &BindGroupLayoutEntries::with_indices(
                ShaderStages::FRAGMENT,
                (
                    (0, texture_2d(TextureSampleType::Float { filterable: true })),
                    (1, sampler(SamplerBindingType::Filtering)),
                    (6, texture_2d(TextureSampleType::Float { filterable: true })),
                    (7, texture_2d(TextureSampleType::Float { filterable: true })),
                ),
            ),
        ),
        lighting_layout: BindGroupLayoutDescriptor::new(
            "bo2zm_geometry_diagnostic_lighting",
            &BindGroupLayoutEntries::with_indices(
                ShaderStages::FRAGMENT,
                (
                    (2, uniform_buffer_sized(false, std::num::NonZeroU64::new(48))),
                    (
                        3,
                        uniform_buffer_sized(false, std::num::NonZeroU64::new(T6_LIGHTS_BYTES)),
                    ),
                    (
                        4,
                        uniform_buffer_sized(false, std::num::NonZeroU64::new(T6_DYN_BYTES)),
                    ),
                ),
            ),
        ),
        shine_texture_layout: BindGroupLayoutDescriptor::new(
            "bo2zm_geometry_diagnostic_shine_maps",
            &BindGroupLayoutEntries::with_indices(
                ShaderStages::FRAGMENT,
                (
                    (0, texture_2d(TextureSampleType::Float { filterable: true })),
                    (1, sampler(SamplerBindingType::Filtering)),
                    (8, texture_2d(TextureSampleType::Float { filterable: true })),
                    (9, texture_2d(TextureSampleType::Float { filterable: true })),
                    (
                        10,
                        texture_2d(TextureSampleType::Float { filterable: true }),
                    ),
                ),
            ),
        ),
        soft_layout: BindGroupLayoutDescriptor::new(
            "bo2zm_geometry_diagnostic_soft_depth",
            &BindGroupLayoutEntries::with_indices(ShaderStages::FRAGMENT, ((0, texture_depth_2d()),)),
        ),
        probe_layout: BindGroupLayoutDescriptor::new(
            "bo2zm_geometry_diagnostic_probes",
            &BindGroupLayoutEntries::with_indices(
                ShaderStages::FRAGMENT,
                (
                    (0, texture_cube_array(TextureSampleType::Float { filterable: true })),
                    (1, sampler(SamplerBindingType::Filtering)),
                    (2, uniform_buffer_sized(false, std::num::NonZeroU64::new(T6_PROBES_BYTES))),
                ),
            ),
        ),
        shader: asset_server.load(SHADER_PATH),
    });
}

fn upload_geometry(
    source: Res<ExtractedDiagnosticGeometry>,
    mut geometry: ResMut<DiagnosticGeometry>,
    device: Res<RenderDevice>,
    mut bloom: ResMut<super::bo2_bloom::Bo2BloomEnabled>,
) {
    if !geometry_diagnostic_enabled() {
        return;
    }
    // bo2zm: BO2's bloom and final colour on a Black Ops II map (it has T6
    // lighting). (`IW4L_T6_NO_BLOOM` turns the bloom off, not the colour.)
    bloom.0 = source.t6_lighting.is_some();
    if source.overlay_gpu_wait {
        use std::sync::atomic::{AtomicBool, Ordering};
        static SKIPPED: AtomicBool = AtomicBool::new(false);
        if !SKIPPED.swap(true, Ordering::Relaxed) {
            diag::info!(
                World,
                "geometry diagnostic GPU upload skipped: overlay GPU residency wait"
            );
        }
        return;
    }
    let geometry_matches = geometry.world_vertex_count == source.world_vertices.len()
        && geometry.world_index_count == source.world_indices.len()
        && geometry.smodel_vertex_count == source.smodel_vertices.len()
        && geometry.smodel_index_count == source.smodel_indices.len();
    if geometry.generation == source.generation && geometry_matches {
        geometry.world_surface_ranges = Arc::clone(&source.world_surface_ranges);
        geometry.smodel_surface_ranges = Arc::clone(&source.smodel_surface_ranges);
    } else {
        geometry.generation = source.generation;
        geometry.world_vertex = None;
        geometry.world_index = None;
        geometry.world_surface_ranges = Arc::new(Vec::new());
        geometry.smodel_vertex = None;
        geometry.smodel_index = None;
        geometry.smodel_surface_ranges = Arc::new(Vec::new());
        geometry.world_vertex_count = source.world_vertices.len();
        geometry.world_index_count = source.world_indices.len();
        geometry.smodel_vertex_count = source.smodel_vertices.len();
        geometry.smodel_index_count = source.smodel_indices.len();
        if !source.world_vertices.is_empty() && !source.world_indices.is_empty() {
            geometry.world_vertex = Some(device.create_buffer_with_data(&BufferInitDescriptor {
                label: Some("iw4_geometry_diagnostic_world_vb"),
                contents: bytemuck::cast_slice(source.world_vertices.as_slice()),
                usage: BufferUsages::VERTEX,
            }));
            geometry.world_index = Some(device.create_buffer_with_data(&BufferInitDescriptor {
                label: Some("iw4_geometry_diagnostic_world_ib"),
                contents: bytemuck::cast_slice(source.world_indices.as_slice()),
                usage: BufferUsages::INDEX,
            }));
            geometry.world_surface_ranges = Arc::clone(&source.world_surface_ranges);
        }
        if !source.smodel_vertices.is_empty() && !source.smodel_indices.is_empty() {
            geometry.smodel_vertex = Some(device.create_buffer_with_data(&BufferInitDescriptor {
                label: Some("iw4_geometry_diagnostic_smodel_vb"),
                contents: bytemuck::cast_slice(source.smodel_vertices.as_slice()),
                usage: BufferUsages::VERTEX,
            }));
            geometry.smodel_index = Some(device.create_buffer_with_data(&BufferInitDescriptor {
                label: Some("iw4_geometry_diagnostic_smodel_ib"),
                contents: bytemuck::cast_slice(source.smodel_indices.as_slice()),
                usage: BufferUsages::INDEX,
            }));
            geometry.smodel_surface_ranges = Arc::clone(&source.smodel_surface_ranges);
        }
    }

    let xmodel_matches = geometry.xmodel_revision == source.xmodel_revision
        && geometry.xmodel_vertex_count == source.xmodel_vertices.len()
        && geometry.xmodel_index_count == source.xmodel_indices.len();
    if xmodel_matches {
        geometry.xmodel_surface_ranges = Arc::clone(&source.xmodel_surface_ranges);
    } else {
        geometry.xmodel_vertex = None;
        geometry.xmodel_index = None;
        geometry.xmodel_surface_ranges = Arc::new(Vec::new());
        geometry.xmodel_vertex_count = source.xmodel_vertices.len();
        geometry.xmodel_index_count = source.xmodel_indices.len();
        geometry.xmodel_revision = source.xmodel_revision;
        if !source.xmodel_vertices.is_empty() && !source.xmodel_indices.is_empty() {
            geometry.xmodel_vertex = Some(device.create_buffer_with_data(&BufferInitDescriptor {
                label: Some("iw4_geometry_diagnostic_xmodel_vb"),
                contents: bytemuck::cast_slice(source.xmodel_vertices.as_slice()),
                usage: BufferUsages::VERTEX,
            }));
            geometry.xmodel_index = Some(device.create_buffer_with_data(&BufferInitDescriptor {
                label: Some("iw4_geometry_diagnostic_xmodel_ib"),
                contents: bytemuck::cast_slice(source.xmodel_indices.as_slice()),
                usage: BufferUsages::INDEX,
            }));
            geometry.xmodel_surface_ranges = Arc::clone(&source.xmodel_surface_ranges);
        }
    }
    geometry.g0_world_surfs.clone_from(&source.g0_world_surfs);
    geometry.world_surface_color = Arc::clone(&source.world_surface_color);
    if !Arc::ptr_eq(&geometry.world_surface_draw, &source.world_surface_draw)
        || !Arc::ptr_eq(&geometry.t6_props, &source.t6_props)
    {
        geometry.t6_styles = t6_styles(&source.world_surface_draw, &source.t6_props);
        let mut blended: Vec<usize> = (0..source.t6_props.len())
            .filter(|&i| t6_blended(source.t6_props[i][2]))
            .collect();
        blended.sort_by_key(|&i| t6_sort_key(source.t6_props[i][2]));
        geometry.t6_blended_props = blended;
    }
    geometry.world_surface_draw = Arc::clone(&source.world_surface_draw);
    geometry.world_surface_lightmap = Arc::clone(&source.world_surface_lightmap);
    geometry.t6_lighting = source.t6_lighting;
    geometry.t6_props = Arc::clone(&source.t6_props);
    geometry.t6_prop_shine = Arc::clone(&source.t6_prop_shine);
    geometry.world_surface_primary = Arc::clone(&source.world_surface_primary);
    geometry.world_surface_layers = Arc::clone(&source.world_surface_layers);
    geometry.world_surface_shine = Arc::clone(&source.world_surface_shine);
    geometry.t6_probe_slot = source.t6_probe_slot;
    geometry.t6_probes = Arc::clone(&source.t6_probes);
    if !Arc::ptr_eq(&geometry.world_layer_uvs, &source.world_layer_uvs) {
        geometry.world_layer_uvs = Arc::clone(&source.world_layer_uvs);
        geometry.world_layer_vertex = (!source.world_layer_uvs.is_empty()
            && source.world_layer_uvs.len() == source.world_vertices.len())
        .then(|| {
            device.create_buffer_with_data(&BufferInitDescriptor {
                label: Some("bo2zm_world_layer_uvs"),
                contents: bytemuck::cast_slice(source.world_layer_uvs.as_slice()),
                usage: BufferUsages::VERTEX,
            })
        });
    }
    geometry.t6_lights = Arc::clone(&source.t6_lights);
    geometry.t6_sky_slot = source.t6_sky_slot;
    if !Arc::ptr_eq(&geometry.t6_prop_instances, &source.t6_prop_instances) {
        geometry.t6_prop_instances = Arc::clone(&source.t6_prop_instances);
        geometry.t6_prop_instance_buffer = (!source.t6_prop_instances.is_empty()).then(|| {
            device.create_buffer_with_data(&BufferInitDescriptor {
                label: Some("bo2zm_prop_instances"),
                contents: bytemuck::cast_slice(source.t6_prop_instances.as_slice()),
                usage: BufferUsages::VERTEX,
            })
        });
    }
}

/// bo2zm: this frame's moving things into GPU buffers.
fn upload_dynamic(
    source: Option<Res<ExtractedT6Dynamic>>,
    mut dynamic: ResMut<DiagnosticDynamic>,
    device: Res<RenderDevice>,
) {
    let source = source.as_deref();
    dynamic.marks = upload_dynamic_draw(
        &device,
        "bo2zm_marks",
        source.and_then(|s| s.marks.as_ref()),
    );
    dynamic.viewmodel = upload_dynamic_draw(
        &device,
        "bo2zm_viewmodel",
        source.and_then(|s| s.viewmodel.as_ref()),
    );
    dynamic.missiles = upload_dynamic_draw(
        &device,
        "bo2zm_missiles",
        source.and_then(|s| s.missiles.as_ref()),
    );
    dynamic.script_static = match source.and_then(|s| s.script_static.as_ref()) {
        Some(s)
            if !s.draws.is_empty() && !s.instances.is_empty() && !s.geometry.0.is_empty() =>
        {
            let stale = dynamic
                .script_static_geometry
                .as_ref()
                .is_none_or(|(revision, _, _)| *revision != s.revision);
            if stale {
                let vb = device.create_buffer_with_data(&BufferInitDescriptor {
                    label: Some("bo2zm_script_static"),
                    contents: bytemuck::cast_slice(s.geometry.0.as_slice()),
                    usage: BufferUsages::VERTEX,
                });
                let ib = device.create_buffer_with_data(&BufferInitDescriptor {
                    label: Some("bo2zm_script_static"),
                    contents: bytemuck::cast_slice(s.geometry.1.as_slice()),
                    usage: BufferUsages::INDEX,
                });
                dynamic.script_static_geometry = Some((s.revision, vb, ib));
            }
            let inst = device.create_buffer_with_data(&BufferInitDescriptor {
                label: Some("bo2zm_script_static"),
                contents: bytemuck::cast_slice(s.instances.as_slice()),
                usage: BufferUsages::VERTEX,
            });
            dynamic
                .script_static_geometry
                .as_ref()
                .map(|(_, vb, ib)| (vb.clone(), ib.clone(), inst, s.draws.clone()))
        }
        _ => None,
    };
    dynamic.effects = upload_dynamic_draw(
        &device,
        "bo2zm_effects",
        source.and_then(|s| s.effects.as_ref()),
    );
    dynamic.viewmodel_effects = upload_dynamic_draw(
        &device,
        "bo2zm_viewmodel_effects",
        source.and_then(|s| s.viewmodel_effects.as_ref()),
    );
}

/// bo2zm: bind groups for the lightmap pages the world surfaces read, with
/// the T6 lighting constants; a page whose view has not uploaded yet is
/// tried again next frame.
fn prepare_world_lightmaps(
    pipeline: Option<Res<DiagnosticPipeline>>,
    geometry: Res<DiagnosticGeometry>,
    registry: Option<Res<RuntimeUploadedImageRegistry>>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    mut lightmaps: ResMut<DiagnosticWorldLightmaps>,
    dynamic_source: Option<Res<ExtractedT6Dynamic>>,
) {
    if !geometry_diagnostic_enabled() {
        return;
    }
    let (Some(pipeline), Some(registry), Some(lighting)) =
        (pipeline, registry, geometry.t6_lighting)
    else {
        return;
    };
    // bo2zm: the lights effects throw this frame.
    let dyn_lights = lightmaps
        .dyn_lights
        .get_or_insert_with(|| {
            device.create_buffer_with_data(&BufferInitDescriptor {
                label: Some("bo2zm_effect_lights"),
                contents: &[0u8; T6_DYN_BYTES as usize],
                usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            })
        })
        .clone();
    {
        let mut table = vec![0.0f32; T6_DYN_BYTES as usize / 4];
        let lights = dynamic_source.as_ref().map_or(&[][..], |d| d.dlights.as_slice());
        let n = lights.len().min(T6_DYN_LIGHTS);
        table[0] = n as f32;
        // The shaders' game time in seconds (BO2's gameTime): the objective
        // glow pulses by it.
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        table[1] = (START.get_or_init(std::time::Instant::now).elapsed().as_secs_f64() % 3600.0) as f32;
        for (i, light) in lights.iter().take(n).enumerate() {
            table[4 + i * 8..4 + i * 8 + 8].copy_from_slice(light);
        }
        queue.write_buffer(&dyn_lights, 0, bytemuck::cast_slice(&table));
    }
    if lightmaps.generation != registry.generation_id {
        lightmaps.bind_groups.clear();
        lightmaps.sky = None;
        lightmaps.lighting_group = None;
        lightmaps.generation = registry.generation_id;
    }
    let constants = lightmaps
        .constants
        .get_or_insert_with(|| {
            device.create_buffer_with_data(&BufferInitDescriptor {
                label: Some("bo2zm_world_lighting"),
                contents: bytemuck::cast_slice(&lighting),
                usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            })
        })
        .clone();
    if lightmaps.written != Some(lighting) {
        queue.write_buffer(&constants, 0, bytemuck::cast_slice(&lighting));
        lightmaps.written = Some(lighting);
    }
    let lights = lightmaps
        .lights
        .get_or_insert_with(|| {
            device.create_buffer_with_data(&BufferInitDescriptor {
                label: Some("bo2zm_primary_lights"),
                contents: &[0u8; T6_LIGHTS_BYTES as usize],
                usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            })
        })
        .clone();
    if lightmaps
        .lights_written
        .as_ref()
        .is_none_or(|have| !Arc::ptr_eq(have, &geometry.t6_lights))
    {
        let mut table = vec![[0.0f32; 16]; T6_MAX_LIGHTS];
        for (row, light) in table.iter_mut().zip(geometry.t6_lights.iter()) {
            *row = *light;
        }
        queue.write_buffer(&lights, 0, bytemuck::cast_slice(&table));
        lightmaps.lights_written = Some(Arc::clone(&geometry.t6_lights));
    }
    if lightmaps.lighting_group.is_none() {
        lightmaps.lighting_group = Some(device.create_bind_group(
            "bo2zm_world_lighting_only",
            &cache.get_bind_group_layout(&pipeline.lighting_layout),
            &BindGroupEntries::with_indices((
                (2, constants.as_entire_binding()),
                (3, lights.as_entire_binding()),
                (4, dyn_lights.as_entire_binding()),
            )),
        ));
    }
    let sampler = lightmaps
        .sampler
        .get_or_insert_with(|| {
            device.create_sampler(&SamplerDescriptor {
                label: Some("bo2zm_world_lightmap_sampler"),
                address_mode_u: AddressMode::ClampToEdge,
                address_mode_v: AddressMode::ClampToEdge,
                address_mode_w: AddressMode::ClampToEdge,
                mag_filter: FilterMode::Linear,
                min_filter: FilterMode::Linear,
                ..Default::default()
            })
        })
        .clone();
    if let Some(slot) = geometry.t6_sky_slot
        && lightmaps.sky.as_ref().is_none_or(|(have, _)| *have != slot)
        && let Some(Some(Ok(view))) = registry.material_images.get(slot as usize)
        && view.dimension == TextureViewDimension::Cube
    {
        let sky_sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("bo2zm_sky_sampler"),
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            mipmap_filter: MipmapFilterMode::Linear,
            ..Default::default()
        });
        let sky = device.create_bind_group(
            "bo2zm_sky",
            &cache.get_bind_group_layout(&pipeline.sky_layout),
            &BindGroupEntries::with_indices((
                (3, &view.view),
                (4, &sky_sampler),
                (5, constants.as_entire_binding()),
                (6, lights.as_entire_binding()),
            )),
        );
        lightmaps.sky = Some((slot, sky));
    }
    let layout = cache.get_bind_group_layout(&pipeline.lightmap_layout);
    for &page in geometry.world_surface_lightmap.iter() {
        if page == u8::MAX || lightmaps.bind_groups.contains_key(&page) {
            continue;
        }
        let Some(Some(views)) = registry.lightmaps.get(usize::from(page)) else {
            continue;
        };
        let Ok(view) = views.secondary.as_ref() else {
            continue;
        };
        let bind_group = device.create_bind_group(
            "bo2zm_world_lightmap",
            &layout,
            &BindGroupEntries::sequential((
                view,
                &sampler,
                constants.as_entire_binding(),
                lights.as_entire_binding(),
                dyn_lights.as_entire_binding(),
            )),
        );
        lightmaps.bind_groups.insert(page, bind_group);
    }
}

/// bo2zm: bind groups for the colour maps the world surfaces sample, from the
/// uploaded material images; a slot whose image has not uploaded yet is
/// tried again next frame.
fn prepare_world_textures(
    pipeline: Option<Res<DiagnosticPipeline>>,
    geometry: Res<DiagnosticGeometry>,
    registry: Option<Res<RuntimeUploadedImageRegistry>>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    mut textures: ResMut<DiagnosticWorldTextures>,
    dynamic: Option<Res<ExtractedT6Dynamic>>,
) {
    if !geometry_diagnostic_enabled() {
        return;
    }
    let (Some(pipeline), Some(registry)) = (pipeline, registry) else {
        return;
    };
    if geometry.world_surface_color.iter().all(|&slot| slot == u32::MAX)
        && geometry.t6_props.is_empty()
    {
        return;
    }
    if textures.generation != registry.generation_id {
        textures.bind_groups.clear();
        textures.layered.clear();
        textures.shine.clear();
        textures.probes = None;
        textures.generation = registry.generation_id;
    }
    let sampler = textures
        .sampler
        .get_or_insert_with(|| {
            device.create_sampler(&SamplerDescriptor {
                label: Some("bo2zm_world_colour_sampler"),
                address_mode_u: AddressMode::Repeat,
                address_mode_v: AddressMode::Repeat,
                address_mode_w: AddressMode::Repeat,
                mag_filter: FilterMode::Linear,
                min_filter: FilterMode::Linear,
                mipmap_filter: MipmapFilterMode::Linear,
                // bo2zm: 16x, 4x at the fast quality setting.
                anisotropy_clamp: if asset_core::t6_fast() { 4 } else { 16 },
                ..Default::default()
            })
        })
        .clone();
    let layout = cache.get_bind_group_layout(&pipeline.texture_layout);
    let prop_slots = geometry.t6_props.iter().map(|draw| draw[1]);
    // bo2zm: every T6 material's colour map (guns, arms, effects).
    let dynamic_slots: Vec<u32> = dynamic
        .as_ref()
        .map(|d| d.materials.iter().map(|m| m.0).collect())
        .unwrap_or_default();
    for slot in geometry
        .world_surface_color
        .iter()
        .copied()
        .chain(prop_slots)
        .chain(dynamic_slots)
    {
        if slot == u32::MAX || textures.bind_groups.contains_key(&slot) {
            continue;
        }
        let Some(Some(Ok(view))) = registry.material_images.get(slot as usize) else {
            continue;
        };
        if view.dimension != TextureViewDimension::D2 {
            continue;
        }
        let bind_group = device.create_bind_group(
            "bo2zm_world_colour_map",
            &layout,
            &BindGroupEntries::sequential((&view.view, &sampler)),
        );
        textures.bind_groups.insert(slot, bind_group);
    }
    // Layered surfaces: colour map plus layer 1 and 2 maps.
    let layered_layout = cache.get_bind_group_layout(&pipeline.layered_texture_layout);
    let view_of = |slot: u32| match registry.material_images.get(slot as usize) {
        Some(Some(Ok(view))) if view.dimension == TextureViewDimension::D2 => Some(view.view.clone()),
        _ => None,
    };
    for (i, &slot) in geometry.world_surface_color.iter().enumerate() {
        let Some(&[l1, l2]) = geometry.world_surface_layers.get(i) else {
            continue;
        };
        if slot == u32::MAX || (l1 == u32::MAX && l2 == u32::MAX) {
            continue;
        }
        let key = (slot, l1, l2);
        if textures.layered.contains_key(&key) {
            continue;
        }
        let Some(base) = view_of(slot) else { continue };
        let layer1 = if l1 == u32::MAX { Some(base.clone()) } else { view_of(l1) };
        let layer2 = if l2 == u32::MAX { Some(base.clone()) } else { view_of(l2) };
        let (Some(layer1), Some(layer2)) = (layer1, layer2) else {
            continue;
        };
        let bind_group = device.create_bind_group(
            "bo2zm_world_layered_colour_maps",
            &layered_layout,
            &BindGroupEntries::with_indices((
                (0, &base),
                (1, &sampler),
                (6, &layer1),
                (7, &layer2),
            )),
        );
        textures.layered.insert(key, bind_group);
    }
    prepare_world_shine(
        &pipeline,
        &geometry,
        dynamic.as_deref(),
        &registry,
        &cache,
        &device,
        &queue,
        &mut textures,
        &sampler,
    );
}

/// bo2zm: the shine surfaces' group 1 (colour, normal, specular maps) and
/// the probes' group 3.
#[allow(clippy::too_many_arguments)]
fn prepare_world_shine(
    pipeline: &DiagnosticPipeline,
    geometry: &DiagnosticGeometry,
    dynamic: Option<&ExtractedT6Dynamic>,
    registry: &RuntimeUploadedImageRegistry,
    cache: &PipelineCache,
    device: &RenderDevice,
    queue: &RenderQueue,
    textures: &mut DiagnosticWorldTextures,
    sampler: &Sampler,
) {
    let no_shine = |s: &[u32; 2]| s[0] == u32::MAX && s[1] == u32::MAX;
    let dynamic_shine: Vec<(u32, u32, u32, u32)> = dynamic
        .into_iter()
        .flat_map(|d| [d.viewmodel.as_ref()].into_iter().flatten())
        .flat_map(|draw| draw.draws.iter())
        .filter(|d| !no_shine(&[d[5], d[6]]))
        .map(|d| (d[2], d[5], d[6], u32::MAX))
        .collect();
    if geometry.world_surface_shine.iter().all(|s| no_shine(&[s[0], s[1]]))
        && geometry
            .t6_prop_shine
            .iter()
            .all(|s| no_shine(&[s[0], s[1]]))
        && dynamic_shine.is_empty()
    {
        return;
    }
    // 1x1 stand-ins: a flat normal (128/255 reads as 0 after BO2's scale
    // and bias), a black specular map (no highlight, no reflection).
    let one_by_one = |label: &'static str, format: TextureFormat, texel: &[u8]| {
        use bevy::render::render_resource::{TextureDescriptor, TextureUsages, TextureViewDescriptor};
        use bevy::render::render_resource::{Extent3d, TextureDimension};
        let texture = device.create_texture_with_data(
            queue,
            &TextureDescriptor {
                label: Some(label),
                size: Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format,
                usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                view_formats: &[],
            },
            bevy::render::render_resource::TextureDataOrder::LayerMajor,
            texel,
        );
        texture.create_view(&TextureViewDescriptor::default())
    };
    if textures.flat_normal.is_none() {
        textures.flat_normal = Some(one_by_one("bo2zm_flat_normal", TextureFormat::Rgba8Unorm, &[128, 128, 255, 255]));
    }
    if textures.no_specular.is_none() {
        textures.no_specular = Some(one_by_one("bo2zm_no_specular", TextureFormat::Rgba8Unorm, &[0, 0, 0, 0]));
    }
    let view_of = |slot: u32| match registry.material_images.get(slot as usize) {
        Some(Some(Ok(view))) if view.dimension == TextureViewDimension::D2 => Some(view.view.clone()),
        _ => None,
    };
    let layout = cache.get_bind_group_layout(&pipeline.shine_texture_layout);
    let world_keys = geometry
        .world_surface_shine
        .iter()
        .zip(geometry.world_surface_color.iter())
        .enumerate()
        .map(|(i, (sh, &colour))| {
            // bo2mp: an emissive flow surface's remap and constants.
            let flow = geometry
                .world_surface_draw
                .get(i)
                .is_some_and(|&code| style_flow(world_style(code, true)));
            let aux = if flow {
                geometry
                    .world_surface_layers
                    .get(i)
                    .map_or(u32::MAX, |l| l[0])
            } else {
                u32::MAX
            };
            (colour, sh[0], sh[1], aux)
        });
    let prop_keys = geometry
        .t6_prop_shine
        .iter()
        .zip(geometry.t6_props.iter())
        .map(|(sh, draw)| {
            let flow = prop_style(draw[2]) & (STYLE_PROP_FLOW | STYLE_PROP_TILE) != 0
                || (prop_style(draw[2]) & STYLE_PROP_FLAG != 0 && draw[2] & T6_UNLIT == 0);
            (draw[1], sh[0], sh[1], if flow { sh[2] } else { u32::MAX })
        });
    let keys: Vec<(u32, u32, u32, u32)> =
        world_keys.chain(prop_keys).chain(dynamic_shine).collect();
    for key in keys {
        let (colour, n, s, aux) = key;
        if n == u32::MAX && s == u32::MAX {
            continue;
        }
        if colour == u32::MAX || textures.shine.contains_key(&key) {
            continue;
        }
        let Some(base) = view_of(colour) else { continue };
        let normal = if n == u32::MAX { textures.flat_normal.clone() } else { view_of(n) };
        let specular = if s == u32::MAX { textures.no_specular.clone() } else { view_of(s) };
        let aux = if aux == u32::MAX { textures.no_specular.clone() } else { view_of(aux) };
        let (Some(normal), Some(specular), Some(aux)) = (normal, specular, aux) else {
            continue;
        };
        let bind_group = device.create_bind_group(
            "bo2zm_world_shine_maps",
            &layout,
            &BindGroupEntries::with_indices((
                (0, &base),
                (1, sampler),
                (8, &normal),
                (9, &specular),
                (10, &aux),
            )),
        );
        textures.shine.insert(key, bind_group);
    }
    // The probes: their lighting table, then their group.
    let table = textures
        .probe_table
        .get_or_insert_with(|| {
            device.create_buffer(&bevy::render::render_resource::BufferDescriptor {
                label: Some("bo2zm_probe_table"),
                size: T6_PROBES_BYTES,
                usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        })
        .clone();
    if textures
        .probe_written
        .as_ref()
        .is_none_or(|have| !Arc::ptr_eq(have, &geometry.t6_probes))
    {
        let mut rows = vec![[0.0f32; 12]; T6_MAX_PROBES];
        for (row, probe) in rows.iter_mut().zip(geometry.t6_probes.iter()) {
            for (k, v) in probe.iter().enumerate() {
                row[k * 4..k * 4 + 4].copy_from_slice(v);
            }
        }
        queue.write_buffer(&table, 0, bytemuck::cast_slice(&rows));
        textures.probe_written = Some(Arc::clone(&geometry.t6_probes));
    }
    if let Some(slot) = geometry.t6_probe_slot
        && textures.probes.as_ref().is_none_or(|(have, _)| *have != slot)
        && let Some(Some(Ok(view))) = registry.material_images.get(slot as usize)
        && view.dimension == TextureViewDimension::CubeArray
    {
        let probe_sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("bo2zm_probe_sampler"),
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            mipmap_filter: MipmapFilterMode::Linear,
            ..Default::default()
        });
        let group = device.create_bind_group(
            "bo2zm_probes",
            &cache.get_bind_group_layout(&pipeline.probe_layout),
            &BindGroupEntries::with_indices((
                (0, &view.view),
                (1, &probe_sampler),
                (2, table.as_entire_binding()),
            )),
        );
        textures.probes = Some((slot, group));
    }
}

fn prepare_views(
    mut commands: Commands,
    geometry: Res<DiagnosticGeometry>,
    dynamic: Option<Res<ExtractedT6Dynamic>>,
    pipeline: Option<Res<DiagnosticPipeline>>,
    mut specialized: ResMut<SpecializedRenderPipelines<DiagnosticPipeline>>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    uniforms: Res<ViewUniforms>,
    views: Query<(Entity, &ExtractedView, &Msaa)>,
) {
    if !geometry_diagnostic_enabled() {
        return;
    }
    let (Some(pipeline), Some(binding)) = (pipeline, uniforms.uniforms.binding()) else {
        return;
    };
    for (entity, view, msaa) in &views {
        let world_pipeline = specialized.specialize(
            &cache,
            &pipeline,
            DiagnosticPipelineKey {
                target: view.target_format,
                samples: msaa.samples(),
                tess: DiagnosticTess::World,
            },
        );
        // bo2zm: the world's and props' styles, plus a prop style for every
        // T6 material (what guns, arms and effects draw with).
        let mut styles: std::collections::BTreeSet<DiagnosticTess> =
            geometry.t6_styles.iter().copied().collect();
        if let Some(dynamic) = dynamic.as_ref() {
            for &(_, code) in dynamic.materials.iter() {
                if code != T6_NONE && code & T6_BLEND_MASK >= T6_SHADOW_ONLY {
                    continue;
                }
                styles.insert(DiagnosticTess::PropTextured(prop_style(code)));
                if prop_shine_code(code) {
                    styles.insert(DiagnosticTess::PropTextured(prop_style(code) | STYLE_SHINE));
                }
                if soft_code(code) {
                    styles.insert(DiagnosticTess::PropTextured(prop_style(code) | STYLE_SOFT));
                }
            }
        }
        let t6_pipelines: HashMap<DiagnosticTess, CachedRenderPipelineId> = styles
            .iter()
            .map(|&tess| {
                let id = specialized.specialize(
                    &cache,
                    &pipeline,
                    DiagnosticPipelineKey {
                        target: view.target_format,
                        samples: msaa.samples(),
                        tess,
                    },
                );
                (tess, id)
            })
            .collect();
        let smodel_pipeline = specialized.specialize(
            &cache,
            &pipeline,
            DiagnosticPipelineKey {
                target: view.target_format,
                samples: msaa.samples(),
                tess: DiagnosticTess::Smodel,
            },
        );
        let xmodel_pipeline = specialized.specialize(
            &cache,
            &pipeline,
            DiagnosticPipelineKey {
                target: view.target_format,
                samples: msaa.samples(),
                tess: DiagnosticTess::XModel,
            },
        );
        let sky_pipeline = specialized.specialize(
            &cache,
            &pipeline,
            DiagnosticPipelineKey {
                target: view.target_format,
                samples: msaa.samples(),
                tess: DiagnosticTess::Sky,
            },
        );
        let bind_group = device.create_bind_group(
            "iw4_geometry_diagnostic_view",
            &cache.get_bind_group_layout(&pipeline.view_layout),
            &BindGroupEntries::single(binding.clone()),
        );
        commands.entity(entity).insert(DiagnosticViewBindGroup {
            bind_group,
            world_pipeline,
            t6_pipelines,
            sky_pipeline,
            smodel_pipeline,
            xmodel_pipeline,
        });
    }
}

fn draw_geometry_diagnostic(
    view: ViewQuery<(
        &ViewTarget,
        &SceneDepthTexture,
        &ExtractedView,
        &ViewUniformOffset,
        &DiagnosticViewBindGroup,
    )>,
    dynamic: Res<DiagnosticDynamic>,
    colour_frame: Res<super::PublishedRenderFrame>,
    geometry: Res<DiagnosticGeometry>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    mut context: RenderContext,
    mut last_census: Local<Option<String>>,
    mut perf: Local<DiagnosticPerf>,
    submitted: Option<Res<super::colour_submit::ExactColourSubmitCensus>>,
    extracted: Option<Res<ExtractedDiagnosticGeometry>>,
    slot: Option<Res<SharedRenderStagesSlot>>,
    mut scratch: Local<DiagnosticDrawScratch>,
    world_textures: Res<DiagnosticWorldTextures>,
    world_lightmaps: Res<DiagnosticWorldLightmaps>,
    diagnostic_pipeline: Res<DiagnosticPipeline>,
) {
    let products = &colour_frame.frame_products;
    if extracted
        .as_ref()
        .is_some_and(|extracted| extracted.overlay_gpu_wait)
    {
        return;
    }
    if !geometry_diagnostic_enabled() {
        return;
    }
    let mut stamp = DiagnosticStamp {
        started: Instant::now(),
        slot: slot.as_ref().map(|s| Arc::clone(&s.0)),
        pass: 0,
        draw_n: 0,
        world_n: 0,
        smodel_n: 0,
        xmodel_n: 0,
    };
    scratch.submitted.clear();
    scratch.exec_ready.clear();
    scratch.draws.clear();

    if let Some(census) = submitted.as_ref() {
        scratch
            .submitted
            .extend(census.submitted_keys.iter().copied());
        scratch
            .exec_ready
            .extend(census.world_exec_ready_keys.iter().copied());
    }
    let colour = products.0.product(FrameProductKind::Colour);
    let emissive = products.0.product(FrameProductKind::Emissive);
    let DiagnosticDrawScratch {
        submitted,
        exec_ready,
        draws,
        smodel_instances,
    } = &mut *scratch;
    draws.extend(
        colour
            .ordered_draws
            .iter()
            .enumerate()
            .filter_map(|(_, item)| {
                let ready = exec_ready.contains(&item.key);
                diagnostic_would_draw(item, ready, submitted).then_some((*item, ready))
            })
            .chain(
                emissive
                    .ordered_draws
                    .iter()
                    .enumerate()
                    .filter_map(|(_, item)| {
                        let ready = exec_ready.contains(&item.key);
                        diagnostic_would_draw(item, ready, submitted).then_some((*item, ready))
                    }),
            ),
    );
    for &surf in &geometry.g0_world_surfs {
        let item = RetainedDrawItem {
            material_id: None,
            material_rank: 0,
            key: 0,
            kind: RetainedDrawKind::world(surf),
            surface_samplers: Default::default(),
            camera_region: None,
        };
        if diagnostic_would_draw(&item, false, submitted) {
            draws.push((item, false));
        }
    }
    if draws.is_empty() && geometry.t6_props.is_empty() && geometry.t6_sky_slot.is_none() {
        return;
    }
    let (target, depth, extracted_view, view_offset, view_bind) = view.into_inner();
    let world_pipeline = cache.get_render_pipeline(view_bind.world_pipeline);
    let t6_pipeline = |tess: DiagnosticTess| {
        view_bind
            .t6_pipelines
            .get(&tess)
            .and_then(|&id| cache.get_render_pipeline(id))
    };
    let sky_pipeline = cache.get_render_pipeline(view_bind.sky_pipeline);
    let smodel_pipeline = cache.get_render_pipeline(view_bind.smodel_pipeline);
    let xmodel_pipeline = cache.get_render_pipeline(view_bind.xmodel_pipeline);
    if world_pipeline.is_none() && smodel_pipeline.is_none() && xmodel_pipeline.is_none() {
        return;
    }
    smodel_instances.clear();
    smodel_instances.extend(draws.iter().map(|(item, _)| match item.kind {
        RetainedDrawKind::Smodel {
            world_from_local, ..
        }
        | RetainedDrawKind::XModel {
            world_from_local, ..
        } => DiagnosticSmodelInstance {
            world_from_local: world_from_local.to_cols_array_2d(),
        },
        _ => DiagnosticSmodelInstance {
            world_from_local: Mat4::IDENTITY.to_cols_array_2d(),
        },
    }));
    let smodel_instance_buffer = (!smodel_instances.is_empty()).then(|| {
        device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("iw4_geometry_diagnostic_smodel_instances"),
            contents: bytemuck::cast_slice(smodel_instances.as_slice()),
            usage: BufferUsages::VERTEX,
        })
    });
    let attachments = [Some(target.get_color_attachment())];
    stamp.pass = 1;
    // bo2zm: the pass's GPU time (`render/bo2zm_world/elapsed_gpu`), which a
    // driver's frame cap on a background window does not hide.
    use bevy::render::diagnostic::RecordDiagnostics;
    let recorder = context.diagnostic_recorder();
    let recorder = recorder.as_deref();
    let gpu_span = recorder.time_span(context.command_encoder(), "bo2zm_world");
    let mut pass =
        context.begin_tracked_render_pass(bevy::render::render_resource::RenderPassDescriptor {
            label: Some("iw4_geometry_diagnostic_pass"),
            color_attachments: &attachments,
            depth_stencil_attachment: Some(depth.get_attachment(StoreOp::Store)),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    pass.set_bind_group(0, &view_bind.bind_group, &[view_offset.offset]);
    let mut bound_tess = None;
    let mut bound_texture = None;
    let mut bound_lightmap = None;
    let mut bound_depth = None;
    let surface_draw = |item: &RetainedDrawItem| match item.kind {
        RetainedDrawKind::World { surf, .. } => geometry
            .world_surface_draw
            .get(usize::from(surf))
            .copied()
            .unwrap_or(T6_NONE),
        _ => T6_NONE,
    };
    // bo2zm: opaque surfaces first; blended ones after every opaque world
    // surface and prop, in their materials' sort order (as the game draws
    // them: decals, then blend layers, then glass).
    let opaque_order: Vec<usize> = (0..draws.len())
        .filter(|&i| !t6_blended(surface_draw(&draws[i].0)))
        .collect();
    let mut blended_order: Vec<usize> = (0..draws.len())
        .filter(|&i| t6_blended(surface_draw(&draws[i].0)))
        .collect();
    blended_order.sort_by_key(|&i| t6_sort_key(surface_draw(&draws[i].0)));
    let viewport = extracted_view.viewport;
    let vp_x = viewport.x as f32;
    let vp_y = viewport.y as f32;
    let vp_w = viewport.z as f32;
    let vp_h = viewport.w as f32;
    let mut world_draws = 0u32;
    let mut smodel_draws = 0u32;
    let mut xmodel_draws = 0u32;
    let fx_draws = 0u32;
    let mut world_skipped = 0u32;
    let mut smodel_skipped = 0u32;
    let mut xmodel_skipped = 0u32;
    let mut fx_skipped = 0u32;
    let mut prop_draws = 0u32;
    let mut props_culled = 0u32;
    // bo2zm: world surfaces drawn untextured (diagnostic colours), a few ids.
    let mut untextured: Vec<u16> = Vec::new();
    let mut untextured_n = 0u32;
    for blended_phase in [false, true] {
        let order = if blended_phase {
            &blended_order
        } else {
            &opaque_order
        };
        for &index in order {
            let (item, ready) = &draws[index];
            if *ready && matches!(item.kind, RetainedDrawKind::World { .. }) {
                continue;
            }
            if submitted.contains(&item.key) {
                continue;
            }
            if !geometry_diagnostic_covers(&item.kind, item.key) {
                xmodel_skipped = xmodel_skipped.saturating_add(1);
                continue;
            }
            let (tess, start, count, instances) = match item.kind {
                RetainedDrawKind::World { surf, .. } => {
                    let (Some(vertex), Some(index), Some(pipeline), Some(&(start, count))) = (
                        geometry.world_vertex.as_ref(),
                        geometry.world_index.as_ref(),
                        world_pipeline,
                        geometry.world_surface_ranges.get(usize::from(surf)),
                    ) else {
                        world_skipped = world_skipped.saturating_add(1);
                        continue;
                    };
                    let colour_slot = geometry
                        .world_surface_color
                        .get(usize::from(surf))
                        .copied()
                        .unwrap_or(u32::MAX);
                    let code = geometry
                        .world_surface_draw
                        .get(usize::from(surf))
                        .copied()
                        .unwrap_or(T6_NONE);
                    if code != T6_NONE && code & T6_BLEND_MASK >= T6_SHADOW_ONLY {
                        world_skipped = world_skipped.saturating_add(1);
                        continue;
                    }
                    let page = geometry
                        .world_surface_lightmap
                        .get(usize::from(surf))
                        .copied()
                        .unwrap_or(u8::MAX);
                    let lightmap = (code != T6_NONE && code & T6_UNLIT == 0)
                        .then(|| world_lightmaps.bind_groups.get(&page))
                        .flatten()
                        .map(|g| (g, u16::from(page)))
                        .or_else(|| {
                            // bo2mp: emissive flow (lava) has no lightmap (its
                            // shader reads none), nor has a distant sea; they
                            // draw in the lightmapped pipeline over the first
                            // page.
                            (code != T6_NONE
                                && code & (T6_FLOW | T6_WATER) != 0
                                && code & T6_UNLIT == 0)
                                .then(|| {
                                    world_lightmaps
                                        .bind_groups
                                        .iter()
                                        .min_by_key(|(k, _)| **k)
                                        .map(|(k, g)| (g, 0x100 | u16::from(*k)))
                                })
                                .flatten()
                        });
                    // Group 2: the page's lightmap group, else lighting only.
                    let group2 = match lightmap {
                        Some(group) => Some(group),
                        None => world_lightmaps.lighting_group.as_ref().map(|g| (g, u16::MAX)),
                    };
                    let mut style = world_style(code, lightmap.is_some());
                    // Layered: its colour, layer 1 and 2 maps, else the
                    // base layer alone.
                    let layers = geometry
                        .world_surface_layers
                        .get(usize::from(surf))
                        .copied()
                        .unwrap_or([u32::MAX; 2]);
                    let layered_group = (style_layers(style) && geometry.world_layer_vertex.is_some())
                        .then(|| world_textures.layered.get(&(colour_slot, layers[0], layers[1])))
                        .flatten();
                    if style_layers(style) && layered_group.is_none() {
                        style = strip_layers(style);
                    }
                    // Shine: an opaque lit lightmapped surface with a normal or
                    // specular map, when the probes are ready.
                    let shine = geometry
                        .world_surface_shine
                        .get(usize::from(surf))
                        .copied()
                        .unwrap_or([u32::MAX; 3]);
                    let shine_group = (style & STYLE_LIGHTMAPPED != 0
                        && !style_layers(style)
                        && style & 3 <= 1
                        && lightmap.is_some()
                        && world_textures.probes.is_some())
                    .then(|| {
                        // bo2mp: emissive flow (lava) binds its colour remap
                        // and constants (layer 1's slot) beside its maps.
                        let aux = if style_flow(style) { layers[0] } else { u32::MAX };
                        world_textures.shine.get(&(colour_slot, shine[0], shine[1], aux))
                    })
                    .flatten();
                    if shine_group.is_some() {
                        style |= STYLE_SHINE;
                    } else if style_water(style) || style_flow(style) {
                        // bo2mp: water's colour map holds its constants; it
                        // waits for its normal maps and the probes.
                        world_skipped = world_skipped.saturating_add(1);
                        continue;
                    }
                    let key = DiagnosticTess::WorldTextured(style);
                    let group1 = match (shine_group, layered_group) {
                        (Some(group), _) => Some(group),
                        (None, Some(group)) => Some(group),
                        (None, None) => world_textures.bind_groups.get(&colour_slot),
                    };
                    let textured = group1.zip(t6_pipeline(key)).filter(|_| group2.is_some());
                    let tess = if let Some((bind_group, textured_pipeline)) = textured {
                        if bound_tess != Some(key) {
                            pass.set_render_pipeline(textured_pipeline);
                            pass.set_vertex_buffer(0, vertex.slice(..));
                            if style_layers(style)
                                && let Some(layer_vertex) = geometry.world_layer_vertex.as_ref()
                            {
                                pass.set_vertex_buffer(1, layer_vertex.slice(..));
                            }
                            pass.set_index_buffer(index.slice(..), IndexFormat::Uint32);
                            if style & STYLE_SHINE != 0
                                && let Some((_, probes)) = world_textures.probes.as_ref()
                            {
                                pass.set_bind_group(3, probes, &[]);
                            }
                            bound_tess = Some(key);
                            bound_texture = None;
                            bound_lightmap = None;
                        }
                        let texture_key = (
                            colour_slot,
                            layered_group.map(|_| layers),
                            shine_group.map(|_| [shine[0], shine[1], layers[0]]),
                        );
                        if bound_texture != Some(texture_key) {
                            pass.set_bind_group(1, bind_group, &[]);
                            bound_texture = Some(texture_key);
                        }
                        if let Some((group, id)) = group2
                            && bound_lightmap != Some(id)
                        {
                            pass.set_bind_group(2, group, &[]);
                            bound_lightmap = Some(id);
                        }
                        key
                    } else {
                        untextured_n += 1;
                        if untextured.len() < 12 {
                            untextured.push(surf);
                        }
                        if bound_tess != Some(DiagnosticTess::World) {
                            pass.set_render_pipeline(pipeline);
                            pass.set_vertex_buffer(0, vertex.slice(..));
                            pass.set_index_buffer(index.slice(..), IndexFormat::Uint32);
                            bound_tess = Some(DiagnosticTess::World);
                        }
                        DiagnosticTess::World
                    };
                    world_draws = world_draws.saturating_add(1);
                    // The surface's primary light rides in the instance index;
                    // a shine surface's reflection probe above it (bits 6..).
                    let light = u32::from(
                        geometry
                            .world_surface_primary
                            .get(usize::from(surf))
                            .copied()
                            .unwrap_or(0),
                    );
                    let instance = if tess == DiagnosticTess::WorldTextured(style) && style & STYLE_SHINE != 0 {
                        (light & 63) | (shine[2].min(T6_MAX_PROBES as u32 - 1) << 6)
                    } else {
                        light
                    };
                    (tess, start, count, instance..instance + 1)
                }
                RetainedDrawKind::Smodel { surface, .. } => {
                    let (
                        Some(vertex),
                        Some(index_buffer),
                        Some(instance_buffer),
                        Some(pipeline),
                        Some(&(start, count)),
                    ) = (
                        geometry.smodel_vertex.as_ref(),
                        geometry.smodel_index.as_ref(),
                        smodel_instance_buffer.as_ref(),
                        smodel_pipeline,
                        geometry.smodel_surface_ranges.get(surface as usize),
                    )
                    else {
                        smodel_skipped = smodel_skipped.saturating_add(1);
                        continue;
                    };
                    if bound_tess != Some(DiagnosticTess::Smodel) {
                        pass.set_render_pipeline(pipeline);
                        pass.set_vertex_buffer(0, vertex.slice(..));
                        pass.set_vertex_buffer(1, instance_buffer.slice(..));
                        pass.set_index_buffer(index_buffer.slice(..), IndexFormat::Uint32);
                        bound_tess = Some(DiagnosticTess::Smodel);
                    }
                    smodel_draws = smodel_draws.saturating_add(1);
                    let instance = u32::try_from(index).unwrap_or(u32::MAX);
                    (
                        DiagnosticTess::Smodel,
                        start,
                        count,
                        instance..instance.saturating_add(1),
                    )
                }
                RetainedDrawKind::XModel { surface, .. } => {
                    let (
                        Some(vertex),
                        Some(index_buffer),
                        Some(instance_buffer),
                        Some(pipeline),
                        Some(&(start, count)),
                    ) = (
                        geometry.xmodel_vertex.as_ref(),
                        geometry.xmodel_index.as_ref(),
                        smodel_instance_buffer.as_ref(),
                        xmodel_pipeline,
                        geometry.xmodel_surface_ranges.get(surface as usize),
                    )
                    else {
                        xmodel_skipped = xmodel_skipped.saturating_add(1);
                        continue;
                    };
                    if bound_tess != Some(DiagnosticTess::XModel) {
                        pass.set_render_pipeline(pipeline);
                        pass.set_vertex_buffer(0, vertex.slice(..));
                        pass.set_vertex_buffer(1, instance_buffer.slice(..));
                        pass.set_index_buffer(index_buffer.slice(..), IndexFormat::Uint32);
                        bound_tess = Some(DiagnosticTess::XModel);
                    }
                    xmodel_draws = xmodel_draws.saturating_add(1);
                    let instance = u32::try_from(index).unwrap_or(u32::MAX);
                    (
                        DiagnosticTess::XModel,
                        start,
                        count,
                        instance..instance.saturating_add(1),
                    )
                }
                RetainedDrawKind::CodeMesh { .. } | RetainedDrawKind::ParticleCloud { .. } => {
                    fx_skipped = fx_skipped.saturating_add(1);
                    continue;
                }
                RetainedDrawKind::MarkMesh { .. } | RetainedDrawKind::Glass { .. } => {
                    fx_skipped = fx_skipped.saturating_add(1);
                    continue;
                }
            };
            if count == 0 {
                match tess {
                    DiagnosticTess::World | DiagnosticTess::WorldTextured(_) => {
                        world_draws = world_draws.saturating_sub(1);
                        world_skipped = world_skipped.saturating_add(1);
                    }
                    DiagnosticTess::Smodel
                    | DiagnosticTess::PropTextured(_)
                    | DiagnosticTess::Sky => {
                        smodel_draws = smodel_draws.saturating_sub(1);
                        smodel_skipped = smodel_skipped.saturating_add(1);
                    }
                    DiagnosticTess::XModel => {
                        xmodel_draws = xmodel_draws.saturating_sub(1);
                        xmodel_skipped = xmodel_skipped.saturating_add(1);
                    }
                }
                continue;
            }
            if vp_w > 0.0 && vp_h > 0.0 {
                let (depth_min, depth_max) =
                    reverse_z_viewport_depth(depth_range_type_for_draw(&item.kind, item.key));
                if bound_depth != Some((depth_min, depth_max)) {
                    pass.set_viewport(vp_x, vp_y, vp_w, vp_h, depth_min, depth_max);
                    bound_depth = Some((depth_min, depth_max));
                }
            }
            pass.draw_indexed(start..start.saturating_add(count), 0, instances);
        }
        // bo2zm: Black Ops II props of this phase (blended ones by sort key).
        if let (Some(vertex), Some(index_buffer), Some(instances), Some(lighting_group)) = (
            geometry.smodel_vertex.as_ref(),
            geometry.smodel_index.as_ref(),
            geometry.t6_prop_instance_buffer.as_ref(),
            world_lightmaps.lighting_group.as_ref(),
        ) {
            let mut bound_prop: Option<DiagnosticTess> = None;
            pass.set_bind_group(2, lighting_group, &[]);
            let mut bound_slot = None;
            if vp_w > 0.0 && vp_h > 0.0 {
                let (depth_min, depth_max) = reverse_z_viewport_depth(0);
                pass.set_viewport(vp_x, vp_y, vp_w, vp_h, depth_min, depth_max);
            }
            pass.set_vertex_buffer(0, vertex.slice(..));
            pass.set_vertex_buffer(1, instances.slice(..));
            pass.set_index_buffer(index_buffer.slice(..), IndexFormat::Uint32);
            let opaque_props = (0..geometry.t6_props.len())
                .filter(|&i| !t6_blended(geometry.t6_props[i][2]));
            // The game's own per-placement draw distance (the fast quality
            // setting hides props at 60% of it).
            let eye = extracted_view.world_from_view.translation();
            let cull_scale = if asset_core::t6_fast() { 0.6 } else { 1.0 };
            let instances_cpu = &geometry.t6_prop_instances;
            let props: Box<dyn Iterator<Item = usize>> = if blended_phase {
                Box::new(geometry.t6_blended_props.iter().copied())
            } else {
                Box::new(opaque_props)
            };
            for i in props {
                let [surface, slot, code, instance] = geometry.t6_props[i];
                if code != T6_NONE && code & T6_BLEND_MASK >= T6_SHADOW_ONLY {
                    continue;
                }
                if let Some(row) = instances_cpu.get(instance as usize)
                    && row[22] > 0.0
                {
                    let d = Vec3::new(row[12], row[13], row[14]) - eye;
                    let cull = row[22] * cull_scale;
                    if d.length_squared() > cull * cull {
                        props_culled += 1;
                        continue;
                    }
                }
                let Some(&(start, count)) = geometry.smodel_surface_ranges.get(surface as usize)
                else {
                    continue;
                };
                // Shine: a lit opaque prop with a normal or specular map.
                let shine = geometry.t6_prop_shine.get(i).copied().unwrap_or([u32::MAX; 3]);
                let flow = prop_style(code) & (STYLE_PROP_FLOW | STYLE_PROP_TILE) != 0
                    || (prop_style(code) & STYLE_PROP_FLAG != 0 && code & T6_UNLIT == 0);
                let shine_group = (prop_shine_code(code)
                    && (shine[0] != u32::MAX || shine[1] != u32::MAX))
                    .then(|| world_textures.probes.as_ref())
                    .flatten()
                    .and_then(|_| {
                        let aux = if flow { shine[2] } else { u32::MAX };
                        world_textures.shine.get(&(slot, shine[0], shine[1], aux))
                    });
                let style = prop_style(code) | if shine_group.is_some() { STYLE_SHINE } else { 0 };
                let key = DiagnosticTess::PropTextured(style);
                let (Some(pipeline), Some(bind_group)) = (
                    t6_pipeline(key),
                    shine_group.or_else(|| world_textures.bind_groups.get(&slot)),
                ) else {
                    continue;
                };
                if count == 0 {
                    continue;
                }
                if bound_prop != Some(key) {
                    pass.set_render_pipeline(pipeline);
                    if shine_group.is_some()
                        && let Some((_, probes)) = world_textures.probes.as_ref()
                    {
                        pass.set_bind_group(3, probes, &[]);
                    }
                    bound_prop = Some(key);
                    bound_slot = None;
                }
                let slot_key = (slot, shine_group.map(|_| shine));
                if bound_slot != Some(slot_key) {
                    pass.set_bind_group(1, bind_group, &[]);
                    bound_slot = Some(slot_key);
                }
                pass.draw_indexed(start..start.saturating_add(count), 0, instance..instance + 1);
                prop_draws = prop_draws.saturating_add(1);
            }
            // The world draws rebind their buffers, pipeline and viewport.
            bound_tess = None;
            bound_texture = None;
            bound_lightmap = None;
            bound_depth = None;
        }
    }
    // bo2zm: the sky last, behind everything already drawn.
    if let (Some(pipeline), Some((_, sky))) = (sky_pipeline, world_lightmaps.sky.as_ref()) {
        if vp_w > 0.0 && vp_h > 0.0 {
            let (depth_min, depth_max) = reverse_z_viewport_depth(0);
            pass.set_viewport(vp_x, vp_y, vp_w, vp_h, depth_min, depth_max);
        }
        pass.set_render_pipeline(pipeline);
        pass.set_bind_group(1, sky, &[]);
        pass.draw(0..3, 0..1);
    }
    // bo2zm: effects (over the sky, which only fills what nothing else
    // covered), then the first-person gun and arms in the viewmodel's depth
    // range; opaque surfaces first, then blended ones by sort key. The
    // world's blended effects draw in a pass of their own over a read-only
    // depth they also read: BO2's zfeather shaders fade a sprite where it
    // nears what is behind it.
    let soft_group = device.create_bind_group(
        "bo2zm_soft_depth",
        &cache.get_bind_group_layout(&diagnostic_pipeline.soft_layout),
        &BindGroupEntries::single(depth.view()),
    );
    let mut viewmodel_draws = 0u32;
    viewmodel_draws += draw_dynamic_list(
        &mut pass,
        dynamic.marks.as_ref(),
        world_lightmaps.lighting_group.as_ref(),
        0,
        [vp_x, vp_y, vp_w, vp_h],
        &|_| true,
        None,
        &cache,
        &view_bind.t6_pipelines,
        &world_textures,
    );
    viewmodel_draws += draw_dynamic_list(
        &mut pass,
        dynamic.missiles.as_ref(),
        world_lightmaps.lighting_group.as_ref(),
        0,
        [vp_x, vp_y, vp_w, vp_h],
        &|_| true,
        None,
        &cache,
        &view_bind.t6_pipelines,
        &world_textures,
    );
    viewmodel_draws += draw_dynamic_list(
        &mut pass,
        dynamic.script_static.as_ref(),
        world_lightmaps.lighting_group.as_ref(),
        0,
        [vp_x, vp_y, vp_w, vp_h],
        &|_| true,
        None,
        &cache,
        &view_bind.t6_pipelines,
        &world_textures,
    );
    viewmodel_draws += draw_dynamic_list(
        &mut pass,
        dynamic.effects.as_ref(),
        world_lightmaps.lighting_group.as_ref(),
        0,
        [vp_x, vp_y, vp_w, vp_h],
        &|code| !t6_blended(code),
        None,
        &cache,
        &view_bind.t6_pipelines,
        &world_textures,
    );
    drop(pass);
    let attachments = [Some(target.get_color_attachment())];
    let mut pass =
        context.begin_tracked_render_pass(bevy::render::render_resource::RenderPassDescriptor {
            label: Some("bo2zm_effects_pass"),
            color_attachments: &attachments,
            depth_stencil_attachment: Some(depth.read_only_attachment()),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    pass.set_bind_group(0, &view_bind.bind_group, &[view_offset.offset]);
    viewmodel_draws += draw_dynamic_list(
        &mut pass,
        dynamic.effects.as_ref(),
        world_lightmaps.lighting_group.as_ref(),
        0,
        [vp_x, vp_y, vp_w, vp_h],
        &t6_blended,
        // (`IW4L_T6_NO_SOFT`: hard edges, a test aid for comparing.)
        std::env::var_os("IW4L_T6_NO_SOFT").is_none().then_some(&soft_group),
        &cache,
        &view_bind.t6_pipelines,
        &world_textures,
    );
    drop(pass);
    let attachments = [Some(target.get_color_attachment())];
    let mut pass =
        context.begin_tracked_render_pass(bevy::render::render_resource::RenderPassDescriptor {
            label: Some("bo2zm_viewmodel_pass"),
            color_attachments: &attachments,
            depth_stencil_attachment: Some(depth.get_attachment(StoreOp::Store)),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    pass.set_bind_group(0, &view_bind.bind_group, &[view_offset.offset]);
    viewmodel_draws += draw_dynamic_list(
        &mut pass,
        dynamic.viewmodel.as_ref(),
        world_lightmaps.lighting_group.as_ref(),
        GFX_DEPTH_RANGE_VIEWMODEL,
        [vp_x, vp_y, vp_w, vp_h],
        &|_| true,
        None,
        &cache,
        &view_bind.t6_pipelines,
        &world_textures,
    );
    viewmodel_draws += draw_dynamic_list(
        &mut pass,
        dynamic.viewmodel_effects.as_ref(),
        world_lightmaps.lighting_group.as_ref(),
        GFX_DEPTH_RANGE_VIEWMODEL,
        [vp_x, vp_y, vp_w, vp_h],
        &|_| true,
        None,
        &cache,
        &view_bind.t6_pipelines,
        &world_textures,
    );
    let _ = viewmodel_draws;
    drop(pass);
    gpu_span.end(context.command_encoder());
    smodel_draws = smodel_draws.saturating_add(prop_draws);
    stamp.draw_n = world_draws
        .saturating_add(smodel_draws)
        .saturating_add(xmodel_draws)
        .saturating_add(fx_draws);
    stamp.world_n = world_draws;
    stamp.smodel_n = smodel_draws;
    stamp.xmodel_n = xmodel_draws;
    // bo2zm: frames per second, frame time and prop counts every 5 s.
    let now = Instant::now();
    if let Some(prev) = perf.last_frame {
        let ms = now.duration_since(prev).as_secs_f32() * 1000.0;
        perf.worst_ms = perf.worst_ms.max(ms);
        // IW4L_PIPELINE_LOG=1: a frame that took over half a second, as it
        // happens (to line up with what was built or loaded then).
        static LOG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if ms >= 500.0 && *LOG.get_or_init(|| std::env::var_os("IW4L_PIPELINE_LOG").is_some()) {
            diag::info!(World, "slow frame: {ms:.0} ms");
        }
    }
    perf.last_frame = Some(now);
    perf.frames += 1;
    perf.props_drawn += u64::from(prop_draws);
    perf.props_culled += u64::from(props_culled);
    let start = *perf.window_start.get_or_insert(now);
    let span = now.duration_since(start).as_secs_f32();
    // (`IW4L_PERF_WINDOW=secs`: a shorter window, for tests that must end
    // before another window takes the foreground.)
    static WINDOW: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    let window = *WINDOW.get_or_init(|| {
        std::env::var("IW4L_PERF_WINDOW")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|v: &f32| *v >= 0.5)
            .unwrap_or(5.0)
    });
    if span >= window && perf.frames > 1 {
        let n = perf.frames as f32;
        diag::info!(
            World,
            "bo2zm perf: {:.1} fps, {:.2} ms average, {:.2} ms worst; props {:.0} drawn, {:.0} hidden by distance per frame; quality {}",
            (n - 1.0) / span,
            span * 1000.0 / (n - 1.0),
            perf.worst_ms,
            perf.props_drawn as f32 / n,
            perf.props_culled as f32 / n,
            if asset_core::t6_fast() { "fast" } else { "full" }
        );
        *perf = DiagnosticPerf {
            last_census_at: perf.last_census_at,
            last_frame: perf.last_frame,
            ..Default::default()
        };
    }
    let g0_queued = geometry.g0_world_surfs.len();
    let line = format!(
        "drawsurf geometry diagnostic: world_draws={world_draws} smodel_draws={smodel_draws} xmodel_draws={xmodel_draws} fx_draws={fx_draws} world_skipped={world_skipped} smodel_skipped={smodel_skipped} xmodel_skipped={xmodel_skipped} fx_skipped={fx_skipped} g0_queued={g0_queued} untextured={untextured_n} {untextured:?}"
    );
    // bo2zm: on change, at most every 5 s (it changes most frames).
    let due = perf
        .last_census_at
        .is_none_or(|at| now.duration_since(at).as_secs_f32() >= 5.0);
    if last_census.as_ref() != Some(&line) && due {
        *last_census = Some(line.clone());
        perf.last_census_at = Some(now);
        diag::warn!(World, "{line}");
    }
}

pub(super) fn register(app: &mut App) {
    if !geometry_diagnostic_enabled() {
        return;
    }
    bevy::asset::embedded_asset!(app, "geometry_diagnostic.wgsl");
    super::bo2_bloom::register(app);
    let Some(render_app) = app.get_sub_app_mut(bevy::render::RenderApp) else {
        return;
    };
    render_app
        .init_resource::<ExtractedDiagnosticGeometry>()
        .init_resource::<DiagnosticGeometry>()
        .init_resource::<DiagnosticWorldTextures>()
        .init_resource::<DiagnosticWorldLightmaps>()
        .init_resource::<DiagnosticDynamic>()
        .init_resource::<ExtractedT6Dynamic>()
        .init_resource::<SpecializedRenderPipelines<DiagnosticPipeline>>()
        .add_systems(RenderStartup, init_pipeline)
        .add_systems(
            Render,
            (
                upload_geometry.in_set(RenderSystems::PrepareResources),
                upload_dynamic.in_set(RenderSystems::PrepareResources),
                prepare_views.in_set(RenderSystems::PrepareBindGroups),
                prepare_world_textures.in_set(RenderSystems::PrepareBindGroups),
                prepare_world_lightmaps.in_set(RenderSystems::PrepareBindGroups),
            ),
        )
        .add_systems(
            Core3d,
            draw_geometry_diagnostic
                .in_set(Core3dSystems::MainPass)
                .after(main_opaque_pass_3d)
                .after(super::draw::ExactColourDrawSet),
        );
}
