//! bo2zm: Black Ops II movement rules that differ from the IW4 code this
//! crate started from. `on == false` keeps the IW4 behaviour exactly, so the
//! two can be compared side by side.

use playerstate_iw4::{PlayerState, UserCmd, pm_flags};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bo2Feel {
    pub on: bool,

    /// player_sprintStrafeSpeedScale: sideways input is scaled by this while
    /// sprinting.
    pub sprint_strafe_speed_scale: f32,

    /// jump_slowdownEnable: landing a jump slows you down (BO2: on in
    /// multiplayer, off in Zombies).
    pub jump_slowdown: bool,

    /// Falls higher than this hurt (bg_fallDamageMinHeight, 128 in both modes).
    pub fall_damage_min_height: f32,

    /// Falls this high or higher do full damage (bg_fallDamageMaxHeight, 300
    /// in both modes).
    pub fall_damage_max_height: f32,

    /// Zombies: no fall damage while this player's gravity is low, and PhD
    /// Flopper turns any fall damage into 1.
    pub zombies: bool,

    /// bg_gravity, for the Zombies low-gravity rule.
    pub gravity: i32,

    /// The fall perk's bit in perks[0] (specialty_fallheight): no fall
    /// damage, in both modes. 0 = none.
    pub fall_damage_perk: u32,

    /// PhD Flopper's bit in perks[0] (specialty_flakjacket): in Zombies any
    /// fall damage becomes 1, so the scripts can set off its blast. 0 = none.
    pub flak_jacket_perk: u32,

    /// Quickdraw's bit in perks[0] (specialty_fastads). 0 = none.
    pub fast_ads_perk: u32,

    /// perk_weapAdsMultiplier: with Quickdraw, aiming in and out takes this
    /// share of the gun's own time (0.5 until read from real BO2); sniper rifles
    /// are left alone.
    pub fast_ads_multiplier: f32,

    /// Stamin-Up's bit in perks[0] (specialty_longersprint). 0 = none.
    pub longer_sprint_perk: u32,

    /// perk_sprintMultiplier: Stamin-Up multiplies the sprint time by this
    /// (BO2 default 2, still capped at 16383 ms).
    pub sprint_multiplier: f32,

    /// Move-faster's bit in perks[0] (specialty_movefaster). 0 = none.
    pub move_faster_perk: u32,

    /// perk_speedMultiplier: move-faster multiplies the walk speed by this
    /// (BO2 default 1.07: 190 -> 203).
    pub speed_multiplier: f32,

    /// g_speed: the walk speed every frame starts from (190).
    pub g_speed: f32,

    /// dtp: dive to prone is allowed (BO2 default 1, in both modes).
    pub dive: bool,

    /// The gun in hand's footstep rhythm scale while sprinting
    /// (fSprintCycleScale), while sprinting crouched
    /// (fDuckedSprintCycleScale) and through a dive (fDtpCycleScale).
    pub sprint_cycle_scale: f32,
    pub ducked_sprint_cycle_scale: f32,
    pub dtp_cycle_scale: f32,

    /// Sprint in any direction at full speed, and dive whichever way you
    /// are moving (an owner's ask, 10-09; BO2 itself sprints forward only).
    /// `BO2_OMNI=off` turns it off.
    pub omni: bool,

    /// mantle_weapon_height: a climb this low (by its climb table height)
    /// keeps the gun in hand; mantle_weapon_anim_height: one higher than
    /// this also plays the gun's climb camera animation. 0 lowers the gun
    /// for every climb.
    pub mantle_weapon_height: f32,
    pub mantle_weapon_anim_height: f32,
}

impl Bo2Feel {
    pub const IW4: Self = Self {
        on: false,
        sprint_strafe_speed_scale: 1.0,
        jump_slowdown: true,
        fall_damage_min_height: 128.0,
        fall_damage_max_height: 300.0,
        zombies: false,
        gravity: 800,
        fall_damage_perk: 0,
        flak_jacket_perk: 0,
        fast_ads_perk: 0,
        fast_ads_multiplier: 1.0,
        longer_sprint_perk: 0,
        sprint_multiplier: 1.0,
        move_faster_perk: 0,
        speed_multiplier: 1.0,
        g_speed: 190.0,
        dive: true,
        sprint_cycle_scale: 1.0,
        ducked_sprint_cycle_scale: 1.0,
        dtp_cycle_scale: 1.0,
        omni: false,
        mantle_weapon_height: 0.0,
        mantle_weapon_anim_height: 0.0,
    };
}

