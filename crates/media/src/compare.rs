//! Before/after comparison of two frames.

use crate::frame::Frame;

/// Per-channel difference treated as "changed" (ignores JPEG noise).
const THRESHOLD: u8 = 24;

pub struct Comparison {
    /// The "after" frame with changed pixels tinted red and the rest dimmed.
    pub highlight: Frame,
    /// Share of pixels that changed, 0.0–1.0.
    pub changed: f32,
    /// Bounding box of the changes in "after" pixels: x, y, width, height.
    pub region: Option<(u32, u32, u32, u32)>,
}

/// Compares `before` and `after`. Frames of different sizes are compared
/// over their common area; the rest counts as changed.
pub fn compare(before: &Frame, after: &Frame) -> Comparison {
    let (w, h) = (after.width, after.height);
    let (old, new) = (before.bgra(), after.bgra());
    let mut out = new.to_vec();
    let (mut changed, mut min_x, mut min_y, mut max_x, mut max_y) =
        (0usize, u32::MAX, u32::MAX, 0, 0);
    for y in 0..h {
        for x in 0..w {
            let ix = ((y * w + x) * 4) as usize;
            let differs = if x < before.width && y < before.height {
                let bx = ((y * before.width + x) * 4) as usize;
                (0..3).any(|c| old[bx + c].abs_diff(new[ix + c]) > THRESHOLD)
            } else {
                true
            };
            // BGRA: index 2 is red. Changes turn red, the rest is dimmed.
            let dim = |value: u8| (value as u16 * 2 / 5) as u8;
            if differs {
                changed += 1;
                (min_x, min_y, max_x, max_y) =
                    (min_x.min(x), min_y.min(y), max_x.max(x), max_y.max(y));
                out[ix] = dim(out[ix]);
                out[ix + 1] = dim(out[ix + 1]);
                out[ix + 2] = 255;
            } else {
                for c in 0..3 {
                    out[ix + c] = dim(out[ix + c]);
                }
            }
        }
    }
    let total = (w * h).max(1) as f32;
    Comparison {
        highlight: Frame::from_bgra(w, h, out),
        changed: changed as f32 / total,
        region: (changed > 0).then(|| (min_x, min_y, max_x - min_x + 1, max_y - min_y + 1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_changed_region() {
        let before = Frame::from_rgba(10, 10, vec![0; 400]);
        let mut pixels = vec![0u8; 400];
        for y in 2..4 {
            for x in 5..8 {
                let ix = (y * 10 + x) * 4;
                pixels[ix] = 255;
            }
        }
        let after = Frame::from_rgba(10, 10, pixels);
        let result = compare(&before, &after);
        assert_eq!(result.region, Some((5, 2, 3, 2)));
        assert!((result.changed - 0.06).abs() < 1e-6);
        assert!(compare(&before, &before).region.is_none());
    }
}
