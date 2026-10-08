//! IW4L_T6_AUTOPLAY=1: a test player for headless runs. Client 0 aims at
//! the nearest zombie it can see (its head box), fires (pressing and letting
//! go, so a pistol fires again), and backs away from one that comes close;
//! an empty magazine reloads itself when it fires. Its commands replace the
//! client's own for the tick. Every five seconds it logs the round, its
//! points and health and the zombies alive.

use std::sync::OnceLock;

use bevy_ecs::prelude::World;
use playerstate_iw4::buttons;

use super::{Zm, frame, with_vm};
use crate::world::ClientId;

pub(crate) fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("IW4L_T6_AUTOPLAY").is_ok_and(|v| v == "1" || v == "2"))
}

/// IW4L_T6_AUTOPLAY=2: the rounds test. The player takes no damage, is
/// given IW4L_T6_AUTOPLAY_GUN (the M14 unless named) at 8 s and never runs
/// out of ammo, so the rounds go on and the game's rules get exercised.
pub(crate) fn rounds_test() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("IW4L_T6_AUTOPLAY").is_ok_and(|v| v == "2"))
}

/// BO2MP_AUTOPLAY=1: the multiplayer test player. Client 0 aims at the
/// nearest enemy player it can see and fires, reloading when empty; with
/// none in sight it faces the nearest. It does not move. (So a headless run
/// shows his hit markers, score popups and kills.)
pub(crate) fn mp_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("BO2MP_AUTOPLAY").is_ok_and(|v| v == "1"))
}

/// The other side's living players: their chests.
fn enemies(world: &mut World, me: ClientId) -> Vec<[f32; 3]> {
    let objs: Vec<(u32, gsc_t6::ObjRef)> = world.resource::<Zm>().players.iter().map(|(c, p)| (*c, p.obj)).collect();
    let teams: Vec<(u32, String)> = with_vm(world, |vm, _| {
        let f = vm.intern("team");
        objs.iter().map(|(c, o)| (*c, vm.to_text(&vm.raw_field(*o, f)))).collect()
    })
    .unwrap_or_default();
    let mine = teams.iter().find(|(c, _)| *c == me.0).map(|(_, t)| t.clone()).unwrap_or_default();
    let f = frame(world);
    teams
        .iter()
        .filter(|(c, t)| *c != me.0 && *t != mine && (t == "allies" || t == "axis"))
        .filter_map(|(c, _)| {
            let p = f.player(ClientId(*c))?;
            (p.health > 0).then(|| [p.origin[0], p.origin[1], p.origin[2] + 44.0])
        })
        .collect()
}

fn drive_mp(world: &mut World, now: i64) {
    let client = ClientId(0);
    let Some(ps) = frame(world).player(client).copied() else {
        return;
    };
    if ps.health <= 0 {
        return;
    }
    let eye = [ps.origin[0], ps.origin[1], ps.origin[2] + ps.view_height_current];
    let targets = enemies(world, client);
    let mut seen: Vec<(f32, [f32; 3])> = Vec::new();
    for aim in &targets {
        let t = frame(world).trace_static_world(eye, *aim, [0.0; 3], [0.0; 3], crate::bullet_collision::MASK_SHOT);
        if t.fraction >= 0.98 {
            seen.push((flat_dist(*aim, ps.origin), *aim));
        }
    }
    seen.sort_by(|a, b| a.0.total_cmp(&b.0));
    let target = seen.first().map(|s| s.1);
    let fire_phase = {
        let mut zm = world.resource_mut::<Zm>();
        zm.autoplay_fire = !zm.autoplay_fire;
        zm.autoplay_fire
    };
    // None in sight: face the nearest one (level), ready when he comes
    // round the corner. (Running at him gets stuck on the houses.)
    let nearest = targets.iter().min_by(|a, b| flat_dist(**a, ps.origin).total_cmp(&flat_dist(**b, ps.origin)));
    let view = match (target, nearest) {
        (Some(t), _) => look(eye, t),
        (None, Some(n)) => [0.0, look(eye, *n)[1], 0.0],
        _ => ps.viewangles,
    };
    let (clip, stock) = {
        let f = frame(world);
        (
            crate::script_player::ammo_clip(&f, client, ps.weapon),
            crate::script_player::ammo_stock(&f, client, ps.weapon),
        )
    };
    let mut press = 0;
    if target.is_some() {
        press |= buttons::ADS;
    }
    if clip == 0 && stock > 0 {
        if fire_phase {
            press |= buttons::RELOAD;
        }
    } else if target.is_some() && ps.f_weapon_pos_frac > 0.9 && fire_phase {
        press |= buttons::ATTACK;
    }
    let mut last_angles = None;
    {
        let mut req = world.resource_mut::<crate::step::StepRequest>();
        for (id, cmd) in &mut req.input.cmds {
            if *id != client {
                continue;
            }
            cmd.buttons = press;
            last_angles = Some(cmd.angles);
        }
    }
    if let Some(angles) = last_angles {
        let mut f = frame(world);
        if let Some(p) = f.player_mut(client) {
            p.delta_angles = std::array::from_fn(|i| {
                let cmd_deg = (angles[i] as u16) as f32 * (360.0 / 65536.0);
                view[i] - cmd_deg
            });
            p.viewangles = view;
        }
    }
    if now % 5000 < i64::from(crate::MATCH_TICK_MS) {
        diag::info!(
            Sim,
            "bo2mp autoplay at {}s: health {}, ammo {clip}+{stock}, enemies alive {}, in sight {}",
            now / 1000,
            ps.health,
            targets.len(),
            seen.len()
        );
    }
}

fn look(from: [f32; 3], to: [f32; 3]) -> [f32; 3] {
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let yaw = d[1].atan2(d[0]).to_degrees();
    let flat = (d[0] * d[0] + d[1] * d[1]).sqrt();
    let pitch = -d[2].atan2(flat).to_degrees();
    [pitch, yaw, 0.0]
}

fn flat_dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Where to shoot a zombie: its chest box, else above its feet.
fn head_of(world: &mut World, n: u32, close: bool) -> Option<[f32; 3]> {
    let (id, origin) = {
        let zm = world.resource::<Zm>();
        (
            zm.presences.by_ent.get(&n).map(|s| s.id),
            zm.ents.get(&n)?.origin,
        )
    };
    let head = id.and_then(|id| {
        let f = frame(world);
        let row = f
            .entity_collision_capabilities()
            .iter()
            .find(|row| row.owner.script_model() == Some(id))?;
        let bones = &row.dobj.as_ref()?.current_collision.as_ref()?.bones;
        // The head up close, else the chest (the biggest box, steady
        // while it walks).
        let (first, second) = if close { (2, 4) } else { (4, 2) };
        bones
            .iter()
            .find(|b| b.part_classification == first)
            .or_else(|| bones.iter().find(|b| b.part_classification == second))
            .map(|b| b.center)
    });
    Some(head.unwrap_or([origin[0], origin[1], origin[2] + 56.0]))
}

pub(crate) fn drive(world: &mut World, now: i64) {
    if mp_enabled() && world.resource::<Zm>().mp {
        drive_mp(world, now);
        return;
    }
    if !enabled() {
        return;
    }
    let client = ClientId(0);
    if rounds_test() {
        rounds_kit(world, now, client);
    }
    let Some(ps) = frame(world).player(client).copied() else {
        return;
    };
    let eye = [
        ps.origin[0],
        ps.origin[1],
        ps.origin[2] + ps.view_height_current,
    ];
    let living: Vec<(u32, [f32; 3])> = {
        let zm = world.resource::<Zm>();
        zm.actors
            .iter()
            .filter(|(_, a)| a.alive && a.scripted.is_none())
            .filter_map(|(n, _)| zm.ents.get(n).map(|e| (*n, e.origin)))
            .collect()
    };
    // The nearest one in sight.
    let mut seen: Vec<(f32, [f32; 3])> = Vec::new();
    for (n, origin) in &living {
        let close = flat_dist(*origin, ps.origin) < 350.0;
        let Some(head) = head_of(world, *n, close) else {
            continue;
        };
        let t = frame(world).trace_static_world(
            eye,
            head,
            [0.0; 3],
            [0.0; 3],
            crate::bullet_collision::MASK_SHOT,
        );
        if t.fraction >= 0.98 {
            seen.push((flat_dist(*origin, ps.origin), head));
        }
    }
    seen.sort_by(|a, b| a.0.total_cmp(&b.0));
    let target = seen.first().map(|s| s.1);
    let closest = living
        .iter()
        .map(|(_, o)| (flat_dist(*o, ps.origin), *o))
        .min_by(|a, b| a.0.total_cmp(&b.0));
    let fire_phase = {
        let mut zm = world.resource_mut::<Zm>();
        zm.autoplay_fire = !zm.autoplay_fire;
        zm.autoplay_fire
    };
    let view = target.map_or(ps.viewangles, |t| look(eye, t));
    let (clip, stock) = {
        let f = frame(world);
        (
            crate::script_player::ammo_clip(&f, client, ps.weapon),
            crate::script_player::ammo_stock(&f, client, ps.weapon),
        )
    };
    // Aim down the sights at one in range; shoot it when near enough to hit.
    let range = seen.first().map_or(f32::MAX, |s| s.0);
    let mut press = 0;
    if range < 700.0 {
        press |= buttons::ADS;
    }
    let close = closest.is_some_and(|(d, _)| d < 64.0);
    if close {
        // Knife one in reach (a fresh press each time).
        if fire_phase {
            press |= weapon_iw4::BUTTON_MELEE;
        }
    } else if clip == 0 && stock > 0 {
        // Empty: reload (pressed every other tick, as a player taps it).
        if fire_phase {
            press |= buttons::RELOAD;
        }
    } else if target.is_some() && range < 450.0 && ps.f_weapon_pos_frac > 0.9 && fire_phase {
        press |= buttons::ATTACK;
    }
    // Back away from one that is close.
    let (mut forward, mut right) = (0i8, 0i8);
    if let Some((d, o)) = closest
        && (64.0..110.0).contains(&d)
    {
        let wish = [ps.origin[0] - o[0], ps.origin[1] - o[1]];
        let len = (wish[0] * wish[0] + wish[1] * wish[1]).sqrt().max(1e-3);
        let (s, c) = view[1].to_radians().sin_cos();
        let (fw, rt) = ([c, s], [s, -c]);
        forward = (((wish[0] * fw[0] + wish[1] * fw[1]) / len) * 127.0).round() as i8;
        right = (((wish[0] * rt[0] + wish[1] * rt[1]) / len) * 127.0).round() as i8;
    }
    // The view turns the way a script's setplayerangles turns it: the delta
    // under the client's own command angles, so the client's camera follows.
    let mut last_angles = None;
    {
        let mut req = world.resource_mut::<crate::step::StepRequest>();
        for (id, cmd) in &mut req.input.cmds {
            if *id != client {
                continue;
            }
            cmd.buttons = press;
            cmd.forwardmove = forward;
            cmd.rightmove = right;
            last_angles = Some(cmd.angles);
        }
    }
    if let Some(angles) = last_angles {
        let mut f = frame(world);
        if let Some(p) = f.player_mut(client) {
            p.delta_angles = std::array::from_fn(|i| {
                let cmd_deg = (angles[i] as u16) as f32 * (360.0 / 65536.0);
                view[i] - cmd_deg
            });
            p.viewangles = view;
        }
    }
    // A status line every five seconds.
    if now % 5000 < i64::from(crate::MATCH_TICK_MS) {
        let (round, score) = with_vm(world, |vm, world| {
            let level = vm.level;
            let round = {
                let f = vm.intern("round_number");
                vm.raw_field(level, f)
            };
            let score = world
                .resource::<Zm>()
                .players
                .get(&0)
                .map(|p| p.obj)
                .map(|o| {
                    let f = vm.intern("score");
                    vm.get_field(world, o, f)
                })
                .unwrap_or_default();
            (vm.to_text(&round), vm.to_text(&score))
        })
        .unwrap_or_default();
        diag::info!(
            Sim,
            "bo2zm t6 autoplay at {}s: round {round}, points {score}, health {}, ammo {clip}+{stock}, zombies alive {}, in sight {}",
            now / 1000,
            ps.health,
            living.len(),
            seen.len()
        );
    }
}

