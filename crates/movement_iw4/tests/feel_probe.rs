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

/// The floor and a ceiling at `roof` (none at infinity). A `slick` floor is
/// ice; a `mantle` floor is a ledge top the player could climb.
struct Room {
    roof: f32,
    slick: bool,
    mantle: bool,
}

impl CollisionBackend for Room {
    fn trace(&self, input: GroundTraceInput) -> Trace {
        if input.tracemask == movement_iw4::CONTENTS_MANTLE && !self.mantle {
            return Trace {
                fraction: 1.0,
                endpos: input.end,
                ..Trace::default()
            };
        }
        let mut hit = Floor.trace(input);
        if self.slick && hit.walkable != 0 {
            hit.surface_flags = 2;
        }
        let start = input.start[2] + input.maxs[2];
        let end = input.end[2] + input.maxs[2];
        if start <= self.roof && end > self.roof {
            let fraction = (self.roof - start) / (end - start);
            if fraction < hit.fraction {
                hit = Trace {
                    fraction,
                    normal: [0.0, 0.0, -1.0],
                    endpos: input.end,
                    ..Trace::default()
                };
                for i in 0..3 {
                    hit.endpos[i] = input.start[i] + (input.end[i] - input.start[i]) * fraction;
                }
            }
        }
        hit
    }
}

fn context(
    feel: Bo2Feel,
    old_buttons: u32,
    ads_allowed: bool,
    weapon_blocks_prone: bool,
) -> PmoveSingleContext {
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
                shellshock_movement_scale: 1.0,
                prone_lerp_ms: if feel.on {
                    movement_iw4::PRONE_LERP_MS
                } else {
                    0
                },
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
        weapon_blocks_prone,
    }
}

