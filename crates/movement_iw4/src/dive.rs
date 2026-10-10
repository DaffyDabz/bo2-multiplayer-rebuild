//! bo2zm: Black Ops II's dive to prone (the "dolphin dive"). Not part of
//! upstream IW4L.
//!
//! While sprinting on the ground, pressing prone (or holding a pad's stance
//! button a moment) throws the player out the way they are pushing, any direction, at the
//! speed they ran. The dive pops up to jump height, holds it a moment, then
//! drops; on touch-down the player slides on their front into prone and
//! holds still a moment. They stay prone until they press jump, crouch,
//! prone or sprint, and getting up from a dive is twice as quick as from an
//! ordinary prone. A get-up pressed during the dive is kept, so the player
//! rises the moment the dive ends.
//!
//! The timings are BO2's own dive dvar defaults: `dtp_startup_delay` 250 ms
//! of sprint before a dive, `dtp_min_speed` 3.16, `dtp_max_apex_duration`
//! 400 ms at the top, `dtp_max_slide_duration` 300 ms of slide,
//! `dtp_post_move_pause` 100 ms, and `dtp_exhaustion_window` 1.5 s from one
//! dive's slide ending to the next dive. The pop is twice a jump's launch
//! speed (`dtp_new_trajectory_multiplier` 2) and the hold is at a jump's
//! height (`jump_height`), so the time in the air matches BO2's own dive
//! animation (`pb_dive_prone`, 22 frames at 30 = 0.733 s from take-off to
//! touch-down; common_zm). As in BO2 the player can steer a little through
//! the flight and the slide: air control at 0.4 of the usual wish speed.
//! A slide that comes to a stop (into a wall) ends the dive at once, and over
//! a ledge top the player could climb (mantle ground) it keeps its speed to
//! the end.
//!
//! State rides in the replicated player state, so the host and the local
//! prediction agree: `PMF_DIVE` in the air, `PMF_DIVE_SLIDE` from touch-down
//! to the end of the pause, `PMF_DIVE_PRONE` while held prone after it,
//! `PMF_DIVE_GETUP` from a get-up press until the player is up;
//! `jump_time` = when the current phase began, `jump_origin_z` = the
//! take-off height.

use playerstate_iw4::{ENTITYNUM_NONE, PlayerState, UserCmd, buttons, pm_flags};

use crate::{
    CONTENTS_MANTLE, CollisionBackend, GroundTraceInput, MoveBounds, Pml, add_predictable_event,
    end_sprint, player_prone_allowed, step_slide_move, view_height,
};

/// In the air, diving.
pub const PMF_DIVE: u32 = 0x0100_0000;
/// On the ground after a dive: sliding, then the pause.
pub const PMF_DIVE_SLIDE: u32 = 0x0200_0000;
/// After a dive: the player stays prone (as BO2 leaves them) until they
/// press jump, crouch, prone or sprint, whatever the keys' hold or toggle
/// style.
pub const PMF_DIVE_PRONE: u32 = 0x0400_0000;
/// Getting up from a dive (or waiting to, once it ends): the stance change
/// runs at twice the speed, and a held prone key does not drop the player
/// back down until it is let go.
pub const PMF_DIVE_GETUP: u32 = 0x0800_0000;

/// A dive starts only after more than this long sprinting
/// (`dtp_startup_delay`).
const STARTUP_MS: i32 = 250;
/// A dive starts only above this ground speed (`dtp_min_speed`): any real
/// movement.
const MIN_SPEED: f32 = 3.16;
/// Another dive waits until more than this long after the last one's slide
/// ended (`dtp_exhaustion_window`).
const EXHAUSTION_MS: i32 = 1500;
/// The take-off's upward speed is this many times a jump's
/// (`dtp_new_trajectory_multiplier`), so the dive reaches a jump's height in
/// under a tenth of a second.
const POP_SCALE: f32 = 2.0;
/// How long after take-off the dive holds a jump's height before it drops
/// (`dtp_max_apex_duration`). With jump height 39 and gravity 800 the drop
/// takes 0.31 s, so the player is in the air about 0.71 s, against the
/// animation's 0.733 s.
const GLIDE_MS: i32 = 400;
/// Within this of the dive's height counts as there, so the hold keeps it.
/// A dive stopped short by a ceiling does not hold: it drops at once.
const AT_TOP: f32 = 1.0;
/// The slide on the ground after touch-down. Friction comes in gradually
/// over it; at its end the player stops.
const SLIDE_MS: i32 = 300;
const SLIDE_FRICTION: f32 = 5.5;
const SLIDE_STOP_SPEED: f32 = 100.0;
/// The still pause after the slide.
const PAUSE_MS: i32 = 100;

