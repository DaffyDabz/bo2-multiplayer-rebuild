use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use asset_core::ZoneGame;
pub use fastfile_iw4::Iw4WireFormat;
use fastfile_iw4::{
    FileHeaderError as Iw4HeaderError, MAGIC_AUTH_HEADER, MAX_XFILE_COUNT, Signing as Iw4Signing,
    ZoneHeader, ZoneStream, parse_file_header as parse_iw4_header, parse_zone_header,
};
use fastfile_iw4::{WireAssetTable, WireTableError, read_wire_asset_table};
use fastfile_iw5::Signing as Iw5Signing;

const AUTHED_CHUNK_SIZE: usize = 0x2000;

const AUTHED_CHUNKS_PER_GROUP: usize = 256;

#[derive(Debug)]
pub enum ZoneOpenError {
    Io(std::io::Error),
    Iw4Header(Iw4HeaderError),
    T5Header(fastfile_t5::FileHeaderError),
    Iw5Header(fastfile_iw5::FileHeaderError),
    // bo2zm: Black Ops II zones.
    T6Header(fastfile_t6::FileHeaderError),
    T6Chunk(fastfile_t6::XChunkError),
    T6NoZoneName,
    T6Inflate {
        chunk: usize,
        message: String,
    },

    UnsupportedVersion {
        got: u32,
    },

    BadAuthHeader,
    Inflate(String),

    Truncated,
    Iw4WireLayout {
        x86: WireTableError,
        x64: WireTableError,
    },
    AmbiguousIw4WireLayout,
    AtPath {
        path: std::path::PathBuf,
        source: Box<ZoneOpenError>,
    },

    WrongGame {
        expected: ZoneGame,
        got: ZoneGame,
    },
}

impl std::fmt::Display for ZoneOpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ZoneOpenError::Io(e) => write!(f, "reading zone: {e}"),
            ZoneOpenError::Iw4Header(e) => write!(f, "bad IW4 fastfile header: {e:?}"),
            ZoneOpenError::T5Header(e) => write!(f, "bad T5 fastfile header: {e:?}"),
            ZoneOpenError::Iw5Header(e) => write!(f, "bad IW5 fastfile header: {e:?}"),
            ZoneOpenError::T6Header(e) => write!(f, "bad T6 fastfile header: {e}"),
            ZoneOpenError::T6Chunk(e) => write!(f, "bad T6 xchunk: {e}"),
            ZoneOpenError::T6NoZoneName => {
                write!(f, "encrypted T6 zone has no zone name to key its chunks")
            }
            ZoneOpenError::T6Inflate { chunk, message } => {
                write!(f, "T6 xchunk {chunk} does not inflate: {message}")
            }
            ZoneOpenError::UnsupportedVersion { got } => {
                write!(
                    f,
                    "unsupported fastfile version {got:#x} (want IW4 0x114, T5 0x1d9, IW5 0x1, or T6 0x93)"
                )
            }
            ZoneOpenError::BadAuthHeader => {
                write!(f, "signed zone without a valid {} auth header", {
                    std::str::from_utf8(MAGIC_AUTH_HEADER).unwrap_or("auth")
                })
            }
            ZoneOpenError::Inflate(e) => write!(f, "inflate failed: {e}"),
            ZoneOpenError::Truncated => write!(f, "inflated zone is shorter than its XFile header"),
            ZoneOpenError::Iw4WireLayout { x86, x64 } => write!(
                f,
                "invalid IW4 asset table: x86 candidate {x86:?}; x64 candidate {x64:?}"
            ),
            ZoneOpenError::AmbiguousIw4WireLayout => write!(
                f,
                "ambiguous IW4 asset table: both x86 and x64 layouts validate"
            ),
            ZoneOpenError::AtPath { path, source } => write!(f, "{}: {source}", path.display()),
            ZoneOpenError::WrongGame { expected, got } => {
                write!(f, "zone game mismatch: expected {expected:?}, got {got:?}")
            }
        }
    }
}

impl std::error::Error for ZoneOpenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::AtPath { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub struct ZoneImage {
    pub game: ZoneGame,
    pub version: u32,
    pub bytes: Vec<u8>,

