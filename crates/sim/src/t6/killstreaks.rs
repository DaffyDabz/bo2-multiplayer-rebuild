//! bo2mp lane D: the engine half of Black Ops II's scorestreaks.
//!
//! BO2's own scripts (`maps/mp/killstreaks/*`) run every scorestreak: score
//! fills a player's momentum, `_globallogic_score` hands him the streak's
//! weapon (`radar_mp`, `supplydrop_mp`, ...) with one in the clip, `_class`
//! puts his three streak weapons on the d-pad (`setactionslot`, cheapest on
//! the right), and raising one (`weapon_change`) calls it in
//! (`_killstreaks::killstreakwaiter`). The engine keeps what BO2's exe kept:
//! the teams' radar state (spy plane, satellite), the player's inventory
//! slot (a care package's streak), the d-pad slots his keys switch to, and
//! which entities lock-on launchers may target.

use std::collections::{BTreeMap, BTreeSet};

use bevy_ecs::prelude::{Resource, World};
use gsc_t6::{Value, Vm};

use super::{Zm, arg, entnum, frame, list, text};
use crate::world::ClientId;

mod craft;
mod ride;
mod turret;

/// Scorestreak state the engine keeps for the scripts.
#[derive(Resource, Default)]
pub(crate) struct Streaks {
    /// Radar by team: the spy plane's mode (0 off, 1 sweep, 2 fast sweep,
    /// `setteamspyplane`) and the satellite (`setteamsatellite`).
    pub spyplane: BTreeMap<String, i32>,
    pub satellite: BTreeMap<String, i32>,
    /// Each player's inventory weapon (`setinventoryweapon`): a streak a
    /// care package gave him, used from its own slot.
    pub inventory: BTreeMap<u32, String>,
    /// Entities launchers may lock on to (`target_set`).
    pub targets: BTreeSet<u32>,
    /// Projectiles handed to the scripts, by projectile id.
    pub thrown: BTreeMap<u32, Thrown>,
    /// Entities falling after `physicslaunch` (a dropped crate): velocity.
    pub falling: BTreeMap<u32, [f32; 3]>,
    /// Entities a player can use (`makeusable`: a care package).
    pub usable: BTreeSet<u32>,
    /// Helicopters and planes the scripts fly (`spawnhelicopter`).
    pub craft: BTreeMap<u32, craft::Craft>,
    /// Turrets (`spawnturret`).
    pub turrets: BTreeMap<u32, turret::Turret>,
    /// Players riding or driving a vehicle (`usevehicle`), by client.
    pub rides: BTreeMap<u32, ride::Ride>,
    /// Lock-on targets of missiles (`missile_settarget`): missile ent ->
    /// target ent.
    pub homing: BTreeMap<u32, u32>,
    /// Test aid (BO2MP_KSLOG): when the hand log last ran.
    pub log_at: i64,
    /// Entities below this number have their `birthtime`.
    pub born_upto: u32,
    /// Vehicles' own weapons from his zones: type -> (turret weapon,
    /// gunner weapons).
    pub vehicle_weapons: BTreeMap<String, (String, Vec<String>)>,
    /// Shots its craft fired (each one's own tracer).
    pub shots: u32,
    /// How each vehicle type drives and how its driver sees it.
    pub vehicle_drive: BTreeMap<String, super::T6VehicleDrive>,
    /// The minimap's frame (`setminimap`): upper left, north, world size.
    pub minimap: Option<hud_iw4::CompassMapBounds>,
}

/// A new map: fresh scorestreak state, the vehicles' weapons from his zones.
pub(super) fn install(world: &mut World, vehicles: Vec<super::T6Vehicle>) {
    let vehicle_drive = vehicles
        .iter()
        .map(|v| (v.name.to_ascii_lowercase(), v.drive))
        .collect();
    let vehicle_weapons = vehicles
        .into_iter()
        .map(|v| (v.name.to_ascii_lowercase(), (v.turret, v.gunners)))
        .collect::<BTreeMap<String, (String, Vec<String>)>>();
    let armed = vehicle_weapons
        .iter()
        .filter(|(_, (t, g))| !t.is_empty() || !g.is_empty())
        .map(|(n, (t, g))| {
            format!(
                "{n}={t}{}",
                if g.is_empty() {
                    String::new()
                } else {
                    format!("+{}", g.join("+"))
                }
            )
        })
        .collect::<Vec<_>>();
    if !vehicle_weapons.is_empty() {
        diag::info!(
            Sim,
            "bo2mp streaks: {} vehicles, armed: {}",
            vehicle_weapons.len(),
            armed.join(" ")
        );
    }
    world.insert_resource(Streaks {
        vehicle_weapons,
        vehicle_drive,
        ..Default::default()
    });
}

