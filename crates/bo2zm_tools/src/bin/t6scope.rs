//! bo2mp: what a Black Ops II gun's definition says about aiming down its
//! sights, read-only. `t6scope <weapon>...` (e.g. `svu_mp dsr50_mp`) looks
//! each weapon up in the multiplayer zones (`T6SCOPE_ZONES` to change them)
//! and prints its scope overlay material and picture, the overlay's size and
//! reticle kind, the ADS zoom fields, transition times, sway, hold-breath
//! flag and ADS sounds. `T6SCOPE_PNG=<dir>` also writes each overlay picture
//! (top level, RGBA) there as `<image>.png`.

use std::path::Path;
use std::process::ExitCode;

use asset_t6::{ImageSource, PackSet, capture_zone};
use fastfile_t6::layout::{WeaponDef as d, WeaponVariantDef as v};

const ZONES: &str = r"C:\Program Files (x86)\Steam\steamapps\common\Call of Duty Black Ops II\zone\all";

fn main() -> ExitCode {
    let names: Vec<String> = std::env::args().skip(1).collect();
    let dir = Path::new(ZONES);
    let packs = match PackSet::open_dir(dir) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("packs: {e}");
            return ExitCode::FAILURE;
        }
    };
    let zones: Vec<String> = std::env::var("T6SCOPE_ZONES").map_or_else(
        |_| ["common_mp", "common_patch_mp", "patch_mp"].map(str::to_owned).to_vec(),
        |z| z.split(',').map(str::to_owned).collect(),
    );
    let png_dir = std::env::var("T6SCOPE_PNG").ok();
    for zone in &zones {
        let capture = match capture_zone(&dir.join(format!("{zone}.ff"))) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{zone}: {e}");
                continue;
            }
        };
        // `T6SCOPE_LOC=zoom`: every localized string whose key or text holds
        // it (a language zone, e.g. `T6SCOPE_ZONES=../english/en_patch_mp`).
        if let Ok(pat) = std::env::var("T6SCOPE_LOC") {
            for (k, t) in &capture.localize {
                if k.to_ascii_lowercase().contains(&pat) || t.to_ascii_lowercase().contains(&pat) {
                    println!("{zone} loc {k} = {t}");
                }
            }
        }
        // `T6SCOPE_MODELS=a,b`: those models' bounds and surfaces' extents.
        if let Ok(list) = std::env::var("T6SCOPE_MODELS") {
            for m in capture.xmodels.iter().filter(|m| list.split(',').any(|n| n == m.name)) {
                println!("{zone} model {} mins {:?} maxs {:?} radius {}", m.name, m.mins, m.maxs, m.radius);
                // Each first-LOD surface: its material and its vertices' box.
                for (i, s) in m.lod0.iter().enumerate() {
                    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
                    for v in s.verts.chunks_exact(32) {
                        for a in 0..3 {
                            let x = f32::from_le_bytes([v[a * 4], v[a * 4 + 1], v[a * 4 + 2], v[a * 4 + 3]]);
                            lo[a] = lo[a].min(x);
                            hi[a] = hi[a].max(x);
                        }
                    }
                    let mat = s
                        .material
                        .and_then(|k| capture.materials.get(k.index))
                        .map_or("-", |m| m.name.as_str());
                    println!("    surf {i} `{mat}` verts {} box {lo:?} .. {hi:?}", s.vert_count);
                }
            }
        }
        // `T6SCOPE_ALIAS=breath`: every sound alias whose name holds it.
        if let Ok(pat) = std::env::var("T6SCOPE_ALIAS") {
            for bank in &capture.sound_banks {
                for a in bank.aliases.iter().filter(|a| a.name.to_ascii_lowercase().contains(&pat)) {
                    println!("{zone} alias {} ({} entries) bank {}", a.name, a.entries.len(), bank.name);
                }
            }
        }
        for w in capture.weapons.iter().filter(|w| {
            names.is_empty() && !w.overlay_material.is_empty() || names.iter().any(|n| n == &w.name)
        }) {
            println!("{zone} {}", w.name);
            println!(
                "  overlay `{}` low `{}` reticle {} interface {} size {}x{} alphaScale {} holdBreath {} antiQuickScope {}",
                w.overlay_material,
                w.overlay_material_low,
                w.def_i32(d::overlayReticle),
                w.def_i32(d::overlayInterface),
                w.def_f32(d::overlayWidth),
                w.def_f32(d::overlayHeight),
                w.var_f32(v::fOverlayAlphaScale),
                w.def_u8(d::bHoldBreathToSteady),
                w.var_u8(v::bAntiQuickScope),
            );
            println!(
                "  zoomFov {} {} {} inFrac {} outFrac {} transIn {}ms transOut {}ms aimDownSight {} adsFireOnly {}",
                w.var_f32(v::fAdsZoomFov1),
                w.var_f32(v::fAdsZoomFov2),
                w.var_f32(v::fAdsZoomFov3),
                w.var_f32(v::fAdsZoomInFrac),
                w.var_f32(v::fAdsZoomOutFrac),
                w.var_i32(v::iAdsTransInTime),
                w.var_i32(v::iAdsTransOutTime),
                w.def_u8(d::aimDownSight),
                w.def_u8(d::adsFireOnly),
            );
            println!(
                "  adsIdle {} speed {} hipIdle {} adsSway max {} lerp {} pitch {} yaw {} horiz {} vert {} adsBobFactor {} adsViewBobMult {}",
                w.def_f32(d::fAdsIdleAmount),
                w.def_f32(d::adsIdleSpeed),
                w.def_f32(d::fHipIdleAmount),
                w.def_f32(d::adsSwayMaxAngle),
                w.def_f32(d::adsSwayLerpSpeed),
                w.def_f32(d::adsSwayPitchScale),
                w.def_f32(d::adsSwayYawScale),
                w.var_f32(v::fAdsSwayHorizScale),
                w.var_f32(v::fAdsSwayVertScale),
                w.def_f32(d::fAdsBobFactor),
                w.def_f32(d::fAdsViewBobMult),
            );
            println!(
                "  sounds adsRaise `{}` adsLower `{}` adsZoom `{}` reticle centre `{}` side `{}` dof {} {}",
                w.def_string(d::adsRaiseSoundPlayer),
                w.def_string(d::adsLowerSoundPlayer),
                w.def_string(d::adsZoomSound),
                w.def_material(d::reticleCenter),
                w.def_material(d::reticleSide),
                w.def_f32(d::adsDofStart),
                w.def_f32(d::adsDofEnd),
            );
            if std::env::var_os("T6SCOPE_FULL").is_some() {
                println!("  hide {:?} attach {:?}", w.hide_tags, w.attach_view_models);
                for (i, a) in w.xanims.iter().enumerate().filter(|(_, a)| !a.is_empty()) {
                    print!(" {i:02x}={a}");
                }
                println!();
            }
            let base = w.name.trim_end_matches("_mp");
            let prefix = format!("au_{base}_");
            for au in capture.attachment_uniques.iter().filter(|a| a.name.starts_with(&prefix)) {
                use fastfile_t6::layout::WeaponAttachmentUnique as u;
                let kind = &au.name[prefix.len()..];
                println!(
                    "  {} overlay `{}` reticle {} view `{}` +`{}` ads `{}` tag `{}` ofs {:?} rot {:?} anims {} alt `{}`",
                    au.name,
                    au.overlay_material,
                    au.i32_at(u::overlayReticle),
                    au.view_model,
                    au.view_model_additional,
                    au.view_model_ads,
                    au.view_model_tag,
                    au.vec3_at(u::viewModelOffsets),
                    au.vec3_at(u::viewModelRotations),
                    au.xanims.iter().filter(|a| !a.is_empty()).count(),
                    au.alt_weapon_name,
                );
                if std::env::var_os("T6SCOPE_FULL").is_some() {
                    println!(
                        "    point {} hide {:?} disableBaseAttachment {} disableBaseClip {} overrideOffsets {}",
                        capture
                            .attachments
                            .iter()
                            .find(|a| kind.split('+').next() == Some(a.name.as_str()))
                            .map_or(-1, |a| a.i32_at(fastfile_t6::layout::WeaponAttachment::attachmentPoint)),
                        au.hide_tags,
                        au.bytes.get(u::disableBaseWeaponAttachment).copied().unwrap_or(0),
                        au.bytes.get(u::disableBaseWeaponClip).copied().unwrap_or(0),
                        au.bytes.get(u::overrideBaseWeaponAttachmentOffsets).copied().unwrap_or(0),
                    );
                    for (i, a) in au.xanims.iter().enumerate().filter(|(_, a)| !a.is_empty()) {
                        print!(" {i:02x}={a}");
                    }
                    println!();
                }
                for part in kind.split('+') {
                    if let Some(a) = capture.attachments.iter().find(|a| a.name == part) {
                        use fastfile_t6::layout::WeaponAttachment as at;
                        println!(
                            "    attachment {} zoomFov {} {} {} inFrac {} outFrac {} transScale {} {} adsIdleScale {} swayOverride {} adsSwayOverride {} mms {}",
                            a.name,
                            a.f32_at(at::fAdsZoomFov1),
                            a.f32_at(at::fAdsZoomFov2),
                            a.f32_at(at::fAdsZoomFov3),
                            a.f32_at(at::fAdsZoomInFrac),
                            a.f32_at(at::fAdsZoomOutFrac),
                            a.f32_at(at::fAdsTransInTimeScale),
                            a.f32_at(at::fAdsTransOutTimeScale),
                            a.f32_at(at::fAdsIdleAmountScale),
                            a.u8_at(at::swayOverride),
                            a.u8_at(at::adsSwayOverride),
                            a.u8_at(at::mmsWeapon),
                        );
                    }
                }
                if !au.overlay_material.is_empty()
                    && let Some(dir) = &png_dir
                {
                    dump_material(&capture, &packs, &au.overlay_material, dir);
                }
            }
            for name in [&w.overlay_material, &w.overlay_material_low] {
                let Some(m) = capture.materials.iter().find(|m| &m.name == name) else {
                    continue;
                };
                let technique = m
                    .technique_set
                    .and_then(|k| capture.technique_sets.get(k.index))
                    .map_or("", |t| t.name.as_str());
                println!("  material {} technique {technique} constants {:?}", m.name, m.constants);
                for t in &m.textures {
                    let Some(img) = t.image.and_then(|k| capture.images.get(k.index)) else {
                        continue;
                    };
                    println!(
                        "    slot {:08x} semantic {} image {} {}x{}",
                        t.name_hash, t.semantic, img.name, img.width, img.height
                    );
                    if let (Some(dir), Some(ImageSource::Pack(i, entry))) = (&png_dir, packs.locate(img))
                        && let Ok(bytes) = packs.packs[i].read(entry)
                        && let Ok(iwi) = ipak_t6::parse_iwi(&bytes)
                    {
                        write_png(dir, &img.name, &iwi);
                    }
                }
            }
        }
    }
    ExitCode::SUCCESS
}

