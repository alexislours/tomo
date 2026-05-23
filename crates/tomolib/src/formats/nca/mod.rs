mod crypto;
mod keys;
mod romfs;
mod verify;

use std::io::{self, Read, Seek, SeekFrom};

use crate::formats::nca::crypto::{Aes128Ecb, ctr_apply, xts_decrypt_sector};
use crate::formats::nsp::PartitionFs;
use crate::{Error, Result};

pub use crate::formats::nca::keys::KeySet;
pub use crate::formats::nca::romfs::FsEntry;

const SECTOR_SIZE: usize = 0x200;
const HEADER_SECTORS: usize = 6;
const HEADER_BLOCK_SIZE: usize = 0xC00;
const PARTITION_NUM: usize = 4;

const MAGIC_NCA2: &[u8; 4] = b"NCA2";
const MAGIC_NCA3: &[u8; 4] = b"NCA3";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentType {
    Program,
    Meta,
    Control,
    Manual,
    Data,
    PublicData,
    Unknown(u8),
}

impl ContentType {
    fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Program,
            1 => Self::Meta,
            2 => Self::Control,
            3 => Self::Manual,
            4 => Self::Data,
            5 => Self::PublicData,
            other => Self::Unknown(other),
        }
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Program => "Program",
            Self::Meta => "Meta",
            Self::Control => "Control",
            Self::Manual => "Manual",
            Self::Data => "Data",
            Self::PublicData => "PublicData",
            Self::Unknown(_) => "Unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistributionType {
    Download,
    GameCard,
    Unknown(u8),
}

impl DistributionType {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Download => "Download",
            Self::GameCard => "GameCard",
            Self::Unknown(_) => "Unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatType {
    RomFs,
    PartitionFs,
    Unknown(u8),
}

impl FormatType {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::RomFs => "RomFs",
            Self::PartitionFs => "PartitionFs",
            Self::Unknown(_) => "Unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashType {
    None,
    HierarchicalSha256,
    HierarchicalIntegrity,
    Other(u8),
}

impl HashType {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::HierarchicalSha256 => "HierarchicalSha256",
            Self::HierarchicalIntegrity => "HierarchicalIntegrity",
            Self::Other(_) => "Other",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncryptionType {
    None,
    AesXts,
    AesCtr,
    AesCtrEx,
    Other(u8),
}

impl EncryptionType {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::AesXts => "AesXts",
            Self::AesCtr => "AesCtr",
            Self::AesCtrEx => "AesCtrEx",
            Self::Other(_) => "Other",
        }
    }
}

#[derive(Debug, Clone)]
pub struct NcaHeader {
    pub format: &'static str,
    pub distribution: DistributionType,
    pub content_type: ContentType,
    pub key_generation: u8,
    pub signature_key_generation: u8,
    pub kaek_index: u8,
    pub content_size: u64,
    pub program_id: u64,
    pub content_index: u32,
    pub sdk_addon_version: u32,
    pub rights_id: [u8; 16],
}

impl NcaHeader {
    #[must_use]
    pub fn has_rights_id(&self) -> bool {
        self.rights_id.iter().any(|&b| b != 0)
    }

    #[must_use]
    pub fn sdk_addon_version_string(&self) -> String {
        let v = self.sdk_addon_version;
        let (major, minor, build, relstep) = (
            (v >> 24) & 0xff,
            (v >> 16) & 0xff,
            (v >> 8) & 0xff,
            v & 0xff,
        );
        if relstep > 0 {
            format!("{major}.{minor}.{build}-{relstep}")
        } else {
            format!("{major}.{minor}.{build}")
        }
    }
}

#[derive(Debug, Clone)]
pub struct Partition {
    pub index: usize,
    pub offset: u64,
    pub size: u64,
    pub format: FormatType,
    pub hash_type: HashType,
    pub enc_type: EncryptionType,
    pub fs_offset: u64,
    pub fs_size: u64,
    ctr_base: [u8; 16],
    hash_meta: verify::HashMeta,
}

#[derive(Debug)]
pub struct Nca {
    pub header: NcaHeader,
    pub partitions: Vec<Partition>,
    content_key: Option<[u8; 16]>,
}

fn le_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(b[off..off + 4].try_into().unwrap())
}
fn le_u64(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(b[off..off + 8].try_into().unwrap())
}

