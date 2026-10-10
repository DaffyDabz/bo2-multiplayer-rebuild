use std::collections::HashMap;

use bevy::prelude::*;

use crate::anim::scene_submission::{AnimDObjSceneSkels, AnimDObjSceneSubmission, AnimSceneSubmit};
use crate::anim::xmodel_pose::PosedModelSurface;
use crate::occupancy::script_model::pose_script_dobj_with_materials;
use crate::{
    MissileDrawPlan, MissileOwnerDraw, XMODEL_OBJECT_ID_MISSILE_BASE, append_missile_surfaces,
    lighting_box_half,
};
use anim_iw4::DOBJ_RADIUS_PARENT_ROOT;
use render_scene::{
    HostGfxScene, ModelLightingOwner, ModelLightingRequest, ModelLightingRequests,
    SmodelPassMaterial, TessMaterials, WorldModelLightingAtlas, WorldPresentFacts,
    XModelSurfaceDraw, scene_quat_from_angles,
};

pub const MISSILE_LIGHTING_Z_OFS: f32 = 4.0;

/// bo2zm: Black Ops II projectiles in flight (a thrown grenade, a rocket,
/// a ballistic knife's blade), drawn by the fallback pass: each one's
/// model (from the first-person catalog, which loads every weapon's
/// projectile model), its surfaces' materials and where it is.
#[derive(Resource, Default, Clone)]
pub struct T6MissileModels {
    pub items: Vec<T6MissileModel>,
}

#[derive(Clone)]
pub struct T6MissileModel {
    pub skel: std::sync::Arc<asset_model::ModelSkel>,
    /// Per surface, its catalog material (none = not drawn).
    pub materials: Vec<Option<u32>>,
    pub world_from_local: Mat4,
    pub origin: [f32; 3],
    /// bo2zm M3 fix list 1: where its light is read from the light grid: a
    /// script model's middle (its first model's bounds centre), not its
    /// feet (a zombie whose feet sat in the ground read black there).
    pub light_at: [f32; 3],
    /// bo2zm M3: skinned positions and normals in model space (an animated
    /// model); none = bind pose.
    pub posed: Option<std::sync::Arc<PosedVerts>>,
    /// bo2mp: the parts hidden on its entity (a mannequin's shot-off head)
    /// and where this model's bones start among them; none = all drawn.
    pub hide: Option<([u32; 6], u32)>,
}

impl T6MissileModel {
    /// bo2mp: a surface's triangles as Black Ops II draws them with the
    /// model's hidden parts: none when every bone the surface is skinned to
    /// is hidden; a rigid surface with some of its bones hidden loses those
    /// bones' vertex lists; else all of them.
    pub fn drawn_indices<'a>(
        &self,
        surface: usize,
        indices: &'a [u32],
    ) -> Option<std::borrow::Cow<'a, [u32]>> {
        let all = Some(std::borrow::Cow::Borrowed(indices));
        let Some((hide, base)) = self.hide else {
            return all;
        };
        let skel = &self.skel;
        let Some(bits) = skel.surface_part_bits.get(surface) else {
            return all;
        };
        if anim_iw4::surface_hidden_whole(bits, &hide, base) {
            return None;
        }
        if skel.surface_deformed.get(surface).copied().flatten() != Some(false)
            || !anim_iw4::surface_hidden(bits, &hide, base)
        {
            return all;
        }
        let hidden = |v: u32| {
            skel.vert_skin.get(v as usize).is_some_and(|skin| {
                anim_iw4::hide_part_bit(&hide, base as usize + usize::from(skin.bones[0]))
            })
        };
        Some(std::borrow::Cow::Owned(
            indices
                .chunks_exact(3)
                .filter(|tri| !tri.iter().any(|&v| hidden(v)))
                .flatten()
                .copied()
                .collect(),
        ))
    }
}

/// bo2zm M3: one model's vertices after skinning.
#[derive(Clone, Default)]
pub struct PosedVerts {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
}

#[derive(Resource, Default)]
pub struct MissileOccupancy {
    pub rows: Vec<OccupiedMissile>,
}

pub struct OccupiedMissile {
    pub index: usize,
    pub id: Option<u32>,
    pub entnum: Option<u32>,
    pub weapon: u32,
    pub ignited: bool,
    pub origin: [f32; 3],
    pub angles: [f32; 3],
    pub name: String,
    pub namespace: asset_core::AssetNamespace,
    pub lighting_origin: [f32; 3],
    pub lighting_owner: ModelLightingOwner,
}

#[derive(Resource, Default)]
struct MissilePoseProduct {
    rows: Vec<MissilePosed>,
}

struct MissilePosed {
    index: usize,
    entnum: Option<u32>,
    origin: [f32; 3],
    angles: [f32; 3],
    name: String,
    namespace: asset_core::AssetNamespace,
    lighting_origin: [f32; 3],
    lighting_owner: ModelLightingOwner,
    surfaces: Vec<PosedModelSurface>,
}

struct MissileSceneSlot<'a> {
    hidden: bool,
    skin_entries: &'a [dpvs_iw4::SceneEntSkinEntry],
}

