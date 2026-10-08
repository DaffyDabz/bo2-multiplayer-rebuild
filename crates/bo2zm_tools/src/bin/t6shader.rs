//! bo2zm: dump a Black Ops II technique set's shaders, read-only.
//! `t6shader <technique set name prefix> [zone.ff] [out dir]` (default
//! Nuketown, `out\shaders`) lists the set's techniques, each pass's
//! shaders and register arguments, and writes every pass's pixel and vertex
//! shader bytecode (D3D11 DXBC) for a disassembler.

use std::path::PathBuf;
use std::process::ExitCode;

const DEFAULT_ZONE: &str =
    r"C:\Program Files (x86)\Steam\steamapps\common\Call of Duty Black Ops II\zone\all\zm_nuked.ff";
const DEFAULT_OUT: &str = r"out\shaders";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(prefix) = args.next() else {
        eprintln!("usage: t6shader <technique set name prefix> [zone.ff] [out dir]");
        return ExitCode::FAILURE;
    };
    let zone = args
        .next()
        .map_or_else(|| PathBuf::from(DEFAULT_ZONE), PathBuf::from);
    let out = args
        .next()
        .map_or_else(|| PathBuf::from(DEFAULT_OUT), PathBuf::from);
    let capture = match asset_t6::capture_zone_with_shaders(&zone) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("t6shader: {e}");
            return ExitCode::FAILURE;
        }
    };
    // `--list`: every technique set in the zone, with its techniques.
    if prefix == "--list" {
        for t in &capture.technique_sets {
            let techs: Vec<String> = t
                .techniques
                .iter()
                .enumerate()
                .filter_map(|(i, x)| x.as_ref().map(|x| format!("{i}:{}", x.name)))
                .collect();
            println!("{} | {}", t.name, techs.join(" "));
        }
        return ExitCode::SUCCESS;
    }
    let Some(set) = capture
        .technique_sets
        .iter()
        .find(|t| t.name.starts_with(&prefix))
    else {
        eprintln!("t6shader: no technique set starts with {prefix}");
        return ExitCode::FAILURE;
    };
    let dir = out.join(&set.name);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("t6shader: {}: {e}", dir.display());
        return ExitCode::FAILURE;
    }
    println!(
        "{} (world vertex format {})",
        set.name, set.world_vert_format
    );
    for (i, tech) in set.techniques.iter().enumerate() {
        let Some(tech) = tech else { continue };
        println!("technique {i}: {} flags {:#x}", tech.name, tech.flags);
        for (p, pass) in tech.passes.iter().enumerate() {
            println!(
                "   pass {p}: vs {} ({} bytes), ps {} ({} bytes)",
                pass.vertex_shader,
                pass.vertex_program.len(),
                pass.pixel_shader,
                pass.pixel_program.len()
            );
            for a in &pass.args {
                println!(
                    "      arg type {} location {} size {} buffer {} u {:#010x}",
                    a.ty, a.location, a.size, a.buffer, a.u
                );
            }
            for (kind, bytes) in [("ps", &pass.pixel_program), ("vs", &pass.vertex_program)] {
                if bytes.is_empty() {
                    continue;
                }
                let file = dir.join(format!("{i:02}_{}_{p}.{kind}.cso", tech.name));
                if let Err(e) = std::fs::write(&file, bytes) {
                    eprintln!("t6shader: {}: {e}", file.display());
                }
            }
        }
    }
    ExitCode::SUCCESS
}
