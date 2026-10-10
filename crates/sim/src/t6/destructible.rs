//! bo2mp: a Black Ops II map's breakables (Nuketown's mannequins, its cars,
//! its clock). A hit breaks pieces in t5_destructible; the breaks reach the
//! map's scripts here, as BO2's `codecallback_destructibleevent` ("broken",
//! "breakafter") and the entity's "death".

use bevy_ecs::prelude::World;
use gsc_t6::Value;

use super::{Zm, frame, tick, weapon_text, with_vm};
use crate::t5_destructible::{DamageKind, Notice};
use crate::world::ClientId;
use crate::{AuthorityModelOwner, ScriptModelId};

/// Which of a piece's scales a hit counts by, from its means of death.
pub(super) fn kind(means: &str) -> DamageKind {
    if means == "MOD_MELEE" {
        DamageKind::Melee
    } else if ["GRENADE", "PROJECTILE", "EXPLOSIVE", "SPLASH"]
        .iter()
        .any(|k| means.contains(k))
    {
        DamageKind::Explosive
    } else if means.contains("BULLET") || means == "MOD_HEAD_SHOT" {
        DamageKind::Bullet
    } else {
        DamageKind::Script
    }
}

/// The breakables a blast at `origin` reaches: (shown model, its nearest
/// part, distance).
pub(super) fn radius_targets(
    world: &mut World,
    origin: [f32; 3],
    radius: f32,
) -> Vec<(ScriptModelId, [f32; 3], f32)> {
    let shown: Vec<(ScriptModelId, [f32; 3])> = {
        let zm = world.resource::<Zm>();
        zm.ents
            .iter()
            .filter(|(_, e)| e.destructible.is_some())
            .filter_map(|(n, e)| Some((zm.presences.by_ent.get(n)?.id, e.origin)))
            .collect()
    };
    if shown.is_empty() {
        return Vec::new();
    }
    let f = frame(world);
    shown
        .into_iter()
        .filter_map(|(id, at)| {
            let parts: Vec<[f32; 3]> = f
                .entity_collision_capabilities()
                .iter()
                .find(|row| row.owner.script_model() == Some(id))
                .and_then(|row| row.dobj.as_ref())
                .and_then(|d| d.current_collision.as_ref())
                .map(|c| c.bones.iter().map(|b| b.center).collect())
                .unwrap_or_default();
            let (mid, d) = parts
                .into_iter()
                .chain(std::iter::once(at))
                .map(|p| (p, gsc_t6::math::length(gsc_t6::math::sub(p, origin))))
                .min_by(|a, b| a.1.total_cmp(&b.1))?;
            (d < radius).then_some((id, mid, d))
        })
        .collect()
}

/// A script's `dodamage` on entity `n`: a breakable takes it on its base
/// piece (a burning car's fire running out). Nothing for anything else.
pub(super) fn script_damage(
    world: &mut World,
    n: u32,
    amount: i32,
    means: &str,
    attacker: Option<ClientId>,
    weapon: &str,
) {
    let id = {
        let zm = world.resource::<Zm>();
        if !zm.ents.get(&n).is_some_and(|e| e.destructible.is_some()) {
            return;
        }
        let Some(shown) = zm.presences.by_ent.get(&n) else {
            return;
        };
        shown.id
    };
    let t = tick(world);
    let mut f = frame(world);
    let weapon = crate::script_player::weapon_named(&f, weapon).unwrap_or(0);
    crate::t5_destructible::apply_damage(
        &mut f,
        t,
        AuthorityModelOwner::ScriptModel(id),
        None,
        amount,
        kind(means),
        attacker,
        weapon,
    );
}

/// Each breakable's breaks since last tick, to the map's scripts.
pub(super) fn flush(world: &mut World) {
    let rows: Vec<(gsc_t6::ObjRef, ScriptModelId)> = {
        let zm = world.resource::<Zm>();
        if zm.destructibles.is_empty() {
            return;
        }
        zm.ents
            .iter()
            .filter(|(_, e)| e.destructible.is_some())
            .filter_map(|(n, e)| Some((e.obj?, zm.presences.by_ent.get(n)?.id)))
            .collect()
    };
    let mut heard = Vec::new();
    {
        let mut f = frame(world);
        for (obj, id) in rows {
            let Some(dobj) = f.collision_owner_mut(id).and_then(|r| r.dobj.as_mut()) else {
                continue;
            };
            let notices = crate::t5_destructible::take_notices(dobj);
            if !notices.is_empty() {
                heard.push((obj, notices));
            }
        }
    }
    if heard.is_empty() {
        return;
    }
    let cb = super::mp::callbacks(world);
    let mut named = Vec::new();
    for (obj, notices) in heard {
        for notice in notices {
            let weapon = match &notice {
                Notice::Broken { weapon, .. } => weapon_text(world, *weapon),
                _ => String::new(),
            };
            named.push((obj, notice, weapon));
        }
    }
    with_vm(world, |vm, world| {
        for (obj, notice, weapon) in named {
            match notice {
                Notice::Broken {
                    notify, attacker, ..
                } => {
                    diag::info!(Sim, "bo2mp destructible broken: {notify} ({weapon})");
                    let args = vec![
                        vm.string("broken"),
                        vm.string(&notify),
                        player(world, attacker),
                        vm.string(&weapon),
                    ];
                    vm.spawn_named(
                        world,
                        &cb,
                        "codecallback_destructibleevent",
                        Value::Object(obj),
                        args,
                    );
                }
                Notice::BreakAfter {
                    piece,
                    time,
                    damage,
                } => {
                    let args = vec![
                        vm.string("breakafter"),
                        Value::Int(i32::try_from(piece).unwrap_or(0)),
                        Value::Float(time),
                        Value::Int(damage),
                    ];
                    vm.spawn_named(
                        world,
                        &cb,
                        "codecallback_destructibleevent",
                        Value::Object(obj),
                        args,
                    );
                }
                Notice::Death { attacker } => {
                    let who = player(world, attacker);
                    vm.notify_str(world, obj, "death", &[who]);
                }
            }
        }
    });
}

fn player(world: &World, who: Option<ClientId>) -> Value {
    who.and_then(|a| {
        world
            .resource::<Zm>()
            .players
            .get(&a.0)
            .map(|p| Value::Object(p.obj))
    })
    .unwrap_or(Value::Undefined)
}
