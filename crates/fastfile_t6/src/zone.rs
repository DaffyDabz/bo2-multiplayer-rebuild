//! The inflated T6 zone image: XFile header, eight blocks, pointer fixups.
//!
//! bo2zm: the stream is fastfile_t5's `ZoneStream` cut down to the parts the
//! T6 walk uses so far, with T6's block table. Per-asset geometry captures get
//! added beside the readers that fill them.
//!
//! Pointers keep T5's encoding: 3 block bits over a 29-bit offset, biased by
//! one, with -1 = data follows in the stream and -2 = data follows and the
//! pointer is aliased through a slot in the insert block (VIRTUAL).

pub const MAX_XFILE_COUNT: usize = 8;

pub const XFILE_BLOCK_TEMP: usize = 0;
pub const XFILE_BLOCK_RUNTIME_VIRTUAL: usize = 1;
pub const XFILE_BLOCK_RUNTIME_PHYSICAL: usize = 2;
pub const XFILE_BLOCK_DELAY_VIRTUAL: usize = 3;
pub const XFILE_BLOCK_DELAY_PHYSICAL: usize = 4;
pub const XFILE_BLOCK_VIRTUAL: usize = 5;
pub const XFILE_BLOCK_PHYSICAL: usize = 6;
pub const XFILE_BLOCK_STREAMER_RESERVE: usize = 7;

pub const XFILE_BLOCK_NAMES: [&str; MAX_XFILE_COUNT] = [
    "temp",
    "runtime_virtual",
    "runtime_physical",
    "delay_virtual",
    "delay_physical",
    "virtual",
    "physical",
    "streamer_reserve",
];

pub const PTR_SIZE: usize = 4;

const BLOCK_SHIFT: u32 = 29;
const OFFSET_MASK: u32 = 0x1FFF_FFFF;

const ZONE_PTR_FOLLOWING: u32 = 0xFFFF_FFFF;
const ZONE_PTR_INSERT: u32 = 0xFFFF_FFFE;

pub const XFILE_HEADER_LEN: usize = 8 + 4 * MAX_XFILE_COUNT;

pub const BLOCK_STACK_CAP: usize = 32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BlockType {
    /// Read from the stream; the offset rewinds when the block is popped.
    Temp,
    /// Read from the stream and kept.
    Normal,
    /// Reserved and zeroed; nothing is read from the stream.
    Runtime,
    /// Declared by the game but never filled during a load.
    Delay,
}

pub const BLOCK_TYPES: [BlockType; MAX_XFILE_COUNT] = [
    BlockType::Temp,
    BlockType::Runtime,
    BlockType::Runtime,
    BlockType::Delay,
    BlockType::Delay,
    BlockType::Normal,
    BlockType::Normal,
    BlockType::Normal,
];

