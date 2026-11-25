//! Favicons for the wire: any pixel layout Servo reports, as small RGBA.
//!
//! A tab draws 16 logical pixels, so a favicon is box-averaged down to at most
//! `wire::MAX_ICON_SIDE` and sent as straight RGBA. Everything about the input
//! is checked before it is read: a page chooses its icon, so its size and the
//! length of its data are untrusted.

use vkore_servo_wire as wire;

/// The most a source icon may measure on a side. Pages ship icons of 16 to a
/// few hundred pixels; refusing larger ones bounds the work done for one.
const MAX_SOURCE_SIDE: u32 = 1024;

/// How a source icon lays out its pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Gray,
    GrayAlpha,
    Rgb,
    Rgba,
    Bgra,
}

impl Layout {
    fn bytes(self) -> usize {
        match self {
            Self::Gray => 1,
            Self::GrayAlpha => 2,
            Self::Rgb => 3,
            Self::Rgba | Self::Bgra => 4,
        }
    }

    fn rgba(self, pixel: &[u8]) -> [u8; 4] {
        match self {
            Self::Gray => [pixel[0], pixel[0], pixel[0], 255],
            Self::GrayAlpha => [pixel[0], pixel[0], pixel[0], pixel[1]],
            Self::Rgb => [pixel[0], pixel[1], pixel[2], 255],
            Self::Rgba => [pixel[0], pixel[1], pixel[2], pixel[3]],
            Self::Bgra => [pixel[2], pixel[1], pixel[0], pixel[3]],
        }
    }
}