fn missile_scene_slot(scene: &render_scene::GfxScene, entnum: u32) -> MissileSceneSlot<'_> {
    MissileSceneSlot {
        hidden: scene.scene_ent_skips_draw(entnum),
        skin_entries: scene
            .scene_ent_skinned_surfs(entnum)
            .map(|surfs| surfs.entries.as_slice())
            .unwrap_or(&[]),
    }
}

#[derive(Resource, Default)]
pub struct MissileBoltState {
    pub rows: HashMap<u32, MissileBoltRow>,

    pub predicted_rows_skipped: u64,

    pub pose_gaps: u64,

    pub play_gaps: u64,

    pub ignition_gaps: u64,
}

pub struct MissileBoltRow {
    pub projectile: Option<u32>,
    pub weapon: u32,
    pub trail_played: bool,
    pub beacon_played: bool,
    pub ignition_played: bool,
    pub ignition_fx_played: bool,
}

/// bo2zm M3: Black Ops II script models (perk machines, the box,
/// power-ups...) the snapshot presents, drawn by the same fallback pass as
/// projectiles: each model of the mover's composition (its base and its
/// attachments) from the T6 catalog, in bind pose, where the mover is.
#[derive(Resource, Default, Clone)]
pub struct T6ScriptModels {
    pub items: Vec<T6MissileModel>,
}