impl Nca {
    pub fn open<R: Read + Seek>(reader: &mut R, keys: &KeySet) -> Result<Self> {
        let mut block = vec![0u8; HEADER_BLOCK_SIZE];
        reader.seek(SeekFrom::Start(0))?;
        reader.read_exact(&mut block)?;

        let (data_cipher, tweak_cipher) = keys.header_ciphers();
        decrypt_header(&mut block, data_cipher, tweak_cipher);

        let hdr = &block[0x200..0x400];
        let magic = &hdr[0..4];
        let format = if magic == MAGIC_NCA3 {
            "NCA3"
        } else if magic == MAGIC_NCA2 {
            "NCA2"
        } else {
            return Err(Error::bad_magic("NCA"));
        };

        let distribution = match hdr[4] {
            0 => DistributionType::Download,
            1 => DistributionType::GameCard,
            other => DistributionType::Unknown(other),
        };
        let content_type = ContentType::from_u8(hdr[5]);
        let key_generation = hdr[6].max(hdr[0x20]);
        let kaek_index = hdr[7];
        let content_size = le_u64(hdr, 0x08);
        let program_id = le_u64(hdr, 0x10);
        let content_index = le_u32(hdr, 0x18);
        let sdk_addon_version = le_u32(hdr, 0x1C);
        let signature_key_generation = hdr[0x21];
        let mut rights_id = [0u8; 16];
        rights_id.copy_from_slice(&hdr[0x30..0x40]);

        let header = NcaHeader {
            format,
            distribution,
            content_type,
            key_generation,
            signature_key_generation,
            kaek_index,
            content_size,
            program_id,
            content_index,
            sdk_addon_version,
            rights_id,
        };

        let master_key_rev = if key_generation == 0 {
            0
        } else {
            key_generation - 1
        };

        let content_key = resolve_content_key(&block, &header, master_key_rev, keys);

        let mut partitions = Vec::new();
        for i in 0..PARTITION_NUM {
            let eo = 0x200 + 0x40 + i * 0x10;
            let start_blk = le_u32(&block, eo);
            let end_blk = le_u32(&block, eo + 4);
            if end_blk <= start_blk {
                continue;
            }

            let offset = u64::from(start_blk) * SECTOR_SIZE as u64;
            let size = u64::from(end_blk - start_blk) * SECTOR_SIZE as u64;

            let fo = 0x400 + i * SECTOR_SIZE;
            let fs_header = &block[fo..fo + SECTOR_SIZE];
            partitions.push(parse_partition(i, offset, size, fs_header)?);
        }

        Ok(Self {
            header,
            partitions,
            content_key,
        })
    }

    #[must_use]
    pub fn has_content_key(&self) -> bool {
        self.content_key.is_some()
    }

    fn section_cipher(&self, part: &Partition) -> Result<SectionCipher> {
        match part.enc_type {
            EncryptionType::None => Ok(SectionCipher::None),
            EncryptionType::AesCtr => {
                let key = self
                    .content_key
                    .ok_or_else(|| Error::unsupported("AES-CTR key was not determined"))?;
                Ok(SectionCipher::Ctr {
                    key,
                    base_ctr: part.ctr_base,
                })
            }
            other => Err(Error::unsupported(format!(
                "section encryption {} is not supported",
                other.name()
            ))),
        }
    }

    fn section_stream<'a, R: Read + Seek>(
        &self,
        reader: &'a mut R,
        part: &Partition,
    ) -> Result<SectionStream<'a, R>> {
        Ok(SectionStream {
            inner: reader,
            fs_abs: part.offset + part.fs_offset,
            fs_size: part.fs_size,
            pos: 0,
            inner_pos: None,
            cipher: self.section_cipher(part)?,
        })
    }

    fn section_stream_full<'a, R: Read + Seek>(
        &self,
        reader: &'a mut R,
        part: &Partition,
    ) -> Result<SectionStream<'a, R>> {
        Ok(SectionStream {
            inner: reader,
            fs_abs: part.offset,
            fs_size: part.size,
            pos: 0,
            inner_pos: None,
            cipher: self.section_cipher(part)?,
        })
    }

    pub fn verify_partition<R: Read + Seek>(&self, reader: &mut R, part: &Partition) -> Result<()> {
        let mut stream = self.section_stream_full(reader, part)?;
        verify::verify(&mut stream, &part.hash_meta)
    }

    pub fn list_partition<R: Read + Seek>(
        &self,
        reader: &mut R,
        part: &Partition,
    ) -> Result<Vec<FsEntry>> {
        let mut stream = self.section_stream(reader, part)?;
        match part.format {
            FormatType::RomFs => romfs::list(&mut stream),
            FormatType::PartitionFs => list_pfs(&mut stream),
            FormatType::Unknown(v) => {
                Err(Error::unsupported(format!("unknown partition format {v}")))
            }
        }
    }

    pub fn copy_file<R: Read + Seek, W: io::Write>(
        &self,
        reader: &mut R,
        part: &Partition,
        entry: &FsEntry,
        out: &mut W,
    ) -> Result<()> {
        let mut stream = self.section_stream(reader, part)?;
        stream.seek(SeekFrom::Start(entry.offset))?;
        let mut remaining = entry.size;
        let mut buf = vec![0u8; 1 << 20];
        while remaining > 0 {
            let want = buf
                .len()
                .min(usize::try_from(remaining).unwrap_or(usize::MAX));
            stream.read_exact(&mut buf[..want])?;
            out.write_all(&buf[..want])?;
            remaining -= want as u64;
        }
        Ok(())
    }
}

