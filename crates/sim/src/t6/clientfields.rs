//! Client fields (`setclientfield` -> `codesetclientfield`,
//! `codesetworldclientfield`): Black Ops II's client scripts turn them into
//! effects and sounds on each client. The engine plays their half here, as
//! those scripts do: a zombie's eye glow, the dirt as it rises, a
//! power-up's glow, a perk machine's trail from the sky, a wall buy's gun.

use bevy_ecs::prelude::World;
use gsc_t6::{ObjKind, Value, Vm};

use super::{Zm, arg, entnum, origin_of};
use crate::EventAudience;

/// Effects waiting their time (or their entity's first showing).
#[derive(Clone, Debug, Default)]
pub(crate) struct ClientFx {
    pending: Vec<Pending>,
}

#[derive(Clone, Debug)]
struct Pending {
    due: i64,
    /// Given up after this (the entity never showed).
    until: i64,
    ent: u32,
    step: Step,
}

#[derive(Clone, Debug)]
enum Step {
    /// An effect where the entity stands, offset.
    At { fx: &'static str, offset: [f32; 3] },
    /// An effect on one of the entity's tags (or taken off it).
    Bolt {
        fx: &'static str,
        tag: &'static str,
        stop: bool,
    },
}

// The client scripts' effects (clientscripts/mp/zombies/_zm.csc,
// _zm_powerups.csc, zm_nuked_fx.csc).
const RISE_BURST: &str = "maps/zombie/fx_mp_zombie_hand_dirt_burst";
const RISE_BILLOW: &str = "maps/zombie/fx_mp_zombie_body_dirt_billowing";
const RISE_DUST: &str = "maps/zombie/fx_mp_zombie_body_dust_falling";
const EYE_GLOW: &str = "misc/fx_zombie_eye_single";
const PERK_METEOR: &str = "maps/zombie/fx_zmb_trail_perk_meteor";
const POWERUP: [&str; 4] = [
    "misc/fx_zombie_powerup_on",
    "misc/fx_zombie_powerup_solo_on",
    "misc/fx_zombie_powerup_on_red",
    "misc/fx_zombie_powerup_caution_on",
];

fn queue(world: &mut World, ent: u32, after_ms: i64, step: Step) {
    let mut zm = world.resource_mut::<Zm>();
    let due = zm.now_ms + after_ms;
    zm.client_fx.pending.push(Pending {
        due,
        until: due + 2000,
        ent,
        step,
    });
}

fn rand_range(vm: &mut Vm<World>, lo: i32, hi: i32) -> f32 {
    (lo + (vm.rand_u32() % (hi - lo) as u32) as i32) as f32
}

/// An entity's field changed: its client script's effects.
fn entity_field(vm: &mut Vm<World>, world: &mut World, ent: &Value, name: &str, value: i64) {
    let Some(n) = entnum(vm, ent) else { return };
    match name {
        // handle_zombie_risers: a burst of dirt, a billow, then dust
        // falling off its back while it climbs out.
        "zombie_riser_fx" if value != 0 => {
            let origin = origin_of(vm, world, ent).unwrap_or([0.0; 3]);
            super::natives_fx::sound(world, EventAudience::All, "zmb_zombie_spawn", origin);
            let burst = [0.0, 0.0, rand_range(vm, 5, 10)];
            queue(
                world,
                n,
                0,
                Step::At {
                    fx: RISE_BURST,
                    offset: burst,
                },
            );
            let billow = [
                rand_range(vm, -10, 10),
                rand_range(vm, -10, 10),
                rand_range(vm, 5, 10),
            ];
            queue(
                world,
                n,
                250,
                Step::At {
                    fx: RISE_BILLOW,
                    offset: billow,
                },
            );
            let mut t = 0;
            while t < 5500 {
                queue(
                    world,
                    n,
                    2250 + t,
                    Step::Bolt {
                        fx: RISE_DUST,
                        tag: "j_spineupper",
                        stop: false,
                    },
                );
                t += 300;
            }
        }
        // createzombieeyes / deletezombieeyes.
        "zombie_has_eyes" => queue(
            world,
            n,
            0,
            Step::Bolt {
                fx: EYE_GLOW,
                tag: "j_eyeball_le",
                stop: value == 0,
            },
        ),
        // powerup_fx_callback: the glow by kind (no glow for 0).
        "powerup_fx" => {
            if let Some(fx) = usize::try_from(value - 1).ok().and_then(|i| POWERUP.get(i)) {
                queue(
                    world,
                    n,
                    0,
                    Step::Bolt {
                        fx,
                        tag: "tag_origin",
                        stop: false,
                    },
                );
            }
        }
        // perk_meteor_fx: the trail while the machine falls.
        "clientfield_perk_intro_fx" => queue(
            world,
            n,
            0,
            Step::Bolt {
                fx: PERK_METEOR,
                tag: "tag_origin",
                stop: value == 0,
            },
        ),
        _ => {}
    }
}

/// A player's own field (`setclientfieldtoplayer`): the perks and power-ups
/// his HUD shows go to his client as `bo2zm_icons` ("field:value,...").
fn player_field(world: &mut World, n: u32, name: &str, value: i32) {
    if !(name.starts_with("perk_") || name.starts_with("powerup_")) {
        return;
    }
    let list = {
        let mut zm = world.resource_mut::<Zm>();
        let Some(p) = zm.players.get_mut(&n) else {
            return;
        };
        let at = p.hud_fields.iter().position(|(k, _)| k == name);
        match (at, value) {
            (Some(i), 0) => {
                p.hud_fields.remove(i);
            }
            (Some(i), v) if p.hud_fields[i].1 != v => p.hud_fields[i].1 = v,
            (Some(_), _) => return,
            (None, 0) => return,
            (None, v) => p.hud_fields.push((name.to_owned(), v)),
        }
        p.hud_fields
            .iter()
            .map(|(k, v)| format!("{k}:{v}"))
            .collect::<Vec<_>>()
            .join(",")
    };
    super::set_client_dvar(world, n, "bo2zm_icons", &list);
}

pub(super) fn bind(vm: &mut Vm<World>) {
    // A script camera: camerasetposition(entity), cameraactivate(on);
    // camerasetlookat() keeps the camera entity's own angles.
    vm.bind("camerasetposition", true, |vm, world, s, a| {
        if let (Some(n), Some(cam)) = (entnum(vm, s), entnum(vm, arg(a, 0)))
            && let Some(p) = world.resource_mut::<Zm>().players.get_mut(&n)
        {
            p.camera = Some(cam);
        }
        Ok(Value::Undefined)
    });
    vm.bind("cameraactivate", true, |vm, world, s, a| {
        let on = !matches!(arg(a, 0), Value::Int(0)) && !arg(a, 0).is_undefined();
        if let Some(n) = entnum(vm, s)
            && let Some(p) = world.resource_mut::<Zm>().players.get_mut(&n)
        {
            p.camera_on = on;
        }
        Ok(Value::Undefined)
    });
    vm.bind("camerasetlookat", true, |_, _, _, _| Ok(Value::Undefined));
    // setclientuivisibilityflag("hud_visible", 0): his HUD hides (the
    // game-over shot); script HUD text still shows.
    vm.bind("setclientuivisibilityflag", true, |vm, world, s, a| {
        if vm.to_text(arg(a, 0)) == "hud_visible"
            && let Some(n) = entnum(vm, s)
        {
            let hidden = matches!(arg(a, 1), Value::Int(0));
            super::set_client_dvar(world, n, "bo2zm_hud_hidden", if hidden { "1" } else { "" });
        }
        Ok(Value::Undefined)
    });
    vm.bind("codesetplayerstateclientfield", false, |vm, world, _, a| {
        if let Some(n) = entnum(vm, arg(a, 0)) {
            let name = vm.to_text(arg(a, 1));
            let value = arg(a, 2).as_int().unwrap_or(0);
            player_field(world, n, &name, value);
        }
        Ok(Value::Undefined)
    });
    // maps/mp/_utility::setclientfield calls these.
    vm.bind("codesetworldclientfield", false, |vm, world, _, a| {
        let name = vm.to_text(arg(a, 0));
        let value = arg(a, 1).as_int().unwrap_or(0);
        super::wallbuys::world_field(vm, world, &name, value);
        Ok(Value::Undefined)
    });
    vm.bind("codesetclientfield", false, |vm, world, _, a| {
        let ent = arg(a, 0).clone();
        let name = vm.to_text(arg(a, 1));
        let value = arg(a, 2).as_int().map_or(0, i64::from);
        entity_field(vm, world, &ent, &name, value);
        Ok(Value::Undefined)
    });
    // Scripts that call the method directly.
    vm.bind("setclientfield", true, |vm, world, s, a| {
        let name = vm.to_text(arg(a, 0));
        let value = arg(a, 1).as_int().unwrap_or(0);
        if matches!(s, Value::Object(o) if matches!(vm.kind(*o), Some(ObjKind::Level))) {
            super::wallbuys::world_field(vm, world, &name, value);
        } else {
            entity_field(vm, world, s, &name, i64::from(value));
        }
        Ok(Value::Undefined)
    });
}

/// Each tick (after the entities show): the effects that are due.
pub(crate) fn advance(world: &mut World, now: i64) {
    let due: Vec<Pending> = {
        let mut zm = world.resource_mut::<Zm>();
        if zm.client_fx.pending.is_empty() {
            return;
        }
        let (due, keep) = std::mem::take(&mut zm.client_fx.pending)
            .into_iter()
            .partition(|p| p.due <= now);
        zm.client_fx.pending = keep;
        due
    };
    for p in due {
        let shown = world
            .resource::<Zm>()
            .presences
            .by_ent
            .get(&p.ent)
            .map(|s| s.origin);
        let Some(origin) = shown else {
            // Not drawn yet (spawned this tick): try again until it is.
            if now < p.until {
                world
                    .resource_mut::<Zm>()
                    .client_fx
                    .pending
                    .push(Pending { due: now + 50, ..p });
            }
            continue;
        };
        match p.step {
            Step::At { fx, offset } => {
                let at = [
                    origin[0] + offset[0],
                    origin[1] + offset[1],
                    origin[2] + offset[2],
                ];
                super::natives_fx::effect(world, fx, at, [0.0, 0.0, 1.0]);
            }
            Step::Bolt { fx, tag, stop } => {
                super::natives_fx::bolt_effect(world, p.ent, fx, tag, stop);
            }
        }
    }
}