/// Sprint in any direction: the sprint rules see how far the stick (or the
/// keys) push in any direction as a push forward.
#[must_use]
pub fn omni_sprint_cmd(cmd: &UserCmd, feel: Bo2Feel) -> UserCmd {
    let mut out = *cmd;
    if feel.on && feel.omni {
        let (f, r) = (f32::from(cmd.forwardmove), f32::from(cmd.rightmove));
        out.forwardmove = libm::sqrtf(f * f + r * r).min(127.0) as i8;
    }
    out
}

/// Sprint in any direction at full speed: while sprinting, moving back or
/// sideways is not slowed.
#[must_use]
pub fn omni_cmd_scale(
    ps: &PlayerState,
    scale: crate::CmdScaleWalkContext,
    feel: Bo2Feel,
) -> crate::CmdScaleWalkContext {
    if feel.on && feel.omni && ps.pm_flags & pm_flags::SPRINTING != 0 {
        crate::CmdScaleWalkContext {
            player_back_speed_scale: 1.0,
            player_strafe_speed_scale: 1.0,
            ..scale
        }
    } else {
        scale
    }
}

/// BO2 feel M14: a shellshock slows walking by the shock file's own number
/// (bg_shock_movement: explosion and pain 1, so not slowed; flashbang 0.8;
/// most others 0.4). The old rule slowed every shock with a non-zero
/// number to 0.4. `movement` is the file's switch; a host that puts the
/// number on the move-speed multiplier itself (bo2mp's `shellshock`)
/// turns it off, so the shock never slows twice.
#[must_use]
pub fn shellshock_walk_scale(movement: bool, movement_scale: f32, feel: Bo2Feel) -> f32 {
    if !movement {
        1.0
    } else if !feel.on {
        0.4
    } else if movement_scale > 0.0 {
        movement_scale
    } else {
        1.0
    }
}

/// Fall perk (specialty_fallheight).
pub const PERK_FALLHEIGHT: u32 = 1 << 29;

/// Flak Jacket, Zombies' PhD Flopper (specialty_flakjacket).
pub const PERK_FLAKJACKET: u32 = 1 << 31;

/// Quickdraw (specialty_fastads).
pub const PERK_FASTADS: u32 = 1 << 30;

/// Stamin-Up, multiplayer's Extreme Conditioning (specialty_longersprint).
pub const PERK_LONGERSPRINT: u32 = 1 << 26;

/// BO2 feel M10b: the crouch-to-prone stage of the eye-height change takes
/// 600 ms (MW2 400), and the stance speed blends over the same time. The
/// curves themselves are the same.
pub const PRONE_LERP_MS: i32 = 600;

/// Move-faster (specialty_movefaster): multiplayer's Lightweight speed and
/// the Turned zombies' perk.
pub const PERK_MOVEFASTER: u32 = 1 << 9;

/// BO2 feel M8b: every frame the walk speed starts again from g_speed; with
/// move-faster it is multiplied by perk_speedMultiplier and cut to a whole
/// number (190 x 1.07 = 203). `None` = MW2 rules, the speed is left alone.
#[must_use]
pub fn player_speed(ps: &PlayerState, feel: Bo2Feel) -> Option<i32> {
    if !feel.on {
        return None;
    }
    let mut speed = feel.g_speed;
    if feel.move_faster_perk != 0 && ps.perks[0] & feel.move_faster_perk != 0 {
        speed *= feel.speed_multiplier;
    }
    Some(speed as i32)
}

/// In Zombies, Stamin-Up raises the gun's move speed to at least this while
/// sprinting (BO2's own number).
pub const STAMIN_UP_SPRINT_MOVE_SPEED_SCALE: f32 = 1.1;

fn stamin_up(ps: &PlayerState, feel: Bo2Feel) -> bool {
    feel.on && feel.longer_sprint_perk != 0 && (ps.perks[0] & feel.longer_sprint_perk) != 0
}

