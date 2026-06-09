use std::io::{self, Read, Seek, SeekFrom};

use crate::formats::nca::crypto::ctr_apply;
use crate::{Error, Result};

const NODE_SIZE: usize = 0x4000;
const NODE_HEADER: usize = 0x10;
const INDIRECT_ENTRY: usize = 0x14;
const SUBSEC_ENTRY: usize = 0x10;

const SOURCE_BASE: u32 = 0;
const SOURCE_PATCH: u32 = 1;

#[derive(Debug, Clone, Copy)]
pub(crate) struct PatchInfo {
    pub indirect_offset: u64,
    pub indirect_size: u64,
    pub aes_ctr_ex_offset: u64,
    pub aes_ctr_ex_size: u64,
}

impl PatchInfo {
    pub(crate) fn parse(fs_header: &[u8]) -> Option<Self> {
        let p = fs_header.get(0x100..0x140)?;
        if &p[0x10..0x14] != b"BKTR" || &p[0x30..0x34] != b"BKTR" {
            return None;
        }
        let r = |o: usize| u64::from_le_bytes(p[o..o + 8].try_into().unwrap());
        let info = Self {
            indirect_offset: r(0x00),
            indirect_size: r(0x08),
            aes_ctr_ex_offset: r(0x20),
            aes_ctr_ex_size: r(0x28),
        };
        if info.indirect_size == 0 || info.aes_ctr_ex_size == 0 {
            return None;
        }
        Some(info)
    }
}

#[derive(Debug)]
struct IndirectEntry {
    virt: u64,
    phys: u64,
    size: u64,
    source: u32,
}

#[derive(Debug)]
struct SubsecEntry {
    offset: u64,
    end: u64,
    generation: u32,
}

#[derive(Debug)]
pub struct Tables {
    indirect: Vec<IndirectEntry>,
    subsec: Vec<SubsecEntry>,
}

fn le_u32(b: &[u8], off: usize) -> Result<u32> {
    b.get(off..off + 4)
        .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
        .ok_or_else(|| Error::malformed("bktr table truncated"))
}

fn le_u64(b: &[u8], off: usize) -> Result<u64> {
    b.get(off..off + 8)
        .map(|s| u64::from_le_bytes(s.try_into().unwrap()))
        .ok_or_else(|| Error::malformed("bktr table truncated"))
}

impl Tables {
    pub(crate) fn parse(indirect: &[u8], aes_ctr_ex: &[u8]) -> Result<Self> {
        let total_size = le_u64(indirect, 0x08)?;
        let bucket_count = le_u32(indirect, 0x04)? as usize;

        let mut entries: Vec<IndirectEntry> = Vec::new();
        for i in 0..bucket_count {
            let base = NODE_SIZE
                .checked_mul(i + 1)
                .ok_or_else(|| Error::malformed("bktr indirect bucket overflow"))?;
            let count = le_u32(indirect, base + 0x04)? as usize;
            for j in 0..count {
                let eo = base + NODE_HEADER + j * INDIRECT_ENTRY;
                entries.push(IndirectEntry {
                    virt: le_u64(indirect, eo)?,
                    phys: le_u64(indirect, eo + 0x08)?,
                    size: 0,
                    source: le_u32(indirect, eo + 0x10)?,
                });
            }
        }
        if entries.is_empty() {
            return Err(Error::malformed("bktr indirect table is empty"));
        }
        for k in 0..entries.len() {
            let end = if k + 1 < entries.len() {
                entries[k + 1].virt
            } else {
                total_size
            };
            let start = entries[k].virt;
            entries[k].size = end
                .checked_sub(start)
                .ok_or_else(|| Error::malformed("bktr indirect entries not ascending"))?;
        }

        let subsec_buckets = le_u32(aes_ctr_ex, 0x04)? as usize;
        let mut subsec: Vec<SubsecEntry> = Vec::new();
        for i in 0..subsec_buckets {
            let base = NODE_SIZE
                .checked_mul(i + 1)
                .ok_or_else(|| Error::malformed("bktr subsection bucket overflow"))?;
            let count = le_u32(aes_ctr_ex, base + 0x04)? as usize;
            let bucket_end = le_u64(aes_ctr_ex, base + 0x08)?;
            for j in 0..count {
                let eo = base + NODE_HEADER + j * SUBSEC_ENTRY;
                let offset = le_u64(aes_ctr_ex, eo)?;
                let end = if j + 1 < count {
                    le_u64(aes_ctr_ex, base + NODE_HEADER + (j + 1) * SUBSEC_ENTRY)?
                } else {
                    bucket_end
                };
                subsec.push(SubsecEntry {
                    offset,
                    end,
                    generation: le_u32(aes_ctr_ex, eo + 0x0C)?,
                });
            }
        }
        if subsec.is_empty() {
            return Err(Error::malformed("bktr subsection table is empty"));
        }
        if subsec.windows(2).any(|w| w[1].offset < w[0].offset) {
            return Err(Error::malformed("bktr subsection entries not ascending"));
        }

        Ok(Self {
            indirect: entries,
            subsec,
        })
    }

