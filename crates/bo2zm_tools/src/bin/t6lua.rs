//! bo2zm M4: read Black Ops II's UI scripts (HavokScript bytecode),
//! read-only.
//!
//! `t6lua [dir]` (default raw\patch_zm, the rawfile dump) reads
//! every `.lua` there and reports how many read exactly, their functions,
//! instructions and constants, and how often each opcode occurs.
//! `T6LUA_DIS=<name part>` lists the first matching script's functions;
//! `T6LUA_STRINGS=<name part>` prints its string constants.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let dir = std::env::args()
        .nth(1)
        .map_or_else(|| PathBuf::from(r"raw\patch_zm"), PathBuf::from);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        eprintln!("cannot read {}", dir.display());
        return ExitCode::FAILURE;
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "lua"))
        .collect();
    files.sort();
    let dis = std::env::var("T6LUA_DIS").ok();
    // T6LUA_FN=<hash>[/<index>] (as an error trace prints it).
    let fn_hash = std::env::var("T6LUA_FN").ok().and_then(|h| {
        let (h, i) = h.split_once('/').map_or((h.as_str(), None), |(h, i)| (h, i.parse::<u32>().ok()));
        u32::from_str_radix(h.trim_start_matches("0x"), 16).ok().map(|h| (h, i))
    });
    let strings = std::env::var("T6LUA_STRINGS").ok();
    let (mut ok, mut bad) = (0usize, 0usize);
    let (mut funcs, mut code, mut consts) = (0usize, 0usize, 0usize);
    let mut ops: BTreeMap<u8, usize> = BTreeMap::new();
    let mut shown_dis = false;
    let mut shown_strings = false;
    for path in &files {
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        match hks_t6::parse(&bytes) {
            Ok(chunk) => {
                ok += 1;
                let mut stack = vec![&chunk.main];
                while let Some(p) = stack.pop() {
                    funcs += 1;
                    code += p.code.len();
                    consts += p.consts.len();
                    for &w in &p.code {
                        *ops.entry(hks_t6::decode(w).op).or_default() += 1;
                    }
                    stack.extend(p.protos.iter().map(|c| &**c));
                }
                // T6LUA_FN=<hash>: the function an error trace names.
                if let Some((want, index)) = fn_hash {
                    let mut stack = vec![&chunk.main];
                    while let Some(p) = stack.pop() {
                        if p.name_hash == want && index.is_none_or(|i| i == p.index) {
                            let mut out = String::new();
                            hks_t6::disassemble_one(p, &mut out);
                            println!("== {name}
{out}");
                        }
                        stack.extend(p.protos.iter().map(|c| &**c));
                    }
                }
                if !shown_dis && dis.as_deref().is_some_and(|d| name.contains(d)) {
                    shown_dis = true;
                    let mut out = String::new();
                    hks_t6::disassemble(&chunk.main, &mut out);
                    println!("== {name}\n{out}");
                }
                if !shown_strings && strings.as_deref().is_some_and(|d| name.contains(d)) {
                    shown_strings = true;
                    let mut stack = vec![&chunk.main];
                    let mut all = Vec::new();
                    while let Some(p) = stack.pop() {
                        for k in &p.consts {
                            if let hks_t6::Const::String(s) = k {
                                all.push(String::from_utf8_lossy(s).into_owned());
                            }
                        }
                        stack.extend(p.protos.iter().map(|c| &**c));
                    }
                    println!("== {name} strings: {}", all.join(" | "));
                }
            }
            Err(e) => {
                bad += 1;
                println!("{name}: {e}");
            }
        }
    }
    println!(
        "{} scripts: {ok} read exactly, {bad} not; {funcs} functions, {code} instructions, {consts} constants",
        files.len()
    );
    // T6LUA_METHODS=1: method names the scripts call (`obj:name()`) that no
    // script assigns anywhere: the engine's element natives.
    if std::env::var_os("T6LUA_METHODS").is_some() {
        let mut called: BTreeMap<String, usize> = BTreeMap::new();
        let mut assigned = std::collections::BTreeSet::new();
        for path in &files {
            let Ok(bytes) = std::fs::read(path) else { continue };
            let Ok(chunk) = hks_t6::parse(&bytes) else { continue };
            let mut stack = vec![&chunk.main];
            while let Some(p) = stack.pop() {
                let k = |i: usize| match p.consts.get(i) {
                    Some(hks_t6::Const::String(s)) => Some(String::from_utf8_lossy(s).into_owned()),
                    _ => None,
                };
                for &w in &p.code {
                    let d = hks_t6::decode(w);
                    match hks_t6::op_name(d.op) {
                        "SELF" if d.c >= 256 => {
                            if let Some(n) = k(usize::from(d.c - 256)) {
                                *called.entry(n).or_default() += 1;
                            }
                        }
                        "SETFIELD" | "SETFIELD_R1" => {
                            if let Some(n) = k(usize::from(d.b)) {
                                assigned.insert(n);
                            }
                        }
                        _ => {}
                    }
                }
                stack.extend(p.protos.iter().map(|c| &**c));
            }
        }
        let mut vm = hks_t6::vm::Vm::new();
        hks_t6::lui::install(&mut vm);
        let natives = hks_t6::lui::native_names();
        let need: Vec<String> = called
            .iter()
            .filter(|(n, _)| !assigned.contains(*n) && !natives.contains(n))
            .map(|(n, c)| format!("{n} {c}"))
            .collect();
        println!("element natives called, not bound ({}): {}", need.len(), need.join(", "));
    }
    // T6LUA_RUN=1: run every script's top level in one VM (require loads
    // the others by module name), with stand-ins for the engine's tables.
    if std::env::var_os("T6LUA_RUN").is_some() {
        run_all(&dir, &files);
    }
    let line: Vec<String> = ops
        .iter()
        .map(|(op, n)| format!("{} {n}", hks_t6::op_name(*op)))
        .collect();
    println!("opcodes: {}", line.join(", "));
    if bad == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Nuketown Zombies' engine values: survival mode, standard rules, the HUD
/// shown.
fn nuketown_values(host: &mut hks_t6::host::Host) {
    let hud_bit = host.field("CoD", "BIT_HUD_VISIBLE").as_num().map_or(0, |n| n as i32);
    let allies = host.field("CoD", "TEAM_ALLIES").as_num().map_or(1, |n| n as i32);
    let offline = host.field("CoD", "SESSIONMODE_OFFLINE").as_num();
    let mut v = host.values.borrow_mut();
    for (k, x) in [
        ("r_fontResolution", "720"),
        ("ui_gametype", "zstandard"),
        ("g_gametype", "zstandard"),
        ("ui_zm_gamemodegroup", "zsurvival"),
        ("ui_zm_mapstartlocation", "nuked"),
        ("ui_mapname", "zm_nuked"),
        ("mapname", "zm_nuked"),
        ("sv_running", "1"),
        ("r_fullscreen", "0"),
        ("r_mode", "1280x720"),
        ("r_monitorCount", "2"),
        ("r_monitor", "0"),
    ] {
        v.dvars.insert(k.to_owned(), x.to_owned());
    }
    v.session_modes = offline.into_iter().collect();
    v.enums.insert("r_mode".to_owned(), vec!["1280x720".to_owned(), "1920x1080".to_owned()]);
    v.settings.insert("startRound".to_owned(), 1.0);
    v.team = allies;
    v.players = vec![vec!["Player".into(), "500".into(), "0".into(), "0".into(), "0".into(), "0".into()]];
    v.bits.insert(hud_bit);
}

/// bo2mp: a multiplayer dump (`...\patch_mp`, or T6LUA_MP=1) runs as the
/// multiplayer front end: `patch_ui_mp` beside it, BO2's multiplayer
/// answers (`hks_t6::mp`), his MP string tables and English text.
fn is_mp(dir: &std::path::Path) -> bool {
    std::env::var_os("T6LUA_MP").is_some()
        || dir.file_name().is_some_and(|n| n.to_string_lossy().to_ascii_lowercase().ends_with("_mp"))
}

/// His multiplayer zones' string tables and English text (T6LUA_GAME =
/// the game's `zone` folder): (localized text, tables as rows).
type ZoneData = (Vec<(String, String)>, Vec<(String, Vec<Vec<String>>)>);

fn mp_zone_data() -> ZoneData {
    let game = PathBuf::from(std::env::var("T6LUA_GAME").unwrap_or_else(|_| {
        r"C:\Program Files (x86)\Steam\steamapps\common\Call of Duty Black Ops II\zone".to_owned()
    }));
    let (mut text, mut tables) = (Vec::new(), Vec::new());
    for z in ["code_post_gfx_mp", "common_mp", "patch_mp", "patch_ui_mp"] {
        let Ok(cap) = asset_t6::capture_zone(&game.join("all").join(format!("{z}.ff"))) else {
            println!("zone {z}: not read");
            continue;
        };
        // T6LUA_MATERIALS=<name part>: the zone's materials and images
        // count, and the materials whose names hold it.
        if let Ok(want) = std::env::var("T6LUA_MATERIALS") {
            let hits: Vec<&str> = cap.materials.iter().map(|m| m.name.as_str()).filter(|n| n.contains(want.as_str())).take(12).collect();
            println!("zone {z}: {} materials, {} images; {want}: {hits:?}", cap.materials.len(), cap.images.len());
        }
        for t in cap.string_tables {
            let rows: Vec<Vec<String>> = t.cells.chunks(t.columns.max(1)).map(<[String]>::to_vec).collect();
            tables.push((t.name, rows));
        }
    }
    for z in ["en_code_post_gfx_mp", "en_common_mp", "en_ui_mp", "en_patch_mp", "en_patch_ui_mp"] {
        if let Ok(cap) = asset_t6::capture_zone(&game.join("english").join(format!("{z}.ff"))) {
            text.extend(cap.localize);
        }
    }
    println!("mp zones: {} strings, {} string tables", text.len(), tables.len());
    // T6LUA_LOCALIZE=<key part>: the English strings whose keys hold it.
    if let Ok(want) = std::env::var("T6LUA_LOCALIZE") {
        for (k, t) in text.iter().filter(|(k, _)| k.contains(want.as_str())).take(40) {
            println!("  loc {k} = {t:?}");
        }
    }
    // T6LUA_TABLES_OUT=<dir>: each table as a tab-separated file.
    if let Ok(out) = std::env::var("T6LUA_TABLES_OUT") {
        let _ = std::fs::create_dir_all(&out);
        for (name, rows) in &tables {
            let body: Vec<String> = rows.iter().map(|r| r.join("\t")).collect();
            let _ = std::fs::write(format!("{out}/{}", name.replace(['/', '\\'], "__")), body.join("\n"));
        }
    }
    (text, tables)
}

/// A host for the scripts: zombies' Nuketown values, or (bo2mp) the
/// multiplayer front end's.
fn new_host(dir: &std::path::Path, files: &[PathBuf], mp: Option<&ZoneData>) -> hks_t6::host::Host {
    // Text width: half the height per character (the harness has no fonts).
    let measure = Box::new(|t: &str, _: &str, h: f32| t.chars().count() as f32 * h * 0.5);
    let mut host = hks_t6::host::Host::new(scripts_of(dir, files), measure);
    if let Some((text, tables)) = mp {
        hks_t6::mp::install(&mut host);
        let mut v = host.values.borrow_mut();
        v.localize = text.iter().map(|(k, t)| (k.to_ascii_uppercase(), t.clone())).collect();
        v.tables = tables.iter().cloned().collect();
    }
    host.load_base();
    if mp.is_some() {
        hks_t6::mp::after_base(&mut host);
        mp_values(&mut host);
    } else {
        nuketown_values(&mut host);
    }
    host
}

/// The multiplayer front end's engine values: no map running, his PC, an
/// offline (local) session, Team Deathmatch on Nuketown 2025 selected.
fn mp_values(host: &mut hks_t6::host::Host) {
    let offline = host.field("CoD", "SESSIONMODE_OFFLINE").as_num();
    let mut v = host.values.borrow_mut();
    for (k, x) in [
        ("r_fontResolution", "720"),
        ("ui_gametype", "tdm"),
        ("ui_gameType", "tdm"),
        ("g_gametype", "tdm"),
        ("ui_mapname", "mp_nuketown_2020"),
        ("sv_running", "0"),
        ("r_fullscreen", "0"),
        ("r_mode", "1280x720"),
        ("r_monitorCount", "2"),
        ("r_monitor", "0"),
    ] {
        v.dvars.insert(k.to_owned(), x.to_owned());
    }
    v.session_modes = offline.into_iter().collect();
    v.enums.insert("r_mode".to_owned(), vec!["1280x720".to_owned(), "1920x1080".to_owned()]);
    v.players = vec![vec!["Player".into()]];
}

/// The scripts in `dir` and, after it, its sibling `patch_ui_zm` (or
/// `patch_ui_mp` for a multiplayer dump; a later zone wins a name).
fn scripts_of(dir: &std::path::Path, files: &[PathBuf]) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut all: Vec<PathBuf> = files.to_vec();
    let sibling = if is_mp(dir) { "patch_ui_mp" } else { "patch_ui_zm" };
    if let Some(ui) = dir.parent().map(|p| p.join(sibling))
        && let Ok(rd) = std::fs::read_dir(&ui)
        && ui != dir
    {
        let mut more: Vec<PathBuf> = rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "lua"))
            .collect();
        more.sort();
        all.extend(more);
    }
    for p in all {
        if let (Some(n), Ok(b)) = (p.file_name(), std::fs::read(&p)) {
            out.push((n.to_string_lossy().into_owned(), b));
        }
    }
    out
}

