//! What a T6 zone walk keeps. Assets whose header loads into TEMP vanish
//! when the asset finishes, so the sink copies what later stages need from
//! inside `asset_loaded`, while the header is still in place.
//!
//! References between assets: a pointer to another asset is either loaded
//! where it stands (the walk fixes the slot up to the body, and reports the
//! slot) or names earlier data: for TEMP assets an alias slot in VIRTUAL, for
//! the rest the body itself. The capture keeps every reported slot, alias
//! and body, so a later structure's asset pointer resolves to "material 12",
//! not to an address that TEMP has since reused.

use std::collections::{BTreeMap, HashMap};

use fastfile_t6::layout as l;
use fastfile_t6::{AssetType, Loaded, Ptr, WalkSink, ZonePtr, ZoneStream};

type Result<T> = fastfile_t6::Result<T>;

/// An asset this capture kept: its type and index into that type's list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AssetKey {
    pub ty: AssetType,
    pub index: usize,
}

/// One GfxImage: where its pixels come from.
#[derive(Clone, Debug)]
pub struct ImageRef {
    pub name: String,
    /// `GfxImage::hash`, the pack index's name hash.
    pub name_hash: u32,
    pub map_type: u8,
    pub width: u16,
    pub height: u16,
    pub depth: u16,
    pub level_count: u8,
    /// Streamed parts; on PC 0 or 1.
    pub streamed_parts: u8,
    /// The pack index's data hash of streamed part 0.
    pub part_hash: u32,
    /// Pixels carried in the zone itself (`GfxImageLoadDef`), with its DXGI
    /// format, when it has any.
    pub embedded: Option<EmbeddedImage>,
}

#[derive(Clone, Debug)]
pub struct EmbeddedImage {
    pub dxgi_format: u32,
    pub level_count: u8,
    pub flags: u8,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct MaterialTexture {
    pub name_hash: u32,
    pub semantic: u8,
    pub sampler_state: u8,
    pub image: Option<AssetKey>,
}

#[derive(Clone, Debug)]
pub struct MaterialRef {
    pub name: String,
    pub sort_key: u8,
    /// `MaterialInfo::gameFlags` (0x4: takes no marks).
    pub game_flags: u32,
    /// `MaterialInfo::surfaceTypeBits`: one bit per surface type.
    pub surface_type_bits: u32,
    pub surface_flags: u32,
    pub contents: u32,
    pub technique_set: Option<AssetKey>,
    pub textures: Vec<MaterialTexture>,
    /// `constantTable`: (name hash, name, literal) per constant.
    pub constants: Vec<(u32, String, [f32; 4])>,
    /// `stateBitsEntry`: per technique slot, its index into `state_bits`
    /// (0xff = the slot draws nothing).
    pub state_bits_entry: [u8; 36],
    /// `stateBitsTable`: each entry's `loadBits` (blend, alpha test, cull,
    /// colour write; depth write and test, polygon offset, stencil).
    pub state_bits: Vec<[u32; 2]>,
    pub state_flags: u8,
    pub camera_region: u8,
}

/// One material draw state (`GfxStateBits::loadBits`), decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DrawState {
    /// GFXS_BLEND_*: 0 disabled, 1 zero, 2 one, 3 src colour, 4 inv src
    /// colour, 5 src alpha, 6 inv src alpha, 7 dst alpha, 8 inv dst alpha,
    /// 9 dst colour, 10 inv dst colour.
    pub src_blend: u8,
    pub dst_blend: u8,
    /// GFXS_BLENDOP_*: 0 disabled, 1 add, 2 subtract, 3 rev subtract,
    /// 4 min, 5 max.
    pub blend_op: u8,
    /// `None` when alpha testing is off; else GFXS_ALPHA_TEST_* (0 = above
    /// 0, 1 = at least 128/255).
    pub alpha_test: Option<u8>,
    /// GFXS_CULL_*: 1 none, 2 back, 3 front.
    pub cull: u8,
    pub color_write_rgb: bool,
    pub color_write_alpha: bool,
    pub depth_write: bool,
    /// `None` when the depth test is off; else GFXS_DEPTHTEST_* (0 always,
    /// 1 less, 2 equal, 3 less-equal).
    pub depth_test: Option<u8>,
    /// GFXS_POLYGON_OFFSET_*: 0 none, 1, 2, 3 = shadow map.
    pub polygon_offset: u8,
}

impl DrawState {
    /// Decode `loadBits` (bit positions per the T6 `GfxStateBitsLoadBits`
    /// layout).
    pub fn decode(bits: [u32; 2]) -> Self {
        let [a, b] = bits;
        let f = |v: u32, at: u32, n: u32| ((v >> at) & ((1 << n) - 1)) as u8;
        Self {
            src_blend: f(a, 0, 4),
            dst_blend: f(a, 4, 4),
            blend_op: f(a, 8, 3),
            alpha_test: (f(a, 11, 1) == 0).then(|| f(a, 12, 1)),
            cull: f(a, 14, 2),
            color_write_rgb: f(a, 27, 1) != 0,
            color_write_alpha: f(a, 28, 1) != 0,
            depth_write: f(b, 0, 1) != 0,
            depth_test: (f(b, 1, 1) == 0).then(|| f(b, 2, 2)),
            polygon_offset: f(b, 4, 2),
        }
    }
}

impl MaterialRef {
    /// The draw state of technique slot `tech` (`MaterialTechniqueType`),
    /// when the material draws in it.
    pub fn draw_state(&self, tech: usize) -> Option<DrawState> {
        let entry = *self.state_bits_entry.get(tech)?;
        self.state_bits
            .get(usize::from(entry))
            .map(|&bits| DrawState::decode(bits))
    }
}

/// One animation (`XAnimParts`): its frames, rate and, for a delta
/// animation, the root's translation keys (frame, xyz).
#[derive(Clone, Debug)]
pub struct XAnimRef {
    pub name: String,
    pub numframes: u16,
    pub framerate: f32,
    pub looping: bool,
    pub delta: bool,
    pub delta_trans: Vec<(u16, [f32; 3])>,
    /// bo2zm M2: the whole animation, as the engine's decoder reads it.
    pub bone_count: [u8; 10],
    /// Track (bone) names, `boneCount[PART_TYPE_ALL]` of them.
    pub names: Vec<String>,
    /// Notetracks: (name, time 0..1).
    pub notifies: Vec<(String, f32)>,
    pub data_byte: Vec<u8>,
    pub data_short: Vec<u16>,
    pub data_int: Vec<u32>,
    pub random_data_byte: Vec<u8>,
    pub random_data_short: Vec<u16>,
    pub random_data_int: Vec<u32>,
    /// Key frame indices (bytes when `numframes < 256`, widened).
    pub indices: Vec<u16>,
}

/// One primary light (`ComPrimaryLight`): the map's sun, omni and spot
/// lights.
#[derive(Clone, Debug)]
pub struct PrimaryLightRef {
    /// GFX_LIGHT_TYPE_*: 1 sun, 2 spot, 3 omni (0 none).
    pub ty: u8,
    pub can_use_shadow_map: u8,
    pub exponent: u8,
    pub priority: u8,
    pub cull_dist: i16,
    pub color: [f32; 3],
    pub dir: [f32; 3],
    pub origin: [f32; 3],
    pub radius: f32,
    pub cos_half_fov_outer: f32,
    pub cos_half_fov_inner: f32,
    pub d_attenuation: f32,
    pub roundness: f32,
    pub diffuse_color: [f32; 4],
    pub falloff: [f32; 4],
    pub angle: [f32; 4],
    pub a_ab_b: [f32; 4],
}

/// The world's static visibility lists (`GfxWorldDpvsStatic`): surface
/// ranges in draw order (indices into `sorted_surf_index`).
#[derive(Clone, Debug, Default)]
pub struct DpvsRanges {
    pub static_surface_count: u32,
    pub lit: (u32, u32),
    pub lit_trans: (u32, u32),
    pub emissive_opaque: (u32, u32),
    pub emissive_trans: (u32, u32),
}

/// One cell AABB tree node (`GfxAabbTree`): the surfaces (a run of
/// `sorted_surf_index`) and static models it draws.
#[derive(Clone, Debug)]
pub struct AabbTreeRef {
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub child_count: u16,
    pub surface_count: u16,
    pub start_surf_index: u16,
    pub smodel_indexes: Vec<u16>,
    pub children_offset: i32,
}

#[derive(Clone, Debug)]
pub struct TechniqueSetRef {
    pub name: String,
    pub world_vert_format: u8,
    /// The 36 technique slots (`MaterialTechniqueSet::techniques`).
    pub techniques: Vec<Option<TechniqueRef>>,
}

/// One technique: its passes.
#[derive(Clone, Debug)]
pub struct TechniqueRef {
    pub name: String,
    pub flags: u16,
    pub passes: Vec<PassRef>,
}

/// One pass: its shaders by name, the pixel shader's D3D11 bytecode (kept
/// only when the capture asks for shaders), and its arguments.
#[derive(Clone, Debug)]
pub struct PassRef {
    pub vertex_shader: String,
    pub pixel_shader: String,
    pub pixel_program: Vec<u8>,
    pub vertex_program: Vec<u8>,
    pub args: Vec<ShaderArgRef>,
}

/// One `MaterialShaderArgument`: what feeds a register.
#[derive(Clone, Copy, Debug)]
pub struct ShaderArgRef {
    pub ty: u16,
    pub location: u16,
    pub size: u16,
    pub buffer: u16,
    /// The union: a name hash, code constant or code sampler index (or a
    /// literal's address, meaningless here).
    pub u: u32,
}

#[derive(Clone, Debug)]
pub struct XModelRef {
    pub name: String,
    pub numsurfs: u8,
    pub materials: Vec<Option<AssetKey>>,
    pub num_bones: u8,
    pub num_lods: u16,
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub radius: f32,
    /// The first (most detailed) LOD's surfaces.
    pub lod0: Vec<XSurfaceRef>,
    /// bo2zm M2: the skeleton. Bone names, the root bone count, each child
    /// bone's parent step, its local rotation (`XModelQuat`) and
    /// translation, every bone's bind matrix (`baseMat`: quat xyzw +
    /// trans), and the hit-location class per bone.
    pub bone_names: Vec<String>,
    pub num_root_bones: u8,
    pub parent_list: Vec<u8>,
    pub quats: Vec<[i16; 4]>,
    pub trans: Vec<[f32; 3]>,
    pub base_mat: Vec<([f32; 4], [f32; 3])>,
    pub part_classification: Vec<u8>,
    /// bo2zm M3: each bone's hit box (`XBoneInfo`: bounds mins, maxs in the
    /// bone's space, radius squared; none when the bone has no box).
    pub bone_info: Vec<Option<([f32; 3], [f32; 3], f32)>>,
    /// Per LOD: (distance, surface count, first surface).
    pub lods: Vec<(f32, u16, u16)>,
    pub contents: u32,
    /// bo2zm fix list 2: the collision LOD (`collLod`, negative = none) and
    /// the collision surfaces (`collSurfs`) bullets and thrown things hit.
    pub coll_lod: i16,
    pub coll_surfs: Vec<XModelCollSurfRef>,
    /// bo2mp: how the model flies when it comes loose (`physPreset`): a
    /// destructible's broken-off piece without one stays put.
    pub phys_preset: Option<AssetKey>,
}

/// One model collision surface (`XModelCollSurf_s`): its triangles (each a
/// plane + the two edge vectors of `XModelCollTri_s`), bounds in model
/// space, bone, contents and surface flags.
#[derive(Clone, Debug, Default)]
pub struct XModelCollSurfRef {
    pub tris: Vec<[[f32; 4]; 3]>,
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub bone_idx: i32,
    pub contents: u32,
    pub surf_flags: u32,
}

/// One placed static model of the clip map (`cStaticModel_s`).
#[derive(Clone, Copy, Debug)]
pub struct ClipStaticModelRef {
    pub model: Option<AssetKey>,
    pub contents: u32,
    pub origin: [f32; 3],
    pub inv_scaled_axis: [[f32; 3]; 3],
    pub absmin: [f32; 3],
    pub absmax: [f32; 3],
}

/// One model surface (`XSurface`) in model space.
#[derive(Clone, Debug, Default)]
pub struct XSurfaceRef {
    /// `GfxPackedVertex`, 32 bytes each: xyz f32x3 @0, binormal sign @12,
    /// colour @16, texcoord half2 @20, normal @24 and tangent @28 packed
    /// like the world's.
    pub verts: Vec<u8>,
    pub vert_count: u16,
    pub indices: Vec<u16>,
    /// Rigid vertex lists: (bone offset, vert count, tri offset, tri count).
    pub vert_lists: Vec<(u16, u16, u16, u16)>,
    pub flags: u8,
    pub material: Option<AssetKey>,
    /// bo2zm M2: weighted vertices (`XSurfaceVertexInfo`): how many
    /// vertices have 1..4 influences, and their packed blend words.
    pub blend_counts: [i16; 4],
    pub blend: Vec<u16>,
    pub part_bits: [u32; 6],
}

/// One world surface (`GfxSurface`).
#[derive(Clone, Debug)]
pub struct WorldSurface {
    /// `srfTriangles_t` bounds (zero on many retail surfaces).
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    /// `GfxSurface::bounds`.
    pub bounds: [[f32; 3]; 2],
    pub vertex_data_offset0: i32,
    pub vertex_data_offset1: i32,
    pub first_vertex: i32,
    pub vertex_count: u16,
    pub tri_count: u16,
    pub base_index: i32,
    pub material: Option<AssetKey>,
    pub lightmap_index: i8,
    pub reflection_probe_index: i8,
    pub primary_light_index: i8,
    pub flags: u8,
}