fn streaks(world: &mut World) -> bevy_ecs::prelude::Mut<'_, Streaks> {
    if !world.contains_resource::<Streaks>() {
        world.insert_resource(Streaks::default());
    }
    world.resource_mut::<Streaks>()
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
    // Scorestreaks, not killstreaks: BO2's exe sets scr_scorestreaks (score
    // earns them, each kept until used) and lets one of each be held.
    // PLACEHOLDER: the exe's own defaults are not in his data files; these
    // are the values BO2's public matches play by. player_useRadius: how
    // near a player must be to use something (the engine's 128; bots
    // capture a care package from inside it).
    for (k, v) in [
        ("scr_scorestreaks", "1"),
        ("scr_scorestreaks_maxstacking", "1"),
        ("player_useradius", "128"),
    ] {
        vm.dvars.entry(k.to_owned()).or_insert_with(|| v.to_owned());
    }
    // Radar: a team's spy plane (UAV: 1 sweeps, 2 sweeps fast with two up)
    // and satellite (Orbital VSAT: enemies shown all the time).
    f!("getteamspyplane", |vm, world, _, a| {
        let team = text(vm, a, 0);
        Ok(Value::Int(
            streaks(world).spyplane.get(&team).copied().unwrap_or(0),
        ))
    });
    f!("setteamspyplane", |vm, world, _, a| {
        let team = text(vm, a, 0);
        let v = arg(a, 1).as_int().unwrap_or(0);
        streaks(world).spyplane.insert(team, v);
        Ok(Value::Undefined)
    });
    f!("getteamsatellite", |vm, world, _, a| {
        let team = text(vm, a, 0);
        Ok(Value::Int(
            streaks(world).satellite.get(&team).copied().unwrap_or(0),
        ))
    });
    f!("setteamsatellite", |vm, world, _, a| {
        let team = text(vm, a, 0);
        let v = arg(a, 1).as_int().unwrap_or(0);
        streaks(world).satellite.insert(team, v);
        Ok(Value::Undefined)
    });
    // The inventory slot: a streak from a care package ("" empties it;
    // "none" when empty, as the scripts compare it with weapon names).
    m!("setinventoryweapon", |vm, world, s, a| {
        let Some(n) = entnum(vm, s) else {
            return Ok(Value::Undefined);
        };
        let w = text(vm, a, 0);
        let mut st = streaks(world);
        if w.is_empty() || w == "none" {
            st.inventory.remove(&n);
        } else {
            st.inventory.insert(n, w);
        }
        Ok(Value::Undefined)
    });
    m!("getinventoryweapon", |vm, world, s, _| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        let w = streaks(world).inventory.get(&n).cloned();
        Ok(vm.string(w.as_deref().unwrap_or("none")))
    });
    // A d-pad slot (1 up, 2 down, 3 left, 4 right): BO2's HUD draws what it
    // holds, and pressing it switches to a weapon it holds (the scorestreak
    // weapons, cheapest on the right). The engine's own slots carry it to
    // the client, which raises the weapon as the key goes down.
    m!("setactionslot", |vm, world, s, a| {
        let id = super::client(vm, world, s)?;
        let slot = arg(a, 0).as_int().unwrap_or(0);
        let kind = text(vm, a, 1).to_ascii_lowercase();
        let held = if kind == "weapon" {
            text(vm, a, 2)
        } else {
            kind.clone()
        };
        {
            let mut zm = world.resource_mut::<Zm>();
            let slots = zm.action_slots.entry(id.0).or_default();
            slots.retain(|(n, _)| *n != slot);
            if !held.is_empty() {
                slots.push((slot, held.clone()));
                slots.sort();
            }
        }
        let (ty, param) = match kind.as_str() {
            "weapon" => (1, super::weapon(world, &held).unwrap_or(0) as i32),
            "altmode" => (2, 0),
            "nightvision" => (3, 0),
            _ => (0, 0),
        };
        let (ty, param) = if ty == 1 && param == 0 {
            (0, 0)
        } else {
            (ty, param)
        };
        if let Ok(i) = usize::try_from(slot - 1)
            && i < 4
            && let Some(ps) = frame(world).player_mut(id)
        {
            ps.action_slot_type[i] = ty;
            ps.action_slot_param[i] = param;
        }
        Ok(Value::Undefined)
    });
    // His inventory and fourth-slot buttons: BO2 tells a streak used from
    // the inventory slot from one on the d-pad by them. Our keys raise
    // d-pad weapons only.
    m!("inventorybuttonpressed", |_, _, _, _| Ok(Value::Int(0)));
    m!("actionslotfourbuttonpressed", |_, _, _, _| Ok(Value::Int(
        0
    )));
    // Lock-on targets (spy planes, helicopters, drones): what launchers and
    // the missile drone may home on.
    f!("target_set", |vm, world, _, a| {
        if let Some(n) = entnum(vm, arg(a, 0)) {
            streaks(world).targets.insert(n);
        }
        Ok(Value::Undefined)
    });
    f!("target_remove", |vm, world, _, a| {
        if let Some(n) = entnum(vm, arg(a, 0)) {
            streaks(world).targets.remove(&n);
        }
        Ok(Value::Undefined)
    });
    f!("target_istarget", |vm, world, _, a| {
        let n = entnum(vm, arg(a, 0)).unwrap_or(u32::MAX);
        Ok(Value::bool(streaks(world).targets.contains(&n)))
    });
    f!("target_getarray", |_, world, _, _| {
        let ns: Vec<u32> = streaks(world).targets.iter().copied().collect();
        let zm = world.resource::<Zm>();
        let v = ns
            .into_iter()
            .filter_map(|n| zm.ents.get(&n).and_then(|e| e.obj).map(Value::Object))
            .collect();
        Ok(list(v))
    });
    f!("target_getoffset", |_, _, _, _| Ok(Value::Vec3([0.0; 3])));
    f!("target_setturretaquire", |_, _, _, _| Ok(Value::Undefined));
    // Stats and the match recorder (no online services), thermal outlines.
    for name in [
        "recordkillstreakevent",
        "recordkillstreakendevent",
        "sendkillstreakdamageevent",
    ] {
        vm.bind(name, true, |_, _, _, _| Ok(Value::Undefined));
        vm.bind(name, false, |_, _, _, _| Ok(Value::Undefined));
    }
    for name in [
        "setdrawinfrared",
        "setforcenocull",
        "setinfraredvision",
        "mayapplyscreeneffect",
    ] {
        vm.bind(name, true, |_, _, _, _| Ok(Value::Undefined));
    }
    // A dropped crate falls (`physicslaunch`): straight down under gravity
    // from where it was let go, then `stationary` where it lands.
    m!("physicslaunch", |vm, world, s, a| {
        if let Some(n) = entnum(vm, s) {
            let v = arg(a, 1).as_vec3().unwrap_or([0.0; 3]);
            streaks(world).falling.insert(n, [0.0, 0.0, v[2]]);
        }
        Ok(Value::Undefined)
    });
    // A model a player can use (the care package): looking near it and
    // pressing use fires its `trigger`, as a use trigger does.
    m!("makeusable", |vm, world, s, _| {
        if let Some(n) = entnum(vm, s) {
            streaks(world).usable.insert(n);
        }
        Ok(Value::Undefined)
    });
    m!("makeunusable", |vm, world, s, _| {
        if let Some(n) = entnum(vm, s) {
            streaks(world).usable.remove(&n);
        }
        Ok(Value::Undefined)
    });
    // Splash damage the scripts deal (a crate landing on someone, a
    // turret's explosion, the EMP's blast): everyone in the radius the
    // blast can see, full at the centre down to the minimum at the edge.
    f!("radiusdamage", |vm, world, _, a| {
        radius_damage(vm, world, None, a)
    });
    m!("radiusdamage", |vm, world, s, a| {
        let inflictor = s.clone();
        radius_damage(vm, world, Some(inflictor), a)
    });
    // A projectile the scripts fire themselves (a missile, the Hellstorm,
    // the Swarm): the engine's own projectile, owned by the player.
    f!("magicbullet", |vm, world, _, a| {
        let name = text(vm, a, 0);
        let (start, end) = (super::vec3(a, 1)?, super::vec3(a, 2)?);
        let Ok(owner) = super::client(vm, world, arg(a, 3)) else {
            return Ok(Value::Undefined);
        };
        let w = super::weapon(world, &name)?;
        if w == 0 {
            return Ok(Value::Undefined);
        }
        let t = super::tick(world);
        let p = match crate::missile::magic_bullet(&mut frame(world), t, owner, w, start, end) {
            Ok(p) => p,
            Err(e) => {
                diag::warn!(Sim, "bo2mp streaks: magicbullet {name}: {e}");
                return Ok(Value::Undefined);
            }
        };
        let classname = if name.contains("grenade") {
            "grenade"
        } else {
            "rocket"
        };
        let (ent, obj) = adopt(vm, world, p.origin, classname);
        streaks(world).thrown.insert(
            p.id.0,
            Thrown {
                ent,
                owner: owner.0,
                origin: p.origin,
                stationary: false,
            },
        );
        if let Some(target) = entnum(vm, arg(a, 4)) {
            streaks(world).homing.insert(ent, target);
        }
        Ok(Value::Object(obj))
    });
    // A plane drops a bomb (`plane launchbomb(weapon, position, velocity)`:
    // the Lightning Strike): the weapon's projectile from there along the
    // velocity, owned by the plane's owner.
    m!("launchbomb", |vm, world, s, a| {
        let name = text(vm, a, 0);
        let from = super::vec3(a, 1)?;
        let v = arg(a, 2).as_vec3().unwrap_or([0.0, 0.0, -5000.0]);
        // Called on the player (the Lightning Strike's dropbomb) or on his
        // plane (its owner).
        let owner = super::client(vm, world, s).ok().or_else(|| match s {
            Value::Object(o) => {
                let f = vm.intern("owner");
                let who = vm.raw_field(*o, f);
                super::client(vm, world, &who).ok()
            }
            _ => None,
        });
        let (Some(owner), Ok(w)) = (owner, super::weapon(world, &name)) else {
            return Ok(Value::Undefined);
        };
        let to = [from[0] + v[0], from[1] + v[1], from[2] + v[2]];
        let t = super::tick(world);
        let p = match crate::missile::magic_bullet(&mut frame(world), t, owner, w, from, to) {
            Ok(p) => p,
            Err(e) => {
                diag::warn!(Sim, "bo2mp streaks: launchbomb {name}: {e}");
                return Ok(Value::Undefined);
            }
        };
        // A bomb leaves at the velocity given, not at the weapon's throw
        // speeds (the planemortar bomb's upward launch sent it skyward).
        if let Some(b) = frame(world).projectile_mut_by_number(p.entnum) {
            b.velocity = v;
            b.pos.tr_base = from;
            b.pos.tr_delta = v;
            b.pos.tr_time = crate::level_time_ms(t);
        }
        let (ent, obj) = adopt(vm, world, p.origin, "rocket");
        streaks(world).thrown.insert(
            p.id.0,
            Thrown {
                ent,
                owner: owner.0,
                origin: p.origin,
                stationary: false,
            },
        );
        Ok(Value::Object(obj))
    });
    // The Hellstorm: the player rides the missile (`linktomissile`) and
    // steers it with his view until it hits or he lets go - the engine's
    // own guided missile, as MW2's Predator.
    m!("linktomissile", |vm, world, s, a| {
        let id = super::client(vm, world, s)?;
        let Some(n) = entnum(vm, arg(a, 0)) else {
            return Ok(Value::Undefined);
        };
        let Some(pid) = streaks(world)
            .thrown
            .iter()
            .find(|(_, t)| t.ent == n)
            .map(|(id, _)| *id)
        else {
            return Ok(Value::Undefined);
        };
        let Some(p) = crate::frame::collect_projectiles(world)
            .into_iter()
            .find(|p| p.id.0 == pid)
        else {
            return Ok(Value::Undefined);
        };
        let mut f = frame(world);
        if f.client_meta(id).is_some() {
            f.client_meta_mut(id).remote_missile = Some(crate::RemoteMissile {
                projectile: p.id,
                entnum: p.entnum,
                angles: math_iw4::vect_to_angles(p.velocity),
                ..Default::default()
            });
        }
        Ok(Value::Undefined)
    });
    m!("unlinkfrommissile", |vm, world, s, _| {
        let id = super::client(vm, world, s)?;
        let mut f = frame(world);
        if f.client_meta(id).is_some() {
            f.client_meta_mut(id).remote_missile = None;
        }
        Ok(Value::Undefined)
    });
    // A missile homes on what the scripts name (the Hunter Killer's car).
    m!("missile_settarget", |vm, world, s, a| {
        if let Some(n) = entnum(vm, s) {
            match entnum(vm, arg(a, 0)) {
                Some(t) => streaks(world).homing.insert(n, t),
                None => streaks(world).homing.remove(&n),
            };
        }
        Ok(Value::Undefined)
    });
    // A projectile goes off now (`detonate`).
    m!("detonate", |vm, world, s, _| {
        let Some(n) = entnum(vm, s) else {
            return Ok(Value::Undefined);
        };
        let id = streaks(world)
            .thrown
            .iter()
            .find(|(_, t)| t.ent == n)
            .map(|(id, _)| *id);
        let now = super::tick(world);
        let now_ms = crate::level_time_ms(now);
        if let Some(id) = id {
            let mut f = frame(world);
            let number = crate::frame::collect_projectiles(f.ecs())
                .into_iter()
                .find(|p| p.id.0 == id)
                .map(|p| p.entnum);
            if let Some(number) = number
                && let Some(p) = f.projectile_mut_by_number(number)
            {
                p.detonate_at_ms = Some(now_ms);
                p.grounded = true;
            }
        }
        Ok(Value::Undefined)
    });
    // A grenade's fuse (the care package marker's smoke waits for it).
    // PLACEHOLDER: the weapon's fuse time is not carried by the engine's
    // weapon facts yet.
    f!("getweaponfusetime", |_, _, _, _| Ok(Value::Int(1000)));
    f!("getweaponprojexplosionsound", |vm, _, _, _| Ok(
        vm.string("")
    ));
    // A weapon with only what is in its clip (the Death Machine): its
    // reserve holds no more than one clip.
    f!("isweaponcliponly", |vm, world, _, a| {
        let w = super::weapon(world, &text(vm, a, 0)).unwrap_or(0);
        let only = frame(world)
            .combat_facts_for(w)
            .is_some_and(|f| f.max_ammo <= f.clip_size);
        Ok(Value::bool(only))
    });
    // Picking a spot on the map (Lightning Strike, Lodestar, Stealth
    // Chopper): BO2's HUD shows the full map with the selector (its
    // material and radius); the pick comes back as `confirm_location`
    // (a bot's own scripts send it themselves), backing out as
    // `cancel_location`.
    fn begin_selection(
        vm: &mut Vm<World>,
        world: &mut World,
        s: &Value,
        a: &[Value],
    ) -> Result<Value, String> {
        let id = super::client(vm, world, s)?;
        let material = text(vm, a, 0);
        let radius = arg(a, 1).as_float().unwrap_or(0.0);
        let mut f = frame(world);
        if f.client_meta(id).is_some() {
            f.client_meta_mut(id).location_selection = Some(crate::LocationSelection {
                material: material.clone(),
                choose_direction: false,
                radius,
            });
        }
        super::set_client_dvar(world, id.0, "bo2mp_locsel", &format!("{material} {radius}"));
        Ok(Value::Undefined)
    }
    m!("beginlocationselection", |vm, world, s, a| begin_selection(
        vm, world, s, a
    ));
    m!("beginlocationmortarselection", |vm, world, s, a| {
        begin_selection(vm, world, s, a)
    });
    m!("beginlocationcomlinkselection", |vm, world, s, a| {
        begin_selection(vm, world, s, a)
    });
    m!("beginlocationairstrikeselection", |vm, world, s, a| {
        begin_selection(vm, world, s, a)
    });
    m!("endlocationselection", |vm, world, s, _| {
        let id = super::client(vm, world, s)?;
        let mut f = frame(world);
        if f.client_meta(id).is_some() {
            f.client_meta_mut(id).location_selection = None;
        }
        super::set_client_dvar(world, id.0, "bo2mp_locsel", "");
        Ok(Value::Undefined)
    });
    // Path nodes a turret or a dropped crate covers (bots keep off them):
    // the engine's bot pathing reads no danger marks yet.
    for name in [
        "setblockweaponpickup",
        "setdangerous",
        "cleardangerous",
        "sethintstringforperk",
        "rotatevelocity",
    ] {
        vm.bind(name, true, |_, _, _, _| Ok(Value::Undefined));
    }
    // Where to aim at someone (`getshootatpos`: a dog's head tracks it):
    // a player's eye, else a body's middle.
    m!("getshootatpos", |vm, world, s, _| {
        if let Ok(id) = super::client(vm, world, s) {
            let f = frame(world);
            let (o, h) = f
                .player(id)
                .map_or(([0.0; 3], 60.0), |ps| (ps.origin, ps.view_height_current));
            return Ok(Value::Vec3([o[0], o[1], o[2] + h]));
        }
        let o = super::origin_of(vm, world, s).unwrap_or([0.0; 3]);
        Ok(Value::Vec3([o[0], o[1], o[2] + 32.0]))
    });
    // A body's eye (`geteye` on an actor: a dog's head tracks its enemy
    // from it): a player's own, else a dog's head height over it.
    m!("geteye", |vm, world, s, _| {
        if let Ok(id) = super::client(vm, world, s) {
            let f = frame(world);
            let (o, h) = f
                .player(id)
                .map_or(([0.0; 3], 60.0), |ps| (ps.origin, ps.view_height_current));
            return Ok(Value::Vec3([o[0], o[1], o[2] + h]));
        }
        let o = super::origin_of(vm, world, s).unwrap_or([0.0; 3]);
        Ok(Value::Vec3([o[0], o[1], o[2] + 24.0]))
    });
    // Which voices it answers (`settalktospecies`): no battle chatter.
    m!("settalktospecies", |_, _, _, _| Ok(Value::Undefined));
    // Whose an actor is (`setentityowner`: a K9 Unit dog never goes for
    // the player who called it).
    m!("setentityowner", |vm, world, s, a| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        if let Ok(id) = super::client(vm, world, arg(a, 0))
            && let Some(x) = world.resource_mut::<Zm>().actors.get_mut(&n)
        {
            x.owner = Some(id.0);
        }
        Ok(Value::Undefined)
    });
    // Whose a missile is (`getmissileowner(missile)`: the player who
    // fired it, the same one its `missile_fire` went to), and handing it
    // to someone else (`missile setmissileowner(player)`).
    f!("getmissileowner", |vm, world, _, a| {
        let n = entnum(vm, arg(a, 0)).unwrap_or(u32::MAX);
        let owner = streaks(world)
            .thrown
            .values()
            .find(|t| t.ent == n)
            .map(|t| t.owner);
        Ok(owner.map_or(Value::Undefined, |o| craft::world_obj(world, o)))
    });
    m!("setmissileowner", |vm, world, s, a| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        if let Ok(id) = super::client(vm, world, arg(a, 0))
            && let Some(t) = streaks(world).thrown.values_mut().find(|t| t.ent == n)
        {
            t.owner = id.0;
        }
        Ok(Value::Undefined)
    });
    // How much of an entity a point sees (`entity sightconetrace(point,
    // ignore)`, `damageconetrace`: a helicopter's or a Guardian's sight of
    // its target): all of it when the line to its middle is clear.
    fn cone_trace(
        vm: &mut Vm<World>,
        world: &mut World,
        s: &Value,
        a: &[Value],
    ) -> Result<Value, String> {
        let from = super::vec3(a, 0)?;
        let Some(to) = super::origin_of(vm, world, s) else {
            return Ok(Value::Float(0.0));
        };
        let mid = if super::is_player(vm, world, s) {
            40.0
        } else {
            0.0
        };
        let to = [to[0], to[1], to[2] + mid];
        let t = frame(world).trace_static_world(
            from,
            to,
            [0.0; 3],
            [0.0; 3],
            crate::bullet_collision::MASK_SHOT,
        );
        Ok(Value::Float(if t.fraction >= 1.0 { 1.0 } else { 0.0 }))
    }
    m!("sightconetrace", |vm, world, s, a| cone_trace(
        vm, world, s, a
    ));
    m!("damageconetrace", |vm, world, s, a| cone_trace(
        vm, world, s, a
    ));
    // The K9 Unit's dogs (actors): put somewhere at once (`teleport`), an
    // animation state of their own (`setanimstate`: jumping down a ledge),
    // no forced target.
    m!("teleport", |vm, world, s, a| {
        let Some(n) = entnum(vm, s) else {
            return Ok(Value::Undefined);
        };
        let p = super::vec3(a, 0)?;
        let ang = arg(a, 1).as_vec3();
        if let Some(e) = world.resource_mut::<Zm>().ents.get_mut(&n) {
            e.origin = p;
            if let Some(ang) = ang {
                e.angles = ang;
            }
        }
        Ok(Value::Undefined)
    });
    m!("setanimstate", |vm, world, s, a| {
        let Some(n) = entnum(vm, s).filter(|n| world.resource::<Zm>().actors.contains_key(n))
        else {
            return Ok(Value::Undefined);
        };
        let state = text(vm, a, 0);
        super::actors::set_state(vm, world, n, &state, &Value::Undefined);
        Ok(Value::Undefined)
    });
    m!("clearentitytarget", |_, _, _, _| Ok(Value::Undefined));
    // Client flags on an entity (`setclientflag` is the client's): none set
    // by the engine itself.
    m!("getclientflag", |_, _, _, _| Ok(Value::Int(0)));
    // A weapon kept out of a match's first moments (none in a local match).
    f!("isweapondisallowedatmatchstart", |_, _, _, _| Ok(
        Value::Int(0)
    ));
    // A box swept through the world (`physicstrace(start, end, mins, maxs,
    // ignore, mask)`: where an RC-XD or an AGR can be set down): BO2 hands
    // back the trace's facts (fraction, position, normal), not a point.
    f!("physicstrace", |vm, world, _, a| {
        let (s, e) = (super::vec3(a, 0)?, super::vec3(a, 1)?);
        let mins = arg(a, 2).as_vec3().unwrap_or([0.0; 3]);
        let maxs = arg(a, 3).as_vec3().unwrap_or([0.0; 3]);
        let t =
            frame(world).trace_static_world(s, e, mins, maxs, crate::bullet_collision::MASK_SHOT);
        let f = if t.startsolid != 0 {
            0.0
        } else {
            t.fraction.clamp(0.0, 1.0)
        };
        let pos: [f32; 3] = std::array::from_fn(|i| s[i] + (e[i] - s[i]) * f);
        let mut arr = gsc_t6::Array::new();
        let k = |vm: &mut Vm<World>, s: &str| gsc_t6::Key::Str(vm.intern(s));
        let surface = vm.string(if f < 1.0 { "default" } else { "none" });
        arr.set(k(vm, "fraction"), Value::Float(f));
        arr.set(k(vm, "position"), Value::Vec3(pos));
        arr.set(
            k(vm, "normal"),
            Value::Vec3(if f < 1.0 { t.normal } else { [0.0; 3] }),
        );
        arr.set(k(vm, "surfacetype"), surface);
        Ok(Value::array(arr))
    });
    // Locking a launcher on a scorestreak (FHJ-18, Stinger): how long a
    // lock takes and how wide its circle is on screen (PLACEHOLDER: the
    // weapon's own lockOnSpeed / lockOnRadius are not in the engine's
    // weapon facts yet), and whether a target sits inside the circle
    // (`target_isincircle(target, player, fov, radius)`: within the angle
    // that radius covers on a 640-wide screen at that field of view).
    m!("getlockonspeed", |_, _, _, _| Ok(Value::Int(1000)));
    m!("getlockonradius", |_, _, _, _| Ok(Value::Int(100)));
    f!("target_isincircle", |vm, world, _, a| {
        let target = arg(a, 0).clone();
        let Ok(id) = super::client(vm, world, arg(a, 1)) else {
            return Ok(Value::Int(0));
        };
        let fov = arg(a, 2).as_float().unwrap_or(65.0);
        let radius = arg(a, 3).as_float().unwrap_or(100.0);
        let (Some(to), Some(ps)) = (
            super::origin_of(vm, world, &target),
            frame(world).player(id).copied(),
        ) else {
            return Ok(Value::Int(0));
        };
        let eye = [
            ps.origin[0],
            ps.origin[1],
            ps.origin[2] + ps.view_height_current,
        ];
        let dir = gsc_t6::math::normalize(gsc_t6::math::sub(to, eye));
        let fwd = gsc_t6::math::angle_vectors(ps.viewangles).0;
        let limit = (radius * (fov.to_radians() * 0.5).tan() / 320.0).atan();
        let angle = gsc_t6::math::dot(dir, fwd).clamp(-1.0, 1.0).acos();
        Ok(Value::bool(angle <= limit))
    });
    // A point turned by angles (`rotatepoint(point, angles)`: the RC-XD's
    // wheels around its spawn, the care package's rear hatch).
    f!("rotatepoint", |_, _, _, a| {
        let p = super::vec3(a, 0)?;
        let ang = arg(a, 1).as_vec3().unwrap_or([0.0; 3]);
        let (f, r, u) = gsc_t6::math::angle_vectors(ang);
        Ok(Value::Vec3(std::array::from_fn(|i| {
            p[0] * f[i] - p[1] * r[i] + p[2] * u[i]
        })))
    });
    // Whether an entity is a vehicle (scorestreak craft count).
    f!("isvehicle", |vm, world, _, a| {
        let n = entnum(vm, arg(a, 0)).unwrap_or(u32::MAX);
        Ok(Value::bool(
            world
                .resource::<Zm>()
                .ents
                .get(&n)
                .is_some_and(|e| e.classname == "script_vehicle"),
        ))
    });
    // Collision contents (a drone that must not block), trigger filters (a
    // turret's hack prompt ignores its turret, wants Hacker), a shield's
    // model, a player's view kick: kept by the scripts' own entities.
    for name in [
        "setcontents",
        "setignoreentfortrigger",
        "setperkfortrigger",
        "viewkick",
        "setweapon",
    ] {
        vm.bind(name, true, |_, _, _, _| Ok(Value::Undefined));
    }
    // A missile decoy (the Escort Drone draws locking missiles to itself)
    // and a drone missile's map mark: kept by the scripts' own entities.
    f!("missile_createattractorent", |_, _, _, a| Ok(
        arg(a, 0).clone()
    ));
    f!("missile_deleteattractor", |_, _, _, _| Ok(Value::Undefined));
    m!("missile_dronesetvisible", |_, _, _, _| Ok(Value::Undefined));
    craft::bind(vm);
    turret::bind(vm);
    ride::bind(vm);
}

