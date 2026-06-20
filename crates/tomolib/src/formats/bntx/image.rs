use crate::formats::bntx::format::{Channel, ChannelFormat};
use crate::formats::bntx::{Texture, TextureInfo, swizzle};
use crate::{Error, Result};

/// A decoded image as tightly packed 8-bit RGBA pixels (`width * height * 4`
/// bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelResolve {
    Raw,
    Resolved,
}

/// Decodes one mip level of a texture to RGBA, un-swizzling and decompressing
/// as needed. The raw stored channels are returned untouched; use
/// [`decode_texture_rgba_with`] to apply the texture's channel selectors.
pub fn decode_texture_rgba(tex: &Texture, mip: usize) -> Result<RgbaImage> {
    decode_texture_rgba_with(tex, mip, ChannelResolve::Raw)
}

/// Decodes one mip level of a texture to RGBA, optionally resolving the
/// texture's channel selectors (constants and swizzles) for an in-game view.
pub fn decode_texture_rgba_with(
    tex: &Texture,
    mip: usize,
    resolve: ChannelResolve,
) -> Result<RgbaImage> {
    let info = &tex.info;
    let ch = info.format.channel().ok_or_else(|| {
        Error::unsupported(format!("BNTX: unknown image format {}", info.format.name()))
    })?;

    let mip_count = info.mip_count.max(1) as usize;
    if mip >= mip_count {
        return Err(Error::out_of_range("BNTX mip", mip, mip_count));
    }

    let width = (info.width >> mip).max(1);
    let height = (info.height >> mip).max(1);

    let (start, end) = tex.mip_byte_range(mip);
    let swizzled = tex
        .image_data
        .get(start..end)
        .ok_or_else(|| Error::malformed("BNTX: mip offset out of range"))?;

    let surface = mip_surface(info, ch, width, height);
    let linear = swizzle::convert(&surface, swizzled, swizzle::Direction::ToLinear)?;

    let mut data = decode_blocks(ch, &linear, width, height)?;
    if resolve == ChannelResolve::Resolved {
        apply_channel_swizzle(&mut data, info);
    }
    Ok(RgbaImage {
        width,
        height,
        data,
    })
}

fn apply_channel_swizzle(data: &mut [u8], info: &TextureInfo) {
    let map = [
        Channel::from_u8(info.channel_r),
        Channel::from_u8(info.channel_g),
        Channel::from_u8(info.channel_b),
        Channel::from_u8(info.channel_a),
    ];
    for px in data.chunks_exact_mut(4) {
        let rgba = [px[0], px[1], px[2], px[3]];
        for (i, ch) in map.iter().enumerate() {
            if let Some(v) = ch.select(rgba) {
                px[i] = v;
            }
        }
    }
}

fn mip_surface(info: &TextureInfo, ch: ChannelFormat, width: u32, height: u32) -> swizzle::Surface {
    let (blk_width, blk_height) = ch.block_dim();
    let base_block_height = 1u32 << info.block_height_log2();
    let block_height = swizzle::mip_block_height(height.div_ceil(blk_height), base_block_height);
    swizzle::Surface {
        width,
        height,
        blk_width,
        blk_height,
        bpp: ch.bytes_per_block(),
        tile_mode: info.tile_mode,
        block_height,
    }
}

