//! Script models on screen: every entity with a model gets a script mover
//! (a networked entity plus a model row the snapshot carries, as MW2's
//! script models do); its pose, visibility and attachments follow the
//! entity every frame, and it goes when the entity does.

use std::collections::BTreeMap;

use bevy_ecs::prelude::World;

use super::{Zm, frame};
use crate::ScriptModelId;
use crate::bullet_collision::{AuthorityDObjState, EntityCollisionCapabilities};

/// Spawned script movers for BO2 entities: inside the spawned range the
/// client spawns models for, clear of IW4L's own.
const T6_PRESENCE_BASE: u32 = 0x6000_0000;

/// A living zombie's box as IW4 packs an entity's `solid` (half-width 15,
/// 1 below its feet, 72 tall: BO2's actor size, as a player's): players
/// cannot walk through zombies.
const ZOMBIE_SOLID: u32 = ((72 + 32) << 16) | (1 << 8) | 15;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Shown {
    pub id: ScriptModelId,
    pub number: i32,
    pub origin: [f32; 3],
    pub angles: [f32; 3],
    pub model: String,
    pub attachments: Vec<(String, String)>,
    pub hidden: bool,
    /// It moved last tick: its client carries it on at that speed until it
    /// is told it stopped.
    pub moving: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Presences {
    pub by_ent: BTreeMap<u32, Shown>,
    pub next: u32,
}

fn near(a: [f32; 3], b: [f32; 3]) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() < 0.01)
}

/// Rows for a zbarrier's pieces: the entity's number shifted, the piece
/// in the low bits, this bit set.
const PIECE_BIT: u32 = 0x8000_0000;

/// Loops the scripts play on entities (`playloopsound`: the box's tune, a
/// perk machine's hum, a power-up's hum), each where its entity stands,
/// owned by its model row (or its number, for an entity with no model).
pub(super) fn publish_loops(world: &mut World) {
    let engines = super::engine_sounds::rows(world);
    let mut rows: Vec<(ScriptModelId, String, [f32; 3], f32, f32)> = {
        let zm = world.resource::<Zm>();
        zm.ents
            .iter()
            .filter_map(|(n, e)| {
                let alias = e.loop_sound.as_ref()?.to_ascii_lowercase();
                let owner = zm.presences.by_ent.get(n).map_or_else(
                    || ScriptModelId::from_wire(0x2000_0000 | (n & 0x0fff_ffff)),
                    |s| s.id,
                );
                Some((owner, alias, e.origin, 1.0, 1.0))
            })
            .collect()
    };
    // Helicopter and drone engines (BO2's client script's loops), owned by
    // the vehicle's model row as its other loops are.
    {
        let zm = world.resource::<Zm>();
        for (n, alias, origin, volume, pitch) in engines {
            let owner = zm.presences.by_ent.get(&n).map_or_else(
                || ScriptModelId::from_wire(0x2000_0000 | (n & 0x0fff_ffff)),
                |s| s.id,
            );
            rows.push((owner, alias.to_owned(), origin, volume, pitch));
        }
    }
    let mut f = frame(world);
    let rows = rows
        .into_iter()
        .map(
            |(owner, alias, origin, volume, pitch)| crate::world_objects::DestructibleLoopSound {
                owner,
                alias_index: f.sound_alias_index(&alias),
                origin,
                volume: crate::world_objects::DestructibleLoopSound::hundredths(volume),
                pitch: crate::world_objects::DestructibleLoopSound::hundredths(pitch),
            },
        )
        .collect();
    f.world_objects_mut().set_destructible_loop_sounds(rows);
}

/// A model that is only something to bump into (Treyarch names them
/// `collision_*`, `zm_collision_*`; their material never draws in BO2):
/// kept solid, never shown.
/// A collision wall's half-size from its name (`collision_wall_128x128x10_
/// standard`: width x height x thickness, centred on its origin; local x,
/// z, y). These models carry no collision surfaces and only a cube for
/// bounds; Nuketown's script builds a closed box from six of them, which
/// fixes the axes.
fn named_wall_size(model: &str) -> Option<[f32; 3]> {
    if !model.starts_with("collision_") || !(model.contains("_wall_") || model.contains("_player_"))
    {
        return None;
    }
    let dims = model.split('_').find_map(|part| {
        let v: Vec<f32> = part.split('x').filter_map(|d| d.parse().ok()).collect();
        (v.len() == 3).then_some(v)
    })?;
    Some([dims[0] * 0.5, dims[2] * 0.5, dims[1] * 0.5])
}

