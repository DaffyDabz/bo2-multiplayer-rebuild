//! bo2mp: players' bodies as Black Ops II shows them to others.
//!
//! - The body the scripts give a player (`setmodel` / `attach` in BO2's
//!   `mpbody/class_*` faction scripts) goes to every client as a client
//!   value (`bo2mp_body`) of that player.
//! - His third-person animations are picked as the game picks them, from
//!   its own `mp/playeranim.script`: each tick the movement type's first
//!   matching item (idle, walk, run, sprint, climbing...) plays on the legs
//!   and torso that no event holds; events (firing, reloading, a weapon
//!   switch, melee, jumping, landing, stance changes, death) play their
//!   item for its duration or the animation's length. The choice goes out
//!   in his state's `legs_anim` / `torso_anim` (an animation's place in the
//!   script's list, see [`crate::t6_playeranim`]).
//! - `cloneplayer` leaves the engine's corpse (it keeps his death
//!   animation) and gives the scripts an entity for it.

use std::collections::BTreeMap;
use std::sync::Arc;

use bevy_ecs::prelude::{Resource, World};
use gsc_t6::{ObjKind, Value, Vm};
use movement_iw4::{PMF_DIVE, PMF_DIVE_SLIDE};
use playerstate_iw4::{PlayerState, mantle_flags, pm_flags};
use weapon_iw4::WeaponState as S;

use super::{Ent, Zm, frame};
use crate::t6_playeranim::{
    T6AnimCommand, T6AnimFacts, T6AnimPart, T6PlayerAnims, t6_anim_index, t6_move_direction,
    t6_anim_part, t6_pack_anim, t6_weapon_class_name, t6_with_duration, t6_with_legs_yaw,
    t6_with_offhand, T6_ANIM_TOGGLE,
};
use crate::world::ClientId;

/// The client value that carries a player's body: `<model>` then
/// `;<model>@<tag>` per attached model.
pub(crate) const BODY_DVAR: &str = "bo2mp_body";

/// Speed (units/s) under which a player stands still (the engine's
/// `PLAYER_MOVE_THRESHHOLD`).
const MOVE_THRESHOLD: f32 = 10.0;

#[derive(Clone, Debug, Default)]
struct Track {
    alive: bool,
    /// How long an event still holds the legs and the torso (ms): the
    /// engine's legs and torso timers, kept here (a frozen player's movement
    /// does not run them down).
    holds: [i32; 2],
    /// Where his legs face (world yaw, degrees): they lag his view when he
    /// turns standing still (BO2's turn-in-place) and twist toward a
    /// diagonal move; the torso keeps to the view.
    legs_yaw: f32,
    /// Turning in place: 1 right, -1 left, 0 not.
    turning: i8,
    /// The dive to prone last tick: 0 none, 1 in the air, 2 sliding.
    dive: u8,
    /// Mantling last tick.
    mantling: bool,
    stance: u8,
    on_ground: bool,
    weapon: u32,
    wstate: i32,
    clip: i32,
    /// When the legs' and the torso's animations started (their value
    /// without the extra bits, level ms), for posing his hit boxes.
    anim_start: [(i32, i64); 2],
}

#[derive(Resource, Default)]
pub(crate) struct Bodies {
    pub script: Option<Arc<T6PlayerAnims>>,
    tracks: BTreeMap<u32, Track>,
    /// The script entity each player's last body (`cloneplayer`) is.
    corpses: BTreeMap<u32, u32>,
    /// Players whose death animation is set (this life's).
    died: std::collections::BTreeSet<u32>,
    /// Each player's look as last sent.
    looks: BTreeMap<u32, BodyLook>,
    /// Posing hit boxes: (total us, most us, times) since the last report.
    hit_us: (u64, u64, u64),
    /// Each body entity's death animation (`getcorpseanim`).
    corpse_anims: BTreeMap<u32, String>,
    rng: u32,
    logged: bool,
}

impl Bodies {
    pub(crate) fn new(script: Option<Arc<T6PlayerAnims>>) -> Self {
        Self {
            script,
            ..Self::default()
        }
    }

    fn next_rand(&mut self) -> u32 {
        // xorshift32: the variant a multi-animation item plays.
        let mut x = self.rng.max(1);
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        x
    }
}

