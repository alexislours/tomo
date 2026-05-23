use std::collections::HashMap;
use std::io::Write;

pub use crate::formats::binio::ByteOrder;

use crate::formats::amta::{
    AMTA_MAGIC, NAME_PTR_OFFSET as AMTA_NAME_PTR_OFFSET, SIZE_OFFSET as AMTA_SIZE_OFFSET, read_name,
};
use crate::formats::bwav::{self, BWAV_MAGIC};
use crate::formats::hash::crc32;
use crate::{Error, Result};

pub const BARS_MAGIC: [u8; 4] = *b"BARS";

const HEADER_SIZE: usize = 0x10;
const OFFSET_SET_SIZE: usize = 8;
const ASSET_ALIGN: usize = 0x40;
const ABSENT: u32 = 0xFFFF_FFFF;

/// One entry in a [`Bars`] archive: an AMTA metadata blob and an optional audio
/// asset (typically a BWAV).
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub hash: u32,
    meta_off: usize,
    meta_len: usize,
    asset: Option<(usize, usize)>,
}

/// A parsed BARS audio archive, owning its bytes so entry data can be borrowed
/// from it.
#[derive(Debug)]
pub struct Bars {
    byte_order: ByteOrder,
    version: u16,
    reset_table: Vec<u8>,
    entries: Vec<Entry>,
    total_size: usize,
    bytes: Vec<u8>,
}

impl Bars {
    #[must_use]
    pub fn byte_order(&self) -> ByteOrder {
        self.byte_order
    }
    #[must_use]
    pub fn version(&self) -> u16 {
        self.version
    }
    #[must_use]
    pub fn reset_table(&self) -> &[u8] {
        &self.reset_table
    }
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }
    #[must_use]
    pub fn total_size(&self) -> usize {
        self.total_size
    }

    /// Returns the AMTA metadata bytes for `entry`.
    #[must_use]
    pub fn meta(&self, entry: &Entry) -> &[u8] {
        &self.bytes[entry.meta_off..entry.meta_off + entry.meta_len]
    }

    /// Returns the audio asset bytes for `entry`, if it has one.
    #[must_use]
    pub fn asset(&self, entry: &Entry) -> Option<&[u8]> {
        entry.asset.map(|(off, len)| &self.bytes[off..off + len])
    }

    /// Parses a BARS archive, taking ownership of `bytes`.
    pub fn parse(bytes: Vec<u8>) -> Result<Self> {
        if bytes.len() < HEADER_SIZE {
            return Err(Error::malformed("file too short to be a BARS"));
        }
        if bytes[0..4] != BARS_MAGIC {
            return Err(Error::bad_magic("BARS"));
        }
        let byte_order = match [bytes[8], bytes[9]] {
            [0xFE, 0xFF] => ByteOrder::Big,
            [0xFF, 0xFE] => ByteOrder::Little,
            _ => return Err(Error::malformed("invalid BOM in BARS header")),
        };
        let total_size = byte_order.read_u32(&bytes, 4, "BARS file size")? as usize;
        let version = byte_order.read_u16(&bytes, 10, "BARS version")?;
        let count = byte_order.read_u32(&bytes, 12, "BARS asset count")? as usize;

        if total_size != bytes.len() {
            return Err(Error::malformed(format!(
                "BARS header claims size {total_size} but buffer is {} bytes",
                bytes.len()
            )));
        }

        let hash_off = HEADER_SIZE;
        let osets_off = hash_off + count * 4;
        let osets_end = osets_off + count * OFFSET_SET_SIZE;
        if bytes.len() < osets_end {
            return Err(Error::malformed("truncated BARS hash/offset tables"));
        }

        let mut hashes = Vec::with_capacity(count);
        let mut prev = None;
        for i in 0..count {
            let h = byte_order.read_u32(&bytes, hash_off + i * 4, "BARS hash")?;
            if let Some(p) = prev
                && h < p
            {
                return Err(Error::malformed(format!(
                    "BARS hash table is not sorted (entry {i}: {h:#010x} < {p:#010x})"
                )));
            }
            prev = Some(h);
            hashes.push(h);
        }

        let mut meta_offs = Vec::with_capacity(count);
        let mut asset_offs = Vec::with_capacity(count);
        for i in 0..count {
            let base = osets_off + i * OFFSET_SET_SIZE;
            let m = byte_order.read_u32(&bytes, base, "BARS meta offset")?;
            let a = byte_order.read_u32(&bytes, base + 4, "BARS asset offset")?;
            meta_offs.push(m as usize);
            asset_offs.push(if a == ABSENT { None } else { Some(a as usize) });
        }

        let first_meta = meta_offs.iter().copied().min().unwrap_or(osets_end);
        if first_meta < osets_end || first_meta > bytes.len() {
            return Err(Error::malformed("BARS metadata offset out of range"));
        }
        let reset_table = bytes[osets_end..first_meta].to_vec();

        let entries = build_entries(&bytes, byte_order, &hashes, &meta_offs, &asset_offs)?;

        Ok(Self {
            byte_order,
            version,
            reset_table,
            entries,
            total_size,
            bytes,
        })
    }
}

