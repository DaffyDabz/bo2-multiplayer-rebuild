//! bo2zm: Black Ops II GfxWorld -> WorldDraw. Not part of upstream IW4L.
//!
//! The T6 world vertex is 36 bytes (measured on Nuketown, t6world probe):
//! position f32x3 @0, binormal sign f32 @12, colour @16, texcoord half2 @20
//! (u in the low half), normal @24 and tangent @28 packed 10:10:10 signed,
//! lightmap coord unorm16 x2 @32 (u low; measured: it spans the whole
//! lightmap atlas, 0..2048 x 1..3070 texels on lightmap 0, where half floats
//! give nonsense). Surfaces come in groups sharing one vertex
//! block: a surface's vertex `i` is `vd0[vertexDataOffset0 + i * 36]`, with
//! `i` from its own triangles (u16, group-relative). Every vertex here is
//! repacked into the 44-byte IW4 world vertex the renderer uploads.
//!
//! Visibility is one synthetic cell holding every surface until the T6 cell
//! and portal tables are read.

use std::collections::HashMap;

use asset_iw4::size as iw4_sz;
use asset_t6::WorldRef;
use dpvs_iw4::{AabbNodeView, Bounds, SurfRange};

use crate::world_draw::{
    CameraRangeKind, CameraSurfRange, CameraSurfRanges, DpvsWorldData, SurfaceDrawFields,
    WorldDraw, WorldLightmap, WorldLightmapGap, WorldSunLight, WorldVertexPayload,
};
use crate::world_mesh::{WorldMeshError, WorldMeshStats};
use crate::{SurfaceCastsSunShadow, world_capture_from_casters};

pub const T6_WORLD_VERTEX: usize = 36;

fn half(bits: u16) -> f32 {
    asset_model::half_to_f32(bits)
}

/// Signed 10:10:10 unit vector (two's complement, /511).
pub fn unpack_t6_unit_vec(packed: u32) -> [f32; 3] {
    let c = |v: u32| {
        let v = (v & 0x3ff) as i32;
        let v = if v >= 512 { v - 1024 } else { v };
        v as f32 / 511.0
    };
    [c(packed), c(packed >> 10), c(packed >> 20)]
}

/// The IW4 packed unit vector: biased bytes over a fixed scale of 1/127.
fn pack_iw4_unit_vec(n: [f32; 3]) -> u32 {
    let b = |v: f32| ((v * 127.0 + 127.5) as i32).clamp(0, 255) as u8;
    u32::from_le_bytes([b(n[0]), b(n[1]), b(n[2]), 63])
}

fn normalized(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 1e-6 {
        [v[0] / l, v[1] / l, v[2] / l]
    } else {
        [0.0, 0.0, 1.0]
    }
}

fn rd_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn rd_f32(b: &[u8], at: usize) -> f32 {
    f32::from_bits(rd_u32(b, at))
}

fn half2(packed: u32) -> [f32; 2] {
    [half(packed as u16), half((packed >> 16) as u16)]
}

struct Decoded {
    packed: [u8; iw4_sz::GFX_WORLD_VERTEX],
    position: [f32; 3],
    normal: [f32; 3],
    tangent: [f32; 4],
    color: [f32; 4],
    uv: [f32; 2],
    lightmap_uv: [f32; 2],
}

