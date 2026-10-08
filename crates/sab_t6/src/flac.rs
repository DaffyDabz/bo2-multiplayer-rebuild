//! FLAC decoding (RFC 9639): stream info, frames, constant / verbatim /
//! fixed / LPC subframes, partitioned Rice residuals, stereo
//! decorrelation, frame CRCs. Written for the bo2zm fork; output is
//! interleaved 16-bit audio, checked against the stream's own MD5 when it
//! carries one.

use std::fmt;

use crate::Pcm16;
use crate::md5::Md5;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlacError {
    NotFlac,
    Eof,
    BadMetadata(&'static str),
    BadFrame(&'static str, usize),
    Crc(&'static str, usize),
}

impl fmt::Display for FlacError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFlac => write!(f, "no fLaC marker"),
            Self::Eof => write!(f, "data ends inside a frame"),
            Self::BadMetadata(what) => write!(f, "bad metadata: {what}"),
            Self::BadFrame(what, at) => write!(f, "bad frame at byte {at}: {what}"),
            Self::Crc(what, at) => write!(f, "{what} mismatch in frame at byte {at}"),
        }
    }
}

type Result<T> = std::result::Result<T, FlacError>;

/// `STREAMINFO`.
#[derive(Clone, Copy, Debug, Default)]
pub struct StreamInfo {
    pub min_block: u16,
    pub max_block: u16,
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u32,
    pub total_samples: u64,
    pub md5: [u8; 16],
}

/// A decoded stream: its info, the audio, and whether the audio matched
/// the stream's MD5 (`None` when the stream carries none).
#[derive(Clone, Debug)]
pub struct FlacStream {
    pub info: StreamInfo,
    pub pcm: Pcm16,
    pub md5_ok: Option<bool>,
}

struct Bits<'a> {
    data: &'a [u8],
    /// Bit position.
    pos: usize,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8], byte: usize) -> Self {
        Self {
            data,
            pos: byte * 8,
        }
    }

    #[inline]
    fn byte_pos(&self) -> usize {
        self.pos >> 3
    }

    /// The next 64 bits (zeros past the end), most significant first.
    #[inline]
    fn peek64(&self) -> u64 {
        let byte = self.pos >> 3;
        let mut buf = [0u8; 8];
        if byte < self.data.len() {
            let n = (self.data.len() - byte).min(8);
            buf[..n].copy_from_slice(&self.data[byte..byte + n]);
        }
        u64::from_be_bytes(buf) << (self.pos & 7)
    }

    #[inline]
    fn read(&mut self, n: u32) -> Result<u32> {
        if n == 0 {
            return Ok(0);
        }
        debug_assert!(n <= 32);
        if self.pos + n as usize > self.data.len() * 8 {
            return Err(FlacError::Eof);
        }
        let v = (self.peek64() >> (64 - n)) as u32;
        self.pos += n as usize;
        Ok(v)
    }

    #[inline]
    fn read_signed(&mut self, n: u32) -> Result<i32> {
        if n == 0 {
            return Ok(0);
        }
        let v = self.read(n)?;
        let shift = 32 - n;
        Ok(((v << shift) as i32) >> shift)
    }

    /// Zeros up to the next one bit; the one is consumed.
    #[inline]
    fn read_unary(&mut self) -> Result<u32> {
        let mut count = 0u32;
        loop {
            let w = self.peek64();
            if w != 0 {
                let lz = w.leading_zeros();
                self.pos += lz as usize + 1;
                if self.pos > self.data.len() * 8 {
                    return Err(FlacError::Eof);
                }
                return Ok(count + lz);
            }
            count += 56;
            self.pos += 56;
            if self.pos > self.data.len() * 8 {
                return Err(FlacError::Eof);
            }
        }
    }

    fn align(&mut self) {
        self.pos = (self.pos + 7) & !7;
    }
}

fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in data {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
        }
    }
    crc
}

fn crc16_table() -> &'static [u16; 256] {
    static TABLE: std::sync::OnceLock<[u16; 256]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t = [0u16; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut c = (i as u16) << 8;
            for _ in 0..8 {
                c = if c & 0x8000 != 0 {
                    (c << 1) ^ 0x8005
                } else {
                    c << 1
                };
            }
            *e = c;
        }
        t
    })
}

fn crc16(data: &[u8]) -> u16 {
    let t = crc16_table();
    let mut crc = 0u16;
    for &b in data {
        crc = (crc << 8) ^ t[usize::from((crc >> 8) as u8 ^ b)];
    }
    crc
}

