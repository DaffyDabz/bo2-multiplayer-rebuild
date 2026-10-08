//! bo2zm: Black Ops II sound.
//!
//! A T6 zone carries sound *banks* (`SndBank`): alias names with their
//! variants (volume, pitch, distances, falloff curves, the file each one
//! plays, by the hash of its name). The audio itself lives beside the
//! zones in `sound/*.sabl` (loaded, 16-bit PCM) and `*.sabs` (streamed,
//! FLAC) files, read by `sab_t6`. Aliases become `CapturedSound`s whose
//! variants name their clip as streamed `(bank file, entry id)`; the clip
//! store decodes a clip on first use through `decode_t6_clip`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use asset_core::ZoneOwner;
use asset_transport::ZoneGame;

use crate::{CapturedAlias, CapturedSndCurve, CapturedSound, SoundCatalog, VoicePriority};

/// Every bank file a set of bank names reads, indexed by entry id.
#[derive(Default)]
pub struct T6BankIndex {
    by_id: HashMap<u32, (Arc<sab_t6::BankFile>, sab_t6::BankEntry)>,
    pub files: Vec<PathBuf>,
}

impl T6BankIndex {
    /// Open every `<bank>.<language>.sab*` in `sound_dir` whose name starts
    /// with one of `banks` (and the languages `all` / `english`). Earlier
    /// banks win an id they share with a later one.
    pub fn open(sound_dir: &Path, banks: &[&str]) -> (Self, Vec<String>) {
        let mut index = Self::default();
        let mut report = Vec::new();
        let Ok(read) = std::fs::read_dir(sound_dir) else {
            report.push(format!("t6 sound: no sound folder at {}", sound_dir.display()));
            return (index, report);
        };
        let mut paths: Vec<PathBuf> = read.filter_map(|e| e.ok().map(|e| e.path())).collect();
        paths.sort();
        for bank in banks {
            for path in &paths {
                let Some(file) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let lower = file.to_ascii_lowercase();
                let wanted = [".all.sabl", ".all.sabs", ".english.sabl", ".english.sabs"]
                    .iter()
                    .any(|tail| lower == format!("{bank}{tail}"));
                if !wanted {
                    continue;
                }
                match sab_t6::BankFile::open(path) {
                    Ok(file) => {
                        let file = Arc::new(file);
                        for e in &file.entries {
                            index
                                .by_id
                                .entry(e.id)
                                .or_insert_with(|| (Arc::clone(&file), *e));
                        }
                        index.files.push(path.clone());
                    }
                    Err(e) => report.push(format!("t6 sound bank gap: {e}")),
                }
            }
        }
        report.push(format!(
            "t6 sound banks: {} files, {} clips",
            index.files.len(),
            index.by_id.len()
        ));
        (index, report)
    }