    pub iw4_wire_format: Option<Iw4WireFormat>,
}

impl ZoneImage {
    pub fn iw4_table(&self) -> Result<WireAssetTable<'_>, ZoneOpenError> {
        if self.game != ZoneGame::Iw4 {
            return Err(ZoneOpenError::WrongGame {
                expected: ZoneGame::Iw4,
                got: self.game,
            });
        }
        select_iw4_table(&self.bytes)
    }

    pub fn header(&self) -> Result<ZoneHeader, ZoneOpenError> {
        match self.game {
            ZoneGame::Iw4 => {
                self.iw4_table()?;
                parse_zone_header(&self.bytes).map_err(|_| ZoneOpenError::Truncated)
            }
            other => Err(ZoneOpenError::WrongGame {
                expected: ZoneGame::Iw4,
                got: other,
            }),
        }
    }

    pub fn t5_header(&self) -> Result<fastfile_t5::ZoneHeader, ZoneOpenError> {
        match self.game {
            ZoneGame::T5 => {
                fastfile_t5::parse_zone_header(&self.bytes).map_err(|_| ZoneOpenError::Truncated)
            }
            other => Err(ZoneOpenError::WrongGame {
                expected: ZoneGame::T5,
                got: other,
            }),
        }
    }

    /// bo2zm: the XFile header of a Black Ops II zone image.
    pub fn t6_header(&self) -> Result<fastfile_t6::ZoneHeader, ZoneOpenError> {
        match self.game {
            ZoneGame::T6 => {
                fastfile_t6::parse_zone_header(&self.bytes).map_err(|_| ZoneOpenError::Truncated)
            }
            other => Err(ZoneOpenError::WrongGame {
                expected: ZoneGame::T6,
                got: other,
            }),
        }
    }

    pub fn iw5_header(&self) -> Result<fastfile_iw5::ZoneHeader, ZoneOpenError> {
        match self.game {
            ZoneGame::Iw5 => {
                fastfile_iw5::parse_zone_header(&self.bytes).map_err(|_| ZoneOpenError::Truncated)
            }
            other => Err(ZoneOpenError::WrongGame {
                expected: ZoneGame::Iw5,
                got: other,
            }),
        }
    }
}

pub fn open_zone_shared(path: impl AsRef<Path>) -> Result<Arc<ZoneImage>, ZoneOpenError> {
    let path = path.as_ref();
    let Some(key) = source_key(path) else {
        return open_zone(path).map(Arc::new);
    };
    if let Some(image) = shared_live(&key) {
        SHARED_HIT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        return Ok(image);
    }
    let flight = flight_for(&key);
    let _held = flight
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    if let Some(image) = shared_live(&key) {
        SHARED_HIT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        return Ok(image);
    }
    let image = Arc::new(open_zone(path)?);
    shared_map()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(key, Arc::downgrade(&image));
    SHARED_MISS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Ok(image)
}

pub fn zone_share_counts() -> (u64, u64) {
    (
        SHARED_HIT.load(std::sync::atomic::Ordering::Relaxed),
        SHARED_MISS.load(std::sync::atomic::Ordering::Relaxed),
    )
}

static SHARED_HIT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static SHARED_MISS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

type ZoneSourceKey = (std::path::PathBuf, u64, Option<std::time::SystemTime>);

fn source_key(path: &Path) -> Option<ZoneSourceKey> {
    let meta = std::fs::metadata(path).ok()?;
    Some((path.to_owned(), meta.len(), meta.modified().ok()))
}

fn shared_map() -> &'static Mutex<HashMap<ZoneSourceKey, Weak<ZoneImage>>> {
    static SHARED: OnceLock<Mutex<HashMap<ZoneSourceKey, Weak<ZoneImage>>>> = OnceLock::new();
    SHARED.get_or_init(|| Mutex::new(HashMap::new()))
}

fn shared_live(key: &ZoneSourceKey) -> Option<Arc<ZoneImage>> {
    let mut map = shared_map()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match map.get(key).and_then(Weak::upgrade) {
        Some(image) => Some(image),
        None => {
            map.remove(key);
            None
        }
    }
}