/// A dive is under way (in the air, sliding or pausing).
pub fn active(ps: &PlayerState) -> bool {
    ps.pm_flags & (PMF_DIVE | PMF_DIVE_SLIDE) != 0
}

/// BO2's `divetoprone` for scripts: from take-off until the slide ends
/// (not the pause after it). PhD Flopper's blast and the dive notifies
/// read this.
pub fn dive_to_prone(ps: &PlayerState) -> bool {
    ps.pm_flags & PMF_DIVE != 0
        || (ps.pm_flags & PMF_DIVE_SLIDE != 0
            && ps.command_time.wrapping_sub(ps.jump_time) < SLIDE_MS)
}

/// Any part of a dive, up to the player being back on their feet.
pub fn any(ps: &PlayerState) -> bool {
    ps.pm_flags & (PMF_DIVE | PMF_DIVE_SLIDE | PMF_DIVE_PRONE | PMF_DIVE_GETUP) != 0
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
    // Prone newly pressed. A pad's stance button sends prone once held
    // `cl_dtpHoldTime` while sprinting (input_iw4); a crouch only crouches.
    if (cmd.buttons & !old_buttons) & buttons::PRONE == 0 {
        return false;
    }
    if cmd.server_time.wrapping_sub(ps.last_sprint_start) <= STARTUP_MS {
        return false;
    }
    if ps.dive_end_time != 0
        && (0..=EXHAUSTION_MS).contains(&cmd.server_time.wrapping_sub(ps.dive_end_time))
    {
        return false;
    }
    ground_speed(ps) > MIN_SPEED
}

fn ground_speed(ps: &PlayerState) -> f32 {
    libm::sqrtf(ps.velocity[0] * ps.velocity[0] + ps.velocity[1] * ps.velocity[1])
}

fn flat_unit(v: [f32; 3]) -> [f32; 2] {
    let len = libm::sqrtf(v[0] * v[0] + v[1] * v[1]);
    if len < 1e-6 {
        [0.0, 0.0]
    } else {
        [v[0] / len, v[1] / len]
    }
}

/// The way the dive goes: where the player pushes the stick or keys; else
/// along their run; else where they look.
fn direction(ps: &PlayerState, pml: &Pml, cmd: &UserCmd, speed: f32) -> [f32; 2] {
    let forward = flat_unit(pml.forward);
    let right = flat_unit(pml.right);
    let f = f32::from(cmd.forwardmove);
    let r = f32::from(cmd.rightmove);
    let wish = [
        forward[0] * f + right[0] * r,
        forward[1] * f + right[1] * r,
        0.0,
    ];
    let dir = flat_unit(wish);
    if dir != [0.0, 0.0] {
        return dir;
    }
    if speed > 1.0 {
        return [ps.velocity[0] / speed, ps.velocity[1] / speed];
    }
    if forward != [0.0, 0.0] {
        forward
    } else {
        [1.0, 0.0]
    }
}

/// The take-off's upward speed: `POP_SCALE` times a jump's.
fn pop_speed(ps: &PlayerState, jump_height: f32) -> f32 {
    POP_SCALE * libm::sqrtf(2.0 * jump_height * ps.gravity as f32)
}

/// Launch the dive: sprint ends, the player crouches (a smaller hull for
/// the flight) and leaves the ground at their running speed.
pub fn start(ps: &mut PlayerState, pml: &mut Pml, cmd: &UserCmd, jump_height: f32) {
    end_sprint(ps, cmd);
    let speed = ground_speed(ps);
    let dir = direction(ps, pml, cmd, speed);
    ps.velocity = [dir[0] * speed, dir[1] * speed, pop_speed(ps, jump_height)];
    ps.pm_flags = (ps.pm_flags
        & !(pm_flags::PRONE | pm_flags::JUMPING | PMF_DIVE_PRONE | PMF_DIVE_GETUP))
        | pm_flags::CROUCH
        | PMF_DIVE;
    ps.ground_entity_num = ENTITYNUM_NONE;
    ps.jump_time = cmd.server_time;
    ps.jump_origin_z = ps.origin[2];
    pml.walking = 0;
    pml.ground_plane = 0;
    pml.almost_ground_plane = 0;
}

