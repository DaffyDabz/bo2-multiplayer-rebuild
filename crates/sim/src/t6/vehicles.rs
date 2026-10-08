//! Script vehicles on paths: `attachpath(node)` then `startpath()` moves a
//! vehicle entity along its chain of `info_vehicle_node`s (each node's
//! `target` names the next) at each node's `speed` (miles per hour, 17.6
//! units a second each), turned along the way; `reached_end_node` at the
//! last node. Nuketown's perk machines ride `perk_arrival_vehicle` down
//! from the sky this way (linked to it).

use std::collections::BTreeMap;

use bevy_ecs::prelude::World;
use gsc_t6::{Value, Vm};

use super::{Zm, arg, entnum, with_vm};

const MPH: f32 = 17.6;

#[derive(Clone, Debug, Default)]
pub(crate) struct VPath {
    /// (point, speed units/s from it).
    pub points: Vec<([f32; 3], f32)>,
    /// bo2mp: each point's node entity (`reached_node` names it).
    pub nodes: Vec<u32>,
    pub seg: usize,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Vehicles {
    /// The node a vehicle's path starts at (`attachpath`).
    pub attached: BTreeMap<u32, u32>,
    pub moving: BTreeMap<u32, VPath>,
}

fn node_chain(vm: &mut Vm<World>, world: &World, start: u32) -> Vec<([f32; 3], f32, u32)> {
    let zm = world.resource::<Zm>();
    let (tn, tg, sp) = (
        vm.intern("targetname"),
        vm.intern("target"),
        vm.intern("speed"),
    );
    let nodes: Vec<(u32, [f32; 3], String, String, f32)> = zm
        .ents
        .iter()
        .filter(|(_, e)| e.classname.starts_with("info_vehicle_node"))
        .filter_map(|(n, e)| {
            let o = e.obj?;
            let name = vm.to_text(&vm.raw_field(o, tn));
            let target = vm.to_text(&vm.raw_field(o, tg));
            let speed = vm.raw_field(o, sp).as_float().unwrap_or(30.0);
            Some((*n, e.origin, name, target, speed))
        })
        .collect();
    let mut out = Vec::new();
    let mut at = nodes.iter().find(|n| n.0 == start);
    let mut seen = 0;
    while let Some(n) = at {
        out.push((n.1, n.4.max(1.0) * MPH, n.0));
        seen += 1;
        if n.3.is_empty() || seen > 256 {
            break;
        }
        at = nodes.iter().find(|m| m.2 == n.3);
    }
    out
}

/// Each tick: vehicles along their paths.
pub(crate) fn advance(world: &mut World, dt: f32) {
    let ids: Vec<u32> = world
        .resource::<Zm>()
        .vehicles
        .moving
        .keys()
        .copied()
        .collect();
    let mut arrived = Vec::new();
    // bo2mp: each node passed, as BO2's engine tells the vehicle (the
    // Warthog starts and stops its strafing runs by them).
    let mut passed: Vec<(u32, u32)> = Vec::new();
    for v in ids {
        let Some(mut path) = world.resource::<Zm>().vehicles.moving.get(&v).cloned() else {
            continue;
        };
        let Some(mut pos) = world.resource::<Zm>().ents.get(&v).map(|e| e.origin) else {
            world.resource_mut::<Zm>().vehicles.moving.remove(&v);
            continue;
        };
        let mut step = path.points.get(path.seg).map_or(0.0, |p| p.1) * dt;
        let mut yaw = None;
        while step > 0.0 {
            let Some(&(to, _)) = path.points.get(path.seg + 1) else {
                break;
            };
            let d = [to[0] - pos[0], to[1] - pos[1], to[2] - pos[2]];
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            if len > 1e-3 && (d[0].abs() + d[1].abs()) > 1e-3 {
                yaw = Some(d[1].atan2(d[0]).to_degrees());
            }
            if len <= step {
                pos = to;
                step -= len;
                path.seg += 1;
                if let Some(&node) = path.nodes.get(path.seg) {
                    passed.push((v, node));
                }
            } else {
                pos = std::array::from_fn(|i| pos[i] + d[i] / len * step);
                step = 0.0;
            }
        }
        let done = path.seg + 1 >= path.points.len();
        let mut zm = world.resource_mut::<Zm>();
        if let Some(e) = zm.ents.get_mut(&v) {
            e.origin = pos;
            if let Some(y) = yaw {
                e.angles = [0.0, y, 0.0];
            }
        }
        if done {
            zm.vehicles.moving.remove(&v);
            arrived.push(v);
        } else {
            zm.vehicles.moving.insert(v, path);
        }
    }
    if arrived.is_empty() && passed.is_empty() {
        return;
    }
    with_vm(world, |vm, world| {
        for (v, node) in passed {
            let zm = world.resource::<Zm>();
            let (Some(o), Some(n)) = (
                zm.ents.get(&v).and_then(|e| e.obj),
                zm.ents.get(&node).and_then(|e| e.obj),
            ) else {
                continue;
            };
            vm.notify_str(world, o, "reached_node", &[Value::Object(n)]);
        }
        for v in arrived {
            if let Some(o) = world.resource::<Zm>().ents.get(&v).and_then(|e| e.obj) {
                vm.notify_str(world, o, "reached_end_node", &[]);
            }
        }
    });
}

pub(super) fn bind(vm: &mut Vm<World>) {
    vm.bind("attachpath", true, |vm, world, s, a| {
        if let (Some(v), Some(node)) = (entnum(vm, s), entnum(vm, arg(a, 0))) {
            world.resource_mut::<Zm>().vehicles.attached.insert(v, node);
        }
        Ok(Value::Undefined)
    });
    vm.bind("startpath", true, |vm, world, s, _| {
        let Some(v) = entnum(vm, s) else {
            return Ok(Value::Undefined);
        };
        let Some(start) = world.resource::<Zm>().vehicles.attached.get(&v).copied() else {
            return Ok(Value::Undefined);
        };
        let chain = node_chain(vm, world, start);
        let points: Vec<([f32; 3], f32)> = chain.iter().map(|(p, s, _)| (*p, *s)).collect();
        let nodes: Vec<u32> = chain.iter().map(|(_, _, n)| *n).collect();
        if let Some(&(first, _)) = points.first() {
            let mut zm = world.resource_mut::<Zm>();
            if let Some(e) = zm.ents.get_mut(&v) {
                e.origin = first;
            }
            zm.vehicles.moving.insert(
                v,
                VPath {
                    points,
                    nodes,
                    seg: 0,
                },
            );
        }
        Ok(Value::Undefined)
    });
    for name in [
        "setspeed",
        "setspeedimmediate",
        "resumespeed",
        "setvehgoalpos",
        "vehicle_detachfrompath",
    ] {
        vm.bind(name, true, |_, _, _, _| Ok(Value::Undefined));
    }
}