fn collect_t6_script_models(
    fpv: Option<Res<assets::PreparedFpvMeshes>>,
    xanims: Option<Res<assets::PreparedXAnims>>,
    presented: Option<Res<net::PresentedSnapshot>>,
    cg_clock: Option<Res<net::FrameClock>>,
    owners: Query<(
        &render_scene::WorldScriptModelInstance,
        &Transform,
        &Visibility,
    )>,
    map_models: Option<Res<asset_world::MapXModelSceneCatalog>>,
    pieces: Query<(&render_scene::WorldDynEntInstance, &Transform, &Visibility)>,
    mut out: ResMut<T6ScriptModels>,
) {
    out.items.clear();
    let Some(fpv) = fpv else {
        return;
    };
    // How far the frame is past the snapshot the animation times came in,
    // so an animated model plays on smoothly between snapshots.
    let since_snapshot = match (
        presented.as_ref().and_then(|p| p.snapshot()),
        cg_clock.as_ref(),
    ) {
        (Some(snapshot), Some(clock)) if clock.started() => {
            ((clock.time() - sim::level_time_ms(snapshot.tick)) as f32 / 1000.0).clamp(0.0, 0.25)
        }
        _ => 0.0,
    };
    let presented_states = presented
        .as_deref()
        .map(crate::occupancy::script_model::presented_dobj_states)
        .unwrap_or_default();
    // IW4L_T6_SMLOG=1: the script models every 5 s (debugging).
    static SMLOG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let log = *SMLOG.get_or_init(|| std::env::var_os("IW4L_T6_SMLOG").is_some()) && {
        static LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let last = LAST.load(std::sync::atomic::Ordering::Relaxed);
        now >= last + 5
            && LAST
                .compare_exchange(
                    last,
                    now,
                    std::sync::atomic::Ordering::Relaxed,
                    std::sync::atomic::Ordering::Relaxed,
                )
                .is_ok()
    };
    for (owner, transform, visibility) in &owners {
        if log {
            let names: Vec<&str> = owner
                .dobj_state
                .composition
                .models
                .iter()
                .map(|d| d.model.as_str())
                .collect();
            if names
                .iter()
                .any(|n| n.contains("vending") || n.contains("packapunch"))
            {
                diag::info!(
                    World,
                    "bo2zm t6 smlog: {:?} at {:?} {:?}",
                    names,
                    transform.translation,
                    visibility
                );
            }
        }
        if matches!(visibility, Visibility::Hidden) {
            continue;
        }
        let world_from_local = transform.to_matrix();
        let origin = transform.translation.to_array();
        // bo2zm M3 fix list 3: the animation times from the same snapshot
        // the frame's time is measured from. The owner's copy of them is
        // refreshed a frame after a new snapshot lands, so for that frame
        // the old times were carried on from the new snapshot's start and
        // the pose stepped back: every zombie's walk hitched 20 times a
        // second, his "the way they walk is just super glitchy looking".
        let state = owner
            .authority_owner
            .and_then(|o| presented_states.get(&o).copied())
            .unwrap_or(&owner.dobj_state);
        // The light is read at the middle of its first model (a zombie's
        // body): its bounds centre, placed.
        let light_at = state
            .composition
            .models
            .first()
            .and_then(|d| fpv.0.get(asset_core::AssetNamespace::T6, &d.model))
            .and_then(|e| e.skel.bounds)
            .map_or(origin, |(lo, hi)| {
                let mid = Vec3::new(
                    (lo[0] + hi[0]) * 0.5,
                    (lo[1] + hi[1]) * 0.5,
                    (lo[2] + hi[2]) * 0.5,
                );
                world_from_local.transform_point3(mid).to_array()
            });
        // IW4L_T6_POSE_LOG=<clip part>: the first model seen playing such a
        // clip, each frame: time past the snapshot, clip time sent, place.
        {
            static WANT: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
            static PICKED: std::sync::atomic::AtomicU32 =
                std::sync::atomic::AtomicU32::new(u32::MAX);
            if let Some(want) = WANT.get_or_init(|| std::env::var("IW4L_T6_POSE_LOG").ok()) {
                let leaf = state
                    .tree
                    .as_ref()
                    .and_then(|t| t.nodes.iter().find(|n| n.clip.is_some()))
                    .map(|n| {
                        (
                            n.clip.clone().unwrap_or_default(),
                            n.state.time,
                            n.state.rate,
                        )
                    });
                let me = owner.gentity_number.map_or(u32::MAX, u32::from);
                let picked = PICKED.load(std::sync::atomic::Ordering::Relaxed);
                let mine = picked == me
                    || (picked == u32::MAX
                        && leaf
                            .as_ref()
                            .is_some_and(|(c, _, _)| c.contains(want.as_str()))
                        && {
                            PICKED.store(me, std::sync::atomic::Ordering::Relaxed);
                            true
                        });
                if mine {
                    let clock = cg_clock.as_ref().map_or(0, |c| c.time());
                    diag::info!(
                        World,
                        "t6 pose #{me}: clock {clock} since {since_snapshot:.4} leaf {leaf:?} drawn {:.4} at ({:.2} {:.2} {:.2})",
                        leaf.as_ref().map_or(0.0, |l| l.1 + l.2 * since_snapshot),
                        origin[0],
                        origin[1],
                        origin[2]
                    );
                }
            }
        }
        let posed = xanims
            .as_ref()
            .and_then(|x| pose_t6_composition(&fpv.0, &x.0, state, since_snapshot));
        let hide = *state.hide_part_bits.words();
        let mut bone_base = 0u32;
        for (i, desc) in state.composition.models.iter().enumerate() {
            let base = bone_base;
            let Some(entry) = fpv.0.get(asset_core::AssetNamespace::T6, &desc.model) else {
                // Once per name: a BO2 model the catalog lacks draws nothing.
                static MISSING: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
                if let Ok(mut m) = MISSING.lock()
                    && !m.contains(&desc.model)
                {
                    m.push(desc.model.clone());
                    diag::warn!(
                        World,
                        "bo2zm t6 script model not in the catalog: {}",
                        desc.model
                    );
                }
                continue;
            };
            bone_base += u32::try_from(entry.skel.bones.len()).unwrap_or(0);
            out.items.push(T6MissileModel {
                skel: entry.skel.clone(),
                materials: entry
                    .material_edges
                    .iter()
                    .map(|edge| edge.bound_index().map(|i| i as u32))
                    .collect(),
                world_from_local,
                origin,
                light_at,
                posed: posed.as_ref().and_then(|p| p.get(i).cloned().flatten()),
                hide: hide.iter().any(|w| *w != 0).then_some((hide, base)),
            });
        }
    }
    // bo2mp: loose pieces the map's own model list cannot draw (a
    // mannequin's broken-off head): their BO2 model from the same catalog,
    // where the piece's physics has it.
    for (piece, transform, visibility) in &pieces {
        if piece.dead || matches!(visibility, Visibility::Hidden) {
            continue;
        }
        let drawn_by_map = map_models.as_deref().is_some_and(|map| {
            !matches!(
                map.get(&piece.current_model),
                None | Some(asset_world::MapXModelSceneAsset::Unavailable { .. })
            )
        });
        if drawn_by_map {
            continue;
        }
        let entry = fpv.0.get(asset_core::AssetNamespace::T6, &piece.current_model.0);
        let origin = transform.translation.to_array();
        if std::env::var_os("IW4L_T6_HITLOG").is_some() {
            static SEEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = SEEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if n % 30 == 0 && n < 600 {
                diag::info!(
                    World,
                    "bo2mp debris {} {} at {origin:?}",
                    piece.current_model.0,
                    if entry.is_some() { "draws" } else { "has no model to draw" }
                );
            }
        }
        let Some(entry) = entry else {
            continue;
        };
        out.items.push(T6MissileModel {
            skel: entry.skel.clone(),
            materials: entry
                .material_edges
                .iter()
                .map(|edge| edge.bound_index().map(|i| i as u32))
                .collect(),
            world_from_local: transform.to_matrix(),
            origin,
            light_at: origin,
            posed: None,
            hide: None,
        });
    }
}

