//! bo2zm: Black Ops II clipMap_t -> ClipCollision. Not part of upstream IW4L.
//!
//! T6 brushes and collision triangles follow T5's shapes (box + extra side
//! planes; triangles in partitions under an AABB tree), so this mirrors the
//! T5 conversion: six axial planes from the box with the axial side flags,
//! then every extra side; surface flags translated the T5 way. The collision
//! tree comes over too, as T5's does: the world's walls are only the brushes
//! its leaves list. Brush models (doors, triggers, the zombie mode's volumes)
//! keep their brushes in the same table, reachable only through `cmodels`;
//! testing every brush instead walls the player inside those volumes.

use asset_t6::{ClipLeafRef, ClipRef};

use crate::clip_collision::{
    ClipBrush, ClipBspLeaf, ClipBspNode, ClipCmodel, ClipCollision, ClipCollisionError,
    ClipMapMaterial, LEAFBRUSH_NODE_MAX_DEPTH, leafbrush_node_visits, leafbrush_range,
    t5_axial_plane_flags, t5_surface_flags,
};

/// Append the brush list under one leaf's brush node (0 = none) and return
/// its range in `out`.
fn append_leaf_brushes(
    clip: &ClipRef,
    leaf: &ClipLeafRef,
    out: &mut Vec<u16>,
) -> Result<(u32, u16), ClipCollisionError> {
    let first_len = out.len();
    if leaf.leaf_brush_node > 0 {
        let mut pending = vec![(leaf.leaf_brush_node as usize, 0)];
        while let Some((index, depth)) = pending.pop() {
            if depth > LEAFBRUSH_NODE_MAX_DEPTH {
                return Err(ClipCollisionError::Truncated);
            }
            let node = clip
                .leafbrush_nodes
                .get(index)
                .ok_or(ClipCollisionError::Truncated)?;
            if node.leaf_brush_count > 0 {
                out.extend_from_slice(&node.brushes);
            } else {
                pending.extend(
                    leafbrush_node_visits(
                        index,
                        node.leaf_brush_count,
                        usize::from(node.child_offset[0]),
                        usize::from(node.child_offset[1]),
                    )
                    .into_iter()
                    .map(|i| (i, depth + 1)),
                );
            }
        }
    }
    leafbrush_range(first_len, out.len())
}

pub fn build_t6_clip_collision(clip: &ClipRef) -> Result<ClipCollision, ClipCollisionError> {
    if clip.brushes.is_empty() && clip.tri_indices.is_empty() {
        return Err(ClipCollisionError::MissingTables);
    }
    let mut out = ClipCollision {
        brushes: Vec::with_capacity(clip.brushes.len()),
        ..ClipCollision::default()
    };
    for b in &clip.brushes {
        let (mins, maxs) = (b.mins, b.maxs);
        let mut planes = Vec::with_capacity(6 + b.sides.len());
        planes.push([1.0, 0.0, 0.0, maxs[0]]);
        planes.push([-1.0, 0.0, 0.0, -mins[0]]);
        planes.push([0.0, 1.0, 0.0, maxs[1]]);
        planes.push([0.0, -1.0, 0.0, -mins[1]]);
        planes.push([0.0, 0.0, 1.0, maxs[2]]);
        planes.push([0.0, 0.0, -1.0, -mins[2]]);
        let axial = [
            b.axial_sflags[0][0] as u32,
            b.axial_sflags[0][1] as u32,
            b.axial_sflags[0][2] as u32,
            b.axial_sflags[1][0] as u32,
            b.axial_sflags[1][1] as u32,
            b.axial_sflags[1][2] as u32,
        ];
        let mut plane_surface_flags = Vec::with_capacity(planes.capacity());
        plane_surface_flags.extend_from_slice(&t5_axial_plane_flags(axial.map(t5_surface_flags)));
        for &(normal, dist, sflags) in &b.sides {
            planes.push([normal[0], normal[1], normal[2], dist]);
            plane_surface_flags.push(t5_surface_flags(sflags as u32));
        }
        out.brushes.push(ClipBrush {
            planes,
            contents: b.contents as u32,
            plane_surface_flags,
            glass_encoded: 0,
        });
    }

    for m in &clip.materials {
        out.materials.push(ClipMapMaterial {
            name: m.name.clone(),
            surface_flags: t5_surface_flags(m.surface_flags as u32),
            content_flags: m.content_flags as u32,
        });
    }

    let mesh = std::sync::Arc::make_mut(&mut out.mesh);
    mesh.verts = clip.verts.clone();
    mesh.tri_indices = clip.tri_indices.clone();
    mesh.tri_edge_is_walkable = clip.tri_edge_is_walkable.clone();
    mesh.partitions = clip
        .partitions
        .iter()
        .map(|&(tri_count, first_tri)| clipmap_iw4::ClipPartition {
            tri_count,
            first_tri,
            first_vert_segment: 0,
            border_count: 0,
            first_border: 0,
        })
        .collect();
    mesh.aabb_trees = clip
        .aabb_trees
        .iter()
        .map(|t| clipmap_iw4::ClipAabbNode {
            origin: t.origin,
            half_size: t.half_size,
            material_index: t.material_index,
            child_count: t.child_count,
            u: t.u,
        })
        .collect();
    let tri_count = clip.tri_indices.len() / 3;
    let leaves: Vec<_> = mesh
        .aabb_trees
        .iter()
        .filter(|n| n.child_count == 0)
        .map(|n| (n.material_index, n.u))
        .collect();
    let sflags: Vec<_> = out.materials.iter().map(|m| m.surface_flags).collect();
    let cflags: Vec<_> = out.materials.iter().map(|m| m.content_flags).collect();
    mesh.tri_surface_flags =
        clipmap_iw4::flatten_tri_surface_flags(&leaves, &clip.partitions, &sflags, tri_count);
    mesh.tri_content_flags =
        clipmap_iw4::flatten_tri_surface_flags(&leaves, &clip.partitions, &cflags, tri_count);
    out.tri_material_index =
        clipmap_iw4::flatten_tri_material_index(&leaves, &clip.partitions, tri_count);

    for n in &clip.nodes {
        out.nodes.push(ClipBspNode {
            plane: n.plane,
            children: [i32::from(n.children[0]), i32::from(n.children[1])],
        });
    }
    let mut roots = std::collections::BTreeSet::new();
    for leaf in &clip.leaves {
        let (first_brush, num_brushes) = append_leaf_brushes(clip, leaf, &mut out.leafbrushes)?;
        let first = u32::from(leaf.first_coll_aabb_index);
        for id in first..first + u32::from(leaf.coll_aabb_count) {
            roots.insert(u16::try_from(id).map_err(|_| ClipCollisionError::Truncated)?);
        }
        out.leaves.push(ClipBspLeaf {
            first_brush,
            num_brushes,
            first_coll_aabb_index: leaf.first_coll_aabb_index,
            coll_aabb_count: leaf.coll_aabb_count,
        });
    }
    std::sync::Arc::make_mut(&mut out.mesh).aabb_roots = roots.into_iter().collect();
    for m in &clip.cmodels {
        let (first_brush, num_brushes) = if m.shares_map_info {
            append_leaf_brushes(clip, &m.leaf, &mut out.leafbrushes)?
        } else {
            leafbrush_range(out.leafbrushes.len(), out.leafbrushes.len())?
        };
        out.cmodels.push(ClipCmodel {
            mins: m.mins,
            maxs: m.maxs,
            radius: m.radius,
            first_brush,
            num_brushes,
        });
    }
    Ok(out)
}
