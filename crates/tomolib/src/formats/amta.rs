use crate::{Error, Result};

pub use crate::formats::binio::ByteOrder;

pub const AMTA_MAGIC: [u8; 4] = *b"AMTA";

pub const SIZE_OFFSET: usize = 0x08;
pub const NAME_PTR_OFFSET: usize = 0x24;

const HEADER_SIZE: usize = 0x34;
const INFO_HEADER_SIZE: usize = 0x1C;
const ENVELOPE_ENTRY_SIZE: usize = 8;

fn name_bounds(bytes: &[u8], byte_order: ByteOrder, off: usize) -> Result<(usize, usize)> {
    let rel = byte_order.read_u32(bytes, off + NAME_PTR_OFFSET, "AMTA name pointer")? as usize;
    let start = off + rel + NAME_PTR_OFFSET;
    if start >= bytes.len() {
        return Err(Error::malformed("AMTA name pointer out of range"));
    }
    let end = bytes[start..]
        .iter()
        .position(|&b| b == 0)
        .map(|p| start + p)
        .ok_or_else(|| Error::malformed("unterminated AMTA name"))?;
    Ok((start, end))
}

pub fn read_name(bytes: &[u8], byte_order: ByteOrder, off: usize) -> Result<String> {
    let (start, end) = name_bounds(bytes, byte_order, off)?;
    String::from_utf8(bytes[start..end].to_vec()).map_err(|_| Error::invalid_utf8("AMTA name"))
}

#[derive(Debug, Clone)]
pub struct EnvelopePoint {
    pub position: u32,
    pub value: f32,
}

#[derive(Debug, Clone)]
pub struct Amta {
    pub byte_order: ByteOrder,
    pub version: u16,
    pub channels: u32,
    pub flags: u32,
    pub section_offsets: [Option<u32>; 5],
    pub marker: u32,
    pub kind: u16,
    pub reserved: u32,
    pub params: [f32; 4],
    pub envelope: Vec<EnvelopePoint>,
    pub name: String,
    pub pre: Vec<u8>,
    pub sections: Vec<u8>,
    pub trailing: Vec<u8>,
}

const SECTION_OFFSET_SLOTS: [usize; 5] = [0x0C, 0x14, 0x18, 0x1C, 0x20];

fn read_f32(bytes: &[u8], byte_order: ByteOrder, off: usize, ctx: &'static str) -> Result<f32> {
    Ok(f32::from_bits(byte_order.read_u32(bytes, off, ctx)?))
}

impl Amta {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < HEADER_SIZE {
            return Err(Error::malformed("file too short to be an AMTA"));
        }
        if bytes[0..4] != AMTA_MAGIC {
            return Err(Error::bad_magic("AMTA"));
        }
        let byte_order = match [bytes[4], bytes[5]] {
            [0xFE, 0xFF] => ByteOrder::Big,
            [0xFF, 0xFE] => ByteOrder::Little,
            _ => return Err(Error::malformed("invalid BOM in AMTA header")),
        };
        let version = byte_order.read_u16(bytes, 6, "AMTA version")?;
        let size = byte_order.read_u32(bytes, SIZE_OFFSET, "AMTA size")? as usize;
        if size != bytes.len() {
            return Err(Error::malformed("AMTA size does not match buffer length"));
        }
        let info_off = byte_order.read_u32(bytes, 0x10, "AMTA info offset")? as usize;
        if info_off < HEADER_SIZE || info_off + INFO_HEADER_SIZE > bytes.len() {
            return Err(Error::malformed("AMTA info offset out of range"));
        }
        let channels = byte_order.read_u32(bytes, 0x2C, "AMTA channel count")?;
        let flags = byte_order.read_u32(bytes, 0x30, "AMTA flags")?;
        let mut raw_section_offsets = [0u32; 5];
        for (slot, off) in raw_section_offsets.iter_mut().zip(SECTION_OFFSET_SLOTS) {
            *slot = byte_order.read_u32(bytes, off, "AMTA section offset")?;
        }
        let pre = bytes[HEADER_SIZE..info_off].to_vec();

        let marker = byte_order.read_u32(bytes, info_off, "AMTA info marker")?;
        let mut params = [0.0f32; 4];
        for (i, p) in params.iter_mut().enumerate() {
            *p = read_f32(bytes, byte_order, info_off + 4 + 4 * i, "AMTA info param")?;
        }
        let count = byte_order.read_u16(bytes, info_off + 0x14, "AMTA envelope count")? as usize;
        let kind = byte_order.read_u16(bytes, info_off + 0x16, "AMTA info kind")?;
        let reserved = byte_order.read_u32(bytes, info_off + 0x18, "AMTA info reserved")?;

        let env_off = info_off + INFO_HEADER_SIZE;
        let info_end = env_off + count * ENVELOPE_ENTRY_SIZE;
        if info_end > bytes.len() {
            return Err(Error::malformed("AMTA envelope overflows buffer"));
        }
        let mut envelope = Vec::with_capacity(count);
        for i in 0..count {
            let o = env_off + i * ENVELOPE_ENTRY_SIZE;
            envelope.push(EnvelopePoint {
                position: byte_order.read_u32(bytes, o, "AMTA envelope position")?,
                value: read_f32(bytes, byte_order, o + 4, "AMTA envelope value")?,
            });
        }

