//! bo2zm: read a Black Ops II map's compiled scripts, read-only.
//! `t6gsc [zone.ff]...` (default the Nuketown patch zone) checks the
//! decoder on every script (string references and import call sites read
//! where the object's tables say) and prints what the map scripts say
//! about effects and ambience. `T6GSC_RAW_OUT=<dir>` also writes every raw
//! file there.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::process::ExitCode;

use asset_t6::{GscObject, MapScriptFacts, capture_zone};

const DEFAULT_ZONE: &str =
    r"C:\Program Files (x86)\Steam\steamapps\common\Call of Duty Black Ops II\zone\all\zm_nuked_patch.ff";

/// `T6GSC_DIS=<compiled .gsc>` [`T6GSC_FN=<function>`]: print the file's
/// functions as decoded instructions (reading BO2's own scripts).
fn disassemble(path: &str) -> ExitCode {
    let Ok(bytes) = std::fs::read(path) else {
        eprintln!("cannot read {path}");
        return ExitCode::FAILURE;
    };
    let obj = match gsc_t6::ScriptObject::parse(&bytes) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let want = std::env::var("T6GSC_FN").ok();
    for f in &obj.functions {
        if want.as_deref().is_some_and(|w| !f.name.eq_ignore_ascii_case(w)) {
            continue;
        }
        println!("function {}({} params)", f.name, f.params);
        for (at, insn) in &f.code {
            let text = match insn {
                gsc_t6::Insn::Plain(op) => gsc_t6::op_name(*op).to_owned(),
                gsc_t6::Insn::Int(op, n) => format!("{} {n}", gsc_t6::op_name(*op)),
                gsc_t6::Insn::Str(op, t) => format!("{} \"{t}\"", gsc_t6::op_name(*op)),
                gsc_t6::Insn::Call(op, i) => format!("{} {}({})", gsc_t6::op_name(*op), i.full(), i.params),
                gsc_t6::Insn::Jump(op, to) => format!("{} -> {to:#x}", gsc_t6::op_name(*op)),
                other => format!("{other:?}"),
            };
            println!("  {at:#06x} {text}");
        }
    }
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    if let Ok(path) = std::env::var("T6GSC_DIS") {
        return disassemble(&path);
    }
    let mut zones: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    if zones.is_empty() {
        zones.push(PathBuf::from(DEFAULT_ZONE));
    }
    let mut scripts: HashMap<String, Vec<u8>> = HashMap::new();
    for zone in &zones {
        match capture_zone(zone) {
            Ok(c) => {
                // T6GSC_RAW_OUT=<dir>: write every raw file (UI scripts,
                // tables, configs) there, `/` as `__` (a later zone wins).
                if let Ok(dir) = std::env::var("T6GSC_RAW_OUT") {
                    for (name, bytes) in &c.raw_files {
                        let flat = name.trim_start_matches("script:").replace(['/', '\\'], "__");
                        let _ = std::fs::write(std::path::Path::new(&dir).join(flat), bytes);
                    }
                    for t in &c.string_tables {
                        let text: String = t
                            .cells
                            .chunks(t.columns.max(1))
                            .map(|row| row.join(",") + "
")
                            .collect();
                        let _ = std::fs::write(std::path::Path::new(&dir).join(t.name.replace(['/', '\\'], "__")), text);
                    }
                    println!("{}: {} raw files, {} string tables written", zone.display(), c.raw_files.len(), c.string_tables.len());
                }
                for (name, bytes) in c.raw_files {
                    if let Some(name) = name.strip_prefix("script:") {
                        scripts.entry(name.to_owned()).or_insert(bytes);
                    }
                }
            }
            Err(e) => {
                eprintln!("{}: {e}", zone.display());
                return ExitCode::FAILURE;
            }
        }
    }
    let (mut s_hit, mut s_all, mut i_hit, mut i_all, mut bad) = (0, 0, 0, 0, 0);
    let sorted: BTreeMap<_, _> = scripts.iter().collect();
    for (name, bytes) in &sorted {
        if name.starts_with(',') {
            continue;
        }
        match GscObject::parse(bytes) {
            Ok(g) => {
                let (a, b, c, d) = g.coverage();
                if a != b || c != d {
                    println!("MISMATCH {name}: strings {a}/{b} imports {c}/{d}");
                }
                s_hit += a;
                s_all += b;
                i_hit += c;
                i_all += d;
            }
            Err(e) => {
                bad += 1;
                println!("PARSE FAIL {name}: {e}");
            }
        }
    }
    println!(
        "scripts {}: strings {s_hit}/{s_all}, imports {i_hit}/{i_all}, parse failures {bad}",
        sorted.len()
    );
    let map = std::env::var("T6GSC_MAP").unwrap_or_else(|_| "zm_nuked".to_owned());
    let facts = MapScriptFacts::read(&map, |n| scripts.get(n).cloned());
    println!("\n== {map}: {} effect table entries", facts.effects.len());
    let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
    for p in &facts.placed {
        *kinds.entry(p.kind.as_str()).or_default() += 1;
    }
    println!("placed {}: {kinds:?}", facts.placed.len());
    let unresolved: Vec<&str> = facts
        .placed
        .iter()
        .filter(|p| !facts.effects.contains_key(&p.fxid))
        .map(|p| p.fxid.as_str())
        .collect();
    println!("placed fxids without an effect table entry: {}", unresolved.len());
    for u in unresolved.iter().take(10) {
        println!("   {u}");
    }
    for p in facts.placed.iter().take(6) {
        println!(
            "   {} {} -> {} at {:?} angles {:?} delay {} exploder {:?}",
            p.kind,
            p.fxid,
            facts.effects.get(&p.fxid).map_or("?", String::as_str),
            p.origin,
            p.angles,
            p.delay,
            p.exploder
        );
    }
    if std::env::var_os("T6GSC_PLACED").is_some() {
        for p in &facts.placed {
            println!(
                "placed	{}	{}	{}	{:.0}	{:.0}	{:.0}	{:.0}	{:.0}	{:.0}	{}",
                p.kind,
                p.fxid,
                facts.effects.get(&p.fxid).map_or("?", String::as_str),
                p.origin[0],
                p.origin[1],
                p.origin[2],
                p.angles[0],
                p.angles[1],
                p.angles[2],
                p.delay
            );
        }
    }
    println!("room tone {:?}", facts.room_tone);
    println!("sounds on effects {}:", facts.sounds_on_fx.len());
    for (fxid, alias, off) in &facts.sounds_on_fx {
        let n = facts.placed.iter().filter(|p| &p.fxid == fxid).count();
        println!("   {fxid} -> {alias} offset {off:?} ({n} placed)");
    }
    println!("loops at points {}: {:?}", facts.loops_at.len(), facts.loops_at);
    if std::env::var_os("T6GSC_CALLS").is_some() {
        // `T6GSC_SCRIPT=<name>`: that script's calls instead.
        let chosen = std::env::var("T6GSC_SCRIPT")
            .unwrap_or_else(|_| "clientscripts/mp/zm_nuked_amb.csc".to_owned());
        for name in [chosen.as_str()] {
            if let Some(g) = scripts.get(name).and_then(|b| GscObject::parse(b).ok()) {
                for c in g.run().calls {
                    println!("call [{}] {}({:?})", c.caller, c.function, c.args);
                }
            }
        }
    }
    ExitCode::SUCCESS
}