fn decode_blocks(ch: ChannelFormat, data: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    let (w, h) = (width as usize, height as usize);
    let px = w * h;

    if ch.is_compressed() {
        let mut buf = vec![0u32; px];
        let r: core::result::Result<(), &'static str> = match ch {
            ChannelFormat::Bc1 => texture2ddecoder::decode_bc1(data, w, h, &mut buf),
            ChannelFormat::Bc2 => texture2ddecoder::decode_bc2(data, w, h, &mut buf),
            ChannelFormat::Bc3 => texture2ddecoder::decode_bc3(data, w, h, &mut buf),
            ChannelFormat::Bc4 => texture2ddecoder::decode_bc4(data, w, h, &mut buf),
            ChannelFormat::Bc5 => texture2ddecoder::decode_bc5(data, w, h, &mut buf),
            ChannelFormat::Bc6H => texture2ddecoder::decode_bc6_unsigned(data, w, h, &mut buf),
            ChannelFormat::Bc7U => texture2ddecoder::decode_bc7(data, w, h, &mut buf),
            _ if ch.is_astc() => {
                let (bw, bh) = ch.block_dim();
                texture2ddecoder::decode_astc(data, w, h, bw as usize, bh as usize, &mut buf)
            }
            _ => {
                return Err(Error::unsupported(format!(
                    "BNTX: cannot decode {}",
                    ch.name()
                )));
            }
        };
        r.map_err(|e| Error::decode(format!("BNTX: block decode failed: {e}")))?;
        Ok(bgra_u32_to_rgba8(&buf))
    } else {
        decode_uncompressed(ch, data, px)
    }
}

fn bgra_u32_to_rgba8(buf: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(buf.len() * 4);
    for &c in buf {
        let [b, g, r, a] = c.to_le_bytes();
        out.extend_from_slice(&[r, g, b, a]);
    }
    out
}

fn decode_uncompressed(ch: ChannelFormat, data: &[u8], px: usize) -> Result<Vec<u8>> {
    let bpp = ch.bytes_per_block() as usize;
    if data.len() < px * bpp {
        return Err(Error::malformed("BNTX: uncompressed data too short"));
    }
    let mut out = vec![0u8; px * 4];
    for i in 0..px {
        let src = &data[i * bpp..i * bpp + bpp];
        let [r, g, b, a] = match ch {
            ChannelFormat::R8G8B8A8 => [src[0], src[1], src[2], src[3]],
            ChannelFormat::B8G8R8A8 => [src[2], src[1], src[0], src[3]],
            ChannelFormat::R8G8 => [src[0], src[1], 0, 255],
            ChannelFormat::R8 => [src[0], src[0], src[0], 255],
            ChannelFormat::R5G6B5 => {
                let v = u16::from_le_bytes([src[0], src[1]]);
                [
                    expand(u32::from(v >> 11) & 0x1F, 5),
                    expand(u32::from(v >> 5) & 0x3F, 6),
                    expand(u32::from(v) & 0x1F, 5),
                    255,
                ]
            }
            _ => {
                return Err(Error::malformed(format!(
                    "BNTX: uncompressed format {} not yet supported for decode",
                    ch.name()
                )));
            }
        };
        out[i * 4..i * 4 + 4].copy_from_slice(&[r, g, b, a]);
    }
    Ok(out)
}

#[inline]
fn expand(v: u32, bits: u32) -> u8 {
    let max = (1u32 << bits) - 1;
    u8::try_from((v * 255 + max / 2) / max).unwrap_or(255)
}

/// Whether [`encode_mips_swizzled`] can produce data in this texture's format.
#[must_use]
pub fn can_encode(info: &TextureInfo) -> bool {
    let Some(ch) = info.format.channel() else {
        return false;
    };
    if squish_format(ch).is_some() || uncompressed_encodable(ch) {
        return true;
    }
    #[cfg(feature = "image-encode-gpu")]
    {
        ch.is_astc()
            || matches!(
                ch,
                ChannelFormat::Bc4 | ChannelFormat::Bc5 | ChannelFormat::Bc7U
            )
    }
    #[cfg(not(feature = "image-encode-gpu"))]
    {
        false
    }
}

