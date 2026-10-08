//! bo2mp: other players, their corpses and the guns and bags on the ground,
//! as Black Ops II draws them, through the same fallback pass as its
//! projectiles and script models (BO2's own materials, lit by the map's
//! light grid).
//!
//! - A player's body is the model BO2's faction scripts gave him
//!   (`setmodel` in `mpbody/class_*`, sent as his `bo2mp_body` client
//!   value), the gun in his hands its BO2 world model (with its attachments)
//!   on the body's `tag_weapon_right`.
//! - His pose: the legs and torso animations the server picked from BO2's
//!   own `mp/playeranim.script` (his state's `legs_anim` / `torso_anim`),
//!   each faded in over its blend time; the torso's animation drives the
//!   upper body, the legs' the rest; his view's pitch bends the spine
//!   through the body's controller tags (`back_low`, `back_mid`, `back_up`).
//!   Nothing is ever drawn in bind pose: a part with no animation plays
//!   the script's standing idle.
//! - A corpse (the engine's clone of a dead player) plays his death
//!   animation to its end and stays in its last pose.
//! - A dropped gun or Scavenger bag: its world model and attachments where
//!   the item lies (a bag only for players with Scavenger, as BO2 shows it).

use std::collections::HashMap;
use std::sync::Arc;

use bevy::prelude::*;
use entity_iw4::{ET_ITEM, ET_PLAYER_CORPSE, TR_GRAVITY, Trajectory, evaluate_trajectory};
use frame::ViewSubject;
use net::{CEntity, CEntityRuntime, FrameClock, LocalPresentClient, PlayerDrawGate, PresentedSnapshot};
use playerstate_iw4::{PERK_SCAVENGER, eflags};
use sim::t6_playeranim::{
    T6AnimFacts, T6PlayerAnims, T6PlayerAnimsRes, t6_anim_duration, t6_anim_index, t6_anim_part,
    t6_legs_yaw,
};

use crate::occupancy::missile::{PosedVerts, T6MissileModel, skin_verts};

/// This frame's BO2 player bodies, corpses and ground items, for the
/// fallback pass.
#[derive(Resource, Default, Clone)]
pub struct T6BodyModels {
    pub items: Vec<T6MissileModel>,
}

/// The client value that names a player's body (see the server's
/// `t6::playeranim`): `<model>` then `;<model>@<tag>` per attachment.
const BODY_DVAR: &str = "bo2mp_body";

/// How fast the drawn legs ease to the server's legs yaw (degrees/s).
const LEGS_YAW_SPEED_DEG: f32 = 360.0;

/// How long a dropped gun takes to tip onto its side as it falls (ms).
const ITEM_TIP_MS: f32 = 250.0;

/// A part's blend when the script names none (ms).
const DEFAULT_BLEND_MS: i32 = 150;

/// The torso's animation outweighs the legs' on the bones both move (the
/// weights are normalised per bone): the legs' only show where the torso's
/// has no track.
const TORSO_OVER_LEGS: f32 = 1000.0;

/// This frame's posed players, for the effects bolted to their guns (muzzle
/// flashes, shell casings) and for tracers' start: each one's bones (in its
/// own space), where it is, and its gun's flash and brass tags.
#[derive(Resource, Default)]
struct T6BodyPoses {
    rows: Vec<PosedBody>,
    local_corpse_root: Option<[f32; 3]>,
}

/// Where the local player's own corpse's `j_mainroot` is in the world this
/// frame (the death-watch camera's origin while he is dead, as the engine's
/// `CG_OffsetThirdPersonView` takes it), or none with no such corpse.
#[derive(Resource, Default)]
pub struct LocalCorpseRoot(pub Option<[f32; 3]>);

struct PosedBody {
    entity: Entity,
    number: u16,
    world_from_local: Mat4,
    bones: Vec<Mat4>,
    flash: Option<u16>,
    brass: Option<u16>,
}

