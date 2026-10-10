//! A player's view on another entity's tag (`player
//! playerlinkweaponviewtodelta(ent, tag, fraction, right, left, up, down)`):
//! the Lodestar (_remotemortar.gsc) links his view to its drone's
//! `tag_player`. His body stays where he stood (BO2's script wants him on
//! the ground first); his view sits at the tag and turns with it as the
//! drone circles, kept to the arcs the script gives around the tag's own
//! facing. `player unlink()` ends it.
//!
//! His screen rides the tag (the client's vehicle view,
//! `bo2mp_vehicle_view`, which also hides the drone from him, as the
//! script's `setinvisibletoplayer(owner)` does), `geteye` answers from the
//! tag (the script aims its laser and missiles from there), and BO2's HUD
//! hears the Lodestar (`bo2mp_vehicle` "remote_mortar_mp"). The gun in his
//! hands does not fire while his view is away (his trigger is the script's:
//! `attackbuttonpressed` fires the drone's missiles), as on a vehicle ride.

use bevy_ecs::prelude::World;
use gsc_t6::{Value, Vm};

use super::super::{Zm, arg, entnum, frame, text};
use super::streaks;
use crate::world::ClientId;

/// His view on an entity's tag.
#[derive(Clone, Debug)]
pub(crate) struct ViewLink {
    pub ent: u32,
    /// The tag in its model's space.
    pub at: [f32; 3],
    /// The tag's own facing in its model: yaw, and pitch (down is +).
    pub yaw: f32,
    pub pitch: f32,
    /// How far he may look from the tag's facing: right, left, up, down.
    pub arcs: [f32; 4],
    /// The entity's yaw last tick (his view turns with it).
    pub last_yaw: f32,
    /// What BO2's HUD hears ("" none).
    pub hud: &'static str,
    pub view_sent: bool,
}

/// The lowest he may ever look (the game's own limit).
const PITCH_DOWN_MAX: f32 = 85.0;

/// A tag's model-space place and rotation (x, y, z, w).
fn tag_pose(world: &mut World, model: &str, tag: &str) -> Option<([f32; 3], [f32; 4])> {
    let cap = frame(world).model_capability(model)??;
    let bone = cap
        .pose
        .bone_names
        .iter()
        .position(|n| n.eq_ignore_ascii_case(tag))?;
    cap.pose
        .base_mat
        .get(bone)
        .map(|(q, t)| (t.to_array(), q.to_array()))
}

fn wrap(a: f32) -> f32 {
    let a = a.rem_euclid(360.0);
    if a > 180.0 { a - 360.0 } else { a }
}

/// His pitch limits ([up, down], as `view_pitch_clamp`) on this link.
fn pitch_clamp(link: &ViewLink) -> [f32; 2] {
    [
        -(link.pitch - link.arcs[2]),
        (link.pitch + link.arcs[3]).min(PITCH_DOWN_MAX),
    ]
}

/// The tag's place in the world on its entity now.
fn tag_world(world: &World, link: &ViewLink) -> Option<[f32; 3]> {
    let e = world.resource::<Zm>().ents.get(&link.ent)?;
    let (f, r, u) = gsc_t6::math::angle_vectors(e.angles);
    let [x, y, z] = link.at;
    Some(std::array::from_fn(|i| e.origin[i] + f[i] * x - r[i] * y + u[i] * z))
}

/// His eye while his view is linked: the tag.
pub(super) fn eye(world: &mut World, client: u32) -> Option<[f32; 3]> {
    let link = streaks(world).view_links.get(&client).cloned()?;
    tag_world(world, &link)
}

/// Ends his view link: his view and his own screen back.
pub(super) fn end(world: &mut World, client: u32) {
    let Some(link) = streaks(world).view_links.remove(&client) else {
        return;
    };
    {
        let mut f = frame(world);
        if f.client_meta(ClientId(client)).is_some() {
            let m = f.client_meta_mut(ClientId(client));
            m.controls.jump_disabled = false;
            m.controls.weapons_disabled = false;
            m.view_pitch_clamp = None;
        }
    }
    super::super::set_client_dvar(world, client, "bo2mp_vehicle_view", "");
    if !link.hud.is_empty() {
        super::super::set_client_dvar(world, client, "bo2mp_vehicle", "");
    }
    diag::info!(Sim, "bo2mp view link: player {client} lets go of ent {}", link.ent);
}

