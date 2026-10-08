//! Sounds and effects the scripts play: entity events the client turns
//! into Black Ops II sound aliases and effects by name (the same events
//! MW2's scripts use).

use bevy_ecs::prelude::World;
use gsc_t6::{Key, ObjKind, Value, Vm};

use super::{Zm, arg, entnum, frame, int, origin_of, text, tick};
use crate::EventAudience;
use crate::world::ClientId;

fn event(
    world: &mut World,
    audience: EventAudience,
    kind: entity_iw4::EntityEventKind,
    parm: u8,
    origin: [f32; 3],
    dir: [f32; 3],
) {
    let t = tick(world);
    frame(world).push_entity_event(
        t,
        audience,
        kind,
        crate::EntityEventPayload {
            number: i32::from(trace_iw4::ENTITYNUM_WORLD),
            event_parm: i32::from(parm),
            origin,
            direction: dir,
            ..Default::default()
        },
    );
}

pub(super) fn sound(world: &mut World, audience: EventAudience, alias: &str, origin: [f32; 3]) {
    if alias.is_empty() {
        return;
    }
    // bo2mp: BO2MP_SNDLOG=1 logs every sound the scripts play (test aid:
    // checked against the client's own sound starts).
    if std::env::var_os("BO2MP_SNDLOG").is_some() {
        diag::info!(Sim, "bo2mp sndlog play {alias} {audience:?}");
    }
    // Black Ops II's own aliases (the client looks names up by game).
    let index = frame(world).sound_alias_index(&format!("t6:{}", alias.to_ascii_lowercase()));
    event(
        world,
        audience,
        entity_iw4::EntityEventKind::SOUND_ALIAS,
        index,
        origin,
        [0.0; 3],
    );
}

/// An effect turned by both its forward and up (sent in origin2).
pub(super) fn effect_axis(
    world: &mut World,
    name: &str,
    origin: [f32; 3],
    forward: [f32; 3],
    up: [f32; 3],
) {
    if name.is_empty() {
        return;
    }
    let index = frame(world).effect_name_index(name);
    let t = tick(world);
    frame(world).push_entity_event(
        t,
        EventAudience::All,
        entity_iw4::EntityEventKind::PLAY_FX,
        crate::EntityEventPayload {
            number: i32::from(trace_iw4::ENTITYNUM_WORLD),
            event_parm: i32::from(index),
            origin,
            origin2: up,
            direction: forward,
            ..Default::default()
        },
    );
}

/// An effect that stays (`spawnfx` + `triggerfx`): a looping one goes on.
/// Without an up its client turns it by its forward alone.
pub(super) fn effect_held(
    world: &mut World,
    name: &str,
    origin: [f32; 3],
    forward: [f32; 3],
    up: Option<[f32; 3]>,
) {
    if name.is_empty() {
        return;
    }
    let index = frame(world).effect_name_index(name);
    let t = tick(world);
    frame(world).push_entity_event(
        t,
        EventAudience::All,
        entity_iw4::EntityEventKind::PLAY_FX,
        crate::EntityEventPayload {
            number: i32::from(trace_iw4::ENTITYNUM_WORLD),
            event_parm: i32::from(index),
            origin,
            origin2: up.unwrap_or([0.0; 3]),
            direction: forward,
            simulation_flags: 0x20,
            ..Default::default()
        },
    );
}

/// A field of a script object.
fn obj_field(vm: &mut Vm<World>, o: &Value, name: &str) -> Value {
    let Some(o) = o.as_obj() else {
        return Value::Undefined;
    };
    let f = vm.intern(name);
    vm.raw_field(o, f)
}

