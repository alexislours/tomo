use std::io::Write;

pub use crate::formats::binio::ByteOrder;

use crate::{Error, Result};

pub const BWAV_MAGIC: [u8; 4] = *b"BWAV";

const HEADER_SIZE: usize = 0x10;
const CHANNEL_INFO_SIZE: usize = 0x4C;
const DATA_ALIGN: usize = 0x40;

/// Codec value for signed 16-bit PCM samples.
pub const CODEC_PCM16: u16 = 0;
/// Codec value for Nintendo DSP-ADPCM samples.
pub const CODEC_DSP_ADPCM: u16 = 1;

/// Per-channel header describing one channel's codec, loop points, and the
/// location of its sample data.
#[derive(Debug, Clone)]
pub struct BwavChannel {
    pub codec: u16,
    pub channel_pan: u16,
    pub sample_rate: u32,
    pub sample_count_full: u32,
    pub sample_count: u32,
    pub coefficients: [i16; 16],
    pub data_offset_full: u32,
    pub data_offset: u32,
    pub loop_flag: u32,
    pub loop_end: u32,
    pub loop_start: u32,
    pub predictor_scale: u16,
    pub history1: i16,
    pub history2: i16,
    pub reserved: u16,
}

#[must_use]
fn sample_data_size(codec: u16, sample_count: u32) -> usize {
    let n = sample_count as usize;
    match codec {
        CODEC_PCM16 => n * 2,
        _ => n.div_ceil(14) * 8,
    }
}

/// A parsed BWAV audio file, owning its bytes and per-channel headers.
#[derive(Debug)]
pub struct Bwav {
    byte_order: ByteOrder,
    version: u16,
    hash: u32,
    prefetch: u16,
    channels: Vec<BwavChannel>,
    bytes: Vec<u8>,
}

impl Bwav {
    #[must_use]
    pub fn byte_order(&self) -> ByteOrder {
        self.byte_order
    }
    #[must_use]
    pub fn version(&self) -> u16 {
        self.version
    }
    #[must_use]
    pub fn hash(&self) -> u32 {
        self.hash
    }
    #[must_use]
    pub fn prefetch(&self) -> u16 {
        self.prefetch
    }
    #[must_use]
    pub fn channels(&self) -> &[BwavChannel] {
        &self.channels
    }

    /// Returns the raw, still-encoded sample bytes for `channel`.
    #[must_use]
    pub fn channel_data(&self, channel: &BwavChannel) -> &[u8] {
        let start = channel.data_offset as usize;
        let end = self.channel_data_end(channel.data_offset);
        &self.bytes[start..end]
    }

    fn channel_data_end(&self, offset: u32) -> usize {
        self.channels
            .iter()
            .map(|c| c.data_offset)
            .filter(|&o| o > offset)
            .min()
            .map_or(self.bytes.len(), |o| o as usize)
    }

    #[must_use]
    pub(crate) fn full_frame_len(&self) -> usize {
        self.channels
            .iter()
            .map(|c| c.data_offset as usize + sample_data_size(c.codec, c.sample_count))
            .max()
            .unwrap_or(HEADER_SIZE)
    }

    /// Parses a BWAV file, taking ownership of `bytes` so channel data can be
    /// borrowed from it later.
    pub fn parse(bytes: Vec<u8>) -> Result<Self> {
        if bytes.len() < HEADER_SIZE {
            return Err(Error::malformed("file too short to be a BWAV"));
        }
        if bytes[0..4] != BWAV_MAGIC {
            return Err(Error::bad_magic("BWAV"));
        }
        let byte_order = match [bytes[4], bytes[5]] {
            [0xFE, 0xFF] => ByteOrder::Big,
            [0xFF, 0xFE] => ByteOrder::Little,
            _ => return Err(Error::malformed("invalid BOM in BWAV header")),
        };

        let version = byte_order.read_u16(&bytes, 6, "BWAV version")?;
        let hash = byte_order.read_u32(&bytes, 8, "BWAV hash")?;
        let prefetch = byte_order.read_u16(&bytes, 12, "BWAV prefetch flag")?;
        let channel_count = byte_order.read_u16(&bytes, 14, "BWAV channel count")? as usize;

        let mut channels = Vec::with_capacity(channel_count);
        for i in 0..channel_count {
            let base = HEADER_SIZE + i * CHANNEL_INFO_SIZE;
            channels.push(parse_channel(&bytes, byte_order, base, i)?);
        }

        for (i, ch) in channels.iter().enumerate() {
            if ch.data_offset as usize > bytes.len() {
                return Err(Error::malformed(format!(
                    "channel {i} data offset {:#x} past buffer ({:#x})",
                    ch.data_offset,
                    bytes.len()
                )));
            }
        }

        Ok(Self {
            byte_order,
            version,
            hash,
            prefetch,
            channels,
            bytes,
        })
    }

