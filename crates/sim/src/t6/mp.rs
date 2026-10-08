//! bo2mp: Black Ops II multiplayer on the same runtime as Zombies.
//!
//! A multiplayer map (`mp_*`) runs its game type from `maps/mp/gametypes/`
//! (Zombies': `maps/mp/gametypes_zm/`). Its scripts call one script no PC
//! zone carries, `maps/mp/gametypes/_globallogic` (the match: start, clock,
//! limits, end, next round); the engine provides it as GSC source
//! ([`GLOBALLOGIC`], OUR CODE written from what the game's scripts call and
//! wait for), compiled with `gsc_t6::compile` and linked beside the zones'.

use bevy_ecs::prelude::World;
use gsc_t6::ScriptObject;

use super::Zm;

/// Our `maps/mp/gametypes/_globallogic`.
const GLOBALLOGIC: &str = include_str!("scripts/mp_globallogic.gsc");

/// A multiplayer map, by its zone name.
pub(super) fn is_mp(map: &str) -> bool {
    map.starts_with("mp_")
}

/// The scripts the engine provides for a multiplayer map, compiled.
/// BO2MP_DEBUG_GSC=<file>: a script of ours (`bo2mp/debug`) whose `main`
/// runs on the level once the match has started (a test aid: print what
/// the game's scripts hold, `println(...)` lands in the log).
pub(super) fn engine_scripts() -> Result<Vec<ScriptObject>, String> {
    let mut out = vec![gsc_t6::compile(
        "maps/mp/gametypes/_globallogic",
        GLOBALLOGIC,
    )?];
    if let Ok(path) = std::env::var("BO2MP_DEBUG_GSC") {
        let src = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
        out.push(gsc_t6::compile("bo2mp/debug", &src)?);
    }
    Ok(out)
}

/// Where the game type's scripts live.
pub(super) fn gametypes(world: &World) -> &'static str {
    if world.get_resource::<Zm>().is_some_and(|z| z.mp) {
        "maps/mp/gametypes"
    } else {
        "maps/mp/gametypes_zm"
    }
}

/// The engine's callbacks script (`codecallback_*`).
pub(super) fn callbacks(world: &World) -> String {
    format!("{}/_callbacksetup", gametypes(world))
}
