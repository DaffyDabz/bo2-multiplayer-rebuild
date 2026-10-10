//! bo2zm M3: Black Ops II Zombies' own scripts and the engine side they call.
//!
//! A BO2 map hands its zones' compiled scripts to [`install`]; the `gsc_t6`
//! VM runs them, here, beside IW4L's GSC runtime (which then only keeps its
//! stub start-up: dvar `bo2zm_t6` set makes its connect callback step aside).
//! The level start follows the game's order: map entities and structs, the
//! game type's `main`, the map's `main`, `codecallback_startgametype`; then
//! every authority frame connects players and runs the scheduler.
//!
//! Engine state the scripts see lives in [`Zm`]: entities (map and spawned),
//! players, movers, tables. Natives are `fn(&mut Vm<World>, &mut World, self,
//! args)`. Not part of upstream IW4L.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use bevy_ecs::prelude::{Resource, World};
use gsc_t6::{Hooks, ObjKind, ObjRef, Program, ScriptObject, Strings, Value, Vm};

use crate::frame::FrameWorld;
use crate::world::ClientId;

mod actors;
mod asd;
mod autoplay;
mod bots;
mod brushes;
mod clientfields;
mod destructible;
mod engine_sounds;
mod fields;
mod globallogic;
mod hud;
mod items;
mod killstreaks;
mod loadout;
mod movers;
mod mp;
mod natives_ai;
mod natives_ent;
mod natives_fx;
mod natives_game;
mod natives_mp;
mod natives_player;
mod nav;
mod playeranim;
mod players;
mod presence;
pub mod stats;
mod triggers;
mod vehicles;
mod wallbuys;
mod zbarrier;

pub(crate) use items::collect_pickups as collect_item_pickups;

/// A string table (`mp/zombiemode.csv`, ...).
#[derive(Clone, Debug, Default)]
pub struct T6Table {
    pub columns: usize,
    pub rows: usize,
    pub cells: Vec<String>,
}

/// What a BO2 map gives the script runtime.
#[derive(Clone, Debug, Default)]
pub struct T6Install {
    pub map: String,
    pub gametype: String,
    /// Compiled script objects in zone load order (later replace earlier).
    pub objects: Vec<Vec<u8>>,
    /// String tables by lower-case name.
    pub tables: BTreeMap<String, T6Table>,
    /// The map's entity string(s).
    pub entities: Vec<String>,
    /// The map's AI path nodes, in node order.
    pub path_nodes: Vec<T6PathNode>,
    /// Animations the server times and moves actors by.
    pub anims: Vec<T6Anim>,
    /// Animation state definitions: (name, text).
    pub animstatedefs: Vec<(String, String)>,
    /// Skeletons and hit boxes of the models entities can wear, by name.
    pub models: Vec<(String, Arc<xmodel_runtime::RetainedModelCapability>)>,
    /// Clips actors are posed with for hits, by (lowercase) name.
    pub clips: Vec<(String, Arc<xmodel_runtime::AnimClip>)>,
    /// The English strings (`ZOMBIE_WEAPON_M14` -> its text).
    pub strings: std::collections::HashMap<String, String>,
    /// Every sound alias the banks hold (lower case).
    pub sound_aliases: BTreeSet<String>,
    /// bo2mp: the game type's settings (`getgametypesetting`), lower-case
    /// name -> value: `mp/gamesettings_default.cfg` then the game type's own.
    pub gamesettings: BTreeMap<String, String>,
    /// bo2mp: the lobby's bots (friends, enemies, difficulty 0-3).
    pub bots: Option<(u32, u32, u32)>,
    /// bo2mp: sound alias lengths in ms (lower-case names).
    pub sound_lengths: std::collections::HashMap<String, u32>,
    /// bo2mp: retrievable weapons (getretrievableweapons).
    pub retrievable_weapons: Vec<String>,
    /// bo2mp: each vehicle's turret and gunner weapons, and how it drives.
    pub vehicles: Vec<T6Vehicle>,
    /// bo2mp: BO2's third-person player animation script, parsed.
    pub playeranim: Option<Arc<crate::t6_playeranim::T6PlayerAnims>>,
    /// bo2mp: the destructibles the map's entities name (`destructibledef`).
    pub destructibles: Vec<Arc<xmodel_runtime::T5DestructibleDef>>,
}

/// bo2mp: a vehicle def from his zones: its guns, how it drives and how
/// its driver sees it.
#[derive(Clone, Debug, Default)]
pub struct T6Vehicle {
    pub name: String,
    pub turret: String,
    pub gunners: Vec<String>,
    pub drive: T6VehicleDrive,
}

/// bo2mp: a vehicle's driving and camera values (`VehicleDef`); speeds in
/// units a second, angles in degrees.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct T6VehicleDrive {
    /// 0: his view from the vehicle's `tag_player`; else third person.
    pub camera_mode: i32,
    pub max_speed: f32,
    pub max_speed_vertical: f32,
    pub accel: f32,
    pub accel_vertical: f32,
    /// Reversing, a share of `max_speed`.
    pub reverse_scale: f32,
    /// The third-person camera: how far back, how high it looks from, how
    /// far up and down it may look.
    pub camera_range: f32,
    pub camera_height: f32,
    pub camera_pitch: [f32; 2],
    /// 0: his own field of view.
    pub camera_fov: f32,
    /// How far he may look up and down from its gun (first person).
    pub turret_pitch: [f32; 2],
    /// `thirdPersonDriver`: the driver's seat views from outside.
    pub third_person_driver: i32,
    /// The commands of `moveUpButtonName`, `moveDownButtonName`,
    /// `switchSeatButtonName`, `attackButtonName`, `attackSecondaryButtonName`
    /// in BO2's default controller binds ("" for none).
    pub buttons: [&'static str; 5],
}

/// An AI path node and its links (node, distance, negotiation).
#[derive(Clone, Debug, Default)]
pub struct T6PathNode {
    pub ty: u32,
    pub spawnflags: u32,
    pub targetname: String,
    pub target: String,
    pub script_noteworthy: String,
    pub script_linkname: String,
    pub animscript: String,
    pub origin: [f32; 3],
    pub angle: f32,
    pub radius: f32,
    pub links: Vec<(u16, f32, bool)>,
}

/// An animation's timing, root motion and notetracks.
#[derive(Clone, Debug, Default)]
pub struct T6Anim {
    pub name: String,
    pub numframes: u16,
    pub framerate: f32,
    pub looping: bool,
    pub delta_trans: Vec<(u16, [f32; 3])>,
    pub notifies: Vec<(String, f32)>,
}

#[derive(Resource)]
pub(crate) struct T6Runtime {
    pub vm: Vm<World>,
}

