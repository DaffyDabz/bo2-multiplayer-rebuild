pub mod classes;
pub mod frontend;
mod gap_hud;
mod launch_report;
mod layers;
mod load_table;
mod loading;
mod menu;
mod menu_load;
mod options;
mod plugin;
mod screen;
mod bo2_font;
mod scope_hint; // bo2mp scope lane
mod overhead_names; // bo2mp: names over heads in BO2's font
mod zm_hud;
mod lui_hud; // bo2zm M4
mod lui_additive; // bo2mp scope: true additive HUD pictures
mod lui_frontend; // bo2mp lane C
mod lui_scene; // bo2mp menus: the front end's 3D backdrop
mod bo2mp_damage; // bo2mp lane C
pub use lui_frontend::{Bo2mpFrontend, Bo2mpFrontendAssets};

pub use classes::equip_txn::{
    EquipTxnWatch, apply_pending_class_equip, resolve_class_equip_transaction,
    sync_class_change_allowed,
};
pub use classes::icons::{
    ClassSelectIconCache, UiAssetRoot, cac_attachment_image, cac_material_iwd_stem,
    cac_weapon_image, pretty_weapon_name,
};
pub use classes::select::{
    ClassChangeAllowed, ClassChangeBlockReason, ClassEquipRefusal, ClassEquipRequest,
    ClassSelectHighlight, ClassSelectOverlayOpen, ClassSelectPhase, ClassSelectStatus,
    PendingClassEquip, accept_class_equip, class_index_by_name, commit_class_equip,
    reject_class_equip,
};
pub use classes::setup::{ClassEditRow, ClassLoadoutCatalog, ClassPickerFolder, ClassSlotState};
pub use classes::store::SessionClassStore;
pub use frame::{AppScreen, LaunchIdentity, LaunchReport};
pub use frame::{ClassPreset, showcase_classes};
pub use gap_hud::GapHud;
pub use lui_additive::AdditiveUi;
pub use overhead_names::{OverheadNameRow, OverheadNameRows};
pub use launch_report::publish_gap_hud;
pub use layers::{
    ApplyUiLayers, GameUiFont, UiCamera, UiDraw, UiLayer, UiLayerVisibility, UiLayers,
    game_text_font,
};
pub use loading::{LoadProgress, LoadingPreviewSource, LoadingScreen};
pub use menu::MenuMapList;
pub use options::{BindingView, PresentModeOverride};
pub use plugin::UiPlugin;
pub use screen::{layers_for_screen, sync_ui_layers};

pub use menu::install_frontend_menus;