    fn find_indirect(&self, virt: u64) -> Result<&IndirectEntry> {
        let idx = self
            .indirect
            .partition_point(|e| e.virt <= virt)
            .checked_sub(1)
            .ok_or_else(|| Error::malformed("bktr virtual offset before first entry"))?;
        Ok(&self.indirect[idx])
    }

    fn find_subsec(&self, off: u64) -> Result<&SubsecEntry> {
        let idx = self
            .subsec
            .partition_point(|e| e.offset <= off)
            .checked_sub(1)
            .ok_or_else(|| Error::malformed("bktr physical offset before first subsection"))?;
        Ok(&self.subsec[idx])
    }
}

pub struct PatchStream<'a, P, B> {
    tables: &'a Tables,
    patch_reader: &'a mut P,
    base_reader: &'a mut B,
    patch_key: [u8; 16],
    patch_offset: u64,
    patch_ctr: [u8; 16],
    base_key: Option<[u8; 16]>,
    base_offset: u64,
    base_ctr: [u8; 16],
    fs_offset: u64,
    fs_size: u64,
    pos: u64,
}

impl<P, B> std::fmt::Debug for PatchStream<'_, P, B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PatchStream")
            .field("fs_offset", &self.fs_offset)
            .field("fs_size", &self.fs_size)
            .field("pos", &self.pos)
            .finish()
    }
}

impl<'a, P, B> PatchStream<'a, P, B> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        tables: &'a Tables,
        patch_reader: &'a mut P,
        base_reader: &'a mut B,
        patch_key: [u8; 16],
        patch_offset: u64,
        patch_ctr: [u8; 16],
        base_key: Option<[u8; 16]>,
        base_offset: u64,
        base_ctr: [u8; 16],
        fs_offset: u64,
        fs_size: u64,
    ) -> Self {
        Self {
            tables,
            patch_reader,
            base_reader,
            patch_key,
            patch_offset,
            patch_ctr,
            base_key,
            base_offset,
            base_ctr,
            fs_offset,
            fs_size,
            pos: 0,
        }
    }
}

fn read_exact_at<R: Read + Seek>(reader: &mut R, abs: u64, buf: &mut [u8]) -> io::Result<()> {
    reader.seek(SeekFrom::Start(abs))?;
    reader.read_exact(buf)
}

fn to_io(err: Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err)
}

impl<P: Read + Seek, B: Read + Seek> Read for PatchStream<'_, P, B> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let remaining = self.fs_size.saturating_sub(self.pos);
        let want_total = buf
            .len()
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        if want_total == 0 {
            return Ok(0);
        }

        let mut filled = 0usize;
        while filled < want_total {
            let virt = self.fs_offset + self.pos + filled as u64;
            let entry = self.tables.find_indirect(virt).map_err(to_io)?;
            let entry_avail = (entry.virt + entry.size).saturating_sub(virt);
            if entry_avail == 0 {
                return Err(to_io(Error::malformed("bktr ran past indirect range")));
            }
            let delta = virt - entry.virt;
            let src = entry.phys + delta;
            let budget = (want_total - filled) as u64;

            match entry.source {
                SOURCE_BASE => {
                    let take = usize::try_from(budget.min(entry_avail)).unwrap_or(usize::MAX);
                    let dst = &mut buf[filled..filled + take];
                    read_exact_at(self.base_reader, self.base_offset + src, dst)?;
                    if let Some(key) = self.base_key {
                        ctr_apply(&key, &self.base_ctr, self.base_offset + src, dst);
                    }
                    filled += take;
                }
                SOURCE_PATCH => {
                    let sub = self.tables.find_subsec(src).map_err(to_io)?;
                    let sub_avail = sub.end.saturating_sub(src);
                    if sub_avail == 0 {
                        return Err(to_io(Error::malformed("bktr ran past subsection range")));
                    }
                    let take = usize::try_from(budget.min(entry_avail).min(sub_avail))
                        .unwrap_or(usize::MAX);
                    let mut ctr = self.patch_ctr;
                    ctr[4..8].copy_from_slice(&sub.generation.to_be_bytes());
                    let dst = &mut buf[filled..filled + take];
                    read_exact_at(self.patch_reader, self.patch_offset + src, dst)?;
                    ctr_apply(&self.patch_key, &ctr, self.patch_offset + src, dst);
                    filled += take;
                }
                other => {
                    return Err(to_io(Error::unsupported(format!(
                        "bktr storage source {other}"
                    ))));
                }
            }
        }

        self.pos += filled as u64;
        Ok(filled)
    }
}