/// An entity the scripts see (not a player).
#[derive(Clone, Debug, Default)]
pub(crate) struct Ent {
    pub obj: Option<ObjRef>,
    pub classname: String,
    pub origin: [f32; 3],
    pub angles: [f32; 3],
    pub model: String,
    pub hidden: bool,
    pub solid: bool,
    /// From the map (not spawned by a script).
    pub map: bool,
    /// Brush model (`*N`) a map entity uses.
    pub brush: Option<String>,
    /// Trigger radius/height (`trigger_radius*`) or brush extents.
    pub radius: f32,
    pub height: f32,
    /// A zbarrier's pieces (the magic box).
    pub zbarrier: Option<zbarrier::ZBarrier>,
    /// A box trigger (`trigger_box`, `trigger_box_use`): width (along its
    /// forward), length (right), height, centred on the origin and turned
    /// by its angles.
    pub box_dims: Option<[f32; 3]>,
    /// `usetriggerrequirelookat`: the trigger is used by looking at it
    /// (`triggers::look_hit`), not by standing in it.
    pub look_at: bool,
    pub hint: Option<String>,
    pub cursor_hint: Option<String>,
    pub link: Option<(u32, [f32; 3], [f32; 3])>,
    pub attached: Vec<(String, String)>,
    pub loop_sound: Option<String>,
    pub can_damage: bool,
    /// Players a trigger (or model) is hidden from.
    pub invisible_to: BTreeSet<u32>,
    pub invisible_to_all: bool,
    pub trigger_off: bool,
    /// bo2mp: the engine's dropped item (a gun, a scavenger bag) this is.
    pub item: Option<i32>,
    /// bo2mp: the destructible definition (`destructibledef`) a map entity
    /// breaks by, lower case.
    pub destructible: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct Player {
    pub obj: ObjRef,
    pub begun: bool,
    pub sessionstate: String,
    pub perks: BTreeSet<String>,
    /// The entity the player is linked to (`playerlinkto`): he goes where
    /// it goes.
    pub linked: Option<u32>,
    /// The knife the scripts gave him (named in a knife hit).
    pub melee_weapon: Option<String>,
    /// Down (last stand): he crawls until `reviveplayer` / `undolaststand`.
    pub laststand: bool,
    /// His weapon and weapon state last tick (weapon events).
    pub last_weapon: u32,
    pub last_wstate: i32,
    pub last_clip: i32,
    /// Whether he was diving / sprinting last tick (`dtp_start` /
    /// `dtp_end`, `sprint_begin` / `sprint_end`).
    pub last_dive: bool,
    pub last_sprint: bool,
    /// His scoreboard counts the engine keeps (headshots, downs, revives...).
    pub stats: BTreeMap<String, i32>,
    /// His HUD's client fields that are set (perks, power-ups), in the
    /// order they were set.
    pub hud_fields: Vec<(String, i32)>,
    /// A script camera (`camerasetposition` + `cameraactivate`): his view
    /// rides that entity (Nuketown's game-over rocket shot).
    pub camera: Option<u32>,
    pub camera_on: bool,
    /// bo2mp: a bot (`addtestclient`), and while its client has not come
    /// yet, when it was made.
    pub test_client: bool,
    pub pending_since: Option<i64>,
    /// bo2mp: the shellshock he is under (its name), when it ends (level
    /// ms) and the speed factor it put on `move_speed_scale_multiplier`
    /// (the shock file's `bg_shock_movement`; 1 when none).
    pub shock: Option<(String, i32)>,
    pub shock_factor: f32,
}

impl Player {
    pub(crate) fn new(obj: ObjRef) -> Self {
        Self {
            obj,
            begun: false,
            sessionstate: "spectator".to_owned(),
            perks: Default::default(),
            linked: None,
            melee_weapon: None,
            laststand: false,
            last_weapon: 0,
            last_wstate: 0,
            last_clip: 0,
            last_dive: false,
            last_sprint: false,
            stats: Default::default(),
            hud_fields: Vec::new(),
            camera: None,
            camera_on: false,
            test_client: false,
            pending_since: None,
            shock: None,
            shock_factor: 1.0,
        }
    }
}

/// Engine state for the scripts.
#[derive(Resource, Default)]
pub(crate) struct Zm {
    pub map: String,
    pub ents: BTreeMap<u32, Ent>,
    pub next_entnum: u32,
    pub players: BTreeMap<u32, Player>,
    pub tables: Arc<BTreeMap<String, T6Table>>,
    pub fx: Vec<String>,
    /// Client systems by id (`clientsysregister`): "musicCmd", ...
    pub client_sys: Vec<String>,
    /// `spawnfx` effects: name, origin, forward, up (when given).
    pub fx_ents: BTreeMap<u32, (String, [f32; 3], [f32; 3], Option<[f32; 3]>)>,
    pub unbound: BTreeMap<String, u64>,
    pub started: bool,
    pub movers: movers::Movers,
    /// Notifies to deliver later: (server ms, object, name).
    pub timers: Vec<(i64, ObjRef, String)>,
    /// bo2mp: menu answers to deliver later as the player's `menuresponse`
    /// (server ms, player, menu, response).
    pub menu_replies: Vec<(i64, ObjRef, String, String)>,
    /// Script movers showing entities' models.
    pub presences: presence::Presences,
    /// The map's AI navigation graph.
    pub nav: nav::Nav,
    /// Brush entities' collision as last sent.
    pub brush_rows: BTreeMap<u32, brushes::BrushRow>,
    /// Path node script objects, by node index.
    pub node_objs: Vec<ObjRef>,
    /// Animations by lower-case name.
    pub anims: std::collections::HashMap<String, T6Anim>,
    pub clips: std::collections::HashMap<String, Arc<xmodel_runtime::AnimClip>>,
    pub strings: std::collections::HashMap<String, String>,
    pub sound_aliases: BTreeSet<String>,
    /// bo2mp: the shellshocks the scripts started (and precached), in order (the index
    /// the player state carries is the position + 1).
    pub shock_names: Vec<String>,
    /// bo2mp: a multiplayer map (game type in `maps/mp/gametypes/`).
    pub mp: bool,
    /// bo2mp: a ranked match (BO2's `level.rankedmatch`: XP counts, the
    /// public class set; Combat Training), else a custom game.
    pub ranked_match: bool,
    /// bo2mp: the game type's settings (`getgametypesetting`).
    pub gamesettings: BTreeMap<String, String>,
    /// bo2mp: team scores (`setteamscore`), the match's end time on the
    /// server clock (`setgameendtime`, 0 = no clock), the minimap's centre.
    pub team_scores: BTreeMap<String, i32>,
    pub sound_lengths: std::collections::HashMap<String, u32>,
    /// bo2mp: retrievable weapons (getretrievableweapons).
    pub retrievable_weapons: Vec<String>,
    /// bo2mp: bots (the engine half of BO2's bot scripts), the bot clients
    /// asked for and not yet handed to the bot roster, bots to remove,
    /// bots throwing a grenade this tick, their difficulty (0 easy .. 3).
    pub bots: BTreeMap<u32, bots::Bot>,
    pub test_client_requests: Vec<u32>,
    pub bot_leaves: Vec<u32>,
    pub bot_throws: Vec<u32>,
    pub bot_difficulty: i32,
    /// bo2mp: items picked up last tick (engine records, see `items`).
    pub item_pickups: Vec<crate::ItemPickupRecord>,
    pub game_end_time: i32,
    pub map_center: [f32; 3],
    /// Script HUD elements (Game Over, Max Ammo...).
    pub huds: hud::Huds,
    /// Flying limbs and when they go.
    pub gibs: Vec<(u32, i64)>,
    /// Client-field effects waiting their time.
    pub client_fx: clientfields::ClientFx,
    /// Script vehicles on their paths (Nuketown's perk arrival).
    pub vehicles: vehicles::Vehicles,
    /// Wall buys' chalk and bought guns (BO2's client-script visuals).
    pub wallbuys: wallbuys::WallBuys,
    pub wallbuy_since: Option<i64>,
    /// The debris piles' path cuts are made (`brushes::cut_debris_paths`).
    pub debris_paths_cut: bool,
    /// What each player's HUD was last sent (round, hint, offhand,
    /// action slots).
    pub hud_sent: BTreeMap<u32, (String, String, String, String)>,
    /// bo2zm M4: each player's d-pad slots (`setactionslot(slot, "weapon",
    /// name)`): (slot, weapon or kind).
    pub action_slots: BTreeMap<u32, Vec<(i32, String)>>,
    /// The test player's fire button, pressed every other tick.
    pub autoplay_fire: bool,
    /// The round the scripts last reported (`setroundsplayed`).
    pub rounds_played: i32,
    /// Animation state definitions by file stem (`zm_nuked_basic`).
    pub asds: std::collections::HashMap<String, asd::AnimStateDef>,
    /// Actors (zombies) by entity number.
    pub actors: BTreeMap<u32, actors::Actor>,
    /// Server time of the last scheduler run.
    pub now_ms: i64,
    pub next_thread_dump: i64,
    /// Players holding use (for the press edge).
    pub use_held: BTreeSet<u32>,
    /// The hint of the use trigger each player faces.
    pub hints: BTreeMap<u32, String>,
    /// bo2mp: the destructible definitions by lower-case name.
    pub destructibles: std::collections::HashMap<String, Arc<xmodel_runtime::T5DestructibleDef>>,
}

/// Entity numbers: players are their client number; the rest start here.
pub(crate) const FIRST_ENTNUM: u32 = 64;

impl Zm {
    pub(crate) fn alloc_entnum(&mut self) -> u32 {
        if self.next_entnum < FIRST_ENTNUM {
            self.next_entnum = FIRST_ENTNUM;
        }
        let n = self.next_entnum;
        self.next_entnum += 1;
        n
    }
}

/// Install a BO2 map's scripts (before [`start`]).
pub(crate) fn install(world: &mut World, inst: T6Install) -> Result<(), String> {
    let mp = mp::is_mp(&inst.map);
    let mut objects = Vec::with_capacity(inst.objects.len() + 1);
    for bytes in &inst.objects {
        objects.push(ScriptObject::parse(bytes)?);
    }
    // bo2mp: the scripts the engine provides (multiplayer's _globallogic).
    if mp {
        objects.extend(mp::engine_scripts()?);
    }
    let mut strings = Strings::default();
    // bo2mp: Black Ops II's linker binds a bare call to the engine's own
    // function when the engine has one, before the script's or its
    // includes'. These are the multiplayer names a script also defines
    // (_spawning's clearspawnpoints() must not run _spawnlogic's, which
    // empties the spawn lists).
    const ENGINE_FIRST: &[(&str, bool)] = &[
        ("addspawnpoints", false),
        ("clearspawnpoints", false),
        ("vectorcross", false),
        ("suicide", true),
    ];
    let program = Program::link_with(objects, &mut strings, if mp { ENGINE_FIRST } else { &[] })?;
    let functions = program.functions.len();
    let builtins = program.builtins.len();
    let mut vm: Vm<World> = Vm::new(program, strings);
    // A new game rolls new dice (which perk falls next, box guns, drops): the
    // scripts' random numbers start from the clock. IW4L_T6_SEED=<n> fixes
    // them for a test that must repeat.
    vm.rng = std::env::var("IW4L_T6_SEED")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(1, |d| d.as_nanos() as u64)
        })
        .wrapping_mul(0x9e37_79b9_7f4a_7c15)
        | 1;
    gsc_t6::natives::bind_core(&mut vm);
    natives_game::bind(&mut vm);
    natives_ent::bind(&mut vm);
    natives_player::bind(&mut vm);
    if !mp {
        globallogic::bind(&mut vm);
    }
    // Sounds and effects replace the quiet stubs bound above.
    natives_fx::bind(&mut vm);
    natives_ai::bind(&mut vm);
    zbarrier::bind(&mut vm);
    clientfields::bind(&mut vm);
    brushes::bind(&mut vm);
    vehicles::bind(&mut vm);
    hud::bind(&mut vm);
    if mp {
        natives_mp::bind(&mut vm);
        stats::bind(&mut vm);
        bots::bind(&mut vm);
        killstreaks::bind(&mut vm);
        loadout::bind(&mut vm);
        items::bind(&mut vm);
        // bo2mp lane F: the body's animation natives (the death path).
        playeranim::bind(&mut vm);
    }
    // IW4L_T6_TRACE=name,name: log those builtins' calls (debugging).
    if let Ok(names) = std::env::var("IW4L_T6_TRACE") {
        vm.trace_builtins(&names, 5000);
    }
    // IW4L_T6_FTRACE=name,name: log those script functions' calls.
    if let Ok(names) = std::env::var("IW4L_T6_FTRACE") {
        vm.trace_functions(&names);
    }
    vm.hooks = Hooks {
        get_field: Some(fields::get),
        set_field: Some(fields::set),
        unbound: Some(unbound),
    };
    // bo2mp: a multiplayer match: 18 players (a full lobby), no online
    // services, ranked play off (a local match, as Combat Training).
    // BO2MP_BOTS=<friends>,<enemies>[,<difficulty 0-3>]: bots, as BO2's
    // bot setup screen sets them (a test aid until the lobby does).
    let mut bot_difficulty = 1;
    let lobby = inst.bots.map(|(f, e, d)| format!("{f},{e},{d}"));
    if mp && let Some(spec) = std::env::var("BO2MP_BOTS").ok().or(lobby) {
        let v: Vec<i32> = spec.split(',').filter_map(|s| s.trim().parse().ok()).collect();
        let (friends, enemies) = (v.first().copied().unwrap_or(0), v.get(1).copied().unwrap_or(0));
        bot_difficulty = v.get(2).copied().unwrap_or(1);
        for (k, n) in [("bot_friends", friends), ("bot_enemies", enemies), ("bot_difficulty", bot_difficulty)] {
            vm.dvars.insert(k.to_owned(), n.to_string());
        }
    }
    if mp {
        for (k, v) in [
            ("sv_maxclients", "18"),
            ("party_maxplayers", "18"),
            ("xblive_wagermatch", "0"),
            ("xblive_basictraining", "0"),
            ("xblive_privatematch", "1"),
            ("scr_xpscale", "1"),
        ] {
            vm.dvars.insert(k.to_owned(), v.to_owned());
        }
        // Combat Training (ranked, lane C): BO2's public basic-training
        // match. BO2's ranked bot branch (_bot.gsc: one bot per human
        // player, "comp stomp") stays off (sv_botsoak, its own switch): the
        // stand-in globallogic fills the teams with the playlist's bots
        // (`combattrainingbots`).
        if stats::ranked() {
            for (k, v) in [("xblive_basictraining", "1"), ("xblive_privatematch", "0"), ("sv_botsoak", "1")] {
                vm.dvars.insert(k.to_owned(), v.to_owned());
            }
        }
    }
    for (k, v) in [
        ("mapname", inst.map.as_str()),
        ("g_gametype", inst.gametype.as_str()),
        ("ui_gametype", inst.gametype.as_str()),
        ("ui_zm_gamemodegroup", "zsurvival"),
        ("ui_zm_mapstartlocation", location(&inst.map)),
        ("zm_gamemodegroup", "zsurvival"),
        ("sv_maxclients", if mp { "18" } else { "4" }),
        ("party_maxplayers", if mp { "18" } else { "4" }),
        ("onlinegame", "0"),
        ("systemlink", "0"),
        ("splitscreen", "0"),
        ("xblive_privatematch", "0"),
        ("developer", "0"),
        ("developer_script", "0"),
        ("zombie_cheat", "0"),
        ("sv_cheats", "0"),
        ("g_gameskill", "1"),
        ("scr_zm_enable_bots", "0"),
        // The box may move (teddy bear): the magic box reads it and no
        // script or config of his sets it, so the engine's own default.
        ("magic_chest_movable", "1"),
        // Client scripts own the map's placed effects and exploders (our
        // client draws the placed effects itself).
        ("cg_usingclientscripts", "1"),
    ] {
        vm.dvars.insert(k.to_owned(), v.to_owned());
    }
    diag::info!(
        Sim,
        "bo2zm t6 scripts: {} objects, {functions} functions, {builtins} builtins, {} unbound",
        inst.objects.len(),
        vm.unbound_builtins().len()
    );
    // IW4L_T6_UNBOUND=1: every builtin the scripts name that has no native.
    if std::env::var("IW4L_T6_UNBOUND").is_ok() {
        let mut names: Vec<String> = vm
            .unbound_builtins()
            .into_iter()
            .map(|(n, m)| if m { format!(".{n}") } else { n })
            .collect();
        names.sort();
        diag::info!(Sim, "bo2zm t6 unbound: {}", names.join(" "));
    }
    // Path nodes are stored where the designer placed them (Nuketown's
    // town grid floats at z 0, the ground is near -60); BO2 drops them to
    // the floor. Ours did not: every "can it walk straight there" sweep ran
    // 60 units up, over low walls, fences and cars, and zombies took those
    // routes into them and stood there (M4 retest 1).
    let mut nodes = inst.path_nodes.clone();
    actors::drop_nodes_to_floor(world, &mut nodes);
    let mut node_objs = Vec::with_capacity(nodes.len());
    for n in &nodes {
        let o = vm.alloc_object(ObjKind::Struct);
        let kind = match n.ty {
            nav::NODE_PATH => "Path",
            nav::NODE_NEGOTIATION_BEGIN => "Begin",
            nav::NODE_NEGOTIATION_END => "End",
            _ => "Path",
        };
        let fields: [(&str, Value); 4] = [
            ("origin", Value::Vec3(n.origin)),
            ("angles", Value::Vec3([0.0, n.angle, 0.0])),
            ("spawnflags", Value::Int(n.spawnflags as i32)),
            ("radius", Value::Float(n.radius)),
        ];
        for (k, v) in fields {
            let f = vm.intern(k);
            vm.set_raw_field(o, f, v);
        }
        for (k, v) in [
            ("type", kind),
            ("targetname", n.targetname.as_str()),
            ("target", n.target.as_str()),
            ("script_noteworthy", n.script_noteworthy.as_str()),
            ("script_linkname", n.script_linkname.as_str()),
            ("animscript", n.animscript.as_str()),
        ] {
            if !v.is_empty() {
                let f = vm.intern(k);
                let s = vm.string(v);
                vm.set_raw_field(o, f, s);
            }
        }
        node_objs.push(o);
    }
    let anims: std::collections::HashMap<String, T6Anim> = inst
        .anims
        .into_iter()
        .map(|a| (a.name.to_ascii_lowercase(), a))
        .collect();
    let asds = inst
        .animstatedefs
        .iter()
        .map(|(name, text)| {
            let stem = name
                .rsplit('/')
                .next()
                .unwrap_or(name)
                .trim_end_matches(".asd")
                .to_ascii_lowercase();
            (stem, asd::AnimStateDef::parse(text))
        })
        .collect();
    // IW4L_T6_ANIM=<part of a name>: those animations' timing and root motion.
    if let Ok(want) = std::env::var("IW4L_T6_ANIM") {
        let mut names: Vec<&String> = anims.keys().filter(|k| k.contains(want.as_str())).collect();
        names.sort();
        for k in names.into_iter().take(40) {
            let a = &anims[k];
            diag::info!(
                Sim,
                "bo2zm t6 anim {k}: {} frames at {} fps, looping {}, {} root keys {:?} .. {:?}, speed {:.1}, notes {:?}",
                a.numframes,
                a.framerate,
                a.looping,
                a.delta_trans.len(),
                a.delta_trans.first(),
                a.delta_trans.last(),
                actors::anim_speed(a),
                a.notifies
            );
        }
    }
    diag::info!(
        Sim,
        "bo2zm t6: {} path nodes, {} animations, {} animstatedefs",
        nodes.len(),
        anims.len(),
        inst.animstatedefs.len()
    );
    diag::info!(
        Sim,
        "bo2zm t6: {} models with hit boxes, {} actor clips",
        inst.models
            .iter()
            .filter(|(_, c)| c.bone_collision.iter().any(Option::is_some))
            .count(),
        inst.clips.len()
    );
    frame(world).add_model_capabilities(inst.models);
    world.insert_resource(Zm {
        map: inst.map.clone(),
        clips: inst.clips.into_iter().collect(),
        // Keys upper-cased: BO2 looks them up without case (the B23R's
        // chalk asks for "ZOMBIE_WEAPON_BERETTA93r", fix list 3).
        strings: inst
            .strings
            .into_iter()
            .map(|(k, v)| (k.to_ascii_uppercase(), v))
            .collect(),
        sound_aliases: inst.sound_aliases,
        mp,
        ranked_match: mp && stats::ranked(),
        bot_difficulty,
        sound_lengths: inst.sound_lengths,
        retrievable_weapons: inst.retrievable_weapons,
        gamesettings: inst.gamesettings,
        tables: Arc::new(inst.tables),
        next_entnum: FIRST_ENTNUM,
        nav: nav::Nav::new(nodes),
        node_objs,
        anims,
        asds,
        destructibles: inst
            .destructibles
            .into_iter()
            .map(|d| (d.name.to_ascii_lowercase(), d))
            .collect(),
        ..Default::default()
    });
    killstreaks::install(world, inst.vehicles);
    world.insert_resource(T6Runtime { vm });
    world.insert_resource(playeranim::Bodies::new(inst.playeranim));
    world.insert_resource(PendingEntities(inst.entities));
    Ok(())
}

