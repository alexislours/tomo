mod codec;
mod emit;
mod parse;
mod registry;

pub use emit::{emit_msbp, emit_msbt};
pub use parse::{parse_msbp, parse_msbt};
pub use registry::Registry;

use crate::Result;
use crate::formats::msbp::Msbp;
use crate::formats::msbt::Msbt;

/// Builds a [`Registry`] from MSBP bytes, returning `None` if they fail to
/// parse.
#[must_use]
pub fn registry_from_msbp_bytes(bytes: &[u8]) -> Option<Registry> {
    Msbp::parse(bytes).ok().map(|m| Registry::from_msbp(&m))
}

/// Parses MSBT `bytes` and renders them as YAML.
///
/// Pass a [`Registry`] (from the title's MSBP) to render control tags with
/// readable names; without one, tags fall back to a raw form.
pub fn msbt_to_yaml(bytes: &[u8], reg: Option<&Registry>) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(bytes.len() * 8);
    emit_msbt(&Msbt::parse(bytes)?, reg, &mut out)?;
    Ok(out)
}

/// Parses MSBP `bytes` and renders them as YAML.
pub fn msbp_to_yaml(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(bytes.len() * 2);
    emit_msbp(&Msbp::parse(bytes)?, &mut out)?;
    Ok(out)
}

mod shared {
    use crate::{Error, Result};

    const HEX: [u8; 16] = *b"0123456789abcdef";

    pub(super) fn hex_encode(bytes: &[u8]) -> String {
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

    pub(super) fn hex_decode(s: &str) -> Result<Vec<u8>> {
        let s = s.trim();
        if !s.len().is_multiple_of(2) {
            return Err(Error::malformed("odd-length hex string"));
        }
        let b = s.as_bytes();
        let mut out = Vec::with_capacity(b.len() / 2);
        for pair in b.chunks_exact(2) {
            let hi = nibble(pair[0]).ok_or_else(|| Error::malformed("bad hex"))?;
            let lo = nibble(pair[1]).ok_or_else(|| Error::malformed("bad hex"))?;
            out.push((hi << 4) | lo);
        }
        Ok(out)
    }

    pub(super) fn write_hex<W: std::io::Write>(w: &mut W, bytes: &[u8]) -> std::io::Result<()> {
        let mut buf = [0u8; 64];
        for group in bytes.chunks(buf.len() / 2) {
            for (chunk, &b) in buf.chunks_exact_mut(2).zip(group) {
                chunk[0] = HEX[usize::from(b >> 4)];
                chunk[1] = HEX[usize::from(b & 0xF)];
            }
            w.write_all(&buf[..group.len() * 2])?;
        }
        Ok(())
    }

    pub(super) fn needs_escape(b: u8) -> bool {
        b < 0x20 || b == b'"' || b == b'\\'
    }

    pub(super) fn write_quoted<W: std::io::Write>(w: &mut W, s: &str) -> std::io::Result<()> {
        w.write_all(b"\"")?;
        let bytes = s.as_bytes();
        let mut start = 0;
        for i in 0..bytes.len() {
            let b = bytes[i];
            if !needs_escape(b) {
                continue;
            }
            w.write_all(&bytes[start..i])?;
            match b {
                b'"' => w.write_all(b"\\\"")?,
                b'\\' => w.write_all(b"\\\\")?,
                b'\n' => w.write_all(b"\\n")?,
                b'\r' => w.write_all(b"\\r")?,
                b'\t' => w.write_all(b"\\t")?,
                _ => write!(w, "\\x{b:02x}")?,
            }
            start = i + 1;
        }
        w.write_all(&bytes[start..])?;
        w.write_all(b"\"")
    }
}
