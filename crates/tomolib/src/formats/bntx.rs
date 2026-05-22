mod dict;
#[cfg(feature = "image-encode-gpu")]
mod encode_gpu;
mod format;
#[cfg(feature = "image")]
pub mod image;
mod swizzle;
mod write;

use crate::formats::binio::{ByteOrder, align_up};
use crate::{Error, Result};

pub use format::{ChannelFormat, ImageFormat, TypeFormat};

pub const BNTX_MAGIC: [u8; 4] = *b"BNTX";

/// Target platform recorded in a BNTX header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Nx,
    Ounce,
    Pc,
    Gen,
    Other([u8; 4]),
}

impl Platform {
    /// The four-byte platform magic stored in the file.
    #[must_use]
    pub fn magic(self) -> [u8; 4] {
        match self {
            Self::Nx => *b"NX  ",
            Self::Ounce => *b"Ounc",
            Self::Pc => *b"PC  ",
            Self::Gen => *b"Gen ",
            Self::Other(m) => m,
        }
    }

    /// Identifies a platform from its four-byte magic.
    #[must_use]
    pub fn from_magic(m: [u8; 4]) -> Self {
        match &m {
            b"NX  " => Self::Nx,
            b"Ounc" => Self::Ounce,
            b"PC  " => Self::Pc,
            b"Gen " => Self::Gen,
            _ => Self::Other(m),
        }
    }

    /// A human-readable name for the platform.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Nx => "NX (Switch)",
            Self::Ounce => "Ounce (Switch 2)",
            Self::Pc => "PC",
            Self::Gen => "Gen (generic)",
            Self::Other(_) => "unknown",
        }
    }
}

/// A typed array of user-metadata values attached to a texture.
#[derive(Debug, Clone, PartialEq)]
pub enum UserValue {
    Int32(Vec<i32>),
    Single(Vec<f32>),
    String(Vec<String>),
    Byte(Vec<u8>),
    WString(Vec<String>),
}

/// A named user-metadata entry attached to a texture.
#[derive(Debug, Clone, PartialEq)]
pub struct UserData {
    pub name: String,
    pub value: UserValue,
}

/// Dimensions, format, and layout parameters of a single texture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextureInfo {
    pub flags: u8,
    pub dim: u8,
    pub tile_mode: u16,
    pub swizzle: u16,
    pub mip_count: u16,
    pub sample_count: u32,
    pub format: ImageFormat,
    pub gpu_access: u32,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub array_count: u32,
    pub texture_layout: u32,
    pub texture_layout2: u32,
    pub reserved: [u8; 20],
    pub image_size: u32,
    pub alignment: u32,
    pub channel_r: u8,
    pub channel_g: u8,
    pub channel_b: u8,
    pub channel_a: u8,
    pub surface_dim: u32,
}

impl TextureInfo {
    /// The log2 of the tile block height, decoded from the texture layout.
    #[must_use]
    pub fn block_height_log2(&self) -> u32 {
        self.texture_layout & 7
    }
}

/// A single texture: its name, [`TextureInfo`], mip offsets, and swizzled image
/// data.
#[derive(Debug, Clone, PartialEq)]
pub struct Texture {
    pub name: String,
    pub info: TextureInfo,
    pub mip_offsets: Vec<u64>,
    pub user_data: Vec<UserData>,
    pub image_data: Vec<u8>,
}

impl Texture {
    pub(crate) fn mip_byte_range(&self, mip: usize) -> (usize, usize) {
        let to_usize = |o: u64| usize::try_from(o).ok();
        let start = self
            .mip_offsets
            .get(mip)
            .and_then(|&o| to_usize(o))
            .unwrap_or(0);
        let end = self
            .mip_offsets
            .get(mip + 1)
            .and_then(|&o| to_usize(o))
            .unwrap_or(self.image_data.len());
        (start, end)
    }
}

/// A parsed BNTX texture container.
#[derive(Debug, Clone, PartialEq)]
pub struct Bntx {
    pub byte_order: ByteOrder,
    pub version: (u16, u8, u8),
    pub alignment_log2: u8,
    pub target_address_size: u8,
    pub flag: u16,
    pub block_offset: u16,
    pub name: String,
    pub platform: Platform,
    pub textures: Vec<Texture>,
}

impl Bntx {
    /// The data alignment in bytes (`1 << alignment_log2`).
    #[must_use]
    pub fn alignment(&self) -> usize {
        1usize << self.alignment_log2
    }

    /// Serializes the container back to the binary BNTX format.
    ///
    /// User-data on textures is not supported when writing.
    pub fn write(&self) -> Result<Vec<u8>> {
        write::write(self)
    }

