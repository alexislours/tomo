/// The numeric interpretation of an image format's channels (the low byte of a
/// packed [`ImageFormat`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TypeFormat {
    Unorm,
    Snorm,
    UInt,
    SInt,
    Float,
    Srgb,
    Depth,
    UScaled,
    SScaled,
    UFloat,
    Unknown(u8),
}

impl TypeFormat {
    #[must_use]
    pub(crate) fn from_u8(v: u8) -> Self {
        match v {
            0x1 => Self::Unorm,
            0x2 => Self::Snorm,
            0x3 => Self::UInt,
            0x4 => Self::SInt,
            0x5 => Self::Float,
            0x6 => Self::Srgb,
            0x7 => Self::Depth,
            0x8 => Self::UScaled,
            0x9 => Self::SScaled,
            0xA => Self::UFloat,
            other => Self::Unknown(other),
        }
    }

    #[must_use]
    pub(crate) fn is_srgb(self) -> bool {
        matches!(self, Self::Srgb)
    }

    #[must_use]
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Unorm => "Unorm",
            Self::Snorm => "Snorm",
            Self::UInt => "UInt",
            Self::SInt => "SInt",
            Self::Float => "Float",
            Self::Srgb => "SRGB",
            Self::Depth => "Depth",
            Self::UScaled => "UScaled",
            Self::SScaled => "SScaled",
            Self::UFloat => "UFloat",
            Self::Unknown(_) => "Unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    Zero,
    One,
    Red,
    Green,
    Blue,
    Alpha,
    Unknown(u8),
}

impl Channel {
    #[must_use]
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Zero,
            1 => Self::One,
            2 => Self::Red,
            3 => Self::Green,
            4 => Self::Blue,
            5 => Self::Alpha,
            other => Self::Unknown(other),
        }
    }

    #[must_use]
    pub(crate) fn select(self, rgba: [u8; 4]) -> Option<u8> {
        Some(match self {
            Self::Zero => 0,
            Self::One => u8::MAX,
            Self::Red => rgba[0],
            Self::Green => rgba[1],
            Self::Blue => rgba[2],
            Self::Alpha => rgba[3],
            Self::Unknown(_) => return None,
        })
    }
}

/// The channel layout / block compression of an image format (the high bits of
/// a packed [`ImageFormat`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChannelFormat {
    None,
    R8,
    R4G4B4A4,
    R5G5B5A1,
    A1B5G5R5,
    R5G6B5,
    B5G6R5,
    R8G8,
    R16,
    R8G8B8A8,
    B8G8R8A8,
    R9G9B9E5F,
    R10G10B10A2,
    R11G11B10F,
    R16G16,
    D24S8,
    R32,
    R16G16B16A16,
    D32FS8,
    R32G32,
    R32G32B32,
    R32G32B32A32,
    Bc1,
    Bc2,
    Bc3,
    Bc4,
    Bc5,
    Bc6H,
    Bc7U,
    Astc4x4,
    Astc5x4,
    Astc5x5,
    Astc6x5,
    Astc6x6,
    Astc8x5,
    Astc8x6,
    Astc8x8,
    Astc10x5,
    Astc10x6,
    Astc10x8,
    Astc10x10,
    Astc12x10,
    Astc12x12,
    B5G5R5A1,
}

impl ChannelFormat {
    #[must_use]
    pub(crate) fn from_u16(v: u16) -> Option<Self> {
        Some(match v {
            0x1 => Self::None,
            0x2 => Self::R8,
            0x3 => Self::R4G4B4A4,
            0x5 => Self::R5G5B5A1,
            0x6 => Self::A1B5G5R5,
            0x7 => Self::R5G6B5,
            0x8 => Self::B5G6R5,
            0x9 => Self::R8G8,
            0xA => Self::R16,
            0xB => Self::R8G8B8A8,
            0xC => Self::B8G8R8A8,
            0xD => Self::R9G9B9E5F,
            0xE => Self::R10G10B10A2,
            0xF => Self::R11G11B10F,
            0x12 => Self::R16G16,
            0x13 => Self::D24S8,
            0x14 => Self::R32,
            0x15 => Self::R16G16B16A16,
            0x16 => Self::D32FS8,
            0x17 => Self::R32G32,
            0x18 => Self::R32G32B32,
            0x19 => Self::R32G32B32A32,
            0x1A => Self::Bc1,
            0x1B => Self::Bc2,
            0x1C => Self::Bc3,
            0x1D => Self::Bc4,
            0x1E => Self::Bc5,
            0x1F => Self::Bc6H,
            0x20 => Self::Bc7U,
            0x2D => Self::Astc4x4,
            0x2E => Self::Astc5x4,
            0x2F => Self::Astc5x5,
            0x30 => Self::Astc6x5,
            0x31 => Self::Astc6x6,
            0x32 => Self::Astc8x5,
            0x33 => Self::Astc8x6,
            0x34 => Self::Astc8x8,
            0x35 => Self::Astc10x5,
            0x36 => Self::Astc10x6,
            0x37 => Self::Astc10x8,
            0x38 => Self::Astc10x10,
            0x39 => Self::Astc12x10,
            0x3A => Self::Astc12x12,
            0x3B => Self::B5G5R5A1,
            _ => return None,
        })
    }

