use super::args::{arg, float, int, optional, string, vector};
use super::entities::EntityKind;
use crate::script::Namespace::{Function, Method};
use crate::script::runtime::{raise, run_now};
use crate::script::{NativeRegistry, Runtime, Value};
use bevy_ecs::prelude::World;

pub(crate) const DAMAGE: &str = "maps/mp/gametypes/_callbacksetup::codecallback_vehicledamage";

const MPH: f32 = 17.6;
const TICK_S: f32 = crate::MATCH_TICK_MS as f32 / 1000.0;
const ARRIVED: f32 = 4.0;
const GUNNER_RANGE: f32 = 8192.0;

#[derive(Clone, Debug)]
pub(crate) struct Heli {
    goal: Option<[f32; 3]>,
    stop_at_goal: bool,
    arrived: bool,
    near_goal: f32,
    near_notified: bool,
    speed: f32,
    max_speed: f32,
    accel: f32,
    decel: f32,
    heading: [f32; 3],
    yaw_speed: f32,
    target_yaw: Option<f32>,
    goal_yaw: Option<f32>,
    look_at: Option<u64>,
    max_pitch: f32,
    max_roll: f32,
    owner: Option<u32>,
    compass: Option<([String; 2], [i32; 2])>,
    weapon: Option<u32>,
    turret: Option<TurretAim>,
    on_target: bool,
    gunner: Option<u32>,
    next_fire_ms: i32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum TurretAim {
    Entity(u64, [f32; 3]),
    Point([f32; 3]),
}

impl Default for Heli {
    fn default() -> Self {
        Self {
            goal: None,
            stop_at_goal: true,
            arrived: false,
            near_goal: 0.0,
            near_notified: false,
            speed: 0.0,
            max_speed: 0.0,
            accel: 0.0,
            decel: 0.0,
            heading: [1.0, 0.0, 0.0],
            yaw_speed: 90.0,
            target_yaw: None,
            goal_yaw: None,
            look_at: None,
            max_pitch: 20.0,
            max_roll: 30.0,
            owner: None,
            compass: None,
            weapon: None,
            turret: None,
            on_target: false,
            gunner: None,
            next_fire_ms: 0,
        }
    }
}

fn heli<'a>(world: &'a mut World, receiver: &Value) -> Result<&'a mut Heli, String> {
    let Value::Object(id) = receiver else {
        return Err("receiver is not a vehicle".into());
    };
    world
        .resource_mut::<Runtime>()
        .into_inner()
        .vehicles
        .get_mut(id)
        .ok_or_else(|| "receiver is not a vehicle".into())
}

pub(crate) fn is_heli(world: &World, receiver: &Value) -> bool {
    matches!(receiver, Value::Object(id) if world.resource::<Runtime>().vehicles.contains_key(id))
}

pub(crate) fn aim_turret(
    world: &mut World,
    receiver: &Value,
    aim: Option<TurretAim>,
) -> Result<Value, String> {
    let heli = heli(world, receiver)?;
    heli.turret = aim;
    heli.on_target = false;
    Ok(Value::Undefined)
}

fn aim_point(world: &mut World, aim: TurretAim) -> Option<[f32; 3]> {
    match aim {
        TurretAim::Point(point) => Some(point),
        TurretAim::Entity(object, offset) => {
            if !world.resource::<Runtime>().live(&object) {
                return None;
            }
            match super::players::entity_field(world, object, "origin") {
                Value::Vector(o) => Some(std::array::from_fn(|i| o[i] + offset[i])),
                _ => None,
            }
        }
    }
}

fn gunner_input(world: &mut World, client: u32) -> Option<([f32; 3], bool)> {
    let mut frame = crate::frame::FrameWorld::from_world(world);
    let angles = frame.player(crate::ClientId(client))?.viewangles;
    let held = crate::script_player::buttons(&mut frame, crate::ClientId(client));
    Some((
        math_iw4::angle_vectors(angles).0,
        held & playerstate_iw4::buttons::ATTACK != 0,
    ))
}

