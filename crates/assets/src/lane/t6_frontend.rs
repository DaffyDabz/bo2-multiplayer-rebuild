//! bo2mp lane C: Black Ops II's multiplayer front end needs no map: its UI
//! scripts, the pictures they name, its fonts, string tables and English
//! text come from the zones the front end loads (`code_post_gfx_mp`,
//! `ui_mp`, `patch_mp`, `patch_ui_mp`, a later one winning a name) and their
//! language zones (`zone/english/en_*`).

use std::path::Path;

use asset_t6::{PackSet, ZoneCapture};

/// The front end's zones, in load order.
const FRONTEND_ZONES: [&str; 4] = ["code_post_gfx_mp", "ui_mp", "patch_mp", "patch_ui_mp"];

/// What the front end draws with: its scripts and their data, the pictures
/// they name, the fonts; and a report (one line per step).
pub struct T6Frontend {
    pub ui: crate::T6Ui,
    pub icons: crate::T6HudIcons,
    pub fonts: crate::T6HudFonts,
    /// The front end's sounds (its menus' `Engine.PlaySound` aliases and
    /// music), from the zones' sound banks and the bank files beside them.
    pub sound: Result<asset_audio::SoundCatalog, String>,
    pub report: Vec<String>,
}

/// Read the front end from his install's `zone/all` folder.
pub fn load_t6_frontend(zone_all: &Path) -> T6Frontend {
    let mut report = Vec::new();
    let mut captures: Vec<ZoneCapture> = Vec::new();
    for z in FRONTEND_ZONES {
        let path = zone_all.join(format!("{z}.ff"));
        match asset_t6::capture_zone(&path) {
            Ok(c) => captures.push(c),
            Err(e) => report.push(format!("t6 front end: {z}: {e}")),
        }
    }
    let lang_dir = zone_all.parent().map(|d| d.join("english"));
    let lang_caps: Vec<ZoneCapture> = FRONTEND_ZONES
        .iter()
        .filter_map(|z| {
            let path = lang_dir.as_ref()?.join(format!("en_{z}.ff"));
            asset_t6::capture_zone(&path).ok()
        })
        .collect();
    let refs: Vec<&ZoneCapture> = captures.iter().collect();
    // BO2's lobby loads common_mp (`loadcommonff`): the menus' own sounds
    // (cac_grid_nav, ... in mpl_common) and Create-a-Class's pictures (the
    // wildcards' menu_mp_bonuscard_*) are there.
    let common: Vec<ZoneCapture> = [zone_all.join("common_mp.ff"), lang_dir.clone().unwrap_or_default().join("en_common_mp.ff")]
        .iter()
        .filter_map(|p| asset_t6::capture_zone(p).map_err(|e| report.push(format!("t6 front end sound: {}: {e}", p.display()))).ok())
        .collect();
    // The pictures: the front end's zones (the latest first), then common_mp's.
    // (A later front-end zone's picture wins: patch_ui_mp holds a 2048 square
    // `menu_mp_soldiers` that replaces ui_mp's 1024 one, which shows far
    // brighter on the left than BO2's menu.)
    let pics: Vec<&ZoneCapture> = captures.iter().rev().chain(common.iter().take(1)).collect();
    // bo2mp: the zones' named visions (mpOutro, mpIntro, ...) for the
    // scripts' `visionsetnaked`.
    asset_world::register_t6_visions(
        captures
            .iter()
            .chain(common.iter())
            .flat_map(|c| c.raw_files.iter())
            .filter_map(|(name, bytes)| {
                let n = name.replace('\\', "/");
                let n = n.strip_prefix("vision/")?.strip_suffix(".vision")?.to_owned();
                let v = asset_world::parse_t6_vision(&String::from_utf8_lossy(bytes))?;
                Some((n, v))
            })
            .collect(),
    );
    let scripts = super::t6_m2::ui_scripts(&refs);
    let mut names = super::t6_m2::ui_strings(&scripts);
    names.extend(table_picture_names(&pics));
    // The button glyphs' pictures (BO2 PC's ^BBUTTON_CYCLE_LEFT^ arrows).
    // (BO2's own icon set, the FontIcon asset, names each glyph's picture.)
    names.extend(glyph_pictures(&refs));
    names.extend(["ui_arrow_left", "ui_arrow_right"].map(str::to_owned));
    // The pips the engine draws for `setupDashes` (code_post_gfx_mp's own
    // materials: grey filled, hollow outline, green better, red worse).
    names.extend(["menu_mp_pip_blue", "menu_mp_pip_outline", "menu_mp_pip_green", "menu_mp_pip_red"].map(str::to_owned));
    // The lobby's map pictures: its scripts make the names ("menu_" .. map
    // .. "_map_select_final").
    for c in &refs {
        for m in &c.materials {
            if m.name.starts_with("menu_mp_") && m.name.ends_with("_map_select_final") {
                names.insert(m.name.clone());
            }
        }
    }
    let packs = match PackSet::open_dir(zone_all) {
        Ok(p) => Some(p),
        Err(e) => {
            report.push(format!("t6 front end image packs: {e}"));
            None
        }
    };
    let icons = super::t6_m2::hud_icons(&pics, packs.as_ref(), &names, &mut report);
    let lang_packs = lang_dir.as_deref().and_then(|d| PackSet::open_dir(d).ok());
    let fonts = super::t6_m2::hud_fonts(&lang_caps, lang_packs.as_ref().or(packs.as_ref()), &mut report);
    let sound = frontend_sound(zone_all, &refs, &lang_caps, &common, &mut report);
    // Create-a-Class asks each grenade's most (Engine.GetMaxAmmoForItem:
    // the weapon file's maxAmmo): a two-column table "bo2mp/maxammo.csv"
    // (weapon name, maxAmmo) from the MP weapons in common_mp.
    let mut max_ammo: Vec<String> = Vec::new();
    for w in common.iter().chain(captures.iter()).flat_map(|c| c.weapons.iter()) {
        if !w.name.contains('+') && !max_ammo.iter().step_by(2).any(|n| *n == w.name) {
            max_ammo.push(w.name.clone());
            max_ammo.push(w.def_i32(fastfile_t6::layout::WeaponDef::iMaxAmmo).to_string());
        }
    }
    drop(common);
    let strings: Vec<(String, String)> = lang_caps.iter().flat_map(|c| c.localize.iter().cloned()).collect();
    let mut tables: Vec<(String, usize, usize, Vec<String>)> = captures
        .iter()
        .flat_map(|c| c.string_tables.iter())
        .map(|t| (t.name.clone(), t.columns, t.rows, t.cells.clone()))
        .collect();
    report.push(format!("t6 front end: {} weapons' maxAmmo for Create-a-Class", max_ammo.len() / 2));
    tables.push(("bo2mp/maxammo.csv".to_owned(), 2, max_ammo.len() / 2, max_ammo));
    report.push(format!(
        "t6 front end: {} scripts, {} English strings, {} string tables, {} pictures",
        scripts.len(),
        strings.len(),
        tables.len(),
        icons.0.len()
    ));
    T6Frontend {
        ui: crate::T6Ui {
            scripts,
            strings: std::sync::Arc::new(strings),
            tables: std::sync::Arc::new(tables),
            configs: std::sync::Arc::new(super::t6_m2::ui_configs(&refs)),
        },
        icons: with_scene_textures(with_loadscreens(icons, zone_all, &refs, packs.as_ref(), &mut report), &refs, packs.as_ref(), &mut report),
        fonts,
        sound,
        report,
    }
}

