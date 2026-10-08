//! bo2zm: Black Ops II (PC) image packs (`.ipak`) and the IWI v27 images
//! inside them. Not part of upstream IW4L.
//!
//! Pack layout (little endian): a 16-byte header (`KAPI`, version 0x50000,
//! file size, section count), then 16-byte section records (type, offset,
//! size, item count). The index section holds 16-byte entries (data hash,
//! name hash, offset into the data section, stored size); the data section
//! holds the entries. An entry is a run of 128-byte-aligned blocks: a block
//! header (u32: file offset of its first output byte in the low 24 bits,
//! command count in the high 8; then 31 u32 commands: size in the low 24
//! bits, kind in the high 8) followed by the commands' payloads back to back.
//! Kind 0 copies, 1 is LZO1X, anything else skips its payload.
//!
//! A zone's GfxImage finds its pixels by (its name hash, its streamed part's
//! data hash); the decoded entry is an IWI v27 file.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

mod iwi;
mod lzo;

pub use iwi::{IWI_HEADER_LEN, IwiError, IwiFormat, IwiImage, parse_iwi};
pub use lzo::{LzoError, decompress as lzo1x_decompress};

use alloc::vec::Vec;

pub const MAGIC: &[u8; 4] = b"KAPI";
pub const VERSION: u32 = 0x50000;
pub const HEADER_LEN: usize = 16;
pub const SECTION_LEN: usize = 16;
pub const INDEX_ENTRY_LEN: usize = 16;
pub const SECTION_INDEX: u32 = 1;
pub const SECTION_DATA: u32 = 2;
pub const BLOCK_HEADER_LEN: usize = 128;
pub const BLOCK_COMMANDS: usize = 31;
/// The largest output one command may produce.
pub const COMMAND_OUTPUT_MAX: usize = 0x8000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IpakError {
    TooShort { need: usize, have: usize },
    BadMagic([u8; 4]),
    BadVersion(u32),
    NoSection(u32),
    BadBlock { at: u64, count: u32 },
    BlockOffset { at: u64, says: u32, have: usize },
    Lzo { at: u64, error: LzoError },
    Truncated { at: u64 },
}