/// bo2zm M3: pose a composition (a zombie's body and head) with its
/// animation tree's leaves and skin every model; `None` when it has no
/// animation (it draws in bind pose).
fn pose_t6_composition(
    fpv: &asset_model::FpvMeshCatalog,
    xanims: &asset_anim::XAnimCatalog,
    state: &xmodel_runtime::DObjSemanticState,
    since_snapshot: f32,
) -> Option<Vec<Option<std::sync::Arc<PosedVerts>>>> {
    use xmodel_runtime::{AnimInstance, Attach, DObj, XAnimSemanticNodeKind};
    let tree = state.tree.as_ref()?;
    let leaves: Vec<_> = tree
        .nodes
        .iter()
        .filter(|n| n.kind == XAnimSemanticNodeKind::Leaf && n.clip.is_some())
        .collect();
    if leaves.is_empty() {
        return None;
    }
    let entries: Vec<&asset_model::FpvMeshEntry> = state
        .composition
        .models
        .iter()
        .map(|d| fpv.get(asset_core::AssetNamespace::T6, &d.model))
        .collect::<Option<Vec<_>>>()?;
    let srcs: Vec<&xmodel_runtime::ModelPoseSrc> = entries
        .iter()
        .map(|e| e.skel.pose.as_ref())
        .collect::<Option<Vec<_>>>()?;
    let models: Vec<(&xmodel_runtime::ModelPoseSrc, Option<Attach>)> = state
        .composition
        .models
        .iter()
        .zip(&srcs)
        .map(|(d, src)| {
            let attach = d.parent_model.map(|p| {
                let parent = usize::from(p);
                // An empty tag joins at the child's root bone (a zombie's
                // head at the body's j_spine4); same-named bones then
                // follow the parent's.
                let tag = match d.attach_tag.as_deref() {
                    Some(t) if !t.is_empty() => t.to_owned(),
                    _ => srcs
                        .get(parent)
                        .map(|p| xmodel_runtime::empty_tag_attach(p, src))
                        .unwrap_or_default(),
                };
                Attach {
                    parent_model: parent,
                    tag,
                }
            });
            (*src, attach)
        })
        .collect();
    let dobj = DObj::build(&models).ok()?;
    let clips: Vec<(
        std::sync::Arc<xmodel_runtime::AnimClip>,
        Vec<Option<usize>>,
        f32,
        f32,
    )> = leaves
        .iter()
        .filter_map(|leaf| {
            let clip = xanims.clip(asset_core::AssetNamespace::T6, leaf.clip.as_deref()?)?;
            let tracks = clip.tracks.iter().map(|t| dobj.find(&t.name)).collect();
            let duration = clip.duration().max(1e-3);
            let mut frac = leaf.state.time + leaf.state.rate * since_snapshot;
            frac = if clip.looping {
                frac.rem_euclid(1.0)
            } else {
                frac.clamp(0.0, 1.0)
            };
            let weight = if leaves.len() == 1 {
                1.0
            } else {
                leaf.state.weight
            };
            Some((clip, tracks, frac * duration, weight))
        })
        .collect();
    if clips.is_empty() {
        return None;
    }
    let instances: Vec<AnimInstance<'_>> = clips
        .iter()
        .map(|(clip, tracks, time, weight)| AnimInstance {
            clip,
            tracks,
            time: *time,
            weight: *weight,
            parts: None,
        })
        .collect();
    let world = dobj.pose(&instances, &dobj.all_parts(), Mat4::IDENTITY);
    let skin = dobj.skin_matrices(&world);
    Some(
        entries
            .iter()
            .enumerate()
            .map(|(mi, entry)| {
                let base = dobj.models.get(mi)?.base;
                Some(std::sync::Arc::new(skin_verts(&entry.skel, |b| {
                    skin.get(base + b).copied().unwrap_or(Mat4::IDENTITY)
                })))
            })
            .collect(),
    )
}

/// CPU skinning: each vertex by its bones and weights (rigid models by
/// their first bone).
pub(crate) fn skin_verts(skel: &asset_model::ModelSkel, bone: impl Fn(usize) -> Mat4) -> PosedVerts {
    let n = skel.positions.len();
    let mut positions = Vec::with_capacity(n);
    let mut normals = Vec::with_capacity(n);
    let blended = skel.vert_skin.len() == n;
    let rigid = bone(0);
    for v in 0..n {
        let p = Vec3::from_array(skel.positions[v]);
        let nn = Vec3::from_array(skel.normals.get(v).copied().unwrap_or([0.0, 0.0, 1.0]));
        let (mut op, mut on) = (Vec3::ZERO, Vec3::ZERO);
        if blended {
            let s = &skel.vert_skin[v];
            let mut total = 0.0;
            for k in 0..4 {
                let w = s.weights[k];
                if w <= 0.0 {
                    continue;
                }
                let m = bone(usize::from(s.bones[k]));
                op += m.transform_point3(p) * w;
                on += m.transform_vector3(nn) * w;
                total += w;
            }
            if total <= 0.0 {
                op = rigid.transform_point3(p);
                on = rigid.transform_vector3(nn);
            }
        } else {
            op = rigid.transform_point3(p);
            on = rigid.transform_vector3(nn);
        }
        positions.push(op.to_array());
        normals.push(on.normalize_or_zero().to_array());
    }
    PosedVerts { positions, normals }
}

