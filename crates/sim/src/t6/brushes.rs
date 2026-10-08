//! Black Ops II's brush-model entities (doors, debris piles: a
//! `script_brushmodel` whose model is a map submodel, `*49`): where their
//! brush stands in the world, what they block, and the AI paths they cut
//! while closed (`disconnectpaths` / `connectpaths`).

use std::collections::BTreeMap;

use bevy_ecs::prelude::World;
use gsc_t6::{Value, Vm};

use super::{Ent, Zm, entnum, frame};
use crate::ScriptModelId;
use crate::bullet_collision::{EntityCollisionCapabilities, LinkedBrushCollisionBrush};

/// A brush entity's collision as last sent.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BrushRow {
    pub origin: [f32; 3],
    pub angles: [f32; 3],
    pub solid: bool,
    /// The never-drawn networked entity that carries it to the client's
    /// prediction (a brush mover naming the submodel, as MW2's are).
    pub number: Option<i32>,
    /// It moved last tick: its client carries it on until told it stopped.
    pub moving: bool,
}

/// Collision rows for brush entities: their own range, clear of the
/// script movers' (`presence`) and the loop sounds' owners.
const T6_BRUSH_BASE: u32 = 0x6800_0000;

fn owner(n: u32) -> ScriptModelId {
    ScriptModelId::from_wire(T6_BRUSH_BASE | (n & 0x07ff_ffff))
}

/// Every `script_brushmodel` blocks players, zombies and bullets as its
/// submodel's brushes, placed by the entity's origin and angles (a door
/// swings open and stands against the wall, solid again), while the
/// scripts keep it solid (`notsolid` while it moves; a bought debris pile
/// is deleted). The brushes are the map's own: no surfaces, collision
/// only (the visible door is a script model). A never-drawn brush mover
/// carries each to the client, whose movement prediction clips against it.
pub(super) fn sync(world: &mut World, now_ms: i64) {
    let wanted: BTreeMap<u32, (u32, [f32; 3], [f32; 3], bool)> = {
        let zm = world.resource::<Zm>();
        zm.ents
            .iter()
            .filter(|(_, e)| e.classname == "script_brushmodel")
            .filter_map(|(n, e)| {
                let m = super::triggers::brush_index(e)?;
                Some((*n, (m, e.origin, e.angles, e.solid)))
            })
            .collect()
    };
    let log = std::env::var_os("IW4L_T6_PATHLOG").is_some();
    let now = now_ms as i32;
    let mut rows = std::mem::take(&mut world.resource_mut::<Zm>().brush_rows);
    {
        let mut f = frame(world);
        rows.retain(|n, row| {
            if wanted.contains_key(n) {
                return true;
            }
            f.remove_collision_owner(owner(*n));
            if let Some(number) = row.number {
                f.remove_script_mover_by_number(number);
            }
            false
        });
        for (n, &(cmodel, origin, angles, solid)) in &wanted {
            let row = rows.entry(*n).or_insert_with(|| {
                let number = f.spawn_script_mover(owner(*n), origin, angles).ok();
                match number.and_then(|number| f.script_mover_mut_by_number(number)) {
                    Some(mover) => {
                        mover.state.index = cmodel as i32;
                        mover.state.solid = entity_iw4::SCRIPT_MOVER_BMODEL_SOLID;
                        mover.state.e_flags |= entity_iw4::CG_SCRIPT_MOVER_NODRAW;
                        mover.nonsolid = !solid;
                    }
                    None => diag::warn!(Sim, "bo2zm t6: no free entity for brush *{cmodel}"),
                }
                let mut cap = EntityCollisionCapabilities::current_tick(
                    crate::AuthorityModelOwner::ScriptModel(owner(*n)),
                    None,
                    vec![LinkedBrushCollisionBrush {
                        cmodel_handle: cmodel,
                        origin,
                        angles,
                    }],
                );
                cap.solid = solid;
                cap.followed_pose = Some((origin, angles));
                f.insert_collision_owner(cap);
                BrushRow {
                    origin,
                    angles,
                    solid,
                    number,
                    moving: false,
                }
            });
            let moved = row.origin != origin || row.angles != angles;
            // A move that ended goes out once more, standing still.
            if moved || row.moving {
                row.moving = moved;
                if let Some(number) = row.number {
                    f.set_script_mover_pose(number, now, origin, angles);
                }
                if let Some(cap) = f.collision_owner_mut(owner(*n)) {
                    for brush in &mut cap.linked_brushes {
                        brush.origin = origin;
                        brush.angles = angles;
                    }
                    cap.followed_pose = Some((origin, angles));
                }
                row.origin = origin;
                row.angles = angles;
            }
            if row.solid != solid {
                row.solid = solid;
                if let Some(mover) = row
                    .number
                    .and_then(|number| f.script_mover_mut_by_number(number))
                {
                    mover.nonsolid = !solid;
                }
                if let Some(cap) = f.collision_owner_mut(owner(*n)) {
                    cap.solid = solid;
                }
                if log {
                    diag::info!(Sim, "bo2zm t6 brush ent{n} *{cmodel}: solid {solid}");
                }
            }
        }
    }
    world.resource_mut::<Zm>().brush_rows = rows;
}

