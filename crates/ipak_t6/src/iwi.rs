//! IWI v27, Black Ops II's image file: `IWi` + version byte 27, a 60-byte
//! header (format, flags, width/height/depth, gamma, 16 gloss bytes, eight
//! cumulative file sizes), then the mip levels from the smallest to the
//! largest, every face of a level together. The cumulative sizes let a
//! reader prove its own size arithmetic: after reading level `i` (i < 8) the
//! file position must equal `file_size_for_picmip[i]`.

pub const IWI_HEADER_LEN: usize = 4 + 60;

const FLAG_NOMIPMAPS: u8 = 1 << 1;
const FLAG_CUBEMAP: u8 = 1 << 2;
const FLAG_VOLMAP: u8 = 1 << 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IwiFormat {
    Rgba8,
    Rgb8,
    LuminanceAlpha,
    Luminance,
    Alpha,
    Dxt1,
    Dxt3,
    Dxt5,
    /// Two-channel BC5 (normal maps).
    Dxn,
    Rgba16F,
}

impl IwiFormat {
    fn from_byte(b: u8) -> Option<IwiFormat> {
        Some(match b {
            0x01 => IwiFormat::Rgba8,
            0x02 => IwiFormat::Rgb8,
            0x03 => IwiFormat::LuminanceAlpha,
            0x04 => IwiFormat::Luminance,
            0x05 => IwiFormat::Alpha,
            0x0B => IwiFormat::Dxt1,
            0x0C => IwiFormat::Dxt3,
            0x0D => IwiFormat::Dxt5,
            0x0E => IwiFormat::Dxn,
            0x13 => IwiFormat::Rgba16F,
            _ => return None,
        })
    }

    pub fn is_block_compressed(self) -> bool {
        matches!(
            self,
            IwiFormat::Dxt1 | IwiFormat::Dxt3 | IwiFormat::Dxt5 | IwiFormat::Dxn
        )
    }

    /// Bytes per 4x4 block, or per pixel for uncompressed formats.
    pub fn unit_bytes(self) -> usize {
        match self {
            IwiFormat::Dxt1 => 8,
            IwiFormat::Dxt3 | IwiFormat::Dxt5 | IwiFormat::Dxn => 16,
            IwiFormat::Rgba8 => 4,
            IwiFormat::Rgb8 => 3,
            IwiFormat::LuminanceAlpha => 2,
            IwiFormat::Luminance | IwiFormat::Alpha => 1,
            IwiFormat::Rgba16F => 8,
        }
    }