/// BO2 feel M13: Stamin-Up multiplies the sprint time, capped at 16383 ms.
#[must_use]
pub fn stamin_up_sprint_time(ps: &PlayerState, max_ms: i32, feel: Bo2Feel) -> i32 {
    if !stamin_up(ps, feel) {
        return max_ms;
    }
    ((max_ms as f32 * feel.sprint_multiplier) as i32).min(0x3fff)
}

/// BO2 feel M13: in Zombies, Stamin-Up sprints with the gun's move speed at
/// least 1.1 (most guns are 1, so 10% faster). A gun with no move speed of
/// its own keeps its aiming speed.
#[must_use]
pub fn stamin_up_cmd_scale(
    ps: &PlayerState,
    scale: crate::CmdScaleWalkContext,
    feel: Bo2Feel,
) -> crate::CmdScaleWalkContext {
    if feel.zombies
        && stamin_up(ps, feel)
        && ps.pm_flags & pm_flags::SPRINTING != 0
        && scale.weapon_move_speed_scale > 0.0
    {
        crate::CmdScaleWalkContext {
            weapon_move_speed_scale: scale
                .weapon_move_speed_scale
                .max(STAMIN_UP_SPRINT_MOVE_SPEED_SCALE),
            ..scale
        }
    } else {
        scale
    }
}

/// BO2 feel G3h: Quickdraw speeds up aiming in and out, except on sniper
/// rifles. Returns the aim-in and aim-out rates (per ms).
#[must_use]
pub fn quickdraw_ads_rates(
    ps: &PlayerState,
    ads_in_rate: f32,
    ads_out_rate: f32,
    sniper: bool,
    feel: Bo2Feel,
) -> (f32, f32) {
    if !feel.on
        || sniper
        || feel.fast_ads_perk == 0
        || (ps.perks[0] & feel.fast_ads_perk) == 0
        || feel.fast_ads_multiplier <= 0.0
    {
        return (ads_in_rate, ads_out_rate);
    }
    (
        ads_in_rate / feel.fast_ads_multiplier,
        ads_out_rate / feel.fast_ads_multiplier,
    )
}

impl Default for Bo2Feel {
    fn default() -> Self {
        Self::IW4
    }
}

/// Extra time added to the sprint end when a sprint is ended by jumping.
pub const SPRINT_END_JUMP_PENALTY_MS: i32 = 800;

const BUTTON_JUMP: u32 = 0x400;

/// Jumping ends a sprint and moves the sprint end 800 ms later, so the next
/// sprint is shorter. Runs right after the sprint update; `before` is the
/// sprinting flag and sprint delay from just before it. A sprint that ran
/// out of time (it sets the sprint delay) does not count.
pub(crate) fn jump_out_of_sprint(
    ps: &mut PlayerState,
    cmd: &UserCmd,
    feel: Bo2Feel,
    before: (u32, i32),
    change: crate::SprintResult,
) {
    if feel.on
        && change == crate::SprintResult::Ended
        && before.0 != 0
        && ps.sprint_delay == before.1
        && (cmd.buttons & BUTTON_JUMP) != 0
    {
        ps.last_sprint_end = ps.last_sprint_end.wrapping_add(SPRINT_END_JUMP_PENALTY_MS);
    }
}

/// A landing counts as "landed higher" only this far above the take-off.
pub const JUMP_LAND_RAISE: f32 = 18.0;

/// Ground friction scale during the hard-landing stun.
pub const HARD_LANDING_FRICTION_SCALE: f32 = 0.3;

/// Walking acceleration on slick ground (the old rule had 1).
pub const SLICK_ACCEL: f32 = 2.0;

/// player_sliding_friction: slick ground still slows you, this much times
/// your speed each second.
pub const SLIDING_FRICTION: f32 = 1.5;

/// Head bob and footstep rhythm per stance: stand, prone, crouch, then the
/// same three running backwards. Columns: running, walking.
const BOB_FACTOR_TABLE: [[f32; 2]; 6] = [
    [0.335, 0.28],
    [0.25, 0.24],
    [0.34, 0.29],
    [0.36, 0.3],
    [0.25, 0.24],
    [0.34, 0.29],
];