fn flight_for(key: &ZoneSourceKey) -> Arc<Mutex<()>> {
    static FLIGHTS: OnceLock<Mutex<HashMap<ZoneSourceKey, Arc<Mutex<()>>>>> = OnceLock::new();
    let flights = FLIGHTS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut flights = flights
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Arc::clone(flights.entry(key.clone()).or_default())
}

pub fn open_zone(path: impl AsRef<Path>) -> Result<ZoneImage, ZoneOpenError> {
    let path = path.as_ref();
    // bo2zm: unsigned T6 zones key their cipher by the file stem.
    let stem = path.file_stem().and_then(|stem| stem.to_str());
    let result = std::fs::read(path)
        .map_err(ZoneOpenError::Io)
        .and_then(|bytes| parse_zone_image_with_stem(&bytes, stem));
    let image = result.map_err(|source| ZoneOpenError::AtPath {
        path: path.to_owned(),
        source: Box::new(source),
    })?;
    Ok(image)
}

fn select_iw4_table(image: &[u8]) -> Result<WireAssetTable<'_>, ZoneOpenError> {
    match (
        read_wire_asset_table(image, Iw4WireFormat::X86),
        read_wire_asset_table(image, Iw4WireFormat::X64),
    ) {
        (Ok(table), Err(_)) | (Err(_), Ok(table)) => Ok(table),
        (Ok(_), Ok(_)) => Err(ZoneOpenError::AmbiguousIw4WireLayout),
        (Err(x86), Err(x64)) => Err(ZoneOpenError::Iw4WireLayout { x86, x64 }),
    }
}

pub fn parse_zone_image(bytes: &[u8]) -> Result<ZoneImage, ZoneOpenError> {
    parse_zone_image_with_stem(bytes, None)
}

fn parse_zone_image_with_stem(
    bytes: &[u8],
    stem: Option<&str>,
) -> Result<ZoneImage, ZoneOpenError> {
    if bytes.len() < 12 {
        return Err(ZoneOpenError::Truncated);
    }
    let version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    match version {
        fastfile_iw4::ZONE_VERSION_PC => parse_iw4_zone_image(bytes),
        fastfile_t5::ZONE_VERSION_PC => parse_t5_zone_image(bytes),
        fastfile_iw5::ZONE_VERSION_PC => parse_iw5_zone_image(bytes),
        fastfile_t6::ZONE_VERSION_PC if fastfile_t6::is_t6_magic(bytes) => {
            parse_t6_zone_image(bytes, stem)
        }
        other => Err(ZoneOpenError::UnsupportedVersion { got: other }),
    }
}

fn inflate_zone(bytes: &[u8]) -> Result<Vec<u8>, ZoneOpenError> {
    let mut decoder = flate2::Decompress::new(true);
    let mut out = Vec::with_capacity(bytes.len());
    loop {
        out.reserve((out.len() / 2).max(1024 * 1024));
        let before = (decoder.total_in(), decoder.total_out());
        let status = decoder
            .decompress_vec(
                &bytes[decoder.total_in() as usize..],
                &mut out,
                flate2::FlushDecompress::None,
            )
            .map_err(|e| ZoneOpenError::Inflate(e.to_string()))?;
        if status == flate2::Status::StreamEnd {
            return Ok(out);
        }
        if before == (decoder.total_in(), decoder.total_out()) {
            return Err(ZoneOpenError::Inflate("truncated zlib stream".into()));
        }
    }
}

fn parse_iw4_zone_image(bytes: &[u8]) -> Result<ZoneImage, ZoneOpenError> {
    let header = parse_iw4_header(bytes).map_err(ZoneOpenError::Iw4Header)?;
    let body = &bytes[fastfile_iw4::FILE_PREAMBLE_LEN..];

    let compressed: Cow<'_, [u8]> = match header.signing {
        Iw4Signing::Unsigned => Cow::Borrowed(body),
        Iw4Signing::Signed => Cow::Owned(dechunk_authed(body)?),
    };

    let image = inflate_zone(&compressed)?;

    if image.len() < fastfile_iw4::XFILE_HEADER_LEN {
        return Err(ZoneOpenError::Truncated);
    }

    let iw4_wire_format = Some(select_iw4_table(&image)?.format());
    Ok(ZoneImage {
        game: ZoneGame::Iw4,
        version: header.version,
        bytes: image,
        iw4_wire_format,
    })
}

