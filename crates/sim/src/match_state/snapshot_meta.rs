use crate::identities::{LifeSequence, MatchPhase};
use crate::world::ClientId;
use crate::world_objects::WorldObjectSnapshot;

use super::ClientLifecycle;
use super::client_view::{
    KillcamHud, LocationSelection, MenuCommand, RadarMode, RemoteMissile, ViewEffects,
};
use super::events::{DebrisRecord, EntityEventRecord, EventRecord, PelletFxRecord, SimEvent};
use super::loadout::LoadoutSpec;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClientSnapshotMeta {
    pub controls: super::ScriptControls,
    pub weapon_lock: crate::WeaponLock,
    pub killcam_hud: Option<KillcamHud>,
    pub lifecycle: ClientLifecycle,
    pub loadout: Option<LoadoutSpec>,
    pub life_sequence: LifeSequence,

    pub item_use_spawn_ms: i32,
    pub item_use_entity: Option<crate::EntityRef>,

    pub ammo_clip: i32,

    pub ammo_stock: i32,

    pub score: i32,
    pub kills: i32,
    pub deaths: i32,
    pub kill_streak: i32,
    pub radar: RadarMode,
    pub remote_missile: Option<RemoteMissile>,

    pub ammo_by_weapon: Vec<(u32, i32, i32)>,

    pub taped_mag_spent: Vec<u32>,

    pub weapon_shot_count: u8,
    pub burst_latch: bool,
    pub burst_latch_secondary: bool,
    pub rechamber_pending: bool,
    pub rechamber_pending_secondary: bool,

    pub dead_since_tick: Option<u32>,

    pub look_at_killer_yaw: i32,

    pub name: [u8; 16],

    pub hud_archival: Vec<hud_iw4::HudElem>,

    pub hud_current: Vec<hud_iw4::HudElem>,

    pub ffa_team: Option<u8>,

    pub client_state_team: i32,

    pub rank: i32,

    pub prestige: i32,

    pub player_card_icon: u32,

    pub player_card_title: u32,

    pub player_card_nameplate: u32,

    pub client_dvars: Vec<(String, String)>,

    pub shellshock: Option<hud_iw4::ShockParams>,

    pub view_effects: ViewEffects,

    pub menu_commands: Vec<MenuCommand>,

    pub location_selection: Option<LocationSelection>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SnapshotMeta {
    pub objectives: crate::ObjectiveMatch,
    pub phase: MatchPhase,

    pub match_elapsed_ms: u32,
    pub prematch: gamemode_iw4::PrematchStep,

    pub score_limit: i32,

    pub time_limit_ms: u32,

    pub kind: gamemode_iw4::GameModeKind,
    pub clients: Vec<(ClientId, ClientSnapshotMeta)>,

    pub journal: Vec<EventRecord>,

    pub entity_events: Vec<EntityEventRecord>,

    pub pellet_fx: Vec<PelletFxRecord>,

    /// bo2mp: the pieces that broke off this tick (see `DebrisRecord`).
    pub debris: Vec<DebrisRecord>,

    pub sound_aliases: crate::SoundAliasCsOccupied,

    pub effect_names: crate::EffectNameCsOccupied,

    pub hud_materials: crate::HudMaterialCsOccupied,

    pub hud_strings: crate::HudStringCsOccupied,

    pub rng: RngDebugMeta,

    pub world_objects: WorldObjectSnapshot,

    pub area_entities: Option<crate::AreaEntityWorldSnapshot>,

    pub entity_dobjs: Vec<(
        crate::AuthorityModelOwner,
        xmodel_runtime::DObjSemanticState,
    )>,

    pub entities: Vec<entity_iw4::EntityState>,

    pub script_movers: Vec<crate::ScriptMoverGentity>,

    pub entity_kernel: crate::EntityKernelSnapshot,

    pub corpses: crate::PlayerCorpsePool,

    pub item_ammo: Vec<DroppedItemAmmo>,

    pub item_pickups: Vec<ItemPickupRecord>,

    /// bo2zm: solid collision models (Nuketown's patch walls, perk
    /// machines, the Mystery Box) as turned boxes; player movement on both
    /// sides clips against them.
    pub oriented_blockers: Vec<OrientedBlocker>,

    /// bo2zm: the Black Ops II movement rules the game runs with, so a
    /// second PC's own guess of its moves (it runs no scripts) follows them
    /// too. None = no rules sent yet.
    pub feel: Option<movement_iw4::Bo2Feel>,
}

/// bo2zm: a solid box at any turn: its place, its axes (forward, left,
/// up) and its bounds along them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OrientedBlocker {
    pub entnum: u16,
    pub origin: [f32; 3],
    pub axis: [[f32; 3]; 3],
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
}