/// The front end's sound catalog: its zones' banks (`mpl_ui`, the music)
/// and the shared multiplayer ones, their clips in the install's `sound`
/// folder.
fn frontend_sound(
    zone_all: &Path,
    captures: &[&ZoneCapture],
    lang_caps: &[ZoneCapture],
    common: &[ZoneCapture],
    report: &mut Vec<String>,
) -> Result<asset_audio::SoundCatalog, String> {
    let dir = zone_all
        .parent()
        .and_then(Path::parent)
        .map(|install| install.join("sound"))
        .filter(|d| d.is_dir())
        .ok_or_else(|| "Black Ops II sound folder not found beside the zones".to_owned())?;
    let bases = super::t6_m2::bank_bases(captures, "mp_frontend");
    let base_refs: Vec<&str> = bases.iter().map(String::as_str).collect();
    let (index, bank_report) = asset_audio::T6BankIndex::open(&dir, &base_refs);
    report.extend(bank_report);
    let banks: Vec<&asset_t6::SndBankRef> = captures
        .iter()
        .copied()
        .chain(lang_caps.iter())
        .chain(common.iter())
        .flat_map(|c| c.sound_banks.iter())
        .collect();
    let globals = captures.iter().find_map(|c| c.snd_globals.as_ref());
    let (catalog, census) =
        asset_audio::build_t6_sound_catalog(&banks, globals, &index, asset_core::ZoneOwner::intern("mp_frontend"));
    report.push(format!(
        "t6 front end sound: {} aliases, {} variants ({} with a clip)",
        census.aliases, census.variants, census.with_clip
    ));
    Ok(catalog)
}

