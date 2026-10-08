//! bo2zm: open Black Ops II zones read-only and print what is in them.
//!
//! `t6zone <file.ff>...` reports each file; `t6zone --nuketown [BO2 folder]`
//! reports the nine zones Nuketown Zombies loads, from `<folder>/zone/all`.
//! Per zone: envelope, record count, inflated image vs the XFile header's
//! size, block sizes, script strings, dependencies, and the asset count by
//! type. Nothing is written anywhere.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use asset_transport::{T6ZoneMemory, ZoneGame, open_zone};
use fastfile_t6::{
    AssetType, MAX_XFILE_COUNT, WalkSink, Walker, XChunks, XFILE_BLOCK_NAMES, XFILE_HEADER_LEN,
    ZoneStream, open_asset_table, parse_file_header,
};

const DEFAULT_BO2: &str = r"C:\Program Files (x86)\Steam\steamapps\common\Call of Duty Black Ops II";

/// The Nuketown Zombies load set, in load order.
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

/// Counts what a walk loaded and remembers where it was.
#[derive(Default)]
struct WalkCensus {
    current: Option<(usize, AssetType)>,
    loaded: BTreeMap<AssetType, usize>,
    /// (stream cursor, VIRTUAL watermark, index, type) at each asset start.
    starts: Vec<(usize, usize, usize, AssetType)>,
}

impl WalkCensus {
    fn asset_at(&self, cursor: usize) -> Option<&(usize, usize, usize, AssetType)> {
        self.starts.iter().rev().find(|s| s.0 <= cursor)
    }
}

impl WalkSink for WalkCensus {
    fn begin_asset(&mut self, s: &ZoneStream<'_>, index: usize, ty: AssetType) {
        self.current = Some((index, ty));
        self.starts.push((
            s.cursor(),
            s.watermark(fastfile_t6::XFILE_BLOCK_VIRTUAL as u8),
            index,
            ty,
        ));
    }

    fn asset_loaded(
        &mut self,
        _s: &ZoneStream<'_>,
        loaded: fastfile_t6::Loaded,
    ) -> fastfile_t6::Result<()> {
        *self.loaded.entry(loaded.ty).or_default() += 1;
        Ok(())
    }
}

