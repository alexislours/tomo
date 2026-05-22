use std::io::Write;

pub use crate::formats::binio::ByteOrder;

use crate::{Error, Result};

pub const SARC_MAGIC: [u8; 4] = *b"SARC";
const SFAT_MAGIC: [u8; 4] = *b"SFAT";
const SFNT_MAGIC: [u8; 4] = *b"SFNT";

const SARC_HEADER_SIZE: u16 = 0x14;
const SFAT_HEADER_SIZE: u16 = 0x0C;
const SFNT_HEADER_SIZE: u16 = 0x08;
const SFAT_ENTRY_SIZE_U32: u32 = 0x10;
const SFAT_ENTRY_SIZE: usize = SFAT_ENTRY_SIZE_U32 as usize;
const DEFAULT_HASH_MULTIPLIER: u32 = 0x65;
const MAX_FILES: usize = 0x3FFF;
const SARC_VERSION: u16 = 0x0100;

/// One file inside a [`Sarc`] archive. `name` is present when the archive
/// includes a name table.
#[derive(Debug, Clone)]
pub struct Entry {
    pub hash: u32,
    pub collision: u8,
    pub name: Option<String>,
    pub data_start: u32,
    pub(crate) data_end: u32,
}

impl Entry {
    /// Length in bytes of this entry's data.
    #[must_use]
    pub fn len(&self) -> u32 {
        self.data_end.saturating_sub(self.data_start)
    }

    /// Whether this entry has no data.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data_end == self.data_start
    }
}

/// A parsed SARC archive, owning its bytes so entry data can be borrowed from
/// it.
#[derive(Debug)]
pub struct Sarc {
    byte_order: ByteOrder,
    version: u16,
    hash_multiplier: u32,
    data_offset: u32,
    total_size: u32,
    entries: Vec<Entry>,
    bytes: Vec<u8>,
}

impl Sarc {
    #[must_use]
    pub fn byte_order(&self) -> ByteOrder {
        self.byte_order
    }
    #[must_use]
    pub fn version(&self) -> u16 {
        self.version
    }
    #[must_use]
    pub fn hash_multiplier(&self) -> u32 {
        self.hash_multiplier
    }
    #[must_use]
    pub fn data_offset(&self) -> u32 {
        self.data_offset
    }
    #[must_use]
    pub fn total_size(&self) -> u32 {
        self.total_size
    }
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Returns the bytes of `entry` within this archive.
    #[must_use]
    pub fn data(&self, entry: &Entry) -> &[u8] {
        let base = self.data_offset as usize;
        &self.bytes[base + entry.data_start as usize..base + entry.data_end as usize]
    }

    /// Parses a SARC archive, taking ownership of `bytes`.
    pub fn parse(bytes: Vec<u8>) -> Result<Self> {
        if bytes.len() < SARC_HEADER_SIZE as usize {
            return Err(Error::malformed("file too short to be a SARC"));
        }
        if bytes[0..4] != SARC_MAGIC {
            return Err(Error::bad_magic("SARC"));
        }

        let byte_order = match [bytes[6], bytes[7]] {
            [0xFE, 0xFF] => ByteOrder::Big,
            [0xFF, 0xFE] => ByteOrder::Little,
            _ => return Err(Error::malformed("invalid BOM in SARC header")),
        };

        let header_size = byte_order.read_u16(&bytes, 4, "SARC header size")?;
        if header_size != SARC_HEADER_SIZE {
            return Err(Error::malformed(format!(
                "unexpected SARC header size {header_size:#x} (want {SARC_HEADER_SIZE:#x})"
            )));
        }
        let total_size = byte_order.read_u32(&bytes, 8, "SARC total size")?;
        let data_offset = byte_order.read_u32(&bytes, 12, "SARC data offset")?;
        let version = byte_order.read_u16(&bytes, 16, "SARC version")?;

        if (total_size as usize) != bytes.len() {
            return Err(Error::malformed(format!(
                "SARC header claims size {total_size} but buffer is {} bytes",
                bytes.len()
            )));
        }
        if data_offset > total_size {
            return Err(Error::malformed(format!(
                "data offset {data_offset:#x} exceeds total size {total_size:#x}"
            )));
        }

        let (entries, hash_multiplier) =
            parse_sfat_sfnt(&bytes, byte_order, data_offset, total_size)?;

        Ok(Self {
            byte_order,
            version,
            hash_multiplier,
            data_offset,
            total_size,
            entries,
            bytes,
        })
    }
}

