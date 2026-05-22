use std::io::Write;

use crate::{Error, Result};

pub const RSTBL_MAGIC: [u8; 6] = *b"RESTBL";
pub const DEFAULT_PATH_SIZE: u32 = 0x100;

const HEADER_SIZE: usize = 0x16;
const CRC_ENTRY_SIZE: usize = 8;

/// A resource-size entry keyed by the CRC32 of a resource path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrcEntry {
    pub hash: u32,
    pub size: u32,
}

/// A resource-size entry keyed by the literal resource path, used for paths
/// that would collide as CRCs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathEntry {
    pub name: String,
    pub size: u32,
}

/// A parsed RSTB (resource size table), mapping resource paths to the buffer
/// size the game must allocate for them.
#[derive(Debug, Clone)]
pub struct Rstbl {
    version: u32,
    path_size: u32,
    crc: Vec<CrcEntry>,
    paths: Vec<PathEntry>,
}

impl Rstbl {
    /// Creates an empty table. `path_size` is the fixed width of name entries
    /// in the path section; [`DEFAULT_PATH_SIZE`] is the usual value.
    #[must_use]
    pub fn new(version: u32, path_size: u32) -> Self {
        Self {
            version,
            path_size,
            crc: Vec::new(),
            paths: Vec::new(),
        }
    }

    #[must_use]
    pub fn version(&self) -> u32 {
        self.version
    }

    #[must_use]
    pub fn path_size(&self) -> u32 {
        self.path_size
    }

    #[must_use]
    pub fn crc_entries(&self) -> &[CrcEntry] {
        &self.crc
    }

    #[must_use]
    pub fn path_entries(&self) -> &[PathEntry] {
        &self.paths
    }

    pub fn set_crc_entries(&mut self, entries: Vec<CrcEntry>) {
        self.crc = entries;
    }

    pub fn set_path_entries(&mut self, entries: Vec<PathEntry>) {
        self.paths = entries;
    }

