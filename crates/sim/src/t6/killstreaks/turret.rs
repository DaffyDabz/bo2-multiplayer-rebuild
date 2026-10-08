//! Scorestreak turrets (`spawnturret`: the Sentry Gun, the Guardian, the
//! remote turrets).
//!
//! BO2's scripts run a turret's life: carried in front of its owner
//! (`carryturret`) until he finds a good spot (`canplayerplaceturret`) and
//! fires to set it down (`stopcarryturret`), then it fires in bursts
//! (`_mgturret::burst_fire_unmanned`: `shootturret` each 0.112 s while
//! `isfiringturret`). The engine half kept here: where a carried turret
//! sits, the enemy an automatic turret has in its sights (nearest living
//! enemy it can see; `turret_target_aquired` / `turret_target_lost`), and a
//! shot's hit on it with the turret's own weapon.

use bevy_ecs::prelude::World;
use gsc_t6::{Array, Key, ObjKind, Value, Vm};

use super::super::{Ent, Zm, arg, entnum, frame, text};
use super::{notify_all, streaks};
use crate::world::ClientId;

/// Where its gun sits above its origin.
const EYE: f32 = 40.0;

#[derive(Clone, Debug, Default)]
pub(crate) struct Turret {
    pub weapon: String,
    pub owner: Option<u32>,
    pub team: String,
    /// The player carrying it (`carryturret`).
    pub carrier: Option<u32>,
    /// `setmode`: "auto_ai" / "auto_nonai" turrets pick their own targets;
    /// "manual" ones are driven by a player.
    pub mode: String,
    pub target: Option<u32>,
}