/// A material's textures, each written as a PNG.
fn dump_material(capture: &asset_t6::ZoneCapture, packs: &PackSet, name: &str, dir: &str) {
    let Some(m) = capture.materials.iter().find(|m| m.name == name) else {
        println!("    material {name}: not in this zone");
        return;
    };
    for t in &m.textures {
        let Some(img) = t.image.and_then(|k| capture.images.get(k.index)) else {
            continue;
        };
        println!("    material {name} image {} {}x{}", img.name, img.width, img.height);
        if let Some(ImageSource::Pack(i, entry)) = packs.locate(img)
            && let Ok(bytes) = packs.packs[i].read(entry)
            && let Ok(iwi) = ipak_t6::parse_iwi(&bytes)
        {
            write_png(dir, &img.name, &iwi);
        }
    }
}

/// An image's top level as an RGBA PNG (BC1/BC2/BC3 and RGBA8 only).
fn write_png(dir: &str, name: &str, iwi: &ipak_t6::IwiImage<'_>) {
    use ipak_t6::IwiFormat as F;
    let (w, h) = (iwi.width as usize, iwi.height as usize);
    let data = iwi.level(0);
    let (block, decode): (usize, fn(&[u8], &mut [u8], usize)) = match iwi.format {
        F::Dxt1 => (8, bcdec_rs::bc1),
        F::Dxt3 => (16, bcdec_rs::bc2),
        F::Dxt5 => (16, bcdec_rs::bc3),
        F::Rgba8 => (0, |_, _, _| {}),
        other => {
            println!("    (not written: {other:?})");
            return;
        }
    };
    let mut rgba = vec![0u8; w * h * 4];
    if block == 0 {
        let n = rgba.len().min(data.len());
        rgba[..n].copy_from_slice(&data[..n]);
    } else {
        let (bw, bh) = (w.div_ceil(4), h.div_ceil(4));
        let mut tile = [0u8; 64];
        for by in 0..bh {
            for bx in 0..bw {
                let at = (by * bw + bx) * block;
                let Some(src) = data.get(at..at + block) else { continue };
                decode(src, &mut tile, 16);
                for y in 0..4 {
                    for x in 0..4 {
                        let (px, py) = (bx * 4 + x, by * 4 + y);
                        if px < w && py < h {
                            let d = (py * w + px) * 4;
                            rgba[d..d + 4].copy_from_slice(&tile[(y * 4 + x) * 4..(y * 4 + x) * 4 + 4]);
                        }
                    }
                }
            }
        }
    }
    let path = Path::new(dir).join(format!("{name}.png"));
    if let Ok(file) = std::fs::File::create(&path) {
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        if let Ok(mut wr) = enc.write_header() {
            let _ = wr.write_image_data(&rgba);
        }
        println!("    wrote {} ({:?} {}x{})", path.display(), iwi.format, w, h);
    }
}
