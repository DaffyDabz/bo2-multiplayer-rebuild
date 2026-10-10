//! bo2zm: runs a player on a flat floor with the Black Ops II movement rules
//! and with the old IW4 ones, and checks what changes.

use movement_iw4::{
    AdsFracContext, AdsIntentContext, AirMoveContext, Bo2Feel, CmdScaleWalkContext,
    CollisionBackend, FlatMantleAnimLength, GroundTraceInput, JumpLaunchContext,
    MeleeChargeWeaponDelays, MoveBounds, PmoveSingleContext, SprintContext, ViewAngleClamp,
    WalkMoveContext, ZeroMantleRootDelta, pmove,
};
use playerstate_iw4::{PlayerState, UserCmd};
use trace_iw4::Trace;

const BO2: Bo2Feel = Bo2Feel {
    on: true,
    sprint_strafe_speed_scale: 0.667,
    jump_slowdown: true,
    ..Bo2Feel::IW4
};

/// A floor at z = 0 and nothing else.
struct Floor;

impl CollisionBackend for Floor {
    fn trace(&self, input: GroundTraceInput) -> Trace {
        let start = input.start[2] + input.mins[2];
        let end = input.end[2] + input.mins[2];
        let mut hit = Trace {
            fraction: 1.0,
            endpos: input.end,
            ..Trace::default()
        };
        if start < 0.0 {
            hit.startsolid = 1;
            hit.allsolid = u8::from(end < 0.0);
            hit.fraction = 0.0;
            hit.endpos = input.start;
            hit.normal = [0.0, 0.0, 1.0];
            return hit;
        }
        if end < 0.0 {
            let fraction = start / (start - end);
            hit.fraction = fraction;
            hit.normal = [0.0, 0.0, 1.0];
            hit.walkable = 1;
            hit.hit_type = 1;
            for i in 0..3 {
                hit.endpos[i] = input.start[i] + (input.end[i] - input.start[i]) * fraction;
            }
        }
        hit
    }
}

fn context(feel: Bo2Feel, old_buttons: u32, ads_allowed: bool) -> PmoveSingleContext {
    let air = AirMoveContext {
        player_spectate_speed_scale: 1.0,
        shellshock_gravity_scale: 1.0,
        shellshock_gravity_bias: 0.0,
    };
    PmoveSingleContext {
        walk: WalkMoveContext {
            cmd_scale: CmdScaleWalkContext {
                player_back_speed_scale: 0.7,
                player_strafe_speed_scale: 0.8,
                player_sprint_speed_scale: 1.5,
                player_last_stand_crawl_speed_scale: 0.15,
                weapon_move_speed_scale: 1.0,
                weapon_ads_move_speed_scale: 1.0,
                shellshock_affects_movement: false,
            },
            weapon_move_scale: 1.0,
            old_buttons,
            jump: JumpLaunchContext {
                jump_height: 39.0,
                dive: false,
                crouch_jump_scale: 1.0,
                jump_ladder_push_vel: 128.0,
            },
            air,
            feel,
        },
        air,
        bounds: MoveBounds {
            mins: [-15.0, -15.0, 0.0],
            maxs: [15.0, 15.0, 70.0],
            tracemask: 0x0281_0011,
        },
        view_angles: ViewAngleClamp {
            pitch_up: 85.0,
            pitch_down: 85.0,
            unclamped_pitch_bit: false,
        },
        sprint: SprintContext {
            weapon_max_sprint_time: 4000,
            sprint_forever: false,
            min_sprint_time_seconds: 1.0,
            sprint_delay_seconds: 0.0,
            sprint_forward_minimum: 105,
            stand_up_clear: true,
            sprint_recharge_pause_seconds: 0.0,
        },
        ads_intent: AdsIntentContext {
            ads_allowed,
            weapon_def_scope: false,
            sprint_hold_ads: false,
        },
        ads_frac: AdsFracContext {
            aim_down_sight: false,
            ads_in_rate: 1.0 / 200.0,
            ads_out_rate: 1.0 / 200.0,
            rechamber_while_ads: true,
            ads_fire_only: false,
        },
        melee_charge: MeleeChargeWeaponDelays {
            melee_delay_ms: 0,
            melee_charge_delay_ms: 0,
        },
        player_melee_range: movement_iw4::MELEE_CHARGE_PLAYER_MELEE_RANGE_DEFAULT,
        old_buttons,
        weapon_blocks_prone: false,
    }
}

struct Run {
    ps: PlayerState,
    time: i32,
    old_buttons: u32,
    feel: Bo2Feel,
    /// The gun in hand can aim.
    ads: bool,
}