fn fire_weapon(world: &mut World, receiver: &Value, args: &[Value]) -> Result<Value, String> {
    let Value::Object(id) = *receiver else {
        return Err("receiver is not a vehicle".into());
    };
    let (weapon, owner, turret, heading) = {
        let heli = heli(world, receiver)?;
        (heli.weapon, heli.owner, heli.turret, heli.heading)
    };
    let weapon = weapon.ok_or("vehicle has no weapon")?;
    let tag = optional(args, 0, string)?.unwrap_or_default();
    let from = super::presence::tag_world(world, id, &tag)
        .map(|(origin, _)| origin)
        .unwrap_or_else(|| super::turrets::muzzle(world, id));
    let aim = match args.get(1) {
        Some(target) if *target != Value::Undefined => {
            let target = super::natives::engine::entity_id(world, target)?;
            let offset = optional(args, 2, vector)?.unwrap_or([0.0; 3]);
            Some(TurretAim::Entity(target, offset))
        }
        _ => turret,
    };
    let point = aim.and_then(|aim| aim_point(world, aim));
    let dir = point
        .and_then(|p| {
            glam::Vec3::from_array(std::array::from_fn(|i| p[i] - from[i])).try_normalize()
        })
        .map_or(heading, |d| d.to_array());
    let kind = crate::frame::FrameWorld::from_world(world)
        .combat_facts_for(weapon)
        .and_then(|f| weapon_iw4::fire_weapon_kind(f.weap_type, f.weap_class));
    if matches!(kind, Some(weapon_iw4::FireWeaponKind::Bullet) | None) {
        super::turrets::fire_bullet(world, id, from, dir, weapon, owner.map(crate::ClientId));
        return Ok(Value::Undefined);
    }
    let owner = owner
        .filter(|client| world.resource::<Runtime>().players.contains_key(client))
        .ok_or("vehicle owner is not connected")?;
    let end = std::array::from_fn(|i| from[i] + dir[i] * 1000.0);
    super::weapons::launch(world, crate::ClientId(owner), weapon, from, end)
}

fn vec_field(runtime: &mut Runtime, object: u64, name: &str) -> [f32; 3] {
    match runtime.object_field(object, name) {
        Value::Vector(v) => v,
        _ => [0.0; 3],
    }
}

fn spawn_vehicle(
    world: &mut World,
    classname: &str,
    origin: [f32; 3],
    angles: [f32; 3],
    model: &str,
    flight: Option<Heli>,
) -> Result<Value, String> {
    let presence = super::presence::spawn_presence(world, origin)?;
    let mut runtime = world.resource_mut::<Runtime>();
    let id = runtime.create_entity(EntityKind::Vehicle, classname)?;
    runtime.set_object_field(id, "origin", Value::Vector(origin));
    runtime.set_object_field(id, "angles", Value::Vector(angles));
    runtime.set_object_field(id, "model", Value::string(model));
    runtime.entities.get_mut(&id).unwrap().presence = Some(presence);
    if let Some(mut flight) = flight {
        flight.heading = math_iw4::angle_vectors(angles).0;
        runtime.set_object_field(id, "veh_speed", Value::Float(0.0));
        runtime.vehicles.insert(id, flight);
    }
    Ok(Value::Object(id))
}