struct ZoneReport {
    name: String,
    counts: BTreeMap<AssetType, usize>,
    assets: usize,
}

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let walk = args.first().is_some_and(|a| a == "--walk");
    if walk {
        args.remove(0);
    }
    let files: Vec<PathBuf> = match args.first().map(String::as_str) {
        Some("--nuketown") => {
            let root = args.get(1).map_or(DEFAULT_BO2, String::as_str);
            let dir = Path::new(root).join("zone").join("all");
            NUKETOWN_ZONES
                .iter()
                .map(|zone| dir.join(format!("{zone}.ff")))
                .collect()
        }
        Some(_) => args.iter().map(PathBuf::from).collect(),
        None => {
            eprintln!("usage: t6zone <zone.ff>... | t6zone --nuketown [BO2 folder]");
            return ExitCode::from(2);
        }
    };

    let mut reports = Vec::new();
    let mut failed = 0usize;
    for path in &files {
        match report(path, walk) {
            Ok(r) => reports.push(r),
            Err(error) => {
                failed += 1;
                println!("== {}\n   FAILED: {error}\n", path.display());
            }
        }
    }

    if reports.len() > 1 {
        print_totals(&reports);
    }
    println!(
        "opened {}/{} zones{}",
        reports.len(),
        files.len(),
        if failed == 0 { "" } else { " - SOME FAILED" }
    );
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn report(path: &Path, do_walk: bool) -> Result<ZoneReport, String> {
    let started = Instant::now();
    let file = std::fs::read(path).map_err(|e| format!("reading: {e}"))?;
    let header = parse_file_header(&file).map_err(|e| e.to_string())?;
    let records = XChunks::new(&file, header.body_offset);
    let mut record_count = 0usize;
    let mut walk = records.clone();
    for record in walk.by_ref() {
        record.map_err(|e| e.to_string())?;
        record_count += 1;
    }
    let records_end = walk.position();

    let image = open_zone(path).map_err(|e| e.to_string())?;
    if image.game != ZoneGame::T6 {
        return Err(format!("opened as {:?}, not T6", image.game));
    }
    let xfile = image.t6_header().map_err(|e| e.to_string())?;
    let content_len = image.bytes.len() - XFILE_HEADER_LEN;

    let mut memory = T6ZoneMemory::for_header(&xfile);
    let mut stream = memory.stream(&image.bytes).map_err(|e| e.to_string())?;
    let table = open_asset_table(&mut stream).map_err(|e| e.to_string())?;
    let list_end = stream.cursor();

    let mut counts: BTreeMap<AssetType, usize> = BTreeMap::new();
    let mut unknown: BTreeMap<u32, usize> = BTreeMap::new();
    for i in 0..table.count() {
        let raw = table.raw_kind(&stream, i).map_err(|e| e.to_string())?;
        match AssetType::from_u32(raw) {
            Some(ty) => *counts.entry(ty).or_default() += 1,
            None => *unknown.entry(raw).or_default() += 1,
        }
    }

    let name = header.zone_name().map(str::to_owned).unwrap_or_else(|| {
        path.file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned())
    });
    println!("== {name}  ({})", path.display());
    println!(
        "   file {} bytes, {:?}, {} records (records end at {:#x}, file pads {} bytes after)",
        file.len(),
        header.signing,
        record_count,
        records_end,
        file.len().saturating_sub(records_end)
    );
    println!(
        "   image {} bytes = 40-byte XFile header + {} content; XFile says size {} external {}  [{}]",
        image.bytes.len(),
        content_len,
        xfile.size,
        xfile.external_size,
        if content_len == xfile.size as usize {
            "content length matches"
        } else {
            "CONTENT LENGTH MISMATCH"
        }
    );
    let blocks: Vec<String> = (0..MAX_XFILE_COUNT)
        .filter(|&i| xfile.block_size[i] != 0)
        .map(|i| format!("{}={}", XFILE_BLOCK_NAMES[i], xfile.block_size[i]))
        .collect();
    println!("   blocks: {}", blocks.join(" "));
    let depends: Vec<&str> = (0..table.depends.count())
        .filter_map(|i| table.depends.get(&stream, i))
        .collect();
    println!(
        "   script strings {}, dependencies {}{}",
        table.strings.count(),
        table.depends.count(),
        if depends.is_empty() {
            String::new()
        } else {
            format!(" ({})", depends.join(", "))
        }
    );
    let sample: Vec<&str> = (0..table.strings.count().min(8))
        .filter_map(|i| table.strings.get(&stream, i))
        .collect();
    if !sample.is_empty() {
        println!("   first script strings: {}", sample.join(" | "));
    }
    println!(
        "   asset list ends at image offset {list_end:#x}; {} bytes of asset bodies follow",
        image.bytes.len() - list_end
    );
    println!("   {} assets:", table.count());
    for (ty, n) in &counts {
        println!("      {:>6}  {}", n, ty.name());
    }
    for (raw, n) in &unknown {
        println!("      {n:>6}  UNKNOWN pool id {raw:#x}");
    }
    if do_walk {
        let walk_started = Instant::now();
        let mut census = WalkCensus::default();
        let result = {
            let mut walker = Walker::new(&mut stream, &mut census);
            let r = walker.walk_assets(&table);
            let trail: Vec<&str> = walker.trail().collect();
            r.map_err(|e| (e, trail))
        };
        match result {
            Ok(()) => {
                let end = stream.cursor();
                let blocks: Vec<String> = (0..MAX_XFILE_COUNT)
                    .filter(|&i| xfile.block_size[i] != 0 || stream.watermark(i as u8) != 0)
                    .map(|i| {
                        format!(
                            "{} {}/{}",
                            XFILE_BLOCK_NAMES[i],
                            stream.watermark(i as u8),
                            xfile.block_size[i]
                        )
                    })
                    .collect();
                let loaded: usize = census.loaded.values().sum();
                println!(
                    "   WALK: {} at image offset {end:#x} of {:#x}; {loaded} asset loads; unsettled offsets {}; {:.2?}",
                    if end == image.bytes.len() {
                        "reached the exact end"
                    } else {
                        "STOPPED SHORT"
                    },
                    image.bytes.len(),
                    stream.unsettled_offsets(),
                    walk_started.elapsed()
                );
                println!("   blocks used/declared: {}", blocks.join(", "));
                if let Some((p, mark, cursor)) = stream.first_unsettled() {
                    let at = census.asset_at(cursor);
                    println!(
                        "   first forward offset: block {} offset {:#x} while block was at {:#x} (stream {:#x}) in asset {:?}",
                        p.block,
                        p.offset,
                        mark,
                        cursor,
                        at.map(|a| (a.2, a.3.name(), a.1))
                    );
                }
                if end != image.bytes.len() {
                    return Err(format!(
                        "walk ended at {end:#x}, image is {:#x}",
                        image.bytes.len()
                    ));
                }
            }
            Err((error, trail)) => {
                let (index, ty) = census
                    .current
                    .unwrap_or((usize::MAX, AssetType::XModelPieces));
                println!(
                    "   WALK FAILED at asset {index} ({}) image offset {:#x}: {error}\n   inside: {}",
                    ty.name(),
                    stream.cursor(),
                    trail.join(" > ")
                );
                return Err(format!("walk failed at asset {index} ({})", ty.name()));
            }
        }
    }
    println!("   read in {:.2?}\n", started.elapsed());

    if !unknown.is_empty() {
        return Err(format!(
            "{} assets with unknown pool ids",
            unknown.values().sum::<usize>()
        ));
    }
    Ok(ZoneReport {
        name,
        assets: table.count(),
        counts,
    })
}

fn print_totals(reports: &[ZoneReport]) {
    let mut all: BTreeMap<AssetType, usize> = BTreeMap::new();
    for r in reports {
        for (ty, n) in &r.counts {
            *all.entry(*ty).or_default() += n;
        }
    }
    println!("== totals by type across {} zones", reports.len());
    let width = reports.iter().map(|r| r.name.len()).max().unwrap_or(4);
    for (ty, total) in &all {
        let per: Vec<String> = reports
            .iter()
            .filter_map(|r| r.counts.get(ty).map(|n| format!("{}={n}", r.name)))
            .collect();
        println!("   {:>6}  {:<18} {}", total, ty.name(), per.join(" "));
    }
    let assets: usize = reports.iter().map(|r| r.assets).sum();
    println!("   {assets:>6}  assets total");
    for r in reports {
        println!("   {:<width$} {:>6} assets", r.name, r.assets);
    }
    println!();
}
