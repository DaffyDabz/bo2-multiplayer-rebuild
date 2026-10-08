//! Wall buys as the player sees them, which BO2 draws from its client
//! scripts (clientscripts/mp/zombies/_zm_weapons.csc), done here on the
//! server: each wall buy's chalk effect (`level._effect[<weapon>_fx]`, the
//! M14's by default) plays at its struct once the player is in; when the
//! scripts flip its world clientfield (`<weapon>_<origin>` = 1, bought)
//! the gun's model (the target struct's) slides out of the wall to its
//! place over a second.

use std::collections::HashMap;

use bevy_ecs::prelude::World;
use gsc_t6::{ObjKind, ObjRef, Value, Vm};

use super::{Ent, Zm, with_vm};

#[derive(Clone, Debug, Default)]
pub(crate) struct WallBuys {
    pub chalk_done: bool,
    /// World clientfields the scripts set (name -> value).
    pub world_fields: HashMap<String, i32>,
    /// Wall buys whose gun is out (by clientfield name).
    pub shown: HashMap<String, u32>,
}

fn field(vm: &mut Vm<World>, o: ObjRef, name: &str) -> Value {
    let f = vm.intern(name);
    vm.raw_field(o, f)
}

/// The struct kinds the server's `init_spawnable_weapon_upgrade` gathers as
/// wall buys: guns and grenades, the Bowie knife, the sickle, Galvaknuckles,
/// built wall buys and Claymores (Nuketown has all but the sickle and the
/// built ones).
fn is_wallbuy(targetname: &str) -> bool {
    matches!(
        targetname,
        "weapon_upgrade"
            | "bowie_upgrade"
            | "sickle_upgrade"
            | "tazer_upgrade"
            | "buildable_wallbuy"
            | "claymore_purchase"
    )
}

/// Every map struct (level.struct).
fn structs(vm: &mut Vm<World>) -> Vec<ObjRef> {
    let all = field(vm, vm.level, "struct");
    let Value::Array(arr) = all else {
        return Vec::new();
    };
    arr.snapshot()
        .values_in_order()
        .filter_map(|v| match v {
            Value::Object(o) => Some(*o),
            _ => None,
        })
        .collect()
}

/// Each tick: the chalk once a player has begun (and a moment after, so
/// his client is listening).
pub(crate) fn advance(world: &mut World, now: i64) {
    if world.resource::<Zm>().wallbuys.chalk_done {
        return;
    }
    let begun = world.resource::<Zm>().players.values().any(|p| p.begun);
    if !begun {
        return;
    }
    let since = {
        let mut zm = world.resource_mut::<Zm>();
        let t = *zm.wallbuy_since.get_or_insert(now);
        now - t
    };
    if since < 1500 {
        return;
    }
    world.resource_mut::<Zm>().wallbuys.chalk_done = true;
    let effects: Vec<(i32, [f32; 3], [f32; 3], [f32; 3])> = with_vm(world, |vm, _| {
        let fx_table = field(vm, vm.level, "_effect");
        let fx_of = |vm: &mut Vm<World>, key: &str| -> Option<i32> {
            let Value::Array(a) = &fx_table else {
                return None;
            };
            let k = gsc_t6::Key::Str(vm.intern(key));
            a.get(&k).and_then(|v| v.as_int())
        };
        let mut out = Vec::new();
        for s in structs(vm) {
            let tn = field(vm, s, "targetname");
            if !is_wallbuy(&vm.to_text(&tn)) {
                continue;
            }
            let w = field(vm, s, "zombie_weapon_upgrade");
            let w = vm.to_text(&w);
            let Some(origin) = field(vm, s, "origin").as_vec3() else {
                continue;
            };
            let angles = field(vm, s, "angles").as_vec3().unwrap_or([0.0; 3]);
            let Some(fx) = fx_of(vm, &format!("{w}_fx")).or_else(|| fx_of(vm, "m14_zm_fx")) else {
                continue;
            };
            let (f, _, u) = gsc_t6::math::angle_vectors(angles);
            out.push((fx, origin, f, u));
        }
        out
    })
    .unwrap_or_default();
    for (fx, origin, forward, up) in effects {
        if let Some(name) = super::natives_fx::fx_name(world, fx) {
            super::natives_fx::effect_axis(world, &name, origin, forward, up);
        }
    }
}

/// A world clientfield changed: a wall buy bought shows its gun.
pub(super) fn world_field(vm: &mut Vm<World>, world: &mut World, name: &str, value: i32) {
    let old = world
        .resource_mut::<Zm>()
        .wallbuys
        .world_fields
        .insert(name.to_owned(), value);
    if old == Some(value) || value != 1 || world.resource::<Zm>().wallbuys.shown.contains_key(name)
    {
        return;
    }
    // The wall buy this field belongs to: `<weapon>_<origin>`.
    let mut found = None;
    for s in structs(vm) {
        let tn = field(vm, s, "targetname");
        if !is_wallbuy(&vm.to_text(&tn)) {
            continue;
        }
        let w = field(vm, s, "zombie_weapon_upgrade");
        let o = field(vm, s, "origin");
        if format!("{}_{}", vm.to_text(&w), vm.to_text(&o)) == name {
            let target = field(vm, s, "target");
            found = Some(vm.to_text(&target));
            break;
        }
    }
    let Some(target) = found else { return };
    let mut model_at = None;
    for s in structs(vm) {
        let tn = field(vm, s, "targetname");
        if vm.to_text(&tn) != target {
            continue;
        }
        let m = field(vm, s, "model");
        let (Some(origin), angles) = (
            field(vm, s, "origin").as_vec3(),
            field(vm, s, "angles").as_vec3().unwrap_or([0.0; 3]),
        ) else {
            continue;
        };
        model_at = Some((vm.to_text(&m), origin, angles));
        break;
    }
    let Some((model, origin, angles)) = model_at else {
        return;
    };
    // Out of the wall: from 8 units along its right to its place.
    let (_, right, _) = gsc_t6::math::angle_vectors(angles);
    let from = std::array::from_fn(|i| origin[i] + right[i] * 8.0);
    let now = world.resource::<Zm>().now_ms;
    let n = world.resource_mut::<Zm>().alloc_entnum();
    let obj = vm.alloc_object(ObjKind::Entity(n));
    let mut zm = world.resource_mut::<Zm>();
    zm.ents.insert(
        n,
        Ent {
            obj: Some(obj),
            classname: "script_model".into(),
            origin: from,
            angles,
            model,
            ..Default::default()
        },
    );
    zm.movers.move_to(n, from, origin, now, 1.0, 0.0, 0.0);
    zm.wallbuys.shown.insert(name.to_owned(), n);
}