/// IW4L_T6_CENSUS=1: every 10 s, each living zombie: state, animation,
/// place, distance to the player, path progress and how far it moved in the
/// last 10 s (a stuck one barely moves far from him).
pub(crate) fn census_test(world: &mut World, now: i64) {
    static ON: OnceLock<bool> = OnceLock::new();
    static LAST: std::sync::Mutex<Option<std::collections::BTreeMap<u32, [f32; 3]>>> =
        std::sync::Mutex::new(None);
    if !*ON.get_or_init(|| std::env::var_os("IW4L_T6_CENSUS").is_some()) {
        return;
    }
    if now % 10_000 >= i64::from(crate::MATCH_TICK_MS) {
        return;
    }
    // Once: each move state's animations and their root-motion speeds.
    static SPEEDS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !SPEEDS.swap(true, std::sync::atomic::Ordering::Relaxed) {
        let zm = world.resource::<Zm>();
        for (file, def) in &zm.asds {
            for (name, st) in &def.states {
                if !name.starts_with("zm_move_") {
                    continue;
                }
                let anims: Vec<String> = st
                    .substates
                    .iter()
                    .map(|(_, a)| {
                        let sp = zm.anims.get(a).map_or(-1.0, super::actors::anim_speed);
                        format!("{a} {sp:.0}")
                    })
                    .collect();
                diag::info!(
                    Sim,
                    "bo2zm t6 census speeds {file}/{name}: {}",
                    anims.join(", ")
                );
            }
        }
    }
    let Some(me) = frame(world).player(ClientId(0)).map(|p| p.origin) else {
        return;
    };
    let mut last = LAST.lock().unwrap();
    let prev = last.get_or_insert_with(Default::default);
    // Each zombie's walk/run/sprint (the scripts' zombie_move_speed).
    let speeds: std::collections::BTreeMap<u32, String> = with_vm(world, |vm, world| {
        let f = vm.intern("zombie_move_speed");
        let zm = world.resource::<Zm>();
        let objs: Vec<(u32, _)> = zm
            .actors
            .iter()
            .filter(|(_, a)| a.alive)
            .filter_map(|(n, _)| Some((*n, zm.ents.get(n)?.obj?)))
            .collect();
        objs.into_iter()
            .map(|(n, o)| (n, vm.to_text(&vm.raw_field(o, f))))
            .collect()
    })
    .unwrap_or_default();
    let rows: Vec<String> = {
        let zm = world.resource::<Zm>();
        zm.actors
            .iter()
            .filter(|(_, a)| a.alive)
            .filter_map(|(n, a)| {
                let e = zm.ents.get(n)?;
                let moved = prev.get(n).map_or(-1.0, |p| flat_dist(*p, e.origin));
                Some(format!(
                    "{n} {} {} {} at ({:.0} {:.0} {:.0}) dist {:.0} moved {:.0} path {}/{} goal {:?}",
                    speeds.get(n).map_or("?", String::as_str),
                    a.script,
                    a.playing.as_ref().map_or("-", |p| p.anim.as_str()),
                    e.origin[0],
                    e.origin[1],
                    e.origin[2],
                    flat_dist(e.origin, me),
                    moved,
                    a.path_i,
                    a.path.len(),
                    a.goal.map(|g| [g[0].round(), g[1].round(), g[2].round()])
                ) + &if (0.0..24.0).contains(&moved) && a.script == "move" {
                    // A stalled one: where it is heading next.
                    let pts: Vec<String> = a
                        .path
                        .iter()
                        .skip(a.path_i)
                        .take(3)
                        .map(|p| format!("({:.0} {:.0} {:.0})", p[0], p[1], p[2]))
                        .collect();
                    format!(" next {}", pts.join(" "))
                } else {
                    String::new()
                })
            })
            .collect()
    };
    {
        let zm = world.resource::<Zm>();
        *prev = zm
            .actors
            .iter()
            .filter(|(_, a)| a.alive)
            .filter_map(|(n, _)| zm.ents.get(n).map(|e| (*n, e.origin)))
            .collect();
    }
    diag::info!(
        Sim,
        "bo2zm t6 census {now}: me ({:.0} {:.0} {:.0}), {} alive | {}",
        me[0],
        me[1],
        me[2],
        rows.len(),
        rows.join(" | ")
    );
}

/// IW4L_T6_ROAM="x y z;x y z;...": from 12 s the player is moved to the
/// next of those places every IW4L_T6_ROAM_SECS (20), round and round, as a
/// player who keeps changing where he stands (do the zombies follow).
pub(crate) fn roam_test(world: &mut World, now: i64) {
    static SPOTS: OnceLock<Vec<[f32; 3]>> = OnceLock::new();
    let spots = SPOTS.get_or_init(|| {
        std::env::var("IW4L_T6_ROAM")
            .ok()
            .map(|v| {
                v.split(';')
                    .filter_map(|p| {
                        let n: Vec<f32> = p
                            .split_whitespace()
                            .filter_map(|x| x.parse().ok())
                            .collect();
                        (n.len() == 3).then(|| [n[0], n[1], n[2]])
                    })
                    .collect()
            })
            .unwrap_or_default()
    });
    if spots.is_empty() || now < 12_000 {
        return;
    }
    let period = std::env::var("IW4L_T6_ROAM_SECS")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(20)
        * 1000;
    let since = now - 12_000;
    if since % period >= i64::from(crate::MATCH_TICK_MS) {
        return;
    }
    let spot = spots[(since / period) as usize % spots.len()];
    super::teleport_player(world, ClientId(0), spot);
    diag::info!(
        Sim,
        "bo2zm t6 roam at {}s: player to ({:.0} {:.0} {:.0})",
        now / 1000,
        spot[0],
        spot[1],
        spot[2]
    );
}