fn decode_vertex(v: &[u8]) -> Decoded {
    let position = [rd_f32(v, 0), rd_f32(v, 4), rd_f32(v, 8)];
    let binormal_sign = rd_f32(v, 12);
    // T6 colour bytes are R, G, B, A (the lit shaders read them as RGBA
    // unorm); the IW4 row holds a D3DCOLOR, B, G, R, A.
    let [r, g, b, a] = [v[16], v[17], v[18], v[19]];
    let uv = half2(rd_u32(v, 20));
    let normal = normalized(unpack_t6_unit_vec(rd_u32(v, 24)));
    let tangent3 = normalized(unpack_t6_unit_vec(rd_u32(v, 28)));
    let lm = rd_u32(v, 32);
    let lightmap_uv = [
        f32::from(lm as u16) / 65535.0,
        f32::from((lm >> 16) as u16) / 65535.0,
    ];

    let mut packed = [0u8; iw4_sz::GFX_WORLD_VERTEX];
    packed[0..12].copy_from_slice(&v[0..12]);
    packed[12..16].copy_from_slice(&binormal_sign.to_le_bytes());
    packed[16..20].copy_from_slice(&[b, g, r, a]);
    packed[20..24].copy_from_slice(&uv[0].to_le_bytes());
    packed[24..28].copy_from_slice(&uv[1].to_le_bytes());
    packed[28..32].copy_from_slice(&lightmap_uv[0].to_le_bytes());
    packed[32..36].copy_from_slice(&lightmap_uv[1].to_le_bytes());
    packed[36..40].copy_from_slice(&pack_iw4_unit_vec(normal).to_le_bytes());
    packed[40..44].copy_from_slice(&pack_iw4_unit_vec(tangent3).to_le_bytes());

    Decoded {
        packed,
        position,
        normal,
        tangent: [tangent3[0], tangent3[1], tangent3[2], binormal_sign],
        color: [r, g, b, a].map(|c| f32::from(c) / 255.0),
        uv,
        lightmap_uv,
    }
}

/// The lighting a T6 world draws with, besides its lightmaps (measured from
/// the lit world shaders, `pimp_shader_lmap_*`): the sun the shaders add,
/// times the lightmap's baked sun visibility, and the exposure multiplier
/// every lit result is scaled by.
#[derive(Clone, Copy, Debug)]
pub struct T6WorldLighting {
    pub sun: WorldSunLight,
    pub exposure_scale: f32,
    /// The sky material's `skyBoxRotation` x, y, z, then its brightness
    /// (`skyColorParm` y), negative when the rotation's w flips z.
    pub sky: [f32; 4],
}

/// One T6 lightmap as the engine's world lightmap. A T6 lightmap is one
/// RGBA8 image of three stacked pages, addressed at `(u, v / 3 + page / 3)`:
/// page 0 base light and page 1 directional light (both `rgb / a`), page 2
/// the light direction (`rgb * 2 - 1`) with the baked sun visibility in
/// alpha. It rides in the secondary slot, read linear; the other slots get
/// 1x1 stand-ins (T6 has no primary page).
pub fn t6_lightmap(image: &asset_t6::ImageRef) -> Option<WorldLightmap> {
    use bevy::asset::RenderAssetUsages;
    use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
    use bevy::prelude::{Image, UVec2};
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

    let embedded = image.embedded.as_ref()?;
    // DXGI_FORMAT_R8G8B8A8_UNORM.
    if embedded.dxgi_format != 28 {
        return None;
    }
    let (w, h) = (u32::from(image.width), u32::from(image.height));
    let bytes = (w * h * 4) as usize;
    let data = embedded.data.get(..bytes)?.to_vec();
    let mut page = Image::new(
        Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    page.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..Default::default()
    });
    let stand_in = || {
        Image::new(
            Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            vec![255, 255, 255, 255],
            TextureFormat::Rgba8Unorm,
            RenderAssetUsages::RENDER_WORLD,
        )
    };
    Some(WorldLightmap {
        primary_image: None,
        secondary_image: Some(page),
        secondary_b_image: None,
        ambient_image: stand_in(),
        directional_image: stand_in(),
        sun_mask_image: stand_in(),
        ambient_source_name: image.name.clone(),
        sun_mask_source_name: String::new(),
        ambient_size: UVec2::new(w, h),
        sun_mask_size: UVec2::ONE,
    })
}

