//! bo2mp: the engine half of Black Ops II's bots.
//!
//! The game's own bot scripts (`maps/mp/bots/_bot*`) decide everything a
//! bot does: where to go (named goals with priorities: "wander",
//! "enemy_patrol", "cover"...), whom to fight (`getthreats`), where to look,
//! when the trigger may be pulled, when to aim down the sights, throw,
//! reload, melee or dive. The engine carries it out: a test client joins
//! (`addtestclient`), walks the best goal's route over the map's path
//! nodes, turns its view toward what it looks at (or down its route) at its
//! difficulty's speed, and presses the buttons. Each tick a bot's command
//! replaces its client's own, as the zombies test player does.

use std::collections::BTreeMap;

use bevy_ecs::prelude::World;
use gsc_t6::{Array, ObjKind, Value, Vm};
use playerstate_iw4::buttons;

use super::{Player, Zm, arg, entnum, frame, list, origin_of, text};
use crate::world::ClientId;

/// A named place a bot wants to be (`addgoal`): the highest priority wins,
/// the newest among equals.
#[derive(Clone, Debug)]
struct Goal {
    name: String,
    pos: [f32; 3],
    radius: f32,
    priority: i32,
    order: u64,
}

/// What a bot's scripts set up for its custom classes (`botclassadditem`,
/// `botclassaddattachment`, `botsetdefaultclass`): read by the loadout.
#[derive(Clone, Debug, Default)]
pub(crate) struct BotClass {
    pub default: Option<String>,
    pub items: Vec<String>,
    /// (weapon, attachment, slot such as "primaryattachment1").
    pub attachments: Vec<(String, String, String)>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Bot {
    goals: Vec<Goal>,
    order: u64,
    /// Route to the best goal: points, the goal last.
    path: Vec<[f32; 3]>,
    path_to: Option<[f32; 3]>,
    replan_at: i64,
    look: Option<[f32; 3]>,
    allow_attack: bool,
    ads: bool,
    attack_ticks: u32,
    use_until: i64,
    melee: bool,
    dive: bool,
    /// 0 stand, 1 crouch, 2 prone (`setstance`).
    stance: u8,
    no_sprint: bool,
    failsafe: Option<[f32; 3]>,
    tick: u32,
    /// Where it was when it last made headway, and when (stuck checks).
    progress: Option<([f32; 3], i64)>,
    pub classes: BTreeMap<i32, BotClass>,
}

impl Bot {
    fn best_goal(&self) -> Option<&Goal> {
        self.goals
            .iter()
            .max_by_key(|g| (g.priority, g.order))
    }
}

fn dist2d(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Yaw and pitch (degrees, the game's: pitch down positive) from a to b.
fn angles_to(a: [f32; 3], b: [f32; 3]) -> (f32, f32) {
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let yaw = d[1].atan2(d[0]).to_degrees();
    let flat = (d[0] * d[0] + d[1] * d[1]).sqrt();
    let pitch = -d[2].atan2(flat).to_degrees();
    (yaw, pitch)
}

fn angle_delta(from: f32, to: f32) -> f32 {
    (to - from + 540.0).rem_euclid(360.0) - 180.0
}

/// The eye of a player: his origin plus his view height.
fn eye(world: &mut World, n: u32) -> Option<[f32; 3]> {
    frame(world)
        .player(ClientId(n))
        .map(|ps| [ps.origin[0], ps.origin[1], ps.origin[2] + ps.view_height_current])
}

/// A player in play: on a team, playing, with health.
fn alive(world: &mut World, n: u32) -> bool {
    let playing = world
        .resource::<Zm>()
        .players
        .get(&n)
        .is_some_and(|p| p.sessionstate == "playing");
    playing && frame(world).player(ClientId(n)).is_some_and(|ps| ps.health > 0)
}

fn team_of(vm: &mut Vm<World>, world: &World, n: u32) -> String {
    let Some(p) = world.resource::<Zm>().players.get(&n) else {
        return String::new();
    };
    let f = vm.intern("team");
    let v = vm.raw_field(p.obj, f);
    vm.to_text(&v)
}

/// Is the way toward `to` blocked at step height (a player's box raised by
/// the walk's step, 24 units on)?
fn blocked_ahead(world: &mut World, origin: [f32; 3], to: [f32; 3]) -> bool {
    use crate::bullet_collision::{MASK_PLAYER_SOLID, PLAYER_MAXS, PLAYER_MINS};
    let d = [to[0] - origin[0], to[1] - origin[1]];
    let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
    if len < 1.0 {
        return false;
    }
    let start = [origin[0], origin[1], origin[2] + 18.0];
    let end = [start[0] + d[0] / len * 24.0, start[1] + d[1] / len * 24.0, start[2]];
    let maxs = [PLAYER_MAXS[0], PLAYER_MAXS[1], PLAYER_MAXS[2] - 18.0];
    let t = frame(world).trace_clip(start, end, PLAYER_MINS, maxs, MASK_PLAYER_SOLID);
    t.startsolid != 0 || t.fraction < 1.0
}

fn sight(world: &mut World, from: [f32; 3], to: [f32; 3]) -> bool {
    let t = frame(world).trace_static_world(
        from,
        to,
        [0.0; 3],
        [0.0; 3],
        crate::bullet_collision::MASK_SHOT,
    );
    t.fraction >= 1.0
}

/// Players on other teams (or every other player without teams), in play.
/// Is this a team game (`level.teambased`)? Free-for-all players all
/// stand on one team name and are each other's enemies.
fn teambased(vm: &mut Vm<World>) -> bool {
    let f = vm.intern("teambased");
    gsc_t6::truthy(&vm.raw_field(vm.level, f))
}

fn enemies_of(vm: &mut Vm<World>, world: &mut World, n: u32, alive_only: bool) -> Vec<u32> {
    let mine = team_of(vm, world, n);
    let teambased = teambased(vm) && mine != "free" && !mine.is_empty();
    let ids: Vec<u32> = world.resource::<Zm>().players.keys().copied().collect();
    let mut out = Vec::new();
    for id in ids {
        if id == n {
            continue;
        }
        let theirs = team_of(vm, world, id);
        if theirs == "spectator" || theirs.is_empty() {
            continue;
        }
        if teambased && theirs == mine {
            continue;
        }
        if alive_only && !alive(world, id) {
            continue;
        }
        out.push(id);
    }
    out
}

fn friends_of(vm: &mut Vm<World>, world: &mut World, n: u32, alive_only: bool) -> Vec<u32> {
    if !teambased(vm) {
        return Vec::new();
    }
    let mine = team_of(vm, world, n);
    let ids: Vec<u32> = world.resource::<Zm>().players.keys().copied().collect();
    let mut out = Vec::new();
    for id in ids {
        if id == n || team_of(vm, world, id) != mine {
            continue;
        }
        if alive_only && !alive(world, id) {
            continue;
        }
        out.push(id);
    }
    out
}

fn objs(world: &World, ids: &[u32]) -> Vec<Value> {
    let zm = world.resource::<Zm>();
    ids.iter()
        .filter_map(|id| zm.players.get(id).map(|p| Value::Object(p.obj)))
        .collect()
}

/// A target's place: a point, an entity, a path node (struct origin).
fn place(vm: &mut Vm<World>, world: &mut World, v: &Value) -> Option<[f32; 3]> {
    if let Some(p) = v.as_vec3() {
        return Some(p);
    }
    if let Some(p) = origin_of(vm, world, v) {
        return Some(p);
    }
    if let Value::Object(o) = v {
        let f = vm.intern("origin");
        return vm.raw_field(*o, f).as_vec3();
    }
    None
}

fn bot_mut<'w>(world: &'w mut World, n: u32) -> Option<bevy_ecs::prelude::Mut<'w, Zm>> {
    let zm = world.resource_mut::<Zm>();
    zm.bots.contains_key(&n).then_some(zm)
}

/// The free client slots: the local player is 0; bots take the next free.
fn free_client(vm: &Vm<World>, world: &mut World) -> Option<u32> {
    let max = vm
        .dvars
        .get("sv_maxclients")
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(18);
    let mut used: Vec<u32> = frame(world)
        .client_ids_sorted()
        .into_iter()
        .map(|c| c.0)
        .collect();
    let zm = world.resource::<Zm>();
    used.extend(zm.players.keys().copied());
    used.extend(zm.test_client_requests.iter().copied());
    (1..max).find(|i| !used.contains(i))
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
    // A bot joins: its player entity at once (the script marks it a bot and
    // starts its spawn thread before it connects), its client next frames.
    f!("addtestclient", |vm, world, _, _| {
        let Some(n) = free_client(vm, world) else {
            return Ok(Value::Undefined);
        };
        let obj = vm.alloc_object(ObjKind::Entity(n));
        let pers = vm.intern("pers");
        vm.set_raw_field(obj, pers, Value::array(Array::new()));
        let mut zm = world.resource_mut::<Zm>();
        let mut player = Player::new(obj);
        player.test_client = true;
        player.pending_since = Some(zm.now_ms);
        zm.players.insert(n, player);
        zm.bots.insert(n, Bot::default());
        zm.test_client_requests.push(n);
        diag::info!(Sim, "bo2mp bot: test client {n} joins");
        Ok(Value::Object(obj))
    });
    m!("istestclient", |vm, world, s, _| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        Ok(Value::bool(
            world
                .resource::<Zm>()
                .players
                .get(&n)
                .is_some_and(|p| p.test_client),
        ))
    });
    m!("botleavegame", |vm, world, s, _| {
        if let Some(n) = entnum(vm, s) {
            world.resource_mut::<Zm>().bot_leaves.push(n);
        }
        Ok(Value::Undefined)
    });
    // Goals.
    m!("addgoal", |vm, world, s, a| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        let Some(pos) = place(vm, world, arg(a, 0)) else {
            return Ok(Value::Undefined);
        };
        let radius = arg(a, 1).as_float().unwrap_or(24.0);
        let priority = arg(a, 2).as_int().unwrap_or(1);
        let name = text(vm, a, 3);
        if let Some(mut zm) = bot_mut(world, n) {
            let b = zm.bots.get_mut(&n).unwrap();
            b.order += 1;
            let order = b.order;
            b.goals.retain(|g| g.name != name);
            b.goals.push(Goal {
                name,
                pos,
                radius,
                priority,
                order,
            });
        }
        Ok(Value::Undefined)
    });
    m!("cancelgoal", |vm, world, s, a| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        let name = text(vm, a, 0);
        if let Some(mut zm) = bot_mut(world, n) {
            zm.bots.get_mut(&n).unwrap().goals.retain(|g| g.name != name);
        }
        Ok(Value::Undefined)
    });
    m!("hasgoal", |vm, world, s, a| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        let name = text(vm, a, 0);
        Ok(Value::bool(world.resource::<Zm>().bots.get(&n).is_some_and(|b| {
            b.goals.iter().any(|g| g.name == name)
        })))
    });
    m!("getgoal", |vm, world, s, a| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        let name = text(vm, a, 0);
        Ok(world
            .resource::<Zm>()
            .bots
            .get(&n)
            .and_then(|b| b.goals.iter().find(|g| g.name == name))
            .map_or(Value::Undefined, |g| Value::Vec3(g.pos)))
    });
    // At a goal (named, else the one it walks to): within its radius. No
    // goal at all: where it wants to be.
    m!("atgoal", |vm, world, s, a| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        let name = arg(a, 0);
        let name = if vm.is_defined(name) {
            Some(vm.to_text(name))
        } else {
            None
        };
        let origin = frame(world).player(ClientId(n)).map(|ps| ps.origin);
        let zm = world.resource::<Zm>();
        let Some(b) = zm.bots.get(&n) else {
            return Ok(Value::Int(0));
        };
        let goal = match &name {
            Some(name) => b.goals.iter().find(|g| &g.name == name),
            None => b.best_goal(),
        };
        let at = match (goal, origin) {
            (Some(g), Some(o)) => dist2d(o, g.pos) <= g.radius.max(16.0) && (o[2] - g.pos[2]).abs() < 72.0,
            (None, _) => name.is_none(),
            _ => false,
        };
        Ok(Value::bool(at))
    });
    // The route ahead: its next point's direction and distance (undefined
    // with no route).
    m!("getlookaheaddir", |vm, world, s, _| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        let origin = frame(world).player(ClientId(n)).map(|ps| ps.origin);
        let next = world
            .resource::<Zm>()
            .bots
            .get(&n)
            .and_then(|b| b.path.first().copied());
        Ok(match (next, origin) {
            (Some(p), Some(o)) => {
                let d = [p[0] - o[0], p[1] - o[1], 0.0];
                let len = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1e-3);
                Value::Vec3([d[0] / len, d[1] / len, 0.0])
            }
            _ => Value::Undefined,
        })
    });
    m!("getlookaheaddist", |vm, world, s, _| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        let origin = frame(world).player(ClientId(n)).map(|ps| ps.origin);
        let next = world
            .resource::<Zm>()
            .bots
            .get(&n)
            .and_then(|b| b.path.first().copied());
        Ok(match (next, origin) {
            (Some(p), Some(o)) => Value::Float(dist2d(p, o)),
            _ => Value::Float(0.0),
        })
    });
    m!("botsetfailsafenode", |vm, world, s, a| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        let p = place(vm, world, arg(a, 0));
        if let Some(mut zm) = bot_mut(world, n) {
            zm.bots.get_mut(&n).unwrap().failsafe = p;
        }
        Ok(Value::Undefined)
    });
    // Looking and buttons.
    m!("lookat", |vm, world, s, a| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        let p = place(vm, world, arg(a, 0));
        if let Some(mut zm) = bot_mut(world, n) {
            zm.bots.get_mut(&n).unwrap().look = p;
        }
        Ok(Value::Undefined)
    });
    m!("clearlookat", |vm, world, s, _| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        if let Some(mut zm) = bot_mut(world, n) {
            zm.bots.get_mut(&n).unwrap().look = None;
        }
        Ok(Value::Undefined)
    });
    m!("allowattack", |vm, world, s, a| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        let on = gsc_t6::truthy(arg(a, 0));
        if let Some(mut zm) = bot_mut(world, n) {
            zm.bots.get_mut(&n).unwrap().allow_attack = on;
        }
        Ok(Value::Undefined)
    });
    m!("pressads", |vm, world, s, a| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        let on = gsc_t6::truthy(arg(a, 0));
        if let Some(mut zm) = bot_mut(world, n) {
            zm.bots.get_mut(&n).unwrap().ads = on;
        }
        Ok(Value::Undefined)
    });
    // One pull of the trigger (an argument: held that many ticks).
    m!("pressattackbutton", |vm, world, s, a| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        let ticks = arg(a, 0).as_int().unwrap_or(1).clamp(1, 40) as u32;
        if let Some(mut zm) = bot_mut(world, n) {
            zm.bots.get_mut(&n).unwrap().attack_ticks = ticks;
        }
        Ok(Value::Undefined)
    });
    // Use held for that long (reloads when nothing is there to use).
    m!("pressusebutton", |vm, world, s, a| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        let secs = arg(a, 0).as_float().unwrap_or(0.1);
        if let Some(mut zm) = bot_mut(world, n) {
            let until = zm.now_ms + (secs * 1000.0) as i64;
            zm.bots.get_mut(&n).unwrap().use_until = until;
        }
        Ok(Value::Undefined)
    });
    m!("pressmelee", |vm, world, s, _| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        if let Some(mut zm) = bot_mut(world, n) {
            zm.bots.get_mut(&n).unwrap().melee = true;
        }
        Ok(Value::Undefined)
    });
    m!("pressdtpbutton", |vm, world, s, _| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        if let Some(mut zm) = bot_mut(world, n) {
            zm.bots.get_mut(&n).unwrap().dive = true;
        }
        Ok(Value::Undefined)
    });
    m!("setstance", |vm, world, s, a| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        let stance = match text(vm, a, 0).as_str() {
            "crouch" => 1,
            "prone" => 2,
            _ => 0,
        };
        if let Some(mut zm) = bot_mut(world, n) {
            zm.bots.get_mut(&n).unwrap().stance = stance;
        }
        Ok(Value::Undefined)
    });
    m!("allowsprint", |vm, world, s, a| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        let on = gsc_t6::truthy(arg(a, 0));
        if let Some(mut zm) = bot_mut(world, n) {
            zm.bots.get_mut(&n).unwrap().no_sprint = !on;
        }
        // bo2mp: the same switch every player has (the server drops sprint).
        let id = crate::world::ClientId(n);
        let mut f = super::frame(world);
        if f.client_meta(id).is_some() {
            f.client_meta_mut(id).controls.sprint_disabled = !on;
        }
        Ok(Value::Undefined)
    });
    // A grenade at a point: look there and throw.
    m!("throwgrenade", |vm, world, s, a| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        let p = place(vm, world, arg(a, 1));
        if let Some(mut zm) = bot_mut(world, n) {
            let b = zm.bots.get_mut(&n).unwrap();
            if p.is_some() {
                b.look = p;
            }
            b.tick = 0;
            b.attack_ticks = 0;
        }
        world.resource_mut::<Zm>().bot_throws.push(n);
        Ok(Value::Undefined)
    });
    // Seeing.
    m!("botsighttracepassed", |vm, world, s, a| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        let Some(from) = eye(world, n) else {
            return Ok(Value::Int(0));
        };
        let target = arg(a, 0);
        let to = match entnum(vm, target) {
            Some(t) if world.resource::<Zm>().players.contains_key(&t) => eye(world, t),
            _ => place(vm, world, target).map(|p| [p[0], p[1], p[2] + 32.0]),
        };
        Ok(Value::bool(to.is_some_and(|to| sight(world, from, to))))
    });
    // Enemies it can see, nearest first, inside its field of view: the
    // argument is the smallest cosine between its view and the enemy (its
    // scripts' `bot.fov`: 0.0872 = 85 degrees each side on normal; -1 =
    // all round).
    m!("getthreats", |vm, world, s, a| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        let fov = arg(a, 0).as_float().unwrap_or(-1.0);
        let Some(from) = eye(world, n) else {
            return Ok(list(Vec::new()));
        };
        let view = frame(world).player(ClientId(n)).map_or([0.0; 3], |ps| ps.viewangles);
        let mut seen: Vec<(f32, u32)> = Vec::new();
        for e in enemies_of(vm, world, n, true) {
            let Some(to) = eye(world, e) else { continue };
            if fov > -1.0 {
                let (yaw, pitch) = (view[1].to_radians(), view[0].to_radians());
                let forward = [pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), -pitch.sin()];
                let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
                let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-3);
                let dot = (forward[0] * d[0] + forward[1] * d[1] + forward[2] * d[2]) / len;
                if dot < fov {
                    continue;
                }
            }
            if sight(world, from, to) {
                seen.push((dist2d(from, to), e));
            }
        }
        seen.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
        let ids: Vec<u32> = seen.into_iter().map(|(_, e)| e).collect();
        Ok(list(objs(world, &ids)))
    });
    m!("getenemies", |vm, world, s, a| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        let alive_only = gsc_t6::truthy(arg(a, 0));
        let ids = enemies_of(vm, world, n, alive_only);
        Ok(list(objs(world, &ids)))
    });
    m!("getfriendlies", |vm, world, s, a| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        let alive_only = gsc_t6::truthy(arg(a, 0));
        let ids = friends_of(vm, world, n, alive_only);
        Ok(list(objs(world, &ids)))
    });
    // Where a player will be in that many frames at his speed.
    m!("predictposition", |vm, world, _, a| {
        let Some(t) = entnum(vm, arg(a, 0)) else {
            return Ok(Value::Undefined);
        };
        let frames = arg(a, 1).as_float().unwrap_or(1.0);
        Ok(frame(world)
            .player(ClientId(t))
            .map_or(Value::Undefined, |ps| {
                Value::Vec3(std::array::from_fn(|i| {
                    ps.origin[i] + ps.velocity[i] * frames * 0.05
                }))
            }))
    });
    // A player's eye height over his feet (where bots aim: torso = feet +
    // height / 1.6).
    m!("getplayerviewheight", |vm, world, s, _| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        Ok(Value::Float(
            frame(world)
                .player(ClientId(n))
                .map_or(60.0, |ps| ps.view_height_current),
        ))
    });
    // An entity's middle: a player's chest, else 32 over its origin.
    m!("getcentroid", |vm, world, s, _| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        if let Some(ps) = frame(world).player(ClientId(n)) {
            let o = ps.origin;
            return Ok(Value::Vec3([o[0], o[1], o[2] + ps.view_height_current * 0.6]));
        }
        Ok(origin_of(vm, world, s).map_or(Value::Undefined, |o| Value::Vec3([o[0], o[1], o[2] + 32.0])))
    });
    f!("weaponisdualwield", |vm, _, _, a| {
        let w = text(vm, a, 0);
        Ok(Value::bool(w.contains("+dw") || w.ends_with("dw_mp")))
    });
    f!("isweaponscopeoverlay", |_, _, _, _| Ok(Value::Int(0)));
    f!("target_istarget", |_, _, _, _| Ok(Value::Int(0)));
    m!("getplayercamerapos", |vm, world, s, _| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        Ok(eye(world, n).map_or(Value::Undefined, Value::Vec3))
    });
    // A bot's own classes (its scripts build five from his item table).
    m!("botsetdefaultclass", |vm, world, s, a| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        let class = arg(a, 0).as_int().unwrap_or(0);
        let name = text(vm, a, 1);
        if let Some(mut zm) = bot_mut(world, n) {
            zm.bots.get_mut(&n).unwrap().classes.entry(class).or_default().default = Some(name);
        }
        Ok(Value::Undefined)
    });
    m!("botclassadditem", |vm, world, s, a| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        let class = arg(a, 0).as_int().unwrap_or(0);
        let item = text(vm, a, 1);
        if let Some(mut zm) = bot_mut(world, n) {
            zm.bots.get_mut(&n).unwrap().classes.entry(class).or_default().items.push(item);
        }
        Ok(Value::Undefined)
    });
    m!("botclassaddattachment", |vm, world, s, a| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        let class = arg(a, 0).as_int().unwrap_or(0);
        let (weapon, attachment, slot) = (text(vm, a, 1), text(vm, a, 2), text(vm, a, 3));
        if let Some(mut zm) = bot_mut(world, n) {
            zm.bots
                .get_mut(&n)
                .unwrap()
                .classes
                .entry(class)
                .or_default()
                .attachments
                .push((weapon, attachment, slot));
        }
        Ok(Value::Undefined)
    });
    m!("botclasssetweaponoption", |_, _, _, _| Ok(Value::Undefined));
    // The player the bots count teams around: the local player (client 0).
    f!("gethostplayerforbots", |_, world, _, _| Ok(world
        .resource::<Zm>()
        .players
        .get(&0)
        .map_or(Value::Undefined, |p| Value::Object(p.obj))));
    m!("ishostforbots", |vm, _, s, _| Ok(Value::bool(entnum(vm, s) == Some(0))));
    // The map's path nodes as the bot scripts ask about them.
    fn node_obj(world: &World, i: u32) -> Value {
        world
            .resource::<Zm>()
            .node_objs
            .get(i as usize)
            .map_or(Value::Undefined, |o| Value::Object(*o))
    }
    fn node_index(vm: &mut Vm<World>, world: &World, v: &Value) -> Option<u32> {
        let Value::Object(o) = v else { return None };
        world
            .resource::<Zm>()
            .node_objs
            .iter()
            .position(|x| x == o)
            .map(|i| i as u32)
            .or_else(|| {
                let f = vm.intern("origin");
                let p = vm.raw_field(*o, f).as_vec3()?;
                world.resource::<Zm>().nav.nearest(p)
            })
    }
    fn node_eye(world: &World, i: u32) -> [f32; 3] {
        let o = world.resource::<Zm>().nav.nodes[i as usize].origin;
        [o[0], o[1], o[2] + 32.0]
    }
    f!("getnearestnode", |vm, world, _, a| {
        let Some(p) = place(vm, world, arg(a, 0)) else {
            return Ok(Value::Undefined);
        };
        let i = world.resource::<Zm>().nav.nearest(p);
        Ok(i.map_or(Value::Undefined, |i| node_obj(world, i)))
    });
    // A node near `from` that can see `to` (nearest first, within 512).
    f!("getvisiblenode", |vm, world, _, a| {
        let (Some(from), Some(to)) = (place(vm, world, arg(a, 0)), place(vm, world, arg(a, 1)))
        else {
            return Ok(Value::Undefined);
        };
        let near = world.resource::<Zm>().nav.within(from, 512.0);
        for (i, _) in near.into_iter().take(12) {
            let e = node_eye(world, i);
            if sight(world, e, [to[0], to[1], to[2] + 32.0]) {
                return Ok(node_obj(world, i));
            }
        }
        Ok(Value::Undefined)
    });
    // The nodes a node sees: its linked neighbours that it can see.
    f!("getvisiblenodes", |vm, world, _, a| {
        let Some(i) = node_index(vm, world, arg(a, 0)) else {
            return Ok(list(Vec::new()));
        };
        let links: Vec<u32> = world.resource::<Zm>().nav.nodes[i as usize]
            .links
            .iter()
            .map(|l| u32::from(l.0))
            .collect();
        let from = node_eye(world, i);
        let mut out = Vec::new();
        for j in links {
            if sight(world, from, node_eye(world, j)) {
                out.push(node_obj(world, j));
            }
        }
        Ok(list(out))
    });
    f!("nodesvisible", |vm, world, _, a| {
        let (Some(i), Some(j)) = (node_index(vm, world, arg(a, 0)), node_index(vm, world, arg(a, 1)))
        else {
            return Ok(Value::Int(0));
        };
        let (x, y) = (node_eye(world, i), node_eye(world, j));
        Ok(Value::bool(sight(world, x, y)))
    });
    f!("nodescanpath", |vm, world, _, a| {
        let (Some(i), Some(j)) = (node_index(vm, world, arg(a, 0)), node_index(vm, world, arg(a, 1)))
        else {
            return Ok(Value::Int(0));
        };
        Ok(Value::bool(world.resource::<Zm>().nav.path(i, j).is_some()))
    });
    // A route between two points: its nodes in order (undefined: none).
    f!("findpath", |vm, world, _, a| {
        let (Some(from), Some(to)) = (place(vm, world, arg(a, 0)), place(vm, world, arg(a, 1)))
        else {
            return Ok(Value::Undefined);
        };
        let route = {
            let nav = &world.resource::<Zm>().nav;
            match (nav.nearest(from), nav.nearest(to)) {
                (Some(x), Some(y)) => nav.path(x, y),
                _ => None,
            }
        };
        Ok(route.map_or(Value::Undefined, |r| {
            list(r.into_iter().map(|i| node_obj(world, i)).collect())
        }))
    });
    f!("canclaimnode", |_, _, _, _| Ok(Value::Int(1)));
    f!("claimnode", |_, _, _, _| Ok(Value::Undefined));
    f!("releaseclaimednode", |_, _, _, _| Ok(Value::Undefined));
    // Bots have every item (their scripts ask before building a class).
    m!("isitemlocked", |_, _, _, _| Ok(Value::Int(0)));
    // What a player is doing, from his state.
    m!("isreloading", |vm, world, s, _| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        Ok(Value::bool(frame(world).player(ClientId(n)).is_some_and(|ps| {
            matches!(ps.weaponstate_primary, 7..=11)
        })))
    });
    for name in ["isfiring", "ismantling", "isonladder", "isthrowinggrenade", "isswitchingweapons", "isremotecontrolling", "iscarryingturret"] {
        vm.bind(name, true, |_, _, _, _| Ok(Value::Int(0)));
    }
}

