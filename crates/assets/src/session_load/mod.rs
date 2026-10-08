use std::path::{Path, PathBuf};
use std::sync::Arc;

use bevy::tasks::{TaskPool, TaskPoolBuilder};

use crate::{
    PreparedGaps, PreparedMap,
    lane::{LoadedWorld, lane},
    lane_capability::PreparedCapability,
};
use asset_anim::{XAnimBuild, XAnimCatalog};
use asset_game::{
    FxCatalog, LocalizeCatalog, MP_LOCALIZED_ZONES, WeaponBuild, WeaponRegistry,
    load_localize_catalog_in_lane,
};
use asset_material::{ImageDemandPlan, MaterialCatalog};
use asset_model::{
    BodyMeshCatalog, FpvMeshBuild, FpvMeshCatalog, ProjectileMeshBuild, WorldWeaponBuild,
    WorldWeaponCatalog,
};
use asset_transport::load_jobs::{self, JobKind};
use asset_transport::{
    LoadProgress, StageHandle, StageId, find_common_mp_for_envelope, find_common_mp_for_zone,
    find_runtime_common_mp, find_runtime_zone, find_zone_file_version, find_zone_for_tree,
    games_root_from_env, open_zone_shared, peek_zone_version,
};
use asset_world::{ClipCollision, IntermissionView, WorldDraw};

pub use asset_world::WorldDrawPolicy;

mod common_cache;
mod common_walks;
mod match_walk;
mod resident_map;

use common_cache::*;
use common_walks::*;
use match_walk::*;

pub use common_cache::{CommonKey, CommonSet, ShellCommon, load_shell_common};
pub use common_walks::{
    MatchMaterialSeed, apply_match_material_map, load_match_material_catalog,
    load_match_material_seed,
};
pub use resident_map::load_prepared_match;

static PROCESS_CPUS: std::sync::OnceLock<Vec<usize>> = std::sync::OnceLock::new();

pub fn publish_process_cpus(cpus: Vec<usize>) {
    let _ = PROCESS_CPUS.set(cpus);
}

pub fn load_workers() -> usize {
    PROCESS_CPUS
        .get()
        .map(Vec::len)
        .or_else(|| std::thread::available_parallelism().ok().map(|n| n.get()))
        .map_or(1, |n| n.saturating_sub(2).max(1))
}

pub fn load_pool() -> &'static TaskPool {
    static POOL: std::sync::OnceLock<TaskPool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        TaskPoolBuilder::new()
            .num_threads(load_workers())
            .thread_name("iw4l load".to_owned())
            .on_thread_spawn(|| {
                if let Some(cpus) = PROCESS_CPUS.get() {
                    set_thread_cpus(cpus);
                }
            })
            .build()
    })
}

#[cfg(target_os = "linux")]
fn set_thread_cpus(cpus: &[usize]) {
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        for cpu in cpus {
            if *cpu >= libc::CPU_SETSIZE as usize {
                return;
            }
            libc::CPU_SET(*cpu, &mut set);
        }
        libc::sched_setaffinity(0, size_of::<libc::cpu_set_t>(), &raw const set);
    }
}

#[cfg(not(target_os = "linux"))]
fn set_thread_cpus(_cpus: &[usize]) {}

#[derive(Default, Clone)]
pub struct PreparedWorld {
    pub draw: Option<WorldDraw>,
    pub dynamic_light: Option<asset_world::ResolvedLightDef>,
    pub static_model_meshes: Vec<asset_world::ModelMesh>,

    pub static_model_instances: Vec<Option<asset_world::StaticModelPlacement>>,

    pub map_xmodel_scene_assets: asset_world::MapXModelSceneCatalog,

    pub script_model_instances: Vec<asset_world::ScriptModelSceneInstance>,

    pub script_brush_models: Vec<asset_world::ScriptBrushModelPlacement>,

    pub flag_descriptors: Vec<asset_world::FlagDescriptor>,

    pub script_structs: Vec<asset_world::MapScriptStruct>,

    pub dyn_ents: asset_world::DynEntCatalog,

    pub smodel_lighting_samples: Vec<asset_model::SmodelLightingSample>,

    pub light_grid: Option<asset_model::OwnedLightGrid>,

    pub fx: asset_game::FxCatalog,

    pub fx_models: asset_game::FxModelCatalog,

    pub fx_glass: Option<asset_world::FxGlassReset>,

    pub impact_fx: Option<asset_game::OwnedFxImpactTable>,
    pub reflection_probe_images: Vec<Option<bevy::prelude::Image>>,
    pub intermission_view: Option<IntermissionView>,

    pub exp_fog: Option<asset_world::ExpFog>,

    pub film_vision: Option<asset_world::FilmVision>,
    pub film_visions: std::collections::BTreeMap<
        String,
        Result<asset_world::FilmVision, asset_world::FilmVisionParseError>,
    >,

    pub createart_name: Option<String>,
    pub min: [f32; 3],
    pub max: [f32; 3],

    pub world_bounds: Option<[f32; 6]>,
    pub policy: WorldDrawPolicy,
}

#[derive(Default, Clone)]
pub struct PreparedMatch {
    pub scripts: crate::ScriptSources,
    pub world: PreparedWorld,

    pub fx: asset_game::FxDefinitions,
    pub materials: crate::MatchMaterials,
    pub clip: Option<Arc<ClipCollision>>,
    pub weapons: Arc<WeaponRegistry>,
    pub fpv_meshes: FpvMeshCatalog,
    pub bodies: Arc<BodyMeshCatalog>,
    pub world_weapons: WorldWeaponCatalog,

    pub projectile_meshes: asset_model::ProjectileMeshCatalog,
    pub xanims: XAnimCatalog,
    pub destructible_death: Vec<crate::DestructibleDeathRow>,
    pub player_anim_sources: asset_anim::PlayerAnimSources,

    pub tracers: asset_game::TracerDefinitions,

    pub strings: LocalizeCatalog,
    pub report: Vec<String>,

    pub prepared_map: PreparedMap,

    pub pen_table: weapon_iw4::PenetrationDepthTable,
    pub pen_table_loaded: bool,
    pub lochit_table: Option<[f32; weapon_iw4::HITLOC_COUNT]>,

    pub xmodel_walk: crate::PreparedXModelWalkCensus,

    pub sound: Option<Result<asset_audio::SoundCatalog, String>>,
}

pub enum MatchLoadOutcome {
    Ready(PreparedMatch),
    Canceled,
}
