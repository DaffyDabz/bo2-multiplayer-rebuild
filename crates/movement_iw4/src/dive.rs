//! bo2zm: Black Ops II's dive to prone (the "dolphin dive"). Not part of
//! upstream IW4L.
//!
//! While sprinting on the ground, pressing prone (or holding stance on a
//! pad) launches the player forward in a low arc. On landing they slide on
//! their front into prone, then hold still a moment before they can crawl
//! or stand. BO2 names these phases in its dvars (`dtp_startup_delay`,
//! `dtp_min_speed`, `dtp_max_slide_duration`, `dtp_post_move_pause`), but
//! their values live in the executable, not the game files. The timing
//! below follows BO2's own dive animations (`pb_dive_prone`, 22 frames at
//! 30 = 0.733 s from take-off to touch-down; `pb_dive_prone_land`, 23
//! frames = 0.767 s on the ground; common_zm); the launch speed and height
//! are fitted to that timing and to BO2's sprint speed, and are meant to be
//! tuned by feel.
//!
//! State rides in the replicated player state, so the host and the local
//! prediction agree: `PMF_DIVE` while in the air, `PMF_DIVE_SLIDE` from
//! touch-down to the end of the pause, `jump_time` = when the current
//! phase began.

use playerstate_iw4::{ENTITYNUM_NONE, PlayerState, UserCmd, buttons, pm_flags};

use crate::{
    AirMoveContext, CollisionBackend, MoveBounds, Pml, air_move, end_sprint, player_prone_allowed,
    step_slide_move,
};

/// In the air, diving.
pub const PMF_DIVE: u32 = 0x0100_0000;
/// On the ground after a dive: sliding, then the pause.
pub const PMF_DIVE_SLIDE: u32 = 0x0200_0000;
/// After a dive: the player stays prone (as BO2 leaves them) until they
/// press jump, crouch or prone, whatever the keys' hold or toggle style.
pub const PMF_DIVE_PRONE: u32 = 0x0400_0000;

/// How long the player must have been sprinting before a dive can start.
const STARTUP_MS: i32 = 100;
/// The least ground speed a dive starts from.
const MIN_SPEED: f32 = 150.0;
/// The dive's ground speed at take-off (at least; a faster sprint keeps
/// its speed).
const LAUNCH_SPEED: f32 = 300.0;
/// The take-off's upward speed: with gravity 800 the player is airborne
/// for about 0.55 s and peaks about 30 units up, leaving the start and end
/// of the 0.733 s take-off animation for leaving and meeting the ground.
const LAUNCH_UP: f32 = 220.0;
/// The slide on the ground after touch-down, slowing to a stop.
const SLIDE_MS: i32 = 500;
/// The still pause after the slide (the landing animation's remainder).
const PAUSE_MS: i32 = 270;

/// A dive is under way (in the air, sliding or pausing).
pub fn active(ps: &PlayerState) -> bool {
    ps.pm_flags & (PMF_DIVE | PMF_DIVE_SLIDE) != 0
}

/// Whether this command starts a dive: sprinting on the ground long enough
/// and fast enough, and prone newly pressed.
pub fn wants(ps: &PlayerState, cmd: &UserCmd, old_buttons: u32) -> bool {
    if active(ps)
        || ps.pm_flags & pm_flags::SPRINTING == 0
        || ps.pm_flags & (pm_flags::LADDER | pm_flags::MANTLE | pm_flags::LAST_STAND) != 0
        || ps.ground_entity_num == ENTITYNUM_NONE
        || (ps.pm_type != 0 && ps.pm_type != 1)
    {
        return false;
    }
    // Prone newly pressed; or a stance toggle newly crouching (a pad's
    // stance button, as console BO2 dives on it; a held keyboard crouch
    // carries STANCE_HELD and only crouches).
    let new = cmd.buttons & !old_buttons;
    let prone = new & buttons::PRONE != 0;
    let pad_stance = new & buttons::CROUCH != 0 && cmd.buttons & buttons::STANCE_HELD == 0;
    if !prone && !pad_stance {
        return false;
    }
    if cmd.server_time.wrapping_sub(ps.last_sprint_start) < STARTUP_MS {
        return false;
    }
    let speed = libm::sqrtf(ps.velocity[0] * ps.velocity[0] + ps.velocity[1] * ps.velocity[1]);
    speed >= MIN_SPEED
}