    pub fn level_bytes(self, width: u32, height: u32, depth: u32) -> usize {
        let (w, h, d) = (
            width.max(1) as usize,
            height.max(1) as usize,
            depth.max(1) as usize,
        );
        if self.is_block_compressed() {
            w.div_ceil(4) * h.div_ceil(4) * d * self.unit_bytes()
        } else {
            w * h * d * self.unit_bytes()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IwiError {
    TooShort {
        need: usize,
        have: usize,
    },
    BadTag([u8; 4]),
    UnknownFormat(u8),
    SizeMismatch {
        level: usize,
        at: usize,
        header: u32,
    },
}

impl core::fmt::Display for IwiError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            IwiError::TooShort { need, have } => write!(f, "iwi needs {need} bytes, has {have}"),
            IwiError::BadTag(t) => write!(f, "not an IWi v27 file: {t:02x?}"),
            IwiError::UnknownFormat(b) => write!(f, "iwi format {b:#x} not handled"),
            IwiError::SizeMismatch { level, at, header } => {
                write!(f, "iwi mip {level} ends at {at}, header says {header}")
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct IwiImage<'a> {
    pub format: IwiFormat,
    pub flags: u8,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub gamma: f32,
    pub faces: u32,
    pub levels: u32,
    data: &'a [u8],
}

impl<'a> IwiImage<'a> {
    pub fn is_cube(&self) -> bool {
        self.flags & FLAG_CUBEMAP != 0
    }

    pub fn is_volume(&self) -> bool {
        self.flags & FLAG_VOLMAP != 0
    }

    pub fn level_dims(&self, level: u32) -> (u32, u32, u32) {
        (
            (self.width >> level).max(1),
            (self.height >> level).max(1),
            (self.depth >> level).max(1),
        )
    }

    /// One face of one level (level 0 is the largest).
    pub fn level_bytes(&self, level: u32) -> usize {
        let (w, h, d) = self.level_dims(level);
        self.format.level_bytes(w, h, d)
    }

    /// All faces of mip `level`, faces back to back.
    pub fn level(&self, level: u32) -> &'a [u8] {
        // Levels are stored smallest first.
        let mut at = 0usize;
        for l in (level + 1..self.levels).rev() {
            at += self.level_bytes(l) * self.faces as usize;
        }
        &self.data[at..at + self.level_bytes(level) * self.faces as usize]
    }
}

fn mip_count(w: u32, h: u32, d: u32) -> u32 {
    let mut n = 1;
    let (mut w, mut h, mut d) = (w, h, d);
    while w > 1 || h > 1 || d > 1 {
        w = (w / 2).max(1);
        h = (h / 2).max(1);
        d = (d / 2).max(1);
        n += 1;
    }
    n
}

pub fn parse_iwi(bytes: &[u8]) -> Result<IwiImage<'_>, IwiError> {
    if bytes.len() < IWI_HEADER_LEN {
        return Err(IwiError::TooShort {
            need: IWI_HEADER_LEN,
            have: bytes.len(),
        });
    }
    let mut tag = [0u8; 4];
    tag.copy_from_slice(&bytes[0..4]);
    if &tag != b"IWi\x1b" {
        return Err(IwiError::BadTag(tag));
    }
    let h = &bytes[4..IWI_HEADER_LEN];
    let format = IwiFormat::from_byte(h[0]).ok_or(IwiError::UnknownFormat(h[0]))?;
    let flags = h[1];
    let rd16 = |o: usize| u16::from_le_bytes([h[o], h[o + 1]]) as u32;
    let rd32 = |o: usize| u32::from_le_bytes([h[o], h[o + 1], h[o + 2], h[o + 3]]);
    let (width, height, depth) = (rd16(2), rd16(4), rd16(6));
    let gamma = f32::from_bits(rd32(8));
    let faces = if flags & FLAG_CUBEMAP != 0 { 6 } else { 1 };
    let levels = if flags & FLAG_NOMIPMAPS != 0 {
        1
    } else {
        mip_count(
            width,
            height,
            if flags & FLAG_VOLMAP != 0 { depth } else { 1 },
        )
    };
    let mut image = IwiImage {
        format,
        flags,
        width,
        height,
        depth: if flags & FLAG_VOLMAP != 0 { depth } else { 1 },
        gamma,
        faces,
        levels,
        data: &[],
    };
    let mut at = IWI_HEADER_LEN;
    for level in (0..levels).rev() {
        at += image.level_bytes(level) * faces as usize;
        if level < 8 {
            let header = rd32(28 + level as usize * 4);
            if at != header as usize {
                return Err(IwiError::SizeMismatch {
                    level: level as usize,
                    at,
                    header,
                });
            }
        }
    }
    if bytes.len() < at {
        return Err(IwiError::TooShort {
            need: at,
            have: bytes.len(),
        });
    }
    image.data = &bytes[IWI_HEADER_LEN..at];
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    fn iwi(format: u8, flags: u8, w: u16, h: u16) -> Vec<u8> {
        let fmt = IwiFormat::from_byte(format).unwrap();
        let levels = if flags & FLAG_NOMIPMAPS != 0 {
            1
        } else {
            mip_count(w as u32, h as u32, 1)
        };
        let mut sizes = [0u32; 8];
        let mut at = IWI_HEADER_LEN;
        for level in (0..levels).rev() {
            at += fmt.level_bytes((w as u32 >> level).max(1), (h as u32 >> level).max(1), 1);
            if level < 8 {
                sizes[level as usize] = at as u32;
            }
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"IWi\x1b");
        out.push(format);
        out.push(flags);
        out.extend_from_slice(&w.to_le_bytes());
        out.extend_from_slice(&h.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1.0f32.to_le_bytes());
        out.extend_from_slice(&[0u8; 16]);
        for s in sizes {
            out.extend_from_slice(&s.to_le_bytes());
        }
        out.resize(at, 0xAB);
        out
    }

    #[test]
    fn dxt1_mip_chain_matches_its_own_sizes() {
        let file = iwi(0x0B, 0, 64, 32);
        let img = parse_iwi(&file).unwrap();
        assert_eq!(img.levels, 7);
        assert_eq!(img.level(0).len(), 16 * 8 * 8);
        assert_eq!(img.level(6).len(), 8);
    }

    #[test]
    fn a_wrong_size_table_is_refused() {
        let mut file = iwi(0x0D, 0, 16, 16);
        file[4 + 28] ^= 1;
        assert!(matches!(
            parse_iwi(&file),
            Err(IwiError::SizeMismatch { .. })
        ));
    }
}
