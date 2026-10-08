//! bo2zm M3: run a Zombies map's server scripts headless, read-only.
//!
//! `t6run [map] [seconds]` (default `zm_nuked`, 30): loads the map's zones,
//! decodes and links every server script, binds the engine-free builtins and
//! a minimal stand-in engine (map entities, structs, one player), then runs
//! what the engine runs at level start: `codescripts/struct` for every
//! script_struct, the game type's `main`, the map's `main`,
//! `codecallback_startgametype`, and after a second
//! `codecallback_playerconnect` for one player; then server frames for the
//! given time. Reports script errors and every builtin the stand-in lacks,
//! most called first. Probe only: the real engine side lives in the game.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use asset_t6::{capture_zone, parse_entities};
use gsc_t6::value::ObjKind;
use gsc_t6::{ObjRef, Program, ScriptObject, Strings, Value, Vm};

const GAME: &str = r"C:\Program Files (x86)\Steam\steamapps\common\Call of Duty Black Ops II\zone\all";
const PRE: [&str; 4] = ["code_post_gfx_zm", "common_zm", "patch_zm", "patch_ui_zm"];

#[derive(Default)]
struct Mock {
    unbound: BTreeMap<String, (u64, String)>,
    ents: Vec<ObjRef>,
    player: Option<ObjRef>,
    next_entnum: u32,
    next_fx: i32,
}

fn unbound(vm: &mut Vm<Mock>, host: &mut Mock, id: u32, _self: &Value, args: &[Value]) -> Value {
    let (name, method) = vm.program.builtins[id as usize];
    let n = format!("{}{}", if method { "." } else { "" }, vm.str(name));
    let e = host.unbound.entry(n).or_insert((0, String::new()));
    if e.0 == 0 {
        e.1 = args
            .iter()
            .map(|a| vm.to_text(a))
            .collect::<Vec<_>>()
            .join(", ");
    }
    e.0 += 1;
    Value::Undefined
}

fn typed(vm: &mut Vm<Mock>, key: &str, val: &str) -> Value {
    match key {
        "origin" | "angles" => {
            let p: Vec<f32> = val
                .split_whitespace()
                .filter_map(|x| x.parse().ok())
                .collect();
            if p.len() == 3 {
                return Value::Vec3([p[0], p[1], p[2]]);
            }
            vm.string(val)
        }
        "spawnflags" | "count" | "health" => Value::Int(val.trim().parse().unwrap_or(0)),
        "radius" | "height" | "speed" => Value::Float(val.trim().parse().unwrap_or(0.0)),
        _ => vm.string(val),
    }
}

fn ents_matching(vm: &mut Vm<Mock>, host: &Mock, args: &[Value]) -> Vec<Value> {
    let want = args.first().map(|v| vm.to_text(v));
    let key = args.get(1).map(|v| vm.to_text(v).to_ascii_lowercase());
    let mut out = Vec::new();
    for &e in &host.ents {
        if !vm.alive(e) {
            continue;
        }
        let ok = match (&want, &key) {
            (Some(w), Some(k)) => {
                let k = vm.intern(k);
                let v = vm.raw_field(e, k);
                matches!(v, Value::Str(_)) && vm.to_text(&v) == *w
            }
            _ => true,
        };
        if ok {
            out.push(Value::Object(e));
        }
    }
    out
}

fn list(vals: Vec<Value>) -> Value {
    let mut a = gsc_t6::Array::new();
    for v in vals {
        a.push(v);
    }
    Value::array(a)
}