pub(super) fn bind(vm: &mut Vm<World>) {
    macro_rules! f {
        ($name:literal, $body:expr) => {
            vm.bind($name, false, $body);
        };
    }
    macro_rules! m {
        ($name:literal, $body:expr) => {
            vm.bind($name, true, $body);
        };
    }
    // `spawnturret(classname, origin, weapon)`.
    f!("spawnturret", |vm, world, _, a| {
        let cls = text(vm, a, 0);
        let origin = super::super::vec3(a, 1)?;
        let weapon = text(vm, a, 2);
        let n = world.resource_mut::<Zm>().alloc_entnum();
        let obj = vm.alloc_object(ObjKind::Entity(n));
        world.resource_mut::<Zm>().ents.insert(
            n,
            Ent {
                obj: Some(obj),
                classname: cls,
                origin,
                can_damage: true,
                ..Default::default()
            },
        );
        streaks(world).turrets.insert(
            n,
            Turret {
                weapon,
                mode: "auto_ai".to_owned(),
                ..Default::default()
            },
        );
        Ok(Value::Object(obj))
    });
    fn with_turret(vm: &Vm<World>, world: &mut World, s: &Value, f: impl FnOnce(&mut Turret)) {
        if let Some(n) = entnum(vm, s)
            && let Some(t) = streaks(world).turrets.get_mut(&n)
        {
            f(t);
        }
    }
    m!("setturretowner", |vm, world, s, a| {
        let o = entnum(vm, arg(a, 0));
        with_turret(vm, world, s, |t| t.owner = o);
        Ok(Value::Undefined)
    });
    m!("setturretteam", |vm, world, s, a| {
        let team = text(vm, a, 0);
        with_turret(vm, world, s, |t| t.team = team);
        Ok(Value::Undefined)
    });
    m!("setmode", |vm, world, s, a| {
        let mode = text(vm, a, 0);
        with_turret(vm, world, s, |t| t.mode = mode);
        Ok(Value::Undefined)
    });
    m!("settargetentity", |vm, world, s, a| {
        let target = entnum(vm, arg(a, 0));
        with_turret(vm, world, s, |t| t.target = target);
        Ok(Value::Undefined)
    });
    m!("cleartargetentity", |vm, world, s, _| {
        with_turret(vm, world, s, |t| t.target = None);
        Ok(Value::Undefined)
    });
    m!("getturrettarget", |vm, world, s, _| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        let t = streaks(world).turrets.get(&n).and_then(|t| t.target);
        Ok(t.map_or(Value::Undefined, |t| super::craft::world_obj(world, t)))
    });
    // A player carries it in front of him.
    m!("carryturret", |vm, world, s, a| {
        let carrier = entnum(vm, s);
        let n = entnum(vm, arg(a, 0)).unwrap_or(u32::MAX);
        if let Some(t) = streaks(world).turrets.get_mut(&n) {
            t.carrier = carrier;
            t.target = None;
        }
        Ok(Value::Undefined)
    });
    m!("stopcarryturret", |vm, world, _, a| {
        let n = entnum(vm, arg(a, 0)).unwrap_or(u32::MAX);
        if let Some(t) = streaks(world).turrets.get_mut(&n) {
            t.carrier = None;
        }
        let origin = arg(a, 1).as_vec3();
        let angles = arg(a, 2).as_vec3();
        if let Some(e) = world.resource_mut::<Zm>().ents.get_mut(&n) {
            if let Some(o) = origin {
                e.origin = o;
            }
            if let Some(ang) = angles {
                e.angles = ang;
            }
        }
        Ok(Value::Undefined)
    });
    // Where a carried turret (or a drone, `canplayerplacevehicle(width,
    // length, distance, ...)`) would stand: on the ground that far in front
    // of him, facing his way; good when the ground is there and level.
    m!("canplayerplaceturret", |vm, world, s, _| place_ahead(
        vm, world, s, 40.0
    ));
    m!("canplayerplacevehicle", |vm, world, s, a| {
        let dist = arg(a, 2).as_float().unwrap_or(50.0);
        place_ahead(vm, world, s, dist)
    });
    // It has an enemy in its sights.
    m!("isfiringturret", |vm, world, s, _| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        Ok(Value::bool(streaks(world).turrets.get(&n).is_some_and(
            |t| t.carrier.is_none() && t.target.is_some(),
        )))
    });
    // One shot at its target: the turret weapon's damage (less with range)
    // when the shot can reach him.
    m!("shootturret", |vm, world, s, _| {
        let Some(n) = entnum(vm, s) else {
            return Ok(Value::Undefined);
        };
        let Some(t) = streaks(world).turrets.get(&n).cloned() else {
            return Ok(Value::Undefined);
        };
        let Some(target) = t.target else {
            return Ok(Value::Undefined);
        };
        let Some(origin) = world.resource::<Zm>().ents.get(&n).map(|e| e.origin) else {
            return Ok(Value::Undefined);
        };
        let eye = [origin[0], origin[1], origin[2] + EYE];
        let Some(ps) = frame(world).player(ClientId(target)).copied() else {
            return Ok(Value::Undefined);
        };
        let mid = [ps.origin[0], ps.origin[1], ps.origin[2] + 40.0];
        if ps.health <= 0 || !sees(world, eye, mid) {
            return Ok(Value::Undefined);
        }
        let w = super::super::weapon(world, &t.weapon).unwrap_or(0);
        // Its flash, sound, tracer and the hit, from its gun's muzzle.
        let muzzle = super::craft::muzzle(world, n, None);
        super::craft::fire_event(world, n, w, muzzle, mid);
        super::craft::bullet_fx(world, n, w, muzzle, mid, Some(target));
        let facts = frame(world).combat_facts_for(w);
        let d = dist(eye, mid);
        let amount = facts.map_or(0, |f| {
            if d <= f.max_damage_range {
                f.damage
            } else if d >= f.min_damage_range {
                f.min_damage
            } else {
                let k =
                    (d - f.max_damage_range) / (f.min_damage_range - f.max_damage_range).max(1.0);
                (f.damage as f32 + (f.min_damage - f.damage) as f32 * k).round() as i32
            }
        });
        if amount <= 0 {
            return Ok(Value::Undefined);
        }
        let attacker = t
            .owner
            .map_or(s.clone(), |o| super::craft::world_obj(world, o));
        let dir = gsc_t6::math::normalize(gsc_t6::math::sub(mid, eye));
        super::super::natives_ai::player_damage(
            vm,
            world,
            target,
            s.clone(),
            attacker,
            amount,
            0,
            "MOD_RIFLE_BULLET",
            &t.weapon,
            mid,
            dir,
            "torso_upper",
        );
        Ok(Value::Undefined)
    });
    // Its look: type, minimap mark, laser, use prompt, being carried.
    for name in [
        "setturrettype",
        "setturretminimapvisible",
        "maketurretusable",
        "maketurretunusable",
        "setturretcarried",
        "laseron",
        "laseroff",
        "setturretspinning",
        "setturretfiretime",
    ] {
        vm.bind(name, true, |_, _, _, _| Ok(Value::Undefined));
    }
}