impl Run {
    fn new(feel: Bo2Feel) -> Self {
        let mut ps = PlayerState::ZERO;
        ps.gravity = 800;
        ps.speed = 190;
        ps.move_speed_scale_multiplier = 1.0;
        ps.ground_entity_num = 1022;
        ps.view_height_target = 60;
        ps.view_height_current = 60.0;
        ps.command_time = 1000;
        Self {
            ps,
            time: 1000,
            old_buttons: 0,
            feel,
            ads: false,
        }
    }

    /// Runs `ms` of 50 ms frames holding these inputs.
    fn hold(&mut self, ms: i32, forward: i8, right: i8, buttons: u32) {
        let frames = ms / 50;
        for _ in 0..frames {
            self.time += 50;
            let mut cmd = UserCmd {
                server_time: self.time,
                forwardmove: forward,
                rightmove: right,
                buttons,
                ..UserCmd::default()
            };
            pmove(
                &mut self.ps,
                &mut cmd,
                context(self.feel, self.old_buttons, self.ads),
                &Floor,
                &FlatMantleAnimLength::default(),
                &ZeroMantleRootDelta,
            );
            self.old_buttons = buttons;
        }
    }

    fn ground_speed(&self) -> f32 {
        (self.ps.velocity[0].powi(2) + self.ps.velocity[1].powi(2)).sqrt()
    }

    fn sideways_speed(&self) -> f32 {
        self.ps.velocity[1].abs()
    }
}

const SPRINT: u32 = 0x2;
const JUMP: u32 = 0x400;

#[test]
fn sprint_strafe_is_slower_sideways_in_bo2() {
    let mut bo2 = Run::new(BO2);
    let mut iw4 = Run::new(Bo2Feel::IW4);
    for run in [&mut bo2, &mut iw4] {
        run.hold(1000, 127, 127, SPRINT);
    }
    eprintln!(
        "sprint+strafe sideways: bo2 {:.1} iw4 {:.1}",
        bo2.sideways_speed(),
        iw4.sideways_speed()
    );
    assert_ne!(bo2.ps.pm_flags & playerstate_iw4::pm_flags::SPRINTING, 0);
    assert!(bo2.sideways_speed() < iw4.sideways_speed() * 0.8);
}

#[test]
fn flat_jump_lands_faster_in_bo2() {
    let mut bo2 = Run::new(BO2);
    let mut iw4 = Run::new(Bo2Feel::IW4);
    for run in [&mut bo2, &mut iw4] {
        run.hold(1000, 127, 0, 0);
        run.hold(50, 127, 0, JUMP);
        // Up and down: about 0.62 s of air at jump height 39.
        while run.ps.ground_entity_num == 0x7FF {
            run.hold(50, 127, 0, 0);
        }
        // The landing slowdown runs on the first frame on the ground.
        run.hold(50, 127, 0, 0);
    }
    eprintln!(
        "speed right after a flat jump lands: bo2 {:.1} iw4 {:.1}",
        bo2.ground_speed(),
        iw4.ground_speed()
    );
    assert!(bo2.ground_speed() > iw4.ground_speed() * 1.05);
}

#[test]
fn jumping_out_of_a_sprint_costs_sprint_in_bo2() {
    let mut bo2 = Run::new(BO2);
    let mut iw4 = Run::new(Bo2Feel::IW4);
    for run in [&mut bo2, &mut iw4] {
        run.hold(1000, 127, 0, SPRINT);
        run.hold(50, 127, 0, SPRINT | JUMP);
    }
    eprintln!(
        "sprint end after a sprint-jump: bo2 {} iw4 {} (time {})",
        bo2.ps.last_sprint_end, iw4.ps.last_sprint_end, bo2.time
    );
    assert_eq!(bo2.ps.last_sprint_end - iw4.ps.last_sprint_end, 800);
}

#[test]
fn sprint_then_prone_dives_only_when_dtp_is_on() {
    let off = Bo2Feel { dive: false, ..BO2 };
    let on = Bo2Feel { dive: true, ..BO2 };
    let mut runs = [Run::new(off), Run::new(on)];
    for run in &mut runs {
        run.hold(1000, 127, 0, SPRINT);
        run.hold(50, 127, 0, SPRINT | playerstate_iw4::buttons::PRONE);
    }
    let dived = |run: &Run| run.ps.pm_flags & movement_iw4::PMF_DIVE != 0;
    assert!(!dived(&runs[0]), "dtp 0 turns the dive off");
    assert!(dived(&runs[1]));
}

const PRONE: u32 = playerstate_iw4::buttons::PRONE;
const STANCE_HELD: u32 = playerstate_iw4::buttons::STANCE_HELD;
const ADS: u32 = 0x800;
const DIVE: Bo2Feel = Bo2Feel {
    dive: true,
    omni: true,
    ..BO2
};

