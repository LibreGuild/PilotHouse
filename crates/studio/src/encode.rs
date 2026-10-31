//! Saving a result: its file format, quality and size limit.
//!
//! Every format is written by the `image` crate PilotHouse already has, so
//! nothing more ships. Metadata (camera, place) is never written: a saved
//! picture carries only its pixels.

use std::io::Cursor;

use anyhow::{Context as _, Result, bail};
use image::codecs::{
    bmp::BmpEncoder,
    gif::GifEncoder,
    ico::{IcoEncoder, IcoFrame},
    jpeg::JpegEncoder,
    png::{CompressionType, FilterType, PngEncoder},
    tiff::TiffEncoder,
    webp::WebPEncoder,
};
use image::{ExtendedColorType, ImageEncoder as _, RgbImage, RgbaImage, imageops};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    #[default]
    Png,
    Jpeg,
    Webp,
    Gif,
    Bmp,
    Tiff,
    Ico,
}

impl Format {
    pub const ALL: [Format; 7] = [
        Format::Png,
        Format::Jpeg,
        Format::Webp,
        Format::Gif,
        Format::Bmp,
        Format::Tiff,
        Format::Ico,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Format::Png => "PNG",
            Format::Jpeg => "JPEG",
            Format::Webp => "WebP",
            Format::Gif => "GIF",
            Format::Bmp => "BMP",
            Format::Tiff => "TIFF",
            Format::Ico => "ICO",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpeg => "jpg",
            Format::Webp => "webp",
            Format::Gif => "gif",
            Format::Bmp => "bmp",
            Format::Tiff => "tiff",
            Format::Ico => "ico",
        }
    }

    /// What a format is good for, for its menu.
    pub fn hint(self) -> &'static str {
        match self {
            Format::Png => "Exact; best for screenshots",
            Format::Jpeg => "Smallest for photos",
            Format::Webp => "Exact, and smaller than PNG",
            Format::Gif => "256 colors",
            Format::Bmp => "Uncompressed",
            Format::Tiff => "For print and archives",
            Format::Ico => "Windows and website icons",
        }
    }

    pub fn from_extension(extension: &str) -> Option<Format> {
        Some(match extension.to_ascii_lowercase().as_str() {
            "png" => Format::Png,
            "jpg" | "jpeg" => Format::Jpeg,
            "webp" => Format::Webp,
            "gif" => Format::Gif,
            "bmp" => Format::Bmp,
            "tif" | "tiff" => Format::Tiff,
            "ico" => Format::Ico,
            _ => return None,
        })
    }

    /// Whether the format keeps see-through parts.
    pub fn keeps_transparency(self) -> bool {
        !matches!(self, Format::Jpeg | Format::Bmp)
    }

    /// Whether the quality setting applies.
    pub fn has_quality(self) -> bool {
        self == Format::Jpeg
    }
}

/// How the result is written.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExportSettings {
    pub format: Format,
    /// JPEG quality, 1 to 100.
    pub quality: u8,
    /// The most kilobytes the file may take: quality, then colors, then
    /// size give way until it fits.
    pub max_kb: Option<u32>,
    /// A PNG of at most 256 colors (much smaller for screenshots).
    pub palette: bool,
}

impl Default for ExportSettings {
    fn default() -> Self {
        ExportSettings {
            format: Format::Png,
            quality: 85,
            max_kb: None,
            palette: false,
        }
    }
}

impl ExportSettings {
    /// The settings for a picture read as `format`.
    pub fn for_format(format: Format) -> Self {
        ExportSettings {
            format,
            ..Default::default()
        }
    }

    pub fn sanitize(&mut self) {
        self.quality = self.quality.clamp(1, 100);
        self.max_kb = self.max_kb.filter(|kb| *kb > 0).map(|kb| kb.min(1_000_000));
    }
}

/// A written file.
#[derive(Debug, Clone)]
pub struct Encoded {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// What was given up to meet the size limit ("quality 62", "1280 ×
    /// 720", "256 colors").
    pub note: Option<String>,
}

/// `image` written as `settings` say.
pub fn encode(image: &RgbaImage, settings: &ExportSettings) -> Result<Encoded> {
    let Some(limit) = settings.max_kb.map(|kb| kb as usize * 1024) else {
        return Ok(Encoded {
            bytes: encode_once(image, settings.format, settings.quality, settings.palette)?,
            width: image.width(),
            height: image.height(),
            note: None,
        });
    };
    fit(image, settings, limit)
}