fn parse_sfat_sfnt(
    bytes: &[u8],
    byte_order: ByteOrder,
    data_offset: u32,
    total_size: u32,
) -> Result<(Vec<Entry>, u32)> {
    let sfat_off = SARC_HEADER_SIZE as usize;
    if bytes.len() < sfat_off + SFAT_HEADER_SIZE as usize {
        return Err(Error::malformed("truncated SFAT header"));
    }
    if bytes[sfat_off..sfat_off + 4] != SFAT_MAGIC {
        return Err(Error::malformed("missing SFAT magic"));
    }
    let sfat_hdr_size = byte_order.read_u16(bytes, sfat_off + 4, "SFAT header size")?;
    if sfat_hdr_size != SFAT_HEADER_SIZE {
        return Err(Error::malformed("unexpected SFAT header size"));
    }
    let node_count = byte_order.read_u16(bytes, sfat_off + 6, "SFAT node count")? as usize;
    let hash_multiplier = byte_order.read_u32(bytes, sfat_off + 8, "SFAT hash multiplier")?;

    if node_count > MAX_FILES {
        return Err(Error::malformed(format!(
            "node count {node_count} exceeds maximum {MAX_FILES}"
        )));
    }

    let entries_off = sfat_off + SFAT_HEADER_SIZE as usize;
    let entries_end = entries_off + node_count * SFAT_ENTRY_SIZE;
    if bytes.len() < entries_end {
        return Err(Error::malformed("truncated SFAT entries"));
    }

    let names_hdr = entries_end;
    if bytes.len() < names_hdr + SFNT_HEADER_SIZE as usize {
        return Err(Error::malformed("truncated SFNT header"));
    }
    if bytes[names_hdr..names_hdr + 4] != SFNT_MAGIC {
        return Err(Error::malformed("missing SFNT magic"));
    }
    let names_hdr_size = byte_order.read_u16(bytes, names_hdr + 4, "SFNT header size")?;
    if names_hdr_size != SFNT_HEADER_SIZE {
        return Err(Error::malformed("unexpected SFNT header size"));
    }
    let names_off = names_hdr + SFNT_HEADER_SIZE as usize;
    if names_off > data_offset as usize {
        return Err(Error::malformed("SFNT runs past data offset"));
    }
    let name_table = &bytes[names_off..data_offset as usize];

    let data_section_size = total_size as usize - data_offset as usize;
    let mut entries = Vec::with_capacity(node_count);
    let mut prev_hash: Option<u32> = None;
    for i in 0..node_count {
        let e = entries_off + i * SFAT_ENTRY_SIZE;
        let hash = byte_order.read_u32(bytes, e, "SFAT entry hash")?;
        let attrs = byte_order.read_u32(bytes, e + 4, "SFAT entry attrs")?;
        let start = byte_order.read_u32(bytes, e + 8, "SFAT entry start")?;
        let end = byte_order.read_u32(bytes, e + 12, "SFAT entry end")?;

        if let Some(prev) = prev_hash
            && hash < prev
        {
            return Err(Error::malformed(format!(
                "SFAT entries are not sorted by hash (entry {i}: {hash:#010x} < {prev:#010x})"
            )));
        }
        prev_hash = Some(hash);

        let collision = ((attrs >> 24) & 0xFF) as u8;
        let name_off_field = attrs & 0x00FF_FFFF;
        let name = if collision == 0 && name_off_field == 0 {
            None
        } else {
            Some(read_cstr(name_table, (name_off_field as usize) * 4)?)
        };

        if (end as usize) > data_section_size {
            return Err(Error::malformed(format!(
                "entry {i} data_end {end:#x} exceeds data section size"
            )));
        }
        if start > end {
            return Err(Error::malformed(format!(
                "entry {i} has start {start:#x} > end {end:#x}"
            )));
        }

        entries.push(Entry {
            hash,
            collision,
            name,
            data_start: start,
            data_end: end,
        });
    }

    Ok((entries, hash_multiplier))
}

