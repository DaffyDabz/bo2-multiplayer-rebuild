use std::sync::Arc;

use asset_anim::{ActiveAnim, ClipScheduler};
use asset_game::{
    ACTION_GOAL_TIME_SECS, ACTIVE_GOAL_WEIGHT, AdsOverlayConvention, IDLE_INTERRUPT_GOAL_TIME_SECS,
    INACTIVE_GOAL_WEIGHT, WEAPON_ANIM_SLOTS, WeaponAnimSlot, WeaponAnimations, playback_rate,
    slot_uses_native_rate,
};
use asset_iw4::size::WEAPON_ANIM_COUNT;

const DISPATCH_SLOT_START: usize = 1;
const DISPATCH_SLOT_END: usize = 0x22;

fn dispatch_slots() -> impl Iterator<Item = usize> {
    // bo2zm: the climb camera animation is posed from the climb, not played.
    (DISPATCH_SLOT_START..=DISPATCH_SLOT_END)
        .chain(WEAPON_ANIM_COUNT..WEAPON_ANIM_SLOTS)
        .filter(|&slot| slot != WeaponAnimSlot::CameraMantle.index())
}

/// bo2zm: Black Ops II's dive animations, full then empty magazine: take-off,
/// in the air, touch-down.
const DIVE_SLOTS: [[WeaponAnimSlot; 3]; 2] = [
    [
        WeaponAnimSlot::DiveIn,
        WeaponAnimSlot::DiveLoop,
        WeaponAnimSlot::DiveOut,
    ],
    [
        WeaponAnimSlot::DiveInEmpty,
        WeaponAnimSlot::DiveLoopEmpty,
        WeaponAnimSlot::DiveOutEmpty,
    ],
];

/// bo2zm: aiming mid-dive brings the gun up this quickly.
const DIVE_INTERRUPT_GOAL_TIME_SECS: f32 = 0.1;

/// bo2zm: Black Ops II's crawl animations, full then empty magazine: in, the
/// four directions (forward, back, right, left), out.
const CRAWL_SLOTS: [[WeaponAnimSlot; 6]; 2] = [
    [
        WeaponAnimSlot::CrawlIn,
        WeaponAnimSlot::CrawlForward,
        WeaponAnimSlot::CrawlBack,
        WeaponAnimSlot::CrawlRight,
        WeaponAnimSlot::CrawlLeft,
        WeaponAnimSlot::CrawlOut,
    ],
    [
        WeaponAnimSlot::CrawlInEmpty,
        WeaponAnimSlot::CrawlForwardEmpty,
        WeaponAnimSlot::CrawlBackEmpty,
        WeaponAnimSlot::CrawlRightEmpty,
        WeaponAnimSlot::CrawlLeftEmpty,
        WeaponAnimSlot::CrawlOutEmpty,
    ],
];
const CRAWL_IN_PART: usize = 0;
const CRAWL_OUT_PART: usize = 5;

/// bo2zm: a new crawl direction blends in over this long (designed by feel
/// until real Black Ops II is read).
const CRAWL_TURN_GOAL_TIME_SECS: f32 = 0.15;

/// bo2zm: leaving a crawl blends over the gun's crawl out-fire time, or this
/// when the gun has none.
const CRAWL_OUT_FIRE_DEFAULT_SECS: f32 = 0.05;

/// A crawl slot's (empty magazine, part: 0 in, 1-4 a direction, 5 out).
fn crawl_part(slot: WeaponAnimSlot) -> Option<(bool, usize)> {
    CRAWL_SLOTS.iter().enumerate().find_map(|(empty, parts)| {
        parts
            .iter()
            .position(|&s| s == slot)
            .map(|part| (empty == 1, part))
    })
}

/// bo2zm: the full-magazine sprint or crawl animation an empty-magazine one
/// stands in for.
fn full_magazine_slot(slot: WeaponAnimSlot) -> Option<WeaponAnimSlot> {
    match slot {
        WeaponAnimSlot::SprintInEmpty => Some(WeaponAnimSlot::SprintIn),
        WeaponAnimSlot::SprintLoopEmpty => Some(WeaponAnimSlot::SprintLoop),
        WeaponAnimSlot::SprintOutEmpty => Some(WeaponAnimSlot::SprintOut),
        WeaponAnimSlot::MeleeEmpty => Some(WeaponAnimSlot::Melee),
        WeaponAnimSlot::MeleeChargeEmpty => Some(WeaponAnimSlot::MeleeCharge),
        // A gun without a first-shots fire plays its fire.
        WeaponAnimSlot::FireIntro => Some(WeaponAnimSlot::Fire),
        WeaponAnimSlot::AdsFireIntro => Some(WeaponAnimSlot::AdsFire),
        _ => crawl_part(slot)
            .filter(|&(empty, _)| empty)
            .map(|(_, part)| CRAWL_SLOTS[0][part]),
    }
}

