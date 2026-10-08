//! bo2mp: a weapon's own HUD materials, read-only.
//! `t6wicon <weapon>...` prints each weapon's material pointers (hudIcon @940,
//! ammoCounterIcon @956, killIcon @1632, reticles) from the zones in
//! `T6ICON_ZONES` (default common_mp,patch_mp,common_patch_mp). With no
//! names it lists every weapon whose name contains `grenade`.

use std::path::Path;
use std::process::ExitCode;

use asset_t6::capture_zone;

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
        println!("zone {zone}: {} weapons", capture.weapons.len());
        for w in &capture.weapons {
            let hit = if names.is_empty() { w.name.contains("grenade") } else { names.iter().any(|n| n == &w.name) };
            if hit {
                println!("{zone} {}: {:?}", w.name, w.materials);
            }
        }
    }
    ExitCode::SUCCESS
}
