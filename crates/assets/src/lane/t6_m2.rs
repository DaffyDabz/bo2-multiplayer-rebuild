//! bo2zm M2: a Black Ops II map's guns, arms, animations, effects and
//! sound, from every zone the map loads.
//!
//! A Zombies map loads shared zones before its own (`code_post_gfx_zm`,
//! `common_zm`, `patch_zm`, ...) and its patch after. Each is walked once
//! and captured; later zones win a name they share with an earlier one
//! (the patch's weapons replace the map's). Comma-named assets are
//! references to another zone's and are skipped.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use asset_anim::XAnimBuild;
use asset_game::{FxCatalog, OwnedFxImpactTable, WeaponBuild, WeaponCatalog};
use asset_material::MaterialCatalog;
use asset_model::{FpvMeshBuild, WorldWeaponBuild};
use asset_t6::{PackSet, ZoneCapture};

/// The arms a Zombies map gives its players, by map (from the map's own
/// script: `zm_nuked.gsc` sets `c_zom_suit_viewhands` for the CIA player and
/// `c_zom_hazmat_viewhands_light` for CDC).
pub const T6_PLAYER_ARMS: &str = "c_zom_suit_viewhands";
const T6_OTHER_ARMS: [&str; 1] = ["c_zom_hazmat_viewhands_light"];

/// Zones a Zombies map loads before its own, in order.
const T6_ZM_PRE_ZONES: [&str; 4] = ["code_post_gfx_zm", "common_zm", "patch_zm", "patch_ui_zm"];

/// bo2mp: zones a multiplayer map loads before its own, in order (later
/// zones win a name: `common_patch_mp`'s guns replace `common_mp`'s, the
/// patch's scripts and tables everything before it). Then the map's two
/// factions (`faction_<name>_mp`, from `mp/mapstable.csv`).
const T6_MP_PRE_ZONES: [&str; 6] = [
    "code_post_gfx_mp",
    "common_mp",
    "common_patch_mp",
    "patch_mp",
    "ui_mp",
    "patch_ui_mp",
];

/// bo2mp: the English text and voice zones a multiplayer map reads
/// (`zone/english/en_<zone>.ff`), the factions' and map's added after.
const T6_MP_LANG_ZONES: [&str; 5] = [
    "code_post_gfx_mp",
    "common_mp",
    "patch_mp",
    "ui_mp",
    "patch_ui_mp",
];

/// bo2mp: a Black Ops II map is multiplayer (`mp_*`) or Zombies (`zm_*`).
pub(crate) fn is_mp_map(map: &str) -> bool {
    map.starts_with("mp_")
}

pub(crate) struct T6Combat {
    pub weapons: WeaponBuild,
    pub fpv: FpvMeshBuild,
    pub world_weapons: WorldWeaponBuild,
    pub xanims: XAnimBuild,
    pub fx: FxCatalog,
    /// The models effects draw (shell casings, debris).
    pub fx_models: asset_game::FxModelCatalog,
    /// The tracers every zone defines (each weapon names its own).
    pub tracers: asset_game::TracerCatalog,
    pub impact_fx: Option<OwnedFxImpactTable>,
    pub sound: Result<asset_audio::SoundCatalog, String>,
    /// The map's footstep tables (first person first), alias ids per
    /// surface and step kind.
    pub footsteps: Vec<asset_t6::FootstepTableRef>,
    /// The default ambient room's tone, from the map's ambience script.
    pub room_tone: Option<String>,
    /// bo2zm M3: every zone's compiled scripts, string tables and entity
    /// strings, in load order.
    pub scripts: crate::T6ScriptSet,
    /// bo2zm M3: the Zombies HUD's icons.
    pub hud_icons: crate::T6HudIcons,
    /// bo2zm M3: the Zombies HUD's fonts.
    pub hud_fonts: crate::T6HudFonts,
    /// bo2zm M4: Black Ops II's UI scripts and what they read.
    pub ui: crate::T6Ui,
    pub report: Vec<String>,
}

/// The Zombies HUD's icons: the perks a Nuketown machine sells and the
/// power-ups that last a while (their materials' colour maps).
const HUD_ICONS: [&str; 21] = [
    "specialty_juggernaut_zombies",
    "specialty_quickrevive_zombies",
    "specialty_fastreload_zombies",
    "specialty_doubletap_zombies",
    "specialty_instakill_zombies",
    "specialty_doublepoints_zombies",
    "specialty_firesale_zombies",
    // The round's chalk marks, rounds 1 to 5 (hudroundstatuszombie.lua).
    "hud_chalk_1",
    "hud_chalk_2",
    "hud_chalk_3",
    "hud_chalk_4",
    "hud_chalk_5",
    // The grenades and mines he carries, one icon each (offhandicons.lua):
    // frag, Semtex, monkey, Claymore.
    "hud_us_grenade",
    "hud_icon_sticky_grenade",
    "hud_cymbal_monkey",
    "hud_icon_claymore",
    // The red screen-edge overlay the health script fades in when he is hit
    // (_zm_playerhealth's healthoverlay).
    "overlay_low_health",
    // The guns' hip-fire crosshair: its four lines and the centre pieces
    // some guns name (weapon defs' reticleSide / reticleCenter).
    "reticle_side_small",
    "reticle_center_cross",
    "reticle_flechette",
    // The orange YOU pin the killcam draws over the player who was killed.
    "headiconyouinkillcam",
];

/// The HUD's fonts by the Lua scripts' names and their files, as
/// `ui/t6/codbase.lua` registers them for English.
const HUD_FONTS: [(&str, &str); 7] = [
    ("Default", "fonts/720/normalFont"),
    ("Condensed", "fonts/720/smallFont"),
    ("Big", "fonts/720/bigFont"),
    ("Morris", "fonts/720/extraBigFont"),
    ("ExtraSmall", "fonts/720/extraSmallFont"),
    ("Italic", "fonts/720/italicFont"),
    ("SmallItalic", "fonts/720/smallItalicFont"),
];

