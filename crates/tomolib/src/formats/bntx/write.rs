use std::collections::{HashMap, HashSet};

use crate::formats::binio::{ByteOrder, align_up};
use crate::formats::bntx::{Bntx, Texture, dict};
use crate::{Error, Result};

const MEMORY_POOL_SIZE: usize = 0x140;
const RUNTIME_SCRATCH: usize = 0x100;

struct Ptr {
    pos: usize,
    in_data: bool,
}

struct Writer {
    bytes: Vec<u8>,
    order: ByteOrder,
    ptrs: Vec<Ptr>,
}

impl Writer {
    fn new(order: ByteOrder) -> Self {
        Self {
            bytes: Vec::new(),
            order,
            ptrs: Vec::new(),
        }
    }

    fn pos(&self) -> usize {
        self.bytes.len()
    }
    fn u8(&mut self, v: u8) {
        self.bytes.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.order.put_u16(&mut self.bytes, v);
    }
    fn u32(&mut self, v: u32) {
        self.order.put_u32(&mut self.bytes, v);
    }
    fn u64(&mut self, v: u64) {
        self.order.put_u64(&mut self.bytes, v);
    }
    fn raw(&mut self, v: &[u8]) {
        self.bytes.extend_from_slice(v);
    }
    fn zeros(&mut self, n: usize) {
        self.bytes.resize(self.bytes.len() + n, 0);
    }
    fn pad_to(&mut self, pos: usize) {
        if pos > self.bytes.len() {
            self.zeros(pos - self.bytes.len());
        }
    }
    fn align(&mut self, a: usize) {
        self.pad_to(align_up(self.bytes.len(), a));
    }

    fn ptr(&mut self, value: u64, in_data: bool) {
        self.ptrs.push(Ptr {
            pos: self.pos(),
            in_data,
        });
        self.u64(value);
    }
    fn reserve_ptr(&mut self, in_data: bool) -> usize {
        let p = self.pos();
        self.ptr(0, in_data);
        p
    }

    fn patch_u16(&mut self, at: usize, v: u16) {
        self.order.write_u16_at(&mut self.bytes, at, v);
    }
    fn patch_u32(&mut self, at: usize, v: u32) {
        self.order.write_u32_at(&mut self.bytes, at, v);
    }
    fn patch_u64(&mut self, at: usize, v: u64) {
        self.order.write_u64_at(&mut self.bytes, at, v);
    }
    fn patch_off32(&mut self, at: usize, off: usize) -> Result<()> {
        self.patch_u32(at, off32(off)?);
        Ok(())
    }
}

#[inline]
fn off32(n: usize) -> Result<u32> {
    u32::try_from(n).map_err(|_| Error::overflow("BNTX: file offset exceeds 4 GiB"))
}

struct Heads {
    name_offset_at: usize,
    block_offset_at: usize,
    rlt_offset_at: usize,
    file_size_at: usize,
    data_ptr_at: usize,
    dict_ptr_at: usize,
    info_array: usize,
}

pub(crate) fn write(bntx: &Bntx) -> Result<Vec<u8>> {
    if bntx.textures.iter().any(|t| !t.user_data.is_empty()) {
        return Err(Error::unsupported(
            "BNTX write: textures with user data are not supported yet",
        ));
    }
    let n = bntx.textures.len();
    let mut w = Writer::new(bntx.byte_order);

    let heads = write_prologue(&mut w, bntx)?;
    let (str_off, str_block) =
        write_strings_and_dict(&mut w, bntx, heads.name_offset_at, heads.dict_ptr_at)?;

    let mut brti_pos = Vec::with_capacity(n);
    let mut miptable_pos = Vec::with_capacity(n);
    for (i, tex) in bntx.textures.iter().enumerate() {
        let start = w.pos();
        brti_pos.push(start);
        w.patch_u64(heads.info_array + i * 8, start as u64);
        write_brti(&mut w, tex, &str_off, start);
        w.zeros(RUNTIME_SCRATCH * 2);
        miptable_pos.push(w.pos());
        for _ in 0..tex.mip_offsets.len().max(1) {
            w.reserve_ptr(true);
        }
    }
    let info_end = w.pos();

    let align = bntx.alignment().max(1);
    let data_start = align_up(info_end + 0x10, align);
    let brtd = data_start - 0x10;
    w.pad_to(brtd);
    w.patch_u64(heads.data_ptr_at, brtd as u64);
    w.ptrs.push(Ptr {
        pos: heads.data_ptr_at,
        in_data: true,
    });
    w.raw(b"BRTD");
    w.u32(0);
    let brtd_size_at = w.pos();
    w.u64(0);
    for (i, tex) in bntx.textures.iter().enumerate() {
        let tex_data = w.pos();
        for (j, &rel) in mip_offsets(tex).iter().enumerate() {
            w.patch_u64(miptable_pos[i] + j * 8, (tex_data as u64) + rel);
        }
        w.raw(&tex.image_data);
    }

    w.align(align);
    let rlt = w.pos();
    w.patch_off32(heads.rlt_offset_at, rlt)?;
    w.patch_u64(brtd_size_at, (rlt - brtd) as u64);
    write_rlt(&mut w, rlt, info_end, brtd);

    finish_block_chain(&mut w, &brti_pos, brtd, heads.block_offset_at, str_block)?;

    let total = w.pos();
    w.patch_off32(heads.file_size_at, total)?;
    Ok(w.bytes)
}

