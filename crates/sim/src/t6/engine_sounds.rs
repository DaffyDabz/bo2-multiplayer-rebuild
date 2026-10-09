//! Helicopter and drone engine loops. BO2 plays them from a client script
//! (`clientscripts/mp/_helicopter_sounds.csc`: `start_helicopter_sounds`,
//! picked by the vehicle's `vehicletype`), which this rebuild does not run.
//! The same loops, with that script's own numbers and update rules, run
//! here and go out with the other entity loops (`presence::publish_loops`),
//! each with the volume and pitch the script would set (`setloopstate`).
//!
//! Every number below is from the disassembly of `_helicopter_sounds.csc`
//! (`init_heli_sound_values` rows, `heli_idle_run_transition`,
//! `drone_up_down_transition`, `drone_rotate_angle`) and `_audio.csc`
//! (`scale_speed`). Not carried over: the script's per-part tags (`snd_rotor`,
//! `snd_tail_rotor`, `tag_body`: every loop sits on the vehicle's origin),
//! its fade times (`playloopsound` 0.5 / 2 s in, `stoploopsound` 4 s out)
//! and `setloopstate`'s rates (their meaning is the exe's; the target level
//! is applied at the script's own update steps), and the rotor-wash dust
//! (`terrain_trace`, a separate surface-sound system).

use std::collections::BTreeMap;

use bevy_ecs::prelude::{Resource, World};
use gsc_t6::ObjRef;

use super::Zm;

/// `self.mph_to_inches_per_sec`.
const MPH: f32 = 17.6;
/// `self.idle_run_trans_speed` (mph).
const IDLE_RUN: f32 = 5.0;

/// How one loop's level follows the vehicle.
#[derive(Clone, Copy)]
enum Drive {
    /// `heli_sound_play`: the alias's own level, never changed.
    Fixed,
    /// `heli_idle_run_transition` with its `helisoundvalues` row:
    /// speed-volume max, volume min, volume max, speed-pitch max, pitch min,
    /// pitch max (mph).
    Run([f32; 6]),
    /// `drone_up_down_transition`'s three loops.
    Up,
    Down,
    Vertical,
    /// `drone_rotate_angle`.
    Turn,
}

struct Part {
    alias: &'static str,
    drive: Drive,
}

struct Type {
    parts: &'static [Part],
    /// `heli_idle_run_transition`'s wait (0.5 s unless given; the drone
    /// passes 0.1 s and `updown`).
    wait_ms: i64,
    updown: bool,
}

const fn p(alias: &'static str, drive: Drive) -> Part {
    Part { alias, drive }
}

/// cobra / hind rows (`init_heli_sound_values`, identical for both).
const COBRA_TURBINE: Drive = Drive::Run([65.0, 0.6, 0.8, 65.0, 1.0, 1.1]);
const COBRA_TOP: Drive = Drive::Run([45.0, 0.7, 1.0, 45.0, 0.95, 1.1]);
const COBRA_TAIL: Drive = Drive::Run([45.0, 0.5, 1.0, 45.0, 0.95, 1.1]);

/// `init_heli_sounds_player_controlled` + `play_player_controlled_sounds`.
const COBRA: Type = Type {
    parts: &[
        p("veh_cobra_rotor_lfe", Drive::Fixed),
        p("veh_cobra_turbine", COBRA_TURBINE),
        p("veh_cobra_rotor", COBRA_TOP),
        p("veh_cobra_tail", COBRA_TAIL),
    ],
    wait_ms: 500,
    updown: false,
};

/// `init_heli_sounds_ai_attack` + `play_attack_ai_sounds`.
const HIND: Type = Type {
    parts: &[
        p("veh_hind_rotor_lfe", Drive::Fixed),
        p("veh_hind_turbine", COBRA_TURBINE),
        p("veh_hind_rotor", COBRA_TOP),
        p("veh_hind_tail", COBRA_TAIL),
    ],
    wait_ms: 500,
    updown: false,
};

/// `init_heli_sounds_supply` + `play_supply_sounds` (no tail loop).
const SUPPLY: Type = Type {
    parts: &[
        p("veh_supply_rotor_lfe", Drive::Fixed),
        p("veh_supply_turbine", Drive::Run([65.0, 0.7, 1.0, 65.0, 1.0, 1.1])),
        p("veh_supply_rotor", Drive::Run([35.0, 0.95, 1.0, 100.0, 1.0, 1.1])),
    ],
    wait_ms: 500,
    updown: false,
};

/// `init_heli_sounds_gunner` + `play_gunner_sounds`.
const HUEY: Type = Type {
    parts: &[
        p("veh_huey_rotor_lfe", Drive::Fixed),
        p("veh_huey_radio", Drive::Fixed),
        p("veh_huey_turbine", Drive::Run([65.0, 0.7, 0.8, 65.0, 1.0, 1.1])),
        p("veh_huey_rotor", Drive::Run([45.0, 0.8, 1.0, 45.0, 0.95, 1.1])),
        p("veh_huey_tail", Drive::Run([45.0, 0.6, 1.0, 45.0, 0.95, 1.0])),
        p("veh_huey_door_wind", Drive::Run([45.0, 0.6, 1.0, 45.0, 0.95, 1.0])),
    ],
    wait_ms: 500,
    updown: false,
};