pub(super) fn hud_fonts(
    captures: &[ZoneCapture],
    packs: Option<&PackSet>,
    report: &mut Vec<String>,
) -> crate::T6HudFonts {
    let mut out = crate::T6HudFonts::default();
    let mut sheet_material = None;
    for (name, file) in HUD_FONTS {
        let Some(font) = captures
            .iter()
            .rev()
            .flat_map(|c| c.fonts.iter())
            .find(|f| f.name.eq_ignore_ascii_case(file))
        else {
            report.push(format!("t6 hud font {name}: {file} missing"));
            continue;
        };
        sheet_material.get_or_insert_with(|| font.material.clone());
        out.fonts.push(crate::T6HudFont {
            name: name.to_owned(),
            pixel_height: font.pixel_height as f32,
            glyphs: font
                .glyphs
                .iter()
                .map(|g| {
                    (
                        g.letter,
                        [
                            i16::from(g.x0),
                            i16::from(g.y0),
                            i16::from(g.dx),
                            i16::from(g.pixel_width),
                            i16::from(g.pixel_height),
                        ],
                        g.st,
                    )
                })
                .collect(),
        });
    }
    // Every one of them draws from the same sheet (`gamefonts_pc`).
    if let (Some(material), Some(packs)) = (sheet_material, packs) {
        let image = captures.iter().find_map(|c| {
            let m = c.materials.iter().find(|m| m.name == material)?;
            let t = m.textures.first()?;
            c.images.get(t.image?.index)
        });
        match image.map(|i| super::t6_materials::decode(packs, i)) {
            Some(Ok(img)) => out.sheet = Some(std::sync::Arc::new(img)),
            Some(Err(e)) => report.push(format!("t6 hud font sheet {material}: {e}")),
            None => report.push(format!("t6 hud font sheet {material}: no image")),
        }
    }
    report.push(format!(
        "t6 hud fonts: {} ({})",
        out.fonts.len(),
        out.fonts
            .iter()
            .map(|f| format!("{} {}px {} glyphs", f.name, f.pixel_height, f.glyphs.len()))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    out
}

/// bo2zm M4: every UI script (`.lua` rawfile) the zones hold, in load
/// order.
/// bo2mp: the zones' config files (`*.cfg`) as text, by name (a later
/// zone's wins).
pub(super) fn ui_configs(captures: &[&ZoneCapture]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (name, bytes) in captures.iter().flat_map(|c| c.raw_files.iter()) {
        if !name.to_ascii_lowercase().ends_with(".cfg") {
            continue;
        }
        let text = String::from_utf8_lossy(bytes).trim_end_matches('\0').to_owned();
        out.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
        out.push((name.clone(), text));
    }
    out
}

pub(super) fn ui_scripts(captures: &[&ZoneCapture]) -> Vec<(String, std::sync::Arc<[u8]>)> {
    captures
        .iter()
        .flat_map(|c| c.raw_files.iter())
        .filter(|(name, _)| name.to_ascii_lowercase().ends_with(".lua"))
        .map(|(name, bytes)| (name.clone(), std::sync::Arc::from(bytes.as_slice())))
        .collect()
}

/// bo2zm M4: the material names the UI scripts may draw: every string
/// constant in them (`RegisterMaterial("hud_chalk_1")`).
pub(super) fn ui_strings(scripts: &[(String, std::sync::Arc<[u8]>)]) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    for (_, bytes) in scripts {
        let Ok(chunk) = hks_t6::parse(bytes) else {
            continue;
        };
        let mut stack = vec![&chunk.main];
        while let Some(p) = stack.pop() {
            for k in &p.consts {
                if let hks_t6::Const::String(s) = k
                    && s.len() < 64
                {
                    out.insert(String::from_utf8_lossy(s).into_owned());
                }
            }
            stack.extend(p.protos.iter().map(|c| &**c));
        }
    }
    out
}

pub(super) fn hud_icons(
    captures: &[&ZoneCapture],
    packs: Option<&PackSet>,
    ui_names: &std::collections::BTreeSet<String>,
    report: &mut Vec<String>,
) -> crate::T6HudIcons {
    let mut icons = Vec::new();
    let Some(packs) = packs else {
        return crate::T6HudIcons(icons);
    };
    // The materials the UI scripts name, beside the hand-picked ones.
    let mut wanted: Vec<String> = HUD_ICONS.iter().map(|s| (*s).to_owned()).collect();
    let mut seen: std::collections::BTreeSet<String> = wanted.iter().cloned().collect();
    for c in captures {
        for m in &c.materials {
            if ui_names.contains(&m.name) && seen.insert(m.name.clone()) {
                wanted.push(m.name.clone());
            }
        }
    }
    let mut failed = 0usize;
    let mut additive = 0usize;
    for name in &wanted {
        let name = name.as_str();
        let mut add = false;
        let mut add_map = None;
        let found = captures.iter().find_map(|c| {
            let m = c.materials.iter().find(|m| m.name == name)?;
            add = super::t6_materials::is_additive(m);
            add_map = super::t6_materials::add_map(m, c);
            // BO2MP_ICON_STATES=<substring>: print the matching pictures' draw states (debugging aid).
            if let Ok(sub) = std::env::var("BO2MP_ICON_STATES")
                && name.contains(sub.as_str())
            {
                for t in 0..36 {
                    if let Some(d) = m.draw_state(t) {
                        eprintln!("icon-state {name} tech {t}: src {} dst {} op {} atest {:?} rgb {} a {}", d.src_blend, d.dst_blend, d.blend_op, d.alpha_test, d.color_write_rgb, d.color_write_alpha);
                    }
                }
            }
            let t = super::t6_materials::colour_texture(m, c)?;
            c.images.get(t.image?.index)
        });
        let Some(image) = found else {
            if HUD_ICONS.contains(&name) {
                report.push(format!("t6 hud icon {name}: no material"));
            }
            failed += 1;
            continue;
        };
        // A `,name` image is a reference to one another zone defines (the
        // Create-a-Class perk pictures `perk_*_256` in ui_mp): its pixels
        // are found in the packs by the name alone.
        let decoded = super::t6_materials::decode(packs, image).or_else(|e| {
            if !image.name.starts_with(',') {
                return Err(e);
            }
            let (i, entry) = packs.locate_name(&image.name).ok_or(e)?;
            let bytes = packs.packs[i].read(entry)?;
            let iwi = ipak_t6::parse_iwi(&bytes).map_err(|e| e.to_string())?;
            super::t6_materials::gpu_image(&iwi)
        });
        match decoded {
            Ok(img) => {
                // BO2MP_ICON_DUMP=<dir>:<substring> writes the matching
                // pictures there as PNGs, as stored (a debugging aid).
                if let Ok(spec) = std::env::var("BO2MP_ICON_DUMP")
                    && let Some((dir, sub)) = spec.rsplit_once('|')
                    && name.contains(sub)
                    && let Some(plain) = super::t6_materials::to_rgba8(&img)
                    && let Some(data) = plain.data.as_ref()
                    && let Ok(file) = std::fs::File::create(format!("{dir}/{name}{}.png", if add { "_ADD" } else { "" }))
                {
                    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), plain.width(), plain.height());
                    enc.set_color(png::ColorType::Rgba);
                    enc.set_depth(png::BitDepth::Eight);
                    if let Ok(mut w) = enc.write_header() {
                        let _ = w.write_image_data(data);
                    }
                }
                // bo2mp: the HUD adds these onto the scene (true additive
                // blending), so keep the pictures as stored under
                // "additive:<name>" next to the see-through copy.
                // bo2mp vehicle screens: a picture with an AddMap (BO2's
                // `sw4_2d_color_add` pixel shader: the colour map's alpha
                // times the element's colour, plus the AddMap's colour, all
                // times its alpha) is kept packed (AddMap colour, colour
                // map alpha) under "additive:" and marked "coloradd:".
                let packed = add_map
                    .and_then(|i| super::t6_materials::decode(packs, i).ok())
                    .and_then(|added| super::t6_materials::pack_color_add(&img, &added));
                let img = match packed {
                    Some((packed, flat)) => {
                        icons.push((format!("coloradd:{name}"), std::sync::Arc::new(packed.clone())));
                        if add {
                            icons.push((format!("additive:{name}"), std::sync::Arc::new(packed)));
                        }
                        flat
                    }
                    None => {
                        if add && let Some(orig) = super::t6_materials::to_rgba8(&img) {
                            icons.push((format!("additive:{name}"), std::sync::Arc::new(orig)));
                        }
                        img
                    }
                };
                // Additive pictures (menu brackets, glows) get their
                // brightness as alpha: the 2D layer only alpha-blends.
                let img = match add.then(|| super::t6_materials::additive_to_alpha(&img)).flatten() {
                    Some(a) => {
                        additive += 1;
                        a
                    }
                    // bo2mp: the minimap's picture as plain pixels (the
                    // HUD turns it about him on the CPU).
                    None if name.starts_with("compass_map_") => super::t6_materials::to_rgba8(&img).unwrap_or(img),
                    None => img,
                };
                // bo2mp: the default emblem on a player's card (BO2 draws the
                // rank's emblem layer, `em_rank_pvt_full`, in the rank's
                // green): the PFC chevron as a white silhouette the
                // card tints.
                if name == "rank_pfc"
                    && let Some(mut sil) = super::t6_materials::to_rgba8(&img)
                    && let Some(data) = sil.data.as_mut()
                {
                    for px in data.chunks_exact_mut(4) {
                        px[0] = 255;
                        px[1] = 255;
                        px[2] = 255;
                    }
                    icons.push(("em_rank_pvt_full".to_owned(), std::sync::Arc::new(sil)));
                }
                icons.push((name.to_owned(), std::sync::Arc::new(img)));
            }
            Err(e) => {
                if HUD_ICONS.contains(&name) {
                    report.push(format!("t6 hud icon {name}: {e}"));
                }
                failed += 1;
            }
        }
    }
    // bo2mp: pictures the scripts name that no material defines; BO2 finds
    // their pixels in the packs by the name alone (the green "NEW" badge on
    // a Create-a-Class / Barracks tile).
    for name in ["menu_mp_lobby_new_small"] {
        if icons.iter().any(|(k, _)| k == name) || !ui_names.contains(name) {
            continue;
        }
        let Some((i, entry)) = packs.locate_name(name) else { continue };
        let Ok(bytes) = packs.packs[i].read(entry) else { continue };
        let Ok(iwi) = ipak_t6::parse_iwi(&bytes) else { continue };
        if let Ok(img) = super::t6_materials::gpu_image(&iwi) {
            icons.push((name.to_owned(), std::sync::Arc::new(img)));
        }
    }
    // bo2mp: each weapon's kill icon (BO2's kill feed draws it between the
    // two names), as "killicon:<weapon>:<ratio>" (WeaponDef killIcon and
    // killIconRatio: 0 square, 1 twice as wide, 2 four times).
    let mut kill_icons = 0usize;
    for c in captures {
        for w in &c.weapons {
            let Some((_, material)) = w.materials.iter().find(|(off, _)| *off == 1632) else { continue };
            let base = w.name.split('+').next().unwrap_or(&w.name).trim_end_matches("_mp").to_owned();
            let prefix = format!("killicon:{base}:");
            if base.is_empty() || icons.iter().any(|(k, _)| k.starts_with(&prefix)) {
                continue;
            }
            let ratio = w.def.get(1636..1640).map_or(0, |b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]));
            let mut add = false;
            let found = captures.iter().find_map(|c| {
                let m = c.materials.iter().find(|m| m.name == *material)?;
                add = super::t6_materials::is_additive(m);
                let t = super::t6_materials::colour_texture(m, c)?;
                c.images.get(t.image?.index)
            });
            let Some(image) = found else { continue };
            if let Ok(img) = super::t6_materials::decode(packs, image) {
                let img = add.then(|| super::t6_materials::additive_to_alpha(&img)).flatten().unwrap_or(img);
                icons.push((format!("{prefix}{ratio}"), std::sync::Arc::new(img)));
                kill_icons += 1;
            }
        }
    }
    // The pictures the engine draws when no gun's icon applies
    // (`hud_obit_death_suicide`, `_falling`,
    // `_crush`, `_grenade_round`, `hud_obit_knife`, `killicondied`,
    // `killiconheadshot`), each as a square icon under its own name.
    let mut obit_names: Vec<String> = Vec::new();
    for c in captures {
        for m in &c.materials {
            if !(m.name.starts_with("hud_obit_") || m.name.starts_with("killicon")) || obit_names.contains(&m.name) {
                continue;
            }
            obit_names.push(m.name.clone());
            let key = format!("killicon:{}:0", m.name);
            if icons.iter().any(|(k, _)| *k == key) {
                continue;
            }
            let add = super::t6_materials::is_additive(m);
            let Some(t) = super::t6_materials::colour_texture(m, c) else { continue };
            let Some(image) = t.image.and_then(|i| c.images.get(i.index)) else { continue };
            if let Ok(img) = super::t6_materials::decode(packs, image) {
                let img = add.then(|| super::t6_materials::additive_to_alpha(&img)).flatten().unwrap_or(img);
                icons.push((key, std::sync::Arc::new(img)));
                kill_icons += 1;
            }
        }
    }
    report.push(format!("t6 kill icons: {kill_icons}"));
    report.push(format!("t6 obituary icon materials: {}", obit_names.join(" ")));
    // bo2mp: each gun's scope picture (its variant's overlayMaterial, e.g.
    // `scope_overlay_svu`), as plain pixels keyed "overlay:<material>": the
    // HUD draws it over the zoomed view and samples its black frame for the
    // bars beside it.
    let mut overlays = 0usize;
    for c in captures {
        let unique_overlays = c
            .attachment_uniques
            .iter()
            .flat_map(|u| [&u.overlay_material, &u.overlay_material_low]);
        for material in c
            .weapons
            .iter()
            .flat_map(|w| [&w.overlay_material, &w.overlay_material_low])
            .chain(unique_overlays)
        {
            let key = format!("overlay:{material}");
            if material.is_empty() || icons.iter().any(|(k, _)| *k == key) {
                continue;
            }
            let found = captures.iter().find_map(|c| {
                let m = c.materials.iter().find(|m| m.name == *material)?;
                let t = super::t6_materials::colour_texture(m, c)?;
                c.images.get(t.image?.index)
            });
            let Some(image) = found else {
                report.push(format!("t6 scope overlay {material}: no picture"));
                continue;
            };
            match super::t6_materials::decode(packs, image) {
                Ok(img) => {
                    let img = super::t6_materials::to_rgba8(&img).unwrap_or(img);
                    icons.push((key, std::sync::Arc::new(img)));
                    overlays += 1;
                }
                Err(e) => report.push(format!("t6 scope overlay {material}: {e}")),
            }
        }
    }
    report.push(format!("t6 scope overlays: {overlays}"));
    // bo2mp: BO2's hit blood (`overlay_low_health_splat`, technique
    // sw4_2d_blood) is two pictures the HUD mixes: the blood's colour
    // (iw2_blood_color) and the reveal map (iw2_blood_reveal: how far in the
    // blood shows); both as plain pixels, by their image names.
    if let Some((m, c)) = captures
        .iter()
        .find_map(|c| c.materials.iter().find(|m| m.name == "overlay_low_health_splat").map(|m| (m, c)))
    {
        for t in &m.textures {
            let Some(image) = t.image.and_then(|k| c.images.get(k.index)) else { continue };
            if let Ok(img) = super::t6_materials::decode(packs, image)
                && let Some(img) = super::t6_materials::to_rgba8(&img)
            {
                icons.push((image.name.clone(), std::sync::Arc::new(img)));
            }
        }
    }
    report.push(format!(
        "t6 hud icons: {} of {} ({} hand-picked, the rest the UI scripts name; {failed} without a picture; {additive} additive made see-through)",
        icons.len(),
        wanted.len(),
        HUD_ICONS.len()
    ));
    crate::T6HudIcons(icons)
}

/// Nuketown's weapons: every base gun the box and walls give, the starting
/// pistol, knives and equipment.
const NUKETOWN_WEAPONS: [&str; 40] = [
    "m1911_zm",
    "beretta93r_zm",
    "fiveseven_zm",
    "fivesevendw_zm",
    "kard_zm",
    "python_zm",
    "judge_zm",
    "ak74u_zm",
    "mp5k_zm",
    "qcw05_zm",
    "870mcs_zm",
    "rottweil72_zm",
    "saiga12_zm",
    "srm1216_zm",
    "m14_zm",
    "m16_zm",
    "fnfal_zm",
    "galil_zm",
    "hk416_zm",
    "saritch_zm",
    "tar21_zm",
    "type95_zm",
    "xm8_zm",
    "hamr_zm",
    "lsat_zm",
    "rpd_zm",
    "barretm82_zm",
    "dsr50_zm",
    "ray_gun_zm",
    "raygun_mark2_zm",
    "knife_ballistic_zm",
    "m32_zm",
    "usrpg_zm",
    "knife_zm",
    "bowie_knife_zm",
    "tazer_knuckles_zm",
    "frag_grenade_zm",
    "sticky_grenade_zm",
    "claymore_zm",
    "cymbal_monkey_zm",
];

