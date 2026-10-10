//! Players joining and leaving: `codecallback_playerconnect` when a client
//! appears, `begin` once it is in the game, `codecallback_playerdisconnect`
//! when it goes.

use bevy_ecs::prelude::World;
use gsc_t6::{Array, ObjKind, ObjRef, Value};

use super::{Player, Zm, frame, with_vm};

pub(super) fn sync(world: &mut World) {
    if !world.resource::<Zm>().started {
        return;
    }
    let clients: Vec<(u32, bool)> = {
        let f = frame(world);
        f.client_ids_sorted()
            .into_iter()
            .map(|id| {
                let joined = f
                    .client_meta(id)
                    .is_some_and(|m| m.lifecycle != crate::ClientLifecycle::Connecting);
                (id.0, joined)
            })
            .collect()
    };
    let known: Vec<u32> = world.resource::<Zm>().players.keys().copied().collect();
    let now = world.resource::<Zm>().now_ms;
    for n in known {
        // bo2mp: a bot made by `addtestclient` waits for its client (up to
        // 15 s, then it is dropped like a disconnect).
        let waiting = world
            .resource::<Zm>()
            .players
            .get(&n)
            .and_then(|p| p.pending_since)
            .is_some_and(|t| now - t < 15_000);
        if waiting {
            continue;
        }
        if !clients.iter().any(|(c, _)| *c == n) {
            world.resource_mut::<Zm>().bots.remove(&n);
            let p = world.resource_mut::<Zm>().players.remove(&n);
            if let Some(p) = p {
                with_vm(world, |vm, world| {
                    let cb = super::mp::callbacks(world);
                    vm.spawn_named(
                        world,
                        &cb,
                        "codecallback_playerdisconnect",
                        Value::Object(p.obj),
                        vec![],
                    );
                    vm.free_object(world, p.obj);
                });
            }
        }
    }
    for (c, joined) in clients {
        let existing = world.resource::<Zm>().players.get(&c).cloned();
        match existing {
            None => {
                with_vm(world, |vm, world| {
                    let obj = vm.alloc_object(ObjKind::Entity(c));
                    world.resource_mut::<Zm>().players.insert(c, Player::new(obj));
                    let pers = vm.intern("pers");
                    vm.set_raw_field(obj, pers, Value::array(Array::new()));
                    diag::info!(Sim, "bo2zm t6: player {c} connects");
                    let cb = super::mp::callbacks(world);
                    if vm
                        .spawn_named(
                            world,
                            &cb,
                            "codecallback_playerconnect",
                            Value::Object(obj),
                            vec![],
                        )
                        .is_none()
                    {
                        diag::warn!(Sim, "bo2zm t6: no {cb}::codecallback_playerconnect");
                    }
                });
            }
            // bo2mp: a bot's client came: it connects now (its entity and
            // pers[] are the ones its script already marked).
            Some(p) if p.pending_since.is_some() => {
                if let Some(pp) = world.resource_mut::<Zm>().players.get_mut(&c) {
                    pp.pending_since = None;
                }
                with_vm(world, |vm, world| {
                    diag::info!(Sim, "bo2mp bot: client {c} connects");
                    let cb = super::mp::callbacks(world);
                    vm.spawn_named(
                        world,
                        &cb,
                        "codecallback_playerconnect",
                        Value::Object(p.obj),
                        vec![],
                    );
                });
            }
            Some(p) if joined && !p.begun => {
                if let Some(pp) = world.resource_mut::<Zm>().players.get_mut(&c) {
                    pp.begun = true;
                }
                // A new game starts in the map's own fog (the last one's
                // game over may have changed it).
                super::set_client_dvar(world, c, "bo2zm_fogbank", "");
                with_vm(world, |vm, world| vm.notify_str(world, p.obj, "begin", &[]));
            }
            Some(_) => {}
        }
    }
    // A player down crawls until he is revived: the movement code's own
    // last stand (pm_type LAST_STAND + its flag: the low view, the crawl,
    // and his client predicts the same - a prone flag set here alone was
    // undone by his movement every tick, his view bouncing up and down).
    let states: Vec<(u32, bool)> = world.resource::<Zm>().players.iter().map(|(c, p)| (*c, p.laststand)).collect();
    for (c, down) in states {
        if let Some(ps) = frame(world).player_mut(crate::world::ClientId(c)) {
            use playerstate_iw4::{PM_TYPE_LAST_STAND, pm_flags};
            if down {
                if ps.pm_type != playerstate_iw4::PM_TYPE_INTERMISSION {
                    ps.pm_type = PM_TYPE_LAST_STAND;
                }
                ps.pm_flags |= pm_flags::LAST_STAND;
            } else if ps.pm_flags & pm_flags::LAST_STAND != 0 {
                ps.pm_flags &= !pm_flags::LAST_STAND;
                if ps.pm_type == PM_TYPE_LAST_STAND {
                    ps.pm_type = 0;
                }
            }
        }
    }
    // A script camera: his eye at the camera entity, looking its way; he
    // stands frozen (intermission) and his gun is put away.
    let cams: Vec<(u32, [f32; 3], [f32; 3], bool)> = {
        let zm = world.resource::<Zm>();
        zm.players
            .iter()
            .filter(|(_, p)| p.camera_on)
            .filter_map(|(c, p)| {
                let e = zm.ents.get(&p.camera?)?;
                Some((*c, e.origin, e.angles, false))
            })
            .collect()
    };
    for (c, origin, angles, _) in cams {
        let id = crate::world::ClientId(c);
        let first = frame(world)
            .player(id)
            .is_some_and(|ps| ps.pm_type != playerstate_iw4::PM_TYPE_INTERMISSION);
        // The camera turns as it moves (the rocket shot tilts down onto the
        // town): his view turns with it the way his client follows a turn
        // (its delta angles), not just the server's copy.
        super::set_player_view(world, id, angles);
        if let Some(ps) = frame(world).player_mut(id) {
            ps.pm_type = playerstate_iw4::PM_TYPE_INTERMISSION;
            ps.velocity = [0.0; 3];
            ps.origin = [origin[0], origin[1], origin[2] - ps.view_height_current];
            if first {
                ps.e_flags ^= playerstate_iw4::eflags::TELEPORT;
                diag::info!(
                    Sim,
                    "bo2zm t6: client {c} views from a camera at ({:.0} {:.0} {:.0}) facing ({:.0} {:.0} {:.0})",
                    origin[0],
                    origin[1],
                    origin[2],
                    angles[0],
                    angles[1],
                    angles[2]
                );
            }
        }
    }
    // Linked players go where their entity goes - not one viewing from a
    // script camera (a player down is linked where he fell; the game-over
    // shot must still take his view to the rocket).
    let links: Vec<(u32, [f32; 3])> = {
        let zm = world.resource::<Zm>();
        zm.players
            .iter()
            .filter(|(_, p)| !(p.camera_on && p.camera.is_some()))
            .filter_map(|(c, p)| {
                p.linked
                    .and_then(|e| zm.ents.get(&e))
                    .map(|e| (*c, e.origin))
            })
            .collect()
    };
    // The movement code's linked mode (no walking; his client takes the
    // server's position as it is) and the entity's position each tick - a
    // teleport each tick snapped his view every time (the up-and-down
    // glitch while he was lowered to the ground, down).
    let linked: std::collections::BTreeSet<u32> = links.iter().map(|(c, _)| *c).collect();
    for (c, origin) in links {
        let id = crate::world::ClientId(c);
        let mut f = frame(world);
        let first = f.player(id).is_some_and(|ps| ps.pm_type != playerstate_iw4::PM_TYPE_NORMAL_LINKED);
        f.set_origin(id, origin);
        if let Some(ps) = f.player_mut(id) {
            if ps.pm_type == 0 {
                ps.pm_type = playerstate_iw4::PM_TYPE_NORMAL_LINKED;
            }
            ps.velocity = [0.0; 3];
            if first {
                ps.e_flags ^= playerstate_iw4::eflags::TELEPORT;
            }
        }
    }
    // Unlinked: back to walking.
    let clients: Vec<u32> = world.resource::<Zm>().players.keys().copied().filter(|c| !linked.contains(c)).collect();
    for c in clients {
        if let Some(ps) = frame(world).player_mut(crate::world::ClientId(c))
            && ps.pm_type == playerstate_iw4::PM_TYPE_NORMAL_LINKED
        {
            ps.pm_type = 0;
        }
    }
}