/// Read the metadata blocks; returns the stream info and the byte where
/// the first frame starts.
pub fn read_stream_info(data: &[u8]) -> Result<(StreamInfo, usize)> {
    if data.len() < 4 || &data[0..4] != b"fLaC" {
        return Err(FlacError::NotFlac);
    }
    let mut at = 4usize;
    let mut info = None;
    loop {
        let head = data.get(at..at + 4).ok_or(FlacError::Eof)?;
        let last = head[0] & 0x80 != 0;
        let ty = head[0] & 0x7f;
        let len = (usize::from(head[1]) << 16) | (usize::from(head[2]) << 8) | usize::from(head[3]);
        let body = data.get(at + 4..at + 4 + len).ok_or(FlacError::Eof)?;
        if ty == 0 {
            if len < 34 {
                return Err(FlacError::BadMetadata("STREAMINFO too short"));
            }
            let mut b = Bits::new(body, 0);
            let min_block = b.read(16)? as u16;
            let max_block = b.read(16)? as u16;
            let _min_frame = b.read(24)?;
            let _max_frame = b.read(24)?;
            let sample_rate = b.read(20)?;
            let channels = b.read(3)? as u16 + 1;
            let bits_per_sample = b.read(5)? + 1;
            let total_hi = u64::from(b.read(4)?);
            let total_lo = u64::from(b.read(32)?);
            let mut md5 = [0u8; 16];
            md5.copy_from_slice(&body[18..34]);
            info = Some(StreamInfo {
                min_block,
                max_block,
                sample_rate,
                channels,
                bits_per_sample,
                total_samples: (total_hi << 32) | total_lo,
                md5,
            });
        }
        at += 4 + len;
        if last {
            break;
        }
    }
    let info = info.ok_or(FlacError::BadMetadata("no STREAMINFO"))?;
    Ok((info, at))
}

/// Decode a whole FLAC stream.
pub fn decode_flac(data: &[u8]) -> Result<FlacStream> {
    let (info, first) = read_stream_info(data)?;
    let channels = usize::from(info.channels);
    let mut samples: Vec<i16> =
        Vec::with_capacity(info.total_samples as usize * channels);
    let mut md5 = (info.md5 != [0u8; 16]).then(Md5::new);
    let mut bits = Bits::new(data, first);
    let mut chans: Vec<Vec<i32>> = vec![Vec::new(); 8];
    let mut md5_bytes: Vec<u8> = Vec::new();
    while bits.byte_pos() + 2 <= data.len() {
        // Trailing padding after the last frame ends the stream.
        let start = bits.byte_pos();
        if data[start] != 0xff || data[start + 1] & 0xfe != 0xf8 {
            if info.total_samples != 0
                && samples.len() as u64 >= info.total_samples * channels as u64
            {
                break;
            }
            return Err(FlacError::BadFrame("no sync code", start));
        }
        let (block, frame_channels, bps) = decode_frame(&mut bits, &info, &mut chans)?;
        // Interleave as 16-bit.
        let shift_down = bps.saturating_sub(16);
        let shift_up = 16u32.saturating_sub(bps);
        for i in 0..block {
            for c in chans.iter().take(frame_channels) {
                let v = c[i];
                let s = if shift_down > 0 {
                    v >> shift_down
                } else {
                    v << shift_up
                };
                samples.push(s.clamp(i16::MIN as i32, i16::MAX as i32) as i16);
            }
        }
        if let Some(m) = md5.as_mut() {
            let bytes = (bps as usize).div_ceil(8);
            md5_bytes.clear();
            for i in 0..block {
                for c in chans.iter().take(frame_channels) {
                    let v = c[i];
                    md5_bytes.extend_from_slice(&v.to_le_bytes()[..bytes]);
                }
            }
            m.update(&md5_bytes);
        }
        if info.total_samples != 0 && samples.len() as u64 >= info.total_samples * channels as u64 {
            break;
        }
    }
    let md5_ok = md5.map(|m| m.finish() == info.md5);
    Ok(FlacStream {
        info,
        pcm: Pcm16 {
            rate: info.sample_rate,
            channels: info.channels,
            samples,
        },
        md5_ok,
    })
}

