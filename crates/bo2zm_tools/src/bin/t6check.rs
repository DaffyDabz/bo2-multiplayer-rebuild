//! bo2zm: readiness checks for a Black Ops II map, read-only.
//! `t6check [BO2 folder]` walks the nine Nuketown Zombies zones and checks
//! that every image they use has pixels: in the zone or in a pack, decoding
//! to an IWI whose size matches the zone's description of it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use asset_t6::{ImageSource, PackSet, capture_zone};

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
    let root = std::env::args()
        .nth(1)
        .map_or_else(|| PathBuf::from(DEFAULT_BO2), PathBuf::from);
    match run(&root) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("t6check: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(root: &Path) -> Result<bool, String> {
    let started = Instant::now();
    let dir = root.join("zone").join("all");
    let packs = PackSet::open_dir(&dir)?;
    println!(
        "packs: {}",
        packs
            .packs
            .iter()
            .map(|p| format!("{} ({})", p.name, p.len()))
            .collect::<Vec<_>>()
            .join(", ")
    );

    let mut ok = true;
    let mut images_total = 0usize;
    let mut by_source: BTreeMap<String, usize> = BTreeMap::new();
    let mut missing = Vec::new();
    let mut size_mismatch = Vec::new();
    let mut decode_fail = Vec::new();
    for zone in NUKETOWN_ZONES {
        let capture = capture_zone(&dir.join(format!("{zone}.ff")))?;
        println!("{zone}: {} images", capture.images.len());
        for image in &capture.images {
            images_total += 1;
            match packs.locate(image) {
                Some(ImageSource::Pack(i, entry)) => {
                    let pack = &packs.packs[i];
                    *by_source.entry(pack.name.clone()).or_default() += 1;
                    match pack.read(entry).and_then(|bytes| {
                        ipak_t6::parse_iwi(&bytes)
                            .map(|iwi| (iwi.width, iwi.height))
                            .map_err(|e| e.to_string())
                    }) {
                        Ok((w, h)) => {
                            if (w, h) != (image.width as u32, image.height as u32) {
                                size_mismatch.push(format!(
                                    "{} zone {}x{} pack {w}x{h}",
                                    image.name, image.width, image.height
                                ));
                            }
                        }
                        Err(e) => decode_fail.push(format!("{}: {e}", image.name)),
                    }
                }
                Some(ImageSource::Embedded) => {
                    *by_source.entry("in the zone".into()).or_default() += 1
                }
                Some(ImageSource::Empty) => {
                    *by_source
                        .entry("no pixels (runtime image)".into())
                        .or_default() += 1
                }
                None => missing.push(format!("{} ({zone})", image.name)),
            }
        }
    }
    println!("images: {images_total}");
    for (k, n) in &by_source {
        println!("   {n:>6}  {k}");
    }
    for (label, list) in [
        ("MISSING", &missing),
        ("SIZE MISMATCH", &size_mismatch),
        ("DECODE FAILED", &decode_fail),
    ] {
        if !list.is_empty() {
            ok = false;
            println!("{label}: {}", list.len());
            for item in list.iter().take(12) {
                println!("   {item}");
            }
        }
    }
    println!(
        "{} in {:.2?}",
        if ok {
            "ALL IMAGES OK"
        } else {
            "IMAGE CHECK FAILED"
        },
        started.elapsed()
    );
    Ok(ok)
}
