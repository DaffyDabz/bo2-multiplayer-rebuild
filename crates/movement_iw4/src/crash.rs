use playerstate_iw4::PlayerState;

use crate::{Bo2Feel, Pml, add_predictable_event, jump};

const EV_FOOTSTEP_RUN: i32 = 0x6c;
const EV_FOOTSTEP_WALK: i32 = 0x6d;

const EV_LANDING_FIRST: i32 = 0x70;

const EV_LANDING_PAIN_FIRST: i32 = 0x8f;

const FALL_LIGHT_IN: f32 = 4.0;

const FALL_MEDIUM_IN: f32 = 8.0;

const FALL_HARD_IN: f32 = 12.0;

const HARD_LAND_VEL_SCALE: f32 = 0.67;

const HALF: f32 = 0.5;

const FOUR: f32 = 4.0;

const TWO: f32 = 2.0;

const NEG_ONE: f32 = -1.0;

pub fn crash_land(ps: &mut PlayerState, pml: &Pml, feel: Bo2Feel) {
    let Some(fall_height) = crash_land_fall_height(ps, pml) else {
        return;
    };
    let surface = jump::ground_surface_type(pml.ground_trace[4]);
    // bo2zm: Black Ops II falls hurt, and a fall that hurts without killing
    // slows you down for a moment.
    if feel.on {
        let damage = bo2_fall_damage(ps, pml, fall_height, feel);
        if damage > 0 {
            bo2_damage_landing(ps, pml, damage, surface);
            return;
        }
    }
    crash_land_apply_sfx(ps, fall_height, surface);
}

/// bo2zm: dive to prone counts falls from this height (dtp_fall_damage_*).
const DIVE_FALL_DAMAGE_MIN_HEIGHT: f32 = 65.0;
const DIVE_FALL_DAMAGE_MAX_HEIGHT: f32 = 200.0;

fn fall_damage_between(fall_height: f32, min: f32, max: f32) -> i32 {
    if fall_height <= min {
        0
    } else if fall_height < max {
        (((fall_height - min) / (max - min) * 100.0) as i32).clamp(0, 100)
    } else {
        100
    }
}

/// bo2zm: the fall damage, 0 to 100 percent of health.
pub fn bo2_fall_damage(ps: &PlayerState, pml: &Pml, fall_height: f32, feel: Bo2Feel) -> i32 {
    let mut damage = fall_damage_between(
        fall_height,
        feel.fall_damage_min_height,
        feel.fall_damage_max_height,
    );
    if (ps.pm_flags & crate::dive::PMF_DIVE) != 0 {
        damage = fall_damage_between(
            fall_height,
            DIVE_FALL_DAMAGE_MIN_HEIGHT,
            DIVE_FALL_DAMAGE_MAX_HEIGHT,
        );
    }
    let perk = feel.fall_damage_perk != 0 && (ps.perks[0] & feel.fall_damage_perk) != 0;
    if feel.zombies {
        if ps.gravity < feel.gravity {
            damage = 0;
        }
        if perk && damage > 0 {
            damage = 1;
        }
    } else if perk {
        damage = 0;
    }
    // A no-damage floor, or a dead player.
    if (pml.ground_trace[4] & 1) != 0 || ps.pm_type >= 8 {
        damage = 0;
    }
    damage
}

/// Stun after a fall that hurts: the longer it is, the slower you go.
const FALL_STUN_BASE_MS: i32 = 500;
const FALL_STUN_PER_DAMAGE_MS: i32 = 35;
const FALL_STUN_MAX_MS: i32 = 2000;

fn bo2_damage_landing(ps: &mut PlayerState, pml: &Pml, damage: i32, surface: i32) {
    let scale = if damage >= 100 || (pml.ground_trace[4] & 2) != 0 {
        HARD_LAND_VEL_SCALE
    } else {
        let stun = (FALL_STUN_PER_DAMAGE_MS * damage + FALL_STUN_BASE_MS).min(FALL_STUN_MAX_MS);
        ps.pm_time = stun;
        ps.pm_flags |= playerstate_iw4::pm_flags::TIME_HARDLANDING;
        if stun <= 500 {
            0.5
        } else if stun < 1500 {
            0.5 - (stun as f32 - 500.0) / 1000.0 * 0.3
        } else {
            0.2
        }
    };
    ps.velocity[0] *= scale;
    ps.velocity[1] *= scale;
    ps.velocity[2] *= scale;
    add_predictable_event(ps, EV_LANDING_PAIN_FIRST + surface, damage);
}

pub fn crash_land_fall_height(ps: &PlayerState, pml: &Pml) -> Option<f32> {
    if ps.gravity == 0 {
        return None;
    }
    let dist = pml.previous_origin[2] - ps.origin[2];
    let vel = pml.previous_velocity[2];
    let acc = -(ps.gravity as f32);
    let a = acc * HALF;
    let den = vel * vel - FOUR * a * dist;
    if den < 0.0 {
        return None;
    }
    let two_a = a * TWO;
    if two_a == 0.0 {
        return None;
    }
    let t = (-vel - libm::sqrtf(den)) / two_a;
    let land_vel = (t * acc + vel) * NEG_ONE;
    Some((land_vel * land_vel) / ((ps.gravity as f32) * TWO))
}