/// Gives way, step by step, until the file is at most `limit` bytes.
fn fit(image: &RgbaImage, settings: &ExportSettings, limit: usize) -> Result<Encoded> {
    let mut picture = std::borrow::Cow::Borrowed(image);
    let mut palette = settings.palette;
    for _ in 0..6 {
        let (bytes, quality) = match settings.format {
            Format::Jpeg => best_quality(&picture, settings.quality, limit)?,
            Format::Png => {
                let bytes = encode_once(&picture, Format::Png, 0, palette)?;
                if bytes.len() > limit && !palette {
                    palette = true;
                    (encode_once(&picture, Format::Png, 0, true)?, None)
                } else {
                    (bytes, None)
                }
            }
            format => (
                encode_once(&picture, format, settings.quality, palette)?,
                None,
            ),
        };
        let mut notes = Vec::new();
        if let Some(quality) = quality.filter(|q| *q != settings.quality) {
            notes.push(format!("quality {quality}"));
        }
        if palette && !settings.palette {
            notes.push("256 colors".into());
        }
        if picture.dimensions() != image.dimensions() {
            notes.push(format!("{} × {}", picture.width(), picture.height()));
        }
        if bytes.len() <= limit {
            return Ok(Encoded {
                width: picture.width(),
                height: picture.height(),
                bytes,
                note: (!notes.is_empty()).then(|| notes.join(", ")),
            });
        }
        // Smaller by the share that is over, and a little more.
        let shrink = ((limit as f64 / bytes.len() as f64).sqrt() * 0.92).clamp(0.3, 0.95);
        let (width, height) = (
            (picture.width() as f64 * shrink).round() as u32,
            (picture.height() as f64 * shrink).round() as u32,
        );
        if width < 16 || height < 16 {
            break;
        }
        picture = std::borrow::Cow::Owned(imageops::resize(
            image,
            width,
            height,
            imageops::FilterType::Lanczos3,
        ));
    }
    bail!(
        "it cannot be made {} KB or smaller; allow more, or choose JPEG",
        limit / 1024
    )
}

/// The best JPEG quality up to `start` that fits in `limit` bytes, or the
/// lowest tried when none does.
fn best_quality(image: &RgbaImage, start: u8, limit: usize) -> Result<(Vec<u8>, Option<u8>)> {
    let first = encode_once(image, Format::Jpeg, start, false)?;
    if first.len() <= limit {
        return Ok((first, Some(start)));
    }
    const LOWEST: u8 = 30;
    let lowest = encode_once(image, Format::Jpeg, LOWEST, false)?;
    if lowest.len() > limit {
        return Ok((lowest, Some(LOWEST)));
    }
    let (mut low, mut high, mut best) = (LOWEST, start, (lowest, LOWEST));
    while high - low > 2 {
        let middle = (low + high) / 2;
        let bytes = encode_once(image, Format::Jpeg, middle, false)?;
        if bytes.len() <= limit {
            best = (bytes, middle);
            low = middle;
        } else {
            high = middle;
        }
    }
    Ok((best.0, Some(best.1)))
}

/// Whether every pixel is opaque.
fn opaque(image: &RgbaImage) -> bool {
    image.pixels().all(|pixel| pixel.0[3] == 255)
}

/// The picture on white, for formats without transparency.
fn flatten(image: &RgbaImage) -> RgbImage {
    RgbImage::from_fn(image.width(), image.height(), |x, y| {
        let [r, g, b, a] = image.get_pixel(x, y).0;
        let a = a as u32;
        let over = |c: u8| ((c as u32 * a + 255 * (255 - a) + 127) / 255) as u8;
        image::Rgb([over(r), over(g), over(b)])
    })
}

fn encode_once(image: &RgbaImage, format: Format, quality: u8, palette: bool) -> Result<Vec<u8>> {
    let (width, height) = image.dimensions();
    let mut out = Vec::new();
    match format {
        Format::Png if palette => return indexed_png(image),
        Format::Png => {
            let encoder =
                PngEncoder::new_with_quality(&mut out, CompressionType::Best, FilterType::Adaptive);
            if opaque(image) {
                encoder.write_image(&flatten(image), width, height, ExtendedColorType::Rgb8)?;
            } else {
                encoder.write_image(image, width, height, ExtendedColorType::Rgba8)?;
            }
        }
        Format::Jpeg => {
            JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100)).write_image(
                &flatten(image),
                width,
                height,
                ExtendedColorType::Rgb8,
            )?;
        }
        Format::Webp => {
            let encoder = WebPEncoder::new_lossless(&mut out);
            if opaque(image) {
                encoder.write_image(&flatten(image), width, height, ExtendedColorType::Rgb8)?;
            } else {
                encoder.write_image(image, width, height, ExtendedColorType::Rgba8)?;
            }
        }
        Format::Gif => {
            let mut encoder = GifEncoder::new_with_speed(&mut out, 10);
            encoder.encode(image, width, height, ExtendedColorType::Rgba8)?;
        }
        Format::Bmp => {
            BmpEncoder::new(&mut out).write_image(
                &flatten(image),
                width,
                height,
                ExtendedColorType::Rgb8,
            )?;
        }
        Format::Tiff => {
            let mut cursor = Cursor::new(&mut out);
            TiffEncoder::new(&mut cursor).write_image(
                image,
                width,
                height,
                ExtendedColorType::Rgba8,
            )?;
        }
        Format::Ico => return ico(image, &[16, 24, 32, 48, 64, 128, 256]),
    }
    Ok(out)
}

