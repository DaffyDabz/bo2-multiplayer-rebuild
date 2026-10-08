//! The 12-byte file preamble and, on signed zones, the auth header that
//! follows it. Everything after these is XChunk records (see `xchunk`).
//!
//! Signed retail PC zones (`TAff0100`) carry a 300-byte auth header:
//! `PHEEBs71`, a u32 of load flags (always zero on retail), the zone name in
//! a NUL-padded 32-byte field and a 256-byte RSA signature. IW4L reads game
//! files it does not own the keys to and never re-signs them, so the
//! signature is kept as bytes and not verified.

pub const MAGIC_SIGNED: &[u8; 8] = b"TAff0100";

pub const MAGIC_UNSIGNED: &[u8; 8] = b"TAffu100";

pub const MAGIC_UNSIGNED_SERVER: &[u8; 8] = b"TAsvu100";

pub const ZONE_VERSION_PC: u32 = 147;

pub const FILE_PREAMBLE_LEN: usize = 8 + 4;

pub const AUTH_MAGIC: &[u8; 8] = b"PHEEBs71";

pub const AUTH_NAME_LEN: usize = 32;

pub const AUTH_SIGNATURE_LEN: usize = 256;

pub const AUTH_HEADER_LEN: usize = 8 + 4 + AUTH_NAME_LEN + AUTH_SIGNATURE_LEN;

/// The name seeds the Salsa20 IV table; the game's buffer held 31 characters
/// and a terminator, so a longer name is cut to 31.
pub const ZONE_NAME_KEY_MAX: usize = AUTH_NAME_LEN - 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signing {
    /// `TAff0100`: auth header, Salsa20-encrypted chunks.
    Signed,
    /// `TAffu100`: no auth header, Salsa20-encrypted chunks keyed by the file name.
    Unsigned,
    /// `TAsvu100`: no auth header, plain chunks.
    UnsignedServer,
}

impl Signing {
    pub const fn is_encrypted(self) -> bool {
        matches!(self, Signing::Signed | Signing::Unsigned)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileHeader {
    pub signing: Signing,
    pub version: u32,
    /// The auth header's load flags; `None` on unsigned zones.
    pub auth_flags: Option<u32>,
    name: [u8; AUTH_NAME_LEN],
    name_len: usize,
    /// File offset of the first XChunk record.
    pub body_offset: usize,
}

impl FileHeader {
    /// The zone name stored in the auth header. Unsigned zones carry none;
    /// their chunks are keyed by the file name instead.
    pub fn zone_name(&self) -> Option<&str> {
        if self.signing != Signing::Signed {
            return None;
        }
        core::str::from_utf8(&self.name[..self.name_len]).ok()
    }

    pub fn signature<'a>(&self, file: &'a [u8]) -> Option<&'a [u8]> {
        if self.signing != Signing::Signed {
            return None;
        }
        let start = FILE_PREAMBLE_LEN + 8 + 4 + AUTH_NAME_LEN;
        file.get(start..start + AUTH_SIGNATURE_LEN)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileHeaderError {
    TooShort { len: usize, need: usize },
    BadMagic { got: [u8; 8] },
    BadVersion { got: u32 },
    BadAuthMagic { got: [u8; 8] },
    NameNotTerminated,
    NameNotUtf8,
}

impl core::fmt::Display for FileHeaderError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FileHeaderError::TooShort { len, need } => {
                write!(f, "file is {len} bytes, the T6 header needs {need}")
            }
            FileHeaderError::BadMagic { got } => write!(f, "not a T6 fastfile: magic {got:02x?}"),
            FileHeaderError::BadVersion { got } => {
                write!(f, "T6 zone version {got} (PC is {ZONE_VERSION_PC})")
            }
            FileHeaderError::BadAuthMagic { got } => {
                write!(f, "signed zone without PHEEBs71 auth header: {got:02x?}")
            }
            FileHeaderError::NameNotTerminated => {
                write!(f, "auth header zone name has no terminator")
            }
            FileHeaderError::NameNotUtf8 => write!(f, "auth header zone name is not UTF-8"),
        }
    }
}

/// True when `bytes` starts like a T6 fastfile of any platform or signing.
pub fn is_t6_magic(bytes: &[u8]) -> bool {
    bytes.len() >= 8
        && (&bytes[0..8] == MAGIC_SIGNED
            || &bytes[0..8] == MAGIC_UNSIGNED
            || &bytes[0..8] == MAGIC_UNSIGNED_SERVER)
}