pub fn block_is_aliasable(block: u8) -> bool {
    matches!(BLOCK_TYPES.get(block as usize), Some(BlockType::Normal))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ZoneError {
    Truncated {
        at: usize,
        needed: usize,
        len: usize,
    },
    BlockOverflow {
        block: usize,
        end: usize,
        size: usize,
        request: usize,
    },
    BadBlock(usize),
    BadOffset {
        block: usize,
        offset: usize,
        size: usize,
    },
    DelayBlockLoad(usize),
    UnknownAssetType(u32),
    NoAssetLoader(crate::asset_type::AssetType),
    UnterminatedString {
        block: usize,
    },
    NotUtf8,
    StackOverflow,
    StackUnderflow,
    NoBlockPushed,
    InsertMapTooSmall {
        needed: usize,
        got: usize,
    },
    /// A list pointer that the format says must be "follows" was something else.
    NotFollowing {
        what: &'static str,
        raw: u32,
    },
    /// A load did not start where the block's write position is: the walk
    /// and the zone disagree about a size somewhere before this point.
    NonContiguous {
        block: usize,
        at: usize,
        expected: usize,
    },
    /// A pointer value the format does not allow at this place.
    BadPointer {
        what: &'static str,
        raw: u32,
    },
    /// A count expression came out negative or absurd.
    BadCount(i64),
    DivideByZero,
    WalkTooDeep,
}

impl core::fmt::Display for ZoneError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ZoneError::Truncated { at, needed, len } => {
                write!(
                    f,
                    "zone truncated: needed {needed} bytes at {at}, have {len}"
                )
            }
            ZoneError::BlockOverflow {
                block,
                end,
                size,
                request,
            } => {
                write!(
                    f,
                    "block {block} overflow: end {end} exceeds size {size} (request {request})"
                )
            }
            ZoneError::BadBlock(b) => write!(f, "invalid block index {b} in zone pointer"),
            ZoneError::BadOffset {
                block,
                offset,
                size,
            } => write!(
                f,
                "offset {offset} out of bounds in block {block} (size {size})"
            ),
            ZoneError::DelayBlockLoad(b) => {
                write!(
                    f,
                    "zone data aimed at delay block {b}, which a load never fills"
                )
            }
            ZoneError::UnknownAssetType(v) => write!(f, "unknown T6 asset pool id {v:#x}"),
            ZoneError::NoAssetLoader(ty) => {
                write!(
                    f,
                    "no T6 zone walk implemented for asset type `{}`",
                    ty.name()
                )
            }
            ZoneError::UnterminatedString { block } => {
                write!(f, "unterminated string in block {block}")
            }
            ZoneError::NotUtf8 => write!(f, "string is not valid UTF-8"),
            ZoneError::StackOverflow => write!(f, "block stack overflow"),
            ZoneError::StackUnderflow => write!(f, "block stack underflow"),
            ZoneError::NoBlockPushed => write!(f, "no block pushed"),
            ZoneError::InsertMapTooSmall { needed, got } => {
                write!(
                    f,
                    "insert-slot map too small: needed {needed} bytes, got {got}"
                )
            }
            ZoneError::NotFollowing { what, raw } => {
                write!(f, "{what} pointer is {raw:#x}, expected data to follow")
            }
            ZoneError::NonContiguous {
                block,
                at,
                expected,
            } => write!(
                f,
                "load at block {block} offset {at:#x}, but the block is at {expected:#x}"
            ),
            ZoneError::BadPointer { what, raw } => {
                write!(f, "pointer {raw:#x} not allowed for {what}")
            }
            ZoneError::BadCount(n) => write!(f, "count expression gave {n}"),
            ZoneError::DivideByZero => write!(f, "count expression divides by zero"),
            ZoneError::WalkTooDeep => write!(f, "zone walk nested too deep"),
        }
    }
}

pub type Result<T> = core::result::Result<T, ZoneError>;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Ptr {
    pub block: u8,
    pub offset: u32,
}