fn place_ahead(
    vm: &mut Vm<World>,
    world: &mut World,
    s: &Value,
    dist: f32,
) -> Result<Value, String> {
    let id = super::super::client(vm, world, s)?;
    let Some(ps) = frame(world).player(id).copied() else {
        return Ok(Value::Undefined);
    };
    let yaw = ps.viewangles[1];
    let (sy, cy) = yaw.to_radians().sin_cos();
    let ahead = [
        ps.origin[0] + cy * dist,
        ps.origin[1] + sy * dist,
        ps.origin[2] + 32.0,
    ];
    let down = [ahead[0], ahead[1], ahead[2] - 96.0];
    let t = frame(world).trace_static_world(
        ahead,
        down,
        [0.0; 3],
        [0.0; 3],
        crate::bullet_collision::MASK_SHOT,
    );
    let f = t.fraction.clamp(0.0, 1.0);
    let at: [f32; 3] = std::array::from_fn(|i| ahead[i] + (down[i] - ahead[i]) * f);
    let good = f < 1.0 && t.normal[2] > 0.7;
    let mut arr = Array::new();
    let k = |vm: &mut Vm<World>, s: &str| Key::Str(vm.intern(s));
    arr.set(k(vm, "origin"), Value::Vec3(at));
    arr.set(k(vm, "angles"), Value::Vec3([0.0, yaw, 0.0]));
    arr.set(k(vm, "result"), Value::Int(i32::from(good)));
    Ok(Value::array(arr))
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn sees(world: &mut World, from: [f32; 3], to: [f32; 3]) -> bool {
    frame(world)
        .trace_static_world(
            from,
            to,
            [0.0; 3],
            [0.0; 3],
            crate::bullet_collision::MASK_SHOT,
        )
        .fraction
        >= 1.0
}

/// Each tick: carried turrets follow their carrier; automatic ones pick the
/// nearest living enemy they can see (within 2400 units) and turn to him.
pub(super) fn advance(world: &mut World) {
    let ids: Vec<u32> = streaks(world).turrets.keys().copied().collect();
    if ids.is_empty() {
        return;
    }
    let teams: Vec<(u32, String)> = super::super::with_vm(world, |vm, world| {
        let f = vm.intern("team");
        world
            .resource::<Zm>()
            .players
            .iter()
            .map(|(c, p)| (*c, vm.to_text(&vm.raw_field(p.obj, f))))
            .collect()
    })
    .unwrap_or_default();
    let mut events = Vec::new();
    for n in ids {
        let Some(origin) = world.resource::<Zm>().ents.get(&n).map(|e| e.origin) else {
            streaks(world).turrets.remove(&n);
            continue;
        };
        let Some(mut t) = streaks(world).turrets.get(&n).cloned() else {
            continue;
        };
        if let Some(c) = t.carrier {
            if let Some(ps) = frame(world).player(ClientId(c)).copied() {
                let (sy, cy) = ps.viewangles[1].to_radians().sin_cos();
                if let Some(e) = world.resource_mut::<Zm>().ents.get_mut(&n) {
                    e.origin = [
                        ps.origin[0] + cy * 40.0,
                        ps.origin[1] + sy * 40.0,
                        ps.origin[2],
                    ];
                    e.angles = [0.0, ps.viewangles[1], 0.0];
                }
            }
            continue;
        }
        if !t.mode.starts_with("auto") {
            continue;
        }
        let eye = [origin[0], origin[1], origin[2] + EYE];
        let mut best: Option<(f32, u32)> = None;
        for (c, team) in &teams {
            if Some(*c) == t.owner || (!t.team.is_empty() && t.team != "free" && *team == t.team) {
                continue;
            }
            let Some(ps) = frame(world).player(ClientId(*c)).copied() else {
                continue;
            };
            if ps.health <= 0 {
                continue;
            }
            let mid = [ps.origin[0], ps.origin[1], ps.origin[2] + 40.0];
            let d = dist(eye, mid);
            if d > 2400.0 || best.is_some_and(|b| b.0 <= d) || !sees(world, eye, mid) {
                continue;
            }
            best = Some((d, *c));
        }
        let now = best.map(|b| b.1);
        // Its firing state changes with its target (the scripts' burst
        // loop waits for `turretstatechange`).
        if now.is_some() != t.target.is_some() {
            events.push((n, "turretstatechange", Vec::new()));
        }
        if now != t.target {
            match now {
                Some(c) => {
                    let who = super::craft::world_obj(world, c);
                    events.push((n, "turret_target_aquired", vec![who]));
                }
                None => events.push((n, "turret_target_lost", Vec::new())),
            }
            t.target = now;
        }
        if let Some(c) = now
            && let Some(ps) = frame(world).player(ClientId(c)).copied()
        {
            let yaw = (ps.origin[1] - origin[1])
                .atan2(ps.origin[0] - origin[0])
                .to_degrees();
            if let Some(e) = world.resource_mut::<Zm>().ents.get_mut(&n) {
                e.angles = [0.0, yaw, 0.0];
            }
        }
        streaks(world).turrets.insert(n, t);
    }
    notify_all(world, events);
}
