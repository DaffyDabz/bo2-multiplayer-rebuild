//! The XAssetList at the start of a T6 zone's content.
//!
//! Layout (24 bytes, read off the stream into no block): script-string count
//! and pointer, dependency count and pointer, asset count and pointer. With
//! the VIRTUAL block pushed, the three arrays follow in that order: the
//! script-string pointer array then each string that follows inline, the
//! dependency-name pointer array then its strings, then the XAsset array
//! (u32 pool id + u32 header pointer per entry). The asset bodies come after
//! that, in array order.

use crate::asset_type::AssetType;
use crate::zone::{Ptr, Result, XFILE_BLOCK_VIRTUAL, ZoneError, ZonePtr, ZoneStream};

pub const XASSET_LIST_LEN: usize = 24;
pub const XASSET_ENTRY_LEN: usize = 8;

/// An array of string pointers loaded into the VIRTUAL block, with every
/// entry rewritten to an offset (or 0 for a null string).
#[derive(Clone, Copy, Debug, Default)]
pub struct StringList {
    array: Option<Ptr>,
    count: usize,
}

impl StringList {
    pub fn count(&self) -> usize {
        self.count
    }

    pub fn array(&self) -> Option<Ptr> {
        self.array
    }

    pub fn get<'s>(&self, s: &'s ZoneStream<'_>, i: usize) -> Option<&'s str> {
        let arr = self.array?;
        if i >= self.count {
            return None;
        }
        match s.ptr_at(arr, i * 4).ok()? {
            ZonePtr::Offset(p) => s.cstr(s.resolve_alias(p)).ok(),
            ZonePtr::Null => Some(""),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AssetTable {
    pub strings: StringList,
    pub depends: StringList,
    array: Option<Ptr>,
    count: usize,
}

impl AssetTable {
    pub fn count(&self) -> usize {
        self.count
    }

    pub fn array(&self) -> Option<Ptr> {
        self.array
    }

    pub fn raw_kind(&self, s: &ZoneStream<'_>, i: usize) -> Result<u32> {
        let arr = self.array.ok_or(ZoneError::NoBlockPushed)?;
        if i >= self.count {
            return Err(ZoneError::BadOffset {
                block: XFILE_BLOCK_VIRTUAL,
                offset: i,
                size: self.count,
            });
        }
        s.u32_at(arr, i * XASSET_ENTRY_LEN)
    }

    pub fn kind(&self, s: &ZoneStream<'_>, i: usize) -> Result<AssetType> {
        let raw = self.raw_kind(s, i)?;
        AssetType::from_u32(raw).ok_or(ZoneError::UnknownAssetType(raw))
    }

    /// The header pointer slot of asset `i` (where its body pointer lives).
    pub fn slot(&self, i: usize) -> Option<Ptr> {
        let arr = self.array?;
        (i < self.count).then(|| arr.at(i * XASSET_ENTRY_LEN + 4))
    }
}

fn following_list(raw: u32, what: &'static str) -> Result<bool> {
    match ZonePtr::decode(raw) {
        ZonePtr::Null => Ok(false),
        ZonePtr::Following => Ok(true),
        _ => Err(ZoneError::NotFollowing { what, raw }),
    }
}

/// Read the XAssetList head and its three arrays. Leaves the VIRTUAL block
/// pushed: the asset bodies load under it, and the walk pops it at the end.
pub fn open_asset_table(s: &mut ZoneStream<'_>) -> Result<AssetTable> {
    let head = s.read_raw(XASSET_LIST_LEN)?;
    let rd = |o: usize| u32::from_le_bytes([head[o], head[o + 1], head[o + 2], head[o + 3]]);
    let string_count = rd(0) as usize;
    let strings_raw = rd(4);
    let depend_count = rd(8) as usize;
    let depends_raw = rd(12);
    let asset_count = rd(16) as usize;
    let assets_raw = rd(20);

    s.push(XFILE_BLOCK_VIRTUAL)?;

    let strings = if following_list(strings_raw, "script string list")? {
        load_string_list(s, string_count)?
    } else {
        StringList::default()
    };
    let depends = if following_list(depends_raw, "dependency list")? {
        load_string_list(s, depend_count)?
    } else {
        StringList::default()
    };
    let array = if following_list(assets_raw, "asset list")? {
        Some(s.alloc_load(4, XASSET_ENTRY_LEN * asset_count)?)
    } else {
        None
    };

    Ok(AssetTable {
        strings,
        depends,
        array,
        count: if array.is_some() { asset_count } else { 0 },
    })
}

fn load_string_list(s: &mut ZoneStream<'_>, count: usize) -> Result<StringList> {
    let arr = s.alloc_load(4, 4 * count)?;
    for i in 0..count {
        let resolved = match s.ptr_at(arr, i * 4)? {
            ZonePtr::Null => None,
            ZonePtr::Offset(q) => {
                s.note_offset(q);
                Some(s.resolve_alias(q))
            }
            _ => {
                s.begin_body(arr.at(i * 4))?;
                Some(s.load_string()?)
            }
        };
        let encoded = match resolved {
            Some(q) => ZonePtr::encode_offset(q),
            None => 0,
        };
        s.write_u32_at(arr, i * 4, encoded)?;
    }
    Ok(StringList {
        array: Some(arr),
        count,
    })
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    use super::*;
    use crate::zone::{MAX_XFILE_COUNT, XFILE_HEADER_LEN};
    use alloc::vec;
    use alloc::vec::Vec;

    /// A zone image with two script strings (one inline, one null), one
    /// dependency and two assets.
    fn image() -> Vec<u8> {
        let mut out = vec![0u8; XFILE_HEADER_LEN];
        // VIRTUAL block size.
        out[8 + 4 * XFILE_BLOCK_VIRTUAL..12 + 4 * XFILE_BLOCK_VIRTUAL]
            .copy_from_slice(&256u32.to_le_bytes());
        let w = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_le_bytes());
        // XAssetList head.
        w(&mut out, 2);
        w(&mut out, u32::MAX);
        w(&mut out, 1);
        w(&mut out, u32::MAX);
        w(&mut out, 2);
        w(&mut out, u32::MAX);
        // Script strings: pointer array, then the one that follows.
        w(&mut out, u32::MAX);
        w(&mut out, 0);
        out.extend_from_slice(b"zombie\0");
        // Dependencies.
        w(&mut out, u32::MAX);
        out.extend_from_slice(b"common_zm\0");
        // Assets: rawfile, zbarrier.
        w(&mut out, AssetType::RawFile as u32);
        w(&mut out, u32::MAX);
        w(&mut out, AssetType::ZBarrier as u32);
        w(&mut out, u32::MAX);
        out
    }

    #[test]
    fn asset_list_reads_strings_depends_and_kinds() {
        let image = image();
        let mut storage: [Vec<u8>; MAX_XFILE_COUNT] = core::array::from_fn(|_| vec![0u8; 256]);
        let mut insert_map = vec![0u8; 16];
        let blocks = {
            let [a, b, c, d, e, f, g, h] = &mut storage;
            [
                a.as_mut_slice(),
                b.as_mut_slice(),
                c.as_mut_slice(),
                d.as_mut_slice(),
                e.as_mut_slice(),
                f.as_mut_slice(),
                g.as_mut_slice(),
                h.as_mut_slice(),
            ]
        };
        let mut s = ZoneStream::new(&image, blocks, &mut insert_map).unwrap();
        let table = open_asset_table(&mut s).unwrap();
        assert_eq!(table.strings.count(), 2);
        assert_eq!(table.strings.get(&s, 0), Some("zombie"));
        assert_eq!(table.strings.get(&s, 1), Some(""));
        assert_eq!(table.depends.get(&s, 0), Some("common_zm"));
        assert_eq!(table.count(), 2);
        assert_eq!(table.kind(&s, 0), Ok(AssetType::RawFile));
        assert_eq!(table.kind(&s, 1), Ok(AssetType::ZBarrier));
        assert_eq!(s.remaining(), 0);
    }
}