/// How a player looks to the others: the body model and attachments his
/// scripts chose, and the gun stowed on his back (`setstowedweapon`).
#[derive(Clone, Debug, Default)]
pub(crate) struct BodyLook {
    pub model: String,
    pub attached: Vec<(String, String)>,
    /// (weapon index, weapon name).
    pub stowed: Option<(u32, String)>,
    /// The riot shield he carries: (model, tag) — in his left hand while
    /// it is his weapon, on his back while he has it
    /// (`refreshshieldattachment`).
    pub shield: Option<(String, &'static str)>,
}

/// The tag BO2 draws the stowed gun on.
const STOWED_TAG: &str = "tag_stowed_back";

/// Change a player's look and send it: `<model>`, then `;<model>@<tag>`
/// per attachment, then `;#<weapon index>@tag_stowed_back` for the stowed
/// gun (the client draws that weapon's world model and attachments).
pub(crate) fn edit_body(world: &mut World, client: u32, f: impl FnOnce(&mut BodyLook)) {
    let mut look = world
        .get_resource::<Bodies>()
        .and_then(|b| b.looks.get(&client).cloned())
        .unwrap_or_default();
    f(&mut look);
    let mut v = look.model.clone();
    for (m, tag) in &look.attached {
        v.push(';');
        v.push_str(m);
        v.push('@');
        v.push_str(tag);
    }
    if let Some((w, _)) = &look.stowed {
        v.push_str(&format!(";#{w}@{STOWED_TAG}"));
    }
    if let Some((m, tag)) = &look.shield {
        v.push_str(&format!(";{m}@{tag}"));
    }
    if let Some(mut b) = world.get_resource_mut::<Bodies>() {
        b.looks.insert(client, look);
    }
    super::set_client_dvar(world, client, BODY_DVAR, &v);
}

/// The riot shield on him: drawn on his body (model, tag) and, for the sim,
/// stopping what comes at the side it faces (in his hands: his front; on
/// his back: his back).
fn set_shield(world: &mut World, id: ClientId, shield: Option<(String, &'static str)>) {
    let held = shield.as_ref().map(|(_, tag)| *tag != STOWED_TAG);
    frame(world).set_riot_shield(id, held);
    edit_body(world, id.0, |l| l.shield = shield);
}

/// Everything attached to him comes off (`detachall`), the riot shield too.
pub(crate) fn detach_all(world: &mut World, client: u32) {
    frame(world).set_riot_shield(ClientId(client), None);
    edit_body(world, client, |l| {
        l.attached.clear();
        l.shield = None;
    });
}

/// The weapon stowed on a player's back (`getstowedweapon`).
pub(crate) fn stowed_of(world: &World, client: u32) -> Option<String> {
    world
        .get_resource::<Bodies>()
        .and_then(|b| b.looks.get(&client))
        .and_then(|l| l.stowed.as_ref())
        .map(|(_, name)| name.clone())
}

fn stance_of(ps: &PlayerState) -> u8 {
    if ps.pm_flags & pm_flags::PRONE != 0 {
        2
    } else if ps.pm_flags & pm_flags::CROUCH != 0 {
        1
    } else {
        0
    }
}

const STANCE_NAMES: [&str; 3] = ["stand", "crouch", "prone"];

fn xy_speed(ps: &PlayerState) -> f32 {
    (ps.velocity[0] * ps.velocity[0] + ps.velocity[1] * ps.velocity[1]).sqrt()
}

fn on_ground(ps: &PlayerState) -> bool {
    ps.ground_entity_num != playerstate_iw4::ENTITYNUM_NONE
}

fn movestatus(ps: &PlayerState) -> &'static str {
    if xy_speed(ps) <= MOVE_THRESHOLD {
        "stationary"
    } else if ps.pm_flags & (pm_flags::WALKING | pm_flags::PRONE) != 0 {
        "walk"
    } else {
        "run"
    }
}

/// The movement type the script is read for, as the engine picks it
/// (`None` in the air: the jump's animation holds).
fn movetype(ps: &PlayerState) -> Option<&'static str> {
    if dive_phase(ps) != 0 || ps.pm_flags & pm_flags::MANTLE != 0 {
        return None;
    }
    if ps.pm_flags & pm_flags::LADDER != 0 {
        // Up unless clearly going down (a still climber holds his rung).
        return Some(if ps.velocity[2] < -10.0 { "climbdown" } else { "climbup" });
    }
    if !on_ground(ps) {
        return None;
    }
    if xy_speed(ps) <= MOVE_THRESHOLD {
        return Some("idle");
    }
    if ps.pm_flags & pm_flags::SPRINTING != 0 && stance_of(ps) == 0 {
        return Some("sprint");
    }
    if ps.pm_flags & (pm_flags::WALKING | pm_flags::PRONE) != 0 {
        return Some("walk");
    }
    Some("run")
}

/// The dive to prone (BO2's dolphin dive): 0 none, 1 in the air, 2 sliding
/// into prone on the ground.
fn dive_phase(ps: &PlayerState) -> u8 {
    if ps.pm_flags & PMF_DIVE != 0 {
        1
    } else if ps.pm_flags & PMF_DIVE_SLIDE != 0 {
        2
    } else {
        0
    }
}

/// BO2's mantle animations by the engine's mantle transition (the same
/// clips the mantle moves the player by): up, by ledge height, then over.
const MANTLE_UP_ANIMS: [&str; 7] = [
    "mp_mantle_up_57",
    "mp_mantle_up_51",
    "mp_mantle_up_45",
    "mp_mantle_up_39",
    "mp_mantle_up_33",
    "mp_mantle_up_27",
    "mp_mantle_up_21",
];
const MANTLE_OVER_MOVES: [&str; 3] = ["mantle_over_high", "mantle_over_mid", "mantle_over_low"];

/// The animation a mantle plays now and how long it still has: up to the
/// ledge, then over it when it is a mantle over (that phase as the script's
/// `mantle_over_*` movement type names it; the fast-mantle copies with the
/// perk's flag).
fn mantle_command(world: &World, script: &T6PlayerAnims, ps: &PlayerState) -> Option<(T6AnimCommand, i32)> {
    let trans = ps.mantle_trans_index;
    let fast = ps.mantle_flags & mantle_flags::FAST_MANTLE != 0;
    let suffix = if fast { "_fast" } else { "" };
    let up_index = usize::try_from(movement_iw4::mantle::trans_up_anim(trans) - 1).ok()?;
    let up_name = format!("{}{suffix}", MANTLE_UP_ANIMS.get(up_index)?);
    let up = script.anims.iter().position(|a| *a == up_name)? as u16;
    let up_len = anim_length_ms(world, script, up);
    let elapsed = ps.mantle_timer;
    if elapsed <= up_len || ps.mantle_flags & mantle_flags::OVER == 0 {
        let command = T6AnimCommand {
            part: T6AnimPart::Both,
            anim: up,
            blend_ms: None,
            blend_out_ms: None,
            duration_ms: None,
            weapon_time_scale: false,
            grenade_anim: false,
        };
        return Some((command, (up_len - elapsed).max(50)));
    }
    let over_index = usize::try_from(movement_iw4::mantle::trans_over_anim(trans) - 8).ok()?;
    let movetype = MANTLE_OVER_MOVES.get(over_index)?;
    let mut command = *script.pick_move(movetype, &T6AnimFacts::default())?.first()?;
    if fast {
        let fast_name = format!("{}_fast", script.anims.get(usize::from(command.anim))?);
        if let Some(i) = script.anims.iter().position(|a| *a == fast_name) {
            command.anim = i as u16;
        }
    }
    let over_len = anim_length_ms(world, script, command.anim);
    Some((command, (up_len + over_len - elapsed).max(50)))
}