/// The minimap's frame from BO2's own _compass (`setminimap(material,
/// northwest x, y, southeast x, y)` with the map's north): where a spot
/// picked on the map is.
pub(super) fn set_minimap(
    world: &mut World,
    northwest: [f32; 2],
    southeast: [f32; 2],
    north_yaw: f32,
) {
    streaks(world).minimap =
        hud_iw4::compass_map_bounds_from_corners(northwest, southeast, north_yaw);
}

/// A player picked a spot on the map (or backed out): the cursor's place on
/// the minimap (0..255 across, down; direction in 256ths of a turn) as a
/// world position for BO2's scripts, as the engine's own selector reports
/// it.
pub(super) fn location_pick(world: &mut World, client: u32, picked: [u8; 3], confirm: bool) {
    let Some(obj) = world.resource::<Zm>().players.get(&client).map(|p| p.obj) else {
        return;
    };
    if !confirm {
        super::with_vm(world, |vm, world| {
            vm.notify_str(world, obj, "cancel_location", &[])
        });
        return;
    }
    let Some(map) = streaks(world).minimap else {
        return;
    };
    let at = |byte: u8| (f32::from(byte as i8) + 128.0) / 255.0;
    let x = at(picked[0]) * map.world_size[0];
    let y = at(picked[1]) * map.world_size[1];
    let location = [
        x * map.north[1] + map.upper_left[0] - y * map.north[0],
        map.upper_left[1] - x * map.north[0] - y * map.north[1],
        0.0,
    ];
    let north_yaw = map.north[1].atan2(map.north[0]).to_degrees();
    let yaw = (f32::from(picked[2]) * (360.0 / 256.0) + north_yaw).rem_euclid(360.0);
    super::with_vm(world, |vm, world| {
        vm.notify_str(
            world,
            obj,
            "confirm_location",
            &[Value::Vec3(location), Value::Float(yaw)],
        )
    });
}