fn build_entries(
    bytes: &[u8],
    byte_order: ByteOrder,
    hashes: &[u32],
    meta_offs: &[usize],
    asset_offs: &[Option<usize>],
) -> Result<Vec<Entry>> {
    let mut boundaries: Vec<usize> = meta_offs.to_vec();
    boundaries.extend(asset_offs.iter().flatten().copied());
    boundaries.push(bytes.len());
    boundaries.sort_unstable();
    boundaries.dedup();
    let next_boundary = |off: usize| -> usize {
        match boundaries.binary_search(&off) {
            Ok(i) => boundaries.get(i + 1).copied().unwrap_or(bytes.len()),
            Err(i) => boundaries.get(i).copied().unwrap_or(bytes.len()),
        }
    };

    let mut entries = Vec::with_capacity(meta_offs.len());
    for (i, &meta_off) in meta_offs.iter().enumerate() {
        if meta_off + AMTA_NAME_PTR_OFFSET + 4 > bytes.len()
            || bytes[meta_off..meta_off + 4] != AMTA_MAGIC
        {
            return Err(Error::malformed(format!(
                "entry {i} is missing its AMTA block"
            )));
        }
        let meta_len =
            byte_order.read_u32(bytes, meta_off + AMTA_SIZE_OFFSET, "AMTA size")? as usize;
        if meta_off + meta_len > bytes.len() {
            return Err(Error::malformed(format!(
                "entry {i} AMTA size overflows buffer"
            )));
        }
        let name = read_name(bytes, byte_order, meta_off)?;

        let asset = match asset_offs[i] {
            None => None,
            Some(off) => {
                if off > bytes.len() {
                    return Err(Error::malformed(format!(
                        "entry {i} asset offset {off:#x} past buffer ({:#x})",
                        bytes.len()
                    )));
                }
                let len = asset_len(bytes, off, next_boundary(off))?;
                Some((off, len))
            }
        };

        entries.push(Entry {
            name,
            hash: hashes[i],
            meta_off,
            meta_len,
            asset,
        });
    }
    Ok(entries)
}

fn asset_len(bytes: &[u8], off: usize, boundary: usize) -> Result<usize> {
    let boundary = boundary.min(bytes.len());
    let span = boundary.saturating_sub(off);
    if off + 4 <= bytes.len() && bytes[off..off + 4] == BWAV_MAGIC {
        let slice = bytes[off..boundary].to_vec();
        let bwav = bwav::Bwav::parse(slice)?;
        return Ok(bwav.full_frame_len().min(span));
    }
    Ok(span)
}

/// An entry to be packed into a BARS archive by [`write()`].
#[derive(Debug)]
pub struct PackEntry<'a> {
    pub name: &'a str,
    pub meta: &'a [u8],
    pub asset: Option<&'a [u8]>,
}

struct Slot<'a> {
    hash: u32,
    entry: &'a PackEntry<'a>,
}

