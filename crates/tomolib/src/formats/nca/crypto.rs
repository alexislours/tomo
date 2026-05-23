use aes::Aes128;
use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use ctr::cipher::{KeyIvInit, StreamCipher, StreamCipherSeek};

pub(crate) const BLOCK: usize = 16;

type Aes128Ctr = ctr::Ctr64BE<Aes128>;

#[derive(Clone)]
pub(crate) struct Aes128Ecb {
    cipher: Aes128,
}

impl std::fmt::Debug for Aes128Ecb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Aes128Ecb")
    }
}

impl Aes128Ecb {
    #[must_use]
    pub(crate) fn new(key: &[u8; BLOCK]) -> Self {
        Self {
            cipher: Aes128::new(GenericArray::from_slice(key)),
        }
    }

    #[must_use]
    pub(crate) fn decrypt_block(&self, block: &[u8; BLOCK]) -> [u8; BLOCK] {
        let mut b = GenericArray::clone_from_slice(block);
        self.cipher.decrypt_block(&mut b);
        b.into()
    }

    fn encrypt_into(&self, block: &mut [u8; BLOCK]) {
        let mut b = GenericArray::clone_from_slice(block);
        self.cipher.encrypt_block(&mut b);
        block.copy_from_slice(&b);
    }
}

#[must_use]
pub(crate) fn aes128_ecb_decrypt(key: &[u8; BLOCK], src: &[u8; BLOCK]) -> [u8; BLOCK] {
    Aes128Ecb::new(key).decrypt_block(src)
}

#[inline]
fn gf_mul_x(tweak: &mut [u8; BLOCK]) {
    let mut carry = 0u8;
    for b in tweak.iter_mut() {
        let next = *b >> 7;
        *b = (*b << 1) | carry;
        carry = next;
    }
    if carry != 0 {
        tweak[0] ^= 0x87;
    }
}

pub(crate) fn xts_decrypt_sector(
    data_cipher: &Aes128Ecb,
    tweak_cipher: &Aes128Ecb,
    sector: u64,
    buf: &mut [u8],
) {
    let mut tweak = [0u8; BLOCK];
    tweak[8..].copy_from_slice(&sector.to_be_bytes());
    tweak_cipher.encrypt_into(&mut tweak);

    for chunk in buf.chunks_mut(BLOCK) {
        for (b, t) in chunk.iter_mut().zip(tweak.iter()) {
            *b ^= *t;
        }
        let mut block = [0u8; BLOCK];
        block.copy_from_slice(chunk);
        let dec = data_cipher.decrypt_block(&block);
        for ((b, d), t) in chunk.iter_mut().zip(dec.iter()).zip(tweak.iter()) {
            *b = *d ^ *t;
        }
        gf_mul_x(&mut tweak);
    }
}

pub(crate) fn ctr_apply(
    key: &[u8; BLOCK],
    base_ctr: &[u8; BLOCK],
    abs_offset: u64,
    buf: &mut [u8],
) {
    let mut cipher = Aes128Ctr::new(key.into(), base_ctr.into());
    cipher.seek(abs_offset);
    cipher.apply_keystream(buf);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctr_is_xor_involution() {
        let key = [0x42; BLOCK];
        let base = [1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0, 0, 0, 0, 0];
        let plain: Vec<u8> = (0..500u32).map(|i| i.to_le_bytes()[0]).collect();
        let mut buf = plain.clone();
        ctr_apply(&key, &base, 0, &mut buf);
        assert_ne!(buf, plain);
        ctr_apply(&key, &base, 0, &mut buf);
        assert_eq!(buf, plain);
    }

    #[test]
    fn ctr_window_matches_full_stream() {
        let key = [0x9a; BLOCK];
        let base = [
            0xde, 0xad, 0xbe, 0xef, 0x11, 0x22, 0x33, 0x44, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let plain: Vec<u8> = (0..1000u32).map(|i| (i * 7).to_le_bytes()[0]).collect();

        let mut full = plain.clone();
        ctr_apply(&key, &base, 0, &mut full);

        for (start, end) in [(0usize, 16), (37, 620), (511, 512), (999, 1000)] {
            let mut window = plain[start..end].to_vec();
            ctr_apply(&key, &base, start as u64, &mut window);
            assert_eq!(window, full[start..end], "window {start}..{end}");
        }
    }
}
