//! Riding and driving scorestreak vehicles (`vehicle usevehicle(player,
//! seat)`): the RC-XD and the Dragonfire are remote controlled, the VTOL
//! Warship is ridden as its gunner. `player unlink()` ends it (BO2's
//! scripts wait for his `unlink`).
//!
//! BO2's scripts run the streak itself (its timer, the RC-XD blowing up when
//! he fires, the HUD's client fields). The engine half kept here: his body
//! stays where he stood while his move keys, view and buttons drive the
//! vehicle; his own gun stays quiet; his screen rides it (the client's
//! vehicle view, `bo2mp_vehicle_view`), and BO2's HUD hears which vehicle
//! he is in (`bo2mp_vehicle`).

use bevy_ecs::prelude::World;
use gsc_t6::{Value, Vm};

use super::super::{T6VehicleDrive, Zm, arg, entnum, frame};
use super::craft::Aim;
use super::{notify_all, streaks};
use crate::world::ClientId;
use playerstate_iw4::buttons;

/// How a vehicle is driven.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Drive {
    /// On its wheels (RC-XD): forward and back, turning to his view.
    Ground,
    /// A small drone (Dragonfire): flies where he looks, up on jump, down on
    /// crouch.
    Air,
    /// He is its gunner (VTOL Warship): it flies its own path, he aims.
    Gunner,
}

#[derive(Clone, Debug)]
pub(crate) struct Ride {
    pub vehicle: u32,
    pub drive: Drive,
    /// Its own driving and camera values (his zones' vehicle def).
    pub info: T6VehicleDrive,
    /// Where his view sits on it in first person (its model's
    /// `tag_player`, model space).
    pub eye: [f32; 3],
    /// Its speed: forward for a ground vehicle, a velocity for a drone.
    pub vel: [f32; 3],
    /// Vertical speed (a ground vehicle falling or jumping).
    pub fall: f32,
    /// When each gun may fire again (server ms): turret, gunner.
    pub next_fire: [i64; 2],
    /// His client was told which model his view rides (once it has one).
    pub view_sent: bool,
}

/// A ground vehicle's box (half width, height).
const GROUND_HALF: f32 = 10.0;
const GROUND_HEIGHT: f32 = 12.0;
/// How high a step it drives up (a player's step).
const GROUND_STEP: f32 = 18.0;

/// A vehicle on wheels or treads (the RC-XD, the AGR).
pub(super) fn is_ground(kind: &str) -> bool {
    kind.starts_with("rc_car") || kind.contains("tank")
}

fn drive_of(kind: &str) -> Drive {
    if is_ground(kind) {
        Drive::Ground
    } else if kind.starts_with("qrdrone") {
        Drive::Air
    } else {
        Drive::Gunner
    }
}

/// A tag's place in a model at rest (model space), from his zones.
pub(super) fn tag_in_model(world: &mut World, model: &str, tag: &str) -> Option<[f32; 3]> {
    let cap = frame(world).model_capability(model)??;
    let bone = cap
        .pose
        .bone_names
        .iter()
        .position(|n| n.eq_ignore_ascii_case(tag))?;
    cap.pose.base_mat.get(bone).map(|(_, t)| t.to_array())
}

/// A model-space point on a vehicle turned by `yaw` and placed at `origin`.
fn on_vehicle(origin: [f32; 3], yaw: f32, local: [f32; 3]) -> [f32; 3] {
    let (s, c) = yaw.to_radians().sin_cos();
    [
        origin[0] + c * local[0] - s * local[1],
        origin[1] + s * local[0] + c * local[1],
        origin[2] + local[2],
    ]
}

/// Ends his ride: his own controls back, his screen back, `unlink`.
pub(super) fn end_ride(world: &mut World, client: u32) {
    let Some(ride) = streaks(world).rides.remove(&client) else {
        return;
    };
    if let Some(c) = streaks(world).craft.get_mut(&ride.vehicle) {
        c.rider = None;
    }
    {
        let mut f = frame(world);
        if f.client_meta(ClientId(client)).is_some() {
            let m = f.client_meta_mut(ClientId(client));
            m.controls.weapons_disabled = false;
            m.controls.jump_disabled = false;
            m.view_pitch_clamp = None;
        }
    }
    super::super::set_client_dvar(world, client, "bo2mp_vehicle_view", "");
    super::super::set_client_dvar(world, client, "bo2mp_vehicle", "");
    notify_all(world, vec![(client, "unlink", Vec::new())]);
}

