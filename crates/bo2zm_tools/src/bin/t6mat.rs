//! bo2zm: measure a Black Ops II map's world materials, read-only.
//! `t6mat [zone.ff]` (default Nuketown) counts what the world surfaces' materials
//! ask for: technique sets, vertex formats, texture slots, and the pixel
//! formats of their colour maps and of the lightmaps.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use asset_t6::{ImageRef, ImageSource, PackSet, ZoneCapture, capture_zone};

const DEFAULT_ZONE: &str =
    r"C:\Program Files (x86)\Steam\steamapps\common\Call of Duty Black Ops II\zone\all\zm_nuked.ff";

/// An IEEE half float.
fn half_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = i32::from((bits >> 10) & 0x1f);
    let frac = f32::from(bits & 0x3ff);
    sign * match exp {
        0 => frac * 2f32.powi(-24),
        31 => f32::INFINITY,
        _ => (1.0 + frac / 1024.0) * 2f32.powi(exp - 15),
    }
}

/// `R_HashString`: case-folded, xor with 33 times the running hash.
fn hash(name: &str) -> u32 {
    name.bytes()
        .fold(0u32, |h, c| u32::from(c | 0x20) ^ h.wrapping_mul(33))
}

const SLOT_NAMES: [&str; 24] = [
    "colorMap",
    "normalMap",
    "specularMap",
    "detailMap",
    "occlusionMap",
    "glossMap",
    "colorMap0",
    "colorMap1",
    "colorMap2",
    "colorMap3",
    "detailNormalMap",
    "specularColorMap",
    "alphaMap",
    "colorMapAlt",
    "normalMap1",
    "specularMap1",
    "glossMap1",
    "detailMap1",
    "colorMap01",
    "colorMap02",
    "colorMap03",
    "detailMapA",
    "heatMap",
    "cucolorMap",
];

/// An image's pixel format: from its pack IWI or its zone load def.
fn image_format(packs: &PackSet, image: &ImageRef) -> String {
    match packs.locate(image) {
        Some(ImageSource::Pack(i, entry)) => match packs.packs[i].read(entry).and_then(|b| {
            ipak_t6::parse_iwi(&b)
                .map_err(|e| e.to_string())
                .map(|iwi| format!("{:?}", iwi.format))
        }) {
            Ok(f) => f,
            Err(e) => format!("unreadable ({e})"),
        },
        Some(ImageSource::Embedded) => format!(
            "in zone, dxgi {}",
            image.embedded.as_ref().map_or(0, |e| e.dxgi_format)
        ),
        Some(ImageSource::Empty) => "no pixels".to_owned(),
        None => "missing".to_owned(),
    }
}

