//! Actor builtins: spawning, goals and paths, orient/anim modes, the
//! animstatedef queries, scripted animations, melee, damage and death; and
//! the player damage the zombies deal (CodeCallback_PlayerDamage ->
//! finishplayerdamage).

use bevy_ecs::prelude::World;
use gsc_t6::{ObjKind, Value, Vm};

use super::actors::{self, ANIMSCRIPTS};
use super::{Zm, arg, entnum, frame, int, is_player, list, num, origin_of, text, vec3};
use crate::world::ClientId;

type R = Result<Value, String>;

fn actor_n(vm: &Vm<World>, world: &World, s: &Value) -> Result<u32, String> {
    let n = entnum(vm, s).ok_or("not an entity")?;
    if world.resource::<Zm>().actors.contains_key(&n) {
        Ok(n)
    } else {
        Err("not an actor".into())
    }
}

fn is_actor(vm: &Vm<World>, world: &World, v: &Value) -> bool {
    entnum(vm, v).is_some_and(|n| world.resource::<Zm>().actors.contains_key(&n))
}

pub(super) fn actor_alive(vm: &Vm<World>, world: &World, v: &Value) -> Option<bool> {
    let n = entnum(vm, v)?;
    world.resource::<Zm>().actors.get(&n).map(|a| a.alive)
}