#[derive(Resource)]
struct PendingEntities(Vec<String>);

fn location(map: &str) -> &'static str {
    match map {
        "zm_nuked" => "nuked",
        "zm_transit" => "transit",
        "zm_highrise" => "rooftop",
        "zm_prison" => "prison",
        "zm_buried" => "processing",
        "zm_tomb" => "tomb",
        _ => "",
    }
}

/// bo2mp: Black Ops II's killcam (`_killcam.gsc`) puts a dead player in
/// spectator with the fields the engine replays from: whom to watch
/// (`spectatorclient`), the entity that killed (`killcamentity`), how far
/// back (`archivetime`), the time offset and the length. The same seats
/// MW2's scripts fill, so the engine's own killcam replay plays it.
pub(crate) fn killcam_seats(world: &World) -> Vec<(ClientId, crate::ScriptSeat)> {
    let (Some(zm), Some(rt)) = (world.get_resource::<Zm>(), world.get_resource::<T6Runtime>()) else {
        return Vec::new();
    };
    if !zm.mp {
        return Vec::new();
    }
    let vm = &rt.vm;
    let field = |o: ObjRef, name: &str| -> Option<Value> {
        let f = vm.strings.find(name)?;
        Some(vm.raw_field(o, f))
    };
    let num = |o: ObjRef, name: &str| field(o, name).and_then(|v| v.as_float());
    let mut out = Vec::new();
    for (client, p) in &zm.players {
        if p.sessionstate != "spectator" {
            continue;
        }
        let archive = num(p.obj, "archivetime").unwrap_or(0.0);
        let spectator = num(p.obj, "spectatorclient").map_or(-1, |v| v as i32);
        if archive <= 0.0 || spectator < 0 {
            continue;
        }
        let seat = crate::ScriptSeat {
            spectator_client: spectator,
            kill_cam_entity: num(p.obj, "killcamentity").map_or(-1, |v| v as i32),
            look_at_entity: num(p.obj, "killcamentitylookat").map_or(-1, |v| v as i32),
            archive_ms: (archive * 1000.0).round() as i32,
            ps_offset_ms: num(p.obj, "psoffsettime").map_or(0, |v| v as i32),
            length_ms: (num(p.obj, "killcamlength").unwrap_or(0.0) * 1000.0).round() as i32,
        };
        out.push((ClientId(*client), seat));
    }
    out
}

