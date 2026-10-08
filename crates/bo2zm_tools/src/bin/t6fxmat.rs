//! bo2zm: the materials a Black Ops II map's placed effects draw with,
//! read-only. `t6fxmat [bo2 root]` reads Nuketown's zones and scripts, walks
//! every effect placed at load (and the effects they spawn), and prints per
//! material its technique set, blend, colour map and pixel format, then a
//! count per (technique, blend, format).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::process::ExitCode;

use asset_t6::{DrawState, FxVisualRef, ImageSource, MapScriptFacts, PackSet, ZoneCapture};

const DEFAULT_BO2: &str = r"C:\Program Files (x86)\Steam\steamapps\common\Call of Duty Black Ops II";
const ZONES: [&str; 8] = [
    "code_post_gfx_zm",
    "common_zm",
    "patch_zm",
    "patch_ui_zm",
    "dlczm0_load_zm",
    "zm_nuked",
    "zm_nuked_patch",
    "code_pre_gfx_zm",
];

fn main() -> ExitCode {
    let root = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| DEFAULT_BO2.to_owned()));
    let dir = root.join("zone").join("all");
    let mut caps: Vec<ZoneCapture> = Vec::new();
    for z in ZONES {
        match asset_t6::capture_zone(&dir.join(format!("{z}.ff"))) {
            Ok(c) => caps.push(c),
            Err(e) => eprintln!("{z}: {e}"),
        }
    }
    let Ok(packs) = PackSet::open_dir(&dir) else {
        eprintln!("no image packs");
        return ExitCode::FAILURE;
    };
    let facts = MapScriptFacts::read("zm_nuked", |name| {
        let want = format!("script:{name}");
        caps.iter().rev().find_map(|c| c.raw_files.iter().find(|(n, _)| *n == want).map(|(_, b)| b.clone()))
    });
    let mut fx = HashMap::new();
    for c in &caps {
        for e in c.fx.iter().filter(|e| !e.name.starts_with(',') && !e.name.is_empty()) {
            fx.insert(e.name.clone(), e);
        }
    }
    // Effects placed at load, and every effect they spawn.
    let mut todo: Vec<String> = facts
        .placed
        .iter()
        .filter(|p| p.kind != "exploder")
        .filter_map(|p| facts.effects.get(&p.fxid).cloned())
        .collect();
    let mut seen = BTreeSet::new();
    let mut materials: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    while let Some(name) = todo.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let Some(e) = fx.get(&name) else { continue };
        for el in &e.elems {
            for v in &el.visuals {
                match v {
                    FxVisualRef::Material(m) => {
                        materials.entry(m.clone()).or_default().insert(format!("{name}#t{}", el.elem_type()));
                    }
                    FxVisualRef::Effect(child) => todo.push(child.clone()),
                    _ => {}
                }
            }
            for child in [&el.effect_on_impact, &el.effect_on_death, &el.effect_emitted] {
                if !child.is_empty() {
                    todo.push(child.clone());
                }
            }
        }
    }
    println!("effects reached {}, materials {}", seen.len(), materials.len());
    let mut combos: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (m, users) in &materials {
        let found = caps.iter().find_map(|c| {
            c.materials.iter().find(|x| x.name == *m && !x.textures.is_empty()).map(|x| (c, x))
        });
        let Some((c, mat)) = found else {
            println!("{m}: NOT FOUND");
            continue;
        };
        let ts = mat.technique_set.map_or("-".to_owned(), |t| c.technique_sets[t.index].name.clone());
        let state = [6usize, 5, 4, 3, 2, 1, 0]
            .iter()
            .find_map(|&t| {
                let e = *mat.state_bits_entry.get(t)?;
                mat.state_bits.get(usize::from(e)).map(|b| (t, DrawState::decode(*b)))
            });
        let tex = mat.textures.iter().find(|t| t.name_hash == 0xa0ab_1041).or(mat.textures.first());
        let img = tex.and_then(|t| t.image).map(|k| &c.images[k.index]);
        let img = img.map(|i| {
            i.name
                .strip_prefix(',')
                .and_then(|real| caps.iter().find_map(|cc| cc.images.iter().find(|x| x.name == real)))
                .unwrap_or(i)
        });
        let format = img.map_or("-".to_owned(), |i| match packs.locate(i) {
            Some(ImageSource::Pack(p, entry)) => packs.packs[p]
                .read(entry)
                .ok()
                .and_then(|b| ipak_t6::parse_iwi(&b).ok().map(|iwi| format!("{:?}", iwi.format)))
                .unwrap_or_else(|| "unreadable".to_owned()),
            Some(ImageSource::Embedded) => "embedded".to_owned(),
            Some(ImageSource::Empty) => "empty".to_owned(),
            None => "missing".to_owned(),
        });
        let blend = state.map_or("no state".to_owned(), |(t, s)| {
            format!("slot{t} {}/{} op{} at{:?}", s.src_blend, s.dst_blend, s.blend_op, s.alpha_test)
        });
        let family = ts.trim_start_matches(',').split('_').next().unwrap_or("").to_owned();
        println!(
            "{m} | {ts} | {blend} | {} {} | users {}",
            img.map_or("-", |i| i.name.as_str()),
            format,
            users.iter().take(2).cloned().collect::<Vec<_>>().join(", ")
        );
        combos.entry(format!("{family} {blend} {format}")).or_default().push(m.clone());
    }
    println!("\n== combos");
    for (k, v) in &combos {
        println!("{:>3} {k}: {}", v.len(), v.iter().take(3).cloned().collect::<Vec<_>>().join(", "));
    }
    ExitCode::SUCCESS
}
