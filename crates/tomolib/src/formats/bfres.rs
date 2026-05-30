pub mod model;

use crate::formats::binio::{ByteOrder, align_up};
use crate::{Error, Result};

pub use model::{
    Attribute, AttributeFormat, IndexFormat, Mesh, Model, PrimitiveType, Shape, SubMesh,
    VertexBuffer,
};

pub const BFRES_MAGIC: [u8; 4] = *b"FRES";

const HOLDS_EXTERNAL_STRINGS: u8 = 0x1;
const HAS_EXTERNAL_GPU: u8 = 0x2;
const MEMORY_POOL_SIZE: u32 = 288;
const MAX_DATA_ALIGN: usize = 1 << 24;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedFile {
    pub name: String,
    pub offset: u32,
    pub size: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubfileGroup {
    pub names: Vec<String>,
    pub values_offset: u32,
    pub dict_offset: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bfres {
    pub byte_order: ByteOrder,
    pub version: u32,
    pub alignment_exp: u8,
    pub target_address_size: u8,
    pub flag: u16,
    pub block_offset: u16,
    pub external_flag: u8,
    pub reserve_flag: u8,
    pub name: String,
    pub file_size: u32,
    pub rlt_offset: u32,
    pub string_pool_offset: u32,
    pub string_pool_size: u32,
    pub buffer_info_offset: u32,
    pub memory_pool_offset: u32,
    pub models: SubfileGroup,
    pub skeletal_anims: SubfileGroup,
    pub material_anims: SubfileGroup,
    pub bone_visibility_anims: SubfileGroup,
    pub shape_anims: SubfileGroup,
    pub scene_anims: SubfileGroup,
    pub embedded_files: Vec<EmbeddedFile>,
    pub embedded_values_offset: u32,
    raw: Vec<u8>,
}

impl Bfres {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes);

        if r.array_at::<4>(0, "FRES magic")? != BFRES_MAGIC {
            return Err(Error::bad_magic("FRES"));
        }

        let bom = r.array_at::<2>(0x0C, "FRES BOM")?;
        let byte_order = match bom {
            [0xFF, 0xFE] => ByteOrder::Little,
            [0xFE, 0xFF] => ByteOrder::Big,
            _ => return Err(Error::malformed("FRES: invalid byte-order mark")),
        };
        r.order = byte_order;

        let version = r.u32_at(0x08)?;
        let version_major = (version >> 16) & 0xFFFF;
        let alignment_exp = r.byte(0x0E)?;
        if alignment_exp >= 64 {
            return Err(Error::malformed("FRES: implausible alignment exponent"));
        }
        let target_address_size = r.byte(0x0F)?;
        let flag = r.u16_at(0x14)?;
        let block_offset = r.u16_at(0x16)?;
        let rlt_offset = r.u32_at(0x18)?;
        let file_size = r.u32_at(0x1C)?;

        let mut pos = 0x20usize;
        let name_off = r.off(pos)?;
        pos += 8;
        let name = r.string_at(name_off)?;

        let model_values = r.off(pos)?;
        pos += 8;
        let model_dict = r.off(pos)?;
        pos += 8;
        if version_major >= 9 {
            pos += 32;
        }

        let [
            skeletal_anims,
            material_anims,
            bone_visibility_anims,
            shape_anims,
            scene_anims,
        ] = read_anim_groups(&r, &mut pos)?;

        let memory_pool_offset = r.off(pos)?;
        pos += 8;
        let buffer_info_offset = r.off(pos)?;
        pos += 8;

        let external_flag = r.byte(0xEE).unwrap_or(0);
        if version_major >= 10 && external_flag & HOLDS_EXTERNAL_STRINGS != 0 {
            return Err(Error::unsupported(
                "FRES: files with external string tables are not supported",
            ));
        }

        let external_values = r.off(pos)?;
        pos += 8;
        let external_dict = r.off(pos)?;
        pos += 8;
        pos += 8;
        let string_pool_offset = r.off(pos)?;
        pos += 8;
        let string_pool_size = r.u32_at(pos)?;
        pos += 4;
        pos += 2;
        if version_major >= 9 {
            pos += 4;
        }
        pos += 10;
        let num_external = r.u16_at(pos)? as usize;
        let reserve_flag = r.byte(0xEF).unwrap_or(0);

        let models = SubfileGroup {
            names: read_dict_keys(&r, model_dict)?,
            values_offset: u32::try_from(model_values).unwrap_or(0),
            dict_offset: u32::try_from(model_dict).unwrap_or(0),
        };

        let embedded_files = read_embedded_files(&r, external_dict, external_values, num_external)?;

        Ok(Self {
            byte_order,
            version,
            alignment_exp,
            target_address_size,
            flag,
            block_offset,
            external_flag,
            reserve_flag,
            name,
            file_size,
            rlt_offset,
            string_pool_offset: u32::try_from(string_pool_offset).unwrap_or(0),
            string_pool_size,
            buffer_info_offset: u32::try_from(buffer_info_offset).unwrap_or(0),
            memory_pool_offset: u32::try_from(memory_pool_offset).unwrap_or(0),
            models,
            skeletal_anims,
            material_anims,
            bone_visibility_anims,
            shape_anims,
            scene_anims,
            embedded_files,
            embedded_values_offset: u32::try_from(external_values).unwrap_or(0),
            raw: bytes.to_vec(),
        })
    }

    #[must_use]
    pub fn raw(&self) -> &[u8] {
        &self.raw
    }

    #[must_use]
    pub fn version_tuple(&self) -> (u16, u8, u8, u8) {
        (
            ((self.version >> 16) & 0xFFFF) as u16,
            ((self.version >> 8) & 0xFF) as u8,
            (self.version & 0xFF) as u8,
            ((self.version >> 24) & 0xFF) as u8,
        )
    }

    #[must_use]
    pub fn alignment(&self) -> usize {
        1usize << self.alignment_exp
    }

    #[must_use]
    pub fn version_major(&self) -> u32 {
        (self.version >> 16) & 0xFFFF
    }

    pub(crate) fn buffer_block_offset(&self) -> Result<u64> {
        if self.has_external_gpu() {
            return Ok(u64::from(self.file_size) + u64::from(MEMORY_POOL_SIZE));
        }
        if self.buffer_info_offset == 0 {
            return Ok(0);
        }
        let r = Reader {
            bytes: &self.raw,
            order: self.byte_order,
        };
        Ok(r.u64_at(self.buffer_info_offset as usize + 8)? & 0xFFFF_FFFF)
    }

    #[must_use]
    pub fn has_external_gpu(&self) -> bool {
        self.version_major() >= 10 && self.external_flag & HAS_EXTERNAL_GPU != 0
    }

    #[must_use]
    pub fn embedded_data(&self, file: &EmbeddedFile) -> Option<&[u8]> {
        let start = file.offset as usize;
        let end = start.checked_add(file.size as usize)?;
        self.raw.get(start..end)
    }

    pub fn write(&self) -> Result<Vec<u8>> {
        Ok(self.raw.clone())
    }

    fn external_section(&self) -> Result<(usize, usize)> {
        let r = Reader {
            bytes: &self.raw,
            order: self.byte_order,
        };
        let rlt = self.rlt_offset as usize;
        if r.array_at::<4>(rlt, "_RLT magic")? != *b"_RLT" {
            return Err(Error::malformed(
                "FRES: relocation table missing _RLT magic",
            ));
        }
        let count = r.u32_at(rlt + 8)? as usize;
        if count == 0 || count > 64 {
            return Err(Error::malformed("FRES: implausible _RLT section count"));
        }
        let desc = rlt + 16 + (count - 1) * 24;
        if desc.checked_add(24).is_none_or(|e| e > self.raw.len()) {
            return Err(Error::malformed("FRES: _RLT descriptor past end of file"));
        }
        let ext_start = r.u32_at(desc + 8)? as usize;
        Ok((ext_start, desc))
    }

    pub fn rebuild_with_embedded(&self, new_data: &[Option<Vec<u8>>]) -> Result<Vec<u8>> {
        let n = self.embedded_files.len();
        if new_data.len() != n {
            return Err(Error::malformed(
                "FRES: embedded replacement count mismatch",
            ));
        }
        let (ext_start, desc) = self.external_section()?;
        let rlt_pos = self.rlt_offset as usize;
        if ext_start > rlt_pos || rlt_pos > self.raw.len() {
            return Err(Error::malformed("FRES: external block past _RLT"));
        }
        let values_end = (self.embedded_values_offset as usize).saturating_add(n * 16);
        if ext_start < values_end {
            return Err(Error::malformed(
                "FRES: external block overlaps embedded values table",
            ));
        }
        let data_align = self.alignment().max(1);
        if data_align > MAX_DATA_ALIGN {
            return Err(Error::malformed("FRES: implausible data alignment"));
        }

        let mut finals: Vec<&[u8]> = Vec::with_capacity(n);
        for (i, repl) in new_data.iter().enumerate() {
            let d: &[u8] = match repl {
                Some(v) => v,
                None => self.embedded_data(&self.embedded_files[i]).unwrap_or(&[]),
            };
            finals.push(d);
        }

        let mut new_offsets = vec![0u64; n];
        let mut placed: Vec<(usize, usize)> = Vec::new();
        let mut cursor = ext_start;
        for (i, d) in finals.iter().enumerate() {
            if d.is_empty() {
                continue;
            }
            let start = align_up(cursor, data_align);
            new_offsets[i] = start as u64;
            placed.push((start, i));
            cursor = start + d.len();
        }
        let new_rlt = align_up(cursor, 8).max(ext_start);
        if new_rlt > u32::MAX as usize {
            return Err(Error::overflow("FRES: rebuilt file exceeds 4 GiB"));
        }

        let mut out = self.raw[..ext_start].to_vec();
        out.resize(new_rlt, 0);
        for &(start, i) in &placed {
            out[start..start + finals[i].len()].copy_from_slice(finals[i]);
        }
        out.extend_from_slice(&self.raw[rlt_pos..]);

        let total = u32::try_from(out.len())
            .map_err(|_| Error::overflow("FRES: rebuilt file exceeds 4 GiB"))?;
        let new_rlt_u32 =
            u32::try_from(new_rlt).map_err(|_| Error::overflow("FRES: _RLT past 4 GiB"))?;
        let order = self.byte_order;
        order.write_u32_at(&mut out, 0x18, new_rlt_u32);
        order.write_u32_at(&mut out, 0x1C, total);
        order.write_u32_at(&mut out, new_rlt + 4, new_rlt_u32);
        let new_desc = new_rlt + (desc - rlt_pos);
        let sec_size = u32::try_from(cursor - ext_start)
            .map_err(|_| Error::overflow("FRES: external section too large"))?;
        order.write_u32_at(&mut out, new_desc + 12, sec_size);

        let values = self.embedded_values_offset as usize;
        for (i, d) in finals.iter().enumerate() {
            let entry = values + i * 16;
            if entry + 16 <= out.len() {
                order.write_u64_at(&mut out, entry, new_offsets[i]);
                order.write_u64_at(&mut out, entry + 8, d.len() as u64);
            }
        }

        Self::parse(&out)?;
        Ok(out)
    }
}