/// One frame into `chans` (per channel, `block` samples each). Returns
/// (block size, channels, bits per sample).
fn decode_frame(
    bits: &mut Bits<'_>,
    info: &StreamInfo,
    chans: &mut [Vec<i32>],
) -> Result<(usize, usize, u32)> {
    let start = bits.byte_pos();
    let sync = bits.read(14)?;
    if sync != 0x3ffe {
        return Err(FlacError::BadFrame("sync", start));
    }
    let _reserved = bits.read(1)?;
    let _blocking = bits.read(1)?;
    let bs_code = bits.read(4)?;
    let sr_code = bits.read(4)?;
    let ch_code = bits.read(4)?;
    let ss_code = bits.read(3)?;
    let _reserved2 = bits.read(1)?;
    // The coded frame / sample number (UTF-8 style).
    let first = bits.read(8)?;
    let extra = match first {
        0x00..=0x7f => 0,
        0xc0..=0xdf => 1,
        0xe0..=0xef => 2,
        0xf0..=0xf7 => 3,
        0xf8..=0xfb => 4,
        0xfc..=0xfd => 5,
        0xfe => 6,
        _ => return Err(FlacError::BadFrame("coded number", start)),
    };
    for _ in 0..extra {
        if bits.read(8)? & 0xc0 != 0x80 {
            return Err(FlacError::BadFrame("coded number continuation", start));
        }
    }
    let block = match bs_code {
        0 => return Err(FlacError::BadFrame("block size code 0", start)),
        1 => 192,
        2..=5 => 576usize << (bs_code - 2),
        6 => bits.read(8)? as usize + 1,
        7 => bits.read(16)? as usize + 1,
        _ => 256usize << (bs_code - 8),
    };
    match sr_code {
        12 => {
            bits.read(8)?;
        }
        13 | 14 => {
            bits.read(16)?;
        }
        15 => return Err(FlacError::BadFrame("sample rate code 15", start)),
        _ => {}
    }
    let header_end = bits.byte_pos();
    let crc = bits.read(8)? as u8;
    if crc8(&bits.data[start..header_end]) != crc {
        return Err(FlacError::Crc("header CRC-8", start));
    }
    let bps = match ss_code {
        0 => info.bits_per_sample,
        1 => 8,
        2 => 12,
        4 => 16,
        5 => 20,
        6 => 24,
        7 => 32,
        _ => return Err(FlacError::BadFrame("sample size code 3", start)),
    };
    let channels = match ch_code {
        0..=7 => ch_code as usize + 1,
        8..=10 => 2,
        _ => return Err(FlacError::BadFrame("channel code", start)),
    };
    if channels > chans.len() {
        return Err(FlacError::BadFrame("too many channels", start));
    }
    for (c, out) in chans.iter_mut().enumerate().take(channels) {
        let side = matches!((ch_code, c), (8, 1) | (9, 0) | (10, 1));
        let sub_bps = bps + u32::from(side);
        if sub_bps > 32 {
            return Err(FlacError::BadFrame("sample size over 32 bits", start));
        }
        decode_subframe(bits, block, sub_bps, out).map_err(|e| match e {
            FlacError::BadFrame(what, _) => FlacError::BadFrame(what, start),
            other => other,
        })?;
    }
    bits.align();
    let crc_at = bits.byte_pos();
    let crc = bits.read(16)? as u16;
    if crc16(&bits.data[start..crc_at]) != crc {
        return Err(FlacError::Crc("frame CRC-16", start));
    }
    // Stereo decorrelation.
    match ch_code {
        8 => {
            // left, side: right = left - side
            let (l, r) = chans.split_at_mut(1);
            for i in 0..block {
                r[0][i] = l[0][i].wrapping_sub(r[0][i]);
            }
        }
        9 => {
            // side, right: left = side + right
            let (l, r) = chans.split_at_mut(1);
            for i in 0..block {
                l[0][i] = l[0][i].wrapping_add(r[0][i]);
            }
        }
        10 => {
            // mid, side
            let (m, s) = chans.split_at_mut(1);
            for i in 0..block {
                let side = s[0][i];
                let mid = (m[0][i] << 1) | (side & 1);
                m[0][i] = (mid + side) >> 1;
                s[0][i] = (mid - side) >> 1;
            }
        }
        _ => {}
    }
    Ok((block, channels, bps))
}