/// bo2mp: bot clients asked for (`addtestclient`), drained.
pub(crate) fn take_test_clients(world: &mut World) -> Vec<u32> {
    world
        .get_resource_mut::<Zm>()
        .map(|mut z| std::mem::take(&mut z.test_client_requests))
        .unwrap_or_default()
}

/// bo2mp: bots to remove (`botleavegame`), drained.
pub(crate) fn take_bot_leaves(world: &mut World) -> Vec<u32> {
    world
        .get_resource_mut::<Zm>()
        .map(|mut z| std::mem::take(&mut z.bot_leaves))
        .unwrap_or_default()
}

/// Run `f` with the VM out of the world.
pub(crate) fn with_vm<R>(
    world: &mut World,
    f: impl FnOnce(&mut Vm<World>, &mut World) -> R,
) -> Option<R> {
    if !world.contains_resource::<T6Runtime>() {
        return None;
    }
    Some(
        world.resource_scope(|world, mut rt: bevy_ecs::prelude::Mut<T6Runtime>| {
            f(&mut rt.vm, world)
        }),
    )
}

/// bo2zm M4: a player's answer from a BO2 menu (`Engine.SendMenuResponse`:
/// "popup_leavegame" "endround", ...) reaches the scripts as his
/// `menuresponse` notify. False when no BO2 map runs.
pub(crate) fn menu_response(world: &mut World, client: u32, menu: &str, response: &str) -> bool {
    with_vm(world, |vm, world| {
        let Some(p) = world.resource::<Zm>().players.get(&client).map(|p| p.obj) else {
            return;
        };
        let (m, r) = (vm.string(menu), vm.string(response));
        diag::info!(Sim, "bo2zm t6 menuresponse from {client}: {menu} {response}");
        vm.notify_str(world, p, "menuresponse", &[m, r]);
    })
    .is_some()
}

/// The level start: map entities and structs, then the game type's and
/// the map's `main` and `codecallback_startgametype`.
pub(crate) fn start(world: &mut World) {
    let Some(PendingEntities(texts)) = world.remove_resource::<PendingEntities>() else {
        return;
    };
    let map = world.resource::<Zm>().map.clone();
    let mp_map = world.resource::<Zm>().mp;
    let gametype = with_vm(world, |vm, _| {
        vm.dvars.get("g_gametype").cloned().unwrap_or_default()
    })
    .unwrap_or_default();
    with_vm(world, |vm, world| {
        let level = Value::Object(vm.level);
        vm.spawn_named(
            world,
            "codescripts/struct",
            "initstructs",
            level.clone(),
            vec![],
        );
        natives_ent::spawn_map_entities(vm, world, &texts);
        // The engine's calls, in order. Functions no script calls are the
        // engine's (measured: zm_nuked::gamemode_callback_setup registers
        // the map's game modes, which the game type's start needs;
        // _zm::post_main; codecallback_finalizeinitialization). bo2mp: a
        // multiplayer map starts its game type, the map, then the match.
        let calls: Vec<(String, &str, bool)> = if mp_map {
            // The map's two factions first (bodies, arms, voices per team:
            // game["set_player_model"]): no script starts their teamset;
            // the engine does, for each faction zone it loaded.
            let mut v: Vec<(String, &str, bool)> = vm
                .program
                .scripts
                .iter()
                .map(|s| s.name.clone())
                .filter(|n| {
                    n.starts_with("maps/mp/teams/_teamset_") && n != "maps/mp/teams/_teamset_multiteam"
                })
                .map(|n| (n, "main", false))
                .collect();
            v.extend([
                (format!("maps/mp/gametypes/{gametype}"), "main", true),
                (format!("maps/mp/{map}"), "main", true),
                (
                    "maps/mp/gametypes/_callbacksetup".to_owned(),
                    "codecallback_startgametype",
                    true,
                ),
            ]);
            v
        } else {
            vec![
                (format!("maps/mp/{map}"), "gamemode_callback_setup", false),
                (format!("maps/mp/gametypes_zm/{gametype}"), "main", true),
                (format!("maps/mp/{map}"), "main", true),
                ("maps/mp/zombies/_zm".to_owned(), "post_main", false),
                (
                    "maps/mp/gametypes_zm/_callbacksetup".to_owned(),
                    "codecallback_startgametype",
                    true,
                ),
                (
                    "maps/mp/gametypes_zm/_callbacksetup".to_owned(),
                    "codecallback_finalizeinitialization",
                    false,
                ),
            ]
        };
        // bo2mp: the test aid's script last (BO2MP_DEBUG_GSC).
        let mut calls = calls;
        if mp_map {
            calls.push(("bo2mp/debug".to_owned(), "main", false));
        }
        for (script, func, required) in calls {
            if vm
                .spawn_named(world, &script, func, level.clone(), vec![])
                .is_none()
                && required
            {
                diag::warn!(Sim, "bo2zm t6: no {script}::{func}");
            }
        }
        diag::info!(
            Sim,
            "bo2zm t6: level started, {} threads",
            vm.thread_count()
        );
    });
    world.resource_mut::<Zm>().started = true;
    report(world);
}

