use asset_iw4::size::{WEAPON_ANIM_COUNT, weap_anim};

pub const ACTION_GOAL_TIME_SECS: f32 = 0.0;

pub const IDLE_INTERRUPT_GOAL_TIME_SECS: f32 = 0.5;

pub const ACTIVE_GOAL_WEIGHT: f32 = 1.0;
pub const INACTIVE_GOAL_WEIGHT: f32 = 0.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnimRateOffsets {
    pub weapon_def: i32,
    pub complete_def: i32,
}

impl AnimRateOffsets {
    pub const NATIVE: Self = Self {
        weapon_def: -1,
        complete_def: -1,
    };

    pub const fn is_native(self) -> bool {
        self.weapon_def < 0 && self.complete_def < 0
    }
}

pub const WEAPON_ANIM_SLOTS: usize = WEAPON_ANIM_COUNT + 28;

pub mod weap_anim_extra {
    use super::WEAPON_ANIM_COUNT;

    pub const RELOAD_QUICK: usize = WEAPON_ANIM_COUNT;
    pub const RELOAD_QUICK_EMPTY: usize = WEAPON_ANIM_COUNT + 1;
    /// bo2zm: Black Ops II's dive-to-prone animations (`d2p_in`, `_loop`,
    /// `_out`, and their empty-magazine ones).
    pub const DIVE_IN: usize = WEAPON_ANIM_COUNT + 2;
    pub const DIVE_LOOP: usize = WEAPON_ANIM_COUNT + 3;
    pub const DIVE_OUT: usize = WEAPON_ANIM_COUNT + 4;
    pub const DIVE_IN_EMPTY: usize = WEAPON_ANIM_COUNT + 5;
    pub const DIVE_LOOP_EMPTY: usize = WEAPON_ANIM_COUNT + 6;
    pub const DIVE_OUT_EMPTY: usize = WEAPON_ANIM_COUNT + 7;
    /// bo2zm: Black Ops II's empty-magazine sprint animations.
    pub const SPRINT_IN_EMPTY: usize = WEAPON_ANIM_COUNT + 8;
    pub const SPRINT_LOOP_EMPTY: usize = WEAPON_ANIM_COUNT + 9;
    pub const SPRINT_OUT_EMPTY: usize = WEAPON_ANIM_COUNT + 10;
    /// bo2zm: Black Ops II's prone crawl animations: in, the four
    /// directions (looping), out; then their empty-magazine ones.
    pub const CRAWL_IN: usize = WEAPON_ANIM_COUNT + 11;
    pub const CRAWL_FORWARD: usize = WEAPON_ANIM_COUNT + 12;
    pub const CRAWL_BACK: usize = WEAPON_ANIM_COUNT + 13;
    pub const CRAWL_RIGHT: usize = WEAPON_ANIM_COUNT + 14;
    pub const CRAWL_LEFT: usize = WEAPON_ANIM_COUNT + 15;
    pub const CRAWL_OUT: usize = WEAPON_ANIM_COUNT + 16;
    pub const CRAWL_IN_EMPTY: usize = WEAPON_ANIM_COUNT + 17;
    pub const CRAWL_FORWARD_EMPTY: usize = WEAPON_ANIM_COUNT + 18;
    pub const CRAWL_BACK_EMPTY: usize = WEAPON_ANIM_COUNT + 19;
    pub const CRAWL_RIGHT_EMPTY: usize = WEAPON_ANIM_COUNT + 20;
    pub const CRAWL_LEFT_EMPTY: usize = WEAPON_ANIM_COUNT + 21;
    pub const CRAWL_OUT_EMPTY: usize = WEAPON_ANIM_COUNT + 22;
    /// bo2zm: Black Ops II's climb camera animation: it moves only the
    /// camera, played over a low climb that keeps the gun in hand.
    pub const CAMERA_MANTLE: usize = WEAPON_ANIM_COUNT + 23;
    /// bo2zm: Black Ops II's melee and charged melee with an empty
    /// magazine (a pistol's slide stays back).
    pub const MELEE_EMPTY: usize = WEAPON_ANIM_COUNT + 24;
    pub const MELEE_CHARGE_EMPTY: usize = WEAPON_ANIM_COUNT + 25;
    /// bo2zm: Black Ops II's first-shots fire, hip and aimed (the HAMR's
    /// fast start, the AN-94's intro shot).
    pub const FIRE_INTRO: usize = WEAPON_ANIM_COUNT + 26;
    pub const ADS_FIRE_INTRO: usize = WEAPON_ANIM_COUNT + 27;
}