/// Encodes an RGBA image into `tex`'s format, swizzled and ready to replace the
/// texture's image data. The image must match the texture's mip 0 dimensions.
pub fn encode_mips_swizzled(img: &RgbaImage, tex: &Texture) -> Result<Vec<u8>> {
    let info = &tex.info;
    if (img.width, img.height) != (info.width, info.height) {
        return Err(Error::malformed(format!(
            "PNG is {}x{} but texture mip 0 is {}x{}",
            img.width, img.height, info.width, info.height
        )));
    }
    let ch = info.format.channel().ok_or_else(|| {
        Error::unsupported(format!("BNTX: unknown format {}", info.format.name()))
    })?;

    let srgb = info.format.is_srgb();

    let mut out = tex.image_data.clone();
    for mip in 0..info.mip_count.max(1) as usize {
        let w = (info.width >> mip).max(1);
        let h = (info.height >> mip).max(1);
        let level = downscale_box(img, w, h);
        let linear = encode_blocks(ch, &level, srgb)?;

        let surface = mip_surface(info, ch, w, h);
        let swizzled = swizzle::convert(&surface, &linear, swizzle::Direction::ToSwizzled)?;

        let (start, end) = tex.mip_byte_range(mip);
        let region = out
            .get_mut(start..end)
            .ok_or_else(|| Error::malformed("BNTX: mip offset out of range"))?;
        if swizzled.len() > region.len() {
            return Err(Error::malformed(format!(
                "BNTX: re-encoded mip {mip} is {} bytes, larger than its {}-byte region",
                swizzled.len(),
                region.len()
            )));
        }
        region[..swizzled.len()].copy_from_slice(&swizzled);
    }
    Ok(out)
}

fn downscale_box(src: &RgbaImage, tw: u32, th: u32) -> RgbaImage {
    if (tw, th) == (src.width, src.height) {
        return src.clone();
    }
    let src_w = src.width as usize;
    let src_h = src.height as usize;
    let dst_w = tw as usize;
    let dst_h = th as usize;
    let mut data = vec![0u8; dst_w * dst_h * 4];
    for ty in 0..dst_h {
        let y0 = ty * src_h / dst_h;
        let y1 = ((ty + 1) * src_h / dst_h).max(y0 + 1);
        for tx in 0..dst_w {
            let x0 = tx * src_w / dst_w;
            let x1 = ((tx + 1) * src_w / dst_w).max(x0 + 1);
            let mut acc = [0u64; 4];
            let mut n = 0u64;
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let i = (sy * src_w + sx) * 4;
                    for (a, &v) in acc.iter_mut().zip(&src.data[i..i + 4]) {
                        *a += u64::from(v);
                    }
                    n += 1;
                }
            }
            let o = (ty * dst_w + tx) * 4;
            for (d, a) in data[o..o + 4].iter_mut().zip(acc) {
                *d = u8::try_from(a / n).unwrap_or(u8::MAX);
            }
        }
    }
    RgbaImage {
        width: tw,
        height: th,
        data,
    }
}

fn encode_blocks(ch: ChannelFormat, img: &RgbaImage, srgb: bool) -> Result<Vec<u8>> {
    let (w, h) = (img.width as usize, img.height as usize);
    if let Some(fmt) = squish_format(ch) {
        let mut out = vec![0u8; fmt.compressed_size(w, h)];
        fmt.compress(&img.data, w, h, squish::Params::default(), &mut out);
        return Ok(out);
    }
    #[cfg(feature = "image-encode-gpu")]
    {
        use super::encode_gpu;
        if ch.is_astc() {
            return encode_gpu::encode_astc(img, ch, srgb);
        }
        match ch {
            ChannelFormat::Bc4 => return Ok(encode_gpu::encode_bc4(img)),
            ChannelFormat::Bc5 => return Ok(encode_gpu::encode_bc5(img)),
            ChannelFormat::Bc7U => return Ok(encode_gpu::encode_bc7(img)),
            _ => {}
        }
    }
    #[cfg(not(feature = "image-encode-gpu"))]
    let _ = srgb;
    encode_uncompressed(ch, img)
}

fn squish_format(ch: ChannelFormat) -> Option<squish::Format> {
    match ch {
        ChannelFormat::Bc1 => Some(squish::Format::Bc1),
        ChannelFormat::Bc2 => Some(squish::Format::Bc2),
        ChannelFormat::Bc3 => Some(squish::Format::Bc3),
        _ => None,
    }
}