/// Each authority frame: connect players, move movers, run the scheduler.
/// IW4L_T6_PROF=1: where a tick's time goes (milliseconds per tick, every
/// 5 s of game).
#[derive(Default)]
struct Prof {
    ticks: u32,
    parts: Vec<(&'static str, f64)>,
}

fn prof_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("IW4L_T6_PROF").is_some())
}

fn prof_add(name: &'static str, since: std::time::Instant) -> std::time::Instant {
    static PROF: std::sync::Mutex<Option<Prof>> = std::sync::Mutex::new(None);
    let now = std::time::Instant::now();
    if let Ok(mut p) = PROF.lock() {
        let p = p.get_or_insert_with(Prof::default);
        if name == "tick" {
            p.ticks += 1;
            if p.ticks >= 100 {
                let line: Vec<String> = p
                    .parts
                    .iter()
                    .map(|(n, ms)| format!("{n} {:.2}", ms / f64::from(p.ticks)))
                    .collect();
                diag::info!(Sim, "bo2zm t6 prof (ms per tick): {}", line.join(", "));
                *p = Prof::default();
            }
        } else {
            let ms = (now - since).as_secs_f64() * 1000.0;
            match p.parts.iter_mut().find(|(n, _)| *n == name) {
                Some(row) => row.1 += ms,
                None => p.parts.push((name, ms)),
            }
        }
    }
    now
}

pub(crate) fn advance(world: &mut World) {
    if !world.contains_resource::<T6Runtime>() {
        return;
    }
    if prof_on() {
        let t0 = std::time::Instant::now();
        advance_inner(world);
        prof_add("total", t0);
        prof_add("tick", t0);
        return;
    }
    advance_inner(world);
}

/// One step's sub-part, timed when profiling.
fn timed(name: &'static str, world: &mut World, f: impl FnOnce(&mut World)) {
    if prof_on() {
        let t0 = std::time::Instant::now();
        f(world);
        prof_add(name, t0);
    } else {
        f(world);
    }
}

fn advance_inner(world: &mut World) {
    let (advances, tick) = {
        let request = world.resource::<crate::step::StepRequest>();
        (request.reason.advances_authority_world(), request.tick)
    };
    if !advances {
        return;
    }
    if world.contains_resource::<PendingEntities>() {
        start(world);
    }
    let now = i64::from(tick.0) * i64::from(crate::MATCH_TICK_MS);
    world.resource_mut::<Zm>().now_ms = now;
    players::sync(world);
    players::weapon_events(world);
    players::shock_tick(world);
    // bo2mp: each player's third-person animations, as BO2's script picks.
    if world.resource::<Zm>().mp {
        playeranim::advance(world);
    }
    players::movement_events(world);
    if !world.resource::<Zm>().debris_paths_cut {
        brushes::cut_debris_paths(world);
    }
    // The test player's buttons go in before the triggers read them.
    autoplay::drive(world, now);
    // bo2mp: the bots' commands too.
    if world.resource::<Zm>().mp {
        bots::drive(world, now);
        // Scorestreaks once every command is in (a bot capturing a crate
        // holds use): thrown and fired projectiles reach the scripts,
        // crafts fly, turrets aim.
        killstreaks::advance(world);
    }
    autoplay::buy_test(world, now);
    autoplay::open_all_test(world, now);
    autoplay::census_test(world, now);
    autoplay::roam_test(world, now);
    autoplay::glide_test(world, now);
    autoplay::only_spawn_test(world);
    autoplay::box_test(world, now);
    autoplay::box_move_test(world, now);
    autoplay::dual_wield_test(world, now);
    autoplay::game_over_test(world, now);
    autoplay::down_test(world, now);
    autoplay::door_test(world, now);
    autoplay::walk_test(world, now);
    autoplay::floor_map(world, now);
    autoplay::kite_test(world, now);
    autoplay::face_test(world);
    autoplay::zombie_block_test(world, now);
    autoplay::solid_walk_test(world, now);
    autoplay::perk_test(world, now);
    autoplay::pap_test(world, now);
    autoplay::powerup_test(world, now);
    autoplay::fx_test(world, now);
    autoplay::look_test(world);
    vehicles::advance(world, crate::MATCH_TICK_MS as f32 / 1000.0);
    movers::advance(world, now);
    zbarrier::advance(world, now);
    wallbuys::advance(world, now);
    if world.resource::<Zm>().mp {
        items::settle(world);
    }
    timed("triggers", world, triggers::dispatch);
    timed("hud", world, |world| {
        publish_hud(world);
        hud::publish(world);
        // bo2mp lane C: his multiplayer screen's values.
        if world.resource::<Zm>().mp {
            natives_mp::publish_screen(world);
            stats::save(world, false);
        }
    });
    timed("scripts", world, |world| {
        with_vm(world, |vm, world| {
            if vm.time_ms < now {
                vm.time_ms = now;
            }
            natives_game::deliver_timers(vm, world, now);
            vm.run_due(world);
        });
    });
    timed("actors", world, |world| actors::think(world, now));
    // Limbs that have lain long enough go.
    let gone: Vec<u32> = {
        let mut zm = world.resource_mut::<Zm>();
        let (old, keep): (Vec<(u32, i64)>, Vec<(u32, i64)>) =
            zm.gibs.iter().partition(|(_, t)| *t <= now);
        zm.gibs = keep;
        old.into_iter().map(|(n, _)| n).collect()
    };
    for n in gone {
        let obj = world
            .resource_mut::<Zm>()
            .ents
            .remove(&n)
            .and_then(|e| e.obj);
        if let Some(o) = obj {
            with_vm(world, |vm, world| vm.free_object(world, o));
        }
    }
    timed("presence", world, |world| {
        presence::sync(world, now);
        destructible::flush(world);
        brushes::sync(world, now);
    });
    timed("loops+fx", world, |world| {
        presence::publish_loops(world);
        clientfields::advance(world, now);
    });

    report(world);
}

/// bo2mp: a player picked a spot on the map for a scorestreak (or backed
/// out); BO2's scripts hear it.
pub(crate) fn location_pick(world: &mut World, client: u32, picked: [u8; 3], confirm: bool) {
    if world.get_resource::<Zm>().is_some_and(|z| z.mp) {
        killstreaks::location_pick(world, client, picked, confirm);
    }
}

/// bo2mp: the console's `give killstreak`, to BO2's own scripts. False when
/// no BO2 match runs.
pub(crate) fn give_killstreak(world: &mut World, client: u32, menu_name: &str) -> bool {
    if !world.get_resource::<Zm>().is_some_and(|z| z.mp) {
        return false;
    }
    killstreaks::give(world, client, menu_name);
    true
}

/// A client dvar for one player's client (its HUD, its music).
pub(crate) fn set_client_dvar(world: &mut World, client: u32, key: &str, value: &str) {
    let mut f = frame(world);
    if f.client_meta(crate::world::ClientId(client)).is_none() {
        return;
    }
    let dvars = &mut f
        .client_meta_mut(crate::world::ClientId(client))
        .client_dvars;
    match dvars.iter_mut().find(|(k, _)| k == key) {
        Some(row) => value.clone_into(&mut row.1),
        None => dvars.push((key.to_owned(), value.to_owned())),
    }
}

