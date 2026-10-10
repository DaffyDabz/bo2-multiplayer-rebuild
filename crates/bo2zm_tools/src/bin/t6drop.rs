//! bo2mp: what a falling body's joint meets in a Black Ops II map, read-only.
//! `t6drop [zone.ff] x y z ...` (default MP Nuketown) builds the map's
//! collision as the game does (brushes and the mesh; not the static models)
//! and, at each point, sweeps boxes of a few sizes straight down with the
//! ragdoll's mask and the player's, then drops one in ragdoll-sized steps.

use std::path::PathBuf;
use std::process::ExitCode;

use asset_t6::capture_zone;

const DEFAULT_ZONE: &str =
    r"D:\SteamLibrary\steamapps\common\Call of Duty Black Ops II\zone\all\mp_nuketown_2020.ff";

/// What the ragdoll's joints collide with: solid, glass and item clip (the
/// fx physics mask; Nuketown's ground is clip-only, no solid bit).
const MASK_RAGDOLL: u32 = 0xc11;
/// The solid bit alone (the ragdoll's first mask).
const MASK_SOLID: u32 = 1;
/// What a player walks on (MASK_PLAYERSOLID).
const MASK_PLAYER: u32 = 0x0281_0011;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1).peekable();
    let path = match args.peek() {
        Some(a) if a.ends_with(".ff") => PathBuf::from(args.next().unwrap_or_default()),
        _ => PathBuf::from(DEFAULT_ZONE),
    };
    let nums: Vec<f32> = args.filter_map(|a| a.parse().ok()).collect();
    let capture = match capture_zone(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("t6drop: {e}");
            return ExitCode::FAILURE;
        }
    };
    let Some(clip) = capture.clip.as_ref() else {
        eprintln!("t6drop: {} has no clipMap", path.display());
        return ExitCode::FAILURE;
    };
    let world = match asset_world::build_t6_clip_collision(clip) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("t6drop: collision: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "{} brushes, {} mesh tris, {} materials",
        world.brushes.len(),
        world.mesh.tri_indices.len() / 3,
        world.materials.len()
    );
    for p in nums.chunks_exact(3) {
        let at = [p[0], p[1], p[2]];
        println!("at {at:?}:");
        for (name, mask) in [("ragdoll", MASK_RAGDOLL), ("solid", MASK_SOLID), ("player", MASK_PLAYER)] {
            for h in [0.25f32, 1.0, 4.0, 8.0] {
                let start = at;
                let hit = world.sweep_box(start, [at[0], at[1], at[2] - 300.0], [-h; 3], [h; 3], mask);
                let still = world.sweep_box(start, start, [-h; 3], [h; 3], mask);
                println!(
                    "  {name:7} half {h:5.2}: down frac {:.4} end z {:9.3} normal {:?} startsolid {} allsolid {} | in place startsolid {}",
                    hit.fraction, hit.endpos[2], hit.normal, hit.startsolid, hit.allsolid, still.startsolid
                );
            }
        }
        // Which contents bits the first floor below has.
        for bit in 0..32 {
            let hit = world.sweep_box(at, [at[0], at[1], at[2] - 300.0], [-1.0; 3], [1.0; 3], 1 << bit);
            if hit.fraction < 1.0 {
                println!("  bit {:#010x}: floor at z {:9.3}", 1u32 << bit, hit.endpos[2]);
            }
        }
        // A joint falling 9 units a step (800 u/s^2 for a while at 90 steps a second).
        let h = 7.0f32;
        let mut z = at[2];
        for step in 0..30 {
            let to = z - 9.0;
            let hit = world.sweep_box([at[0], at[1], z], [at[0], at[1], to], [-h; 3], [h; 3], MASK_RAGDOLL);
            println!(
                "  fall {step:2}: {z:8.3} -> {to:8.3}: frac {:.4} end z {:8.3} startsolid {} allsolid {}",
                hit.fraction, hit.endpos[2], hit.startsolid, hit.allsolid
            );
            if hit.fraction < 1.0 && !hit.startsolid {
                break;
            }
            z = if hit.startsolid { to } else { hit.endpos[2] };
        }
    }
    ExitCode::SUCCESS
}