fn collision_only(model: &str) -> bool {
    model.starts_with("collision_")
        || model.starts_with("zm_collision_")
        || model.contains("_collision_")
}

/// IW4L_T6_NEAR=<seconds>: at that time, every drawn entity within 250
/// units of player 0 (debugging what stands in view).
fn report_near(world: &mut World, now_ms: i64) {
    static AT: std::sync::OnceLock<Option<i64>> = std::sync::OnceLock::new();
    let Some(at) = *AT.get_or_init(|| {
        std::env::var("IW4L_T6_NEAR")
            .ok()
            .and_then(|s| s.parse::<i64>().ok())
    }) else {
        return;
    };
    if !(at * 1000..at * 1000 + i64::from(crate::MATCH_TICK_MS)).contains(&now_ms) {
        return;
    }
    let Some(me) = frame(world)
        .player(crate::world::ClientId(0))
        .map(|p| p.origin)
    else {
        return;
    };
    let zm = world.resource::<Zm>();
    for (n, s) in &zm.presences.by_ent {
        let d = ((s.origin[0] - me[0]).powi(2)
            + (s.origin[1] - me[1]).powi(2)
            + (s.origin[2] - me[2]).powi(2))
        .sqrt();
        if d < 250.0 {
            let class = zm.ents.get(n).map_or("", |e| e.classname.as_str());
            diag::info!(
                Sim,
                "bo2zm t6 near: ent{n} {class} model {} hidden {} at {:.0} ({:.0} {:.0} {:.0}) attached {:?}",
                s.model,
                s.hidden,
                d,
                s.origin[0],
                s.origin[1],
                s.origin[2],
                s.attachments
            );
        }
    }
}

