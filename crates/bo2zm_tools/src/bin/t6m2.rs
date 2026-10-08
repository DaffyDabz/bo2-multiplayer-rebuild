//! bo2zm: check what the M2 capture reads from Nuketown's zones: weapons,
//! their models and animations, effects, sound banks and the bank files on
//! disk. Read-only; prints a report.
//!
//! `t6m2 [BO2 folder] [--weapons] [--fx] [--sound] [--decode]`

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const DEFAULT_BO2: &str = r"C:\Program Files (x86)\Steam\steamapps\common\Call of Duty Black Ops II";

const NUKETOWN_ZONES: [&str; 9] = [
    "code_pre_gfx_zm",
    "code_post_gfx_zm",
    "common_zm",
    "patch_zm",
    "ui_zm",
    "patch_ui_zm",
    "dlczm0_load_zm",
    "zm_nuked",
    "zm_nuked_patch",
];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_else(|| DEFAULT_BO2.to_owned());
    let want = |flag: &str| args.iter().any(|a| a == flag) || !args.iter().any(|a| a.starts_with("--"));
    let root = PathBuf::from(root);
    let mut captures = Vec::new();
    // T6M2_ZONES=a,b: other zones (e.g. the campaign's `common`).
    let zones: Vec<String> = std::env::var("T6M2_ZONES")
        .map(|z| z.split(',').map(str::to_owned).collect())
        .unwrap_or_else(|_| NUKETOWN_ZONES.iter().map(|z| (*z).to_owned()).collect());
    for zone in &zones {
        let path = root.join("zone").join("all").join(format!("{zone}.ff"));
        match asset_t6::capture_zone(&path) {
            Ok(c) => captures.push(c),
            Err(e) => {
                eprintln!("{zone}: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    for c in &captures {
        println!(
            "zone {}: weapons={} fx={} tracers={} impact_tables={} footstep_tables={} footstep_fx={} banks={} xmodels={} xanims={} strings={} unresolved={}",
            c.zone,
            c.weapons.len(),
            c.fx.len(),
            c.tracers.len(),
            c.impact_tables.len(),
            c.footstep_tables.len(),
            c.footstep_fx_tables.len(),
            c.sound_banks.len(),
            c.xmodels.len(),
            c.xanims.len(),
            c.strings.len(),
            c.unresolved_refs
        );
    }
    // Models and anims by name across zones (later zones win).
    let mut models: HashMap<String, (&str, &asset_t6::XModelRef)> = HashMap::new();
    let mut anims: HashMap<String, &asset_t6::XAnimRef> = HashMap::new();
    let mut fx: HashMap<String, &asset_t6::FxEffectRef> = HashMap::new();
    for c in &captures {
        for m in &c.xmodels {
            if !m.name.starts_with(',') {
                models.insert(m.name.clone(), (c.zone.as_str(), m));
            }
        }
        for a in &c.xanims {
            if !a.name.starts_with(',') {
                anims.insert(a.name.clone(), a);
            }
        }
        for e in &c.fx {
            if !e.name.starts_with(',') {
                fx.insert(e.name.clone(), e);
            }
        }
    }
    // T6M2_NOTES=a,b: the notetracks of every animation whose name starts
    // with one of the prefixes.
    if let Ok(prefixes) = std::env::var("T6M2_NOTES") {
        let prefixes: Vec<&str> = prefixes.split(',').collect();
        let mut names: Vec<&String> = anims.keys().filter(|n| prefixes.iter().any(|p| n.starts_with(p))).collect();
        names.sort();
        for n in names {
            println!("notes {n}: {:?}", anims[n].notifies);
        }
    }
    if want("--weapons") {
        weapons_report(&captures, &models, &anims, &fx);
    }
    if want("--fx") {
        fx_report(&captures, &fx);
    }
    if want("--sound") {
        sound_report(&root, &captures, want("--decode"));
    }
    ExitCode::SUCCESS
}

fn weapons_report(
    captures: &[asset_t6::ZoneCapture],
    models: &HashMap<String, (&str, &asset_t6::XModelRef)>,
    anims: &HashMap<String, &asset_t6::XAnimRef>,
    fx: &HashMap<String, &asset_t6::FxEffectRef>,
) {
    use fastfile_t6::layout::WeaponDef as d;
    let mut weapons: BTreeMap<String, &asset_t6::WeaponRef> = BTreeMap::new();
    for c in captures {
        for w in &c.weapons {
            if !w.name.starts_with(',') {
                weapons.insert(w.name.clone(), w);
            }
        }
    }
    println!("\n== weapons: {}", weapons.len());
    let mut missing_models = 0;
    let mut missing_anims = 0;
    let mut missing_fx = 0;
    for (name, w) in &weapons {
        let gun = w.gun_models.first().cloned().unwrap_or_default();
        let hand = w.def_model(d::handXModel).to_owned();
        // T6M2_SOUNDS=a,b: the named weapons' sound fields.
        if std::env::var("T6M2_SOUNDS").is_ok_and(|v| v.split(',').any(|x| x == name)) {
            for (field, off) in [
                ("fireSound", d::fireSound), ("fireSoundPlayer", d::fireSoundPlayer), ("fireLastSound", d::fireLastSound),
                ("fireKillcamSound", d::fireKillcamSound), ("crackSound", d::crackSound), ("whizbySound", d::whizbySound),
                ("reloadSound", d::reloadSound), ("reloadEmptySound", d::reloadEmptySound), ("reloadStartSound", d::reloadStartSound),
                ("reloadEndSound", d::reloadEndSound), ("rechamberSound", d::rechamberSound), ("raiseSound", d::raiseSound),
                ("emptyFireSound", d::emptyFireSound), ("pickupSound", d::pickupSound), ("meleeHitSound", d::meleeHitSound),
            ] {
                println!("sound {name}.{field} = '{}'", w.def_string(off));
            }
        }
        // T6M2_TRACERS=1: each weapon's tracer.
        if std::env::var_os("T6M2_TRACERS").is_some() && name.ends_with("_zm") {
            println!(
                "tracer {name}: {:?} view flash {:?} world flash {:?} reticle {:?}/{:?} sizes {}/{}",
                w.def_tracer(d::tracerType),
                w.def_fx(d::viewFlashEffect),
                w.def_fx(d::worldFlashEffect),
                w.def_material(d::reticleCenter),
                w.def_material(d::reticleSide),
                w.def_i32(d::iReticleCenterSize),
                w.def_i32(d::iReticleSideSize)
            );
        }
        if std::env::var("T6M2_WEAPON").is_ok_and(|want| want == *name) {
            println!("weapon {name} models {:?} tracer {:?} hide tags {:?} attach view models {:?} camo {:?} camo ptr {:#x}", w.models, w.def_tracer(d::tracerType), w.hide_tags, w.attach_view_models, w.camo, w.def_i32(d::weaponCamo));
            println!("   gun models {:?} world models {:?} anims idle {:?} strings {:?}", w.gun_models, w.world_models, w.xanims.get(1), w.strings);
            for (i, a) in w.xanims.iter().enumerate().filter(|(_, a)| !a.is_empty()) {
                println!("   anim {i:#04x} {a}");
            }
            println!(
                "   fuseTime {} aiFuseTime {} cookOffHold {} timedDetonation {} explosionRadius {} inner {} projSpeed {} activateDist {} projExplosion {} stickiness {} holdToThrow {} fx {:?}",
                w.def_i32(d::fuseTime),
                w.def_i32(d::aiFuseTime),
                w.def_u8(d::bCookOffHold),
                w.def_u8(d::timedDetonation),
                w.def_i32(d::iExplosionRadius),
                w.def_i32(d::iExplosionInnerDamage),
                w.def_i32(d::iProjectileSpeed),
                w.def_i32(d::iProjectileActivateDist),
                w.def_i32(d::projExplosion),
                w.def_i32(d::stickiness),
                w.def_u8(d::holdButtonToThrow),
                w.def_fx(d::projExplosionEffect),
            );
            let proj = w.def_model(d::projectileModel);
            println!(
                "   rolling {} rotateType {} rotate {} keepRolling {} forceBounce {} noCrumple {} projModel '{proj}' bounds {:?} clipModel '{}' parallel {:?} perpendicular {:?}",
                w.def_i32(d::isRollingGrenade),
                w.def_i32(d::rotateType),
                w.def_u8(d::rotate),
                w.def_u8(d::bKeepRolling),
                w.def_u8(d::bForceBounce),
                w.def_u8(d::bNoCrumpleMissile),
                models.get(proj).map(|(_, m)| (m.mins, m.maxs, m.radius, m.contents)),
                w.def_model(d::worldClipModel),
                w.parallel_bounce,
                w.perpendicular_bounce,
            );
            for (slot, a) in w.xanims.iter().enumerate() {
                if !a.is_empty() {
                    let notes = anims.get(a.as_str()).or_else(|| anims.get(a.to_ascii_lowercase().as_str())).map(|x| x.notifies.clone()).unwrap_or_default();
                    println!("   anim slot {slot:#04x}: {a} notes {notes:?}");
                }
            }
            println!("   notetrack sound map {:?}", w.notetrack_sounds);
            println!("   strings {:?}", w.strings.iter().filter(|(_, s)| !s.is_empty()).collect::<Vec<_>>());
        }
        // T6M2_AIM=1: one line per weapon of what aims, throws and sounds it.
        if std::env::var_os("T6M2_AIM").is_some() {
            let f = |o: usize| w.def_f32(o);
            println!(
                "aim {name}: impactType {} hip min {}/{}/{} max {}/{}/{} decay {} fire_add {} turn_add {} move_add {} | ads {} | hip view kick p[{}, {}] y[{}, {}] min {} | ads view kick p[{}, {}] y[{}, {}] min {} | hip gun kick p[{}, {}] | ads gun kick p[{}, {}] | ads scatter [{}, {}] | hip scatter [{}, {}] playerSpread {} aiSpread {} stackFireSpread {} reticleSidePos {} fireTime {} | flash ofs view {:?} world {:?} altTag {} | proj speed {} up {} rel up {} fwd {} parent {} | melee snd swipe '{}' player '{}' hit '{}' miss '{}' | tracer {:?}",
                w.def_i32(d::impactType),
                f(d::fHipSpreadStandMin), f(d::fHipSpreadDuckedMin), f(d::fHipSpreadProneMin),
                f(d::hipSpreadStandMax), f(d::hipSpreadDuckedMax), f(d::hipSpreadProneMax),
                f(d::fHipSpreadDecayRate), f(d::fHipSpreadFireAdd), f(d::fHipSpreadTurnAdd), f(d::fHipSpreadMoveAdd),
                f(d::fAdsSpread),
                f(d::fHipViewKickPitchMin), f(d::fHipViewKickPitchMax), f(d::fHipViewKickYawMin), f(d::fHipViewKickYawMax), f(d::fHipViewKickMinMagnitude),
                f(d::fAdsViewKickPitchMin), f(d::fAdsViewKickPitchMax), f(d::fAdsViewKickYawMin), f(d::fAdsViewKickYawMax), f(d::fAdsViewKickMinMagnitude),
                f(d::fHipGunKickPitchMin), f(d::fHipGunKickPitchMax),
                f(d::fAdsGunKickPitchMin), f(d::fAdsGunKickPitchMax),
                f(d::fAdsViewScatterMin), f(d::fAdsViewScatterMax),
                f(d::fHipViewScatterMin), f(d::fHipViewScatterMax), f(d::playerSpread), f(d::aiSpread), f(d::stackFireSpread), f(d::fHipReticleSidePos), w.def_i32(d::iFireTime), w.def_vec3(d::vViewFlashOffset), w.def_vec3(d::vWorldFlashOffset), w.def_u8(d::bUseAltTagFlash),
                w.def_i32(d::iProjectileSpeed), w.def_i32(d::iProjectileSpeedUp), w.def_i32(d::iProjectileSpeedRelativeUp),
                w.def_i32(d::iProjectileSpeedForward), f(d::fProjectileTakeParentVelocity),
                w.def_string(d::meleeSwipeSound), w.def_string(d::meleeSwipeSoundPlayer), w.def_string(d::meleeHitSound), w.def_string(d::meleeMissSound),
                w.def_tracer(d::tracerType),
            );
        }
        let world = w.world_models.first().cloned().unwrap_or_default();
        let anim_named = w.xanims.iter().filter(|a| !a.is_empty()).count();
        let anim_found = w
            .xanims
            .iter()
            .filter(|a| !a.is_empty() && anims.contains_key(a.as_str()))
            .count();
        let gun_found = gun.is_empty() || models.contains_key(&gun);
        let hand_found = hand.is_empty() || models.contains_key(&hand);
        if !gun_found || !hand_found {
            missing_models += 1;
        }
        missing_anims += anim_named - anim_found;
        for (_, f) in &w.fx {
            if !fx.contains_key(f) {
                missing_fx += 1;
            }
        }
        let gun_bones = models.get(&gun).map_or(0, |m| m.1.bone_names.len());
        let hand_bones = models.get(&hand).map_or(0, |m| m.1.bone_names.len());
        println!(
            "{name}: type={} class={} fire={}ms clip={} dmg={} gun='{gun}'{} ({gun_bones} bones) hands='{hand}'{} ({hand_bones} bones) world='{world}' anims {anim_found}/{anim_named} fire_snd='{}' flash='{}' wflash='{}' eject='{}' notetrack_snds={}",
            w.def_i32(d::weapType),
            w.def_i32(d::weapClass),
            w.def_i32(d::iFireTime),
            w.var_i32(fastfile_t6::layout::WeaponVariantDef::iClipSize),
            w.def_i32(d::damage),
            if gun_found { "" } else { " MISSING" },
            if hand_found { "" } else { " MISSING" },
            w.def_string(d::fireSoundPlayer),
            w.def_fx(d::viewFlashEffect),
            w.def_fx(d::worldFlashEffect),
            w.def_fx(d::viewShellEjectEffect),
            w.notetrack_sounds.len(),
        );
    }
    println!(
        "weapons: {} with a missing gun/hand model, {missing_anims} named anims not found, {missing_fx} fx names not found",
        missing_models
    );
    // Skinning sanity for every model with bones.
    let mut checked = 0;
    let mut bad = Vec::new();
    for (name, (_, m)) in models {
        if m.bone_names.len() < 2 {
            continue;
        }
        checked += 1;
        for (i, srf) in m.lod0.iter().enumerate() {
            let rigid: usize = srf.vert_lists.iter().map(|v| usize::from(v.1)).sum();
            let blended: usize = srf.blend_counts.iter().map(|&c| c.max(0) as usize).sum();
            if rigid + blended != usize::from(srf.vert_count) {
                bad.push(format!(
                    "{name} surf {i}: rigid {rigid} + blended {blended} != verts {}",
                    srf.vert_count
                ));
            }
        }
        if m.base_mat.len() != m.bone_names.len() {
            bad.push(format!("{name}: base_mat {} != bones {}", m.base_mat.len(), m.bone_names.len()));
        }
    }
    println!("skinned models checked: {checked}, problems: {}", bad.len());
    for b in bad.iter().take(20) {
        println!("  {b}");
    }
    // Animation data sanity: track names vs counts, data presence.
    let mut empty = 0;
    let mut with_data = 0;
    for a in anims.values() {
        if a.names.len() != usize::from(a.bone_count[9]) {
            empty += 1;
        } else if !a.data_short.is_empty() || !a.data_byte.is_empty() || !a.random_data_short.is_empty() {
            with_data += 1;
        }
    }
    println!("anims: {} total, {with_data} with data, {empty} with name/count mismatch", anims.len());
}

fn fx_report(captures: &[asset_t6::ZoneCapture], fx: &HashMap<String, &asset_t6::FxEffectRef>) {
    let mut types: BTreeMap<u8, usize> = BTreeMap::new();
    let mut visual_kinds: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut missing_children = BTreeMap::new();
    for e in fx.values() {
        for el in &e.elems {
            *types.entry(el.elem_type()).or_default() += 1;
            for v in &el.visuals {
                let k = match v {
                    asset_t6::FxVisualRef::Material(_) => "material",
                    asset_t6::FxVisualRef::Mark(_) => "mark",
                    asset_t6::FxVisualRef::Model(_) => "model",
                    asset_t6::FxVisualRef::Effect(name) => {
                        if !fx.contains_key(name) {
                            *missing_children.entry(name.clone()).or_insert(0) += 1;
                        }
                        "effect"
                    }
                    asset_t6::FxVisualRef::Sound(_) => "sound",
                    asset_t6::FxVisualRef::Light(_) => "light",
                    asset_t6::FxVisualRef::None => "none",
                };
                *visual_kinds.entry(k).or_default() += 1;
            }
            for child in [&el.effect_on_impact, &el.effect_on_death, &el.effect_emitted] {
                if !child.is_empty() && !fx.contains_key(child) {
                    *missing_children.entry(child.clone()).or_insert(0) += 1;
                }
            }
        }
    }
    println!("\n== fx: {} effects; elem types {:?}; visuals {:?}", fx.len(), types, visual_kinds);
    // T6M2_FLAGS=1: per element flag bit, how many elements carry it (by
    // element type) and a few of their effects.
    if std::env::var_os("T6M2_FLAGS").is_some() {
        use fastfile_t6::layout::FxElemDef as d;
        let mut names: Vec<&String> = fx.keys().collect();
        names.sort();
        for bit in 0..32 {
            let mask = 1i32 << bit;
            let mut by_type: BTreeMap<u8, usize> = BTreeMap::new();
            let mut some: Vec<String> = Vec::new();
            for name in &names {
                for (i, el) in fx[*name].elems.iter().enumerate() {
                    if el.i32_at(d::flags) & mask != 0 {
                        *by_type.entry(el.elem_type()).or_default() += 1;
                        if some.len() < 400 {
                            some.push(format!("{name}#{i}"));
                        }
                    }
                }
            }
            if !by_type.is_empty() {
                println!("flag {mask:#010x}: by type {by_type:?} e.g. {some:?}");
            }
        }
    }
    // T6M2_FONT=<substring>: each matching font: size, material, glyphs.
    if let Ok(want) = std::env::var("T6M2_FONT") {
        for c in captures {
            for f in c.fonts.iter().filter(|f| f.name.contains(&want)) {
                println!(
                    "font {} ({}): pixel height {}, material {}, {} glyphs, {} kerning pairs",
                    f.name,
                    c.zone,
                    f.pixel_height,
                    f.material,
                    f.glyphs.len(),
                    f.kerning.len()
                );
                for g in &f.glyphs {
                    println!(
                        "   {:?}: x0 {} y0 {} dx {} size {}x{} st {:?}",
                        char::from_u32(u32::from(g.letter)),
                        g.x0,
                        g.y0,
                        g.dx,
                        g.pixel_width,
                        g.pixel_height,
                        g.st
                    );
                }
            }
        }
    }
    // T6M2_TABLE=<substring>: each matching string table, row by row.
    if let Ok(want) = std::env::var("T6M2_TABLE") {
        for c in captures {
            for t in c.string_tables.iter().filter(|t| t.name.contains(&want)) {
                println!("table {} ({}): {} x {}", t.name, c.zone, t.rows, t.columns);
                for r in 0..t.rows {
                    let row: Vec<&str> = (0..t.columns)
                        .map(|k| t.cells.get(r * t.columns + k).map_or("", String::as_str))
                        .collect();
                    println!("   {r}: {}", row.join(" | "));
                }
            }
        }
    }
    // T6M2_CAMO=<substring>: each matching weapon camo: its sets (images
    // per camo index) and material swaps; and the weapons that use it.
    if let Ok(want) = std::env::var("T6M2_CAMO") {
        for c in captures {
            println!(
                "zone {}: {} camos loaded, {} kept",
                c.zone,
                c.loads.get(&fastfile_t6::AssetType::WeaponCamo).copied().unwrap_or(0),
                c.weapon_camos.len()
            );
            for camo in c.weapon_camos.iter().filter(|x| x.name.contains(&want)) {
                println!(
                    "camo {} ({}): base images '{}' '{}', {} sets, {} material sets",
                    camo.name,
                    c.zone,
                    camo.solid_base_image,
                    camo.pattern_base_image,
                    camo.sets.len(),
                    camo.material_sets.len()
                );
                for (i, set) in camo.sets.iter().enumerate() {
                    println!("   set {i}: solid '{}' pattern '{}' offset {:?} scale {}", set.solid, set.pattern, set.offset, set.scale);
                }
                for (i, list) in camo.material_sets.iter().enumerate() {
                    for m in list {
                        println!("   materials {i}: flags {:#x} {:?} -> {:?} consts {:?}", m.replace_flags, m.base, m.camo, m.consts);
                    }
                }
                let users: Vec<&str> = captures
                    .iter()
                    .flat_map(|cc| cc.weapons.iter())
                    .filter(|w| w.camo == camo.name)
                    .map(|w| w.name.as_str())
                    .collect();
                println!("   used by {users:?}");
            }
        }
    }
    // T6M2_FX=<substring>: each matching effect's elements: type, flags,
    // counts, atlas, visuals and each material's technique set and textures.
    if let Ok(want) = std::env::var("T6M2_FX") {
        use fastfile_t6::layout::FxElemDef as d;
        let mut names: Vec<&String> = fx.keys().filter(|n| n.contains(&want)).collect();
        names.sort();
        for name in names {
            let e = fx[name];
            println!("effect {name}: flags {:#x} elems {} (looping {} oneshot {} emission {}) loop life {}",
                e.flags, e.elems.len(), e.looping_count, e.one_shot_count, e.emission_count, e.msec_looping_life);
            for (i, el) in e.elems.iter().enumerate() {
                let b = |o: usize| el.raw.get(o).copied().unwrap_or(0);
                println!("  elem {i}: type {} flags {:#010x} spawn [{}, {}] life [{}, {}] vis {} lighting {} sort {} atlas {:?} visuals {:?}",
                    el.elem_type(), el.i32_at(d::flags), el.i32_at(d::spawn), el.i32_at(d::spawn + 4),
                    el.i32_at(d::lifeSpanMsec), el.i32_at(d::lifeSpanMsec + 4), b(d::visualCount), b(d::lightingFrac), b(d::sortOrder),
                    el.raw.get(d::atlas..d::atlas + 8), el.visuals);
                let vel_n = b(d::velIntervalCount);
                println!("     spawn range [{}, {}] fade in range [{}, {}] fade out range [{}, {}] spawn frustum cull radius {}",
                    el.f32_at(d::spawnRange), el.f32_at(d::spawnRange + 4),
                    el.f32_at(d::fadeInRange), el.f32_at(d::fadeInRange + 4), el.f32_at(d::fadeOutRange), el.f32_at(d::fadeOutRange + 4), el.f32_at(d::spawnFrustumCullRadius));
                println!(
                    "     rotation axis {} initial rotation [{}, {}] spawn angles {:?} angular vel {:?} billboard pivot [{}, {}] spawn delay [{}, {}] alpha fade {} offset height [{}, {}]",
                    el.i32_at(d::rotationAxis),
                    el.f32_at(d::initialRotation),
                    el.f32_at(d::initialRotation + 4),
                    (0..6)
                        .map(|k| el.f32_at(d::spawnAngles + k * 4))
                        .collect::<Vec<_>>(),
                    (0..6)
                        .map(|k| el.f32_at(d::angularVelocity + k * 4))
                        .collect::<Vec<_>>(),
                    el.f32_at(d::billboardPivot),
                    el.f32_at(d::billboardPivot + 4),
                    el.i32_at(d::spawnDelayMsec),
                    el.i32_at(d::spawnDelayMsec + 4),
                    el.raw
                        .get(d::alphaFadeTimeMsec..d::alphaFadeTimeMsec + 2)
                        .map_or(0, |b| u16::from_le_bytes([b[0], b[1]])),
                    el.f32_at(d::spawnOffsetHeight),
                    el.f32_at(d::spawnOffsetHeight + 4)
                );
                println!("     gravity [{}, {}] wind {} vel intervals {} vis intervals {} u (cloud density) [{}, {}] / [{:#x}, {:#x}] spawn origin {:?} offset radius [{}, {}]",
                    el.f32_at(d::gravity), el.f32_at(d::gravity + 4), el.f32_at(d::windInfluence), vel_n, b(d::visStateIntervalCount),
                    el.f32_at(d::u), el.f32_at(d::u + 4), el.i32_at(d::u), el.i32_at(d::u + 4),
                    (0..6).map(|k| el.f32_at(d::spawnOrigin + k * 4)).collect::<Vec<_>>(),
                    el.f32_at(d::spawnOffsetRadius), el.f32_at(d::spawnOffsetRadius + 4));
                let vf = |k: usize, at: usize| -> [f32; 6] {
                    std::array::from_fn(|i| {
                        let o = k * 96 + at + i * 4;
                        el.vel_samples.get(o..o + 4).map_or(f32::NAN, |b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    })
                };
                let vis = |k: usize, at: usize| -> f32 {
                    let o = k * 48 + at;
                    el.vis_samples.get(o..o + 4).map_or(f32::NAN, |b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                };
                let vis_n = usize::from(b(d::visStateIntervalCount));
                let rgba = |k: usize, at: usize| -> Vec<u8> {
                    el.vis_samples.get(k * 48 + at..k * 48 + at + 4).map_or(Vec::new(), |b| b.to_vec())
                };
                println!("     vis colour first {:?} (+{:?}) last {:?} (+{:?})", rgba(0, 0), rgba(0, 24), rgba(vis_n, 0), rgba(vis_n, 24));
                if std::env::var_os("T6M2_VIS_RAW").is_some() {
                    for k in 0..=vis_n {
                        let raw: Vec<String> = (0..12).map(|i| {
                            let o = k * 48 + i * 4;
                            el.vis_samples.get(o..o + 4).map_or("-".to_owned(), |b| format!("{:02x}{:02x}{:02x}{:02x}/{:.3}", b[0], b[1], b[2], b[3], f32::from_le_bytes([b[0], b[1], b[2], b[3]])))
                        }).collect();
                        println!("       vis[{k}] {}", raw.join(" "));
                    }
                }
                println!("     vis first: size {} (+{}) x {} (+{}) scale {} (+{}); last: size {} (+{}) x {} scale {}",
                    vis(0, 12), vis(0, 36), vis(0, 16), vis(0, 40), vis(0, 20), vis(0, 44),
                    vis(vis_n, 12), vis(vis_n, 36), vis(vis_n, 16), vis(vis_n, 20));
                let last = usize::from(vel_n);
                println!("     vel first sample: local vel {:?} delta {:?} world vel {:?} delta {:?}",
                    vf(0, 0), vf(0, 24), vf(0, 48), vf(0, 72));
                println!("     vel last sample: local vel {:?} delta {:?} world vel {:?} delta {:?}",
                    vf(last, 0), vf(last, 24), vf(last, 48), vf(last, 72));
                for v in &el.visuals {
                    let names: Vec<&String> = match v {
                        asset_t6::FxVisualRef::Material(m) => vec![m],
                        asset_t6::FxVisualRef::Mark(pair) => pair.iter().collect(),
                        _ => Vec::new(),
                    };
                    for m in names {
                        for c in captures {
                            if let Some(mat) = c.materials.iter().find(|x| x.name == *m) {
                                let ts = mat.technique_set.map_or("-".to_owned(), |t| c.technique_sets[t.index].name.clone());
                                let texs: Vec<String> = mat.textures.iter().map(|t| {
                                    let img = t.image.map(|k| &c.images[k.index]);
                                    let home = img.and_then(|i| i.name.strip_prefix(',')).map(|real| {
                                        captures.iter().find_map(|cc| cc.images.iter().find(|x| x.name == real).map(|x| format!(" [home {} {}x{}]", cc.zone, x.width, x.height))).unwrap_or_else(|| " [NO HOME]".to_owned())
                                    }).unwrap_or_default();
                                    format!("{:#010x}:{}{home}", t.name_hash, img.map_or("-".to_owned(), |i| format!("{} {}x{}", i.name, i.width, i.height)))
                                }).collect();
                                let state = mat.state_bits.first().map(|b| asset_t6::DrawState::decode(*b));
                                println!("     material {m} ({}) techset {ts} textures {texs:?} consts {:?} state0 {:?}", c.zone, mat.constants.iter().map(|k| (&k.1, k.2)).collect::<Vec<_>>(), state);
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
    println!("fx children not found: {} {:?}", missing_children.len(), missing_children.keys().take(10).collect::<Vec<_>>());
    for c in captures {
        for t in &c.impact_tables {
            let named = t.rows.iter().map(|r| r.0.iter().chain(r.1.iter()).filter(|n| !n.is_empty()).count()).sum::<usize>();
            let found = t.rows.iter().map(|r| r.0.iter().chain(r.1.iter()).filter(|n| !n.is_empty() && fx.contains_key(n.as_str())).count()).sum::<usize>();
            println!("impact table {} ({}): {} rows, {found}/{named} effects found", t.name, c.zone, t.rows.len());
            for (i, r) in t.rows.iter().enumerate().take(3) {
                println!("  row {i}: concrete='{}' metal='{}' flesh0='{}'", r.0[5], r.0[13], r.1[0]);
            }
            // T6M2_IMPACT=1: every row, every surface.
            if std::env::var_os("T6M2_IMPACT").is_some() {
                for (i, r) in t.rows.iter().enumerate() {
                    println!("  row {i}:");
                    for (s, n) in r.0.iter().enumerate() {
                        println!("     surf {s:>2}: {n}");
                    }
                    println!("     flesh: {:?}", r.1);
                }
            }
        }
        for t in &c.tracers {
            println!("tracer {} ({}): mat='{}' type={} every={} speed={} len={} width={} screw={}/{} fade={}/{} repeat={} colours={:?}", t.name, c.zone, t.material, t.ty, t.draw_interval, t.speed, t.beam_length, t.beam_width, t.screw_radius, t.screw_dist, t.fade_time, t.fade_scale, t.tex_repeat_rate, t.colors);
            for cc in captures {
                if let Some(mat) = cc.materials.iter().find(|x| x.name == t.material) {
                    let ts = mat.technique_set.map_or("-".to_owned(), |k| cc.technique_sets[k.index].name.clone());
                    let texs: Vec<String> = mat.textures.iter().map(|x| format!("{:#010x}:{}", x.name_hash, x.image.map_or("-".to_owned(), |k| { let i = &cc.images[k.index]; format!("{} {}x{} hash {:#010x} parts {} part_hash {:#010x} embedded {:?}", i.name, i.width, i.height, i.name_hash, i.streamed_parts, i.part_hash, i.embedded.as_ref().map(|e| (e.dxgi_format, e.data.len()))) }))).collect();
                    let state = mat.state_bits.first().map(|b| asset_t6::DrawState::decode(*b));
                    println!("   material {} ({}) techset {ts} textures {texs:?} consts {:?} state0 {:?}", t.material, cc.zone, mat.constants.iter().map(|k| (&k.1, k.2)).collect::<Vec<_>>(), state);
                    break;
                }
            }
        }
        for t in &c.footstep_fx_tables {
            println!("footstep fx table {} ({}): dirt='{}' concrete='{}'", t.name, c.zone, t.fx[6], t.fx[5]);
        }
    }
}

fn sound_report(root: &Path, captures: &[asset_t6::ZoneCapture], decode: bool) {
    // Every bank file in the sound folder, indexed by entry id.
    let dir = root.join("sound");
    let mut index: HashMap<u32, (PathBuf, sab_t6::BankEntry)> = HashMap::new();
    let mut files = 0;
    if let Ok(read) = std::fs::read_dir(&dir) {
        let mut names: Vec<PathBuf> = read.filter_map(|e| e.ok().map(|e| e.path())).collect();
        names.sort();
        for path in names {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_owned();
            let wanted = ["zmb_nuked", "zmb_common", "zmb_patch", "zmb_code_post_gfx", "cmn_root"]
                .iter()
                .any(|p| name.starts_with(p))
                && (name.contains(".all.") || name.contains(".english."));
            if !wanted {
                continue;
            }
            match sab_t6::BankFile::open(&path) {
                Ok(bank) => {
                    files += 1;
                    for e in &bank.entries {
                        index.entry(e.id).or_insert_with(|| (path.clone(), *e));
                    }
                }
                Err(e) => println!("bank {name}: {e}"),
            }
        }
    }
    println!("\n== sound: {files} bank files, {} entries indexed", index.len());
    let mut total = 0;
    let mut entries = 0;
    let mut resolved = 0;
    let mut unresolved_names = Vec::new();
    for c in captures {
        for b in &c.sound_banks {
            total += b.aliases.len();
            for a in &b.aliases {
                for e in &a.entries {
                    entries += 1;
                    if e.asset_id == 0 || index.contains_key(&e.asset_id) {
                        resolved += 1;
                    } else if unresolved_names.len() < 12 {
                        unresolved_names.push(format!("{} -> {} ({:#x})", e.name, e.asset_file, e.asset_id));
                    }
                }
            }
            println!("bank {} ({}): {} aliases", b.name, c.zone, b.aliases.len());
        }
        if let Some(g) = &c.snd_globals {
            println!("driver globals {} ({}): {} curves, {} groups; curve 0 = {} {:?}", g.name, c.zone, g.curves.len(), g.groups.len(),
                g.curves.first().map_or("", |c| c.name.as_str()), g.curves.first().map(|c| c.points));
        }
        for t in &c.footstep_tables {
            println!("footstep table {} ({}): dirt ids {:?}", t.name, c.zone, t.aliases[6]);
            if std::env::var_os("T6M2_FOOTSTEPS").is_some() && t.name == "default_1st_person" {
                let names: std::collections::HashMap<u32, &str> = captures
                    .iter()
                    .flat_map(|c| c.sound_banks.iter())
                    .flat_map(|b| b.aliases.iter())
                    .map(|a| (sab_t6::sound_hash(&a.name), a.name.as_str()))
                    .collect();
                for (surf, row) in t.aliases.iter().enumerate() {
                    let row: Vec<String> = row
                        .iter()
                        .map(|id| names.get(id).map_or(format!("{id:#x}"), |n| (*n).to_owned()))
                        .collect();
                    println!("   surf {surf:>2}: {}", row.join(" | "));
                }
            }
        }
    }
    if let Ok(want) = std::env::var("T6M2_ALIAS") {
        for c in captures {
            for b in &c.sound_banks {
                for a in b.aliases.iter().filter(|a| a.name.contains(&want)) {
                    let e = a.entries.first();
                    println!(
                        "alias {} ({}): {} variants, flags {:?}, file {}, secondary '{}', ids {:?}",
                        a.name,
                        c.zone,
                        a.entries.len(),
                        e.map(|e| e.flags),
                        e.map_or("", |e| e.asset_file.as_str()),
                        e.map_or("", |e| e.secondary.as_str()),
                        a.entries.iter().map(|e| format!("{:#x}", e.asset_id)).collect::<Vec<_>>()
                    );
                    if std::env::var_os("T6M2_FIND_BANK").is_some() {
                        for e in &a.entries {
                            let mut homes = Vec::new();
                            if let Ok(dir) = std::fs::read_dir(root.join("sound")) {
                                for f in dir.flatten() {
                                    if let Ok(bank) = sab_t6::BankFile::open(&f.path())
                                        && bank.entries.iter().any(|x| x.id == e.asset_id)
                                    {
                                        homes.push(f.file_name().to_string_lossy().into_owned());
                                    }
                                }
                            }
                            println!("   variant {:#010x} {} -> {:?}", e.asset_id, e.asset_file, homes);
                        }
                    }
                }
            }
        }
    }
    println!("aliases {total}, variants {entries}, file resolved {resolved}, unresolved {}", entries - resolved);
    for n in &unresolved_names {
        println!("  unresolved: {n}");
    }
    if decode {
        let mut ok = 0;
        let mut md5_bad = 0;
        let mut failed = 0;
        let mut frames = 0u64;
        let started = std::time::Instant::now();
        for (path, e) in index.values() {
            let Ok(bank) = sab_t6::BankFile::open(path) else { continue };
            let Ok(data) = bank.read(e) else { failed += 1; continue };
            match e.format {
                sab_t6::SoundFormat::Flac => match sab_t6::decode_flac(&data) {
                    Ok(stream) => {
                        if stream.md5_ok == Some(false) {
                            md5_bad += 1;
                            println!("  md5 mismatch: {:#x} in {}", e.id, path.display());
                        } else {
                            ok += 1;
                        }
                        frames += stream.pcm.frames() as u64;
                    }
                    Err(err) => {
                        failed += 1;
                        println!("  flac fail {:#x} in {}: {err}", e.id, path.display());
                    }
                },
                sab_t6::SoundFormat::Pcm16 => {
                    ok += 1;
                    frames += u64::from(e.frame_count);
                }
                other => {
                    failed += 1;
                    println!("  format {other:?} {:#x}", e.id);
                }
            }
        }
        println!(
            "decode: {ok} ok, {md5_bad} md5 mismatch, {failed} failed, {:.1} h of audio in {:.1}s",
            frames as f64 / 48000.0 / 3600.0,
            started.elapsed().as_secs_f32()
        );
    }
}
