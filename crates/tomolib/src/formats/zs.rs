use std::io::{Read, Write};

use crate::{Error, Result};

pub const ZSTD_MAGIC: u32 = 0xFD2F_B528;
/// Compression level used by tooling when none is specified.
pub const DEFAULT_LEVEL: i32 = 9;
const DEFAULT_WINDOW_LOG: u32 = 21;

const MAX_FRAME_HEADER: usize = 18;

/// Sizes read from a zstd frame header without decompressing the payload.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ZsInfo {
    pub compressed_size: u64,
    /// Decompressed size when the frame records it, otherwise `None`.
    pub decompressed_size: Option<u64>,
}

/// Reads the frame header of a zstd stream to report its sizes.
///
/// `compressed_size` is passed through to [`ZsInfo::compressed_size`]; only the
/// first few bytes of `reader` are consumed.
pub fn info(reader: impl Read, compressed_size: u64) -> Result<ZsInfo> {
    let mut buf = Vec::with_capacity(MAX_FRAME_HEADER);
    reader.take(MAX_FRAME_HEADER as u64).read_to_end(&mut buf)?;

    if buf.len() < 5 {
        return Err(Error::malformed("file too short to be a zstd frame"));
    }
    let magic = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if magic != ZSTD_MAGIC {
        return Err(Error::bad_magic("zstd"));
    }

    Ok(ZsInfo {
        compressed_size,
        decompressed_size: zstd::zstd_safe::get_frame_content_size(&buf).ok().flatten(),
    })
}

/// Decompresses a zstd stream from `reader` into `writer`, returning the number
/// of bytes written.
pub fn decompress(reader: impl Read, mut writer: impl Write) -> Result<u64> {
    let mut decoder = zstd::stream::Decoder::new(reader)?;
    Ok(std::io::copy(&mut decoder, &mut writer)?)
}

/// Compresses `reader` into `writer` at the given `level`, returning the number
/// of input bytes read.
///
/// Pass `pledged_src_size` when the input length is known so the decompressed
/// size is recorded in the frame header.
pub fn compress(
    mut reader: impl Read,
    writer: impl Write,
    level: i32,
    pledged_src_size: Option<u64>,
) -> Result<u64> {
    let mut encoder = zstd::stream::Encoder::new(writer, level)?;
    encoder.set_parameter(zstd::stream::raw::CParameter::WindowLog(DEFAULT_WINDOW_LOG))?;
    if let Some(size) = pledged_src_size {
        encoder.set_pledged_src_size(Some(size))?;
        encoder.include_contentsize(true)?;
    }
    let n = std::io::copy(&mut reader, &mut encoder)?;
    encoder.finish()?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn compress_to_vec(input: &[u8], level: i32) -> Vec<u8> {
        let mut out = Vec::new();
        compress(Cursor::new(input), &mut out, level, None).unwrap();
        out
    }

    #[test]
    fn round_trip_compress_decompress() {
        let payload: Vec<u8> = (0..4096u32).flat_map(u32::to_le_bytes).collect();
        let compressed = compress_to_vec(&payload, 3);

        let mut decompressed = Vec::new();
        let n = decompress(Cursor::new(&compressed), &mut decompressed).unwrap();
        assert_eq!(n, payload.len() as u64);
        assert_eq!(decompressed, payload);
    }

    #[test]
    fn round_trip_empty_payload() {
        let compressed = compress_to_vec(&[], 3);
        let mut out = Vec::new();
        decompress(Cursor::new(&compressed), &mut out).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn info_reports_compressed_size() {
        let payload = b"the quick brown fox jumps over the lazy dog".repeat(64);
        let compressed = compress_to_vec(&payload, 3);
        let compressed_len = compressed.len() as u64;
        let info = info(Cursor::new(&compressed), compressed_len).unwrap();
        assert_eq!(info.compressed_size, compressed_len);
    }

    #[test]
    fn info_reads_decompressed_size_when_frame_records_it() {
        let payload = b"the quick brown fox jumps over the lazy dog".repeat(64);
        let compressed = zstd::bulk::compress(&payload, 3).unwrap();
        let compressed_len = compressed.len() as u64;
        let info = info(Cursor::new(&compressed), compressed_len).unwrap();
        assert_eq!(info.compressed_size, compressed_len);
        assert_eq!(info.decompressed_size, Some(payload.len() as u64));
    }

    #[test]
    fn info_rejects_too_short() {
        let err = info(Cursor::new([0u8; 3].as_slice()), 3).unwrap_err();
        let crate::Error::Malformed(msg) = err else {
            panic!("expected Malformed error variant, got {err:?}");
        };
        assert!(msg.contains("too short"));
    }

    #[test]
    fn info_rejects_bad_magic() {
        let bytes = [0u8; 16];
        let err = info(Cursor::new(bytes.as_slice()), bytes.len() as u64).unwrap_err();
        assert!(
            matches!(err, crate::Error::BadMagic { format: "zstd" }),
            "got {err:?}"
        );
    }

    #[test]
    fn info_magic_constant_is_little_endian_28b52ffd() {
        assert_eq!(u32::from_le_bytes([0x28, 0xB5, 0x2F, 0xFD]), ZSTD_MAGIC);
    }

    #[test]
    fn decompress_rejects_garbage() {
        let garbage = vec![0u8; 64];
        let mut out = Vec::new();
        let err = decompress(Cursor::new(&garbage), &mut out).unwrap_err();
        assert!(matches!(err, crate::Error::Io(_)));
    }
}
