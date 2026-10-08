//! bo2zm: retail T6 (Black Ops II, PC) zone reading. Not part of upstream
//! IW4L; added by the bo2zm fork.
//!
//! The file is a 12-byte preamble, an auth header on signed zones
//! (`envelope`), then XChunk records that decrypt and inflate into the zone
//! image (`xchunk`; inflating is the transport's job, like the other lanes).
//! The image is an XFile header and the content stream (`zone`), which
//! starts with the XAssetList (`content`).

#![no_std]
#![forbid(unsafe_code)]

mod asset_type;
mod content;
mod envelope;
mod salsa20;
mod sha1;
mod walk;
#[rustfmt::skip]
mod walk_gen;
/// Generated T6 structure layouts: `layout::GfxImage::name` is a byte offset.
#[rustfmt::skip]
pub mod layout_gen;
pub use layout_gen as layout;
mod xchunk;
mod zone;

pub use asset_type::{ASSET_TYPE_COUNT, AssetType};
pub use content::{AssetTable, StringList, XASSET_ENTRY_LEN, XASSET_LIST_LEN, open_asset_table};
pub use envelope::{
    AUTH_HEADER_LEN, AUTH_MAGIC, AUTH_NAME_LEN, AUTH_SIGNATURE_LEN, FILE_PREAMBLE_LEN, FileHeader,
    FileHeaderError, MAGIC_SIGNED, MAGIC_UNSIGNED, MAGIC_UNSIGNED_SERVER, Signing,
    ZONE_NAME_KEY_MAX, ZONE_VERSION_PC, is_t6_magic, parse_file_header,
};
pub use walk::{Loaded, NoSink, WalkSink, Walker};
pub use xchunk::{
    ChunkCipher, HASH_TABLE_LEN, SALSA20_KEY_PC, STREAM_COUNT, VANILLA_BUFFER_SIZE, XCHUNK_SIZE,
    XChunk, XChunkError, XChunks,
};
pub use zone::{
    BLOCK_STACK_CAP, BLOCK_TYPES, BlockType, MAX_XFILE_COUNT, PTR_SIZE, Ptr, Result,
    XFILE_BLOCK_DELAY_PHYSICAL, XFILE_BLOCK_DELAY_VIRTUAL, XFILE_BLOCK_NAMES, XFILE_BLOCK_PHYSICAL,
    XFILE_BLOCK_RUNTIME_PHYSICAL, XFILE_BLOCK_RUNTIME_VIRTUAL, XFILE_BLOCK_STREAMER_RESERVE,
    XFILE_BLOCK_TEMP, XFILE_BLOCK_VIRTUAL, XFILE_HEADER_LEN, ZoneError, ZoneHeader, ZonePtr,
    ZoneStream, block_is_aliasable, parse_zone_header,
};
