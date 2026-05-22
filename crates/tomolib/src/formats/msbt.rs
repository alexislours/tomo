use std::collections::BTreeMap;

use crate::formats::lms::{self, Header, Section, rd_u32, u32_len};
use crate::{Error, Result};

pub const MSBT_MAGIC: [u8; 8] = *b"MsgStdBn";

/// Per-section bookkeeping (reserved bytes and padding) preserved so a file can
/// be rewritten byte-for-byte.
#[derive(Debug, Clone)]
pub struct SecMeta {
    pub(crate) reserved: [u8; 8],
    pub(crate) padding: u8,
}

impl Default for SecMeta {
    fn default() -> Self {
        Self {
            reserved: [0; 8],
            padding: 0xAB,
        }
    }
}

/// A single message: its label, style index, attribute bytes, and encoded text.
#[derive(Debug, Clone)]
pub struct Message {
    pub(crate) label: String,
    pub(crate) style: u32,
    pub(crate) attr: Vec<u8>,
    pub(crate) text: Vec<u8>,
}

/// A parsed MSBT message table.
///
/// Most users go through [`yaml`](crate::formats::lms::yaml) to convert to and
/// from a readable text form rather than touching the section fields directly.
#[derive(Debug, Clone)]
pub struct Msbt {
    pub header: Header,
    pub order: Vec<[u8; 4]>,
    pub meta: BTreeMap<[u8; 4], SecMeta>,
    pub lbl1_buckets: u32,
    pub ato1: Vec<u8>,
    pub attr_size: u32,
    pub atr1_pool: Vec<u8>,
    pub messages: Vec<Message>,
    pub raw: BTreeMap<[u8; 4], Vec<u8>>,
}

impl Msbt {
    /// Parses an MSBT file.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let (header, count) = Header::parse(bytes, MSBT_MAGIC)?;
        let sections = lms::read_sections(bytes, count)?;

        let mut order = Vec::with_capacity(sections.len());
        let mut meta = BTreeMap::new();
        let mut raw = BTreeMap::new();
        let mut by: BTreeMap<[u8; 4], &Section> = BTreeMap::new();
        for s in &sections {
            order.push(s.magic);
            meta.insert(
                s.magic,
                SecMeta {
                    reserved: s.reserved,
                    padding: s.padding,
                },
            );
            by.insert(s.magic, s);
        }

        let txt = by
            .get(b"TXT2")
            .ok_or_else(|| Error::malformed("MSBT missing TXT2"))?;
        let mut texts = parse_txt2(&txt.body)?;
        let n = texts.len();

        let (mut labels, lbl1_buckets) = match by.get(b"LBL1") {
            Some(s) => parse_lbl1(&s.body, n)?,
            None => (vec![String::new(); n], 0),
        };

        let (mut records, attr_size, atr1_pool) = match by.get(b"ATR1") {
            Some(s) => parse_atr1(&s.body, n)?,
            None => (vec![Vec::new(); n], 0, Vec::new()),
        };

        let styles = match by.get(b"TSY1") {
            Some(s) => parse_tsy1(&s.body, n)?,
            None => vec![0u32; n],
        };

        let ato1 = by.get(b"ATO1").map(|s| s.body.clone()).unwrap_or_default();

        for s in &sections {
            if !matches!(&s.magic, b"LBL1" | b"ATO1" | b"ATR1" | b"TSY1" | b"TXT2") {
                raw.insert(s.magic, s.body.clone());
            }
        }

        let messages = (0..n)
            .map(|i| Message {
                label: std::mem::take(&mut labels[i]),
                style: styles[i],
                attr: std::mem::take(&mut records[i]),
                text: std::mem::take(&mut texts[i]),
            })
            .collect();