/// One static model placement (`GfxStaticModelDrawInst`).
#[derive(Clone, Debug)]
pub struct StaticModel {
    pub origin: [f32; 3],
    pub axis: [[f32; 3]; 3],
    pub scale: f32,
    pub model: Option<AssetKey>,
    pub flags: i32,
    /// Baked per-vertex light for its first LOD
    /// (`lmapVertexInfo[0].lmapVertexColors`), one u32 per vertex; empty
    /// when the instance has none.
    pub lmap_colors: Vec<u32>,
    /// `lightingSH`, quantized (V0, V1, V2: four u16 each).
    pub lighting_sh: [u16; 12],
    /// `colorsIndex`: the light grid colour set the compiler picked for
    /// this placement (its light when it has no baked vertex light).
    pub colors_index: u16,
    pub primary_light_index: u8,
    pub reflection_probe_index: u8,
    /// `cullDist`: beyond this distance the instance is not drawn.
    pub cull_dist: f32,
}

/// The map's GfxWorld, in its own terms.
#[derive(Clone, Debug, Default)]
pub struct WorldRef {
    pub name: String,
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub vertex_count: u32,
    pub vd0: Vec<u8>,
    pub vd1: Vec<u8>,
    pub indices: Vec<u16>,
    pub surfaces: Vec<WorldSurface>,
    pub static_models: Vec<StaticModel>,
    pub lightmaps: Vec<[Option<AssetKey>; 2]>,
    /// `GfxWorldDraw::reflectionProbes`: the cube maps surfaces reflect
    /// (`WorldSurface::reflection_probe_index` picks one).
    pub reflection_probes: Vec<ReflectionProbeRef>,
    pub sky_box_model: String,
    /// `GfxWorld::sunLight`: the sun the lit world shaders add to the
    /// lightmap (`sunDiffuse` times its baked visibility).
    pub sun: Option<SunRef>,
    /// `GfxWorld::exposureVolumes`: (exposure, increase scale, decrease
    /// scale, feather range) per volume.
    pub exposure_volumes: Vec<[f32; 4]>,
    /// bo2mp: `GfxWorld::sunParse.initWorldSun.exposure`, the map's own
    /// exposure where no exposure volume holds the eye.
    pub init_sun_exposure: f32,
    /// `GfxWorld::lightGrid`, the static-model lighting source.
    pub light_grid: Option<LightGridRef>,
    /// `GfxWorld::lutMaterial`: the map's colour grading material.
    pub lut_material: Option<AssetKey>,
    /// `GfxWorld::lutVolumes`: per volume its box, control word, transition
    /// time and the band of the LUT image it picks.
    pub lut_volumes: Vec<([f32; 3], [f32; 3], u32, u32, u32)>,
    /// `GfxWorld::skyDynIntensity`: angle0, angle1, factor0, factor1.
    pub sky_dyn_intensity: [f32; 4],
    /// The static visibility lists.
    pub dpvs: DpvsRanges,
    /// `sortedSurfIndex`: draw order -> surface.
    pub sorted_surf_index: Vec<u16>,
    /// bo2zm M3: the brush models (`GfxBrushModel`, submodel n = entry n:
    /// doors, debris): their first surface and surface count in `surfaces`,
    /// and their bounds.
    pub brush_models: Vec<(u32, u32, [[f32; 3]; 2])>,
    /// Per cell, its AABB trees.
    pub cells: Vec<Vec<AabbTreeRef>>,
    /// `GfxWorld::primaryLightCount`.
    pub primary_light_count: u32,
    /// `GfxWorld::sunParse.initWorldFog`: the fog the map starts with.
    pub init_fog: Option<WorldFogRef>,
    /// `GfxWorld::worldFogVolumes`: per volume its box, control words and
    /// fog.
    pub fog_volumes: Vec<([f32; 3], [f32; 3], u32, u32, WorldFogRef)>,
}

/// One `GfxWorldFog`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WorldFogRef {
    pub base_dist: f32,
    pub half_dist: f32,
    pub base_height: f32,
    pub half_height: f32,
    pub sun_fog_pitch: f32,
    pub sun_fog_yaw: f32,
    pub sun_fog_inner: f32,
    pub sun_fog_outer: f32,
    pub fog_color: [f32; 3],
    pub fog_opacity: f32,
    pub sun_fog_color: [f32; 3],
    pub sun_fog_opacity: f32,
}

fn read_world_fog(s: &ZoneStream<'_>, f: Ptr) -> Result<WorldFogRef> {
    Ok(WorldFogRef {
        base_dist: s.f32_at(f, l::GfxWorldFog::baseDist)?,
        half_dist: s.f32_at(f, l::GfxWorldFog::halfDist)?,
        base_height: s.f32_at(f, l::GfxWorldFog::baseHeight)?,
        half_height: s.f32_at(f, l::GfxWorldFog::halfHeight)?,
        sun_fog_pitch: s.f32_at(f, l::GfxWorldFog::sunFogPitch)?,
        sun_fog_yaw: s.f32_at(f, l::GfxWorldFog::sunFogYaw)?,
        sun_fog_inner: s.f32_at(f, l::GfxWorldFog::sunFogInner)?,
        sun_fog_outer: s.f32_at(f, l::GfxWorldFog::sunFogOuter)?,
        fog_color: vec3(s, f, l::GfxWorldFog::fogColor)?,
        fog_opacity: s.f32_at(f, l::GfxWorldFog::fogOpacity)?,
        sun_fog_color: vec3(s, f, l::GfxWorldFog::sunFogColor)?,
        sun_fog_opacity: s.f32_at(f, l::GfxWorldFog::sunFogOpacity)?,
    })
}

/// The world's light grid (`GfxLightGrid`): grid-space bounds, row axes,
/// the compressed rows, entries (4 bytes each) and colour sets (56
/// directions of RGB8, 168 bytes each).
#[derive(Clone, Debug, Default)]
pub struct LightGridRef {
    pub sun_primary_light_index: u32,
    pub mins: [u16; 3],
    pub maxs: [u16; 3],
    pub offset: f32,
    pub row_axis: u32,
    pub col_axis: u32,
    pub row_data_start: Vec<u8>,
    pub raw_row_data: Vec<u8>,
    pub entries: Vec<u8>,
    pub colors: Vec<u8>,
    pub color_count: u32,
    /// `coeffs`: per set, 27 u16 (`GfxCompressedLightGridCoeffs`, 54 bytes);
    /// a grid with SH coefficients has no colour sets.
    pub coeffs: Vec<u8>,
    pub coeff_count: u32,
    /// `skyGridVolumes`: the light for places outside the grid (the
    /// distant terrain): bounds, the point the light was taken at, its
    /// coefficient set, primary light and that light's visibility.
    pub sky_volumes: Vec<SkyGridVolumeRef>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SkyGridVolumeRef {
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub lighting_origin: [f32; 3],
    pub colors_index: u16,
    pub primary_light_index: u8,
    pub visibility: u8,
}

/// One reflection probe (`GfxReflectionProbe`): where it was captured, the
/// lighting there as BO2's shaders evaluate it per normal (`lightingSH`, three
/// vec4: colour scale + z^2 weight, linear + constant, quadratic terms), and
/// its cube map.
#[derive(Clone, Debug)]
pub struct ReflectionProbeRef {
    pub origin: [f32; 3],
    pub lighting_sh: [[f32; 4]; 3],
    pub image: Option<AssetKey>,
    pub mip_lod_bias: f32,
}

/// The world's sun light (`GfxLight`).
#[derive(Clone, Copy, Debug)]
pub struct SunRef {
    pub color: [f32; 3],
    pub dir: [f32; 3],
    pub diffuse_color: [f32; 4],
}

/// One collision brush (`cbrush_t`): its box, contents, axial side flags
/// ([min x, y, z], [max x, y, z]) and the extra side planes.
#[derive(Clone, Debug)]
pub struct ClipBrushRef {
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub contents: i32,
    pub axial_cflags: [[i32; 3]; 2],
    pub axial_sflags: [[i32; 3]; 2],
    /// (normal, dist, surface flags) per non-axial side.
    pub sides: Vec<([f32; 3], f32, i32)>,
}

#[derive(Clone, Copy, Debug)]
pub struct ClipAabbRef {
    pub origin: [f32; 3],
    pub half_size: [f32; 3],
    pub material_index: u16,
    pub child_count: u16,
    pub u: i32,
}

#[derive(Clone, Debug)]
pub struct ClipMaterialRef {
    pub name: String,
    pub surface_flags: i32,
    pub content_flags: i32,
}

/// One collision tree node (`cNode_t`): its split plane (normal, dist) and
/// two children; a negative child `c` is leaf `-1 - c`.
#[derive(Clone, Copy, Debug)]
pub struct ClipNodeRef {
    pub plane: [f32; 4],
    pub children: [i16; 2],
}

/// One collision tree leaf (`cLeaf_s`): its mesh trees and the root of its
/// brush list in `leafbrush_nodes` (0 = no brushes).
#[derive(Clone, Copy, Debug, Default)]
pub struct ClipLeafRef {
    pub first_coll_aabb_index: u16,
    pub coll_aabb_count: u16,
    pub brush_contents: i32,
    pub leaf_brush_node: i32,
}

/// One node of a leaf's brush list tree (`cLeafBrushNode_s`): brush indices
/// when `leaf_brush_count > 0`, else child offsets from this node.
#[derive(Clone, Debug)]
pub struct ClipLeafBrushNodeRef {
    pub leaf_brush_count: i16,
    pub brushes: Vec<u16>,
    pub child_offset: [u16; 2],
}

/// One brush model (`cmodel_t`): model 0 is the world, the rest belong to
/// entities (doors, triggers, volumes) and are not world walls.
#[derive(Clone, Copy, Debug)]
pub struct ClipCmodelRef {
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub radius: f32,
    pub leaf: ClipLeafRef,
    /// Its `ClipInfo` is the map's own (or none): its brush indices index
    /// the map's brushes.
    pub shares_map_info: bool,
}

/// The map's clipMap_t, in its own terms.
#[derive(Clone, Debug, Default)]
pub struct ClipRef {
    pub name: String,
    pub brushes: Vec<ClipBrushRef>,
    pub materials: Vec<ClipMaterialRef>,
    pub verts: Vec<[f32; 3]>,
    pub tri_indices: Vec<u16>,
    pub tri_edge_is_walkable: Vec<u8>,
    /// (triCount, firstTri) per partition.
    pub partitions: Vec<(u8, i32)>,
    pub aabb_trees: Vec<ClipAabbRef>,
    pub static_model_count: u32,
    pub nodes: Vec<ClipNodeRef>,
    pub leaves: Vec<ClipLeafRef>,
    pub leafbrush_nodes: Vec<ClipLeafBrushNodeRef>,
    pub cmodels: Vec<ClipCmodelRef>,
    /// `ClipInfo::brushContents`: per-brush contents the world tree tests.
    pub brush_contents: Vec<i32>,
    /// bo2zm fix list 2: `staticModelList`, the placed models bullets and
    /// thrown things collide with.
    pub static_models: Vec<ClipStaticModelRef>,
}

fn read_leaf(s: &ZoneStream<'_>, p: Ptr) -> Result<ClipLeafRef> {
    Ok(ClipLeafRef {
        first_coll_aabb_index: s.u16_at(p, l::cLeaf_s::firstCollAabbIndex)?,
        coll_aabb_count: s.u16_at(p, l::cLeaf_s::collAabbCount)?,
        brush_contents: s.i32_at(p, l::cLeaf_s::brushContents)?,
        leaf_brush_node: s.i32_at(p, l::cLeaf_s::leafBrushNode)?,
    })
}

/// Follow a pointer field that the walk already resolved.
pub(crate) fn deref(s: &ZoneStream<'_>, p: Ptr, off: usize) -> Result<Option<Ptr>> {
    Ok(match s.ptr_at(p, off)? {
        ZonePtr::Offset(q) => Some(s.resolve_alias(q)),
        _ => None,
    })
}

pub(crate) fn string_field(s: &ZoneStream<'_>, p: Ptr, off: usize) -> Result<String> {
    Ok(match deref(s, p, off)? {
        Some(q) => String::from_utf8_lossy(s.cstr_bytes(q)?).into_owned(),
        None => String::new(),
    })
}

fn vec3(s: &ZoneStream<'_>, p: Ptr, off: usize) -> Result<[f32; 3]> {
    Ok([
        s.f32_at(p, off)?,
        s.f32_at(p, off + 4)?,
        s.f32_at(p, off + 8)?,
    ])
}

fn bytes_field(s: &ZoneStream<'_>, p: Ptr, off: usize, len: usize) -> Result<Vec<u8>> {
    Ok(match deref(s, p, off)? {
        Some(q) if len > 0 => s.slice_at(q, 0, len)?.to_vec(),
        _ => Vec::new(),
    })
}

pub fn read_image(s: &ZoneStream<'_>, img: Ptr) -> Result<ImageRef> {
    let part = img.at(l::GfxImage::streamedParts);
    let embedded = match deref(s, img, l::GfxImage::texture)? {
        Some(def) => {
            let size = s.u32_at(def, l::GfxImageLoadDef::resourceSize)? as usize;
            if size == 0 {
                None
            } else {
                Some(EmbeddedImage {
                    dxgi_format: s.u32_at(def, l::GfxImageLoadDef::format)?,
                    level_count: s.u8_at(def, l::GfxImageLoadDef::levelCount)?,
                    flags: s.u8_at(def, l::GfxImageLoadDef::flags)?,
                    data: s.slice_at(def, l::GfxImageLoadDef::data, size)?.to_vec(),
                })
            }
        }
        None => None,
    };
    Ok(ImageRef {
        name: string_field(s, img, l::GfxImage::name)?,
        name_hash: s.u32_at(img, l::GfxImage::hash)?,
        map_type: s.u8_at(img, l::GfxImage::mapType)?,
        width: s.u16_at(img, l::GfxImage::width)?,
        height: s.u16_at(img, l::GfxImage::height)?,
        depth: s.u16_at(img, l::GfxImage::depth)?,
        level_count: s.u8_at(img, l::GfxImage::levelCount)?,
        streamed_parts: s.u8_at(img, l::GfxImage::streamedPartCount)?,
        part_hash: s.u32_at(part, l::GfxStreamedPartInfo::hash)?,
        embedded,
    })
}

/// bo2zm M3: one AI path node (GameWorldMp `pathnode_t`): its kind (path,
/// cover, negotiation begin/end, ...), names, where it is and its links.
#[derive(Clone, Debug, Default)]
pub struct PathNodeRef {
    pub ty: u32,
    pub spawnflags: u32,
    pub targetname: String,
    pub target: String,
    pub script_noteworthy: String,
    pub script_linkname: String,
    pub animscript: String,
    pub origin: [f32; 3],
    pub angle: f32,
    pub radius: f32,
    /// (node, distance, negotiation link).
    pub links: Vec<(u16, f32, bool)>,
}

/// bo2zm M3: a string table: `rows` x `columns` cells, row-major.
#[derive(Clone, Debug, Default)]
pub struct StringTableRef {
    pub name: String,
    pub columns: usize,
    pub rows: usize,
    pub cells: Vec<String>,
}

/// bo2mp: a vehicle's own weapons (`VehicleDef::turretWeapon`,
/// `gunnerWeapon[4]`): what a scorestreak's helicopter fires.
#[derive(Clone, Debug, Default)]
pub struct VehicleRef {
    pub name: String,
    pub turret_weapon: String,
    pub gunner_weapons: Vec<String>,
    pub drive: VehicleDrive,
}

/// bo2mp: a destructible's definition (`DestructibleDef`): the models it
/// stands for. A map entity names a destructible with `destructibledef`;
/// its own `model` is only the editor's stand-in.
#[derive(Clone, Debug, Default)]
pub struct DestructibleRef {
    pub name: String,
    /// The model the entity is built with.
    pub model: String,
    /// The pristine variant.
    pub pristine_model: String,
    /// The pieces that break off (`DestructiblePiece`), piece 0 the base.
    pub pieces: Vec<DestructiblePieceRef>,
    /// Only the clients break it (nothing the server tracks).
    pub client_only: bool,
}

/// bo2mp: one piece of a destructible (`DestructiblePiece`): its health,
/// how much each kind of damage counts, the stages it breaks through and
/// the bones it starts with hidden.
#[derive(Clone, Debug, Default)]
pub struct DestructiblePieceRef {
    pub stages: [DestructibleStageRef; 5],
    pub parent_piece: u8,
    pub parent_damage_percent: f32,
    pub bullet_damage_scale: f32,
    pub explosive_damage_scale: f32,
    pub melee_damage_scale: f32,
    pub health: i32,
    /// Hide-part bits (`hideBones`), the model's bone order.
    pub hide_bones: [u32; 5],
    /// The first of the piece's constraints (`physConstraints->data[0]`),
    /// the only one a break reads: a launch (type 7) aims what breaks off.
    pub constraint: Option<PhysConstraintRef>,
}

/// bo2mp: a physics preset (`PhysPreset`): how a loose piece weighs,
/// bounces and falls.
#[derive(Clone, Debug, Default)]
pub struct PhysPresetRef {
    pub name: String,
    pub mass: f32,
    pub bounce: f32,
    pub friction: f32,
    pub bullet_force_scale: f32,
    pub explosive_force_scale: f32,
    pub pieces_spread_fraction: f32,
    pub pieces_upward_velocity: f32,
    pub gravity_scale: f32,
    pub center_of_mass_offset: [f32; 3],
}

/// bo2mp: one physics constraint (`PhysConstraint`), the fields a break's
/// launch reads: the push's strength (`power`), its direction as angles
/// added to the thing's own (`scale`) and how far off its centre it is
/// pushed (`spin_scale`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PhysConstraintRef {
    pub kind: i32,
    pub power: f32,
    pub scale: [f32; 3],
    pub spin_scale: f32,
}