/// The M2 census: every Nuketown weapon complete (first-person and world
/// models, launcher and melee models, magazine and scope attachments,
/// animations), and nothing the weapons,
/// their animations, the impact table or the map scripts name missing from
/// the effects or the sound bank. A sound BO2's own banks lack (an
/// animation's cue Nuketown never shipped) is silent in BO2 too: counted
/// apart, as is an alias BO2 ships without audio.
#[allow(clippy::too_many_arguments)]
fn census(
    captures: &[&ZoneCapture],
    names: &[String],
    weapon_refs: &HashMap<String, &asset_t6::WeaponRef>,
    models_missing: &[String],
    xanims: &XAnimBuild,
    fx: &FxCatalog,
    impact: Option<&OwnedFxImpactTable>,
    map_fx: &asset_audio::ScriptedMapFx,
    room_tone: Option<&str>,
    sound: Option<&asset_audio::SoundCatalog>,
) -> Vec<String> {
    use fastfile_t6::layout::WeaponDef as d;
    let ns = asset_core::AssetNamespace::T6;
    let mut missing_weapons: Vec<String> = Vec::new();
    let mut missing_fx: Vec<String> = Vec::new();
    let mut missing_sounds: Vec<String> = Vec::new();
    let mut absent_in_bo2: Vec<String> = Vec::new();
    let mut silent: usize = 0;
    let (mut fx_n, mut snd_n) = (0usize, 0usize);
    let anim_notifies: HashMap<String, &asset_t6::XAnimRef> = captures
        .iter()
        .flat_map(|c| c.xanims.iter())
        .filter(|a| !a.name.starts_with(','))
        .map(|a| (a.name.to_ascii_lowercase(), a))
        .collect();
    let aliases: HashMap<String, &asset_audio::CapturedSound> = sound
        .map(|cat| {
            cat.sounds
                .iter()
                .map(|s| (s.name.to_ascii_lowercase(), s))
                .collect()
        })
        .unwrap_or_default();
    let mut check_fx = |name: &str, what: &str, missing: &mut Vec<String>| {
        if name.is_empty() {
            return;
        }
        fx_n += 1;
        if fx.get_in(ns, name.trim_start_matches(',')).is_none() {
            missing.push(format!("{what}: {name}"));
        }
    };
    // A sound named by the data: in the bank with audio (or shipped silent),
    // else absent from BO2's own banks (silent in BO2 too). Missing means
    // in the bank but no audio found: a failure here.
    let mut check_sound = |name: &str, _from_anim: bool, what: &str| {
        if name.is_empty() {
            return;
        }
        snd_n += 1;
        match aliases.get(&name.to_ascii_lowercase()) {
            None => absent_in_bo2.push(name.to_owned()),
            Some(sound) => {
                let audible = sound.aliases.iter().any(|row| row.streamed.is_some());
                let shipped_silent = sound.aliases.iter().all(|row| {
                    row.streamed.is_none() && row.file_name.as_deref().unwrap_or("").is_empty()
                });
                if !audible {
                    if shipped_silent {
                        silent += 1;
                    } else {
                        missing_sounds.push(format!("{what}: {name} (no audio)"));
                    }
                }
            }
        }
    };
    for name in names.iter().map(String::as_str) {
        let Some(w) = weapon_refs.get(name) else {
            missing_weapons.push(format!("{name}: not in the zones"));
            continue;
        };
        let mut gaps = Vec::new();
        for model in w
            .gun_models
            .first()
            .into_iter()
            .chain(w.world_models.first())
            .map(String::as_str)
            .chain(
                [d::rocketModel, d::projectileModel, d::additionalMeleeModel]
                    .map(|f| w.def_model(f)),
            )
            .chain(w.attach_view_models.iter().map(|a| a.1.as_str()))
            .filter(|m| !m.is_empty() && !m.starts_with(','))
        {
            if models_missing.iter().any(|m| m == model) {
                gaps.push(format!("model {model}"));
            }
        }
        for anim in w.xanims.iter().filter(|a| !a.is_empty()) {
            let key = anim.trim_start_matches(',').to_ascii_lowercase();
            if xanims.get(ns, &key).is_none() {
                gaps.push(format!("anim {anim}"));
            }
            if let Some(a) = anim_notifies.get(&key) {
                for (note, _) in &a.notifies {
                    if let Some(alias) = note.strip_prefix("sndnt#") {
                        check_sound(alias, true, name);
                    }
                }
            }
        }
        if !gaps.is_empty() {
            missing_weapons.push(format!("{name}: {}", gaps.join(", ")));
        }
        for field in [
            d::viewFlashEffect,
            d::worldFlashEffect,
            d::viewShellEjectEffect,
            d::worldShellEjectEffect,
            d::viewLastShotEjectEffect,
            d::worldLastShotEjectEffect,
            d::projExplosionEffect,
            d::projTrailEffect,
            d::projIgnitionEffect,
        ] {
            check_fx(w.def_fx(field), name, &mut missing_fx);
        }
        for field in [
            d::fireSound,
            d::fireSoundPlayer,
            d::emptyFireSound,
            d::emptyFireSoundPlayer,
            d::reloadSound,
            d::reloadSoundPlayer,
            d::reloadEmptySound,
            d::reloadEmptySoundPlayer,
            d::raiseSound,
            d::raiseSoundPlayer,
            d::putawaySound,
            d::putawaySoundPlayer,
            d::projExplosionSound,
            d::meleeSwipeSoundPlayer,
            d::meleeSwipeSound,
            d::meleeHitSound,
            d::meleeMissSound,
            d::pullbackSoundPlayer,
        ] {
            check_sound(w.def_string(field), false, name);
        }
        for alias in &w.bounce_sounds {
            check_sound(alias, false, name);
        }
        // A map value may name the player's alias then the others'.
        for (_, value) in &w.notetrack_sounds {
            for alias in value.split_whitespace() {
                check_sound(alias, false, name);
            }
        }
    }
    if let Some(table) = impact {
        for row in 0..table.row_count() {
            for surf in 0..40 {
                if let Some(n) = table.effect_name(row, surf, Some(0)) {
                    check_fx(n.name, "impact table", &mut missing_fx);
                }
            }
            for flesh in 0..8 {
                if let Some(n) = table.effect_name(row, 7, Some(flesh)) {
                    check_fx(n.name, "impact table", &mut missing_fx);
                }
            }
        }
    }
    for shot in &map_fx.oneshots {
        check_fx(&shot.fxid, "map", &mut missing_fx);
    }
    for l in &map_fx.loop_sounds {
        check_sound(&l.soundalias, false, "map loop");
    }
    for r in &map_fx.random_sounds {
        check_sound(&r.soundalias, false, "map");
    }
    if let Some(tone) = room_tone {
        check_sound(tone, false, "room tone");
    }
    for gait in [
        "sprint",
        "run",
        "walk",
        "prone",
        "crouch_run",
        "crouch_walk",
    ] {
        for surf in [
            "default", "asphalt", "concrete", "dirt", "metal", "wood", "grass", "gravel",
        ] {
            check_sound(&format!("fly_step_{gait}_plr_{surf}"), false, "footsteps");
        }
    }
    for surf in [
        "default", "asphalt", "concrete", "dirt", "metal", "wood", "grass", "gravel",
    ] {
        check_sound(&format!("fly_land_plr_{surf}"), false, "landings");
    }
    missing_fx.sort();
    missing_fx.dedup();
    missing_sounds.sort();
    missing_sounds.dedup();
    absent_in_bo2.sort();
    absent_in_bo2.dedup();
    vec![
        format!(
            "t6 census: {} missing weapons of {} (models, first person, animations){}",
            missing_weapons.len(),
            names.len(),
            if missing_weapons.is_empty() {
                String::new()
            } else {
                format!(": {missing_weapons:?}")
            }
        ),
        format!(
            "t6 census: {} missing FX of {fx_n} named (weapons, impacts, map){}",
            missing_fx.len(),
            if missing_fx.is_empty() {
                String::new()
            } else {
                format!(": {:?}", &missing_fx[..missing_fx.len().min(12)])
            }
        ),
        format!(
            "t6 census: {} missing sounds of {snd_n} named (weapons, animations, footsteps, landings, map){}; {} names BO2's own banks lack (silent in BO2 too) {:?}; {silent} shipped silent",
            missing_sounds.len(),
            if missing_sounds.is_empty() {
                String::new()
            } else {
                format!(": {:?}", &missing_sounds[..missing_sounds.len().min(12)])
            },
            absent_in_bo2.len(),
            &absent_in_bo2[..absent_in_bo2.len().min(8)],
        ),
    ]
}

/// The map's compiled client scripts, read for its placed effects and
/// ambience (later zones' copies win: the patch zone fixes the map's).
fn map_scripts(
    captures: &[&ZoneCapture],
    map: &str,
    report: &mut Vec<String>,
) -> (asset_audio::ScriptedMapFx, Option<String>) {
    let facts = asset_t6::MapScriptFacts::read(map, |name| {
        let want = format!("script:{name}");
        captures.iter().rev().find_map(|c| {
            c.raw_files
                .iter()
                .find(|(n, _)| *n == want)
                .map(|(_, bytes)| bytes.clone())
        })
    });
    let shown: Vec<&asset_t6::PlacedFx> = facts
        .placed
        .iter()
        .filter(|p| p.exploder.is_none() && p.kind != "exploder")
        .collect();
    // IW4L_T6_PLACEDLOG=1: each shown placed effect (test aid).
    if std::env::var_os("IW4L_T6_PLACEDLOG").is_some() {
        for p in &shown {
            report.push(format!(
                "t6 placed fx {} = {} {} at {:?} angles {:?} delay {}",
                p.fxid,
                facts.effects.get(&p.fxid).map_or("?", String::as_str),
                p.kind,
                p.origin,
                p.angles,
                p.delay
            ));
        }
    }
    let oneshots: Vec<asset_audio::CreateFxOneshot> = shown
        .iter()
        .map(|p| asset_audio::CreateFxOneshot {
            fxid: facts
                .effects
                .get(&p.fxid)
                .cloned()
                .unwrap_or_else(|| p.fxid.clone()),
            origin_inches: p.origin,
            angles_deg: p.angles,
            // A loop effect's delay is its repeat time, not a pre-roll.
            delay: if p.kind == "loopfx" { 0.0 } else { p.delay },
        })
        .collect();
    let mut loop_sounds: Vec<asset_audio::CreateFxLoopSound> = Vec::new();
    for (fxid, alias, off) in &facts.sounds_on_fx {
        for p in shown.iter().filter(|p| &p.fxid == fxid) {
            loop_sounds.push(asset_audio::CreateFxLoopSound {
                soundalias: alias.clone(),
                origin_inches: [0, 1, 2].map(|a| p.origin[a] + off[a]),
            });
        }
    }
    for (alias, origin) in &facts.loops_at {
        loop_sounds.push(asset_audio::CreateFxLoopSound {
            soundalias: alias.clone(),
            origin_inches: *origin,
        });
    }
    let random_sounds: Vec<asset_audio::RandomPointSound> = facts
        .random_sounds
        .iter()
        .map(|(alias, wait, places)| asset_audio::RandomPointSound {
            soundalias: alias.clone(),
            wait_secs: *wait,
            places_inches: places.clone(),
        })
        .collect();
    let unresolved = shown
        .iter()
        .filter(|p| !facts.effects.contains_key(&p.fxid))
        .count();
    report.push(format!(
        "t6 map scripts: {} placed effects ({} shown at load, {} exploders wait for the game, {unresolved} without an effect table entry); {} loop sounds ({} on effects, {} at points); {} now-and-then sounds; room tone {}",
        facts.placed.len(),
        oneshots.len(),
        facts.placed.len() - shown.len(),
        loop_sounds.len(),
        loop_sounds.len() - facts.loops_at.len(),
        facts.loops_at.len(),
        random_sounds.len(),
        facts.room_tone.as_deref().unwrap_or("none"),
    ));
    (
        asset_audio::ScriptedMapFx {
            oneshots,
            loop_sounds,
            random_sounds,
        },
        facts.room_tone,
    )
}