/// How long the weapon takes for the action an event starts (ms): the
/// time left on the weapon (`weapon_time`, which the reload, raise or drop
/// just set, perks included); a segmented reload (a shotgun's shell by
/// shell) adds its shells and its end.
fn weapon_action_ms(world: &mut World, ps: &PlayerState, event: &str, clip: i32) -> Option<i32> {
    let facts = frame(world).combat_facts_for(ps.weapon)?;
    let left = ps.weapon_time.max(0);
    let ms = if event == "reload" && facts.segmented_reload {
        let shells = (facts.clip_size - clip).max(1);
        facts.reload_start_time_ms.max(left) + shells * facts.reload_time_ms + facts.reload_end_time_ms
    } else if left > 0 {
        left
    } else {
        match event {
            "reload" if clip == 0 => facts.reload_empty_time_ms,
            "reload" => facts.reload_time_ms,
            "dropweapon" => facts.drop_time_ms,
            _ => facts.raise_time_ms,
        }
    };
    (ms > 0).then_some(ms + crate::MATCH_TICK_MS as i32)
}

/// The weapon facts the script reads (anim type, class), for `weapon`.
fn weapon_words(world: &mut World, script: &T6PlayerAnims, weapon: u32) -> (String, &'static str) {
    let facts = frame(world).combat_facts_for(weapon);
    let anim_type = facts.map_or(1, |f| f.player_anim_type);
    let class = facts.map_or(0, |f| f.weap_class);
    (
        script.anim_type_name(anim_type).to_owned(),
        t6_weapon_class_name(class),
    )
}

fn anim_length_ms(world: &World, script: &T6PlayerAnims, anim: u16) -> i32 {
    let Some(name) = script.anims.get(usize::from(anim)) else {
        return 0;
    };
    world
        .resource::<Zm>()
        .anims
        .get(name)
        .filter(|a| a.framerate > 0.0)
        .map_or(500, |a| ((f32::from(a.numframes) / a.framerate) * 1000.0) as i32)
}

/// Start a command on the parts it names. An event restarts its animation
/// (the toggle bit flips) and holds the parts for its time; a movement
/// animation only takes parts no event holds and keeps one already playing.
fn apply(ps: &mut PlayerState, holds: &mut [i32; 2], command: &T6AnimCommand, hold_ms: Option<i32>) {
    let legs = matches!(command.part, T6AnimPart::Legs | T6AnimPart::Both);
    let torso = matches!(command.part, T6AnimPart::Torso | T6AnimPart::Both);
    let set = |value: &mut i32, timer: &mut i32| match hold_ms {
        Some(ms) => {
            let toggle = *value & T6_ANIM_TOGGLE == 0;
            *value = t6_pack_anim(command.anim, toggle);
            *timer = ms.max(0);
        }
        None => {
            if *timer > 0 || t6_anim_index(*value) == Some(command.anim) {
                return;
            }
            let toggle = *value & T6_ANIM_TOGGLE != 0;
            *value = t6_pack_anim(command.anim, !toggle);
        }
    };
    let [legs_hold, torso_hold] = holds;
    if legs {
        set(&mut ps.legs_anim, legs_hold);
    }
    if torso {
        set(&mut ps.torso_anim, torso_hold);
    }
}

/// Legs that start turning in place once the view is this far from them.
const TURN_START_DEG: f32 = 45.0;
/// Legs never trail the view by more than this.
const TURN_MAX_DEG: f32 = 90.0;
/// How fast the legs come round turning in place (degrees/s).
const TURN_SPEED_DEG: f32 = 180.0;
/// How fast the legs twist to a move's direction (degrees/s).
const TWIST_SPEED_DEG: f32 = 540.0;

fn norm180(mut a: f32) -> f32 {
    while a > 180.0 {
        a -= 360.0;
    }
    while a < -180.0 {
        a += 360.0;
    }
    a
}

fn approach(from: f32, to: f32, step: f32) -> f32 {
    if (to - from).abs() <= step { to } else { from + step * (to - from).signum() }
}

/// One tick of the legs' yaw: moving, they face the move within its 4-way
/// animation (a diagonal twists them up to 45 degrees); standing, they stay
/// put until the view is `TURN_START_DEG` away, then turn in place to it.
/// Returns the legs' offset from the view (degrees).
fn legs_yaw_step(track: &mut Track, ps: &PlayerState, ground: bool, direction: &str) -> f32 {
    let view = ps.viewangles[1];
    let dt = crate::MATCH_TICK_MS as f32 / 1000.0;
    if !track.alive {
        track.legs_yaw = view;
        track.turning = 0;
    }
    let mut off = norm180(track.legs_yaw - view);
    if ps.pm_flags & pm_flags::LADDER != 0 {
        // On a ladder he faces it (its normal points out at him).
        track.turning = 0;
        let v = ps.v_ladder_vec;
        if v[0] * v[0] + v[1] * v[1] > 1e-4 {
            off = norm180((-v[1]).atan2(-v[0]).to_degrees() - view);
        }
        track.legs_yaw = view + off;
        return off;
    }
    if ground && xy_speed(ps) > MOVE_THRESHOLD {
        track.turning = 0;
        let heading = ps.velocity[1].atan2(ps.velocity[0]).to_degrees();
        let rel = norm180(heading - view);
        let center = match direction {
            "left" => 90.0,
            "right" => -90.0,
            "backward" => 180.0,
            _ => 0.0,
        };
        let want = norm180(rel - center).clamp(-45.0, 45.0);
        off = approach(off, want, TWIST_SPEED_DEG * dt);
    } else if ground {
        if track.turning != 0 {
            off = approach(off, 0.0, TURN_SPEED_DEG * dt);
            if off.abs() < 1.0 {
                off = 0.0;
                track.turning = 0;
            }
        } else if off.abs() > TURN_START_DEG {
            // Legs left of the view: he turned right.
            track.turning = if off > 0.0 { 1 } else { -1 };
        }
    }
    off = off.clamp(-TURN_MAX_DEG, TURN_MAX_DEG);
    track.legs_yaw = view + off;
    off
}

