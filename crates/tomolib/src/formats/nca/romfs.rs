use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};

use crate::{Error, Result};

const ROMFS_HEADER_SIZE: usize = 0x50;
const DIR_ENTRY_HEAD: usize = 0x18;
const FILE_ENTRY_HEAD: usize = 0x20;

#[derive(Debug, Clone)]
pub struct FsEntry {
    pub path: String,
    pub offset: u64,
    pub size: u64,
}

fn le_u32(b: &[u8], off: usize) -> Result<u32> {
    b.get(off..off + 4)
        .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
        .ok_or_else(|| Error::malformed("romfs entry truncated"))
}

fn le_u64(b: &[u8], off: usize) -> Result<u64> {
    b.get(off..off + 8)
        .map(|s| u64::from_le_bytes(s.try_into().unwrap()))
        .ok_or_else(|| Error::malformed("romfs entry truncated"))
}

fn read_at<S: Read + Seek>(stream: &mut S, offset: u64, len: usize) -> Result<Vec<u8>> {
    stream.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf)?;
    Ok(buf)
}

fn read_table<S: Read + Seek>(
    stream: &mut S,
    offset: u64,
    size: u64,
    bound: u64,
    ctx: &str,
) -> Result<Vec<u8>> {
    let end = offset
        .checked_add(size)
        .ok_or_else(|| Error::malformed(format!("{ctx} offset overflow")))?;
    if end > bound {
        return Err(Error::malformed(format!("{ctx} extends past section")));
    }
    let len = usize::try_from(size).map_err(|_| Error::malformed(format!("{ctx} too large")))?;
    read_at(stream, offset, len)
}

fn align4(n: usize) -> usize {
    (n + 3) & !3
}

pub(crate) fn list<S: Read + Seek>(stream: &mut S) -> Result<Vec<FsEntry>> {
    let stream_len = stream.seek(SeekFrom::End(0))?;
    let header = read_at(stream, 0, ROMFS_HEADER_SIZE)?;
    if le_u64(&header, 0)? != ROMFS_HEADER_SIZE as u64 {
        return Err(Error::malformed("unexpected romfs header size"));
    }
    let dir_entry_off = le_u64(&header, 0x18)?;
    let dir_entry_size = le_u64(&header, 0x20)?;
    let file_entry_off = le_u64(&header, 0x38)?;
    let file_entry_size = le_u64(&header, 0x40)?;
    let data_offset = le_u64(&header, 0x48)?;

    let dir_table = read_table(
        stream,
        dir_entry_off,
        dir_entry_size,
        stream_len,
        "romfs dir table",
    )?;
    let file_table = read_table(
        stream,
        file_entry_off,
        file_entry_size,
        stream_len,
        "romfs file table",
    )?;

    let mut dir_paths: HashMap<u32, String> = HashMap::new();
    let mut pos = 0usize;
    while pos < dir_table.len() {
        let entry = &dir_table[pos..];
        let parent = le_u32(entry, 0)?;
        let name_size = le_u32(entry, 0x14)? as usize;
        let name_end = DIR_ENTRY_HEAD + name_size;
        let name = entry
            .get(DIR_ENTRY_HEAD..name_end)
            .ok_or_else(|| Error::malformed("romfs dir name truncated"))?;
        let v_addr = u32::try_from(pos).map_err(|_| Error::malformed("romfs dir addr overflow"))?;

        let path = if v_addr == 0 {
            String::new()
        } else {
            let parent_path = dir_paths
                .get(&parent)
                .ok_or_else(|| Error::malformed("romfs dir has unknown parent"))?;
            let name =
                std::str::from_utf8(name).map_err(|_| Error::invalid_utf8("romfs dir name"))?;
            join(parent_path, name)
        };
        dir_paths.insert(v_addr, path);
        pos += DIR_ENTRY_HEAD + align4(name_size);
    }

    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < file_table.len() {
        let entry = &file_table[pos..];
        let parent = le_u32(entry, 0)?;
        let file_data_offset = le_u64(entry, 0x08)?;
        let file_data_size = le_u64(entry, 0x10)?;
        let name_size = le_u32(entry, 0x1C)? as usize;
        let name_end = FILE_ENTRY_HEAD + name_size;
        let name = entry
            .get(FILE_ENTRY_HEAD..name_end)
            .ok_or_else(|| Error::malformed("romfs file name truncated"))?;
        let name = std::str::from_utf8(name).map_err(|_| Error::invalid_utf8("romfs file name"))?;

        let parent_path = dir_paths
            .get(&parent)
            .ok_or_else(|| Error::malformed("romfs file has unknown parent"))?;

        out.push(FsEntry {
            path: join(parent_path, name),
            offset: data_offset + file_data_offset,
            size: file_data_size,
        });
        pos += FILE_ENTRY_HEAD + align4(name_size);
    }

    Ok(out)
}

fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}
