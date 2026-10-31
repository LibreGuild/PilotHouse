//! RGBA frames as NV12, the form every Windows H.264 and HEVC encoder takes
//! (hardware and software): video range, BT.709 for HD and BT.601 below,
//! as players assume of video that does not say.

use image::RgbaImage;

/// Bytes an NV12 frame of `width` by `height` takes.
pub(crate) fn nv12_len(width: u32, height: u32) -> usize {
    let (width, height) = (width as usize, height as usize);
    width * height + width.div_ceil(2) * 2 * height.div_ceil(2)
}

/// Whether frames this size are HD (and so BT.709).
pub(crate) fn is_hd(height: u32) -> bool {
    height >= 720
}

/// Writes `image` into `out` as NV12: the Y plane, then U and V
/// interleaved at half size, each 2×2 block's colour averaged. `out` is at
/// least [`nv12_len`] long.
pub(crate) fn nv12(image: &RgbaImage, out: &mut [u8]) {
    let (width, height) = (image.width() as usize, image.height() as usize);
    let chroma_width = width.div_ceil(2);
    assert!(out.len() >= nv12_len(width as u32, height as u32));
    // Weights times 2^14, for R, G and B, video range (Y 16–235, U and V
    // 16–240): BT.709 or BT.601.
    let [y_weights, u_weights, v_weights]: [[i32; 3]; 3] = if is_hd(height as u32) {
        [
            [2992, 10063, 1016],
            [-1649, -5548, 7197],
            [7197, -6537, -660],
        ]
    } else {
        [
            [4207, 8260, 1604],
            [-2429, -4768, 7197],
            [7197, -6026, -1171],
        ]
    };
    let pixels = image.as_raw();
    let (luma, chroma) = out.split_at_mut(width * height);
    for (row, line) in luma.chunks_exact_mut(width).enumerate() {
        let source = &pixels[row * width * 4..(row + 1) * width * 4];
        for (y, rgba) in line.iter_mut().zip(source.as_chunks::<4>().0) {
            let value = y_weights[0] * rgba[0] as i32
                + y_weights[1] * rgba[1] as i32
                + y_weights[2] * rgba[2] as i32;
            *y = (16 + ((value + 8192) >> 14)) as u8;
        }
    }
    for (block_row, line) in chroma
        .chunks_exact_mut(chroma_width * 2)
        .take(height.div_ceil(2))
        .enumerate()
    {
        let top = block_row * 2;
        let bottom = (top + 1).min(height - 1);
        for (block, uv) in line.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            let left = block * 2;
            let right = (left + 1).min(width - 1);
            let mut sum = [0i32; 3];
            for (row, column) in [(top, left), (top, right), (bottom, left), (bottom, right)] {
                let at = (row * width + column) * 4;
                for (total, value) in sum.iter_mut().zip(&pixels[at..at + 3]) {
                    *total += *value as i32;
                }
            }
            // The sums are four pixels': the weights' shift takes two more.
            let weigh = |weights: [i32; 3]| {
                let value = weights[0] * sum[0] + weights[1] * sum[1] + weights[2] * sum[2];
                (128 + ((value + (1 << 15)) >> 16)).clamp(16, 240) as u8
            };
            uv[0] = weigh(u_weights);
            uv[1] = weigh(v_weights);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn convert(color: [u8; 3], height: u32) -> (u8, u8, u8) {
        let image =
            RgbaImage::from_pixel(4, height, image::Rgba([color[0], color[1], color[2], 255]));
        let mut out = vec![0; nv12_len(4, height)];
        nv12(&image, &mut out);
        let chroma = 4 * height as usize;
        (out[0], out[chroma], out[chroma + 1])
    }

    #[test]
    fn known_colours() {
        // HD (BT.709).
        assert_eq!(convert([255, 255, 255], 720), (235, 128, 128));
        assert_eq!(convert([0, 0, 0], 720), (16, 128, 128));
        assert_eq!(convert([255, 0, 0], 720), (63, 102, 240));
        assert_eq!(convert([0, 0, 255], 720), (32, 240, 118));
        // SD (BT.601).
        assert_eq!(convert([255, 0, 0], 480), (81, 90, 240));
        assert_eq!(convert([0, 255, 0], 480), (145, 54, 34));
    }

    #[test]
    fn odd_sizes_fit() {
        let image = RgbaImage::from_pixel(5, 3, image::Rgba([10, 20, 30, 255]));
        let mut out = vec![0; nv12_len(5, 3)];
        assert_eq!(out.len(), 15 + 6 * 2);
        nv12(&image, &mut out);
        assert!(out.iter().all(|value| *value >= 16));
    }
}
