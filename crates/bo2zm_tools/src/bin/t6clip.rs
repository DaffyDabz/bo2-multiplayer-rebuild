//! bo2zm: measure a Black Ops II map's collision, read-only.
//! `t6clip [zone.ff] [x y z ...]` (default Nuketown, at its player start)
//! counts brush contents bits, then lists every brush whose player-sized hull
//! holds each point: the brushes a player standing there would be inside.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use asset_t6::{ClipBrushRef, ClipRef, capture_zone};

const DEFAULT_ZONE: &str =
    r"C:\Program Files (x86)\Steam\steamapps\common\Call of Duty Black Ops II\zone\all\zm_nuked.ff";

/// IW4's standing player box.
const PLAYER_MINS: [f32; 3] = [-15.0, -15.0, 0.0];
const PLAYER_MAXS: [f32; 3] = [15.0, 15.0, 70.0];

/// Every plane of a brush: the six box planes, then the extra sides.
fn planes(b: &ClipBrushRef) -> Vec<[f32; 4]> {
    let mut p = vec![
        [1.0, 0.0, 0.0, b.maxs[0]],
        [-1.0, 0.0, 0.0, -b.mins[0]],
        [0.0, 1.0, 0.0, b.maxs[1]],
        [0.0, -1.0, 0.0, -b.mins[1]],
        [0.0, 0.0, 1.0, b.maxs[2]],
        [0.0, 0.0, -1.0, -b.mins[2]],
    ];
    p.extend(b.sides.iter().map(|&(n, d, _)| [n[0], n[1], n[2], d]));
    p
}

/// Whether the box at `origin` overlaps the brush: every plane pushed out by
/// the box's reach along its normal must still have the origin behind it.
fn box_inside(b: &ClipBrushRef, origin: [f32; 3]) -> bool {
    planes(b).iter().all(|p| {
        let reach: f32 = (0..3)
            .map(|i| {
                if p[i] < 0.0 {
                    p[i] * PLAYER_MINS[i]
                } else {
                    p[i] * PLAYER_MAXS[i]
                }
            })
            .sum();
        p[0] * origin[0] + p[1] * origin[1] + p[2] * origin[2] - (p[3] + reach) < 0.0
    })
}