/// bo2mp: a model's constraint set (`PhysConstraints`).
#[derive(Clone, Debug, Default)]
pub struct PhysConstraintsRef {
    pub name: String,
    pub data: Vec<PhysConstraintRef>,
}

/// bo2mp: one stage of a piece (`DestructibleStage`): the bone it shows,
/// the share of the piece's health it lasts to, and what breaking out of
/// it plays and tells the scripts. Empty strings are unset.
#[derive(Clone, Debug, Default)]
pub struct DestructibleStageRef {
    /// Empty: the stage shows nothing (the piece's last).
    pub show_bone: String,
    pub break_health: f32,
    /// A stage that breaks by itself after this many seconds.
    pub max_time: f32,
    pub flags: u32,
    pub break_effect: String,
    pub break_sound: String,
    /// What the scripts hear (`CodeCallback_DestructibleEvent` "broken").
    pub break_notify: String,
    pub loop_sound: String,
    pub spawn_models: [String; 3],
    pub phys_preset: bool,
}

/// bo2mp: how a vehicle drives and how its driver sees it (`VehicleDef`).
/// Speeds are units a second, angles degrees.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VehicleDrive {
    /// `cameraMode`: 0 = his view from the vehicle's `tag_player`, else the
    /// third-person camera below.
    pub camera_mode: i32,
    pub max_speed: f32,
    pub max_speed_vertical: f32,
    pub accel: f32,
    pub accel_vertical: f32,
    /// Reversing, a share of `max_speed` (`nitrousVehParams.m_reverse_scale`).
    pub reverse_scale: f32,
    /// `thirdPersonCameraRange`, `thirdPersonCameraHeight[1]`.
    pub camera_range: f32,
    pub camera_height: f32,
    /// How far the third-person camera looks up and down
    /// (`thirdPersonCameraMinPitchClamp`, `...MaxPitchClamp`).
    pub camera_pitch: [f32; 2],
    /// `cameraFOV` (0: his own).
    pub camera_fov: f32,
    /// How far the driver looks up and down from its gun
    /// (`turretViewLimits[2]`, `[3]`).
    pub turret_pitch: [f32; 2],
    /// `thirdPersonDriver`: the driver's seat views from outside.
    pub third_person_driver: i32,
    /// The command each of the vehicle's buttons stands for, in BO2's
    /// default controller binds (players/bindings_mp.bdg: A +gostand, B
    /// +stance, X +usereload, Y +weapnext_inventory, LSHLDR +smoke, RSHLDR
    /// +frag, LSTICK +breath_sprint, RSTICK +melee, LTRIG +speed_throw, RTRIG
    /// +attack; "" for none): `moveUpButtonName`, `moveDownButtonName`,
    /// `switchSeatButtonName`, `attackButtonName`, `attackSecondaryButtonName`.
    /// BO2 puts the vehicle's own commands on the keys he has for these.
    pub buttons: [&'static str; 5],
}

/// The command BO2's default controller binds put on a button name.
fn vehicle_button_command(name: &str) -> &'static str {
    match name {
        "BUTTON_A" => "+gostand",
        "BUTTON_B" => "+stance",
        "BUTTON_X" => "+usereload",
        "BUTTON_Y" => "+weapnext_inventory",
        "BUTTON_LSHLDR" => "+smoke",
        "BUTTON_RSHLDR" => "+frag",
        "BUTTON_LSTICK" => "+breath_sprint",
        "BUTTON_RSTICK" => "+melee",
        "BUTTON_LTRIG" => "+speed_throw",
        "BUTTON_RTRIG" => "+attack",
        _ => "",
    }
}

/// Everything one walk captured.
#[derive(Default)]
pub struct ZoneCapture {
    pub zone: String,
    pub images: Vec<ImageRef>,
    pub materials: Vec<MaterialRef>,
    pub technique_sets: Vec<TechniqueSetRef>,
    pub xmodels: Vec<XModelRef>,
    pub world: Option<WorldRef>,
    pub clip: Option<ClipRef>,
    /// `ComWorld::primaryLights`.
    pub primary_lights: Vec<PrimaryLightRef>,
    /// Every RawFile (vision files, configs, Lua): (name, bytes).
    pub raw_files: Vec<(String, Vec<u8>)>,
    /// bo2zm M3: every StringTable (`mp/zombiemode.csv`, ...).
    pub string_tables: Vec<StringTableRef>,
    /// bo2zm M3: every localized string (`ZOMBIE_WEAPON_M14` -> its text),
    /// from a language zone (`zone/english/en_*.ff`).
    pub localize: Vec<(String, String)>,
    /// bo2zm M3: the map's AI path nodes and their links (GameWorldMp).
    pub path_nodes: Vec<PathNodeRef>,
    /// Every XAnimParts: its timing and root translation.
    pub xanims: Vec<XAnimRef>,
    /// Every MapEnts entity string, in load order (the clipmap's own first).
    pub map_ents: Vec<String>,
    /// bo2zm M2: the zone's script strings (bone names, notetracks).
    pub strings: Vec<String>,
    pub weapons: Vec<crate::m2::WeaponRef>,
    /// bo2mp: weapon attachments (`acog`, `vzoom`: what an attachment
    /// changes on every gun) and each gun's own take on one
    /// (`au_svu_acog`: its sight model, scope overlay, animations).
    pub attachments: Vec<crate::m2::AttachmentRef>,
    pub attachment_uniques: Vec<crate::m2::AttachmentUniqueRef>,
    /// bo2mp: vehicles and their weapons.
    pub vehicles: Vec<VehicleRef>,
    /// bo2mp: destructible definitions (a map's cars, barrels, ...).
    pub destructibles: Vec<DestructibleRef>,
    /// bo2mp: physics presets and constraint sets (what a broken-off piece
    /// flies with).
    pub phys_presets: Vec<PhysPresetRef>,
    pub phys_constraints: Vec<PhysConstraintsRef>,
    /// bo2zm M3: weapon camos (Pack-a-Punch's look).
    pub weapon_camos: Vec<crate::m2::WeaponCamoRef>,
    /// bo2zm M3: fonts (the HUD's text).
    pub fonts: Vec<crate::m2::FontRef>,
    pub fx: Vec<crate::m2::FxEffectRef>,
    pub tracers: Vec<crate::m2::TracerRef>,
    pub impact_tables: Vec<crate::m2::ImpactTableRef>,
    pub footstep_tables: Vec<crate::m2::FootstepTableRef>,
    pub footstep_fx_tables: Vec<crate::m2::FootstepFxTableRef>,
    pub sound_banks: Vec<crate::m2::SndBankRef>,
    pub snd_globals: Option<crate::m2::SndDriverGlobalsRef>,
    pub loads: BTreeMap<AssetType, usize>,
    /// The slot each kept asset was loaded through, with the load's serial:
    /// TEMP addresses are reused, so a slot entry only counts for a parent
    /// that began before it.
    slots: HashMap<Ptr, (AssetKey, u64)>,
    /// Insert aliases (VIRTUAL, stable): what later references go through.
    aliases: HashMap<Ptr, AssetKey>,
    /// `started` of the asset whose fields are being read.
    parent_started: u64,
    /// Asset pointers that named nothing this capture kept.
    pub unresolved_refs: usize,
    /// Keep every pass's shader bytecode (probes only; a game load does not).
    pub keep_shaders: bool,
}

impl ZoneCapture {
    /// The asset an asset-pointer slot of the asset being read names, if
    /// this capture kept it: a load through that very slot during this
    /// parent, else the alias the pointer goes through.
    pub fn asset_at(&mut self, s: &ZoneStream<'_>, slot: Ptr) -> Result<Option<AssetKey>> {
        let raw = s.u32_at(slot, 0)?;
        if raw == 0 {
            return Ok(None);
        }
        if let Some((key, serial)) = self.slots.get(&slot)
            && *serial > self.parent_started
        {
            return Ok(Some(*key));
        }
        if let ZonePtr::Offset(p) = ZonePtr::decode(raw)
            && let Some(key) = self.aliases.get(&p)
        {
            return Ok(Some(*key));
        }
        self.unresolved_refs += 1;
        Ok(None)
    }

    fn keep(&mut self, loaded: Loaded, key: AssetKey) {
        self.slots.insert(loaded.slot, (key, loaded.serial));
        // Later references name either the insert alias or, when the asset
        // loaded through a slot outside TEMP (an asset-list entry, an array
        // of asset pointers), that slot itself; both stay put for the zone.
        if let Some(alias) = loaded.alias {
            self.aliases.insert(alias, key);
        }
        if loaded.slot.block as usize != fastfile_t6::XFILE_BLOCK_TEMP {
            self.aliases.insert(loaded.slot, key);
        }
    }

    pub fn image(&self, key: AssetKey) -> Option<&ImageRef> {
        (key.ty == AssetType::Image)
            .then(|| self.images.get(key.index))
            .flatten()
    }

    pub fn material(&self, key: AssetKey) -> Option<&MaterialRef> {
        (key.ty == AssetType::Material)
            .then(|| self.materials.get(key.index))
            .flatten()
    }