fn bind_mock(vm: &mut Vm<Mock>) {
    vm.hooks.unbound = Some(unbound);
    vm.bind("getent", false, |vm, host, _, a| {
        let v = ents_matching(vm, host, a);
        Ok(v.into_iter().next().unwrap_or_default())
    });
    vm.bind("getentarray", false, |vm, host, _, a| {
        let v = ents_matching(vm, host, a);
        Ok(list(v))
    });
    vm.bind("getplayers", false, |_, host, _, _| {
        Ok(list(
            host.player.iter().map(|p| Value::Object(*p)).collect(),
        ))
    });
    vm.bind("isplayer", false, |_, host, _, a| {
        Ok(Value::bool(
            matches!(a.first(), Some(Value::Object(o)) if Some(*o) == host.player),
        ))
    });
    vm.bind("isalive", false, |vm, _, _, a| {
        Ok(Value::bool(a.first().is_some_and(|v| vm.is_defined(v))))
    });
    vm.bind("spawn", false, |vm, host, _, a| {
        let o = vm.alloc_object(ObjKind::Entity(host.next_entnum));
        host.next_entnum += 1;
        let cls = vm.intern("classname");
        let org = vm.intern("origin");
        if let Some(c) = a.first() {
            vm.set_raw_field(o, cls, c.clone());
        }
        if let Some(p) = a.get(1) {
            vm.set_raw_field(o, org, p.clone());
        }
        host.ents.push(o);
        Ok(Value::Object(o))
    });
    vm.bind("delete", true, |vm, host, s, _| {
        if let Value::Object(o) = s {
            vm.free_object(host, *o);
        }
        Ok(Value::Undefined)
    });
    vm.bind("loadfx", false, |_, host, _, _| {
        host.next_fx += 1;
        Ok(Value::Int(host.next_fx))
    });
    for n in [
        "precachemodel",
        "precacheshader",
        "precachestring",
        "precacheitem",
        "precacherumble",
        "precachemenu",
        "precacheshellshock",
        "precacheanimstatedef",
        "precachevehicle",
        "precacheleaderboards",
        "registerclientfield",
        "setdvar",
        "makedvarserverinfo",
        "setmatchflag",
        "setmatchtalkflag",
        "setclientnamemode",
        "setvoipattachment",
    ] {
        vm.bind(n, false, |_, _, _, _| Ok(Value::Undefined));
    }
    vm.bind("getentitynumber", true, |vm, _, s, _| {
        Ok(match s {
            Value::Object(o) => match vm.kind(*o) {
                Some(ObjKind::Entity(n)) => Value::Int(n as i32),
                _ => Value::Undefined,
            },
            _ => Value::Undefined,
        })
    });
    vm.bind("sessionmodeiszombiesgame", false, |_, _, _, _| {
        Ok(Value::Int(1))
    });
    vm.bind("ismp", false, |_, _, _, _| Ok(Value::Int(0)));
    vm.bind("issplitscreen", false, |_, _, _, _| Ok(Value::Int(0)));
    vm.bind("getnumexpectedplayers", false, |_, _, _, _| {
        Ok(Value::Int(1))
    });
    vm.bind("getnumconnectedplayers", false, |_, host, _, _| {
        Ok(Value::Int(i32::from(host.player.is_some())))
    });
}