/// The entity's submodel bounds (its own space: a submodel's brushes are
/// authored about the entity's origin, measured: door `*49` spans
/// -1.8..23.8 x -0.6..59 x -52..52 at origin -503 361 -6), placed by its
/// origin and angles: the world box around them.
pub(super) fn world_box(world: &mut World, e: &Ent) -> Option<([f32; 3], [f32; 3])> {
    let n = super::triggers::brush_index(e)?;
    let (lo, hi) = {
        let f = frame(world);
        let m = f.clip_cmodels().models.get(n as usize)?;
        (m.mins, m.maxs)
    };
    let (fw, rt, up) = gsc_t6::math::angle_vectors(e.angles);
    // IW axes: forward, left (= -right), up.
    let left = [-rt[0], -rt[1], -rt[2]];
    let mut wlo = [f32::MAX; 3];
    let mut whi = [f32::MIN; 3];
    for corner in 0..8 {
        let p = [
            if corner & 1 == 0 { lo[0] } else { hi[0] },
            if corner & 2 == 0 { lo[1] } else { hi[1] },
            if corner & 4 == 0 { lo[2] } else { hi[2] },
        ];
        for i in 0..3 {
            let w = e.origin[i] + fw[i] * p[0] + left[i] * p[1] + up[i] * p[2];
            wlo[i] = wlo[i].min(w);
            whi[i] = whi[i].max(w);
        }
    }
    Some((wlo, whi))
}

/// A model entity's world box: its model's bounds turned by its angles,
/// placed.
fn model_box(world: &mut World, e: &Ent) -> Option<([f32; 3], [f32; 3])> {
    let (mid, half) = frame(world).model_capability(&e.model).flatten()?.bounds?;
    let (fw, rt, up) = gsc_t6::math::angle_vectors(e.angles);
    let left = [-rt[0], -rt[1], -rt[2]];
    let mut wlo = [f32::MAX; 3];
    let mut whi = [f32::MIN; 3];
    for corner in 0..8 {
        let p: [f32; 3] = std::array::from_fn(|i| {
            if corner & (1 << i) == 0 {
                mid[i] - half[i]
            } else {
                mid[i] + half[i]
            }
        });
        for i in 0..3 {
            let w = e.origin[i] + fw[i] * p[0] + left[i] * p[1] + up[i] * p[2];
            wlo[i] = wlo[i].min(w);
            whi[i] = whi[i].max(w);
        }
    }
    Some((wlo, whi))
}

