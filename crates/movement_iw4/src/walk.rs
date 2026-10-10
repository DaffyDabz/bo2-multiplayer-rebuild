use playerstate_iw4::{PlayerState, UserCmd};

use crate::{
    AirMoveContext, CmdScaleWalkContext, CollisionBackend, JumpCheckContext, JumpCheckResult,
    JumpLaunchContext, MoveBounds, Pml, StanceSurface, accelerate, air_move, cmd_scale_walk,
    friction, jump, stance_surface_type, step_slide_move,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WalkMoveContext {
    pub cmd_scale: CmdScaleWalkContext,

    pub weapon_move_scale: f32,

    pub old_buttons: u32,

    pub jump: JumpLaunchContext,

    pub air: AirMoveContext,

    pub feel: crate::Bo2Feel, // bo2zm
}

#[allow(clippy::assign_op_pattern)]
pub fn walk_move<C: CollisionBackend>(
    ps: &mut PlayerState,
    pml: &mut Pml,
    cmd: &mut UserCmd,
    context: WalkMoveContext,
    bounds: MoveBounds,
    collision: &C,
) {
    if (ps.pm_flags & 0x2000) != 0 {
        prone_velocity_scale(ps, context.feel);
    }
    // bo2zm: Black Ops II slows sideways input while sprinting (not with
    // sprint in any direction).
    if context.feel.on && !context.feel.omni && (ps.pm_flags & 0x4000) != 0 {
        cmd.rightmove =
            ((cmd.rightmove as f32) * context.feel.sprint_strafe_speed_scale) as i32 as i8;
    }

    let gate = JumpCheckContext {
        time_since_jump: cmd.server_time.wrapping_sub(ps.jump_time),
        old_buttons: context.old_buttons,
        stance_surface_type: stance_surface_type(ps) as u8,
    };
    if let JumpCheckResult::Launched { .. } = jump::check(ps, pml, cmd, gate, context.jump) {
        air_move(ps, pml, cmd, context.air, bounds, collision);
        return;
    }

    friction(ps, pml, context.feel.on);

    let cmd_scale = crate::feel::omni_cmd_scale(ps, context.cmd_scale, context.feel); // bo2zm
    let command_scale =
        cmd_scale_walk(ps, cmd, cmd_scale) * crate::damage_scale_walk(ps.damage_timer);
    crate::walk_move_drop_damage_timer(ps, pml.frametime);
    let mut forward = pml.forward;
    let mut right = pml.right;
    forward[2] = 0.0;
    right[2] = 0.0;
    normalize(&mut forward);
    normalize(&mut right);

    let mut wishdir = [
        (cmd.rightmove as f32) * right[0] + (cmd.forwardmove as f32) * forward[0],
        (cmd.rightmove as f32) * right[1] + (cmd.forwardmove as f32) * forward[1],
        (cmd.rightmove as f32) * right[2] + (cmd.forwardmove as f32) * forward[2],
    ];
    let wishspeed = normalize(&mut wishdir);
    clip_to_ground_plane(&mut wishdir, &pml.ground_trace[1..4]);

    accelerate(
        ps,
        pml,
        &wishdir,
        wishspeed * context.weapon_move_scale * command_scale,
        walk_accel_scale(ps, pml),
    );

    if (pml.ground_trace[4] & 2) != 0 || (ps.pm_flags & 0x100) != 0 {
        ps.velocity[2] -= (ps.gravity as f32) * pml.frametime;
    }

    clip_to_ground_plane(&mut ps.velocity, &pml.ground_trace[1..4]);
    if ps.velocity[0] != 0.0 || ps.velocity[1] != 0.0 {
        step_slide_move(
            ps,
            pml,
            collision,
            bounds.mins,
            bounds.maxs,
            bounds.tracemask,
            None,
        );
    }
}

fn prone_velocity_scale(ps: &mut PlayerState, feel: crate::Bo2Feel) {
    // bo2zm: Black Ops II only slows a landing to 0.5 when you came down at
    // least 18 units above the take-off; a flat landing keeps 0.65.
    let raise = if feel.on {
        crate::feel::JUMP_LAND_RAISE
    } else {
        0.0
    };
    let mut scale = 1.0_f32;
    if ps.pm_time < 0x709 {
        if ps.pm_time == 0 {
            if ps.jump_origin_z + raise <= ps.origin[2] {
                ps.pm_time = 0x4b0;
                scale = 0.5;
            } else {
                ps.pm_time = 0x708;
                scale = 0.65;
            }
        }
    } else {
        ps.pm_flags &= 0xffbfdfff;
        ps.jump_origin_z = 0.0;
        scale = 0.65;
    }

    let slowdown = !feel.on || feel.jump_slowdown;
    if (ps.pm_flags & 0x400000) == 0 && slowdown {
        ps.velocity[0] *= scale;
        ps.velocity[1] *= scale;
        ps.velocity[2] *= scale;
    }
}

fn normalize(vector: &mut [f32; 3]) -> f32 {
    let length = libm::sqrtf(vector[0] * vector[0] + vector[1] * vector[1] + vector[2] * vector[2]);
    let divisor = if length <= 0.0 { 1.0 } else { length };
    let scale = 1.0 / divisor;
    vector[0] *= scale;
    vector[1] *= scale;
    vector[2] *= scale;
    length
}

fn walk_accel_scale(ps: &PlayerState, pml: &Pml) -> f32 {
    const ACCEL_PRONE: f32 = 19.0;

    const ACCEL_CROUCH: f32 = 12.0;

    const ACCEL_STAND: f32 = 9.0;

    const ACCEL_SLICK: f32 = 1.0;

    const SLOW_WALK_SCALE: f32 = 0.25;

    let slick = (pml.ground_trace[4] & 2) != 0 || (ps.pm_flags & 0x100) != 0;
    let mut accel = if slick {
        ACCEL_SLICK
    } else {
        match stance_surface_type(ps) {
            StanceSurface::Prone | StanceSurface::LastStand => ACCEL_PRONE,
            StanceSurface::Crouch => ACCEL_CROUCH,
            StanceSurface::Stand => ACCEL_STAND,
        }
    };
    if (ps.pm_flags & 0x80) != 0 {
        accel *= SLOW_WALK_SCALE;
    }
    accel
}

fn clip_to_ground_plane(vector: &mut [f32; 3], normal: &[u32]) {
    let normal = [
        f32::from_bits(normal[0]),
        f32::from_bits(normal[1]),
        f32::from_bits(normal[2]),
    ];
    crate::project_velocity(vector, &normal);
}

#[cfg(test)]
mod tests {
    use playerstate_iw4::PlayerState;

    use super::prone_velocity_scale;
    use crate::Bo2Feel;

    const BO2: Bo2Feel = Bo2Feel {
        on: true,
        sprint_strafe_speed_scale: 0.667,
        jump_slowdown: true,
        ..Bo2Feel::IW4
    };

    fn landed(raise: f32, feel: Bo2Feel) -> f32 {
        let mut ps = PlayerState::ZERO;
        ps.pm_flags = 0x2000;
        ps.jump_origin_z = 100.0;
        ps.origin = [0.0, 0.0, 100.0 + raise];
        ps.velocity = [100.0, 0.0, 0.0];
        prone_velocity_scale(&mut ps, feel);
        ps.velocity[0]
    }

    #[test]
    fn flat_landing_keeps_065_in_bo2() {
        assert!((landed(0.0, BO2) - 65.0).abs() < 1e-3);
        assert!((landed(0.0, Bo2Feel::IW4) - 50.0).abs() < 1e-3);
    }

    #[test]
    fn landing_18_up_slows_to_half_in_bo2() {
        assert!((landed(18.0, BO2) - 50.0).abs() < 1e-3);
        assert!((landed(17.0, BO2) - 65.0).abs() < 1e-3);
    }

    #[test]
    fn slowdown_off_keeps_speed() {
        let off = Bo2Feel {
            jump_slowdown: false,
            ..BO2
        };
        assert!((landed(0.0, off) - 100.0).abs() < 1e-3);
    }
}