        let (name_start, name_end) = name_bounds(bytes, byte_order, 0)?;
        if name_start < info_end {
            return Err(Error::malformed("AMTA name pointer out of range"));
        }
        let name = String::from_utf8(bytes[name_start..name_end].to_vec())
            .map_err(|_| Error::invalid_utf8("AMTA name"))?;

        let mut section_offsets = [None; 5];
        for (slot, raw) in section_offsets.iter_mut().zip(raw_section_offsets) {
            if raw == 0 {
                continue;
            }
            let target = raw as usize;
            if target < info_end || target > name_start {
                return Err(Error::malformed(
                    "AMTA section offset outside the sections region",
                ));
            }
            *slot = Some(
                u32::try_from(target - info_end)
                    .map_err(|_| Error::overflow("AMTA section offset > u32"))?,
            );
        }

        let sections = bytes[info_end..name_start].to_vec();
        let mut trailing = bytes[name_end + 1..].to_vec();
        let pad = (4 - (name_end + 1) % 4) % 4;
        if trailing.len() == pad && trailing.iter().all(|&b| b == 0) {
            trailing.clear();
        }

        Ok(Self {
            byte_order,
            version,
            channels,
            flags,
            section_offsets,
            marker,
            kind,
            reserved,
            params,
            envelope,
            name,
            pre,
            sections,
            trailing,
        })
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let bo = self.byte_order;
        let count = u16::try_from(self.envelope.len())
            .map_err(|_| Error::overflow("AMTA envelope count > u16"))?;
        let info_len = INFO_HEADER_SIZE + self.envelope.len() * ENVELOPE_ENTRY_SIZE;
        let info_off = HEADER_SIZE + self.pre.len();
        let info_end = info_off + info_len;
        let name_start = info_end + self.sections.len();
        let name_rel = name_start - NAME_PTR_OFFSET;