/// IW4L_T6_ONLY_SPAWN="x y": every zombie spawns at the active spawn spot
/// nearest x y (the scripts' spawn list cut to that one each tick).
pub(crate) fn only_spawn_test(world: &mut World) {
    static AT: OnceLock<Option<[f32; 2]>> = OnceLock::new();
    let Some(at) = *AT.get_or_init(|| {
        let v: Vec<f32> = std::env::var("IW4L_T6_ONLY_SPAWN")
            .ok()?
            .split_whitespace()
            .filter_map(|x| x.parse().ok())
            .collect();
        (v.len() == 2).then(|| [v[0], v[1]])
    }) else {
        return;
    };
    with_vm(world, |vm, _| {
        let (list_f, origin_f) = (vm.intern("zombie_spawn_locations"), vm.intern("origin"));
        let gsc_t6::Value::Array(arr) = vm.raw_field(vm.level, list_f) else {
            return;
        };
        let spots: Vec<gsc_t6::Value> = arr.snapshot().values_in_order().cloned().collect();
        let best = spots
            .iter()
            .filter_map(|v| {
                let gsc_t6::Value::Object(o) = v else {
                    return None;
                };
                let p = vm.raw_field(*o, origin_f).as_vec3()?;
                Some(((p[0] - at[0]).powi(2) + (p[1] - at[1]).powi(2), v.clone()))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((_, v)) = best
            && spots.len() > 1
        {
            let mut one = gsc_t6::Array::new();
            one.push(v);
            vm.set_raw_field(vm.level, list_f, gsc_t6::Value::array(one));
        }
    });
}

/// IW4L_T6_GLIDE="x y;x y;...": from 12 s the player glides from place to
/// place at 120 units/s over the ground, resting 5 s at each, round and
/// round: a player who walks about the map (do the zombies keep up).
pub(crate) fn glide_test(world: &mut World, now: i64) {
    static SPOTS: OnceLock<Vec<[f32; 2]>> = OnceLock::new();
    static AT: std::sync::Mutex<(usize, i64)> = std::sync::Mutex::new((0, 0));
    let spots = SPOTS.get_or_init(|| {
        std::env::var("IW4L_T6_GLIDE")
            .ok()
            .map(|v| {
                v.split(';')
                    .filter_map(|p| {
                        let n: Vec<f32> = p
                            .split_whitespace()
                            .filter_map(|x| x.parse().ok())
                            .collect();
                        (n.len() == 2).then(|| [n[0], n[1]])
                    })
                    .collect()
            })
            .unwrap_or_default()
    });
    if spots.is_empty() || now < 12_000 {
        return;
    }
    let Some(me) = frame(world).player(ClientId(0)).map(|p| p.origin) else {
        return;
    };
    let mut at = AT.lock().unwrap();
    if now < at.1 {
        return;
    }
    let target = spots[at.0 % spots.len()];
    let d = [target[0] - me[0], target[1] - me[1]];
    let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
    let step = 120.0 * crate::MATCH_TICK_MS as f32 / 1000.0;
    if len <= step {
        at.0 += 1;
        at.1 = now + 5000;
        diag::info!(
            Sim,
            "bo2zm t6 glide at {}s: reached ({:.0} {:.0})",
            now / 1000,
            target[0],
            target[1]
        );
        return;
    }
    let next = [
        me[0] + d[0] / len * step,
        me[1] + d[1] / len * step,
        me[2] + 40.0,
    ];
    // Onto the ground below (stairs and porches up, yards down).
    let down = [next[0], next[1], next[2] - 400.0];
    let t = frame(world).trace_static_world(
        next,
        down,
        crate::bullet_collision::PLAYER_MINS,
        crate::bullet_collision::PLAYER_MAXS,
        crate::bullet_collision::MASK_PLAYER_SOLID,
    );
    let feet: [f32; 3] = std::array::from_fn(|i| next[i] + (down[i] - next[i]) * t.fraction);
    super::teleport_player(world, ClientId(0), feet);
}

/// IW4L_T6_OPENALL=1: at 5 s every door and debris pile opens (the zone
/// flags), so the whole map is in play (test aid).
pub(crate) fn open_all_test(world: &mut World, now: i64) {
    static ON: OnceLock<bool> = OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("IW4L_T6_OPENALL").is_some()) {
        return;
    }
    let tick = i64::from(crate::MATCH_TICK_MS);
    // IW4L_T6_OPENALL=buy: each door and debris pile is bought instead, by
    // its own script (paths joined, doors swung, as when he buys them);
    // buy:<flag>,<flag>: only those (the doors' script_flag).
    let mode = std::env::var("IW4L_T6_OPENALL").unwrap_or_default();
    let buy = mode == "buy" || mode.starts_with("buy:");
    let only: Vec<String> = mode
        .strip_prefix("buy:")
        .map(|l| l.split(',').map(str::to_owned).collect())
        .unwrap_or_default();
    if !buy && (5000..5000 + tick).contains(&now) {
        open_all_zones(world);
    }
    if buy && (5000..5000 + tick).contains(&now) {
        give_points(world, 100_000);
    }
    // One a tick from 5.5 s (a door's script waits on its own trigger).
    if buy && (5500..8500).contains(&now) {
        let i = ((now - 5500) / tick) as usize;
        with_vm(world, |vm, world| {
            let (tn, flag_f) = (vm.intern("targetname"), vm.intern("script_flag"));
            let zm = world.resource::<Zm>();
            let mut trigs: Vec<(u32, gsc_t6::ObjRef)> = zm
                .ents
                .iter()
                .filter(|(_, e)| e.classname.starts_with("trigger_use"))
                .filter_map(|(n, e)| {
                    let o = e.obj?;
                    let name = vm.to_text(&vm.raw_field(o, tn));
                    let flag = vm.to_text(&vm.raw_field(o, flag_f));
                    ((name == "zombie_door" || name == "zombie_debris")
                        && (only.is_empty() || only.contains(&flag)))
                    .then_some((*n, o))
                })
                .collect();
            trigs.sort_by_key(|t| t.0);
            let player = zm.players.get(&0).map(|p| p.obj);
            if let (Some((n, t)), Some(p)) = (trigs.get(i).copied(), player) {
                diag::info!(Sim, "bo2zm t6 open all: buying ent{n}");
                // (who, force): forced, as the scripts' own open-all does.
                vm.notify_str(
                    world,
                    t,
                    "trigger",
                    &[gsc_t6::Value::Object(p), gsc_t6::Value::Int(1)],
                );
            }
        });
    }
}

/// IW4L_T6_BUY=<wall weapon> (e.g. m14_zm): at 5 s the doors open and he
/// gets 10000 points; at 6 s the player is put in front of that wall buy,
/// looking at it, and holds use from 7 to 7.5 s; at 9 s his weapons and
/// points are logged.
pub(crate) fn buy_test(world: &mut World, now: i64) {
    static WANT: OnceLock<Option<String>> = OnceLock::new();
    let Some(want) = WANT
        .get_or_init(|| std::env::var("IW4L_T6_BUY").ok())
        .clone()
    else {
        return;
    };
    let client = ClientId(0);
    let tick = i64::from(crate::MATCH_TICK_MS);
    if (5000..5000 + tick).contains(&now) {
        open_all_zones(world);
        give_points(world, 10000);
    }
    if (6000..6000 + tick).contains(&now) {
        // The wall buy's struct.
        let spot = with_vm(world, |vm, _| {
            let structs = {
                let f = vm.intern("struct");
                vm.raw_field(vm.level, f)
            };
            let gsc_t6::Value::Array(arr) = structs else {
                return None;
            };
            let key = vm.intern("zombie_weapon_upgrade");
            let (o, a) = (vm.intern("origin"), vm.intern("angles"));
            for v in arr.snapshot().values_in_order() {
                let gsc_t6::Value::Object(s) = v else {
                    continue;
                };
                let name = vm.raw_field(*s, key);
                if vm.to_text(&name) == want {
                    let origin = vm.raw_field(*s, o).as_vec3()?;
                    let angles = vm.raw_field(*s, a).as_vec3().unwrap_or([0.0; 3]);
                    return Some((origin, angles));
                }
            }
            None
        })
        .flatten();
        let Some((origin, angles)) = spot else {
            diag::warn!(Sim, "bo2zm t6 buy test: no wall buy {want}");
            return;
        };
        // A wall buy faces its struct's left (the scripts' require_look_from
        // test: the eye must be on the side of -anglestoright).
        let (_, rt, _) = gsc_t6::math::angle_vectors([0.0, angles[1], 0.0]);
        // IW4L_T6_BUY_FLIP=1: from its right instead; IW4L_T6_BUY_DIST=<u>:
        // that far from it (18).
        let dist = std::env::var("IW4L_T6_BUY_DIST")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(18.0);
        let side = if std::env::var_os("IW4L_T6_BUY_FLIP").is_some() {
            -dist
        } else {
            dist
        };
        let stand = [
            origin[0] - rt[0] * side,
            origin[1] - rt[1] * side,
            origin[2] + 8.0,
        ];
        let down = [stand[0], stand[1], stand[2] - 300.0];
        let t = frame(world).trace_static_world(
            stand,
            down,
            crate::bullet_collision::PLAYER_MINS,
            crate::bullet_collision::PLAYER_MAXS,
            crate::bullet_collision::MASK_PLAYER_SOLID,
        );
        let feet: [f32; 3] = std::array::from_fn(|i| stand[i] + (down[i] - stand[i]) * t.fraction);
        let eye = [feet[0], feet[1], feet[2] + 60.0];
        let view = look(eye, origin);
        super::teleport_player(world, client, feet);
        super::set_player_view(world, client, view);
        diag::info!(
            Sim,
            "bo2zm t6 buy test: {want} at ({:.0} {:.0} {:.0}); player at ({:.0} {:.0} {:.0}) looking {:.0} {:.0}",
            origin[0],
            origin[1],
            origin[2],
            feet[0],
            feet[1],
            feet[2],
            view[0],
            view[1]
        );
    }
    if (7000..7500).contains(&now) {
        let mut req = world.resource_mut::<crate::step::StepRequest>();
        for (id, cmd) in &mut req.input.cmds {
            if *id == client {
                cmd.buttons |= buttons::USE;
            }
        }
    }
    if (9000..9000 + tick).contains(&now) {
        let names: Vec<String> = {
            let f = frame(world);
            crate::script_player::weapons(&f, client, crate::script_player::WeaponList::All)
                .into_iter()
                .map(|w| crate::script_player::weapon_name(&f, w))
                .collect()
        };
        let hint = world
            .resource::<Zm>()
            .hints
            .get(&0)
            .cloned()
            .unwrap_or_default();
        diag::info!(Sim, "bo2zm t6 buy test: weapons {names:?}, hint {hint:?}");
    }
}

/// IW4L_T6_BOX=1: the magic box. At 12 s the player gets 10000 points and
/// is put on the active box's use side, looking at it; he presses use at
/// 13 s (open, 950) and again at 19.5 s (take the weapon); at 22 s his
/// weapons and points are logged.
pub(crate) fn box_test(world: &mut World, now: i64) {
    static ON: OnceLock<bool> = OnceLock::new();
    if !*ON.get_or_init(|| std::env::var("IW4L_T6_BOX").is_ok_and(|v| v != "move" && v != "bear")) {
        return;
    }
    let client = ClientId(0);
    let tick = i64::from(crate::MATCH_TICK_MS);
    if (11000..11000 + tick).contains(&now) {
        open_all_zones(world);
    }
    if (12000..12000 + tick).contains(&now) {
        give_points(world, 10000);
        let Some(origin) = stand_at_box(world, client) else {
            diag::warn!(Sim, "bo2zm t6 box test: no active box");
            return;
        };
        diag::info!(
            Sim,
            "bo2zm t6 box test: box at ({:.0} {:.0} {:.0})",
            origin[0],
            origin[1],
            origin[2]
        );
    }
    // IW4L_T6_BOX=look: he only stands there (the box's light, closed).
    let look_only = std::env::var("IW4L_T6_BOX").is_ok_and(|v| v == "look");
    if !look_only && ((13000..13300).contains(&now) || (19500..19800).contains(&now)) {
        press_use(world, client);
    }
    if (22000..22000 + tick).contains(&now) {
        let names: Vec<String> = {
            let f = frame(world);
            crate::script_player::weapons(&f, client, crate::script_player::WeaponList::All)
                .into_iter()
                .map(|w| crate::script_player::weapon_name(&f, w))
                .collect()
        };
        let score = frame(world).client_meta(client).map_or(0, |m| m.score);
        diag::info!(Sim, "bo2zm t6 box test: weapons {names:?}, points {score}");
    }
}

/// The doors and debris opened (the box may start behind them): the zone
/// flags Nuketown's own zone init names.
fn open_all_zones(world: &mut World) {
    with_vm(world, |vm, world| {
        let level = gsc_t6::Value::Object(vm.level);
        for f in [
            "culdesac_2_openhouse2_f1",
            "openhouse2_f1_2_openhouse2_backyard",
            "openhouse2_f1_2_openhouse2_f2",
            "openhouse2_backyard_2_openhouse2_f2",
            "culdesac_2_openhouse1_f1",
            "openhouse1_f1_2_openhouse1_backyard",
            "openhouse1_f2_openhouse1_f1",
            "openhouse1_backyard_2_openhouse1_f2",
            "culdesac_2_truck",
            "openhouse2_backyard_2_ammo_door",
        ] {
            let name = vm.string(f);
            vm.spawn_named(
                world,
                "common_scripts/utility",
                "flag_set",
                level.clone(),
                vec![name],
            );
        }
    });
}

/// Player 0's points set (the scripts' `score`).
fn give_points(world: &mut World, points: i32) {
    with_vm(world, |vm, world| {
        if let Some(p) = world.resource::<Zm>().players.get(&0).map(|p| p.obj) {
            let f = vm.intern("score");
            vm.set_field(world, p, f, gsc_t6::Value::Int(points));
        }
    });
}

/// Use held for this tick.
fn press_use(world: &mut World, client: ClientId) {
    let mut req = world.resource_mut::<crate::step::StepRequest>();
    for (id, cmd) in &mut req.input.cmds {
        if *id == client {
            cmd.buttons |= buttons::USE;
        }
    }
}

/// The box that can be used (its zbarrier's script `state` is "initial",
/// "open" or "close", or "arriving" once its lid piece has opened; "away"
/// is the empty spot, "leaving" the move): where it stands.
fn active_box(world: &mut World) -> Option<([f32; 3], [f32; 3])> {
    with_vm(world, |vm, world| {
        let state = vm.intern("state");
        let zm = world.resource::<Zm>();
        zm.ents.values().find_map(|e| {
            let z = e.zbarrier.as_ref()?;
            let s = vm.to_text(&vm.raw_field(e.obj?, state));
            let arrived = s == "arriving" && z.pieces.get(1).is_some_and(|p| p.state == "open");
            (matches!(s.as_str(), "initial" | "open" | "close") || arrived)
                .then_some((e.origin, e.angles))
        })
    })
    .flatten()
}

/// The player put on the showing box's use side, looking at it; the box's
/// place.
fn stand_at_box(world: &mut World, client: ClientId) -> Option<[f32; 3]> {
    let (origin, angles) = active_box(world)?;
    let (_, rt, _) = gsc_t6::math::angle_vectors([0.0, angles[1], 0.0]);
    let stand = [
        origin[0] - rt[0] * 48.0,
        origin[1] - rt[1] * 48.0,
        origin[2] + 20.0,
    ];
    let down = [stand[0], stand[1], stand[2] - 200.0];
    let t = frame(world).trace_static_world(
        stand,
        down,
        crate::bullet_collision::PLAYER_MINS,
        crate::bullet_collision::PLAYER_MAXS,
        crate::bullet_collision::MASK_PLAYER_SOLID,
    );
    let feet: [f32; 3] = std::array::from_fn(|i| stand[i] + (down[i] - stand[i]) * t.fraction);
    let eye = [feet[0], feet[1], feet[2] + 60.0];
    let view = look(eye, [origin[0], origin[1], origin[2] + 20.0]);
    super::teleport_player(world, client, feet);
    super::set_player_view(world, client, view);
    Some(origin)
}

/// IW4L_T6_BOX=move: the box until the teddy bear moves it (run with
/// IW4L_T6_GOD=1). From 12 s, every 9 s: 10000 points, the player at the
/// box that is showing, use (open), use 6 s later (take); 8.5 s in, the gun
/// in hand, the box's place and the scripts' `level.chest_accessed` /
/// `level.chest_moves` are logged. While the box is away nothing is pressed.
pub(crate) fn box_move_test(world: &mut World, now: i64) {
    static ON: OnceLock<bool> = OnceLock::new();
    if !*ON.get_or_init(|| std::env::var("IW4L_T6_BOX").is_ok_and(|v| v == "move" || v == "bear")) {
        return;
    }
    let client = ClientId(0);
    let tick = i64::from(crate::MATCH_TICK_MS);
    if (11000..11000 + tick).contains(&now) {
        open_all_zones(world);
        // IW4L_T6_BOX=bear: the first use brings the teddy bear (the
        // scripts' rule: eight uses and no move yet).
        if std::env::var("IW4L_T6_BOX").is_ok_and(|v| v == "bear") {
            with_vm(world, |vm, world| {
                let f = vm.intern("chest_accessed");
                let level = vm.level;
                vm.set_field(world, level, f, gsc_t6::Value::Int(8));
            });
        }
    }
    if now < 12000 {
        return;
    }
    let t = (now - 12000) % 9000;
    if t < tick {
        give_points(world, 10000);
        if stand_at_box(world, client).is_none() {
            diag::info!(
                Sim,
                "bo2zm t6 box move test at {}s: no box showing",
                now / 1000
            );
        }
    }
    if active_box(world).is_some() && ((1000..1300).contains(&t) || (7000..7300).contains(&t)) {
        press_use(world, client);
    }
    if (8500..8500 + tick).contains(&t) {
        let (accessed, moves) = with_vm(world, |vm, _| {
            let a = vm.intern("chest_accessed");
            let m = vm.intern("chest_moves");
            (
                vm.raw_field(vm.level, a).as_int(),
                vm.raw_field(vm.level, m).as_int(),
            )
        })
        .unwrap_or((None, None));
        let gun = {
            let f = frame(world);
            f.player(client)
                .map(|p| crate::script_player::weapon_name(&f, p.weapon))
                .unwrap_or_default()
        };
        let place =
            active_box(world).map(|(o, _)| format!("({:.0} {:.0} {:.0})", o[0], o[1], o[2]));
        diag::info!(
            Sim,
            "bo2zm t6 box move test at {}s: gun {gun}, box {}, chest_accessed {accessed:?}, chest_moves {moves:?}",
            now / 1000,
            place.unwrap_or_else(|| "away".to_owned())
        );
    }
}

/// IW4L_T6_DWTEST=<weapon> (e.g. fivesevendw_zm): a dual-wield check. At 8 s
/// the weapon is given; from 12 s, one tick of attack (the left gun) every
/// 0.5 s for 2 s, then one tick of the aim button (the right gun) every
/// 0.5 s for 2 s, then reload at 17 s; at 11.9, 14.1, 16.1 and 19 s the
/// clips (right, left) and stock are logged.
pub(crate) fn dual_wield_test(world: &mut World, now: i64) {
    static WANT: OnceLock<Option<String>> = OnceLock::new();
    let Some(want) = WANT
        .get_or_init(|| std::env::var("IW4L_T6_DWTEST").ok())
        .clone()
    else {
        return;
    };
    let client = ClientId(0);
    let tick = i64::from(crate::MATCH_TICK_MS);
    if (8000..8000 + tick).contains(&now) {
        with_vm(world, |vm, world| {
            if let Some(p) = world.resource::<Zm>().players.get(&client.0).map(|p| p.obj) {
                let g = vm.string(&want);
                vm.spawn_named(
                    world,
                    "maps/mp/zombies/_zm_weapons",
                    "weapon_give",
                    gsc_t6::Value::Object(p),
                    vec![g],
                );
            }
        });
    }
    let button = if (12000..14000).contains(&now) && (now - 12000) % 500 < tick {
        Some(buttons::ATTACK)
    } else if (14200..16200).contains(&now) && (now - 14200) % 500 < tick {
        Some(buttons::THROW)
    } else if (17000..17000 + tick).contains(&now) {
        Some(buttons::RELOAD)
    } else {
        None
    };
    if let Some(b) = button {
        let mut req = world.resource_mut::<crate::step::StepRequest>();
        for (id, cmd) in &mut req.input.cmds {
            if *id == client {
                cmd.buttons |= b;
            }
        }
    }
    // Each change of either hand's animation while the test fires.
    if (11900..19000).contains(&now) {
        static LAST: std::sync::Mutex<(i32, i32)> = std::sync::Mutex::new((-1, -1));
        let anims = frame(world)
            .player(client)
            .map(|ps| (ps.weap_anim, ps.weap_anim_secondary));
        if let (Some(anims), Ok(mut last)) = (anims, LAST.lock())
            && *last != anims
        {
            diag::info!(
                Sim,
                "bo2zm t6 dual test at {now}ms: animations right {:#x} left {:#x}",
                anims.0,
                anims.1
            );
            *last = anims;
        }
    }
    for at in [11900, 14100, 16100, 19000] {
        if (at..at + tick).contains(&now) {
            let f = frame(world);
            let Some(ps) = f.player(client) else { return };
            let w = ps.weapon;
            let right = crate::script_player::ammo_clip(&f, client, w);
            let left = crate::script_player::left_clip(&f, client, w);
            let stock = crate::script_player::ammo_stock(&f, client, w);
            diag::info!(
                Sim,
                "bo2zm t6 dual test at {}ms: {} hands {} clips right {right} left {left} stock {stock} anims {}/{}",
                now,
                crate::script_player::weapon_name(&f, w),
                ps.last_weapon_hand + 1,
                ps.weap_anim,
                ps.weap_anim_secondary
            );
        }
    }
}

/// IW4L_T6_DOOR=<script_flag> (e.g. culdesac_2_openhouse1_f1): a door or
/// debris pile. At 6 s the player gets 10000 points and stands in its use
/// trigger, looking at what it opens; he holds use from 7 to 7.5 s; at 9 s
/// his points, the level flag and the door's place are logged.
pub(crate) fn door_test(world: &mut World, now: i64) {
    static WANT: OnceLock<Option<String>> = OnceLock::new();
    let Some(want) = WANT
        .get_or_init(|| std::env::var("IW4L_T6_DOOR").ok())
        .clone()
    else {
        return;
    };
    let client = ClientId(0);
    let tick = i64::from(crate::MATCH_TICK_MS);
    // The trigger and what it moves (its target's place).
    let find = |world: &mut World| -> Option<(super::Ent, Option<[f32; 3]>)> {
        with_vm(world, |vm, world| {
            let (flag_f, tn, target) = (
                vm.intern("script_flag"),
                vm.intern("targetname"),
                vm.intern("target"),
            );
            let zm = world.resource::<Zm>();
            let trig = zm.ents.values().find(|e| {
                e.classname.starts_with("trigger_use")
                    && e.obj.is_some_and(|o| {
                        let name = vm.to_text(&vm.raw_field(o, tn));
                        (name == "zombie_door" || name == "zombie_debris")
                            && vm.to_text(&vm.raw_field(o, flag_f)) == want
                    })
            })?;
            let aim = trig.obj.and_then(|o| {
                let t = vm.to_text(&vm.raw_field(o, target));
                zm.ents
                    .values()
                    .find(|e| {
                        e.obj
                            .is_some_and(|eo| vm.to_text(&vm.raw_field(eo, tn)) == t)
                    })
                    .map(|e| e.origin)
            });
            Some((trig.clone(), aim))
        })
        .flatten()
    };
    if (6000..6000 + tick).contains(&now) {
        give_points(world, 10000);
        let Some((trig, aim)) = find(world) else {
            diag::warn!(Sim, "bo2zm t6 door test: no door or debris for {want}");
            return;
        };
        let c = super::triggers::center(world, &trig);
        let stand = [c[0], c[1], c[2] + 40.0];
        let down = [stand[0], stand[1], stand[2] - 300.0];
        let t = frame(world).trace_static_world(
            stand,
            down,
            crate::bullet_collision::PLAYER_MINS,
            crate::bullet_collision::PLAYER_MAXS,
            crate::bullet_collision::MASK_PLAYER_SOLID,
        );
        let feet: [f32; 3] = std::array::from_fn(|i| stand[i] + (down[i] - stand[i]) * t.fraction);
        let eye = [feet[0], feet[1], feet[2] + 60.0];
        let to = aim.unwrap_or(c);
        super::teleport_player(world, client, feet);
        super::set_player_view(world, client, look(eye, [to[0], to[1], to[2] + 40.0]));
        diag::info!(
            Sim,
            "bo2zm t6 door test: {want} trigger at ({:.0} {:.0} {:.0}), opens {:?}; player at ({:.0} {:.0} {:.0})",
            c[0],
            c[1],
            c[2],
            aim.map(|a| a.map(|v| v.round())),
            feet[0],
            feet[1],
            feet[2]
        );
    }
    if (7000..7500).contains(&now) {
        press_use(world, client);
    }
    if (9000..9000 + tick).contains(&now) {
        let score = frame(world).client_meta(client).map_or(0, |m| m.score);
        let flag_on = with_vm(world, |vm, _| {
            let f = vm.intern("flag");
            match vm.raw_field(vm.level, f) {
                gsc_t6::Value::Array(a) => {
                    let k = gsc_t6::Key::Str(vm.intern(&want));
                    a.get(&k).is_some_and(|v| gsc_t6::truthy(&v))
                }
                _ => false,
            }
        })
        .unwrap_or(false);
        let hint = world
            .resource::<Zm>()
            .hints
            .get(&0)
            .cloned()
            .unwrap_or_default();
        diag::info!(
            Sim,
            "bo2zm t6 door test: points {score}, flag {want} {flag_on}, hint {hint:?}"
        );
    }
    // Every piece the door moves (the trigger's target): its place and turn
    // before, during and after the open.
    if [6500, 7100, 7250, 7400, 7600, 8200, 9000, 11000]
        .iter()
        .any(|t| (*t..*t + tick).contains(&now))
    {
        let rows = with_vm(world, |vm, world| {
            let (flag_f, tn, target) = (
                vm.intern("script_flag"),
                vm.intern("targetname"),
                vm.intern("target"),
            );
            let zm = world.resource::<Zm>();
            // A bought debris pile deletes its triggers: the pieces they
            // named are remembered.
            static NAMED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
            let mut targets: Vec<String> = zm
                .ents
                .values()
                .filter_map(|e| {
                    let o = e.obj?;
                    (e.classname.starts_with("trigger_use")
                        && vm.to_text(&vm.raw_field(o, flag_f)) == want)
                        .then(|| vm.to_text(&vm.raw_field(o, target)))
                })
                .collect();
            if let Ok(mut named) = NAMED.lock() {
                if targets.is_empty() {
                    targets = named.clone();
                } else {
                    *named = targets.clone();
                }
            }
            zm.ents
                .iter()
                .filter(|(_, e)| {
                    e.obj.is_some_and(|o| targets.contains(&vm.to_text(&vm.raw_field(o, tn))))
                })
                .map(|(n, e)| {
                    format!(
                        "ent{n} {} {:?} at ({:.0} {:.0} {:.0}) angles ({:.0} {:.0} {:.0}) hidden {}",
                        e.classname,
                        if e.model.is_empty() { e.brush.clone().unwrap_or_default() } else { e.model.clone() },
                        e.origin[0],
                        e.origin[1],
                        e.origin[2],
                        e.angles[0],
                        e.angles[1],
                        e.angles[2],
                        e.hidden
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
        diag::info!(Sim, "bo2zm t6 door test at {now}ms: {}", rows.join(" | "));
    }
}

/// IW4L_T6_SOLIDWALK=<model>[:<seconds>] (with IW4L_T6_GOD=1): at that time
/// (default 30 s) the player stands 70 units in front of the first entity
/// with that model (a perk machine's zm_collision_perks1), faces it and
/// walks into it for 1.5 s; his distance to it is logged every 100 ms.
pub(crate) fn solid_walk_test(world: &mut World, now: i64) {
    static WANT: OnceLock<Option<(String, i64)>> = OnceLock::new();
    let Some((model, at)) = WANT.get_or_init(|| {
        let v = std::env::var("IW4L_T6_SOLIDWALK").ok()?;
        let (m, s) = v.split_once(':').unwrap_or((v.as_str(), "30"));
        Some((m.to_owned(), s.parse::<i64>().unwrap_or(30) * 1000))
    }) else {
        return;
    };
    let (at, tick) = (*at, i64::from(crate::MATCH_TICK_MS));
    if !(at..at + 1700).contains(&now) {
        return;
    }
    let Some((n, origin, yaw)) = world
        .resource::<Zm>()
        .ents
        .iter()
        .find(|(_, e)| e.model == *model)
        .map(|(n, e)| (*n, e.origin, e.angles[1]))
    else {
        if (at..at + tick).contains(&now) {
            diag::info!(Sim, "bo2zm t6 solidwalk: no {model} at {now}ms");
        }
        return;
    };
    let client = ClientId(0);
    if (at..at + tick).contains(&now) {
        // IW4L_T6_SOLIDSIDE=<degrees>: which side he comes from (0 = along
        // its forward).
        let side: f32 = std::env::var("IW4L_T6_SOLIDSIDE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0);
        let (s, c) = (yaw + side).to_radians().sin_cos();
        let stand = [origin[0] + c * 70.0, origin[1] + s * 70.0, origin[2] + 8.0];
        super::teleport_player(world, client, stand);
        super::set_player_view(world, client, [0.0, yaw + side + 180.0, 0.0]);
        diag::info!(
            Sim,
            "bo2zm t6 solidwalk: ent{n} {model} at ({:.0} {:.0} {:.0}) yaw {yaw:.0}",
            origin[0],
            origin[1],
            origin[2]
        );
        return;
    }
    {
        let mut req = world.resource_mut::<crate::step::StepRequest>();
        for (id, cmd) in &mut req.input.cmds {
            if *id == client {
                cmd.forwardmove = 127;
            }
        }
    }
    if (now - at) % 100 < tick
        && let Some(me) = frame(world).player(client).map(|p| p.origin)
    {
        let d = ((me[0] - origin[0]).powi(2) + (me[1] - origin[1]).powi(2)).sqrt();
        // The map alone: how far toward its centre he could go.
        let to = [origin[0], origin[1], me[2]];
        let t = frame(world).trace_static_world(
            [me[0], me[1], me[2] + 1.0],
            [to[0], to[1], to[2] + 1.0],
            crate::bullet_collision::PLAYER_MINS,
            crate::bullet_collision::PLAYER_MAXS,
            crate::bullet_collision::MASK_PLAYER_SOLID,
        );
        diag::info!(
            Sim,
            "bo2zm t6 solidwalk at {now}ms: {d:.0} from ent{n}; the map alone stops him {:.0} from it",
            d * (1.0 - t.fraction)
        );
    }
}

/// IW4L_T6_ZBLOCK=1 (with IW4L_T6_GOD=1): from 20 s, once a living zombie
/// stands within 100 units of player 0, he turns to it and walks into it
/// for 1.5 s; his distance to it is logged every 100 ms (a zombie stops him
/// ~30 units off, centre to centre; without its box he walks through).
pub(crate) fn zombie_block_test(world: &mut World, now: i64) {
    static ON: OnceLock<bool> = OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("IW4L_T6_ZBLOCK").is_some()) || now < 20000 {
        return;
    }
    static AT: std::sync::Mutex<Option<(u32, i64)>> = std::sync::Mutex::new(None);
    let client = ClientId(0);
    let Some(me) = frame(world).player(client).map(|p| p.origin) else {
        return;
    };
    let flat = |a: [f32; 3], b: [f32; 3]| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt();
    let mut at = AT.lock().unwrap_or_else(|e| e.into_inner());
    if at.is_none() {
        let zm = world.resource::<Zm>();
        let near = zm
            .actors
            .iter()
            .filter(|(_, a)| a.alive)
            .filter_map(|(n, _)| zm.ents.get(n).map(|e| (*n, flat(e.origin, me))))
            .filter(|(_, d)| *d < 100.0)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((n, d)) = near {
            diag::info!(
                Sim,
                "bo2zm t6 zblock: zombie ent{n} {d:.0} away at {now}ms, walking into it"
            );
            *at = Some((n, now));
        }
    }
    let Some((n, start)) = *at else {
        return;
    };
    if now > start + 1500 {
        return;
    }
    let Some(z) = world.resource::<Zm>().ents.get(&n).map(|e| e.origin) else {
        return;
    };
    let yaw = (z[1] - me[1]).atan2(z[0] - me[0]).to_degrees();
    super::set_player_view(world, client, [0.0, yaw, 0.0]);
    {
        let mut req = world.resource_mut::<crate::step::StepRequest>();
        for (id, cmd) in &mut req.input.cmds {
            if *id == client {
                cmd.forwardmove = 127;
            }
        }
    }
    if (now - start) % 100 < i64::from(crate::MATCH_TICK_MS) {
        let number = world
            .resource::<Zm>()
            .presences
            .by_ent
            .get(&n)
            .map(|s| s.number);
        let solid = number
            .and_then(|number| frame(world).script_mover_by_number(number))
            .map(|m| m.state.solid);
        let packed = frame(world).packed_solid_movers().len();
        diag::info!(
            Sim,
            "bo2zm t6 zblock at {now}ms: {:.0} from ent{n} (me {:.0} {:.0}, it {:.0} {:.0}) mover {number:?} solid {solid:?}, {packed} packed",
            flat(me, z),
            me[0],
            me[1],
            z[0],
            z[1]
        );
    }
}

/// IW4L_T6_FLOORMAP="x0 x1 y0 y1 step": at 2 s, the ground height under
/// each point of that grid (a straight drop from z 40, under the truck roof), one line per row.
pub(crate) fn floor_map(world: &mut World, now: i64) {
    let tick = i64::from(crate::MATCH_TICK_MS);
    if !(2000..2000 + tick).contains(&now) {
        return;
    }
    let Ok(spec) = std::env::var("IW4L_T6_FLOORMAP") else {
        return;
    };
    let v: Vec<f32> = spec
        .split_whitespace()
        .filter_map(|s| s.parse().ok())
        .collect();
    let [x0, x1, y0, y1, step] = v[..] else {
        return;
    };
    let mut y = y0;
    while y <= y1 {
        let mut row = Vec::new();
        let mut x = x0;
        while x <= x1 {
            let (a, b) = ([x, y, 40.0], [x, y, -200.0]);
            let t = frame(world).trace_static_world(
                a,
                b,
                [0.0; 3],
                [0.0; 3],
                crate::bullet_collision::MASK_PLAYER_SOLID,
            );
            row.push(format!("{:.0}", a[2] + (b[2] - a[2]) * t.fraction));
            x += step;
        }
        diag::info!(
            Sim,
            "bo2zm t6 floor y {y:.0} x {x0:.0}..: {}",
            row.join(" ")
        );
        y += step;
    }
}

/// IW4L_T6_WALK="x y z yaw secs": at 12 s the player stands at x y z facing
/// yaw, then walks forward for secs; his place is logged every 250 ms (does
/// a door or debris stop him).
pub(crate) fn walk_test(world: &mut World, now: i64) {
    static WANT: OnceLock<Option<[f32; 5]>> = OnceLock::new();
    let Some([x, y, z, yaw, secs]) = *WANT.get_or_init(|| {
        let v: Vec<f32> = std::env::var("IW4L_T6_WALK")
            .ok()?
            .split_whitespace()
            .filter_map(|s| s.parse().ok())
            .collect();
        (v.len() == 5).then(|| [v[0], v[1], v[2], v[3], v[4]])
    }) else {
        return;
    };
    let client = ClientId(0);
    let tick = i64::from(crate::MATCH_TICK_MS);
    if (12000..12000 + tick).contains(&now) {
        super::teleport_player(world, client, [x, y, z]);
        super::set_player_view(world, client, [0.0, yaw, 0.0]);
    }
    let end = 12300 + (secs * 1000.0) as i64;
    // IW4L_T6_WALK_CLIENT: the console's +forward walks him instead (his
    // client predicts the walk: does it clip as the server does).
    let pushed = std::env::var_os("IW4L_T6_WALK_CLIENT").is_none();
    // IW4L_T6_WALK_JUMP=<ms>: he presses jump that long into the walk.
    let jump_at = std::env::var("IW4L_T6_WALK_JUMP")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .map(|ms| 12300 + ms);
    if pushed && (12300..end).contains(&now) {
        let jump = jump_at.is_some_and(|t| (t..t + 100).contains(&now));
        let mut req = world.resource_mut::<crate::step::StepRequest>();
        for (id, cmd) in &mut req.input.cmds {
            if *id == client {
                if jump {
                    cmd.buttons |= buttons::JUMP;
                }
                cmd.forwardmove = 127;
            }
        }
    }
    // IW4L_T6_WALK_LOOK="x y z": when the walk ends he looks at that point
    // (a wall buy's chalk) and holds use for 500 ms.
    if let Some(at) = std::env::var("IW4L_T6_WALK_LOOK").ok().and_then(|v| {
        let p: Vec<f32> = v
            .split_whitespace()
            .filter_map(|s| s.parse().ok())
            .collect();
        (p.len() == 3).then(|| [p[0], p[1], p[2]])
    }) && (end + 200..end + 1200).contains(&now)
    {
        if let Some(ps) = frame(world).player(client).copied() {
            let eye = [
                ps.origin[0],
                ps.origin[1],
                ps.origin[2] + ps.view_height_current,
            ];
            super::set_player_view(world, client, look(eye, at));
        }
        if (end + 600..end + 1100).contains(&now) {
            let mut req = world.resource_mut::<crate::step::StepRequest>();
            for (id, cmd) in &mut req.input.cmds {
                if *id == client {
                    cmd.buttons |= buttons::USE;
                }
            }
        }
    }
    if (12000..end + 1500).contains(&now) && (now - 12000) % 250 < tick {
        if let Some(ps) = frame(world).player(client) {
            diag::info!(
                Sim,
                "bo2zm t6 walk test at {now}ms: ({:.0} {:.0} {:.0})",
                ps.origin[0],
                ps.origin[1],
                ps.origin[2]
            );
        }
    }
}

/// IW4L_T6_FACE=1: the player keeps facing the nearest living zombie
/// within 800 units (its chest), to watch one come (test aid).
pub(crate) fn face_test(world: &mut World) {
    static ON: OnceLock<bool> = OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("IW4L_T6_FACE").is_some()) {
        return;
    }
    let client = ClientId(0);
    let Some(ps) = frame(world).player(client).copied() else {
        return;
    };
    let nearest = {
        let zm = world.resource::<Zm>();
        zm.actors
            .iter()
            .filter(|(_, a)| a.alive)
            .filter_map(|(n, a)| {
                zm.ents
                    .get(n)
                    .map(|e| (*n, e.origin, e.angles[1], a.clone()))
            })
            .filter(|(_, o, _, _)| flat_dist(*o, ps.origin) < 800.0)
            .min_by(|a, b| flat_dist(a.1, ps.origin).total_cmp(&flat_dist(b.1, ps.origin)))
    };
    // Every 100 ms: that zombie's state, animation, facing against the way
    // to him, and distance.
    let now = world.resource::<Zm>().now_ms;
    if let Some((n, o, yaw, a)) = &nearest
        && now % 100 < i64::from(crate::MATCH_TICK_MS)
    {
        let to_me = (ps.origin[1] - o[1])
            .atan2(ps.origin[0] - o[0])
            .to_degrees();
        diag::info!(
            Sim,
            "bo2zm t6 face test {now}: zombie {n} {} {} yaw {yaw:.0} to me {to_me:.0} dist {:.0} path {}/{}",
            a.script,
            a.playing.as_ref().map_or("-", |p| p.anim.as_str()),
            flat_dist(*o, ps.origin),
            a.path_i,
            a.path.len()
        );
    }
    let target = nearest.map(|t| t.1);
    if let Some(o) = target {
        let eye = [
            ps.origin[0],
            ps.origin[1],
            ps.origin[2] + ps.view_height_current,
        ];
        super::set_player_view(world, client, look(eye, [o[0], o[1], o[2] + 45.0]));
    }
}

/// IW4L_T6_KITE=1 (with IW4L_T6_GOD=1): once a zombie is swinging at him
/// (combat, within 80 units), the player turns away from it and walks for
/// 6 s, then stands 6 s; every 500 ms his place and each zombie within 400
/// units (state, animation, place, goal) are logged: does a zombie that
/// swung follow him.
pub(crate) fn kite_test(world: &mut World, now: i64) {
    static ON: OnceLock<bool> = OnceLock::new();
    static START: std::sync::Mutex<Option<(i64, f32)>> = std::sync::Mutex::new(None);
    if !*ON.get_or_init(|| std::env::var("IW4L_T6_KITE").is_ok()) {
        return;
    }
    let client = ClientId(0);
    let tick = i64::from(crate::MATCH_TICK_MS);
    let Some(ps) = frame(world).player(client).copied() else {
        return;
    };
    let start = *START.lock().unwrap();
    let Some((t0, yaw)) = start else {
        // A zombie swinging close by: turn away from it and go.
        let found = {
            let zm = world.resource::<Zm>();
            zm.actors
                .iter()
                .filter(|(_, a)| a.alive && a.script == "combat")
                .filter_map(|(n, _)| zm.ents.get(n).map(|e| e.origin))
                .find(|o| flat_dist(*o, ps.origin) < 80.0)
        };
        if let Some(o) = found {
            let away = (ps.origin[1] - o[1])
                .atan2(ps.origin[0] - o[0])
                .to_degrees();
            *START.lock().unwrap() = Some((now, away));
            super::set_player_view(world, client, [0.0, away, 0.0]);
            diag::info!(
                Sim,
                "bo2zm t6 kite test: zombie swinging at ({:.0} {:.0}); walking away at yaw {away:.0}",
                o[0],
                o[1]
            );
        }
        return;
    };
    if now - t0 < 6000 {
        super::set_player_view(world, client, [0.0, yaw, 0.0]);
        let mut req = world.resource_mut::<crate::step::StepRequest>();
        for (id, cmd) in &mut req.input.cmds {
            if *id == client {
                cmd.forwardmove = 127;
            }
        }
    }
    if now - t0 <= 12_000 && (now - t0) % 500 < tick {
        let rows: Vec<String> = {
            let zm = world.resource::<Zm>();
            zm.actors
                .iter()
                .filter(|(_, a)| a.alive)
                .filter_map(|(n, a)| {
                    let e = zm.ents.get(n)?;
                    (flat_dist(e.origin, ps.origin) < 400.0).then(|| {
                        format!(
                            "{n} {} {} at ({:.0} {:.0}) goal {:?}",
                            a.script,
                            a.playing.as_ref().map_or("-", |p| p.anim.as_str()),
                            e.origin[0],
                            e.origin[1],
                            a.goal.map(|g| [g[0].round(), g[1].round()])
                        )
                    })
                })
                .collect()
        };
        diag::info!(
            Sim,
            "bo2zm t6 kite test +{}ms: me ({:.0} {:.0}) | {}",
            now - t0,
            ps.origin[0],
            ps.origin[1],
            rows.join(" | ")
        );
    }
}

/// IW4L_T6_GAMEOVER=<seconds>: then the level's `end_game` (the scripts'
/// game over: GAME OVER, the map's intermission shot, a new game).
pub(crate) fn game_over_test(world: &mut World, now: i64) {
    static AT: OnceLock<Option<i64>> = OnceLock::new();
    let Some(at) = *AT.get_or_init(|| {
        std::env::var("IW4L_T6_GAMEOVER")
            .ok()
            .and_then(|s| s.parse::<i64>().ok())
    }) else {
        return;
    };
    let tick = i64::from(crate::MATCH_TICK_MS);
    if (at * 1000..at * 1000 + tick).contains(&now) {
        with_vm(world, |vm, world| {
            let level = vm.level;
            vm.notify_str(world, level, "end_game", &[]);
        });
        diag::info!(Sim, "bo2zm t6 game over test: end_game at {at}s");
    }
}

/// IW4L_T6_DOWN=<seconds>: then a zombie's blow takes all his health (he
/// goes down; solo with no Quick Revive that is the game over). Every
/// second after, his eye height and view are logged (down-glitch test).
pub(crate) fn down_test(world: &mut World, now: i64) {
    static AT: OnceLock<Option<i64>> = OnceLock::new();
    let Some(at) = *AT.get_or_init(|| std::env::var("IW4L_T6_DOWN").ok().and_then(|s| s.parse::<i64>().ok()))
    else {
        return;
    };
    let tick = i64::from(crate::MATCH_TICK_MS);
    if (at * 1000..at * 1000 + tick).contains(&now) {
        with_vm(world, |vm, world| {
            let point = frame(world).player(ClientId(0)).map_or([0.0; 3], |ps| ps.origin);
            super::natives_ai::player_damage(
                vm,
                world,
                0,
                gsc_t6::Value::Undefined,
                gsc_t6::Value::Undefined,
                1000,
                0,
                "MOD_MELEE",
                "none",
                point,
                [0.0, 0.0, -1.0],
                "torso_upper",
            );
        });
        diag::info!(Sim, "bo2zm t6 down test: 1000 damage at {at}s");
    }
    if now > at * 1000 && now < at * 1000 + 60_000 && now % 250 < tick {
        let (down, linked) = world
            .resource::<Zm>()
            .players
            .get(&0)
            .map_or((false, None), |p| (p.laststand, p.linked));
        if let Some(ps) = frame(world).player(ClientId(0)) {
            diag::info!(
                Sim,
                "bo2zm t6 down test +{}ms: down {down} linked {linked:?} z {:.1} view_h {:.1} pm_type {} eflags {:#x} pm_flags {:#x}",
                now - at * 1000,
                ps.origin[2],
                ps.view_height_current,
                ps.pm_type,
                ps.e_flags,
                ps.pm_flags
            );
        }
    }
}

/// IW4L_T6_PERK=<machine targetname> (e.g. vending_revive, or `landed`
/// for the lowest machine): at 42 s (the
/// first perk has landed) the player gets 10000 points and stands 40 units
/// in front of the machine, looking at it; he presses use at 43 s; at 50 s
/// his perks, weapons and points are logged.
pub(crate) fn perk_test(world: &mut World, now: i64) {
    static WANT: OnceLock<Option<String>> = OnceLock::new();
    let Some(want) = WANT
        .get_or_init(|| std::env::var("IW4L_T6_PERK").ok())
        .clone()
    else {
        return;
    };
    let client = ClientId(0);
    let tick = i64::from(crate::MATCH_TICK_MS);
    // "landed": the test starts once a perk machine is on the ground
    // (Nuketown drops them from the sky, the first some time into round 1):
    // its times count from then as if it were 42 s.
    static BASE: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(i64::MIN);
    if want == "landed" && BASE.load(std::sync::atomic::Ordering::Relaxed) == i64::MIN {
        if now < 42000 || now % 1000 >= tick {
            return;
        }
        let low = with_vm(world, |vm, world| {
            let tn = vm.intern("targetname");
            world
                .resource::<Zm>()
                .ents
                .values()
                .filter(|e| e.obj.is_some_and(|o| vm.to_text(&vm.raw_field(o, tn)).starts_with("vending_")))
                .map(|e| e.origin[2])
                .fold(f32::MAX, f32::min)
        })
        .unwrap_or(f32::MAX);
        // On the ground (Nuketown's streets are near 0; a falling one
        // passes through 500).
        if low > 150.0 {
            return;
        }
        BASE.store(now - 42000, std::sync::atomic::Ordering::Relaxed);
    }
    let base = BASE.load(std::sync::atomic::Ordering::Relaxed);
    let now = if base == i64::MIN { now } else { now - base };
    if (42000..42000 + tick).contains(&now) {
        let machine = with_vm(world, |vm, world| {
            let tn = vm.intern("targetname");
            let zm = world.resource::<Zm>();
            // "landed": whichever perk machine is lowest.
            if want == "landed" {
                return zm
                    .ents
                    .values()
                    .filter(|e| e.obj.is_some_and(|o| vm.to_text(&vm.raw_field(o, tn)).starts_with("vending_")))
                    .min_by(|a, b| a.origin[2].total_cmp(&b.origin[2]))
                    .map(|e| (e.origin, e.angles));
            }
            zm.ents.values().find_map(|e| {
                let o = e.obj?;
                (vm.to_text(&vm.raw_field(o, tn)) == want).then_some((e.origin, e.angles))
            })
        })
        .flatten();
        let Some((origin, angles)) = machine else {
            diag::warn!(Sim, "bo2zm t6 perk test: no {want}");
            return;
        };
        with_vm(world, |vm, world| {
            if let Some(p) = world.resource::<Zm>().players.get(&0).map(|p| p.obj) {
                let f = vm.intern("score");
                vm.set_field(world, p, f, gsc_t6::Value::Int(10000));
            }
        });
        // A machine faces 90 degrees right of its angles (bring_perk pushes
        // it back along forward(angles - 90)).
        let (fw, _, _) = gsc_t6::math::angle_vectors([0.0, angles[1] - 90.0, 0.0]);
        let stand = [
            origin[0] + fw[0] * stand_dist(),
            origin[1] + fw[1] * stand_dist(),
            origin[2] + 30.0,
        ];
        let down = [stand[0], stand[1], stand[2] - 200.0];
        let t = frame(world).trace_static_world(
            stand,
            down,
            crate::bullet_collision::PLAYER_MINS,
            crate::bullet_collision::PLAYER_MAXS,
            crate::bullet_collision::MASK_PLAYER_SOLID,
        );
        let feet: [f32; 3] = std::array::from_fn(|i| stand[i] + (down[i] - stand[i]) * t.fraction);
        let eye = [feet[0], feet[1], feet[2] + 60.0];
        let view = look(eye, [origin[0], origin[1], origin[2] + 40.0]);
        super::teleport_player(world, client, feet);
        super::set_player_view(world, client, view);
        diag::info!(
            Sim,
            "bo2zm t6 perk test: {want} at ({:.0} {:.0} {:.0}); player at ({:.0} {:.0} {:.0})",
            origin[0],
            origin[1],
            origin[2],
            feet[0],
            feet[1],
            feet[2]
        );
    }
    if (43000..43300).contains(&now) {
        let mut req = world.resource_mut::<crate::step::StepRequest>();
        for (id, cmd) in &mut req.input.cmds {
            if *id == client {
                cmd.buttons |= buttons::USE;
            }
        }
    }
    if (50000..50000 + tick).contains(&now) {
        let perks: Vec<String> = world
            .resource::<Zm>()
            .players
            .get(&0)
            .map(|p| p.perks.iter().cloned().collect())
            .unwrap_or_default();
        let names: Vec<String> = {
            let f = frame(world);
            crate::script_player::weapons(&f, client, crate::script_player::WeaponList::All)
                .into_iter()
                .map(|w| crate::script_player::weapon_name(&f, w))
                .collect()
        };
        let score = frame(world).client_meta(client).map_or(0, |m| m.score);
        let hint = world
            .resource::<Zm>()
            .hints
            .get(&0)
            .cloned()
            .unwrap_or_default();
        diag::info!(
            Sim,
            "bo2zm t6 perk test: perks {perks:?}, weapons {names:?}, points {score}, hint {hint:?}"
        );
    }
}

/// An entity's object by a key's value (`targetname`, `script_noteworthy`).
fn ent_by(
    world: &mut World,
    key: &str,
    value: &str,
) -> Option<(gsc_t6::ObjRef, [f32; 3], [f32; 3])> {
    with_vm(world, |vm, world| {
        let k = vm.intern(key);
        let zm = world.resource::<Zm>();
        zm.ents.values().find_map(|e| {
            let o = e.obj?;
            (vm.to_text(&vm.raw_field(o, k)) == value).then_some((o, e.origin, e.angles))
        })
    })
    .flatten()
}

/// IW4L_T6_PAP=1: Pack-a-Punch. At 25 s Nuketown's machine is brought down
/// by the game's own `bring_perk`; at 90 s the player, given 10000 points,
/// stands at it and uses it with his gun; at 100 s he takes the upgraded
/// gun back; at 106 s his weapons and points are logged.
pub(crate) fn pap_test(world: &mut World, now: i64) {
    static ON: OnceLock<bool> = OnceLock::new();
    if !*ON.get_or_init(|| std::env::var("IW4L_T6_PAP").is_ok()) {
        return;
    }
    let client = ClientId(0);
    let tick = i64::from(crate::MATCH_TICK_MS);
    let machine = |world: &mut World| {
        let (trigger, _, _) = ent_by(world, "script_noteworthy", "specialty_weapupgrade")?;
        let target = with_vm(world, |vm, _| {
            let t = vm.intern("target");
            vm.to_text(&vm.raw_field(trigger, t))
        })?;
        let (m, origin, angles) = ent_by(world, "targetname", &target)?;
        Some((trigger, m, origin, angles))
    };
    if (25000..25000 + tick).contains(&now) {
        let Some((trigger, m, _, _)) = machine(world) else {
            diag::warn!(Sim, "bo2zm t6 pap test: no machine");
            return;
        };
        with_vm(world, |vm, world| {
            let level = gsc_t6::Value::Object(vm.level);
            vm.spawn_named(
                world,
                "maps/mp/zm_nuked_perks",
                "bring_perk",
                level,
                vec![gsc_t6::Value::Object(m), gsc_t6::Value::Object(trigger)],
            );
        });
        diag::info!(Sim, "bo2zm t6 pap test: bringing the machine");
    }
    if (90000..90000 + tick).contains(&now) {
        let Some((_, _, origin, angles)) = machine(world) else {
            return;
        };
        with_vm(world, |vm, world| {
            if let Some(p) = world.resource::<Zm>().players.get(&0).map(|p| p.obj) {
                let f = vm.intern("score");
                vm.set_field(world, p, f, gsc_t6::Value::Int(10000));
            }
        });
        stand_at(world, client, origin, angles);
    }
    if (91000..91300).contains(&now) || (100000..100300).contains(&now) {
        let mut req = world.resource_mut::<crate::step::StepRequest>();
        for (id, cmd) in &mut req.input.cmds {
            if *id == client {
                cmd.buttons |= buttons::USE;
            }
        }
    }
    if [92000, 96000, 101000, 106000]
        .iter()
        .any(|t| (*t..*t + tick).contains(&now))
    {
        let names: Vec<String> = {
            let f = frame(world);
            crate::script_player::weapons(&f, client, crate::script_player::WeaponList::All)
                .into_iter()
                .map(|w| crate::script_player::weapon_name(&f, w))
                .collect()
        };
        let score = frame(world).client_meta(client).map_or(0, |m| m.score);
        let hint = world
            .resource::<Zm>()
            .hints
            .get(&0)
            .cloned()
            .unwrap_or_default();
        diag::info!(
            Sim,
            "bo2zm t6 pap test at {}s: weapons {names:?}, points {score}, hint {hint:?}",
            now / 1000
        );
    }
}

/// IW4L_T6_POWERUP=<name>[,<name>...]: from 30 s, one every 12 s, that
/// power-up drops 60 units in front of the player (the game's own
/// specific_powerup_drop) and he walks onto it at the next second.
pub(crate) fn powerup_test(world: &mut World, now: i64) {
    static WANT: OnceLock<Vec<String>> = OnceLock::new();
    let want = WANT.get_or_init(|| {
        std::env::var("IW4L_T6_POWERUP")
            .map(|s| s.split(',').map(str::to_owned).collect())
            .unwrap_or_default()
    });
    if want.is_empty() || now < 30000 {
        return;
    }
    let tick = i64::from(crate::MATCH_TICK_MS);
    let step = (now - 30000) / 12000;
    let into = (now - 30000) % 12000;
    let Some(name) = want.get(step as usize) else {
        return;
    };
    let client = ClientId(0);
    let Some(ps) = frame(world).player(client).copied() else {
        return;
    };
    let (f, _, _) = gsc_t6::math::angle_vectors([0.0, ps.viewangles[1], 0.0]);
    let spot = [
        ps.origin[0] + f[0] * 60.0,
        ps.origin[1] + f[1] * 60.0,
        ps.origin[2],
    ];
    if into < tick {
        with_vm(world, |vm, world| {
            let n = vm.string(name);
            vm.spawn_named(
                world,
                "maps/mp/zombies/_zm_powerups",
                "specific_powerup_drop",
                gsc_t6::Value::Object(vm.level),
                vec![n, gsc_t6::Value::Vec3(spot)],
            );
        });
        diag::info!(Sim, "bo2zm t6 powerup test: {name} dropped");
    }
    if (1000..1000 + tick).contains(&into) {
        super::teleport_player(world, client, spot);
        diag::info!(Sim, "bo2zm t6 powerup test: walked onto {name}");
    }
}

/// How far in front of a machine the tests stand (IW4L_T6_STAND, 40).
fn stand_dist() -> f32 {
    static D: OnceLock<f32> = OnceLock::new();
    *D.get_or_init(|| {
        std::env::var("IW4L_T6_STAND")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(40.0)
    })
}

/// The player stands 40 units in front of a machine, looking at it.
fn stand_at(world: &mut World, client: ClientId, origin: [f32; 3], angles: [f32; 3]) {
    // A machine faces 90 degrees right of its angles (bring_perk pushes
    // it back along forward(angles - 90)).
    let (fw, _, _) = gsc_t6::math::angle_vectors([0.0, angles[1] - 90.0, 0.0]);
    let stand = [
        origin[0] + fw[0] * stand_dist(),
        origin[1] + fw[1] * stand_dist(),
        origin[2] + 30.0,
    ];
    let down = [stand[0], stand[1], stand[2] - 200.0];
    let t = frame(world).trace_static_world(
        stand,
        down,
        crate::bullet_collision::PLAYER_MINS,
        crate::bullet_collision::PLAYER_MAXS,
        crate::bullet_collision::MASK_PLAYER_SOLID,
    );
    let feet: [f32; 3] = std::array::from_fn(|i| stand[i] + (down[i] - stand[i]) * t.fraction);
    let eye = [feet[0], feet[1], feet[2] + 60.0];
    let view = look(eye, [origin[0], origin[1], origin[2] + 40.0]);
    super::teleport_player(world, client, feet);
    super::set_player_view(world, client, view);
    diag::info!(
        Sim,
        "bo2zm t6 test: machine at ({:.0} {:.0} {:.0}); player at ({:.0} {:.0} {:.0})",
        origin[0],
        origin[1],
        origin[2],
        feet[0],
        feet[1],
        feet[2]
    );
}

/// The rounds test's kit: the gun at 8 s, ammo topped up every 2 s.
fn rounds_kit(world: &mut World, now: i64, client: ClientId) {
    let tick = i64::from(crate::MATCH_TICK_MS);
    if (8000..8000 + tick).contains(&now) {
        let gun = std::env::var("IW4L_T6_AUTOPLAY_GUN").unwrap_or_else(|_| "m14_zm".to_owned());
        with_vm(world, |vm, world| {
            if let Some(p) = world.resource::<Zm>().players.get(&client.0).map(|p| p.obj) {
                let g = vm.string(&gun);
                vm.spawn_named(
                    world,
                    "maps/mp/zombies/_zm_weapons",
                    "weapon_give",
                    gsc_t6::Value::Object(p),
                    vec![g],
                );
            }
        });
        diag::info!(Sim, "bo2zm t6 autoplay: given {gun}");
    }
    if now % 2000 < tick {
        let mut f = frame(world);
        if let Some(w) = f.player(client).map(|p| p.weapon) {
            crate::script_player::set_ammo_stock(&mut f, client, w, 400);
        }
    }
}

/// IW4L_T6_FXTEST=<effect name>: at 6 s that effect plays (held, turned
/// with a forward and an up) 80 units in front of the player, facing him
/// along its left (a wall buy's chalk faces its struct's left).
pub(crate) fn fx_test(world: &mut World, now: i64) {
    static WANT: OnceLock<Option<String>> = OnceLock::new();
    let Some(want) = WANT
        .get_or_init(|| std::env::var("IW4L_T6_FXTEST").ok())
        .clone()
    else {
        return;
    };
    let tick = i64::from(crate::MATCH_TICK_MS);
    if !(6000..6000 + tick).contains(&now) {
        return;
    }
    let Some(ps) = frame(world).player(ClientId(0)).copied() else {
        return;
    };
    let yaw = ps.viewangles[1];
    let (f, _, _) = gsc_t6::math::angle_vectors([0.0, yaw, 0.0]);
    let at = [
        ps.origin[0] + f[0] * 80.0,
        ps.origin[1] + f[1] * 80.0,
        ps.origin[2] + 50.0,
    ];
    // The effect's forward runs across his view; its left faces him.
    // IW4L_T6_FXTEST_UP=1: its forward points up (as the map places its
    // fires), its up away from him.
    let fx_forward = gsc_t6::math::angle_vectors([0.0, yaw + 90.0, 0.0]).0;
    if std::env::var_os("IW4L_T6_FXTEST_UP").is_some() {
        super::natives_fx::effect_axis(world, &want, at, [0.0, 0.0, 1.0], f);
    } else {
        super::natives_fx::effect_axis(world, &want, at, fx_forward, [0.0, 0.0, 1.0]);
    }
    diag::info!(
        Sim,
        "bo2zm t6 fx test: {want} at ({:.0} {:.0} {:.0})",
        at[0],
        at[1],
        at[2]
    );
}

/// IW4L_T6_LOOKAT=1 (pictures): the player's view follows the nearest
/// living zombie's head every tick. IW4L_T6_LOOKAT=orbit<deg> (e.g.
/// orbit180 = behind it): the player stands 70 units from that zombie at
/// that angle from the way it faces, looking at its head.
pub(crate) fn look_test(world: &mut World) {
    static ON: OnceLock<Option<String>> = OnceLock::new();
    let Some(mode) = ON
        .get_or_init(|| std::env::var("IW4L_T6_LOOKAT").ok())
        .clone()
    else {
        return;
    };
    if let Some(deg) = mode
        .strip_prefix("orbit")
        .and_then(|d| d.parse::<f32>().ok())
    {
        orbit_look(world, deg);
        return;
    }
    let client = ClientId(0);
    let Some(ps) = frame(world).player(client).copied() else {
        return;
    };
    let eye = [
        ps.origin[0],
        ps.origin[1],
        ps.origin[2] + ps.view_height_current,
    ];
    let nearest = {
        let zm = world.resource::<Zm>();
        zm.actors
            .iter()
            .filter(|(_, a)| a.alive)
            .filter_map(|(n, _)| zm.ents.get(n).map(|e| (*n, flat_dist(e.origin, ps.origin))))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(n, _)| n)
    };
    let Some(n) = nearest else { return };
    let Some(head) = head_of(world, n, true) else {
        return;
    };
    super::set_player_view(world, client, look(eye, head));
}

/// IW4L_T6_LOOKAT=orbit<deg>: see `look_test`.
fn orbit_look(world: &mut World, deg: f32) {
    let client = ClientId(0);
    let Some(ps) = frame(world).player(client).copied() else {
        return;
    };
    let nearest = {
        let zm = world.resource::<Zm>();
        zm.actors
            .iter()
            .filter(|(_, a)| a.alive)
            .filter_map(|(n, _)| {
                zm.ents
                    .get(n)
                    .map(|e| (*n, e.origin, e.angles[1], flat_dist(e.origin, ps.origin)))
            })
            .min_by(|a, b| a.3.total_cmp(&b.3))
    };
    let Some((n, origin, yaw, _)) = nearest else {
        return;
    };
    let Some(head) = head_of(world, n, true) else {
        return;
    };
    let a = (yaw + deg).to_radians();
    let stand = [
        origin[0] + a.cos() * 70.0,
        origin[1] + a.sin() * 70.0,
        origin[2] + 40.0,
    ];
    let down = [stand[0], stand[1], stand[2] - 200.0];
    let t = frame(world).trace_static_world(
        stand,
        down,
        crate::bullet_collision::PLAYER_MINS,
        crate::bullet_collision::PLAYER_MAXS,
        crate::bullet_collision::MASK_PLAYER_SOLID,
    );
    let feet: [f32; 3] = std::array::from_fn(|i| stand[i] + (down[i] - stand[i]) * t.fraction);
    let eye = [feet[0], feet[1], feet[2] + ps.view_height_current];
    super::teleport_player(world, client, feet);
    super::set_player_view(world, client, look(eye, head));
}
