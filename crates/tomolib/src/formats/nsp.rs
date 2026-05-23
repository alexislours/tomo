use std::io::Read;

use crate::formats::binio::ByteOrder;
use crate::{Error, Result};

pub const PFS0_MAGIC: [u8; 4] = *b"PFS0";
pub const HFS0_MAGIC: [u8; 4] = *b"HFS0";

const HEADER_SIZE: usize = 0x10;
const PFS0_ENTRY_SIZE: usize = 0x18;
const HFS0_ENTRY_SIZE: usize = 0x40;
const MAX_HEADER_SIZE: usize = 0x40 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionKind {
    Pfs0,
    Hfs0,
}

impl PartitionKind {
    fn entry_size(self) -> usize {
        match self {
            Self::Pfs0 => PFS0_ENTRY_SIZE,
            Self::Hfs0 => HFS0_ENTRY_SIZE,
        }
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Pfs0 => "PFS0",
            Self::Hfs0 => "HFS0",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub offset: u64,
    pub size: u64,
    pub hash: Option<[u8; 32]>,
    pub hash_protected_size: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct PartitionFs {
    kind: PartitionKind,
    header_size: u64,
    entries: Vec<Entry>,
}

impl PartitionFs {
    #[must_use]
    pub fn kind(&self) -> PartitionKind {
        self.kind
    }

    #[must_use]
    pub fn header_size(&self) -> u64 {
        self.header_size
    }

    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    #[must_use]
    pub fn payload_size(&self) -> u64 {
        self.entries.iter().map(|e| e.size).sum()
    }

    pub fn read_header<R: Read>(reader: &mut R) -> Result<Self> {
        let bo = ByteOrder::Little;

        let mut buf = vec![0u8; HEADER_SIZE];
        reader.read_exact(&mut buf)?;

        let kind = match buf[0..4] {
            ref m if m == PFS0_MAGIC => PartitionKind::Pfs0,
            ref m if m == HFS0_MAGIC => PartitionKind::Hfs0,
            _ => return Err(Error::bad_magic("NSP")),
        };

        let file_num = bo.read_u32(&buf, 4, "PFS file count")? as usize;
        let name_table_size = bo.read_u32(&buf, 8, "PFS name table size")? as usize;
        let entry_size = kind.entry_size();

        let rest_len = file_num
            .checked_mul(entry_size)
            .and_then(|n| n.checked_add(name_table_size))
            .ok_or_else(|| Error::overflow("PFS header size overflow"))?;
        let full_size = HEADER_SIZE + rest_len;
        if full_size > MAX_HEADER_SIZE {
            return Err(Error::malformed(format!(
                "PFS header is implausibly large ({full_size} bytes)"
            )));
        }

        buf.resize(full_size, 0);
        reader.read_exact(&mut buf[HEADER_SIZE..])?;

        let names_off = HEADER_SIZE + file_num * entry_size;
        let name_table = &buf[names_off..];

        let mut entries = Vec::with_capacity(file_num);
        for i in 0..file_num {
            let eo = HEADER_SIZE + i * entry_size;
            let data_offset = bo.read_u64(&buf, eo, "PFS entry data offset")?;
            let size = bo.read_u64(&buf, eo + 8, "PFS entry size")?;
            let name_offset = bo.read_u32(&buf, eo + 16, "PFS entry name offset")? as usize;
            let name = read_name(name_table, name_offset)?;

            let (hash, hash_protected_size) = if kind == PartitionKind::Hfs0 {
                let protected = bo.read_u32(&buf, eo + 20, "HFS entry hash size")?;
                let mut h = [0u8; 32];
                h.copy_from_slice(&buf[eo + 0x20..eo + 0x40]);
                (Some(h), Some(u64::from(protected)))
            } else {
                (None, None)
            };

            let offset = (full_size as u64)
                .checked_add(data_offset)
                .ok_or_else(|| Error::overflow("PFS entry offset overflow"))?;

            entries.push(Entry {
                name,
                offset,
                size,
                hash,
                hash_protected_size,
            });
        }

        Ok(Self {
            kind,
            header_size: full_size as u64,
            entries,
        })
    }
}

fn read_name(table: &[u8], off: usize) -> Result<String> {
    if off >= table.len() {
        return Err(Error::out_of_range("PFS name offset", off, table.len()));
    }
    let rest = &table[off..];
    let end = rest
        .iter()
        .position(|&b| b == 0)
        .ok_or_else(|| Error::malformed("unterminated PFS entry name"))?;
    std::str::from_utf8(&rest[..end])
        .map(str::to_owned)
        .map_err(|_| Error::invalid_utf8("PFS entry name"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_pfs0(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut names = Vec::new();
        let mut name_offsets = Vec::new();
        for (name, _) in files {
            name_offsets.push(u32::try_from(names.len()).unwrap());
            names.extend_from_slice(name.as_bytes());
            names.push(0);
        }
        let mut hdr = Vec::new();
        hdr.extend_from_slice(&PFS0_MAGIC);
        hdr.extend_from_slice(&u32::try_from(files.len()).unwrap().to_le_bytes());
        hdr.extend_from_slice(&u32::try_from(names.len()).unwrap().to_le_bytes());
        hdr.extend_from_slice(&[0u8; 4]);
        let mut data_off = 0u64;
        for ((_, data), &no) in files.iter().zip(&name_offsets) {
            hdr.extend_from_slice(&data_off.to_le_bytes());
            hdr.extend_from_slice(&(data.len() as u64).to_le_bytes());
            hdr.extend_from_slice(&no.to_le_bytes());
            hdr.extend_from_slice(&[0u8; 4]);
            data_off += data.len() as u64;
        }
        hdr.extend_from_slice(&names);
        let header_len = hdr.len();
        for (_, data) in files {
            hdr.extend_from_slice(data);
        }
        assert_eq!(
            header_len,
            HEADER_SIZE + files.len() * PFS0_ENTRY_SIZE + names.len()
        );
        hdr
    }

    #[test]
    fn parses_entries_and_absolute_offsets() {
        let bytes = build_pfs0(&[("a.nca", b"hello"), ("b.cnmt.xml", b"world!!")]);
        let header_len = bytes.len() - 5 - 7;
        let mut cursor = std::io::Cursor::new(bytes.clone());
        let fs = PartitionFs::read_header(&mut cursor).unwrap();

        assert_eq!(fs.kind(), PartitionKind::Pfs0);
        assert_eq!(fs.header_size(), header_len as u64);
        assert_eq!(fs.payload_size(), 12);

        let e = fs.entries();
        assert_eq!(e[0].name, "a.nca");
        assert_eq!(e[0].offset, header_len as u64);
        assert_eq!(e[0].size, 5);
        assert_eq!(
            &bytes[usize::try_from(e[0].offset).unwrap()..][..5],
            b"hello"
        );
        assert_eq!(e[1].name, "b.cnmt.xml");
        assert_eq!(e[1].offset, header_len as u64 + 5);
        assert_eq!(
            &bytes[usize::try_from(e[1].offset).unwrap()..][..7],
            b"world!!"
        );
    }

    #[test]
    fn rejects_bad_magic() {
        let mut cursor = std::io::Cursor::new(*b"XXXX\0\0\0\0\0\0\0\0\0\0\0\0");
        assert!(matches!(
            PartitionFs::read_header(&mut cursor),
            Err(Error::BadMagic { format: "NSP" })
        ));
    }
}