fn parse_t5_zone_image(bytes: &[u8]) -> Result<ZoneImage, ZoneOpenError> {
    let header = fastfile_t5::parse_file_header(bytes).map_err(ZoneOpenError::T5Header)?;
    let body = &bytes[fastfile_t5::FILE_PREAMBLE_LEN..];

    let _ = header.signing;
    let image = inflate_zone(body)?;

    if image.len() < fastfile_t5::XFILE_HEADER_LEN {
        return Err(ZoneOpenError::Truncated);
    }

    Ok(ZoneImage {
        game: ZoneGame::T5,
        version: header.version,
        bytes: image,
        iw4_wire_format: None,
    })
}

/// bo2zm: decrypt and inflate every XChunk record, in file order, into one
/// image. Each record is its own raw-deflate stream of at most one chunk.
fn parse_t6_zone_image(bytes: &[u8], stem: Option<&str>) -> Result<ZoneImage, ZoneOpenError> {
    let header = fastfile_t6::parse_file_header(bytes).map_err(ZoneOpenError::T6Header)?;
    let mut cipher = if header.signing.is_encrypted() {
        let name = header
            .zone_name()
            .or(stem)
            .ok_or(ZoneOpenError::T6NoZoneName)?;
        let cipher = fastfile_t6::ChunkCipher::new(name.as_bytes(), &fastfile_t6::SALSA20_KEY_PC)
            .ok_or(ZoneOpenError::T6NoZoneName)?;
        Some(Box::new(cipher))
    } else {
        None
    };

    let mut image: Vec<u8> = Vec::new();
    let mut record = vec![0u8; fastfile_t6::XCHUNK_SIZE];
    let mut inflated = vec![0u8; fastfile_t6::XCHUNK_SIZE];
    let mut inflater = flate2::Decompress::new(false);
    for chunk in fastfile_t6::XChunks::new(bytes, header.body_offset) {
        let chunk = chunk.map_err(ZoneOpenError::T6Chunk)?;
        let data = &mut record[..chunk.data.len()];
        data.copy_from_slice(chunk.data);
        if let Some(cipher) = cipher.as_mut() {
            cipher.decrypt(chunk.stream, data);
        }
        let produced = inflate_t6_chunk(&mut inflater, data, &mut inflated, chunk.index)?;
        image.extend_from_slice(&inflated[..produced]);
        if chunk.index == 0
            && let Ok(xfile) = fastfile_t6::parse_zone_header(&image)
        {
            image.reserve(xfile.size as usize + fastfile_t6::XFILE_HEADER_LEN);
        }
    }

    if image.len() < fastfile_t6::XFILE_HEADER_LEN {
        return Err(ZoneOpenError::Truncated);
    }

    Ok(ZoneImage {
        game: ZoneGame::T6,
        version: header.version,
        bytes: image,
        iw4_wire_format: None,
    })
}

/// Inflate one record into `out` (one chunk long) and return the byte count.
/// The record must end its deflate stream within that one chunk, as the
/// game's own reader requires. A fixed scratch buffer, not `decompress_vec`:
/// that zero-fills a `Vec`'s whole spare capacity on every call, which on a
/// 370 MB map image reserved up front cost minutes.
fn inflate_t6_chunk(
    inflater: &mut flate2::Decompress,
    data: &[u8],
    out: &mut [u8],
    chunk: usize,
) -> Result<usize, ZoneOpenError> {
    inflater.reset(false);
    let status = inflater
        .decompress(data, out, flate2::FlushDecompress::Finish)
        .map_err(|e| ZoneOpenError::T6Inflate {
            chunk,
            message: e.to_string(),
        })?;
    let produced = inflater.total_out() as usize;
    if status != flate2::Status::StreamEnd {
        return Err(ZoneOpenError::T6Inflate {
            chunk,
            message: format!(
                "stream did not end within the {:#x}-byte chunk ({produced} bytes out, {status:?})",
                fastfile_t6::XCHUNK_SIZE
            ),
        });
    }
    Ok(produced)
}