fn pick(bodies: &mut Bodies, commands: &[T6AnimCommand]) -> Option<T6AnimCommand> {
    if commands.is_empty() {
        return None;
    }
    let r = bodies.next_rand() as usize;
    Some(commands[r % commands.len()])
}

/// Each tick: every living player's events, then his movement animation.
pub(super) fn advance(world: &mut World) {
    let Some(script) = world
        .get_resource::<Bodies>()
        .and_then(|b| b.script.clone())
    else {
        return;
    };
    if !world.resource::<Bodies>().logged {
        world.resource_mut::<Bodies>().logged = true;
        diag::info!(Sim, "bo2mp bodies: {}", script.report_line());
    }
    let clients: Vec<u32> = world.resource::<Zm>().players.keys().copied().collect();
    for c in clients {
        let id = ClientId(c);
        let alive = frame(world)
            .client_meta(id)
            .is_some_and(|m| m.lifecycle == crate::ClientLifecycle::Alive);
        let Some(ps) = frame(world).player(id).copied() else {
            continue;
        };
        let alive = alive && ps.pm_type < playerstate_iw4::PM_TYPE_DEAD;
        let mut track = world
            .resource::<Bodies>()
            .tracks
            .get(&c)
            .cloned()
            .unwrap_or_default();
        if alive {
            world.resource_mut::<Bodies>().died.remove(&c);
        }
        if !alive {
            track.alive = false;
            world.resource_mut::<Bodies>().tracks.insert(c, track);
            frame(world).set_t6_hit_bones(id, None);
            continue;
        }
        for hold in &mut track.holds {
            *hold = (*hold - crate::MATCH_TICK_MS as i32).max(0);
        }
        let clip = crate::script_player::ammo_clip(&frame(world), id, ps.weapon);
        let stance = stance_of(&ps);
        let ground = on_ground(&ps);
        let (anim_type, class) = weapon_words(world, &script, ps.weapon);
        let state = |v: i32| S::from_i32(v).ok();
        let st = state(ps.weaponstate_primary);
        let last = state(track.wstate);
        let reloading = |s: Option<S>| {
            matches!(
                s,
                Some(S::Reloading | S::ReloadStart | S::ReloadEnd | S::ReloadingInterrupt | S::ReloadStartInterrupt)
            )
        };
        let dropping = |s: Option<S>| matches!(s, Some(S::Dropping | S::DroppingQuick | S::DroppingAltswitch));
        let raising = |s: Option<S>| matches!(s, Some(S::Raising | S::RaisingAltswitch));
        let meleeing = |s: Option<S>| matches!(s, Some(S::MeleeInit | S::MeleeFire | S::MeleeEnd));
        let priming = |s: Option<S>| matches!(s, Some(S::OffhandInit | S::OffhandPrepare | S::OffhandHold));
        let throwing = |s: Option<S>| matches!(s, Some(S::OffhandStart | S::Offhand));
        let mut events: Vec<(&str, bool)> = Vec::new();
        let dive = dive_phase(&ps);
        let mantling = ps.pm_flags & pm_flags::MANTLE != 0;
        // A dive or a mantle moves the stance and leaves the ground on its
        // own: their animations say so, not stance changes, jumps or landings.
        let mantle_held = track.holds[0] > 0
            && script
                .name_of(ps.legs_anim)
                .is_some_and(|n| n.starts_with("mp_mantle"));
        let own_motion = dive != 0 || track.dive != 0 || mantling || track.mantling || mantle_held;
        if !track.alive {
            events.push(("firstraiseweapon", false));
        } else if dive == 1 && track.dive != 1 {
            events.push(("dtp_takeoff", false));
        } else if dive == 2 && track.dive != 2 {
            events.push(("dtp_land", false));
        } else if own_motion {
        } else {
            match (track.stance, stance) {
                (0, 1) => events.push(("stand_to_crouch", false)),
                (1, 0) => events.push(("crouch_to_stand", false)),
                (1, 2) | (0, 2) => events.push(("crouch_to_prone", false)),
                (2, 1) => events.push(("prone_to_crouch", false)),
                (2, 0) => events.push(("prone_to_stand", false)),
                _ => {}
            }
            if track.on_ground && !ground && ps.velocity[2] > 0.0 {
                events.push(("jump", false));
            }
            if !track.on_ground && ground {
                events.push(("land", false));
            }
            if dropping(st) && !dropping(last) {
                events.push(("dropweapon", false));
            }
            if raising(st) && !raising(last) {
                events.push(("raiseweapon", false));
            }
            if reloading(st) && !reloading(last) {
                events.push(("reload", false));
            }
            if meleeing(st) && !meleeing(last) {
                events.push(("meleeattack", false));
            }
            if priming(st) && !priming(last) {
                events.push(("prime_grenade", true));
            }
            if throwing(st) && !throwing(last) {
                events.push(("fireweapon", true));
            }
            if ps.weapon == track.weapon && clip < track.clip && !reloading(st) {
                events.push(("fireweapon", false));
            }
        }
        // Weapon events go on through a dive or a mantle.
        if own_motion && track.alive {
            if priming(st) && !priming(last) {
                events.push(("prime_grenade", true));
            }
            if throwing(st) && !throwing(last) {
                events.push(("fireweapon", true));
            }
            if ps.weapon == track.weapon && clip < track.clip && !reloading(st) {
                events.push(("fireweapon", false));
            }
        }
        let direction = t6_move_direction(ps.velocity, ps.viewangles[1]);
        let position = if ps.pm_flags & pm_flags::ADS_INTENT != 0 { "ads" } else { "hip" };
        let was_turning = track.turning;
        let legs_off = legs_yaw_step(&mut track, &ps, ground, direction);
        if std::env::var_os("BO2MP_BODYLOG").is_some()
            && (track.turning != was_turning || world.resource::<Zm>().now_ms % 1000 == 0)
        {
            diag::info!(
                Sim,
                "bo2mp body {c}: legs turn {} (view {:.0}, legs {:.0}, ground {ground}, speed {:.0})",
                track.turning,
                ps.viewangles[1],
                track.legs_yaw,
                xy_speed(&ps)
            );
        }
        let mut ps_new = ps;
        let offhand = u32::try_from(ps.off_hand_index).unwrap_or(0);
        let (off_type, off_class) = if offhand != 0 {
            weapon_words(world, &script, offhand)
        } else {
            ("default".to_owned(), "grenade")
        };
        for (event, grenade) in events {
            let facts = T6AnimFacts {
                anim_type: if grenade { off_type.as_str() } else { anim_type.as_str() },
                weapon_class: if grenade { off_class } else { class },
                stance: STANCE_NAMES[usize::from(stance)],
                direction,
                movestatus: movestatus(&ps),
                weapon_position: position,
                ..T6AnimFacts::default()
            };
            let Some(commands) = script.pick_event(event, &facts) else {
                continue;
            };
            let Some(command) = pick(&mut world.resource_mut::<Bodies>(), commands) else {
                continue;
            };
            // A reload, and the script's weaponTimeScale animations (raise,
            // drop), take as long as the weapon does: they end with the
            // first-person action.
            let timed = (event == "reload" || command.weapon_time_scale)
                .then(|| weapon_action_ms(world, &ps, event, clip))
                .flatten();
            let hold = timed.or(command.duration_ms).unwrap_or_else(|| anim_length_ms(world, &script, command.anim));
            apply(&mut ps_new, &mut track.holds, &command, Some(hold));
            if let Some(ms) = timed
                && matches!(command.part, T6AnimPart::Torso | T6AnimPart::Both)
            {
                ps_new.torso_anim = t6_with_duration(ps_new.torso_anim, ms);
            }
            // The script's grenadeAnim animations handle the offhand item
            // (a grenade thrown, a claymore planted): it is in his hand.
            if command.grenade_anim
                && grenade
                && offhand != 0
                && matches!(command.part, T6AnimPart::Torso | T6AnimPart::Both)
            {
                ps_new.torso_anim = t6_with_offhand(ps_new.torso_anim, offhand);
            }
            if std::env::var_os("BO2MP_BODYLOG").is_some() && matches!(event, "reload" | "dropweapon" | "raiseweapon") {
                diag::info!(
                    Sim,
                    "bo2mp body {c}: {event} {:?} own {} ms, weapon {:?} ms",
                    script.anims.get(usize::from(command.anim)),
                    anim_length_ms(world, &script, command.anim),
                    timed
                );
            }
        }
        if mantling
            && let Some((command, left_ms)) = mantle_command(world, &script, &ps)
            && t6_anim_index(ps_new.legs_anim) != Some(command.anim)
        {
            apply(&mut ps_new, &mut track.holds, &command, Some(left_ms));
        }
        if let Some(mut mt) = movetype(&ps) {
            if mt == "idle" && track.turning != 0 {
                mt = if track.turning > 0 { "turnright" } else { "turnleft" };
            }
            let facts = T6AnimFacts {
                anim_type: &anim_type,
                weapon_class: class,
                stance: STANCE_NAMES[usize::from(stance)],
                direction,
                movestatus: movestatus(&ps),
                weapon_position: position,
                ..T6AnimFacts::default()
            };
            let commands = script
                .pick_move(mt, &facts)
                .or_else(|| (mt != "idle").then(|| script.pick_move("idle", &facts)).flatten());
            if let Some(commands) = commands {
                // Keep a variant already playing (a two-animation idle
                // must not flip each tick).
                let playing = commands
                    .iter()
                    .find(|c| t6_anim_index(ps_new.legs_anim) == Some(c.anim))
                    .copied();
                let command = match playing {
                    Some(c) => Some(c),
                    None => pick(&mut world.resource_mut::<Bodies>(), commands),
                };
                if let Some(command) = command {
                    apply(&mut ps_new, &mut track.holds, &command, None);
                }
            }
        }
        ps_new.legs_anim = t6_with_legs_yaw(ps_new.legs_anim, legs_off);
        if ps_new.legs_anim != ps.legs_anim || ps_new.torso_anim != ps.torso_anim {
            let changed = (ps_new.legs_anim ^ ps.legs_anim) & 0xffff != 0
                || ps_new.torso_anim != ps.torso_anim;
            if changed && std::env::var_os("BO2MP_BODYLOG").is_some() {
                diag::info!(
                    Sim,
                    "bo2mp body {c}: legs {:?} torso {:?} (timers {} {}) legs yaw {legs_off:.0} at ({:.0} {:.0} {:.0}) yaw {:.0}",
                    script.name_of(ps_new.legs_anim),
                    script.name_of(ps_new.torso_anim),
                    track.holds[0],
                    track.holds[1],
                    ps.origin[0],
                    ps.origin[1],
                    ps.origin[2],
                    ps.viewangles[1]
                );
            }
            if let Some(p) = frame(world).player_mut(id) {
                p.legs_anim = ps_new.legs_anim;
                p.torso_anim = ps_new.torso_anim;
            }
        }
        let now = world.resource::<Zm>().now_ms;
        let mut anim_start = track.anim_start;
        for (k, value) in [ps_new.legs_anim, ps_new.torso_anim].into_iter().enumerate() {
            let v = t6_anim_part(value);
            if anim_start[k].0 != v {
                anim_start[k] = (v, now);
            }
        }
        track = Track {
            alive: true,
            holds: track.holds,
            legs_yaw: track.legs_yaw,
            turning: track.turning,
            dive: dive_phase(&ps),
            mantling: ps.pm_flags & pm_flags::MANTLE != 0,
            stance,
            on_ground: ground,
            weapon: ps.weapon,
            wstate: ps.weaponstate_primary,
            clip,
            anim_start,
        };
        // His hit boxes as his body stands now.
        let mut posed_ps = ps;
        posed_ps.legs_anim = ps_new.legs_anim;
        posed_ps.torso_anim = ps_new.torso_anim;
        let started = std::time::Instant::now();
        let bones = hit_bones(world, c, &posed_ps, &track, &script, now);
        frame(world).set_t6_hit_bones(id, bones.map(Arc::new));
        if std::env::var_os("BO2MP_HITLOG").is_some() {
            let us = started.elapsed().as_micros() as u64;
            let mut b = world.resource_mut::<Bodies>();
            b.hit_us = (b.hit_us.0 + us, b.hit_us.1.max(us), b.hit_us.2 + 1);
            if now % 5000 < crate::MATCH_TICK_MS as i64 && c == 0 {
                let (sum, max, n) = std::mem::take(&mut b.hit_us);
                diag::info!(Sim, "bo2mp hit boxes posed: {n} times, {} us average, {max} us at most", sum / n.max(1));
            }
        }
        world.resource_mut::<Bodies>().tracks.insert(c, track);
    }
}