/// player_moveThreshhold: slower than this counts as standing still.
pub const PLAYER_MOVE_THRESHHOLD: f32 = 2.0;

/// player_runbkThreshhold: standing up and slower than this walks.
const PLAYER_RUNBK_THRESHHOLD: f32 = 60.0;

/// player_sprintThreshhold: standing up, sprinting only counts this fast.
const PLAYER_SPRINT_THRESHHOLD: f32 = 185.0;

/// player_sprintCameraBob.
const PLAYER_SPRINT_CAMERA_BOB: f32 = 0.5;

/// Ladder rhythm: climbing speed against 0.75 x 127, walking 0.4 of that.
const LADDER_MAX_SPEED: f32 = 0.5 * 1.5 * 127.0;
const LADDER_BOB: f32 = 0.45;
const LADDER_WALK_BOB: f32 = 0.35;

/// Moves the head bob and footstep rhythm one frame on the ground. The caller
/// has already ruled out standing still, no input, the air and turrets.
pub(crate) fn bob_cycle(
    ps: &mut PlayerState,
    msec: i32,
    forwardmove: i8,
    rightmove: i8,
    xyspeed: f32,
    server_time: i32,
    scales: crate::CmdScaleWalkContext,
    feel: Bo2Feel,
) {
    let scales = stamin_up_cmd_scale(ps, scales, feel);
    // Downed (22) uses the prone row.
    let stance = match ps.view_height_target {
        22 | 11 => 1,
        40 => 2,
        _ => 0,
    };
    let backward = (ps.pm_flags & pm_flags::BACKWARDS_RUN) != 0;
    let row = stance + 3 * usize::from(backward);
    let mut walking = (ps.pm_flags & pm_flags::WALKING) != 0 || ps.leanf != 0.0;
    let mut sprinting = (ps.pm_flags & pm_flags::SPRINTING) != 0;
    if stance == 0 {
        walking |= xyspeed <= PLAYER_RUNBK_THRESHHOLD;
        sprinting &= xyspeed >= PLAYER_SPRINT_THRESHHOLD;
    }
    let max_speed = crate::get_bob_max_speed(
        ps,
        forwardmove,
        rightmove,
        walking,
        sprinting,
        server_time,
        scales,
    ) * ps.move_speed_scale_multiplier;
    if max_speed <= 0.0 {
        return;
    }
    let factor = if row == 0 && sprinting {
        PLAYER_SPRINT_CAMERA_BOB
    } else {
        BOB_FACTOR_TABLE[row][usize::from(walking)]
    };
    let bobmove = xyspeed / max_speed * factor;
    // The gun in hand speeds up or slows the rhythm while the sprint flag
    // is on (crouched: its own scale), whatever the speed.
    let sprint_flag = (ps.pm_flags & pm_flags::SPRINTING) != 0;
    let scale = if sprint_flag && ps.view_height_target == 40 {
        feel.ducked_sprint_cycle_scale
    } else if sprint_flag {
        feel.sprint_cycle_scale
    } else {
        1.0
    };
    ps.bob_cycle = step_cycle(ps.bob_cycle, msec, bobmove, scale);
}

/// Rhythm speed through a dive (dtpBobMove).
const DTP_BOB_MOVE: f32 = 0.24;

/// Through a whole dive, air and slide, the rhythm runs at its own pace
/// (times the gun's dive scale) and the normal step rhythm is skipped.
/// Returns true while diving.
pub(crate) fn dive_bob_cycle(ps: &mut PlayerState, msec: i32, feel: Bo2Feel) -> bool {
    if (ps.pm_flags & (crate::dive::PMF_DIVE | crate::dive::PMF_DIVE_SLIDE)) == 0 {
        return false;
    }
    ps.bob_cycle = step_cycle(ps.bob_cycle, msec, DTP_BOB_MOVE, feel.dtp_cycle_scale);
    true
}

