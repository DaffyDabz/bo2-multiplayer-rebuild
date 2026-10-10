//! Engine-owned entity fields (`self.origin`, a player's `health`, ...).

use bevy_ecs::prelude::World;
use gsc_t6::{ObjKind, ObjRef, Str, Value, Vm};

use super::{Zm, frame};
use crate::world::ClientId;

/// A player's scoreboard counts: engine fields, 0 until set (the scripts
/// count `attacker.headshots++` from nothing).
const STATS: &[&str] = &[
    "headshots",
    "assists",
    "downs",
    "revives",
    "plants",
    "defuses",
    "returns",
    "captures",
    "destructions",
    "survived",
    "stabs",
    "tomahawks",
    "humiliated",
    "x2kills",
    "agrkills",
    "hacks",
    "momentum",
    "objtime",
];

pub(super) fn get(
    vm: &mut Vm<World>,
    world: &mut World,
    _obj: ObjRef,
    kind: ObjKind,
    field: Str,
) -> Option<Value> {
    let ObjKind::Entity(n) = kind else {
        return None;
    };
    let name = vm.str(field);
    let player = world
        .resource::<Zm>()
        .players
        .get(&n)
        .map(|p| p.sessionstate.clone());
    if let Some(sessionstate) = player {
        let id = ClientId(n);
        return match name {
            "origin" => Some(Value::Vec3(
                frame(world).player(id).map_or([0.0; 3], |ps| ps.origin),
            )),
            "angles" => Some(Value::Vec3(
                frame(world).player(id).map_or([0.0; 3], |ps| ps.viewangles),
            )),
            "health" => Some(Value::Int(
                frame(world).player(id).map_or(0, |ps| ps.health),
            )),
            "maxhealth" => {
                let f = frame(world);
                let meta = f.client_meta(id).map_or(0, |m| m.max_health);
                Some(Value::Int(if meta > 0 {
                    meta
                } else {
                    f.player(id).map_or(100, |ps| ps.max_health)
                }))
            }
            "score" => Some(Value::Int(
                frame(world).client_meta(id).map_or(0, |m| m.score),
            )),
            "kills" => Some(Value::Int(
                frame(world).client_meta(id).map_or(0, |m| m.kills),
            )),
            "deaths" => Some(Value::Int(
                frame(world).client_meta(id).map_or(0, |m| m.deaths),
            )),
            "sessionstate" => Some(vm.string(&sessionstate)),
            "classname" => Some(vm.string("player")),
            "name" => {
                let n = frame(world)
                    .client_meta(id)
                    .map(|m| {
                        let end = m.name.iter().position(|&b| b == 0).unwrap_or(m.name.len());
                        String::from_utf8_lossy(&m.name[..end]).into_owned()
                    })
                    .unwrap_or_else(|| "Player".to_owned());
                Some(vm.string(&n))
            }
            "velocity" => Some(Value::Vec3(
                frame(world).player(id).map_or([0.0; 3], |ps| ps.velocity),
            )),
            // 1 from a dive's take-off until its slide ends.
            "divetoprone" => Some(Value::Int(
                frame(world)
                    .player(id)
                    .map_or(0, |ps| movement_iw4::dive_to_prone(ps) as i32),
            )),
            s if STATS.contains(&s) => Some(Value::Int(
                world
                    .resource::<Zm>()
                    .players
                    .get(&n)
                    .and_then(|p| p.stats.get(s).copied())
                    .unwrap_or(0),
            )),
            _ => None,
        };
    }
    let zm = world.resource::<Zm>();
    if let Some(a) = zm.actors.get(&n) {
        match name {
            "health" => return Some(Value::Int(a.health)),
            "maxhealth" => return Some(Value::Int(a.maxhealth)),
            "enemy" => return Some(a.enemy.map_or(Value::Undefined, Value::Object)),
            // bo2mp: where its path leads next (a dog runs only on a
            // straight stretch: `dog_move::shouldrun`).
            "lookaheaddist" | "lookaheaddir" => {
                let me = zm.ents.get(&n).map_or([0.0; 3], |e| e.origin);
                let next = a.path.get(a.path_i).copied().unwrap_or(me);
                let d = [next[0] - me[0], next[1] - me[1], 0.0];
                let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
                return Some(if name == "lookaheaddist" {
                    Value::Float(len)
                } else if len > 1e-3 {
                    Value::Vec3([d[0] / len, d[1] / len, 0.0])
                } else {
                    let (s, c) = zm
                        .ents
                        .get(&n)
                        .map_or(0.0, |e| e.angles[1])
                        .to_radians()
                        .sin_cos();
                    Value::Vec3([c, s, 0.0])
                });
            }
            _ => {}
        }
    }
    let e = zm.ents.get(&n)?;
    match name {
        "origin" => Some(Value::Vec3(e.origin)),
        "angles" => Some(Value::Vec3(e.angles)),
        "model" => {
            let m = e.model.clone();
            Some(vm.string(&m))
        }
        "classname" => {
            let c = e.classname.clone();
            Some(vm.string(&c))
        }
        _ => None,
    }
}

