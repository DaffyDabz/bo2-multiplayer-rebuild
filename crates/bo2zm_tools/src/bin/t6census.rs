//! bo2zm M3: census of a Zombies map's server scripts, read-only.
//!
//! `t6census [map]` (default `zm_nuked`) loads the map's zones in load order
//! (later zones win a script name), checks every script decodes against its
//! own tables, resolves every script-to-script call, and walks from the
//! engine's entry points (the map's `main`, the game type's `main`, every
//! `codecallback_*`, the AI type and character scripts) to list the engine
//! built-ins the reachable scripts call. Full lists go to
//! `out\m3census\`.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use asset_t6::capture_zone;
use asset_t6::gsc::{GscArg, GscObject};

const GAME: &str = r"C:\Program Files (x86)\Steam\steamapps\common\Call of Duty Black Ops II\zone\all";
const PRE: [&str; 4] = ["code_post_gfx_zm", "common_zm", "patch_zm", "patch_ui_zm"];
/// Multiplayer zones read before an `mp_` map (bo2mp; the map's own zone comes last).
const PRE_MP: [&str; 5] = [
    "code_post_gfx_mp",
    "common_mp",
    "patch_mp",
    "ui_mp",
    "patch_ui_mp",
];
const OUT_ZM: &str = r"out\m3census";
const OUT_MP: &str = r"out\mpcensus";

const GET_FUNCTION: u8 = 0x15;
const CALL_BUILTIN: u8 = 0x29;
const CALL_BUILTIN_METHOD: u8 = 0x2a;
const SCRIPT_FUNCTION_CALL: u8 = 0x2e;
const SCRIPT_METHOD_CALL: u8 = 0x30;
const SCRIPT_THREAD_CALL: u8 = 0x32;
const SCRIPT_METHOD_THREAD_CALL: u8 = 0x34;

fn norm(s: &str) -> String {
    let s = s.replace('\\', "/").to_ascii_lowercase();
    s.strip_suffix(".gsc").map_or(s.clone(), str::to_owned)
}

struct Script {
    zone: String,
    size: usize,
    obj: GscObject,
    /// Lower-case export name -> export index.
    by_name: HashMap<String, usize>,
}