/// The torso's animation outweighs the legs' on the bones both move (as
/// his body is drawn).
const TORSO_OVER_LEGS: f32 = 1000.0;

/// His hit boxes in his own space (feet at the origin, facing +x): BO2's
/// per-bone boxes of his body and head models (their bone info, each with
/// its hit location), posed as his body is drawn — the animations the
/// script plays on his legs and torso at their time, his aim, his legs
/// turned under him.
fn hit_bones(
    world: &mut World,
    c: u32,
    ps: &PlayerState,
    track: &Track,
    script: &T6PlayerAnims,
    now: i64,
) -> Option<Vec<xmodel_runtime::CollisionBone>> {
    use xmodel_runtime::{AnimInstance, Attach, DObj};
    let look = world.get_resource::<Bodies>()?.looks.get(&c)?.clone();
    if look.model.is_empty() {
        return None;
    }
    let caps: Vec<(Arc<xmodel_runtime::RetainedModelCapability>, Option<String>)> = {
        let f = frame(world);
        let body = f.model_capability(&look.model).flatten()?;
        let mut v = vec![(body, None)];
        for (m, tag) in &look.attached {
            if let Some(cap) = f.model_capability(m).flatten() {
                v.push((cap, Some(tag.clone())));
            }
        }
        v
    };
    let models: Vec<(&xmodel_runtime::ModelPoseSrc, Option<Attach>)> = caps
        .iter()
        .map(|(cap, tag)| {
            let attach = tag.as_ref().map(|t| Attach {
                parent_model: 0,
                tag: if t.is_empty() {
                    xmodel_runtime::empty_tag_attach(&caps[0].0.pose, &cap.pose)
                } else {
                    t.clone()
                },
            });
            (&cap.pose, attach)
        })
        .collect();
    let dobj = DObj::build(&models).ok()?;
    // The upper body the torso's animation drives: j_spinelower down.
    let mut upper = anim_iw4::PartBits::default();
    if let Some(root) = dobj.find("j_spinelower") {
        let mut on = vec![false; dobj.bones.len()];
        for i in 0..dobj.bones.len() {
            on[i] = i == root || dobj.bones[i].parent.is_some_and(|p| p < i && on[p]);
            if on[i] {
                upper.set(i);
            }
        }
    } else {
        upper = dobj.all_parts();
    }
    let playing = |value: i32, start: i64, torso: bool| {
        let name = script.name_of(value)?;
        let clip = world.resource::<Zm>().clips.get(&name.to_ascii_lowercase())?.clone();
        let duration = clip.duration().max(1e-3);
        let mut rate = 1.0;
        if torso && let Some(secs) = crate::t6_playeranim::t6_anim_duration(value) {
            rate = (duration / secs.max(0.05)).clamp(0.2, 5.0);
        }
        let t = (now - start).max(0) as f32 / 1000.0 * rate;
        let t = if clip.looping { t.rem_euclid(duration) } else { t.min(duration) };
        Some((clip, t))
    };
    let legs = playing(ps.legs_anim, track.anim_start[0].1, false);
    let torso = playing(ps.torso_anim, track.anim_start[1].1, true);
    let tracks: Vec<Vec<Option<usize>>> = [&legs, &torso]
        .iter()
        .map(|p| p.as_ref().map_or_else(Vec::new, |(clip, _)| dobj.tracks_for(clip)))
        .collect();
    let mut instances = Vec::new();
    if let Some((clip, t)) = &legs {
        instances.push(AnimInstance { clip, tracks: &tracks[0], time: *t, weight: 1.0, parts: None });
    }
    if let Some((clip, t)) = &torso {
        instances.push(AnimInstance {
            clip,
            tracks: &tracks[1],
            time: *t,
            weight: TORSO_OVER_LEGS,
            parts: Some(&upper),
        });
    }
    let input = xmodel_runtime::PlayerControllerInput {
        view_pitch_deg: ps.viewangles[0],
        prone: ps.e_flags & playerstate_iw4::eflags::PRONE != 0,
        crouch: ps.e_flags & playerstate_iw4::eflags::DUCK != 0,
        lean_frac: 0.0,
    };
    let mut posed = dobj.pose_with_controller(&instances, &dobj.all_parts(), glam::Mat4::IDENTITY, |dobj, _, locals| {
        xmodel_runtime::apply_player_controller(dobj, locals, input);
    });
    // His legs turned under him (turning in place, a diagonal move); on a
    // ladder all of him faces it.
    let legs_yaw = crate::t6_playeranim::t6_legs_yaw(ps.legs_anim).to_radians();
    let climbing = script
        .name_of(ps.legs_anim)
        .is_some_and(|n| n.starts_with("pb_climb") || n.starts_with("pb_riot_climb"));
    if legs_yaw.abs() > 1e-4 {
        if climbing {
            let turn = glam::Mat4::from_rotation_z(legs_yaw);
            for m in &mut posed {
                *m = turn * *m;
            }
        } else if let (Some(pelvis), Some(stabilizer)) = (dobj.find("pelvis"), dobj.find("torso_stabilizer"))
            && let Some(pivot) = posed.get(pelvis).map(|m| m.w_axis.truncate())
        {
            let turn = glam::Mat4::from_translation(pivot)
                * glam::Mat4::from_rotation_z(legs_yaw)
                * glam::Mat4::from_translation(-pivot);
            let mut legs = vec![false; dobj.bones.len().min(posed.len())];
            for i in 0..legs.len() {
                legs[i] = i == pelvis || (i != stabilizer && dobj.bones[i].parent.is_some_and(|p| p < i && legs[p]));
                if legs[i] {
                    posed[i] = turn * posed[i];
                }
            }
        }
    }
    let mut out = Vec::new();
    for (slot, (cap, _)) in dobj.models.iter().zip(&caps) {
        for local in 0..slot.bone_count {
            let Some(Some(bc)) = cap.bone_collision.get(local) else {
                continue;
            };
            let Some(m) = posed.get(slot.base + local) else {
                continue;
            };
            let Some(mut b) = xmodel_runtime::collision_bone_from_local_box(
                u16::try_from(slot.base + local).ok()?,
                bc.midpoint,
                bc.half_size,
                *m,
            ) else {
                continue;
            };
            b.part_classification = bc.part_classification;
            out.push(b);
        }
    }
    if std::env::var_os("BO2MP_HITLOG").is_some() && now % 5000 < crate::MATCH_TICK_MS as i64 {
        let mut kinds: Vec<u8> = out.iter().map(|b| b.part_classification).collect();
        kinds.sort_unstable();
        kinds.dedup();
        diag::info!(
            Sim,
            "bo2mp hit boxes {c}: {} boxes on {} ({} models), hit locations {kinds:?}; legs {:?} ({}) torso {:?} ({}); boxes {:?}",
            out.len(),
            look.model,
            caps.len(),
            script.name_of(ps.legs_anim),
            legs.is_some(),
            script.name_of(ps.torso_anim),
            torso.is_some(),
            out.iter()
                .map(|b| (b.part_classification, b.center.map(|v| v.round()), b.half_size.map(|v| v.round())))
                .collect::<Vec<_>>()
        );
    }
    (!out.is_empty()).then_some(out)
}