pub(super) fn bind(vm: &mut Vm<World>) {
    macro_rules! m {
        ($name:literal, $body:expr) => {
            vm.bind($name, true, $body);
        };
    }
    // `vehicle usevehicle(player, seat)`: he takes it.
    m!("usevehicle", |vm, world, s, a| {
        let (Some(v), Ok(id)) = (entnum(vm, s), super::super::client(vm, world, arg(a, 0))) else {
            return Ok(Value::Undefined);
        };
        let seat = arg(a, 1).as_int().unwrap_or(0);
        let kind = match s {
            Value::Object(o) => {
                let f = vm.intern("vehicletype");
                vm.to_text(&vm.raw_field(*o, f))
            }
            _ => String::new(),
        };
        end_ride(world, id.0);
        let drive = drive_of(&kind);
        let info = streaks(world)
            .vehicle_drive
            .get(&kind.to_ascii_lowercase())
            .copied()
            .unwrap_or_default();
        let model = world
            .resource::<Zm>()
            .ents
            .get(&v)
            .map(|e| e.model.clone())
            .unwrap_or_default();
        let eye = tag_in_model(world, &model, "tag_player").unwrap_or([0.0; 3]);
        streaks(world).rides.insert(
            id.0,
            Ride {
                vehicle: v,
                drive,
                info,
                eye,
                vel: [0.0; 3],
                fall: 0.0,
                next_fire: [0; 2],
                view_sent: false,
            },
        );
        // His view keeps to its limits: the gun's in first person, the
        // camera's in third.
        let pitch = if info.camera_mode == 0 {
            info.turret_pitch
        } else {
            info.camera_pitch
        };
        {
            let mut f = frame(world);
            if f.client_meta(id).is_some() {
                f.client_meta_mut(id).view_pitch_clamp = (pitch != [0.0; 2]).then_some(pitch);
            }
        }
        if let Some(c) = streaks(world).craft.get_mut(&v) {
            c.rider = Some(id.0);
            // Driven, not flown to goals.
            if drive != Drive::Gunner {
                c.goal = None;
            }
        }
        // His view starts the vehicle's way.
        let yaw = world
            .resource::<Zm>()
            .ents
            .get(&v)
            .map_or(0.0, |e| e.angles[1]);
        if drive != Drive::Gunner {
            super::super::set_player_view(world, id, [0.0, yaw, 0.0]);
        }
        super::super::set_client_dvar(world, id.0, "bo2mp_vehicle", &format!("{kind} {seat}"));
        diag::info!(
            Sim,
            "bo2mp vehicle: player {} takes {kind} (ent {v}, seat {seat}, {drive:?}, {model} \
             tag_player {eye:?}, {info:?})",
            id.0
        );
        Ok(Value::Undefined)
    });
    // He lets go (`player unlink()`), or an entity comes off its parent.
    m!("unlink", |vm, world, s, _| {
        let Some(n) = entnum(vm, s) else {
            return Ok(Value::Undefined);
        };
        if streaks(world).rides.contains_key(&n) {
            end_ride(world, n);
        }
        let mut zm = world.resource_mut::<Zm>();
        if let Some(p) = zm.players.get_mut(&n) {
            p.linked = None;
        } else if let Some(e) = zm.ents.get_mut(&n) {
            e.link = None;
        }
        Ok(Value::Undefined)
    });
    m!("isremotecontrolling", |vm, world, s, _| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        Ok(Value::bool(
            streaks(world)
                .rides
                .get(&n)
                .is_some_and(|r| r.drive != Drive::Gunner),
        ))
    });
    m!("isinvehicle", |vm, world, s, _| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        Ok(Value::bool(streaks(world).rides.contains_key(&n)))
    });
    m!("getvehicleoccupied", |vm, world, s, _| {
        let n = entnum(vm, s).unwrap_or(u32::MAX);
        let v = streaks(world).rides.get(&n).map(|r| r.vehicle);
        Ok(v.map_or(Value::Undefined, |v| super::craft::world_obj(world, v)))
    });
    // `vehicle launchvehicle(velocity, point, ...)`: a push (the RC-XD's
    // jump).
    m!("launchvehicle", |vm, world, s, a| {
        let v = entnum(vm, s).unwrap_or(u32::MAX);
        let push = arg(a, 0).as_vec3().unwrap_or([0.0; 3]);
        let mut st = streaks(world);
        if let Some(r) = st.rides.values_mut().find(|r| r.vehicle == v) {
            // BO2's push is a small number the physics scales; a hop.
            r.fall = (push[2] * 30.0).clamp(0.0, 400.0);
        }
        Ok(Value::Undefined)
    });
}