/// Build the engine's world from a captured T6 GfxWorld. `surface_materials`
/// holds each world surface's material as an index into the lane's catalog
/// (one entry per `world.surfaces`, in order; missing entries are `None`).
/// BO2 materials have no technique the engine can run, so their surfaces
/// draw through the renderer's fallback pass, which samples the colour map.
pub fn build_t6_world_draw(
    world: &WorldRef,
    surface_materials: &[Option<usize>],
    surface_layers: &[asset_core::T6Layers],
    lightmaps: Vec<Option<WorldLightmap>>,
    lighting: Option<T6WorldLighting>,
) -> Result<WorldDraw, WorldMeshError> {
    if world.surfaces.is_empty() || world.vd0.is_empty() {
        return Err(WorldMeshError::NoGeometry);
    }

    // Each vertex group is expanded once per second-stream layout its
    // surfaces read it with (layered surfaces add their layers' texcoords
    // from `vd1`), sized by the largest index those surfaces use.
    let group_key = |i: usize| -> (i32, i32, asset_core::T6Layers) {
        let surf = &world.surfaces[i];
        match surface_layers.get(i) {
            Some(layers) if layers.any() => {
                (surf.vertex_data_offset0, surf.vertex_data_offset1, *layers)
            }
            _ => (
                surf.vertex_data_offset0,
                -1,
                asset_core::T6Layers::default(),
            ),
        }
    };
    let mut group_len: HashMap<(i32, i32, asset_core::T6Layers), usize> = HashMap::new();
    for (i, surf) in world.surfaces.iter().enumerate() {
        let first = surf.base_index as usize;
        let count = surf.tri_count as usize * 3;
        let indices = world
            .indices
            .get(first..first + count)
            .ok_or(WorldMeshError::NoGeometry)?;
        let need = indices.iter().map(|&i| i as usize + 1).max().unwrap_or(0);
        let e = group_len.entry(group_key(i)).or_insert(0);
        *e = (*e).max(need);
    }

    let mut group_base: HashMap<(i32, i32, asset_core::T6Layers), u32> = HashMap::new();
    let mut layer_uvs: Vec<[f32; 4]> = Vec::new();
    let mut packed_vertices = Vec::new();
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut tangents = Vec::new();
    let mut colors = Vec::new();
    let mut texture_uvs = Vec::new();
    let mut lightmap_uvs = Vec::new();
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];

    let mut packed_indices: Vec<u32> = Vec::new();
    let n = world.surfaces.len();
    let mut surface_index_ranges = Vec::with_capacity(n);
    let mut surface_first_vertex = Vec::with_capacity(n);
    let mut surface_draw_fields = Vec::with_capacity(n);
    let mut surface_lightmap_indices = Vec::with_capacity(n);
    let mut surface_reflection_probes = Vec::with_capacity(n);
    let mut surface_primary_lights = Vec::with_capacity(n);
    let mut surface_bounds = Vec::with_capacity(n);

    // The surfaces the game draws: the static visibility list's. Others
    // (Nuketown's orange light dome) only feed the light compiler.
    let mut drawn = vec![world.sorted_surf_index.is_empty(); world.surfaces.len()];
    for &i in &world.sorted_surf_index {
        if let Some(d) = drawn.get_mut(usize::from(i)) {
            *d = true;
        }
    }
    for (surf_index, surf) in world.surfaces.iter().enumerate() {
        let key = group_key(surf_index);
        let base = match group_base.get(&key) {
            Some(&base) => base,
            None => {
                let base = positions.len() as u32;
                let count = group_len[&key];
                let start = surf.vertex_data_offset0 as usize;
                let block = world
                    .vd0
                    .get(start..start + count * T6_WORLD_VERTEX)
                    .ok_or(WorldMeshError::NoGeometry)?;
                let (_, start1, layers) = key;
                for (vi, v) in block.chunks_exact(T6_WORLD_VERTEX).enumerate() {
                    // Layers 1 and 2's texcoords from the second stream.
                    let mut layer = [0.0f32; 4];
                    if start1 >= 0 {
                        for k in 0..2 {
                            if layers.kinds[k] == 0 {
                                continue;
                            }
                            let at = start1 as usize
                                + vi * usize::from(layers.stride)
                                + usize::from(layers.uv_offsets[k]);
                            if let Some(b) = world.vd1.get(at..at + 4) {
                                let uv = half2(rd_u32(b, 0));
                                layer[k * 2] = uv[0];
                                layer[k * 2 + 1] = uv[1];
                            }
                        }
                    }
                    layer_uvs.push(layer);
                    let d = decode_vertex(v);
                    for axis in 0..3 {
                        min[axis] = min[axis].min(d.position[axis]);
                        max[axis] = max[axis].max(d.position[axis]);
                    }
                    packed_vertices.push(d.packed);
                    positions.push(d.position);
                    normals.push(d.normal);
                    tangents.push(d.tangent);
                    colors.push(d.color);
                    texture_uvs.push(d.uv);
                    lightmap_uvs.push(d.lightmap_uv);
                }
                group_base.insert(key, base);
                base
            }
        };
        let range_start = packed_indices.len() as u32;
        let first = surf.base_index as usize;
        let count = if drawn[surf_index] {
            surf.tri_count as usize * 3
        } else {
            0
        };
        for &i in &world.indices[first..first + count] {
            packed_indices.push(base + i as u32);
        }
        surface_index_ranges.push((range_start, count as u32));
        surface_first_vertex.push(base);
        let lightmap_index = surf.lightmap_index as u8;
        let probe = surf.reflection_probe_index as u8;
        let light = surf.primary_light_index as u8;
        surface_lightmap_indices.push(lightmap_index);
        surface_reflection_probes.push(probe);
        surface_primary_lights.push(light);
        surface_draw_fields.push(SurfaceDrawFields {
            first_vertex: base,
            tri_count: (count / 3) as u16,
            base_index: range_start,
            lightmap_index,
            reflection_probe_index: probe,
            primary_light_index: light,
        });
        surface_bounds.push(Bounds::from_mins_maxs(surf.bounds[0], surf.bounds[1]));
    }

    let surface_materials: Vec<Option<usize>> = (0..n)
        .map(|i| surface_materials.get(i).copied().flatten())
        .collect();
    // A surface is lightmapped when its lightmap index names a page this
    // world built (31 = none).
    let surface_lightmapped: Vec<bool> = surface_lightmap_indices
        .iter()
        .map(|&i| lightmaps.get(usize::from(i)).is_some_and(Option::is_some))
        .collect();
    let (batches, surface_batch_ranges) = crate::world_t5::make_material_batches(
        &positions,
        &normals,
        &tangents,
        &colors,
        &texture_uvs,
        &lightmap_uvs,
        &packed_indices,
        &surface_index_ranges,
        &surface_materials,
        &surface_lightmapped,
        &surface_lightmap_indices,
        &surface_primary_lights,
        &surface_reflection_probes,
    );

    let dpvs = one_cell_dpvs(world, surface_bounds)?;
    let stats = WorldMeshStats {
        vertices: positions.len(),
        triangles: packed_indices.len() / 3,
        surfaces: n,
        skipped_surfaces: 0,
        sky_surfaces: 0,
        sky_material: None,
        unrouted_surfaces: n,
        undecided_state_bits_surfaces: 0,
        min,
        max,
        bounds: None,
    };

    Ok(WorldDraw {
        batches,
        sky_model: None,
        lightmap: if lightmaps.iter().any(Option::is_some) {
            Ok(lightmaps)
        } else {
            Err(WorldLightmapGap::Missing)
        },
        stats,
        packed_vertices: WorldVertexPayload::T6(packed_vertices),
        vertex_layer: Vec::new(),
        surface_vertex_layer: vec![0; n],
        surface_first_vertex,
        surface_draw_fields,
        positions,
        normals,
        tangents,
        colors,
        texture_uvs,
        lightmap_uvs,
        packed_indices,
        surface_index_ranges,
        surface_batch_ranges,
        surface_lightmapped,
        surface_lightmap_indices,
        surface_reflection_probes,
        surface_primary_lights,
        sort_key_distortion: None,
        capture: world_capture_from_casters(SurfaceCastsSunShadow::with_len(n)),
        brush_models: Vec::new(),
        brush_model_bounds: Vec::new(),
        surface_materials,
        primary_lights: Vec::new(),
        light_defs: Vec::new(),
        sun_primary_light_count: 0,
        light_region_hulls: None,
        shadow_geometry: Vec::new(),
        reflection_probes: Vec::new(),
        dpvs,
        outdoor_image_name: None,
        outdoor_image: None,
        outdoor_lookup: [0; 16],
        sun_effects: None,
        // bo2zm: the T5 sun and exposure carriers hold the T6 world's; for a
        // T6 world only the renderer's fallback draw reads them.
        t5_sun_parse_exposure: lighting.map(|l| l.exposure_scale),
        t5_sky_dynamic_intensity: lighting.map(|l| l.sky),
        t5_sun_light: lighting.map(|l| l.sun),
        t5_tree_scatter_intensity: None,
        t5_tree_scatter_amount: None,
        t5_exposure_volume_count: 0,
        t6_lights: Vec::new(),
        t6_fog_banks: Vec::new(),
        t6_probes: Vec::new(),
        t6_layer_uvs: layer_uvs,
    })
}