impl OrientedBlocker {
    /// The six planes (outward normal, distance) of the box in the world.
    pub fn planes(&self) -> [[f32; 4]; 6] {
        let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let mut out = [[0.0; 4]; 6];
        for k in 0..3 {
            let n = self.axis[k];
            let at = dot(n, self.origin);
            out[k * 2] = [n[0], n[1], n[2], at + self.maxs[k]];
            out[k * 2 + 1] = [-n[0], -n[1], -n[2], -(at + self.mins[k])];
        }
        out
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DroppedItemAmmo {
    pub entnum: i32,
    pub clip_r: i32,
    pub clip_l: i32,
    pub stock: i32,
    pub scavenger: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ItemPickupRecord {
    pub picker: i32,
    pub weapon: u32,
    pub from_entnum: i32,
    pub clip_r: i32,
    pub clip_l: i32,
    pub stock: i32,
    pub swapped_entnum: i32,
    pub picker_pm_type: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RngDebugMeta {
    pub root_seed: u64,
    pub scheme: u32,
    pub spawn_draws: u64,
    pub combat_draws: u64,
    pub bot_draws: u64,
}

impl SnapshotMeta {
    pub fn for_client(&self, id: ClientId) -> Option<&ClientSnapshotMeta> {
        self.clients.iter().find(|(c, _)| *c == id).map(|(_, m)| m)
    }

    pub fn events_for(&self, id: ClientId) -> impl Iterator<Item = &SimEvent> {
        self.journal
            .iter()
            .filter(move |record| record.audience.projects_to(id))
            .map(|record| &record.event)
    }

    pub fn script_dvars(&self, id: ClientId) -> ScriptDvars<'_> {
        ScriptDvars {
            client: self
                .for_client(id)
                .map_or(&[], |m| m.client_dvars.as_slice()),
            server: &self.objectives.server_info,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ScriptDvars<'a> {
    client: &'a [(String, String)],
    server: &'a [(String, String)],
}

impl<'a> ScriptDvars<'a> {
    pub fn string(&self, name: &str) -> Option<&'a str> {
        self.client
            .iter()
            .chain(self.server)
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn int(&self, name: &str) -> Option<i32> {
        let value = self.string(name)?.trim();
        value
            .parse::<i32>()
            .ok()
            .or_else(|| value.parse::<f32>().ok().map(|v| v as i32))
    }

    pub fn text(&self, name: &str, localize: impl Fn(&str) -> Option<String>) -> Option<String> {
        let mut parts = self.string(name)?.split(crate::HUD_PRINT_ARG_SEPARATOR);
        let head = parts.next().unwrap_or_default();
        let Some(key) = head.strip_prefix('@') else {
            return Some(head.to_owned());
        };
        let template = localize(key)?;
        Some(parts.enumerate().fold(template, |line, (i, arg)| {
            line.replace(&format!("&&{}", i + 1), arg)
        }))
    }
}

#[cfg(test)]
mod oriented_blocker_tests {
    use super::OrientedBlocker;

    /// A 128 x 10 x 128 wall turned 30 degrees stops the player where he
    /// first touches it, and lets him by beside its end.
    #[test]
    fn turned_wall_stops_the_player_box() {
        let (s, c) = 30f32.to_radians().sin_cos();
        let wall = OrientedBlocker {
            entnum: 1,
            origin: [0.0; 3],
            axis: [[c, s, 0.0], [-s, c, 0.0], [0.0, 0.0, 1.0]],
            mins: [-64.0, -5.0, -64.0],
            maxs: [64.0, 5.0, 64.0],
        };
        let planes = wall.planes();
        let brush = trace_iw4::BrushRef {
            planes: &planes,
            contents: 1,
            plane_surface_flags: &[0; 6],
            glass_encoded: 0,
        };
        let t = trace_iw4::trace_box(
            [brush],
            [0.0, -100.0, 0.0],
            [0.0, 100.0, 0.0],
            [-15.0, -15.0, 0.0],
            [15.0, 15.0, 70.0],
            1,
        );
        assert!(t.fraction < 1.0, "went through");
        // Touching: the player clips as an upright capsule of radius 15 (the
        // engine's), so 15 beyond the wall's half-thickness of 5.
        let along = t.endpos[0] * -s + t.endpos[1] * c;
        let want = -(5.0 + 15.0);
        assert!(
            (along - want).abs() < 0.5,
            "stopped at {along}, want {want}"
        );
        // Beside it, 64 + 30 off along its length, he passes.
        let past = trace_iw4::trace_box(
            [brush],
            [100.0 * c, -100.0, 0.0],
            [100.0 * c, 100.0, 0.0],
            [-15.0, -15.0, 0.0],
            [15.0, 15.0, 70.0],
            1,
        );
        assert!(past.fraction >= 1.0, "hit beside it: {}", past.fraction);
    }
}