fn parse_iw5_zone_image(bytes: &[u8]) -> Result<ZoneImage, ZoneOpenError> {
    let header = fastfile_iw5::parse_file_header(bytes).map_err(ZoneOpenError::Iw5Header)?;
    let body = &bytes[fastfile_iw5::FILE_PREAMBLE_LEN..];
    let compressed: Cow<'_, [u8]> = match header.signing {
        Iw5Signing::Unsigned => Cow::Borrowed(body),
        Iw5Signing::Signed => Cow::Owned(dechunk_authed(body)?),
    };

    let image = inflate_zone(&compressed)?;

    if image.len() < fastfile_iw5::XFILE_HEADER_LEN {
        return Err(ZoneOpenError::Truncated);
    }

    Ok(ZoneImage {
        game: ZoneGame::Iw5,
        version: header.version,
        bytes: image,
        iw4_wire_format: None,
    })
}

fn dechunk_authed(body: &[u8]) -> Result<Vec<u8>, ZoneOpenError> {
    if body.len() < 8 || &body[0..8] != MAGIC_AUTH_HEADER {
        return Err(ZoneOpenError::BadAuthHeader);
    }

    let mut pos = AUTHED_CHUNK_SIZE;
    let mut out = Vec::with_capacity(body.len());
    let mut chunk_in_group = 0usize;

    while pos < body.len() {
        let end = (pos + AUTHED_CHUNK_SIZE).min(body.len());

        if chunk_in_group != 0 {
            out.extend_from_slice(&body[pos..end]);
        }
        pos = end;
        chunk_in_group = (chunk_in_group + 1) % (AUTHED_CHUNKS_PER_GROUP + 1);
    }

    Ok(out)
}

pub struct ZoneMemory {
    blocks: [Vec<u8>; MAX_XFILE_COUNT],
    insert_map: Vec<u8>,
}

pub fn xfile_arena_row(
    label: &str,
    block_size: &[u32],
    temp: usize,
    virtual_block: usize,
) -> String {
    let bytes: u64 = block_size.iter().copied().map(u64::from).sum();
    let temp_b = block_size.get(temp).copied().unwrap_or(0);
    let virt_b = block_size.get(virtual_block).copied().unwrap_or(0);
    format!(
        "{label}: bytes={bytes} ({:.1}MiB) temp={temp_b} virtual={virt_b}",
        bytes as f64 / (1024.0 * 1024.0),
    )
}

impl ZoneMemory {
    pub fn for_header(header: &ZoneHeader) -> ZoneMemory {
        let blocks = std::array::from_fn(|i| vec![0u8; header.block_size[i] as usize]);
        let insert_map = vec![0u8; ZoneStream::insert_map_len(header)];
        ZoneMemory { blocks, insert_map }
    }

    pub fn total_bytes(&self) -> usize {
        self.blocks.iter().map(|b| b.len()).sum()
    }

    pub fn stream<'a>(
        &'a mut self,
        image: &'a [u8],
    ) -> Result<ZoneStream<'a>, fastfile_iw4::ZoneError> {
        let [b0, b1, b2, b3, b4, b5, b6, b7] = &mut self.blocks;
        let blocks: [&mut [u8]; MAX_XFILE_COUNT] = [
            b0.as_mut_slice(),
            b1.as_mut_slice(),
            b2.as_mut_slice(),
            b3.as_mut_slice(),
            b4.as_mut_slice(),
            b5.as_mut_slice(),
            b6.as_mut_slice(),
            b7.as_mut_slice(),
        ];
        ZoneStream::new(image, blocks, &mut self.insert_map)
    }
}

pub struct T5ZoneMemory {
    blocks: [Vec<u8>; fastfile_t5::MAX_XFILE_COUNT],
    insert_map: Vec<u8>,
}

impl T5ZoneMemory {
    pub fn for_header(header: &fastfile_t5::ZoneHeader) -> T5ZoneMemory {
        let blocks = std::array::from_fn(|i| vec![0u8; t5_block_capacity(header, i)]);
        let insert_map = vec![0u8; fastfile_t5::ZoneStream::insert_map_len(header)];
        T5ZoneMemory { blocks, insert_map }
    }

    pub fn total_bytes(&self) -> usize {
        self.blocks.iter().map(|b| b.len()).sum()
    }