/// A thrown or fired projectile the scripts hold (`grenade_fire`,
/// `missile_fire`, `magicbullet`): its entity, whose it is, where it was,
/// whether it came to rest.
#[derive(Clone, Debug)]
pub(crate) struct Thrown {
    ent: u32,
    owner: u32,
    origin: [f32; 3],
    stationary: bool,
}

/// A script entity standing for something the engine draws itself (a
/// projectile): hidden, not solid.
fn adopt(
    vm: &mut Vm<World>,
    world: &mut World,
    origin: [f32; 3],
    classname: &str,
) -> (u32, gsc_t6::ObjRef) {
    let ent = world.resource_mut::<Zm>().alloc_entnum();
    let obj = vm.alloc_object(gsc_t6::ObjKind::Entity(ent));
    world.resource_mut::<Zm>().ents.insert(
        ent,
        super::Ent {
            obj: Some(obj),
            classname: classname.to_owned(),
            origin,
            hidden: true,
            ..Default::default()
        },
    );
    (ent, obj)
}

/// `radiusdamage(origin, range, max, min, attacker, means, weapon)`.
fn radius_damage(
    vm: &mut Vm<World>,
    world: &mut World,
    inflictor: Option<Value>,
    a: &[Value],
) -> Result<Value, String> {
    let origin = super::vec3(a, 0)?;
    let range = super::num(a, 1)?.max(1.0);
    let (max, min) = (
        super::num(a, 2).unwrap_or(0.0),
        super::num(a, 3).unwrap_or(0.0),
    );
    let attacker = arg(a, 4).clone();
    let means = match arg(a, 5) {
        Value::Undefined => "MOD_EXPLOSIVE".to_owned(),
        v => vm.to_text(v),
    };
    let weapon = match arg(a, 6) {
        Value::Undefined => "none".to_owned(),
        v => vm.to_text(v),
    };
    let inflictor = inflictor.unwrap_or_else(|| attacker.clone());
    let sees = |world: &mut World, to: [f32; 3]| {
        frame(world)
            .trace_static_world(
                origin,
                to,
                [0.0; 3],
                [0.0; 3],
                crate::bullet_collision::MASK_SHOT,
            )
            .fraction
            >= 1.0
    };
    let amount = |d: f32| (max - (max - min) * (d / range)).round() as i32;
    let dist = |p: [f32; 3]| {
        ((p[0] - origin[0]).powi(2) + (p[1] - origin[1]).powi(2) + (p[2] - origin[2]).powi(2))
            .sqrt()
    };
    // Players: aimed at their middle.
    let players: Vec<u32> = world.resource::<Zm>().players.keys().copied().collect();
    for n in players {
        let Some(ps) = frame(world).player(ClientId(n)).copied() else {
            continue;
        };
        if ps.health <= 0 {
            continue;
        }
        let mid = [ps.origin[0], ps.origin[1], ps.origin[2] + 32.0];
        let d = dist(mid);
        if d > range || !sees(world, mid) {
            continue;
        }
        let dir = gsc_t6::math::normalize(gsc_t6::math::sub(mid, origin));
        super::natives_ai::player_damage(
            vm,
            world,
            n,
            inflictor.clone(),
            attacker.clone(),
            amount(d),
            0,
            &means,
            &weapon,
            origin,
            dir,
            "none",
        );
    }
    // Actors (dogs) and entities that take damage (turrets, crates).
    let ents: Vec<(u32, [f32; 3], bool, Option<gsc_t6::ObjRef>)> = {
        let zm = world.resource::<Zm>();
        zm.ents
            .iter()
            .filter(|(n, e)| e.can_damage || zm.actors.contains_key(n))
            .map(|(n, e)| (*n, e.origin, zm.actors.contains_key(n), e.obj))
            .collect()
    };
    for (n, at, actor, obj) in ents {
        let d = dist(at);
        if d > range || inflictor.as_obj() == obj {
            continue;
        }
        if actor {
            super::actors::damage(
                vm,
                world,
                n,
                inflictor.clone(),
                attacker.clone(),
                amount(d),
                0,
                &means,
                &weapon,
                origin,
                [0.0; 3],
                "none",
            );
        } else if let Some(o) = obj {
            let (m, w, none) = (vm.string(&means), vm.string(&weapon), vm.string(""));
            let args = [
                Value::Int(amount(d)),
                attacker.clone(),
                Value::Vec3(gsc_t6::math::normalize(gsc_t6::math::sub(at, origin))),
                Value::Vec3(origin),
                m,
                none.clone(),
                none.clone(),
                none,
                w,
                Value::Int(0),
            ];
            vm.notify_str(world, o, "damage", &args);
        }
    }
    Ok(Value::Undefined)
}