impl<P, B> Seek for PatchStream<'_, P, B> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let new = match pos {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::End(n) => self.fs_size.checked_add_signed(n),
            SeekFrom::Current(n) => self.pos.checked_add_signed(n),
        };
        self.pos =
            new.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek out of range"))?;
        Ok(self.pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put_u32(buf: &mut [u8], off: usize, v: u32) {
        buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn put_u64(buf: &mut [u8], off: usize, v: u64) {
        buf[off..off + 8].copy_from_slice(&v.to_le_bytes());
    }

    fn build_indirect(entries: &[(u64, u64, u32)], total: u64) -> Vec<u8> {
        let mut b = vec![0u8; NODE_SIZE + NODE_HEADER + entries.len() * INDIRECT_ENTRY];
        put_u32(&mut b, 0x04, 1);
        put_u64(&mut b, 0x08, total);
        put_u32(
            &mut b,
            NODE_SIZE + 0x04,
            u32::try_from(entries.len()).unwrap(),
        );
        put_u64(&mut b, NODE_SIZE + 0x08, total);
        for (i, (virt, phys, src)) in entries.iter().enumerate() {
            let eo = NODE_SIZE + NODE_HEADER + i * INDIRECT_ENTRY;
            put_u64(&mut b, eo, *virt);
            put_u64(&mut b, eo + 0x08, *phys);
            put_u32(&mut b, eo + 0x10, *src);
        }
        b
    }

    fn build_subsec(entries: &[(u64, u32)], bucket_end: u64) -> Vec<u8> {
        let mut b = vec![0u8; NODE_SIZE + NODE_HEADER + entries.len() * SUBSEC_ENTRY];
        put_u32(&mut b, 0x04, 1);
        put_u64(&mut b, 0x08, bucket_end);
        put_u32(
            &mut b,
            NODE_SIZE + 0x04,
            u32::try_from(entries.len()).unwrap(),
        );
        put_u64(&mut b, NODE_SIZE + 0x08, bucket_end);
        for (i, (offset, generation)) in entries.iter().enumerate() {
            let eo = NODE_SIZE + NODE_HEADER + i * SUBSEC_ENTRY;
            put_u64(&mut b, eo, *offset);
            put_u32(&mut b, eo + 0x0C, *generation);
        }
        b
    }

    #[test]
    fn parses_entry_sizes_and_sources() {
        let indirect = build_indirect(
            &[
                (0, 0x1000, SOURCE_BASE),
                (0x100, 0, SOURCE_PATCH),
                (0x200, 0x200, SOURCE_BASE),
            ],
            0x300,
        );
        let subsec = build_subsec(&[(0, 5), (0x80, 9)], 0x100);
        let t = Tables::parse(&indirect, &subsec).unwrap();

        assert_eq!(t.indirect.len(), 3);
        assert_eq!(t.indirect[0].size, 0x100);
        assert_eq!(t.indirect[1].size, 0x100);
        assert_eq!(t.indirect[2].size, 0x100);
        assert_eq!(t.indirect[1].source, SOURCE_PATCH);
        assert_eq!(t.indirect[1].phys, 0);
    }

    #[test]
    fn floor_lookups_pick_covering_entry() {
        let indirect = build_indirect(
            &[
                (0, 0, SOURCE_BASE),
                (0x100, 0, SOURCE_PATCH),
                (0x200, 0, SOURCE_BASE),
            ],
            0x300,
        );
        let subsec = build_subsec(&[(0, 5), (0x80, 9)], 0x100);
        let t = Tables::parse(&indirect, &subsec).unwrap();

        assert_eq!(t.find_indirect(0).unwrap().virt, 0);
        assert_eq!(t.find_indirect(0xFF).unwrap().virt, 0);
        assert_eq!(t.find_indirect(0x100).unwrap().virt, 0x100);
        assert_eq!(t.find_indirect(0x2FF).unwrap().virt, 0x200);

        assert_eq!(t.find_subsec(0).unwrap().generation, 5);
        assert_eq!(t.find_subsec(0x7F).unwrap().generation, 5);
        assert_eq!(t.find_subsec(0x80).unwrap().generation, 9);
        assert_eq!(t.find_subsec(0x80).unwrap().end, 0x100);
    }

    #[test]
    fn rejects_empty_tables() {
        let indirect = build_indirect(&[], 0);
        let subsec = build_subsec(&[(0, 1)], 0x10);
        assert!(Tables::parse(&indirect, &subsec).is_err());
    }

    #[test]
    fn rejects_unsorted_subsections() {
        let indirect = build_indirect(&[(0, 0, SOURCE_BASE)], 0x100);
        let subsec = build_subsec(&[(0x80, 1), (0x10, 2)], 0x100);
        assert!(Tables::parse(&indirect, &subsec).is_err());
    }
}
