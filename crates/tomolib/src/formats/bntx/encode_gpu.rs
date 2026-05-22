#![allow(clippy::unnecessary_wraps)]

use crate::formats::bntx::format::ChannelFormat;
use crate::formats::bntx::image::RgbaImage;
use crate::{Error, Result};

const ASTC_PRESET: ctt_astcenc::Preset = ctt_astcenc::Preset::Medium;

pub(crate) fn encode_astc(img: &RgbaImage, ch: ChannelFormat, srgb: bool) -> Result<Vec<u8>> {
    use ctt_astcenc::{Context, Flags, Profile, Swizzle, bindings, config_init};

    let (bw, bh) = ch.block_dim();
    let profile = if srgb { Profile::LdrSrgb } else { Profile::Ldr };

    let config = config_init(profile, bw, bh, 1, ASTC_PRESET, Flags::USE_DECODE_UNORM8)
        .map_err(|e| Error::decode(format!("ASTC config: {e}")))?;
    let mut ctx = Context::new(&config).map_err(|e| Error::decode(format!("ASTC ctx: {e}")))?;

    let blocks_x = img.width.div_ceil(bw) as usize;
    let blocks_y = img.height.div_ceil(bh) as usize;
    let mut out = vec![0u8; blocks_x * blocks_y * 16];

    let mut buf = img.data.clone();
    let mut plane: *mut core::ffi::c_void = buf.as_mut_ptr().cast();
    let mut image = bindings::astcenc_image {
        dim_x: img.width,
        dim_y: img.height,
        dim_z: 1,
        data_type: bindings::astcenc_type_ASTCENC_TYPE_U8,
        data: &raw mut plane,
    };

    ctx.compress(&mut image, Swizzle::IDENTITY, &mut out)
        .map_err(|e| Error::decode(format!("ASTC encode: {e}")))?;
    Ok(out)
}

pub(crate) fn encode_bc4(img: &RgbaImage) -> Result<Vec<u8>> {
    let r: Vec<u8> = img.data.chunks_exact(4).map(|p| p[0]).collect();
    let surf = intel_tex_2::RSurface {
        width: img.width,
        height: img.height,
        stride: img.width,
        data: &r,
    };
    Ok(intel_tex_2::bc4::compress_blocks(&surf))
}

pub(crate) fn encode_bc5(img: &RgbaImage) -> Result<Vec<u8>> {
    let rg: Vec<u8> = img
        .data
        .chunks_exact(4)
        .flat_map(|p| [p[0], p[1]])
        .collect();
    let surf = intel_tex_2::RgSurface {
        width: img.width,
        height: img.height,
        stride: img.width * 2,
        data: &rg,
    };
    Ok(intel_tex_2::bc5::compress_blocks(&surf))
}

pub(crate) fn encode_bc7(img: &RgbaImage) -> Result<Vec<u8>> {
    let surf = intel_tex_2::RgbaSurface {
        width: img.width,
        height: img.height,
        stride: img.width * 4,
        data: &img.data,
    };
    Ok(intel_tex_2::bc7::compress_blocks(
        &intel_tex_2::bc7::alpha_basic_settings(),
        &surf,
    ))
}
