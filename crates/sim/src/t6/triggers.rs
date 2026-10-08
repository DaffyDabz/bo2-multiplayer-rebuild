//! Triggers: touch triggers (`trigger_multiple`, `trigger_radius`) notify
//! `trigger` with the player every frame he is inside; use triggers
//! (`trigger_use`, `trigger_use_touch`, `trigger_radius_use`) when he
//! presses use inside the one he faces best. A trigger hidden from a player
//! (`setinvisibletoplayer`) ignores him.

use bevy_ecs::prelude::World;
use gsc_t6::{ObjRef, Value};

use super::{Ent, Zm, frame, with_vm};
use crate::bullet_collision::{PLAYER_MAXS, PLAYER_MINS};
use crate::script::host::triggers::{Volume, t6_volume};
use crate::world::ClientId;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Touch,
    Use,
}

pub(super) fn kind(classname: &str) -> Option<Kind> {
    match classname {
        "trigger_multiple" | "trigger_radius" | "trigger_once" | "trigger_box" => Some(Kind::Touch),
        "trigger_use" | "trigger_use_touch" | "trigger_radius_use" | "trigger_box_use" => {
            Some(Kind::Use)
        }
        _ => None,
    }
}

pub(super) fn brush_index(e: &Ent) -> Option<u32> {
    e.brush.as_deref()?.strip_prefix('*')?.parse().ok()
}

fn cylinder(e: &Ent) -> Option<(f32, f32)> {
    (e.radius > 0.0).then_some((e.radius, e.height.max(1.0)))
}

/// Does `e`'s volume touch the box `mins..maxs`?
pub(super) fn touches(world: &mut World, e: &Ent, mins: [f32; 3], maxs: [f32; 3]) -> bool {
    // A turned box: the other box's centre against it, grown by that box's
    // reach along each of its axes.
    if let Some([w, l, h]) = e.box_dims {
        let (f, r, u) = gsc_t6::math::angle_vectors(e.angles);
        let c: [f32; 3] = std::array::from_fn(|i| (mins[i] + maxs[i]) * 0.5 - e.origin[i]);
        let half: [f32; 3] = std::array::from_fn(|i| (maxs[i] - mins[i]) * 0.5);
        let reach =
            |a: [f32; 3]| a[0].abs() * half[0] + a[1].abs() * half[1] + a[2].abs() * half[2];
        let dot = |a: [f32; 3]| a[0] * c[0] + a[1] * c[1] + a[2] * c[2];
        return dot(f).abs() <= w * 0.5 + reach(f)
            && dot(r).abs() <= l * 0.5 + reach(r)
            && dot(u).abs() <= h * 0.5 + reach(u);
    }
    let f = frame(world);
    match t6_volume(&f, e.origin, cylinder(e), brush_index(e)) {
        Some(v) => v.touches(mins, maxs),
        None => false,
    }
}

/// A look-at use trigger's test: how far along the player's view (from
/// his eye, within the use radius) it meets the trigger's turned box, with
/// nothing solid in front of it (the chalk board the weapon sits on is
/// allowed: a few units). BO2 builds each wall buy's box from the
/// weapon model on the wall, nudged out of the wall so the view meets it
/// before the wall, and asks the engine for a look (`require_look_at`).
fn look_hit(world: &mut World, e: &Ent, eye: [f32; 3], dir: [f32; 3], reach: f32) -> Option<f32> {
    let [w, l, h] = e.box_dims?;
    let (f, r, u) = gsc_t6::math::angle_vectors(e.angles);
    let rel = gsc_t6::math::sub(eye, e.origin);
    let (mut near, mut far) = (0.0f32, reach);
    for (axis, half) in [(f, w * 0.5), (r, l * 0.5), (u, h * 0.5)] {
        let o = gsc_t6::math::dot(rel, axis);
        let d = gsc_t6::math::dot(dir, axis);
        if d.abs() < 1e-6 {
            if o.abs() > half {
                return None;
            }
            continue;
        }
        let (a, b) = ((-half - o) / d, (half - o) / d);
        near = near.max(a.min(b));
        far = far.min(a.max(b));
        if near > far {
            return None;
        }
    }
    let to: [f32; 3] = std::array::from_fn(|i| eye[i] + dir[i] * near);
    let t = frame(world).trace_static_world(
        eye,
        to,
        [0.0; 3],
        [0.0; 3],
        crate::bullet_collision::MASK_SHOT,
    );
    (t.fraction.clamp(0.0, 1.0) * near >= near - 8.0).then_some(near)
}

pub(super) fn center(world: &mut World, e: &Ent) -> [f32; 3] {
    if e.box_dims.is_some() {
        return e.origin;
    }
    if cylinder(e).is_some() {
        return [e.origin[0], e.origin[1], e.origin[2] + e.height * 0.5];
    }
    let f = frame(world);
    let model = brush_index(e).and_then(|n| {
        f.clip_cmodels()
            .models
            .get(n as usize)
            .map(|m| (m.mins, m.maxs))
    });
    match model {
        Some((lo, hi)) => std::array::from_fn(|i| e.origin[i] + (lo[i] + hi[i]) * 0.5),
        None => e.origin,
    }
}

