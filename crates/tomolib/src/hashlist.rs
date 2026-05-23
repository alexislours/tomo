#[must_use]
pub(crate) fn murmur3_x86_32_seed0(bytes: &[u8]) -> u32 {
    const C1: u32 = 0xCC9E_2D51;
    const C2: u32 = 0x1B87_3593;

    let mut h: u32 = 0;
    let mut chunks = bytes.chunks_exact(4);
    for chunk in &mut chunks {
        let mut k = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        k = k.wrapping_mul(C1).rotate_left(15).wrapping_mul(C2);
        h ^= k;
        h = h.rotate_left(13).wrapping_mul(5).wrapping_add(0xE654_6B64);
    }

    let tail = chunks.remainder();
    let mut k1: u32 = 0;
    if tail.len() >= 3 {
        k1 ^= u32::from(tail[2]) << 16;
    }
    if tail.len() >= 2 {
        k1 ^= u32::from(tail[1]) << 8;
    }
    if let Some(&b) = tail.first() {
        k1 ^= u32::from(b);
        k1 = k1.wrapping_mul(C1).rotate_left(15).wrapping_mul(C2);
        h ^= k1;
    }

    let len_u32 = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
    h ^= len_u32;
    h ^= h >> 16;
    h = h.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 13;
    h = h.wrapping_mul(0xC2B2_AE35);
    h ^= h >> 16;
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        assert_eq!(murmur3_x86_32_seed0(b""), 0);
        assert_eq!(murmur3_x86_32_seed0(b"Accessory"), 0xA320_6403);
        assert_eq!(murmur3_x86_32_seed0(b"Action"), 0xF59B_6FEA);
        assert_eq!(murmur3_x86_32_seed0(b"Actor"), 0x9610_D708);
        assert_eq!(murmur3_x86_32_seed0(b"ActorKey"), 0xB12B_1C59);
    }
}