/// Each tick (his command is in): drive each ridden vehicle and keep his
/// body where it stood.
pub(super) fn advance(world: &mut World) {
    let rides: Vec<(u32, Ride)> = streaks(world)
        .rides
        .iter()
        .map(|(c, r)| (*c, r.clone()))
        .collect();
    if rides.is_empty() {
        return;
    }
    let dt = crate::MATCH_TICK_MS as f32 / 1000.0;
    let now = world.resource::<Zm>().now_ms;
    for (c, mut ride) in rides {
        let id = ClientId(c);
        // Gone (blown up, deleted) or he died: the ride ends.
        let alive = frame(world).player(id).is_some_and(|ps| ps.health > 0);
        let Some((pos, ang)) = world
            .resource::<Zm>()
            .ents
            .get(&ride.vehicle)
            .map(|e| (e.origin, e.angles))
        else {
            end_ride(world, c);
            continue;
        };
        if !alive {
            end_ride(world, c);
            continue;
        }
        // His screen rides the vehicle's model once it has one (it is
        // made the tick after the vehicle), with its def's own values:
        // "<model> 0 <range> <height> 0 <up> <down> <fov>" behind it (third
        // person), "<model> 1 <x> <y> <z> <up> <down> <fov>" from its
        // `tag_player` (first person).
        if !ride.view_sent {
            let model = world
                .resource::<Zm>()
                .presences
                .by_ent
                .get(&ride.vehicle)
                .map(|p| (p.id.to_wire(), p.number));
            if let Some((m, number)) = model {
                let i = ride.info;
                let (mode, at, [up, down]) = if i.camera_mode == 0 {
                    (1, ride.eye, i.turret_pitch)
                } else {
                    (0, [i.camera_range, i.camera_height, 0.0], i.camera_pitch)
                };
                let [x, y, z] = at;
                let view = format!(
                    "{m} {mode} {x} {y} {z} {up} {down} {} {number}",
                    i.camera_fov
                );
                super::super::set_client_dvar(world, c, "bo2mp_vehicle_view", &view);
                ride.view_sent = true;
            }
        }
        // His command: read it, then keep his body still.
        let (fwd, right, held) = {
            let mut req = world.resource_mut::<crate::step::StepRequest>();
            let mut out = (0i8, 0i8, 0u32);
            for (cid, cmd) in &mut req.input.cmds {
                if *cid != id {
                    continue;
                }
                out = (cmd.forwardmove, cmd.rightmove, out.2 | cmd.buttons);
                cmd.forwardmove = 0;
                cmd.rightmove = 0;
            }
            out
        };
        {
            let mut f = frame(world);
            if f.client_meta(id).is_some() {
                let ctl = &mut f.client_meta_mut(id).controls;
                ctl.weapons_disabled = true;
                ctl.jump_disabled = true;
            }
        }
        let view = frame(world).player(id).map_or([0.0; 3], |ps| ps.viewangles);
        let (f3, _, _) = gsc_t6::math::angle_vectors(view);
        let (sy, cy) = view[1].to_radians().sin_cos();
        let flat_fwd = [cy, sy, 0.0];
        let flat_right = [sy, -cy, 0.0];
        let fwd = f32::from(fwd) / 127.0;
        let right = f32::from(right) / 127.0;
        let mut new_pos = pos;
        let mut new_ang = ang;
        let info = ride.info;
        // Its speed heads for what his keys ask at its own acceleration.
        let toward = |v: f32, want: f32, accel: f32| {
            let d = want - v;
            v + d.clamp(-accel * dt, accel * dt)
        };
        match ride.drive {
            Drive::Ground => {
                let top = if fwd < 0.0 {
                    info.max_speed * info.reverse_scale
                } else {
                    info.max_speed
                };
                ride.vel[0] = toward(ride.vel[0], top * fwd, info.accel);
                let step = ride.vel[0] * dt;
                let mins = [-GROUND_HALF, -GROUND_HALF, 2.0];
                let maxs = [GROUND_HALF, GROUND_HALF, GROUND_HEIGHT];
                let sweep = |world: &mut World, from: [f32; 3], to: [f32; 3]| {
                    let t = frame(world).trace_static_world(
                        from,
                        to,
                        mins,
                        maxs,
                        crate::bullet_collision::MASK_SHOT,
                    );
                    let f = if t.startsolid != 0 {
                        0.0
                    } else {
                        t.fraction.clamp(0.0, 1.0)
                    };
                    std::array::from_fn::<f32, 3, _>(|i| from[i] + (to[i] - from[i]) * f)
                };
                // Up a step, along, then down onto the ground under it
                // (kerbs, slopes and stairs), falling when nothing is there.
                let raised = sweep(world, pos, [pos[0], pos[1], pos[2] + GROUND_STEP]);
                let lift = raised[2] - pos[2];
                let want = [
                    raised[0] + flat_fwd[0] * step,
                    raised[1] + flat_fwd[1] * step,
                    raised[2],
                ];
                new_pos = sweep(world, raised, want);
                if step.abs() > 0.01 && (new_pos[0] - want[0]).hypot(new_pos[1] - want[1]) > 0.1 {
                    ride.vel[0] = 0.0;
                }
                ride.fall -= 800.0 * dt;
                let drop = [new_pos[0], new_pos[1], new_pos[2] - lift + ride.fall * dt];
                let landed = sweep(world, new_pos, drop);
                if (landed[2] - drop[2]).abs() > 0.01 {
                    ride.fall = 0.0;
                }
                new_pos = landed;
                new_ang = [0.0, view[1], 0.0];
            }
            Drive::Air => {
                let mut climb = 0.0;
                if held & buttons::JUMP != 0 {
                    climb += 1.0;
                }
                if held & (buttons::CROUCH | buttons::PRONE) != 0 {
                    climb -= 1.0;
                }
                for i in 0..2 {
                    let want = (flat_fwd[i] * fwd + flat_right[i] * right) * info.max_speed;
                    ride.vel[i] = toward(ride.vel[i], want, info.accel);
                }
                ride.vel[2] = toward(
                    ride.vel[2],
                    climb * info.max_speed_vertical,
                    info.accel_vertical,
                );
                let want: [f32; 3] = std::array::from_fn(|i| pos[i] + ride.vel[i] * dt);
                let t = frame(world).trace_static_world(
                    pos,
                    want,
                    [-12.0, -12.0, -8.0],
                    [12.0, 12.0, 8.0],
                    crate::bullet_collision::MASK_SHOT,
                );
                let f = if t.startsolid != 0 {
                    0.0
                } else {
                    t.fraction.clamp(0.0, 1.0)
                };
                new_pos = std::array::from_fn(|i| pos[i] + (want[i] - pos[i]) * f);
                if f < 1.0 {
                    ride.vel = [0.0; 3];
                }
                new_ang = [0.0, view[1], 0.0];
            }
            Drive::Gunner => {}
        }
        if let Some(e) = world.resource_mut::<Zm>().ents.get_mut(&ride.vehicle) {
            e.origin = new_pos;
            e.angles = new_ang;
        }
        // His guns: fire shoots the turret's, aim-down-sights the gunner's
        // (the VTOL's rockets), where he looks. The RC-XD's fire is its
        // script's (it blows up).
        if ride.drive != Drive::Ground {
            let eye = on_vehicle(new_pos, new_ang[1], ride.eye);
            let far: [f32; 3] = std::array::from_fn(|i| eye[i] + f3[i] * 8192.0);
            let t = frame(world).trace_static_world(
                eye,
                far,
                [0.0; 3],
                [0.0; 3],
                crate::bullet_collision::MASK_SHOT,
            );
            let fr = t.fraction.clamp(0.0, 1.0);
            let aim: [f32; 3] = std::array::from_fn(|i| eye[i] + (far[i] - eye[i]) * fr);
            for (slot, button, gunner) in [
                (0usize, buttons::ATTACK, None),
                (1, buttons::ADS, Some(0usize)),
            ] {
                if held & button == 0 || now < ride.next_fire[slot] {
                    continue;
                }
                let interval =
                    super::craft::fire_from(world, ride.vehicle, gunner, Aim::Point(aim));
                if let Some(ms) = interval {
                    ride.next_fire[slot] = now + i64::from(ms.max(50));
                }
            }
        }
        if let Some(r) = streaks(world).rides.get_mut(&c) {
            r.vel = ride.vel;
            r.fall = ride.fall;
            r.next_fire = ride.next_fire;
            r.view_sent = ride.view_sent;
        }
    }
}