pub(crate) fn register(registry: &mut NativeRegistry) {
    registry.register(Function, "spawnhelicopter", |world, _, args| {
        let owner = world
            .resource::<Runtime>()
            .player_client_of(arg(args, 0)?)
            .ok_or("spawnHelicopter owner is not a player")?;
        let (origin, angles) = (vector(args, 1)?, vector(args, 2)?);
        let vehicle = string(args, 3)?;
        let model = string(args, 4)?;
        let frame = crate::frame::FrameWorld::from_world(world);
        let weapon = frame.vehicle_turret_weapon(&vehicle);
        let compass = frame.vehicle_compass(&vehicle).cloned();
        spawn_vehicle(
            world,
            "script_vehicle",
            origin,
            angles,
            &model,
            Some(Heli {
                owner: Some(owner),
                compass,
                weapon,
                ..Heli::default()
            }),
        )
    });
    registry.register(Function, "spawnplane", |world, _, args| {
        world
            .resource::<Runtime>()
            .player_client_of(arg(args, 0)?)
            .ok_or("spawnPlane owner is not a player")?;
        let classname = string(args, 1)?;
        let origin = vector(args, 2)?;
        spawn_vehicle(world, &classname, origin, [0.0; 3], "", None)
    });
    registry.register(Method, "setvehgoalpos", |world, receiver, args| {
        let goal = vector(args, 0)?;
        let stop = optional(args, 1, int)?.unwrap_or(0) != 0;
        let heli = heli(world, receiver)?;
        heli.goal = Some(goal);
        heli.stop_at_goal = stop;
        heli.arrived = false;
        heli.near_notified = false;
        Ok(Value::Undefined)
    });
    registry.register(Method, "vehicle_setspeed", |world, receiver, args| {
        let speed = float(args, 0)?.max(0.0) * MPH;
        let accel = optional(args, 1, float)?.unwrap_or(speed / MPH).max(1.0) * MPH;
        let decel = optional(args, 2, float)?.map_or(accel, |d| d.max(1.0) * MPH);
        let heli = heli(world, receiver)?;
        heli.max_speed = speed;
        heli.accel = accel;
        heli.decel = decel;
        Ok(Value::Undefined)
    });
    registry.register(
        Method,
        "vehicle_setspeedimmediate",
        |world, receiver, args| {
            let speed = float(args, 0)?.max(0.0) * MPH;
            let heli = heli(world, receiver)?;
            heli.max_speed = speed;
            heli.speed = speed;
            heli.accel = heli.accel.max(speed);
            heli.decel = heli.decel.max(speed);
            Ok(Value::Undefined)
        },
    );
    registry.register(Method, "vehicle_getspeed", |world, receiver, _| {
        Ok(Value::Float(heli(world, receiver)?.speed / MPH))
    });
    registry.register(Method, "setyawspeed", |world, receiver, args| {
        let speed = float(args, 0)?.max(0.0);
        heli(world, receiver)?.yaw_speed = speed;
        Ok(Value::Undefined)
    });
    registry.register(Method, "settargetyaw", |world, receiver, args| {
        let yaw = float(args, 0)?;
        heli(world, receiver)?.target_yaw = Some(yaw);
        Ok(Value::Undefined)
    });
    registry.register(Method, "cleartargetyaw", |world, receiver, _| {
        heli(world, receiver)?.target_yaw = None;
        Ok(Value::Undefined)
    });
    registry.register(Method, "setgoalyaw", |world, receiver, args| {
        let yaw = float(args, 0)?;
        heli(world, receiver)?.goal_yaw = Some(yaw);
        Ok(Value::Undefined)
    });
    registry.register(Method, "cleargoalyaw", |world, receiver, _| {
        heli(world, receiver)?.goal_yaw = None;
        Ok(Value::Undefined)
    });
    registry.register(Method, "setmaxpitchroll", |world, receiver, args| {
        let (pitch, roll) = (float(args, 0)?.abs(), float(args, 1)?.abs());
        let heli = heli(world, receiver)?;
        heli.max_pitch = pitch;
        heli.max_roll = roll;
        Ok(Value::Undefined)
    });
    registry.register(Method, "setneargoalnotifydist", |world, receiver, args| {
        let dist = float(args, 0)?.max(0.0);
        heli(world, receiver)?.near_goal = dist;
        Ok(Value::Undefined)
    });
    registry.register(Method, "setlookatent", |world, receiver, args| {
        let target = super::natives::engine::entity_id(world, arg(args, 0)?)?;
        heli(world, receiver)?.look_at = Some(target);
        Ok(Value::Undefined)
    });
    registry.register(Method, "clearlookatent", |world, receiver, _| {
        heli(world, receiver)?.look_at = None;
        Ok(Value::Undefined)
    });
    for name in ["sethoverparams", "setturningability", "setdamagestage"] {
        registry.register(Method, name, |world, receiver, _| {
            heli(world, receiver)?;
            Ok(Value::Undefined)
        });
    }
    registry.register(Method, "vehicleturretcontrolon", |world, receiver, args| {
        let client = world
            .resource::<Runtime>()
            .player_client_of(arg(args, 0)?)
            .ok_or("parameter 1: not a player")?;
        let heli = heli(world, receiver)?;
        heli.gunner = Some(client);
        heli.on_target = false;
        Ok(Value::Undefined)
    });
    registry.register(
        Method,
        "vehicleturretcontroloff",
        |world, receiver, args| {
            world
                .resource::<Runtime>()
                .player_client_of(arg(args, 0)?)
                .ok_or("parameter 1: not a player")?;
            let heli = heli(world, receiver)?;
            heli.gunner = None;
            heli.turret = None;
            Ok(Value::Undefined)
        },
    );
    registry.register(Method, "setvehweapon", |world, receiver, args| {
        let name = string(args, 0)?;
        let weapon = crate::script_player::weapon_named(
            &crate::frame::FrameWorld::from_world(world),
            &name,
        )?;
        heli(world, receiver)?.weapon = Some(weapon);
        Ok(Value::Undefined)
    });
    registry.register(Method, "setturrettargetvec", |world, receiver, args| {
        let point = vector(args, 0)?;
        aim_turret(world, receiver, Some(TurretAim::Point(point)))
    });
    registry.register(Method, "clearturrettarget", |world, receiver, _| {
        aim_turret(world, receiver, None)
    });
    registry.register(Method, "fireweapon", fire_weapon);
    // Vehicle turrets here traverse freely, so only the optional sight trace
    // from the muzzle can refuse a point.
    registry.register(
        Method,
        "vehicle_canturrettargetpoint",
        |world, receiver, args| {
            let Value::Object(id) = *receiver else {
                return Err("receiver is not a vehicle".into());
            };
            heli(world, receiver)?;
            let point = vector(args, 0)?;
            if optional(args, 1, int)?.unwrap_or(0) == 0 {
                return Ok(Value::Int(1));
            }
            let from = super::turrets::muzzle(world, id);
            let ignore = super::natives::engine::trace_ignore(world, args.get(2))
                .with(super::turrets::ignore_self(world, id));
            let outcome = super::natives::engine::entity_trace(
                world,
                from,
                point,
                crate::bullet_collision::MASK_SHOT,
                ignore,
            );
            Ok(Value::Int(
                matches!(outcome, crate::bullet_collision::TraceOutcome::Miss { .. }).into(),
            ))
        },
    );
    registry.register(Method, "vehicle_finishdamage", |world, receiver, args| {
        let Some((object, _)) = world.resource::<Runtime>().entity(receiver) else {
            return Err("receiver is not a vehicle".into());
        };
        let attacker = arg(args, 1)?.clone();
        let amount = int(args, 2)?;
        let flags = optional(args, 3, int)?.unwrap_or(0);
        let means = string(args, 4)?;
        let weapon = string(args, 5)?;
        let point = vector(args, 6)?;
        let dir = vector(args, 7)?;
        let part = optional(args, 11, string)?.unwrap_or_default();
        let mut runtime = world.resource_mut::<Runtime>();
        let before = match runtime.object_field(object, "health") {
            Value::Int(health) => health,
            Value::Float(health) => health as i32,
            _ => 0,
        };
        let after = before.saturating_sub(amount);
        runtime.set_object_field(object, "health", Value::Int(after));
        let model = runtime.object_field(object, "model");
        drop(runtime);
        raise(
            world,
            receiver.clone(),
            "damage",
            vec![
                Value::Int(amount),
                attacker.clone(),
                Value::Vector(dir),
                Value::Vector(point),
                Value::string(&means),
                model,
                Value::string(""),
                Value::string(&part),
                Value::Int(flags),
                Value::string(&weapon),
            ],
        );
        if before > 0 && after <= 0 {
            raise(world, receiver.clone(), "death", vec![attacker]);
        }
        Ok(Value::Undefined)
    });
}