    fn read_material(&mut self, s: &ZoneStream<'_>, m: Ptr) -> Result<MaterialRef> {
        let info = m.at(l::Material::info);
        let count = s.u8_at(m, l::Material::textureCount)? as usize;
        let mut textures = Vec::with_capacity(count);
        if let Some(table) = deref(s, m, l::Material::textureTable)? {
            for i in 0..count {
                let t = table.at(i * l::MaterialTextureDef::SIZE);
                textures.push(MaterialTexture {
                    name_hash: s.u32_at(t, l::MaterialTextureDef::nameHash)?,
                    semantic: s.u8_at(t, l::MaterialTextureDef::semantic)?,
                    sampler_state: s.u8_at(t, l::MaterialTextureDef::samplerState)?,
                    image: self.asset_at(s, t.at(l::MaterialTextureDef::image))?,
                });
            }
        }
        let constant_count = s.u8_at(m, l::Material::constantCount)? as usize;
        let mut constants = Vec::with_capacity(constant_count);
        if let Some(table) = deref(s, m, l::Material::constantTable)? {
            for i in 0..constant_count {
                let c = table.at(i * l::MaterialConstantDef::SIZE);
                let name = s.slice_at(c, l::MaterialConstantDef::name, 12)?;
                let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
                constants.push((
                    s.u32_at(c, l::MaterialConstantDef::nameHash)?,
                    String::from_utf8_lossy(&name[..end]).into_owned(),
                    [
                        s.f32_at(c, l::MaterialConstantDef::literal)?,
                        s.f32_at(c, l::MaterialConstantDef::literal + 4)?,
                        s.f32_at(c, l::MaterialConstantDef::literal + 8)?,
                        s.f32_at(c, l::MaterialConstantDef::literal + 12)?,
                    ],
                ));
            }
        }
        let mut state_bits_entry = [0xffu8; 36];
        for (i, e) in state_bits_entry.iter_mut().enumerate() {
            *e = s.u8_at(m, l::Material::stateBitsEntry + i)?;
        }
        let state_count = usize::from(s.u8_at(m, l::Material::stateBitsCount)?);
        let mut state_bits = Vec::with_capacity(state_count);
        if let Some(table) = deref(s, m, l::Material::stateBitsTable)? {
            for i in 0..state_count {
                let e = table.at(i * l::GfxStateBits::SIZE + l::GfxStateBits::loadBits);
                state_bits.push([s.u32_at(e, 0)?, s.u32_at(e, 4)?]);
            }
        }
        Ok(MaterialRef {
            name: string_field(s, info, l::MaterialInfo::name)?,
            sort_key: s.u8_at(info, l::MaterialInfo::sortKey)?,
            game_flags: s.u32_at(info, l::MaterialInfo::gameFlags)?,
            surface_type_bits: s.u32_at(info, l::MaterialInfo::surfaceTypeBits)?,
            surface_flags: s.u32_at(info, l::MaterialInfo::surfaceFlags)?,
            contents: s.u32_at(info, l::MaterialInfo::contents)?,
            technique_set: self.asset_at(s, m.at(l::Material::techniqueSet))?,
            textures,
            constants,
            state_bits_entry,
            state_bits,
            state_flags: s.u8_at(m, l::Material::stateFlags)?,
            camera_region: s.u8_at(m, l::Material::cameraRegion)?,
        })
    }

