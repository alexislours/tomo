use crate::{Error, Result};

pub mod yaml;

pub(crate) const BOM_LE: u16 = 0xFEFF;

/// The 0x20-byte header shared by the LMS family of files (MSBT, MSBP).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub(crate) reserved_a: [u8; 2],
    pub(crate) encoding: u8,
    pub version: u8,
    pub(crate) reserved_b: [u8; 2],
    pub(crate) reserved_tail: [u8; 10],
}

impl Header {
    pub(crate) fn parse(bytes: &[u8], magic: [u8; 8]) -> Result<(Self, u16)> {
        if bytes.len() < 0x20 {
            return Err(Error::malformed("LMS file too small for header"));
        }
        if bytes[..8] != magic {
            return Err(Error::bad_magic("LMS"));
        }
        let bom = u16::from_le_bytes([bytes[8], bytes[9]]);
        if bom != BOM_LE {
            return Err(Error::unsupported(
                "only little-endian LMS files are supported",
            ));
        }
        let reserved_a = [bytes[0x0A], bytes[0x0B]];
        let encoding = bytes[0x0C];
        let version = bytes[0x0D];
        let section_count = u16::from_le_bytes([bytes[0x0E], bytes[0x0F]]);
        let reserved_b = [bytes[0x10], bytes[0x11]];
        let mut reserved_tail = [0u8; 10];
        reserved_tail.copy_from_slice(&bytes[0x16..0x20]);
        Ok((
            Self {
                reserved_a,
                encoding,
                version,
                reserved_b,
                reserved_tail,
            },
            section_count,
        ))
    }

    fn write(&self, out: &mut Vec<u8>, magic: [u8; 8], section_count: u16, file_size: u32) {
        out.extend_from_slice(&magic);
        out.extend_from_slice(&BOM_LE.to_le_bytes());
        out.extend_from_slice(&self.reserved_a);
        out.push(self.encoding);
        out.push(self.version);
        out.extend_from_slice(&section_count.to_le_bytes());
        out.extend_from_slice(&self.reserved_b);
        out.extend_from_slice(&file_size.to_le_bytes());
        out.extend_from_slice(&self.reserved_tail);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Section {
    pub(crate) magic: [u8; 4],
    pub(crate) reserved: [u8; 8],
    pub(crate) padding: u8,
    pub(crate) body: Vec<u8>,
}

pub(crate) fn u32_len(n: usize) -> Result<u32> {
    u32::try_from(n).map_err(|_| Error::overflow("length exceeds u32"))
}

pub(crate) fn u16_len(n: usize) -> Result<u16> {
    u16::try_from(n).map_err(|_| Error::overflow("count exceeds u16"))
}

pub(crate) fn byte(b: &[u8], o: usize) -> u8 {
    b.get(o).copied().unwrap_or(0)
}

pub(crate) fn rd_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([byte(b, o), byte(b, o + 1)])
}

pub(crate) fn rd_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([byte(b, o), byte(b, o + 1), byte(b, o + 2), byte(b, o + 3)])
}

pub(crate) fn read_sections(bytes: &[u8], count: u16) -> Result<Vec<Section>> {
    let mut off = 0x20usize;
    let mut out = Vec::with_capacity(count as usize);
    for _ in 0..count {
        if off + 0x10 > bytes.len() {
            return Err(Error::malformed("section header out of range"));
        }
        let mut magic = [0u8; 4];
        magic.copy_from_slice(&bytes[off..off + 4]);
        let size = u32::from_le_bytes([
            bytes[off + 4],
            bytes[off + 5],
            bytes[off + 6],
            bytes[off + 7],
        ]) as usize;
        let mut reserved = [0u8; 8];
        reserved.copy_from_slice(&bytes[off + 8..off + 0x10]);
        let body_start = off + 0x10;
        let body_end = body_start + size;
        if body_end > bytes.len() {
            return Err(Error::malformed("section body out of range"));
        }
        let body = bytes[body_start..body_end].to_vec();
        let padded_end = align16(body_end);
        let padding = if padded_end > body_end {
            bytes.get(body_end).copied().unwrap_or(0xAB)
        } else {
            0xAB
        };
        out.push(Section {
            magic,
            reserved,
            padding,
            body,
        });
        off = padded_end;
    }
    Ok(out)
}

pub(crate) fn write_file(magic: [u8; 8], header: &Header, sections: &[Section]) -> Result<Vec<u8>> {
    let mut out =
        Vec::with_capacity(0x20 + sections.iter().map(|s| s.body.len() + 0x20).sum::<usize>());
    out.resize(0x20, 0);
    for s in sections {
        out.extend_from_slice(&s.magic);
        out.extend_from_slice(&u32_len(s.body.len())?.to_le_bytes());
        out.extend_from_slice(&s.reserved);
        out.extend_from_slice(&s.body);
        let target = align16(out.len());
        out.resize(target, s.padding);
    }
    let file_size = u32_len(out.len())?;
    let mut head = Vec::with_capacity(0x20);
    header.write(&mut head, magic, u16_len(sections.len())?, file_size);
    out[..0x20].copy_from_slice(&head);
    Ok(out)
}