/// One cell, one AABB node over every surface, identity sort order.
fn one_cell_dpvs(
    world: &WorldRef,
    surface_bounds: Vec<Bounds>,
) -> Result<DpvsWorldData, WorldMeshError> {
    let n = world.surfaces.len();
    let count = u16::try_from(n).map_err(|_| WorldMeshError::NoGeometry)?;
    let mut out = DpvsWorldData::new(CameraSurfRanges::new(
        CameraSurfRange {
            kind: CameraRangeKind::LitOpaque,
            begin: 0,
            end: n as u32,
        },
        Vec::new(),
    ));
    out.cell_count = 1;
    out.static_surface_count = n;
    out.static_surface_count_no_decal = n;
    out.lit_opaque_begin = 0;
    out.lit_opaque_end = n as u32;
    out.sorted_surf_index = (0..count).collect();
    out.surface_bounds = surface_bounds;
    out.cell_roots = vec![SurfRange { start: 0, count }];
    out.aabb_trees = vec![vec![AabbNodeView {
        bounds: Bounds::from_mins_maxs(world.mins, world.maxs),
        child_count: 0,
        children_offset: 0,
        start_surf: 0,
        surface_count: count,
        start_surf_no_decal: 0,
        surface_count_no_decal: count,
        smodel_index_start: 0,
        smodel_index_count: 0,
    }]];
    out.aabb_smodel_indices = vec![Vec::new()];
    out.portals_per_cell = vec![Vec::new()];
    out.cell_reflection_probes = vec![Vec::new()];
    out.checked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t6_unit_vectors_are_signed_ten_bit() {
        // x = +511, y = -511 (0x201), z = 0.
        let packed = 0x1ff | 0x201 << 10;
        let v = unpack_t6_unit_vec(packed);
        assert!((v[0] - 1.0).abs() < 1e-6 && (v[1] + 1.0).abs() < 1e-6 && v[2] == 0.0);
    }

    #[test]
    fn iw4_repack_round_trips_through_iw4_unpack() {
        let n = normalized([0.3, -0.5, 0.8]);
        let back = asset_model::unpack_unit_vec(pack_iw4_unit_vec(n));
        for axis in 0..3 {
            assert!((back[axis] - n[axis]).abs() < 0.02);
        }
    }
}