    #[must_use]
    pub(crate) fn block_dim(self) -> (u32, u32) {
        match self {
            Self::Bc1
            | Self::Bc2
            | Self::Bc3
            | Self::Bc4
            | Self::Bc5
            | Self::Bc6H
            | Self::Bc7U
            | Self::Astc4x4 => (4, 4),
            Self::Astc5x4 => (5, 4),
            Self::Astc5x5 => (5, 5),
            Self::Astc6x5 => (6, 5),
            Self::Astc6x6 => (6, 6),
            Self::Astc8x5 => (8, 5),
            Self::Astc8x6 => (8, 6),
            Self::Astc8x8 => (8, 8),
            Self::Astc10x5 => (10, 5),
            Self::Astc10x6 => (10, 6),
            Self::Astc10x8 => (10, 8),
            Self::Astc10x10 => (10, 10),
            Self::Astc12x10 => (12, 10),
            Self::Astc12x12 => (12, 12),
            _ => (1, 1),
        }
    }

    #[must_use]
    pub(crate) fn bytes_per_block(self) -> u32 {
        match self {
            Self::None => 0,
            Self::R8 => 1,
            Self::R4G4B4A4
            | Self::R5G5B5A1
            | Self::A1B5G5R5
            | Self::R5G6B5
            | Self::B5G6R5
            | Self::R8G8
            | Self::R16
            | Self::B5G5R5A1 => 2,
            Self::R8G8B8A8
            | Self::B8G8R8A8
            | Self::R9G9B9E5F
            | Self::R10G10B10A2
            | Self::R11G11B10F
            | Self::R16G16
            | Self::D24S8
            | Self::R32
            | Self::D32FS8 => 4,
            Self::R16G16B16A16 | Self::R32G32 | Self::Bc1 | Self::Bc4 => 8,
            Self::R32G32B32 => 12,
            Self::R32G32B32A32
            | Self::Bc2
            | Self::Bc3
            | Self::Bc5
            | Self::Bc6H
            | Self::Bc7U
            | Self::Astc4x4
            | Self::Astc5x4
            | Self::Astc5x5
            | Self::Astc6x5
            | Self::Astc6x6
            | Self::Astc8x5
            | Self::Astc8x6
            | Self::Astc8x8
            | Self::Astc10x5
            | Self::Astc10x6
            | Self::Astc10x8
            | Self::Astc10x10
            | Self::Astc12x10
            | Self::Astc12x12 => 16,
        }
    }

    #[must_use]
    pub(crate) fn is_compressed(self) -> bool {
        self.block_dim() != (1, 1)
    }

    #[must_use]
    pub(crate) fn is_astc(self) -> bool {
        matches!(
            self,
            Self::Astc4x4
                | Self::Astc5x4
                | Self::Astc5x5
                | Self::Astc6x5
                | Self::Astc6x6
                | Self::Astc8x5
                | Self::Astc8x6
                | Self::Astc8x8
                | Self::Astc10x5
                | Self::Astc10x6
                | Self::Astc10x8
                | Self::Astc10x10
                | Self::Astc12x10
                | Self::Astc12x12
        )
    }

