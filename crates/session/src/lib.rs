mod admission;
// bo2mp lane C: the match BO2's front end set up.
pub mod bo2mp_setup;
pub mod combat_table;
pub mod content_manifest;
pub mod life_front;
pub mod lifecycle;
pub mod local_arm;
pub mod loadout;
pub mod match_apply;
pub mod plugin;
pub mod startup;
pub mod view_subject;

pub use content_manifest::{
    AuthorityWeaponProfile, ManifestFact, ManifestGap, RuntimeRuleset, SessionContentManifest,
    SessionManifestError, SessionWeaponId, SessionWeaponManifestRow,
};

pub use life_front::{LifeFrontCensus, LifeNotifyCensus};
pub use lifecycle::{
    LiveWorldIdentity, SessionSwapCompletion, SessionSwapRequest, SessionSwapResult,
    SessionSwapTarget, TeardownGaps, TeardownRequest,
};
pub use local_arm::{
    arm_local_from_presented, join_local_on_class_select, sync_prediction_metrics_to_probe,
};
pub use loadout::{
    AuthoritativeClassProjection, ClassRow, authoritative_class_lock_reason, perk_catalog_id,
    project_class, resolve_class_weapon,
};
pub use match_apply::{apply_prepared_match, install_script_model_id};
pub use startup::{PendingConsoleLines, StartupCommands};
pub use bo2mp_setup::Bo2mpMatchSetup;
pub use plugin::SessionPlugin;
