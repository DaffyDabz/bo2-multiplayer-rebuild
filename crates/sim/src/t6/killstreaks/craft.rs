//! Scorestreak aircraft and vehicles the scripts spawn: helicopters
//! (`spawnhelicopter`: the care package chopper, the Stealth Chopper, the
//! Escort Drone, the VTOL Warship), script vehicles (`spawnvehicle`) and
//! planes (`spawnplane`: a model the scripts move themselves).
//!
//! BO2's engine flies a helicopter to the goal its scripts set
//! (`setvehgoalpos(point, stop)`) at the speed they set (`setspeed`, miles
//! per hour, with its acceleration), turned to face where it goes - or a
//! goal yaw, a target yaw, an entity it looks at - and tells the scripts
//! `near_goal` (inside `setneargoalnotifydist`) and `goal` (there). The
//! scripts chain goals into flight paths (`_airsupport::followpath`).

use bevy_ecs::prelude::World;
use gsc_t6::{ObjKind, Value, Vm};

use super::super::{Ent, Zm, arg, entnum, text};
use super::{notify_all, streaks};

/// Miles per hour in units a second.
const MPH: f32 = 17.6;

#[derive(Clone, Debug)]
pub(crate) struct Craft {
    /// Wanted speed and acceleration (units/s, units/s²), speed now.
    pub speed: f32,
    pub accel: f32,
    pub cur: f32,
    /// Where it flies and whether it stops there.
    pub goal: Option<[f32; 3]>,
    pub stop: bool,
    /// `near_goal` distance (0: none) and whether it was told.
    pub near: f32,
    pub near_sent: bool,
    pub goal_yaw: Option<f32>,
    pub target_yaw: Option<f32>,
    pub look_at: Option<u32>,
    /// Turn rate (degrees a second).
    pub yaw_speed: f32,
    /// The weapon it fires (its vehicle's turret weapon, or `setvehweapon`),
    /// its gunners' weapons, the player it fires for, and what its turret
    /// aims at.
    pub weapon: String,
    pub gunners: Vec<String>,
    pub owner: Option<u32>,
    /// The player riding or driving it (`usevehicle`).
    pub rider: Option<u32>,
    pub aim: Option<Aim>,
    /// It drives on the ground (the AGR, the RC-XD): its goal is reached
    /// over the map's path nodes (`route`), on the ground.
    pub ground: bool,
    pub route: Vec<[f32; 3]>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Aim {
    Ent(u32),
    Point([f32; 3]),
}

impl Default for Craft {
    fn default() -> Self {
        Self {
            // A helicopter's cruise until its scripts set one.
            speed: 60.0 * MPH,
            accel: 20.0 * MPH,
            cur: 0.0,
            goal: None,
            stop: true,
            near: 0.0,
            near_sent: false,
            goal_yaw: None,
            target_yaw: None,
            look_at: None,
            yaw_speed: 90.0,
            weapon: String::new(),
            gunners: Vec::new(),
            owner: None,
            rider: None,
            aim: None,
            ground: false,
            route: Vec::new(),
        }
    }
}

/// A new script vehicle entity with its model (drawn by the presences).
fn spawn_craft(
    vm: &mut Vm<World>,
    world: &mut World,
    origin: [f32; 3],
    angles: [f32; 3],
    model: &str,
    kind: &str,
    owner: Option<u32>,
    flies: bool,
) -> Value {
    let n = world.resource_mut::<Zm>().alloc_entnum();
    let obj = vm.alloc_object(ObjKind::Entity(n));
    world.resource_mut::<Zm>().ents.insert(
        n,
        Ent {
            obj: Some(obj),
            classname: "script_vehicle".to_owned(),
            origin,
            angles,
            model: model.to_owned(),
            ..Default::default()
        },
    );
    let f = vm.intern("vehicletype");
    let k = vm.string(kind);
    vm.set_raw_field(obj, f, k);
    // Its engine loops (BO2's client script `_helicopter_sounds.csc`).
    super::super::engine_sounds::spawned(world, n, kind);
    if flies {
        // Its guns, from the vehicle's own data in his zones.
        let (weapon, gunners) = streaks(world)
            .vehicle_weapons
            .get(&kind.to_ascii_lowercase())
            .cloned()
            .unwrap_or_default();
        // A ground vehicle drives at its vehicle def's own speed.
        let ground = super::ride::is_ground(kind);
        let drive = streaks(world)
            .vehicle_drive
            .get(&kind.to_ascii_lowercase())
            .copied()
            .unwrap_or_default();
        let mut c = Craft {
            owner,
            weapon,
            gunners,
            ground,
            ..Default::default()
        };
        if ground && drive.max_speed > 0.0 {
            c.speed = drive.max_speed;
            c.accel = drive.accel.max(1.0);
        }
        streaks(world).craft.insert(n, c);
    }
    Value::Object(obj)
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
    // `spawnhelicopter(owner, origin, angles, vehicle type, model)`.
    f!("spawnhelicopter", |vm, world, _, a| {
        let owner =
            entnum(vm, arg(a, 0)).filter(|n| world.resource::<Zm>().players.contains_key(n));
        let origin = super::super::vec3(a, 1)?;
        let angles = arg(a, 2).as_vec3().unwrap_or([0.0; 3]);
        let (kind, model) = (text(vm, a, 3), text(vm, a, 4));
        Ok(spawn_craft(
            vm, world, origin, angles, &model, &kind, owner, true,
        ))
    });
    // `spawnvehicle(model, targetname, vehicle type, origin, angles)`.
    f!("spawnvehicle", |vm, world, _, a| {
        let (model, name, kind) = (text(vm, a, 0), text(vm, a, 1), text(vm, a, 2));
        let origin = super::super::vec3(a, 3)?;
        let angles = arg(a, 4).as_vec3().unwrap_or([0.0; 3]);
        let v = spawn_craft(vm, world, origin, angles, &model, &kind, None, true);
        if let Value::Object(o) = &v {
            let f = vm.intern("targetname");
            let s = vm.string(&name);
            vm.set_raw_field(*o, f, s);
        }
        Ok(v)
    });
    // `spawnplane(owner, classname, origin)`: a model the scripts move.
    f!("spawnplane", |vm, world, _, a| {
        let owner =
            entnum(vm, arg(a, 0)).filter(|n| world.resource::<Zm>().players.contains_key(n));
        let origin = super::super::vec3(a, 2)?;
        let v = spawn_craft(vm, world, origin, [0.0; 3], "", "plane", owner, false);
        if let Some(n) = entnum(vm, &v)
            && let Some(e) = world.resource_mut::<Zm>().ents.get_mut(&n)
        {
            e.classname = "script_model".to_owned();
        }
        Ok(v)
    });
    fn craft_of(vm: &Vm<World>, world: &mut World, s: &Value) -> Option<u32> {
        let n = entnum(vm, s)?;
        streaks(world).craft.contains_key(&n).then_some(n)
    }
    fn with_craft(vm: &Vm<World>, world: &mut World, s: &Value, f: impl FnOnce(&mut Craft)) {
        if let Some(n) = craft_of(vm, world, s)
            && let Some(c) = streaks(world).craft.get_mut(&n)
        {
            f(c);
        }
    }
    m!("setspeed", |vm, world, s, a| {
        let speed = arg(a, 0).as_float().unwrap_or(0.0) * MPH;
        let accel = arg(a, 1).as_float().map(|x| x * MPH);
        with_craft(vm, world, s, |c| {
            c.speed = speed.max(0.0);
            if let Some(x) = accel.filter(|x| *x > 0.0) {
                c.accel = x;
            }
        });
        Ok(Value::Undefined)
    });
    m!("setspeedimmediate", |vm, world, s, a| {
        let speed = arg(a, 0).as_float().unwrap_or(0.0) * MPH;
        with_craft(vm, world, s, |c| {
            c.speed = speed.max(0.0);
            c.cur = c.speed;
        });
        Ok(Value::Undefined)
    });
    m!("getspeed", |vm, world, s, _| {
        let n = craft_of(vm, world, s);
        Ok(Value::Float(
            n.and_then(|n| streaks(world).craft.get(&n).map(|c| c.cur))
                .unwrap_or(0.0),
        ))
    });
    m!("getspeedmph", |vm, world, s, _| {
        let n = craft_of(vm, world, s);
        Ok(Value::Float(
            n.and_then(|n| streaks(world).craft.get(&n).map(|c| c.cur / MPH))
                .unwrap_or(0.0),
        ))
    });
    // `setvehgoalpos(goal, stop)`: true when it can get there (a ground
    // vehicle over the map's path nodes; anything flying, straight).
    m!("setvehgoalpos", |vm, world, s, a| {
        let goal = super::super::vec3(a, 0)?;
        let stop = super::super::flag(a, 1, false);
        let Some(n) = craft_of(vm, world, s) else {
            return Ok(Value::Int(0));
        };
        let ground = streaks(world).craft.get(&n).is_some_and(|c| c.ground);
        let route = if ground {
            let from = world
                .resource::<Zm>()
                .ents
                .get(&n)
                .map_or(goal, |e| e.origin);
            ground_route(world, from, goal)
        } else {
            Some(Vec::new())
        };
        let Some(route) = route else {
            return Ok(Value::Int(0));
        };
        with_craft(vm, world, s, |c| {
            c.goal = Some(goal);
            c.stop = stop;
            c.near_sent = false;
            c.route = route;
        });
        Ok(Value::Int(1))
    });
    m!("clearvehgoalpos", |vm, world, s, _| {
        with_craft(vm, world, s, |c| {
            c.goal = None;
            c.route.clear();
        });
        Ok(Value::Undefined)
    });
    // Whose it is (`setowner`): its shots are that player's.
    m!("setowner", |vm, world, s, a| {
        let owner =
            entnum(vm, arg(a, 0)).filter(|n| world.resource::<Zm>().players.contains_key(n));
        with_craft(vm, world, s, |c| {
            if owner.is_some() {
                c.owner = owner;
            }
        });
        Ok(Value::Undefined)
    });
    m!("setneargoalnotifydist", |vm, world, s, a| {
        let d = arg(a, 0).as_float().unwrap_or(0.0);
        with_craft(vm, world, s, |c| c.near = d);
        Ok(Value::Undefined)
    });
    m!("setgoalyaw", |vm, world, s, a| {
        let y = arg(a, 0).as_float();
        with_craft(vm, world, s, |c| c.goal_yaw = y);
        Ok(Value::Undefined)
    });
    m!("cleargoalyaw", |vm, world, s, _| {
        with_craft(vm, world, s, |c| c.goal_yaw = None);
        Ok(Value::Undefined)
    });
    m!("settargetyaw", |vm, world, s, a| {
        let y = arg(a, 0).as_float();
        with_craft(vm, world, s, |c| c.target_yaw = y);
        Ok(Value::Undefined)
    });
    m!("cleartargetyaw", |vm, world, s, _| {
        with_craft(vm, world, s, |c| c.target_yaw = None);
        Ok(Value::Undefined)
    });
    m!("setyawspeed", |vm, world, s, a| {
        let y = arg(a, 0).as_float().unwrap_or(90.0);
        with_craft(vm, world, s, |c| c.yaw_speed = y.max(1.0));
        Ok(Value::Undefined)
    });
    m!("setlookatent", |vm, world, s, a| {
        let t = entnum(vm, arg(a, 0));
        with_craft(vm, world, s, |c| c.look_at = t);
        Ok(Value::Undefined)
    });
    m!("clearlookatent", |vm, world, s, _| {
        with_craft(vm, world, s, |c| c.look_at = None);
        Ok(Value::Undefined)
    });
    m!("setvehvelocity", |vm, world, s, a| {
        let v = arg(a, 0).as_vec3().unwrap_or([0.0; 3]);
        let speed = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        with_craft(vm, world, s, |c| c.cur = speed);
        Ok(Value::Undefined)
    });
    // Its weapon and turret: what it fires and at what.
    m!("setvehweapon", |vm, world, s, a| {
        let w = text(vm, a, 0);
        with_craft(vm, world, s, |c| c.weapon = w);
        Ok(Value::Undefined)
    });
    // Its turret on a target: on it at once (`turret_on_target`), and
    // whether it sees it from there (`turret_on_vistarget`, `turret_no_vis`:
    // the AGR fires only at what its gun sees).
    fn aim_at(vm: &mut Vm<World>, world: &mut World, s: &Value, aim: Option<Aim>) {
        with_craft(vm, world, s, |c| c.aim = aim);
        if let Some(aim) = aim
            && let Some(o) = s.as_obj()
        {
            super::super::natives_game::notify_later(vm, world, o, "turret_on_target", 50);
            let from = super::super::origin_of(vm, world, s).unwrap_or([0.0; 3]);
            let eye = [from[0], from[1], from[2] + 32.0];
            let to = match aim {
                Aim::Point(p) => Some(p),
                Aim::Ent(t) => {
                    let v = world_obj(world, t);
                    let is_player = world.resource::<Zm>().players.contains_key(&t);
                    super::super::origin_of(vm, world, &v)
                        .map(|p| [p[0], p[1], p[2] + if is_player { 40.0 } else { 16.0 }])
                }
            };
            let seen = to.is_some_and(|to| {
                super::super::frame(world)
                    .trace_static_world(
                        eye,
                        to,
                        [0.0; 3],
                        [0.0; 3],
                        crate::bullet_collision::MASK_SHOT,
                    )
                    .fraction
                    >= 1.0
            });
            let note = if seen {
                "turret_on_vistarget"
            } else {
                "turret_no_vis"
            };
            super::super::natives_game::notify_later(vm, world, o, note, 100);
        }
    }
    m!("setturrettargetent", |vm, world, s, a| {
        let t = entnum(vm, arg(a, 0)).map(Aim::Ent);
        aim_at(vm, world, s, t);
        Ok(Value::Undefined)
    });
    m!("setgunnertargetent", |vm, world, s, a| {
        let t = entnum(vm, arg(a, 0)).map(Aim::Ent);
        aim_at(vm, world, s, t);
        Ok(Value::Undefined)
    });
    m!("setgunnertargetvec", |vm, world, s, a| {
        let p = arg(a, 0).as_vec3().map(Aim::Point);
        aim_at(vm, world, s, p);
        Ok(Value::Undefined)
    });
    m!("cleargunnertarget", |vm, world, s, _| {
        with_craft(vm, world, s, |c| c.aim = None);
        Ok(Value::Undefined)
    });
    m!("setturrettargetvec", |vm, world, s, a| {
        let p = arg(a, 0).as_vec3().map(Aim::Point);
        aim_at(vm, world, s, p);
        Ok(Value::Undefined)
    });
    m!("clearturrettarget", |vm, world, s, _| {
        with_craft(vm, world, s, |c| c.aim = None);
        Ok(Value::Undefined)
    });
    // `fireweapon([tag, target])`: its turret weapon at the target (or what
    // its turret aims at); `firegunnerweapon(index)`: that gunner's.
    m!("fireweapon", |vm, world, s, a| {
        let target = entnum(vm, arg(a, 1)).map(Aim::Ent);
        fire(vm, world, s, None, target);
        Ok(Value::Undefined)
    });
    m!("firegunnerweapon", |vm, world, s, a| {
        let gunner = arg(a, 0).as_int().unwrap_or(0).max(0) as usize;
        fire(vm, world, s, Some(gunner), None);
        Ok(Value::Undefined)
    });
    // How it sways, tilts and sounds, its model for the other team, its
    // damage looks, its avoidance of others: the look of its flight.
    for name in [
        "sethoverparams",
        "setmaxpitchroll",
        "setturningability",
        "setjitterparams",
        "setrotorspeed",
        "setvehicleavoidance",
        "setdamagestage",
        "setdefaultdroppitch",
        "setphysacceleration",
        "setbrake",
        "setvehicleteam",
        "setenemymodel",
        "makevehicleunusable",
        "makevehicleusable",
        "setangularvelocity",
        "disablegunnerfiring",
    ] {
        vm.bind(name, true, |_, _, _, _| Ok(Value::Undefined));
    }
    m!("getangularvelocity", |_, _, _, _| Ok(Value::Vec3([0.0; 3])));
    // Inside the playable area's height lock (`isinsideheightlock`,
    // `ismissileinsideheightlock`: a Dragonfire or a missile out of it
    // counts down and blows up): over the minimap's frame.
    fn inside_lock(vm: &mut Vm<World>, world: &mut World, s: &Value) -> Value {
        let Some(o) = super::super::origin_of(vm, world, s) else {
            return Value::Int(1);
        };
        let Some(map) = streaks(world).minimap else {
            return Value::Int(1);
        };
        let d = [o[0] - map.upper_left[0], o[1] - map.upper_left[1]];
        let x = d[0] * map.north[1] - d[1] * map.north[0];
        let y = -d[0] * map.north[0] - d[1] * map.north[1];
        Value::bool(
            (0.0..=map.world_size[0]).contains(&x) && (0.0..=map.world_size[1]).contains(&y),
        )
    }
    m!("isinsideheightlock", |vm, world, s, _| Ok(inside_lock(
        vm, world, s
    )));
    m!("ismissileinsideheightlock", |vm, world, s, _| Ok(
        inside_lock(vm, world, s)
    ));
}

/// One shot of a craft's weapon (its turret's, or a gunner's) at its aim:
/// a gun's bullet hits when the line is clear (its damage less with
/// range), anything else leaves as the engine's own projectile. The shot
/// is its player's.
fn fire(vm: &mut Vm<World>, world: &mut World, s: &Value, gunner: Option<usize>, aim: Option<Aim>) {
    if let Some(n) = entnum(vm, s) {
        let aim = aim.or_else(|| streaks(world).craft.get(&n).and_then(|c| c.aim));
        if let Some(aim) = aim {
            fire_n(vm, world, n, gunner, aim);
        }
    }
}

/// A shot from a player-driven craft (the Dragonfire, the VTOL's guns):
/// the time until that gun fires again (ms), None when it has no such gun.
pub(super) fn fire_from(world: &mut World, n: u32, gunner: Option<usize>, aim: Aim) -> Option<i32> {
    super::super::with_vm(world, |vm, world| fire_n(vm, world, n, gunner, aim)).flatten()
}

fn fire_n(
    vm: &mut Vm<World>,
    world: &mut World,
    n: u32,
    gunner: Option<usize>,
    aim: Aim,
) -> Option<i32> {
    let c = streaks(world).craft.get(&n).cloned()?;
    let name = match gunner {
        Some(i) => c.gunners.get(i).cloned().unwrap_or_default(),
        None => c.weapon.clone(),
    };
    let owner = c.owner.or(c.rider)?;
    let w = super::super::weapon(world, &name)
        .ok()
        .filter(|w| *w != 0)?;
    let start = muzzle(world, n, gunner);
    let (to, victim) = match aim {
        Aim::Point(p) => (p, None),
        Aim::Ent(t) => {
            let target = world_obj(world, t);
            let o = super::super::origin_of(vm, world, &target)?;
            let is_player = world.resource::<Zm>().players.contains_key(&t);
            (
                if is_player {
                    [o[0], o[1], o[2] + 40.0]
                } else {
                    o
                },
                is_player.then_some(t),
            )
        }
    };
    let facts = super::super::frame(world).combat_facts_for(w);
    let interval = facts.map_or(100, |f| f.fire_time_ms.max(1));
    // Its gun's flash and sound at the muzzle, for everyone.
    fire_event(world, n, w, start, to);
    if facts.is_some_and(|f| f.weap_type == weapon_iw4::WEAPTYPE_BULLET) {
        // Aimed at a point (a player at the gun): the first enemy within a
        // body's width of the line takes it.
        let victim = victim.or_else(|| ray_victim(vm, world, owner, start, to));
        let Some(t) = victim else {
            bullet_fx(world, n, w, start, to, None);
            return Some(interval);
        };
        let Some(ps) = super::super::frame(world)
            .player(crate::world::ClientId(t))
            .copied()
        else {
            bullet_fx(world, n, w, start, to, None);
            return Some(interval);
        };
        let mid = [ps.origin[0], ps.origin[1], ps.origin[2] + 40.0];
        let clear = super::super::frame(world)
            .trace_static_world(
                start,
                mid,
                [0.0; 3],
                [0.0; 3],
                crate::bullet_collision::MASK_SHOT,
            )
            .fraction
            >= 1.0;
        if !clear || ps.health <= 0 {
            bullet_fx(world, n, w, start, mid, None);
            return Some(interval);
        }
        bullet_fx(world, n, w, start, mid, Some(t));
        let d = ((mid[0] - start[0]).powi(2)
            + (mid[1] - start[1]).powi(2)
            + (mid[2] - start[2]).powi(2))
        .sqrt();
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
        if amount > 0 {
            let attacker = world_obj(world, owner);
            let inflictor = world_obj(world, n);
            let dir = gsc_t6::math::normalize(gsc_t6::math::sub(mid, start));
            super::super::natives_ai::player_damage(
                vm,
                world,
                t,
                inflictor,
                attacker,
                amount,
                0,
                "MOD_RIFLE_BULLET",
                &name,
                mid,
                dir,
                "torso_upper",
            );
        }
        return Some(interval);
    }
    let t = super::super::tick(world);
    let _ = crate::missile::magic_bullet(
        &mut super::super::frame(world),
        t,
        crate::world::ClientId(owner),
        w,
        start,
        to,
    );
    Some(interval)
}

/// Where a craft's gun fires from: its model's flash tag (the turret's
/// `tag_flash`, a gunner's `tag_flash_gunner<N>`) turned with it, else
/// under its middle.
pub(super) fn muzzle(world: &mut World, n: u32, gunner: Option<usize>) -> [f32; 3] {
    let Some((origin, angles, model)) = world
        .resource::<Zm>()
        .ents
        .get(&n)
        .map(|e| (e.origin, e.angles, e.model.clone()))
    else {
        return [0.0; 3];
    };
    let tag = gunner.map_or("tag_flash".to_owned(), |i| {
        format!("tag_flash_gunner{}", i + 1)
    });
    let local = super::ride::tag_in_model(world, &model, &tag)
        .or_else(|| super::ride::tag_in_model(world, &model, "tag_flash"));
    let Some(l) = local else {
        return [origin[0], origin[1], origin[2] - 48.0];
    };
    let (f, r, u) = gsc_t6::math::angle_vectors(angles);
    std::array::from_fn(|i| origin[i] + f[i] * l[0] - r[i] * l[1] + u[i] * l[2])
}

/// The gun firing (`EV_FIRE_WEAPON` on the craft's own entity): its world
/// flash and fire sound play at the muzzle, as a player's do at his gun.
pub(super) fn fire_event(world: &mut World, n: u32, w: u32, start: [f32; 3], to: [f32; 3]) {
    let Some(number) = world
        .resource::<Zm>()
        .presences
        .by_ent
        .get(&n)
        .map(|p| p.number)
    else {
        return;
    };
    let t = super::super::tick(world);
    super::super::frame(world).push_entity_event(
        t,
        crate::EventAudience::All,
        entity_iw4::predicted_weapon_fire_event(0, false),
        crate::EntityEventPayload {
            number,
            weapon: w,
            origin: start,
            direction: math_iw4::vect_to_angles(gsc_t6::math::sub(to, start)),
            ..Default::default()
        },
    );
}

/// What a craft's bullet looks like: its tracer from the muzzle and its
/// impact where it lands (on the first thing in the way, or the player it
/// hit), the weapon's own effects for that surface.
pub(super) fn bullet_fx(
    world: &mut World,
    n: u32,
    w: u32,
    start: [f32; 3],
    to: [f32; 3],
    hit: Option<u32>,
) {
    let Some(number) = world
        .resource::<Zm>()
        .presences
        .by_ent
        .get(&n)
        .map(|p| p.number)
    else {
        return;
    };
    let dir = gsc_t6::math::normalize(gsc_t6::math::sub(to, start));
    let (end, normal, surface_flags) = if hit.is_some() {
        (
            to,
            [-dir[0], -dir[1], -dir[2]],
            weapon_iw4::SURF_TYPE_FLESH << 20,
        )
    } else {
        // On past its aim point to whatever stops it.
        let far: [f32; 3] = std::array::from_fn(|i| start[i] + dir[i] * 8192.0);
        let tr = super::super::frame(world).trace_static_world(
            start,
            far,
            [0.0; 3],
            [0.0; 3],
            crate::bullet_collision::MASK_SHOT,
        );
        if tr.startsolid != 0 {
            return;
        }
        let f = tr.fraction.clamp(0.0, 1.0);
        let end: [f32; 3] = std::array::from_fn(|i| start[i] + (far[i] - start[i]) * f);
        if f >= 1.0 {
            (end, [0.0; 3], 0)
        } else {
            (end, tr.normal, tr.surface_flags)
        }
    };
    let correlation = {
        let mut st = streaks(world);
        st.shots = st.shots.wrapping_add(1);
        st.shots
    };
    super::super::frame(world).push_pellet_fx(crate::PelletFxRecord {
        attacker: number,
        weapon: w,
        correlation,
        pellet: 0,
        hand: 0,
        start,
        end,
        normal,
        surf_type: trace_iw4::surface_type_from_flags(surface_flags),
        surface_flags,
        flesh_flags: 0,
    });
}

/// The nearest living enemy of `owner` within 24 units of the line.
fn ray_victim(
    vm: &mut Vm<World>,
    world: &mut World,
    owner: u32,
    start: [f32; 3],
    end: [f32; 3],
) -> Option<u32> {
    let team = |vm: &mut Vm<World>, world: &World, n: u32| {
        let o = world.resource::<Zm>().players.get(&n)?.obj;
        let f = vm.intern("team");
        Some(vm.to_text(&vm.raw_field(o, f)))
    };
    let mine = team(vm, world, owner);
    let dir = gsc_t6::math::sub(end, start);
    let len2 = gsc_t6::math::dot(dir, dir).max(1.0);
    let players: Vec<u32> = world.resource::<Zm>().players.keys().copied().collect();
    let mut best: Option<(f32, u32)> = None;
    for p in players {
        if p == owner {
            continue;
        }
        let theirs = team(vm, world, p);
        if mine
            .as_deref()
            .is_some_and(|m| m != "free" && Some(m) == theirs.as_deref())
        {
            continue;
        }
        let Some(ps) = super::super::frame(world)
            .player(crate::world::ClientId(p))
            .copied()
        else {
            continue;
        };
        if ps.health <= 0 {
            continue;
        }
        let mid = [ps.origin[0], ps.origin[1], ps.origin[2] + 40.0];
        let rel = gsc_t6::math::sub(mid, start);
        let k = (gsc_t6::math::dot(rel, dir) / len2).clamp(0.0, 1.0);
        let near: [f32; 3] = std::array::from_fn(|i| start[i] + dir[i] * k);
        let off = gsc_t6::math::sub(mid, near);
        if gsc_t6::math::dot(off, off) <= 24.0 * 24.0 && best.is_none_or(|b| k < b.0) {
            best = Some((k, p));
        }
    }
    best.map(|b| b.1)
}

pub(super) fn world_obj(world: &World, n: u32) -> Value {
    let zm = world.resource::<Zm>();
    zm.players
        .get(&n)
        .map(|p| Value::Object(p.obj))
        .or_else(|| zm.ents.get(&n).and_then(|e| e.obj).map(Value::Object))
        .unwrap_or(Value::Undefined)
}

fn angle_norm(a: f32) -> f32 {
    let mut a = a % 360.0;
    if a > 180.0 {
        a -= 360.0;
    }
    if a < -180.0 {
        a += 360.0;
    }
    a
}

/// Each tick: every craft toward its goal, turned toward what it faces.
pub(super) fn advance(world: &mut World) {
    let dt = crate::MATCH_TICK_MS as f32 / 1000.0;
    let ids: Vec<u32> = streaks(world).craft.keys().copied().collect();
    let mut events = Vec::new();
    for n in ids {
        let Some((pos, ang)) = world
            .resource::<Zm>()
            .ents
            .get(&n)
            .map(|e| (e.origin, e.angles))
        else {
            streaks(world).craft.remove(&n);
            continue;
        };
        let Some(mut c) = streaks(world).craft.get(&n).cloned() else {
            continue;
        };
        let mut pos = pos;
        let mut heading = None;
        if c.ground && c.goal.is_some() && c.rider.is_none() {
            // On the ground along its route, at its own speed.
            let next = c.route.first().copied().or(c.goal).unwrap_or(pos);
            let d = [next[0] - pos[0], next[1] - pos[1]];
            let dist = (d[0] * d[0] + d[1] * d[1]).sqrt();
            c.cur = (c.cur + c.accel * dt).min(c.speed);
            let step = c.cur * dt;
            if dist <= step.max(4.0) {
                pos = [next[0], next[1], pos[2]];
                if c.route.is_empty() {
                    c.goal = None;
                    c.cur = 0.0;
                    events.push((n, "reached_end_node", Vec::new()));
                    events.push((n, "goal", Vec::new()));
                } else {
                    c.route.remove(0);
                }
            } else {
                pos = [
                    pos[0] + d[0] / dist * step,
                    pos[1] + d[1] / dist * step,
                    pos[2],
                ];
                heading = Some(d[1].atan2(d[0]).to_degrees());
            }
            // Its wheels on the ground under it.
            let up = [pos[0], pos[1], pos[2] + 24.0];
            let down = [pos[0], pos[1], pos[2] - 96.0];
            let t = super::super::frame(world).trace_static_world(
                up,
                down,
                [0.0; 3],
                [0.0; 3],
                crate::bullet_collision::MASK_SHOT,
            );
            if t.fraction < 1.0 {
                pos[2] = up[2] + (down[2] - up[2]) * t.fraction;
            }
        } else if let Some(goal) = c.goal {
            let d = [goal[0] - pos[0], goal[1] - pos[1], goal[2] - pos[2]];
            let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            // Speed up toward the wanted speed; brake to stop at the goal.
            let mut want = c.speed;
            if c.stop {
                want = want.min((2.0 * c.accel * dist).sqrt());
            }
            if c.cur < want {
                c.cur = (c.cur + c.accel * dt).min(want);
            } else {
                c.cur = (c.cur - c.accel * dt).max(want);
            }
            let step = (c.cur * dt).max(if c.stop { 2.0 } else { 0.0 });
            if c.near > 0.0 && !c.near_sent && dist <= c.near {
                c.near_sent = true;
                events.push((n, "near_goal", Vec::new()));
            }
            if dist <= step.max(1.0) {
                pos = goal;
                c.goal = None;
                if c.stop {
                    c.cur = 0.0;
                }
                events.push((n, "goal", Vec::new()));
            } else {
                pos = std::array::from_fn(|i| pos[i] + d[i] / dist * step);
                if d[0].abs() + d[1].abs() > 1.0 {
                    heading = Some(d[1].atan2(d[0]).to_degrees());
                }
            }
        }
        // What it faces: an entity it looks at, a target yaw, its goal yaw
        // once there, else where it flies.
        let look = c.look_at.and_then(|t| {
            let o = if world.resource::<Zm>().players.contains_key(&t) {
                super::super::frame(world)
                    .player(crate::world::ClientId(t))
                    .map(|ps| ps.origin)
            } else {
                world.resource::<Zm>().ents.get(&t).map(|e| e.origin)
            }?;
            Some((o[1] - pos[1]).atan2(o[0] - pos[0]).to_degrees())
        });
        let want_yaw = look
            .or(c.target_yaw)
            .or(if c.goal.is_none() { c.goal_yaw } else { None })
            .or(heading);
        let mut yaw = ang[1];
        if let Some(w) = want_yaw {
            let delta = angle_norm(w - yaw);
            let turn = c.yaw_speed * dt;
            yaw += delta.clamp(-turn, turn);
        }
        if let Some(e) = world.resource_mut::<Zm>().ents.get_mut(&n) {
            e.origin = pos;
            e.angles = [ang[0], angle_norm(yaw), ang[2]];
        }
        streaks(world).craft.insert(n, c);
    }
    notify_all(world, events);
}

/// A ground vehicle's way to `goal`: the map's path nodes from the one
/// nearest it to the one nearest the goal, then the goal (straight when
/// close and clear). None when no route joins.
fn ground_route(world: &mut World, from: [f32; 3], goal: [f32; 3]) -> Option<Vec<[f32; 3]>> {
    let d = [goal[0] - from[0], goal[1] - from[1]];
    if (d[0] * d[0] + d[1] * d[1]).sqrt() < 256.0
        && super::super::frame(world)
            .trace_static_world(
                [from[0], from[1], from[2] + 24.0],
                [goal[0], goal[1], goal[2] + 24.0],
                [0.0; 3],
                [0.0; 3],
                crate::bullet_collision::MASK_SHOT,
            )
            .fraction
            >= 1.0
    {
        return Some(vec![goal]);
    }
    let zm = world.resource::<Zm>();
    let nav = &zm.nav;
    let (a, b) = (nav.nearest(from)?, nav.nearest(goal)?);
    let nodes = nav.path(a, b)?;
    let mut route: Vec<[f32; 3]> = nodes
        .iter()
        .map(|&i| nav.nodes[i as usize].origin)
        .collect();
    route.push(goal);
    Some(route)
}
