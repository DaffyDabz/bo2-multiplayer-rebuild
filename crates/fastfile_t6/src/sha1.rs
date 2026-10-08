//! SHA-1 (FIPS 180-4), one-shot. The zone cipher hashes each decrypted
//! XChunk to derive the next chunk's IV; nothing here is used for security.

pub const DIGEST_LEN: usize = 20;

fn compress(h: &mut [u32; 5], block: &[u8]) {
    let mut w = [0u32; 80];
    for (i, word) in w.iter_mut().take(16).enumerate() {
        *word = u32::from_be_bytes([
            block[i * 4],
            block[i * 4 + 1],
            block[i * 4 + 2],
            block[i * 4 + 3],
        ]);
    }
    for i in 16..80 {
        w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
    }
    let [mut a, mut b, mut c, mut d, mut e] = *h;
    for (i, wi) in w.iter().enumerate() {
        let (f, k) = match i {
            0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
            20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
            40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
            _ => (b ^ c ^ d, 0xCA62_C1D6),
        };
        let t = a
            .rotate_left(5)
            .wrapping_add(f)
            .wrapping_add(e)
            .wrapping_add(k)
            .wrapping_add(*wi);
        e = d;
        d = c;
        c = b.rotate_left(30);
        b = a;
        a = t;
    }
    h[0] = h[0].wrapping_add(a);
    h[1] = h[1].wrapping_add(b);
    h[2] = h[2].wrapping_add(c);
    h[3] = h[3].wrapping_add(d);
    h[4] = h[4].wrapping_add(e);
}

pub fn digest(data: &[u8]) -> [u8; DIGEST_LEN] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut blocks = data.chunks_exact(64);
    for block in blocks.by_ref() {
        compress(&mut h, block);
    }
    let rest = blocks.remainder();
    let bit_len = (data.len() as u64).wrapping_mul(8);

    // Tail: remainder, 0x80, zero pad, 64-bit big-endian bit length.
    let mut tail = [0u8; 128];
    tail[..rest.len()].copy_from_slice(rest);
    tail[rest.len()] = 0x80;
    let tail_len = if rest.len() + 1 + 8 <= 64 { 64 } else { 128 };
    tail[tail_len - 8..tail_len].copy_from_slice(&bit_len.to_be_bytes());
    for block in tail[..tail_len].chunks_exact(64) {
        compress(&mut h, block);
    }

    let mut out = [0u8; DIGEST_LEN];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(d: [u8; DIGEST_LEN]) -> [u8; 40] {
        const H: &[u8; 16] = b"0123456789abcdef";
        let mut out = [0u8; 40];
        for (i, b) in d.iter().enumerate() {
            out[i * 2] = H[(b >> 4) as usize];
            out[i * 2 + 1] = H[(b & 15) as usize];
        }
        out
    }

    #[test]
    fn fips_vectors() {
        assert_eq!(
            &hex(digest(b"")),
            b"da39a3ee5e6b4b0d3255bfef95601890afd80709"
        );
        assert_eq!(
            &hex(digest(b"abc")),
            b"a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            &hex(digest(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            b"84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
    }

    #[test]
    fn padding_boundaries() {
        // 55 bytes fit one padded block, 56 spill into a second.
        let a = [b'a'; 64];
        assert_ne!(digest(&a[..55]), digest(&a[..56]));
        assert_ne!(digest(&a[..63]), digest(&a[..64]));
    }
}
