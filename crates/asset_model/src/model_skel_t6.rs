//! bo2zm: Black Ops II models with their skeletons, as the engine's
//! skinned model (`ModelSkel`) — viewmodel guns and arms, world guns.
//!
//! Vertices are BO2's `GfxPackedVertex` (32 bytes): position, binormal
//! sign, colour R,G,B,A, texcoord half2, normal and tangent packed 10:10:10
//! signed. The engine's skinning reads the IW4 packing (colour B,G,R,A, u
//! in the high half, unit vectors as biased bytes), so each vertex is
//! repacked. Rigid vertex lists and blend weights use the same bone units
//! (64 bytes per bone) as IW4 and Black Ops.

use bevy::math::{Quat, Vec3};

use asset_iw4::size as sz;
use xmodel_runtime::ModelPoseSrc;

use crate::{BoneBind, ModelSkel, VertSkin, WalkLocalMaterialIndex, half_to_f32};

const BONE_STRIDE: u16 = 64;
const T6_PACKED_VERTEX: usize = 32;

fn rd_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn rd_f32(b: &[u8], at: usize) -> f32 {
    f32::from_bits(rd_u32(b, at))
}

/// BO2's packed unit vector: three signed 10-bit components over 511.
pub fn unpack_t6_unit_vec(packed: u32) -> [f32; 3] {
    let c = |v: u32| {
        let v = (v & 0x3ff) as i32;
        let v = if v >= 512 { v - 1024 } else { v };
        v as f32 / 511.0
    };
    [c(packed), c(packed >> 10), c(packed >> 20)]
}

