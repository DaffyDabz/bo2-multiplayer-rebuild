//! bo2zm: where Black Ops II keeps a HUD icon's pixels, read-only.
//! `t6icon <material>...` looks each material up in Nuketown's zones
//! (common_zm, zm_nuked, patch_zm) and prints its colour map's image: size,
//! levels, and whether its pixels sit in an image pack or in the zone.
//! `T6ICON_ZONES=common_mp,ui_mp` looks in those zones instead;
//! `T6ICON_PNG=<dir>` also writes each pack image's top level there as
//! `<image>.png` (RGBA), and its material's constants are printed.

use std::path::Path;
use std::process::ExitCode;

use asset_t6::{ImageSource, PackSet, capture_zone};

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
    let zones: Vec<String> = std::env::var("T6ICON_ZONES").map_or_else(
        |_| ["common_zm", "zm_nuked", "patch_zm"].map(str::to_owned).to_vec(),
        |z| z.split(',').map(str::to_owned).collect(),
    );
    let png_dir = std::env::var("T6ICON_PNG").ok();
    // T6ICON_BYNAME=1: each name as an image name, looked up in the packs
    // by its hash alone (how a `,name` reference finds its pixels).
    if std::env::var_os("T6ICON_BYNAME").is_some() {
        for n in &names {
            if std::env::var_os("T6ICON_ALLPACKS").is_some() {
                let h = ipak_t6::hash_name(n.trim_start_matches(','));
                for p in &packs.packs {
                    for e in p.find_name(h) {
                        println!("  {n}: pack {} key {:x} size {}", p.name, e.key, e.size);
                    }
                }
            }
            match packs.locate_name(n) {
                Some((i, entry)) => {
                    println!("{n}: pack {} size {}", packs.packs[i].name, entry.size);
                    if let Some(dir) = &png_dir
                        && let Ok(bytes) = packs.packs[i].read(entry)
                        && let Ok(iwi) = ipak_t6::parse_iwi(&bytes)
                    {
                        write_png(dir, n, &iwi);
                    }
                }
                None => println!("{n}: in no pack"),
            }
        }
        return ExitCode::SUCCESS;
    }
    for zone in &zones {
        let capture = match capture_zone(&dir.join(format!("{zone}.ff"))) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{zone}: {e}");
                continue;
            }
        };
        for m in capture
            .materials
            .iter()
            .filter(|m| names.iter().any(|n| n == &m.name))
        {
            let techniques = m
                .technique_set
                .and_then(|k| capture.technique_sets.get(k.index))
                .map_or("", |t| t.name.as_str());
            println!("{zone} {} technique {techniques} constants {:?}", m.name, m.constants);
            // T6ICON_STATES=1: the draw state of every technique slot the material has.
            if std::env::var_os("T6ICON_STATES").is_some() {
                for t in 0..36 {
                    if let Some(d) = m.draw_state(t) {
                        println!("  tech {t}: src {} dst {} op {} atest {:?} rgb {} a {}", d.src_blend, d.dst_blend, d.blend_op, d.alpha_test, d.color_write_rgb, d.color_write_alpha);
                    }
                }
            }
            for t in &m.textures {
                let Some(img) = t.image.and_then(|k| capture.images.get(k.index)) else {
                    continue;
                };
                let source = match packs.locate(img) {
                    Some(ImageSource::Pack(i, entry)) => {
                        if let Some(dir) = &png_dir
                            && let Ok(bytes) = packs.packs[i].read(entry)
                            && let Ok(iwi) = ipak_t6::parse_iwi(&bytes)
                        {
                            write_png(dir, &img.name, &iwi);
                        }
                        format!("pack {i}")
                    }
                    Some(ImageSource::Embedded) => {
                        let e = img.embedded.as_ref().unwrap();
                        format!(
                            "zone dxgi {} levels {} {} bytes",
                            e.dxgi_format,
                            e.level_count,
                            e.data.len()
                        )
                    }
                    Some(ImageSource::Empty) => "empty".to_owned(),
                    None => "nowhere".to_owned(),
                };
                println!(
                    "  slot {:08x} semantic {} image {} (hash {:08x}, of name {:08x}) {}x{} levels {} streamed {} -> {source}",
                    t.name_hash,
                    t.semantic,
                    img.name,
                    img.name_hash,
                    ipak_t6::hash_name(img.name.trim_start_matches(',')),
                    img.width,
                    img.height,
                    img.level_count,
                    img.streamed_parts
                );
            }
        }
    }
    ExitCode::SUCCESS
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
            println!("  (not written: {other:?})");
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
        println!("  wrote {} ({:?})", path.display(), iwi.format);
    }
}