/// `init_heli_sounds_player_drone` + `play_player_drone_sounds`.
const QRDRONE: Type = Type {
    parts: &[
        p("veh_qrdrone_turbine_idle", Drive::Run([30.0, 0.8, 0.0, 16.0, 0.9, 1.1])),
        p("veh_qrdrone_turbine_moving", Drive::Run([30.0, 0.0, 0.9, 20.0, 0.9, 1.1])),
        p("veh_qrdrone_move_down", Drive::Down),
        p("veh_qrdrone_move_up", Drive::Up),
        p("veh_qrdrone_vertical", Drive::Vertical),
        p("veh_qrdrone_idle_rotate", Drive::Turn),
    ],
    wait_ms: 100,
    updown: true,
};

/// `init_heli_sounds_heli_guard` + `play_heli_guard_sounds`.
const HELI_GUARD: Type = Type {
    parts: &[
        p("veh_overwatch_lfe", Drive::Fixed),
        p("veh_overwatch_turbine", Drive::Run([10.0, 0.9, 1.0, 30.0, 0.9, 1.05])),
        p("veh_overwatch_rotor", Drive::Run([10.0, 0.9, 1.0, 30.0, 0.9, 1.1])),
    ],
    wait_ms: 500,
    updown: false,
};

/// `start_helicopter_sounds`' switch on `vehicletype`.
fn type_of(vehicletype: &str) -> Option<&'static Type> {
    Some(match vehicletype.to_ascii_lowercase().as_str() {
        "heli_ai_mp" | "zombie_cobra" => &HIND,
        "heli_guard_mp" => &HELI_GUARD,
        "heli_gunner_mp" => &HUEY,
        "heli_player_controlled_firstperson_mp"
        | "heli_player_controlled_mp"
        | "heli_player_gunner_mp" => &COBRA,
        "heli_supplydrop_mp" => &SUPPLY,
        "qrdrone_mp" => &QRDRONE,
        _ => return None,
    })
}

/// `_audio.csc` `scale_speed`.
fn scale_speed(x1: f32, x2: f32, y1: f32, y2: f32, z: f32) -> f32 {
    let z = z.max(x1).min(x2);
    (z - x1) / (x2 - x1) * (y2 - y1) + y1
}

struct Engine {
    obj: Option<ObjRef>,
    vehicletype: String,
    ty: &'static Type,
    started: bool,
    /// Last tick's origin and time (its velocity: `getvelocity`).
    last: [f32; 3],
    last_ms: i64,
    speed_mph: f32,
    /// The run loops' next step (`wait_time`).
    run_at: i64,
    /// The drone's 0.1 s windows: when the next ends, and where it began.
    window_at: i64,
    window_z: f32,
    window_yaw: f32,
    /// `self.qrdrone_z_difference`.
    z_diff: f32,
    /// Each part's (volume, pitch).
    level: Vec<(f32, f32)>,
}

#[derive(Resource, Default)]
pub(crate) struct EngineSounds {
    by_ent: BTreeMap<u32, Engine>,
}

/// A vehicle was spawned (`spawnhelicopter`, `spawnvehicle`): its engine
/// loops start if BO2's client script has some for its type.
pub(crate) fn spawned(world: &mut World, n: u32, vehicletype: &str) {
    let Some(ty) = type_of(vehicletype) else {
        return;
    };
    let (obj, origin, angles, now) = {
        let zm = world.resource::<Zm>();
        let Some(e) = zm.ents.get(&n) else {
            return;
        };
        (e.obj, e.origin, e.angles, zm.now_ms)
    };
    // Up/down loops start silent (`setloopstate(alias, 0, 0)`); the rest at
    // their alias's level until the first step sets them.
    let level = ty
        .parts
        .iter()
        .map(|p| match p.drive {
            Drive::Up | Drive::Down | Drive::Vertical | Drive::Turn => (0.0, 1.0),
            _ => (1.0, 1.0),
        })
        .collect();
    world
        .get_resource_or_insert_with(EngineSounds::default)
        .by_ent
        .insert(
            n,
            Engine {
                obj,
                vehicletype: vehicletype.to_ascii_lowercase(),
                ty,
                started: false,
                last: origin,
                last_ms: now,
                speed_mph: 0.0,
                run_at: now,
                window_at: now + 100,
                window_z: origin[2],
                window_yaw: angles[1].abs(),
                z_diff: 0.0,
                level,
            },
        );
}

fn sndlog() -> bool {
    std::env::var_os("BO2MP_SNDLOG").is_some()
}