fn flag(run: &Run, flag: u32) -> bool {
    run.ps.pm_flags & flag != 0
}

/// Sprints 1 s, dives pushing this way (a tap of the default hold-style
/// prone key), and runs until the dive leaves the air. Returns the time in
/// the air (ms) and the highest point.
fn dive(run: &mut Run, forward: i8, right: i8, buttons: u32) -> (i32, f32) {
    run.hold(1000, forward, right, SPRINT);
    run.hold(50, forward, right, SPRINT | PRONE | STANCE_HELD | buttons);
    assert!(flag(run, movement_iw4::PMF_DIVE), "dived");
    let start = run.time - 50;
    let mut top = run.ps.origin[2];
    while flag(run, movement_iw4::PMF_DIVE) {
        run.hold(50, 0, 0, SPRINT | buttons);
        top = top.max(run.ps.origin[2]);
        assert!(run.time - start < 3000, "the dive came down");
    }
    (run.time - start, top)
}

/// Runs until the view is at standing height and still.
fn ms_to_stand(run: &mut Run, buttons: u32) -> i32 {
    let start = run.time;
    while run.ps.view_height_current != 60.0 || run.ps.view_height_lerp_time != 0 {
        run.hold(50, 0, 0, buttons);
        assert!(run.time - start < 3000, "stood up");
    }
    run.time - start
}

#[test]
fn omni_sprint_runs_full_speed_any_way_in_bo2() {
    let mut forward = Run::new(DIVE);
    let mut back = Run::new(DIVE);
    let mut side = Run::new(DIVE);
    let mut old_back = Run::new(BO2);
    forward.hold(1000, 127, 0, SPRINT);
    back.hold(1000, -127, 0, SPRINT);
    side.hold(1000, 0, 127, SPRINT);
    old_back.hold(1000, -127, 0, SPRINT);
    eprintln!(
        "sprint speed: forward {:.1} back {:.1} side {:.1}; without omni back {:.1}",
        forward.ground_speed(),
        back.ground_speed(),
        side.ground_speed(),
        old_back.ground_speed()
    );
    for run in [&forward, &back, &side] {
        assert!(flag(run, playerstate_iw4::pm_flags::SPRINTING));
        assert!((run.ground_speed() - forward.ground_speed()).abs() < 1.0);
    }
    assert!(!flag(&old_back, playerstate_iw4::pm_flags::SPRINTING));
}

#[test]
fn dive_goes_the_way_you_push() {
    // Sprinting to the right (yaw 0: right is -y), then diving.
    let mut run = Run::new(DIVE);
    run.hold(1000, 0, 127, SPRINT);
    run.hold(50, 0, 127, SPRINT | PRONE);
    let mut back = Run::new(DIVE);
    back.hold(1000, -127, 0, SPRINT);
    back.hold(50, -127, 0, SPRINT | PRONE);
    eprintln!(
        "dive velocity: right {:?}, back {:?}",
        run.ps.velocity, back.ps.velocity
    );
    assert!(flag(&run, movement_iw4::PMF_DIVE) && flag(&back, movement_iw4::PMF_DIVE));
    assert!(run.ps.velocity[1] < -200.0 && run.ps.velocity[0].abs() < 1.0);
    assert!(back.ps.velocity[0] < -200.0 && back.ps.velocity[1].abs() < 1.0);
}

#[test]
fn dive_rises_to_jump_height_and_lands_with_the_animation() {
    let mut run = Run::new(DIVE);
    let (air_ms, top) = dive(&mut run, 127, 0, 0);
    eprintln!("dive: {air_ms} ms in the air, top {top:.1}");
    // pb_dive_prone: 0.733 s from take-off to touch-down.
    assert!((700..=800).contains(&air_ms), "air time {air_ms}");
    assert!((top - 39.0).abs() < 0.5, "top {top}");
    assert!(flag(&run, movement_iw4::PMF_DIVE_SLIDE));
    assert!(flag(&run, playerstate_iw4::pm_flags::PRONE));
}

#[test]
fn dive_slides_pauses_then_holds_prone_without_the_prone_lock() {
    let mut run = Run::new(DIVE);
    dive(&mut run, 127, 0, 0);
    let landed = run.time;
    while flag(&run, movement_iw4::PMF_DIVE_SLIDE) {
        assert_eq!(run.ps.pm_time, 0, "no 1.8 s prone lock");
        run.hold(50, 0, 0, SPRINT);
    }
    let on_ground = run.time - landed;
    eprintln!("slide + pause: {on_ground} ms");
    assert!((400..=450).contains(&on_ground));
    assert!(flag(&run, movement_iw4::PMF_DIVE_PRONE));
    assert_eq!(run.ground_speed(), 0.0);
    run.hold(500, 0, 0, 0);
    assert!(flag(&run, playerstate_iw4::pm_flags::PRONE), "stays prone");
    assert_eq!(run.ps.pm_flags & playerstate_iw4::pm_flags::JUMPING, 0);
}