    /// Parses a BNTX file.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes);

        let bom = r.array_at::<2>(0x0C, "BNTX BOM")?;
        let byte_order = match bom {
            [0xFF, 0xFE] => ByteOrder::Little,
            [0xFE, 0xFF] => ByteOrder::Big,
            _ => return Err(Error::malformed("BNTX: invalid byte-order mark")),
        };
        r.order = byte_order;

        if r.array_at::<4>(0, "BNTX magic")? != BNTX_MAGIC {
            return Err(Error::bad_magic("BNTX"));
        }

        let version = (r.u16_at(0x0A)?, r.byte(0x09)?, r.byte(0x08)?);
        let alignment_log2 = r.byte(0x0E)?;
        let target_address_size = r.byte(0x0F)?;
        let name_offset = r.u32_at(0x10)?;
        let flag = r.u16_at(0x14)?;
        let block_offset = r.u16_at(0x16)?;

        let platform = Platform::from_magic(r.array_at::<4>(0x20, "NX header magic")?);
        let texture_count = r.u32_at(0x24)? as usize;
        let texture_info_array = r.offset(0x28)?;

        let name = r.name_at(name_offset.checked_sub(2).map_or(0, |n| n as usize))?;

        if texture_count.saturating_mul(8) > bytes.len() {
            return Err(Error::out_of_range(
                "BNTX texture count",
                texture_count,
                bytes.len(),
            ));
        }
        let mut textures = Vec::with_capacity(texture_count);
        for i in 0..texture_count {
            let info_off = r.offset(texture_info_array + i * 8)?;
            textures.push(read_texture(&mut r, info_off)?);
        }

        Ok(Self {
            byte_order,
            version,
            alignment_log2,
            target_address_size,
            flag,
            block_offset,
            name,
            platform,
            textures,
        })
    }
}

fn read_texture(r: &mut Reader, off: usize) -> Result<Texture> {
    if r.array_at::<4>(off, "BRTI magic")? != *b"BRTI" {
        return Err(Error::malformed("BNTX: texture info missing BRTI magic"));
    }
    let b = off + 0x10;
    let info = TextureInfo {
        flags: r.byte(b)?,
        dim: r.byte(b + 0x1)?,
        tile_mode: r.u16_at(b + 0x2)?,
        swizzle: r.u16_at(b + 0x4)?,
        mip_count: r.u16_at(b + 0x6)?,
        sample_count: r.u32_at(b + 0x8)?,
        format: ImageFormat::from_raw(r.u32_at(b + 0xC)?),
        gpu_access: r.u32_at(b + 0x10)?,
        width: r.u32_at(b + 0x14)?,
        height: r.u32_at(b + 0x18)?,
        depth: r.u32_at(b + 0x1C)?,
        array_count: r.u32_at(b + 0x20)?,
        texture_layout: r.u32_at(b + 0x24)?,
        texture_layout2: r.u32_at(b + 0x28)?,
        reserved: r.array_at::<20>(b + 0x2C, "BRTI reserved")?,
        image_size: r.u32_at(b + 0x40)?,
        alignment: r.u32_at(b + 0x44)?,
        channel_r: r.byte(b + 0x48)?,
        channel_g: r.byte(b + 0x49)?,
        channel_b: r.byte(b + 0x4A)?,
        channel_a: r.byte(b + 0x4B)?,
        surface_dim: r.u32_at(b + 0x4C)?,
    };
    let name_offset = r.offset(b + 0x50)?;
    let data_pointers_offset = r.offset(b + 0x60)?;
    let user_data_offset = r.offset(b + 0x68)?;
    let user_data_dict_offset = r.offset(b + 0x88)?;

    let name = r.name_at(name_offset)?;

    let mip_count = info.mip_count.max(1) as usize;
    let mut mip_offsets = Vec::new();
    let mut image_data = Vec::new();
    if data_pointers_offset != 0 {
        let mut abs = Vec::with_capacity(mip_count);
        for i in 0..mip_count {
            abs.push(r.u64_at(data_pointers_offset + i * 8)?);
        }
        let base = usize::try_from(abs[0])
            .map_err(|_| Error::overflow("BNTX: image data offset exceeds addressable range"))?;
        let size = info.image_size as usize;
        image_data = r.slice(base, size, "BNTX image data")?.to_vec();
        mip_offsets = abs.iter().map(|&o| o.saturating_sub(abs[0])).collect();
    }

    let mut user_data = Vec::new();
    if user_data_dict_offset != 0 && user_data_offset != 0 {
        let count = dict::entry_count(r, user_data_dict_offset)?;
        for i in 0..count {
            user_data.push(read_user_data(r, user_data_offset + i * 0x18)?);
        }
    }

    Ok(Texture {
        name,
        info,
        mip_offsets,
        user_data,
        image_data,
    })
}