fn report(world: &mut World) {
    let msgs = with_vm(world, |vm, _| std::mem::take(&mut vm.messages)).unwrap_or_default();
    for m in msgs {
        diag::warn!(Sim, "bo2zm t6: {m}");
    }
    // IW4L_T6_THREADS=<seconds>: list every waiting thread that often.
    let every = std::env::var("IW4L_T6_THREADS")
        .ok()
        .and_then(|s| s.parse::<i64>().ok());
    if let Some(every) = every {
        let now = world.resource::<Zm>().now_ms;
        let due = {
            let mut zm = world.resource_mut::<Zm>();
            if now >= zm.next_thread_dump {
                zm.next_thread_dump = now + every.max(1) * 1000;
                true
            } else {
                false
            }
        };
        if due {
            let lines = with_vm(world, |vm, _| vm.describe_threads()).unwrap_or_default();
            diag::info!(Sim, "bo2zm t6 threads at {}s: {}", now / 1000, lines.len());
            let mut counts: BTreeMap<String, usize> = BTreeMap::new();
            for l in lines {
                *counts.entry(l).or_default() += 1;
            }
            for (l, n) in counts {
                diag::info!(Sim, "bo2zm t6 thread x{n}: {l}");
            }
        }
    }
    // IW4L_T6_EVAL=level.a.b,...: those values every 5 s (debugging).
    if let Ok(paths) = std::env::var("IW4L_T6_EVAL") {
        let now = world.resource::<Zm>().now_ms;
        if now % 5000 < i64::from(crate::MATCH_TICK_MS) {
            let lines = with_vm(world, |vm, world| {
                let mut out = Vec::new();
                for path in paths.split(',') {
                    let mut parts = path.split('.');
                    let mut v = match parts.next() {
                        Some("level") => Value::Object(vm.level),
                        Some("anim") => Value::Object(vm.anim),
                        _ => continue,
                    };
                    for p in parts {
                        v = match &v {
                            Value::Object(o) => {
                                let f = vm.intern(p);
                                vm.get_field(world, *o, f)
                            }
                            Value::Array(a) => {
                                let k = match p.parse::<i32>() {
                                    Ok(i) => gsc_t6::Key::Int(i),
                                    Err(_) => gsc_t6::Key::Str(vm.intern(p)),
                                };
                                a.get(&k).unwrap_or_default()
                            }
                            _ => Value::Undefined,
                        };
                    }
                    let text = match &v {
                        Value::Array(a) => {
                            let snap = a.snapshot();
                            let items: Vec<String> = snap
                                .keys()
                                .map(|k| {
                                    let val = snap.get(&k).cloned().unwrap_or_default();
                                    format!("{}={}", vm.to_text(&k.value()), vm.to_text(&val))
                                })
                                .collect();
                            format!("[{}]", items.join(", "))
                        }
                        other => vm.to_text(other),
                    };
                    out.push(format!("{path} = {text}"));
                }
                out
            })
            .unwrap_or_default();
            for l in lines {
                diag::info!(Sim, "bo2zm t6 eval at {}s: {l}", now / 1000);
            }
        }
    }
    // IW4L_T6_ACTORS=<ms>: every actor's place, animscript and animation.
    let every = std::env::var("IW4L_T6_ACTORS")
        .ok()
        .and_then(|s| s.parse::<i64>().ok());
    if let Some(every) = every {
        let zm = world.resource::<Zm>();
        let now = zm.now_ms;
        if now % every.max(50) < i64::from(crate::MATCH_TICK_MS) {
            for (n, a) in &zm.actors {
                let e = zm.ents.get(n);
                let o = e.map_or([0.0; 3], |e| e.origin);
                let anim = a.playing.as_ref().map_or("-".to_owned(), |p| {
                    format!("{}/{}:{}", p.state, p.substate, p.anim)
                });
                let next = a.path.get(a.path_i).copied().unwrap_or([0.0; 3]);
                let goal = a.goal.unwrap_or([0.0; 3]);
                diag::info!(
                    Sim,
                    "bo2zm t6 actor {n} at {}ms: ({:.0} {:.0} {:.0}) yaw {:.0} script {} anim {} hp {} alive {} path {}/{} next ({:.0} {:.0} {:.0}) goal ({:.0} {:.0} {:.0}) scripted {} traverse {} model {}",
                    now,
                    o[0],
                    o[1],
                    o[2],
                    e.map_or(0.0, |e| e.angles[1]),
                    a.script,
                    anim,
                    a.health,
                    a.alive,
                    a.path_i,
                    a.path.len(),
                    next[0],
                    next[1],
                    next[2],
                    goal[0],
                    goal[1],
                    goal[2],
                    a.scripted.is_some(),
                    a.traverse.is_some(),
                    e.map_or("", |e| e.model.as_str())
                );
            }
        }
    }
}

fn unbound(vm: &mut Vm<World>, world: &mut World, id: u32, _self: &Value, args: &[Value]) -> Value {
    let (name, method) = vm.program.builtins[id as usize];
    let n = format!("{}{}", if method { "." } else { "" }, vm.str(name));
    let mut zm = world.resource_mut::<Zm>();
    let count = zm.unbound.entry(n.clone()).or_insert(0);
    *count += 1;
    if *count == 1 {
        let a: Vec<String> = args.iter().map(|a| vm.to_text(a)).collect();
        diag::warn!(Sim, "bo2zm t6: unbound builtin {n}({})", a.join(", "));
    }
    Value::Undefined
}

/// Black Ops II's hit locations, in the order its models' bones name them
/// (part classification) and its weapons' location multipliers run: one
/// more than the IW4 list (torso_mid), so the IW4 names would be off from
/// torso_mid on. (t6mp's own table.)
pub(crate) const T6_HITLOC_NAMES: [&str; 21] = [
    "none",
    "helmet",
    "head",
    "neck",
    "torso_upper",
    "torso_mid",
    "torso_lower",
    "right_arm_upper",
    "left_arm_upper",
    "right_arm_lower",
    "left_arm_lower",
    "right_hand",
    "left_hand",
    "right_leg_upper",
    "left_leg_upper",
    "right_leg_lower",
    "left_leg_lower",
    "right_foot",
    "left_foot",
    "gun",
    "riotshield",
];

/// bo2zm M3: engine damage on a player (fall, explosion, ...) goes to
/// the BO2 scripts' CodeCallback_PlayerDamage; false when they don't run.
pub(crate) fn player_hit(world: &mut World, hit: &crate::script_player::Hit) -> bool {
    if !world.contains_resource::<T6Runtime>() {
        return false;
    }
    // A bullet or a knife the riot shield stopped: BO2's scripts know the
    // hit location as "riotshield".
    let hitloc = if hit.hitloc == crate::bullet_collision::HITLOC_SHIELD {
        "riotshield"
    } else {
        T6_HITLOC_NAMES
            .get(usize::from(hit.hitloc))
            .copied()
            .unwrap_or("none")
    };
    let mut wname = weapon_text(world, hit.weapon);
    let mut amount = hit.amount;
    // bo2mp: BO2's melee button slashes with the knife every class carries
    // (knife_mp, given as the melee weapon; its file's iMeleeDamage 150 - a
    // kill), whatever gun is up (the guns' own iMeleeDamage is 25); the riot
    // shield bashes and a held knife stabs with their own.
    if hit.means == "MOD_MELEE" && world.resource::<Zm>().mp {
        // The gun's row already melees with the knife's damage (the combat
        // table's melee weapon); the scripts are told it was the knife.
        let f = frame(world);
        if let Some(facts) = f.combat_facts_for(hit.weapon)
            && facts.knife_model != 0
            && facts.knife_model != hit.weapon
        {
            amount = amount.max(facts.melee_damage);
            wname = crate::script_player::weapon_name(&f, facts.knife_model);
        }
    }
    if std::env::var_os("BO2MP_HITLOG").is_some() {
        diag::info!(
            Sim,
            "bo2mp hit: player {} by {:?} with {wname} ({}): {hitloc} for {amount}",
            hit.victim.0,
            hit.attacker.map(|a| a.0),
            hit.means
        );
    }
    with_vm(world, |vm, world| {
        let attacker = hit
            .attacker
            .and_then(|a| {
                world
                    .resource::<Zm>()
                    .players
                    .get(&a.0)
                    .map(|p| Value::Object(p.obj))
            })
            .unwrap_or(Value::Undefined);
        natives_ai::player_damage(
            vm,
            world,
            hit.victim.0,
            attacker.clone(),
            attacker,
            amount,
            hit.flags,
            hit.means,
            &wname,
            hit.point,
            hit.dir,
            hitloc,
        );
    });
    true
}