/// The pictures the menus build from table cells (`menu_mp_weapons_tar21`
/// + `_big`, `perk_lightweight` + `_256`): every material whose name is a
/// cell, or a cell and a `_` suffix.
pub(super) fn table_picture_names(captures: &[&ZoneCapture]) -> std::collections::BTreeSet<String> {
    let cells: std::collections::HashSet<&str> = captures
        .iter()
        .flat_map(|c| c.string_tables.iter())
        .flat_map(|t| t.cells.iter())
        .map(String::as_str)
        .filter(|c| c.len() >= 4 && !c.contains(' '))
        .collect();
    let mut out = std::collections::BTreeSet::new();
    for c in captures {
        for m in &c.materials {
            let n = m.name.as_str();
            if cells.contains(n) || n.match_indices('_').any(|(i, _)| cells.contains(&n[..i])) {
                out.insert(n.to_owned());
            }
        }
    }
    out
}

/// BO2's loading screen pictures (its Loading menu shows
/// `loadscreen_<map>`): one per map whose zone is in his install, from the
/// front end's zones or the map packs' `dlc<N>_load_mp` zones (Nuketown
/// 2025's is in dlc0_load_mp), added to the front end's pictures.
fn with_loadscreens(
    mut icons: crate::T6HudIcons,
    zone_all: &Path,
    refs: &[&ZoneCapture],
    packs: Option<&PackSet>,
    report: &mut Vec<String>,
) -> crate::T6HudIcons {
    let Some(packs) = packs else { return icons };
    let loads: Vec<ZoneCapture> = (0..8)
        .map(|n| zone_all.join(format!("dlc{n}_load_mp.ff")))
        .filter(|p| p.is_file())
        .filter_map(|p| asset_t6::capture_zone(&p).ok())
        .collect();
    let mut added = Vec::new();
    // The latest zone's picture wins, as for every front-end picture (patch_mp
    // holds 2048 square `loadscreen_mp_*` that replace code_post_gfx_mp's):
    // walk the zones latest first and keep the first copy of a name.
    for c in loads.iter().rev().chain(refs.iter().copied().rev()) {
        for m in &c.materials {
            let Some(map) = m.name.strip_prefix("loadscreen_") else { continue };
            if !zone_all.join(format!("{map}.ff")).is_file() || icons.0.iter().any(|(k, _)| *k == m.name) {
                continue;
            }
            let Some(image) = super::t6_materials::colour_texture(m, c).and_then(|t| c.images.get(t.image?.index)) else {
                continue;
            };
            match super::t6_materials::decode(packs, image) {
                Ok(img) => {
                    icons.0.push((m.name.clone(), std::sync::Arc::new(img)));
                    added.push(m.name.clone());
                }
                Err(e) => report.push(format!("t6 front end: {}: {e}", m.name)),
            }
        }
    }
    report.push(format!("t6 front end: loading screen pictures {added:?}"));
    icons
}