/// bo2mp: each destructible definition's model, by definition name.
fn destructible_models(captures: &[&ZoneCapture]) -> HashMap<String, String> {
    let mut defs = HashMap::new();
    for c in captures {
        for d in &c.destructibles {
            if real(&d.name) && real(&d.model) {
                defs.insert(d.name.to_ascii_lowercase(), d.model.clone());
            }
        }
    }
    defs
}

/// bo2mp: each destructible definition the server breaks (pieces, stages,
/// what each break plays and tells the scripts), later zones replacing
/// earlier; client-only ones are left to the clients.
fn destructible_defs(captures: &[&ZoneCapture]) -> Vec<std::sync::Arc<xmodel_runtime::T5DestructibleDef>> {
    let text = |s: &str| (!s.is_empty()).then(|| s.to_owned());
    let mut defs: Vec<std::sync::Arc<xmodel_runtime::T5DestructibleDef>> = Vec::new();
    for c in captures {
        for d in &c.destructibles {
            if !real(&d.name) || !real(&d.model) || d.pieces.is_empty() || d.client_only {
                continue;
            }
            let pieces = d
                .pieces
                .iter()
                .map(|p| xmodel_runtime::T5DestructiblePiece {
                    stages: p.stages.clone().map(|st| xmodel_runtime::T5DestructibleStage {
                        spawn_presets: st.spawn_models.clone().map(|m| debris_preset(captures, &m)),
                        show_bone: text(&st.show_bone),
                        break_health: st.break_health,
                        max_time: st.max_time,
                        flags: st.flags,
                        break_effect: text(&st.break_effect),
                        break_sound: text(&st.break_sound),
                        break_notify: text(&st.break_notify),
                        loop_sound: text(&st.loop_sound),
                        has_phys_preset: st.phys_preset,
                        spawn_models: st.spawn_models.clone().map(|m| text(&m)),
                    }),
                    parent_piece: p.parent_piece,
                    parent_damage_percent: p.parent_damage_percent,
                    bullet_damage_scale: p.bullet_damage_scale,
                    explosive_damage_scale: p.explosive_damage_scale,
                    melee_damage_scale: p.melee_damage_scale,
                    health: p.health,
                    hide_bones: p.hide_bones,
                })
                .collect();
            defs.retain(|x| !x.name.eq_ignore_ascii_case(&d.name));
            defs.push(std::sync::Arc::new(xmodel_runtime::T5DestructibleDef {
                name: d.name.clone(),
                model: d.model.clone(),
                pieces,
                client_only: false,
            }));
        }
    }
    defs
}

/// bo2mp: how a destructible's spawn model flies once it breaks off: its
/// model's `physPreset`, the latest zone's copy of the model winning.
fn debris_preset(captures: &[&ZoneCapture], model: &str) -> Option<xmodel_runtime::DebrisPreset> {
    if model.is_empty() {
        return None;
    }
    captures.iter().rev().find_map(|c| {
        let p = c
            .xmodels
            .iter()
            .filter(|x| x.name.trim_start_matches(',') == model)
            .find_map(|x| x.phys_preset)
            .and_then(|k| c.phys_presets.get(k.index))?;
        (p.mass > 0.0).then_some(xmodel_runtime::DebrisPreset {
            mass: p.mass,
            bounce: p.bounce,
            friction: p.friction,
            bullet_force_scale: p.bullet_force_scale,
            explosive_force_scale: p.explosive_force_scale,
            gravity_scale: p.gravity_scale,
        })
    })
}