impl core::fmt::Display for IpakError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            IpakError::TooShort { need, have } => write!(f, "ipak needs {need} bytes, has {have}"),
            IpakError::BadMagic(m) => write!(f, "not an ipak: magic {m:02x?}"),
            IpakError::BadVersion(v) => write!(f, "ipak version {v:#x} (want {VERSION:#x})"),
            IpakError::NoSection(t) => write!(f, "ipak has no section of type {t}"),
            IpakError::BadBlock { at, count } => {
                write!(f, "ipak block at {at:#x} claims {count} commands")
            }
            IpakError::BlockOffset { at, says, have } => write!(
                f,
                "ipak block at {at:#x} continues output at {says:#x}, but {have:#x} bytes came before"
            ),
            IpakError::Lzo { at, error } => write!(f, "ipak command at {at:#x}: {error}"),
            IpakError::Truncated { at } => write!(f, "ipak entry ends inside a block at {at:#x}"),
        }
    }
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Section {
    pub kind: u32,
    pub offset: u32,
    pub size: u32,
    pub item_count: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IpakHeader {
    pub file_size: u32,
    pub index: Section,
    pub data: Section,
}

/// Parse the header and section table from the first bytes of a pack
/// (`HEADER_LEN + section_count * SECTION_LEN` of them; 256 always suffice
/// for retail packs).
pub fn parse_header(bytes: &[u8]) -> Result<IpakHeader, IpakError> {
    if bytes.len() < HEADER_LEN {
        return Err(IpakError::TooShort {
            need: HEADER_LEN,
            have: bytes.len(),
        });
    }
    let mut magic = [0u8; 4];
    magic.copy_from_slice(&bytes[0..4]);
    if &magic != MAGIC {
        return Err(IpakError::BadMagic(magic));
    }
    let version = u32_at(bytes, 4);
    if version != VERSION {
        return Err(IpakError::BadVersion(version));
    }
    let file_size = u32_at(bytes, 8);
    let count = u32_at(bytes, 12) as usize;
    let need = HEADER_LEN + count * SECTION_LEN;
    if bytes.len() < need {
        return Err(IpakError::TooShort {
            need,
            have: bytes.len(),
        });
    }
    let mut index = None;
    let mut data = None;
    for i in 0..count {
        let at = HEADER_LEN + i * SECTION_LEN;
        let s = Section {
            kind: u32_at(bytes, at),
            offset: u32_at(bytes, at + 4),
            size: u32_at(bytes, at + 8),
            item_count: u32_at(bytes, at + 12),
        };
        match s.kind {
            SECTION_INDEX => index = Some(s),
            SECTION_DATA => data = Some(s),
            _ => {}
        }
    }
    Ok(IpakHeader {
        file_size,
        index: index.ok_or(IpakError::NoSection(SECTION_INDEX))?,
        data: data.ok_or(IpakError::NoSection(SECTION_DATA))?,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct IndexEntry {
    pub key: u64,
    /// Offset into the data section.
    pub offset: u32,
    /// Stored bytes, block headers included.
    pub size: u32,
}

impl IndexEntry {
    pub const fn key_of(name_hash: u32, data_hash: u32) -> u64 {
        (name_hash as u64) << 32 | data_hash as u64
    }

    pub fn name_hash(&self) -> u32 {
        (self.key >> 32) as u32
    }

    pub fn data_hash(&self) -> u32 {
        self.key as u32
    }
}

/// Parse the index section's bytes; entries come back sorted by key.
pub fn parse_index(bytes: &[u8], count: usize) -> Result<Vec<IndexEntry>, IpakError> {
    let need = count * INDEX_ENTRY_LEN;
    if bytes.len() < need {
        return Err(IpakError::TooShort {
            need,
            have: bytes.len(),
        });
    }
    let mut out: Vec<IndexEntry> = (0..count)
        .map(|i| {
            let at = i * INDEX_ENTRY_LEN;
            let data_hash = u32_at(bytes, at);
            let name_hash = u32_at(bytes, at + 4);
            IndexEntry {
                key: IndexEntry::key_of(name_hash, data_hash),
                offset: u32_at(bytes, at + 8),
                size: u32_at(bytes, at + 12),
            }
        })
        .collect();
    out.sort();
    Ok(out)
}

/// Decode one stored entry. `stored` is the entry's bytes and `file_at` the
/// pack-file offset they start at (blocks align to 128 bytes of the file).
pub fn decode_entry(stored: &[u8], file_at: u64, out: &mut Vec<u8>) -> Result<(), IpakError> {
    let mut pos = 0usize;
    loop {
        let abs = file_at + pos as u64;
        let aligned = abs.next_multiple_of(BLOCK_HEADER_LEN as u64);
        pos += (aligned - abs) as usize;
        if pos >= stored.len() {
            return Ok(());
        }
        let at = file_at + pos as u64;
        let header = stored
            .get(pos..pos + BLOCK_HEADER_LEN)
            .ok_or(IpakError::Truncated { at })?;
        let first = u32_at(header, 0);
        let offset = first & 0x00FF_FFFF;
        let count = first >> 24;
        if count as usize > BLOCK_COMMANDS {
            return Err(IpakError::BadBlock { at, count });
        }
        let carries_data = (0..count as usize).any(|c| u32_at(header, 4 + c * 4) >> 24 <= 1);
        if carries_data && offset as usize != out.len() & 0x00FF_FFFF {
            return Err(IpakError::BlockOffset {
                at,
                says: offset,
                have: out.len(),
            });
        }
        pos += BLOCK_HEADER_LEN;
        for c in 0..count as usize {
            let cmd = u32_at(header, 4 + c * 4);
            let size = (cmd & 0x00FF_FFFF) as usize;
            let kind = cmd >> 24;
            let at = file_at + pos as u64;
            let payload = stored
                .get(pos..pos + size)
                .ok_or(IpakError::Truncated { at })?;
            match kind {
                0 => out.extend_from_slice(payload),
                1 => {
                    let limit = out.len() + COMMAND_OUTPUT_MAX;
                    lzo::decompress(payload, out, limit)
                        .map_err(|error| IpakError::Lzo { at, error })?;
                }
                _ => {}
            }
            pos += size;
        }
    }
}

/// Black Ops II's string hash for asset names (`R_HashString`, seed 0):
/// lower-cased ASCII, `h = h * 33 ^ c`.
pub fn hash_name(name: &str) -> u32 {
    let mut h = 0u32;
    for &c in name.as_bytes() {
        h = h.wrapping_mul(33) ^ (c as u32 | 0x20);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn block(offset: u32, cmds: &[(u32, &[u8])]) -> Vec<u8> {
        let mut b = vec![0u8; BLOCK_HEADER_LEN];
        b[0..4].copy_from_slice(&(offset | (cmds.len() as u32) << 24).to_le_bytes());
        for (i, (kind, data)) in cmds.iter().enumerate() {
            let cmd = data.len() as u32 | kind << 24;
            b[4 + i * 4..8 + i * 4].copy_from_slice(&cmd.to_le_bytes());
        }
        for (_, data) in cmds {
            b.extend_from_slice(data);
        }
        b
    }

    #[test]
    fn blocks_copy_decompress_and_skip() {
        let mut stored = block(
            0,
            &[
                (0, b"abc"),
                (0xCF, b"zz"),
                (1, &[22, 1, 2, 3, 4, 5, 0x11, 0, 0]),
            ],
        );
        stored.resize(stored.len().next_multiple_of(BLOCK_HEADER_LEN), 0);
        stored.extend(block(8, &[(0, b"!")]));
        let mut out = vec![];
        decode_entry(&stored, 0x1000, &mut out).unwrap();
        assert_eq!(out, [b'a', b'b', b'c', 1, 2, 3, 4, 5, b'!']);
    }

    #[test]
    fn a_block_that_skips_ahead_is_refused() {
        let stored = block(5, &[(0, b"x")]);
        let mut out = vec![];
        assert!(matches!(
            decode_entry(&stored, 0, &mut out),
            Err(IpakError::BlockOffset { .. })
        ));
    }

    #[test]
    fn index_entries_sort_by_name_then_data_hash() {
        let mut bytes = vec![];
        for (d, n) in [(2u32, 9u32), (1, 9), (7, 3)] {
            bytes.extend_from_slice(&d.to_le_bytes());
            bytes.extend_from_slice(&n.to_le_bytes());
            bytes.extend_from_slice(&0u32.to_le_bytes());
            bytes.extend_from_slice(&0u32.to_le_bytes());
        }
        let idx = parse_index(&bytes, 3).unwrap();
        assert_eq!(idx[0].name_hash(), 3);
        assert_eq!((idx[1].name_hash(), idx[1].data_hash()), (9, 1));
        assert_eq!((idx[2].name_hash(), idx[2].data_hash()), (9, 2));
    }

    #[test]
    fn name_hash_lowercases() {
        assert_eq!(hash_name("ABC"), hash_name("abc"));
        assert_ne!(hash_name("abc"), hash_name("abd"));
        assert_eq!(hash_name(""), 0);
    }
}