    /// Looks up the recorded size for a resource by name, checking the path
    /// entries first and then the CRC table.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<u32> {
        if let Some(e) = self.paths.iter().find(|e| e.name == name) {
            return Some(e.size);
        }
        let h = crc32(name.as_bytes());
        self.crc
            .binary_search_by_key(&h, |e| e.hash)
            .ok()
            .map(|i| self.crc[i].size)
    }

    /// Records the size for a resource by name, updating an existing entry or
    /// inserting a new CRC entry. Existing path entries take precedence.
    pub fn set(&mut self, name: &str, size: u32) {
        if let Some(e) = self.paths.iter_mut().find(|e| e.name == name) {
            e.size = size;
            return;
        }
        let h = crc32(name.as_bytes());
        match self.crc.binary_search_by_key(&h, |e| e.hash) {
            Ok(i) => self.crc[i].size = size,
            Err(i) => self.crc.insert(i, CrcEntry { hash: h, size }),
        }
    }

    /// Parses an RSTB file.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < HEADER_SIZE {
            return Err(Error::malformed("file too short to be a RESTBL"));
        }
        if bytes[0..6] != RSTBL_MAGIC {
            return Err(Error::bad_magic("RESTBL"));
        }
        let version = read_u32(bytes, 0x06);
        let path_size = read_u32(bytes, 0x0A);
        let crc_count = read_u32(bytes, 0x0E) as usize;
        let path_count = read_u32(bytes, 0x12) as usize;

        let path_size_usize = path_size as usize;
        let path_entry_size = path_size_usize
            .checked_add(4)
            .ok_or_else(|| Error::overflow("path entry size overflow"))?;

        let crc_bytes = crc_count
            .checked_mul(CRC_ENTRY_SIZE)
            .ok_or_else(|| Error::overflow("CRC table size overflow"))?;
        let path_bytes = path_count
            .checked_mul(path_entry_size)
            .ok_or_else(|| Error::overflow("path table size overflow"))?;
        let total = HEADER_SIZE
            .checked_add(crc_bytes)
            .and_then(|n| n.checked_add(path_bytes))
            .ok_or_else(|| Error::overflow("RESTBL total size overflow"))?;
        if bytes.len() != total {
            return Err(Error::malformed(format!(
                "RESTBL size mismatch: header implies {total} bytes, buffer is {} bytes",
                bytes.len()
            )));
        }

        let mut crc = Vec::with_capacity(crc_count);
        let mut off = HEADER_SIZE;
        let mut prev: Option<u32> = None;
        for i in 0..crc_count {
            let hash = read_u32(bytes, off);
            let size = read_u32(bytes, off + 4);
            if let Some(p) = prev
                && hash < p
            {
                return Err(Error::malformed(format!(
                    "CRC entries not sorted by hash (entry {i}: {hash:#010x} < {p:#010x})"
                )));
            }
            if let Some(p) = prev
                && hash == p
            {
                return Err(Error::malformed(format!(
                    "duplicate CRC hash {hash:#010x} at entry {i}"
                )));
            }
            prev = Some(hash);
            crc.push(CrcEntry { hash, size });
            off += CRC_ENTRY_SIZE;
        }

        let mut paths = Vec::with_capacity(path_count);
        let mut prev_name: Option<&[u8]> = None;
        for i in 0..path_count {
            let name_bytes = &bytes[off..off + path_size_usize];
            let size = read_u32(bytes, off + path_size_usize);
            if let Some(p) = prev_name
                && name_bytes <= p
            {
                return Err(Error::malformed(format!(
                    "path entries not sorted by name (entry {i})"
                )));
            }
            prev_name = Some(name_bytes);
            let end = name_bytes
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(name_bytes.len());
            for &b in &name_bytes[end..] {
                if b != 0 {
                    return Err(Error::malformed(format!(
                        "path entry {i} has non-zero bytes after NUL terminator"
                    )));
                }
            }
            let name = std::str::from_utf8(&name_bytes[..end])
                .map_err(|_| Error::invalid_utf8("RESTBL path entry"))?
                .to_owned();
            paths.push(PathEntry { name, size });
            off += path_entry_size;
        }

        Ok(Self {
            version,
            path_size,
            crc,
            paths,
        })
    }

    /// Serializes the table to `writer`, returning the number of bytes written.
    pub fn write<W: Write>(&self, writer: &mut W) -> Result<u64> {
        let path_size_usize = self.path_size as usize;
        let path_entry_size = path_size_usize
            .checked_add(4)
            .ok_or_else(|| Error::overflow("path entry size overflow"))?;

        let crc_count = u32::try_from(self.crc.len())
            .map_err(|_| Error::overflow("CRC entry count exceeds u32"))?;
        let path_count = u32::try_from(self.paths.len())
            .map_err(|_| Error::overflow("path entry count exceeds u32"))?;

        for w in self.crc.windows(2) {
            if w[0].hash > w[1].hash {
                return Err(Error::malformed("CRC entries are not sorted by hash"));
            }
            if w[0].hash == w[1].hash {
                return Err(Error::malformed(format!(
                    "duplicate CRC hash {:#010x}",
                    w[0].hash
                )));
            }
        }

        let mut prev_name: Option<&str> = None;
        for (i, e) in self.paths.iter().enumerate() {
            let bytes = e.name.as_bytes();
            if bytes.contains(&0) {
                return Err(Error::malformed(format!(
                    "path entry {i} (`{}`) contains a NUL byte",
                    e.name.escape_debug()
                )));
            }
            if bytes.len() > path_size_usize {
                return Err(Error::overflow(format!(
                    "path entry {i} (`{}`) is {} bytes, exceeds field width {path_size_usize}",
                    e.name.escape_debug(),
                    bytes.len()
                )));
            }
            if let Some(prev) = prev_name {
                if e.name == prev {
                    return Err(Error::malformed(format!(
                        "duplicate path entry `{}`",
                        e.name.escape_debug()
                    )));
                }
                if e.name.as_str() < prev {
                    return Err(Error::malformed(format!(
                        "path entries not sorted at index {i} (`{}`)",
                        e.name.escape_debug()
                    )));
                }
            }
            prev_name = Some(&e.name);
        }

        let total_size = HEADER_SIZE
            .checked_add(self.crc.len() * CRC_ENTRY_SIZE)
            .and_then(|n| n.checked_add(self.paths.len() * path_entry_size))
            .ok_or_else(|| Error::overflow("RESTBL total size overflow"))?;

        let mut out = Vec::with_capacity(total_size);
        out.extend_from_slice(&RSTBL_MAGIC);
        out.extend_from_slice(&self.version.to_le_bytes());
        out.extend_from_slice(&self.path_size.to_le_bytes());
        out.extend_from_slice(&crc_count.to_le_bytes());
        out.extend_from_slice(&path_count.to_le_bytes());

        for e in &self.crc {
            out.extend_from_slice(&e.hash.to_le_bytes());
            out.extend_from_slice(&e.size.to_le_bytes());
        }
        for e in &self.paths {
            let bytes = e.name.as_bytes();
            out.extend_from_slice(bytes);
            out.resize(out.len() + (path_size_usize - bytes.len()), 0);
            out.extend_from_slice(&e.size.to_le_bytes());
        }

        debug_assert_eq!(out.len(), total_size);
        writer.write_all(&out)?;
        Ok(out.len() as u64)
    }
}

fn read_u32(bytes: &[u8], off: usize) -> u32 {
    let arr: [u8; 4] = bytes[off..off + 4].try_into().expect("checked length");
    u32::from_le_bytes(arr)
}