fn read_anim_groups(r: &Reader, pos: &mut usize) -> Result<[SubfileGroup; 5]> {
    let mut out: [SubfileGroup; 5] = Default::default();
    for g in &mut out {
        let values_offset = r.off(*pos)?;
        *pos += 8;
        let dict_offset = r.off(*pos)?;
        *pos += 8;
        *g = SubfileGroup {
            names: read_dict_keys(r, dict_offset)?,
            values_offset: u32::try_from(values_offset).unwrap_or(0),
            dict_offset: u32::try_from(dict_offset).unwrap_or(0),
        };
    }
    Ok(out)
}

fn read_dict_keys(r: &Reader, dict_off: usize) -> Result<Vec<String>> {
    if dict_off == 0 {
        return Ok(Vec::new());
    }
    let count = usize::try_from(r.i32_at(dict_off + 4)?)
        .map_err(|_| Error::malformed("FRES: negative dict count"))?;
    if count > r.bytes.len() / 16 {
        return Err(Error::out_of_range("FRES dict count", count, r.bytes.len()));
    }
    let mut names = Vec::with_capacity(count);
    for i in 1..=count {
        let node = dict_off + 8 + i * 16;
        let key_off = r.off(node + 8)?;
        names.push(r.string_at(key_off)?);
    }
    Ok(names)
}

