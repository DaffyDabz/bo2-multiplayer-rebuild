use crate::tick::{WeaponCombatFacts, WeaponHandState};
use crate::weap_anim::{start_weapon_anim, weap_anim_event};
use crate::weaponstate::WeaponState;
use playerstate_iw4::pm_flags;

/// Sprint recovery (specialty_sprintrecovery).
pub const PERK_SPRINTRECOVERY: u32 = 1 << 10;

/// perk_sprintRecoveryMultiplier: with sprint recovery the gun comes up after
/// a sprint in this share of its own time (BO2 default 0.3).
pub const SPRINT_RECOVERY_MULTIPLIER: f32 = 0.3;

pub fn weapon_check_for_sprint(
    hand: &mut WeaponHandState,
    facts: &WeaponCombatFacts,
    pm_flags: u32,
    perks0: u32,
) {
    if hand.weapon == 0 {
        return;
    }
    let Ok(ws) = WeaponState::from_i32(hand.weaponstate) else {
        return;
    };
    if !check_for_sprint_allowed(ws) {
        return;
    }
    let sprinting = pm_flags & pm_flags::SPRINTING != 0;
    if sprinting && !ws.is_sprint() {
        begin_sprint(hand, facts);
    } else if !sprinting && pm_flags & PMF_DIVE_ANY != 0 && ws.is_sprint() {
        // bo2zm: a dive ends the sprint with the gun ready at once, so the
        // player can aim and fire through it (an owner's ask, 10-09), and plays
        // Black Ops II's dive animation on it.
        hand.weaponstate = WeaponState::Ready as i32;
        hand.weapon_time = 0;
        hand.weapon_delay = 0;
        let anim = if hand.clip == 0 {
            weap_anim_event::DIVE_IN_EMPTY
        } else {
            weap_anim_event::DIVE_IN
        };
        start_weapon_anim(&mut hand.weap_anim, anim);
    } else if !sprinting && matches!(ws, WeaponState::SprintIn | WeaponState::SprintLoop) {
        begin_sprint_out(hand, facts, perks0);
    }
}

/// bo2zm: the dive flags (movement_iw4::dive PMF_DIVE | PMF_DIVE_SLIDE).
const PMF_DIVE: u32 = 0x0100_0000;
const PMF_DIVE_ANY: u32 = PMF_DIVE | 0x0200_0000;

/// bo2zm: the rest of Black Ops II's dive animation on a ready gun: the
/// touch-down one when the dive leaves the air, and the gun up at once when
/// the player aims before the slide ends. A shot replaces the animation.
pub fn weapon_dive_anim(hand: &mut WeaponHandState, pm_flags: u32) {
    if hand.weapon == 0 || hand.weaponstate != WeaponState::Ready as i32 {
        return;
    }
    let anim = hand.weap_anim as u32 & crate::WEAP_ANIM_EVENT_MASK;
    let in_air = matches!(
        anim,
        weap_anim_event::DIVE_IN | weap_anim_event::DIVE_IN_EMPTY
    );
    if !in_air
        && !matches!(
            anim,
            weap_anim_event::DIVE_OUT | weap_anim_event::DIVE_OUT_EMPTY
        )
    {
        return;
    }
    let next = if pm_flags & pm_flags::ADS_INTENT != 0 && pm_flags & PMF_DIVE_ANY != 0 {
        weap_anim_event::FORCE_IDLE
    } else if in_air && pm_flags & PMF_DIVE == 0 {
        anim + (weap_anim_event::DIVE_OUT - weap_anim_event::DIVE_IN)
    } else {
        return;
    };
    start_weapon_anim(&mut hand.weap_anim, next);
}

fn check_for_sprint_allowed(ws: WeaponState) -> bool {
    !matches!(
        ws,
        WeaponState::Raising
            | WeaponState::RaisingAltswitch
            | WeaponState::Dropping
            | WeaponState::DroppingQuick
            | WeaponState::DroppingAltswitch
            | WeaponState::Firing
            | WeaponState::Rechambering
            | WeaponState::MeleeInit
            | WeaponState::MeleeFire
            | WeaponState::MeleeEnd
            | WeaponState::OffhandInit
            | WeaponState::OffhandPrepare
            | WeaponState::OffhandHold
            | WeaponState::OffhandStart
            | WeaponState::Offhand
            | WeaponState::OffhandEnd
            | WeaponState::NightVisionWear
            | WeaponState::NightVisionRemove
    )
}