#[must_use]
fn crc32(data: &[u8]) -> u32 {
    let mut c: u32 = 0xFFFF_FFFF;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            let mask = (c & 1).wrapping_neg();
            c = (c >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_known_vectors() {
        assert_eq!(crc32(b""), 0x0000_0000);
        assert_eq!(crc32(b"a"), 0xE8B7_BE43);
        assert_eq!(crc32(b"abc"), 0x3524_41C2);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    fn make_simple() -> Rstbl {
        let mut t = Rstbl::new(1, DEFAULT_PATH_SIZE);
        t.set_crc_entries(vec![
            CrcEntry {
                hash: 0x0000_0001,
                size: 16,
            },
            CrcEntry {
                hash: 0x0000_00FF,
                size: 32,
            },
            CrcEntry {
                hash: 0xDEAD_BEEF,
                size: 64,
            },
        ]);
        t.set_path_entries(vec![
            PathEntry {
                name: "Aaa/bbb.bgyml".into(),
                size: 100,
            },
            PathEntry {
                name: "Zzz/ccc.bntx".into(),
                size: 200,
            },
        ]);
        t
    }

    #[test]
    fn round_trip_synthetic() {
        let t = make_simple();
        let mut buf = Vec::new();
        t.write(&mut buf).unwrap();
        let back = Rstbl::parse(&buf).unwrap();
        assert_eq!(back.version(), t.version());
        assert_eq!(back.path_size(), t.path_size());
        assert_eq!(back.crc_entries(), t.crc_entries());
        assert_eq!(back.path_entries(), t.path_entries());
    }

    #[test]
    fn parse_rejects_too_short() {
        let err = Rstbl::parse(&[0u8; 4]).unwrap_err();
        let Error::Malformed(msg) = err else {
            panic!("expected Malformed, got {err:?}");
        };
        assert!(msg.contains("too short"));
    }

    #[test]
    fn parse_rejects_bad_magic() {
        let mut bytes = Vec::new();
        make_simple().write(&mut bytes).unwrap();
        bytes[0] = b'X';
        let err = Rstbl::parse(&bytes).unwrap_err();
        assert!(
            matches!(err, Error::BadMagic { format: "RESTBL" }),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_rejects_size_mismatch() {
        let mut bytes = Vec::new();
        make_simple().write(&mut bytes).unwrap();
        bytes.pop();
        let err = Rstbl::parse(&bytes).unwrap_err();
        let Error::Malformed(msg) = err else {
            panic!("expected Malformed, got {err:?}");
        };
        assert!(msg.contains("size mismatch"));
    }

    #[test]
    fn write_rejects_unsorted_crc() {
        let mut t = Rstbl::new(1, DEFAULT_PATH_SIZE);
        t.set_crc_entries(vec![
            CrcEntry { hash: 5, size: 0 },
            CrcEntry { hash: 4, size: 0 },
        ]);
        let mut buf = Vec::new();
        let err = t.write(&mut buf).unwrap_err();
        let Error::Malformed(msg) = err else {
            panic!("expected Malformed, got {err:?}");
        };
        assert!(msg.contains("not sorted"));
    }

    #[test]
    fn write_rejects_oversized_path_name() {
        let mut t = Rstbl::new(1, 16);
        t.set_path_entries(vec![PathEntry {
            name: "this/is/way/too/long.bin".into(),
            size: 1,
        }]);
        let mut buf = Vec::new();
        let err = t.write(&mut buf).unwrap_err();
        let Error::Overflow(msg) = err else {
            panic!("expected Overflow, got {err:?}");
        };
        assert!(msg.contains("exceeds field width"));
    }

    #[test]
    fn set_updates_existing_path_entry() {
        let mut t = make_simple();
        t.set("Aaa/bbb.bgyml", 999);
        assert_eq!(t.get("Aaa/bbb.bgyml"), Some(999));
        assert_eq!(t.path_entries().len(), 2);
    }

    #[test]
    fn set_inserts_new_crc_entry_in_sorted_position() {
        let mut t = Rstbl::new(1, DEFAULT_PATH_SIZE);
        t.set("foo", 10);
        t.set("bar", 20);
        t.set("baz", 30);
        let hashes: Vec<u32> = t.crc_entries().iter().map(|e| e.hash).collect();
        let mut sorted = hashes.clone();
        sorted.sort_unstable();
        assert_eq!(hashes, sorted);
        assert_eq!(t.get("foo"), Some(10));
        assert_eq!(t.get("bar"), Some(20));
        assert_eq!(t.get("baz"), Some(30));
    }

    #[test]
    fn set_replaces_existing_crc_entry() {
        let mut t = Rstbl::new(1, DEFAULT_PATH_SIZE);
        t.set("foo", 10);
        t.set("foo", 99);
        assert_eq!(t.get("foo"), Some(99));
        assert_eq!(t.crc_entries().len(), 1);
    }
}