/// Each authority frame (before the scripts run): projectiles players threw
/// or fired become script entities (BO2's engine hands them to the scripts:
/// the care package marker, a launcher's rocket) that tell the scripts when
/// they come to rest (`stationary`) and go off (`explode`, `death`);
/// homing missiles steer; dropped crates fall; usable models are used;
/// helicopters fly; the radar state goes to each player's HUD.
pub(super) fn advance(world: &mut World) {
    birth(world);
    projectiles(world);
    falling(world);
    use_models(world);
    ride::advance(world);
    craft::advance(world);
    turret::advance(world);
    publish_radar(world);
    hand_log(world);
}

/// A new entity's `birthtime`: the server time it came (the spy plane
/// scripts compare it with its owner's spawn time).
fn birth(world: &mut World) {
    let from = streaks(world).born_upto;
    let (to, now) = {
        let zm = world.resource::<Zm>();
        (zm.next_entnum, zm.now_ms)
    };
    if to <= from {
        return;
    }
    let objs: Vec<gsc_t6::ObjRef> = world
        .resource::<Zm>()
        .ents
        .range(from..to)
        .filter_map(|(_, e)| e.obj)
        .collect();
    super::with_vm(world, |vm, _| {
        let f = vm.intern("birthtime");
        for o in objs {
            if matches!(vm.raw_field(o, f), Value::Undefined) {
                vm.set_raw_field(o, f, Value::Int(now as i32));
            }
        }
    });
    streaks(world).born_upto = to;
}