/// `activateclientexploder(id)`: the client's half of an exploder (Nuketown's
/// perk machines landing). The client scripts would play each createfx
/// exploder's effects; the server's own functions for them play them here.
fn client_exploder(vm: &mut Vm<World>, world: &mut World, id: &Value) {
    let level = Value::Object(vm.level);
    let ids = obj_field(vm, &level, "_exploder_ids");
    let Value::Array(ids) = ids else { return };
    let num = {
        let ids = ids.read();
        ids.keys()
            .find(|k| ids.get(k).and_then(Value::as_int) == id.as_int())
    };
    let Some(num) = num else { return };
    let Value::Array(all) = obj_field(vm, &level, "createfxexploders") else {
        return;
    };
    let Some(Value::Array(ents)) = all.get(&num) else {
        return;
    };
    let ents: Vec<Value> = ents.read().values_in_order().cloned().collect();
    for ent in ents {
        let Value::Array(v) = obj_field(vm, &ent, "v") else {
            continue;
        };
        let has = |vm: &mut Vm<World>, k: &str| {
            let key = Key::Str(vm.intern(k));
            v.get(&key).filter(|x| !x.is_undefined())
        };
        let mut run = |vm: &mut Vm<World>, world: &mut World, f: &str| {
            vm.spawn_named(world, "maps/mp/_utility", f, ent.clone(), Vec::new());
        };
        if has(vm, "firefx").is_some() {
            run(vm, world, "fire_effect");
        }
        let fxid = has(vm, "fxid").map(|x| vm.to_text(&x));
        if fxid.is_some_and(|f| f != "No FX") {
            run(vm, world, "cannon_effect");
        } else if has(vm, "soundalias").is_some() {
            run(vm, world, "sound_effect");
        }
    }
}

pub(super) fn effect(world: &mut World, name: &str, origin: [f32; 3], forward: [f32; 3]) {
    if name.is_empty() {
        return;
    }
    let index = frame(world).effect_name_index(name);
    event(
        world,
        EventAudience::All,
        entity_iw4::EntityEventKind::PLAY_FX,
        index,
        origin,
        forward,
    );
}

/// The effect a `loadfx` id names.
pub(super) fn fx_name(world: &World, id: i32) -> Option<String> {
    let zm = world.resource::<Zm>();
    zm.fx.get(usize::try_from(id - 1).ok()?).cloned()
}

fn player_audience(vm: &Vm<World>, world: &World, v: &Value) -> Option<EventAudience> {
    let n = entnum(vm, v)?;
    world
        .resource::<Zm>()
        .players
        .contains_key(&n)
        .then_some(EventAudience::Client(ClientId(n)))
}

/// An effect bolted to entity `n`'s tag (`playfxontag`'s engine half) or,
/// with `stop`, taken off it; false when the entity shows no model.
pub(super) fn bolt_effect(world: &mut World, n: u32, name: &str, tag: &str, stop: bool) -> bool {
    let Some((number, model, angles, origin)) = ({
        let zm = world.resource::<Zm>();
        zm.presences
            .by_ent
            .get(&n)
            .map(|s| (s.number, s.model.clone(), s.angles, s.origin))
    }) else {
        return false;
    };
    let attached = world
        .resource::<Zm>()
        .ents
        .get(&n)
        .map(|e| e.attached.clone())
        .unwrap_or_default();
    let bone = composition_bone(world, &model, &attached, tag).unwrap_or(0);
    // IW4L_T6_BOLTLOG=1: each effect bolted to a bone, and where.
    if std::env::var_os("IW4L_T6_BOLTLOG").is_some() {
        diag::info!(
            Sim,
            "bo2zm t6 bolt: {name} on ent{n} {model} {attached:?} tag {tag} -> bone {bone}{}",
            if stop { " (off)" } else { "" }
        );
    }
    let forward = gsc_t6::math::angle_vectors(angles).0;
    let index = frame(world).effect_name_index(name);
    let t = tick(world);
    frame(world).push_entity_event(
        t,
        EventAudience::All,
        entity_iw4::EntityEventKind::PLAY_FX,
        crate::EntityEventPayload {
            number,
            event_parm: i32::from(index),
            origin,
            direction: forward,
            surf_type: u8::try_from(bone).unwrap_or(0),
            simulation_flags: if stop { 0x80 } else { 0x40 },
            ..Default::default()
        },
    );
    true
}

