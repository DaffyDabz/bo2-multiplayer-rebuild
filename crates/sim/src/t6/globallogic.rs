//! `maps/mp/gametypes_zm/_globallogic`: Zombies calls it (`init`, the start
//! callback, `endgame`, ...) but no zone on a PC install carries it. These
//! are OUR CODE, written from what the Zombies scripts read and wait for:
//! the level fields nothing else sets (teams, game type, flags), the
//! game-type helpers' `init`s nothing else starts, `game[]` state, then the
//! game type's own `onstartgametype` and `prematch_over`.

use bevy_ecs::prelude::World;
use gsc_t6::{Array, Key, Value, Vm};

use super::{arg, list, text};

const NS: &str = "maps/mp/gametypes_zm/_globallogic::";

fn set_level(vm: &mut Vm<World>, field: &str, v: Value) {
    let f = vm.intern(field);
    let level = vm.level;
    vm.set_raw_field(level, f, v);
}

fn level_field(vm: &mut Vm<World>, field: &str) -> Value {
    let f = vm.intern(field);
    vm.raw_field(vm.level, f)
}

fn str_array(vm: &mut Vm<World>, pairs: &[(&str, Value)]) -> Value {
    let mut a = Array::new();
    for (k, v) in pairs {
        let k = Key::Str(vm.intern(k));
        a.set(k, v.clone());
    }
    Value::array(a)
}

fn set_game(vm: &mut Vm<World>, key: &str, v: Value) {
    let k = Key::Str(vm.intern(key));
    if !matches!(vm.game, Value::Array(_)) {
        vm.game = Value::array(Array::new());
    }
    if let Value::Array(a) = &mut vm.game {
        a.make_unique();
        a.write().set(k, v);
    }
}

fn game_has(vm: &mut Vm<World>, key: &str) -> bool {
    let k = Key::Str(vm.intern(key));
    matches!(&vm.game, Value::Array(a) if a.get(&k).is_some())
}

fn init(vm: &mut Vm<World>, _world: &mut World) {
    let mapname = vm.dvars.get("mapname").cloned().unwrap_or_default();
    let gametype = vm.dvars.get("g_gametype").cloned().unwrap_or_default();
    for (k, v) in [
        ("splitscreen", 0),
        ("xenon", 0),
        ("ps3", 0),
        ("wiiu", 0),
        ("console", 0),
        ("onlinegame", 0),
        ("systemlink", 0),
        ("rankedmatch", 0),
        ("leaguematch", 0),
        ("contractsenabled", 0),
        ("teambased", 1),
        ("teamcount", 1),
        ("multiteam", 0),
        ("overrideteamscore", 0),
        ("overrideplayerscore", 0),
        ("displayhalftime", 0),
        ("displayroundendtext", 1),
        ("endgameonscorelimit", 1),
        ("endgameontimelimit", 1),
        ("scoreroundbased", 0),
        ("resetplayerscoreeveryround", 0),
        ("gameforfeited", 0),
        ("forceautoassign", 0),
        ("laststatustime", 0),
        ("lastslowprocessframe", 0),
        ("postroundtime", 7),
        // Counters _weapons and the killstreaks count up from nothing.
        ("globalshotsfired", 0),
        ("globalcrossbowfired", 0),
        ("globalkillstreaksdestroyed", 0),
        ("globallarryskilled", 0),
        ("inovertime", 0),
        ("infinalkillcam", 0),
        ("oldschool", 0),
        ("prematchperiod", 0),
        ("intermission", 0),
        ("gameended", 0),
        ("skipvote", 0),
        ("hostforcedend", 0),
        ("hostforceddraw", 0),
        ("timerstopped", 0),
        ("objidstart", 0),
        ("foreverid", 0),
        ("ingraceperiod", 0),
        ("numlives", 0),
        // No class menus in Zombies: auto-assign spawns the player.
        ("disablecac", 1),
        ("disableclassselection", 1),
        ("playerrespawndelay", 0),
        ("waverespawndelay", 0),
        ("playerforcerespawn", 0),
        ("playerqueuedrespawn", 0),
        ("useintermissionpointsonwavespawn", 0),
        ("discardtime", 0),
        ("timerpausetime", 0),
        ("disableprematchmessages", 1),
        ("hardcoremode", 0),
        ("infinalfight", 0),
        ("inprematchperiod", 0),
        ("perksenabled", 0),
        ("playedstartingmusic", 1),
        ("playermaxhealth", 100),
        ("takelivesondeath", 0),
        ("wagermatch", 0),
    ] {
        set_level(vm, k, Value::Int(v));
    }
    let s = vm.string(&mapname);
    set_level(vm, "script", s);
    let s = vm.string("CLASS_CUSTOM1");
    set_level(vm, "defaultclass", s);
    let s = vm.string(&gametype);
    set_level(vm, "gametype", s);
    let axis = vm.string("axis");
    set_level(vm, "zombie_team", axis);
    let allies = vm.string("allies");
    let axis = vm.string("axis");
    let teams = str_array(vm, &[("allies", allies), ("axis", axis)]);
    set_level(vm, "teams", teams);
    let teamindex = str_array(
        vm,
        &[
            ("neutral", Value::Int(0)),
            ("allies", Value::Int(1)),
            ("axis", Value::Int(2)),
        ],
    );
    set_level(vm, "teamindex", teamindex);
    let empty = list(Vec::new());
    let placement = str_array(
        vm,
        &[
            ("allies", empty.clone()),
            ("axis", empty.clone()),
            ("all", empty.clone()),
        ],
    );
    set_level(vm, "placement", placement);
    set_level(vm, "waswinning", empty.clone());
    let halftime = vm.string("halftime");
    set_level(vm, "halftimetype", halftime);
    let drop = vm
        .dvars
        .get("sv_maxclients")
        .and_then(|s| s.parse().ok())
        .unwrap_or(4);
    set_level(vm, "dropteam", Value::Int(drop));
    set_level(vm, "defaultoffenseradius", Value::Int(560));
    set_level(vm, "players", empty);
}