pub fn register_missile_systems(app: &mut App) {
    app.init_resource::<MissileDrawPlan>()
        .init_resource::<T6MissileModels>()
        .init_resource::<T6ScriptModels>()
        .add_systems(
            Update,
            collect_t6_missiles
                .in_set(frame::RenderSet::Anim)
                .before(frame::WorkerCmdSet::CellDynModel),
        )
        // After the script models take this frame's snapshot: their place
        // was otherwise this frame's or last frame's, whichever system the
        // scheduler ran first (fix list 3).
        .add_systems(
            Update,
            collect_t6_script_models
                .in_set(frame::RenderSet::Anim)
                .after(crate::occupancy::script_model::ScriptModelDrawSet)
                .before(frame::WorkerCmdSet::CellDynModel),
        )
        .init_resource::<MissileOccupancy>()
        .init_resource::<MissilePoseProduct>()
        .init_resource::<MissileBoltState>()
        .add_systems(
            Update,
            occupy_missile_scene_ents
                .in_set(frame::RenderSet::Anim)
                .before(frame::WorkerCmdSet::CellDynModel)
                .in_set(render_scene::GfxSceneAdd)
                .in_set(AnimSceneSubmit),
        )
        .add_systems(
            Update,
            publish_t6_script_model_poses
                .after(frame::WorkerCmdSet::SkinModel)
                .before(frame::WorkerCmdSet::FxRemaining)
                .in_set(frame::ClientSet::Present),
        )
        .add_systems(
            Update,
            publish_missile_dobj_poses
                .after(occupy_missile_scene_ents)
                .after(frame::WorkerCmdSet::SkinModel)
                .before(frame::WorkerCmdSet::FxRemaining)
                .in_set(frame::ClientSet::Present),
        )
        .add_systems(
            Update,
            (pose_missiles, append_missile_draws)
                .chain()
                .after(frame::WorkerCmdSet::CellSceneEnt)
                .after(frame::WorkerCmdSet::DpvsEnt)
                .before(frame::WorkerCmdSet::CellDynModel)
                .in_set(frame::WorkerCmdSet::SkinModel),
        );
}

/// bo2zm: every Black Ops II projectile the snapshot presents, with its
/// model from the first-person catalog (the projectile catalog has none).
fn collect_t6_missiles(
    presented: Option<Res<net::PresentedSnapshot>>,
    weapons: Option<Res<assets::PreparedWeapons>>,
    fpv: Option<Res<assets::PreparedFpvMeshes>>,
    cg_clock: Option<Res<net::FrameClock>>,
    mut out: ResMut<T6MissileModels>,
) {
    out.items.clear();
    let (Some(snapshot), Some(weapons), Some(fpv)) =
        (presented.as_ref(), weapons.as_ref(), fpv.as_ref())
    else {
        return;
    };
    let reg = &weapons.0;
    let rows = snapshot.presented_projectiles();
    if rows.is_empty() {
        return;
    }
    let at_time = cg_clock
        .as_ref()
        .filter(|clock| clock.started())
        .map(|clock| clock.time())
        .unwrap_or_else(|| snapshot.tick().map(sim::level_time_ms).unwrap_or(0));
    let at_time = snapshot.trajectory_time_ms(at_time);
    for row in rows.iter() {
        if reg.namespace_of(row.weapon()) != Some(asset_core::AssetNamespace::T6)
            || entity_iw4::missile_nodraw(0, row.launch_time(), at_time).is_some()
        {
            continue;
        }
        let Some(name) = reg.projectile_model_of(row.weapon()) else {
            continue;
        };
        let Some(entry) = fpv.0.get(asset_core::AssetNamespace::T6, name) else {
            continue;
        };
        let origin = match row {
            net::PresentedProjectile::Authoritative(projectile) => {
                snapshot.projectile_origin_at(projectile, at_time)
            }
            net::PresentedProjectile::Predicted { .. } => row.origin_at(at_time),
        };
        let angles = entity_iw4::evaluate_trajectory(&row.apos(), at_time);
        out.items.push(T6MissileModel {
            skel: entry.skel.clone(),
            materials: entry
                .material_edges
                .iter()
                .map(|edge| edge.bound_index().map(|i| i as u32))
                .collect(),
            world_from_local: missile_world_from_local(origin, angles),
            origin,
            light_at: origin,
            posed: None,
            hide: None,
        });
    }
}

pub fn missile_lighting_origin(origin: [f32; 3]) -> [f32; 3] {
    [origin[0], origin[1], origin[2] + MISSILE_LIGHTING_Z_OFS]
}

pub fn missile_pose_catalog(
    assets: Option<&assets::PreparedProjectileMeshes>,
) -> Option<&asset_model::ProjectileMeshCatalog> {
    assets
        .map(|prepared| &prepared.0)
        .filter(|catalog| !catalog.is_empty())
}

pub fn missile_lighting_atlas<'a>(
    atlas: Option<&'a WorldModelLightingAtlas>,
    scene_atlas: Option<&'a WorldModelLightingAtlas>,
) -> Option<&'a WorldModelLightingAtlas> {
    atlas.or(scene_atlas)
}