/// A shellshock that has run its time ends (the engine clears it; the
/// script's `stopshellshock` does the same earlier).
pub(super) fn shock_tick(world: &mut World) {
    let level = crate::level_time_ms(super::tick(world));
    let ended: Vec<u32> = world
        .resource::<Zm>()
        .players
        .iter()
        .filter(|(_, p)| p.shock.as_ref().is_some_and(|(_, end)| *end < level))
        .map(|(c, _)| *c)
        .collect();
    for c in ended {
        super::natives_player::end_shock(world, c, "time ran out");
    }
}

/// `dtp_start` when he dives, `dtp_end` when the dive's slide ends (PhD
/// Flopper, the dive challenges, the bots); `sprint_begin` / `sprint_end`
/// (sprinting into a step trigger plays its sound).
pub(super) fn movement_events(world: &mut World) {
    let clients: Vec<(u32, ObjRef, bool, bool)> = world
        .resource::<Zm>()
        .players
        .iter()
        .map(|(c, p)| (*c, p.obj, p.last_dive, p.last_sprint))
        .collect();
    for (c, obj, was_diving, was_sprinting) in clients {
        let Some(ps) = frame(world).player(crate::world::ClientId(c)).copied() else {
            continue;
        };
        let diving = ps.pm_type <= 9 && movement_iw4::dive_to_prone(&ps);
        let sprinting = ps.last_sprint_start != 0 && ps.last_sprint_end < ps.last_sprint_start;
        if let Some(p) = world.resource_mut::<Zm>().players.get_mut(&c) {
            p.last_dive = diving;
            p.last_sprint = sprinting;
        }
        let mut events: Vec<&str> = Vec::new();
        if diving != was_diving {
            events.push(if diving { "dtp_start" } else { "dtp_end" });
        }
        if sprinting != was_sprinting {
            events.push(if sprinting { "sprint_begin" } else { "sprint_end" });
        }
        if events.is_empty() {
            continue;
        }
        with_vm(world, |vm, world| {
            for e in events {
                vm.notify_str(world, obj, e, &[]);
            }
        });
    }
}

