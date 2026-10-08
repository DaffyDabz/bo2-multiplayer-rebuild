//! bo2mp: a weapon's switch timing and animation names, read-only.
//! `t6alt <weapon>...` prints each weapon's raise/drop/alt-raise/alt-drop
//! times and the ADS and alt animation slots from the zones in `T6ICON_ZONES`
//! (default common_mp,common_patch_mp). With no names, lists every weapon
//! whose name contains `dualoptic` or has an alt weapon with that tag.

use std::path::Path;
use std::process::ExitCode;

use asset_t6::capture_zone;
use fastfile_t6::layout::{WeaponDef as d, WeaponVariantDef as v};

const ZONES: &str = r"C:\Program Files (x86)\Steam\steamapps\common\Call of Duty Black Ops II\zone\all";

fn main() -> ExitCode {
    let names: Vec<String> = std::env::args().skip(1).collect();
    let zones: Vec<String> = std::env::var("T6ICON_ZONES").map_or_else(
        |_| ["common_mp", "common_patch_mp"].map(str::to_owned).to_vec(),
        |z| z.split(',').map(str::to_owned).collect(),
    );
    for zone in &zones {
        let capture = match capture_zone(&Path::new(ZONES).join(format!("{zone}.ff"))) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{zone}: {e}");
                continue;
            }
        };
        for w in &capture.weapons {
            let hit = if names.is_empty() {
                w.name.contains("dualoptic") || w.name.starts_with("hamr")
            } else {
                names.iter().any(|n| n == &w.name)
            };
            if !hit {
                continue;
            }
            println!(
                "{zone} {}: alt='{}' variant[altRaise {} adsIn {} adsOut {}] def[drop {} raise {} altDrop {} firstRaise {} emptyRaise {}]",
                w.name,
                w.alt_weapon_name,
                w.var_i32(v::iAltRaiseTime),
                w.var_i32(v::iAdsTransInTime),
                w.var_i32(v::iAdsTransOutTime),
                w.def_i32(d::iDropTime),
                w.def_i32(d::iRaiseTime),
                w.def_i32(d::iAltDropTime),
                w.def_i32(d::iFirstRaiseTime),
                w.def_i32(d::iEmptyRaiseTime),
            );
            println!("    ammo='{}' clip='{}' clipSize {} startAmmo {} maxAmmo {} sharedAmmoCap {}",
                w.ammo_name, w.clip_name, w.var_i32(v::iClipSize), w.def_i32(d::iStartAmmo), w.def_i32(d::iMaxAmmo), w.def_i32(d::iSharedAmmoCap));
            for slot in [0x17, 0x19, 0x1A, 0x1B, 0x55, 0x56, 0x57, 0x58] {
                if let Some(n) = w.xanims.get(slot).filter(|n| !n.is_empty()) {
                    let len = capture
                        .xanims
                        .iter()
                        .find(|a| a.name.eq_ignore_ascii_case(n))
                        .map(|a| {
                            let ms = f64::from(a.numframes) / f64::from(a.framerate) * 1000.0;
                            let notes: Vec<String> = a.notifies.iter().map(|(k, t)| format!("{k}@{:.0}ms", f64::from(*t) * ms)).collect();
                            format!(" [{} frames @ {} fps = {ms:.0} ms; notes {}]", a.numframes, a.framerate, notes.join(","))
                        })
                        .unwrap_or_else(|| " [anim not in this zone]".into());
                    println!("    xanim 0x{slot:02X} = {n}{len}");
                }
            }
        }
    }
    ExitCode::SUCCESS
}
