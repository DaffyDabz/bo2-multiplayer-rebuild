//! XChunk records: the body of a T6 zone after its header.
//!
//! Each record is a u32 LE length and that many bytes, at most `XCHUNK_SIZE`.
//! Records go round-robin to `STREAM_COUNT` streams (record k feeds stream
//! k % 4); a record decrypts (Salsa20, per stream) and then inflates (raw
//! deflate, independently) to at most `XCHUNK_SIZE` bytes. The zone image is
//! the inflated records concatenated in file order, up to the first record
//! whose length is zero (or the end of the file).
//!
//! The game reads the file through a `VANILLA_BUFFER_SIZE` window that starts
//! at file offset 0. A length field never straddles that window: when fewer
//! than 4 bytes are left in it, the writer padded to the next window and the
//! length sits there. Payloads do straddle it.
//!
//! The cipher keeps, per stream, an index into a table of 200 SHA-1-sized
//! hash blocks. A record's IV is the first 8 bytes of its stream's current
//! block; after decrypting, the SHA-1 of the plaintext is XORed into the
//! stream's next block, which becomes current. The table starts filled with
//! the zone name: every name byte repeated four times, the name cycled.

use crate::salsa20;
use crate::sha1;

pub const STREAM_COUNT: usize = 4;

pub const XCHUNK_SIZE: usize = 0x8000;

pub const VANILLA_BUFFER_SIZE: usize = 0x80000;

const LEN_FIELD: usize = 4;

const BLOCK_HASHES_COUNT: usize = 200;

const HASH_BLOCK_LEN: usize = sha1::DIGEST_LEN;

const IV_LEN: usize = 8;

pub const HASH_TABLE_LEN: usize = BLOCK_HASHES_COUNT * STREAM_COUNT * HASH_BLOCK_LEN;

/// The public Salsa20 key of retail T6 PC zones.
pub const SALSA20_KEY_PC: [u8; 32] = [
    0x64, 0x1D, 0x8A, 0x2F, 0xE3, 0x1D, 0x3A, 0xA6, 0x36, 0x22, 0xBB, 0xC9, 0xCE, 0x85, 0x87, 0x22,
    0x9D, 0x42, 0xB0, 0xF8, 0xED, 0x9B, 0x92, 0x41, 0x30, 0xBF, 0x88, 0xB6, 0x5E, 0xDC, 0x50, 0xBE,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XChunkError {
    /// A length field promised more than one chunk.
    TooLarge { at: usize, len: usize },
    /// The file ended inside a payload.
    Truncated { at: usize, len: usize, have: usize },
}

impl core::fmt::Display for XChunkError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            XChunkError::TooLarge { at, len } => write!(
                f,
                "xchunk at file offset {at:#x} claims {len:#x} bytes (max {XCHUNK_SIZE:#x})"
            ),
            XChunkError::Truncated { at, len, have } => write!(
                f,
                "xchunk at file offset {at:#x} claims {len:#x} bytes, file has {have:#x} left"
            ),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct XChunk<'a> {
    /// Record number in file order.
    pub index: usize,
    pub stream: usize,
    /// File offset of the payload (after its length field).
    pub offset: usize,
    pub data: &'a [u8],
}

/// Walks the XChunk records of a whole zone file from `body_offset`.
#[derive(Clone, Debug)]
pub struct XChunks<'a> {
    file: &'a [u8],
    pos: usize,
    index: usize,
    done: bool,
}

impl<'a> XChunks<'a> {
    /// `file` is the whole zone file; `body_offset` comes from the header.
    pub fn new(file: &'a [u8], body_offset: usize) -> XChunks<'a> {
        XChunks {
            file,
            pos: body_offset,
            index: 0,
            done: false,
        }
    }

    /// File offset just past the last record read; after the walk ends this
    /// is where the terminating zero length (or the end of file) sat.
    pub fn position(&self) -> usize {
        self.pos
    }
}

impl<'a> Iterator for XChunks<'a> {
    type Item = Result<XChunk<'a>, XChunkError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let left_in_window = VANILLA_BUFFER_SIZE - self.pos % VANILLA_BUFFER_SIZE;
        if left_in_window < LEN_FIELD {
            self.pos += left_in_window;
        }
        let Some(len_bytes) = self.file.get(self.pos..self.pos + LEN_FIELD) else {
            self.done = true;
            return None;
        };
        let len = u32::from_le_bytes(len_bytes.try_into().unwrap()) as usize;
        if len == 0 {
            self.done = true;
            return None;
        }
        let at = self.pos;
        if len > XCHUNK_SIZE {
            self.done = true;
            return Some(Err(XChunkError::TooLarge { at, len }));
        }
        let start = at + LEN_FIELD;
        let Some(data) = self.file.get(start..start + len) else {
            self.done = true;
            return Some(Err(XChunkError::Truncated {
                at,
                len,
                have: self.file.len().saturating_sub(start),
            }));
        };
        let chunk = XChunk {
            index: self.index,
            stream: self.index % STREAM_COUNT,
            offset: start,
            data,
        };
        self.pos = start + len;
        self.index += 1;
        Some(Ok(chunk))
    }
}