/// Once, at the start: each debris pile's pieces that the map flags as
/// cutting paths (spawnflags 1, BO2's dynamic path: the truck's chairs,
/// armoires and clip) cut the zombies' paths through them, as BO2's engine
/// does when they spawn; buying the debris joins them again (the scripts'
/// `connectpaths` on each piece). Before, zombies walked through the truck
/// while its debris still stood.
pub(crate) fn cut_debris_paths(world: &mut World) {
    world.resource_mut::<Zm>().debris_paths_cut = true;
    let pieces: Vec<(u32, Ent)> = super::with_vm(world, |vm, world| {
        let (tn, target, flags, model_f) = (
            vm.intern("targetname"),
            vm.intern("target"),
            vm.intern("spawnflags"),
            vm.intern("model"),
        );
        let zm = world.resource::<Zm>();
        let text = |vm: &Vm<World>, e: &Ent, f| {
            e.obj
                .map(|o| vm.to_text(&vm.raw_field(o, f)))
                .unwrap_or_default()
        };
        let targets: Vec<String> = zm
            .ents
            .values()
            .filter(|e| text(vm, e, tn) == "zombie_debris")
            .map(|e| text(vm, e, target))
            .filter(|t| !t.is_empty())
            .collect();
        zm.ents
            .iter()
            .filter(|(_, e)| targets.contains(&text(vm, e, tn)))
            .filter(|(_, e)| {
                e.obj
                    .and_then(|o| vm.raw_field(o, flags).as_int())
                    .is_some_and(|v| v & 1 != 0)
            })
            .map(|(n, e)| {
                // A clip brush's model may live only in its script fields.
                let mut e = e.clone();
                if e.model.is_empty()
                    && let Some(o) = e.obj
                {
                    e.model = vm.to_text(&vm.raw_field(o, model_f));
                }
                (*n, e)
            })
            .collect()
    })
    .unwrap_or_default();
    for (n, e) in pieces {
        let bounds = if e.model.starts_with('*') {
            world_box(world, &e)
        } else {
            model_box(world, &e)
        };
        if bounds.is_none() && std::env::var_os("IW4L_T6_PATHLOG").is_some() {
            diag::info!(Sim, "bo2zm t6 debris piece ent{n} {}: no bounds", e.model);
        }
        if let Some((lo, hi)) = bounds {
            let mut zm = world.resource_mut::<Zm>();
            zm.nav.disconnect(n, lo, hi);
            if std::env::var_os("IW4L_T6_PATHLOG").is_some() {
                let cut = zm.nav.cut_count();
                diag::info!(
                    Sim,
                    "bo2zm t6 debris piece ent{n} {} cuts paths: {cut} links cut in all",
                    e.model
                );
            }
        }
    }
}

pub(super) fn bind(vm: &mut Vm<World>) {
    // A closed door cuts the paths through it; an opened one joins them.
    vm.bind("disconnectpaths", true, |vm, world, s, _| {
        let Some(n) = entnum(vm, s) else {
            return Ok(Value::Undefined);
        };
        let Some(e) = world.resource::<Zm>().ents.get(&n).cloned() else {
            return Ok(Value::Undefined);
        };
        if let Some((lo, hi)) = world_box(world, &e) {
            let mut zm = world.resource_mut::<Zm>();
            zm.nav.disconnect(n, lo, hi);
            if std::env::var_os("IW4L_T6_PATHLOG").is_some() {
                let cut = zm.nav.cut_count();
                diag::info!(
                    Sim,
                    "bo2zm t6 disconnectpaths ent{n} box ({:.0} {:.0} {:.0})..({:.0} {:.0} {:.0}): {cut} links cut in all",
                    lo[0],
                    lo[1],
                    lo[2],
                    hi[0],
                    hi[1],
                    hi[2]
                );
            }
        }
        Ok(Value::Undefined)
    });
    vm.bind("connectpaths", true, |vm, world, s, _| {
        if let Some(n) = entnum(vm, s) {
            let mut zm = world.resource_mut::<Zm>();
            zm.nav.connect(n);
            if std::env::var_os("IW4L_T6_PATHLOG").is_some() {
                diag::info!(
                    Sim,
                    "bo2zm t6 connectpaths ent{n}: {} links cut in all",
                    zm.nav.cut_count()
                );
            }
        }
        Ok(Value::Undefined)
    });
}