/// A key that gets the player up from a dive: jump, crouch, prone or sprint
/// newly pressed, or a stance toggle switched off (a pad, or toggle-style
/// keys).
fn getup_pressed(cmd: &UserCmd, old_buttons: u32) -> bool {
    let new = cmd.buttons & !old_buttons;
    if new & (buttons::JUMP | buttons::CROUCH | buttons::PRONE | buttons::SPRINT) != 0 {
        return true;
    }
    old_buttons & buttons::STANCE_HELD == 0
        && old_buttons & !cmd.buttons & (buttons::PRONE | buttons::CROUCH) != 0
}

/// While the dive is under way, a get-up press is kept for when it ends.
pub fn queue_getup(ps: &mut PlayerState, cmd: &UserCmd, old_buttons: u32) {
    if getup_pressed(cmd, old_buttons) {
        ps.pm_flags |= PMF_DIVE_GETUP;
    }
}

/// Getting up: a prone toggle still on would drop the player straight back
/// down, so the client is told to let it go (as when a sprint stands a
/// prone player up).
fn begin_getup(ps: &mut PlayerState, cmd: &UserCmd) {
    ps.pm_flags = (ps.pm_flags & !PMF_DIVE_PRONE) | PMF_DIVE_GETUP;
    if cmd.buttons & buttons::PRONE != 0 && cmd.buttons & buttons::STANCE_HELD == 0 {
        add_predictable_event(ps, EV_STANCE_FORCE_STAND, 0);
    }
}

const EV_STANCE_FORCE_STAND: i32 = 6;

/// Move one tick of a dive. In the air: up to a jump's height, level until
/// `GLIDE_MS`, then down under gravity. On the ground: the slide, friction
/// coming in over `SLIDE_MS`, then still until the pause ends. The player's
/// own push steers a little through the flight and the slide, not the
/// pause.
#[allow(clippy::too_many_arguments)]
pub fn advance<C: CollisionBackend>(
    ps: &mut PlayerState,
    pml: &mut Pml,
    cmd: &UserCmd,
    bounds: MoveBounds,
    collision: &C,
    jump_height: f32,
    spectate_speed_scale: f32,
) {
    let mut elapsed = cmd.server_time.wrapping_sub(ps.jump_time);
    let gravity = ps.gravity as f32;
    let dt = pml.frametime.max(0.001);
    if ps.pm_flags & PMF_DIVE != 0 {
        crate::air::steer(ps, pml, cmd, spectate_speed_scale);
        let top = ps.jump_origin_z + jump_height;
        if elapsed < GLIDE_MS {
            let next_vz = ps.velocity[2] - gravity * dt;
            let rise = (ps.velocity[2] + next_vz) * 0.5 * dt;
            if ps.origin[2] >= top - AT_TOP || ps.origin[2] + rise >= top {
                let pop = pop_speed(ps, jump_height);
                ps.velocity[2] = ((top - ps.origin[2]) / dt).clamp(-pop, pop);
                slide(ps, pml, collision, bounds, None);
                return;
            }
        }
        slide(ps, pml, collision, bounds, Some(gravity));
        return;
    }
    if elapsed < SLIDE_MS && speed_3d(ps) < MIN_SPEED {
        // BO2: a slide that has stopped (into a wall) ends the dive now; the
        // pause starts from here.
        ps.jump_time = cmd.server_time.wrapping_sub(SLIDE_MS);
        ps.velocity = [0.0; 3];
        elapsed = SLIDE_MS;
    }
    let pausing = elapsed >= SLIDE_MS;
    if pml.walking == 0 {
        // Slid off an edge: fall.
        if !pausing {
            crate::air::steer(ps, pml, cmd, spectate_speed_scale);
        }
        slide(ps, pml, collision, bounds, Some(gravity));
        return;
    }
    let speed = ground_speed(ps);
    let next = if pausing {
        0.0
    } else {
        let ramp = if on_mantle_ground(ps, collision, bounds) {
            0.0
        } else {
            (elapsed.max(0) as f32 / SLIDE_MS as f32).min(1.0)
        };
        let control = speed.max(SLIDE_STOP_SPEED);
        (speed - control * SLIDE_FRICTION * ramp * dt).max(0.0)
    };
    if speed > 1e-3 {
        ps.velocity[0] *= next / speed;
        ps.velocity[1] *= next / speed;
    }
    if !pausing {
        crate::air::steer(ps, pml, cmd, spectate_speed_scale);
    }
    if (pml.ground_trace[4] & 2) != 0 {
        ps.velocity[2] -= gravity * pml.frametime;
    }
    let normal = [
        f32::from_bits(pml.ground_trace[1]),
        f32::from_bits(pml.ground_trace[2]),
        f32::from_bits(pml.ground_trace[3]),
    ];
    crate::project_velocity(&mut ps.velocity, &normal);
    if ps.velocity[0] != 0.0 || ps.velocity[1] != 0.0 {
        slide(ps, pml, collision, bounds, None);
    }
}