/// bo2zm: an empty magazine sprints with Black Ops II's empty-magazine
/// sprint animations (a gun without them plays its full ones).
fn sprint_anim(hand: &WeaponHandState, full: u32) -> u32 {
    if hand.clip != 0 {
        return full;
    }
    full + (weap_anim_event::SPRINT_IN_EMPTY - weap_anim_event::SPRINT_IN)
}

fn begin_sprint(hand: &mut WeaponHandState, facts: &WeaponCombatFacts) {
    let time = if facts.sprint_raise_time_ms > 0 {
        facts.sprint_raise_time_ms
    } else {
        1
    };
    hand.weaponstate = WeaponState::SprintIn as i32;
    hand.weapon_time = time;
    hand.weapon_delay = 0;
    hand.shot_count = 0;
    hand.burst_latch = false;
    let anim = sprint_anim(hand, weap_anim_event::SPRINT_IN);
    start_weapon_anim(&mut hand.weap_anim, anim);
}

fn sprint_loop(hand: &mut WeaponHandState) {
    hand.weaponstate = WeaponState::SprintLoop as i32;
    hand.weapon_time = 0;
    hand.weapon_delay = 0;
    let anim = sprint_anim(hand, weap_anim_event::SPRINT_LOOP);
    start_weapon_anim(&mut hand.weap_anim, anim);
}

fn begin_sprint_out(hand: &mut WeaponHandState, facts: &WeaponCombatFacts, perks0: u32) {
    let mut time = facts.sprint_drop_time_ms;
    if perks0 & PERK_SPRINTRECOVERY != 0 {
        time = (time as f32 * SPRINT_RECOVERY_MULTIPLIER) as i32;
    }
    let time = if time > 0 { time } else { 1 };
    hand.weaponstate = WeaponState::SprintOut as i32;
    hand.weapon_time = time;
    hand.weapon_delay = 0;
    let anim = sprint_anim(hand, weap_anim_event::SPRINT_OUT);
    start_weapon_anim(&mut hand.weap_anim, anim);
}