fn main() -> ExitCode {
    let path = std::env::args()
        .nth(1)
        .map_or_else(|| PathBuf::from(DEFAULT_ZONE), PathBuf::from);
    match run(&path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("t6mat: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(path: &Path) -> Result<(), String> {
    let capture: ZoneCapture = capture_zone(path).map_err(|e| e.to_string())?;
    // T6MAT_ANIM=<substring>: animations whose name contains it: frames,
    // rate, length and the root translation keys.
    if let Ok(want) = std::env::var("T6MAT_ANIM") {
        for a in capture.xanims.iter().filter(|a| a.name.contains(&want)) {
            let secs = f32::from(a.numframes) / a.framerate.max(0.001);
            println!(
                "xanim {} frames {} rate {} = {:.3} s loop {} delta {} keys {}",
                a.name,
                a.numframes,
                a.framerate,
                secs,
                a.looping,
                a.delta,
                a.delta_trans.len()
            );
            for (f, p) in &a.delta_trans {
                println!(
                    "   frame {f:>3} t {:.3}s  [{:.2}, {:.2}, {:.2}]",
                    f32::from(*f) / a.framerate.max(0.001),
                    p[0],
                    p[1],
                    p[2]
                );
            }
        }
        if capture.world.is_none() {
            return Ok(());
        }
    }
    // T6MAT_FOG: the createart script's volumetric fog.
    if std::env::var("T6MAT_FOG").is_ok() {
        for (name, bytes) in &capture.raw_files {
            if name.contains("createart") {
                println!("{name}: {:?}", asset_t6::parse_art_fog(bytes));
            }
        }
        if capture.world.is_none() {
            return Ok(());
        }
    }
    // T6MAT_MAT=<substring>: materials whose name contains it: technique
    // set, sort key, textures (slot, image, size) and per-slot draw states.
    if let Ok(want) = std::env::var("T6MAT_MAT") {
        let names: BTreeMap<u32, &str> = SLOT_NAMES.iter().map(|n| (hash(n), *n)).collect();
        // T6MAT_MAT_PNG=<dir>: each texture's top level written there as RGBA.
        let png_dir = std::env::var("T6MAT_MAT_PNG").ok();
        let packs = PackSet::open_dir(path.parent().ok_or("zone has no folder")?)?;
        for m in capture.materials.iter().filter(|m| m.name.contains(&want)) {
            let ts = m.technique_set.map_or("-".to_owned(), |t| {
                capture.technique_sets[t.index].name.clone()
            });
            println!(
                "material {} techset {ts} sort {} flags {:#x} state_flags {:#x}",
                m.name, m.sort_key, m.surface_flags, m.state_flags
            );
            for t in &m.textures {
                let slot = names.get(&t.name_hash).copied().unwrap_or("?");
                let img = t.image.map(|k| &capture.images[k.index]);
                println!(
                    "   tex {slot:<14} {:#010x} sem {} samp {:#04x} {}",
                    t.name_hash,
                    t.semantic,
                    t.sampler_state,
                    img.map_or("-".to_owned(), |i| format!(
                        "{} {}x{} embedded {}",
                        i.name,
                        i.width,
                        i.height,
                        i.embedded.is_some()
                    ))
                );
                if let (Some(dir), Some(img)) = (&png_dir, img)
                    && let Some(ImageSource::Pack(pi, entry)) = packs.locate(img)
                    && let Ok(bytes) = packs.packs[pi].read(entry)
                    && let Ok(iwi) = ipak_t6::parse_iwi(&bytes)
                {
                    let (w, h) = (iwi.width as usize, iwi.height as usize);
                    let data = iwi.level(0);
                    let mut rgba = vec![0u8; w * h * 4];
                    let block = match iwi.format {
                        ipak_t6::IwiFormat::Dxt1 => 8,
                        ipak_t6::IwiFormat::Dxt3
                        | ipak_t6::IwiFormat::Dxt5
                        | ipak_t6::IwiFormat::Dxn => 16,
                        _ => 0,
                    };
                    if block == 0 {
                        let n = rgba.len().min(data.len());
                        rgba[..n].copy_from_slice(&data[..n]);
                    } else {
                        let (bw, bh) = (w.div_ceil(4), h.div_ceil(4));
                        for by in 0..bh {
                            for bx in 0..bw {
                                let o = (by * bw + bx) * block;
                                let Some(src) = data.get(o..o + block) else {
                                    continue;
                                };
                                let mut tile = [0u8; 64];
                                match iwi.format {
                                    ipak_t6::IwiFormat::Dxt1 => bcdec_rs::bc1(src, &mut tile, 16),
                                    ipak_t6::IwiFormat::Dxt3 => bcdec_rs::bc2(src, &mut tile, 16),
                                    ipak_t6::IwiFormat::Dxt5 => bcdec_rs::bc3(src, &mut tile, 16),
                                    _ => {
                                        let mut rg = [0u8; 32];
                                        bcdec_rs::bc5(src, &mut rg, 8, false);
                                        for k in 0..16 {
                                            tile[k * 4] = rg[k * 2];
                                            tile[k * 4 + 1] = rg[k * 2 + 1];
                                            tile[k * 4 + 3] = 255;
                                        }
                                    }
                                }
                                for ty in 0..4 {
                                    for tx in 0..4 {
                                        let (x, y) = (bx * 4 + tx, by * 4 + ty);
                                        if x < w && y < h {
                                            let d = (y * w + x) * 4;
                                            let t = (ty * 4 + tx) * 4;
                                            rgba[d..d + 4].copy_from_slice(&tile[t..t + 4]);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    let flat: String = img
                        .name
                        .chars()
                        .map(|c| {
                            if c.is_ascii_alphanumeric() || c == '_' {
                                c
                            } else {
                                '_'
                            }
                        })
                        .collect();
                    let out = Path::new(dir).join(format!("{flat}.png"));
                    let _ = std::fs::create_dir_all(dir);
                    if let Ok(file) = std::fs::File::create(&out) {
                        let mut enc =
                            png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
                        enc.set_color(png::ColorType::Rgba);
                        enc.set_depth(png::BitDepth::Eight);
                        if let Ok(mut wr) = enc.write_header() {
                            let _ = wr.write_image_data(&rgba);
                        }
                    }
                    println!("      -> {} ({:?})", out.display(), iwi.format);
                }
            }
            for (h, n, v) in &m.constants {
                println!("   const {n:<20} {h:#010x} {v:?}");
            }
            let mut seen = std::collections::BTreeSet::new();
            for (slot, &e) in m.state_bits_entry.iter().enumerate() {
                if e == 0xff || !seen.insert(e) {
                    continue;
                }
                if let Some(bits) = m.state_bits.get(usize::from(e)) {
                    println!(
                        "   state slot {slot} entry {e}: {:?}",
                        asset_t6::DrawState::decode(*bits)
                    );
                }
            }
        }
        if capture.world.is_none() {
            return Ok(());
        }
    }
    // T6MAT_RAW=<dir>: list this zone's rawfiles and write them there.
    if let Ok(dir) = std::env::var("T6MAT_RAW") {
        for (name, bytes) in &capture.raw_files {
            println!("rawfile {name} ({} bytes)", bytes.len());
            if !dir.is_empty() {
                let flat: String = name
                    .chars()
                    .map(|c| {
                        if matches!(c, '/' | '\\' | ':') {
                            '_'
                        } else {
                            c
                        }
                    })
                    .collect();
                let out = Path::new(&dir).join(flat);
                if let Err(e) =
                    std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&out, bytes))
                {
                    println!("  write {}: {e}", out.display());
                }
            }
        }
        if capture.world.is_none() {
            return Ok(());
        }
    }
    let world = capture.world.as_ref().ok_or("no GfxWorld")?;
    let packs = PackSet::open_dir(path.parent().ok_or("zone has no folder")?)?;
    let names: BTreeMap<u32, &str> = SLOT_NAMES.iter().map(|n| (hash(n), *n)).collect();

    // T6MAT_TINTS=1: materials without layers (no b1/m1.. token) whose
    // surfaces' vertex colours are far from white (any channel under 200):
    // what a plain shader would tint with them.
    if std::env::var_os("T6MAT_TINTS").is_some() {
        let mut out: BTreeMap<String, (usize, [u8; 4], String)> = BTreeMap::new();
        for srf in &world.surfaces {
            let Some(k) = srf.material else { continue };
            let m = &capture.materials[k.index];
            let t = m.technique_set.map_or(String::new(), |t| {
                capture.technique_sets[t.index].name.clone()
            });
            let layered = t.split('_').any(|w| {
                let b = w.as_bytes();
                b.len() >= 3
                    && matches!(b[0], b'b' | b'm')
                    && (b'1'..=b'3').contains(&b[1])
                    && b[2] == b'c'
            });
            // T6MAT_TINTS=multiply: instead, the materials with multiply
            // layers but no blend layer (drawn tinted before fix list 2).
            let multiply_only = !t.split('_').any(|w| w.starts_with("b1"))
                && t.split('_').any(|w| {
                    let b = w.as_bytes();
                    b.len() >= 3 && b[0] == b'm' && (b'1'..=b'3').contains(&b[1]) && b[2] == b'c'
                });
            let want_multiply = std::env::var("T6MAT_TINTS").is_ok_and(|v| v == "multiply");
            if (want_multiply && !multiply_only) || (!want_multiply && layered) {
                continue;
            }
            let first = srf.base_index as usize;
            let n = usize::from(srf.tri_count) * 3;
            let mut lo = [255u8; 4];
            for &idx in world.indices.get(first..first + n).unwrap_or(&[]) {
                let at = srf.vertex_data_offset0 as usize + usize::from(idx) * 36 + 16;
                if let Some(c) = world.vd0.get(at..at + 4) {
                    for q in 0..4 {
                        lo[q] = lo[q].min(c[q]);
                    }
                }
            }
            if lo[..3].iter().any(|&c| c < 200) {
                let e = out
                    .entry(m.name.clone())
                    .or_insert((0, [255; 4], t.clone()));
                e.0 += 1;
                for q in 0..4 {
                    e.1[q] = e.1[q].min(lo[q]);
                }
            }
        }
        for (name, (n, lo, t)) in &out {
            println!("tinted {name} [{t}]: {n} surfaces, colour min {lo:?}");
        }
        return Ok(());
    }
    // T6MAT_NEAR=x,y,z,r: every world surface whose bounds come within r of
    // the point: material, technique set, lightmap, triangles and its
    // vertices' colour range (vertex record @16, R G B A bytes).
    if let Ok(spec) = std::env::var("T6MAT_NEAR") {
        let v: Vec<f32> = spec
            .split(',')
            .filter_map(|x| x.trim().parse().ok())
            .collect();
        if let [x, y, z, r] = v[..] {
            for (si, srf) in world.surfaces.iter().enumerate() {
                let [lo, hi] = srf.bounds;
                let p = [x, y, z];
                let d2: f32 = (0..3)
                    .map(|i| {
                        let c = p[i].clamp(lo[i], hi[i]);
                        (p[i] - c) * (p[i] - c)
                    })
                    .sum();
                if d2 > r * r {
                    continue;
                }
                let (mname, tname) =
                    srf.material
                        .map_or(("(none)".to_owned(), String::new()), |k| {
                            let m = &capture.materials[k.index];
                            let t = m.technique_set.map_or(String::new(), |t| {
                                capture.technique_sets[t.index].name.clone()
                            });
                            (m.name.clone(), t)
                        });
                let first = srf.base_index as usize;
                let n = usize::from(srf.tri_count) * 3;
                let (mut clo, mut chi) = ([255u8; 4], [0u8; 4]);
                for &idx in world.indices.get(first..first + n).unwrap_or(&[]) {
                    let at = srf.vertex_data_offset0 as usize + usize::from(idx) * 36 + 16;
                    if let Some(c) = world.vd0.get(at..at + 4) {
                        for k in 0..4 {
                            clo[k] = clo[k].min(c[k]);
                            chi[k] = chi[k].max(c[k]);
                        }
                    }
                }
                println!(
                    "near surf {si}: {mname} [{tname}] lightmap {} tris {} colour min {clo:?} max {chi:?} bounds {:?}",
                    srf.lightmap_index, srf.tri_count, srf.bounds
                );
            }
        }
        return Ok(());
    }

    let mut by_material: BTreeMap<usize, (usize, usize)> = BTreeMap::new();
    let mut no_material = 0usize;
    for s in &world.surfaces {
        match s.material {
            Some(k) => {
                let e = by_material.entry(k.index).or_default();
                e.0 += 1;
                e.1 += usize::from(s.tri_count);
            }
            None => no_material += 1,
        }
    }
    let total_tris: usize = by_material.values().map(|v| v.1).sum();
    println!(
        "{} surfaces ({} without material), {} materials, {total_tris} triangles",
        world.surfaces.len(),
        no_material,
        by_material.len()
    );

    let mut techsets: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut vert_formats: BTreeMap<u8, usize> = BTreeMap::new();
    let mut slots: BTreeMap<(String, u8), usize> = BTreeMap::new();
    let mut color_formats: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut no_color: Vec<(String, usize)> = Vec::new();
    for (&mi, &(_, tris)) in &by_material {
        let m = &capture.materials[mi];
        let ts = m.technique_set.map(|k| &capture.technique_sets[k.index]);
        let ts_name = ts.map_or("(none)".to_owned(), |t| t.name.clone());
        let prefix = ts_name.split('_').take(2).collect::<Vec<_>>().join("_");
        let e = techsets.entry(prefix).or_default();
        e.0 += 1;
        e.1 += tris;
        *vert_formats
            .entry(ts.map_or(255, |t| t.world_vert_format))
            .or_default() += tris;
        let mut color = None;
        for t in &m.textures {
            let name = names
                .get(&t.name_hash)
                .map_or_else(|| format!("{:#010x}", t.name_hash), |n| (*n).to_owned());
            *slots.entry((name, t.semantic)).or_default() += 1;
            if t.name_hash == hash("colorMap") {
                color = t.image;
            }
        }
        match color {
            Some(k) => {
                let f = image_format(&packs, &capture.images[k.index]);
                let e = color_formats.entry(f).or_default();
                e.0 += 1;
                e.1 += tris;
            }
            None => no_color.push((m.name.clone(), tris)),
        }
    }
    // Materials on very large surfaces, and every material outside the lit
    // families: what draws across the sky or blends.
    let mut big: BTreeMap<usize, (f32, usize)> = BTreeMap::new();
    for srf in &world.surfaces {
        let Some(k) = srf.material else { continue };
        let ext = (0..3)
            .map(|i| srf.bounds[1][i] - srf.bounds[0][i])
            .fold(0f32, f32::max);
        let e = big.entry(k.index).or_insert((0.0, 0));
        e.0 = e.0.max(ext);
        e.1 += usize::from(srf.tri_count);
    }
    let mut list: Vec<_> = big.into_iter().collect();
    list.sort_by(|a, b| b.1.0.total_cmp(&a.1.0));
    println!("materials by largest surface extent:");
    for (mi, (ext, tris)) in list.iter().take(12) {
        let m = &capture.materials[*mi];
        let ts = m.technique_set.map_or("(none)".to_owned(), |k| {
            capture.technique_sets[k.index].name.clone()
        });
        println!(
            "   {ext:>8.0} units {tris:>6} tris  {}  [{ts}] sort {}",
            m.name, m.sort_key
        );
    }
    // Surfaces reaching high above the street: what can cover the sky.
    let mut high: BTreeMap<usize, (usize, f32, f32)> = BTreeMap::new();
    for srf in &world.surfaces {
        let Some(k) = srf.material else { continue };
        if srf.bounds[1][2] < 800.0 {
            continue;
        }
        let e = high.entry(k.index).or_insert((0, f32::MAX, f32::MIN));
        e.0 += usize::from(srf.tri_count);
        e.1 = e.1.min(srf.bounds[0][2]);
        e.2 = e.2.max(srf.bounds[1][2]);
    }
    println!("materials on surfaces reaching above z=800:");
    for (mi, (tris, lo, hi)) in &high {
        let m = &capture.materials[*mi];
        let ts = m.technique_set.map_or("(none)".to_owned(), |k| {
            capture.technique_sets[k.index].name.clone()
        });
        println!(
            "   {tris:>6} tris z {lo:.0}..{hi:.0}  {}  [{ts}] sort {}",
            m.name, m.sort_key
        );
    }
    // Rays from the east-view camera (t6_tex3): which surface is hit first.
    let eye = [116.0f32, -392.0, 20.0];
    for (yaw, pitch) in [
        (0.0f32, 10.0f32),
        (0.0, 20.0),
        (0.0, 30.0),
        (20.0, 25.0),
        (-20.0, 25.0),
    ] {
        let (cy, sy) = (yaw.to_radians().cos(), yaw.to_radians().sin());
        let (cp, sp) = (pitch.to_radians().cos(), pitch.to_radians().sin());
        let dir = [cy * cp, sy * cp, sp];
        let mut best: Option<(f32, usize)> = None;
        for (si, srf) in world.surfaces.iter().enumerate() {
            let base = srf.base_index as usize;
            for t in 0..usize::from(srf.tri_count) {
                let mut p = [[0f32; 3]; 3];
                let mut ok = true;
                for c in 0..3 {
                    let Some(&idx) = world.indices.get(base + t * 3 + c) else {
                        ok = false;
                        break;
                    };
                    let at = srf.vertex_data_offset0 as usize + usize::from(idx) * 36;
                    let Some(v) = world.vd0.get(at..at + 12) else {
                        ok = false;
                        break;
                    };
                    let f = |o: usize| f32::from_le_bytes([v[o], v[o + 1], v[o + 2], v[o + 3]]);
                    p[c] = [f(0), f(4), f(8)];
                }
                if !ok {
                    continue;
                }
                // Moller-Trumbore.
                let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
                let cross = |a: [f32; 3], b: [f32; 3]| {
                    [
                        a[1] * b[2] - a[2] * b[1],
                        a[2] * b[0] - a[0] * b[2],
                        a[0] * b[1] - a[1] * b[0],
                    ]
                };
                let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
                let e1 = sub(p[1], p[0]);
                let e2 = sub(p[2], p[0]);
                let h = cross(dir, e2);
                let a = dot(e1, h);
                if a.abs() < 1e-6 {
                    continue;
                }
                let f = 1.0 / a;
                let sv = sub(eye, p[0]);
                let u = f * dot(sv, h);
                if !(0.0..=1.0).contains(&u) {
                    continue;
                }
                let q = cross(sv, e1);
                let v = f * dot(dir, q);
                if v < 0.0 || u + v > 1.0 {
                    continue;
                }
                let dist = f * dot(e2, q);
                if dist > 1.0 && best.is_none_or(|(d, _)| dist < d) {
                    best = Some((dist, si));
                }
            }
        }
        match best {
            Some((d, si)) => {
                let srf = &world.surfaces[si];
                let m = srf.material.map_or("(none)".to_owned(), |k| {
                    capture.materials[k.index].name.clone()
                });
                println!(
                    "ray yaw {yaw} pitch {pitch}: surface {si} at {d:.0} units, {m}, vertex group {}, bounds {:?}",
                    srf.vertex_data_offset0, srf.bounds
                );
            }
            None => println!("ray yaw {yaw} pitch {pitch}: sky (no hit)"),
        }
    }
    println!("materials outside wpc_lit / lit_sm:");
    for (mi, (_, tris)) in &list {
        let m = &capture.materials[*mi];
        let ts = m.technique_set.map_or("(none)".to_owned(), |k| {
            capture.technique_sets[k.index].name.clone()
        });
        if ts.starts_with("wpc_lit") || ts.starts_with("lit_sm") {
            continue;
        }
        let tex: Vec<String> = m
            .textures
            .iter()
            .map(|t| {
                let n = names
                    .get(&t.name_hash)
                    .map_or_else(|| format!("{:#x}", t.name_hash), |n| (*n).to_owned());
                let img = t
                    .image
                    .map_or("-".to_owned(), |k| capture.images[k.index].name.clone());
                format!("{n}={img}")
            })
            .collect();
        println!(
            "   {tris:>6} tris  {}  [{ts}] sort {}  {}",
            m.name,
            m.sort_key,
            tex.join(" ")
        );
    }
    println!("technique set families (materials, triangles):");
    let mut ts: Vec<_> = techsets.into_iter().collect();
    ts.sort_by(|a, b| b.1.1.cmp(&a.1.1));
    for (name, (n, tris)) in ts.iter().take(20) {
        println!("   {tris:>7} tris  {n:>4} materials  {name}");
    }
    println!("world vertex formats by triangles: {vert_formats:?}");
    println!("texture slots over the materials (slot, semantic): count");
    let mut sl: Vec<_> = slots.into_iter().collect();
    sl.sort_by(|a, b| b.1.cmp(&a.1));
    for ((name, sem), n) in sl.iter().take(24) {
        println!("   {n:>5}  {name} semantic {sem}");
    }
    println!("colour map formats (materials, triangles):");
    for (f, (n, tris)) in &color_formats {
        println!("   {tris:>7} tris  {n:>4}  {f}");
    }
    no_color.sort_by(|a, b| b.1.cmp(&a.1));
    println!("materials with no colorMap: {}", no_color.len());
    for (name, tris) in no_color.iter().take(15) {
        println!("   {tris:>7} tris  {name}");
    }
    println!("lightmaps: {}", world.lightmaps.len());
    for (i, pair) in world.lightmaps.iter().enumerate() {
        let f: Vec<String> = pair
            .iter()
            .map(|k| {
                k.map_or("none".to_owned(), |k| {
                    let img = &capture.images[k.index];
                    format!(
                        "{} {}x{} {}",
                        img.name,
                        img.width,
                        img.height,
                        image_format(&packs, img)
                    )
                })
            })
            .collect();
        println!("   {i}: {}", f.join(" | "));
    }
    // Vertex colour bytes (+16, R G B A) over every corner.
    let mut sum = [0u64; 4];
    let mut n = 0u64;
    for srf in &world.surfaces {
        let base = srf.base_index as usize;
        for k in 0..usize::from(srf.tri_count) * 3 {
            let Some(&idx) = world.indices.get(base + k) else {
                continue;
            };
            let at = srf.vertex_data_offset0 as usize + usize::from(idx) * 36 + 16;
            let Some(c) = world.vd0.get(at..at + 4) else {
                continue;
            };
            for i in 0..4 {
                sum[i] += u64::from(c[i]);
            }
            n += 1;
        }
    }
    println!(
        "vertex colour mean (R G B A bytes): {:?}",
        sum.map(|v| v / n.max(1))
    );
    println!("sun: {:?}", world.sun);
    println!("exposure volumes: {:?}", world.exposure_volumes);
    println!("init world sun exposure: {}", world.init_sun_exposure);
    if let Some(g) = &world.light_grid {
        println!(
            "light grid: mins {:?} maxs {:?} offset {} row axis {} col axis {} rows {} raw {} entries {} colours {} sun light {}",
            g.mins,
            g.maxs,
            g.offset,
            g.row_axis,
            g.col_axis,
            g.row_data_start.len() / 2,
            g.raw_row_data.len(),
            g.entries.len() / 4,
            g.color_count,
            g.sun_primary_light_index
        );
        println!(
            "light grid coeff sets: {} ({} bytes)",
            g.coeff_count,
            g.coeffs.len()
        );
        for set in g.coeffs.chunks_exact(54).skip(20000).step_by(5000).take(6) {
            let v: Vec<String> = set
                .chunks_exact(2)
                .map(|b| format!("{:5}", u16::from_le_bytes([b[0], b[1]])))
                .collect();
            println!("   {}", v.join(" "));
        }
    }
    // The first lightmap's pixels: channel ranges, and a PNG when asked.
    if let Some(Some(k)) = world.lightmaps.first().map(|pair| pair[1])
        && let Some(e) = capture.images[k.index].embedded.as_ref()
    {
        let img = &capture.images[k.index];
        let (w, h) = (usize::from(img.width), usize::from(img.height));
        let px = &e.data[..(w * h * 4).min(e.data.len())];
        let mut lo = [255u8; 4];
        let mut hi = [0u8; 4];
        let mut sum = [0u64; 4];
        for c in px.chunks_exact(4) {
            for i in 0..4 {
                lo[i] = lo[i].min(c[i]);
                hi[i] = hi[i].max(c[i]);
                sum[i] += u64::from(c[i]);
            }
        }
        let n = (px.len() / 4).max(1) as u64;
        println!(
            "lightmap 0 secondary {w}x{h}, {} bytes ({} levels): channel min {lo:?} max {hi:?} mean {:?}",
            e.data.len(),
            e.level_count,
            sum.map(|s| s / n)
        );
        if let Ok(out) = std::env::var("T6MAT_LIGHTMAP_PNG")
            && let Ok(file) = std::fs::File::create(&out)
        {
            let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            let rgb: Vec<u8> = px
                .chunks_exact(4)
                .flat_map(|c| [c[0], c[1], c[2]])
                .collect();
            if let Ok(mut wr) = enc.write_header() {
                let _ = wr.write_image_data(&rgb);
            }
            let alpha_out = out.replace(".png", "_alpha.png");
            if let Ok(file) = std::fs::File::create(&alpha_out) {
                let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
                enc.set_color(png::ColorType::Grayscale);
                enc.set_depth(png::BitDepth::Eight);
                let a: Vec<u8> = px.chunks_exact(4).map(|c| c[3]).collect();
                if let Ok(mut wr) = enc.write_header() {
                    let _ = wr.write_image_data(&a);
                }
            }
        }
    }
    let mut lm: BTreeMap<i8, usize> = BTreeMap::new();
    for s in &world.surfaces {
        *lm.entry(s.lightmap_index).or_default() += 1;
    }
    println!("surfaces per lightmap index: {lm:?}");
    // Lightmap coordinates (half2 at +32 of each 36-byte vertex) per lightmap.
    let half = |bits: u16| -> f32 {
        let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
        let exp = i32::from((bits >> 10) & 0x1f);
        let frac = f32::from(bits & 0x3ff);
        sign * match exp {
            0 => frac * 2f32.powi(-24),
            31 => f32::INFINITY,
            _ => (1.0 + frac / 1024.0) * 2f32.powi(exp - 15),
        }
    };
    let mut ranges: BTreeMap<i8, ([f32; 2], [f32; 2], usize)> = BTreeMap::new();
    let mut tex_range = ([f32::MAX; 2], [f32::MIN; 2]);
    for srf in &world.surfaces {
        let e = ranges
            .entry(srf.lightmap_index)
            .or_insert(([f32::MAX; 2], [f32::MIN; 2], 0));
        let base = srf.base_index as usize;
        for k in 0..usize::from(srf.tri_count) * 3 {
            let Some(&idx) = world.indices.get(base + k) else {
                continue;
            };
            let at = srf.vertex_data_offset0 as usize + usize::from(idx) * 36;
            let Some(v) = world.vd0.get(at..at + 36) else {
                continue;
            };
            let uv = [
                half(u16::from_le_bytes([v[32], v[33]])),
                half(u16::from_le_bytes([v[34], v[35]])),
            ];
            let tc = [
                half(u16::from_le_bytes([v[20], v[21]])),
                half(u16::from_le_bytes([v[22], v[23]])),
            ];
            for i in 0..2 {
                e.0[i] = e.0[i].min(uv[i]);
                e.1[i] = e.1[i].max(uv[i]);
                tex_range.0[i] = tex_range.0[i].min(tc[i]);
                tex_range.1[i] = tex_range.1[i].max(tc[i]);
            }
            e.2 += 1;
        }
    }
    // Which 4-byte field is which: two halfs in 0..1, or a packed unit vector.
    for off in [20usize, 24, 28, 32] {
        let (mut unit_halfs, mut unit_vec, mut n) = (0usize, 0usize, 0usize);
        for srf in world.surfaces.iter().filter(|s| s.lightmap_index == 0) {
            let base = srf.base_index as usize;
            for k in 0..usize::from(srf.tri_count) * 3 {
                let Some(&idx) = world.indices.get(base + k) else {
                    continue;
                };
                let at = srf.vertex_data_offset0 as usize + usize::from(idx) * 36 + off;
                let Some(v) = world.vd0.get(at..at + 4) else {
                    continue;
                };
                n += 1;
                let h = [
                    half(u16::from_le_bytes([v[0], v[1]])),
                    half(u16::from_le_bytes([v[2], v[3]])),
                ];
                if h.iter().all(|x| (0.0..=1.0).contains(x)) {
                    unit_halfs += 1;
                }
                let w = u32::from_le_bytes([v[0], v[1], v[2], v[3]]);
                let comp = |shift: u32| {
                    let raw = ((w >> shift) & 0x3ff) as i32;
                    let signed = if raw >= 512 { raw - 1024 } else { raw };
                    signed as f32 / 511.0
                };
                let len = (comp(0).powi(2) + comp(10).powi(2) + comp(20).powi(2)).sqrt();
                if (len - 1.0).abs() < 0.02 {
                    unit_vec += 1;
                }
            }
        }
        println!(
            "vertex +{off}: {:.1}% two halfs in 0..1, {:.1}% unit 10:10:10 vectors ({n} corners)",
            100.0 * unit_halfs as f32 / n.max(1) as f32,
            100.0 * unit_vec as f32 / n.max(1) as f32
        );
    }
    // Lightmap texels per world unit per triangle, reading +32 three ways.
    let decoders: [(&str, fn(u16) -> f32); 3] = [
        ("unorm16", |b| f32::from(b) / 65535.0),
        ("snorm16", |b| f32::from(b as i16) / 32767.0),
        ("half", |b| {
            let sign = if b & 0x8000 != 0 { -1.0 } else { 1.0 };
            let exp = i32::from((b >> 10) & 0x1f);
            let frac = f32::from(b & 0x3ff);
            sign * match exp {
                0 => frac * 2f32.powi(-24),
                _ => (1.0 + frac / 1024.0) * 2f32.powi(exp - 15),
            }
        }),
    ];
    for (label, dec) in decoders {
        let mut ratios = Vec::new();
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for srf in world.surfaces.iter().filter(|s| s.lightmap_index == 0) {
            let base = srf.base_index as usize;
            for t in 0..usize::from(srf.tri_count) {
                let mut p = [[0f32; 3]; 3];
                let mut q = [[0f32; 2]; 3];
                let mut ok = true;
                for c in 0..3 {
                    let Some(&idx) = world.indices.get(base + t * 3 + c) else {
                        ok = false;
                        break;
                    };
                    let at = srf.vertex_data_offset0 as usize + usize::from(idx) * 36;
                    let Some(v) = world.vd0.get(at..at + 36) else {
                        ok = false;
                        break;
                    };
                    let f = |o: usize| f32::from_le_bytes([v[o], v[o + 1], v[o + 2], v[o + 3]]);
                    p[c] = [f(0), f(4), f(8)];
                    q[c] = [
                        dec(u16::from_le_bytes([v[32], v[33]])) * 2048.0,
                        dec(u16::from_le_bytes([v[34], v[35]])) * 3072.0,
                    ];
                    for i in 0..2 {
                        lo[i] = lo[i].min(q[c][i]);
                        hi[i] = hi[i].max(q[c][i]);
                    }
                }
                if !ok {
                    continue;
                }
                let e1 = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
                let e2 = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
                let cr = [
                    e1[1] * e2[2] - e1[2] * e2[1],
                    e1[2] * e2[0] - e1[0] * e2[2],
                    e1[0] * e2[1] - e1[1] * e2[0],
                ];
                let world_area = 0.5 * (cr[0] * cr[0] + cr[1] * cr[1] + cr[2] * cr[2]).sqrt();
                let uv_area = 0.5
                    * ((q[1][0] - q[0][0]) * (q[2][1] - q[0][1])
                        - (q[2][0] - q[0][0]) * (q[1][1] - q[0][1]))
                        .abs();
                if world_area > 1.0 && uv_area.is_finite() {
                    ratios.push((uv_area / world_area).sqrt());
                }
            }
        }
        ratios.sort_by(f32::total_cmp);
        let pct = |p: f32| {
            ratios
                .get(((ratios.len() as f32 - 1.0) * p) as usize)
                .copied()
                .unwrap_or(0.0)
        };
        println!(
            "+32 as {label}: texels/unit p10 {:.4} p50 {:.4} p90 {:.4}; texel range u {:.0}..{:.0} v {:.0}..{:.0}",
            pct(0.1),
            pct(0.5),
            pct(0.9),
            lo[0],
            hi[0],
            lo[1],
            hi[1]
        );
    }
    for (lm, (lo, hi, n)) in &ranges {
        println!(
            "lightmap {lm}: {n} corners, lightmap uv u {:.4}..{:.4} v {:.4}..{:.4}",
            lo[0], hi[0], lo[1], hi[1]
        );
    }
    println!(
        "texture uv over all corners: u {:.2}..{:.2} v {:.2}..{:.2}",
        tex_range.0[0], tex_range.1[0], tex_range.0[1], tex_range.1[1]
    );
    // Static models: placements, LOD0 geometry, vertices inside model bounds.
    let mut used: BTreeMap<usize, usize> = BTreeMap::new();
    for sm in &world.static_models {
        if let Some(k) = sm.model {
            *used.entry(k.index).or_default() += 1;
        }
    }
    let (mut tris, mut verts, mut inside, mut multi_bone, mut bone_lists) =
        (0usize, 0usize, 0usize, 0usize, 0usize);
    for (&mi, &n) in &used {
        let m = &capture.xmodels[mi];
        if m.num_bones > 1 {
            multi_bone += 1;
        }
        for srf in &m.lod0 {
            tris += n * srf.indices.len() / 3;
            bone_lists += srf.vert_lists.iter().filter(|l| l.0 != 0).count();
            for v in srf.verts.chunks_exact(32) {
                let f = |o: usize| f32::from_le_bytes([v[o], v[o + 1], v[o + 2], v[o + 3]]);
                let p = [f(0), f(4), f(8)];
                verts += 1;
                if (0..3).all(|i| p[i] >= m.mins[i] - 1.0 && p[i] <= m.maxs[i] + 1.0) {
                    inside += 1;
                }
            }
        }
    }
    println!(
        "static models: {} placed, {} distinct; {tris} placed LOD0 triangles; {verts} distinct-model vertices, {:.2}% inside model bounds; {multi_bone} models with >1 bone, {bone_lists} vertex lists on a non-root bone",
        world.static_models.len(),
        used.len(),
        100.0 * inside as f32 / verts.max(1) as f32
    );
    // Model vertex fields: +20 texcoord, +24 / +28 normal and tangent.
    {
        let (mut n, mut uv_unit, mut v24_1010, mut v28_1010, mut v24_u8) =
            (0usize, 0usize, 0usize, 0usize, 0usize);
        let unit1010 = |w: u32| {
            let c = |sh: u32| {
                let r = ((w >> sh) & 0x3ff) as i32;
                (if r >= 512 { r - 1024 } else { r }) as f32 / 511.0
            };
            ((c(0).powi(2) + c(10).powi(2) + c(20).powi(2)).sqrt() - 1.0).abs() < 0.02
        };
        let unit_u8 = |w: u32| {
            let b = w.to_le_bytes();
            let scale = (f32::from(b[3]) + 192.0) / 32_385.0;
            let v: Vec<f32> = (0..3).map(|i| (f32::from(b[i]) - 127.0) * scale).collect();
            ((v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt() - 1.0).abs() < 0.05
        };
        for &mi in used.keys() {
            for srf in &capture.xmodels[mi].lod0 {
                for v in srf.verts.chunks_exact(32) {
                    let w = |o: usize| u32::from_le_bytes([v[o], v[o + 1], v[o + 2], v[o + 3]]);
                    n += 1;
                    let t = w(20);
                    let lo = half(t as u16);
                    let hi = half((t >> 16) as u16);
                    if (0.0..=1.0).contains(&lo) && (0.0..=1.0).contains(&hi) {
                        uv_unit += 1;
                    }
                    if unit1010(w(24)) {
                        v24_1010 += 1;
                    }
                    if unit1010(w(28)) {
                        v28_1010 += 1;
                    }
                    if unit_u8(w(24)) {
                        v24_u8 += 1;
                    }
                }
            }
        }
        let pct = |k: usize| 100.0 * k as f32 / n.max(1) as f32;
        println!(
            "model vertices: {n}; +20 two halfs in 0..1 {:.1}%; +24 unit 10:10:10 {:.1}%, +28 unit 10:10:10 {:.1}%, +24 unit IW4 u8x4 {:.1}%",
            pct(uv_unit),
            pct(v24_1010),
            pct(v28_1010),
            pct(v24_u8)
        );
    }
    {
        let (mut with, mut matching, mut total) = (0usize, 0usize, 0usize);
        let mut sample = None;
        for sm in &world.static_models {
            total += 1;
            if sm.lmap_colors.is_empty() {
                continue;
            }
            with += 1;
            let verts: usize = sm.model.map_or(0, |k| {
                capture.xmodels[k.index]
                    .lod0
                    .iter()
                    .map(|s| usize::from(s.vert_count))
                    .sum()
            });
            if verts == sm.lmap_colors.len() {
                matching += 1;
            }
            if sample.is_none() {
                sample = Some((
                    sm.lmap_colors.iter().take(6).copied().collect::<Vec<_>>(),
                    sm.lighting_sh,
                ));
            }
        }
        println!(
            "static model vertex light: {with} of {total} instances have LOD0 colours, {matching} with one per LOD0 vertex; sample {sample:x?}"
        );
        let nonzero_sh = world
            .static_models
            .iter()
            .filter(|m| m.lighting_sh.iter().any(|&v| v != 0))
            .count();
        println!("static models with nonzero lightingSH: {nonzero_sh}");
        // T6MAT_SMODEL=<name part>: those placements' baked vertex light.
        if let Ok(want) = std::env::var("T6MAT_SMODEL") {
            for sm in &world.static_models {
                let Some(k) = sm.model else { continue };
                let xm = &capture.xmodels[k.index];
                if !xm.name.contains(&want) {
                    continue;
                }
                let verts: usize = xm.lod0.iter().map(|s| usize::from(s.vert_count)).sum();
                let ch = |c: &u32, i: usize| c.to_le_bytes()[i] as f32;
                let n = sm.lmap_colors.len().max(1) as f32;
                let mean: Vec<f32> = (0..4)
                    .map(|i| sm.lmap_colors.iter().map(|c| ch(c, i)).sum::<f32>() / n)
                    .collect();
                let dark = sm
                    .lmap_colors
                    .iter()
                    .filter(|c| (0..3).map(|i| ch(c, i)).sum::<f32>() < 30.0)
                    .count();
                println!(
                    "smodel {} at {:?} scale {} verts {verts} colours {} mean {:?} dark {dark} sh {:?} colorsIndex {} probe {} light {}",
                    xm.name,
                    sm.origin,
                    sm.scale,
                    sm.lmap_colors.len(),
                    mean,
                    sm.lighting_sh,
                    sm.colors_index,
                    sm.reflection_probe_index,
                    sm.primary_light_index
                );
                for srf in &xm.lod0 {
                    let mat = srf.material.map(|m| &capture.materials[m.index]);
                    println!(
                        "   surf verts {} material {} techset {}",
                        srf.vert_count,
                        mat.map_or("-", |m| m.name.as_str()),
                        mat.and_then(|m| m.technique_set)
                            .map_or("-".to_owned(), |t| capture.technique_sets[t.index]
                                .name
                                .clone())
                    );
                }
                // The first colours, per surface, as RGBA bytes.
                let mut at = 0usize;
                for srf in &xm.lod0 {
                    let n = usize::from(srf.vert_count);
                    let part: Vec<[u8; 4]> = sm
                        .lmap_colors
                        .iter()
                        .skip(at)
                        .take(n.min(6))
                        .map(|c| c.to_le_bytes())
                        .collect();
                    println!("   colours from {at}: {part:?}");
                    at += n;
                }
            }
        }
        let mut fams: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        for sm in &world.static_models {
            let Some(k) = sm.model else { continue };
            for srf in &capture.xmodels[k.index].lod0 {
                let name = srf
                    .material
                    .and_then(|m| capture.materials[m.index].technique_set)
                    .map_or("(none)".to_owned(), |t| {
                        capture.technique_sets[t.index].name.clone()
                    });
                let e = fams.entry(name).or_default();
                e.0 += 1;
                if !sm.lmap_colors.is_empty() {
                    e.1 += 1;
                }
            }
        }
        let mut fl: Vec<_> = fams.into_iter().collect();
        fl.sort_by(|a, b| b.1.0.cmp(&a.1.0));
        println!("static model surface technique sets (placed surfaces, with vertex light):");
        for (name, (n, lit)) in fl.iter().take(15) {
            println!("   {n:>5} {lit:>5}  {name}");
        }
    }
    if let Some(sky) = capture
        .xmodels
        .iter()
        .find(|m| m.name == world.sky_box_model)
    {
        for srf in &sky.lod0 {
            let Some(mk) = srf.material else { continue };
            let m = &capture.materials[mk.index];
            let ts = m.technique_set.map_or("(none)".to_owned(), |k| {
                capture.technique_sets[k.index].name.clone()
            });
            let tex: Vec<String> = m
                .textures
                .iter()
                .map(|t| {
                    let img = t.image.map(|k| &capture.images[k.index]);
                    format!(
                        "{:#x}/sem{}={}",
                        t.name_hash,
                        t.semantic,
                        img.map_or("-".to_owned(), |i| format!(
                            "{} {}x{} map{} {}",
                            i.name,
                            i.width,
                            i.height,
                            i.map_type,
                            image_format(&packs, i)
                        ))
                    )
                })
                .collect();
            println!(
                "sky surface: {} verts {} tris, material {} [{ts}] sort {}: {}",
                srf.vert_count,
                srf.indices.len() / 3,
                m.name,
                m.sort_key,
                tex.join(", ")
            );
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for v in srf.verts.chunks_exact(32) {
                for i in 0..3 {
                    let f =
                        f32::from_le_bytes([v[i * 4], v[i * 4 + 1], v[i * 4 + 2], v[i * 4 + 3]]);
                    lo[i] = lo[i].min(f);
                    hi[i] = hi[i].max(f);
                }
            }
            println!("sky bounds {lo:?} .. {hi:?}");
            println!("sky material constants: {:?}", m.constants);
        }
    }
    for (si, srf) in world.surfaces.iter().enumerate() {
        let Some(k) = srf.material else { continue };
        let m = &capture.materials[k.index];
        if m.name.contains("light_orange") || m.name.contains("caulk") {
            println!(
                "surface {si}: {} surfaceFlags {:#x} contents {:#x} gfx flags {:#x} sort {} tris {} lightmap {} constants {:?}",
                m.name,
                m.surface_flags,
                m.contents,
                srf.flags,
                m.sort_key,
                srf.tri_count,
                srf.lightmap_index,
                m.constants
            );
        }
    }
    {
        let mut flags: BTreeMap<u8, (usize, String)> = BTreeMap::new();
        for srf in &world.surfaces {
            let e = flags.entry(srf.flags).or_insert((0, String::new()));
            e.0 += 1;
            if e.1.is_empty() {
                e.1 = srf
                    .material
                    .map_or(String::new(), |k| capture.materials[k.index].name.clone());
            }
        }
        println!(
            "gfx surface flags: {:?}",
            flags
                .iter()
                .map(|(f, (n, m))| format!("{f:#x}:{n} ({m})"))
                .collect::<Vec<_>>()
        );
    }
    println!(
        "sky dyn intensity {:?}; lut volumes {:?}",
        world.sky_dyn_intensity, world.lut_volumes
    );
    if let Some(k) = world.lut_material {
        let m = &capture.materials[k.index];
        let ts = m.technique_set.map_or("(none)".to_owned(), |t| {
            capture.technique_sets[t.index].name.clone()
        });
        for t in &m.textures {
            let img = t.image.map(|k| &capture.images[k.index]);
            println!(
                "lut material {} [{ts}]: slot {:#x} sem {} -> {}",
                m.name,
                t.name_hash,
                t.semantic,
                img.map_or("-".to_owned(), |i| format!(
                    "{} {}x{}x{} map{} {}",
                    i.name,
                    i.width,
                    i.height,
                    i.depth,
                    i.map_type,
                    image_format(&packs, i)
                ))
            );
        }
        println!("lut constants {:?}", m.constants);
        if let Ok(out) = std::env::var("T6MAT_LUT_PNG")
            && let Some(img) = m
                .textures
                .first()
                .and_then(|t| t.image)
                .map(|k| &capture.images[k.index])
            && let Some(ImageSource::Pack(i, entry)) = packs.locate(img)
            && let Ok(bytes) = packs.packs[i].read(entry)
            && let Ok(iwi) = ipak_t6::parse_iwi(&bytes)
        {
            let px = iwi.level(0);
            println!(
                "lut iwi {:?} {}x{} levels {} bytes {}",
                iwi.format,
                iwi.width,
                iwi.height,
                iwi.levels,
                px.len()
            );
            if let Ok(file) = std::fs::File::create(&out) {
                let mut enc =
                    png::Encoder::new(std::io::BufWriter::new(file), iwi.width, iwi.height);
                enc.set_color(png::ColorType::Rgba);
                enc.set_depth(png::BitDepth::Eight);
                if let Ok(mut wr) = enc.write_header() {
                    let _ = wr.write_image_data(px);
                }
            }
            for x in [0usize, 63, 64, 127, 960, 1023] {
                let o = x * 4;
                println!("lut row0 x{x}: {:?}", &px[o..o + 4]);
            }
            for y in [0usize, 63] {
                let o = (y * iwi.width as usize) * 4;
                println!("lut y{y} x0: {:?}", &px[o..o + 4]);
            }
        }
    }
    if std::env::var("T6MAT_TREES").is_ok() {
        for sm in world.static_models.iter() {
            let Some(k) = sm.model else { continue };
            let m = &capture.xmodels[k.index];
            let want = std::env::var("T6MAT_TREES").unwrap_or_default();
            // `T6MAT_TREES=*`: every placed model.
            if want == "*"
                || (want.len() > 1 && m.name.contains(&want))
                || (want.len() <= 1
                    && (m.name.contains("tree")
                        || m.name.contains("foliage")
                        || m.name.contains("bush")))
            {
                let mats: Vec<String> = m
                    .lod0
                    .iter()
                    .filter_map(|s| s.material)
                    .map(|mk| {
                        let mat = &capture.materials[mk.index];
                        let ts = mat.technique_set.map_or("-".to_owned(), |t| {
                            capture.technique_sets[t.index].name.clone()
                        });
                        format!("{} [{ts}]", mat.name)
                    })
                    .collect();
                println!("tree {} at {:?}: {}", m.name, sm.origin, mats.join(", "));
            }
        }
    }
    // T6MAT_PROBES: the reflection probes: origin, cube image, format.
    if std::env::var("T6MAT_PROBES").is_ok() {
        println!("reflection probes {}", world.reflection_probes.len());
        for (i, p) in world.reflection_probes.iter().enumerate() {
            let img = p.image.map(|k| &capture.images[k.index]);
            println!(
                "probe {i}: origin {:?} sh {:?} lod bias {} image {}",
                p.origin,
                p.lighting_sh,
                p.mip_lod_bias,
                img.map_or("-".to_owned(), |i| format!(
                    "{} {}x{} map{} levels {} {}",
                    i.name,
                    i.width,
                    i.height,
                    i.map_type,
                    i.level_count,
                    image_format(&packs, i)
                ))
            );
        }
        if let Some(e) = world
            .reflection_probes
            .get(1)
            .and_then(|p| p.image)
            .and_then(|k| capture.images[k.index].embedded.as_ref())
        {
            println!(
                "probe 1 embedded: dxgi {} levels {} flags {:#x} bytes {}",
                e.dxgi_format,
                e.level_count,
                e.flags,
                e.data.len()
            );
        }
        let mut per: BTreeMap<i8, usize> = BTreeMap::new();
        for srf in &world.surfaces {
            *per.entry(srf.reflection_probe_index).or_default() += 1;
        }
        println!("surfaces per probe {per:?}");
    }
    if std::env::var("T6MAT_TECHSETS").is_ok() {
        let mut fam: BTreeMap<String, usize> = BTreeMap::new();
        for t in &capture.technique_sets {
            let stem = t
                .name
                .rsplit_once('_')
                .map_or(t.name.as_str(), |(a, _)| a)
                .to_owned();
            *fam.entry(stem).or_default() += 1;
        }
        for (f, n) in fam {
            println!("techset {n:>3} {f}");
        }
    }
    let sky = capture
        .xmodels
        .iter()
        .find(|m| m.name == world.sky_box_model);
    if let Some(m) = sky {
        for srf in &m.lod0 {
            if let Some(k) = srf.material {
                let mat = &capture.materials[k.index];
                println!("sky material {} constants {:?}", mat.name, mat.constants);
            }
        }
    }
    println!(
        "sky: {} ({})",
        world.sky_box_model,
        sky.map_or("not in this zone".to_owned(), |m| format!(
            "{} LOD0 surfaces, {} triangles",
            m.lod0.len(),
            m.lod0.iter().map(|s| s.indices.len() / 3).sum::<usize>()
        ))
    );
    if std::env::var("T6MAT_STATES").is_ok() {
        states_report(&capture, world);
    }
    // T6MAT_VD1: the second vertex stream's bytes per vertex, by the
    // technique set's world vertex format (from the gaps between groups),
    // and its first records decoded as half2 UV + 10:10:10 vector.
    if std::env::var("T6MAT_VD1").is_ok() {
        let mut groups: BTreeMap<i32, (u32, u8, usize)> = BTreeMap::new();
        for (i, srf) in world.surfaces.iter().enumerate() {
            let vf = srf
                .material
                .and_then(|k| capture.materials[k.index].technique_set)
                .map_or(255, |k| capture.technique_sets[k.index].world_vert_format);
            let need = {
                let first = srf.base_index as usize;
                let n = usize::from(srf.tri_count) * 3;
                world.indices[first..first + n]
                    .iter()
                    .map(|&x| u32::from(x) + 1)
                    .max()
                    .unwrap_or(0)
            };
            let e = groups.entry(srf.vertex_data_offset1).or_insert((0, vf, i));
            e.0 = e.0.max(need);
            if e.1 != vf {
                e.1 = 254;
            }
        }
        let keys: Vec<i32> = groups.keys().copied().collect();
        let mut per: BTreeMap<u8, BTreeMap<String, usize>> = BTreeMap::new();
        for (j, k) in keys.iter().enumerate() {
            let (verts, vf, _) = groups[k];
            let next = keys.get(j + 1).copied().unwrap_or(world.vd1.len() as i32);
            let gap = next - k;
            let bpv = if verts > 0 {
                format!("{:.2}", f64::from(gap) / f64::from(verts))
            } else {
                "-".into()
            };
            *per.entry(vf).or_default().entry(bpv).or_default() += 1;
        }
        println!(
            "vd1 {} bytes, {} groups; bytes per vertex by world vertex format: {per:?}",
            world.vd1.len(),
            keys.len()
        );
        // Per technique set: measured bytes per vertex against the rule
        // "each extra layer: half2 UV, plus 4 bytes when it has a normal map".
        let mut by_set: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
        for (j, k) in keys.iter().enumerate() {
            let (verts, _, si) = groups[k];
            if verts == 0 {
                continue;
            }
            let next = keys.get(j + 1).copied().unwrap_or(world.vd1.len() as i32);
            let bpv = f64::from(next - k) / f64::from(verts);
            let srf = &world.surfaces[si];
            let set = srf
                .material
                .and_then(|m| capture.materials[m.index].technique_set)
                .map_or("-".to_owned(), |t| {
                    capture.technique_sets[t.index].name.clone()
                });
            *by_set
                .entry(set)
                .or_default()
                .entry(format!("{bpv:.2}"))
                .or_default() += 1;
        }
        for (set, m) in &by_set {
            let mut predicted = 0;
            for tok in set.split('_') {
                let b = tok.as_bytes();
                if b.len() >= 2 && matches!(b[0], b'b' | b'm') && (b'1'..=b'3').contains(&b[1]) {
                    predicted += 4;
                    if tok.contains(&format!("n{}", b[1] as char)) {
                        predicted += 4;
                    }
                }
            }
            println!("   vd1 {set}: predicted {predicted}, measured {m:?}");
        }
        for vf in [1u8, 2, 3, 4, 5, 7] {
            let Some((k, (verts, _, si))) = groups.iter().find(|(_, g)| g.1 == vf && g.0 > 3)
            else {
                continue;
            };
            let srf = &world.surfaces[*si];
            let name = srf
                .material
                .map_or("-".to_owned(), |m| capture.materials[m.index].name.clone());
            println!("vf {vf}: group vd1 {k:#x}, {verts} verts, surface {si} {name}");
            for v in 0..3usize {
                for stride in [8usize, 16, 24] {
                    let at = *k as usize + v * stride;
                    let Some(r) = world.vd1.get(at..at + stride) else {
                        continue;
                    };
                    let words: Vec<String> = r
                        .chunks_exact(4)
                        .map(|c| {
                            let w = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                            format!(
                                "[{:.3} {:.3} | {:08x}]",
                                half_f32(w as u16),
                                half_f32((w >> 16) as u16),
                                w
                            )
                        })
                        .collect();
                    println!("   v{v} stride {stride}: {}", words.join(" "));
                }
            }
        }
    }
    // T6MAT_SKYGRID: the light grid's sky volumes, and how many static
    // models fall outside the grid / inside a sky volume.
    if std::env::var("T6MAT_SKYGRID").is_ok()
        && let Some(g) = world.light_grid.as_ref()
    {
        println!("sky grid volumes: {}", g.sky_volumes.len());
        for v in g.sky_volumes.iter().take(20) {
            println!("   {:?}", v);
        }
        let inside = |p: [f32; 3], v: &asset_t6::SkyGridVolumeRef| {
            (0..3).all(|a| p[a] >= v.mins[a] && p[a] <= v.maxs[a])
        };
        let in_sky = world
            .static_models
            .iter()
            .filter(|s| g.sky_volumes.iter().any(|v| inside(s.origin, v)))
            .count();
        println!(
            "static models inside a sky volume: {in_sky} of {}",
            world.static_models.len()
        );
    }
    // T6MAT_WORLDFOG: the map's own fog (initial and per volume).
    if std::env::var("T6MAT_WORLDFOG").is_ok() {
        println!("init world fog: {:?}", world.init_fog);
        for (mins, maxs, control, ex, fog) in &world.fog_volumes {
            println!("fog volume {mins:?}..{maxs:?} control {control:#x} ex {ex:#x}: {fog:?}");
        }
    }
    // T6MAT_CULL: the static models' cull distances.
    if std::env::var("T6MAT_CULL").is_ok() {
        let mut d: Vec<f32> = world.static_models.iter().map(|s| s.cull_dist).collect();
        d.sort_by(f32::total_cmp);
        let pick = |q: f32| {
            d.get(((d.len() as f32 - 1.0) * q) as usize)
                .copied()
                .unwrap_or(0.0)
        };
        println!(
            "cull dist over {} placements: zero {}, min {:.0} p10 {:.0} p50 {:.0} p90 {:.0} max {:.0}",
            d.len(),
            d.iter().filter(|&&v| v <= 0.0).count(),
            pick(0.0),
            pick(0.1),
            pick(0.5),
            pick(0.9),
            pick(1.0)
        );
    }
    // T6MAT_SIZES: each drawn colour map's size in the zone against the
    // size of the pack image we decode, and how many images are streamed.
    if std::env::var("T6MAT_SIZES").is_ok() {
        let mut wanted: std::collections::BTreeSet<usize> = world
            .surfaces
            .iter()
            .filter_map(|s| s.material.map(|k| k.index))
            .collect();
        for sm in &world.static_models {
            if let Some(k) = sm.model {
                for srf in &capture.xmodels[k.index].lod0 {
                    if let Some(m) = srf.material {
                        wanted.insert(m.index);
                    }
                }
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        let (mut same, mut smaller, mut other) = (0, 0, 0);
        let mut by_size: BTreeMap<(u32, u32), usize> = BTreeMap::new();
        for mi in wanted {
            let m = &capture.materials[mi];
            for t in &m.textures {
                let Some(k) = t.image else { continue };
                if t.semantic != 2 || !seen.insert(k.index) {
                    continue;
                }
                let img = &capture.images[k.index];
                let iwi = match packs.locate(img) {
                    Some(ImageSource::Pack(i, e)) => packs.packs[i].read(e).ok().and_then(|b| {
                        ipak_t6::parse_iwi(&b)
                            .ok()
                            .map(|w| (w.width, w.height, w.levels, packs.packs[i].name.clone()))
                    }),
                    _ => None,
                };
                match &iwi {
                    Some((w, h, _, _))
                        if *w == u32::from(img.width) && *h == u32::from(img.height) =>
                    {
                        same += 1
                    }
                    Some((w, _, _, _)) if *w < u32::from(img.width) => smaller += 1,
                    _ => other += 1,
                }
                if let Some((w, h, _, _)) = iwi {
                    *by_size.entry((w, h)).or_default() += 1;
                }
                if seen.len() <= 25 {
                    println!(
                        "image {} zone {}x{} levels {} parts {} -> pack {:?}",
                        img.name, img.width, img.height, img.level_count, img.streamed_parts, iwi
                    );
                }
            }
        }
        println!("colour images: {same} full size, {smaller} smaller in the pack, {other} other");
        println!("decoded sizes: {by_size:?}");
    }
    Ok(())
}

/// A draw state in a few words.
fn state_words(st: &asset_t6::DrawState) -> String {
    const BLEND: [&str; 11] = [
        "-", "0", "1", "sc", "1-sc", "sa", "1-sa", "da", "1-da", "dc", "1-dc",
    ];
    let b = |v: u8| BLEND.get(usize::from(v)).copied().unwrap_or("?");
    let mut w = Vec::new();
    if st.blend_op == 0 {
        w.push("opaque".to_owned());
    } else {
        w.push(format!(
            "blend {}*src op{} {}*dst",
            b(st.src_blend),
            st.blend_op,
            b(st.dst_blend)
        ));
    }
    if let Some(a) = st.alpha_test {
        w.push(if a == 1 { "atest>=128" } else { "atest>0" }.to_owned());
    }
    w.push(
        match st.cull {
            1 => "cull none",
            2 => "cull back",
            3 => "cull front",
            _ => "cull ?",
        }
        .to_owned(),
    );
    if !st.color_write_rgb {
        w.push("no rgb write".to_owned());
    }
    w.push(
        if st.depth_write {
            "zwrite"
        } else {
            "no zwrite"
        }
        .to_owned(),
    );
    match st.depth_test {
        None => w.push("no ztest".to_owned()),
        Some(t) if t != 3 => w.push(format!("ztest {t}")),
        _ => {}
    }
    if st.polygon_offset != 0 {
        w.push(format!("offset {}", st.polygon_offset));
    }
    w.join(", ")
}

/// bo2zm: what the game itself says about drawing: each material's draw
/// states, the world's visibility lists, the primary lights, mirrored
/// static model placements.
fn states_report(capture: &ZoneCapture, world: &asset_t6::WorldRef) {
    let d = &world.dpvs;
    println!(
        "dpvs: {} static surfaces of {}; lit {:?}, lit trans {:?}, emissive opaque {:?}, emissive trans {:?}; sorted index {} entries; {} cells",
        d.static_surface_count,
        world.surfaces.len(),
        d.lit,
        d.lit_trans,
        d.emissive_opaque,
        d.emissive_trans,
        world.sorted_surf_index.len(),
        world.cells.len()
    );
    // Where each surface sits in draw order, and whether a cell draws it.
    let mut position = vec![usize::MAX; world.surfaces.len()];
    for (i, &s) in world.sorted_surf_index.iter().enumerate() {
        if let Some(p) = position.get_mut(usize::from(s)) {
            *p = i;
        }
    }
    let mut in_cell = vec![false; world.sorted_surf_index.len()];
    let mut smodel_in_cell = vec![false; world.static_models.len()];
    for trees in &world.cells {
        for t in trees {
            for i in 0..usize::from(t.surface_count) {
                if let Some(c) = in_cell.get_mut(usize::from(t.start_surf_index) + i) {
                    *c = true;
                }
            }
            for &m in &t.smodel_indexes {
                if let Some(c) = smodel_in_cell.get_mut(usize::from(m)) {
                    *c = true;
                }
            }
        }
    }
    println!(
        "cells draw {} of {} sorted surfaces, {} of {} static models",
        in_cell.iter().filter(|&&c| c).count(),
        in_cell.len(),
        smodel_in_cell.iter().filter(|&&c| c).count(),
        smodel_in_cell.len()
    );
    let range_of = |p: usize| -> &'static str {
        let p = p as u32;
        let inr = |r: (u32, u32)| p >= r.0 && p < r.1;
        if inr(d.lit) {
            "lit"
        } else if inr(d.lit_trans) {
            "lit trans"
        } else if inr(d.emissive_opaque) {
            "emissive opaque"
        } else if inr(d.emissive_trans) {
            "emissive trans"
        } else {
            "no list"
        }
    };
    // Per material: its surfaces' lists and cell coverage.
    let mut per_mat: BTreeMap<usize, BTreeMap<String, usize>> = BTreeMap::new();
    for (si, srf) in world.surfaces.iter().enumerate() {
        let Some(k) = srf.material else { continue };
        let p = position[si];
        let key = if p == usize::MAX {
            "not sorted".to_owned()
        } else {
            format!(
                "{}{}",
                range_of(p),
                if in_cell[p] { "" } else { " (no cell)" }
            )
        };
        *per_mat.entry(k.index).or_default().entry(key).or_default() += usize::from(srf.tri_count);
    }
    println!("world materials: lists (triangles) | technique slots with a state | main state");
    for (mi, lists) in &per_mat {
        let m = &capture.materials[*mi];
        let ts = m.technique_set.map(|k| &capture.technique_sets[k.index]);
        let slots: Vec<String> = ts
            .map(|t| {
                t.techniques
                    .iter()
                    .enumerate()
                    .filter(|(_, x)| x.is_some())
                    .map(|(i, _)| format!("{i}:{}", m.state_bits_entry[i]))
                    .collect()
            })
            .unwrap_or_default();
        let main = [6usize, 5, 4, 3, 2]
            .iter()
            .find_map(|&t| m.draw_state(t).map(|s| (t, s)));
        println!(
            "  {} [{}] sort {} lists {:?} | {} | {}",
            m.name,
            ts.map_or("-", |t| t.name.as_str()),
            m.sort_key,
            lists,
            slots.join(" "),
            main.map_or("none".to_owned(), |(t, s)| format!(
                "t{t}: {}",
                state_words(&s)
            ))
        );
    }
    // Static model materials: grouped by technique set and main state.
    let mut groups: BTreeMap<(String, String), (usize, Vec<String>)> = BTreeMap::new();
    for sm in &world.static_models {
        let Some(k) = sm.model else { continue };
        for srf in &capture.xmodels[k.index].lod0 {
            let Some(mk) = srf.material else { continue };
            let m = &capture.materials[mk.index];
            let ts = m.technique_set.map_or("-".to_owned(), |t| {
                capture.technique_sets[t.index].name.clone()
            });
            let main = [6usize, 5, 4, 3, 2]
                .iter()
                .find_map(|&t| m.draw_state(t).map(|s| (t, s)))
                .map_or("none".to_owned(), |(t, s)| {
                    format!("t{t}: {}", state_words(&s))
                });
            let e = groups.entry((ts, main)).or_default();
            e.0 += 1;
            if e.1.len() < 4 && !e.1.contains(&m.name) {
                e.1.push(m.name.clone());
            }
        }
    }
    println!(
        "static model surfaces by technique set and main state (placed surfaces, sample materials):"
    );
    for ((ts, main), (n, names)) in &groups {
        println!("  {n:>5} [{ts}] {main} | {}", names.join(", "));
    }
    // Mirrored placements: an axis basis with a negative determinant.
    let det = |a: &[[f32; 3]; 3]| {
        a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
            - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
            + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0])
    };
    let mirrored: Vec<&asset_t6::StaticModel> = world
        .static_models
        .iter()
        .filter(|s| det(&s.axis) < 0.0)
        .collect();
    let mut dets: Vec<f32> = world.static_models.iter().map(|s| det(&s.axis)).collect();
    dets.sort_by(f32::total_cmp);
    println!(
        "static models: {} mirrored (axis det < 0) of {}; det min {:.3} median {:.3} max {:.3}; scale min {:.3} max {:.3}",
        mirrored.len(),
        world.static_models.len(),
        dets.first().copied().unwrap_or(0.0),
        dets.get(dets.len() / 2).copied().unwrap_or(0.0),
        dets.last().copied().unwrap_or(0.0),
        world
            .static_models
            .iter()
            .map(|s| s.scale)
            .fold(f32::MAX, f32::min),
        world
            .static_models
            .iter()
            .map(|s| s.scale)
            .fold(f32::MIN, f32::max),
    );
    let mut mirrored_names: BTreeMap<String, usize> = BTreeMap::new();
    for s in &mirrored {
        if let Some(k) = s.model {
            *mirrored_names
                .entry(capture.xmodels[k.index].name.clone())
                .or_default() += 1;
        }
    }
    for (n, c) in mirrored_names.iter().take(40) {
        println!("  mirrored {c:>3} {n}");
    }
    // Primary lights.
    let mut by_type: BTreeMap<u8, usize> = BTreeMap::new();
    for p in &capture.primary_lights {
        *by_type.entry(p.ty).or_default() += 1;
    }
    println!(
        "primary lights: {} in ComWorld (GfxWorld says {}), by type {:?}",
        capture.primary_lights.len(),
        world.primary_light_count,
        by_type
    );
    for (i, p) in capture.primary_lights.iter().enumerate() {
        println!(
            "  {i:>3} type {} shadow {} origin [{:.0}, {:.0}, {:.0}] radius {:.0} colour [{:.2}, {:.2}, {:.2}] diffuse {:?} dir [{:.2}, {:.2}, {:.2}] fov cos {:.3}/{:.3} datt {:.3} falloff {:?} aAbB {:?} angle {:?}",
            p.ty,
            p.can_use_shadow_map,
            p.origin[0],
            p.origin[1],
            p.origin[2],
            p.radius,
            p.color[0],
            p.color[1],
            p.color[2],
            p.diffuse_color,
            p.dir[0],
            p.dir[1],
            p.dir[2],
            p.cos_half_fov_outer,
            p.cos_half_fov_inner,
            p.d_attenuation,
            p.falloff,
            p.a_ab_b,
            p.angle
        );
    }
}