fn resolve_content_key(
    block: &[u8],
    header: &NcaHeader,
    master_key_rev: u8,
    keys: &KeySet,
) -> Option<[u8; 16]> {
    if header.has_rights_id() {
        return keys
            .decrypt_title_key(&header.rights_id, master_key_rev)
            .ok();
    }

    let key_area_off = 0x200 + 0x100;
    let mut wrapped = [0u8; 16];
    wrapped.copy_from_slice(&block[key_area_off + 0x20..key_area_off + 0x30]);
    if wrapped.iter().all(|&b| b == 0) {
        return None;
    }
    keys.decrypt_key_area_key(usize::from(header.kaek_index), master_key_rev, &wrapped)
        .ok()
}

fn parse_partition(index: usize, offset: u64, size: u64, fs_header: &[u8]) -> Result<Partition> {
    let format = match fs_header[2] {
        0 => FormatType::RomFs,
        1 => FormatType::PartitionFs,
        other => FormatType::Unknown(other),
    };
    let hash_type = match fs_header[3] {
        1 => HashType::None,
        2 => HashType::HierarchicalSha256,
        3 => HashType::HierarchicalIntegrity,
        other => HashType::Other(other),
    };
    let enc_type = match fs_header[4] {
        1 => EncryptionType::None,
        2 => EncryptionType::AesXts,
        3 => EncryptionType::AesCtr,
        4 => EncryptionType::AesCtrEx,
        other => EncryptionType::Other(other),
    };

    let generation = le_u32(fs_header, 0x140);
    let secure_value = le_u32(fs_header, 0x144);
    let mut ctr_base = [0u8; 16];
    ctr_base[0..4].copy_from_slice(&secure_value.to_be_bytes());
    ctr_base[4..8].copy_from_slice(&generation.to_be_bytes());

    let hash_info = &fs_header[0x08..0x08 + 0xF8];
    let (hash_meta, fs_offset, fs_size) = match hash_type {
        HashType::HierarchicalSha256 => verify::parse_sha256(hash_info)?,
        HashType::HierarchicalIntegrity => verify::parse_ivfc(hash_info)?,
        HashType::None => (verify::HashMeta::None, 0, size),
        HashType::Other(v) => {
            return Err(Error::unsupported(format!("unsupported hash type {v}")));
        }
    };

    Ok(Partition {
        index,
        offset,
        size,
        format,
        hash_type,
        enc_type,
        fs_offset,
        fs_size,
        ctr_base,
        hash_meta,
    })
}

fn decrypt_header(block: &mut [u8], data_cipher: &Aes128Ecb, tweak_cipher: &Aes128Ecb) {
    let mut probe = block[SECTOR_SIZE..2 * SECTOR_SIZE].to_vec();
    xts_decrypt_sector(data_cipher, tweak_cipher, 1, &mut probe);
    let nca2 = &probe[0..4] == MAGIC_NCA2;

    for i in 0..HEADER_SECTORS {
        let sector = if nca2 && i >= 2 { 0 } else { i as u64 };
        let range = i * SECTOR_SIZE..(i + 1) * SECTOR_SIZE;
        xts_decrypt_sector(data_cipher, tweak_cipher, sector, &mut block[range]);
    }
}

fn list_pfs<R: Read + Seek>(stream: &mut SectionStream<'_, R>) -> Result<Vec<FsEntry>> {
    stream.seek(SeekFrom::Start(0))?;
    let fs = PartitionFs::read_header(stream)?;
    Ok(fs
        .entries()
        .iter()
        .map(|e| FsEntry {
            path: e.name.clone(),
            offset: e.offset,
            size: e.size,
        })
        .collect())
}

#[allow(clippy::large_enum_variant)]
enum SectionCipher {
    None,
    Ctr { key: [u8; 16], base_ctr: [u8; 16] },
}

pub struct SectionStream<'a, R> {
    inner: &'a mut R,
    fs_abs: u64,
    fs_size: u64,
    pos: u64,
    inner_pos: Option<u64>,
    cipher: SectionCipher,
}

impl<R> std::fmt::Debug for SectionStream<'_, R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SectionStream")
            .field("fs_abs", &self.fs_abs)
            .field("fs_size", &self.fs_size)
            .field("pos", &self.pos)
            .finish()
    }
}

impl<R: Read + Seek> Read for SectionStream<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let remaining = self.fs_size.saturating_sub(self.pos);
        let n = buf
            .len()
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        if n == 0 {
            return Ok(0);
        }
        let abs = self.fs_abs + self.pos;
        if self.inner_pos != Some(abs) {
            self.inner.seek(SeekFrom::Start(abs))?;
        }
        self.inner.read_exact(&mut buf[..n])?;
        self.inner_pos = Some(abs + n as u64);
        if let SectionCipher::Ctr { key, base_ctr } = &self.cipher {
            ctr_apply(key, base_ctr, abs, &mut buf[..n]);
        }
        self.pos += n as u64;
        Ok(n)
    }
}

impl<R: Read + Seek> Seek for SectionStream<'_, R> {
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
