//! Black Ops II image packs on disk: open their indexes, find a GfxImage's
//! entry, read and decode it. Packs are only read.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ipak_t6::{IndexEntry, decode_entry, parse_header, parse_index};

use crate::capture::ImageRef;

pub struct Pack {
    pub name: String,
    pub path: PathBuf,
    data_offset: u64,
    index: Vec<IndexEntry>,
    file: Mutex<File>,
}

impl Pack {
    pub fn open(path: &Path) -> Result<Pack, String> {
        let mut file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let head = read_at(&mut file, 0, 256)?;
        let header = parse_header(&head).map_err(|e| format!("{}: {e}", path.display()))?;
        let bytes = read_at(
            &mut file,
            header.index.offset as u64,
            header.index.item_count as usize * ipak_t6::INDEX_ENTRY_LEN,
        )?;
        let index = parse_index(&bytes, header.index.item_count as usize)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Pack {
            name: path
                .file_stem()
                .map_or_else(String::new, |s| s.to_string_lossy().into_owned()),
            path: path.to_owned(),
            data_offset: header.data.offset as u64,
            index,
            file: Mutex::new(file),
        })
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    pub fn find(&self, name_hash: u32, data_hash: u32) -> Option<IndexEntry> {
        let key = IndexEntry::key_of(name_hash, data_hash);
        self.index
            .binary_search_by_key(&key, |e| e.key)
            .ok()
            .map(|i| self.index[i])
    }

    /// Every entry stored under an image name's hash (any data hash).
    pub fn find_name(&self, name_hash: u32) -> Vec<IndexEntry> {
        let lo = IndexEntry::key_of(name_hash, 0);
        let start = self.index.partition_point(|e| e.key < lo);
        self.index[start..].iter().take_while(|e| e.name_hash() == name_hash).copied().collect()
    }

    /// The decoded entry: an IWI v27 file.
    pub fn read(&self, entry: IndexEntry) -> Result<Vec<u8>, String> {
        let at = self.data_offset + entry.offset as u64;
        let stored = {
            let mut file = self
                .file
                .lock()
                .map_err(|_| "pack file lock poisoned".to_owned())?;
            read_at(&mut file, at, entry.size as usize)?
        };
        let mut out = Vec::new();
        decode_entry(&stored, at, &mut out).map_err(|e| format!("{}: {e}", self.name))?;
        Ok(out)
    }
}

fn read_at(file: &mut File, at: u64, len: usize) -> Result<Vec<u8>, String> {
    file.seek(SeekFrom::Start(at)).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; len];
    let mut got = 0;
    while got < len {
        match file.read(&mut buf[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) => return Err(e.to_string()),
        }
    }
    buf.truncate(got);
    Ok(buf)
}

/// Every pack of an install's zone folder, searched in priority order:
/// patches and map packs before the shared ones.
pub struct PackSet {
    pub packs: Vec<Pack>,
}

/// Where an image's pixels are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageSource {
    /// In a pack: which pack (index into `PackSet::packs`) and entry.
    Pack(usize, IndexEntry),
    /// In the zone itself.
    Embedded,
    /// Not streamed and not embedded: nothing to draw.
    Empty,
}

impl PackSet {
    pub fn open_dir(dir: &Path) -> Result<PackSet, String> {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("ipak"))
            })
            .collect();
        paths.sort_by_key(|p| priority(p));
        let packs = paths
            .iter()
            .map(|p| Pack::open(p))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(PackSet { packs })
    }

    /// An image by its name alone (a zone's `,name` reference to an image
    /// another zone defines): the first pack holding that name, in
    /// priority order, and its largest entry (the full mip chain).
    pub fn locate_name(&self, name: &str) -> Option<(usize, IndexEntry)> {
        let h = ipak_t6::hash_name(name.trim_start_matches(','));
        self.packs
            .iter()
            .enumerate()
            .find_map(|(i, p)| p.find_name(h).into_iter().max_by_key(|e| e.size).map(|e| (i, e)))
    }

    pub fn locate(&self, image: &ImageRef) -> Option<ImageSource> {
        if image.embedded.is_some() {
            return Some(ImageSource::Embedded);
        }
        if image.streamed_parts == 0 {
            return Some(ImageSource::Empty);
        }
        self.packs.iter().enumerate().find_map(|(i, p)| {
            p.find(image.name_hash, image.part_hash)
                .map(|e| ImageSource::Pack(i, e))
        })
    }
}

/// Lower sorts first: zone patches, then map packs, then shared packs.
fn priority(path: &Path) -> (u8, String) {
    let stem = path
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().to_ascii_lowercase());
    let rank = if stem.ends_with("_patch") {
        0
    } else if stem.starts_with("zm_") || stem.starts_with("mp_") {
        1
    } else if stem.starts_with("patch") {
        2
    } else if stem.starts_with("dlc") {
        3
    } else if stem == "base" {
        5
    } else {
        4
    };
    (rank, stem)
}