fn speed_3d(ps: &PlayerState) -> f32 {
    let v = ps.velocity;
    libm::sqrtf(v[0] * v[0] + v[1] * v[1] + v[2] * v[2])
}

/// BO2: over ground the player could mantle (a ledge top), the slide has no
/// friction until it ends.
fn on_mantle_ground<C: CollisionBackend>(
    ps: &PlayerState,
    collision: &C,
    bounds: MoveBounds,
) -> bool {
    let o = ps.origin;
    collision
        .trace(GroundTraceInput {
            start: [o[0], o[1], o[2] + 1.0],
            end: [o[0], o[1], o[2] - 1.0],
            mins: bounds.mins,
            maxs: bounds.maxs,
            tracemask: CONTENTS_MANTLE,
        })
        .fraction
        < 1.0
}

fn slide<C: CollisionBackend>(
    ps: &mut PlayerState,
    pml: &Pml,
    collision: &C,
    bounds: MoveBounds,
    gravity: Option<f32>,
) {
    step_slide_move(
        ps,
        pml,
        collision,
        bounds.mins,
        bounds.maxs,
        bounds.tracemask,
        gravity,
    );
}

/// After the tick's ground trace: touching down ends the flight (prone,
/// where there is room; else crouched), and the pause's end ends the dive:
/// held prone, or getting up if a get-up was pressed during it.
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
        ps.dive_end_time = ps.jump_time.wrapping_add(SLIDE_MS);
        ps.velocity[0] = 0.0;
        ps.velocity[1] = 0.0;
        if ps.pm_flags & PMF_DIVE_GETUP != 0 {
            begin_getup(ps, cmd);
        } else if ps.pm_flags & pm_flags::PRONE != 0 {
            ps.pm_flags |= PMF_DIVE_PRONE;
        }
    }
}

/// After a dive the player holds prone until a get-up key (or something
/// else stood them up); `true` while it holds, so the stance update is
/// skipped.
pub fn holds_prone(ps: &mut PlayerState, cmd: &UserCmd, old_buttons: u32) -> bool {
    if ps.pm_flags & PMF_DIVE_PRONE == 0 {
        return false;
    }
    if getup_pressed(cmd, old_buttons) || ps.pm_flags & pm_flags::SPRINTING != 0 {
        begin_getup(ps, cmd);
        return false;
    }
    if ps.pm_flags & pm_flags::PRONE == 0 {
        ps.pm_flags &= !PMF_DIVE_PRONE;
        return false;
    }
    true
}

/// The command the stance and sprint updates see: while getting up from a
/// dive, a held prone key is left out, so it does not drop the player back
/// down (a hold-style key pressed to dive, or a prone toggle the client has
/// not yet let go).
pub fn stance_cmd(ps: &PlayerState, cmd: &UserCmd) -> UserCmd {
    let mut out = *cmd;
    if ps.pm_flags & PMF_DIVE_GETUP != 0 {
        out.buttons &= !buttons::PRONE;
    }
    out
}

/// The command the aim update sees: the sprint key, held through a dive,
/// does not drop the aim, and the player's own push (it only steers a dive)
/// does not count as crawling.
pub fn ads_cmd(ps: &PlayerState, cmd: &UserCmd, diving: bool) -> UserCmd {
    let mut out = *cmd;
    if diving || ps.pm_flags & PMF_DIVE_PRONE != 0 {
        out.buttons &= !buttons::SPRINT;
    }
    if diving {
        out.forwardmove = 0;
        out.rightmove = 0;
    }
    out
}

/// The get-up is over once the view has come to rest out of prone (both
/// steps, lying to crouched and crouched to standing, are done) and the
/// prone key is let go. Run before the view update, so a step that has just
/// ended and the next one not yet begun does not end it early.
pub fn finish_getup(ps: &mut PlayerState, cmd: &UserCmd) {
    if ps.pm_flags & PMF_DIVE_GETUP == 0 || active(ps) || ps.pm_flags & PMF_DIVE_PRONE != 0 {
        return;
    }
    if ps.view_height_lerp_time == 0
        && ps.view_height_target != view_height::PRONE
        && ps.view_height_current == ps.view_height_target as f32
        && cmd.buttons & buttons::PRONE == 0
    {
        ps.pm_flags &= !PMF_DIVE_GETUP;
    }
}