fn read_cstr(table: &[u8], off: usize) -> Result<String> {
    if off >= table.len() {
        return Err(Error::malformed("filename offset out of range"));
    }
    let bytes = &table[off..];
    let end = bytes
        .iter()
        .position(|&b| b == 0)
        .ok_or_else(|| Error::malformed("unterminated filename"))?;
    String::from_utf8(bytes[..end].to_vec()).map_err(|_| Error::invalid_utf8("SARC filename"))
}

#[must_use]
pub(crate) fn hash_name(name: &str, multiplier: u32) -> u32 {
    let mut h: u32 = 0;
    for &b in name.as_bytes() {
        let extended = i32::from(b.cast_signed()).cast_unsigned();
        h = h.wrapping_mul(multiplier).wrapping_add(extended);
    }
    h
}

/// A named file to be packed into an archive by [`write()`].
#[derive(Debug)]
pub struct PackEntry<'a> {
    pub name: &'a str,
    pub data: &'a [u8],
}

struct Slot<'a> {
    hash: u32,
    name_off: u32,
    data_start: u32,
    data_end: u32,
    file: &'a PackEntry<'a>,
}

struct Layout<'a> {
    slots: Vec<Slot<'a>>,
    name_table: Vec<u8>,
    data_offset: u32,
    total_size: u32,
}

impl<'a> Layout<'a> {
    fn build(files: &'a [PackEntry<'a>], alignment: u32) -> Result<Self> {
        for f in files {
            if f.name.is_empty() {
                return Err(Error::malformed("entry name is empty"));
            }
            if f.name.as_bytes().contains(&0) {
                return Err(Error::malformed(format!(
                    "entry name `{}` contains a NUL byte",
                    f.name.escape_debug()
                )));
            }
        }

        let multiplier = DEFAULT_HASH_MULTIPLIER;
        let mut slots: Vec<Slot<'a>> = files
            .iter()
            .map(|f| Slot {
                hash: hash_name(f.name, multiplier),
                name_off: 0,
                data_start: 0,
                data_end: 0,
                file: f,
            })
            .collect();
        slots.sort_by_key(|s| s.hash);

        for w in slots.windows(2) {
            if w[0].hash == w[1].hash {
                let msg = if w[0].file.name == w[1].file.name {
                    format!("duplicate entry name `{}`", w[0].file.name)
                } else {
                    format!(
                        "hash collision between `{}` and `{}` is not supported",
                        w[0].file.name, w[1].file.name
                    )
                };
                return Err(Error::malformed(msg));
            }
        }

        let mut name_table: Vec<u8> = Vec::new();
        for slot in &mut slots {
            let off = name_table.len();
            debug_assert!(off.is_multiple_of(4));
            let words =
                u32::try_from(off / 4).map_err(|_| Error::overflow("name offset exceeds u32"))?;
            if words > 0x00FF_FFFF {
                return Err(Error::overflow("name table exceeds 24-bit offset field"));
            }
            slot.name_off = words;
            name_table.extend_from_slice(slot.file.name.as_bytes());
            name_table.push(0);
            name_table.resize(name_table.len().next_multiple_of(4), 0);
        }

        let entry_count =
            u32::try_from(slots.len()).map_err(|_| Error::overflow("entry count exceeds u32"))?;
        let sfat_size = u32::from(SFAT_HEADER_SIZE) + entry_count * SFAT_ENTRY_SIZE_U32;
        let name_table_len = u32::try_from(name_table.len())
            .map_err(|_| Error::overflow("name table exceeds u32"))?;
        let names_size = u32::from(SFNT_HEADER_SIZE) + name_table_len;
        let data_offset =
            (u32::from(SARC_HEADER_SIZE) + sfat_size + names_size).next_multiple_of(alignment);

        let mut cursor: u32 = 0;
        for slot in &mut slots {
            cursor = cursor.next_multiple_of(alignment);
            let len = u32::try_from(slot.file.data.len()).map_err(|_| {
                Error::overflow(format!("file `{}` exceeds u32 size", slot.file.name))
            })?;
            slot.data_start = cursor;
            slot.data_end = cursor
                .checked_add(len)
                .ok_or_else(|| Error::overflow("data section overflow"))?;
            cursor = slot.data_end;
        }
        let total_size = data_offset
            .checked_add(cursor)
            .ok_or_else(|| Error::overflow("archive size overflow"))?;

        Ok(Self {
            slots,
            name_table,
            data_offset,
            total_size,
        })
    }
}