pub(super) fn set(
    vm: &mut Vm<World>,
    world: &mut World,
    _obj: ObjRef,
    kind: ObjKind,
    field: Str,
    value: &Value,
) -> bool {
    let ObjKind::Entity(n) = kind else {
        return false;
    };
    let name = vm.str(field).to_owned();
    let is_player = world.resource::<Zm>().players.contains_key(&n);
    if is_player {
        let id = ClientId(n);
        let int = |v: &Value| v.as_int().unwrap_or(0);
        match name.as_str() {
            "origin" => {
                if let Some(v) = value.as_vec3() {
                    super::teleport_player(world, id, v);
                }
            }
            "angles" => {
                if let Some(v) = value.as_vec3() {
                    super::set_player_view(world, id, v);
                }
            }
            "health" => {
                if let Some(ps) = frame(world).player_mut(id) {
                    ps.health = int(value);
                }
            }
            "maxhealth" => {
                let m = int(value).max(1);
                let mut f = frame(world);
                f.client_meta_mut(id).max_health = m;
                if let Some(ps) = f.player_mut(id) {
                    ps.max_health = m;
                    ps.health = ps.health.min(m);
                }
            }
            "score" | "kills" | "deaths" => {
                let v = int(value);
                let mut f = frame(world);
                if f.client_meta(id).is_some() {
                    let meta = f.client_meta_mut(id);
                    match name.as_str() {
                        "score" => meta.score = v,
                        "kills" => meta.kills = v,
                        _ => meta.deaths = v,
                    }
                }
            }
            s if STATS.contains(&s) => {
                let v = int(value);
                if let Some(p) = world.resource_mut::<Zm>().players.get_mut(&n) {
                    p.stats.insert(name.clone(), v);
                }
            }
            "sessionstate" => {
                let s = vm.to_text(value);
                if let Some(p) = world.resource_mut::<Zm>().players.get_mut(&n) {
                    // Out of play (game over): no longer down and crawling.
                    if s != "playing" {
                        p.laststand = false;
                    }
                    p.sessionstate = s;
                }
            }
            "sessionteam" => {
                let team = match vm.to_text(value).as_str() {
                    "axis" => entity_iw4::TEAM_AXIS,
                    "allies" => entity_iw4::TEAM_ALLIES,
                    "spectator" => entity_iw4::TEAM_SPECTATOR,
                    _ => entity_iw4::TEAM_FREE,
                };
                let mut f = frame(world);
                if f.client_meta(id).is_some() {
                    f.client_meta_mut(id).client_state_team = team;
                }
                return false;
            }
            _ => return false,
        }
        return true;
    }
    let mut zm = world.resource_mut::<Zm>();
    if let Some(a) = zm.actors.get_mut(&n) {
        match name.as_str() {
            "health" => {
                a.health = value.as_int().unwrap_or(0);
                return true;
            }
            "maxhealth" => {
                a.maxhealth = value.as_int().unwrap_or(0);
                return true;
            }
            _ => {}
        }
    }
    let Some(e) = zm.ents.get_mut(&n) else {
        return false;
    };
    match name.as_str() {
        "origin" => {
            if let Some(v) = value.as_vec3() {
                e.origin = v;
            }
            zm.movers.stop(n, true, false);
            true
        }
        "angles" => {
            if let Some(v) = value.as_vec3() {
                e.angles = v;
            }
            zm.movers.stop(n, false, true);
            true
        }
        "model" | "classname" => true,
        _ => false,
    }
}