#[test]
fn getting_up_from_a_dive_is_twice_as_quick() {
    let mut dived = Run::new(DIVE);
    dive(&mut dived, 127, 0, 0);
    while flag(&dived, movement_iw4::PMF_DIVE_SLIDE) || dived.ps.view_height_lerp_time != 0 {
        dived.hold(50, 0, 0, 0);
    }
    dived.hold(50, 0, 0, JUMP);
    let from_dive = 50 + ms_to_stand(&mut dived, 0);

    let mut prone = Run::new(DIVE);
    prone.hold(2500, 0, 0, PRONE | STANCE_HELD);
    assert_eq!(prone.ps.view_height_current, 11.0);
    let from_prone = ms_to_stand(&mut prone, 0);
    eprintln!("prone to standing: after a dive {from_dive} ms, ordinary {from_prone} ms");
    assert!(from_dive * 10 <= from_prone * 6);
    dived.hold(50, 0, 0, 0);
    assert!(!flag(&dived, movement_iw4::PMF_DIVE_GETUP), "get-up over");
}

#[test]
fn a_held_prone_key_does_not_drop_you_back_down() {
    // Dived on a held prone key, kept holding it, and pressed jump.
    let held = PRONE | STANCE_HELD;
    let mut run = Run::new(DIVE);
    dive(&mut run, 127, 0, held);
    while !flag(&run, movement_iw4::PMF_DIVE_PRONE) {
        run.hold(50, 0, 0, held);
    }
    run.hold(50, 0, 0, held | JUMP);
    ms_to_stand(&mut run, held);
    run.hold(500, 0, 0, held);
    assert_eq!(run.ps.pm_flags & 3, 0, "still standing");
    run.hold(50, 0, 0, 0);
    assert!(!flag(&run, movement_iw4::PMF_DIVE_GETUP));
    run.hold(1000, 0, 0, held);
    assert!(flag(&run, playerstate_iw4::pm_flags::PRONE), "prone key works again");
}

#[test]
fn a_get_up_pressed_mid_dive_is_kept() {
    let mut run = Run::new(DIVE);
    run.hold(1000, 127, 0, SPRINT);
    run.hold(50, 127, 0, SPRINT | PRONE | STANCE_HELD);
    run.hold(100, 0, 0, 0);
    run.hold(50, 0, 0, JUMP);
    assert!(flag(&run, movement_iw4::PMF_DIVE), "still in the air");
    while flag(&run, movement_iw4::PMF_DIVE) || flag(&run, movement_iw4::PMF_DIVE_SLIDE) {
        run.hold(50, 0, 0, 0);
    }
    assert!(!flag(&run, movement_iw4::PMF_DIVE_PRONE));
    ms_to_stand(&mut run, 0);
    assert_eq!(run.ps.pm_flags & 3, 0);
}

#[test]
fn aiming_works_through_a_dive_with_sprint_held() {
    let mut run = Run::new(DIVE);
    run.ads = true;
    run.hold(1000, 127, 0, SPRINT);
    run.hold(50, 127, 0, SPRINT | PRONE | STANCE_HELD);
    run.hold(50, 127, 0, SPRINT | ADS);
    assert!(flag(&run, movement_iw4::PMF_DIVE));
    assert!(flag(&run, playerstate_iw4::pm_flags::ADS_INTENT), "aims in the air");
    while !flag(&run, movement_iw4::PMF_DIVE_PRONE) {
        run.hold(50, 127, 0, SPRINT | ADS);
        assert!(flag(&run, playerstate_iw4::pm_flags::ADS_INTENT), "aims on the ground");
    }
}

#[test]
fn a_prone_toggle_switched_off_gets_you_up() {
    // Toggle-style prone: the key latches prone on (no STANCE_HELD) and a
    // second press lets it go.
    let mut run = Run::new(DIVE);
    run.hold(1000, 127, 0, SPRINT);
    run.hold(50, 127, 0, SPRINT | PRONE);
    while !flag(&run, movement_iw4::PMF_DIVE_PRONE) {
        run.hold(50, 0, 0, PRONE);
    }
    run.hold(500, 0, 0, PRONE);
    assert!(flag(&run, movement_iw4::PMF_DIVE_PRONE), "the latch alone holds prone");
    run.hold(50, 0, 0, 0);
    ms_to_stand(&mut run, 0);
    assert_eq!(run.ps.pm_flags & 3, 0);
}