    fn read_technique_set(&mut self, s: &ZoneStream<'_>, t: Ptr) -> Result<TechniqueSetRef> {
        let slots = (l::MaterialTechniqueSet::SIZE - l::MaterialTechniqueSet::techniques) / 4;
        let mut techniques = Vec::with_capacity(slots);
        for i in 0..slots {
            let Some(tech) = deref(s, t, l::MaterialTechniqueSet::techniques + i * 4)? else {
                techniques.push(None);
                continue;
            };
            let pass_count = usize::from(s.u16_at(tech, l::MaterialTechnique::passCount)?);
            let mut passes = Vec::with_capacity(pass_count);
            for p in 0..pass_count {
                let pass = tech.at(l::MaterialTechnique::passArray + p * l::MaterialPass::SIZE);
                let shader = |off: usize, load: usize| -> Result<(String, Vec<u8>)> {
                    let Some(sh) = deref(s, pass, off)? else {
                        return Ok((String::new(), Vec::new()));
                    };
                    let name = string_field(s, sh, l::MaterialPixelShader::name)?;
                    let program = if self.keep_shaders {
                        let def = sh.at(load);
                        let size = s.u32_at(def, l::GfxPixelShaderLoadDef::programSize)? as usize;
                        bytes_field(s, def, l::GfxPixelShaderLoadDef::program, size)?
                    } else {
                        Vec::new()
                    };
                    Ok((name, program))
                };
                let load = l::MaterialPixelShader::prog + l::MaterialPixelShaderProgram::loadDef;
                let (pixel_shader, pixel_program) = shader(l::MaterialPass::pixelShader, load)?;
                let (vertex_shader, vertex_program) = shader(l::MaterialPass::vertexShader, load)?;
                let arg_count = usize::from(s.u8_at(pass, l::MaterialPass::perPrimArgCount)?)
                    + usize::from(s.u8_at(pass, l::MaterialPass::perObjArgCount)?)
                    + usize::from(s.u8_at(pass, l::MaterialPass::stableArgCount)?);
                let mut args = Vec::with_capacity(arg_count);
                if let Some(arr) = deref(s, pass, l::MaterialPass::args)? {
                    for a in 0..arg_count {
                        let arg = arr.at(a * l::MaterialShaderArgument::SIZE);
                        args.push(ShaderArgRef {
                            ty: s.u16_at(arg, l::MaterialShaderArgument::r#type)?,
                            location: s.u16_at(arg, l::MaterialShaderArgument::location)?,
                            size: s.u16_at(arg, l::MaterialShaderArgument::size)?,
                            buffer: s.u16_at(arg, l::MaterialShaderArgument::buffer)?,
                            u: s.u32_at(arg, l::MaterialShaderArgument::u)?,
                        });
                    }
                }
                passes.push(PassRef {
                    vertex_shader,
                    pixel_shader,
                    pixel_program,
                    vertex_program,
                    args,
                });
            }
            techniques.push(Some(TechniqueRef {
                name: string_field(s, tech, l::MaterialTechnique::name)?,
                flags: s.u16_at(tech, l::MaterialTechnique::flags)?,
                passes,
            }));
        }
        Ok(TechniqueSetRef {
            name: string_field(s, t, l::MaterialTechniqueSet::name)?,
            world_vert_format: s.u8_at(t, l::MaterialTechniqueSet::worldVertFormat)?,
            techniques,
        })
    }

    fn read_xmodel(&mut self, s: &ZoneStream<'_>, x: Ptr) -> Result<XModelRef> {
        let numsurfs = s.u8_at(x, l::XModel::numsurfs)?;
        let mut materials = Vec::with_capacity(numsurfs as usize);
        if let Some(handles) = deref(s, x, l::XModel::materialHandles)? {
            for i in 0..numsurfs as usize {
                materials.push(self.asset_at(s, handles.at(i * 4))?);
            }
        }
        let lod_info = x.at(l::XModel::lodInfo);
        let lod_surfs = usize::from(s.u16_at(lod_info, l::XModelLodInfo::numsurfs)?);
        let lod_first = usize::from(s.u16_at(lod_info, l::XModelLodInfo::surfIndex)?);
        let mut lod0 = Vec::with_capacity(lod_surfs);
        if let Some(surfs) = deref(s, x, l::XModel::surfs)? {
            for j in lod_first..(lod_first + lod_surfs).min(usize::from(numsurfs)) {
                let sf = surfs.at(j * l::XSurface::SIZE);
                let vert_count = s.u16_at(sf, l::XSurface::vertCount)?;
                let tri_count = usize::from(s.u16_at(sf, l::XSurface::triCount)?);
                let verts = bytes_field(
                    s,
                    sf,
                    l::XSurface::verts0,
                    usize::from(vert_count) * l::GfxPackedVertex::SIZE,
                )?;
                let mut indices = Vec::with_capacity(tri_count * 3);
                if let Some(t) = deref(s, sf, l::XSurface::triIndices)? {
                    for k in 0..tri_count * 3 {
                        indices.push(s.u16_at(t, k * 2)?);
                    }
                }
                let list_count = usize::from(s.u8_at(sf, l::XSurface::vertListCount)?);
                let mut vert_lists = Vec::with_capacity(list_count);
                if let Some(vl) = deref(s, sf, l::XSurface::vertList)? {
                    for k in 0..list_count {
                        let e = vl.at(k * l::XRigidVertList::SIZE);
                        vert_lists.push((
                            s.u16_at(e, l::XRigidVertList::boneOffset)?,
                            s.u16_at(e, l::XRigidVertList::vertCount)?,
                            s.u16_at(e, l::XRigidVertList::triOffset)?,
                            s.u16_at(e, l::XRigidVertList::triCount)?,
                        ));
                    }
                }
                let vi = sf.at(l::XSurface::vertInfo);
                let mut blend_counts = [0i16; 4];
                for (k, c) in blend_counts.iter_mut().enumerate() {
                    *c = s.i16_at(vi, l::XSurfaceVertexInfo::vertCount + k * 2)?;
                }
                // Words per vertex with k+1 influences: 1 + 2k.
                let blend_words: usize = blend_counts
                    .iter()
                    .enumerate()
                    .map(|(k, &c)| c.max(0) as usize * (1 + 2 * k))
                    .sum();
                let mut blend = Vec::new();
                if blend_words > 0
                    && let Some(b) = deref(s, vi, l::XSurfaceVertexInfo::vertsBlend)?
                {
                    blend.reserve(blend_words);
                    for k in 0..blend_words {
                        blend.push(s.u16_at(b, k * 2)?);
                    }
                }
                let mut part_bits = [0u32; 6];
                for (k, w) in part_bits.iter_mut().enumerate() {
                    *w = s.u32_at(sf, l::XSurface::partBits + k * 4)?;
                }
                lod0.push(XSurfaceRef {
                    verts,
                    vert_count,
                    indices,
                    vert_lists,
                    flags: s.u8_at(sf, l::XSurface::flags)?,
                    material: materials.get(j).copied().flatten(),
                    blend_counts,
                    blend,
                    part_bits,
                });
            }
        }
        // The skeleton.
        let num_bones = usize::from(s.u8_at(x, l::XModel::numBones)?);
        let num_root_bones = s.u8_at(x, l::XModel::numRootBones)?;
        let num_child = num_bones.saturating_sub(usize::from(num_root_bones));
        let mut bone_names = Vec::with_capacity(num_bones);
        if let Some(arr) = deref(s, x, l::XModel::boneNames)? {
            for i in 0..num_bones {
                let id = s.u16_at(arr, i * 2)?;
                bone_names.push(
                    self.strings
                        .get(usize::from(id))
                        .cloned()
                        .unwrap_or_default(),
                );
            }
        }
        let mut parent_list = Vec::with_capacity(num_child);
        if let Some(arr) = deref(s, x, l::XModel::parentList)? {
            for i in 0..num_child {
                parent_list.push(s.u8_at(arr, i)?);
            }
        }
        let mut quats = Vec::with_capacity(num_child);
        if let Some(arr) = deref(s, x, l::XModel::quats)? {
            for i in 0..num_child {
                let q = arr.at(i * l::XModelQuat::SIZE);
                quats.push([
                    s.i16_at(q, 0)?,
                    s.i16_at(q, 2)?,
                    s.i16_at(q, 4)?,
                    s.i16_at(q, 6)?,
                ]);
            }
        }
        // Three floats per bone, packed (measured: the HAMR's tag_flash sits
        // 32.95 ahead of j_gun in `baseMat` and at floats 9..11 here).
        let mut trans = Vec::with_capacity(num_child);
        if let Some(arr) = deref(s, x, l::XModel::trans)? {
            for i in 0..num_child {
                trans.push(vec3(s, arr, i * 12)?);
            }
        }
        let mut base_mat = Vec::with_capacity(num_bones);
        if let Some(arr) = deref(s, x, l::XModel::baseMat)? {
            for i in 0..num_bones {
                let m = arr.at(i * l::DObjAnimMat::SIZE);
                base_mat.push((
                    [
                        s.f32_at(m, 0)?,
                        s.f32_at(m, 4)?,
                        s.f32_at(m, 8)?,
                        s.f32_at(m, 12)?,
                    ],
                    vec3(s, m, l::DObjAnimMat::trans)?,
                ));
            }
        }
        let mut part_classification = Vec::with_capacity(num_bones);
        if let Some(arr) = deref(s, x, l::XModel::partClassification)? {
            for i in 0..num_bones {
                part_classification.push(s.u8_at(arr, i)?);
            }
        }
        let mut bone_info = Vec::with_capacity(num_bones);
        if let Some(arr) = deref(s, x, l::XModel::boneInfo)? {
            for i in 0..num_bones {
                let b = arr.at(i * l::XBoneInfo::SIZE);
                let mins = vec3(s, b, l::XBoneInfo::bounds)?;
                let maxs = vec3(s, b, l::XBoneInfo::bounds + 12)?;
                let r2 = s.f32_at(b, l::XBoneInfo::radiusSquared)?;
                let ok = r2 > 0.0
                    && r2.is_finite()
                    && (0..3).all(|k| mins[k].is_finite() && maxs[k].is_finite() && mins[k] <= maxs[k]);
                bone_info.push(ok.then_some((mins, maxs, r2)));
            }
        }
        let mut coll_surfs = Vec::new();
        let coll_count = s.i32_at(x, l::XModel::numCollSurfs)?.clamp(0, 4096) as usize;
        if let Some(arr) = deref(s, x, l::XModel::collSurfs)? {
            for i in 0..coll_count {
                let cs = arr.at(i * l::XModelCollSurf_s::SIZE);
                let tri_count =
                    s.i32_at(cs, l::XModelCollSurf_s::numCollTris)?.clamp(0, 1 << 20) as usize;
                let mut tris = Vec::with_capacity(tri_count);
                if let Some(t) = deref(s, cs, l::XModelCollSurf_s::collTris)? {
                    for j in 0..tri_count {
                        let row = t.at(j * l::XModelCollTri_s::SIZE);
                        let v4 = |off: usize| -> Result<[f32; 4]> {
                            Ok([
                                s.f32_at(row, off)?,
                                s.f32_at(row, off + 4)?,
                                s.f32_at(row, off + 8)?,
                                s.f32_at(row, off + 12)?,
                            ])
                        };
                        tris.push([
                            v4(l::XModelCollTri_s::plane)?,
                            v4(l::XModelCollTri_s::svec)?,
                            v4(l::XModelCollTri_s::tvec)?,
                        ]);
                    }
                }
                coll_surfs.push(XModelCollSurfRef {
                    tris,
                    mins: vec3(s, cs, l::XModelCollSurf_s::mins)?,
                    maxs: vec3(s, cs, l::XModelCollSurf_s::maxs)?,
                    bone_idx: s.i32_at(cs, l::XModelCollSurf_s::boneIdx)?,
                    contents: s.u32_at(cs, l::XModelCollSurf_s::contents)?,
                    surf_flags: s.u32_at(cs, l::XModelCollSurf_s::surfFlags)?,
                });
            }
        }
        let mut lods = Vec::with_capacity(4);
        for i in 0..4 {
            let li = x.at(l::XModel::lodInfo + i * l::XModelLodInfo::SIZE);
            lods.push((
                s.f32_at(li, l::XModelLodInfo::dist)?,
                s.u16_at(li, l::XModelLodInfo::numsurfs)?,
                s.u16_at(li, l::XModelLodInfo::surfIndex)?,
            ));
        }
        Ok(XModelRef {
            name: string_field(s, x, l::XModel::name)?,
            numsurfs,
            materials,
            num_bones: num_bones as u8,
            num_lods: s.u16_at(x, l::XModel::numLods)?,
            mins: vec3(s, x, l::XModel::mins)?,
            maxs: vec3(s, x, l::XModel::maxs)?,
            radius: s.f32_at(x, l::XModel::radius)?,
            lod0,
            bone_names,
            num_root_bones,
            parent_list,
            quats,
            trans,
            base_mat,
            part_classification,
            bone_info,
            lods,
            contents: s.u32_at(x, l::XModel::contents)?,
            coll_lod: s.i16_at(x, l::XModel::collLod)?,
            coll_surfs,
            phys_preset: self
                .asset_at(s, x.at(l::XModel::physPreset))?
                .filter(|k| k.ty == AssetType::PhysPreset),
        })
    }

    fn read_clip(&mut self, s: &ZoneStream<'_>, c: Ptr) -> Result<ClipRef> {
        let info = c.at(l::clipMap_t::info);
        let mut materials = Vec::new();
        let material_count = s.u32_at(info, l::ClipInfo::numMaterials)? as usize;
        if let Some(arr) = deref(s, info, l::ClipInfo::materials)? {
            for i in 0..material_count {
                let m = arr.at(i * l::ClipMaterial::SIZE);
                materials.push(ClipMaterialRef {
                    name: string_field(s, m, l::ClipMaterial::name)?,
                    surface_flags: s.i32_at(m, l::ClipMaterial::surfaceFlags)?,
                    content_flags: s.i32_at(m, l::ClipMaterial::contentFlags)?,
                });
            }
        }

        let mut brushes = Vec::new();
        let brush_count = s.u16_at(info, l::ClipInfo::numBrushes)? as usize;
        if let Some(arr) = deref(s, info, l::ClipInfo::brushes)? {
            for i in 0..brush_count {
                let b = arr.at(i * l::cbrush_t::SIZE);
                let mut axial_cflags = [[0i32; 3]; 2];
                let mut axial_sflags = [[0i32; 3]; 2];
                for side in 0..2 {
                    for axis in 0..3 {
                        let o = (side * 3 + axis) * 4;
                        axial_cflags[side][axis] = s.i32_at(b, l::cbrush_t::axial_cflags + o)?;
                        axial_sflags[side][axis] = s.i32_at(b, l::cbrush_t::axial_sflags + o)?;
                    }
                }
                let numsides = s.u32_at(b, l::cbrush_t::numsides)? as usize;
                let mut sides = Vec::with_capacity(numsides);
                if let Some(arr) = deref(s, b, l::cbrush_t::sides)? {
                    for j in 0..numsides {
                        let side = arr.at(j * l::cbrushside_t::SIZE);
                        if let Some(plane) = deref(s, side, l::cbrushside_t::plane)? {
                            sides.push((
                                vec3(s, plane, l::cplane_s::normal)?,
                                s.f32_at(plane, l::cplane_s::dist)?,
                                s.i32_at(side, l::cbrushside_t::sflags)?,
                            ));
                        }
                    }
                }
                brushes.push(ClipBrushRef {
                    mins: vec3(s, b, l::cbrush_t::mins)?,
                    maxs: vec3(s, b, l::cbrush_t::maxs)?,
                    contents: s.i32_at(b, l::cbrush_t::contents)?,
                    axial_cflags,
                    axial_sflags,
                    sides,
                });
            }
        }

        let vert_count = s.u32_at(c, l::clipMap_t::vertCount)? as usize;
        let mut verts = Vec::with_capacity(vert_count);
        if let Some(arr) = deref(s, c, l::clipMap_t::verts)? {
            for i in 0..vert_count {
                verts.push(vec3(s, arr, i * 12)?);
            }
        }
        let tri_count = s.u32_at(c, l::clipMap_t::triCount)? as usize;
        let mut tri_indices = Vec::with_capacity(tri_count * 3);
        if let Some(arr) = deref(s, c, l::clipMap_t::triIndices)? {
            for i in 0..tri_count * 3 {
                tri_indices.push(s.u16_at(arr, i * 2)?);
            }
        }
        let walk_len = (3 * tri_count).div_ceil(32) * 4;
        let tri_edge_is_walkable = bytes_field(s, c, l::clipMap_t::triEdgeIsWalkable, walk_len)?;
        let partition_count = s.u32_at(c, l::clipMap_t::partitionCount)? as usize;
        let mut partitions = Vec::with_capacity(partition_count);
        if let Some(arr) = deref(s, c, l::clipMap_t::partitions)? {
            for i in 0..partition_count {
                let p = arr.at(i * l::CollisionPartition::SIZE);
                partitions.push((
                    s.u8_at(p, l::CollisionPartition::triCount)?,
                    s.i32_at(p, l::CollisionPartition::firstTri)?,
                ));
            }
        }
        let tree_count = s.u32_at(c, l::clipMap_t::aabbTreeCount)? as usize;
        let mut aabb_trees = Vec::with_capacity(tree_count);
        if let Some(arr) = deref(s, c, l::clipMap_t::aabbTrees)? {
            for i in 0..tree_count {
                let t = arr.at(i * l::CollisionAabbTree::SIZE);
                aabb_trees.push(ClipAabbRef {
                    origin: vec3(s, t, l::CollisionAabbTree::origin)?,
                    half_size: vec3(s, t, l::CollisionAabbTree::halfSize)?,
                    material_index: s.u16_at(t, l::CollisionAabbTree::materialIndex)?,
                    child_count: s.u16_at(t, l::CollisionAabbTree::childCount)?,
                    u: s.i32_at(t, l::CollisionAabbTree::u)?,
                });
            }
        }

        let node_count = s.u32_at(c, l::clipMap_t::numNodes)? as usize;
        let mut nodes = Vec::with_capacity(node_count);
        if let Some(arr) = deref(s, c, l::clipMap_t::nodes)? {
            for i in 0..node_count {
                let n = arr.at(i * l::cNode_t::SIZE);
                let plane = match deref(s, n, l::cNode_t::plane)? {
                    Some(p) => {
                        let normal = vec3(s, p, l::cplane_s::normal)?;
                        [
                            normal[0],
                            normal[1],
                            normal[2],
                            s.f32_at(p, l::cplane_s::dist)?,
                        ]
                    }
                    None => [0.0; 4],
                };
                nodes.push(ClipNodeRef {
                    plane,
                    children: [
                        s.i16_at(n, l::cNode_t::children)?,
                        s.i16_at(n, l::cNode_t::children + 2)?,
                    ],
                });
            }
        }
        let leaf_count = s.u32_at(c, l::clipMap_t::numLeafs)? as usize;
        let mut leaves = Vec::with_capacity(leaf_count);
        if let Some(arr) = deref(s, c, l::clipMap_t::leafs)? {
            for i in 0..leaf_count {
                leaves.push(read_leaf(s, arr.at(i * l::cLeaf_s::SIZE))?);
            }
        }
        let lb_count = s.u32_at(info, l::ClipInfo::leafbrushNodesCount)? as usize;
        let mut leafbrush_nodes = Vec::with_capacity(lb_count);
        if let Some(arr) = deref(s, info, l::ClipInfo::leafbrushNodes)? {
            for i in 0..lb_count {
                let n = arr.at(i * l::cLeafBrushNode_s::SIZE);
                let data = n.at(l::cLeafBrushNode_s::data);
                let leaf_brush_count = s.i16_at(n, l::cLeafBrushNode_s::leafBrushCount)?;
                let mut brushes = Vec::new();
                let mut child_offset = [0u16; 2];
                if leaf_brush_count > 0 {
                    if let Some(b) = deref(s, data, l::cLeafBrushNodeLeaf_t::brushes)? {
                        for j in 0..leaf_brush_count as usize {
                            brushes.push(s.u16_at(b, j * 2)?);
                        }
                    }
                } else {
                    let o = l::cLeafBrushNodeChildren_t::childOffset;
                    child_offset = [s.u16_at(data, o)?, s.u16_at(data, o + 2)?];
                }
                leafbrush_nodes.push(ClipLeafBrushNodeRef {
                    leaf_brush_count,
                    brushes,
                    child_offset,
                });
            }
        }
        let model_count = s.u32_at(c, l::clipMap_t::numSubModels)? as usize;
        let mut cmodels = Vec::with_capacity(model_count);
        if let Some(arr) = deref(s, c, l::clipMap_t::cmodels)? {
            for i in 0..model_count {
                let m = arr.at(i * l::cmodel_t::SIZE);
                let shares_map_info = match deref(s, m, l::cmodel_t::info)? {
                    Some(p) => p == info,
                    None => true,
                };
                cmodels.push(ClipCmodelRef {
                    mins: vec3(s, m, l::cmodel_t::mins)?,
                    maxs: vec3(s, m, l::cmodel_t::maxs)?,
                    radius: s.f32_at(m, l::cmodel_t::radius)?,
                    leaf: read_leaf(s, m.at(l::cmodel_t::leaf))?,
                    shares_map_info,
                });
            }
        }
        let mut brush_contents = Vec::with_capacity(brush_count);
        if let Some(arr) = deref(s, info, l::ClipInfo::brushContents)? {
            for i in 0..brush_count {
                brush_contents.push(s.i32_at(arr, i * 4)?);
            }
        }
        let static_count = s.u32_at(c, l::clipMap_t::numStaticModels)? as usize;
        let mut static_models = Vec::with_capacity(static_count);
        if let Some(arr) = deref(s, c, l::clipMap_t::staticModelList)? {
            for i in 0..static_count {
                let row = arr.at(i * l::cStaticModel_s::SIZE);
                let axis = l::cStaticModel_s::invScaledAxis;
                static_models.push(ClipStaticModelRef {
                    model: self.asset_at(s, row.at(l::cStaticModel_s::xmodel))?,
                    contents: s.u32_at(row, l::cStaticModel_s::contents)?,
                    origin: vec3(s, row, l::cStaticModel_s::origin)?,
                    inv_scaled_axis: [
                        vec3(s, row, axis)?,
                        vec3(s, row, axis + 12)?,
                        vec3(s, row, axis + 24)?,
                    ],
                    absmin: vec3(s, row, l::cStaticModel_s::absmin)?,
                    absmax: vec3(s, row, l::cStaticModel_s::absmax)?,
                });
            }
        }
        Ok(ClipRef {
            name: string_field(s, c, l::clipMap_t::name)?,
            brushes,
            materials,
            verts,
            tri_indices,
            tri_edge_is_walkable,
            partitions,
            aabb_trees,
            static_model_count: s.u32_at(c, l::clipMap_t::numStaticModels)?,
            nodes,
            leaves,
            leafbrush_nodes,
            cmodels,
            brush_contents,
            static_models,
        })
    }

    fn read_world(&mut self, s: &ZoneStream<'_>, w: Ptr) -> Result<WorldRef> {
        let draw = w.at(l::GfxWorld::draw);
        let vertex_count = s.u32_at(draw, l::GfxWorldDraw::vertexCount)?;
        let size0 = s.u32_at(draw, l::GfxWorldDraw::vertexDataSize0)? as usize;
        let size1 = s.u32_at(draw, l::GfxWorldDraw::vertexDataSize1)? as usize;
        let vd0 = bytes_field(
            s,
            draw.at(l::GfxWorldDraw::vd0),
            l::GfxWorldVertexData0::data,
            size0,
        )?;
        let vd1 = bytes_field(
            s,
            draw.at(l::GfxWorldDraw::vd1),
            l::GfxWorldVertexData1::data,
            size1,
        )?;
        let index_count = s.u32_at(draw, l::GfxWorldDraw::indexCount)? as usize;
        let index_bytes = bytes_field(s, draw, l::GfxWorldDraw::indices, index_count * 2)?;
        let indices = index_bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();

        let mut lightmaps = Vec::new();
        let lightmap_count = s.i32_at(draw, l::GfxWorldDraw::lightmapCount)?.max(0) as usize;
        if let Some(arr) = deref(s, draw, l::GfxWorldDraw::lightmaps)? {
            for i in 0..lightmap_count {
                let e = arr.at(i * l::GfxLightmapArray::SIZE);
                lightmaps.push([
                    self.asset_at(s, e.at(l::GfxLightmapArray::primary))?,
                    self.asset_at(s, e.at(l::GfxLightmapArray::secondary))?,
                ]);
            }
        }

        let mut reflection_probes = Vec::new();
        let probe_count = s.u32_at(draw, l::GfxWorldDraw::reflectionProbeCount)? as usize;
        if let Some(arr) = deref(s, draw, l::GfxWorldDraw::reflectionProbes)? {
            for i in 0..probe_count {
                let p = arr.at(i * l::GfxReflectionProbe::SIZE);
                let mut lighting_sh = [[0.0f32; 4]; 3];
                for (k, row) in lighting_sh.iter_mut().enumerate() {
                    for (c, v) in row.iter_mut().enumerate() {
                        *v = s.f32_at(p, l::GfxReflectionProbe::lightingSH + k * 16 + c * 4)?;
                    }
                }
                reflection_probes.push(ReflectionProbeRef {
                    origin: vec3(s, p, l::GfxReflectionProbe::origin)?,
                    lighting_sh,
                    image: self.asset_at(s, p.at(l::GfxReflectionProbe::reflectionImage))?,
                    mip_lod_bias: s.f32_at(p, l::GfxReflectionProbe::mipLodBias)?,
                });
            }
        }

        let dpvs = w.at(l::GfxWorld::dpvs);
        let surface_count = s.u32_at(w, l::GfxWorld::surfaceCount)? as usize;
        let mut surfaces = Vec::with_capacity(surface_count);
        if let Some(arr) = deref(s, dpvs, l::GfxWorldDpvsStatic::surfaces)? {
            for i in 0..surface_count {
                let g = arr.at(i * l::GfxSurface::SIZE);
                let t = g.at(l::GfxSurface::tris);
                surfaces.push(WorldSurface {
                    mins: vec3(s, t, l::srfTriangles_t::mins)?,
                    maxs: vec3(s, t, l::srfTriangles_t::maxs)?,
                    bounds: [
                        vec3(s, g, l::GfxSurface::bounds)?,
                        vec3(s, g, l::GfxSurface::bounds + 12)?,
                    ],
                    vertex_data_offset0: s.i32_at(t, l::srfTriangles_t::vertexDataOffset0)?,
                    vertex_data_offset1: s.i32_at(t, l::srfTriangles_t::vertexDataOffset1)?,
                    first_vertex: s.i32_at(t, l::srfTriangles_t::firstVertex)?,
                    vertex_count: s.u16_at(t, l::srfTriangles_t::vertexCount)?,
                    tri_count: s.u16_at(t, l::srfTriangles_t::triCount)?,
                    base_index: s.i32_at(t, l::srfTriangles_t::baseIndex)?,
                    material: self.asset_at(s, g.at(l::GfxSurface::material))?,
                    lightmap_index: s.u8_at(g, l::GfxSurface::lightmapIndex)? as i8,
                    reflection_probe_index: s.u8_at(g, l::GfxSurface::reflectionProbeIndex)? as i8,
                    primary_light_index: s.u8_at(g, l::GfxSurface::primaryLightIndex)? as i8,
                    flags: s.u8_at(g, l::GfxSurface::flags)?,
                });
            }
        }

        let smodel_count = s.u32_at(dpvs, l::GfxWorldDpvsStatic::smodelCount)? as usize;
        let mut static_models = Vec::with_capacity(smodel_count);
        if let Some(arr) = deref(s, dpvs, l::GfxWorldDpvsStatic::smodelDrawInsts)? {
            for i in 0..smodel_count {
                let d = arr.at(i * l::GfxStaticModelDrawInst::SIZE);
                let p = d.at(l::GfxStaticModelDrawInst::placement);
                let axis = p.at(l::GfxPackedPlacement::axis);
                let info = d.at(l::GfxStaticModelDrawInst::lmapVertexInfo);
                let count =
                    s.u32_at(info, l::GfxStaticModelLmapVertexInfo::numLmapVertexColors)? as usize;
                let mut lmap_colors = Vec::with_capacity(count);
                if let Some(colors) =
                    deref(s, info, l::GfxStaticModelLmapVertexInfo::lmapVertexColors)?
                {
                    for k in 0..count {
                        lmap_colors.push(s.u32_at(colors, k * 4)?);
                    }
                }
                let sh = d.at(l::GfxStaticModelDrawInst::lightingSH);
                let mut lighting_sh = [0u16; 12];
                for (k, v) in lighting_sh.iter_mut().enumerate() {
                    *v = s.u16_at(sh, k * 2)?;
                }
                static_models.push(StaticModel {
                    origin: vec3(s, p, l::GfxPackedPlacement::origin)?,
                    axis: [vec3(s, axis, 0)?, vec3(s, axis, 12)?, vec3(s, axis, 24)?],
                    scale: s.f32_at(p, l::GfxPackedPlacement::scale)?,
                    model: self.asset_at(s, d.at(l::GfxStaticModelDrawInst::model))?,
                    flags: s.i32_at(d, l::GfxStaticModelDrawInst::flags)?,
                    lmap_colors,
                    lighting_sh,
                    colors_index: s.u16_at(d, l::GfxStaticModelDrawInst::colorsIndex)?,
                    primary_light_index: s
                        .u8_at(d, l::GfxStaticModelDrawInst::primaryLightIndex)?,
                    reflection_probe_index: s
                        .u8_at(d, l::GfxStaticModelDrawInst::reflectionProbeIndex)?,
                    cull_dist: s.f32_at(d, l::GfxStaticModelDrawInst::cullDist)?,
                });
            }
        }

        let range = |b: usize, e: usize| -> Result<(u32, u32)> {
            Ok((s.u32_at(dpvs, b)?, s.u32_at(dpvs, e)?))
        };
        let dpvs_ranges = DpvsRanges {
            static_surface_count: s.u32_at(dpvs, l::GfxWorldDpvsStatic::staticSurfaceCount)?,
            lit: range(
                l::GfxWorldDpvsStatic::litSurfsBegin,
                l::GfxWorldDpvsStatic::litSurfsEnd,
            )?,
            lit_trans: range(
                l::GfxWorldDpvsStatic::litTransSurfsBegin,
                l::GfxWorldDpvsStatic::litTransSurfsEnd,
            )?,
            emissive_opaque: range(
                l::GfxWorldDpvsStatic::emissiveOpaqueSurfsBegin,
                l::GfxWorldDpvsStatic::emissiveOpaqueSurfsEnd,
            )?,
            emissive_trans: range(
                l::GfxWorldDpvsStatic::emissiveTransSurfsBegin,
                l::GfxWorldDpvsStatic::emissiveTransSurfsEnd,
            )?,
        };
        let brush_model_count = s.u32_at(w, l::GfxWorld::modelCount)? as usize;
        let mut brush_models = Vec::with_capacity(brush_model_count);
        if let Some(arr) = deref(s, w, l::GfxWorld::models)? {
            for i in 0..brush_model_count.min(4096) {
                let m = arr.at(i * l::GfxBrushModel::SIZE);
                brush_models.push((
                    s.u32_at(m, l::GfxBrushModel::startSurfIndex)?,
                    s.u32_at(m, l::GfxBrushModel::surfaceCount)?,
                    [
                        vec3(s, m, l::GfxBrushModel::bounds)?,
                        vec3(s, m, l::GfxBrushModel::bounds + 12)?,
                    ],
                ));
            }
        }
        let sorted_count = dpvs_ranges.static_surface_count as usize;
        let mut sorted_surf_index = Vec::with_capacity(sorted_count);
        if let Some(arr) = deref(s, dpvs, l::GfxWorldDpvsStatic::sortedSurfIndex)? {
            for i in 0..sorted_count {
                sorted_surf_index.push(s.u16_at(arr, i * 2)?);
            }
        }
        let cell_count = s.u32_at(
            w.at(l::GfxWorld::dpvsPlanes),
            l::GfxWorldDpvsPlanes::cellCount,
        )? as usize;
        let mut cells = Vec::with_capacity(cell_count);
        if let Some(arr) = deref(s, w, l::GfxWorld::cells)? {
            for i in 0..cell_count {
                let cell = arr.at(i * l::GfxCell::SIZE);
                let tree_count = s.i32_at(cell, l::GfxCell::aabbTreeCount)?.max(0) as usize;
                let mut trees = Vec::with_capacity(tree_count);
                if let Some(t) = deref(s, cell, l::GfxCell::aabbTree)? {
                    for j in 0..tree_count {
                        let n = t.at(j * l::GfxAabbTree::SIZE);
                        let smodel_count =
                            usize::from(s.u16_at(n, l::GfxAabbTree::smodelIndexCount)?);
                        let mut smodel_indexes = Vec::with_capacity(smodel_count);
                        if let Some(ix) = deref(s, n, l::GfxAabbTree::smodelIndexes)? {
                            for k in 0..smodel_count {
                                smodel_indexes.push(s.u16_at(ix, k * 2)?);
                            }
                        }
                        trees.push(AabbTreeRef {
                            mins: vec3(s, n, l::GfxAabbTree::mins)?,
                            maxs: vec3(s, n, l::GfxAabbTree::maxs)?,
                            child_count: s.u16_at(n, l::GfxAabbTree::childCount)?,
                            surface_count: s.u16_at(n, l::GfxAabbTree::surfaceCount)?,
                            start_surf_index: s.u16_at(n, l::GfxAabbTree::startSurfIndex)?,
                            smodel_indexes,
                            children_offset: s.i32_at(n, l::GfxAabbTree::childrenOffset)?,
                        });
                    }
                }
                cells.push(trees);
            }
        }

        Ok(WorldRef {
            name: string_field(s, w, l::GfxWorld::name)?,
            mins: vec3(s, w, l::GfxWorld::mins)?,
            maxs: vec3(s, w, l::GfxWorld::maxs)?,
            dpvs: dpvs_ranges,
            sorted_surf_index,
            brush_models,
            cells,
            primary_light_count: s.u32_at(w, l::GfxWorld::primaryLightCount)?,
            init_fog: Some(read_world_fog(
                s,
                w.at(l::GfxWorld::sunParse + l::SunLightParseParams::initWorldFog),
            )?),
            fog_volumes: {
                let n = s.u32_at(w, l::GfxWorld::worldFogVolumeCount)? as usize;
                let mut v = Vec::with_capacity(n);
                if let Some(arr) = deref(s, w, l::GfxWorld::worldFogVolumes)? {
                    for i in 0..n {
                        let e = arr.at(i * l::GfxWorldFogVolume::SIZE);
                        v.push((
                            vec3(s, e, l::GfxWorldFogVolume::mins)?,
                            vec3(s, e, l::GfxWorldFogVolume::maxs)?,
                            s.u32_at(e, l::GfxWorldFogVolume::control)?,
                            s.u32_at(e, l::GfxWorldFogVolume::controlEx)?,
                            read_world_fog(s, e.at(l::GfxWorldFogVolume::volumeWorldFog))?,
                        ));
                    }
                }
                v
            },
            vertex_count,
            vd0,
            vd1,
            indices,
            surfaces,
            static_models,
            lightmaps,
            reflection_probes,
            sky_box_model: string_field(s, w, l::GfxWorld::skyBoxModel)?,
            lut_material: self.asset_at(s, w.at(l::GfxWorld::lutMaterial))?,
            lut_volumes: {
                let n = s.u32_at(w, l::GfxWorld::lutVolumeCount)? as usize;
                let mut v = Vec::with_capacity(n);
                if let Some(arr) = deref(s, w, l::GfxWorld::lutVolumes)? {
                    for i in 0..n {
                        let e = arr.at(i * l::GfxLutVolume::SIZE);
                        v.push((
                            vec3(s, e, l::GfxLutVolume::mins)?,
                            vec3(s, e, l::GfxLutVolume::maxs)?,
                            s.u32_at(e, l::GfxLutVolume::control)?,
                            s.u32_at(e, l::GfxLutVolume::lutTransitionTime)?,
                            s.u32_at(e, l::GfxLutVolume::lutIndex)?,
                        ));
                    }
                }
                v
            },
            sky_dyn_intensity: {
                let d = w.at(l::GfxWorld::skyDynIntensity);
                [
                    s.f32_at(d, l::GfxSkyDynamicIntensity::angle0)?,
                    s.f32_at(d, l::GfxSkyDynamicIntensity::angle1)?,
                    s.f32_at(d, l::GfxSkyDynamicIntensity::factor0)?,
                    s.f32_at(d, l::GfxSkyDynamicIntensity::factor1)?,
                ]
            },
            light_grid: {
                let g = w.at(l::GfxWorld::lightGrid);
                let u16x3 = |off: usize| -> Result<[u16; 3]> {
                    Ok([
                        s.u16_at(g, off)?,
                        s.u16_at(g, off + 2)?,
                        s.u16_at(g, off + 4)?,
                    ])
                };
                let mins = u16x3(l::GfxLightGrid::mins)?;
                let maxs = u16x3(l::GfxLightGrid::maxs)?;
                let row_axis = s.u32_at(g, l::GfxLightGrid::rowAxis)?;
                let rows = if row_axis < 3 {
                    usize::from(maxs[row_axis as usize].saturating_sub(mins[row_axis as usize])) + 1
                } else {
                    0
                };
                let raw_size = s.u32_at(g, l::GfxLightGrid::rawRowDataSize)? as usize;
                let entry_count = s.u32_at(g, l::GfxLightGrid::entryCount)? as usize;
                let color_count = s.u32_at(g, l::GfxLightGrid::colorCount)?;
                let grid = LightGridRef {
                    sun_primary_light_index: s.u32_at(g, l::GfxLightGrid::sunPrimaryLightIndex)?,
                    mins,
                    maxs,
                    offset: s.f32_at(g, l::GfxLightGrid::offset)?,
                    row_axis,
                    col_axis: s.u32_at(g, l::GfxLightGrid::colAxis)?,
                    row_data_start: bytes_field(s, g, l::GfxLightGrid::rowDataStart, rows * 2)?,
                    raw_row_data: bytes_field(s, g, l::GfxLightGrid::rawRowData, raw_size)?,
                    entries: bytes_field(s, g, l::GfxLightGrid::entries, entry_count * 4)?,
                    colors: bytes_field(
                        s,
                        g,
                        l::GfxLightGrid::colors,
                        color_count as usize * l::GfxCompressedLightGridColors::SIZE,
                    )?,
                    color_count,
                    coeffs: {
                        let n = s.u32_at(g, l::GfxLightGrid::coeffCount)? as usize;
                        bytes_field(
                            s,
                            g,
                            l::GfxLightGrid::coeffs,
                            n * l::GfxCompressedLightGridCoeffs::SIZE,
                        )?
                    },
                    coeff_count: s.u32_at(g, l::GfxLightGrid::coeffCount)?,
                    sky_volumes: {
                        let n = s.u32_at(g, l::GfxLightGrid::skyGridVolumeCount)? as usize;
                        let mut v = Vec::with_capacity(n);
                        if let Some(arr) = deref(s, g, l::GfxLightGrid::skyGridVolumes)? {
                            for i in 0..n {
                                let e = arr.at(i * l::GfxSkyGridVolume::SIZE);
                                v.push(SkyGridVolumeRef {
                                    mins: vec3(s, e, l::GfxSkyGridVolume::mins)?,
                                    maxs: vec3(s, e, l::GfxSkyGridVolume::maxs)?,
                                    lighting_origin: vec3(
                                        s,
                                        e,
                                        l::GfxSkyGridVolume::lightingOrigin,
                                    )?,
                                    colors_index: s.u16_at(e, l::GfxSkyGridVolume::colorsIndex)?,
                                    primary_light_index: s
                                        .u8_at(e, l::GfxSkyGridVolume::primaryLightIndex)?,
                                    visibility: s.u8_at(e, l::GfxSkyGridVolume::visibility)?,
                                });
                            }
                        }
                        v
                    },
                };
                Some(grid)
            },
            init_sun_exposure: s.f32_at(
                w,
                l::GfxWorld::sunParse
                    + l::SunLightParseParams::initWorldSun
                    + l::GfxWorldSun::exposure,
            )?,
            exposure_volumes: {
                let n = s.u32_at(w, l::GfxWorld::exposureVolumeCount)? as usize;
                let mut v = Vec::with_capacity(n);
                if let Some(arr) = deref(s, w, l::GfxWorld::exposureVolumes)? {
                    for i in 0..n {
                        let e = arr.at(i * l::GfxExposureVolume::SIZE);
                        v.push([
                            s.f32_at(e, l::GfxExposureVolume::exposure)?,
                            s.f32_at(e, l::GfxExposureVolume::luminanceIncreaseScale)?,
                            s.f32_at(e, l::GfxExposureVolume::luminanceDecreaseScale)?,
                            s.f32_at(e, l::GfxExposureVolume::featherRange)?,
                        ]);
                    }
                }
                v
            },
            sun: match deref(s, w, l::GfxWorld::sunLight)? {
                Some(light) => Some(SunRef {
                    color: vec3(s, light, l::GfxLight::color)?,
                    dir: vec3(s, light, l::GfxLight::dir)?,
                    diffuse_color: [
                        s.f32_at(light, l::GfxLight::diffuseColor)?,
                        s.f32_at(light, l::GfxLight::diffuseColor + 4)?,
                        s.f32_at(light, l::GfxLight::diffuseColor + 8)?,
                        s.f32_at(light, l::GfxLight::diffuseColor + 12)?,
                    ],
                }),
                None => None,
            },
        })
    }
}

impl WalkSink for ZoneCapture {
    fn asset_loaded(&mut self, s: &ZoneStream<'_>, loaded: Loaded) -> Result<()> {
        *self.loads.entry(loaded.ty).or_default() += 1;
        self.parent_started = loaded.started;
        let body = loaded.body;
        let key = match loaded.ty {
            AssetType::Image => {
                let image = read_image(s, body)?;
                self.images.push(image);
                Some(self.images.len() - 1)
            }
            AssetType::Material => {
                let material = self.read_material(s, body)?;
                self.materials.push(material);
                Some(self.materials.len() - 1)
            }
            AssetType::TechniqueSet => {
                let set = self.read_technique_set(s, body)?;
                self.technique_sets.push(set);
                Some(self.technique_sets.len() - 1)
            }
            AssetType::XModel => {
                let model = self.read_xmodel(s, body)?;
                self.xmodels.push(model);
                Some(self.xmodels.len() - 1)
            }
            AssetType::GfxWorld => {
                self.world = Some(self.read_world(s, body)?);
                None
            }
            AssetType::ClipMap | AssetType::ClipMapPvs => {
                self.clip = Some(self.read_clip(s, body)?);
                None
            }
            AssetType::XAnimParts => {
                let numframes = s.u16_at(body, l::XAnimParts::numframes)?;
                let mut delta_trans = Vec::new();
                if let Some(dp) = deref(s, body, l::XAnimParts::deltaPart)?
                    && let Some(t) = deref(s, dp, l::XAnimDeltaPart::trans)?
                {
                    let size = usize::from(s.u16_at(t, l::XAnimPartTrans::size)?);
                    let small = s.u8_at(t, l::XAnimPartTrans::smallTrans)? != 0;
                    let u = t.at(l::XAnimPartTrans::u);
                    if size == 0 {
                        delta_trans.push((0, vec3(s, u, l::XAnimPartTransData::frame0)?));
                    } else {
                        let mins = vec3(s, u, l::XAnimPartTransFrames::mins)?;
                        let span = vec3(s, u, l::XAnimPartTransFrames::size)?;
                        let keys = size + 1;
                        let frames = deref(s, u, l::XAnimPartTransFrames::frames)?;
                        let idx = u.at(l::XAnimPartTransFrames::indices);
                        for k in 0..keys {
                            let frame = if keys == usize::from(numframes) + 1 {
                                k as u16
                            } else if numframes >= 256 {
                                s.u16_at(idx, k * 2)?
                            } else {
                                u16::from(s.u8_at(idx, k)?)
                            };
                            let Some(f) = frames else { break };
                            // mins + size * the raw quantized value (size is
                            // the step per unit, as in the IW channels).
                            let v = |axis: usize| -> Result<f32> {
                                Ok(if small {
                                    f32::from(s.u8_at(f, k * 3 + axis)?)
                                } else {
                                    f32::from(s.u16_at(f, (k * 3 + axis) * 2)?)
                                })
                            };
                            delta_trans.push((
                                frame,
                                [
                                    mins[0] + span[0] * v(0)?,
                                    mins[1] + span[1] * v(1)?,
                                    mins[2] + span[2] * v(2)?,
                                ],
                            ));
                        }
                    }
                }
                let mut bone_count = [0u8; 10];
                for (k, c) in bone_count.iter_mut().enumerate() {
                    *c = s.u8_at(body, l::XAnimParts::boneCount + k)?;
                }
                let mut names = Vec::with_capacity(usize::from(bone_count[9]));
                if let Some(arr) = deref(s, body, l::XAnimParts::names)? {
                    for k in 0..usize::from(bone_count[9]) {
                        let id = s.u16_at(arr, k * 2)?;
                        names.push(self.strings.get(usize::from(id)).cloned().unwrap_or_default());
                    }
                }
                let notify_count = usize::from(s.u8_at(body, l::XAnimParts::notifyCount)?);
                let mut notifies = Vec::with_capacity(notify_count);
                if let Some(arr) = deref(s, body, l::XAnimParts::notify)? {
                    for k in 0..notify_count {
                        let e = arr.at(k * l::XAnimNotifyInfo::SIZE);
                        let id = s.u16_at(e, l::XAnimNotifyInfo::name)?;
                        notifies.push((
                            self.strings.get(usize::from(id)).cloned().unwrap_or_default(),
                            s.f32_at(e, l::XAnimNotifyInfo::time)?,
                        ));
                    }
                }
                let bytes = |field: usize, n: usize| -> Result<Vec<u8>> {
                    Ok(match deref(s, body, field)? {
                        Some(p) if n > 0 => s.slice_at(p, 0, n)?.to_vec(),
                        _ => Vec::new(),
                    })
                };
                let shorts = |field: usize, n: usize| -> Result<Vec<u16>> {
                    let raw = bytes(field, n * 2)?;
                    Ok(raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect())
                };
                let ints = |field: usize, n: usize| -> Result<Vec<u32>> {
                    let raw = bytes(field, n * 4)?;
                    Ok(raw
                        .chunks_exact(4)
                        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                        .collect())
                };
                let data_byte = bytes(
                    l::XAnimParts::dataByte,
                    usize::from(s.u16_at(body, l::XAnimParts::dataByteCount)?),
                )?;
                let data_short = shorts(
                    l::XAnimParts::dataShort,
                    usize::from(s.u16_at(body, l::XAnimParts::dataShortCount)?),
                )?;
                let data_int = ints(
                    l::XAnimParts::dataInt,
                    usize::from(s.u16_at(body, l::XAnimParts::dataIntCount)?),
                )?;
                let random_data_short = shorts(
                    l::XAnimParts::randomDataShort,
                    s.u32_at(body, l::XAnimParts::randomDataShortCount)? as usize,
                )?;
                let random_data_byte = bytes(
                    l::XAnimParts::randomDataByte,
                    usize::from(s.u16_at(body, l::XAnimParts::randomDataByteCount)?),
                )?;
                let random_data_int = ints(
                    l::XAnimParts::randomDataInt,
                    usize::from(s.u16_at(body, l::XAnimParts::randomDataIntCount)?),
                )?;
                let index_count = s.u32_at(body, l::XAnimParts::indexCount)? as usize;
                let indices = if numframes < 256 {
                    bytes(l::XAnimParts::indices, index_count)?
                        .into_iter()
                        .map(u16::from)
                        .collect()
                } else {
                    shorts(l::XAnimParts::indices, index_count)?
                };
                self.xanims.push(XAnimRef {
                    name: string_field(s, body, l::XAnimParts::name)?,
                    numframes,
                    framerate: s.f32_at(body, l::XAnimParts::framerate)?,
                    looping: s.u8_at(body, l::XAnimParts::bLoop)? != 0,
                    delta: s.u8_at(body, l::XAnimParts::bDelta)? != 0,
                    delta_trans,
                    bone_count,
                    names,
                    notifies,
                    data_byte,
                    data_short,
                    data_int,
                    random_data_byte,
                    random_data_short,
                    random_data_int,
                    indices,
                });
                None
            }
            AssetType::RawFile => {
                let len = s.u32_at(body, l::RawFile::len)? as usize;
                let name = string_field(s, body, l::RawFile::name)?;
                let bytes = bytes_field(s, body, l::RawFile::buffer, len)?;
                self.raw_files.push((name, bytes));
                None
            }
            AssetType::GameWorldMp => {
                // PathData is inline in GameWorldMp (at `path`).
                let pd = body.at(l::GameWorldMp::path);
                let count = s.u32_at(pd, l::PathData::nodeCount)? as usize;
                if let Some(nodes) = deref(s, pd, l::PathData::nodes)? {
                    let sstr = |i: u16| -> String {
                        self.strings.get(usize::from(i)).cloned().unwrap_or_default()
                    };
                    for i in 0..count {
                        let n = nodes.at(i * l::pathnode_t::SIZE);
                        let c = n.at(l::pathnode_t::constant);
                        let link_count = s.i16_at(c, l::pathnode_constant_t::totalLinkCount)?.max(0) as usize;
                        let mut links = Vec::with_capacity(link_count);
                        if let Some(lp) = deref(s, c, l::pathnode_constant_t::Links)? {
                            for k in 0..link_count {
                                let lk = lp.at(k * l::pathlink_s::SIZE);
                                links.push((
                                    s.u16_at(lk, l::pathlink_s::nodeNum)?,
                                    s.f32_at(lk, l::pathlink_s::fDist)?,
                                    s.u8_at(lk, l::pathlink_s::negotiationLink)? != 0,
                                ));
                            }
                        }
                        self.path_nodes.push(PathNodeRef {
                            ty: s.u32_at(c, l::pathnode_constant_t::r#type)?,
                            spawnflags: u32::from(s.u16_at(c, l::pathnode_constant_t::spawnflags)?),
                            targetname: sstr(s.u16_at(c, l::pathnode_constant_t::targetname)?),
                            target: sstr(s.u16_at(c, l::pathnode_constant_t::target)?),
                            script_noteworthy: sstr(s.u16_at(c, l::pathnode_constant_t::script_noteworthy)?),
                            script_linkname: sstr(s.u16_at(c, l::pathnode_constant_t::script_linkName)?),
                            animscript: sstr(s.u16_at(c, l::pathnode_constant_t::animscript)?),
                            origin: vec3(s, c, l::pathnode_constant_t::vOrigin)?,
                            angle: s.f32_at(c, l::pathnode_constant_t::fAngle)?,
                            radius: s.f32_at(c, l::pathnode_constant_t::fRadius)?,
                            links,
                        });
                    }
                }
                None
            }
            AssetType::Localize => {
                let name = string_field(s, body, l::LocalizeEntry::name)?;
                let value = string_field(s, body, l::LocalizeEntry::value)?;
                if !name.is_empty() {
                    self.localize.push((name, value));
                }
                None
            }
            AssetType::FontIcon => {
                // bo2mp: BO2's button pictures (`^BBUTTON_MOUSE_LEFT^`): each icon's
                // name, picture and its size scales, and the aliases that name
                // one icon for another. Published as two tables so the UI
                // reads them like any BO2 table.
                let entries = s.u32_at(body, l::FontIcon::numEntries)? as usize;
                let aliases = s.u32_at(body, l::FontIcon::numAliasEntries)? as usize;
                let mut cells = Vec::with_capacity(entries * 6);
                if let Some(arr) = deref(s, body, l::FontIcon::fontIconEntry)? {
                    for i in 0..entries {
                        let e = arr.at(i * l::FontIconEntry::SIZE);
                        // Each entry's picture was loaded through its own slot while
                        // this icon set was read (TEMP addresses are reused, so
                        // the slot is how it is found, not its address).
                        let material = match self.asset_at(s, e.at(l::FontIconEntry::fontIconMaterialHandle))? {
                            Some(k) => self.material(k).map(|m| m.name.clone()).unwrap_or_default(),
                            None => String::new(),
                        };
                        cells.push(string_field(s, e.at(l::FontIconEntry::fontIconName), l::FontIconName::string)?);
                        cells.push(s.u32_at(e.at(l::FontIconEntry::fontIconName), l::FontIconName::hash)?.to_string());
                        cells.push(material);
                        cells.push(s.i32_at(e, l::FontIconEntry::fontIconSize)?.to_string());
                        cells.push(s.f32_at(e, l::FontIconEntry::xScale)?.to_string());
                        cells.push(s.f32_at(e, l::FontIconEntry::yScale)?.to_string());
                    }
                }
                self.string_tables.push(StringTableRef { name: "bo2mp/fonticon.csv".to_owned(), columns: 6, rows: cells.len() / 6, cells });
                let mut cells = Vec::with_capacity(aliases * 2);
                if let Some(arr) = deref(s, body, l::FontIcon::fontIconAlias)? {
                    for i in 0..aliases {
                        let a = arr.at(i * l::FontIconAlias::SIZE);
                        cells.push(s.u32_at(a, l::FontIconAlias::aliasHash)?.to_string());
                        cells.push(s.u32_at(a, l::FontIconAlias::buttonHash)?.to_string());
                    }
                }
                self.string_tables.push(StringTableRef { name: "bo2mp/fonticonalias.csv".to_owned(), columns: 2, rows: cells.len() / 2, cells });
                None
            }
            AssetType::StringTable => {
                let name = string_field(s, body, l::StringTable::name)?;
                let columns = s.u32_at(body, l::StringTable::columnCount)? as usize;
                let rows = s.u32_at(body, l::StringTable::rowCount)? as usize;
                let mut cells = Vec::with_capacity(columns * rows);
                if let Some(arr) = deref(s, body, l::StringTable::values)? {
                    for i in 0..columns * rows {
                        cells.push(string_field(s, arr.at(i * l::StringTableCell::SIZE), l::StringTableCell::string)?);
                    }
                }
                self.string_tables.push(StringTableRef {
                    name,
                    columns,
                    rows,
                    cells,
                });
                None
            }
            // Compiled scripts are kept with the rawfiles, named `script:`.
            AssetType::ScriptParseTree => {
                let len = s.u32_at(body, l::ScriptParseTree::len)? as usize;
                let name = string_field(s, body, l::ScriptParseTree::name)?;
                let bytes = bytes_field(s, body, l::ScriptParseTree::buffer, len)?;
                self.raw_files.push((format!("script:{name}"), bytes));
                None
            }
            AssetType::ComWorld => {
                let n = s.u32_at(body, l::ComWorld::primaryLightCount)? as usize;
                if let Some(arr) = deref(s, body, l::ComWorld::primaryLights)? {
                    for i in 0..n {
                        let p = arr.at(i * l::ComPrimaryLight::SIZE);
                        let v4 = |off: usize| -> Result<[f32; 4]> {
                            Ok([
                                s.f32_at(p, off)?,
                                s.f32_at(p, off + 4)?,
                                s.f32_at(p, off + 8)?,
                                s.f32_at(p, off + 12)?,
                            ])
                        };
                        self.primary_lights.push(PrimaryLightRef {
                            ty: s.u8_at(p, l::ComPrimaryLight::r#type)?,
                            can_use_shadow_map: s.u8_at(p, l::ComPrimaryLight::canUseShadowMap)?,
                            exponent: s.u8_at(p, l::ComPrimaryLight::exponent)?,
                            priority: s.u8_at(p, l::ComPrimaryLight::priority)?,
                            cull_dist: s.i16_at(p, l::ComPrimaryLight::cullDist)?,
                            color: vec3(s, p, l::ComPrimaryLight::color)?,
                            dir: vec3(s, p, l::ComPrimaryLight::dir)?,
                            origin: vec3(s, p, l::ComPrimaryLight::origin)?,
                            radius: s.f32_at(p, l::ComPrimaryLight::radius)?,
                            cos_half_fov_outer: s.f32_at(p, l::ComPrimaryLight::cosHalfFovOuter)?,
                            cos_half_fov_inner: s.f32_at(p, l::ComPrimaryLight::cosHalfFovInner)?,
                            d_attenuation: s.f32_at(p, l::ComPrimaryLight::dAttenuation)?,
                            roundness: s.f32_at(p, l::ComPrimaryLight::roundness)?,
                            diffuse_color: v4(l::ComPrimaryLight::diffuseColor)?,
                            falloff: v4(l::ComPrimaryLight::falloff)?,
                            angle: v4(l::ComPrimaryLight::angle)?,
                            a_ab_b: v4(l::ComPrimaryLight::aAbB)?,
                        });
                    }
                }
                None
            }
            // bo2zm M2: guns, effects, sounds.
            AssetType::Weapon => {
                let weapon = self.read_weapon(s, body)?;
                self.weapons.push(weapon);
                None
            }
            AssetType::Attachment => {
                let a = self.read_attachment(s, body)?;
                self.attachments.push(a);
                None
            }
            AssetType::AttachmentUnique => {
                let a = self.read_attachment_unique(s, body)?;
                self.attachment_uniques.push(a);
                None
            }
            AssetType::WeaponCamo => {
                let camo = self.read_weapon_camo(s, body)?;
                self.weapon_camos.push(camo);
                Some(self.weapon_camos.len() - 1)
            }
            AssetType::Font => {
                let font = self.read_font(s, body)?;
                self.fonts.push(font);
                None
            }
            AssetType::Fx => {
                let fx = self.read_fx(s, body)?;
                self.fx.push(fx);
                Some(self.fx.len() - 1)
            }
            AssetType::Tracer => {
                let tracer = self.read_tracer(s, body)?;
                self.tracers.push(tracer);
                Some(self.tracers.len() - 1)
            }
            AssetType::ImpactFx => {
                let table = self.read_impact_table(s, body)?;
                self.impact_tables.push(table);
                None
            }
            AssetType::FootstepTable => {
                let table = self.read_footstep_table(s, body)?;
                self.footstep_tables.push(table);
                None
            }
            AssetType::FootstepFxTable => {
                let table = self.read_footstep_fx_table(s, body)?;
                self.footstep_fx_tables.push(table);
                None
            }
            AssetType::Sound => {
                let bank = self.read_sound_bank(s, body)?;
                self.sound_banks.push(bank);
                None
            }
            AssetType::SndDriverGlobals => {
                self.snd_globals = Some(self.read_snd_driver_globals(s, body)?);
                None
            }
            AssetType::VehicleDef => {
                // Its guns are named, not pointed at (`turretWeapon`,
                // `gunnerWeapon[4]` are strings in BO2's vehicle def).
                let name_at = |off: usize| string_field(s, body, off).unwrap_or_default();
                let gunner_weapons = (0..4)
                    .map(|i| name_at(l::VehicleDef::gunnerWeapon + i * 4))
                    .filter(|g| !g.is_empty())
                    .collect();
                let f = |off: usize| s.f32_at(body, off).unwrap_or(0.0);
                let v = l::VehicleDef::nitrousVehParams;
                let tp = l::VehicleDef::thirdPersonCameraHeight;
                let tv = l::VehicleDef::turretViewLimits;
                let drive = VehicleDrive {
                    camera_mode: s.i32_at(body, l::VehicleDef::cameraMode).unwrap_or(0),
                    max_speed: f(l::VehicleDef::maxSpeed),
                    max_speed_vertical: f(l::VehicleDef::maxSpeedVertical),
                    accel: f(l::VehicleDef::accel),
                    accel_vertical: f(l::VehicleDef::accelVertical),
                    reverse_scale: f(v + l::VehicleParameter::m_reverse_scale),
                    camera_range: f(l::VehicleDef::thirdPersonCameraRange),
                    camera_height: f(tp + 4),
                    camera_pitch: [
                        f(l::VehicleDef::thirdPersonCameraMinPitchClamp),
                        f(l::VehicleDef::thirdPersonCameraMaxPitchClamp),
                    ],
                    camera_fov: f(l::VehicleDef::cameraFOV),
                    turret_pitch: [f(tv + 8), f(tv + 12)],
                    third_person_driver: s.i32_at(body, l::VehicleDef::thirdPersonDriver).unwrap_or(0),
                    buttons: [
                        l::VehicleDef::moveUpButtonName,
                        l::VehicleDef::moveDownButtonName,
                        l::VehicleDef::switchSeatButtonName,
                        l::VehicleDef::attackButtonName,
                        l::VehicleDef::attackSecondaryButtonName,
                    ]
                    .map(|o| vehicle_button_command(&name_at(o))),
                };
                self.vehicles.push(VehicleRef {
                    name: name_at(l::VehicleDef::name),
                    turret_weapon: name_at(l::VehicleDef::turretWeapon),
                    gunner_weapons,
                    drive,
                });
                None
            }
            AssetType::PhysPreset => {
                use l::PhysPreset as pp;
                self.phys_presets.push(PhysPresetRef {
                    name: string_field(s, body, pp::name)?,
                    mass: s.f32_at(body, pp::mass)?,
                    bounce: s.f32_at(body, pp::bounce)?,
                    friction: s.f32_at(body, pp::friction)?,
                    bullet_force_scale: s.f32_at(body, pp::bulletForceScale)?,
                    explosive_force_scale: s.f32_at(body, pp::explosiveForceScale)?,
                    pieces_spread_fraction: s.f32_at(body, pp::piecesSpreadFraction)?,
                    pieces_upward_velocity: s.f32_at(body, pp::piecesUpwardVelocity)?,
                    gravity_scale: s.f32_at(body, pp::gravityScale)?,
                    center_of_mass_offset: vec3(s, body, pp::centerOfMassOffset)?,
                });
                Some(self.phys_presets.len() - 1)
            }
            AssetType::PhysConstraints => {
                use l::{PhysConstraint as pc, PhysConstraints as pcs};
                let count = s.u32_at(body, pcs::count)?.min(16) as usize;
                let mut data = Vec::with_capacity(count);
                for i in 0..count {
                    let c = body.at(pcs::data + i * pc::SIZE);
                    data.push(PhysConstraintRef {
                        kind: s.i32_at(c, pc::r#type)?,
                        power: s.f32_at(c, pc::power)?,
                        scale: vec3(s, c, pc::scale)?,
                        spin_scale: s.f32_at(c, pc::spin_scale)?,
                    });
                }
                self.phys_constraints.push(PhysConstraintsRef {
                    name: string_field(s, body, pcs::name)?,
                    data,
                });
                Some(self.phys_constraints.len() - 1)
            }
            AssetType::DestructibleDef => {
                let name = string_field(s, body, l::DestructibleDef::name).unwrap_or_default();
                let model_of = |this: &mut Self, off: usize| -> String {
                    this.asset_at(s, body.at(off))
                        .ok()
                        .flatten()
                        .filter(|k| k.ty == AssetType::XModel)
                        .and_then(|k| this.xmodels.get(k.index))
                        .map(|m| m.name.trim_start_matches(',').to_owned())
                        .unwrap_or_default()
                };
                let model = model_of(self, l::DestructibleDef::model);
                let pristine_model = model_of(self, l::DestructibleDef::pristineModel);
                let count = s.i32_at(body, l::DestructibleDef::numPieces)?.clamp(0, 64) as usize;
                let mut pieces = Vec::with_capacity(count);
                if let Some(first) = deref(s, body, l::DestructibleDef::pieces)? {
                    use l::{DestructiblePiece as dp, DestructibleStage as ds};
                    for i in 0..count {
                        let p = first.at(i * dp::SIZE);
                        let mut stages: [DestructibleStageRef; 5] = Default::default();
                        for (j, stage) in stages.iter_mut().enumerate() {
                            let q = p.at(dp::stages + j * ds::SIZE);
                            let bone = s.u16_at(q, ds::showBone)?;
                            let break_effect = self
                                .asset_at(s, q.at(ds::breakEffect))?
                                .filter(|k| k.ty == AssetType::Fx)
                                .and_then(|k| self.fx.get(k.index))
                                .map(|f| f.name.trim_start_matches(',').to_owned())
                                .unwrap_or_default();
                            let mut spawn_models: [String; 3] = Default::default();
                            for (k, m) in spawn_models.iter_mut().enumerate() {
                                *m = self
                                    .asset_at(s, q.at(ds::spawnModel + k * 4))?
                                    .filter(|k| k.ty == AssetType::XModel)
                                    .and_then(|k| self.xmodels.get(k.index))
                                    .map(|m| m.name.trim_start_matches(',').to_owned())
                                    .unwrap_or_default();
                            }
                            *stage = DestructibleStageRef {
                                show_bone: if bone == 0 {
                                    String::new()
                                } else {
                                    self.strings.get(usize::from(bone)).cloned().unwrap_or_default()
                                },
                                break_health: s.f32_at(q, ds::breakHealth)?,
                                max_time: s.f32_at(q, ds::maxTime)?,
                                flags: s.u32_at(q, ds::flags)?,
                                break_effect,
                                break_sound: string_field(s, q, ds::breakSound)?,
                                break_notify: string_field(s, q, ds::breakNotify)?,
                                loop_sound: string_field(s, q, ds::loopSound)?,
                                spawn_models,
                                phys_preset: s.u32_at(q, ds::physPreset)? != 0,
                            };
                        }
                        let mut hide_bones = [0u32; 5];
                        for (j, word) in hide_bones.iter_mut().enumerate() {
                            *word = s.u32_at(p, dp::hideBones + j * 4)?;
                        }
                        let constraint = self
                            .asset_at(s, p.at(dp::physConstraints))?
                            .filter(|k| k.ty == AssetType::PhysConstraints)
                            .and_then(|k| self.phys_constraints.get(k.index))
                            .and_then(|c| c.data.first().copied());
                        pieces.push(DestructiblePieceRef {
                            stages,
                            parent_piece: s.slice_at(p, dp::parentPiece, 1)?[0],
                            parent_damage_percent: s.f32_at(p, dp::parentDamagePercent)?,
                            bullet_damage_scale: s.f32_at(p, dp::bulletDamageScale)?,
                            explosive_damage_scale: s.f32_at(p, dp::explosiveDamageScale)?,
                            melee_damage_scale: s.f32_at(p, dp::meleeDamageScale)?,
                            health: s.i32_at(p, dp::health)?,
                            hide_bones,
                            constraint,
                        });
                    }
                }
                let client_only = s.i32_at(body, l::DestructibleDef::clientOnly)? != 0;
                self.destructibles.push(DestructibleRef {
                    name,
                    model,
                    pristine_model,
                    pieces,
                    client_only,
                });
                None
            }
            AssetType::MapEnts => {
                let len = s.u32_at(body, l::MapEnts::numEntityChars)? as usize;
                let text = bytes_field(s, body, l::MapEnts::entityString, len)?;
                let end = text.iter().position(|&b| b == 0).unwrap_or(text.len());
                self.map_ents
                    .push(String::from_utf8_lossy(&text[..end]).into_owned());
                None
            }
            _ => None,
        };
        if let Some(index) = key {
            self.keep(
                loaded,
                AssetKey {
                    ty: loaded.ty,
                    index,
                },
            );
        }
        Ok(())
    }
}