/// bo2mp: a map's entity text with each destructible built from its
/// definition. An entity with a `destructibledef` carries the editor's
/// stand-in in `model` (`veh_t6_police_car`, no such model in any zone);
/// the engine builds it from the definition's model, so that is what the
/// entity is given here.
fn destructible_entities(text: &str, defs: &HashMap<String, String>) -> String {
    if defs.is_empty() || !text.contains("\"destructibledef\"") {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut block: Vec<&str> = Vec::new();
    let flush = |block: &mut Vec<&str>, out: &mut String| {
        let model = block
            .iter()
            .find_map(|l| {
                let mut q = l.split('"').skip(1).step_by(2);
                (q.next()?.eq_ignore_ascii_case("destructibledef")).then(|| q.next())?
            })
            .and_then(|d| defs.get(&d.to_ascii_lowercase()));
        for l in block.drain(..) {
            let is_model = l.split('"').nth(1).is_some_and(|k| k.eq_ignore_ascii_case("model"));
            match model {
                Some(m) if is_model => {
                    out.push_str(&format!("\"model\" \"{m}\""));
                }
                _ => out.push_str(l),
            }
            out.push_str("\n");
        }
    };
    for line in text.lines() {
        block.push(line);
        if line.trim() == "}" {
            flush(&mut block, &mut out);
        }
    }
    flush(&mut block, &mut out);
    out
}

/// bo2zm M3: every XModel the zones carry whose name a server script
/// (as a string constant), a map entity (`"model"`) or a destructible's
/// broken-off piece names.
fn script_model_names(
    captures: &[&ZoneCapture],
    destructibles: &HashMap<String, String>,
) -> BTreeSet<String> {
    let models: BTreeSet<&str> = captures
        .iter()
        .flat_map(|c| c.xmodels.iter().map(|m| m.name.as_str()))
        .filter(|n| real(n))
        .collect();
    let mut named = BTreeSet::new();
    let mut take = |s: &str| {
        if models.contains(s) {
            named.insert(s.to_owned());
        }
    };
    for c in captures {
        for (name, bytes) in &c.raw_files {
            if !name.starts_with("script:") || !name.ends_with(".gsc") {
                continue;
            }
            for token in bytes.split(|&b| b == 0) {
                if (3..96).contains(&token.len()) && token.iter().all(|b| b.is_ascii_graphic()) {
                    take(&String::from_utf8_lossy(token));
                }
            }
        }
        // Every gun's world model (the box's floating gun, bought wall guns).
        for w in &c.weapons {
            for m in &w.world_models {
                take(m);
            }
        }
        for d in &c.destructibles {
            for st in d.pieces.iter().flat_map(|p| &p.stages) {
                for m in &st.spawn_models {
                    take(m);
                }
            }
        }
        for ents in &c.map_ents {
            for e in asset_t6::parse_entities(&destructible_entities(ents, destructibles)) {
                if let Some(m) = e.get("model").filter(|m| !m.starts_with('*')) {
                    take(m);
                }
                // A zbarrier's pieces (the magic box) name their models.
                for i in 1..=8 {
                    if let Some(m) = e.get(&format!("zbarrierboardmodel{i}")) {
                        take(m);
                    }
                }
            }
        }
    }
    named
}

/// The zones around the map zone, in load order, that exist beside it.
fn zone_paths(map_path: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let dir = map_path.parent().unwrap_or(Path::new("."));
    let stem = map_path
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let shared: &[&str] = if is_mp_map(&stem) {
        &T6_MP_PRE_ZONES
    } else {
        &T6_ZM_PRE_ZONES
    };
    let mut pre: Vec<PathBuf> = shared
        .iter()
        .map(|z| dir.join(format!("{z}.ff")))
        .collect();
    // Nuketown is the first Zombies DLC map: its load zone carries the
    // intro's sound bank.
    if stem == "zm_nuked" {
        pre.push(dir.join("dlczm0_load_zm.ff"));
    }
    // A multiplayer map has no patch zone (its fixes are in patch_mp).
    let post = if is_mp_map(&stem) {
        Vec::new()
    } else {
        vec![dir.join(format!("{stem}_patch.ff"))]
    };
    let exists = |v: Vec<PathBuf>| v.into_iter().filter(|p| p.is_file()).collect();
    (exists(pre), exists(post))
}

/// bo2mp: a multiplayer map's two factions' zones, from `mp/mapstable.csv`
/// in the zones read so far: the map's row names the allies' and axis'
/// team sets (columns 1 and 2) and, on the snow, wet and sand maps, the
/// look in its last column (16): Downhill = `faction_seals_snow_mp` +
/// `faction_pmc_snow_mp`. A team set without that look loads its own zone.
const MAPSTABLE_TEAMS: [usize; 2] = [1, 2];
const MAPSTABLE_LOOK: usize = 16;

fn mp_faction_zones(
    captures: &[ZoneCapture],
    map: &str,
    dir: &Path,
    report: &mut Vec<String>,
) -> Vec<PathBuf> {
    let Some(t) = captures
        .iter()
        .rev()
        .flat_map(|c| c.string_tables.iter())
        .find(|t| t.name.eq_ignore_ascii_case("mp/mapstable.csv"))
    else {
        report.push("t6 mp factions: no mp/mapstable.csv".to_owned());
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = Vec::new();
    for r in 0..t.rows {
        let row = &t.cells[r * t.columns..((r + 1) * t.columns).min(t.cells.len())];
        if !row.first().is_some_and(|c| c.eq_ignore_ascii_case(map)) {
            continue;
        }
        let look = row
            .get(MAPSTABLE_LOOK)
            .filter(|c| !c.is_empty())
            .map(|c| format!("_{}", c.to_ascii_lowercase()))
            .unwrap_or_default();
        for team in MAPSTABLE_TEAMS.iter().filter_map(|&i| row.get(i)) {
            let team = team.to_ascii_lowercase();
            if team.is_empty() || team.len() >= 32 {
                continue;
            }
            let path = [
                format!("faction_{team}{look}_mp.ff"),
                format!("faction_{team}_mp.ff"),
            ]
            .into_iter()
            .map(|name| dir.join(name))
            .find(|p| p.is_file());
            if let Some(path) = path.filter(|p| !out.contains(p)) {
                out.push(path);
            }
        }
    }
    report.push(format!(
        "t6 mp factions for {map}: {}",
        out.iter()
            .filter_map(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(", ")
    ));
    out
}

/// bo2mp: a material drawn by BO2's sight reticle shader
/// (`sw4_3d_reticle_dynamic`: the reflex's dot, the EOTech's ring, the
/// ACOG's chevron), whose picture is projected to infinity along the sight;
/// (its capture, index) where it is defined.
fn reticle_material(
    captures: &[&ZoneCapture],
    homes: &HashMap<String, (usize, usize)>,
    ci: usize,
    key: asset_t6::AssetKey,
) -> Option<(usize, usize)> {
    let m = captures[ci].materials.get(key.index)?;
    let (hc, hm) = if m.name.starts_with(',') {
        *homes.get(m.name.trim_start_matches(','))?
    } else {
        (ci, key.index)
    };
    let def = captures[hc].materials.get(hm)?;
    let set = def
        .technique_set
        .and_then(|k| captures[hc].technique_sets.get(k.index))?;
    set.name.contains("reticle_dynamic").then_some((hc, hm))
}

/// bo2mp: a sight model's reticle as Black Ops II's shader draws it (fxc
/// disassembly of `sw4_3d_reticle_dynamic`): its picture's red channel is
/// the dot or ring in `Reticle_Color`, its green the glow around it raised
/// to `Glow_Falloff` in `Glow_Color`, both times `Emissive_Amount`, added on
/// (the picture is that colour, for the HUD to add onto the scene). Returns the picture and `Color_Map_Scale`
/// (the picture spans 1 / scale in tangent of the view angle).
fn sight_reticle(
    captures: &[&ZoneCapture],
    homes: &HashMap<String, (usize, usize)>,
    packs: &PackSet,
    model: &str,
    top: bool,
) -> Option<(f32, bevy::prelude::Image)> {
    let (ci, m) = captures
        .iter()
        .enumerate()
        .rev()
        .find_map(|(ci, c)| c.xmodels.iter().find(|m| m.name == model).map(|m| (ci, m)))?;
    // The Hybrid's two reticles: the scope's (`_down`) and the little dot
    // above it (`_up`), one for each of its two modes (`top`: the dot's).
    let found: Vec<(usize, usize)> = m
        .lod0
        .iter()
        .filter_map(|s| s.material)
        .chain(m.materials.iter().flatten().copied())
        .filter_map(|k| reticle_material(captures, homes, ci, k))
        .collect();
    let (hc, hm) = found
        .iter()
        .copied()
        .find(|&(hc, hm)| captures[hc].materials[hm].name.ends_with("_up") == top)
        .or_else(|| found.first().copied())?;
    let c = captures[hc];
    let mat = &c.materials[hm];
    let constant = |prefix: &str| {
        mat.constants
            .iter()
            .find(|(_, name, _)| name.starts_with(prefix))
            .map(|(_, _, v)| *v)
    };
    let scale = constant("Color_Map_Sc")?[0];
    let core = constant("Reticle_Colo").unwrap_or([1.0; 4]);
    let glow = constant("Glow_Color").unwrap_or([1.0, 0.0, 0.0, 1.0]);
    let falloff = constant("Glow_Falloff").map_or(1.0, |v| v[0]);
    let emissive = constant("Emissive_Amo").map_or(1.0, |v| v[0]);
    let noise = constant("Color_Map_No").map_or(0.0, |v| v[0]);
    // Its picture (slot 0xb607c0fe; the noise map beside it is another
    // zone's and empty here).
    let image = mat
        .textures
        .iter()
        .filter_map(|t| Some((t.name_hash, c.images.get(t.image?.index)?)))
        .filter(|(_, img)| img.width > 0)
        .max_by_key(|(hash, img)| (*hash == 0xb607_c0fe, u32::from(img.width) * u32::from(img.height)))
        .map(|(_, img)| img)?;
    let img = super::t6_materials::to_rgba8(&super::t6_materials::decode(packs, image).ok()?)?;
    let mut rgba = img.data.clone()?;
    // A colour map is read as sRGB (the engine's own rule: `use_srgb_reads`
    // for every colour map), so the shader's picture values are the linear
    // ones.
    let linear = |byte: u8| {
        let v = f32::from(byte) / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    for px in rgba.chunks_exact_mut(4) {
        let r = linear(px[0]);
        let g = linear(px[1]).powf(falloff);
        // The shader's noise map (another zone's, not held here) shimmers
        // the glow; its mean, one half, stands in for it. The shader's
        // output is HDR (`Emissive_Amount` runs to 9), which the game
        // squeezes before it shows: 1 - e^(-c * EXPOSURE) (a fitted soft
        // knee, no data behind it). The material adds that onto the scene
        // (blend ONE / ONE or ONE / 1 - alpha with a zero alpha), so the
        // HUD draws it additively: the picture is the colour added.
        const NOISE_MEAN: f32 = 0.5;
        const EXPOSURE: f32 = 0.35;
        for i in 0..3 {
            let c = (r * core[i] * (1.0 - noise * NOISE_MEAN) + g * glow[i] * NOISE_MEAN) * emissive;
            px[i] = ((1.0 - (-c * EXPOSURE).exp()) * 255.0).round() as u8;
        }
        px[3] = 255;
    }
    let mut out = bevy::prelude::Image::new(
        img.texture_descriptor.size,
        bevy::render::render_resource::TextureDimension::D2,
        rgba,
        bevy::render::render_resource::TextureFormat::Rgba8Unorm,
        bevy::asset::RenderAssetUsages::default(),
    );
    out.sampler = img.sampler.clone();
    Some((scale, out))
}

fn real(name: &str) -> bool {
    !name.is_empty() && !name.starts_with(',')
}

/// Where each material is really defined: name -> (capture, index), the
/// last zone that defines it (not a comma-named reference) winning.
fn material_homes(captures: &[&ZoneCapture]) -> HashMap<String, (usize, usize)> {
    let mut homes = HashMap::new();
    for (ci, c) in captures.iter().enumerate() {
        for (mi, m) in c.materials.iter().enumerate() {
            if real(&m.name) {
                homes.insert(m.name.clone(), (ci, mi));
            }
        }
    }
    homes
}

/// For every capture, the material indices (into that capture) the wanted
/// models and effects draw with, each taken from the zone that defines it.
/// bo2zm M3: Pack-a-Punch's look. The scripts give an upgraded gun camo
/// 39 on this map (`get_pack_a_punch_weapon_options`; 40 on Mob of the
/// Dead, 45 on Origins); `mp/weaponoptions_zm.csv`'s row for it says it
/// swaps materials (a 1 in column 3) and which of the gun's camo material
/// sets (column 4, counted from 1: the M1911's camo1 -> the zombies camo).
/// Each upgraded gun's view model gets a copy with those swaps,
/// `<model>+camo<n>`.
#[derive(Default)]
struct PapCamo {
    /// Copy -> (the gun model, swaps base material -> camo material).
    models: BTreeMap<String, (String, Vec<(String, String)>)>,
    /// Upgraded weapon -> its gun model's copy.
    weapon_models: HashMap<String, String>,
}

fn pap_camo(
    captures: &[&ZoneCapture],
    weapon_refs: &HashMap<String, &asset_t6::WeaponRef>,
    camo_index: u32,
) -> PapCamo {
    let mut out = PapCamo::default();
    let want = camo_index.to_string();
    let set = captures
        .iter()
        .rev()
        .flat_map(|c| c.string_tables.iter())
        .find(|t| t.name == "mp/weaponoptions_zm.csv")
        .and_then(|t| {
            (0..t.rows).find_map(|r| {
                let cell = |k: usize| t.cells.get(r * t.columns + k).map_or("", String::as_str);
                (cell(0) == want && cell(1) == "camo" && cell(3) == "1")
                    .then(|| cell(4).parse::<usize>().ok())
                    .flatten()
            })
        })
        .filter(|set| *set > 0);
    let Some(set) = set else {
        return out;
    };
    let camos: HashMap<&str, &asset_t6::WeaponCamoRef> = captures
        .iter()
        .flat_map(|c| c.weapon_camos.iter())
        .map(|c| (c.name.as_str(), c))
        .collect();
    for (name, w) in weapon_refs {
        if !name.contains("_upgraded") {
            continue;
        }
        let Some(list) = camos
            .get(w.camo.as_str())
            .and_then(|camo| camo.material_sets.get(set - 1))
        else {
            continue;
        };
        let swaps: Vec<(String, String)> = list
            .iter()
            .flat_map(|m| m.base.iter().cloned().zip(m.camo.iter().cloned()))
            .filter(|(base, camo)| real(base) && real(camo))
            .collect();
        let Some(gun) = w.gun_models.first().filter(|n| real(n)) else {
            continue;
        };
        if swaps.is_empty() {
            continue;
        }
        let copy = format!("{gun}+camo{camo_index}");
        out.models
            .entry(copy.clone())
            .or_insert((gun.clone(), swaps));
        out.weapon_models.insert(name.clone(), copy);
    }
    out
}

fn wanted_materials(
    captures: &[&ZoneCapture],
    model_names: &BTreeSet<String>,
    homes: &HashMap<String, (usize, usize)>,
) -> Vec<BTreeSet<usize>> {
    let mut wanted: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); captures.len()];
    let mut want = |name: &str| {
        if let Some(&(ci, mi)) = homes.get(name.trim_start_matches(',')) {
            wanted[ci].insert(mi);
        }
    };
    // Models: from the last capture that defines each.
    for name in model_names {
        if let Some((ci, model)) = captures
            .iter()
            .enumerate()
            .rev()
            .find_map(|(ci, c)| c.xmodels.iter().find(|m| &m.name == name).map(|m| (ci, m)))
        {
            for srf in &model.lod0 {
                if let Some(k) = srf.material
                    && let Some(m) = captures[ci].materials.get(k.index)
                {
                    want(&m.name);
                }
            }
        }
    }
    // Effects and tracers: their material names.
    for c in captures {
        for e in c.fx.iter().filter(|e| real(&e.name)) {
            for el in &e.elems {
                for v in &el.visuals {
                    match v {
                        asset_t6::FxVisualRef::Material(n) => want(n),
                        asset_t6::FxVisualRef::Mark([a, b]) => {
                            want(a);
                            want(b);
                        }
                        _ => {}
                    }
                }
            }
        }
        for t in c.tracers.iter().filter(|t| real(&t.name)) {
            want(&t.material);
        }
    }
    wanted
}

/// The sound bank files a map's banks read: each bank asset's own files,
/// the map's location banks (`zmb_<location>*`), and every file those
/// name as a dependency.
pub(super) fn bank_bases(captures: &[&ZoneCapture], map: &str) -> Vec<String> {
    let mut bases: Vec<String> = Vec::new();
    let mut push = |b: String| {
        if !b.is_empty() && !bases.contains(&b) {
            bases.push(b);
        }
    };
    if let Some(location) = map.strip_prefix("zm_") {
        push(format!("zmb_{location}"));
        push(format!("zmb_{location}_real"));
        push(format!("zmb_{location}_real_intro"));
    }
    for c in captures {
        for b in &c.sound_banks {
            let base = b
                .name
                .rsplit_once('.')
                .map_or(b.name.as_str(), |(base, _)| base);
            push(base.to_owned());
        }
    }
    let deps: [&str; 3] = if is_mp_map(map) {
        ["mpl_common", "mpl_code_post_gfx", "cmn_root"]
    } else {
        ["zmb_common", "zmb_code_post_gfx", "cmn_root"]
    };
    for dep in deps {
        push(dep.to_owned());
    }
    bases
}

pub(crate) fn load_t6_combat(
    map_path: &Path,
    map_capture: &ZoneCapture,
    materials: &mut MaterialCatalog,
    packs: Option<&PackSet>,
) -> T6Combat {
    let mut report = Vec::new();
    let (pre, post) = zone_paths(map_path);
    let capture = |path: &PathBuf, report: &mut Vec<String>| match asset_t6::capture_zone(path) {
        Ok(c) => Some(c),
        Err(e) => {
            report.push(format!("t6 zone gap: {e}"));
            None
        }
    };
    let mut pre_caps: Vec<ZoneCapture> =
        pre.iter().filter_map(|p| capture(p, &mut report)).collect();
    let map_stem = map_path
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    // bo2mp: a multiplayer map's factions (bodies, arms, team voices).
    if is_mp_map(&map_stem) {
        let dir = map_path.parent().unwrap_or(Path::new("."));
        for path in mp_faction_zones(&pre_caps, &map_stem, dir, &mut report) {
            if let Some(c) = capture(&path, &mut report) {
                pre_caps.push(c);
            }
        }
    }
    let post_caps: Vec<ZoneCapture> = post
        .iter()
        .filter_map(|p| capture(p, &mut report))
        .collect();
    let captures: Vec<&ZoneCapture> = pre_caps
        .iter()
        .chain(std::iter::once(map_capture))
        .chain(post_caps.iter())
        .collect();
    report.push(format!(
        "t6 combat zones: {}",
        captures
            .iter()
            .map(|c| c.zone.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ));

    // bo2mp vehicle screens: the match zones' named visions
    // (`vision/<name>.vision`, later zones winning a name) for the scripts'
    // `visionsetnaked` and `setvisionsetforplayer` (the VTOL Warship's
    // `remote_mortar_enhanced`, _helicopter_gunner.gsc).
    if is_mp_map(&map_stem) {
        let visions: Vec<(String, asset_world::T6Vision)> = captures
            .iter()
            .flat_map(|c| c.raw_files.iter())
            .filter_map(|(name, bytes)| {
                let n = name.replace('\\', "/");
                let n = n.strip_prefix("vision/")?.strip_suffix(".vision")?.to_owned();
                Some((n, asset_world::parse_t6_vision(&String::from_utf8_lossy(bytes))?))
            })
            .collect();
        report.push(format!(
            "t6 match visions: {} ({})",
            visions.len(),
            visions.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ")
        ));
        asset_world::register_t6_visions(visions);
    }

    // Weapons: later zones replace earlier ones of the same name.
    let mut weapon_refs: HashMap<String, &asset_t6::WeaponRef> = HashMap::new();
    let mut weapon_order: Vec<String> = Vec::new();
    for c in &captures {
        for w in c.weapons.iter().filter(|w| real(&w.name)) {
            if weapon_refs.insert(w.name.clone(), w).is_none() {
                weapon_order.push(w.name.clone());
            }
        }
    }
    // bo2mp: each multiplayer gun's sight attachments (`au_<gun>_<sight>`
    // with its `<sight>` attachment), later zones winning a name.
    let mut attachments: HashMap<&str, &asset_t6::AttachmentRef> = HashMap::new();
    let mut uniques: HashMap<&str, &asset_t6::AttachmentUniqueRef> = HashMap::new();
    for c in &captures {
        for a in c.attachments.iter().filter(|a| real(&a.name)) {
            attachments.insert(a.name.as_str(), a);
        }
        for u in c.attachment_uniques.iter().filter(|u| real(&u.name)) {
            uniques.insert(u.name.as_str(), u);
        }
    }
    let mut sights: Vec<(String, &asset_t6::AttachmentRef, &asset_t6::AttachmentUniqueRef)> = Vec::new();
    for name in &weapon_order {
        let Some(base) = name.strip_suffix("_mp") else { continue };
        for sight in asset_game::T6_SIGHT_ATTACHMENTS {
            if let (Some(a), Some(u)) = (
                attachments.get(sight),
                uniques.get(format!("au_{base}_{sight}").as_str()),
            ) {
                sights.push((name.clone(), a, u));
            }
        }
    }
    let pap = pap_camo(&captures, &weapon_refs, 39);
    report.push(format!(
        "t6 Pack-a-Punch camo: {} gun models, {} upgraded weapons",
        pap.models.len(),
        pap.weapon_models.len()
    ));
    let mut model_names: BTreeSet<String> = BTreeSet::new();
    for w in weapon_refs.values() {
        if let Some(gun) = w.gun_models.first().filter(|n| real(n)) {
            model_names.insert(gun.clone());
        }
        let hands = w.def_model(fastfile_t6::layout::WeaponDef::handXModel);
        if real(hands) {
            model_names.insert(hands.to_owned());
        }
        // The magazine and (snipers) the scope, attached to the gun.
        for (_, model, ..) in &w.attach_view_models {
            if real(model) {
                model_names.insert(model.clone());
            }
        }
        // What a launcher shows loaded and fires: the RPG's rocket, the
        // ballistic knife's blade, the knife a gun's melee swings.
        for field in [
            fastfile_t6::layout::WeaponDef::rocketModel,
            fastfile_t6::layout::WeaponDef::projectileModel,
            fastfile_t6::layout::WeaponDef::additionalMeleeModel,
        ] {
            let model = w.def_model(field);
            if real(model) {
                model_names.insert(model.to_owned());
            }
        }
    }
    // bo2mp: the sight attachments' models.
    for (_, _, u) in &sights {
        for model in [&u.view_model, &u.view_model_additional, &u.view_model_ads] {
            if real(model) {
                model_names.insert(model.clone());
            }
        }
    }
    // A multiplayer map's arms come with its factions' class scripts
    // (`mpbody/class_*`), named below with the script models.
    if !is_mp_map(&map_stem) {
        model_names.insert(T6_PLAYER_ARMS.to_owned());
        for arms in T6_OTHER_ARMS {
            model_names.insert(arms.to_owned());
        }
    }
    // bo2zm M3: models the map's own scripts and entities name (perk
    // machines, the box, power-ups, zombie bodies and heads...), drawn as
    // script models and actors from the same catalog.
    let destructibles = destructible_models(&captures);
    let script_models = script_model_names(&captures, &destructibles);
    report.push(format!(
        "t6 script models: {} named by the map's scripts and entities",
        script_models.len()
    ));
    model_names.extend(script_models);

    // Third-person gun models too (a weapon cannot be given without one).
    let mut world_names: BTreeSet<String> = BTreeSet::new();
    for w in weapon_refs.values() {
        if let Some(world) = w.world_models.first().filter(|n| real(n)) {
            world_names.insert(world.clone());
        }
    }
    // Effect models: shell casings, debris.
    let mut fx_model_names: BTreeSet<String> = BTreeSet::new();
    for c in &captures {
        for e in c.fx.iter().filter(|e| real(&e.name)) {
            for el in &e.elems {
                for v in &el.visuals {
                    if let asset_t6::FxVisualRef::Model(n) = v
                        && real(n)
                    {
                        fx_model_names.insert(n.clone());
                    }
                }
            }
        }
    }
    let all_models: BTreeSet<String> = model_names
        .union(&world_names)
        .cloned()
        .chain(fx_model_names.iter().cloned())
        .collect();

    // Materials the guns, arms and effects draw with, linked per zone.
    let homes = material_homes(&captures);
    let mut wanted = wanted_materials(&captures, &all_models, &homes);
    for (_, swaps) in pap.models.values() {
        for (_, camo) in swaps {
            if let Some(&(ci, mi)) = homes.get(camo.trim_start_matches(',')) {
                wanted[ci].insert(mi);
            }
        }
    }
    // Images by name, from the zone that holds their pixels.
    let mut image_homes: HashMap<String, asset_t6::ImageRef> = HashMap::new();
    for c in &captures {
        for img in &c.images {
            if real(&img.name) {
                image_homes.insert(img.name.clone(), img.clone());
            }
        }
    }
    // bo2mp: a world colour map named `,<image>` is a picture an earlier
    // zone holds (Carrier's parked jets and drone); the world, linked before
    // those zones were read, drew it flat. Give it its pixels now.
    if let Some(packs) = packs {
        let (mut given, mut left) = (0usize, Vec::new());
        for img in materials.images.iter_mut() {
            let Some(name) = img
                .name
                .as_str()
                .strip_suffix("#flat")
                .and_then(|n| n.strip_prefix(','))
            else {
                continue;
            };
            match image_homes
                .get(name)
                .map(|real| (real, super::t6_materials::decode(packs, real)))
            {
                Some((real, Ok(gpu))) => {
                    img.decoded = Some(std::sync::Arc::new(gpu));
                    img.map_type = real.map_type;
                    img.width = real.width;
                    img.height = real.height;
                    img.depth = real.depth;
                    img.level_count = real.level_count;
                    given += 1;
                }
                _ => left.push(name.to_owned()),
            }
        }
        report.push(format!(
            "t6 world colour maps from earlier zones: {given} given pixels, {} not held anywhere {left:?}",
            left.len()
        ));
    }
    let mut local: Vec<HashMap<usize, usize>> = Vec::with_capacity(captures.len());
    for (c, want) in captures.iter().zip(&wanted) {
        let linked = super::t6_materials::link_materials_with(
            materials,
            c,
            packs,
            want.iter().copied(),
            &c.zone,
            Some(&image_homes),
        );
        report.extend(
            linked
                .report
                .into_iter()
                .map(|line| format!("{} ({}): {line}", "t6 combat", c.zone)),
        );
        local.push(linked.local);
    }

    // A model surface's material, in the catalog: the capture's own row
    // when it linked it, else the row of the zone that defines it.
    let catalog_material = |ci: usize, key: asset_t6::AssetKey| -> Option<usize> {
        if let Some(&i) = local[ci].get(&key.index) {
            return Some(i);
        }
        let name = captures[ci]
            .materials
            .get(key.index)?
            .name
            .trim_start_matches(',');
        let &(hc, hm) = homes.get(name)?;
        local[hc].get(&hm).copied()
    };
    let find_model = |name: &str| {
        captures
            .iter()
            .enumerate()
            .rev()
            .find_map(|(ci, c)| c.xmodels.iter().find(|m| m.name == name).map(|m| (ci, m)))
    };

    // First-person meshes: every gun and the players' arms.
    let mut fpv = FpvMeshBuild::default();
    fpv.set_capture_ns(asset_core::AssetNamespace::T6);
    let mut fpv_missing = Vec::new();
    for name in &model_names {
        let Some((ci, model)) = find_model(name) else {
            fpv_missing.push(name.clone());
            continue;
        };
        // bo2mp: a sight's reticle surface is drawn by the HUD at
        // infinity (its shader's projection), not as a picture on the glass.
        let resolve = |k: asset_t6::AssetKey| {
            if reticle_material(&captures, &homes, ci, k).is_some() {
                return None;
            }
            catalog_material(ci, k)
        };
        match asset_model::capture_model_skel_t6(model, resolve) {
            Some(skel) => fpv.insert_in(asset_core::AssetNamespace::T6, skel, Some(materials)),
            None => fpv_missing.push(format!("{name} (unreadable)")),
        }
    }
    // The upgraded guns' copies, their camo surfaces swapped.
    for (copy, (gun, swaps)) in &pap.models {
        let Some((ci, model)) = find_model(gun) else {
            continue;
        };
        let swapped = |k: asset_t6::AssetKey| -> Option<usize> {
            let name = captures[ci]
                .materials
                .get(k.index)?
                .name
                .trim_start_matches(',');
            match swaps
                .iter()
                .find(|(base, _)| base.trim_start_matches(',') == name)
            {
                Some((_, camo)) => {
                    let &(hc, hm) = homes.get(camo.trim_start_matches(','))?;
                    local[hc].get(&hm).copied()
                }
                None => catalog_material(ci, k),
            }
        };
        if let Some(mut skel) = asset_model::capture_model_skel_t6(model, swapped) {
            skel.name = copy.clone();
            fpv.insert_in(asset_core::AssetNamespace::T6, skel, Some(materials));
        }
    }
    // Third-person gun models.
    let mut world_weapons = WorldWeaponBuild::default();
    world_weapons.set_capture_ns(asset_core::AssetNamespace::T6);
    let mut world_missing = Vec::new();
    for name in &world_names {
        let Some((ci, model)) = find_model(name) else {
            world_missing.push(name.clone());
            continue;
        };
        match asset_model::capture_model_skel_t6(model, |k| catalog_material(ci, k)) {
            Some(skel) => {
                world_weapons.insert_in(asset_core::AssetNamespace::T6, skel, Some(materials))
            }
            None => world_missing.push(format!("{name} (unreadable)")),
        }
    }
    report.push(format!(
        "t6 world gun models: {} built, {} missing {:?}",
        world_weapons.len(),
        world_missing.len(),
        world_missing.iter().take(8).collect::<Vec<_>>()
    ));
    report.push(format!(
        "t6 first-person meshes: {} built, {} missing {:?}",
        fpv.len(),
        fpv_missing.len(),
        fpv_missing.iter().take(8).collect::<Vec<_>>()
    ));

    // Animations: every one, later zones win.
    let mut xanims = XAnimBuild::default();
    xanims.set_capture_ns(asset_core::AssetNamespace::T6);
    let mut anim_n = 0usize;
    for c in &captures {
        for a in &c.xanims {
            anim_n += usize::from(xanims.insert_t6(a));
        }
    }
    report.push(format!(
        "t6 animations: {anim_n} captured, {} kept, {} gaps",
        xanims.len(),
        xanims.capture_gaps
    ));

    // The weapon catalog.
    let mut catalog = WeaponCatalog::default();
    catalog.set_capture_ns(asset_core::AssetNamespace::T6);
    for name in &weapon_order {
        let w = weapon_refs[name];
        // A dual-wield weapon's left-hand weapon (its gun and dw_left_*
        // animations join the right one's viewmodel).
        let left = weapon_refs
            .get(w.def_string(fastfile_t6::layout::WeaponDef::szDualWieldWeaponName))
            .copied();
        catalog.capture_t6(w, left, &pap.weapon_models);
    }
    // bo2mp: every gun with each of its sight attachments.
    let mut sight_n = 0usize;
    let mut reticle_pictures: HashMap<(String, bool), (f32, std::sync::Arc<bevy::prelude::Image>)> =
        HashMap::new();
    let mut sight_reticles: Vec<(String, std::sync::Arc<bevy::prelude::Image>)> = Vec::new();
    for (name, a, u) in &sights {
        let optic = if real(&u.view_model) { u.view_model.as_str() } else { "" };
        let hybrid = a.name == "dualoptic" && !u.alt_weapon_name.is_empty();
        // The picture of one of the sight model's reticles (`top`: the
        // little dot above a Hybrid's scope, which its first mode aims
        // through; the other mode, and every other sight, the first below).
        let mut picture = |top: bool| match (packs, optic) {
            (_, "") | (None, _) => None,
            (Some(packs), optic) => match reticle_pictures.get(&(optic.to_owned(), top)) {
                Some(found) => Some(found.clone()),
                None => sight_reticle(&captures, &homes, packs, optic, top).map(|(scale, img)| {
                    let found = (scale, std::sync::Arc::new(img));
                    reticle_pictures.insert((optic.to_owned(), top), found.clone());
                    found
                }),
            },
        };
        let reticle = picture(hybrid);
        let anim_ms = |name: &str| {
            captures.iter().flat_map(|c| c.xanims.iter()).find(|a| a.name.eq_ignore_ascii_case(name)).and_then(|a| {
                (a.framerate > 0.0).then(|| (f64::from(a.numframes) / f64::from(a.framerate) * 1000.0).round() as i32)
            })
        };
        let scale = reticle.as_ref().map_or(0.0, |(scale, _)| *scale);
        if catalog.capture_t6_attachment(weapon_refs[name], a, u, scale, None, &anim_ms) {
            // Hybrid Optic: its other mode is a weapon of its own, with the
            // other reticle.
            if hybrid && let Some(alt) = weapon_refs.get(u.alt_weapon_name.as_str()) {
                let other = picture(false);
                let other_scale = other.as_ref().map_or(0.0, |(scale, _)| *scale);
                if catalog.capture_t6_attachment(alt, a, u, other_scale, Some(&format!("{name}+{}", a.name)), &anim_ms) {
                    if let Some((_, img)) = other {
                        sight_reticles.push((format!("reticle:{}+{}", u.alt_weapon_name, a.name), img));
                    }
                }
            }
            sight_n += 1;
            if let Some((_, img)) = reticle {
                sight_reticles.push((format!("reticle:{name}+{}", a.name), img));
            }
        }
    }
    report.push(format!(
        "t6 sight reticles: {} sight models with a reticle ({:?})",
        reticle_pictures.len(),
        reticle_pictures.iter().map(|((m, top), (s, _))| format!("{m}{} {s}", if *top { " (top)" } else { "" })).collect::<Vec<_>>()
    ));
    report.push(format!(
        "t6 sight attachments: {sight_n} guns with a sight ({} attachments, {} uniques read)",
        attachments.len(),
        uniques.len()
    ));
    // bo2zm fix list 2: a rolling grenade rests and rolls on its body: the
    // nearest face of its projectile model's bounds to the model's origin
    // (the frag: a ball of about 1.4 units around it, the fuse above).
    let mut rolling = Vec::new();
    for name in &weapon_order {
        let w = weapon_refs[name];
        if w.def_i32(fastfile_t6::layout::WeaponDef::isRollingGrenade) == 0 {
            continue;
        }
        let model = w.def_model(fastfile_t6::layout::WeaponDef::projectileModel);
        if let Some((_, m)) = find_model(model) {
            let radius = (0..3)
                .flat_map(|a| [m.mins[a].abs(), m.maxs[a].abs()])
                .fold(f32::MAX, f32::min);
            if radius.is_finite() && radius > 0.1 {
                catalog.set_t6_rolling_radius(name, radius);
                rolling.push(format!("{name} {radius:.2}"));
            }
        }
    }
    report.push(format!("t6 rolling grenades: {}", rolling.join(", ")));
    let captured = catalog.len();
    let mut weapons = catalog.into_build();
    weapons.stamp_namespace(asset_core::AssetNamespace::T6);
    report.push(format!(
        "t6 weapons: {captured} captured, {} in the catalog",
        weapons.len()
    ));

    // Effects: every one, later zones win.
    let mut fx = FxCatalog::default();
    fx.set_capture_ns(asset_core::AssetNamespace::T6);
    let mut fx_n = 0usize;
    for c in &captures {
        for e in &c.fx {
            fx_n += usize::from(fx.capture_t6(e));
        }
    }
    report.push(format!("t6 effects: {fx_n} captured, {} kept", fx.len()));
    // bo2zm fix list 3: a gun whose first-person flash is the flash other
    // players see (it names the same effect for both) gets a first-person
    // copy at `T6_FIRST_PERSON_FLASH_SCALE` (our choice; see there). Bullet
    // guns only: the launchers' flashes are launch smoke or already small.
    // A flash made for the first person keeps its own size even without
    // parts drawn with the gun (the Ray Gun's).
    let mut first_person = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for name in &weapon_order {
        let w = weapon_refs[name];
        let view = w.def_fx(fastfile_t6::layout::WeaponDef::viewFlashEffect);
        let world = w.def_fx(fastfile_t6::layout::WeaponDef::worldFlashEffect);
        let bullet = w.def_i32(fastfile_t6::layout::WeaponDef::weapType) == 0;
        if !bullet || view.is_empty() || view != world || !seen.insert(view) {
            continue;
        }
        if let Some((before, after)) = fx.add_t6_first_person_flash(view) {
            first_person.push(format!("{view} {before:.1}->{after:.1}"));
        }
    }
    report.push(format!(
        "t6 first-person flashes made from far-view ones (x{}): {}",
        asset_game::T6_FIRST_PERSON_FLASH_SCALE,
        first_person.join(", ")
    ));
    let mut fx_models = asset_game::FxModelCatalog::default();
    fx_models.set_capture_ns(asset_core::AssetNamespace::T6);
    let mut fx_model_missing = Vec::new();
    for name in &fx_model_names {
        match find_model(name)
            .and_then(|(ci, m)| asset_model::capture_model_skel_t6(m, |k| catalog_material(ci, k)))
        {
            Some(skel) => fx_models.insert_skel(skel, materials),
            None => fx_model_missing.push(name.clone()),
        }
    }
    report.push(format!(
        "t6 effect models: {} built, {} missing {:?}",
        fx_models.len(),
        fx_model_missing.len(),
        &fx_model_missing[..fx_model_missing.len().min(8)]
    ));
    // Tracers: every one, later zones win.
    let mut tracers = asset_game::TracerCatalog::default();
    tracers.set_capture_ns(asset_core::AssetNamespace::T6);
    let mut tracer_n = 0usize;
    for c in &captures {
        for t in &c.tracers {
            tracer_n += usize::from(tracers.capture_t6(t));
        }
    }
    report.push(format!(
        "t6 tracers: {tracer_n} captured, {} kept",
        tracers.len()
    ));
    let impact_fx = captures
        .iter()
        .rev()
        .find_map(|c| c.impact_tables.first())
        .map(OwnedFxImpactTable::from_t6);
    report.push(format!(
        "t6 impact table: {}",
        impact_fx.as_ref().map_or_else(
            || "none".to_owned(),
            |t| format!("{} ({} rows)", t.name, t.row_count())
        )
    ));

    // Sound: every bank's aliases, the clips in the bank files beside the
    // zones (`<install>/sound`).
    let map = map_path
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let sound_dir = map_path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .map(|install| install.join("sound"));
    let (map_fx, room_tone) = map_scripts(&captures, &map, &mut report);
    // The language zones beside zone/all (zone/english/en_*.ff): the
    // English strings (hints, perk and power-up names) and the voices'
    // aliases (the announcer's, the players').
    let lang_dir = map_path
        .parent()
        .and_then(Path::parent)
        .map(|d| d.join("english"));
    let lang_zones: Vec<String> = if is_mp_map(&map) {
        // bo2mp: the shared zones', then the factions' (team voices), then
        // the map's.
        T6_MP_LANG_ZONES
            .iter()
            .map(|z| (*z).to_owned())
            .chain(
                captures
                    .iter()
                    .filter(|c| c.zone.starts_with("faction_"))
                    .map(|c| c.zone.clone()),
            )
            .chain(std::iter::once(map.clone()))
            .collect()
    } else {
        [
            "code_post_gfx_zm",
            "common_zm",
            "patch_zm",
            "ui_zm",
            "patch_ui_zm",
            map.as_str(),
        ]
        .iter()
        .map(|z| (*z).to_owned())
        .collect()
    };
    let lang_caps: Vec<ZoneCapture> = lang_zones
        .iter()
        .filter_map(|z| {
            let path = lang_dir.as_ref()?.join(format!("en_{z}.ff"));
            if !path.is_file() {
                return None;
            }
            match asset_t6::capture_zone(&path) {
                Ok(c) => Some(c),
                Err(e) => {
                    report.push(format!("t6 language zone gap: {e}"));
                    None
                }
            }
        })
        .collect();
    // bo2mp: each alias's length in ms (its longest variant's clip), for
    // the scripts' soundgetplaybacktime (the announcer waits one line out).
    let mut sound_lengths: Vec<(String, u32)> = Vec::new();
    let sound = match sound_dir {
        Some(dir) if dir.is_dir() => {
            let bases = bank_bases(&captures, &map);
            let base_refs: Vec<&str> = bases.iter().map(String::as_str).collect();
            let (index, bank_report) = asset_audio::T6BankIndex::open(&dir, &base_refs);
            report.extend(bank_report);
            let banks: Vec<&asset_t6::SndBankRef> = captures
                .iter()
                .copied()
                .chain(lang_caps.iter())
                .flat_map(|c| c.sound_banks.iter())
                .collect();
            let globals = captures.iter().find_map(|c| c.snd_globals.as_ref());
            let (mut catalog, census) = asset_audio::build_t6_sound_catalog(
                &banks,
                globals,
                &index,
                asset_core::ZoneOwner::intern(&map),
            );
            report.push(format!(
                "t6 sound: {} aliases, {} variants ({} with a clip, {} without); {} falloff curves",
                census.aliases,
                census.variants,
                census.with_clip,
                census.without_clip,
                globals.map_or(0, |g| g.curves.len())
            ));
            for s in &catalog.sounds {
                let ms = s
                    .aliases
                    .iter()
                    .filter_map(|a| a.streamed.as_ref())
                    .filter_map(|(_, id)| u32::from_str_radix(id, 16).ok())
                    .filter_map(|id| index.get(id))
                    .map(|(_, e)| (u64::from(e.frame_count) * 1000 / u64::from(e.rate().max(1))) as u32)
                    .max();
                if let Some(ms) = ms {
                    sound_lengths.push((s.name.to_ascii_lowercase(), ms));
                }
            }
            catalog.scripted_map_fx = Some(map_fx);
            Ok(catalog)
        }
        _ => Err("Black Ops II sound folder not found beside the zones".to_owned()),
    };
    let models_missing: Vec<String> = fpv_missing
        .iter()
        .chain(world_missing.iter())
        .cloned()
        .collect();
    // The guns checked: Nuketown Zombies' list, or (bo2mp) every
    // multiplayer gun the zones carry.
    let census_names: Vec<String> = if is_mp_map(&map) {
        let mut v: Vec<String> = weapon_order
            .iter()
            .filter(|n| n.ends_with("_mp"))
            .cloned()
            .collect();
        v.sort();
        v
    } else {
        NUKETOWN_WEAPONS.iter().map(|n| (*n).to_owned()).collect()
    };
    let census_lines = census(
        &captures,
        &census_names,
        &weapon_refs,
        &models_missing,
        &xanims,
        &fx,
        impact_fx.as_ref(),
        sound
            .as_ref()
            .ok()
            .and_then(|c| c.scripted_map_fx.as_ref())
            .unwrap_or(&asset_audio::ScriptedMapFx::default()),
        room_tone.as_deref(),
        sound.as_ref().ok(),
    );
    for line in &census_lines {
        diag::info!(World, "{line}");
    }
    report.extend(census_lines);
    let footsteps = captures
        .iter()
        .rev()
        .find(|c| !c.footstep_tables.is_empty())
        .map(|c| c.footstep_tables.clone())
        .unwrap_or_default();
    let mut scripts = crate::T6ScriptSet::default();
    let (mut playeranim, mut playeranim_types) = (None, None);
    for c in &captures {
        for (name, bytes) in &c.raw_files {
            if let Some(n) = name.strip_prefix("script:")
                && !n.starts_with(',')
                && n.ends_with(".gsc")
            {
                scripts.objects.push(bytes.clone());
            }
        }
        for t in &c.string_tables {
            if real(&t.name) {
                scripts
                    .tables
                    .push((t.name.clone(), t.columns, t.rows, t.cells.clone()));
            }
        }
        for v in &c.vehicles {
            scripts.vehicles.retain(|n| n.name != v.name);
            scripts.vehicles.push(v.clone());
        }
        scripts.entities.extend(
            c.map_ents
                .iter()
                .map(|t| destructible_entities(t, &destructibles)),
        );
        // bo2mp: every destructible the map's entities can name.
        for d in destructible_defs(&[*c]) {
            scripts.destructibles.retain(|x| x.name != d.name);
            scripts.destructibles.push(d);
        }
        if !c.path_nodes.is_empty() {
            scripts.path_nodes = c.path_nodes.clone();
        }
        for (name, bytes) in &c.raw_files {
            if name.starts_with("animstatedefs/") && name.ends_with(".asd") {
                let text = String::from_utf8_lossy(bytes).into_owned();
                scripts.animstatedefs.retain(|(n, _)| n != name);
                scripts.animstatedefs.push((name.clone(), text));
            }
            // bo2mp: the shellshocks (`shock/flashbang.shock`, ...).
            let lower = name.replace('\\', "/").to_ascii_lowercase();
            if let Some(shock) = lower
                .strip_prefix("shock/")
                .and_then(|n| n.strip_suffix(".shock"))
            {
                let text = String::from_utf8_lossy(bytes).into_owned();
                scripts.shocks.retain(|(n, _)| n != shock);
                scripts.shocks.push((shock.to_owned(), text));
            }
            if name.contains("/gamesettings_") && name.ends_with(".cfg") {
                let text = String::from_utf8_lossy(bytes).into_owned();
                scripts.gamesettings.retain(|(n, _)| n != name);
                scripts.gamesettings.push((name.clone(), text));
            }
            // bo2mp: how a dead body goes limp.
            if lower == "ragdoll.cfg" {
                scripts.ragdoll = Some(String::from_utf8_lossy(bytes).into_owned());
            }
            // bo2mp: how players look to others (third-person animations).
            if name == "mp/playeranim.script" {
                playeranim = Some(String::from_utf8_lossy(bytes).into_owned());
            }
            if name == "mp/playeranimtypes.txt" {
                playeranim_types = Some(String::from_utf8_lossy(bytes).into_owned());
            }
        }
    }
    scripts.playeranim = playeranim.zip(playeranim_types);
    // Every animation's timing, root motion and notetracks; later zones win.
    let mut anim_facts: HashMap<String, crate::T6AnimFacts> = HashMap::new();
    for c in &captures {
        for a in c.xanims.iter().filter(|a| real(&a.name)) {
            anim_facts.insert(
                a.name.clone(),
                crate::T6AnimFacts {
                    name: a.name.clone(),
                    numframes: a.numframes,
                    framerate: a.framerate,
                    looping: a.looping,
                    delta_trans: a.delta_trans.clone(),
                    notifies: a.notifies.clone(),
                },
            );
        }
    }
    scripts.anims = anim_facts.into_values().collect();
    let mut strings: HashMap<String, String> = HashMap::new();
    for c in &lang_caps {
        for (k, v) in &c.localize {
            strings.insert(k.clone(), v.clone());
        }
    }
    // bo2mp: the map's name as players know it (`mp/mapstable.csv` column
    // 3: `MPUI_LA` = "AFTERMATH").
    if is_mp_map(&map) {
        let key = captures
            .iter()
            .rev()
            .flat_map(|c| c.string_tables.iter())
            .find(|t| t.name.eq_ignore_ascii_case("mp/mapstable.csv"))
            .and_then(|t| {
                t.cells
                    .chunks(t.columns.max(1))
                    .find(|r| r.first().is_some_and(|c| c.eq_ignore_ascii_case(&map)))
                    .and_then(|r| r.get(3).cloned())
            });
        if let Some(key) = key {
            let name = strings
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(&key))
                .map_or("-", |(_, v)| v.as_str());
            report.push(format!("t6 mp map name: {key} = {name}"));
        }
    }
    scripts.strings = strings.into_iter().collect();
    scripts.sound_lengths = sound_lengths;
    // bo2mp: weapons a player can pick back up once placed or thrown (their
    // weapon file's bRetrievable: the hatchet, claymore, Bouncing Betty...),
    // as the engine's getretrievableweapons lists them.
    let mut retrievable: Vec<String> = weapon_refs
        .iter()
        .filter(|(_, w)| w.def_u8(fastfile_t6::layout::WeaponDef::bRetrievable) != 0)
        .map(|(n, _)| n.to_ascii_lowercase())
        .collect();
    retrievable.sort();
    report.push(format!("t6 retrievable weapons: {}", retrievable.join(", ")));
    // bo2mp: each weapon's own melee damage (iMeleeDamage), to check the
    // knife against BO2 (a melee hit kills in one).
    let mut melee: Vec<String> = weapon_refs
        .iter()
        .filter(|(n, _)| ["knife_mp", "knife_held_mp", "mp7_mp", "an94_mp", "dsr50_mp", "fiveseven_mp", "riotshield_mp", "knife_ballistic_mp"].contains(&n.as_str()))
        .map(|(n, w)| format!("{n}={}", w.def_i32(fastfile_t6::layout::WeaponDef::iMeleeDamage)))
        .collect();
    melee.sort();
    report.push(format!("t6 melee damage: {}", melee.join(" ")));
    scripts.retrievable_weapons = retrievable;
    // Every sound alias the banks hold (the scripts' soundexists).
    if let Ok(cat) = &sound {
        scripts.sound_aliases = cat
            .sounds
            .iter()
            .map(|s| s.name.to_ascii_lowercase())
            .collect();
    }
    report.push(format!(
        "t6 scripts: {} server script objects, {} string tables, {} entity strings, {} path nodes, {} animations, {} animstatedefs, {} English strings",
        scripts.objects.len(),
        scripts.tables.len(),
        scripts.entities.len(),
        scripts.path_nodes.len(),
        scripts.anims.len(),
        scripts.animstatedefs.len(),
        scripts.strings.len()
    ));
    let ui_script_list = ui_scripts(&captures);
    let mut ui_names = ui_strings(&ui_script_list);
    // bo2mp lane C: a multiplayer map's menus (the class menu) also draw
    // pictures named by table cells (weapons, perks).
    if map.starts_with("mp_") {
        ui_names.extend(super::t6_frontend::table_picture_names(&captures));
        // The minimap the engine draws: the map's picture, the pings
        // (him, his team), its overlay; the scripts' hit marker, the
        // outcome screen's team icons; his damage feedback (blood, the
        // low-health edge, the hit's direction).
        for c in &captures {
            for m in &c.materials {
                let n = m.name.as_str();
                if n.starts_with("compass_map_")
                    || n.starts_with("compassping_")
                    || n == "mp_minimap_overlay"
                    || n.starts_with("damage_feedback")
                    || n.starts_with("faction_")
                    || n.starts_with("overlay_low_health")
                    || n == "hit_direction"
                    || (n.starts_with("map_") && n.contains("selector"))
                    // The scorestreak earned popup's big icon (the item's
                    // picture and "_big", notificationpopups.lua) and the
                    // column's small ones (rewardselection.lua).
                    || n.starts_with("hud_ks_")
                    // The equipment pips beside his ammo (offhandicons.lua).
                    || matches!(
                        n,
                        "hud_us_grenade"
                            | "hud_grenadeicon"
                            | "hud_icon_sticky_grenade"
                            | "hud_hatchet"
                            | "hud_icon_claymore"
                            | "hud_icon_satchelcharge"
                            | "hud_bounce_betty"
                            | "hud_us_flashgrenade"
                            | "hud_us_stungrenade"
                            | "hud_willy_pete"
                            | "hud_empgrenade"
                            | "hud_trophy_system"
                            | "hud_tact_insert"
                    )
                {
                    ui_names.insert(n.to_owned());
                }
            }
        }
    }
    let mut hud_icons = hud_icons(&captures, packs, &ui_names, &mut report);
    hud_icons.0.extend(sight_reticles);
    report.push(format!("t6 ui: {} scripts", ui_script_list.len()));
    let ui = crate::T6Ui {
        scripts: ui_script_list,
        strings: std::sync::Arc::new(scripts.strings.clone()),
        tables: std::sync::Arc::new(scripts.tables.clone()),
        configs: std::sync::Arc::new(ui_configs(&captures)),
    };
    // The font sheet's pixels are in the language's own image packs.
    let lang_packs = map_path
        .parent()
        .and_then(Path::parent)
        .map(|d| d.join("english"))
        .and_then(|d| asset_t6::PackSet::open_dir(&d).ok());
    let hud_fonts = hud_fonts(&lang_caps, lang_packs.as_ref().or(packs), &mut report);
    T6Combat {
        weapons,
        fpv,
        world_weapons,
        xanims,
        fx,
        fx_models,
        tracers,
        impact_fx,
        sound,
        footsteps,
        room_tone,
        scripts,
        hud_icons,
        hud_fonts,
        ui,
        report,
    }
}
