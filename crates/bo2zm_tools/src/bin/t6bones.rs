//! bo2zm: print a T6 model's bones and an animation's tracks/notetracks.
//! `t6bones <zone.ff> <model or anim name>...`
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((zone, names)) = args.split_first() else {
        eprintln!("usage: t6bones <zone.ff> <name>...");
        return;
    };
    let c = asset_t6::capture_zone(std::path::Path::new(zone)).expect("capture");
    for name in names {
        if let Some(m) = c.xmodels.iter().find(|m| &m.name == name) {
            println!(
                "model {name}: {} bones ({} root), {} surfaces",
                m.bone_names.len(),
                m.num_root_bones,
                m.lod0.len()
            );
            for (i, b) in m.bone_names.iter().enumerate() {
                let parent = if i < usize::from(m.num_root_bones) {
                    -1
                } else {
                    let step = m
                        .parent_list
                        .get(i - usize::from(m.num_root_bones))
                        .copied()
                        .unwrap_or(0);
                    i as i32 - i32::from(step)
                };
                let (q, t) = m.base_mat.get(i).copied().unwrap_or_default();
                println!(
                    "  {i:3} {b:30} parent {parent:3} base q=({:.3},{:.3},{:.3},{:.3}) t=({:.2},{:.2},{:.2})",
                    q[0], q[1], q[2], q[3], t[0], t[1], t[2]
                );
            }
            for (k, t) in m.trans.iter().enumerate() {
                println!(
                    "  trans[{k}] (bone {}) = ({:.2},{:.2},{:.2}) quat {:?}",
                    k + usize::from(m.num_root_bones),
                    t[0],
                    t[1],
                    t[2],
                    m.quats.get(k)
                );
            }
            for (i, s) in m.lod0.iter().enumerate() {
                // T6BONES_VC=1: the surface's vertex colours (min/max/mean
                // per channel) and texcoord range (half2, u low), from each
                // 32-byte packed vertex.
                if std::env::var_os("T6BONES_VC").is_some() {
                    let half = |h: u16| -> f32 {
                        let e = i32::from((h >> 10) & 0x1f);
                        let f = f32::from(h & 0x3ff) / 1024.0;
                        let v = if e == 0 {
                            f * 2f32.powi(-14)
                        } else {
                            (1.0 + f) * 2f32.powi(e - 15)
                        };
                        if h & 0x8000 != 0 { -v } else { v }
                    };
                    let n = usize::from(s.vert_count);
                    let (mut lo, mut hi, mut sum) = ([255u8; 4], [0u8; 4], [0u32; 4]);
                    let (mut ulo, mut uhi) = ([f32::MAX; 2], [f32::MIN; 2]);
                    for v in s.verts.chunks_exact(32).take(n) {
                        for k in 0..4 {
                            lo[k] = lo[k].min(v[16 + k]);
                            hi[k] = hi[k].max(v[16 + k]);
                            sum[k] += u32::from(v[16 + k]);
                        }
                        let t = u32::from_le_bytes([v[20], v[21], v[22], v[23]]);
                        let uv = [half(t as u16), half((t >> 16) as u16)];
                        for k in 0..2 {
                            ulo[k] = ulo[k].min(uv[k]);
                            uhi[k] = uhi[k].max(uv[k]);
                        }
                    }
                    let mean: Vec<u32> = sum.iter().map(|x| x / n.max(1) as u32).collect();
                    println!(
                        "  surf {i}: vertex colour rgba min {lo:?} max {hi:?} mean {mean:?}; uv u {:.2}..{:.2} v {:.2}..{:.2}",
                        ulo[0], uhi[0], ulo[1], uhi[1]
                    );
                }
                println!(
                    "  surf {i}: part bits {:08x?} rigid lists (bone offset, verts, tri offset, tris) {:?}",
                    s.part_bits, s.vert_lists
                );
                println!(
                    "  surf {i}: verts {} tris {} rigid lists {} blend {:?} material {:?}",
                    s.vert_count,
                    s.indices.len() / 3,
                    s.vert_lists.len(),
                    s.blend_counts,
                    s.material.map(|k| c.materials[k.index].name.clone())
                );
            }
        }
        for m in c
            .materials
            .iter()
            .filter(|m| m.name.trim_start_matches(',') == name)
        {
            let tech = m
                .technique_set
                .map(|k| c.technique_sets[k.index].name.clone())
                .unwrap_or_default();
            println!("material {} techset {tech} sort {}", m.name, m.sort_key);
            for t in &m.textures {
                let img = t
                    .image
                    .map(|k| {
                        (
                            c.images[k.index].name.clone(),
                            c.images[k.index].width,
                            c.images[k.index].height,
                        )
                    })
                    .unwrap_or_default();
                println!(
                    "  texture hash {:#010x} semantic {} -> {:?}",
                    t.name_hash, t.semantic, img
                );
            }
            for k in &m.constants {
                println!("  constant {} = {:?}", k.1, k.2);
            }
        }
        // bo2mp: a destructible's pieces: health, hidden bones, each stage's
        // shown bone and what it spawns.
        if let Some(d) = c.destructibles.iter().find(|d| &d.name == name) {
            println!(
                "destructible {name}: model {} pristine {} pieces {} client_only {}",
                d.model,
                d.pristine_model,
                d.pieces.len(),
                d.client_only
            );
            for (i, p) in d.pieces.iter().enumerate() {
                println!(
                    "  piece {i}: parent {} health {} bullet x{} hide_bones {:08x?}",
                    p.parent_piece, p.health, p.bullet_damage_scale, p.hide_bones
                );
                for (k, st) in p.stages.iter().enumerate() {
                    if st.show_bone.is_empty() && st.break_health == 0.0 && k > 0 {
                        continue;
                    }
                    println!(
                        "    stage {k}: show `{}` break_health {} flags {:x} phys {} spawn {:?}",
                        st.show_bone, st.break_health, st.flags, st.phys_preset, st.spawn_models
                    );
                }
            }
        }
        if let Some(e) = c.fx.iter().find(|e| &e.name == name) {
            println!(
                "fx {name}: loop {} oneshot {} emit {} flags {:#x}",
                e.looping_count, e.one_shot_count, e.emission_count, e.flags
            );
            for (i, el) in e.elems.iter().enumerate() {
                println!(
                    "  elem {i}: type {} flags {:#010x} visuals {:?} life {}+{} spawn {} {} lighting {}",
                    el.elem_type(),
                    el.i32_at(0),
                    el.visuals,
                    el.i32_at(48),
                    el.i32_at(52),
                    el.i32_at(4),
                    el.i32_at(8),
                    el.raw[261]
                );
            }
        }
        if let Some(w) = c.weapons.iter().find(|w| &w.name == name) {
            println!("weapon {name}:");
            for (i, a) in w.xanims.iter().enumerate() {
                if !a.is_empty() {
                    println!("  anim slot {i:2}: {a}");
                }
            }
            for (k, v) in &w.notetrack_sounds {
                println!("  notetrack sound {k} -> {v}");
            }
            for (o, v) in &w.strings {
                println!("  def string @{o}: {v}");
            }
        }
        if let Some(a) = c.xanims.iter().find(|a| &a.name == name) {
            println!(
                "anim {name}: {} frames @ {} fps loop={} delta={} counts {:?} tracks {:?}",
                a.numframes, a.framerate, a.looping, a.delta, a.bone_count, a.names
            );
            println!("  notifies {:?}", a.notifies);
            // T6BONES_DELTA=1: the root motion keys (frame, x y z) and the
            // speed between keys.
            if std::env::var_os("T6BONES_DELTA").is_some() {
                let keys = &a.delta_trans;
                println!("  delta keys {}", keys.len());
                for w in keys.windows(2) {
                    let (f0, p0) = w[0];
                    let (f1, p1) = w[1];
                    let d = ((p1[0] - p0[0]).powi(2) + (p1[1] - p0[1]).powi(2)).sqrt();
                    let dt = f32::from(f1.saturating_sub(f0)) / a.framerate.max(1.0);
                    println!(
                        "  frame {f0:3}->{f1:3}: ({:.1} {:.1} {:.1}) speed {:.1}",
                        p1[0],
                        p1[1],
                        p1[2],
                        if dt > 0.0 { d / dt } else { 0.0 }
                    );
                }
            }
            // T6BONES_SAMPLE=<bone>: that bone's rotation each frame, as the
            // game's decoder reads it (yaw/pitch/roll in degrees).
            // T6BONES_FLIPS=1: per track, neighbouring frames whose
            // rotations sit on opposite sides (dot < 0): a plain blend between
            // them swings the bone the long way round.
            if std::env::var_os("T6BONES_FLIPS").is_some() {
                let parts = xmodel_runtime::RawXAnimParts {
                    name: a.name.clone(),
                    data_byte: a.data_byte.clone(),
                    data_short: a.data_short.clone(),
                    data_int: a.data_int.clone(),
                    random_data_byte: a.random_data_byte.clone(),
                    random_data_short: a.random_data_short.clone(),
                    random_data_int: a.random_data_int.clone(),
                    numframes: a.numframes,
                    flags: u8::from(a.looping) | (u8::from(a.delta) << 1),
                    bone_count: a.bone_count,
                    framerate: a.framerate,
                    names: a.names.clone(),
                    notifies: Vec::new(),
                    indices: a.indices.clone(),
                    delta_trans: None,
                };
                if let Ok(clip) = xmodel_runtime::AnimClip::from_parts(&parts) {
                    let mut total = 0;
                    for (ti, tr) in clip.tracks.iter().enumerate() {
                        let mut flips = Vec::new();
                        for f in 0..a.numframes {
                            let t0 = f32::from(f) / a.framerate.max(1.0);
                            let t1 = f32::from(f + 1) / a.framerate.max(1.0);
                            let (Some(q0), Some(q1)) = (
                                clip.sample_track(ti, t0).and_then(|s| s.rotation),
                                clip.sample_track(ti, t1).and_then(|s| s.rotation),
                            ) else {
                                continue;
                            };
                            let dot = q0[0] * q1[0] + q0[1] * q1[1] + q0[2] * q1[2] + q0[3] * q1[3];
                            if dot < 0.0 {
                                flips.push(f);
                            }
                        }
                        if !flips.is_empty() {
                            total += flips.len();
                            println!(
                                "  flips {}: frames {:?}",
                                tr.name,
                                &flips[..flips.len().min(12)]
                            );
                        }
                    }
                    println!("  flips total {total}");
                }
            }
            if let Ok(bone) = std::env::var("T6BONES_SAMPLE") {
                let parts = xmodel_runtime::RawXAnimParts {
                    name: a.name.clone(),
                    data_byte: a.data_byte.clone(),
                    data_short: a.data_short.clone(),
                    data_int: a.data_int.clone(),
                    random_data_byte: a.random_data_byte.clone(),
                    random_data_short: a.random_data_short.clone(),
                    random_data_int: a.random_data_int.clone(),
                    numframes: a.numframes,
                    flags: u8::from(a.looping) | (u8::from(a.delta) << 1),
                    bone_count: a.bone_count,
                    framerate: a.framerate,
                    names: a.names.clone(),
                    notifies: Vec::new(),
                    indices: a.indices.clone(),
                    delta_trans: None,
                };
                match xmodel_runtime::AnimClip::from_parts(&parts) {
                    Ok(clip) => {
                        let Some(track) = clip.tracks.iter().position(|t| t.name == bone) else {
                            println!("  no track {bone}");
                            continue;
                        };
                        for f in 0..a.numframes {
                            let t = f32::from(f) / a.framerate.max(1.0);
                            let s = clip.sample_track(track, t);
                            if let Some(v) = s.and_then(|s| s.translation) {
                                println!("  {bone} frame {f:2}: trans ({:.3} {:.3} {:.3})", v[0], v[1], v[2]);
                            }
                            if let Some(q) = s.and_then(|s| s.rotation) {
                                let [x, y, z, w] = [q[0], q[1], q[2], q[3]];
                                // The bone's axes in its parent's space; each
                                // one's heading on the ground (degrees).
                                let ax = [
                                    1.0 - 2.0 * (y * y + z * z),
                                    2.0 * (x * y + w * z),
                                    2.0 * (x * z - w * y),
                                ];
                                let ay = [
                                    2.0 * (x * y - w * z),
                                    1.0 - 2.0 * (x * x + z * z),
                                    2.0 * (y * z + w * x),
                                ];
                                let az = [
                                    2.0 * (x * z + w * y),
                                    2.0 * (y * z - w * x),
                                    1.0 - 2.0 * (x * x + y * y),
                                ];
                                let h = |v: [f32; 3]| v[1].atan2(v[0]).to_degrees();
                                println!(
                                    "  {bone} frame {f:2}: x ({:.2} {:.2} {:.2}) heading x {:6.1} y {:6.1} z {:6.1}",
                                    ax[0],
                                    ax[1],
                                    ax[2],
                                    h(ax),
                                    h(ay),
                                    h(az)
                                );
                            } else {
                                println!("  {bone} frame {f:2}: no rotation");
                            }
                        }
                    }
                    Err(e) => println!("  decode: {e:?}"),
                }
            }
        }
    }
}