    /// Decodes one channel to signed 16-bit PCM samples.
    ///
    /// Supports [`CODEC_PCM16`] and [`CODEC_DSP_ADPCM`]; other codecs return an
    /// error.
    pub fn decode_channel(&self, index: usize) -> Result<Vec<i16>> {
        let ch = self
            .channels
            .get(index)
            .ok_or_else(|| Error::out_of_range("BWAV channel", index, self.channels.len()))?;
        let data = self.channel_data(ch);
        match ch.codec {
            CODEC_PCM16 => Ok(decode_pcm16(
                data,
                self.byte_order,
                ch.sample_count as usize,
            )),
            CODEC_DSP_ADPCM => Ok(decode_dsp_adpcm(
                data,
                ch.sample_count as usize,
                &ch.coefficients,
                ch.history1,
                ch.history2,
            )),
            other => Err(Error::unsupported(format!(
                "BWAV codec {other} is not supported for decoding"
            ))),
        }
    }
}

fn parse_channel(
    bytes: &[u8],
    byte_order: ByteOrder,
    base: usize,
    index: usize,
) -> Result<BwavChannel> {
    if base + CHANNEL_INFO_SIZE > bytes.len() {
        return Err(Error::malformed(format!(
            "truncated BWAV channel info {index}"
        )));
    }
    let codec = byte_order.read_u16(bytes, base, "BWAV codec")?;
    let channel_pan = byte_order.read_u16(bytes, base + 0x02, "BWAV channel pan")?;
    let sample_rate = byte_order.read_u32(bytes, base + 0x04, "BWAV sample rate")?;
    let sample_count_full = byte_order.read_u32(bytes, base + 0x08, "BWAV sample count full")?;
    let sample_count = byte_order.read_u32(bytes, base + 0x0C, "BWAV sample count")?;

    let mut coefficients = [0i16; 16];
    for (i, c) in coefficients.iter_mut().enumerate() {
        *c = read_i16(bytes, byte_order, base + 0x10 + i * 2, "BWAV coefficient")?;
    }

    let data_offset_full = byte_order.read_u32(bytes, base + 0x30, "BWAV data offset full")?;
    let data_offset = byte_order.read_u32(bytes, base + 0x34, "BWAV data offset")?;
    let loop_flag = byte_order.read_u32(bytes, base + 0x38, "BWAV loop flag")?;
    let loop_end = byte_order.read_u32(bytes, base + 0x3C, "BWAV loop end")?;
    let loop_start = byte_order.read_u32(bytes, base + 0x40, "BWAV loop start")?;
    let predictor_scale = byte_order.read_u16(bytes, base + 0x44, "BWAV predictor scale")?;
    let history1 = read_i16(bytes, byte_order, base + 0x46, "BWAV history 1")?;
    let history2 = read_i16(bytes, byte_order, base + 0x48, "BWAV history 2")?;
    let reserved = byte_order.read_u16(bytes, base + 0x4A, "BWAV reserved")?;

    Ok(BwavChannel {
        codec,
        channel_pan,
        sample_rate,
        sample_count_full,
        sample_count,
        coefficients,
        data_offset_full,
        data_offset,
        loop_flag,
        loop_end,
        loop_start,
        predictor_scale,
        history1,
        history2,
        reserved,
    })
}

fn read_i16(bytes: &[u8], byte_order: ByteOrder, off: usize, ctx: &'static str) -> Result<i16> {
    Ok(byte_order.read_u16(bytes, off, ctx)?.cast_signed())
}

fn decode_pcm16(data: &[u8], byte_order: ByteOrder, sample_count: usize) -> Vec<i16> {
    data.chunks_exact(2)
        .take(sample_count)
        .map(|c| {
            let a = [c[0], c[1]];
            match byte_order {
                ByteOrder::Little => i16::from_le_bytes(a),
                ByteOrder::Big => i16::from_be_bytes(a),
            }
        })
        .collect()
}

