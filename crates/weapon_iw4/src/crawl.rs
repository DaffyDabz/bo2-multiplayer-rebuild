//! bo2zm: Black Ops II's prone crawl animations on the gun.
//!
//! While the player crawls prone with the gun ready, the gun plays the
//! crawl animation of the direction the player crawls; it plays the
//! crawl-out when the player stops, and comes straight up when the player
//! aims or fires. The rules are designed by feel until real Black Ops II is
//! read; the animations and their times are each gun's own.

use crate::tick::WeaponHandState;
use crate::weap_anim::{start_weapon_anim, weap_anim_event};
use crate::weaponstate::WeaponState;
use playerstate_iw4::{buttons, pm_flags};

/// The dive flags (movement_iw4::dive PMF_DIVE, PMF_DIVE_SLIDE,
/// PMF_DIVE_GETUP): no crawl animation through a dive or its get-up.
const PMF_DIVE_NO_CRAWL: u32 = 0x0100_0000 | 0x0200_0000 | 0x0800_0000;

/// The crawl direction for this move input: the stronger axis, forward and
/// back on a tie.
fn crawl_direction(move_input: [i8; 2]) -> Option<u32> {
    let [forward, right] = move_input.map(i32::from);
    if forward == 0 && right == 0 {
        return None;
    }
    Some(if forward.abs() >= right.abs() {
        if forward > 0 {
            weap_anim_event::CRAWL_FORWARD
        } else {
            weap_anim_event::CRAWL_BACK
        }
    } else if right > 0 {
        weap_anim_event::CRAWL_RIGHT
    } else {
        weap_anim_event::CRAWL_LEFT
    })
}

/// Whether this gun animation is one of the crawl directions.
fn crawling(anim: u32) -> bool {
    (weap_anim_event::CRAWL_FORWARD..=weap_anim_event::CRAWL_LEFT_EMPTY).contains(&anim)
        && anim != weap_anim_event::CRAWL_OUT
}

pub fn weapon_crawl_anim(
    hand: &mut WeaponHandState,
    pm_flags: u32,
    pm_type: i32,
    buttons: u32,
    move_input: [i8; 2],
) {
    if hand.weapon == 0 || hand.weaponstate != WeaponState::Ready as i32 {
        return;
    }
    let anim = hand.weap_anim as u32 & crate::WEAP_ANIM_EVENT_MASK;
    let aiming = pm_flags & (pm_flags::ADS_INTENT | pm_flags::PRONEMOVE_OVERRIDDEN) != 0;
    let firing = buttons & buttons::ATTACK != 0;
    let prone = pm_flags & pm_flags::PRONE != 0 && pm_flags & PMF_DIVE_NO_CRAWL == 0;
    let wanted = if prone && !aiming && !firing && pm_type < 8 {
        crawl_direction(move_input)
    } else {
        None
    };
    let empty = if hand.clip == 0 {
        weap_anim_event::CRAWL_EMPTY_OFFSET
    } else {
        0
    };
    let next = match wanted {
        Some(direction) => {
            let starting = !crawling(anim);
            if starting && (hand.weapon_time > 0 || hand.weapon_delay > 0) {
                return;
            }
            direction + empty
        }
        None if crawling(anim) && (aiming || firing) => weap_anim_event::FORCE_IDLE,
        None if crawling(anim) => weap_anim_event::CRAWL_OUT + empty,
        None => return,
    };
    if next != anim {
        start_weapon_anim(&mut hand.weap_anim, next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRONE: u32 = pm_flags::PRONE;

    fn ready_hand() -> WeaponHandState {
        WeaponHandState {
            weapon: 1,
            clip: 30,
            weaponstate: WeaponState::Ready as i32,
            ..WeaponHandState::default()
        }
    }

    fn anim(hand: &WeaponHandState) -> u32 {
        hand.weap_anim as u32 & crate::WEAP_ANIM_EVENT_MASK
    }

    #[test]
    fn crawling_plays_the_direction_the_player_crawls() {
        let mut hand = ready_hand();
        weapon_crawl_anim(&mut hand, PRONE, 0, 0, [127, 0]);
        assert_eq!(anim(&hand), weap_anim_event::CRAWL_FORWARD);
        weapon_crawl_anim(&mut hand, PRONE, 0, 0, [0, -127]);
        assert_eq!(anim(&hand), weap_anim_event::CRAWL_LEFT);
        weapon_crawl_anim(&mut hand, PRONE, 0, 0, [-127, 40]);
        assert_eq!(anim(&hand), weap_anim_event::CRAWL_BACK);
        weapon_crawl_anim(&mut hand, PRONE, 0, 0, [20, 127]);
        assert_eq!(anim(&hand), weap_anim_event::CRAWL_RIGHT);
        // Held direction: the event is not restarted.
        let raw = hand.weap_anim;
        weapon_crawl_anim(&mut hand, PRONE, 0, 0, [20, 127]);
        assert_eq!(hand.weap_anim, raw);
    }

    #[test]
    fn stopping_plays_the_crawl_out_once() {
        let mut hand = ready_hand();
        weapon_crawl_anim(&mut hand, PRONE, 0, 0, [127, 0]);
        weapon_crawl_anim(&mut hand, PRONE, 0, 0, [0, 0]);
        assert_eq!(anim(&hand), weap_anim_event::CRAWL_OUT);
        let raw = hand.weap_anim;
        weapon_crawl_anim(&mut hand, PRONE, 0, 0, [0, 0]);
        assert_eq!(hand.weap_anim, raw);
    }

    #[test]
    fn aiming_or_firing_brings_the_gun_straight_up() {
        for (flags, buttons) in [(PRONE | pm_flags::ADS_INTENT, 0), (PRONE, buttons::ATTACK)] {
            let mut hand = ready_hand();
            weapon_crawl_anim(&mut hand, PRONE, 0, 0, [127, 0]);
            weapon_crawl_anim(&mut hand, flags, 0, buttons, [127, 0]);
            assert_eq!(anim(&hand), weap_anim_event::FORCE_IDLE);
        }
    }

    #[test]
    fn an_empty_gun_crawls_with_the_empty_animations() {
        let mut hand = ready_hand();
        hand.clip = 0;
        weapon_crawl_anim(&mut hand, PRONE, 0, 0, [127, 0]);
        assert_eq!(anim(&hand), weap_anim_event::CRAWL_FORWARD_EMPTY);
        weapon_crawl_anim(&mut hand, PRONE, 0, 0, [0, 0]);
        assert_eq!(anim(&hand), weap_anim_event::CRAWL_OUT_EMPTY);
    }

    #[test]
    fn no_crawl_standing_diving_or_mid_action() {
        let mut hand = ready_hand();
        weapon_crawl_anim(&mut hand, 0, 0, 0, [127, 0]);
        weapon_crawl_anim(&mut hand, PRONE | 0x0200_0000, 0, 0, [127, 0]);
        assert_eq!(hand.weap_anim, 0, "standing or sliding out of a dive");
        hand.weapon_time = 100;
        weapon_crawl_anim(&mut hand, PRONE, 0, 0, [127, 0]);
        assert_eq!(hand.weap_anim, 0, "a shot's recovery finishes first");
        hand.weapon_time = 0;
        hand.weaponstate = WeaponState::Reloading as i32;
        weapon_crawl_anim(&mut hand, PRONE, 0, 0, [127, 0]);
        assert_eq!(hand.weap_anim, 0, "reloading");
    }
}