        let to_u32 = |n: usize, what: &'static str| -> Result<u32> {
            u32::try_from(n).map_err(|_| Error::overflow(what))
        };
        let section = |slot: Option<u32>| -> Result<u32> {
            match slot {
                None => Ok(0),
                Some(rel) => to_u32(info_end + rel as usize, "AMTA section offset > u32"),
            }
        };

        let mut out: Vec<u8> = Vec::with_capacity(name_start + self.name.len() + 8);
        out.extend_from_slice(&AMTA_MAGIC);
        out.extend_from_slice(&bo.bom());
        bo.put_u16(&mut out, self.version);
        bo.put_u32(&mut out, 0);
        bo.put_u32(&mut out, section(self.section_offsets[0])?);
        bo.put_u32(&mut out, to_u32(info_off, "AMTA info offset > u32")?);
        bo.put_u32(&mut out, section(self.section_offsets[1])?);
        bo.put_u32(&mut out, section(self.section_offsets[2])?);
        bo.put_u32(&mut out, section(self.section_offsets[3])?);
        bo.put_u32(&mut out, section(self.section_offsets[4])?);
        bo.put_u32(&mut out, to_u32(name_rel, "AMTA name offset > u32")?);
        bo.put_u32(&mut out, crate::formats::hash::crc32(self.name.as_bytes()));
        bo.put_u32(&mut out, self.channels);
        bo.put_u32(&mut out, self.flags);
        debug_assert_eq!(out.len(), HEADER_SIZE);

        out.extend_from_slice(&self.pre);
        bo.put_u32(&mut out, self.marker);
        for p in &self.params {
            bo.put_u32(&mut out, p.to_bits());
        }
        bo.put_u16(&mut out, count);
        bo.put_u16(&mut out, self.kind);
        bo.put_u32(&mut out, self.reserved);
        for point in &self.envelope {
            bo.put_u32(&mut out, point.position);
            bo.put_u32(&mut out, point.value.to_bits());
        }
        out.extend_from_slice(&self.sections);
        out.extend_from_slice(self.name.as_bytes());
        out.push(0);
        if self.trailing.is_empty() {
            while !out.len().is_multiple_of(4) {
                out.push(0);
            }
        } else {
            out.extend_from_slice(&self.trailing);
        }

        let total = to_u32(out.len(), "AMTA size > u32")?;
        bo.write_u32_at(&mut out, SIZE_OFFSET, total);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(name: &str, count: u8) -> Amta {
        Amta {
            byte_order: ByteOrder::Little,
            version: 0x0500,
            channels: 2,
            flags: 0x0400_0101,
            section_offsets: [None; 5],
            marker: 0x6f,
            kind: 2,
            reserved: 0,
            params: [0.35, 0.08, -22.8, -27.4],
            envelope: (0..count)
                .map(|i| EnvelopePoint {
                    position: u32::from(i) * 4800,
                    value: 1.0 / f32::from(i + 1),
                })
                .collect(),
            name: name.to_string(),
            pre: Vec::new(),
            sections: Vec::new(),
            trailing: Vec::new(),
        }
    }

    #[test]
    fn round_trips_through_bytes() {
        let amta = sample("SE_Test_Track", 4);
        let bytes = amta.to_bytes().unwrap();
        let parsed = Amta::parse(&bytes).unwrap();
        assert_eq!(parsed.name, "SE_Test_Track");
        assert_eq!(parsed.channels, 2);
        assert_eq!(parsed.envelope.len(), 4);
        assert_eq!(
            parsed.params.map(f32::to_bits),
            amta.params.map(f32::to_bits)
        );
        assert_eq!(parsed.to_bytes().unwrap(), bytes);
    }

    #[test]
    fn big_endian_round_trip_preserves_kind_and_count() {
        let mut amta = sample("BGM_BE", 3);
        amta.byte_order = ByteOrder::Big;
        amta.kind = 7;
        let bytes = amta.to_bytes().unwrap();
        let parsed = Amta::parse(&bytes).unwrap();
        assert_eq!(parsed.byte_order, ByteOrder::Big);
        assert_eq!(parsed.kind, 7);
        assert_eq!(parsed.envelope.len(), 3);
        assert_eq!(parsed.to_bytes().unwrap(), bytes);
    }

    #[test]
    fn name_pointer_resolves_via_read_name() {
        let bytes = sample("BGM_Foo", 2).to_bytes().unwrap();
        assert_eq!(read_name(&bytes, ByteOrder::Little, 0).unwrap(), "BGM_Foo");
    }

    #[test]
    fn rejects_bad_magic() {
        let err = Amta::parse(&[0u8; HEADER_SIZE]).unwrap_err();
        assert!(matches!(err, Error::BadMagic { .. }));
    }

    #[test]
    fn rejects_too_short() {
        let err = Amta::parse(b"AMTA").unwrap_err();
        assert!(matches!(err, Error::Malformed(_)));
    }

    #[test]
    fn preserves_sub_alignment_trailing() {
        let mut bytes = sample("SE_Test_Track", 4).to_bytes().unwrap();
        assert_eq!(bytes.len() % 4, 0);
        bytes.pop();
        let total = u32::try_from(bytes.len()).unwrap();
        ByteOrder::Little.write_u32_at(&mut bytes, SIZE_OFFSET, total);
        let parsed = Amta::parse(&bytes).unwrap();
        assert_eq!(parsed.to_bytes().unwrap(), bytes);
    }

    #[test]
    fn preserves_pre_and_trailing_regions() {
        let mut amta = sample("SE_Variant", 1);
        amta.pre = vec![0x01, 0x01, 0x00, 0x00];
        amta.sections = vec![
            1, 0, 0, 0, 0, 0, 0, 0, 0x10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        amta.trailing = b"extra\0\0\0".to_vec();
        let bytes = amta.to_bytes().unwrap();
        let parsed = Amta::parse(&bytes).unwrap();
        assert_eq!(parsed.pre, amta.pre);
        assert_eq!(parsed.sections, amta.sections);
        assert_eq!(parsed.trailing, amta.trailing);
        assert_eq!(parsed.to_bytes().unwrap(), bytes);
    }

    #[test]
    fn section_offsets_track_envelope_length_changes() {
        let mut amta = sample("BGM_Loop", 2);
        amta.sections = vec![0xAA; 0x20];
        amta.section_offsets[1] = Some(0);
        amta.section_offsets[2] = Some(0x14);

        let bytes = amta.to_bytes().unwrap();
        let parsed = Amta::parse(&bytes).unwrap();
        assert_eq!(parsed.section_offsets[1], Some(0));
        assert_eq!(parsed.section_offsets[2], Some(0x14));

        let info_end = HEADER_SIZE + INFO_HEADER_SIZE + 2 * ENVELOPE_ENTRY_SIZE;
        let mark = ByteOrder::Little.read_u32(&bytes, 0x14, "mark").unwrap() as usize;
        assert_eq!(mark, info_end);

        let mut grown = parsed;
        grown.envelope.push(EnvelopePoint {
            position: 9600,
            value: 0.25,
        });
        let grown_bytes = grown.to_bytes().unwrap();
        let new_info_end = HEADER_SIZE + INFO_HEADER_SIZE + 3 * ENVELOPE_ENTRY_SIZE;
        let new_mark = ByteOrder::Little
            .read_u32(&grown_bytes, 0x14, "mark")
            .unwrap() as usize;
        assert_eq!(new_mark, new_info_end);
        assert_eq!(&grown_bytes[new_mark..new_mark + 0x20], &[0xAA; 0x20]);
        assert_eq!(
            Amta::parse(&grown_bytes).unwrap().section_offsets[2],
            Some(0x14)
        );
    }

    #[test]
    fn rejects_section_offset_outside_sections_region() {
        let mut bytes = sample("SE_Test_Track", 2).to_bytes().unwrap();
        ByteOrder::Little.write_u32_at(&mut bytes, 0x14, 0x04);
        let err = Amta::parse(&bytes).unwrap_err();
        assert!(matches!(err, Error::Malformed(_)));
    }
}