/// Writes a BARS archive from `entries`, returning the number of bytes written.
pub fn write<W: Write>(
    writer: &mut W,
    entries: &[PackEntry<'_>],
    byte_order: ByteOrder,
    version: u16,
    reset_table: &[u8],
) -> Result<u64> {
    for e in entries {
        if e.name.is_empty() {
            return Err(Error::malformed("entry name is empty"));
        }
    }

    let mut slots: Vec<Slot<'_>> = entries
        .iter()
        .map(|e| Slot {
            hash: crc32(e.name.as_bytes()),
            entry: e,
        })
        .collect();
    slots.sort_by_key(|s| s.hash);

    for w in slots.windows(2) {
        if w[0].hash == w[1].hash {
            let msg = if w[0].entry.name == w[1].entry.name {
                format!("duplicate entry name `{}`", w[0].entry.name)
            } else {
                format!(
                    "hash collision between `{}` and `{}` is not supported",
                    w[0].entry.name, w[1].entry.name
                )
            };
            return Err(Error::malformed(msg));
        }
    }

    let count = slots.len();
    let osets_off = HEADER_SIZE + count * 4;
    let osets_end = osets_off + count * OFFSET_SET_SIZE;
    let data_start = osets_end + reset_table.len();

    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(&BARS_MAGIC);
    byte_order.put_u32(&mut out, 0);
    out.extend_from_slice(&byte_order.bom());
    byte_order.put_u16(&mut out, version);
    let count_u32 = u32::try_from(count).map_err(|_| Error::overflow("BARS asset count > u32"))?;
    byte_order.put_u32(&mut out, count_u32);

    for slot in &slots {
        byte_order.put_u32(&mut out, slot.hash);
    }

    let osets_pos = out.len();
    out.resize(out.len() + count * OFFSET_SET_SIZE, 0);
    out.extend_from_slice(reset_table);
    debug_assert_eq!(out.len(), data_start);

    let mut meta_offs = Vec::with_capacity(count);
    for slot in &slots {
        meta_offs.push(out.len());
        out.extend_from_slice(slot.entry.meta);
    }

    let mut asset_offs: Vec<Option<usize>> = vec![None; count];
    let mut placed: HashMap<&[u8], usize> = HashMap::new();
    for (i, slot) in slots.iter().enumerate() {
        let Some(data) = slot.entry.asset else {
            continue;
        };
        if let Some(&off) = placed.get(data) {
            asset_offs[i] = Some(off);
            continue;
        }
        let target = out.len().next_multiple_of(ASSET_ALIGN);
        out.resize(target, 0);
        let off = out.len();
        out.extend_from_slice(data);
        placed.insert(data, off);
        asset_offs[i] = Some(off);
    }

    for (i, meta_off) in meta_offs.iter().enumerate() {
        let base = osets_pos + i * OFFSET_SET_SIZE;
        let m = u32::try_from(*meta_off).map_err(|_| Error::overflow("meta offset > u32"))?;
        byte_order.write_u32_at(&mut out, base, m);
        let a = match asset_offs[i] {
            Some(off) => u32::try_from(off).map_err(|_| Error::overflow("asset offset > u32"))?,
            None => ABSENT,
        };
        byte_order.write_u32_at(&mut out, base + 4, a);
    }

    let total = u32::try_from(out.len()).map_err(|_| Error::overflow("BARS size > u32"))?;
    byte_order.write_u32_at(&mut out, 4, total);

    writer.write_all(&out)?;
    Ok(u64::from(total))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn amta(name: &str) -> Vec<u8> {
        let mut block = vec![0u8; 0x34];
        block[0..4].copy_from_slice(&AMTA_MAGIC);
        block[4] = 0xFF;
        block[5] = 0xFE;
        let name_off = u32::try_from(block.len() - AMTA_NAME_PTR_OFFSET).unwrap();
        block[AMTA_NAME_PTR_OFFSET..AMTA_NAME_PTR_OFFSET + 4]
            .copy_from_slice(&name_off.to_le_bytes());
        block.extend_from_slice(name.as_bytes());
        block.push(0);
        block.resize(block.len().next_multiple_of(4), 0);
        let size = u32::try_from(block.len()).unwrap();
        block[AMTA_SIZE_OFFSET..AMTA_SIZE_OFFSET + 4].copy_from_slice(&size.to_le_bytes());
        block
    }

    #[test]
    fn round_trip_with_dedup_and_absent() {
        let a_meta = amta("AAA_one");
        let b_meta = amta("BBB_two");
        let c_meta = amta("CCC_three");
        let shared = vec![1u8, 2, 3, 4, 5];
        let entries = vec![
            PackEntry {
                name: "AAA_one",
                meta: &a_meta,
                asset: Some(&shared),
            },
            PackEntry {
                name: "BBB_two",
                meta: &b_meta,
                asset: Some(&shared),
            },
            PackEntry {
                name: "CCC_three",
                meta: &c_meta,
                asset: None,
            },
        ];
        let mut buf = Vec::new();
        write(&mut buf, &entries, ByteOrder::Little, 0x0102, &[]).unwrap();
        let bars = Bars::parse(buf).unwrap();
        assert_eq!(bars.entries().len(), 3);

        let by_name: std::collections::BTreeMap<&str, &Entry> = bars
            .entries()
            .iter()
            .map(|e| (e.name.as_str(), e))
            .collect();
        assert_eq!(bars.asset(by_name["AAA_one"]).unwrap(), shared.as_slice());
        assert_eq!(bars.asset(by_name["BBB_two"]).unwrap(), shared.as_slice());
        assert!(bars.asset(by_name["CCC_three"]).is_none());

        let hashes: Vec<u32> = bars.entries().iter().map(|e| e.hash).collect();
        let mut sorted = hashes.clone();
        sorted.sort_unstable();
        assert_eq!(hashes, sorted);
        assert_eq!(bars.total_size(), bars.bytes.len());
        for e in bars.entries() {
            assert_eq!(e.hash, crc32(e.name.as_bytes()));
            if let Some((off, _)) = e.asset {
                assert_eq!(off % ASSET_ALIGN, 0);
            }
        }
    }

    fn dsp_bwav() -> Vec<u8> {
        let info = bwav::BwavChannel {
            codec: bwav::CODEC_DSP_ADPCM,
            channel_pan: 2,
            sample_rate: 48000,
            sample_count_full: 28,
            sample_count: 28,
            coefficients: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
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
        let data: Vec<u8> = (0..16u8).collect();
        let mut buf = Vec::new();
        bwav::write(
            &mut buf,
            ByteOrder::Little,
            1,
            0,
            0,
            &[bwav::PackChannel { info, data: &data }],
        )
        .unwrap();
        buf
    }

    #[test]
    fn bwav_asset_round_trips_through_bars() {
        let asset = dsp_bwav();
        let meta = amta("BGM_Test_Track");
        let entries = vec![PackEntry {
            name: "BGM_Test_Track",
            meta: &meta,
            asset: Some(&asset),
        }];
        let mut buf = Vec::new();
        write(&mut buf, &entries, ByteOrder::Little, 0x0102, &[]).unwrap();
        let bars = Bars::parse(buf).unwrap();

        let e = &bars.entries()[0];
        let extracted = bars.asset(e).unwrap();
        assert_eq!(extracted, asset.as_slice());
        let parsed = bwav::Bwav::parse(extracted.to_vec()).unwrap();
        assert_eq!(parsed.channels().len(), 1);
        assert_eq!(parsed.channels()[0].sample_count, 28);
    }

    #[test]
    fn rejects_bad_magic() {
        let err = Bars::parse(vec![0u8; 0x10]).unwrap_err();
        assert!(matches!(err, Error::BadMagic { .. }));
    }

    #[test]
    fn rejects_out_of_range_asset_offset() {
        let asset = dsp_bwav();
        let meta = amta("BGM_Test_Track");
        let entries = vec![PackEntry {
            name: "BGM_Test_Track",
            meta: &meta,
            asset: Some(&asset),
        }];
        let mut buf = Vec::new();
        write(&mut buf, &entries, ByteOrder::Little, 0x0102, &[]).unwrap();

        let asset_off_field = HEADER_SIZE + 4 + 4;
        buf[asset_off_field..asset_off_field + 4].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());

        let err = Bars::parse(buf).unwrap_err();
        assert!(matches!(err, Error::Malformed(_)));
    }
}
