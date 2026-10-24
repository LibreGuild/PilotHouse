//! One picture from a device or browser, and conversions.
//!
//! Pixels are kept in BGRA, the layout GPUI uploads, inside the very
//! `RenderImage` the pane draws. A frame is converted once, on the thread
//! that decoded it, so showing it on the UI thread costs no copy at all.
//! RGBA is produced only for the rare consumers that need it (PNG export,
//! annotation, recording).

use std::sync::Arc;

use anyhow::{Context as _, Result};
use gpui_kit::RenderImage;
use image::{ImageBuffer, RgbaImage};

/// A BGRA picture. Cheap to clone (the pixels are shared).
#[derive(Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    image: Arc<RenderImage>,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Frame({}x{})", self.width, self.height)
    }
}

impl Frame {
    /// Takes RGBA pixels, swapping them to BGRA in place (no copy).
    pub fn from_rgba(width: u32, height: u32, mut pixels: Vec<u8>) -> Self {
        swap_red_blue(&mut pixels);
        Self::from_bgra(width, height, pixels)
    }

    /// Copies RGBA pixels from a borrowed buffer (a network message).
    pub fn from_rgba_slice(width: u32, height: u32, rgba: &[u8]) -> Self {
        // A bulk copy then an in-place swap beats building pixel by pixel.
        Self::from_rgba(width, height, rgba.to_vec())
    }

    /// Copies RGB pixels (three bytes each), adding an opaque alpha.
    pub fn from_rgb_slice(width: u32, height: u32, rgb: &[u8]) -> Self {
        // Fixed-size chunks on a pre-filled buffer let the compiler vectorize.
        let mut pixels = vec![255u8; rgb.len() / 3 * 4];
        let (out_pixels, _) = pixels.as_chunks_mut::<4>();
        let (in_pixels, _) = rgb.as_chunks::<3>();
        for (out, pixel) in out_pixels.iter_mut().zip(in_pixels) {
            out[0] = pixel[2];
            out[1] = pixel[1];
            out[2] = pixel[0];
        }
        Self::from_bgra(width, height, pixels)
    }

    pub fn from_bgra(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        debug_assert_eq!(pixels.len(), (width as usize) * (height as usize) * 4);
        let buffer: RgbaImage =
            ImageBuffer::from_raw(width, height, pixels).unwrap_or_else(|| ImageBuffer::new(1, 1));
        let (width, height) = buffer.dimensions();
        Self {
            width,
            height,
            image: Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])),
        }
    }

    /// Decodes PNG or JPEG bytes (the format is sniffed).
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let image = image::load_from_memory(bytes)
            .context("decoding image")?
            .to_rgba8();
        let (width, height) = image.dimensions();
        Ok(Self::from_rgba(width, height, image.into_raw()))
    }

    /// The pixels, in BGRA order.
    pub fn bgra(&self) -> &[u8] {
        self.image.as_bytes(0).unwrap_or_default()
    }

    /// The image GPUI draws; shared, not copied.
    pub fn render_image(&self) -> Arc<RenderImage> {
        self.image.clone()
    }

    /// An RGBA copy, for encoding and drawing on.
    pub fn to_image(&self) -> RgbaImage {
        let mut pixels = self.bgra().to_vec();
        swap_red_blue(&mut pixels);
        ImageBuffer::from_raw(self.width, self.height, pixels)
            .unwrap_or_else(|| ImageBuffer::new(1, 1))
    }

    /// PNG bytes. Fast compression: captures are written while the user
    /// waits, and size matters less than speed here.
    pub fn encode_png(&self) -> Result<Vec<u8>> {
        use image::ImageEncoder as _;
        use image::codecs::png::{CompressionType, FilterType, PngEncoder};
        let mut out = Vec::new();
        let image = self.to_image();
        PngEncoder::new_with_quality(&mut out, CompressionType::Fast, FilterType::Adaptive)
            .write_image(
                image.as_raw(),
                self.width,
                self.height,
                image::ExtendedColorType::Rgba8,
            )?;
        Ok(out)
    }

    /// A copy no larger than `max` on its longest side (for recordings).
    pub fn downscaled(&self, max: u32) -> Frame {
        let longest = self.width.max(self.height);
        if longest <= max {
            return self.clone();
        }
        let scale = max as f32 / longest as f32;
        let (width, height) = (
            ((self.width as f32 * scale) as u32).max(1),
            ((self.height as f32 * scale) as u32).max(1),
        );
        // Resampling treats channels alike, so it can run on BGRA directly.
        let source: RgbaImage =
            ImageBuffer::from_raw(self.width, self.height, self.bgra().to_vec())
                .unwrap_or_else(|| ImageBuffer::new(1, 1));
        let resized = image::imageops::resize(
            &source,
            width,
            height,
            image::imageops::FilterType::Triangle,
        );
        Frame::from_bgra(width, height, resized.into_raw())
    }
}

fn swap_red_blue(pixels: &mut [u8]) {
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_round_trip_and_downscale() {
        let rgba: Vec<u8> = (0..4 * 2 * 4).map(|i| i as u8).collect();
        let frame = Frame::from_rgba(4, 2, rgba.clone());
        assert_eq!(&frame.bgra()[..4], &[2, 1, 0, 3]);
        assert_eq!(frame.to_image().into_raw(), rgba);
        let decoded = Frame::decode(&frame.encode_png().unwrap()).unwrap();
        assert_eq!((decoded.width, decoded.height), (4, 2));
        assert_eq!(decoded.bgra(), frame.bgra());
        assert_eq!(Frame::from_rgba_slice(4, 2, &rgba).bgra(), frame.bgra());
        assert_eq!(
            Frame::from_rgb_slice(1, 1, &[10, 20, 30]).bgra(),
            &[30, 20, 10, 255]
        );
        let small = Frame::from_rgba(100, 50, vec![0; 100 * 50 * 4]).downscaled(20);
        assert_eq!((small.width, small.height), (20, 10));
    }
}

#[cfg(test)]
mod bench {
    use super::*;

    #[test]
    #[ignore]
    fn conversion_speed() {
        let (w, h) = (640u32, 1280u32);
        let rgb = vec![7u8; (w * h * 3) as usize];
        let rgba = vec![7u8; (w * h * 4) as usize];
        let time = |name: &str, f: &dyn Fn()| {
            let start = std::time::Instant::now();
            for _ in 0..50 {
                f();
            }
            eprintln!("{name}: {:?}/frame", start.elapsed() / 50);
        };
        time("from_rgb_slice", &|| {
            drop(Frame::from_rgb_slice(w, h, &rgb))
        });
        time("from_rgba_slice", &|| {
            drop(Frame::from_rgba_slice(w, h, &rgba))
        });
        time("old: to_vec + swap", &|| {
            drop(Frame::from_rgba(w, h, rgba.clone()))
        });
    }
}
