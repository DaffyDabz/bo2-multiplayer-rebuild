//! bo2zm: Black Ops II sound banks.
//!
//! A bank file (`sound/<bank>.<language>.sabl` loaded, `.sabs` streamed)
//! starts with a 2048-byte header (`"2UX#"`, version 14) and ends with its
//! entry table; each entry names one sound by the hash of its file name
//! (the alias's `assetId`) and says where its bytes are, how many frames,
//! the rate, channels and format. Measured on the Steam install
//! (2026-10-01): every loaded bank holds 16-bit PCM, every streamed bank
//! FLAC, all at 48 kHz.
//!
//! [`flac`] is our own decoder (RFC 9639), checked against each stream's
//! own MD5 of the decoded audio. Not part of upstream IW4L.

pub mod bank;
pub mod flac;
pub mod md5;

pub use bank::{BankEntry, BankFile, BankHeader, SoundFormat, sound_hash};
pub use flac::{FlacError, FlacStream, decode_flac};

/// Decoded audio: interleaved 16-bit samples.
#[derive(Clone, Debug, Default)]
pub struct Pcm16 {
    pub rate: u32,
    pub channels: u16,
    pub samples: Vec<i16>,
}

impl Pcm16 {
    pub fn frames(&self) -> usize {
        if self.channels == 0 {
            0
        } else {
            self.samples.len() / usize::from(self.channels)
        }
    }
}