/// Each tick: the bots' commands (route, view, buttons) replace their
/// clients' own; a bot whose client never came is dropped.
pub(super) fn drive(world: &mut World, now: i64) {
    let ids: Vec<u32> = world.resource::<Zm>().bots.keys().copied().collect();
    let difficulty = world
        .resource::<Zm>()
        .bot_difficulty
        .clamp(0, 3);
    // Degrees the view may turn per 50 ms tick: easy .. "fu".
    let turn = [9.0f32, 18.0, 27.0, 40.0][difficulty as usize];
    let throws = std::mem::take(&mut world.resource_mut::<Zm>().bot_throws);
    for n in ids {
        if !alive(world, n) {
            continue;
        }
        let Some((origin, view, height, ladder)) = frame(world)
            .player(ClientId(n))
            .map(|ps| {
                (
                    ps.origin,
                    ps.viewangles,
                    ps.view_height_current,
                    ps.pm_flags & playerstate_iw4::pm_flags::LADDER != 0,
                )
            })
        else {
            continue;
        };
        // The route: replan when the best goal moved or the route ran out.
        let (goal, path_to, replan_at) = {
            let zm = world.resource::<Zm>();
            let b = &zm.bots[&n];
            (b.best_goal().cloned(), b.path_to, b.replan_at)
        };
        if let Some(g) = &goal {
            let moved = path_to.is_none_or(|p| dist2d(p, g.pos) > 16.0);
            let far = dist2d(origin, g.pos) > g.radius;
            let empty = world.resource::<Zm>().bots[&n].path.is_empty();
            if (moved || (empty && far)) && now >= replan_at {
                let route = plan(world, origin, g.pos);
                let mut zm = world.resource_mut::<Zm>();
                let b = zm.bots.get_mut(&n).unwrap();
                b.path = route;
                b.path_to = Some(g.pos);
                b.replan_at = now + 500;
            }
        } else {
            let mut zm = world.resource_mut::<Zm>();
            let b = zm.bots.get_mut(&n).unwrap();
            b.path.clear();
            b.path_to = None;
        }
        let throw = throws.contains(&n);
        // Something at step height between it and the next route point
        // (a ledge, a railing, a low wall), as opposed to stairs or a slope
        // the walk steps up by itself: only then does a jump (and the
        // mantle it starts) help; jumping on stairs under a ceiling holds
        // a bot in place.
        let climb_blocked: Option<([f32; 3], bool)> = {
            let peek = {
                let zm = world.resource::<Zm>();
                let path = &zm.bots[&n].path;
                path.iter()
                    .find(|q| dist2d(origin, **q) >= 24.0)
                    .or(path.last())
                    .copied()
            };
            peek.map(|p| (p, dist2d(origin, p) < 160.0 && blocked_ahead(world, origin, p)))
        };
        // Blocked at step height toward the next point: going up (a ledge)
        // it jumps at once; level or down (a railing it drops over, a low
        // fence) it jumps once it makes no headway.
        let ahead_blocked = climb_blocked.is_some_and(|(_, b)| b);
        let climb_blocked = climb_blocked
            .is_some_and(|(p, b)| b && p[2] - origin[2] > 18.0 && dist2d(origin, p) < 96.0);
        let mut zm = world.resource_mut::<Zm>();
        let b = zm.bots.get_mut(&n).unwrap();
        b.tick = b.tick.wrapping_add(1);
        while b.path.len() > 1 && dist2d(origin, b.path[0]) < 24.0 {
            b.path.remove(0);
        }
        let at_goal = goal
            .as_ref()
            .is_none_or(|g| dist2d(origin, g.pos) <= g.radius.max(16.0));
        let next = (!at_goal).then(|| b.path.first().copied()).flatten();
        // The view: what it looks at, else down its route, else as it is.
        let eye = [origin[0], origin[1], origin[2] + height];
        let target = b.look.or_else(|| next.map(|p| [p[0], p[1], p[2] + height]));
        let mut want = view;
        if let Some(t) = target {
            let (yaw, pitch) = angles_to(eye, t);
            want[1] = view[1] + angle_delta(view[1], yaw).clamp(-turn, turn);
            want[0] = view[0] + angle_delta(view[0], pitch).clamp(-turn, turn);
        }
        // Moving toward the next point, relative to where it faces.
        let (mut forward, mut right) = (0i8, 0i8);
        if let Some(p) = next {
            let wish = [p[0] - origin[0], p[1] - origin[1]];
            let len = (wish[0] * wish[0] + wish[1] * wish[1]).sqrt().max(1e-3);
            let (s, c) = want[1].to_radians().sin_cos();
            forward = (((wish[0] * c + wish[1] * s) / len) * 127.0).round() as i8;
            right = (((wish[0] * s - wish[1] * c) / len) * 127.0).round() as i8;
        }
        let mut press = 0u32;
        // The trigger: held while allowed, let go one tick in four so a
        // semi-automatic fires again.
        if (b.allow_attack && b.tick % 4 != 0) || b.attack_ticks > 0 {
            press |= buttons::ATTACK;
        }
        b.attack_ticks = b.attack_ticks.saturating_sub(1);
        if b.ads {
            press |= buttons::ADS;
        }
        if b.use_until > now {
            press |= buttons::USE_RELOAD;
        }
        if std::mem::take(&mut b.melee) {
            press |= buttons::MELEE_CHARGE;
        }
        if std::mem::take(&mut b.dive) {
            press |= buttons::PRONE;
        }
        match b.stance {
            1 => press |= buttons::CROUCH,
            2 => press |= buttons::PRONE,
            _ => {}
        }
        if throw {
            press |= buttons::FRAG;
        }
        // Jumps and vaults: BO2's engine takes a bot over the negotiation
        // links its path nodes mark (ledges, railings, low walls); ours
        // jumps where the route climbs close ahead (the movement code
        // mantles from a jump), and when walking makes no headway.
        // On a ladder it climbs (looking up its route, walking forward):
        // a jump would let go of it.
        if next.is_some() && !ladder && climb_blocked {
            press |= buttons::JUMP;
        }
        if next.is_some() {
            match b.progress {
                Some((at, since)) if dist2d(at, origin) < 12.0 => {
                    let stuck = now - since;
                    if stuck > 600 && (stuck / 300) % 2 == 0 && !ladder && ahead_blocked {
                        press |= buttons::JUMP;
                    }
                    // No headway: back off and step aside for a moment
                    // (a doorway corner, another player, a prop), then on.
                    if (1200..2400).contains(&stuck) && !ladder {
                        forward = -127;
                        right = if (n + (since / 3000) as u32) % 2 == 0 { 127 } else { -127 };
                    }
                    if std::env::var_os("BO2MP_BOTLOG").is_some() && stuck % 1000 == 0 && stuck > 0 {
                        diag::info!(
                            Sim,
                            "bo2mp bot {n} stuck {stuck} ms at {:.0} {:.0} {:.0} next {:?} ahead_blocked {ahead_blocked}",
                            origin[0],
                            origin[1],
                            origin[2],
                            next.map(|p| [p[0] as i32, p[1] as i32, p[2] as i32])
                        );
                    }
                    // Still stuck: drop the route; the next tick plans again.
                    if stuck > 3000 {
                        b.path.clear();
                        b.path_to = None;
                        b.replan_at = now + 250;
                        b.progress = Some((origin, now));
                    }
                }
                _ => b.progress = Some((origin, now)),
            }
        } else {
            b.progress = None;
        }
        let far = goal
            .as_ref()
            .is_some_and(|g| dist2d(origin, g.pos) > 384.0);
        if next.is_some() && far && b.look.is_none() && !b.no_sprint && !b.ads && b.stance == 0 {
            press |= buttons::SPRINT;
        }
        // BO2MP_BOTLOG=1: each bot every 2 s (test aid).
        if std::env::var_os("BO2MP_BOTLOG").is_some() && now % 2000 == 0 {
            diag::info!(
                Sim,
                "bo2mp botlog {n}: at {:.0} {:.0} {:.0} view {:.0} {:.0} goal {:?} path {} look {:?} attack {} ads {} press {press:#x} move {forward} {right}",
                origin[0],
                origin[1],
                origin[2],
                view[0],
                view[1],
                goal.as_ref().map(|g| g.name.as_str()),
                b.path.len(),
                b.look.map(|l| [l[0] as i32, l[1] as i32, l[2] as i32]),
                b.allow_attack,
                b.ads,
            );
        }
        drop(zm);
        // BO2MP_BOTS_STILL=1: the bots stand where they are and do nothing;
        // =fire:N (or fire:N,M) and those bots fire where they face too
        // (test aids, for a target that stays put and a shooter that does).
        if let Some(still) = std::env::var_os("BO2MP_BOTS_STILL") {
            let firing = still
                .to_str()
                .and_then(|s| s.strip_prefix("fire:"))
                .is_some_and(|ids| ids.split(',').any(|i| i.trim().parse::<u32>() == Ok(n)));
            let attack = if firing && (now / 100) % 2 == 0 { buttons::ATTACK } else { 0 };
            (forward, right, press, want) = (0, 0, attack, view);
        }
        // Its command for the tick, the view turned as a script's
        // setplayerangles turns it (the delta under the command angles).
        let mut cmd_angles = None;
        {
            let mut req = world.resource_mut::<crate::step::StepRequest>();
            for (id, cmd) in &mut req.input.cmds {
                if id.0 != n {
                    continue;
                }
                cmd.buttons = press;
                cmd.forwardmove = forward;
                cmd.rightmove = right;
                cmd_angles = Some(cmd.angles);
            }
        }
        if let Some(angles) = cmd_angles {
            let mut f = frame(world);
            if let Some(p) = f.player_mut(ClientId(n)) {
                p.delta_angles = std::array::from_fn(|i| {
                    let cmd_deg = (angles[i] as u16) as f32 * (360.0 / 65536.0);
                    want[i] - cmd_deg
                });
                p.viewangles = want;
            }
        }
    }
}

/// A route from a place to a goal over the path nodes (the goal last);
/// straight there when the map has no route between them.
fn plan(world: &mut World, from: [f32; 3], to: [f32; 3]) -> Vec<[f32; 3]> {
    let zm = world.resource::<Zm>();
    let nav = &zm.nav;
    let (Some(a), Some(b)) = (nav.nearest(from), nav.nearest(to)) else {
        return vec![to];
    };
    let Some(nodes) = nav.path(a, b) else {
        return vec![to];
    };
    let mut out: Vec<[f32; 3]> = nodes
        .iter()
        .map(|&i| nav.nodes[i as usize].origin)
        .collect();
    // Skip the first node when the second is nearer (already past it).
    if out.len() > 1 && dist2d(from, out[1]) < dist2d(out[0], out[1]) {
        out.remove(0);
    }
    out.push(to);
    out
}