/// The brushes listed under one leaf brush node (0 = none), by the same
/// visit rule the engine uses: brush lists at positive counts, else the
/// node after a negative count and each nonzero child offset.
fn leaf_brushes(clip: &ClipRef, root: i32) -> Vec<u16> {
    let mut out = Vec::new();
    if root <= 0 {
        return out;
    }
    let mut pending = vec![root as usize];
    while let Some(index) = pending.pop() {
        let Some(node) = clip.leafbrush_nodes.get(index) else {
            continue;
        };
        if node.leaf_brush_count > 0 {
            out.extend_from_slice(&node.brushes);
            continue;
        }
        if node.leaf_brush_count < 0 {
            pending.push(index + 1);
        }
        for off in node.child_offset {
            if off != 0 {
                pending.push(index + usize::from(off));
            }
        }
    }
    out
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1).peekable();
    let path = match args.peek() {
        Some(a) if a.ends_with(".ff") => PathBuf::from(args.next().unwrap_or_default()),
        _ => PathBuf::from(DEFAULT_ZONE),
    };
    let nums: Vec<f32> = args.filter_map(|a| a.parse().ok()).collect();
    let capture = match capture_zone(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("t6clip: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Ok(out) = std::env::var("T6CLIP_ENTS") {
        let text: String = capture.map_ents.concat();
        if let Err(e) = std::fs::write(&out, text) {
            eprintln!("t6clip: {out}: {e}");
        }
    }
    let Some(clip) = capture.clip.as_ref() else {
        eprintln!("t6clip: {} has no clipMap", path.display());
        return ExitCode::FAILURE;
    };

    let mut bits: BTreeMap<u32, usize> = BTreeMap::new();
    for b in &clip.brushes {
        for bit in 0..32 {
            if (b.contents as u32) & (1 << bit) != 0 {
                *bits.entry(1 << bit).or_default() += 1;
            }
        }
    }
    println!("{} brushes; contents bits:", clip.brushes.len());
    for (bit, n) in &bits {
        println!("   {bit:#010x}  {n}");
    }
    let mut mats: BTreeMap<u32, usize> = BTreeMap::new();
    for m in &clip.materials {
        *mats.entry(m.content_flags as u32).or_default() += 1;
    }
    println!("{} clip materials; content flags:", clip.materials.len());
    for (flags, n) in &mats {
        println!("   {flags:#010x}  {n}");
    }

    let mut points: Vec<[f32; 3]> = nums.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
    if points.is_empty() {
        for text in &capture.map_ents {
            for e in asset_t6::parse_entities(text) {
                if e.classname() == "info_player_start"
                    && let Some(o) = e.vec3("origin")
                {
                    points.push(o);
                    points.push([o[0], o[1], o[2] + 228.0]);
                }
            }
        }
    }
    // Leaves the tree reaches from node 0; a negative child c is leaf -1-c.
    let mut reach = vec![false; clip.leaves.len()];
    let mut stack = if clip.nodes.is_empty() {
        vec![]
    } else {
        vec![0i32]
    };
    let mut seen_nodes = vec![false; clip.nodes.len()];
    while let Some(c) = stack.pop() {
        if c < 0 {
            if let Some(r) = reach.get_mut((-1 - c) as usize) {
                *r = true;
            }
            continue;
        }
        let i = c as usize;
        if i >= clip.nodes.len() || seen_nodes[i] {
            continue;
        }
        seen_nodes[i] = true;
        stack.extend(clip.nodes[i].children.map(i32::from));
    }
    println!(
        "tree reaches {} of {} nodes and {} of {} leaves",
        seen_nodes.iter().filter(|v| **v).count(),
        clip.nodes.len(),
        reach.iter().filter(|v| **v).count(),
        clip.leaves.len()
    );
    let mut world = vec![false; clip.brushes.len()];
    let mut world_reached = vec![false; clip.brushes.len()];
    for (li, leaf) in clip.leaves.iter().enumerate() {
        for b in leaf_brushes(clip, leaf.leaf_brush_node) {
            if let Some(w) = world.get_mut(usize::from(b)) {
                *w = true;
            }
            if reach[li]
                && let Some(w) = world_reached.get_mut(usize::from(b))
            {
                *w = true;
            }
        }
    }
    println!(
        "brushes listed by reached leaves: {}; by any leaf: {}",
        world_reached.iter().filter(|v| **v).count(),
        world.iter().filter(|v| **v).count()
    );
    let mut owner: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, m) in clip.cmodels.iter().enumerate() {
        for b in leaf_brushes(clip, m.leaf.leaf_brush_node) {
            owner.entry(usize::from(b)).or_default().push(i);
        }
    }
    println!(
        "tree: {} nodes, {} leaves, {} leaf brush nodes, {} brush models ({} share the map's info); {} brushes listed by leaves",
        clip.nodes.len(),
        clip.leaves.len(),
        clip.leafbrush_nodes.len(),
        clip.cmodels.len(),
        clip.cmodels.iter().filter(|m| m.shares_map_info).count(),
        world.iter().filter(|w| **w).count()
    );
    if let Some(m) = clip.cmodels.first() {
        println!(
            "model 0: {} brushes, mins [{:.0}, {:.0}, {:.0}] maxs [{:.0}, {:.0}, {:.0}]",
            leaf_brushes(clip, m.leaf.leaf_brush_node).len(),
            m.mins[0],
            m.mins[1],
            m.mins[2],
            m.maxs[0],
            m.maxs[1],
            m.maxs[2]
        );
    }
    for p in points {
        let hits: Vec<usize> = clip
            .brushes
            .iter()
            .enumerate()
            .filter(|(_, b)| box_inside(b, p))
            .map(|(i, _)| i)
            .collect();
        println!(
            "player box at [{:.1}, {:.1}, {:.1}]: inside {} brushes",
            p[0],
            p[1],
            p[2],
            hits.len()
        );
        for &i in hits.iter().take(20) {
            let b = &clip.brushes[i];
            let whose = format!(
                "{}{}",
                match (world_reached[i], world[i]) {
                    (true, _) => "WORLD WALL",
                    (false, true) => "entity leaf only",
                    (false, false) => "no leaf",
                },
                owner
                    .get(&i)
                    .map_or_else(String::new, |m| format!(", brush model {m:?}"))
            );
            println!(
                "   brush {i} ({whose}): contents {:#010x}, {} extra sides, mins [{:.0}, {:.0}, {:.0}] maxs [{:.0}, {:.0}, {:.0}]",
                b.contents as u32,
                b.sides.len(),
                b.mins[0],
                b.mins[1],
                b.mins[2],
                b.maxs[0],
                b.maxs[1],
                b.maxs[2]
            );
            println!(
                "      table contents {:#x?}",
                clip.brush_contents.get(i).map(|c| *c as u32)
            );
            println!(
                "      axial cflags {:x?} sflags {:x?}; side sflags {:x?}",
                b.axial_cflags,
                b.axial_sflags,
                b.sides.iter().map(|s| s.2).collect::<Vec<_>>()
            );
        }
    }
    let mut same = 0;
    let mut differ: BTreeMap<(u32, u32, bool), usize> = BTreeMap::new();
    for (i, b) in clip.brushes.iter().enumerate() {
        let t = clip.brush_contents.get(i).copied().unwrap_or(-1);
        if t == b.contents {
            same += 1;
        } else {
            *differ
                .entry((b.contents as u32, t as u32, owner.contains_key(&i)))
                .or_default() += 1;
        }
    }
    println!(
        "table vs brush contents: {} brushes, {} table entries, {same} same",
        clip.brushes.len(),
        clip.brush_contents.len()
    );
    for ((b, t, owned), n) in differ {
        println!("   brush {b:#010x} table {t:#010x} model-owned {owned}: {n}");
    }
    let owned: usize = owner.len();
    let both = owner.keys().filter(|&&i| world[i]).count();
    println!("{owned} brushes owned by brush models; {both} of them also in world leaves");
    for (i, m) in clip.cmodels.iter().enumerate().take(12) {
        let list = leaf_brushes(clip, m.leaf.leaf_brush_node);
        println!(
            "   model {i}: {} brushes {:?}.., contents {:#x}, mins [{:.0}, {:.0}, {:.0}] maxs [{:.0}, {:.0}, {:.0}]",
            list.len(),
            &list[..list.len().min(4)],
            m.leaf.brush_contents as u32,
            m.mins[0],
            m.mins[1],
            m.mins[2],
            m.maxs[0],
            m.maxs[1],
            m.maxs[2]
        );
    }
    ExitCode::SUCCESS
}
