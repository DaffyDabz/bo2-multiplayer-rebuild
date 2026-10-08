//! The bank file: header, entry table, entry bytes.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::Pcm16;

pub const BANK_MAGIC: &[u8; 4] = b"2UX#";
pub const BANK_VERSION: u32 = 14;
pub const HEADER_SIZE: usize = 2048;
pub const ENTRY_SIZE: usize = 20;

/// Frame rates by `frameRateIndex`.
pub const FRAME_RATES: [u32; 9] = [
    8000, 12000, 16000, 24000, 32000, 44100, 48000, 96000, 192000,
];

/// `snd_asset_format`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundFormat {
    Pcm16,
    Pcm24,
    Pcm32,
    Ieee,
    Xma4,
    Mp3,
    MsAdpcm,
    Wma,
    Flac,
    WiiUAdpcm,
    Mpc,
    Unknown(u8),
}

impl SoundFormat {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Pcm16,
            1 => Self::Pcm24,
            2 => Self::Pcm32,
            3 => Self::Ieee,
            4 => Self::Xma4,
            5 => Self::Mp3,
            6 => Self::MsAdpcm,
            7 => Self::Wma,
            8 => Self::Flac,
            9 => Self::WiiUAdpcm,
            10 => Self::Mpc,
            other => Self::Unknown(other),
        }
    }
}

/// `SND_HashName`: the hash an alias's `id` and `assetId` use (sdbm-like,
/// lower-cased, seed 0x1505; 0 for an empty name, never 0 otherwise).
pub fn sound_hash(name: &str) -> u32 {
    if name.is_empty() {
        return 0;
    }
    let mut h: u32 = 0x1505;
    for b in name.bytes() {
        h = u32::from(b.to_ascii_lowercase()).wrapping_add(h.wrapping_mul(0x1003f));
    }
    if h == 0 { 1 } else { h }
}

/// `SndAssetBankHeader`.
#[derive(Clone, Debug)]
pub struct BankHeader {
    pub version: u32,
    pub entry_size: u32,
    pub entry_count: u32,
    pub dependency_count: u32,
    pub file_size: u64,
    pub entry_offset: u64,
    /// Bank names this one depends on (its own name first).
    pub dependencies: Vec<String>,
}

/// `SndAssetBankEntry`.
#[derive(Clone, Copy, Debug)]
pub struct BankEntry {
    pub id: u32,
    pub size: u32,
    pub offset: u32,
    pub frame_count: u32,
    pub frame_rate_index: u8,
    pub channel_count: u8,
    pub looping: bool,
    pub format: SoundFormat,
}

impl BankEntry {
    pub fn rate(&self) -> u32 {
        FRAME_RATES
            .get(usize::from(self.frame_rate_index))
            .copied()
            .unwrap_or(48000)
    }
}

/// One opened bank file: its header and entries. Entry bytes are read on
/// demand.
#[derive(Clone, Debug)]
pub struct BankFile {
    pub path: PathBuf,
    pub header: BankHeader,
    pub entries: Vec<BankEntry>,
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from(u32_at(b, at)) | (u64::from(u32_at(b, at + 4)) << 32)
}

impl BankFile {
    pub fn open(path: &Path) -> Result<Self, String> {
        let mut file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut head = vec![0u8; HEADER_SIZE];
        file.read_exact(&mut head)
            .map_err(|e| format!("{}: header: {e}", path.display()))?;
        let header = parse_header(&head).map_err(|e| format!("{}: {e}", path.display()))?;
        let table_len = header.entry_count as usize * header.entry_size as usize;
        file.seek(SeekFrom::Start(header.entry_offset))
            .map_err(|e| format!("{}: entry table: {e}", path.display()))?;
        let mut table = vec![0u8; table_len];
        file.read_exact(&mut table)
            .map_err(|e| format!("{}: entry table: {e}", path.display()))?;
        let entries = parse_entries(&table, header.entry_size as usize);
        Ok(Self {
            path: path.to_path_buf(),
            header,
            entries,
        })
    }