fn occupy_missile_scene_ents(
    presented: Option<Res<net::PresentedSnapshot>>,
    weapons: Option<Res<assets::PreparedWeapons>>,
    projectile_meshes: Option<Res<assets::PreparedProjectileMeshes>>,
    mut occupancy: ResMut<MissileOccupancy>,
    mut scene_skels: ResMut<AnimDObjSceneSkels>,
    mut scene_submissions: MessageWriter<AnimDObjSceneSubmission>,
    cg_clock: Option<Res<net::FrameClock>>,
    local: Option<Res<net::LocalPresentClient>>,
) {
    occupancy.rows.clear();
    let Some(snapshot) = presented.as_ref() else {
        return;
    };
    let rows = snapshot.presented_projectiles();
    let at_time = cg_clock
        .as_ref()
        .filter(|clock| clock.started())
        .map(|clock| clock.time())
        .unwrap_or_else(|| snapshot.tick().map(sim::level_time_ms).unwrap_or(0));
    let at_time = snapshot.trajectory_time_ms(at_time);
    let weapons_reg = weapons.as_ref().map(|w| &w.0).filter(|reg| !reg.is_empty());
    let catalog = missile_pose_catalog(projectile_meshes.as_deref());
    let Some(catalog) = catalog else {
        return;
    };
    let piloted = local
        .and_then(|local| {
            snapshot
                .snapshot()?
                .meta
                .for_client(local.0)?
                .remote_missile
        })
        .map(|link| link.projectile);
    for (index, row) in rows.iter().enumerate() {
        if entity_iw4::missile_nodraw(0, row.launch_time(), at_time).is_some() {
            continue;
        }
        if piloted.is_some() && row.authoritative_id() == piloted {
            continue;
        }
        let model = weapons_reg.and_then(|reg| reg.projectile_model_of(row.weapon()));
        let Some(name) = model else {
            continue;
        };
        let ns = weapons_reg
            .and_then(|reg| reg.namespace_of(row.weapon()))
            .unwrap_or(asset_core::AssetNamespace::Iw4);
        let Some(entry) = catalog.get(ns, name) else {
            continue;
        };
        let Some(_pose) = entry.skel.pose.as_ref() else {
            continue;
        };
        let origin = match row {
            net::PresentedProjectile::Authoritative(projectile) => {
                snapshot.projectile_origin_at(projectile, at_time)
            }
            net::PresentedProjectile::Predicted { .. } => row.origin_at(at_time),
        };
        let lighting_origin = missile_lighting_origin(origin);
        let angles = entity_iw4::evaluate_trajectory(&row.apos(), at_time);
        let entnum = match row {
            net::PresentedProjectile::Authoritative(projectile) => {
                u32::try_from(projectile.entnum).ok()
            }
            net::PresentedProjectile::Predicted { .. } => None,
        };
        if let Some(entnum) = entnum {
            scene_submissions.write(AnimDObjSceneSubmission {
                render_fx_flags: 0,
                has_tree: false,
                origin,
                lighting_origin,
                radius: entry.skel.radius,
                entnum,
                quat: Some(scene_quat_from_angles(angles)),
                occupy_model_n: 1,
                models: vec![scene_skels.shared(name, &entry.skel, 0)],
                hide_part_bits: [0; 6],
                store_skin: true,
            });
        }
        let lighting_owner = match row.authoritative_id() {
            Some(id) => ModelLightingOwner::Missile(id.0),
            None => ModelLightingOwner::PredictedMissile {
                owner: row.owner().0,
                weapon: row.weapon(),
            },
        };
        occupancy.rows.push(OccupiedMissile {
            index,
            id: row.authoritative_id().map(|id| id.0),
            entnum,
            weapon: row.weapon(),
            ignited: match row {
                net::PresentedProjectile::Authoritative(p) => weapons_reg
                    .and_then(|reg| reg.facts_of(p.weapon))
                    .is_none_or(|facts| {
                        at_time >= p.spawn_time_ms.saturating_add(facts.ignition_delay_ms)
                    }),
                net::PresentedProjectile::Predicted { .. } => false,
            },
            origin,
            angles,
            name: name.to_owned(),
            namespace: ns,
            lighting_origin,
            lighting_owner,
        });
    }
}

/// bo2zm M3: every BO2 script model (by its entity number) publishes its
/// bones (bind pose) where it stands, so an effect played on one of its
/// tags (`playfxontag`: a power-up's glow, a perk machine's light) rides
/// it, and goes when it goes.
fn publish_t6_script_model_poses(
    fpv: Option<Res<assets::PreparedFpvMeshes>>,
    xanims: Option<Res<assets::PreparedXAnims>>,
    owners: Query<(&render_scene::WorldScriptModelInstance, &Transform)>,
    mut dobj_poses: ResMut<crate::anim::dobj_pose::HostDObjPoseFrame>,
    (cg_clock, presented): (
        Option<Res<net::FrameClock>>,
        Option<Res<net::PresentedSnapshot>>,
    ),
) {
    let Some(fpv) = fpv else {
        return;
    };
    // bo2zm M3 fix list 2: the bones at the client's clock, as the body is
    // drawn (the zombies' eye glow rode a pose made 20 times a second).
    let ahead =
        crate::occupancy::script_model::anim_ahead_secs(cg_clock.as_deref(), presented.as_deref());
    let presented_states = presented
        .as_deref()
        .map(crate::occupancy::script_model::presented_dobj_states)
        .unwrap_or_default();
    for (owner, transform) in &owners {
        let Some(entnum) = owner.gentity_number else {
            continue;
        };
        // The times from the snapshot `ahead` is measured from (fix list 3).
        let current = owner
            .authority_owner
            .and_then(|o| presented_states.get(&o).copied())
            .unwrap_or(&owner.dobj_state);
        let advanced =
            crate::occupancy::script_model::advance_script_anim(current, ahead, |clip| {
                xanims
                    .as_deref()?
                    .0
                    .clip(asset_core::AssetNamespace::T6, clip)
                    .map(|c| c.looping)
            });
        let state = advanced.as_ref().unwrap_or(current);
        let Some(bones) = t6_composition_bones(&fpv.0, xanims.as_deref().map(|x| &x.0), state)
        else {
            continue;
        };
        let _ = dobj_poses.publish(u32::from(entnum), true, 0, transform.to_matrix(), &bones);
    }
}