/// The textures of the front end's 3D background (main.lua's lobby
/// backdrop: the holotable grids `ui_holotable_grid*` and the globe
/// `ui_globe`), as plain pixels by image name (`globe_map`, `plus_tile`,
/// ...): the UI draws them itself (`lui_scene`), as a 2D picture cannot
/// show a turned plane or a sphere.
fn with_scene_textures(
    mut icons: crate::T6HudIcons,
    refs: &[&ZoneCapture],
    packs: Option<&PackSet>,
    report: &mut Vec<String>,
) -> crate::T6HudIcons {
    let Some(packs) = packs else { return icons };
    let mut added = Vec::new();
    for name in ["ui_globe", "ui_holotable_grid", "ui_holotable_grid2", "ui_holotable_grid3"] {
        let Some((m, c)) = refs.iter().find_map(|c| c.materials.iter().find(|m| m.name == name).map(|m| (m, *c))) else {
            continue;
        };
        for t in &m.textures {
            let Some(image) = t.image.and_then(|k| c.images.get(k.index)) else {
                report.push(format!("t6 front end: {name} texture without an image"));
                continue;
            };
            report.push(format!("t6 front end: {name} texture {}", image.name));
            let key = format!("scene:{}", image.name);
            if icons.0.iter().any(|(k, _)| *k == key) {
                continue;
            }
            if let Ok(img) = super::t6_materials::decode(packs, image)
                && let Some(img) = super::t6_materials::to_rgba8(&img)
            {
                icons.0.push((key, std::sync::Arc::new(img)));
                added.push(image.name.clone());
            }
        }
    }
    report.push(format!("t6 front end: 3D backdrop textures {added:?}"));
    // Pictures the engine itself puts in the menus (no script names them):
    // the lobby rows' speaker.
    let glyphs = glyph_pictures(refs);
    for name in ["nottalkingicon", "talkingicon", "ui_arrow_left", "ui_arrow_right", "menu_theater_nodata", "lui_loader"]
        .into_iter()
        .map(str::to_owned)
        .chain(glyphs)
    {
        let name = name.as_str();
        let Some((m, c)) = refs.iter().find_map(|c| c.materials.iter().find(|m| m.name == name).map(|m| (m, *c))) else {
            continue;
        };
        if icons.0.iter().any(|(k, _)| k == name) {
            continue;
        }
        let Some(image) = super::t6_materials::colour_texture(m, c).and_then(|t| c.images.get(t.image?.index)) else { continue };
        if let Ok(img) = super::t6_materials::decode(packs, image) {
            icons.0.push((name.to_owned(), std::sync::Arc::new(img)));
        }
    }
    icons
}

/// The pictures of BO2's button glyphs (`^BBUTTON_MOUSE_LEFT^` ...): the
/// material each FontIcon entry names (the capture's `bo2mp/fonticon.csv`).
fn glyph_pictures(refs: &[&ZoneCapture]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in refs.iter().flat_map(|c| c.string_tables.iter()).filter(|t| t.name == "bo2mp/fonticon.csv") {
        for row in t.cells.chunks(t.columns.max(1)) {
            if let Some(m) = row.get(2).filter(|m| !m.is_empty()) && !out.contains(m) {
                out.push(m.clone());
            }
        }
    }
    out
}

/// bo2mp: a picture's top level as plain RGBA8 (block formats decoded), for
/// the UI to blur.
pub fn plain_rgba8(image: &bevy::prelude::Image) -> Option<bevy::prelude::Image> {
    super::t6_materials::to_rgba8(image)
}