/// `CodeCallback_PlayerDamage` on player `victim` (a zombie's swing, a
/// fall, an explosion...).
#[allow(clippy::too_many_arguments)]
pub(crate) fn player_damage(
    vm: &mut Vm<World>,
    world: &mut World,
    victim: u32,
    inflictor: Value,
    attacker: Value,
    amount: i32,
    flags: i32,
    means: &str,
    weapon: &str,
    point: [f32; 3],
    dir: [f32; 3],
    hitloc: &str,
) {
    // IW4L_T6_GOD=1 (tests): players take no damage.
    static GOD: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *GOD.get_or_init(|| std::env::var("IW4L_T6_GOD").is_ok()) || super::autoplay::rounds_test() {
        return;
    }
    let Some(obj) = world.resource::<Zm>().players.get(&victim).map(|p| p.obj) else {
        return;
    };
    let means = vm.string(means);
    let weapon = vm.string(weapon);
    let hitloc = vm.string(hitloc);
    let args = vec![
        inflictor,
        attacker,
        Value::Int(amount),
        Value::Int(flags),
        means,
        weapon,
        Value::Vec3(point),
        Value::Vec3(dir),
        hitloc,
        Value::Int(0),
        Value::Int(0),
    ];
    let cb = super::mp::callbacks(world);
    vm.spawn_named(
        world,
        &cb,
        "codecallback_playerdamage",
        Value::Object(obj),
        args,
    );
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
    m!("spawnactor", |vm, world, s, _| {
        let Some(sp) = s.as_obj() else {
            return Ok(Value::Undefined);
        };
        Ok(actors::spawn_actor(vm, world, sp).map_or(Value::Undefined, Value::Object))
    });
    f!("getfreeactorcount", |_, world, _, _| {
        let alive = world
            .resource::<Zm>()
            .actors
            .values()
            .filter(|a| a.alive)
            .count() as i32;
        Ok(Value::Int((32 - alive).max(0)))
    });
    for name in ["getaiarray", "getaispeciesarray"] {
        vm.bind(name, false, |vm, world, _, a| {
            let team = match arg(a, 0) {
                Value::Undefined => None,
                v => Some(vm.to_text(v)),
            };
            let team_s = vm.intern("team");
            let actors: Vec<gsc_t6::ObjRef> = world
                .resource::<Zm>()
                .actors
                .values()
                .filter(|x| x.alive)
                .filter_map(|x| x.obj)
                .collect();
            let out = actors
                .into_iter()
                .filter(|o| match &team {
                    None => true,
                    Some(t) if t == "all" => true,
                    Some(t) => vm.to_text(&vm.raw_field(*o, team_s)) == *t,
                })
                .map(Value::Object)
                .collect();
            Ok(list(out))
        });
    }
    f!("getcorpsearray", |_, world, _, _| {
        let out = world
            .resource::<Zm>()
            .actors
            .values()
            .filter(|x| !x.alive)
            .filter_map(|x| x.obj.map(Value::Object))
            .collect();
        Ok(list(out))
    });
    f!("isai", |vm, world, _, a| Ok(Value::bool(
        actor_alive(vm, world, arg(a, 0)).is_some()
    )));
    f!("isalive", |vm, world, _, a| {
        let v = arg(a, 0);
        if !vm.is_defined(v) {
            return Ok(Value::Int(0));
        }
        if let Some(alive) = actor_alive(vm, world, v) {
            return Ok(Value::bool(alive));
        }
        if let Some(n) = entnum(vm, v)
            && let Some(p) = world.resource::<Zm>().players.get(&n).cloned()
        {
            let h = frame(world).player(ClientId(n)).map_or(0, |ps| ps.health);
            return Ok(Value::bool(p.sessionstate == "playing" && h > 0));
        }
        Ok(Value::bool(entnum(vm, v).is_some()))
    });
    f!("issentient", |vm, world, _, a| {
        let v = arg(a, 0);
        Ok(Value::bool(
            is_player(vm, world, v) || actor_alive(vm, world, v) == Some(true),
        ))
    });
    m!("setgoalpos", |vm, world, s, a| {
        let n = actor_n(vm, world, s)?;
        let goal = vec3(a, 0)?;
        set_goal(vm, world, n, goal);
        Ok(Value::Undefined)
    });
    m!("setgoalnode", |vm, world, s, a| {
        let n = actor_n(vm, world, s)?;
        let Some(goal) = arg(a, 0).as_obj().map(|o| {
            let f = vm.intern("origin");
            vm.raw_field(o, f)
        }) else {
            return Ok(Value::Undefined);
        };
        if let Some(goal) = goal.as_vec3() {
            set_goal(vm, world, n, goal);
        }
        Ok(Value::Undefined)
    });
    m!("setgoalentity", |vm, world, s, a| {
        let n = actor_n(vm, world, s)?;
        let target = arg(a, 0).clone();
        if let Some(goal) = origin_of(vm, world, &target) {
            set_goal(vm, world, n, goal);
        }
        Ok(Value::Undefined)
    });
    m!("calcpathlength", |vm, world, s, a| {
        let from = origin_of(vm, world, s).unwrap_or([0.0; 3]);
        let to = vec3(a, 0)?;
        Ok(actors::path_length(world, from, to).map_or(Value::Float(-1.0), Value::Float))
    });
    m!("isingoal", |vm, world, s, a| {
        let n = actor_n(vm, world, s)?;
        let p = vec3(a, 0)?;
        let obj = s.as_obj().unwrap();
        let gr = vm.intern("goalradius");
        let radius = vm.raw_field(obj, gr).as_float().unwrap_or(32.0);
        let goal = world.resource::<Zm>().actors.get(&n).and_then(|x| x.goal);
        Ok(Value::bool(goal.is_none_or(|g| {
            let d = [g[0] - p[0], g[1] - p[1]];
            (d[0] * d[0] + d[1] * d[1]).sqrt() <= radius.max(64.0)
        })))
    });
    for name in [
        "maymovetopoint",
        "maymovefrompointtopoint",
        "cansee",
        "canshoot",
    ] {
        vm.bind(name, true, |_, _, _, _| Ok(Value::Int(1)));
    }
    m!("forceteleport", |vm, world, s, a| {
        let Some(n) = entnum(vm, s) else {
            return Ok(Value::Undefined);
        };
        let p = vec3(a, 0)?;
        let ang = arg(a, 1).as_vec3();
        let mut zm = world.resource_mut::<Zm>();
        if let Some(e) = zm.ents.get_mut(&n) {
            e.origin = p;
            if let Some(ang) = ang {
                e.angles = ang;
            }
        }
        Ok(Value::Undefined)
    });
    // bo2zm M3 fix list 2: tilt to the slope (actors::pitch_to_ground).
    m!("setpitchorient", |vm, world, s, _| {
        let n = actor_n(vm, world, s)?;
        if let Some(x) = world.resource_mut::<Zm>().actors.get_mut(&n) {
            x.pitch_orient = true;
        }
        Ok(Value::Undefined)
    });
    m!("clearpitchorient", |vm, world, s, _| {
        let n = actor_n(vm, world, s)?;
        if let Some(x) = world.resource_mut::<Zm>().actors.get_mut(&n) {
            x.pitch_orient = false;
        }
        Ok(Value::Undefined)
    });
    m!("orientmode", |vm, world, s, a| {
        let n = actor_n(vm, world, s)?;
        let mode = text(vm, a, 0);
        let yaw = arg(a, 1).as_float();
        let point = arg(a, 1).as_vec3();
        let mut zm = world.resource_mut::<Zm>();
        if let Some(x) = zm.actors.get_mut(&n) {
            x.orientmode = mode;
            if let Some(y) = yaw {
                x.orient_yaw = y;
            }
            if let Some(p) = point {
                x.orient_point = p;
            }
        }
        Ok(Value::Undefined)
    });
    m!("animmode", |vm, world, s, a| {
        let n = actor_n(vm, world, s)?;
        let mode = text(vm, a, 0);
        if let Some(x) = world.resource_mut::<Zm>().actors.get_mut(&n) {
            x.animmode = mode;
        }
        Ok(Value::Undefined)
    });
    m!("setphysparams", |vm, world, s, a| {
        let n = actor_n(vm, world, s)?;
        let r = num(a, 0).unwrap_or(15.0);
        let h = num(a, 2).unwrap_or(72.0);
        if let Some(x) = world.resource_mut::<Zm>().actors.get_mut(&n) {
            x.radius = r;
            x.height = h;
        }
        Ok(Value::Undefined)
    });
    m!("setanimstatefromasd", |vm, world, s, a| {
        let n = actor_n(vm, world, s)?;
        let state = text(vm, a, 0);
        let sub = arg(a, 1).clone();
        actors::set_state(vm, world, n, &state, &sub);
        Ok(Value::Undefined)
    });
    m!("getanimstatefromasd", |vm, world, s, _| {
        let n = actor_n(vm, world, s)?;
        let st = world
            .resource::<Zm>()
            .actors
            .get(&n)
            .and_then(|x| x.playing.as_ref().map(|p| p.state.clone()));
        Ok(st.map_or(Value::Undefined, |st| vm.string(&st)))
    });
    m!("getanimsubstatefromasd", |vm, world, s, a| {
        let n = actor_n(vm, world, s)?;
        // (state, alias) -> that alias's index; (state) -> a pick; () ->
        // the current substate.
        if let Value::Str(_) = arg(a, 0) {
            let state = text(vm, a, 0);
            let zm = world.resource::<Zm>();
            let Some(x) = zm.actors.get(&n) else {
                return Ok(Value::Int(0));
            };
            let Some(def) = zm.asds.get(&x.asd) else {
                return Ok(Value::Int(0));
            };
            let Some(st) = def.state(&state, x.missing_legs) else {
                return Ok(Value::Int(0));
            };
            if let Value::Str(_) = arg(a, 1) {
                let alias = vm.to_text(arg(a, 1)).to_ascii_lowercase();
                return Ok(Value::Int(
                    st.substates
                        .iter()
                        .position(|(n, _)| *n == alias)
                        .unwrap_or(0) as i32,
                ));
            }
            let count = st.substates.len().max(1) as u32;
            return Ok(Value::Int((vm.rand_u32() % count) as i32));
        }
        let cur = world
            .resource::<Zm>()
            .actors
            .get(&n)
            .and_then(|x| x.playing.as_ref().map(|p| p.substate));
        Ok(Value::Int(cur.unwrap_or(0) as i32))
    });
    m!("hasanimstatefromasd", |vm, world, s, a| {
        let n = actor_n(vm, world, s)?;
        let state = text(vm, a, 0);
        let zm = world.resource::<Zm>();
        let has = zm
            .actors
            .get(&n)
            .and_then(|x| zm.asds.get(&x.asd))
            .is_some_and(|d| d.state(&state, false).is_some());
        Ok(Value::bool(has))
    });
    m!("getanimlengthfromasd", |vm, world, s, _| {
        let n = actor_n(vm, world, s)?;
        let ms = world
            .resource::<Zm>()
            .actors
            .get(&n)
            .and_then(|x| x.playing.as_ref().map(|p| p.length_ms))
            .unwrap_or(1000);
        Ok(Value::Float(ms as f32 / 1000.0))
    });
    m!("getanimhasnotetrackfromasd", |vm, world, s, a| {
        let n = actor_n(vm, world, s)?;
        let note = text(vm, a, 0);
        let zm = world.resource::<Zm>();
        let has = zm
            .actors
            .get(&n)
            .and_then(|x| x.playing.as_ref())
            .and_then(|p| zm.anims.get(&p.anim))
            .is_some_and(|an| {
                an.notifies
                    .iter()
                    .any(|(nn, _)| nn.eq_ignore_ascii_case(&note))
            });
        Ok(Value::bool(has))
    });
    m!("animscripted", |vm, world, s, a| {
        let n = actor_n(vm, world, s)?;
        let origin = vec3(a, 0)?;
        let angles = vec3(a, 1).unwrap_or([0.0; 3]);
        if let Some(x) = world.resource_mut::<Zm>().actors.get_mut(&n) {
            x.scripted = Some(actors::Scripted { origin, angles });
            x.path.clear();
        }
        // The engine hands the arguments to zm_scripted::init, then runs
        // zm_scripted::main (which calls startscriptedanim).
        let obj = s.as_obj().unwrap();
        vm.spawn_named(
            world,
            &format!("{ANIMSCRIPTS}/zm_scripted"),
            "init",
            Value::Object(obj),
            a.to_vec(),
        );
        actors::run_script(vm, world, n, "scripted");
        Ok(Value::Undefined)
    });
    m!("startscriptedanim", |vm, world, s, a| {
        let n = actor_n(vm, world, s)?;
        let origin = vec3(a, 0).unwrap_or([0.0; 3]);
        let angles = vec3(a, 1).unwrap_or([0.0; 3]);
        let state = text(vm, a, 2);
        let sub = arg(a, 3).clone();
        let mode = text(vm, a, 4);
        {
            let mut zm = world.resource_mut::<Zm>();
            if let Some(e) = zm.ents.get_mut(&n) {
                e.origin = origin;
                e.angles = [0.0, angles[1], 0.0];
            }
            if let Some(x) = zm.actors.get_mut(&n) {
                x.scripted = Some(actors::Scripted { origin, angles });
                if !mode.is_empty() {
                    x.animmode = mode;
                }
            }
        }
        actors::set_state(vm, world, n, &state, &sub);
        Ok(Value::Undefined)
    });
    m!("stopanimscripted", |vm, world, s, _| {
        let n = actor_n(vm, world, s)?;
        let obj = s.as_obj().unwrap();
        if let Some(x) = world.resource_mut::<Zm>().actors.get_mut(&n) {
            x.scripted = None;
            x.script.clear();
        }
        vm.notify_str(world, obj, "end_sequence", &[]);
        Ok(Value::Undefined)
    });
    m!("isinscriptedstate", |vm, world, s, _| {
        let n = actor_n(vm, world, s)?;
        Ok(Value::bool(
            world
                .resource::<Zm>()
                .actors
                .get(&n)
                .is_some_and(|x| x.scripted.is_some()),
        ))
    });
    m!("getnegotiationstartnode", |vm, world, s, _| {
        let n = actor_n(vm, world, s)?;
        let zm = world.resource::<Zm>();
        let node = zm.actors.get(&n).and_then(|x| {
            let i = x.path_i;
            x.path_nodes.get(i).copied()
        });
        Ok(node
            .and_then(|i| zm.node_objs.get(i as usize).copied())
            .map_or(Value::Undefined, Value::Object))
    });
    m!("traversemode", |_, _, _, _| Ok(Value::Undefined));
    m!("setcharacterindex", |vm, world, s, a| {
        let n = actor_n(vm, world, s)?;
        let i = int(a, 0).unwrap_or(0);
        if let Some(x) = world.resource_mut::<Zm>().actors.get_mut(&n) {
            x.character_index = i;
        }
        Ok(Value::Undefined)
    });
    m!("getcharacterindex", |vm, world, s, _| {
        let n = actor_n(vm, world, s)?;
        Ok(Value::Int(
            world
                .resource::<Zm>()
                .actors
                .get(&n)
                .map_or(0, |x| x.character_index),
        ))
    });
    m!("getattachsize", |vm, world, s, _| {
        let Some(n) = entnum(vm, s) else {
            return Ok(Value::Int(0));
        };
        Ok(Value::Int(
            world
                .resource::<Zm>()
                .ents
                .get(&n)
                .map_or(0, |e| e.attached.len() as i32),
        ))
    });
    m!("getattachmodelname", |vm, world, s, a| {
        let Some(n) = entnum(vm, s) else {
            return Ok(Value::Undefined);
        };
        let i = int(a, 0).unwrap_or(0).max(0) as usize;
        let name = world
            .resource::<Zm>()
            .ents
            .get(&n)
            .and_then(|e| e.attached.get(i).map(|x| x.0.clone()));
        Ok(name.map_or(Value::Undefined, |m| vm.string(&m)))
    });
    for name in [
        "setaimanimweights",
        "setflashbanged",
        "enableaimassist",
        "disableaimassist",
        "clearentityowner",
        "allowedstances",
        "allowpitchangle",
        "setengagementmindist",
        "setengagementmaxdist",
        "pushplayer",
        "startragdoll",
        "launchragdoll",
        "clearenemy",
        "cleargoalvolume",
        "setgoalvolume",
        "setfreecameralockonallowed",
        "setcanbeseen",
        "setavoidancemask",
        "pushactors",
        "setmaxhealth",
    ] {
        vm.bind(name, true, |_, _, _, _| Ok(Value::Undefined));
    }
    m!("melee", |vm, world, s, _| {
        let n = actor_n(vm, world, s)?;
        let (enemy, me) = {
            let zm = world.resource::<Zm>();
            let x = zm.actors.get(&n).ok_or("no actor")?;
            (x.enemy, zm.ents.get(&n).map(|e| (e.origin, e.angles)))
        };
        let (Some(enemy), Some((origin, angles))) = (enemy, me) else {
            return Ok(Value::Undefined);
        };
        let ev = Value::Object(enemy);
        let Some(p) = origin_of(vm, world, &ev) else {
            return Ok(Value::Undefined);
        };
        let obj = s.as_obj().unwrap();
        let reach_s = vm.intern("meleeattackdist");
        let reach = vm
            .raw_field(obj, reach_s)
            .as_float()
            .unwrap_or(64.0)
            .max(64.0)
            + 16.0;
        let d = gsc_t6::math::sub(p, origin);
        let flat = (d[0] * d[0] + d[1] * d[1]).sqrt();
        if flat > reach || d[2].abs() > 72.0 {
            return Ok(Value::Undefined);
        }
        let fwd = gsc_t6::math::angle_vectors(angles).0;
        if flat > 1.0 && (fwd[0] * d[0] + fwd[1] * d[1]) / flat < 0.2 {
            return Ok(Value::Undefined);
        }
        let Some(victim) =
            entnum(vm, &ev).filter(|v| world.resource::<Zm>().players.contains_key(v))
        else {
            return Ok(Value::Undefined);
        };
        let dmg_s = vm.intern("meleedamage");
        let mut dmg = vm.raw_field(obj, dmg_s).as_int().unwrap_or(60);
        // bo2mp: a K9 Unit dog bites with its own weapon (`aiweapon`,
        // dog_bite_mp): named in the hit, and its melee damage.
        let weapon_s = vm.intern("aiweapon");
        let dog = world
            .resource::<Zm>()
            .actors
            .get(&n)
            .is_some_and(actors::is_mp_dog);
        let weapon = match vm.raw_field(obj, weapon_s) {
            v if dog && !v.is_undefined() => vm.to_text(&v),
            _ => "none".to_owned(),
        };
        if let Ok(w) = super::weapon(world, &weapon)
            && let Some(f) = frame(world).combat_facts_for(w)
            && f.melee_damage > 0
        {
            dmg = f.melee_damage;
        }
        let dir = gsc_t6::math::normalize(d);
        player_damage(
            vm,
            world,
            victim,
            s.clone(),
            s.clone(),
            dmg,
            0,
            "MOD_MELEE",
            &weapon,
            p,
            dir,
            "torso_upper",
        );
        Ok(ev)
    });
    m!("finishactordamage", |vm, world, s, a| {
        let n = actor_n(vm, world, s)?;
        let obj = s.as_obj().unwrap();
        let amount = int(a, 2).unwrap_or(0);
        let attacker = arg(a, 1).clone();
        let mod_ = arg(a, 4).clone();
        let weapon = arg(a, 5).clone();
        let point = arg(a, 6).clone();
        let dir = arg(a, 7).clone();
        let loc = arg(a, 8).clone();
        for (k, v) in [
            ("attacker", attacker.clone()),
            ("damagelocation", loc.clone()),
            ("damagemod", mod_.clone()),
            ("damageweapon", weapon.clone()),
            ("damagetaken", Value::Int(amount)),
        ] {
            let f = vm.intern(k);
            vm.set_raw_field(obj, f, v);
        }
        let health = {
            let mut zm = world.resource_mut::<Zm>();
            let Some(x) = zm.actors.get_mut(&n) else {
                return Ok(Value::Undefined);
            };
            if !x.alive {
                return Ok(Value::Undefined);
            }
            x.health -= amount.max(0);
            x.health
        };
        let none = vm.string("");
        vm.notify_str(
            world,
            obj,
            "damage",
            &[
                Value::Int(amount),
                attacker.clone(),
                dir.clone(),
                point,
                mod_.clone(),
                none.clone(),
                none.clone(),
                none,
                weapon.clone(),
            ],
        );
        if health <= 0 {
            let killed = vec![
                arg(a, 0).clone(),
                attacker.clone(),
                Value::Int(amount),
                mod_,
                weapon,
                dir,
                loc,
                Value::Int(0),
            ];
            actors::die(vm, world, n, attacker, killed);
        }
        Ok(Value::Undefined)
    });
    m!("dodamage", |vm, world, s, a| {
        let amount = num(a, 0).unwrap_or(0.0) as i32;
        let point = arg(a, 1).as_vec3().unwrap_or([0.0; 3]);
        let attacker = match arg(a, 2) {
            Value::Undefined => s.clone(),
            v => v.clone(),
        };
        let inflictor = match arg(a, 3) {
            Value::Undefined => attacker.clone(),
            v => v.clone(),
        };
        let hitloc = match arg(a, 4) {
            Value::Undefined => "none".to_owned(),
            v => vm.to_text(v),
        };
        let means = match arg(a, 5) {
            Value::Undefined => "MOD_UNKNOWN".to_owned(),
            v => vm.to_text(v),
        };
        let weapon = match arg(a, 7) {
            Value::Undefined => "none".to_owned(),
            v => vm.to_text(v),
        };
        let Some(n) = entnum(vm, s) else {
            return Ok(Value::Undefined);
        };
        if is_actor(vm, world, s) {
            actors::damage(
                vm, world, n, inflictor, attacker, amount, 0, &means, &weapon, point, [0.0; 3],
                &hitloc,
            );
        } else if is_player(vm, world, s) {
            player_damage(
                vm, world, n, inflictor, attacker, amount, 0, &means, &weapon, point, [0.0; 3],
                &hitloc,
            );
        } else if let Some(o) = s.as_obj() {
            let m = vm.string(&means);
            vm.notify_str(
                world,
                o,
                "damage",
                &[
                    Value::Int(amount),
                    attacker,
                    Value::Vec3([0.0; 3]),
                    Value::Vec3(point),
                    m,
                ],
            );
        }
        Ok(Value::Undefined)
    });
    m!("finishplayerdamage", |vm, world, s, a| {
        let n = entnum(vm, s).ok_or("not a player")?;
        let id = ClientId(n);
        let amount = int(a, 2).unwrap_or(0);
        let dir = arg(a, 7).as_vec3();
        let finish = crate::script_player::finish_damage(&mut frame(world), id, amount, dir);
        // bo2mp: in multiplayer a lethal hit kills: the engine's death (body,
        // the client dead), then BO2's own CodeCallback_PlayerKilled (kill
        // feed, score, killcam, respawn).
        if finish == crate::script_player::Finish::Killed && world.resource::<Zm>().mp {
            let attacker = entnum(vm, arg(a, 1))
                .filter(|n| world.resource::<Zm>().players.contains_key(n))
                .map(ClientId);
            let tick = super::tick(world);
            // bo2mp: his death animation (BO2's DEATH event) before the
            // engine's death; his corpse keeps it.
            let means = vm.to_text(arg(a, 4));
            let weapon = vm.to_text(arg(a, 5));
            super::playeranim::death(world, id, &means, dir, &weapon);
            crate::script_player::kill(&mut frame(world), tick, id, attacker, None);
            let obj = s.as_obj().unwrap();
            let args = vec![
                arg(a, 0).clone(),
                arg(a, 1).clone(),
                Value::Int(amount),
                arg(a, 4).clone(),
                arg(a, 5).clone(),
                arg(a, 7).clone(),
                arg(a, 8).clone(),
                arg(a, 9).clone(),
                Value::Int(0),
            ];
            let cb = super::mp::callbacks(world);
            vm.spawn_named(
                world,
                &cb,
                "codecallback_playerkilled",
                Value::Object(obj),
                args,
            );
            return Ok(Value::Undefined);
        }
        if finish != crate::script_player::Finish::Hurt {
            // Zombies: a lethal hit puts the player into last stand (the
            // scripts decide what follows).
            if let Some(ps) = frame(world).player_mut(id) {
                ps.health = ps.health.max(1);
            }
            if let Some(p) = world.resource_mut::<Zm>().players.get_mut(&n) {
                p.laststand = true;
            }
            let obj = s.as_obj().unwrap();
            let args = vec![
                arg(a, 0).clone(),
                arg(a, 1).clone(),
                Value::Int(amount),
                arg(a, 4).clone(),
                arg(a, 5).clone(),
                arg(a, 7).clone(),
                arg(a, 8).clone(),
                Value::Int(0),
                Value::Int(0),
            ];
            let cb = super::mp::callbacks(world);
            vm.spawn_named(
                world,
                &cb,
                "codecallback_playerlaststand",
                Value::Object(obj),
                args,
            );
        }
        Ok(Value::Undefined)
    });
    // `gib(type, limb tags)`: each limb's own model (its character's
    // gibspawnN at gibspawntagN) flies off and falls, gone after a while.
    m!("gib", |vm, world, s, a| {
        let n = actor_n(vm, world, s)?;
        let Some(obj) = s.as_obj() else {
            return Ok(Value::Undefined);
        };
        let up = vm.to_text(arg(a, 0)) == "up";
        let tags: Vec<String> = match arg(a, 1) {
            Value::Array(arr) => arr
                .snapshot()
                .values_in_order()
                .map(|v| vm.to_text(v).to_ascii_lowercase())
                .collect(),
            v => vec![vm.to_text(v).to_ascii_lowercase()],
        };
        let mut pieces = Vec::new();
        for i in 1..=8 {
            let tag = {
                let f = vm.intern(&format!("gibspawntag{i}"));
                vm.raw_field(obj, f)
            };
            let model = {
                let f = vm.intern(&format!("gibspawn{i}"));
                vm.raw_field(obj, f)
            };
            if tag.is_undefined() || model.is_undefined() {
                continue;
            }
            let tag = vm.to_text(&tag).to_ascii_lowercase();
            if tags.contains(&tag) {
                pieces.push(vm.to_text(&model));
            }
        }
        let (origin, yaw) = {
            let zm = world.resource::<Zm>();
            zm.ents
                .get(&n)
                .map_or(([0.0; 3], 0.0), |e| (e.origin, e.angles[1]))
        };
        let now = world.resource::<Zm>().now_ms;
        for (k, model) in pieces.into_iter().enumerate() {
            let side = if k % 2 == 0 { 90.0 } else { -90.0 };
            let (dx, dy) = (yaw + side).to_radians().sin_cos();
            let from = [origin[0], origin[1], origin[2] + 45.0];
            let vel = if up {
                [dy * 60.0, dx * 60.0, 420.0]
            } else {
                [dy * 140.0, dx * 140.0, 160.0]
            };
            let g = world.resource_mut::<Zm>().alloc_entnum();
            let gobj = vm.alloc_object(ObjKind::Entity(g));
            let mut zm = world.resource_mut::<Zm>();
            zm.ents.insert(
                g,
                super::Ent {
                    obj: Some(gobj),
                    classname: "script_model".into(),
                    origin: from,
                    angles: [0.0, yaw, 0.0],
                    model,
                    ..Default::default()
                },
            );
            zm.movers.gravity(g, from, vel, now, 1.2);
            zm.gibs.push((g, now + 6000));
        }
        Ok(Value::Undefined)
    });
    vm.bind("radiusdamage", false, |vm, world, _, a| {
        radius_damage(vm, world, a);
        Ok(Value::Undefined)
    });
    // bo2mp: `ent radiusdamage(...)` (the Demolition bomb's blast): the
    // same, the entity being the blast's source.
    vm.bind("radiusdamage", true, |vm, world, _, a| {
        radius_damage(vm, world, a);
        Ok(Value::Undefined)
    });
    let _ = ObjKind::Struct;
}