#[inline]
#[must_use]
pub(crate) fn align16(n: usize) -> usize {
    crate::formats::binio::align_up(n, 16)
}

#[inline]
#[must_use]
pub(crate) fn align4(n: usize) -> usize {
    crate::formats::binio::align_up(n, 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> Header {
        Header {
            reserved_a: [0, 0],
            encoding: 1,
            version: 3,
            reserved_b: [0, 0],
            reserved_tail: [0; 10],
        }
    }

    #[test]
    fn label_table_round_trips() {
        let entries = vec![
            ("Alpha".to_string(), 0u32),
            ("Beta".to_string(), 1),
            ("Gamma".to_string(), 2),
            ("Delta".to_string(), 3),
        ];
        let body = write_label_table(7, &entries).unwrap();
        let table = read_label_table(&body).unwrap();
        assert_eq!(table.buckets, 7);
        let mut got = table.entries;
        got.sort_by_key(|e| e.1);
        assert_eq!(got, entries);
    }

    #[test]
    fn sections_round_trip_through_file() {
        let magic = *b"MsgStdBn";
        let sections = vec![
            Section {
                magic: *b"LBL1",
                reserved: [0; 8],
                padding: 0xAB,
                body: vec![1, 2, 3],
            },
            Section {
                magic: *b"TXT2",
                reserved: [9; 8],
                padding: 0xAB,
                body: vec![4, 5, 6, 7, 8],
            },
        ];
        let bytes = write_file(magic, &header(), &sections).unwrap();
        let (parsed_header, count) = Header::parse(&bytes, magic).unwrap();
        assert_eq!(parsed_header, header());
        let parsed = read_sections(&bytes, count).unwrap();
        assert_eq!(parsed, sections);
        let bytes2 = write_file(magic, &parsed_header, &parsed).unwrap();
        assert_eq!(bytes, bytes2);
    }
}

#[must_use]
pub(crate) fn label_hash(label: &[u8], num_buckets: u32) -> u32 {
    let mut h: u32 = 0;
    for &c in label {
        h = h.wrapping_mul(0x492).wrapping_add(u32::from(c));
    }
    h % num_buckets
}

#[derive(Debug, Clone, Default)]
pub(crate) struct LabelTable {
    pub(crate) buckets: u32,
    pub(crate) entries: Vec<(String, u32)>,
}

pub(crate) fn read_label_table(body: &[u8]) -> Result<LabelTable> {
    if body.len() < 4 {
        return Err(Error::malformed("label table too small"));
    }
    let buckets = u32::from_le_bytes([body[0], body[1], body[2], body[3]]);
    let mut entries = Vec::new();
    for i in 0..buckets as usize {
        let base = 4 + 8 * i;
        if base + 8 > body.len() {
            return Err(Error::malformed("label bucket out of range"));
        }
        let count =
            u32::from_le_bytes([body[base], body[base + 1], body[base + 2], body[base + 3]]);
        let mut p = u32::from_le_bytes([
            body[base + 4],
            body[base + 5],
            body[base + 6],
            body[base + 7],
        ]) as usize;
        for _ in 0..count {
            if p >= body.len() {
                return Err(Error::malformed("label entry out of range"));
            }
            let len = body[p] as usize;
            p += 1;
            if p + len + 4 > body.len() {
                return Err(Error::malformed("label name out of range"));
            }
            let name = String::from_utf8_lossy(&body[p..p + len]).into_owned();
            p += len;
            let value = u32::from_le_bytes([body[p], body[p + 1], body[p + 2], body[p + 3]]);
            p += 4;
            entries.push((name, value));
        }
    }
    Ok(LabelTable { buckets, entries })
}

pub(crate) fn write_label_table(buckets: u32, entries: &[(String, u32)]) -> Result<Vec<u8>> {
    let nb = buckets.max(1) as usize;
    let mut by_bucket: Vec<Vec<&(String, u32)>> = vec![Vec::new(); nb];
    for e in entries {
        let b = label_hash(e.0.as_bytes(), buckets) as usize;
        by_bucket[b].push(e);
    }
    let header_len = 4 + 8 * nb;
    let mut bucket_headers = Vec::with_capacity(nb);
    let mut data = Vec::new();
    for bucket in &by_bucket {
        let offset = header_len + data.len();
        bucket_headers.push((u32_len(bucket.len())?, u32_len(offset)?));
        for (name, value) in bucket {
            data.push(u8::try_from(name.len()).map_err(|_| Error::overflow("label too long"))?);
            data.extend_from_slice(name.as_bytes());
            data.extend_from_slice(&value.to_le_bytes());
        }
    }
    let mut out = Vec::with_capacity(header_len + data.len());
    out.extend_from_slice(&buckets.to_le_bytes());
    for (count, offset) in bucket_headers {
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
    }
    out.extend_from_slice(&data);
    Ok(out)
}