    /// One entry's bytes.
    pub fn read(&self, entry: &BankEntry) -> Result<Vec<u8>, String> {
        let mut file =
            File::open(&self.path).map_err(|e| format!("{}: {e}", self.path.display()))?;
        file.seek(SeekFrom::Start(u64::from(entry.offset)))
            .map_err(|e| format!("{}: seek: {e}", self.path.display()))?;
        let mut data = vec![0u8; entry.size as usize];
        file.read_exact(&mut data)
            .map_err(|e| format!("{}: entry {:#x}: {e}", self.path.display(), entry.id))?;
        Ok(data)
    }

    /// One entry decoded to 16-bit PCM.
    pub fn decode(&self, entry: &BankEntry) -> Result<Pcm16, String> {
        let data = self.read(entry)?;
        decode_entry(entry, &data)
    }
}

/// Decode an entry's bytes (16-bit PCM as stored, or FLAC).
pub fn decode_entry(entry: &BankEntry, data: &[u8]) -> Result<Pcm16, String> {
    match entry.format {
        SoundFormat::Pcm16 => {
            let samples: Vec<i16> = data
                .chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]))
                .collect();
            Ok(Pcm16 {
                rate: entry.rate(),
                channels: u16::from(entry.channel_count.max(1)),
                samples,
            })
        }
        SoundFormat::Flac => crate::flac::decode_flac(data)
            .map(|s| s.pcm)
            .map_err(|e| format!("entry {:#x}: flac: {e}", entry.id)),
        other => Err(format!("entry {:#x}: format {other:?} not decoded", entry.id)),
    }
}

pub fn parse_header(head: &[u8]) -> Result<BankHeader, String> {
    if head.len() < HEADER_SIZE {
        return Err("short header".into());
    }
    if &head[0..4] != BANK_MAGIC {
        return Err(format!("not a sound bank: {:02x?}", &head[0..4]));
    }
    let version = u32_at(head, 4);
    if version != BANK_VERSION {
        return Err(format!("bank version {version}, want {BANK_VERSION}"));
    }
    let entry_size = u32_at(head, 8);
    if entry_size as usize != ENTRY_SIZE {
        return Err(format!("entry size {entry_size}, want {ENTRY_SIZE}"));
    }
    let dependency_size = u32_at(head, 16) as usize;
    let dependency_count = u32_at(head, 24);
    let mut dependencies = Vec::new();
    for i in 0..dependency_count as usize {
        let at = 72 + i * dependency_size;
        let Some(raw) = head.get(at..at + dependency_size) else {
            break;
        };
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        if end > 0 {
            dependencies.push(String::from_utf8_lossy(&raw[..end]).into_owned());
        }
    }
    Ok(BankHeader {
        version,
        entry_size,
        entry_count: u32_at(head, 20),
        dependency_count,
        file_size: u64_at(head, 32),
        entry_offset: u64_at(head, 40),
        dependencies,
    })
}

pub fn parse_entries(table: &[u8], entry_size: usize) -> Vec<BankEntry> {
    table
        .chunks_exact(entry_size)
        .map(|e| BankEntry {
            id: u32_at(e, 0),
            size: u32_at(e, 4),
            offset: u32_at(e, 8),
            frame_count: u32_at(e, 12),
            frame_rate_index: e[16],
            channel_count: e[17],
            looping: e[18] != 0,
            format: SoundFormat::from_u8(e[19]),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_matches_the_seed_rule() {
        assert_eq!(sound_hash(""), 0);
        // One character: 'a' + 0x1003f * 0x1505.
        assert_eq!(
            sound_hash("a"),
            0x61u32.wrapping_add(0x1505u32.wrapping_mul(0x1003f))
        );
        assert_eq!(sound_hash("ABC"), sound_hash("abc"));
    }
}