impl Ptr {
    pub fn at(self, delta: usize) -> Ptr {
        Ptr {
            block: self.block,
            offset: self.offset.wrapping_add(delta as u32),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ZonePtr {
    Null,
    Following,
    Insert,
    Offset(Ptr),
}

impl ZonePtr {
    pub fn decode(v: u32) -> ZonePtr {
        match v {
            0 => ZonePtr::Null,
            ZONE_PTR_FOLLOWING => ZonePtr::Following,
            ZONE_PTR_INSERT => ZonePtr::Insert,
            _ => {
                let e = v - 1;
                ZonePtr::Offset(Ptr {
                    block: (e >> BLOCK_SHIFT) as u8,
                    offset: e & OFFSET_MASK,
                })
            }
        }
    }

    pub fn encode_offset(p: Ptr) -> u32 {
        (((p.block as u32) << BLOCK_SHIFT) | (p.offset & OFFSET_MASK)).wrapping_add(1)
    }

    pub fn is_following(self) -> bool {
        matches!(self, ZonePtr::Following | ZonePtr::Insert)
    }

    pub fn is_null(self) -> bool {
        matches!(self, ZonePtr::Null)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ZoneHeader {
    pub size: u32,
    pub external_size: u32,
    pub block_size: [u32; MAX_XFILE_COUNT],
}

pub fn parse_zone_header(image: &[u8]) -> Result<ZoneHeader> {
    if image.len() < XFILE_HEADER_LEN {
        return Err(ZoneError::Truncated {
            at: 0,
            needed: XFILE_HEADER_LEN,
            len: image.len(),
        });
    }
    let rd = |o: usize| u32::from_le_bytes([image[o], image[o + 1], image[o + 2], image[o + 3]]);
    let mut block_size = [0u32; MAX_XFILE_COUNT];
    for (i, b) in block_size.iter_mut().enumerate() {
        *b = rd(8 + i * 4);
    }
    Ok(ZoneHeader {
        size: rd(0),
        external_size: rd(4),
        block_size,
    })
}

fn align_up(v: usize, align: usize) -> usize {
    debug_assert!(align.is_power_of_two());
    let mask = (align as u32).wrapping_sub(1);
    (v as u32).wrapping_add(mask) as usize & !(mask as usize)
}

pub struct ZoneStream<'a> {
    data: &'a [u8],
    cursor: usize,
    blocks: [&'a mut [u8]; MAX_XFILE_COUNT],
    offsets: [usize; MAX_XFILE_COUNT],
    stack: [u8; BLOCK_STACK_CAP],
    stack_depth: usize,
    temp_saved: [usize; BLOCK_STACK_CAP],
    temp_depth: usize,
    insert_map: &'a mut [u8],
    pending_insert: Option<Ptr>,
    unsettled_offsets: usize,
    first_unsettled: Option<(Ptr, usize, usize)>,
    header: ZoneHeader,
}

impl<'a> ZoneStream<'a> {
    pub fn insert_map_len(header: &ZoneHeader) -> usize {
        let slots = header.block_size[XFILE_BLOCK_VIRTUAL] as usize / PTR_SIZE;
        slots.div_ceil(8) + 1
    }

    /// `blocks[i]` must hold at least `header.block_size[i]` bytes. Delay
    /// blocks are never written and may be empty.
    pub fn new(
        image: &'a [u8],
        blocks: [&'a mut [u8]; MAX_XFILE_COUNT],
        insert_map: &'a mut [u8],
    ) -> Result<ZoneStream<'a>> {
        let header = parse_zone_header(image)?;
        for (i, b) in blocks.iter().enumerate() {
            if BLOCK_TYPES[i] == BlockType::Delay {
                continue;
            }
            let want = header.block_size[i] as usize;
            if b.len() < want {
                return Err(ZoneError::BlockOverflow {
                    block: i,
                    end: want,
                    size: b.len(),
                    request: want,
                });
            }
        }
        let needed = Self::insert_map_len(&header);
        if insert_map.len() < needed {
            return Err(ZoneError::InsertMapTooSmall {
                needed,
                got: insert_map.len(),
            });
        }
        insert_map.fill(0);
        Ok(ZoneStream {
            data: image,
            cursor: XFILE_HEADER_LEN,
            blocks,
            offsets: [0; MAX_XFILE_COUNT],
            stack: [0; BLOCK_STACK_CAP],
            stack_depth: 0,
            temp_saved: [0; BLOCK_STACK_CAP],
            temp_depth: 0,
            insert_map,
            pending_insert: None,
            unsettled_offsets: 0,
            first_unsettled: None,
            header,
        })
    }

    pub fn header(&self) -> &ZoneHeader {
        &self.header
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.cursor)
    }

    pub fn peek(&self, n: usize) -> &[u8] {
        let end = (self.cursor + n).min(self.data.len());
        &self.data[self.cursor..end]
    }

    pub fn unsettled_offsets(&self) -> usize {
        self.unsettled_offsets
    }

    pub fn finish(&self) -> Result<()> {
        if self.remaining() != 0 {
            return Err(ZoneError::Truncated {
                at: self.cursor,
                needed: self.data.len(),
                len: self.data.len(),
            });
        }
        Ok(())
    }

    pub fn push(&mut self, block: usize) -> Result<()> {
        if block >= MAX_XFILE_COUNT {
            return Err(ZoneError::BadBlock(block));
        }
        if self.stack_depth >= BLOCK_STACK_CAP {
            return Err(ZoneError::StackOverflow);
        }
        if BLOCK_TYPES[block] == BlockType::Temp {
            if self.temp_depth >= BLOCK_STACK_CAP {
                return Err(ZoneError::StackOverflow);
            }
            self.temp_saved[self.temp_depth] = self.offsets[block];
            self.temp_depth += 1;
        }
        self.stack[self.stack_depth] = block as u8;
        self.stack_depth += 1;
        Ok(())
    }

    pub fn pop(&mut self) -> Result<()> {
        if self.stack_depth == 0 {
            return Err(ZoneError::StackUnderflow);
        }
        self.stack_depth -= 1;
        let b = self.stack[self.stack_depth] as usize;
        if BLOCK_TYPES[b] == BlockType::Temp {
            if self.temp_depth == 0 {
                return Err(ZoneError::StackUnderflow);
            }
            self.temp_depth -= 1;
            self.offsets[b] = self.temp_saved[self.temp_depth];
        }
        Ok(())
    }

    fn top(&self) -> Result<usize> {
        if self.stack_depth == 0 {
            return Err(ZoneError::NoBlockPushed);
        }
        Ok(self.stack[self.stack_depth - 1] as usize)
    }

    /// The block allocations currently go to.
    pub fn top_block(&self) -> Result<usize> {
        self.top()
    }

    /// Align the current block's write position and return it, reserving
    /// nothing: the bytes arrive through `load_bytes`, possibly in pieces.
    pub fn align_alloc(&mut self, align: usize) -> Result<Ptr> {
        self.align_pos(align)?;
        let b = self.top()?;
        Ok(Ptr {
            block: b as u8,
            offset: self.offsets[b] as u32,
        })
    }

    /// Fill `size` bytes at the current block's write position: from the
    /// stream for temp and normal blocks, zeros for runtime blocks.
    pub fn load_bytes(&mut self, size: usize) -> Result<Ptr> {
        let b = self.top()?;
        if BLOCK_TYPES[b] == BlockType::Delay {
            return Err(ZoneError::DelayBlockLoad(b));
        }
        let pos = self.offsets[b];
        let end = pos + size;
        if end > self.blocks[b].len() {
            return Err(ZoneError::BlockOverflow {
                block: b,
                end,
                size: self.blocks[b].len(),
                request: size,
            });
        }
        if BLOCK_TYPES[b] == BlockType::Runtime {
            self.blocks[b][pos..end].fill(0);
        } else {
            let s = self.cursor;
            if s + size > self.data.len() {
                return Err(ZoneError::Truncated {
                    at: s,
                    needed: size,
                    len: self.data.len(),
                });
            }
            self.blocks[b][pos..end].copy_from_slice(&self.data[s..s + size]);
            self.cursor += size;
        }
        self.offsets[b] = end;
        Ok(Ptr {
            block: b as u8,
            offset: pos as u32,
        })
    }

    /// Point a reserved insert slot at `body` and mark it as an alias.
    pub fn bind_insert(&mut self, slot: Ptr, body: Ptr) -> Result<()> {
        self.pending_insert = Some(slot);
        self.bind_pending_insert(body)
    }

    /// Read bytes straight off the stream, into no block (the XAssetList head).
    pub fn read_raw(&mut self, size: usize) -> Result<&[u8]> {
        let s = self.cursor;
        if s + size > self.data.len() {
            return Err(ZoneError::Truncated {
                at: s,
                needed: size,
                len: self.data.len(),
            });
        }
        self.cursor += size;
        Ok(&self.data[s..s + size])
    }

    pub fn align_pos(&mut self, align: usize) -> Result<()> {
        let b = self.top()?;
        let pos = align_up(self.offsets[b], align);
        if pos > self.blocks[b].len() {
            return Err(ZoneError::BlockOverflow {
                block: b,
                end: pos,
                size: self.blocks[b].len(),
                request: 0,
            });
        }
        self.offsets[b] = pos;
        Ok(())
    }

    pub fn alloc_load(&mut self, align: usize, size: usize) -> Result<Ptr> {
        let b = self.top()?;
        if BLOCK_TYPES[b] == BlockType::Delay {
            return Err(ZoneError::DelayBlockLoad(b));
        }
        let pos = align_up(self.offsets[b], align);
        let end = pos + size;
        if end > self.blocks[b].len() {
            return Err(ZoneError::BlockOverflow {
                block: b,
                end,
                size: self.blocks[b].len(),
                request: size,
            });
        }

        if BLOCK_TYPES[b] == BlockType::Runtime {
            self.blocks[b][pos..end].fill(0);
        } else {
            let s = self.cursor;
            if s + size > self.data.len() {
                return Err(ZoneError::Truncated {
                    at: s,
                    needed: size,
                    len: self.data.len(),
                });
            }
            self.blocks[b][pos..end].copy_from_slice(&self.data[s..s + size]);
            self.cursor += size;
        }

        self.offsets[b] = end;
        let body = Ptr {
            block: b as u8,
            offset: pos as u32,
        };
        self.bind_pending_insert(body)?;
        Ok(body)
    }

    pub fn load_string(&mut self) -> Result<Ptr> {
        let b = self.top()?;
        if BLOCK_TYPES[b] == BlockType::Delay {
            return Err(ZoneError::DelayBlockLoad(b));
        }
        let start = self.offsets[b];
        let mut off = start;
        loop {
            if off >= self.blocks[b].len() {
                return Err(ZoneError::BlockOverflow {
                    block: b,
                    end: off,
                    size: self.blocks[b].len(),
                    request: 0,
                });
            }
            if self.cursor >= self.data.len() {
                return Err(ZoneError::UnterminatedString { block: b });
            }
            let byte = self.data[self.cursor];
            self.cursor += 1;
            self.blocks[b][off] = byte;
            off += 1;
            if byte == 0 {
                break;
            }
        }
        self.offsets[b] = off;
        let body = Ptr {
            block: b as u8,
            offset: start as u32,
        };
        self.bind_pending_insert(body)?;
        Ok(body)
    }

    pub fn insert_pointer_slot(&mut self) -> Result<Ptr> {
        let b = XFILE_BLOCK_VIRTUAL;
        let pos = align_up(self.offsets[b], PTR_SIZE);
        let end = pos + PTR_SIZE;
        if end > self.blocks[b].len() {
            return Err(ZoneError::BlockOverflow {
                block: b,
                end,
                size: self.blocks[b].len(),
                request: PTR_SIZE,
            });
        }
        self.offsets[b] = end;
        Ok(Ptr {
            block: b as u8,
            offset: pos as u32,
        })
    }

    fn bind_pending_insert(&mut self, body: Ptr) -> Result<()> {
        let Some(slot) = self.pending_insert.take() else {
            return Ok(());
        };
        let b = slot.block as usize;
        let off = slot.offset as usize;
        if b >= MAX_XFILE_COUNT || off + PTR_SIZE > self.blocks[b].len() {
            return Err(ZoneError::BadOffset {
                block: b,
                offset: off,
                size: self.blocks.get(b).map_or(0, |x| x.len()),
            });
        }
        self.blocks[b][off..off + PTR_SIZE]
            .copy_from_slice(&ZonePtr::encode_offset(body).to_le_bytes());
        self.mark_insert_slot(slot);
        Ok(())
    }

    fn insert_bit(slot: Ptr) -> (usize, u8) {
        let idx = slot.offset as usize / PTR_SIZE;
        (idx / 8, 1u8 << (idx % 8))
    }

    fn mark_insert_slot(&mut self, slot: Ptr) {
        if slot.block as usize != XFILE_BLOCK_VIRTUAL {
            return;
        }
        let (byte, bit) = Self::insert_bit(slot);
        if let Some(cell) = self.insert_map.get_mut(byte) {
            *cell |= bit;
        }
    }

    fn is_insert_slot(&self, p: Ptr) -> bool {
        if p.block as usize != XFILE_BLOCK_VIRTUAL {
            return false;
        }
        let (byte, bit) = Self::insert_bit(p);
        self.insert_map.get(byte).is_some_and(|c| c & bit != 0)
    }

    pub fn resolve_alias(&self, p: Ptr) -> Ptr {
        if !self.is_insert_slot(p) {
            return p;
        }
        match self.u32_at(p, 0).map(ZonePtr::decode) {
            Ok(ZonePtr::Offset(body)) => body,
            _ => p,
        }
    }

    pub fn begin_body_with_insert(&mut self, slot: Ptr) -> Result<(bool, Option<Ptr>)> {
        match self.ptr_at(slot, 0)? {
            ZonePtr::Null => Ok((false, None)),
            ZonePtr::Offset(p) => {
                self.note_offset(p);
                Ok((false, None))
            }
            ZonePtr::Following => Ok((true, None)),
            ZonePtr::Insert => {
                let reserved = self.insert_pointer_slot()?;
                self.pending_insert = Some(reserved);
                Ok((true, Some(reserved)))
            }
        }
    }

    pub fn begin_body(&mut self, slot: Ptr) -> Result<bool> {
        Ok(self.begin_body_with_insert(slot)?.0)
    }

    fn bytes_at(&self, p: Ptr, off: usize, len: usize) -> Result<&[u8]> {
        let b = p.block as usize;
        if b >= MAX_XFILE_COUNT {
            return Err(ZoneError::BadBlock(b));
        }
        let s = p.offset as usize + off;
        let buf = &self.blocks[b];
        if s + len > buf.len() {
            return Err(ZoneError::BadOffset {
                block: b,
                offset: s,
                size: buf.len(),
            });
        }
        Ok(&buf[s..s + len])
    }

    pub fn u8_at(&self, p: Ptr, off: usize) -> Result<u8> {
        Ok(self.bytes_at(p, off, 1)?[0])
    }

    pub fn slice_at(&self, p: Ptr, off: usize, len: usize) -> Result<&[u8]> {
        self.bytes_at(p, off, len)
    }

    pub fn u16_at(&self, p: Ptr, off: usize) -> Result<u16> {
        let b = self.bytes_at(p, off, 2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn i16_at(&self, p: Ptr, off: usize) -> Result<i16> {
        Ok(self.u16_at(p, off)? as i16)
    }

    pub fn i32_at(&self, p: Ptr, off: usize) -> Result<i32> {
        Ok(self.u32_at(p, off)? as i32)
    }

    pub fn u32_at(&self, p: Ptr, off: usize) -> Result<u32> {
        let b = self.bytes_at(p, off, 4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn f32_at(&self, p: Ptr, off: usize) -> Result<f32> {
        Ok(f32::from_bits(self.u32_at(p, off)?))
    }

    pub fn ptr_at(&self, p: Ptr, off: usize) -> Result<ZonePtr> {
        Ok(ZonePtr::decode(self.u32_at(p, off)?))
    }

    pub fn first_unsettled(&self) -> Option<(Ptr, usize, usize)> {
        self.first_unsettled
    }

    pub fn fixup_slot(&mut self, slot: Ptr, body: Ptr) -> Result<()> {
        self.write_u32_at(slot, 0, ZonePtr::encode_offset(body))
    }

    pub fn follow_array(
        &mut self,
        parent: Ptr,
        field: usize,
        align: usize,
        elem_size: usize,
        count: usize,
    ) -> Result<Option<Ptr>> {
        let body = match self.ptr_at(parent, field)? {
            ZonePtr::Null => return Ok(None),
            ZonePtr::Offset(p) => {
                self.note_offset(p);
                self.resolve_alias(p)
            }
            _ => {
                self.begin_body(parent.at(field))?;
                self.alloc_load(align, elem_size * count)?
            }
        };
        self.fixup_slot(parent.at(field), body)?;
        Ok(Some(body))
    }

    pub fn follow_string(&mut self, parent: Ptr, field: usize) -> Result<Option<Ptr>> {
        let body = match self.ptr_at(parent, field)? {
            ZonePtr::Null => return Ok(None),
            ZonePtr::Offset(p) => {
                self.note_offset(p);
                self.resolve_alias(p)
            }
            _ => {
                self.begin_body(parent.at(field))?;
                self.load_string()?
            }
        };
        self.fixup_slot(parent.at(field), body)?;
        Ok(Some(body))
    }

    pub fn write_u32_at(&mut self, p: Ptr, off: usize, v: u32) -> Result<()> {
        let b = p.block as usize;
        if b >= MAX_XFILE_COUNT {
            return Err(ZoneError::BadBlock(b));
        }
        let s = p.offset as usize + off;
        let len = self.blocks[b].len();
        if s + 4 > len {
            return Err(ZoneError::BadOffset {
                block: b,
                offset: s,
                size: len,
            });
        }
        self.blocks[b][s..s + 4].copy_from_slice(&v.to_le_bytes());
        Ok(())
    }

    pub fn cstr_bytes(&self, p: Ptr) -> Result<&[u8]> {
        let b = p.block as usize;
        if b >= MAX_XFILE_COUNT {
            return Err(ZoneError::BadBlock(b));
        }
        let buf = &self.blocks[b];
        let s = p.offset as usize;
        if s >= buf.len() {
            return Err(ZoneError::BadOffset {
                block: b,
                offset: s,
                size: buf.len(),
            });
        }
        let end = buf[s..]
            .iter()
            .position(|&c| c == 0)
            .map_or(buf.len(), |i| s + i);
        Ok(&buf[s..end])
    }

    pub fn cstr(&self, p: Ptr) -> Result<&str> {
        core::str::from_utf8(self.cstr_bytes(p)?).map_err(|_| ZoneError::NotUtf8)
    }

    pub fn watermark(&self, block: u8) -> usize {
        self.offsets.get(block as usize).copied().unwrap_or(0)
    }

    pub fn block_overrun(&self, block: u8) -> usize {
        let index = block as usize;
        assert!(
            index < MAX_XFILE_COUNT,
            "T6 block index {block} exceeds {MAX_XFILE_COUNT} blocks"
        );
        self.offsets[index].saturating_sub(self.header.block_size[index] as usize)
    }

    pub fn offset_is_settled(&self, p: Ptr) -> bool {
        (p.offset as usize) < self.watermark(p.block)
    }

    pub fn note_offset(&mut self, p: Ptr) {
        if !self.offset_is_settled(p) {
            self.unsettled_offsets += 1;
            if self.first_unsettled.is_none() {
                self.first_unsettled = Some((p, self.watermark(p.block), self.cursor));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointers_use_three_block_bits() {
        let p = Ptr {
            block: XFILE_BLOCK_VIRTUAL as u8,
            offset: 0x1234,
        };
        let raw = ZonePtr::encode_offset(p);
        assert_eq!(raw, (5 << 29 | 0x1234) + 1);
        assert_eq!(ZonePtr::decode(raw), ZonePtr::Offset(p));
        assert_eq!(ZonePtr::decode(0), ZonePtr::Null);
        assert_eq!(ZonePtr::decode(u32::MAX), ZonePtr::Following);
        assert_eq!(ZonePtr::decode(u32::MAX - 1), ZonePtr::Insert);
    }

    #[test]
    fn header_is_forty_bytes_of_sizes() {
        let mut image = [0u8; XFILE_HEADER_LEN];
        image[0] = 9;
        image[8 + 4 * XFILE_BLOCK_STREAMER_RESERVE] = 7;
        let h = parse_zone_header(&image).unwrap();
        assert_eq!(XFILE_HEADER_LEN, 40);
        assert_eq!(h.size, 9);
        assert_eq!(h.block_size[XFILE_BLOCK_STREAMER_RESERVE], 7);
        assert!(parse_zone_header(&image[..39]).is_err());
    }
}