fn run_all(dir: &std::path::Path, files: &[PathBuf]) {
    use hks_t6::value::Value;
    let zone_data = is_mp(dir).then(mp_zone_data);
    let mut host = new_host(dir, files, zone_data.as_ref());
    host.step_budget = 50_000_000;
    let (mut ran, mut failed) = (0usize, 0usize);
    let mut errors: BTreeMap<String, usize> = BTreeMap::new();
    for path in files {
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let Ok(chunk) = hks_t6::parse(&bytes) else {
            continue;
        };
        let f = host.vm.load(chunk.main);
        host.vm.steps = 0;
        let r = host.vm.call(f, vec![]);
        // One VM runs every script here (the engine loads a few): a script
        // that locks globals (the HUD's DisableGlobals) must not stop the
        // next ones.
        let enable = host.field("LUI", "EnableGlobals");
        if enable.truthy() {
            let _ = host.vm.call(enable, vec![]);
        }
        match r {
            Ok(_) => ran += 1,
            Err(e) => {
                failed += 1;
                let short: String = e.msg.chars().take(90).collect();
                *errors.entry(short.clone()).or_insert(0) += 1;
                if failed <= 15 {
                    println!("run {name}: {e}");
                }
            }
        }
    }
    println!("ran {ran} top levels, {failed} failed");
    let missing = host.missing.borrow().clone();
    if !missing.is_empty() {
        println!("require found no script for ({}): {}", missing.len(), missing.join(" "));
    }
    // The menus the scripts registered (LUI.createMenu.<name>).
    if let Value::Table(t) = host.field("LUI", "createMenu") {
        let mut names = Vec::new();
        let mut k = Value::Nil;
        while let Some((key, _)) = t.borrow().next(&k) {
            names.push(key.to_string());
            k = key;
        }
        names.sort();
        println!("menus ({}): {}", names.len(), names.join(" "));
    }
    if let Ok(spec) = std::env::var("T6LUA_MENU") {
        // A fresh VM loaded as the engine loads a game: the base, then the
        // HUD's scripts (T6LUA_LOAD, default T6.HUD) - re-running every
        // top level above resets LUI's tables.
        // bo2mp: the front end loads T6.Main (its menus require the rest).
        let mut game = new_host(dir, files, zone_data.as_ref());
        let base = if zone_data.is_some() { "T6.Main" } else { "T6.HUD" };
        let load = std::env::var("T6LUA_LOAD").unwrap_or_else(|_| base.to_owned());
        // The engine's roots exist before the front end's scripts load
        // (CheckClasses adds its stats checker to UIRootFull at load).
        if zone_data.is_some() {
            game.root(16.0 / 9.0);
        }
        for m in load.split(',') {
            game.require(m);
        }
        build_menu(&mut game, &spec);
        let missing = game.missing.borrow().clone();
        println!("game require found no script for ({}): {}", missing.len(), missing.join(" "));
        for e in game.errors.iter().take(20) {
            println!("  game script error: {e}");
        }
        let a = game.asked.borrow();
        let mut v: Vec<(&String, &usize)> = a.iter().collect();
        v.sort_by(|x, y| y.1.cmp(x.1));
        let top: Vec<String> = v.into_iter().take(60).map(|(k, n)| format!("{k} {n}")).collect();
        println!("game engine fields asked, unanswered: {}", top.join(", "));
    }
    for (e, n) in errors.iter().take(20) {
        println!("  {n} x {e}");
    }
    for e in host.errors.iter().take(20) {
        println!("  script error: {e}");
    }
    let top: Vec<String> = {
        let a = host.asked.borrow();
        let mut v: Vec<(&String, &usize)> = a.iter().collect();
        v.sort_by(|x, y| y.1.cmp(x.1));
        v.into_iter()
            .take(40)
            .map(|(k, n)| format!("{k} {n}"))
            .collect()
    };
    println!("engine fields asked, unanswered: {}", top.join(", "));
}

