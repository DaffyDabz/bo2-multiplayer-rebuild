//! bo2mp: guns and scavenger bags on the ground. BO2's `_weapons.gsc`
//! drops a dead player's gun (`dropitem`) and, for a killer with Scavenger,
//! a bag (`dropscavengeritem`); the engine's own dropped items
//! (`crate::item`: fall, touch for ammo, hold use to swap, bags for
//! Scavenger only) carry them. An item's script entity hears `trigger`
//! (player, the gun he swapped out) or `scavenger` (player) when picked up,
//! then dies, as BO2's `watchpickup` and `scavenger_think` wait for.

use bevy_ecs::prelude::World;
use gsc_t6::{ObjKind, ObjRef, Value, Vm};
use playerstate_iw4::ENTITYNUM_NONE;

use super::{Ent, Zm, arg, client, entnum, frame, text, tick, weapon, with_vm};

const WEAPON_ITEM: &str = "weapon_item";
const SCAVENGER_ITEM: &str = "scavenger_item";

/// A script entity for the engine's dropped item `number`.
fn item_entity(vm: &mut Vm<World>, world: &mut World, number: i32, classname: &str) -> Value {
    let origin = frame(world)
        .dropped_item_by_number(number)
        .map_or([0.0; 3], |i| i.origin);
    let n = world.resource_mut::<Zm>().alloc_entnum();
    let obj = vm.alloc_object(ObjKind::Entity(n));
    world.resource_mut::<Zm>().ents.insert(
        n,
        Ent {
            obj: Some(obj),
            classname: classname.to_owned(),
            origin,
            item: Some(number),
            ..Default::default()
        },
    );
    Value::Object(obj)
}

/// The engine's item behind a script entity.
fn item_number(vm: &Vm<World>, world: &World, v: &Value) -> Option<i32> {
    let n = entnum(vm, v)?;
    world.resource::<Zm>().ents.get(&n)?.item
}

/// The engine's item goes with its script entity (`delete`).
pub(super) fn forget(world: &mut World, number: i32) {
    let mut f = frame(world);
    if f.remove_dropped_item_by_number(number).is_some() {
        f.free_dynamic_entity_number(number);
    }
}

pub(super) fn bind(vm: &mut Vm<World>) {
    macro_rules! m {
        ($name:literal, $body:expr) => {
            vm.bind($name, true, $body);
        };
    }
    m!("dropitem", |vm, world, s, a| {
        let id = client(vm, world, s)?;
        let w = weapon(world, &text(vm, a, 0))?;
        if w == 0 {
            return Ok(Value::Undefined);
        }
        let t = tick(world);
        match crate::item::drop_weapon(&mut frame(world), t, id, w) {
            Some(number) => {
                diag::info!(Sim, "bo2mp item {number}: player {} dropped {}", id.0, text(vm, a, 0));
                Ok(item_entity(vm, world, number, WEAPON_ITEM))
            }
            None => Ok(Value::Undefined),
        }
    });
    // The bag is BO2's `scavenger_item_mp` (its world model is the bag).
    m!("dropscavengeritem", |vm, world, s, a| {
        let id = client(vm, world, s)?;
        let name = text(vm, a, 0);
        let w = weapon(world, &name).unwrap_or(0);
        if w == 0 {
            diag::warn!(Sim, "bo2mp: no weapon {name} for the scavenger bag");
            return Ok(Value::Undefined);
        }
        let t = tick(world);
        match crate::item::drop_scavenger_item(&mut frame(world), t, id, w) {
            Some(number) => {
                diag::info!(Sim, "bo2mp item {number}: player {} dropped a scavenger bag", id.0);
                Ok(item_entity(vm, world, number, SCAVENGER_ITEM))
            }
            None => Ok(Value::Undefined),
        }
    });
    m!("itemweaponsetammo", |vm, world, s, a| {
        let Some(number) = item_number(vm, world, s) else {
            return Ok(Value::Undefined);
        };
        let clip = arg(a, 0).as_int().unwrap_or(0);
        let stock = arg(a, 1).as_int().unwrap_or(0);
        let clip_l = arg(a, 2).as_int();
        if let Some(item) = frame(world).dropped_item_mut_by_number(number) {
            item.clip_r = clip.max(0);
            item.stock = stock.max(0);
            if let Some(l) = clip_l {
                item.clip_l = l.max(0);
            }
        }
        Ok(Value::Undefined)
    });
    m!("getitemweaponname", |vm, world, s, _| {
        let Some(number) = item_number(vm, world, s) else {
            return Ok(vm.string("none"));
        };
        let f = frame(world);
        let w = f
            .dropped_item_by_number(number)
            .map_or(0, |i| u32::try_from(i.state.index).unwrap_or(0));
        let name = crate::script_player::weapon_name(&f, w);
        Ok(vm.string(&name))
    });
}

/// After the engine's touch phase: keep this tick's pickups for the
/// scripts' next frame ([`settle`]).
pub(crate) fn collect_pickups(world: &mut World) {
    if !world.contains_resource::<Zm>() {
        return;
    }
    let pickups = frame(world).item_pickups_mut().clone();
    if !pickups.is_empty() {
        world.resource_mut::<Zm>().item_pickups.extend(pickups);
    }
}

/// Items follow the engine's: picked-up ones tell the scripts and die,
/// the rest keep its origin (they fall).
pub(super) fn settle(world: &mut World) {
    let pickups = std::mem::take(&mut world.resource_mut::<Zm>().item_pickups);
    let items: Vec<(u32, i32, ObjRef, bool)> = world
        .resource::<Zm>()
        .ents
        .iter()
        .filter_map(|(n, e)| Some((*n, e.item?, e.obj?, e.classname == SCAVENGER_ITEM)))
        .collect();
    if items.is_empty() {
        return;
    }
    let mut gone = Vec::new();
    {
        let mut moved = Vec::new();
        let f = frame(world);
        for (n, number, obj, _) in &items {
            match f.dropped_item_by_number(*number) {
                Some(i) => moved.push((*n, i.origin)),
                None => gone.push((*n, *obj)),
            }
        }
        let mut zm = world.resource_mut::<Zm>();
        for (n, o) in moved {
            if let Some(e) = zm.ents.get_mut(&n) {
                e.origin = o;
            }
        }
    }
    with_vm(world, |vm, world| {
        for p in pickups {
            let Some((_, _, obj, scavenger)) = items.iter().find(|i| i.1 == p.from_entnum) else {
                continue;
            };
            let Some(player) = world
                .resource::<Zm>()
                .players
                .get(&(p.picker as u32))
                .map(|x| x.obj)
            else {
                continue;
            };
            if !vm.alive(*obj) {
                continue;
            }
            diag::info!(
                Sim,
                "bo2mp item {}: player {} picked it up (swapped out item {})",
                p.from_entnum,
                p.picker,
                p.swapped_entnum
            );
            if *scavenger {
                vm.notify_str(world, *obj, "scavenger", &[Value::Object(player)]);
            } else {
                let swapped = if p.swapped_entnum == ENTITYNUM_NONE {
                    Value::Undefined
                } else {
                    item_entity(vm, world, p.swapped_entnum, WEAPON_ITEM)
                };
                vm.notify_str(world, *obj, "trigger", &[Value::Object(player), swapped]);
            }
        }
        for (n, obj) in gone {
            world.resource_mut::<Zm>().ents.remove(&n);
            vm.free_object(world, obj);
        }
    });
}