/// A lethal hit: his death animation, as the script's DEATH event picks it
/// from how he died (`dmgType`: explosive, headshot, melee, shotgun) and
/// from where (`dmgDirection`: the side the hit came from), his stance and
/// his movement. The engine's corpse copies it.
pub(super) fn death(world: &mut World, id: ClientId, means: &str, dir: Option<[f32; 3]>, weapon: &str) {
    let Some(script) = world
        .get_resource::<Bodies>()
        .and_then(|b| b.script.clone())
    else {
        return;
    };
    let Some(ps) = frame(world).player(id).copied() else {
        return;
    };
    let w = super::weapon(world, weapon).unwrap_or(0);
    let (anim_type, class) = weapon_words(world, &script, w);
    let means = means.to_ascii_uppercase();
    let dmg_type = if means == "MOD_HEAD_SHOT" {
        "headshot"
    } else if means.starts_with("MOD_MELEE") || means == "MOD_BAYONET" {
        if anim_type == "riotshield" { "meleebash" } else { "melee" }
    } else if means.contains("GRENADE") || means.contains("EXPLOSIVE") || means.contains("PROJECTILE") {
        "explosive"
    } else if class == "spread" {
        "normal_shotgun"
    } else {
        "normal"
    };
    // The side the hit came from: the way back along its direction, against
    // the way he faced.
    let dmg_direction = dir
        .filter(|d| d[0] * d[0] + d[1] * d[1] > 1e-6)
        .map_or("front", |d| {
            let from = (-d[1]).atan2(-d[0]).to_degrees();
            let mut rel = from - ps.viewangles[1];
            while rel > 180.0 {
                rel -= 360.0;
            }
            while rel < -180.0 {
                rel += 360.0;
            }
            if rel.abs() <= 45.0 {
                "front"
            } else if rel.abs() >= 135.0 {
                "back"
            } else if rel > 0.0 {
                "left"
            } else {
                "right"
            }
        });
    let stance = STANCE_NAMES[usize::from(stance_of(&ps))];
    let facts = T6AnimFacts {
        anim_type: &anim_type,
        weapon_class: class,
        stance,
        direction: t6_move_direction(ps.velocity, ps.viewangles[1]),
        movestatus: movestatus(&ps),
        weapon_position: "hip",
        dmg_type,
        dmg_direction,
    };
    let Some(commands) = script.pick_event("death", &facts) else {
        return;
    };
    let Some(command) = pick(&mut world.resource_mut::<Bodies>(), commands) else {
        return;
    };
    let both = T6AnimCommand {
        part: T6AnimPart::Both,
        ..command
    };
    let hold = anim_length_ms(world, &script, command.anim);
    if std::env::var_os("BO2MP_BODYLOG").is_some() {
        diag::info!(
            Sim,
            "bo2mp body {}: death {:?} ({dmg_type} from the {dmg_direction}, {stance})",
            id.0,
            script.anims.get(usize::from(command.anim))
        );
    }
    if let Some(p) = frame(world).player_mut(id) {
        apply(p, &mut [0, 0], &both, Some(hold));
    }
    world.resource_mut::<Bodies>().died.insert(id.0);
}

