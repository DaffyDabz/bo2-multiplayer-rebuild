//! bo2zm: measure a Black Ops II map's world geometry, read-only.
//! `t6world [zone.ff]` (default Nuketown) captures the GfxWorld and reports
//! vertex streams, surfaces, materials and static models, and tests where
//! positions sit inside a world vertex: every surface's vertices must fall
//! inside that surface's own bounds.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use asset_t6::{WorldRef, capture_zone};

const DEFAULT_ZONE: &str =
    r"C:\Program Files (x86)\Steam\steamapps\common\Call of Duty Black Ops II\zone\all\zm_nuked.ff";

fn f32_at(b: &[u8], at: usize) -> Option<f32> {
    b.get(at..at + 4)
        .map(|s| f32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn main() -> ExitCode {
    let path = std::env::args()
        .nth(1)
        .map_or_else(|| PathBuf::from(DEFAULT_ZONE), PathBuf::from);
    let capture = match capture_zone(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("t6world: {e}");
            return ExitCode::FAILURE;
        }
    };
    let Some(world) = capture.world.as_ref() else {
        eprintln!("t6world: {} has no GfxWorld", path.display());
        return ExitCode::FAILURE;
    };
    report(&capture, world);
    if let Some(c) = &capture.clip {
        let sided = c.brushes.iter().filter(|b| !b.sides.is_empty()).count();
        println!(
            "clip {}: {} brushes ({} with extra sides), {} materials, {} verts, {} tris, {} partitions, {} aabb nodes, {} static models",
            c.name,
            c.brushes.len(),
            sided,
            c.materials.len(),
            c.verts.len(),
            c.tri_indices.len() / 3,
            c.partitions.len(),
            c.aabb_trees.len(),
            c.static_model_count
        );
    }
    for (i, ents) in capture.map_ents.iter().enumerate() {
        println!("map ents {i}: {} chars", ents.len());
        let mut classes: BTreeMap<&str, usize> = BTreeMap::new();
        for line in ents.lines() {
            if let Some(rest) = line.trim().strip_prefix("\"classname\" ") {
                *classes.entry(rest.trim_matches('"')).or_default() += 1;
            }
        }
        for (k, n) in classes {
            println!("   {n:>5}  {k}");
        }
        // T6WORLD_ENTS=<text>: print every entity block that contains it.
        if let Ok(want) = std::env::var("T6WORLD_ENTS") {
            let mut block = String::new();
            for line in ents.lines() {
                block.push_str(line);
                block.push_str("\n");
                if line.trim() == "}" {
                    if block.contains(&want) {
                        println!("{block}");
                    }
                    block.clear();
                }
            }
        }
    }
    ExitCode::SUCCESS
}

fn report(capture: &asset_t6::ZoneCapture, w: &WorldRef) {
    println!(
        "world {}: bounds {:?} .. {:?}; sky model `{}`",
        w.name, w.mins, w.maxs, w.sky_box_model
    );
    println!(
        "vertices {}: vd0 {} bytes ({:.3} per vertex), vd1 {} bytes ({:.3} per vertex); indices {}",
        w.vertex_count,
        w.vd0.len(),
        w.vd0.len() as f64 / w.vertex_count.max(1) as f64,
        w.vd1.len(),
        w.vd1.len() as f64 / w.vertex_count.max(1) as f64,
        w.indices.len()
    );
    let tris: usize = w.surfaces.iter().map(|s| s.tri_count as usize).sum();
    let with_mat = w.surfaces.iter().filter(|s| s.material.is_some()).count();
    println!(
        "surfaces {} ({} triangles), {} with a resolved material; static models {} ({} resolved); lightmaps {}",
        w.surfaces.len(),
        tris,
        with_mat,
        w.static_models.len(),
        w.static_models.iter().filter(|m| m.model.is_some()).count(),
        w.lightmaps.len()
    );
    println!(
        "capture: {} materials, {} techsets, {} xmodels, {} images; unresolved asset refs {}",
        capture.materials.len(),
        capture.technique_sets.len(),
        capture.xmodels.len(),
        capture.images.len(),
        capture.unresolved_refs
    );

    // vd0: which stride and position offset put every surface's vertices
    // inside its bounds?
    let s = &w.surfaces;
    for stride in [12usize, 16, 20, 24, 28, 32, 36, 40, 44, 48] {
        for pos_off in [0usize, 4, 8, 12] {
            if pos_off + 12 > stride {
                continue;
            }
            let (mut inside, mut total) = (0usize, 0usize);
            for surf in s.iter().take(400) {
                for v in 0..surf.vertex_count as usize {
                    let base = surf.first_vertex as usize + v;
                    let at = base * stride + pos_off;
                    let (Some(x), Some(y), Some(z)) = (
                        f32_at(&w.vd0, at),
                        f32_at(&w.vd0, at + 4),
                        f32_at(&w.vd0, at + 8),
                    ) else {
                        continue;
                    };
                    total += 1;
                    let eps = 0.5;
                    if (0..3).all(|i| {
                        let c = [x, y, z][i];
                        c >= surf.mins[i] - eps && c <= surf.maxs[i] + eps
                    }) {
                        inside += 1;
                    }
                }
            }
            if total > 0 && inside * 100 / total >= 50 {
                println!(
                    "vd0 stride {stride} position at +{pos_off}: {inside}/{total} vertices inside their surface bounds (first_vertex indexing)"
                );
            }
        }
    }
    // Same test, addressing by vertexDataOffset0 instead of firstVertex.
    for pos_off in [0usize, 4, 8, 12] {
        let (mut inside, mut total) = (0usize, 0usize);
        for surf in s.iter().take(400) {
            let at = surf.vertex_data_offset0 as usize + pos_off;
            let (Some(x), Some(y), Some(z)) = (
                f32_at(&w.vd0, at),
                f32_at(&w.vd0, at + 4),
                f32_at(&w.vd0, at + 8),
            ) else {
                continue;
            };
            total += 1;
            if (0..3).all(|i| {
                let c = [x, y, z][i];
                c >= surf.mins[i] - 0.5 && c <= surf.maxs[i] + 0.5
            }) {
                inside += 1;
            }
        }
        if total > 0 {
            println!(
                "vd0 by vertexDataOffset0, position at +{pos_off}: {inside}/{total} first vertices inside bounds"
            );
        }
    }
    let mut offs: BTreeMap<i32, (usize, i64, i64)> = BTreeMap::new();
    for surf in s {
        let e = offs
            .entry(surf.vertex_data_offset0)
            .or_insert((0, i64::MAX, 0));
        e.0 += 1;
        e.1 = e.1.min(surf.first_vertex as i64);
        e.2 = e.2.max(surf.first_vertex as i64 + surf.vertex_count as i64);
    }
    println!("vertexDataOffset0 groups: {}", offs.len());
    for (off, (n, lo, hi)) in offs.iter().take(12) {
        println!("   offset {off:>9}: {n:>5} surfaces, firstVertex {lo}..{hi}");
    }
    for (i, surf) in s.iter().enumerate().take(8) {
        println!(
            "   surf {i}: off0 {} off1 {} first {} n {} tris {} base {} flags {:#x} lmap {}",
            surf.vertex_data_offset0,
            surf.vertex_data_offset1,
            surf.first_vertex,
            surf.vertex_count,
            surf.tri_count,
            surf.base_index,
            surf.flags,
            surf.lightmap_index
        );
    }
    // Hypothesis: vertex = vd0[vertexDataOffset0 + index * stride], index
    // from the surface's own triangles (u16, relative to its group).
    for stride in [32usize, 36, 40] {
        for pos_off in [0usize, 4, 8, 12, 16, 20, 24] {
            if pos_off + 12 > stride {
                continue;
            }
            let (mut inside, mut total, mut out_of_range) = (0usize, 0usize, 0usize);
            let mut zero_bounds = 0usize;
            for surf in s {
                let (lo, hi) = (surf.bounds[0], surf.bounds[1]);
                if lo == [0.0; 3] && hi == [0.0; 3] {
                    zero_bounds += 1;
                    continue;
                }
                let first = surf.base_index as usize;
                let count = surf.tri_count as usize * 3;
                for &idx in w.indices.get(first..first + count).unwrap_or(&[]) {
                    let at = surf.vertex_data_offset0 as usize + idx as usize * stride + pos_off;
                    let (Some(x), Some(y), Some(z)) = (
                        f32_at(&w.vd0, at),
                        f32_at(&w.vd0, at + 4),
                        f32_at(&w.vd0, at + 8),
                    ) else {
                        out_of_range += 1;
                        continue;
                    };
                    total += 1;
                    if (0..3).all(|i| {
                        let c = [x, y, z][i];
                        c >= lo[i] - 0.5 && c <= hi[i] + 0.5
                    }) {
                        inside += 1;
                    }
                }
            }
            if total > 0 && inside * 100 / total >= 20 {
                println!(
                    "vd0 group-relative, stride {stride}, position at +{pos_off}: {inside}/{total} triangle corners inside GfxSurface bounds ({out_of_range} past the end, {zero_bounds} surfaces without bounds)"
                );
            }
        }
    }
    // Which packed field is the normal, and how is it encoded? Compare each
    // candidate decoding with the face normal of the triangle it belongs to.
    type Dec = fn(u32) -> [f32; 3];
    fn sext10(v: u32) -> f32 {
        let v = (v & 0x3ff) as i32;
        let v = if v >= 512 { v - 1024 } else { v };
        v as f32 / 511.0
    }
    let decoders: [(&str, Dec); 9] = [
        ("10:10:10 signed zyx", |p| {
            [sext10(p >> 20), sext10(p >> 10), sext10(p)]
        }),
        ("octahedral snorm16", |p| {
            let x = (p & 0xffff) as u16 as i16 as f32 / 32767.0;
            let y = (p >> 16) as u16 as i16 as f32 / 32767.0;
            let z = 1.0 - x.abs() - y.abs();
            let (x, y) = if z < 0.0 {
                ((1.0 - y.abs()) * x.signum(), (1.0 - x.abs()) * y.signum())
            } else {
                (x, y)
            };
            let l = (x * x + y * y + z * z).sqrt().max(1e-9);
            [x / l, y / l, z / l]
        }),
        ("octahedral unorm16", |p| {
            let x = (p & 0xffff) as f32 / 65535.0 * 2.0 - 1.0;
            let y = (p >> 16) as f32 / 65535.0 * 2.0 - 1.0;
            let z = 1.0 - x.abs() - y.abs();
            let (x, y) = if z < 0.0 {
                ((1.0 - y.abs()) * x.signum(), (1.0 - x.abs()) * y.signum())
            } else {
                (x, y)
            };
            let l = (x * x + y * y + z * z).sqrt().max(1e-9);
            [x / l, y / l, z / l]
        }),
        ("11:11:10 signed", |p| {
            let s11 = |v: u32| {
                let v = (v & 0x7ff) as i32;
                (if v >= 1024 { v - 2048 } else { v }) as f32 / 1023.0
            };
            let s10 = |v: u32| sext10(v);
            [s11(p), s11(p >> 11), s10(p >> 22)]
        }),
        ("iw4 bytes+scale", |p| {
            let b = p.to_le_bytes();
            let sc = (b[3] as f32 + 192.0) / 32385.0;
            [
                (b[0] as f32 - 127.0) * sc,
                (b[1] as f32 - 127.0) * sc,
                (b[2] as f32 - 127.0) * sc,
            ]
        }),
        ("10:10:10 signed", |p| {
            [sext10(p), sext10(p >> 10), sext10(p >> 20)]
        }),
        ("10:10:10 unsigned", |p| {
            let f = |v: u32| (v & 0x3ff) as f32 / 1023.0 * 2.0 - 1.0;
            [f(p), f(p >> 10), f(p >> 20)]
        }),
        ("8:8:8 snorm", |p| {
            let b = p.to_le_bytes();
            [
                b[0] as i8 as f32 / 127.0,
                b[1] as i8 as f32 / 127.0,
                b[2] as i8 as f32 / 127.0,
            ]
        }),
        ("8:8:8 unorm", |p| {
            let b = p.to_le_bytes();
            [
                b[0] as f32 / 127.5 - 1.0,
                b[1] as f32 / 127.5 - 1.0,
                b[2] as f32 / 127.5 - 1.0,
            ]
        }),
    ];
    let read_pos = |at: usize| -> [f32; 3] {
        [
            f32_at(&w.vd0, at).unwrap_or(0.0),
            f32_at(&w.vd0, at + 4).unwrap_or(0.0),
            f32_at(&w.vd0, at + 8).unwrap_or(0.0),
        ]
    };
    for field in [20usize, 24, 28, 32] {
        for (name, dec) in decoders {
            let (mut n, mut sum_dot, mut sum_abs, mut sum_len) = (0usize, 0f64, 0f64, 0f64);
            for surf in s.iter().take(3000) {
                let first = surf.base_index as usize;
                for t in 0..surf.tri_count as usize {
                    let idx = &w.indices[first + t * 3..first + t * 3 + 3];
                    let at = |i: u16| surf.vertex_data_offset0 as usize + i as usize * 36;
                    let (a, b, c) = (
                        read_pos(at(idx[0])),
                        read_pos(at(idx[1])),
                        read_pos(at(idx[2])),
                    );
                    let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                    let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                    let f = [
                        e1[1] * e2[2] - e1[2] * e2[1],
                        e1[2] * e2[0] - e1[0] * e2[2],
                        e1[0] * e2[1] - e1[1] * e2[0],
                    ];
                    let fl = (f[0] * f[0] + f[1] * f[1] + f[2] * f[2]).sqrt();
                    if fl < 1e-3 {
                        continue;
                    }
                    let f = [f[0] / fl, f[1] / fl, f[2] / fl];
                    let raw = u32::from_le_bytes(
                        w.vd0[at(idx[0]) + field..at(idx[0]) + field + 4]
                            .try_into()
                            .unwrap(),
                    );
                    let v = dec(raw);
                    let vl = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                    if vl < 1e-6 {
                        continue;
                    }
                    let d = (v[0] * f[0] + v[1] * f[1] + v[2] * f[2]) / vl;
                    n += 1;
                    sum_dot += d as f64;
                    sum_abs += d.abs() as f64;
                    sum_len += vl as f64;
                }
            }
            if n > 0 && (sum_abs / n as f64 > 0.8 || (sum_len / n as f64 - 1.0).abs() < 0.02) {
                println!(
                    "field +{field} as {name:<18}: mean dot with face normal {:+.3}, mean |dot| {:.3}, mean length {:.3} ({n} triangles)",
                    sum_dot / n as f64,
                    sum_abs / n as f64,
                    sum_len / n as f64
                );
            }
        }
    }
    // Which half of the texcoord pair at +20 is u? The tangent (+28) runs
    // along increasing u: compare it with dP/du solved per triangle.
    fn half(bits: u16) -> f32 {
        let sign = if bits & 0x8000 != 0 { -1.0f32 } else { 1.0 };
        let e = ((bits >> 10) & 0x1f) as i32;
        let m = (bits & 0x3ff) as f32;
        if e == 0 {
            sign * m * 2f32.powi(-24)
        } else if e == 31 {
            f32::NAN
        } else {
            sign * (1.0 + m / 1024.0) * 2f32.powi(e - 15)
        }
    }
    for (label, lo_is_u) in [("u = low half", true), ("u = high half", false)] {
        let (mut n, mut agree) = (0usize, 0f64);
        for surf in s.iter().take(3000) {
            let first = surf.base_index as usize;
            for t in 0..surf.tri_count as usize {
                let idx = &w.indices[first + t * 3..first + t * 3 + 3];
                let at = |i: u16| surf.vertex_data_offset0 as usize + i as usize * 36;
                let p: Vec<[f32; 3]> = idx.iter().map(|&i| read_pos(at(i))).collect();
                let uv: Vec<[f32; 2]> = idx
                    .iter()
                    .map(|&i| {
                        let raw =
                            u32::from_le_bytes(w.vd0[at(i) + 20..at(i) + 24].try_into().unwrap());
                        let (lo, hi) = (half(raw as u16), half((raw >> 16) as u16));
                        if lo_is_u { [lo, hi] } else { [hi, lo] }
                    })
                    .collect();
                let e1 = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
                let e2 = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
                let (du1, dv1) = (uv[1][0] - uv[0][0], uv[1][1] - uv[0][1]);
                let (du2, dv2) = (uv[2][0] - uv[0][0], uv[2][1] - uv[0][1]);
                let det = du1 * dv2 - du2 * dv1;
                if det.abs() < 1e-8 {
                    continue;
                }
                let r = 1.0 / det;
                let dpdu = [
                    (e1[0] * dv2 - e2[0] * dv1) * r,
                    (e1[1] * dv2 - e2[1] * dv1) * r,
                    (e1[2] * dv2 - e2[2] * dv1) * r,
                ];
                let l = (dpdu[0] * dpdu[0] + dpdu[1] * dpdu[1] + dpdu[2] * dpdu[2]).sqrt();
                if !l.is_finite() || l < 1e-6 {
                    continue;
                }
                let raw =
                    u32::from_le_bytes(w.vd0[at(idx[0]) + 28..at(idx[0]) + 32].try_into().unwrap());
                let tan = [sext10(raw), sext10(raw >> 10), sext10(raw >> 20)];
                let d = (tan[0] * dpdu[0] + tan[1] * dpdu[1] + tan[2] * dpdu[2]) / l;
                n += 1;
                agree += d as f64;
            }
        }
        println!(
            "texcoord {label}: mean cos(tangent, dP/du) {:+.3} over {n} triangles",
            agree / n.max(1) as f64
        );
    }
    for (si, surf) in [(0usize, &s[0]), (6, &s[6])] {
        println!(
            "surface {si}: bounds {:?} .. {:?}, group at vd0 {:#x}, vd1 {:#x}",
            surf.mins, surf.maxs, surf.vertex_data_offset0, surf.vertex_data_offset1
        );
        for v in 0..3usize {
            let at = surf.vertex_data_offset0 as usize + v * 36;
            let rec = &w.vd0[at..at + 36];
            let floats: Vec<String> = rec
                .chunks_exact(4)
                .map(|c| format!("{:.3}", f32::from_le_bytes([c[0], c[1], c[2], c[3]])))
                .collect();
            let hex: Vec<String> = rec
                .chunks_exact(4)
                .map(|c| format!("{:02x}{:02x}{:02x}{:02x}", c[0], c[1], c[2], c[3]))
                .collect();
            println!("   v{v} f32: {}", floats.join(" "));
            println!("   v{v} hex: {}", hex.join(" "));
            let at1 = surf.vertex_data_offset1 as usize + v * 8;
            if let Some(r1) = w.vd1.get(at1..at1 + 8) {
                println!("   v{v} vd1: {:02x?}", r1);
            }
        }
    }
    let first = &s[0];
    println!(
        "surface 0: first_vertex {} vertex_data_offset0 {} vertex_data_offset1 {} vertices {} tris {} base_index {}",
        first.first_vertex,
        first.vertex_data_offset0,
        first.vertex_data_offset1,
        first.vertex_count,
        first.tri_count,
        first.base_index
    );
    let mut sets: BTreeMap<String, usize> = BTreeMap::new();
    for surf in s {
        let name = surf
            .material
            .and_then(|k| capture.material(k))
            .and_then(|m| m.technique_set)
            .and_then(|k| capture.technique_sets.get(k.index))
            .map_or_else(
                || "?".to_owned(),
                |t| format!("{} (vf {})", t.name, t.world_vert_format),
            );
        *sets.entry(name).or_default() += 1;
    }
    println!("world surfaces by technique set: {}", sets.len());
    let mut sorted: Vec<_> = sets.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));
    for (name, n) in sorted.iter().take(25) {
        println!("   {n:>5}  {name}");
    }
}