/// `radiusdamage(origin, range, max, min, attacker, means, weapon)`: every
/// zombie and player in range takes max..min by distance (not through
/// walls), as the callbacks would from a blast.
fn radius_damage(vm: &mut Vm<World>, world: &mut World, a: &[Value]) {
    let Some(origin) = arg(a, 0).as_vec3() else {
        return;
    };
    let range = arg(a, 1).as_float().unwrap_or(0.0).max(1.0);
    let max = arg(a, 2).as_float().unwrap_or(0.0);
    let min = arg(a, 3).as_float().unwrap_or(0.0);
    let attacker = arg(a, 4).clone();
    let means = match arg(a, 5) {
        Value::Undefined => "MOD_EXPLOSIVE".to_owned(),
        v => vm.to_text(v),
    };
    let weapon = match arg(a, 6) {
        Value::Undefined => "none".to_owned(),
        v => vm.to_text(v),
    };
    let amount = |d: f32| (max - (max - min) * (d / range).clamp(0.0, 1.0)).round() as i32;
    let clear = |world: &mut World, to: [f32; 3]| {
        let start = [origin[0], origin[1], origin[2] + 4.0];
        super::frame(world)
            .trace_static_world(
                start,
                to,
                [0.0; 3],
                [0.0; 3],
                crate::bullet_collision::MASK_SHOT,
            )
            .fraction
            >= 0.99
    };
    let actors: Vec<(u32, [f32; 3])> = {
        let zm = world.resource::<Zm>();
        zm.actors
            .iter()
            .filter(|(_, x)| x.alive)
            .filter_map(|(n, x)| {
                zm.ents
                    .get(n)
                    .map(|e| (*n, [e.origin[0], e.origin[1], e.origin[2] + x.height * 0.5]))
            })
            .collect()
    };
    for (n, mid) in actors {
        let d = gsc_t6::math::length(gsc_t6::math::sub(mid, origin));
        if d > range || !clear(world, mid) {
            continue;
        }
        let dir = gsc_t6::math::normalize(gsc_t6::math::sub(mid, origin));
        actors::damage(
            vm,
            world,
            n,
            attacker.clone(),
            attacker.clone(),
            amount(d),
            1,
            &means,
            &weapon,
            mid,
            dir,
            "none",
        );
    }
    let players: Vec<(u32, [f32; 3])> = {
        let ids: Vec<u32> = world.resource::<Zm>().players.keys().copied().collect();
        let f = super::frame(world);
        ids.into_iter()
            .filter_map(|c| {
                f.player(crate::world::ClientId(c))
                    .map(|p| (c, [p.origin[0], p.origin[1], p.origin[2] + 36.0]))
            })
            .collect()
    };
    for (c, mid) in players {
        let d = gsc_t6::math::length(gsc_t6::math::sub(mid, origin));
        if d > range || !clear(world, mid) {
            continue;
        }
        let dir = gsc_t6::math::normalize(gsc_t6::math::sub(mid, origin));
        player_damage(
            vm,
            world,
            c,
            attacker.clone(),
            attacker.clone(),
            amount(d),
            1,
            &means,
            &weapon,
            mid,
            dir,
            "none",
        );
    }
}

fn set_goal(vm: &mut Vm<World>, world: &mut World, n: u32, goal: [f32; 3]) {
    let found = actors::plan(world, n, goal);
    let obj = {
        let mut zm = world.resource_mut::<Zm>();
        let Some(x) = zm.actors.get_mut(&n) else {
            return;
        };
        x.goal = Some(goal);
        x.goal_sent = false;
        if !found {
            x.path.clear();
            x.path_nodes.clear();
        }
        x.obj
    };
    if !found && let Some(o) = obj {
        super::natives_game::notify_later(vm, world, o, "bad_path", 50);
    }
}