/// `cloneplayer`: the engine's corpse of him (his death animation, where he
/// fell) and an entity the scripts keep for it (`self.body`). His previous
/// body's entity goes.
pub(super) fn clone_player(vm: &mut Vm<World>, world: &mut World, client: u32) -> Value {
    let id = ClientId(client);
    let Some(ps) = frame(world).player(id).copied() else {
        return Value::Undefined;
    };
    // A death no hit announced (a fall, a suicide): his death animation
    // first, never a standing corpse.
    if !world
        .get_resource::<Bodies>()
        .is_some_and(|b| b.died.contains(&client))
    {
        death(world, id, "MOD_UNKNOWN", None, "none");
    }
    let tick = super::tick(world);
    if crate::script_player::clone_corpse(&mut frame(world), tick, id).is_none() {
        return Value::Undefined;
    }
    let old = world
        .get_resource_mut::<Bodies>()
        .and_then(|mut b| b.corpses.remove(&client));
    if let Some(n) = old {
        if let Some(mut b) = world.get_resource_mut::<Bodies>() {
            b.corpse_anims.remove(&n);
        }
        let obj = world.resource_mut::<Zm>().ents.remove(&n).and_then(|e| e.obj);
        if let Some(o) = obj {
            vm.free_object(world, o);
        }
    }
    let n = world.resource_mut::<Zm>().alloc_entnum();
    let obj = vm.alloc_object(ObjKind::Entity(n));
    world.resource_mut::<Zm>().ents.insert(
        n,
        Ent {
            obj: Some(obj),
            classname: "player_corpse".to_owned(),
            origin: ps.origin,
            angles: [0.0, ps.viewangles[1], 0.0],
            ..Default::default()
        },
    );
    // The death animation the corpse plays (his legs' as he fell).
    let legs = frame(world).player(id).map(|p| p.legs_anim);
    let anim = legs.and_then(|legs| {
        world
            .get_resource::<Bodies>()?
            .script
            .as_ref()?
            .name_of(legs)
            .map(str::to_owned)
    });
    if let Some(mut b) = world.get_resource_mut::<Bodies>() {
        b.corpses.insert(client, n);
        if let Some(anim) = anim {
            b.corpse_anims.insert(n, anim);
        }
    }
    Value::Object(obj)
}