fn normalized(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 1e-6 {
        [v[0] / l, v[1] / l, v[2] / l]
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// The IW4 packed unit vector: biased bytes over a fixed scale of 1/127.
fn pack_iw4_unit_vec(n: [f32; 3]) -> u32 {
    let b = |v: f32| ((v * 127.0 + 127.5) as i32).clamp(0, 255) as u8;
    u32::from_le_bytes([b(n[0]), b(n[1]), b(n[2]), 63])
}

/// One BO2 packed vertex as the IW4 packed vertex the engine skins.
pub fn t6_to_iw4_packed_vertex(v: &[u8]) -> [u8; sz::GFX_PACKED_VERTEX] {
    let [r, g, b, a] = [v[16], v[17], v[18], v[19]];
    let tex = rd_u32(v, 20);
    let normal = normalized(unpack_t6_unit_vec(rd_u32(v, 24)));
    let tangent = normalized(unpack_t6_unit_vec(rd_u32(v, 28)));
    let mut packed = [0u8; sz::GFX_PACKED_VERTEX];
    packed[0..16].copy_from_slice(&v[0..16]);
    packed[16..20].copy_from_slice(&[b, g, r, a]);
    packed[20..24].copy_from_slice(&tex.rotate_left(16).to_le_bytes());
    packed[24..28].copy_from_slice(&pack_iw4_unit_vec(normal).to_le_bytes());
    packed[28..32].copy_from_slice(&pack_iw4_unit_vec(tangent).to_le_bytes());
    packed
}

/// Per-vertex bone influences of one surface: weighted vertices first (by
/// influence count), then the rigid vertex lists. `None` when a bone
/// offset is not a whole bone or names a bone the model lacks.
fn surface_skins(srf: &asset_t6::XSurfaceRef, num_bones: usize) -> Option<Vec<VertSkin>> {
    let n = usize::from(srf.vert_count);
    let mut skins = vec![
        VertSkin {
            bones: [0; 4],
            weights: [1.0, 0.0, 0.0, 0.0],
            ..Default::default()
        };
        n
    ];
    let bone_at = |raw: u16| -> Option<u16> {
        if raw % BONE_STRIDE != 0 {
            return None;
        }
        let bone = raw / BONE_STRIDE;
        ((bone as usize) < num_bones).then_some(bone)
    };
    let mut vertex = 0usize;
    let mut cursor = 0usize;
    for (bucket, &count) in srf.blend_counts.iter().enumerate() {
        let influences = bucket + 1;
        for _ in 0..count.max(0) {
            let words = 1 + (influences - 1) * 2;
            let w = srf.blend.get(cursor..cursor + words)?;
            cursor += words;
            let mut skin = VertSkin {
                bones: [bone_at(w[0])?, 0, 0, 0],
                ..Default::default()
            };
            let mut remaining = 1.0f32;
            for extra in 1..influences {
                let b = w[1 + (extra - 1) * 2];
                let raw = w[2 + (extra - 1) * 2];
                let weight = dpvs_iw4::skin_blend_weight(raw);
                skin.bones[extra] = bone_at(b)?;
                skin.weights[extra] = weight;
                skin.weight_u16[extra] = raw;
                remaining -= weight;
            }
            skin.weights[0] = remaining;
            *skins.get_mut(vertex)? = skin;
            vertex += 1;
        }
    }
    for &(bone_offset, run, _, _) in &srf.vert_lists {
        let bone = bone_at(bone_offset)?;
        for _ in 0..run {
            if vertex >= n {
                break;
            }
            skins[vertex] = VertSkin {
                bones: [bone, 0, 0, 0],
                weights: [1.0, 0.0, 0.0, 0.0],
                ..Default::default()
            };
            vertex += 1;
        }
    }
    Some(skins)
}

/// A BO2 model (first LOD) as the engine's skinned model. `material` maps a
/// surface's material to the catalog it was linked into.
pub fn capture_model_skel_t6(
    model: &asset_t6::XModelRef,
    material: impl Fn(asset_t6::AssetKey) -> Option<usize>,
) -> Option<ModelSkel> {
    let num_bones = model.bone_names.len();
    if num_bones == 0 || model.base_mat.len() != num_bones {
        return None;
    }
    let bones: Vec<BoneBind> = model
        .base_mat
        .iter()
        .map(|(q, t)| BoneBind { quat: *q, trans: *t })
        .collect();
    let find = |name: &str| model.bone_names.iter().position(|b| b == name);
    let num_root_bones = usize::from(model.num_root_bones);
    let pose = ModelPoseSrc {
        name: model.name.clone(),
        num_bones,
        num_root_bones,
        scale: 1.0,
        no_scale_part_bits: [0; 6],
        bone_names: model.bone_names.clone(),
        parent_list: model.parent_list.clone(),
        quats: model.quats.clone(),
        trans: model.trans.clone(),
        base_mat: bones
            .iter()
            .map(|b| {
                (
                    Quat::from_xyzw(b.quat[0], b.quat[1], b.quat[2], b.quat[3]),
                    Vec3::from_array(b.trans),
                )
            })
            .collect(),
    };

    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut colors = Vec::new();
    let mut uvs = Vec::new();
    let mut indices = Vec::new();
    let mut surface_materials = Vec::with_capacity(model.lod0.len());
    let mut surface_vertex_ranges = Vec::with_capacity(model.lod0.len());
    let mut surface_index_ranges = Vec::with_capacity(model.lod0.len());
    let mut surface_part_bits = Vec::with_capacity(model.lod0.len());
    let mut surface_deformed = Vec::with_capacity(model.lod0.len());
    let mut surface_vert_list_count = Vec::with_capacity(model.lod0.len());
    let mut vert_skin = Vec::new();
    let mut packed_vertices = Vec::new();
    let (mut rigid_verts, mut blend_verts) = (0usize, 0usize);
    for srf in &model.lod0 {
        let n = usize::from(srf.vert_count);
        if srf.verts.len() < n * T6_PACKED_VERTEX || srf.indices.iter().any(|&i| usize::from(i) >= n) {
            return None;
        }
        let skins = surface_skins(srf, num_bones)?;
        let base = positions.len();
        let index_start = indices.len();
        for (v, skin) in srf.verts.chunks_exact(T6_PACKED_VERTEX).take(n).zip(&skins) {
            positions.push([rd_f32(v, 0), rd_f32(v, 4), rd_f32(v, 8)]);
            normals.push(normalized(unpack_t6_unit_vec(rd_u32(v, 24))));
            colors.push([v[16], v[17], v[18], v[19]].map(|c| f32::from(c) / 255.0));
            let tex = rd_u32(v, 20);
            uvs.push([half_to_f32(tex as u16), half_to_f32((tex >> 16) as u16)]);
            packed_vertices.push(t6_to_iw4_packed_vertex(v));
            if skin.weights[0] == 1.0 && skin.weights[1] == 0.0 {
                rigid_verts += 1;
            } else {
                blend_verts += 1;
            }
            vert_skin.push(*skin);
        }
        indices.extend(srf.indices.iter().map(|&i| base as u32 + u32::from(i)));
        surface_vertex_ranges.push((base, n));
        surface_index_ranges.push((index_start, indices.len() - index_start));
        surface_part_bits.push(srf.part_bits);
        surface_deformed.push(Some(srf.flags & 0x80 != 0));
        surface_vert_list_count.push(Some(srf.vert_lists.len() as u32));
        surface_materials.push(
            srf.material
                .and_then(&material)
                .map(WalkLocalMaterialIndex::from_walk),
        );
    }
    let surfaces = surface_vertex_ranges.len();
    Some(ModelSkel {
        name: model.name.clone(),
        bones,
        // bo2zm M3: hit boxes per bone (what bullets hit on a zombie).
        bone_collision: (0..num_bones)
            .map(|b| {
                let (mins, maxs, radius_sq) = (*model.bone_info.get(b)?)?;
                Some(xmodel_runtime::BoneCollision {
                    midpoint: std::array::from_fn(|k| (mins[k] + maxs[k]) * 0.5),
                    half_size: std::array::from_fn(|k| (maxs[k] - mins[k]) * 0.5),
                    radius_sq,
                    part_classification: model.part_classification.get(b).copied().unwrap_or(0),
                })
            })
            .collect(),
        bone_names: model.bone_names.clone(),
        tag_view: find("tag_view"),
        tag_weapon: find("tag_weapon"),
        pose: Some(pose),
        positions,
        normals,
        colors,
        uvs,
        indices,
        surface_materials,
        surface_vertex_ranges,
        surface_index_ranges,
        surface_part_bits,
        surface_deformed,
        surface_vert_list_count,
        vert_skin,
        rigid_verts,
        blend_verts,
        packed_vertices,
        radius: Some(model.radius),
        // (mid, half), the convention every model skeleton uses.
        bounds: Some((
            std::array::from_fn(|k| (model.mins[k] + model.maxs[k]) * 0.5),
            std::array::from_fn(|k| (model.maxs[k] - model.mins[k]) * 0.5),
        )),
        contents: Some(model.contents),
        coll_lod: -1,
        coll_surfs: Vec::new(),
        lod: Some(crate::ModelLodSelector::T5 {
            num_lods: 1,
            lod_dist: [0.0; 4],
        }),
        lod_smc: None,
        lod_part_bits: None,
        lod_surf_span: [(0, surfaces as u16), (0, 0), (0, 0), (0, 0)],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_vec_round_trip() {
        // +x packed as 511 in the low ten bits.
        assert_eq!(unpack_t6_unit_vec(511), [1.0, 0.0, 0.0]);
        // -z: 1024 - 511 = 513 in bits 20..29.
        let v = unpack_t6_unit_vec(513 << 20);
        assert!((v[2] + 1.0).abs() < 1e-6);
    }
}