fn write_prologue(w: &mut Writer, bntx: &Bntx) -> Result<Heads> {
    w.raw(b"BNTX");
    w.u32(0);
    w.u8(bntx.version.2);
    w.u8(bntx.version.1);
    w.u16(bntx.version.0);
    w.u16(0xFEFF);
    w.u8(bntx.alignment_log2);
    w.u8(bntx.target_address_size);
    let name_offset_at = w.pos();
    w.u32(0);
    w.u16(bntx.flag);
    let block_offset_at = w.pos();
    w.u16(0);
    let rlt_offset_at = w.pos();
    w.u32(0);
    let file_size_at = w.pos();
    w.u32(0);

    w.raw(&bntx.platform.magic());
    w.u32(
        u32::try_from(bntx.textures.len())
            .map_err(|_| Error::overflow("BNTX: too many textures"))?,
    );
    let info_ptr_at = w.pos();
    w.u64(0);
    let data_ptr_at = w.pos();
    w.u64(0);
    let dict_ptr_at = w.pos();
    w.u64(0);
    let mempool_ptr_at = w.pos();
    w.u64(0);
    w.u64(0);
    w.u64(0);

    let mempool = w.pos();
    w.patch_u64(mempool_ptr_at, mempool as u64);
    w.ptrs.push(Ptr {
        pos: mempool_ptr_at,
        in_data: false,
    });
    w.zeros(MEMORY_POOL_SIZE);

    w.align(8);
    let info_array = w.pos();
    w.patch_u64(info_ptr_at, info_array as u64);
    w.ptrs.push(Ptr {
        pos: info_ptr_at,
        in_data: false,
    });
    for _ in 0..bntx.textures.len() {
        w.reserve_ptr(false);
    }

    Ok(Heads {
        name_offset_at,
        block_offset_at,
        rlt_offset_at,
        file_size_at,
        data_ptr_at,
        dict_ptr_at,
        info_array,
    })
}

#[derive(Clone, Copy)]
struct StrBlock {
    block: usize,
    next_at: usize,
    size_at: usize,
}

fn write_strings_and_dict<'a>(
    w: &mut Writer,
    bntx: &'a Bntx,
    name_offset_at: usize,
    dict_ptr_at: usize,
) -> Result<(HashMap<&'a str, usize>, StrBlock)> {
    let mut strings: Vec<&str> = vec![""];
    let mut seen: HashSet<&str> = HashSet::from([""]);
    for name in bntx
        .textures
        .iter()
        .map(|t| t.name.as_str())
        .chain(std::iter::once(bntx.name.as_str()))
    {
        if seen.insert(name) {
            strings.push(name);
        }
    }

    w.align(4);
    let str_block = w.pos();
    w.raw(b"_STR");
    let str_next_at = w.pos();
    w.u32(0);
    let str_size_at = w.pos();
    w.u64(0);
    w.u32(off32(strings.len() - 1)?);

    let mut str_off: HashMap<&str, usize> = HashMap::new();
    for s in &strings {
        let off = w.pos();
        str_off.insert(s, off);
        w.u16(u16::try_from(s.len()).map_err(|_| Error::overflow("BNTX: string too long"))?);
        w.raw(s.as_bytes());
        w.u8(0);
        w.align(2);
    }
    let file_name_off = *str_off.get(bntx.name.as_str()).expect("file name interned");
    w.patch_off32(name_offset_at, file_name_off + 2)?;

    w.align(8);
    let dic = w.pos();
    w.patch_u64(dict_ptr_at, dic as u64);
    w.ptrs.push(Ptr {
        pos: dict_ptr_at,
        in_data: false,
    });
    let names: Vec<String> = bntx.textures.iter().map(|t| t.name.clone()).collect();
    let nodes = dict::build(&names);
    w.raw(b"_DIC");
    w.u32(off32(nodes.len() - 1)?);
    for node in &nodes {
        w.u32(node.reference);
        w.u16(node.left);
        w.u16(node.right);
        let key = node.key.map_or("", |i| names[i].as_str());
        w.ptr(*str_off.get(key).expect("dict key interned") as u64, false);
    }

    Ok((
        str_off,
        StrBlock {
            block: str_block,
            next_at: str_next_at,
            size_at: str_size_at,
        },
    ))
}

