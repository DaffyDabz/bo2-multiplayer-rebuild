use bevy::prelude::Resource;

use crate::asset_graph::DestructibleDeathRow;
use asset_anim::XAnimCatalog;
use asset_game::WeaponRegistry;
use asset_model::{BodyMeshCatalog, FpvMeshCatalog, WorldWeaponCatalog};

/// The one material population a prepared match owns, and the map-zone-local
/// index space that resolves into it.
///
/// Geometry keeps references — local material indices and, after the merge,
/// nothing else. The pool itself is never nested inside an optional product.
#[derive(Clone, Default)]
pub struct MatchMaterials {
    pub population: std::sync::Arc<asset_material::MaterialDefinitions>,

    pub common_profile_id: u64,

    pub products_id: u64,

    /// map-zone-local material index -> row in `population`
    pub map_ids: Vec<Option<usize>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreparedGaps {
    pub lines: Vec<String>,
}

/// What the map itself declares about the match, captured once by the lane and
/// moved from there to its single owner in [`PreparedMap`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapFacts {
    pub minimap_corners: Option<asset_world::MinimapCorners>,

    pub north_yaw: Option<f32>,

    pub airstrike_height: Option<f32>,

    pub compass: asset_world::MapCompassDeclaration,

    pub script_sound: asset_audio::MapScriptSoundFacts,

    pub team_settings: asset_game::MapTeamSettings,

    pub t5_teamset: Option<String>,

    pub objective_visuals: asset_game::ObjectiveVisuals,

    /// bo2zm: a Black Ops II map's footstep tables (alias ids per surface
    /// and step kind), first person first.
    pub t6_footsteps: Vec<asset_t6::FootstepTableRef>,
    /// bo2zm: the Zombies HUD's icons (perks, timed power-ups).
    pub t6_hud_icons: T6HudIcons,
    /// bo2zm: the Zombies HUD's fonts.
    pub t6_hud_fonts: T6HudFonts,
    /// bo2zm M4: Black Ops II's own UI scripts and what they read.
    pub t6_ui: T6Ui,
    /// bo2mp: the third-person player animation script and its anim types
    /// (every client poses other players with it).
    pub t6_playeranim: Option<(String, String)>,
    /// bo2mp: each player animation's move speed (units/s, from its root
    /// motion): a legs animation plays at the player's speed over this.
    pub t6_anim_speeds: Vec<(String, f32)>,
}

/// bo2zm: Black Ops II's HUD fonts: the English `fonts/720/*` bitmap fonts,
/// all on one sheet (white, alpha the coverage), each by the name the HUD's
/// Lua scripts give it (`ui/t6/codbase.lua`: Default = normalFont,
/// Condensed = smallFont, Big = bigFont, Morris = extraBigFont, ...).
#[derive(Clone, Debug, Default, Resource)]
pub struct T6HudFonts {
    pub sheet: Option<std::sync::Arc<bevy::prelude::Image>>,
    pub fonts: Vec<T6HudFont>,
}

/// One font: its pixel height and glyphs (letter; x0, y0 against the pen
/// and the baseline, advance, width, height in pixels; s0, t0, s1, t1 on the
/// sheet).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct T6HudFont {
    pub name: String,
    pub pixel_height: f32,
    pub glyphs: Vec<(u16, [i16; 5], [f32; 4])>,
}

impl PartialEq for T6HudFonts {
    fn eq(&self, other: &Self) -> bool {
        self.fonts == other.fonts && self.sheet.is_some() == other.sheet.is_some()
    }
}

/// bo2zm M4: Black Ops II's UI scripts (the HavokScript `.lua` rawfiles,
/// `ui/lui/*`, `ui/t6/*`, `ui_mp/t6/*`, in zone load order: a later one
/// wins a name), the English strings they localize and the string tables
/// they look up. Their pictures are in `T6HudIcons`.
#[derive(Clone, Debug, Default, Resource)]
pub struct T6Ui {
    pub scripts: Vec<(String, std::sync::Arc<[u8]>)>,
    pub strings: std::sync::Arc<Vec<(String, String)>>,
    /// (name, columns, rows, cells row-major).
    pub tables: std::sync::Arc<Vec<(String, usize, usize, Vec<String>)>>,
    /// bo2mp: the zones' config files (`default_private.cfg`, ...) by
    /// name, for the menus' `exec <file>`.
    pub configs: std::sync::Arc<Vec<(String, String)>>,
}