/// A tag's bone index in an entity's composition (its model, then each
/// attached model, as its client numbers them).
fn composition_bone(
    world: &mut World,
    model: &str,
    attached: &[(String, String)],
    tag: &str,
) -> Option<usize> {
    let f = frame(world);
    let base = f.model_capability(model).flatten()?;
    let mut caps = vec![(base, None::<String>)];
    for (m, t) in attached {
        if let Some(c) = f.model_capability(m).flatten() {
            caps.push((c, Some(t.clone())));
        }
    }
    let models: Vec<(
        &xmodel_runtime::ModelPoseSrc,
        Option<xmodel_runtime::Attach>,
    )> = caps
        .iter()
        .map(|(c, t)| {
            let attach = t.as_ref().map(|t| xmodel_runtime::Attach {
                parent_model: 0,
                tag: if t.is_empty() {
                    xmodel_runtime::empty_tag_attach(&caps[0].0.pose, &c.pose)
                } else {
                    t.clone()
                },
            });
            (&c.pose, attach)
        })
        .collect();
    let dobj = xmodel_runtime::DObj::build(&models).ok()?;
    let want = tag.to_ascii_lowercase();
    dobj.bones
        .iter()
        .position(|b| b.name.eq_ignore_ascii_case(&want))
}

pub(super) fn bind(vm: &mut Vm<World>) {
    // Is there such a sound alias (the scripts count an alias' variants
    // with it: `vox_..._0`, `_1`, ...)?
    vm.bind("soundexists", false, |vm, world, _, a| {
        let alias = text(vm, a, 0).to_ascii_lowercase();
        Ok(Value::bool(
            world.resource::<Zm>().sound_aliases.contains(&alias),
        ))
    });
    vm.bind("playsound", true, |vm, world, s, a| {
        let alias = text(vm, a, 0);
        let origin = origin_of(vm, world, s).unwrap_or([0.0; 3]);
        sound(world, EventAudience::All, &alias, origin);
        // A notify name: sent when the sound would be done (lengths are not
        // known here yet: a second).
        if let (Value::Str(_), Some(o)) = (arg(a, 1), s.as_obj()) {
            let n = text(vm, a, 1);
            super::natives_game::notify_later(vm, world, o, &n, 1000);
        }
        Ok(Value::Undefined)
    });
    vm.bind("playsoundwithnotify", true, |vm, world, s, a| {
        let alias = text(vm, a, 0);
        let origin = origin_of(vm, world, s).unwrap_or([0.0; 3]);
        sound(world, EventAudience::All, &alias, origin);
        if let Some(o) = s.as_obj() {
            let n = text(vm, a, 1);
            super::natives_game::notify_later(vm, world, o, &n, 1000);
        }
        Ok(Value::Undefined)
    });
    vm.bind("playsoundontag", true, |vm, world, s, a| {
        let alias = text(vm, a, 0);
        let origin = origin_of(vm, world, s).unwrap_or([0.0; 3]);
        sound(world, EventAudience::All, &alias, origin);
        Ok(Value::Undefined)
    });
    vm.bind("playsoundasmaster", true, |vm, world, s, a| {
        let alias = text(vm, a, 0);
        let origin = origin_of(vm, world, s).unwrap_or([0.0; 3]);
        sound(world, EventAudience::All, &alias, origin);
        Ok(Value::Undefined)
    });
    vm.bind("playsoundtoplayer", true, |vm, world, s, a| {
        let alias = text(vm, a, 0);
        let who = arg(a, 1).clone();
        let Some(audience) = player_audience(vm, world, &who) else {
            return Ok(Value::Undefined);
        };
        let origin = origin_of(vm, world, s)
            .or_else(|| origin_of(vm, world, &who))
            .unwrap_or([0.0; 3]);
        sound(world, audience, &alias, origin);
        Ok(Value::Undefined)
    });
    vm.bind("playlocalsound", true, |vm, world, s, a| {
        let alias = text(vm, a, 0);
        let Some(audience) = player_audience(vm, world, s) else {
            return Ok(Value::Undefined);
        };
        let origin = origin_of(vm, world, s).unwrap_or([0.0; 3]);
        sound(world, audience, &alias, origin);
        Ok(Value::Undefined)
    });
    vm.bind("playsoundtoteam", true, |vm, world, s, a| {
        let alias = text(vm, a, 0);
        let origin = origin_of(vm, world, s).unwrap_or([0.0; 3]);
        sound(world, EventAudience::All, &alias, origin);
        Ok(Value::Undefined)
    });
    vm.bind("playsoundatposition", false, |vm, world, _, a| {
        let alias = text(vm, a, 0);
        let origin = arg(a, 1).as_vec3().unwrap_or([0.0; 3]);
        sound(world, EventAudience::All, &alias, origin);
        Ok(Value::Undefined)
    });
    vm.bind("playsound", false, |vm, world, _, a| {
        // playsound(localclientnum or 0, alias, origin) on the server: the
        // alias and where.
        let alias = text(vm, a, 1);
        let origin = arg(a, 2).as_vec3().unwrap_or([0.0; 3]);
        sound(world, EventAudience::All, &alias, origin);
        Ok(Value::Undefined)
    });
    vm.bind("playfx", false, |vm, world, _, a| {
        let Some(name) = int(a, 0).ok().and_then(|id| fx_name(world, id)) else {
            return Ok(Value::Undefined);
        };
        let origin = arg(a, 1).as_vec3().unwrap_or([0.0; 3]);
        let forward = arg(a, 2).as_vec3().unwrap_or([0.0, 0.0, 1.0]);
        match arg(a, 3).as_vec3() {
            Some(up) => effect_axis(world, &name, origin, forward, up),
            None => effect(world, &name, origin, forward),
        }
        let _ = vm;
        Ok(Value::Undefined)
    });
    // spawnfx(fx, origin, forward, up): an effect placed, started by
    // triggerfx.
    vm.bind("spawnfx", false, |vm, world, _, a| {
        let name = int(a, 0)
            .ok()
            .and_then(|id| fx_name(world, id))
            .unwrap_or_default();
        let origin = arg(a, 1).as_vec3().unwrap_or([0.0; 3]);
        let forward = arg(a, 2).as_vec3().unwrap_or([0.0, 0.0, 1.0]);
        let up = arg(a, 3).as_vec3();
        let n = world.resource_mut::<Zm>().alloc_entnum();
        let obj = vm.alloc_object(ObjKind::Entity(n));
        let mut zm = world.resource_mut::<Zm>();
        zm.ents.insert(
            n,
            super::Ent {
                obj: Some(obj),
                classname: "fx".into(),
                origin,
                ..Default::default()
            },
        );
        zm.fx_ents.insert(n, (name, origin, forward, up));
        Ok(Value::Object(obj))
    });
    vm.bind("triggerfx", false, |vm, world, _, a| {
        let Some(n) = entnum(vm, arg(a, 0)) else {
            return Ok(Value::Undefined);
        };
        let placed = world.resource::<Zm>().fx_ents.get(&n).cloned();
        if let Some((name, origin, forward, up)) = placed {
            effect_held(world, &name, origin, forward, up);
        }
        Ok(Value::Undefined)
    });
    vm.bind("activateclientexploder", false, |vm, world, _, a| {
        let id = arg(a, 0).clone();
        client_exploder(vm, world, &id);
        Ok(Value::Undefined)
    });
    vm.bind("playfxontag", false, |vm, world, _, a| {
        let Some(name) = int(a, 0).ok().and_then(|id| fx_name(world, id)) else {
            return Ok(Value::Undefined);
        };
        let who = arg(a, 1).clone();
        let tag = text(vm, a, 2);
        let origin = origin_of(vm, world, &who).unwrap_or([0.0; 3]);
        // Bolted to the entity's shown model: its client keeps the effect
        // on that tag while the entity lives.
        if let Some(n) = entnum(vm, &who)
            && bolt_effect(world, n, &name, &tag, false)
        {
            return Ok(Value::Undefined);
        }
        effect(world, &name, origin, [0.0, 0.0, 1.0]);
        Ok(Value::Undefined)
    });
}