pub(crate) fn damage(
    world: &mut World,
    object: u64,
    hit: &super::entity_damage::EntityHit,
    attacker: Value,
    weapon: &str,
    part: &str,
) {
    let args = vec![
        Value::Undefined,
        attacker,
        Value::Int(hit.amount),
        Value::Int(hit.flags),
        Value::string(hit.means),
        Value::string(weapon),
        Value::Vector(hit.point),
        Value::Vector(hit.dir),
        Value::string("none"),
        Value::Int(0),
        Value::Int(0),
        Value::string(part),
    ];
    let now = super::players::now_ms(world);
    let _ = run_now(world, DAMAGE, Value::Object(object), args, now);
}

fn approach_angle(from: f32, to: f32, step: f32) -> f32 {
    let delta = math_iw4::angle_subtract(to, from);
    if delta.abs() <= step {
        to
    } else {
        from + step.copysign(delta)
    }
}

pub(crate) fn advance(world: &mut World) {
    let ids: Vec<u64> = world
        .resource::<Runtime>()
        .vehicles
        .keys()
        .copied()
        .collect();
    let now = crate::level_time_ms(world.resource::<crate::step::StepRequest>().tick);
    for id in ids {
        let (gunner, weapon) = world
            .resource::<Runtime>()
            .vehicles
            .get(&id)
            .map_or((None, None), |heli| (heli.gunner, heli.weapon));
        let control = gunner
            .filter(|client| world.resource::<Runtime>().players.contains_key(client))
            .and_then(|client| gunner_input(world, client));
        let fire_ms = weapon
            .and_then(|weapon| crate::frame::FrameWorld::from_world(world).combat_facts_for(weapon))
            .map_or(100, |facts| facts.fire_time_ms.max(1));
        let mut runtime = world.resource_mut::<Runtime>();
        if !runtime.entities.contains_key(&id) {
            runtime.vehicles.remove(&id);
            continue;
        }
        let origin = vec_field(&mut runtime, id, "origin");
        let angles = vec_field(&mut runtime, id, "angles");
        let look_at = runtime.vehicles[&id]
            .look_at
            .filter(|target| runtime.entities.contains_key(target));
        let look_at = look_at.map(|target| vec_field(&mut runtime, target, "origin"));
        let heli = runtime.vehicles.get_mut(&id).unwrap();
        let before = heli.speed;
        let mut notes = Vec::new();
        let mut next;
        match heli.goal.filter(|_| !(heli.arrived && heli.stop_at_goal)) {
            Some(goal) => {
                let to: [f32; 3] = std::array::from_fn(|i| goal[i] - origin[i]);
                let dist = to.iter().map(|v| v * v).sum::<f32>().sqrt();
                let wanted = if heli.stop_at_goal {
                    heli.max_speed.min((2.0 * heli.decel * dist).sqrt())
                } else {
                    heli.max_speed
                };
                heli.speed = if wanted > heli.speed {
                    (heli.speed + heli.accel * TICK_S).min(wanted)
                } else {
                    (heli.speed - heli.decel * TICK_S).max(wanted)
                };
                if dist > f32::EPSILON {
                    heli.heading = to.map(|v| v / dist);
                }
                let step = (heli.speed * TICK_S).min(dist);
                next = std::array::from_fn(|i| origin[i] + heli.heading[i] * step);
                let left = dist - step;
                if heli.near_goal > 0.0 && !heli.near_notified && left <= heli.near_goal {
                    heli.near_notified = true;
                    notes.push("near_goal");
                }
                if !heli.arrived && left <= ARRIVED.max(heli.speed * TICK_S) {
                    heli.arrived = true;
                    if heli.stop_at_goal {
                        next = goal;
                        heli.speed = 0.0;
                    }
                    notes.push("goal");
                }
            }
            None => {
                heli.speed = (heli.speed - heli.decel * TICK_S).max(0.0);
                next = std::array::from_fn(|i| origin[i] + heli.heading[i] * heli.speed * TICK_S);
            }
        }
        let desired_yaw = heli
            .target_yaw
            .or(heli.arrived.then_some(heli.goal_yaw).flatten())
            .or(look_at
                .map(|at| math_iw4::vect_to_angles([at[0] - next[0], at[1] - next[1], 0.0])[1]))
            .or(
                (heli.speed > 1.0 && heli.heading[..2].iter().any(|v| v.abs() > 0.01))
                    .then(|| math_iw4::vect_to_angles(heli.heading)[1]),
            )
            .unwrap_or(angles[1]);
        let yaw = approach_angle(angles[1], desired_yaw, heli.yaw_speed * TICK_S);
        let turn = math_iw4::angle_subtract(yaw, angles[1]) / TICK_S;
        let accel = (heli.speed - before) / TICK_S;
        let pitch = (accel / MPH)
            .clamp(-heli.max_pitch, heli.max_pitch)
            .clamp(-25.0, 25.0);
        let roll = (turn * 0.25)
            .clamp(-heli.max_roll, heli.max_roll)
            .clamp(-35.0, 35.0);
        match control {
            Some((view, attack)) => {
                heli.turret = Some(TurretAim::Point(std::array::from_fn(|i| {
                    next[i] + view[i] * GUNNER_RANGE
                })));
                heli.on_target = true;
                if attack && now >= heli.next_fire_ms {
                    heli.next_fire_ms = now + fire_ms;
                    notes.push("turret_fire");
                }
            }
            None if heli.gunner.is_some() => {
                heli.gunner = None;
                heli.turret = None;
            }
            None => {}
        }
        if heli.turret.is_some() && !heli.on_target {
            heli.on_target = true;
            notes.push("turret_on_target");
        }
        let speed = heli.speed;
        runtime.set_object_field(id, "origin", Value::Vector(next));
        runtime.set_object_field(id, "angles", Value::Vector([pitch, yaw, roll]));
        runtime.set_object_field(id, "veh_speed", Value::Float(speed / MPH));
        drop(runtime);
        for note in notes {
            raise(world, Value::Object(id), note, Vec::new());
        }
    }
}

pub(crate) fn compass_rows(world: &mut World) -> Vec<crate::CompassVehicle> {
    let mut rows: Vec<_> = world
        .resource::<Runtime>()
        .vehicles
        .iter()
        .filter_map(|(id, heli)| Some((*id, heli.owner?, heli.compass.clone()?)))
        .collect();
    rows.sort_by_key(|(id, _, _)| *id);
    rows.into_iter()
        .filter_map(|(id, owner, (icons, size))| {
            if !world.resource::<Runtime>().live(&id) {
                return None;
            }
            let Value::Vector(origin) = super::players::entity_field(world, id, "origin") else {
                return None;
            };
            let yaw = match super::players::entity_field(world, id, "angles") {
                Value::Vector(v) => v[1],
                _ => 0.0,
            };
            let team = match super::players::entity_field(world, id, "team") {
                Value::String(team) if &*team == "axis" => entity_iw4::TEAM_AXIS,
                Value::String(team) if &*team == "allies" => entity_iw4::TEAM_ALLIES,
                _ => entity_iw4::TEAM_FREE,
            };
            Some(crate::CompassVehicle {
                origin,
                yaw,
                owner,
                team,
                icons,
                size,
            })
        })
        .collect()
}