/// This tick's engine loops: (entity, alias, origin, volume, pitch). A
/// vehicle's loops end with it (`entityshutdown`).
pub(super) fn rows(world: &mut World) -> Vec<(u32, &'static str, [f32; 3], f32, f32)> {
    let Some(mut engines) = world.remove_resource::<EngineSounds>() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    {
        let zm = world.resource::<Zm>();
        let now = zm.now_ms;
        engines.by_ent.retain(|n, eng| {
            let alive = zm.ents.get(n).filter(|e| e.obj == eng.obj);
            let Some(e) = alive else {
                if sndlog() && eng.started {
                    for part in eng.ty.parts {
                        diag::info!(
                            Sim,
                            "bo2mp sndlog engine stop {} on ent {n} ({})",
                            part.alias,
                            eng.vehicletype
                        );
                    }
                }
                return false;
            };
            step(eng, e.origin, e.angles, now);
            if !eng.started {
                eng.started = true;
                if sndlog() {
                    for part in eng.ty.parts {
                        diag::info!(
                            Sim,
                            "bo2mp sndlog engine start {} on ent {n} ({})",
                            part.alias,
                            eng.vehicletype
                        );
                    }
                }
            }
            for (part, (vol, pitch)) in eng.ty.parts.iter().zip(&eng.level) {
                out.push((*n, part.alias, e.origin, *vol, *pitch));
            }
            true
        });
    }
    world.insert_resource(engines);
    out
}

fn step(eng: &mut Engine, origin: [f32; 3], angles: [f32; 3], now: i64) {
    let dt = (now - eng.last_ms) as f32 / 1000.0;
    if dt > 0.0 {
        let d = [
            origin[0] - eng.last[0],
            origin[1] - eng.last[1],
            origin[2] - eng.last[2],
        ];
        eng.speed_mph = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() / dt / MPH;
        eng.last = origin;
        eng.last_ms = now;
    }
    let ty = eng.ty;
    // The drone's 0.1 s windows (`drone_up_down_transition`,
    // `drone_rotate_angle`).
    if ty.updown && now >= eng.window_at {
        eng.z_diff = eng.window_z - origin[2];
        let turning = (eng.window_yaw - angles[1].abs()).abs();
        eng.window_z = origin[2];
        eng.window_yaw = angles[1].abs();
        eng.window_at = now + 100;
        let z = eng.z_diff;
        for (part, level) in ty.parts.iter().zip(eng.level.iter_mut()) {
            match part.drive {
                Drive::Up => {
                    *level = if z < 0.0 {
                        (scale_speed(5.0, 40.0, 0.0, 1.0, -z), scale_speed(5.0, 40.0, 0.9, 1.1, -z))
                    } else {
                        (0.0, 1.0)
                    };
                }
                Drive::Vertical => {
                    *level = if z < 0.0 {
                        (scale_speed(5.0, 50.0, 0.0, 1.0, -z), scale_speed(5.0, 50.0, 0.9, 1.1, -z))
                    } else {
                        (scale_speed(5.0, 50.0, 0.0, 1.0, z), scale_speed(5.0, 50.0, 0.95, 0.8, z))
                    };
                }
                // Rising leaves the down loop as it was (the script sets it
                // only while falling or level).
                Drive::Down if z >= 0.0 => {
                    *level = (scale_speed(5.0, 50.0, 0.0, 1.0, z), scale_speed(5.0, 50.0, 1.0, 0.8, z));
                }
                Drive::Turn => {
                    *level = (
                        scale_speed(0.0, 5.0, 0.0, 0.4, turning),
                        scale_speed(0.0, 4.0, 0.9, 1.05, turning),
                    );
                }
                _ => {}
            }
        }
    }
    if now >= eng.run_at {
        eng.run_at = now + ty.wait_ms;
        let s = eng.speed_mph;
        let vertical = if ty.updown {
            scale_speed(5.0, 50.0, 0.0, 1.0, eng.z_diff.abs())
        } else {
            0.0
        };
        for (part, level) in ty.parts.iter().zip(eng.level.iter_mut()) {
            if let Drive::Run([sv, v0, v1, sp, p0, p1]) = part.drive {
                *level = (
                    scale_speed(IDLE_RUN, sv, v0, v1, s) - vertical,
                    scale_speed(IDLE_RUN, sp, p0, p1, s),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_speed_clamps_and_maps() {
        assert_eq!(scale_speed(5.0, 65.0, 0.6, 0.8, 0.0), 0.6);
        assert_eq!(scale_speed(5.0, 65.0, 0.6, 0.8, 100.0), 0.8);
        assert!((scale_speed(5.0, 65.0, 0.6, 0.8, 35.0) - 0.7).abs() < 1e-6);
    }

    #[test]
    fn types_follow_the_client_script() {
        assert_eq!(type_of("heli_ai_mp").unwrap().parts[0].alias, "veh_hind_rotor_lfe");
        assert_eq!(type_of("heli_player_gunner_mp").unwrap().parts[1].alias, "veh_cobra_turbine");
        assert_eq!(type_of("QRDRONE_MP").unwrap().parts.len(), 6);
        assert!(type_of("plane").is_none());
    }
}