/// bo2zm M3: a composition's bones in model space (its base model's, then
/// each attachment's, the way the server numbers them for an effect's
/// tag), posed by its animation tree's first leaf when it has one.
fn t6_composition_bones(
    fpv: &asset_model::FpvMeshCatalog,
    xanims: Option<&asset_anim::XAnimCatalog>,
    state: &xmodel_runtime::DObjSemanticState,
) -> Option<Vec<Mat4>> {
    use xmodel_runtime::{AnimInstance, Attach, DObj, XAnimSemanticNodeKind};
    let srcs: Vec<&xmodel_runtime::ModelPoseSrc> = state
        .composition
        .models
        .iter()
        .map(|d| {
            fpv.get(asset_core::AssetNamespace::T6, &d.model)
                .and_then(|e| e.skel.pose.as_ref())
        })
        .collect::<Option<Vec<_>>>()?;
    let models: Vec<(&xmodel_runtime::ModelPoseSrc, Option<Attach>)> = state
        .composition
        .models
        .iter()
        .zip(&srcs)
        .map(|(d, src)| {
            let attach = d.parent_model.map(|p| {
                let parent = usize::from(p);
                let tag = match d.attach_tag.as_deref() {
                    Some(t) if !t.is_empty() => t.to_owned(),
                    _ => srcs
                        .get(parent)
                        .map(|p| xmodel_runtime::empty_tag_attach(p, src))
                        .unwrap_or_default(),
                };
                Attach {
                    parent_model: parent,
                    tag,
                }
            });
            (*src, attach)
        })
        .collect();
    let dobj = DObj::build(&models).ok()?;
    let leaf = state.tree.as_ref().and_then(|t| {
        t.nodes
            .iter()
            .find(|n| n.kind == XAnimSemanticNodeKind::Leaf && n.clip.is_some())
    });
    let clip = leaf.and_then(|l| xanims?.clip(asset_core::AssetNamespace::T6, l.clip.as_deref()?));
    let bones = match (leaf, clip) {
        (Some(l), Some(clip)) => {
            let tracks: Vec<Option<usize>> =
                clip.tracks.iter().map(|t| dobj.find(&t.name)).collect();
            let frac = if clip.looping {
                l.state.time.rem_euclid(1.0)
            } else {
                l.state.time.clamp(0.0, 1.0)
            };
            let inst = AnimInstance {
                clip: &clip,
                tracks: &tracks,
                time: frac * clip.duration().max(1e-3),
                weight: 1.0,
                parts: None,
            };
            dobj.pose(&[inst], &dobj.all_parts(), Mat4::IDENTITY)
        }
        _ => dobj.pose(&[], &dobj.all_parts(), Mat4::IDENTITY),
    };
    Some(bones)
}

fn publish_missile_dobj_poses(
    occupancy: Res<MissileOccupancy>,
    prepared: Res<crate::anim::model_materials::PreparedModelMaterials>,
    mut dobj_poses: ResMut<crate::anim::dobj_pose::HostDObjPoseFrame>,
) {
    for row in &occupancy.rows {
        let Some(entnum) = row.entnum else {
            continue;
        };
        let Some(dobj) = prepared.projectile_dobj(row.namespace, &row.name) else {
            continue;
        };
        let entity_world = missile_world_from_local(row.origin, row.angles);
        let Ok(local) = xmodel_runtime::pose_dobj_with_controller(
            dobj,
            &xmodel_runtime::DObjPoseRequest::bind_pose(),
            Mat4::IDENTITY,
            |_, _, _| {},
        ) else {
            continue;
        };

        let _ = dobj_poses.publish(entnum, true, 0, entity_world, &local);
    }
}