/// What his weapon did since last tick, as the engine tells scripts:
/// `weapon_switch_started`, `weapon_change` (a new weapon in hand),
/// `weapon_change_complete` (raised), `weapon_fired`, `reload_start`,
/// `reload` (the magazine went in).
pub(super) fn weapon_events(world: &mut World) {
    use weapon_iw4::WeaponState as S;
    let clients: Vec<(u32, ObjRef, u32, i32, i32)> = world
        .resource::<Zm>()
        .players
        .iter()
        .map(|(c, p)| (*c, p.obj, p.last_weapon, p.last_wstate, p.last_clip))
        .collect();
    for (c, obj, last_w, last_s, last_clip) in clients {
        let Some(ps) = frame(world).player(crate::world::ClientId(c)).copied() else {
            continue;
        };
        let (w, st) = (ps.weapon, ps.weaponstate_primary);
        let clip = crate::script_player::ammo_clip(&frame(world), crate::world::ClientId(c), w);
        let state = |v: i32| S::from_i32(v).ok();
        let raising = |v: i32| matches!(state(v), Some(S::Raising | S::RaisingAltswitch));
        let dropping = |v: i32| {
            matches!(
                state(v),
                Some(S::Dropping | S::DroppingQuick | S::DroppingAltswitch)
            )
        };
        let reloading = |v: i32| {
            matches!(
                state(v),
                Some(
                    S::Reloading
                        | S::ReloadStart
                        | S::ReloadEnd
                        | S::ReloadingInterrupt
                        | S::ReloadStartInterrupt
                )
            )
        };
        let mut events: Vec<&str> = Vec::new();
        if dropping(st) && !dropping(last_s) {
            events.push("weapon_switch_started");
        }
        if w != last_w && w != 0 {
            events.push("weapon_change");
        }
        if raising(last_s) && !raising(st) && !dropping(st) {
            events.push("weapon_change_complete");
        }
        if w == last_w && clip < last_clip && !reloading(st) {
            events.push("weapon_fired");
        }
        if reloading(st) && !reloading(last_s) {
            events.push("reload_start");
        }
        if reloading(last_s) && !reloading(st) && clip > last_clip {
            events.push("reload");
        }
        if let Some(p) = world.resource_mut::<Zm>().players.get_mut(&c) {
            p.last_weapon = w;
            p.last_wstate = st;
            p.last_clip = clip;
        }
        if events.is_empty() {
            continue;
        }
        let name = super::weapon_text(world, w);
        with_vm(world, |vm, world| {
            let n = vm.string(&name);
            for e in events {
                vm.notify_str(world, obj, e, &[n.clone()]);
            }
        });
    }
}
