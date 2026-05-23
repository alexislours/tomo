use crate::formats::binio::ByteOrder;
use crate::{Error, Result};

pub const TYPE_NORMAL: u8 = 0x04;
pub const TYPE_LOOP: u8 = 0x0C;
pub const TYPE_LOOP_WAIT: u8 = 0x10;

pub const VERSION: u8 = 0x03;

const LE: ByteOrder = ByteOrder::Little;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bnvib {
    pub vib_type: u8,
    pub version: u8,
    pub sample_rate: u16,
    pub loop_start: u32,
    pub loop_end: u32,
    pub loop_wait: u32,
    pub samples: Vec<u32>,
}

impl Bnvib {
    #[must_use]
    pub fn is_loop(&self) -> bool {
        matches!(self.vib_type, TYPE_LOOP | TYPE_LOOP_WAIT)
    }

    #[must_use]
    pub fn has_wait(&self) -> bool {
        self.vib_type == TYPE_LOOP_WAIT
    }

    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 6 {
            return Err(Error::malformed("file too short to be a BNVIB"));
        }
        let vib_type = bytes[0];
        let version = bytes[4];
        let sample_rate = LE.read_u16(bytes, 6, "BNVIB sample rate")?;

        let (loop_start, loop_end, loop_wait, size_off) = match vib_type {
            TYPE_NORMAL => (0, 0, 0, 8),
            TYPE_LOOP => {
                let start = LE.read_u32(bytes, 8, "BNVIB loop start")?;
                let end = LE.read_u32(bytes, 12, "BNVIB loop end")?;
                (start, end, 0, 16)
            }
            TYPE_LOOP_WAIT => {
                let start = LE.read_u32(bytes, 8, "BNVIB loop start")?;
                let end = LE.read_u32(bytes, 12, "BNVIB loop end")?;
                let wait = LE.read_u32(bytes, 16, "BNVIB loop wait")?;
                (start, end, wait, 20)
            }
            other => {
                return Err(Error::unsupported(format!(
                    "unknown BNVIB vibration type {other:#04x}"
                )));
            }
        };

        let size = LE.read_u32(bytes, size_off, "BNVIB vibration size")? as usize;
        if !size.is_multiple_of(4) {
            return Err(Error::malformed(format!(
                "BNVIB vibration size {size} is not a multiple of 4"
            )));
        }
        let data_off = size_off + 4;
        let end = data_off
            .checked_add(size)
            .filter(|&e| e <= bytes.len())
            .ok_or_else(|| {
                Error::truncated(
                    "BNVIB samples",
                    data_off,
                    size,
                    bytes.len().saturating_sub(data_off),
                )
            })?;

        let samples = bytes[data_off..end]
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();

        Ok(Self {
            vib_type,
            version,
            sample_rate,
            loop_start,
            loop_end,
            loop_wait,
            samples,
        })
    }

    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(24 + self.samples.len() * 4);
        out.extend_from_slice(&[self.vib_type, 0, 0, 0, self.version, 0]);
        LE.put_u16(&mut out, self.sample_rate);
        if self.is_loop() {
            LE.put_u32(&mut out, self.loop_start);
            LE.put_u32(&mut out, self.loop_end);
        }
        if self.has_wait() {
            LE.put_u32(&mut out, self.loop_wait);
        }
        let size = u32::try_from(self.samples.len().saturating_mul(4)).unwrap_or(u32::MAX);
        LE.put_u32(&mut out, size);
        for &s in &self.samples {
            out.extend_from_slice(&s.to_le_bytes());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_stream(n: u32) -> Vec<u32> {
        (0..n).map(|i| i.wrapping_mul(0x0101_0101)).collect()
    }

    fn round_trip(vib: &Bnvib) {
        let bytes = vib.to_bytes();
        let parsed = Bnvib::parse(&bytes).unwrap();
        assert_eq!(&parsed, vib);
        assert_eq!(parsed.to_bytes(), bytes);
    }

    #[test]
    fn normal_round_trip() {
        round_trip(&Bnvib {
            vib_type: TYPE_NORMAL,
            version: VERSION,
            sample_rate: 200,
            loop_start: 0,
            loop_end: 0,
            loop_wait: 0,
            samples: sample_stream(52),
        });
    }

    #[test]
    fn loop_round_trip() {
        round_trip(&Bnvib {
            vib_type: TYPE_LOOP,
            version: VERSION,
            sample_rate: 200,
            loop_start: 4,
            loop_end: 453,
            loop_wait: 0,
            samples: sample_stream(612),
        });
    }

    #[test]
    fn loop_wait_round_trip() {
        round_trip(&Bnvib {
            vib_type: TYPE_LOOP_WAIT,
            version: VERSION,
            sample_rate: 200,
            loop_start: 1,
            loop_end: 99,
            loop_wait: 7,
            samples: sample_stream(100),
        });
    }

    #[test]
    fn header_offsets_match_spec() {
        let vib = Bnvib {
            vib_type: TYPE_LOOP,
            version: VERSION,
            sample_rate: 200,
            loop_start: 4,
            loop_end: 453,
            loop_wait: 0,
            samples: sample_stream(1),
        };
        let b = vib.to_bytes();
        assert_eq!(b[0], TYPE_LOOP);
        assert_eq!(b[4], VERSION);
        assert_eq!(&b[6..8], &200u16.to_le_bytes());
        assert_eq!(&b[8..12], &4u32.to_le_bytes());
        assert_eq!(&b[12..16], &453u32.to_le_bytes());
        assert_eq!(&b[16..20], &4u32.to_le_bytes());
    }

    #[test]
    fn rejects_short_file() {
        assert!(matches!(
            Bnvib::parse(&[0x04, 0, 0]).unwrap_err(),
            Error::Malformed(_)
        ));
    }

    #[test]
    fn rejects_unknown_type() {
        let mut b = vec![0u8; 12];
        b[0] = 0x07;
        assert!(matches!(
            Bnvib::parse(&b).unwrap_err(),
            Error::Unsupported(_)
        ));
    }

    #[test]
    fn rejects_truncated_samples() {
        let mut b = vec![0u8; 12];
        b[0] = TYPE_NORMAL;
        b[4] = VERSION;
        b[8..12].copy_from_slice(&64u32.to_le_bytes());
        assert!(matches!(
            Bnvib::parse(&b).unwrap_err(),
            Error::Truncated { .. }
        ));
    }
}
