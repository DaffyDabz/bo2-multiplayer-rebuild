//! bo2mp: front-end picture names defined in more than one zone, read-only.
//! `t6dups` lists every material name that two or more of code_post_gfx_mp,
//! ui_mp, patch_mp, patch_ui_mp and common_mp define: per copy the colour
//! image (name, size, source, a hash of its stored bytes) and a hash of the
//! material's draw state, which copy the menus picked before (first zone
//! wins) and after (latest front-end zone wins, common_mp last), and
//! whether the UI scripts or string tables mention the name.

use std::collections::{BTreeMap, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::process::ExitCode;

use asset_t6::{ImageSource, PackSet, ZoneCapture, capture_zone};

const ZONES: &str = r"C:\Program Files (x86)\Steam\steamapps\common\Call of Duty Black Ops II\zone\all";
const ORDER: [&str; 5] = ["code_post_gfx_mp", "ui_mp", "patch_mp", "patch_ui_mp", "common_mp"];
const COLOR_MAP_HASH: u32 = 0xa0ab_1041;

fn h<T: Hash + ?Sized>(v: &T) -> u64 {
    let mut s = DefaultHasher::new();
    v.hash(&mut s);
    s.finish()
}

struct Copy {
    zone: &'static str,
    image: String,
    size: String,
    source: String,
    bytes: u64,
    state: u64,
}

fn main() -> ExitCode {
    let dir = Path::new(ZONES);
    let Ok(packs) = PackSet::open_dir(dir) else {
        eprintln!("packs");
        return ExitCode::FAILURE;
    };
    let caps: Vec<(&'static str, ZoneCapture)> = ORDER
        .iter()
        .filter_map(|z| capture_zone(&dir.join(format!("{z}.ff"))).ok().map(|c| (*z, c)))
        .collect();
    // Text the UI scripts and tables carry.
    let mut text: Vec<u8> = Vec::new();
    for (z, c) in &caps {
        if *z == "common_mp" {
            continue;
        }
        for (_, b) in &c.raw_files {
            text.extend_from_slice(b);
        }
        for t in &c.string_tables {
            for cell in &t.cells {
                text.extend_from_slice(cell.as_bytes());
            }
        }
    }
    let mut by_name: BTreeMap<&str, Vec<Copy>> = BTreeMap::new();
    for (zone, c) in &caps {
        for m in &c.materials {
            let tex = m
                .textures
                .iter()
                .find(|t| t.name_hash == COLOR_MAP_HASH && t.image.is_some())
                .or_else(|| m.textures.iter().find(|t| t.image.is_some()));
            let img = tex.and_then(|t| t.image).and_then(|k| c.images.get(k.index));
            let (image, size, source, bytes) = match img {
                None => ("-".into(), "-".into(), "no image".into(), 0),
                Some(i) => {
                    let (src, b) = match packs.locate(i) {
                        Some(ImageSource::Pack(p, e)) => ("pack".to_owned(), packs.packs[p].read(e).map_or(0, |v| h(&v))),
                        Some(ImageSource::Embedded) => ("zone".to_owned(), i.embedded.as_ref().map_or(0, |e| h(&e.data))),
                        Some(ImageSource::Empty) => ("empty".to_owned(), 0),
                        None => ("nowhere".to_owned(), 0),
                    };
                    (i.name.clone(), format!("{}x{}", i.width, i.height), src, b)
                }
            };
            let consts: Vec<(&str, [u32; 4])> = m.constants.iter().map(|(_, n, v)| (n.as_str(), v.map(f32::to_bits))).collect();
            let state = h(&(&m.state_bits_entry[..], &m.state_bits, m.state_flags, consts));
            by_name.entry(m.name.as_str()).or_default().push(Copy { zone, image, size, source, bytes, state });
        }
    }
    let (mut dups, mut changed, mut used_changed) = (0, 0, 0);
    for (name, copies) in &by_name {
        if copies.len() < 2 {
            continue;
        }
        dups += 1;
        let old = &copies[0];
        let new = copies.iter().rev().find(|c| c.zone != "common_mp").unwrap_or(old);
        // common_mp is last in both orders; with only common copies the pick stays.
        let old_pick = copies.iter().find(|c| c.zone != "common_mp").unwrap_or(old);
        let differs = old_pick.zone != new.zone && (old_pick.bytes != new.bytes || old_pick.state != new.state || old_pick.image != new.image);
        let used = text.windows(name.len()).any(|w| w == name.as_bytes());
        if differs {
            changed += 1;
            if used {
                used_changed += 1;
            }
        }
        println!("{name}{}{}", if differs { "  CHANGED" } else { "" }, if used { "  [named by UI]" } else { "" });
        for c in copies {
            let tag = match (std::ptr::eq(c, old_pick), std::ptr::eq(c, new)) {
                (true, true) => "old+new",
                (true, false) => "old",
                (false, true) => "new",
                _ => "",
            };
            println!("    {:<17} {:<8} img {} {} {} bytes {:016x} state {:016x}", c.zone, tag, c.image, c.size, c.source, c.bytes, c.state);
        }
    }
    println!("{dups} duplicate names; {changed} change pick with different content; {used_changed} of those named by UI scripts/tables");
    ExitCode::SUCCESS
}