fn decode_subframe(bits: &mut Bits<'_>, block: usize, bps: u32, out: &mut Vec<i32>) -> Result<()> {
    out.clear();
    if bits.read(1)? != 0 {
        return Err(FlacError::BadFrame("subframe padding bit", 0));
    }
    let ty = bits.read(6)?;
    let wasted = if bits.read(1)? != 0 {
        bits.read_unary()? + 1
    } else {
        0
    };
    if wasted >= bps {
        return Err(FlacError::BadFrame("wasted bits", 0));
    }
    let bps = bps - wasted;
    match ty {
        0 => {
            let v = bits.read_signed(bps)?;
            out.resize(block, v);
        }
        1 => {
            for _ in 0..block {
                out.push(bits.read_signed(bps)?);
            }
        }
        8..=12 => {
            let order = (ty - 8) as usize;
            if order > block {
                return Err(FlacError::BadFrame("fixed order over block", 0));
            }
            for _ in 0..order {
                out.push(bits.read_signed(bps)?);
            }
            residual(bits, block, order, out)?;
            match order {
                0 => {}
                1 => {
                    for i in 1..block {
                        out[i] = out[i].wrapping_add(out[i - 1]);
                    }
                }
                2 => {
                    for i in 2..block {
                        let p = 2 * i64::from(out[i - 1]) - i64::from(out[i - 2]);
                        out[i] = (i64::from(out[i]) + p) as i32;
                    }
                }
                3 => {
                    for i in 3..block {
                        let p = 3 * i64::from(out[i - 1]) - 3 * i64::from(out[i - 2])
                            + i64::from(out[i - 3]);
                        out[i] = (i64::from(out[i]) + p) as i32;
                    }
                }
                _ => {
                    for i in 4..block {
                        let p = 4 * i64::from(out[i - 1]) - 6 * i64::from(out[i - 2])
                            + 4 * i64::from(out[i - 3])
                            - i64::from(out[i - 4]);
                        out[i] = (i64::from(out[i]) + p) as i32;
                    }
                }
            }
        }
        32..=63 => {
            let order = (ty - 31) as usize;
            if order > block {
                return Err(FlacError::BadFrame("lpc order over block", 0));
            }
            for _ in 0..order {
                out.push(bits.read_signed(bps)?);
            }
            let precision = bits.read(4)?;
            if precision == 15 {
                return Err(FlacError::BadFrame("lpc precision 15", 0));
            }
            let precision = precision + 1;
            let shift = bits.read_signed(5)?;
            if shift < 0 {
                return Err(FlacError::BadFrame("negative lpc shift", 0));
            }
            let mut coefs = [0i64; 32];
            for c in coefs.iter_mut().take(order) {
                *c = i64::from(bits.read_signed(precision)?);
            }
            residual(bits, block, order, out)?;
            for i in order..block {
                let mut sum = 0i64;
                for (j, c) in coefs.iter().take(order).enumerate() {
                    sum += c * i64::from(out[i - 1 - j]);
                }
                out[i] = (i64::from(out[i]) + (sum >> shift)) as i32;
            }
        }
        _ => return Err(FlacError::BadFrame("reserved subframe type", 0)),
    }
    if wasted > 0 {
        for s in out.iter_mut() {
            *s <<= wasted;
        }
    }
    Ok(())
}

fn residual(bits: &mut Bits<'_>, block: usize, order: usize, out: &mut Vec<i32>) -> Result<()> {
    let method = bits.read(2)?;
    let (param_bits, escape) = match method {
        0 => (4, 15),
        1 => (5, 31),
        _ => return Err(FlacError::BadFrame("residual coding method", 0)),
    };
    let porder = bits.read(4)?;
    let partitions = 1usize << porder;
    if block % partitions != 0 {
        return Err(FlacError::BadFrame("partition order", 0));
    }
    let per = block >> porder;
    if per < order {
        return Err(FlacError::BadFrame("partition shorter than order", 0));
    }
    for p in 0..partitions {
        let count = if p == 0 { per - order } else { per };
        let param = bits.read(param_bits)?;
        if param == escape {
            let raw = bits.read(5)?;
            for _ in 0..count {
                out.push(bits.read_signed(raw)?);
            }
        } else {
            for _ in 0..count {
                let q = u64::from(bits.read_unary()?);
                let r = u64::from(bits.read(param)?);
                let v = (q << param) | r;
                let s = ((v >> 1) as i64) ^ -((v & 1) as i64);
                out.push(s as i32);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crcs_match_known_values() {
        // CRC-8 (poly 0x07) of "123456789" is 0xF4; CRC-16 (poly 0x8005,
        // no reflection, init 0) is 0xFEE8.
        assert_eq!(crc8(b"123456789"), 0xf4);
        assert_eq!(crc16(b"123456789"), 0xfee8);
    }

    #[test]
    fn unary_and_signed_reads() {
        let data = [0b0001_0110, 0b1000_0000];
        let mut b = Bits::new(&data, 0);
        assert_eq!(b.read_unary().unwrap(), 3);
        // Next three bits: 011.
        assert_eq!(b.read_signed(3).unwrap(), 3);
        let data = [0b1100_0000];
        let mut b = Bits::new(&data, 0);
        // 110 as a 3-bit signed value.
        assert_eq!(b.read_signed(3).unwrap(), -2);
    }
}
