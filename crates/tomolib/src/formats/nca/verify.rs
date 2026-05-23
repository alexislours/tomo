use std::io::{Read, Seek, SeekFrom};

use sha2::{Digest, Sha256};

use crate::{Error, Result};

const HASH: usize = 32;

#[derive(Debug, Clone)]
pub(crate) struct Layer {
    pub offset: u64,
    pub size: u64,
    pub block_size: u64,
}

#[derive(Debug, Clone)]
pub(crate) enum HashMeta {
    None,
    Tree {
        master: Vec<u8>,
        layers: Vec<Layer>,
        pad_blocks: bool,
    },
}

fn le_u32(b: &[u8], off: usize) -> Result<u32> {
    b.get(off..off + 4)
        .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
        .ok_or_else(|| Error::malformed("hash info truncated"))
}

fn le_u64(b: &[u8], off: usize) -> Result<u64> {
    b.get(off..off + 8)
        .map(|s| u64::from_le_bytes(s.try_into().unwrap()))
        .ok_or_else(|| Error::malformed("hash info truncated"))
}

pub(crate) fn parse_sha256(hash_info: &[u8]) -> Result<(HashMeta, u64, u64)> {
    let master = hash_info
        .get(0..HASH)
        .ok_or_else(|| Error::malformed("sha256 master hash truncated"))?
        .to_vec();
    let block_size = u64::from(le_u32(hash_info, 0x20)?);
    let layer_num = le_u32(hash_info, 0x24)? as usize;
    if layer_num == 0 {
        return Err(Error::malformed("HierarchicalSha256 has zero layers"));
    }
    let mut layers = Vec::with_capacity(layer_num);
    for i in 0..layer_num {
        let lo = 0x28 + i * 0x10;
        layers.push(Layer {
            offset: le_u64(hash_info, lo)?,
            size: le_u64(hash_info, lo + 8)?,
            block_size,
        });
    }
    let data = layers.last().unwrap();
    let (off, size) = (data.offset, data.size);
    Ok((
        HashMeta::Tree {
            master,
            layers,
            pad_blocks: false,
        },
        off,
        size,
    ))
}

pub(crate) fn parse_ivfc(hash_info: &[u8]) -> Result<(HashMeta, u64, u64)> {
    if hash_info.get(0..4) != Some(b"IVFC") {
        return Err(Error::malformed("bad IVFC magic in hash info"));
    }
    let master_hash_size = le_u32(hash_info, 0x08)? as usize;
    let layer_num = le_u32(hash_info, 0x0C)? as usize;
    let real_layers = layer_num
        .checked_sub(1)
        .filter(|&n| n != 0)
        .ok_or_else(|| Error::malformed("IVFC has too few layers"))?;

    let mut layers = Vec::with_capacity(real_layers);
    for i in 0..real_layers {
        let lo = 0x10 + i * 0x18;
        let log2 = le_u32(hash_info, lo + 0x10)?;
        if log2 >= 64 {
            return Err(Error::malformed("IVFC block size out of range"));
        }
        layers.push(Layer {
            offset: le_u64(hash_info, lo)?,
            size: le_u64(hash_info, lo + 8)?,
            block_size: 1u64 << log2,
        });
    }

    let master_off = (0x10 + 0x18 * layer_num).next_multiple_of(0x20);
    let master = hash_info
        .get(master_off..master_off + master_hash_size)
        .ok_or_else(|| Error::malformed("IVFC master hash truncated"))?
        .to_vec();

    let data = layers.last().unwrap();
    let (off, size) = (data.offset, data.size);
    Ok((
        HashMeta::Tree {
            master,
            layers,
            pad_blocks: true,
        },
        off,
        size,
    ))
}

fn check_bounds(offset: u64, size: u64, bound: u64, ctx: &str) -> Result<()> {
    let end = offset
        .checked_add(size)
        .ok_or_else(|| Error::malformed(format!("{ctx}: offset overflow")))?;
    if end > bound {
        return Err(Error::malformed(format!("{ctx}: extends past section")));
    }
    Ok(())
}

fn read_at<S: Read + Seek>(stream: &mut S, offset: u64, size: u64) -> Result<Vec<u8>> {
    let len = usize::try_from(size).map_err(|_| Error::malformed("hash layer too large"))?;
    stream.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf)?;
    Ok(buf)
}