/// An animation value's name (a `%anim` reference or a name string).
fn anim_name(vm: &Vm<World>, v: &Value) -> Option<String> {
    match v {
        Value::Anim(id) => vm
            .program
            .anims
            .get(*id as usize)
            .map(|(_, a)| vm.strings.get(*a).to_ascii_lowercase()),
        Value::Str(_) | Value::IStr(_) => Some(vm.to_text(v).to_ascii_lowercase()),
        _ => None,
    }
}

/// The animation natives BO2's death path asks of a body
/// (`_globallogic_player::delaystartragdoll`): the corpse's death
/// animation, whether it is a ragdoll (never: there is no ragdoll), an
/// animation's notetracks and their times (fractions of it), its length in
/// seconds, from the animations the zones hold.
pub(super) fn bind(vm: &mut Vm<World>) {
    // BO2's riot shield on the body (maps/mp/_riotshield calls this when his
    // weapon changes): the script's carried model (level.carriedshieldmodel)
    // in his left hand while the shield is his weapon, its stowed model
    // (level.stowedshieldmodel) on his back while he only has it.
    vm.bind("refreshshieldattachment", true, |vm, world, s, _| {
        let id = super::client(vm, world, s)?;
        let level_text = |vm: &mut Vm<World>, name: &str| {
            let f = vm.intern(name);
            match vm.raw_field(vm.level, f) {
                Value::Undefined => String::new(),
                v => vm.to_text(&v),
            }
        };
        let carried = level_text(vm, "carriedshieldmodel");
        let stowed = level_text(vm, "stowedshieldmodel");
        let Some(script) = world.get_resource::<Bodies>().and_then(|b| b.script.clone()) else {
            return Ok(Value::Undefined);
        };
        let current = frame(world).player(id).map_or(0, |ps| ps.weapon);
        let weapons = crate::script_player::weapons(&frame(world), id, crate::script_player::WeaponList::All);
        let mut shield_of = |w: u32| w != 0 && weapon_words(world, &script, w).0 == "riotshield";
        let held = shield_of(current);
        let has = held || weapons.into_iter().any(&mut shield_of);
        let shield = if held && !carried.is_empty() {
            Some((carried, "tag_weapon_left"))
        } else if has && !stowed.is_empty() {
            Some((stowed, STOWED_TAG))
        } else {
            None
        };
        set_shield(world, id, shield);
        Ok(Value::Undefined)
    });
    // detachshieldmodel(model, tag): BO2's death path takes the carried
    // shield off his hand.
    vm.bind("detachshieldmodel", true, |vm, world, s, a| {
        let id = super::client(vm, world, s)?;
        let model = super::text(vm, a, 0);
        let on = world
            .get_resource::<Bodies>()
            .and_then(|b| b.looks.get(&id.0))
            .and_then(|l| l.shield.clone())
            .is_some_and(|(m, _)| m.eq_ignore_ascii_case(&model));
        if on {
            set_shield(world, id, None);
        }
        Ok(Value::Undefined)
    });
    vm.bind("getcorpseanim", true, |vm, world, s, _| {
        let name = super::entnum(vm, s).and_then(|n| {
            world
                .get_resource::<Bodies>()
                .and_then(|b| b.corpse_anims.get(&n).cloned())
        });
        Ok(name.map_or(Value::Undefined, |n| vm.string(&n)))
    });
    vm.bind("isragdoll", true, |_, _, _, _| Ok(Value::Int(0)));
    vm.bind("animhasnotetrack", false, |vm, world, _, a| {
        let Some(anim) = anim_name(vm, super::arg(a, 0)) else {
            return Ok(Value::Int(0));
        };
        let note = vm.to_text(super::arg(a, 1));
        let has = world
            .resource::<Zm>()
            .anims
            .get(&anim)
            .is_some_and(|x| x.notifies.iter().any(|(n, _)| n.eq_ignore_ascii_case(&note)));
        Ok(Value::bool(has))
    });
    vm.bind("getnotetracktimes", false, |vm, world, _, a| {
        let anim = anim_name(vm, super::arg(a, 0)).unwrap_or_default();
        let note = vm.to_text(super::arg(a, 1));
        let times: Vec<Value> = world
            .resource::<Zm>()
            .anims
            .get(&anim)
            .map(|x| {
                x.notifies
                    .iter()
                    .filter(|(n, _)| n.eq_ignore_ascii_case(&note))
                    .map(|(_, t)| Value::Float(*t))
                    .collect()
            })
            .unwrap_or_default();
        Ok(super::list(times))
    });
    vm.bind("getanimlength", false, |vm, world, _, a| {
        let len = anim_name(vm, super::arg(a, 0))
            .and_then(|anim| {
                let x = world.resource::<Zm>().anims.get(&anim)?.clone();
                (x.framerate > 0.0).then(|| f32::from(x.numframes) / x.framerate)
            })
            .unwrap_or(0.0);
        Ok(Value::Float(len))
    });
}