fn uncompressed_encodable(ch: ChannelFormat) -> bool {
    matches!(
        ch,
        ChannelFormat::R8G8B8A8 | ChannelFormat::B8G8R8A8 | ChannelFormat::R8G8 | ChannelFormat::R8
    )
}

fn encode_uncompressed(ch: ChannelFormat, img: &RgbaImage) -> Result<Vec<u8>> {
    let px = (img.width * img.height) as usize;
    let mut out = Vec::with_capacity(px * ch.bytes_per_block() as usize);
    for p in img.data.chunks_exact(4) {
        let [r, g, b, a] = [p[0], p[1], p[2], p[3]];
        match ch {
            ChannelFormat::R8G8B8A8 => out.extend_from_slice(&[r, g, b, a]),
            ChannelFormat::B8G8R8A8 => out.extend_from_slice(&[b, g, r, a]),
            ChannelFormat::R8G8 => out.extend_from_slice(&[r, g]),
            ChannelFormat::R8 => out.push(r),
            _ => {
                return Err(Error::malformed(format!(
                    "BNTX: cannot encode to {} (try --raw)",
                    ch.name()
                )));
            }
        }
    }
    Ok(out)
}

/// Encodes an [`RgbaImage`] as a PNG.
pub fn rgba_to_png(img: &RgbaImage) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, img.width, img.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        enc.set_filter(png::Filter::NoFilter);
        let mut w = enc
            .write_header()
            .map_err(|e| Error::decode(format!("PNG header: {e}")))?;
        w.write_image_data(&img.data)
            .map_err(|e| Error::decode(format!("PNG data: {e}")))?;
        w.finish()
            .map_err(|e| Error::decode(format!("PNG finish: {e}")))?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn rgba8_info(w: u32, h: u32) -> TextureInfo {
        TextureInfo {
            flags: 9,
            dim: 2,
            tile_mode: 0,
            swizzle: 0,
            mip_count: 1,
            sample_count: 1,
            format: crate::formats::bntx::ImageFormat::from_raw(0x0b01),
            gpu_access: 0x21,
            width: w,
            height: h,
            depth: 1,
            array_count: 1,
            texture_layout: 4,
            texture_layout2: 0,
            reserved: [0; 20],
            image_size: w * h * 4,
            alignment: 512,
            channel_r: 2,
            channel_g: 3,
            channel_b: 4,
            channel_a: 5,
            surface_dim: 1,
        }
    }

    fn encode_single_mip(img: &RgbaImage, info: &TextureInfo) -> Vec<u8> {
        let ch = info.format.channel().expect("known format");
        let (bw, bh) = ch.block_dim();
        let block_height =
            swizzle::mip_block_height(info.height.div_ceil(bh), 1u32 << info.block_height_log2());
        let size = swizzle::swizzled_surface_size(
            info.width.div_ceil(bw),
            info.height.div_ceil(bh),
            ch.bytes_per_block(),
            block_height,
            info.tile_mode == 1,
            info.tile_mode == 1,
        );
        let mut single = info.clone();
        single.mip_count = 1;
        let tex = Texture {
            name: "t".into(),
            info: single,
            mip_offsets: vec![0],
            user_data: vec![],
            image_data: vec![0u8; size],
        };
        encode_mips_swizzled(img, &tex).expect("encode")
    }

    #[test]
    fn uncompressed_encode_swizzle_round_trips() {
        let info = rgba8_info(64, 64);
        let mut data = vec![0u8; 64 * 64 * 4];
        for (i, b) in data.iter_mut().enumerate() {
            *b = u8::try_from(i % 256).unwrap();
        }
        let img = RgbaImage {
            width: 64,
            height: 64,
            data,
        };
        let swizzled = encode_single_mip(&img, &info);
        let tex = Texture {
            name: "t".into(),
            info: info.clone(),
            mip_offsets: vec![0],
            user_data: vec![],
            image_data: swizzled,
        };
        let back = decode_texture_rgba(&tex, 0).expect("decode");
        assert_eq!(back, img, "uncompressed should round-trip losslessly");
    }

    #[test]
    fn channel_selectors_resolve_to_mask() {
        let mut info = rgba8_info(4, 4);
        info.channel_r = 1;
        info.channel_g = 1;
        info.channel_b = 1;
        info.channel_a = 2;
        let data: Vec<u8> = (0..4u32 * 4 * 4)
            .map(|i| u8::try_from(i % 256).unwrap())
            .collect();
        let img = RgbaImage {
            width: 4,
            height: 4,
            data: data.clone(),
        };
        let swizzled = encode_single_mip(&img, &info);
        let tex = Texture {
            name: "t".into(),
            info,
            mip_offsets: vec![0],
            user_data: vec![],
            image_data: swizzled,
        };
        let resolved = decode_texture_rgba_with(&tex, 0, ChannelResolve::Resolved).expect("decode");
        for (px, src) in resolved.data.chunks_exact(4).zip(data.chunks_exact(4)) {
            assert_eq!(px, &[255, 255, 255, src[0]]);
        }
        let raw = decode_texture_rgba(&tex, 0).expect("decode");
        assert_eq!(
            raw.data, data,
            "without resolve the stored data is untouched"
        );
    }

    #[test]
    fn bc1_encode_is_swizzle_invertible() {
        let mut info = rgba8_info(64, 32);
        info.format = crate::formats::bntx::ImageFormat::from_raw(0x1a01);
        let img = RgbaImage {
            width: 64,
            height: 32,
            data: (0..64u32 * 32 * 4)
                .map(|i| u8::try_from(i * 7 % 256).unwrap())
                .collect(),
        };
        let swizzled = encode_single_mip(&img, &info);
        assert!(!swizzled.is_empty());
        let tex = Texture {
            name: "t".into(),
            info,
            mip_offsets: vec![0],
            user_data: vec![],
            image_data: swizzled,
        };
        let back = decode_texture_rgba(&tex, 0).unwrap();
        assert_eq!((back.width, back.height), (64, 32));
    }

    #[test]
    fn downscale_box_halves_and_averages() {
        let src = RgbaImage {
            width: 2,
            height: 2,
            data: vec![
                0, 0, 0, 0, 40, 80, 120, 160, 40, 80, 120, 160, 80, 160, 240, 248,
            ],
        };
        let out = downscale_box(&src, 1, 1);
        assert_eq!((out.width, out.height), (1, 1));
        assert_eq!(out.data, vec![40, 80, 120, 142]);
    }
}