/// The per-stream Salsa20 state of one zone.
#[derive(Clone)]
pub struct ChunkCipher {
    key: [u8; 32],
    table: [u8; HASH_TABLE_LEN],
    current: [usize; STREAM_COUNT],
}

impl ChunkCipher {
    /// `zone_name` is the auth header's name (or, for unsigned zones, the
    /// file stem). Returns `None` for an empty name: the table cannot be
    /// seeded from nothing.
    pub fn new(zone_name: &[u8], key: &[u8; 32]) -> Option<ChunkCipher> {
        let name = &zone_name[..zone_name.len().min(crate::ZONE_NAME_KEY_MAX)];
        if name.is_empty() {
            return None;
        }
        let mut table = [0u8; HASH_TABLE_LEN];
        for (i, word) in table.chunks_exact_mut(4).enumerate() {
            word.fill(name[i % name.len()]);
        }
        Some(ChunkCipher {
            key: *key,
            table,
            current: [0; STREAM_COUNT],
        })
    }

    fn block_at(stream: usize, index: usize) -> usize {
        (index * STREAM_COUNT + stream) * HASH_BLOCK_LEN
    }

    /// Decrypt one record of `stream` in place. Records of a stream must be
    /// fed in file order: each one's IV depends on the one before.
    pub fn decrypt(&mut self, stream: usize, data: &mut [u8]) {
        let at = Self::block_at(stream, self.current[stream]);
        let mut iv = [0u8; IV_LEN];
        iv.copy_from_slice(&self.table[at..at + IV_LEN]);
        salsa20::xor_keystream(&self.key, &iv, data);

        let hash = sha1::digest(data);
        let next = (self.current[stream] + 1) % BLOCK_HASHES_COUNT;
        self.current[stream] = next;
        let at = Self::block_at(stream, next);
        for (cell, h) in self.table[at..at + HASH_BLOCK_LEN].iter_mut().zip(hash) {
            *cell ^= h;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(out: &mut [u8], at: usize, payload: &[u8]) -> usize {
        out[at..at + 4].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        out[at + 4..at + 4 + payload.len()].copy_from_slice(payload);
        at + 4 + payload.len()
    }

    #[test]
    fn records_go_round_robin_and_stop_at_zero_length() {
        let mut file = [0u8; 64];
        let mut at = 8;
        for i in 0..5u8 {
            at = record(&mut file, at, &[i; 3]);
        }
        let chunks: [_; 5] = core::array::from_fn(|_| None::<(usize, usize, u8)>);
        let mut got = chunks;
        let mut walk = XChunks::new(&file, 8);
        for slot in got.iter_mut() {
            let c = walk.next().unwrap().unwrap();
            *slot = Some((c.index, c.stream, c.data[0]));
        }
        assert!(walk.next().is_none());
        assert_eq!(got[4], Some((4, 0, 4)));
        assert_eq!(got[3], Some((3, 3, 3)));
        assert_eq!(walk.position(), at);
    }

    #[test]
    fn a_length_field_never_straddles_the_read_window() {
        extern crate alloc;
        let mut file = alloc::vec![0u8; VANILLA_BUFFER_SIZE + 16];
        // A 10-byte record that ends 3 bytes short of the window edge: the
        // next length field sits at the start of the next window.
        let start = VANILLA_BUFFER_SIZE - 3 - 10 - 4;
        record(&mut file, start, &[1; 10]);
        record(&mut file, VANILLA_BUFFER_SIZE, &[9; 5]);
        let mut walk = XChunks::new(&file, start);
        assert_eq!(walk.next().unwrap().unwrap().data, &[1; 10]);
        let second = walk.next().unwrap().unwrap();
        assert_eq!(second.offset, VANILLA_BUFFER_SIZE + 4);
        assert_eq!(second.data, &[9; 5]);
        assert!(walk.next().is_none());
    }

    #[test]
    fn oversized_records_are_refused() {
        let mut file = [0u8; 16];
        file[0..4].copy_from_slice(&((XCHUNK_SIZE + 1) as u32).to_le_bytes());
        assert!(matches!(
            XChunks::new(&file, 0).next(),
            Some(Err(XChunkError::TooLarge { .. }))
        ));
    }

    #[test]
    fn cipher_table_is_seeded_with_the_name_and_iv_chains_per_stream() {
        let fresh = ChunkCipher::new(b"ab", &SALSA20_KEY_PC).unwrap();
        assert_eq!(&fresh.table[..12], b"aaaabbbbaaaa");
        assert!(ChunkCipher::new(b"", &SALSA20_KEY_PC).is_none());

        let mut a = fresh.clone();
        let mut first = [0u8; 40];
        a.decrypt(1, &mut first);
        // Only stream 1 moved on.
        assert_eq!(a.current, [0, 1, 0, 0]);

        // Deterministic: a fresh cipher gives the same first record.
        let mut b = fresh.clone();
        let mut again = [0u8; 40];
        b.decrypt(1, &mut again);
        assert_eq!(again, first);

        // Streams start from different table rows, so different IVs.
        let mut c = fresh.clone();
        let mut other = [0u8; 40];
        c.decrypt(0, &mut other);
        assert_ne!(other, first);

        // A stream's second record uses the chained IV, not the first one.
        let mut second = [0u8; 40];
        a.decrypt(1, &mut second);
        assert_ne!(second, first);
    }
}