/// T6LUA_MENU=<name>[:<event>,...]: open that menu on a 16:9 root as the
/// engine does (`addmenu`), send the root the events, run T6LUA_MS (default
/// 2000) of frames and print what is drawn. An event is `name=value` (its
/// newValue) or `name/field=value/...`.
fn build_menu(host: &mut hks_t6::host::Host, spec: &str) {
    use hks_t6::value::Value;
    let (name, events) = spec.split_once(':').unwrap_or((spec, ""));
    if host.root(16.0 / 9.0).is_none() {
        println!("menu {name}: no root");
        return;
    }
    host.open_menu(name);
    // Events split on top-level ',' (braces nest); within one, '/' splits
    // the fields. `@<ms>:` in front sends it at that time.
    let mut timed: Vec<(f64, String)> = split_top(events, ',')
        .into_iter()
        .map(|ev| match ev.strip_prefix('@').and_then(|r| r.split_once(':')) {
            Some((ms, rest)) => (ms.parse::<f64>().unwrap_or(0.0), rest.to_owned()),
            None => (0.0, ev),
        })
        .collect();
    timed.sort_by(|a, b| a.0.total_cmp(&b.0));
    let send = |host: &mut hks_t6::host::Host, ev: &str| {
        let mut parts = split_top(ev, '/').into_iter();
        let head = parts.next().unwrap_or_default();
        let (ev, val) = head.split_once('=').unwrap_or((head.as_str(), "1"));
        let mut fields: Vec<(String, Value)> = vec![("newValue".to_owned(), lua_value(val))];
        for f in parts {
            let (k, v) = f.split_once('=').unwrap_or((f.as_str(), "1"));
            fields.push((k.to_owned(), lua_value(v)));
        }
        let refs: Vec<(&str, Value)> = fields.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        host.root_event(ev, &refs);
    };
    let run_ms = std::env::var("T6LUA_MS").ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(2000.0);
    // T6LUA_WATCH=<id>: that element's alpha every frame.
    let watch = std::env::var("T6LUA_WATCH").ok().and_then(|v| v.parse::<usize>().ok());
    let mut next = 0;
    let mut t = 0.0;
    while t <= run_ms {
        while next < timed.len() && timed[next].0 <= t {
            if std::env::var_os("T6LUA_CALLS").is_some() {
                println!("== event {} at {t}", timed[next].1);
            }
            send(host, &timed[next].1.clone());
            next += 1;
        }
        host.frame(t);
        // Lay out every frame, as the game draws (the mouse hit-tests the
        // last layout).
        let _ = host.drawn();
        if let Some(id) = watch
            && let Some(d) = host.drawn().into_iter().find(|d| d.id == id)
        {
            println!("  t {t:>6.0} #{id} alpha {:.2}", d.alpha);
        }
        t += 16.0;
    }
    // T6LUA_PROBE=<id>.<field>.<field>...: a field path on an element.
    if let Ok(probe) = std::env::var("T6LUA_PROBE") {
        let mut parts = probe.split('.');
        let head = parts.next().unwrap_or("");
        // `G.<global>...` starts from a global instead.
        let (mut v, mut path) = if head == "G" {
            let g = parts.next().unwrap_or("");
            (host.vm.global(g), g.to_owned())
        } else {
            let id = head.parse::<usize>().unwrap_or(0);
            (host.element_by_id(id).unwrap_or(Value::Nil), format!("#{id}"))
        };
        for f in parts {
            v = host.vm.index(&v, &Value::str(f)).unwrap_or(Value::Nil);
            path.push('.');
            path.push_str(f);
            println!("  probe {path} = {v}");
        }
        let g = host.field("LUI", "UIElement");
        let gb = host.vm.index(&g, &Value::str("GamepadButton")).unwrap_or(Value::Nil);
        println!("  probe LUI.UIElement.GamepadButton = {gb}");
    }
    if std::env::var_os("T6LUA_TREE").is_some() {
        for line in host.tree() {
            println!("  tree {line}");
        }
    }
    let drawn = host.drawn();
    let shown = drawn.iter().filter(|d| d.alpha > 0.01).count();
    println!("menu {name}: {} elements, {shown} visible", drawn.len());
    let all = std::env::var_os("T6LUA_ALL").is_some();
    for d in drawn.iter().filter(|d| all || (d.alpha > 0.01 && (d.material.is_some() || d.text.is_some()))).take(if all { 2000 } else { 80 }) {
        println!(
            "  #{:<4} {:<6} ({:.0} {:.0} {:.0} {:.0}) a {:.2} rgb ({:.2} {:.2} {:.2}) {}{}",
            d.id,
            d.kind,
            d.rect[0],
            d.rect[1],
            d.rect[2],
            d.rect[3],
            d.alpha,
            d.rgb[0],
            d.rgb[1],
            d.rgb[2],
            d.material.as_deref().map(|m| format!("image {m} ")).unwrap_or_default(),
            d.text.as_deref().map(|t| format!("text {t:?} font {}", d.font.as_deref().unwrap_or("-"))).unwrap_or_default(),
        );
    }
}

/// `s` split on `sep` outside braces.
fn split_top(s: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let (mut depth, mut cur) = (0i32, String::new());
    for c in s.chars() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            _ => {}
        }
        if c == sep && depth == 0 {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// A value: `true`, `false`, a number, `{a;k=v;...}` (a table: positional
/// items and named fields, `;` between) or else a string.
fn lua_value(v: &str) -> hks_t6::value::Value {
    use hks_t6::value::{Table, Value};
    let v = v.trim();
    if let Some(inner) = v.strip_prefix('{').and_then(|x| x.strip_suffix('}')) {
        let t = Table::new_ref();
        let mut n = 0;
        for item in split_top(inner, ';') {
            match item.split_once('=') {
                Some((k, x)) if !k.contains('{') => {
                    // A number key is a number (`4={...}`).
                    let key = k.trim().parse::<f32>().map_or_else(|_| Value::str(k.trim()), Value::Num);
                    t.borrow_mut().set(key, lua_value(x));
                }
                _ => {
                    n += 1;
                    t.borrow_mut().set(Value::Num(n as f32), lua_value(&item));
                }
            }
        }
        return Value::Table(t);
    }
    match v {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        _ => v.parse::<f32>().map_or_else(|_| Value::str(v), Value::Num),
    }
}