fn read_embedded_files(
    r: &Reader,
    dict_off: usize,
    values_off: usize,
    count: usize,
) -> Result<Vec<EmbeddedFile>> {
    if count == 0 || values_off == 0 {
        return Ok(Vec::new());
    }
    let names = read_dict_keys(r, dict_off)?;
    let mut files = Vec::with_capacity(count);
    for i in 0..count {
        let entry = values_off + i * 16;
        let offset = r.off(entry)?;
        let size = r.off(entry + 8)?;
        let name = names.get(i).cloned().unwrap_or_default();
        files.push(EmbeddedFile {
            name,
            offset: u32::try_from(offset).unwrap_or(0),
            size: u32::try_from(size).unwrap_or(0),
        });
    }
    Ok(files)
}

pub(crate) struct Reader<'a> {
    pub bytes: &'a [u8],
    pub order: ByteOrder,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            order: ByteOrder::Little,
        }
    }

    pub(crate) fn byte(&self, off: usize) -> Result<u8> {
        self.bytes
            .get(off)
            .copied()
            .ok_or_else(|| Error::truncated("FRES byte", off, 1, 0))
    }

    pub(crate) fn array_at<const N: usize>(
        &self,
        off: usize,
        ctx: &'static str,
    ) -> Result<[u8; N]> {
        crate::formats::binio::read_array::<N>(self.bytes, off, ctx)
    }

    pub(crate) fn u16_at(&self, off: usize) -> Result<u16> {
        self.order.read_u16(self.bytes, off, "FRES u16")
    }

    pub(crate) fn u32_at(&self, off: usize) -> Result<u32> {
        self.order.read_u32(self.bytes, off, "FRES u32")
    }

    pub(crate) fn i32_at(&self, off: usize) -> Result<i32> {
        Ok(self.u32_at(off)?.cast_signed())
    }

    pub(crate) fn u64_at(&self, off: usize) -> Result<u64> {
        self.order.read_u64(self.bytes, off, "FRES u64")
    }

    pub(crate) fn u16_be_at(&self, off: usize) -> Result<u16> {
        ByteOrder::Big.read_u16(self.bytes, off, "FRES u16 BE")
    }

    pub(crate) fn u32_be_at(&self, off: usize) -> Result<u32> {
        ByteOrder::Big.read_u32(self.bytes, off, "FRES u32 BE")
    }

    pub(crate) fn off(&self, off: usize) -> Result<usize> {
        usize::try_from(self.u64_at(off)? & 0xFFFF_FFFF)
            .map_err(|_| Error::overflow("FRES: offset exceeds addressable range"))
    }

    pub(crate) fn slice(&self, off: usize, len: usize, ctx: &'static str) -> Result<&'a [u8]> {
        let end = off
            .checked_add(len)
            .filter(|&e| e <= self.bytes.len())
            .ok_or_else(|| Error::truncated(ctx, off, len, self.bytes.len().saturating_sub(off)))?;
        Ok(&self.bytes[off..end])
    }

    pub(crate) fn string_at(&self, off: usize) -> Result<String> {
        if off == 0 || off + 2 > self.bytes.len() {
            return Ok(String::new());
        }
        let len = self.u16_at(off)? as usize;
        let s = self.slice(off + 2, len, "FRES string")?;
        Ok(String::from_utf8_lossy(s).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_v10() -> Vec<u8> {
        let mut b = vec![0u8; 0x100];
        b[0..4].copy_from_slice(b"FRES");
        b[4..8].copy_from_slice(&[0x20, 0x20, 0x20, 0x20]);
        b[8..12].copy_from_slice(&0x000A_0202u32.to_le_bytes());
        b[0x0C] = 0xFF;
        b[0x0D] = 0xFE;
        b[0x0E] = 0x0C;
        let len = u32::try_from(b.len()).unwrap();
        b[0x1C..0x20].copy_from_slice(&len.to_le_bytes());
        b
    }

    #[test]
    fn rejects_bad_magic() {
        let err = Bfres::parse(b"NOPE........").unwrap_err();
        assert!(matches!(err, Error::BadMagic { .. }));
    }

    #[test]
    fn rejects_short() {
        assert!(Bfres::parse(b"FRES").is_err());
    }

    #[test]
    fn parses_minimal_header() {
        let bytes = minimal_v10();
        let bfres = Bfres::parse(&bytes).expect("parse minimal");
        assert_eq!(bfres.byte_order, ByteOrder::Little);
        assert_eq!(bfres.version_major(), 10);
        assert_eq!(bfres.version_tuple(), (10, 2, 2, 0));
        assert_eq!(bfres.alignment(), 0x1000);
        assert!(bfres.models.names.is_empty());
        assert!(bfres.embedded_files.is_empty());
        assert_eq!(bfres.name, "");
    }

    #[test]
    fn write_is_byte_identical() {
        let bytes = minimal_v10();
        let bfres = Bfres::parse(&bytes).expect("parse");
        assert_eq!(bfres.write().expect("write"), bytes);
    }

    #[test]
    fn rejects_invalid_bom() {
        let mut bytes = minimal_v10();
        bytes[0x0C] = 0x12;
        bytes[0x0D] = 0x34;
        assert!(Bfres::parse(&bytes).is_err());
    }

    fn le32(b: &mut [u8], off: usize, v: u32) {
        b[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn le64(b: &mut [u8], off: usize, v: u64) {
        b[off..off + 8].copy_from_slice(&v.to_le_bytes());
    }
    fn u32o(x: usize) -> u32 {
        u32::try_from(x).unwrap()
    }
    fn push_str(b: &mut Vec<u8>, s: &str) {
        b.extend_from_slice(&u16::try_from(s.len()).unwrap().to_le_bytes());
        b.extend_from_slice(s.as_bytes());
        b.push(0);
        if !b.len().is_multiple_of(2) {
            b.push(0);
        }
    }
    fn pad(b: &mut Vec<u8>, a: usize) {
        while !b.len().is_multiple_of(a) {
            b.push(0);
        }
    }

    fn fixture(data: &[u8]) -> Vec<u8> {
        let mut b = vec![0u8; 0x100];
        let name_off = b.len();
        push_str(&mut b, "fix");
        let ext_name_off = b.len();
        push_str(&mut b, "tex.bntx");

        pad(&mut b, 8);
        let dict_off = b.len();
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&1i32.to_le_bytes());
        b.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&0u64.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&(ext_name_off as u64).to_le_bytes());

        pad(&mut b, 8);
        let values_off = b.len();
        b.extend_from_slice(&0u64.to_le_bytes());
        b.extend_from_slice(&(data.len() as u64).to_le_bytes());

        pad(&mut b, 8);
        let data_off = b.len();
        b.extend_from_slice(data);
        le64(&mut b, values_off, data_off as u64);

        pad(&mut b, 8);
        let rlt = b.len();
        b.extend_from_slice(b"_RLT");
        b.extend_from_slice(&u32o(rlt).to_le_bytes());
        b.extend_from_slice(&6u32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        for k in 0..6 {
            b.extend_from_slice(&0u64.to_le_bytes());
            if k == 5 {
                b.extend_from_slice(&u32o(data_off).to_le_bytes());
                b.extend_from_slice(&u32o(data.len()).to_le_bytes());
            } else {
                b.extend_from_slice(&0u32.to_le_bytes());
                b.extend_from_slice(&0u32.to_le_bytes());
            }
            b.extend_from_slice(&0u32.to_le_bytes());
            b.extend_from_slice(&0u32.to_le_bytes());
        }
        let total = b.len();

        b[0..4].copy_from_slice(b"FRES");
        b[4..8].copy_from_slice(b"    ");
        le32(&mut b, 0x08, 0x000A_0202);
        b[0x0C] = 0xFF;
        b[0x0D] = 0xFE;
        b[0x0E] = 0x03;
        le32(&mut b, 0x18, u32o(rlt));
        le32(&mut b, 0x1C, u32o(total));
        le64(&mut b, 0x20, name_off as u64);
        le64(&mut b, 0xB8, values_off as u64);
        le64(&mut b, 0xC0, dict_off as u64);
        le64(&mut b, 0xD0, name_off as u64);
        le32(&mut b, 0xD8, 16);
        b[0xEC] = 1;
        b
    }

    #[test]
    fn fixture_parses_embedded() {
        let data = b"BNTX\x00\x01\x02\x03payload-bytes!!";
        let bytes = fixture(data);
        let bfres = Bfres::parse(&bytes).expect("parse fixture");
        assert_eq!(bfres.embedded_files.len(), 1);
        assert_eq!(bfres.embedded_files[0].name, "tex.bntx");
        assert_eq!(
            bfres.embedded_data(&bfres.embedded_files[0]),
            Some(&data[..])
        );
    }

    #[test]
    fn rebuild_noop_is_byte_identical() {
        let data = b"BNTX-original-content".to_vec();
        let bytes = fixture(&data);
        let bfres = Bfres::parse(&bytes).unwrap();
        let rebuilt = bfres.rebuild_with_embedded(&[None]).unwrap();
        assert_eq!(rebuilt, bytes);
    }

    #[test]
    fn resize_grow_then_shrink_round_trips() {
        let data = b"BNTX-small".to_vec();
        let bytes = fixture(&data);
        let bfres = Bfres::parse(&bytes).unwrap();

        let bigger = b"BNTX-a-much-larger-replacement-texture-blob".to_vec();
        let grown = bfres
            .rebuild_with_embedded(&[Some(bigger.clone())])
            .unwrap();
        let gp = Bfres::parse(&grown).expect("grown reparses");
        assert_eq!(gp.embedded_files[0].size as usize, bigger.len());
        assert_eq!(gp.embedded_data(&gp.embedded_files[0]), Some(&bigger[..]));
        assert_eq!(gp.file_size as usize, grown.len());

        let shrunk = gp.rebuild_with_embedded(&[Some(data.clone())]).unwrap();
        assert_eq!(shrunk, bytes, "shrink back to original size is exact");
    }

    #[test]
    fn resize_smaller_reparses() {
        let data = b"BNTX-this-is-the-bigger-original-payload".to_vec();
        let bytes = fixture(&data);
        let bfres = Bfres::parse(&bytes).unwrap();
        let smaller = b"BNTXtiny".to_vec();
        let out = bfres
            .rebuild_with_embedded(&[Some(smaller.clone())])
            .unwrap();
        let p = Bfres::parse(&out).unwrap();
        assert_eq!(p.embedded_data(&p.embedded_files[0]), Some(&smaller[..]));
        assert!(out.len() < bytes.len());
    }
}