pub(super) fn bind(vm: &mut Vm<World>) {
    vm.bind("playerlinkweaponviewtodelta", true, |vm, world, s, a| {
        let id = super::super::client(vm, world, s)?;
        let Some(ent) = entnum(vm, arg(a, 0)) else {
            return Ok(Value::Undefined);
        };
        let tag = text(vm, a, 1);
        let num = |i: usize| arg(a, i).as_float().unwrap_or(0.0);
        let arcs = [num(3), num(4), num(5), num(6)];
        let Some((model, ent_yaw)) = world
            .resource::<Zm>()
            .ents
            .get(&ent)
            .map(|e| (e.model.clone(), e.angles[1]))
        else {
            return Ok(Value::Undefined);
        };
        end(world, id.0);
        super::ride::end_ride(world, id.0);
        let (at, q) = tag_pose(world, &model, &tag).unwrap_or(([0.0; 3], [0.0, 0.0, 0.0, 1.0]));
        // The tag's facing: its x axis turned by its rotation.
        let [x, y, z, w] = q;
        let fwd = [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y + w * z),
            2.0 * (x * z - w * y),
        ];
        let yaw = fwd[1].atan2(fwd[0]).to_degrees();
        let pitch = -fwd[2].clamp(-1.0, 1.0).asin().to_degrees();
        // The Lodestar's drone: BO2's HUD has its own screen (ReaperHUD).
        let hud = if model.starts_with("veh_t6_drone_pegasus") {
            "remote_mortar_mp"
        } else {
            ""
        };
        let link = ViewLink {
            ent,
            at,
            yaw,
            pitch,
            arcs,
            last_yaw: ent_yaw,
            hud,
            view_sent: false,
        };
        let clamp = pitch_clamp(&link);
        {
            let mut f = frame(world);
            if f.client_meta(id).is_some() {
                let m = f.client_meta_mut(id);
                m.view_pitch_clamp = Some(clamp);
                m.controls.jump_disabled = true;
                m.controls.weapons_disabled = true;
            }
        }
        if !hud.is_empty() {
            super::super::set_client_dvar(world, id.0, "bo2mp_vehicle", &format!("{hud} 0 - - - - -"));
        }
        diag::info!(
            Sim,
            "bo2mp view link: player {} on ent {ent} ({model} {tag} at {at:?}, facing yaw {yaw:.1} \
             pitch {pitch:.1}, arcs {arcs:?}, pitch limits {clamp:?})",
            id.0
        );
        streaks(world).view_links.insert(id.0, link);
        Ok(Value::Undefined)
    });
}

/// Each tick (his command is in): his body stays still, his view turns with
/// the entity and keeps to its arcs; gone or dead, the link ends.
pub(super) fn advance(world: &mut World) {
    let links: Vec<(u32, ViewLink)> = streaks(world)
        .view_links
        .iter()
        .map(|(c, l)| (*c, l.clone()))
        .collect();
    for (c, mut link) in links {
        let id = ClientId(c);
        let alive = frame(world).player(id).is_some_and(|ps| ps.health > 0);
        let ent_yaw = world.resource::<Zm>().ents.get(&link.ent).map(|e| e.angles[1]);
        let Some(ent_yaw) = ent_yaw.filter(|_| alive) else {
            end(world, c);
            continue;
        };
        // His body stays where he stood, his own gun quiet; his last
        // command's view.
        let cmd_yaw = {
            let mut req = world.resource_mut::<crate::step::StepRequest>();
            let mut last = None;
            for (cid, cmd) in &mut req.input.cmds {
                if *cid != id {
                    continue;
                }
                cmd.forwardmove = 0;
                cmd.rightmove = 0;
                last = Some(cmd.angles[1]);
            }
            last
        };
        let turn = wrap(ent_yaw - link.last_yaw);
        link.last_yaw = ent_yaw;
        let facing = ent_yaw + link.yaw;
        {
            let mut f = frame(world);
            if f.client_meta(id).is_some() {
                let ctl = &mut f.client_meta_mut(id).controls;
                ctl.jump_disabled = true;
                ctl.weapons_disabled = true;
            }
            if let Some(ps) = f.player_mut(id) {
                ps.delta_angles[1] += turn;
                ps.viewangles[1] = wrap(ps.viewangles[1] + turn);
                let commanded = cmd_yaw.map(|a| a as f32 * movement_iw4::SHORT2ANGLE);
                let view = commanded.map_or(ps.viewangles[1], |a| a + ps.delta_angles[1]);
                let rel = wrap(view - facing);
                let kept = rel.clamp(-link.arcs[0], link.arcs[1]);
                if kept != rel {
                    ps.delta_angles[1] += kept - rel;
                    ps.viewangles[1] = wrap(ps.viewangles[1] + kept - rel);
                }
                ps.delta_angles[1] = wrap(ps.delta_angles[1]);
            }
        }
        // His screen rides the tag once the entity has a model on his
        // client (made the tick after it): "<model> 1 <x> <y> <z> <up>
        // <down> <fov> <number>".
        if !link.view_sent {
            let shown = world
                .resource::<Zm>()
                .presences
                .by_ent
                .get(&link.ent)
                .map(|p| (p.id.to_wire(), p.number));
            if let Some((m, number)) = shown {
                let [x, y, z] = link.at;
                let [up, down] = pitch_clamp(&link);
                let view = format!("{m} 1 {x} {y} {z} {up} {down} 0 {number}");
                super::super::set_client_dvar(world, c, "bo2mp_vehicle_view", &view);
                link.view_sent = true;
            }
        }
        streaks(world).view_links.insert(c, link);
    }
}