        Ok(Self {
            header,
            order,
            meta,
            lbl1_buckets,
            ato1,
            attr_size,
            atr1_pool,
            messages,
            raw,
        })
    }

    /// Serializes the message table back to the binary MSBT format.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut sections = Vec::with_capacity(self.order.len());
        for magic in &self.order {
            let m = self.meta.get(magic).cloned().unwrap_or_default();
            let body = self.build_section(*magic)?;
            sections.push(Section {
                magic: *magic,
                reserved: m.reserved,
                padding: m.padding,
                body,
            });
        }
        lms::write_file(MSBT_MAGIC, &self.header, &sections)
    }

    fn build_section(&self, magic: [u8; 4]) -> Result<Vec<u8>> {
        Ok(match &magic {
            b"LBL1" => {
                let mut entries: Vec<(String, u32)> = Vec::with_capacity(self.messages.len());
                for (i, m) in self.messages.iter().enumerate() {
                    entries.push((m.label.clone(), u32_len(i)?));
                }
                entries.sort_by_key(|e| e.1);
                lms::write_label_table(self.lbl1_buckets, &entries)?
            }
            b"ATO1" => self.ato1.clone(),
            b"ATR1" => {
                let na = self.messages.len();
                let mut b =
                    Vec::with_capacity(8 + na * self.attr_size as usize + self.atr1_pool.len());
                b.extend_from_slice(&u32_len(na)?.to_le_bytes());
                b.extend_from_slice(&self.attr_size.to_le_bytes());
                for m in &self.messages {
                    b.extend_from_slice(&m.attr);
                }
                b.extend_from_slice(&self.atr1_pool);
                b
            }
            b"TSY1" => {
                let mut b = Vec::with_capacity(4 * self.messages.len());
                for m in &self.messages {
                    b.extend_from_slice(&m.style.to_le_bytes());
                }
                b
            }
            b"TXT2" => {
                let n = self.messages.len();
                let header = 4 + 4 * n;
                let mut data = Vec::new();
                let mut offsets = Vec::with_capacity(n);
                for m in &self.messages {
                    offsets.push(u32_len(header + data.len())?);
                    data.extend_from_slice(&m.text);
                }
                let mut b = Vec::with_capacity(header + data.len());
                b.extend_from_slice(&u32_len(n)?.to_le_bytes());
                for o in offsets {
                    b.extend_from_slice(&o.to_le_bytes());
                }
                b.extend_from_slice(&data);
                b
            }
            other => self
                .raw
                .get(other)
                .cloned()
                .ok_or_else(|| Error::unsupported("unknown MSBT section on rebuild"))?,
        })
    }

    #[must_use]
    pub(crate) fn ato1_ints(&self) -> Vec<Option<u32>> {
        self.ato1
            .chunks_exact(4)
            .map(|c| {
                let v = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                if v == 0xFFFF_FFFF { None } else { Some(v) }
            })
            .collect()
    }
}

fn parse_txt2(body: &[u8]) -> Result<Vec<Vec<u8>>> {
    let n = rd_u32(body, 0) as usize;
    if 4 + n.saturating_mul(4) > body.len() {
        return Err(Error::out_of_range("MSBT TXT2 count", n, body.len()));
    }
    let mut offs = Vec::with_capacity(n + 1);
    for i in 0..n {
        offs.push(rd_u32(body, 4 + i * 4) as usize);
    }
    offs.push(body.len());
    let mut texts = Vec::with_capacity(n);
    for i in 0..n {
        if offs[i] > offs[i + 1] || offs[i + 1] > body.len() {
            return Err(Error::out_of_range(
                "MSBT TXT2 entry",
                offs[i + 1],
                body.len(),
            ));
        }
        texts.push(body[offs[i]..offs[i + 1]].to_vec());
    }
    Ok(texts)
}

fn parse_lbl1(body: &[u8], n: usize) -> Result<(Vec<String>, u32)> {
    let table = lms::read_label_table(body)?;
    let mut labels = vec![String::new(); n];
    for (name, idx) in table.entries {
        let i = idx as usize;
        if i >= labels.len() {
            return Err(Error::out_of_range(
                "MSBT LBL1 label index",
                i,
                labels.len(),
            ));
        }
        labels[i] = name;
    }
    Ok((labels, table.buckets))
}

