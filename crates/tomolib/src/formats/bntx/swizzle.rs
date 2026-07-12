#[inline]
fn round_up(n: u128, a: u128) -> u128 {
    n.div_ceil(a) * a
}

#[inline]
fn block_linear_address(x: u32, y: u32, width: u32, bpp: u32, block_height: u32) -> u64 {
    let bh = u64::from(block_height);
    let width_in_gobs = (u64::from(width) * u64::from(bpp)).div_ceil(64);
    let gob = u64::from(y / (8 * block_height)) * 512 * bh * width_in_gobs
        + u64::from(x) * u64::from(bpp) / 64 * 512 * bh
        + u64::from(y % (8 * block_height) / 8) * 512;
    let xb = u64::from(x) * u64::from(bpp);
    gob + ((xb % 64) / 32) * 256
        + u64::from((y % 8) / 2) * 64
        + ((xb % 32) / 16) * 32
        + u64::from(y % 2) * 16
        + (xb % 16)
}

#[must_use]
pub(crate) fn swizzled_surface_size(
    width_blocks: u32,
    height_blocks: u32,
    bpp: u32,
    block_height: u32,
    linear: bool,
    round_pitch: bool,
) -> Option<usize> {
    let width_bytes = u128::from(width_blocks) * u128::from(bpp);
    let size = if linear {
        let pitch = if round_pitch {
            round_up(width_bytes, 32)
        } else {
            width_bytes
        };
        pitch * u128::from(height_blocks)
    } else {
        round_up(width_bytes, 64)
            * round_up(u128::from(height_blocks), u128::from(block_height) * 8)
    };
    usize::try_from(size).ok()
}

#[must_use]
pub(crate) fn mip_block_height(height_blocks: u32, base_block_height: u32) -> u32 {
    let mut bh = base_block_height;
    while bh > 1 && height_blocks.div_ceil(8) <= bh / 2 {
        bh /= 2;
    }
    bh
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Surface {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) blk_width: u32,
    pub(crate) blk_height: u32,
    pub(crate) bpp: u32,
    pub(crate) tile_mode: u16,
    pub(crate) block_height: u32,
    pub(crate) round_pitch: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    ToLinear,
    ToSwizzled,
}

pub(crate) fn convert(surface: &Surface, data: &[u8], dir: Direction) -> crate::Result<Vec<u8>> {
    let Surface {
        width,
        height,
        blk_width,
        blk_height,
        bpp,
        tile_mode,
        block_height,
        round_pitch,
    } = *surface;
    let to_swizzle = dir == Direction::ToSwizzled;
    let width_blocks = width.div_ceil(blk_width);
    let height_blocks = height.div_ceil(blk_height);
    let linear = tile_mode == 1;

    let surf_size = swizzled_surface_size(
        width_blocks,
        height_blocks,
        bpp,
        block_height,
        linear,
        round_pitch,
    )
    .ok_or_else(|| crate::Error::overflow("BNTX: surface size exceeds addressable range"))?;
    let linear_size =
        usize::try_from(u128::from(width_blocks) * u128::from(height_blocks) * u128::from(bpp))
            .map_err(|_| crate::Error::overflow("BNTX: surface size exceeds addressable range"))?;

    let src_size = if to_swizzle { linear_size } else { surf_size };
    if data.len() < src_size {
        return Err(crate::Error::truncated(
            "BNTX swizzle surface",
            0,
            src_size,
            data.len(),
        ));
    }

    let bpp_us = bpp as usize;
    let pitch = if linear {
        let p = u64::from(width_blocks) * u64::from(bpp);
        if round_pitch { p.div_ceil(32) * 32 } else { p }
    } else {
        0
    };

    let mut out = vec![0u8; if to_swizzle { surf_size } else { linear_size }];

    for y in 0..height_blocks {
        for x in 0..width_blocks {
            let swizzled = if linear {
                usize::try_from(u64::from(y) * pitch + u64::from(x) * u64::from(bpp))
                    .unwrap_or(usize::MAX)
            } else {
                usize::try_from(block_linear_address(x, y, width_blocks, bpp, block_height))
                    .unwrap_or(usize::MAX)
            };
            let row_major = usize::try_from(
                (u64::from(y) * u64::from(width_blocks) + u64::from(x)) * u64::from(bpp),
            )
            .unwrap_or(usize::MAX);

            let (src, dst, base) = if to_swizzle {
                (row_major, swizzled, surf_size)
            } else {
                (swizzled, row_major, linear_size)
            };
            if src + bpp_us <= data.len() && dst + bpp_us <= base {
                out[dst..dst + bpp_us].copy_from_slice(&data[src..src + bpp_us]);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_swizzle_is_identity() {
        let (w, h, bw, bh, bpp, block_height) = (64u32, 64u32, 4u32, 4u32, 8u32, 2u32);
        let wb = w.div_ceil(bw);
        let hb = h.div_ceil(bh);
        let linear: Vec<u8> = (0..(wb * hb * bpp)).map(|i| (i % 251) as u8).collect();

        let surface = Surface {
            width: w,
            height: h,
            blk_width: bw,
            blk_height: bh,
            bpp,
            tile_mode: 0,
            block_height,
            round_pitch: false,
        };
        let swizzled = convert(&surface, &linear, Direction::ToSwizzled).unwrap();
        let back = convert(&surface, &swizzled, Direction::ToLinear).unwrap();
        assert_eq!(back, linear);
    }

    #[test]
    fn linear_tight_pitch_is_identity() {
        let surface = Surface {
            width: 6,
            height: 6,
            blk_width: 1,
            blk_height: 1,
            bpp: 4,
            tile_mode: 1,
            block_height: 1,
            round_pitch: false,
        };
        let linear: Vec<u8> = (0..144u32).map(|i| (i % 251) as u8).collect();
        let swizzled = convert(&surface, &linear, Direction::ToSwizzled).unwrap();
        assert_eq!(swizzled, linear);

        let rounded = Surface {
            round_pitch: true,
            ..surface
        };
        assert!(convert(&rounded, &linear, Direction::ToLinear).is_err());
        let padded = convert(&rounded, &linear, Direction::ToSwizzled).unwrap();
        assert_eq!(padded.len(), 192);
        let back = convert(&rounded, &padded, Direction::ToLinear).unwrap();
        assert_eq!(back, linear);
    }

    #[test]
    fn convert_rejects_overflowing_dimensions() {
        for tile_mode in [0u16, 1] {
            let surface = Surface {
                width: u32::MAX,
                height: u32::MAX,
                blk_width: 1,
                blk_height: 1,
                bpp: 4,
                tile_mode,
                block_height: 1,
                round_pitch: tile_mode == 1,
            };
            assert!(convert(&surface, &[0u8; 64], Direction::ToLinear).is_err());
            assert!(convert(&surface, &[0u8; 64], Direction::ToSwizzled).is_err());
        }
    }

    #[test]
    fn deswizzle_rejects_truncated_input() {
        let surface = Surface {
            width: 64,
            height: 64,
            blk_width: 4,
            blk_height: 4,
            bpp: 8,
            tile_mode: 0,
            block_height: 2,
            round_pitch: false,
        };
        let too_short = vec![0u8; 16];
        assert!(convert(&surface, &too_short, Direction::ToLinear).is_err());
    }

    #[test]
    fn mip_block_height_shrinks() {
        assert_eq!(mip_block_height(64, 8), 8);
        assert_eq!(mip_block_height(4, 8), 1);
        assert_eq!(mip_block_height(1, 16), 1);
    }
}
