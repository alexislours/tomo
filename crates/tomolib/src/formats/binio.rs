use crate::{Error, Result};

/// Byte order of a binary file, reported by parsers and accepted by writers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ByteOrder {
    Little,
    Big,
}

impl ByteOrder {
    pub(crate) fn read_u16(self, bytes: &[u8], off: usize, ctx: &'static str) -> Result<u16> {
        let a = read_array::<2>(bytes, off, ctx)?;
        Ok(match self {
            Self::Little => u16::from_le_bytes(a),
            Self::Big => u16::from_be_bytes(a),
        })
    }

    /// Reads a `u32` at `off`; `ctx` labels the location in any error.
    pub fn read_u32(self, bytes: &[u8], off: usize, ctx: &'static str) -> Result<u32> {
        let a = read_array::<4>(bytes, off, ctx)?;
        Ok(match self {
            Self::Little => u32::from_le_bytes(a),
            Self::Big => u32::from_be_bytes(a),
        })
    }

    /// Reads a `u64` at `off`; `ctx` labels the location in any error.
    pub fn read_u64(self, bytes: &[u8], off: usize, ctx: &'static str) -> Result<u64> {
        let a = read_array::<8>(bytes, off, ctx)?;
        Ok(match self {
            Self::Little => u64::from_le_bytes(a),
            Self::Big => u64::from_be_bytes(a),
        })
    }

    /// Reads a 24-bit integer at `off` into a `u32`; `ctx` labels any error.
    pub fn read_u24(self, bytes: &[u8], off: usize, ctx: &'static str) -> Result<u32> {
        let b = read_array::<3>(bytes, off, ctx)?;
        Ok(match self {
            Self::Little => u32::from(b[0]) | (u32::from(b[1]) << 8) | (u32::from(b[2]) << 16),
            Self::Big => (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]),
        })
    }

    pub(crate) fn put_u16(self, out: &mut Vec<u8>, n: u16) {
        let a = match self {
            Self::Little => n.to_le_bytes(),
            Self::Big => n.to_be_bytes(),
        };
        out.extend_from_slice(&a);
    }

    pub(crate) fn put_u32(self, out: &mut Vec<u8>, n: u32) {
        let a = match self {
            Self::Little => n.to_le_bytes(),
            Self::Big => n.to_be_bytes(),
        };
        out.extend_from_slice(&a);
    }

    pub(crate) fn put_u64(self, out: &mut Vec<u8>, n: u64) {
        let a = match self {
            Self::Little => n.to_le_bytes(),
            Self::Big => n.to_be_bytes(),
        };
        out.extend_from_slice(&a);
    }

    pub(crate) fn put_u24(self, out: &mut Vec<u8>, n: u32) {
        let b = n.to_le_bytes();
        match self {
            Self::Little => out.extend_from_slice(&[b[0], b[1], b[2]]),
            Self::Big => out.extend_from_slice(&[b[2], b[1], b[0]]),
        }
    }

    pub(crate) fn write_u16_at(self, out: &mut [u8], offset: usize, n: u16) {
        let a = match self {
            Self::Little => n.to_le_bytes(),
            Self::Big => n.to_be_bytes(),
        };
        out[offset..offset + 2].copy_from_slice(&a);
    }

    pub(crate) fn write_u32_at(self, out: &mut [u8], offset: usize, n: u32) {
        let a = match self {
            Self::Little => n.to_le_bytes(),
            Self::Big => n.to_be_bytes(),
        };
        out[offset..offset + 4].copy_from_slice(&a);
    }

    pub(crate) fn write_u64_at(self, out: &mut [u8], offset: usize, n: u64) {
        let a = match self {
            Self::Little => n.to_le_bytes(),
            Self::Big => n.to_be_bytes(),
        };
        out[offset..offset + 8].copy_from_slice(&a);
    }

    #[must_use]
    pub(crate) fn bom(self) -> [u8; 2] {
        match self {
            Self::Big => [0xFE, 0xFF],
            Self::Little => [0xFF, 0xFE],
        }
    }
}

pub(crate) fn read_array<const N: usize>(
    bytes: &[u8],
    off: usize,
    ctx: &'static str,
) -> Result<[u8; N]> {
    let end = off
        .checked_add(N)
        .filter(|&e| e <= bytes.len())
        .ok_or_else(|| Error::truncated(ctx, off, N, bytes.len().saturating_sub(off)))?;
    let mut a = [0u8; N];
    a.copy_from_slice(&bytes[off..end]);
    Ok(a)
}

/// Rounds `n` up to the next multiple of `a`, which must be a power of two.
#[inline]
#[must_use]
pub fn align_up(n: usize, a: usize) -> usize {
    (n + a - 1) & !(a - 1)
}