/// A dive slot's (empty magazine, part: 0 take-off, 1 in the air, 2 touch-down).
fn dive_part(slot: WeaponAnimSlot) -> Option<(bool, usize)> {
    DIVE_SLOTS.iter().enumerate().find_map(|(empty, parts)| {
        parts
            .iter()
            .position(|&s| s == slot)
            .map(|part| (empty == 1, part))
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewmodelEvent {
    Fire,

    Raise { first: bool, quick: bool },
    Reload { empty: bool },
    ReloadStart,
    ReloadEnd,
    Rechamber,
    Drop,
    SprintIn,
    SprintLoop,
    SprintOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeaponState {
    Ready,
    Raising { first: bool, quick: bool },
    Firing,
    Reloading { empty: bool },
    ReloadStarting,
    ReloadEnding,
    Rechambering,
    Dropping,
    SprintIn,
    SprintLoop,
    SprintOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventResult {
    Started(WeaponState),
    IgnoredState(WeaponState),
    IgnoredMissingClip(WeaponAnimSlot),
}

#[derive(Debug, Clone, Default)]
pub struct AdvanceResult {
    pub notifies: Vec<String>,
}

#[derive(Debug)]
pub struct ViewmodelController {
    weapon: WeaponAnimations,
    tree: ClipScheduler,
    state: WeaponState,
    action: Option<WeaponAnimSlot>,

    action_remaining: Option<f32>,

    last_ads_frac: f32,

    predicted_perks0: u32,

    /// bo2zm fix list 2: the trigger came up while a Black Ops II fire
    /// animation still played; it settles to idle when that animation ends.
    settle_when_done: bool,

    /// bo2zm: the crawl direction to play when the crawl-in ends.
    crawl_next: Option<WeaponAnimSlot>,
}

impl ViewmodelController {
    pub fn new(weapon: WeaponAnimations) -> Self {
        let mut tree = ClipScheduler::new(WEAPON_ANIM_SLOTS);
        weapon
            .install_clips(&mut tree)
            .expect("weapon animation index is within tree");

        let mut this = Self {
            weapon,
            tree,
            state: WeaponState::Ready,
            action: None,
            action_remaining: None,
            last_ads_frac: 0.0,
            predicted_perks0: 0,
            settle_when_done: false,
            crawl_next: None,
        };
        this.set_weight(
            WeaponAnimSlot::Idle,
            ACTIVE_GOAL_WEIGHT,
            ACTION_GOAL_TIME_SECS,
        );

        // Bones animated only by ADS clips still need their lowered pose at the hip.
        if let Some(clip) = this.weapon.clip(WeaponAnimSlot::AdsDown).cloned() {
            this.set_weight(WeaponAnimSlot::AdsDown, ACTIVE_GOAL_WEIGHT, 0.0);
            this.tree
                .set_time(WeaponAnimSlot::AdsDown.index(), clip.duration())
                .expect("ads down index is within tree");
            this.tree
                .set_rate(WeaponAnimSlot::AdsDown.index(), 0.0)
                .expect("ads down index is within tree");
        } else if this.weapon.ads_overlay == AdsOverlayConvention::PlayAdsAnim
            && this.weapon.clip(WeaponAnimSlot::AdsUp).is_some()
        {
            this.set_weight(WeaponAnimSlot::AdsUp, ACTIVE_GOAL_WEIGHT, 0.0);
        }
        this
    }

    pub fn state(&self) -> WeaponState {
        self.state
    }

    pub fn set_predicted_perks(&mut self, perks0: u32) {
        self.predicted_perks0 = perks0;
    }

    pub fn weapon_name(&self) -> &str {
        &self.weapon.name
    }

    pub fn weapon(&self) -> &WeaponAnimations {
        &self.weapon
    }

    pub fn active_anims(&self) -> impl Iterator<Item = ActiveAnim<'_>> {
        self.tree.active()
    }

    pub fn animation_weight(&self, slot: WeaponAnimSlot) -> f32 {
        self.tree.weight(slot.index()).unwrap_or(0.0)
    }

    pub fn animation_time(&self, slot: WeaponAnimSlot) -> f32 {
        self.tree.time(slot.index()).unwrap_or(0.0)
    }

    pub fn handle(&mut self, event: ViewmodelEvent) -> EventResult {
        if !self.event_allowed(event) {
            return EventResult::IgnoredState(self.state);
        }
        match event {
            ViewmodelEvent::Fire => {
                let slot = if self.last_ads_frac > 0.0
                    && self.weapon.clip(WeaponAnimSlot::AdsFire).is_some()
                {
                    WeaponAnimSlot::AdsFire
                } else {
                    WeaponAnimSlot::Fire
                };
                let timer = self.weapon.fire_time_ms;
                self.start_action(
                    slot,
                    WeaponState::Firing,
                    if timer > 0 { Some(timer) } else { None },
                )
            }
            ViewmodelEvent::Raise { first, quick } => {
                let slot = match (first, quick) {
                    (true, _) => WeaponAnimSlot::FirstRaise,
                    (false, true) => WeaponAnimSlot::QuickRaise,
                    (false, false) => WeaponAnimSlot::Raise,
                };

                let timer = match (first, quick) {
                    (false, false) => Some(self.weapon.raise_time_ms),
                    (false, true) => positive_ms(self.weapon.quick_raise_time_ms),
                    (true, _) => None,
                };
                self.start_action(slot, WeaponState::Raising { first, quick }, timer)
            }
            ViewmodelEvent::Reload { empty } => {
                let slot = if empty {
                    WeaponAnimSlot::ReloadEmpty
                } else {
                    WeaponAnimSlot::Reload
                };
                let timer = if empty {
                    positive_ms(self.weapon.reload_empty_time_ms)
                } else {
                    positive_ms(self.weapon.reload_time_ms)
                };
                self.start_action(slot, WeaponState::Reloading { empty }, timer)
            }
            ViewmodelEvent::ReloadStart => self.start_action(
                WeaponAnimSlot::ReloadStart,
                WeaponState::ReloadStarting,
                positive_ms(self.weapon.reload_start_time_ms),
            ),
            ViewmodelEvent::ReloadEnd => self.start_action(
                WeaponAnimSlot::ReloadEnd,
                WeaponState::ReloadEnding,
                positive_ms(self.weapon.reload_end_time_ms),
            ),
            ViewmodelEvent::Rechamber => {
                self.start_action(WeaponAnimSlot::Rechamber, WeaponState::Rechambering, None)
            }
            ViewmodelEvent::Drop => self.start_action(
                WeaponAnimSlot::Drop,
                WeaponState::Dropping,
                positive_ms(self.weapon.drop_time_ms),
            ),
            ViewmodelEvent::SprintIn => self.start_action(
                WeaponAnimSlot::SprintIn,
                WeaponState::SprintIn,
                positive_ms(self.weapon.sprint_raise_time_ms),
            ),
            ViewmodelEvent::SprintLoop => self.start_action(
                WeaponAnimSlot::SprintLoop,
                WeaponState::SprintLoop,
                positive_ms(self.weapon.sprint_loop_time_ms),
            ),
            ViewmodelEvent::SprintOut => self.start_action(
                WeaponAnimSlot::SprintOut,
                WeaponState::SprintOut,
                positive_ms(self.weapon.sprint_drop_time_ms),
            ),
        }
    }

    pub fn dispatch_sz_xanim_index(&mut self, slot_index: usize) -> EventResult {
        if slot_index == WeaponAnimSlot::AdsUp.index()
            || slot_index == WeaponAnimSlot::AdsDown.index()
            || slot_index == WeaponAnimSlot::CameraMantle.index()
        {
            return EventResult::IgnoredState(self.state);
        }
        let Some(slot) = WeaponAnimSlot::from_index(slot_index) else {
            return EventResult::IgnoredState(self.state);
        };
        if matches!(slot, WeaponAnimSlot::Idle | WeaponAnimSlot::EmptyIdle) {
            self.start_idle();
            return EventResult::Started(WeaponState::Ready);
        }
        // bo2zm: a gun without Black Ops II's dive animations comes straight up.
        if let Some((empty, _)) = dive_part(slot)
            && self.weapon.clip(slot).is_none()
        {
            self.start_idle_family(empty, true);
            return EventResult::Started(WeaponState::Ready);
        }
        if let Some((empty, part)) = crawl_part(slot) {
            return self.dispatch_crawl(self.clip_or_full(slot), empty, part);
        }
        let slot = self.clip_or_full(slot);
        let (state, timer) = self.state_and_timer_for_slot(slot);
        self.start_action(slot, state, timer)
    }

    /// bo2zm: this slot, or its full-magazine one when the gun has no
    /// empty-magazine animation for it.
    fn clip_or_full(&self, slot: WeaponAnimSlot) -> WeaponAnimSlot {
        match full_magazine_slot(slot) {
            Some(full) if self.weapon.clip(slot).is_none() => full,
            _ => slot,
        }
    }

    /// bo2zm: Black Ops II's crawl. A direction from anything else plays the
    /// crawl-in first, then the direction the player crawls by then; a new
    /// direction blends across; the crawl-out settles to idle. A gun without
    /// the animation keeps what it plays.
    fn dispatch_crawl(&mut self, slot: WeaponAnimSlot, empty: bool, part: usize) -> EventResult {
        if self.weapon.clip(slot).is_none() {
            return EventResult::Started(self.state);
        }
        let current = self.action.and_then(crawl_part);
        if part == CRAWL_OUT_PART {
            if current.is_none() {
                return EventResult::IgnoredState(self.state);
            }
            let goal = self.crawl_out_fire_secs();
            return self.start_crawl(slot, goal);
        }
        match current {
            Some((_, CRAWL_IN_PART)) => {
                self.crawl_next = Some(slot);
                EventResult::Started(self.state)
            }
            Some(_) => self.start_crawl(slot, CRAWL_TURN_GOAL_TIME_SECS),
            None => {
                let crawl_in = self.clip_or_full(CRAWL_SLOTS[usize::from(empty)][CRAWL_IN_PART]);
                if self.weapon.clip(crawl_in).is_none() {
                    return self.start_crawl(slot, CRAWL_TURN_GOAL_TIME_SECS);
                }
                let result = self.start_crawl(crawl_in, ACTION_GOAL_TIME_SECS);
                self.crawl_next = Some(slot);
                result
            }
        }
    }

    fn start_crawl(&mut self, slot: WeaponAnimSlot, goal_time: f32) -> EventResult {
        let (state, timer) = self.state_and_timer_for_slot(slot);
        self.start_action_over(slot, state, timer, goal_time)
    }

    /// bo2zm: a crawl-in or crawl direction plays (not the crawl-out).
    fn crawling(&self) -> bool {
        self.action
            .and_then(crawl_part)
            .is_some_and(|(_, part)| part != CRAWL_OUT_PART)
    }

    fn crawl_out_fire_secs(&self) -> f32 {
        positive_ms(self.weapon.crawl_times_ms[5])
            .map_or(CRAWL_OUT_FIRE_DEFAULT_SECS, |ms| ms as f32 / 1000.0)
    }

    fn state_and_timer_for_slot(&self, slot: WeaponAnimSlot) -> (WeaponState, Option<i32>) {
        match slot {
            WeaponAnimSlot::Fire
            | WeaponAnimSlot::AdsFire
            | WeaponAnimSlot::LastShot
            | WeaponAnimSlot::AdsLastShot
            | WeaponAnimSlot::HoldFire => {
                let timer = self.weapon.fire_time_ms;
                (
                    WeaponState::Firing,
                    if timer > 0 { Some(timer) } else { None },
                )
            }
            WeaponAnimSlot::FireIntro | WeaponAnimSlot::AdsFireIntro => (
                WeaponState::Firing,
                positive_ms(self.weapon.intro_fire_time_ms),
            ),
            WeaponAnimSlot::FirstRaise => (
                WeaponState::Raising {
                    first: true,
                    quick: false,
                },
                None,
            ),
            WeaponAnimSlot::QuickRaise => (
                WeaponState::Raising {
                    first: false,
                    quick: true,
                },
                positive_ms(self.weapon.quick_raise_time_ms),
            ),

            WeaponAnimSlot::AltRaise => (
                WeaponState::Raising {
                    first: false,
                    quick: false,
                },
                positive_ms(self.weapon.alternate_raise_time_ms),
            ),
            WeaponAnimSlot::EmptyRaise => (
                WeaponState::Raising {
                    first: false,
                    quick: false,
                },
                None,
            ),
            WeaponAnimSlot::Raise => (
                WeaponState::Raising {
                    first: false,
                    quick: false,
                },
                Some(self.weapon.raise_time_ms),
            ),
            WeaponAnimSlot::Reload => (
                WeaponState::Reloading { empty: false },
                positive_ms(self.weapon.reload_time_ms),
            ),
            WeaponAnimSlot::ReloadEmpty => (
                WeaponState::Reloading { empty: true },
                positive_ms(self.weapon.reload_empty_time_ms),
            ),
            WeaponAnimSlot::ReloadQuick => (
                WeaponState::Reloading { empty: false },
                positive_ms(self.weapon.reload_quick_time_ms),
            ),
            WeaponAnimSlot::ReloadQuickEmpty => (
                WeaponState::Reloading { empty: true },
                positive_ms(self.weapon.reload_quick_empty_time_ms),
            ),
            WeaponAnimSlot::ReloadStart => (
                WeaponState::ReloadStarting,
                positive_ms(self.weapon.reload_start_time_ms),
            ),
            WeaponAnimSlot::ReloadEnd => (
                WeaponState::ReloadEnding,
                positive_ms(self.weapon.reload_end_time_ms),
            ),
            WeaponAnimSlot::Rechamber | WeaponAnimSlot::AdsRechamber => {
                (WeaponState::Rechambering, None)
            }
            WeaponAnimSlot::Drop => (WeaponState::Dropping, positive_ms(self.weapon.drop_time_ms)),
            WeaponAnimSlot::QuickDrop => (
                WeaponState::Dropping,
                positive_ms(self.weapon.quick_drop_time_ms),
            ),

            WeaponAnimSlot::AltDrop => (
                WeaponState::Dropping,
                positive_ms(self.weapon.alternate_drop_time_ms),
            ),
            WeaponAnimSlot::EmptyDrop => (WeaponState::Dropping, None),
            WeaponAnimSlot::SprintIn => (
                WeaponState::SprintIn,
                positive_ms(self.weapon.sprint_raise_time_ms),
            ),
            WeaponAnimSlot::SprintLoop => (
                WeaponState::SprintLoop,
                positive_ms(self.weapon.sprint_loop_time_ms),
            ),
            WeaponAnimSlot::SprintOut => (
                WeaponState::SprintOut,
                positive_ms(self.weapon.sprint_drop_time_ms),
            ),
            WeaponAnimSlot::SprintInEmpty => (
                WeaponState::SprintIn,
                positive_ms(self.weapon.sprint_raise_time_ms),
            ),
            WeaponAnimSlot::SprintLoopEmpty => (
                WeaponState::SprintLoop,
                positive_ms(self.weapon.sprint_loop_time_ms),
            ),
            WeaponAnimSlot::SprintOutEmpty => (
                WeaponState::SprintOut,
                positive_ms(self.weapon.sprint_drop_time_ms),
            ),
            WeaponAnimSlot::CrawlIn | WeaponAnimSlot::CrawlInEmpty => (
                WeaponState::Ready,
                positive_ms(self.weapon.crawl_times_ms[0]),
            ),
            WeaponAnimSlot::CrawlForward | WeaponAnimSlot::CrawlForwardEmpty => (
                WeaponState::Ready,
                positive_ms(self.weapon.crawl_times_ms[1]),
            ),
            WeaponAnimSlot::CrawlBack | WeaponAnimSlot::CrawlBackEmpty => (
                WeaponState::Ready,
                positive_ms(self.weapon.crawl_times_ms[2]),
            ),
            WeaponAnimSlot::CrawlRight | WeaponAnimSlot::CrawlRightEmpty => (
                WeaponState::Ready,
                positive_ms(self.weapon.crawl_times_ms[3]),
            ),
            WeaponAnimSlot::CrawlLeft | WeaponAnimSlot::CrawlLeftEmpty => (
                WeaponState::Ready,
                positive_ms(self.weapon.crawl_times_ms[4]),
            ),
            WeaponAnimSlot::CrawlOut | WeaponAnimSlot::CrawlOutEmpty => (
                WeaponState::Ready,
                positive_ms(self.weapon.crawl_times_ms[6]),
            ),
            WeaponAnimSlot::Melee | WeaponAnimSlot::MeleeEmpty => {
                (WeaponState::Ready, positive_ms(self.weapon.melee_time_ms))
            }
            WeaponAnimSlot::MeleeCharge | WeaponAnimSlot::MeleeChargeEmpty => (
                WeaponState::Ready,
                positive_ms(self.weapon.melee_charge_time_ms),
            ),
            WeaponAnimSlot::Idle | WeaponAnimSlot::EmptyIdle => (WeaponState::Ready, None),
            WeaponAnimSlot::AdsUp | WeaponAnimSlot::AdsDown => (WeaponState::Ready, None),
            WeaponAnimSlot::CameraMantle => (WeaponState::Ready, None),
            WeaponAnimSlot::DiveIn | WeaponAnimSlot::DiveInEmpty => (
                WeaponState::Ready,
                positive_ms(self.weapon.dive_times_ms[0]),
            ),
            WeaponAnimSlot::DiveLoop | WeaponAnimSlot::DiveLoopEmpty => (
                WeaponState::Ready,
                positive_ms(self.weapon.dive_times_ms[1]),
            ),
            WeaponAnimSlot::DiveOut | WeaponAnimSlot::DiveOutEmpty => (
                WeaponState::Ready,
                positive_ms(self.weapon.dive_times_ms[2]),
            ),
        }
    }

    pub fn advance(&mut self, dt_secs: f32) -> AdvanceResult {
        let dt_secs = dt_secs.max(0.0);
        let notifies = self.tree.advance(dt_secs);

        if let Some(remaining) = self.action_remaining.as_mut() {
            *remaining = (*remaining - dt_secs).max(0.0);
        }

        // bo2zm: Black Ops II's dive: take-off runs into the in-air
        // animation, which holds its last frame until touch-down; touch-down
        // settles to idle.
        if let Some(slot) = self.action
            && let Some((empty, part)) = dive_part(slot)
            && part != 1
            && self.action_finished(slot)
        {
            if part == 2 {
                self.start_idle_family(empty, false);
            } else {
                let next = DIVE_SLOTS[usize::from(empty)][1];
                if self.weapon.clip(next).is_some() {
                    let (state, timer) = self.state_and_timer_for_slot(next);
                    self.start_action(next, state, timer);
                }
            }
        }

        // bo2zm: Black Ops II's crawl: the crawl-in runs into the direction
        // the player crawls; the crawl-out settles to idle.
        if let Some(slot) = self.action
            && let Some((empty, part)) = crawl_part(slot)
            && matches!(part, CRAWL_IN_PART | CRAWL_OUT_PART)
            && self.action_finished(slot)
        {
            match self.crawl_next.take() {
                Some(next) if part == CRAWL_IN_PART => {
                    self.start_crawl(next, ACTION_GOAL_TIME_SECS);
                }
                _ => self.start_idle_family(empty, false),
            }
        }

        if matches!(self.state, WeaponState::Firing)
            && let Some(slot) = self.action
            && self.action_finished(slot)
        {
            self.finish_fire_cycle();
        }
        // bo2zm fix list 2: a played-out fire animation settles to idle.
        if self.settle_when_done {
            if !matches!(self.state, WeaponState::Ready) || self.action.is_some() {
                self.settle_when_done = false;
            } else if !self.dispatch_range_unfinished() {
                self.settle_when_done = false;
                if anim_log() {
                    diag::info!(
                        Fpv,
                        "fire anim played out: fire clip at {:.3} s, settling to idle",
                        self.animation_time(WeaponAnimSlot::Fire)
                    );
                }
                self.start_idle();
            }
        }

        AdvanceResult { notifies }
    }

    pub fn apply_idle_weap_anim(&mut self, empty_mag: bool) -> bool {
        // bo2zm: a crawl loops; the idle edge ends it (quickly) instead of
        // waiting for it.
        if self.crawling() {
            self.start_idle_family(empty_mag, true);
            return true;
        }
        if self.dispatch_range_unfinished() {
            return false;
        }
        self.start_idle_family(empty_mag, false);
        true
    }

    pub fn apply_force_idle_weap_anim(&mut self, empty_mag: bool) {
        if self.action.is_some_and(|slot| dive_part(slot).is_some()) {
            self.start_idle_family_over(empty_mag, DIVE_INTERRUPT_GOAL_TIME_SECS);
            return;
        }
        // bo2zm: aiming out of a crawl brings the gun up over the gun's crawl
        // out-fire time.
        if self.action.is_some_and(|slot| crawl_part(slot).is_some()) {
            let goal = self.crawl_out_fire_secs();
            self.start_idle_family_over(empty_mag, goal);
            return;
        }
        let interrupt = self.dispatch_range_unfinished();
        self.start_idle_family(empty_mag, interrupt);
    }

    pub fn settle_fire_to_idle(&mut self) {
        if !matches!(self.state, WeaponState::Ready) || self.action.is_some() {
            return;
        }
        // bo2zm fix list 2: a Black Ops II hip fire animation (its recoil)
        // runs past the gun's fire time; it plays out, and the idle edge
        // takes over when it ends (as the aimed one always did).
        if self.weapon.fire_plays_out && self.dispatch_range_unfinished() {
            if anim_log() && !self.settle_when_done {
                diag::info!(
                    Fpv,
                    "trigger up: fire clip at {:.3} s of {:.3} s plays out",
                    self.animation_time(WeaponAnimSlot::Fire),
                    self.weapon
                        .clip(WeaponAnimSlot::Fire)
                        .map_or(0.0, |c| c.duration())
                );
            }
            self.settle_when_done = true;
            return;
        }
        if self.animation_weight(WeaponAnimSlot::Fire) > 0.01
            || self.animation_weight(WeaponAnimSlot::FireIntro) > 0.01
        {
            self.start_idle();
        }
    }

    fn event_allowed(&self, event: ViewmodelEvent) -> bool {
        match event {
            ViewmodelEvent::Fire => {
                matches!(self.state, WeaponState::Ready | WeaponState::Firing)
            }

            ViewmodelEvent::SprintLoop => !matches!(self.state, WeaponState::SprintLoop),
            ViewmodelEvent::SprintIn => {
                matches!(
                    self.state,
                    WeaponState::Ready | WeaponState::Firing | WeaponState::SprintOut
                )
            }

            ViewmodelEvent::Raise { .. }
            | ViewmodelEvent::Reload { .. }
            | ViewmodelEvent::ReloadStart
            | ViewmodelEvent::ReloadEnd
            | ViewmodelEvent::Rechamber
            | ViewmodelEvent::Drop
            | ViewmodelEvent::SprintOut => true,
        }
    }

    pub fn apply_ads_overlay_frame(&mut self, f_weapon_pos_frac: f32) {
        let scrub = weapon_iw4::ads_overlay_scrub(f_weapon_pos_frac);
        self.last_ads_frac = scrub.ads_up_weight;
        if matches!(
            self.action,
            Some(
                WeaponAnimSlot::Melee
                    | WeaponAnimSlot::MeleeCharge
                    | WeaponAnimSlot::MeleeEmpty
                    | WeaponAnimSlot::MeleeChargeEmpty
            )
        ) {
            self.set_weight(WeaponAnimSlot::AdsUp, INACTIVE_GOAL_WEIGHT, 0.0);
            self.set_weight(WeaponAnimSlot::AdsDown, INACTIVE_GOAL_WEIGHT, 0.0);
            return;
        }
        const OVERLAY_SCRUB_RATE: f32 = 0.0;
        let aiming = f_weapon_pos_frac > 0.0;
        let has_ads_down = self.weapon.clip(WeaponAnimSlot::AdsDown).is_some();
        if let Some(clip) = self.weapon.clip(WeaponAnimSlot::AdsUp).cloned() {
            let time = clip.duration() * scrub.ads_up_time_norm;
            let ads_up_weight = match self.weapon.ads_overlay {
                AdsOverlayConvention::WeightIsFrac => scrub.ads_up_weight,
                AdsOverlayConvention::PlayAdsAnim => {
                    if has_ads_down && !aiming {
                        INACTIVE_GOAL_WEIGHT
                    } else {
                        ACTIVE_GOAL_WEIGHT
                    }
                }
            };
            self.tree
                .set_rate(WeaponAnimSlot::AdsUp.index(), OVERLAY_SCRUB_RATE)
                .expect("ads up index is within tree");
            self.tree
                .set_time(WeaponAnimSlot::AdsUp.index(), time)
                .expect("ads up index is within tree");
            self.tree
                .set_goal_weight(
                    WeaponAnimSlot::AdsUp.index(),
                    ads_up_weight,
                    ACTION_GOAL_TIME_SECS,
                )
                .expect("ads up index is within tree");
        }
        if let Some(clip) = self.weapon.clip(WeaponAnimSlot::AdsDown).cloned() {
            let time = clip.duration() * scrub.ads_down_time_norm;
            self.tree
                .set_rate(WeaponAnimSlot::AdsDown.index(), OVERLAY_SCRUB_RATE)
                .expect("ads down index is within tree");
            self.tree
                .set_time(WeaponAnimSlot::AdsDown.index(), time)
                .expect("ads down index is within tree");
            if self.weapon.ads_overlay == AdsOverlayConvention::PlayAdsAnim {
                let ads_down_weight = if aiming {
                    INACTIVE_GOAL_WEIGHT
                } else {
                    ACTIVE_GOAL_WEIGHT
                };
                self.tree
                    .set_goal_weight(
                        WeaponAnimSlot::AdsDown.index(),
                        ads_down_weight,
                        ACTION_GOAL_TIME_SECS,
                    )
                    .expect("ads down index is within tree");
            }
        }
    }

    fn perk_scaled_reload_timer(&self, state: WeaponState, timer_ms: Option<i32>) -> Option<i32> {
        let reload_family = matches!(
            state,
            WeaponState::Reloading { .. } | WeaponState::ReloadStarting | WeaponState::ReloadEnding
        );
        if !(reload_family
            && weapon_iw4::perk_fastreload_eligible(
                self.predicted_perks0,
                self.weapon.inherits_perks,
            ))
        {
            return timer_ms;
        }
        let m = weapon_iw4::PERK_WEAP_RELOAD_MULTIPLIER_DEFAULT;
        if m <= 0.0 {
            return timer_ms;
        }
        timer_ms.map(|t| ((t as f32) * m).round() as i32)
    }

    fn start_action(
        &mut self,
        slot: WeaponAnimSlot,
        state: WeaponState,
        timer_ms: Option<i32>,
    ) -> EventResult {
        // bo2zm: a shot (or anything else) out of a crawl blends from it over
        // the gun's crawl out-fire time.
        let goal_time = if self.crawling() {
            self.crawl_out_fire_secs()
        } else {
            ACTION_GOAL_TIME_SECS
        };
        self.start_action_over(slot, state, timer_ms, goal_time)
    }

    fn start_action_over(
        &mut self,
        slot: WeaponAnimSlot,
        state: WeaponState,
        timer_ms: Option<i32>,
        goal_time: f32,
    ) -> EventResult {
        self.crawl_next = None;
        let Some(clip) = self.weapon.clip(slot).cloned() else {
            self.zero_dispatch_slots(goal_time);
            self.action = None;
            self.action_remaining = None;
            self.state = state;
            return EventResult::IgnoredMissingClip(slot);
        };
        let duration = clip.duration();
        let timer_ms = self.perk_scaled_reload_timer(state, timer_ms);
        self.zero_dispatch_slots(goal_time);
        self.tree
            .set_clip(slot.index(), Arc::clone(&clip))
            .expect("action index is within tree");
        let rate = if slot_uses_native_rate(slot.index()) {
            1.0
        } else {
            timer_ms
                .filter(|time| *time > 0)
                .and_then(|time| {
                    let len_ms = (duration * 1000.0).round() as i32;
                    Some(playback_rate(len_ms, time))
                })
                .unwrap_or(1.0)
        };
        self.tree
            .set_rate(slot.index(), rate)
            .expect("action index is within tree");
        self.tree
            .set_goal_weight(slot.index(), ACTIVE_GOAL_WEIGHT, goal_time)
            .expect("action index is within tree");
        self.action = Some(slot);

        self.action_remaining = if clip.looping {
            None
        } else {
            timer_ms
                .filter(|time| *time > 0)
                .map(|time| time as f32 / 1000.0)
        };
        self.state = state;
        EventResult::Started(state)
    }

    fn start_idle(&mut self) {
        self.start_idle_family(false, false);
    }

    fn start_idle_family(&mut self, empty: bool, interrupt: bool) {
        let goal_time = if interrupt {
            IDLE_INTERRUPT_GOAL_TIME_SECS
        } else {
            ACTION_GOAL_TIME_SECS
        };
        self.start_idle_family_over(empty, goal_time);
    }

    fn start_idle_family_over(&mut self, empty: bool, goal_time: f32) {
        let slot = if empty && self.weapon.clip(WeaponAnimSlot::EmptyIdle).is_some() {
            WeaponAnimSlot::EmptyIdle
        } else {
            WeaponAnimSlot::Idle
        };
        self.action = None;
        self.action_remaining = None;
        self.crawl_next = None;
        self.state = WeaponState::Ready;
        self.zero_dispatch_slots(goal_time);
        self.set_weight(slot, ACTIVE_GOAL_WEIGHT, goal_time);
    }

    fn dispatch_range_unfinished(&self) -> bool {
        for node in dispatch_slots() {
            let weight = self.tree.weight(node).unwrap_or(0.0);
            if weight <= 0.0 {
                continue;
            }
            let Some(clip) = self.weapon.clip_at(node) else {
                continue;
            };
            if clip.looping {
                return true;
            }
            let time = self.tree.time(node).unwrap_or(0.0);
            if time < clip.duration() {
                return true;
            }
        }
        false
    }

    fn zero_dispatch_slots(&mut self, goal_time: f32) {
        for node in dispatch_slots() {
            self.tree
                .set_goal_weight(node, INACTIVE_GOAL_WEIGHT, goal_time)
                .expect("action index is within tree");
            self.tree
                .set_rate(node, 1.0)
                .expect("action index is within tree");
        }
    }

    fn finish_fire_cycle(&mut self) {
        self.action = None;
        self.action_remaining = None;
        self.state = WeaponState::Ready;
    }

    fn action_finished(&self, slot: WeaponAnimSlot) -> bool {
        if let Some(remaining) = self.action_remaining {
            return remaining <= 0.0;
        }
        let Some(clip) = self.weapon.clip(slot) else {
            return true;
        };
        !clip.looping && self.animation_time(slot) >= clip.duration()
    }

    fn set_weight(&mut self, slot: WeaponAnimSlot, weight: f32, goal_time: f32) {
        if self.weapon.clip(slot).is_some() {
            self.tree
                .set_goal_weight(slot.index(), weight, goal_time)
                .expect("verified animation index is within tree");
        }
    }
}

/// bo2zm test aid: `IW4L_ANIM_LOG` set = log the fire animation's settle.
fn anim_log() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("IW4L_ANIM_LOG").is_some())
}

fn positive_ms(ms: i32) -> Option<i32> {
    (ms > 0).then_some(ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use asset_anim::xanim_clip::{AnimClip, Translation};

    fn clip(name: &str, frames: u16, looping: bool) -> Arc<AnimClip> {
        Arc::new(AnimClip {
            name: name.to_owned(),
            framerate: 30.0,
            numframes: frames,
            looping,
            tracks: Vec::new(),
            notifies: Vec::new(),
            delta_translation: Translation::Default,
        })
    }

    /// The M14's dive as Black Ops II has it: 10, 15 and 10 frames played
    /// over 200, 600 and 250 ms.
    fn m14(dive_clips: bool) -> ViewmodelController {
        let mut gun = WeaponAnimations::empty("m14_zm")
            .with_clip(WeaponAnimSlot::Idle, clip("idle", 30, true));
        if dive_clips {
            gun = gun
                .with_clip(WeaponAnimSlot::DiveIn, clip("d2p_in", 10, false))
                .with_clip(WeaponAnimSlot::DiveLoop, clip("d2p_loop", 15, false))
                .with_clip(WeaponAnimSlot::DiveOut, clip("d2p_out", 10, false));
        }
        gun.dive_times_ms = [200, 600, 250];
        ViewmodelController::new(gun)
    }

    fn playing(c: &ViewmodelController, slot: WeaponAnimSlot) -> bool {
        c.animation_weight(slot) == ACTIVE_GOAL_WEIGHT
    }

    #[test]
    fn an_empty_knife_plays_its_own_animation_or_the_full_one() {
        let gun = WeaponAnimations::empty("fiveseven_zm")
            .with_clip(WeaponAnimSlot::Idle, clip("idle", 30, true))
            .with_clip(WeaponAnimSlot::Melee, clip("tactical_melee", 20, false));
        let mut c = ViewmodelController::new(gun.clone());
        c.dispatch_sz_xanim_index(WeaponAnimSlot::MeleeEmpty.index());
        c.advance(0.05);
        assert!(
            playing(&c, WeaponAnimSlot::Melee),
            "no empty one: the full one"
        );
        let gun = gun.with_clip(
            WeaponAnimSlot::MeleeEmpty,
            clip("tactical_melee_empty", 20, false),
        );
        let mut c = ViewmodelController::new(gun);
        c.dispatch_sz_xanim_index(WeaponAnimSlot::MeleeEmpty.index());
        c.advance(0.05);
        assert!(playing(&c, WeaponAnimSlot::MeleeEmpty));
        assert!(!playing(&c, WeaponAnimSlot::Melee));
    }

    #[test]
    fn a_dive_plays_take_off_then_holds_in_the_air_then_lands_to_idle() {
        let mut c = m14(true);
        c.dispatch_sz_xanim_index(WeaponAnimSlot::DiveIn.index());
        c.advance(0.15);
        assert!(playing(&c, WeaponAnimSlot::DiveIn));
        assert!(!playing(&c, WeaponAnimSlot::Idle));
        c.advance(0.1);
        assert!(playing(&c, WeaponAnimSlot::DiveLoop), "take-off ran into the air");
        assert!(!playing(&c, WeaponAnimSlot::DiveIn));
        c.advance(2.0);
        assert!(playing(&c, WeaponAnimSlot::DiveLoop), "held until touch-down");
        c.dispatch_sz_xanim_index(WeaponAnimSlot::DiveOut.index());
        c.advance(0.2);
        assert!(playing(&c, WeaponAnimSlot::DiveOut));
        c.advance(0.1);
        assert!(playing(&c, WeaponAnimSlot::Idle), "landed to idle");
        assert!(!playing(&c, WeaponAnimSlot::DiveOut));
        assert_eq!(c.state(), WeaponState::Ready);
    }

    #[test]
    fn aiming_mid_dive_brings_the_gun_up_quickly() {
        let mut c = m14(true);
        c.dispatch_sz_xanim_index(WeaponAnimSlot::DiveIn.index());
        c.advance(0.1);
        c.apply_force_idle_weap_anim(false);
        c.advance(0.1);
        assert!(playing(&c, WeaponAnimSlot::Idle));
        assert_eq!(c.animation_weight(WeaponAnimSlot::DiveIn), 0.0);
    }

    #[test]
    fn a_gun_without_dive_animations_stays_up() {
        let mut c = m14(false);
        assert_eq!(
            c.dispatch_sz_xanim_index(WeaponAnimSlot::DiveIn.index()),
            EventResult::Started(WeaponState::Ready)
        );
        c.advance(0.6);
        assert!(playing(&c, WeaponAnimSlot::Idle));
    }

    /// The 870's crawl as Black Ops II has it: in 10 frames over 300 ms,
    /// 30-frame direction loops over 1000-1200 ms, out 10 frames over
    /// 100 ms, out-fire 48 ms.
    fn r870(crawl_clips: bool) -> ViewmodelController {
        let mut gun = WeaponAnimations::empty("870mcs_zm")
            .with_clip(WeaponAnimSlot::Idle, clip("idle", 30, true))
            .with_clip(WeaponAnimSlot::Fire, clip("fire", 10, false));
        if crawl_clips {
            gun = gun
                .with_clip(WeaponAnimSlot::CrawlIn, clip("crawl_in", 10, false))
                .with_clip(WeaponAnimSlot::CrawlForward, clip("crawl_forward", 30, true))
                .with_clip(WeaponAnimSlot::CrawlLeft, clip("crawl_left", 30, true))
                .with_clip(WeaponAnimSlot::CrawlOut, clip("crawl_out", 10, false));
        }
        gun.crawl_times_ms = [300, 1100, 1000, 1200, 1100, 48, 100];
        ViewmodelController::new(gun)
    }

    #[test]
    fn a_crawl_plays_crawl_in_then_the_direction_then_out_to_idle() {
        let mut c = r870(true);
        c.dispatch_sz_xanim_index(WeaponAnimSlot::CrawlForward.index());
        c.advance(0.2);
        assert!(playing(&c, WeaponAnimSlot::CrawlIn));
        assert!(!playing(&c, WeaponAnimSlot::Idle));
        c.advance(0.15);
        assert!(playing(&c, WeaponAnimSlot::CrawlForward), "crawl-in ran into the crawl");
        assert!(!playing(&c, WeaponAnimSlot::CrawlIn));
        c.advance(3.0);
        assert!(playing(&c, WeaponAnimSlot::CrawlForward), "the crawl loops");
        c.dispatch_sz_xanim_index(WeaponAnimSlot::CrawlOut.index());
        c.advance(0.05);
        assert!(playing(&c, WeaponAnimSlot::CrawlOut));
        assert_eq!(c.animation_weight(WeaponAnimSlot::CrawlForward), 0.0);
        c.advance(0.06);
        assert!(playing(&c, WeaponAnimSlot::Idle), "out settled to idle");
        assert!(!playing(&c, WeaponAnimSlot::CrawlOut));
        // A crawl-out with no crawl before it does nothing.
        assert_eq!(
            c.dispatch_sz_xanim_index(WeaponAnimSlot::CrawlOut.index()),
            EventResult::IgnoredState(WeaponState::Ready)
        );
    }

    #[test]
    fn a_new_direction_blends_across_and_waits_for_the_crawl_in() {
        let mut c = r870(true);
        c.dispatch_sz_xanim_index(WeaponAnimSlot::CrawlForward.index());
        c.advance(0.1);
        c.dispatch_sz_xanim_index(WeaponAnimSlot::CrawlLeft.index());
        assert!(playing(&c, WeaponAnimSlot::CrawlIn), "the crawl-in plays on");
        c.advance(0.25);
        assert!(playing(&c, WeaponAnimSlot::CrawlLeft), "then the newest direction");
        assert_eq!(c.animation_weight(WeaponAnimSlot::CrawlForward), 0.0);
        c.dispatch_sz_xanim_index(WeaponAnimSlot::CrawlForward.index());
        c.advance(0.05);
        let blend = c.animation_weight(WeaponAnimSlot::CrawlForward);
        assert!(blend > 0.0 && blend < 1.0, "blending in, {blend}");
        c.advance(0.2);
        assert!(playing(&c, WeaponAnimSlot::CrawlForward));
        assert_eq!(c.animation_weight(WeaponAnimSlot::CrawlLeft), 0.0);
    }

    #[test]
    fn a_shot_or_aim_out_of_a_crawl_blends_over_the_out_fire_time() {
        let mut c = r870(true);
        c.dispatch_sz_xanim_index(WeaponAnimSlot::CrawlForward.index());
        c.advance(0.5);
        c.handle(ViewmodelEvent::Fire);
        c.advance(0.02);
        let blend = c.animation_weight(WeaponAnimSlot::Fire);
        assert!(blend > 0.0 && blend < 1.0, "blending in, {blend}");
        c.advance(0.04);
        assert!(playing(&c, WeaponAnimSlot::Fire));
        assert_eq!(c.animation_weight(WeaponAnimSlot::CrawlForward), 0.0);

        let mut c = r870(true);
        c.dispatch_sz_xanim_index(WeaponAnimSlot::CrawlForward.index());
        c.advance(0.5);
        c.apply_force_idle_weap_anim(false);
        c.advance(0.06);
        assert!(playing(&c, WeaponAnimSlot::Idle));
        assert_eq!(c.animation_weight(WeaponAnimSlot::CrawlForward), 0.0);

        let mut c = r870(true);
        c.dispatch_sz_xanim_index(WeaponAnimSlot::CrawlForward.index());
        c.advance(0.5);
        assert!(c.apply_idle_weap_anim(false), "the idle edge ends a crawl");
        c.advance(0.5);
        assert!(playing(&c, WeaponAnimSlot::Idle));
    }

    #[test]
    fn a_gun_without_crawl_animations_stays_as_it_is() {
        let mut c = r870(false);
        assert_eq!(
            c.dispatch_sz_xanim_index(WeaponAnimSlot::CrawlForward.index()),
            EventResult::Started(WeaponState::Ready)
        );
        c.advance(1.0);
        assert!(playing(&c, WeaponAnimSlot::Idle));
    }

    #[test]
    fn an_empty_gun_without_empty_animations_plays_the_full_ones() {
        let gun = WeaponAnimations::empty("ak74u_zm")
            .with_clip(WeaponAnimSlot::Idle, clip("idle", 30, true))
            .with_clip(WeaponAnimSlot::SprintLoop, clip("sprint_loop", 20, true))
            .with_clip(WeaponAnimSlot::CrawlIn, clip("crawl_in", 10, false))
            .with_clip(WeaponAnimSlot::CrawlBack, clip("crawl_back", 30, true));
        let mut c = ViewmodelController::new(gun);
        assert_eq!(
            c.dispatch_sz_xanim_index(WeaponAnimSlot::SprintLoopEmpty.index()),
            EventResult::Started(WeaponState::SprintLoop)
        );
        c.advance(0.1);
        assert!(playing(&c, WeaponAnimSlot::SprintLoop));
        c.dispatch_sz_xanim_index(WeaponAnimSlot::Idle.index());
        c.dispatch_sz_xanim_index(WeaponAnimSlot::CrawlBackEmpty.index());
        c.advance(0.1);
        assert!(playing(&c, WeaponAnimSlot::CrawlIn));
        c.advance(0.5);
        assert!(playing(&c, WeaponAnimSlot::CrawlBack));
    }

    #[test]
    fn the_first_shots_play_their_own_fire_or_the_gun_s_fire() {
        let hamr = WeaponAnimations::empty("hamr_mp")
            .with_clip(WeaponAnimSlot::Idle, clip("idle", 30, true))
            .with_clip(WeaponAnimSlot::Fire, clip("secondary_fire", 6, false))
            .with_clip(WeaponAnimSlot::FireIntro, clip("fire", 6, false));
        let mut c = ViewmodelController::new(hamr);
        assert_eq!(
            c.dispatch_sz_xanim_index(WeaponAnimSlot::FireIntro.index()),
            EventResult::Started(WeaponState::Firing)
        );
        c.advance(0.01);
        assert!(playing(&c, WeaponAnimSlot::FireIntro));

        let gun = WeaponAnimations::empty("an94_mp")
            .with_clip(WeaponAnimSlot::Idle, clip("idle", 30, true))
            .with_clip(WeaponAnimSlot::Fire, clip("fire", 6, false));
        let mut c = ViewmodelController::new(gun);
        assert_eq!(
            c.dispatch_sz_xanim_index(WeaponAnimSlot::FireIntro.index()),
            EventResult::Started(WeaponState::Firing)
        );
        c.advance(0.01);
        assert!(playing(&c, WeaponAnimSlot::Fire));
    }

    #[test]
    fn a_shot_mid_dive_replaces_the_dive() {
        let mut c = m14(true);
        c.dispatch_sz_xanim_index(WeaponAnimSlot::DiveIn.index());
        c.advance(0.1);
        assert_eq!(c.handle(ViewmodelEvent::Fire), EventResult::IgnoredMissingClip(WeaponAnimSlot::Fire));
        c.advance(0.5);
        assert!(!playing(&c, WeaponAnimSlot::DiveLoop), "no in-air animation after a shot");
    }
}