pub(super) fn sync(world: &mut World, now_ms: i64) {
    report_near(world, now_ms);
    let now = now_ms as i32;
    let zm = world.resource::<Zm>();
    let pieces: Vec<(
        u32,
        ([f32; 3], [f32; 3], String, Vec<(String, String)>, bool),
    )> = zm
        .ents
        .iter()
        .filter_map(|(n, e)| e.zbarrier.as_ref().map(|z| (*n, e, z)))
        .flat_map(|(n, e, z)| {
            z.pieces.iter().enumerate().filter_map(move |(i, p)| {
                // A weapon-rise piece (a bare tag) shows the box's
                // spinning gun while it opens, where its clip's root
                // motion has lifted it, turned as the box turns the gun
                // it picks (the chest's angles + 180 yaw).
                let anim = zm.anims.get(&p.open_anim);
                if let Some((gun, off)) = super::zbarrier::rise_gun(p, anim, now_ms) {
                    let (f, r, u) = gsc_t6::math::angle_vectors(e.angles);
                    let origin = std::array::from_fn(|c| {
                        e.origin[c] + f[c] * off[0] - r[c] * off[1] + u[c] * off[2]
                    });
                    let angles = [e.angles[0], e.angles[1] + 180.0, e.angles[2]];
                    return Some((
                        PIECE_BIT | (n << 4) | i as u32,
                        (origin, angles, gun, Vec::new(), e.hidden),
                    ));
                }
                if p.model.is_empty() || p.model == "tag_origin" {
                    return None;
                }
                Some((
                    PIECE_BIT | (n << 4) | i as u32,
                    (
                        e.origin,
                        e.angles,
                        p.model.clone(),
                        Vec::new(),
                        !p.shown || e.hidden,
                    ),
                ))
            })
        })
        .collect();
    // Collision models (a perk machine's zm_collision_perks1, Nuketown's
    // patch walls, some lying on their sides) block players as their own
    // bounds turned with them while the scripts keep them solid (fix list
    // 3: they were square boxes of their widest side, ignoring the turn: a
    // 128-unit block for each 10-unit patch wall, one by the bunker, one in
    // the middle of the map, and the walls on their sides stayed open).
    let mut blocker_ents: Vec<(u32, [f32; 3], [f32; 3], String)> = zm
        .ents
        .iter()
        .filter(|(_, e)| e.solid && collision_only(&e.model))
        .map(|(n, e)| (*n, e.origin, e.angles, e.model.clone()))
        .collect();
    // The Mystery Box is solid where it stands (any box piece shown there:
    // the box, or the closed one): before, he walked through it.
    blocker_ents.extend(zm.ents.iter().filter_map(|(n, e)| {
        let z = e.zbarrier.as_ref()?;
        let piece = z
            .pieces
            .iter()
            .find(|p| p.shown && p.model.contains("magic_box"))?;
        (!e.hidden).then(|| (*n, e.origin, e.angles, piece.model.clone()))
    }));
    let mut wanted: BTreeMap<u32, ([f32; 3], [f32; 3], String, Vec<(String, String)>, bool)> = zm
        .ents
        .iter()
        .filter(|(n, e)| {
            e.obj.is_some()
                && !e.model.is_empty()
                && !e.model.starts_with('*')
                // Spawners are never drawn (the actors they spawn are).
                && (!e.classname.starts_with("actor_") || zm.actors.contains_key(n))
        })
        .map(|(n, e)| {
            (
                *n,
                (
                    e.origin,
                    e.angles,
                    e.model.clone(),
                    e.attached.clone(),
                    e.hidden || e.invisible_to_all || collision_only(&e.model),
                ),
            )
        })
        .collect();
    wanted.extend(pieces);
    // Retire the gone.
    let gone: Vec<(u32, Shown)> = world
        .resource::<Zm>()
        .presences
        .by_ent
        .iter()
        .filter(|(n, _)| !wanted.contains_key(n))
        .map(|(n, s)| (*n, s.clone()))
        .collect();
    for (n, shown) in gone {
        let mut f = frame(world);
        f.remove_script_mover_by_number(shown.number);
        f.remove_collision_owner(shown.id);
        world.resource_mut::<Zm>().presences.by_ent.remove(&n);
    }
    for (n, (origin, angles, model, attachments, hidden)) in wanted {
        let existing = world.resource::<Zm>().presences.by_ent.get(&n).cloned();
        let mut shown = match existing {
            Some(s) => s,
            None => {
                let serial = {
                    let mut zm = world.resource_mut::<Zm>();
                    let s = zm.presences.next;
                    zm.presences.next += 1;
                    s
                };
                let id = ScriptModelId::from_wire(T6_PRESENCE_BASE + serial);
                let mut f = frame(world);
                let Ok(number) = f.spawn_script_mover(id, origin, angles) else {
                    diag::warn!(Sim, "bo2zm t6: no free entity for model {model}");
                    continue;
                };
                let capability = f.model_capability(&model).flatten();
                f.insert_collision_owner(EntityCollisionCapabilities::current_tick(
                    crate::AuthorityModelOwner::ScriptModel(id),
                    Some(AuthorityDObjState::at_pose(
                        &model, capability, origin, angles,
                    )),
                    Vec::new(),
                ));
                Shown {
                    id,
                    number,
                    origin,
                    angles,
                    model: String::new(),
                    attachments: Vec::new(),
                    hidden: !hidden,
                    moving: false,
                }
            }
        };
        let (yaw_goal, holds_still) = world
            .resource::<Zm>()
            .actors
            .get(&n)
            .filter(|a| a.alive)
            .map_or((None, false), |a| {
                let still = matches!(a.script.as_str(), "combat" | "stop")
                    && a.scripted.is_none()
                    && a.traverse.is_none();
                (a.yaw_goal, still)
            });
        let prev_yaw = shown.angles[1];
        let mut f = frame(world);
        let moved = !near(shown.origin, origin) || !near(shown.angles, angles);
        // A move that ended goes out once more, standing still (else the
        // client keeps it going: perk machines flew off into the sky).
        if moved || shown.moving || yaw_goal.is_some() {
            shown.moving = moved;
            f.set_script_mover_pose(shown.number, now, origin, angles);
            // bo2zm M3 fix list 2: a turning zombie's client turns it toward
            // the facing it is turning to at the server's rate and stops
            // there, as the next tick will. Carried on at the last tick's
            // turn instead, the body swung past the end of every turn (up to
            // 12 degrees) and snapped back.
            // A zombie that has stopped (attacking, standing) moves no
            // further next tick: it is drawn where it stands, not carried on
            // a few units past its stop and pulled back.
            if holds_still && let Some(mover) = f.script_mover_mut_by_number(shown.number) {
                let state = &mut mover.state;
                state.tr_type = entity_iw4::TR_STATIONARY;
                state.tr_time = now;
                state.tr_base = origin;
                state.tr_delta = [0.0; 3];
                state.tr_duration = 0;
            }
            // IW4L_T6_NOTURNGOAL=1: the old carry-on, to compare (test aid).
            let use_goal = std::env::var_os("IW4L_T6_NOTURNGOAL").is_none();
            if use_goal
                && let Some(goal) = yaw_goal
                && let Some(mover) = f.script_mover_mut_by_number(shown.number)
            {
                // From the facing it had last tick (where his client shows it
                // now): the turn trails the server by a tick, so it starts
                // smoothly instead of jumping a whole tick's turn (18 degrees).
                let from = [angles[0], prev_yaw, angles[2]];
                let left = math_iw4::angle_subtract(goal, from[1]);
                let state = &mut mover.state;
                if left.abs() > 0.05 {
                    let rate = super::actors::TURN_DEG_PER_S;
                    state.apos_tr_type = entity_iw4::TR_LINEAR_STOP;
                    state.apos_tr_time = now;
                    state.apos_tr_base = from;
                    state.apos_tr_delta = [0.0, rate.copysign(left), 0.0];
                    state.apos_tr_duration = ((left.abs() / rate) * 1000.0).round().max(1.0) as i32;
                } else {
                    state.apos_tr_type = entity_iw4::TR_STATIONARY;
                    state.apos_tr_time = now;
                    state.apos_tr_base = angles;
                    state.apos_tr_delta = [0.0; 3];
                    state.apos_tr_duration = 0;
                }
            }
            if let Some(row) = f.collision_owner_mut(shown.id) {
                if let Some(dobj) = row.dobj.as_mut() {
                    dobj.set_world_pose(origin, angles);
                }
                row.followed_pose = Some((origin, angles));
            }
            shown.origin = origin;
            shown.angles = angles;
        }
        if shown.hidden != hidden {
            if let Some(mover) = f.script_mover_mut_by_number(shown.number) {
                if hidden {
                    mover.state.e_flags |= entity_iw4::CG_SCRIPT_MOVER_NODRAW;
                } else {
                    mover.state.e_flags &= !entity_iw4::CG_SCRIPT_MOVER_NODRAW;
                }
                mover.nonsolid = true;
            }
            if let Some(row) = f.collision_owner_mut(shown.id) {
                row.hidden = hidden;
                row.solid = false;
            }
            shown.hidden = hidden;
        }
        if shown.model != model || shown.attachments != attachments {
            let capability = f.model_capability(&model).flatten();
            if let Some(row) = f.collision_owner_mut(shown.id) {
                match row.dobj.as_mut() {
                    Some(dobj) => {
                        if dobj.current_model != model {
                            dobj.replace_model(&model, capability);
                        }
                    }
                    None => {
                        row.dobj = Some(AuthorityDObjState::at_pose(
                            &model, capability, origin, angles,
                        ))
                    }
                }
                if let Some(dobj) = row.dobj.as_mut() {
                    let a: Vec<(&str, &str)> = attachments
                        .iter()
                        .map(|(m, t)| (m.as_str(), t.as_str()))
                        .collect();
                    dobj.set_attachments(&a);
                }
            }
            shown.model = model;
            shown.attachments = attachments;
        }
        // Actors: bullets hit the living (their posed hit boxes), and the
        // playing animation goes out as the model row's one leaf.
        let alive_actor = world
            .resource::<Zm>()
            .actors
            .get(&n)
            .is_some_and(|a| a.alive);
        let boxes = if alive_actor && !hidden {
            let f = frame(world);
            let placement = f
                .entity_collision_capabilities()
                .iter()
                .find(|row| row.owner.script_model() == Some(shown.id))
                .and_then(|row| row.dobj.as_ref())
                .map(|d| d.world_from_model);
            placement.and_then(|m| super::actors::hitboxes(world, n, now_ms, m))
        } else {
            None
        };
        {
            let mut f = frame(world);
            let packed = if boxes.is_some() { ZOMBIE_SOLID } else { 0 };
            if let Some(mover) = f.script_mover_mut_by_number(shown.number)
                && mover.state.solid != packed
            {
                mover.state.solid = packed;
            }
            if let Some(row) = f.collision_owner_mut(shown.id) {
                row.solid = boxes.is_some();
                if let Some(dobj) = row.dobj.as_mut() {
                    dobj.current_collision = boxes.map(|bones| {
                        crate::bullet_collision::AuthorityDObjCollision { bones, coll: None }
                    });
                    dobj.materialized_model_revision = Some(dobj.model_revision);
                    dobj.materialized_pose_revision = Some(dobj.pose_revision);
                }
            }
        }
        let anim = if n & PIECE_BIT != 0 {
            let (e, i) = ((n & !PIECE_BIT) >> 4, (n & 0xf) as usize);
            world
                .resource::<Zm>()
                .ents
                .get(&e)
                .and_then(|e| e.zbarrier.as_ref())
                .and_then(|z| z.pieces.get(i))
                .and_then(|p| super::zbarrier::piece_pose(p, now_ms).1)
        } else {
            super::actors::present_anim(world, n, now_ms)
        };
        if let Some((clip, time, rate)) = anim {
            let mut f = frame(world);
            if let Some(row) = f.collision_owner_mut(shown.id)
                && let Some(dobj) = row.dobj.as_mut()
            {
                use xmodel_runtime::{
                    XAnimNodeState, XAnimSemanticNode, XAnimSemanticNodeKind, XAnimTreeSnapshot,
                };
                let rev = dobj.semantic_state.pose_revision.wrapping_add(1);
                dobj.semantic_state.pose_revision = rev;
                dobj.semantic_state.tree = Some(XAnimTreeSnapshot {
                    definition_revision: 1,
                    state_revision: rev,
                    nodes: vec![XAnimSemanticNode {
                        parent: None,
                        kind: XAnimSemanticNodeKind::Leaf,
                        clip: Some(clip),
                        parts: None,
                        state: XAnimNodeState {
                            time,
                            old_time: time,
                            weight: 1.0,
                            goal_weight: 1.0,
                            rate,
                            ..XAnimNodeState::default()
                        },
                    }],
                });
            }
        }
        world.resource_mut::<Zm>().presences.by_ent.insert(n, shown);
    }
    // The collision models as turned boxes, for his movement (the server's
    // and his client's prediction: they go out in the snapshot).
    let mut f = frame(world);
    let blockers: Vec<crate::OrientedBlocker> = blocker_ents
        .iter()
        .filter_map(|(n, origin, angles, model)| {
            let (lo, hi) = match named_wall_size(model) {
                Some(half) => (half.map(|v| -v), half),
                None => {
                    // The model's bounds come as middle and half-size.
                    let (mid, half) = f.model_capability(model).flatten().and_then(|c| c.bounds)?;
                    (
                        std::array::from_fn(|i| mid[i] - half[i]),
                        std::array::from_fn(|i| mid[i] + half[i]),
                    )
                }
            };
            let (fw, rt, up) = gsc_t6::math::angle_vectors(*angles);
            Some(crate::OrientedBlocker {
                entnum: *n as u16,
                origin: *origin,
                axis: [fw, [-rt[0], -rt[1], -rt[2]], up],
                mins: lo,
                maxs: hi,
            })
        })
        .collect();
    // IW4L_T6_BLOCKERLOG=1: each blocker when the list changes (test aid).
    if std::env::var_os("IW4L_T6_BLOCKERLOG").is_some()
        && f.oriented_blockers() != blockers.as_slice()
    {
        for b in &blockers {
            diag::info!(
                Sim,
                "bo2zm t6 blocker ent{} at ({:.0} {:.0} {:.0}) fwd ({:.2} {:.2} {:.2}) up ({:.2} {:.2} {:.2}) bounds {:?}..{:?}",
                b.entnum,
                b.origin[0],
                b.origin[1],
                b.origin[2],
                b.axis[0][0],
                b.axis[0][1],
                b.axis[0][2],
                b.axis[2][0],
                b.axis[2][1],
                b.axis[2][2],
                b.mins,
                b.maxs
            );
        }
    }
    f.set_oriented_blockers(blockers);
}