/// The game-type helpers MP's `_globallogic` starts and Zombies' copies
/// keep (`init`s no Zombies script calls), in MP's order.
const HELPER_INITS: [&str; 14] = [
    "_menus",
    "_hud",
    "_serversettings",
    "_clientids",
    "_weaponobjects",
    "_weapons",
    "_scoreboard",
    "_shellshock",
    "_damagefeedback",
    "_spectating",
    "_gameobjects",
    "_spawnlogic",
    "_spawning",
    "_globallogic_audio",
];

fn start_game_type(vm: &mut Vm<World>, world: &mut World) {
    let level = Value::Object(vm.level);
    set_level(vm, "prematchperiod", Value::Int(0));
    set_level(vm, "intermission", Value::Int(0));
    if !game_has(vm, "gamestarted") {
        for (k, v) in [
            ("allies", "cdc"),
            ("axis", "zombies"),
            ("attackers", "allies"),
            ("defenders", "axis"),
        ] {
            if !game_has(vm, k) {
                let v = vm.string(v);
                set_game(vm, k, v);
            }
        }
        let playing = vm.string("playing");
        set_game(vm, "state", playing);
        let pre = level_field(vm, "onprecachegametype");
        vm.call_value(world, &pre, level.clone(), vec![]);
        set_game(vm, "gamestarted", Value::Int(1));
        set_game(vm, "totalKills", Value::Int(0));
        let scores = str_array(vm, &[("allies", Value::Int(0)), ("axis", Value::Int(0))]);
        set_game(vm, "teamScores", scores.clone());
        set_game(vm, "totalKillsTeam", scores.clone());
        set_game(vm, "roundswon", scores);
    }
    for k in [
        "timepassed",
        "roundsplayed",
        "overtime_round",
        "switchedsides",
        "roundMillisecondsAlreadyPassed",
    ] {
        if !game_has(vm, k) {
            set_game(vm, k, Value::Int(0));
        }
    }
    for helper in HELPER_INITS {
        let script = format!("maps/mp/gametypes_zm/{helper}");
        let _ = vm.spawn_named(world, &script, "init", level.clone(), vec![]);
    }
    let start = level_field(vm, "onstartgametype");
    vm.call_value(world, &start, level.clone(), vec![]);
    let now = vm.time_ms as i32;
    set_level(vm, "starttime", Value::Int(now));
    set_level(vm, "prematch_over", Value::Int(1));
    set_level(vm, "gamestarted", Value::Int(1));
    let lv = vm.level;
    vm.notify_str(world, lv, "prematch_over", &[]);
    vm.notify_str(world, lv, "game_started", &[]);
}

fn end_game(vm: &mut Vm<World>, world: &mut World, a: &[Value]) {
    if gsc_t6::truthy(&level_field(vm, "gameended")) {
        return;
    }
    set_level(vm, "gameended", Value::Int(1));
    let post = vm.string("postgame");
    set_game(vm, "state", post);
    let reason = text(vm, a, 1);
    diag::info!(Sim, "bo2zm t6: game ended ({reason})");
    let winner = arg(a, 0).clone();
    let lv = vm.level;
    vm.notify_str(world, lv, "game_ended", &[winner]);
}

pub(super) fn bind(vm: &mut Vm<World>) {
    let b = |vm: &mut Vm<World>, name: &str, f: gsc_t6::Native<World>| {
        vm.bind(&format!("{NS}{name}"), false, f);
        vm.bind(&format!("{NS}{name}"), true, f);
    };
    b(vm, "init", |vm, world, _, _| {
        init(vm, world);
        Ok(Value::Undefined)
    });
    b(vm, "callback_startgametype", |vm, world, _, _| {
        start_game_type(vm, world);
        Ok(Value::Undefined)
    });
    b(vm, "endgame", |vm, world, _, a| {
        end_game(vm, world, a);
        Ok(Value::Undefined)
    });
    b(vm, "forceend", |vm, world, _, _| {
        end_game(vm, world, &[]);
        Ok(Value::Undefined)
    });
    b(vm, "getcurrentgamemode", |vm, _, _, _| {
        Ok(vm.string("normal"))
    });
    b(vm, "getgamelength", |vm, _, _, _| {
        let start = level_field(vm, "starttime").as_int().unwrap_or(0);
        Ok(Value::Int(vm.time_ms as i32 - start))
    });
    b(vm, "getkillstreaks", |_, _, _, _| Ok(list(Vec::new())));
    for name in [
        "blank",
        "registerfriendlyfiredelay",
        "bbplayermatchend",
        "checkscorelimit",
        "checkteamscorelimitsoon",
        "gamehistoryplayerquit",
        "incrementmatchcompletionstat",
        "killserverpc",
        "listenforgameend",
        "removedisconnectedplayerfromplacement",
        "updateteamstatus",
        "wavespawntimer",
    ] {
        b(vm, name, |_, _, _, _| Ok(Value::Undefined));
    }
}
