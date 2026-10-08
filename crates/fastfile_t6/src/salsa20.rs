//! Salsa20/20 with a 256-bit key and a 64-bit nonce, written from
//! D. J. Bernstein's specification ("Salsa20 specification", 2005). The zone
//! cipher restarts the block counter at zero for every XChunk, so the only
//! entry point is a whole-buffer XOR from counter 0.

const SIGMA: [u32; 4] = [0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574];

#[inline(always)]
fn quarter(x: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    x[b] ^= x[a].wrapping_add(x[d]).rotate_left(7);
    x[c] ^= x[b].wrapping_add(x[a]).rotate_left(9);
    x[d] ^= x[c].wrapping_add(x[b]).rotate_left(13);
    x[a] ^= x[d].wrapping_add(x[c]).rotate_left(18);
}

fn block(input: &[u32; 16]) -> [u8; 64] {
    let mut x = *input;
    for _ in 0..10 {
        // Column round.
        quarter(&mut x, 0, 4, 8, 12);
        quarter(&mut x, 5, 9, 13, 1);
        quarter(&mut x, 10, 14, 2, 6);
        quarter(&mut x, 15, 3, 7, 11);
        // Row round.
        quarter(&mut x, 0, 1, 2, 3);
        quarter(&mut x, 5, 6, 7, 4);
        quarter(&mut x, 10, 11, 8, 9);
        quarter(&mut x, 15, 12, 13, 14);
    }
    let mut out = [0u8; 64];
    for (i, word) in x.iter().enumerate() {
        let sum = word.wrapping_add(input[i]);
        out[i * 4..i * 4 + 4].copy_from_slice(&sum.to_le_bytes());
    }
    out
}

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// XOR `data` with the Salsa20/20 keystream for (`key`, `nonce`), block
/// counter starting at 0.
pub fn xor_keystream(key: &[u8; 32], nonce: &[u8; 8], data: &mut [u8]) {
    let mut state = [0u32; 16];
    state[0] = SIGMA[0];
    state[1] = word(key, 0);
    state[2] = word(key, 4);
    state[3] = word(key, 8);
    state[4] = word(key, 12);
    state[5] = SIGMA[1];
    state[6] = word(nonce, 0);
    state[7] = word(nonce, 4);
    state[8] = 0;
    state[9] = 0;
    state[10] = SIGMA[2];
    state[11] = word(key, 16);
    state[12] = word(key, 20);
    state[13] = word(key, 24);
    state[14] = word(key, 28);
    state[15] = SIGMA[3];

    for chunk in data.chunks_mut(64) {
        let stream = block(&state);
        for (byte, k) in chunk.iter_mut().zip(stream.iter()) {
            *byte ^= k;
        }
        state[8] = state[8].wrapping_add(1);
        if state[8] == 0 {
            state[9] = state[9].wrapping_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xor_twice_is_identity() {
        let key = [7u8; 32];
        let nonce = [3u8; 8];
        let mut data = [0u8; 200];
        for (i, b) in data.iter_mut().enumerate() {
            *b = i as u8;
        }
        let original = data;
        xor_keystream(&key, &nonce, &mut data);
        assert_ne!(data, original);
        xor_keystream(&key, &nonce, &mut data);
        assert_eq!(data, original);
    }

    #[test]
    fn estream_set1_vector0_256_bit_key() {
        // eSTREAM Salsa20/20 test vectors, 256-bit key, set 1 vector 0:
        // key = 80 00..00, IV = 0, stream[0..63].
        let mut key = [0u8; 32];
        key[0] = 0x80;
        let mut stream = [0u8; 64];
        xor_keystream(&key, &[0u8; 8], &mut stream);
        let want: [u8; 64] = [
            0xE3, 0xBE, 0x8F, 0xDD, 0x8B, 0xEC, 0xA2, 0xE3, 0xEA, 0x8E, 0xF9, 0x47, 0x5B, 0x29,
            0xA6, 0xE7, 0x00, 0x39, 0x51, 0xE1, 0x09, 0x7A, 0x5C, 0x38, 0xD2, 0x3B, 0x7A, 0x5F,
            0xAD, 0x9F, 0x68, 0x44, 0xB2, 0x2C, 0x97, 0x55, 0x9E, 0x27, 0x23, 0xC7, 0xCB, 0xBD,
            0x3F, 0xE4, 0xFC, 0x8D, 0x9A, 0x07, 0x44, 0x65, 0x2A, 0x83, 0xE7, 0x2A, 0x9C, 0x46,
            0x18, 0x76, 0xAF, 0x4D, 0x7E, 0xF1, 0xA1, 0x17,
        ];
        assert_eq!(stream, want);
    }

    #[test]
    fn keystream_is_continuous_across_64_byte_blocks() {
        let key = [1u8; 32];
        let nonce = [2u8; 8];
        let mut long = [0u8; 128];
        xor_keystream(&key, &nonce, &mut long);
        let mut first = [0u8; 64];
        xor_keystream(&key, &nonce, &mut first);
        assert_eq!(&long[..64], &first[..]);
        assert_ne!(&long[64..], &first[..]);
    }
}