fn decode_dsp_adpcm(
    data: &[u8],
    sample_count: usize,
    coefficients: &[i16; 16],
    history1: i16,
    history2: i16,
) -> Vec<i16> {
    let mut out = Vec::with_capacity(sample_count);
    let mut hist1 = i32::from(history1);
    let mut hist2 = i32::from(history2);

    for frame in data.chunks(8) {
        if out.len() >= sample_count || frame.is_empty() {
            break;
        }
        let header = frame[0];
        let scale = 1i32 << (header & 0x0F);
        let coef_index = ((header >> 4) & 0x07) as usize;
        let c1 = i32::from(coefficients[coef_index * 2]);
        let c2 = i32::from(coefficients[coef_index * 2 + 1]);

        for &byte in &frame[1..] {
            for nibble in [byte >> 4, byte & 0x0F] {
                if out.len() >= sample_count {
                    break;
                }
                let delta = sign_extend4(nibble);
                let predicted = ((i64::from(delta) * i64::from(scale)) << 11)
                    + 0x400
                    + i64::from(c1) * i64::from(hist1)
                    + i64::from(c2) * i64::from(hist2);
                let sample =
                    i16::try_from((predicted >> 11).clamp(-32768, 32767)).unwrap_or_default();
                hist2 = hist1;
                hist1 = i32::from(sample);
                out.push(sample);
            }
        }
    }
    out
}

fn sign_extend4(nibble: u8) -> i32 {
    let n = i32::from(nibble & 0x0F);
    if n >= 8 { n - 16 } else { n }
}

/// One channel to be written by [`write()`]: its header plus its encoded sample
/// bytes.
#[derive(Debug)]
pub struct PackChannel<'a> {
    pub info: BwavChannel,
    pub data: &'a [u8],
}