/// A system that took long logs it (a frame's stall named).
struct SlowGuard(std::time::Instant, &'static str);

impl Drop for SlowGuard {
    fn drop(&mut self) {
        let ms = self.0.elapsed().as_secs_f64() * 1000.0;
        if ms > 20.0 {
            diag::warn!(World, "bo2mp slow: {} took {ms:.1} ms", self.1);
        }
    }
}

/// When each part of the last frame began: First, PreUpdate, Update,
/// PostUpdate, Last (finding which part a stall is in).
static FRAME_MARKS: std::sync::Mutex<[Option<std::time::Instant>; 5]> = std::sync::Mutex::new([None; 5]);

fn mark_frame<const PART: usize>() {
    if let Ok(mut marks) = FRAME_MARKS.lock() {
        marks[PART] = Some(std::time::Instant::now());
    }
}

/// A frame that came long after the last one, logged with the time each
/// part of it took (finding stalls). "after Last" is the render world's
/// extraction and the wait for the frame to show.
fn log_frame_gaps(mut last: Local<Option<std::time::Instant>>) {
    let now = std::time::Instant::now();
    let marks = FRAME_MARKS.lock().map(|m| *m).unwrap_or([None; 5]);
    if let Some(prev) = *last {
        let ms = (now - prev).as_secs_f64() * 1000.0;
        if ms > 250.0 {
            let span = |a: Option<std::time::Instant>, b: Option<std::time::Instant>| match (a, b) {
                (Some(a), Some(b)) if b >= a => format!("{:.0}", (b - a).as_secs_f64() * 1000.0),
                _ => "?".to_owned(),
            };
            diag::warn!(
                World,
                "bo2mp frame gap: {ms:.0} ms (First {} ms, PreUpdate+fixed {} ms, Update {} ms, PostUpdate {} ms, after Last {} ms; Update stretches: {})",
                span(marks[0], marks[1]),
                span(marks[1], marks[2]),
                span(marks[2], marks[3]),
                span(marks[3], marks[4]),
                span(marks[4], Some(now)),
                slow_update_spans(),
            );
        }
    }
    *last = Some(now);
    mark_frame::<0>();
}

/// When each stretch of Update ended (its client sets in order), for the
/// late-frame log.
const UPDATE_SPANS: [&str; 15] = [
    "start", "Load", "Receive", "Reconcile", "Input", "Predict", "Send", "FrontendPrepare", "Anim", "Fx",
    "FrontendAssemble", "Gpu", "Ui", "Effects", "Diag",
];
static UPDATE_MARKS: std::sync::Mutex<[Option<std::time::Instant>; 15]> = std::sync::Mutex::new([None; 15]);

fn mark_update<const K: usize>() {
    if let Ok(mut marks) = UPDATE_MARKS.lock() {
        marks[K] = Some(std::time::Instant::now());
    }
}

/// The stretches of the last Update that took 10 ms or more.
fn slow_update_spans() -> String {
    let marks = UPDATE_MARKS.lock().map(|m| *m).unwrap_or([None; 15]);
    let mut out = Vec::new();
    for k in 1..marks.len() {
        if let (Some(a), Some(b)) = (marks[k - 1], marks[k])
            && b >= a
        {
            let ms = (b - a).as_secs_f64() * 1000.0;
            if ms >= 10.0 {
                out.push(format!("{} {ms:.0}", UPDATE_SPANS[k]));
            }
        }
    }
    out.join(", ")
}

fn register_update_marks(app: &mut App) {
    use frame::{ClientSet as C, RenderSet as R};
    app.add_systems(
        Update,
        (
            mark_update::<0>.before(C::Load),
            mark_update::<1>.after(C::Load).before(C::Receive),
            mark_update::<2>.after(C::Receive).before(C::Reconcile),
            mark_update::<3>.after(C::Reconcile).before(C::Input),
            mark_update::<4>.after(C::Input).before(C::Predict),
            mark_update::<5>.after(C::Predict).before(C::Send),
            mark_update::<6>.after(C::Send).before(C::Present),
            mark_update::<7>.after(R::FrontendPrepare).before(R::Anim),
            mark_update::<8>.after(R::Anim).before(R::Fx),
            mark_update::<9>.after(R::Fx).before(R::FrontendAssemble),
            mark_update::<10>.after(R::FrontendAssemble).before(R::Gpu),
            mark_update::<11>.after(C::Present).before(C::Ui),
            mark_update::<12>.after(C::Ui).before(C::Effects),
            mark_update::<13>.after(C::Effects).before(C::Diag),
            mark_update::<14>.after(C::Diag),
        ),
    );
}

pub fn register_t6_body_systems(app: &mut App) {
    register_update_marks(app);
    crate::occupancy::t6_sounds::register_t6_sound_systems(app);
    app.add_systems(First, log_frame_gaps)
        .add_systems(PreUpdate, mark_frame::<1>)
        .add_systems(Update, mark_frame::<2>)
        .add_systems(PostUpdate, mark_frame::<3>)
        .add_systems(Last, mark_frame::<4>);
    app.init_resource::<T6ItemShapes>()
        .add_systems(Update, prepare_t6_bodies.before(collect_t6_bodies));
    app.init_resource::<T6BodyModels>()
        .init_resource::<T6BodyPoses>()
        .init_resource::<LocalCorpseRoot>()
        .add_systems(
            Update,
            collect_t6_bodies
                .in_set(frame::RenderSet::Anim)
                .after(net::PresentedPublished)
                .before(frame::WorkerCmdSet::CellDynModel),
        )
        .add_systems(
            Update,
            publish_t6_body_poses
                .after(collect_t6_bodies)
                .after(crate::anim::dobj_pose::begin_dobj_pose_frame)
                .after(frame::WorkerCmdSet::SkinModel)
                .before(frame::WorkerCmdSet::FxRemaining)
                .in_set(frame::ClientSet::Present),
        );
}

/// Each posed player's bones go where the effect system looks for bolted
/// effects (his number), and his entity carries his gun's flash and brass
/// tags, as the engine's own remote bodies do: his muzzle flash and shell
/// casings play from his gun, his tracers start at its muzzle.
fn publish_t6_body_poses(
    poses: Res<T6BodyPoses>,
    mut corpse_root: ResMut<LocalCorpseRoot>,
    mut dobj_poses: ResMut<crate::anim::dobj_pose::HostDObjPoseFrame>,
    mut commands: Commands,
) {
    corpse_root.0 = poses.local_corpse_root;
    for row in &poses.rows {
        let _ = dobj_poses.publish(u32::from(row.number), true, 0, row.world_from_local, &row.bones);
        let target = |bone: Option<u16>| {
            let bone = bone?;
            let world = row.world_from_local * *row.bones.get(usize::from(bone))?;
            let o = xmodel_runtime::bone_orientation(&[world], 0).ok()?;
            Some(fx::FxBoltTarget {
                dobj: u32::from(row.number),
                bone,
                centity_teleport: false,
                orientation: fx::FxBoltOrientation {
                    origin: o.origin,
                    axis: o.axis,
                },
            })
        };
        commands
            .entity(row.entity)
            .try_insert(crate::occupancy::remote_body::RemoteFxBolts {
                flash: target(row.flash),
                brass: target(row.brass),
                knife: None,
                laser: None,
            });
    }
}

#[derive(Clone, Copy, Default, Debug)]
struct Playing {
    value: i32,
    /// How far into its clip (seconds of clip time).
    phase: f32,
}

#[derive(Clone, Copy, Default, Debug)]
struct PartTrack {
    cur: Playing,
    prev: Option<Playing>,
    switched: i32,
    blend_ms: i32,
}

impl PartTrack {
    fn see(&mut self, value: i32, now: i32, script: &T6PlayerAnims, first: bool) {
        let value = t6_anim_part(value);
        if !first && value == self.cur.value {
            return;
        }
        let blend = t6_anim_index(value)
            .and_then(|a| script.command_of(a))
            .and_then(|c| c.blend_ms)
            .or_else(|| {
                t6_anim_index(self.cur.value)
                    .and_then(|a| script.command_of(a))
                    .and_then(|c| c.blend_out_ms)
            })
            .unwrap_or(DEFAULT_BLEND_MS);
        self.prev = (!first && self.cur.value != 0).then_some(self.cur);
        self.cur = Playing { value, phase: 0.0 };
        self.switched = now;
        self.blend_ms = blend.max(1);
    }

    /// Both animations on by `dt` seconds at `rate`.
    fn advance(&mut self, dt: f32, rate: f32) {
        self.cur.phase += dt * rate;
        if let Some(prev) = self.prev.as_mut() {
            prev.phase += dt * rate;
        }
    }

    /// (playing, weight) pairs: the current one fading in over the previous.
    fn weighted(&self, now: i32) -> Vec<(Playing, f32)> {
        let w = ((now - self.switched) as f32 / self.blend_ms as f32).clamp(0.0, 1.0);
        let mut out = vec![(self.cur, w)];
        if let Some(prev) = self.prev
            && w < 1.0
        {
            out.push((prev, 1.0 - w));
        } else {
            out[0].1 = 1.0;
        }
        out
    }
}

#[derive(Clone, Copy, Default, Debug)]
struct BodyTrack {
    legs: PartTrack,
    torso: PartTrack,
    /// The legs' yaw offset as drawn (degrees), easing to the server's.
    legs_yaw: f32,
    seen: i32,
    corpse: bool,
    /// The legs' animation and its phase last frame (for its notetracks).
    heard: Option<Playing>,
}

#[derive(Default)]
struct BodyLocals {
    tracks: HashMap<u16, BodyTrack>,
    /// Each corpse's body: (entity, his client, death animation) -> body.
    corpse_bodies: HashMap<(u16, u32, i32), String>,
    last_now: i32,
    /// When each ground item was first seen (its tip onto its side).
    item_seen: HashMap<i32, i32>,
    logged: Vec<String>,
}

/// A weapon's shape on the ground: its world model and attachments placed
/// on it (bind pose), and how far across it is.
fn item_shape(
    registry: &asset_game::WeaponRegistry,
    catalog: &asset_model::WorldWeaponCatalog,
    weapon: u32,
) -> Option<Arc<ItemShape>> {
    let parts = gun_parts(registry, catalog, weapon, None, 0);
    if parts.is_empty() {
        return None;
    }
    let dobj = build_dobj(&parts)?;
    let world = dobj.pose(&[], &dobj.all_parts(), Mat4::IDENTITY);
    let posed = skin_all(&dobj, &parts, &world);
    let ys = posed.iter().flat_map(|v| v.positions.iter().map(|p| p[1]));
    let (low_y, high_y) = ys.fold((0.0f32, 0.0f32), |(lo, hi), y| (lo.min(y), hi.max(y)));
    Some(Arc::new(ItemShape {
        models: parts
            .into_iter()
            .zip(posed)
            .map(|(p, v)| (p.skel, p.materials, v))
            .collect(),
        low_y,
        high_y,
    }))
}

/// Every weapon's ground shape, built once (at map load, see
/// [`prepare_t6_bodies`]).
#[derive(Resource, Default)]
struct T6ItemShapes(HashMap<u32, Option<Arc<ItemShape>>>);

/// At map load, before the match: every animation BO2's player script names
/// decoded (in parallel), and every weapon's ground shape built, so the
/// first body or dropped gun of a kind costs nothing extra mid-match.
fn prepare_t6_bodies(
    script: Option<Res<T6PlayerAnimsRes>>,
    xanims: Option<Res<assets::PreparedXAnims>>,
    weapons: Option<Res<assets::PreparedWeapons>>,
    world_weapons: Option<Res<assets::PreparedWorldWeapons>>,
    mut shapes: ResMut<T6ItemShapes>,
    mut done: Local<Option<(usize, u64)>>,
) {
    let (Some(script), Some(xanims), Some(weapons), Some(world_weapons)) = (script, xanims, weapons, world_weapons)
    else {
        return;
    };
    let Some(script) = script.0.as_ref() else {
        return;
    };
    // Once per loaded map (its catalogs), when the weapons are bound to
    // its world models.
    if weapons.0.world_catalog_identity() != world_weapons.0.identity() {
        return;
    }
    let owner = (std::sync::Arc::as_ptr(&weapons.0) as usize, world_weapons.0.identity());
    if *done == Some(owner) {
        return;
    }
    *done = Some(owner);
    let started = std::time::Instant::now();
    let names: Vec<&str> = script.anims.iter().map(String::as_str).collect();
    let catalog = &xanims.0;
    let decoded = std::sync::atomic::AtomicUsize::new(0);
    let chunk = names.len().div_ceil(8).max(1);
    bevy::tasks::ComputeTaskPool::get().scope(|scope| {
        for part in names.chunks(chunk) {
            let decoded = &decoded;
            scope.spawn(async move {
                for name in part {
                    if catalog.clip(asset_core::AssetNamespace::T6, name).is_some() {
                        decoded.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            });
        }
    });
    let clips_ms = started.elapsed().as_secs_f64() * 1000.0;
    shapes.0.clear();
    let mut built = 0usize;
    if !world_weapons.0.is_empty() {
        for weapon in 1..=weapons.0.len() as u32 {
            let shape = item_shape(&weapons.0, &world_weapons.0, weapon);
            built += usize::from(shape.is_some());
            shapes.0.insert(weapon, shape);
        }
    }
    diag::info!(
        World,
        "bo2mp bodies prepared at load: {} of {} player animations decoded in {clips_ms:.0} ms, {built} weapon ground shapes; {:.0} ms in all",
        decoded.load(std::sync::atomic::Ordering::Relaxed),
        names.len(),
        started.elapsed().as_secs_f64() * 1000.0
    );
}

struct ItemShape {
    models: Vec<(Arc<asset_model::ModelSkel>, Vec<Option<u32>>, Arc<PosedVerts>)>,
    /// The gun's extent across (its own y): how far it lies above its
    /// origin when it rests on either side.
    low_y: f32,
    high_y: f32,
}

fn note_once(locals: &mut BodyLocals, line: String) {
    if !locals.logged.contains(&line) {
        diag::info!(World, "{line}");
        locals.logged.push(line);
    }
}

/// A model with surfaces that have no material (they draw nothing), once.
fn note_unbound(locals: &mut BodyLocals, part: &Part) {
    let missing = part.materials.iter().filter(|m| m.is_none()).count();
    if missing > 0 {
        note_once(
            locals,
            format!(
                "bo2mp body: {} has {missing} of {} surfaces with no material",
                part.skel.name,
                part.materials.len()
            ),
        );
    }
}

/// One model of a composition: its skeleton and per-surface materials.
struct Part {
    skel: Arc<asset_model::ModelSkel>,
    materials: Vec<Option<u32>>,
    attach: Option<xmodel_runtime::Attach>,
}

/// Per surface, its material in the catalog (none = not drawn).
macro_rules! materials_of {
    ($edges:expr) => {
        $edges
            .iter()
            .map(|edge| edge.bound_index().map(|i| i as u32))
            .collect::<Vec<Option<u32>>>()
    };
}

/// A weapon's world model and its attachments (on the gun's tags), the gun
/// itself attached to `parent` at `tag` (or the root of the composition).
fn gun_parts(
    registry: &asset_game::WeaponRegistry,
    catalog: &asset_model::WorldWeaponCatalog,
    weapon: u32,
    parent: Option<(usize, &str)>,
    base_index: usize,
) -> Vec<Part> {
    let Some(entry) = registry.world_model_entry(weapon, catalog) else {
        return Vec::new();
    };
    if entry.skel.pose.is_none() {
        return Vec::new();
    }
    let mut out = vec![Part {
        skel: entry.skel.clone(),
        materials: materials_of!(entry.material_edges),
        attach: parent.map(|(p, tag)| xmodel_runtime::Attach {
            parent_model: p,
            tag: tag.to_owned(),
        }),
    }];
    for attachment in crate::anim::remote_body::world_attachments(registry, catalog, weapon) {
        out.push(Part {
            skel: attachment.entry.skel.clone(),
            materials: materials_of!(attachment.entry.material_edges),
            attach: Some(xmodel_runtime::Attach {
                parent_model: base_index,
                tag: attachment.tag.to_string(),
            }),
        });
    }
    out
}

fn build_dobj(parts: &[Part]) -> Option<xmodel_runtime::DObj> {
    let srcs: Vec<(&xmodel_runtime::ModelPoseSrc, Option<xmodel_runtime::Attach>)> = parts
        .iter()
        .map(|p| Some((p.skel.pose.as_ref()?, p.attach.clone())))
        .collect::<Option<Vec<_>>>()?;
    xmodel_runtime::DObj::build(&srcs).ok()
}

fn skin_all(dobj: &xmodel_runtime::DObj, parts: &[Part], world: &[Mat4]) -> Vec<Arc<PosedVerts>> {
    let skin = dobj.skin_matrices(world);
    parts
        .iter()
        .enumerate()
        .map(|(mi, part)| {
            let base = dobj.models.get(mi).map_or(0, |m| m.base);
            Arc::new(skin_verts(&part.skel, |b| {
                skin.get(base + b).copied().unwrap_or(Mat4::IDENTITY)
            }))
        })
        .collect()
}

/// The bones the torso's animation drives: `j_spinelower` and everything
/// below it in the hierarchy (spine, arms, head, the gun on the hand).
/// Parents come before their children in a DObj's bone list.
fn upper_body(dobj: &xmodel_runtime::DObj) -> anim_iw4::PartBits {
    let Some(root) = dobj.find("j_spinelower") else {
        return dobj.all_parts();
    };
    let mut bits = anim_iw4::PartBits::default();
    let mut upper = vec![false; dobj.bones.len()];
    for i in 0..dobj.bones.len() {
        upper[i] = i == root
            || dobj.bones[i]
                .parent
                .is_some_and(|p| p < i && upper[p]);
        if upper[i] {
            bits.set(i);
        }
    }
    bits
}

/// The surface under a point (BO2's surface flags), by the clip.
fn surface_under(clip: Option<&asset_world::ClipCollision>, at: [f32; 3]) -> u32 {
    let Some(clip) = clip else { return 0 };
    let hit = clip.sweep_box([at[0], at[1], at[2] + 8.0], [at[0], at[1], at[2] - 64.0], [0.0; 3], [0.0; 3], 1);
    if hit.startsolid || hit.fraction >= 1.0 { 0 } else { hit.surface_flags }
}

/// The sounds his body's animations call for this frame: entering a dive's
/// takeoff or landing animation (another player's), and every bodyfall
/// notetrack his legs' animation passed since the last frame.
#[allow(clippy::too_many_arguments)]
fn body_sounds(
    out: &mut MessageWriter<audio::AliasCommand>,
    script: &T6PlayerAnims,
    xanims: &assets::PreparedXAnims,
    clip: Option<&asset_world::ClipCollision>,
    track: &mut BodyTrack,
    origin: [f32; 3],
    number: u32,
    corpse: bool,
    other: bool,
) {
    use crate::occupancy::t6_sounds::{play_t6, surface_name};
    let cur = track.legs.cur;
    let last = track.heard.replace(cur);
    let Some(name) = script.name_of(cur.value) else { return };
    let same = last.is_some_and(|l| t6_anim_part(l.value) == t6_anim_part(cur.value));
    if !same && other && !corpse && last.is_some() && name.contains("dive_prone") {
        let surface = surface_name(surface_under(clip, origin));
        if name.ends_with("_land") {
            play_t6(out, format!("fly_dtp_land_npc_{surface}"), Some("fly_dtp_land_npc_default".to_owned()), Some(origin), Some(number));
        } else {
            play_t6(out, "fly_dtp_launch_npc".to_owned(), None, Some(origin), Some(number));
        }
    }
    let Some(anim) = xanims.0.clip(asset_core::AssetNamespace::T6, name) else { return };
    if anim.notifies.is_empty() {
        return;
    }
    let duration = anim.duration().max(1e-3);
    // Where the animation was last frame and is now (fractions); a new
    // animation from its start, one first seen mid-way from where it is.
    let from = match last {
        Some(l) if same => l.phase / duration,
        Some(_) => 0.0,
        None => cur.phase / duration,
    };
    let to = cur.phase / duration;
    if to <= from && !(!same && last.is_some()) {
        return;
    }
    for note in &anim.notifies {
        let crossed = if !same && last.is_some() {
            note.time >= 0.0 && note.time <= to
        } else {
            note.time > from && note.time <= to
        };
        if !crossed {
            continue;
        }
        let size = match note.name.as_str() {
            "bodyfall large" => "large",
            "bodyfall small" => "small",
            _ => continue,
        };
        // BO2 names a body fall for these surfaces (in multiplayer each is
        // its gear fall, the alias's secondary); others fall as the gear.
        const BODYFALL_SURFACES: [&str; 15] = [
            "asphalt", "bark", "carpet", "ceramic", "cloth", "concrete", "dirt", "grass", "gravel", "mud", "plastic",
            "rock", "rubber", "sand", "wood",
        ];
        let surface = surface_name(surface_under(clip, origin));
        let alias = if BODYFALL_SURFACES.contains(&surface) {
            format!("fly_bodyfall_{size}_{surface}")
        } else {
            "fly_gear_fall_npc".to_owned()
        };
        play_t6(out, alias, None, Some(origin), Some(number));
    }
}

/// A corpse's fit to the ground at one placement: the body tilted onto
/// the slope under it, the height it is moved by, and how far each point
/// (hips, head, feet) is left above its own ground.
struct GroundFit {
    turned: Mat4,
    shift: f32,
    pivot: Vec3,
    /// (its world point, the ground under it, how far it hangs over that)
    rows: Vec<(Vec3, f32, f32)>,
    /// The height the body would need to move (before the 24-unit cap).
    need: f32,
}

impl GroundFit {
    fn worst_hang(&self) -> f32 {
        self.rows.iter().map(|r| r.2).fold(0.0, f32::max)
    }

    fn placed(&self) -> Mat4 {
        Mat4::from_translation(Vec3::new(0.0, 0.0, self.shift)) * self.turned
    }
}

fn fit_to_ground(
    dobj: &xmodel_runtime::DObj,
    world: &[Mat4],
    placed: Mat4,
    origin: [f32; 3],
    clip: &asset_world::ClipCollision,
    lying: f32,
) -> Option<GroundFit> {
    const SOLID: u32 = 1;
    let bone = |name: &str| dobj.find(name).and_then(|i| world.get(i)).map(|m| m.w_axis.truncate());
    let (pelvis, head) = (bone("pelvis")?, bone("j_head")?);
    let mut keys = vec![pelvis, head];
    keys.extend(["j_ankle_le", "j_ankle_ri"].iter().filter_map(|n| bone(n)));
    // (model-space point, its world point, the ground under it)
    let mut rows: Vec<(Vec3, Vec3, f32)> = Vec::new();
    for local in keys {
        let at = placed.transform_point3(local);
        let top = [at.x, at.y, origin[2] + 48.0];
        let bottom = [at.x, at.y, origin[2] - 64.0];
        let hit = clip.sweep_box(top, bottom, [0.0; 3], [0.0; 3], SOLID);
        if hit.startsolid || hit.fraction >= 1.0 {
            continue;
        }
        rows.push((local, at, hit.endpos[2]));
    }
    if rows.is_empty() {
        return None;
    }
    let pivot = placed.transform_point3(pelvis);
    // The body's length on the ground: hips to head, flat.
    let along = {
        let d = placed.transform_point3(head) - pivot;
        Vec3::new(d.x, d.y, 0.0).normalize_or_zero()
    };
    let mut turned = placed;
    if along != Vec3::ZERO && rows.len() >= 2 {
        // Least squares: ground height against distance along the body.
        let n = rows.len() as f32;
        let ss: Vec<(f32, f32)> = rows.iter().map(|(_, at, g)| ((*at - pivot).dot(along), *g)).collect();
        let (ms, mg) = ss.iter().fold((0.0, 0.0), |(a, b), (s, g)| (a + s / n, b + g / n));
        let (num, den) = ss
            .iter()
            .fold((0.0, 0.0), |(a, b), (s, g)| (a + (s - ms) * (g - mg), b + (s - ms) * (s - ms)));
        if den > 16.0 {
            let slope = (num / den).clamp(-0.7, 0.7);
            let pitch = slope.atan() * lying;
            let axis = Vec3::Z.cross(along).normalize_or_zero();
            if axis != Vec3::ZERO {
                // Raise the head end when the ground rises toward it.
                let tilt = Mat4::from_translation(pivot)
                    * Mat4::from_axis_angle(axis, -pitch)
                    * Mat4::from_translation(-pivot);
                turned = tilt * placed;
            }
        }
    }
    // Each point's height over its ground against its height in the pose.
    let needs: Vec<f32> = rows
        .iter()
        .map(|(local, _, g)| (g + local.z) - turned.transform_point3(*local).z)
        .collect();
    let need = needs.iter().copied().fold(f32::MIN, f32::max);
    let shift = need.clamp(-24.0, 24.0);
    Some(GroundFit {
        turned,
        shift,
        pivot,
        rows: rows.iter().zip(&needs).map(|((_, at, g), n)| (*at, *g, shift - n)).collect(),
        need,
    })
}

/// Ground contact for a corpse: the ground below its head, hips and feet
/// (straight down, solid only), a slope fitted along its length (feet to
/// head), the body tilted onto it about the hips by `lying` (0 standing at
/// the start of the death animation .. 1 lying at its end), then raised or
/// lowered so the point that would sink most sits at its own height in the
/// pose (at most 24 units). A body the death animation carried out over a
/// ledge (part of it, or all of it, left hanging more than 24 units over
/// its ground) slides back toward the side that has ground first. No
/// physics.
fn rest_on_ground(
    dobj: &xmodel_runtime::DObj,
    world: &[Mat4],
    placed: Mat4,
    origin: [f32; 3],
    clip: &asset_world::ClipCollision,
    lying: f32,
) -> Mat4 {
    let Some(mut fit) = fit_to_ground(dobj, world, placed, origin, clip, lying) else {
        return placed;
    };
    let mut slid = Vec3::ZERO;
    if fit.worst_hang() > 24.0 {
        // The way back to ground: toward the points that rest on it, from
        // the ones left hanging; with none resting (all of it floats),
        // toward the spot the engine's corpse lies on (on ground, by its
        // own trace).
        let flat = |v: Vec3| Vec3::new(v.x, v.y, 0.0);
        let mean = |pick: &dyn Fn(f32) -> bool| {
            let pts: Vec<Vec3> = fit.rows.iter().filter(|r| pick(r.2)).map(|r| flat(r.0)).collect();
            (!pts.is_empty()).then(|| pts.iter().copied().sum::<Vec3>() / pts.len() as f32)
        };
        let held = mean(&|hang| hang < 4.0);
        let hanging = mean(&|hang| hang > 24.0);
        let way = match (held, hanging) {
            (Some(held), Some(hanging)) => held - hanging,
            _ => flat(Vec3::from(origin)) - flat(fit.pivot),
        };
        if way.length() > 1.0 {
            // Half, all, then half again of the way: the first that leaves
            // nothing hanging, else the one that hangs least.
            for f in [0.5f32, 1.0, 1.5] {
                let step = way * f;
                let Some(next) = fit_to_ground(dobj, world, Mat4::from_translation(step) * placed, origin, clip, lying)
                else {
                    continue;
                };
                let better = next.worst_hang() < fit.worst_hang();
                if better {
                    fit = next;
                    slid = step;
                }
                if fit.worst_hang() <= 24.0 {
                    break;
                }
            }
        }
    }
    if std::env::var_os("BO2MP_BODYDRAWLOG").is_some() {
        static LAST: std::sync::Mutex<u64> = std::sync::Mutex::new(0);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        if let Ok(mut last) = LAST.lock()
            && *last != now
        {
            *last = now;
            diag::info!(
                World,
                "bo2mp corpse rest: ground points {:?} (x y ground hang) lying {lying:.2} need {:.1} shift {:.1} slid {:.0}",
                fit.rows
                    .iter()
                    .map(|(at, g, hang)| [at.x.round(), at.y.round(), g.round(), hang.round()])
                    .collect::<Vec<_>>(),
                fit.need,
                fit.shift,
                slid.length()
            );
        }
    }
    fit.placed()
}

/// Turn the legs (the `pelvis` and everything under it but the torso, which
/// hangs from `torso_stabilizer`) about the pelvis by `angle` (radians,
/// about up), leaving the upper body where the pose put it.
fn twist_legs(dobj: &xmodel_runtime::DObj, world: &mut [Mat4], angle: f32) {
    if angle.abs() < 1e-4 {
        return;
    }
    let (Some(pelvis), Some(stabilizer)) = (dobj.find("pelvis"), dobj.find("torso_stabilizer")) else {
        return;
    };
    let Some(pivot) = world.get(pelvis).map(|m| m.w_axis.truncate()) else {
        return;
    };
    let turn = Mat4::from_translation(pivot) * Mat4::from_rotation_z(angle) * Mat4::from_translation(-pivot);
    let mut legs = vec![false; dobj.bones.len().min(world.len())];
    for i in 0..legs.len() {
        legs[i] = i == pelvis
            || (i != stabilizer && dobj.bones[i].parent.is_some_and(|p| p < i && legs[p]));
        if legs[i] {
            world[i] = turn * world[i];
        }
    }
}

/// The script's standing idle for these facts: what a part plays when the
/// server has named nothing yet.
fn idle_value(script: &T6PlayerAnims) -> i32 {
    script
        .pick_move("idle", &T6AnimFacts::default())
        .and_then(|c| c.first())
        .map_or(0, |c| sim::t6_playeranim::t6_pack_anim(c.anim, false))
}

#[allow(clippy::too_many_arguments)]
fn collect_t6_bodies(
    fpv: Option<Res<assets::PreparedFpvMeshes>>,
    xanims: Option<Res<assets::PreparedXAnims>>,
    weapons: Option<Res<assets::PreparedWeapons>>,
    world_weapons: Option<Res<assets::PreparedWorldWeapons>>,
    script: Option<Res<T6PlayerAnimsRes>>,
    presented: Option<Res<PresentedSnapshot>>,
    local: Option<Res<LocalPresentClient>>,
    subject: Option<Res<ViewSubject>>,
    cg_clock: Option<Res<FrameClock>>,
    clip: Option<Res<crate::occupancy::DynEntPhysClip>>,
    mut shapes: ResMut<T6ItemShapes>,
    ents: Query<(Entity, &CEntity, &CEntityRuntime)>,
    mut out: ResMut<T6BodyModels>,
    mut poses: ResMut<T6BodyPoses>,
    mut locals: Local<BodyLocals>,
    mut sounds: MessageWriter<audio::AliasCommand>,
) {
    out.items.clear();
    poses.rows.clear();
    poses.local_corpse_root = None;
    let mut corpse_root_time: Option<i32> = None;
    let _slow = SlowGuard(std::time::Instant::now(), "t6 bodies");
    if std::env::var_os("BO2MP_NO_T6BODY").is_some() {
        return;
    }
    let (Some(fpv), Some(xanims), Some(script), Some(presented)) = (fpv, xanims, script, presented) else {
        return;
    };
    let Some(script) = script.0.clone() else {
        return;
    };
    let Some(snapshot) = presented.snapshot() else {
        return;
    };
    let now = cg_clock
        .as_ref()
        .filter(|c| c.started())
        .map(|c| c.time())
        .unwrap_or_else(|| sim::level_time_ms(snapshot.tick));
    // The clock went back (a killcam's replay starts): every animation
    // starts again from what the replay shows.
    if now < locals.last_now {
        locals.tracks.clear();
    }
    let dt = (now - locals.last_now).clamp(0, 250) as f32 / 1000.0;
    locals.last_now = now;
    let local_id = local.as_ref().map(|l| l.0);
    let in_killcam = subject.as_ref().is_some_and(|s| s.in_killcam());
    let eyes = match subject.as_deref() {
        Some(ViewSubject::Seat {
            focus: Some(focus), ..
        }) => i32::try_from(*focus).unwrap_or(0),
        _ => local_id.map_or(0, |l| i32::try_from(l.0).unwrap_or(0)),
    };
    let gate = PlayerDrawGate {
        eyes_entity_num: eyes,
        other_flags: local_id
            .and_then(|l| presented.player(l))
            .map_or(0, |ps| ps.other_flags),
        rendering_third_person: local_id.is_some_and(|l| {
            crate::occupancy::third_person::presented_is_third_person(&presented, l, in_killcam)
        }),
    };
    let registry = weapons.as_ref().map(|w| &*w.0);
    let catalog = world_weapons.as_ref().map(|w| &w.0).filter(|c| !c.is_empty());
    let fallback = idle_value(&script);
    let mut seen: Vec<u16> = Vec::new();
    for (entity, identity, runtime) in &ents {
        let kind = net::remote_body_submit_kind(identity.number(), runtime, gate);
        if !kind.submits() {
            continue;
        }
        let corpse = runtime.pose_e_type == ET_PLAYER_CORPSE as u8
            || runtime.next_state.e_type == ET_PLAYER_CORPSE;
        let client = if corpse {
            u32::try_from(runtime.next_state.client_num).ok().map(sim::ClientId)
        } else {
            identity.client()
        };
        let Some(client) = client else { continue };
        let current = snapshot
            .meta
            .for_client(client)
            .and_then(|m| m.client_dvars.iter().find(|(k, _)| k == BODY_DVAR))
            .map(|(_, v)| v.as_str())
            .unwrap_or("");
        // A corpse keeps the body it fell in (he may respawn as another
        // class): the one his client had when it first showed.
        let body: String = if corpse {
            let key = (identity.number(), client.0, runtime.next_state.legs_anim);
            locals
                .corpse_bodies
                .entry(key)
                .or_insert_with(|| current.to_owned())
                .clone()
        } else {
            current.to_owned()
        };
        let mut names = body.split(';');
        let model = names.next().unwrap_or("");
        if model.is_empty() {
            note_once(&mut locals, format!("bo2mp body: client {} has no body model yet", client.0));
            continue;
        }
        let Some(entry) = fpv.0.get(asset_core::AssetNamespace::T6, model) else {
            note_once(&mut locals, format!("bo2mp body: {model} is not in the model catalog"));
            continue;
        };
        let mut parts = vec![Part {
            skel: entry.skel.clone(),
            materials: materials_of!(entry.material_edges),
            attach: None,
        }];
        let mut left_hand_model = false;
        for extra in names {
            let Some((m, tag)) = extra.split_once('@') else { continue };
            left_hand_model |= tag == "tag_weapon_left";
            // A weapon on a tag (the stowed gun on his back): its world
            // model and attachments.
            if let Some(index) = m.strip_prefix('#') {
                if let (Ok(w), Some(registry), Some(catalog)) = (index.parse::<u32>(), registry, catalog) {
                    let base = parts.len();
                    parts.extend(gun_parts(registry, catalog, w, Some((0, tag)), base));
                }
                continue;
            }
            // A model from the character catalog, or a weapon's world model
            // (BO2's stow_on_hip attaches one at tag_stowed_hip_rear).
            let found = fpv
                .0
                .get(asset_core::AssetNamespace::T6, m)
                .map(|e| (e.skel.clone(), materials_of!(e.material_edges)))
                .or_else(|| {
                    let e = catalog?.get(asset_core::AssetNamespace::T6, m)?;
                    Some((e.skel.clone(), materials_of!(e.material_edges)))
                });
            let Some((skel, materials)) = found else {
                note_once(&mut locals, format!("bo2mp body: {m} is not in the model catalog"));
                continue;
            };
            let tag = if tag.is_empty() {
                parts[0]
                    .skel
                    .pose
                    .as_ref()
                    .zip(skel.pose.as_ref())
                    .map(|(p, c)| xmodel_runtime::empty_tag_attach(p, c))
                    .unwrap_or_default()
            } else {
                tag.to_owned()
            };
            parts.push(Part {
                skel,
                materials,
                attach: Some(xmodel_runtime::Attach {
                    parent_model: 0,
                    tag,
                }),
            });
        }
        // The gun in his right hand (a corpse dropped his), and a dual-wield
        // pair's left-hand gun on tag_weapon_left. While a torso animation
        // handles a grenade or a piece of equipment (the script's
        // grenadeAnim: a throw, a claymore planted), that item is in his
        // hand instead of the gun.
        let held_weapon = u32::try_from(runtime.next_state.index).unwrap_or(0);
        let offhand = sim::t6_playeranim::t6_offhand(runtime.next_state.torso_anim);
        let weapon = offhand.unwrap_or(held_weapon);
        let mut right_gun = None;
        if !corpse
            && weapon != 0
            && let (Some(registry), Some(catalog)) = (registry, catalog)
        {
            let base = parts.len();
            // BO2 holds the riot shield on the left arm (its riotshield
            // animations brace it there); every other weapon in the right hand.
            let shield = registry
                .facts_of(weapon)
                .is_some_and(|f| script.anim_type_name(f.player_anim_type) == "riotshield");
            let tag = if shield {
                "tag_weapon_left"
            } else {
                parts[0]
                    .skel
                    .pose
                    .as_ref()
                    .and_then(|p| xmodel_runtime::tp_weapon_attach_tag(&p.bone_names))
                    .unwrap_or("tag_weapon_right")
            };
            // The riot shield in his hand is the carried model the script
            // attaches there (refreshshieldattachment); the weapon's own
            // world model is the one that lies on the ground.
            let gun = if shield && left_hand_model {
                Vec::new()
            } else {
                gun_parts(registry, catalog, weapon, Some((0, tag)), base)
            };
            if !gun.is_empty() {
                right_gun = Some(base);
            }
            parts.extend(gun);
            let left = registry.dual_wield_weapon_of(weapon);
            if offhand.is_none() && left != 0 && left != weapon {
                let base = parts.len();
                parts.extend(gun_parts(registry, catalog, left, Some((0, "tag_weapon_left")), base));
            }
        }
        let Some(dobj) = build_dobj(&parts) else {
            note_once(&mut locals, format!("bo2mp body: {model} has no skeleton to pose"));
            continue;
        };
        for part in &parts {
            note_unbound(&mut locals, part);
        }
        // His animations, as the server picked them.
        let number = identity.number();
        seen.push(number);
        let first = !locals.tracks.contains_key(&number);
        let track = locals.tracks.entry(number).or_default();
        if !first && track.corpse != corpse {
            *track = BodyTrack::default();
        }
        let first = first || track.seen == 0;
        let legs = match runtime.next_state.legs_anim {
            v if t6_anim_index(v).is_some() => v,
            _ => fallback,
        };
        let torso = match runtime.next_state.torso_anim {
            v if t6_anim_index(v).is_some() => v,
            _ => legs,
        };
        track.legs.see(legs, now, &script, first);
        track.torso.see(torso, now, &script, first);
        // A moving legs animation plays at his speed over its own (feet
        // that keep to the ground); the torso with it when it is the same.
        let speed = if corpse {
            0.0
        } else {
            presented
                .player(client)
                .map_or(0.0, |ps| ps.velocity[0].hypot(ps.velocity[1]))
        };
        let legs_rate = t6_anim_index(track.legs.cur.value)
            .map(|a| script.speed_of(a))
            .filter(|&natural| natural > 1.0 && speed > 1.0)
            .map_or(1.0, |natural| (speed / natural).clamp(0.3, 3.0));
        // A torso animation timed to the weapon (a reload, a raise or drop)
        // plays over the weapon's time.
        let timed_rate = t6_anim_duration(torso).and_then(|secs| {
            let name = script.name_of(torso)?;
            let clip = xanims.0.clip(asset_core::AssetNamespace::T6, name)?;
            Some((clip.duration() / secs.max(0.05)).clamp(0.2, 5.0))
        });
        let torso_rate = if let Some(rate) = timed_rate {
            rate
        } else if t6_anim_index(track.torso.cur.value) == t6_anim_index(track.legs.cur.value) {
            legs_rate
        } else {
            1.0
        };
        track.legs.advance(dt, legs_rate);
        track.torso.advance(dt, torso_rate);
        // BO2's own sounds his body's animations call for: a dive's takeoff
        // and landing, and a body hitting the ground (the death animation's
        // bodyfall notetracks), by the surface under him.
        body_sounds(
            &mut sounds,
            &script,
            &xanims,
            clip.as_deref().and_then(|c| c.0.as_deref()),
            track,
            runtime.origin,
            u32::from(identity.number()),
            corpse,
            Some(client) != local_id,
        );
        let legs_target = if corpse { 0.0 } else { t6_legs_yaw(runtime.next_state.legs_anim) };
        track.legs_yaw = if first {
            legs_target
        } else {
            let step = LEGS_YAW_SPEED_DEG * dt;
            let d = legs_target - track.legs_yaw;
            if d.abs() <= step { legs_target } else { track.legs_yaw + step * d.signum() }
        };
        track.seen = now;
        track.corpse = corpse;
        let track = *track;
        let upper = upper_body(&dobj);
        let all = dobj.all_parts();
        let mut clips: Vec<(Arc<xmodel_runtime::AnimClip>, Vec<Option<usize>>, f32, f32, bool)> = Vec::new();
        for (part, torso_part) in [(&track.legs, false), (&track.torso, true)] {
            for (playing, w) in part.weighted(now) {
                let Some(name) = script.name_of(playing.value) else { continue };
                let Some(clip) = xanims.0.clip(asset_core::AssetNamespace::T6, name) else {
                    note_once(&mut locals, format!("bo2mp body: animation {name} is not loaded"));
                    continue;
                };
                let duration = clip.duration().max(1e-3);
                let mut t = playing.phase.max(0.0);
                t = if clip.looping { t.rem_euclid(duration) } else { t.min(duration) };
                let tracks = dobj.tracks_for(&clip);
                let weight = if torso_part { w * TORSO_OVER_LEGS } else { w };
                clips.push((clip, tracks, t, weight, torso_part));
            }
        }
        if clips.is_empty() {
            // Never a bind pose: the script's idle.
            if let Some(clip) = script
                .name_of(fallback)
                .and_then(|n| xanims.0.clip(asset_core::AssetNamespace::T6, n))
            {
                let tracks = dobj.tracks_for(&clip);
                clips.push((clip, tracks, 0.0, 1.0, false));
            } else {
                continue;
            }
        }
        let instances: Vec<xmodel_runtime::AnimInstance<'_>> = clips
            .iter()
            .map(|(clip, tracks, time, weight, torso_part)| xmodel_runtime::AnimInstance {
                clip,
                tracks,
                time: *time,
                weight: *weight,
                parts: torso_part.then_some(&upper),
            })
            .collect();
        let pitch = runtime.angles[0];
        let prone = runtime.next_state.e_flags & eflags::PRONE != 0;
        let crouch = runtime.next_state.e_flags & eflags::DUCK != 0;
        let legs_yaw = track.legs_yaw.to_radians();
        let mut world = dobj.pose_with_controller(&instances, &all, Mat4::IDENTITY, |dobj, _, locals| {
            if !corpse {
                xmodel_runtime::apply_player_controller(
                    dobj,
                    locals,
                    xmodel_runtime::PlayerControllerInput {
                        view_pitch_deg: pitch,
                        prone,
                        crouch,
                        lean_frac: 0.0,
                    },
                );
            }
        });
        // On a ladder he faces it, all of him (the climb's hands are on its
        // rungs); otherwise his legs turn in place behind his view and twist
        // to a diagonal move while the torso keeps to the view.
        let climbing = script
            .name_of(track.legs.cur.value)
            .is_some_and(|n| n.starts_with("pb_climb") || n.starts_with("pb_riot_climb"));
        if !corpse && !climbing {
            twist_legs(&dobj, &mut world, legs_yaw);
        }
        let posed = skin_all(&dobj, &parts, &world);
        let origin = runtime.origin;
        let body_yaw = if climbing { runtime.angles[1] + track.legs_yaw } else { runtime.angles[1] };
        let mut world_from_local = Transform {
            translation: Vec3::from_array(origin),
            rotation: Quat::from_rotation_z(body_yaw.to_radians()),
            scale: Vec3::ONE,
        }
        .to_matrix();
        // A corpse rests on the ground under it: tilted along its length to
        // the ground under its head, hips and feet, and set so none of them
        // sinks below the height the death animation gives it.
        if corpse && let Some(clip) = clip.as_deref().and_then(|c| c.0.as_deref()) {
            let lying = track
                .legs
                .weighted(now)
                .first()
                .and_then(|(p, _)| {
                    let clip = xanims.0.clip(asset_core::AssetNamespace::T6, script.name_of(p.value)?)?;
                    Some((p.phase / clip.duration().max(1e-3)).clamp(0.0, 1.0))
                })
                .unwrap_or(1.0);
            world_from_local = rest_on_ground(&dobj, &world, world_from_local, origin, clip, lying);
        }
        // The local player's own corpse: where its j_mainroot is, for the
        // death-watch camera (the newest one if he has several).
        if corpse
            && local_id == Some(client)
            && corpse_root_time.is_none_or(|t| runtime.next_state.tr_time >= t)
            && let Some(root) = dobj.find("j_mainroot").and_then(|i| world.get(i))
        {
            corpse_root_time = Some(runtime.next_state.tr_time);
            poses.local_corpse_root = Some(world_from_local.transform_point3(root.w_axis.truncate()).to_array());
            if std::env::var_os("BO2MP_BODYDRAWLOG").is_some() {
                diag::info!(World, "bo2mp corpse root: {:?} entity origin {:?}", poses.local_corpse_root, origin);
            }
        }
        let light_at = parts[0].skel.bounds.map_or([origin[0], origin[1], origin[2] + 36.0], |(lo, hi)| {
            let mid = Vec3::new((lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5, (lo[2] + hi[2]) * 0.5);
            world_from_local.transform_point3(mid).to_array()
        });
        for (part, verts) in parts.iter().zip(posed) {
            out.items.push(T6MissileModel {
                skel: part.skel.clone(),
                materials: part.materials.clone(),
                world_from_local,
                origin,
                light_at,
                posed: Some(verts),
            });
        }
        // His gun's tags, for its muzzle flash, shells and tracers.
        if let Some(gun) = right_gun {
            let tag = |name: &str| {
                dobj.bones
                    .iter()
                    .position(|b| b.model == gun && b.name == name)
                    .and_then(|i| u16::try_from(i).ok())
            };
            poses.rows.push(PosedBody {
                entity,
                number: identity.number(),
                world_from_local,
                flash: tag("tag_flash"),
                brass: tag("tag_brass"),
                bones: world.clone(),
            });
        }
        if std::env::var_os("BO2MP_BODYDRAWLOG").is_some() && (now / 1000) != ((now - 16) / 1000) {
            diag::info!(
                World,
                "bo2mp body draw: ent {number} client {} {model} corpse {corpse} weapon {weapon} parts {} legs {:?} torso {:?} at {:?} yaw {:.0} pitch {:.0}",
                client.0,
                parts.len(),
                script.name_of(track.legs.cur.value),
                script.name_of(track.torso.cur.value),
                origin,
                runtime.angles[1],
                pitch
            );
        }
    }
    locals.tracks.retain(|n, _| seen.contains(n));
    locals.corpse_bodies.retain(|(n, _, _), _| seen.contains(n));

    drop(_slow);
    let _slow_items = SlowGuard(std::time::Instant::now(), "t6 ground items");
    // Guns and Scavenger bags on the ground.
    let (Some(registry), Some(catalog)) = (registry, catalog) else {
        return;
    };
    let scavenger = local_id
        .and_then(|l| presented.player(l))
        .is_some_and(|ps| (ps.perks[0] & PERK_SCAVENGER) != 0);
    let mut seen_items: Vec<i32> = Vec::new();
    for es in snapshot.meta.entities.iter().filter(|es| es.e_type == ET_ITEM) {
        let Some(weapon) = u32::try_from(es.index).ok().filter(|&w| w != 0) else {
            continue;
        };
        if es.e_flags & 0x20 != 0 {
            continue;
        }
        let bag = snapshot
            .meta
            .item_ammo
            .iter()
            .any(|row| row.entnum == es.number && row.scavenger != 0);
        if bag && !scavenger {
            continue;
        }
        let shape = shapes
            .0
            .entry(weapon)
            .or_insert_with(|| item_shape(registry, catalog, weapon))
            .clone();
        let Some(shape) = shape else { continue };
        let pos = Trajectory {
            tr_time: es.tr_time,
            tr_type: es.tr_type,
            tr_duration: es.tr_duration,
            tr_delta: es.tr_delta,
            tr_base: es.tr_base,
        };
        let apos = Trajectory {
            tr_time: es.apos_tr_time,
            tr_type: es.apos_tr_type,
            tr_duration: es.apos_tr_duration,
            tr_delta: es.apos_tr_delta,
            tr_base: es.apos_tr_base,
        };
        let origin = evaluate_trajectory(&pos, now);
        let [_, yaw, _] = evaluate_trajectory(&apos, now);
        // A dropped gun lies on its side with the yaw it fell with (which
        // side by its number), tipping over as it falls; lifted so that
        // side rests on the ground.
        let first_seen = *locals.item_seen.entry(es.number).or_insert(now);
        seen_items.push(es.number);
        let tip = if es.tr_type == TR_GRAVITY {
            ((now - first_seen) as f32 / ITEM_TIP_MS).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let side = if es.number % 2 == 0 { 1.0 } else { -1.0 };
        let lift = if side > 0.0 { -shape.low_y } else { shape.high_y } * tip;
        let world_from_local = Transform {
            translation: Vec3::new(origin[0], origin[1], origin[2] + lift),
            rotation: Quat::from_rotation_z(yaw.to_radians())
                * Quat::from_rotation_x((side * 90.0 * tip).to_radians()),
            scale: Vec3::ONE,
        }
        .to_matrix();
        let light_at = [origin[0], origin[1], origin[2] + 4.0];
        for (skel, materials, verts) in &shape.models {
            out.items.push(T6MissileModel {
                skel: skel.clone(),
                materials: materials.clone(),
                world_from_local,
                origin,
                light_at,
                posed: Some(verts.clone()),
            });
        }
    }
    locals.item_seen.retain(|n, _| seen_items.contains(n));
}
