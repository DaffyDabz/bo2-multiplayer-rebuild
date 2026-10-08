//! bo2zm fix list 2: what the map's placed models (`clipMap_t`
//! `staticModelList`) give bullets and thrown things to hit: per model, how
//! many are placed, the instance and model contents, the collision LOD and
//! the collision surfaces (triangles, contents, surface type). Read-only.
//!
//! `t6props [zone.ff]` (default Nuketown). `T6PROPS_NEAR=x,y,z,r` lists
//! the placements within r units of a point.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

const DEFAULT_ZONE: &str =
    r"C:\Program Files (x86)\Steam\steamapps\common\Call of Duty Black Ops II\zone\all\zm_nuked.ff";

fn main() -> ExitCode {
    let path = std::env::args()
        .nth(1)
        .map_or_else(|| PathBuf::from(DEFAULT_ZONE), PathBuf::from);
    let capture = match asset_t6::capture_zone(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("t6props: {e}");
            return ExitCode::FAILURE;
        }
    };
    // T6PROPS_MODEL=<substring>: every model of the zone whose name contains
    // it: bounds, contents, collision LOD and collision surfaces.
    if let Ok(want) = std::env::var("T6PROPS_MODEL") {
        for m in capture
            .xmodels
            .iter()
            .filter(|m| m.name.contains(want.as_str()))
        {
            println!(
                "model {}: bounds {:?}..{:?} radius {} contents {:#x} coll lod {} surfs {}",
                m.name,
                m.mins,
                m.maxs,
                m.radius,
                m.contents,
                m.coll_lod,
                m.coll_surfs.len()
            );
            for c in &m.coll_surfs {
                println!(
                    "  coll surf: {} tris, {:?}..{:?}, contents {:#x}, surf flags {:#x}",
                    c.tris.len(),
                    c.mins,
                    c.maxs,
                    c.contents,
                    c.surf_flags
                );
            }
        }
        return ExitCode::SUCCESS;
    }
    // T6PROPS_MATFLAGS=<substring>: every material of the zone whose name
    // contains it (decals too), with its game flags and surface type bits.
    if let Ok(want) = std::env::var("T6PROPS_MATFLAGS") {
        for m in capture
            .materials
            .iter()
            .filter(|m| m.name.contains(want.as_str()))
        {
            println!(
                "material {}: game flags {:#x} type bits {:#x} surface flags {:#x} sort {}",
                m.name, m.game_flags, m.surface_type_bits, m.surface_flags, m.sort_key
            );
        }
        return ExitCode::SUCCESS;
    }
    let Some(clip) = capture.clip.as_ref() else {
        eprintln!("t6props: no clipMap");
        return ExitCode::FAILURE;
    };
    // T6PROPS_CMODEL=<n>[,<n>...]: those submodels' bounds and brush count.
    if let Ok(list) = std::env::var("T6PROPS_CMODEL") {
        for n in list
            .split(',')
            .filter_map(|v| v.trim().parse::<usize>().ok())
        {
            if let Some(m) = clip.cmodels.get(n) {
                println!(
                    "cmodel {n}: mins {:?} maxs {:?} radius {} leaf brush node {} shares map info {}",
                    m.mins, m.maxs, m.radius, m.leaf.leaf_brush_node, m.shares_map_info
                );
            }
        }
        println!("cmodels: {}", clip.cmodels.len());
        if let Some(world) = capture.world.as_ref() {
            let statics: std::collections::HashSet<u16> =
                world.sorted_surf_index.iter().copied().collect();
            println!(
                "gfx brush models: {}, static surfaces {} of {}",
                world.brush_models.len(),
                world.sorted_surf_index.len(),
                world.surfaces.len()
            );
            for n in list
                .split(',')
                .filter_map(|v| v.trim().parse::<usize>().ok())
            {
                if let Some((start, count, bounds)) = world.brush_models.get(n) {
                    let in_static = (*start..start + count)
                        .filter(|i| statics.contains(&(*i as u16)))
                        .count();
                    println!(
                        "gfx brush model {n}: surfaces {start}..{} ({count}), {in_static} in the static list, bounds {bounds:?}",
                        start + count
                    );
                }
            }
        }
    }
    println!(
        "clip static models: {} (header count {}), world static models: {}",
        clip.static_models.len(),
        clip.static_model_count,
        capture.world.as_ref().map_or(0, |w| w.static_models.len())
    );
    let near: Option<[f32; 4]> = std::env::var("T6PROPS_NEAR").ok().and_then(|v| {
        let n: Vec<f32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        (n.len() == 4).then(|| [n[0], n[1], n[2], n[3]])
    });
    // name -> (placed, instance contents set, model)
    let mut by_name: BTreeMap<String, (usize, Vec<u32>, Option<&asset_t6::XModelRef>)> =
        BTreeMap::new();
    let mut unresolved = 0;
    for sm in &clip.static_models {
        let model = sm.model.map(|k| &capture.xmodels[k.index]);
        if model.is_none() {
            unresolved += 1;
        }
        let name = model.map_or("<none>".to_owned(), |m| m.name.clone());
        if let Some([x, y, z, r]) = near {
            let d = ((sm.origin[0] - x).powi(2)
                + (sm.origin[1] - y).powi(2)
                + (sm.origin[2] - z).powi(2))
            .sqrt();
            if d <= r {
                println!(
                    "near: {name} at {:?} contents {:#x} abs {:?}..{:?} dist {d:.0}",
                    sm.origin.map(|v| v.round()),
                    sm.contents,
                    sm.absmin.map(|v| v.round()),
                    sm.absmax.map(|v| v.round())
                );
                // T6PROPS_EXACT=1: the exact place, the inverse axis and the
                // drawn mesh's own bounds (LOD 0 vertices, model space).
                if std::env::var_os("T6PROPS_EXACT").is_some() {
                    let mut lo = [f32::MAX; 3];
                    let mut hi = [f32::MIN; 3];
                    for s in model.map(|m| m.lod0.as_slice()).unwrap_or_default() {
                        for v in s.verts.chunks_exact(32) {
                            for (i, c) in v[..12].chunks_exact(4).enumerate() {
                                let f = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                                lo[i] = lo[i].min(f);
                                hi[i] = hi[i].max(f);
                            }
                        }
                    }
                    println!(
                        "      origin {:?} inv axis {:?} mesh {lo:?}..{hi:?}",
                        sm.origin, sm.inv_scaled_axis
                    );
                    // The faces in the middle (|x| < 20, |z| < 6): each
                    // vertex's y there, rounded to a tenth, with counts.
                    let mut ys: BTreeMap<i32, usize> = BTreeMap::new();
                    for s in model.map(|m| m.lod0.as_slice()).unwrap_or_default() {
                        for v in s.verts.chunks_exact(32) {
                            let f =
                                |o: usize| f32::from_le_bytes([v[o], v[o + 1], v[o + 2], v[o + 3]]);
                            if f(0).abs() < 20.0 && f(8).abs() < 6.0 {
                                *ys.entry((f(4) * 10.0).round() as i32).or_default() += 1;
                            }
                        }
                    }
                    println!("      middle vertex y (tenths: count) {ys:?}");
                    let mats: Vec<String> = model
                        .map(|m| {
                            m.materials
                                .iter()
                                .map(|k| {
                                    k.map_or("-".to_owned(), |k| {
                                        let mat = &capture.materials[k.index];
                                        let state = mat
                                            .state_bits
                                            .first()
                                            .map(|b| asset_t6::DrawState::decode(*b));
                                        format!(
                                            "{} sort {} state0 {state:?}",
                                            mat.name, mat.sort_key
                                        )
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    println!("      materials {mats:?}");
                    println!(
                        "      lods {:?}",
                        model.map(|m| m.lods.clone()).unwrap_or_default()
                    );
                    // T6PROPS_TRIS=1: every LOD 0 triangle (model space).
                    if std::env::var_os("T6PROPS_TRIS").is_some() {
                        for (si, s) in model
                            .map(|m| m.lod0.as_slice())
                            .unwrap_or_default()
                            .iter()
                            .enumerate()
                        {
                            let p = |i: u16| -> [f32; 3] {
                                let o = usize::from(i) * 32;
                                std::array::from_fn(|k| {
                                    let b = &s.verts[o + k * 4..o + k * 4 + 4];
                                    (f32::from_le_bytes([b[0], b[1], b[2], b[3]]) * 10.0).round()
                                        / 10.0
                                })
                            };
                            for t in s.indices.chunks_exact(3) {
                                println!(
                                    "      surf {si} tri {:?} {:?} {:?}",
                                    p(t[0]),
                                    p(t[1]),
                                    p(t[2])
                                );
                            }
                        }
                    }
                }
            }
        }
        let e = by_name.entry(name).or_insert((0, Vec::new(), model));
        e.0 += 1;
        if !e.1.contains(&sm.contents) {
            e.1.push(sm.contents);
        }
    }
    println!("unresolved model pointers: {unresolved}");
    let mut with_coll = 0;
    let mut placed_with_coll = 0;
    let mut placed = 0;
    for (name, (count, contents, model)) in &by_name {
        placed += count;
        let Some(m) = model else {
            println!("{name}: x{count} contents {contents:x?} NO MODEL");
            continue;
        };
        let tris: usize = m.coll_surfs.iter().map(|s| s.tris.len()).sum();
        if tris > 0 && m.coll_lod >= 0 {
            with_coll += 1;
            placed_with_coll += count;
        }
        let surfs: Vec<String> = m
            .coll_surfs
            .iter()
            .take(4)
            .map(|s| {
                format!(
                    "[{} tris c={:#x} sf={:#x} (type {}) bone {} {:?}..{:?}]",
                    s.tris.len(),
                    s.contents,
                    s.surf_flags,
                    (s.surf_flags >> 20) & 0x1f,
                    s.bone_idx,
                    s.mins.map(|v| (v * 10.0).round() / 10.0),
                    s.maxs.map(|v| (v * 10.0).round() / 10.0)
                )
            })
            .collect();
        println!(
            "{name}: x{count} inst contents {contents:x?} model contents {:#x} collLod {} collSurfs {} tris {tris} radius {:.0} {}",
            m.contents,
            m.coll_lod,
            m.coll_surfs.len(),
            m.radius,
            surfs.join(" ")
        );
    }
    println!(
        "models: {} placed kinds, {with_coll} with collision; placements {placed}, {placed_with_coll} with collision",
        by_name.len()
    );
    // T6PROPS_MAT=<substring>: world surfaces drawn with matching materials
    // (bounds, surface type) and the clip materials of that name.
    if let (Ok(want), Some(world)) = (std::env::var("T6PROPS_MAT"), capture.world.as_ref()) {
        let mut groups: BTreeMap<String, Vec<&asset_t6::WorldSurface>> = BTreeMap::new();
        for srf in &world.surfaces {
            let Some(k) = srf.material else { continue };
            let m = &capture.materials[k.index];
            if m.name.contains(want.as_str()) {
                groups.entry(m.name.clone()).or_default().push(srf);
            }
        }
        for (name, list) in &groups {
            let m = capture.materials.iter().find(|m| &m.name == name);
            println!(
                "world material {name}: {} surfaces, surface flags {:#x} (type {}) game flags {:#x} type bits {:#x}",
                list.len(),
                m.map_or(0, |m| m.surface_flags),
                m.map_or(0, |m| (m.surface_flags >> 20) & 0x1f),
                m.map_or(0, |m| m.game_flags),
                m.map_or(0, |m| m.surface_type_bits)
            );
            for srf in list.iter().take(12) {
                println!(
                    "   bounds {:?}..{:?} tris {}",
                    srf.bounds[0].map(|v| v.round()),
                    srf.bounds[1].map(|v| v.round()),
                    srf.tri_count
                );
            }
        }
        if let Some(clip) = capture.clip.as_ref() {
            for m in clip
                .materials
                .iter()
                .filter(|m| m.name.contains(want.as_str()))
            {
                println!(
                    "clip material {}: surface flags {:#x} (type {}) contents {:#x}",
                    m.name,
                    m.surface_flags,
                    (m.surface_flags >> 20) & 0x1f,
                    m.content_flags
                );
            }
        }
    }
    // T6PROPS_AT=x,y,z,r: world surfaces whose bounds come within r of a
    // point, with their material's game flags and surface type bits.
    if let (Ok(at), Some(world)) = (std::env::var("T6PROPS_AT"), capture.world.as_ref()) {
        let v: Vec<f32> = at
            .split(',')
            .filter_map(|x| x.trim().parse().ok())
            .collect();
        if let [x, y, z, r] = v[..] {
            for srf in &world.surfaces {
                let [lo, hi] = srf.bounds;
                let d2: f32 = [x, y, z]
                    .iter()
                    .enumerate()
                    .map(|(i, p)| (lo[i] - p).max(0.0).max(p - hi[i]).powi(2))
                    .sum();
                if d2 > r * r {
                    continue;
                }
                let Some(k) = srf.material else { continue };
                let m = &capture.materials[k.index];
                println!(
                    "surface at {:?}..{:?} tris {} material {} type {} game flags {:#x} type bits {:#x}",
                    lo.map(|v| v.round()),
                    hi.map(|v| v.round()),
                    srf.tri_count,
                    m.name,
                    (m.surface_flags >> 20) & 0x1f,
                    m.game_flags,
                    m.surface_type_bits
                );
            }
        }
    }
    ExitCode::SUCCESS
}