/// Ladder rhythm: follows the climbing speed (down runs it backwards).
pub(crate) fn ladder_bob_cycle(ps: &mut PlayerState, msec: i32) {
    let bobmove = if (ps.pm_flags & pm_flags::WALKING) == 0 && ps.leanf == 0.0 {
        ps.velocity[2] / LADDER_MAX_SPEED * LADDER_BOB
    } else {
        ps.velocity[2] / (LADDER_MAX_SPEED * 0.4) * LADDER_WALK_BOB
    };
    ps.bob_cycle = step_cycle(ps.bob_cycle, msec, bobmove, 1.0);
}

/// The rhythm counter steps by whole units, rounding down.
fn step_cycle(old: i32, msec: i32, bobmove: f32, scale: f32) -> i32 {
    let old = old as u8;
    i32::from((f32::from(old) + msec as f32 * bobmove * scale) as i32 as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Pml, SprintContext, friction, update_sprint};

    const BO2: Bo2Feel = Bo2Feel {
        on: true,
        sprint_strafe_speed_scale: 0.667,
        jump_slowdown: true,
        ..Bo2Feel::IW4
    };

    fn walking_pml() -> Pml {
        Pml {
            forward: [1.0, 0.0, 0.0],
            right: [0.0, -1.0, 0.0],
            up: [0.0, 0.0, 1.0],
            frametime: 0.05,
            msec: 50,
            walking: 1,
            ground_plane: 1,
            almost_ground_plane: 1,
            ground_trace: [0; 11],
            previous_origin: [0.0; 3],
            previous_velocity: [0.0; 3],
            holdrand: 0,
        }
    }

    #[test]
    fn hard_landing_slides_in_bo2_and_stops_in_iw4() {
        let pml = walking_pml();
        let mut ps = PlayerState::ZERO;
        ps.pm_flags = 0x80;
        ps.velocity = [200.0, 0.0, 0.0];
        let mut iw4 = ps;
        friction(&mut ps, &pml, true);
        friction(&mut iw4, &pml, false);
        // control 200 * 0.3 * 5.5 * 0.05 = 16.5 lost; IW4 loses 110.
        assert!((ps.velocity[0] - 183.5).abs() < 1e-3, "{}", ps.velocity[0]);
        assert!((iw4.velocity[0] - 90.0).abs() < 1e-3, "{}", iw4.velocity[0]);
    }

    #[test]
    fn walking_friction_ignores_vertical_speed_in_bo2() {
        let pml = walking_pml();
        let mut ps = PlayerState::ZERO;
        ps.velocity = [150.0, 0.0, -80.0];
        friction(&mut ps, &pml, true);
        // speed 150 on the ground: 150 * 5.5 * 0.05 = 41.25 lost.
        assert!((ps.velocity[0] - 108.75).abs() < 1e-3, "{}", ps.velocity[0]);
    }

    fn scales() -> crate::CmdScaleWalkContext {
        crate::CmdScaleWalkContext {
            player_back_speed_scale: 0.7,
            player_strafe_speed_scale: 0.8,
            player_sprint_speed_scale: 1.5,
            player_last_stand_crawl_speed_scale: 0.15,
            weapon_move_speed_scale: 1.0,
            weapon_ads_move_speed_scale: 1.0,
            shellshock_movement_scale: 1.0,
            prone_lerp_ms: 0,
        }
    }

    /// One 50 ms step of the head bob rhythm from 0, standing on the ground.
    fn bob_step(feel: Bo2Feel, speed: f32, forward: i8, flags: u32) -> i32 {
        let mut ps = PlayerState::ZERO;
        ps.speed = 190;
        ps.move_speed_scale_multiplier = 1.0;
        ps.view_height_target = 60;
        ps.ground_entity_num = 1022;
        ps.pm_flags = flags;
        ps.velocity = [speed, 0.0, 0.0];
        crate::footsteps_bob_cycle(&mut ps, 50, forward, 0, true, 1000, scales(), feel);
        ps.bob_cycle
    }

    #[test]
    fn running_rhythm_rounds_down_in_bo2() {
        // 190 / 190 * 0.335 * 50 = 16.75.
        assert_eq!(bob_step(BO2, 190.0, 127, 0), 16);
        assert_eq!(bob_step(Bo2Feel::IW4, 190.0, 127, 0), 17);
    }

    #[test]
    fn slow_running_bobs_like_walking_in_bo2() {
        // Under 60: walking column 0.28 against 190 x 0.4.
        // BO2 50 / 76 * 0.28 * 50 = 9.2; IW4 50 / 190 * 0.335 * 50 = 4.4.
        assert_eq!(bob_step(BO2, 50.0, 127, 0), 9);
        assert_eq!(bob_step(Bo2Feel::IW4, 50.0, 127, 0), 4);
    }

    #[test]
    fn running_backwards_uses_its_own_row_in_bo2() {
        // 133 / (190 x 0.7) * 0.36 * 50 = 18; IW4 uses the forward 0.335.
        let back = pm_flags::BACKWARDS_RUN;
        assert_eq!(bob_step(BO2, 133.0, -127, back), 18);
        assert_eq!(bob_step(Bo2Feel::IW4, 133.0, -127, back), 17);
    }

    #[test]
    fn script_move_speed_counts_in_bo2() {
        let mut ps = PlayerState::ZERO;
        ps.speed = 190;
        ps.move_speed_scale_multiplier = 0.5;
        ps.view_height_target = 60;
        ps.ground_entity_num = 1022;
        ps.velocity = [95.0, 0.0, 0.0];
        crate::footsteps_bob_cycle(&mut ps, 50, 127, 0, true, 1000, scales(), BO2);
        // 95 / (190 x 0.5) * 0.335 * 50 = 16.75.
        assert_eq!(ps.bob_cycle, 16);
    }

    #[test]
    fn ladder_rhythm_follows_climb_direction_in_bo2() {
        let mut ps = PlayerState::ZERO;
        ps.pm_flags = pm_flags::LADDER;
        ps.ground_entity_num = 0x7FF;
        ps.jump_time = -10_000;
        ps.bob_cycle = 100;
        ps.velocity = [0.0, 0.0, -95.25];
        let mut iw4 = ps;
        crate::ladder_footsteps(&mut ps, 50, 1000, BO2);
        crate::ladder_footsteps(&mut iw4, 50, 1000, Bo2Feel::IW4);
        // Down at full speed: 0.45 x 50 = 22.5 back, rounded down from 77.5.
        assert_eq!(ps.bob_cycle, 77);
        assert_eq!(iw4.bob_cycle, 123);
    }

    fn sprint_context() -> SprintContext {
        SprintContext {
            weapon_max_sprint_time: 4000,
            sprint_forever: false,
            min_sprint_time_seconds: 1.0,
            sprint_delay_seconds: 0.0,
            sprint_forward_minimum: 105,
            stand_up_clear: true,
            sprint_recharge_pause_seconds: 0.0,
        }
    }

    #[test]
    fn jumping_out_of_a_sprint_pushes_the_sprint_end_800_ms() {
        for (feel, expected_end) in [(BO2, 2800), (Bo2Feel::IW4, 2000)] {
            let mut ps = PlayerState::ZERO;
            ps.pm_flags = playerstate_iw4::pm_flags::SPRINTING;
            ps.last_sprint_start = 1000;
            ps.sprint_start_max_length = 4000;
            let cmd = UserCmd {
                server_time: 2000,
                forwardmove: 127,
                buttons: 0x2 | 0x400,
                ..UserCmd::default()
            };
            let before = (
                ps.pm_flags & playerstate_iw4::pm_flags::SPRINTING,
                ps.sprint_delay,
            );
            let change = update_sprint(&mut ps, &cmd, 0x2, sprint_context());
            jump_out_of_sprint(&mut ps, &cmd, feel, before, change);
            assert_eq!(ps.pm_flags & playerstate_iw4::pm_flags::SPRINTING, 0);
            assert_eq!(ps.last_sprint_end, expected_end);
        }
    }

    #[test]
    fn quickdraw_halves_aim_time_except_on_snipers() {
        let feel = Bo2Feel {
            fast_ads_perk: PERK_FASTADS,
            fast_ads_multiplier: 0.5,
            ..BO2
        };
        let mut ps = PlayerState::ZERO;
        let rates = (1.0 / 250.0, 1.0 / 200.0);
        assert_eq!(
            quickdraw_ads_rates(&ps, rates.0, rates.1, false, feel),
            rates
        );
        ps.perks[0] |= PERK_FASTADS;
        assert_eq!(
            quickdraw_ads_rates(&ps, rates.0, rates.1, false, feel),
            (1.0 / 125.0, 1.0 / 100.0)
        );
        assert_eq!(
            quickdraw_ads_rates(&ps, rates.0, rates.1, true, feel),
            rates
        );
        assert_eq!(
            quickdraw_ads_rates(&ps, rates.0, rates.1, false, Bo2Feel::IW4),
            rates
        );
    }

    #[test]
    fn sprint_rhythm_follows_the_gun_in_bo2() {
        let sprint = pm_flags::SPRINTING;
        let galil = Bo2Feel {
            sprint_cycle_scale: 0.85,
            ..BO2
        };
        let base = bob_step(BO2, 285.0, 127, sprint);
        let scaled = bob_step(galil, 285.0, 127, sprint);
        assert!(scaled < base, "{scaled} vs {base}");
        // Not sprinting: the gun's sprint scale does nothing.
        assert_eq!(bob_step(galil, 190.0, 127, 0), bob_step(BO2, 190.0, 127, 0));
    }

    #[test]
    fn diving_runs_its_own_rhythm_in_the_air_in_bo2() {
        let mut ps = PlayerState::ZERO;
        ps.speed = 190;
        ps.move_speed_scale_multiplier = 1.0;
        ps.view_height_target = 40;
        ps.ground_entity_num = 0x7FF;
        ps.pm_flags = crate::dive::PMF_DIVE;
        ps.velocity = [300.0, 0.0, 50.0];
        let mut iw4 = ps;
        crate::footsteps_bob_cycle(&mut ps, 50, 127, 0, true, 1000, scales(), BO2);
        crate::footsteps_bob_cycle(&mut iw4, 50, 127, 0, true, 1000, scales(), Bo2Feel::IW4);
        // 0.24 x 50 = 12; IW4 keeps its normal step rhythm.
        assert_eq!(ps.bob_cycle, 12);
        assert_ne!(iw4.bob_cycle, 12);
    }

    #[test]
    fn move_faster_walks_at_203() {
        let feel = Bo2Feel {
            move_faster_perk: PERK_MOVEFASTER,
            speed_multiplier: 1.07,
            ..BO2
        };
        let mut ps = PlayerState::ZERO;
        ps.speed = 150;
        assert_eq!(player_speed(&ps, feel), Some(190));
        ps.perks[0] |= PERK_MOVEFASTER;
        assert_eq!(player_speed(&ps, feel), Some(203));
        assert_eq!(player_speed(&ps, Bo2Feel::IW4), None);
    }

    /// Eye height and stance speed `ms` after a drop to prone starts at
    /// crouch height.
    fn prone_after(ms: i32, prone_lerp_ms: i32) -> (f32, f32) {
        let mut ps = PlayerState::ZERO;
        ps.view_height_target = crate::view_height::PRONE;
        ps.view_height_current = 40.0;
        let pml = walking_pml();
        let cmd = UserCmd {
            server_time: 1000,
            ..UserCmd::default()
        };
        crate::update_view_height(&mut ps, &pml, &cmd, prone_lerp_ms);
        assert_eq!(ps.view_height_lerp_time, 1000);
        let scale = crate::stance_speed_scale(&ps, 1000 + ms, 0.15, prone_lerp_ms);
        let cmd = UserCmd {
            server_time: 1000 + ms,
            ..cmd
        };
        crate::update_view_height(&mut ps, &pml, &cmd, prone_lerp_ms);
        (ps.view_height_current, scale)
    }

    #[test]
    fn bo2_prone_takes_600_ms() {
        let (height, scale) = prone_after(400, PRONE_LERP_MS);
        assert!(height > 11.0, "still going down at 400 ms: {height}");
        // Two thirds of the way: 0.15 x 2/3 + 0.65 x 1/3.
        assert!((scale - 0.3167).abs() < 0.001, "{scale}");
        assert_eq!(prone_after(600, PRONE_LERP_MS).0, 11.0);
        // MW2: down at 400 ms.
        assert_eq!(prone_after(400, 0).0, 11.0);
        assert!((prone_after(300, 0).1 - 0.275).abs() < 0.001);
    }
}