pub fn parse_file_header(bytes: &[u8]) -> Result<FileHeader, FileHeaderError> {
    if bytes.len() < FILE_PREAMBLE_LEN {
        return Err(FileHeaderError::TooShort {
            len: bytes.len(),
            need: FILE_PREAMBLE_LEN,
        });
    }
    let mut magic = [0u8; 8];
    magic.copy_from_slice(&bytes[0..8]);
    let signing = if &magic == MAGIC_SIGNED {
        Signing::Signed
    } else if &magic == MAGIC_UNSIGNED {
        Signing::Unsigned
    } else if &magic == MAGIC_UNSIGNED_SERVER {
        Signing::UnsignedServer
    } else {
        return Err(FileHeaderError::BadMagic { got: magic });
    };
    let version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    if version != ZONE_VERSION_PC {
        return Err(FileHeaderError::BadVersion { got: version });
    }

    let mut name = [0u8; AUTH_NAME_LEN];
    if signing != Signing::Signed {
        return Ok(FileHeader {
            signing,
            version,
            auth_flags: None,
            name,
            name_len: 0,
            body_offset: FILE_PREAMBLE_LEN,
        });
    }

    let need = FILE_PREAMBLE_LEN + AUTH_HEADER_LEN;
    if bytes.len() < need {
        return Err(FileHeaderError::TooShort {
            len: bytes.len(),
            need,
        });
    }
    let auth = &bytes[FILE_PREAMBLE_LEN..need];
    let mut auth_magic = [0u8; 8];
    auth_magic.copy_from_slice(&auth[0..8]);
    if &auth_magic != AUTH_MAGIC {
        return Err(FileHeaderError::BadAuthMagic { got: auth_magic });
    }
    let flags = u32::from_le_bytes(auth[8..12].try_into().unwrap());
    name.copy_from_slice(&auth[12..12 + AUTH_NAME_LEN]);
    let name_len = name
        .iter()
        .position(|&b| b == 0)
        .ok_or(FileHeaderError::NameNotTerminated)?;
    if core::str::from_utf8(&name[..name_len]).is_err() {
        return Err(FileHeaderError::NameNotUtf8);
    }
    Ok(FileHeader {
        signing,
        version,
        auth_flags: Some(flags),
        name,
        name_len,
        body_offset: need,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signed(name: &[u8]) -> [u8; FILE_PREAMBLE_LEN + AUTH_HEADER_LEN] {
        let mut b = [0u8; FILE_PREAMBLE_LEN + AUTH_HEADER_LEN];
        b[0..8].copy_from_slice(MAGIC_SIGNED);
        b[8..12].copy_from_slice(&ZONE_VERSION_PC.to_le_bytes());
        b[12..20].copy_from_slice(AUTH_MAGIC);
        b[24..24 + name.len()].copy_from_slice(name);
        b
    }

    #[test]
    fn signed_header_names_the_zone_and_starts_chunks_at_312() {
        let b = signed(b"zm_nuked");
        let h = parse_file_header(&b).unwrap();
        assert_eq!(h.signing, Signing::Signed);
        assert_eq!(h.zone_name(), Some("zm_nuked"));
        assert_eq!(h.auth_flags, Some(0));
        assert_eq!(h.body_offset, 312);
        assert!(h.signing.is_encrypted());
    }

    #[test]
    fn server_zone_has_no_auth_header_and_no_cipher() {
        let mut b = [0u8; 16];
        b[0..8].copy_from_slice(MAGIC_UNSIGNED_SERVER);
        b[8..12].copy_from_slice(&ZONE_VERSION_PC.to_le_bytes());
        let h = parse_file_header(&b).unwrap();
        assert_eq!(h.body_offset, FILE_PREAMBLE_LEN);
        assert!(!h.signing.is_encrypted());
        assert_eq!(h.zone_name(), None);
    }

    #[test]
    fn other_versions_and_magics_are_refused() {
        let mut b = signed(b"x");
        b[8..12].copy_from_slice(&146u32.to_le_bytes());
        assert_eq!(
            parse_file_header(&b),
            Err(FileHeaderError::BadVersion { got: 146 })
        );
        let mut b = signed(b"x");
        b[0..4].copy_from_slice(b"IWff");
        assert!(matches!(
            parse_file_header(&b),
            Err(FileHeaderError::BadMagic { .. })
        ));
    }

    #[test]
    fn a_full_32_byte_name_is_refused() {
        let b = signed(&[b'a'; 32]);
        assert_eq!(
            parse_file_header(&b),
            Err(FileHeaderError::NameNotTerminated)
        );
    }
}