fn pose_missiles(
    occupancy: Res<MissileOccupancy>,
    projectile_meshes: Option<Res<assets::PreparedProjectileMeshes>>,
    gfx: Res<HostGfxScene>,
    prepared: Res<crate::anim::model_materials::PreparedModelMaterials>,
    mut product: ResMut<MissilePoseProduct>,
) {
    product.rows.clear();
    let catalog = missile_pose_catalog(projectile_meshes.as_deref());
    let Some(catalog) = catalog else {
        return;
    };
    for row in &occupancy.rows {
        let slot = row
            .entnum
            .map(|entnum| missile_scene_slot(&gfx.scene, entnum))
            .unwrap_or(MissileSceneSlot {
                hidden: false,
                skin_entries: &[],
            });
        if slot.hidden {
            continue;
        }
        let Some(entry) = catalog.get(row.namespace, &row.name) else {
            continue;
        };
        let Some(dobj) = prepared.projectile_dobj(row.namespace, &row.name) else {
            continue;
        };
        let dobj_state = xmodel_runtime::DObjSemanticState::bind_pose(row.name.clone(), 1, 1);
        let Ok(request) = dobj_state.resolve_request(|_| None) else {
            continue;
        };
        let Some((surfaces, _)) = pose_script_dobj_with_materials(
            None,
            &[&entry.skel],
            dobj,
            &request,
            None,
            slot.skin_entries,
        ) else {
            continue;
        };
        product.rows.push(MissilePosed {
            index: row.index,
            entnum: row.entnum,
            origin: row.origin,
            angles: row.angles,
            name: row.name.clone(),
            namespace: row.namespace,
            lighting_origin: row.lighting_origin,
            lighting_owner: row.lighting_owner,
            surfaces,
        });
    }
}

fn append_missile_draws(
    product: Res<MissilePoseProduct>,
    projectile_meshes: Option<Res<assets::PreparedProjectileMeshes>>,
    atlas: Option<Res<WorldModelLightingAtlas>>,
    atpoint: Res<render_scene::DynAtPointLookup>,
    tess: Option<Res<TessMaterials>>,
    facts: Res<WorldPresentFacts>,
    mut plan: ResMut<MissileDrawPlan>,
    mut lighting_requests: ResMut<ModelLightingRequests>,
    prepared: Res<crate::anim::model_materials::PreparedModelMaterials>,
    mut staged: Local<MissileDrawPlan>,
    mut draws: Local<Vec<XModelSurfaceDraw>>,
    mut owners: Local<Vec<MissileOwnerDraw>>,
) {
    // Missiles are rebuilt from scratch every frame, so the rebuild goes to a
    // staging plan whose allocations survive the frame; `plan` keeps the
    // revisions, which is the part a consumer reads.
    let staging = &mut *staged;
    staging.clear_rebuild();
    draws.clear();
    owners.clear();
    let catalog = missile_pose_catalog(projectile_meshes.as_deref());
    let atlas_ref = missile_lighting_atlas(atlas.as_deref(), None);
    if catalog.is_none() || atlas_ref.is_none() || !facts.spawned || tess.is_none() {
        plan.publish_rebuild(staging, &mut draws, &mut owners);
        return;
    }
    let (Some(catalog), Some(_), Some(tess)) = (catalog, atlas_ref, tess.as_deref()) else {
        unreachable!("required missile draw resources checked above");
    };
    for row in &product.rows {
        let Some(entry) = catalog.get(row.namespace, &row.name) else {
            continue;
        };
        let materials: Vec<Option<SmodelPassMaterial>> = row
            .surfaces
            .iter()
            .map(|surface| {
                let authored = entry.material_index(surface.surface_index)?;
                prepared
                    .projectile_material(&tess.catalog, authored)
                    .cloned()
            })
            .collect();
        if materials.iter().all(Option::is_none) {
            continue;
        }
        let box_half = entry
            .skel
            .radius
            .and_then(|radius| lighting_box_half(&[radius], &[DOBJ_RADIUS_PARENT_ROOT]));
        let lookup_fallback = atpoint.fallback(row.lighting_origin, box_half);
        let pending_lighting = Some(lighting_requests.request(ModelLightingRequest {
            owner: row.lighting_owner,
            origin: row.lighting_origin,
            lookup_fallback,
        }));
        let surfaces_idx = append_missile_surfaces(staging, &row.surfaces, &materials);
        if surfaces_idx.is_empty() {
            continue;
        }
        let object_id = XMODEL_OBJECT_ID_MISSILE_BASE.saturating_add(row.index as u16);
        owners.push(MissileOwnerDraw {
            object_id,
            model: row.name.clone(),
        });
        let world_from_local = missile_world_from_local(row.origin, row.angles);
        let caster_bound = entry
            .skel
            .radius
            .map(|radius| render_scene::XModelCasterBound {
                origin: row.origin,
                radius: radius.max(1.0),
            });
        for (surface, material) in surfaces_idx {
            draws.push(XModelSurfaceDraw {
                surface,
                material,
                world_from_local,
                lighting_handle: 0,
                pending_lighting,
                colour_refusal: None,
                object_id,
                scene_light_index: 0,
                reflection_probe_index: 0,
                packed_lighting: None,
                is_scope: false,
                scene_entnum: row.entnum,
                caster_bound,
            });
        }
    }
    plan.publish_rebuild(staging, &mut draws, &mut owners);
}

pub(crate) fn missile_world_from_local(origin: [f32; 3], angles: [f32; 3]) -> Mat4 {
    let [pitch, yaw, roll] = angles;
    Transform {
        translation: Vec3::from_array(origin),
        rotation: Quat::from_euler(
            EulerRot::ZYX,
            yaw.to_radians(),
            pitch.to_radians(),
            roll.to_radians(),
        ),
        scale: Vec3::ONE,
    }
    .to_matrix()
}