fn read_user_data(r: &mut Reader, off: usize) -> Result<UserData> {
    let name_offset = r.offset(off)?;
    let data_offset = r.offset(off + 0x8)?;
    let count = r.u32_at(off + 0x10)? as usize;
    if count > r.bytes.len() {
        return Err(Error::out_of_range(
            "BNTX user data count",
            count,
            r.bytes.len(),
        ));
    }
    let ty = r.byte(off + 0x14)?;
    let name = r.name_at(name_offset)?;

    let value = match ty {
        0 => {
            let mut v = Vec::with_capacity(count);
            for i in 0..count {
                v.push(r.i32_at(data_offset + i * 4)?);
            }
            UserValue::Int32(v)
        }
        1 => {
            let mut v = Vec::with_capacity(count);
            for i in 0..count {
                v.push(f32::from_bits(r.u32_at(data_offset + i * 4)?));
            }
            UserValue::Single(v)
        }
        2 => {
            let (mut values, mut pos) = (Vec::with_capacity(count), data_offset);
            for _ in 0..count {
                let (text, used) = r.name_at_len(pos)?;
                values.push(text);
                pos += used;
            }
            UserValue::String(values)
        }
        3 => UserValue::Byte(r.slice(data_offset, count, "user data bytes")?.to_vec()),
        4 => {
            let (mut values, mut pos) = (Vec::with_capacity(count), data_offset);
            for _ in 0..count {
                let (text, used) = r.wname_at_len(pos)?;
                values.push(text);
                pos += used;
            }
            UserValue::WString(values)
        }
        other => {
            return Err(Error::unsupported(format!(
                "BNTX: unknown user-data type {other}"
            )));
        }
    };

    Ok(UserData { name, value })
}