struct Run {
    ps: PlayerState,
    time: i32,
    old_buttons: u32,
    feel: Bo2Feel,
    /// The gun in hand can aim.
    ads: bool,
    /// The gun in hand blocks prone.
    blocks_prone: bool,
    /// A ceiling this high (none at infinity).
    roof: f32,
    /// The floor is ice.
    slick: bool,
    /// The floor is a ledge top the player could climb.
    mantle: bool,
    /// Walking speed scale from a shellshock (1 = none).
    shock: f32,
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
            blocks_prone: false,
            roof: f32::INFINITY,
            slick: false,
            mantle: false,
            shock: 1.0,
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
            let mut ctx = context(self.feel, self.old_buttons, self.ads, self.blocks_prone);
            ctx.walk.cmd_scale.shellshock_movement_scale = self.shock;
            pmove(
                &mut self.ps,
                &mut cmd,
                ctx,
                &Room {
                    roof: self.roof,
                    slick: self.slick,
                    mantle: self.mantle,
                },
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
fn a_dive_needs_over_a_quarter_second_of_sprint() {
    let mut early = Run::new(DIVE);
    let mut late = Run::new(DIVE);
    early.hold(150, 127, 0, SPRINT);
    early.hold(50, 127, 0, SPRINT | PRONE | STANCE_HELD);
    late.hold(300, 127, 0, SPRINT);
    late.hold(50, 127, 0, SPRINT | PRONE | STANCE_HELD);
    assert!(
        !flag(&early, movement_iw4::PMF_DIVE),
        "0.15 s of sprint is too soon"
    );
    assert!(flag(&late, movement_iw4::PMF_DIVE), "0.3 s of sprint dives");
}

#[test]
fn another_dive_waits_a_second_and_a_half_after_the_slide() {
    // Dive, get straight up, then sprint and dive again: 0.3 s of sprint is
    // too soon after the slide, 1.3 s is not.
    let again = |sprint_ms: i32| {
        let mut run = Run::new(DIVE);
        dive(&mut run, 127, 0, 0);
        while flag(&run, movement_iw4::PMF_DIVE_SLIDE) || run.ps.view_height_lerp_time != 0 {
            run.hold(50, 0, 0, 0);
        }
        let slide_end = run.ps.dive_end_time;
        run.hold(50, 0, 0, JUMP);
        ms_to_stand(&mut run, 0);
        run.hold(sprint_ms, 127, 0, SPRINT);
        run.hold(50, 127, 0, SPRINT | PRONE | STANCE_HELD);
        (flag(&run, movement_iw4::PMF_DIVE), run.time - slide_end)
    };
    let (soon, soon_ms) = again(300);
    let (later, later_ms) = again(1300);
    eprintln!("second dive {soon_ms} ms after the slide: {soon}; {later_ms} ms: {later}");
    assert!(!soon && soon_ms <= 1500);
    assert!(later && later_ms > 1500);
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
fn a_dive_steers_a_little() {
    // Dive straight ahead, then push right through the flight: BO2's air
    // control at 0.4 of the 190 wish speed, about 76 a second each second.
    let steer = |right: i8| {
        let mut run = Run::new(DIVE);
        run.hold(1000, 127, 0, SPRINT);
        run.hold(50, 127, 0, SPRINT | PRONE | STANCE_HELD);
        assert!(flag(&run, movement_iw4::PMF_DIVE), "dived");
        while flag(&run, movement_iw4::PMF_DIVE) {
            run.hold(50, 0, right, SPRINT);
            assert!(run.time < 5000, "the dive came down");
        }
        (run.sideways_speed(), run.ps.velocity[0])
    };
    let (still, ahead) = steer(0);
    let (pushed, pushed_ahead) = steer(127);
    eprintln!("sideways speed at touch-down: {still:.1} still, {pushed:.1} pushing right");
    assert!(still < 0.01);
    assert!((30.0..=60.0).contains(&pushed), "pushed {pushed}");
    assert!(
        (pushed_ahead - ahead).abs() < 0.01,
        "pushing sideways keeps the forward speed"
    );
}

#[test]
fn a_dive_under_a_low_ceiling_drops_at_once() {
    // The ceiling stops the rise at 20 of the 39: no hold, straight down.
    let mut run = Run::new(DIVE);
    run.roof = 90.0;
    let (air_ms, top) = dive(&mut run, 127, 0, 0);
    eprintln!("dive under a ceiling: {air_ms} ms in the air, top {top:.1}");
    assert!(top <= 20.01, "top {top}");
    assert!(air_ms < 400, "air time {air_ms}");
}

#[test]
fn a_gun_that_blocks_prone_cannot_dive() {
    let mut run = Run::new(DIVE);
    run.blocks_prone = true;
    run.hold(1000, 127, 0, SPRINT);
    run.hold(50, 127, 0, SPRINT | PRONE | STANCE_HELD);
    assert!(!flag(&run, movement_iw4::PMF_DIVE));
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
fn a_slide_that_stops_ends_the_dive_at_once() {
    let mut run = Run::new(DIVE);
    dive(&mut run, 127, 0, 0);
    let landed = run.time;
    // Stopped dead (into a wall) just after touch-down.
    run.ps.velocity = [1.0, 0.0, 0.0];
    run.hold(50, 0, 0, SPRINT);
    assert!(!movement_iw4::dive_to_prone(&run.ps), "the dive ended");
    while flag(&run, movement_iw4::PMF_DIVE_SLIDE) {
        run.hold(50, 0, 0, SPRINT);
    }
    assert_eq!(
        run.ps.dive_end_time,
        landed + 50,
        "the next dive counts from the stop"
    );
    let held = run.time - landed;
    eprintln!("stopped slide: free after {held} ms");
    assert!((150..=200).contains(&held), "only the pause after it");
}

/// Dives forward and returns the ground speed on the slide's last frame.
fn slide_end_speed(mantle: bool) -> f32 {
    let mut run = Run::new(DIVE);
    run.mantle = mantle;
    dive(&mut run, 127, 0, 0);
    let mut speed = 0.0;
    while movement_iw4::dive_to_prone(&run.ps) {
        speed = run.ground_speed();
        run.hold(50, 0, 0, SPRINT);
    }
    speed
}

#[test]
fn a_slide_over_a_ledge_top_keeps_its_speed() {
    let floor = slide_end_speed(false);
    let ledge = slide_end_speed(true);
    eprintln!("slide's last frame: floor {floor}, ledge top {ledge}");
    assert!(ledge > floor + 100.0);
}

#[test]
fn a_crouch_while_sprinting_only_crouches() {
    let mut run = Run::new(DIVE);
    run.hold(1000, 127, 0, SPRINT);
    run.hold(50, 127, 0, SPRINT | playerstate_iw4::buttons::CROUCH);
    assert!(!flag(&run, movement_iw4::PMF_DIVE), "no dive on a crouch");
}

#[test]
fn scripts_see_the_dive_from_take_off_to_the_slides_end() {
    let mut run = Run::new(DIVE);
    run.hold(1000, 127, 0, SPRINT);
    assert!(
        !movement_iw4::dive_to_prone(&run.ps),
        "sprinting is not diving"
    );
    run.hold(50, 127, 0, SPRINT | PRONE | STANCE_HELD);
    while flag(&run, movement_iw4::PMF_DIVE) {
        assert!(movement_iw4::dive_to_prone(&run.ps), "in the air");
        run.hold(50, 0, 0, SPRINT);
    }
    let landed = run.time;
    while movement_iw4::dive_to_prone(&run.ps) {
        run.hold(50, 0, 0, SPRINT);
        assert!(run.time - landed < 1000, "the slide ended");
    }
    let slide = run.time - landed;
    eprintln!("dive seen on the ground for {slide} ms");
    assert!((250..=300).contains(&slide));
    assert!(
        flag(&run, movement_iw4::PMF_DIVE_SLIDE),
        "the pause still holds him"
    );
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
    assert!(
        flag(&run, playerstate_iw4::pm_flags::PRONE),
        "prone key works again"
    );
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
    assert!(
        flag(&run, playerstate_iw4::pm_flags::ADS_INTENT),
        "aims in the air"
    );
    while !flag(&run, movement_iw4::PMF_DIVE_PRONE) {
        run.hold(50, 127, 0, SPRINT | ADS);
        assert!(
            flag(&run, playerstate_iw4::pm_flags::ADS_INTENT),
            "aims on the ground"
        );
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
    assert!(
        flag(&run, movement_iw4::PMF_DIVE_PRONE),
        "the latch alone holds prone"
    );
    run.hold(50, 0, 0, 0);
    ms_to_stand(&mut run, 0);
    assert_eq!(run.ps.pm_flags & 3, 0);
}

const STAMIN_UP: Bo2Feel = Bo2Feel {
    longer_sprint_perk: movement_iw4::PERK_LONGERSPRINT,
    sprint_multiplier: 2.0,
    ..BO2
};

/// Holds sprint with a gun in hand until the sprint runs out. Returns how
/// long it lasted (ms) and the speed after 1 s of it.
fn sprint_out(feel: Bo2Feel, perk: bool) -> (i32, f32) {
    let mut run = Run::new(feel);
    run.ps.weapon = 1;
    if perk {
        run.ps.perks[0] |= movement_iw4::PERK_LONGERSPRINT;
    }
    run.hold(1000, 127, 0, SPRINT);
    let speed = run.ground_speed();
    while flag(&run, playerstate_iw4::pm_flags::SPRINTING) && run.time < 20_000 {
        run.hold(50, 127, 0, SPRINT);
    }
    (run.ps.last_sprint_end - run.ps.last_sprint_start, speed)
}

#[test]
fn stamin_up_sprints_twice_as_long_and_a_tenth_faster_in_zombies() {
    let zombies = Bo2Feel {
        zombies: true,
        ..STAMIN_UP
    };
    let (plain_ms, plain_speed) = sprint_out(zombies, false);
    let (perk_ms, perk_speed) = sprint_out(zombies, true);
    let (mp_ms, mp_speed) = sprint_out(STAMIN_UP, true);
    eprintln!(
        "sprint: no perk {plain_ms} ms {plain_speed:.1}; Stamin-Up {perk_ms} ms {perk_speed:.1}; MP {mp_ms} ms {mp_speed:.1}"
    );
    assert_eq!(plain_ms, 4000);
    assert_eq!(perk_ms, 8000);
    assert_eq!(mp_ms, 8000);
    assert!((perk_speed / plain_speed - 1.1).abs() < 0.01);
    assert!(
        (mp_speed - plain_speed).abs() < 0.5,
        "multiplayer's perk adds no speed"
    );
}

/// Walks forward on ice for `ms`, then lets go for `coast_ms`. Returns the
/// speed after the walk and after the coast.
fn walk_on_ice(feel: Bo2Feel, ms: i32, coast_ms: i32) -> (f32, f32) {
    let mut run = Run::new(feel);
    run.slick = true;
    run.hold(ms, 127, 0, 0);
    let walked = run.ground_speed();
    run.hold(coast_ms, 0, 0, 0);
    (walked, run.ground_speed())
}

#[test]
fn ice_picks_up_twice_as_fast_and_still_slows_you_in_bo2() {
    let (bo2_start, _) = walk_on_ice(BO2, 200, 0);
    let (iw4_start, _) = walk_on_ice(Bo2Feel::IW4, 200, 0);
    let (bo2_full, bo2_coast) = walk_on_ice(BO2, 3000, 500);
    let (iw4_full, iw4_coast) = walk_on_ice(Bo2Feel::IW4, 3000, 500);
    eprintln!(
        "ice: 0.2 s in BO2 {bo2_start:.1} old {iw4_start:.1}; full {bo2_full:.1}/{iw4_full:.1}; after 0.5 s coasting BO2 {bo2_coast:.1} old {iw4_coast:.1}"
    );
    // Acceleration 2 against sliding friction 1.5: 19 a frame, less 7.5%
    // (speeds snap to whole numbers each frame).
    assert!((bo2_start - 68.0).abs() < 1.0);
    assert!((iw4_start - 40.0).abs() < 1.0);
    assert!((bo2_full - 190.0).abs() < 1.0 && (iw4_full - 190.0).abs() < 1.0);
    // About 190 x 0.925^10: BO2's ice still slows you; the old ice never did.
    assert!((bo2_coast - 88.0).abs() < 1.0);
    assert!((iw4_coast - 190.0).abs() < 1.0);
}

fn walk_shocked(feel: Bo2Feel, shock_file_movement: f32) -> f32 {
    let mut run = Run::new(feel);
    run.ps.pm_flags |= playerstate_iw4::pm_flags::SHELLSHOCKED;
    run.shock = movement_iw4::shellshock_walk_scale(true, shock_file_movement, feel);
    run.hold(3000, 127, 0, 0);
    run.ground_speed()
}

#[test]
fn a_shellshock_slows_you_by_its_own_number_in_bo2() {
    // BO2's shock files: explosion and pain 1.0, flashbang 0.8, most 0.4.
    let bo2: Vec<f32> = [1.0, 0.8, 0.4].map(|m| walk_shocked(BO2, m)).to_vec();
    let old: Vec<f32> = [1.0, 0.8, 0.4]
        .map(|m| walk_shocked(Bo2Feel::IW4, m))
        .to_vec();
    eprintln!("shocked walk: BO2 {bo2:?} old {old:?}");
    assert!(
        (bo2[0] - 190.0).abs() < 1.0,
        "an explosion does not slow you"
    );
    assert!((bo2[1] - 152.0).abs() < 1.0);
    assert!((bo2[2] - 76.0).abs() < 1.0);
    assert!(
        old.iter().all(|s| (s - 76.0).abs() < 1.0),
        "the old rule: 0.4"
    );
}