fn main() -> ExitCode {
    let map = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "zm_nuked".to_owned());
    let seconds: i64 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);
    let dir = PathBuf::from(GAME);
    let mut zones: Vec<String> = PRE.iter().map(|z| (*z).to_owned()).collect();
    if map == "zm_nuked" {
        zones.push("dlczm0_load_zm".into());
    }
    zones.push(map.clone());
    zones.push(format!("{map}_patch"));

    let t0 = std::time::Instant::now();
    let mut objects = Vec::new();
    let mut ent_text = Vec::new();
    let (mut sl, mut sr, mut il, mut ir) = (0, 0, 0, 0);
    for z in &zones {
        let path = dir.join(format!("{z}.ff"));
        if !path.is_file() {
            continue;
        }
        let cap = match capture_zone(&path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{z}: {e}");
                return ExitCode::FAILURE;
            }
        };
        ent_text.extend(cap.map_ents.iter().cloned());
        // T6RUN_ENTS=<text>: each map entity block holding it, then stop.
        if let Ok(want) = std::env::var("T6RUN_ENTS") {
            for text in &cap.map_ents {
                for block in text.split('}') {
                    if block.contains(want.as_str()) {
                        println!("{z}: {}", block.trim().replace(['\n', '\r'], " "));
                    }
                }
            }
            continue;
        }
        for (name, bytes) in cap.raw_files {
            let Some(sname) = name.strip_prefix("script:") else {
                continue;
            };
            if sname.starts_with(',') {
                continue;
            }
            match ScriptObject::parse(&bytes) {
                Ok(o) => {
                    if o.string_refs.0 != o.string_refs.1 || o.import_sites.0 != o.import_sites.1 {
                        println!(
                            "  coverage {}: strings {}/{} imports {}/{}",
                            o.name,
                            o.string_refs.1,
                            o.string_refs.0,
                            o.import_sites.1,
                            o.import_sites.0
                        );
                    }
                    sl += o.string_refs.0;
                    sr += o.string_refs.1;
                    il += o.import_sites.0;
                    ir += o.import_sites.1;
                    objects.push(o);
                }
                Err(e) => {
                    println!("DECODE FAIL {sname}: {e}");
                    return ExitCode::FAILURE;
                }
            }
        }
    }
    println!(
        "{} script objects; string refs read {sr}/{sl}, import sites read {ir}/{il} ({:.1}s)",
        objects.len(),
        t0.elapsed().as_secs_f32()
    );
    let mut strings = Strings::default();
    let program = match Program::link(objects, &mut strings) {
        Ok(p) => p,
        Err(e) => {
            println!("LINK FAIL: {e}");
            return ExitCode::FAILURE;
        }
    };
    let ops: usize = program.functions.iter().map(|f| f.code.len()).sum();
    println!(
        "linked {} scripts, {} functions, {} instructions, {} call sites, {} builtins",
        program.scripts.len(),
        program.functions.len(),
        ops,
        program.sites.len(),
        program.builtins.len()
    );
    let mut vm: Vm<Mock> = Vm::new(program, strings);
    gsc_t6::natives::bind_core(&mut vm);
    bind_mock(&mut vm);
    let mut host = Mock::default();
    for (k, v) in [
        ("mapname", map.as_str()),
        ("g_gametype", "zstandard"),
        ("ui_gametype", "zstandard"),
        ("ui_zm_gamemodegroup", "zsurvival"),
        ("ui_zm_mapstartlocation", "nuked"),
        ("sv_maxclients", "4"),
        ("party_maxplayers", "4"),
        ("onlinegame", "0"),
        ("systemlink", "0"),
        ("splitscreen", "0"),
        ("xblive_privatematch", "0"),
        ("zm_gamemodegroup", "zsurvival"),
    ] {
        vm.dvars.insert(k.to_owned(), v.to_owned());
    }

    // Map entities and structs.
    let level = Value::Object(vm.level);
    let _ = vm.spawn_named(
        &mut host,
        "codescripts/struct",
        "initstructs",
        level.clone(),
        vec![],
    );
    let createstruct = vm
        .program
        .find(&vm.strings, "codescripts/struct", "createstruct");
    let structs_s = vm.intern("struct");
    let (mut n_ents, mut n_structs) = (0, 0);
    for text in &ent_text {
        for e in parse_entities(text) {
            let cls = e.classname().to_owned();
            if cls == "script_struct" {
                let Some(cs) = createstruct else { continue };
                vm.spawn(&mut host, cs, level.clone(), vec![]);
                // The struct createstruct appended last.
                let arr = vm.raw_field(vm.level, structs_s);
                let Value::Array(a) = arr else { continue };
                let Some(Value::Object(s)) = a.get(&gsc_t6::Key::Int(a.len() as i32 - 1)) else {
                    continue;
                };
                for (k, v) in &e.fields {
                    let key = k.to_ascii_lowercase();
                    let val = typed(&mut vm, &key, v);
                    let f = vm.intern(&key);
                    vm.set_raw_field(s, f, val);
                }
                n_structs += 1;
            } else if !cls.starts_with("node_") && cls != "worldspawn" {
                let o = vm.alloc_object(ObjKind::Entity(host.next_entnum));
                host.next_entnum += 1;
                for (k, v) in &e.fields {
                    let key = k.to_ascii_lowercase();
                    let val = typed(&mut vm, &key, v);
                    let f = vm.intern(&key);
                    vm.set_raw_field(o, f, val);
                }
                host.ents.push(o);
                n_ents += 1;
            }
        }
    }
    println!("map: {n_ents} entities, {n_structs} structs");

    let steps0 = std::time::Instant::now();
    for (script, func) in [
        ("maps/mp/gametypes_zm/zstandard", "main"),
        (&*format!("maps/mp/{map}"), "main"),
        (
            "maps/mp/gametypes_zm/_callbacksetup",
            "codecallback_startgametype",
        ),
    ] {
        match vm.spawn_named(&mut host, script, func, level.clone(), vec![]) {
            Some(_) => println!("ran {script}::{func}; threads {}", vm.thread_count()),
            None => println!("NO FUNCTION {script}::{func}"),
        }
    }
    for frame in 0..(seconds * 20) {
        if frame == 20 {
            let p = vm.alloc_object(ObjKind::Entity(host.next_entnum));
            host.next_entnum += 1;
            host.player = Some(p);
            let cls = vm.intern("classname");
            let player_s = vm.string("player");
            vm.set_raw_field(p, cls, player_s);
            let r = vm.spawn_named(
                &mut host,
                "maps/mp/gametypes_zm/_callbacksetup",
                "codecallback_playerconnect",
                Value::Object(p),
                vec![],
            );
            println!(
                "player connect: {}",
                if r.is_some() { "ran" } else { "NO FUNCTION" }
            );
        }
        vm.run_frame(&mut host);
        if frame % 200 == 199 {
            println!("t={:>4}s threads {}", (frame + 1) / 20, vm.thread_count());
        }
    }
    println!(
        "ran {seconds}s of frames in {:.2}s",
        steps0.elapsed().as_secs_f32()
    );
    println!("\n== script errors ({})", vm.messages.len());
    for m in vm.messages.iter().take(60) {
        println!("  {m}");
    }
    let mut un: Vec<_> = host.unbound.iter().collect();
    un.sort_by(|a, b| b.1.0.cmp(&a.1.0));
    println!("\n== unbound builtins called ({})", un.len());
    for (n, (c, a)) in un.iter().take(150) {
        println!("  {c:>6} {n}({})", a.chars().take(60).collect::<String>());
    }
    ExitCode::SUCCESS
}
