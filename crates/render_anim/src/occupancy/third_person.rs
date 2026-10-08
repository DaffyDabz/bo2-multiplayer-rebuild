use asset_world::ClipCollision;
use net::PresentedSnapshot;
use playerstate_iw4::{
    CG_CAMERA_PULLBACK_BOX_HALF, CG_CAMERA_PULLBACK_CLIPMASK, CG_THIRD_PERSON_RANGE_DEFAULT,
    KillCamMode, OffsetThirdPersonViewInputs, ThirdPersonViewInputs, is_third_person_view,
    offset_third_person_view,
};
use sim::ClientId;

use render_scene::WorldCameraPose;

pub const CG_THIRD_PERSON_ANGLE_MP: f32 = 356.0;

pub fn presented_is_third_person(
    presented: &PresentedSnapshot,
    local: ClientId,
    in_killcam: bool,
) -> bool {
    let Some(ps) = presented.player(local) else {
        return false;
    };
    if in_killcam && ps.kill_cam_entity != playerstate_iw4::ENTITYNUM_NONE {
        return true;
    }
    if remote_missile_camera(presented, local, 0).is_some()
        || vehicle_view(presented, local).is_some()
    {
        return true;
    }
    is_third_person_view(ThirdPersonViewInputs {
        pm_type: ps.pm_type,
        other_flags: ps.other_flags,
        link_flags: ps.link_flags,
        cg_third_person: false,
        in_killcam,
        killcam_mode: KillCamMode::Mode0,
    })
}

pub fn remote_missile_camera(
    presented: &PresentedSnapshot,
    local: ClientId,
    at_time: i32,
) -> Option<WorldCameraPose> {
    let link = presented
        .snapshot()?
        .meta
        .for_client(local)?
        .remote_missile
        .filter(|link| link.unlink_at_ms.is_none())?;
    let missile = presented
        .presented_projectiles()
        .iter()
        .find(|p| p.authoritative_id() == Some(link.projectile))?;
    Some(WorldCameraPose {
        origin: missile.origin_at(at_time),
        angles: link.angles,
    })
}

/// bo2mp: riding a Black Ops II scorestreak vehicle (`bo2mp_vehicle_view`
/// from the server, the vehicle def's own values): "<script model> 0
/// <range> <height> 0 <up> <down> <fov>" sees it from behind (the RC-XD),
/// "<script model> 1 <x> <y> <z> <up> <down> <fov>" from its `tag_player`
/// (the Dragonfire, the VTOL Warship's gun); then its entity number.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VehicleView {
    pub model: u32,
    pub first_person: bool,
    /// Third person: range, height. First person: the tag in model space.
    pub at: [f32; 3],
    /// How far he may look up and down.
    pub pitch: [f32; 2],
    /// Its field of view (0: his own).
    pub fov: f32,
    /// Its entity number (its gun's fire events).
    pub number: i32,
}

pub fn vehicle_view(presented: &PresentedSnapshot, local: ClientId) -> Option<VehicleView> {
    let snapshot = presented.snapshot()?;
    let text = snapshot
        .meta
        .script_dvars(local)
        .string("bo2mp_vehicle_view")?;
    let mut parts = text.split_whitespace();
    let model = parts.next()?.parse().ok()?;
    let mode: u8 = parts.next()?.parse().ok()?;
    let mut num = || {
        parts
            .next()
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.0)
    };
    let at = [num(), num(), num()];
    let pitch = [num(), num()];
    let fov = num();
    let number = num() as i32;
    Some(VehicleView {
        model,
        first_person: mode == 1,
        at,
        pitch,
        fov,
        number,
    })
}

/// His view riding the vehicle whose model sits at `origin`, turned by
/// `rotation`: from its tag (first person), or the vehicle's third-person
/// camera - `range` back from a point `height` over it, along his view,
/// stopped by walls.
pub fn vehicle_camera(
    presented: &PresentedSnapshot,
    local: ClientId,
    view: VehicleView,
    origin: [f32; 3],
    rotation: bevy::math::Quat,
    clip: Option<&ClipCollision>,
) -> WorldCameraPose {
    let mut angles = presented.player(local).map_or([0.0; 3], |ps| ps.viewangles);
    if view.pitch != [0.0; 2] {
        angles[0] = angles[0].clamp(-view.pitch[0], view.pitch[1]);
    }
    angles[2] = 0.0;
    let o = bevy::math::Vec3::from_array(origin);
    if view.first_person {
        let eye = o + rotation * bevy::math::Vec3::from_array(view.at);
        return WorldCameraPose {
            origin: eye.to_array(),
            angles,
        };
    }
    let [range, height, _] = view.at;
    let look_at = o + rotation * bevy::math::Vec3::new(0.0, 0.0, height);
    let (fwd, _, _) = math_iw4::angle_vectors(angles);
    let want = look_at - bevy::math::Vec3::from_array(fwd) * range;
    let half = VEHICLE_CAMERA_BOX_HALF;
    let fraction = clip.map_or(1.0, |clip| {
        clip.sweep_box(
            look_at.to_array(),
            want.to_array(),
            [-half; 3],
            [half; 3],
            CG_CAMERA_PULLBACK_CLIPMASK,
        )
        .fraction
        .clamp(0.0, 1.0)
    });
    WorldCameraPose {
        origin: look_at.lerp(want, fraction).to_array(),
        angles,
    }
}

/// The vehicle camera's box against walls (the engine's own size).
const VEHICLE_CAMERA_BOX_HALF: f32 = 6.0;

pub fn death_watch_camera(
    presented: &PresentedSnapshot,
    local: ClientId,
    clip: Option<&ClipCollision>,
    corpse_j_mainroot: Option<[f32; 3]>,
) -> Option<WorldCameraPose> {
    let ps = presented.player(local)?;
    let clip = clip?;
    let offset = presented.view_offset();
    let yaw = presented
        .snapshot()?
        .meta
        .for_client(local)
        .map(|meta| meta.look_at_killer_yaw as f32)
        .unwrap_or(ps.viewangles[1]);

    let half = CG_CAMERA_PULLBACK_BOX_HALF;
    let trace = |start: [f32; 3], end: [f32; 3]| {
        clip.sweep_box(
            start,
            end,
            [-half, -half, -half],
            [half, half, half],
            CG_CAMERA_PULLBACK_CLIPMASK,
        )
        .fraction
    };
    let view = offset_third_person_view(
        OffsetThirdPersonViewInputs {
            origin: [
                ps.origin[0] + offset[0],
                ps.origin[1] + offset[1],
                ps.origin[2] + offset[2],
            ],
            view_height_current: ps.view_height_current,
            viewangles: ps.viewangles,
            pm_type: ps.pm_type,
            look_at_killer_yaw: yaw,

            corpse_j_mainroot,
            other_flags: ps.other_flags,
            delta_time: ps.delta_time,
            cg_third_person_angle: CG_THIRD_PERSON_ANGLE_MP,
            cg_third_person_range: CG_THIRD_PERSON_RANGE_DEFAULT,
        },
        trace,
    );
    Some(WorldCameraPose {
        origin: view.origin,
        angles: view.angles,
    })
}