/// Notify players or entities (by entity number) in one go.
fn notify_all(world: &mut World, events: Vec<(u32, &'static str, Vec<Value>)>) {
    if events.is_empty() {
        return;
    }
    super::with_vm(world, |vm, world| {
        for (ent, name, args) in events {
            let zm = world.resource::<Zm>();
            let obj = zm
                .players
                .get(&ent)
                .map(|p| p.obj)
                .or_else(|| zm.ents.get(&ent).and_then(|e| e.obj));
            if let Some(o) = obj
                && vm.alive(o)
            {
                vm.notify_str(world, o, name, &args);
            }
        }
    });
}

fn projectiles(world: &mut World) {
    let live: Vec<crate::ProjectileState> = crate::frame::collect_projectiles(world)
        .into_iter()
        .filter(|p| p.live)
        .collect();
    let known: BTreeMap<u32, Thrown> = std::mem::take(&mut streaks(world).thrown);
    let mut keep: BTreeMap<u32, Thrown> = BTreeMap::new();
    let mut events: Vec<(u32, &'static str, Vec<Value>)> = Vec::new();
    for p in &live {
        let id = p.id.0;
        if let Some(mut t) = known.get(&id).cloned() {
            // The scripts deleted it (the Hunter Killer's thrown case): it
            // is gone from the world too.
            if !world.resource::<Zm>().ents.contains_key(&t.ent) {
                frame(world).remove_projectile_by_number(p.entnum);
                continue;
            }
            t.origin = p.origin;
            if let Some(e) = world.resource_mut::<Zm>().ents.get_mut(&t.ent) {
                e.origin = p.origin;
            }
            if !t.stationary && (p.grounded || p.pos.tr_type == entity_iw4::TR_STATIONARY) {
                t.stationary = true;
                events.push((t.ent, "stationary", Vec::new()));
            }
            keep.insert(id, t);
            continue;
        }
        let weap_type = frame(world)
            .combat_facts_for(p.weapon)
            .map_or(-1, |f| f.weap_type);
        let (classname, notify) = match weap_type {
            weapon_iw4::WEAPTYPE_GRENADE => ("grenade", "grenade_fire"),
            weapon_iw4::WEAPTYPE_PROJECTILE => ("rocket", "missile_fire"),
            _ => continue,
        };
        if !world.resource::<Zm>().players.contains_key(&p.owner.0) {
            continue;
        }
        let name = super::weapon_text(world, p.weapon);
        let made = super::with_vm(world, |vm, world| {
            let (ent, obj) = adopt(vm, world, p.origin, classname);
            let w = vm.string(&name);
            (ent, obj, w)
        });
        if let Some((ent, obj, w)) = made {
            keep.insert(
                id,
                Thrown {
                    ent,
                    owner: p.owner.0,
                    origin: p.origin,
                    stationary: false,
                },
            );
            events.push((p.owner.0, notify, vec![Value::Object(obj), w]));
        }
    }
    // Gone: it went off where it last was.
    let mut gone = Vec::new();
    for (id, t) in &known {
        if keep.contains_key(id) {
            continue;
        }
        events.push((t.ent, "explode", vec![Value::Vec3(t.origin)]));
        events.push((t.ent, "death", Vec::new()));
        gone.push(t.ent);
    }
    streaks(world).thrown = keep;
    // Homing missiles turn toward their target's middle.
    let homing: Vec<(u32, u32)> = streaks(world)
        .homing
        .iter()
        .map(|(m, t)| (*m, *t))
        .collect();
    for (missile, target) in homing {
        let id = streaks(world)
            .thrown
            .iter()
            .find(|(_, t)| t.ent == missile)
            .map(|(id, _)| *id);
        let Some(id) = id else {
            streaks(world).homing.remove(&missile);
            continue;
        };
        let to = if world.resource::<Zm>().players.contains_key(&target) {
            frame(world)
                .player(ClientId(target))
                .map(|ps| [ps.origin[0], ps.origin[1], ps.origin[2] + 40.0])
        } else {
            world.resource::<Zm>().ents.get(&target).map(|e| e.origin)
        };
        let Some(to) = to else {
            continue;
        };
        let number = live.iter().find(|p| p.id.0 == id).map(|p| p.entnum);
        if let Some(number) = number
            && let Some(p) = frame(world).projectile_mut_by_number(number)
        {
            p.guide.target = Some(crate::MissileTarget::Point(to));
        }
    }
    notify_all(world, events);
    for ent in gone {
        let obj = world
            .resource_mut::<Zm>()
            .ents
            .remove(&ent)
            .and_then(|e| e.obj);
        streaks(world).homing.remove(&ent);
        if let Some(o) = obj {
            super::with_vm(world, |vm, world| vm.free_object(world, o));
        }
    }
}

/// Dropped crates fall at BO2's gravity (800 units/s²) until they hit the
/// world, then settle upright: `stationary`.
fn falling(world: &mut World) {
    let dt = crate::MATCH_TICK_MS as f32 / 1000.0;
    let list: Vec<(u32, [f32; 3])> = streaks(world)
        .falling
        .iter()
        .map(|(n, v)| (*n, *v))
        .collect();
    let mut events = Vec::new();
    for (n, mut v) in list {
        let Some(pos) = world.resource::<Zm>().ents.get(&n).map(|e| e.origin) else {
            streaks(world).falling.remove(&n);
            continue;
        };
        v[2] -= 800.0 * dt;
        let to: [f32; 3] = std::array::from_fn(|i| pos[i] + v[i] * dt);
        let t = frame(world).trace_static_world(
            pos,
            to,
            [-16.0, -16.0, 0.0],
            [16.0, 16.0, 16.0],
            crate::bullet_collision::MASK_SHOT,
        );
        let f = t.fraction.clamp(0.0, 1.0);
        let mut at: [f32; 3] = std::array::from_fn(|i| pos[i] + (to[i] - pos[i]) * f);
        // Caught on something thinner than itself (a lamp post, a sign): it
        // tips off the side it does not rest on and falls on, as BO2's
        // physics drops it (crates stood on Nuketown's posts in mid-air).
        let mut tipped = false;
        if f < 1.0 && t.startsolid == 0 {
            let mut away = [0.0f32; 2];
            let mut held = 0;
            for (dx, dy) in [(-14.0, -14.0), (14.0, -14.0), (-14.0, 14.0), (14.0, 14.0)] {
                let from = [at[0] + dx, at[1] + dy, at[2] + 1.0];
                let under = frame(world).trace_static_world(
                    from,
                    [from[0], from[1], from[2] - 12.0],
                    [0.0; 3],
                    [0.0; 3],
                    crate::bullet_collision::MASK_SHOT,
                );
                if under.fraction < 1.0 {
                    held += 1;
                } else {
                    away[0] += dx;
                    away[1] += dy;
                }
            }
            let len = (away[0] * away[0] + away[1] * away[1]).sqrt();
            if held < 3 && len > 0.0 {
                let side = [
                    at[0] + away[0] / len * 12.0,
                    at[1] + away[1] / len * 12.0,
                    at[2] + 1.0,
                ];
                let s = frame(world).trace_static_world(
                    [at[0], at[1], at[2] + 1.0],
                    side,
                    [-16.0, -16.0, 0.0],
                    [16.0, 16.0, 16.0],
                    crate::bullet_collision::MASK_SHOT,
                );
                if s.startsolid == 0 && s.fraction > 0.0 {
                    at = std::array::from_fn(|i| {
                        let start = if i == 2 { at[2] + 1.0 } else { at[i] };
                        start + (side[i] - start) * s.fraction.clamp(0.0, 1.0)
                    });
                    tipped = true;
                    v = [0.0, 0.0, v[2].min(0.0)];
                }
            }
        }
        if let Some(e) = world.resource_mut::<Zm>().ents.get_mut(&n) {
            e.origin = at;
            if f < 1.0 && !tipped {
                e.angles = [0.0, e.angles[1], 0.0];
            }
        }
        if (f < 1.0 && !tipped) || at[2] < -20000.0 {
            streaks(world).falling.remove(&n);
            events.push((n, "stationary", Vec::new()));
        } else {
            streaks(world).falling.insert(n, v);
        }
    }
    notify_all(world, events);
}

/// Usable models: a living player pressing use within reach (BO2's
/// player_useRadius, 128) fires the nearest one's `trigger`.
fn use_models(world: &mut World) {
    let usable: Vec<u32> = streaks(world).usable.iter().copied().collect();
    if usable.is_empty() {
        return;
    }
    let players: Vec<(u32, gsc_t6::ObjRef)> = world
        .resource::<Zm>()
        .players
        .iter()
        .filter(|(_, p)| p.sessionstate == "playing")
        .map(|(c, p)| (*c, p.obj))
        .collect();
    let mut events = Vec::new();
    for (c, pobj) in players {
        let held = crate::script_player::buttons(&mut frame(world), ClientId(c))
            & (playerstate_iw4::buttons::USE | playerstate_iw4::buttons::USE_RELOAD)
            != 0;
        let pressed = held && !world.resource::<Zm>().use_held.contains(&c);
        if held && std::env::var_os("BO2MP_KSLOG").is_some() {
            diag::info!(
                Sim,
                "bo2mp kslog use {c}: held, pressed {pressed}, {} usable",
                usable.len()
            );
        }
        if !pressed {
            continue;
        }
        let Some(me) = frame(world).player(ClientId(c)).map(|ps| ps.origin) else {
            continue;
        };
        let best = usable
            .iter()
            .filter_map(|n| {
                let e = world.resource::<Zm>().ents.get(n)?;
                let d = ((e.origin[0] - me[0]).powi(2)
                    + (e.origin[1] - me[1]).powi(2)
                    + (e.origin[2] - me[2]).powi(2))
                .sqrt();
                (d <= 128.0).then_some((d, *n))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((_, n)) = best {
            events.push((n, "trigger", vec![Value::Object(pobj)]));
        }
    }
    notify_all(world, events);
}

/// Each player's HUD hears his team's radar (`bo2mp_radar`: spy plane mode,
/// satellite, an enemy counter-UAV up), as BO2's minimap reads it.
fn publish_radar(world: &mut World) {
    if world.resource::<Zm>().now_ms % 500 != 0 {
        return;
    }
    let (teams, cuav) = super::with_vm(world, |vm, world| {
        let (team, tn) = (vm.intern("team"), vm.intern("targetname"));
        let zm = world.resource::<Zm>();
        let teams: Vec<(u32, String)> = zm
            .players
            .iter()
            .map(|(c, p)| (*c, vm.to_text(&vm.raw_field(p.obj, team))))
            .collect();
        // Counter-UAVs up, by team (BO2's scripts name them `counteruav`).
        let cuav: Vec<String> = zm
            .ents
            .values()
            .filter_map(|e| e.obj)
            .filter(|o| vm.to_text(&vm.raw_field(*o, tn)) == "counteruav")
            .map(|o| vm.to_text(&vm.raw_field(o, team)))
            .collect();
        (teams, cuav)
    })
    .unwrap_or_default();
    for (c, team) in teams {
        let (uav, sat) = {
            let st = streaks(world);
            (
                st.spyplane.get(&team).copied().unwrap_or(0),
                st.satellite.get(&team).copied().unwrap_or(0),
            )
        };
        let jammed = i32::from(cuav.iter().any(|t| *t != team));
        super::set_client_dvar(world, c, "bo2mp_radar", &format!("{uav} {sat} {jammed}"));
    }
}

/// BO2MP_KSLOG=1 (test aid): every second, each living player's weapon, its
/// state and what the scripts asked him to switch to.
fn hand_log(world: &mut World) {
    if std::env::var_os("BO2MP_KSLOG").is_none() {
        return;
    }
    let now = world.resource::<Zm>().now_ms;
    if now - streaks(world).log_at < 1000 {
        return;
    }
    streaks(world).log_at = now;
    let players: Vec<u32> = world.resource::<Zm>().players.keys().copied().collect();
    for c in players {
        let Some(ps) = frame(world).player(ClientId(c)).copied() else {
            continue;
        };
        if ps.health <= 0 {
            continue;
        }
        let name = super::weapon_text(world, ps.weapon);
        let switch_to = frame(world)
            .client_meta(ClientId(c))
            .map_or(0, |m| m.controls.switch_to);
        let to = super::weapon_text(world, switch_to);
        let facts = frame(world).combat_facts_for(ps.weapon).map(|f| {
            (
                f.weap_type,
                f.weap_class,
                f.fire_type,
                f.fire_time_ms,
                f.fire_delay_ms,
                f.clip_size,
            )
        });
        let held = crate::script_player::buttons(&mut frame(world), ClientId(c));
        diag::info!(
            Sim,
            "bo2mp kslog {c}: holds {name} state {} time {} delay {} switch_to {to} slots {:?} {:?} facts(type class fire time delay clip) {facts:?} buttons {held:#x}",
            ps.weaponstate_primary,
            ps.weapon_time,
            ps.weapon_delay,
            ps.action_slot_type,
            ps.action_slot_param
        );
    }
}