pub fn weapon_advance_sprint(
    hand: &mut WeaponHandState,
    weap_flags: &mut u32,
    pm_flags_word: &mut u32,
    pm_type: i32,
) {
    match WeaponState::from_i32(hand.weaponstate) {
        Ok(WeaponState::SprintIn) if hand.weapon_time <= 0 => sprint_loop(hand),
        Ok(WeaponState::SprintOut) if hand.weapon_time <= 0 => {
            crate::melee::weapon_settle_ready(hand, weap_flags, pm_flags_word, pm_type);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sprinting_hand() -> (WeaponHandState, WeaponCombatFacts) {
        let facts = WeaponCombatFacts {
            sprint_raise_time_ms: 200,
            sprint_drop_time_ms: 250,
            ..WeaponCombatFacts::default()
        };
        let mut hand = WeaponHandState {
            weapon: 1,
            ..WeaponHandState::default()
        };
        weapon_check_for_sprint(&mut hand, &facts, pm_flags::SPRINTING, 0);
        hand.weapon_time = 0;
        weapon_advance_sprint(&mut hand, &mut 0, &mut 0, 0);
        assert_eq!(hand.weaponstate, WeaponState::SprintLoop as i32);
        (hand, facts)
    }

    fn anim(hand: &WeaponHandState) -> u32 {
        hand.weap_anim as u32 & crate::WEAP_ANIM_EVENT_MASK
    }

    /// Sprints, dives, and runs the dive's first tick.
    fn dived_hand(clip: i32) -> WeaponHandState {
        let (mut hand, facts) = sprinting_hand();
        hand.clip = clip;
        weapon_check_for_sprint(&mut hand, &facts, PMF_DIVE, 0);
        weapon_advance_sprint(&mut hand, &mut 0, &mut 0, 0);
        weapon_dive_anim(&mut hand, PMF_DIVE);
        hand
    }

    #[test]
    fn a_dive_brings_the_gun_straight_up() {
        let hand = dived_hand(30);
        assert_eq!(hand.weaponstate, WeaponState::Ready as i32);
        assert_eq!(hand.weapon_time, 0);
        assert_eq!(anim(&hand), weap_anim_event::DIVE_IN);
    }

    #[test]
    fn the_landing_animation_plays_at_touch_down() {
        let mut hand = dived_hand(30);
        let sliding = PMF_DIVE_ANY & !PMF_DIVE;
        weapon_dive_anim(&mut hand, sliding);
        assert_eq!(anim(&hand), weap_anim_event::DIVE_OUT);
        // Played once: the slide and lying prone change nothing.
        let raw = hand.weap_anim;
        weapon_dive_anim(&mut hand, sliding);
        weapon_dive_anim(&mut hand, pm_flags::PRONE);
        assert_eq!(hand.weap_anim, raw);
    }

    #[test]
    fn an_empty_gun_dives_with_the_empty_animations() {
        let mut hand = dived_hand(0);
        assert_eq!(anim(&hand), weap_anim_event::DIVE_IN_EMPTY);
        weapon_dive_anim(&mut hand, 0);
        assert_eq!(anim(&hand), weap_anim_event::DIVE_OUT_EMPTY);
    }

    #[test]
    fn aiming_mid_dive_brings_the_gun_up() {
        let mut hand = dived_hand(30);
        weapon_dive_anim(&mut hand, PMF_DIVE | pm_flags::ADS_INTENT);
        assert_eq!(anim(&hand), weap_anim_event::FORCE_IDLE);
        // Nothing more to play: no landing animation after it.
        let raw = hand.weap_anim;
        weapon_dive_anim(&mut hand, pm_flags::PRONE | pm_flags::ADS_INTENT);
        assert_eq!(hand.weap_anim, raw);
    }

    #[test]
    fn aiming_after_the_slide_leaves_the_gun_alone() {
        let mut hand = dived_hand(30);
        weapon_dive_anim(&mut hand, 0);
        let raw = hand.weap_anim;
        weapon_dive_anim(&mut hand, pm_flags::PRONE | pm_flags::ADS_INTENT);
        assert_eq!(hand.weap_anim, raw);
    }

    #[test]
    fn an_empty_gun_sprints_with_the_empty_animations() {
        let (mut hand, facts) = sprinting_hand();
        assert_eq!(anim(&hand), weap_anim_event::SPRINT_LOOP_EMPTY);
        weapon_check_for_sprint(&mut hand, &facts, 0, 0);
        assert_eq!(anim(&hand), weap_anim_event::SPRINT_OUT_EMPTY);
        let mut full = WeaponHandState {
            weapon: 1,
            clip: 30,
            ..WeaponHandState::default()
        };
        weapon_check_for_sprint(&mut full, &facts, pm_flags::SPRINTING, 0);
        assert_eq!(anim(&full), weap_anim_event::SPRINT_IN);
    }

    #[test]
    fn stopping_a_sprint_still_lowers_the_gun_out() {
        let (mut hand, facts) = sprinting_hand();
        weapon_check_for_sprint(&mut hand, &facts, 0, 0);
        weapon_advance_sprint(&mut hand, &mut 0, &mut 0, 0);
        assert_eq!(hand.weaponstate, WeaponState::SprintOut as i32);
        assert_eq!(hand.weapon_time, 250);
    }

    #[test]
    fn sprint_recovery_brings_the_gun_up_in_30_percent() {
        let (mut hand, facts) = sprinting_hand();
        weapon_check_for_sprint(&mut hand, &facts, 0, PERK_SPRINTRECOVERY);
        assert_eq!(hand.weaponstate, WeaponState::SprintOut as i32);
        assert_eq!(hand.weapon_time, 75);
        let (mut hand, facts) = sprinting_hand();
        weapon_check_for_sprint(&mut hand, &facts, 0, 0);
        assert_eq!(hand.weapon_time, 250);
    }
}