/// Writes a BWAV file from the given channels, returning the number of bytes
/// written. Data offsets in each [`PackChannel::info`] are recomputed.
pub fn write<W: Write>(
    writer: &mut W,
    byte_order: ByteOrder,
    version: u16,
    hash: u32,
    prefetch: u16,
    channels: &[PackChannel<'_>],
) -> Result<u64> {
    if channels.is_empty() {
        return Err(Error::malformed("BWAV needs at least one channel"));
    }

    let info_end = HEADER_SIZE + channels.len() * CHANNEL_INFO_SIZE;
    let mut cursor = info_end.next_multiple_of(DATA_ALIGN);

    let mut offsets = Vec::with_capacity(channels.len());
    let mut last_end = info_end;
    for ch in channels {
        let off = u32::try_from(cursor)
            .map_err(|_| Error::overflow("BWAV channel data offset exceeds u32"))?;
        offsets.push(off);
        last_end = cursor + ch.data.len();
        cursor = last_end.next_multiple_of(DATA_ALIGN);
    }

    let mut out = vec![0u8; last_end];
    out[0..4].copy_from_slice(&BWAV_MAGIC);
    out[4..6].copy_from_slice(&byte_order.bom());
    byte_order.write_u16_at(&mut out, 6, version);
    byte_order.write_u32_at(&mut out, 8, hash);
    byte_order.write_u16_at(&mut out, 12, prefetch);
    let channel_count =
        u16::try_from(channels.len()).map_err(|_| Error::overflow("BWAV channel count > u16"))?;
    byte_order.write_u16_at(&mut out, 14, channel_count);

    for (i, ch) in channels.iter().enumerate() {
        let base = HEADER_SIZE + i * CHANNEL_INFO_SIZE;
        let info = &ch.info;
        let off = offsets[i];
        byte_order.write_u16_at(&mut out, base, info.codec);
        byte_order.write_u16_at(&mut out, base + 0x02, info.channel_pan);
        byte_order.write_u32_at(&mut out, base + 0x04, info.sample_rate);
        byte_order.write_u32_at(&mut out, base + 0x08, info.sample_count_full);
        byte_order.write_u32_at(&mut out, base + 0x0C, info.sample_count);
        for (j, c) in info.coefficients.iter().enumerate() {
            byte_order.write_u16_at(&mut out, base + 0x10 + j * 2, c.cast_unsigned());
        }
        let off_full = if info.data_offset_full == info.data_offset {
            off
        } else {
            info.data_offset_full
        };
        byte_order.write_u32_at(&mut out, base + 0x30, off_full);
        byte_order.write_u32_at(&mut out, base + 0x34, off);
        byte_order.write_u32_at(&mut out, base + 0x38, info.loop_flag);
        byte_order.write_u32_at(&mut out, base + 0x3C, info.loop_end);
        byte_order.write_u32_at(&mut out, base + 0x40, info.loop_start);
        byte_order.write_u16_at(&mut out, base + 0x44, info.predictor_scale);
        byte_order.write_u16_at(&mut out, base + 0x46, info.history1.cast_unsigned());
        byte_order.write_u16_at(&mut out, base + 0x48, info.history2.cast_unsigned());
        byte_order.write_u16_at(&mut out, base + 0x4A, info.reserved);

        let start = off as usize;
        out[start..start + ch.data.len()].copy_from_slice(ch.data);
    }

    writer.write_all(&out)?;
    Ok(out.len() as u64)
}

/// Builds a standard RIFF/WAVE file from decoded PCM channels, each given as
/// `(sample_rate, samples)`.
#[must_use]
pub fn build_wav(channels: &[(u32, Vec<i16>)]) -> Vec<u8> {
    let channel_count = u16::try_from(channels.len()).unwrap_or(u16::MAX);
    let sample_rate = channels.first().map_or(48000, |c| c.0);
    let frames = channels.iter().map(|c| c.1.len()).max().unwrap_or(0);
    let bits_per_sample = 16u16;
    let block_align = channel_count * (bits_per_sample / 8);
    let byte_rate = sample_rate * u32::from(block_align);
    let data_len = u32::try_from(frames * channels.len() * 2).unwrap_or(u32::MAX);

    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channel_count.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits_per_sample.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for frame in 0..frames {
        for (_, samples) in channels {
            let s = samples.get(frame).copied().unwrap_or(0);
            out.extend_from_slice(&s.to_le_bytes());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm16_channel(samples: &[i16]) -> (Bwav, Vec<u8>) {
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let ch = BwavChannel {
            codec: CODEC_PCM16,
            channel_pan: 2,
            sample_rate: 48000,
            sample_count_full: u32::try_from(samples.len()).unwrap(),
            sample_count: u32::try_from(samples.len()).unwrap(),
            coefficients: [0; 16],
            data_offset_full: 0,
            data_offset: 0,
            loop_flag: 0,
            loop_end: 0xFFFF_FFFF,
            loop_start: 0,
            predictor_scale: 0,
            history1: 0,
            history2: 0,
            reserved: 0,
        };
        let mut buf = Vec::new();
        write(
            &mut buf,
            ByteOrder::Little,
            1,
            0,
            0,
            &[PackChannel {
                info: ch,
                data: &data,
            }],
        )
        .unwrap();
        (Bwav::parse(buf.clone()).unwrap(), buf)
    }

    #[test]
    fn pcm16_round_trip_and_decode() {
        let samples = [0i16, 1, -1, 100, -100, 32767, -32768, 42];
        let (bwav, _) = pcm16_channel(&samples);
        assert_eq!(bwav.channels().len(), 1);
        let decoded = bwav.decode_channel(0).unwrap();
        assert_eq!(decoded, samples);
    }

    #[test]
    fn pcm16_container_round_trip() {
        let samples: Vec<i16> = (0..200i16)
            .map(|i| i.wrapping_mul(13).wrapping_sub(1000))
            .collect();
        let (bwav, original) = pcm16_channel(&samples);
        let mut rebuilt = Vec::new();
        let ch = &bwav.channels()[0];
        write(
            &mut rebuilt,
            bwav.byte_order(),
            bwav.version(),
            bwav.hash(),
            bwav.prefetch(),
            &[PackChannel {
                info: ch.clone(),
                data: bwav.channel_data(ch),
            }],
        )
        .unwrap();
        assert_eq!(rebuilt, original);
    }

    #[test]
    fn data_size_matches_codec() {
        assert_eq!(sample_data_size(CODEC_PCM16, 100), 200);
        assert_eq!(sample_data_size(CODEC_DSP_ADPCM, 14), 8);
        assert_eq!(sample_data_size(CODEC_DSP_ADPCM, 15), 16);
        assert_eq!(sample_data_size(CODEC_DSP_ADPCM, 96000), 54864);
    }

    #[test]
    fn dsp_adpcm_decodes_full_sample_count() {
        let info = BwavChannel {
            codec: CODEC_DSP_ADPCM,
            channel_pan: 0,
            sample_rate: 48000,
            sample_count_full: 28,
            sample_count: 28,
            coefficients: [
                0x0800, 0, 0x0c00, -0x0400, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            ],
            data_offset_full: 0,
            data_offset: 0,
            loop_flag: 0,
            loop_end: 0xFFFF_FFFF,
            loop_start: 0,
            predictor_scale: 0,
            history1: 0,
            history2: 0,
            reserved: 0,
        };
        let data: Vec<u8> = (0..16u8).map(|b| b.wrapping_mul(17)).collect();
        let mut buf = Vec::new();
        write(
            &mut buf,
            ByteOrder::Little,
            1,
            0,
            0,
            &[PackChannel { info, data: &data }],
        )
        .unwrap();
        let bwav = Bwav::parse(buf).unwrap();
        let decoded = bwav.decode_channel(0).unwrap();
        assert_eq!(decoded.len(), 28);
    }

    #[test]
    fn rejects_bad_magic() {
        let err = Bwav::parse(vec![0u8; 0x10]).unwrap_err();
        assert!(matches!(err, Error::BadMagic { .. }));
    }
}