    pub fn get(&self, id: u32) -> Option<&(Arc<sab_t6::BankFile>, sab_t6::BankEntry)> {
        self.by_id.get(&id)
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

/// Opened bank files by path, shared by every clip decode.
fn open_banks() -> &'static Mutex<HashMap<String, Arc<sab_t6::BankFile>>> {
    static BANKS: OnceLock<Mutex<HashMap<String, Arc<sab_t6::BankFile>>>> = OnceLock::new();
    BANKS.get_or_init(Default::default)
}

/// Decode one clip: `bank` is the bank file's path, `id` the entry id as
/// eight hex digits (what a T6 alias variant names as its streamed clip).
pub fn decode_t6_clip(bank: &str, id: &str) -> Result<sab_t6::Pcm16, String> {
    let id = u32::from_str_radix(id, 16).map_err(|e| format!("clip id `{id}`: {e}"))?;
    let file = {
        let mut banks = open_banks().lock().map_err(|_| "bank cache poisoned".to_owned())?;
        match banks.get(bank) {
            Some(file) => Arc::clone(file),
            None => {
                let file = Arc::new(sab_t6::BankFile::open(Path::new(bank))?);
                banks.insert(bank.to_owned(), Arc::clone(&file));
                file
            }
        }
    };
    let entry = file
        .entries
        .iter()
        .find(|e| e.id == id)
        .copied()
        .ok_or_else(|| format!("{bank}: no clip {id:08x}"))?;
    file.decode(&entry)
}

/// The curve name a falloff curve index stands for in the driver globals.
fn curve(globals: Option<&asset_t6::SndDriverGlobalsRef>, index: u32) -> Option<CapturedSndCurve> {
    let c = globals?.curves.get(index as usize)?;
    // Curves end at the first point whose x falls back below the last.
    let mut knots: Vec<(f32, f32)> = Vec::with_capacity(8);
    for p in c.points {
        if knots.last().is_some_and(|&(x, _)| p[0] < x) {
            break;
        }
        knots.push((p[0], p[1]));
    }
    Some(CapturedSndCurve {
        name: format!("t6/curve/{}", c.name),
        knots,
    })
}

fn alias_variant(
    a: &asset_t6::SndAliasRef,
    list: &str,
    index: &T6BankIndex,
    globals: Option<&asset_t6::SndDriverGlobalsRef>,
) -> CapturedAlias {
    let (vol_min, vol_max) = a.volume();
    let (pitch_min, pitch_max) = a.pitch();
    let streamed = index
        .get(a.asset_id)
        .map(|(file, _)| (file.path.to_string_lossy().into_owned(), format!("{:08x}", a.asset_id)));
    CapturedAlias {
        alias_name: list.to_owned(),
        subtitle: (!a.subtitle.is_empty()).then(|| a.subtitle.clone()),
        secondary: (!a.secondary.is_empty()).then(|| a.secondary.clone()),
        file_type: Some(2),
        file_exists: Some(u8::from(streamed.is_some())),
        file_name: (!a.asset_file.is_empty()).then(|| a.asset_file.clone()),
        streamed,
        vol_min,
        vol_max,
        pitch_min: if pitch_min > 0.0 { pitch_min } else { 1.0 },
        pitch_max: if pitch_max > 0.0 { pitch_max } else { 1.0 },
        dist_min: f32::from(a.dist_min),
        dist_max: f32::from(a.dist_max),
        flags: Some(a.flags[0]),
        probability: f32::from(a.probability) / 255.0,
        start_delay: i32::from(a.start_delay),
        volume_falloff: curve(globals, a.volume_falloff_curve()),
        near_falloff: curve(globals, a.volume_min_falloff_curve()),
        voice_priority: Some(VoicePriority {
            thresholds: [a.min_priority_threshold, a.max_priority_threshold],
            values: [a.min_priority, a.max_priority],
            distance_max: f32::from(a.dist_max),
        }),
        envelop_min: f32::from(a.envelop_min),
        envelop_max: f32::from(a.envelop_max),
        envelop_percentage: f32::from(a.envelop_percentage) / 65535.0,
        limit_count: Some(a.limit_count),
        entity_limit_count: Some(a.entity_limit_count),
        ..CapturedAlias::default()
    }
}

/// What a T6 catalog build found.
#[derive(Clone, Debug, Default)]
pub struct T6SoundCensus {
    pub aliases: usize,
    pub variants: usize,
    pub with_clip: usize,
    pub without_clip: usize,
}

/// Build the engine's sound catalog from T6 banks (later banks override an
/// alias an earlier one defines). `index` says where each clip lives.
pub fn build_t6_sound_catalog(
    banks: &[&asset_t6::SndBankRef],
    globals: Option<&asset_t6::SndDriverGlobalsRef>,
    index: &T6BankIndex,
    zone: ZoneOwner,
) -> (SoundCatalog, T6SoundCensus) {
    let mut catalog = SoundCatalog::default();
    catalog.set_capture_game(ZoneGame::T6);
    catalog.set_capture_zone(zone);
    let mut census = T6SoundCensus::default();
    for bank in banks {
        for list in &bank.aliases {
            if list.name.is_empty() {
                continue;
            }
            let aliases: Vec<CapturedAlias> = list
                .entries
                .iter()
                .map(|a| alias_variant(a, &list.name, index, globals))
                .collect();
            census.aliases += 1;
            census.variants += aliases.len();
            let with = aliases.iter().filter(|a| a.streamed.is_some()).count();
            census.with_clip += with;
            census.without_clip += aliases.len() - with;
            catalog.ingest_sound(CapturedSound {
                name: list.name.clone(),
                aliases,
                game: ZoneGame::T6,
                zone,
            });
        }
    }
    catalog.finalize();
    (catalog, census)
}