/// Writes a SARC archive containing `files`, aligning each file's data to
/// `alignment` (which must be a power of two), and returns the bytes written.
///
/// Entry names must be unique and free of NUL bytes; hash collisions are not
/// supported.
pub fn write<W: Write>(
    writer: &mut W,
    files: &[PackEntry<'_>],
    byte_order: ByteOrder,
    alignment: u32,
) -> Result<u64> {
    if files.len() > MAX_FILES {
        return Err(Error::overflow(format!(
            "too many files: {} > {MAX_FILES}",
            files.len()
        )));
    }
    if !alignment.is_power_of_two() {
        return Err(Error::malformed(format!(
            "alignment {alignment} is not a power of two"
        )));
    }

    let layout = Layout::build(files, alignment)?;
    let multiplier = DEFAULT_HASH_MULTIPLIER;

    let mut out: Vec<u8> = Vec::with_capacity(layout.total_size as usize);

    out.extend_from_slice(&SARC_MAGIC);
    byte_order.put_u16(&mut out, SARC_HEADER_SIZE);
    out.extend_from_slice(&byte_order.bom());
    byte_order.put_u32(&mut out, layout.total_size);
    byte_order.put_u32(&mut out, layout.data_offset);
    byte_order.put_u16(&mut out, SARC_VERSION);
    out.extend_from_slice(&[0, 0]);

    out.extend_from_slice(&SFAT_MAGIC);
    byte_order.put_u16(&mut out, SFAT_HEADER_SIZE);
    let entry_count_u16 = u16::try_from(layout.slots.len())
        .map_err(|_| Error::overflow("entry count exceeds u16"))?;
    byte_order.put_u16(&mut out, entry_count_u16);
    byte_order.put_u32(&mut out, multiplier);

    for slot in &layout.slots {
        let attrs: u32 = (1u32 << 24) | (slot.name_off & 0x00FF_FFFF);
        byte_order.put_u32(&mut out, slot.hash);
        byte_order.put_u32(&mut out, attrs);
        byte_order.put_u32(&mut out, slot.data_start);
        byte_order.put_u32(&mut out, slot.data_end);
    }

    out.extend_from_slice(&SFNT_MAGIC);
    byte_order.put_u16(&mut out, SFNT_HEADER_SIZE);
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&layout.name_table);

    for slot in &layout.slots {
        let target = layout.data_offset as usize + slot.data_start as usize;
        out.resize(target, 0);
        out.extend_from_slice(slot.file.data);
    }

    writer.write_all(&out)?;
    Ok(u64::from(layout.total_size))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn pack(files: &[PackEntry<'_>], order: ByteOrder, align: u32) -> Vec<u8> {
        let mut buf = Vec::new();
        write(&mut buf, files, order, align).unwrap();
        buf
    }

    #[test]
    fn hash_empty_is_zero() {
        assert_eq!(hash_name("", DEFAULT_HASH_MULTIPLIER), 0);
        assert_eq!(hash_name("", 1), 0);
        assert_eq!(hash_name("", 0xFFFF_FFFF), 0);
    }

    #[test]
    fn hash_single_byte_is_byte_value() {
        assert_eq!(hash_name("a", 0x65), 0x61);
        assert_eq!(hash_name("\x01", 0x65), 0x01);
    }

    #[test]
    fn hash_two_bytes_matches_formula() {
        let expected = 0x65u32
            .wrapping_mul(u32::from(b'a'))
            .wrapping_add(u32::from(b'b'));
        assert_eq!(hash_name("ab", 0x65), expected);
    }

    #[test]
    fn hash_sign_extends_high_bytes() {
        let h = hash_name("\u{0080}", 0x65);
        let step1 = 0u32.wrapping_mul(0x65).wrapping_add(0xFFFF_FFC2);
        let expected = step1.wrapping_mul(0x65).wrapping_add(0xFFFF_FF80);
        assert_eq!(h, expected);
    }

    #[test]
    fn hash_is_deterministic() {
        let a = hash_name("Pack/ZsDic.pack", DEFAULT_HASH_MULTIPLIER);
        let b = hash_name("Pack/ZsDic.pack", DEFAULT_HASH_MULTIPLIER);
        assert_eq!(a, b);
    }

    fn assert_round_trip(files: &[(&str, &[u8])], order: ByteOrder, align: u32) {
        let entries: Vec<PackEntry<'_>> = files
            .iter()
            .map(|(n, d)| PackEntry { name: n, data: d })
            .collect();
        let buf = pack(&entries, order, align);
        let sarc = Sarc::parse(buf).unwrap();
        assert_eq!(sarc.entries().len(), files.len());
        assert_eq!(sarc.byte_order(), order);
        assert_eq!(sarc.hash_multiplier(), DEFAULT_HASH_MULTIPLIER);
        assert!(
            sarc.data_offset().is_multiple_of(align),
            "data offset {:#x} is not aligned to {align}",
            sarc.data_offset()
        );

        let by_name: BTreeMap<String, Vec<u8>> = sarc
            .entries()
            .iter()
            .map(|e| (e.name.clone().unwrap(), sarc.data(e).to_vec()))
            .collect();
        for (name, data) in files {
            assert_eq!(by_name[*name], *data, "data mismatch for `{name}`");
        }
    }

    #[test]
    fn round_trip_basic_little_endian() {
        assert_round_trip(
            &[
                ("alpha.bin", b"hello world"),
                ("beta.bin", &[0u8; 32]),
                ("gamma/nested.bin", b"x"),
            ],
            ByteOrder::Little,
            4,
        );
    }

    #[test]
    fn round_trip_big_endian() {
        assert_round_trip(
            &[("a", b"1"), ("b", b"22"), ("c", b"333")],
            ByteOrder::Big,
            4,
        );
    }

    #[test]
    fn round_trip_higher_alignment() {
        assert_round_trip(
            &[
                ("first.bin", b"AAAA"),
                ("second.bin", &[0xFFu8; 17]),
                ("third.bin", &[0x42u8; 257]),
            ],
            ByteOrder::Little,
            0x100,
        );
    }

    #[test]
    fn round_trip_single_file() {
        assert_round_trip(&[("only.bin", b"solo")], ByteOrder::Little, 4);
    }

    #[test]
    fn round_trip_empty_file_payload() {
        let entries = [PackEntry {
            name: "empty.bin",
            data: &[],
        }];
        let buf = pack(&entries, ByteOrder::Little, 4);
        let sarc = Sarc::parse(buf).unwrap();
        assert_eq!(sarc.entries().len(), 1);
        let e = &sarc.entries()[0];
        assert!(e.is_empty());
        assert_eq!(e.len(), 0);
        assert_eq!(sarc.data(e), b"");
    }

    #[test]
    fn entries_are_sorted_by_hash() {
        let names = ["zulu", "alpha", "mike", "bravo", "echo"];
        let datas: Vec<Vec<u8>> = names.iter().map(|n| n.as_bytes().to_vec()).collect();
        let entries: Vec<PackEntry<'_>> = names
            .iter()
            .zip(&datas)
            .map(|(n, d)| PackEntry { name: n, data: d })
            .collect();
        let buf = pack(&entries, ByteOrder::Little, 4);
        let sarc = Sarc::parse(buf).unwrap();

        let hashes: Vec<u32> = sarc.entries().iter().map(|e| e.hash).collect();
        let mut sorted = hashes.clone();
        sorted.sort_unstable();
        assert_eq!(hashes, sorted, "entries must be sorted ascending by hash");

        for e in sarc.entries() {
            let name = e.name.as_deref().unwrap();
            assert_eq!(e.hash, hash_name(name, DEFAULT_HASH_MULTIPLIER));
            assert_eq!(sarc.data(e), name.as_bytes());
        }
    }

    #[test]
    fn header_fields_match_spec() {
        let buf = pack(
            &[PackEntry {
                name: "x.bin",
                data: &[1, 2, 3, 4],
            }],
            ByteOrder::Little,
            4,
        );
        assert_eq!(&buf[0..4], &SARC_MAGIC);
        assert_eq!(u16::from_le_bytes([buf[4], buf[5]]), SARC_HEADER_SIZE);
        assert_eq!(&buf[6..8], &[0xFF, 0xFE]);
        assert_eq!(u16::from_le_bytes([buf[16], buf[17]]), SARC_VERSION);
        let sfat_off = SARC_HEADER_SIZE as usize;
        assert_eq!(&buf[sfat_off..sfat_off + 4], &SFAT_MAGIC);
    }

    fn parse_err(bytes: Vec<u8>) -> String {
        match Sarc::parse(bytes) {
            Ok(ok) => panic!("expected parse error, got {ok:?}"),
            Err(e) => e.to_string(),
        }
    }

    fn one_entry_bytes() -> Vec<u8> {
        pack(
            &[PackEntry {
                name: "x",
                data: b"y",
            }],
            ByteOrder::Little,
            4,
        )
    }

    #[test]
    fn parse_rejects_too_short() {
        let msg = parse_err(vec![0u8; 4]);
        assert!(msg.contains("too short"), "msg={msg}");
    }

    #[test]
    fn parse_rejects_bad_magic() {
        let mut bytes = one_entry_bytes();
        bytes[0..4].copy_from_slice(b"XXXX");
        let msg = parse_err(bytes);
        assert!(msg.contains("bad magic"), "msg={msg}");
    }

    #[test]
    fn parse_rejects_invalid_bom() {
        let mut bytes = one_entry_bytes();
        bytes[6] = 0xAB;
        bytes[7] = 0xCD;
        let msg = parse_err(bytes);
        assert!(msg.contains("BOM"), "msg={msg}");
    }

    #[test]
    fn parse_rejects_size_mismatch() {
        let mut bytes = one_entry_bytes();
        bytes.pop();
        let msg = parse_err(bytes);
        assert!(msg.contains("size"), "msg={msg}");
    }

    #[test]
    fn parse_rejects_bad_header_size() {
        let mut bytes = one_entry_bytes();
        bytes[4] = 0x10;
        let msg = parse_err(bytes);
        assert!(msg.contains("SARC header size"), "msg={msg}");
    }

    #[test]
    fn parse_rejects_data_offset_past_total_size() {
        let mut bytes = one_entry_bytes();
        let bogus = u32::try_from(bytes.len())
            .unwrap()
            .wrapping_add(1)
            .to_le_bytes();
        bytes[12..16].copy_from_slice(&bogus);
        let msg = parse_err(bytes);
        assert!(msg.contains("data offset"), "msg={msg}");
    }

    #[test]
    fn parse_rejects_unsorted_hashes() {
        let mut bytes = pack(
            &[
                PackEntry {
                    name: "a",
                    data: b"1",
                },
                PackEntry {
                    name: "b",
                    data: b"2",
                },
            ],
            ByteOrder::Little,
            4,
        );
        let entries_off = SARC_HEADER_SIZE as usize + SFAT_HEADER_SIZE as usize;
        let (left, right) = bytes.split_at_mut(entries_off + SFAT_ENTRY_SIZE);
        left[entries_off..].swap_with_slice(&mut right[..SFAT_ENTRY_SIZE]);
        let msg = parse_err(bytes);
        assert!(msg.contains("not sorted by hash"), "msg={msg}");
    }

    #[test]
    fn parse_rejects_missing_sfat_magic() {
        let mut bytes = one_entry_bytes();
        bytes[SARC_HEADER_SIZE as usize] = b'Z';
        let msg = parse_err(bytes);
        assert!(msg.contains("SFAT"), "msg={msg}");
    }

    #[test]
    fn write_rejects_non_power_of_two_alignment() {
        let mut buf = Vec::new();
        let err = write(
            &mut buf,
            &[PackEntry {
                name: "x",
                data: b"y",
            }],
            ByteOrder::Little,
            3,
        )
        .unwrap_err();
        let Error::Malformed(msg) = err else {
            panic!("expected Format error");
        };
        assert!(msg.contains("power of two"), "msg={msg}");
    }

    #[test]
    fn write_rejects_duplicate_names() {
        let mut buf = Vec::new();
        let err = write(
            &mut buf,
            &[
                PackEntry {
                    name: "dup",
                    data: b"1",
                },
                PackEntry {
                    name: "dup",
                    data: b"2",
                },
            ],
            ByteOrder::Little,
            4,
        )
        .unwrap_err();
        let Error::Malformed(msg) = err else {
            panic!("expected Format error");
        };
        assert!(msg.contains("duplicate"), "msg={msg}");
    }

    #[test]
    fn write_rejects_nul_in_name() {
        let mut buf = Vec::new();
        let err = write(
            &mut buf,
            &[PackEntry {
                name: "foo\0bar",
                data: b"x",
            }],
            ByteOrder::Little,
            4,
        )
        .unwrap_err();
        let Error::Malformed(msg) = err else {
            panic!("expected Format error");
        };
        assert!(msg.contains("NUL"), "msg={msg}");
    }

    #[test]
    fn write_rejects_empty_name() {
        let mut buf = Vec::new();
        let err = write(
            &mut buf,
            &[PackEntry {
                name: "",
                data: b"x",
            }],
            ByteOrder::Little,
            4,
        )
        .unwrap_err();
        let Error::Malformed(msg) = err else {
            panic!("expected Format error");
        };
        assert!(msg.contains("empty"), "msg={msg}");
    }

    #[test]
    fn entry_helpers() {
        let buf = pack(
            &[
                PackEntry {
                    name: "a",
                    data: b"",
                },
                PackEntry {
                    name: "b",
                    data: b"xy",
                },
            ],
            ByteOrder::Little,
            4,
        );
        let sarc = Sarc::parse(buf).unwrap();
        let by_name: BTreeMap<_, _> = sarc
            .entries()
            .iter()
            .map(|e| (e.name.clone().unwrap(), e))
            .collect();
        assert!(by_name["a"].is_empty());
        assert_eq!(by_name["a"].len(), 0);
        assert!(!by_name["b"].is_empty());
        assert_eq!(by_name["b"].len(), 2);
    }
}
