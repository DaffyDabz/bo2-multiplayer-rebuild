//! bo2zm M3: a Black Ops II world's fog bank as its client scripts pick it
//! (`setworldfogactivebank`, which the server sends as the client dvar
//! `bo2zm_fogbank`): the light table's two fog rows become that bank's.
//! Nuketown's game over picks bank 2 (no fog) for the rocket shot.

use std::sync::Arc;

use bevy::prelude::*;
use net::{LocalPresentClient, PresentedSnapshot};

use crate::prepare::scene::world::WorldScene;

/// The light table's fog rows (`assets::lane::t6`): the sun fog blend, then
/// the fog itself.
const SUN_FOG_SLOT: usize = 30;
const FOG_SLOT: usize = 31;

pub(crate) fn register(app: &mut App) {
    app.add_systems(Update, apply_t6_fog_bank.in_set(frame::RenderSet::Anim));
}

fn apply_t6_fog_bank(
    presented: Option<Res<PresentedSnapshot>>,
    local: Option<Res<LocalPresentClient>>,
    scene: Option<ResMut<WorldScene>>,
) {
    let Some(mut scene) = scene else { return };
    if scene.t6_fog_banks.is_empty() {
        return;
    }
    let want = match (presented.as_deref(), local.as_deref()) {
        (Some(p), Some(l)) => p
            .snapshot()
            .and_then(|s| {
                s.meta
                    .script_dvars(l.0)
                    .string("bo2zm_fogbank")
                    .and_then(|v| v.trim().parse::<u32>().ok())
            })
            .unwrap_or(0),
        _ => 0,
    };
    if want == scene.t6_fog_bank {
        return;
    }
    let mut table = (*scene.t6_lights_loaded).clone();
    let rows = scene
        .t6_fog_banks
        .iter()
        .find(|(bank, _)| *bank == want)
        .map(|(_, rows)| *rows);
    if let Some([sun, fog]) = rows
        && table.len() > FOG_SLOT
    {
        table[SUN_FOG_SLOT] = sun;
        table[FOG_SLOT] = fog;
    }
    diag::info!(World, "bo2zm fog bank {} -> {want}", scene.t6_fog_bank);
    scene.t6_lights = Arc::new(table);
    scene.t6_fog_bank = want;
}