/// Launch the dive: sprint ends, the player crouches (a smaller hull for
/// the flight) and leaves the ground along their run, or their view when
/// barely moving.
pub fn start(ps: &mut PlayerState, pml: &mut Pml, cmd: &UserCmd) {
    end_sprint(ps, cmd);
    let speed = libm::sqrtf(ps.velocity[0] * ps.velocity[0] + ps.velocity[1] * ps.velocity[1]);
    let mut dir = if speed > 1.0 {
        [ps.velocity[0] / speed, ps.velocity[1] / speed]
    } else {
        [pml.forward[0], pml.forward[1]]
    };
    let len = libm::sqrtf(dir[0] * dir[0] + dir[1] * dir[1]).max(1e-6);
    dir = [dir[0] / len, dir[1] / len];
    let ground_speed = speed.max(LAUNCH_SPEED);
    ps.velocity = [dir[0] * ground_speed, dir[1] * ground_speed, LAUNCH_UP];
    ps.pm_flags =
        (ps.pm_flags & !(pm_flags::PRONE | pm_flags::JUMPING)) | pm_flags::CROUCH | PMF_DIVE;
    ps.ground_entity_num = ENTITYNUM_NONE;
    ps.jump_time = cmd.server_time;
    ps.jump_origin_z = ps.origin[2];
    pml.walking = 0;
    pml.ground_plane = 0;
    pml.almost_ground_plane = 0;
}

/// Move one tick of a dive, the player's own input ignored. In the air:
/// gravity and collision only. On the ground: the slide slows evenly to a
/// stop over `SLIDE_MS`, then the player holds still until the pause ends.
pub fn advance<C: CollisionBackend>(
    ps: &mut PlayerState,
    pml: &mut Pml,
    cmd: &UserCmd,
    air: AirMoveContext,
    bounds: MoveBounds,
    collision: &C,
) {
    let mut still = *cmd;
    still.forwardmove = 0;
    still.rightmove = 0;
    if ps.pm_flags & PMF_DIVE != 0 || pml.walking == 0 {
        air_move(ps, pml, &still, air, bounds, collision);
        return;
    }
    let elapsed = cmd.server_time.wrapping_sub(ps.jump_time);
    let speed = libm::sqrtf(ps.velocity[0] * ps.velocity[0] + ps.velocity[1] * ps.velocity[1]);
    let next = if elapsed >= SLIDE_MS {
        0.0
    } else {
        (speed - LAUNCH_SPEED * pml.frametime * 1000.0 / SLIDE_MS as f32).max(0.0)
    };
    if speed > 1e-3 {
        ps.velocity[0] *= next / speed;
        ps.velocity[1] *= next / speed;
    }
    if (pml.ground_trace[4] & 2) != 0 {
        ps.velocity[2] -= (ps.gravity as f32) * pml.frametime;
    }
    let normal = [
        f32::from_bits(pml.ground_trace[1]),
        f32::from_bits(pml.ground_trace[2]),
        f32::from_bits(pml.ground_trace[3]),
    ];
    crate::project_velocity(&mut ps.velocity, &normal);
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

/// After the tick's ground trace: touching down ends the flight (prone,
/// where there is room; else crouched), and the pause's end ends the dive.
pub fn settle<C: CollisionBackend>(
    ps: &mut PlayerState,
    pml: &Pml,
    cmd: &UserCmd,
    collision: &C,
    bounds: MoveBounds,
) {
    if ps.pm_flags & PMF_DIVE != 0 {
        if pml.walking == 0 {
            return;
        }
        let prone = player_prone_allowed(ps, collision, bounds.maxs[0], false);
        ps.pm_flags &= !(PMF_DIVE | pm_flags::JUMPING);
        ps.pm_flags |= PMF_DIVE_SLIDE;
        if prone {
            ps.pm_flags = (ps.pm_flags & !pm_flags::CROUCH) | pm_flags::PRONE;
        }
        ps.jump_time = cmd.server_time;
        return;
    }
    if ps.pm_flags & PMF_DIVE_SLIDE != 0
        && cmd.server_time.wrapping_sub(ps.jump_time) >= SLIDE_MS + PAUSE_MS
    {
        ps.pm_flags &= !PMF_DIVE_SLIDE;
        if ps.pm_flags & pm_flags::PRONE != 0 {
            ps.pm_flags |= PMF_DIVE_PRONE;
        }
        ps.velocity[0] = 0.0;
        ps.velocity[1] = 0.0;
    }
}

/// After a dive the player holds prone until a stance key or jump is newly
/// pressed (or something else stood them up); `true` while it holds, so the
/// stance update is skipped.
pub fn holds_prone(ps: &mut PlayerState, cmd: &UserCmd, old_buttons: u32) -> bool {
    if ps.pm_flags & PMF_DIVE_PRONE == 0 {
        return false;
    }
    let stance_keys = buttons::PRONE | buttons::CROUCH | buttons::JUMP;
    let pressed = cmd.buttons & !old_buttons & stance_keys;
    if pressed != 0 || ps.pm_flags & pm_flags::PRONE == 0 {
        ps.pm_flags &= !PMF_DIVE_PRONE;
        // A prone press after a dive stands back up, as BO2's prone key does.
        if pressed & buttons::PRONE != 0 {
            ps.pm_flags &= !pm_flags::PRONE;
        }
        return false;
    }
    true
}