/// A PNG of at most 256 colors: often a third of the size for screenshots
/// and diagrams, with little visible change.
fn indexed_png(image: &RgbaImage) -> Result<Vec<u8>> {
    let quantizer = color_quant::NeuQuant::new(10, 256, image.as_raw());
    let map = quantizer.color_map_rgba();
    let indices: Vec<u8> = image
        .pixels()
        .map(|pixel| quantizer.index_of(&pixel.0) as u8)
        .collect();
    let colors = map.len() / 4;
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, image.width(), image.height());
        encoder.set_color(png::ColorType::Indexed);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::High);
        encoder.set_palette(
            map.as_chunks::<4>()
                .0
                .iter()
                .flat_map(|c| [c[0], c[1], c[2]])
                .collect::<Vec<u8>>(),
        );
        if !opaque(image) {
            encoder.set_trns(
                map.as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| c[3])
                    .collect::<Vec<u8>>(),
            );
        }
        let mut writer = encoder.write_header().context("writing the PNG")?;
        writer
            .write_image_data(&indices)
            .context("writing the PNG")?;
        debug_assert!(colors <= 256);
    }
    Ok(out)
}

/// The picture on a square of see-through, centered.
pub(crate) fn square(image: &RgbaImage) -> RgbaImage {
    let side = image.width().max(image.height());
    if image.width() == image.height() {
        return image.clone();
    }
    let mut out = RgbaImage::new(side, side);
    imageops::overlay(
        &mut out,
        image,
        ((side - image.width()) / 2) as i64,
        ((side - image.height()) / 2) as i64,
    );
    out
}

/// An icon file holding the picture at each of `sizes` no larger than it.
pub(crate) fn ico(image: &RgbaImage, sizes: &[u32]) -> Result<Vec<u8>> {
    let picture = square(image);
    let side = picture.width();
    let mut wanted: Vec<u32> = sizes.iter().copied().filter(|size| *size <= side).collect();
    if wanted.is_empty() {
        wanted.push(side.min(256));
    }
    let frames = wanted
        .into_iter()
        .map(|size| {
            let resized = imageops::resize(&picture, size, size, imageops::FilterType::Lanczos3);
            IcoFrame::as_png(resized.as_raw(), size, size, ExtendedColorType::Rgba8)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut out = Vec::new();
    IcoEncoder::new(&mut out).encode_images(&frames)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Noise: a picture no format makes small without giving way.
    fn photo() -> RgbaImage {
        let mut seed = 12_345u32;
        RgbaImage::from_fn(320, 240, |_, _| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let [a, b, c, _] = seed.to_le_bytes();
            image::Rgba([a, b, c, 255])
        })
    }

    /// Scattered pixels of 200 colors, like a busy screenshot.
    fn screenshot() -> RgbaImage {
        let colors: Vec<[u8; 4]> = (0..200u32)
            .map(|i| {
                [
                    (i * 37 % 256) as u8,
                    (i * 91 % 256) as u8,
                    (i * 53 % 256) as u8,
                    255,
                ]
            })
            .collect();
        let mut seed = 99u32;
        RgbaImage::from_fn(320, 240, |_, _| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            image::Rgba(colors[(seed >> 16) as usize % colors.len()])
        })
    }

    #[test]
    fn every_format_writes_a_file_that_reads_back() {
        let picture = photo();
        for format in Format::ALL {
            let encoded = encode(&picture, &ExportSettings::for_format(format)).unwrap();
            let read = image::load_from_memory(&encoded.bytes).unwrap();
            let expected = if format == Format::Ico {
                (256, 256)
            } else {
                (320, 240)
            };
            assert_eq!((read.width(), read.height()), expected, "{format:?}");
        }
    }

    #[test]
    fn a_size_limit_is_met() {
        let picture = photo();
        for format in [Format::Jpeg, Format::Png] {
            let settings = ExportSettings {
                format,
                max_kb: Some(20),
                ..ExportSettings::for_format(format)
            };
            let encoded = encode(&picture, &settings).unwrap();
            assert!(encoded.bytes.len() <= 20 * 1024, "{format:?}");
            assert!(encoded.note.is_some(), "{format:?} says what gave way");
        }
    }

    #[test]
    fn a_palette_png_is_smaller() {
        let picture = screenshot();
        let full = encode(&picture, &ExportSettings::default()).unwrap();
        let palette = encode(
            &picture,
            &ExportSettings {
                palette: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(palette.bytes.len() < full.bytes.len());
        assert!(image::load_from_memory(&palette.bytes).is_ok());
    }

    #[test]
    fn transparency_turns_white_where_it_cannot_be_kept() {
        let picture = RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 0, 0]));
        let encoded = encode(&picture, &ExportSettings::for_format(Format::Jpeg)).unwrap();
        let read = image::load_from_memory(&encoded.bytes).unwrap().to_rgb8();
        assert!(read.get_pixel(1, 1).0[0] > 240);
    }
}