/// bo2zm M3: a bullet (or a knife, a blast) on a script model that shows a
/// BO2 entity: an actor takes it through CodeCallback_ActorDamage with the
/// hit location of the bone it struck (the weapon's location multiplier
/// applied, as the engine does); false when the model isn't one of ours.
pub(crate) fn entity_hit(world: &mut World, hit: &crate::script::EntityHit) -> bool {
    if !world.contains_resource::<T6Runtime>() {
        return false;
    }
    let Some(n) = world
        .resource::<Zm>()
        .presences
        .by_ent
        .iter()
        .find(|(_, s)| s.id == hit.target)
        .map(|(n, _)| *n)
    else {
        return false;
    };
    let alive = world
        .resource::<Zm>()
        .actors
        .get(&n)
        .is_some_and(|a| a.alive);
    if !alive {
        // bo2mp: a map's breakable (a Nuketown mannequin, a car) takes a
        // blast or a knife by the piece's own scale.
        let breakable = world
            .resource::<Zm>()
            .ents
            .get(&n)
            .is_some_and(|e| e.destructible.is_some());
        if breakable {
            let t = tick(world);
            crate::t5_destructible::apply_damage(
                &mut frame(world),
                t,
                crate::AuthorityModelOwner::ScriptModel(hit.target),
                hit.bone.and_then(|b| u16::try_from(b).ok()),
                hit.amount,
                destructible::kind(hit.means),
                Some(hit.dir),
                hit.attacker,
                hit.weapon,
            );
        }
        return true;
    }
    // The struck bone's hit location.
    let part = hit.bone.and_then(|bone| {
        let f = frame(world);
        f.entity_collision_capabilities()
            .iter()
            .find(|row| row.owner.script_model() == Some(hit.target))
            .and_then(|row| row.dobj.as_ref())
            .and_then(|d| d.current_collision.as_ref())
            .and_then(|c| c.bones.iter().find(|b| usize::from(b.bone) == bone))
            .map(|b| b.part_classification)
    });
    let part = part.unwrap_or(4);
    let hitloc = T6_HITLOC_NAMES
        .get(usize::from(part))
        .copied()
        .unwrap_or("none");
    // The weapon's location multiplier (not for a knife).
    let melee = hit.means == "MOD_MELEE";
    let scale = if melee {
        1.0
    } else {
        let f = frame(world);
        f.combat_facts_for(hit.weapon)
            .map_or(1.0, |facts| facts.location_scale(part))
    };
    let amount = ((hit.amount as f32) * scale).round().max(1.0) as i32;
    // A knife hit names the knife the scripts gave him.
    let weapon = if melee {
        hit.attacker
            .and_then(|a| {
                world
                    .resource::<Zm>()
                    .players
                    .get(&a.0)
                    .and_then(|p| p.melee_weapon.clone())
            })
            .unwrap_or_else(|| "knife_zm".to_owned())
    } else {
        weapon_text(world, hit.weapon)
    };
    if melee || std::env::var("IW4L_T6_HITLOG").is_ok() {
        diag::info!(
            Sim,
            "bo2zm t6 hit ent{n}: {} {weapon} {hitloc} x{scale:.2} = {amount}",
            hit.means
        );
    }
    with_vm(world, |vm, world| {
        let attacker = hit
            .attacker
            .and_then(|a| {
                world
                    .resource::<Zm>()
                    .players
                    .get(&a.0)
                    .map(|p| Value::Object(p.obj))
            })
            .unwrap_or(Value::Undefined);
        actors::damage(
            vm,
            world,
            n,
            attacker.clone(),
            attacker,
            amount,
            hit.flags,
            hit.means,
            &weapon,
            hit.point,
            hit.dir,
            hitloc,
        );
    });
    true
}

/// A script string as the player reads it: a localized key (`&"KEY"` or a
/// bare `KEY`) becomes its English text with `&&1`.. filled from `args`
/// (themselves localized when they are keys); other values print as is.
pub(crate) fn localize(vm: &Vm<World>, world: &World, v: &Value, args: &[Value]) -> String {
    let zm = world.resource::<Zm>();
    let raw = vm.to_text(v);
    let key = raw.trim_start_matches('&');
    let mut out = match zm.strings.get(&key.to_ascii_uppercase()) {
        Some(t) => t.clone(),
        None if matches!(v, Value::IStr(_)) => key.to_owned(),
        None => raw.clone(),
    };
    for (i, a) in args.iter().enumerate() {
        let text = match a {
            Value::IStr(_) => {
                let k = vm.to_text(a);
                zm.strings
                    .get(&k.trim_start_matches('&').to_ascii_uppercase())
                    .cloned()
                    .unwrap_or(k)
            }
            other => vm.to_text(other),
        };
        out = out.replace(&format!("&&{}", i + 1), &text);
    }
    out
}

/// The grenades and mines a player carries, as the HUD shows them (BO2's
/// offhand icons, one per grenade): `name:count` by `;`, lethal first.
fn offhand_counts(world: &mut World, c: u32) -> String {
    const OFFHANDS: [&str; 16] = [
        "frag_grenade_zm",
        "sticky_grenade_zm",
        "cymbal_monkey_zm",
        "claymore_zm",
        // bo2mp: a multiplayer class's lethal then tactical equipment.
        "frag_grenade_mp",
        "sticky_grenade_mp",
        "hatchet_mp",
        "claymore_mp",
        "satchel_charge_mp",
        "bouncingbetty_mp",
        "flash_grenade_mp",
        "concussion_grenade_mp",
        "willy_pete_mp",
        "emp_grenade_mp",
        "trophy_system_mp",
        "tactical_insertion_mp",
    ];
    let f = frame(world);
    let id = crate::world::ClientId(c);
    let held = crate::script_player::weapons(&f, id, crate::script_player::WeaponList::All);
    let mut out: Vec<String> = Vec::new();
    for want in OFFHANDS {
        for &w in &held {
            let name = crate::script_player::weapon_name(&f, w);
            if name == want || name.trim_end_matches("_mp") == want {
                let (clip, stock) = (
                    crate::script_player::ammo_clip(&f, id, w),
                    crate::script_player::ammo_stock(&f, id, w),
                );
                if std::env::var_os("IW4L_T6_HINTLOG").is_some() {
                    diag::info!(Sim, "bo2zm t6 offhand {want}: clip {clip} stock {stock}");
                }
                out.push(format!("{want}:{}", clip + stock));
            }
        }
    }
    out.join(";")
}

/// Each player's HUD values the scripts own go out as client dvars: the
/// round (`bo2zm_round`), the hint of the use trigger he faces
/// (`bo2zm_hint`) and his grenades (`bo2zm_offhand`); sent when they
/// change.
fn publish_hud(world: &mut World) {
    let round = with_vm(world, |vm, _| {
        let f = vm.intern("round_number");
        vm.raw_field(vm.level, f).as_int().unwrap_or(0)
    })
    .unwrap_or(0)
    .to_string();
    let clients: Vec<u32> = world.resource::<Zm>().players.keys().copied().collect();
    for c in clients {
        let hint = world
            .resource::<Zm>()
            .hints
            .get(&c)
            .cloned()
            .unwrap_or_default();
        let offhand = offhand_counts(world, c);
        let slots = world
            .resource::<Zm>()
            .action_slots
            .get(&c)
            .map(|v| v.iter().map(|(n, w)| format!("{n}:{w}")).collect::<Vec<_>>().join(";"))
            .unwrap_or_default();
        // bo2mp: the scorestreak column: his picks (once) and his momentum.
        let momentum = world
            .resource::<Zm>()
            .players
            .get(&c)
            .and_then(|p| p.stats.get("momentum").copied())
            .unwrap_or(0)
            .to_string();
        let picks = loadout::streak_picks(world);
        if !picks.is_empty() {
            let mut f = frame(world);
            if f.client_meta(crate::world::ClientId(c)).is_some() {
                let dvars = &mut f.client_meta_mut(crate::world::ClientId(c)).client_dvars;
                for (name, value) in [("bo2mp_streaks", picks), ("bo2mp_momentum", momentum)] {
                    match dvars.iter_mut().find(|(k, _)| k == name) {
                        Some(row) if row.1 != value => row.1 = value,
                        Some(_) => {}
                        None => dvars.push((name.to_owned(), value)),
                    }
                }
            }
        }
        let sent = world.resource::<Zm>().hud_sent.get(&c).cloned();
        if sent
            .as_ref()
            .is_some_and(|(r, h, o, a)| *r == round && *h == hint && *o == offhand && *a == slots)
        {
            continue;
        }
        let mut f = frame(world);
        if f.client_meta(crate::world::ClientId(c)).is_none() {
            continue;
        }
        if std::env::var_os("IW4L_T6_HINTLOG").is_some() {
            diag::info!(
                Sim,
                "bo2zm t6 hud out for {c}: round {round:?} hint {hint:?} offhand {offhand:?}"
            );
        }
        let dvars = &mut f.client_meta_mut(crate::world::ClientId(c)).client_dvars;
        for (name, value) in [
            ("bo2zm_round", round.clone()),
            ("bo2zm_hint", hint.clone()),
            ("bo2zm_offhand", offhand.clone()),
            ("bo2zm_actionslots", slots.clone()),
        ] {
            match dvars.iter_mut().find(|(k, _)| k == name) {
                Some(row) => row.1 = value,
                None => dvars.push((name.to_owned(), value)),
            }
        }
        world
            .resource_mut::<Zm>()
            .hud_sent
            .insert(c, (round.clone(), hint, offhand, slots));
    }
}