    pub fn stream<'a>(
        &'a mut self,
        image: &'a [u8],
    ) -> Result<fastfile_t5::ZoneStream<'a>, fastfile_t5::ZoneError> {
        let [b0, b1, b2, b3, b4, b5, b6] = &mut self.blocks;
        let blocks: [&mut [u8]; fastfile_t5::MAX_XFILE_COUNT] = [
            b0.as_mut_slice(),
            b1.as_mut_slice(),
            b2.as_mut_slice(),
            b3.as_mut_slice(),
            b4.as_mut_slice(),
            b5.as_mut_slice(),
            b6.as_mut_slice(),
        ];
        fastfile_t5::ZoneStream::new(image, blocks, &mut self.insert_map)
    }
}

fn t5_block_capacity(header: &fastfile_t5::ZoneHeader, block: usize) -> usize {
    let logical = header.block_size[block] as usize;
    if logical == 0 || block != fastfile_t5::XFILE_BLOCK_RUNTIME {
        return logical;
    }

    logical.saturating_add(0x0f).next_multiple_of(0x1000)
}

/// bo2zm: block storage for one T6 zone walk.
pub struct T6ZoneMemory {
    blocks: [Vec<u8>; fastfile_t6::MAX_XFILE_COUNT],
    insert_map: Vec<u8>,
}

impl T6ZoneMemory {
    pub fn for_header(header: &fastfile_t6::ZoneHeader) -> T6ZoneMemory {
        let blocks = std::array::from_fn(|i| vec![0u8; header.block_size[i] as usize]);
        let insert_map = vec![0u8; fastfile_t6::ZoneStream::insert_map_len(header)];
        T6ZoneMemory { blocks, insert_map }
    }

    pub fn total_bytes(&self) -> usize {
        self.blocks.iter().map(|b| b.len()).sum()
    }

    pub fn stream<'a>(
        &'a mut self,
        image: &'a [u8],
    ) -> Result<fastfile_t6::ZoneStream<'a>, fastfile_t6::ZoneError> {
        let [b0, b1, b2, b3, b4, b5, b6, b7] = &mut self.blocks;
        let blocks: [&mut [u8]; fastfile_t6::MAX_XFILE_COUNT] = [
            b0.as_mut_slice(),
            b1.as_mut_slice(),
            b2.as_mut_slice(),
            b3.as_mut_slice(),
            b4.as_mut_slice(),
            b5.as_mut_slice(),
            b6.as_mut_slice(),
            b7.as_mut_slice(),
        ];
        fastfile_t6::ZoneStream::new(image, blocks, &mut self.insert_map)
    }
}

pub struct Iw5ZoneMemory {
    blocks: [Vec<u8>; fastfile_iw5::MAX_XFILE_COUNT],
    insert_map: Vec<u8>,
}

impl Iw5ZoneMemory {
    pub fn for_header(header: &fastfile_iw5::ZoneHeader) -> Iw5ZoneMemory {
        let blocks = std::array::from_fn(|i| vec![0u8; header.block_size[i] as usize]);
        let insert_map = vec![0u8; fastfile_iw5::ZoneStream::insert_map_len(header)];
        Iw5ZoneMemory { blocks, insert_map }
    }

    pub fn total_bytes(&self) -> usize {
        self.blocks.iter().map(|b| b.len()).sum()
    }

    pub fn stream<'a>(
        &'a mut self,
        image: &'a [u8],
    ) -> Result<fastfile_iw5::ZoneStream<'a>, fastfile_iw5::ZoneError> {
        let [b0, b1, b2, b3, b4, b5, b6, b7, b8] = &mut self.blocks;
        let blocks: [&mut [u8]; fastfile_iw5::MAX_XFILE_COUNT] = [
            b0.as_mut_slice(),
            b1.as_mut_slice(),
            b2.as_mut_slice(),
            b3.as_mut_slice(),
            b4.as_mut_slice(),
            b5.as_mut_slice(),
            b6.as_mut_slice(),
            b7.as_mut_slice(),
            b8.as_mut_slice(),
        ];
        fastfile_iw5::ZoneStream::new(image, blocks, &mut self.insert_map)
    }
}