pub const ANIM_RATE_TABLE: [AnimRateOffsets; WEAPON_ANIM_COUNT] = [
    AnimRateOffsets::NATIVE,
    AnimRateOffsets::NATIVE,
    AnimRateOffsets::NATIVE,
    AnimRateOffsets::NATIVE,
    AnimRateOffsets {
        weapon_def: 0x25c,
        complete_def: -1,
    },
    AnimRateOffsets::NATIVE,
    AnimRateOffsets::NATIVE,
    AnimRateOffsets {
        weapon_def: 0x264,
        complete_def: -1,
    },
    AnimRateOffsets {
        weapon_def: 0x268,
        complete_def: -1,
    },
    AnimRateOffsets {
        weapon_def: 0x26c,
        complete_def: -1,
    },
    AnimRateOffsets {
        weapon_def: 0x274,
        complete_def: -1,
    },
    AnimRateOffsets {
        weapon_def: 0x27c,
        complete_def: -1,
    },
    AnimRateOffsets {
        weapon_def: 0x284,
        complete_def: -1,
    },
    AnimRateOffsets {
        weapon_def: 0x28c,
        complete_def: -1,
    },
    AnimRateOffsets {
        weapon_def: -1,
        complete_def: 0x54,
    },
    AnimRateOffsets {
        weapon_def: 0x29c,
        complete_def: -1,
    },
    AnimRateOffsets {
        weapon_def: 0x288,
        complete_def: -1,
    },
    AnimRateOffsets {
        weapon_def: -1,
        complete_def: 0x44,
    },
    AnimRateOffsets {
        weapon_def: 0x290,
        complete_def: -1,
    },
    AnimRateOffsets {
        weapon_def: 0x298,
        complete_def: -1,
    },
    AnimRateOffsets {
        weapon_def: 0x294,
        complete_def: -1,
    },
    AnimRateOffsets {
        weapon_def: 0x2a0,
        complete_def: -1,
    },
    AnimRateOffsets {
        weapon_def: 0x2a4,
        complete_def: -1,
    },
    AnimRateOffsets {
        weapon_def: 0x2a8,
        complete_def: -1,
    },
    AnimRateOffsets {
        weapon_def: 0x2ac,
        complete_def: -1,
    },
    AnimRateOffsets {
        weapon_def: 0x2b0,
        complete_def: -1,
    },
    AnimRateOffsets {
        weapon_def: 0x2b4,
        complete_def: -1,
    },
    AnimRateOffsets {
        weapon_def: 0x2b8,
        complete_def: -1,
    },
    AnimRateOffsets {
        weapon_def: 0x2bc,
        complete_def: -1,
    },
    AnimRateOffsets::NATIVE,
    AnimRateOffsets {
        weapon_def: 0x2c0,
        complete_def: -1,
    },
    AnimRateOffsets {
        weapon_def: 0x2cc,
        complete_def: -1,
    },
    AnimRateOffsets::NATIVE,
    AnimRateOffsets::NATIVE,
    AnimRateOffsets::NATIVE,
    AnimRateOffsets::NATIVE,
    AnimRateOffsets::NATIVE,
];

pub const WEAP_ANIM_EVENT_MASK: u32 = 0xffff_fdff;

