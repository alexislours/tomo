#[inline]
fn round_up(n: u32, a: u32) -> u32 {
    n.div_ceil(a) * a
}

#[inline]
fn block_linear_address(x: u32, y: u32, width: u32, bpp: u32, block_height: u32) -> u64 {
    let width_in_gobs = (width * bpp).div_ceil(64);
    let gob = u64::from((y / (8 * block_height)) * 512 * block_height * width_in_gobs)
        + u64::from((x * bpp / 64) * 512 * block_height)
        + u64::from((y % (8 * block_height) / 8) * 512);
    let xb = x * bpp;
    gob + u64::from(
        ((xb % 64) / 32) * 256
            + ((y % 8) / 2) * 64
            + ((xb % 32) / 16) * 32
            + (y % 2) * 16
            + (xb % 16),
    )
}

#[must_use]
pub(crate) fn swizzled_surface_size(
    width_blocks: u32,
    height_blocks: u32,
    bpp: u32,
    block_height: u32,
    linear: bool,
    round_pitch: bool,
) -> usize {
    if linear {
        let mut pitch = width_blocks * bpp;
        if round_pitch {
            pitch = round_up(pitch, 32);
        }
        (pitch * height_blocks) as usize
    } else {
        let pitch = round_up(width_blocks * bpp, 64);
        (pitch * round_up(height_blocks, block_height * 8)) as usize
    }
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
    } = *surface;
    let to_swizzle = dir == Direction::ToSwizzled;
    let width_blocks = width.div_ceil(blk_width);
    let height_blocks = height.div_ceil(blk_height);
    let linear = tile_mode == 1;
    let round_pitch = linear;

    let surf_size = swizzled_surface_size(
        width_blocks,
        height_blocks,
        bpp,
        block_height,
        linear,
        round_pitch,
    );
    let linear_size = (width_blocks * height_blocks * bpp) as usize;

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
        let p = width_blocks * bpp;
        if round_pitch { round_up(p, 32) } else { p }
    } else {
        0
    };

    let mut out = vec![0u8; if to_swizzle { surf_size } else { linear_size }];

    for y in 0..height_blocks {
        for x in 0..width_blocks {
            let swizzled = if linear {
                (y * pitch + x * bpp) as usize
            } else {
                usize::try_from(block_linear_address(x, y, width_blocks, bpp, block_height))
                    .unwrap_or(usize::MAX)
            };
            let row_major = ((y * width_blocks + x) * bpp) as usize;

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
        };
        let swizzled = convert(&surface, &linear, Direction::ToSwizzled).unwrap();
        let back = convert(&surface, &swizzled, Direction::ToLinear).unwrap();
        assert_eq!(back, linear);
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