impl PartialEq for T6Ui {
    fn eq(&self, other: &Self) -> bool {
        self.scripts.len() == other.scripts.len()
            && self.scripts.iter().zip(&other.scripts).all(|(a, b)| a.0 == b.0 && a.1.len() == b.1.len())
            && self.strings.len() == other.strings.len()
            && self.tables.len() == other.tables.len()
    }
}

/// bo2zm: Black Ops II HUD icons by material name, each its colour map read
/// from the game's image packs (block-compressed, as the GPU takes it).
#[derive(Clone, Debug, Default, Resource)]
pub struct T6HudIcons(pub Vec<(String, std::sync::Arc<bevy::prelude::Image>)>);

impl PartialEq for T6HudIcons {
    fn eq(&self, other: &Self) -> bool {
        self.0.len() == other.0.len() && self.0.iter().zip(&other.0).all(|(a, b)| a.0 == b.0)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PreparedMap {
    pub zone: String,

    pub namespace: Option<asset_core::AssetNamespace>,
    pub spawns: Vec<asset_world::SpawnPoint>,

    pub facts: MapFacts,
    pub gaps: PreparedGaps,
}

#[derive(Clone, Debug, Default, Resource)]
pub struct PreparedWeapons(pub std::sync::Arc<WeaponRegistry>);

#[derive(Clone, Debug, Default, Resource)]
pub struct MatchType10SoundHints(pub Vec<String>);

#[derive(Clone, Debug, Default, Resource)]
pub struct PreparedFpvMeshes(pub FpvMeshCatalog);

#[derive(Clone, Debug, Default, Resource)]
pub struct PreparedBodies(pub std::sync::Arc<BodyMeshCatalog>);

#[derive(Clone, Debug, Default, Resource)]
pub struct PreparedWorldWeapons(pub WorldWeaponCatalog);

#[derive(Clone, Debug, Default, Resource)]
pub struct PreparedProjectileMeshes(pub asset_model::ProjectileMeshCatalog);

#[derive(Clone, Debug, Default, Resource)]
pub struct PreparedXModelWalkCensus {
    pub walked_n: usize,

    pub unclassified_n: usize,

    pub projectile_names: Vec<String>,
}

impl PreparedXModelWalkCensus {
    pub fn projectile_walked_n(&self) -> usize {
        self.projectile_names.len()
    }

    pub fn projectile_names_csv(&self) -> String {
        self.projectile_names.join(",")
    }

    pub fn report_line(&self, source: &str) -> String {
        format!(
            "{source} XModels: {} unique names, {} unclassified (model_kind None); projectile_* walked: {} ({})",
            self.walked_n,
            self.unclassified_n,
            self.projectile_walked_n(),
            if self.projectile_names.is_empty() {
                "none".to_owned()
            } else {
                self.projectile_names_csv()
            }
        )
    }
}

#[derive(Clone, Debug, Default, Resource)]
pub struct PreparedXAnims(pub XAnimCatalog);

#[derive(Clone, Debug, Default, Resource)]
pub struct PreparedDestructibleDeath(pub Vec<DestructibleDeathRow>);

#[derive(Clone, Debug, Default, Resource)]
pub struct PreparedLocalizedStrings(pub asset_game::LocalizeCatalog);

#[derive(Clone, Debug, Default, Resource)]
pub struct SessionCompass {
    pub corners: Option<asset_world::MinimapCorners>,

    pub north_yaw: Option<f32>,

    pub declaration: asset_world::MapCompassDeclaration,
}

#[derive(Clone, Copy, Debug, Default, Resource, PartialEq, Eq)]
pub struct PreparedBodyClips(pub bool);