fn block_hash(chunk: &[u8], block_size: usize, pad: bool, scratch: &mut [u8]) -> [u8; HASH] {
    let mut h = Sha256::new();
    if pad && chunk.len() < block_size {
        scratch[..chunk.len()].copy_from_slice(chunk);
        scratch[chunk.len()..].fill(0);
        h.update(&scratch[..block_size]);
    } else {
        h.update(chunk);
    }
    h.finalize().into()
}

fn check(expected: &[u8], got: &[u8; HASH], ctx: &str, block: usize) -> Result<()> {
    if expected == got {
        Ok(())
    } else {
        Err(Error::integrity(format!(
            "{ctx}: block {block} failed hash check"
        )))
    }
}

fn verify_layer(data: &[u8], block_size: usize, hashes: &[u8], pad: bool, ctx: &str) -> Result<()> {
    let block_num = data.len().div_ceil(block_size);
    if hashes.len() < block_num * HASH {
        return Err(Error::malformed(format!(
            "{ctx}: parent hash table too small"
        )));
    }
    let mut scratch = vec![0u8; block_size];
    for i in 0..block_num {
        let start = i * block_size;
        let end = ((i + 1) * block_size).min(data.len());
        let digest = block_hash(&data[start..end], block_size, pad, &mut scratch);
        check(&hashes[i * HASH..i * HASH + HASH], &digest, ctx, i)?;
    }
    Ok(())
}

fn verify_data<S: Read + Seek>(
    stream: &mut S,
    layer: &Layer,
    hashes: &[u8],
    pad: bool,
    bound: u64,
) -> Result<()> {
    check_bounds(layer.offset, layer.size, bound, "data")?;
    let block_size = usize::try_from(layer.block_size)
        .map_err(|_| Error::malformed("data block size too large"))?;
    let block_num = usize::try_from(layer.size.div_ceil(layer.block_size))
        .map_err(|_| Error::malformed("data layer too large"))?;
    if hashes.len() < block_num * HASH {
        return Err(Error::malformed("data: hash table too small"));
    }

    let blocks_per_read = ((4 << 20) / block_size).max(1);
    let buf_len = usize::try_from(
        (blocks_per_read as u64)
            .saturating_mul(layer.block_size)
            .min(layer.size),
    )
    .map_err(|_| Error::malformed("data read buffer too large"))?;
    let mut buf = vec![0u8; buf_len];
    let mut scratch = vec![0u8; block_size];

    stream.seek(SeekFrom::Start(layer.offset))?;
    let mut remaining = layer.size;
    let mut block = 0usize;
    while remaining > 0 {
        let want = usize::try_from(remaining)
            .unwrap_or(usize::MAX)
            .min(buf.len());
        stream.read_exact(&mut buf[..want])?;
        let mut off = 0;
        while off < want {
            let end = (off + block_size).min(want);
            let digest = block_hash(&buf[off..end], block_size, pad, &mut scratch);
            check(
                &hashes[block * HASH..block * HASH + HASH],
                &digest,
                "data",
                block,
            )?;
            off = end;
            block += 1;
        }
        remaining -= want as u64;
    }
    Ok(())
}

pub(crate) fn verify<S: Read + Seek>(stream: &mut S, meta: &HashMeta) -> Result<()> {
    let HashMeta::Tree {
        master,
        layers,
        pad_blocks,
    } = meta
    else {
        return Ok(());
    };
    let Some((data_layer, hash_layers)) = layers.split_last() else {
        return Ok(());
    };

    let bound = stream.seek(SeekFrom::End(0))?;

    let mut parent = master.clone();
    for (i, layer) in hash_layers.iter().enumerate() {
        let bs = usize::try_from(layer.block_size)
            .map_err(|_| Error::malformed("hash block size too large"))?;
        check_bounds(layer.offset, layer.size, bound, &format!("hash layer {i}"))?;
        let loaded = read_at(stream, layer.offset, layer.size)?;
        verify_layer(
            &loaded,
            bs,
            &parent,
            *pad_blocks,
            &format!("hash layer {i}"),
        )?;
        parent = loaded;
    }
    verify_data(stream, data_layer, &parent, *pad_blocks, bound)
}