/// Put a player somewhere from the server (a script's `setorigin`, a
/// link): the teleport bit flips so his client jumps there too.
pub(crate) fn teleport_player(world: &mut World, client: crate::world::ClientId, origin: [f32; 3]) {
    let mut f = frame(world);
    f.set_origin(client, origin);
    if let Some(ps) = f.player_mut(client) {
        ps.e_flags ^= playerstate_iw4::eflags::TELEPORT;
    }
}

/// Turn a player's view from the server (`setplayerangles`): the delta
/// under his last command's angles, so his client's view turns too.
pub(crate) fn set_player_view(world: &mut World, client: crate::world::ClientId, view: [f32; 3]) {
    let mut f = frame(world);
    let cmd = f
        .old_cmd_angles_mut()
        .iter()
        .find(|(c, _)| *c == client)
        .map(|(_, a)| *a);
    if let Some(ps) = f.player_mut(client) {
        if let Some(cmd) = cmd {
            ps.delta_angles =
                std::array::from_fn(|i| view[i] - (cmd[i] as u16) as f32 * (360.0 / 65536.0));
        }
        ps.viewangles = view;
    }
}

/// bo2zm M3: the living zombies a blast at `origin` reaches: (their shown
/// model, their middle, distance), nearest first.
pub(crate) fn radius_targets(
    world: &mut World,
    origin: [f32; 3],
    radius: f32,
) -> Vec<(crate::ScriptModelId, [f32; 3], f32)> {
    if !world.contains_resource::<T6Runtime>() {
        return Vec::new();
    }
    let zm = world.resource::<Zm>();
    let mut out: Vec<(crate::ScriptModelId, [f32; 3], f32)> = zm
        .actors
        .iter()
        .filter(|(_, a)| a.alive)
        .filter_map(|(n, a)| {
            let e = zm.ents.get(n)?;
            let id = zm.presences.by_ent.get(n)?.id;
            let mid = [e.origin[0], e.origin[1], e.origin[2] + a.height * 0.5];
            let d = gsc_t6::math::length(gsc_t6::math::sub(mid, origin));
            (d < radius).then_some((id, mid, d))
        })
        .collect();
    // bo2mp: and the map's breakables, measured to their nearest part.
    out.extend(destructible::radius_targets(world, origin, radius));
    out.sort_by(|a, b| a.2.total_cmp(&b.2));
    out
}

// ---- helpers for natives -------------------------------------------------

pub(crate) fn arg(a: &[Value], i: usize) -> &Value {
    a.get(i).unwrap_or(&Value::Undefined)
}

pub(crate) fn num(a: &[Value], i: usize) -> Result<f32, String> {
    arg(a, i).as_float().ok_or_else(|| {
        format!(
            "argument {} is {}, not a number",
            i + 1,
            arg(a, i).type_name()
        )
    })
}

pub(crate) fn int(a: &[Value], i: usize) -> Result<i32, String> {
    arg(a, i).as_int().ok_or_else(|| {
        format!(
            "argument {} is {}, not a number",
            i + 1,
            arg(a, i).type_name()
        )
    })
}

pub(crate) fn vec3(a: &[Value], i: usize) -> Result<[f32; 3], String> {
    arg(a, i).as_vec3().ok_or_else(|| {
        format!(
            "argument {} is {}, not a vector",
            i + 1,
            arg(a, i).type_name()
        )
    })
}

pub(crate) fn text(vm: &Vm<World>, a: &[Value], i: usize) -> String {
    match arg(a, i) {
        Value::Undefined => String::new(),
        v => vm.to_text(v),
    }
}

pub(crate) fn flag(a: &[Value], i: usize, default: bool) -> bool {
    match arg(a, i) {
        Value::Undefined => default,
        v => gsc_t6::truthy(v),
    }
}

pub(crate) fn list(vals: Vec<Value>) -> Value {
    let mut a = gsc_t6::Array::new();
    for v in vals {
        a.push(v);
    }
    Value::array(a)
}

/// The entity number of an entity object.
pub(crate) fn entnum(vm: &Vm<World>, v: &Value) -> Option<u32> {
    match v {
        Value::Object(o) => match vm.kind(*o)? {
            ObjKind::Entity(n) => Some(n),
            _ => None,
        },
        _ => None,
    }
}

/// The client of a player object.
pub(crate) fn client(vm: &Vm<World>, world: &World, v: &Value) -> Result<ClientId, String> {
    let n = entnum(vm, v).ok_or("not an entity")?;
    if world.resource::<Zm>().players.contains_key(&n) {
        Ok(ClientId(n))
    } else {
        Err("not a player".into())
    }
}

pub(crate) fn is_player(vm: &Vm<World>, world: &World, v: &Value) -> bool {
    entnum(vm, v).is_some_and(|n| world.resource::<Zm>().players.contains_key(&n))
}

pub(crate) fn tick(world: &World) -> crate::Tick {
    world.resource::<crate::step::StepRequest>().tick
}

pub(crate) fn frame(world: &mut World) -> FrameWorld<'_> {
    FrameWorld::from_world(world)
}

/// An entity's origin (players from the sim).
pub(crate) fn origin_of(vm: &Vm<World>, world: &mut World, v: &Value) -> Option<[f32; 3]> {
    let n = entnum(vm, v)?;
    if world.resource::<Zm>().players.contains_key(&n) {
        return frame(world).player(ClientId(n)).map(|ps| ps.origin);
    }
    world.resource::<Zm>().ents.get(&n).map(|e| e.origin)
}

/// Weapon index by script name (`m1911_zm`; IW4L registers `_mp` names).
pub(crate) fn weapon(world: &mut World, name: &str) -> Result<u32, String> {
    if name == "none" || name.is_empty() {
        return Ok(0);
    }
    // bo2mp: a gun with attachments (`mk48_mp+reflex+fmj`) is its base gun
    // with its sight, if it has one (`mk48_mp+reflex`, the scope lane's
    // catalog weapon: sight model, ADS, zoom, overlay); its other
    // attachments are not drawn or modelled yet.
    let mut parts = name.split('+');
    let name = parts.next().unwrap_or(name);
    let f = frame(world);
    for att in parts {
        let base = if name.ends_with("_mp") { name.to_owned() } else { format!("{name}_mp") };
        if let Ok(w) = crate::script_player::weapon_named(&f, &format!("{base}+{att}")) {
            return Ok(w);
        }
    }
    crate::script_player::weapon_named(&f, &format!("{name}_mp"))
        .or_else(|_| crate::script_player::weapon_named(&f, name))
}

/// A weapon's script name: Zombies' `m1911_zm` (the engine's `_mp`
/// dropped); multiplayer's own names end in `_mp` (`mp7_mp`, `radar_mp`),
/// as BO2's scripts compare them.
pub(crate) fn weapon_text(world: &mut World, w: u32) -> String {
    let mp = world.get_resource::<Zm>().is_some_and(|z| z.mp);
    let f = frame(world);
    let n = crate::script_player::weapon_name(&f, w);
    if mp {
        return n;
    }
    n.strip_suffix("_mp").map_or(n.clone(), str::to_owned)
}

pub(crate) fn names(set: &BTreeSet<String>) -> String {
    set.iter().cloned().collect::<Vec<_>>().join(", ")
}