pub(super) fn player_box(world: &mut World, client: u32) -> Option<([f32; 3], [f32; 3])> {
    let o = frame(world).player(ClientId(client))?.origin;
    Some((
        std::array::from_fn(|i| o[i] + PLAYER_MINS[i]),
        std::array::from_fn(|i| o[i] + PLAYER_MAXS[i]),
    ))
}

/// Fire the frame's triggers.
pub(super) fn dispatch(world: &mut World) {
    let players: Vec<(u32, ObjRef)> = world
        .resource::<Zm>()
        .players
        .iter()
        .filter(|(_, p)| p.sessionstate == "playing")
        .map(|(c, p)| (*c, p.obj))
        .collect();
    let triggers: Vec<(u32, Ent, Kind)> = world
        .resource::<Zm>()
        .ents
        .iter()
        .filter_map(|(n, e)| kind(&e.classname).map(|k| (*n, e.clone(), k)))
        .filter(|(_, e, _)| e.obj.is_some() && !e.trigger_off)
        .collect();
    // The use reach: the scripts raise player_useRadius_zm to their
    // largest trigger's (64 on Nuketown).
    let use_radius = with_vm(world, |vm, _| {
        vm.dvars
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("player_useRadius_zm"))
            .and_then(|(_, v)| v.parse::<f32>().ok())
    })
    .flatten()
    .unwrap_or(64.0)
    .max(1.0);
    let mut fire: Vec<(ObjRef, ObjRef)> = Vec::new();
    for (client, pobj) in players {
        let Some((mins, maxs)) = player_box(world, client) else {
            continue;
        };
        let held = crate::script_player::buttons(&mut frame(world), ClientId(client))
            & (playerstate_iw4::buttons::USE | playerstate_iw4::buttons::USE_RELOAD)
            != 0;
        let pressed = {
            let mut zm = world.resource_mut::<Zm>();
            let was = zm.use_held.contains(&client);
            if held {
                zm.use_held.insert(client);
            } else {
                zm.use_held.remove(&client);
            }
            held && !was
        };
        let (eye, fwd) = {
            let f = frame(world);
            let ps = f.player(ClientId(client));
            let eye = ps.map_or([0.0; 3], |p| {
                [
                    p.origin[0],
                    p.origin[1],
                    p.origin[2] + p.view_height_current,
                ]
            });
            let fwd = gsc_t6::math::angle_vectors(ps.map_or([0.0; 3], |p| p.viewangles)).0;
            (eye, fwd)
        };
        let mut best: Option<(f32, ObjRef, Option<String>)> = None;
        for (_, e, k) in &triggers {
            if e.invisible_to.contains(&client) || e.invisible_to_all {
                continue;
            }
            let looked = *k == Kind::Use && e.look_at && e.box_dims.is_some();
            if looked {
                if look_hit(world, e, eye, fwd, use_radius).is_none() {
                    continue;
                }
            } else if !touches(world, e, mins, maxs) {
                continue;
            }
            let obj = e.obj.unwrap();
            match k {
                Kind::Touch => fire.push((obj, pobj)),
                Kind::Use => {
                    let c = center(world, e);
                    let d = gsc_t6::math::normalize(gsc_t6::math::sub(c, eye));
                    let score = gsc_t6::math::dot(d, fwd);
                    if best.as_ref().is_none_or(|b| score > b.0) {
                        best = Some((score, obj, e.hint.clone()));
                    }
                }
            }
        }
        {
            let mut zm = world.resource_mut::<Zm>();
            // IW4L_T6_HINTLOG=1: each change of the faced trigger's hint and
            // the trigger it came from.
            if std::env::var_os("IW4L_T6_HINTLOG").is_some() {
                let now_hint = best.as_ref().and_then(|b| b.2.clone());
                if zm.hints.get(&client) != now_hint.as_ref() {
                    let from = best.as_ref().and_then(|b| {
                        triggers
                            .iter()
                            .find(|(_, e, _)| e.obj == Some(b.1))
                            .map(|(n, e, _)| {
                                format!(
                                    "ent{n} {} at ({:.0} {:.0} {:.0}) angles {:?} box {:?}",
                                    e.classname,
                                    e.origin[0],
                                    e.origin[1],
                                    e.origin[2],
                                    e.angles,
                                    e.box_dims
                                )
                            })
                    });
                    diag::info!(
                        Sim,
                        "bo2zm t6 hint for {client}: {now_hint:?} from {from:?}"
                    );
                }
            }
            match &best {
                Some((_, _, Some(h))) => {
                    zm.hints.insert(client, h.clone());
                }
                _ => {
                    zm.hints.remove(&client);
                }
            }
        }
        if pressed && let Some((_, obj, _)) = best {
            if std::env::var("IW4L_T6_TRIGLOG").is_ok() {
                diag::info!(Sim, "bo2zm t6 use by {client} on {obj:?}");
            }
            fire.push((obj, pobj));
        }
    }
    if fire.is_empty() {
        return;
    }
    with_vm(world, |vm, world| {
        for (t, p) in fire {
            if vm.alive(t) {
                vm.notify_str(world, t, "trigger", &[Value::Object(p)]);
            }
        }
    });
}

#[allow(dead_code)]
fn _volume_type(_: Volume<'_>) {}