fn main() -> ExitCode {
    let map = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "zm_nuked".to_owned());
    let dir = PathBuf::from(std::env::var("T6CENSUS_DIR").unwrap_or_else(|_| GAME.to_owned()));
    let mp = map.starts_with("mp_");
    let pre: &[&str] = if mp { &PRE_MP } else { &PRE };
    let mut zones: Vec<String> = pre.iter().map(|z| (*z).to_owned()).collect();
    if map == "zm_nuked" {
        zones.push("dlczm0_load_zm".into());
    }
    zones.push(map.clone());
    if !mp {
        zones.push(format!("{map}_patch"));
    }
    // T6CENSUS_OUT=<dir>: where the reports go (default per mode).
    let out = std::env::var("T6CENSUS_OUT")
        .unwrap_or_else(|_| (if mp { OUT_MP } else { OUT_ZM }).to_owned());
    let out = out.as_str();
    if let Ok(list) = std::env::var("T6CENSUS_ZONES") {
        zones = list.split(',').map(str::to_owned).collect();
    }
    let find = std::env::var("T6CENSUS_FIND").ok();

    let mut scripts: BTreeMap<String, Script> = BTreeMap::new();
    let mut overrides = Vec::new();
    let mut raw_names: BTreeMap<String, (String, usize)> = BTreeMap::new();
    for z in &zones {
        let path = dir.join(format!("{z}.ff"));
        if !path.is_file() {
            println!("zone {z}: not present");
            continue;
        }
        let cap = match capture_zone(&path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{z}: {e}");
                return ExitCode::FAILURE;
            }
        };
        if std::env::var("T6CENSUS_PATHS").is_ok() && !cap.path_nodes.is_empty() {
            let nodes = &cap.path_nodes;
            let mut types: BTreeMap<u32, usize> = BTreeMap::new();
            let mut links = 0usize;
            let mut neg = 0usize;
            for n in nodes {
                *types.entry(n.ty).or_default() += 1;
                links += n.links.len();
                neg += n.links.iter().filter(|l| l.2).count();
            }
            println!(
                "PATHS {z}: {} nodes, types {types:?}, {links} links ({neg} negotiation)",
                nodes.len()
            );
            // T6CENSUS_NODES=<step>: every step-th node's place.
            if let Some(step) = std::env::var("T6CENSUS_NODES").ok().and_then(|v| v.parse::<usize>().ok()) {
                for n in nodes.iter().step_by(step.max(1)) {
                    println!("NODE {:.0} {:.0} {:.0}", n.origin[0], n.origin[1], n.origin[2]);
                }
            }
            for n in nodes.iter().filter(|n| !n.animscript.is_empty()).take(4) {
                println!(
                    "  {:?} ty {} tn '{}' target '{}' anim '{}' links {:?}",
                    n.origin,
                    n.ty,
                    n.targetname,
                    n.target,
                    n.animscript,
                    &n.links[..n.links.len().min(4)]
                );
            }
            // T6CENSUS_NEAR="x y r": every node within r (2D) of x y, in full.
            if let Some(v) = std::env::var("T6CENSUS_NEAR").ok() {
                let f: Vec<f32> = v.split_whitespace().filter_map(|t| t.parse().ok()).collect();
                if let [x, y, r] = f[..] {
                    for (i, n) in nodes.iter().enumerate() {
                        let d = ((n.origin[0] - x).powi(2) + (n.origin[1] - y).powi(2)).sqrt();
                        if d <= r {
                            println!(
                                "NEAR {i} ({:.0} {:.0} {:.0}) ty {} anim '{}' links {:?}",
                                n.origin[0], n.origin[1], n.origin[2], n.ty, n.animscript, n.links
                            );
                        }
                    }
                }
            }
            for n in nodes.iter().take(2) {
                println!(
                    "  {:?} ty {} r {} links {:?}",
                    n.origin,
                    n.ty,
                    n.radius,
                    &n.links[..n.links.len().min(6)]
                );
            }
        }
        // T6CENSUS_TABLE=<name part>: print every matching string table
        // (tab-separated rows) - bo2mp.
        if let Ok(want) = std::env::var("T6CENSUS_TABLE") {
            for t in cap.string_tables.iter().filter(|t| t.name.contains(want.as_str())) {
                println!("TABLE {z}: {} ({} x {})", t.name, t.rows, t.columns);
                for r in 0..t.rows {
                    let row = &t.cells[r * t.columns..((r + 1) * t.columns).min(t.cells.len())];
                    println!("{}", row.join("\t"));
                }
            }
        }
        let mut n = 0;
        for (name, bytes) in cap.raw_files {
            let Some(sname) = name.strip_prefix("script:") else {
                if !name.starts_with(',') {
                    if let Ok(dir) = std::env::var("T6CENSUS_RAWDUMP") {
                        let path = format!("{dir}/{}", name.replace(['/', '\\'], "__"));
                        let _ = std::fs::create_dir_all(&dir);
                        let _ = std::fs::write(path, &bytes);
                    }
                    raw_names.insert(name, (z.clone(), bytes.len()));
                }
                continue;
            };
            if let Some(f) = &find
                && sname.contains(f.as_str())
            {
                println!("FIND {z}: {sname} {} bytes", bytes.len());
            }
            if sname.starts_with(',') {
                continue;
            }
            let obj = match GscObject::parse(&bytes) {
                Ok(o) => o,
                Err(e) => {
                    println!("PARSE FAIL {z}/{sname}: {e}");
                    continue;
                }
            };
            n += 1;
            let key = norm(sname);
            if let Ok(dir) = std::env::var("T6CENSUS_DUMPALL") {
                let path = format!("{dir}/{}", sname.replace(['/', '\\'], "__"));
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::fs::write(path, &bytes);
            }
            if std::env::var("T6CENSUS_DUMP").is_ok_and(|d| d == key) {
                let _ = std::fs::create_dir_all(out);
                let _ = std::fs::write(format!("{out}/dump.bin"), &bytes);
                println!("DUMPED {key} ({} bytes)", bytes.len());
            }
            if let Some(old) = scripts.get(&key) {
                overrides.push(format!(
                    "{key}: {} ({} bytes) -> {z} ({} bytes)",
                    old.zone,
                    old.size,
                    bytes.len()
                ));
            }
            let by_name = obj
                .exports
                .iter()
                .enumerate()
                .map(|(i, e)| (e.name.to_ascii_lowercase(), i))
                .collect();
            scripts.insert(
                key,
                Script {
                    zone: z.clone(),
                    size: bytes.len(),
                    obj,
                    by_name,
                },
            );
        }
        println!("zone {z}: {n} scripts");
    }

    // `T6CENSUS_DIS=script::function`: a listing.
    if let Ok(want) = std::env::var("T6CENSUS_DIS") {
        let (k, f) = want.rsplit_once("::").unwrap_or((want.as_str(), "main"));
        let Some(s) = scripts.get(&norm(k)) else {
            println!("no script {k}");
            return ExitCode::FAILURE;
        };
        let Some(&i) = s.by_name.get(&f.to_ascii_lowercase()) else {
            println!("no function {f} in {k}");
            return ExitCode::FAILURE;
        };
        println!(
            "{k}::{f} ({} params) from {}",
            s.obj.exports[i].params, s.zone
        );
        for o in s.obj.decode(i) {
            let flags = s
                .obj
                .import_flags
                .get(&o.at)
                .map_or(String::new(), |f| format!(" [flags {f:#x}]"));
            println!(
                "  {:06x} {:<34} {:?}{flags}",
                o.at,
                asset_t6::gsc::op_name(o.op),
                o.arg
            );
        }
        return ExitCode::SUCCESS;
    }

    // `T6CENSUS_OP=0x56`: the smallest functions using that opcode.
    if let Ok(want) = std::env::var("T6CENSUS_OP") {
        let want = u8::from_str_radix(want.trim_start_matches("0x"), 16).unwrap_or(0);
        let mut hits = Vec::new();
        for (k, s) in &scripts {
            for i in 0..s.obj.exports.len() {
                let ops = s.obj.decode(i);
                if ops.iter().any(|o| o.op == want) {
                    hits.push((ops.len(), format!("{k}::{}", s.obj.exports[i].name)));
                }
            }
        }
        hits.sort();
        for (n, h) in hits.iter().take(12) {
            println!("{n}	{h}");
        }
        println!("{} functions", hits.len());
        return ExitCode::SUCCESS;
    }

    // `T6CENSUS_REF=field`: functions that take a reference to a field
    // (assign to it) by that name.
    if let Ok(want) = std::env::var("T6CENSUS_REF") {
        for (k, s) in &scripts {
            for i in 0..s.obj.exports.len() {
                let n = s
                    .obj
                    .decode(i)
                    .iter()
                    .filter(|o| {
                        (o.op == 0x21 || (std::env::var("T6CENSUS_READS").is_ok() && o.op == 0x20))
                            && matches!(&o.arg, GscArg::Str(f, _) if f.eq_ignore_ascii_case(&want))
                    })
                    .count();
                if n > 0 {
                    println!("{k}::{} x{n}", s.obj.exports[i].name);
                }
            }
        }
        return ExitCode::SUCCESS;
    }

    // Decoder check and opcode use over every function of every script.
    let (mut sh, mut sa, mut ih, mut ia, mut funcs) = (0, 0, 0, 0, 0);
    let mut opcodes: BTreeMap<u8, usize> = BTreeMap::new();
    let mut site_ops: BTreeMap<u8, usize> = BTreeMap::new();
    for s in scripts.values() {
        let (a, b, c, d) = s.obj.coverage();
        if (a != b || c != d) && std::env::var("T6CENSUS_MISMATCH").is_ok() {
            println!("MISMATCH {}: strings {a}/{b} imports {c}/{d}", s.obj.name);
        }
        sh += a;
        sa += b;
        ih += c;
        ia += d;
        funcs += s.obj.exports.len();
        for i in 0..s.obj.exports.len() {
            for o in s.obj.decode(i) {
                *opcodes.entry(o.op).or_default() += 1;
                if s.obj.imports.contains_key(&o.at) {
                    *site_ops.entry(o.op).or_default() += 1;
                }
            }
        }
    }
    println!(
        "scripts {} functions {funcs}: strings {sh}/{sa}, import sites {ih}/{ia}",
        scripts.len()
    );
    println!("opcodes used: {}", opcodes.len());
    println!("import site opcodes: {site_ops:x?}");

    // Resolve a script call from `caller`.
    let resolve = |caller: &str, name: &str| -> Option<(String, usize)> {
        let lname = name.to_ascii_lowercase();
        let lname = match lname.split_once("::") {
            Some(("", f)) => f.to_owned(),
            _ => lname,
        };
        if let Some((ns, f)) = lname.split_once("::") {
            let key = norm(ns);
            let s = scripts.get(&key)?;
            return s.by_name.get(f).map(|&i| (key, i));
        }
        let s = scripts.get(caller)?;
        if let Some(&i) = s.by_name.get(&lname) {
            return Some((caller.to_owned(), i));
        }
        for inc in &s.obj.includes {
            let key = norm(inc);
            if let Some(&i) = scripts.get(&key).and_then(|t| t.by_name.get(&lname)) {
                return Some((key, i));
            }
        }
        None
    };

    // Entry points.
    let mut entries: Vec<(String, usize)> = Vec::new();
    let map_key = format!("maps/mp/{map}");
    for (key, s) in &scripts {
        for (i, e) in s.obj.exports.iter().enumerate() {
            let n = e.name.to_ascii_lowercase();
            if s.obj.name.ends_with(".csc") {
                continue;
            }
            let entry = n.starts_with("codecallback_")
                || (key == &map_key && n == "main")
                || ((key.starts_with("maps/mp/gametypes_zm/")
                    || key.starts_with("maps/mp/gametypes/"))
                    && n == "main"
                    && !key.contains("/_"))
                || key.starts_with("aitype/")
                || key.starts_with("character/");
            if entry {
                entries.push((key.clone(), i));
            }
        }
    }
    let entry_names: BTreeSet<String> = entries
        .iter()
        .map(|(k, i)| format!("{k}::{}", scripts[k].obj.exports[*i].name))
        .collect();

    // Walk.
    let mut seen: BTreeSet<(String, usize)> = BTreeSet::new();
    let mut queue: VecDeque<(String, usize)> = entries.into_iter().collect();
    // builtin name -> (function sites, method sites, scripts)
    let mut builtins: BTreeMap<String, (usize, usize, BTreeSet<String>)> = BTreeMap::new();
    let mut unresolved: BTreeMap<String, usize> = BTreeMap::new();
    let mut pointer_builtins: BTreeSet<String> = BTreeSet::new();
    while let Some(f) = queue.pop_front() {
        if !seen.insert(f.clone()) {
            continue;
        }
        let s = &scripts[&f.0];
        for o in s.obj.decode(f.1) {
            let GscArg::Call(name, _) = &o.arg else {
                continue;
            };
            match o.op {
                CALL_BUILTIN | CALL_BUILTIN_METHOD => {
                    let e = builtins.entry(name.to_ascii_lowercase()).or_default();
                    if o.op == CALL_BUILTIN {
                        e.0 += 1;
                    } else {
                        e.1 += 1;
                    }
                    e.2.insert(f.0.clone());
                }
                GET_FUNCTION
                | SCRIPT_FUNCTION_CALL
                | SCRIPT_METHOD_CALL
                | SCRIPT_THREAD_CALL
                | SCRIPT_METHOD_THREAD_CALL => match resolve(&f.0, name) {
                    Some(t) => queue.push_back(t),
                    None if o.op == GET_FUNCTION => {
                        pointer_builtins.insert(name.to_ascii_lowercase());
                    }
                    None => {
                        let n = name.trim_start_matches("::").to_ascii_lowercase();
                        if n.contains("::") {
                            *unresolved
                                .entry(format!("{} <- {}", name, f.0))
                                .or_default() += 1;
                        } else {
                            let e = builtins.entry(n).or_default();
                            if matches!(o.op, SCRIPT_METHOD_CALL | SCRIPT_METHOD_THREAD_CALL) {
                                e.1 += 1;
                            } else {
                                e.0 += 1;
                            }
                            e.2.insert(f.0.clone());
                        }
                    }
                },
                _ => {}
            }
        }
    }
    let reach_scripts: BTreeSet<&String> = seen.iter().map(|(k, _)| k).collect();
    let nf = builtins.values().filter(|b| b.0 > 0).count();
    let nm = builtins.values().filter(|b| b.1 > 0).count();
    println!(
        "entries {}; reachable functions {} in {} scripts; builtins {} ({nf} as functions, {nm} as methods); \
         pointers to non-script names {}; unresolved script calls {}",
        entry_names.len(),
        seen.len(),
        reach_scripts.len(),
        builtins.len(),
        pointer_builtins.len(),
        unresolved.len()
    );

    // Reports.
    let _ = std::fs::create_dir_all(out);
    let mut t = String::new();
    for (k, s) in &scripts {
        let reach = seen.iter().filter(|(kk, _)| kk == k).count();
        let _ = writeln!(
            t,
            "{k}\t{}\t{}\t{} fns\t{reach} reached\tincl {}",
            s.zone,
            s.size,
            s.obj.exports.len(),
            s.obj.includes.join(",")
        );
    }
    let _ = std::fs::write(format!("{out}/scripts.txt"), t);
    let mut t = String::new();
    for (n, (f, m, ss)) in &builtins {
        let kind = match (*f > 0, *m > 0) {
            (true, true) => "both",
            (true, false) => "func",
            _ => "meth",
        };
        let _ = writeln!(
            t,
            "{kind}\t{n}\t{}\t{}",
            f + m,
            ss.iter().cloned().collect::<Vec<_>>().join(",")
        );
    }
    let _ = std::fs::write(format!("{out}/builtins.txt"), t);
    let _ = std::fs::write(
        format!("{out}/unresolved.txt"),
        unresolved
            .iter()
            .map(|(k, v)| format!("{v}\t{k}\n"))
            .collect::<String>(),
    );
    let _ = std::fs::write(
        format!("{out}/pointer_names.txt"),
        pointer_builtins
            .iter()
            .map(|k| format!("{k}\n"))
            .collect::<String>(),
    );
    let _ = std::fs::write(
        format!("{out}/entries.txt"),
        entry_names
            .iter()
            .map(|k| format!("{k}\n"))
            .collect::<String>(),
    );
    let _ = std::fs::write(
        format!("{out}/opcodes.txt"),
        opcodes
            .iter()
            .map(|(k, v)| format!("{k:#04x}\t{v}\n"))
            .collect::<String>(),
    );
    let _ = std::fs::write(format!("{out}/overrides.txt"), overrides.join("\n"));
    let _ = std::fs::write(
        format!("{out}/rawfiles.txt"),
        raw_names
            .iter()
            .map(|(k, (z, n))| format!("{k}\t{z}\t{n}\n"))
            .collect::<String>(),
    );
    let mut t = String::new();
    for (k, i) in &seen {
        let _ = writeln!(t, "{k}::{}", scripts[k].obj.exports[*i].name);
    }
    let _ = std::fs::write(format!("{out}/reached.txt"), t);
    println!("overrides {}; reports in {out}", overrides.len());
    ExitCode::SUCCESS
}