    #[must_use]
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::R8 => "R8",
            Self::R4G4B4A4 => "R4G4B4A4",
            Self::R5G5B5A1 => "R5G5B5A1",
            Self::A1B5G5R5 => "A1B5G5R5",
            Self::R5G6B5 => "R5G6B5",
            Self::B5G6R5 => "B5G6R5",
            Self::R8G8 => "R8G8",
            Self::R16 => "R16",
            Self::R8G8B8A8 => "R8G8B8A8",
            Self::B8G8R8A8 => "B8G8R8A8",
            Self::R9G9B9E5F => "R9G9B9E5F",
            Self::R10G10B10A2 => "R10G10B10A2",
            Self::R11G11B10F => "R11G11B10F",
            Self::R16G16 => "R16G16",
            Self::D24S8 => "D24S8",
            Self::R32 => "R32",
            Self::R16G16B16A16 => "R16G16B16A16",
            Self::D32FS8 => "D32FS8",
            Self::R32G32 => "R32G32",
            Self::R32G32B32 => "R32G32B32",
            Self::R32G32B32A32 => "R32G32B32A32",
            Self::Bc1 => "BC1",
            Self::Bc2 => "BC2",
            Self::Bc3 => "BC3",
            Self::Bc4 => "BC4",
            Self::Bc5 => "BC5",
            Self::Bc6H => "BC6H",
            Self::Bc7U => "BC7U",
            Self::Astc4x4 => "ASTC_4x4",
            Self::Astc5x4 => "ASTC_5x4",
            Self::Astc5x5 => "ASTC_5x5",
            Self::Astc6x5 => "ASTC_6x5",
            Self::Astc6x6 => "ASTC_6x6",
            Self::Astc8x5 => "ASTC_8x5",
            Self::Astc8x6 => "ASTC_8x6",
            Self::Astc8x8 => "ASTC_8x8",
            Self::Astc10x5 => "ASTC_10x5",
            Self::Astc10x6 => "ASTC_10x6",
            Self::Astc10x8 => "ASTC_10x8",
            Self::Astc10x10 => "ASTC_10x10",
            Self::Astc12x10 => "ASTC_12x10",
            Self::Astc12x12 => "ASTC_12x12",
            Self::B5G5R5A1 => "B5G5R5A1",
        }
    }
}

/// A texture's pixel format, packing a [`ChannelFormat`] and [`TypeFormat`]
/// into the raw value stored in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImageFormat {
    raw: u32,
}

impl ImageFormat {
    /// Wraps a raw packed format value as read from a BNTX file.
    #[must_use]
    pub fn from_raw(raw: u32) -> Self {
        Self { raw }
    }

    /// The raw packed format value.
    #[must_use]
    pub fn raw(self) -> u32 {
        self.raw
    }

    #[must_use]
    pub(crate) fn channel(self) -> Option<ChannelFormat> {
        ChannelFormat::from_u16(u16::try_from(self.raw >> 8).unwrap_or(0))
    }

    #[must_use]
    pub(crate) fn ty(self) -> TypeFormat {
        TypeFormat::from_u8((self.raw & 0xFF) as u8)
    }

    #[must_use]
    pub(crate) fn is_srgb(self) -> bool {
        self.ty().is_srgb()
    }

    /// A readable name such as `Bc7U_Srgb`, or the hex value if unrecognized.
    #[must_use]
    pub fn name(self) -> String {
        match self.channel() {
            Some(ch) => format!("{}_{}", ch.name(), self.ty().name()),
            None => format!("{:#06x}", self.raw),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_packed_value() {
        let f = ImageFormat::from_raw(0x2f06);
        assert_eq!(f.channel(), Some(ChannelFormat::Astc5x5));
        assert_eq!(f.ty(), TypeFormat::Srgb);
        assert_eq!(f.channel().unwrap().block_dim(), (5, 5));
        assert_eq!(f.channel().unwrap().bytes_per_block(), 16);
        assert_eq!(f.name(), "ASTC_5x5_SRGB");
    }

    #[test]
    fn bc_block_sizes() {
        assert_eq!(ChannelFormat::Bc1.bytes_per_block(), 8);
        assert_eq!(ChannelFormat::Bc7U.bytes_per_block(), 16);
        assert_eq!(ChannelFormat::Bc1.block_dim(), (4, 4));
        assert!(ChannelFormat::Astc8x8.is_astc());
    }

    #[test]
    fn rgba8_is_uncompressed() {
        let f = ImageFormat::from_raw(0x0b01);
        assert_eq!(f.channel(), Some(ChannelFormat::R8G8B8A8));
        assert!(!f.channel().unwrap().is_compressed());
        assert_eq!(f.channel().unwrap().bytes_per_block(), 4);
    }
}