pub(crate) struct Reader<'a> {
    pub bytes: &'a [u8],
    pub order: ByteOrder,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            order: ByteOrder::Little,
        }
    }

    fn byte(&self, off: usize) -> Result<u8> {
        self.bytes
            .get(off)
            .copied()
            .ok_or_else(|| Error::truncated("BNTX byte", off, 1, 0))
    }

    fn array_at<const N: usize>(&self, off: usize, ctx: &'static str) -> Result<[u8; N]> {
        crate::formats::binio::read_array::<N>(self.bytes, off, ctx)
    }

    pub(crate) fn u16_at(&self, off: usize) -> Result<u16> {
        self.order.read_u16(self.bytes, off, "BNTX u16")
    }

    pub(crate) fn u32_at(&self, off: usize) -> Result<u32> {
        self.order.read_u32(self.bytes, off, "BNTX u32")
    }

    fn i32_at(&self, off: usize) -> Result<i32> {
        Ok(self.u32_at(off)?.cast_signed())
    }

    pub(crate) fn u64_at(&self, off: usize) -> Result<u64> {
        self.order.read_u64(self.bytes, off, "BNTX u64")
    }

    fn offset(&self, off: usize) -> Result<usize> {
        usize::try_from(self.u64_at(off)?)
            .map_err(|_| Error::overflow("BNTX: offset exceeds addressable range"))
    }

    fn slice(&self, off: usize, len: usize, ctx: &'static str) -> Result<&'a [u8]> {
        let end = off
            .checked_add(len)
            .filter(|&e| e <= self.bytes.len())
            .ok_or_else(|| Error::truncated(ctx, off, len, self.bytes.len().saturating_sub(off)))?;
        Ok(&self.bytes[off..end])
    }

    pub(crate) fn name_at(&self, off: usize) -> Result<String> {
        if off == 0 {
            return Ok(String::new());
        }
        Ok(self.name_at_len(off)?.0)
    }

    fn name_at_len(&self, off: usize) -> Result<(String, usize)> {
        let len = self.u16_at(off)? as usize;
        let s = self.slice(off + 2, len, "BNTX name")?;
        let s = std::str::from_utf8(s)
            .map_err(|_| Error::invalid_utf8("BNTX name"))?
            .to_owned();
        let consumed = align_up(2 + len + 1, 2);
        Ok((s, consumed))
    }

    fn wname_at_len(&self, off: usize) -> Result<(String, usize)> {
        let len = self.u16_at(off)? as usize;
        let mut units = Vec::with_capacity(len);
        for i in 0..len {
            units.push(self.u16_at(off + 2 + i * 2)?);
        }
        let s: String = char::decode_utf16(units)
            .map(|r| r.unwrap_or('\u{FFFD}'))
            .collect();
        let consumed = align_up(2 + len * 2 + 2, 2);
        Ok((s, consumed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(width: u32, height: u32, mip_count: u16, image_size: u32) -> TextureInfo {
        TextureInfo {
            flags: 9,
            dim: 2,
            tile_mode: 0,
            swizzle: 0,
            mip_count,
            sample_count: 1,
            format: ImageFormat::from_raw(0x0b01),
            gpu_access: 0x21,
            width,
            height,
            depth: 1,
            array_count: 1,
            texture_layout: 4,
            texture_layout2: 0,
            reserved: [0; 20],
            image_size,
            alignment: 512,
            channel_r: 2,
            channel_g: 3,
            channel_b: 4,
            channel_a: 5,
            surface_dim: 1,
        }
    }

    fn texture(name: &str, mip_offsets: Vec<u64>, image_data: Vec<u8>) -> Texture {
        let len = u32::try_from(image_data.len()).unwrap();
        Texture {
            name: name.to_owned(),
            info: info(8, 8, u16::try_from(mip_offsets.len().max(1)).unwrap(), len),
            mip_offsets,
            user_data: Vec::new(),
            image_data,
        }
    }

    fn sample(order: ByteOrder, textures: Vec<Texture>) -> Bntx {
        Bntx {
            byte_order: order,
            version: (4, 1, 0),
            alignment_log2: 9,
            target_address_size: 8,
            flag: 0,
            block_offset: 0,
            name: "Sample".to_owned(),
            platform: Platform::Nx,
            textures,
        }
    }

    fn assert_round_trip(bntx: &Bntx) {
        let bytes1 = bntx.write().expect("first write");
        let parsed1 = Bntx::parse(&bytes1).expect("first parse");
        let bytes2 = parsed1.write().expect("second write");
        let parsed2 = Bntx::parse(&bytes2).expect("second parse");

        assert_eq!(bytes1, bytes2, "serialization is not a fixed point");
        assert_eq!(parsed1, parsed2, "reparse is not a fixed point");

        assert_eq!(parsed1.byte_order, bntx.byte_order);
        assert_eq!(parsed1.version, bntx.version);
        assert_eq!(parsed1.name, bntx.name);
        assert_eq!(parsed1.platform, bntx.platform);
        assert_eq!(parsed1.textures.len(), bntx.textures.len());
        for (got, want) in parsed1.textures.iter().zip(&bntx.textures) {
            assert_eq!(got.name, want.name);
            assert_eq!(got.info, want.info);
            assert_eq!(got.mip_offsets, want.mip_offsets);
            assert_eq!(got.image_data, want.image_data);
        }
    }

    #[test]
    fn round_trip_single_texture_little_endian() {
        let bntx = sample(
            ByteOrder::Little,
            vec![texture("Icon", vec![0], (0..=255).collect())],
        );
        assert_round_trip(&bntx);
    }

    #[test]
    fn round_trip_big_endian() {
        let bntx = sample(
            ByteOrder::Big,
            vec![texture("Icon", vec![0], (0..=255).collect())],
        );
        assert_round_trip(&bntx);
    }

    #[test]
    fn round_trip_multi_texture_multi_mip() {
        let bntx = sample(
            ByteOrder::Little,
            vec![
                texture("First", vec![0, 256], (0..=255).cycle().take(512).collect()),
                texture("Second", vec![0], vec![0xAB; 64]),
                texture("Third", vec![0, 128, 192], vec![7; 256]),
            ],
        );
        assert_round_trip(&bntx);
    }

    #[test]
    fn round_trip_preserves_reserved_and_channel_fields() {
        let mut tex = texture("Icon", vec![0], vec![1; 128]);
        tex.info.reserved = [0xCD; 20];
        tex.info.channel_r = 1;
        tex.info.channel_g = 6;
        tex.info.channel_b = 7;
        tex.info.channel_a = 0;
        tex.info.texture_layout = 0x0007_0004;
        let bntx = sample(ByteOrder::Little, vec![tex]);
        assert_round_trip(&bntx);
    }

    #[test]
    fn write_rejects_user_data() {
        let mut bntx = sample(
            ByteOrder::Little,
            vec![texture("Icon", vec![0], vec![0; 16])],
        );
        bntx.textures[0].user_data.push(UserData {
            name: "meta".to_owned(),
            value: UserValue::Int32(vec![1, 2, 3]),
        });
        let err = bntx.write().unwrap_err();
        assert!(err.to_string().contains("user data"));
    }
}