/// `data` (`width` x `height` pixels in `layout`) as a wire icon, or `None`
/// when its size is out of bounds or its data is too short for its size.
pub fn pack(width: u32, height: u32, layout: Layout, data: &[u8]) -> Option<wire::Icon> {
    if !(1..=MAX_SOURCE_SIDE).contains(&width) || !(1..=MAX_SOURCE_SIDE).contains(&height) {
        return None;
    }
    let bytes = layout.bytes();
    let needed = (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(bytes)?;
    if data.len() < needed {
        return None;
    }
    // The smallest whole-number reduction that fits: blocks of `factor` by
    // `factor` source pixels become one.
    let factor = width.max(height).div_ceil(wire::MAX_ICON_SIDE).max(1);
    let (out_w, out_h) = (width.div_ceil(factor), height.div_ceil(factor));
    let mut out = Vec::with_capacity((out_w * out_h * 4) as usize);
    for block_y in 0..out_h {
        for block_x in 0..out_w {
            // Average weighted by alpha, so a transparent pixel's colour
            // (often black) does not darken its opaque neighbours.
            let (mut r, mut g, mut b, mut a, mut count) = (0_u32, 0_u32, 0_u32, 0_u32, 0_u32);
            for y in block_y * factor..((block_y + 1) * factor).min(height) {
                for x in block_x * factor..((block_x + 1) * factor).min(width) {
                    let at = (y as usize * width as usize + x as usize) * bytes;
                    let [pr, pg, pb, pa] = layout.rgba(&data[at..at + bytes]);
                    let weight = u32::from(pa);
                    r += u32::from(pr) * weight;
                    g += u32::from(pg) * weight;
                    b += u32::from(pb) * weight;
                    a += weight;
                    count += 1;
                }
            }
            out.extend_from_slice(
                &match (r.checked_div(a), g.checked_div(a), b.checked_div(a)) {
                    (Some(r), Some(g), Some(b)) => [r as u8, g as u8, b as u8, (a / count) as u8],
                    // Nothing in the block was visible at all.
                    _ => [0, 0, 0, 0],
                },
            );
        }
    }
    wire::Icon::pack(out_w, out_h, &out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, pixel: [u8; 4]) -> Vec<u8> {
        pixel
            .iter()
            .copied()
            .cycle()
            .take((width * height * 4) as usize)
            .collect()
    }

    #[test]
    fn a_small_icon_passes_through_unchanged() {
        let data = solid(16, 16, [10, 20, 30, 255]);
        let icon = pack(16, 16, Layout::Rgba, &data).unwrap();
        assert_eq!((icon.width, icon.height), (16, 16));
        assert_eq!(icon.pixels().unwrap(), data);
    }

    #[test]
    fn a_large_icon_is_averaged_down_to_the_limit() {
        for (width, height, expect) in [
            (64, 64, (32, 32)),
            (100, 50, (25, 13)),
            (33, 33, (17, 17)),
            (1024, 1024, (32, 32)),
        ] {
            let icon = pack(
                width,
                height,
                Layout::Rgba,
                &solid(width, height, [9, 9, 9, 255]),
            )
            .unwrap();
            assert_eq!((icon.width, icon.height), expect, "{width}x{height}");
            assert!(icon.width <= wire::MAX_ICON_SIDE && icon.height <= wire::MAX_ICON_SIDE);
            assert!(
                icon.pixels()
                    .unwrap()
                    .chunks(4)
                    .all(|p| p == [9, 9, 9, 255])
            );
        }
    }

    #[test]
    fn every_layout_becomes_straight_rgba() {
        let cases: [(Layout, &[u8], [u8; 4]); 5] = [
            (Layout::Gray, &[7], [7, 7, 7, 255]),
            (Layout::GrayAlpha, &[7, 100], [7, 7, 7, 100]),
            (Layout::Rgb, &[1, 2, 3], [1, 2, 3, 255]),
            (Layout::Rgba, &[1, 2, 3, 4], [1, 2, 3, 4]),
            (Layout::Bgra, &[1, 2, 3, 4], [3, 2, 1, 4]),
        ];
        for (layout, pixel, expect) in cases {
            let icon = pack(1, 1, layout, pixel).unwrap();
            assert_eq!(icon.pixels().unwrap(), expect, "{layout:?}");
        }
    }

    #[test]
    fn a_transparent_pixel_does_not_darken_its_neighbours() {
        // Two pixels, one opaque red and one fully transparent black, averaged
        // together by a 2x reduction of a 64x64 icon's corner block.
        let mut data = solid(64, 64, [0, 0, 0, 0]);
        for pixel in 0..64 * 64 / 2 {
            data[pixel * 8..pixel * 8 + 4].copy_from_slice(&[255, 0, 0, 255]);
        }
        let icon = pack(64, 64, Layout::Rgba, &data).unwrap();
        let first = &icon.pixels().unwrap()[..4];
        // Half coverage, but the colour that is there is still pure red.
        assert_eq!(first[0], 255);
        assert_eq!((first[1], first[2]), (0, 0));
        assert!((120..=135).contains(&first[3]), "alpha {}", first[3]);
    }

    #[test]
    fn a_fully_transparent_block_is_transparent_black() {
        let icon = pack(2, 2, Layout::Rgba, &solid(2, 2, [200, 100, 50, 0])).unwrap();
        assert_eq!(icon.pixels().unwrap()[..4], [0, 0, 0, 0]);
    }

    #[test]
    fn nonsense_sizes_and_short_data_are_refused() {
        assert!(pack(0, 16, Layout::Rgba, &[]).is_none());
        assert!(pack(16, 0, Layout::Rgba, &[]).is_none());
        assert!(pack(MAX_SOURCE_SIDE + 1, 1, Layout::Rgba, &[0; 8]).is_none());
        // Data shorter than its dimensions say.
        assert!(pack(16, 16, Layout::Rgba, &[0; 16 * 16 * 4 - 1]).is_none());
        assert!(pack(4, 4, Layout::Rgb, &[0; 4 * 4 * 3 - 1]).is_none());
        // Data longer than needed is fine: a frame strip's first frame is used.
        assert!(pack(2, 2, Layout::Rgba, &[0; 64]).is_some());
    }
}