fn mip_offsets(tex: &Texture) -> &[u64] {
    if tex.mip_offsets.is_empty() {
        &[0]
    } else {
        &tex.mip_offsets
    }
}

fn write_brti(w: &mut Writer, tex: &Texture, str_off: &HashMap<&str, usize>, start: usize) {
    let info = &tex.info;
    w.raw(b"BRTI");
    w.u32(0);
    w.u64(0);

    w.u8(info.flags);
    w.u8(info.dim);
    w.u16(info.tile_mode);
    w.u16(info.swizzle);
    w.u16(info.mip_count);
    w.u32(info.sample_count);
    w.u32(info.format.raw());
    w.u32(info.gpu_access);
    w.u32(info.width);
    w.u32(info.height);
    w.u32(info.depth);
    w.u32(info.array_count);
    w.u32(info.texture_layout);
    w.u32(info.texture_layout2);
    w.raw(&info.reserved);
    w.u32(u32::try_from(tex.image_data.len()).unwrap_or(info.image_size));
    w.u32(info.alignment);
    w.u8(info.channel_r);
    w.u8(info.channel_g);
    w.u8(info.channel_b);
    w.u8(info.channel_a);
    w.u32(info.surface_dim);

    let name = *str_off
        .get(tex.name.as_str())
        .expect("texture name interned");
    let runtime = start + 0xA0;
    let view = runtime + RUNTIME_SCRATCH;
    let mip_table = runtime + RUNTIME_SCRATCH * 2;
    w.ptr(name as u64, false);
    w.ptr(0x20, false);
    w.ptr(mip_table as u64, false);
    w.u64(0);
    w.ptr(runtime as u64, false);
    w.ptr(view as u64, false);
    w.u64(0);
    w.u64(0);
}

fn finish_block_chain(
    w: &mut Writer,
    brti_pos: &[usize],
    brtd: usize,
    block_offset_at: usize,
    str_block: StrBlock,
) -> Result<()> {
    let first_block = brti_pos.first().copied().unwrap_or(brtd);
    let block = u16::try_from(str_block.block)
        .map_err(|_| Error::overflow("BNTX: first block offset exceeds 64 KiB"))?;
    w.patch_u16(block_offset_at, block);
    let str_to_first = first_block - str_block.block;
    w.patch_off32(str_block.next_at, str_to_first)?;
    w.patch_u64(str_block.size_at, str_to_first as u64);
    for (i, &here) in brti_pos.iter().enumerate() {
        let next = brti_pos.get(i + 1).copied().unwrap_or(brtd);
        let delta = (next - here) as u64;
        w.patch_off32(here + 4, next - here)?;
        w.patch_u64(here + 8, delta);
    }
    Ok(())
}

fn write_rlt(w: &mut Writer, rlt: usize, info_end: usize, brtd: usize) {
    let mut s0: Vec<usize> = w
        .ptrs
        .iter()
        .filter(|p| !p.in_data)
        .map(|p| p.pos)
        .collect();
    let mut s1: Vec<usize> = w.ptrs.iter().filter(|p| p.in_data).map(|p| p.pos).collect();
    s0.sort_unstable();
    s1.sort_unstable();
    let e0 = group_entries(&s0);
    let e1 = group_entries(&s1);

    w.raw(b"_RLT");
    w.u32(u32::try_from(rlt).unwrap_or(0));
    w.u32(2);
    w.u32(0);
    write_section(w, 0, info_end, 0, e0.len());
    write_section(w, brtd, rlt - brtd, e0.len(), e1.len());
    for e in e0.iter().chain(e1.iter()) {
        w.u32(u32::try_from(e.offset).unwrap_or(0));
        w.u16(e.array_count);
        w.u8(e.pointer_count);
        w.u8(e.padding_count);
    }
}

fn write_section(w: &mut Writer, offset: usize, size: usize, first_entry: usize, count: usize) {
    w.u64(0);
    w.u32(u32::try_from(offset).unwrap_or(0));
    w.u32(u32::try_from(size).unwrap_or(0));
    w.u32(u32::try_from(first_entry).unwrap_or(0));
    w.u32(u32::try_from(count).unwrap_or(0));
}

struct RltEntry {
    offset: usize,
    array_count: u16,
    pointer_count: u8,
    padding_count: u8,
}

fn group_entries(positions: &[usize]) -> Vec<RltEntry> {
    let mut entries = Vec::new();
    let mut i = 0;
    while i < positions.len() {
        let start = positions[i];
        let mut run = 1usize;
        while i + run < positions.len() && positions[i + run] == start + run * 8 {
            run += 1;
        }
        let run = run.min(255);
        entries.push(RltEntry {
            offset: start,
            array_count: 1,
            pointer_count: u8::try_from(run).unwrap_or(255),
            padding_count: 0,
        });
        i += run;
    }
    entries
}