fn crash_land_apply_sfx(ps: &mut PlayerState, fall_height: f32, surface: i32) {
    if fall_height <= FALL_LIGHT_IN {
        return;
    }
    if fall_height < FALL_MEDIUM_IN {
        if surface != 0 {
            add_predictable_event(ps, EV_FOOTSTEP_WALK, surface);
        }
        return;
    }
    if fall_height < FALL_HARD_IN {
        if surface != 0 {
            add_predictable_event(ps, EV_FOOTSTEP_RUN, surface);
        }
        return;
    }
    ps.velocity[0] *= HARD_LAND_VEL_SCALE;
    ps.velocity[1] *= HARD_LAND_VEL_SCALE;
    ps.velocity[2] *= HARD_LAND_VEL_SCALE;
    if surface != 0 {
        add_predictable_event(ps, EV_LANDING_FIRST + surface, 0);
    }
}

#[cfg(test)]
mod bo2_tests {
    use super::*;

    const ZM: Bo2Feel = Bo2Feel {
        on: true,
        sprint_strafe_speed_scale: 0.667,
        jump_slowdown: true,
        fall_damage_min_height: 128.0,
        fall_damage_max_height: 350.0,
        zombies: true,
        gravity: 800,
        fall_damage_perk: 0x40,
        ..Bo2Feel::IW4
    };
    const MP: Bo2Feel = Bo2Feel {
        fall_damage_max_height: 300.0,
        zombies: false,
        ..ZM
    };

    /// Lands from `height` units up, running at 100 u/s.
    fn land(height: f32, feel: Bo2Feel, setup: impl Fn(&mut PlayerState)) -> PlayerState {
        let mut ps = PlayerState::ZERO;
        ps.gravity = 800;
        ps.velocity = [100.0, 0.0, 0.0];
        setup(&mut ps);
        let pml = Pml {
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
            previous_velocity: [0.0, 0.0, -libm::sqrtf(1600.0 * height)],
            holdrand: 0,
        };
        crash_land(&mut ps, &pml, feel);
        ps
    }

    fn pain(ps: &PlayerState) -> Option<i32> {
        (ps.events_0 == EV_LANDING_PAIN_FIRST).then_some(ps.event_parms_0)
    }

    #[test]
    fn falls_do_not_hurt_in_iw4() {
        let ps = land(200.0, Bo2Feel::IW4, |_| {});
        assert_eq!(pain(&ps), None);
        assert_eq!(ps.pm_time, 0);
    }

    #[test]
    fn a_fall_that_hurts_slows_you_down_in_bo2() {
        // Zombies: 128 to 350 units. 200 = 32 damage, 1620 ms at 0.2 speed.
        let ps = land(200.0, ZM, |_| {});
        assert_eq!(pain(&ps), Some(32));
        assert_eq!(ps.pm_time, 1620);
        assert!(ps.pm_flags & 0x80 != 0);
        assert!((ps.velocity[0] - 20.0).abs() < 1e-3);
        // Multiplayer: 128 to 300 units.
        assert_eq!(pain(&land(200.0, MP, |_| {})), Some(41));
    }

    #[test]
    fn a_full_damage_fall_keeps_two_thirds_of_your_speed() {
        let ps = land(400.0, ZM, |_| {});
        assert_eq!(pain(&ps), Some(100));
        assert_eq!(ps.pm_time, 0);
        assert!((ps.velocity[0] - 67.0).abs() < 1e-3);
    }

    #[test]
    fn short_falls_do_not_hurt() {
        assert_eq!(pain(&land(130.0, ZM, |_| {})), None);
    }

    #[test]
    fn the_fall_perk_in_zombies_and_multiplayer() {
        let perk = |ps: &mut PlayerState| ps.perks[0] = 0x40;
        // Zombies: 1 damage and a short stun.
        let ps = land(300.0, ZM, perk);
        assert_eq!(pain(&ps), Some(1));
        assert_eq!(ps.pm_time, 535);
        // Multiplayer: no damage.
        assert_eq!(pain(&land(300.0, MP, perk)), None);
    }

    #[test]
    fn diving_counts_falls_from_65_units() {
        let diving = |ps: &mut PlayerState| ps.pm_flags |= crate::dive::PMF_DIVE;
        assert_eq!(pain(&land(100.0, ZM, diving)), Some(25));
        assert_eq!(pain(&land(100.0, ZM, |_| {})), None);
    }

    #[test]
    fn low_gravity_falls_do_not_hurt_in_zombies() {
        let ps = land(200.0, ZM, |ps| ps.gravity = 400);
        assert_eq!(pain(&ps), None);
    }
}
