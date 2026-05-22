const HEX: [u8; 16] = *b"0123456789abcdef";

pub(crate) fn encode(bytes: &[u8]) -> String {
    let mut buf = vec![0u8; bytes.len() * 2];
    for (chunk, &b) in buf.chunks_exact_mut(2).zip(bytes) {
        chunk[0] = HEX[usize::from(b >> 4)];
        chunk[1] = HEX[usize::from(b & 0xF)];
    }
    String::from_utf8(buf).expect("hex digits are valid ascii")
}

fn nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

pub(crate) fn decode(s: &str) -> anyhow::Result<Vec<u8>> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) {
        anyhow::bail!("odd-length hex string");
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len() / 2);
    for pair in b.chunks_exact(2) {
        let hi = nibble(pair[0]).ok_or_else(|| anyhow::anyhow!("bad hex"))?;
        let lo = nibble(pair[1]).ok_or_else(|| anyhow::anyhow!("bad hex"))?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

pub(crate) fn decode_fixed<const N: usize>(s: &str) -> anyhow::Result<[u8; N]> {
    let v = decode(s)?;
    if v.len() != N {
        anyhow::bail!("expected {} hex chars, got {}", N * 2, s.len());
    }
    let mut out = [0u8; N];
    out.copy_from_slice(&v);
    Ok(out)
}