pub fn slot_for_weap_anim_event(masked_event: u32) -> Option<usize> {
    let ev = masked_event & WEAP_ANIM_EVENT_MASK;
    Some(match ev {
        0 | 1 => return None,
        2 => weap_anim::FIRE,
        3 => weap_anim::LASTSHOT,
        4 => weap_anim::RECHAMBER,
        5 => weap_anim::ADS_FIRE,
        6 => weap_anim::ADS_LASTSHOT,
        7 => weap_anim::ADS_RECHAMBER,
        8 => weap_anim::MELEE,
        9 => weap_anim::MELEE_CHARGE,
        0xa => weap_anim::DROP,
        0xb => weap_anim::RAISE,
        0xc => weap_anim::FIRST_RAISE,
        0xd => weap_anim::RELOAD,
        0xe => weap_anim::RELOAD_EMPTY,
        0xf => weap_anim::RELOAD_START,
        0x10 => weap_anim::RELOAD_END,
        0x11 => weap_anim::ALT_DROP,
        0x12 => weap_anim::ALT_RAISE,
        0x13 => weap_anim::QUICK_DROP,
        0x14 => weap_anim::QUICK_RAISE,
        0x15 => weap_anim::EMPTY_DROP,
        0x16 => weap_anim::EMPTY_RAISE,
        0x17 => weap_anim::SPRINT_IN,
        0x18 => weap_anim::SPRINT_LOOP,
        0x19 => weap_anim::SPRINT_OUT,
        0x1a => weap_anim::STUNNED_START,
        0x1b => weap_anim::STUNNED_LOOP,
        0x1c => weap_anim::STUNNED_END,
        0x1d => weap_anim::HOLD_FIRE,
        0x1e => weap_anim::DETONATE,
        0x1f => weap_anim::NIGHTVISION_WEAR,
        0x20 => weap_anim::NIGHTVISION_REMOVE,
        0x21 => weap_anim_extra::RELOAD_QUICK,
        0x22 => weap_anim_extra::RELOAD_QUICK_EMPTY,
        0x23 => weap_anim_extra::DIVE_IN,
        0x24 => weap_anim_extra::DIVE_OUT,
        0x25 => weap_anim_extra::DIVE_IN_EMPTY,
        0x26 => weap_anim_extra::DIVE_OUT_EMPTY,
        0x27 => weap_anim_extra::SPRINT_IN_EMPTY,
        0x28 => weap_anim_extra::SPRINT_LOOP_EMPTY,
        0x29 => weap_anim_extra::SPRINT_OUT_EMPTY,
        0x2a => weap_anim_extra::CRAWL_FORWARD,
        0x2b => weap_anim_extra::CRAWL_BACK,
        0x2c => weap_anim_extra::CRAWL_RIGHT,
        0x2d => weap_anim_extra::CRAWL_LEFT,
        0x2e => weap_anim_extra::CRAWL_OUT,
        0x2f => weap_anim_extra::CRAWL_FORWARD_EMPTY,
        0x30 => weap_anim_extra::CRAWL_BACK_EMPTY,
        0x31 => weap_anim_extra::CRAWL_RIGHT_EMPTY,
        0x32 => weap_anim_extra::CRAWL_LEFT_EMPTY,
        0x33 => weap_anim_extra::CRAWL_OUT_EMPTY,
        0x34 => weap_anim_extra::MELEE_EMPTY,
        0x35 => weap_anim_extra::MELEE_CHARGE_EMPTY,
        0x36 => weap_anim_extra::FIRE_INTRO,
        0x37 => weap_anim_extra::ADS_FIRE_INTRO,
        _ => weap_anim::IDLE,
    })
}

pub fn playback_rate(clip_length_ms: i32, weapon_timer_ms: i32) -> f32 {
    if weapon_timer_ms < 0 {
        return 1.0;
    }
    if weapon_timer_ms == 0 {
        return 0.0;
    }
    if clip_length_ms <= 0 {
        return 1.0;
    }
    clip_length_ms as f32 / weapon_timer_ms as f32
}

pub fn slot_uses_native_rate(slot: usize) -> bool {
    // bo2zm: the dive, crawl and empty-magazine melee animations play over
    // the gun's own times.
    if (weap_anim_extra::DIVE_IN..=weap_anim_extra::DIVE_OUT_EMPTY).contains(&slot)
        || (weap_anim_extra::CRAWL_IN..=weap_anim_extra::CRAWL_OUT_EMPTY).contains(&slot)
        || (weap_anim_extra::MELEE_EMPTY..=weap_anim_extra::MELEE_CHARGE_EMPTY).contains(&slot)
    {
        return false;
    }
    ANIM_RATE_TABLE
        .get(slot)
        .map(|e| e.is_native())
        .unwrap_or(true)
}

pub fn known_rate_timer_offset(slot: usize) -> Option<i32> {
    let e = ANIM_RATE_TABLE.get(slot)?;
    if e.weapon_def >= 0 {
        Some(e.weapon_def)
    } else {
        None
    }
}

pub fn known_complete_rate_timer_offset(slot: usize) -> Option<i32> {
    let e = ANIM_RATE_TABLE.get(slot)?;
    if e.complete_def >= 0 {
        Some(e.complete_def)
    } else {
        None
    }
}