/// Decodes a PNG into an [`RgbaImage`].
pub fn png_to_rgba(bytes: &[u8]) -> Result<RgbaImage> {
    let dec = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = dec
        .read_info()
        .map_err(|e| Error::decode(format!("PNG decode: {e}")))?;
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| Error::decode("PNG decode: image dimensions too large".to_string()))?;
    let mut buf = vec![0u8; size];
    let frame = reader
        .next_frame(&mut buf)
        .map_err(|e| Error::decode(format!("PNG frame: {e}")))?;
    buf.truncate(frame.buffer_size());

    let (width, height) = (frame.width, frame.height);
    let data = match frame.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&v| [v, v, v, 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Indexed => {
            return Err(Error::unsupported("PNG: indexed color is not supported"));
        }
    };
    Ok(RgbaImage {
        width,
        height,
        data,
    })
}

#[cfg(all(test, feature = "image-encode-gpu"))]
mod gpu_tests {
    use super::*;

    #[test]
    fn can_encode_matrix() {
        use crate::formats::bntx::ImageFormat;
        let check = |raw: u32, expect: bool| {
            let mut info = super::tests::rgba8_info(8, 8);
            info.format = ImageFormat::from_raw(raw);
            assert_eq!(
                can_encode(&info),
                expect,
                "can_encode mismatch for {}",
                info.format.name()
            );
        };
        check(0x2f06, true);
        check(0x2d01, true);
        check(0x1d01, true);
        check(0x1e02, true);
        check(0x2001, true);
        check(0x1f0a, false);
        check(0x1505, false);
    }
}