fn parse_atr1(body: &[u8], n: usize) -> Result<(Vec<Vec<u8>>, u32, Vec<u8>)> {
    let na = rd_u32(body, 0) as usize;
    let attr_size = rd_u32(body, 4);
    let recs = 8 + na * attr_size as usize;
    let mut records: Vec<Vec<u8>> = vec![Vec::new(); n];
    for (i, rec) in records.iter_mut().take(na.min(n)).enumerate() {
        let start = 8 + i * attr_size as usize;
        let end = start + attr_size as usize;
        if end > body.len() {
            return Err(Error::truncated(
                "MSBT ATR1 record",
                start,
                attr_size as usize,
                body.len().saturating_sub(start),
            ));
        }
        *rec = body[start..end].to_vec();
    }
    let pool = if recs <= body.len() {
        body[recs..].to_vec()
    } else {
        Vec::new()
    };
    Ok((records, attr_size, pool))
}

fn parse_tsy1(body: &[u8], n: usize) -> Result<Vec<u32>> {
    let mut styles = vec![0u32; n];
    for (i, style) in styles.iter_mut().enumerate() {
        if 4 * i + 4 > body.len() {
            return Err(Error::truncated(
                "MSBT TSY1 style",
                4 * i,
                4,
                body.len().saturating_sub(4 * i),
            ));
        }
        *style = rd_u32(body, 4 * i);
    }
    Ok(styles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::lms::yaml::{emit_msbt, parse_msbt};

    fn sample() -> Msbt {
        let header = Header {
            reserved_a: [0, 0],
            encoding: 1,
            version: 3,
            reserved_b: [0, 0],
            reserved_tail: [0; 10],
        };
        let messages = vec![
            Message {
                label: "Greeting".into(),
                style: 0,
                attr: vec![0x01, 0x00, 0x00, 0x00],
                text: vec![0x48, 0x00, 0x69, 0x00, 0x00, 0x00],
            },
            Message {
                label: "Farewell".into(),
                style: 5,
                attr: vec![0x02, 0x00, 0x00, 0x00],
                text: vec![0x42, 0x00, 0x79, 0x00, 0x65, 0x00, 0x00, 0x00],
            },
        ];
        let order = vec![*b"LBL1", *b"ATR1", *b"TSY1", *b"TXT2"];
        let mut meta = BTreeMap::new();
        for m in &order {
            meta.insert(*m, SecMeta::default());
        }
        Msbt {
            header,
            order,
            meta,
            lbl1_buckets: 7,
            ato1: Vec::new(),
            attr_size: 4,
            atr1_pool: Vec::new(),
            messages,
            raw: BTreeMap::new(),
        }
    }

    #[test]
    fn struct_byte_round_trip() {
        let b1 = sample().to_bytes().unwrap();
        let parsed = Msbt::parse(&b1).unwrap();
        let b2 = parsed.to_bytes().unwrap();
        assert_eq!(b1, b2);
        assert_eq!(parsed.messages.len(), 2);
        assert_eq!(parsed.messages[1].label, "Farewell");
        assert_eq!(parsed.messages[1].style, 5);
        assert_eq!(parsed.messages[0].attr, vec![0x01, 0x00, 0x00, 0x00]);
        assert_eq!(
            parsed.messages[0].text,
            vec![0x48, 0x00, 0x69, 0x00, 0x00, 0x00]
        );
    }

    #[test]
    fn yaml_round_trip_without_registry() {
        let b1 = sample().to_bytes().unwrap();
        let parsed = Msbt::parse(&b1).unwrap();
        let mut yaml = Vec::new();
        emit_msbt(&parsed, None, &mut yaml).unwrap();
        let text = std::str::from_utf8(&yaml).unwrap();
        let from_yaml = parse_msbt(text, None).unwrap();
        assert_eq!(from_yaml.to_bytes().unwrap(), b1);
    }
}
